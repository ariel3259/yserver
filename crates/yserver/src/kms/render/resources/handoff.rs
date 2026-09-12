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
            ResourceError, ResourceService,
            drm_cleanup::{DrmCleanupRegistry, FileFamilyClosed},
        },
    },
    platform::drm::DrmDeviceKey,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceBarrier {
    FileFamilyClosed(DrmDeviceKey),
    DeviceLost(DrmDeviceKey),
}

impl DeviceBarrier {
    pub(crate) fn from_file_family_closed(closed: &FileFamilyClosed) -> Self {
        Self::FileFamilyClosed(closed.device_key)
    }

    pub(crate) fn device(&self) -> DrmDeviceKey {
        match self {
            Self::FileFamilyClosed(d) | Self::DeviceLost(d) => *d,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsDisposition {
    Outstanding,
    Discharged,
    Superseded(DeviceBarrier),
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
}

impl IncarnationBundle {
    pub(crate) fn new(
        owner: DeviceCommitOwner<CommitResources>,
        resources: ResourceService,
        consumer: CommitResourceConsumer,
        drm: DrmCleanupRegistry,
        executor: Option<crate::kms::executor::KmsIoExecutor>,
        ingress: CompletionIngress,
    ) -> Self {
        Self {
            owner,
            resources,
            consumer,
            drm,
            executor,
            ingress,
        }
    }
}

pub(crate) struct TeardownRelease {
    pub(crate) incarnation: IncarnationId,
    pub(crate) entries: Vec<AllocationKey>,
    _sealed: (),
}

impl TeardownRelease {
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
        bundle: IncarnationBundle,
    ) -> Result<(), (ResourceError, RecipientSlot, IncarnationBundle)> {
        if slot.device != bundle.resources.device()
            || slot.incarnation != bundle.owner.incarnation()
        {
            return Err((ResourceError::WrongIncarnation, slot, bundle));
        }
        if self.recipients.contains_key(&slot.incarnation) {
            return Err((ResourceError::Busy, slot, bundle));
        }
        let incarnation = slot.incarnation;
        let _ = slot; // consumes reservation made before activation
        self.recipients.insert(incarnation, bundle);
        Ok(())
    }

    pub(crate) fn service(&mut self, now: Instant) {
        for bundle in self.recipients.values_mut() {
            let events = std::mem::take(&mut bundle.ingress.pending_events);
            for ev in events {
                let _ = bundle.consumer.consume(ev, &mut bundle.resources);
            }
            if let Ok(available) = bundle.resources.service_completions(now) {
                let _ = bundle
                    .consumer
                    .on_available(&available, &mut bundle.resources);
            }
        }
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
        bundle.ingress.push_descriptor(fd);
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
        RecipientSlot::new(device, incarnation, RecipientReservation::new_for_tests())
    }

    pub(crate) fn issue_teardown_release(
        &self,
        incarnation: IncarnationId,
        entries: Vec<AllocationKey>,
    ) -> TeardownRelease {
        TeardownRelease::mint_for_supervisor(incarnation, entries)
    }
}
