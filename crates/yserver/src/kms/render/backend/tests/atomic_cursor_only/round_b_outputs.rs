use super::support::AtomicCursorFixture;

fn create_cursor(
    fixture: &mut AtomicCursorFixture,
    width: u16,
    height: u16,
    hotspot: (u16, u16),
) -> yserver_core::backend::CursorHandle {
    use yserver_core::backend::{AnyHandle, Backend};

    let pixmap = Backend::create_pixmap(&mut fixture.backend, None, 32, width, height)
        .expect("create ARGB cursor pixmap");
    let pixels = [0x31, 0x72, 0xb3, 0xff].repeat(usize::from(width) * usize::from(height));
    Backend::put_image(
        &mut fixture.backend,
        None,
        pixmap.as_raw(),
        32,
        width,
        height,
        0,
        0,
        &pixels,
    )
    .expect("write ARGB cursor pixels");
    let picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(pixmap),
        0,
        0,
        &[],
    )
    .expect("create ARGB cursor picture")
    .expect("picture handle");
    Backend::render_create_cursor(&mut fixture.backend, None, picture, hotspot.0, hotspot.1)
        .expect("create cursor")
        .expect("cursor handle")
}

fn record_for_crtc(
    records: &[crate::drm::atomic::tests_support::AtomicCommitRecord],
    crtc: ::drm::control::crtc::Handle,
) -> Option<&crate::drm::atomic::tests_support::AtomicCommitRecord> {
    records
        .iter()
        .find(|record| record.events.iter().any(|event| event.crtc == crtc))
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: route by raw CRTC alone, classify Mixed as all-HW, attach while a peer event is outstanding, or replay another output's transition.
fn atomic_cursor_only_cross_output_membership_preserves_mixed_mode() {
    use crate::drm::atomic::AtomicCommitKind;
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new_with_cursor_mixed_three_output_layout()
        .expect("build offset mixed-cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let cursor = create_cursor(&mut fixture, 64, 64, (0, 0));
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("define shared cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 780, 150);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit shared HW/HW frames");

    let outputs = &fixture.backend.platform.outputs;
    let crtcs = outputs
        .iter()
        .map(|output| output.output.crtc)
        .collect::<Vec<_>>();
    let initial = fixture.recorders.atomic.records();
    assert_eq!(
        initial.len(),
        3,
        "one scene request owns each primary output"
    );
    let first_hw = record_for_crtc(&initial, crtcs[0]).expect("first HW output commit");
    let second_hw = record_for_crtc(&initial, crtcs[1]).expect("second HW output commit");
    let software_peer = record_for_crtc(&initial, crtcs[2]).expect("software peer primary");
    assert_eq!(first_hw.kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(second_hw.kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(software_peer.kind, AtomicCommitKind::Scene);
    for (output_idx, expected_x) in [(0, 780), (1, -20)] {
        let layout = &fixture.backend.platform.outputs[output_idx];
        let device = fixture
            .backend
            .platform
            .device_for_key(layout.key.device_key)
            .unwrap();
        let properties = device
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .properties_for(layout.output.crtc)
            .unwrap();
        let commit = record_for_crtc(&initial, layout.output.crtc).unwrap();
        assert!(
            commit.properties.iter().any(|update| {
                update.object == properties.plane.into()
                    && update.property == properties.crtc_x
                    && update.value == i64::from(expected_x) as u64
            }),
            "cursor position is output-local for CRTC {:?}",
            layout.output.crtc
        );
    }

    // The peer event retires first. Its shared sprite remains owned by the
    // other output until that output's own event drains.
    fixture.retire_output(1, 1);
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert!(!fixture.backend.scene.has_pending_page_flip(1));
    fixture.retire_output(2, 1);
    fixture.retire_output(0, 1);
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let (lease0, lease1, properties0, properties1) = {
        let device = fixture.backend.platform.device_for_key(device_key).unwrap();
        let plane = device.cursor.plane.as_ref().unwrap();
        (
            device.cursor.crtc_states[&crtcs[0]].applied.unwrap().lease,
            device.cursor.crtc_states[&crtcs[1]].applied.unwrap().lease,
            plane.properties_for(crtcs[0]).unwrap(),
            plane.properties_for(crtcs[1]).unwrap(),
        )
    };
    assert_eq!(
        lease0, lease1,
        "overlapping HW outputs share the sprite buffer"
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .device_for_key(device_key)
            .unwrap()
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(lease0),
        Some(2),
        "both live CRTCs own the shared sprite after staggered frame events",
    );

    let before_hw_crossing = fixture.recorders.atomic.records().len();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 900, 150);
    Backend::maybe_composite(&mut fixture.backend).expect("cross to the second HW output");
    let after_hw_crossing = fixture.recorders.atomic.records();
    let crossing = &after_hw_crossing[before_hw_crossing..];
    assert_eq!(
        crossing.len(),
        2,
        "detach old HW member and move new member"
    );
    let detach = record_for_crtc(crossing, crtcs[0]).expect("old HW output detach");
    let move_second = record_for_crtc(crossing, crtcs[1]).expect("new HW output position");
    assert_eq!(detach.kind, AtomicCommitKind::CursorOnly);
    assert_eq!(move_second.kind, AtomicCommitKind::CursorOnly);
    assert!(detach.properties.iter().any(|update| {
        update.object == properties0.plane.into()
            && update.property == properties0.fb_id
            && update.value == 0
    }));
    assert!(
        crossing
            .iter()
            .all(|record| record.kind == AtomicCommitKind::CursorOnly),
        "HW membership changes do not allocate a new primary framebuffer",
    );
    fixture.retire_output(0, 2);
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(1)
    );
    let before_peer_reentry = fixture.recorders.atomic.records().len();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 780, 150);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("re-enter output zero during output one's event");
    let peer_reentry_records = fixture.recorders.atomic.records();
    assert_eq!(peer_reentry_records.len(), before_peer_reentry + 1);
    let reattach = &peer_reentry_records[before_peer_reentry];
    assert_eq!(reattach.kind, AtomicCommitKind::CursorOnly);
    assert!(reattach.events.iter().any(|event| event.crtc == crtcs[0]));
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(1)
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .device_for_key(device_key)
            .unwrap()
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(lease0),
        Some(2),
        "the peer's in-flight event keeps its sprite user while output zero reattaches",
    );
    fixture.retire_output(0, 3);
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(1)
    );
    fixture.retire_output(1, 2);
    Backend::maybe_composite(&mut fixture.backend).expect("apply output one's coalesced position");
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(1)
    );
    fixture.retire_output(1, 3);
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw
    );

    let before_mixed_crossing = fixture.recorders.atomic.records().len();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 1590, 150);
    Backend::maybe_composite(&mut fixture.backend).expect("cross into the SW peer footprint");
    let after_mixed_crossing = fixture.recorders.atomic.records();
    let mixed = &after_mixed_crossing[before_mixed_crossing..];
    let hw_peer = record_for_crtc(mixed, crtcs[1]).expect("HW peer motion");
    let sw_peer = record_for_crtc(mixed, crtcs[2]).expect("SW peer repaint");
    assert_eq!(hw_peer.kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(sw_peer.kind, AtomicCommitKind::Scene);
    assert!(hw_peer.properties.iter().any(|update| {
        update.object == properties1.plane.into()
            && update.property == properties1.crtc_x
            && update.value == 790
    }));
    assert!(
        sw_peer
            .properties
            .iter()
            .all(|update| update.object != properties1.plane.into())
    );
    let hw_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(1)
        .unwrap();
    let sw_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(2)
        .unwrap();
    assert_eq!(
        hw_receipt.mode,
        crate::kms::render::scene::CursorAcceptedMode::Hardware
    );
    assert_eq!(
        hw_receipt.primary_cursor,
        crate::kms::render::scene::CursorPrimaryContent::Cursorless
    );
    assert_eq!(
        sw_receipt.mode,
        crate::kms::render::scene::CursorAcceptedMode::Software
    );
    assert!(matches!(
        sw_receipt.primary_cursor,
        crate::kms::render::scene::CursorPrimaryContent::Software { .. }
    ));
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Mixed
    );
    for output_idx in [1, 2, 0] {
        if fixture.backend.scene.has_pending_page_flip(output_idx) {
            fixture.retire_output(output_idx, 3);
        }
    }
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(2)
    );
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Mixed
    );

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("hide SW/HW cursor through Backend");
    for output_idx in [2, 0, 1] {
        if fixture.backend.scene.has_pending_page_flip(output_idx) {
            fixture.retire_output(output_idx, 4);
        }
    }
    fixture
        .backend
        .disable_output()
        .expect("disable and drain each output through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: publish the TEST_ONLY cursor delta, omit it from the probe, or submit the direct successor before the peer event.
fn atomic_cursor_only_direct_probe_and_grouped_unflip_keep_cursor_owners() {
    super::round_e::run_atomic_cursor_only_deferred_direct_replacement_reaches_later_crtcs();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: detach HW at grouped unflip/intent, publish SW from cursorless retained content, omit SW pixels from the detach frame, or allow Async while HW remains bound.
fn atomic_cursor_only_tearing_handoff_keeps_hw_until_software_detach_frame() {
    super::super::tearing::run_entry_from_held_direct_with_visible_hw_cursor_handoff();
}
