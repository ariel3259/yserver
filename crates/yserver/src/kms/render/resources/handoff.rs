#![allow(dead_code)]
use std::{collections::BTreeMap, os::fd::OwnedFd, time::Instant};

use crate::{
    kms::{
        owner::{
            device::{DeviceCommitOwner, OwnerEvent},
            identity::IncarnationId,
        },
        render::resources::{
            AllocationKey, CommitResourceConsumer, CommitResources, RecipientReservation,
            ResourceError, ResourceService, TransportGate,
            drm_cleanup::{DrmCleanupRegistry, FileFamilyClosed},
        },
    },
    platform::drm::DrmDeviceKey,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceBarrier {
    FileFamilyClosed {
        device: DrmDeviceKey,
        incarnation: IncarnationId,
        _private: (),
    },
    DeviceLost {
        device: DrmDeviceKey,
        _private: (),
    },
}

impl DeviceBarrier {
    pub(crate) fn from_file_family_closed(closed: FileFamilyClosed) -> Self {
        Self::FileFamilyClosed {
            device: closed.device_key,
            incarnation: closed.incarnation,
            _private: (),
        }
    }

    pub(crate) fn from_device_loss(device: DrmDeviceKey, _proof: DeviceLossProof) -> Self {
        Self::DeviceLost {
            device,
            _private: (),
        }
    }

    pub(crate) fn device(&self) -> DrmDeviceKey {
        match self {
            Self::FileFamilyClosed { device, .. } | Self::DeviceLost { device, .. } => *device,
        }
    }

    pub(crate) fn incarnation(&self) -> Option<IncarnationId> {
        match self {
            Self::FileFamilyClosed { incarnation, .. } => Some(*incarnation),
            Self::DeviceLost { .. } => None,
        }
    }
}

pub(crate) struct DeviceLossProof {
    _private: (),
}

impl DeviceLossProof {
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self { _private: () }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsDisposition {
    Outstanding,
    Discharged,
    Superseded(DeviceBarrier),
    /// F-14/S2-m1: a rejected/pre-IPC-cancelled commit's registration was
    /// cancelled, not discharged -- the displacement never happened, so no
    /// completion proof was ever correlated to it (R6). Distinct from
    /// `Discharged` (a real `HardwareComplete` proof was applied) so a test
    /// can observe the difference through `ResourceService::cancel`
    /// (`cancel_pre_ipc_commit`, `ResourcesStillCurrent`, `ResourcesReleased`)
    /// versus `apply_validated_proof` (`discharge_commit_kms_obligations`).
    /// Also closes the stale-disposition bug: `record_device_barrier` only
    /// ever flips an `Outstanding` entry, so a `Cancelled` one is never
    /// mistaken for a still-live obligation and never causes a spurious
    /// `Superseded` flip or dirty mark for a commit that never happened.
    Cancelled,
}

#[derive(Debug, Default)]
pub(crate) struct CompletionIngress {
    pub(crate) pending_events: Vec<OwnerEvent<CommitResources>>,
    pub(crate) returned_descriptors: Vec<OwnedFd>,
}

impl CompletionIngress {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push_event(&mut self, event: OwnerEvent<CommitResources>) {
        self.pending_events.push(event);
    }

    pub(crate) fn push_descriptor(&mut self, fd: OwnedFd) {
        self.returned_descriptors.push(fd);
    }
}

pub(crate) struct RecipientSlot {
    pub(crate) device: DrmDeviceKey,
    pub(crate) incarnation: IncarnationId,
    pub(crate) reservation: RecipientReservation,
}

impl RecipientSlot {
    pub(crate) fn new(
        device: DrmDeviceKey,
        incarnation: IncarnationId,
        reservation: RecipientReservation,
    ) -> Self {
        Self {
            device,
            incarnation,
            reservation,
        }
    }

    pub(crate) fn device(&self) -> DrmDeviceKey {
        self.device
    }

    pub(crate) fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }
}

pub(crate) struct IncarnationBundle {
    pub(crate) owner: DeviceCommitOwner<CommitResources>,
    pub(crate) resources: ResourceService,
    pub(crate) consumer: CommitResourceConsumer,
    pub(crate) drm: DrmCleanupRegistry,
    pub(crate) executor: Option<crate::kms::executor::KmsIoExecutor>,
    pub(crate) ingress: CompletionIngress,
    pub(crate) gate: TransportGate,
}

impl IncarnationBundle {
    pub(crate) fn new(
        owner: DeviceCommitOwner<CommitResources>,
        resources: ResourceService,
        consumer: CommitResourceConsumer,
        drm: DrmCleanupRegistry,
        executor: Option<crate::kms::executor::KmsIoExecutor>,
        ingress: CompletionIngress,
        gate: TransportGate,
    ) -> Self {
        Self {
            owner,
            resources,
            consumer,
            drm,
            executor,
            ingress,
            gate,
        }
    }
}

pub(crate) struct TeardownRelease {
    pub(crate) incarnation: IncarnationId,
    pub(crate) entries: Vec<AllocationKey>,
    _sealed: (),
}

impl TeardownRelease {
    #[cfg(test)]
    pub(crate) fn mint_for_supervisor(
        incarnation: IncarnationId,
        entries: Vec<AllocationKey>,
    ) -> Self {
        Self {
            incarnation,
            entries,
            _sealed: (),
        }
    }
}

#[derive(Default)]
pub(crate) struct HandoffRouter {
    recipients: BTreeMap<IncarnationId, IncarnationBundle>,
}

impl HandoffRouter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn is_registered(&self, incarnation: &IncarnationId) -> bool {
        self.recipients.contains_key(incarnation)
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn transfer(
        &mut self,
        slot: RecipientSlot,
        mut bundle: IncarnationBundle,
    ) -> Result<(), (ResourceError, RecipientSlot, IncarnationBundle)> {
        if slot.device != bundle.resources.device()
            || slot.incarnation != bundle.owner.incarnation()
            || bundle.gate.device() != slot.device
            || bundle.gate.incarnation() != slot.incarnation
        {
            bundle.gate.force_close();
            return Err((ResourceError::WrongIncarnation, slot, bundle));
        }
        if self.recipients.contains_key(&slot.incarnation) {
            bundle.gate.force_close();
            return Err((ResourceError::Busy, slot, bundle));
        }

        // B-7: Revocation precedes close
        let revoked = bundle.gate.revoke_owner_writes();
        if let Err(err) = bundle.gate.close() {
            bundle.gate.force_close();
            return Err((err, slot, bundle));
        }

        // Treat every revoked grant as possibly dispatched: its owner record is Quarantined (round-4 M-2)
        if revoked > 0 {
            let events = bundle.owner.quarantine_live();
            for ev in events {
                if let Err(err) = bundle.consumer.consume(ev, &mut bundle.resources) {
                    bundle.gate.force_close();
                    return Err((err, slot, bundle));
                }
            }
            bundle.consumer.capacity.close_admission();
        }

        let incarnation = slot.incarnation;
        let _ = slot; // consumes reservation made before activation
        self.recipients.insert(incarnation, bundle);
        Ok(())
    }

    pub(crate) fn service(&mut self, now: Instant) -> Result<(), ResourceError> {
        for bundle in self.recipients.values_mut() {
            let events = std::mem::take(&mut bundle.ingress.pending_events);
            for ev in events {
                bundle.consumer.consume(ev, &mut bundle.resources)?;
            }
            let available = bundle.resources.service_completions(now)?;
            bundle
                .consumer
                .on_available(&available, &mut bundle.resources)?;
            // F8-M2: the router's own teardown step. `deliver_descriptor`
            // registers a late returned descriptor under the incident
            // (M-11); nothing else ever closed it, so a recipient that
            // received one could never mint the fd-family barrier. Once the
            // ingress just drained above carries no more events and the
            // helper is reaped, no further late reply can arrive on this
            // incident, so whatever has accumulated so far is safe to
            // close. Idempotent when nothing is pending.
            if bundle.drm.helper_reaped() {
                bundle.drm.close_returned_descriptors();
            }
        }
        Ok(())
    }

    pub(crate) fn deliver_event(
        &mut self,
        incarnation: IncarnationId,
        event: OwnerEvent<CommitResources>,
    ) -> Result<(), ResourceError> {
        let bundle = self
            .recipients
            .get_mut(&incarnation)
            .ok_or(ResourceError::Detached)?;
        bundle.ingress.push_event(event);
        Ok(())
    }

    pub(crate) fn deliver_descriptor(
        &mut self,
        incarnation: IncarnationId,
        fd: OwnedFd,
    ) -> Result<(), ResourceError> {
        let bundle = self
            .recipients
            .get_mut(&incarnation)
            .ok_or(ResourceError::Detached)?;
        bundle.drm.register_returned_descriptor(fd);
        Ok(())
    }

    pub(crate) fn get_bundle_mut(
        &mut self,
        incarnation: &IncarnationId,
    ) -> Option<&mut IncarnationBundle> {
        self.recipients.get_mut(incarnation)
    }
}

// B-6: `RetainingSupervisor` is a test fixture (the plan's own words) --
// `reserve_slot` below is the production code that needed
// `RecipientReservation::new_for_tests` to be reachable outside
// `#[cfg(test)]`, which was the actual bug (R8: no production
// `RecipientReservation`). The fixture, not the constructor, moves under
// `#[cfg(test)]`.
#[cfg(test)]
pub(crate) struct RetainingSupervisor {
    pub(crate) router: HandoffRouter,
}

#[cfg(test)]
impl Default for RetainingSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl RetainingSupervisor {
    pub(crate) fn new() -> Self {
        Self {
            router: HandoffRouter::new(),
        }
    }

    pub(crate) fn reserve_slot(
        &self,
        device: DrmDeviceKey,
        incarnation: IncarnationId,
    ) -> RecipientSlot {
        RecipientSlot::new(
            device,
            incarnation,
            RecipientReservation::new_for_tests(device, incarnation),
        )
    }

    pub(crate) fn issue_teardown_release(
        &self,
        incarnation: IncarnationId,
        entries: Vec<AllocationKey>,
    ) -> TeardownRelease {
        TeardownRelease::mint_for_supervisor(incarnation, entries)
    }
}
