//! Bounded asynchronous sequence arms and consumer cancellation.
//!
//! Tracks up to 256 active sequence arms and 4096 logical consumers per
//! device. Manages sequence tokens, dedup index, early-event staging,
//! and publication latching upon cancellation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub use crate::kms::owner::identity::SequenceArmToken;
use crate::kms::owner::{
    clock::{ClockKey, ClockSample},
    lifecycle::LifecycleEpochId,
};

pub const MAX_ACTIVE_ARMS: usize = 256;
pub const MAX_LOGICAL_CONSUMERS: usize = 4096;
pub const SEQUENCE_TOMBSTONE_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum SequencePurpose {
    IdleClockWake,
    PresentTargetWake,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SequenceConsumer(pub u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ClockSampleOrigin {
    PageFlip,
    Sequence(SequencePurpose),
}

#[derive(Debug, thiserror::Error, Clone, Copy, Eq, PartialEq)]
pub enum SequenceError {
    #[error("clock not ready")]
    ClockNotReady,
    #[error("capacity exceeded")]
    Capacity,
    #[error("identity space exhausted")]
    IdentityExhausted,
    #[error("unknown arm")]
    UnknownArm,
    #[error("invalid identity")]
    InvalidIdentity,
    #[error("invalid sample")]
    InvalidSample,
    #[error("busy")]
    Busy,
    #[error("transport unavailable")]
    TransportUnavailable,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SequenceArmPhase {
    PendingDispatch,
    InFlight,
    Armed,
}

#[derive(Debug)]
pub struct SequenceArm {
    pub token: SequenceArmToken,
    pub key: ClockKey,
    pub lifecycle_epoch: LifecycleEpochId,
    pub topology_generation: u64,
    pub purpose: SequencePurpose,
    pub requested_target: u64,
    pub scheduled_target: Option<u64>,
    pub consumers: BTreeSet<SequenceConsumer>,
    pub phase: SequenceArmPhase,
    pub publishable: bool,
    pub staged_sample: Option<ClockSample>,
}

#[derive(Debug, Default)]
pub struct SequenceArms {
    pub(crate) arms: BTreeMap<SequenceArmToken, SequenceArm>,
    pub(crate) index: BTreeMap<(ClockKey, SequencePurpose, u64), SequenceArmToken>,
    pub(crate) fifo: VecDeque<SequenceArmToken>,
    pub(crate) tombstones: VecDeque<SequenceArmToken>,
    pub(crate) total_consumers: usize,
}

impl SequenceArms {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_tombstone(&mut self, token: SequenceArmToken) {
        self.tombstones.push_back(token);
        if self.tombstones.len() > SEQUENCE_TOMBSTONE_CAPACITY {
            self.tombstones.pop_front();
        }
    }

    pub fn is_tombstone(&self, token: SequenceArmToken) -> bool {
        self.tombstones.contains(&token)
    }

    pub fn get(&self, token: &SequenceArmToken) -> Option<&SequenceArm> {
        self.arms.get(token)
    }

    pub fn contains_key(&self, token: &SequenceArmToken) -> bool {
        self.arms.contains_key(token)
    }

    pub fn cancel_consumer(&mut self, consumer: SequenceConsumer) {
        let mut to_process = Vec::new();
        for (token, arm) in &mut self.arms {
            if arm.consumers.remove(&consumer) {
                self.total_consumers = self.total_consumers.saturating_sub(1);
                if arm.consumers.is_empty() {
                    arm.publishable = false;
                    to_process.push(*token);
                }
            }
        }
        for token in to_process {
            if let Some(mut arm) = self.arms.remove(&token) {
                self.index
                    .remove(&(arm.key, arm.purpose, arm.requested_target));
                if arm.phase == SequenceArmPhase::PendingDispatch
                    || arm.phase == SequenceArmPhase::Armed
                {
                    self.fifo.retain(|t| *t != token);
                    self.push_tombstone(token);
                } else {
                    // InFlight: retain in arms map so reply correlation can resolve and release lease
                    arm.consumers.clear();
                    arm.publishable = false;
                    self.arms.insert(token, arm);
                }
            }
        }
    }

    pub fn cancel_matching_clock(&mut self, key: ClockKey) {
        let matching_tokens: Vec<SequenceArmToken> = self
            .arms
            .iter()
            .filter_map(|(&token, arm)| if arm.key == key { Some(token) } else { None })
            .collect();
        for token in matching_tokens {
            if let Some(mut arm) = self.arms.remove(&token) {
                self.index
                    .remove(&(arm.key, arm.purpose, arm.requested_target));
                self.total_consumers = self.total_consumers.saturating_sub(arm.consumers.len());
                self.fifo.retain(|t| *t != token);
                if arm.phase == SequenceArmPhase::InFlight {
                    arm.consumers.clear();
                    arm.publishable = false;
                    self.arms.insert(token, arm);
                } else {
                    self.push_tombstone(token);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        drm::event_stream::DrmEventRecord,
        kms::owner::{
            NeverResource,
            clock::{ClockKey, ClockSource, ProbeState},
            device::DeviceCommitOwner,
            identity::{ClockEpochId, IncarnationId},
            lifecycle::LifecycleEpochId,
        },
    };
    use std::time::Instant;

    fn setup_owner() -> (DeviceCommitOwner<NeverResource>, ClockKey) {
        let mut owner =
            DeviceCommitOwner::new(IncarnationId::first(), LifecycleEpochId::first(), 1);
        let key = ClockKey {
            hardware_crtc: 1,
            epoch: ClockEpochId::first(),
        };
        owner
            .install_clock(key, LifecycleEpochId::first(), 1)
            .unwrap();
        let clock = owner.clock_mut(key).unwrap();
        clock.source = ClockSource::KernelSequence;
        clock.probe = ProbeState::Succeeded;
        clock.reference = Some(0);
        (owner, key)
    }

    /// [ID-1..3, COMMIT-5, CAP-1..4, MULTI]
    /// Two consumers for (CRTC=1,epoch=1,target=50) share one token.
    /// Cancel one: event still resolves for the other.
    /// Cancel both: delayed event changes no clock.
    #[test]
    fn two_consumers_share_one_token_and_cancellation_preserves_remaining() {
        let (mut owner, key) = setup_owner();
        let c1 = SequenceConsumer(101);
        let c2 = SequenceConsumer(102);

        let t1 = owner
            .reserve_arm(key, SequencePurpose::PresentTargetWake, 50, &[c1])
            .expect("reserve c1");
        let t2 = owner
            .reserve_arm(key, SequencePurpose::PresentTargetWake, 50, &[c2])
            .expect("reserve c2");
        assert_eq!(
            t1, t2,
            "consumers for identical target must share arm token"
        );

        // Subpart A: Cancel c1, then deliver event. c2 remains, so clock advances.
        owner.cancel_consumer(c1);
        let events = owner.apply_sequence_event(
            IncarnationId::first(),
            t1.as_user_data(),
            1_000_000,
            50,
            Instant::now(),
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, crate::kms::owner::device::OwnerEvent::ClockSample { .. })),
            "ClockSample must be emitted for remaining consumer c2"
        );
        assert_eq!(
            owner.clock(key).unwrap().latest.map(|s| s.msc),
            Some(50),
            "clock MSC must advance to 50 for c2"
        );

        // Subpart B: Cancel both consumers: delayed event changes no clock.
        let (mut owner2, key2) = setup_owner();
        let t = owner2
            .reserve_arm(key2, SequencePurpose::PresentTargetWake, 60, &[c1, c2])
            .expect("reserve c1 and c2");
        owner2.cancel_consumer(c1);
        owner2.cancel_consumer(c2);
        let events2 = owner2.apply_sequence_event(
            IncarnationId::first(),
            t.as_user_data(),
            2_000_000,
            60,
            Instant::now(),
        );
        assert!(
            events2.is_empty(),
            "delayed event for cancelled arm must produce no events"
        );
        assert_eq!(
            owner2.clock(key2).unwrap().latest,
            None,
            "delayed event for cancelled arm must change no clock"
        );
    }

    /// [ID-1..3, COMMIT-5, CAP-1..4, MULTI]
    /// Fill 256 distinct arms and assert arm 257 returns Capacity without eviction.
    #[test]
    fn fill_256_distinct_arms_and_assert_257_returns_capacity_without_eviction() {
        let (mut owner, key) = setup_owner();
        for target in 1..=256 {
            let res = owner.reserve_arm(
                key,
                SequencePurpose::PresentTargetWake,
                target,
                &[SequenceConsumer(target)],
            );
            assert!(res.is_ok(), "arm {target} must be reserved");
        }

        let overflow = owner.reserve_arm(
            key,
            SequencePurpose::PresentTargetWake,
            257,
            &[SequenceConsumer(257)],
        );
        assert_eq!(
            overflow,
            Err(SequenceError::Capacity),
            "arm 257 must return Capacity"
        );

        // Verify arm 1 was not evicted: arm 1's target still deduplicates
        let c1_extra = SequenceConsumer(9999);
        let dedup = owner.reserve_arm(key, SequencePurpose::PresentTargetWake, 1, &[c1_extra]);
        assert!(
            dedup.is_ok(),
            "arm 1 must still exist without eviction and be shared"
        );
    }

    /// [ID-1..3, COMMIT-5, CAP-1..4, MULTI]
    /// Reuse raw CRTC 1 after epoch invalidation and prove old token cannot consume a new arm.
    #[test]
    fn reuse_raw_crtc_1_after_epoch_invalidation_and_old_token_cannot_consume_new_arm() {
        let (mut owner, key1) = setup_owner();
        let c = SequenceConsumer(501);
        let old_token = owner
            .reserve_arm(key1, SequencePurpose::PresentTargetWake, 75, &[c])
            .expect("reserve epoch 1");

        // Invalidate epoch 1 clock
        owner.invalidate_clock(key1);

        // Install new epoch 2 clock for the same hardware CRTC 1
        let key2 = ClockKey {
            hardware_crtc: 1,
            epoch: ClockEpochId::first().next(),
        };
        owner
            .install_clock(key2, LifecycleEpochId::first(), 1)
            .unwrap();
        let clock2 = owner.clock_mut(key2).unwrap();
        clock2.source = ClockSource::KernelSequence;
        clock2.probe = ProbeState::Succeeded;
        clock2.reference = Some(0);

        // Reserve new arm on epoch 2
        let new_token = owner
            .reserve_arm(key2, SequencePurpose::PresentTargetWake, 75, &[c])
            .expect("reserve epoch 2");
        assert_ne!(old_token, new_token, "new arm must have distinct token");

        // Attempt to consume new arm using old token
        let events = owner.apply_sequence_event(
            IncarnationId::first(),
            old_token.as_user_data(),
            1_000_000,
            75,
            Instant::now(),
        );
        assert!(
            events.is_empty(),
            "old token cannot produce events for new epoch"
        );
        assert_eq!(
            owner.clock(key2).unwrap().latest,
            None,
            "old token cannot advance epoch 2 clock"
        );
    }

    /// [ID-1..3, COMMIT-5, CAP-1..4, MULTI]
    /// Cross-device tokens with equal raw values cannot consume one another
    /// because the device owner is selected before token lookup.
    #[test]
    fn cross_device_tokens_with_equal_raw_values_cannot_consume_one_another() {
        let (mut owner1, key1) = setup_owner();
        let (mut owner2, key2) = setup_owner();

        let c = SequenceConsumer(801);
        let t1 = owner1
            .reserve_arm(key1, SequencePurpose::PresentTargetWake, 42, &[c])
            .expect("reserve owner1");
        let t2 = owner2
            .reserve_arm(key2, SequencePurpose::PresentTargetWake, 42, &[c])
            .expect("reserve owner2");

        // Verify equal raw values (both first sequence arm allocation from fresh allocator)
        assert_eq!(t1.as_user_data(), t2.as_user_data());

        // Apply event to owner1 only: owner1 clock updates, owner2 is untouched
        let events1 = owner1.apply_sequence_event(
            IncarnationId::first(),
            t1.as_user_data(),
            1_000_000,
            42,
            Instant::now(),
        );
        assert!(!events1.is_empty(), "owner1 must resolve event");
        assert_eq!(
            owner1.clock(key1).unwrap().latest.map(|s| s.msc),
            Some(42),
            "owner1 clock must advance"
        );
        assert_eq!(
            owner2.clock(key2).unwrap().latest,
            None,
            "owner2 clock must remain untouched"
        );
    }

    /// [ID-1..3, COMMIT-5, CAP-1..4, MULTI]
    /// Wrong-type current arm event poisons; unknown wrong-type token does not.
    #[test]
    fn wrong_type_current_arm_event_poisons_unknown_wrong_type_token_does_not() {
        let (mut owner, key) = setup_owner();
        let c = SequenceConsumer(901);
        let arm_token = owner
            .reserve_arm(key, SequencePurpose::PresentTargetWake, 100, &[c])
            .expect("reserve arm");

        // Subpart A: Unknown wrong-type token arrives -> does NOT poison
        let unknown_raw = 0xdead_beef;
        let unknown_event = DrmEventRecord::PageFlip {
            crtc_id: 1,
            sequence: 10,
            tv_sec: 1,
            tv_usec: 0,
            user_data: unknown_raw,
        };
        let events = owner.apply_drm_event(IncarnationId::first(), unknown_event, Instant::now());
        assert!(events.is_empty());
        assert!(!owner.is_poisoned(), "unknown token must not poison");

        // Subpart B: Current active arm token arrives as wrong-type event (PageFlip instead of CrtcSequence) -> POISONS
        let wrong_type_event = DrmEventRecord::PageFlip {
            crtc_id: 1,
            sequence: 100,
            tv_sec: 1,
            tv_usec: 0,
            user_data: arm_token.as_user_data(),
        };
        let poison_events =
            owner.apply_drm_event(IncarnationId::first(), wrong_type_event, Instant::now());
        assert!(
            owner.is_poisoned(),
            "current arm wrong-type event must poison"
        );
        assert!(
            poison_events.iter().any(|e| matches!(
                e,
                crate::kms::owner::device::OwnerEvent::MechanismFailed { .. }
            )),
            "MechanismFailed must be emitted upon contradiction"
        );
    }

    /// [ID-1..3, COMMIT-5, CAP-1..4, MULTI]
    /// Send QUEUE, stage its event, cancel the last consumer, then deliver QueueAccepted.
    /// Assert unchanged general/completion clocks, no clock or Present-wake output,
    /// and lease retention until reply resolution followed by exact-once disposal.
    #[test]
    fn staged_event_cancelled_last_consumer_then_queue_accepted_produces_no_wakes_and_releases_lease()
     {
        use crate::kms::executor::test_support::{self, StubBehaviour};
        use std::time::Duration;

        let (mut owner, key) = setup_owner();
        let c1 = SequenceConsumer(1001);
        let mut executor = test_support::spawn_stub_helper(StubBehaviour::AcceptQueueWith(50))
            .expect("spawn stub");

        let token = owner
            .reserve_arm(key, SequencePurpose::PresentTargetWake, 50, &[c1])
            .expect("reserve arm");
        owner
            .send_next_sequence_on(&mut executor)
            .expect("send QUEUE");

        // Assert slot has sequence lease outstanding
        assert_eq!(owner.slot().queue_outstanding(), Some(token));

        // Stage event before reply arrives
        let early_events = owner.apply_sequence_event(
            IncarnationId::first(),
            token.as_user_data(),
            1_000_000,
            50,
            Instant::now(),
        );
        assert!(
            early_events.is_empty(),
            "early event before reply must stage without emitting events"
        );
        assert_eq!(
            owner.clock(key).unwrap().latest,
            None,
            "staged event must not advance clock yet"
        );

        // Cancel the last consumer while exchange is in flight
        owner.cancel_consumer(c1);

        // Assert lease is still retained before reply arrives
        assert_eq!(
            owner.slot().queue_outstanding(),
            Some(token),
            "lease must be retained until reply resolution"
        );

        // Deliver QueueAccepted
        test_support::wait_readable(executor.control_fd().unwrap(), Duration::from_secs(5));
        let reply = executor.poll_reply().expect("poll reply");
        let reply_events = owner.apply_host_call_event(reply);

        // Assert no clock or Present-wake output
        assert!(
            reply_events.is_empty(),
            "cancelled arm must emit no clock or Present-wake output upon acceptance"
        );
        assert_eq!(
            owner.clock(key).unwrap().latest,
            None,
            "clocks must remain unchanged"
        );

        // Assert lease released and exact-once disposal into tombstones
        assert_eq!(
            owner.slot().queue_outstanding(),
            None,
            "lease must be released upon reply resolution"
        );
        assert!(
            owner.sequence_arms.is_tombstone(token),
            "arm must be disposed into tombstones"
        );
        assert!(
            !owner.sequence_arms.arms.contains_key(&token),
            "arm must be removed from arms map"
        );
    }

    /// [ID-1..3, COMMIT-5, CAP-1..4, MULTI]
    /// Cancel consumer before event, and create a fresh same-target arm while old exchange resolves.
    /// The new token cannot revive old evidence.
    #[test]
    fn cancel_before_event_fresh_same_target_arm_cannot_revive_old_evidence() {
        use crate::kms::executor::test_support::{self, StubBehaviour};
        use std::time::Duration;

        let (mut owner, key) = setup_owner();
        let c1 = SequenceConsumer(2001);
        let mut executor = test_support::spawn_stub_helper(StubBehaviour::AcceptQueueWith(60))
            .expect("spawn stub");

        let token1 = owner
            .reserve_arm(key, SequencePurpose::PresentTargetWake, 60, &[c1])
            .expect("reserve arm 1");
        owner
            .send_next_sequence_on(&mut executor)
            .expect("send QUEUE 1");

        // Cancel c1 before event arrives
        owner.cancel_consumer(c1);

        // Create fresh same-target arm while old exchange resolves
        let c2 = SequenceConsumer(2002);
        let token2 = owner
            .reserve_arm(key, SequencePurpose::PresentTargetWake, 60, &[c2])
            .expect("reserve arm 2 on same target");
        assert_ne!(token1, token2, "fresh arm must receive a distinct token");

        // Deliver old event for token1
        let old_events = owner.apply_sequence_event(
            IncarnationId::first(),
            token1.as_user_data(),
            2_000_000,
            60,
            Instant::now(),
        );
        assert!(
            old_events.is_empty(),
            "event for cancelled in-flight arm must produce no events"
        );

        // Deliver QueueAccepted for token1
        test_support::wait_readable(executor.control_fd().unwrap(), Duration::from_secs(5));
        let reply = executor.poll_reply().expect("poll reply");
        let reply_events = owner.apply_host_call_event(reply);
        assert!(
            reply_events.is_empty(),
            "resolution of old cancelled exchange must produce no events"
        );
        assert_eq!(
            owner.clock(key).unwrap().latest,
            None,
            "old exchange cannot advance clock or revive evidence"
        );
        assert_eq!(
            owner.slot().queue_outstanding(),
            None,
            "lease must be released"
        );

        // Token 2 remains pending dispatch and unaffected
        assert!(owner.sequence_arms.arms.contains_key(&token2));
        assert_eq!(
            owner.sequence_arms.arms.get(&token2).unwrap().phase,
            SequenceArmPhase::PendingDispatch
        );
    }

    /// [ID-1..3, COMMIT-5, CAP-1..4, MULTI]
    /// Repeat no-publication assertion for legacy output when cancelled.
    #[test]
    fn staged_cancellation_no_publication_for_legacy_output() {
        use crate::kms::executor::test_support::{self, StubBehaviour};
        use std::time::Duration;

        let mut owner = DeviceCommitOwner::<NeverResource>::new_legacy(
            IncarnationId::first(),
            LifecycleEpochId::first(),
            1,
        );
        let key = ClockKey {
            hardware_crtc: 1,
            epoch: ClockEpochId::first(),
        };
        owner
            .install_clock(key, LifecycleEpochId::first(), 1)
            .unwrap();
        let clock = owner.clock_mut(key).unwrap();
        clock.source = ClockSource::KernelSequence;
        clock.probe = ProbeState::Succeeded;
        clock.reference = Some(0);

        let mut executor = test_support::spawn_stub_helper(StubBehaviour::AcceptQueueWith(70))
            .expect("spawn stub");

        let c = SequenceConsumer(3001);
        let token = owner
            .reserve_arm(key, SequencePurpose::IdleClockWake, 1, &[c])
            .expect("reserve arm");
        owner
            .send_next_sequence_on(&mut executor)
            .expect("send QUEUE");

        // Stage event
        let early_events = owner.apply_sequence_event(
            IncarnationId::first(),
            token.as_user_data(),
            3_000_000,
            70,
            Instant::now(),
        );
        assert!(early_events.is_empty());

        // Cancel consumer
        owner.cancel_consumer(c);

        // Deliver QueueAccepted
        test_support::wait_readable(executor.control_fd().unwrap(), Duration::from_secs(5));
        let reply = executor.poll_reply().expect("poll reply");
        let reply_events = owner.apply_host_call_event(reply);
        assert!(
            reply_events.is_empty(),
            "legacy mode must emit no LegacyClockSample when cancelled"
        );
    }

    /// [ID-1..3, COMMIT-5, CAP-1..4, MULTI]
    /// Cancellation does not hide a contradictory rejection:
    /// an observed event followed by QueueRejected must poison the owner.
    #[test]
    fn cancellation_does_not_hide_contradictory_rejection() {
        use crate::kms::executor::test_support::{self, StubBehaviour};
        use std::time::Duration;

        let (mut owner, key) = setup_owner();
        let mut executor =
            test_support::spawn_stub_helper(StubBehaviour::RejectQueueWith(libc::EINVAL))
                .expect("spawn stub");

        let c = SequenceConsumer(4001);
        let token = owner
            .reserve_arm(key, SequencePurpose::PresentTargetWake, 80, &[c])
            .expect("reserve arm");
        owner
            .send_next_sequence_on(&mut executor)
            .expect("send QUEUE");

        // Stage event before rejection arrives
        let early_events = owner.apply_sequence_event(
            IncarnationId::first(),
            token.as_user_data(),
            4_000_000,
            80,
            Instant::now(),
        );
        assert!(early_events.is_empty());

        // Cancel consumer
        owner.cancel_consumer(c);

        // Deliver QueueRejected
        test_support::wait_readable(executor.control_fd().unwrap(), Duration::from_secs(5));
        let reply = executor.poll_reply().expect("poll reply");
        let reply_events = owner.apply_host_call_event(reply);

        // Contradiction must poison owner despite cancellation
        assert!(
            owner.is_poisoned(),
            "contradictory rejection after event must poison owner even if cancelled"
        );
        assert!(
            reply_events.iter().any(|e| matches!(
                e,
                crate::kms::owner::device::OwnerEvent::MechanismFailed {
                    reason: crate::kms::owner::device::MechanismFailure::ActiveEventContradiction,
                }
            )),
            "MechanismFailed must be emitted upon contradiction"
        );
    }
}
