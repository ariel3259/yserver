use super::support::AtomicCursorFixture;

pub(super) fn install_copied_recording_route(
    fixture: &mut AtomicCursorFixture,
) -> std::rc::Rc<crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor> {
    install_copied_recording_route_with_hotspot(fixture, false)
}

/// Copied recording route whose sink plane carries `HOTSPOT_X/Y` when `hotspot` is set.
pub(super) fn install_copied_recording_route_with_hotspot(
    fixture: &mut AtomicCursorFixture,
    hotspot: bool,
) -> std::rc::Rc<crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor> {
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let sink_key = crate::platform::drm::DrmDeviceKey {
        major: 226,
        minor: 99,
    };
    let sink_device = std::rc::Rc::new(crate::drm::Device::for_tests().expect("sink DRM device"));
    let sink_atomic = std::rc::Rc::new(
        crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor::default(),
    );
    let sink_executor: std::rc::Rc<dyn crate::drm::atomic::AtomicCommitExecutor> =
        sink_atomic.clone();
    let event_source = std::rc::Rc::clone(&fixture.recorders.page_flip_events);
    event_source.register_device(&sink_device);
    let source_device = &fixture.backend.platform.devices[0];
    let cursor_buffer_io = std::rc::Rc::clone(&source_device.cursor_buffer_io);
    fixture
        .backend
        .platform
        .devices
        .push(crate::kms::render::platform::KmsDevice {
            key: sink_key,
            device: std::rc::Rc::clone(&sink_device),
            async_page_flip: false,
            atomic_commit_sink: std::rc::Rc::new(crate::drm::atomic::AtomicCommitSink::new(
                sink_executor,
            )),
            page_flip_event_source: event_source.clone(),
            cursor_buffer_io,
            test_page_flip_event_source: Some(event_source),
            cursor: crate::kms::render::platform::KmsCursorState::new(),
        });
    let route = crate::kms::scanout_route::ScanoutRoute::new(
        fixture.backend.platform.outputs[0]
            .scanout_route
            .render_device_id,
        sink_key,
        crate::kms::scanout_route::RenderKmsRelationship::Different,
    );
    let output_keys = fixture
        .backend
        .platform
        .outputs
        .iter_mut()
        .map(|output| {
            output.key.device_key = sink_key;
            output.scanout_route = route;
            (output.output.crtc, output.key.clone())
        })
        .collect::<Vec<_>>();
    for (crtc, output_key) in output_keys {
        fixture
            .backend
            .pstate
            .crtc_key_by_id
            .insert(u32::from(crtc), output_key);
    }
    fixture.backend.refresh_present_crtc_clock_epochs();
    if hotspot {
        fixture
            .backend
            .platform
            .for_tests_initialize_cursor_planes_with_hotspot_properties(Some(64), Some(64));
    } else {
        fixture
            .backend
            .platform
            .for_tests_initialize_cursor_planes_with_capabilities(Some(64), Some(64));
    }
    fixture
        .install_copied_recording_route(route)
        .expect("install lavapipe copied route");
    let layouts = fixture.backend.platform.scene_output_layouts();
    fixture.backend.scene =
        crate::kms::render::scene::SceneCompositor::new(&fixture.backend.platform, &layouts)
            .expect("rebuild scene with copied sink identity");
    sink_atomic
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: publish a receipt when the copied handoff is unavailable; the empty receipt assertion fails.
fn atomic_cursor_only_unavailable_handoff_publishes_no_receipt() {
    use crate::kms::render::scene::{OutputRoute, OutputSink};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build unavailable cursor fixture")
    else {
        return;
    };
    let sink_atomic = install_copied_recording_route(&mut fixture);
    // Construction-time setup: retain destination slots but remove their
    // paired copied render sources. The Backend tick acquires a real slot and
    // reaches the sink's `FinishOutcome::Unavailable` check before recording.
    let Some(crate::kms::vk::scanout::OutputScanout::Copied(pool)) =
        fixture.backend.platform.scanout_pools[0].as_mut()
    else {
        panic!("copied route is installed before the scenario");
    };
    pool.sources.clear();
    assert_eq!(
        OutputSink::route(&fixture.backend.platform, 0),
        OutputRoute::Copied,
    );
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("unavailable handoff is recoverable");

    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None,
        "an unavailable frame publishes no cursor receipt",
    );
    assert!(
        !fixture.backend.scene.has_pending_page_flip(0),
        "an unavailable frame creates no presentation obligation",
    );
    assert!(
        fixture.backend.platform.staged_cursor_frames.is_empty(),
        "an unavailable frame stages no cursor metadata",
    );
    assert!(
        sink_atomic.records().is_empty(),
        "an unavailable copied handoff emits no sink commit",
    );
    fixture
        .backend
        .disable_output()
        .expect("disable and drain no-slot output through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit canceled-plan lease release; the prepared-user and final framebuffer ledger assertions fail.
fn atomic_cursor_only_cancelled_copied_frame_publishes_no_receipt() {
    use crate::kms::render::scene::{
        CursorFrameAssignment, CursorPlaneMode, OutputRoute, OutputSink,
    };
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build copied cancellation fixture")
    else {
        return;
    };
    let sink_atomic = install_copied_recording_route(&mut fixture);
    fixture
        .recorders
        .render_completion_readiness
        .hold_jobs_without_polling();
    assert_eq!(
        OutputSink::route(&fixture.backend.platform, 0),
        OutputRoute::Copied,
    );
    let cursor = create_solid_argb_cursor(&mut fixture, 32, [0, 0, 255, 255]);
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, cursor)
        .expect("define hardware cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    let accepted_mode_before = fixture.backend.scene.cursor_mode();
    assert_eq!(accepted_mode_before, CursorPlaneMode::Hidden);
    Backend::maybe_composite(&mut fixture.backend).expect("submit copied render");
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None
    );
    assert_eq!(fixture.backend.platform.staged_cursor_frames.len(), 1);
    let (&(output_idx, bo_idx), staged) = fixture
        .backend
        .platform
        .staged_cursor_frames
        .iter()
        .next()
        .expect("copied frame owns its staged sprite plan");
    assert_eq!(output_idx, 0);
    assert_eq!(staged.plan.assignment, CursorFrameAssignment::Hardware);
    assert!(staged.cursor_updates_reserved);
    assert_eq!(staged.cursor_updates.len(), 1);
    assert!(staged.cursor_updates[0].prepared.next.is_some());
    assert_eq!(
        cursor_plane_for_output(&fixture, 0).for_tests_prepared_sprite_user_count(),
        1,
        "the copied frame holds a prepared sprite lease until final submit or cancellation",
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        1,
        "the actual Vulkan render completion remains pending before cancellation",
    );
    assert_eq!(
        sink_atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [],
        "the copied frame has not reached KMS",
    );
    let job_id = fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_ids()[0];
    assert!(
        fixture
            .recorders
            .render_completion_readiness
            .captured_fd_exists(job_id),
        "the real Vulkan job owns a pollable completion fd before cancellation",
    );
    assert!(
        !fixture
            .recorders
            .render_completion_readiness
            .observed_ready_jobs()
            .contains(&job_id),
        "the production completion queue has not observed readiness before cancellation",
    );
    let old_frame = fixture.backend.platform.staged_cursor_frames[&(0, bo_idx)]
        .plan
        .frame;
    Backend::set_dpms_power(&mut fixture.backend, 3)
        .expect("Backend DPMS-off cancels the undelivered copied frame");
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None,
        "cancellation publishes no receipt",
    );
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        accepted_mode_before,
        "cancellation leaves the previously accepted cursor mode unchanged",
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0,
        "Backend DPMS-off unregisters and removes the canceled completion entry",
    );
    assert!(
        fixture.backend.platform.staged_cursor_frames.is_empty(),
        "Backend DPMS-off releases the staged frame metadata",
    );
    assert_eq!(
        cursor_plane_for_output(&fixture, 0).for_tests_prepared_sprite_user_count(),
        0,
        "cancellation releases the frame-owned lease exactly once",
    );
    assert!(
        !fixture.backend.scene.has_pending_page_flips(),
        "no scene event obligation survives cancellation and shutdown",
    );
    fixture.assert_cursor_quiescent();
    assert!(
        sink_atomic
            .records()
            .iter()
            .all(|record| record.events.is_empty()),
        "DPMS may detach the output, but the canceled copied frame emits no page-flip request",
    );
    let commits_at_cancel = sink_atomic.records().len();
    fixture
        .recorders
        .render_completion_readiness
        .wait_for_captured_fd_signal(job_id)
        .expect("the canceled job's original GPU completion fd eventually signals");
    assert!(
        fixture
            .recorders
            .render_completion_readiness
            .captured_fd_is_signaled(job_id)
            .expect("observe the old job's signaled completion fd"),
    );
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None,
        "delivery of the old already-signalled fd cannot publish a canceled frame",
    );
    assert_eq!(sink_atomic.records().len(), commits_at_cancel);
    assert_eq!(
        fixture.backend.platform.outputs.len(),
        1,
        "DPMS-off retains output A so it can be re-enabled through Backend",
    );
    fixture
        .recorders
        .render_completion_readiness
        .release_unpolled_jobs();
    fixture
        .recorders
        .render_completion_readiness
        .hold_ready_jobs();
    Backend::set_dpms_power(&mut fixture.backend, 0).expect("Backend DPMS-on re-enables output A");
    super::round_a::damage_cursor_root(&mut fixture);
    let output_crtc = fixture.backend.platform.outputs[0].output.crtc;
    let (new_job, _new_bo, new_frame) = wait_for_copied_job_for_crtc(&mut fixture, output_crtc);
    assert_ne!(new_job, job_id);
    assert_ne!(
        new_frame, old_frame,
        "reenabled output owns a new cursor frame id"
    );
    wait_for_held_scanout_job(&mut fixture, new_job);
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(new_job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let reenabled = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("reenabled output publishes only its new frame receipt");
    assert_eq!(reenabled.frame, Some(new_frame));
    assert_eq!(fixture.backend.platform.staged_cursor_frames.len(), 0);
    fixture.retire_output(0, 2);
    let before_shutdown = sink_atomic.records().len();
    fixture
        .backend
        .disable_output()
        .expect("Backend shutdown drains the reenabled output");
    assert!(
        sink_atomic.records()[before_shutdown..]
            .iter()
            .all(|record| record.events.is_empty())
    );
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: treat a copied sink-copy EIO as successful; the no-receipt assertion must fail.
fn atomic_cursor_only_sink_copy_failure_releases_frame_and_retries() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build copied failure fixture")
    else {
        return;
    };
    let sink_atomic = install_copied_recording_route(&mut fixture);
    fixture
        .recorders
        .render_completion_readiness
        .hold_ready_jobs();
    fixture.recorders.copied_scanout_copy.fail_next();

    let cursor = create_solid_argb_cursor(&mut fixture, 32, [0, 0, 255, 255]);
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, cursor)
        .expect("define hardware cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    let accepted_mode_before = fixture.backend.scene.cursor_mode();
    Backend::maybe_composite(&mut fixture.backend).expect("submit real copied rendering");
    let job = fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_ids()[0];
    let (_, _, _, failed_frame) = fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_associations()[0];
    wait_for_held_scanout_job(&mut fixture, job);
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None
    );
    assert_eq!(
        cursor_plane_for_output(&fixture, 0).for_tests_prepared_sprite_user_count(),
        1,
        "the staged frame owns its sprite until the copy dependency fails",
    );

    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    assert_eq!(fixture.recorders.copied_scanout_copy.attempts(), 1);
    assert_eq!(fixture.recorders.copied_scanout_copy.failures(), 1);
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None
    );
    assert_eq!(fixture.backend.scene.cursor_mode(), accepted_mode_before);
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0,
    );
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        cursor_plane_for_output(&fixture, 0).for_tests_prepared_sprite_user_count(),
        0,
        "the failed copied frame releases its reserved sprite lease",
    );
    assert!(
        sink_atomic.records().is_empty(),
        "copy failure precedes KMS"
    );

    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let (retry_job, _retry_bo, retry_frame) = wait_for_copied_job_for_crtc(&mut fixture, crtc);
    assert_ne!(retry_job, job);
    assert_ne!(retry_frame, failed_frame);
    wait_for_held_scanout_job(&mut fixture, retry_job);
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(retry_job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let retry_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("retry publishes a receipt after the copied KMS acceptance");
    assert_eq!(retry_receipt.frame, Some(retry_frame));
    assert_eq!(retry_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(
        retry_receipt.primary_cursor,
        CursorPrimaryContent::Cursorless
    );
    assert_eq!(fixture.recorders.copied_scanout_copy.attempts(), 2);
    assert_eq!(fixture.recorders.copied_scanout_copy.failures(), 1);
    let records = sink_atomic.records();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor
    );
    assert_eq!(records[0].events.len(), 1);
    fixture.retire_output(0, 1);
    fixture
        .backend
        .disable_output()
        .expect("Backend disables and drains the retried output");
    fixture.assert_cursor_quiescent();
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: turn a failed copied sink submit into an accepted receipt; the no-receipt and retry assertions fail.
fn atomic_cursor_only_copied_submit_failure_releases_frame_and_retries() {
    use crate::{
        drm::atomic::tests_support::ScriptedResult,
        kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent},
    };
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build copied retry fixture") else {
        return;
    };
    let sink_atomic = install_copied_recording_route(&mut fixture);
    fixture
        .recorders
        .render_completion_readiness
        .hold_ready_jobs();
    let root = fixture.backend.core.window_id;
    let cursor = create_solid_argb_cursor(&mut fixture, 32, [0, 0, 255, 255]);
    Backend::define_cursor(&mut fixture.backend, None, root, cursor)
        .expect("define hardware cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("build copied frame through Backend");
    let job = fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_ids()[0];
    let association = fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_associations()[0];
    let failed_frame = association.3;
    wait_for_held_scanout_job(&mut fixture, job);
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None
    );

    sink_atomic.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None
    );
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0
    );
    assert_eq!(
        cursor_plane_for_output(&fixture, 0).for_tests_prepared_sprite_user_count(),
        0,
        "failed copied submit releases the staged hardware lease",
    );
    let failed = sink_atomic.records();
    assert_eq!(
        failed.len(),
        2,
        "one failed page-flip attempt and its TEST_ONLY cursor check are recorded: {failed:?}"
    );
    assert_ne!(failed[0].result, ScriptedResult::Ok);
    assert_eq!(
        failed[1].flags,
        ::drm::control::AtomicCommitFlags::TEST_ONLY
    );
    assert!(failed[1].events.is_empty());
    assert!(
        failed
            .iter()
            .all(|record| { record.kind != crate::drm::atomic::AtomicCommitKind::CursorOnly })
    );

    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let (retry_job, _retry_bo, retry_frame) = wait_for_copied_job_for_crtc(&mut fixture, crtc);
    assert_ne!(retry_frame, failed_frame);
    wait_for_held_scanout_job(&mut fixture, retry_job);
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(retry_job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("retry publishes after its successful atomic acceptance");
    assert_eq!(receipt.frame, Some(retry_frame));
    assert_eq!(receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(receipt.primary_cursor, CursorPrimaryContent::Cursorless);
    let records = sink_atomic.records();
    assert_eq!(records.len(), 3);
    assert_eq!(
        records[0].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor
    );
    assert_eq!(
        records[1].flags,
        ::drm::control::AtomicCommitFlags::TEST_ONLY
    );
    assert_eq!(
        records[2].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor
    );
    assert_eq!(records[2].result, ScriptedResult::Ok);
    assert_eq!(records[2].events.len(), 1);
    fixture.retire_output(0, 1);
    fixture
        .backend
        .disable_output()
        .expect("Backend shuts down after copied retry retires");
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0
    );
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: reuse F0's cached frame id/content receipt for F1; the exact F1 assertions fail.
fn atomic_cursor_only_frame_receipts_name_the_accepted_frame() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorFrameId, CursorPrimaryContent};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new_with_cursor_capabilities(None, None)
        .expect("build software cursor scene fixture")
    else {
        return;
    };
    {
        let output = &mut fixture.backend.platform.outputs[0].output;
        output.plane_fb_id_prop = ::drm::control::from_u32(71).expect("FB_ID property");
        output.plane_crtc_id_prop = ::drm::control::from_u32(72).expect("CRTC_ID property");
        output.plane_src_x_prop = ::drm::control::from_u32(73).expect("SRC_X property");
        output.plane_src_y_prop = ::drm::control::from_u32(74).expect("SRC_Y property");
        output.plane_src_w_prop = ::drm::control::from_u32(75).expect("SRC_W property");
        output.plane_src_h_prop = ::drm::control::from_u32(76).expect("SRC_H property");
        output.plane_crtc_x_prop = ::drm::control::from_u32(77).expect("CRTC_X property");
        output.plane_crtc_y_prop = ::drm::control::from_u32(78).expect("CRTC_Y property");
        output.plane_crtc_w_prop = ::drm::control::from_u32(79).expect("CRTC_W property");
        output.plane_crtc_h_prop = ::drm::control::from_u32(80).expect("CRTC_H property");
    }
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let setup = super::round_a::setup_cursor_only_direct_present(&mut fixture);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("accept a cursorless retained primary before direct admission");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    fixture.retire_output(0, 29);
    let baseline_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted cursorless primary receipt");
    assert_eq!(
        baseline_receipt.primary_cursor,
        CursorPrimaryContent::Cursorless
    );
    assert!(fixture.backend.scene.direct_scanout_primary_ready());
    let (candidate, event) = super::round_a::direct_present_for_id(&setup, 20_900);
    let before_test_only = fixture.recorders.atomic.records().len();
    Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
    assert!(
        !Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("TEST_ONLY probe remains a non-presenting Backend operation"),
        "the first direct candidate performs only M1 TEST_ONLY validation"
    );
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(baseline_receipt),
        "TEST_ONLY validation publishes no accepted cursor receipt"
    );
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        before_test_only,
        "TEST_ONLY emits no live atomic commit or event obligation"
    );
    let source_id = fixture
        .backend
        .store
        .lookup(setup.source_xid)
        .expect("direct source was created through Backend");
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m1
            .entries
            .get(&source_id)
            .and_then(|entry| entry.framebuffer())
            .is_some(),
        "the injected TEST_ONLY executor accepted M1"
    );
    assert!(!fixture.backend.pstate.scanout_m2.active());

    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(baseline_receipt),
        "no cursor receipt exists before the first accepted output frame"
    );

    let root = fixture.backend.core.window_id;
    let first_cursor = create_oversized_cursor(&mut fixture, (2, 3));
    Backend::define_cursor(&mut fixture.backend, None, root, first_cursor)
        .expect("define F0 cursor");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit F0 through Backend");

    let f0 = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted F0 cursor receipt");
    assert_eq!(f0.frame, Some(CursorFrameId(2)));
    assert_eq!(f0.mode, CursorAcceptedMode::Software);
    let CursorPrimaryContent::Software {
        pos: f0_pos,
        record_version: f0_version,
    } = f0.primary_cursor
    else {
        panic!("F0 primary contains its software cursor");
    };
    assert_eq!(f0_pos, (98, 87));
    assert_ne!(f0_version, 0);
    fixture.retire_output(0, 1);
    let f0_fb = u32::from(
        fixture
            .backend
            .platform
            .retained_composed_framebuffer(0)
            .expect("F0 is the retained primary framebuffer"),
    );

    let failed_cursor = create_oversized_cursor(&mut fixture, (3, 4));
    Backend::define_cursor(&mut fixture.backend, None, root, failed_cursor)
        .expect("define rejected cursor frame");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 115, 95);
    super::round_a::damage_cursor_root(&mut fixture);
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(5)));
    Backend::maybe_composite(&mut fixture.backend).expect("failed atomic handoff is recoverable");
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(f0),
        "a rejected handoff publishes no receipt or mode change"
    );
    assert!(
        !fixture.backend.scene.has_pending_page_flip(0),
        "a rejected handoff creates no presentation obligation"
    );
    assert!(
        fixture.backend.platform.staged_cursor_frames.is_empty(),
        "a rejected handoff releases its staged frame plan"
    );

    let cursor = create_oversized_cursor(&mut fixture, (4, 5));
    Backend::define_cursor(&mut fixture.backend, None, root, cursor).expect("define F1 cursor");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 130, 100);
    super::round_a::damage_cursor_root(&mut fixture);
    let retry_deadline = fixture
        .backend
        .scene
        .earliest_retry_deadline()
        .expect("rejected frame schedules the production retry deadline");
    while std::time::Instant::now() < retry_deadline {
        std::hint::spin_loop();
    }
    Backend::maybe_composite(&mut fixture.backend).expect("submit F1 through Backend");

    let f1 = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted F1 cursor receipt");
    assert_eq!(f1.frame, Some(CursorFrameId(4)));
    assert_eq!(f1.mode, CursorAcceptedMode::Software);
    let CursorPrimaryContent::Software {
        pos: f1_pos,
        record_version: f1_version,
    } = f1.primary_cursor
    else {
        panic!("F1 primary contains its software cursor");
    };
    assert_eq!(f1_pos, (126, 95));
    assert_ne!(f1_version, f0_version, "F1 records the replacement sprite");
    fixture.retire_output(0, 2);
    let f1_fb = u32::from(
        fixture
            .backend
            .platform
            .retained_composed_framebuffer(0)
            .expect("F1 is the retained primary framebuffer"),
    );

    let primary_fb_property = fixture.backend.platform.outputs[0].output.plane_fb_id_prop;
    let primary_plane = fixture.backend.platform.outputs[0].output.plane;
    let records = fixture.recorders.atomic.records();
    let frame_records: Vec<_> = records
        .iter()
        .filter(|record| record.events.len() == 1)
        .collect();
    assert_eq!(
        frame_records.len(),
        4,
        "the retained baseline, F0, the rejection, and F1 have flip attempts"
    );
    let frame_fb = |index: usize| {
        frame_records[index]
            .properties
            .iter()
            .find(|update| {
                update.object == primary_plane.into() && update.property == primary_fb_property
            })
            .map(|update| update.value)
            .expect("the accepted frame records its primary framebuffer")
    };
    assert_eq!(
        frame_fb(1),
        u64::from(f0_fb),
        "F0 receipt accompanies its rendered FB: {frame_records:?}"
    );
    assert_eq!(
        frame_fb(3),
        u64::from(f1_fb),
        "F1 receipt accompanies its rendered FB: {frame_records:?}"
    );
    assert_eq!(
        frame_records[1].result,
        crate::drm::atomic::tests_support::ScriptedResult::Ok
    );
    assert!(matches!(
        frame_records[2].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            raw_os_error: Some(5),
            ..
        }
    ));
    assert_eq!(
        frame_records[3].result,
        crate::drm::atomic::tests_support::ScriptedResult::Ok
    );

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("hide cursor through Backend");
    fixture.retire_output(0, 3);
    fixture
        .backend
        .disable_output()
        .expect("disable and drain output through Backend");
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: retain the two-phase cursorless HW→SW frame; the SW-primary plus detach assertion fails.
// Mutation: apply the requested mode before frame acceptance; the pre-accept mode assertion fails.
fn atomic_cursor_only_shared_transition_is_one_primary_commit() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPlaneMode, CursorPrimaryContent};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build shared transition fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let root = fixture.backend.core.window_id;
    let oversized = create_oversized_cursor(&mut fixture, (2, 3));
    Backend::define_cursor(&mut fixture.backend, None, root, oversized)
        .expect("establish software mode through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("accept initial software frame");
    let software = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("software frame receipt");
    assert_eq!(software.mode, CursorAcceptedMode::Software);
    assert!(matches!(
        software.primary_cursor,
        CursorPrimaryContent::Software { .. }
    ));
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        CursorPlaneMode::Mixed,
        "the accepted software receipt remains in C1 probation until its flip retires",
    );
    assert_eq!(
        fixture.recorders.atomic.records()[0].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor
    );
    fixture.retire_output(0, 1);
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Sw);

    // Software → hardware: the cursorless primary and attach share one
    // accepted request. The scene receipt remains frame-scoped.
    fixture.define_visible_cursor();
    super::round_a::damage_cursor_root(&mut fixture);
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Sw);
    Backend::maybe_composite(&mut fixture.backend).expect("accept shared SW→HW frame");
    let hardware = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted hardware frame receipt");
    assert_eq!(hardware.mode, CursorAcceptedMode::Hardware);
    assert_eq!(hardware.primary_cursor, CursorPrimaryContent::Cursorless);
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Mixed);
    assert_eq!(
        fixture.recorders.atomic.records()[1].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        "the cursor attach rides the primary frame",
    );
    assert_eq!(fixture.recorders.atomic.records()[1].events.len(), 1);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    fixture.retire_output(0, 2);
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Hw);

    // Hardware → software: the accepted primary contains SW pixels while
    // the same request detaches the old HW binding.
    let software_again = create_oversized_cursor(&mut fixture, (3, 4));
    Backend::define_cursor(&mut fixture.backend, None, root, software_again)
        .expect("request software mode through Backend");
    super::round_a::damage_cursor_root(&mut fixture);
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(hardware),
        "the last accepted output mode remains HW until the shared frame is accepted",
    );
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    Backend::maybe_composite(&mut fixture.backend).expect("accept shared HW→SW frame");
    let software_again_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted software handoff receipt");
    assert_eq!(software_again_receipt.mode, CursorAcceptedMode::Software);
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Mixed);
    assert!(matches!(
        software_again_receipt.primary_cursor,
        CursorPrimaryContent::Software { .. }
    ));
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Mixed);
    assert_eq!(
        fixture.recorders.atomic.records()[2].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        "the SW pixels and plane detach share one accepted request",
    );
    let output = &fixture.backend.platform.outputs[0];
    let crtc = output.output.crtc;
    let device = fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .expect("transition output device");
    let plane_properties = device
        .cursor
        .plane
        .as_ref()
        .and_then(|plane| plane.properties_for(crtc))
        .expect("eligible cursor properties");
    let handoff = &fixture.recorders.atomic.records()[2];
    for property in [plane_properties.fb_id, plane_properties.crtc_id] {
        assert!(handoff.properties.iter().any(|update| {
            update.object == plane_properties.plane.into()
                && update.property == property
                && update.value == 0
        }));
    }
    assert!(
        handoff
            .properties
            .iter()
            .any(|update| update.object == output.output.plane.into()
                && update.property == output.output.plane_fb_id_prop),
        "the same request carries its primary framebuffer",
    );
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    fixture.retire_output(0, 3);
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Sw);

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("accept hidden software frame");
    let hidden = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("hidden frame receipt");
    assert_eq!(hidden.mode, CursorAcceptedMode::Hidden);
    assert_eq!(hidden.primary_cursor, CursorPrimaryContent::Cursorless);
    fixture.retire_output(0, 4);
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "accepted transition sequence is SW, attach, SW+detach, hidden",
    );
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    fixture
        .backend
        .disable_output()
        .expect("disable and drain through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: apply Hardware mode when scanout-slot acquisition returns no BO; the Hidden-mode and unchanged-receipt assertions fail.
fn atomic_cursor_only_no_bo_does_not_publish_assignment() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPlaneMode};
    use yserver_core::backend::Backend;

    let Some(mut fixture) =
        AtomicCursorFixture::new_with_scanout_acquisition_script([true, true, false])
            .expect("build no-BO transition fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    // The acquisition schedule is part of fixture construction, before the
    // scenario starts. Backend reaches the real acquire_scanout_bo entry and
    // sees two unavailable acquisitions before the fixture permits a retry.
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 140, 110);
    super::round_a::damage_cursor_root(&mut fixture);
    let receipt_before = fixture.backend.scene.for_tests_cursor_commit_receipt(0);
    Backend::maybe_composite(&mut fixture.backend).expect("unavailable slot is a clean skip");
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        receipt_before
    );
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Hidden);
    assert!(fixture.recorders.atomic.records().is_empty());
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert!(!fixture.backend.scene.has_pending_page_flips());
    let (device_key, crtc) = {
        let output = &fixture.backend.platform.outputs[0];
        (output.key.device_key, output.output.crtc)
    };
    let cursor_state = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.crtc_states.get(&crtc));
    assert!(cursor_state.is_none_or(|state| state.intent.is_none()));

    // An idle scheduler pass while the real slot is unavailable must not
    // attach the unsubmitted scene assignment through cursor-only work.
    Backend::maybe_composite(&mut fixture.backend).expect("repeat idle no-slot pass");
    assert!(fixture.recorders.atomic.records().is_empty());
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        receipt_before
    );

    Backend::maybe_composite(&mut fixture.backend).expect("available slot accepts the scene plan");
    let accepted = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("available retry receipt");
    assert_eq!(accepted.mode, CursorAcceptedMode::Hardware);
    assert_eq!(
        fixture.recorders.atomic.records()[0].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
    );
    fixture.retire_output(0, 1);
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("hide accepted cursor through scene");
    fixture.retire_output(0, 2);
    fixture
        .backend
        .disable_output()
        .expect("disable and drain through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: transfer a rejected frame's Scene plan to cursor-only intent; the unchanged receipt and empty idle trace fail.
fn atomic_cursor_only_failed_frame_cannot_be_stolen_by_cursor_only() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPlaneMode};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build rejected transition fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let root = fixture.backend.core.window_id;
    let software_cursor = create_oversized_cursor(&mut fixture, (2, 3));
    Backend::define_cursor(&mut fixture.backend, None, root, software_cursor)
        .expect("establish software cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("accept software baseline");
    let baseline = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("software baseline receipt");
    assert_eq!(baseline.mode, CursorAcceptedMode::Software);
    fixture.retire_output(0, 1);

    fixture.define_visible_cursor();
    super::round_a::damage_cursor_root(&mut fixture);
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(5)));
    Backend::maybe_composite(&mut fixture.backend).expect("rejected handoff is recoverable");
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(baseline)
    );
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Sw);
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    let output = &fixture.backend.platform.outputs[0];
    let device = fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .expect("cursor output device");
    let state = device.cursor.crtc_states.get(&output.output.crtc);
    assert!(state.is_none_or(|state| {
        state.applied.is_none() && state.intent.is_none() && state.in_flight.is_none()
    }));

    let records_after_reject = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend).expect("idle scheduling before retry");
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        records_after_reject
    );
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(baseline)
    );
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Sw);
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());

    let retry_deadline = fixture
        .backend
        .scene
        .earliest_retry_deadline()
        .expect("rejected frame schedules production retry");
    while std::time::Instant::now() < retry_deadline {
        std::hint::spin_loop();
    }
    Backend::maybe_composite(&mut fixture.backend).expect("retry accepted handoff through Backend");
    assert_eq!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted retry receipt")
            .mode,
        CursorAcceptedMode::Hardware,
    );
    fixture.retire_output(0, 2);
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("hide accepted retry cursor");
    fixture.retire_output(0, 3);
    fixture
        .backend
        .disable_output()
        .expect("disable and drain through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: observe pointer updates only in HW/Mixed mode; the first-attach P1 and hidden-show P2 coordinates fail.
fn atomic_cursor_only_first_hw_attach_samples_motion_while_software() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build first-attach fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let root = fixture.backend.core.window_id;
    let oversized = create_oversized_cursor(&mut fixture, (1, 2));
    Backend::define_cursor(&mut fixture.backend, None, root, oversized)
        .expect("establish software mode");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 90, 80);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("accept software baseline");
    assert_eq!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("software baseline")
            .mode,
        CursorAcceptedMode::Software,
    );
    fixture.retire_output(0, 1);

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 230, 170);
    fixture.define_visible_cursor();
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("attach first HW sprite at P1");
    let first = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("first hardware attach receipt");
    assert_eq!(first.mode, CursorAcceptedMode::Hardware);
    assert_eq!(first.primary_cursor, CursorPrimaryContent::Cursorless);
    assert_eq!(
        fixture.recorders.atomic.records()[1].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor
    );
    let (output_x, output_y, device_key, crtc) = {
        let output = &fixture.backend.platform.outputs[0];
        (
            output.x,
            output.y,
            output.key.device_key,
            output.output.crtc,
        )
    };
    let device = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .expect("cursor output device");
    let binding = device
        .cursor
        .crtc_states
        .get(&crtc)
        .and_then(|state| state.applied)
        .expect("accepted attach binding");
    assert_eq!(binding.x, 230 - output_x - i32::from(binding.hot_x));
    assert_eq!(binding.y, 170 - output_y - i32::from(binding.hot_y));
    fixture.retire_output(0, 2);

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("detach cursor while hidden");
    fixture.retire_output(0, 3);
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 345, 215);
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("show cursor at hidden P2 motion");
    let shown = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("hidden motion show receipt");
    assert_eq!(shown.mode, CursorAcceptedMode::Hardware);
    let binding = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.crtc_states.get(&crtc))
        .and_then(|state| state.applied)
        .expect("accepted reattach binding");
    assert_eq!(binding.x, 345 - output_x - i32::from(binding.hot_x));
    assert_eq!(binding.y, 215 - output_y - i32::from(binding.hot_y));
    fixture.retire_output(0, 4);

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("final cursor detach");
    fixture.retire_output(0, 5);
    fixture
        .backend
        .disable_output()
        .expect("disable and drain through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

fn create_oversized_cursor(fixture: &mut AtomicCursorFixture, hot: (u16, u16)) -> u32 {
    use yserver_core::backend::Backend;

    let source = Backend::create_pixmap(&mut fixture.backend, None, 1, 65, 65)
        .expect("create oversized cursor source");
    Backend::create_cursor(
        &mut fixture.backend,
        None,
        source,
        None,
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
        hot.0,
        hot.1,
    )
    .expect("create oversized software cursor")
    .as_raw()
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit Shared submit-failure reservation rollback; the prepared lease remains owned after the rejected frame.
fn atomic_cursor_only_shared_submit_failure_releases_reserved_lease_and_retries() {
    use crate::kms::render::scene::CursorAcceptedMode;
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build Shared rollback fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 140, 110);
    super::round_a::damage_cursor_root(&mut fixture);

    // The Shared producer reserves the cursor lease before reaching the
    // recording atomic sink. Failing the sink therefore exercises rollback
    // after a real successful reservation.
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    Backend::maybe_composite(&mut fixture.backend).expect("failed Shared submit is recoverable");
    let failed_records = fixture.recorders.atomic.records();
    let failed_frames: Vec<_> = failed_records
        .iter()
        .filter(|record| !record.events.is_empty())
        .collect();
    assert_eq!(
        failed_frames.len(),
        1,
        "the Shared frame reached atomic submit"
    );
    assert_eq!(
        failed_frames[0].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        "the rejected Shared request carries its cursor delta",
    );
    assert!(matches!(
        failed_frames[0].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            raw_os_error: Some(libc::EIO),
            ..
        }
    ));
    let output = &fixture.backend.platform.outputs[0];
    let plane_properties = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .expect("cursor plane")
        .properties_for(output.output.crtc)
        .expect("cursor plane properties");
    assert!(
        failed_frames[0]
            .properties
            .iter()
            .any(|update| update.object == plane_properties.plane.into())
    );
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None,
        "a failed Shared frame publishes no cursor receipt",
    );
    assert!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("cursor plane")
            .sprite_pool_users_are_zero(),
        "a failed reservation leaves no prepared or accepted lease",
    );

    let retry_deadline = fixture
        .backend
        .scene
        .earliest_retry_deadline()
        .expect("failed Shared frame schedules its production retry");
    while std::time::Instant::now() < retry_deadline {
        std::hint::spin_loop();
    }
    Backend::maybe_composite(&mut fixture.backend).expect("retry after Shared submit failure");
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .is_some(),
        "Backend retry submits the still-damaged Shared frame",
    );
    let receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("retry publishes the accepted frame receipt");
    assert_eq!(receipt.mode, CursorAcceptedMode::Hardware);
    fixture.retire_output(0, 1);

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit cursorless scene frame");
    fixture.retire_output(0, 2);
    let frame_records: Vec<_> = fixture
        .recorders
        .atomic
        .records()
        .into_iter()
        .filter(|record| !record.events.is_empty())
        .collect();
    assert_eq!(
        frame_records
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
        "the frame commits are rejected handoff, accepted retry, then hide",
    );
    assert!(matches!(
        frame_records[0].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            raw_os_error: Some(libc::EIO),
            ..
        }
    ));
    assert!(
        frame_records[1..].iter().all(|record| {
            record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
        })
    );
    fixture
        .backend
        .disable_output()
        .expect("disable and drain through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: drop the accepted-Shared lease recovery after its preflight fails; mode or lease teardown assertions fail.
fn atomic_cursor_only_shared_settlement_failure_recovers_accepted_lease() {
    use crate::kms::{cursor_plane::CursorLeaseBoundary, render::scene::CursorAcceptedMode};
    use yserver_core::backend::Backend;

    let Some(mut fixture) =
        AtomicCursorFixture::new_with_cursor_lease_failure(CursorLeaseBoundary::Accept)
            .expect("build scripted accepted-settlement fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 140, 110);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("accepted atomic frame recovers its cursor lease settlement");

    let receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted frame publishes its receipt");
    assert_eq!(receipt.mode, CursorAcceptedMode::Hardware);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(fixture.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.recorders.atomic.records()[0].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
    );
    fixture.retire_output(0, 1);

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("detach through a shared frame");
    fixture.retire_output(0, 2);
    fixture
        .backend
        .disable_output()
        .expect("disable and drain through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit composed-unflip reservation rollback after its atomic request fails; teardown retains a prepared lease.
fn atomic_cursor_only_composed_unflip_reservation_failure_rolls_back_before_fallback() {
    use yserver_core::backend::{AnyHandle, Backend};

    let Some(mut fixture) =
        AtomicCursorFixture::new_with_two_outputs().expect("build two-output rollback fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let direct = super::round_a::setup_cursor_only_direct_present(&mut fixture);

    let source = Backend::create_pixmap(&mut fixture.backend, None, 64, 64, 1)
        .expect("create boundary cursor source through Backend");
    Backend::put_image(
        &mut fixture.backend,
        None,
        source.as_raw(),
        64,
        64,
        1,
        0,
        0,
        &[0x31; 8 * 64],
    )
    .expect("paint boundary cursor through Backend");
    let picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(source),
        0,
        0,
        &[],
    )
    .expect("create cursor picture through Backend")
    .expect("cursor picture");
    let cursor = Backend::render_create_cursor(&mut fixture.backend, None, picture, 32, 0)
        .expect("create boundary cursor through Backend")
        .expect("cursor handle");
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("define boundary cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 800, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor frames");
    fixture.retire_output(0, 1);
    fixture.retire_output(1, 1);
    fixture.retire_output(0, 2);
    fixture.retire_output(1, 2);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(1));

    let mut direct_entered = false;
    for attempt in 0..16_u64 {
        let (candidate, event) = super::round_a::direct_present_for_id(&direct, 25_000 + attempt);
        Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
        if Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit direct probe through Backend")
        {
            let frame = 10 + u32::try_from(attempt).expect("frame fits");
            fixture.retire_output(0, frame);
            fixture.retire_output(1, frame);
            Backend::maybe_composite(&mut fixture.backend).expect("settle direct retirement");
            if fixture.backend.pstate.scanout_m2.active() {
                direct_entered = true;
                break;
            }
        } else if fixture.backend.scene.has_pending_page_flips() {
            let frame = 40 + u32::try_from(attempt).expect("frame fits");
            fixture.retire_output(0, frame);
            fixture.retire_output(1, frame);
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle composed direct-probation frame");
        }
    }
    assert!(direct_entered, "Backend direct Presents enter held scanout");
    // A Hide/Show round trip leaves a real show lease pending behind the
    // accepted detach events. The grouped unflip inherits that pending bind
    // once the old events retire, and must return its reservations if submit
    // then fails.
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    let before_hide = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("schedule the accepted held-direct detach before unflip");
    assert_eq!(fixture.recorders.atomic.records().len(), before_hide + 2);
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(1));
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_outputs(&[0, 1])
    );
    Backend::unmap_subwindow(&mut fixture.backend, None, direct.window_xid)
        .expect("request composed unflip through Backend");
    assert!(fixture.backend.pstate.scanout_m2.unflip_requested);
    let replacement_source = Backend::create_pixmap(&mut fixture.backend, None, 64, 64, 1)
        .expect("create replacement cursor source through Backend");
    Backend::put_image(
        &mut fixture.backend,
        None,
        replacement_source.as_raw(),
        64,
        64,
        1,
        0,
        0,
        &[0xa7; 8 * 64],
    )
    .expect("paint replacement cursor through Backend");
    let replacement_picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(replacement_source),
        0,
        0,
        &[],
    )
    .expect("create replacement picture through Backend")
    .expect("replacement picture");
    let replacement =
        Backend::render_create_cursor(&mut fixture.backend, None, replacement_picture, 32, 0)
            .expect("create replacement cursor through Backend")
            .expect("replacement cursor");
    Backend::define_cursor(&mut fixture.backend, None, root, replacement.as_raw())
        .expect("define replacement cursor while detached");
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    assert!(!fixture.backend.effective_cursor_hidden());
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_outputs(&[0, 1])
    );
    assert!(
        (0..2).any(|output_idx| {
            fixture
                .backend
                .platform
                .for_tests_cursor_show_intent(output_idx)
                .is_some()
        }),
        "the replacement cursor show lease remains pending behind detach events",
    );
    assert!(fixture.backend.pstate.scanout_m2.unflip_requested);
    let before_wait = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend).expect("grouped unflip waits for detach events");
    assert_eq!(fixture.recorders.atomic.records().len(), before_wait);
    fixture.retire_output(0, 60);
    fixture.retire_output(1, 60);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_outputs(&[0, 1])
    );
    for output in &fixture.backend.platform.outputs {
        assert!(
            fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
                .is_some_and(|state| state.applied.is_none())
        );
    }
    let before_unflip = fixture.recorders.atomic.records().len();
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    Backend::maybe_composite(&mut fixture.backend)
        .expect("reservation failure falls back to per-output scene commits");
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("cursor plane")
            .for_tests_prepared_sprite_user_count(),
        0,
        "the rejected grouped submit releases its reserved cursor leases before fallback retires",
    );

    assert!(fixture.backend.pstate.scanout_m2.degraded_composed_unflip);
    assert_eq!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .keys()
            .copied()
            .collect::<std::collections::HashSet<_>>(),
        [0, 1].into(),
        "fallback waits for both composed output flips",
    );
    assert!(fixture.backend.scene.has_pending_page_flips());
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(1));
    let all_records = fixture.recorders.atomic.records();
    let unflip_records: Vec<_> = all_records[before_unflip..]
        .iter()
        .filter(|record| !record.events.is_empty())
        .collect();
    assert_eq!(
        unflip_records
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
        "the rejected grouped request is followed by both Backend scene fallbacks",
    );
    assert!(matches!(
        unflip_records[0].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            raw_os_error: Some(libc::EIO),
            ..
        }
    ));
    assert_eq!(
        unflip_records[0].events.len(),
        2,
        "the failed grouped request reached the atomic sink with both output events",
    );
    let cursor_plane_objects: Vec<_> = fixture
        .backend
        .platform
        .outputs
        .iter()
        .filter_map(|output| {
            fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .and_then(|device| device.cursor.plane.as_ref())
                .and_then(|plane| plane.properties_for(output.output.crtc))
                .map(|properties| properties.plane.into())
        })
        .collect();
    assert!(
        unflip_records[0]
            .properties
            .iter()
            .any(|update| cursor_plane_objects.contains(&update.object)),
        "the failed grouped request carries a cursor update reserved before submit",
    );
    assert!(
        unflip_records[1..]
            .iter()
            .all(|record| record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok)
    );
    assert_eq!(
        fixture.backend.pstate.scanout_m2.unflip_outputs_to_replace,
        [0, 1].into(),
        "both direct outputs remain pinned through their accepted scene events",
    );
    let source_pin = fixture
        .backend
        .pstate
        .scanout_m2
        .current
        .as_ref()
        .expect("direct frame remains held until both outputs retire")
        .source_pin;
    fixture.retire_output(0, 61);
    assert_eq!(
        fixture.backend.pstate.scanout_m2.unflip_outputs_to_replace,
        [1].into(),
        "the first primary event retires only its own output",
    );
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert!(
        fixture
            .backend
            .present_source_pins
            .contains_key(&source_pin)
    );
    fixture.retire_output(1, 61);
    assert!(!fixture.backend.pstate.scanout_m2.active());
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(!fixture.backend.scene.has_pending_page_flips());

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit hidden cursor frames");
    fixture.retire_output(0, 62);
    fixture.retire_output(1, 62);
    fixture
        .backend
        .disable_output()
        .expect("disable and drain through Backend");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: remove the partial-reservation rollback in reserve_cursor_plane_updates; the first lease remains prepared after the second reservation fails.
fn atomic_cursor_only_composed_unflip_partial_reservation_failure_releases_prior_lease() {
    use yserver_core::backend::{AnyHandle, Backend};

    // The two initial shared attaches consume two successful reservations;
    // fail the second reservation of the later two-output composed unflip.
    let Some(mut fixture) =
        AtomicCursorFixture::new_with_two_outputs_and_cursor_lease_failure_after(
            crate::kms::cursor_plane::CursorLeaseBoundary::Reserve,
            3,
        )
        .expect("build scripted partial-reservation fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let direct = super::round_a::setup_cursor_only_direct_present(&mut fixture);

    let source = Backend::create_pixmap(&mut fixture.backend, None, 64, 64, 1)
        .expect("create boundary cursor source through Backend");
    Backend::put_image(
        &mut fixture.backend,
        None,
        source.as_raw(),
        64,
        64,
        1,
        0,
        0,
        &[0x31; 8 * 64],
    )
    .expect("paint boundary cursor through Backend");
    let picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(source),
        0,
        0,
        &[],
    )
    .expect("create cursor picture through Backend")
    .expect("cursor picture");
    let cursor = Backend::render_create_cursor(&mut fixture.backend, None, picture, 32, 0)
        .expect("create boundary cursor through Backend")
        .expect("cursor handle");
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("define boundary cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 800, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor frames");
    fixture.retire_output(0, 1);
    fixture.retire_output(1, 1);
    fixture.retire_output(0, 2);
    fixture.retire_output(1, 2);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(1));

    let mut direct_entered = false;
    for attempt in 0..16_u64 {
        let (candidate, event) = super::round_a::direct_present_for_id(&direct, 27_000 + attempt);
        Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
        if Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit direct probe through Backend")
        {
            let frame = 20 + u32::try_from(attempt).expect("frame fits");
            fixture.retire_output(0, frame);
            fixture.retire_output(1, frame);
            Backend::maybe_composite(&mut fixture.backend).expect("settle direct retirement");
            if fixture.backend.pstate.scanout_m2.active() {
                direct_entered = true;
                break;
            }
        } else if fixture.backend.scene.has_pending_page_flips() {
            let frame = 50 + u32::try_from(attempt).expect("frame fits");
            fixture.retire_output(0, frame);
            fixture.retire_output(1, frame);
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle composed direct-probation frame");
        }
    }
    assert!(direct_entered, "Backend direct Presents enter held scanout");
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(1));
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    let before_hide = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("schedule accepted hides while the held direct frame has no successor");
    assert_eq!(fixture.recorders.atomic.records().len(), before_hide + 2);
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(1));
    for output in &fixture.backend.platform.outputs {
        assert!(
            fixture
                .backend
                .platform
                .for_tests_atomic_cursor_commit_is_in_flight(
                    output.key.device_key,
                    output.output.crtc,
                )
        );
    }
    let replacement_source = Backend::create_pixmap(&mut fixture.backend, None, 64, 64, 1)
        .expect("create replacement cursor source through Backend");
    Backend::put_image(
        &mut fixture.backend,
        None,
        replacement_source.as_raw(),
        64,
        64,
        1,
        0,
        0,
        &[0xa7; 8 * 64],
    )
    .expect("paint replacement cursor through Backend");
    let replacement_picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(replacement_source),
        0,
        0,
        &[],
    )
    .expect("create replacement picture through Backend")
    .expect("replacement picture");
    let replacement =
        Backend::render_create_cursor(&mut fixture.backend, None, replacement_picture, 32, 0)
            .expect("create replacement cursor through Backend")
            .expect("replacement cursor");

    let (successor, successor_event) = super::round_a::direct_present_for_id(&direct, 28_000);
    Backend::note_present_scanout_candidate(&mut fixture.backend, successor);
    let _ = Backend::try_present_direct(&mut fixture.backend, successor, successor_event)
        .expect("queue a direct successor through Backend");
    assert!(
        fixture.backend.pstate.scanout_m2.pending.is_some()
            || fixture.backend.pstate.scanout_m2.queued_successor.is_some()
    );
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    assert!(!fixture.backend.effective_cursor_hidden());
    Backend::define_cursor(&mut fixture.backend, None, root, replacement.as_raw())
        .expect("replace the cursor while both detached outputs await events");
    assert!(
        (0..2).all(|output_idx| fixture
            .backend
            .platform
            .for_tests_cursor_show_intent(output_idx)
            .is_some()),
        "both replacement leases wait behind detach events: {:?}",
        (0..2)
            .map(|output_idx| fixture
                .backend
                .platform
                .for_tests_cursor_show_intent(output_idx))
            .collect::<Vec<_>>(),
    );
    Backend::unmap_subwindow(&mut fixture.backend, None, direct.window_xid)
        .expect("request composed unflip through Backend");
    assert!(fixture.backend.pstate.scanout_m2.unflip_requested);
    let records_before_cursor_retirement = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("grouped unflip waits for both cursor detach events");
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        records_before_cursor_retirement
    );
    fixture.retire_output(0, 80);
    fixture.retire_output(1, 81);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_outputs(&[0, 1])
    );
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        records_before_cursor_retirement,
        "cursor event retirement waits for the grouped primary successor",
    );
    assert!((0..2).all(|output_idx| {
        fixture
            .backend
            .platform
            .for_tests_cursor_show_intent(output_idx)
            .is_some()
    }));
    assert!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("cursor plane")
            .sprite_pool_users_are_zero()
    );
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("cursor plane")
            .for_tests_prepared_sprite_user_count(),
        0,
        "the reservation fixture starts with no prepared framebuffer users",
    );

    let before_partial_reservation = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("partial composed-unflip reservation falls back through Backend");
    let prepared_users = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .expect("cursor plane")
        .for_tests_prepared_sprite_user_count();
    assert_eq!(
        prepared_users, 0,
        "the first successful reservation is released after the second fails",
    );
    assert!(
        fixture.backend.pstate.scanout_m2.degraded_composed_unflip,
        "the scripted second reservation should reject grouped submit: {:#?}",
        &fixture.recorders.atomic.records()[before_partial_reservation..]
    );
    assert_eq!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .keys()
            .copied()
            .collect::<std::collections::HashSet<_>>(),
        [0, 1].into(),
        "fallback owns both output flips",
    );
    assert!(fixture.backend.scene.has_pending_page_flips());
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());

    let after_partial_reservation = fixture.recorders.atomic.records();
    let fallback_records: Vec<_> = after_partial_reservation[before_partial_reservation..]
        .iter()
        .filter(|record| !record.events.is_empty())
        .collect();
    assert_eq!(
        fallback_records
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
        "reservation failure submits only the two per-output fallback frames",
    );
    assert!(fallback_records.iter().all(|record| {
        record.events.len() == 1
            && record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
    }));
    assert!(
        after_partial_reservation[before_partial_reservation..]
            .iter()
            .all(|record| record.events.len() != 2),
        "no grouped unflip reached atomic submit with only part of its cursor lease set",
    );
    let fallback_bindings: Vec<_> = fixture
        .backend
        .platform
        .outputs
        .iter()
        .map(|output| {
            fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
                .and_then(|state| state.applied)
                .expect("Backend fallback accepted a cursor binding")
        })
        .collect();
    for output_idx in 0..2 {
        let receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(output_idx)
            .expect("Backend fallback publishes an accepted cursor receipt");
        assert_eq!(
            receipt.mode,
            crate::kms::render::scene::CursorAcceptedMode::Hardware
        );
        assert_eq!(
            receipt.primary_cursor,
            crate::kms::render::scene::CursorPrimaryContent::Cursorless,
            "each fallback primary leaves the cursor to its accepted hardware plane",
        );
    }
    assert_eq!(fallback_bindings[0].lease, fallback_bindings[1].lease);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("cursor plane")
            .sprite_user_count(fallback_bindings[0].lease),
        Some(2),
        "the only live users are the two accepted fallback bindings",
    );

    fixture.retire_output(0, 71);
    fixture.retire_output(1, 71);
    assert!(!fixture.backend.pstate.scanout_m2.active());
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(!fixture.backend.scene.has_pending_page_flips());

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit hidden cursor frames");
    fixture.retire_output(0, 72);
    fixture.retire_output(1, 72);
    fixture
        .backend
        .disable_output()
        .expect("disable and drain through Backend");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit the copied frame's cursor_updates from SceneWithCursor; the late X/Y delta must disappear.
fn atomic_cursor_only_second_copied_frame_carries_late_motion_delta() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build copied motion fixture") else {
        return;
    };
    let sink_atomic = install_copied_recording_route(&mut fixture);
    fixture
        .recorders
        .render_completion_readiness
        .hold_ready_jobs();

    let root = fixture.backend.core.window_id;
    let cursor = create_solid_argb_cursor(&mut fixture, 32, [0, 0, 255, 255]);
    Backend::define_cursor(&mut fixture.backend, None, root, cursor)
        .expect("define the hardware cursor through Backend");
    let p0 = (100, 90);
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, p0.0, p0.1);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("build first copied frame");

    let first_job = fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_ids()[0];
    wait_for_held_scanout_job(&mut fixture, first_job);
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(first_job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let first_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("first copied frame accepted its cursor binding");
    assert_eq!(first_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(
        first_receipt.primary_cursor,
        CursorPrimaryContent::Cursorless
    );
    let first_record = sink_atomic.records().pop().expect("first copied commit");
    assert_eq!(
        first_record.kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor
    );
    fixture.retire_output(0, 1);
    assert!(
        !fixture.backend.scene.has_pending_page_flip(0),
        "the output is ready before its second copied completion is produced",
    );

    sink_atomic.clear_records();
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("build the second copied frame");
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let (second_job, second_bo, second_frame) = wait_for_copied_job_for_crtc(&mut fixture, crtc);
    let staged = &fixture.backend.platform.staged_cursor_frames[&(0, second_bo)];
    assert_eq!(staged.plan.frame, second_frame);
    assert!(
        staged.cursor_updates.is_empty(),
        "the second frame starts with no cursor delta because its binding and position match applied state",
    );
    wait_for_held_scanout_job(&mut fixture, second_job);

    let p1 = (137, 119);
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, p1.0, p1.1);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("retain late motion behind the second copied frame");
    assert!(sink_atomic.records().is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        1,
        "the second frame remains the only pending scene acknowledgement",
    );

    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(second_job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("second copied frame publishes after atomic acceptance");
    assert_eq!(receipt.frame, Some(second_frame));
    assert_eq!(receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(receipt.primary_cursor, CursorPrimaryContent::Cursorless);
    let records = sink_atomic.records();
    assert_eq!(
        records.len(),
        1,
        "the late delta rides the copied primary commit"
    );
    let record = &records[0];
    assert_eq!(
        record.kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor
    );
    assert_eq!(record.events.len(), 1);
    assert_eq!(
        record.events[0].crtc,
        fixture.backend.platform.outputs[0].output.crtc
    );
    let cursor_properties = cursor_plane_for_output(&fixture, 0)
        .properties_for(fixture.backend.platform.outputs[0].output.crtc)
        .expect("output cursor properties");
    assert!(record.properties.iter().any(|update| {
        update.object == cursor_properties.plane.into()
            && update.property == cursor_properties.crtc_x
            && update.value == p1.0 as i64 as u64
    }));
    assert!(record.properties.iter().any(|update| {
        update.object == cursor_properties.plane.into()
            && update.property == cursor_properties.crtc_y
            && update.value == p1.1 as i64 as u64
    }));
    assert!(
        !record.properties.iter().any(|update| {
            update.object == cursor_properties.plane.into()
                && update.property == cursor_properties.fb_id
        }),
        "motion reuses the accepted sprite framebuffer",
    );

    fixture.retire_output(0, 2);
    fixture
        .backend
        .disable_output()
        .expect("Backend disables and drains the copied output");
    fixture.assert_cursor_quiescent();
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: publish the copied frame receipt when source rendering is accepted; the pre-submit assertion fails.
fn atomic_cursor_only_copied_frame_keeps_assignment_until_actual_submit() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build copied cursor fixture") else {
        return;
    };
    let sink_atomic = install_copied_recording_route(&mut fixture);
    fixture
        .recorders
        .render_completion_readiness
        .hold_ready_jobs();
    let cursor = fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 90, 80);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit copied F0 through Backend");

    let job_id = fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_ids()[0];
    wait_for_held_scanout_job(&mut fixture, job_id);
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None
    );
    assert!(sink_atomic.records().is_empty(), "F0 has not reached KMS");

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 137, 119);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("retain P1 behind copied F0");
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None,
        "neither cursor mode nor frame assignment is published before KMS acceptance",
    );
    assert!(sink_atomic.records().is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        1,
        "the output's one pending scene acknowledgement still owns F0",
    );

    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(job_id);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("copied frame publishes after its final atomic acceptance");
    assert_eq!(receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(receipt.primary_cursor, CursorPrimaryContent::Cursorless);
    assert_eq!(
        receipt.frame,
        Some(crate::kms::render::scene::CursorFrameId(1))
    );
    let record = sink_atomic
        .records()
        .pop()
        .expect("accepted copied KMS commit");
    assert_eq!(record.events.len(), 1);
    let cursor_plane = fixture.backend.platform.devices[1]
        .cursor
        .plane
        .as_ref()
        .expect("copied KMS device owns its cursor plane");
    let cursor_properties = cursor_plane
        .properties_for(fixture.backend.platform.outputs[0].output.crtc)
        .expect("cursor plane properties for copied output");
    let cursor_position = |property| {
        record
            .properties
            .iter()
            .find(|update| {
                update.object == cursor_properties.plane.into() && update.property == property
            })
            .map(|update| update.value)
            .expect("final copied request carries the cursor position")
    };
    assert_eq!(cursor_position(cursor_properties.crtc_x), 137);
    assert_eq!(cursor_position(cursor_properties.crtc_y), 119);

    fixture.retire_output(0, 1);
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    assert!(
        !fixture.backend.platform.scanout_pools[0]
            .as_ref()
            .expect("copied pool remains installed")
            .display_pool()
            .has_pending_pageflip(),
        "copied destination BO retired with the Backend page-flip entry",
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0),
        "frame-owned cursor event retired with the Backend page-flip entry",
    );
    fixture
        .backend
        .disable_output()
        .expect("Backend disables and drains copied output");
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0,
    );
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
    let _ = cursor;
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: recompute a deferred SW frame's cursor assignment from global state; V1/P1 no longer matches F0 pixels.
fn atomic_cursor_only_software_frame_keeps_rendered_cursor_snapshot() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorFrameId, CursorPrimaryContent};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new_with_cursor_capabilities(None, None)
        .expect("build copied software cursor fixture")
    else {
        return;
    };
    let sink_atomic = install_copied_recording_route(&mut fixture);
    fixture
        .recorders
        .render_completion_readiness
        .hold_ready_jobs();
    let p0 = (90, 80);
    let p1 = (210, 160);
    let v0 = create_solid_argb_cursor(&mut fixture, 65, [0, 0, 255, 255]);
    let v0_version = fixture.backend.cursor_records[&v0].version;
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, v0)
        .expect("define V0 through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, p0.0, p0.1);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit copied F0 through Backend");

    let f0_job = fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_ids()[0];
    wait_for_held_scanout_job(&mut fixture, f0_job);
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None
    );
    assert!(sink_atomic.records().is_empty());

    let v1 = create_solid_argb_cursor(&mut fixture, 65, [255, 0, 0, 255]);
    let v1_version = fixture.backend.cursor_records[&v1].version;
    Backend::define_cursor(&mut fixture.backend, None, root, v1)
        .expect("define V1 while F0 acknowledgement is pending");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, p1.0, p1.1);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("F0 acknowledgement blocks F1");
    assert!(fixture.backend.scene.scene_structure_dirty);
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        1,
    );
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None
    );
    assert!(sink_atomic.records().is_empty());

    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(f0_job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let f0 = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("F0 receipt follows KMS acceptance");
    assert_eq!(f0.frame, Some(CursorFrameId(1)));
    assert_eq!(f0.mode, CursorAcceptedMode::Software);
    assert_eq!(
        f0.primary_cursor,
        CursorPrimaryContent::Software {
            pos: p0,
            record_version: v0_version,
        },
        "F0 retains the assignment recorded with its rendered pixels",
    );
    fixture.retire_output(0, 1);
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert!(
        fixture
            .backend
            .scene
            .output_ready_for_submit(0, &fixture.backend.platform),
        "output is ready for F1 after F0 retirement",
    );
    assert!(
        fixture.backend.platform.scanout_pools[0]
            .as_ref()
            .expect("copied pool remains installed")
            .display_pool()
            .bos
            .iter()
            .any(|bo| bo.state.phase == crate::kms::vk::scanout::BoPhase::Free)
    );
    assert!(fixture.backend.scene.scene_structure_dirty);
    let f0_pixel = read_copied_pixel(&mut fixture, (p0.0 + 4, p0.1 + 4));
    assert_eq!(&f0_pixel[..3], &[0, 0, 255], "F0 pixels contain red V0");

    Backend::define_cursor(&mut fixture.backend, None, root, v0)
        .expect("reselect V0 through Backend after F0 retirement");
    Backend::define_cursor(&mut fixture.backend, None, root, v1)
        .expect("reselect V1 through Backend for later F1");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, p1.0 + 1, p1.1);
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, p1.0, p1.1);
    Backend::put_image(
        &mut fixture.backend,
        None,
        root,
        24,
        1,
        1,
        0,
        0,
        &[0x20, 0x40, 0x60, 0],
    )
    .expect("damage the root with a real pixel update through Backend");
    assert!(
        fixture.backend.for_tests_scene_wants_compose()
            || fixture.backend.for_tests_scene_wants_scene(),
        "later cursor/root damage remains runnable after F0 retirement",
    );
    let f1_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let f1_jobs = loop {
        Backend::maybe_composite(&mut fixture.backend).expect("submit later F1 through Backend");
        let jobs = fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_ids();
        if !jobs.is_empty() {
            break jobs;
        }
        assert!(
            std::time::Instant::now() < f1_deadline,
            "later F1 did not produce a copied render job",
        );
        std::thread::yield_now();
    };
    assert_eq!(
        f1_jobs.len(),
        1,
        "later F1 completion remains deferred; scene_pending={}, records={:?}",
        fixture.backend.scene.has_pending_page_flip(0),
        sink_atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
    );
    let f1_job = f1_jobs[0];
    wait_for_held_scanout_job(&mut fixture, f1_job);
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(f1_job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let f1 = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("F1 receipt follows KMS acceptance");
    assert_eq!(f1.frame, Some(CursorFrameId(2)));
    assert_eq!(f1.mode, CursorAcceptedMode::Software);
    assert_eq!(
        f1.primary_cursor,
        CursorPrimaryContent::Software {
            pos: p1,
            record_version: v1_version,
        },
        "only F1 describes V1 at P1",
    );
    fixture.retire_output(0, 2);
    let f1_pixel = read_copied_pixel(&mut fixture, (p1.0 + 4, p1.1 + 4));
    assert_eq!(&f1_pixel[..3], &[255, 0, 0], "F1 pixels contain blue V1");
    assert_eq!(
        sink_atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "the exact copied primary commit sequence is F0 then F1",
    );

    fixture
        .backend
        .disable_output()
        .expect("Backend disables and drains copied output");
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0,
    );
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: release a frame-owned first-bind lease early; pressure must not rewrite A's staged V0 buffer.
fn atomic_cursor_only_built_sprite_is_not_reused_before_submit() {
    use crate::kms::render::scene::{
        CursorAcceptedMode, CursorFrameAssignment, CursorPrimaryContent,
    };
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new_with_two_outputs()
        .expect("build two-output copied cursor fixture")
    else {
        return;
    };
    let sink_atomic = install_copied_recording_route(&mut fixture);
    fixture
        .recorders
        .render_completion_readiness
        .hold_ready_jobs();
    for (output_idx, output) in fixture.backend.platform.outputs.iter_mut().enumerate() {
        let base = u32::try_from(101 + output_idx * 10).expect("property handle fits");
        output.output.plane_fb_id_prop = ::drm::control::from_u32(base).expect("FB_ID property");
        output.output.plane_crtc_id_prop =
            ::drm::control::from_u32(base + 1).expect("CRTC_ID property");
        output.output.plane_src_x_prop =
            ::drm::control::from_u32(base + 2).expect("SRC_X property");
        output.output.plane_src_y_prop =
            ::drm::control::from_u32(base + 3).expect("SRC_Y property");
        output.output.plane_src_w_prop =
            ::drm::control::from_u32(base + 4).expect("SRC_W property");
        output.output.plane_src_h_prop =
            ::drm::control::from_u32(base + 5).expect("SRC_H property");
        output.output.plane_crtc_x_prop =
            ::drm::control::from_u32(base + 6).expect("CRTC_X property");
        output.output.plane_crtc_y_prop =
            ::drm::control::from_u32(base + 7).expect("CRTC_Y property");
        output.output.plane_crtc_w_prop =
            ::drm::control::from_u32(base + 8).expect("CRTC_W property");
        output.output.plane_crtc_h_prop =
            ::drm::control::from_u32(base + 9).expect("CRTC_H property");
    }
    let root = fixture.backend.core.window_id;
    let p0 = (120, 90);
    let p1 = (920, 110);
    let p_final = (240, 190);
    let v0 = create_solid_argb_cursor(&mut fixture, 32, [0, 0, 255, 255]);
    let v0_version = fixture.backend.cursor_records[&v0].version;
    Backend::define_cursor(&mut fixture.backend, None, root, v0)
        .expect("define F0 hardware sprite through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, p0.0, p0.1);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit A and B copied renders");

    let initial_jobs = fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_associations();
    assert_eq!(
        initial_jobs.len(),
        2,
        "both outputs own one real render job"
    );
    let crtc_a = fixture.backend.platform.outputs[0].output.crtc;
    let crtc_b = fixture.backend.platform.outputs[1].output.crtc;
    let key_a =
        crate::kms::render::platform::CrtcKey::for_output(&fixture.backend.platform.outputs[0]);
    let key_b =
        crate::kms::render::platform::CrtcKey::for_output(&fixture.backend.platform.outputs[1]);
    let (job_a, bo_a, frame_a) = initial_jobs
        .iter()
        .find(|(_, key, _, _)| *key == key_a)
        .map(|(job, _, bo, frame)| (*job, *bo, *frame))
        .expect("A job carries its device-qualified CRTC identity");
    let (job_b0, bo_b0, frame_b0) = initial_jobs
        .iter()
        .find(|(_, key, _, _)| *key == key_b)
        .map(|(job, _, bo, frame)| (*job, *bo, *frame))
        .expect("B job carries its device-qualified CRTC identity");
    for (job, output_idx, bo_idx, frame) in
        [(job_a, 0, bo_a, frame_a), (job_b0, 1, bo_b0, frame_b0)]
    {
        let association = fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_associations()
            .into_iter()
            .find(|(pending_job, _, _, _)| *pending_job == job)
            .expect("pending completion association remains live");
        assert_eq!(association.2, bo_idx);
        assert_eq!(association.3, frame);
        assert_eq!(
            fixture.backend.platform.staged_cursor_frames[&(output_idx, bo_idx)]
                .plan
                .frame,
            frame,
        );
        wait_for_held_scanout_job(&mut fixture, job);
    }
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None
    );
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(1),
        None
    );
    assert!(sink_atomic.records().is_empty());

    let f0_staged = &fixture.backend.platform.staged_cursor_frames[&(0, bo_a)];
    assert_eq!(f0_staged.plan.frame, frame_a);
    assert_eq!(f0_staged.plan.assignment, CursorFrameAssignment::Hardware);
    assert!(!f0_staged.plan.hardware_only);
    assert_eq!(
        f0_staged.plan.sprite.as_ref().unwrap().record_version,
        v0_version,
        "the accepted first-bind frame owns its immutable V0 sprite snapshot",
    );
    assert!(f0_staged.cursor_updates_reserved);
    let f0_binding = f0_staged.cursor_updates[0]
        .prepared
        .next
        .expect("F0 staged first-bind hardware attachment");
    assert_eq!(f0_binding.lease.version(), v0_version);
    let (f0_device_fd, f0_handle, f0_bytes) =
        cursor_framebuffer_bytes(&fixture, f0_binding.framebuffer);
    assert_eq!(&f0_bytes[..4], &[0, 0, 255, 255]);
    assert_eq!(
        cursor_plane_for_output(&fixture, 0).for_tests_prepared_sprite_user_count(),
        1,
        "F0 retains its frame-owned sprite lease while copied completion is held",
    );

    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(job_b0);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let b0_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(1)
        .expect("B0 receipt follows B0's atomic acceptance");
    assert_eq!(b0_receipt.frame, Some(frame_b0));
    assert_eq!(b0_receipt.mode, CursorAcceptedMode::Hidden);
    assert_eq!(b0_receipt.primary_cursor, CursorPrimaryContent::Cursorless);
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None
    );
    let b0_pool_fb = fixture.backend.platform.scanout_pools[1]
        .as_ref()
        .expect("B copied pool")
        .display_pool()
        .bos[bo_b0]
        .fb_handle
        .expect("B0 primary framebuffer");
    let first = sink_atomic.records();
    assert_eq!(first.len(), 1);
    assert_primary_framebuffer(&fixture, &first[0], 1, u32::from(b0_pool_fb));
    assert_eq!(first[0].events[0].crtc, crtc_b);
    assert!(
        fixture.backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&crtc_b)
            .and_then(|state| state.applied)
            .is_none()
    );
    fixture.retire_output(1, 1);

    let v1 = create_solid_argb_cursor(&mut fixture, 32, [255, 0, 0, 255]);
    let v1_version = fixture.backend.cursor_records[&v1].version;
    Backend::define_cursor(&mut fixture.backend, None, root, v1)
        .expect("define V1 through Backend while A remains held");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, p1.0, p1.1);
    super::round_a::damage_cursor_root(&mut fixture);
    let (job_b1, bo_b1, frame_b1) = wait_for_copied_job_for_crtc(&mut fixture, crtc_b);
    wait_for_held_scanout_job(&mut fixture, job_b1);
    let b1_staged = &fixture.backend.platform.staged_cursor_frames[&(1, bo_b1)];
    assert_eq!(b1_staged.plan.frame, frame_b1);
    assert_eq!(b1_staged.plan.assignment, CursorFrameAssignment::Hardware);
    assert!(!b1_staged.plan.hardware_only);
    let b1_fb = b1_staged.cursor_updates[0]
        .prepared
        .next
        .expect("B1 staged first-bind V1 sprite")
        .framebuffer;
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(job_b1);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let b1_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(1)
        .expect("B1 receipt follows B1's atomic acceptance");
    assert_eq!(b1_receipt.frame, Some(frame_b1));
    assert_eq!(b1_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(b1_receipt.primary_cursor, CursorPrimaryContent::Cursorless);
    let b1_binding = fixture
        .backend
        .platform
        .device_for_key(fixture.backend.platform.outputs[1].key.device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc_b]
        .applied
        .expect("B1 accepted V1 cursor binding");
    assert_eq!(b1_binding.lease.version(), v1_version);
    assert_eq!(b1_binding.framebuffer, b1_fb);
    let (_, _, b1_bytes) = cursor_framebuffer_bytes(&fixture, b1_binding.framebuffer);
    let b1 = sink_atomic.records().pop().expect("B1 atomic record");
    assert_eq!(b1.events[0].crtc, crtc_b);
    assert_cursor_framebuffer(&fixture, &b1, 1, b1_binding.framebuffer);
    fixture.retire_output(1, 2);

    let v2 = create_solid_argb_cursor(&mut fixture, 32, [0, 255, 0, 255]);
    let v2_version = fixture.backend.cursor_records[&v2].version;
    Backend::define_cursor(&mut fixture.backend, None, root, v2)
        .expect("define V2 through Backend while A remains held");
    super::round_a::damage_cursor_root(&mut fixture);
    let (job_b2, bo_b2, frame_b2) = wait_for_copied_job_for_crtc(&mut fixture, crtc_b);
    wait_for_held_scanout_job(&mut fixture, job_b2);
    let b2_staged = &fixture.backend.platform.staged_cursor_frames[&(1, bo_b2)];
    assert_eq!(b2_staged.plan.frame, frame_b2);
    assert!(b2_staged.plan.hardware_only);
    assert!(b2_staged.cursor_updates.is_empty());
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(job_b2);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let b2_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(1)
        .expect("B2 receipt follows B2's atomic acceptance");
    assert_eq!(b2_receipt.frame, Some(frame_b2));
    assert_eq!(b2_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(b2_receipt.primary_cursor, CursorPrimaryContent::Cursorless);
    let b2_binding = fixture
        .backend
        .platform
        .device_for_key(fixture.backend.platform.outputs[1].key.device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc_b]
        .applied
        .expect("B2 accepted V2 cursor binding");
    assert_eq!(b2_binding.lease.version(), v2_version);
    let b2 = sink_atomic.records().pop().expect("B2 atomic record");
    assert_eq!(b2.events[0].crtc, crtc_b);
    assert_cursor_framebuffer(&fixture, &b2, 1, b2_binding.framebuffer);
    assert_eq!(
        cursor_framebuffer_bytes(&fixture, b1_binding.framebuffer).2,
        b1_bytes,
        "B2 keeps B1's old framebuffer immutable until B2's replacement event",
    );
    assert_eq!(
        cursor_framebuffer_bytes(&fixture, f0_binding.framebuffer).2,
        f0_bytes,
        "B2 cannot overwrite F0 while its copied render completion is held",
    );
    fixture.retire_output(1, 3);

    let v3 = create_solid_argb_cursor(&mut fixture, 32, [255, 255, 0, 255]);
    Backend::define_cursor(&mut fixture.backend, None, root, v3)
        .expect("define pressure version V3 through Backend");
    assert_eq!(
        fixture
            .recorders
            .cursor_buffer_ledger
            .mapped_bytes(f0_device_fd, f0_handle)
            .expect("F0 framebuffer remains mapped under cursor pool pressure"),
        f0_bytes,
        "newer cursor versions never rewrite F0's reserved V0 framebuffer",
    );
    assert!(fixture.recorders.cursor_buffer_ledger.records().iter().all(|record| {
        !matches!(record, crate::kms::cursor_plane::ops::CursorBufferRecord::RemoveFb { fb, .. } if *fb == f0_binding.framebuffer)
    }));

    Backend::warp_pointer_root(
        &mut fixture.backend,
        &mut fixture.state,
        p_final.0,
        p_final.1,
    );
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(job_a);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
    let f0_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("F0 receipt follows its own copied atomic acceptance");
    assert_eq!(f0_receipt.frame, Some(frame_a));
    assert_eq!(f0_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(f0_receipt.primary_cursor, CursorPrimaryContent::Cursorless);
    let f0_binding = fixture
        .backend
        .platform
        .device_for_key(fixture.backend.platform.outputs[0].key.device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc_a]
        .applied
        .expect("A accepted its live cursor binding");
    assert_eq!(
        f0_binding.lease.version(),
        v0_version,
        "A's copied frame submits the first-bind sprite that its staged plan owns",
    );
    assert_eq!(
        &cursor_framebuffer_bytes(&fixture, f0_binding.framebuffer).2[..4],
        &[0, 0, 255, 255],
        "the first-bind framebuffer still contains the staged V0 bytes",
    );
    let records = sink_atomic.records();
    assert_eq!(records.len(), 4);
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
        "the exact copied commit order is B0, B1, B2, then A's held F0",
    );
    assert_eq!(records[3].events[0].crtc, crtc_a);
    assert_cursor_framebuffer(&fixture, &records[3], 0, f0_binding.framebuffer);
    let cursor_properties = cursor_plane_for_output(&fixture, 0)
        .properties_for(crtc_a)
        .expect("A cursor properties");
    assert!(records[3].properties.iter().any(|update| {
        update.object == cursor_properties.plane.into()
            && update.property == cursor_properties.crtc_x
            && update.value == p_final.0 as i64 as u64
    }));
    assert!(records[3].properties.iter().any(|update| {
        update.object == cursor_properties.plane.into()
            && update.property == cursor_properties.crtc_y
            && update.value == p_final.1 as i64 as u64
    }));
    assert_ne!(v1_version, v2_version);
    fixture.retire_output(0, 4);
    fixture
        .backend
        .disable_output()
        .expect("Backend disables both copied outputs and drains cursor leases");
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0
    );
    assert!(
        sink_atomic
            .records()
            .iter()
            .all(|record| record.events.is_empty() || record.events.len() == 1)
    );
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: run a cursor operation from retirement (the no-intent trace grows), or clear a newer pointer generation on old retirement (the newest-position commit disappears).
fn atomic_cursor_only_retirement_is_cursor_operation_free() {
    use crate::{
        drm::atomic::AtomicCommitKind,
        kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent},
    };
    use yserver_core::backend::Backend;

    for queue_newer_cursor in [false, true] {
        let Some(mut fixture) =
            AtomicCursorFixture::new().expect("build cursor retirement fixture")
        else {
            return;
        };
        fixture
            .backend
            .platform
            .for_tests_accept_output_power_commits();

        let root = fixture.backend.core.window_id;
        let first = create_solid_argb_cursor(&mut fixture, 16, [0x20, 0x40, 0xe0, 0xff]);
        let first_version = fixture.backend.cursor_records[&first].version;
        Backend::define_cursor(&mut fixture.backend, None, root, first)
            .expect("define first cursor through Backend");
        let p0 = (120, 90);
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, p0.0, p0.1);
        super::round_a::damage_cursor_root(&mut fixture);
        Backend::maybe_composite(&mut fixture.backend).expect("accept cursor-bearing scene frame");

        let first_receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted scene cursor receipt");
        assert_eq!(first_receipt.mode, CursorAcceptedMode::Hardware);
        assert_eq!(
            first_receipt.primary_cursor,
            CursorPrimaryContent::Cursorless
        );
        assert_eq!(
            first_receipt.frame,
            Some(crate::kms::render::scene::CursorFrameId(1))
        );
        assert_eq!(fixture.recorders.atomic.records().len(), 1);
        assert_eq!(
            fixture.recorders.atomic.records()[0].kind,
            AtomicCommitKind::SceneWithCursor,
        );

        let records_before_retirement = fixture.recorders.atomic.records();
        let buffers_before_retirement = fixture.recorders.cursor_buffer_ledger.records();
        fixture.retire_output(0, 1);
        assert!(!fixture.backend.scene.has_pending_page_flip(0));
        let records = fixture.recorders.atomic.records();
        assert_eq!(
            records, records_before_retirement,
            "retiring a frame with no newer cursor intent submits no cursor I/O",
        );
        assert_eq!(
            fixture.recorders.cursor_buffer_ledger.records(),
            buffers_before_retirement,
            "receipt-only retirement does not upload, add, or remove cursor buffers",
        );

        if queue_newer_cursor {
            let intermediate = create_solid_argb_cursor(&mut fixture, 16, [0x50, 0xc0, 0x10, 0xff]);
            Backend::define_cursor(&mut fixture.backend, None, root, intermediate)
                .expect("queue an intermediate cursor image through Backend");
            Backend::maybe_composite(&mut fixture.backend)
                .expect("submit the intermediate cursor-only update");
            let intermediate_records = fixture.recorders.atomic.records();
            assert_eq!(intermediate_records.len(), 2);
            assert_eq!(intermediate_records[1].kind, AtomicCommitKind::CursorOnly);

            let newest = create_solid_argb_cursor(&mut fixture, 16, [0xe0, 0x30, 0x10, 0xff]);
            let newest_version = fixture.backend.cursor_records[&newest].version;
            Backend::define_cursor(&mut fixture.backend, None, root, newest)
                .expect("queue the newest cursor image while the prior event is pending");
            let latest_position = (230, 170);
            Backend::warp_pointer_root(
                &mut fixture.backend,
                &mut fixture.state,
                latest_position.0,
                latest_position.1,
            );
            assert_eq!(fixture.backend.effective_cursor_xid, Some(newest));
            assert_eq!(fixture.recorders.atomic.records().len(), 2);
            let latest_intent = fixture
                .backend
                .platform
                .for_tests_cursor_show_intent(0)
                .expect("newest pointer position remains live behind the prior event");
            assert_eq!(
                (latest_intent.2, latest_intent.3),
                (
                    latest_position.0 - fixture.backend.platform.outputs[0].x,
                    latest_position.1 - fixture.backend.platform.outputs[0].y,
                ),
            );

            fixture.retire_output(0, 2);
            Backend::before_block(&mut fixture.backend);
            let after_motion = fixture.recorders.atomic.records();
            assert_eq!(after_motion.len(), 3);
            assert_eq!(
                after_motion
                    .iter()
                    .map(|record| record.kind)
                    .collect::<Vec<_>>(),
                [
                    AtomicCommitKind::SceneWithCursor,
                    AtomicCommitKind::CursorOnly,
                    AtomicCommitKind::CursorOnly,
                ],
                "old retirement submits only the newest live position",
            );
            let properties = cursor_plane_for_output(&fixture, 0)
                .properties_for(fixture.backend.platform.outputs[0].output.crtc)
                .expect("output cursor properties");
            assert!(
                atomic_cursor_framebuffer(&after_motion[2], properties).is_none(),
                "the position update does not replay an older framebuffer operation",
            );
            assert_eq!(
                atomic_cursor_position(&after_motion[2], properties),
                Some((
                    latest_position.0 - fixture.backend.platform.outputs[0].x,
                    latest_position.1 - fixture.backend.platform.outputs[0].y
                )),
                "the newest live motion owns the post-retirement cursor-only update",
            );

            fixture.retire_output(0, 3);
            Backend::maybe_composite(&mut fixture.backend)
                .expect("retry the newest image after the old framebuffer lease retires");
            let records = fixture.recorders.atomic.records();
            assert_eq!(records.len(), 4);
            assert_eq!(
                records.iter().map(|record| record.kind).collect::<Vec<_>>(),
                [
                    AtomicCommitKind::SceneWithCursor,
                    AtomicCommitKind::CursorOnly,
                    AtomicCommitKind::CursorOnly,
                    AtomicCommitKind::CursorOnly,
                ],
                "the newest image is submitted after its framebuffer lease becomes available",
            );
            let newest_framebuffer = atomic_cursor_framebuffer(&records[3], properties)
                .expect("newest cursor commit has an FB_ID");
            assert_eq!(
                atomic_cursor_position(&records[3], properties),
                None,
                "the newest image update omits already accepted pointer coordinates",
            );
            let (_, _, newest_bytes) = cursor_framebuffer_bytes(&fixture, newest_framebuffer);
            assert_eq!(
                newest_bytes[..4],
                [0xe0, 0x30, 0x10, 0xff],
                "the newest cursor generation {newest_version} survives older retirement",
            );
            fixture.retire_output(0, 4);
        }
        assert_eq!(
            first_version,
            fixture.backend.cursor_records[&first].version
        );
        fixture
            .backend
            .disable_output()
            .expect("Backend disables and drains the cursor output");
        fixture.assert_cursor_quiescent();
        let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        drop(fixture);
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: clear a newer cursor generation when the older scene frame retires; the latest FB_ID and position assertions fail.
fn atomic_cursor_only_newer_generation_queued_before_scene_retirement_survives() {
    use crate::{
        drm::atomic::AtomicCommitKind,
        kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent},
    };
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build scene retirement fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let root = fixture.backend.core.window_id;
    let first = create_solid_argb_cursor(&mut fixture, 16, [0x20, 0x40, 0xe0, 0xff]);
    Backend::define_cursor(&mut fixture.backend, None, root, first)
        .expect("define first cursor through Backend");
    let first_position = (120, 90);
    Backend::warp_pointer_root(
        &mut fixture.backend,
        &mut fixture.state,
        first_position.0,
        first_position.1,
    );
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("accept cursor-bearing scene frame");

    let first_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted first scene cursor receipt");
    assert_eq!(first_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(
        first_receipt.primary_cursor,
        CursorPrimaryContent::Cursorless
    );
    assert_eq!(
        first_receipt.frame,
        Some(crate::kms::render::scene::CursorFrameId(1))
    );
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [AtomicCommitKind::SceneWithCursor],
        "the first cursor state is accepted with exactly one scene frame",
    );
    fixture.retire_output(0, 1);
    assert!(!fixture.backend.scene.has_pending_page_flip(0));

    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("accept a stable-Hardware scene frame before queuing newer intent");
    let scene_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted stable-Hardware scene receipt");
    assert_eq!(scene_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(
        scene_receipt.primary_cursor,
        CursorPrimaryContent::Cursorless
    );
    assert_ne!(scene_receipt.frame, first_receipt.frame);
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [AtomicCommitKind::SceneWithCursor, AtomicCommitKind::Scene],
        "the stable-Hardware primary frame carries no cursor plane update",
    );

    let newest = create_solid_argb_cursor(&mut fixture, 16, [0xe0, 0x30, 0x10, 0xff]);
    let newest_version = fixture.backend.cursor_records[&newest].version;
    Backend::define_cursor(&mut fixture.backend, None, root, newest)
        .expect("queue the newest cursor image before scene retirement");
    let newest_position = (230, 170);
    Backend::warp_pointer_root(
        &mut fixture.backend,
        &mut fixture.state,
        newest_position.0,
        newest_position.1,
    );
    Backend::maybe_composite(&mut fixture.backend)
        .expect("queue the newest intent while the scene frame remains in flight");
    assert_eq!(fixture.backend.effective_cursor_xid, Some(newest));
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(scene_receipt),
        "the pending stable-Hardware scene still owns only its accepted intermediate state",
    );
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        2,
        "newer cursor intent waits behind the accepted scene frame",
    );
    let newest_intent = fixture
        .backend
        .platform
        .for_tests_cursor_show_intent(0)
        .expect("newest intent remains pending until the old scene event");
    assert_eq!(newest_intent.0.version(), newest_version);
    assert_eq!(
        (newest_intent.2, newest_intent.3),
        (
            newest_position.0 - fixture.backend.platform.outputs[0].x,
            newest_position.1 - fixture.backend.platform.outputs[0].y,
        ),
    );

    fixture.retire_output(0, 2);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("run the composition tail after scene retirement");
    Backend::before_block(&mut fixture.backend);
    let records = fixture.recorders.atomic.records();
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [
            AtomicCommitKind::SceneWithCursor,
            AtomicCommitKind::Scene,
            AtomicCommitKind::CursorOnly
        ],
        "scene retirement releases its event, then submits only the newest cursor update",
    );
    let newest_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("newest cursor-only acceptance publishes a receipt");
    assert_eq!(newest_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(
        newest_receipt.primary_cursor,
        CursorPrimaryContent::Cursorless
    );
    assert_eq!(newest_receipt.frame, None);
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));

    let properties = cursor_plane_for_output(&fixture, 0)
        .properties_for(fixture.backend.platform.outputs[0].output.crtc)
        .expect("cursor plane properties");
    let newest_commit = records.last().expect("newest cursor-only commit");
    let uploaded = atomic_cursor_framebuffer(newest_commit, properties)
        .expect("post-retirement commit binds the newest image");
    assert_eq!(
        atomic_cursor_position(newest_commit, properties),
        Some((
            newest_position.0 - fixture.backend.platform.outputs[0].x,
            newest_position.1 - fixture.backend.platform.outputs[0].y,
        )),
        "the post-retirement commit uses the newest live position",
    );
    let (_, _, newest_bytes) = cursor_framebuffer_bytes(&fixture, uploaded);
    assert_eq!(newest_bytes[..4], [0xe0, 0x30, 0x10, 0xff]);
    assert_eq!(
        fixture
            .backend
            .platform
            .cursor_plane_uploaded_version_for_output(0),
        Some(newest_version),
    );

    fixture.retire_output(0, 3);
    fixture
        .backend
        .disable_output()
        .expect("Backend disables and drains the cursor output");
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [
            AtomicCommitKind::SceneWithCursor,
            AtomicCommitKind::Scene,
            AtomicCommitKind::CursorOnly,
            AtomicCommitKind::Scene,
        ],
        "the scene retirement scenario has an exact cleanup commit sequence",
    );
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: clear a newer cursor generation when the older direct frame retires; the latest FB_ID and position assertions fail.
fn atomic_cursor_only_newer_generation_queued_before_direct_retirement_survives() {
    use crate::{
        drm::atomic::AtomicCommitKind,
        kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent},
    };
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build direct retirement fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let setup = super::round_a::setup_cursor_only_direct_present(&mut fixture);

    let root = fixture.backend.core.window_id;
    let first = create_solid_argb_cursor(&mut fixture, 16, [0x20, 0x40, 0xe0, 0xff]);
    Backend::define_cursor(&mut fixture.backend, None, root, first)
        .expect("define first cursor through Backend");
    let first_position = (120, 90);
    Backend::warp_pointer_root(
        &mut fixture.backend,
        &mut fixture.state,
        first_position.0,
        first_position.1,
    );
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("accept cursor-bearing scene frame");
    let first_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted first scene cursor receipt");
    assert_eq!(first_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(
        first_receipt.primary_cursor,
        CursorPrimaryContent::Cursorless
    );
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    fixture.retire_output(0, 1);
    assert!(fixture.backend.scene.direct_scanout_primary_ready());

    let mut direct_accepted = false;
    for attempt in 0..16_u64 {
        let (candidate, event) = super::round_a::direct_present_for_id(&setup, 31_000 + attempt);
        Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
        if Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit direct frame through Backend")
        {
            direct_accepted = true;
            break;
        }
        if fixture.backend.scene.has_pending_page_flip(0) {
            fixture.retire_output(0, 10 + u32::try_from(attempt).expect("event fits"));
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle direct probe fallback through Backend");
        }
    }
    assert!(
        direct_accepted,
        "Backend Present reaches held direct scanout"
    );
    assert!(fixture.backend.pstate.scanout_m2.pending.is_some());
    let records_before_newest = fixture.recorders.atomic.records();
    assert_eq!(
        records_before_newest
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [AtomicCommitKind::SceneWithCursor, AtomicCommitKind::Scene],
        "the accepted direct frame follows the cursor-bearing scene frame",
    );
    assert_eq!(records_before_newest[1].events.len(), 1);
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(first_receipt),
        "direct primary acceptance leaves cursor ownership at the prior accepted state",
    );

    let newest = create_solid_argb_cursor(&mut fixture, 16, [0xe0, 0x30, 0x10, 0xff]);
    let newest_version = fixture.backend.cursor_records[&newest].version;
    Backend::define_cursor(&mut fixture.backend, None, root, newest)
        .expect("queue the newest cursor image before direct retirement");
    let newest_position = (230, 170);
    Backend::warp_pointer_root(
        &mut fixture.backend,
        &mut fixture.state,
        newest_position.0,
        newest_position.1,
    );
    Backend::maybe_composite(&mut fixture.backend)
        .expect("hold cursor generation behind the in-flight direct frame");
    assert_eq!(fixture.backend.effective_cursor_xid, Some(newest));
    assert_eq!(
        fixture.recorders.atomic.records(),
        records_before_newest,
        "newer cursor intent does not extend the in-flight direct commit",
    );
    let newest_intent = fixture
        .backend
        .platform
        .for_tests_cursor_show_intent(0)
        .expect("newest intent remains pending until the old direct event");
    assert_eq!(newest_intent.0.version(), newest_version);
    assert_eq!(
        (newest_intent.2, newest_intent.3),
        (
            newest_position.0 - fixture.backend.platform.outputs[0].x,
            newest_position.1 - fixture.backend.platform.outputs[0].y,
        ),
    );

    fixture.retire_output(0, 20);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("run the composition tail after direct retirement");
    Backend::before_block(&mut fixture.backend);
    let records = fixture.recorders.atomic.records();
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [
            AtomicCommitKind::SceneWithCursor,
            AtomicCommitKind::Scene,
            AtomicCommitKind::CursorOnly,
        ],
        "direct retirement releases its event, then submits only the newest cursor update",
    );
    assert!(fixture.backend.pstate.scanout_m2.active());
    let newest_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("newest cursor-only acceptance publishes a receipt");
    assert_eq!(newest_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(
        newest_receipt.primary_cursor,
        CursorPrimaryContent::Cursorless
    );
    assert_eq!(newest_receipt.frame, None);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));

    let properties = cursor_plane_for_output(&fixture, 0)
        .properties_for(fixture.backend.platform.outputs[0].output.crtc)
        .expect("cursor plane properties");
    let newest_commit = records.last().expect("newest cursor-only commit");
    let uploaded = atomic_cursor_framebuffer(newest_commit, properties)
        .expect("post-retirement commit binds the newest image");
    assert_eq!(
        atomic_cursor_position(newest_commit, properties),
        Some((
            newest_position.0 - fixture.backend.platform.outputs[0].x,
            newest_position.1 - fixture.backend.platform.outputs[0].y,
        )),
        "the post-retirement commit uses the newest live position",
    );
    let (_, _, newest_bytes) = cursor_framebuffer_bytes(&fixture, uploaded);
    assert_eq!(newest_bytes[..4], [0xe0, 0x30, 0x10, 0xff]);
    assert_eq!(
        fixture
            .backend
            .platform
            .cursor_plane_uploaded_version_for_output(0),
        Some(newest_version),
    );

    fixture.retire_output(0, 21);
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("Backend disables direct scanout");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [
            AtomicCommitKind::SceneWithCursor,
            AtomicCommitKind::Scene,
            AtomicCommitKind::CursorOnly,
            AtomicCommitKind::Scene,
        ],
        "the direct retirement scenario has an exact cleanup commit sequence",
    );
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

fn atomic_cursor_position(
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
) -> Option<(i32, i32)> {
    let x = record.properties.iter().find_map(|update| {
        (update.object == properties.plane.into() && update.property == properties.crtc_x)
            .then_some(update.value as i64 as i32)
    })?;
    let y = record.properties.iter().find_map(|update| {
        (update.object == properties.plane.into() && update.property == properties.crtc_y)
            .then_some(update.value as i64 as i32)
    })?;
    Some((x, y))
}

fn atomic_cursor_framebuffer(
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
) -> Option<u32> {
    record.properties.iter().find_map(|update| {
        (update.object == properties.plane.into() && update.property == properties.fb_id)
            .then(|| u32::try_from(update.value).expect("cursor FB_ID fits u32"))
    })
}

fn wait_for_copied_job_for_crtc(
    fixture: &mut AtomicCursorFixture,
    crtc: ::drm::control::crtc::Handle,
) -> (u64, usize, crate::kms::render::scene::CursorFrameId) {
    use yserver_core::backend::Backend;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some((job, _, bo_idx, frame)) = fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_associations()
            .into_iter()
            .find(|(_, key, _, _)| key.crtc == crtc)
        {
            return (job, bo_idx, frame);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "output {crtc:?} did not produce its next copied frame",
        );
        Backend::maybe_composite(&mut fixture.backend)
            .expect("advance the copied frame through Backend");
        std::thread::yield_now();
    }
}

fn cursor_framebuffer_bytes(
    fixture: &AtomicCursorFixture,
    framebuffer: u32,
) -> (i32, u32, Vec<u8>) {
    use crate::kms::cursor_plane::ops::CursorBufferRecord;

    let (device_fd, handle) = fixture
        .recorders
        .cursor_buffer_ledger
        .records()
        .into_iter()
        .find_map(|record| match record {
            CursorBufferRecord::AddFb {
                device_fd,
                handle,
                fb,
                ..
            } if fb == framebuffer => Some((device_fd, handle)),
            _ => None,
        })
        .expect("cursor framebuffer has an AddFB2 record");
    let bytes = fixture
        .recorders
        .cursor_buffer_ledger
        .mapped_bytes(device_fd, handle)
        .expect("cursor framebuffer mapping remains live");
    (device_fd, handle, bytes)
}

fn assert_primary_framebuffer(
    fixture: &AtomicCursorFixture,
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    output_idx: usize,
    framebuffer: u32,
) {
    let output = &fixture.backend.platform.outputs[output_idx].output;
    assert!(
        record.properties.iter().any(|update| {
            update.object == output.plane.into()
                && update.property == output.plane_fb_id_prop
                && update.value == u64::from(framebuffer)
        }),
        "primary FB {framebuffer} absent from {:?}; plane={:?} property={:?}",
        record,
        output.plane,
        output.plane_fb_id_prop
    );
}

fn assert_cursor_framebuffer(
    fixture: &AtomicCursorFixture,
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    output_idx: usize,
    framebuffer: u32,
) {
    let output = &fixture.backend.platform.outputs[output_idx];
    let cursor = fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(output.output.crtc))
        .expect("output has cursor plane properties");
    assert!(record.properties.iter().any(|update| {
        update.object == cursor.plane.into()
            && update.property == cursor.fb_id
            && update.value == u64::from(framebuffer)
    }));
}

fn cursor_plane_for_output(
    fixture: &AtomicCursorFixture,
    output_idx: usize,
) -> &crate::kms::cursor_plane::CursorPlane {
    let output = &fixture.backend.platform.outputs[output_idx];
    fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .expect("output owns a cursor plane")
}

fn wait_for_held_scanout_job(fixture: &mut AtomicCursorFixture, job_id: u64) {
    use yserver_core::backend::Backend;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !fixture
        .recorders
        .render_completion_readiness
        .observed_ready_jobs()
        .contains(&job_id)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "real Vulkan completion job {job_id} did not signal before timeout",
        );
        assert!(
            fixture
                .backend
                .platform
                .for_tests_pending_scanout_render_completion_ids()
                .contains(&job_id)
        );
        Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
        std::thread::yield_now();
    }
}

fn create_solid_argb_cursor(
    fixture: &mut AtomicCursorFixture,
    dimension: u16,
    bgra: [u8; 4],
) -> u32 {
    use yserver_core::backend::{AnyHandle, Backend};

    let source = Backend::create_pixmap(&mut fixture.backend, None, 32, dimension, dimension)
        .expect("create solid ARGB cursor source");
    let pixels = bgra.repeat(usize::from(dimension) * usize::from(dimension));
    Backend::put_image(
        &mut fixture.backend,
        None,
        source.as_raw(),
        32,
        dimension,
        dimension,
        0,
        0,
        &pixels,
    )
    .expect("upload solid ARGB cursor pixels through Backend");
    let picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(source),
        0,
        0,
        &[],
    )
    .expect("create cursor source picture")
    .expect("source picture handle");
    Backend::render_create_cursor(&mut fixture.backend, None, picture, 0, 0)
        .expect("create ARGB cursor through Backend")
        .expect("cursor handle")
        .as_raw()
}

fn read_copied_pixel(fixture: &mut AtomicCursorFixture, position: (i32, i32)) -> [u8; 4] {
    let bytes = crate::kms::render::backend::read_scanout_region(
        &mut fixture.backend,
        ash::vk::Rect2D {
            offset: ash::vk::Offset2D {
                x: position.0,
                y: position.1,
            },
            extent: ash::vk::Extent2D {
                width: 1,
                height: 1,
            },
        },
        crate::kms::render::backend::ScanoutReadSelection::PermissiveDump,
    )
    .expect("read accepted copied scanout pixel");
    bytes.try_into().expect("one BGRA pixel")
}
