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
    let expected_animation_pixels = fixture.backend.anim_cursor_records[&animation.as_raw()].frames
        [1]
    .record
    .bgra_bytes
    .clone();
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
        fixture.backend.cursor_records[&animation.as_raw()].bgra_bytes,
        expected_animation_pixels,
        "retirement preserves the current animation frame's pixels",
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
// Mutations: make hotspot-only changes wait for scene damage, skip event-retry acceptance receipts, or require an HW-visible primary before show-after-hide.
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

    // The hide is queued behind a real primary flip. Its event retries the
    // cursor intent, and that accepted retry must publish Hidden bookkeeping.
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit primary before hide");
    assert!(fixture.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::Scene,
    );
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("hold hide behind primary flip");
    assert_eq!(fixture.recorders.atomic.records().len(), 4);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    fixture.retire_output(0, 5);
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
        "the event-triggered accepted detach publishes its cursorless receipt",
    );
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly,
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    fixture.retire_output(0, 6);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );

    // A later show is HW-only even though the accepted primary is still
    // cursorless. Then queue another hide behind that show's actual event.
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    Backend::maybe_composite(&mut fixture.backend).expect("show from Hidden without a primary");
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly,
    );
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw,
    );
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("hold hide behind show event");
    let before_show_retirement = fixture.recorders.atomic.records().len();
    fixture.retire_output(0, 7);
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        before_show_retirement + 1
    );
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly,
    );
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
        "the show-event retry publishes the accepted hide",
    );
    fixture.retire_output(0, 8);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );

    use crate::drm::atomic::AtomicCommitKind;
    let expected = [
        AtomicCommitKind::SceneWithCursor,
        AtomicCommitKind::CursorOnly,
        AtomicCommitKind::CursorOnly,
        AtomicCommitKind::Scene,
        AtomicCommitKind::CursorOnly,
        AtomicCommitKind::CursorOnly,
        AtomicCommitKind::CursorOnly,
    ];
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        expected,
        "every image, hotspot, hide, show, and retry has one accepted delta",
    );
    fixture
        .backend
        .disable_output()
        .expect("disable output after cursor-only show/hide sequence");
    fixture.assert_cursor_quiescent();
    let mut expected_with_disable = expected.to_vec();
    expected_with_disable.push(AtomicCommitKind::Scene);
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        expected_with_disable,
    );
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
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
    Backend::maybe_composite(&mut fixture.backend).expect("attach visible mixed animation");
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly,
        "a visible cursor selected from a cursorless Hidden primary attaches without a scene frame",
    );
    retire_cursor_only(&mut fixture, 4);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("XFIXES hide visible mixed cursor");
    retire_cursor_only(&mut fixture, 5);
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    Backend::maybe_composite(&mut fixture.backend).expect("XFIXES show visible mixed cursor");
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly,
    );
    retire_cursor_only(&mut fixture, 6);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    finish_hw_cursor(
        fixture,
        7,
        &[
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
        ],
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: require a successor primary for a held-direct hotspot, hide, or show; or fail to retire each cursor-only event before cleanup.
fn atomic_cursor_only_held_direct_hotspot_and_visibility_progress() {
    use crate::drm::atomic::AtomicCommitKind;

    let Some(mut fixture) = AtomicCursorFixture::new_with_hotspot_properties()
        .expect("construct direct cursor fixture with hotspot properties")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let direct = super::round_a::setup_cursor_only_direct_present(&mut fixture);
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit visible cursor primary");
    fixture.retire_output(0, 1);
    if fixture
        .backend
        .platform
        .cursor_atomic_event_pending_for_output(0)
    {
        fixture.retire_output(0, 2);
    }

    let mut direct_entered = false;
    for attempt in 0..16_u64 {
        let (candidate, event) = super::round_a::direct_present_for_id(&direct, 26_000 + attempt);
        Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
        if Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit real direct Present through Backend")
        {
            fixture.retire_output(0, 20 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend).expect("settle direct Present");
            if fixture.backend.pstate.scanout_m2.active() {
                direct_entered = true;
                break;
            }
        } else if fixture.backend.scene.has_pending_page_flips() {
            fixture.retire_output(0, 40 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle composed direct-probation frame");
        }
    }
    assert!(direct_entered, "Backend Presents hold a Vsync direct frame");
    assert!(fixture.backend.pstate.scanout_m2.active());
    let before_intents = fixture.recorders.atomic.records().len();

    let replacement = create_argb_cursor(&mut fixture.backend, [0x51, 0x62, 0x73, 0xff], 1, 2);
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, replacement.as_raw())
        .expect("replace the held-direct cursor and hotspot through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit held-direct hotspot update");
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        AtomicCommitKind::CursorOnly,
    );
    fixture.retire_output(0, 60);

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("hide cursor on held direct frame");
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        AtomicCommitKind::CursorOnly,
    );
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    fixture.retire_output(0, 61);

    Backend::set_cursor_hidden(&mut fixture.backend, false);
    Backend::maybe_composite(&mut fixture.backend).expect("show cursor on held direct frame");
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        AtomicCommitKind::CursorOnly,
    );
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    fixture.retire_output(0, 62);

    assert_eq!(
        fixture.recorders.atomic.records()[before_intents..]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            AtomicCommitKind::CursorOnly,
            AtomicCommitKind::CursorOnly,
            AtomicCommitKind::CursorOnly,
        ],
        "held-direct image/hotspot, hide, and show each progress once",
    );

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("detach before direct cleanup");
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        AtomicCommitKind::CursorOnly,
    );
    fixture.retire_output(0, 63);
    Backend::unmap_subwindow(&mut fixture.backend, None, direct.window_xid)
        .expect("request held-direct unflip through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit Backend direct unflip");
    fixture.retire_output(0, 64);
    assert!(!fixture.backend.pstate.scanout_m2.active());
    fixture
        .backend
        .disable_output()
        .expect("disable output after direct cursor handoff");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: use only cursor_hidden, turn cursor wakes into generic damage, reset the cap per observation, cap XTEST deadline or CopyArea progress, replace run_core WarpPointer with Backend warps, replace the sink after On entry, or omit final Backend cleanup.
fn atomic_cursor_only_tearing_cursor_cap_survives_all_motion_sources() {
    assert!(
        super::super::tearing::round2::run_task6_present_and_xtest_motion_keep_cursor_compose_cap_under_run_core_load(false),
        "run_core exercises root/window warp, XTEST, Present/Copy, structural damage, and generic wakes with construction-selected dependencies and full cleanup",
    );
    assert!(
        super::super::tearing::round2::run_task5_handoff_unflip_starts_cursor_compose_interval(),
        "the accepted temporary direct handoff starts the shared cursor compose interval",
    );
}
