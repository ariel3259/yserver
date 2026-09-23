use std::sync::atomic::{AtomicU64, Ordering};

use super::{Admission, AdmissionDecision, AdmissionError, Admitted, ReadinessSnapshot};

static NEXT_LOCK_SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
#[must_use = "a locked admission must be confirmed or aborted"]
pub struct AdmissionToken {
    decision: AdmissionDecision,
    serial: u64,
}

impl AdmissionToken {
    pub fn decision(&self) -> &AdmissionDecision {
        &self.decision
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    pub decision: AdmissionDecision,
    pub sequence: u64,
}

impl Admission {
    pub fn lock(
        &mut self,
        decision: AdmissionDecision,
        snapshot: &ReadinessSnapshot,
    ) -> Result<AdmissionToken, AdmissionError> {
        if self.locked.is_some() {
            return Err(AdmissionError::AlreadyLocked);
        }
        if self.decide(snapshot) != Some(decision.clone()) {
            return Err(AdmissionError::DecisionMismatch);
        }

        let serial = NEXT_LOCK_SERIAL
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .expect("admission lock serial overflow");
        self.locked = Some(serial);

        Ok(AdmissionToken { decision, serial })
    }

    pub fn confirm(&mut self, token: AdmissionToken) -> Result<Confirmed, AdmissionError> {
        let AdmissionToken { decision, serial } = token;
        if self.locked != Some(serial) {
            return Err(AdmissionError::TokenMismatch);
        }

        if !matches!(
            decision.admitted,
            Admitted::Topology { .. } | Admitted::Unflip { .. } | Admitted::CursorRecovery { .. }
        ) {
            self.count_older_ticket_waits(&decision.carried);
        }
        self.consume(&decision.admitted);
        if let Some(combined_primary) = &decision.combined_primary {
            self.consume(combined_primary);
        }
        for carried in &decision.carried {
            self.consume_maintenance(carried);
        }
        for key in &decision.ages {
            self.age_maintenance(*key);
        }
        self.last_primary_crtcs = decision.primary_crtcs();
        self.locked = None;
        self.sequence = self
            .sequence
            .checked_add(1)
            .expect("admission sequence overflow");

        Ok(Confirmed {
            decision,
            sequence: self.sequence,
        })
    }

    pub fn abort(&mut self, token: AdmissionToken) -> Result<(), AdmissionError> {
        if self.locked != Some(token.serial) {
            return Err(AdmissionError::TokenMismatch);
        }

        self.locked = None;
        Ok(())
    }

    pub fn is_locked(&self) -> bool {
        self.locked.is_some()
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    fn consume(&mut self, admitted: &Admitted) {
        match admitted {
            Admitted::Topology { tag } => {
                if self.topology == Some(*tag) {
                    self.topology = None;
                }
            }
            Admitted::Unflip { crtcs } => {
                if self
                    .unflip
                    .as_ref()
                    .is_some_and(|barrier| barrier.crtcs == *crtcs)
                {
                    self.unflip = None;
                }
            }
            Admitted::Composed { crtc, generation } => {
                if self
                    .composed
                    .get(crtc)
                    .is_some_and(|intent| intent.generation == *generation)
                {
                    self.composed.remove(crtc);
                }
            }
            Admitted::Direct { successor } => {
                if self
                    .direct
                    .as_ref()
                    .is_some_and(|queued| queued.successor == *successor)
                {
                    self.direct = None;
                }
            }
            Admitted::Maintenance { .. } => {}
            Admitted::Bundle { members } => {
                for member in members {
                    self.consume(member);
                }
            }
            Admitted::CursorRecovery { crtc } => {
                self.cursor_recovery.remove(crtc);
            }
        }
    }

    fn consume_maintenance(&mut self, carried: &super::CarriedMaintenance) {
        if self
            .maintenance_slots
            .get(&carried.key)
            .is_some_and(|intent| {
                intent.generation == carried.generation && intent.ticket == carried.ticket
            })
        {
            self.maintenance_slots.remove(&carried.key);
        }
        self.maintenance_bounds.remove(&carried.key);
        self.maintenance_submitted.insert(
            carried.key,
            super::intents::SubmittedMaintenance {
                generation: carried.generation,
                ticket: carried.ticket,
            },
        );
    }
}
