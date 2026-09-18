use std::collections::BTreeMap;

use super::CrtcId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IntentKey {
    Unflip,
    Composed { crtc: CrtcId, generation: u64 },
    Direct { source_generation: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    SourceWaits,
    NoReusableBuffer,
    OrdinaryRetirementOccupied,
    ExitRetirementOccupied,
    ComposedReturnNotEstablished,
    NotDirectEligible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    Waiting(WaitReason),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadinessSnapshot {
    pub layout_generation: u64,
    pub topology_generation: u64,
    pub retirement_wake: bool,
    reports: BTreeMap<IntentKey, Readiness>,
}

impl ReadinessSnapshot {
    pub fn new(layout_generation: u64, topology_generation: u64) -> Self {
        Self {
            layout_generation,
            topology_generation,
            retirement_wake: false,
            reports: BTreeMap::new(),
        }
    }

    pub fn report(&mut self, key: IntentKey, readiness: Readiness) {
        self.reports.insert(key, readiness);
    }

    pub fn readiness(&self, key: IntentKey) -> Option<Readiness> {
        self.reports.get(&key).copied()
    }

    pub fn is_ready(&self, key: IntentKey) -> bool {
        self.readiness(key) == Some(Readiness::Ready)
    }
}
