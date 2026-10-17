use super::*;

fn create_argb_cursor_with_hotspot(
    backend: &mut KmsBackend,
    pixel: [u8; 4],
    hot_x: u16,
    hot_y: u16,
) -> yserver_core::backend::CursorHandle {
    use yserver_core::backend::{AnyHandle, Backend};

    let source = Backend::create_pixmap(backend, None, 32, 4, 4).expect("create cursor pixmap");
    let bytes = pixel.repeat(16);
    Backend::put_image(backend, None, source.as_raw(), 32, 4, 4, 0, 0, &bytes)
        .expect("write cursor pixels");
    let picture =
        Backend::render_create_picture(backend, None, AnyHandle::Pixmap(source), 0, 0, &[])
            .expect("create RENDER picture")
            .expect("picture handle");
    Backend::render_create_cursor(backend, None, picture, hot_x, hot_y)
        .expect("create RENDER cursor")
        .expect("cursor handle")
}

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
// Mutations: append FB_ID to Async, settle a filtered delta, reattach on Vsync fallback, or classify Async EINVAL as cursor failure.
fn atomic_cursor_only_c1_async_entry_detaches_an_accepted_hw_binding() {
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x7a60;
    let Some(mut backend) = live_scene_backend() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let (_show, _hide) = install_cursor_plane_with_successful_show(&mut backend);
    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let mut state = yserver_core::server::ServerState::new();
    let cursor = create_bitmap_cursor_via_backend(&mut backend, true, true);
    let root = backend.core.window_id;
    Backend::define_cursor(&mut backend, None, root, cursor.as_raw())
        .expect("define cursor before Async probation");
    Backend::set_cursor_hidden(&mut backend, false);
    Backend::warp_pointer_root(&mut backend, &mut state, 300, 160);
    damage_root(&mut backend);
    Backend::maybe_composite(&mut backend).expect("submit Vsync cursor attach");
    assert!(backend.scene.has_pending_page_flips());
    retire_composed_frame(&mut backend, &mut state, 61);
    if backend.platform.cursor_atomic_event_pending_for_output(0) {
        retire_composed_frame(&mut backend, &mut state, 62);
    }
    let crtc = backend.platform.outputs[0].output.crtc;
    let properties = backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .expect("installed cursor plane")
        .properties_for(crtc)
        .expect("cursor properties");
    let accepted = backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("Vsync attach accepted before Async probation");
    assert!(backend.platform.cursor_plane_visible_for_output(0));

    backend.platform.devices[0].async_page_flip = true;
    for index in 0..24_u64 {
        let present_id = 22_500 + index;
        if try_live_direct_for_ids(
            &mut backend,
            present_id,
            WINDOW,
            WINDOW,
            yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
        )
        .is_some()
        {
            retire_direct_present(&mut backend, &mut state, 70 + index as u32);
        } else if backend.scene.has_pending_page_flips() {
            retire_composed_frame(&mut backend, &mut state, 100 + index as u32);
        } else {
            Backend::maybe_composite(&mut backend).expect("advance Async probation");
        }
        if backend.pstate.tearing.is_on() {
            if backend.pstate.scanout_m2.pending.is_some() {
                retire_direct_present(&mut backend, &mut state, 140 + index as u32);
            }
            if !backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty() {
                retire_composed_frame(&mut backend, &mut state, 180 + index as u32);
            }
            Backend::maybe_composite(&mut backend).expect("settle On entry cursor detach");
            break;
        }
    }
    assert!(
        backend.pstate.tearing.is_on(),
        "real Async Presents enter On"
    );
    assert_cursorless_async_records(&backend, &recorder.records(), properties.plane);
    for frame in 200..208 {
        if backend.platform.cursor_atomic_event_pending_for_output(0)
            || backend.scene.has_pending_page_flips()
            || !backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty()
        {
            retire_composed_frame(&mut backend, &mut state, frame);
        }
        Backend::maybe_composite(&mut backend).expect("drain On entry retirement");
        if !backend.platform.cursor_atomic_event_pending_for_output(0)
            && !backend.scene.has_pending_page_flips()
            && backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty()
        {
            break;
        }
    }
    assert!(
        recorder.records().iter().any(|record| {
            record.properties.iter().any(|update| {
                update.object == properties.plane.into()
                    && update.property == properties.fb_id
                    && update.value == 0
            })
        }),
        "On entry detaches the previously accepted HW cursor",
    );
    assert!(!backend.platform.cursor_plane_visible_for_output(0));
    let state_after_entry = backend.platform.devices[0]
        .cursor
        .crtc_states
        .get(&crtc)
        .cloned()
        .unwrap_or_default();
    assert!(state_after_entry.applied.is_none());
    assert!(state_after_entry.intent.is_none());
    assert!(state_after_entry.in_flight.is_none());
    assert_eq!(accepted.lease.version(), 1);

    let first_on_cursor =
        create_argb_cursor_with_hotspot(&mut backend, [0x11, 0x22, 0x33, 0xff], 1, 1);
    Backend::define_cursor(&mut backend, None, root, first_on_cursor.as_raw())
        .expect("change cursor image and hotspot while On");
    Backend::warp_pointer_root(&mut backend, &mut state, 340, 180);
    damage_root(&mut backend);
    let async_start = recorder.records().len();
    Backend::maybe_composite(&mut backend).expect("submit shared composed Async while On");
    let first_async = recorder.records();
    assert!(
        first_async[async_start..]
            .iter()
            .any(|record| record.kind == AtomicCommitKind::Async)
    );
    assert_cursorless_async_records(&backend, &first_async, properties.plane);
    assert!(backend.scene.has_pending_page_flips());

    let latest_cursor =
        create_argb_cursor_with_hotspot(&mut backend, [0xa1, 0xb2, 0xc3, 0xff], 2, 1);
    Backend::define_cursor(&mut backend, None, root, latest_cursor.as_raw())
        .expect("queue latest image and hotspot behind Async");
    Backend::warp_pointer_root(&mut backend, &mut state, 365, 195);
    Backend::maybe_composite(&mut backend)
        .expect("keep latest cursor intent while Async is pending");
    let filtered = backend.platform.devices[0]
        .cursor
        .crtc_states
        .get(&crtc)
        .cloned()
        .unwrap_or_default();
    assert!(filtered.applied.is_none());
    assert!(filtered.intent.is_none());
    assert!(filtered.in_flight.is_none());
    assert!(!backend.platform.cursor_atomic_event_pending_for_output(0));
    assert!(
        backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_pool_users_are_zero()
    );
    retire_composed_frame(&mut backend, &mut state, 209);

    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    damage_root(&mut backend);
    let fallback_start = recorder.records().len();
    Backend::maybe_composite(&mut backend).expect("Async EINVAL falls back to cursorless Vsync");
    let fallback_records = recorder.records();
    assert!(
        fallback_records[fallback_start..]
            .iter()
            .any(|record| record.kind == AtomicCommitKind::Async
                && record.result
                    == crate::drm::atomic::tests_support::ScriptedResult::Error {
                        kind: std::io::ErrorKind::InvalidInput,
                        raw_os_error: Some(libc::EINVAL),
                    })
    );
    assert!(
        fallback_records[fallback_start..]
            .iter()
            .any(|record| record.kind == AtomicCommitKind::Scene)
    );
    assert_cursorless_async_records(&backend, &fallback_records, properties.plane);
    assert!(
        fallback_records[fallback_start..]
            .iter()
            .filter(|record| record.kind == AtomicCommitKind::Scene)
            .all(|record| record
                .properties
                .iter()
                .all(|update| { update.object != properties.plane.into() }))
    );
    assert!(backend.pstate.tearing.is_on());
    assert!(!backend.platform.cursor_atomic_event_pending_for_output(0));
    assert!(
        backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&crtc)
            .is_none_or(|state| state.applied.is_none() && state.intent.is_none())
    );
    let device_key = backend.platform.outputs[0].key.device_key;
    assert!(
        !backend
            .platform
            .for_tests_cursor_permanently_disabled(device_key)
    );
    assert_eq!(
        backend
            .platform
            .for_tests_cursor_fallback_remaining(device_key, crtc),
        None,
        "Async EINVAL suspends the Async stream without cursor failure backoff",
    );
    retire_composed_frame(&mut backend, &mut state, 210);

    let _ = try_live_direct_for_ids(&mut backend, 22_900, WINDOW, WINDOW, 0);
    assert!(
        !backend.pstate.tearing.is_on(),
        "non-Async Backend Present exits On"
    );
    Backend::set_cursor_hidden(&mut backend, true);
    Backend::set_cursor_hidden(&mut backend, false);
    if backend.pstate.scanout_m2.pending.is_some() {
        retire_direct_present(&mut backend, &mut state, 211);
    }
    for frame in 212..224 {
        Backend::maybe_composite(&mut backend).expect("reattach latest cursor after On");
        if backend.scene.has_pending_page_flips()
            || !backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty()
        {
            retire_composed_frame(&mut backend, &mut state, frame);
        }
        if backend.platform.cursor_plane_visible_for_output(0)
            && backend.platform.devices[0]
                .cursor
                .crtc_states
                .get(&crtc)
                .is_some_and(|state| state.applied.is_some() && state.in_flight.is_none())
        {
            break;
        }
    }
    let rebound = backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("exit On reattaches the latest logical cursor");
    assert_eq!(
        rebound.lease.version(),
        backend.cursor_records[&latest_cursor.as_raw()].version
    );
    assert_eq!((rebound.hot_x, rebound.hot_y), (2, 1));
    assert_eq!((rebound.x, rebound.y), (363, 194));

    for frame in 230..246 {
        if backend.pstate.scanout_m2.pending.is_some() {
            retire_direct_present(&mut backend, &mut state, frame);
        } else if backend.scene.has_pending_page_flips()
            || backend.platform.cursor_atomic_event_pending_for_output(0)
            || !backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty()
        {
            retire_composed_frame(&mut backend, &mut state, frame);
        }
        Backend::maybe_composite(&mut backend).expect("drain post-exit scanout events");
        if backend.pstate.scanout_m2.pending.is_none()
            && !backend.scene.has_pending_page_flips()
            && !backend.platform.cursor_atomic_event_pending_for_output(0)
            && backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty()
        {
            break;
        }
    }

    backend.platform.for_tests_accept_output_power_commits();
    backend
        .disable_output()
        .expect("Backend shutdown drains the cursor detach");
    assert_tearing_off(&backend);
    assert_direct_queue_drained(&backend);
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.platform.cursor_atomic_event_pending_for_output(0));
    assert!(!backend.platform.cursor_plane_visible_for_output(0));
    assert!(
        backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&crtc)
            .is_none_or(|state| {
                state.applied.is_none() && state.intent.is_none() && state.in_flight.is_none()
            })
    );
    assert!(
        backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .is_none_or(|plane| plane.sprite_pool_users_are_zero())
    );
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
