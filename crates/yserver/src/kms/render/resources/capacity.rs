#![allow(dead_code)]
use std::{cell::Cell, fmt, rc::Rc};

use crate::kms::render::resources::{availability::ResourceError, commit::CommitResources};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DirectRole {
    Current,
    Submitted,
    Successor,
    Preparing,
    OrdinaryRetirement,
    ExitRetirement,
}

impl DirectRole {
    pub(crate) fn index(self) -> usize {
        match self {
            DirectRole::Current => 0,
            DirectRole::Submitted => 1,
            DirectRole::Successor => 2,
            DirectRole::Preparing => 3,
            DirectRole::OrdinaryRetirement => 4,
            DirectRole::ExitRetirement => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoleState {
    Vacant,
    Reserved(u64),
    Occupied(u64),
}

pub(crate) struct RoleReservation {
    pub(crate) role: DirectRole,
    pub(crate) serial: u64,
    pub(crate) closed: Rc<Cell<bool>>,
    pub(crate) discharged: bool,
}

impl RoleReservation {
    pub(crate) fn new_for_test(role: DirectRole, serial: u64, closed: Rc<Cell<bool>>) -> Self {
        Self {
            role,
            serial,
            closed,
            discharged: false,
        }
    }

    pub(crate) fn role(&self) -> DirectRole {
        self.role
    }

    pub(crate) fn serial(&self) -> u64 {
        self.serial
    }
}

impl Drop for RoleReservation {
    fn drop(&mut self) {
        if !self.discharged {
            self.closed.set(true);
        }
    }
}

impl PartialEq for RoleReservation {
    fn eq(&self, other: &Self) -> bool {
        self.role == other.role && self.serial == other.serial
    }
}

impl Eq for RoleReservation {}

impl fmt::Debug for RoleReservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoleReservation")
            .field("role", &self.role)
            .field("serial", &self.serial)
            .field("discharged", &self.discharged)
            .finish()
    }
}

#[derive(Debug)]
pub(crate) struct DirectCapacity {
    roles: [RoleState; 6],
    next_serial: u64,
    admission_closed: Rc<Cell<bool>>,
}

impl Default for DirectCapacity {
    fn default() -> Self {
        Self::new()
    }
}

impl DirectCapacity {
    pub(crate) fn new() -> Self {
        Self {
            roles: [RoleState::Vacant; 6],
            next_serial: 0,
            admission_closed: Rc::new(Cell::new(false)),
        }
    }

    pub(crate) fn is_admission_closed(&self) -> bool {
        self.admission_closed.get()
    }

    pub(crate) fn close_admission(&mut self) {
        self.admission_closed.set(true);
    }

    pub(crate) fn occupied(&self) -> usize {
        self.roles
            .iter()
            .filter(|s| !matches!(s, RoleState::Vacant))
            .count()
    }

    pub(crate) fn is_vacant(&self, role: DirectRole) -> bool {
        self.roles[role.index()] == RoleState::Vacant
    }

    pub(crate) fn can_enter_direct(&self) -> bool {
        !self.admission_closed.get()
            && self.roles[DirectRole::OrdinaryRetirement.index()] == RoleState::Vacant
            && self.roles[DirectRole::ExitRetirement.index()] == RoleState::Vacant
    }

    pub(crate) fn reserve(&mut self, role: DirectRole) -> Result<RoleReservation, ResourceError> {
        if self.admission_closed.get() {
            return Err(ResourceError::Busy);
        }
        let idx = role.index();
        if self.roles[idx] != RoleState::Vacant {
            return Err(ResourceError::Busy);
        }
        self.next_serial = self
            .next_serial
            .checked_add(1)
            .ok_or(ResourceError::Exhausted)?;
        let serial = self.next_serial;
        self.roles[idx] = RoleState::Reserved(serial);
        Ok(RoleReservation {
            role,
            serial,
            closed: Rc::clone(&self.admission_closed),
            discharged: false,
        })
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn attach(
        &mut self,
        slot: RoleReservation,
        mut value: CommitResources,
    ) -> Result<CommitResources, (ResourceError, RoleReservation, CommitResources)> {
        let idx = slot.role.index();
        if self.roles[idx] == RoleState::Reserved(slot.serial) {
            self.roles[idx] = RoleState::Occupied(slot.serial);
            value.direct_role = Some(slot);
            Ok(value)
        } else {
            self.close_admission();
            Err((ResourceError::InvalidState, slot, value))
        }
    }

    pub(crate) fn cancel_reservation(
        &mut self,
        mut slot: RoleReservation,
    ) -> Result<(), (ResourceError, RoleReservation)> {
        let idx = slot.role.index();
        if self.roles[idx] == RoleState::Reserved(slot.serial) {
            self.roles[idx] = RoleState::Vacant;
            slot.discharged = true;
            Ok(())
        } else {
            self.close_admission();
            Err((ResourceError::InvalidState, slot))
        }
    }

    pub(crate) fn finish_role(
        &mut self,
        mut slot: RoleReservation,
    ) -> Result<(), (ResourceError, RoleReservation)> {
        let idx = slot.role.index();
        if self.roles[idx] == RoleState::Occupied(slot.serial)
            || self.roles[idx] == RoleState::Reserved(slot.serial)
        {
            self.roles[idx] = RoleState::Vacant;
            slot.discharged = true;
            Ok(())
        } else {
            self.close_admission();
            Err((ResourceError::InvalidState, slot))
        }
    }

    pub(crate) fn move_role(
        &mut self,
        slot: &mut RoleReservation,
        to: DirectRole,
    ) -> Result<(), ResourceError> {
        if slot.role == to {
            return Ok(());
        }
        let to_idx = to.index();
        if self.roles[to_idx] != RoleState::Vacant {
            return Err(ResourceError::Busy);
        }
        let from_idx = slot.role.index();
        let state = match self.roles[from_idx] {
            RoleState::Reserved(s) if s == slot.serial => RoleState::Reserved(s),
            RoleState::Occupied(s) if s == slot.serial => RoleState::Occupied(s),
            _ => {
                self.close_admission();
                return Err(ResourceError::InvalidState);
            }
        };
        self.roles[from_idx] = RoleState::Vacant;
        self.roles[to_idx] = state;
        slot.role = to;
        Ok(())
    }

    pub(crate) fn move_into_reserved(
        &mut self,
        occupied: &mut RoleReservation,
        mut reserved: RoleReservation,
    ) -> Result<(), (ResourceError, RoleReservation)> {
        if occupied.role == reserved.role {
            self.close_admission();
            return Err((ResourceError::InvalidState, reserved));
        }
        let occ_idx = occupied.role.index();
        let res_idx = reserved.role.index();
        let occ_valid = match self.roles[occ_idx] {
            RoleState::Occupied(s) | RoleState::Reserved(s) => s == occupied.serial,
            RoleState::Vacant => false,
        };
        let res_valid = match self.roles[res_idx] {
            RoleState::Reserved(s) => s == reserved.serial,
            _ => false,
        };
        if !occ_valid || !res_valid {
            self.close_admission();
            return Err((ResourceError::InvalidState, reserved));
        }
        self.roles[occ_idx] = RoleState::Vacant;
        self.roles[res_idx] = RoleState::Occupied(reserved.serial);
        occupied.role = reserved.role;
        occupied.serial = reserved.serial;
        reserved.discharged = true;
        Ok(())
    }
}
