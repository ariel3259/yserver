//! Integration tests for process-isolated KMS executor.

use std::{
    io::Read,
    time::{Duration, Instant},
};

use yserver::kms::executor::{
    BoundaryViolation, ClockProbeLease, ExecutorState, HostCallClass, HostCallEvent,
    HostCallOutcome, HostCallPhase, HostCallReservation, KmsIoExecutor, SendError, SubmittingProof,
    UnknownReason, ValidationLease,
    test_support::{self, ScriptedReply, StubBehaviour, *},
};

#[test]
fn the_real_helper_reaches_the_raw_ioctl_at_all() {
    let device = TestDevice::open_never_a_drm_device(); // /dev/null
    let mut executor = spawn_real_helper_for_tests(&device);
    let outcome = dispatch_and_wait_for_tests(&mut executor, &three_property_request_for_tests());
    match outcome {
        HostCallOutcome::Rejected { errno, .. } => assert_eq!(
            errno,
            libc::ENOTTY,
            "a non-DRM descriptor must fail in ioctl dispatch, proving the call was made"
        ),
        other => panic!("expected an explicit rejection from the real ioctl, got {other:?}"),
    }
}

#[test]
#[ignore = "requires a real DRM device with master; run explicitly"]
fn the_helper_reports_a_kernel_rejection_of_an_invalid_object_on_real_hardware() {
    let Some(device) = TestDevice::open_real_drm_or_ignore() else {
        panic!("no DRM device; this test is #[ignore]d and was run explicitly")
    };
    let mut executor = spawn_real_helper_for_tests(&device);
    let outcome = dispatch_and_wait_for_tests(&mut executor, &invalid_object_request_for_tests());
    match outcome {
        HostCallOutcome::Rejected { errno, .. } => assert_ne!(errno, 0),
        other => panic!("expected an explicit rejection, got {other:?}"),
    }
}

#[test]
fn a_reply_bitmap_outside_the_declared_slot_table_is_malformed() {
    for (slot_count, mask, fds) in [
        (0usize, 0b1000_0000_0000_0000_0000_0000_0000_0000u32, 1usize),
        (1, 0b10, 1),
    ] {
        let mut executor = spawn_scripted_helper_for_tests(ScriptedReply::Accepted { mask, fds });
        let outcome =
            dispatch_and_wait_for_tests(&mut executor, &request_with_slots_for_tests(slot_count));
        assert!(
            matches!(
                outcome,
                HostCallOutcome::Unknown(UnknownReason::MalformedReply)
            ),
            "slot_count={slot_count} mask={mask:#b}"
        );
    }
}

#[test]
fn a_reply_whose_correlation_does_not_match_is_malformed_never_a_rejection() {
    let mut executor = spawn_scripted_helper_for_tests(ScriptedReply::StaleCorrelation);
    let outcome = dispatch_and_wait_for_tests(&mut executor, &small_atomic_request_for_tests());
    assert!(matches!(
        outcome,
        HostCallOutcome::Unknown(UnknownReason::MalformedReply)
    ));
}

#[test]
fn the_stub_target_always_exists_so_this_suite_cannot_silently_run_nothing() {
    let device = TestDevice::open_stub();
    assert!(device.is_stub());
}

// ---------------------------------------------------------------------------
// Step 1: Non-blocking tests
// ---------------------------------------------------------------------------

#[test]
fn send_returns_against_a_helper_that_will_never_reply() {
    // NeverReply never writes to the control socket. A `send` that waited for
    // a reply could not return from this call at any speed, so reaching the
    // next line is the proof — no threshold required.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(
            &test_support::small_atomic_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send returned, so it did not wait for a reply");
    assert!(executor.poll_reply().is_none());
}

#[test]
fn poll_reply_returns_none_repeatedly_against_a_silent_helper() {
    // Same argument: one blocking receive against NeverReply would never
    // return, so completing two hundred of them is the proof.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(
            &test_support::small_atomic_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    for _ in 0..200 {
        assert!(executor.poll_reply().is_none());
    }
}

#[test]
fn a_readable_control_fd_yields_the_outcome_with_its_correlation() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::RejectWith(libc::EINVAL)).expect("spawn");
    let request = small_atomic_request_for_tests();
    let sent = request.correlation();
    executor
        .send(
            &request,
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    match executor.poll_reply() {
        Some(HostCallEvent::Outcome {
            correlation,
            outcome: HostCallOutcome::Rejected { errno, .. },
        }) => {
            assert_eq!(correlation, sent);
            assert_eq!(errno, libc::EINVAL);
        }
        other => panic!("expected a correlated rejection, got {other:?}"),
    }
}

#[test]
fn only_one_host_call_may_be_in_flight() {
    // This is what serializes host calls now that send returns immediately.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(
            &small_atomic_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("first");
    assert_eq!(
        executor
            .send(
                &small_atomic_request_for_tests(),
                HostCallReservation::Submitting(SubmittingProof::for_tests()),
            )
            .unwrap_err(),
        SendError::AlreadyInFlight
    );
}

// ---------------------------------------------------------------------------
// Step 2: Terminalization tests
// ---------------------------------------------------------------------------

/// Table-driven so no acceptance-unknown path can be added later without
/// declaring its terminalization behaviour here.
#[test]
fn every_acceptance_unknown_path_terminalizes_once_and_stays_serialized() {
    struct Case {
        name: &'static str,
        drive: fn() -> (KmsIoExecutor, HostCallEvent),
    }
    let cases = [
        Case {
            name: "send failure",
            drive: drive_send_failure,
        },
        Case {
            name: "helper exit",
            drive: drive_helper_exit,
        },
        Case {
            name: "malformed reply",
            drive: drive_malformed_reply,
        },
        Case {
            name: "watchdog expiry",
            drive: drive_watchdog_expiry,
        },
    ];
    for case in cases {
        let (mut executor, event) = (case.drive)();
        assert!(
            matches!(
                event,
                HostCallEvent::Outcome {
                    outcome: HostCallOutcome::Unknown(_),
                    ..
                }
            ),
            "{}: expected one terminal unknown outcome, got {event:?}",
            case.name
        );
        assert_eq!(
            executor.state(),
            ExecutorState::Stalled,
            "{}: not serialized",
            case.name
        );
        assert_eq!(
            executor
                .send(
                    &small_atomic_request_for_tests(),
                    HostCallReservation::Submitting(SubmittingProof::for_tests()),
                )
                .unwrap_err(),
            SendError::Stalled,
            "{}: a second ioctl reached a helper whose acceptance is unknown",
            case.name
        );
        assert!(
            executor.poll_reply().is_none() || executor.state() == ExecutorState::Stalled,
            "{}: a second terminal event was emitted",
            case.name
        );
    }
}

#[test]
fn a_send_failure_still_produces_exactly_one_terminal_event() {
    // COMMIT-6 installs the record before the send, so a failed send must not
    // leave the caller with a record and no outcome. The parent cannot prove
    // the helper did not act, so the conservative classification is unknown.
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::ExitBeforeReply).expect("spawn");
    test_support::wait_for_helper_exit(&mut executor, Duration::from_secs(5));
    let request = small_atomic_request_for_tests();
    let sent = request.correlation();
    let err = executor
        .send(
            &request,
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .unwrap_err();
    assert_eq!(err, SendError::Ipc);
    match executor.poll_reply() {
        Some(HostCallEvent::Outcome {
            correlation,
            outcome: HostCallOutcome::Unknown(_),
        }) => {
            assert_eq!(correlation, sent);
        }
        other => panic!("a failed send must terminalize its request, got {other:?}"),
    }
    assert!(executor.poll_reply().is_none(), "terminalized twice");
}

#[test]
fn eof_after_the_watchdog_does_not_emit_a_second_terminal_event() {
    // The helper dies from the watchdog's own SIGTERM, so its EOF arrives
    // after a terminal Unknown(WatchdogExpired) was already emitted. EOF then
    // means reap progress, not a second outcome.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(
            &small_atomic_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    let expiry = executor
        .tick(Instant::now() + Duration::from_secs(3))
        .expect("watchdog");
    assert!(matches!(
        expiry,
        HostCallEvent::Outcome {
            outcome: HostCallOutcome::Unknown(UnknownReason::WatchdogExpired),
            ..
        }
    ));
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    assert!(
        executor.poll_reply().is_none(),
        "EOF emitted a second terminal event"
    );
}

#[test]
fn a_malformed_reply_terminalizes_and_does_not_permit_a_retry() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::ReplyWithForeignCorrelation).expect("spawn");
    executor
        .send(
            &small_atomic_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    assert!(matches!(
        executor.poll_reply(),
        Some(HostCallEvent::Outcome {
            outcome: HostCallOutcome::Unknown(UnknownReason::MalformedReply),
            ..
        })
    ));
    assert_eq!(
        executor
            .send(
                &small_atomic_request_for_tests(),
                HostCallReservation::Submitting(SubmittingProof::for_tests()),
            )
            .unwrap_err(),
        SendError::Stalled,
        "a retry after a malformed reply would be a second ioctl under unknown acceptance"
    );
}

#[test]
fn the_watchdog_fires_from_tick_without_sleeping_to_reach_it() {
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(
            &small_atomic_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    // `tick` takes `now` as a parameter, so this is deterministic regardless
    // of scheduling: the deadline is crossed by passing a later instant, not
    // by waiting for one. That a `tick` implementation does not sleep to
    // reach its deadline is enforced by Task 7's grep, not by a stopwatch.
    assert!(
        executor.tick(Instant::now()).is_none(),
        "fired before the deadline"
    );
    let event = executor.tick(Instant::now() + Duration::from_secs(3));
    assert!(matches!(
        event,
        Some(HostCallEvent::Outcome {
            outcome: HostCallOutcome::Unknown(UnknownReason::WatchdogExpired),
            ..
        })
    ));
}

#[test]
fn a_reaped_executor_refuses_to_send_rather_than_appearing_available() {
    // The reviewed draft asserted that send succeeds after a forced reap.
    // It cannot: a reaped executor has no helper and no control socket, and
    // this stage defines no respawn. A new incarnation is 2b's job.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(
            &small_atomic_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    executor.tick(Instant::now() + Duration::from_secs(3));
    assert_eq!(executor.state(), ExecutorState::Stalled);
    test_support::reap_within(&mut executor, Duration::from_secs(5));
    assert_eq!(executor.state(), ExecutorState::Reaped);
    assert!(
        executor.control_fd().is_none(),
        "a reaped executor still exposes a control fd"
    );
    assert_eq!(
        executor
            .send(
                &small_atomic_request_for_tests(),
                HostCallReservation::Submitting(SubmittingProof::for_tests()),
            )
            .unwrap_err(),
        SendError::Reaped
    );
}

#[test]
fn helper_death_while_in_flight_is_acceptance_unknown_never_a_rejection() {
    // Whether the parent observes EOF before or after `try_wait` reports the
    // child reaped is a scheduling detail. Both classifications are
    // acceptance-unknown under COMMIT-6, and asserting on which one arrives
    // would be a racy test of an irrelevant distinction. What must hold is
    // that neither is a rejection.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(
            &small_atomic_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    test_support::kill_helper(&mut executor);
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    match executor.poll_reply() {
        Some(HostCallEvent::Outcome {
            outcome:
                HostCallOutcome::Unknown(UnknownReason::HelperExited | UnknownReason::IpcFailure),
            ..
        }) => {}
        other => panic!("helper death must be acceptance-unknown, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Step 3: Late-reply and descriptor-ownership tests
// ---------------------------------------------------------------------------

/// Returns (read_end, executor). The stub inherits the pipe's write end in
/// the KMS_FD slot and hands a duplicate of it back as the request's single
/// out-fence, then closes its own copies. The read end therefore reports EOF
/// exactly when the last parent-side copy is closed — which is what "adopted
/// and closed exactly once" means, observed rather than asserted.
fn executor_returning_a_pipe_write_end(
    delay: Duration,
    ignore_termination: bool,
) -> (std::fs::File, KmsIoExecutor) {
    let (read_end, write_end) = test_support::pipe_pair();
    let executor = test_support::spawn_stub_helper_with_inherited_fd(
        StubBehaviour::AcceptAfterReturningInheritedFd {
            delay,
            ignore_termination,
        },
        &write_end,
    )
    .expect("spawn");
    drop(write_end); // only the helper's copy and the returned duplicate remain
    (read_end, executor)
}

/// The read end is O_NONBLOCK, which is what makes this a *test* rather than a
/// hang. On a pipe whose write ends are all closed, a nonblocking read returns
/// `Ok(0)`; while any write end is still open and no data is queued it returns
/// `WouldBlock`. A blocking read in the second state waits forever, so
/// `test_support::pipe_pair` sets O_NONBLOCK on the read end before returning
/// it, and this helper distinguishes the two states instead of deadlocking on
/// the negative assertion below.
fn pipe_is_at_eof(read_end: &mut std::fs::File) -> bool {
    let mut buf = [0u8; 1];
    match read_end.read(&mut buf) {
        Ok(0) => true,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
        other => panic!("unexpected read on the fence pipe: {other:?}"),
    }
}

/// Proves *adoption and release*: the descriptor arrives owned, and after the
/// owner drops it no copy of the write end survives anywhere. It does not and
/// cannot prove close cardinality — a double close of a raw fd is not visible
/// through EOF — and it is a joint parent+helper assertion, because a helper
/// that leaked its own copy would keep the pipe open and fail this too.
#[test]
fn an_accepted_reply_adopts_its_out_fence_and_releases_it_on_drop() {
    let (mut read_end, mut executor) =
        executor_returning_a_pipe_write_end(Duration::from_millis(0), false);
    executor
        .send(
            &fence_returning_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    let Some(HostCallEvent::Outcome {
        outcome: HostCallOutcome::Accepted { out_fences, .. },
        ..
    }) = executor.poll_reply()
    else {
        panic!("expected an accepted outcome carrying one fence");
    };
    assert_eq!(
        out_fences.len(),
        1,
        "the descriptor is adopted, not dropped on the floor"
    );
    assert!(
        !pipe_is_at_eof(&mut read_end),
        "released before the owner dropped it"
    );
    drop(out_fences);
    assert!(
        pipe_is_at_eof(&mut read_end),
        "the adopted descriptor was leaked"
    );
}

#[test]
fn a_reply_arriving_after_the_watchdog_is_a_late_reply_whose_fds_are_adopted() {
    // The helper must survive the watchdog's SIGTERM to reply late at all.
    // StubBehaviour::AcceptAfter alone does not: stage 1's stub dies on
    // SIGTERM unless ignore_termination is set.
    let (mut read_end, mut executor) =
        executor_returning_a_pipe_write_end(Duration::from_millis(400), true);
    executor
        .send(
            &fence_returning_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    assert!(
        executor
            .tick(Instant::now() + Duration::from_secs(3))
            .is_some(),
        "watchdog"
    );
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    let rep = executor.poll_reply();
    let Some(HostCallEvent::LateReply {
        outcome: HostCallOutcome::Accepted { out_fences, .. },
        ..
    }) = rep
    else {
        panic!("expected a late reply");
    };
    assert_eq!(out_fences.len(), 1, "the late fd is adopted, not leaked");
    drop(out_fences);
    assert!(
        pipe_is_at_eof(&mut read_end),
        "the late descriptor was leaked"
    );
    assert_eq!(
        executor.state(),
        ExecutorState::Stalled,
        "a late reply is not reap proof and must not release serialization"
    );
    test_support::kill_and_reap(&mut executor);
}

#[test]
fn a_reply_declaring_more_fences_than_it_carries_is_malformed() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::AcceptDeclaringMissingFence).expect("spawn");
    executor
        .send(
            &fence_returning_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    assert!(matches!(
        executor.poll_reply(),
        Some(HostCallEvent::Outcome {
            outcome: HostCallOutcome::Unknown(UnknownReason::MalformedReply),
            ..
        })
    ));
}

// ---------------------------------------------------------------------------
// Step 4: Boundary and validation tests
// ---------------------------------------------------------------------------

#[test]
fn the_blocking_form_is_refused_once_the_seat_is_active() {
    // RejectWithRepeatedly, not RejectWith: stage 1's RejectWith answers one
    // request and then falls into a blocking one-byte read and exits
    // (`test_support.rs:213-231`). This test dispatches twice against one
    // executor, so a one-shot helper would make the second call wait out its
    // thirty-second watchdog and return Unknown instead of Ok.
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::RejectWithRepeatedly(libc::EINVAL))
            .expect("spawn");
    assert_eq!(executor.phase(), HostCallPhase::ColdStart);
    assert!(
        executor
            .dispatch_blocking_at_boundary(
                &blocking_atomic_request_for_tests(),
                HostCallReservation::Submitting(SubmittingProof::for_tests()),
            )
            .is_ok(),
        "cold start is a permitted blocking boundary"
    );

    executor.enter_seat_active();
    assert_eq!(
        executor
            .dispatch_blocking_at_boundary(
                &blocking_atomic_request_for_tests(),
                HostCallReservation::Submitting(SubmittingProof::for_tests()),
            )
            .unwrap_err(),
        BoundaryViolation,
        "COMMIT-5: no seat-active path may wait on a host call"
    );

    executor.enter_final_offline();
    assert!(
        executor
            .dispatch_blocking_at_boundary(
                &blocking_atomic_request_for_tests(),
                HostCallReservation::Submitting(SubmittingProof::for_tests()),
            )
            .is_ok(),
        "final offline is the other permitted blocking boundary"
    );
}

#[test]
fn send_refuses_a_cold_start_class_once_the_seat_is_active() {
    // COMMIT-5 on the path production actually uses. Guarding only the
    // blocking wrapper left this open: the request never blocks the core, but
    // it does ask the kernel for a blocking ioctl, and the relabelled
    // validation case silently buys a 30-second watchdog.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor.enter_seat_active();
    for request in [
        test_support::blocking_atomic_request_for_tests(),
        test_support::validation_request_for_tests(HostCallClass::ColdStartOrOfflineValidation),
    ] {
        let reservation = match request.class().is_validation() {
            true => HostCallReservation::Validation(ValidationLease::for_tests()),
            false => HostCallReservation::Submitting(SubmittingProof::for_tests()),
        };
        assert_eq!(
            executor.send(&request, reservation).unwrap_err(),
            SendError::BoundaryViolation,
            "{:?} must not be sendable while seat-active",
            request.class()
        );
    }
}

#[test]
fn a_validation_timeout_is_abandoned_not_acceptance_unknown() {
    // spec:320-329 — a validation timeout invalidates the candidate snapshot
    // but "never classifies hardware state as acceptance-unknown because no
    // live mutation was requested". Unknown would send 2b's owner into
    // COMMIT-6 quarantine over a call that touched nothing.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(
            &test_support::validation_request_for_tests(HostCallClass::SeatActiveValidation),
            HostCallReservation::Validation(ValidationLease::for_tests()),
        )
        .expect("send");
    let event = executor
        .tick(Instant::now() + Duration::from_secs(3))
        .expect("watchdog");
    assert!(
        matches!(
            event,
            HostCallEvent::Outcome {
                outcome: HostCallOutcome::ValidationAbandoned(UnknownReason::WatchdogExpired),
                ..
            }
        ),
        "got {event:?}"
    );
    // The executor is still unreliable, so serialization is unchanged.
    assert_eq!(executor.state(), ExecutorState::Stalled);
}

#[test]
fn a_reply_of_the_wrong_family_is_malformed_even_with_a_matching_correlation() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::ReplyWithWrongFamily).expect("spawn");
    executor
        .send(
            &test_support::probe_request_for_tests(),
            HostCallReservation::ClockProbe(ClockProbeLease::for_tests()),
        )
        .expect("send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    assert!(matches!(
        executor.poll_reply(),
        Some(HostCallEvent::Outcome {
            outcome: HostCallOutcome::Unknown(UnknownReason::MalformedReply),
            ..
        })
    ));
}

#[test]
fn a_clock_probe_is_sent_under_its_own_lease_and_its_sequence_survives() {
    // spec:642-645 — a probe owns no commit resources, so requiring a
    // SubmittingProof would mean installing a commit record for a read-only
    // query. And 2b picks KernelSequence vs Unresolved from this number, so
    // the outcome has to carry it.
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::AcceptProbeWith(4242)).expect("spawn");
    executor
        .send(
            &test_support::probe_request_for_tests(),
            HostCallReservation::ClockProbe(ClockProbeLease::for_tests()),
        )
        .expect("a probe is a legal send under a probe lease");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    match executor.poll_reply() {
        Some(HostCallEvent::Outcome {
            outcome: HostCallOutcome::ProbeAccepted { sequence, .. },
            ..
        }) => {
            assert_eq!(sequence, 4242);
        }
        other => panic!("the probe sequence must reach the caller, got {other:?}"),
    }
}

#[test]
fn validation_is_sent_under_a_validation_lease_not_a_submitting_record() {
    // spec:651-653 — ValidationOnly is neither a live blocking commit nor a
    // submitted record. Requiring SubmittingProof here would make the API
    // unusable for the validation the spec mandates.
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::RejectWith(libc::EINVAL)).expect("spawn");
    executor.enter_seat_active();
    executor
        .send(
            &validation_request_for_tests(HostCallClass::SeatActiveValidation),
            HostCallReservation::Validation(ValidationLease::for_tests()),
        )
        .expect("validation is a legal seat-active send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    assert!(matches!(
        executor.poll_reply(),
        Some(HostCallEvent::Outcome {
            outcome: HostCallOutcome::Rejected { .. },
            ..
        })
    ));
}

#[test]
fn the_reservation_kind_must_match_the_request_class() {
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    assert_eq!(
        executor
            .send(
                &validation_request_for_tests(HostCallClass::SeatActiveValidation),
                HostCallReservation::Submitting(SubmittingProof::for_tests()),
            )
            .unwrap_err(),
        SendError::ReservationMismatch
    );
    assert_eq!(
        executor
            .send(
                &small_atomic_request_for_tests(),
                HostCallReservation::Validation(ValidationLease::for_tests()),
            )
            .unwrap_err(),
        SendError::ReservationMismatch
    );
    assert_eq!(
        executor
            .send(
                &test_support::probe_request_for_tests(),
                HostCallReservation::Submitting(SubmittingProof::for_tests()),
            )
            .unwrap_err(),
        SendError::ReservationMismatch,
        "a probe must not be able to consume a commit record's proof"
    );
}

#[test]
fn each_class_carries_the_watchdog_the_spec_assigns_it() {
    // spec:320-329 gives seat-active validation two seconds and
    // cold-start/offline validation thirty. The two carry identical flags, so
    // only the explicit class field can tell them apart.
    use yserver::kms::executor::HostCallClass::*;
    for (class, expected) in [
        (SeatActiveNonblock, 2),
        (SeatActiveValidation, 2),
        (ColdStartOrOfflineBlocking, 30),
        (ColdStartOrOfflineValidation, 30),
    ] {
        assert_eq!(class.watchdog(), Duration::from_secs(expected), "{class:?}");
    }
    assert!(SeatActiveValidation.is_validation() && ColdStartOrOfflineValidation.is_validation());
    assert!(!SeatActiveNonblock.is_validation() && !ColdStartOrOfflineBlocking.is_validation());
}
