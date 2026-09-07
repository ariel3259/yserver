use super::{
    build::{BuildError, CommitDescription, build_atomic_request, same_persistent_properties},
    clock::{ClockKey, ClockSource, CrtcClock, LegacyDrainPermit, ProbeState},
    identity::{ClockEpochId, CommitId, EventToken, IdentityAllocator, IncarnationId},
    ledger::{LedgerState, Submitted},
    lifecycle::{ClockProbeId, LifecycleEpochId, LifecycleTransitionId},
    record::{
        CommitRecord, FailureCause, RecordState, RefusalCause, TerminalState, Tombstone,
        UnknownCause,
    },
    sequence::{
        ClockSampleOrigin, MAX_ACTIVE_ARMS, MAX_LOGICAL_CONSUMERS, SequenceArm, SequenceArmPhase,
        SequenceArmToken, SequenceArms, SequenceConsumer, SequenceError, SequencePurpose,
    },
    slot::{ClockProbeLease, DeviceSlot, SlotError, ValidationLease},
};
use crate::kms::executor::{
    HostCallClass, HostCallEvent, HostCallOutcome, HostCallReservation, KmsIoExecutor, SendError,
    UnknownReason,
    protocol::{
        ClockProbeRequest, HostCallCorrelation, HostCallRequest, RequestSeq, SequenceQueueRequest,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Instant,
};

#[derive(Debug)]
pub enum OwnerEvent<R> {
    Dispatched {
        commit: CommitId,
    },
    Accepted {
        commit: CommitId,
    },
    Terminal {
        commit: CommitId,
        terminal: TerminalState,
    },
    ResourcesReleased {
        commit: CommitId,
        resources: Vec<R>,
    },
    ResourcesStillCurrent {
        commit: CommitId,
        resources: Vec<R>,
    },
    Quarantined {
        commit: CommitId,
    },
    ValidationResolved {
        commit: CommitId,
        outcome: ValidationOutcome,
    },
    ClockProbeResolved {
        key: ClockKey,
        outcome: ProbeOutcome,
    },
    StaleReply {
        correlation: HostCallCorrelation,
    },
    ClockSample {
        key: ClockKey,
        sample: crate::kms::owner::clock::ClockSample,
        origin: ClockSampleOrigin,
    },
    LegacyClockSample {
        key: ClockKey,
        sample: crate::kms::owner::clock::ClockSample,
        purpose: SequencePurpose,
    },
    SequenceArmFailed {
        key: ClockKey,
        consumers: BTreeSet<SequenceConsumer>,
        errno: Option<i32>,
    },
    MechanismFailed {
        reason: MechanismFailure,
    },
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
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ValidationOutcome {
    Passed,
    Rejected { errno: i32 },
    Abandoned(UnknownReason),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProbeOutcome {
    Ready { reference: u64 },
    Rejected { errno: i32 },
    Unknown(UnknownReason),
    Contradictory,
}
// crates/yserver/src/kms/owner/device.rs

const TOMBSTONE_RING_CAPACITY: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum DispatchError<R> {
    #[error("slot: {0}")]
    Slot(#[from] SlotError),
    #[error("build: {0}")]
    Build(#[from] BuildError),
    #[error("identity space exhausted within this incarnation")]
    IdentityExhausted,
    #[error("no live record to send")]
    NoLiveRecord,
    #[error("this record's request was already sent")]
    AlreadySent,
    #[error("this description is not the one the outstanding lease validated")]
    ValidationDoesNotMatch,
    #[error("legacy KMS transport is still active")]
    LegacyTransportActive,
    #[error("legacy drain proof does not match the active transport")]
    InvalidLegacyDrainProof,
    #[error("clock identity does not match the current owner context")]
    InvalidCompletionContext,
    #[error("clock is not ready for CRTC {0}")]
    ClockNotReady(u32),
    /// The executor refused before any IPC. Carries the events the caller
    /// must still drain, because the record was terminalized here.
    #[error("executor refused before dispatch: {cause:?}")]
    Refused {
        cause: RefusalCause,
        events: Vec<OwnerEvent<R>>,
    },
}
#[derive(Debug)]
pub struct DeviceCommitOwner<R> {
    slot: DeviceSlot,
    live: Option<CommitRecord<R>>,
    /// A built-but-unsent validation and its lease. A validation installs no
    /// record, so it cannot live in `live`.
    pending_validation: Option<(CommitId, HostCallRequest, ValidationLease)>,
    /// A sent validation awaiting its reply: the commit id and the full
    /// correlation the reply must equal under `ID-3`.
    validation_in_flight: Option<(CommitId, HostCallCorrelation)>,
    /// The description an outstanding lease certifies. `begin_validated`
    /// refuses anything that does not serialize identically to it, so the
    /// lease cannot vouch for a request nobody checked.
    validated_description: Option<(CommitId, CommitDescription)>,
    pending_probe: Option<(ClockKey, ClockProbeId, ClockProbeLease)>,
    probe_in_flight: Option<(ClockKey, ClockProbeId, HostCallCorrelation)>,
    next_probe_id: u64,
    tombstones: VecDeque<Tombstone>,
    identities: IdentityAllocator,
    lifecycle_epoch: LifecycleEpochId,
    transition: Option<LifecycleTransitionId>,
    topology_generation: u64,
    next_seq: u64,
    validation_passed: bool,
    clocks: BTreeMap<ClockKey, CrtcClock>,
    last_clock_epoch: BTreeMap<u32, ClockEpochId>,
    legacy_drain_permit: Option<LegacyDrainPermit>,
    pub(crate) sequence_arms: SequenceArms,
    pub(crate) mechanism_failure: Option<MechanismFailure>,
}

impl<R> DeviceCommitOwner<R> {
    pub fn new(
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        topology_generation: u64,
    ) -> Self {
        Self {
            slot: DeviceSlot::default(),
            live: None,
            pending_validation: None,
            validation_in_flight: None,
            validated_description: None,
            pending_probe: None,
            probe_in_flight: None,
            next_probe_id: 0,
            validation_passed: false,
            tombstones: VecDeque::new(),
            identities: IdentityAllocator::new(incarnation),
            lifecycle_epoch,
            transition: None,
            topology_generation,
            next_seq: 0,
            clocks: BTreeMap::new(),
            last_clock_epoch: BTreeMap::new(),
            legacy_drain_permit: None,
            sequence_arms: SequenceArms::new(),
            mechanism_failure: None,
        }
    }

    pub(crate) fn new_legacy(
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        topology_generation: u64,
    ) -> Self {
        let mut owner = Self::new(incarnation, lifecycle_epoch, topology_generation);
        owner.legacy_drain_permit = Some(LegacyDrainPermit::new(incarnation, lifecycle_epoch));
        owner
    }

    pub fn install_clock(
        &mut self,
        key: ClockKey,
        lifecycle: LifecycleEpochId,
        generation: u64,
    ) -> Result<(), DispatchError<R>> {
        if key.epoch.get() == 0
            || lifecycle != self.lifecycle_epoch
            || generation != self.topology_generation
        {
            return Err(DispatchError::InvalidCompletionContext);
        }
        if let Some(clock) = self.clocks.get(&key) {
            return if clock.lifecycle_epoch == lifecycle && clock.topology_generation == generation
            {
                Ok(())
            } else {
                Err(DispatchError::InvalidCompletionContext)
            };
        }
        if self
            .last_clock_epoch
            .get(&key.hardware_crtc)
            .is_some_and(|last| key.epoch <= *last)
        {
            return Err(DispatchError::InvalidCompletionContext);
        }
        self.clocks
            .retain(|clock_key, _| clock_key.hardware_crtc != key.hardware_crtc);
        self.last_clock_epoch.insert(key.hardware_crtc, key.epoch);
        self.clocks
            .insert(key, CrtcClock::new(key, lifecycle, generation));
        Ok(())
    }

    pub fn clock(&self, key: ClockKey) -> Option<&CrtcClock> {
        self.clocks.get(&key)
    }

    #[doc(hidden)]
    pub fn clock_mut(&mut self, key: ClockKey) -> Option<&mut CrtcClock> {
        self.clocks.get_mut(&key)
    }

    pub fn invalidate_clock(&mut self, key: ClockKey) -> Vec<OwnerEvent<R>> {
        self.clocks.remove(&key);
        if let Some((k, probe_id, lease)) = self.pending_probe.take() {
            if k == key {
                let _ = self.slot.release_probe(probe_id);
            } else {
                self.pending_probe = Some((k, probe_id, lease));
            }
        }
        self.sequence_arms.cancel_matching_clock(key);
        Vec::new()
    }

    pub fn reserve_arm(
        &mut self,
        key: ClockKey,
        purpose: SequencePurpose,
        target: u64,
        consumers: &[SequenceConsumer],
    ) -> Result<SequenceArmToken, SequenceError> {
        if self.is_poisoned() {
            return Err(SequenceError::ClockNotReady);
        }
        let clock = self.clocks.get(&key).ok_or(SequenceError::ClockNotReady)?;
        if self.legacy_drain_permit.is_none()
            && (clock.probe != ProbeState::Succeeded || clock.source != ClockSource::KernelSequence)
        {
            return Err(SequenceError::ClockNotReady);
        }
        if clock.queue_failed {
            return Err(SequenceError::ClockNotReady);
        }

        // Dedup check: (key, purpose, requested_target)
        if let Some(&token) = self.sequence_arms.index.get(&(key, purpose, target))
            && let Some(arm) = self.sequence_arms.arms.get_mut(&token)
            && arm.publishable
        {
            let mut new_consumers = 0;
            for c in consumers {
                if !arm.consumers.contains(c) {
                    new_consumers += 1;
                }
            }
            if self.sequence_arms.total_consumers + new_consumers > MAX_LOGICAL_CONSUMERS {
                return Err(SequenceError::Capacity);
            }
            for c in consumers {
                if arm.consumers.insert(*c) {
                    self.sequence_arms.total_consumers += 1;
                }
            }
            return Ok(token);
        }

        if self.sequence_arms.arms.len() >= MAX_ACTIVE_ARMS {
            return Err(SequenceError::Capacity);
        }
        if self.sequence_arms.total_consumers + consumers.len() > MAX_LOGICAL_CONSUMERS {
            return Err(SequenceError::Capacity);
        }

        let token = self
            .identities
            .checked_next_sequence_arm()
            .ok_or(SequenceError::IdentityExhausted)?;

        let arm = SequenceArm {
            token,
            key,
            lifecycle_epoch: clock.lifecycle_epoch,
            topology_generation: clock.topology_generation,
            purpose,
            requested_target: target,
            scheduled_target: None,
            consumers: consumers.iter().copied().collect(),
            phase: SequenceArmPhase::PendingDispatch,
            publishable: true,
            staged_sample: None,
        };

        self.sequence_arms.total_consumers += arm.consumers.len();
        self.sequence_arms
            .index
            .insert((key, purpose, target), token);
        self.sequence_arms.fifo.push_back(token);
        self.sequence_arms.arms.insert(token, arm);

        Ok(token)
    }

    pub fn send_next_sequence_on(
        &mut self,
        executor: &mut KmsIoExecutor,
    ) -> Result<Vec<OwnerEvent<R>>, DispatchError<R>> {
        if self.is_poisoned() {
            return Err(DispatchError::Refused {
                cause: RefusalCause::BoundaryViolation,
                events: Vec::new(),
            });
        }
        while let Some(token) = self.sequence_arms.fifo.pop_front() {
            let Some(arm) = self.sequence_arms.arms.get_mut(&token) else {
                continue;
            };
            if arm.phase != SequenceArmPhase::PendingDispatch {
                continue;
            }
            match self.slot.acquire_queue(token) {
                Ok(lease) => {
                    let _clock = match self.clocks.get(&arm.key) {
                        Some(c) => c,
                        None => {
                            let _ = self.slot.release_queue(token);
                            return Err(DispatchError::ClockNotReady(arm.key.hardware_crtc));
                        }
                    };
                    let next_seq = match self.next_seq.checked_add(1) {
                        Some(s) => s,
                        None => {
                            let _ = self.slot.release_queue(token);
                            return Err(DispatchError::IdentityExhausted);
                        }
                    };
                    let correlation = HostCallCorrelation::SequenceQueue {
                        seq: RequestSeq::from_raw(next_seq),
                        incarnation: self.identities.incarnation(),
                        lifecycle_epoch: arm.lifecycle_epoch,
                        topology_generation: arm.topology_generation,
                        hardware_crtc: arm.key.hardware_crtc,
                        clock_epoch: arm.key.epoch,
                        token: arm.token,
                    };
                    let (relative, sequence) = match arm.purpose {
                        SequencePurpose::IdleClockWake => (true, 1),
                        SequencePurpose::PresentTargetWake => (false, arm.requested_target),
                    };
                    let request = HostCallRequest::SequenceQueue(SequenceQueueRequest {
                        correlation,
                        relative,
                        sequence,
                    });
                    match executor.send(&request, HostCallReservation::SequenceQueue(lease)) {
                        Ok(()) | Err(SendError::Ipc) => {
                            self.next_seq = next_seq;
                            arm.phase = SequenceArmPhase::InFlight;
                            return Ok(Vec::new());
                        }
                        Err(other) => {
                            let _ = self.slot.release_queue(token);
                            arm.phase = SequenceArmPhase::PendingDispatch;
                            self.sequence_arms.fifo.push_front(token);
                            let cause = Self::refusal_cause(other);
                            return Err(DispatchError::Refused {
                                cause,
                                events: Vec::new(),
                            });
                        }
                    }
                }
                Err(_) => {
                    // Slot is busy with another queue or unresolved atomic/probe.
                    // Put token back at head of FIFO and return without spinning.
                    self.sequence_arms.fifo.push_front(token);
                    return Ok(Vec::new());
                }
            }
        }
        Ok(Vec::new())
    }

    pub fn cancel_consumer(&mut self, consumer: SequenceConsumer) {
        self.sequence_arms.cancel_consumer(consumer);
    }

    pub fn apply_sequence_event(
        &mut self,
        incarnation: IncarnationId,
        token: u64,
        time_ns: i64,
        sequence: u64,
        _now: Instant,
    ) -> Vec<OwnerEvent<R>> {
        if incarnation != self.identities.incarnation() || token == 0 || time_ns < 0 {
            return Vec::new();
        }
        let Some(arm_token) = SequenceArmToken::from_user_data(token) else {
            return Vec::new();
        };
        let Some(arm) = self.sequence_arms.arms.get_mut(&arm_token) else {
            return Vec::new();
        };
        let clock = match self.clocks.get_mut(&arm.key) {
            Some(c) => c,
            None => return Vec::new(),
        };
        if arm.key.epoch != clock.key.epoch
            || arm.lifecycle_epoch != clock.lifecycle_epoch
            || arm.topology_generation != clock.topology_generation
        {
            return Vec::new();
        }

        let ust = time_ns as u64;
        let sample = crate::kms::owner::clock::ClockSample { msc: sequence, ust };

        if arm.phase == SequenceArmPhase::InFlight {
            if arm.staged_sample.is_none() {
                arm.staged_sample = Some(sample);
            }
            return Vec::new();
        }

        let publishable = arm.publishable;
        let purpose = arm.purpose;
        let key = arm.key;
        let consumers_len = arm.consumers.len();
        let target = arm.requested_target;

        self.sequence_arms.arms.remove(&arm_token);
        self.sequence_arms.index.remove(&(key, purpose, target));
        self.sequence_arms.total_consumers = self
            .sequence_arms
            .total_consumers
            .saturating_sub(consumers_len);
        self.sequence_arms.fifo.retain(|t| *t != arm_token);
        self.sequence_arms.push_tombstone(arm_token);

        if !publishable {
            return Vec::new();
        }

        if self.legacy_drain_permit.is_some() {
            vec![OwnerEvent::LegacyClockSample {
                key,
                sample,
                purpose,
            }]
        } else {
            if let Some(ref mut r) = clock.reference
                && sequence > *r
            {
                *r = sequence;
            }
            clock.observe(sample);
            vec![OwnerEvent::ClockSample {
                key,
                sample,
                origin: ClockSampleOrigin::Sequence(purpose),
            }]
        }
    }

    pub fn apply_drm_event(
        &mut self,
        incarnation: IncarnationId,
        event: crate::drm::event_stream::DrmEventRecord,
        now: Instant,
    ) -> Vec<OwnerEvent<R>> {
        if incarnation != self.identities.incarnation() {
            return Vec::new();
        }
        let user_data = match &event {
            crate::drm::event_stream::DrmEventRecord::PageFlip { user_data, .. }
            | crate::drm::event_stream::DrmEventRecord::Vblank { user_data, .. }
            | crate::drm::event_stream::DrmEventRecord::CrtcSequence { user_data, .. } => {
                *user_data
            }
        };
        if user_data == 0 {
            return Vec::new();
        }
        let Some(token) = SequenceArmToken::from_user_data(user_data) else {
            return Vec::new();
        };

        if self.sequence_arms.arms.contains_key(&token)
            && !matches!(
                event,
                crate::drm::event_stream::DrmEventRecord::CrtcSequence { .. }
            )
        {
            self.mechanism_failure = Some(MechanismFailure::ActiveEventContradiction);
            return vec![OwnerEvent::MechanismFailed {
                reason: MechanismFailure::ActiveEventContradiction,
            }];
        }

        match event {
            crate::drm::event_stream::DrmEventRecord::CrtcSequence {
                sequence,
                time_ns,
                user_data,
                ..
            } => self.apply_sequence_event(incarnation, user_data, time_ns, sequence, now),
            _ => Vec::new(),
        }
    }

    pub fn is_poisoned(&self) -> bool {
        self.mechanism_failure.is_some()
    }

    pub(crate) fn clock_context(&self) -> (LifecycleEpochId, u64) {
        (self.lifecycle_epoch, self.topology_generation)
    }

    #[allow(dead_code)] // Task 7 supplies the only checked production proof issuer.
    pub(crate) fn finish_legacy_transport(
        &mut self,
        proof: crate::kms::render::platform::LegacyDrained,
    ) -> Result<(), DispatchError<R>> {
        let Some(permit) = self.legacy_drain_permit.as_ref() else {
            return Err(DispatchError::InvalidLegacyDrainProof);
        };
        if permit.incarnation != self.identities.incarnation()
            || permit.lifecycle != self.lifecycle_epoch
            || !proof.matches(permit.incarnation, permit.lifecycle)
        {
            return Err(DispatchError::InvalidLegacyDrainProof);
        }
        self.clocks.clear();
        self.legacy_drain_permit = None;
        Ok(())
    }

    fn next_correlation(
        &mut self,
    ) -> Result<(CommitId, EventToken, HostCallCorrelation), DispatchError<R>> {
        let commit = self
            .identities
            .checked_next_commit()
            .ok_or(DispatchError::IdentityExhausted)?;
        let event_token = self
            .identities
            .checked_next_event_token()
            .ok_or(DispatchError::IdentityExhausted)?;
        self.next_seq = self
            .next_seq
            .checked_add(1)
            .ok_or(DispatchError::IdentityExhausted)?;
        Ok((
            commit,
            event_token,
            HostCallCorrelation::Atomic {
                seq: RequestSeq::from_raw(self.next_seq), // production, not for_tests
                incarnation: self.identities.incarnation(),
                lifecycle_epoch: self.lifecycle_epoch,
                transition: self.transition,
                commit,
                event_token,
            },
        ))
    }

    /// Install the record and reserve the slot. No IPC happens here.
    /// Building precedes reserving, so a description that cannot produce a
    /// valid request never consumes the slot.
    pub fn begin(
        &mut self,
        desc: &CommitDescription,
        ledger: Submitted<R>,
    ) -> Result<(CommitId, Vec<OwnerEvent<R>>), DispatchError<R>> {
        if self.legacy_drain_permit.is_some() {
            return Err(DispatchError::LegacyTransportActive);
        }
        let (commit, event_token, correlation) = self.next_correlation()?;
        let (request, closure) =
            build_atomic_request(desc, correlation, HostCallClass::SeatActiveNonblock)?;
        for &crtc in closure.kernel_event() {
            let clock = self.clocks.values().find(|c| c.key.hardware_crtc == crtc);
            match clock {
                Some(c) if c.source == ClockSource::KernelSequence && !c.queue_failed => {}
                _ => return Err(DispatchError::ClockNotReady(crtc)),
            }
        }
        let proof = self.slot.reserve(commit)?;
        let mut record = CommitRecord::new(
            commit,
            event_token,
            self.identities.incarnation(),
            self.lifecycle_epoch,
            self.transition,
            self.topology_generation,
            closure,
            correlation,
            ledger,
        );
        record.attach_request(HostCallRequest::Atomic(request), proof);
        self.live = Some(record);
        Ok((commit, Vec::new()))
    }

    /// Send the request `begin` built.
    ///
    /// A refusal from `send` before it installs `InFlight` — `Reaped`,
    /// `Stalled`, `AlreadyInFlight`, `ReservationMismatch`,
    /// `BoundaryViolation` — means no IPC crossed the uncertainty boundary
    /// and no terminal event was queued, so the record is `NeverDispatched`
    /// and the slot is released here. Only `SendError::Ipc` means the write
    /// was attempted, and 2a has already queued the terminal event for it.
    pub fn send_on(
        &mut self,
        executor: &mut KmsIoExecutor,
    ) -> Result<Vec<OwnerEvent<R>>, DispatchError<R>> {
        let record = self.live.as_mut().ok_or(DispatchError::NoLiveRecord)?;
        let commit = record.commit_id();
        let (request, proof) = record.take_request().ok_or(DispatchError::AlreadySent)?;
        match executor.send(&request, HostCallReservation::Submitting(proof)) {
            Ok(()) => {
                record.mark_dispatched();
                Ok(vec![OwnerEvent::Dispatched { commit }])
            }
            Err(SendError::Ipc) => {
                // The write was attempted; 2a queued a terminal event that
                // `apply_host_call_event` will deliver. Dispatched is correct.
                record.mark_dispatched();
                Ok(vec![OwnerEvent::Dispatched { commit }])
            }
            Err(other) => {
                let cause = match other {
                    SendError::Reaped => RefusalCause::Reaped,
                    SendError::Stalled => RefusalCause::Stalled,
                    SendError::AlreadyInFlight => RefusalCause::AlreadyInFlight,
                    SendError::ReservationMismatch => RefusalCause::ReservationMismatch,
                    SendError::BoundaryViolation => RefusalCause::BoundaryViolation,
                    SendError::Ipc => unreachable!("handled above"),
                };
                let terminal =
                    TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(cause));
                record.terminalize(terminal);
                let events = self.retire_live(commit, terminal);
                Err(DispatchError::Refused { cause, events })
            }
        }
    }

    pub fn dispatch(
        &mut self,
        desc: &CommitDescription,
        ledger: Submitted<R>,
        executor: &mut KmsIoExecutor,
    ) -> Result<(CommitId, Vec<OwnerEvent<R>>), DispatchError<R>> {
        let (commit, mut events) = self.begin(desc, ledger)?;
        events.extend(self.send_on(executor)?);
        Ok((commit, events))
    }

    /// Map a pre-install `SendError` to the refusal it represents. `Ipc` is
    /// deliberately absent: it is the one variant meaning the write was
    /// attempted, so it is acceptance-unknown rather than a refusal, and both
    /// send paths handle it before reaching here.
    fn refusal_cause(err: SendError) -> RefusalCause {
        match err {
            SendError::Reaped => RefusalCause::Reaped,
            SendError::Stalled => RefusalCause::Stalled,
            SendError::AlreadyInFlight => RefusalCause::AlreadyInFlight,
            SendError::ReservationMismatch => RefusalCause::ReservationMismatch,
            SendError::BoundaryViolation => RefusalCause::BoundaryViolation,
            SendError::Ipc => unreachable!("handled by the caller"),
        }
    }

    /// Proceed from a passed validation to the live call it validated.
    ///
    /// Refuses unless `desc` serializes identically to the description the
    /// outstanding lease was taken for: a lease that certified one request
    /// must not admit another. On success the lease becomes the slot
    /// reservation in one step, so nothing can be admitted in between.
    pub fn begin_validated(
        &mut self,
        desc: &CommitDescription,
        ledger: Submitted<R>,
    ) -> Result<(CommitId, Vec<OwnerEvent<R>>), DispatchError<R>> {
        if self.legacy_drain_permit.is_some() {
            return Err(DispatchError::LegacyTransportActive);
        }
        if !self.validation_passed {
            return Err(DispatchError::ValidationDoesNotMatch);
        }
        let (lease_commit, validated) = self
            .validated_description
            .as_ref()
            .ok_or(DispatchError::ValidationDoesNotMatch)?;
        let (lease_commit, validated) = (*lease_commit, validated.clone());
        let (commit, event_token, correlation) = self.next_correlation()?;
        let (request, closure) =
            build_atomic_request(desc, correlation, HostCallClass::SeatActiveNonblock)?;
        let (reference, _) =
            build_atomic_request(&validated, correlation, HostCallClass::SeatActiveValidation)?;
        // Compare the serialized persistent properties, not the descriptions:
        // the live request legitimately differs from the validation by its
        // out-fence entries and its flags, and by nothing else.
        if !same_persistent_properties(&request, &reference, desc.property_ids.out_fence_ptr) {
            return Err(DispatchError::ValidationDoesNotMatch);
        }
        for &crtc in closure.kernel_event() {
            let clock = self.clocks.values().find(|c| c.key.hardware_crtc == crtc);
            match clock {
                Some(c) if c.source == ClockSource::KernelSequence && !c.queue_failed => {}
                _ => return Err(DispatchError::ClockNotReady(crtc)),
            }
        }
        let proof = self.slot.consume_validation(lease_commit, commit)?;
        self.validated_description = None;
        self.validation_passed = false;
        let mut record = CommitRecord::new(
            commit,
            event_token,
            self.identities.incarnation(),
            self.lifecycle_epoch,
            self.transition,
            self.topology_generation,
            closure,
            correlation,
            ledger,
        );
        record.attach_request(HostCallRequest::Atomic(request), proof);
        self.live = Some(record);
        Ok((commit, Vec::new()))
    }

    /// End the exclusive interval without proceeding: a failed or abandoned
    /// validation, or a caller that decides not to submit.
    pub fn abandon_validation(&mut self, commit: CommitId) -> Result<(), DispatchError<R>> {
        self.slot.abandon_validation(commit)?;
        self.pending_validation = None;
        self.validation_in_flight = None;
        self.validated_description = None;
        self.validation_passed = false;
        Ok(())
    }

    /// Send the validation `begin_validation` built. Mirrors `send_on`:
    /// the stored lease moves into `HostCallReservation::Validation`, a
    /// pre-IPC refusal releases the lease and clears `pending_validation`
    /// (nothing crossed the boundary, so nothing is uncertain), a
    /// `SendError::Ipc` keeps both — 2a queued a terminal event that
    /// `apply_host_call_event` will deliver under the stored correlation —
    /// and a second call finds the lease already taken and returns
    /// `DispatchError::AlreadySent`.
    ///
    /// The lease is **not** released on a successful send: it is released by
    /// `consume_validation` at the live call, or by `abandon_validation`.
    pub fn send_validation_on(
        &mut self,
        executor: &mut KmsIoExecutor,
    ) -> Result<Vec<OwnerEvent<R>>, DispatchError<R>> {
        let (commit, request, lease) = self
            .pending_validation
            .take()
            .ok_or(DispatchError::AlreadySent)?;
        let correlation = request.correlation();
        match executor.send(&request, HostCallReservation::Validation(lease)) {
            Ok(()) | Err(SendError::Ipc) => {
                // The lease moved into the executor; the owner keeps the
                // correlation so the reply can be matched under ID-3.
                self.validation_in_flight = Some((commit, correlation));
                Ok(vec![OwnerEvent::Dispatched { commit }])
            }
            Err(other) => {
                let cause = Self::refusal_cause(other);
                self.abandon_validation(commit)?;
                Err(DispatchError::Refused {
                    cause,
                    events: Vec::new(),
                })
            }
        }
    }

    /// A `TEST_ONLY` request. Takes the exclusive validation lease, installs
    /// no record, never touches the commit slot.
    pub fn begin_validation(
        &mut self,
        desc: &CommitDescription,
    ) -> Result<CommitId, DispatchError<R>> {
        if self.legacy_drain_permit.is_some() {
            return Err(DispatchError::LegacyTransportActive);
        }
        let (commit, _token, correlation) = self.next_correlation()?;
        let (request, _closure) =
            build_atomic_request(desc, correlation, HostCallClass::SeatActiveValidation)?;
        let lease = self.slot.acquire_validation(commit)?;
        self.pending_validation = Some((commit, HostCallRequest::Atomic(request), lease));
        // Recorded here, at the moment the lease is taken, so `begin_validated`
        // has something to compare against. A lease with no recorded
        // description could vouch for anything.
        self.validated_description = Some((commit, desc.clone()));
        Ok(commit)
    }

    pub fn begin_clock_probe(&mut self, key: ClockKey) -> Result<ClockProbeId, DispatchError<R>> {
        if self.legacy_drain_permit.is_some() {
            return Err(DispatchError::LegacyTransportActive);
        }
        let clock = self
            .clocks
            .get(&key)
            .ok_or(DispatchError::ClockNotReady(key.hardware_crtc))?;
        if clock.probe == ProbeState::Failed || clock.probe != ProbeState::NotStarted {
            return Err(DispatchError::ClockNotReady(key.hardware_crtc));
        }
        if self.pending_probe.is_some() || self.probe_in_flight.is_some() {
            return Err(DispatchError::Refused {
                cause: RefusalCause::AlreadyInFlight,
                events: Vec::new(),
            });
        }
        let next_id = self
            .next_probe_id
            .checked_add(1)
            .ok_or(DispatchError::IdentityExhausted)?;
        let probe_id = ClockProbeId::from_raw(next_id);
        let lease = self.slot.acquire_probe(probe_id)?;
        self.next_probe_id = next_id;
        self.pending_probe = Some((key, probe_id, lease));
        Ok(probe_id)
    }

    pub fn send_clock_probe_on(
        &mut self,
        executor: &mut KmsIoExecutor,
    ) -> Result<Vec<OwnerEvent<R>>, DispatchError<R>> {
        let (key, probe_id, lease) = self
            .pending_probe
            .take()
            .ok_or(DispatchError::AlreadySent)?;
        let clock = self
            .clocks
            .get(&key)
            .ok_or(DispatchError::ClockNotReady(key.hardware_crtc))?;
        let next_seq = self
            .next_seq
            .checked_add(1)
            .ok_or(DispatchError::IdentityExhausted)?;
        let correlation = HostCallCorrelation::ClockProbe {
            seq: RequestSeq::from_raw(next_seq),
            incarnation: self.identities.incarnation(),
            lifecycle_epoch: clock.lifecycle_epoch,
            topology_generation: clock.topology_generation,
            hardware_crtc: key.hardware_crtc,
            clock_epoch: key.epoch,
            probe: probe_id,
        };
        let request = HostCallRequest::ClockProbe(ClockProbeRequest { correlation });
        match executor.send(&request, HostCallReservation::ClockProbe(lease)) {
            Ok(()) | Err(SendError::Ipc) => {
                self.next_seq = next_seq;
                self.probe_in_flight = Some((key, probe_id, correlation));
                if let Some(c) = self.clocks.get_mut(&key) {
                    c.probe = ProbeState::InFlight(probe_id);
                }
                Ok(Vec::new())
            }
            Err(other) => {
                let _ = self.slot.release_probe(probe_id);
                let cause = Self::refusal_cause(other);
                Err(DispatchError::Refused {
                    cause,
                    events: Vec::new(),
                })
            }
        }
    }

    pub fn apply_host_call_event(&mut self, event: HostCallEvent) -> Vec<OwnerEvent<R>> {
        let (correlation, outcome, late) = match event {
            HostCallEvent::Outcome {
                correlation,
                outcome,
            } => (correlation, outcome, false),
            HostCallEvent::LateReply {
                correlation,
                outcome,
            } => (correlation, outcome, true),
        };

        if late {
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        }
        if let HostCallCorrelation::ClockProbe { .. } = correlation {
            return self.resolve_clock_probe(correlation, outcome);
        }
        if let HostCallCorrelation::SequenceQueue { .. } = correlation {
            return self.resolve_sequence_queue(correlation, outcome);
        }
        let HostCallCorrelation::Atomic { commit, .. } = correlation else {
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        };
        if self
            .validation_in_flight
            .is_some_and(|(_, c)| c == correlation)
        {
            return self.resolve_validation(commit, outcome);
        }
        if !self.is_current(&correlation) {
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        }
        let Some(record) = self.live.as_mut() else {
            return Vec::new();
        };
        if matches!(record.state(), RecordState::Terminal(_)) {
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        }
        self.apply_to_live(commit, outcome)
    }
}

impl<R> DispatchError<R> {
    pub fn into_events(self) -> Vec<OwnerEvent<R>> {
        match self {
            Self::Refused { events, .. } => events,
            _ => Vec::new(),
        }
    }
}
impl<R> DeviceCommitOwner<R> {
    pub fn slot(&self) -> &DeviceSlot {
        &self.slot
    }
    pub fn live_record(&self) -> Option<&CommitRecord<R>> {
        self.live.as_ref()
    }
    pub fn sequence_arms(&self) -> &SequenceArms {
        &self.sequence_arms
    }
    pub fn tombstones(&self) -> &VecDeque<Tombstone> {
        &self.tombstones
    }
    #[doc(hidden)]
    pub fn mark_dispatched_for_tests(&mut self) {
        let record = self.live.as_mut().expect("record");
        record.take_request();
        record.mark_dispatched();
    }
    #[doc(hidden)]
    pub fn mark_probe_dispatched_for_tests(&mut self) {
        let (key, probe_id, _) = self.pending_probe.take().expect("pending probe");
        let clock = self.clocks.get(&key).expect("clock");
        self.next_seq = self.next_seq.checked_add(1).expect("seq");
        let correlation = HostCallCorrelation::ClockProbe {
            seq: RequestSeq::from_raw(self.next_seq),
            incarnation: self.identities.incarnation(),
            lifecycle_epoch: clock.lifecycle_epoch,
            topology_generation: clock.topology_generation,
            hardware_crtc: key.hardware_crtc,
            clock_epoch: key.epoch,
            probe: probe_id,
        };
        self.probe_in_flight = Some((key, probe_id, correlation));
        if let Some(c) = self.clocks.get_mut(&key) {
            c.probe = ProbeState::InFlight(probe_id);
        }
    }
    #[doc(hidden)]
    pub fn mark_validation_dispatched_for_tests(&mut self) {
        let (commit, request, _) = self.pending_validation.take().expect("validation");
        self.validation_in_flight = Some((commit, request.correlation()));
    }
    fn resolve_clock_probe(
        &mut self,
        correlation: HostCallCorrelation,
        outcome: HostCallOutcome,
    ) -> Vec<OwnerEvent<R>> {
        let Some((key, probe_id, expected_correlation)) = self.probe_in_flight.take() else {
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        };
        if correlation != expected_correlation {
            self.probe_in_flight = Some((key, probe_id, expected_correlation));
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        }
        match outcome {
            HostCallOutcome::ProbeAccepted { sequence, .. } => {
                let _ = self.slot.release_probe(probe_id);
                if let Some(clock) = self.clocks.get_mut(&key) {
                    clock.install_reference(sequence);
                }
                vec![OwnerEvent::ClockProbeResolved {
                    key,
                    outcome: ProbeOutcome::Ready {
                        reference: sequence,
                    },
                }]
            }
            HostCallOutcome::Rejected {
                errno,
                unexpected_fence_output,
                ..
            } => {
                let _ = self.slot.release_probe(probe_id);
                if let Some(clock) = self.clocks.get_mut(&key) {
                    clock.probe = ProbeState::Failed;
                    clock.source = ClockSource::Unresolved;
                }
                let outcome = if unexpected_fence_output {
                    ProbeOutcome::Contradictory
                } else {
                    ProbeOutcome::Rejected { errno }
                };
                vec![OwnerEvent::ClockProbeResolved { key, outcome }]
            }
            HostCallOutcome::Unknown(reason) => {
                if let Some(clock) = self.clocks.get_mut(&key) {
                    clock.probe = ProbeState::Failed;
                }
                vec![OwnerEvent::ClockProbeResolved {
                    key,
                    outcome: ProbeOutcome::Unknown(reason),
                }]
            }
            _ => {
                let _ = self.slot.release_probe(probe_id);
                if let Some(clock) = self.clocks.get_mut(&key) {
                    clock.probe = ProbeState::Failed;
                    clock.source = ClockSource::Unresolved;
                }
                vec![OwnerEvent::ClockProbeResolved {
                    key,
                    outcome: ProbeOutcome::Contradictory,
                }]
            }
        }
    }

    fn resolve_sequence_queue(
        &mut self,
        correlation: HostCallCorrelation,
        outcome: HostCallOutcome,
    ) -> Vec<OwnerEvent<R>> {
        let HostCallCorrelation::SequenceQueue {
            seq: _,
            incarnation,
            lifecycle_epoch,
            topology_generation,
            hardware_crtc,
            clock_epoch,
            token,
        } = correlation
        else {
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        };

        if incarnation != self.identities.incarnation() {
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        }

        if self.slot.queue_outstanding() != Some(token) {
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        }

        let Some(arm) = self.sequence_arms.arms.get_mut(&token) else {
            let _ = self.slot.release_queue(token);
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        };

        if arm.key.hardware_crtc != hardware_crtc
            || arm.key.epoch != clock_epoch
            || arm.lifecycle_epoch != lifecycle_epoch
            || arm.topology_generation != topology_generation
        {
            let _ = self.slot.release_queue(token);
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        }

        match outcome {
            HostCallOutcome::QueueAccepted { sequence, .. } => {
                let _ = self.slot.release_queue(token);
                arm.scheduled_target = Some(sequence);
                let staged = arm.staged_sample.take();
                let publishable = arm.publishable;
                let purpose = arm.purpose;
                let key = arm.key;
                let target = arm.requested_target;
                let consumers_len = arm.consumers.len();

                if let Some(sample) = staged {
                    self.sequence_arms.arms.remove(&token);
                    self.sequence_arms.index.remove(&(key, purpose, target));
                    self.sequence_arms.total_consumers = self
                        .sequence_arms
                        .total_consumers
                        .saturating_sub(consumers_len);
                    self.sequence_arms.push_tombstone(token);

                    if !publishable {
                        return Vec::new();
                    }

                    let clock = match self.clocks.get_mut(&key) {
                        Some(c) => c,
                        None => return Vec::new(),
                    };

                    if self.legacy_drain_permit.is_some() {
                        vec![OwnerEvent::LegacyClockSample {
                            key,
                            sample,
                            purpose,
                        }]
                    } else {
                        if let Some(ref mut r) = clock.reference
                            && sample.msc > *r
                        {
                            *r = sample.msc;
                        }
                        clock.observe(sample);
                        vec![OwnerEvent::ClockSample {
                            key,
                            sample,
                            origin: ClockSampleOrigin::Sequence(purpose),
                        }]
                    }
                } else if !publishable {
                    self.sequence_arms.arms.remove(&token);
                    self.sequence_arms.index.remove(&(key, purpose, target));
                    self.sequence_arms.total_consumers = self
                        .sequence_arms
                        .total_consumers
                        .saturating_sub(consumers_len);
                    self.sequence_arms.push_tombstone(token);
                    Vec::new()
                } else {
                    arm.phase = SequenceArmPhase::Armed;
                    Vec::new()
                }
            }
            HostCallOutcome::Rejected { errno, .. } => {
                let _ = self.slot.release_queue(token);
                let had_staged = arm.staged_sample.is_some();
                let key = arm.key;
                let consumers = arm.consumers.clone();
                let target = arm.requested_target;
                let purpose = arm.purpose;
                let consumers_len = arm.consumers.len();
                let publishable = arm.publishable;

                self.sequence_arms.arms.remove(&token);
                self.sequence_arms.index.remove(&(key, purpose, target));
                self.sequence_arms.total_consumers = self
                    .sequence_arms
                    .total_consumers
                    .saturating_sub(consumers_len);
                self.sequence_arms.push_tombstone(token);

                if had_staged {
                    self.mechanism_failure = Some(MechanismFailure::ActiveEventContradiction);
                    return vec![OwnerEvent::MechanismFailed {
                        reason: MechanismFailure::ActiveEventContradiction,
                    }];
                }

                if let Some(clock) = self.clocks.get_mut(&key) {
                    clock.queue_failed = true;
                }

                if publishable && !consumers.is_empty() {
                    vec![OwnerEvent::SequenceArmFailed {
                        key,
                        consumers,
                        errno: Some(errno),
                    }]
                } else {
                    Vec::new()
                }
            }
            HostCallOutcome::Unknown(_reason) => {
                // Unknown retains the lease and closes readiness
                self.mechanism_failure = Some(MechanismFailure::HostCallUnknown);
                vec![OwnerEvent::MechanismFailed {
                    reason: MechanismFailure::HostCallUnknown,
                }]
            }
            _ => {
                let _ = self.slot.release_queue(token);
                self.mechanism_failure = Some(MechanismFailure::HostCallUnknown);
                vec![OwnerEvent::MechanismFailed {
                    reason: MechanismFailure::HostCallUnknown,
                }]
            }
        }
    }
    fn is_current(&self, correlation: &HostCallCorrelation) -> bool {
        self.live
            .as_ref()
            .is_some_and(|r| r.correlation() == correlation && r.milestones().dispatched)
    }
    fn adopt_and_close(outcome: HostCallOutcome) {
        drop(outcome);
    }
    fn resolve_validation(
        &mut self,
        commit: CommitId,
        outcome: HostCallOutcome,
    ) -> Vec<OwnerEvent<R>> {
        let outcome = match outcome {
            HostCallOutcome::Accepted {
                out_fence_mask: 0,
                ref out_fences,
                ..
            } if out_fences.is_empty() => ValidationOutcome::Passed,
            HostCallOutcome::Rejected { errno, .. } => ValidationOutcome::Rejected { errno },
            HostCallOutcome::Unknown(r) | HostCallOutcome::ValidationAbandoned(r) => {
                ValidationOutcome::Abandoned(r)
            }
            _ => ValidationOutcome::Abandoned(UnknownReason::MalformedReply),
        };
        self.validation_in_flight = None;
        self.validation_passed = outcome == ValidationOutcome::Passed;
        vec![OwnerEvent::ValidationResolved { commit, outcome }]
    }
    fn push_tombstone(&mut self, tombstone: Tombstone) {
        self.tombstones.push_back(tombstone);
        if self.tombstones.len() > TOMBSTONE_RING_CAPACITY {
            self.tombstones.pop_front();
        }
    }
    fn retire_live(&mut self, commit: CommitId, terminal: TerminalState) -> Vec<OwnerEvent<R>> {
        let mut record = self.live.take().expect("live record");
        let ledger = record.take_rejected_ledger();
        let mut events = vec![OwnerEvent::Terminal { commit, terminal }];
        match ledger {
            LedgerState::Submitted(s) => {
                let (old, resources) = s.rejected();
                events.push(OwnerEvent::ResourcesReleased { commit, resources });
                events.push(OwnerEvent::ResourcesStillCurrent {
                    commit,
                    resources: old.into_current(),
                });
            }
            LedgerState::Rejected(old) => events.push(OwnerEvent::ResourcesStillCurrent {
                commit,
                resources: old.into_current(),
            }),
            _ => unreachable!("only a proven refusal or rejection retires"),
        }
        self.push_tombstone(record.tombstone().expect("terminal"));
        self.slot.release(commit).expect("reserved slot");
        events
    }
    fn apply_to_live(&mut self, commit: CommitId, outcome: HostCallOutcome) -> Vec<OwnerEvent<R>> {
        let record = self.live.as_mut().expect("live");
        // A second outcome cannot reverse an already consumed acceptance.
        if record.milestones().accepted {
            return vec![OwnerEvent::StaleReply {
                correlation: *record.correlation(),
            }];
        }
        let cause = match outcome {
            HostCallOutcome::Accepted {
                out_fence_mask,
                out_fences,
                ..
            } => {
                let expected = record.closure().expected_completion().len();
                let returned = out_fence_mask.count_ones() as usize;
                // The actual builder slot table is retained on the record.
                record.adopt_returned_fences(out_fence_mask, out_fences);
                if returned == expected {
                    let _ = self.slot.resolve_atomic_reply(commit);
                    record.mark_accepted();
                    return vec![OwnerEvent::Accepted { commit }];
                }
                UnknownCause::IncompleteFenceOutput { expected, returned }
            }
            HostCallOutcome::Rejected { errno, .. } => {
                let resources = record.terminalize_rejected(errno);
                let terminal = *record.state();
                let RecordState::Terminal(terminal) = terminal else {
                    unreachable!()
                };
                let mut events = vec![OwnerEvent::ResourcesReleased { commit, resources }];
                events.extend(self.retire_live(commit, terminal));
                return events;
            }
            HostCallOutcome::Unknown(reason) => UnknownCause::HostCall(reason),
            HostCallOutcome::ValidationAbandoned(_)
            | HostCallOutcome::ProbeAccepted { .. }
            | HostCallOutcome::QueueAccepted { .. } => UnknownCause::ContradictoryEvidence,
        };
        let terminal = TerminalState::CompletionUnknown(cause);
        record.terminalize(terminal);
        let tombstone = record.tombstone().expect("terminal");
        self.push_tombstone(tombstone);
        vec![
            OwnerEvent::Terminal { commit, terminal },
            OwnerEvent::Quarantined { commit },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::owner::{
        clock::{ClockSample, ClockSource},
        test_fixtures::*,
    };

    /// [ID-1..3, CAP-1..4, COMMIT-2] Catches device-wide capability state
    /// leaking between hardware CRTCs in one epoch.
    #[test]
    fn clock_rows_isolate_two_crtcs_on_one_device() {
        let mut owner = owner_for_tests();
        let epoch = ClockEpochId::first();
        let first = ClockKey {
            hardware_crtc: 7,
            epoch,
        };
        let second = ClockKey {
            hardware_crtc: 8,
            epoch,
        };
        owner
            .install_clock(first, LifecycleEpochId::first(), 1)
            .unwrap();
        owner
            .install_clock(second, LifecycleEpochId::first(), 1)
            .unwrap();
        owner.clock_mut(first).unwrap().queue_failed = true;
        assert!(owner.clock(first).unwrap().queue_failed);
        assert!(!owner.clock(second).unwrap().queue_failed);
    }

    /// [ID-1..3, CAP-1..4, COMMIT-2] Catches source/reference/sample state
    /// surviving replacement by a newer epoch on the same hardware CRTC.
    #[test]
    fn newer_epoch_resets_the_clock_and_old_epoch_cannot_be_reused() {
        let mut owner = owner_for_tests();
        let old = ClockKey {
            hardware_crtc: 7,
            epoch: ClockEpochId::first(),
        };
        owner
            .install_clock(old, LifecycleEpochId::first(), 1)
            .unwrap();
        let clock = owner.clock_mut(old).unwrap();
        clock.install_reference(9);
        clock.observe(ClockSample { msc: 10, ust: 100 });

        let new = ClockKey {
            hardware_crtc: 7,
            epoch: ClockEpochId::first().next(),
        };
        owner
            .install_clock(new, LifecycleEpochId::first(), 1)
            .unwrap();
        assert!(owner.clock(old).is_none());
        let clock = owner.clock(new).unwrap();
        assert_eq!(clock.source, ClockSource::Unresolved);
        assert_eq!(clock.reference, None);
        assert_eq!(clock.latest, None);
        assert!(matches!(
            owner.install_clock(old, LifecycleEpochId::first(), 1),
            Err(DispatchError::InvalidCompletionContext)
        ));
    }

    /// [ID-1..3, CAP-1..4, COMMIT-2] Catches legacy construction opening
    /// owner submissions or consuming identities before handover.
    #[test]
    fn legacy_permit_refuses_owner_work_before_allocating_identity() {
        let mut owner =
            DeviceCommitOwner::new_legacy(IncarnationId::first(), LifecycleEpochId::first(), 1);
        assert!(matches!(
            owner.begin(&single_active_crtc(), ledger()),
            Err(DispatchError::LegacyTransportActive)
        ));
        assert!(matches!(
            owner.begin_validation(&single_active_crtc()),
            Err(DispatchError::LegacyTransportActive)
        ));
        assert!(owner.live_record().is_none());
        assert!(owner.slot().validation_outstanding().is_none());
    }

    #[test]
    fn a_validation_must_pass_before_its_lease_can_be_consumed() {
        let mut o = owner_for_tests();
        let desc = single_active_crtc();
        let c = o.begin_validation(&desc).unwrap();
        assert!(matches!(
            o.begin_validated(&desc, ledger()),
            Err(DispatchError::ValidationDoesNotMatch)
        ));
        o.mark_validation_dispatched_for_tests();
        o.apply_host_call_event(rejected(c, libc::EINVAL));
        assert!(matches!(
            o.begin_validated(&desc, ledger()),
            Err(DispatchError::ValidationDoesNotMatch)
        ));
        assert_eq!(o.slot().validation_outstanding(), Some(c));
    }

    #[test]
    fn validation_ignores_every_mismatched_identity_and_late_reply() {
        for field in 0..6 {
            let mut o = owner_for_tests();
            let c = o.begin_validation(&single_active_crtc()).unwrap();
            o.mark_validation_dispatched_for_tests();
            let mut event = accepted(c, 0, 0);
            let HostCallEvent::Outcome {
                correlation:
                    HostCallCorrelation::Atomic {
                        seq,
                        incarnation,
                        lifecycle_epoch,
                        transition,
                        commit,
                        event_token,
                    },
                ..
            } = &mut event
            else {
                unreachable!()
            };
            match field {
                0 => *seq = RequestSeq::from_raw(999),
                1 => *incarnation = IncarnationId::from_raw(999),
                2 => *lifecycle_epoch = LifecycleEpochId::from_raw(999),
                3 => *transition = Some(LifecycleTransitionId::from_raw(1)),
                4 => *commit = CommitId::for_tests(999),
                _ => *event_token = EventToken::for_tests(999),
            }
            assert!(matches!(
                o.apply_host_call_event(event).as_slice(),
                [OwnerEvent::StaleReply { .. }]
            ));
            assert!(matches!(
                o.apply_host_call_event(late_accepted(c, 0, 0)).as_slice(),
                [OwnerEvent::StaleReply { .. }]
            ));
            assert!(!o.validation_passed);
            assert!(o.validation_in_flight.is_some());
            assert!(matches!(
                o.apply_host_call_event(accepted(c, 0, 0)).as_slice(),
                [OwnerEvent::ValidationResolved {
                    outcome: ValidationOutcome::Passed,
                    ..
                }]
            ));
        }
    }

    #[test]
    fn an_unsent_validation_cannot_be_resolved_by_a_reply() {
        let mut o = owner_for_tests();
        let c = o.begin_validation(&single_active_crtc()).unwrap();
        assert!(matches!(
            o.apply_host_call_event(accepted(c, 0, 0)).as_slice(),
            [OwnerEvent::StaleReply { .. }]
        ));
        assert!(!o.validation_passed);
    }

    #[test]
    fn sequence_exhaustion_refuses_without_reserving() {
        let mut o = owner_for_tests();
        o.next_seq = u64::MAX;
        assert!(matches!(
            o.begin(&single_active_crtc(), ledger()),
            Err(DispatchError::IdentityExhausted)
        ));
        assert_eq!(o.slot().occupant(), None);
    }

    #[test]
    fn partial_fences_remain_mapped_and_owned_after_unknown() {
        let mut o = owner_for_tests();
        let (c, _) = o.begin(&two_active_crtcs(), ledger()).unwrap();
        o.mark_dispatched_for_tests();
        o.apply_host_call_event(accepted(c, 0b10, 1));
        let r = o.live_record().unwrap();
        assert!(matches!(r.ledger(), LedgerState::Quarantined(q) if q.held().len() == 2));
        assert_eq!(r.fence_evidence().unwrap().by_crtc()[0].0, 2);
        o.apply_host_call_event(rejected(c, libc::EINVAL));
        assert_eq!(o.slot().occupant(), Some(c));
        assert_eq!(
            o.live_record().unwrap().fence_evidence().unwrap().by_crtc()[0].0,
            2
        );
    }

    #[test]
    fn a_validation_send_refusal_clears_its_lease_and_description() {
        let mut o = owner_for_tests();
        o.begin_validation(&single_active_crtc()).unwrap();
        let mut executor = reaped_executor_for_tests();
        assert!(matches!(
            o.send_validation_on(&mut executor),
            Err(DispatchError::Refused {
                cause: RefusalCause::Reaped,
                ..
            })
        ));
        assert_eq!(o.slot().validation_outstanding(), None);
        assert!(o.validated_description.is_none());
        assert!(matches!(
            o.send_validation_on(&mut executor),
            Err(DispatchError::AlreadySent)
        ));
    }
    // crates/yserver/src/kms/owner/device.rs  (#[cfg(test)] mod tests)

    #[test]
    fn begin_installs_the_record_and_reserves_the_slot_before_any_ipc() {
        // COMMIT-6's ordering, expressed as an API: a send that fails still finds
        // a device with a record that owns the uncertainty.
        let mut o = owner_for_tests();
        let (commit, events) = o.begin(&single_active_crtc(), ledger()).expect("begin");
        assert_eq!(o.slot().occupant(), Some(commit));
        assert!(!o.live_record().expect("record").milestones().dispatched);
        assert!(
            events.is_empty(),
            "begin emits nothing; send_on emits Dispatched"
        );
    }

    #[test]
    fn a_second_begin_is_refused_while_a_record_lives() {
        // spec:1330-1334.
        let mut o = owner_for_tests();
        o.begin(&single_active_crtc(), ledger()).expect("first");
        let err = o
            .begin(&single_active_crtc(), ledger())
            .expect_err("refused");
        assert!(matches!(
            err,
            DispatchError::Slot(SlotError::AlreadyOccupied(_))
        ));
    }

    #[test]
    fn a_construction_failure_never_consumes_the_slot() {
        // Building precedes reserving, so a description that cannot produce a
        // valid request leaves the device admissible.
        let mut o = owner_for_tests();
        let mut bad = single_active_crtc();
        bad.page_flip_event = true;
        bad.objects.push(off_to_off_crtc(2));
        assert!(o.begin(&bad, ledger()).is_err());
        assert_eq!(o.slot().occupant(), None);
        assert!(o.live_record().is_none());
    }

    #[test]
    fn an_executor_refusal_before_ipc_is_never_dispatched_not_acceptance_unknown() {
        // spec:612-617 — a refusal before send is cancellation, not uncertainty.
        // `send` returns Reaped / Stalled / AlreadyInFlight / ReservationMismatch
        // / BoundaryViolation *before* installing InFlight (executor/mod.rs:665-692)
        // and queues no terminal event, so treating every Err as acceptance-unknown
        // would strand a slot-holding record forever.
        let mut o = owner_for_tests();
        let mut executor = reaped_executor_for_tests();
        o.begin(&single_active_crtc(), ledger()).expect("begin");
        let events = o.send_on(&mut executor).expect_err("refused").into_events();
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::Terminal {
                terminal: TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                    RefusalCause::Reaped
                )),
                ..
            }
        )));
        // Both halves of the ledger must come back out. Revision 2 dropped the
        // record here, destroying the old state the hardware is still scanning.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OwnerEvent::ResourcesReleased { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OwnerEvent::ResourcesStillCurrent { .. }))
        );
        assert_eq!(o.slot().occupant(), None, "nothing crossed the boundary");
    }

    #[test]
    fn an_explicit_rejection_is_the_only_proof_of_failed_before_submit() {
        let mut o = owner_for_tests();
        let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
        o.mark_dispatched_for_tests();
        let events = o.apply_host_call_event(rejected(commit, libc::EBUSY));
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::Terminal {
                terminal: TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected { errno }),
                ..
            } if *errno == libc::EBUSY
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::ResourcesReleased { resources, .. } if resources.len() == 1
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::ResourcesStillCurrent { resources, .. } if resources.len() == 1
        )));
        assert_eq!(
            o.slot().occupant(),
            None,
            "a proven rejection releases the slot"
        );
    }

    #[test]
    fn every_acceptance_unknown_reason_keeps_the_slot_held() {
        // COMMIT-6. `UnknownReason::ALL` is compile-checked complete, so a fifth
        // reason cannot be added without deciding which side of this it falls on.
        for reason in UnknownReason::ALL {
            let mut o = owner_for_tests();
            let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
            o.mark_dispatched_for_tests();
            o.apply_host_call_event(unknown(commit, reason));
            assert_eq!(
                o.slot().occupant(),
                Some(commit),
                "{reason:?} released the slot"
            );
            assert!(matches!(
                o.live_record().expect("retained").ledger(),
                LedgerState::Quarantined(_)
            ));
        }
    }

    #[test]
    fn a_complete_acceptance_is_recorded_and_does_not_complete_or_release() {
        // The whole point of the 2b split: acceptance is Accepted, not Completed.
        let mut o = owner_for_tests();
        let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
        o.mark_dispatched_for_tests();
        let events = o.apply_host_call_event(accepted(commit, 0b1, 1));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OwnerEvent::Accepted { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, OwnerEvent::Terminal { .. }))
        );
        let r = o.live_record().expect("still live");
        assert!(r.milestones().accepted);
        assert!(!r.milestones().hardware_complete);
        assert_eq!(*r.state(), RecordState::Submitting);
        assert_eq!(o.slot().occupant(), Some(commit));
    }

    #[test]
    fn a_short_out_fence_mask_is_completion_unknown_not_acceptance() {
        // spec:1955-1962, 2129 — a holder still at -1 after live success is
        // missing completion evidence. The helper sets bit i only when holder i
        // came back non-negative, and the executor's consistency checks accept a
        // mask narrower than the slot table, so this decision is the owner's.
        let mut o = owner_for_tests();
        let (commit, _) = o.begin(&two_active_crtcs(), ledger()).expect("begin");
        o.mark_dispatched_for_tests();
        let events = o.apply_host_call_event(accepted(commit, 0b01, 1)); // 2 expected, 1 back
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::Terminal {
                terminal: TerminalState::CompletionUnknown(UnknownCause::IncompleteFenceOutput {
                    expected: 2,
                    returned: 1
                }),
                ..
            }
        )));
        assert!(!o.live_record().expect("retained").milestones().accepted);
        assert_eq!(o.slot().occupant(), Some(commit), "acceptance is unproven");
    }

    #[test]
    fn a_validation_outcome_under_a_commit_record_is_contradictory_and_terminal() {
        // spec:612-617 — a dispatched result that is neither an explicit
        // rejection nor a normally consumed success becomes CompletionUnknown.
        // The draft only logged a warning and left the record stranded.
        let mut o = owner_for_tests();
        let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
        o.mark_dispatched_for_tests();
        o.apply_host_call_event(validation_abandoned(commit, UnknownReason::WatchdogExpired));
        assert!(matches!(
            o.live_record().expect("retained").state(),
            RecordState::Terminal(TerminalState::CompletionUnknown(
                UnknownCause::ContradictoryEvidence
            ))
        ));
        assert_eq!(o.slot().occupant(), Some(commit));
    }

    #[test]
    fn an_uncorrelated_outcome_never_touches_the_live_record() {
        // ID-3.
        let mut o = owner_for_tests();
        let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
        o.mark_dispatched_for_tests();
        let events = o.apply_host_call_event(rejected(CommitId::for_tests(999), libc::EINVAL));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OwnerEvent::StaleReply { .. }))
        );
        assert_eq!(
            *o.live_record().expect("untouched").state(),
            RecordState::Submitting
        );
        assert_eq!(o.slot().occupant(), Some(commit));
    }

    #[test]
    fn a_late_reply_never_revives_a_terminalized_record() {
        // spec:2205-2210 — a later success is accepted-stale and quarantined.
        let mut o = owner_for_tests();
        let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
        o.mark_dispatched_for_tests();
        o.apply_host_call_event(unknown(commit, UnknownReason::WatchdogExpired));
        let events = o.apply_host_call_event(late_accepted(commit, 0b1, 1));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OwnerEvent::StaleReply { .. }))
        );
        let r = o.live_record().expect("retained");
        assert!(!r.milestones().accepted);
        assert!(matches!(r.ledger(), LedgerState::Quarantined(_)));
    }

    #[test]
    fn a_validation_resolves_its_own_lease_though_it_has_no_record() {
        // The draft consulted `is_current` — which compares against the live
        // record — before looking for the outstanding validation. A validation
        // deliberately has no record, so its outcome was rejected as
        // uncorrelated and its lease never released.
        let mut o = owner_for_tests();
        let commit = o.begin_validation(&single_active_crtc()).expect("validate");
        o.mark_validation_dispatched_for_tests();
        assert_eq!(o.slot().occupant(), None);
        assert_eq!(o.slot().validation_outstanding(), Some(commit));
        let events = o.apply_host_call_event(accepted(commit, 0, 0));
        assert!(events.iter().any(|e| matches!(
            e,
            OwnerEvent::ValidationResolved {
                outcome: ValidationOutcome::Passed,
                ..
            }
        )));
        assert_eq!(
            o.slot().validation_outstanding(),
            Some(commit),
            "spec:305-325: the lease protects the gap between the validation and \
         the live call, so the TEST_ONLY reply does not end it"
        );
        assert!(
            o.tombstones().is_empty(),
            "a validation leaves no commit tombstone"
        );
    }

    #[test]
    fn the_lease_ends_at_the_live_call_and_only_for_the_request_it_validated() {
        let mut o = owner_for_tests();
        let desc = single_active_crtc();
        let commit = o.begin_validation(&desc).expect("validate");
        o.mark_validation_dispatched_for_tests();
        o.apply_host_call_event(accepted(commit, 0, 0));

        // A different description cannot ride a lease taken for this one.
        let err = o
            .begin_validated(&two_active_crtcs(), ledger())
            .expect_err("refused");
        assert!(matches!(err, DispatchError::ValidationDoesNotMatch));
        assert_eq!(
            o.slot().validation_outstanding(),
            Some(commit),
            "the lease survives"
        );

        let (live, _) = o
            .begin_validated(&desc, ledger())
            .expect("the validated request proceeds");
        assert_eq!(o.slot().validation_outstanding(), None);
        assert_eq!(o.slot().occupant(), Some(live));
    }

    #[test]
    fn an_abandoned_validation_frees_the_device() {
        let mut o = owner_for_tests();
        let commit = o.begin_validation(&single_active_crtc()).expect("validate");
        o.mark_validation_dispatched_for_tests();
        o.apply_host_call_event(rejected(commit, libc::EINVAL));
        o.abandon_validation(commit).expect("abandon");
        assert_eq!(o.slot().validation_outstanding(), None);
        o.begin(&single_active_crtc(), ledger())
            .expect("admissible again");
    }

    #[test]
    fn a_probe_outcome_under_a_probe_correlation_is_dropped_not_misread() {
        let mut o = owner_for_tests();
        let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
        o.mark_dispatched_for_tests();
        let events = o.apply_host_call_event(probe_accepted_event(42));
        assert!(matches!(events.as_slice(), [OwnerEvent::StaleReply { .. }]));
        assert_eq!(
            *o.live_record().expect("untouched").state(),
            RecordState::Submitting
        );
        let _ = commit;
    }

    #[test]
    fn the_tombstone_ring_keeps_the_last_sixty_four() {
        // spec:1697-1704.
        let mut o = owner_for_tests();
        let mut created = Vec::new();
        for _ in 0..70 {
            let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
            created.push(commit);
            o.mark_dispatched_for_tests();
            o.apply_host_call_event(rejected(commit, libc::EINVAL));
        }
        assert_eq!(o.tombstones().len(), 64);
        assert_eq!(
            o.tombstones()[0].commit,
            created[6],
            "the oldest six are evicted"
        );
    }
}
