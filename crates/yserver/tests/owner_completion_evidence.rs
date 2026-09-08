use std::time::{Duration, Instant};
use yserver::kms::{
    executor::{
        HostCallClass, HostCallEvent, HostCallOutcome, UnknownReason,
        protocol::{HostCallCorrelation, RequestSeq},
        test_support::{self, ScriptedReply, StubBehaviour},
    },
    owner::{
        build::CommitDescription,
        clock::{ClockKey, ClockSource, ProbeState},
        completion::{CompletionClass, CompletionContext, MechanismFailure},
        deadlines::{DeadlineError, fast_hardware, lifecycle_hardware, primary_event},
        device::{DeviceCommitOwner, DispatchError, OwnerEvent, ProbeOutcome},
        fences::{FencePollSet, FenceQuery, FenceStatus},
        identity::{ClockEpochId, CommitId, IncarnationId},
        ledger::Submitted,
        lifecycle::{ClockProbeId, LifecycleEpochId},
        qualification::CompletionQualification,
        record::{FailureCause, RefusalCause, TerminalState, UnknownCause},
        sequence::{SequenceConsumer, SequencePurpose},
        test_fixtures::*,
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

struct MockFenceQuery<F: FnMut(std::os::fd::BorrowedFd<'_>) -> std::io::Result<FenceStatus>> {
    func: F,
}

impl<F: FnMut(std::os::fd::BorrowedFd<'_>) -> std::io::Result<FenceStatus>> FenceQuery
    for MockFenceQuery<F>
{
    fn status(&mut self, fd: std::os::fd::BorrowedFd<'_>) -> std::io::Result<FenceStatus> {
        (self.func)(fd)
    }
}

struct DummyPollSet;

impl FencePollSet for DummyPollSet {
    fn register(&mut self, _fd: std::os::fd::BorrowedFd<'_>, _token: u64) -> std::io::Result<()> {
        Ok(())
    }
    fn unregister(&mut self, _fd: std::os::fd::BorrowedFd<'_>) -> std::io::Result<()> {
        Ok(())
    }
}

// =========================================================================
// Task 6 Step 1: Deadlines (COMMIT-2, COMMIT-5, CAP-1..4, MULTI)
// =========================================================================

/// [COMMIT-2, COMMIT-5, CAP-1..4] Pure arithmetic and bounds on deadline durations.
#[test]
fn hardware_and_event_windows_are_independent() {
    assert_eq!(fast_hardware([None]), Ok(Duration::from_millis(100)));
    assert_eq!(
        fast_hardware([Some(Duration::from_millis(400))]),
        Ok(Duration::from_millis(1200))
    );
    assert_eq!(
        fast_hardware([Some(Duration::from_secs(1))]),
        Ok(Duration::from_secs(2))
    );
    assert_eq!(primary_event(None), Ok(Duration::from_millis(50)));
    assert_eq!(
        primary_event(Some(Duration::from_secs(1))),
        Ok(Duration::from_millis(500))
    );
    assert_eq!(
        lifecycle_hardware(None),
        Err(DeadlineError::LifecycleUnvalidated)
    );
    assert_eq!(
        lifecycle_hardware(Some(Duration::from_secs(28))),
        Ok(Duration::from_secs(30))
    );
    assert_eq!(
        lifecycle_hardware(Some(Duration::from_secs(29))),
        Err(DeadlineError::LifecycleUnvalidated)
    );
}

/// [COMMIT-2, COMMIT-5, CAP-1..4, MULTI] No-sleep owner tests at deadline minus 1ns / exactly deadline.
/// Before acceptance no hardware timer; before HardwareComplete no Present timer.
/// After HardwareComplete create timers only for still-missing Present CRTCs, each from its own mode.
/// Arrival on that service turn precedes expiry. Producer waiting never reserves the slot or enters this timer table.
#[test]
fn no_sleep_owner_deadlines_exact_expiry_and_precedence() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let desc = single_active_crtc_with_present(1);
    let mut mode_periods = std::collections::BTreeMap::new();
    mode_periods.insert(1, Some(Duration::from_millis(40))); // 40ms period

    let mut clocks = std::collections::BTreeMap::new();
    clocks.insert(1, key);

    let context = CompletionContext {
        class: CompletionClass::FastUpdate,
        host_class: HostCallClass::SeatActiveNonblock,
        allow_modeset: false,
        clocks,
        mode_periods,
        lifecycle_observed_max: None,
    };

    let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
    // Before dispatch & acceptance: no deadline timer!
    assert_eq!(owner.completion_deadline(), None);

    owner.mark_dispatched_for_tests();
    // After dispatch but before acceptance: still no hardware timer!
    assert_eq!(owner.completion_deadline(), None);

    let t0 = Instant::now();
    // fast_hardware for 40ms: 40 * 3 = 120ms (clamped 100ms..2s) -> 120ms
    let hw_expected_dur = Duration::from_millis(120);
    let hw_expected_deadline = t0 + hw_expected_dur;

    owner.apply_host_call_event_at(accepted(commit, 0b1, 1), t0);
    assert_eq!(owner.completion_deadline(), Some(hw_expected_deadline));

    // Deadline minus 1ns: tick_completion returns nothing
    let pre_expiry = hw_expected_deadline - Duration::from_nanos(1);
    assert!(owner.tick_completion(pre_expiry).is_empty());
    assert!(!owner.is_poisoned());

    // Fences arrive at t1 before deadline
    let t1 = t0 + Duration::from_millis(50);
    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    let hw_events = owner.observe_fences(&mut query, &mut poll_set, t1);
    assert!(
        hw_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::HardwareComplete { .. }))
    );

    // HardwareComplete is reached! Now Present deadline begins for still-missing Present CRTC 1.
    // primary_event for 40ms: 40 * 2 = 80ms (clamped 50ms..500ms) -> 80ms
    let present_expected_dur = Duration::from_millis(80);
    let present_expected_deadline = t1 + present_expected_dur;
    assert_eq!(owner.completion_deadline(), Some(present_expected_deadline));

    // At present deadline minus 1ns: tick_completion returns nothing
    assert!(
        owner
            .tick_completion(present_expected_deadline - Duration::from_nanos(1))
            .is_empty()
    );
    assert!(!owner.is_poisoned());

    // Arrival on that service turn precedes expiry!
    // If PageFlip arrives at exactly present_expected_deadline, applied first:
    let ev = event_for_current_record(&owner, 1, 101, 10, 0);
    let page_events = owner.apply_drm_event(IncarnationId::first(), ev, present_expected_deadline);
    assert!(
        page_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Presented { .. }))
    );
    assert!(page_events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::Completed,
            ..
        }
    )));

    // After completion, timer is gone and tick_completion does nothing
    assert_eq!(owner.completion_deadline(), None);
    assert!(owner.tick_completion(present_expected_deadline).is_empty());
    assert!(!owner.is_poisoned());
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Exact expiry test: tick_completion at exactly deadline triggers timeout.
#[test]
fn owner_deadline_exact_expiry_triggers_hardware_timeout() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let desc = single_active_crtc();
    let (commit, _) = owner.begin(&desc, ledger()).unwrap();
    owner.mark_dispatched_for_tests();

    let t0 = Instant::now();
    owner.apply_host_call_event_at(accepted(commit, 0b1, 1), t0);
    let dl = owner.completion_deadline().expect("hardware deadline");

    // Exactly at deadline: triggers HardwareTimeout
    let events = owner.tick_completion(dl);
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::MechanismFailed {
            reason: MechanismFailure::HardwareTimeout
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::CompletionUnknown(UnknownCause::Mechanism(
                MechanismFailure::HardwareTimeout
            )),
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Quarantined { .. }))
    );
    assert!(owner.is_poisoned());
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Exact expiry test: tick_completion at exactly present deadline triggers timeout.
#[test]
fn owner_deadline_exact_expiry_triggers_present_timeout() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let desc = single_active_crtc_with_present(1);
    let mut mode_periods = std::collections::BTreeMap::new();
    mode_periods.insert(1, None);

    let mut clocks = std::collections::BTreeMap::new();
    clocks.insert(1, key);

    let context = CompletionContext {
        class: CompletionClass::FastUpdate,
        host_class: HostCallClass::SeatActiveNonblock,
        allow_modeset: false,
        clocks,
        mode_periods,
        lifecycle_observed_max: None,
    };

    let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
    owner.mark_dispatched_for_tests();

    let t0 = Instant::now();
    owner.apply_host_call_event_at(accepted(commit, 0b1, 1), t0);

    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    owner.observe_fences(&mut query, &mut poll_set, t0);

    let present_dl = owner.completion_deadline().expect("present deadline");
    let events = owner.tick_completion(present_dl);
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::MechanismFailed {
            reason: MechanismFailure::PresentTimeout
        }
    )));
    assert!(owner.is_poisoned());
}

// =========================================================================
// Task 6 Step 2: Completion & Qualification (COMMIT-2, COMMIT-5, CAP-1..4, MULTI)
// =========================================================================

/// [COMMIT-2, COMMIT-5, CAP-1..4] Ordinary fast commit with all evidence reaches Completed
/// but leaves qualification false (Unqualified).
#[test]
fn ordinary_fast_commit_completes_but_leaves_qualification_unqualified() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let desc = single_active_crtc_with_present(1);
    let mut mode_periods = std::collections::BTreeMap::new();
    mode_periods.insert(1, None);
    let mut clocks = std::collections::BTreeMap::new();
    clocks.insert(1, key);

    let context = CompletionContext {
        class: CompletionClass::FastUpdate,
        host_class: HostCallClass::SeatActiveNonblock,
        allow_modeset: false,
        clocks,
        mode_periods,
        lifecycle_observed_max: None,
    };

    let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
    owner.mark_dispatched_for_tests();

    let now = Instant::now();
    owner.apply_host_call_event_at(accepted(commit, 0b1, 1), now);

    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    owner.observe_fences(&mut query, &mut poll_set, now);

    let ev = event_for_current_record(&owner, 1, 101, 1, 0);
    let events = owner.apply_drm_event(IncarnationId::first(), ev, now);

    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::Completed,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OwnerEvent::CompletionRetired { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OwnerEvent::CompletionQualificationChanged { .. }))
    );
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
    assert_eq!(owner.slot().occupant(), None);
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Exact install candidate with nonempty expected set and full evidence qualifies.
#[test]
fn install_candidate_with_nonempty_expected_set_qualifies() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let mut crtcs = std::collections::BTreeSet::new();
    crtcs.insert(1);
    let caps = completion_caps_for_tests(IncarnationId::first(), 1, true, true, true, crtcs);
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = single_active_crtc_with_present(1);
    let context = lifecycle_context_for_crtcs(&[(1, key)], Some(Duration::from_secs(10)));

    let (commit, _) = owner
        .begin_install_restore(&desc, ledger(), context)
        .unwrap();
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Awaiting {
            topology_generation: 1,
            commit
        }
    );
    owner.mark_dispatched_for_tests();

    let now = Instant::now();
    owner.apply_host_call_event_at(accepted(commit, 0b1, 1), now);

    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    owner.observe_fences(&mut query, &mut poll_set, now);

    let ev = event_for_current_record(&owner, 1, 101, 1, 0);
    let events = owner.apply_drm_event(IncarnationId::first(), ev, now);

    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::CompletionQualificationChanged { qualified: true }
    )));
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Qualified {
            topology_generation: 1,
            commit
        }
    );
    assert_eq!(owner.slot().occupant(), None);
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Empty expected set cannot qualify.
#[test]
fn empty_expected_set_install_cannot_qualify() {
    let mut owner = owner_for_tests();
    let caps = completion_caps_for_tests(
        IncarnationId::first(),
        1,
        true,
        true,
        true,
        std::collections::BTreeSet::new(),
    );
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = CommitDescription {
        objects: Vec::new(),
        crtc_state: Vec::new(),
        present_consumers: Vec::new(),
        page_flip_event: false,
        property_ids: TEST_PROPERTY_IDS,
    };
    let context = lifecycle_context_for_crtcs(&[], Some(Duration::from_secs(10)));

    let (commit, _) = owner
        .begin_install_restore(&desc, ledger(), context)
        .unwrap();
    // Empty expected set leaves Unqualified!
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
    owner.mark_dispatched_for_tests();

    let now = Instant::now();
    owner.apply_host_call_event_at(accepted(commit, 0, 0), now);

    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    let events = owner.observe_fences(&mut query, &mut poll_set, now);

    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::Completed,
            ..
        }
    )));
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Successful ioctl with pending fences cannot qualify.
#[test]
fn pending_fences_cannot_qualify() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let mut crtcs = std::collections::BTreeSet::new();
    crtcs.insert(1);
    let caps = completion_caps_for_tests(IncarnationId::first(), 1, true, true, true, crtcs);
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = single_active_crtc();
    let context = lifecycle_context_for_crtcs(&[(1, key)], Some(Duration::from_secs(10)));

    let (commit, _) = owner
        .begin_install_restore(&desc, ledger(), context)
        .unwrap();
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Awaiting {
            topology_generation: 1,
            commit
        }
    );
    owner.mark_dispatched_for_tests();

    let now = Instant::now();
    owner.apply_host_call_event_at(accepted(commit, 0b1, 1), now);

    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Pending),
    };
    let mut poll_set = DummyPollSet;
    let events = owner.observe_fences(&mut query, &mut poll_set, now);

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OwnerEvent::HardwareComplete { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OwnerEvent::CompletionQualificationChanged { .. }))
    );
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Awaiting {
            topology_generation: 1,
            commit
        }
    );
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Failed/missing caps prevent qualification dispatch.
#[test]
fn failed_or_missing_caps_prevent_qualification_dispatch() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let desc = single_active_crtc();
    let context = lifecycle_context_for_crtcs(&[(1, key)], Some(Duration::from_secs(10)));

    // 1. Missing caps
    let err = owner
        .begin_install_restore(&desc, ledger(), context.clone())
        .unwrap_err();
    assert!(matches!(err, DispatchError::InvalidCompletionCaps));

    // 2. Structurally incapable caps (atomic_enabled: false)
    let caps_incapable = completion_caps_for_tests(
        IncarnationId::first(),
        1,
        false, // not atomic
        true,
        true,
        [1].into_iter().collect(),
    );
    install_test_completion_caps(&mut owner, caps_incapable).unwrap();
    let err = owner
        .begin_install_restore(&desc, ledger(), context)
        .unwrap_err();
    assert!(matches!(err, DispatchError::InvalidCompletionCaps));

    // Unqualified remains, not poisoned
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
    assert!(!owner.is_poisoned());
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Missing or above-28s lifecycle measurements refuse before dispatch with no poison.
#[test]
fn missing_or_excessive_lifecycle_timing_prevents_qualification_dispatch() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let caps = completion_caps_for_tests(
        IncarnationId::first(),
        1,
        true,
        true,
        true,
        [1].into_iter().collect(),
    );
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = single_active_crtc();

    // 1. None
    let ctx_none = lifecycle_context_for_crtcs(&[(1, key)], None);
    let err = owner
        .begin_install_restore(&desc, ledger(), ctx_none)
        .unwrap_err();
    assert!(matches!(err, DispatchError::LifecycleUnvalidated));

    // 2. 29 seconds (> 28s)
    let ctx_29s = lifecycle_context_for_crtcs(&[(1, key)], Some(Duration::from_secs(29)));
    let err = owner
        .begin_install_restore(&desc, ledger(), ctx_29s)
        .unwrap_err();
    assert!(matches!(err, DispatchError::LifecycleUnvalidated));

    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
    assert!(!owner.is_poisoned());
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Topology invalidation closes a formerly qualified gate and cancels clocks/arms.
#[test]
fn topology_invalidation_closes_qualified_gate_and_cancels_clocks_and_arms() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let caps = completion_caps_for_tests(
        IncarnationId::first(),
        1,
        true,
        true,
        true,
        [1].into_iter().collect(),
    );
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = single_active_crtc();
    let context = lifecycle_context_for_crtcs(&[(1, key)], Some(Duration::from_secs(10)));
    let (commit, _) = owner
        .begin_install_restore(&desc, ledger(), context)
        .unwrap();
    owner.mark_dispatched_for_tests();

    let now = Instant::now();
    owner.apply_host_call_event_at(accepted(commit, 0b1, 1), now);
    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    owner.observe_fences(&mut query, &mut poll_set, now);

    assert_eq!(
        owner.qualification(),
        CompletionQualification::Qualified {
            topology_generation: 1,
            commit
        }
    );

    // Invalidate topology to generation 2
    let events = owner.invalidate_topology(2);
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::CompletionQualificationChanged { qualified: false }
    )));
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 2
        }
    );
    assert!(owner.clock(key).is_none());
}

/// [COMMIT-2, COMMIT-5, CAP-1..4, MULTI] Error fence after one success yields no CompletionRetired.
#[test]
fn error_fence_after_one_success_yields_no_completion_retired() {
    let mut owner = owner_for_tests();
    let k1 = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let k2 = ClockKey {
        hardware_crtc: 2,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(k1, LifecycleEpochId::first(), 1)
        .unwrap();
    owner
        .install_clock(k2, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(k1).unwrap().install_reference(100);
    owner.clock_mut(k2).unwrap().install_reference(100);

    let caps = completion_caps_for_tests(
        IncarnationId::first(),
        1,
        true,
        true,
        true,
        [1, 2].into_iter().collect(),
    );
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = two_active_crtcs();
    let context = lifecycle_context_for_crtcs(&[(1, k1), (2, k2)], Some(Duration::from_secs(10)));
    let (commit, _) = owner
        .begin_install_restore(&desc, ledger(), context)
        .unwrap();
    owner.mark_dispatched_for_tests();

    let now = Instant::now();
    owner.apply_host_call_event_at(accepted(commit, 0b11, 2), now);

    // CRTC 1 fence succeeds, CRTC 2 fence fails with Error(-EIO)
    let mut call_count = 0;
    let mut query = MockFenceQuery {
        func: move |_| {
            call_count += 1;
            if call_count == 1 {
                Ok(FenceStatus::Success)
            } else {
                Ok(FenceStatus::Error(-5))
            }
        },
    };
    let mut poll_set = DummyPollSet;
    let events = owner.observe_fences(&mut query, &mut poll_set, now);

    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::MechanismFailed {
            reason: MechanismFailure::FenceError
        }
    )));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OwnerEvent::CompletionRetired { .. }))
    );
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
    assert_eq!(owner.slot().occupant(), Some(commit)); // Quarantined slot retained!
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Completing a tracked-resource commit frees the slot
/// but destroys neither old nor new resource; drop counts stay zero until
/// the receiving test deliberately drops CompletionRetired resources.
#[test]
fn tracked_resource_commit_frees_slot_without_destroying_resources_until_dropped() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let drop_counter = Arc::new(AtomicUsize::new(0));

    #[derive(Debug)]
    struct TrackedResource {
        id: u32,
        drops: Arc<AtomicUsize>,
    }
    impl Drop for TrackedResource {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    let mut owner: DeviceCommitOwner<TrackedResource> =
        DeviceCommitOwner::new(IncarnationId::first(), LifecycleEpochId::first(), 1);
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let old_res = TrackedResource {
        id: 1,
        drops: drop_counter.clone(),
    };
    let new_res = TrackedResource {
        id: 2,
        drops: drop_counter.clone(),
    };
    let tracked_ledger = Submitted::new(vec![old_res], vec![new_res]);

    let desc = single_active_crtc();
    let (commit, _) = owner.begin(&desc, tracked_ledger).unwrap();
    owner.mark_dispatched_for_tests();

    let now = Instant::now();
    owner.apply_host_call_event_at(accepted(commit, 0b1, 1), now);

    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    let events = owner.observe_fences(&mut query, &mut poll_set, now);

    // Slot is freed!
    assert_eq!(owner.slot().occupant(), None);

    // Drop count MUST still be 0! Neither old nor new was dropped.
    assert_eq!(drop_counter.load(Ordering::SeqCst), 0);

    // Extract CompletionRetired event
    let mut retired_resources = None;
    for event in events {
        if let OwnerEvent::CompletionRetired { resources, .. } = event {
            retired_resources = Some(resources);
        }
    }
    let retired = retired_resources.expect("CompletionRetired event");

    // Still 0 drops while held by receiver!
    assert_eq!(drop_counter.load(Ordering::SeqCst), 0);
    assert_eq!(retired.old()[0].id, 1);
    assert_eq!(retired.new()[0].id, 2);

    // Deliberately drop resources
    drop(retired);

    // Now both old and new are dropped!
    assert_eq!(drop_counter.load(Ordering::SeqCst), 2);
}

// =========================================================================
// Task 6 Step 3a: Pre-IPC candidate retirement (COMMIT-2, COMMIT-5, CAP-1..4)
// =========================================================================

/// [COMMIT-2, COMMIT-5, CAP-1..4] Begin a valid install candidate, force a proven
/// BoundaryViolation refusal before IPC, and assert matching Awaiting becomes
/// Unqualified with the slot released and no new poison.
/// Correct the executor boundary and explicitly begin a fresh valid candidate; full evidence qualifies it.
#[test]
fn pre_ipc_boundary_violation_resets_awaiting_candidate_to_unqualified_without_poison() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let caps = completion_caps_for_tests(
        IncarnationId::first(),
        1,
        true,
        true,
        true,
        [1].into_iter().collect(),
    );
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = single_active_crtc();
    let mut clocks = std::collections::BTreeMap::new();
    clocks.insert(1, key);
    let mut mode_periods = std::collections::BTreeMap::new();
    mode_periods.insert(1, None);

    // ColdStartOrOfflineBlocking will violate the boundary on SeatActive executor!
    let context_blocking = CompletionContext {
        class: CompletionClass::LifecycleInstallRestore,
        host_class: HostCallClass::ColdStartOrOfflineBlocking,
        allow_modeset: false,
        clocks: clocks.clone(),
        mode_periods: mode_periods.clone(),
        lifecycle_observed_max: Some(Duration::from_secs(10)),
    };

    let (commit1, _) = owner
        .begin_install_restore(&desc, ledger(), context_blocking)
        .unwrap();

    assert_eq!(
        owner.qualification(),
        CompletionQualification::Awaiting {
            topology_generation: 1,
            commit: commit1
        }
    );

    // Spawn executor (defaults to SeatActive phase)
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::Scripted(ScriptedReply::Accepted {
            mask: 0b1,
            fds: 1,
        }))
        .unwrap();
    executor.enter_seat_active();

    let send_res = owner.send_on(&mut executor);
    assert!(matches!(
        send_res,
        Err(DispatchError::Refused {
            cause: RefusalCause::BoundaryViolation,
            ..
        })
    ));

    // Awaiting becomes Unqualified, slot is released, no poison!
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
    assert_eq!(owner.slot().occupant(), None);
    assert!(!owner.is_poisoned());

    // Explicitly begin a fresh valid candidate with SeatActiveNonblock!
    let context_nonblock = CompletionContext {
        class: CompletionClass::LifecycleInstallRestore,
        host_class: HostCallClass::SeatActiveNonblock,
        allow_modeset: false,
        clocks,
        mode_periods,
        lifecycle_observed_max: Some(Duration::from_secs(10)),
    };

    let (commit2, _) = owner
        .begin_install_restore(&desc, ledger(), context_nonblock)
        .unwrap();
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Awaiting {
            topology_generation: 1,
            commit: commit2
        }
    );

    // Complete commit2 with the clean executor
    owner.send_on(&mut executor).unwrap();
    test_support::wait_readable(executor.control_fd().unwrap(), Duration::from_secs(5));
    owner.apply_host_call_event(executor.poll_reply().unwrap());

    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    let events = owner.observe_fences(&mut query, &mut poll_set, Instant::now());

    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::CompletionQualificationChanged { qualified: true }
    )));
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Qualified {
            topology_generation: 1,
            commit: commit2
        }
    );
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Pre-dispatch cancellation resets Awaiting candidate to Unqualified.
#[test]
fn pre_dispatch_cancellation_resets_awaiting_candidate_to_unqualified() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let caps = completion_caps_for_tests(
        IncarnationId::first(),
        1,
        true,
        true,
        true,
        [1].into_iter().collect(),
    );
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = single_active_crtc();
    let context = lifecycle_context_for_crtcs(&[(1, key)], Some(Duration::from_secs(10)));
    let (commit, _) = owner
        .begin_install_restore(&desc, ledger(), context)
        .unwrap();

    assert_eq!(
        owner.qualification(),
        CompletionQualification::Awaiting {
            topology_generation: 1,
            commit
        }
    );

    let events = owner.cancel_live(commit).unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(_)),
            ..
        }
    )));
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
    assert_eq!(owner.slot().occupant(), None);
    assert!(!owner.is_poisoned());
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Stale commit or generation retirement does not clear a new candidate.
#[test]
fn stale_commit_or_generation_retirement_does_not_clear_new_candidate() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let caps = completion_caps_for_tests(
        IncarnationId::first(),
        1,
        true,
        true,
        true,
        [1].into_iter().collect(),
    );
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = single_active_crtc();
    let context = lifecycle_context_for_crtcs(&[(1, key)], Some(Duration::from_secs(10)));
    let (commit, _) = owner
        .begin_install_restore(&desc, ledger(), context)
        .unwrap();

    assert_eq!(
        owner.qualification(),
        CompletionQualification::Awaiting {
            topology_generation: 1,
            commit
        }
    );

    // Mismatched commit ID fails cancel_live
    assert!(owner.cancel_live(CommitId::for_tests(999)).is_err());
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Awaiting {
            topology_generation: 1,
            commit
        }
    );

    // Stale reply does not touch live record or candidate
    let stale_event = rejected(CommitId::for_tests(999), libc::EINVAL);
    let events = owner.apply_host_call_event(stale_event);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OwnerEvent::StaleReply { .. }))
    );
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Awaiting {
            topology_generation: 1,
            commit
        }
    );
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Attempted write (SendError::Ipc) follows unknown reconciliation and cannot authorize fresh candidate on poisoned incarnation.
#[test]
fn attempted_write_ipc_outcome_follows_unknown_reconciliation_and_poisons() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let caps = completion_caps_for_tests(
        IncarnationId::first(),
        1,
        true,
        true,
        true,
        [1].into_iter().collect(),
    );
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = single_active_crtc();
    let context = lifecycle_context_for_crtcs(&[(1, key)], Some(Duration::from_secs(10)));
    let (commit, _) = owner
        .begin_install_restore(&desc, ledger(), context.clone())
        .unwrap();
    owner.mark_dispatched_for_tests();

    // Reconcile as Unknown (e.g. from Ipc outcome)
    let events = owner.apply_host_call_event(yserver::kms::owner::test_fixtures::unknown(
        commit,
        UnknownReason::IpcFailure,
    ));

    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::CompletionUnknown(UnknownCause::HostCall(
                UnknownReason::IpcFailure
            )),
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Quarantined { .. }))
    );

    // Qualification was reset, but slot is held in quarantine!
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
    assert_eq!(owner.slot().occupant(), Some(commit));

    // Fresh candidate cannot be admitted because slot is held!
    let err = owner
        .begin_install_restore(&desc, ledger(), context)
        .unwrap_err();
    assert!(matches!(err, DispatchError::Slot(_)));
}

// =========================================================================
// Task 6 Step 3b: Deadline construction failures without sleeps (COMMIT-2, COMMIT-5, CAP-1..4)
// =========================================================================

/// [COMMIT-2, COMMIT-5, CAP-1..4] Inject hardware deadline construction overflow at acceptance:
/// produces DeadlineOverflow failure/unknown terminal, retains slot/ledger, closes qualification,
/// and emits no CompletionRetired; later evidence cannot complete it.
#[test]
fn deadline_construction_overflow_at_acceptance_fails_closed_and_quarantines() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let caps = completion_caps_for_tests(
        IncarnationId::first(),
        1,
        true,
        true,
        true,
        [1].into_iter().collect(),
    );
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = single_active_crtc();
    let context = lifecycle_context_for_crtcs(&[(1, key)], Some(Duration::from_secs(10)));
    let (commit, _) = owner
        .begin_install_restore(&desc, ledger(), context)
        .unwrap();
    owner.mark_dispatched_for_tests();

    // Inject hardware deadline overflow!
    owner.inject_hardware_deadline_overflow_for_tests();

    let events = owner.apply_host_call_event_at(accepted(commit, 0b1, 1), Instant::now());
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::MechanismFailed {
            reason: MechanismFailure::DeadlineOverflow
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::CompletionUnknown(UnknownCause::Mechanism(
                MechanismFailure::DeadlineOverflow
            )),
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Quarantined { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OwnerEvent::CompletionRetired { .. }))
    );

    // Retains slot and closes qualification
    assert_eq!(owner.slot().occupant(), Some(commit));
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
    assert!(owner.is_poisoned());

    // Later evidence cannot complete it
    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    let fence_events = owner.observe_fences(&mut query, &mut poll_set, Instant::now());
    assert!(fence_events.is_empty());
    assert_eq!(owner.slot().occupant(), Some(commit));
}

/// [COMMIT-2, COMMIT-5, CAP-1..4, MULTI] Inject checked-add failure on one entry in a two-CRTC missing-Present map
/// at HardwareComplete: transactional failure produces DeadlineOverflow, retains slot/ledger, closes qualification.
#[test]
fn deadline_construction_overflow_at_hardware_complete_fails_closed_and_quarantines() {
    let mut owner = owner_for_tests();
    let k1 = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let k2 = ClockKey {
        hardware_crtc: 2,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(k1, LifecycleEpochId::first(), 1)
        .unwrap();
    owner
        .install_clock(k2, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(k1).unwrap().install_reference(100);
    owner.clock_mut(k2).unwrap().install_reference(100);

    let caps = completion_caps_for_tests(
        IncarnationId::first(),
        1,
        true,
        true,
        true,
        [1, 2].into_iter().collect(),
    );
    install_test_completion_caps(&mut owner, caps).unwrap();

    let desc = two_active_crtcs_with_present(1, 2);
    let context = lifecycle_context_for_crtcs(&[(1, k1), (2, k2)], Some(Duration::from_secs(10)));
    let (commit, _) = owner
        .begin_install_restore(&desc, ledger(), context)
        .unwrap();
    owner.mark_dispatched_for_tests();

    owner.apply_host_call_event_at(accepted(commit, 0b11, 2), Instant::now());

    // Inject Present deadline overflow for CRTC 2!
    owner.inject_present_deadline_overflow_for_tests(2);

    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    let events = owner.observe_fences(&mut query, &mut poll_set, Instant::now());

    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::MechanismFailed {
            reason: MechanismFailure::DeadlineOverflow
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::CompletionUnknown(UnknownCause::Mechanism(
                MechanismFailure::DeadlineOverflow
            )),
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Quarantined { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OwnerEvent::CompletionRetired { .. }))
    );

    assert_eq!(owner.slot().occupant(), Some(commit));
    assert_eq!(
        owner.qualification(),
        CompletionQualification::Unqualified {
            topology_generation: 1
        }
    );
    assert!(owner.is_poisoned());
}

// =========================================================================
// Task 6 Step 4: Full ordering matrix (COMMIT-2, COMMIT-5, CAP-1..4, MULTI)
// =========================================================================

/// [COMMIT-2, COMMIT-5, CAP-1..4] Matrix permutation 1: Reply -> Fence -> Page -> Completed.
#[test]
fn matrix_order_reply_then_fence_then_page() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let desc = single_active_crtc_with_present(1);
    let mut mode_periods = std::collections::BTreeMap::new();
    mode_periods.insert(1, None);
    let mut clocks = std::collections::BTreeMap::new();
    clocks.insert(1, key);
    let context = CompletionContext {
        class: CompletionClass::FastUpdate,
        host_class: HostCallClass::SeatActiveNonblock,
        allow_modeset: false,
        clocks,
        mode_periods,
        lifecycle_observed_max: None,
    };
    let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
    owner.mark_dispatched_for_tests();

    let now = Instant::now();
    // 1. Reply
    let reply_events = owner.apply_host_call_event_at(accepted(commit, 0b1, 1), now);
    assert_eq!(reply_events.len(), 1);
    assert!(matches!(reply_events[0], OwnerEvent::Accepted { .. }));

    // 2. Fence
    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    let fence_events = owner.observe_fences(&mut query, &mut poll_set, now);
    assert_eq!(fence_events.len(), 1);
    assert!(matches!(
        fence_events[0],
        OwnerEvent::HardwareComplete { .. }
    ));

    // 3. Page
    let ev = event_for_current_record(&owner, 1, 101, 1, 0);
    let page_events = owner.apply_drm_event(IncarnationId::first(), ev, now);
    assert!(
        page_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Presented { .. }))
    );
    assert!(page_events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::Completed,
            ..
        }
    )));
    assert!(
        page_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::CompletionRetired { .. }))
    );
    assert_eq!(owner.slot().occupant(), None);
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Matrix permutation 2: Reply -> Page -> Fence -> Completed.
#[test]
fn matrix_order_reply_then_page_then_fence() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let desc = single_active_crtc_with_present(1);
    let mut mode_periods = std::collections::BTreeMap::new();
    mode_periods.insert(1, None);
    let mut clocks = std::collections::BTreeMap::new();
    clocks.insert(1, key);
    let context = CompletionContext {
        class: CompletionClass::FastUpdate,
        host_class: HostCallClass::SeatActiveNonblock,
        allow_modeset: false,
        clocks,
        mode_periods,
        lifecycle_observed_max: None,
    };
    let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
    owner.mark_dispatched_for_tests();

    let now = Instant::now();
    // 1. Reply
    let reply_events = owner.apply_host_call_event_at(accepted(commit, 0b1, 1), now);
    assert_eq!(reply_events.len(), 1);
    assert!(matches!(reply_events[0], OwnerEvent::Accepted { .. }));

    // 2. Page
    let ev = event_for_current_record(&owner, 1, 101, 1, 0);
    let page_events = owner.apply_drm_event(IncarnationId::first(), ev, now);
    assert!(
        page_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Presented { .. }))
    );
    assert!(
        !page_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Terminal { .. }))
    );

    // 3. Fence
    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    let fence_events = owner.observe_fences(&mut query, &mut poll_set, now);
    assert!(
        fence_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::HardwareComplete { .. }))
    );
    assert!(fence_events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::Completed,
            ..
        }
    )));
    assert!(
        fence_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::CompletionRetired { .. }))
    );
    assert_eq!(owner.slot().occupant(), None);
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Matrix permutation 3: Page (Staged) -> Reply -> Fence -> Completed.
#[test]
fn matrix_order_page_then_reply_then_fence() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let desc = single_active_crtc_with_present(1);
    let mut mode_periods = std::collections::BTreeMap::new();
    mode_periods.insert(1, None);
    let mut clocks = std::collections::BTreeMap::new();
    clocks.insert(1, key);
    let context = CompletionContext {
        class: CompletionClass::FastUpdate,
        host_class: HostCallClass::SeatActiveNonblock,
        allow_modeset: false,
        clocks,
        mode_periods,
        lifecycle_observed_max: None,
    };
    let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
    owner.mark_dispatched_for_tests();

    let now = Instant::now();
    // 1. Page (Staged: emits no ClockSample or Presented)
    let ev = event_for_current_record(&owner, 1, 101, 1, 0);
    let pre_events = owner.apply_drm_event(IncarnationId::first(), ev, now);
    assert!(pre_events.is_empty());

    // 2. Reply (Emits Accepted, ClockSample, Presented)
    let reply_events = owner.apply_host_call_event_at(accepted(commit, 0b1, 1), now);
    assert!(
        reply_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Accepted { .. }))
    );
    assert!(
        reply_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::ClockSample { .. }))
    );
    assert!(
        reply_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Presented { .. }))
    );
    assert!(
        !reply_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Terminal { .. }))
    );

    // 3. Fence
    let mut query = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let mut poll_set = DummyPollSet;
    let fence_events = owner.observe_fences(&mut query, &mut poll_set, now);
    assert!(
        fence_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::HardwareComplete { .. }))
    );
    assert!(fence_events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::Completed,
            ..
        }
    )));
    assert!(
        fence_events
            .iter()
            .any(|e| matches!(e, OwnerEvent::CompletionRetired { .. }))
    );
    assert_eq!(owner.slot().occupant(), None);
}

/// [COMMIT-2, COMMIT-5, CAP-1..4, MULTI] Matrix two-CRTC partial fences: CRTC 1 Success, CRTC 2 Pending -> not complete; CRTC 2 Success -> HardwareComplete & Completed.
#[test]
fn matrix_two_crtc_partial_fences_to_completion() {
    let mut owner = owner_for_tests();
    let k1 = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    let k2 = ClockKey {
        hardware_crtc: 2,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(k1, LifecycleEpochId::first(), 1)
        .unwrap();
    owner
        .install_clock(k2, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(k1).unwrap().install_reference(100);
    owner.clock_mut(k2).unwrap().install_reference(100);

    let desc = two_active_crtcs();
    let mut mode_periods = std::collections::BTreeMap::new();
    mode_periods.insert(1, None);
    mode_periods.insert(2, None);
    let context = CompletionContext {
        class: CompletionClass::FastUpdate,
        host_class: HostCallClass::SeatActiveNonblock,
        allow_modeset: false,
        clocks: std::collections::BTreeMap::new(),
        mode_periods,
        lifecycle_observed_max: None,
    };
    let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
    owner.mark_dispatched_for_tests();

    let now = Instant::now();
    owner.apply_host_call_event_at(accepted(commit, 0b11, 2), now);

    // Turn 1: Slot 0 (CRTC 1) Success, Slot 1 (CRTC 2) Pending
    let mut call_count = 0;
    let mut query = MockFenceQuery {
        func: move |_| {
            call_count += 1;
            if call_count == 1 {
                Ok(FenceStatus::Success)
            } else {
                Ok(FenceStatus::Pending)
            }
        },
    };
    let mut poll_set = DummyPollSet;
    let events1 = owner.observe_fences(&mut query, &mut poll_set, now);
    assert!(events1.is_empty());
    assert_eq!(owner.slot().occupant(), Some(commit));

    // Turn 2: Slot 1 (CRTC 2) becomes Success
    let mut query2 = MockFenceQuery {
        func: |_| Ok(FenceStatus::Success),
    };
    let events2 = owner.observe_fences(&mut query2, &mut poll_set, now);
    assert!(
        events2
            .iter()
            .any(|e| matches!(e, OwnerEvent::HardwareComplete { .. }))
    );
    assert!(events2.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::Completed,
            ..
        }
    )));
    assert!(
        events2
            .iter()
            .any(|e| matches!(e, OwnerEvent::CompletionRetired { .. }))
    );
    assert_eq!(owner.slot().occupant(), None);
}

/// [COMMIT-2, COMMIT-5, CAP-1..4] Sequence-only events never satisfy completion milestones.
#[test]
fn sequence_only_events_never_satisfy_completion_milestones() {
    let mut owner = owner_for_tests();
    let key = ClockKey {
        hardware_crtc: 1,
        epoch: ClockEpochId::first(),
    };
    owner
        .install_clock(key, LifecycleEpochId::first(), 1)
        .unwrap();
    owner.clock_mut(key).unwrap().install_reference(100);

    let token = owner
        .reserve_arm(
            key,
            SequencePurpose::IdleClockWake,
            1,
            &[SequenceConsumer(1)],
        )
        .unwrap();
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::AcceptQueueWith(0)).unwrap();
    owner.send_next_sequence_on(&mut executor).unwrap();

    let events = owner.apply_sequence_event(
        IncarnationId::first(),
        token.as_user_data(),
        1_000_000,
        10,
        Instant::now(),
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OwnerEvent::HardwareComplete { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Presented { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OwnerEvent::Terminal { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OwnerEvent::CompletionRetired { .. }))
    );
}
