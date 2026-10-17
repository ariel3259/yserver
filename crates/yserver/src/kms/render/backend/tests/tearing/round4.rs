use super::*;

fn assert_cursorless_async_records(
    backend: &KmsBackend,
    records: &[crate::drm::atomic::tests_support::AtomicCommitRecord],
    cursor_plane: ::drm::control::plane::Handle,
) {
    for record in records
        .iter()
        .filter(|record| record.kind == AtomicCommitKind::Async)
    {
        assert!(
            record
                .flags
                .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_ASYNC)
        );
        assert!(
            record
                .flags
                .contains(::drm::control::AtomicCommitFlags::NONBLOCK)
        );
        assert!(!record.properties.iter().any(|update| {
            update.object == cursor_plane.into()
                || update.object == backend.platform.outputs[0].output.crtc.into()
                    && Some(update.property)
                        == backend.platform.outputs[0].output.crtc_out_fence_ptr_prop
        }));
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: append cursor FB_ID to Async, settle a filtered delta, or reattach on Vsync fallback.
fn atomic_cursor_only_c1_async_filters_before_cursor_preparation() {
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x7a61;
    let Some((mut backend, recorder, mut state)) = live_tearing_cursor_backend(WINDOW, 22_000)
    else {
        return;
    };
    let (_show, _hide) = install_cursor_plane_with_successful_show(&mut backend);
    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let crtc = backend.platform.outputs[0].output.crtc;
    let properties = backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .expect("installed cursor plane")
        .properties_for(crtc)
        .expect("cursor plane properties");

    let first = create_bitmap_cursor_via_backend(&mut backend, true, true);
    let root = backend.core.window_id;
    Backend::define_cursor(&mut backend, None, root, first.as_raw())
        .expect("queue cursor shape through Backend while tearing is On");
    Backend::warp_pointer_root(&mut backend, &mut state, 300, 160);
    damage_root(&mut backend);
    Backend::maybe_composite(&mut backend).expect("submit shared composed Async frame");
    assert!(backend.scene.has_pending_page_flips());
    assert_eq!(recorder.records().len(), 1);
    assert_eq!(recorder.records()[0].kind, AtomicCommitKind::Async);
    assert_cursorless_async_records(&backend, &recorder.records(), properties.plane);
    assert!(!backend.platform.cursor_plane_visible_for_output(0));
    let state_before = backend.platform.devices[0]
        .cursor
        .crtc_states
        .get(&crtc)
        .cloned()
        .unwrap_or_default();
    assert!(state_before.applied.is_none());
    assert!(state_before.in_flight.is_none());
    assert!(state_before.intent.is_none());

    let newer = create_bitmap_cursor_via_backend(&mut backend, false, true);
    Backend::define_cursor(&mut backend, None, root, newer.as_raw())
        .expect("queue newer cursor image and hotspot while Async is pending");
    Backend::warp_pointer_root(&mut backend, &mut state, 325, 175);
    Backend::maybe_composite(&mut backend).expect("keep image and motion behind pending Async");
    assert_eq!(recorder.records().len(), 1);
    assert_eq!(
        backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&crtc)
            .cloned()
            .unwrap_or_default(),
        state_before,
        "filtered Async motion and image changes do not publish or settle plane state",
    );
    retire_composed_frame(&mut backend, &mut state, 70);
    assert!(backend.pstate.tearing.is_on());
    assert!(
        backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&crtc)
            .is_none_or(|state| state.in_flight.is_none())
    );

    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    damage_root(&mut backend);
    Backend::maybe_composite(&mut backend).expect("Async EINVAL retries the same frame at Vsync");
    let fallback_records = recorder.records();
    assert_eq!(fallback_records.len(), 3);
    assert_eq!(fallback_records[1].kind, AtomicCommitKind::Async);
    assert_eq!(fallback_records[2].kind, AtomicCommitKind::Scene);
    assert_cursorless_async_records(&backend, &fallback_records, properties.plane);
    assert!(
        fallback_records[2]
            .properties
            .iter()
            .all(|update| update.object != properties.plane.into())
    );
    assert!(backend.pstate.tearing.is_on());
    assert!(!backend.platform.cursor_plane_visible_for_output(0));
    assert!(
        backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&crtc)
            .is_none_or(|state| state.applied.is_none() && state.in_flight.is_none())
    );
    assert!(backend.scene.has_pending_page_flips());
    retire_composed_frame(&mut backend, &mut state, 71);

    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    damage_root(&mut backend);
    Backend::maybe_composite(&mut backend).expect("Async EBUSY leaves no cursor event obligation");
    let after_busy = recorder.records();
    assert_eq!(after_busy.len(), 4);
    assert_eq!(after_busy[3].kind, AtomicCommitKind::Async);
    assert_cursorless_async_records(&backend, &after_busy, properties.plane);
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.platform.cursor_atomic_event_pending_for_output(0));
    assert!(backend.pstate.tearing.is_on());
    assert_eq!(
        after_busy
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            AtomicCommitKind::Async,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
        ],
    );
    backend.platform.for_tests_accept_output_power_commits();
    backend
        .disable_output()
        .expect("Backend shutdown exits tearing and detaches any accepted state");
    assert_tearing_off(&backend);
    assert_direct_queue_drained(&backend);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: allow a hidden-direct Async to bind cursor state or settle EINVAL's rejected cursor delta.
fn atomic_cursor_only_c1_hidden_direct_async_fallback_stays_cursorless() {
    use yserver_core::{backend::Backend, server::ServerState};

    const WINDOW: u32 = 0x7a62;
    let Some(mut backend) = live_scene_backend() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    qualify_test_direct_async(&mut backend, &recorder, WINDOW, 23_000);
    let mut state = ServerState::new();
    let (_show, _hide) = install_cursor_plane_with_successful_show(&mut backend);
    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let crtc = backend.platform.outputs[0].output.crtc;
    let properties = backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .expect("installed cursor plane")
        .properties_for(crtc)
        .expect("cursor plane properties");
    let (width, height) = backend.platform.fb_dimensions();
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    assert!(
        try_direct_present_for_window(
            &mut backend,
            23_100,
            WINDOW,
            width,
            height,
            yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
        )
        .is_some()
    );
    assert!(backend.pstate.tearing.is_on());
    let records = recorder.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].kind, AtomicCommitKind::Async);
    assert_eq!(
        records[0].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            kind: std::io::ErrorKind::InvalidInput,
            raw_os_error: Some(libc::EINVAL),
        },
    );
    assert_eq!(records[1].kind, AtomicCommitKind::Scene);
    assert_cursorless_async_records(&backend, &records, properties.plane);
    assert!(
        records
            .iter()
            .filter(|record| record.kind == AtomicCommitKind::Scene)
            .all(|record| record
                .properties
                .iter()
                .all(|update| { update.object != properties.plane.into() }))
    );
    assert!(
        records
            .iter()
            .all(|record| record.kind != AtomicCommitKind::CursorOnly)
    );

    let first = create_bitmap_cursor_via_backend(&mut backend, true, true);
    let root = backend.core.window_id;
    Backend::define_cursor(&mut backend, None, root, first.as_raw())
        .expect("define visible cursor through Backend while direct fallback is pending");
    Backend::set_cursor_hidden(&mut backend, false);
    let replacement = create_bitmap_cursor_via_backend(&mut backend, false, true);
    Backend::define_cursor(&mut backend, None, root, replacement.as_raw())
        .expect("queue image and hotspot change while direct flip is pending");
    Backend::warp_pointer_root(&mut backend, &mut state, 340, 180);
    Backend::maybe_composite(&mut backend).expect("cursor handoff waits for direct retirement");
    assert_eq!(recorder.records().len(), 2);
    assert!(backend.pstate.scanout_m2.pending.is_some());
    retire_direct_present(&mut backend, &mut state, 99);
    assert!(recorder.records().iter().all(|record| {
        record
            .properties
            .iter()
            .all(|update| update.object != properties.plane.into())
    }));
    assert!(!backend.platform.cursor_plane_visible_for_output(0));
    assert!(
        backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&crtc)
            .is_none_or(|state| state.applied.is_none() && state.in_flight.is_none())
    );

    if backend.pstate.scanout_m2.pending.is_some() {
        retire_direct_present(&mut backend, &mut state, 99);
    }
    if backend.scene.has_pending_page_flips() {
        retire_composed_frame(&mut backend, &mut state, 100);
    }
    Backend::set_cursor_hidden(&mut backend, true);
    backend.platform.for_tests_accept_output_power_commits();
    backend
        .disable_output()
        .expect("Backend shutdown exits hidden direct scanout");
    assert_tearing_off(&backend);
    assert_direct_queue_drained(&backend);
}
