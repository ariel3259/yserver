//! Present wake sources on KMS devices without `CRTC_QUEUE_SEQUENCE`.

use super::*;
use std::{
    cell::Cell,
    os::fd::{AsFd as _, AsRawFd as _},
    rc::Rc,
    time::{Duration, Instant},
};
use yserver_core::backend::PresentClockSource;

/// RANDR CRTC bound to the second, never-flipping output.
const SECOND_CRTC_XID: u32 = 0x5400;

fn recording_source(
    b: &KmsBackend,
) -> Rc<crate::kms::render::platform::flip_events::RecordingPageFlipEventSource> {
    b.platform.devices[0]
        .test_page_flip_event_source
        .clone()
        .expect("fixture records page-flip events")
}

/// Two outputs on one device; Present bound to output 1 which never flips.
/// Output 1's MSC stands at 5 with a completion sample at the same MSC.
fn two_output_fixture() -> (KmsBackend, CrtcKey, Rc<Cell<Instant>>) {
    let mut b = KmsBackend::for_tests();
    push_test_output(&mut b, 2);
    b.pstate.direct.sync_output_count(b.platform.outputs.len());
    let output_key = b.platform.outputs[1].key.clone();
    b.pstate.crtc_key_by_id.insert(SECOND_CRTC_XID, output_key);
    b.refresh_present_crtc_clock_epochs();
    b.scene.scene_structure_dirty = false;
    let second = CrtcKey::for_output(&b.platform.outputs[1]);
    b.platform.vblank_fallback.pipes.insert(second, 1);
    b.platform.record_vblank_clock(second, 5, 1_000);
    b.platform.record_completion_clock(
        second,
        yserver_core::backend::PresentClockSample {
            msc: 5,
            ust: 1_000,
            source: PresentClockSource::PageFlip,
        },
        None,
    );
    let now = Rc::new(Cell::new(Instant::now()));
    let clock = Rc::clone(&now);
    b.for_tests_set_monotonic_clock(Rc::new(move || clock.get()));
    (b, second, now)
}

fn drain_kernel_events(b: &mut KmsBackend) {
    let fd = b.platform.devices[0].device.as_fd().as_raw_fd();
    let (_, sequences) = b
        .platform
        .drain_page_flip_events(fd)
        .expect("recorded events drain");
    for seq in sequences {
        b.on_crtc_sequence_event(seq.device_key, seq.user_data, seq.time_ns, seq.sequence);
    }
}

/// The marco freeze: neither CRTC_QUEUE_SEQUENCE nor WAIT_VBLANK on the
/// device, CompleteNotify held for MSC 6 on an output that never flips. The
/// completion clock the core's due check reads must still reach 6.
#[test]
fn held_completion_on_a_non_flipping_output_advances_without_kernel_vblank() {
    let (mut b, _, now) = two_output_fixture();
    recording_source(&b).set_legacy_vblank(Err(libc::EOPNOTSUPP));
    let mut state = yserver_core::server::ServerState::new();

    let armed = b
        .arm_present_completion_idle_vblanks(SECOND_CRTC_XID, &[6])
        .expect("arming never errors on an unsupported device");
    assert_eq!(armed, 1, "a software vblank stands in for the kernel one");
    assert!(!b.present_absolute_vblank_arm_supported(SECOND_CRTC_XID));
    let tick = now.get() + Duration::from_nanos(16_666_666);
    assert_eq!(b.platform.next_software_vblank_deadline(), Some(tick));
    let wakeup = b.next_wakeup().expect("the software vblank wakes the loop");
    assert!(wakeup <= tick);

    now.set(now.get() + Duration::from_millis(17));
    b.poll_deferred_input(&mut state);

    let clock = b.present_get_completion_clock(SECOND_CRTC_XID);
    assert_eq!(clock.msc, 6, "CompleteNotify for MSC 6 is now due");
    assert_eq!(clock.source, PresentClockSource::IdleSequence);
    assert_eq!(b.present_get_ust_msc(SECOND_CRTC_XID).0, 6);
    let first = CrtcKey::for_output(&b.platform.outputs[0]);
    assert_eq!(
        b.platform.present_get_ust_msc(first).0,
        0,
        "the other output's clock is untouched"
    );
    assert!(b.platform.vblank_fallback.software.is_empty());
}

#[test]
fn software_vblank_does_not_double_count_an_msc_another_source_advanced() {
    let (mut b, second, now) = two_output_fixture();
    recording_source(&b).set_legacy_vblank(Err(libc::EOPNOTSUPP));
    let mut state = yserver_core::server::ServerState::new();
    b.arm_present_completion_idle_vblanks(SECOND_CRTC_XID, &[6])
        .unwrap();

    // A page flip on this output lands first.
    b.platform.record_vblank_clock(second, 6, 2_000);
    now.set(now.get() + Duration::from_millis(17));
    b.poll_deferred_input(&mut state);

    assert_eq!(b.present_get_ust_msc(SECOND_CRTC_XID).0, 6);
    assert!(b.platform.vblank_fallback.software.is_empty());
}

#[test]
fn software_vblank_waits_one_frame_period() {
    let (mut b, _, now) = two_output_fixture();
    recording_source(&b).set_legacy_vblank(Err(libc::EOPNOTSUPP));
    let mut state = yserver_core::server::ServerState::new();
    b.arm_present_completion_idle_vblanks(SECOND_CRTC_XID, &[6])
        .unwrap();

    now.set(now.get() + Duration::from_millis(10));
    b.poll_deferred_input(&mut state);
    assert_eq!(b.present_get_completion_clock(SECOND_CRTC_XID).msc, 5);
    assert_eq!(
        b.arm_present_completion_idle_vblanks(SECOND_CRTC_XID, &[6])
            .unwrap(),
        0,
        "one armed software vblank per CRTC"
    );
}

#[test]
fn software_vblanks_stop_while_the_outputs_are_off() {
    let (mut b, _, now) = two_output_fixture();
    recording_source(&b).set_legacy_vblank(Err(libc::EOPNOTSUPP));
    let mut state = yserver_core::server::ServerState::new();
    b.arm_present_completion_idle_vblanks(SECOND_CRTC_XID, &[6])
        .unwrap();

    b.kms_outputs_active = false;
    now.set(now.get() + Duration::from_millis(17));
    b.poll_deferred_input(&mut state);

    assert_eq!(b.present_get_ust_msc(SECOND_CRTC_XID).0, 5);
    assert!(b.platform.next_software_vblank_deadline().is_none());
}

/// Xorg `ms_queue_vblank`: once the device rejects CRTC_QUEUE_SEQUENCE the
/// tagged next-vblank wake is queued through WAIT_VBLANK, and its event releases
/// the held completion through the sequence-event path.
#[test]
fn wait_vblank_fallback_arms_shared_wake_and_its_event_releases_completion() {
    let (mut b, second, _) = two_output_fixture();
    let source = recording_source(&b);
    source.set_legacy_vblank(Ok(5));
    let crtc_id = u32::from(second.crtc);

    assert!(b.present_absolute_vblank_arm_supported(SECOND_CRTC_XID));
    let armed =
        Backend::arm_present_shared_wake(&mut b, SECOND_CRTC_XID).expect("legacy arm succeeds");
    assert!(armed);
    assert!(
        b.pstate
            .crtc_queue_sequence_unsupported_devices
            .contains(&second.device_key),
        "the fixture device rejects CRTC_QUEUE_SEQUENCE"
    );
    assert_eq!(
        source.legacy_vblank_arms(),
        [(
            1,
            true,
            1,
            crtc_id | crate::drm::page_flip::LEGACY_VBLANK_ABSOLUTE_SIGNAL
        )]
    );
    assert!(b.present_absolute_vblank_arm_supported(SECOND_CRTC_XID));

    // Busy scene: only an explicitly armed (tagged) event may release it.
    b.scene.scene_structure_dirty = true;
    source.fire_legacy_vblank_arms(3_000_000, 6);
    drain_kernel_events(&mut b);

    let clock = b.present_get_completion_clock(SECOND_CRTC_XID);
    assert_eq!((clock.msc, clock.ust), (6, 3_000));
    assert!(!b.pstate.shared_wake_armed.contains(&second));
}

#[test]
fn wait_vblank_fallback_arms_the_relative_idle_vblank() {
    let (mut b, second, _) = two_output_fixture();
    let source = recording_source(&b);
    source.set_legacy_vblank(Ok(5));
    let crtc_id = u32::from(second.crtc);

    assert_eq!(
        b.arm_present_completion_idle_vblanks(SECOND_CRTC_XID, &[6])
            .unwrap(),
        1
    );
    assert_eq!(source.legacy_vblank_arms(), [(1, true, 1, crtc_id)]);
    assert!(b.platform.vblank_fallback.software.is_empty());

    source.fire_legacy_vblank_arms(4_000_000, 6);
    drain_kernel_events(&mut b);
    assert_eq!(b.present_get_completion_clock(SECOND_CRTC_XID).msc, 6);
    assert!(!b.pstate.armed_vblank_targets.contains_key(&second));
}

#[test]
fn frame_zero_flip_takes_the_legacy_vblank_count() {
    let (mut b, second, _) = two_output_fixture();
    let source = recording_source(&b);
    source.set_legacy_vblank(Ok(4_242));
    source.queue_flip(second.crtc, 0);
    drain_kernel_events(&mut b);
    assert_eq!(b.platform.present_get_ust_msc(second).0, 4_242);
    assert_eq!(source.legacy_vblank_queries(), 1);
}

#[test]
fn frame_zero_flip_without_wait_vblank_keeps_the_software_count() {
    let (mut b, second, _) = two_output_fixture();
    let source = recording_source(&b);
    source.set_legacy_vblank(Err(libc::EOPNOTSUPP));
    b.platform.software_msc.insert(second, 7);
    source.queue_flip(second.crtc, 0);
    source.queue_flip(second.crtc, 0);
    drain_kernel_events(&mut b);
    assert_eq!(b.platform.present_get_ust_msc(second).0, 9);
    assert_eq!(
        source.legacy_vblank_queries(),
        1,
        "the unsupported query is latched after one try"
    );
    assert!(!b.platform.wait_vblank_supported(second.device_key));
}

#[test]
fn legacy_absolute_user_data_matches_the_sequence_tag() {
    assert_eq!(
        crate::drm::page_flip::SEQUENCE_ABSOLUTE_USER_DATA,
        crate::kms::render::backend::ABSOLUTE_SEQ_TAG
    );
}

#[test]
fn frame_interval_follows_the_mode_timing() {
    use crate::kms::render::platform::vblank_fallback::mode_frame_interval;
    let exact = crate::platform::drm::Mode {
        clock_khz: 148_500,
        htotal: 2_200,
        vtotal: 1_125,
        vrefresh: 60,
        ..Default::default()
    };
    assert_eq!(
        mode_frame_interval(&exact),
        Duration::from_nanos(16_666_666)
    );
    let nominal = crate::platform::drm::Mode {
        vrefresh: 75,
        ..Default::default()
    };
    assert_eq!(
        mode_frame_interval(&nominal),
        Duration::from_nanos(13_333_333)
    );
}
