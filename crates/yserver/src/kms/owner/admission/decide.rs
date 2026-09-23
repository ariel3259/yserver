use std::collections::BTreeSet;

use super::{
    Admission, CrtcId, DirectSuccessor, IntentKey, MaintenanceIntent, MaintenanceKey,
    PrimaryOrdinal, ReadinessSnapshot,
};

impl Admission {
    pub fn decide(&self, snapshot: &ReadinessSnapshot) -> Option<AdmissionDecision> {
        if let Some(tag) = self.topology() {
            return Some(self.decision(
                Tier::Topology,
                Admitted::Topology { tag },
                Vec::new(),
                snapshot,
            ));
        }

        if let Some(unflip) = self.unflip()
            && snapshot.is_ready(IntentKey::Unflip)
        {
            return Some(self.decision(
                Tier::Unflip,
                Admitted::Unflip {
                    crtcs: unflip.crtcs.clone(),
                },
                Vec::new(),
                snapshot,
            ));
        }

        if let Some(&crtc) = self
            .cursor_recovery()
            .iter()
            .find(|&&crtc| snapshot.is_ready(IntentKey::CursorRecovery { crtc }))
        {
            return Some(self.decision(
                Tier::Unflip,
                Admitted::CursorRecovery { crtc },
                Vec::new(),
                snapshot,
            ));
        }

        let candidates = self.primary_candidates(snapshot);
        let owed = owed_crtcs(&candidates, &self.last_primary_crtcs);

        if snapshot.retirement_wake
            && let Some(direct) = candidates.iter().find(|candidate| {
                matches!(candidate.admitted, Admitted::Direct { .. })
                    && round_robin_allows(candidate, &self.last_primary_crtcs, &owed)
                    && owed.is_subset(&candidate.crtcs)
                    && self.direct_absorbs_all_ready_aged(candidate, snapshot)
            })
        {
            return Some(self.primary_decision(Tier::DirectSuccessor, direct, snapshot));
        }

        if let Some(maintenance) = self
            .maintenance_intents()
            .filter(|(key, intent)| {
                intent.aged
                    && snapshot.is_ready(IntentKey::Maintenance {
                        key: *key,
                        generation: intent.generation,
                    })
            })
            .min_by_key(|(key, intent)| (intent.ticket, *key))
        {
            return Some(self.maintenance_decision(
                Tier::AgedMaintenance,
                maintenance.0,
                maintenance.1,
                snapshot,
                &candidates,
                &owed,
            ));
        }

        if let Some(bundle) = self.bundle_candidate(snapshot, &candidates, &owed) {
            let members = bundle
                .iter()
                .map(|candidate| candidate.admitted.clone())
                .collect::<Vec<_>>();
            let carried = self.carried_for_primaries(&bundle, snapshot);
            return Some(self.decision(
                Tier::Bundle,
                Admitted::Bundle { members },
                carried,
                snapshot,
            ));
        }

        if snapshot.retirement_wake
            && let Some(direct) = candidates.iter().find(|candidate| {
                matches!(candidate.admitted, Admitted::Direct { .. })
                    && round_robin_allows(candidate, &self.last_primary_crtcs, &owed)
                    && owed.is_subset(&candidate.crtcs)
            })
        {
            return Some(self.primary_decision(Tier::Primary, direct, snapshot));
        }

        if let Some(candidate) = candidates
            .iter()
            .filter(|candidate| round_robin_allows(candidate, &self.last_primary_crtcs, &owed))
            .min_by_key(|candidate| candidate.ordinal)
        {
            return Some(self.primary_decision(Tier::Primary, candidate, snapshot));
        }

        if let Some(maintenance) = self
            .maintenance_intents()
            .filter(|(key, intent)| {
                !intent.aged
                    && snapshot.is_ready(IntentKey::Maintenance {
                        key: *key,
                        generation: intent.generation,
                    })
                    && self.has_compatible_primary(*key, *intent, snapshot, &candidates, &owed)
            })
            .min_by_key(|(key, intent)| (intent.ticket, *key))
        {
            return Some(self.maintenance_decision(
                Tier::Maintenance,
                maintenance.0,
                maintenance.1,
                snapshot,
                &candidates,
                &owed,
            ));
        }

        self.maintenance_intents()
            .filter(|(key, intent)| {
                !intent.aged
                    && snapshot.is_ready(IntentKey::Maintenance {
                        key: *key,
                        generation: intent.generation,
                    })
            })
            .min_by_key(|(key, intent)| (intent.ticket, *key))
            .map(|(key, intent)| {
                self.maintenance_decision(
                    Tier::Maintenance,
                    key,
                    intent,
                    snapshot,
                    &candidates,
                    &owed,
                )
            })
    }

    fn bundle_candidate<'a>(
        &self,
        snapshot: &ReadinessSnapshot,
        candidates: &'a [Candidate],
        owed: &BTreeSet<CrtcId>,
    ) -> Option<Vec<&'a Candidate>> {
        if self.unflip().is_some() || !self.cursor_recovery().is_empty() {
            return None;
        }

        let members = candidates
            .iter()
            .filter(|candidate| {
                matches!(candidate.admitted, Admitted::Composed { .. })
                    && candidate
                        .crtcs
                        .iter()
                        .all(|crtc| snapshot.homogeneous_group.contains(crtc))
            })
            .collect::<Vec<_>>();
        if members.len() < 2 {
            return None;
        }

        let bundle_crtcs = members
            .iter()
            .flat_map(|candidate| candidate.crtcs.iter().copied())
            .collect::<BTreeSet<_>>();
        (bundle_crtcs.is_disjoint(&self.last_primary_crtcs) || owed.is_empty()).then_some(members)
    }

    fn has_compatible_primary(
        &self,
        key: MaintenanceKey,
        intent: MaintenanceIntent,
        snapshot: &ReadinessSnapshot,
        candidates: &[Candidate],
        owed: &BTreeSet<CrtcId>,
    ) -> bool {
        let maintenance = IntentKey::Maintenance {
            key,
            generation: intent.generation,
        };
        candidates.iter().any(|candidate| {
            candidate.crtcs.contains(&key.crtc)
                && round_robin_allows(candidate, &self.last_primary_crtcs, owed)
                && snapshot.is_compatible(maintenance, candidate.intent)
        })
    }

    fn primary_candidates(&self, snapshot: &ReadinessSnapshot) -> Vec<Candidate> {
        let mut candidates = Vec::new();

        for (crtc, composed) in self.composed_intents() {
            if self.primary_is_blocked(crtc) {
                continue;
            }

            let intent = IntentKey::Composed {
                crtc,
                generation: composed.generation,
            };
            if snapshot.is_ready(intent) {
                candidates.push(Candidate {
                    ordinal: composed.ordinal,
                    crtcs: BTreeSet::from([crtc]),
                    intent,
                    admitted: Admitted::Composed {
                        crtc,
                        generation: composed.generation,
                    },
                });
            }
        }

        if let Some(direct) = self.direct()
            && direct.successor.layout_generation == snapshot.layout_generation
            && direct.successor.topology_generation == snapshot.topology_generation
            && !direct
                .successor
                .crtcs
                .iter()
                .any(|&crtc| self.primary_is_blocked(crtc))
        {
            let intent = IntentKey::Direct {
                source_generation: direct.successor.source_generation,
            };
            if snapshot.is_ready(intent)
                && !self.direct_has_incompatible_maintenance(&direct.successor, intent, snapshot)
            {
                candidates.push(Candidate {
                    ordinal: direct.ordinal,
                    crtcs: direct.successor.crtcs.clone(),
                    intent,
                    admitted: Admitted::Direct {
                        successor: direct.successor.clone(),
                    },
                });
            }
        }

        candidates
    }

    fn primary_is_blocked(&self, crtc: CrtcId) -> bool {
        self.unflip()
            .is_some_and(|barrier| barrier.crtcs.contains(&crtc))
            || self.cursor_recovery().contains(&crtc)
    }

    fn primary_decision(
        &self,
        tier: Tier,
        candidate: &Candidate,
        snapshot: &ReadinessSnapshot,
    ) -> AdmissionDecision {
        self.decision(
            tier,
            candidate.admitted.clone(),
            self.carried_for_primary(candidate, snapshot),
            snapshot,
        )
    }

    fn direct_absorbs_all_ready_aged(
        &self,
        candidate: &Candidate,
        snapshot: &ReadinessSnapshot,
    ) -> bool {
        self.maintenance_intents().all(|(key, intent)| {
            let maintenance = IntentKey::Maintenance {
                key,
                generation: intent.generation,
            };
            !intent.aged
                || !snapshot.is_ready(maintenance)
                || (candidate.crtcs.contains(&key.crtc)
                    && snapshot.is_compatible(maintenance, candidate.intent))
        })
    }

    fn direct_has_incompatible_maintenance(
        &self,
        successor: &DirectSuccessor,
        direct: IntentKey,
        snapshot: &ReadinessSnapshot,
    ) -> bool {
        self.maintenance_intents().any(|(key, intent)| {
            successor.crtcs.contains(&key.crtc)
                && !snapshot.is_compatible(
                    IntentKey::Maintenance {
                        key,
                        generation: intent.generation,
                    },
                    direct,
                )
        })
    }

    fn maintenance_decision(
        &self,
        tier: Tier,
        key: MaintenanceKey,
        intent: MaintenanceIntent,
        snapshot: &ReadinessSnapshot,
        candidates: &[Candidate],
        owed: &BTreeSet<CrtcId>,
    ) -> AdmissionDecision {
        let maintenance_intent = IntentKey::Maintenance {
            key,
            generation: intent.generation,
        };
        let combined_primary = candidates
            .iter()
            .filter(|candidate| candidate.crtcs.contains(&key.crtc))
            .filter(|candidate| round_robin_allows(candidate, &self.last_primary_crtcs, owed))
            .filter(|candidate| snapshot.is_compatible(maintenance_intent, candidate.intent))
            .min_by_key(|candidate| candidate.ordinal);

        let (combined_primary, carried) = match combined_primary {
            Some(candidate) => (
                Some(candidate.admitted.clone()),
                self.carried_for_primary(candidate, snapshot),
            ),
            None => (
                None,
                vec![CarriedMaintenance {
                    key,
                    generation: intent.generation,
                    ticket: intent.ticket,
                }],
            ),
        };

        let mut decision = self.decision(
            tier,
            Admitted::Maintenance {
                key,
                generation: intent.generation,
            },
            carried,
            snapshot,
        );
        decision.combined_primary = combined_primary;
        decision
    }

    fn carried_for_primary(
        &self,
        candidate: &Candidate,
        snapshot: &ReadinessSnapshot,
    ) -> Vec<CarriedMaintenance> {
        self.carried_for_primaries(std::slice::from_ref(&candidate), snapshot)
    }

    fn carried_for_primaries(
        &self,
        candidates: &[&Candidate],
        snapshot: &ReadinessSnapshot,
    ) -> Vec<CarriedMaintenance> {
        self.maintenance_intents()
            .filter_map(|(key, intent)| {
                let maintenance = IntentKey::Maintenance {
                    key,
                    generation: intent.generation,
                };
                (candidates.iter().any(|candidate| {
                    candidate.crtcs.contains(&key.crtc)
                        && snapshot.is_compatible(maintenance, candidate.intent)
                }) && snapshot.is_ready(maintenance))
                .then_some(CarriedMaintenance {
                    key,
                    generation: intent.generation,
                    ticket: intent.ticket,
                })
            })
            .collect()
    }

    fn decision(
        &self,
        tier: Tier,
        admitted: Admitted,
        carried: Vec<CarriedMaintenance>,
        snapshot: &ReadinessSnapshot,
    ) -> AdmissionDecision {
        let carried_keys = carried
            .iter()
            .map(|carried| carried.key)
            .collect::<BTreeSet<_>>();
        let ages = self
            .maintenance_intents()
            .filter_map(|(key, intent)| {
                let maintenance = IntentKey::Maintenance {
                    key,
                    generation: intent.generation,
                };
                (snapshot.is_ready(maintenance) && !carried_keys.contains(&key)).then_some(key)
            })
            .collect();

        AdmissionDecision {
            tier,
            admitted,
            carried,
            combined_primary: None,
            ages,
        }
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    ordinal: PrimaryOrdinal,
    crtcs: BTreeSet<CrtcId>,
    intent: IntentKey,
    admitted: Admitted,
}

fn owed_crtcs(candidates: &[Candidate], last_primary_crtcs: &BTreeSet<CrtcId>) -> BTreeSet<CrtcId> {
    candidates
        .iter()
        .filter(|candidate| candidate.crtcs.is_disjoint(last_primary_crtcs))
        .flat_map(|candidate| candidate.crtcs.iter().copied())
        .collect()
}

fn round_robin_allows(
    candidate: &Candidate,
    last_primary_crtcs: &BTreeSet<CrtcId>,
    owed: &BTreeSet<CrtcId>,
) -> bool {
    candidate.crtcs.is_disjoint(last_primary_crtcs) || owed.is_empty()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Topology = 1,
    Unflip = 2,
    DirectSuccessor = 3,
    AgedMaintenance = 4,
    Bundle = 5,
    Primary = 6,
    Maintenance = 7,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admitted {
    Topology {
        tag:
            crate::kms::owner::lifecycle::TransitionTag<crate::kms::owner::identity::IncarnationId>,
    },
    Unflip {
        crtcs: BTreeSet<CrtcId>,
    },
    Composed {
        crtc: CrtcId,
        generation: u64,
    },
    Direct {
        successor: DirectSuccessor,
    },
    Maintenance {
        key: MaintenanceKey,
        generation: u64,
    },
    Bundle {
        members: Vec<Admitted>,
    },
    CursorRecovery {
        crtc: CrtcId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedMaintenance {
    pub key: MaintenanceKey,
    pub generation: u64,
    pub ticket: super::AdmissionTicket,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionDecision {
    pub tier: Tier,
    pub admitted: Admitted,
    pub carried: Vec<CarriedMaintenance>,
    pub combined_primary: Option<Admitted>,
    pub ages: BTreeSet<MaintenanceKey>,
}

impl AdmissionDecision {
    pub fn primary_crtcs(&self) -> BTreeSet<CrtcId> {
        let mut crtcs = primary_crtcs(&self.admitted);
        if let Admitted::Maintenance { .. } = self.admitted {
            crtcs = self
                .combined_primary
                .as_ref()
                .map_or_else(BTreeSet::new, primary_crtcs);
        }
        crtcs
    }
}

fn primary_crtcs(admitted: &Admitted) -> BTreeSet<CrtcId> {
    match admitted {
        Admitted::Topology { .. } => BTreeSet::new(),
        Admitted::Unflip { crtcs } => crtcs.clone(),
        Admitted::Composed { crtc, .. } => BTreeSet::from([*crtc]),
        Admitted::Direct { successor } => successor.crtcs.clone(),
        Admitted::Maintenance { .. } => BTreeSet::new(),
        Admitted::Bundle { members } => members.iter().flat_map(primary_crtcs).collect(),
        Admitted::CursorRecovery { crtc } => BTreeSet::from([*crtc]),
    }
}
