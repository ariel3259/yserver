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

        self.consume(&decision.admitted);
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
            Admitted::Topology { generation } => {
                if self.topology == Some(*generation) {
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
        }
    }
}
