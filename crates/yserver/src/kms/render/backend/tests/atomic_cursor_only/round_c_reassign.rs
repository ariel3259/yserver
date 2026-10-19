//! Live cursor-plane inventory reassignment while both primary routes stay lit.

use std::{
    cell::Cell,
    os::fd::{AsFd as _, AsRawFd as _},
    rc::Rc,
    time::Duration,
};

use super::{
    round_a::{ATOMIC_CURSOR_PROPERTIES, cursor_topology_snapshot, damage_cursor_root},
    support::AtomicCursorFixture,
};
use crate::{
    drm::atomic::{
        AtomicCommitKind,
        tests_support::{AtomicCommitRecord, ScriptedResult},
    },
    kms::render::{
        platform::{
            CrtcKey, cursor_reassign::CursorPlaneKey,
            cursor_recovery::RecordingCursorRecoveryReporter, cursor_state::CursorBinding,
        },
        scene::{CursorAcceptedMode, CursorPrimaryContent},
    },
};
use ::drm::control::{AtomicCommitFlags, plane, property};
use yserver_core::backend::Backend;

fn live_records(fixture: &AtomicCursorFixture) -> Vec<AtomicCommitRecord> {
    fixture
        .recorders
        .atomic
        .records()
        .into_iter()
        .filter(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY))
        .collect()
}

fn live_records_from(
    atomic: &crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor,
) -> Vec<AtomicCommitRecord> {
    atomic
        .records()
        .into_iter()
        .filter(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY))
        .collect()
}

fn property_value(
    record: &AtomicCommitRecord,
    object: impl Into<::drm::control::RawResourceHandle>,
    property: property::Handle,
) -> Option<u64> {
    let object = object.into();
    record.properties.iter().find_map(|update| {
        (update.object == object && update.property == property).then_some(update.value)
    })
}

fn output_binding(fixture: &AtomicCursorFixture, output_idx: usize) -> Option<CursorBinding> {
    let output = &fixture.backend.platform.outputs[output_idx];
    fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
        .and_then(|state| state.applied)
}

fn output_records(
    records: &[AtomicCommitRecord],
    fixture: &AtomicCursorFixture,
    output_idx: usize,
) -> Vec<AtomicCommitRecord> {
    let output = &fixture.backend.platform.outputs[output_idx].output;
    let device_fd = fixture
        .backend
        .platform
        .device_for_key(fixture.backend.platform.outputs[output_idx].key.device_key)
        .expect("output DRM device")
        .device
        .as_fd()
        .as_raw_fd();
    records
        .iter()
        .filter(|record| {
            record.device_fd == device_fd
                && (record
                    .properties
                    .iter()
                    .any(|update| update.object == output.plane.into())
                    || record.events.iter().any(|event| event.crtc == output.crtc))
        })
        .cloned()
        .collect()
}

fn accepted_primary_framebuffers(
    records: &[AtomicCommitRecord],
    output: &crate::kms::backend::ActiveOutput,
) -> Vec<u64> {
    records
        .iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .filter_map(|record| {
            property_value(record, output.output.plane, output.output.plane_fb_id_prop)
        })
        .collect()
}

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
        .expect("output has a currently assigned cursor plane")
}

fn cursor_property_handles(
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
) -> Vec<property::Handle> {
    let mut handles = vec![
        properties.fb_id,
        properties.crtc_id,
        properties.crtc_x,
        properties.crtc_y,
        properties.crtc_w,
        properties.crtc_h,
        properties.src_x,
        properties.src_y,
        properties.src_w,
        properties.src_h,
    ];
    handles.extend(properties.hotspot_x);
    handles.extend(properties.hotspot_y);
    handles
}

fn record_sets_cursor_property(
    record: &AtomicCommitRecord,
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
) -> bool {
    let object = properties.plane.into();
    let handles = cursor_property_handles(properties);
    record
        .properties
        .iter()
        .any(|update| update.object == object && handles.contains(&update.property))
}

fn cursor_lease_users(
    fixture: &AtomicCursorFixture,
    lease: crate::kms::cursor_plane::sprites::SpriteBufferLease,
) -> usize {
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    fixture
        .backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.sprite_user_count(lease))
        .unwrap_or(0)
}

fn cursor_lease_users_for_output(
    fixture: &AtomicCursorFixture,
    output_idx: usize,
    lease: crate::kms::cursor_plane::sprites::SpriteBufferLease,
) -> usize {
    let output = &fixture.backend.platform.outputs[output_idx];
    fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.sprite_user_count(lease))
        .unwrap_or(0)
}

fn randr_output_for_connector(
    fixture: &mut AtomicCursorFixture,
    connector: &str,
) -> yserver_core::randr::RandrOutput {
    Backend::randr_outputs_and_modes(&mut fixture.backend)
        .0
        .into_iter()
        .find(|output| output.name == connector)
        .expect("RandR output for connector")
}

fn queue_inventory_rescan(
    fixture: &mut AtomicCursorFixture,
    elapsed_ms: &Cell<u64>,
    plane_handle: plane::Handle,
    possible_crtc: ::drm::control::crtc::Handle,
) {
    queue_cursor_topology_rescan(fixture, elapsed_ms, &[(plane_handle, possible_crtc)]);
}

fn queue_cursor_topology_rescan(
    fixture: &mut AtomicCursorFixture,
    elapsed_ms: &Cell<u64>,
    assignments: &[(plane::Handle, ::drm::control::crtc::Handle)],
) {
    queue_cursor_topology_snapshot(fixture, assignments);
    trigger_inventory_rescan(fixture, elapsed_ms);
}

fn queue_cursor_topology_snapshot(
    fixture: &mut AtomicCursorFixture,
    assignments: &[(plane::Handle, ::drm::control::crtc::Handle)],
) {
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let crtcs = fixture
        .backend
        .platform
        .outputs
        .iter()
        .map(|output| u32::from(output.output.crtc))
        .collect::<Vec<_>>();
    let planes = assignments
        .iter()
        .map(|(plane, crtc)| {
            (
                u32::from(*plane),
                vec![u32::from(*crtc)],
                ATOMIC_CURSOR_PROPERTIES.to_vec(),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        fixture
            .backend
            .platform
            .for_tests_queue_cursor_discovery_snapshot(
                device_key,
                cursor_topology_snapshot("atomic-cursor-test", true, &crtcs, &planes,),
            )
    );
}

fn trigger_inventory_rescan(fixture: &mut AtomicCursorFixture, elapsed_ms: &Cell<u64>) {
    fixture.recorders.connector_snapshot.queue_snapshot(
        fixture
            .backend
            .platform
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
            .collect(),
    );
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue backend hotplug input");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    assert!(
        fixture.backend.pstate.hotplug_rescan_deadline.is_some(),
        "Backend observes the queued display hotplug edge",
    );
    elapsed_ms.set(elapsed_ms.get() + 150);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert!(
        fixture.backend.pstate.hotplug_rescan_deadline.is_none(),
        "Backend runs the matured display rescan",
    );
}

fn queue_connector_removal_rescan(
    fixture: &mut AtomicCursorFixture,
    elapsed_ms: &Cell<u64>,
    retained_output_indices: &[usize],
) {
    let device_keys: std::collections::HashSet<_> = fixture
        .backend
        .platform
        .outputs
        .iter()
        .map(|output| output.key.device_key)
        .collect();
    for device_key in device_keys {
        let current_routes: Vec<_> = fixture
            .backend
            .platform
            .outputs
            .iter()
            .filter(|output| output.key.device_key == device_key)
            .filter_map(|output| {
                fixture
                    .backend
                    .platform
                    .device_for_key(output.key.device_key)
                    .and_then(|device| device.cursor.plane.as_ref())
                    .and_then(|plane| plane.properties_for(output.output.crtc))
                    .map(|properties| (output.output.crtc, properties))
            })
            .collect();
        let crtcs = fixture
            .backend
            .platform
            .outputs
            .iter()
            .filter(|output| output.key.device_key == device_key)
            .map(|output| u32::from(output.output.crtc))
            .collect::<Vec<_>>();
        let planes = current_routes
            .iter()
            .map(|(crtc, properties)| {
                (
                    u32::from(properties.plane),
                    vec![u32::from(*crtc)],
                    ATOMIC_CURSOR_PROPERTIES.to_vec(),
                )
            })
            .collect::<Vec<_>>();
        assert!(
            fixture
                .backend
                .platform
                .for_tests_queue_cursor_discovery_snapshot(
                    device_key,
                    cursor_topology_snapshot("atomic-cursor-test", true, &crtcs, &planes),
                ),
            "cursor topology is queued for every active DRM device: {device_key:?}",
        );
    }
    let retained = retained_output_indices
        .iter()
        .map(|&index| {
            let output = &fixture.backend.platform.outputs[index];
            crate::kms::render::platform::ConnectorSnapshot {
                key: output.key.clone(),
                modes: vec![output.output.picked.clone()],
                mm_width: 0,
                mm_height: 0,
                edid: Vec::new(),
                connector_type: "test".to_owned(),
            }
        })
        .collect();
    fixture
        .recorders
        .connector_snapshot
        .queue_snapshot(retained);
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue backend hotplug input");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    assert!(
        fixture.backend.pstate.hotplug_rescan_deadline.is_some(),
        "Backend observes the queued connector-removal edge",
    );
    elapsed_ms.set(elapsed_ms.get() + 150);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert!(
        fixture.backend.pstate.hotplug_rescan_deadline.is_none(),
        "Backend runs the matured connector-removal rescan",
    );
}

fn retire_if_pending(fixture: &mut AtomicCursorFixture, output_idx: usize, frame: u32) {
    if fixture.backend.scene.has_pending_page_flip(output_idx)
        || fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(output_idx)
    {
        fixture.retire_output(output_idx, frame);
    }
}

fn drive_scene_retry_deadline_through_backend(fixture: &mut AtomicCursorFixture) {
    let Some(deadline) = fixture.backend.scene.earliest_retry_deadline() else {
        return;
    };
    while std::time::Instant::now() < deadline {
        let _ = Backend::maybe_composite(&mut fixture.backend);
    }
}

fn drive_backend_until_scene_cursor_frames(
    fixture: &mut AtomicCursorFixture,
    records_before: usize,
    output_indices: &[usize],
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let records = live_records(fixture);
        if output_indices.iter().all(|output_idx| {
            output_records(&records[records_before..], fixture, *output_idx)
                .iter()
                .any(|record| {
                    record.kind == AtomicCommitKind::SceneWithCursor
                        && record.result == ScriptedResult::Ok
                })
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "scene cursor frames did not reach the atomic sink: {records:?}",
        );
        let _ = Backend::maybe_composite(&mut fixture.backend);
        Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
        std::thread::yield_now();
    }
}

fn read_pixel(fixture: &mut AtomicCursorFixture, x: i32, y: i32) -> [u8; 4] {
    read_pixel_with_selection(fixture, x, y, SwapFrameReadSelection::PermissiveDump)
}

#[derive(Clone, Copy)]
enum SwapFrameReadSelection {
    LatestComposedFrame,
    PermissiveDump,
}

fn read_pixel_with_selection(
    fixture: &mut AtomicCursorFixture,
    x: i32,
    y: i32,
    selection: SwapFrameReadSelection,
) -> [u8; 4] {
    let rect = ash::vk::Rect2D {
        offset: ash::vk::Offset2D { x, y },
        extent: ash::vk::Extent2D {
            width: 1,
            height: 1,
        },
    };
    let bytes = match selection {
        SwapFrameReadSelection::LatestComposedFrame => {
            crate::kms::render::backend::read_latest_composed_region(&mut fixture.backend, rect)
        }
        SwapFrameReadSelection::PermissiveDump => crate::kms::render::backend::read_scanout_region(
            &mut fixture.backend,
            rect,
            crate::kms::render::backend::ScanoutReadSelection::PermissiveDump,
        ),
    };
    bytes
        .expect("read composed cursor pixels")
        .try_into()
        .expect("one BGRA pixel")
}

fn assert_full_cursor_attach(
    record: &AtomicCommitRecord,
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
    crtc: ::drm::control::crtc::Handle,
    crtc_x: i64,
    crtc_y: i64,
) {
    let object: ::drm::control::RawResourceHandle = properties.plane.into();
    assert_eq!(record.kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(record.result, ScriptedResult::Ok);
    assert!(record.events.iter().any(|event| event.crtc == crtc));
    for (property, expected) in [
        (properties.crtc_id, u64::from(u32::from(crtc))),
        (properties.crtc_x, crtc_x as u64),
        (properties.crtc_y, crtc_y as u64),
        (properties.src_x, 0),
        (properties.src_y, 0),
        (properties.src_w, 64_u64 << 16),
        (properties.src_h, 64_u64 << 16),
        (properties.crtc_w, 64),
        (properties.crtc_h, 64),
    ] {
        assert_eq!(
            property_value(record, object, property),
            Some(expected),
            "accepted attach property {property:?} has the submitted value: {record:?}",
        );
    }
    assert!(
        property_value(record, object, properties.fb_id)
            .is_some_and(|framebuffer| framebuffer != 0),
        "accepted attach names a framebuffer",
    );
}

fn accepted_primary_output(
    fixture: &AtomicCursorFixture,
    record: &AtomicCommitRecord,
) -> Option<usize> {
    fixture.backend.platform.outputs.iter().position(|output| {
        let device_fd = fixture
            .backend
            .platform
            .device_for_key(output.key.device_key)
            .expect("output DRM device")
            .device
            .as_fd()
            .as_raw_fd();
        record.properties.iter().any(|update| {
            record.device_fd == device_fd
                && update.object == output.output.plane.into()
                && update.property == output.output.plane_fb_id_prop
        })
    })
}

fn submit_next_swap_scene_frame(
    fixture: &mut AtomicCursorFixture,
    expected_output_idx: usize,
    blocked_output_idx: Option<usize>,
) -> AtomicCommitRecord {
    let records_before = live_records(fixture).len();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(blocked_output_idx) = blocked_output_idx {
            fixture.backend.platform.scanout_pools[blocked_output_idx]
                .as_mut()
                .expect("blocked peer owns a primary pool")
                .display_pool_mut()
                .for_tests_script_unavailable_acquisitions([true]);
        }
        let _ = Backend::maybe_composite(&mut fixture.backend);
        if let Some(blocked_output_idx) = blocked_output_idx {
            fixture.backend.platform.scanout_pools[blocked_output_idx]
                .as_mut()
                .expect("blocked peer retains its primary pool")
                .display_pool_mut()
                .unavailable_acquisitions
                .clear();
        }

        let records = live_records(fixture);
        let accepted_primary_frames: Vec<_> = records[records_before..]
            .iter()
            .filter(|record| {
                record.result == ScriptedResult::Ok
                    && accepted_primary_output(fixture, record).is_some()
            })
            .collect();
        assert!(
            accepted_primary_frames.len() <= 1,
            "one Backend scheduling pass accepts at most one primary frame: {accepted_primary_frames:?}",
        );
        if let Some(record) = accepted_primary_frames.first() {
            assert_eq!(
                accepted_primary_output(fixture, record),
                Some(expected_output_idx),
                "the peer's unavailable BO keeps this pass on the expected output: {record:?}",
            );
            assert!(matches!(
                record.kind,
                AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
            ));
            return (*record).clone();
        }

        Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
        assert!(
            std::time::Instant::now() < deadline,
            "output {expected_output_idx} did not submit a scene frame: {records:?}",
        );
        std::thread::yield_now();
    }
}

struct PendingSwapFrame {
    record: AtomicCommitRecord,
    output_idx: usize,
    event_frame: u32,
    hardware_bound: bool,
    hardware_mode: bool,
    cursor_root_position: (i32, i32),
}

fn assert_swap_frame_applied_ownership(
    fixture: &mut AtomicCursorFixture,
    record: &AtomicCommitRecord,
    hardware_owners: &mut std::collections::HashMap<CursorPlaneKey, CrtcKey>,
    plane_p: plane::Handle,
    properties_p: crate::kms::cursor_plane::topology::CursorPlaneProperties,
    plane_q: plane::Handle,
    properties_q: crate::kms::cursor_plane::topology::CursorPlaneProperties,
    crtc_a: ::drm::control::crtc::Handle,
    crtc_b: ::drm::control::crtc::Handle,
    frame: u32,
    cursor_root_position: (i32, i32),
) -> PendingSwapFrame {
    assert_swap_frame_applied_ownership_inner(
        fixture,
        record,
        hardware_owners,
        plane_p,
        properties_p,
        plane_q,
        properties_q,
        crtc_a,
        crtc_b,
        frame,
        cursor_root_position,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn assert_swap_frame_applied_ownership_inner(
    fixture: &mut AtomicCursorFixture,
    record: &AtomicCommitRecord,
    hardware_owners: &mut std::collections::HashMap<CursorPlaneKey, CrtcKey>,
    plane_p: plane::Handle,
    properties_p: crate::kms::cursor_plane::topology::CursorPlaneProperties,
    plane_q: plane::Handle,
    properties_q: crate::kms::cursor_plane::topology::CursorPlaneProperties,
    crtc_a: ::drm::control::crtc::Handle,
    crtc_b: ::drm::control::crtc::Handle,
    frame: u32,
    cursor_root_position: (i32, i32),
    verify_all_output_owners: bool,
) -> PendingSwapFrame {
    assert_eq!(record.result, ScriptedResult::Ok);
    let output_idx = accepted_primary_output(fixture, record)
        .expect("every verified accepted frame updates a primary framebuffer");
    let output = &fixture.backend.platform.outputs[output_idx].output;
    let crtc = output.crtc;
    let output_crtc_key = CrtcKey::for_output(&fixture.backend.platform.outputs[output_idx]);
    assert!(
        record.events.iter().any(|event| event.crtc == crtc),
        "accepted primary frame owns its output event: {record:?}",
    );

    // Apply every accepted request to the model, including frames with no
    // cursor delta. Compare the backend's applied binding before this frame's
    // event drains; the companion rejection row checks ownership before acceptance.
    let cursor_planes = if plane_p == plane_q {
        let properties = [properties_p, properties_q]
            .into_iter()
            .find(|properties| {
                property_value(record, plane_p, properties.fb_id).is_some()
                    && property_value(record, plane_p, properties.crtc_id).is_some()
            })
            .unwrap_or(properties_p);
        vec![(plane_p, properties)]
    } else {
        vec![(plane_p, properties_p), (plane_q, properties_q)]
    };
    let cursor_changes: Vec<_> = cursor_planes
        .into_iter()
        .filter_map(|(plane, properties)| {
            let framebuffer = property_value(record, plane, properties.fb_id)?;
            let owner = property_value(record, plane, properties.crtc_id)?;
            Some((
                CursorPlaneKey {
                    device_key: output_crtc_key.device_key,
                    plane,
                },
                framebuffer,
                owner,
            ))
        })
        .collect();
    assert!(
        cursor_changes.len() <= 1,
        "one frame changes at most one cursor plane: {record:?}",
    );
    if let Some((plane_key, framebuffer, owner)) = cursor_changes.first().copied() {
        if owner == 0 {
            assert_eq!(framebuffer, 0, "detach clears FB_ID");
            hardware_owners.remove(&plane_key);
        } else {
            assert_ne!(framebuffer, 0, "attach has an FB_ID");
            assert_eq!(owner, u64::from(u32::from(crtc)));
            hardware_owners.insert(plane_key, output_crtc_key);
        }
    }

    let output_routes: Vec<_> = fixture
        .backend
        .platform
        .outputs
        .iter()
        .enumerate()
        .map(|(output_idx, output)| (output_idx, CrtcKey::for_output(output)))
        .collect();
    if output_routes.len() == 2 {
        assert_eq!(output_routes[0].1.crtc, crtc_a);
        assert_eq!(output_routes[1].1.crtc, crtc_b);
    }
    if verify_all_output_owners {
        for (output_idx, expected_crtc) in output_routes {
            let hardware_count = hardware_owners
                .values()
                .filter(|owner| **owner == expected_crtc)
                .count();
            assert!(
                hardware_count <= 1,
                "one output cannot own two cursor planes"
            );
            let expected_plane = hardware_owners
                .iter()
                .find_map(|(plane_key, owner)| (*owner == expected_crtc).then_some(*plane_key));
            let actual_plane = output_binding(fixture, output_idx).map(|binding| CursorPlaneKey {
                device_key: expected_crtc.device_key,
                plane: binding.properties.plane,
            });
            assert_eq!(
                actual_plane, expected_plane,
                "applied owner matches this accepted frame's recorded hardware state for {expected_crtc:?}; owners={hardware_owners:?}, properties={properties_p:?}/{properties_q:?}, cursor_changes={cursor_changes:?}: {record:?}",
            );
        }
    }

    // Capture ownership synchronously after acceptance, before this frame's
    // event is drained.
    assert!(fixture.backend.scene.has_pending_page_flip(output_idx));
    let hardware_bound = hardware_owners
        .values()
        .any(|owner| *owner == output_crtc_key);
    let hardware_mode = fixture
        .backend
        .scene
        .output_uses_hardware_cursor(output_idx);
    assert!(
        !hardware_bound || hardware_mode,
        "an applied hardware binding requires hardware scene mode: {:?}",
        record,
    );
    PendingSwapFrame {
        record: record.clone(),
        output_idx,
        event_frame: frame,
        hardware_bound,
        hardware_mode,
        cursor_root_position,
    }
}

fn assert_swap_frame_primary_content(
    fixture: &mut AtomicCursorFixture,
    pending: &PendingSwapFrame,
    selection: SwapFrameReadSelection,
) {
    let (output_crtc, output_x, output_y, output_width, output_height) = {
        let layout = &fixture.backend.platform.outputs[pending.output_idx];
        (
            CrtcKey::for_output(layout),
            layout.x,
            layout.y,
            layout.width,
            layout.height,
        )
    };
    let output_right = output_x.saturating_add(i32::from(output_width));
    let output_bottom = output_y.saturating_add(i32::from(output_height));
    let (cursor_root_x, cursor_root_y) = pending.cursor_root_position;
    let cursor_right = cursor_root_x.saturating_add(4);
    let cursor_bottom = cursor_root_y.saturating_add(4);
    let cursor_overlaps_output = cursor_root_x < output_right
        && output_x < cursor_right
        && cursor_root_y < output_bottom
        && output_y < cursor_bottom;
    let cursor_x = if cursor_overlaps_output {
        cursor_root_x
            .max(output_x)
            .min(output_right.saturating_sub(1))
    } else {
        output_x.saturating_add(8)
    };
    let cursor_y = if cursor_overlaps_output {
        cursor_root_y
            .max(output_y)
            .min(output_bottom.saturating_sub(1))
    } else {
        output_y.saturating_add(8)
    };
    let clear_x = if !cursor_overlaps_output || cursor_root_x < output_x {
        cursor_x.saturating_add(4)
    } else if cursor_root_x.saturating_add(8) < output_right {
        cursor_x.saturating_add(8)
    } else {
        cursor_x.saturating_sub(8)
    };
    let clear_y = cursor_y;
    let cursor_pixel = read_pixel_with_selection(fixture, cursor_x, cursor_y, selection);
    let clear_pixel = read_pixel_with_selection(fixture, clear_x, clear_y, selection);
    // Mutation "frame in HW mode with no binding and no SW compose" must fail:
    // software visibility follows the recorded applied owner, not scene mode.
    assert_eq!(
        cursor_pixel[..3] != clear_pixel[..3],
        !pending.hardware_bound && cursor_overlaps_output,
        "accepted primary pixels at {:?} carry software cursor exactly when {output_crtc:?} has no recorded hardware binding; mode_hw={}, plane_bound={}, cursor={:?}, output=({}, {}) {}x{}, pixels={:?}/{:?}, frame={:?}",
        (cursor_x, cursor_y),
        pending.hardware_mode,
        pending.hardware_bound,
        pending.cursor_root_position,
        output_x,
        output_y,
        output_width,
        output_height,
        cursor_pixel,
        clear_pixel,
        pending.record,
    );
}

fn assert_swap_frame_pending_pixels(fixture: &mut AtomicCursorFixture, pending: &PendingSwapFrame) {
    assert_swap_frame_primary_content(
        fixture,
        pending,
        SwapFrameReadSelection::LatestComposedFrame,
    );
}

fn assert_swap_frame_primary_pixels(fixture: &mut AtomicCursorFixture, pending: &PendingSwapFrame) {
    fixture.retire_output(pending.output_idx, pending.event_frame);
    assert!(
        fixture
            .backend
            .platform
            .retained_composed_framebuffer(pending.output_idx)
            .is_some(),
        "the accepted primary is the output's on-screen scanout BO: {:?}",
        pending.record,
    );
    assert_swap_frame_primary_content(fixture, pending, SwapFrameReadSelection::PermissiveDump);
}

pub(super) fn assert_accepted_ineligible_software_frame(
    fixture: &mut AtomicCursorFixture,
    record: &AtomicCommitRecord,
    cursor_plane: plane::Handle,
    frame: u32,
    cursor_root_position: (i32, i32),
) -> [u8; 4] {
    assert_eq!(record.result, ScriptedResult::Ok);
    let output_idx = accepted_primary_output(fixture, record)
        .expect("accepted software frame updates its primary framebuffer");
    assert_eq!(output_idx, 0);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(output_idx)
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_visible_for_output(output_idx)
    );
    assert!(output_binding(fixture, output_idx).is_none());
    assert!(
        !fixture
            .backend
            .scene
            .output_uses_hardware_cursor(output_idx)
    );
    assert!(
        record
            .properties
            .iter()
            .all(|update| update.object != cursor_plane.into())
    );

    let pending = PendingSwapFrame {
        record: record.clone(),
        output_idx,
        event_frame: frame,
        hardware_bound: false,
        hardware_mode: false,
        cursor_root_position,
    };
    assert_swap_frame_pending_pixels(fixture, &pending);
    assert_swap_frame_primary_pixels(fixture, &pending);
    read_pixel_with_selection(
        fixture,
        cursor_root_position.0,
        cursor_root_position.1,
        SwapFrameReadSelection::PermissiveDump,
    )
}

pub(super) fn assert_accepted_ineligible_retired_software_frame(
    fixture: &mut AtomicCursorFixture,
    record: &AtomicCommitRecord,
    cursor_plane: plane::Handle,
    cursor_root_position: (i32, i32),
) {
    assert_eq!(record.result, ScriptedResult::Ok);
    let output_idx = accepted_primary_output(fixture, record)
        .expect("accepted software frame updates its primary framebuffer");
    assert_eq!(output_idx, 0);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(output_idx)
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_visible_for_output(output_idx)
    );
    assert!(output_binding(fixture, output_idx).is_none());
    assert!(
        !fixture
            .backend
            .scene
            .output_uses_hardware_cursor(output_idx)
    );
    assert!(
        record
            .properties
            .iter()
            .all(|update| update.object != cursor_plane.into())
    );
    assert!(
        fixture
            .backend
            .platform
            .retained_composed_framebuffer(output_idx)
            .is_some(),
        "accepted retired primary remains the output's on-screen scanout BO: {record:?}"
    );

    let pending = PendingSwapFrame {
        record: record.clone(),
        output_idx,
        event_frame: 0,
        hardware_bound: false,
        hardware_mode: false,
        cursor_root_position,
    };
    assert_swap_frame_primary_content(fixture, &pending, SwapFrameReadSelection::PermissiveDump);
}

#[allow(clippy::too_many_arguments)]
fn submit_and_verify_swap_frames_until_owner(
    fixture: &mut AtomicCursorFixture,
    expected_output_idx: usize,
    blocked_output_idx: Option<usize>,
    hardware_owners: &mut std::collections::HashMap<CursorPlaneKey, CrtcKey>,
    plane_p: plane::Handle,
    properties_p: crate::kms::cursor_plane::topology::CursorPlaneProperties,
    plane_q: plane::Handle,
    properties_q: crate::kms::cursor_plane::topology::CursorPlaneProperties,
    crtc_a: ::drm::control::crtc::Handle,
    crtc_b: ::drm::control::crtc::Handle,
    next_event_frame: &mut u32,
    cursor_root_position: (i32, i32),
    target_plane: plane::Handle,
    target_owner: u64,
) -> PendingSwapFrame {
    let target_properties = if target_plane == plane_p {
        properties_p
    } else {
        assert_eq!(target_plane, plane_q);
        properties_q
    };
    loop {
        let record = submit_next_swap_scene_frame(fixture, expected_output_idx, blocked_output_idx);
        let pending = assert_swap_frame_applied_ownership(
            fixture,
            &record,
            hardware_owners,
            plane_p,
            properties_p,
            plane_q,
            properties_q,
            crtc_a,
            crtc_b,
            *next_event_frame,
            cursor_root_position,
        );
        *next_event_frame += 1;
        if property_value(&record, target_plane, target_properties.crtc_id) == Some(target_owner) {
            return pending;
        }
        assert_swap_frame_primary_pixels(fixture, &pending);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and confirmed failing): eagerly detach on matching; publish Software after skipped or
// rejected A frame; "record ownership before acceptance" (the primary-attributed rejection must preserve the old
// binding); attach B when A accepts before its event; switch CRTC_ID directly from A to B; attach B while keeping
// its old software pixels in the primary.
pub(super) fn atomic_cursor_only_active_reassignment_waits_for_software_frame() {
    let Some(mut fixture) = AtomicCursorFixture::new_with_two_outputs()
        .expect("build active two-output reassignment fixture")
    else {
        return;
    };
    let reporter = Rc::new(RecordingCursorRecoveryReporter::default());
    let reporter_trait: Rc<
        dyn crate::kms::render::platform::cursor_recovery::CursorRecoveryReporter,
    > = reporter.clone();
    fixture.backend.platform.services.cursor_recovery_reporter = reporter_trait;
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || base + Duration::from_millis(clock.get())));

    let crtc_a = fixture.backend.platform.outputs[0].output.crtc;
    let crtc_b = fixture.backend.platform.outputs[1].output.crtc;
    let key_a =
        crate::kms::render::platform::CrtcKey::for_output(&fixture.backend.platform.outputs[0]);
    let key_b =
        crate::kms::render::platform::CrtcKey::for_output(&fixture.backend.platform.outputs[1]);
    let plane = ::drm::control::from_u32(0x51).expect("synthetic cursor plane handle");

    // Establish an A-only assignment through the real debounced Backend hotplug route.
    queue_inventory_rescan(&mut fixture, &elapsed_ms, plane, crtc_a);
    assert!(
        fixture
            .backend
            .platform
            .active_cursor_crtcs
            .contains(&key_a)
    );
    assert!(
        fixture
            .backend
            .platform
            .active_cursor_crtcs
            .contains(&key_b)
    );
    let properties_a = cursor_properties(&fixture, 0);
    assert_eq!(properties_a.plane, plane);
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );

    // A 4×4 cursor crosses the 800-pixel seam: A uses HW, while B's primary
    // proves that its software cursor is really composed at the seam.
    let root = fixture.backend.core.window_id;
    let cursor = super::round_b_producers::create_argb_cursor_extent(
        &mut fixture.backend,
        [0x20, 0x50, 0xf0, 0xff],
        4,
        4,
        0,
        0,
    );
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("select opaque seam cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 798, 90);
    damage_cursor_root(&mut fixture);
    let before_initial_frames = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("present initial A-HW/B-SW frames");
    let initial = live_records(&fixture)[before_initial_frames..].to_vec();
    let a_initial = output_records(&initial, &fixture, 0);
    let b_initial = output_records(&initial, &fixture, 1);
    assert!(
        a_initial.iter().any(|record| {
            record.kind == AtomicCommitKind::SceneWithCursor && record.result == ScriptedResult::Ok
        }),
        "A accepted its frame-owned hardware cursor: {a_initial:?}",
    );
    assert!(
        b_initial
            .iter()
            .any(|record| record.kind == AtomicCommitKind::Scene
                && record.result == ScriptedResult::Ok),
        "B accepted its software-cursor scene frame: {b_initial:?}",
    );
    let a_initial_frame = a_initial
        .iter()
        .find(|record| record.kind == AtomicCommitKind::SceneWithCursor)
        .expect("A's accepted HW frame");
    assert_full_cursor_attach(
        a_initial_frame,
        cursor_properties(&fixture, 0),
        crtc_a,
        798,
        90,
    );
    let old_primary_a = property_value(
        a_initial_frame,
        fixture.backend.platform.outputs[0].output.plane,
        fixture.backend.platform.outputs[0].output.plane_fb_id_prop,
    )
    .expect("A's initial primary framebuffer");
    retire_if_pending(&mut fixture, 0, 1);
    retire_if_pending(&mut fixture, 1, 2);
    let old_binding = output_binding(&fixture, 0).expect("A's accepted hardware owner");
    assert_eq!(old_binding.properties.plane, plane);
    assert_eq!(cursor_lease_users(&fixture, old_binding.lease), 1);
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(1)
    );
    let software_pixel = read_pixel(&mut fixture, 800, 90);
    let beside_pixel = read_pixel(&mut fixture, 804, 90);
    assert_ne!(
        software_pixel[..3],
        beside_pixel[..3],
        "B composes cursor pixels"
    );

    // Move P's discovered assignment to B. This inventory-only epoch must not
    // disable either primary or discard A's accepted cursor binding.
    queue_inventory_rescan(&mut fixture, &elapsed_ms, plane, crtc_b);
    assert!(
        fixture
            .backend
            .platform
            .active_cursor_crtcs
            .contains(&key_a)
    );
    assert!(
        fixture
            .backend
            .platform
            .active_cursor_crtcs
            .contains(&key_b)
    );
    assert_eq!(output_binding(&fixture, 0), Some(old_binding));
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );

    // A has no render BO for one tick. B still gets its ordinary software
    // frame; matching alone cannot detach A or attach B.
    fixture.backend.platform.scanout_pools[0]
        .as_mut()
        .expect("A owns its primary pool")
        .display_pool_mut()
        .for_tests_script_unavailable_acquisitions([true]);
    damage_cursor_root(&mut fixture);
    let before_skip = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("schedule B while A has no BO");
    let after_skip = live_records(&fixture);
    assert_eq!(
        output_records(&after_skip[before_skip..], &fixture, 0).len(),
        0,
        "A's skipped transition submits no detach",
    );
    assert!(
        accepted_primary_framebuffers(&after_skip, &fixture.backend.platform.outputs[0])
            .iter()
            .all(|framebuffer| *framebuffer == old_primary_a),
        "A keeps scanning out its previously accepted primary",
    );
    assert_eq!(output_binding(&fixture, 0), Some(old_binding));
    assert_eq!(cursor_lease_users(&fixture, old_binding.lease), 1);
    let a_under_hw = read_pixel(&mut fixture, 798, 90);
    let a_beside_hw = read_pixel(&mut fixture, 790, 90);
    assert_eq!(
        a_under_hw[..3],
        a_beside_hw[..3],
        "A's old primary remains cursorless under its HW plane",
    );
    assert!(output_binding(&fixture, 1).is_none());
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(1),
        "B retains the software-cursor primary while A has no BO",
    );
    let b_still_software = read_pixel(&mut fixture, 800, 90);
    let b_still_clear = read_pixel(&mut fixture, 804, 90);
    assert_ne!(b_still_software[..3], b_still_clear[..3]);
    retire_if_pending(&mut fixture, 1, 3);

    // Reject A's next HW→SW transition at its primary plane. Both the full
    // request and cursor-stripped TEST_ONLY fail, so the cursor is not blamed.
    let primary_a = fixture.backend.platform.outputs[0].output.plane_fb_id_prop;
    fixture.recorders.atomic.reject_properties(&[primary_a]);
    damage_cursor_root(&mut fixture);
    let before_rejected = fixture.recorders.atomic.records().len();
    let _ = Backend::maybe_composite(&mut fixture.backend);
    let attempts = fixture.recorders.atomic.records()[before_rejected..].to_vec();
    let a_attempts: Vec<_> = attempts
        .iter()
        .filter(|record| {
            record.properties.iter().any(|update| {
                update.object == fixture.backend.platform.outputs[0].output.plane.into()
            })
        })
        .collect();
    assert_eq!(
        a_attempts.len(),
        3,
        "live, full probe, stripped probe: {a_attempts:?}"
    );
    assert_eq!(a_attempts[0].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(
        a_attempts[0].result,
        ScriptedResult::Error {
            kind: std::io::ErrorKind::InvalidInput,
            raw_os_error: Some(libc::EINVAL),
        }
    );
    assert!(a_attempts[1].flags.contains(AtomicCommitFlags::TEST_ONLY));
    assert_eq!(a_attempts[1].result, a_attempts[0].result);
    assert!(a_attempts[2].flags.contains(AtomicCommitFlags::TEST_ONLY));
    assert_eq!(a_attempts[2].kind, AtomicCommitKind::Scene);
    assert_eq!(a_attempts[2].result, a_attempts[0].result);
    assert!(
        a_attempts[2]
            .properties
            .iter()
            .all(|update| update.object != properties_a.plane.into()),
        "the attribution retry strips every cursor field",
    );
    assert_eq!(output_binding(&fixture, 0), Some(old_binding));
    assert_eq!(cursor_lease_users(&fixture, old_binding.lease), 1);
    assert!(
        accepted_primary_framebuffers(
            &live_records(&fixture),
            &fixture.backend.platform.outputs[0]
        )
        .iter()
        .all(|framebuffer| *framebuffer == old_primary_a),
        "the primary-attributed rejection does not replace A's old primary",
    );
    let a_under_rejected = read_pixel(&mut fixture, 798, 90);
    let a_beside_rejected = read_pixel(&mut fixture, 790, 90);
    assert_eq!(
        a_under_rejected[..3],
        a_beside_rejected[..3],
        "the rejected frame leaves A's cursorless primary on screen",
    );
    assert_eq!(
        reporter.calls(),
        0,
        "primary failure does not qualify a cursor key"
    );
    fixture.recorders.atomic.clear_property_rejections();
    retire_if_pending(&mut fixture, 1, 4);

    // A's next submitted software frame owns the detach. It keeps the accepted
    // binding and framebuffer user until that frame's event is drained.
    damage_cursor_root(&mut fixture);
    let before_detach = live_records(&fixture).len();
    drive_scene_retry_deadline_through_backend(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit A's software handoff");
    let after_detach = live_records(&fixture);
    let a_detaches: Vec<_> = output_records(&after_detach[before_detach..], &fixture, 0)
        .into_iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .collect();
    let detach = a_detaches
        .iter()
        .find(|record| record.kind == AtomicCommitKind::SceneWithCursor)
        .unwrap_or_else(|| panic!("A's accepted software frame carries the old plane detach: all={after_detach:?}, since={:?}", &after_detach[before_detach..]));
    assert_eq!(
        property_value(detach, plane, properties_a.fb_id),
        Some(0),
        "the accepted A frame explicitly detaches P",
    );
    assert_eq!(property_value(detach, plane, properties_a.crtc_id), Some(0));
    assert_eq!(
        detach
            .events
            .iter()
            .filter(|event| event.crtc == crtc_a)
            .count(),
        1,
        "A's detach frame owns its event",
    );
    assert_eq!(
        output_binding(&fixture, 0),
        None,
        "accepted detach changes A's state"
    );
    let a_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted A software-frame receipt");
    assert_eq!(a_receipt.mode, CursorAcceptedMode::Software);
    assert!(matches!(
        a_receipt.primary_cursor,
        CursorPrimaryContent::Software { .. }
    ));
    let a_state = fixture.backend.platform.devices[0]
        .cursor
        .crtc_states
        .get(&crtc_a)
        .expect("A cursor state");
    assert!(
        a_state.in_flight.is_some(),
        "accepted detach still owes its event"
    );
    assert_eq!(cursor_lease_users(&fixture, old_binding.lease), 1);
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(1)
    );
    let b_software_pixel = read_pixel(&mut fixture, 800, 90);
    let b_beside = read_pixel(&mut fixture, 804, 90);
    assert_ne!(
        b_software_pixel[..3],
        b_beside[..3],
        "B remains software while P is held by A"
    );

    // B receives its own software frame while A's detach event is outstanding;
    // no cursor-only or direct-transfer request can steal P.
    assert!(output_binding(&fixture, 1).is_none());
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );
    assert!(
        output_records(&after_detach[before_detach..], &fixture, 1)
            .iter()
            .filter(|record| record.result == ScriptedResult::Ok)
            .all(|record| !record_sets_cursor_property(record, properties_a)),
        "no accepted peer frame attaches P before A's event",
    );
    retire_if_pending(&mut fixture, 1, 5);
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert_eq!(cursor_lease_users(&fixture, old_binding.lease), 1);

    // A's event releases the old lease and opens B's attach barrier. B then
    // submits a separate SW→HW scene frame that erases the software pixels.
    let before_a_event = live_records(&fixture).len();
    fixture.retire_output(0, 6);
    let after_a_event = live_records(&fixture);
    assert!(output_binding(&fixture, 1).is_none());
    assert!(
        after_a_event[before_a_event..]
            .iter()
            .all(|record| !record_sets_cursor_property(record, properties_a)),
        "A's event only opens the barrier; B still needs its own scene frame",
    );
    assert_eq!(cursor_lease_users(&fixture, old_binding.lease), 0);
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(1)
    );
    let a_software_pixel = read_pixel(&mut fixture, 798, 90);
    let beside_a = read_pixel(&mut fixture, 804, 90);
    assert_ne!(
        a_software_pixel[..3],
        beside_a[..3],
        "A presents its software cursor frame"
    );
    let b_software_pixel = read_pixel(&mut fixture, 800, 90);
    assert_ne!(
        b_software_pixel[..3],
        beside_a[..3],
        "B remains software while P is held by A"
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );
    let before_attach = live_records(&fixture).len();
    drive_scene_retry_deadline_through_backend(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit B's separate hardware frame");
    drive_backend_until_scene_cursor_frames(&mut fixture, before_attach, &[1]);
    let after_attach = live_records(&fixture);
    let b_attach = output_records(&after_attach[before_attach..], &fixture, 1)
        .into_iter()
        .find(|record| record.kind == AtomicCommitKind::SceneWithCursor)
        .unwrap_or_else(|| {
            panic!(
                "B's own scene frame attaches P: {:?}",
                &after_attach[before_attach..]
            )
        });
    let properties_b = cursor_properties(&fixture, 1);
    assert_eq!(properties_b.plane, plane);
    assert_full_cursor_attach(&b_attach, properties_b, crtc_b, -2, 90);
    let b_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(1)
        .expect("accepted B hardware-frame receipt");
    let b_mode_after_acceptance = fixture.backend.scene.output_uses_hardware_cursor(1);
    let aggregate_mode_after_acceptance = fixture.backend.scene.cursor_mode();
    retire_if_pending(&mut fixture, 1, 8);
    assert_eq!(
        output_binding(&fixture, 1).map(|binding| binding.lease.sprite_key()),
        Some(Some((4, 4, 0, 0))),
        "sprite identity comes from the selected 4×4 cursor, independent of applied binding",
    );
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    assert!(
        !fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(1)
    );
    let b_hardware_pixel = read_pixel(&mut fixture, 800, 90);
    let b_clear_pixel = read_pixel(&mut fixture, 804, 90);
    assert_eq!(
        b_hardware_pixel[..3],
        b_clear_pixel[..3],
        "B's accepted primary erased SW pixels"
    );
    assert_eq!(b_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(b_receipt.primary_cursor, CursorPrimaryContent::Cursorless);
    assert!(
        b_mode_after_acceptance,
        "B mode changes when its attach is accepted"
    );
    assert_eq!(
        aggregate_mode_after_acceptance,
        crate::kms::render::scene::CursorPlaneMode::Mixed,
    );
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        aggregate_mode_after_acceptance
    );

    let accepted_cursor_routes: Vec<_> = live_records(&fixture)
        .into_iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .filter_map(|record| {
            property_value(&record, plane, properties_a.crtc_id).map(|value| (record.kind, value))
        })
        .collect();
    assert_eq!(
        accepted_cursor_routes,
        vec![
            (
                AtomicCommitKind::SceneWithCursor,
                u64::from(u32::from(crtc_a))
            ),
            (AtomicCommitKind::SceneWithCursor, 0),
            (
                AtomicCommitKind::SceneWithCursor,
                u64::from(u32::from(crtc_b))
            ),
        ],
        "accepted state is attach A, detach A, then attach B in separate scene frames",
    );

    retire_if_pending(&mut fixture, 0, 7);
    Backend::shutdown_outputs(&mut fixture.backend).expect("disable both outputs through Backend");
    fixture.assert_cursor_quiescent();
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert!(!fixture.backend.platform.cursor_reassignment_is_waiting());
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(
        ledger.end_state(),
        (0, 0, 0),
        "no cursor framebuffer remains allocated"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and confirmed failing): detach selected from desired matching; skip SW compose on the frame
// that detaches; record ownership before acceptance. Every accepted primary frame, including a cursorless delta,
// checks applied ownership at acceptance and primary pixels after its own event retires.
fn atomic_cursor_only_matching_swap_detaches_each_accepted_plane() {
    let Some(mut fixture) =
        AtomicCursorFixture::new_with_two_outputs().expect("build two-plane matching-swap fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || base + Duration::from_millis(clock.get())));

    let crtc_a = fixture.backend.platform.outputs[0].output.crtc;
    let crtc_b = fixture.backend.platform.outputs[1].output.crtc;
    let plane_p = cursor_properties(&fixture, 0).plane;
    let plane_q = cursor_properties(&fixture, 1).plane;

    // Establish two independently assigned planes through the debounced host-hotplug route.
    queue_cursor_topology_rescan(
        &mut fixture,
        &elapsed_ms,
        &[(plane_p, crtc_a), (plane_q, crtc_b)],
    );
    let properties_a = cursor_properties(&fixture, 0);
    let properties_b = cursor_properties(&fixture, 1);
    assert_eq!(properties_a.plane, plane_p);
    assert_eq!(properties_b.plane, plane_q);
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );

    // The cursor straddles both active routes, so each output must always have
    // either its hardware plane or the software pixels in its accepted primary.
    let root = fixture.backend.core.window_id;
    let cursor = super::round_b_producers::create_argb_cursor_extent(
        &mut fixture.backend,
        [0x20, 0x50, 0xf0, 0xff],
        4,
        4,
        0,
        0,
    );
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("select cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 798, 90);
    damage_cursor_root(&mut fixture);

    let records_before_initial = live_records(&fixture).len();
    let mut hardware_owners = std::collections::HashMap::new();
    let mut next_event_frame = 1;
    let a_initial_frame = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        0,
        Some(1),
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_p,
        u64::from(u32::from(crtc_a)),
    );
    assert_full_cursor_attach(&a_initial_frame.record, properties_a, crtc_a, 798, 90);
    let b_initial_frame = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        1,
        Some(0),
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_q,
        u64::from(u32::from(crtc_b)),
    );
    assert_full_cursor_attach(&b_initial_frame.record, properties_b, crtc_b, -2, 90);
    assert_swap_frame_primary_pixels(&mut fixture, &a_initial_frame);
    assert_swap_frame_primary_pixels(&mut fixture, &b_initial_frame);
    let old_binding_a = output_binding(&fixture, 0).expect("A's accepted P binding");
    let old_binding_b = output_binding(&fixture, 1).expect("B's accepted Q binding");
    assert_eq!(old_binding_a.properties.plane, plane_p);
    assert_eq!(old_binding_b.properties.plane, plane_q);

    // Swap the maximum matching while both old bindings remain accepted.
    queue_cursor_topology_rescan(
        &mut fixture,
        &elapsed_ms,
        &[(plane_p, crtc_b), (plane_q, crtc_a)],
    );
    assert_eq!(cursor_properties(&fixture, 0).plane, plane_q);
    assert_eq!(cursor_properties(&fixture, 1).plane, plane_p);
    assert_eq!(
        output_binding(&fixture, 0).unwrap().properties.plane,
        plane_p
    );
    assert_eq!(
        output_binding(&fixture, 1).unwrap().properties.plane,
        plane_q
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );

    damage_cursor_root(&mut fixture);
    let detach_a = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        0,
        Some(1),
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_p,
        0,
    );
    let detach_b = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        1,
        Some(0),
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_q,
        0,
    );
    for (record, properties, crtc) in [
        (&detach_a.record, properties_a, crtc_a),
        (&detach_b.record, properties_b, crtc_b),
    ] {
        let other_object: ::drm::control::RawResourceHandle = if properties.plane == plane_p {
            plane_q.into()
        } else {
            plane_p.into()
        };
        assert_eq!(record.result, ScriptedResult::Ok);
        assert_eq!(record.kind, AtomicCommitKind::SceneWithCursor);
        assert!(record.events.iter().any(|event| event.crtc == crtc));
        assert_eq!(
            property_value(record, properties.plane, properties.fb_id),
            Some(0)
        );
        assert_eq!(
            property_value(record, properties.plane, properties.crtc_id),
            Some(0)
        );
        assert!(
            record
                .properties
                .iter()
                .all(|update| update.object != other_object),
            "one scene frame must detach only its own accepted plane: {record:?}",
        );
    }
    assert_eq!(output_binding(&fixture, 0), None);
    assert_eq!(output_binding(&fixture, 1), None);
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.scene.has_pending_page_flip(1));
    assert!(cursor_lease_users(&fixture, old_binding_a.lease) > 0);
    assert!(cursor_lease_users(&fixture, old_binding_b.lease) > 0);
    assert_swap_frame_primary_pixels(&mut fixture, &detach_a);
    assert_swap_frame_primary_pixels(&mut fixture, &detach_b);
    assert_eq!(cursor_lease_users(&fixture, old_binding_a.lease), 0);
    assert_eq!(cursor_lease_users(&fixture, old_binding_b.lease), 0);
    assert!(!fixture.backend.scene.output_uses_hardware_cursor(0));
    assert!(!fixture.backend.scene.output_uses_hardware_cursor(1));

    let accepted_routes: Vec<_> = live_records(&fixture)[records_before_initial..]
        .iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .filter_map(|record| {
            [(plane_p, properties_a), (plane_q, properties_b)]
                .into_iter()
                .find_map(|(plane, properties)| {
                    property_value(record, plane, properties.crtc_id)
                        .map(|owner| (u32::from(plane), owner))
                })
        })
        .collect();
    assert_eq!(
        accepted_routes,
        vec![
            (u32::from(plane_p), u64::from(u32::from(crtc_a))),
            (u32::from(plane_q), u64::from(u32::from(crtc_b))),
            (u32::from(plane_p), 0),
            (u32::from(plane_q), 0),
        ],
        "each accepted intermediate frame changes only its own object/property binding",
    );

    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(1)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );

    let records_before_replacement = live_records(&fixture).len();
    let attach_a = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        0,
        Some(1),
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_q,
        u64::from(u32::from(crtc_a)),
    );
    let swapped_a = cursor_properties(&fixture, 0);
    let swapped_b = cursor_properties(&fixture, 1);
    assert_full_cursor_attach(&attach_a.record, swapped_a, crtc_a, 798, 90);
    let attach_b = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        1,
        Some(0),
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_p,
        u64::from(u32::from(crtc_b)),
    );
    assert_full_cursor_attach(&attach_b.record, swapped_b, crtc_b, -2, 90);
    assert_swap_frame_primary_pixels(&mut fixture, &attach_a);
    assert_swap_frame_primary_pixels(&mut fixture, &attach_b);
    let after_replacement = live_records(&fixture);
    assert_eq!(
        output_records(
            &after_replacement[records_before_replacement..],
            &fixture,
            0
        )
        .iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .count(),
        1,
        "A's accepted replacement is one separately owned frame",
    );
    assert_eq!(
        output_records(
            &after_replacement[records_before_replacement..],
            &fixture,
            1
        )
        .iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .count(),
        1,
        "B's accepted replacement is one separately owned frame",
    );
    assert_eq!(
        output_binding(&fixture, 0).unwrap().properties.plane,
        plane_q
    );
    assert_eq!(
        output_binding(&fixture, 1).unwrap().properties.plane,
        plane_p
    );
    let final_records = live_records(&fixture);
    let final_routes: Vec<_> = final_records[records_before_initial..]
        .iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .filter_map(|record| {
            [(plane_p, properties_a), (plane_q, properties_b)]
                .into_iter()
                .find_map(|(plane, properties)| {
                    property_value(record, plane, properties.crtc_id)
                        .map(|owner| (u32::from(plane), owner))
                })
        })
        .collect();
    assert_eq!(
        final_routes,
        vec![
            (u32::from(plane_p), u64::from(u32::from(crtc_a))),
            (u32::from(plane_q), u64::from(u32::from(crtc_b))),
            (u32::from(plane_p), 0),
            (u32::from(plane_q), 0),
            (u32::from(plane_q), u64::from(u32::from(crtc_a))),
            (u32::from(plane_p), u64::from(u32::from(crtc_b))),
        ],
        "the recorded transfer is detach both old owners, then attach both new owners",
    );

    // Primary pixels are cursorless exactly while a hardware plane is bound;
    // after the SW handoff, the scene footprint supplies those same pixels.
    for (output_idx, cursor_x, clear_x) in [(0, 798, 790), (1, 800, 804)] {
        assert!(
            fixture
                .backend
                .scene
                .output_uses_hardware_cursor(output_idx)
        );
        assert!(
            !fixture
                .backend
                .scene
                .for_tests_cursor_footprint_was_presented(output_idx)
        );
        let cursorless = read_pixel(&mut fixture, cursor_x, 90);
        let clear = read_pixel(&mut fixture, clear_x, 90);
        assert_eq!(cursorless[..3], clear[..3]);
    }

    Backend::shutdown_outputs(&mut fixture.backend).expect("disable both outputs through Backend");
    fixture.assert_cursor_quiescent();
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert!(!fixture.backend.platform.cursor_reassignment_is_waiting());
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation "fallback ignores peer owner": B's successful disable must never
// detach P while A's failed disable leaves P accepted on A.
fn atomic_cursor_only_lifecycle_fallback_never_detaches_peer_cursor() {
    let Some(mut fixture) = AtomicCursorFixture::new_with_two_outputs()
        .expect("build peer-owned lifecycle fallback fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let device = Rc::clone(
        &fixture
            .backend
            .platform
            .device_for_key(device_key)
            .expect("fixture device")
            .device,
    );
    fixture.recorders.page_flip_events.register_device(&device);
    let crtc_a = fixture.backend.platform.outputs[0].output.crtc;
    let crtc_b = fixture.backend.platform.outputs[1].output.crtc;
    let plane_p = ::drm::control::from_u32(0x51).expect("synthetic cursor plane handle");
    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || base + Duration::from_millis(clock.get())));
    queue_cursor_topology_rescan(&mut fixture, &elapsed_ms, &[(plane_p, crtc_a)]);
    let properties_a = cursor_properties(&fixture, 0);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );

    let root = fixture.backend.core.window_id;
    let cursor = super::round_b_producers::create_argb_cursor_extent(
        &mut fixture.backend,
        [0x20, 0x50, 0xf0, 0xff],
        4,
        4,
        0,
        0,
    );
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("select cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 798, 90);
    damage_cursor_root(&mut fixture);

    let mut hardware_owners = std::collections::HashMap::new();
    let mut next_event_frame = 1;
    let initial_a = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        0,
        Some(1),
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_p,
        properties_a,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_p,
        u64::from(u32::from(crtc_a)),
    );
    assert_swap_frame_primary_pixels(&mut fixture, &initial_a);
    let initial_b_record = submit_next_swap_scene_frame(&mut fixture, 1, Some(0));
    assert_eq!(
        initial_b_record.kind,
        AtomicCommitKind::Scene,
        "B uses a software cursor while only A has a matching plane",
    );
    let initial_b = assert_swap_frame_applied_ownership(
        &mut fixture,
        &initial_b_record,
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_p,
        properties_a,
        crtc_a,
        crtc_b,
        next_event_frame,
        (798, 90),
    );
    assert_swap_frame_primary_pixels(&mut fixture, &initial_b);
    let old_binding_a = output_binding(&fixture, 0).expect("A accepted P");
    assert_eq!(old_binding_a.properties.plane, plane_p);
    assert_eq!(output_binding(&fixture, 1), None);

    // B remains SW while it newly desires P; A's accepted P binding blocks
    // that destination until A's lifecycle disable succeeds.
    queue_cursor_topology_rescan(&mut fixture, &elapsed_ms, &[(plane_p, crtc_b)]);
    assert!(fixture.backend.platform.cursor_reassignment_is_waiting());
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );
    assert_eq!(
        output_binding(&fixture, 0).unwrap().properties.plane,
        plane_p
    );
    assert_eq!(output_binding(&fixture, 1), None);
    assert!(cursor_lease_users_for_output(&fixture, 0, old_binding_a.lease) > 0);

    let output_b = super::round_a::clone_drm_output(&fixture.backend.platform.outputs[1].output);
    let records_before_disable = live_records(&fixture).len();
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    assert!(Backend::shutdown_outputs(&mut fixture.backend).is_err());
    let disable_records = live_records(&fixture);
    let attempts = &disable_records[records_before_disable..];
    let failed_a = attempts
        .iter()
        .find(|record| record.result != ScriptedResult::Ok)
        .expect("A's lifecycle disable fails first");
    assert_eq!(failed_a.flags, AtomicCommitFlags::ALLOW_MODESET);
    assert_eq!(
        property_value(failed_a, plane_p, properties_a.fb_id),
        Some(0)
    );
    assert_eq!(
        property_value(failed_a, plane_p, properties_a.crtc_id),
        Some(0)
    );
    let successful_b = attempts
        .iter()
        .find(|record| {
            record.result == ScriptedResult::Ok
                && property_value(record, output_b.plane, output_b.plane_fb_id_prop) == Some(0)
        })
        .expect("the power loop continues and disables B");
    assert_eq!(successful_b.flags, AtomicCommitFlags::ALLOW_MODESET);
    assert_ne!(
        property_value(successful_b, plane_p, properties_a.fb_id),
        Some(0),
        "B must not detach P while A's disable is rejected",
    );
    assert_ne!(
        property_value(successful_b, plane_p, properties_a.crtc_id),
        Some(0),
        "B must not clear P's accepted CRTC owner",
    );
    assert_eq!(
        output_binding(&fixture, 0).unwrap().properties.plane,
        plane_p
    );
    assert!(cursor_lease_users_for_output(&fixture, 0, old_binding_a.lease) > 0);

    // Retry the actual Backend shutdown so all accepted ownership is drained
    // and the test fixture has no pending obligations or framebuffer users.
    Backend::shutdown_outputs(&mut fixture.backend).expect("retry shutdown after preserving A");
    fixture.assert_cursor_quiescent();
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert!(!fixture.backend.platform.cursor_reassignment_is_waiting());
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: allow the desired matching alone to release B's attach barrier;
// clear A's lifecycle owner before its old event drain; or attach P to an
// obsolete destination after the disable. C8's per-frame helper checks every
// accepted primary's content and applied cursor owner before its event.
fn atomic_cursor_only_disabled_reassignment_uses_lifecycle_detach() {
    let Some(mut fixture) =
        AtomicCursorFixture::new_with_two_outputs().expect("build disabled reassignment fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let device = Rc::clone(
        &fixture
            .backend
            .platform
            .device_for_key(device_key)
            .expect("fixture device")
            .device,
    );
    fixture.recorders.page_flip_events.register_device(&device);

    let crtc_a = fixture.backend.platform.outputs[0].output.crtc;
    let crtc_b = fixture.backend.platform.outputs[1].output.crtc;
    let plane_p = cursor_properties(&fixture, 0).plane;
    let plane_q = cursor_properties(&fixture, 1).plane;
    assert_ne!(plane_p, plane_q);
    let properties_a = cursor_properties(&fixture, 0);
    let properties_b = cursor_properties(&fixture, 1);

    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || base + Duration::from_millis(clock.get())));

    let root = fixture.backend.core.window_id;
    let cursor = super::round_b_producers::create_argb_cursor_extent(
        &mut fixture.backend,
        [0x20, 0x50, 0xf0, 0xff],
        4,
        4,
        0,
        0,
    );
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("select cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 798, 90);
    damage_cursor_root(&mut fixture);

    let mut hardware_owners = std::collections::HashMap::new();
    let mut next_event_frame = 1;
    let initial_a = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        0,
        Some(1),
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_p,
        u64::from(u32::from(crtc_a)),
    );
    assert_full_cursor_attach(&initial_a.record, properties_a, crtc_a, 798, 90);
    assert_swap_frame_pending_pixels(&mut fixture, &initial_a);
    let initial_b = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        1,
        Some(0),
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_q,
        u64::from(u32::from(crtc_b)),
    );
    assert_full_cursor_attach(&initial_b.record, properties_b, crtc_b, -2, 90);
    assert_swap_frame_pending_pixels(&mut fixture, &initial_b);
    let old_binding_a = output_binding(&fixture, 0).expect("A's accepted P binding");
    let old_binding_b = output_binding(&fixture, 1).expect("B's accepted Q binding");
    assert_eq!(old_binding_a.properties.plane, plane_p);
    assert_eq!(old_binding_b.properties.plane, plane_q);

    // Reassign both planes. A remains the accepted P owner while B is the
    // desired P destination; topology alone cannot release its attach barrier.
    queue_cursor_topology_rescan(
        &mut fixture,
        &elapsed_ms,
        &[(plane_p, crtc_b), (plane_q, crtc_a)],
    );
    assert_eq!(
        output_binding(&fixture, 0).unwrap().properties.plane,
        plane_p
    );
    assert_eq!(
        output_binding(&fixture, 1).unwrap().properties.plane,
        plane_q
    );
    assert!(fixture.backend.platform.cursor_reassignment_is_waiting());
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1),
        "desired topology cannot attach P while A still owns it",
    );
    assert_eq!(
        output_binding(&fixture, 0).unwrap().properties.plane,
        plane_p
    );
    assert_eq!(
        output_binding(&fixture, 1).unwrap().properties.plane,
        plane_q
    );

    // Preserve the old events in the controlled source. A's CRTC is the
    // transfer source; B's accepted frame may still own its own event. The
    // lifecycle drain consumes both after blocking disables.
    fixture
        .recorders
        .page_flip_events
        .queue_flip_for_device(&device, crtc_a, 90);
    fixture
        .recorders
        .page_flip_events
        .queue_flip_for_device(&device, crtc_b, 91);
    let connector_a = fixture.backend.platform.outputs[0]
        .output
        .connector_name
        .clone();
    let disabled_output =
        super::round_a::clone_drm_output(&fixture.backend.platform.outputs[0].output);
    let output_a = randr_output_for_connector(&mut fixture, &connector_a);

    // The first blocking disable fails. A keeps P as its accepted owner, and
    // no accepted request may attach P to the waiting destination B.
    let before_failed_disable = live_records(&fixture).len();
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    assert!(
        Backend::apply_crtc_config(
            &mut fixture.backend,
            output_a.output_id,
            &connector_a,
            None,
            0,
            0,
        )
        .is_err()
    );
    assert_eq!(
        output_binding(&fixture, 0).unwrap().properties.plane,
        plane_p
    );
    assert!(cursor_lease_users_for_output(&fixture, 0, old_binding_a.lease) > 0);
    let after_failed_disable = live_records(&fixture);
    let failed_a_disable = after_failed_disable[before_failed_disable..]
        .iter()
        .find(|record| record.result != ScriptedResult::Ok)
        .expect("A lifecycle disable is rejected");
    assert_eq!(failed_a_disable.kind, AtomicCommitKind::Scene);
    assert_eq!(failed_a_disable.flags, AtomicCommitFlags::ALLOW_MODESET);
    assert_eq!(
        property_value(failed_a_disable, plane_p, properties_a.fb_id),
        Some(0)
    );
    assert_eq!(
        property_value(failed_a_disable, plane_p, properties_a.crtc_id),
        Some(0)
    );
    assert!(
        after_failed_disable[before_failed_disable..]
            .iter()
            .filter(|record| record.result == ScriptedResult::Ok)
            .all(
                |record| property_value(record, plane_p, properties_a.crtc_id)
                    != Some(u64::from(u32::from(crtc_b)))
            )
    );

    // Retry through the same Backend CRTC entry. The lifecycle observer runs
    // after A's blocking detach and before the old A event is consumed.
    let drain_observations = Rc::new(std::cell::RefCell::new(Vec::new()));
    let observations = Rc::clone(&drain_observations);
    let p_lease = old_binding_a.lease;
    let p_plane = plane_p;
    let a_crtc = crtc_a;
    fixture
        .recorders
        .old_event_drain_observer
        .set(Rc::new(move |platform| {
            let state = platform
                .device_for_key(device_key)
                .and_then(|device| device.cursor.crtc_states.get(&a_crtc));
            let lifecycle_plane = state
                .and_then(|state| state.lifecycle_pending)
                .map(|binding| binding.properties.plane);
            let users = platform
                .device_for_key(device_key)
                .and_then(|device| device.cursor.plane.as_ref())
                .and_then(|plane| plane.sprite_user_count(p_lease));
            observations.borrow_mut().push((lifecycle_plane, users));
        }));
    let before_successful_disable = live_records(&fixture).len();
    assert!(
        Backend::apply_crtc_config(
            &mut fixture.backend,
            output_a.output_id,
            &connector_a,
            None,
            0,
            0,
        )
        .expect("disable A through Backend after retry")
    );
    let observations = drain_observations.borrow();
    let (lifecycle_plane, users) = observations
        .first()
        .copied()
        .expect("old-event drain observed lifecycle ownership");
    assert_eq!(lifecycle_plane, Some(p_plane));
    assert!(
        users.unwrap_or(0) > 0,
        "A's old lease lives through the drain"
    );
    drop(observations);
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    assert_eq!(fixture.backend.platform.outputs.len(), 1);
    assert_eq!(fixture.backend.platform.outputs[0].output.crtc, crtc_b);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(p_lease),
        Some(0)
    );
    assert!(!fixture.backend.platform.cursor_reassignment_is_waiting());

    // A's disable record contains its accepted P detach. The later B attach
    // is a separate full-state request after the old-event drain completed.
    let records = live_records(&fixture);
    let a_disable = records[before_successful_disable..]
        .iter()
        .find(|record| {
            record.result == ScriptedResult::Ok
                && record.kind == AtomicCommitKind::Scene
                && property_value(record, plane_p, properties_a.fb_id) == Some(0)
                && property_value(record, plane_p, properties_a.crtc_id) == Some(0)
        })
        .expect("A blocking disable detaches the applied P plane");
    assert_eq!(a_disable.flags, AtomicCommitFlags::ALLOW_MODESET);
    assert!(a_disable.events.is_empty());
    for property in [
        disabled_output.plane_fb_id_prop,
        disabled_output.plane_crtc_id_prop,
    ] {
        assert_eq!(
            property_value(a_disable, disabled_output.plane, property),
            Some(0)
        );
    }

    // The earlier failed disable leaves KMS inactive. Re-enable B through
    // Backend and inspect its eventual accepted full-state attachment.
    Backend::set_dpms_power(&mut fixture.backend, 0).expect("re-enable surviving B");
    let b_properties = cursor_properties(&fixture, 0);
    assert_eq!(b_properties.plane, plane_p);
    let mut new_owner_hardware = std::collections::HashMap::new();
    let b_attach = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        0,
        None,
        &mut new_owner_hardware,
        plane_p,
        b_properties,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_p,
        u64::from(u32::from(crtc_b)),
    );
    assert_full_cursor_attach(&b_attach.record, b_properties, crtc_b, -2, 90);
    assert_eq!(
        output_binding(&fixture, 0).unwrap().properties.plane,
        plane_p
    );
    assert_swap_frame_primary_pixels(&mut fixture, &b_attach);
    assert!(
        !fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    assert_eq!(
        read_pixel(&mut fixture, 800, 90)[..3],
        read_pixel(&mut fixture, 804, 90)[..3]
    );
    Backend::shutdown_outputs(&mut fixture.backend).expect("disable outputs through Backend");
    fixture.assert_cursor_quiescent();
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert!(!fixture.backend.platform.cursor_reassignment_is_waiting());
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: keep a canceled destination after matching retarget; attach to a
// destination removed while the transfer waits; or route a delayed event by
// raw CRTC/plane/FB handle without its DRM device key. Accepted scene frames
// use the shared C8 ownership and primary-content checks.
fn atomic_cursor_only_reassignment_cancellation_keeps_device_identity() {
    let Some(mut fixture) = AtomicCursorFixture::new_with_two_outputs()
        .expect("build reassignment cancellation fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let crtc_a = fixture.backend.platform.outputs[0].output.crtc;
    let crtc_b = fixture.backend.platform.outputs[1].output.crtc;
    let plane_p = cursor_properties(&fixture, 0).plane;
    let plane_q = cursor_properties(&fixture, 1).plane;
    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || base + Duration::from_millis(clock.get())));

    queue_cursor_topology_rescan(
        &mut fixture,
        &elapsed_ms,
        &[(plane_p, crtc_a), (plane_q, crtc_b)],
    );
    let properties_a = cursor_properties(&fixture, 0);
    let properties_b = cursor_properties(&fixture, 1);
    let root = fixture.backend.core.window_id;
    let cursor = super::round_b_producers::create_argb_cursor_extent(
        &mut fixture.backend,
        [0x20, 0x50, 0xf0, 0xff],
        4,
        4,
        0,
        0,
    );
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("select cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 798, 90);
    damage_cursor_root(&mut fixture);

    let mut hardware_owners = std::collections::HashMap::new();
    let mut next_event_frame = 20;
    let initial_a = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        0,
        Some(1),
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_p,
        u64::from(u32::from(crtc_a)),
    );
    assert_full_cursor_attach(&initial_a.record, properties_a, crtc_a, 798, 90);
    assert_swap_frame_pending_pixels(&mut fixture, &initial_a);
    let initial_b = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        1,
        Some(0),
        &mut hardware_owners,
        plane_p,
        properties_a,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_q,
        u64::from(u32::from(crtc_b)),
    );
    assert_full_cursor_attach(&initial_b.record, properties_b, crtc_b, -2, 90);
    assert_swap_frame_pending_pixels(&mut fixture, &initial_b);

    let live_a = output_binding(&fixture, 0).expect("A owns P before reassignment");
    let live_b = output_binding(&fixture, 1).expect("B owns Q before reassignment");
    queue_cursor_topology_rescan(
        &mut fixture,
        &elapsed_ms,
        &[(plane_p, crtc_b), (plane_q, crtc_a)],
    );
    assert!(fixture.backend.platform.cursor_reassignment_is_waiting());
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );

    // A newer matching returns each plane to its accepted owner before any
    // transfer frame is accepted. The pending B destination is canceled in
    // place; the ordinary next Backend scheduling pass has no cursor delta.
    queue_cursor_topology_rescan(
        &mut fixture,
        &elapsed_ms,
        &[(plane_p, crtc_a), (plane_q, crtc_b)],
    );
    assert!(!fixture.backend.platform.cursor_reassignment_is_waiting());
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );
    assert_eq!(output_binding(&fixture, 0), Some(live_a));
    assert_eq!(output_binding(&fixture, 1), Some(live_b));
    let before_idle_pass = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("run idle scheduling pass");
    assert_eq!(live_records(&fixture).len(), before_idle_pass);

    // Put the transfer back in flight and remove B through Backend hotplug.
    // Its disappearance cancels the desired destination without accepting an
    // intermediate attach or leaving the source lease parked.
    queue_cursor_topology_rescan(
        &mut fixture,
        &elapsed_ms,
        &[(plane_p, crtc_b), (plane_q, crtc_a)],
    );
    assert!(fixture.backend.platform.cursor_reassignment_is_waiting());
    let removed_key = fixture.backend.platform.outputs[1].key.clone();
    for output_idx in 0..fixture.backend.platform.outputs.len() {
        let output = &fixture.backend.platform.outputs[output_idx];
        let device = Rc::clone(
            &fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .expect("hotplug output device")
                .device,
        );
        fixture.recorders.page_flip_events.register_device(&device);
        fixture.recorders.page_flip_events.queue_flip_for_device(
            &device,
            output.output.crtc,
            60 + u32::try_from(output_idx).expect("output index fits"),
        );
    }
    queue_connector_removal_rescan(&mut fixture, &elapsed_ms, &[0]);
    assert_eq!(fixture.backend.platform.outputs.len(), 1);
    assert_ne!(fixture.backend.platform.outputs[0].key, removed_key);
    assert!(!fixture.backend.platform.cursor_reassignment_is_waiting());
    assert_eq!(output_binding(&fixture, 0), None);

    // A fresh matching on the surviving CRTC may attach P. It is accepted as
    // a new full-state request after the removed destination is gone.
    queue_cursor_topology_rescan(&mut fixture, &elapsed_ms, &[(plane_p, crtc_a)]);
    assert_eq!(cursor_properties(&fixture, 0).plane, plane_p);
    assert!(!fixture.backend.platform.cursor_reassignment_is_waiting());
    damage_cursor_root(&mut fixture);
    let survivor_properties = cursor_properties(&fixture, 0);
    let mut survivor_hardware = std::collections::HashMap::new();
    let survivor_attach = submit_and_verify_swap_frames_until_owner(
        &mut fixture,
        0,
        None,
        &mut survivor_hardware,
        plane_p,
        survivor_properties,
        plane_q,
        properties_b,
        crtc_a,
        crtc_b,
        &mut next_event_frame,
        (798, 90),
        plane_p,
        u64::from(u32::from(crtc_a)),
    );
    assert_full_cursor_attach(
        &survivor_attach.record,
        survivor_properties,
        crtc_a,
        798,
        90,
    );
    assert_eq!(
        output_binding(&fixture, 0).unwrap().properties.plane,
        plane_p
    );
    assert_swap_frame_primary_pixels(&mut fixture, &survivor_attach);
    assert!(
        !fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    assert_eq!(
        read_pixel(&mut fixture, 798, 90)[..3],
        read_pixel(&mut fixture, 790, 90)[..3]
    );
    let survivor_before_idle = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("ordinary scheduling pass after removed-destination cleanup");
    assert_eq!(
        live_records(&fixture).len(),
        survivor_before_idle,
        "a vanished transfer destination is not retried after cleanup",
    );
    assert!(!fixture.backend.platform.cursor_reassignment_is_waiting());
    Backend::shutdown_outputs(&mut fixture.backend).expect("shut down cancellation fixture");
    fixture.assert_cursor_quiescent();
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert!(!fixture.backend.platform.cursor_reassignment_is_waiting());
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));

    // Device identity is part of the CRTC, plane and framebuffer owner key.
    // This fixture exposes equal raw object/framebuffer handles on both cards.
    let Some(mut colliding) = AtomicCursorFixture::new_with_colliding_device_handles()
        .expect("build cross-device collision fixture")
    else {
        return;
    };
    colliding
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let key_a =
        crate::kms::render::platform::CrtcKey::for_output(&colliding.backend.platform.outputs[0]);
    let key_b =
        crate::kms::render::platform::CrtcKey::for_output(&colliding.backend.platform.outputs[1]);
    let properties_a = cursor_properties(&colliding, 0);
    let properties_b = cursor_properties(&colliding, 1);
    assert_eq!(key_a.crtc, key_b.crtc);
    assert_ne!(key_a.device_key, key_b.device_key);
    assert_eq!(properties_a.plane, properties_b.plane);

    let root = colliding.backend.core.window_id;
    let cursor = super::round_b_producers::create_argb_cursor_extent(
        &mut colliding.backend,
        [0x20, 0x50, 0xf0, 0xff],
        4,
        4,
        0,
        0,
    );
    Backend::define_cursor(&mut colliding.backend, None, root, cursor.as_raw())
        .expect("define cursor across both devices");
    Backend::warp_pointer_root(&mut colliding.backend, &mut colliding.state, 100, 90);
    damage_cursor_root(&mut colliding);
    let before_colliding_frames = live_records(&colliding).len();
    drive_backend_until_scene_cursor_frames(&mut colliding, before_colliding_frames, &[0, 1]);
    let colliding_records = live_records(&colliding);
    let mut accepted_by_device = Vec::new();
    for record in &colliding_records[before_colliding_frames..] {
        if record.result == ScriptedResult::Ok
            && let Some(output_idx) = accepted_primary_output(&colliding, record)
        {
            let output = &colliding.backend.platform.outputs[output_idx];
            let key = crate::kms::render::platform::CrtcKey::for_output(output);
            let properties = if output_idx == 0 {
                properties_a
            } else {
                properties_b
            };
            let binding = output_binding(&colliding, output_idx)
                .expect("each card retains its accepted cursor binding");
            assert_eq!(binding.properties.plane, properties.plane);
            assert_eq!(
                binding.framebuffer,
                property_value(record, properties.plane, properties.fb_id)
                    .expect("accepted cursor framebuffer property") as u32
            );
            assert_eq!(
                property_value(record, properties.plane, properties.crtc_id),
                Some(u64::from(u32::from(key.crtc))),
                "event and plane state route to the device-qualified CRTC",
            );
            accepted_by_device.push((output_idx, record.clone()));
        }
    }
    assert_eq!(
        accepted_by_device.len(),
        2,
        "each DRM device accepted its own frame"
    );
    let binding_a = output_binding(&colliding, 0).unwrap();
    let binding_b = output_binding(&colliding, 1).unwrap();
    assert_eq!(binding_a.properties.plane, binding_b.properties.plane);
    assert_eq!(binding_a.framebuffer, binding_b.framebuffer);
    assert!(
        colliding.backend.platform.devices[0].cursor.crtc_states[&key_a.crtc]
            .in_flight
            .is_some()
    );
    assert!(
        colliding
            .backend
            .platform
            .device_for_key(key_b.device_key)
            .unwrap()
            .cursor
            .crtc_states[&key_b.crtc]
            .in_flight
            .is_some()
    );

    // Retiring card A's event must leave card B's identical raw handles and
    // framebuffer lease owned until card B's own device event arrives.
    let frame_a = accepted_by_device
        .iter()
        .find(|(output_idx, _)| *output_idx == 0)
        .expect("card A accepted frame");
    let mut colliding_owners = std::collections::HashMap::new();
    let pending_a = assert_swap_frame_applied_ownership_inner(
        &mut colliding,
        &frame_a.1,
        &mut colliding_owners,
        properties_a.plane,
        properties_a,
        properties_b.plane,
        properties_b,
        key_a.crtc,
        key_b.crtc,
        70,
        (100, 90),
        false,
    );
    assert_eq!(pending_a.output_idx, 0);
    assert_swap_frame_primary_pixels(&mut colliding, &pending_a);
    assert_eq!(
        cursor_lease_users_for_output(&colliding, 1, binding_b.lease),
        1
    );
    assert!(
        colliding
            .backend
            .platform
            .device_for_key(key_b.device_key)
            .unwrap()
            .cursor
            .crtc_states[&key_b.crtc]
            .in_flight
            .is_some(),
        "A's raw CRTC event cannot settle B's same-valued event obligation",
    );
    let frame_b = accepted_by_device
        .iter()
        .find(|(output_idx, _)| *output_idx == 1)
        .expect("card B accepted frame");
    let pending_b = assert_swap_frame_applied_ownership(
        &mut colliding,
        &frame_b.1,
        &mut colliding_owners,
        properties_a.plane,
        properties_a,
        properties_b.plane,
        properties_b,
        key_a.crtc,
        key_b.crtc,
        71,
        (100, 90),
    );
    assert_eq!(pending_b.output_idx, 1);
    assert_swap_frame_primary_pixels(&mut colliding, &pending_b);
    assert_eq!(
        cursor_lease_users_for_output(&colliding, 1, binding_b.lease),
        1
    );
    // Remove card A through Backend hotplug, shifting card B into output index
    // zero. After B accepts a new cursor frame, an injected delayed A event
    // carries the same raw CRTC, plane and framebuffer handles. Device and
    // topology identity must keep that stale event from settling B's lease.
    let removed_key = colliding.backend.platform.outputs[0].key.clone();
    let surviving_key = colliding.backend.platform.outputs[1].key.clone();
    let device_a = Rc::clone(
        &colliding
            .backend
            .platform
            .device_for_key(key_a.device_key)
            .expect("old card A remains open for stale event delivery")
            .device,
    );
    colliding
        .recorders
        .page_flip_events
        .register_device(&device_a);
    let hotplug_base = std::time::Instant::now();
    let hotplug_elapsed = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&hotplug_elapsed);
    colliding
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || {
            hotplug_base + Duration::from_millis(clock.get())
        }));
    let cursor_discovery_calls_before = colliding
        .backend
        .platform
        .for_tests_cursor_discovery_calls(key_a.device_key)
        .expect("card A cursor discovery recorder");
    queue_connector_removal_rescan(&mut colliding, &hotplug_elapsed, &[1]);
    assert_eq!(
        colliding
            .backend
            .platform
            .for_tests_cursor_discovery_calls(key_a.device_key),
        Some(cursor_discovery_calls_before + 1),
        "backend rescan probes the topology on card A",
    );
    assert_eq!(
        colliding.backend.platform.outputs.len(),
        1,
        "removing card A leaves only card B: {:?}; power={:?}",
        colliding
            .backend
            .platform
            .outputs
            .iter()
            .map(|output| &output.key)
            .collect::<Vec<_>>(),
        colliding.recorders.output_power.records(),
    );
    assert_eq!(colliding.backend.platform.outputs[0].key, surviving_key);
    assert_ne!(colliding.backend.platform.outputs[0].key, removed_key);
    assert_eq!(
        CrtcKey::for_output(&colliding.backend.platform.outputs[0]),
        key_b,
        "card B reused output index zero with the same raw CRTC identity",
    );
    assert_eq!(output_binding(&colliding, 0), None);

    queue_cursor_topology_rescan(
        &mut colliding,
        &hotplug_elapsed,
        &[(properties_b.plane, key_b.crtc)],
    );
    let new_properties = cursor_properties(&colliding, 0);
    Backend::warp_pointer_root(&mut colliding.backend, &mut colliding.state, 900, 90);
    damage_cursor_root(&mut colliding);
    let mut new_owner_hardware = std::collections::HashMap::new();
    let mut new_event_frame = 82;
    let new_owner_frame = submit_and_verify_swap_frames_until_owner(
        &mut colliding,
        0,
        None,
        &mut new_owner_hardware,
        new_properties.plane,
        new_properties,
        new_properties.plane,
        new_properties,
        key_b.crtc,
        key_b.crtc,
        &mut new_event_frame,
        (900, 90),
        new_properties.plane,
        u64::from(u32::from(key_b.crtc)),
    );
    let new_output_x = colliding.backend.platform.outputs[0].x;
    assert_full_cursor_attach(
        &new_owner_frame.record,
        new_properties,
        key_b.crtc,
        i64::from(900_i32.saturating_sub(new_output_x)),
        90,
    );
    assert_swap_frame_pending_pixels(&mut colliding, &new_owner_frame);
    let new_binding = output_binding(&colliding, 0).expect("B accepts a new cursor owner");
    let new_lease_users = cursor_lease_users_for_output(&colliding, 0, new_binding.lease);
    assert_eq!(new_lease_users, 1);
    let device_a_fd = device_a.as_fd().as_raw_fd();
    colliding
        .recorders
        .page_flip_events
        .queue_flip_for_device(&device_a, key_a.crtc, 71);
    Backend::on_page_flip_ready(&mut colliding.backend, &mut colliding.state, device_a_fd);
    assert!(colliding.backend.scene.has_pending_page_flip(0));
    assert!(
        colliding
            .backend
            .platform
            .device_for_key(key_b.device_key)
            .unwrap()
            .cursor
            .crtc_states[&key_b.crtc]
            .in_flight
            .is_some(),
        "A's stale event cannot settle B's post-reuse cursor obligation",
    );
    assert_eq!(
        output_binding(&colliding, 0),
        Some(new_binding),
        "A's stale event cannot clear B's applied binding",
    );
    assert_eq!(
        cursor_lease_users_for_output(&colliding, 0, new_binding.lease),
        new_lease_users,
        "A's stale event cannot release B's new lease",
    );
    assert_eq!(colliding.recorders.page_flip_events.queued_flip_count(), 0);
    colliding.retire_output(0, 83);
    assert_eq!(
        cursor_lease_users_for_output(&colliding, 0, new_binding.lease),
        1,
        "B's own event retires its old reference but keeps its on-screen lease",
    );

    Backend::shutdown_outputs(&mut colliding.backend).expect("shut down surviving card B");
    colliding.assert_cursor_quiescent();
    assert!(colliding.backend.platform.staged_cursor_frames.is_empty());
    assert!(!colliding.backend.platform.cursor_reassignment_is_waiting());
    let ledger = Rc::clone(&colliding.recorders.cursor_buffer_ledger);
    drop(colliding);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: make the cursor-only lane read desired matching instead of the accepted binding; the P detach or Q attach ownership checks must fail. Read only OnScreen in the latest-composed oracle; the pending-frame pixel assertion must fail.
fn atomic_cursor_only_changed_matching_hides_applied_plane_before_showing_new_plane() {
    let Some(mut fixture) =
        AtomicCursorFixture::new_with_two_outputs().expect("build accepted-plane matching fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || base + Duration::from_millis(clock.get())));

    let crtc_a = fixture.backend.platform.outputs[0].output.crtc;
    let initial_plane_p = cursor_properties(&fixture, 0).plane;
    let initial_plane_q = cursor_properties(&fixture, 1).plane;
    assert_ne!(initial_plane_p, initial_plane_q);

    // Discovery inputs are queued before the scenario. Backend hotplug edges
    // below consume them at real topology boundaries.
    queue_cursor_topology_snapshot(&mut fixture, &[(initial_plane_p, crtc_a)]);
    queue_cursor_topology_snapshot(&mut fixture, &[(initial_plane_q, crtc_a)]);
    trigger_inventory_rescan(&mut fixture, &elapsed_ms);
    let plane_p = cursor_properties(&fixture, 0);
    assert_eq!(plane_p.plane, initial_plane_p);

    let root = fixture.backend.core.window_id;
    let cursor = super::round_b_producers::create_argb_cursor_extent(
        &mut fixture.backend,
        [0x20, 0x50, 0xf0, 0xff],
        4,
        4,
        0,
        0,
    );
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("select cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    let before_initial = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial P cursor frame");
    drive_backend_until_scene_cursor_frames(&mut fixture, before_initial, &[0]);
    let initial_records = output_records(&live_records(&fixture)[before_initial..], &fixture, 0);
    let initial = initial_records
        .iter()
        .find(|record| {
            record.result == ScriptedResult::Ok && record.kind == AtomicCommitKind::SceneWithCursor
        })
        .expect("accepted initial P frame");
    assert_full_cursor_attach(initial, plane_p, crtc_a, 100, 90);
    let binding_p = output_binding(&fixture, 0).expect("P is the accepted binding");
    assert_eq!(binding_p.properties.plane, plane_p.plane);
    assert_eq!(
        cursor_lease_users_for_output(&fixture, 0, binding_p.lease),
        1
    );
    let initial_pending = PendingSwapFrame {
        record: initial.clone(),
        output_idx: 0,
        event_frame: 1,
        hardware_bound: true,
        hardware_mode: true,
        cursor_root_position: (100, 90),
    };
    assert_swap_frame_pending_pixels(&mut fixture, &initial_pending);
    assert_swap_frame_primary_pixels(&mut fixture, &initial_pending);

    // A now desires Q while P remains applied and leased.
    trigger_inventory_rescan(&mut fixture, &elapsed_ms);
    let plane_q = cursor_properties(&fixture, 0);
    assert_eq!(plane_q.plane, initial_plane_q);
    assert_eq!(output_binding(&fixture, 0), Some(binding_p));
    assert_eq!(
        cursor_lease_users_for_output(&fixture, 0, binding_p.lease),
        1
    );

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    let before_hide = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("hide accepted P through cursor lane");
    let hide_records = output_records(&live_records(&fixture)[before_hide..], &fixture, 0);
    let hide = hide_records
        .iter()
        .find(|record| record.result == ScriptedResult::Ok)
        .expect("accepted P hide request");
    assert_eq!(hide.kind, AtomicCommitKind::CursorOnly);
    assert_eq!(property_value(hide, plane_p.plane, plane_p.fb_id), Some(0));
    assert_eq!(
        property_value(hide, plane_p.plane, plane_p.crtc_id),
        Some(0)
    );
    assert!(!record_sets_cursor_property(hide, plane_q));
    assert_eq!(output_binding(&fixture, 0), None);
    assert_eq!(
        cursor_lease_users_for_output(&fixture, 0, binding_p.lease),
        1
    );
    fixture.retire_output(0, 2);
    assert_eq!(
        cursor_lease_users_for_output(&fixture, 0, binding_p.lease),
        0,
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );

    Backend::set_cursor_hidden(&mut fixture.backend, false);
    damage_cursor_root(&mut fixture);
    let before_show = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("show on desired Q through Backend");
    drive_backend_until_scene_cursor_frames(&mut fixture, before_show, &[0]);
    let show_records = output_records(&live_records(&fixture)[before_show..], &fixture, 0);
    let show = show_records
        .iter()
        .find(|record| {
            record.result == ScriptedResult::Ok && record.kind == AtomicCommitKind::SceneWithCursor
        })
        .expect("accepted Q show frame");
    assert_full_cursor_attach(show, plane_q, crtc_a, 100, 90);
    assert!(!record_sets_cursor_property(show, plane_p));
    let binding_q = output_binding(&fixture, 0).expect("Q replaces the detached P binding");
    assert_eq!(binding_q.properties.plane, plane_q.plane);
    assert_eq!(
        cursor_lease_users_for_output(&fixture, 0, binding_p.lease),
        if binding_q.lease == binding_p.lease {
            1
        } else {
            0
        },
        "the P lease was released at detach and can count again only if Q reuses that exact lease",
    );
    assert_eq!(
        cursor_lease_users_for_output(&fixture, 0, binding_q.lease),
        1
    );
    let show_pending = PendingSwapFrame {
        record: show.clone(),
        output_idx: 0,
        event_frame: 3,
        hardware_bound: true,
        hardware_mode: true,
        cursor_root_position: (100, 90),
    };
    assert_swap_frame_pending_pixels(&mut fixture, &show_pending);
    assert_swap_frame_primary_pixels(&mut fixture, &show_pending);
    assert_eq!(output_binding(&fixture, 0), Some(binding_q));
    assert_eq!(
        cursor_lease_users_for_output(&fixture, 0, binding_p.lease),
        if binding_q.lease == binding_p.lease {
            1
        } else {
            0
        }
    );

    let kinds: Vec<_> = output_records(&live_records(&fixture), &fixture, 0)
        .iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            AtomicCommitKind::SceneWithCursor,
            AtomicCommitKind::CursorOnly,
            AtomicCommitKind::SceneWithCursor,
        ],
        "P attach, accepted P hide, then separate Q frame-owned show",
    );

    for output_idx in 0..fixture.backend.platform.outputs.len() {
        retire_if_pending(&mut fixture, output_idx, 4 + output_idx as u32);
    }
    Backend::shutdown_outputs(&mut fixture.backend).expect("drain cursor owners through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    fixture.shutdown_drawables();
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: keep held direct on eligibility loss; the moved cursor must appear in the accepted software unflip frame.
fn atomic_cursor_only_eligibility_loss_unflips_held_direct_cursor() {
    use super::{
        round_a::setup_cursor_only_direct_present,
        round_b_producers::{admit_held_direct_cursorless, create_argb_cursor_extent},
    };

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build held-direct cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || base + Duration::from_millis(clock.get())));

    let output = &fixture.backend.platform.outputs[0];
    let crtc = output.output.crtc;
    let plane_p = cursor_properties(&fixture, 0);
    // The empty inventory is queued before the scenario; Backend consumes it
    // only after the accepted direct frame is held.
    queue_cursor_topology_snapshot(&mut fixture, &[]);

    let direct = setup_cursor_only_direct_present(&mut fixture);
    let root = fixture.backend.core.window_id;
    let cursor =
        create_argb_cursor_extent(&mut fixture.backend, [0x20, 0x50, 0xf0, 0xff], 4, 4, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("select visible hardware cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    let before_initial = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("accept initial hardware cursor frame");
    drive_backend_until_scene_cursor_frames(&mut fixture, before_initial, &[0]);
    let initial = output_records(&live_records(&fixture)[before_initial..], &fixture, 0)
        .into_iter()
        .find(|record| {
            record.result == ScriptedResult::Ok && record.kind == AtomicCommitKind::SceneWithCursor
        })
        .expect("initial hardware cursor frame");
    assert_full_cursor_attach(&initial, plane_p, crtc, 100, 90);
    let binding = output_binding(&fixture, 0).expect("accepted hardware binding");
    assert_eq!(binding.properties.plane, plane_p.plane);
    assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 1);
    fixture.retire_output(0, 1);

    let before_direct = live_records(&fixture).len();
    admit_held_direct_cursorless(&mut fixture, &direct, 36_000, 20);
    assert!(fixture.backend.pstate.scanout_m2.hold_direct);
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert_eq!(output_binding(&fixture, 0), Some(binding));
    assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 1);

    trigger_inventory_rescan(&mut fixture, &elapsed_ms);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    assert_eq!(output_binding(&fixture, 0), Some(binding));
    assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 1);

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 140, 120);
    damage_cursor_root(&mut fixture);
    let before_unflip = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("eligibility loss requests the composed software-cursor unflip");
    assert!(!fixture.backend.pstate.scanout_m2.hold_direct);
    let unflip_records = output_records(&live_records(&fixture)[before_unflip..], &fixture, 0);
    assert_eq!(
        unflip_records.len(),
        1,
        "one accepted unflip frame: {unflip_records:?}"
    );
    let unflip = &unflip_records[0];
    assert_eq!(unflip.kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(unflip.result, ScriptedResult::Ok);
    assert_eq!(unflip.events.len(), 1);
    assert_eq!(
        property_value(unflip, plane_p.plane, plane_p.fb_id),
        Some(0)
    );
    assert_eq!(
        property_value(unflip, plane_p.plane, plane_p.crtc_id),
        Some(0)
    );
    assert_eq!(output_binding(&fixture, 0), None);
    assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 1);
    assert!(fixture.backend.pstate.scanout_m2.degraded_composed_unflip);
    assert!(!fixture.backend.scene.output_uses_hardware_cursor(0));
    let receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted software unflip receipt");
    assert!(
        matches!(
            receipt.primary_cursor,
            CursorPrimaryContent::Software {
                pos: (140, 120),
                ..
            }
        ),
        "unflip frame captures software cursor at moved pointer: {receipt:?}",
    );

    let pending = PendingSwapFrame {
        record: unflip.clone(),
        output_idx: 0,
        event_frame: 2,
        hardware_bound: false,
        hardware_mode: false,
        cursor_root_position: (140, 120),
    };
    assert_swap_frame_primary_pixels(&mut fixture, &pending);
    assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 0);
    assert!(!fixture.backend.pstate.scanout_m2.active());
    assert!(!fixture.backend.pstate.scanout_m2.hold_direct);
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );

    let accepted_after_initial: Vec<_> =
        output_records(&live_records(&fixture)[before_direct..], &fixture, 0)
            .into_iter()
            .filter(|record| record.result == ScriptedResult::Ok)
            .map(|record| record.kind)
            .collect();
    assert_eq!(
        accepted_after_initial,
        vec![AtomicCommitKind::Scene, AtomicCommitKind::SceneWithCursor],
        "the held direct frame retains the accepted cursor, then one SW+detach unflip follows motion",
    );

    Backend::shutdown_outputs(&mut fixture.backend).expect("drain output through Backend");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    fixture.shutdown_drawables();
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: queue ordinary Hide on eligibility loss; a cursor-only detach during any no-BO frame must fail.
fn atomic_cursor_only_visible_eligibility_loss_stays_frame_owned_across_skips() {
    let Some(mut fixture) =
        AtomicCursorFixture::new_with_scanout_acquisition_script([false, true, true, true])
            .expect("build visible eligibility-loss fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || base + Duration::from_millis(clock.get())));

    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let plane_p = cursor_properties(&fixture, 0);
    queue_cursor_topology_snapshot(&mut fixture, &[]);

    let root = fixture.backend.core.window_id;
    let cursor = super::round_b_producers::create_argb_cursor_extent(
        &mut fixture.backend,
        [0x20, 0x50, 0xf0, 0xff],
        4,
        4,
        0,
        0,
    );
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("select visible hardware cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    let before_initial = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("accept initial hardware cursor frame");
    drive_backend_until_scene_cursor_frames(&mut fixture, before_initial, &[0]);
    let initial = output_records(&live_records(&fixture)[before_initial..], &fixture, 0)
        .into_iter()
        .find(|record| {
            record.result == ScriptedResult::Ok && record.kind == AtomicCommitKind::SceneWithCursor
        })
        .expect("accepted initial hardware cursor frame");
    assert_full_cursor_attach(&initial, plane_p, crtc, 100, 90);
    let binding = output_binding(&fixture, 0).expect("accepted hardware binding");
    assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 1);
    fixture.retire_output(0, 1);

    trigger_inventory_rescan(&mut fixture, &elapsed_ms);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    assert_eq!(output_binding(&fixture, 0), Some(binding));
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 140, 120);
    damage_cursor_root(&mut fixture);

    for skipped_frame in 0..3 {
        let before_skip = live_records(&fixture).len();
        Backend::maybe_composite(&mut fixture.backend)
            .expect("leave the software-cursor frame pending without a BO");
        let skipped_records = output_records(&live_records(&fixture)[before_skip..], &fixture, 0);
        assert!(
            skipped_records.is_empty(),
            "skipped software frame {skipped_frame} has no primary or cursor-only request: {skipped_records:?}",
        );
        assert_eq!(output_binding(&fixture, 0), Some(binding));
        assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
        assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 1);
        let cursor_pixel = read_pixel(&mut fixture, 140, 120);
        let clear_pixel = read_pixel(&mut fixture, 148, 120);
        assert_eq!(
            cursor_pixel[..3],
            clear_pixel[..3],
            "the old primary remains cursorless while P supplies the visible cursor",
        );
    }

    let before_accepted = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("accept the newest software-cursor frame with P detached");
    drive_backend_until_scene_cursor_frames(&mut fixture, before_accepted, &[0]);
    let accepted = output_records(&live_records(&fixture)[before_accepted..], &fixture, 0)
        .into_iter()
        .find(|record| {
            record.result == ScriptedResult::Ok && record.kind == AtomicCommitKind::SceneWithCursor
        })
        .expect("accepted software handoff frame");
    assert_eq!(
        property_value(&accepted, plane_p.plane, plane_p.fb_id),
        Some(0)
    );
    assert_eq!(
        property_value(&accepted, plane_p.plane, plane_p.crtc_id),
        Some(0)
    );
    assert_eq!(output_binding(&fixture, 0), None);
    assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 1);
    let receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted software-frame receipt");
    assert!(matches!(
        receipt.primary_cursor,
        CursorPrimaryContent::Software {
            pos: (140, 120),
            ..
        }
    ));
    let pending = PendingSwapFrame {
        record: accepted,
        output_idx: 0,
        event_frame: 2,
        hardware_bound: false,
        hardware_mode: false,
        cursor_root_position: (140, 120),
    };
    assert_swap_frame_pending_pixels(&mut fixture, &pending);
    assert_swap_frame_primary_pixels(&mut fixture, &pending);
    assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 0);

    let accepted_kinds: Vec<_> = output_records(&live_records(&fixture), &fixture, 0)
        .iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        accepted_kinds,
        [
            AtomicCommitKind::SceneWithCursor,
            AtomicCommitKind::SceneWithCursor
        ],
        "P attach then only the accepted software frame detaches P",
    );

    Backend::shutdown_outputs(&mut fixture.backend).expect("drain output through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    fixture.shutdown_drawables();
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: mark awaiting before acceptance, retire unflip on any event, or skip direct-shadow materialization; each must fail its pending, pin, or content assertion.
fn atomic_cursor_only_held_direct_unflip_tracks_accepted_primary_event_and_content() {
    use super::{
        round_a::setup_cursor_only_direct_present,
        round_b_producers::{admit_held_direct_cursorless, create_argb_cursor_extent},
    };

    let Some(mut fixture) =
        AtomicCursorFixture::new_with_scanout_acquisition_script([false, true, false, false])
            .expect("build delayed held-direct unflip fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let clock = Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || base + Duration::from_millis(clock.get())));

    let output = &fixture.backend.platform.outputs[0];
    let crtc = output.output.crtc;
    let plane_p = cursor_properties(&fixture, 0);
    queue_cursor_topology_snapshot(&mut fixture, &[]);
    let direct = setup_cursor_only_direct_present(&mut fixture);
    let root = fixture.backend.core.window_id;
    let cursor =
        create_argb_cursor_extent(&mut fixture.backend, [0x20, 0x50, 0xf0, 0xff], 4, 4, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("select visible hardware cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    let before_initial = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("accept initial hardware cursor frame");
    drive_backend_until_scene_cursor_frames(&mut fixture, before_initial, &[0]);
    let initial = output_records(&live_records(&fixture)[before_initial..], &fixture, 0)
        .into_iter()
        .find(|record| {
            record.result == ScriptedResult::Ok && record.kind == AtomicCommitKind::SceneWithCursor
        })
        .expect("accepted initial hardware cursor frame");
    assert_full_cursor_attach(&initial, plane_p, crtc, 100, 90);
    let binding = output_binding(&fixture, 0).expect("accepted hardware binding");
    fixture.retire_output(0, 1);

    let direct_id = admit_held_direct_cursorless(&mut fixture, &direct, 36_000, 20);
    assert!(fixture.backend.pstate.scanout_m2.hold_direct);
    assert_eq!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .current
            .as_ref()
            .unwrap()
            .candidate
            .present_id,
        direct_id
    );
    assert_eq!(output_binding(&fixture, 0), Some(binding));
    let old_composed_pixel = read_pixel(&mut fixture, 20, 20);
    Backend::put_image(
        &mut fixture.backend,
        None,
        direct.source_xid,
        24,
        1,
        1,
        20,
        20,
        &[0x17, 0x83, 0xe1, 0xff],
    )
    .expect("update the held direct source through Backend");

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 130, 110);
    let before_cursor_motion = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("move the visible cursor over the held direct frame");
    let cursor_motion =
        output_records(&live_records(&fixture)[before_cursor_motion..], &fixture, 0)
            .into_iter()
            .find(|record| record.result == ScriptedResult::Ok)
            .expect("accepted held-direct cursor-only motion");
    assert_eq!(cursor_motion.kind, AtomicCommitKind::CursorOnly);
    assert_eq!(output_binding(&fixture, 0).unwrap().x, 130);
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    let current = fixture.backend.pstate.scanout_m2.current.as_ref().unwrap();
    let source_id = current.source_id;
    let fallback_id = current.fallback_target.backing_id();
    let source_pin = current.source_pin;
    let fallback_pin = current.fallback_target_pin;
    assert_eq!(
        fixture.backend.present_source_pins.get(&source_pin),
        Some(&source_id)
    );
    assert_eq!(
        fixture.backend.present_source_pins.get(&fallback_pin),
        Some(&fallback_id)
    );

    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    trigger_inventory_rescan(&mut fixture, &elapsed_ms);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 140, 120);
    damage_cursor_root(&mut fixture);
    let before_wait = output_records(&live_records(&fixture), &fixture, 0).len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("eligibility-loss replacement waits for the cursor-only event");
    assert_eq!(
        output_records(&live_records(&fixture), &fixture, 0).len(),
        before_wait,
        "no primary replacement is accepted while its cursor-only event is pending",
    );
    assert!(fixture.backend.pstate.scanout_m2.unflip_requested);
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert_eq!(
        fixture.backend.present_source_pins.get(&source_pin),
        Some(&source_id)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    let primary_device = fixture
        .backend
        .platform
        .device_for_key(fixture.backend.platform.outputs[0].key.device_key)
        .expect("primary device");
    let cursor_event_id = *primary_device
        .atomic_commit_sink
        .for_tests_pending_page_flip_commit_ids(&primary_device.device, crtc)
        .last()
        .expect("accepted held-direct cursor-only event identity");

    fixture.retire_output_with_commit_id(0, 2, cursor_event_id);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert_eq!(
        fixture.backend.present_source_pins.get(&source_pin),
        Some(&source_id)
    );
    assert_eq!(
        fixture.backend.present_source_pins.get(&fallback_pin),
        Some(&fallback_id),
        "a cursor-only event cannot retire the direct frame",
    );

    let primary_fb = fixture.backend.platform.outputs[0].output.plane_fb_id_prop;
    let before_skip = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend).expect("skip first replacement without a BO");
    assert!(output_records(&live_records(&fixture)[before_skip..], &fixture, 0).is_empty());
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert_eq!(
        fixture.backend.present_source_pins.get(&source_pin),
        Some(&source_id)
    );

    let before_rejected = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("reject the second primary replacement and retain the direct frame");
    let rejected = output_records(&live_records(&fixture)[before_rejected..], &fixture, 0);
    let rejected_primary = rejected
        .iter()
        .find(|record| record.result != ScriptedResult::Ok && record.events.len() == 1)
        .expect("second replacement reaches and is rejected by the recording atomic sink");
    assert!(rejected_primary.properties.iter().any(|update| {
        update.object == fixture.backend.platform.outputs[0].output.plane.into()
            && update.property == primary_fb
    }));
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert_eq!(
        fixture.backend.present_source_pins.get(&source_pin),
        Some(&source_id)
    );

    let before_accepted = live_records(&fixture).len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("third replacement accepts the current software-cursor frame");
    drive_backend_until_scene_cursor_frames(&mut fixture, before_accepted, &[0]);
    let accepted = output_records(&live_records(&fixture)[before_accepted..], &fixture, 0)
        .into_iter()
        .find(|record| {
            record.result == ScriptedResult::Ok && record.kind == AtomicCommitKind::SceneWithCursor
        })
        .expect("accepted third replacement");
    assert_eq!(
        property_value(&accepted, plane_p.plane, plane_p.fb_id),
        Some(0)
    );
    assert_eq!(
        property_value(&accepted, plane_p.plane, plane_p.crtc_id),
        Some(0)
    );
    assert_eq!(output_binding(&fixture, 0), None);
    let replacement_event_id = *fixture
        .backend
        .pstate
        .scanout_m2
        .unflip_awaiting_outputs
        .get(&0)
        .expect("accepted primary replacement event identity");
    assert!(cursor_event_id < replacement_event_id);
    assert_eq!(
        fixture.backend.pstate.scanout_m2.unflip_awaiting_outputs,
        [(0, replacement_event_id)].into(),
        "only the accepted primary replacement creates its event obligation",
    );
    assert_eq!(
        fixture.backend.pstate.scanout_m2.unflip_outputs_to_replace,
        [0].into(),
        "the accepted output remains direct until its own primary event retires",
    );
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert_eq!(
        fixture.backend.present_source_pins.get(&source_pin),
        Some(&source_id)
    );
    assert_eq!(
        fixture.backend.present_source_pins.get(&fallback_pin),
        Some(&fallback_id)
    );
    assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 1);
    let receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted unflip scene receipt");
    assert!(matches!(
        receipt.primary_cursor,
        CursorPrimaryContent::Software {
            pos: (140, 120),
            ..
        }
    ));
    let pending = PendingSwapFrame {
        record: accepted.clone(),
        output_idx: 0,
        event_frame: 3,
        hardware_bound: false,
        hardware_mode: false,
        cursor_root_position: (140, 120),
    };
    for stale_event_id in [cursor_event_id, cursor_event_id - 1] {
        fixture.retire_output_with_commit_id(0, 30, stale_event_id);
        assert!(fixture.backend.pstate.scanout_m2.active());
        assert!(fixture.backend.pstate.scanout_m2.current.is_some());
        assert_eq!(
            fixture.backend.pstate.scanout_m2.unflip_awaiting_outputs,
            [(0, replacement_event_id)].into(),
            "an unrelated or duplicate earlier event cannot retire the accepted replacement",
        );
        assert_eq!(
            fixture.backend.present_source_pins.get(&source_pin),
            Some(&source_id)
        );
        assert_eq!(
            fixture.backend.present_source_pins.get(&fallback_pin),
            Some(&fallback_id)
        );
        assert!(fixture.backend.scene.has_pending_page_flip(0));
    }
    fixture.retire_output_with_commit_id(0, pending.event_frame, replacement_event_id);
    assert!(
        fixture
            .backend
            .platform
            .retained_composed_framebuffer(pending.output_idx)
            .is_some(),
        "the accepted primary is the output's on-screen scanout BO: {:?}",
        pending.record,
    );
    assert_swap_frame_primary_content(
        &mut fixture,
        &pending,
        SwapFrameReadSelection::PermissiveDump,
    );
    assert!(!fixture.backend.pstate.scanout_m2.active());
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_outputs_to_replace
            .is_empty()
    );
    assert!(fixture.backend.present_source_pins.is_empty());
    assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 0);
    let materialized_pixel = read_pixel(&mut fixture, 20, 20);
    assert_ne!(
        materialized_pixel[..3],
        old_composed_pixel[..3],
        "the accepted software primary contains the newest held-direct source content",
    );

    let accepted_kinds: Vec<_> = output_records(&live_records(&fixture), &fixture, 0)
        .iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        accepted_kinds,
        [
            AtomicCommitKind::SceneWithCursor,
            AtomicCommitKind::Scene,
            AtomicCommitKind::CursorOnly,
            AtomicCommitKind::SceneWithCursor,
        ],
        "initial HW frame, the direct-admission scene, held-direct cursor motion, accepted SW replacement",
    );

    Backend::shutdown_outputs(&mut fixture.backend).expect("drain output through Backend");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    fixture.shutdown_drawables();
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: record an awaiting replacement as soon as the copied scene reports Composed; this must fail before KMS acceptance.
// Mutation: attribute event to oldest queued entry; the late initial identity must not retire the accepted replacement.
// Mutation: leave user_data at 0 in the commit request; the accepted identity assertions must fail.
fn atomic_cursor_only_copied_unflip_awaits_only_an_accepted_replacement() {
    use super::{
        round_a::setup_cursor_only_direct_present,
        round_b_producers::{admit_held_direct_cursorless, create_argb_cursor_extent},
    };

    let elapsed_ms = Rc::new(Cell::new(0_u64));
    let base = std::time::Instant::now();
    let clock = Rc::clone(&elapsed_ms);
    let monotonic_clock = Rc::new(move || base + Duration::from_millis(clock.get()));
    let Some(mut fixture) =
        AtomicCursorFixture::new_with_hotspot_properties_and_clock(monotonic_clock)
            .expect("build copied held-direct unflip fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let kms_key = fixture.backend.platform.outputs[0].key.device_key;
    let render_device_id = fixture.backend.platform.outputs[0]
        .scanout_route
        .render_device_id;
    let route = crate::kms::scanout_route::ScanoutRoute::new(
        render_device_id,
        kms_key,
        crate::kms::scanout_route::RenderKmsRelationship::Different,
    );
    fixture
        .install_copied_recording_route(route)
        .expect("install copied route on the present KMS device");
    for output in &mut fixture.backend.platform.outputs {
        output.scanout_route = route;
    }
    let layouts = fixture.backend.platform.scene_output_layouts();
    fixture.backend.scene =
        crate::kms::render::scene::SceneCompositor::new(&fixture.backend.platform, &layouts)
            .expect("rebuild copied route scene");
    let atomic = fixture.recorders.atomic.clone();
    assert_eq!(
        crate::kms::render::scene::OutputSink::route(&fixture.backend.platform, 0),
        crate::kms::render::scene::OutputRoute::Copied,
    );
    let direct = setup_cursor_only_direct_present(&mut fixture);
    let root = fixture.backend.core.window_id;
    let cursor =
        create_argb_cursor_extent(&mut fixture.backend, [0x20, 0x50, 0xf0, 0xff], 4, 4, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("select visible hardware cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("compose initial copied cursor frame");
    let initial = live_records_from(&atomic)
        .into_iter()
        .find(|record| {
            record.kind == AtomicCommitKind::SceneWithCursor && record.result == ScriptedResult::Ok
        })
        .expect("accepted copied hardware cursor frame");
    let initial_event_id = initial.user_data;
    assert_ne!(
        initial_event_id, 0,
        "an accepted PAGE_FLIP_EVENT commit carries its identity",
    );
    let binding = output_binding(&fixture, 0).expect("accepted copied hardware cursor binding");
    fixture.retire_output_with_commit_id(0, 1, initial_event_id);

    queue_cursor_topology_snapshot(&mut fixture, &[]);
    let direct_id = admit_held_direct_cursorless(&mut fixture, &direct, 36_000, 20);
    assert_eq!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .current
            .as_ref()
            .expect("held direct frame")
            .candidate
            .present_id,
        direct_id
    );
    let current = fixture.backend.pstate.scanout_m2.current.as_ref().unwrap();
    let source_pin = current.source_pin;
    let fallback_pin = current.fallback_target_pin;
    let source_id = current.source_id;
    let fallback_id = current.fallback_target.backing_id();

    trigger_inventory_rescan(&mut fixture, &elapsed_ms);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    fixture
        .recorders
        .render_completion_readiness
        .hold_ready_jobs();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 140, 120);
    damage_cursor_root(&mut fixture);
    let before_unflip = live_records_from(&atomic).len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("compose copied software-cursor unflip before KMS submission");
    let unflip_job = *fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_ids()
        .first()
        .expect("copied unflip is waiting for render completion");
    assert!(fixture.backend.pstate.scanout_m2.degraded_composed_unflip);
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty(),
        "Composed and WaitingForRenderCompletion do not create KMS event obligations",
    );
    assert_eq!(
        fixture.backend.present_source_pins.get(&source_pin),
        Some(&source_id)
    );
    assert_eq!(
        fixture.backend.present_source_pins.get(&fallback_pin),
        Some(&fallback_id)
    );
    assert_eq!(output_binding(&fixture, 0), Some(binding));

    atomic.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(unflip_job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let rejected = live_records_from(&atomic)[before_unflip..]
        .iter()
        .find(|record| record.result != ScriptedResult::Ok && record.events.len() == 1)
        .cloned()
        .expect("copied replacement atomic submission was rejected");
    assert_eq!(rejected.kind, AtomicCommitKind::SceneWithCursor);
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert_eq!(
        fixture.backend.present_source_pins.get(&source_pin),
        Some(&source_id)
    );
    assert_eq!(
        fixture.backend.present_source_pins.get(&fallback_pin),
        Some(&fallback_id)
    );
    assert_eq!(output_binding(&fixture, 0), Some(binding));
    assert!(!fixture.backend.scene.has_pending_page_flip(0));

    let deadline = fixture
        .backend
        .scene
        .earliest_retry_deadline()
        .expect("rejected copied replacement schedules a real retry");
    while std::time::Instant::now() < deadline {
        let _ = Backend::maybe_composite(&mut fixture.backend);
        std::thread::yield_now();
    }
    Backend::maybe_composite(&mut fixture.backend).expect("recompose copied replacement");
    let accepted_job = *fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_ids()
        .first()
        .expect("retry composes another copied replacement");
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(accepted_job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let accepted = live_records_from(&atomic)[before_unflip..]
        .iter()
        .rev()
        .find(|record| record.result == ScriptedResult::Ok && record.events.len() == 1)
        .cloned()
        .expect("retry copied replacement was accepted");
    assert_eq!(accepted.kind, AtomicCommitKind::SceneWithCursor);
    let accepted_event_id = *fixture
        .backend
        .pstate
        .scanout_m2
        .unflip_awaiting_outputs
        .get(&0)
        .expect("accepted copied replacement event identity");
    assert_eq!(
        accepted.user_data, accepted_event_id,
        "the pending entry is keyed by the identity the kernel echoes",
    );
    assert_eq!(
        fixture.backend.pstate.scanout_m2.unflip_awaiting_outputs,
        [(0, accepted_event_id)].into()
    );
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert_eq!(
        fixture.backend.present_source_pins.get(&source_pin),
        Some(&source_id)
    );
    assert_eq!(
        fixture.backend.present_source_pins.get(&fallback_pin),
        Some(&fallback_id)
    );
    assert_eq!(output_binding(&fixture, 0), None);

    // The initial hardware frame's event retired above, so its entry is gone.
    // Its late identity matches nothing: it retires nothing and R keeps its
    // pins. Mutation: attribute event to oldest queued entry (R is retired here).
    fixture.retire_output_with_commit_id(0, 1, initial_event_id);
    assert!(fixture.backend.pstate.scanout_m2.active());
    assert_eq!(
        fixture.backend.pstate.scanout_m2.unflip_awaiting_outputs,
        [(0, accepted_event_id)].into(),
    );
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert_eq!(
        fixture.backend.present_source_pins.get(&source_pin),
        Some(&source_id)
    );
    assert_eq!(
        fixture.backend.present_source_pins.get(&fallback_pin),
        Some(&fallback_id)
    );
    assert_eq!(output_binding(&fixture, 0), None);

    fixture.retire_output_with_commit_id(0, 2, accepted_event_id);
    assert!(!fixture.backend.pstate.scanout_m2.active());
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert!(fixture.backend.present_source_pins.is_empty());
    assert_eq!(cursor_lease_users_for_output(&fixture, 0, binding.lease), 0);
    for record in live_records_from(&atomic) {
        if record.result == ScriptedResult::Ok
            && record.flags.contains(AtomicCommitFlags::PAGE_FLIP_EVENT)
        {
            assert_ne!(
                record.user_data, 0,
                "every accepted PAGE_FLIP_EVENT commit carries its identity",
            );
        }
    }
    Backend::shutdown_outputs(&mut fixture.backend).expect("drain copied output through Backend");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    fixture.shutdown_drawables();
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}
