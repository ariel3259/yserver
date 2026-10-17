use super::support::AtomicCursorFixture;

fn install_copied_recording_route(
    fixture: &mut AtomicCursorFixture,
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
    let legacy_cursor_io = std::rc::Rc::clone(&source_device.legacy_cursor_io);
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
            legacy_cursor_io,
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
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    fixture.backend.platform.outputs[0].key.device_key = sink_key;
    fixture.backend.platform.outputs[0].scanout_route = route;
    fixture.backend.pstate.crtc_key_by_id.insert(
        u32::from(crtc),
        fixture.backend.platform.outputs[0].key.clone(),
    );
    fixture.backend.refresh_present_crtc_clock_epochs();
    fixture
        .backend
        .platform
        .for_tests_initialize_cursor_planes_with_capabilities(Some(64), Some(64));
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
// Mutation: retain staged cursor plans when Backend cancels copied completions; cleanup assertions fail.
fn atomic_cursor_only_cancelled_copied_frame_publishes_no_receipt() {
    use crate::kms::render::scene::{CursorPlaneMode, OutputRoute, OutputSink};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build copied cancellation fixture")
    else {
        return;
    };
    let sink_atomic = install_copied_recording_route(&mut fixture);
    fixture
        .backend
        .platform
        .for_tests_hold_scanout_render_completions();
    assert_eq!(
        OutputSink::route(&fixture.backend.platform, 0),
        OutputRoute::Copied,
    );
    let cursor = create_oversized_cursor(&mut fixture, (2, 3));
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, cursor)
        .expect("define software cursor through Backend");
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

    fixture
        .backend
        .disable_output()
        .expect("Backend shutdown cancels the copied frame before completion dispatch");
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
        "Backend shutdown removes the canceled completion entry",
    );
    assert!(
        fixture.backend.platform.staged_cursor_frames.is_empty(),
        "Backend shutdown releases the staged frame metadata",
    );
    assert!(
        !fixture.backend.scene.has_pending_page_flips(),
        "no scene event obligation survives cancellation and shutdown",
    );
    fixture.assert_cursor_quiescent();
    assert_eq!(
        sink_atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [],
        "the canceled frame never reaches the copied KMS sink",
    );
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [crate::drm::atomic::AtomicCommitKind::Scene],
        "shutdown detaches the output once and leaves no live event",
    );
    assert!(fixture.recorders.atomic.records()[0].events.is_empty());
    assert!(fixture.recorders.legacy_cursor.records().is_empty());
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
        None,
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
        None,
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
    assert_eq!(f0.frame, Some(CursorFrameId(1)));
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
    assert_eq!(f1.frame, Some(CursorFrameId(3)));
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
        3,
        "F0, the rejection, and F1 have flip attempts"
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
        frame_fb(0),
        u64::from(f0_fb),
        "F0 receipt accompanies its rendered FB: {frame_records:?}"
    );
    assert_eq!(
        frame_fb(2),
        u64::from(f1_fb),
        "F1 receipt accompanies its rendered FB: {frame_records:?}"
    );
    assert_eq!(
        frame_records[0].result,
        crate::drm::atomic::tests_support::ScriptedResult::Ok
    );
    assert!(matches!(
        frame_records[1].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            raw_os_error: Some(5),
            ..
        }
    ));
    assert_eq!(
        frame_records[2].result,
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
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Sw);
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
// Mutation: omit pre-render reservation rollback or descriptor-slot release; retry cannot accept and teardown retains a lease.
fn atomic_cursor_only_shared_reservation_failure_releases_slots_and_retries() {
    use crate::kms::{cursor_plane::CursorLeaseBoundary, render::scene::CursorAcceptedMode};
    use yserver_core::backend::Backend;

    let Some(mut fixture) =
        AtomicCursorFixture::new_with_cursor_lease_failures(CursorLeaseBoundary::Reserve, 3)
            .expect("build scripted Shared reservation fixture")
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

    Backend::maybe_composite(&mut fixture.backend).expect("failed reservation is retryable");
    assert!(fixture.recorders.atomic.records().is_empty());
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("cursor plane")
            .sprite_pool_users_are_zero(),
        "a failed reservation leaves no prepared or accepted lease",
    );

    for _ in 0..3 {
        Backend::maybe_composite(&mut fixture.backend).expect("retry after reservation failure");
        if fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .is_some()
        {
            break;
        }
    }
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .is_some(),
        "Backend retry leaves the damaged shared frame pending"
    );
    let receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("retry publishes the accepted frame receipt");
    assert_eq!(receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(
        fixture.recorders.atomic.records()[0].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
    );
    fixture.retire_output(0, 1);

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit cursorless scene frame");
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
// Mutation: skip the degraded composed-unflip fallback after cursor reservation fails; its accepted per-output trace and teardown assertions fail.
fn atomic_cursor_only_composed_unflip_reservation_failure_rolls_back_before_fallback() {
    use yserver_core::backend::{AnyHandle, Backend};

    let Some(mut fixture) = AtomicCursorFixture::new_with_two_outputs_cursor_lease_failures(
        crate::kms::cursor_plane::CursorLeaseBoundary::Reserve,
        [2, 0],
    )
    .expect("build two-output reservation rollback fixture") else {
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

    // The new lease spans both CRTCs. The fixture script allows both initial
    // Shared attachments, then fails the direct sprite replacement and the
    // composed-unflip retry at the same real reservation boundary.
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
        .expect("define replacement cursor before unflip");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 801, 90);
    assert!(
        fixture
            .backend
            .platform
            .for_tests_cursor_show_intent(0)
            .is_some()
    );
    assert!(
        fixture
            .backend
            .platform
            .for_tests_cursor_show_intent(1)
            .is_some()
    );
    assert!(fixture.backend.pstate.scanout_m2.unflip_requested);
    Backend::unmap_subwindow(&mut fixture.backend, None, direct.window_xid)
        .expect("request composed unflip through Backend");
    assert!(fixture.backend.pstate.scanout_m2.unflip_requested);
    let before_unflip = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("reservation failure falls back to per-output scene commits");

    assert!(fixture.backend.pstate.scanout_m2.degraded_composed_unflip);
    assert_eq!(
        fixture.backend.pstate.scanout_m2.unflip_awaiting_outputs,
        [0, 1].into(),
        "fallback waits for both composed output flips",
    );
    assert!(fixture.backend.scene.has_pending_page_flips());
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(1));
    assert_eq!(
        fixture.recorders.atomic.records()[before_unflip..]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
        "failed grouped reservation submits only the two Backend scene fallbacks",
    );
    fixture.retire_output(0, 61);
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
