//! Vblank-locked motion with continuous client damage: the output keeps
//! committing scene frames, and cursor changes ride them, as master does.
//!
//! The model: one commit in flight per CRTC, whose flip event is delivered
//! only at the next simulated vblank. Each interval brings pointer motion and
//! client damage on the root, then the vblank retires the in-flight commit and
//! the loop ticks.

use super::{
    round_b_producers::begin_hw_cursor,
    round_c_attribution::{output_cursor_handles, teardown},
    support::AtomicCursorFixture,
};
use crate::drm::atomic::AtomicCommitKind;
use ::drm::control::{AtomicCommitFlags, property};
use yserver_core::backend::Backend;

const VBLANKS: u32 = 30;
const MIN_SCENE_COMMITS: usize = 25;
const MIN_SCENE_WITH_CURSOR_COMMITS: usize = 25;
const POINTER_START: (i32, i32) = (120, 100);

fn damage_colour(vblank: u32) -> u32 {
    0x0020_0000 | (vblank + 1)
}

fn crtc_position_of(
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    crtc_x: property::Handle,
    crtc_y: property::Handle,
) -> Option<(u64, u64)> {
    let x = record
        .properties
        .iter()
        .find(|update| update.property == crtc_x)?
        .value;
    let y = record
        .properties
        .iter()
        .find(|update| update.property == crtc_y)?
        .value;
    Some((x, y))
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: remove the flip-event frame-work guard so its cursor drain takes a damaged
// frame's vblank. The in-loop sequence, SceneWithCursor count, and coordinates must fail.
fn atomic_cursor_only_vblank_motion_with_damage_keeps_scene_commits() {
    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let handles = output_cursor_handles(&fixture.backend);
    let handle = |name: &str| {
        handles
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap_or_else(|| panic!("output has no {name} property"))
            .1
    };
    let crtc_x = handle("crtc_x");
    let crtc_y = handle("crtc_y");
    let root = fixture.backend.core.window_id;
    let width = fixture.backend.platform.fb_w;
    let height = fixture.backend.platform.fb_h;
    let mut frame = 10;
    Backend::fill_rectangle(
        &mut fixture.backend,
        None,
        root,
        damage_colour(0),
        0,
        0,
        width,
        height,
    )
    .expect("prime scene damage through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("prime an in-flight scene commit");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    let first_record = fixture.recorders.atomic.records().len();

    let mut scene_with_cursor_commits = 0;
    let mut last_scene_cursor_vblank = None;
    let mut last_scene_cursor_position = None;
    for vblank in 0..VBLANKS {
        // Queue motion and damage while the previous scene commit is in flight.
        // Then deliver its vblank and let the scene tick use the freed CRTC.
        Backend::warp_pointer_root(
            &mut fixture.backend,
            &mut fixture.state,
            POINTER_START.0 + vblank as i32,
            POINTER_START.1 + vblank as i32,
        );
        Backend::fill_rectangle(
            &mut fixture.backend,
            None,
            root,
            damage_colour(vblank),
            0,
            0,
            width,
            height,
        )
        .expect("damage the root through Backend");
        Backend::maybe_composite(&mut fixture.backend)
            .expect("queue motion and damage behind the in-flight frame");
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        frame += 1;
        fixture.retire_output(0, frame);
        let before_tick = fixture.recorders.atomic.records().len();
        Backend::maybe_composite(&mut fixture.backend).expect("composite tick through Backend");

        let expected_position = (
            u64::try_from(POINTER_START.0 + vblank as i32).expect("positive x"),
            u64::try_from(POINTER_START.1 + vblank as i32).expect("positive y"),
        );
        let after_tick = fixture.recorders.atomic.records();
        let tick_cursor_scenes: Vec<_> = after_tick[before_tick..]
            .iter()
            .filter(|record| {
                !record.flags.contains(AtomicCommitFlags::TEST_ONLY)
                    && record.kind == AtomicCommitKind::SceneWithCursor
            })
            .collect();
        for record in tick_cursor_scenes {
            assert_eq!(
                record.result,
                crate::drm::atomic::tests_support::ScriptedResult::Ok,
                "SceneWithCursor must be accepted at vblank {vblank}",
            );
            let position = crtc_position_of(record, crtc_x, crtc_y)
                .expect("SceneWithCursor carries its current hardware cursor position");
            assert_eq!(
                position, expected_position,
                "SceneWithCursor at vblank {vblank} must carry that interval's latest motion",
            );
            scene_with_cursor_commits += 1;
            last_scene_cursor_vblank = Some(vblank);
            last_scene_cursor_position = Some(position);
        }
        assert!(
            last_scene_cursor_vblank.is_some_and(|last| vblank.saturating_sub(last) <= 2)
                || vblank < 2,
            "no accepted SceneWithCursor for more than two vblanks; last={last_scene_cursor_vblank:?}",
        );
    }

    let loop_records: Vec<_> = fixture.recorders.atomic.records()[first_record..]
        .iter()
        .filter(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY))
        .cloned()
        .collect();
    let scene_commits = loop_records
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
            )
        })
        .count();
    assert!(
        scene_with_cursor_commits >= MIN_SCENE_WITH_CURSOR_COMMITS,
        "SceneWithCursor commits {scene_with_cursor_commits} of {VBLANKS} vblanks, need {MIN_SCENE_WITH_CURSOR_COMMITS}",
    );
    assert_eq!(
        last_scene_cursor_position,
        Some((
            u64::try_from(POINTER_START.0 + VBLANKS as i32 - 1).expect("positive x"),
            u64::try_from(POINTER_START.1 + VBLANKS as i32 - 1).expect("positive y"),
        )),
        "the latest motion is carried during the measured loop",
    );
    let kinds: Vec<_> = loop_records.iter().map(|record| record.kind).collect();
    assert!(
        scene_commits >= MIN_SCENE_COMMITS,
        "scene commits {scene_commits} of {VBLANKS} vblanks, need {MIN_SCENE_COMMITS}; kinds: {kinds:?}",
    );

    // Settle: retire every obligation and tick until the output is idle.
    for _ in 0..16 {
        if !fixture.backend.scene.has_pending_page_flip(0)
            && !fixture
                .backend
                .platform
                .cursor_commit_in_flight_for_output(0)
            && !fixture.backend.for_tests_scene_wants_scene()
        {
            break;
        }
        if fixture.backend.scene.has_pending_page_flip(0)
            || fixture
                .backend
                .platform
                .cursor_commit_in_flight_for_output(0)
        {
            frame += 1;
            fixture.retire_output(0, frame);
        }
        Backend::maybe_composite(&mut fixture.backend).expect("settle tick through Backend");
    }
    assert!(
        !fixture.backend.scene.has_pending_page_flip(0)
            && !fixture
                .backend
                .platform
                .cursor_commit_in_flight_for_output(0),
        "output still owes a flip event after settling",
    );

    // The final primary content shows the latest damage.
    let last_colour = damage_colour(VBLANKS - 1);
    let pixel = crate::kms::render::backend::read_scanout_region(
        &mut fixture.backend,
        ash::vk::Rect2D {
            offset: ash::vk::Offset2D { x: 0, y: 0 },
            extent: ash::vk::Extent2D {
                width: 1,
                height: 1,
            },
        },
        crate::kms::render::backend::ScanoutReadSelection::OnScreenOnly,
    )
    .expect("read the on-screen primary pixel");
    assert_eq!(
        &pixel[..3],
        &[
            (last_colour & 0xff) as u8,
            ((last_colour >> 8) & 0xff) as u8,
            ((last_colour >> 16) & 0xff) as u8,
        ],
        "final primary content must show the last damage",
    );

    // The last cursor position committed is the latest motion.
    let latest = (
        u64::try_from(POINTER_START.0 + VBLANKS as i32 - 1).expect("positive x"),
        u64::try_from(POINTER_START.1 + VBLANKS as i32 - 1).expect("positive y"),
    );
    let last_position = fixture
        .recorders
        .atomic
        .records()
        .iter()
        .rev()
        .find_map(|record| crtc_position_of(record, crtc_x, crtc_y))
        .expect("a cursor commit carried a position");
    assert_eq!(
        last_position, latest,
        "the last committed cursor position is the latest motion",
    );

    // End state: the cursor is still visible here, so teardown detaches it and then
    // asserts no intent, obligation, or leaked sprite framebuffer remains.
    teardown(fixture);
}
