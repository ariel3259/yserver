use super::*;

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: publish Software or detach HW when the Backend scene path has no free BO.
fn tearing_on_no_scene_bo_keeps_hw_until_sw_retry() {
    run_entry_with_no_scene_slot_keeps_hw_until_retry();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: accept the failed render's Software assignment or detach despite no accepted frame.
fn tearing_on_render_failure_keeps_hw_until_teardown() {
    run_entry_with_render_failure_keeps_hw_until_teardown();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: publish Software or detach while the grouped successor owns HW, or schedule another commit before each per-CRTC event retires.
fn tearing_grouped_two_output_handoff_waits_for_each_event() {
    run_two_output_grouped_tearing_handoff_waits_for_each_event();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: classify by XFIXES only, or preserve the deferred cursor wake on tearing exit.
fn transparent_cursor_is_hidden_for_direct_scanout_and_define_cursor_handoff() {
    use yserver_core::{backend::Backend, server::ServerState};

    const WINDOW: u32 = 0x78a1;
    let Some(mut backend) = live_scene_backend() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let mut state = ServerState::new();
    let (show_executor, _) = install_cursor_plane_with_successful_show(&mut backend);
    let root = backend.core.window_id;
    assert!(
        backend.effective_cursor_hidden(),
        "no effective cursor starts hidden"
    );

    let transparent = create_bitmap_cursor_via_backend(&mut backend, false, false);
    assert!(backend.cursor_records[&transparent.as_raw()].is_fully_transparent);
    Backend::define_cursor(&mut backend, None, root, transparent.as_raw())
        .expect("define transparent core cursor outside tearing");
    assert!(!backend.cursor_hidden, "the cursor is not XFIXES-hidden");
    assert!(backend.effective_cursor_hidden());
    damage_root_and_composite(&mut backend);
    assert!(
        backend.scene.has_pending_page_flips(),
        "expected a pending scene page flip: {}",
        tearing_composite_diagnostics(&backend, None),
    );
    retire_composed_frame(&mut backend, &mut state, 51);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    assert!(show_executor.calls().is_empty());
    assert!(!backend.platform.any_cursor_plane_visible());

    // The visible one-pixel cursor remains a real cursor through CreateCursor.
    let opaque = create_bitmap_cursor_via_backend(&mut backend, true, true);
    assert!(!backend.cursor_records[&opaque.as_raw()].is_fully_transparent);

    // Classification also covers core glyphs, recoloring, and the animation-wide
    // rule: one opaque frame makes the animated cursor visible.
    let missing_font = yserver_core::backend::FontHandle::from_raw(0x78a2).unwrap();
    let glyph = Backend::create_glyph_cursor(
        &mut backend,
        None,
        missing_font,
        None,
        0,
        0,
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
    )
    .expect("create transparent fallback glyph cursor");
    assert!(backend.cursor_records[&glyph.as_raw()].is_fully_transparent);
    Backend::recolor_cursor(
        &mut backend,
        None,
        glyph.as_raw(),
        (0, 0, 0),
        (u16::MAX, 0, 0),
    )
    .expect("recolor transparent glyph cursor");
    assert!(backend.cursor_records[&glyph.as_raw()].is_fully_transparent);
    let all_transparent =
        Backend::create_anim_cursor(&mut backend, None, &[(transparent, 40), (glyph, 40)])
            .expect("create all-transparent animation")
            .expect("animation handle");
    assert!(backend.anim_cursor_records[&all_transparent.as_raw()].is_fully_transparent);
    let mixed = Backend::create_anim_cursor(&mut backend, None, &[(transparent, 40), (opaque, 40)])
        .expect("create mixed animation")
        .expect("animation handle");
    assert!(!backend.anim_cursor_records[&mixed.as_raw()].is_fully_transparent);
    Backend::define_cursor(&mut backend, None, root, all_transparent.as_raw())
        .expect("define all-transparent animation");
    assert!(backend.effective_cursor_hidden());
    Backend::define_cursor(&mut backend, None, root, mixed.as_raw())
        .expect("define animation with an opaque frame");
    assert!(!backend.effective_cursor_hidden());
    Backend::define_cursor(&mut backend, None, root, transparent.as_raw())
        .expect("restore transparent core cursor");
    assert!(backend.effective_cursor_hidden());
    damage_root_and_composite(&mut backend);
    retire_composed_frame(&mut backend, &mut state, 55);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    assert!(show_executor.calls().is_empty());
    assert!(!backend.platform.any_cursor_plane_visible());

    // The test cursor plane above checks the non-tearing path. Remove that test
    // device now so every direct async commit below is cursor-plane-free.
    backend.platform.devices[0].cursor.plane = None;
    recorder.clear_records();
    present_until_direct_async(&mut backend, &recorder, &mut state, WINDOW, 15_100);
    assert!(backend.pstate.tearing.is_on());
    assert!(backend.pstate.scanout_m2.active());
    assert!(!backend.platform.any_cursor_plane_visible());

    // DefineCursor from a fully transparent image to one opaque pixel must
    // unwind direct scanout and compose the cursor while tearing remains On.
    Backend::define_cursor(&mut backend, None, root, opaque.as_raw())
        .expect("define one-pixel cursor");
    assert!(!backend.effective_cursor_hidden());
    assert!(backend.tearing_cursor_wants_visible());
    assert!(backend.pstate.scanout_m2.unflip_requested);
    assert_eq!(
        backend.pstate.scanout_m2.unflip_reason,
        Some("tearing_cursor_handoff")
    );
    Backend::maybe_composite(&mut backend).expect("submit cursor handoff");
    assert!(!recorder.records().is_empty());
    retire_composed_frame(&mut backend, &mut state, 52);
    assert!(!backend.pstate.scanout_m2.active());
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    damage_root(&mut backend);
    Backend::maybe_composite(&mut backend).expect("compose visible one-pixel cursor");
    assert!(
        backend.scene.has_pending_page_flips(),
        "expected a pending scene page flip: {}",
        tearing_composite_diagnostics(&backend, None),
    );
    retire_composed_frame(&mut backend, &mut state, 53);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );
    assert!(backend.scene.for_tests_cursor_footprint_was_presented(0));
    assert!(
        try_live_direct_for_ids(
            &mut backend,
            15_150,
            WINDOW,
            WINDOW,
            yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
        )
        .is_none(),
        "a one-pixel visible cursor blocks direct scanout after its handoff"
    );

    // A transparent RENDER cursor must clear the scene cursor and permit direct
    // async scanout again after its hidden frame retires.
    let render_cursor = create_render_cursor_via_backend(&mut backend);
    assert!(backend.cursor_records[&render_cursor.as_raw()].is_fully_transparent);
    Backend::define_cursor(&mut backend, None, root, render_cursor.as_raw())
        .expect("define transparent RENDER cursor");
    assert!(backend.effective_cursor_hidden());
    Backend::maybe_composite(&mut backend).expect("compose hidden RENDER cursor");
    assert!(
        backend.scene.has_pending_page_flips(),
        "expected a pending scene page flip: {}",
        tearing_composite_diagnostics(&backend, None),
    );
    retire_composed_frame(&mut backend, &mut state, 54);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    assert!(!backend.platform.any_cursor_plane_visible());
    present_until_direct_async(&mut backend, &recorder, &mut state, WINDOW, 15_200);

    let sequence: Vec<_> = recorder
        .records()
        .iter()
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        sequence,
        [
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Async,
            AtomicCommitKind::Async,
        ],
        "transparent → visible → transparent cursor transitions must hand off composed ↔ direct"
    );
    assert!(backend.pstate.tearing.is_on());
    assert!(backend.pstate.scanout_m2.active());
    assert!(backend.pstate.scanout_m2.pending.is_none());
    assert_direct_queue_drained(&backend);
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert!(backend.pstate.tearing.deferred_cursor_wake.is_none());
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.scene.cursor_transition_pending());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    assert!(!backend.platform.any_cursor_plane_visible());

    seed_pending_tearing_cursor_wake(&mut backend);
    let exit_present =
        try_direct_present_with_live_storage(&mut backend, 15_300, WINDOW, 800, 600, 0);
    assert!(
        exit_present.is_some(),
        "a non-async Backend Present exits tearing"
    );
    assert_tearing_off(&backend);
    assert!(backend.pstate.scanout_m2.pending.is_some());
    retire_direct_present(&mut backend, &mut state, 55);

    assert_tearing_off(&backend);
    assert!(backend.pstate.scanout_m2.active());
    assert!(backend.pstate.scanout_m2.hold_direct);
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert_direct_queue_drained(&backend);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
        "tearing exit preserves transparent-as-hidden cursor policy"
    );
    assert!(!backend.platform.any_cursor_plane_visible());
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.scene.cursor_transition_pending());
    assert_commit_sequence(
        &recorder,
        &[
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Async,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
        ],
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit the direct cursor rebind from DefineCursor; the held Vsync frame must keep a visible sprite.
fn define_visible_cursor_rebinds_held_vsync_direct_scanout() {
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x78b1;
    let Some((mut backend, recorder, mut state, show_executor, hide_executor)) =
        super::round2::held_vsync_direct_scanout_with_transparent_cursor(WINDOW, 15_300, 1)
    else {
        return;
    };
    let root = backend.core.window_id;
    assert!(backend.pstate.scanout_m2.active());
    assert!(backend.pstate.scanout_m2.hold_direct);
    assert!(!backend.platform.any_cursor_plane_visible());

    let visible = create_bitmap_cursor_via_backend(&mut backend, true, true);
    let before_define = recorder.records().len();
    Backend::define_cursor(&mut backend, None, root, visible.as_raw())
        .expect("select opaque root cursor while direct scanout is held");
    assert_eq!(
        recorder.records().len(),
        before_define,
        "DefineCursor only queues the direct first-bind intent"
    );
    let before_show = recorder.records().len();
    Backend::maybe_composite(&mut backend).expect("schedule the accepted cursor rebind");
    assert_eq!(recorder.records().len(), before_show + 1);
    assert_eq!(
        recorder.records()[before_show].kind,
        AtomicCommitKind::CursorOnly
    );
    assert!(!backend.effective_cursor_hidden());
    assert!(show_executor.calls().is_empty());
    assert!(hide_executor.calls().is_empty());
    assert!(backend.platform.any_cursor_plane_visible());
    assert!(backend.pstate.scanout_m2.active());
    assert!(backend.pstate.scanout_m2.hold_direct);
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    retire_recorded_direct_flip(&mut backend, &mut state, 71);

    assert!(matches!(
        backend.pstate.tearing.state,
        crate::kms::render::backend::TearingState::Off
    ));
    assert_eq!(backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(backend.platform.async_ledger.for_tests_is_empty());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw,
        "the scene records the hardware plane rebound during direct scanout"
    );
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.scene.cursor_transition_pending());
    assert!(backend.platform.any_cursor_plane_visible());
    assert!(backend.pstate.scanout_m2.active());
    assert!(backend.pstate.scanout_m2.hold_direct);
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert_direct_queue_drained(&backend);
    super::round2::finish_held_direct_cursor_test(
        &mut backend,
        &mut state,
        &recorder,
        WINDOW,
        90,
        Some(0),
    );
    backend.platform.wait_idle_bounded();
}

fn retire_recorded_output_flip(
    backend: &mut KmsBackend,
    state: &mut yserver_core::server::ServerState,
    output_idx: usize,
    frame: u32,
) {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    use yserver_core::backend::Backend;

    let output = &backend.platform.outputs[output_idx];
    let (device, crtc) = (
        std::rc::Rc::clone(
            &backend
                .platform
                .device_for_key(output.key.device_key)
                .expect("recorded output device")
                .device,
        ),
        output.output.crtc,
    );
    let source = backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.test_page_flip_event_source.as_ref())
        .expect("recording output page-flip source");
    source.register_device(&device);
    source.queue_flip_for_device(&device, crtc, frame);
    let drm_fd = device.as_fd().as_raw_fd();
    Backend::on_page_flip_ready(backend, state, drm_fd);
}

fn live_scene_backend_with_clock_and_no_slot_on_fourth_scene_frame(
    clock: std::rc::Rc<dyn Fn() -> std::time::Instant>,
) -> Option<KmsBackend> {
    let mut backend = live_scene_backend_with_clock(clock)?;
    let pool = backend.platform.scanout_pools[0]
        .as_mut()
        .expect("recording scene fixture has a scanout pool")
        .display_pool_mut();
    // The cursorless setup and visible-HW scene acquire normally. The next
    // SW+detach frame sees a scripted busy result from the real BO pool.
    pool.for_tests_script_unavailable_acquisitions([false, false, true]);
    prime_cursorless_scene_primary(&mut backend);
    Some(backend)
}

pub(super) fn run_entry_from_held_direct_with_visible_hw_cursor_handoff() {
    run_entry_from_held_direct_with_visible_hw_cursor_handoff_inner(HandoffFrameFailure::None);
}

pub(super) fn run_entry_with_no_scene_slot_keeps_hw_until_retry() {
    run_entry_from_held_direct_with_visible_hw_cursor_handoff_inner(
        HandoffFrameFailure::NoSceneSlot,
    );
}

pub(super) fn run_entry_with_render_failure_keeps_hw_until_teardown() {
    run_entry_from_held_direct_with_visible_hw_cursor_handoff_inner(
        HandoffFrameFailure::RenderSubmit,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HandoffFrameFailure {
    None,
    NoSceneSlot,
    RenderSubmit,
}

fn run_entry_from_held_direct_with_visible_hw_cursor_handoff_inner(
    frame_failure: HandoffFrameFailure,
) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use yserver_core::{backend::Backend, server::ServerState};
    use yserver_protocol::x11::{ClientId, ResourceId};

    const WINDOW: ResourceId = ResourceId(0x0100_7805);
    let clock_base = std::time::Instant::now();
    let scheduler_time_ns = std::sync::Arc::new(AtomicU64::new(0));
    let clock_elapsed_ns = std::sync::Arc::clone(&scheduler_time_ns);
    let clock = std::rc::Rc::new(move || {
        clock_base + std::time::Duration::from_nanos(clock_elapsed_ns.load(Ordering::SeqCst))
    });
    let backend = match frame_failure {
        HandoffFrameFailure::NoSceneSlot => {
            live_scene_backend_with_clock_and_no_slot_on_fourth_scene_frame(clock)
        }
        HandoffFrameFailure::RenderSubmit => {
            live_scene_backend_with_clock_and_compose_error_after(clock, 2).map(|mut backend| {
                prime_cursorless_scene_primary(&mut backend);
                backend
            })
        }
        HandoffFrameFailure::None => live_scene_backend_with_clock(clock).map(|mut backend| {
            prime_cursorless_scene_primary(&mut backend);
            backend
        }),
    };
    let Some(mut backend) = backend else {
        return;
    };
    for device in &mut backend.platform.devices {
        device.async_page_flip = true;
    }
    let recorder = recording_sink(&mut backend, 0);
    let mut state = ServerState::new();
    let host_window = materialize_test_cow_stage(&mut backend, &mut state, ClientId(1), WINDOW);
    let (_show_executor, _hide_executor) = install_cursor_plane_with_successful_show(&mut backend);
    let cursor = define_visible_cursor_through_backend(&mut backend, &mut state);
    prepare_visible_hw_cursor(&mut backend, &mut state);
    assert_eq!(backend.effective_cursor_xid, Some(cursor));
    assert!(recorder.records().iter().any(|record| {
        record.kind == AtomicCommitKind::SceneWithCursor
            && record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
    }));
    assert!(!backend.platform.any_cursor_commit_in_flight());
    assert!(!backend.scene.has_pending_page_flips());
    recorder.clear_records();
    let cursor_commits_before_direct_entry = recorder.records().len();

    // Fill M2 probation through real Backend Presents without entering tearing.
    for index in 0..7_u64 {
        assert!(
            try_live_direct_for_ids(&mut backend, 12_650 + index, WINDOW.0, host_window, 0,)
                .is_none(),
            "M2 probation Present {index} must remain composed"
        );
    }
    assert_eq!(backend.pstate.scanout_m2.eligible_root_streak, 7);
    assert!(recorder.records().is_empty());
    assert!(
        try_live_direct_for_ids(&mut backend, 12_657, WINDOW.0, host_window, 0).is_some(),
        "the eighth eligible root Present enters held direct scanout"
    );
    retire_direct_present(&mut backend, &mut state, 2);
    assert!(backend.pstate.scanout_m2.current.is_some());
    assert!(
        recorder.records().len() > cursor_commits_before_direct_entry,
        "M2 binds the effective cursor when direct ownership retires"
    );
    assert!(recorder.records().iter().all(|record| {
        record.kind != AtomicCommitKind::CursorOnly
            || (record
                .flags
                .contains(::drm::control::AtomicCommitFlags::NONBLOCK)
                && record
                    .flags
                    .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT))
    }));
    assert!(backend.platform.any_cursor_plane_visible());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw
    );

    for index in 0..7_u64 {
        assert!(
            try_live_direct_for_ids(
                &mut backend,
                12_658 + index,
                WINDOW.0,
                host_window,
                yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
            )
            .is_some(),
            "qualifying Async Present {index} stays direct until entry"
        );
        retire_direct_present(&mut backend, &mut state, 3 + index as u32);
        assert!(backend.pstate.scanout_m2.current.is_some());
        assert!(backend.platform.any_cursor_plane_visible());
        assert!(!backend.pstate.tearing.is_on());
    }

    assert!(
        try_live_direct_for_ids(
            &mut backend,
            12_665,
            WINDOW.0,
            host_window,
            yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
        )
        .is_none(),
        "the entry Present falls back to Copy when handoff is requested"
    );
    assert!(backend.pstate.tearing.is_on());
    assert!(backend.pstate.scanout_m2.unflip_requested);
    assert_eq!(
        backend.pstate.scanout_m2.unflip_reason,
        Some("tearing_cursor_handoff")
    );
    assert!(backend.pstate.scanout_m2.current.is_some());
    assert!(backend.platform.any_cursor_plane_visible());
    assert!(backend.cow_id.is_some(), "the COW is materialized");
    assert_eq!(
        backend
            .store
            .lookup(yserver_core::resources::COMPOSITE_OVERLAY_WINDOW.0),
        backend.cow_id
    );

    assert!(
        backend.pstate.scanout_m2.pending.is_none(),
        "the accepted Vsync unflip is scheduled after the current direct event drains"
    );
    retire_direct_present(&mut backend, &mut state, 9);
    assert!(backend.pstate.scanout_m2.pending.is_none());
    Backend::maybe_composite(&mut backend).expect("submit grouped cursor handoff");
    assert!(backend.pstate.scanout_m2.current.is_some());
    assert!(!backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert!(backend.pstate.scanout_m2.unflip_shadow_ready);
    assert!(backend.platform.any_cursor_plane_visible());
    assert_eq!(
        recorder.records().last().unwrap().kind,
        AtomicCommitKind::Scene,
        "the grouped replacement retains the actual HW cursor owner",
    );
    let records_at_unflip = recorder.records();
    let unflip = records_at_unflip
        .last()
        .expect("accepted grouped primary replacement");
    assert_eq!(unflip.kind, AtomicCommitKind::Scene);
    assert_eq!(
        unflip.result,
        crate::drm::atomic::tests_support::ScriptedResult::Ok
    );
    assert_eq!(unflip.events.len(), 1);
    let output = &backend.platform.outputs[0];
    let device = backend
        .platform
        .device_for_key(output.key.device_key)
        .expect("cursor device");
    let cursor_state = device
        .cursor
        .crtc_states
        .get(&output.output.crtc)
        .expect("cursor CRTC state");
    let prior = cursor_state
        .applied
        .expect("grouped unflip retains the HW binding");
    assert!(cursor_state.in_flight.is_none());
    assert_eq!(
        device
            .cursor
            .plane
            .as_ref()
            .expect("cursor plane")
            .sprite_user_count(prior.lease),
        Some(1)
    );
    assert!(
        !backend.platform.any_cursor_commit_in_flight(),
        "the grouped primary event does not create a cursor detach obligation"
    );
    assert!(!backend.scene.cursor_transition_pending());
    assert!(records_at_unflip.iter().all(|record| {
        !record
            .flags
            .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_ASYNC)
    }));
    let mut expected = vec![AtomicCommitKind::Scene; 9];
    assert_commit_sequence(&recorder, &expected);

    let records_before_unflip_retirement = recorder.records().len();
    Backend::maybe_composite(&mut backend).expect("wait for grouped primary event");
    assert_eq!(recorder.records().len(), records_before_unflip_retirement);
    assert!(backend.platform.any_cursor_plane_visible());
    assert!(backend.pstate.tearing.is_on());

    retire_composed_frame(&mut backend, &mut state, 10);
    assert!(backend.pstate.scanout_m2.current.is_none());
    assert!(!backend.pstate.scanout_m2.hold_direct);
    assert!(backend.platform.any_cursor_plane_visible());
    assert!(!backend.platform.any_cursor_commit_in_flight());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw,
        "the retained primary receipt does not publish SW before SW pixels exist",
    );

    if frame_failure == HandoffFrameFailure::NoSceneSlot {
        let before_unavailable_frame = recorder.records().len();
        let acquisitions_before_sw = backend.platform.scanout_pools[0]
            .as_ref()
            .unwrap()
            .display_pool()
            .unavailable_acquisitions
            .len();
        assert_eq!(
            acquisitions_before_sw, 1,
            "fixture scripts no-slot on this SW frame after all earlier acquisitions",
        );
        Backend::maybe_composite(&mut backend).expect("unavailable SW frame remains recoverable");
        assert_eq!(recorder.records().len(), before_unavailable_frame);
        assert!(backend.pstate.tearing.is_on());
        assert!(backend.platform.any_cursor_plane_visible());
        assert!(!backend.platform.any_cursor_commit_in_flight());
        assert!(!backend.scene.has_pending_page_flips());
        assert_eq!(
            backend.scene.cursor_mode(),
            crate::kms::render::scene::CursorPlaneMode::Hw,
            "no BO means no software receipt and no cursor detach",
        );
        assert_eq!(
            backend
                .scene
                .for_tests_cursor_commit_receipt(0)
                .expect("grouped unflip retains a cursorless HW receipt")
                .mode,
            crate::kms::render::scene::CursorAcceptedMode::Hardware,
        );
    }
    if frame_failure == HandoffFrameFailure::RenderSubmit {
        let records_before_failed_render = recorder.records().len();
        Backend::maybe_composite(&mut backend).expect("render failure remains recoverable");
        assert_eq!(
            recorder.records().len(),
            records_before_failed_render,
            "a target preparation error cannot submit a primary or cursor detach",
        );
        assert!(!backend.platform_renderer_failed_for_tests());
        assert!(backend.pstate.tearing.is_on());
        assert!(backend.platform.any_cursor_plane_visible());
        assert!(!backend.platform.any_cursor_commit_in_flight());
        assert!(!backend.scene.has_pending_page_flips());
        assert_eq!(
            backend.scene.cursor_mode(),
            crate::kms::render::scene::CursorPlaneMode::Hw,
            "failed rendering publishes no Software receipt",
        );
        assert_eq!(
            backend
                .scene
                .for_tests_cursor_commit_receipt(0)
                .expect("grouped unflip retains the cursorless Hardware receipt")
                .mode,
            crate::kms::render::scene::CursorAcceptedMode::Hardware,
        );
        let output = &backend.platform.outputs[0];
        let cursor_state = backend
            .platform
            .device_for_key(output.key.device_key)
            .expect("cursor device")
            .cursor
            .crtc_states
            .get(&output.output.crtc)
            .expect("cursor CRTC state");
        assert!(cursor_state.in_flight.is_none());
        assert_eq!(
            recorder.records()[records_before_failed_render..]
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            [],
            "the failed attempt creates no commit or event obligation",
        );
        backend.platform.for_tests_accept_output_power_commits();
        Backend::set_dpms_power(&mut backend, 3)
            .expect("disable the output through Backend after render failure");
        assert!(!backend.platform.any_cursor_plane_visible());
        assert!(!backend.platform.any_cursor_commit_in_flight());
        assert!(!backend.scene.has_pending_page_flips());
        assert!(!backend.scene.cursor_transition_pending());
        assert_direct_queue_drained(&backend);
        let output = &backend.platform.outputs[0];
        let cursor_state = backend
            .platform
            .device_for_key(output.key.device_key)
            .expect("cursor device")
            .cursor
            .crtc_states
            .get(&output.output.crtc)
            .expect("cursor CRTC state");
        assert!(cursor_state.intent.is_none());
        assert!(cursor_state.lifecycle_pending.is_none());
        assert!(cursor_state.applied.is_none());
        assert!(cursor_state.in_flight.is_none());
        assert_commit_sequence(&recorder, &expected);
        let recorders = std::rc::Rc::clone(
            backend
                .platform
                .services
                .test_recorders
                .as_ref()
                .expect("recording test services"),
        );
        backend.platform.wait_idle_bounded();
        drop(backend);
        assert_eq!(recorders.cursor_buffer_ledger.end_state(), (0, 0, 0));
        return;
    }
    Backend::maybe_composite(&mut backend)
        .expect("compose software cursor and detach in one frame");
    assert!(
        backend.scene.has_pending_page_flips(),
        "expected a pending scene page flip: {}",
        tearing_composite_diagnostics(&backend, None),
    );
    assert!(backend.scene.cursor_transition_pending());
    assert_eq!(
        recorder.records().last().unwrap().kind,
        AtomicCommitKind::SceneWithCursor
    );
    assert!(!backend.platform.any_cursor_plane_visible());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Mixed,
        "the accepted SW receipt remains aggregate Mixed until its event retires",
    );
    assert_eq!(
        backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .unwrap()
            .mode,
        crate::kms::render::scene::CursorAcceptedMode::Software,
        "the accepted scene request already publishes its output-local SW mode",
    );
    let software_frame = recorder.records().last().unwrap().clone();
    let output = &backend.platform.outputs[0];
    let properties = backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(output.output.crtc))
        .expect("cursor properties");
    assert!(
        software_frame.properties.iter().any(|update| {
            update.object == properties.plane.into()
                && update.property == properties.fb_id
                && update.value == 0
        }),
        "software pixels and plane detach share the accepted scene request"
    );
    let records_before_detach_drain = recorder.records().len();
    Backend::warp_pointer_root(&mut backend, &mut state, 65, 64);
    Backend::maybe_composite(&mut backend).expect("hold motion behind accepted SW detach");
    assert_eq!(
        recorder.records().len(),
        records_before_detach_drain,
        "the SW+detach frame owns the CRTC until its event retires",
    );
    assert!(backend.platform.any_cursor_commit_in_flight());
    assert!(!backend.platform.any_cursor_plane_visible());
    assert!(recorder.records().iter().all(|record| {
        !record
            .flags
            .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_ASYNC)
    }));
    let sw_compose_time = backend
        .pstate
        .tearing
        .last_cursor_compose
        .expect("accepted SW+detach frame starts the compose interval");
    let interval = backend
        .pstate
        .tearing
        .cursor_compose_interval(&backend.platform);
    let interval_ns = u64::try_from(interval.as_nanos()).expect("vblank interval fits");
    let compose_offset_ns = u64::try_from(
        sw_compose_time
            .saturating_duration_since(clock_base)
            .as_nanos(),
    )
    .expect("compose offset fits");
    scheduler_time_ns.store(compose_offset_ns + interval_ns / 2, Ordering::SeqCst);
    Backend::warp_pointer_root(&mut backend, &mut state, 66, 64);
    assert_eq!(
        backend.pstate.tearing.deferred_cursor_wake,
        Some(sw_compose_time + interval),
        "accepted pending Software receipt keeps pacing while aggregate mode is Mixed",
    );
    Backend::maybe_composite(&mut backend).expect("hold cursor motion behind SW detach event");
    assert_eq!(recorder.records().len(), records_before_detach_drain);
    retire_composed_frame(&mut backend, &mut state, 12);
    assert!(!backend.platform.any_cursor_commit_in_flight());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );
    assert!(backend.scene.for_tests_cursor_footprint_was_presented(0));
    Backend::maybe_composite(&mut backend).expect("event retirement remains inside compose cap");
    assert_eq!(recorder.records().len(), records_before_detach_drain);
    scheduler_time_ns.store(compose_offset_ns + interval_ns, Ordering::SeqCst);
    Backend::maybe_composite(&mut backend).expect("compose the deferred cursor motion at deadline");
    assert_eq!(recorder.records().len(), records_before_detach_drain + 1);
    assert_eq!(
        recorder.records().last().unwrap().kind,
        AtomicCommitKind::Async
    );
    retire_composed_frame(&mut backend, &mut state, 13);

    assert!(backend.pstate.tearing.is_on());
    assert!(!backend.pstate.scanout_m2.active());
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert!(!backend.pstate.scanout_m2.unflip_shadow_ready);
    assert!(!backend.scene.has_pending_page_flips());
    assert_direct_queue_drained(&backend);
    expected.push(AtomicCommitKind::SceneWithCursor);
    expected.push(AtomicCommitKind::Async);
    assert_commit_sequence(&recorder, &expected);

    let output = &backend.platform.outputs[0];
    let cursor_device = backend
        .platform
        .device_for_key(output.key.device_key)
        .expect("cursor device");
    let cursor_state = cursor_device
        .cursor
        .crtc_states
        .get(&output.output.crtc)
        .expect("cursor CRTC state");
    assert!(cursor_state.applied.is_none());
    assert!(cursor_state.lifecycle_pending.is_none());
    assert!(cursor_state.intent.is_none());
    assert!(cursor_state.in_flight.is_none());
    assert!(
        cursor_device
            .cursor
            .plane
            .as_ref()
            .expect("cursor plane")
            .sprite_pool_users_are_zero()
    );
    assert!(!backend.platform.any_cursor_commit_in_flight());
    assert!(recorder.records().iter().all(|record| {
        record.kind != AtomicCommitKind::CursorOnly
            || (record
                .flags
                .contains(::drm::control::AtomicCommitFlags::NONBLOCK)
                && record
                    .flags
                    .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT))
    }));
    let recorders = std::rc::Rc::clone(
        backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("recording test services"),
    );
    backend.platform.wait_idle_bounded();
    drop(backend);
    assert_eq!(recorders.cursor_buffer_ledger.end_state(), (0, 0, 0));
}

pub(super) fn run_two_output_grouped_tearing_handoff_waits_for_each_event() {
    use yserver_core::{
        backend::{AnyHandle, Backend},
        server::ServerState,
    };
    use yserver_protocol::x11::{ClientId, ResourceId};

    const WINDOW: ResourceId = ResourceId(0x0100_78b6);
    let Some(mut backend) = live_scene_backend_with_outputs(2) else {
        return;
    };
    backend.platform.for_tests_accept_output_power_commits();
    for device in &mut backend.platform.devices {
        device.async_page_flip = true;
    }
    prime_cursorless_scene_primary(&mut backend);
    let recorder = recording_sink(&mut backend, 0);
    let mut state = ServerState::new();
    let host_window = materialize_test_cow_stage(&mut backend, &mut state, ClientId(1), WINDOW);

    let device_key = backend.platform.outputs[0].key.device_key;
    let device = std::rc::Rc::clone(
        &backend
            .platform
            .device_for_key(device_key)
            .expect("two-output cursor device")
            .device,
    );
    let show = std::rc::Rc::new(super::RecordingCursorShowExecutor::default());
    let mut plane = crate::kms::cursor_plane::CursorPlane::for_tests_with_show_executor(
        device, 64, 64, true, show,
    );
    install_test_atomic_cursor_properties(&backend, &mut plane, device_key);
    let hide = std::rc::Rc::new(RecordingCursorHideExecutor::default());
    hide.script([TestCursorHideResult::Ok; 8]);
    plane.for_tests_set_hide_executor(hide);
    backend.platform.devices[0].cursor.plane = Some(plane);

    let pixmap = Backend::create_pixmap(&mut backend, None, 32, 64, 64)
        .expect("create 64px cursor image through Backend");
    let cursor_pixels = [0x42, 0x83, 0xc4, 0xff].repeat(64 * 64);
    Backend::put_image(
        &mut backend,
        None,
        pixmap.as_raw(),
        32,
        64,
        64,
        0,
        0,
        &cursor_pixels,
    )
    .expect("paint 64px cursor image through Backend");
    let picture =
        Backend::render_create_picture(&mut backend, None, AnyHandle::Pixmap(pixmap), 0, 0, &[])
            .expect("create cursor picture through Backend")
            .expect("cursor picture handle");
    let cursor = Backend::render_create_cursor(&mut backend, None, picture, 0, 0)
        .expect("create shared cursor through Backend")
        .expect("cursor handle");
    let root = backend.core.window_id;
    Backend::define_cursor(&mut backend, None, root, cursor.as_raw())
        .expect("define cursor across both outputs");
    Backend::warp_pointer_root(&mut backend, &mut state, 780, 100);
    damage_root(&mut backend);
    Backend::maybe_composite(&mut backend).expect("bind shared cursor to both CRTCs");
    assert!(backend.scene.has_pending_page_flip(0));
    assert!(backend.scene.has_pending_page_flip(1));
    retire_composed_frame(&mut backend, &mut state, 1);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw
    );
    assert!(backend.platform.any_cursor_plane_visible());
    assert_eq!(
        backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(
                backend.platform.devices[0].cursor.crtc_states
                    [&backend.platform.outputs[0].output.crtc]
                    .applied
                    .unwrap()
                    .lease
            ),
        Some(2),
        "both CRTCs retain the same cursor lease",
    );
    recorder.clear_records();

    fn present_full_root(
        backend: &mut KmsBackend,
        present_id: u64,
        window: u32,
        options: u32,
    ) -> bool {
        use yserver_core::backend::Backend;
        let (width, height) = backend.platform.fb_dimensions();
        let (candidate, event, _) = prepare_direct_present_for_ids_with_storage(
            backend, present_id, window, window, width, height, options, true,
        );
        Backend::try_present_direct(backend, candidate, event).expect("Backend full-root Present")
    }

    for index in 0..7_u64 {
        assert!(!present_full_root(
            &mut backend,
            17_600 + index,
            host_window,
            0
        ));
        assert!(recorder.records().is_empty());
    }
    assert!(present_full_root(&mut backend, 17_607, host_window, 0));
    retire_direct_present(&mut backend, &mut state, 20);
    assert!(backend.pstate.scanout_m2.current.is_some());
    for index in 0..7_u64 {
        assert!(present_full_root(
            &mut backend,
            17_608 + index,
            host_window,
            yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
        ));
        retire_direct_present(&mut backend, &mut state, 21 + index as u32);
        assert!(backend.pstate.scanout_m2.current.is_some());
        assert!(backend.platform.any_cursor_plane_visible());
        assert!(!backend.pstate.tearing.is_on());
    }
    assert!(!present_full_root(
        &mut backend,
        17_615,
        host_window,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    ));
    assert!(backend.pstate.tearing.is_on());
    assert!(backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.platform.any_cursor_plane_visible());
    retire_direct_present(&mut backend, &mut state, 30);
    assert!(backend.pstate.scanout_m2.pending.is_none());
    Backend::maybe_composite(&mut backend).expect("submit two-CRTC grouped unflip");
    let before_unflip_event = recorder.records().len();
    let grouped_unflip = recorder.records().last().unwrap().clone();
    assert_eq!(grouped_unflip.kind, AtomicCommitKind::Scene);
    assert_eq!(grouped_unflip.events.len(), 2);
    assert_eq!(backend.pstate.scanout_m2.unflip_awaiting_outputs.len(), 2);
    assert!(backend.platform.any_cursor_plane_visible());

    retire_recorded_output_flip(&mut backend, &mut state, 0, 31);
    assert_eq!(backend.pstate.scanout_m2.unflip_awaiting_outputs.len(), 1);
    assert!(backend.pstate.scanout_m2.current.is_some());
    Backend::maybe_composite(&mut backend).expect("wait for the second grouped CRTC event");
    assert_eq!(recorder.records().len(), before_unflip_event);
    assert!(backend.platform.any_cursor_plane_visible());
    assert!(!backend.platform.any_cursor_commit_in_flight());

    retire_recorded_output_flip(&mut backend, &mut state, 1, 32);
    assert!(backend.pstate.scanout_m2.current.is_none());
    assert!(backend.platform.any_cursor_plane_visible());
    Backend::maybe_composite(&mut backend).expect("submit per-output SW+detach frames");
    backend.platform.wait_idle_bounded();
    Backend::on_scanout_render_completion(&mut backend, &mut state);
    for _ in 0..2 {
        if recorder.records().len() >= before_unflip_event + 3 {
            break;
        }
        Backend::maybe_composite(&mut backend).expect("advance the other output's SW frame");
        backend.platform.wait_idle_bounded();
        Backend::on_scanout_render_completion(&mut backend, &mut state);
    }
    let accepted_sw = recorder.records();
    let sw_records = &accepted_sw[before_unflip_event..];
    assert_eq!(
        sw_records.len(),
        2,
        "per-output handoff records: {:?}; pending=[{}, {}]",
        sw_records
            .iter()
            .map(|record| (record.kind, &record.events))
            .collect::<Vec<_>>(),
        backend.scene.has_pending_page_flip(0),
        backend.scene.has_pending_page_flip(1),
    );
    assert!(
        sw_records
            .iter()
            .all(|record| record.kind == AtomicCommitKind::SceneWithCursor)
    );
    assert!(sw_records.iter().all(|record| record.events.len() == 1));
    for output_idx in 0..2 {
        let receipt = backend
            .scene
            .for_tests_cursor_commit_receipt(output_idx)
            .unwrap();
        assert_eq!(
            receipt.mode,
            crate::kms::render::scene::CursorAcceptedMode::Software
        );
        assert!(matches!(
            receipt.primary_cursor,
            crate::kms::render::scene::CursorPrimaryContent::Software { .. }
        ));
        let output = &backend.platform.outputs[output_idx];
        let properties = backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .properties_for(output.output.crtc)
            .unwrap();
        assert!(sw_records.iter().any(|record| {
            record
                .events
                .iter()
                .any(|event| event.crtc == output.output.crtc)
                && record.properties.iter().any(|update| {
                    update.object == properties.plane.into()
                        && update.property == properties.fb_id
                        && update.value == 0
                })
        }));
    }
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Mixed,
        "each accepted SW receipt stays pending until its own event",
    );
    assert!(!backend.platform.any_cursor_plane_visible());
    assert!(backend.platform.cursor_commit_in_flight_for_output(0));
    assert!(backend.platform.cursor_commit_in_flight_for_output(1));
    let before_detach_events = recorder.records().len();
    Backend::maybe_composite(&mut backend).expect("hold both SW detach obligations");
    assert_eq!(recorder.records().len(), before_detach_events);
    assert!(
        recorder.records()[before_unflip_event..]
            .iter()
            .all(|record| !record
                .flags
                .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_ASYNC))
    );

    retire_recorded_output_flip(&mut backend, &mut state, 0, 33);
    assert!(!backend.platform.cursor_commit_in_flight_for_output(0));
    assert!(backend.platform.cursor_commit_in_flight_for_output(1));
    Backend::maybe_composite(&mut backend).expect("second detach event still owns its CRTC");
    assert_eq!(recorder.records().len(), before_detach_events);
    retire_recorded_output_flip(&mut backend, &mut state, 1, 34);
    assert!(!backend.platform.any_cursor_commit_in_flight());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );
    assert!(!backend.scene.has_pending_page_flips());
    assert_eq!(
        recorder
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [
            vec![AtomicCommitKind::Scene; 9],
            vec![AtomicCommitKind::SceneWithCursor; 2],
        ]
        .concat(),
        "the Vsync scene sequence ends with exactly one SW+detach primary per CRTC",
    );

    Backend::set_cursor_hidden(&mut backend, true);
    Backend::maybe_composite(&mut backend).expect("hide SW cursor through Backend");
    for output_idx in 0..2 {
        if backend.scene.has_pending_page_flip(output_idx) {
            retire_recorded_output_flip(
                &mut backend,
                &mut state,
                output_idx,
                35 + u32::try_from(output_idx).unwrap(),
            );
        }
    }
    backend
        .disable_output()
        .expect("disable and drain grouped outputs through Backend");
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.platform.any_cursor_commit_in_flight());
    assert!(backend.pstate.scanout_m2.current.is_none());
    assert!(backend.pstate.scanout_m2.pending.is_none());
    assert!(backend.pstate.scanout_m2.queued_successor.is_none());
    assert_direct_queue_drained(&backend);
    for output in &backend.platform.outputs {
        let cursor_state = backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&output.output.crtc)
            .expect("output cursor state");
        assert!(cursor_state.applied.is_none());
        assert!(cursor_state.lifecycle_pending.is_none());
        assert!(cursor_state.intent.is_none());
        assert!(cursor_state.in_flight.is_none());
    }
    let recorders = std::rc::Rc::clone(
        backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("recording test services"),
    );
    backend.platform.wait_idle_bounded();
    drop(backend);
    assert_eq!(recorders.cursor_buffer_ledger.end_state(), (0, 0, 0));
}
