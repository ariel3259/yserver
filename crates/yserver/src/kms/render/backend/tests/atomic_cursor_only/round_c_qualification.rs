//! Cursor-plane qualification is per CRTC, parameter key, and topology epoch.

use std::{rc::Rc, time::Duration};

use super::{
    round_a::damage_cursor_root,
    round_b_producers::{begin_hw_cursor, create_argb_cursor_extent},
    round_c_attribution::{cursor_fixture, output_cursor_handles},
    support::AtomicCursorFixture,
};
use crate::drm::atomic::{
    AtomicCommitKind,
    tests_support::{AtomicCommitRecord, ScriptedResult},
};
use ::drm::control::AtomicCommitFlags;
use yserver_core::backend::Backend;

fn handle(
    handles: &[(&'static str, ::drm::control::property::Handle)],
    name: &str,
) -> ::drm::control::property::Handle {
    handles
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .unwrap_or_else(|| panic!("output has no {name} property"))
        .1
}

fn live_records(fixture: &super::support::AtomicCursorFixture) -> Vec<AtomicCommitRecord> {
    fixture
        .recorders
        .atomic
        .records()
        .into_iter()
        .filter(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY))
        .collect()
}

fn rejected_cursor_attempts(
    records: &[AtomicCommitRecord],
    cursor_property: ::drm::control::property::Handle,
) -> Vec<AtomicCommitRecord> {
    records
        .iter()
        .filter(|record| {
            matches!(record.result, ScriptedResult::Error { .. })
                && record
                    .properties
                    .iter()
                    .any(|update| update.property == cursor_property)
        })
        .cloned()
        .collect()
}

fn cursor_attach_attempts(
    records: &[AtomicCommitRecord],
    fb_property: ::drm::control::property::Handle,
    width_property: ::drm::control::property::Handle,
) -> Vec<AtomicCommitRecord> {
    records
        .iter()
        .filter(|record| {
            record
                .properties
                .iter()
                .any(|update| update.property == fb_property && update.value != 0)
                && record
                    .properties
                    .iter()
                    .any(|update| update.property == width_property && update.value != 0)
        })
        .cloned()
        .collect()
}

fn assert_cursorless_primary(
    record: &AtomicCommitRecord,
    handles: &[(&'static str, ::drm::control::property::Handle)],
) {
    assert!(
        record.properties.iter().all(|update| {
            handles
                .iter()
                .find(|(_, property)| *property == update.property)
                .is_none_or(|(name, _)| matches!(*name, "fb_id" | "crtc_id") && update.value == 0)
        }),
        "software primary may carry only explicit zero cursor detach IDs: {record:?}",
    );
}

fn assert_scene_cursor_rejection_retry(records: &[AtomicCommitRecord]) {
    assert_eq!(
        records.len(),
        2,
        "cursor rejection and cursorless retry: {records:?}"
    );
    assert_eq!(records[0].kind, AtomicCommitKind::SceneWithCursor);
    assert!(matches!(records[0].result, ScriptedResult::Error { .. }));
    assert_eq!(records[1].kind, AtomicCommitKind::Scene);
    assert_eq!(records[1].result, ScriptedResult::Ok);
    assert_eq!(
        records[1].events.len(),
        1,
        "accepted retry owns one flip event"
    );
}

fn assert_software_recompose(fixture: &mut super::support::AtomicCursorFixture, frame: u32) {
    let before = live_records(fixture).len();
    damage_cursor_root(fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit software cursor frame");
    let records = live_records(fixture)[before..].to_vec();
    let accepted = records
        .iter()
        .filter(|record| {
            record.kind != AtomicCommitKind::CursorOnly && record.result == ScriptedResult::Ok
        })
        .collect::<Vec<_>>();
    assert_eq!(
        accepted.len(),
        1,
        "one accepted software scene frame: {records:?}"
    );
    assert_cursorless_primary(accepted[0], &output_cursor_handles(&fixture.backend));
    retire_pending(fixture, frame);
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0),
        "frame {frame}: accepted scene records a software cursor footprint; mode={:?} hidden={} records={records:?}",
        fixture.backend.scene.cursor_mode(),
        fixture.backend.effective_cursor_hidden(),
    );
    let under_pointer = read_output_pixel(&mut fixture.backend, 125, 95);
    let beside_pointer = read_output_pixel(&mut fixture.backend, 160, 130);
    assert_ne!(
        under_pointer[..3],
        beside_pointer[..3],
        "accepted primary content includes software cursor pixels",
    );
}

fn assert_accepted_software_primary_content(fixture: &mut super::support::AtomicCursorFixture) {
    assert_accepted_software_primary_content_at(fixture, 125, 95);
}

fn assert_accepted_software_primary_content_at(
    fixture: &mut super::support::AtomicCursorFixture,
    x: i32,
    y: i32,
) {
    let under_pointer = read_output_pixel(&mut fixture.backend, x, y);
    let beside_pointer = read_output_pixel(&mut fixture.backend, 160, 130);
    assert_ne!(
        under_pointer[..3],
        beside_pointer[..3],
        "accepted intermediate primary already contains software cursor pixels",
    );
}

fn assert_cursorless_primary_content_at(
    fixture: &mut super::support::AtomicCursorFixture,
    x: i32,
    y: i32,
) {
    let under_pointer = read_output_pixel(&mut fixture.backend, x, y);
    let beside_pointer = read_output_pixel(&mut fixture.backend, 160, 130);
    assert_eq!(
        under_pointer[..3],
        beside_pointer[..3],
        "accepted cursorless retry contains no software cursor pixels",
    );
}

fn retire_software_scene_frame(fixture: &mut super::support::AtomicCursorFixture, frame: u32) {
    let before = live_records(fixture).len();
    damage_cursor_root(fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit software scene frame");
    let records = live_records(fixture)[before..].to_vec();
    let primary = records
        .iter()
        .filter(|record| {
            record.kind != AtomicCommitKind::CursorOnly && record.result == ScriptedResult::Ok
        })
        .collect::<Vec<_>>();
    assert_eq!(primary.len(), 1, "one accepted software frame: {records:?}");
    assert_cursorless_primary(primary[0], &output_cursor_handles(&fixture.backend));
    retire_pending(fixture, frame);
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
}

fn read_output_pixel(
    backend: &mut crate::kms::render::backend::KmsBackend,
    x: i32,
    y: i32,
) -> [u8; 4] {
    let bytes = crate::kms::render::backend::read_scanout_region(
        backend,
        ash::vk::Rect2D {
            offset: ash::vk::Offset2D { x, y },
            extent: ash::vk::Extent2D {
                width: 1,
                height: 1,
            },
        },
        crate::kms::render::backend::ScanoutReadSelection::PermissiveDump,
    )
    .expect("read composed output pixel");
    bytes.try_into().expect("one BGRA pixel")
}

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

fn cursor_plane_properties(
    fixture: &super::support::AtomicCursorFixture,
    output_idx: usize,
) -> crate::kms::cursor_plane::topology::CursorPlaneProperties {
    let output = &fixture.backend.platform.outputs[output_idx];
    fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(output.output.crtc))
        .expect("output has cursor plane properties")
}

fn updates_for_plane(
    records: &[AtomicCommitRecord],
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
) -> Vec<AtomicCommitRecord> {
    records
        .iter()
        .filter(|record| {
            record.properties.iter().any(|update| {
                update.object == properties.plane.into()
                    && (update.property == properties.fb_id
                        || update.property == properties.crtc_id)
            })
        })
        .cloned()
        .collect()
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations this test must reject: key history by record version; forget older
// keys on a new signature; retry after N SW retirements; retry every animation
// frame among rejected keys; share rejection across CRTCs; clear on unchanged
// DPMS/VT; retain keys after a changed epoch; or clear ownership at rejection.
// Its intermediate checks also catch retaining HW after an accepted detach,
// leaving that accepted scene detach in DetachPending, or omitting SW pixels on retry.
fn atomic_cursor_only_requalification_is_once_per_key_per_epoch() {
    let reporter = Rc::new(
        crate::kms::render::platform::cursor_recovery::RecordingCursorRecoveryReporter::default(),
    );
    let Some((mut fixture, handles)) = cursor_fixture(&reporter) else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let output = &fixture.backend.platform.outputs[0];
    let device_key = output.key.device_key;
    let crtc = output.output.crtc;
    let prior_binding = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.crtc_states.get(&crtc))
        .and_then(|state| state.applied)
        .expect("initial accepted cursor binding");
    let submitted_key = prior_binding.lease.sprite_key();
    assert!(
        submitted_key.is_some(),
        "the rejected sprite has a parameter key"
    );

    // Reject K1 on a cursor-only motion. Make the first explicit detach return
    // EBUSY so ownership can be inspected before the detach accepts.
    fixture
        .recorders
        .atomic
        .reject_properties(&[handle(&handles, "crtc_x")]);
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    let before_k1 = live_records(&fixture).len();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 125, 95);
    Backend::maybe_composite(&mut fixture.backend).expect("submit rejected K1 motion");
    let k1_motion = live_records(&fixture)[before_k1..].to_vec();
    assert_eq!(
        k1_motion.len(),
        2,
        "K1 rejection then a detach waiting for a flip: {k1_motion:?}"
    );
    assert_eq!(k1_motion[0].kind, AtomicCommitKind::CursorOnly);
    assert_eq!(k1_motion[1].kind, AtomicCommitKind::CursorOnly);
    assert!(matches!(k1_motion[0].result, ScriptedResult::Error { .. }));
    assert_eq!(
        k1_motion[1].result,
        ScriptedResult::Error {
            kind: std::io::ErrorKind::ResourceBusy,
            raw_os_error: Some(libc::EBUSY),
        },
        "the explicit detach remains unaccepted after EBUSY",
    );

    let state = &fixture
        .backend
        .platform
        .device_for_key(device_key)
        .expect("cursor device")
        .cursor
        .crtc_states[&crtc];
    assert_eq!(
        state.fallback,
        crate::kms::render::platform::cursor_state::CursorFallbackState::DetachPending {
            rejected_key: submitted_key,
        },
        "K1 rejection leaves its detach owed with the submitted key",
    );
    assert_eq!(
        state.applied,
        Some(prior_binding),
        "rejected K1 still owns the previously accepted plane binding",
    );
    assert_eq!(
        state.intent,
        Some(crate::kms::render::platform::cursor_state::CursorIntent::Hide)
    );
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw,
        "software mode waits for an accepted detach",
    );
    assert!(
        !fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0),
        "no software cursor is composed while rejected K1 remains bound",
    );

    fixture.recorders.atomic.clear_property_rejections();
    let before_scene = live_records(&fixture).len();
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit explicit detach through Backend");
    let detach_scene = live_records(&fixture)[before_scene..].to_vec();
    assert_eq!(
        detach_scene.len(),
        1,
        "one explicit-detach scene frame: {detach_scene:?}"
    );
    assert_eq!(
        detach_scene[0].kind,
        AtomicCommitKind::SceneWithCursor,
        "the scene frame carries the explicit detach",
    );
    assert_eq!(detach_scene[0].result, ScriptedResult::Ok);
    assert_eq!(detach_scene[0].events.len(), 1);
    assert!(
        detach_scene[0]
            .properties
            .iter()
            .any(|update| { update.property == handle(&handles, "fb_id") && update.value == 0 }),
        "the accepted scene explicitly detaches the hardware plane",
    );
    assert!(
        detach_scene[0]
            .properties
            .iter()
            .any(|update| { update.property == handle(&handles, "crtc_id") && update.value == 0 })
    );
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    retire_pending(&mut fixture, 3);
    let settled_fallback = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .expect("cursor device")
        .cursor
        .crtc_states[&crtc]
        .fallback;
    assert_eq!(
        settled_fallback,
        crate::kms::render::platform::cursor_state::CursorFallbackState::Software {
            rejected_key: submitted_key,
        },
        "an accepted scene-owned detach settles the recovery fallback",
    );
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw,
        "software mode begins only after the explicit detach accepts",
    );
    assert_accepted_software_primary_content_at(&mut fixture, 125, 95);
    assert_software_recompose(&mut fixture, 4);

    // K2 changes dimensions; K3 changes only the hotspot. Each new key gets
    // one hardware probe, and the recording sink rejects that probe.
    let root = fixture.backend.core.window_id;
    let fb_property = handle(&handles, "fb_id");
    let width_property = handle(&handles, "crtc_w");
    let k2 = create_argb_cursor_extent(&mut fixture.backend, [0x21, 0x42, 0x63, 0xff], 32, 4, 0, 0);
    fixture
        .recorders
        .atomic
        .reject_properties(&[width_property]);
    let before_k2 = live_records(&fixture).len();
    Backend::define_cursor(&mut fixture.backend, None, root, k2.as_raw())
        .expect("select K2 through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit K2 qualification");
    let k2_records = live_records(&fixture)[before_k2..].to_vec();
    let k2_attempts = rejected_cursor_attempts(&k2_records, width_property);
    assert_scene_cursor_rejection_retry(&k2_records);
    assert!(
        !fixture.backend.platform.cursor_plane_visible_for_output(0),
        "rejected K2 show and accepted cursorless retry preserve detached ownership",
    );
    assert_eq!(
        k2_attempts.len(),
        1,
        "K2 gets one rejected plane attempt: {k2_attempts:?}"
    );
    fixture.recorders.atomic.clear_property_rejections();
    retire_pending(&mut fixture, 5);
    assert_cursorless_primary_content_at(&mut fixture, 125, 95);
    assert_software_recompose(&mut fixture, 6);

    let k3 = create_argb_cursor_extent(&mut fixture.backend, [0x35, 0x46, 0x57, 0xff], 32, 4, 1, 0);
    fixture
        .recorders
        .atomic
        .reject_properties(&[width_property]);
    let before_k3 = live_records(&fixture).len();
    Backend::define_cursor(&mut fixture.backend, None, root, k3.as_raw())
        .expect("select K3 through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit K3 qualification");
    let k3_records = live_records(&fixture)[before_k3..].to_vec();
    let k3_attempts = rejected_cursor_attempts(&k3_records, width_property);
    assert_scene_cursor_rejection_retry(&k3_records);
    assert!(
        !fixture.backend.platform.cursor_plane_visible_for_output(0),
        "rejected K3 show and accepted cursorless retry preserve detached ownership",
    );
    assert_eq!(
        k3_attempts.len(),
        1,
        "K3 gets one rejected plane attempt: {k3_attempts:?}"
    );
    fixture.recorders.atomic.clear_property_rejections();
    retire_pending(&mut fixture, 7);
    assert_cursorless_primary_content_at(&mut fixture, 125, 95);
    assert_software_recompose(&mut fixture, 8);

    // Revisiting K1 in the same topology epoch must not forget its earlier
    // rejection when K2 and K3 were observed later.
    let k1_again =
        create_argb_cursor_extent(&mut fixture.backend, [0x67, 0x78, 0x89, 0xff], 1, 1, 0, 0);
    let before_revisit = live_records(&fixture).len();
    Backend::define_cursor(&mut fixture.backend, None, root, k1_again.as_raw())
        .expect("reselect K1 through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("render K1 revisit");
    let revisit = live_records(&fixture)[before_revisit..].to_vec();
    assert!(
        cursor_attach_attempts(&revisit, fb_property, width_property).is_empty(),
        "K1 was already rejected in this topology epoch: {revisit:?}",
    );
    retire_pending(&mut fixture, 6);
    assert_software_recompose(&mut fixture, 9);

    // Pixel-only updates and animation frame versions keep the same key.
    let (base, elapsed_ms) = super::round_a::schedule_rescan_clock(&mut fixture.backend);
    let k3_pixel_update =
        create_argb_cursor_extent(&mut fixture.backend, [0x9a, 0xab, 0xbc, 0xff], 32, 4, 1, 0);
    let before_pixel_update = live_records(&fixture).len();
    Backend::define_cursor(&mut fixture.backend, None, root, k3_pixel_update.as_raw())
        .expect("select same-key pixel update through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("compose same-key pixel update");
    assert!(
        cursor_attach_attempts(
            &live_records(&fixture)[before_pixel_update..],
            fb_property,
            width_property,
        )
        .is_empty(),
        "pixel changes do not requalify a rejected key",
    );
    retire_pending(&mut fixture, 11);
    assert_software_recompose(&mut fixture, 12);

    let animation_frame_k2 =
        create_argb_cursor_extent(&mut fixture.backend, [0x10, 0x20, 0x30, 0xff], 32, 4, 0, 0);
    let animation_frame_k3 =
        create_argb_cursor_extent(&mut fixture.backend, [0xe0, 0xd0, 0xc0, 0xff], 32, 4, 1, 0);
    let animation_frame_k1 =
        create_argb_cursor_extent(&mut fixture.backend, [0x70, 0x80, 0x90, 0xff], 1, 1, 0, 0);
    let animation = Backend::create_anim_cursor(
        &mut fixture.backend,
        None,
        &[
            (animation_frame_k2, 20),
            (animation_frame_k3, 20),
            (animation_frame_k1, 20),
        ],
    )
    .expect("create animation through Backend")
    .expect("animation handle");
    Backend::define_cursor(&mut fixture.backend, None, root, animation.as_raw())
        .expect("select same-key animation through Backend");
    let animation_version = fixture.backend.cursor_records[&animation.as_raw()].version;
    let before_animation = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("submit animation frame zero");
    retire_pending(&mut fixture, 13);
    let mut prior_animation_version = animation_version;
    let mut animation_keys = Vec::new();
    for tick in 0..9 {
        let deadline = Backend::next_wakeup(&fixture.backend).expect("animation frame deadline");
        elapsed_ms.set(
            u64::try_from(deadline.saturating_duration_since(base).as_millis())
                .expect("animation milliseconds fit"),
        );
        let before_tick = live_records(&fixture).len();
        Backend::maybe_composite(&mut fixture.backend)
            .expect("advance animation through Backend scheduler");
        let record = &fixture.backend.cursor_records[&animation.as_raw()];
        assert_ne!(
            record.version, prior_animation_version,
            "Backend scheduler advanced animation tick {tick}",
        );
        prior_animation_version = record.version;
        let key = (record.width, record.height, record.hot_x, record.hot_y);
        animation_keys.push(key);
        let expected_key = match tick % 3 {
            0 => (32, 4, 1, 0),
            1 => (1, 1, 0, 0),
            _ => (32, 4, 0, 0),
        };
        assert_eq!(key, expected_key, "animation alternates rejected keys");

        let tick_records = live_records(&fixture)[before_tick..].to_vec();
        assert!(
            cursor_attach_attempts(&tick_records, fb_property, width_property).is_empty(),
            "animation tick {tick} did not retry a rejected key: {tick_records:?}",
        );
        assert!(
            !fixture.backend.platform.cursor_plane_visible_for_output(0),
            "animation tick {tick} retains detached cursor ownership",
        );
        let accepted_primaries = tick_records
            .iter()
            .filter(|record| {
                record.kind != AtomicCommitKind::CursorOnly && record.result == ScriptedResult::Ok
            })
            .collect::<Vec<_>>();
        assert_eq!(
            accepted_primaries.len(),
            1,
            "animation tick {tick} accepts one software scene frame: {tick_records:?}",
        );
        assert_cursorless_primary(
            accepted_primaries[0],
            &output_cursor_handles(&fixture.backend),
        );
        retire_pending(&mut fixture, 14 + tick);
        assert_accepted_software_primary_content(&mut fixture);
    }
    assert_eq!(
        animation_keys,
        vec![
            (32, 4, 1, 0),
            (1, 1, 0, 0),
            (32, 4, 0, 0),
            (32, 4, 1, 0),
            (1, 1, 0, 0),
            (32, 4, 0, 0),
            (32, 4, 1, 0),
            (1, 1, 0, 0),
            (32, 4, 0, 0),
        ],
        "scheduler repeatedly alternates among K1, K2, and K3",
    );
    assert!(
        cursor_attach_attempts(
            &live_records(&fixture)[before_animation..],
            fb_property,
            width_property,
        )
        .is_empty(),
        "animation versions do not requalify already-rejected parameter keys",
    );
    assert_software_recompose(&mut fixture, 15);

    // Repeated successful SW retirements cannot make any rejected key
    // eligible again, and rejected keys do not schedule retry work by
    // themselves.
    let before_sw_retirements = live_records(&fixture).len();
    for frame in 20..52 {
        retire_software_scene_frame(&mut fixture, frame);
    }
    assert!(
        cursor_attach_attempts(
            &live_records(&fixture)[before_sw_retirements..],
            fb_property,
            width_property,
        )
        .is_empty(),
        "32 SW retirements do not requalify K1, K2, or K3",
    );
    Backend::define_cursor(&mut fixture.backend, None, root, k1_again.as_raw())
        .expect("select static rejected key to check idle wakeups");
    Backend::maybe_composite(&mut fixture.backend).expect("settle static rejected key");
    retire_pending(&mut fixture, 52);
    let now = fixture.backend.monotonic_now();
    assert!(
        Backend::next_wakeup(&fixture.backend).is_none_or(|deadline| deadline > now),
        "all rejected keys leave no due idle retry wakeup",
    );

    // Transparent/hidden changes and lifecycle/presentation policy edges do
    // not create a fresh qualification opportunity for the unchanged key.
    let epoch_before_boundaries = fixture.backend.platform.devices[0]
        .cursor
        .cursor_topology_epoch;
    let before_boundaries = live_records(&fixture).len();
    let transparent = create_argb_cursor_extent(&mut fixture.backend, [0, 0, 0, 0], 1, 1, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, transparent.as_raw())
        .expect("select transparent same-key cursor");
    Backend::maybe_composite(&mut fixture.backend).expect("settle transparent cursor");
    retire_pending(&mut fixture, 60);
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("settle hidden cursor");
    retire_pending(&mut fixture, 61);
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    Backend::define_cursor(&mut fixture.backend, None, root, k1_again.as_raw())
        .expect("restore rejected key after hidden toggle");
    Backend::maybe_composite(&mut fixture.backend).expect("settle visible rejected key");
    retire_pending(&mut fixture, 62);

    Backend::set_dpms_power(&mut fixture.backend, 3).expect("DPMS off through Backend");
    Backend::set_dpms_power(&mut fixture.backend, 0).expect("DPMS on through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("settle unchanged-topology DPMS cycle");
    retire_pending(&mut fixture, 63);
    assert!(
        cursor_attach_attempts(
            &live_records(&fixture)[before_boundaries..],
            fb_property,
            width_property,
        )
        .is_empty(),
        "DPMS cycle does not retry a rejected key"
    );
    fixture
        .backend
        .for_tests_set_vt_resume_executor(Rc::new(LogicalVtResume));
    Backend::finish_vt_release(
        &mut fixture.backend,
        &mut fixture.state,
        &yserver_core::core_loop::input_inventory::InputInventory::default(),
    );
    Backend::on_vt_acquire(&mut fixture.backend, &mut fixture.state);
    Backend::maybe_composite(&mut fixture.backend).expect("settle unchanged-topology VT cycle");
    retire_pending(&mut fixture, 64);
    assert!(
        cursor_attach_attempts(
            &live_records(&fixture)[before_boundaries..],
            fb_property,
            width_property,
        )
        .is_empty(),
        "VT cycle does not retry a rejected key"
    );
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .cursor_topology_epoch,
        epoch_before_boundaries,
        "DPMS and VT cycles with unchanged topology preserve the epoch",
    );

    let tearing_window = 0x7c10;
    fixture.state = super::super::tearing::enter_tearing_without_direct_scanout(
        &mut fixture.backend,
        tearing_window,
        20_000,
    );
    assert!(fixture.backend.pstate.tearing.is_on());
    Backend::unmap_subwindow(&mut fixture.backend, None, tearing_window)
        .expect("exit tearing through Backend unmap");
    Backend::maybe_composite(&mut fixture.backend).expect("settle tearing exit");
    retire_pending(&mut fixture, 65);
    assert!(!fixture.backend.pstate.tearing.is_on());
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .cursor_topology_epoch,
        epoch_before_boundaries,
        "tearing On/Off with unchanged topology preserves the epoch",
    );
    assert!(
        cursor_attach_attempts(
            &live_records(&fixture)[before_boundaries..],
            fb_property,
            width_property,
        )
        .is_empty(),
        "transparent, hidden, DPMS, VT, and tearing do not retry rejected keys",
    );

    // A real changed cursor-plane assignment reached through Backend hotplug
    // advances the device epoch and gives each remembered key one new attempt.
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let (base, elapsed_ms) = super::round_a::schedule_rescan_clock(&mut fixture.backend);
    let mut property_names = super::round_a::ATOMIC_CURSOR_PROPERTIES.to_vec();
    property_names.extend(["HOTSPOT_X", "HOTSPOT_Y"]);
    let new_width_property = ::drm::control::from_u32(0x9004).expect("new CRTC_W property");
    fixture
        .recorders
        .atomic
        .reject_properties(&[new_width_property]);
    assert!(
        fixture
            .backend
            .platform
            .for_tests_queue_cursor_discovery_snapshot(
                device_key,
                super::round_a::cursor_topology_snapshot(
                    "atomic-cursor-test",
                    true,
                    &[1],
                    &[(0x9000, vec![1], property_names)],
                ),
            )
    );
    let output = &fixture.backend.platform.outputs[0];
    fixture.recorders.connector_snapshot.queue_snapshot(vec![
        crate::kms::render::platform::ConnectorSnapshot {
            key: output.key.clone(),
            modes: vec![output.output.picked.clone()],
            mm_width: 0,
            mm_height: 0,
            edid: Vec::new(),
            connector_type: "test".to_owned(),
        },
    ]);
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue topology edge");
    let epoch_before_topology = fixture.backend.platform.devices[0]
        .cursor
        .cursor_topology_epoch;
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    assert_eq!(
        fixture.backend.pstate.hotplug_rescan_deadline,
        Some(base + Duration::from_millis(150)),
    );
    elapsed_ms.set(150);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .cursor_topology_epoch,
        epoch_before_topology + 1,
        "changed cursor-plane topology advances exactly one epoch",
    );
    retire_pending(&mut fixture, 66);

    let current_properties = cursor_plane_properties(&fixture, 0);
    assert_eq!(current_properties.crtc_w, new_width_property);
    let new_fb_property = current_properties.fb_id;
    let before_new_epoch = live_records(&fixture).len();
    let k1_epoch =
        create_argb_cursor_extent(&mut fixture.backend, [0x12, 0x34, 0x56, 0xff], 1, 1, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, k1_epoch.as_raw())
        .expect("reselect K1 after changed topology");
    Backend::maybe_composite(&mut fixture.backend).expect("qualify K1 in new epoch");
    let k1_epoch_records = live_records(&fixture)[before_new_epoch..].to_vec();
    let k1_epoch_attempts =
        cursor_attach_attempts(&k1_epoch_records, new_fb_property, new_width_property);
    assert_scene_cursor_rejection_retry(&k1_epoch_records);
    assert_eq!(
        k1_epoch_attempts.len(),
        1,
        "K1 gets one new-epoch attempt: {k1_epoch_attempts:?}"
    );
    retire_pending(&mut fixture, 67);
    assert_software_recompose(&mut fixture, 68);

    let before_k2_epoch = live_records(&fixture).len();
    Backend::define_cursor(&mut fixture.backend, None, root, k2.as_raw())
        .expect("reselect K2 after changed topology");
    Backend::maybe_composite(&mut fixture.backend).expect("qualify K2 in new epoch");
    let k2_epoch_records = live_records(&fixture)[before_k2_epoch..].to_vec();
    let k2_epoch_attempts =
        cursor_attach_attempts(&k2_epoch_records, new_fb_property, new_width_property);
    assert_scene_cursor_rejection_retry(&k2_epoch_records);
    assert_eq!(
        k2_epoch_attempts.len(),
        1,
        "K2 gets one new-epoch attempt: {k2_epoch_attempts:?}"
    );
    retire_pending(&mut fixture, 69);
    assert_software_recompose(&mut fixture, 70);

    let before_k3_epoch = live_records(&fixture).len();
    Backend::define_cursor(&mut fixture.backend, None, root, k3.as_raw())
        .expect("reselect K3 after changed topology");
    Backend::maybe_composite(&mut fixture.backend).expect("qualify K3 in new epoch");
    let k3_epoch_records = live_records(&fixture)[before_k3_epoch..].to_vec();
    let k3_epoch_attempts =
        cursor_attach_attempts(&k3_epoch_records, new_fb_property, new_width_property);
    assert_scene_cursor_rejection_retry(&k3_epoch_records);
    assert_eq!(
        k3_epoch_attempts.len(),
        1,
        "K3 gets one new-epoch attempt: {k3_epoch_attempts:?}"
    );
    retire_pending(&mut fixture, 71);
    assert_software_recompose(&mut fixture, 72);

    let before_k1_epoch_revisit = live_records(&fixture).len();
    Backend::define_cursor(&mut fixture.backend, None, root, k1_epoch.as_raw())
        .expect("revisit K1 in the new epoch");
    Backend::maybe_composite(&mut fixture.backend).expect("settle new-epoch K1 revisit");
    assert!(
        cursor_attach_attempts(
            &live_records(&fixture)[before_k1_epoch_revisit..],
            new_fb_property,
            new_width_property,
        )
        .is_empty()
    );
    fixture.recorders.atomic.clear_property_rejections();
    retire_pending(&mut fixture, 73);
    assert_software_recompose(&mut fixture, 74);
    fixture
        .backend
        .disable_output()
        .expect("Backend teardown detaches the cursor");
    fixture.assert_cursor_quiescent();
    let first_ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(first_ledger.end_state(), (0, 0, 0));

    // Qualification is local to one CRTC even when a single sprite spans two
    // outputs. A's refused cursor-only move detaches A; B accepts the same key
    // and continues to qualify it on the next image update.
    let Some(mut peer) = AtomicCursorFixture::new_with_two_outputs()
        .expect("construct two-output qualification fixture")
    else {
        return;
    };
    peer.backend
        .platform
        .for_tests_accept_output_power_commits();
    let peer_root = peer.backend.core.window_id;
    let shared_key =
        create_argb_cursor_extent(&mut peer.backend, [0x45, 0x56, 0x67, 0xff], 32, 4, 0, 0);
    Backend::define_cursor(&mut peer.backend, None, peer_root, shared_key.as_raw())
        .expect("select shared CRTC key");
    Backend::warp_pointer_root(&mut peer.backend, &mut peer.state, 790, 90);
    damage_cursor_root(&mut peer);
    Backend::maybe_composite(&mut peer.backend).expect("submit spanning cursor frame");
    retire_pending(&mut peer, 100);
    assert!(peer.backend.platform.cursor_plane_visible_for_output(0));
    assert!(peer.backend.platform.cursor_plane_visible_for_output(1));

    let output_a = cursor_plane_properties(&peer, 0);
    let output_b = cursor_plane_properties(&peer, 1);
    assert_ne!(output_a.plane, output_b.plane);
    peer.recorders.atomic.reject_properties(&[output_a.crtc_x]);
    let before_peer_rejection = live_records(&peer).len();
    Backend::warp_pointer_root(&mut peer.backend, &mut peer.state, 791, 90);
    Backend::maybe_composite(&mut peer.backend).expect("reject only output A's cursor move");
    let peer_rejection_records = live_records(&peer)[before_peer_rejection..].to_vec();
    assert_eq!(
        peer_rejection_records.len(),
        3,
        "output A rejection, output A detach, then output B acceptance: {peer_rejection_records:?}",
    );
    assert_eq!(peer_rejection_records[0].kind, AtomicCommitKind::CursorOnly);
    assert!(matches!(
        peer_rejection_records[0].result,
        ScriptedResult::Error { .. }
    ));
    assert!(peer_rejection_records[0].properties.iter().any(|update| {
        update.object == output_a.plane.into() && update.property == output_a.crtc_x
    }));
    assert_eq!(peer_rejection_records[1].kind, AtomicCommitKind::CursorOnly);
    assert_eq!(peer_rejection_records[1].result, ScriptedResult::Ok);
    assert!(peer_rejection_records[1].properties.iter().any(|update| {
        update.object == output_a.plane.into()
            && update.property == output_a.fb_id
            && update.value == 0
    }));
    assert_eq!(peer_rejection_records[2].kind, AtomicCommitKind::CursorOnly);
    assert_eq!(peer_rejection_records[2].result, ScriptedResult::Ok);
    assert!(peer_rejection_records[2].properties.iter().any(|update| {
        update.object == output_b.plane.into() && update.property == output_b.crtc_x
    }));
    assert_eq!(
        peer_rejection_records
            .iter()
            .filter(|record| {
                matches!(record.result, ScriptedResult::Error { .. })
                    && record.properties.iter().any(|update| {
                        update.object == output_a.plane.into() && update.property == output_a.crtc_x
                    })
            })
            .count(),
        1,
        "output A has one refused request",
    );
    assert!(
        peer_rejection_records.iter().any(|record| {
            record.result == ScriptedResult::Ok
                && record.properties.iter().any(|update| {
                    update.object == output_b.plane.into() && update.property == output_b.crtc_x
                })
        }),
        "output B accepted the same-key motion",
    );
    retire_pending(&mut peer, 101);
    assert!(!peer.backend.platform.cursor_plane_visible_for_output(0));
    assert!(peer.backend.platform.cursor_plane_visible_for_output(1));

    peer.recorders.atomic.clear_property_rejections();
    let same_key_pixels =
        create_argb_cursor_extent(&mut peer.backend, [0x88, 0x99, 0xaa, 0xff], 32, 4, 0, 0);
    let before_peer_version = live_records(&peer).len();
    Backend::define_cursor(&mut peer.backend, None, peer_root, same_key_pixels.as_raw())
        .expect("update pixels for the shared key");
    Backend::maybe_composite(&mut peer.backend).expect("settle output-local eligibility");
    let peer_version_records = live_records(&peer)[before_peer_version..].to_vec();
    assert!(
        updates_for_plane(&peer_version_records, output_a).is_empty(),
        "output A does not requalify its rejected key",
    );
    assert!(
        updates_for_plane(&peer_version_records, output_b)
            .iter()
            .any(|record| record.result == ScriptedResult::Ok),
        "output B remains independently eligible for the shared key",
    );
    retire_pending(&mut peer, 102);
    assert!(
        peer.backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    let under_a = read_output_pixel(&mut peer.backend, 791, 90);
    let beside_a = read_output_pixel(&mut peer.backend, 760, 120);
    assert_ne!(
        under_a[..3],
        beside_a[..3],
        "output A primary contains its SW cursor"
    );

    // Detach B through Backend, then prove a later attach is still eligible
    // there while A remains suppressed for the same rejected key.
    let before_peer_hide = live_records(&peer).len();
    Backend::set_cursor_hidden(&mut peer.backend, true);
    Backend::maybe_composite(&mut peer.backend).expect("hide the still-qualified output B cursor");
    let peer_hide_records = live_records(&peer)[before_peer_hide..].to_vec();
    let peer_b_hide = updates_for_plane(&peer_hide_records, output_b);
    assert_eq!(
        peer_b_hide.len(),
        1,
        "B's accepted hide: {peer_hide_records:?}"
    );
    assert_eq!(peer_b_hide[0].result, ScriptedResult::Ok);
    assert!(
        peer_b_hide[0]
            .properties
            .iter()
            .any(|update| { update.property == output_b.fb_id && update.value == 0 })
    );
    retire_pending(&mut peer, 103);
    assert!(!peer.backend.platform.cursor_plane_visible_for_output(0));
    assert!(!peer.backend.platform.cursor_plane_visible_for_output(1));

    let before_peer_show = live_records(&peer).len();
    Backend::set_cursor_hidden(&mut peer.backend, false);
    Backend::maybe_composite(&mut peer.backend).expect("requalify only output B after its hide");
    let peer_show_records = live_records(&peer)[before_peer_show..].to_vec();
    assert!(
        cursor_attach_attempts(&peer_show_records, output_a.fb_id, output_a.crtc_w).is_empty(),
        "A's rejected key stays software after the global visibility toggle: {peer_show_records:?}",
    );
    let peer_b_attach = cursor_attach_attempts(&peer_show_records, output_b.fb_id, output_b.crtc_w);
    assert_eq!(
        peer_b_attach.len(),
        1,
        "B gets a fresh attach: {peer_show_records:?}"
    );
    assert_eq!(peer_b_attach[0].result, ScriptedResult::Ok);
    assert_eq!(peer_b_attach[0].events.len(), 1);
    retire_pending(&mut peer, 104);
    assert!(!peer.backend.platform.cursor_plane_visible_for_output(0));
    assert!(peer.backend.platform.cursor_plane_visible_for_output(1));
    assert!(
        peer.backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    let under_a_after_rebind = read_output_pixel(&mut peer.backend, 791, 90);
    let beside_a_after_rebind = read_output_pixel(&mut peer.backend, 760, 120);
    assert_ne!(
        under_a_after_rebind[..3],
        beside_a_after_rebind[..3],
        "output A keeps composited cursor pixels while B reattaches",
    );

    peer.backend
        .disable_output()
        .expect("Backend teardown both output cursor bindings");
    peer.assert_cursor_quiescent();
    let peer_ledger = Rc::clone(&peer.recorders.cursor_buffer_ledger);
    drop(peer);
    assert_eq!(peer_ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit qualified rejection recording in ordinary sprite uploads;
// the repeated same-key Backend display must then consume the queued EIO.
fn atomic_cursor_only_upload_rejection_is_classified_per_sprite_key() {
    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor scene fixture") else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let root = fixture.backend.core.window_id;
    let fb_property = output_cursor_handles(&fixture.backend)
        .into_iter()
        .find(|(name, _)| *name == "fb_id")
        .expect("cursor FB_ID property")
        .1;
    let width_property = output_cursor_handles(&fixture.backend)
        .into_iter()
        .find(|(name, _)| *name == "crtc_w")
        .expect("cursor CRTC_W property")
        .1;

    // Resource and busy errors leave this key eligible. Each attempt is made
    // through Backend::define_cursor while the original HW binding remains.
    for (index, raw_os_error) in [libc::EBUSY, libc::ENOMEM, libc::EIO]
        .into_iter()
        .enumerate()
    {
        fixture.recorders.cursor_buffer_ledger.fail_after(
            crate::kms::cursor_plane::ops::CursorBufferOperation::Write,
            0,
            raw_os_error,
        );
        let pixel = [u8::try_from(0x20 + index).unwrap(), 0x40, 0x60, 0xff];
        let cursor = create_argb_cursor_extent(&mut fixture.backend, pixel, 1, 1, 0, 0);
        Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
            .expect("display resource-error cursor through Backend");
        assert!(
            fixture
                .backend
                .platform
                .cursor_plane_available_for_output(0)
        );
        assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
        assert_eq!(
            fixture
                .recorders
                .cursor_buffer_ledger
                .failures_taken()
                .len(),
            index + 1,
            "each unrelated upload error was observed",
        );
    }

    // EINVAL disqualifies the captured K1 candidate. Preserve the accepted
    // hardware binding until a real scene frame accepts the detach.
    fixture.recorders.cursor_buffer_ledger.fail_after(
        crate::kms::cursor_plane::ops::CursorBufferOperation::Write,
        0,
        libc::EINVAL,
    );
    let rejected =
        create_argb_cursor_extent(&mut fixture.backend, [0xe0, 0x30, 0x50, 0xff], 1, 1, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, rejected.as_raw())
        .expect("display EINVAL candidate through Backend");
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0),
        "raw EINVAL suppresses the submitted sprite key in this epoch",
    );
    assert!(
        fixture.backend.platform.cursor_plane_visible_for_output(0),
        "upload rejection does not claim the accepted HW binding was detached",
    );
    assert_eq!(
        fixture
            .recorders
            .cursor_buffer_ledger
            .failures_taken()
            .len(),
        4,
        "the classified EINVAL write was observed",
    );

    let before_detach = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("accept software scene and detach");
    let detach_records = live_records(&fixture)[before_detach..].to_vec();
    assert_eq!(
        detach_records.len(),
        1,
        "one accepted scene detach: {detach_records:?}"
    );
    assert_eq!(detach_records[0].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(detach_records[0].result, ScriptedResult::Ok);
    assert_eq!(detach_records[0].events.len(), 1);
    assert_cursorless_primary(&detach_records[0], &output_cursor_handles(&fixture.backend));
    assert!(
        detach_records[0]
            .properties
            .iter()
            .any(|update| { update.property == fb_property && update.value == 0 })
    );
    assert!(
        !fixture.backend.platform.cursor_plane_visible_for_output(0),
        "accepted scene detach clears applied HW ownership",
    );
    retire_pending(&mut fixture, 200);
    let under_pointer = read_output_pixel(&mut fixture.backend, 100, 90);
    let beside_pointer = read_output_pixel(&mut fixture.backend, 160, 130);
    assert_ne!(
        under_pointer[..3],
        beside_pointer[..3],
        "accepted software replacement contains the rejected sprite pixels",
    );

    // Repeated pixel versions share K1. An attempted ordinary reupload would
    // consume the queued EIO, while a key-based suppression leaves it unused.
    fixture.recorders.cursor_buffer_ledger.fail_after(
        crate::kms::cursor_plane::ops::CursorBufferOperation::Write,
        0,
        libc::EIO,
    );
    let repeated =
        create_argb_cursor_extent(&mut fixture.backend, [0x30, 0xd0, 0x40, 0xff], 1, 1, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, repeated.as_raw())
        .expect("revisit rejected K1 through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("compose rejected K1 in software");
    assert_eq!(
        fixture
            .recorders
            .cursor_buffer_ledger
            .failures_taken()
            .len(),
        4,
        "the rejected key prevents another ordinary upload attempt",
    );
    let repeated_records = live_records(&fixture);
    assert!(
        cursor_attach_attempts(
            &repeated_records[before_detach + detach_records.len()..],
            fb_property,
            width_property
        )
        .is_empty(),
        "same-key pixel update does not attempt a plane attach",
    );
    assert!(
        !fixture.backend.platform.cursor_plane_visible_for_output(0),
        "same-key pixel update retains detached ownership",
    );
    retire_pending(&mut fixture, 201);
    assert_accepted_software_primary_content_at(&mut fixture, 100, 90);

    fixture
        .backend
        .disable_output()
        .expect("Backend teardown drains the cursor state");
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

fn retire_pending(fixture: &mut super::support::AtomicCursorFixture, frame: u32) {
    for output_idx in 0..fixture.backend.platform.outputs.len() {
        if fixture.backend.scene.has_pending_page_flip(output_idx)
            || fixture
                .backend
                .platform
                .cursor_atomic_event_pending_for_output(output_idx)
        {
            fixture.retire_output(output_idx, frame + u32::try_from(output_idx).unwrap_or(0));
        }
    }
}
