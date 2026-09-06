use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use crate::kms::{
    executor::{
        UnknownReason,
        protocol::{HostCallCorrelation, HostCallRequest, OutFenceSlot},
    },
    owner::{
        closure::AtomicCrtcClosure,
        identity::{CommitId, EventToken, IncarnationId},
        ledger::{LedgerState, Submitted},
        lifecycle::{LifecycleEpochId, LifecycleTransitionId},
        slot::SubmittingProof,
    },
};

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct Milestones {
    pub producer_ready: bool,
    pub dispatched: bool,
    pub accepted: bool,
    pub hardware_complete: bool,     // 2b-ii
    pub presented: bool,             // 2b-ii
    pub prior_buffer_released: bool, // 2b-ii
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TerminalState {
    Completed,
    FailedBeforeSubmit(FailureCause),
    CompletionUnknown(UnknownCause),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FailureCause {
    /// Cancelled, or refused by the executor, before any IPC crossed the
    /// uncertainty boundary.
    NeverDispatched(RefusalCause),
    /// An explicit ioctl rejection — the only post-dispatch proof of
    /// `FailedBeforeSubmit` that `COMMIT-6` permits.
    IoctlRejected { errno: i32 },
}

/// Why the executor refused *before* installing `InFlight`. Each maps to a
/// `SendError` that `executor/mod.rs:665-692` returns before it writes
/// anything, so no IPC occurred and nothing is uncertain.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RefusalCause {
    Reaped,
    Stalled,
    AlreadyInFlight,
    ReservationMismatch,
    BoundaryViolation,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UnknownCause {
    HostCall(UnknownReason),
    /// Live success whose out-fence output is short of
    /// `ExpectedCompletionCrtcs`. `spec:1955-1962`, `spec:2129`.
    IncompleteFenceOutput {
        expected: usize,
        returned: usize,
    },
    /// An outcome whose shape contradicts the request class.
    ContradictoryEvidence,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RecordState {
    Submitting,
    Terminal(TerminalState),
}

#[derive(Debug)]
pub struct FenceEvidence {
    /// The slot table the request was built with, in slot order.
    pub(crate) slots: Vec<OutFenceSlot>,
    /// Bit *i* set means slot *i* produced a descriptor. The helper sets it
    /// only when holder *i* came back non-negative (`helper.rs:263-270`).
    pub(crate) mask: u32,
    /// One descriptor per set bit, in ascending bit order.
    pub(crate) fences: Vec<OwnedFd>,
}

impl FenceEvidence {
    /// `(crtc_id, fd)` pairs. Without the slot table this mapping is lost and
    /// 2b-ii cannot tell which CRTC a descriptor proves.
    pub fn by_crtc(&self) -> Vec<(u32, BorrowedFd<'_>)> {
        let mut result = Vec::new();
        let mut fence_idx = 0;
        for (i, slot) in self.slots.iter().enumerate() {
            if (self.mask & (1 << i)) == 0 {
                continue;
            }
            if let Some(fd) = self.fences.get(fence_idx) {
                result.push((slot.crtc_id, fd.as_fd()));
                fence_idx += 1;
            }
        }
        result
    }

    pub fn returned(&self) -> usize {
        self.mask.count_ones() as usize
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Tombstone {
    pub commit: CommitId,
    pub event_token: EventToken,
    pub incarnation: IncarnationId,
    pub lifecycle_epoch: LifecycleEpochId,
    pub kernel_event_crtcs: Vec<u32>,
    pub present_event_crtcs: Vec<u32>,
    pub observed_crtcs: Vec<u32>,
    pub terminal: TerminalState,
}

#[derive(Debug)]
pub struct CommitRecord<R> {
    commit: CommitId,
    event_token: EventToken,
    incarnation: IncarnationId,
    lifecycle_epoch: LifecycleEpochId,
    #[allow(dead_code)]
    transition: Option<LifecycleTransitionId>,
    #[allow(dead_code)]
    topology_generation: u64,
    closure: AtomicCrtcClosure,
    correlation: HostCallCorrelation,
    milestones: Milestones,
    ledger: LedgerState<R>,
    observed_crtcs: Vec<u32>,
    fences: Option<FenceEvidence>,
    pending_request: Option<(HostCallRequest, SubmittingProof)>,
    out_fence_slots: Vec<OutFenceSlot>,
    state: RecordState,
}

impl<R> CommitRecord<R> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        commit: CommitId,
        event_token: EventToken,
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        transition: Option<LifecycleTransitionId>,
        topology_generation: u64,
        closure: AtomicCrtcClosure,
        correlation: HostCallCorrelation,
        ledger: Submitted<R>,
    ) -> Self {
        Self {
            commit,
            event_token,
            incarnation,
            lifecycle_epoch,
            transition,
            topology_generation,
            closure,
            correlation,
            milestones: Milestones {
                producer_ready: true,
                ..Milestones::default()
            },
            ledger: LedgerState::Submitted(ledger),
            observed_crtcs: Vec::new(),
            fences: None,
            pending_request: None,
            out_fence_slots: Vec::new(),
            state: RecordState::Submitting,
        }
    }

    pub fn commit_id(&self) -> CommitId {
        self.commit
    }

    pub fn event_token(&self) -> EventToken {
        self.event_token
    }

    pub fn closure(&self) -> &AtomicCrtcClosure {
        &self.closure
    }

    pub fn correlation(&self) -> &HostCallCorrelation {
        &self.correlation
    }

    pub fn milestones(&self) -> &Milestones {
        &self.milestones
    }

    pub fn ledger(&self) -> &LedgerState<R> {
        &self.ledger
    }

    pub fn state(&self) -> &RecordState {
        &self.state
    }

    pub fn attach_request(&mut self, req: HostCallRequest, proof: SubmittingProof) {
        if let HostCallRequest::Atomic(ref atomic) = req {
            self.out_fence_slots = atomic.out_fence_slots.clone();
        }
        self.pending_request = Some((req, proof));
    }

    pub(super) fn adopt_returned_fences(&mut self, mask: u32, fences: Vec<OwnedFd>) {
        self.adopt_fences(self.out_fence_slots.clone(), mask, fences);
    }

    pub(super) fn take_rejected_ledger(&mut self) -> LedgerState<R> {
        assert!(matches!(
            self.state,
            RecordState::Terminal(TerminalState::FailedBeforeSubmit(_))
        ));
        std::mem::replace(&mut self.ledger, LedgerState::Poisoned)
    }

    pub fn take_request(&mut self) -> Option<(HostCallRequest, SubmittingProof)> {
        self.pending_request.take()
    }

    pub fn fence_evidence(&self) -> Option<&FenceEvidence> {
        self.fences.as_ref()
    }

    /// Set at `send` return, not at reply.
    pub fn mark_dispatched(&mut self) {
        self.milestones.dispatched = true;
    }

    /// Set only by an explicit `HostCallOutcome::Accepted` whose fence output
    /// is complete. It implies nothing about hardware completion.
    pub fn mark_accepted(&mut self) {
        self.milestones.accepted = true;
        self.advance_ledger(|l| match l {
            LedgerState::Submitted(s) => LedgerState::Accepted(s.accepted()),
            other => other,
        });
    }

    pub fn adopt_fences(&mut self, slots: Vec<OutFenceSlot>, mask: u32, fences: Vec<OwnedFd>) {
        self.fences = Some(FenceEvidence {
            slots,
            mask,
            fences,
        });
    }

    /// The one place a resource leaves a record. Returns the never-current
    /// new state by value; a terminalized record returns an empty vector.
    pub fn terminalize_rejected(&mut self, errno: i32) -> Vec<R> {
        if matches!(self.state, RecordState::Terminal(_)) {
            return Vec::new();
        }
        let mut released = Vec::new();
        self.advance_ledger(|l| match l {
            LedgerState::Submitted(s) => {
                let (rejected, freed) = s.rejected();
                released = freed;
                LedgerState::Rejected(rejected)
            }
            other => other,
        });
        self.state = RecordState::Terminal(TerminalState::FailedBeforeSubmit(
            FailureCause::IoctlRejected { errno },
        ));
        released
    }

    pub fn terminalize(&mut self, terminal: TerminalState) {
        if matches!(self.state, RecordState::Terminal(_)) {
            return;
        }
        if matches!(terminal, TerminalState::CompletionUnknown(_)) {
            self.advance_ledger(|l| match l {
                LedgerState::Submitted(s) => LedgerState::Quarantined(s.unknown()),
                LedgerState::Accepted(a) => LedgerState::Quarantined(a.unknown()),
                other => other,
            });
        }
        self.state = RecordState::Terminal(terminal);
    }

    fn advance_ledger(&mut self, f: impl FnOnce(LedgerState<R>) -> LedgerState<R>) {
        let taken = std::mem::replace(&mut self.ledger, LedgerState::Poisoned);
        self.ledger = f(taken);
    }

    pub fn tombstone(&self) -> Option<Tombstone> {
        let RecordState::Terminal(terminal) = self.state else {
            return None;
        };
        Some(Tombstone {
            commit: self.commit,
            event_token: self.event_token,
            incarnation: self.incarnation,
            lifecycle_epoch: self.lifecycle_epoch,
            kernel_event_crtcs: self.closure.kernel_event().to_vec(),
            present_event_crtcs: self.closure.present_event().to_vec(),
            observed_crtcs: self.observed_crtcs.clone(),
            terminal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;

    use crate::kms::{
        executor::protocol::golden_atomic_request_for_tests,
        owner::closure::{CrtcPower, ObjectKind, PropertyIds, SerializedObject},
    };

    #[derive(Debug, PartialEq)]
    struct TestResource(u32);

    fn pipe_read_end() -> OwnedFd {
        let mut fds = [-1i32; 2];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0);
        unsafe {
            libc::close(fds[1]);
            OwnedFd::from_raw_fd(fds[0])
        }
    }

    fn request_for_tests() -> HostCallRequest {
        HostCallRequest::Atomic(golden_atomic_request_for_tests())
    }

    fn record() -> CommitRecord<TestResource> {
        let closure = AtomicCrtcClosure::compute(
            &[SerializedObject {
                object: 1,
                kind: ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![(21, 1)],
            }],
            &[CrtcPower {
                crtc_id: 1,
                old_active: true,
                new_active: true,
            }],
            &PropertyIds {
                crtc_id: 20,
                active: 21,
                out_fence_ptr: 22,
            },
            true,
            &[1],
        )
        .expect("closure");

        let correlation = HostCallCorrelation::Atomic {
            seq: crate::kms::executor::protocol::RequestSeq::from_raw(1),
            incarnation: IncarnationId::from_raw(1),
            lifecycle_epoch: LifecycleEpochId::from_raw(1),
            transition: None,
            commit: CommitId::for_tests(1),
            event_token: EventToken::tagged_for_tests(1),
        };

        CommitRecord::new(
            CommitId::for_tests(1),
            EventToken::tagged_for_tests(1),
            IncarnationId::from_raw(1),
            LifecycleEpochId::from_raw(1),
            None,
            1,
            closure,
            correlation,
            Submitted::new(vec![TestResource(66)], vec![TestResource(77)]),
        )
    }

    #[test]
    fn a_new_record_is_submitting_and_already_producer_ready() {
        // spec:2114-2118 — a record exists only after every source dependency
        // completed; a thing that could exist without it is a queued intent,
        // which is 2c's.
        let r = record();
        assert_eq!(*r.state(), RecordState::Submitting);
        assert!(r.milestones().producer_ready);
        assert!(!r.milestones().dispatched);
    }

    #[test]
    fn dispatch_and_acceptance_are_independently_typed() {
        // spec:2124-2130 — code records both and may not infer either from the
        // other.
        let mut r = record();
        r.mark_dispatched();
        assert!(r.milestones().dispatched);
        assert!(!r.milestones().accepted, "send is not acceptance");
        r.mark_accepted();
        assert!(r.milestones().accepted);
    }

    #[test]
    fn acceptance_is_not_terminal_and_sets_no_completion_milestone() {
        // spec:1996-2000 and 2137-2139. Completed needs the section 6.3 evidence
        // for the class; 2b-i can observe none of it.
        let mut r = record();
        r.mark_dispatched();
        r.mark_accepted();
        assert_eq!(*r.state(), RecordState::Submitting);
        assert!(!r.milestones().hardware_complete);
        assert!(!r.milestones().presented);
        assert!(!r.milestones().prior_buffer_released);
    }

    #[test]
    fn the_first_terminal_state_wins() {
        // spec:2205-2210 — a later explicit result is accepted-stale.
        let mut r = record();
        r.mark_dispatched();
        r.terminalize(TerminalState::CompletionUnknown(UnknownCause::HostCall(
            UnknownReason::WatchdogExpired,
        )));
        let first = *r.state();
        r.terminalize(TerminalState::FailedBeforeSubmit(
            FailureCause::IoctlRejected { errno: libc::EBUSY },
        ));
        assert_eq!(
            *r.state(),
            first,
            "a terminalized record does not terminalize again"
        );
    }

    #[test]
    fn an_unknown_terminal_quarantines_the_ledger() {
        let mut r = record();
        r.mark_dispatched();
        r.terminalize(TerminalState::CompletionUnknown(
            UnknownCause::ContradictoryEvidence,
        ));
        assert!(matches!(r.ledger(), LedgerState::Quarantined(_)));
        assert!(r.ledger().releases_nothing());
    }

    #[test]
    fn a_rejection_yields_the_new_state_once_and_never_again() {
        let mut r = record();
        r.mark_dispatched();
        let released = r.terminalize_rejected(libc::EINVAL);
        assert_eq!(released, vec![TestResource(77)]);
        let again = r.terminalize_rejected(libc::EINVAL);
        assert!(
            again.is_empty(),
            "a terminalized record releases nothing a second time"
        );
    }

    #[test]
    fn the_request_is_taken_exactly_once() {
        let mut r = record();
        r.attach_request(request_for_tests(), SubmittingProof::for_tests());
        assert!(r.take_request().is_some());
        assert!(
            r.take_request().is_none(),
            "a second send must not reuse one reservation"
        );
    }

    #[test]
    fn fence_evidence_maps_each_descriptor_back_to_its_crtc() {
        // A bare Vec<OwnedFd> loses this: an Accepted reply returns only the
        // descriptors that came back, so position alone does not name a CRTC.
        let mut r = record();
        r.adopt_fences(
            vec![
                OutFenceSlot {
                    crtc_id: 5,
                    value_index: 1,
                },
                OutFenceSlot {
                    crtc_id: 9,
                    value_index: 3,
                },
            ],
            0b10, // only slot 1 produced a descriptor
            vec![pipe_read_end()],
        );
        let ev = r.fence_evidence().expect("evidence");
        assert_eq!(ev.returned(), 1);
        assert_eq!(ev.by_crtc().len(), 1);
        assert_eq!(
            ev.by_crtc()[0].0,
            9,
            "the set bit is slot 1, whose CRTC is 9"
        );
    }

    #[test]
    fn a_tombstone_keeps_identity_and_sets_but_owns_no_resource() {
        // spec:1699-1701.
        let mut r = record();
        r.mark_dispatched();
        let _ = r.terminalize_rejected(libc::EINVAL);
        let t = r.tombstone().expect("a terminalized record tombstones");
        assert_eq!(t.commit, r.commit_id());
        assert_eq!(t.event_token, r.event_token());
        assert_eq!(t.kernel_event_crtcs, r.closure().kernel_event().to_vec());
        assert_eq!(t.present_event_crtcs, r.closure().present_event().to_vec());
        assert!(matches!(t.terminal, TerminalState::FailedBeforeSubmit(_)));
    }

    #[test]
    fn a_live_record_does_not_tombstone() {
        assert!(record().tombstone().is_none());
    }
}
