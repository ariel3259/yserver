//! Cursor-only progress during primary retry backoff and deferred sprite upload.

use super::{
    round_a::damage_cursor_root,
    round_b_producers::{begin_hw_cursor, create_argb_cursor_extent},
    round_c_attribution::{output_cursor_handles, teardown},
    support::AtomicCursorFixture,
};
use crate::drm::atomic::{
    AtomicCommitKind,
    tests_support::{AtomicCommitRecord, ScriptedResult},
};
use ::drm::control::{AtomicCommitFlags, property};
use yserver_core::backend::Backend;

fn live_records(
    atomic: &crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor,
) -> Vec<AtomicCommitRecord> {
    atomic
        .records()
        .into_iter()
        .filter(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY))
        .collect()
}

fn property_value(record: &AtomicCommitRecord, property: property::Handle) -> Option<u64> {
    record
        .properties
        .iter()
        .find_map(|update| (update.property == property).then_some(update.value))
}

fn cursor_property(handles: &[(&'static str, property::Handle)], name: &str) -> property::Handle {
    handles
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .unwrap_or_else(|| panic!("output has no {name} property"))
        .1
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: exempt only position-only intents during primary retry backoff; the new
// sprite/hotspot intent must still reach an accepted CursorOnly commit with each latest position.
fn atomic_cursor_only_moves_during_primary_submit_retry_backoff() {
    let Some(mut fixture) = AtomicCursorFixture::new_with_hotspot_properties()
        .expect("build hotspot-capable cursor fixture")
    else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let handles = output_cursor_handles(&fixture.backend);
    let fb_id = cursor_property(&handles, "fb_id");
    let crtc_x = cursor_property(&handles, "crtc_x");
    let crtc_y = cursor_property(&handles, "crtc_y");
    let hotspot_x = cursor_property(&handles, "hotspot_x");
    let hotspot_y = cursor_property(&handles, "hotspot_y");
    let root = fixture.backend.core.window_id;
    let (width, height) = fixture.backend.platform.fb_dimensions();
    let newest =
        create_argb_cursor_extent(&mut fixture.backend, [0xe1, 0x82, 0x43, 0xff], 4, 4, 1, 1);
    Backend::define_cursor(&mut fixture.backend, None, root, newest.as_raw())
        .expect("select newest sprite and hotspot through Backend");
    let newest_version = fixture.backend.cursor_records[&newest.as_raw()].version;
    let mut frame = 2;
    const PASSES: u32 = 4;
    let before_loop = live_records(&fixture.recorders.atomic).len();

    for pass in 0..PASSES {
        if let Some(deadline) = fixture.backend.scene.earliest_retry_deadline() {
            while std::time::Instant::now() < deadline {
                std::thread::yield_now();
            }
        }
        let x = 120 + i32::try_from(pass).expect("pass fits") * 3;
        let y = 105 + i32::try_from(pass).expect("pass fits") * 2;
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, x, y);
        Backend::fill_rectangle(
            &mut fixture.backend,
            None,
            root,
            0x0040_1000 | (pass + 1),
            0,
            0,
            width,
            height,
        )
        .expect("damage the root through Backend");
        fixture
            .recorders
            .atomic
            .script_result(Err(std::io::Error::from_raw_os_error(libc::ENOMEM)));
        let before = live_records(&fixture.recorders.atomic).len();
        Backend::maybe_composite(&mut fixture.backend)
            .expect("record primary retry and cursor-only progress");

        let pass_records = live_records(&fixture.recorders.atomic)[before..].to_vec();
        assert_eq!(
            pass_records.len(),
            2,
            "pass {pass}: one rejected primary and one accepted cursor commit: {pass_records:?}",
        );
        assert!(matches!(
            pass_records[0].kind,
            AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
        ));
        assert_eq!(
            pass_records[0].result,
            ScriptedResult::Error {
                kind: std::io::Error::from_raw_os_error(libc::ENOMEM).kind(),
                raw_os_error: Some(libc::ENOMEM),
            },
            "pass {pass}: the primary submit hits scripted ENOMEM",
        );
        assert_eq!(pass_records[1].kind, AtomicCommitKind::CursorOnly);
        assert_eq!(pass_records[1].result, ScriptedResult::Ok);
        assert_eq!(pass_records[1].events.len(), 1);
        assert_eq!(
            property_value(&pass_records[1], crtc_x),
            Some((x - 1) as u64)
        );
        assert_eq!(
            property_value(&pass_records[1], crtc_y),
            Some((y - 1) as u64)
        );
        let binding = fixture.backend.platform.devices[0].cursor.crtc_states
            [&fixture.backend.platform.outputs[0].output.crtc]
            .applied
            .expect("accepted cursor-only binding");
        assert_eq!(binding.lease.version(), newest_version);
        assert_eq!((binding.hot_x, binding.hot_y), (1, 1));
        assert_eq!((binding.x, binding.y), (x - 1, y - 1));
        if pass == 0 {
            assert_eq!(
                property_value(&pass_records[1], fb_id),
                Some(u64::from(binding.framebuffer)),
                "the first cursor-only commit carries the replacement sprite",
            );
            assert_eq!(property_value(&pass_records[1], hotspot_x), Some(1));
            assert_eq!(property_value(&pass_records[1], hotspot_y), Some(1));
        }
        frame += 1;
        fixture.retire_output(0, frame);
    }

    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0),
        "the last cursor-only event is retired",
    );
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    assert!(
        fixture.backend.platform.devices[0].cursor.crtc_states
            [&fixture.backend.platform.outputs[0].output.crtc]
            .intent
            .is_none(),
        "no cursor intent remains parked behind primary retry",
    );
    assert!(
        !fixture
            .backend
            .platform
            .for_tests_atomic_cursor_pending_position(
                fixture.backend.platform.outputs[0].key.device_key,
                fixture.backend.platform.outputs[0].output.crtc,
            )
            .is_some(),
        "no cursor intent remains parked behind primary retry",
    );
    let commits: Vec<_> = live_records(&fixture.recorders.atomic)[before_loop..]
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                AtomicCommitKind::Scene
                    | AtomicCommitKind::SceneWithCursor
                    | AtomicCommitKind::CursorOnly
            )
        })
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        commits,
        [
            AtomicCommitKind::SceneWithCursor,
            AtomicCommitKind::CursorOnly
        ]
        .repeat(PASSES as usize),
        "each damaged retry pass rejects its primary then accepts cursor progress",
    );
    let latest_x = 120 + i32::try_from(PASSES - 1).expect("pass fits") * 3;
    let latest_y = 105 + i32::try_from(PASSES - 1).expect("pass fits") * 2;
    let binding = fixture.backend.platform.devices[0].cursor.crtc_states
        [&fixture.backend.platform.outputs[0].output.crtc]
        .applied
        .expect("latest cursor state remains applied");
    assert_eq!((binding.x, binding.y), (latest_x - 1, latest_y - 1));
    teardown(fixture);
}

fn failed_scene_cursor_transition_returns_to_cursor_only_lane(show: bool) {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent};
    use std::{cell::Cell, rc::Rc, time::Instant};
    use yserver_core::backend::Backend;

    let now = Rc::new(Cell::new(Instant::now()));
    let clock_now = Rc::clone(&now);
    let clock: Rc<dyn Fn() -> Instant> = Rc::new(move || clock_now.get());
    let Some(mut fixture) = AtomicCursorFixture::new_with_clock(clock)
        .expect("build failed scene cursor transition fixture")
    else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let output = &fixture.backend.platform.outputs[0];
    let device_key = output.key.device_key;
    let crtc = output.output.crtc;
    let initial_binding = fixture.backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("initial cursor binding is accepted");

    let mut before_transition = live_records(&fixture.recorders.atomic).len();
    if show {
        Backend::set_cursor_hidden(&mut fixture.backend, true);
        Backend::maybe_composite(&mut fixture.backend).expect("accept the setup hide");
        let setup_records = live_records(&fixture.recorders.atomic);
        let hide = setup_records[before_transition..]
            .last()
            .expect("setup hide commit");
        assert_eq!(hide.kind, AtomicCommitKind::CursorOnly);
        assert_eq!(hide.result, ScriptedResult::Ok);
        assert_eq!(hide.events.len(), 1);
        assert!(
            fixture.backend.platform.devices[0].cursor.crtc_states[&crtc]
                .applied
                .is_none()
        );
        fixture.retire_output(0, 3);

        before_transition = live_records(&fixture.recorders.atomic).len();
        Backend::set_cursor_hidden(&mut fixture.backend, false);
    } else {
        Backend::set_cursor_hidden(&mut fixture.backend, true);
    }

    super::round_a::damage_cursor_root(&mut fixture);
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::ENOMEM)));
    Backend::maybe_composite(&mut fixture.backend)
        .expect("failed scene submit remains recoverable");

    let rejected = live_records(&fixture.recorders.atomic)[before_transition..].to_vec();
    assert_eq!(
        rejected.len(),
        1,
        "the scene failure itself publishes no accepted cursor transition: {rejected:?}",
    );
    assert_eq!(rejected[0].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(
        rejected[0].result,
        ScriptedResult::Error {
            kind: std::io::Error::from_raw_os_error(libc::ENOMEM).kind(),
            raw_os_error: Some(libc::ENOMEM),
        },
        "the scene submit fails with ENOMEM",
    );
    let state = &fixture.backend.platform.devices[0].cursor.crtc_states[&crtc];
    assert_eq!(
        state.applied,
        if show { None } else { Some(initial_binding) },
        "failed scene submission leaves applied ownership untouched",
    );
    assert!(state.in_flight.is_none());
    assert!(
        !state.retry_backoff_cursor_suppressed,
        "a failed scene submit releases cursor-only retry eligibility immediately",
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert!(!fixture.backend.scene.has_pending_page_flip(0));

    let recovery_start = live_records(&fixture.recorders.atomic).len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("requeue and submit cursor intent during scene backoff");
    let recovered = live_records(&fixture.recorders.atomic)[recovery_start..].to_vec();
    assert_eq!(
        recovered.len(),
        1,
        "the effective cursor requeue makes one cursor-only commit: {recovered:?}",
    );
    assert_eq!(recovered[0].kind, AtomicCommitKind::CursorOnly);
    assert_eq!(recovered[0].result, ScriptedResult::Ok);
    assert_eq!(recovered[0].events.len(), 1);
    assert!(!recovered[0].flags.contains(AtomicCommitFlags::TEST_ONLY));

    let state = &fixture.backend.platform.devices[0].cursor.crtc_states[&crtc];
    assert_eq!(state.intent, None);
    assert!(state.in_flight.is_some());
    assert_eq!(state.applied.is_some(), show);
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    if show {
        let receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted cursor-only show receipt");
        assert_eq!(receipt.mode, CursorAcceptedMode::Hardware);
        assert_eq!(receipt.primary_cursor, CursorPrimaryContent::Cursorless);
        assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    } else {
        let handles = output_cursor_handles(&fixture.backend);
        assert_eq!(
            property_value(&recovered[0], cursor_property(&handles, "fb_id")),
            Some(0),
            "the accepted cursor-only hide explicitly detaches FB_ID",
        );
        assert_eq!(
            property_value(&recovered[0], cursor_property(&handles, "crtc_id")),
            Some(0),
            "the accepted cursor-only hide explicitly detaches CRTC_ID",
        );
        assert!(state.applied.is_none());
        assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    }
    assert_eq!(
        live_records(&fixture.recorders.atomic)[before_transition..]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [
            AtomicCommitKind::SceneWithCursor,
            AtomicCommitKind::CursorOnly
        ],
        "the failed scene is followed by exactly one accepted cursor-only transition",
    );

    fixture.retire_output(0, if show { 4 } else { 3 });
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    let state = &fixture.backend.platform.devices[0].cursor.crtc_states[&crtc];
    assert!(state.intent.is_none());
    assert!(state.in_flight.is_none());
    assert!(!state.retry_backoff_cursor_suppressed);
    assert!(
        fixture
            .backend
            .platform
            .device_for_key(device_key)
            .is_some()
    );
    teardown(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: keep suppression latched after a failed scene submit; the later Hide detach must be absent.
fn atomic_cursor_only_failed_scene_hide_returns_to_cursor_only_lane() {
    failed_scene_cursor_transition_returns_to_cursor_only_lane(false);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: keep suppression latched after a failed scene submit; the later Show attach must be absent.
fn atomic_cursor_only_failed_scene_show_returns_to_cursor_only_lane() {
    failed_scene_cursor_transition_returns_to_cursor_only_lane(true);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: clear deferred retry eligibility when a buffer becomes free, or let scene
// ownership suppress a retry upload; either leaves the newest sprite unapplied under damage.
fn atomic_cursor_only_retries_deferred_sprite_while_scene_damage_continues() {
    let Some(mut fixture) = AtomicCursorFixture::new_with_hotspot_properties()
        .expect("build hotspot-capable cursor fixture")
    else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let before_replacements = live_records(&fixture.recorders.atomic).len();
    let root = fixture.backend.core.window_id;
    let initial_binding = fixture.backend.platform.devices[0].cursor.crtc_states
        [&fixture.backend.platform.outputs[0].output.crtc]
        .applied
        .expect("initial hardware cursor binding");

    let first_replacement =
        create_argb_cursor_extent(&mut fixture.backend, [0x19, 0x38, 0x57, 0xff], 4, 4, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, first_replacement.as_raw())
        .expect("select first replacement through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit first sprite replacement");
    let in_flight_binding = fixture.backend.platform.devices[0].cursor.crtc_states
        [&fixture.backend.platform.outputs[0].output.crtc]
        .intent;
    assert!(
        in_flight_binding.is_none(),
        "accepted replacement consumes its intent"
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    let first_binding = fixture.backend.platform.devices[0].cursor.crtc_states
        [&fixture.backend.platform.outputs[0].output.crtc]
        .in_flight
        .as_ref()
        .and_then(|commit| commit.next)
        .expect("replacement binding is protected by its event");
    assert_ne!(first_binding.lease, initial_binding.lease);
    {
        let plane = fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap();
        assert_eq!(plane.sprite_user_count(initial_binding.lease), Some(1));
        assert_eq!(plane.sprite_user_count(first_binding.lease), Some(1));
    }

    let newest =
        create_argb_cursor_extent(&mut fixture.backend, [0xe1, 0x82, 0x43, 0xff], 4, 4, 1, 1);
    Backend::define_cursor(&mut fixture.backend, None, root, newest.as_raw())
        .expect("defer newest sprite while both buffers are in use");
    let newest_version = fixture.backend.cursor_records[&newest.as_raw()].version;
    assert_eq!(
        fixture
            .backend
            .platform
            .cursor_plane_uploaded_version_for_output(0),
        Some(fixture.backend.cursor_records[&first_replacement.as_raw()].version),
        "both sprite buffers remain owned, so newest upload is deferred",
    );

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 145, 125);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("retain newest cursor while the previous cursor event is in flight");
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );

    fixture.retire_output(0, 3);
    let handles = output_cursor_handles(&fixture.backend);
    let fb_id = cursor_property(&handles, "fb_id");
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    ledger.fail_after(
        crate::kms::cursor_plane::ops::CursorBufferOperation::Write,
        0,
        libc::EAGAIN,
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_deferred_upload_for_output(0, newest_version, 1, 1,)
    );
    let (width, height) = fixture.backend.platform.fb_dimensions();
    let mut frames_to_newest = None;
    let mut frame = 3;
    for attempt in 1..=3 {
        let before_attempt = live_records(&fixture.recorders.atomic).len();
        Backend::fill_rectangle(
            &mut fixture.backend,
            None,
            root,
            0x0030_5000 | attempt,
            0,
            0,
            width,
            height,
        )
        .expect("continue client damage through Backend");
        Backend::maybe_composite(&mut fixture.backend)
            .expect("retry deferred image and submit the damaged scene");
        let records = live_records(&fixture.recorders.atomic);
        let attempt_records = &records[before_attempt..];
        assert_eq!(
            attempt_records.len(),
            1,
            "damage submits exactly one scene frame {attempt}: {attempt_records:?}",
        );
        let scene_record = attempt_records
            .iter()
            .rev()
            .find(|record| {
                matches!(
                    record.kind,
                    AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
                )
            })
            .expect("each damaged pass submits its scene");
        assert!(
            matches!(
                scene_record.kind,
                AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
            ),
            "frame {attempt} submits the damaged scene, got {:?}",
            scene_record.kind,
        );
        assert_eq!(
            scene_record.result,
            ScriptedResult::Ok,
            "frame {attempt} scene accepted",
        );
        if attempt == 2 {
            assert_eq!(
                fixture
                    .backend
                    .platform
                    .cursor_plane_uploaded_version_for_output(0),
                Some(newest_version),
                "the second damaged pass stages the retryable newest upload",
            );
        }
        if attempt == 1 {
            assert_eq!(
                ledger.failures_taken(),
                vec![crate::kms::cursor_plane::ops::CursorBufferOperation::Write],
                "the released buffer's first retry returns EAGAIN",
            );
            assert!(
                fixture
                    .backend
                    .platform
                    .cursor_plane_deferred_upload_for_output(0, newest_version, 1, 1,)
            );
        }
        let current_binding = fixture.backend.platform.devices[0].cursor.crtc_states
            [&fixture.backend.platform.outputs[0].output.crtc]
            .applied;
        let newest_is_current =
            current_binding.is_some_and(|binding| binding.lease.version() == newest_version);
        if newest_is_current
            && records.iter().any(|record| {
                record.kind == AtomicCommitKind::SceneWithCursor
                    && record.result == ScriptedResult::Ok
                    && property_value(record, fb_id)
                        == current_binding.map(|binding| u64::from(binding.framebuffer))
            })
        {
            frames_to_newest = Some(attempt);
            break;
        }
        frame += 1;
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        fixture.retire_output(0, frame);
        if attempt == 1 {
            assert!(fixture.backend.scene.output_uses_hardware_cursor(0));
            assert!(
                fixture
                    .backend
                    .platform
                    .cursor_plane_deferred_upload_for_output(0, newest_version, 1, 1,)
            );
        }
    }
    let records = live_records(&fixture.recorders.atomic);
    assert!(
        frames_to_newest.is_some_and(|frames| frames <= 3),
        "newest image reaches a damaged scene within three frames: {records:?}",
    );
    assert_eq!(
        ledger.failures_taken(),
        vec![crate::kms::cursor_plane::ops::CursorBufferOperation::Write],
        "the released buffer's first upload attempt was transient EAGAIN",
    );
    let accepted = records
        .iter()
        .rev()
        .find(|record| {
            record.kind == AtomicCommitKind::SceneWithCursor
                && record.result == ScriptedResult::Ok
                && property_value(record, fb_id).is_some()
        })
        .cloned()
        .expect("the damaged scene accepts the newest sprite");
    let hotspot_x = cursor_property(&handles, "hotspot_x");
    let hotspot_y = cursor_property(&handles, "hotspot_y");
    let crtc_x = cursor_property(&handles, "crtc_x");
    let crtc_y = cursor_property(&handles, "crtc_y");
    let binding = fixture.backend.platform.devices[0].cursor.crtc_states
        [&fixture.backend.platform.outputs[0].output.crtc]
        .applied
        .expect("newest sprite is now applied");
    assert_eq!(binding.lease.version(), newest_version);
    assert_eq!((binding.hot_x, binding.hot_y), (1, 1));
    assert_eq!(
        property_value(&accepted, fb_id),
        Some(u64::from(binding.framebuffer))
    );
    assert_eq!(property_value(&accepted, hotspot_x), Some(1));
    assert_eq!(property_value(&accepted, hotspot_y), Some(1));
    assert_eq!(property_value(&accepted, crtc_x), Some(144));
    assert_eq!(property_value(&accepted, crtc_y), Some(124));
    let sequence: Vec<_> = records[before_replacements..]
        .iter()
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        sequence,
        [
            AtomicCommitKind::CursorOnly,
            AtomicCommitKind::SceneWithCursor,
            AtomicCommitKind::SceneWithCursor,
        ],
        "the first replacement, EAGAIN frame, and newest sprite are accepted in order",
    );
    if fixture.backend.scene.has_pending_page_flip(0) {
        frame += 1;
        fixture.retire_output(0, frame);
    }
    assert!(
        !fixture.backend.scene.has_pending_page_flip(0)
            && !fixture
                .backend
                .platform
                .cursor_commit_in_flight_for_output(0)
            && fixture.backend.platform.devices[0].cursor.crtc_states
                [&fixture.backend.platform.outputs[0].output.crtc]
                .intent
                .is_none(),
        "newest image is retired with no pending cursor intent",
    );
    teardown(fixture);
}
