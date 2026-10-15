use super::*;

use crate::drm::atomic::{
    AtomicCommitKind, AtomicCommitSink, AtomicPropertyUpdate,
    tests_support::RecordingAtomicCommitExecutor,
};

fn recording_sink(
    backend: &mut KmsBackend,
    device_idx: usize,
) -> std::rc::Rc<RecordingAtomicCommitExecutor> {
    let recorder = std::rc::Rc::new(RecordingAtomicCommitExecutor::default());
    backend.platform.devices[device_idx].atomic_commit_sink =
        std::rc::Rc::new(AtomicCommitSink::new(recorder.clone()));
    recorder
}

fn raw<T: ::drm::control::ResourceHandle>(value: u32) -> T {
    ::drm::control::from_u32(value).expect("nonzero test handle")
}

fn set_scene_fence_properties(output: &mut crate::platform::drm::Output) {
    output.plane_in_fence_fd_prop = Some(raw(81));
    output.crtc_out_fence_ptr_prop = Some(raw(82));
}

fn set_unique_direct_plane_properties(output: &mut crate::platform::drm::Output) {
    output.plane_fb_id_prop = raw(71);
    output.plane_crtc_id_prop = raw(72);
    output.plane_src_x_prop = raw(73);
    output.plane_src_y_prop = raw(74);
    output.plane_src_w_prop = raw(75);
    output.plane_src_h_prop = raw(76);
    output.plane_crtc_x_prop = raw(77);
    output.plane_crtc_y_prop = raw(78);
    output.plane_crtc_w_prop = raw(79);
    output.plane_crtc_h_prop = raw(80);
}

fn update<H: ::drm::control::ResourceHandle>(
    object: H,
    property: ::drm::control::property::Handle,
    value: u64,
) -> AtomicPropertyUpdate {
    AtomicPropertyUpdate::new(object, property, value)
}

fn prepare_direct_present(
    backend: &mut KmsBackend,
    present_id: u64,
    width: u16,
    height: u16,
) -> (
    yserver_core::backend::PresentScanoutCandidate,
    yserver_core::backend::CompletedPresentEvent,
    crate::drm::async_flip::AsyncShape,
) {
    use crate::kms::render::store::{DrawableKind, Storage};
    use ash::vk;
    use yserver_core::backend::{CompletedPresentEvent, PresentScanoutCandidate, PresentWake};

    let target_xid = 0x7300 + u32::try_from(present_id).expect("test id fits") * 0x10;
    let source_xid = target_xid + 1;
    super::seed_window(backend, target_xid, None, 0, 0);
    let target = backend.windows.get_mut(&target_xid).expect("target window");
    target.width = width;
    target.height = height;
    backend.core.top_level_order.push(target_xid);
    let source_id = backend
        .store
        .allocate(
            source_xid,
            DrawableKind::Pixmap,
            24,
            false,
            Storage::for_tests_null(
                vk::Extent2D {
                    width: u32::from(width),
                    height: u32::from(height),
                },
                vk::Format::B8G8R8A8_UNORM,
            ),
        )
        .expect("allocate direct source");
    let mut crtcs: Vec<_> = backend
        .platform
        .outputs
        .iter()
        .map(|layout| layout.output.crtc)
        .collect();
    crtcs.sort_by_key(|crtc| u32::from(*crtc));
    let shape = crate::drm::async_flip::AsyncShape {
        fourcc: 0x3432_5258,
        modifier: 0,
        pitch: u32::from(width) * 4,
        width: u32::from(width),
        height: u32::from(height),
        crtcs,
        direct: true,
    };
    backend.scanout_m1.entries.insert(
        source_id,
        crate::kms::render::backend::ScanoutM1ProbeEntry::accepted_with_shape(
            crate::drm::modeset::DirectScanoutProbeFramebuffer::for_tests(raw(
                0x8100 + u32::try_from(present_id).expect("test id fits")
            )),
            shape.clone(),
        ),
    );

    let output = &backend.platform.outputs[0].output;
    let crtc_id = u32::from(output.crtc);
    let crtc_epoch = backend.present_crtc_clock_epoch(crtc_id);
    let candidate = PresentScanoutCandidate {
        client_id: 1,
        present_id,
        crtc_id,
        crtc_epoch,
        src_pixmap_xid: source_xid,
        dst_window_xid: target_xid,
        src_host_xid: source_xid,
        paint_dst_host_xid: target_xid,
        completion_dst_host_xid: target_xid,
        src_width: width,
        src_height: height,
        x_off: 0,
        y_off: 0,
        valid_region_xid: 0,
        update_region_xid: 0,
        update_is_full: true,
        explicit_sync: false,
        options: 0,
    };
    let event = CompletedPresentEvent {
        client_id: yserver_protocol::x11::ClientId(1),
        serial: u32::try_from(present_id).expect("test serial fits"),
        host_xid: source_xid,
        dst_host_xid: target_xid,
        options: 0,
        present_id,
        window_generation: 0,
        crtc_id,
        crtc_epoch,
        msc_offset: 0,
        completion_clock: None,
        wake: PresentWake::Pixmap { idle_fence_xid: 0 },
        completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
        emit_idle: true,
    };
    (candidate, event, shape)
}

fn retire_recorded_direct_flip(
    backend: &mut KmsBackend,
    state: &mut yserver_core::server::ServerState,
    frame: u32,
) {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    use yserver_core::backend::Backend;

    let source = backend.platform.devices[0]
        .test_page_flip_event_source
        .as_ref()
        .expect("recording page-flip source");
    for (index, output) in backend.platform.outputs.iter().enumerate() {
        source.queue_flip(output.output.crtc, frame + u32::try_from(index).unwrap());
    }
    let drm_fd = backend.platform.devices[0].device.as_fd().as_raw_fd();
    Backend::on_page_flip_ready(backend, state, drm_fd);
}

fn direct_backend(output_count: usize) -> KmsBackend {
    let mut backend = KmsBackend::for_tests_with_vk().expect("Vulkan test fixture");
    if output_count == 2 {
        backend.platform.fb_w = 1600;
        let mut output_b = clone_test_drm_output(&backend.platform.outputs[0].output);
        output_b.connector = raw(103);
        output_b.encoder = raw(104);
        output_b.crtc = raw(101);
        output_b.plane = raw(102);
        output_b.connector_name = "test-b".to_owned();
        let route = backend.platform.outputs[0].scanout_route;
        let active = crate::kms::backend::ActiveOutput::new(
            route,
            output_b,
            crate::drm::Swapchain::empty_for_tests(),
            800,
            0,
        );
        let key = active.key.clone();
        backend.platform.outputs.push(active);
        backend.platform.outputs[1].x = 800;
        backend.platform.fb_h = 600;
        backend.crtc_key_by_id.insert(101, key);
        backend.refresh_present_crtc_clock_epochs();
    } else {
        assert_eq!(output_count, 1);
        backend.platform.fb_w = 800;
        backend.platform.fb_h = 600;
    }
    for layout in &backend.platform.outputs {
        backend
            .crtc_key_by_id
            .insert(u32::from(layout.output.crtc), layout.key.clone());
    }
    backend.refresh_present_crtc_clock_epochs();
    let layouts = backend.platform.scene_output_layouts();
    backend.scene = crate::kms::render::scene::SceneCompositor::new(&backend.platform, &layouts)
        .expect("construct direct test scene");
    backend.scene.for_tests_set_hw_cursor_mode();
    backend.scene.scene_structure_dirty = false;
    backend.scanout_m2.cursor_bound_all = true;
    backend.scanout_m2.eligible_root_streak =
        crate::kms::render::backend::SCANOUT_M2_ELIGIBLE_ROOT_PROBATION - 1;
    backend.platform.requested_flip_timing = crate::drm::async_flip::FlipTiming::Async;
    backend
}

fn set_direct_second_crtc(backend: &mut KmsBackend, crtc_id: u32) {
    let previous = u32::from(backend.platform.outputs[1].output.crtc);
    let output_key = backend.platform.outputs[1].key.clone();
    backend.platform.outputs[1].output.crtc = raw(crtc_id);
    backend.crtc_key_by_id.remove(&previous);
    backend.crtc_key_by_id.insert(crtc_id, output_key);
    backend.refresh_present_crtc_clock_epochs();
}

fn try_direct_present(
    backend: &mut KmsBackend,
    present_id: u64,
    width: u16,
    height: u16,
) -> crate::drm::async_flip::AsyncShape {
    use yserver_core::backend::Backend;

    let (candidate, event, shape) = prepare_direct_present(backend, present_id, width, height);
    let submitted = Backend::try_present_direct(backend, candidate, event)
        .unwrap_or_else(|error| panic!("direct Present entry {present_id}: {error}"));
    assert!(
        submitted,
        "eligible direct Present {present_id} must submit"
    );
    shape
}

#[test]
// Mutations: keep cursor positions in the async builder, or keep OUT_FENCE_PTR.
fn async_direct_request_has_flag_no_out_fence_no_cursor() {
    use yserver_core::server::ServerState;

    let mut backend = direct_backend(1);
    let recorder = recording_sink(&mut backend, 0);
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
    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let shape = try_direct_present(&mut backend, 61, 800, 600);

    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, AtomicCommitKind::Async);
    assert!(
        records[0]
            .flags
            .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_ASYNC)
    );
    assert!(
        records[0]
            .flags
            .contains(::drm::control::AtomicCommitFlags::NONBLOCK)
    );
    let output = &backend.platform.outputs[0].output;
    assert!(!records[0].properties.iter().any(|update| {
        update.object == position.plane.into()
            || update.object == output.crtc.into() && update.property == raw(82)
    }));
    assert_eq!(
        backend
            .scanout_m2
            .pending
            .as_ref()
            .map(|frame| frame.async_outcome),
        Some(crate::drm::async_flip::AsyncOutcome::Accepted),
    );
    let mut state = ServerState::new();
    retire_recorded_direct_flip(&mut backend, &mut state, 1);
    assert!(backend.scanout_m2.pending.is_none());
    assert!(backend.scanout_m2.queued_successor.is_none());
    assert!(backend.scanout_m2.deferred_successor_skips.is_empty());
    assert!(
        backend
            .platform
            .async_ledger
            .allows(crate::drm::async_flip::AsyncStream::Direct, &shape)
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: keep cursor positions in the async builder, or keep OUT_FENCE_PTR.
fn async_scene_request_has_flag_no_out_fence_no_cursor() {
    let mut backend = live_scene_backend();
    backend.platform.requested_flip_timing = crate::drm::async_flip::FlipTiming::Async;
    let recorder = recording_sink(&mut backend, 0);
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
    damage_root_and_composite(&mut backend);

    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, AtomicCommitKind::Async);
    assert!(
        records[0]
            .flags
            .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_ASYNC)
    );
    let output = &backend.platform.outputs[0].output;
    assert!(!records[0].properties.iter().any(|update| {
        update.object == position.plane.into()
            || update.object == output.crtc.into() && update.property == raw(82)
    }));
    assert_eq!(
        backend
            .platform
            .async_outcomes
            .get(&crate::drm::async_flip::AsyncStream::Composed(output.crtc)),
        Some(&crate::drm::async_flip::AsyncOutcome::Accepted),
    );
    let mut state = yserver_core::server::ServerState::new();
    retire_composed_frame(&mut backend, &mut state, 1);
    assert!(!backend.scene.has_pending_page_flips());
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: add PAGE_FLIP_ASYNC unconditionally to either producer.
fn vsync_requests_unchanged() {
    use yserver_core::server::ServerState;

    let mut direct = direct_backend(1);
    direct.platform.requested_flip_timing = crate::drm::async_flip::FlipTiming::Vsync;
    let direct_recorder = recording_sink(&mut direct, 0);
    let direct_position = direct.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(51),
        raw(61),
        raw(62),
        123,
        45,
        3,
        4,
    );
    set_scene_fence_properties(&mut direct.platform.outputs[0].output);
    set_unique_direct_plane_properties(&mut direct.platform.outputs[0].output);
    let direct_shape = try_direct_present(&mut direct, 62, 800, 600);

    let direct_records = direct_recorder.records();
    assert_eq!(direct_records.len(), 1);
    let expected_flags = ::drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT
        | ::drm::control::AtomicCommitFlags::NONBLOCK;
    assert_eq!(direct_records[0].flags, expected_flags);
    assert_eq!(direct_records[0].kind, AtomicCommitKind::SceneWithCursor);
    let direct_output = &direct.platform.outputs[0].output;

    let expected_direct = vec![
        update(
            direct_output.plane,
            direct_output.plane_fb_id_prop,
            0x8100 + 62,
        ),
        update(
            direct_output.plane,
            direct_output.plane_crtc_id_prop,
            u64::from(u32::from(direct_output.crtc)),
        ),
        update(direct_output.plane, direct_output.plane_src_x_prop, 0),
        update(direct_output.plane, direct_output.plane_src_y_prop, 0),
        update(
            direct_output.plane,
            direct_output.plane_src_w_prop,
            800 << 16,
        ),
        update(
            direct_output.plane,
            direct_output.plane_src_h_prop,
            600 << 16,
        ),
        update(direct_output.plane, direct_output.plane_crtc_x_prop, 0),
        update(direct_output.plane, direct_output.plane_crtc_y_prop, 0),
        update(direct_output.plane, direct_output.plane_crtc_w_prop, 800),
        update(direct_output.plane, direct_output.plane_crtc_h_prop, 600),
        update(
            direct_position.plane,
            direct_position.crtc_x,
            i64::from(direct_position.x) as u64,
        ),
        update(
            direct_position.plane,
            direct_position.crtc_y,
            i64::from(direct_position.y) as u64,
        ),
    ];
    assert_eq!(direct_records[0].properties, expected_direct);
    let mut direct_state = ServerState::new();
    retire_recorded_direct_flip(&mut direct, &mut direct_state, 1);
    assert!(direct.scanout_m2.pending.is_none());
    assert!(
        direct
            .platform
            .async_ledger
            .allows(crate::drm::async_flip::AsyncStream::Direct, &direct_shape)
    );

    let mut scene = live_scene_backend();
    scene.platform.requested_flip_timing = crate::drm::async_flip::FlipTiming::Vsync;
    let scene_recorder = recording_sink(&mut scene, 0);
    let scene_position = scene.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(51),
        raw(61),
        raw(62),
        123,
        45,
        3,
        4,
    );
    damage_root_and_composite(&mut scene);
    let scene_records = scene_recorder.records();
    assert_eq!(scene_records.len(), 1);
    assert_eq!(scene_records[0].flags, expected_flags);
    assert_eq!(scene_records[0].kind, AtomicCommitKind::SceneWithCursor);
    let scene_output = &scene.platform.outputs[0].output;
    let scene_fb = scene_records[0].properties[0].value;
    let in_fence_fd = scene_records[0].properties[2].value;
    let out_fence_ptr = scene_records[0].properties[3].value;
    assert!(
        in_fence_fd == u64::MAX
            || in_fence_fd <= u64::try_from(i32::MAX).expect("i32 max is nonnegative")
    );
    assert_ne!(out_fence_ptr, 0);
    let pool_has_framebuffer = scene.platform.scanout_pools[0]
        .as_ref()
        .and_then(|pool| match pool {
            crate::kms::vk::scanout::OutputScanout::Shared(pool) => Some(pool),
            crate::kms::vk::scanout::OutputScanout::Copied(_) => None,
        })
        .is_some_and(|pool| {
            pool.bos.iter().any(|bo| {
                bo.fb_handle
                    .is_some_and(|fb| u64::from(u32::from(fb)) == scene_fb)
            })
        });
    assert!(pool_has_framebuffer, "scene flip uses a pool framebuffer");

    let expected_scene = vec![
        update(scene_output.plane, scene_output.plane_fb_id_prop, scene_fb),
        update(
            scene_output.plane,
            scene_output.plane_crtc_id_prop,
            u64::from(u32::from(scene_output.crtc)),
        ),
        update(scene_output.plane, raw(81), in_fence_fd),
        update(scene_output.crtc, raw(82), out_fence_ptr),
        update(
            scene_position.plane,
            scene_position.crtc_x,
            i64::from(scene_position.x) as u64,
        ),
        update(
            scene_position.plane,
            scene_position.crtc_y,
            i64::from(scene_position.y) as u64,
        ),
    ];
    assert_eq!(scene_records[0].properties, expected_scene);
    let mut scene_state = ServerState::new();
    retire_composed_frame(&mut scene, &mut scene_state, 2);
    assert!(!scene.scene.has_pending_page_flips());
}

#[test]
// Mutations: drop the same-frame Vsync resubmit or consume a cursor move on an
// accepted async flip.
fn async_einval_resubmits_same_frame_vsync() {
    use yserver_core::server::ServerState;

    let mut backend = direct_backend(1);
    let recorder = recording_sink(&mut backend, 0);
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
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Ok(()));
    recorder.script_result(Ok(()));
    let shape = try_direct_present(&mut backend, 31, 800, 600);

    assert_eq!(
        backend
            .scanout_m2
            .pending
            .as_ref()
            .map(|frame| frame.async_outcome),
        Some(crate::drm::async_flip::AsyncOutcome::FellBack { suspended: false }),
    );
    let first = recorder.records();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].kind, AtomicCommitKind::Async);
    assert_eq!(first[1].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(first[0].properties[0].value, first[1].properties[0].value);
    assert!(
        first[0]
            .properties
            .iter()
            .all(|update| update.object != position.plane.into())
    );
    assert!(first[1].properties.iter().any(|update| {
        update.object == position.plane.into()
            && update.property == position.crtc_x
            && update.value == i64::from(position.x) as u64
    }));
    assert!(
        backend
            .platform
            .async_ledger
            .allows(crate::drm::async_flip::AsyncStream::Direct, &shape,)
    );

    let mut state = ServerState::new();
    retire_recorded_direct_flip(&mut backend, &mut state, 1);
    let pending_cursor = backend.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(51),
        raw(61),
        raw(62),
        123,
        45,
        3,
        4,
    );
    let _next_shape = try_direct_present(&mut backend, 32, 800, 600);
    assert!(
        backend
            .platform
            .for_tests_atomic_position_is_unsent(pending_cursor.device_key, pending_cursor.crtc,)
    );
    let records = recorder.records();
    assert_eq!(records.len(), 3);
    assert_eq!(records[2].kind, AtomicCommitKind::Async);
    assert_eq!(
        backend
            .scanout_m2
            .pending
            .as_ref()
            .map(|frame| frame.async_outcome),
        Some(crate::drm::async_flip::AsyncOutcome::Accepted),
    );
    retire_recorded_direct_flip(&mut backend, &mut state, 2);
    assert!(backend.scanout_m2.pending.is_none());
    assert!(backend.scanout_m2.queued_successor.is_none());
    assert!(backend.scanout_m2.deferred_successor_skips.is_empty());
    assert_eq!(
        backend
            .platform
            .async_ledger
            .for_tests_consecutive_einval(crate::drm::async_flip::AsyncStream::Direct,),
        0
    );
}

#[test]
// Mutations: suspend after one EINVAL, or never suspend this grouped stream.
fn multi_crtc_async_einval_suspends_after_two() {
    use yserver_core::server::ServerState;

    let mut backend = direct_backend(2);
    let recorder = recording_sink(&mut backend, 0);
    for _ in 0..2 {
        recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
        recorder.script_result(Ok(()));
    }
    for _ in 0..3 {
        recorder.script_result(Ok(()));
    }
    let mut state = ServerState::new();

    let first_shape = try_direct_present(&mut backend, 41, 1600, 600);
    let mut records = recorder.records();
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [AtomicCommitKind::Async, AtomicCommitKind::Scene,]
    );
    assert_eq!(
        backend
            .scanout_m2
            .pending
            .as_ref()
            .map(|frame| frame.async_outcome),
        Some(crate::drm::async_flip::AsyncOutcome::FellBack { suspended: false })
    );
    assert_eq!(
        records[0].events.len(),
        2,
        "direct transaction groups both CRTCs"
    );
    assert_eq!(records[1].events.len(), 2);
    retire_recorded_direct_flip(&mut backend, &mut state, 10);

    let _same_shape = try_direct_present(&mut backend, 42, 1600, 600);
    records = recorder.records();
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
        ]
    );
    assert_eq!(
        backend
            .scanout_m2
            .pending
            .as_ref()
            .map(|frame| frame.async_outcome),
        Some(crate::drm::async_flip::AsyncOutcome::FellBack { suspended: true })
    );
    retire_recorded_direct_flip(&mut backend, &mut state, 20);

    for present_id in [43, 44] {
        let _same_shape = try_direct_present(&mut backend, present_id, 1600, 600);
        retire_recorded_direct_flip(&mut backend, &mut state, u32::try_from(present_id).unwrap());
    }
    records = recorder.records();
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Scene,
        ]
    );
    assert!(
        backend
            .platform
            .async_ledger
            .for_tests_consecutive_einval(crate::drm::async_flip::AsyncStream::Direct,)
            >= 2
    );

    backend.platform.outputs[1].output.crtc = raw(105);
    let output_key = backend.platform.outputs[1].key.clone();
    backend.crtc_key_by_id.remove(&101);
    backend.crtc_key_by_id.insert(105, output_key);
    backend.refresh_present_crtc_clock_epochs();
    let changed_shape = try_direct_present(&mut backend, 45, 1600, 600);
    assert_ne!(first_shape.crtcs, changed_shape.crtcs);
    records = recorder.records();
    assert_eq!(
        records.last().expect("shape-change commit").kind,
        AtomicCommitKind::Async
    );
    assert_eq!(
        backend
            .scanout_m2
            .pending
            .as_ref()
            .map(|frame| frame.async_outcome),
        Some(crate::drm::async_flip::AsyncOutcome::Accepted)
    );
    retire_recorded_direct_flip(&mut backend, &mut state, 50);
    assert!(backend.scanout_m2.pending.is_none());
    assert!(backend.scanout_m2.queued_successor.is_none());
    assert!(backend.scanout_m2.deferred_successor_skips.is_empty());
    assert_eq!(
        backend
            .platform
            .async_ledger
            .for_tests_consecutive_einval(crate::drm::async_flip::AsyncStream::Direct,),
        0
    );
    assert!(
        backend
            .platform
            .async_ledger
            .allows(crate::drm::async_flip::AsyncStream::Direct, &changed_shape,)
    );
}

#[test]
// Mutation: fail to make a changed shape current before its EBUSY submission.
fn direct_shape_change_on_ebusy_resets_consecutive_einval_history() {
    use yserver_core::{backend::Backend, server::ServerState};

    let mut backend = direct_backend(2);
    let recorder = recording_sink(&mut backend, 0);
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Ok(()));
    let stream = crate::drm::async_flip::AsyncStream::Direct;
    let mut state = ServerState::new();

    let (candidate_a, event_a, shape_a) = prepare_direct_present(&mut backend, 71, 1600, 600);
    assert_eq!(
        Backend::try_present_direct(&mut backend, candidate_a, event_a)
            .expect_err("first Vsync fallback remains EBUSY")
            .raw_os_error(),
        Some(libc::EBUSY),
    );
    assert!(backend.scanout_m2.pending.is_none());
    assert_eq!(
        backend
            .platform
            .async_ledger
            .for_tests_consecutive_einval(stream),
        1,
    );

    backend.scanout_m2.eligible_root_streak =
        crate::kms::render::backend::SCANOUT_M2_ELIGIBLE_ROOT_PROBATION - 1;
    set_direct_second_crtc(&mut backend, 105);
    let (candidate_b, event_b, shape_b) = prepare_direct_present(&mut backend, 72, 1600, 600);
    assert_ne!(shape_a, shape_b);
    assert_eq!(
        Backend::try_present_direct(&mut backend, candidate_b, event_b)
            .expect_err("changed-shape async EBUSY remains an error")
            .raw_os_error(),
        Some(libc::EBUSY),
    );
    assert_eq!(
        backend
            .platform
            .async_ledger
            .for_tests_consecutive_einval(stream),
        0,
        "submitting shape B resets A even when B returns EBUSY",
    );
    assert!(backend.scanout_m2.pending.is_none());
    assert!(backend.scanout_m2.queued_successor.is_none());
    assert!(backend.scanout_m2.deferred_successor_skips.is_empty());

    backend.scanout_m2.eligible_root_streak =
        crate::kms::render::backend::SCANOUT_M2_ELIGIBLE_ROOT_PROBATION - 1;
    set_direct_second_crtc(&mut backend, 101);
    let shape_a_again = try_direct_present(&mut backend, 73, 1600, 600);
    assert_eq!(shape_a, shape_a_again);
    assert_eq!(
        backend
            .scanout_m2
            .pending
            .as_ref()
            .map(|frame| frame.async_outcome),
        Some(crate::drm::async_flip::AsyncOutcome::FellBack { suspended: false }),
    );
    let records = recorder.records();
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
        ],
    );
    assert_eq!(
        records[1].properties[0].value,
        records[0].properties[0].value
    );
    assert_eq!(
        records[4].properties[0].value,
        records[3].properties[0].value
    );
    retire_recorded_direct_flip(&mut backend, &mut state, 2);
    assert!(backend.scanout_m2.pending.is_none());
    assert!(backend.scanout_m2.queued_successor.is_none());
    assert!(backend.scanout_m2.deferred_successor_skips.is_empty());
    assert_eq!(
        backend
            .platform
            .async_ledger
            .for_tests_consecutive_einval(stream),
        1,
    );
    assert!(backend.platform.async_ledger.allows(stream, &shape_a));
}

#[test]
// Mutation: treat EBUSY as EINVAL and resubmit the direct frame as Vsync.
fn async_ebusy_is_not_suspension() {
    use yserver_core::backend::Backend;

    let mut backend = direct_backend(1);
    let recorder = recording_sink(&mut backend, 0);
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    let (candidate, event, shape) = prepare_direct_present(&mut backend, 51, 800, 600);
    assert_eq!(
        Backend::try_present_direct(&mut backend, candidate, event)
            .expect_err("async EBUSY remains an error")
            .raw_os_error(),
        Some(libc::EBUSY),
    );
    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, AtomicCommitKind::Async);
    assert_eq!(
        backend
            .platform
            .async_ledger
            .for_tests_consecutive_einval(crate::drm::async_flip::AsyncStream::Direct,),
        0,
    );
    assert!(
        backend
            .platform
            .async_ledger
            .allows(crate::drm::async_flip::AsyncStream::Direct, &shape,)
    );
    assert!(backend.scanout_m2.pending.is_none());
    assert!(backend.scanout_m2.queued_successor.is_none());
    assert!(backend.scanout_m2.deferred_successor_skips.is_empty());
}

fn live_scene_backend() -> KmsBackend {
    live_scene_backend_with_outputs(1)
}

fn live_scene_backend_with_outputs(output_count: usize) -> KmsBackend {
    scene_backend_with_recording_pools(output_count)
        .unwrap_or_else(|error| panic!("recording scene fixture unavailable: {error}"))
}

fn scene_backend_with_recording_pools(output_count: usize) -> Result<KmsBackend, String> {
    let screen_width = if output_count == 2 { 1600 } else { 800 };
    let mut backend = KmsBackend::for_tests_with_vk_dimensions(screen_width, 600)
        .map_err(|error| format!("Vulkan fixture unavailable: {error}"))?;
    if output_count == 2 {
        super::push_test_output(&mut backend, 201);
        backend.platform.outputs[1].x = 800;
    } else {
        assert_eq!(output_count, 1);
    }
    let layouts = backend.platform.scene_output_layouts();
    backend.scene = crate::kms::render::scene::SceneCompositor::new(&backend.platform, &layouts)
        .map_err(|error| format!("live SceneCompositor unavailable: {error:?}"))?;
    backend
        .platform
        .scanout_pools
        .resize_with(output_count, || None);
    backend
        .platform
        .bo_generations
        .resize_with(output_count, Vec::new);

    let vk = std::sync::Arc::clone(
        backend
            .platform
            .gpu
            .vk
            .as_ref()
            .expect("Vk-backed test fixture has a context"),
    );
    for output_idx in 0..output_count {
        let width = u32::from(backend.platform.outputs[output_idx].width);
        let height = u32::from(backend.platform.outputs[output_idx].height);
        let route = backend.platform.outputs[output_idx].scanout_route;
        let key = backend.platform.outputs[output_idx].key.clone();
        let drm = std::rc::Rc::clone(
            &backend
                .platform
                .device_for_key(key.device_key)
                .expect("test output has a KMS device")
                .device,
        );
        let output = &mut backend.platform.outputs[output_idx].output;
        set_scene_fence_properties(output);
        set_unique_direct_plane_properties(output);

        let mut bos = Vec::with_capacity(2);
        for bo_idx in 0..2 {
            let xid = 0x6a00
                + u32::try_from(output_idx).expect("output index fits") * 0x10
                + u32::try_from(bo_idx).expect("BO index fits");
            let storage = backend
                .platform
                .gpu
                .allocate_drawable_storage(
                    u16::try_from(width).expect("test width fits"),
                    u16::try_from(height).expect("test height fits"),
                    32,
                )
                .map_err(|error| format!("test target image allocation failed: {error:?}"))?;
            let image = storage.image;
            let image_view = storage.image_view;
            backend
                .store
                .allocate(
                    xid,
                    crate::kms::render::store::DrawableKind::Pixmap,
                    32,
                    false,
                    storage,
                )
                .map_err(|error| format!("test target storage registration failed: {error:?}"))?;
            let mut bo = crate::kms::vk::scanout::ScanoutBo::for_tests_compose_target(
                std::sync::Arc::clone(&vk),
                std::rc::Rc::clone(&drm),
                raw(0x5200 + u32::try_from(output_idx * 2 + bo_idx).expect("test BO handle fits")),
                width,
                height,
                image,
                image_view,
            )
            .map_err(|error| format!("test compose BO allocation failed: {error}"))?;
            if bo_idx == 0 {
                bo.state.phase = crate::kms::vk::scanout::BoPhase::OnScreen;
            }
            bos.push(bo);
        }
        let pool = crate::kms::vk::scanout::ScanoutBoPool::for_tests_recording_pool(
            route, width, height, bos,
        );
        backend.platform.bo_generations[output_idx] =
            vec![crate::kms::render::platform::BoGenerationEntry::default(); 2];
        backend.platform.scanout_pools[output_idx] =
            Some(crate::kms::vk::scanout::OutputScanout::Shared(pool));
    }
    Ok(backend)
}

fn damage_root_and_composite(backend: &mut KmsBackend) {
    use yserver_core::backend::Backend;
    Backend::clear_area(
        backend,
        None,
        backend.core.window_id,
        0,
        None,
        0,
        0,
        backend.platform.fb_w,
        backend.platform.fb_h,
        (0, 0),
    )
    .expect("Backend::clear_area root damage");
    Backend::maybe_composite(backend).expect("Backend::maybe_composite");
}

fn retire_composed_frame(
    backend: &mut KmsBackend,
    state: &mut yserver_core::server::ServerState,
    frame: u32,
) {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    use yserver_core::backend::Backend;

    backend.platform.wait_idle_bounded();
    for output in &backend.platform.outputs {
        backend.platform.devices[0]
            .test_page_flip_event_source
            .as_ref()
            .expect("recording page-flip source")
            .queue_flip(output.output.crtc, frame);
    }
    let drm_fd = backend.platform.devices[0].device.as_fd().as_raw_fd();
    Backend::on_page_flip_ready(backend, state, drm_fd);
}

fn assert_scene_async_vsync_pair(
    records: &[crate::drm::atomic::tests_support::AtomicCommitRecord],
    start: usize,
) {
    assert_eq!(records[start].kind, AtomicCommitKind::Async);
    assert_eq!(records[start + 1].kind, AtomicCommitKind::Scene);
    assert_eq!(
        records[start].properties[0].value,
        records[start + 1].properties[0].value
    );
    assert!(
        records[start]
            .flags
            .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_ASYNC)
    );
    assert!(!records[start].properties.iter().any(|update| {
        update.object == records[start + 1].events[0].crtc.into() && update.property == raw(82)
    }));
    assert!(records[start + 1].properties.iter().any(|update| {
        update.object == records[start + 1].events[0].crtc.into() && update.property == raw(82)
    }));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: mark cursor positions submitted on an accepted async scene flip.
fn composed_async_accept_keeps_cursor_move_unsent() {
    use yserver_core::server::ServerState;

    let mut backend = live_scene_backend();
    backend.scene.for_tests_set_hw_cursor_mode();
    backend.platform.requested_flip_timing = crate::drm::async_flip::FlipTiming::Async;
    let recorder = recording_sink(&mut backend, 0);
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
    let stream = crate::drm::async_flip::AsyncStream::Composed(position.crtc);
    let mut state = ServerState::new();

    damage_root_and_composite(&mut backend);
    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, AtomicCommitKind::Async);
    assert!(
        !records[0]
            .properties
            .iter()
            .any(|update| update.object == position.plane.into())
    );
    assert!(
        backend
            .platform
            .for_tests_atomic_position_is_unsent(position.device_key, position.crtc,)
    );
    assert_eq!(
        backend.platform.async_outcomes.get(&stream),
        Some(&crate::drm::async_flip::AsyncOutcome::Accepted),
    );
    retire_composed_frame(&mut backend, &mut state, 1);

    backend.platform.requested_flip_timing = crate::drm::async_flip::FlipTiming::Vsync;
    damage_root_and_composite(&mut backend);
    let records = recorder.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].kind, AtomicCommitKind::SceneWithCursor);
    assert!(
        records[1]
            .properties
            .iter()
            .any(|update| update.object == position.plane.into())
    );
    assert!(
        !backend
            .platform
            .for_tests_atomic_position_is_unsent(position.device_key, position.crtc,)
    );
    retire_composed_frame(&mut backend, &mut state, 2);
    assert!(!backend.scene.has_pending_page_flips());
    assert!(
        backend
            .platform
            .async_ledger
            .allows(stream, &scene_shape(&backend, 0),)
    );
    assert_eq!(
        backend.platform.async_outcomes.get(&stream),
        Some(&crate::drm::async_flip::AsyncOutcome::NotRequested),
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: keep the async ledger out of PlatformBackend::finish_frame.
fn composed_async_einval_suspends_after_two() {
    use yserver_core::server::ServerState;

    let mut backend = live_scene_backend();
    backend.platform.requested_flip_timing = crate::drm::async_flip::FlipTiming::Async;
    let recorder = recording_sink(&mut backend, 0);
    for _ in 0..2 {
        recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
        recorder.script_result(Ok(()));
    }
    let output = &backend.platform.outputs[0].output;
    let stream = crate::drm::async_flip::AsyncStream::Composed(output.crtc);
    let mut state = ServerState::new();

    damage_root_and_composite(&mut backend);
    let records = recorder.records();
    assert_eq!(records.len(), 2);
    assert_scene_async_vsync_pair(&records, 0);
    assert_eq!(
        backend.platform.async_outcomes.get(&stream),
        Some(&crate::drm::async_flip::AsyncOutcome::FellBack { suspended: false }),
    );
    retire_composed_frame(&mut backend, &mut state, 1);

    damage_root_and_composite(&mut backend);
    let records = recorder.records();
    assert_eq!(records.len(), 4);
    assert_scene_async_vsync_pair(&records, 2);
    assert_eq!(
        records[0].properties[0].value,
        records[1].properties[0].value
    );
    assert_eq!(
        records[2].properties[0].value,
        records[3].properties[0].value
    );
    assert_eq!(
        backend.platform.async_outcomes.get(&stream),
        Some(&crate::drm::async_flip::AsyncOutcome::FellBack { suspended: true }),
    );
    retire_composed_frame(&mut backend, &mut state, 2);

    damage_root_and_composite(&mut backend);
    let records = recorder.records();
    assert_eq!(records.len(), 5);
    assert_eq!(records[4].kind, AtomicCommitKind::Scene);
    retire_composed_frame(&mut backend, &mut state, 3);
    assert!(!backend.scene.has_pending_page_flips());
    assert!(
        !backend
            .platform
            .async_ledger
            .allows(stream, &scene_shape(&backend, 0))
    );
    assert_eq!(
        backend.platform.async_outcomes.get(&stream),
        Some(&crate::drm::async_flip::AsyncOutcome::NotRequested),
    );
}

fn scene_shape(backend: &KmsBackend, output_idx: usize) -> crate::drm::async_flip::AsyncShape {
    let output = &backend.platform.outputs[output_idx].output;
    let Some(Some(crate::kms::vk::scanout::OutputScanout::Shared(pool))) =
        backend.platform.scanout_pools.get(output_idx)
    else {
        panic!("live-scene fixture has a shared scanout pool");
    };
    let bo = &pool.bos[0];
    let modifier = match pool.allocation_plan {
        crate::kms::vk::scanout::ScanoutAllocationPlan::GbmModifier(modifier)
        | crate::kms::vk::scanout::ScanoutAllocationPlan::DrmModifier(modifier) => modifier,
        crate::kms::vk::scanout::ScanoutAllocationPlan::PaddedExplicitLinear { .. }
        | crate::kms::vk::scanout::ScanoutAllocationPlan::ExplicitLinear
        | crate::kms::vk::scanout::ScanoutAllocationPlan::LegacyLinear => 0,
    };
    crate::drm::async_flip::AsyncShape {
        fourcc: 0x3432_5258,
        modifier,
        pitch: bo.pitch,
        width: bo.width,
        height: bo.height,
        crtcs: vec![output.crtc],
        direct: false,
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: store only one current shape across composed CRTC streams.
fn two_outputs_composed_both_rejecting_each_reach_suspension() {
    use yserver_core::server::ServerState;

    let mut backend = live_scene_backend_with_outputs(2);
    backend.platform.requested_flip_timing = crate::drm::async_flip::FlipTiming::Async;
    let recorder = recording_sink(&mut backend, 0);
    for _ in 0..4 {
        recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
        recorder.script_result(Ok(()));
    }
    recorder.script_result(Ok(()));
    recorder.script_result(Ok(()));
    let crtc_a = backend.platform.outputs[0].output.crtc;
    let crtc_b = backend.platform.outputs[1].output.crtc;
    let stream_a = crate::drm::async_flip::AsyncStream::Composed(crtc_a);
    let stream_b = crate::drm::async_flip::AsyncStream::Composed(crtc_b);
    let mut state = ServerState::new();

    for frame in 1..=2 {
        damage_root_and_composite(&mut backend);
        let records = recorder.records();
        let start = usize::try_from(frame - 1).unwrap() * 4;
        assert_eq!(records.len(), start + 4);
        assert_scene_async_vsync_pair(&records, start);
        assert_scene_async_vsync_pair(&records, start + 2);
        retire_composed_frame(&mut backend, &mut state, frame);
    }

    let shape_a = scene_shape(&backend, 0);
    let shape_b = scene_shape(&backend, 1);
    assert!(!backend.platform.async_ledger.allows(stream_a, &shape_a));
    assert!(!backend.platform.async_ledger.allows(stream_b, &shape_b));
    assert_eq!(
        backend
            .platform
            .async_ledger
            .for_tests_consecutive_einval(stream_a),
        2
    );
    assert_eq!(
        backend
            .platform
            .async_ledger
            .for_tests_consecutive_einval(stream_b),
        2
    );

    damage_root_and_composite(&mut backend);
    let records = recorder.records();
    assert_eq!(records.len(), 10);
    assert_eq!(records[8].kind, AtomicCommitKind::Scene);
    assert_eq!(records[9].kind, AtomicCommitKind::Scene);
    retire_composed_frame(&mut backend, &mut state, 3);

    let kinds: Vec<_> = recorder
        .records()
        .into_iter()
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Scene,
        ]
    );
    assert!(!backend.scene.has_pending_page_flips());
    assert_eq!(
        backend.platform.async_outcomes.get(&stream_a),
        Some(&crate::drm::async_flip::AsyncOutcome::NotRequested),
    );
    assert_eq!(
        backend.platform.async_outcomes.get(&stream_b),
        Some(&crate::drm::async_flip::AsyncOutcome::NotRequested),
    );
}

fn clone_test_drm_output(output: &crate::platform::drm::Output) -> crate::platform::drm::Output {
    crate::platform::drm::Output {
        connector: output.connector,
        connector_name: output.connector_name.clone(),
        encoder: output.encoder,
        crtc: output.crtc,
        plane: output.plane,
        mode: output.mode,
        picked: output.picked.clone(),
        plane_fb_id_prop: output.plane_fb_id_prop,
        plane_crtc_id_prop: output.plane_crtc_id_prop,
        plane_src_x_prop: output.plane_src_x_prop,
        plane_src_y_prop: output.plane_src_y_prop,
        plane_src_w_prop: output.plane_src_w_prop,
        plane_src_h_prop: output.plane_src_h_prop,
        plane_crtc_x_prop: output.plane_crtc_x_prop,
        plane_crtc_y_prop: output.plane_crtc_y_prop,
        plane_crtc_w_prop: output.plane_crtc_w_prop,
        plane_crtc_h_prop: output.plane_crtc_h_prop,
        plane_in_fence_fd_prop: output.plane_in_fence_fd_prop,
        crtc_out_fence_ptr_prop: output.crtc_out_fence_ptr_prop,
        scanout_modifiers: output.scanout_modifiers.clone(),
        mm_width: output.mm_width,
        mm_height: output.mm_height,
        edid: output.edid.clone(),
        connector_type: output.connector_type.clone(),
        modes: output.modes.clone(),
    }
}

fn pollable_test_device() -> (
    std::rc::Rc<crate::drm::Device>,
    std::os::unix::net::UnixStream,
) {
    let (device_fd, peer) = std::os::unix::net::UnixStream::pair().expect("test socket pair");
    let device = crate::drm::Device::from_inherited_kms_fd(
        std::os::fd::OwnedFd::from(device_fd),
        "<pollable test fd>",
    );
    (std::rc::Rc::new(device), peer)
}

fn query_capabilities_in_core(
    state: &mut yserver_core::server::ServerState,
    backend: &mut KmsBackend,
    targets: &[u32],
) -> Vec<u32> {
    use std::{
        collections::{HashMap, HashSet, VecDeque},
        io::Read,
        sync::{Arc, Mutex, atomic::AtomicU16},
        time::Duration,
    };
    use yserver_core::{
        core_loop::{
            CoreSender, Message, ResetPolicy, auth::AuthState, channel,
            poll_tokens::ClientIdAllocator, run_core,
        },
        server::ClientState,
        transport::Transport,
    };
    use yserver_protocol::x11::{
        ClientByteOrder, ClientId, RequestHeader, SequenceNumber, present as x11present,
    };

    let client_id = ClientId(1);
    let (transport, mut peer) = Transport::pair().expect("client socket pair");
    peer.set_read_timeout(Some(Duration::from_secs(3)))
        .expect("reply timeout");
    state.clients.insert(
        client_id.0,
        ClientState {
            writer: Arc::new(Mutex::new(transport)),
            byte_order: ClientByteOrder::LittleEndian,
            last_sequence: Arc::new(AtomicU16::new(0)),
            resource_id_base: 0,
            resource_id_mask: u32::MAX,
            event_masks: HashMap::new(),
            save_set: HashSet::new(),
            big_requests_enabled: false,
            xi2_masks: HashMap::new(),
            xi1_event_classes: HashSet::new(),
            xi1_window_event_classes: HashMap::new(),
            outbound: VecDeque::new(),
            watching_writable: false,
            write_failed: false,
            focused_window: yserver_core::resources::ROOT_WINDOW,
            reader_control: None,
            is_local: true,
            fd_passing: true,
        },
    );

    let (poll, sender, receiver) = channel().expect("core channel");
    let request_sender = sender.bind();
    let shutdown_sender = sender.clone_handle();
    let targets = targets.to_vec();
    struct ShutdownOnDrop(CoreSender);
    impl Drop for ShutdownOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(Message::Shutdown);
        }
    }
    let driver = std::thread::spawn(move || {
        let _shutdown_on_drop = ShutdownOnDrop(shutdown_sender);
        let mut replies = Vec::with_capacity(targets.len());
        for (index, target) in targets.into_iter().enumerate() {
            request_sender
                .send(Message::Request {
                    id: client_id,
                    sequence: SequenceNumber(u16::try_from(index + 1).expect("sequence fits")),
                    accepted_at: None,
                    header: RequestHeader {
                        opcode: 145,
                        data: x11present::QUERY_CAPABILITIES,
                        length_units: 2,
                    },
                    body: target.to_le_bytes().to_vec(),
                    attached_fd: None,
                })
                .expect("queue Present::QueryCapabilities");

            let mut reply = [0; 32];
            peer.read_exact(&mut reply)
                .expect("read Present::QueryCapabilities reply");
            assert_eq!(reply[0], 1, "reply marker");
            replies.push(u32::from_le_bytes(reply[8..12].try_into().unwrap()));
        }
        replies
    });

    run_core(
        poll,
        receiver,
        sender.clone_handle(),
        state,
        backend,
        [],
        &ClientIdAllocator::new(),
        AuthState::new(None),
        ResetPolicy::NoReset,
        None,
    )
    .expect("run core through Present::QueryCapabilities and shutdown");
    driver.join().expect("query driver thread")
}

#[test]
fn present_capabilities_follow_the_windows_output_device() {
    use std::rc::Rc;
    use yserver_core::{
        backend::Backend,
        resources::{ROOT_VISUAL, ROOT_WINDOW},
        server::{BackendCapabilities, ServerState},
    };
    use yserver_protocol::x11::{ClientId, CreateWindowRequest, ResourceId};

    // Mutations killed: route every CRTC through the primary KMS device, or
    // hard-code `async_flip: true`; the latter also makes target 0 advertise Async.
    let mut backend = KmsBackend::for_tests();
    let (device_a, _device_a_peer) = pollable_test_device();
    backend.platform.devices[0].device = device_a;
    let device_a_key = backend.platform.devices[0].key;
    let device_b_key = crate::platform::drm::DrmDeviceKey {
        major: 226,
        minor: 99,
    };
    let commits_a =
        Rc::new(crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor::default());
    backend.platform.devices[0].atomic_commit_sink =
        Rc::new(crate::drm::atomic::AtomicCommitSink::new(commits_a.clone()));
    let (device_b, _device_b_peer) = pollable_test_device();
    let commits_b =
        Rc::new(crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor::default());
    backend
        .platform
        .devices
        .push(crate::kms::render::platform::KmsDevice {
            key: device_b_key,
            device: device_b,
            atomic_commit_sink: Rc::new(crate::drm::atomic::AtomicCommitSink::new(
                commits_b.clone(),
            )),
            page_flip_event_source: Rc::new(
                crate::kms::render::platform::flip_events::DrmPageFlipEventSource,
            ),
            #[cfg(test)]
            test_page_flip_event_source: None,
            cursor: crate::kms::render::platform::KmsCursorState::new(),
            async_page_flip: false,
        });

    backend.platform.devices[0].async_page_flip = true;
    let mut output_b = clone_test_drm_output(&backend.platform.outputs[0].output);
    output_b.connector_name = "test-b".to_owned();
    let route_b = crate::kms::scanout_route::ScanoutRoute::new(
        crate::kms::render::platform::RenderDeviceId::UnverifiedFallback,
        device_b_key,
        crate::kms::scanout_route::RenderKmsRelationship::Unknown,
    );
    backend
        .platform
        .outputs
        .push(crate::kms::backend::ActiveOutput::new(
            route_b,
            output_b,
            crate::drm::Swapchain::empty_for_tests(),
            800,
            0,
        ));

    let (outputs, modes) = Backend::randr_outputs_and_modes(&mut backend);
    let (width, height) = Backend::fb_dimensions(&backend);
    let mut state = ServerState::with_randr_outputs_and_modes(
        width,
        height,
        outputs,
        modes,
        BackendCapabilities::from_backend(&backend),
    );
    let window_a = ResourceId(0x0100_0001);
    let window_b = ResourceId(0x0100_0002);
    for (window, x) in [(window_a, 10), (window_b, 810)] {
        state.resources.create_window(
            ClientId(1),
            CreateWindowRequest {
                depth: 24,
                window,
                parent: ROOT_WINDOW,
                x,
                y: 10,
                width: 100,
                height: 100,
                class: 1,
                visual: ROOT_VISUAL,
                ..Default::default()
            },
        );
    }
    let crtc_a = state.randr.outputs[0].crtc_id;
    let crtc_b = state.randr.outputs[1].crtc_id;
    assert_ne!(device_a_key, device_b_key);
    assert_ne!(crtc_a, crtc_b);

    let first = query_capabilities_in_core(
        &mut state,
        &mut backend,
        &[window_a.0, window_b.0, crtc_a, crtc_b, 0],
    );
    assert_eq!(
        first.iter().map(|caps| caps & 0x1 != 0).collect::<Vec<_>>(),
        [true, false, true, false, false],
        "windows resolve to their output device and CRTC targets resolve directly",
    );
    assert!(first.iter().all(|caps| caps & 0x8 == 0));

    backend.platform.devices[0].async_page_flip = false;
    backend.platform.devices[1].async_page_flip = true;
    let swapped = query_capabilities_in_core(
        &mut state,
        &mut backend,
        &[window_a.0, window_b.0, crtc_a, crtc_b, 0],
    );
    assert_eq!(
        swapped
            .iter()
            .map(|caps| caps & 0x1 != 0)
            .collect::<Vec<_>>(),
        [false, true, false, true, false],
        "advertised Async follows the owning device when flags swap",
    );
    assert!(swapped.iter().all(|caps| caps & 0x8 == 0));

    assert!(state.present_pending_msc.is_empty());
    assert!(state.present_window_msc.is_empty());
    assert!(backend.pending_present_batches.is_empty());
    assert!(backend.pending_present_source_waits.is_empty());
    assert!(backend.retained_present_wakes.is_empty());
    assert!(backend.pending_completed_events_on_shutdown.is_empty());
    assert_eq!(backend.vt_state, crate::vt::state::VtState::Active);
    assert!(commits_a.records().is_empty());
    assert!(commits_b.records().is_empty());
}
