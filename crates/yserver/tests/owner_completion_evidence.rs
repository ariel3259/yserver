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

/// [ID-1..3, COMMIT-5, CAP-1..4, MULTI] 2-second watchdog expiry on SequenceQueue
/// retains queue lease on slot and latches mechanism failure.
#[test]
fn sequence_queue_watchdog_expiry_retains_queue_exclusion_and_poisons() {
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let mut owner = owner_for_tests();
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let clock = owner.clock_mut(key).unwrap();
    clock.install_reference(10);

    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).unwrap();
    let c = yserver::kms::owner::sequence::SequenceConsumer(5001);
    let token = owner
        .reserve_arm(
            key,
            yserver::kms::owner::sequence::SequencePurpose::PresentTargetWake,
            50,
            &[c],
        )
        .unwrap();
    owner.send_next_sequence_on(&mut executor).unwrap();

    let deadline = executor.next_deadline().expect("watchdog deadline");
    let event = executor
        .tick(deadline + Duration::from_millis(1))
        .expect("watchdog event");
    let events = owner.apply_host_call_event(event);
    assert!(owner.is_poisoned());
    assert!(matches!(
        events.as_slice(),
        [OwnerEvent::MechanismFailed {
            reason: yserver::kms::owner::device::MechanismFailure::HostCallUnknown,
        }]
    ));
    assert_eq!(owner.slot().queue_outstanding(), Some(token));
}

/// [ID-1..3, COMMIT-5, CAP-1..4, MULTI] ReplyWithWrongFamily on SequenceQueue
/// yields Unknown(MalformedReply) and poisons owner.
#[test]
fn sequence_queue_wrong_family_reply_yields_unknown_and_poisons() {
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let mut owner = owner_for_tests();
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let clock = owner.clock_mut(key).unwrap();
    clock.install_reference(10);

    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::ReplyWithWrongFamily).unwrap();
    let c = yserver::kms::owner::sequence::SequenceConsumer(5002);
    let _token = owner
        .reserve_arm(
            key,
            yserver::kms::owner::sequence::SequencePurpose::PresentTargetWake,
            50,
            &[c],
        )
        .unwrap();
    owner.send_next_sequence_on(&mut executor).unwrap();

    test_support::wait_readable(executor.control_fd().unwrap(), Duration::from_secs(5));
    let reply_event = executor.poll_reply().unwrap();
    let events = owner.apply_host_call_event(reply_event);
    assert!(owner.is_poisoned());
    assert!(matches!(
        events.as_slice(),
        [OwnerEvent::MechanismFailed {
            reason: yserver::kms::owner::device::MechanismFailure::HostCallUnknown,
        }]
    ));
}

/// [ID-1..3, COMMIT-5, CAP-1..4, MULTI] Unresolved atomic excludes QUEUE, but
/// accepted atomic awaiting fences/evidence permits QUEUE (preserving absolute wakes).
#[test]
fn unresolved_atomic_excludes_queue_but_accepted_atomic_permits_queue() {
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let mut owner = owner_for_tests();
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let clock = owner.clock_mut(key).unwrap();
    clock.install_reference(10);

    let desc = single_active_crtc();
    let c = yserver::kms::owner::sequence::SequenceConsumer(5003);
    let token = owner
        .reserve_arm(
            key,
            yserver::kms::owner::sequence::SequencePurpose::PresentTargetWake,
            50,
            &[c],
        )
        .unwrap();

    // 1. Begin atomic: commit is now reserved on slot, but unresolved.
    let (commit, _) = owner.begin(&desc, ledger()).unwrap();

    // While atomic host call is unresolved, send_next_sequence_on cannot acquire queue lease:
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).unwrap();
    owner.send_next_sequence_on(&mut executor).unwrap();
    assert_eq!(
        owner.slot().queue_outstanding(),
        None,
        "unresolved atomic must exclude queue"
    );

    // 2. Mark atomic dispatched, then accept atomic reply:
    owner.mark_dispatched_for_tests();
    let dummy = std::fs::File::open("/dev/null").unwrap();
    let accept_outcome = HostCallOutcome::Accepted {
        out_fence_mask: 1,
        out_fences: vec![dummy.into()],
        helper_duration_ns: 100,
        round_trip_ns: 200,
    };
    let correlation = *owner.live_record().unwrap().correlation();
    let accept_event = HostCallEvent::Outcome {
        correlation,
        outcome: accept_outcome,
    };
    let events = owner.apply_host_call_event(accept_event);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Accepted { commit: c } if *c == commit))
    );

    // Now atomic is accepted and waiting for completion evidence (slot.occupant is still Some(commit)!)
    assert_eq!(owner.slot().occupant(), Some(commit));
    assert!(owner.slot().atomic_reply_resolved());

    // Send queue on a second executor stub:
    let mut queue_executor =
        test_support::spawn_stub_helper(StubBehaviour::AcceptQueueWith(50)).unwrap();
    owner.send_next_sequence_on(&mut queue_executor).unwrap();
    assert_eq!(
        owner.slot().queue_outstanding(),
        Some(token),
        "accepted atomic awaiting evidence must permit queue"
    );

    test_support::wait_readable(queue_executor.control_fd().unwrap(), Duration::from_secs(5));
    let reply = queue_executor.poll_reply().unwrap();
    let _reply_events = owner.apply_host_call_event(reply);
    assert_eq!(owner.slot().queue_outstanding(), None);
    assert_eq!(
        owner.sequence_arms().get(&token).unwrap().phase,
        yserver::kms::owner::sequence::SequenceArmPhase::Armed
    );
}

/// [ID-1..3, COMMIT-5, CAP-1..4, MULTI] Step 5a: Verify shutdown cancellation.
/// Populate two execution-stage consumers sharing a target, call actual
/// shutdown_drain_present_pending_exec with RecordingBackend, assert both ids
/// are cancelled before the store is emptied, then deliver the old arm event
/// and assert no clock/milestone change. Include an unresolved queue reply so
/// logical cancellation cannot release its host-call lease.
#[test]
fn shutdown_drain_cancels_shared_target_consumers_and_retains_lease_until_reply() {
    use std::time::Instant;
    use yserver::kms::owner::sequence::{SequenceConsumer, SequencePurpose};
    use yserver_core::{
        backend::RecordingBackend,
        core_loop::process_request::shutdown_drain_present_pending_exec,
        server::{PendingPresentEntry, PendingPresentPixmap, PendingPresentRequest, ServerState},
    };
    use yserver_protocol::x11::{ClientId, present::PixmapRequest};

    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let mut owner = owner_for_tests();
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    let clock = owner.clock_mut(key).unwrap();
    clock.install_reference(10);

    let c1 = SequenceConsumer(6001);
    let c2 = SequenceConsumer(6002);
    let target = 50;

    // Reserve arm with two consumers sharing target
    let token = owner
        .reserve_arm(key, SequencePurpose::PresentTargetWake, target, &[c1, c2])
        .expect("reserve shared arm");

    // Queue request is sent and remains in flight (unresolved)
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).unwrap();
    owner.send_next_sequence_on(&mut executor).unwrap();
    assert_eq!(owner.slot().queue_outstanding(), Some(token));

    // Core state has two execution-stage entries sharing target
    let mut state = ServerState::new();
    let mut backend = RecordingBackend::new();

    fn make_stub_entry(id: u64, target_msc: u64) -> PendingPresentEntry {
        PendingPresentEntry {
            pending: PendingPresentPixmap {
                origin: None,
                client_id: ClientId(1),
                request: PendingPresentRequest::Pixmap(PixmapRequest {
                    window: 0x10,
                    pixmap: 0x1,
                    serial: 1,
                    valid: 0,
                    update: 0,
                    x_off: 0,
                    y_off: 0,
                    target_crtc: 0,
                    wait_fence: 0,
                    idle_fence: 0,
                    options: 0,
                    target_msc,
                    divisor: 0,
                    remainder: 0,
                    notifies: Vec::new(),
                }),
                wake: yserver_core::backend::PresentWake::Pixmap { idle_fence_xid: 0 },
                masked_options: 0,
                src_host_xid: 0x2,
                paint_dst_host_xid: 0x10,
                completion_dst_host_xid: 0x10,
                src_width: 1,
                src_height: 1,
                update_rects: None,
                present_id: id,
                window_generation: 0,
                crtc_id: 1,
                crtc_epoch: 1,
                msc_offset: 0,
                effective_target_msc: Some(target_msc),
            },
            source_ready: true,
            wait_id: None,
            pin: None,
        }
    }

    state
        .present_pending_exec
        .insert(c1.0, make_stub_entry(c1.0, target));
    state
        .present_pending_exec
        .insert(c2.0, make_stub_entry(c2.0, target));

    // Drain shutdown
    shutdown_drain_present_pending_exec(&mut state, &mut backend);

    // Both IDs are cancelled before/during drain, and store is emptied
    assert!(state.present_pending_exec.is_empty());
    assert_eq!(
        backend.cancelled_present_sequence_consumers.len(),
        2,
        "both consumers cancelled by shutdown drain"
    );
    assert!(backend.cancelled_present_sequence_consumers.contains(&c1.0));
    assert!(backend.cancelled_present_sequence_consumers.contains(&c2.0));

    // Propagate cancellations to owner
    for &id in &backend.cancelled_present_sequence_consumers {
        owner.cancel_consumer(SequenceConsumer(id));
    }

    // Unresolved queue reply retains host-call lease despite logical cancellation
    assert_eq!(
        owner.slot().queue_outstanding(),
        Some(token),
        "unresolved reply retains lease"
    );

    // Old arm event delivered: assert no clock or milestone change
    let events = owner.apply_sequence_event(
        IncarnationId::first(),
        token.as_user_data(),
        1_000_000,
        target,
        Instant::now(),
    );
    assert!(events.is_empty(), "cancelled arm must produce no events");
    assert_eq!(
        owner.clock(key).unwrap().latest,
        None,
        "cancelled arm must not change clock"
    );
    assert_eq!(
        owner.slot().queue_outstanding(),
        Some(token),
        "lease still retained until reply"
    );
}
