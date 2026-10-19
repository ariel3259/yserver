use super::*;

pub(super) fn create_argb_cursor_with_hotspot(
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

pub(super) fn assert_cursorless_async_records(
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
// Mutation: append cursor fields or OUT_FENCE_PTR to Async; the accepted-binding cursorless
// request audit must fail.
pub(super) fn atomic_cursor_only_c1_hidden_direct_entry_detaches_accepted_binding() {
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x7a60;
    let Some(mut backend) = live_scene_backend() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    install_atomic_cursor_plane_stub(&mut backend);
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
    Backend::warp_pointer_root(&mut backend, &mut state, 307, 167);
    assert!(
        backend.platform.devices[0].cursor.crtc_states[&crtc]
            .intent
            .is_some(),
        "motion is pending when hidden-direct Async admission begins"
    );

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
            Backend::before_block(&mut backend);
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
        Backend::before_block(&mut backend);
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
    assert!(
        !backend.platform.any_cursor_plane_visible(),
        "hidden-direct entry has detached the previously accepted binding",
    );
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
    backend.shutdown_destroy_drawables();
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
// Mutations: admit Async while the hidden cursor is still applied, append cursor properties to
// Async, or ignore the newest cursor intent on exit; this trace and the binding-value assertions
// must fail.
pub(super) fn atomic_cursor_only_c1_hidden_direct_rejected_detach_keeps_lease_and_rebinds_latest() {
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x7a62;
    let Some(mut backend) = live_scene_backend() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    install_atomic_cursor_plane_stub(&mut backend);
    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let mut state = yserver_core::server::ServerState::new();
    let cursor = create_bitmap_cursor_via_backend(&mut backend, true, true);
    let root = backend.core.window_id;
    Backend::define_cursor(&mut backend, None, root, cursor.as_raw())
        .expect("define cursor before direct Async probation");
    Backend::set_cursor_hidden(&mut backend, false);
    Backend::warp_pointer_root(&mut backend, &mut state, 300, 160);
    damage_root(&mut backend);
    Backend::maybe_composite(&mut backend).expect("submit accepted Vsync cursor binding");
    assert!(backend.scene.has_pending_page_flips());
    retire_composed_frame(&mut backend, &mut state, 301);
    if backend.platform.cursor_atomic_event_pending_for_output(0) {
        retire_composed_frame(&mut backend, &mut state, 302);
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
        .expect("initial hardware cursor attach is accepted");
    assert!(backend.platform.cursor_plane_visible_for_output(0));
    assert_eq!(
        backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("installed cursor plane")
            .sprite_user_count(accepted.lease),
        Some(1)
    );
    recorder.clear_records();

    // Preload direct entry and the detach rejection before entry.
    recorder.script_result(Ok(()));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    backend.platform.devices[0].async_page_flip = true;
    let mut direct_entry_pending = false;
    for index in 0..24_u64 {
        let submitted = try_live_direct_for_ids(
            &mut backend,
            22_600 + index,
            WINDOW,
            WINDOW,
            yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
        );
        if backend.pstate.tearing.is_on() {
            assert!(
                submitted.is_some(),
                "On entry has an accepted direct Present awaiting Backend retirement",
            );
            direct_entry_pending = true;
            break;
        }
        if submitted.is_some() {
            retire_direct_present(&mut backend, &mut state, 310 + index as u32);
        } else if backend.scene.has_pending_page_flips() {
            retire_composed_frame(&mut backend, &mut state, 340 + index as u32);
        } else {
            Backend::maybe_composite(&mut backend).expect("advance direct Async entry");
        }
    }
    assert!(
        backend.pstate.tearing.is_on(),
        "Async direct Presents enter On"
    );
    assert!(direct_entry_pending);
    assert!(backend.pstate.scanout_m2.pending.is_some());
    Backend::set_cursor_hidden(&mut backend, true);
    assert!(
        backend.effective_cursor_hidden(),
        "XFIXES hide applies during On entry"
    );
    assert_eq!(
        backend.platform.devices[0].cursor.crtc_states[&crtc]
            .applied
            .expect("pending direct entry still retains the accepted HW binding"),
        accepted
    );
    retire_direct_present(&mut backend, &mut state, 399);
    Backend::before_block(&mut backend);
    let records_after_entry = recorder.records();
    assert_cursorless_async_records(&backend, &records_after_entry, properties.plane);
    let is_rejected_detach = |record: &crate::drm::atomic::tests_support::AtomicCommitRecord| {
        matches!(
            record.kind,
            AtomicCommitKind::CursorOnly | AtomicCommitKind::SceneWithCursor
        ) && record.result
            == crate::drm::atomic::tests_support::ScriptedResult::Error {
                kind: std::io::ErrorKind::ResourceBusy,
                raw_os_error: Some(libc::EBUSY),
            }
            && record.properties.iter().any(|update| {
                update.object == properties.plane.into()
                    && update.property == properties.fb_id
                    && update.value == 0
            })
    };
    let rejected_index = records_after_entry
        .iter()
        .position(is_rejected_detach)
        .expect("preloaded EBUSY rejects the On handoff detach");
    // The accepted binding survives the hidden direct-entry failure.
    let records_after_retries = records_after_entry.clone();
    assert_eq!(
        records_after_retries
            .iter()
            .filter(|record| is_rejected_detach(record))
            .count(),
        1,
        "preloaded EBUSY retains the accepted binding after direct retirement",
    );
    assert_cursorless_async_records(&backend, &records_after_retries, properties.plane);
    assert_eq!(
        backend.platform.devices[0].cursor.crtc_states[&crtc]
            .applied
            .expect("rejected detach preserves actual HW binding"),
        accepted
    );
    assert!(
        backend.pstate.scanout_m2.active(),
        "rejected detach keeps direct scanout active",
    );
    assert!(backend.pstate.tearing.is_on());
    assert_eq!(
        backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("installed cursor plane")
            .sprite_user_count(accepted.lease),
        Some(1),
        "rejected detach retains the accepted sprite lease",
    );

    assert!(
        backend.effective_cursor_hidden(),
        "the accepted binding remains hidden through rejected detach retries"
    );
    assert_eq!(
        backend.platform.devices[0].cursor.crtc_states[&crtc]
            .applied
            .expect("hidden cursor remains bound until accepted detach"),
        accepted
    );
    let blocked_async_start = recorder.records().len();
    for index in 0..9_u64 {
        assert!(
            try_live_direct_for_ids(
                &mut backend,
                23_500 + index,
                WINDOW,
                WINDOW,
                yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
            )
            .is_none(),
            "a still-bound hidden cursor blocks direct Async admission at attempt {index}",
        );
    }
    assert!(
        recorder.records()[blocked_async_start..]
            .iter()
            .all(|record| record.kind != AtomicCommitKind::Async),
        "an outstanding rejected detach keeps Async blocked while the real cursor is hidden and bound",
    );
    assert_eq!(
        backend.platform.devices[0].cursor.crtc_states[&crtc]
            .applied
            .expect("blocked direct Async admissions retain the cursor binding"),
        accepted,
    );
    assert_eq!(
        backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("installed cursor plane")
            .sprite_user_count(accepted.lease),
        Some(1),
    );
    assert!(
        records_after_retries[rejected_index + 1..]
            .iter()
            .all(|record| record.kind != AtomicCommitKind::Async)
    );
    assert_cursorless_async_records(&backend, &recorder.records(), properties.plane);

    // A normal Backend Present leaves On and requests the grouped composed unflip. The consumed
    // one-shot EBUSY fault is gone, so the next Backend composite accepts the explicit detach.
    let detach_retry = try_live_direct_for_ids(&mut backend, 23_590, WINDOW, WINDOW, 0);
    assert!(
        detach_retry.is_none(),
        "the non-Async Present starts the On exit"
    );
    assert!(
        !backend.pstate.tearing.is_on(),
        "Vsync Present leaves On for retry"
    );
    assert!(backend.pstate.scanout_m2.unflip_requested);
    let detach_retry_start = recorder.records().len();
    Backend::maybe_composite(&mut backend).expect("submit accepted grouped detach retry");
    assert!(!backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert!(backend.platform.cursor_atomic_event_pending_for_output(0));
    retire_composed_frame(&mut backend, &mut state, 400);
    let detach_retry_records = recorder.records();
    assert!(
        detach_retry_records[detach_retry_start..]
            .iter()
            .any(|record| {
                record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
                    && record.properties.iter().any(|update| {
                        update.object == properties.plane.into()
                            && update.property == properties.fb_id
                            && update.value == 0
                    })
            }),
        "the retried cursor detach is accepted by the grouped scene commit"
    );
    Backend::maybe_composite(&mut backend).expect("settle accepted grouped detach retry");
    assert!(!backend.platform.cursor_plane_visible_for_output(0));
    assert!(
        backend.platform.devices[0].cursor.crtc_states[&crtc]
            .applied
            .is_none()
    );
    assert!(backend.effective_cursor_hidden());

    let latest_cursor =
        create_argb_cursor_with_hotspot(&mut backend, [0xa1, 0xb2, 0xc3, 0xff], 2, 1);
    Backend::define_cursor(&mut backend, None, root, latest_cursor.as_raw())
        .expect("queue latest sprite while the cursor remains hidden");
    Backend::warp_pointer_root(&mut backend, &mut state, 365, 195);
    let latest_version = backend.cursor_records[&latest_cursor.as_raw()].version;

    // Keep the logical cursor hidden while the fullscreen client regains direct scanout and
    // produces actual Async commits. A count assertion keeps the Async property audit non-vacuous.
    let async_trace_start = recorder.records().len();
    for index in 0..12_u64 {
        let submitted = try_live_direct_for_ids(
            &mut backend,
            23_600 + index,
            WINDOW,
            WINDOW,
            yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
        );
        if submitted.is_some() {
            assert!(backend.pstate.scanout_m2.pending.is_some());
            retire_direct_present(&mut backend, &mut state, 420 + index as u32);
        } else if backend.scene.has_pending_page_flips()
            || backend.platform.cursor_atomic_event_pending_for_output(0)
        {
            retire_composed_frame(&mut backend, &mut state, 470 + index as u32);
        } else {
            Backend::maybe_composite(&mut backend).expect("advance hidden direct Async admission");
        }
        assert!(backend.effective_cursor_hidden());
        assert!(!backend.platform.cursor_plane_visible_for_output(0));
        assert!(
            backend.platform.devices[0].cursor.crtc_states[&crtc]
                .applied
                .is_none()
        );
        let async_count = recorder.records()[async_trace_start..]
            .iter()
            .filter(|record| record.kind == AtomicCommitKind::Async)
            .count();
        if backend.pstate.tearing.is_on() && async_count >= 3 {
            break;
        }
    }
    let async_trace = recorder.records();
    let async_records = &async_trace[async_trace_start..];
    assert!(
        async_records
            .iter()
            .filter(|record| record.kind == AtomicCommitKind::Async)
            .count()
            >= 3,
        "the direct Async audit must include at least three accepted requests",
    );
    assert!(
        backend.pstate.tearing.is_on(),
        "Async direct scanout re-enters On"
    );
    assert!(backend.pstate.scanout_m2.active());
    assert_cursorless_async_records(&backend, async_records, properties.plane);
    assert!(backend.effective_cursor_hidden());
    assert!(!backend.platform.cursor_plane_visible_for_output(0));
    assert!(
        backend.platform.devices[0].cursor.crtc_states[&crtc]
            .applied
            .is_none()
    );

    Backend::set_cursor_hidden(&mut backend, false);

    let records_before_exit = recorder.records().len();
    let _ = try_live_direct_for_ids(&mut backend, 24_000, WINDOW, WINDOW, 0);
    assert!(
        !backend.pstate.tearing.is_on(),
        "normal non-Async Present exits On"
    );
    if backend.pstate.scanout_m2.pending.is_some() {
        retire_direct_present(&mut backend, &mut state, 410);
    }
    Backend::set_cursor_hidden(&mut backend, true);
    Backend::set_cursor_hidden(&mut backend, false);
    assert!(!backend.effective_cursor_hidden());
    Backend::maybe_composite(&mut backend).expect("submit latest post-On software-primary frame");

    let retry_wait_started = std::time::Instant::now();
    for frame in 411..435 {
        let deadline = [
            backend.scene.earliest_retry_deadline(),
            backend.pstate.tearing.deferred_cursor_wake,
        ]
        .into_iter()
        .flatten()
        .min();
        if let Some(deadline) = deadline {
            while std::time::Instant::now() < deadline {
                assert!(retry_wait_started.elapsed() < std::time::Duration::from_secs(5));
                std::thread::yield_now();
            }
        }
        if backend.platform.devices[0].cursor.crtc_states[&crtc]
            .applied
            .is_some()
            && !backend.scene.has_pending_page_flips()
        {
            damage_root(&mut backend);
        }
        Backend::on_scanout_render_completion(&mut backend, &mut state);
        Backend::maybe_composite(&mut backend)
            .expect("rebind latest cursor after hidden-direct exit");
        if backend.scene.has_pending_page_flips()
            || backend.platform.cursor_atomic_event_pending_for_output(0)
            || !backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty()
        {
            retire_composed_frame(&mut backend, &mut state, frame);
        }
        let state_after_exit = &backend.platform.devices[0].cursor.crtc_states[&crtc];
        if state_after_exit.applied.is_some_and(|binding| {
            binding.lease.version() == latest_version && binding.framebuffer != accepted.framebuffer
        }) && state_after_exit.in_flight.is_none()
            && !backend.platform.cursor_atomic_event_pending_for_output(0)
        {
            break;
        }
        std::thread::yield_now();
    }
    let rebound = backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .unwrap_or_else(|| {
            panic!(
                "normal exit rebinds latest cursor: state={:?}, mode={:?}, pending_scene={}, cursor_event={}, m2_pending={}, m2_current={}, unflip={:?}, records={:?}",
                backend.platform.devices[0].cursor.crtc_states[&crtc],
                backend.scene.cursor_mode(),
                backend.scene.has_pending_page_flips(),
                backend.platform.cursor_atomic_event_pending_for_output(0),
                backend.pstate.scanout_m2.pending.is_some(),
                backend.pstate.scanout_m2.current.is_some(),
                backend.pstate.scanout_m2.unflip_awaiting_outputs,
                recorder.records().iter().map(|record| (record.kind, record.result)).collect::<Vec<_>>(),
            )
        });
    assert_eq!(rebound.lease.version(), latest_version);
    assert_ne!(rebound.lease, accepted.lease);
    assert_eq!((rebound.hot_x, rebound.hot_y), (2, 1));
    assert_eq!((rebound.x, rebound.y), (363, 194));
    assert!(
        recorder.records()[records_before_exit..]
            .iter()
            .any(|record| {
                record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
                    && record.properties.iter().any(|update| {
                        update.object == properties.plane.into()
                            && update.property == properties.fb_id
                            && update.value == u64::from(rebound.framebuffer)
                    })
            })
    );
    assert_cursorless_async_records(&backend, &recorder.records(), properties.plane);

    for frame in 435..451 {
        if backend.scene.has_pending_page_flips()
            || backend.platform.cursor_atomic_event_pending_for_output(0)
        {
            retire_composed_frame(&mut backend, &mut state, frame);
        }
        if !backend.scene.has_pending_page_flips()
            && !backend.platform.cursor_atomic_event_pending_for_output(0)
        {
            break;
        }
        Backend::maybe_composite(&mut backend).expect("drain hidden-direct exit frame");
    }
    backend.platform.for_tests_accept_output_power_commits();
    Backend::set_dpms_power(&mut backend, 3).expect("Backend teardown after On exit");
    let _ = Backend::drain_completed_present_events(&mut backend);
    let _ = Backend::drain_retired_present_idle_events(&mut backend);
    assert!(!backend.pstate.tearing.is_on());
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.platform.any_cursor_commit_in_flight());
    assert!(!backend.platform.any_cursor_plane_visible());
    assert!(backend.pstate.scanout_m2.pending.is_none());
    assert!(backend.pstate.scanout_m2.current.is_none());
    assert!(backend.pstate.scanout_m2.queued_successor.is_none());
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert_direct_queue_drained(&backend);
    assert!(backend.platform.async_ledger.for_tests_is_empty());
    assert!(
        backend.platform.devices[0]
            .cursor
            .crtc_states
            .values()
            .all(|state| state.applied.is_none()
                && state.intent.is_none()
                && state.in_flight.is_none()
                && state.lifecycle_pending.is_none())
    );
    assert!(
        backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("installed cursor plane")
            .sprite_pool_users_are_zero()
    );
    let records = recorder.records();
    let async_count = records
        .iter()
        .filter(|record| record.kind == AtomicCommitKind::Async)
        .count();
    let mut expected_sequence = vec![
        (
            AtomicCommitKind::Scene,
            crate::drm::atomic::tests_support::ScriptedResult::Ok,
        ),
        (
            AtomicCommitKind::CursorOnly,
            crate::drm::atomic::tests_support::ScriptedResult::Error {
                kind: std::io::ErrorKind::ResourceBusy,
                raw_os_error: Some(libc::EBUSY),
            },
        ),
        (
            AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::tests_support::ScriptedResult::Ok,
        ),
        (
            AtomicCommitKind::Scene,
            crate::drm::atomic::tests_support::ScriptedResult::Ok,
        ),
    ];
    expected_sequence.extend((0..async_count).map(|_| {
        (
            AtomicCommitKind::Async,
            crate::drm::atomic::tests_support::ScriptedResult::Ok,
        )
    }));
    // The latest-intent rebind rides the scene frame: the intent is sampled at scene submission.
    expected_sequence.extend([
        (
            AtomicCommitKind::Scene,
            crate::drm::atomic::tests_support::ScriptedResult::Ok,
        ),
        (
            AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::tests_support::ScriptedResult::Ok,
        ),
        (
            AtomicCommitKind::Scene,
            crate::drm::atomic::tests_support::ScriptedResult::Ok,
        ),
    ]);
    assert_eq!(
        records
            .iter()
            .map(|record| (record.kind, record.result))
            .collect::<Vec<_>>(),
        expected_sequence,
        "hidden EBUSY, accepted detach, cursorless direct Async run, latest rebind, and teardown sequence",
    );
    for present_id in (22_600..22_624_u64)
        .chain(23_500..23_509)
        .chain(23_590..23_591)
        .chain(23_600..23_612)
        .chain(24_000..24_001)
    {
        let source_xid = 0x8300 + u32::try_from(present_id).expect("test Present ID fits") * 0x10;
        if backend.store.lookup(source_xid).is_some() {
            Backend::free_pixmap(&mut backend, None, source_xid)
                .expect("free drained test Present source");
        }
    }
    backend.shutdown_destroy_drawables();
    backend.platform.wait_idle_bounded();
    let recorders = std::rc::Rc::clone(
        backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("recording test services"),
    );
    drop(backend);
    assert_eq!(recorders.cursor_buffer_ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: append cursor FB_ID to Async, settle a filtered delta, or reattach on Vsync fallback.
pub(super) fn atomic_cursor_only_c1_async_filters_before_cursor_preparation() {
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x7a61;
    let Some((mut backend, recorder, mut state)) = live_tearing_cursor_backend(WINDOW, 22_000)
    else {
        return;
    };
    install_atomic_cursor_plane_stub(&mut backend);
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
    backend.shutdown_destroy_drawables();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: allow a hidden-direct Async to bind cursor state or settle EINVAL's rejected cursor delta.
pub(super) fn atomic_cursor_only_c1_hidden_direct_async_fallback_stays_cursorless() {
    use yserver_core::{backend::Backend, server::ServerState};

    const WINDOW: u32 = 0x7a62;
    let Some(mut backend) = live_scene_backend() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    qualify_test_direct_async(&mut backend, &recorder, WINDOW, 23_000);
    let mut state = ServerState::new();
    install_atomic_cursor_plane_stub(&mut backend);
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
    backend.shutdown_destroy_drawables();
}
