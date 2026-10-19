//! Cursor-only EINVAL recovery: an explicit detach precedes Software, and a
//! rejected detach keeps the accepted hardware binding without a retry loop.

use std::rc::Rc;

use super::{
    round_a::damage_cursor_root,
    round_b_producers::{begin_hw_cursor, create_argb_cursor, retire_cursor_only},
    round_c_attribution::{cursor_fixture, cursor_only_handles, output_cursor_handles, teardown},
    support::AtomicCursorFixture,
};
use crate::{
    drm::atomic::{
        AtomicCommitKind,
        tests_support::{AtomicCommitRecord, ScriptedResult},
    },
    kms::render::{
        platform::cursor_recovery::RecordingCursorRecoveryReporter,
        scene::{CursorAcceptedMode, CursorCommitReceipt, CursorPrimaryContent},
    },
};
use ::drm::control::{AtomicCommitFlags, property};
use yserver_core::backend::Backend;

fn handle(handles: &[(&'static str, property::Handle)], name: &str) -> property::Handle {
    handles
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .unwrap_or_else(|| panic!("output has no {name} property"))
        .1
}

/// Non-`TEST_ONLY` requests in submission order.
fn live_records(fixture: &AtomicCursorFixture) -> Vec<AtomicCommitRecord> {
    fixture
        .recorders
        .atomic
        .records()
        .into_iter()
        .filter(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY))
        .collect()
}

/// The explicit detach: a cursor-only request carrying exactly `FB_ID=0` and
/// `CRTC_ID=0` on the output's cursor plane.
fn is_detach(record: &AtomicCommitRecord, handles: &[(&'static str, property::Handle)]) -> bool {
    let fb_id = handle(handles, "fb_id");
    let crtc_id = handle(handles, "crtc_id");
    record.kind == AtomicCommitKind::CursorOnly
        && record.properties.len() == 2
        && record
            .properties
            .iter()
            .any(|update| update.property == fb_id && update.value == 0)
        && record
            .properties
            .iter()
            .any(|update| update.property == crtc_id && update.value == 0)
}

fn applied_binding(
    fixture: &AtomicCursorFixture,
) -> Option<crate::kms::render::platform::cursor_state::CursorBinding> {
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    fixture
        .backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.crtc_states.get(&crtc))
        .and_then(|state| state.applied)
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and seen to fail on this test):
// M1 switches the output to Software on the first EINVAL with no detach record;
// M2 accepts Software without submitting the explicit detach;
// M3 releases the old framebuffer at detach acceptance instead of at its event;
// M4 releases the old framebuffer at the refused attach, before any detach;
// M5 requalifies the rejected key when the same key is displayed again.
fn atomic_cursor_only_einval_detaches_before_software() {
    let reporter = Rc::new(RecordingCursorRecoveryReporter::default());
    let Some((mut fixture, handles)) = cursor_fixture(&reporter) else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let binding = applied_binding(&fixture).expect("hardware binding accepted");
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let before = live_records(&fixture).len();

    // Every cursor-only request carrying CRTC_X is rejected; the detach does not.
    fixture
        .recorders
        .atomic
        .reject_properties(&[handle(&handles, "crtc_x")]);
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 40, 30);
    Backend::maybe_composite(&mut fixture.backend).expect("submit rejected motion");

    let live = live_records(&fixture)[before..].to_vec();
    assert_eq!(
        live.len(),
        2,
        "the rejected motion, then the explicit detach: {live:?}"
    );
    assert_eq!(live[0].kind, AtomicCommitKind::CursorOnly);
    assert_eq!(
        live[0].result,
        ScriptedResult::Error {
            kind: std::io::ErrorKind::InvalidInput,
            raw_os_error: Some(libc::EINVAL),
        },
    );
    assert!(
        live[0]
            .properties
            .iter()
            .any(|update| update.property == handle(&handles, "crtc_x")),
        "the rejected request is the motion commit",
    );
    assert!(is_detach(&live[1], &handles), "detach shape: {:?}", live[1]);
    assert_eq!(live[1].result, ScriptedResult::Ok);
    assert_eq!(live[1].events.len(), 1, "accepted detach owes its event");
    assert_eq!(reporter.calls(), 0, "an accepted detach retains nothing");

    // Software begins only with the accepted detach. The old framebuffer
    // stays owned by the in-flight detach until its flip event.
    let plane = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .expect("cursor plane");
    let state = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.crtc_states.get(&crtc))
        .expect("CRTC cursor state");
    assert_eq!(state.applied, None, "the accepted detach unbinds the plane");
    assert!(state.in_flight.is_some(), "the detach owes its event");
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(CursorCommitReceipt {
            frame: None,
            mode: CursorAcceptedMode::Software,
            primary_cursor: CursorPrimaryContent::Cursorless,
            needs_recompose: true,
        }),
        "the accepted detach publishes Software and asks the scene to recompose",
    );
    assert_eq!(
        plane.sprite_user_count(binding.lease),
        Some(1),
        "the old framebuffer stays owned until the detach event",
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );

    // The latest wanted image of the same key arrives during recovery.
    let before_image = live_records(&fixture).len();
    fixture.define_visible_cursor();
    Backend::maybe_composite(&mut fixture.backend).expect("submit same-key image");
    assert_eq!(
        live_records(&fixture).len(),
        before_image,
        "a same-key image cannot resurrect the rejected hardware key",
    );

    retire_cursor_only(&mut fixture, 3);
    assert_eq!(
        plane_user_count(&fixture, binding.lease),
        0,
        "the detach event releases the old framebuffer",
    );

    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit software recompose");
    let scene = live_records(&fixture).pop().expect("recompose record");
    assert_eq!(scene.kind, AtomicCommitKind::Scene);
    assert!(
        scene
            .properties
            .iter()
            .all(|update| !handles.iter().any(|(_, cursor)| *cursor == update.property)),
        "the software recompose carries no cursor property",
    );
    fixture.retire_output(0, 4);
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw,
    );
    teardown(fixture);
}

fn plane_user_count(
    fixture: &AtomicCursorFixture,
    lease: crate::kms::cursor_plane::sprites::SpriteBufferLease,
) -> usize {
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    fixture
        .backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.sprite_user_count(lease))
        .unwrap_or(0)
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and seen to fail on this test):
// M6 retries the suppressed cursor work on every motion or image tick, so new cursor-only requests appear;
// M7 reports every suppressed observation to the reporter instead of the single rejected detach.
fn atomic_cursor_only_rejected_detach_keeps_hardware_without_retry_loop() {
    let reporter = Rc::new(RecordingCursorRecoveryReporter::default());
    let Some((mut fixture, handles)) = cursor_fixture(&reporter) else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let binding = applied_binding(&fixture).expect("hardware binding accepted");
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let before = live_records(&fixture).len();

    // The motion carries CRTC_X and the detach carries FB_ID: both are rejected.
    fixture
        .recorders
        .atomic
        .reject_properties(&[handle(&handles, "crtc_x"), handle(&handles, "fb_id")]);
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 40, 30);
    Backend::maybe_composite(&mut fixture.backend).expect("submit rejected motion");

    let live = live_records(&fixture)[before..].to_vec();
    assert_eq!(
        live.len(),
        2,
        "the rejected motion, then the rejected detach: {live:?}"
    );
    assert!(is_detach(&live[1], &handles), "detach shape: {:?}", live[1]);
    assert!(matches!(live[1].result, ScriptedResult::Error { .. }));
    assert_eq!(reporter.calls(), 1, "the rejected detach is reported once");

    // The plane is left as it is: the binding and its framebuffer stay accepted.
    let state = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.crtc_states.get(&crtc))
        .expect("CRTC cursor state");
    assert_eq!(
        state.applied,
        Some(binding),
        "the accepted binding is retained"
    );
    assert!(state.in_flight.is_none(), "a rejected detach owes no event");
    assert_eq!(plane_user_count(&fixture, binding.lease), 1);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));

    // Repeated motion, image and compose ticks neither resend the cursor work
    // nor report again. Each tick's scene frames retire before the next tick,
    // so later ticks submit again.
    let cursor = cursor_only_handles(&output_cursor_handles(&fixture.backend));
    let settled = live_records(&fixture).len();
    let window = fixture.backend.core.window_id;
    for tick in 0..5_i32 {
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 50 + tick, 30);
        Backend::maybe_composite(&mut fixture.backend).expect("motion tick");
        // An opaque image, so a software cursor painted into a primary is visible.
        let image = create_argb_cursor(&mut fixture.backend, [0x31, 0x42, 0x53, 0xff], 0, 0);
        Backend::define_cursor(&mut fixture.backend, None, window, image.as_raw())
            .expect("select opaque image through Backend");
        Backend::maybe_composite(&mut fixture.backend).expect("image tick");
        damage_cursor_root(&mut fixture);
        Backend::maybe_composite(&mut fixture.backend).expect("compose tick");
        retire_pending_events(&mut fixture, 10 + u32::try_from(tick).unwrap_or(0));

        // The accepted primary carries no software cursor: no footprint was
        // presented, and the pixel under the pointer matches the pixel beside it.
        assert!(
            !fixture
                .backend
                .scene
                .for_tests_cursor_footprint_was_presented(0),
            "tick {tick}: an accepted primary painted a software cursor footprint beside the retained hardware cursor",
        );
        let under_pointer = read_output_pixel(&mut fixture.backend, 51 + tick, 31);
        let beside_pointer = read_output_pixel(&mut fixture.backend, 60, 60);
        assert_eq!(
            under_pointer[..3],
            beside_pointer[..3],
            "tick {tick}: the primary pixel under the pointer carries a software cursor",
        );

        assert_eq!(
            fixture
                .backend
                .platform
                .device_for_key(device_key)
                .and_then(|device| device.cursor.crtc_states.get(&crtc))
                .and_then(|state| state.applied),
            Some(binding),
            "tick {tick}: the rejected detach keeps the accepted binding",
        );
        assert!(
            fixture.backend.platform.cursor_plane_visible_for_output(0),
            "tick {tick}: the hardware cursor stays on the plane",
        );
    }
    let ticks = &live_records(&fixture)[settled..];
    let accepted_primary: Vec<_> = ticks
        .iter()
        .filter(|record| {
            record.kind != AtomicCommitKind::CursorOnly && record.result == ScriptedResult::Ok
        })
        .collect();
    assert!(
        !accepted_primary.is_empty(),
        "the ticks submit accepted primary frames: {ticks:?}",
    );
    for record in &accepted_primary {
        assert!(
            record
                .properties
                .iter()
                .all(|update| !cursor.contains(&update.property)),
            "every accepted primary frame is cursorless: {record:?}",
        );
    }
    let suppressed_cursor_only: Vec<_> = ticks
        .iter()
        .filter(|record| record.kind == AtomicCommitKind::CursorOnly)
        .cloned()
        .collect();
    assert!(
        suppressed_cursor_only.is_empty(),
        "no cursor-only retry after the rejected detach: {suppressed_cursor_only:?}",
    );
    assert_eq!(reporter.calls(), 1, "repeated ticks are not reported again");

    // Deliver the last scene flip before the later successful lifecycle disable.
    retire_pending_events(&mut fixture, 3);
    teardown(fixture);
}

/// One composed scanout pixel of output 0.
fn read_output_pixel(
    backend: &mut crate::kms::render::backend::KmsBackend,
    x: i32,
    y: i32,
) -> [u8; 4] {
    let bytes = crate::kms::render::backend::read_scanout_region(
        backend,
        ash::vk::Rect2D {
            offset: ash::vk::Offset2D { x, y },
            extent: ash::vk::Extent2D {
                width: 1,
                height: 1,
            },
        },
        crate::kms::render::backend::ScanoutReadSelection::PermissiveDump,
    )
    .expect("read the raw composed scene output");
    bytes.try_into().expect("one BGRA pixel")
}

/// Deliver whatever flip event the output still owes, through Backend.
fn retire_pending_events(fixture: &mut AtomicCursorFixture, frame: u32) {
    if fixture.backend.scene.has_pending_page_flips()
        || fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    {
        fixture.retire_output(0, frame);
    }
}

/// A refused explicit detach waits for its flip. A newer image arrives during
/// that wait; once the flip retires, the detach is accepted and the newer key
/// qualifies for hardware. The refused key stays refused, not the newer one.
#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation (run and seen to fail): M10 records the sprite key at detach acceptance instead of the refused key, so the
// newer image is suppressed as if it had failed.
fn atomic_cursor_only_delayed_detach_requalifies_newer_key() {
    use super::round_b_producers::{begin_hw_cursor, create_argb_cursor_extent};
    use yserver_core::backend::Backend;

    let reporter = Rc::new(RecordingCursorRecoveryReporter::default());
    let Some((mut fixture, handles)) = cursor_fixture(&reporter) else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let binding = applied_binding(&fixture).expect("hardware binding accepted");

    // The motion commit is refused by CRTC_X; the explicit detach is busy.
    fixture
        .recorders
        .atomic
        .reject_properties(&[handle(&handles, "crtc_x")]);
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 40, 30);
    Backend::maybe_composite(&mut fixture.backend).expect("submit refused motion");

    let state = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.crtc_states.get(&crtc))
        .expect("CRTC cursor state")
        .clone();
    assert_eq!(state.applied, Some(binding), "the kernel kept the binding");
    assert_eq!(reporter.calls(), 0, "a busy detach is not a refusal");

    // The newer image (2x2, hotspot 1,1) arrives while the detach waits.
    fixture.recorders.atomic.clear_property_rejections();
    let window = fixture.backend.core.window_id;
    let newer =
        create_argb_cursor_extent(&mut fixture.backend, [0x21, 0x43, 0x65, 0xff], 2, 2, 1, 1);
    Backend::define_cursor(&mut fixture.backend, None, window, newer.as_raw())
        .expect("select newer image while the detach waits");
    Backend::maybe_composite(&mut fixture.backend).expect("queue newer image");
    // A real primary flip carries the waiting intent; its event retires the wait.
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit primary flip");

    // The flip retires; the pending detach goes out and the newer key attaches.
    for frame in 3..12 {
        retire_pending_events(&mut fixture, frame);
        Backend::maybe_composite(&mut fixture.backend).expect("recover after busy detach");
    }
    let state = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.crtc_states.get(&crtc))
        .expect("CRTC cursor state")
        .clone();
    let attached = state.applied.expect("the newer key is bound");
    assert_eq!(
        (attached.hot_x, attached.hot_y),
        (1, 1),
        "the newer image is shown"
    );
    assert_eq!(reporter.calls(), 0, "the newer key was never refused");
    retire_pending_events(&mut fixture, 20);
    teardown(fixture);
}
