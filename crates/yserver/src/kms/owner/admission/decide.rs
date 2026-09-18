use std::collections::BTreeSet;

use super::{Admission, CrtcId, DirectSuccessor, IntentKey, PrimaryOrdinal, ReadinessSnapshot};

impl Admission {
    pub fn decide(&self, snapshot: &ReadinessSnapshot) -> Option<AdmissionDecision> {
        if let Some(generation) = self.topology() {
            return Some(AdmissionDecision {
                tier: Tier::Topology,
                admitted: Admitted::Topology { generation },
            });
        }

        if let Some(unflip) = self.unflip()
            && snapshot.is_ready(IntentKey::Unflip)
        {
            return Some(AdmissionDecision {
                tier: Tier::Unflip,
                admitted: Admitted::Unflip {
                    crtcs: unflip.crtcs.clone(),
                },
            });
        }

        let mut candidates = Vec::new();

        for (crtc, composed) in self.composed_intents() {
            if self
                .unflip()
                .is_some_and(|barrier| barrier.crtcs.contains(&crtc))
            {
                continue;
            }

            let key = IntentKey::Composed {
                crtc,
                generation: composed.generation,
            };
            if snapshot.is_ready(key) {
                candidates.push(Candidate {
                    ordinal: composed.ordinal,
                    crtcs: BTreeSet::from([crtc]),
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
            && snapshot.is_ready(IntentKey::Direct {
                source_generation: direct.successor.source_generation,
            })
        {
            candidates.push(Candidate {
                ordinal: direct.ordinal,
                crtcs: direct.successor.crtcs.clone(),
                admitted: Admitted::Direct {
                    successor: direct.successor.clone(),
                },
            });
        }

        let owed = owed_crtcs(&candidates, &self.last_primary_crtcs);

        if snapshot.retirement_wake
            && let Some(direct) = candidates.iter().find(|candidate| {
                matches!(candidate.admitted, Admitted::Direct { .. })
                    && round_robin_allows(candidate, &self.last_primary_crtcs, &owed)
                    && owed.is_subset(&candidate.crtcs)
            })
        {
            return Some(AdmissionDecision {
                tier: Tier::Primary,
                admitted: direct.admitted.clone(),
            });
        }

        candidates
            .into_iter()
            .filter(|candidate| round_robin_allows(candidate, &self.last_primary_crtcs, &owed))
            .min_by_key(|candidate| candidate.ordinal)
            .map(|candidate| AdmissionDecision {
                tier: Tier::Primary,
                admitted: candidate.admitted,
            })
    }
}

#[derive(Debug)]
struct Candidate {
    ordinal: PrimaryOrdinal,
    crtcs: BTreeSet<CrtcId>,
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
    Primary = 6,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admitted {
    Topology { generation: u64 },
    Unflip { crtcs: BTreeSet<CrtcId> },
    Composed { crtc: CrtcId, generation: u64 },
    Direct { successor: DirectSuccessor },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionDecision {
    pub tier: Tier,
    pub admitted: Admitted,
}

impl AdmissionDecision {
    pub fn primary_crtcs(&self) -> BTreeSet<CrtcId> {
        match &self.admitted {
            Admitted::Topology { .. } => BTreeSet::new(),
            Admitted::Unflip { crtcs } => crtcs.clone(),
            Admitted::Composed { crtc, .. } => BTreeSet::from([*crtc]),
            Admitted::Direct { successor } => successor.crtcs.clone(),
        }
    }
}
