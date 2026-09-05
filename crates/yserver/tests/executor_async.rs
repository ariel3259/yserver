//! Integration tests for process-isolated KMS executor.

use yserver::kms::executor::{test_support::*, *};

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
