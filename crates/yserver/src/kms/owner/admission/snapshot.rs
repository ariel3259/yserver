use std::collections::{BTreeMap, BTreeSet};

use super::{CrtcId, MaintenanceKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IntentKey {
    Unflip,
    Composed {
        crtc: CrtcId,
        generation: u64,
    },
    Direct {
        source_generation: u64,
    },
    Maintenance {
        key: MaintenanceKey,
        generation: u64,
    },
    CursorRecovery {
        crtc: CrtcId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    SourceWaits,
    NoReusableBuffer,
    OrdinaryRetirementOccupied,
    ExitRetirementOccupied,
    ComposedReturnNotEstablished,
    UnflipShadowNotMaterialized,
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
    pub homogeneous_group: BTreeSet<CrtcId>,
    reports: BTreeMap<IntentKey, Readiness>,
    compatible: BTreeSet<(IntentKey, IntentKey)>,
}

impl ReadinessSnapshot {
    pub fn new(layout_generation: u64, topology_generation: u64) -> Self {
        Self {
            layout_generation,
            topology_generation,
            retirement_wake: false,
            homogeneous_group: BTreeSet::new(),
            reports: BTreeMap::new(),
            compatible: BTreeSet::new(),
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

    pub fn report_compatible(&mut self, maintenance: IntentKey, primary: IntentKey) {
        self.compatible.insert((maintenance, primary));
    }

    pub fn is_compatible(&self, maintenance: IntentKey, primary: IntentKey) -> bool {
        self.compatible.contains(&(maintenance, primary))
    }
}
