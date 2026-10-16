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

fn backend_with_live_scene() -> Option<KmsBackend> {
    use yserver_core::backend::Backend;

    let mut fixture = match super::atomic_cursor_only::support::AtomicCursorFixture::new() {
        Ok(Some(fixture)) => fixture,
        Ok(None) => return None,
        Err(error) => {
            eprintln!("skipping: no Vulkan test fixture: {error}");
            return None;
        }
    };
    fixture.define_visible_cursor();
    let root = fixture.backend.core.window_id;
    let width = fixture.backend.platform.fb_w;
    let height = fixture.backend.platform.fb_h;
    Backend::clear_area(
        &mut fixture.backend,
        None,
        root,
        0,
        None,
        0,
        0,
        width,
        height,
        (0, 0),
    )
    .expect("damage root through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit visible cursor scene");
    fixture.retire_output(0, 1);
    assert!(fixture.backend.platform.any_cursor_commit_in_flight());
    fixture.retire_output(0, 2);
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    Some(fixture.backend)
}

fn install_visible_cursor_via_backend(backend: &mut KmsBackend) {
    use yserver_core::backend::Backend;

    let source = Backend::create_pixmap(backend, None, 1, 1, 1).expect("cursor source pixmap");
    Backend::put_image(backend, None, source.as_raw(), 1, 1, 1, 0, 0, &[1, 0, 0, 0])
        .expect("paint visible cursor pixel");
    let cursor = Backend::create_cursor(
        backend,
        None,
        source,
        None,
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
        0,
        0,
    )
    .expect("create visible cursor");
    Backend::define_cursor(backend, None, backend.core.window_id, cursor.as_raw())
        .expect("define visible root cursor");
    assert!(!backend.effective_cursor_hidden());
}

fn backend_with_hidden_dead_zone_scene() -> Option<KmsBackend> {
    let mut backend = match KmsBackend::for_tests_with_vk() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("skipping: no Vulkan test device: {error}");
            return None;
        }
    };
    push_test_output(&mut backend, 0xCA11);
    backend.platform.outputs[1].x = 900;
    backend.platform.outputs[1].width = 800;
    backend.platform.outputs[1].height = 400;
    backend.platform.fb_w = 1700;
    backend.platform.fb_h = 600;
    let layouts = backend.platform.scene_output_layouts();
    backend.scene = crate::kms::render::scene::SceneCompositor::new(&backend.platform, &layouts)
        .expect("construct dead-zone live scene");
    backend.scene.scene_structure_dirty = false;
    for drawable in backend.store.armed_damaged_ids() {
        if let Some(snapshot) = backend.store.peek_presentation_damage(drawable) {
            backend.store.ack_presentation_damage(snapshot);
        }
    }
    Some(backend)
}

fn warp(backend: &mut KmsBackend, state: &mut yserver_core::server::ServerState, x: i32, y: i32) {
    use yserver_core::backend::Backend;
    Backend::warp_pointer_root(backend, state, x, y);
}

fn maybe_composite(backend: &mut KmsBackend) {
    use yserver_core::backend::Backend;
    Backend::maybe_composite(backend).expect("run backend scheduling pass");
}

fn retire_test_flip(
    backend: &mut KmsBackend,
    state: &mut yserver_core::server::ServerState,
    frame: u32,
) {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    use yserver_core::backend::Backend;

    let output = &backend.platform.outputs[0].output;
    let crtc = output.crtc;
    let device = &backend.platform.devices[0].device;
    let source = backend.platform.devices[0]
        .test_page_flip_event_source
        .as_ref()
        .expect("test event source");
    source.register_device(device);
    source.queue_flip_for_device(device, crtc, frame);
    let drm_fd = backend.platform.devices[0].device.as_fd().as_raw_fd();
    Backend::on_page_flip_ready(backend, state, drm_fd);
}

fn assert_no_cursor_work(backend: &KmsBackend, device_key: crate::platform::drm::DrmDeviceKey) {
    let crtc = backend.platform.outputs[0].output.crtc;
    assert!(
        !backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(device_key, crtc)
    );
    assert!(
        backend
            .platform
            .for_tests_atomic_cursor_pending_position(device_key, crtc)
            .is_none()
    );
}

#[test]
// Mutation: retain the first position instead of replacing it with the latest.
fn idle_motion_coalesces_until_cursor_flip_retires() {
    use yserver_core::server::ServerState;

    let Some(mut backend) = backend_with_live_scene() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    let mut state = ServerState::new();

    warp(&mut backend, &mut state, 120, 80);
    warp(&mut backend, &mut state, 180, 130);
    let first_pending = backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert_eq!(first_pending.len(), 1);
    assert!(first_pending[0].motion_timestamp.is_some());
    assert!(
        recorder.records().is_empty(),
        "motion must not submit directly"
    );
    assert!(backend.platform.has_eligible_atomic_cursor_move());
    assert!(backend.scanout_allowed());
    assert!(backend.kms_outputs_active);
    maybe_composite(&mut backend);
    assert_eq!(
        backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_motion_to_visible_sample_count(),
        0,
        "motion is not visible until its commit retires"
    );

    let first_records = recorder.records();
    assert_eq!(first_records.len(), 1);
    assert_eq!(first_records[0].kind, AtomicCommitKind::CursorOnly);
    assert_cursor_properties(&first_records[0], first_pending[0]);
    assert!(
        backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(device_key, first_pending[0].crtc)
    );
    assert!(
        !backend
            .platform
            .for_tests_atomic_position_is_unsent(device_key, first_pending[0].crtc)
    );

    warp(&mut backend, &mut state, 260, 170);
    let latest = backend
        .platform
        .for_tests_atomic_cursor_pending_position(device_key, first_pending[0].crtc)
        .expect("motion during the cursor flip stays pending");
    assert!(latest.motion_timestamp.is_some());
    maybe_composite(&mut backend);
    assert_eq!(
        recorder.records().len(),
        1,
        "the in-flight commit owns the CRTC"
    );

    retire_test_flip(&mut backend, &mut state, 1);
    assert_eq!(
        backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_motion_to_visible_sample_count(),
        1,
        "the first cursor flip records motion-to-visible"
    );

    let records = recorder.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].kind, AtomicCommitKind::CursorOnly);
    assert_cursor_properties(&records[1], latest);
    assert!(
        backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(device_key, latest.crtc)
    );
    retire_test_flip(&mut backend, &mut state, 2);
    assert_eq!(
        backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_motion_to_visible_sample_count(),
        2,
        "the latest cursor flip records motion-to-visible"
    );
    assert_no_cursor_work(&backend, device_key);
}

#[test]
// Mutation: restore Task 6's Hidden fast-path skip. Re-entry must wake the
// scene through the Backend pointer entry and make this predicate true.
fn hidden_cursor_motion_from_dead_zone_wakes_scene_on_reentry() {
    use yserver_core::{backend::Backend, server::ServerState};

    let Some(mut backend) = backend_with_hidden_dead_zone_scene() else {
        return;
    };
    install_visible_cursor_via_backend(&mut backend);
    let mut state = ServerState::new();
    Backend::warp_pointer_root(&mut backend, &mut state, 850, 450);
    // Model the initial retirement at a location covered by neither output.
    backend.scene.scene_structure_dirty = false;
    assert!(!backend.cursor_hidden, "this is not XFIXES HideCursor");
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
    );
    assert!(!backend.for_tests_scene_wants_compose());

    // The gap and the shorter output's lower edge form the dead zone.
    assert_eq!(
        (backend.core.cursor_x, backend.core.cursor_y),
        (850.0, 450.0)
    );
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
    );
    // Model the scene consuming that wake. The next pointer event alone must
    // re-arm the scheduling predicate when it returns to output 1.
    backend.scene.scene_structure_dirty = false;
    assert!(!backend.for_tests_scene_wants_compose());
    Backend::warp_pointer_root(&mut backend, &mut state, 950, 350);
    assert_eq!(
        (backend.core.cursor_x, backend.core.cursor_y),
        (950.0, 350.0)
    );
    assert!(backend.for_tests_scene_wants_compose());
}

#[test]
// Mutation: make `output_commit_in_flight` always false in the backend sink.
fn cursor_only_commit_blocks_scene_readiness_until_its_flip_retires() {
    use crate::kms::render::scene::OutputSink;
    use yserver_core::server::ServerState;

    let Some(mut backend) = backend_with_live_scene() else {
        return;
    };
    let _recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    let mut state = ServerState::new();
    warp(&mut backend, &mut state, 230, 160);
    maybe_composite(&mut backend);

    assert!(OutputSink::output_commit_in_flight(&backend.platform, 0));
    assert!(!backend.scene.has_output_ready_for_submit(&backend.platform));
    retire_test_flip(&mut backend, &mut state, 1);
    assert!(!OutputSink::output_commit_in_flight(&backend.platform, 0));
    assert!(backend.scene.has_output_ready_for_submit(&backend.platform));
    assert_no_cursor_work(&backend, device_key);
}

#[test]
// Mutation: let pointer motion submit cursor-only immediately, bypassing scene priority.
fn scene_damage_after_cursor_retirement_submits_scene_with_latest_position() {
    use yserver_core::server::ServerState;

    let Some(mut backend) = backend_with_live_scene() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    let mut state = ServerState::new();

    warp(&mut backend, &mut state, 90, 70);
    maybe_composite(&mut backend);
    assert_eq!(recorder.records().len(), 1);
    Backend::mark_dirty(&mut backend);
    maybe_composite(&mut backend);
    assert_eq!(
        recorder.records().len(),
        1,
        "scene waits for cursor retirement"
    );

    retire_test_flip(&mut backend, &mut state, 1);
    warp(&mut backend, &mut state, 340, 210);
    let latest = backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0])[0];
    maybe_composite(&mut backend);
    assert_eq!(recorder.records().len(), 2, "scene submits through Backend");
    let records = recorder.records();
    assert_eq!(records[0].kind, AtomicCommitKind::CursorOnly);
    assert_eq!(records[1].kind, AtomicCommitKind::SceneWithCursor);
    assert_cursor_properties(&records[1], latest);
    retire_test_flip(&mut backend, &mut state, 2);
    assert_no_cursor_work(&backend, device_key);
}

#[test]
// Mutation: do not set `cursor_retry_waits_for_flip` after a cursor-only EBUSY.
fn cursor_only_ebusy_waits_for_the_next_flip_before_retrying() {
    use yserver_core::server::ServerState;

    let Some(mut backend) = backend_with_live_scene() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    let mut state = ServerState::new();
    recorder.script_result(Err(io::Error::from_raw_os_error(libc::EBUSY)));

    warp(&mut backend, &mut state, 180, 130);
    maybe_composite(&mut backend);
    let crtc = backend.platform.outputs[0].output.crtc;
    assert_eq!(recorder.records().len(), 1);
    assert_eq!(recorder.records()[0].kind, AtomicCommitKind::CursorOnly);
    assert_eq!(
        backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_ebusy_count(),
        1,
    );
    assert!(backend.platform.cursor_retry_waits_for_flip(0));
    assert!(
        backend
            .platform
            .for_tests_atomic_position_is_unsent(device_key, crtc)
    );
    assert!(
        !backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(device_key, crtc)
    );
    assert!(!backend.for_tests_scene_wants_compose());

    for _ in 0..3 {
        maybe_composite(&mut backend);
        assert_eq!(
            recorder.records().len(),
            1,
            "no cursor-only retry occurs before a flip event",
        );
        assert!(!backend.for_tests_scene_wants_compose());
    }

    warp(&mut backend, &mut state, 260, 170);
    let latest = backend
        .platform
        .for_tests_atomic_cursor_pending_position(device_key, crtc)
        .expect("new motion replaces the unsent position");
    assert!(backend.platform.cursor_retry_waits_for_flip(0));
    maybe_composite(&mut backend);
    assert_eq!(recorder.records().len(), 1);
    assert!(!backend.for_tests_scene_wants_compose());

    retire_test_flip(&mut backend, &mut state, 1);
    assert!(!backend.platform.cursor_retry_waits_for_flip(0));
    let records = recorder.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].kind, AtomicCommitKind::CursorOnly);
    assert_cursor_properties(&records[1], latest);
    assert!(
        backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(device_key, crtc)
    );
    retire_test_flip(&mut backend, &mut state, 2);
    assert_no_cursor_work(&backend, device_key);
}
