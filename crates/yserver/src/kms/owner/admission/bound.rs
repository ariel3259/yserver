use std::collections::BTreeSet;

use super::{Admission, AdmissionTicket, CarriedMaintenance, MaintenanceKey};

#[derive(Debug, Clone)]
pub(super) struct MaintenanceBound {
    pub(super) ticket: AdmissionTicket,
    pub(super) waited: u64,
    pub(super) older_aged: BTreeSet<MaintenanceKey>,
}

impl Admission {
    /// Reports an aged identity that has waited through more older-ticket
    /// maintenance admissions than its growing allowance permits.
    pub fn bound_violation(&self) -> Option<MaintenanceKey> {
        self.maintenance_bounds.iter().find_map(|(&key, bound)| {
            let allowance = (bound.older_aged.len() as u64).saturating_mul(2);
            (bound.waited > allowance).then_some(key)
        })
    }

    pub(super) fn age_maintenance(&mut self, key: MaintenanceKey) {
        let Some(intent) = self.maintenance_slots.get_mut(&key) else {
            return;
        };
        intent.aged = true;
        let ticket = intent.ticket;

        if let Some(bound) = self.maintenance_bounds.get_mut(&key) {
            bound.ticket = ticket;
        } else {
            let older_aged = self
                .maintenance_slots
                .iter()
                .filter_map(|(&other_key, other)| {
                    (other_key != key && other.aged && other.ticket < ticket).then_some(other_key)
                })
                .collect();
            self.maintenance_bounds.insert(
                key,
                MaintenanceBound {
                    ticket,
                    waited: 0,
                    older_aged,
                },
            );
        }

        for (&other_key, other_bound) in &mut self.maintenance_bounds {
            if other_key != key && ticket < other_bound.ticket {
                other_bound.older_aged.insert(key);
            }
        }
    }

    pub(super) fn count_older_ticket_waits(&mut self, carried: &[CarriedMaintenance]) {
        for (&key, bound) in &mut self.maintenance_bounds {
            if carried.iter().any(|item| item.key == key) {
                continue;
            }
            if carried.iter().any(|item| item.ticket < bound.ticket) {
                bound.waited = bound.waited.saturating_add(1);
            }
        }
    }
}
