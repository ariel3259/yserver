//! Pure global lifecycle projection and protocol DPMS aggregation.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    ArbiterInput, ClientModesetTag, DesiredField, DesiredIntent, Disposition, IncidentOrigin,
    LifecycleAction, LifecycleArbiter, LifecycleEventId, LifecycleKind, OutputProjection,
    OutputProjectionRemoval, RecoveryAttemptOutcome, RecoveryIdAllocator, SeatTarget,
    TransitionTag, dpms_target_for_level,
};

/// Result of one coordinator-assigned event projection to a device.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CoordinatorDispatch<D, I> {
    pub device: D,
    pub event_id: LifecycleEventId,
    pub actions: Vec<LifecycleAction<I>>,
}

/// Actions caused by an external prerequisite observation that has no
/// lifecycle event identity of its own.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CoordinatorActionDispatch<D, I> {
    pub device: D,
    pub actions: Vec<LifecycleAction<I>>,
}

/// New output target plus any device synchronization event needed to inherit
/// the coordinator's current global DPMS state.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CoordinatorOutputAddition<D, I> {
    pub projection: Option<OutputProjection>,
    pub synchronization: Option<CoordinatorDispatch<D, I>>,
}

/// Failure to assign or route a coordinator-owned lifecycle input.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CoordinatorError {
    EventIdExhausted,
    EpochExhausted,
    RecoveryIdExhausted,
    UnknownDevice,
    DuplicateDevice,
    InvalidDpmsLevel(u8),
    GlobalIntentRequiresCoordinator,
    CoordinatorOwnedInput,
}

#[derive(Debug, Eq, PartialEq)]
struct CoordinatorDevice<O, I> {
    arbiter: LifecycleArbiter<O, I>,
    recovery_ids: RecoveryIdAllocator,
}

#[derive(Debug, Eq, PartialEq)]
struct DpmsRequest<D> {
    level: u8,
    epoch: u64,
    representatives: BTreeMap<D, LifecycleEventId>,
    terminal_outcomes: BTreeMap<D, Disposition>,
    removed_devices: BTreeSet<D>,
}

/// The server-wide lifecycle coordinator. Device arbiters remain independent;
/// this type owns global targets and the only `LifecycleEventId` allocator.
#[derive(Debug, Eq, PartialEq)]
pub struct LifecycleCoordinator<D, O, I> {
    shutdown_requested: bool,
    seat_target: Option<SeatTarget>,
    seat_epoch: u64,
    seat_target_from_read_only_feed: bool,
    protocol_dpms_level: u8,
    dpms_epoch: u64,
    next_event_id: Option<LifecycleEventId>,
    devices: BTreeMap<D, CoordinatorDevice<O, I>>,
    dpms_request: Option<DpmsRequest<D>>,
}

impl<D: Ord + Clone, O: Ord + Clone, I: Clone + Eq> Default for LifecycleCoordinator<D, O, I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Ord + Clone, O: Ord + Clone, I: Clone + Eq> LifecycleCoordinator<D, O, I> {
    pub fn new() -> Self {
        Self {
            shutdown_requested: false,
            seat_target: None,
            seat_epoch: 0,
            seat_target_from_read_only_feed: false,
            protocol_dpms_level: 0,
            dpms_epoch: 0,
            next_event_id: Some(LifecycleEventId::first()),
            devices: BTreeMap::new(),
            dpms_request: None,
        }
    }

    pub const fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    pub const fn seat_target(&self) -> Option<SeatTarget> {
        self.seat_target
    }

    pub const fn seat_epoch(&self) -> u64 {
        self.seat_epoch
    }

    pub const fn protocol_dpms_level(&self) -> u8 {
        self.protocol_dpms_level
    }

    pub const fn dpms_epoch(&self) -> u64 {
        self.dpms_epoch
    }

    pub fn device(&self, key: &D) -> Option<&LifecycleArbiter<O, I>> {
        self.devices.get(key).map(|entry| &entry.arbiter)
    }

    /// Register an independent device arbiter. Existing global seat and
    /// shutdown targets are immediately projected to the new device.
    pub fn add_device(
        &mut self,
        key: D,
        arbiter: LifecycleArbiter<O, I>,
    ) -> Result<Vec<CoordinatorDispatch<D, I>>, CoordinatorError> {
        if self.devices.contains_key(&key) {
            return Err(CoordinatorError::DuplicateDevice);
        }
        self.devices.insert(
            key.clone(),
            CoordinatorDevice {
                arbiter,
                recovery_ids: RecoveryIdAllocator::new(),
            },
        );

        let mut result = Vec::new();
        if self.shutdown_requested {
            result.push(self.project_with_new_id(key, DesiredIntent::Shutdown)?);
        } else if let Some(target) = self.seat_target {
            if self.seat_target_from_read_only_feed {
                self.devices
                    .get_mut(&key)
                    .expect("device was inserted")
                    .arbiter
                    .observe_seat_target(target, self.seat_epoch);
            } else {
                result.push(self.project_with_new_id(
                    key,
                    DesiredIntent::Seat {
                        target,
                        epoch: self.seat_epoch,
                    },
                )?);
            }
        }
        Ok(result)
    }

    /// Add a stable protocol output. If it joins after a global DPMS request,
    /// first project that request's current level and epoch to its device.
    pub fn add_protocol_output(
        &mut self,
        device: &D,
        output: O,
    ) -> Result<CoordinatorOutputAddition<D, I>, CoordinatorError> {
        let entry = self
            .devices
            .get(device)
            .ok_or(CoordinatorError::UnknownDevice)?;
        if entry.arbiter.desired().dpms_targets().contains_key(&output) {
            return Ok(CoordinatorOutputAddition {
                projection: None,
                synchronization: None,
            });
        }

        let current_representative = self
            .dpms_request
            .as_ref()
            .and_then(|request| request.representatives.get(device).copied());
        let current_representative_is_terminal = current_representative.is_some_and(|event_id| {
            entry
                .arbiter
                .desired()
                .representative(DesiredField::Dpms)
                .is_some_and(|representative| {
                    representative.event_id == event_id
                        && representative
                            .disposition
                            .is_some_and(Disposition::is_terminal)
                })
        });
        let needs_dpms_projection = self.dpms_epoch != 0
            && (current_representative.is_none() || current_representative_is_terminal);

        let mut sync_dispatch = None;
        if needs_dpms_projection {
            let event_id = self.allocate_event_id()?;
            if let Some(request) = self.dpms_request.as_mut()
                && request.epoch == self.dpms_epoch
            {
                request.representatives.insert(device.clone(), event_id);
                request.terminal_outcomes.remove(device);
                request.removed_devices.remove(device);
            }
            let actions = self.project_existing_id(
                device,
                event_id,
                DesiredIntent::Dpms {
                    level: self.protocol_dpms_level,
                    epoch: self.dpms_epoch,
                },
            )?;
            sync_dispatch = Some(CoordinatorDispatch {
                device: device.clone(),
                event_id,
                actions,
            });
        }

        let entry = self
            .devices
            .get_mut(device)
            .ok_or(CoordinatorError::UnknownDevice)?;
        let projection = entry.arbiter.add_protocol_output(output);
        if projection.is_some_and(|projection| {
            self.dpms_request
                .as_ref()
                .and_then(|request| request.representatives.get(device))
                == projection.representative.as_ref()
        }) && let Some(request) = self.dpms_request.as_mut()
        {
            request.removed_devices.remove(device);
        }
        Ok(CoordinatorOutputAddition {
            projection,
            synchronization: sync_dispatch,
        })
    }

    /// Commit the output membership installed by a client modeset. A DPMS
    /// event may have arrived after dispatch, so the protocol projection is
    /// taken from the coordinator's current desired state; the accepted KMS
    /// state is promoted separately from the prepared modeset description.
    /// The device and output membership were verified before dispatch.
    pub fn commit_staged_protocol_output(&mut self, device: &D, output: O) {
        let current_representative = self
            .dpms_request
            .as_ref()
            .and_then(|request| request.representatives.get(device).copied());
        let entry = self
            .devices
            .get_mut(device)
            .expect("staged output's lifecycle device was verified before dispatch");
        let projection =
            if let Some(projection) = entry.arbiter.desired().dpms_targets().get(&output) {
                *projection
            } else {
                entry
                    .arbiter
                    .add_protocol_output(output.clone())
                    .expect("staged output projection is added exactly once")
            };
        if projection.representative == current_representative
            && self
                .dpms_request
                .as_ref()
                .and_then(|request| request.representatives.get(device))
                == projection.representative.as_ref()
            && let Some(request) = self.dpms_request.as_mut()
        {
            request.removed_devices.remove(device);
        }
    }

    /// Remove one protocol output and remember its current request projection
    /// as invalidated only when that device has no remaining projection for it.
    pub fn remove_protocol_output(
        &mut self,
        device: &D,
        output: &O,
    ) -> Result<Option<OutputProjectionRemoval>, CoordinatorError> {
        let entry = self
            .devices
            .get_mut(device)
            .ok_or(CoordinatorError::UnknownDevice)?;
        let removal = entry.arbiter.remove_protocol_output(output);
        let Some(removal) = removal else {
            return Ok(None);
        };

        if let Some(request) = self.dpms_request.as_mut()
            && let Some(event_id) = request.representatives.get(device).copied()
            && removal.projection.representative == Some(event_id)
            && !entry
                .arbiter
                .desired()
                .dpms_targets()
                .values()
                .any(|projection| projection.representative == Some(event_id))
        {
            request.removed_devices.insert(device.clone());
        }
        Ok(Some(removal))
    }

    /// Infallibly invalidate a protocol output projection after the KMS
    /// disable that removed it. The target's presence is checked while the
    /// modeset is prepared, before the kernel can accept the transaction.
    pub fn invalidate_staged_protocol_output(&mut self, device: &D, output: &O) {
        let entry = self
            .devices
            .get_mut(device)
            .expect("removed output's lifecycle device was verified before dispatch");
        let removal = entry
            .arbiter
            .remove_protocol_output(output)
            .expect("staged output projection is invalidated exactly once");

        if let Some(request) = self.dpms_request.as_mut()
            && let Some(event_id) = request.representatives.get(device).copied()
            && removal.projection.representative == Some(event_id)
            && !entry
                .arbiter
                .desired()
                .dpms_targets()
                .values()
                .any(|projection| projection.representative == Some(event_id))
        {
            request.removed_devices.insert(device.clone());
        }
    }

    /// Accept a shutdown request. The global bit is monotonic and is updated
    /// before any per-device arbiter receives the event.
    pub fn request_shutdown(&mut self) -> Result<Vec<CoordinatorDispatch<D, I>>, CoordinatorError> {
        let keys: Vec<_> = self.devices.keys().cloned().collect();
        let event_ids = self.reserve_event_ids(keys.len())?;
        self.shutdown_requested = true;
        keys.into_iter()
            .zip(event_ids)
            .map(|(key, event_id)| {
                self.project_existing_id(&key, event_id, DesiredIntent::Shutdown)
                    .map(|actions| CoordinatorDispatch {
                        device: key,
                        event_id,
                        actions,
                    })
            })
            .collect()
    }

    /// Replace the global seat target and project the same generation to every
    /// device. Each device gets its own coordinator-allocated representative.
    pub fn set_seat_target(
        &mut self,
        target: SeatTarget,
    ) -> Result<Vec<CoordinatorDispatch<D, I>>, CoordinatorError> {
        let epoch = self
            .seat_epoch
            .checked_add(1)
            .ok_or(CoordinatorError::EpochExhausted)?;
        let keys: Vec<_> = self.devices.keys().cloned().collect();
        let event_ids = self.reserve_event_ids(keys.len())?;
        self.seat_target = Some(target);
        self.seat_epoch = epoch;
        self.seat_target_from_read_only_feed = false;
        keys.into_iter()
            .zip(event_ids)
            .map(|(key, event_id)| {
                self.project_existing_id(&key, event_id, DesiredIntent::Seat { target, epoch })
                    .map(|actions| CoordinatorDispatch {
                        device: key,
                        event_id,
                        actions,
                    })
            })
            .collect()
    }

    /// Accept one global X11 DPMS level. Levels 1–3 are off and level 0 is on;
    /// all affected devices receive the same epoch and one representative.
    pub fn set_protocol_dpms_level(
        &mut self,
        level: u8,
    ) -> Result<Vec<CoordinatorDispatch<D, I>>, CoordinatorError> {
        if dpms_target_for_level(level).is_none() {
            return Err(CoordinatorError::InvalidDpmsLevel(level));
        }
        let epoch = self
            .dpms_epoch
            .checked_add(1)
            .ok_or(CoordinatorError::EpochExhausted)?;
        let keys: Vec<_> = self
            .devices
            .iter()
            .filter(|(_, entry)| !entry.arbiter.desired().dpms_targets().is_empty())
            .map(|(key, _)| key.clone())
            .collect();
        let event_ids = self.reserve_event_ids(keys.len())?;

        self.protocol_dpms_level = level;
        self.dpms_epoch = epoch;
        self.dpms_request = Some(DpmsRequest {
            level,
            epoch,
            representatives: BTreeMap::new(),
            terminal_outcomes: BTreeMap::new(),
            removed_devices: BTreeSet::new(),
        });

        keys.into_iter()
            .zip(event_ids)
            .map(|(key, event_id)| {
                self.dpms_request
                    .as_mut()
                    .expect("request was installed before projection")
                    .representatives
                    .insert(key.clone(), event_id);
                let actions =
                    self.project_existing_id(&key, event_id, DesiredIntent::Dpms { level, epoch })?;
                Ok(CoordinatorDispatch {
                    device: key,
                    event_id,
                    actions,
                })
            })
            .collect()
    }

    /// Project a device-local lifecycle intent. Global shutdown, seat, DPMS,
    /// and normal-recovery inputs have coordinator-owned entry points.
    pub fn project_device_intent(
        &mut self,
        device: &D,
        intent: DesiredIntent,
    ) -> Result<CoordinatorDispatch<D, I>, CoordinatorError> {
        if matches!(
            intent,
            DesiredIntent::Shutdown
                | DesiredIntent::Seat { .. }
                | DesiredIntent::Dpms { .. }
                | DesiredIntent::NormalRecovery { .. }
        ) {
            return Err(CoordinatorError::GlobalIntentRequiresCoordinator);
        }
        self.project_with_new_id(device.clone(), intent)
    }

    /// Route a driver-reported completion loss through Table U. The loss gets
    /// exactly one new event id here and reaches only its reported device.
    pub fn report_completion_loss(
        &mut self,
        device: &D,
    ) -> Result<CoordinatorDispatch<D, I>, CoordinatorError> {
        let (kind, existing_boundary_id) = {
            let entry = self
                .devices
                .get(device)
                .ok_or(CoordinatorError::UnknownDevice)?;
            let kind = entry.arbiter.transition().map(|transition| transition.kind);
            let boundary_id = entry.arbiter.recovery().and_then(|incident| {
                matches!(incident.origin(), IncidentOrigin::Boundary).then_some(incident.id())
            });
            (kind, boundary_id)
        };
        let boundary_recovery_id = if kind.is_some_and(is_recovery_boundary) {
            match existing_boundary_id {
                Some(id) => Some(id),
                None => Some(
                    self.devices
                        .get_mut(device)
                        .expect("device was checked above")
                        .recovery_ids
                        .allocate()
                        .ok_or(CoordinatorError::RecoveryIdExhausted)?,
                ),
            }
        } else {
            None
        };
        let event_id = self.allocate_event_id()?;
        let actions = self.apply_input(
            device,
            ArbiterInput::CompletionUnknown {
                event_id,
                boundary_recovery_id,
            },
        )?;
        Ok(CoordinatorDispatch {
            device: device.clone(),
            event_id,
            actions,
        })
    }

    /// Record seat ownership observed by the existing Legacy VT path. This
    /// updates only the prerequisite snapshot, then lets pending work such as
    /// DPMS converge when ownership returns. It does not create a VT event or
    /// ask the driver to execute VT policy.
    pub fn observe_seat_target(
        &mut self,
        target: SeatTarget,
    ) -> Result<Vec<CoordinatorActionDispatch<D, I>>, CoordinatorError> {
        let epoch = self
            .seat_epoch
            .checked_add(1)
            .ok_or(CoordinatorError::EpochExhausted)?;
        self.seat_target = Some(target);
        self.seat_epoch = epoch;
        self.seat_target_from_read_only_feed = true;

        let keys: Vec<_> = self.devices.keys().cloned().collect();
        keys.into_iter()
            .map(|key| {
                let actions =
                    self.apply_input(&key, ArbiterInput::SeatTargetObserved { target, epoch })?;
                Ok(CoordinatorActionDispatch {
                    device: key,
                    actions,
                })
            })
            .collect()
    }

    /// Feed acknowledged driver/executor inputs to one arbiter. Event and
    /// completion-loss inputs stay coordinator-owned.
    pub fn apply_device_input(
        &mut self,
        device: &D,
        input: ArbiterInput<I>,
    ) -> Result<Vec<LifecycleAction<I>>, CoordinatorError> {
        if matches!(
            &input,
            ArbiterInput::LifecycleEvent { .. }
                | ArbiterInput::CompletionUnknown { .. }
                | ArbiterInput::RecoveryIncidentAllocated { .. }
                | ArbiterInput::SeatTargetObserved { .. }
        ) {
            return Err(CoordinatorError::CoordinatorOwnedInput);
        }
        self.apply_input(device, input)
    }

    /// Tell the arbiter that the one authorized attempt started or resolved.
    pub fn recovery_attempt(
        &mut self,
        device: &D,
        tag: TransitionTag<I>,
        outcome: RecoveryAttemptOutcome,
    ) -> Result<Vec<LifecycleAction<I>>, CoordinatorError> {
        self.apply_device_input(device, ArbiterInput::RecoveryAttempt { tag, outcome })
    }

    /// Mark one client modeset as dispatched on its device. Returns false if
    /// the captured lifecycle context is no longer current or a transition is
    /// already active.
    pub fn client_modeset_submitting(
        &mut self,
        device: &D,
        tag: &ClientModesetTag<I>,
    ) -> Result<bool, CoordinatorError> {
        let entry = self
            .devices
            .get_mut(device)
            .ok_or(CoordinatorError::UnknownDevice)?;
        Ok(entry.arbiter.client_modeset_submitting(tag))
    }

    /// Resolve a dispatched client modeset at the Owner result boundary and
    /// converge any REC-4 events projected while it was in flight.
    pub fn client_modeset_resolved(
        &mut self,
        device: &D,
        tag: &ClientModesetTag<I>,
    ) -> Result<Vec<LifecycleAction<I>>, CoordinatorError> {
        let entry = self
            .devices
            .get_mut(device)
            .ok_or(CoordinatorError::UnknownDevice)?;
        Ok(entry.arbiter.client_modeset_resolved(tag))
    }

    /// The latest protocol DPMS request is complete only if every device
    /// representative applied, or its entire current projection was removed.
    pub fn protocol_dpms_request_applied(&self) -> bool {
        let Some(request) = &self.dpms_request else {
            return true;
        };
        debug_assert!(request.level <= 3);
        request.representatives.iter().all(|(device, event_id)| {
            let Some(entry) = self.devices.get(device) else {
                return false;
            };
            if let Some(disposition) = request.terminal_outcomes.get(device) {
                return matches!(disposition, Disposition::Applied(_))
                    || *disposition
                        == Disposition::Invalidated(
                            super::InvalidationReason::ProtocolOutputRemoved,
                        );
            }
            let representative = entry.arbiter.desired().representative(DesiredField::Dpms);
            if representative.is_some_and(|representative| {
                representative.event_id == *event_id
                    && matches!(representative.disposition, Some(Disposition::Applied(_)))
            }) {
                return true;
            }
            if representative.is_some_and(|representative| {
                representative.event_id == *event_id
                    && representative.disposition
                        == Some(Disposition::Invalidated(
                            super::InvalidationReason::ProtocolOutputRemoved,
                        ))
            }) {
                return true;
            }
            request.removed_devices.contains(device)
                && !entry
                    .arbiter
                    .desired()
                    .dpms_targets()
                    .values()
                    .any(|projection| projection.representative == Some(*event_id))
        })
    }

    fn project_with_new_id(
        &mut self,
        device: D,
        intent: DesiredIntent,
    ) -> Result<CoordinatorDispatch<D, I>, CoordinatorError> {
        let event_id = self.allocate_event_id()?;
        let actions = self.project_existing_id(&device, event_id, intent)?;
        Ok(CoordinatorDispatch {
            device,
            event_id,
            actions,
        })
    }

    fn project_existing_id(
        &mut self,
        device: &D,
        event_id: LifecycleEventId,
        intent: DesiredIntent,
    ) -> Result<Vec<LifecycleAction<I>>, CoordinatorError> {
        self.apply_input(device, ArbiterInput::LifecycleEvent { event_id, intent })
    }

    fn apply_input(
        &mut self,
        device: &D,
        input: ArbiterInput<I>,
    ) -> Result<Vec<LifecycleAction<I>>, CoordinatorError> {
        let entry = self
            .devices
            .get_mut(device)
            .ok_or(CoordinatorError::UnknownDevice)?;
        let mut actions = entry.arbiter.apply(input);
        let mut index = 0;
        while index < actions.len() {
            let allocation = match actions.get(index) {
                Some(LifecycleAction::AllocateRecoveryIncident { event_id, seed }) => {
                    Some((*event_id, *seed))
                }
                _ => None,
            };
            if let Some((event_id, seed)) = allocation {
                let incident = entry
                    .recovery_ids
                    .allocate_incident(seed)
                    .ok_or(CoordinatorError::RecoveryIdExhausted)?;
                actions.extend(
                    entry
                        .arbiter
                        .apply(ArbiterInput::RecoveryIncidentAllocated { event_id, incident }),
                );
            }
            index += 1;
        }
        let current_dpms_event = self
            .dpms_request
            .as_ref()
            .and_then(|request| request.representatives.get(device).copied());
        if let (Some(event_id), Some(request)) = (current_dpms_event, self.dpms_request.as_mut()) {
            for action in &actions {
                if let LifecycleAction::DispositionChanged {
                    event_id: changed_event,
                    disposition,
                } = action
                    && *changed_event == event_id
                    && disposition.is_terminal()
                {
                    request
                        .terminal_outcomes
                        .insert(device.clone(), *disposition);
                }
            }
        }
        Ok(actions)
    }

    fn allocate_event_id(&mut self) -> Result<LifecycleEventId, CoordinatorError> {
        let event_id = self
            .next_event_id
            .ok_or(CoordinatorError::EventIdExhausted)?;
        self.next_event_id = event_id.checked_next();
        Ok(event_id)
    }

    fn reserve_event_ids(
        &mut self,
        count: usize,
    ) -> Result<Vec<LifecycleEventId>, CoordinatorError> {
        let mut next = self.next_event_id;
        let mut ids = Vec::with_capacity(count);
        for _ in 0..count {
            let event_id = next.ok_or(CoordinatorError::EventIdExhausted)?;
            ids.push(event_id);
            next = event_id.checked_next();
        }
        self.next_event_id = next;
        Ok(ids)
    }
}

fn is_recovery_boundary(kind: LifecycleKind) -> bool {
    matches!(
        kind,
        LifecycleKind::DeviceAddedOrReplaced
            | LifecycleKind::VTAcquire
            | LifecycleKind::AdministrativeReprobe
            | LifecycleKind::IdentityChangingHotplug
    )
}

#[cfg(test)]
mod tests {
    use super::{CoordinatorError, LifecycleCoordinator, dpms_target_for_level};
    use crate::kms::owner::lifecycle::{
        ArbiterInput, CompletionUnknownRow, CompletionUnknownRowKind, DesiredField, DesiredIntent,
        DeviceLifecycleState, Disposition, DpmsTarget, IncidentOrigin, LifecycleAction,
        LifecycleArbiter, LifecycleCommitOutcome, LifecycleEventId, LifecycleKind,
        LifecycleReceipt, LifecycleReceiptResult, RecoveryAttemptOutcome, RecoveryAttemptTrigger,
        RecoveryId, RecoveryIncidentState, SeatTarget, TopologyChangeClass, TransitionTag, table_u,
    };

    type Coordinator = LifecycleCoordinator<u32, u32, u32>;
    type Action = LifecycleAction<u32>;

    fn with_devices(device_ids: &[u32]) -> Coordinator {
        let mut coordinator = Coordinator::new();
        for device in device_ids {
            coordinator
                .add_device(*device, LifecycleArbiter::new(*device))
                .unwrap();
        }
        coordinator
    }

    fn add_output(coordinator: &mut Coordinator, device: u32, output: u32) {
        let addition = coordinator.add_protocol_output(&device, output).unwrap();
        assert!(addition.synchronization.is_none());
        assert!(addition.projection.is_some());
    }

    fn acknowledge_receipts(coordinator: &mut Coordinator, device: u32) -> TransitionTag<u32> {
        let tag = coordinator
            .device(&device)
            .and_then(LifecycleArbiter::transition_tag)
            .expect("device has an active transition");
        for receipt in LifecycleReceipt::ALL {
            coordinator
                .apply_device_input(
                    &device,
                    ArbiterInput::Receipt {
                        tag,
                        receipt,
                        result: LifecycleReceiptResult::Succeeded,
                    },
                )
                .unwrap();
        }
        tag
    }

    fn finish_transition(
        coordinator: &mut Coordinator,
        device: u32,
        outcome: LifecycleCommitOutcome,
    ) -> TransitionTag<u32> {
        let tag = acknowledge_receipts(coordinator, device);
        coordinator
            .apply_device_input(&device, ArbiterInput::CommitOutcome { tag, outcome })
            .unwrap();
        tag
    }

    fn row_setup(kind: CompletionUnknownRowKind, dpms_level: u8) -> Coordinator {
        let mut coordinator = with_devices(&[1]);
        match kind {
            CompletionUnknownRowKind::NormalLive => {}
            CompletionUnknownRowKind::VTRelease => {
                coordinator.set_seat_target(SeatTarget::Released).unwrap();
            }
            CompletionUnknownRowKind::DeviceRemoved => {
                coordinator
                    .project_device_intent(
                        &1,
                        DesiredIntent::DevicePresence {
                            present: false,
                            identity_epoch: 1,
                        },
                    )
                    .unwrap();
            }
            CompletionUnknownRowKind::Shutdown => {
                coordinator.request_shutdown().unwrap();
            }
            CompletionUnknownRowKind::DeviceAddedOrReplaced => {
                coordinator.set_seat_target(SeatTarget::Owned).unwrap();
                coordinator
                    .project_device_intent(
                        &1,
                        DesiredIntent::DevicePresence {
                            present: true,
                            identity_epoch: 1,
                        },
                    )
                    .unwrap();
            }
            CompletionUnknownRowKind::VTAcquire => {
                coordinator
                    .project_device_intent(
                        &1,
                        DesiredIntent::DevicePresence {
                            present: true,
                            identity_epoch: 1,
                        },
                    )
                    .unwrap();
                coordinator.set_seat_target(SeatTarget::Owned).unwrap();
            }
            CompletionUnknownRowKind::AdministrativeReprobe => {
                coordinator
                    .project_device_intent(&1, DesiredIntent::AdministrativeReprobe { epoch: 1 })
                    .unwrap();
            }
            CompletionUnknownRowKind::IdentityChangingHotplug => {
                coordinator
                    .project_device_intent(
                        &1,
                        DesiredIntent::Topology {
                            discovery_epoch: 1,
                            change_class: TopologyChangeClass::IdentityChanging,
                        },
                    )
                    .unwrap();
            }
            CompletionUnknownRowKind::TopologyRebuild => {
                coordinator
                    .project_device_intent(
                        &1,
                        DesiredIntent::Topology {
                            discovery_epoch: 1,
                            change_class: TopologyChangeClass::SameIdentity,
                        },
                    )
                    .unwrap();
            }
            CompletionUnknownRowKind::DPMS => {
                add_output(&mut coordinator, 1, 10);
                coordinator.set_protocol_dpms_level(dpms_level).unwrap();
            }
        }
        coordinator
    }

    fn row_with_id(kind: CompletionUnknownRowKind, dpms_level: u8) -> CompletionUnknownRow {
        match kind {
            CompletionUnknownRowKind::NormalLive => CompletionUnknownRow::NormalLive,
            CompletionUnknownRowKind::VTRelease => CompletionUnknownRow::VTRelease,
            CompletionUnknownRowKind::DeviceRemoved => CompletionUnknownRow::DeviceRemoved,
            CompletionUnknownRowKind::Shutdown => CompletionUnknownRow::Shutdown,
            CompletionUnknownRowKind::DeviceAddedOrReplaced => {
                CompletionUnknownRow::DeviceAddedOrReplaced {
                    recovery_id: RecoveryId::from_raw(1),
                }
            }
            CompletionUnknownRowKind::VTAcquire => CompletionUnknownRow::VTAcquire {
                recovery_id: RecoveryId::from_raw(1),
            },
            CompletionUnknownRowKind::AdministrativeReprobe => {
                CompletionUnknownRow::AdministrativeReprobe {
                    recovery_id: RecoveryId::from_raw(1),
                }
            }
            CompletionUnknownRowKind::IdentityChangingHotplug => {
                CompletionUnknownRow::IdentityChangingHotplug {
                    recovery_id: RecoveryId::from_raw(1),
                }
            }
            CompletionUnknownRowKind::TopologyRebuild => CompletionUnknownRow::TopologyRebuild,
            CompletionUnknownRowKind::DPMS => CompletionUnknownRow::DPMS {
                target: dpms_target_for_level(dpms_level).expect("valid test DPMS level"),
            },
        }
    }

    fn complete_recovery_attempt(
        coordinator: &mut Coordinator,
        result: RecoveryAttemptOutcome,
    ) -> (RecoveryId, TransitionTag<u32>) {
        let tag = acknowledge_receipts(coordinator, 1);
        let recovery_id = match coordinator.device(&1).unwrap().recovery() {
            Some(incident) => incident.id(),
            None => panic!("Table U created a recovery incident"),
        };
        coordinator
            .recovery_attempt(
                &1,
                tag,
                RecoveryAttemptOutcome::Started(RecoveryAttemptTrigger::AfterReap),
            )
            .unwrap();
        assert_eq!(
            coordinator.device(&1).unwrap().state(),
            DeviceLifecycleState::Recovering(recovery_id)
        );
        coordinator.recovery_attempt(&1, tag, result).unwrap();
        (recovery_id, tag)
    }

    #[test]
    fn c0_3a_dpms_projects_one_epoch_to_every_device() {
        let mut coordinator = with_devices(&[1, 2, 3]);
        for device in 1..=3 {
            add_output(&mut coordinator, device, device * 10);
            add_output(&mut coordinator, device, device * 10 + 1);
        }

        for level in 1..=3 {
            let dispatches = coordinator.set_protocol_dpms_level(level).unwrap();
            let epoch = coordinator.dpms_epoch();
            assert_eq!(epoch, u64::from(level));
            assert_eq!(dispatches.len(), 3);
            assert_eq!(
                dispatches
                    .iter()
                    .map(|dispatch| dispatch.event_id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                3,
                "one coordinator id per device"
            );
            for device in 1..=3 {
                let added = coordinator
                    .add_protocol_output(&device, device * 100 + u32::from(level))
                    .unwrap()
                    .projection
                    .unwrap();
                assert_eq!(added.level, level);
                assert_eq!(added.epoch, Some(epoch));
                let arbiter = coordinator.device(&device).unwrap();
                let representative = arbiter
                    .desired()
                    .representative(DesiredField::Dpms)
                    .unwrap();
                assert_eq!(representative.kind, LifecycleKind::DPMS);
                assert_eq!(
                    arbiter.desired().dpms_targets().len(),
                    2 + usize::from(level),
                    "both outputs share the device representative"
                );
                for projection in arbiter.desired().dpms_targets().values() {
                    assert_eq!(projection.level, level);
                    assert_eq!(projection.epoch, Some(epoch));
                    assert_eq!(projection.representative, Some(representative.event_id));
                    assert_eq!(
                        dpms_target_for_level(projection.level),
                        Some(DpmsTarget::Off),
                        "levels 1, 2, and 3 all request off"
                    );
                }
            }
        }
    }

    #[test]
    fn c0_3aii_one_dpms_level_mapping() {
        for (level, expected) in [
            (0, DpmsTarget::On),
            (1, DpmsTarget::Off),
            (2, DpmsTarget::Off),
            (3, DpmsTarget::Off),
        ] {
            assert_eq!(dpms_target_for_level(level), Some(expected));

            let mut coordinator = with_devices(&[1]);
            add_output(&mut coordinator, 1, 10);
            coordinator.set_protocol_dpms_level(level).unwrap();
            let projected = coordinator
                .device(&1)
                .unwrap()
                .desired()
                .dpms_targets()
                .get(&10)
                .expect("projected output");
            assert_eq!(dpms_target_for_level(projected.level), Some(expected));

            let loss = coordinator.report_completion_loss(&1).unwrap();
            assert!(loss.actions.iter().any(|action| matches!(
                action,
                LifecycleAction::CompletionLossTableU {
                    row: CompletionUnknownRow::DPMS { target },
                    ..
                } if *target == expected
            )));
        }
    }

    #[test]
    fn c0_3a_protocol_applied_only_when_every_device_applied() {
        let mut coordinator = with_devices(&[1, 2, 3]);
        for device in 1..=3 {
            add_output(&mut coordinator, device, device * 10);
        }
        coordinator.set_protocol_dpms_level(1).unwrap();

        finish_transition(
            &mut coordinator,
            1,
            LifecycleCommitOutcome::Completed {
                dpms_projections_retired: true,
            },
        );
        finish_transition(
            &mut coordinator,
            2,
            LifecycleCommitOutcome::Rejected {
                topology_latched_generation: Some(7),
            },
        );
        let removal = coordinator.remove_protocol_output(&3, &30).unwrap();
        assert_eq!(
            removal.unwrap().projection.representative,
            Some(
                coordinator
                    .device(&3)
                    .unwrap()
                    .desired()
                    .representative(DesiredField::Dpms)
                    .unwrap()
                    .event_id
            )
        );
        assert!(!coordinator.protocol_dpms_request_applied());

        coordinator
            .project_device_intent(
                &2,
                DesiredIntent::Topology {
                    discovery_epoch: 8,
                    change_class: TopologyChangeClass::SameIdentity,
                },
            )
            .unwrap();
        finish_transition(
            &mut coordinator,
            2,
            LifecycleCommitOutcome::Completed {
                dpms_projections_retired: true,
            },
        );
        finish_transition(
            &mut coordinator,
            2,
            LifecycleCommitOutcome::Completed {
                dpms_projections_retired: true,
            },
        );
        assert!(coordinator.protocol_dpms_request_applied());

        let mut terminal = with_devices(&[4]);
        add_output(&mut terminal, 4, 40);
        terminal.set_protocol_dpms_level(1).unwrap();
        terminal.request_shutdown().unwrap();
        assert_eq!(
            terminal
                .dpms_request
                .as_ref()
                .unwrap()
                .terminal_outcomes
                .get(&4),
            Some(&Disposition::Invalidated(
                crate::kms::owner::lifecycle::InvalidationReason::Shutdown
            ))
        );
        assert!(!terminal.protocol_dpms_request_applied());
    }

    #[test]
    fn c0_3a_shutdown_is_monotonic() {
        let mut coordinator = with_devices(&[1, 2]);
        coordinator.request_shutdown().unwrap();
        assert!(coordinator.shutdown_requested());
        coordinator.set_seat_target(SeatTarget::Owned).unwrap();
        assert!(coordinator.shutdown_requested());
        assert_eq!(coordinator.seat_target(), Some(SeatTarget::Owned));
        for device in [1, 2] {
            assert!(
                coordinator
                    .device(&device)
                    .unwrap()
                    .desired()
                    .shutdown_requested()
            );
        }
    }

    #[test]
    fn c0_3a_loss_report_gets_a_coordinator_event_id() {
        let mut coordinator = with_devices(&[1, 2]);
        let dispatch = coordinator.report_completion_loss(&1).unwrap();
        assert_eq!(dispatch.event_id, LifecycleEventId::from_raw(1));
        let incident = coordinator.device(&1).unwrap().recovery().unwrap();
        assert_eq!(incident.representative(), Some(dispatch.event_id));
        let representative = coordinator
            .device(&1)
            .unwrap()
            .desired()
            .representative(DesiredField::Recovery)
            .unwrap();
        assert_eq!(representative.event_id, dispatch.event_id);
        assert_eq!(representative.disposition, None);
        assert!(coordinator.device(&2).unwrap().recovery().is_none());
        assert!(
            coordinator
                .device(&2)
                .unwrap()
                .desired()
                .representative(DesiredField::Recovery)
                .is_none()
        );

        let second = coordinator.report_completion_loss(&1).unwrap();
        assert_eq!(second.event_id, LifecycleEventId::from_raw(2));
        assert_eq!(coordinator.device(&1).unwrap().recovery(), Some(incident));
        assert_eq!(
            coordinator
                .device(&1)
                .unwrap()
                .desired()
                .representative(DesiredField::Recovery)
                .unwrap()
                .event_id,
            dispatch.event_id
        );
        assert!(second.actions.iter().any(|action| matches!(
            action,
            Action::DispositionChanged {
                event_id,
                disposition: Disposition::AbsorbedByEvent(representative),
            } if *event_id == second.event_id && *representative == dispatch.event_id
        )));
    }

    #[test]
    fn c0_3a_loss_reaches_table_u_through_the_arbiter() {
        let cases = [
            (CompletionUnknownRowKind::NormalLive, 0),
            (CompletionUnknownRowKind::VTRelease, 0),
            (CompletionUnknownRowKind::DeviceRemoved, 0),
            (CompletionUnknownRowKind::Shutdown, 0),
            (CompletionUnknownRowKind::DeviceAddedOrReplaced, 0),
            (CompletionUnknownRowKind::VTAcquire, 0),
            (CompletionUnknownRowKind::AdministrativeReprobe, 0),
            (CompletionUnknownRowKind::IdentityChangingHotplug, 0),
            (CompletionUnknownRowKind::TopologyRebuild, 0),
            (CompletionUnknownRowKind::DPMS, 0),
            (CompletionUnknownRowKind::DPMS, 1),
        ];
        for (kind, level) in cases {
            for resolution in [
                RecoveryAttemptOutcome::Qualified,
                RecoveryAttemptOutcome::FailedOrUnknown,
            ] {
                let creates_incident = matches!(
                    kind,
                    CompletionUnknownRowKind::NormalLive
                        | CompletionUnknownRowKind::TopologyRebuild
                        | CompletionUnknownRowKind::DPMS
                );
                if !creates_incident && resolution == RecoveryAttemptOutcome::FailedOrUnknown {
                    // The same no-incident rows need not be enumerated twice.
                    continue;
                }

                let mut coordinator = row_setup(kind, level);
                let dispatch = coordinator.report_completion_loss(&1).unwrap();
                let row = row_with_id(kind, level);
                let expected = table_u(row, dispatch.event_id, None);
                assert!(
                    dispatch.actions.iter().any(|action| matches!(
                        action,
                        Action::CompletionLossTableU { row: actual_row, outcome }
                            if *actual_row == row && *outcome == expected
                    )),
                    "Table U handoff differed for {kind:?}, level {level}"
                );
                assert!(dispatch.actions.iter().any(|action| matches!(
                    action,
                    Action::CompletionBarriersRequired(outcome) if *outcome == expected
                )));
                assert_eq!(
                    dispatch
                        .actions
                        .iter()
                        .any(|action| matches!(action, Action::ReleaseSeat(_))),
                    expected.logical.release_seat,
                    "seat action differed for {kind:?}"
                );
                assert_eq!(
                    dispatch
                        .actions
                        .iter()
                        .any(|action| matches!(action, Action::WithdrawOutputs(_))),
                    expected.logical.withdraw_protocol_work,
                    "withdraw action differed for {kind:?}"
                );
                assert_eq!(
                    dispatch
                        .actions
                        .iter()
                        .any(|action| matches!(action, Action::TerminalizeProtocolWork(_))),
                    expected.logical.withdraw_protocol_work,
                    "protocol terminalization differed for {kind:?}"
                );
                assert_eq!(
                    coordinator.device(&1).unwrap().admission_open(),
                    !expected.logical.stop_admission,
                    "admission outcome differed for {kind:?}"
                );

                if let Some(seed) = expected.recovery.new_incident {
                    let incident = coordinator.device(&1).unwrap().recovery().unwrap();
                    assert_eq!(incident.representative(), Some(dispatch.event_id));
                    assert_eq!(incident.state(), seed.initial_state);
                    assert_eq!(
                        incident.budget(),
                        crate::kms::owner::lifecycle::RecoveryAttemptBudget::Available
                    );
                    let representative = coordinator
                        .device(&1)
                        .unwrap()
                        .desired()
                        .representative(DesiredField::Recovery)
                        .unwrap();
                    assert_eq!(representative.event_id, dispatch.event_id);
                    assert_eq!(representative.disposition, None);

                    if incident.state() == RecoveryIncidentState::Paused {
                        coordinator.set_protocol_dpms_level(0).unwrap();
                    }
                    let (_, attempt_tag) = complete_recovery_attempt(&mut coordinator, resolution);
                    let disposition = coordinator
                        .device(&1)
                        .unwrap()
                        .desired()
                        .representative(DesiredField::Recovery)
                        .unwrap()
                        .disposition;
                    match resolution {
                        RecoveryAttemptOutcome::Qualified => {
                            assert_eq!(
                                disposition,
                                Some(Disposition::Applied(attempt_tag.transition))
                            );
                            assert_eq!(
                                coordinator.device(&1).unwrap().state(),
                                DeviceLifecycleState::Ready
                            );
                        }
                        RecoveryAttemptOutcome::FailedOrUnknown => {
                            assert_eq!(
                                disposition,
                                Some(Disposition::Invalidated(
                                    crate::kms::owner::lifecycle::InvalidationReason::RecoveryFailed
                                ))
                            );
                            assert_eq!(
                                coordinator.device(&1).unwrap().state(),
                                DeviceLifecycleState::RecoveryFailed
                            );
                        }
                        RecoveryAttemptOutcome::Started(_) => unreachable!(),
                    }
                } else {
                    assert_eq!(
                        coordinator.device(&1).unwrap().recovery(),
                        expected.recovery.current_incident,
                        "incident fate differed for {kind:?}"
                    );
                    let terminal = match expected.logical.event {
                        crate::kms::owner::lifecycle::EventFate::Terminal {
                            event_id,
                            disposition,
                        } => Some((event_id, disposition)),
                        crate::kms::owner::lifecycle::EventFate::PendingRepresentative(_) => None,
                    };
                    if let Some((event_id, disposition)) = terminal {
                        assert!(
                            dispatch.actions.iter().any(|action| matches!(
                                action,
                                Action::DispositionChanged {
                                    event_id: actual_id,
                                    disposition: actual_disposition,
                                } if *actual_id == event_id && *actual_disposition == disposition
                            )),
                            "event disposition differed for {kind:?}"
                        );
                    } else if let crate::kms::owner::lifecycle::EventFate::PendingRepresentative(
                        representative,
                    ) = expected.logical.event
                    {
                        assert_eq!(
                            coordinator
                                .device(&1)
                                .unwrap()
                                .recovery()
                                .and_then(|incident| incident.representative()),
                            Some(representative),
                            "pending representative differed for {kind:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn c0_3a_recovery_outcome_settles_the_incident() {
        let mut successful = with_devices(&[1]);
        successful.report_completion_loss(&1).unwrap();
        let success_tag = acknowledge_receipts(&mut successful, 1);
        let recovery_id = successful.device(&1).unwrap().recovery().unwrap().id();
        assert!(
            successful
                .recovery_attempt(
                    &1,
                    success_tag,
                    RecoveryAttemptOutcome::Started(RecoveryAttemptTrigger::Timer),
                )
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            successful.device(&1).unwrap().state(),
            DeviceLifecycleState::Poisoned,
            "a non-AfterReap trigger is unauthorized"
        );
        successful
            .recovery_attempt(
                &1,
                success_tag,
                RecoveryAttemptOutcome::Started(RecoveryAttemptTrigger::AfterReap),
            )
            .unwrap();
        assert_eq!(
            successful.device(&1).unwrap().state(),
            DeviceLifecycleState::Recovering(recovery_id)
        );
        successful
            .recovery_attempt(&1, success_tag, RecoveryAttemptOutcome::Qualified)
            .unwrap();
        assert_eq!(
            successful.device(&1).unwrap().state(),
            DeviceLifecycleState::Ready
        );
        assert_eq!(
            successful
                .device(&1)
                .unwrap()
                .desired()
                .representative(DesiredField::Recovery)
                .unwrap()
                .disposition,
            Some(Disposition::Applied(success_tag.transition))
        );
        assert!(successful.device(&1).unwrap().recovery().is_none());

        let mut failed = with_devices(&[1]);
        let failed_report = failed.report_completion_loss(&1).unwrap();
        let failed_id = failed.device(&1).unwrap().recovery().unwrap().id();
        let failed_tag = acknowledge_receipts(&mut failed, 1);
        failed
            .recovery_attempt(
                &1,
                failed_tag,
                RecoveryAttemptOutcome::Started(RecoveryAttemptTrigger::AfterReap),
            )
            .unwrap();
        failed
            .recovery_attempt(&1, failed_tag, RecoveryAttemptOutcome::FailedOrUnknown)
            .unwrap();
        assert_eq!(
            failed.device(&1).unwrap().state(),
            DeviceLifecycleState::RecoveryFailed
        );
        assert_eq!(
            failed
                .device(&1)
                .unwrap()
                .desired()
                .representative(DesiredField::Recovery)
                .unwrap()
                .disposition,
            Some(Disposition::Invalidated(
                crate::kms::owner::lifecycle::InvalidationReason::RecoveryFailed
            ))
        );
        assert_eq!(
            failed.device(&1).unwrap().recovery().unwrap().id(),
            failed_id
        );
        assert_eq!(
            failed_report.event_id,
            failed
                .device(&1)
                .unwrap()
                .recovery()
                .unwrap()
                .representative()
                .unwrap()
        );

        let mut unauthorized = with_devices(&[1]);
        unauthorized
            .project_device_intent(&1, DesiredIntent::AdministrativeReprobe { epoch: 1 })
            .unwrap();
        let unauthorized_tag = acknowledge_receipts(&mut unauthorized, 1);
        unauthorized
            .recovery_attempt(
                &1,
                unauthorized_tag,
                RecoveryAttemptOutcome::Started(RecoveryAttemptTrigger::AfterReap),
            )
            .unwrap();
        assert_eq!(
            unauthorized.device(&1).unwrap().state(),
            DeviceLifecycleState::Ready,
            "a table with no recovery incident did not authorize an attempt"
        );

        let mut stale = with_devices(&[1]);
        stale.report_completion_loss(&1).unwrap();
        let stale_tag = acknowledge_receipts(&mut stale, 1);
        stale
            .recovery_attempt(
                &1,
                stale_tag,
                RecoveryAttemptOutcome::Started(RecoveryAttemptTrigger::AfterReap),
            )
            .unwrap();
        stale
            .project_device_intent(
                &1,
                DesiredIntent::Topology {
                    discovery_epoch: 1,
                    change_class: TopologyChangeClass::SameIdentity,
                },
            )
            .unwrap();
        let current_tag = acknowledge_receipts(&mut stale, 1);
        assert_ne!(stale_tag.transition, current_tag.transition);
        let state_before = stale.device(&1).unwrap().state();
        let transition_before = stale.device(&1).unwrap().transition();
        let recovery_before = stale.device(&1).unwrap().recovery();
        let disposition_before = stale
            .device(&1)
            .unwrap()
            .desired()
            .representative(DesiredField::Recovery)
            .and_then(|representative| representative.disposition);
        stale
            .recovery_attempt(&1, stale_tag, RecoveryAttemptOutcome::Qualified)
            .unwrap();
        assert_eq!(stale.device(&1).unwrap().state(), state_before);
        assert_eq!(stale.device(&1).unwrap().transition(), transition_before);
        assert_eq!(stale.device(&1).unwrap().recovery(), recovery_before);
        assert_eq!(
            stale
                .device(&1)
                .unwrap()
                .desired()
                .representative(DesiredField::Recovery)
                .and_then(|representative| representative.disposition),
            disposition_before
        );

        let mut boundary = with_devices(&[1]);
        boundary.report_completion_loss(&1).unwrap();
        boundary.set_seat_target(SeatTarget::Owned).unwrap();
        boundary
            .project_device_intent(
                &1,
                DesiredIntent::DevicePresence {
                    present: true,
                    identity_epoch: 2,
                },
            )
            .unwrap();
        let boundary_incident = boundary.device(&1).unwrap().recovery().unwrap();
        assert_eq!(boundary_incident.origin(), IncidentOrigin::Boundary);
        assert_eq!(boundary_incident.representative(), None);
        let boundary_tag = acknowledge_receipts(&mut boundary, 1);
        boundary
            .recovery_attempt(
                &1,
                boundary_tag,
                RecoveryAttemptOutcome::Started(RecoveryAttemptTrigger::AfterReap),
            )
            .unwrap();
        boundary
            .recovery_attempt(&1, boundary_tag, RecoveryAttemptOutcome::Qualified)
            .unwrap();
        assert!(boundary.device(&1).unwrap().recovery().is_none());
        assert_eq!(
            boundary.device(&1).unwrap().state(),
            DeviceLifecycleState::Ready
        );
        assert!(
            boundary
                .device(&1)
                .unwrap()
                .desired()
                .representative(DesiredField::Presence)
                .is_some_and(|representative| {
                    representative.disposition
                        == Some(Disposition::Applied(boundary_tag.transition))
                })
        );
    }

    #[test]
    fn c0_3a_devices_converge_independently() {
        let mut coordinator = with_devices(&[1, 2]);
        add_output(&mut coordinator, 1, 10);
        add_output(&mut coordinator, 2, 20);
        coordinator.set_protocol_dpms_level(1).unwrap();
        assert!(coordinator.device(&1).unwrap().transition().is_some());
        assert!(coordinator.device(&2).unwrap().transition().is_some());

        finish_transition(
            &mut coordinator,
            2,
            LifecycleCommitOutcome::Completed {
                dpms_projections_retired: true,
            },
        );
        assert_eq!(
            coordinator.device(&2).unwrap().state(),
            DeviceLifecycleState::Ready
        );
        assert!(coordinator.device(&2).unwrap().transition().is_none());
        assert!(coordinator.device(&1).unwrap().transition().is_some());
        assert!(!coordinator.protocol_dpms_request_applied());

        finish_transition(
            &mut coordinator,
            1,
            LifecycleCommitOutcome::Completed {
                dpms_projections_retired: true,
            },
        );
        assert!(coordinator.protocol_dpms_request_applied());
    }

    #[test]
    fn c0_3a_coordinator_rejects_direct_global_and_driver_event_inputs() {
        let mut coordinator = with_devices(&[1]);
        assert_eq!(
            coordinator.project_device_intent(&1, DesiredIntent::Shutdown),
            Err(CoordinatorError::GlobalIntentRequiresCoordinator)
        );
        assert_eq!(
            coordinator.apply_device_input(
                &1,
                ArbiterInput::CompletionUnknown {
                    event_id: LifecycleEventId::from_raw(99),
                    boundary_recovery_id: None,
                },
            ),
            Err(CoordinatorError::CoordinatorOwnedInput)
        );
        assert_eq!(
            coordinator.apply_device_input(
                &1,
                ArbiterInput::SeatTargetObserved {
                    target: SeatTarget::Released,
                    epoch: 1,
                },
            ),
            Err(CoordinatorError::CoordinatorOwnedInput)
        );
    }
}
