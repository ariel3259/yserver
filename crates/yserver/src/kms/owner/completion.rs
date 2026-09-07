//! Completion evidence state, context, and qualification for KMS commits.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

use crate::kms::{
    executor::HostCallClass,
    owner::clock::{ClockKey, ClockSample},
};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CompletionClass {
    FastUpdate,
    LifecycleInstallRestore,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CompletionContext {
    pub class: CompletionClass,
    pub host_class: HostCallClass,
    pub allow_modeset: bool,
    pub clocks: BTreeMap<u32, ClockKey>,
    pub mode_periods: BTreeMap<u32, Option<Duration>>,
    pub lifecycle_observed_max: Option<Duration>,
}

#[derive(Debug, Default)]
pub struct CompletionState {
    pub observed: BTreeSet<u32>,
    pub staged_general: BTreeMap<u32, ClockSample>,
    pub staged_present: BTreeMap<u32, ClockSample>,
    pub successful_fences: BTreeSet<u32>,
    pub accepted_at: Option<Instant>,
    pub hardware_complete_at: Option<Instant>,
    pub hardware_deadline: Option<Instant>,
    pub present_deadlines: BTreeMap<u32, Instant>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MechanismFailure {
    MalformedEvent,
    ActiveEventContradiction,
    ClockContradiction,
    FenceInvalid,
    FenceError,
    FencePollError,
    HardwareTimeout,
    PresentTimeout,
    DeadlineOverflow,
    HostCallUnknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        drm::event_stream::DrmEventRecord,
        kms::{
            executor::HostCallOutcome,
            owner::{
                clock::ClockKey,
                device::OwnerEvent,
                identity::{ClockEpochId, IncarnationId},
                ledger::LedgerState,
                lifecycle::LifecycleEpochId,
                record::{TerminalState, UnknownCause},
                sequence::{SequenceConsumer, SequencePurpose},
                test_fixtures::*,
            },
        },
    };

    fn setup_owner_with_clock(
        crtc: u32,
        reference: u64,
    ) -> (
        crate::kms::owner::device::DeviceCommitOwner<TestResource>,
        ClockKey,
    ) {
        let mut owner = owner_for_tests();
        let key = ClockKey {
            hardware_crtc: crtc,
            epoch: ClockEpochId::first(),
        };
        owner
            .install_clock(key, LifecycleEpochId::first(), 1)
            .unwrap();
        let clock = owner.clock_mut(key).unwrap();
        clock.install_reference(reference);
        (owner, key)
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Valid page event before ioctl success
    /// stages sample in CompletionState, but emits no Presented event.
    #[test]
    fn valid_page_before_ioctl_success_stages_but_emits_no_presented() {
        let (mut owner, key) = setup_owner_with_clock(1, 100);
        let desc = single_active_crtc_with_present(1);
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (_commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        let event = event_for_current_record(&owner, 1, 101, 10, 5_000);
        let now = Instant::now();
        let events = owner.apply_drm_event(IncarnationId::first(), event, now);

        // Pre-accept page event must not emit Presented or general ClockSample
        assert!(
            events.is_empty(),
            "pre-accept event must emit no owner events"
        );

        let record = owner.live_record().unwrap();
        assert!(record.completion_state().observed.contains(&1));
        let general_sample = record.completion_state().staged_general.get(&1).unwrap();
        assert_eq!(general_sample.msc, 101);
        assert_eq!(general_sample.ust, 10_005_000);
        let present_sample = record.completion_state().staged_present.get(&1).unwrap();
        assert_eq!(present_sample.msc, 101);
        assert_eq!(present_sample.ust, 10_005_000);
        assert!(!record.milestones().presented);
        assert!(!record.milestones().accepted);
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Explicit success publishes staged
    /// general samples; if all Present CRTCs are staged, publishes Presented { commit, samples }.
    #[test]
    fn explicit_success_publishes_staged_samples_and_presented() {
        let (mut owner, key) = setup_owner_with_clock(1, 100);
        let desc = single_active_crtc_with_present(1);
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        let event = event_for_current_record(&owner, 1, 101, 10, 5_000);
        let now = Instant::now();
        owner.apply_drm_event(IncarnationId::first(), event, now);

        let reply = accepted(commit, 1, 1);
        let reply_events = owner.apply_host_call_event_at(reply, now);

        let mut has_accepted = false;
        let mut has_clock_sample = false;
        let mut has_presented = false;

        for ev in &reply_events {
            match ev {
                OwnerEvent::Accepted { commit: c } => {
                    assert_eq!(*c, commit);
                    has_accepted = true;
                }
                OwnerEvent::ClockSample { key: k, sample, .. } => {
                    assert_eq!(*k, key);
                    assert_eq!(sample.msc, 101);
                    assert_eq!(sample.ust, 10_005_000);
                    has_clock_sample = true;
                }
                OwnerEvent::Presented { commit: c, samples } => {
                    assert_eq!(*c, commit);
                    let sample = samples.get(&1).unwrap();
                    assert_eq!(sample.msc, 101);
                    assert_eq!(sample.ust, 10_005_000);
                    has_presented = true;
                }
                _ => {}
            }
        }

        assert!(has_accepted, "Accepted must be emitted");
        assert!(
            has_clock_sample,
            "ClockSample must be published on acceptance"
        );
        assert!(
            has_presented,
            "Presented must be emitted when all Present CRTCs staged"
        );
        assert!(owner.live_record().unwrap().milestones().presented);
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Explicit rejection after a consumer
    /// or non-consumer event quarantines both resource sets (contradictory CompletionUnknown, retaining slot).
    #[test]
    fn explicit_rejection_after_consumer_event_quarantines_both_resource_sets() {
        let (mut owner, key) = setup_owner_with_clock(1, 100);
        let desc = single_active_crtc_with_present(1);
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        let event = event_for_current_record(&owner, 1, 101, 10, 5_000);
        owner.apply_drm_event(IncarnationId::first(), event, Instant::now());

        // Now deliver rejection
        let reject_event = crate::kms::executor::HostCallEvent::Outcome {
            correlation: *owner.live_record().unwrap().correlation(),
            outcome: HostCallOutcome::Rejected {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                errno: 22,
                unexpected_fence_output: false,
            },
        };
        let events = owner.apply_host_call_event(reject_event);

        assert!(matches!(
            events.as_slice(),
            [
                OwnerEvent::Terminal {
                    commit: c,
                    terminal: TerminalState::CompletionUnknown(UnknownCause::ContradictoryEvidence),
                },
                OwnerEvent::Quarantined { commit: c2 },
            ] if *c == commit && *c2 == commit
        ));

        // Slot remains reserved; live record survives
        assert_eq!(owner.slot().occupant(), Some(commit));
        assert!(owner.live_record().is_some());

        // Both resource sets quarantined
        let record = owner.live_record().unwrap();
        match record.ledger() {
            LedgerState::Quarantined(q) => {
                assert_eq!(
                    q.held(),
                    &[
                        TestResource::OldFramebuffer(66),
                        TestResource::NewFramebuffer(77)
                    ]
                );
            }
            other => panic!("expected Quarantined ledger, got {:?}", other),
        }
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Explicit rejection after a non-consumer
    /// event also quarantines both resource sets.
    #[test]
    fn explicit_rejection_after_non_consumer_event_quarantines_both_resource_sets() {
        let (mut owner, key) = setup_owner_with_clock(1, 100);
        let desc = single_active_crtc_non_consumer_page_flip();
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        let event = event_for_current_record(&owner, 1, 101, 10, 5_000);
        owner.apply_drm_event(IncarnationId::first(), event, Instant::now());

        let reject_event = crate::kms::executor::HostCallEvent::Outcome {
            correlation: *owner.live_record().unwrap().correlation(),
            outcome: HostCallOutcome::Rejected {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                errno: 22,
                unexpected_fence_output: false,
            },
        };
        let events = owner.apply_host_call_event(reject_event);

        assert!(matches!(
            events.as_slice(),
            [
                OwnerEvent::Terminal {
                    commit: c,
                    terminal: TerminalState::CompletionUnknown(UnknownCause::ContradictoryEvidence),
                },
                OwnerEvent::Quarantined { commit: c2 },
            ] if *c == commit && *c2 == commit
        ));

        assert_eq!(owner.slot().occupant(), Some(commit));
        assert!(owner.live_record().is_some());
        let record = owner.live_record().unwrap();
        assert!(matches!(record.ledger(), LedgerState::Quarantined(_)));
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Two Present CRTCs require two distinct
    /// events; duplicate first event does not complete the set.
    #[test]
    fn two_present_crtcs_require_two_distinct_events_duplicate_does_not_complete() {
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
        owner.clock_mut(k2).unwrap().install_reference(200);

        let desc = two_active_crtcs_with_present(1, 2);
        let context = fast_context_for_crtcs(&[(1, k1), (2, k2)]);

        let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        // Deliver event for CRTC 1
        let ev1 = event_for_current_record(&owner, 1, 101, 10, 5_000);
        owner.apply_drm_event(IncarnationId::first(), ev1, Instant::now());

        // Deliver DUPLICATE event for CRTC 1
        let ev1_dup = event_for_current_record(&owner, 1, 102, 10, 6_000);
        let dup_events = owner.apply_drm_event(IncarnationId::first(), ev1_dup, Instant::now());
        assert!(
            dup_events.is_empty(),
            "duplicate event must be telemetry only"
        );

        // Deliver Accepted reply (count 2 out-fences)
        let reply = accepted(commit, 3, 2);
        let reply_events = owner.apply_host_call_event(reply);
        assert!(
            reply_events
                .iter()
                .any(|e| matches!(e, OwnerEvent::Accepted { .. }))
        );
        assert!(
            !reply_events
                .iter()
                .any(|e| matches!(e, OwnerEvent::Presented { .. })),
            "Presented must not be emitted until CRTC 2 event arrives"
        );

        // Now deliver event for CRTC 2
        let ev2 = event_for_current_record(&owner, 2, 201, 10, 5_000);
        let ev2_events = owner.apply_drm_event(IncarnationId::first(), ev2, Instant::now());

        assert!(
            ev2_events
                .iter()
                .any(|e| matches!(e, OwnerEvent::Presented { .. })),
            "Presented must be emitted after CRTC 2 event arrives"
        );
        assert!(owner.live_record().unwrap().milestones().presented);
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Raw zero wraps from a trusted high reference.
    #[test]
    fn raw_zero_wraps_from_trusted_high_reference() {
        let (mut owner, key) = setup_owner_with_clock(1, 0xFFFF_FFFE);
        let desc = single_active_crtc_with_present(1);
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        let event = event_for_current_record(&owner, 1, 0, 10, 5_000);
        owner.apply_drm_event(IncarnationId::first(), event, Instant::now());

        let reply = accepted(commit, 1, 1);
        let events = owner.apply_host_call_event(reply);

        let presented = events
            .iter()
            .find_map(|e| match e {
                OwnerEvent::Presented { samples, .. } => Some(samples),
                _ => None,
            })
            .expect("Presented event");

        let sample = presented.get(&1).unwrap();
        assert_eq!(sample.msc, 0x1_0000_0000);
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Invalid microseconds poisons immediately.
    #[test]
    fn poison_invalid_microseconds() {
        let (mut owner, key) = setup_owner_with_clock(1, 100);
        let desc = single_active_crtc_with_present(1);
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (_commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        let token = owner.live_record().unwrap().event_token();
        let bytes = page_event_bytes(1, 101, 10, 1_000_000, token.as_user_data());
        let (mut records, _) = crate::drm::event_stream::parse_event_buffer_partial(&bytes);
        let event = records.pop().unwrap();

        let events = owner.apply_drm_event(IncarnationId::first(), event, Instant::now());
        assert!(owner.is_poisoned());
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::MechanismFailed {
                reason: MechanismFailure::ClockContradiction,
            }
        )));
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Exact half-range poisons immediately.
    #[test]
    fn poison_exact_half_range() {
        let (mut owner, key) = setup_owner_with_clock(1, 0);
        let desc = single_active_crtc_with_present(1);
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (_commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        let event = event_for_current_record(&owner, 1, 0x8000_0000, 10, 5_000);
        let events = owner.apply_drm_event(IncarnationId::first(), event, Instant::now());
        assert!(owner.is_poisoned());
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::MechanismFailed {
                reason: MechanismFailure::ClockContradiction,
            }
        )));
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Wrong current CRTC poisons immediately.
    #[test]
    fn poison_wrong_current_crtc() {
        let (mut owner, key) = setup_owner_with_clock(1, 100);
        let desc = single_active_crtc_with_present(1);
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (_commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        // Event specifies CRTC 2, which is not in KernelEventCrtcs
        let event = event_for_current_record(&owner, 2, 101, 10, 5_000);
        let events = owner.apply_drm_event(IncarnationId::first(), event, Instant::now());
        assert!(owner.is_poisoned());
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::MechanismFailed {
                reason: MechanismFailure::ActiveEventContradiction,
            }
        )));
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Zero current CRTC poisons immediately.
    #[test]
    fn poison_zero_current_crtc() {
        let (mut owner, key) = setup_owner_with_clock(1, 100);
        let desc = single_active_crtc_with_present(1);
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (_commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        // Event specifies CRTC 0
        let event = event_for_current_record(&owner, 0, 101, 10, 5_000);
        let events = owner.apply_drm_event(IncarnationId::first(), event, Instant::now());
        assert!(owner.is_poisoned());
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::MechanismFailed {
                reason: MechanismFailure::ActiveEventContradiction,
            }
        )));
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Wrong current event type poisons immediately.
    #[test]
    fn poison_wrong_current_event_type() {
        let (mut owner, key) = setup_owner_with_clock(1, 100);
        let desc = single_active_crtc_with_present(1);
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (_commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        let token = owner.live_record().unwrap().event_token();
        // Send Vblank instead of PageFlip for current atomic token
        let event = DrmEventRecord::Vblank {
            crtc_id: 1,
            sequence: 101,
            tv_sec: 10,
            tv_usec: 5_000,
            user_data: token.as_user_data(),
        };
        let events = owner.apply_drm_event(IncarnationId::first(), event, Instant::now());
        assert!(owner.is_poisoned());
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::MechanismFailed {
                reason: MechanismFailure::ActiveEventContradiction,
            }
        )));
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Unknown, tombstoned, and old-incarnation
    /// events do not touch C.0 state (telemetry only).
    #[test]
    fn telemetry_only_for_unknown_tombstoned_old_incarnation() {
        let (mut owner, key) = setup_owner_with_clock(1, 100);
        let desc = single_active_crtc_with_present(1);
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (_commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        // 1. Old incarnation event
        let old_inc = IncarnationId::from_raw(99);
        let ev_old = event_for_current_record(&owner, 1, 101, 10, 5_000);
        let evs = owner.apply_drm_event(old_inc, ev_old, Instant::now());
        assert!(evs.is_empty());
        assert!(!owner.is_poisoned());

        // 2. Unknown token event
        let ev_unknown = DrmEventRecord::PageFlip {
            crtc_id: 1,
            sequence: 101,
            tv_sec: 10,
            tv_usec: 5_000,
            user_data: 999_999,
        };
        let evs = owner.apply_drm_event(IncarnationId::first(), ev_unknown, Instant::now());
        assert!(evs.is_empty());
        assert!(!owner.is_poisoned());
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] A sequence advance after staging cannot
    /// replace the stored Present timestamp.
    #[test]
    fn sequence_advance_after_staging_cannot_replace_stored_present_timestamp() {
        let (mut owner, key) = setup_owner_with_clock(1, 100);
        let desc = single_active_crtc_with_present(1);
        let context = fast_context_for_crtcs(&[(1, key)]);

        let (commit, _) = owner.begin_with_context(&desc, ledger(), context).unwrap();
        owner.mark_dispatched_for_tests();

        // Stage page event with MSC 101, UST 10_005_000
        let page_ev = event_for_current_record(&owner, 1, 101, 10, 5_000);
        owner.apply_drm_event(IncarnationId::first(), page_ev, Instant::now());

        // Advance the clock through a sequence arm to MSC 105, UST 20_000_000
        let c = SequenceConsumer(9001);
        let arm_token = owner
            .reserve_arm(key, SequencePurpose::IdleClockWake, 105, &[c])
            .unwrap();
        // Resolve queue accepted
        let queue_reply = crate::kms::executor::HostCallEvent::Outcome {
            correlation: crate::kms::executor::protocol::HostCallCorrelation::SequenceQueue {
                seq: crate::kms::executor::protocol::RequestSeq::from_raw(10),
                incarnation: IncarnationId::first(),
                lifecycle_epoch: LifecycleEpochId::first(),
                topology_generation: 1,
                hardware_crtc: 1,
                clock_epoch: ClockEpochId::first(),
                token: arm_token,
            },
            outcome: HostCallOutcome::QueueAccepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                sequence: 105,
            },
        };
        owner.apply_host_call_event(queue_reply);

        // Sequence event arrives advancing clock to 105
        let seq_ev = DrmEventRecord::CrtcSequence {
            user_data: arm_token.as_user_data(),
            time_ns: 20_000_000_000,
            sequence: 105,
        };
        owner.apply_drm_event(IncarnationId::first(), seq_ev, Instant::now());

        // Assert general clock advanced to 105
        assert_eq!(owner.clock(key).unwrap().latest.unwrap().msc, 105);

        // Now accept the atomic commit
        let reply = accepted(commit, 1, 1);
        let events = owner.apply_host_call_event(reply);

        let presented = events
            .iter()
            .find_map(|e| match e {
                OwnerEvent::Presented { samples, .. } => Some(samples),
                _ => None,
            })
            .expect("Presented event");

        // The Present timestamp MUST still be the staged 101, not replaced by the sequence advance!
        assert_eq!(presented.get(&1).unwrap().msc, 101);
        assert_eq!(presented.get(&1).unwrap().ust, 10_005_000);
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] report_stream_failure latches MalformedEvent
    /// on the current incarnation only.
    #[test]
    fn report_stream_failure_poisons_current_incarnation_only() {
        let mut owner = owner_for_tests();
        let wrong_inc = IncarnationId::from_raw(99);
        let events = owner.report_stream_failure(wrong_inc, Instant::now());
        assert!(events.is_empty());
        assert!(!owner.is_poisoned());

        let events = owner.report_stream_failure(IncarnationId::first(), Instant::now());
        assert!(owner.is_poisoned());
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::MechanismFailed {
                reason: MechanismFailure::MalformedEvent,
            }
        )));
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Active sequence token receiving a PageFlip
    /// poisons immediately with ActiveEventContradiction.
    #[test]
    fn active_sequence_token_with_wrong_event_type_poisons() {
        let (mut owner, key) = setup_owner_with_clock(1, 100);
        let c = SequenceConsumer(9001);
        let arm_token = owner
            .reserve_arm(key, SequencePurpose::IdleClockWake, 105, &[c])
            .unwrap();

        let wrong_ev = DrmEventRecord::PageFlip {
            crtc_id: 1,
            sequence: 105,
            tv_sec: 10,
            tv_usec: 5_000,
            user_data: arm_token.as_user_data(),
        };
        let events = owner.apply_drm_event(IncarnationId::first(), wrong_ev, Instant::now());
        assert!(owner.is_poisoned());
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::MechanismFailed {
                reason: MechanismFailure::ActiveEventContradiction,
            }
        )));
    }

    /// [ID-1..3, COMMIT-2, COMMIT-6, MULTI] Legacy drain permit delivers LegacyPageFlip
    /// on user_data == 0.
    #[test]
    fn legacy_permit_delivers_legacy_page_flip() {
        use crate::kms::owner::device::DeviceCommitOwner;
        let mut owner: DeviceCommitOwner<TestResource> =
            DeviceCommitOwner::new_legacy(IncarnationId::first(), LifecycleEpochId::first(), 1);

        let legacy_ev = DrmEventRecord::PageFlip {
            crtc_id: 1,
            sequence: 42,
            tv_sec: 12,
            tv_usec: 34,
            user_data: 0,
        };
        let events = owner.apply_drm_event(IncarnationId::first(), legacy_ev, Instant::now());
        assert!(matches!(
            events.as_slice(),
            [OwnerEvent::LegacyPageFlip {
                crtc_id: 1,
                sequence: 42,
                tv_sec: 12,
                tv_usec: 34,
            }]
        ));
    }
}
