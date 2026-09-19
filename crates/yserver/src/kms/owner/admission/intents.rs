use std::collections::{BTreeMap, BTreeSet};

use super::{AdmissionError, CrtcId};
use crate::kms::owner::admission::bound::MaintenanceBound;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MaintenanceClass {
    Cursor,
    Gamma,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaintenanceKey {
    pub crtc: CrtcId,
    pub class: MaintenanceClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AdmissionTicket(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceIntent {
    pub generation: u64,
    pub ticket: AdmissionTicket,
    pub aged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReentryKind {
    Rejected,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reentry {
    Reentered,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrimaryOrdinal(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposedIntent {
    pub generation: u64,
    pub ordinal: PrimaryOrdinal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectSuccessor {
    pub source_generation: u64,
    pub layout_generation: u64,
    pub topology_generation: u64,
    pub crtcs: BTreeSet<CrtcId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedDirect {
    pub successor: DirectSuccessor,
    pub ordinal: PrimaryOrdinal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnflipBarrier {
    pub crtcs: BTreeSet<CrtcId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SubmittedMaintenance {
    pub(super) generation: u64,
    pub(super) ticket: AdmissionTicket,
}

#[derive(Debug, Default)]
pub struct Admission {
    pub(super) composed: BTreeMap<CrtcId, ComposedIntent>,
    pub(super) direct: Option<QueuedDirect>,
    pub(super) unflip: Option<UnflipBarrier>,
    pub(super) topology: Option<u64>,
    pub(super) maintenance_slots: BTreeMap<MaintenanceKey, MaintenanceIntent>,
    pub(super) maintenance_current: BTreeMap<MaintenanceKey, u64>,
    pub(super) maintenance_submitted: BTreeMap<MaintenanceKey, SubmittedMaintenance>,
    pub(super) rejection_counts: BTreeMap<MaintenanceKey, u32>,
    pub(super) maintenance_bounds: BTreeMap<MaintenanceKey, MaintenanceBound>,
    pub(super) cursor_recovery: BTreeSet<CrtcId>,
    next_ordinal: u64,
    next_ticket: u64,
    pub(super) locked: Option<u64>,
    pub(super) sequence: u64,
    pub(super) last_primary_crtcs: BTreeSet<CrtcId>,
}

impl Admission {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_composed(&mut self, crtc: CrtcId, generation: u64) -> Result<(), AdmissionError> {
        if let Some(current) = self.composed.get(&crtc)
            && generation <= current.generation
        {
            return Err(AdmissionError::StaleGeneration {
                queued: current.generation,
                offered: generation,
            });
        }

        let ordinal = if let Some(current) = self.composed.get(&crtc) {
            current.ordinal
        } else {
            self.allocate_ordinal()
        };
        self.composed.insert(
            crtc,
            ComposedIntent {
                generation,
                ordinal,
            },
        );
        Ok(())
    }

    pub fn set_direct_successor(
        &mut self,
        successor: DirectSuccessor,
    ) -> Result<Option<DirectSuccessor>, AdmissionError> {
        if successor.crtcs.is_empty() {
            return Err(AdmissionError::EmptyCrtcSet);
        }
        if self.unflip.is_some() {
            return Err(AdmissionError::UnflipPending);
        }
        if let Some(current) = &self.direct
            && successor.source_generation <= current.successor.source_generation
        {
            return Err(AdmissionError::StaleGeneration {
                queued: current.successor.source_generation,
                offered: successor.source_generation,
            });
        }

        let ordinal = if let Some(current) = &self.direct {
            current.ordinal
        } else {
            self.allocate_ordinal()
        };
        let displaced = self.direct.replace(QueuedDirect { successor, ordinal });
        Ok(displaced.map(|queued| queued.successor))
    }

    pub fn withdraw_direct(&mut self, source_generation: u64) -> Option<DirectSuccessor> {
        if self
            .direct
            .as_ref()
            .is_some_and(|queued| queued.successor.source_generation == source_generation)
        {
            self.direct.take().map(|queued| queued.successor)
        } else {
            None
        }
    }

    pub fn request_unflip(
        &mut self,
        crtcs: BTreeSet<CrtcId>,
    ) -> Result<Option<DirectSuccessor>, AdmissionError> {
        if crtcs.is_empty() {
            return Err(AdmissionError::EmptyCrtcSet);
        }

        if let Some(barrier) = &mut self.unflip {
            barrier.crtcs.extend(crtcs);
        } else {
            self.unflip = Some(UnflipBarrier { crtcs });
        }

        Ok(self.direct.take().map(|queued| queued.successor))
    }

    pub fn request_topology(&mut self, generation: u64) -> Result<(), AdmissionError> {
        if let Some(queued) = self.topology
            && generation <= queued
        {
            return Err(AdmissionError::StaleGeneration {
                queued,
                offered: generation,
            });
        }
        self.topology = Some(generation);
        Ok(())
    }

    /// Queue the latest desired maintenance generation for an identity.
    ///
    /// A generation already shown by the device is an omission, not a new
    /// intent. A generation offered while the identity is submitted receives a
    /// fresh ticket and is aged so it cannot be mistaken for new work.
    pub fn set_maintenance(
        &mut self,
        key: MaintenanceKey,
        generation: u64,
        behind_commit: bool,
    ) -> Result<(), AdmissionError> {
        if let Some(current) = self.maintenance_slots.get(&key)
            && generation <= current.generation
        {
            return Err(AdmissionError::StaleGeneration {
                queued: current.generation,
                offered: generation,
            });
        }

        if self.maintenance_current.get(&key) == Some(&generation) {
            self.maintenance_slots.remove(&key);
            return Ok(());
        }

        let submitted_ticket = self
            .maintenance_submitted
            .get(&key)
            .map(|submitted| submitted.ticket);
        let (ticket, already_aged) = if let Some(current) = self.maintenance_slots.get(&key) {
            if submitted_ticket == Some(current.ticket) {
                (self.allocate_ticket()?, true)
            } else {
                (current.ticket, current.aged)
            }
        } else {
            (self.allocate_ticket()?, false)
        };

        self.maintenance_slots.insert(
            key,
            MaintenanceIntent {
                generation,
                ticket,
                aged: already_aged || behind_commit || submitted_ticket.is_some(),
            },
        );
        if already_aged || behind_commit || submitted_ticket.is_some() {
            self.age_maintenance(key);
        }
        Ok(())
    }

    /// Re-enter a carried maintenance generation after its terminal outcome.
    pub fn reenter(
        &mut self,
        key: MaintenanceKey,
        generation: u64,
        ticket: AdmissionTicket,
        kind: ReentryKind,
    ) -> Reentry {
        if self
            .maintenance_submitted
            .get(&key)
            .is_some_and(|submitted| {
                submitted.generation == generation && submitted.ticket == ticket
            })
        {
            self.maintenance_submitted.remove(&key);
        }

        let rejection_count = match kind {
            ReentryKind::Rejected => {
                let count = self.rejection_counts.entry(key).or_default();
                if *count < 2 {
                    *count += 1;
                }
                *count
            }
            ReentryKind::Unknown => self.rejection_count(key),
        };

        if kind == ReentryKind::Rejected && rejection_count >= 2 {
            self.maintenance_slots.remove(&key);
            self.maintenance_bounds.remove(&key);
            return Reentry::Dropped;
        }

        match self.maintenance_slots.get_mut(&key) {
            Some(existing) if existing.generation >= generation => {
                existing.ticket = existing.ticket.min(ticket);
                existing.aged = true;
            }
            Some(existing) => {
                *existing = MaintenanceIntent {
                    generation,
                    ticket,
                    aged: true,
                };
            }
            None => {
                self.maintenance_slots.insert(
                    key,
                    MaintenanceIntent {
                        generation,
                        ticket,
                        aged: true,
                    },
                );
            }
        }

        self.age_maintenance(key);

        Reentry::Reentered
    }

    /// Record the generation currently shown by the device.
    pub fn note_completed(&mut self, key: MaintenanceKey, generation: u64) {
        if self
            .maintenance_submitted
            .get(&key)
            .is_some_and(|submitted| submitted.generation == generation)
        {
            self.maintenance_submitted.remove(&key);
        }

        if self
            .maintenance_slots
            .get(&key)
            .is_some_and(|intent| intent.generation <= generation)
        {
            self.maintenance_slots.remove(&key);
            self.maintenance_bounds.remove(&key);
        }

        self.maintenance_current
            .entry(key)
            .and_modify(|current| *current = (*current).max(generation))
            .or_insert(generation);
        self.rejection_counts.remove(&key);
    }

    pub fn request_cursor_recovery(&mut self, crtc: CrtcId) {
        self.cursor_recovery.insert(crtc);
    }

    pub fn maintenance(&self, key: MaintenanceKey) -> Option<MaintenanceIntent> {
        self.maintenance_slots.get(&key).copied()
    }

    pub fn rejection_count(&self, key: MaintenanceKey) -> u32 {
        self.rejection_counts.get(&key).copied().unwrap_or(0)
    }

    pub fn cursor_recovery(&self) -> &BTreeSet<CrtcId> {
        &self.cursor_recovery
    }

    pub fn composed(&self, crtc: CrtcId) -> Option<ComposedIntent> {
        self.composed.get(&crtc).copied()
    }

    pub(super) fn composed_intents(&self) -> impl Iterator<Item = (CrtcId, ComposedIntent)> + '_ {
        self.composed.iter().map(|(&crtc, &intent)| (crtc, intent))
    }

    pub(super) fn maintenance_intents(
        &self,
    ) -> impl Iterator<Item = (MaintenanceKey, MaintenanceIntent)> + '_ {
        self.maintenance_slots
            .iter()
            .map(|(&key, &intent)| (key, intent))
    }

    pub fn direct(&self) -> Option<&QueuedDirect> {
        self.direct.as_ref()
    }

    pub fn unflip(&self) -> Option<&UnflipBarrier> {
        self.unflip.as_ref()
    }

    pub fn topology(&self) -> Option<u64> {
        self.topology
    }

    fn allocate_ordinal(&mut self) -> PrimaryOrdinal {
        let ordinal = PrimaryOrdinal(self.next_ordinal);
        self.next_ordinal = self
            .next_ordinal
            .checked_add(1)
            .expect("primary ordinal overflow");
        ordinal
    }

    fn allocate_ticket(&mut self) -> Result<AdmissionTicket, AdmissionError> {
        let next = self
            .next_ticket
            .checked_add(1)
            .ok_or(AdmissionError::TicketOverflow)?;
        let ticket = AdmissionTicket(self.next_ticket);
        self.next_ticket = next;
        Ok(ticket)
    }
}
