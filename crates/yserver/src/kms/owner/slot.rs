use crate::kms::owner::{identity::CommitId, lifecycle::ClockProbeId};

/// Linear proof that the one device slot was reserved for this commit.
///
/// Defined **here**, not in `executor/`, and its issuing constructor `issue`
/// is private to this module — so no other module can mint one *through the
/// production path*.
#[derive(Debug)]
pub struct SubmittingProof(());

/// Linear proof of the exclusive owner validation lease.
#[derive(Debug)]
pub struct ValidationLease(());

/// Lease authorizing a clock-probe query.
#[derive(Debug)]
pub struct ClockProbeLease(());

impl ClockProbeLease {
    #[doc(hidden)]
    pub const fn for_tests() -> Self {
        Self(())
    }
    pub(crate) fn issue() -> Self {
        Self(())
    }
}

impl SubmittingProof {
    fn issue() -> Self {
        Self(())
    }

    /// The one deliberate seam: stage 2a's executor tests construct a proof
    /// with no owner in play. Task 7's grep bounds its use.
    #[doc(hidden)]
    pub const fn for_tests() -> Self {
        Self(())
    }
}

impl ValidationLease {
    fn issue() -> Self {
        Self(())
    }

    #[doc(hidden)]
    pub const fn for_tests() -> Self {
        Self(())
    }
}

#[derive(Debug, Default)]
pub struct DeviceSlot {
    occupant: Option<CommitId>,
    validation: Option<CommitId>,
    probing: Option<ClockProbeId>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SlotError {
    #[error("the device slot is already held by commit {0:?}")]
    AlreadyOccupied(CommitId),
    #[error("an exclusive validation lease is outstanding for commit {0:?}")]
    ValidationOutstanding(CommitId),
    #[error("the device slot is not held by commit {0:?}")]
    NotHeld(CommitId),
    #[error("no validation lease is outstanding for commit {0:?}")]
    NoValidationLease(CommitId),
    #[error("probe lease is outstanding")]
    ProbeOutstanding(ClockProbeId),
    #[error("no probe lease is outstanding")]
    NoProbeLease(ClockProbeId),
}

impl DeviceSlot {
    pub fn reserve(&mut self, commit: CommitId) -> Result<SubmittingProof, SlotError> {
        if let Some(held) = self.occupant {
            return Err(SlotError::AlreadyOccupied(held));
        }
        if let Some(validating) = self.validation {
            return Err(SlotError::ValidationOutstanding(validating));
        }
        if let Some(probe) = self.probing {
            return Err(SlotError::ProbeOutstanding(probe));
        }
        self.occupant = Some(commit);
        Ok(SubmittingProof::issue())
    }

    pub fn release(&mut self, commit: CommitId) -> Result<(), SlotError> {
        match self.occupant {
            Some(held) if held == commit => {
                self.occupant = None;
                Ok(())
            }
            _ => Err(SlotError::NotHeld(commit)),
        }
    }

    pub fn acquire_validation(&mut self, commit: CommitId) -> Result<ValidationLease, SlotError> {
        if let Some(validating) = self.validation {
            return Err(SlotError::ValidationOutstanding(validating));
        }
        if let Some(probe) = self.probing {
            return Err(SlotError::ProbeOutstanding(probe));
        }
        // spec:305-325 — the lease exists so no persistent generation changes
        // before the live call. An unresolved commit may still change one.
        if let Some(held) = self.occupant {
            return Err(SlotError::AlreadyOccupied(held));
        }
        self.validation = Some(commit);
        Ok(ValidationLease::issue())
    }

    pub fn acquire_probe(&mut self, probe: ClockProbeId) -> Result<ClockProbeLease, SlotError> {
        if let Some(held) = self.occupant {
            return Err(SlotError::AlreadyOccupied(held));
        }
        if let Some(validating) = self.validation {
            return Err(SlotError::ValidationOutstanding(validating));
        }
        if let Some(probing) = self.probing {
            return Err(SlotError::ProbeOutstanding(probing));
        }
        self.probing = Some(probe);
        Ok(ClockProbeLease::issue())
    }

    pub fn release_probe(&mut self, probe: ClockProbeId) -> Result<(), SlotError> {
        if self.probing != Some(probe) {
            return Err(SlotError::NoProbeLease(probe));
        }
        self.probing = None;
        Ok(())
    }

    /// End the exclusive interval by proceeding to the live call. Releasing
    /// and reserving in one step is the point: two calls would leave a window
    /// in which neither is held and another commit could be admitted.
    /// `lease` is the id the lease was taken under; `commit` is the live
    /// commit that now takes the slot. They differ: a validation and the call
    /// it validated are two allocations, and the slot must end up holding the
    /// one whose record exists.
    pub fn consume_validation(
        &mut self,
        lease: CommitId,
        commit: CommitId,
    ) -> Result<SubmittingProof, SlotError> {
        match self.validation {
            Some(held) if held == lease => {
                if let Some(occupied) = self.occupant {
                    return Err(SlotError::AlreadyOccupied(occupied));
                }
                self.validation = None;
                self.occupant = Some(commit);
                Ok(SubmittingProof::issue())
            }
            _ => Err(SlotError::NoValidationLease(lease)),
        }
    }

    /// End the exclusive interval without proceeding.
    pub fn abandon_validation(&mut self, commit: CommitId) -> Result<(), SlotError> {
        match self.validation {
            Some(held) if held == commit => {
                self.validation = None;
                Ok(())
            }
            _ => Err(SlotError::NoValidationLease(commit)),
        }
    }

    pub fn occupant(&self) -> Option<CommitId> {
        self.occupant
    }

    pub fn validation_outstanding(&self) -> Option<CommitId> {
        self.validation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(n: u64) -> CommitId {
        CommitId::for_tests(n)
    }

    fn p(n: u64) -> crate::kms::owner::lifecycle::ClockProbeId {
        crate::kms::owner::lifecycle::ClockProbeId::from_raw(n)
    }

    /// [ID-3, COMMIT-5, CAP-1..4] Catches a probe being admitted while an
    /// atomic or validation owns the device, or either owner bypassing a probe.
    #[test]
    fn probe_atomic_and_validation_reservations_are_mutually_exclusive() {
        let mut slot = DeviceSlot::default();
        let _probe = slot.acquire_probe(p(1)).unwrap();
        assert_eq!(
            slot.reserve(c(1)).unwrap_err(),
            SlotError::ProbeOutstanding(p(1))
        );
        assert_eq!(
            slot.acquire_validation(c(1)).unwrap_err(),
            SlotError::ProbeOutstanding(p(1))
        );
        slot.release_probe(p(1)).unwrap();

        let _atomic = slot.reserve(c(2)).unwrap();
        assert_eq!(
            slot.acquire_probe(p(2)).unwrap_err(),
            SlotError::AlreadyOccupied(c(2))
        );
        slot.release(c(2)).unwrap();

        let _validation = slot.acquire_validation(c(3)).unwrap();
        assert_eq!(
            slot.acquire_probe(p(3)).unwrap_err(),
            SlotError::ValidationOutstanding(c(3))
        );
    }

    #[test]
    fn one_commit_may_hold_the_device_slot() {
        // spec:1328-1331 — exactly one dispatched-or-submitted live atomic
        // transaction per DRM device, not one per CRTC.
        let mut slot = DeviceSlot::default();
        let _proof = slot.reserve(c(1)).expect("first reservation");
        assert_eq!(
            slot.reserve(c(2)).expect_err("refused"),
            SlotError::AlreadyOccupied(c(1))
        );
    }

    #[test]
    fn the_slot_is_not_released_by_a_stranger_or_by_a_late_result() {
        // spec:1330-1332 — the slot is not released merely because the ioctl
        // result is late. Only its holder releases it, by id.
        let mut slot = DeviceSlot::default();
        let _proof = slot.reserve(c(1)).expect("reserve");
        assert_eq!(
            slot.release(c(2)).expect_err("refused"),
            SlotError::NotHeld(c(2))
        );
        assert_eq!(slot.occupant(), Some(c(1)));
    }

    #[test]
    #[allow(clippy::drop_non_drop)]
    fn dropping_the_proof_does_not_release_the_slot() {
        // A guard would release on unwind and on every early return. The one
        // thing this slot must never do is free itself because a result was late.
        let mut slot = DeviceSlot::default();
        drop(slot.reserve(c(1)).expect("reserve"));
        assert_eq!(slot.occupant(), Some(c(1)));
    }

    #[test]
    fn validation_does_not_occupy_the_submitted_commit_slot() {
        // spec:322-323 — TEST_ONLY does not occupy the submitted-commit slot.
        let mut slot = DeviceSlot::default();
        let _lease = slot.acquire_validation(c(1)).expect("lease");
        assert_eq!(slot.occupant(), None);
    }

    #[test]
    fn an_outstanding_validation_lease_blocks_a_new_commit() {
        // spec:324-325 — the exclusive lease exists so no persistent generation
        // can change before the live call. Admitting another commit is exactly
        // such a change. The first draft of this plan asserted the opposite.
        let mut slot = DeviceSlot::default();
        let _lease = slot.acquire_validation(c(1)).expect("lease");
        assert_eq!(
            slot.reserve(c(2)).expect_err("refused"),
            SlotError::ValidationOutstanding(c(1))
        );
        slot.abandon_validation(c(1)).expect("abandon");
        let _proof = slot
            .reserve(c(2))
            .expect("admissible once the lease is gone");
    }

    #[test]
    fn an_unresolved_commit_blocks_a_new_validation() {
        // The other half of spec:305-325. Revision 2 only blocked the commit; a
        // validation could still begin while a live commit was unresolved and
        // might yet change a persistent generation.
        let mut slot = DeviceSlot::default();
        let _proof = slot.reserve(c(1)).expect("reserve");
        assert_eq!(
            slot.acquire_validation(c(2)).expect_err("refused"),
            SlotError::AlreadyOccupied(c(1))
        );
    }

    #[test]
    fn consuming_a_lease_takes_the_slot_with_no_window_in_between() {
        // The exclusive interval ends by becoming the live commit, not by
        // releasing and hoping to re-reserve.
        let mut slot = DeviceSlot::default();
        let _lease = slot.acquire_validation(c(1)).expect("lease");
        let _proof = slot.consume_validation(c(1), c(2)).expect("consume");
        assert_eq!(slot.validation_outstanding(), None);
        assert_eq!(
            slot.occupant(),
            Some(c(2)),
            "the LIVE commit takes the slot; the lease's id was a different allocation"
        );
    }

    #[test]
    fn abandoning_a_lease_leaves_the_device_admissible() {
        let mut slot = DeviceSlot::default();
        let _lease = slot.acquire_validation(c(1)).expect("lease");
        slot.abandon_validation(c(1)).expect("abandon");
        assert_eq!(slot.validation_outstanding(), None);
        let _proof = slot.reserve(c(2)).expect("admissible");
    }

    #[test]
    fn a_stranger_can_neither_consume_nor_abandon_a_lease() {
        let mut slot = DeviceSlot::default();
        let _lease = slot.acquire_validation(c(1)).expect("lease");
        assert_eq!(
            slot.consume_validation(c(2), c(3)).expect_err("refused"),
            SlotError::NoValidationLease(c(2))
        );
        assert_eq!(
            slot.abandon_validation(c(2)).expect_err("refused"),
            SlotError::NoValidationLease(c(2))
        );
        assert_eq!(slot.validation_outstanding(), Some(c(1)));
    }

    #[test]
    fn the_validation_lease_is_exclusive() {
        let mut slot = DeviceSlot::default();
        let _lease = slot.acquire_validation(c(1)).expect("first lease");
        assert_eq!(
            slot.acquire_validation(c(2)).expect_err("refused"),
            SlotError::ValidationOutstanding(c(1))
        );
    }

    #[test]
    fn releasing_and_reserving_again_is_permitted() {
        let mut slot = DeviceSlot::default();
        let _proof = slot.reserve(c(1)).expect("reserve");
        slot.release(c(1)).expect("release");
        assert_eq!(slot.occupant(), None);
        let _proof = slot.reserve(c(2)).expect("re-reserve");
    }
}
