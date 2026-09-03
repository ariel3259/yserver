//! Integration coverage for the C.0 executor substrate. These tests spawn
//! real helper processes; they never touch a real DRM device.

use yserver::kms::executor::*;

#[test]
fn a_watchdog_expiry_is_unknown_and_never_rejection() {
    let mut executor = yserver::kms::executor::test_support::spawn_stub_helper(
        yserver::kms::executor::test_support::StubBehaviour::NeverReply,
    )
    .expect("spawn");
    let outcome = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
    assert!(
        matches!(
            outcome,
            HostCallOutcome::Unknown(UnknownReason::WatchdogExpired)
        ),
        "a timeout must stay Unknown: got {outcome:?}"
    );
}

#[test]
fn a_helper_that_exits_before_replying_is_unknown_and_never_rejection() {
    let mut executor = yserver::kms::executor::test_support::spawn_stub_helper(
        yserver::kms::executor::test_support::StubBehaviour::ExitBeforeReply,
    )
    .expect("spawn");
    let outcome = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
    assert!(matches!(
        outcome,
        HostCallOutcome::Unknown(UnknownReason::HelperExited)
    ));
}

#[test]
fn an_explicit_errno_reply_is_rejection() {
    let mut executor = yserver::kms::executor::test_support::spawn_stub_helper(
        yserver::kms::executor::test_support::StubBehaviour::RejectWith(libc::EBUSY),
    )
    .expect("spawn");
    let outcome = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
    assert!(matches!(outcome, HostCallOutcome::Rejected { errno, .. } if errno == libc::EBUSY));
}

#[test]
fn termination_alone_is_not_reap_proof() {
    let mut executor = yserver::kms::executor::test_support::spawn_stub_helper(
        yserver::kms::executor::test_support::StubBehaviour::IgnoreTermination,
    )
    .expect("spawn");
    executor.request_termination();
    assert!(
        matches!(executor.try_reap(), ReapState::Running | ReapState::Stalled),
        "a signal is a request, not proof"
    );
}

#[test]
fn the_helper_replies_with_its_own_measured_ioctl_duration() {
    // The stub performs a deliberate 5 ms sleep in place of an ioctl, so the
    // reply's helper duration must exceed it while the transport term stays
    // far below. This is the split the transport criteria depend on.
    let mut executor = yserver::kms::executor::test_support::spawn_stub_helper(
        yserver::kms::executor::test_support::StubBehaviour::AcceptAfter(
            std::time::Duration::from_millis(5),
        ),
    )
    .expect("spawn");
    let outcome = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
    match outcome {
        HostCallOutcome::Accepted {
            helper_duration_ns,
            round_trip_ns,
            ..
        } => {
            assert!(
                helper_duration_ns >= 5_000_000,
                "helper must report its own ioctl time"
            );
            assert!(
                round_trip_ns >= helper_duration_ns,
                "the round trip contains the ioctl, so it cannot be shorter"
            );
        }
        other => panic!("expected Accepted, got {other:?}"),
    }
}

#[test]
fn the_helper_never_reads_the_event_fd() {
    // The owner writes a synthetic event into the shared pipe standing in for
    // the DRM event stream; after a full request/reply exchange the owner must
    // still be able to read every byte.
    let (owner_side, helper_side) = std::io::pipe().expect("pipe");
    let mut executor = yserver::kms::executor::test_support::spawn_stub_helper_with_event_fd(
        yserver::kms::executor::test_support::StubBehaviour::AcceptAfter(std::time::Duration::ZERO),
        helper_side,
    )
    .expect("spawn");
    yserver::kms::executor::test_support::write_synthetic_event(&owner_side, 32);
    let _ = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
    assert_eq!(
        yserver::kms::executor::test_support::readable_bytes(&owner_side),
        32,
        "the helper consumed events the owner will now never see"
    );
}
