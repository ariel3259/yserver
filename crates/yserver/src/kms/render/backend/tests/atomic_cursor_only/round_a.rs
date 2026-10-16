use super::support::AtomicCursorFixture;

fn clone_drm_output(output: &crate::platform::drm::Output) -> crate::platform::drm::Output {
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

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: bypass the injected atomic executor; the accepted commit sequence must fail.
// Mutation: omit legacy position recording; the post-retirement bind/move sequence must fail.
fn atomic_cursor_only_trace_covers_backend_motion_and_retirement() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    let recorders = std::rc::Rc::clone(&fixture.recorders);
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
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

    Backend::maybe_composite(&mut fixture.backend).expect("submit cursor show frame");
    assert!(
        fixture.backend.scene.has_pending_page_flips(),
        "Backend composition must own the cursor show until its event retires",
    );
    let first = recorders.atomic.records();
    assert_eq!(
        first.len(),
        1,
        "show frame must use the selected atomic executor"
    );
    assert_eq!(
        first[0].kind,
        crate::drm::atomic::AtomicCommitKind::Scene,
        "the first scene frame establishes cursor ownership",
    );
    assert_eq!(
        first[0].result,
        crate::drm::atomic::tests_support::ScriptedResult::Ok,
        "the scene request was accepted",
    );
    assert_eq!(first[0].events.len(), 1);
    assert!(
        recorders.legacy_cursor.records().is_empty(),
        "legacy bind happens only after the accepted show frame retires",
    );

    fixture.retire_output(0, 1);
    let legacy_after_show = recorders.legacy_cursor.records();
    assert_eq!(legacy_after_show.len(), 2);
    assert_eq!(
        legacy_after_show
            .iter()
            .map(|record| record.operation)
            .collect::<Vec<_>>(),
        vec![
            crate::kms::cursor_plane::ops::LegacyCursorOperation::Bind,
            crate::kms::cursor_plane::ops::LegacyCursorOperation::Move,
        ],
    );
    assert!(legacy_after_show[0].framebuffer.is_some());
    assert!(legacy_after_show[1].position.is_some());

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 180, 140);
    let latest = fixture
        .backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert_eq!(
        latest.len(),
        1,
        "Backend motion must retain one cursor intent"
    );
    Backend::maybe_composite(&mut fixture.backend).expect("submit cursor-only motion");
    let after_motion = recorders.atomic.records();
    assert_eq!(
        after_motion
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
        ],
        "accepted scene and motion requests must stay ordered",
    );
    assert!(
        after_motion.iter().all(|record| {
            record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
        })
    );
    for (property, value) in [
        (latest[0].crtc_x, i64::from(latest[0].x) as u64),
        (latest[0].crtc_y, i64::from(latest[0].y) as u64),
    ] {
        assert!(after_motion[1].properties.iter().any(|update| {
            update.object == latest[0].plane.into()
                && update.property == property
                && update.value == value
        }));
    }
    assert_eq!(after_motion[1].events.len(), 1);
    assert!(
        fixture
            .backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(latest[0].device_key, latest[0].crtc,)
    );
    fixture.retire_output(0, 2);
    fixture.assert_cursor_quiescent();

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("submit cursor hide frame");
    assert!(fixture.backend.scene.has_pending_page_flips());
    fixture.retire_output(0, 3);
    fixture.assert_cursor_quiescent();
    assert_eq!(
        recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
        "the accepted commit sequence is show, motion, hide",
    );
    assert_eq!(
        recorders
            .legacy_cursor
            .records()
            .last()
            .map(|record| record.operation),
        Some(crate::kms::cursor_plane::ops::LegacyCursorOperation::Hide),
    );

    let ledger = std::rc::Rc::clone(&recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(
        ledger.end_state(),
        (0, 0, 0),
        "cursor resources must be released"
    );
}

#[test]
// Mutation: ignore the selected hotplug source; the queued edge must fail to arm Backend's deadline.
fn atomic_cursor_only_hotplug_source_arms_backend_debounce() {
    use std::{cell::Cell, time::Duration};
    use yserver_core::backend::Backend;

    let mut fixture = AtomicCursorFixture::without_scene().expect("build backend fixture");
    let base = std::time::Instant::now();
    let elapsed_ms = std::rc::Rc::new(Cell::new(0_u64));
    let clock_ms = std::rc::Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(std::rc::Rc::new(move || {
            base + Duration::from_millis(clock_ms.get())
        }));
    assert!(Backend::next_wakeup(&fixture.backend).is_none());

    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    assert!(Backend::next_wakeup(&fixture.backend).is_none());

    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue pollable recording hotplug edge");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    let deadline = Backend::next_wakeup(&fixture.backend).expect("Backend advertises debounce");
    assert_eq!(deadline, base + Duration::from_millis(150));

    fixture.assert_cursor_quiescent();
    assert!(
        fixture.recorders.atomic.records().is_empty(),
        "an empty synthetic connector rescan must not submit atomic commits",
    );

    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: select production mode-blob operations in KmsServices::for_tests; the service identity assertion must fail.
fn atomic_cursor_only_mode_blob_lifetime_uses_shared_modeset() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build backend fixture") else {
        return;
    };
    fixture
        .backend
        .initialize_provider_output_sources()
        .expect("automatically attach the selected renderer to the synthetic KMS sink");
    let selected: std::rc::Rc<dyn crate::drm::io::ModeBlobIo> =
        std::rc::Rc::clone(&fixture.backend.platform.services.mode_blobs);
    let recording: std::rc::Rc<dyn crate::drm::io::ModeBlobIo> =
        fixture.recorders.mode_blobs.clone();
    assert!(
        std::rc::Rc::ptr_eq(&selected, &recording),
        "for_tests must select the recording mode-blob operations",
    );
    assert_eq!(
        crate::drm::io::ModeBlobIo::for_tests_record_count(fixture.recorders.mode_blobs.as_ref()),
        Some(0),
        "for_tests must select the recording mode-blob operations",
    );

    let output = clone_drm_output(&fixture.backend.platform.outputs[0].output);
    let connector = output.connector_name.clone();
    let mode = yserver_core::backend::ModeSpec {
        width: output.picked.width,
        height: output.picked.height,
        vrefresh: output.picked.vrefresh,
    };
    let output_id = fixture
        .backend
        .randr_outputs_and_modes()
        .0
        .into_iter()
        .find(|candidate| candidate.name == connector)
        .expect("synthetic RANDR output")
        .output_id;
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture.backend.pstate.crtc_config_discovery_override = Some(output);

    let attempt = Backend::apply_crtc_config(
        &mut fixture.backend,
        output_id,
        &connector,
        Some(mode),
        13,
        0,
    );
    let error = attempt.expect_err("synthetic /dev/null KMS must reject DRM property lookup");
    assert_eq!(
        error.raw_os_error(),
        Some(libc::ENOTTY),
        "RANDR reaches the shared modeset entry, whose direct property lookup is the deepest operation available on the fixture fd",
    );
    assert_eq!(fixture.recorders.mode_blobs.live_count(), 0);
    assert!(fixture.recorders.mode_blobs.records().is_empty());
    assert!(
        fixture.recorders.atomic.records().is_empty(),
        "the fake KMS fd must stop at property lookup before atomic submission",
    );
    fixture.assert_cursor_quiescent();

    let blobs = std::rc::Rc::clone(&fixture.recorders.mode_blobs);
    let cursor_resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(blobs.live_count(), 0);
    assert_eq!(cursor_resources.end_state(), (0, 0, 0));
}

#[test]
// Mutation: drain the shared recording queue without matching the DRM device; the colliding CRTC event must remain queued.
fn atomic_cursor_only_flip_events_are_scoped_to_the_ready_device() {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    use yserver_core::backend::Backend;

    let mut fixture = AtomicCursorFixture::without_scene().expect("build backend fixture");
    let primary = std::rc::Rc::clone(&fixture.backend.platform.devices[0].device);
    let second = std::rc::Rc::new(
        crate::drm::Device::for_tests().expect("open second synthetic DRM device"),
    );
    let second_key = crate::platform::drm::DrmDeviceKey { major: 0, minor: 1 };
    let source = std::rc::Rc::clone(&fixture.recorders.page_flip_events);
    source.register_device(&second);
    fixture
        .backend
        .platform
        .devices
        .push(crate::kms::render::platform::KmsDevice {
            key: second_key,
            device: std::rc::Rc::clone(&second),
            async_page_flip: false,
            atomic_commit_sink: std::rc::Rc::new(crate::drm::atomic::AtomicCommitSink::new(
                std::rc::Rc::clone(&fixture.backend.platform.services.atomic),
            )),
            page_flip_event_source: source.clone(),
            cursor_buffer_io: std::rc::Rc::clone(&fixture.backend.platform.services.cursor_buffers),
            legacy_cursor_io: std::rc::Rc::clone(&fixture.backend.platform.services.legacy_cursor),
            test_page_flip_event_source: Some(source.clone()),
            cursor: crate::kms::render::platform::KmsCursorState::new(),
        });

    let colliding_crtc = ::drm::control::from_u32(1).expect("synthetic CRTC handle");
    source.queue_flip_for_device(&primary, colliding_crtc, 7);
    source.queue_flip_for_device(&second, colliding_crtc, 8);
    let primary_fd = primary.as_fd().as_raw_fd();
    Backend::on_page_flip_ready(&mut fixture.backend, &mut fixture.state, primary_fd);

    assert_eq!(source.queued_flip_count_for_device(&primary), 0);
    assert_eq!(
        source.queued_flip_count_for_device(&second),
        1,
        "draining one DRM fd cannot consume the other card's colliding CRTC event",
    );
    Backend::on_page_flip_ready(
        &mut fixture.backend,
        &mut fixture.state,
        second.as_fd().as_raw_fd(),
    );
    assert_eq!(source.queued_flip_count(), 0);
    assert!(fixture.recorders.atomic.records().is_empty());
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert!(fixture.backend.pstate.scanout_m2.pending.is_none());
    assert!(fixture.backend.pstate.scanout_m2.current.is_none());
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_none());
    fixture.assert_cursor_quiescent();

    let resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}
