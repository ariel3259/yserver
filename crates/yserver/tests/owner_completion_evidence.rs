use std::time::Duration;
use yserver::kms::{
    executor::{
        HostCallEvent, HostCallOutcome, UnknownReason,
        protocol::{HostCallCorrelation, RequestSeq},
        test_support::{self, StubBehaviour},
    },
    owner::{
        clock::{ClockKey, ClockSource, ProbeState},
        device::{DispatchError, OwnerEvent, ProbeOutcome},
        identity::{ClockEpochId, IncarnationId},
        lifecycle::{ClockProbeId, LifecycleEpochId},
        test_fixtures::{ledger, owner_for_tests, single_active_crtc},
    },
};

/// [ID-3, COMMIT-5, CAP-1..4] Real stub AcceptProbeWith must leave source
/// Unresolved after send and set KernelSequence only after its polled reply.
#[test]
fn a_probe_round_trip_selects_a_reference_without_a_timestamp() {
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let mut owner = owner_for_tests();
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::AcceptProbeWith(0x1_0000_0000)).unwrap();
    owner.begin_clock_probe(key).unwrap();
    owner.send_clock_probe_on(&mut executor).unwrap();
    assert_eq!(owner.clock(key).unwrap().source, ClockSource::Unresolved);
    test_support::wait_readable(executor.control_fd().unwrap(), Duration::from_secs(5));
    owner.apply_host_call_event(executor.poll_reply().unwrap());
    assert_eq!(owner.clock(key).unwrap().reference, Some(0x1_0000_0000));
    assert_eq!(owner.clock(key).unwrap().latest, None);
}

/// [ID-3, COMMIT-5, CAP-1..4] Real stub RejectProbeWith(EOPNOTSUPP) must leave
/// ProbeState::Failed and ClockSource::Unresolved, and refuse same-epoch retry.
#[test]
fn probe_rejected_with_eopnotsupp_leaves_failed_unresolved_and_refuses_same_epoch_retry() {
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let mut owner = owner_for_tests();
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::RejectProbeWith(libc::EOPNOTSUPP)).unwrap();
    owner.begin_clock_probe(key).unwrap();
    owner.send_clock_probe_on(&mut executor).unwrap();
    assert_eq!(owner.clock(key).unwrap().source, ClockSource::Unresolved);
    test_support::wait_readable(executor.control_fd().unwrap(), Duration::from_secs(5));
    let events = owner.apply_host_call_event(executor.poll_reply().unwrap());
    assert!(matches!(
        events.as_slice(),
        [OwnerEvent::ClockProbeResolved {
            key: k,
            outcome: ProbeOutcome::Rejected { errno: libc::EOPNOTSUPP },
        }] if *k == key
    ));
    assert_eq!(owner.clock(key).unwrap().source, ClockSource::Unresolved);
    assert_eq!(owner.clock(key).unwrap().probe, ProbeState::Failed);
    assert_eq!(owner.clock(key).unwrap().reference, None);

    // Refuses same-epoch retry:
    assert!(owner.begin_clock_probe(key).is_err());
}

/// [ID-3, COMMIT-5, CAP-1..4] RejectWith sends atomic Rejected (wrong family for probe),
/// yielding Unknown(MalformedReply).
#[test]
fn probe_wrong_family_reply_yields_unknown_malformed_reply_and_retains_probe_exclusion() {
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let mut owner = owner_for_tests();
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::RejectWith(libc::EINVAL)).unwrap();
    owner.begin_clock_probe(key).unwrap();
    owner.send_clock_probe_on(&mut executor).unwrap();
    test_support::wait_readable(executor.control_fd().unwrap(), Duration::from_secs(5));
    let reply_event = executor.poll_reply().unwrap();
    let events = owner.apply_host_call_event(reply_event);
    assert!(matches!(
        events.as_slice(),
        [OwnerEvent::ClockProbeResolved {
            key: k,
            outcome: ProbeOutcome::Unknown(UnknownReason::MalformedReply),
        }] if *k == key
    ));
    assert_eq!(owner.clock(key).unwrap().probe, ProbeState::Failed);

    // Unknown retains probe exclusion on slot:
    let desc = single_active_crtc();
    assert!(owner.begin(&desc, ledger()).is_err());
    assert!(owner.begin_validation(&desc).is_err());
}

/// [ID-3, COMMIT-5, CAP-1..4] Use NeverReply plus executor.next_deadline()+1ms for watchdog;
/// assert retained probe exclusion.
#[test]
fn probe_watchdog_expiry_retains_probe_exclusion() {
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let mut owner = owner_for_tests();
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).unwrap();
    owner.begin_clock_probe(key).unwrap();
    owner.send_clock_probe_on(&mut executor).unwrap();
    let deadline = executor.next_deadline().expect("watchdog deadline");
    let event = executor
        .tick(deadline + Duration::from_millis(1))
        .expect("watchdog event");
    let events = owner.apply_host_call_event(event);
    assert!(matches!(
        events.as_slice(),
        [OwnerEvent::ClockProbeResolved {
            key: k,
            outcome: ProbeOutcome::Unknown(UnknownReason::WatchdogExpired),
        }] if *k == key
    ));
    assert_eq!(owner.clock(key).unwrap().probe, ProbeState::Failed);

    // Retained probe exclusion:
    let desc = single_active_crtc();
    assert!(owner.begin(&desc, ledger()).is_err());
    assert!(owner.begin_validation(&desc).is_err());
}

/// [ID-3, COMMIT-5, CAP-1..4] Vary each full correlation field and LateReply;
/// none resolves the current probe.
#[test]
fn probe_correlation_field_variations_and_late_replies_do_not_resolve_current_probe() {
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let mut owner = owner_for_tests();
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let probe_id = owner.begin_clock_probe(key).unwrap();
    owner.mark_probe_dispatched_for_tests();

    let valid_corr = HostCallCorrelation::ClockProbe {
        seq: RequestSeq::from_raw(1),
        incarnation: IncarnationId::first(),
        lifecycle_epoch: LifecycleEpochId::first(),
        topology_generation: 1,
        hardware_crtc: 1,
        clock_epoch: ClockEpochId::first(),
        probe: probe_id,
    };

    for field in 0..7 {
        let mut corr = valid_corr;
        let HostCallCorrelation::ClockProbe {
            seq,
            incarnation,
            lifecycle_epoch,
            topology_generation,
            hardware_crtc,
            clock_epoch,
            probe,
        } = &mut corr
        else {
            unreachable!()
        };
        match field {
            0 => *seq = RequestSeq::from_raw(999),
            1 => *incarnation = IncarnationId::from_raw(999),
            2 => *lifecycle_epoch = LifecycleEpochId::from_raw(999),
            3 => *topology_generation = 999,
            4 => *hardware_crtc = 999,
            5 => *clock_epoch = ClockEpochId::from_raw(999),
            6 => *probe = ClockProbeId::from_raw(999),
            _ => unreachable!(),
        }
        let event = HostCallEvent::Outcome {
            correlation: corr,
            outcome: HostCallOutcome::ProbeAccepted {
                sequence: 100,
                helper_duration_ns: 0,
                round_trip_ns: 0,
            },
        };
        let events = owner.apply_host_call_event(event);
        assert!(matches!(events.as_slice(), [OwnerEvent::StaleReply { .. }]));
        assert_eq!(owner.clock(key).unwrap().source, ClockSource::Unresolved);
        assert_eq!(
            owner.clock(key).unwrap().probe,
            ProbeState::InFlight(probe_id)
        );
    }

    let late_event = HostCallEvent::LateReply {
        correlation: valid_corr,
        outcome: HostCallOutcome::ProbeAccepted {
            sequence: 100,
            helper_duration_ns: 0,
            round_trip_ns: 0,
        },
    };
    let events = owner.apply_host_call_event(late_event);
    assert!(matches!(events.as_slice(), [OwnerEvent::StaleReply { .. }]));
    assert_eq!(owner.clock(key).unwrap().source, ClockSource::Unresolved);
    assert_eq!(
        owner.clock(key).unwrap().probe,
        ProbeState::InFlight(probe_id)
    );
}

/// [ID-3, COMMIT-5, CAP-1..4] Test mutual exclusion with validation/atomic and refusal in legacy mode.
#[test]
fn probe_refused_in_legacy_mode_and_mutual_exclusion_with_commit_and_validation() {
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };

    // Refusal in legacy mode:
    let mut legacy_owner = yserver::kms::owner::test_fixtures::legacy_owner_for_tests();
    legacy_owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    assert!(matches!(
        legacy_owner.begin_clock_probe(key),
        Err(DispatchError::LegacyTransportActive)
    ));

    // Mutual exclusion: active commit blocks probe
    let mut owner = owner_for_tests();
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let desc = single_active_crtc();
    let _ = owner.begin(&desc, ledger()).unwrap();
    assert!(owner.begin_clock_probe(key).is_err());

    // Mutual exclusion: active validation blocks probe
    let mut owner2 = owner_for_tests();
    owner2
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let _ = owner2.begin_validation(&desc).unwrap();
    assert!(owner2.begin_clock_probe(key).is_err());

    // Mutual exclusion: active probe blocks begin and begin_validation
    let mut owner3 = owner_for_tests();
    owner3
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let _probe = owner3.begin_clock_probe(key).unwrap();
    assert!(owner3.begin(&desc, ledger()).is_err());
    assert!(owner3.begin_validation(&desc).is_err());
}
