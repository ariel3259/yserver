// crates/yserver/tests/owner_commit_record.rs
//! The owner driven by real stub helper processes, so the correlation the
//! record matches against actually made a round trip.
//!
//! Every fixture here comes from `yserver::kms::owner::test_fixtures`, which
//! is `#[doc(hidden)] pub` rather than `#[cfg(test)]` precisely so this crate
//! can reach it.

use std::time::Duration;
use yserver::kms::{
    executor::{
        UnknownReason,
        test_support::{self, StubBehaviour},
    },
    owner::{
        device::OwnerEvent,
        record::{FailureCause, TerminalState, UnknownCause},
        test_fixtures::{ledger, owner_for_tests, single_active_crtc},
    },
};

// Every one of these must be imported: none is in the prelude, and revision 2
// used all six unqualified.

#[test]
fn a_rejecting_helper_drives_the_record_to_failed_before_submit() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::RejectWith(libc::EBUSY)).expect("spawn");
    let mut owner = owner_for_tests();
    let (commit, _) = owner
        .dispatch(&single_active_crtc(), ledger(), &mut executor)
        .expect("dispatch");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    let events = owner.apply_host_call_event(executor.poll_reply().expect("reply"));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected { .. }),
            ..
        }
    )));
    assert_eq!(owner.slot().occupant(), None);
    assert_eq!(owner.tombstones().back().expect("tombstone").commit, commit);
}

#[test]
fn a_helper_that_dies_leaves_the_slot_held_and_the_ledger_quarantined() {
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    let mut owner = owner_for_tests();
    let (commit, _) = owner
        .dispatch(&single_active_crtc(), ledger(), &mut executor)
        .expect("dispatch");
    test_support::kill_helper(&mut executor);
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    owner.apply_host_call_event(executor.poll_reply().expect("terminal event"));
    assert_eq!(
        owner.slot().occupant(),
        Some(commit),
        "COMMIT-6: a dead helper proves nothing about acceptance"
    );
}

#[test]
fn a_watchdog_expiry_reaches_the_owner_through_tick_without_sleeping() {
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    let mut owner = owner_for_tests();
    let (commit, _) = owner
        .dispatch(&single_active_crtc(), ledger(), &mut executor)
        .expect("dispatch");
    let past = executor.next_deadline().expect("a deadline exists") + Duration::from_millis(1);
    owner.apply_host_call_event(executor.tick(past).expect("the watchdog fires"));
    assert_eq!(owner.slot().occupant(), Some(commit));
    assert!(matches!(
        owner.tombstones().back().expect("tombstone").terminal,
        TerminalState::CompletionUnknown(UnknownCause::HostCall(UnknownReason::WatchdogExpired))
    ));
}
