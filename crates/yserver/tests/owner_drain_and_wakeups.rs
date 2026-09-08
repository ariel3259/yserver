//! Integration tests for Task 7:
//! Backend wiring: legacy drain, wakeups, admission control, and failure latching.
//!
//! Covers:
//! - Step 1: `poll_fds` includes aggregate poller fd under `OwnerCompletion`;
//!   adopted fence / eventfd wakes poller; multi-device routing & token isolation;
//!   partial raw buffer with valid prefix delivers valid event and latches stream error.
//! - Step 2 & 2a: Legacy drain and handover proof issuance: admission control refusal,
//!   pending flips refusal, event dispositions (Applied, RecipientGone, StaleOrAlreadyTerminal, BackendFailure),
//!   failure latching refusing future handovers on the incarnation, and proof consumption.
//! - Step 5: `next_wakeup` includes `owner_completion_deadline` even when seat is inactive (`kms_outputs_active = false`).

#![cfg(target_os = "linux")]

use std::{
    io::Write,
    os::{
        fd::{AsFd, AsRawFd},
        unix::net::UnixStream,
    },
    time::{Duration, Instant},
};

use nix::sys::eventfd::{EfdFlags, EventFd};
use yserver::{
    drm::{Device, DrainStop, DrmEventRecord},
    kms::{
        executor::HostCallClass,
        owner::{
            clock::ClockKey,
            completion::{CompletionClass, CompletionContext, MechanismFailure},
            device::{DeviceCommitOwner, OwnerEvent},
            identity::{ClockEpochId, IncarnationId},
            lifecycle::LifecycleEpochId,
            test_fixtures::*,
        },
        render::{KmsBackend, LegacyEventCancellation, LegacyEventDisposition},
    },
    platform::drm::DrmDeviceKey,
};
use yserver_core::{
    backend::{Backend, BackendFdKind},
    core_loop::{CoreReceiver, Message, channel},
};

fn recv_core_message(rx: &CoreReceiver) -> Option<Message> {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if let Some(msg) = rx.try_recv_all().next() {
            return Some(msg);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

fn encode_page_flip(
    crtc_id: u32,
    sequence: u32,
    tv_sec: u32,
    tv_usec: u32,
    user_data: u64,
) -> [u8; 32] {
    let mut buf = [0u8; 32];
    buf[0..4].copy_from_slice(&2u32.to_ne_bytes()); // DRM_EVENT_FLIP_COMPLETE = 2
    buf[4..8].copy_from_slice(&32u32.to_ne_bytes());
    buf[8..16].copy_from_slice(&user_data.to_ne_bytes());
    buf[16..20].copy_from_slice(&tv_sec.to_ne_bytes());
    buf[20..24].copy_from_slice(&tv_usec.to_ne_bytes());
    buf[24..28].copy_from_slice(&sequence.to_ne_bytes());
    buf[28..32].copy_from_slice(&crtc_id.to_ne_bytes());
    buf
}

/// Step 1: `poll_fds` exposes aggregate poller under `OwnerCompletion`;
/// readiness on an adopted descriptor wakes the poller; mechanism failure detaches
/// the poller and signals shutdown to the core loop.
#[test]
fn owner_completion_poller_in_poll_fds_and_wakes_on_event() {
    let mut backend = KmsBackend::for_tests();
    let (_poll, sender, rx) = channel().unwrap();
    backend.set_input_sender(sender);

    // 1. Verify OwnerCompletion is published in poll_fds
    let fds = backend.poll_fds();
    let poller_entry = fds
        .iter()
        .find(|(_, kind)| *kind == BackendFdKind::OwnerCompletion);
    assert!(
        poller_entry.is_some(),
        "poll_fds must contain OwnerCompletion"
    );
    let poller_fd = poller_entry.unwrap().0;

    // 2. Register an eventfd with the aggregate poller
    let event_fd = EventFd::from_value_and_flags(0, EfdFlags::EFD_CLOEXEC | EfdFlags::EFD_NONBLOCK)
        .expect("eventfd");
    backend
        .platform
        .owner_completion_poller
        .register(event_fd.as_fd(), 42)
        .expect("register eventfd");

    // Initially, poller fd has no readiness
    let mut poll_fd = libc::pollfd {
        fd: poller_fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let res = unsafe { libc::poll(&mut poll_fd, 1, 0) };
    assert_eq!(res, 0, "poller should not be readable before event");

    // Signal eventfd
    event_fd.write(1).expect("write eventfd");

    // Poller fd becomes readable!
    let res = unsafe { libc::poll(&mut poll_fd, 1, 1000) };
    assert_eq!(res, 1, "poller must be readable after eventfd is signaled");
    assert_ne!(
        poll_fd.revents & libc::POLLIN,
        0,
        "poller must report POLLIN"
    );

    // 3. Mechanism failure detaches poller and requests clean exit
    let dev_key = DrmDeviceKey { major: 0, minor: 0 };
    let disp = backend.dispose_legacy_drain_event(
        dev_key,
        OwnerEvent::MechanismFailed {
            reason: MechanismFailure::FencePollError,
        },
    );
    assert_eq!(
        disp,
        LegacyEventDisposition::Cancelled(LegacyEventCancellation::BackendFailure)
    );
    assert!(
        backend.platform.owner_completion_detached,
        "owner_completion_detached must be true after FencePollError"
    );

    // poll_fds must now omit OwnerCompletion
    let fds_after = backend.poll_fds();
    assert!(
        !fds_after
            .iter()
            .any(|(_, kind)| *kind == BackendFdKind::OwnerCompletion),
        "poll_fds must omit OwnerCompletion after detaching"
    );

    // Verify CoreSender received Message::Shutdown
    let msg = recv_core_message(&rx).expect("must receive shutdown message");
    assert!(
        matches!(msg, Message::Shutdown),
        "request_exit must send Message::Shutdown"
    );
}

/// Step 1: Raw events on a second device update only that device's owner, even
/// when raw incarnation numbers collide.
#[test]
fn multi_device_drain_routing_and_token_isolation() {
    let mut backend = KmsBackend::for_tests();

    // Device 1 is (0, 0) created by for_tests() with IncarnationId::first()
    let key1 = DrmDeviceKey { major: 0, minor: 0 };
    let key2 = DrmDeviceKey {
        major: 226,
        minor: 1,
    };

    // Create a unix stream pair to act as device 2's DRM fd
    let (reader, mut writer) = UnixStream::pair().unwrap();
    reader.set_nonblocking(true).unwrap();
    let reader_raw_fd = reader.as_raw_fd();
    let drm_device2 = Device::from_inherited_kms_fd(reader.into(), "/dev/dri/card1");

    // Add device 2 with the same incarnation number to test collision safety
    backend.platform.add_test_device_with_drm_device(
        key2,
        drm_device2,
        IncarnationId::first(),
        LifecycleEpochId::first(),
    );

    // Encode a legacy page flip event (user_data == 0) for CRTC 1
    let raw_event = encode_page_flip(1, 100, 10, 500, 0);
    writer.write_all(&raw_event).unwrap();

    let now = Instant::now();
    let (events, drain_stop) = backend.platform.drain_owner_events(reader_raw_fd, now);

    assert_eq!(drain_stop.unwrap(), DrainStop::WouldBlock);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, key2, "event must be attributed to device 2");
    assert!(
        matches!(
            events[0].1,
            OwnerEvent::LegacyPageFlip {
                crtc_id: 1,
                sequence: 100,
                tv_sec: 10,
                tv_usec: 500,
            }
        ),
        "event contents must match raw packet"
    );

    // Verify device 1 owner was untouched
    let owner1 = backend.platform.owner_ref(key1).unwrap();
    assert_eq!(owner1.incarnation(), IncarnationId::first());
}

/// Step 1: Partial buffer with one valid event followed by a malformed tail
/// delivers the valid event AND latches the stream failure. Neither is dropped.
#[test]
fn partial_buffer_delivers_valid_prefix_and_latches_malformed_error() {
    let mut backend = KmsBackend::for_tests();
    let key = DrmDeviceKey {
        major: 226,
        minor: 2,
    };

    let (reader, mut writer) = UnixStream::pair().unwrap();
    reader.set_nonblocking(true).unwrap();
    let reader_raw_fd = reader.as_raw_fd();
    let drm_dev = Device::from_inherited_kms_fd(reader.into(), "/dev/dri/card2");

    backend.platform.add_test_device_with_drm_device(
        key,
        drm_dev,
        IncarnationId::first(),
        LifecycleEpochId::first(),
    );

    // 32-byte valid event + 5 bytes trailing malformed header
    let valid_event = encode_page_flip(1, 42, 5, 250, 0);
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&valid_event);
    buffer.extend_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55]); // 5 bytes: less than 8-byte header!

    writer.write_all(&buffer).unwrap();

    let now = Instant::now();
    let (events, drain_res) = backend.platform.drain_owner_events(reader_raw_fd, now);

    // Drain result must be an error due to truncated tail
    assert!(drain_res.is_err(), "drain_res must report stream error");

    // Events must contain BOTH the valid prefix and the reported failure
    assert_eq!(events.len(), 2, "must retain prefix and report failure");
    assert!(
        matches!(
            events[0].1,
            OwnerEvent::LegacyPageFlip {
                crtc_id: 1,
                sequence: 42,
                ..
            }
        ),
        "first event must be the valid prefix"
    );
    assert!(
        matches!(events[1].1, OwnerEvent::MechanismFailed { .. }),
        "second event must be the latched stream failure"
    );
}

/// Step 2 & 2a: Handover refusal when admission is not stopped or when flips are pending.
#[test]
fn legacy_handover_refuses_when_admission_not_stopped_or_flips_pending() {
    let mut backend = KmsBackend::for_tests();
    let key = DrmDeviceKey { major: 0, minor: 0 };
    let now = Instant::now();

    // 1. Admission not stopped -> PermissionDenied
    backend.set_stopped_admission_for_tests(false);
    let err = backend
        .try_finish_legacy_transport(key, now)
        .expect_err("must refuse handover when admission not stopped");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

    // 2. Admission stopped, but pending flips exist -> ResourceBusy
    backend.set_stopped_admission_for_tests(true);
    backend.set_pending_flips_for_tests(true);
    let err = backend
        .try_finish_legacy_transport(key, now)
        .expect_err("must refuse handover when flips are pending");
    assert_eq!(err.kind(), std::io::ErrorKind::ResourceBusy);

    // 3. Clear pending flips -> proceeds past early guards
    backend.set_pending_flips_for_tests(false);
    assert!(!backend.has_pending_flips_for_device(key));
}

/// Step 2a: `dispose_legacy_drain_event` classifies:
/// - Applied: live recipient CRTC
/// - Cancelled(RecipientGone): valid CRTC with no active output
/// - Cancelled(StaleOrAlreadyTerminal): invalid CRTC handle
/// - Cancelled(BackendFailure): forced error or mechanism failure
#[test]
fn dispose_legacy_drain_event_classifications() {
    let mut backend = KmsBackend::for_tests();
    let (_poll, sender, _rx) = channel().unwrap();
    backend.set_input_sender(sender);
    let key = DrmDeviceKey { major: 0, minor: 0 };

    // 1. Applied: CRTC 1 is mapped to output 0 in for_tests()
    let ev_applied = OwnerEvent::LegacyPageFlip {
        crtc_id: 1,
        sequence: 1,
        tv_sec: 1,
        tv_usec: 0,
    };
    let disp1 = backend.dispose_legacy_drain_event(key, ev_applied);
    assert_eq!(disp1, LegacyEventDisposition::Applied);

    // 2. RecipientGone: CRTC 2 is a valid handle but has no mapped output
    let ev_gone = OwnerEvent::LegacyPageFlip {
        crtc_id: 2,
        sequence: 2,
        tv_sec: 1,
        tv_usec: 0,
    };
    let disp2 = backend.dispose_legacy_drain_event(key, ev_gone);
    assert_eq!(
        disp2,
        LegacyEventDisposition::Cancelled(LegacyEventCancellation::RecipientGone)
    );

    // 3. StaleOrAlreadyTerminal: CRTC 0 is an invalid handle
    let ev_stale = OwnerEvent::LegacyPageFlip {
        crtc_id: 0,
        sequence: 3,
        tv_sec: 1,
        tv_usec: 0,
    };
    let disp3 = backend.dispose_legacy_drain_event(key, ev_stale);
    assert_eq!(
        disp3,
        LegacyEventDisposition::Cancelled(LegacyEventCancellation::StaleOrAlreadyTerminal)
    );

    // 4. BackendFailure: forced via force_dispose_failure_crtc_for_tests
    backend.force_dispose_failure_crtc_for_tests = Some(1);
    let ev_fail = OwnerEvent::LegacyPageFlip {
        crtc_id: 1,
        sequence: 4,
        tv_sec: 1,
        tv_usec: 0,
    };
    let disp4 = backend.dispose_legacy_drain_event(key, ev_fail);
    assert_eq!(
        disp4,
        LegacyEventDisposition::Cancelled(LegacyEventCancellation::BackendFailure)
    );
    assert!(
        backend
            .legacy_handover_failed
            .contains(&IncarnationId::first()),
        "incarnation must be latched in legacy_handover_failed"
    );
}

/// Step 2a: Handover ordering matrix with events A/B/C:
/// - Three live recipients: all Applied, proof issued & consumed, permit removed.
/// - Recipient gone on B: B receives RecipientGone, A/C Applied, proof issued & consumed.
/// - Forced internal failure on B: B receives BackendFailure, C still receives disposition,
///   failure latched, shutdown requested, NO proof consumed, permit remains.
/// - Subsequent handover call on failed incarnation refuses immediately.
#[test]
fn legacy_handover_matrix_and_failure_latching() {
    let now = Instant::now();

    // Scenario 1: Three live recipients A/B/C -> all Applied, proof consumed
    {
        let mut backend = KmsBackend::for_tests();
        backend.set_stopped_admission_for_tests(true);
        let key = DrmDeviceKey { major: 0, minor: 0 };

        let ev_a = OwnerEvent::LegacyPageFlip {
            crtc_id: 1,
            sequence: 10,
            tv_sec: 1,
            tv_usec: 0,
        };
        let ev_b = OwnerEvent::LegacyPageFlip {
            crtc_id: 1,
            sequence: 11,
            tv_sec: 1,
            tv_usec: 1,
        };
        let ev_c = OwnerEvent::LegacyPageFlip {
            crtc_id: 1,
            sequence: 12,
            tv_sec: 1,
            tv_usec: 2,
        };

        // Dispose A, B, C
        assert_eq!(
            backend.dispose_legacy_drain_event(key, ev_a),
            LegacyEventDisposition::Applied
        );
        assert_eq!(
            backend.dispose_legacy_drain_event(key, ev_b),
            LegacyEventDisposition::Applied
        );
        assert_eq!(
            backend.dispose_legacy_drain_event(key, ev_c),
            LegacyEventDisposition::Applied
        );

        // Finish handover
        backend
            .try_finish_legacy_transport(key, now)
            .expect("handover must succeed when drained");
        let owner = backend.platform.owner_ref(key).unwrap();
        assert!(
            !owner.has_legacy_drain_permit(),
            "permit must be removed after successful handover"
        );
    }

    // Scenario 2: B's recipient is gone -> B receives RecipientGone, A & C Applied, proof consumed
    {
        let mut backend = KmsBackend::for_tests();
        backend.set_stopped_admission_for_tests(true);
        let key = DrmDeviceKey { major: 0, minor: 0 };

        let ev_a = OwnerEvent::LegacyPageFlip {
            crtc_id: 1,
            sequence: 20,
            tv_sec: 2,
            tv_usec: 0,
        };
        let ev_b = OwnerEvent::LegacyPageFlip {
            crtc_id: 2, // No output -> RecipientGone
            sequence: 21,
            tv_sec: 2,
            tv_usec: 1,
        };
        let ev_c = OwnerEvent::LegacyPageFlip {
            crtc_id: 1,
            sequence: 22,
            tv_sec: 2,
            tv_usec: 2,
        };

        assert_eq!(
            backend.dispose_legacy_drain_event(key, ev_a),
            LegacyEventDisposition::Applied
        );
        assert_eq!(
            backend.dispose_legacy_drain_event(key, ev_b),
            LegacyEventDisposition::Cancelled(LegacyEventCancellation::RecipientGone)
        );
        assert_eq!(
            backend.dispose_legacy_drain_event(key, ev_c),
            LegacyEventDisposition::Applied
        );

        backend
            .try_finish_legacy_transport(key, now)
            .expect("handover must succeed despite RecipientGone");
        let owner = backend.platform.owner_ref(key).unwrap();
        assert!(!owner.has_legacy_drain_permit());
    }

    // Scenario 3: Forced internal failure on B -> B receives BackendFailure, C still disposed,
    // shutdown requested, NO proof consumed, and subsequent call is refused.
    {
        let mut backend = KmsBackend::for_tests();
        let (_poll, sender, rx) = channel().unwrap();
        backend.set_input_sender(sender);
        backend.set_stopped_admission_for_tests(true);
        let key = DrmDeviceKey { major: 0, minor: 0 };

        let ev_a = OwnerEvent::LegacyPageFlip {
            crtc_id: 1,
            sequence: 30,
            tv_sec: 3,
            tv_usec: 0,
        };
        let ev_b = OwnerEvent::LegacyPageFlip {
            crtc_id: 2,
            sequence: 31,
            tv_sec: 3,
            tv_usec: 1,
        };
        let ev_c = OwnerEvent::LegacyPageFlip {
            crtc_id: 1,
            sequence: 32,
            tv_sec: 3,
            tv_usec: 2,
        };

        // Force failure on CRTC 2
        backend.force_dispose_failure_crtc_for_tests = Some(2);

        assert_eq!(
            backend.dispose_legacy_drain_event(key, ev_a),
            LegacyEventDisposition::Applied
        );
        assert_eq!(
            backend.dispose_legacy_drain_event(key, ev_b),
            LegacyEventDisposition::Cancelled(LegacyEventCancellation::BackendFailure)
        );
        // C still receives a disposition!
        assert_eq!(
            backend.dispose_legacy_drain_event(key, ev_c),
            LegacyEventDisposition::Applied
        );

        // Failure latch is set
        assert!(
            backend
                .legacy_handover_failed
                .contains(&IncarnationId::first())
        );

        // Shutdown was requested
        let msg = recv_core_message(&rx).expect("shutdown message");
        assert!(matches!(msg, Message::Shutdown));

        // Permit remains on owner (NO proof consumed)
        let owner = backend.platform.owner_ref(key).unwrap();
        assert!(
            owner.has_legacy_drain_permit(),
            "permit must remain when handover fails"
        );

        // Second handover call refuses immediately without issuing proof
        let err = backend
            .try_finish_legacy_transport(key, now)
            .expect_err("must refuse after failed handover");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string()
                .contains("handover previously failed for this incarnation")
        );
    }
}

/// Step 5: `next_wakeup` incorporates `owner_completion_deadline` even when
/// the seat is inactive (`kms_outputs_active = false`).
#[test]
fn next_wakeup_incorporates_owner_completion_deadline_even_when_seat_inactive() {
    let mut backend = KmsBackend::for_tests();
    let key = DrmDeviceKey {
        major: 226,
        minor: 5,
    };

    // Construct an owner with an accepted commit with a hardware deadline
    let mut owner = never_owner_for_tests();
    let clock_key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(clock_key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(clock_key).unwrap().install_reference(100);
    let desc = single_active_crtc_with_present(1);
    let mut mode_periods = std::collections::BTreeMap::new();
    mode_periods.insert(1, Some(Duration::from_millis(40)));

    let mut clocks = std::collections::BTreeMap::new();
    clocks.insert(1, clock_key);

    let context = CompletionContext {
        class: CompletionClass::FastUpdate,
        host_class: HostCallClass::SeatActiveNonblock,
        allow_modeset: false,
        clocks,
        mode_periods,
        lifecycle_observed_max: None,
    };

    let (commit, _) = owner
        .begin_with_context(&desc, never_ledger(), context)
        .unwrap();
    owner.mark_dispatched_for_tests();

    let t0 = Instant::now();
    let hw_expected_dur = Duration::from_millis(120);
    let expected_deadline = t0 + hw_expected_dur;
    owner.apply_host_call_event_at(accepted(commit, 0b1, 1), t0);

    assert_eq!(owner.completion_deadline(), Some(expected_deadline));

    // Install owner onto backend
    let drm_device = Device::for_tests().expect("drm device");
    backend
        .platform
        .add_test_device_with_owner(key, drm_device, owner);

    // With active KMS outputs: next_wakeup must include the owner deadline
    let wakeup = backend.next_wakeup().expect("must have wakeup deadline");
    assert_eq!(wakeup, expected_deadline);

    // Inactive seat (e.g. DPMS off, VT switched away): kms_outputs_active = false
    backend.kms_outputs_active = false;

    // next_wakeup STILL incorporates owner_completion_deadline!
    let wakeup_inactive = backend
        .next_wakeup()
        .expect("must have owner completion deadline even when seat inactive");
    assert_eq!(
        wakeup_inactive, expected_deadline,
        "owner deadline must be chained outside allow_kms_timers"
    );
}

/// Verification of legacy zero-token flip vs C.0 token isolation.
#[test]
fn legacy_zero_token_flip_vs_c0_token_isolation() {
    let now = Instant::now();

    // 1. Owner with LegacyDrainPermit receives user_data == 0 -> produces LegacyPageFlip
    let mut legacy_owner: DeviceCommitOwner<yserver::kms::owner::NeverResource> =
        DeviceCommitOwner::new_legacy(IncarnationId::first(), LifecycleEpochId::first(), 1);
    assert!(legacy_owner.has_legacy_drain_permit());

    let flip_zero = DrmEventRecord::PageFlip {
        crtc_id: 1,
        sequence: 42,
        tv_sec: 1,
        tv_usec: 500,
        user_data: 0,
    };
    let events = legacy_owner.apply_drm_event(IncarnationId::first(), flip_zero, now);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        OwnerEvent::LegacyPageFlip {
            crtc_id: 1,
            sequence: 42,
            tv_sec: 1,
            tv_usec: 500,
        }
    ));

    // 2. Owner without LegacyDrainPermit receives user_data == 0 -> ignored (empty)
    let mut c0_owner = owner_for_tests();
    assert!(!c0_owner.has_legacy_drain_permit());
    let events2 = c0_owner.apply_drm_event(c0_owner.incarnation(), flip_zero, now);
    assert!(
        events2.is_empty(),
        "user_data == 0 without permit must be ignored"
    );

    // 3. A non-zero token (e.g. 0xdead_beef) NEVER produces LegacyPageFlip
    let flip_c0_token = DrmEventRecord::PageFlip {
        crtc_id: 1,
        sequence: 43,
        tv_sec: 1,
        tv_usec: 600,
        user_data: 0xdead_beef,
    };
    let events3 = legacy_owner.apply_drm_event(IncarnationId::first(), flip_c0_token, now);
    assert!(
        !events3
            .iter()
            .any(|e| matches!(e, OwnerEvent::LegacyPageFlip { .. })),
        "C.0 token must never produce LegacyPageFlip"
    );
}
