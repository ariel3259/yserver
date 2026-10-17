use super::{round_a::damage_cursor_root, support::AtomicCursorFixture};
use std::{cell::Cell, rc::Rc, time::Instant};
use yserver_core::backend::{AnyHandle, Backend, CursorHandle};

fn create_argb_cursor(
    backend: &mut crate::kms::render::backend::KmsBackend,
    pixel: [u8; 4],
    hot_x: u16,
    hot_y: u16,
) -> CursorHandle {
    let source =
        Backend::create_pixmap(backend, None, 32, 4, 4).expect("create ARGB cursor pixmap");
    Backend::put_image(
        backend,
        None,
        source.as_raw(),
        32,
        4,
        4,
        0,
        0,
        &pixel.repeat(16),
    )
    .expect("write ARGB cursor pixels");
    let picture =
        Backend::render_create_picture(backend, None, AnyHandle::Pixmap(source), 0, 0, &[])
            .expect("create ARGB cursor picture")
            .expect("picture handle");
    Backend::render_create_cursor(backend, None, picture, hot_x, hot_y)
        .expect("create ARGB cursor")
        .expect("cursor handle")
}

fn begin_hw_cursor(fixture: &mut AtomicCursorFixture) -> u32 {
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let cursor = fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor frame");
    assert_eq!(
        fixture.recorders.atomic.records()[0].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
    );
    fixture.retire_output(0, 1);
    fixture.retire_output(0, 2);
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw,
    );
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    cursor
}

fn retire_cursor_only(fixture: &mut AtomicCursorFixture, frame: u32) {
    use crate::drm::atomic::AtomicCommitKind;

    let records = fixture.recorders.atomic.records();
    assert_eq!(
        records.last().expect("cursor commit record").kind,
        AtomicCommitKind::CursorOnly,
        "recorded kinds: {:?}; mode: {:?}; HW output: {}; scene dirty: {}",
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        fixture.backend.scene.cursor_mode(),
        fixture.backend.scene.output_uses_hardware_cursor(0),
        fixture.backend.for_tests_scene_wants_scene(),
    );
    assert_eq!(records.last().unwrap().events.len(), 1);
    fixture.retire_output(0, frame);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
}

fn finish_hw_cursor(
    fixture: AtomicCursorFixture,
    last_frame: u32,
    expected_before_hide: &[crate::drm::atomic::AtomicCommitKind],
) {
    use crate::drm::atomic::AtomicCommitKind;

    let mut fixture = fixture;
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("submit Backend cursor hide");
    retire_cursor_only(&mut fixture, last_frame);
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
    );
    let mut expected = expected_before_hide.to_vec();
    expected.push(AtomicCommitKind::CursorOnly);
    let records = fixture.recorders.atomic.records();
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        expected,
        "the idle cursor sequence contains one commit per accepted state delta",
    );
    fixture
        .backend
        .disable_output()
        .expect("disable output through Backend");
    fixture.assert_cursor_quiescent();
    let sequence: Vec<_> = fixture
        .recorders
        .atomic
        .records()
        .iter()
        .map(|record| record.kind)
        .collect();
    expected.push(AtomicCommitKind::Scene);
    assert_eq!(
        sequence, expected,
        "hide and disable produce the exact cleanup trace"
    );
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

fn transparent_bitmap_cursor(
    backend: &mut crate::kms::render::backend::KmsBackend,
) -> CursorHandle {
    let source = Backend::create_pixmap(backend, None, 1, 1, 1).expect("create source bitmap");
    let mask = Backend::create_pixmap(backend, None, 1, 1, 1).expect("create mask bitmap");
    Backend::put_image(backend, None, source.as_raw(), 1, 1, 1, 0, 0, &[0; 4])
        .expect("write transparent source bit");
    Backend::put_image(backend, None, mask.as_raw(), 1, 1, 1, 0, 0, &[0; 4])
        .expect("write transparent mask bit");
    Backend::create_cursor(
        backend,
        None,
        source,
        Some(mask),
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
        0,
        0,
    )
    .expect("create transparent cursor")
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: require scene damage for sprite uploads or publish an older animation record at retirement.
fn atomic_cursor_only_stationary_hw_image_and_animation_make_progress() {
    let now = Rc::new(Cell::new(Instant::now()));
    let clock_now = Rc::clone(&now);
    let clock: Rc<dyn Fn() -> Instant> = Rc::new(move || clock_now.get());
    let Some(mut fixture) = AtomicCursorFixture::new_with_clock(clock)
        .expect("construct clock-controlled cursor fixture")
    else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let first_binding = fixture.backend.platform.devices[0].cursor.crtc_states
        [&fixture.backend.platform.outputs[0].output.crtc]
        .applied
        .expect("initial HW cursor binding");

    let replacement = create_argb_cursor(&mut fixture.backend, [0x11, 0x22, 0x33, 0xff], 0, 0);
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, replacement.as_raw())
        .expect("replace stationary cursor through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("schedule stationary image update");
    assert!(!fixture.backend.scene.has_pending_page_flips());
    retire_cursor_only(&mut fixture, 3);
    let replacement_binding = fixture.backend.platform.devices[0].cursor.crtc_states
        [&fixture.backend.platform.outputs[0].output.crtc]
        .applied
        .expect("replacement binding accepted");
    assert_ne!(replacement_binding.lease, first_binding.lease);
    assert_ne!(replacement_binding.framebuffer, first_binding.framebuffer);
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
        ],
        "a stationary image replacement uses one cursor-only commit",
    );

    let next = create_argb_cursor(&mut fixture.backend, [0xaa, 0xbb, 0xcc, 0xff], 0, 0);
    let animation =
        Backend::create_anim_cursor(&mut fixture.backend, None, &[(replacement, 40), (next, 40)])
            .expect("create animation through Backend")
            .expect("animation handle");
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, animation.as_raw())
        .expect("select animation through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit animation's current image");
    retire_cursor_only(&mut fixture, 4);
    let animation_first = fixture.backend.platform.devices[0].cursor.crtc_states
        [&fixture.backend.platform.outputs[0].output.crtc]
        .applied
        .expect("animation frame zero binding");

    let deadline = Backend::next_wakeup(&fixture.backend).expect("animation deadline is scheduled");
    now.set(deadline);
    Backend::maybe_composite(&mut fixture.backend).expect("advance animation through scheduler");
    retire_cursor_only(&mut fixture, 5);
    let animation_second = fixture.backend.platform.devices[0].cursor.crtc_states
        [&fixture.backend.platform.outputs[0].output.crtc]
        .applied
        .expect("animation frame one binding");
    assert_ne!(animation_second.framebuffer, animation_first.framebuffer);
    assert_eq!(
        fixture.backend.cursor_records[&animation.as_raw()].version,
        fixture
            .backend
            .platform
            .cursor_plane_uploaded_version_for_output(0)
            .expect("uploaded animation frame version"),
        "the accepted sprite identity is the newest animation record",
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
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
        ],
    );

    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit unrelated primary frame");
    assert!(fixture.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::Scene,
    );
    let waiting_first = create_argb_cursor(&mut fixture.backend, [0x10, 0x20, 0x30, 0xff], 0, 0);
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, waiting_first.as_raw())
        .expect("queue first image while a primary flip is pending");
    Backend::maybe_composite(&mut fixture.backend).expect("hold first image behind primary flip");
    let waiting_latest = create_argb_cursor(&mut fixture.backend, [0x40, 0x50, 0x60, 0xff], 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, waiting_latest.as_raw())
        .expect("coalesce newest image while primary flip is pending");
    Backend::maybe_composite(&mut fixture.backend).expect("hold newest image behind primary flip");
    assert_eq!(fixture.recorders.atomic.records().len(), 5);
    assert!(fixture.backend.scene.has_pending_page_flips());
    fixture.retire_output(0, 6);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("submit coalesced cursor intent after primary retirement");
    retire_cursor_only(&mut fixture, 7);
    let latest_version = fixture.backend.cursor_records[&waiting_latest.as_raw()].version;
    assert_eq!(
        fixture
            .backend
            .platform
            .cursor_plane_uploaded_version_for_output(0),
        Some(latest_version),
        "the held stream uploads the newest coalesced sprite identity",
    );

    let scene_owned = create_argb_cursor(&mut fixture.backend, [0x70, 0x80, 0x90, 0xff], 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, scene_owned.as_raw())
        .expect("queue a cursor image before the next frame is built");
    damage_cursor_root(&mut fixture);
    let before_scene_owned = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend).expect("let the scene frame own the update");
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        before_scene_owned + 1,
        "the ready primary frame submits once and owns the cursor update",
    );
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
    );
    assert!(fixture.backend.scene.has_pending_page_flips());
    let receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted scene cursor receipt");
    assert!(
        receipt.frame.is_some(),
        "frame-owned cursor receipt keeps its frame ID"
    );
    assert_eq!(
        receipt.mode,
        crate::kms::render::scene::CursorAcceptedMode::Hardware
    );
    fixture.retire_output(0, 8);
    assert_eq!(
        fixture
            .backend
            .platform
            .cursor_plane_uploaded_version_for_output(0),
        Some(fixture.backend.cursor_records[&scene_owned.as_raw()].version),
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
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
        "exact sequence separates held intent, cursor-only progress, and frame ownership",
    );
    finish_hw_cursor(
        fixture,
        9,
        &[
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: make hotspot-only changes wait for scene damage or retain the old FB/hotspot after the cursor-only receipt.
fn atomic_cursor_only_hw_visibility_and_hotspot_need_no_scene_frame() {
    let Some(mut fixture) = AtomicCursorFixture::new_with_hotspot_properties()
        .expect("construct cursor fixture with hotspot properties")
    else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    assert!(
        !fixture.backend.scene.owes_repaint(),
        "the hotspot scenario starts with an idle primary",
    );
    let root = fixture.backend.core.window_id;
    let pixels = [0x31, 0x42, 0x53, 0xff];
    let base = create_argb_cursor(&mut fixture.backend, pixels, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, base.as_raw())
        .expect("select base image through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit base image");
    retire_cursor_only(&mut fixture, 3);

    let hotspot = create_argb_cursor(&mut fixture.backend, pixels, 1, 2);
    Backend::define_cursor(&mut fixture.backend, None, root, hotspot.as_raw())
        .expect("change only hotspot through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit hotspot-only update");
    assert!(!fixture.backend.scene.has_pending_page_flips());
    retire_cursor_only(&mut fixture, 4);
    let binding = fixture.backend.platform.devices[0].cursor.crtc_states
        [&fixture.backend.platform.outputs[0].output.crtc]
        .applied
        .expect("hotspot replacement accepted");
    assert_eq!((binding.hot_x, binding.hot_y), (1, 2));
    assert_eq!((binding.x, binding.y), (99, 88));
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), 3);
    assert_eq!(
        records[1].kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    assert_eq!(
        records[2].kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    assert!(records[2].properties.iter().any(|update| {
        update.object == binding.properties.plane.into()
            && update.property == binding.properties.fb_id
            && update.value == u64::from(binding.framebuffer)
    }));
    if let Some(hot_x) = binding.properties.hotspot_x {
        assert!(records[2].properties.iter().any(|update| {
            update.object == binding.properties.plane.into()
                && update.property == hot_x
                && update.value == 1
        }));
    }
    if let Some(hot_y) = binding.properties.hotspot_y {
        assert!(records[2].properties.iter().any(|update| {
            update.object == binding.properties.plane.into()
                && update.property == hot_y
                && update.value == 2
        }));
    }
    finish_hw_cursor(
        fixture,
        5,
        &[
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
        ],
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: classify only by XFIXES visibility, allowing transparent core, RENDER, glyph, or all-transparent animation records to attach.
fn atomic_cursor_only_effective_visibility_overrides_hw_intent() {
    let Some(mut fixture) = AtomicCursorFixture::new().expect("construct cursor fixture") else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    assert!(
        !fixture.backend.scene.owes_repaint(),
        "the effective visibility scenario starts with an idle primary",
    );
    let root = fixture.backend.core.window_id;

    let transparent = transparent_bitmap_cursor(&mut fixture.backend);
    assert!(fixture.backend.cursor_records[&transparent.as_raw()].is_fully_transparent);
    Backend::define_cursor(&mut fixture.backend, None, root, transparent.as_raw())
        .expect("select transparent core cursor");
    assert!(
        !fixture.backend.for_tests_scene_wants_scene(),
        "transparent cursor selection leaves an otherwise idle primary alone",
    );
    Backend::maybe_composite(&mut fixture.backend).expect("detach transparent cursor");
    retire_cursor_only(&mut fixture, 3);
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
    );
    let settled_records = fixture.recorders.atomic.records().len();

    let source = Backend::create_pixmap(&mut fixture.backend, None, 32, 1, 1)
        .expect("create transparent RENDER cursor pixmap");
    Backend::put_image(
        &mut fixture.backend,
        None,
        source.as_raw(),
        32,
        1,
        1,
        0,
        0,
        &[0; 4],
    )
    .expect("write transparent RENDER pixel");
    let picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(source),
        0,
        0,
        &[],
    )
    .expect("create transparent RENDER picture")
    .expect("picture handle");
    let render_cursor = Backend::render_create_cursor(&mut fixture.backend, None, picture, 0, 0)
        .expect("create transparent RENDER cursor")
        .expect("cursor handle");
    Backend::define_cursor(&mut fixture.backend, None, root, render_cursor.as_raw())
        .expect("select transparent RENDER cursor");
    Backend::maybe_composite(&mut fixture.backend).expect("keep transparent RENDER cursor hidden");
    assert_eq!(fixture.recorders.atomic.records().len(), settled_records);

    let missing_font = yserver_core::backend::FontHandle::from_raw(0xb410).unwrap();
    let glyph = Backend::create_glyph_cursor(
        &mut fixture.backend,
        None,
        missing_font,
        None,
        0,
        0,
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
    )
    .expect("create transparent glyph cursor");
    assert!(fixture.backend.cursor_records[&glyph.as_raw()].is_fully_transparent);
    let all_transparent = Backend::create_anim_cursor(
        &mut fixture.backend,
        None,
        &[(transparent, 40), (glyph, 40)],
    )
    .expect("create all-transparent animation")
    .expect("animation handle");
    assert!(fixture.backend.anim_cursor_records[&all_transparent.as_raw()].is_fully_transparent);
    Backend::define_cursor(&mut fixture.backend, None, root, all_transparent.as_raw())
        .expect("select all-transparent animation");
    Backend::maybe_composite(&mut fixture.backend).expect("retain hidden animated cursor");
    assert_eq!(fixture.recorders.atomic.records().len(), settled_records);

    let opaque = create_argb_cursor(&mut fixture.backend, [0x91, 0x82, 0x73, 0xff], 0, 0);
    let mixed = Backend::create_anim_cursor(
        &mut fixture.backend,
        None,
        &[(transparent, 40), (opaque, 40)],
    )
    .expect("create mixed animation")
    .expect("animation handle");
    Backend::define_cursor(&mut fixture.backend, None, root, mixed.as_raw())
        .expect("select mixed animation");
    assert!(!fixture.backend.effective_cursor_hidden());
    Backend::maybe_composite(&mut fixture.backend).expect("submit visible mixed animation frame");
    assert!(fixture.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        "a visible cursor selected from Hidden is attached by its scene frame",
    );
    fixture.retire_output(0, 4);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("XFIXES hide visible mixed cursor");
    retire_cursor_only(&mut fixture, 5);
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    Backend::maybe_composite(&mut fixture.backend).expect("XFIXES show visible mixed cursor frame");
    assert!(fixture.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
    );
    fixture.retire_output(0, 6);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    finish_hw_cursor(
        fixture,
        7,
        &[
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: use only cursor_hidden for effective visibility, route On motion to ordinary HW intent, turn cursor wakes into mark_dirty damage, reset the cap clock per observation, or cap client damage.
fn atomic_cursor_only_tearing_cursor_cap_survives_all_motion_sources() {
    // Exercise the accepted-HW-cursor On handoff and Backend host/root motion
    // here; run_core adds XTEST, Presents, and generic wakes under its clock.
    let now = Rc::new(Cell::new(Instant::now()));
    let clock_now = Rc::clone(&now);
    let clock: Rc<dyn Fn() -> Instant> = Rc::new(move || clock_now.get());
    let Some(on_fixture) = super::super::tearing::enter_on_after_accepted_hw_cursor(clock) else {
        return;
    };
    let mut backend = on_fixture.backend;
    let mut state = on_fixture.state;
    let recorder = on_fixture.recorder;
    let cursor_plane = on_fixture.cursor_plane.into();
    let interval = backend
        .pstate
        .tearing
        .cursor_compose_interval(&backend.platform);
    let first_compose = backend.monotonic_now() + interval;
    now.set(first_compose);
    Backend::set_cursor_hidden(&mut backend, false);
    Backend::on_host_input(
        &mut backend,
        &mut state,
        yserver_core::core_loop::HostInputEvent::PointerMotion {
            origin: yserver_core::core_loop::InputOrigin::NestedHost,
            x: 110,
            y: 100,
            time: 1,
            relative: false,
            dx: 10,
            dy: 10,
            motion_delta: None,
        },
    );
    Backend::maybe_composite(&mut backend).expect("compose first On cursor motion");
    super::super::tearing::retire_composed_frame(&mut backend, &mut state, 21);
    let last_compose = backend
        .pstate
        .tearing
        .last_cursor_compose
        .expect("first On cursor compose records its timestamp");
    let before_deadline = recorder.records().len();
    let deadline = last_compose + interval;
    now.set(last_compose + interval / 2);
    Backend::warp_pointer_root(&mut backend, &mut state, 140, 120);
    // The X11 request handler resolves window coordinates through ServerState
    // and calls this Backend root warp entry for both root and child targets.
    Backend::warp_pointer_root(&mut backend, &mut state, 150, 130);
    Backend::on_host_input(
        &mut backend,
        &mut state,
        yserver_core::core_loop::HostInputEvent::PointerMotion {
            origin: yserver_core::core_loop::InputOrigin::NestedHost,
            x: 155,
            y: 134,
            time: 2,
            relative: false,
            dx: 5,
            dy: 4,
            motion_delta: None,
        },
    );
    assert_eq!(backend.pstate.tearing.deferred_cursor_wake, Some(deadline),);
    Backend::maybe_composite(&mut backend).expect("hold cursor-only compose to cap");
    assert_eq!(recorder.records().len(), before_deadline);
    assert!(!backend.scene.has_pending_page_flips());

    now.set(deadline);
    Backend::maybe_composite(&mut backend).expect("compose latest cursor at deadline");
    assert_eq!(recorder.records().len(), before_deadline + 1);
    assert_eq!(
        recorder.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::Async,
    );
    super::super::tearing::retire_composed_frame(&mut backend, &mut state, 22);
    assert_eq!(
        (backend.core.cursor_x as i32, backend.core.cursor_y as i32),
        (155, 134),
        "the deadline compose observes the newest host motion position",
    );
    assert_eq!(backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(backend.pstate.tearing.is_on());
    assert!(!backend.platform.any_cursor_plane_visible());
    let kinds: Vec<_> = recorder
        .records()
        .iter()
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            crate::drm::atomic::AtomicCommitKind::Async,
            crate::drm::atomic::AtomicCommitKind::Async,
        ]
    );
    assert!(recorder.records().iter().all(|record| {
        !record
            .properties
            .iter()
            .any(|update| update.object == cursor_plane)
    }));
    backend.platform.for_tests_accept_output_power_commits();
    backend
        .disable_output()
        .expect("disable output after On cursor cap test");
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.platform.any_cursor_commit_in_flight());
    for device in &backend.platform.devices {
        for state in device.cursor.crtc_states.values() {
            assert!(state.intent.is_none());
            assert!(state.in_flight.is_none());
        }
    }
    let expected = vec![
        crate::drm::atomic::AtomicCommitKind::Async,
        crate::drm::atomic::AtomicCommitKind::Async,
    ];
    assert_eq!(
        recorder
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        expected,
        "On motion is held to the deadline and disable drains exactly once",
    );
    let recorders = Rc::clone(
        backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("recording KMS fixture"),
    );
    let ledger = Rc::clone(&recorders.cursor_buffer_ledger);
    drop(backend);
    assert_eq!(ledger.end_state(), (0, 0, 0));
    assert!(
        super::super::tearing::round2::run_task6_present_and_xtest_motion_keep_cursor_compose_cap_under_run_core_load(false),
        "run_core producer stream exercises Present, XTEST, and generic request wakes",
    );
    assert!(
        super::super::tearing::round2::run_task5_handoff_unflip_starts_cursor_compose_interval(),
        "the accepted temporary direct handoff starts the shared cursor compose interval",
    );
}
