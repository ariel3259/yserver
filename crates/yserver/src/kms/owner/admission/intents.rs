use std::collections::{BTreeMap, BTreeSet};

use super::{AdmissionError, CrtcId};

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

#[derive(Debug, Default)]
pub struct Admission {
    composed: BTreeMap<CrtcId, ComposedIntent>,
    direct: Option<QueuedDirect>,
    unflip: Option<UnflipBarrier>,
    topology: Option<u64>,
    next_ordinal: u64,
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

    pub fn composed(&self, crtc: CrtcId) -> Option<ComposedIntent> {
        self.composed.get(&crtc).copied()
    }

    pub(super) fn composed_intents(&self) -> impl Iterator<Item = (CrtcId, ComposedIntent)> + '_ {
        self.composed.iter().map(|(&crtc, &intent)| (crtc, intent))
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
}
