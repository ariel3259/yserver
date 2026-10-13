use super::*;

use crate::drm::atomic::{
    AtomicCommitKind, AtomicCommitSink,
    tests_support::{AtomicCommitRecord, RecordingAtomicCommitExecutor},
};

fn recording_sink(
    backend: &mut KmsBackend,
    device_idx: usize,
) -> Rc<RecordingAtomicCommitExecutor> {
    let recorder = Rc::new(RecordingAtomicCommitExecutor::default());
    backend.platform.devices[device_idx].atomic_commit_sink =
        Rc::new(AtomicCommitSink::new(recorder.clone()));
    recorder
}

fn raw<T: ::drm::control::ResourceHandle>(value: u32) -> T {
    ::drm::control::from_u32(value).expect("nonzero test handle")
}

fn assert_cursor_properties(
    record: &AtomicCommitRecord,
    position: crate::drm::atomic::CursorPlanePosition,
) {
    for (property, value) in [
        (position.crtc_x, i64::from(position.x) as u64),
        (position.crtc_y, i64::from(position.y) as u64),
    ] {
        assert!(
            record.properties.iter().any(|update| {
                update.object == position.plane.into()
                    && update.property == property
                    && update.value == value
            }),
            "missing cursor property {property:?}={value} on {:?}: {:?}",
            position.plane,
            record.properties,
        );
    }
}

fn set_scene_fence_properties(output: &mut crate::platform::drm::Output) {
    output.plane_in_fence_fd_prop = Some(raw(81));
    output.crtc_out_fence_ptr_prop = Some(raw(82));
}

#[test]
// Mutation: omit `request.add_cursor_position` from the shared scene-flip
// builder. The recorded request must then miss these latest-position values.
fn shared_scene_flip_records_the_latest_cursor_position() {
    let mut backend = KmsBackend::for_tests();
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    let position = backend.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(51),
        raw(61),
        raw(62),
        123,
        45,
        3,
        4,
    );
    assert_eq!((position.x, position.y), (120, 41));
    let positions = backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert_eq!(positions, vec![position]);

    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let output = &backend.platform.outputs[0].output;
    let mut out_fence = -1;
    crate::drm::page_flip::submit_scene_flip_with_fences(
        &backend.platform.devices[0].device,
        &backend.platform.devices[0].atomic_commit_sink,
        output,
        raw(71),
        -1,
        &mut out_fence,
        &positions,
    )
    .expect("record shared scene flip");

    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, AtomicCommitKind::SceneWithCursor);
    assert!(
        records[0]
            .flags
            .contains(::drm::control::AtomicCommitFlags::NONBLOCK)
    );
    assert_cursor_properties(&records[0], position);
    backend
        .platform
        .cursor_plane_scene_commit_submitted(&positions);
    assert!(
        !backend
            .platform
            .for_tests_atomic_position_is_unsent(device_key, position.crtc)
    );
}

#[test]
// Mutation: source the copied flip's cursor plane from the renderer device.
// Both devices intentionally reuse the raw CRTC handle with different planes.
fn copied_scene_flip_uses_the_sink_devices_cursor_plane() {
    let mut backend = KmsBackend::for_tests();
    let renderer_key = backend.platform.devices[0].key;
    let sink_key = test_device_key(77);
    push_test_device(&mut backend, sink_key);
    backend.platform.outputs[0].key.device_key = renderer_key;
    let renderer_position = backend.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(151),
        raw(161),
        raw(162),
        100,
        90,
        0,
        0,
    );
    backend.platform.outputs[0].key.device_key = sink_key;
    let sink_position = backend.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(251),
        raw(261),
        raw(262),
        42,
        33,
        0,
        0,
    );
    assert_ne!(renderer_position.plane, sink_position.plane);

    let recorder = recording_sink(&mut backend, 1);
    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let positions = backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert_eq!(positions, vec![sink_position]);
    let sink_device = &backend.platform.devices[1];
    let mut out_fence = -1;
    crate::drm::page_flip::submit_scene_flip_with_fences(
        &sink_device.device,
        &sink_device.atomic_commit_sink,
        &backend.platform.outputs[0].output,
        raw(71),
        -1,
        &mut out_fence,
        &positions,
    )
    .expect("record copied sink flip");

    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, AtomicCommitKind::SceneWithCursor);
    assert_cursor_properties(&records[0], sink_position);
    backend
        .platform
        .cursor_plane_scene_commit_submitted(&positions);
    assert!(
        backend
            .platform
            .for_tests_atomic_position_is_unsent(renderer_key, renderer_position.crtc)
    );
    assert!(
        !backend
            .platform
            .for_tests_atomic_position_is_unsent(sink_key, sink_position.crtc)
    );
}

#[test]
// Mutation: return atomic positions for every visible plane without checking
// the per-device atomic capability. The legacy request would gain cursor fields.
fn legacy_scene_flip_has_no_atomic_cursor_properties() {
    let mut backend = KmsBackend::for_tests();
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    let position = backend.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(51),
        raw(61),
        raw(62),
        123,
        45,
        3,
        4,
    );
    backend.platform.devices[0].cursor.atomic_cursor = false;
    let positions = backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert!(positions.is_empty());

    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let mut out_fence = -1;
    crate::drm::page_flip::submit_scene_flip_with_fences(
        &backend.platform.devices[0].device,
        &backend.platform.devices[0].atomic_commit_sink,
        &backend.platform.outputs[0].output,
        raw(71),
        -1,
        &mut out_fence,
        &positions,
    )
    .expect("record legacy scene flip");

    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, AtomicCommitKind::Scene);
    assert!(
        !records[0]
            .properties
            .iter()
            .any(|update| update.object == position.plane.into())
    );
    assert!(
        backend
            .platform
            .for_tests_atomic_position_is_unsent(device_key, position.crtc)
    );
}
