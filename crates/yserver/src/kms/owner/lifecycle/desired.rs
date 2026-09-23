//! Per-device `REC-5` desired state and its bounded representative ledger.

use std::collections::BTreeMap;

use super::{Disposition, InvalidationReason, LifecycleEventId, LifecycleKind, RecoveryId};

/// The fields with one current event representative in `LifecycleDesired`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum DesiredField {
    Shutdown,
    Presence,
    Seat,
    AdministrativeReprobe,
    Topology,
    Dpms,
    Recovery,
}

impl DesiredField {
    /// All representative fields; independent of the number of events seen.
    pub const ALL: [Self; 7] = [
        Self::Shutdown,
        Self::Presence,
        Self::Seat,
        Self::AdministrativeReprobe,
        Self::Topology,
        Self::Dpms,
        Self::Recovery,
    ];
}

/// Projected seat ownership target.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum SeatTarget {
    Owned,
    Released,
}

/// Whether topology dirtiness changes device identity or only discovered objects.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum TopologyChangeClass {
    SameIdentity,
    IdentityChanging,
}

/// A coordinator-projected lifecycle input. Epochs are supplied by the owner
/// above this pure layer; this type does not allocate or advance them.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum DesiredIntent {
    Shutdown,
    DevicePresence {
        present: bool,
        identity_epoch: u64,
    },
    Seat {
        target: SeatTarget,
        epoch: u64,
    },
    AdministrativeReprobe {
        epoch: u64,
    },
    Topology {
        discovery_epoch: u64,
        change_class: TopologyChangeClass,
    },
    Dpms {
        level: u8,
        epoch: u64,
    },
    NormalRecovery {
        recovery_id: RecoveryId,
    },
}

impl DesiredIntent {
    pub const fn kind(self) -> LifecycleKind {
        match self {
            Self::Shutdown => LifecycleKind::Shutdown,
            Self::DevicePresence { present: true, .. } => LifecycleKind::DeviceAddedOrReplaced,
            Self::DevicePresence { present: false, .. } => LifecycleKind::DeviceRemoved,
            Self::Seat {
                target: SeatTarget::Owned,
                ..
            } => LifecycleKind::VTAcquire,
            Self::Seat {
                target: SeatTarget::Released,
                ..
            } => LifecycleKind::VTRelease,
            Self::AdministrativeReprobe { .. } => LifecycleKind::AdministrativeReprobe,
            Self::Topology {
                change_class: TopologyChangeClass::SameIdentity,
                ..
            } => LifecycleKind::TopologyRebuild,
            Self::Topology {
                change_class: TopologyChangeClass::IdentityChanging,
                ..
            } => LifecycleKind::IdentityChangingHotplug,
            Self::Dpms { .. } => LifecycleKind::DPMS,
            Self::NormalRecovery { .. } => LifecycleKind::NormalRecovery,
        }
    }

    const fn field(self) -> DesiredField {
        match self.kind() {
            LifecycleKind::Shutdown => DesiredField::Shutdown,
            LifecycleKind::DeviceRemoved | LifecycleKind::DeviceAddedOrReplaced => {
                DesiredField::Presence
            }
            LifecycleKind::VTRelease | LifecycleKind::VTAcquire => DesiredField::Seat,
            LifecycleKind::AdministrativeReprobe => DesiredField::AdministrativeReprobe,
            LifecycleKind::IdentityChangingHotplug | LifecycleKind::TopologyRebuild => {
                DesiredField::Topology
            }
            LifecycleKind::DPMS => DesiredField::Dpms,
            LifecycleKind::NormalRecovery => DesiredField::Recovery,
        }
    }
}

/// The current representative and its optional arbiter disposition.
/// `None` means it is projected and awaits arbitration; `Deferred` is the only
/// nonterminal disposition once the arbiter has classified it.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Representative {
    pub field: DesiredField,
    pub event_id: LifecycleEventId,
    pub kind: LifecycleKind,
    pub disposition: Option<Disposition>,
}

/// One event that reached a terminal disposition during this projection.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DispositionChange {
    pub event_id: LifecycleEventId,
    pub disposition: Disposition,
}

/// Immediate result of projecting one event. Terminal history is returned to
/// the caller and is not retained, keeping the snapshot bounded by fields.
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct ProjectionResult {
    pub event_disposition: Option<Disposition>,
    pub terminalized: Vec<DispositionChange>,
}

/// One DPMS target in the current stable protocol-output domain.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct OutputProjection {
    pub level: u8,
    pub epoch: Option<u64>,
    pub representative: Option<LifecycleEventId>,
}

/// One output projection forgotten because its stable protocol output left
/// the current domain. This does not terminalize the shared DPMS request.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct OutputProjectionRemoval {
    pub projection: OutputProjection,
    pub reason: InvalidationReason,
}

/// The per-device desired snapshot from C.0 `REC-5`.
///
/// `O` is the caller's stable protocol-output identity type. Only currently
/// present outputs are retained in the DPMS projection map.
#[derive(Debug, Eq, PartialEq)]
pub struct LifecycleDesired<O> {
    shutdown_requested: bool,
    device_presence: Option<bool>,
    identity_epoch: Option<u64>,
    seat_target: Option<SeatTarget>,
    seat_epoch: Option<u64>,
    administrative_reprobe_epoch: Option<u64>,
    topology_dirty: bool,
    discovery_epoch: Option<u64>,
    change_class: Option<TopologyChangeClass>,
    protocol_dpms_level: u8,
    projected_dpms_epoch: Option<u64>,
    dpms_targets: BTreeMap<O, OutputProjection>,
    recovery_incident: Option<RecoveryId>,
    representatives: Vec<Representative>,
}

impl<O: Ord> Default for LifecycleDesired<O> {
    fn default() -> Self {
        Self {
            shutdown_requested: false,
            device_presence: None,
            identity_epoch: None,
            seat_target: None,
            seat_epoch: None,
            administrative_reprobe_epoch: None,
            topology_dirty: false,
            discovery_epoch: None,
            change_class: None,
            protocol_dpms_level: 0,
            projected_dpms_epoch: None,
            dpms_targets: BTreeMap::new(),
            recovery_incident: None,
            representatives: Vec::new(),
        }
    }
}

impl<O: Ord> LifecycleDesired<O> {
    pub const REPRESENTATIVE_FIELD_COUNT: usize = DesiredField::ALL.len();

    pub const fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    pub const fn device_presence(&self) -> Option<bool> {
        self.device_presence
    }

    pub const fn identity_epoch(&self) -> Option<u64> {
        self.identity_epoch
    }

    pub const fn seat_target(&self) -> Option<SeatTarget> {
        self.seat_target
    }

    pub const fn seat_epoch(&self) -> Option<u64> {
        self.seat_epoch
    }

    /// Update the observed seat prerequisite without creating a VT lifecycle
    /// representative. Stage 3a consumes the existing VT path as a read-only
    /// feed; stages 3b/3c own VT transition execution.
    pub(crate) fn observe_seat_target(&mut self, target: SeatTarget, epoch: u64) {
        self.seat_target = Some(target);
        self.seat_epoch = Some(epoch);
    }

    pub const fn administrative_reprobe_epoch(&self) -> Option<u64> {
        self.administrative_reprobe_epoch
    }

    pub const fn topology_dirty(&self) -> bool {
        self.topology_dirty
    }

    pub const fn discovery_epoch(&self) -> Option<u64> {
        self.discovery_epoch
    }

    pub const fn change_class(&self) -> Option<TopologyChangeClass> {
        self.change_class
    }

    pub const fn protocol_dpms_level(&self) -> u8 {
        self.protocol_dpms_level
    }

    pub const fn projected_dpms_epoch(&self) -> Option<u64> {
        self.projected_dpms_epoch
    }

    pub fn dpms_targets(&self) -> &BTreeMap<O, OutputProjection> {
        &self.dpms_targets
    }

    pub const fn recovery_incident(&self) -> Option<RecoveryId> {
        self.recovery_incident
    }

    pub fn representatives(&self) -> &[Representative] {
        &self.representatives
    }

    pub fn retained_representative_count(&self) -> usize {
        self.representatives.len()
    }

    pub fn representative(&self, field: DesiredField) -> Option<Representative> {
        self.representatives
            .iter()
            .copied()
            .find(|representative| representative.field == field)
    }

    pub fn disposition(&self, event_id: LifecycleEventId) -> Option<Disposition> {
        self.representatives
            .iter()
            .find(|representative| representative.event_id == event_id)
            .and_then(|representative| representative.disposition)
    }

    /// Assign or advance a representative's disposition. A terminal value is
    /// immutable; terminal history is returned to the projector and forgotten.
    pub fn set_disposition(
        &mut self,
        event_id: LifecycleEventId,
        disposition: Disposition,
    ) -> bool {
        let Some(representative) = self
            .representatives
            .iter_mut()
            .find(|representative| representative.event_id == event_id)
        else {
            return false;
        };

        if representative
            .disposition
            .is_some_and(Disposition::is_terminal)
        {
            return false;
        }

        representative.disposition = Some(disposition);
        true
    }

    /// Resolve a prerequisite-deferred representative to a terminal outcome.
    pub fn resolve_deferred(
        &mut self,
        event_id: LifecycleEventId,
        disposition: Disposition,
    ) -> bool {
        if !disposition.is_terminal() {
            return false;
        }
        let Some(representative) = self
            .representatives
            .iter_mut()
            .find(|representative| representative.event_id == event_id)
        else {
            return false;
        };
        if !matches!(representative.disposition, Some(Disposition::Deferred(_))) {
            return false;
        }
        representative.disposition = Some(disposition);
        true
    }

    /// Add a stable output to the current protocol domain. It inherits the
    /// latest global DPMS level and epoch before topology installation.
    pub fn add_protocol_output(&mut self, output: O) -> Option<OutputProjection> {
        if self.dpms_targets.contains_key(&output) {
            return None;
        }

        let projection = OutputProjection {
            level: self.protocol_dpms_level,
            epoch: self.projected_dpms_epoch,
            representative: self
                .representative(DesiredField::Dpms)
                .map(|entry| entry.event_id),
        };
        self.dpms_targets.insert(output, projection);
        Some(projection)
    }

    /// Remove exactly one output projection. A repeated removal has no second
    /// invalidation because the target is forgotten with the output.
    pub fn remove_protocol_output(&mut self, output: &O) -> Option<OutputProjectionRemoval> {
        let projection = self.dpms_targets.remove(output)?;
        Some(OutputProjectionRemoval {
            projection,
            reason: InvalidationReason::ProtocolOutputRemoved,
        })
    }

    /// Project one event into the snapshot, coalescing or superseding its
    /// field's prior representative according to `REC-5`.
    pub fn project(
        &mut self,
        event_id: LifecycleEventId,
        intent: DesiredIntent,
    ) -> ProjectionResult {
        let kind = intent.kind();
        let field = intent.field();

        if let Some(existing) = self.representative(field)
            && ((kind == LifecycleKind::Shutdown && existing.kind == kind)
                || (kind == LifecycleKind::DeviceRemoved && existing.kind == kind)
                || (kind == LifecycleKind::NormalRecovery && self.recovery_incident.is_some()))
        {
            return ProjectionResult {
                event_disposition: Some(Disposition::AbsorbedByEvent(existing.event_id)),
                terminalized: Vec::new(),
            };
        }

        if self.shutdown_requested && kind != LifecycleKind::Shutdown {
            return ProjectionResult {
                event_disposition: Some(Disposition::Invalidated(InvalidationReason::Shutdown)),
                terminalized: Vec::new(),
            };
        }

        let mut result = ProjectionResult::default();
        if kind == LifecycleKind::Shutdown {
            self.shutdown_requested = true;
            self.invalidate_pending_representatives(InvalidationReason::Shutdown, &mut result);
        }

        match intent {
            DesiredIntent::Shutdown => {}
            DesiredIntent::DevicePresence {
                present,
                identity_epoch,
            } => {
                self.device_presence = Some(present);
                self.identity_epoch = Some(identity_epoch);
            }
            DesiredIntent::Seat { target, epoch } => {
                self.seat_target = Some(target);
                self.seat_epoch = Some(epoch);
            }
            DesiredIntent::AdministrativeReprobe { epoch } => {
                self.administrative_reprobe_epoch = Some(epoch);
            }
            DesiredIntent::Topology {
                discovery_epoch,
                change_class,
            } => {
                self.topology_dirty = true;
                self.discovery_epoch = Some(discovery_epoch);
                self.change_class = Some(change_class);
            }
            DesiredIntent::Dpms { level, epoch } => {
                self.protocol_dpms_level = level;
                self.projected_dpms_epoch = Some(epoch);
                for projection in self.dpms_targets.values_mut() {
                    *projection = OutputProjection {
                        level,
                        epoch: Some(epoch),
                        representative: Some(event_id),
                    };
                }
            }
            DesiredIntent::NormalRecovery { recovery_id } => {
                self.recovery_incident = Some(recovery_id);
            }
        }

        if let Some(index) = self
            .representatives
            .iter()
            .position(|representative| representative.field == field)
        {
            let displaced = self.representatives[index];
            self.representatives[index] = Representative {
                field,
                event_id,
                kind,
                disposition: None,
            };
            if !displaced.disposition.is_some_and(Disposition::is_terminal) {
                result.terminalized.push(DispositionChange {
                    event_id: displaced.event_id,
                    disposition: Disposition::SupersededBy(event_id),
                });
            }
        } else {
            self.representatives.push(Representative {
                field,
                event_id,
                kind,
                disposition: None,
            });
        }

        result
    }

    /// Clear dirty topology only when the caller completed the current
    /// discovery generation.
    pub fn clear_topology_dirty(&mut self, discovery_epoch: u64) -> bool {
        if self.discovery_epoch != Some(discovery_epoch) {
            return false;
        }
        self.topology_dirty = false;
        true
    }

    /// Remove the incident identity after its representative reaches a
    /// terminal outcome.
    pub fn clear_recovery_incident(&mut self, recovery_id: RecoveryId) -> bool {
        if self.recovery_incident != Some(recovery_id) {
            return false;
        }
        if !self
            .representative(DesiredField::Recovery)
            .and_then(|representative| representative.disposition)
            .is_some_and(Disposition::is_terminal)
        {
            return false;
        }
        self.recovery_incident = None;
        true
    }

    fn invalidate_pending_representatives(
        &mut self,
        reason: InvalidationReason,
        result: &mut ProjectionResult,
    ) {
        for representative in self.representatives.drain(..) {
            if !representative
                .disposition
                .is_some_and(Disposition::is_terminal)
            {
                result.terminalized.push(DispositionChange {
                    event_id: representative.event_id,
                    disposition: Disposition::Invalidated(reason),
                });
            }
        }
        self.recovery_incident = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{DesiredField, DesiredIntent, LifecycleDesired, SeatTarget, TopologyChangeClass};
    use crate::kms::owner::lifecycle::{
        Disposition, InvalidationReason, LifecycleEventId, LifecycleKind, LifecycleTransitionId,
        Prerequisite, RecoveryId,
    };

    fn event(raw: u64) -> LifecycleEventId {
        LifecycleEventId::from_raw(raw)
    }

    fn intent_for(kind: LifecycleKind, generation: u64) -> DesiredIntent {
        match kind {
            LifecycleKind::Shutdown => DesiredIntent::Shutdown,
            LifecycleKind::DeviceRemoved => DesiredIntent::DevicePresence {
                present: false,
                identity_epoch: generation,
            },
            LifecycleKind::VTRelease => DesiredIntent::Seat {
                target: SeatTarget::Released,
                epoch: generation,
            },
            LifecycleKind::DeviceAddedOrReplaced => DesiredIntent::DevicePresence {
                present: true,
                identity_epoch: generation,
            },
            LifecycleKind::VTAcquire => DesiredIntent::Seat {
                target: SeatTarget::Owned,
                epoch: generation,
            },
            LifecycleKind::AdministrativeReprobe => {
                DesiredIntent::AdministrativeReprobe { epoch: generation }
            }
            LifecycleKind::IdentityChangingHotplug => DesiredIntent::Topology {
                discovery_epoch: generation,
                change_class: TopologyChangeClass::IdentityChanging,
            },
            LifecycleKind::TopologyRebuild => DesiredIntent::Topology {
                discovery_epoch: generation,
                change_class: TopologyChangeClass::SameIdentity,
            },
            LifecycleKind::DPMS => DesiredIntent::Dpms {
                level: (generation % 4) as u8,
                epoch: generation,
            },
            LifecycleKind::NormalRecovery => DesiredIntent::NormalRecovery {
                recovery_id: RecoveryId::from_raw(generation),
            },
        }
    }

    fn field_for(kind: LifecycleKind) -> DesiredField {
        intent_for(kind, 1).field()
    }

    #[test]
    fn c0_3a_equal_kind_coalescing_table() {
        for kind in LifecycleKind::ALL {
            let mut desired = LifecycleDesired::<u32>::default();
            if kind == LifecycleKind::DPMS {
                desired.add_protocol_output(7);
            }

            let first = event(1);
            let second = event(2);
            let first_result = desired.project(first, intent_for(kind, 1));
            assert_eq!(first_result.event_disposition, None, "{kind:?}");

            let second_result = desired.project(second, intent_for(kind, 2));
            let representative = desired.representative(field_for(kind)).unwrap();

            if matches!(
                kind,
                LifecycleKind::Shutdown
                    | LifecycleKind::DeviceRemoved
                    | LifecycleKind::NormalRecovery
            ) {
                assert_eq!(
                    second_result.event_disposition,
                    Some(Disposition::AbsorbedByEvent(first)),
                    "{kind:?}"
                );
                assert!(second_result.terminalized.is_empty(), "{kind:?}");
                assert_eq!(representative.event_id, first, "{kind:?}");
            } else {
                assert_eq!(second_result.event_disposition, None, "{kind:?}");
                assert_eq!(
                    second_result.terminalized,
                    [super::DispositionChange {
                        event_id: first,
                        disposition: Disposition::SupersededBy(second),
                    }],
                    "{kind:?}"
                );
                assert_eq!(representative.event_id, second, "{kind:?}");
            }

            if matches!(
                kind,
                LifecycleKind::TopologyRebuild | LifecycleKind::IdentityChangingHotplug
            ) {
                assert_eq!(desired.discovery_epoch(), Some(2), "{kind:?}");
            }
        }
    }

    #[test]
    fn c0_3a_storm_stays_bounded() {
        let mut desired = LifecycleDesired::<u32>::default();
        for output in 0..4 {
            desired.add_protocol_output(output);
        }

        for index in 0..10_000_u64 {
            let kind = if index == 9_999 {
                LifecycleKind::Shutdown
            } else {
                LifecycleKind::ALL[(index as usize % (LifecycleKind::ALL.len() - 1)) + 1]
            };
            let id = event(index + 1);
            let result = desired.project(id, intent_for(kind, index + 1));
            assert!(
                result
                    .terminalized
                    .iter()
                    .all(|change| change.disposition.is_terminal()),
                "event {id:?} left a displaced representative nonterminal"
            );
            if let Some(disposition) = result.event_disposition {
                assert!(disposition.is_terminal(), "event {id:?}: {disposition:?}");
            } else {
                assert!(
                    desired
                        .representatives()
                        .iter()
                        .any(|representative| representative.event_id == id),
                    "unclassified event {id:?} was neither retained nor terminalized"
                );
            }
            assert!(
                desired.retained_representative_count()
                    <= LifecycleDesired::<u32>::REPRESENTATIVE_FIELD_COUNT,
                "representative ledger grew with event count"
            );
            assert!(desired.dpms_targets().len() <= 4);
        }
        assert_eq!(desired.retained_representative_count(), 1);
        assert_eq!(
            desired.representative(DesiredField::Shutdown).unwrap().kind,
            LifecycleKind::Shutdown
        );
    }

    #[test]
    fn c0_3a_terminal_disposition_is_immutable() {
        for terminal in Disposition::ALL
            .into_iter()
            .filter(|value| value.is_terminal())
        {
            let mut desired = LifecycleDesired::<u32>::default();
            let id = event(1);
            desired.project(
                id,
                DesiredIntent::Seat {
                    target: SeatTarget::Owned,
                    epoch: 1,
                },
            );
            assert!(desired.set_disposition(id, terminal));
            assert!(
                !desired.set_disposition(id, Disposition::Deferred(Prerequisite::SeatReleased))
            );
            assert_eq!(desired.disposition(id), Some(terminal));
        }
    }

    fn deferred_presence() -> (LifecycleDesired<u32>, LifecycleEventId) {
        let mut desired = LifecycleDesired::default();
        desired.project(
            event(1),
            DesiredIntent::Seat {
                target: SeatTarget::Released,
                epoch: 1,
            },
        );
        let presence = event(2);
        desired.project(
            presence,
            DesiredIntent::DevicePresence {
                present: true,
                identity_epoch: 1,
            },
        );
        assert!(
            desired.set_disposition(presence, Disposition::Deferred(Prerequisite::SeatReleased))
        );
        (desired, presence)
    }

    #[test]
    fn c0_3a_deferred_reaches_one_terminal() {
        let (mut acquired, acquired_presence) = deferred_presence();
        acquired.project(
            event(3),
            DesiredIntent::Seat {
                target: SeatTarget::Owned,
                epoch: 2,
            },
        );
        let applied = Disposition::Applied(LifecycleTransitionId::from_raw(1));
        assert!(acquired.resolve_deferred(acquired_presence, applied));
        assert_eq!(acquired.disposition(acquired_presence), Some(applied));

        let (mut shutdown, shutdown_presence) = deferred_presence();
        let shutdown_result = shutdown.project(event(3), DesiredIntent::Shutdown);
        let invalidated = Disposition::Invalidated(InvalidationReason::Shutdown);
        assert!(
            shutdown_result
                .terminalized
                .contains(&super::DispositionChange {
                    event_id: shutdown_presence,
                    disposition: invalidated,
                })
        );
        assert_eq!(shutdown.disposition(shutdown_presence), None);

        let (mut replaced, replaced_presence) = deferred_presence();
        let newer = event(3);
        let replacement_result = replaced.project(
            newer,
            DesiredIntent::DevicePresence {
                present: true,
                identity_epoch: 2,
            },
        );
        assert_eq!(
            replacement_result.terminalized,
            [super::DispositionChange {
                event_id: replaced_presence,
                disposition: Disposition::SupersededBy(newer),
            }]
        );
    }

    #[test]
    fn c0_3a_projection_follows_the_output_domain() {
        let mut desired = LifecycleDesired::default();
        let initial = desired.add_protocol_output(10).unwrap();
        desired.add_protocol_output(20).unwrap();
        assert_eq!(initial.level, 0);

        let event_id = event(1);
        desired.project(event_id, DesiredIntent::Dpms { level: 3, epoch: 7 });
        assert_eq!(desired.protocol_dpms_level(), 3);
        assert_eq!(desired.projected_dpms_epoch(), Some(7));
        for projection in desired.dpms_targets().values() {
            assert_eq!(
                *projection,
                super::OutputProjection {
                    level: 3,
                    epoch: Some(7),
                    representative: Some(event_id),
                }
            );
        }

        let inherited = desired.add_protocol_output(30).unwrap();
        assert_eq!(inherited.level, 3);
        assert_eq!(inherited.epoch, Some(7));
        assert_eq!(inherited.representative, Some(event_id));

        let removed = desired.remove_protocol_output(&20).unwrap();
        assert_eq!(removed.projection.representative, Some(event_id));
        assert_eq!(removed.reason, InvalidationReason::ProtocolOutputRemoved);
        assert_eq!(desired.remove_protocol_output(&20), None);
        assert_eq!(desired.disposition(event_id), None);
        assert!(desired.dpms_targets().contains_key(&10));
        assert!(desired.dpms_targets().contains_key(&30));
        assert!(!desired.dpms_targets().contains_key(&20));
    }
}
