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
