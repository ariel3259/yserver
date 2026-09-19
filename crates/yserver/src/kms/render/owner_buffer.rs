#![allow(
    dead_code,
    reason = "OwnerBuffer is wired into the scene in Task 2 of the refactor"
)]

use crate::kms::{
    backend::OutputKey,
    owner::identity::CommitId,
    render::resources::{AllocationKey, AllocationLease},
};

/// Immutable identity of one owner-route scanout generation.
///
/// The scene uses the output key, buffer index, generation and commit id to
/// find members at different owner milestones. Keeping the other identity
/// fields beside them makes every consuming transition carry the same member
/// without reconstructing any of its identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnerBufferIdentity {
    pub(crate) output_key: OutputKey,
    pub(crate) crtc: u32,
    pub(crate) bo_idx: usize,
    pub(crate) generation: u64,
    pub(crate) managed_key: AllocationKey,
}

/// The logical state of an owner-held scanout buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerBufferState {
    Rendering,
    Desired,
    Displaced,
    Submitted,
    Accepted,
    Current,
    Releasing,
    Quarantined,
}

/// One owner-route scanout buffer and its state-specific payload.
///
/// `P` is the captured scene acknowledgement payload. Task 2 instantiates it
/// with the scene's `PendingAck`; keeping the state machine independent here
/// lets Task 1 prove all ownership transitions without constructing Vulkan
/// state. `Free` is intentionally not a variant: a free buffer is absent from
/// the owner's collection.
#[derive(Debug)]
pub(crate) enum OwnerBuffer<P> {
    Rendering {
        identity: OwnerBufferIdentity,
        pending_ack: P,
        descriptor_slot: usize,
    },
    Desired {
        identity: OwnerBufferIdentity,
        pending_ack: P,
        allocation_lease: AllocationLease,
        descriptor_slot: Option<usize>,
    },
    Displaced {
        identity: OwnerBufferIdentity,
        pending_ack: P,
        allocation_lease: Option<AllocationLease>,
        descriptor_slot: Option<usize>,
    },
    Submitted {
        identity: OwnerBufferIdentity,
        pending_ack: P,
        commit_id: CommitId,
    },
    Accepted {
        identity: OwnerBufferIdentity,
        pending_ack: P,
        commit_id: CommitId,
    },
    Current {
        identity: OwnerBufferIdentity,
        pending_ack: P,
        commit_id: CommitId,
    },
    Releasing {
        identity: OwnerBufferIdentity,
        pending_ack: P,
        commit_id: CommitId,
    },
    Quarantined {
        identity: OwnerBufferIdentity,
    },
}

impl<P> OwnerBuffer<P> {
    pub(crate) fn rendering(
        identity: OwnerBufferIdentity,
        pending_ack: P,
        descriptor_slot: usize,
    ) -> Self {
        Self::Rendering {
            identity,
            pending_ack,
            descriptor_slot,
        }
    }

    pub(crate) fn state(&self) -> OwnerBufferState {
        match self {
            Self::Rendering { .. } => OwnerBufferState::Rendering,
            Self::Desired { .. } => OwnerBufferState::Desired,
            Self::Displaced { .. } => OwnerBufferState::Displaced,
            Self::Submitted { .. } => OwnerBufferState::Submitted,
            Self::Accepted { .. } => OwnerBufferState::Accepted,
            Self::Current { .. } => OwnerBufferState::Current,
            Self::Releasing { .. } => OwnerBufferState::Releasing,
            Self::Quarantined { .. } => OwnerBufferState::Quarantined,
        }
    }

    pub(crate) fn identity(&self) -> &OwnerBufferIdentity {
        match self {
            Self::Rendering { identity, .. }
            | Self::Desired { identity, .. }
            | Self::Displaced { identity, .. }
            | Self::Submitted { identity, .. }
            | Self::Accepted { identity, .. }
            | Self::Current { identity, .. }
            | Self::Releasing { identity, .. }
            | Self::Quarantined { identity } => identity,
        }
    }

    pub(crate) fn commit_id(&self) -> Option<CommitId> {
        match self {
            Self::Submitted { commit_id, .. }
            | Self::Accepted { commit_id, .. }
            | Self::Current { commit_id, .. }
            | Self::Releasing { commit_id, .. } => Some(*commit_id),
            Self::Rendering { .. }
            | Self::Desired { .. }
            | Self::Displaced { .. }
            | Self::Quarantined { .. } => None,
        }
    }

    pub(crate) fn owns_allocation_lease(&self) -> bool {
        match self {
            Self::Desired { .. } => true,
            Self::Displaced {
                allocation_lease, ..
            } => allocation_lease.is_some(),
            Self::Rendering { .. }
            | Self::Submitted { .. }
            | Self::Accepted { .. }
            | Self::Current { .. }
            | Self::Releasing { .. }
            | Self::Quarantined { .. } => false,
        }
    }

    pub(crate) fn descriptor_slot(&self) -> Option<usize> {
        match self {
            Self::Rendering {
                descriptor_slot, ..
            } => Some(*descriptor_slot),
            Self::Desired {
                descriptor_slot, ..
            } => *descriptor_slot,
            Self::Displaced {
                descriptor_slot, ..
            } => *descriptor_slot,
            Self::Submitted { .. }
            | Self::Accepted { .. }
            | Self::Current { .. }
            | Self::Releasing { .. }
            | Self::Quarantined { .. } => None,
        }
    }

    pub(crate) fn has_pending_ack(&self) -> bool {
        !matches!(self, Self::Quarantined { .. })
    }

    /// The render completion made a Rendering generation admissible. The
    /// caller supplies the newly acquired retain lease; a refusal returns
    /// both the buffer and that lease unchanged.
    pub(crate) fn into_desired(
        self,
        allocation_lease: AllocationLease,
    ) -> Result<Self, Box<(Self, AllocationLease)>> {
        match self {
            Self::Rendering {
                identity,
                pending_ack,
                descriptor_slot,
            } => Ok(Self::Desired {
                identity,
                pending_ack,
                allocation_lease,
                descriptor_slot: Some(descriptor_slot),
            }),
            other => Err(Box::new((other, allocation_lease))),
        }
    }

    /// A newer generation can replace a generation that has not entered an
    /// owner commit. A Desired lease remains owned until the displaced
    /// buffer's compose work is retired; a Rendering buffer has none.
    pub(crate) fn into_displaced(self) -> Result<Self, Box<Self>> {
        match self {
            Self::Rendering {
                identity,
                pending_ack,
                descriptor_slot,
            } => Ok(Self::Displaced {
                identity,
                pending_ack,
                allocation_lease: None,
                descriptor_slot: Some(descriptor_slot),
            }),
            Self::Desired {
                identity,
                pending_ack,
                allocation_lease,
                descriptor_slot,
            } => Ok(Self::Displaced {
                identity,
                pending_ack,
                allocation_lease: Some(allocation_lease),
                descriptor_slot,
            }),
            Self::Submitted {
                identity,
                pending_ack,
                ..
            } => Ok(Self::Displaced {
                identity,
                pending_ack,
                allocation_lease: None,
                descriptor_slot: None,
            }),
            other => Err(Box::new(other)),
        }
    }

    /// Admission takes the Desired allocation into the owner ledger. The
    /// lease is returned separately because Submitted no longer owns it.
    pub(crate) fn into_submitted(
        self,
        commit_id: CommitId,
    ) -> Result<(Self, AllocationLease), Box<Self>> {
        match self {
            Self::Desired {
                identity,
                pending_ack,
                allocation_lease,
                ..
            } => Ok((
                Self::Submitted {
                    identity,
                    pending_ack,
                    commit_id,
                },
                allocation_lease,
            )),
            other => Err(Box::new(other)),
        }
    }

    /// A pre-IPC refusal restores the ledger's returned allocation to the
    /// same generation. If the receiver is not Submitted, neither argument
    /// is consumed.
    pub(crate) fn into_desired_after_refusal(
        self,
        allocation_lease: AllocationLease,
    ) -> Result<Self, Box<(Self, AllocationLease)>> {
        match self {
            Self::Submitted {
                identity,
                pending_ack,
                ..
            } => Ok(Self::Desired {
                identity,
                pending_ack,
                allocation_lease,
                descriptor_slot: None,
            }),
            other => Err(Box::new((other, allocation_lease))),
        }
    }

    pub(crate) fn into_accepted(self) -> Result<Self, Box<Self>> {
        match self {
            Self::Submitted {
                identity,
                pending_ack,
                commit_id,
            } => Ok(Self::Accepted {
                identity,
                pending_ack,
                commit_id,
            }),
            other => Err(Box::new(other)),
        }
    }

    pub(crate) fn into_current(self) -> Result<Self, Box<Self>> {
        match self {
            Self::Accepted {
                identity,
                pending_ack,
                commit_id,
            } => Ok(Self::Current {
                identity,
                pending_ack,
                commit_id,
            }),
            other => Err(Box::new(other)),
        }
    }

    pub(crate) fn into_releasing(self) -> Result<Self, Box<Self>> {
        match self {
            Self::Current {
                identity,
                pending_ack,
                commit_id,
            } => Ok(Self::Releasing {
                identity,
                pending_ack,
                commit_id,
            }),
            other => Err(Box::new(other)),
        }
    }

    /// Roll back a compound Current → Releasing operation when installing a
    /// newer Current buffer fails. This is the only reverse transition in
    /// the owner machine and preserves the old commit identity.
    pub(crate) fn restore_current_after_release_abort(self) -> Result<Self, Box<Self>> {
        match self {
            Self::Releasing {
                identity,
                pending_ack,
                commit_id,
            } => Ok(Self::Current {
                identity,
                pending_ack,
                commit_id,
            }),
            other => Err(Box::new(other)),
        }
    }

    /// CompletionUnknown poisons the owner buffer. The payload, any lease,
    /// descriptor slot and commit id are deliberately dropped; recovery has
    /// no exit from this marker in C.0.
    pub(crate) fn into_quarantined(self) -> Result<Self, Box<Self>> {
        match self {
            Self::Submitted { identity, .. } | Self::Accepted { identity, .. } => {
                Ok(Self::Quarantined { identity })
            }
            other => Err(Box::new(other)),
        }
    }

    pub(crate) fn into_free(self) -> Result<OwnerBufferIdentity, Box<Self>> {
        match self {
            Self::Displaced { identity, .. } | Self::Releasing { identity, .. } => Ok(identity),
            other => Err(Box::new(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::super::resources::{self, AllocationKey, UseKind};
    use crate::kms::{backend::OutputKey, owner::identity::CommitId};

    use super::{OwnerBuffer, OwnerBufferIdentity, OwnerBufferState};

    #[derive(Debug)]
    struct TestPendingAck {
        drops: Rc<Cell<usize>>,
    }

    impl Drop for TestPendingAck {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    fn pending_ack(drops: &Rc<Cell<usize>>) -> TestPendingAck {
        TestPendingAck {
            drops: Rc::clone(drops),
        }
    }

    fn identity(key: AllocationKey, generation: u64, bo_idx: usize) -> OwnerBufferIdentity {
        OwnerBufferIdentity {
            output_key: OutputKey::new(key.device, "DP-1"),
            crtc: 42,
            bo_idx,
            generation,
            managed_key: key,
        }
    }

    fn assert_identity(buffer: &OwnerBuffer<TestPendingAck>, expected: &OwnerBufferIdentity) {
        assert_eq!(buffer.identity(), expected);
    }

    const ALL_STATES: [OwnerBufferState; 8] = [
        OwnerBufferState::Rendering,
        OwnerBufferState::Desired,
        OwnerBufferState::Displaced,
        OwnerBufferState::Submitted,
        OwnerBufferState::Accepted,
        OwnerBufferState::Current,
        OwnerBufferState::Releasing,
        OwnerBufferState::Quarantined,
    ];

    #[derive(Clone, Copy, Debug)]
    enum Transition {
        IntoDesired,
        IntoDisplaced,
        IntoSubmitted,
        IntoDesiredAfterRefusal,
        IntoAccepted,
        IntoCurrent,
        IntoReleasing,
        RestoreCurrentAfterReleaseAbort,
        IntoQuarantined,
        IntoFree,
    }

    const TRANSITIONS: [(Transition, &[OwnerBufferState]); 10] = [
        (Transition::IntoDesired, &[OwnerBufferState::Rendering]),
        (
            Transition::IntoDisplaced,
            &[
                OwnerBufferState::Rendering,
                OwnerBufferState::Desired,
                OwnerBufferState::Submitted,
            ],
        ),
        (Transition::IntoSubmitted, &[OwnerBufferState::Desired]),
        (
            Transition::IntoDesiredAfterRefusal,
            &[OwnerBufferState::Submitted],
        ),
        (Transition::IntoAccepted, &[OwnerBufferState::Submitted]),
        (Transition::IntoCurrent, &[OwnerBufferState::Accepted]),
        (Transition::IntoReleasing, &[OwnerBufferState::Current]),
        (
            Transition::RestoreCurrentAfterReleaseAbort,
            &[OwnerBufferState::Releasing],
        ),
        (
            Transition::IntoQuarantined,
            &[OwnerBufferState::Submitted, OwnerBufferState::Accepted],
        ),
        (
            Transition::IntoFree,
            &[OwnerBufferState::Displaced, OwnerBufferState::Releasing],
        ),
    ];

    fn buffer_for_state(
        state: OwnerBufferState,
        held: super::super::resources::AllocationLease,
        identity: OwnerBufferIdentity,
        drops: &Rc<Cell<usize>>,
        commit: CommitId,
    ) -> OwnerBuffer<TestPendingAck> {
        let rendering = OwnerBuffer::rendering(identity, pending_ack(drops), 20);
        match state {
            OwnerBufferState::Rendering => {
                drop(held);
                rendering
            }
            OwnerBufferState::Desired => rendering
                .into_desired(held)
                .expect("Rendering must enter Desired"),
            OwnerBufferState::Displaced => {
                drop(held);
                rendering
                    .into_displaced()
                    .expect("Rendering must enter Displaced")
            }
            OwnerBufferState::Submitted => {
                let (submitted, returned_lease) = rendering
                    .into_desired(held)
                    .expect("Rendering must enter Desired")
                    .into_submitted(commit)
                    .expect("Desired must enter Submitted");
                drop(returned_lease);
                submitted
            }
            OwnerBufferState::Accepted => {
                let (submitted, returned_lease) = rendering
                    .into_desired(held)
                    .expect("Rendering must enter Desired")
                    .into_submitted(commit)
                    .expect("Desired must enter Submitted");
                drop(returned_lease);
                submitted
                    .into_accepted()
                    .expect("Submitted must enter Accepted")
            }
            OwnerBufferState::Current => {
                let (submitted, returned_lease) = rendering
                    .into_desired(held)
                    .expect("Rendering must enter Desired")
                    .into_submitted(commit)
                    .expect("Desired must enter Submitted");
                drop(returned_lease);
                submitted
                    .into_accepted()
                    .expect("Submitted must enter Accepted")
                    .into_current()
                    .expect("Accepted must enter Current")
            }
            OwnerBufferState::Releasing => {
                let (submitted, returned_lease) = rendering
                    .into_desired(held)
                    .expect("Rendering must enter Desired")
                    .into_submitted(commit)
                    .expect("Desired must enter Submitted");
                drop(returned_lease);
                submitted
                    .into_accepted()
                    .expect("Submitted must enter Accepted")
                    .into_current()
                    .expect("Accepted must enter Current")
                    .into_releasing()
                    .expect("Current must enter Releasing")
            }
            OwnerBufferState::Quarantined => {
                let (submitted, returned_lease) = rendering
                    .into_desired(held)
                    .expect("Rendering must enter Desired")
                    .into_submitted(commit)
                    .expect("Desired must enter Submitted");
                drop(returned_lease);
                submitted
                    .into_quarantined()
                    .expect("Submitted must enter Quarantined")
            }
        }
    }

    fn assert_unchanged(
        buffer: &OwnerBuffer<TestPendingAck>,
        source: OwnerBufferState,
        expected: &OwnerBufferIdentity,
    ) {
        assert_eq!(buffer.state(), source);
        assert_identity(buffer, expected);
        assert_eq!(
            buffer.has_pending_ack(),
            source != OwnerBufferState::Quarantined
        );
        assert_eq!(
            buffer.commit_id().is_some(),
            matches!(
                source,
                OwnerBufferState::Submitted
                    | OwnerBufferState::Accepted
                    | OwnerBufferState::Current
                    | OwnerBufferState::Releasing
            )
        );
        assert_eq!(
            buffer.owns_allocation_lease(),
            matches!(source, OwnerBufferState::Desired)
        );
        assert_eq!(
            buffer.descriptor_slot(),
            match source {
                OwnerBufferState::Rendering | OwnerBufferState::Desired => Some(20),
                OwnerBufferState::Displaced => Some(20),
                OwnerBufferState::Submitted
                | OwnerBufferState::Accepted
                | OwnerBufferState::Current
                | OwnerBufferState::Releasing
                | OwnerBufferState::Quarantined => None,
            }
        );
    }

    #[test]
    fn c0_conv_cir_owner_buffer_legal_transitions() {
        let (mut service, held, _drops) = resources::tests::spy_service();
        let key = held.key();
        let expected = identity(key, 7, 3);
        let commit = CommitId::for_tests(11);
        let drops = Rc::new(Cell::new(0));

        let rendering = OwnerBuffer::rendering(expected.clone(), pending_ack(&drops), 9);
        assert_eq!(rendering.state(), OwnerBufferState::Rendering);
        assert_identity(&rendering, &expected);

        let desired = rendering
            .into_desired(held)
            .expect("Rendering must enter Desired with its retained lease");
        assert_eq!(desired.state(), OwnerBufferState::Desired);
        assert!(desired.owns_allocation_lease());
        assert_identity(&desired, &expected);

        let (submitted, returned_lease) = desired
            .into_submitted(commit)
            .expect("Desired must enter Submitted and return its lease");
        assert_eq!(submitted.state(), OwnerBufferState::Submitted);
        assert_eq!(submitted.commit_id(), Some(commit));
        assert!(!submitted.owns_allocation_lease());
        assert_identity(&submitted, &expected);

        let accepted = submitted
            .into_accepted()
            .expect("Submitted must enter Accepted");
        assert_eq!(accepted.state(), OwnerBufferState::Accepted);
        assert_eq!(accepted.commit_id(), Some(commit));
        assert_identity(&accepted, &expected);

        let current = accepted
            .into_current()
            .expect("Accepted must enter Current");
        assert_eq!(current.state(), OwnerBufferState::Current);
        assert_eq!(current.commit_id(), Some(commit));
        assert_identity(&current, &expected);

        let releasing = current
            .into_releasing()
            .expect("Current must enter Releasing");
        assert_eq!(releasing.state(), OwnerBufferState::Releasing);
        assert_eq!(releasing.commit_id(), Some(commit));
        assert_identity(&releasing, &expected);

        let current = releasing
            .restore_current_after_release_abort()
            .expect("a failed compound release must restore Current");
        assert_eq!(current.state(), OwnerBufferState::Current);
        assert_eq!(current.commit_id(), Some(commit));
        assert_identity(&current, &expected);

        let freed = current
            .into_releasing()
            .expect("Current must enter Releasing")
            .into_free()
            .expect("Releasing must become Free");
        assert_eq!(freed, expected);
        drop(returned_lease);

        let accepted = OwnerBuffer::rendering(expected.clone(), pending_ack(&drops), 9)
            .into_desired(
                service
                    .reserve(key, UseKind::Retain)
                    .expect("test allocation remains available"),
            )
            .expect("Rendering must enter Desired")
            .into_submitted(commit)
            .expect("Desired must enter Submitted")
            .0
            .into_accepted()
            .expect("Submitted must enter Accepted");
        let quarantined = accepted
            .into_quarantined()
            .expect("CompletionUnknown may poison Accepted");
        assert_eq!(quarantined.state(), OwnerBufferState::Quarantined);
        assert_identity(&quarantined, &expected);

        let rendering = OwnerBuffer::rendering(expected.clone(), pending_ack(&drops), 10);
        let displaced = rendering
            .into_displaced()
            .expect("Rendering may be displaced before readiness");
        assert_eq!(displaced.state(), OwnerBufferState::Displaced);
        assert_identity(&displaced, &expected);
        assert_eq!(
            displaced.into_free().expect("Displaced must become Free"),
            expected
        );

        let lease = service
            .reserve(key, UseKind::Retain)
            .expect("test allocation remains available");
        let desired = OwnerBuffer::rendering(expected.clone(), pending_ack(&drops), 11)
            .into_desired(lease)
            .expect("Rendering must enter Desired");
        let displaced = desired
            .into_displaced()
            .expect("Desired may be displaced before admission");
        assert_eq!(displaced.state(), OwnerBufferState::Displaced);
        assert!(displaced.owns_allocation_lease());
        assert_identity(&displaced, &expected);
        assert_eq!(
            displaced.into_free().expect("Displaced must become Free"),
            expected
        );

        let lease = service
            .reserve(key, UseKind::Retain)
            .expect("test allocation remains available");
        let desired = OwnerBuffer::rendering(expected.clone(), pending_ack(&drops), 12)
            .into_desired(lease)
            .expect("Rendering must enter Desired");
        let (submitted, returned_lease) = desired
            .into_submitted(commit)
            .expect("Desired must enter Submitted");
        let desired = submitted
            .into_desired_after_refusal(returned_lease)
            .expect("pre-IPC refusal must restore Desired");
        assert_eq!(desired.state(), OwnerBufferState::Desired);
        assert_identity(&desired, &expected);
        drop(
            desired
                .into_displaced()
                .expect("cleanup test state")
                .into_free(),
        );

        let lease = service
            .reserve(key, UseKind::Retain)
            .expect("test allocation remains available");
        let submitted = OwnerBuffer::rendering(expected.clone(), pending_ack(&drops), 13)
            .into_desired(lease)
            .expect("Rendering must enter Desired")
            .into_submitted(commit)
            .expect("Desired must enter Submitted")
            .0;
        let displaced = submitted
            .into_displaced()
            .expect("a post-IPC rejection must displace Submitted");
        assert_eq!(displaced.state(), OwnerBufferState::Displaced);
        assert_identity(&displaced, &expected);
        assert_eq!(displaced.into_free().expect("cleanup"), expected);
    }

    #[test]
    fn c0_conv_cir_owner_buffer_refuses_illegal_transitions() {
        let legal_cells: usize = TRANSITIONS
            .iter()
            .map(|(_, legal_sources)| legal_sources.len())
            .sum();
        let expected_refusals = ALL_STATES.len() * TRANSITIONS.len() - legal_cells;
        let mut refused_cells = 0;

        for (transition, legal_sources) in TRANSITIONS {
            for source in ALL_STATES {
                if legal_sources.contains(&source) {
                    continue;
                }
                refused_cells += 1;
                let (mut service, held, _resource_drops) = resources::tests::spy_service();
                let key = held.key();
                let expected = identity(key, 8, 4);
                let drops = Rc::new(Cell::new(0));
                let commit = CommitId::for_tests(12);
                let buffer = buffer_for_state(source, held, expected.clone(), &drops, commit);
                let drops_before = drops.get();

                match transition {
                    Transition::IntoDesired => match buffer.into_desired(
                        service
                            .reserve(key, UseKind::Retain)
                            .expect("test allocation remains available"),
                    ) {
                        Ok(_) => panic!("{source:?} accepted into Desired"),
                        Err(returned) => {
                            let (returned, lease) = *returned;
                            assert_unchanged(&returned, source, &expected);
                            assert_eq!(lease.key(), key);
                            assert_eq!(drops.get(), drops_before);
                        }
                    },
                    Transition::IntoDisplaced => match buffer.into_displaced() {
                        Ok(_) => panic!("{source:?} accepted into Displaced"),
                        Err(returned) => {
                            assert_unchanged(&returned, source, &expected);
                            assert_eq!(drops.get(), drops_before);
                        }
                    },
                    Transition::IntoSubmitted => match buffer.into_submitted(commit) {
                        Ok(_) => panic!("{source:?} accepted into Submitted"),
                        Err(returned) => {
                            assert_unchanged(&returned, source, &expected);
                            assert_eq!(drops.get(), drops_before);
                        }
                    },
                    Transition::IntoDesiredAfterRefusal => match buffer.into_desired_after_refusal(
                        service
                            .reserve(key, UseKind::Retain)
                            .expect("test allocation remains available"),
                    ) {
                        Ok(_) => panic!("{source:?} accepted into Desired after refusal"),
                        Err(returned) => {
                            let (returned, lease) = *returned;
                            assert_unchanged(&returned, source, &expected);
                            assert_eq!(lease.key(), key);
                            assert_eq!(drops.get(), drops_before);
                        }
                    },
                    Transition::IntoAccepted => match buffer.into_accepted() {
                        Ok(_) => panic!("{source:?} accepted into Accepted"),
                        Err(returned) => {
                            assert_unchanged(&returned, source, &expected);
                            assert_eq!(drops.get(), drops_before);
                        }
                    },
                    Transition::IntoCurrent => match buffer.into_current() {
                        Ok(_) => panic!("{source:?} accepted into Current"),
                        Err(returned) => {
                            assert_unchanged(&returned, source, &expected);
                            assert_eq!(drops.get(), drops_before);
                        }
                    },
                    Transition::IntoReleasing => match buffer.into_releasing() {
                        Ok(_) => panic!("{source:?} accepted into Releasing"),
                        Err(returned) => {
                            assert_unchanged(&returned, source, &expected);
                            assert_eq!(drops.get(), drops_before);
                        }
                    },
                    Transition::RestoreCurrentAfterReleaseAbort => {
                        match buffer.restore_current_after_release_abort() {
                            Ok(_) => panic!("{source:?} accepted Current restoration"),
                            Err(returned) => {
                                assert_unchanged(&returned, source, &expected);
                                assert_eq!(drops.get(), drops_before);
                            }
                        }
                    }
                    Transition::IntoQuarantined => match buffer.into_quarantined() {
                        Ok(_) => panic!("{source:?} accepted into Quarantined"),
                        Err(returned) => {
                            assert_unchanged(&returned, source, &expected);
                            assert_eq!(drops.get(), drops_before);
                        }
                    },
                    Transition::IntoFree => match buffer.into_free() {
                        Ok(_) => panic!("{source:?} accepted into Free"),
                        Err(returned) => {
                            assert_unchanged(&returned, source, &expected);
                            assert_eq!(drops.get(), drops_before);
                        }
                    },
                }
            }
        }

        assert_eq!(
            (
                ALL_STATES.len(),
                TRANSITIONS.len(),
                legal_cells,
                refused_cells
            ),
            (8, 10, 14, expected_refusals)
        );
    }

    #[test]
    fn c0_conv_cir_quarantine_drops_the_payload() {
        let (_service, held, drops) = resources::tests::spy_service();
        let key = held.key();
        let expected = identity(key, 9, 5);
        let commit = CommitId::for_tests(13);

        let submitted = OwnerBuffer::rendering(expected.clone(), pending_ack(&drops), 31)
            .into_desired(held)
            .expect("Rendering must enter Desired")
            .into_submitted(commit)
            .expect("Desired must enter Submitted")
            .0;
        assert_eq!(submitted.state(), OwnerBufferState::Submitted);
        assert!(submitted.has_pending_ack());
        assert!(!submitted.owns_allocation_lease());
        assert_eq!(submitted.descriptor_slot(), None);
        assert_eq!(submitted.commit_id(), Some(commit));

        let quarantined = submitted
            .into_quarantined()
            .expect("CompletionUnknown must poison the buffer");
        assert_eq!(quarantined.state(), OwnerBufferState::Quarantined);
        assert_identity(&quarantined, &expected);
        assert!(!quarantined.has_pending_ack());
        assert!(!quarantined.owns_allocation_lease());
        assert_eq!(quarantined.descriptor_slot(), None);
        assert_eq!(quarantined.commit_id(), None);
        assert_eq!(drops.get(), 1, "quarantine must drop PendingAck");
    }
}
