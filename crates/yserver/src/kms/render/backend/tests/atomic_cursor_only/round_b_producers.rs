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

fn create_argb_cursor_extent(
    backend: &mut crate::kms::render::backend::KmsBackend,
    pixel: [u8; 4],
    width: u16,
    height: u16,
    hot_x: u16,
    hot_y: u16,
) -> CursorHandle {
    let source = Backend::create_pixmap(backend, None, 32, width, height)
        .expect("create ARGB cursor pixmap");
    Backend::put_image(
        backend,
        None,
        source.as_raw(),
        32,
        width,
        height,
        0,
        0,
        &pixel.repeat(usize::from(width) * usize::from(height)),
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

fn retire_scene_outputs(fixture: &mut AtomicCursorFixture, frame: u32) {
    let output_count = fixture.backend.platform.outputs.len();
    for output_idx in 0..output_count {
        if fixture.backend.scene.has_pending_page_flip(output_idx) {
            fixture.retire_output(output_idx, frame + u32::try_from(output_idx).unwrap());
        }
    }
}

fn admit_held_direct_cursorless(
    fixture: &mut AtomicCursorFixture,
    direct: &super::round_a::CursorOnlyDirectPresentSetup,
    first_present_id: u64,
    first_frame: u32,
) -> u64 {
    let output_count = fixture.backend.platform.outputs.len();
    for attempt in 0..16_u64 {
        let present_id = first_present_id + attempt;
        let (candidate, event) = super::round_a::direct_present_for_id(direct, present_id);
        Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
        if Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit real direct Present through Backend")
        {
            for output_idx in 0..output_count {
                if fixture
                    .backend
                    .pstate
                    .scanout_m2
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.awaiting_outputs.contains(&output_idx))
                {
                    fixture
                        .retire_output(output_idx, first_frame + u32::try_from(attempt).unwrap());
                }
            }
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle held direct retirement through Backend");
            if fixture.backend.pstate.scanout_m2.current.is_some()
                && fixture.backend.pstate.scanout_m2.pending.is_none()
            {
                return present_id;
            }
        } else {
            retire_scene_outputs(fixture, first_frame + 40 + u32::try_from(attempt).unwrap());
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle direct-admission composition through Backend");
        }
    }
    panic!("Backend direct Presents did not establish a held Vsync frame")
}

fn assert_cursor_only_keeps_direct_primary(
    fixture: &AtomicCursorFixture,
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
) {
    assert_eq!(
        record.kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    assert_eq!(record.events.len(), 1, "one accepted CRTC event is owed");
    let primary_fb = fixture.backend.platform.outputs[0].output.plane_fb_id_prop;
    assert!(
        record
            .properties
            .iter()
            .all(|update| update.property != primary_fb),
        "cursor-only commit leaves the held direct primary framebuffer unchanged"
    );
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert!(fixture.backend.pstate.scanout_m2.pending.is_none());
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_none());
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
// Mutations: rely on the next Present, bind at direct retirement, keep the visible-only guard, or omit the hide producer.
fn atomic_cursor_only_held_direct_services_show_hide_animation_and_first_bind() {
    use crate::{
        drm::atomic::AtomicCommitKind,
        kms::render::scene::{CursorAcceptedMode, CursorCommitReceipt, CursorPrimaryContent},
    };
    use std::time::Duration;

    let now = Rc::new(Cell::new(Instant::now()));
    let clock_now = Rc::clone(&now);
    let clock: Rc<dyn Fn() -> Instant> = Rc::new(move || clock_now.get());
    let Some(mut fixture) = AtomicCursorFixture::new_with_hotspot_properties_and_clock(clock)
        .expect("construct clock-controlled direct cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let direct = super::round_a::setup_cursor_only_direct_present(&mut fixture);
    let transparent = transparent_bitmap_cursor(&mut fixture.backend);
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, transparent.as_raw())
        .expect("select transparent cursor before direct admission");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit cursorless retained primary");
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(CursorCommitReceipt {
            frame: fixture
                .backend
                .scene
                .for_tests_cursor_commit_receipt(0)
                .and_then(|receipt| receipt.frame),
            mode: CursorAcceptedMode::Hidden,
            primary_cursor: CursorPrimaryContent::Cursorless,
            needs_recompose: false,
        }),
        "the retained primary is verified cursorless before direct ownership"
    );
    retire_scene_outputs(&mut fixture, 1);
    let direct_present_id = admit_held_direct_cursorless(&mut fixture, &direct, 26_000, 20);
    assert!(fixture.backend.pstate.scanout_m2.active());
    assert!(fixture.backend.pstate.scanout_m2.hold_direct);
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_none());
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));

    let mut expected = vec![AtomicCommitKind::SceneWithCursor, AtomicCommitKind::Scene];
    let before_show = fixture.recorders.atomic.records().len();
    let first = create_argb_cursor(&mut fixture.backend, [0x51, 0x62, 0x73, 0xff], 1, 2);
    Backend::define_cursor(&mut fixture.backend, None, root, first.as_raw())
        .expect("show cursor while direct primary is held");
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        before_show,
        "cursor producer only queues the first-bind intent"
    );
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
        "intent does not publish Hardware before atomic acceptance"
    );
    Backend::maybe_composite(&mut fixture.backend).expect("schedule first direct cursor attach");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_show + 1);
    let attach = records.last().unwrap();
    assert_cursor_only_keeps_direct_primary(&fixture, attach);
    let accepted = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted cursor-only attach receipt");
    assert_eq!(accepted.frame, None);
    assert_eq!(accepted.mode, CursorAcceptedMode::Hardware);
    assert_eq!(accepted.primary_cursor, CursorPrimaryContent::Cursorless);
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw
    );
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    expected.push(AtomicCommitKind::CursorOnly);

    let before_late_motion = fixture.recorders.atomic.records().len();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 104, 95);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("retain latest motion behind accepted first-bind event");
    assert_eq!(fixture.recorders.atomic.records().len(), before_late_motion);
    fixture.retire_output(0, 60);
    Backend::maybe_composite(&mut fixture.backend).expect("schedule post-attach motion");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_late_motion + 1);
    let motion = records.last().unwrap();
    assert_cursor_only_keeps_direct_primary(&fixture, motion);
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let properties = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .unwrap()
        .properties_for(crtc)
        .unwrap();
    assert!(motion.properties.iter().any(|update| {
        update.object == properties.plane.into()
            && update.property == properties.crtc_x
            && update.value == i64::from(103_i32) as u64
    }));
    assert!(motion.properties.iter().any(|update| {
        update.object == properties.plane.into()
            && update.property == properties.crtc_y
            && update.value == i64::from(93_i32) as u64
    }));
    fixture.retire_output(0, 61);
    expected.push(AtomicCommitKind::CursorOnly);

    let before_image = fixture.recorders.atomic.records().len();
    let image = create_argb_cursor(&mut fixture.backend, [0x11, 0x22, 0x33, 0xff], 1, 2);
    Backend::define_cursor(&mut fixture.backend, None, root, image.as_raw())
        .expect("replace held-direct image");
    assert_eq!(fixture.recorders.atomic.records().len(), before_image);
    Backend::maybe_composite(&mut fixture.backend).expect("schedule held-direct image");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_image + 1);
    let image_update = records.last().unwrap();
    assert_cursor_only_keeps_direct_primary(&fixture, image_update);
    assert!(image_update.properties.iter().any(|update| {
        update.object == properties.plane.into() && update.property == properties.fb_id
    }));
    fixture.retire_output(0, 62);
    expected.push(AtomicCommitKind::CursorOnly);

    let before_hotspot = fixture.recorders.atomic.records().len();
    let hotspot = create_argb_cursor(&mut fixture.backend, [0x11, 0x22, 0x33, 0xff], 3, 4);
    Backend::define_cursor(&mut fixture.backend, None, root, hotspot.as_raw())
        .expect("change held-direct hotspot");
    assert_eq!(fixture.recorders.atomic.records().len(), before_hotspot);
    Backend::maybe_composite(&mut fixture.backend).expect("schedule held-direct hotspot");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_hotspot + 1);
    let hotspot_update = records.last().unwrap();
    assert_cursor_only_keeps_direct_primary(&fixture, hotspot_update);
    assert!(hotspot_update.properties.iter().any(|update| {
        update.object == properties.plane.into()
            && update.property == properties.hotspot_x.unwrap()
            && update.value == 3
    }));
    assert!(hotspot_update.properties.iter().any(|update| {
        update.object == properties.plane.into()
            && update.property == properties.hotspot_y.unwrap()
            && update.value == 4
    }));
    fixture.retire_output(0, 63);
    expected.push(AtomicCommitKind::CursorOnly);

    let next = create_argb_cursor(&mut fixture.backend, [0xaa, 0xbb, 0xcc, 0xff], 3, 4);
    let animation =
        Backend::create_anim_cursor(&mut fixture.backend, None, &[(hotspot, 40), (next, 40)])
            .expect("create held-direct animation")
            .expect("animation handle");
    let before_animation = fixture.recorders.atomic.records().len();
    Backend::define_cursor(&mut fixture.backend, None, root, animation.as_raw())
        .expect("select held-direct animation");
    Backend::maybe_composite(&mut fixture.backend).expect("schedule animation frame zero");
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        before_animation + 1
    );
    assert_cursor_only_keeps_direct_primary(
        &fixture,
        fixture.recorders.atomic.records().last().unwrap(),
    );
    fixture.retire_output(0, 64);
    expected.push(AtomicCommitKind::CursorOnly);

    let deadline = Backend::next_wakeup(&fixture.backend).expect("animation has a deadline");
    now.set(deadline + Duration::from_millis(1));
    let before_animation_frame = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend).expect("schedule animation frame one");
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        before_animation_frame + 1
    );
    assert_cursor_only_keeps_direct_primary(
        &fixture,
        fixture.recorders.atomic.records().last().unwrap(),
    );
    fixture.retire_output(0, 65);
    expected.push(AtomicCommitKind::CursorOnly);

    let before_hide = fixture.recorders.atomic.records().len();
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    assert_eq!(fixture.recorders.atomic.records().len(), before_hide);
    Backend::maybe_composite(&mut fixture.backend).expect("schedule held-direct hide");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_hide + 1);
    let hide = records.last().unwrap();
    assert_cursor_only_keeps_direct_primary(&fixture, hide);
    assert!(hide.properties.iter().any(|update| {
        update.object == properties.plane.into()
            && update.property == properties.fb_id
            && update.value == 0
    }));
    assert!(hide.properties.iter().any(|update| {
        update.object == properties.plane.into()
            && update.property == properties.crtc_id
            && update.value == 0
    }));
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(CursorCommitReceipt {
            frame: None,
            mode: CursorAcceptedMode::Hidden,
            primary_cursor: CursorPrimaryContent::Cursorless,
            needs_recompose: false,
        })
    );
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    fixture.retire_output(0, 66);
    expected.push(AtomicCommitKind::CursorOnly);
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert_eq!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .current
            .as_ref()
            .unwrap()
            .candidate
            .present_id,
        direct_present_id,
        "cursor-only changes leave the one accepted direct primary held"
    );
    assert_eq!(
        fixture.recorders.atomic.records()[..before_show]
            .iter()
            .filter(|record| record.properties.iter().any(|update| {
                update.property == fixture.backend.platform.outputs[0].output.plane_fb_id_prop
                    && update.value == 0x8100
            }))
            .count(),
        1,
        "the accepted direct source framebuffer remains the only held primary"
    );
    assert_eq!(
        fixture.recorders.atomic.records()[..before_show]
            .iter()
            .filter(|record| record.kind == AtomicCommitKind::CursorOnly)
            .count(),
        0
    );

    // A sprite beyond the accepted cursor capacity requests the normal composed recovery.
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    let oversized =
        create_argb_cursor_extent(&mut fixture.backend, [0x44, 0x55, 0x66, 0xff], 65, 1, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, oversized.as_raw())
        .expect("select oversized cursor through Backend");
    assert!(fixture.backend.pstate.scanout_m2.unflip_requested);
    Backend::maybe_composite(&mut fixture.backend).expect("submit normal composed direct recovery");
    assert!(
        !fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    let records = fixture.recorders.atomic.records();
    let unflip = records.last().unwrap();
    assert_eq!(unflip.kind, AtomicCommitKind::SceneWithCursor);
    assert!(unflip.properties.iter().any(|update| {
        update.property == fixture.backend.platform.outputs[0].output.plane_fb_id_prop
            && update.value != 0x8100
    }));
    fixture.retire_output(0, 67);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("compose software fallback after direct primary retires");
    assert_eq!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .map(|receipt| receipt.primary_cursor),
        Some(CursorPrimaryContent::Software {
            pos: (104, 95),
            record_version: fixture.backend.cursor_records[&oversized.as_raw()].version
        }),
        "software pixels are first accepted in a composed primary after direct unflip"
    );
    if fixture.backend.scene.has_pending_page_flip(0) {
        fixture.retire_output(0, 68);
    }
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );
    assert!(!fixture.backend.pstate.scanout_m2.active());
    expected.extend([
        AtomicCommitKind::SceneWithCursor,
        AtomicCommitKind::SceneWithCursor,
    ]);
    let sequence: Vec<_> = fixture
        .recorders
        .atomic
        .records()
        .iter()
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        sequence, expected,
        "the no-successor direct cursor trace is exact"
    );

    Backend::set_dpms_power(&mut fixture.backend, 3).expect("disable output through Backend");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: omit the external acceptance receipt, publish at intent, retain Hidden after accepted rebind, or guard first bind on existing visibility.
fn atomic_cursor_only_vsync_direct_rebind_receipt_routes_motion_to_hardware() {
    use crate::{
        drm::atomic::AtomicCommitKind,
        kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent},
    };
    use std::io;

    let Some(mut fixture) = AtomicCursorFixture::new_with_two_outputs()
        .expect("construct two-output held-direct cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let direct = super::round_a::setup_cursor_only_direct_present(&mut fixture);
    let transparent = transparent_bitmap_cursor(&mut fixture.backend);
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, transparent.as_raw())
        .expect("select transparent cursor before direct admission");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 810, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("submit initial cursorless primary on both outputs");
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .is_some()
    );
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(1)
            .is_some()
    );
    retire_scene_outputs(&mut fixture, 1);

    // Build real direct-entry probation, then put accepted scene acknowledgements
    // back in flight. The final candidate must wait for those Backend events.
    for attempt in 0..crate::kms::render::backend::SCANOUT_M2_ELIGIBLE_ROOT_PROBATION - 1 {
        let (candidate, event) =
            super::round_a::direct_present_for_id(&direct, 27_000 + u64::from(attempt));
        Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
        assert!(
            !Backend::try_present_direct(&mut fixture.backend, candidate, event)
                .expect("direct probation stays on the composed path"),
            "probation attempt {attempt} cannot submit before the complete stable run"
        );
        assert!(!fixture.backend.pstate.scanout_m2.active());
    }
    let damage_source = Backend::create_pixmap(&mut fixture.backend, None, 24, 1, 1)
        .expect("create real scene-damage source through Backend");
    Backend::put_image(
        &mut fixture.backend,
        None,
        damage_source.as_raw(),
        24,
        1,
        1,
        0,
        0,
        &[0x31, 0x22, 0x11, 0],
    )
    .expect("write scene-damage source through Backend");
    Backend::copy_area(
        &mut fixture.backend,
        None,
        damage_source.as_raw(),
        direct.window_xid,
        0,
        0,
        0,
        0,
        1,
        1,
    )
    .expect("change composed content through Backend");
    Backend::maybe_composite(&mut fixture.backend)
        .expect("create a real pending scene acknowledgement");
    assert!(fixture.backend.scene.has_pending_page_flips());
    let pending_receipts = [
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        fixture.backend.scene.for_tests_cursor_commit_receipt(1),
    ];
    let records_before_wait = fixture.recorders.atomic.records().len();
    let (candidate, event) = super::round_a::direct_present_for_id(&direct, 27_100);
    Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
    assert!(
        !Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("scene acknowledgements prevent direct admission"),
        "an accepted scene frame still owns each output until its event"
    );
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        records_before_wait,
        "no direct transaction may overwrite an outstanding scene receipt"
    );
    assert_eq!(
        [
            fixture.backend.scene.for_tests_cursor_commit_receipt(0),
            fixture.backend.scene.for_tests_cursor_commit_receipt(1),
        ],
        pending_receipts
    );
    fixture.retire_output(1, 10);
    let (candidate, event) = super::round_a::direct_present_for_id(&direct, 27_101);
    Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
    assert!(
        !Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("the remaining output acknowledgement still owns its primary")
    );
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        pending_receipts[0]
    );
    fixture.retire_output(0, 11);
    Backend::maybe_composite(&mut fixture.backend).expect("finish pending scene work");

    let direct_present_id = admit_held_direct_cursorless(&mut fixture, &direct, 27_200, 20);
    assert!(fixture.backend.pstate.scanout_m2.active());
    assert!(fixture.backend.pstate.scanout_m2.hold_direct);
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_none());
    assert!(!fixture.backend.platform.any_cursor_plane_visible());
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );

    let visible = create_argb_cursor(&mut fixture.backend, [0x33, 0x66, 0x99, 0xff], 1, 1);
    let before_show = fixture.recorders.atomic.records().len();
    Backend::define_cursor(&mut fixture.backend, None, root, visible.as_raw())
        .expect("select opaque cursor while Vsync direct is held");
    assert_eq!(fixture.recorders.atomic.records().len(), before_show);
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
        "selecting an image does not publish a Hardware acceptance"
    );
    Backend::maybe_composite(&mut fixture.backend).expect("submit first atomic direct attach");
    let attach_records = fixture.recorders.atomic.records();
    assert_eq!(attach_records.len(), before_show + 2);
    assert!(
        attach_records[before_show..]
            .iter()
            .all(|record| record.kind == AtomicCommitKind::CursorOnly)
    );
    for output_idx in 0..2 {
        let receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(output_idx)
            .expect("accepted first-bind receipt");
        assert_eq!(receipt.frame, None);
        assert_eq!(receipt.mode, CursorAcceptedMode::Hardware);
        assert_eq!(receipt.primary_cursor, CursorPrimaryContent::Cursorless);
        assert!(
            fixture
                .backend
                .platform
                .cursor_plane_visible_for_output(output_idx)
        );
        assert!(
            fixture
                .backend
                .platform
                .cursor_atomic_event_pending_for_output(output_idx)
        );
    }
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw
    );

    let before_motion = fixture.recorders.atomic.records().len();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 830, 110);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("retain current position behind first-bind events");
    assert_eq!(fixture.recorders.atomic.records().len(), before_motion);
    for (output_idx, expected_x) in [(1, 29_i32), (0, 829_i32)] {
        let before_output_event = fixture.recorders.atomic.records().len();
        fixture.retire_output(output_idx, 30 + u32::try_from(output_idx).unwrap());
        Backend::maybe_composite(&mut fixture.backend)
            .expect("route current motion through the accepted Hardware receipt");
        let records = fixture.recorders.atomic.records();
        assert_eq!(records.len(), before_output_event + 1);
        let movement = records.last().unwrap();
        assert_eq!(movement.kind, AtomicCommitKind::CursorOnly);
        let layout = &fixture.backend.platform.outputs[output_idx];
        let properties = fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .properties_for(layout.output.crtc)
            .unwrap();
        assert!(movement.properties.iter().any(|update| {
            update.object == properties.plane.into()
                && update.property == properties.crtc_x
                && update.value == i64::from(expected_x) as u64
        }));
        assert!(movement.properties.iter().any(|update| {
            update.object == properties.plane.into()
                && update.property == properties.crtc_y
                && update.value == i64::from(109_i32) as u64
        }));
        assert!(
            movement
                .properties
                .iter()
                .all(|update| { update.object == properties.plane.into() })
        );
    }
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(1)
    );
    fixture.retire_output(0, 40);
    Backend::maybe_composite(&mut fixture.backend).expect("settle output zero motion");
    fixture.retire_output(1, 41);
    Backend::maybe_composite(&mut fixture.backend).expect("settle output one motion");
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_outputs(&[0, 1])
    );

    let hide_cursor = transparent_bitmap_cursor(&mut fixture.backend);
    let before_hide = fixture.recorders.atomic.records().len();
    Backend::define_cursor(&mut fixture.backend, None, root, hide_cursor.as_raw())
        .expect("select transparent cursor while both hardware planes are bound");
    assert_eq!(fixture.recorders.atomic.records().len(), before_hide);
    Backend::maybe_composite(&mut fixture.backend).expect("submit accepted cursor hides");
    let hide_records = fixture.recorders.atomic.records();
    assert_eq!(hide_records.len(), before_hide + 2);
    assert!(
        hide_records[before_hide..]
            .iter()
            .all(|record| record.kind == AtomicCommitKind::CursorOnly)
    );
    for output_idx in 0..2 {
        let receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(output_idx)
            .expect("accepted hide receipt");
        assert_eq!(receipt.mode, CursorAcceptedMode::Hidden);
        assert_eq!(receipt.primary_cursor, CursorPrimaryContent::Cursorless);
        assert!(
            !fixture
                .backend
                .platform
                .cursor_plane_visible_for_output(output_idx)
        );
    }
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    fixture.retire_output(1, 50);
    fixture.retire_output(0, 51);

    let before_rebind = fixture.recorders.atomic.records().len();
    Backend::define_cursor(&mut fixture.backend, None, root, visible.as_raw())
        .expect("reselect opaque cursor after both accepted hides");
    assert_eq!(fixture.recorders.atomic.records().len(), before_rebind);
    Backend::maybe_composite(&mut fixture.backend).expect("rebind after accepted hides");
    let rebind_records = fixture.recorders.atomic.records();
    assert_eq!(rebind_records.len(), before_rebind + 2);
    assert!(
        rebind_records[before_rebind..]
            .iter()
            .all(|record| record.kind == AtomicCommitKind::CursorOnly)
    );
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw,
        "accepted external attach receipts restore Hardware routing"
    );
    for output_idx in 0..2 {
        let receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(output_idx)
            .expect("accepted rebind receipt");
        assert_eq!(receipt.mode, CursorAcceptedMode::Hardware);
        assert_eq!(receipt.primary_cursor, CursorPrimaryContent::Cursorless);
    }
    fixture.retire_output(0, 52);
    fixture.retire_output(1, 53);

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("detach before rejected attach");
    assert!(!fixture.backend.platform.any_cursor_plane_visible());
    fixture.retire_output(1, 54);
    fixture.retire_output(0, 55);
    fixture
        .recorders
        .atomic
        .script_result(Err(io::Error::from_raw_os_error(libc::EBUSY)));
    let before_rejected_attach = fixture.recorders.atomic.records().len();
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    Backend::maybe_composite(&mut fixture.backend).expect("attempt rejected direct cursor attach");
    let rejected_records = fixture.recorders.atomic.records();
    assert_eq!(rejected_records.len(), before_rejected_attach + 2);
    assert_eq!(
        rejected_records[before_rejected_attach].kind,
        AtomicCommitKind::CursorOnly
    );
    assert_eq!(
        rejected_records[before_rejected_attach].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            kind: io::ErrorKind::ResourceBusy,
            raw_os_error: Some(libc::EBUSY),
        }
    );
    assert_eq!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .unwrap()
            .mode,
        CursorAcceptedMode::Hidden,
        "EBUSY does not publish Hardware for the rejected output"
    );
    assert_eq!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(1)
            .unwrap()
            .mode,
        CursorAcceptedMode::Hardware,
        "the independent accepted output publishes its own Hardware receipt"
    );
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(1));
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw,
        "the scene treats Hidden as neutral while per-output receipts retain the rejected detach"
    );
    assert!(fixture.backend.pstate.scanout_m2.active());
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_none());
    fixture.retire_output(1, 56);

    let sequence: Vec<_> = fixture
        .recorders
        .atomic
        .records()
        .iter()
        .map(|record| record.kind)
        .collect();
    let mut expected = vec![AtomicCommitKind::SceneWithCursor; 2];
    expected.extend([AtomicCommitKind::Scene; 2]);
    expected.extend([AtomicCommitKind::CursorOnly; 12]);
    assert_eq!(
        sequence, expected,
        "direct admission, accepted transitions, motion, and rejected attach are exact"
    );
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("disable output through Backend");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
    let _ = direct_present_id;
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
