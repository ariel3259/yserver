use super::{round_a::damage_cursor_root, support::AtomicCursorFixture};

#[test]
// Mutation: return immediately on old-event drain failure; the queued PRESENT completion must still be forwarded.
fn atomic_cursor_only_shutdown_event_drain_failure_forwards_present_completion() {
    use yserver_core::backend::{Backend, CompletedPresentEvent, PresentWake};

    let backend = crate::kms::render::backend::KmsBackend::for_tests();
    let recorders = std::rc::Rc::clone(
        backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("test backend owns recording services"),
    );
    let mut fixture = AtomicCursorFixture {
        backend,
        recorders,
        state: yserver_core::server::ServerState::new(),
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let output = &fixture.backend.platform.outputs[0];
    let crtc = output.output.crtc;
    let root = fixture.backend.core.window_id;
    let device = std::rc::Rc::clone(&fixture.backend.platform.devices[0].device);
    fixture.recorders.page_flip_events.register_device(&device);

    Backend::enqueue_present_completion(
        &mut fixture.backend,
        CompletedPresentEvent {
            client_id: yserver_protocol::x11::ClientId(1),
            serial: 0xa505,
            host_xid: 0x5a05,
            dst_host_xid: root,
            options: 0,
            present_id: 0xa505,
            window_generation: 0,
            crtc_id: u32::from(crtc),
            crtc_epoch: 0,
            msc_offset: 0,
            completion_clock: None,
            wake: PresentWake::Pixmap { idle_fence_xid: 0 },
            completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
            emit_idle: true,
        },
        root,
    );
    assert_eq!(
        fixture.backend.pending_present_batches.len(),
        1,
        "Backend enqueue leaves one PRESENT batch for shutdown to forward",
    );

    fixture
        .recorders
        .page_flip_events
        .queue_flip_for_device(&device, crtc, 1);
    fixture
        .recorders
        .page_flip_events
        .fail_next_drain(libc::EIO);
    let error = fixture
        .backend
        .disable_output()
        .expect_err("injected old-event drain failure must propagate");
    assert_eq!(error.raw_os_error(), Some(libc::EIO));
    assert!(
        fixture.backend.platform.devices[0].cursor.plane.is_none(),
        "shutdown error handling does not create or publish cursor ownership",
    );
    assert!(fixture.backend.pending_present_batches.is_empty());
    assert_eq!(
        fixture.backend.pending_completed_events_on_shutdown.len(),
        1,
        "PRESENT completion is forwarded on the failed old-event drain path",
    );
    assert_eq!(
        fixture.backend.pending_completed_events_on_shutdown[0].serial,
        0xa505,
    );

    fixture.recorders.page_flip_events.queue_flip_for_device(
        &fixture.backend.platform.devices[0].device,
        crtc,
        1,
    );
    fixture
        .backend
        .disable_output()
        .expect("retry old-event drain and complete shutdown");
    let forwarded = fixture.backend.take_shutdown_present_events();
    assert_eq!(
        forwarded
            .iter()
            .map(|event| event.serial)
            .collect::<Vec<_>>(),
        vec![0xa505],
        "shutdown delivers the retained completion exactly once",
    );
    assert!(fixture.backend.pending_present_batches.is_empty());
    assert!(
        fixture
            .backend
            .pending_completed_events_on_shutdown
            .is_empty()
    );
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "both shutdown attempts use the accepted blocking disable path",
    );
    assert_eq!(
        fixture.recorders.cursor_buffer_ledger.end_state(),
        (0, 0, 0)
    );
    fixture.assert_cursor_quiescent();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: classify OutputScanout::Copied as Shared; the copied-route assertion must fail.
fn atomic_cursor_only_copied_scene_routes_to_sink_device_via_backend() {
    use crate::kms::render::scene::{OutputRoute, OutputSink};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build copied scene fixture") else {
        return;
    };
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
        .install_copied_recording_route(route)
        .expect("install lavapipe copied scene route");
    let layouts = fixture.backend.platform.scene_output_layouts();
    fixture.backend.scene =
        crate::kms::render::scene::SceneCompositor::new(&fixture.backend.platform, &layouts)
            .expect("rebuild scene with copied sink identity");
    assert_eq!(
        OutputSink::route(&fixture.backend.platform, 0),
        OutputRoute::Copied,
        "Backend scene output must enter the copied route",
    );
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit copied scene frame");

    let mut copied_commit = false;
    for frame in 1..8 {
        fixture.backend.platform.wait_idle_bounded();
        Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
        if fixture.backend.scene.has_pending_page_flips() {
            fixture.retire_output(0, frame);
        }
        if !sink_atomic.records().is_empty() {
            copied_commit = true;
            break;
        }
        Backend::maybe_composite(&mut fixture.backend).expect("continue copied scene submission");
    }
    assert!(
        copied_commit,
        "copied scene submission must use the KMS sink device's atomic recorder",
    );
    assert_eq!(
        sink_atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![crate::drm::atomic::AtomicCommitKind::Scene],
        "one copied scene flip is recorded on the sink device",
    );
    assert_eq!(sink_atomic.records()[0].events.len(), 1);
    assert!(fixture.recorders.atomic.records().is_empty());
    fixture
        .backend
        .disable_output()
        .expect("disable copied sink through Backend shutdown");
    fixture.assert_cursor_quiescent();
    assert!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count()
            == 0,
        "copied route render completions are drained",
    );
    assert_eq!(
        sink_atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![crate::drm::atomic::AtomicCommitKind::Scene],
        "copied scene uses the sink's atomic recorder",
    );
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![crate::drm::atomic::AtomicCommitKind::Scene],
        "Backend lifecycle disable is recorded once",
    );
    assert!(
        fixture.recorders.atomic.records()[0].events.is_empty(),
        "lifecycle disable does not leave a page-flip event obligation",
    );
    assert!(fixture.recorders.legacy_cursor.records().is_empty());
    let resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}
