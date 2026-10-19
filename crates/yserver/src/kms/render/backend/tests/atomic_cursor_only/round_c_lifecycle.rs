//! Blocking lifecycle disables preserve cursor owners through old-event drain.

use super::{
    round_a::damage_cursor_root,
    round_b_producers::{begin_hw_cursor, retire_cursor_only},
    support::AtomicCursorFixture,
};
use crate::{
    drm::atomic::{AtomicCommitKind, tests_support::ScriptedResult},
    kms::render::platform::cursor_state::CursorBinding,
};
use ::drm::control::AtomicCommitFlags;
use std::{cell::RefCell, rc::Rc};
use yserver_core::backend::Backend;

fn cursor_properties(
    fixture: &AtomicCursorFixture,
    output_idx: usize,
) -> crate::kms::cursor_plane::topology::CursorPlaneProperties {
    let output = &fixture.backend.platform.outputs[output_idx];
    fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(output.output.crtc))
        .expect("fixture output has atomic cursor properties")
}

fn assert_output_off_records(
    fixture: &AtomicCursorFixture,
    records: &[crate::drm::atomic::tests_support::AtomicCommitRecord],
    records_from: usize,
    outputs: &[crate::platform::drm::Output],
) {
    let blocking_off_records: Vec<_> = records[records_from..]
        .iter()
        .filter(|record| {
            record.kind == AtomicCommitKind::Scene
                && record.flags == AtomicCommitFlags::ALLOW_MODESET
                && record.events.is_empty()
                && record.properties.iter().all(|update| update.value == 0)
                && outputs.iter().any(|output| {
                    let primary: ::drm::control::plane::Handle = output.plane;
                    record.properties.iter().any(|update| {
                        update.object == primary.into()
                            && update.property == output.plane_fb_id_prop
                            && update.value == 0
                    })
                })
        })
        .collect();
    assert_eq!(
        blocking_off_records.len(),
        outputs.len(),
        "one blocking all-off record is accepted per output in topology order",
    );
    for (off, output) in blocking_off_records.into_iter().zip(outputs) {
        let primary: ::drm::control::plane::Handle = output.plane;
        assert!(off.properties.iter().any(|update| {
            update.object == primary.into()
                && update.property == output.plane_fb_id_prop
                && update.value == 0
        }));
        let cursor = fixture.backend.platform.devices.iter().find_map(|device| {
            device
                .cursor
                .plane
                .as_ref()
                .and_then(|plane| plane.properties_for(output.crtc))
        });
        assert_blocking_output_off(off, output, cursor);
    }
}

fn assert_all_cursor_state_quiescent(fixture: &AtomicCursorFixture) {
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    for device in &fixture.backend.platform.devices {
        assert!(device.cursor.crtc_states.values().all(|state| {
            state.applied.is_none()
                && state.lifecycle_pending.is_none()
                && state.intent.is_none()
                && state.in_flight.is_none()
        }));
        assert!(
            device
                .cursor
                .plane
                .as_ref()
                .is_none_or(|plane| plane.sprite_pool_users_are_zero())
        );
    }
}

fn queue_flip_for_output(fixture: &AtomicCursorFixture, output_idx: usize, frame: u32) {
    let output = &fixture.backend.platform.outputs[output_idx];
    let device = &fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .expect("fixture output device")
        .device;
    fixture
        .recorders
        .page_flip_events
        .queue_flip_for_device(device, output.output.crtc, frame);
}

fn make_devices_pollable_for_run_core(
    fixture: &mut AtomicCursorFixture,
) -> Vec<std::os::unix::net::UnixStream> {
    let mut peers = Vec::new();
    for (index, device) in fixture.backend.platform.devices.iter_mut().enumerate() {
        let (device_fd, peer) =
            std::os::unix::net::UnixStream::pair().expect("create pollable fixture DRM fd");
        let replacement = Rc::new(crate::drm::Device::from_inherited_kms_fd(
            std::os::fd::OwnedFd::from(device_fd),
            format!("<atomic cursor lifecycle device {index}>"),
        ));
        fixture
            .recorders
            .page_flip_events
            .register_device(&replacement);
        device.device = replacement;
        peers.push(peer);
    }
    peers
}

pub(super) fn assert_blocking_detach(
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    output: &crate::drm::modeset::Output,
    cursor: crate::kms::cursor_plane::topology::CursorPlaneProperties,
) {
    assert_blocking_output_off(record, output, Some(cursor));
}

fn assert_blocking_output_off(
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    output: &crate::drm::modeset::Output,
    cursor: Option<crate::kms::cursor_plane::topology::CursorPlaneProperties>,
) {
    assert_eq!(record.kind, AtomicCommitKind::Scene);
    assert_eq!(record.flags, AtomicCommitFlags::ALLOW_MODESET);
    assert!(
        record.events.is_empty(),
        "all-off commits have no replacement event"
    );
    assert!(record.properties.iter().all(|update| update.value == 0));
    if let Some(cursor) = cursor {
        let plane: ::drm::control::plane::Handle = cursor.plane;
        assert!(record.properties.iter().any(|update| {
            update.object == plane.into() && update.property == cursor.fb_id && update.value == 0
        }));
        assert!(record.properties.iter().any(|update| {
            update.object == plane.into() && update.property == cursor.crtc_id && update.value == 0
        }));
    }

    let primary: ::drm::control::plane::Handle = output.plane;
    assert!(
        record.properties.iter().any(|update| {
            update.object == primary.into()
                && update.property == output.plane_fb_id_prop
                && update.value == 0
        }),
        "blocking lifecycle request disables the primary FB_ID"
    );
    assert!(
        record.properties.iter().any(|update| {
            update.object == primary.into()
                && update.property == output.plane_crtc_id_prop
                && update.value == 0
        }),
        "blocking lifecycle request detaches the primary CRTC_ID"
    );

    let crtc: ::drm::control::crtc::Handle = output.crtc;
    let active = ::drm::control::from_u32(0xD002).expect("recorded CRTC ACTIVE property");
    let mode_id = ::drm::control::from_u32(0xD003).expect("recorded CRTC MODE_ID property");
    assert!(
        record.properties.iter().any(|update| {
            update.object == crtc.into() && update.property == active && update.value == 0
        }),
        "blocking lifecycle request disables CRTC ACTIVE"
    );
    assert!(
        record.properties.iter().any(|update| {
            update.object == crtc.into() && update.property == mode_id && update.value == 0
        }),
        "blocking lifecycle request clears CRTC MODE_ID"
    );
    let connector: ::drm::control::connector::Handle = output.connector;
    let connector_crtc_id =
        ::drm::control::from_u32(0xD001).expect("recorded connector CRTC_ID property");
    assert!(
        record.properties.iter().any(|update| {
            update.object == connector.into()
                && update.property == connector_crtc_id
                && update.value == 0
        }),
        "blocking lifecycle request detaches the connector"
    );
}

fn assert_primary_frame(
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    output: &crate::drm::modeset::Output,
) {
    assert_eq!(
        record.result,
        ScriptedResult::Ok,
        "primary frame is accepted"
    );
    let primary: ::drm::control::plane::Handle = output.plane;
    let crtc_id_value = u64::from(u32::from(output.crtc));
    let fb_ids = [
        output.plane_fb_id_prop,
        ::drm::control::from_u32(0xD201).expect("recorded modeset primary FB_ID property"),
    ];
    assert!(
        record.properties.iter().any(|update| {
            update.object == primary.into()
                && fb_ids.contains(&update.property)
                && update.value != 0
        }),
        "accepted scene frame has a primary framebuffer: {record:?}"
    );
    let crtc_ids = [
        output.plane_crtc_id_prop,
        ::drm::control::from_u32(0xD202).expect("recorded modeset primary CRTC_ID property"),
    ];
    assert!(
        record.properties.iter().any(|update| {
            update.object == primary.into()
                && crtc_ids.contains(&update.property)
                && update.value == crtc_id_value
        }),
        "accepted scene frame is assigned to its output CRTC"
    );
    if record.flags == AtomicCommitFlags::ALLOW_MODESET {
        let crtc: ::drm::control::crtc::Handle = output.crtc;
        let active =
            ::drm::control::from_u32(0xD101).expect("recorded modeset CRTC ACTIVE property");
        let mode_id =
            ::drm::control::from_u32(0xD102).expect("recorded modeset CRTC MODE_ID property");
        assert!(
            record.properties.iter().any(|update| {
                update.object == crtc.into() && update.property == active && update.value == 1
            }),
            "accepted modeset frame activates its CRTC"
        );
        assert!(
            record.properties.iter().any(|update| {
                update.object == crtc.into() && update.property == mode_id && update.value != 0
            }),
            "accepted modeset frame carries a mode blob"
        );
    }
}

fn set_distinct_primary_properties(fixture: &mut AtomicCursorFixture) {
    for (output_idx, layout) in fixture.backend.platform.outputs.iter_mut().enumerate() {
        let base = 0x7100 + u32::try_from(output_idx).expect("output index fits") * 16;
        let property = |offset| ::drm::control::from_u32(base + offset).expect("test property ID");
        layout.output.plane_fb_id_prop = property(0);
        layout.output.plane_crtc_id_prop = property(1);
        layout.output.plane_src_x_prop = property(2);
        layout.output.plane_src_y_prop = property(3);
        layout.output.plane_src_w_prop = property(4);
        layout.output.plane_src_h_prop = property(5);
        layout.output.plane_crtc_x_prop = property(6);
        layout.output.plane_crtc_y_prop = property(7);
        layout.output.plane_crtc_w_prop = property(8);
        layout.output.plane_crtc_h_prop = property(9);
    }
}

fn assert_full_attach(
    backend: &crate::kms::render::backend::KmsBackend,
    output_idx: usize,
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    binding: CursorBinding,
) {
    let output = &backend.platform.outputs[output_idx];
    let cursor = backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(output.output.crtc))
        .expect("fixture output has atomic cursor properties");
    let plane: ::drm::control::plane::Handle = cursor.plane;
    let device = backend
        .platform
        .device_for_key(output.key.device_key)
        .expect("cursor device");
    let pool = device.cursor.plane.as_ref().expect("cursor pool");
    let width = u64::from(pool.width());
    let height = u64::from(pool.height());
    let mut expected = vec![
        (cursor.fb_id, u64::from(binding.framebuffer)),
        (cursor.crtc_id, u64::from(u32::from(output.output.crtc))),
        (cursor.crtc_x, binding.x as i64 as u64),
        (cursor.crtc_y, binding.y as i64 as u64),
        (cursor.src_x, 0),
        (cursor.src_y, 0),
        (cursor.src_w, width << 16),
        (cursor.src_h, height << 16),
        (cursor.crtc_w, width),
        (cursor.crtc_h, height),
    ];
    if let Some(hotspot_x) = cursor.hotspot_x {
        expected.push((hotspot_x, u64::from(binding.hot_x)));
    }
    if let Some(hotspot_y) = cursor.hotspot_y {
        expected.push((hotspot_y, u64::from(binding.hot_y)));
    }
    assert_eq!(
        record.result,
        ScriptedResult::Ok,
        "cursor attach is accepted"
    );
    assert_eq!(
        record
            .properties
            .iter()
            .filter(|update| update.object == plane.into())
            .count(),
        expected.len()
    );
    assert_cursor_binding_properties(
        record,
        cursor,
        output.output.crtc,
        binding,
        width,
        height,
        true,
    );
}

fn assert_cursor_binding_properties(
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    cursor: crate::kms::cursor_plane::topology::CursorPlaneProperties,
    crtc: ::drm::control::crtc::Handle,
    binding: CursorBinding,
    width: u64,
    height: u64,
    require_all: bool,
) {
    let plane: ::drm::control::plane::Handle = cursor.plane;
    let mut expected = vec![
        (cursor.fb_id, u64::from(binding.framebuffer)),
        (cursor.crtc_id, u64::from(u32::from(crtc))),
        (cursor.crtc_x, binding.x as i64 as u64),
        (cursor.crtc_y, binding.y as i64 as u64),
        (cursor.src_x, 0),
        (cursor.src_y, 0),
        (cursor.src_w, width << 16),
        (cursor.src_h, height << 16),
        (cursor.crtc_w, width),
        (cursor.crtc_h, height),
    ];
    if let Some(hotspot_x) = cursor.hotspot_x {
        expected.push((hotspot_x, u64::from(binding.hot_x)));
    }
    if let Some(hotspot_y) = cursor.hotspot_y {
        expected.push((hotspot_y, u64::from(binding.hot_y)));
    }
    for (property, value) in expected {
        let update = record
            .properties
            .iter()
            .find(|update| update.object == plane.into() && update.property == property);
        if require_all || update.is_some() {
            assert_eq!(update.map(|update| update.value), Some(value));
        }
    }
}

fn assert_scene_frame(
    backend: &crate::kms::render::backend::KmsBackend,
    output_idx: usize,
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
) {
    assert_primary_frame(record, &backend.platform.outputs[output_idx].output);
    let output = &backend.platform.outputs[output_idx];
    let Some(cursor) = backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(output.output.crtc))
    else {
        assert_eq!(record.kind, AtomicCommitKind::Scene);
        return;
    };
    let plane: ::drm::control::plane::Handle = cursor.plane;
    let cursor_updates: Vec<_> = record
        .properties
        .iter()
        .filter(|update| update.object == plane.into())
        .collect();
    if record.kind == AtomicCommitKind::SceneWithCursor {
        // Mutation: classify a cursorless scene as SceneWithCursor; this assertion must reject it.
        assert!(
            !cursor_updates.is_empty(),
            "SceneWithCursor must carry cursor-plane updates: {record:?}"
        );
    }
    let binding = backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
        .and_then(|state| state.applied);
    match (record.kind, binding) {
        (AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor, Some(binding)) => {
            if !cursor_updates.is_empty() {
                let device = backend
                    .platform
                    .device_for_key(output.key.device_key)
                    .expect("cursor device");
                let pool = device.cursor.plane.as_ref().expect("cursor pool");
                assert_cursor_binding_properties(
                    record,
                    cursor,
                    output.output.crtc,
                    binding,
                    u64::from(pool.width()),
                    u64::from(pool.height()),
                    true,
                );
            }
        }
        (AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor, None)
            if !cursor_updates.is_empty() =>
        {
            assert!(
                cursor_updates
                    .iter()
                    .any(|update| { update.property == cursor.fb_id && update.value == 0 }),
                "scene frame without applied cursor must explicitly detach FB_ID"
            );
            assert!(
                cursor_updates
                    .iter()
                    .any(|update| { update.property == cursor.crtc_id && update.value == 0 }),
                "scene frame without applied cursor must explicitly detach CRTC_ID"
            );
            assert!(cursor_updates.iter().all(|update| update.value == 0));
        }
        (AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor, None) => {}
        _ => panic!("not a scene frame: {record:?}"),
    }
}

fn assert_accepted_cursor_only_record(
    backend: &crate::kms::render::backend::KmsBackend,
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
) {
    assert_eq!(record.result, ScriptedResult::Ok);
    assert_eq!(record.kind, AtomicCommitKind::CursorOnly);
    assert_eq!(record.events.len(), 1, "cursor-only commit owns one event");
    let output_idx = (0..backend.platform.outputs.len())
        .find(|&output_idx| {
            backend
                .platform
                .outputs
                .get(output_idx)
                .is_some_and(|output| {
                    backend
                        .platform
                        .device_for_key(output.key.device_key)
                        .and_then(|device| device.cursor.plane.as_ref())
                        .and_then(|plane| plane.properties_for(output.output.crtc))
                        .is_some_and(|cursor| {
                            record.properties.iter().any(|update| {
                                update.object == cursor.plane.into()
                                    && update.property == cursor.fb_id
                            })
                        })
                })
        })
        .expect("accepted cursor-only record names a fixture cursor plane");
    let output = &backend.platform.outputs[output_idx];
    let cursor = backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(output.output.crtc))
        .expect("cursor properties for accepted cursor-only record");
    let plane: ::drm::control::plane::Handle = cursor.plane;
    let state = backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
        .expect("cursor state for accepted cursor-only record");
    if let Some(binding) = state.applied {
        let full_attach = record.properties.iter().any(|update| {
            update.object == plane.into()
                && update.property == cursor.crtc_id
                && update.value == u64::from(u32::from(output.output.crtc))
        });
        let pool = backend
            .platform
            .device_for_key(output.key.device_key)
            .and_then(|device| device.cursor.plane.as_ref())
            .expect("cursor pool");
        assert_cursor_binding_properties(
            record,
            cursor,
            output.output.crtc,
            binding,
            u64::from(pool.width()),
            u64::from(pool.height()),
            full_attach,
        );
        assert!(record.properties.iter().any(|update| {
            update.object == plane.into()
                && update.property == cursor.fb_id
                && update.value == u64::from(binding.framebuffer)
        }));
        assert!(record.properties.iter().any(|update| {
            update.object == plane.into()
                && update.property == cursor.crtc_id
                && update.value == u64::from(u32::from(output.output.crtc))
        }));
    } else {
        assert!(record.properties.iter().any(|update| {
            update.object == plane.into() && update.property == cursor.fb_id && update.value == 0
        }));
        assert!(record.properties.iter().any(|update| {
            update.object == plane.into() && update.property == cursor.crtc_id && update.value == 0
        }));
        assert!(record.properties.iter().all(|update| update.value == 0));
    }
}

fn attach_cursor_on_every_output(fixture: &mut AtomicCursorFixture) {
    set_distinct_primary_properties(fixture);
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(fixture);
    for attempt in 0..16_u32 {
        let before = fixture.recorders.atomic.records().len();
        Backend::maybe_composite(&mut fixture.backend).expect("composite visible cursor");
        let records = fixture.recorders.atomic.records();
        for record in &records[before..] {
            if record.flags.contains(AtomicCommitFlags::TEST_ONLY) {
                continue;
            }
            if matches!(
                record.kind,
                AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
            ) && record.result == ScriptedResult::Ok
            {
                let output_idx = (0..fixture.backend.platform.outputs.len())
                    .find(|&output_idx| {
                        let output = &fixture.backend.platform.outputs[output_idx].output;
                        record.properties.iter().any(|update| {
                            update.object == output.plane.into()
                                && update.property == output.plane_fb_id_prop
                        })
                    })
                    .expect("accepted primary frame belongs to a fixture output");
                assert_scene_frame(&fixture.backend, output_idx, record);
                assert_eq!(record.events.len(), 1, "accepted scene frame has one event");
            } else if record.kind == AtomicCommitKind::CursorOnly
                && record.result == ScriptedResult::Ok
            {
                assert_accepted_cursor_only_record(&fixture.backend, record);
            }
        }
        for output_idx in 0..fixture.backend.platform.outputs.len() {
            if fixture.backend.scene.has_pending_page_flip(output_idx)
                || fixture
                    .backend
                    .platform
                    .cursor_commit_in_flight_for_output(output_idx)
            {
                fixture.retire_output(output_idx, attempt * 4 + output_idx as u32 + 1);
            }
        }
        if (0..fixture.backend.platform.outputs.len()).all(|output_idx| {
            fixture
                .backend
                .platform
                .cursor_plane_visible_for_output(output_idx)
                && !fixture.backend.scene.has_pending_page_flip(output_idx)
                && !fixture
                    .backend
                    .platform
                    .cursor_commit_in_flight_for_output(output_idx)
        }) {
            return;
        }
    }
    panic!("visible cursor did not settle on every output");
}

fn queue_cursor_move_for_lifecycle(fixture: &mut AtomicCursorFixture, frame: u32) {
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 130, 110);
    Backend::maybe_composite(&mut fixture.backend).expect("submit lifecycle-bound cursor move");
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    queue_flip_for_output(fixture, 0, frame);
}

fn shared_pool_partial_disable_keeps_peer_user() {
    let Some(mut fixture) = AtomicCursorFixture::new_with_overlapping_outputs()
        .expect("build shared cursor framebuffer fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    attach_cursor_on_every_output(&mut fixture);
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    assert_eq!(
        fixture.backend.platform.outputs[1].key.device_key,
        device_key
    );
    let bindings: Vec<_> = (0..2)
        .map(|output_idx| {
            let output = &fixture.backend.platform.outputs[output_idx];
            fixture
                .backend
                .platform
                .device_for_key(device_key)
                .unwrap()
                .cursor
                .crtc_states[&output.output.crtc]
                .applied
                .expect("both CRTCs share the visible sprite")
        })
        .collect();
    assert_eq!(bindings[0].lease, bindings[1].lease);
    assert_eq!(bindings[0].framebuffer, bindings[1].framebuffer);
    let pool = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .plane
        .as_ref()
        .unwrap();
    assert_eq!(pool.sprite_user_count(bindings[0].lease), Some(2));

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 110, 100);
    Backend::maybe_composite(&mut fixture.backend).expect("submit shared-FB cursor moves");
    let crtc_ids: Vec<_> = (0..2)
        .map(|output_idx| {
            let output = &fixture.backend.platform.outputs[output_idx];
            assert!(
                fixture
                    .backend
                    .platform
                    .cursor_atomic_event_pending_for_output(output_idx)
            );
            queue_flip_for_output(&fixture, output_idx, 70 + output_idx as u32);
            let state = &fixture
                .backend
                .platform
                .device_for_key(device_key)
                .unwrap()
                .cursor
                .crtc_states[&output.output.crtc];
            assert_eq!(
                state.applied.map(|binding| binding.framebuffer),
                Some(bindings[output_idx].framebuffer)
            );
            output.output.crtc
        })
        .collect();
    fixture.recorders.atomic.script_result(Ok(()));
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    let error = Backend::shutdown_outputs(&mut fixture.backend)
        .expect_err("peer CRTC disable failure is surfaced");
    assert!(
        error
            .to_string()
            .contains("disable_output atomic commit rejected")
    );
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 2);

    let state_a = &fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc_ids[0]];
    assert_eq!(state_a.applied, None);
    assert!(state_a.lifecycle_pending.is_some());
    assert!(state_a.in_flight.is_some());
    let state_b = &fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc_ids[1]];
    assert!(
        state_b.applied.is_some(),
        "failed CRTC keeps its accepted cursor"
    );
    assert!(state_b.lifecycle_pending.is_none());
    assert!(state_b.in_flight.is_some());
    assert!(
        fixture
            .backend
            .platform
            .device_for_key(device_key)
            .unwrap()
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(bindings[0].lease)
            .unwrap_or(0)
            > 1,
        "A's successful disable cannot reclaim shared F0 while B owns it",
    );

    Backend::shutdown_outputs(&mut fixture.backend).expect("all-off retry drains both CRTCs");
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    assert_eq!(
        fixture
            .backend
            .platform
            .device_for_key(device_key)
            .unwrap()
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(bindings[0].lease),
        Some(0),
    );
    fixture.assert_cursor_quiescent();
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorPolicyRow {
    Hardware,
    Software,
    Hidden,
}

struct LifecycleVtResume;

impl crate::kms::render::backend::kms::session::VtResumeExecutor for LifecycleVtResume {
    fn acquire_master(
        &self,
        _device: &crate::kms::render::platform::KmsDevice,
    ) -> std::io::Result<()> {
        Ok(())
    }

    fn release_master(
        &self,
        _device: &crate::kms::render::platform::KmsDevice,
    ) -> std::io::Result<()> {
        Ok(())
    }

    fn probe_connector_snapshot(
        &self,
        platform: &crate::kms::render::platform::PlatformBackend,
    ) -> std::io::Result<Vec<crate::kms::render::platform::ConnectorSnapshot>> {
        Ok(platform
            .outputs
            .iter()
            .map(|output| crate::kms::render::platform::ConnectorSnapshot {
                key: output.key.clone(),
                modes: vec![output.output.picked.clone()],
                mm_width: 0,
                mm_height: 0,
                edid: Vec::new(),
                connector_type: "test".to_owned(),
            })
            .collect())
    }
}

fn new_policy_fixture(policy: CursorPolicyRow) -> Option<AtomicCursorFixture> {
    let fixture = if policy == CursorPolicyRow::Software {
        AtomicCursorFixture::new_with_two_outputs_software_cursor()
    } else {
        AtomicCursorFixture::new_with_two_outputs()
    };
    fixture.expect("build lifecycle policy fixture")
}

fn inspect_accepted_lifecycle_records(fixture: &AtomicCursorFixture, records_from: usize) {
    let records = fixture.recorders.atomic.records();
    inspect_accepted_lifecycle_record_slice(fixture, &records[records_from..]);
}

fn inspect_accepted_lifecycle_record_slice(
    fixture: &AtomicCursorFixture,
    records: &[crate::drm::atomic::tests_support::AtomicCommitRecord],
) {
    for record in records {
        if record.flags.contains(AtomicCommitFlags::TEST_ONLY)
            || record.result != ScriptedResult::Ok
        {
            continue;
        }
        match record.kind {
            AtomicCommitKind::CursorOnly => {
                assert_accepted_cursor_only_record(&fixture.backend, record);
            }
            AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor => {
                let output_idx = (0..fixture.backend.platform.outputs.len()).find(|&output_idx| {
                    let output = &fixture.backend.platform.outputs[output_idx].output;
                    record.properties.iter().any(|update| {
                        update.object == output.plane.into()
                            && update.property == output.plane_fb_id_prop
                            && update.value != 0
                    })
                });
                if let Some(output_idx) = output_idx {
                    assert_scene_frame(&fixture.backend, output_idx, record);
                }
            }
            AtomicCommitKind::Async => {
                panic!("lifecycle restoration must not submit an async frame: {record:?}");
            }
        }
    }
}

fn drain_policy_fixture_events(fixture: &mut AtomicCursorFixture, first_frame: u32) {
    for offset in 0..16_u32 {
        let pending: Vec<_> = (0..fixture.backend.platform.outputs.len())
            .filter(|&output_idx| {
                fixture.backend.scene.has_pending_page_flip(output_idx)
                    || fixture
                        .backend
                        .platform
                        .cursor_commit_in_flight_for_output(output_idx)
            })
            .collect();
        if pending.is_empty() {
            return;
        }
        for (index, output_idx) in pending.into_iter().enumerate() {
            fixture.retire_output(
                output_idx,
                first_frame + offset * 8 + u32::try_from(index).expect("small output index"),
            );
        }
    }
    panic!("policy fixture events did not drain");
}

fn assert_policy_row(fixture: &mut AtomicCursorFixture, policy: CursorPolicyRow) {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPlaneMode, CursorPrimaryContent};

    assert!(!fixture.backend.platform.outputs.is_empty());
    let pointer = fixture.backend.platform.cursor_pointer_root;
    let mut software_pixel_found = false;
    for output_idx in 0..fixture.backend.platform.outputs.len() {
        let receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(output_idx)
            .expect("lifecycle has an accepted cursor frame per live output");
        let output = &fixture.backend.platform.outputs[output_idx];
        let cursor_intersects = pointer.0 < output.x + i32::from(output.width)
            && pointer.0 + 4 > output.x
            && pointer.1 < output.y + i32::from(output.height)
            && pointer.1 + 4 > output.y;
        match policy {
            CursorPolicyRow::Hardware => {
                assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Hw);
                assert_eq!(receipt.mode, CursorAcceptedMode::Hardware);
                assert_eq!(receipt.primary_cursor, CursorPrimaryContent::Cursorless);
                assert_eq!(
                    fixture
                        .backend
                        .platform
                        .cursor_plane_visible_for_output(output_idx),
                    cursor_intersects,
                    "HW plane visibility matches the cursor footprint on output {output_idx}",
                );
                if cursor_intersects {
                    let binding = fixture
                        .backend
                        .platform
                        .device_for_key(output.key.device_key)
                        .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
                        .and_then(|state| state.applied)
                        .expect("HW cursor has an accepted plane binding");
                    assert_eq!(binding.x, pointer.0 - output.x);
                    assert_eq!(binding.y, pointer.1 - output.y);
                    assert_eq!((binding.hot_x, binding.hot_y), (0, 0));
                    assert!(
                        fixture
                            .backend
                            .platform
                            .device_for_key(output.key.device_key)
                            .and_then(|device| device.cursor.plane.as_ref())
                            .and_then(|pool| pool.sprite_user_count(binding.lease))
                            .is_some_and(|users| users > 0),
                        "HW cursor's accepted sprite still has a live owner",
                    );
                }
            }
            CursorPolicyRow::Software => {
                assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Sw);
                assert_eq!(receipt.mode, CursorAcceptedMode::Software);
                assert!(matches!(
                    receipt.primary_cursor,
                    CursorPrimaryContent::Software { .. }
                ));
                assert!(!fixture.backend.platform.any_cursor_plane_visible());
                if cursor_intersects {
                    let x = pointer.0.max(output.x);
                    let y = pointer.1.max(output.y) + 1;
                    let pixel = super::round_c_frames::read_output_pixel(&mut *fixture, x, y);
                    software_pixel_found |= pixel == [0, 0, 255, 255];
                }
            }
            CursorPolicyRow::Hidden => {
                assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Hidden);
                assert_eq!(receipt.mode, CursorAcceptedMode::Hidden);
                assert_eq!(receipt.primary_cursor, CursorPrimaryContent::Cursorless);
                assert!(!fixture.backend.platform.any_cursor_plane_visible());
                assert!(
                    fixture
                        .backend
                        .platform
                        .device_for_key(output.key.device_key)
                        .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
                        .is_none_or(|state| state.applied.is_none())
                );
            }
        }
    }
    if policy == CursorPolicyRow::Software {
        assert!(
            software_pixel_found,
            "software cursor's red pixel reaches scanout"
        );
    }
}

fn drive_policy_frame(fixture: &mut AtomicCursorFixture, policy: CursorPolicyRow, frame: u32) {
    use yserver_core::backend::Backend;

    for attempt in 0..8_u32 {
        let before = fixture.recorders.atomic.records().len();
        Backend::mark_dirty(&mut fixture.backend);
        Backend::maybe_composite(&mut fixture.backend).expect("compose lifecycle cursor policy");
        inspect_accepted_lifecycle_records(fixture, before);
        drain_policy_fixture_events(fixture, frame + attempt * 20);
        if !fixture.backend.for_tests_scene_wants_scene()
            && !fixture.backend.scene.has_pending_page_flips()
            && !fixture.backend.platform.any_cursor_commit_in_flight()
        {
            break;
        }
    }
    assert_policy_row(fixture, policy);
}

fn configure_policy(fixture: &mut AtomicCursorFixture, policy: CursorPolicyRow) {
    use yserver_core::backend::Backend;

    let root = fixture.backend.core.window_id;
    let cursor = super::round_b_producers::create_argb_cursor(
        &mut fixture.backend,
        [0x00, 0x00, 0xff, 0xff],
        0,
        0,
    );
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("define lifecycle matrix cursor through Backend");
    if policy == CursorPolicyRow::Hidden {
        Backend::set_cursor_hidden(&mut fixture.backend, true);
    }
    let pointer_x = if fixture.backend.platform.outputs.len() > 1 {
        798
    } else {
        100
    };
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, pointer_x, 90);
    damage_cursor_root(fixture);
    drive_policy_frame(fixture, policy, 100);
}

fn new_tearing_lifecycle_fixture(
    window: u32,
    first_present: u64,
) -> Option<(
    AtomicCursorFixture,
    Rc<crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor>,
)> {
    let (backend, recorder, state) =
        super::super::tearing::live_tearing_cursor_backend(window, first_present)?;
    let recorders = Rc::clone(
        backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("Tearing fixture uses recording services"),
    );
    let mut fixture = AtomicCursorFixture {
        backend,
        recorders,
        state,
    };
    fixture
        .backend
        .platform
        .for_tests_initialize_cursor_planes_with_capabilities(Some(64), Some(64));
    fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    set_distinct_primary_properties(&mut fixture);
    assert!(fixture.backend.pstate.tearing.is_on());
    Some((fixture, recorder))
}

fn queue_tearing_async_obligation(
    fixture: &mut AtomicCursorFixture,
    recorder: &crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor,
    frame: u32,
) -> (std::time::Instant, Rc<std::cell::Cell<u64>>) {
    use yserver_core::backend::Backend;

    let clock_base = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let elapsed_ns = Rc::new(std::cell::Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ns);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || {
            clock_base + std::time::Duration::from_nanos(clock.get())
        }));
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 160, 120);
    Backend::maybe_composite(&mut fixture.backend).expect("submit Tearing On async scene");
    assert_eq!(
        recorder.records().last().unwrap().kind,
        AtomicCommitKind::Async
    );
    assert!(!fixture.backend.platform.async_ledger.for_tests_is_empty());
    assert!(fixture.backend.scene.has_pending_page_flips());

    let compose_interval = fixture
        .backend
        .pstate
        .tearing
        .cursor_compose_interval(&fixture.backend.platform);
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 161, 121);
    assert_eq!(
        fixture.backend.pstate.tearing.deferred_cursor_wake,
        Some(clock_base + compose_interval),
        "a second Backend cursor motion schedules the deferred cap wake",
    );

    let output = &fixture.backend.platform.outputs[0];
    let device = &fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .expect("Tearing output device")
        .device;
    fixture
        .recorders
        .page_flip_events
        .queue_flip_for_device(device, output.output.crtc, frame);
    (clock_base, elapsed_ns)
}

fn randr_output_for_connector(
    fixture: &mut AtomicCursorFixture,
    connector: &str,
) -> yserver_core::randr::RandrOutput {
    use yserver_core::backend::Backend;

    Backend::randr_outputs_and_modes(&mut fixture.backend)
        .0
        .into_iter()
        .find(|output| output.name == connector)
        .expect("RandR output is registered for lifecycle route")
}

fn exercise_mode_lifecycle_matrix_row(fixture: &mut AtomicCursorFixture, policy: CursorPolicyRow) {
    use std::{cell::Cell, rc::Rc, time::Duration};
    use yserver_core::{backend::Backend, core_loop::input_inventory::InputInventory};

    fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    fixture
        .backend
        .for_tests_set_vt_resume_executor(Rc::new(LifecycleVtResume));
    set_distinct_primary_properties(fixture);
    let original_outputs: Vec<_> = fixture
        .backend
        .platform
        .outputs
        .iter()
        .map(|layout| super::round_a::clone_drm_output(&layout.output))
        .collect();
    configure_policy(fixture, policy);

    // DPMS all-off and wake preserve the selected cursor policy.
    let before_dpms = fixture.recorders.atomic.records().len();
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("DPMS off lifecycle matrix row");
    let records = fixture.recorders.atomic.records();
    assert_output_off_records(fixture, &records, before_dpms, &original_outputs);
    Backend::set_dpms_power(&mut fixture.backend, 0).expect("DPMS on lifecycle matrix row");
    drive_policy_frame(fixture, policy, 200);

    // VT release/acquire exercises the same policy restoration after master
    // loss, including the full hidden detach and composed SW frame.
    let before_vt = fixture.recorders.atomic.records().len();
    Backend::finish_vt_release(
        &mut fixture.backend,
        &mut fixture.state,
        &InputInventory::default(),
    );
    let records = fixture.recorders.atomic.records();
    assert_output_off_records(fixture, &records, before_vt, &original_outputs);
    let before_vt_acquire = records.len();
    Backend::on_vt_acquire(&mut fixture.backend, &mut fixture.state);
    inspect_accepted_lifecycle_records(fixture, before_vt_acquire);
    let records = fixture.recorders.atomic.records();
    let resumed_cursor_only_count = records[before_vt_acquire..]
        .iter()
        .filter(|record| {
            record.kind == AtomicCommitKind::CursorOnly && record.result == ScriptedResult::Ok
        })
        .count();
    assert_eq!(
        resumed_cursor_only_count,
        if policy == CursorPolicyRow::Software {
            0
        } else {
            original_outputs.len()
        },
        "Backend VT acquire immediately restores hardware or Hidden plane state",
    );
    drive_policy_frame(fixture, policy, 300);

    // A RandR CRTC disable takes the complete old topology off and restores
    // the selected policy on the surviving output.
    let disabled_output = &original_outputs[0];
    let disabled_name = disabled_output.connector_name.clone();
    let randr_output = randr_output_for_connector(fixture, &disabled_name);
    let before_crtc = fixture.recorders.atomic.records().len();
    assert!(
        Backend::apply_crtc_config(
            &mut fixture.backend,
            randr_output.output_id,
            &disabled_name,
            None,
            0,
            0,
        )
        .expect("disable CRTC through Backend")
    );
    let records = fixture.recorders.atomic.records();
    assert_output_off_records(fixture, &records, before_crtc, &original_outputs);
    assert_eq!(fixture.backend.platform.outputs.len(), 1);
    inspect_accepted_lifecycle_records(fixture, before_crtc);
    drive_policy_frame(fixture, policy, 400);

    // Remove the surviving output through Backend hotplug dispatch. The
    // former CRTC remains off and no cursor owner survives the physical drop.
    let output = super::round_a::clone_drm_output(&fixture.backend.platform.outputs[0].output);
    let remaining_output = vec![output];
    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || base + Duration::from_millis(clock.get())));
    fixture
        .recorders
        .connector_snapshot
        .queue_snapshot(Vec::new());
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue lifecycle matrix removal");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    elapsed_ms.set(150);
    let before_remove = fixture.recorders.atomic.records().len();
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    let records = fixture.recorders.atomic.records();
    assert_output_off_records(fixture, &records, before_remove, &remaining_output);
    assert!(fixture.backend.platform.outputs.is_empty());
    for device in &fixture.backend.platform.devices {
        assert!(device.cursor.crtc_states.values().all(|state| {
            state.applied.is_none()
                && state.lifecycle_pending.is_none()
                && state.intent.is_none()
                && state.in_flight.is_none()
        }));
    }
    fixture.assert_cursor_quiescent();
    assert_all_cursor_state_quiescent(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: omit CRTC_X from a VT hardware-cursor attach or skip VT rearm; accepted plane-property and binding assertions must fail.
fn atomic_cursor_only_lifecycle_restores_hw_sw_and_hidden_policy() {
    use yserver_core::backend::Backend;

    for policy in [
        CursorPolicyRow::Hardware,
        CursorPolicyRow::Software,
        CursorPolicyRow::Hidden,
    ] {
        let Some(mut fixture) = new_policy_fixture(policy) else {
            return;
        };
        exercise_mode_lifecycle_matrix_row(&mut fixture, policy);
        assert!(fixture.backend.platform.outputs.is_empty());
        assert!(fixture.backend.platform.devices.iter().all(|device| {
            device.cursor.crtc_states.values().all(|state| {
                state.applied.is_none()
                    && state.lifecycle_pending.is_none()
                    && state.intent.is_none()
                    && state.in_flight.is_none()
            })
        }));
        fixture.assert_cursor_quiescent();
        assert_all_cursor_state_quiescent(&fixture);
        let before_shutdown = fixture.recorders.atomic.records().len();
        Backend::shutdown_outputs(&mut fixture.backend).expect("matrix fixture shutdown");
        let records = fixture.recorders.atomic.records();
        assert_eq!(
            records.len(),
            before_shutdown,
            "removed outputs are not disabled twice"
        );
        fixture.assert_cursor_quiescent();
        let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        drop(fixture);
        assert_eq!(resources.end_state(), (0, 0, 0));
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: omit Backend shutdown delegation (no disable trace); submit a separate cursor hide before the blocking disable (wrong sequence); release cursor owners before old-event drain (drain snapshot loses live buffers); omit DPMS resume invalidation (resume is not a full cursor attach); or clear expected old events (queued event and owner assertions fail).
fn atomic_cursor_only_all_lifecycle_paths_detach_and_drain() {
    let Some(mut fixture) = AtomicCursorFixture::new().expect("build lifecycle fixture") else {
        return;
    };
    let _device_peers = make_devices_pollable_for_run_core(&mut fixture);
    fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    set_distinct_primary_properties(&mut fixture);
    begin_hw_cursor(&mut fixture);
    let output = &fixture.backend.platform.outputs[0];
    let crtc = output.output.crtc;
    let device_key = output.key.device_key;
    let cursor = cursor_properties(&fixture, 0);
    let prior = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc]
        .applied
        .expect("Backend accepted a live hardware cursor");

    // DPMS off consumes an accepted cursor-only event only after its blocking
    // all-off request has detached the cursor and primary planes together.
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 101, 90);
    Backend::maybe_composite(&mut fixture.backend).expect("submit DPMS-bound cursor move");
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    queue_flip_for_output(&fixture, 0, 31);
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    let before_dpms_drain = ledger.end_state();
    let dpms_drain_observations = Rc::new(RefCell::new(Vec::new()));
    let observations = Rc::clone(&dpms_drain_observations);
    let ledger_for_observer = Rc::clone(&ledger);
    fixture
        .recorders
        .old_event_drain_observer
        .set(Rc::new(move |platform| {
            let users = platform
                .device_for_key(device_key)
                .and_then(|device| device.cursor.plane.as_ref())
                .and_then(|plane| plane.sprite_user_count(prior.lease));
            observations
                .borrow_mut()
                .push((users, ledger_for_observer.end_state()));
        }));
    let before_dpms_off = fixture.recorders.atomic.records().len();
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("DPMS blocking disable and drain");
    let records = fixture.recorders.atomic.records();
    assert_eq!(
        records.len(),
        before_dpms_off + 1,
        "no separate cursor hide commit"
    );
    assert_blocking_detach(
        &records[before_dpms_off],
        &fixture.backend.platform.outputs[0].output,
        cursor,
    );
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    let dpms_drain = dpms_drain_observations.borrow();
    let (dpms_drain_users, dpms_drain_resources) =
        dpms_drain.first().expect("DPMS drain observation");
    assert!(
        dpms_drain_users.unwrap_or(0) > 0,
        "DPMS old event still owns its cursor FB"
    );
    assert_eq!(*dpms_drain_resources, before_dpms_drain);
    assert_eq!(
        fixture
            .backend
            .platform
            .device_for_key(device_key)
            .unwrap()
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(prior.lease),
        Some(0),
        "the lifecycle owner retires after the event drain",
    );
    fixture.assert_cursor_quiescent();
    assert_all_cursor_state_quiescent(&fixture);

    // On re-modesets first, then restores the current visible HW policy with
    // a complete atomic cursor state.
    let before_dpms_on = records.len();
    Backend::set_dpms_power(&mut fixture.backend, 0).expect("DPMS enable and cursor restore");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_dpms_on + 2);
    assert_eq!(records[before_dpms_on].kind, AtomicCommitKind::Scene);
    assert_eq!(records[before_dpms_on].result, ScriptedResult::Ok);
    assert_primary_frame(
        &records[before_dpms_on],
        &fixture.backend.platform.outputs[0].output,
    );
    assert!(records[before_dpms_on].events.is_empty());
    let rebound = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc]
        .applied
        .expect("resume accepted the visible cursor");
    assert_full_attach(&fixture.backend, 0, &records[before_dpms_on + 1], rebound);
    retire_cursor_only(&mut fixture, 32);

    // Terminating run_core is not output teardown. The same trait entry used
    // by lib.rs after the loop must emit the complete shutdown disable.
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 120, 100);
    Backend::maybe_composite(&mut fixture.backend).expect("submit shutdown-bound cursor move");
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    queue_flip_for_output(&fixture, 0, 33);
    let before_shutdown = fixture.recorders.atomic.records().len();
    let shutdown_drain_observations = Rc::new(RefCell::new(Vec::new()));
    let observations = Rc::clone(&shutdown_drain_observations);
    let ledger_for_drain = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    let before_shutdown_drain = ledger_for_drain.end_state();
    let shutdown_binding = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc]
        .applied
        .expect("shutdown cursor move is still applied");
    let shutdown_lease = shutdown_binding.lease;
    fixture
        .recorders
        .old_event_drain_observer
        .set(Rc::new(move |platform| {
            let users = platform
                .device_for_key(device_key)
                .and_then(|device| device.cursor.plane.as_ref())
                .and_then(|plane| plane.sprite_user_count(shutdown_lease));
            observations
                .borrow_mut()
                .push((users, ledger_for_drain.end_state()));
        }));
    fixture
        .recorders
        .page_flip_events
        .delay_next_ready_checks(1);

    let (poll, sender, receiver) = yserver_core::core_loop::channel().expect("core channel");
    sender
        .send(yserver_core::core_loop::Message::Shutdown)
        .expect("queue core shutdown");
    yserver_core::core_loop::run_core(
        poll,
        receiver,
        sender.clone_handle(),
        &mut fixture.state,
        &mut fixture.backend,
        [],
        &yserver_core::core_loop::poll_tokens::ClientIdAllocator::new(),
        yserver_core::core_loop::auth::AuthState::new(None),
        yserver_core::core_loop::ResetPolicy::NoReset,
        None,
    )
    .expect("core loop exits from Shutdown");
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        before_shutdown,
        "run_core return alone does not disable KMS outputs",
    );

    Backend::shutdown_outputs(&mut fixture.backend).expect("post-loop Backend shutdown");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_shutdown + 1);
    assert_blocking_detach(
        &records[before_shutdown],
        &fixture.backend.platform.outputs[0].output,
        cursor,
    );
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    let shutdown_drain = shutdown_drain_observations.borrow();
    let (shutdown_drain_users, shutdown_drain_resources) = shutdown_drain
        .first()
        .expect("shutdown old-event drain observation");
    assert!(
        shutdown_drain_users.unwrap_or(0) > 0,
        "shutdown event still owns its cursor FB"
    );
    assert_eq!(*shutdown_drain_resources, before_shutdown_drain);
    fixture.assert_cursor_quiescent();
    assert_all_cursor_state_quiescent(&fixture);
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: omit cursor detach from VT, CRTC disable, or connector removal; the same-request object/property checks and final owner ledger must fail.
fn atomic_cursor_only_vt_crtc_disable_and_removal_use_lifecycle_detach() {
    use yserver_core::{backend::Backend, core_loop::input_inventory::InputInventory};

    struct LogicalVtResume;
    impl crate::kms::render::backend::kms::session::VtResumeExecutor for LogicalVtResume {
        fn acquire_master(
            &self,
            _device: &crate::kms::render::platform::KmsDevice,
        ) -> std::io::Result<()> {
            Ok(())
        }

        fn release_master(
            &self,
            _device: &crate::kms::render::platform::KmsDevice,
        ) -> std::io::Result<()> {
            Ok(())
        }

        fn probe_connector_snapshot(
            &self,
            platform: &crate::kms::render::platform::PlatformBackend,
        ) -> std::io::Result<Vec<crate::kms::render::platform::ConnectorSnapshot>> {
            Ok(platform
                .outputs
                .iter()
                .map(|output| crate::kms::render::platform::ConnectorSnapshot {
                    key: output.key.clone(),
                    modes: vec![output.output.picked.clone()],
                    mm_width: 0,
                    mm_height: 0,
                    edid: Vec::new(),
                    connector_type: "test".to_owned(),
                })
                .collect())
        }
    }

    // VT release/acquire is driven through Backend entries. The queued cursor
    // event is consumed only after the blocking detach request is accepted.
    let Some(mut vt_fixture) = AtomicCursorFixture::new().expect("build VT lifecycle fixture")
    else {
        return;
    };
    vt_fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    vt_fixture
        .backend
        .for_tests_set_vt_resume_executor(Rc::new(LogicalVtResume));
    attach_cursor_on_every_output(&mut vt_fixture);
    let vt_cursor = cursor_properties(&vt_fixture, 0);
    let vt_crtc = vt_fixture.backend.platform.outputs[0].output.crtc;
    let vt_output =
        super::round_a::clone_drm_output(&vt_fixture.backend.platform.outputs[0].output);
    let vt_binding = vt_fixture.backend.platform.devices[0].cursor.crtc_states[&vt_crtc]
        .applied
        .expect("VT starts with an accepted visible cursor");
    queue_cursor_move_for_lifecycle(&mut vt_fixture, 81);
    let before_vt = vt_fixture.recorders.atomic.records().len();
    Backend::finish_vt_release(
        &mut vt_fixture.backend,
        &mut vt_fixture.state,
        &InputInventory::default(),
    );
    let records = vt_fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_vt + 1);
    assert_blocking_detach(&records[before_vt], &vt_output, vt_cursor);
    assert_eq!(vt_fixture.recorders.page_flip_events.queued_flip_count(), 0);
    assert_eq!(
        vt_fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(vt_binding.lease),
        Some(0),
        "the drained VT event releases its final cursor owner",
    );
    vt_fixture.assert_cursor_quiescent();

    let before_vt_resume = records.len();
    Backend::on_vt_acquire(&mut vt_fixture.backend, &mut vt_fixture.state);
    let records = vt_fixture.recorders.atomic.records();
    assert_eq!(
        records.len(),
        before_vt_resume + 2,
        "resume accepted records: {:?}",
        &records[before_vt_resume..]
    );
    assert_primary_frame(&records[before_vt_resume], &vt_output);
    assert_eq!(
        records[before_vt_resume + 1].kind,
        AtomicCommitKind::CursorOnly
    );
    let rebound = vt_fixture.backend.platform.devices[0].cursor.crtc_states[&vt_crtc]
        .applied
        .expect("VT resume restores the current visible cursor policy");
    assert_full_attach(
        &vt_fixture.backend,
        0,
        &records[before_vt_resume + 1],
        rebound,
    );
    vt_fixture.retire_output(0, 82);
    Backend::shutdown_outputs(&mut vt_fixture.backend).expect("VT fixture shutdown");
    vt_fixture.assert_cursor_quiescent();
    let resources = Rc::clone(&vt_fixture.recorders.cursor_buffer_ledger);
    drop(vt_fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));

    // RandR CRTC disable performs a complete blocking disable before the
    // Backend removes the route and its cursor state.
    let Some(mut crtc_fixture) = AtomicCursorFixture::new().expect("build CRTC fixture") else {
        return;
    };
    crtc_fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    attach_cursor_on_every_output(&mut crtc_fixture);
    let cursor = cursor_properties(&crtc_fixture, 0);
    let output = super::round_a::clone_drm_output(&crtc_fixture.backend.platform.outputs[0].output);
    let connector = output.connector_name.clone();
    let randr_output = Backend::randr_outputs_and_modes(&mut crtc_fixture.backend)
        .0
        .into_iter()
        .find(|candidate| candidate.name == connector)
        .expect("RandR output for CRTC disable");
    queue_cursor_move_for_lifecycle(&mut crtc_fixture, 83);
    let before_disable = crtc_fixture.recorders.atomic.records().len();
    assert!(
        Backend::apply_crtc_config(
            &mut crtc_fixture.backend,
            randr_output.output_id,
            &connector,
            None,
            0,
            0,
        )
        .expect("disable CRTC through Backend")
    );
    let records = crtc_fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_disable + 1);
    assert_blocking_detach(&records[before_disable], &output, cursor);
    assert!(crtc_fixture.backend.platform.outputs.is_empty());
    assert_eq!(
        crtc_fixture.recorders.page_flip_events.queued_flip_count(),
        0
    );
    crtc_fixture.assert_cursor_quiescent();
    let resources = Rc::clone(&crtc_fixture.recorders.cursor_buffer_ledger);
    drop(crtc_fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));

    // Physical removal is dispatched through Backend hotplug and the deferred
    // rescan entry. It uses the same accepted all-off request before dropping
    // the live output.
    let Some(mut removal_fixture) =
        AtomicCursorFixture::new().expect("build hotplug lifecycle fixture")
    else {
        return;
    };
    removal_fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    attach_cursor_on_every_output(&mut removal_fixture);
    let cursor = cursor_properties(&removal_fixture, 0);
    let output =
        super::round_a::clone_drm_output(&removal_fixture.backend.platform.outputs[0].output);
    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(std::cell::Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    removal_fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || {
            base + std::time::Duration::from_millis(clock.get())
        }));
    removal_fixture
        .recorders
        .connector_snapshot
        .queue_snapshot(Vec::new());
    removal_fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue physical removal");
    Backend::on_display_hotplug(&mut removal_fixture.backend, &mut removal_fixture.state);
    elapsed_ms.set(150);
    queue_cursor_move_for_lifecycle(&mut removal_fixture, 84);
    let before_removal = removal_fixture.recorders.atomic.records().len();
    Backend::poll_deferred_input(&mut removal_fixture.backend, &mut removal_fixture.state);
    let records = removal_fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_removal + 1);
    assert_blocking_detach(&records[before_removal], &output, cursor);
    assert!(removal_fixture.backend.platform.outputs.is_empty());
    assert_eq!(
        removal_fixture
            .recorders
            .page_flip_events
            .queued_flip_count(),
        0
    );
    removal_fixture.assert_cursor_quiescent();
    let resources = Rc::clone(&removal_fixture.recorders.cursor_buffer_ledger);
    drop(removal_fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: release the lifecycle owner when blocking disable returns even though its old flip never arrives; retained lease/event assertions must fail before recovery.
fn atomic_cursor_only_missing_old_event_fails_stop_and_retains_owners() {
    let Some(mut fixture) = AtomicCursorFixture::new().expect("build missing-event fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    set_distinct_primary_properties(&mut fixture);
    begin_hw_cursor(&mut fixture);
    let output = super::round_a::clone_drm_output(&fixture.backend.platform.outputs[0].output);
    let cursor = cursor_properties(&fixture, 0);
    let crtc = output.crtc;
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 117, 104);
    Backend::maybe_composite(&mut fixture.backend).expect("submit old cursor flip");
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    let prior = fixture.backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("accepted cursor binding with the outstanding event");
    let before_disable = fixture.recorders.atomic.records().len();
    let error = Backend::set_dpms_power(&mut fixture.backend, 3)
        .expect_err("missing old event must hit the bounded fail-stop");
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_disable + 1);
    assert_blocking_detach(&records[before_disable], &output, cursor);
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    let state = &fixture.backend.platform.devices[0].cursor.crtc_states[&crtc];
    assert_eq!(state.applied, None, "accepted blocking detach is applied");
    assert_eq!(state.lifecycle_pending, Some(prior));
    assert!(
        state.in_flight.is_some(),
        "the old event obligation remains"
    );
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(prior.lease),
        Some(1),
        "timeout cannot reclaim a possibly scanned-out cursor FB",
    );
    // Deliver the event only after the fail-stop assertions, then let the
    // actual post-loop Backend teardown entry drain and settle its owners.
    queue_flip_for_output(&fixture, 0, 85);
    Backend::shutdown_outputs(&mut fixture.backend).expect("Backend retry drains the event");
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(prior.lease),
        Some(0),
        "successful event drain settles the retained cursor owner",
    );
    fixture.assert_cursor_quiescent();
    assert_all_cursor_state_quiescent(&fixture);
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: preserve tearing probation across DPMS off; the reset and fresh-probation assertions must fail.
fn atomic_cursor_only_tearing_dpms_exit_clears_accounting_and_restores_cursor() {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x7c7c;
    let Some((backend, recorder, state)) =
        super::super::tearing::live_tearing_cursor_backend(WINDOW, 17_000)
    else {
        return;
    };
    let recorders = Rc::clone(
        backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("Tearing fixture uses recording services"),
    );
    let mut fixture = AtomicCursorFixture {
        backend,
        recorders,
        state,
    };
    fixture
        .backend
        .platform
        .for_tests_initialize_cursor_planes_with_capabilities(Some(64), Some(64));
    fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    set_distinct_primary_properties(&mut fixture);
    let lifecycle_recorder = Rc::clone(&fixture.recorders.atomic);
    assert!(fixture.backend.pstate.tearing.is_on());
    fixture.backend.platform.devices[0]
        .atomic_commit_sink
        .set_tearing_on(true);
    let clock_base = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let elapsed_ns = Rc::new(std::cell::Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ns);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || {
            clock_base + std::time::Duration::from_nanos(clock.get())
        }));
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 160, 120);
    Backend::maybe_composite(&mut fixture.backend).expect("submit Tearing On async scene");
    assert_eq!(
        recorder.records().last().unwrap().kind,
        AtomicCommitKind::Async
    );
    assert!(!fixture.backend.platform.async_ledger.for_tests_is_empty());
    assert!(fixture.backend.scene.has_pending_page_flips());
    let compose_interval = fixture
        .backend
        .pstate
        .tearing
        .cursor_compose_interval(&fixture.backend.platform);
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 161, 121);
    assert_eq!(
        fixture.backend.pstate.tearing.deferred_cursor_wake,
        Some(clock_base + compose_interval),
        "Backend cursor motion during the compose interval schedules a deferred wake",
    );

    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let cursor = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .and_then(|plane| plane.properties_for(crtc))
        .expect("fixture cursor properties");
    let output = super::round_a::clone_drm_output(&fixture.backend.platform.outputs[0].output);
    let device = Rc::clone(&fixture.backend.platform.devices[0].device);
    let event_source = Rc::clone(
        fixture.backend.platform.devices[0]
            .test_page_flip_event_source
            .as_ref()
            .expect("recording page-flip event source"),
    );
    event_source.register_device(&device);
    event_source.queue_flip_for_device(&device, crtc, 86);
    assert!(
        fixture
            .backend
            .pstate
            .tearing
            .deferred_cursor_wake
            .is_some()
    );

    let before_off = lifecycle_recorder.records().len();
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("Tearing On DPMS all-off");
    let records = lifecycle_recorder.records();
    assert_eq!(records.len(), before_off + 1);
    assert_blocking_detach(&records[before_off], &output, cursor);
    assert_eq!(event_source.queued_flip_count(), 0);
    assert!(!fixture.backend.pstate.tearing.is_on());
    assert_eq!(fixture.backend.pstate.tearing.probation, 0);
    assert_eq!(fixture.backend.pstate.tearing.probation_owner, None);
    assert_eq!(fixture.backend.pstate.tearing.last_cursor_compose, None);
    assert_eq!(fixture.backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(fixture.backend.platform.async_ledger.for_tests_is_empty());
    assert!(!fixture.backend.scene.has_pending_page_flips());

    let before_on = records.len();
    let before_cursor_resume = recorder.records().len();
    Backend::set_dpms_power(&mut fixture.backend, 0).expect("restore cursor policy after DPMS");
    let records = lifecycle_recorder.records();
    assert!(records.len() > before_on);
    let mut resumed_scene_frames = 0;
    for record in &records[before_on..] {
        if matches!(
            record.kind,
            AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
        ) {
            assert_scene_frame(&fixture.backend, 0, record);
            resumed_scene_frames += 1;
        } else if record.kind == AtomicCommitKind::CursorOnly && record.result == ScriptedResult::Ok
        {
            assert_accepted_cursor_only_record(&fixture.backend, record);
        }
    }
    assert!(
        resumed_scene_frames > 0,
        "DPMS resume emits a primary frame"
    );
    let cursor_resume_records = recorder.records();
    let cursor_resume_scenes: Vec<_> = cursor_resume_records[before_cursor_resume..]
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
            )
        })
        .collect();
    for record in cursor_resume_scenes {
        assert_scene_frame(&fixture.backend, 0, record);
    }
    if fixture
        .backend
        .platform
        .cursor_atomic_event_pending_for_output(0)
    {
        event_source.register_device(&device);
        event_source.queue_flip_for_device(&device, crtc, 87);
        Backend::on_page_flip_ready(
            &mut fixture.backend,
            &mut fixture.state,
            device.as_fd().as_raw_fd(),
        );
    }
    if fixture.backend.scene.has_pending_page_flip(0) {
        event_source.register_device(&device);
        event_source.queue_flip_for_device(&device, crtc, 88);
        Backend::on_page_flip_ready(
            &mut fixture.backend,
            &mut fixture.state,
            device.as_fd().as_raw_fd(),
        );
    }
    assert!(fixture.backend.kms_outputs_active);
    assert!(!fixture.backend.pstate.tearing.is_on());
    assert_eq!(fixture.backend.pstate.tearing.probation, 0);
    assert_eq!(fixture.backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(fixture.backend.platform.async_ledger.for_tests_is_empty());
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
        "the resumed cursor policy matches the hidden plane state",
    );
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    Backend::shutdown_outputs(&mut fixture.backend).expect("tearing fixture shutdown");
    let shutdown_record = lifecycle_recorder
        .records()
        .last()
        .cloned()
        .expect("Tearing shutdown all-off record");
    assert_blocking_detach(&shutdown_record, &output, cursor);
    assert_eq!(event_source.queued_flip_count(), 0);
    fixture.assert_cursor_quiescent();
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: keep Tearing On through Backend shutdown; the reset assertion must fail.
fn atomic_cursor_only_tearing_shutdown_clears_accounting_and_detaches_planes() {
    use yserver_core::backend::Backend;

    let Some((mut fixture, recorder)) = new_tearing_lifecycle_fixture(0x7c7d, 17_100) else {
        return;
    };
    let output = super::round_a::clone_drm_output(&fixture.backend.platform.outputs[0].output);
    let cursor = cursor_properties(&fixture, 0);
    let _clock = queue_tearing_async_obligation(&mut fixture, &recorder, 91);
    assert!(fixture.backend.pstate.tearing.is_on());
    assert!(
        fixture
            .backend
            .pstate
            .tearing
            .deferred_cursor_wake
            .is_some()
    );

    let records_before_shutdown = fixture.recorders.atomic.records().len();
    Backend::shutdown_outputs(&mut fixture.backend).expect("Backend Tearing shutdown");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), records_before_shutdown + 1);
    assert_eq!(
        recorder.records().len(),
        1,
        "shutdown adds no replacement async frame"
    );
    assert_eq!(
        recorder
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [AtomicCommitKind::Async],
        "the sole submitted async frame remains the cursor scene obligation",
    );
    assert_blocking_detach(&records[records_before_shutdown], &output, cursor);
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    assert!(!fixture.backend.pstate.tearing.is_on());
    assert_eq!(fixture.backend.pstate.tearing.probation, 0);
    assert_eq!(fixture.backend.pstate.tearing.probation_owner, None);
    assert_eq!(fixture.backend.pstate.tearing.last_cursor_compose, None);
    assert_eq!(fixture.backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(fixture.backend.platform.async_ledger.for_tests_is_empty());
    fixture.assert_cursor_quiescent();
    assert_all_cursor_state_quiescent(&fixture);
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: keep Tearing On through Backend hotplug removal; the reset assertion must fail.
fn atomic_cursor_only_tearing_removal_clears_accounting_and_detaches_planes() {
    use std::{cell::Cell, rc::Rc, time::Duration};
    use yserver_core::backend::Backend;

    let Some((mut fixture, recorder)) = new_tearing_lifecycle_fixture(0x7c7e, 17_200) else {
        return;
    };
    let output = super::round_a::clone_drm_output(&fixture.backend.platform.outputs[0].output);
    let cursor = cursor_properties(&fixture, 0);
    let (_clock_base, _elapsed_ns) = queue_tearing_async_obligation(&mut fixture, &recorder, 92);
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    let hotplug_base = std::time::Instant::now();
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || {
            hotplug_base + Duration::from_millis(clock.get())
        }));
    fixture
        .recorders
        .connector_snapshot
        .queue_snapshot(Vec::new());
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue Tearing connector removal");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    elapsed_ms.set(150);
    let before_removal = fixture.recorders.atomic.records().len();
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);

    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_removal + 1);
    assert_eq!(
        recorder
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [AtomicCommitKind::Async],
        "removal adds no replacing async frame",
    );
    assert_blocking_detach(&records[before_removal], &output, cursor);
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    assert!(fixture.backend.platform.outputs.is_empty());
    assert!(!fixture.backend.pstate.tearing.is_on());
    assert_eq!(fixture.backend.pstate.tearing.probation, 0);
    assert_eq!(fixture.backend.pstate.tearing.probation_owner, None);
    assert_eq!(fixture.backend.pstate.tearing.last_cursor_compose, None);
    assert_eq!(fixture.backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(fixture.backend.platform.async_ledger.for_tests_is_empty());
    assert!(fixture.backend.platform.devices.iter().all(|device| {
        device.cursor.crtc_states.values().all(|state| {
            state.applied.is_none()
                && state.lifecycle_pending.is_none()
                && state.intent.is_none()
                && state.in_flight.is_none()
        })
    }));
    fixture.assert_cursor_quiescent();
    assert_all_cursor_state_quiescent(&fixture);
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

fn queue_readded_output_pool(
    fixture: &mut AtomicCursorFixture,
    output_key: &crate::kms::backend::OutputKey,
    route: crate::kms::scanout_route::ScanoutRoute,
    width: u16,
    height: u16,
) {
    use std::{rc::Rc, sync::Arc};

    let vk = Arc::clone(
        fixture
            .backend
            .platform
            .gpu
            .vk
            .as_ref()
            .expect("re-added output fixture has Vulkan"),
    );
    let device = Rc::clone(
        &fixture
            .backend
            .platform
            .device_for_key(output_key.device_key)
            .expect("re-added output has a KMS device")
            .device,
    );
    let width_u32 = u32::from(width);
    let height_u32 = u32::from(height);
    let mut bos = Vec::with_capacity(2);
    for bo_index in 0..2_u32 {
        let xid = 0x00f1_0000 + bo_index;
        let storage = fixture
            .backend
            .platform
            .gpu
            .allocate_drawable_storage(width, height, 32)
            .expect("allocate re-added test output image");
        let image = storage.image;
        let image_view = storage.image_view;
        fixture
            .backend
            .store
            .allocate(
                xid,
                crate::kms::render::store::DrawableKind::Pixmap,
                32,
                false,
                storage,
            )
            .expect("register re-added test output image");
        let bo = crate::kms::vk::scanout::ScanoutBo::for_tests_compose_target(
            Arc::clone(&vk),
            Rc::clone(&device),
            ::drm::control::from_u32(0x00d1_0000 + bo_index)
                .expect("synthetic re-added output framebuffer"),
            width_u32,
            height_u32,
            image,
            image_view,
            None,
        )
        .expect("create re-added output scanout target");
        bos.push(bo);
    }
    let pool = crate::kms::vk::scanout::ScanoutBoPool::for_tests_recording_pool(
        route, width_u32, height_u32, bos,
    );
    fixture.recorders.connector_scanout_pools.queue(
        route,
        width_u32,
        height_u32,
        crate::kms::vk::scanout::OutputScanout::Shared(pool),
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: retain Tearing across connector removal, skip cursor-topology invalidation on re-add, upload a stale/different sprite on re-add, or submit another Async frame during the first post-reconnect Present; the reset, fresh attach, BO-pixel, probation, and fresh-record-count assertions must fail.
fn atomic_cursor_only_tearing_hotplug_readd_restores_cursor_and_restarts_probation() {
    use std::{cell::Cell, rc::Rc, time::Duration};
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x7c7f;
    let Some((mut fixture, recorder)) = new_tearing_lifecycle_fixture(WINDOW, 17_300) else {
        return;
    };
    let sprite_cursor = super::round_b_producers::create_argb_cursor(
        &mut fixture.backend,
        [0x27, 0x53, 0xa9, 0xff],
        1,
        2,
    );
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, sprite_cursor.as_raw())
        .expect("set the reconnect cursor sprite through Backend before removal");
    let expected_sprite = {
        let record = &fixture.backend.cursor_records[&sprite_cursor.as_raw()];
        (
            record.width,
            record.height,
            record.hot_x,
            record.hot_y,
            record.bgra_bytes.clone(),
        )
    };
    let output = super::round_a::clone_drm_output(&fixture.backend.platform.outputs[0].output);
    let output_key = fixture.backend.platform.outputs[0].key.clone();
    let scanout_route = fixture.backend.platform.outputs[0].scanout_route;
    let cursor = cursor_properties(&fixture, 0);
    assert!(fixture.backend.platform.devices[0].cursor.plane.is_some());

    let (_clock_base, _elapsed_ns) = queue_tearing_async_obligation(&mut fixture, &recorder, 93);
    let cursor_commits_before_enable = recorder.records().len();
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    let hotplug_base = std::time::Instant::now();
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || {
            hotplug_base + Duration::from_millis(clock.get())
        }));
    fixture
        .recorders
        .connector_snapshot
        .queue_snapshot(Vec::new());
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue active Tearing connector removal");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    elapsed_ms.set(150);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);

    assert!(fixture.backend.platform.outputs.is_empty());
    assert!(!fixture.backend.pstate.tearing.is_on());
    assert_eq!(fixture.backend.pstate.tearing.probation, 0);
    assert_eq!(fixture.backend.pstate.tearing.probation_owner, None);
    assert_eq!(fixture.backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(fixture.backend.platform.async_ledger.for_tests_is_empty());
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    let removed_cursor_topology_epoch = fixture.backend.platform.devices[0]
        .cursor
        .cursor_topology_epoch;

    // The synthetic hotplug discovery preserves the physical connector and its
    // modes. Backend config then re-enables the route through the ordinary
    // CRTC configuration entry, using the recorded DRM assignment.
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    let reconnect_base = std::time::Instant::now();
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || {
            reconnect_base + Duration::from_millis(clock.get())
        }));
    fixture.recorders.connector_snapshot.queue_snapshot(vec![
        crate::kms::render::platform::ConnectorSnapshot {
            key: output_key.clone(),
            modes: output.modes.clone(),
            mm_width: output.mm_width,
            mm_height: output.mm_height,
            edid: output.edid.clone(),
            connector_type: output.connector_type.clone(),
        },
    ]);
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue connector re-add");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    elapsed_ms.set(150);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);

    let (randr_outputs, _) = Backend::randr_outputs_and_modes(&mut fixture.backend);
    let randr_output = randr_outputs
        .iter()
        .find(|candidate| candidate.name == output_key.connector_name)
        .expect("Backend hotplug re-add publishes the connector for RandR config");

    let sink_endpoint =
        crate::kms::render::backend::RandrProviderEndpoint::Kms(output_key.device_key);
    if let Some(source_endpoint) = fixture.backend.selected_render_provider_endpoint()
        && source_endpoint != sink_endpoint
    {
        let providers = Backend::randr_providers(&mut fixture.backend);
        let source_id = providers
            .iter()
            .find(|provider| {
                provider.capabilities
                    & yserver_protocol::x11::randr::PROVIDER_CAPABILITY_SOURCE_OUTPUT
                    != 0
            })
            .expect("selected renderer has a source provider")
            .provider_id;
        let sink_id = providers
            .iter()
            .find(|provider| provider.outputs.contains(&randr_output.output_id))
            .expect("re-added connector belongs to a KMS sink provider")
            .provider_id;
        fixture.state.randr.set_providers(providers);
        assert!(
            Backend::set_provider_output_source(
                &mut fixture.backend,
                &mut fixture.state,
                sink_id,
                Some(source_id),
            )
            .expect("authorize the selected renderer for the re-added KMS sink")
        );
    }

    fixture
        .recorders
        .crtc_output_discovery
        .queue_output(super::round_a::clone_drm_output(&output));
    queue_readded_output_pool(
        &mut fixture,
        &output_key,
        scanout_route,
        output.picked.width,
        output.picked.height,
    );
    assert!(
        Backend::apply_crtc_config(
            &mut fixture.backend,
            randr_output.output_id,
            &output_key.connector_name,
            Some(yserver_core::backend::ModeSpec {
                width: output.picked.width,
                height: output.picked.height,
                vrefresh: output.picked.vrefresh,
            }),
            0,
            0,
        )
        .expect("enable re-added connector through Backend CRTC configuration")
    );
    assert_eq!(fixture.recorders.crtc_output_discovery.calls(), 1);
    assert_eq!(fixture.backend.platform.outputs.len(), 1);
    assert_eq!(fixture.backend.platform.outputs[0].key, output_key);
    assert!(!fixture.backend.pstate.tearing.is_on());
    assert_eq!(fixture.backend.pstate.tearing.probation, 0);
    assert_eq!(fixture.backend.pstate.tearing.probation_owner, None);
    assert!(fixture.backend.platform.async_ledger.for_tests_is_empty());
    assert!(
        fixture.backend.platform.devices[0]
            .cursor
            .cursor_topology_epoch
            > removed_cursor_topology_epoch,
        "connector re-add starts a fresh cursor qualification epoch",
    );

    let async_commits_before_readd = recorder
        .records()
        .iter()
        .filter(|record| record.kind == AtomicCommitKind::Async)
        .count();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 162, 122);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("restore cursor policy after re-add");
    let cursor_records = recorder.records();
    inspect_accepted_lifecycle_record_slice(
        &fixture,
        &cursor_records[cursor_commits_before_enable..],
    );
    drain_policy_fixture_events(&mut fixture, 940);
    let records = recorder.records();
    let readd_records = &records[cursor_commits_before_enable..];
    let attach_attempts: Vec<_> = readd_records
        .iter()
        .filter(|record| {
            !record.flags.contains(AtomicCommitFlags::TEST_ONLY)
                && record.properties.iter().any(|update| {
                    update.object == cursor.plane.into()
                        && update.property == cursor.fb_id
                        && update.value != 0
                })
        })
        .collect();
    // Mutation: carry the old applied cursor comparison across the topology epoch; the fresh attach attempt or binding must be absent.
    assert_eq!(
        attach_attempts.len(),
        1,
        "re-added CRTC gets exactly one fresh cursor attach opportunity; mode={:?}, records={readd_records:?}, state={:?}",
        fixture.backend.scene.cursor_mode(),
        fixture.backend.platform.devices[0].cursor.crtc_states
            [&fixture.backend.platform.outputs[0].output.crtc],
    );
    let output_idx = 0;
    if fixture.backend.scene.cursor_mode() == crate::kms::render::scene::CursorPlaneMode::Hw {
        let receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(output_idx)
            .expect("re-added output accepted a cursor policy frame");
        assert_eq!(
            receipt.mode,
            crate::kms::render::scene::CursorAcceptedMode::Hardware
        );
        assert!(
            fixture
                .backend
                .platform
                .cursor_plane_visible_for_output(output_idx)
        );
        let binding = fixture.backend.platform.devices[0].cursor.crtc_states
            [&fixture.backend.platform.outputs[0].output.crtc]
            .applied
            .expect("hardware policy has an accepted re-added cursor binding");
        assert_eq!(
            attach_attempts[0]
                .properties
                .iter()
                .find(|update| {
                    update.object == cursor.plane.into() && update.property == cursor.fb_id
                })
                .map(|update| update.value),
            Some(u64::from(binding.framebuffer)),
            "re-add binds the framebuffer for the current cursor sprite",
        );
        assert_full_attach(&fixture.backend, output_idx, attach_attempts[0], binding);
        assert!(
            fixture.backend.platform.devices[0]
                .cursor
                .plane
                .as_ref()
                .and_then(|pool| pool.sprite_user_count(binding.lease))
                .is_some_and(|users| users > 0)
        );
        // Mutation: upload a stale/different sprite on re-add; checking the actual bound BO pixels and hotspot must fail even when the applied binding agrees with the commit.
        assert_eq!(
            binding.lease.sprite_key(),
            Some((
                expected_sprite.0,
                expected_sprite.1,
                expected_sprite.2,
                expected_sprite.3,
            )),
            "re-add stages the dimensions and hotspot of the cursor set before removal",
        );
        assert_eq!(
            (binding.hot_x, binding.hot_y),
            (expected_sprite.2, expected_sprite.3),
            "re-add binds the cursor sprite set before connector removal",
        );
        let (device_fd, handle, capacity_width, capacity_height) = fixture
            .recorders
            .cursor_buffer_ledger
            .records()
            .into_iter()
            .find_map(|record| match record {
                crate::kms::cursor_plane::ops::CursorBufferRecord::AddFb {
                    device_fd,
                    handle,
                    fb,
                    width,
                    height,
                    ..
                } if fb == binding.framebuffer => Some((device_fd, handle, width, height)),
                _ => None,
            })
            .expect("the accepted re-add framebuffer has a recorded cursor BO");
        let uploaded_bytes = fixture
            .recorders
            .cursor_buffer_ledger
            .mapped_bytes(device_fd, handle)
            .expect("the accepted re-add cursor BO remains mapped");
        assert!(expected_sprite.0 <= capacity_width as u16);
        assert!(expected_sprite.1 <= capacity_height as u16);
        let stride = uploaded_bytes.len() / capacity_height as usize;
        assert_eq!(uploaded_bytes.len() % capacity_height as usize, 0);
        let source_stride = expected_sprite.0 as usize * 4;
        for row in 0..expected_sprite.1 as usize {
            let uploaded_start = row * stride;
            let source_start = row * source_stride;
            assert_eq!(
                &uploaded_bytes[uploaded_start..uploaded_start + source_stride],
                &expected_sprite.4[source_start..source_start + source_stride],
                "re-add cursor BO row {row} matches the sprite set before removal",
            );
        }
    } else {
        let receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(output_idx)
            .expect("re-added output accepted a software-cursor frame");
        assert_eq!(
            receipt.mode,
            crate::kms::render::scene::CursorAcceptedMode::Software
        );
        assert!(matches!(
            receipt.primary_cursor,
            crate::kms::render::scene::CursorPrimaryContent::Software { .. }
        ));
        assert!(
            !fixture
                .backend
                .platform
                .cursor_plane_visible_for_output(output_idx)
        );
    }

    // Mutation: retain probation or submit another Async frame during reconnect; the first fresh owner/count and empty-ledger assertions must fail.
    let _ = Backend::randr_outputs_and_modes(&mut fixture.backend);
    let (width, height) = fixture.backend.platform.fb_dimensions();
    let (candidate, event, _) = super::super::tearing::prepare_direct_present_for_ids(
        &mut fixture.backend,
        37_301,
        WINDOW,
        WINDOW,
        width,
        height,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    );
    let present_submitted = Backend::try_present_direct(&mut fixture.backend, candidate, event)
        .expect("first post-reconnect Present remains in Vsync probation");
    let records_after_present = recorder.records();
    assert!(!present_submitted);
    assert!(!fixture.backend.pstate.tearing.is_on());
    assert_eq!(fixture.backend.pstate.tearing.probation, 1);
    assert_eq!(fixture.backend.pstate.tearing.probation_owner, Some(WINDOW));
    assert!(fixture.backend.platform.async_ledger.for_tests_is_empty());
    assert_eq!(
        records_after_present
            .iter()
            .filter(|record| record.kind == AtomicCommitKind::Async)
            .count(),
        async_commits_before_readd,
        "probation attempt submits no async replacement frame",
    );

    Backend::shutdown_outputs(&mut fixture.backend).expect("drain re-added output");
    for bo_index in 0..2_u32 {
        let xid = 0x00f1_0000 + bo_index;
        Backend::free_pixmap(&mut fixture.backend, None, xid)
            .expect("release re-added scanout test image");
        assert!(fixture.backend.store.lookup(xid).is_none());
    }
    assert_all_cursor_state_quiescent(&fixture);
    fixture.assert_cursor_quiescent();
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: skip exit_tearing_for_window in Backend::destroy_subwindow; the reset, cursor restoration, and drained-owner assertions must fail.
fn atomic_cursor_only_tearing_window_destroy_uses_backend_lifecycle_row() {
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x7c81;
    let Some((mut fixture, recorder)) = new_tearing_lifecycle_fixture(WINDOW, 17_400) else {
        return;
    };
    let (_clock_base, _elapsed_ns) = queue_tearing_async_obligation(&mut fixture, &recorder, 94);
    assert!(fixture.backend.pstate.tearing.is_on());
    assert!(!fixture.backend.platform.async_ledger.for_tests_is_empty());
    assert!(fixture.backend.windows.contains_key(&WINDOW));

    Backend::destroy_subwindow(&mut fixture.backend, None, WINDOW)
        .expect("destroy the active Tearing owner through Backend");
    assert!(!fixture.backend.pstate.tearing.is_on());
    assert_eq!(fixture.backend.pstate.tearing.probation, 0);
    assert_eq!(fixture.backend.pstate.tearing.probation_owner, None);
    assert_eq!(fixture.backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(fixture.backend.platform.async_ledger.for_tests_is_empty());
    assert!(!fixture.backend.windows.contains_key(&WINDOW));

    fixture.retire_output(0, 950);
    drive_policy_frame(&mut fixture, CursorPolicyRow::Hardware, 952);
    let records = recorder.records();
    assert_eq!(
        records.first().map(|record| record.kind),
        Some(AtomicCommitKind::Async),
        "destroy drains the old accepted async frame before normal cursor policy",
    );
    assert!(records.iter().any(|record| {
        matches!(
            record.kind,
            AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
        ) && record.result == ScriptedResult::Ok
    }));
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(fixture.backend.platform.async_ledger.for_tests_is_empty());

    Backend::shutdown_outputs(&mut fixture.backend).expect("window lifecycle fixture shutdown");
    assert_all_cursor_state_quiescent(&fixture);
    fixture.assert_cursor_quiescent();
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: guard hidden resume recovery on `scanout_m2.current`; after VT clears direct ownership, the required explicit cursor-plane detach must disappear and this test must fail.
fn atomic_cursor_only_hidden_resume_detaches_after_direct_ownership_clears() {
    use yserver_core::{backend::Backend, core_loop::input_inventory::InputInventory};

    struct LogicalVtResume;
    impl crate::kms::render::backend::kms::session::VtResumeExecutor for LogicalVtResume {
        fn acquire_master(
            &self,
            _device: &crate::kms::render::platform::KmsDevice,
        ) -> std::io::Result<()> {
            Ok(())
        }

        fn release_master(
            &self,
            _device: &crate::kms::render::platform::KmsDevice,
        ) -> std::io::Result<()> {
            Ok(())
        }

        fn probe_connector_snapshot(
            &self,
            platform: &crate::kms::render::platform::PlatformBackend,
        ) -> std::io::Result<Vec<crate::kms::render::platform::ConnectorSnapshot>> {
            Ok(platform
                .outputs
                .iter()
                .map(|output| crate::kms::render::platform::ConnectorSnapshot {
                    key: output.key.clone(),
                    modes: vec![output.output.picked.clone()],
                    mm_width: 0,
                    mm_height: 0,
                    edid: Vec::new(),
                    connector_type: "test".to_owned(),
                })
                .collect())
        }
    }

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build hidden-resume fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    set_distinct_primary_properties(&mut fixture);
    fixture
        .backend
        .for_tests_set_vt_resume_executor(Rc::new(LogicalVtResume));
    begin_hw_cursor(&mut fixture);
    let cursor = cursor_properties(&fixture, 0);
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let stale_binding = fixture.backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("visible cursor starts attached");

    // The cursor becomes Hidden as VT suspension begins. Reject the blocking
    // disable that would otherwise detach it, leaving a real accepted binding
    // for the independent hidden-on-resume recovery hook to clear.
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));

    Backend::finish_vt_release(
        &mut fixture.backend,
        &mut fixture.state,
        &InputInventory::default(),
    );
    assert_eq!(
        fixture.backend.platform.devices[0].cursor.crtc_states[&crtc].applied,
        Some(stale_binding),
        "rejected suspended detach retains the stale hardware binding",
    );
    assert!(fixture.backend.pstate.scanout_m2.current.is_none());
    assert!(fixture.backend.pstate.scanout_m2.pending.is_none());
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_none());
    assert!(
        fixture.backend.platform.cursor_plane_visible_for_output(0),
        "the rejected blocking hide leaves the stale applied binding intact until resume recovery",
    );
    let before_hidden_resume = fixture.recorders.atomic.records().len();
    Backend::on_vt_acquire(&mut fixture.backend, &mut fixture.state);

    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_hidden_resume + 2);
    assert_eq!(records[before_hidden_resume].kind, AtomicCommitKind::Scene);
    assert_primary_frame(
        &records[before_hidden_resume],
        &fixture.backend.platform.outputs[0].output,
    );
    let hidden_recovery = &records[before_hidden_resume + 1];
    assert_eq!(hidden_recovery.result, ScriptedResult::Ok);
    assert_eq!(hidden_recovery.kind, AtomicCommitKind::CursorOnly);
    assert_eq!(hidden_recovery.events.len(), 1);
    let plane: ::drm::control::plane::Handle = cursor.plane;
    for property in [cursor.fb_id, cursor.crtc_id] {
        assert!(
            hidden_recovery.properties.iter().any(|update| {
                update.object == plane.into() && update.property == property && update.value == 0
            }),
            "hidden resume omitted explicit detach property {property:?}"
        );
    }
    assert!(fixture.backend.pstate.scanout_m2.current.is_none());
    assert_eq!(
        fixture.backend.platform.devices[0].cursor.crtc_states[&crtc].applied, None,
        "hidden resume recovery clears the binding without direct ownership",
    );
    assert!(fixture.backend.effective_cursor_hidden());
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    fixture.retire_output(0, 89);
    fixture.assert_cursor_quiescent();
    Backend::shutdown_outputs(&mut fixture.backend).expect("hidden fixture shutdown");
    fixture.assert_cursor_quiescent();
    assert_all_cursor_state_quiescent(&fixture);
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: clear a failed peer's accepted cursor users; drop expected old-event CRTCs; or qualify ownership by raw handle without the device key. Each breaks the retained-owner, event-drain, or independent-device assertions below.
fn atomic_cursor_only_partial_disable_failure_retains_remaining_owners() {
    shared_pool_partial_disable_keeps_peer_user();

    let Some(mut fixture) = AtomicCursorFixture::new_with_colliding_device_handles()
        .expect("build two-device cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    assert_eq!(fixture.backend.platform.outputs.len(), 2);
    assert_ne!(
        fixture.backend.platform.outputs[0].key.device_key,
        fixture.backend.platform.outputs[1].key.device_key,
    );
    attach_cursor_on_every_output(&mut fixture);

    let outputs: Vec<_> = (0..2)
        .map(|output_idx| {
            let output = &fixture.backend.platform.outputs[output_idx];
            let binding = fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .unwrap()
                .cursor
                .crtc_states[&output.output.crtc]
                .applied
                .expect("each device accepted a cursor binding");
            (output.key.device_key, output.output.crtc, binding)
        })
        .collect();
    assert_eq!(outputs[0].2.framebuffer, outputs[1].2.framebuffer);
    let pool_user_counts: Vec<_> = outputs
        .iter()
        .map(|&(device_key, crtc, binding)| {
            let device = fixture.backend.platform.device_for_key(device_key).unwrap();
            let pool = device.cursor.plane.as_ref().expect("cursor sprite pool");
            assert!(pool.sprite_user_count(binding.lease).unwrap_or(0) > 0);
            let state = &device.cursor.crtc_states[&crtc];
            assert_eq!(state.applied, Some(binding));
            pool.sprite_user_count(binding.lease).unwrap()
        })
        .collect();
    assert_eq!(pool_user_counts, vec![1, 1]);

    // Each device accepts an in-flight move before the lifecycle boundary.
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 110, 100);
    Backend::maybe_composite(&mut fixture.backend).expect("submit per-device cursor moves");
    for output_idx in 0..2 {
        assert!(
            fixture
                .backend
                .platform
                .cursor_atomic_event_pending_for_output(output_idx)
        );
        queue_flip_for_output(&fixture, output_idx, 51 + output_idx as u32);
    }
    let moved_bindings: Vec<_> = outputs
        .iter()
        .map(|&(device_key, crtc, _)| {
            fixture
                .backend
                .platform
                .device_for_key(device_key)
                .unwrap()
                .cursor
                .crtc_states[&crtc]
                .applied
                .expect("accepted cursor move remains applied until disable")
        })
        .collect();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    let before_failure = ledger.end_state();
    fixture.recorders.atomic.script_result(Ok(()));
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));

    let before_partial_disable = fixture.recorders.atomic.records().len();
    let error = Backend::shutdown_outputs(&mut fixture.backend)
        .expect_err("second device disable failure is surfaced");
    assert!(
        error
            .to_string()
            .contains("disable_output atomic commit rejected")
    );
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_partial_disable + 2);
    assert_blocking_detach(
        &records[before_partial_disable],
        &fixture.backend.platform.outputs[0].output,
        cursor_properties(&fixture, 0),
    );
    assert_blocking_detach(
        &records[before_partial_disable + 1],
        &fixture.backend.platform.outputs[1].output,
        cursor_properties(&fixture, 1),
    );
    assert_eq!(records[before_partial_disable].result, ScriptedResult::Ok);
    assert!(matches!(
        records[before_partial_disable + 1].result,
        ScriptedResult::Error {
            raw_os_error: Some(libc::EIO),
            ..
        }
    ));
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 2);
    assert_eq!(ledger.end_state(), before_failure);

    let first_key = outputs[0].0;
    let first_crtc = outputs[0].1;
    let first_binding = moved_bindings[0];
    let second_key = outputs[1].0;
    let second_crtc = outputs[1].1;
    let second_binding = moved_bindings[1];
    let first_state = &fixture
        .backend
        .platform
        .device_for_key(first_key)
        .unwrap()
        .cursor
        .crtc_states[&first_crtc];
    assert_eq!(first_state.applied, None);
    assert_eq!(first_state.lifecycle_pending, Some(first_binding));
    assert!(
        first_state.in_flight.is_some(),
        "A retains its old-event obligation"
    );
    let second_state = &fixture
        .backend
        .platform
        .device_for_key(second_key)
        .unwrap()
        .cursor
        .crtc_states[&second_crtc];
    assert_eq!(second_state.applied, Some(second_binding));
    assert!(second_state.lifecycle_pending.is_none());
    assert!(
        second_state.in_flight.is_some(),
        "B retains its old-event obligation"
    );
    for (device_key, binding) in [(first_key, first_binding), (second_key, second_binding)] {
        let pool = fixture
            .backend
            .platform
            .device_for_key(device_key)
            .unwrap()
            .cursor
            .plane
            .as_ref()
            .unwrap();
        assert!(
            pool.sprite_user_count(binding.lease).unwrap_or(0) > 0,
            "successful A disable cannot release B's device-local owner or vice versa",
        );
    }

    // The retry completes both blocking disables; event readiness is held off
    // for both devices once so the all-off drain must keep every lease alive.
    let drain_observations = Rc::new(RefCell::new(Vec::new()));
    let observations = Rc::clone(&drain_observations);
    let ledger_for_drain = Rc::clone(&ledger);
    let leases = [
        (first_key, first_binding.lease),
        (second_key, second_binding.lease),
    ];
    fixture
        .recorders
        .old_event_drain_observer
        .set(Rc::new(move |platform| {
            let users = leases.map(|(device_key, lease)| {
                platform
                    .device_for_key(device_key)
                    .and_then(|device| device.cursor.plane.as_ref())
                    .and_then(|plane| plane.sprite_user_count(lease))
            });
            observations
                .borrow_mut()
                .push((users, ledger_for_drain.end_state()));
        }));
    fixture
        .recorders
        .page_flip_events
        .delay_next_ready_checks(2);
    let before_retry = fixture.recorders.atomic.records().len();
    Backend::shutdown_outputs(&mut fixture.backend).expect("retry all-off and drain old events");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_retry + 2);
    assert_blocking_detach(
        &records[before_retry],
        &fixture.backend.platform.outputs[0].output,
        cursor_properties(&fixture, 0),
    );
    assert_blocking_detach(
        &records[before_retry + 1],
        &fixture.backend.platform.outputs[1].output,
        cursor_properties(&fixture, 1),
    );
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    let drain = drain_observations.borrow();
    let (drain_users, drain_resources) = drain.first().expect("partial disable drain observation");
    assert!(drain_users.iter().all(|users| users.unwrap_or(0) > 0));
    assert_eq!(*drain_resources, before_failure);
    for ((device_key, crtc, _), binding) in outputs.into_iter().zip(moved_bindings) {
        let device = fixture.backend.platform.device_for_key(device_key).unwrap();
        let state = &device.cursor.crtc_states[&crtc];
        assert_eq!(state.applied, None);
        assert_eq!(state.lifecycle_pending, None);
        assert_eq!(state.intent, None);
        assert_eq!(state.in_flight, None);
        assert_eq!(
            device
                .cursor
                .plane
                .as_ref()
                .unwrap()
                .sprite_user_count(binding.lease),
            Some(0),
        );
    }
    fixture.assert_cursor_quiescent();
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}
