/// Between dispatch and a typed outcome. Owns both possible states;
/// cancellation can no longer classify the request as never-submitted.
#[derive(Debug)]
pub struct Submitted<R> {
    old: Vec<R>,
    new: Vec<R>,
}

/// Accepted: both possible states stay owned until the class-specific
/// replacement rule and `PriorBufferReleased` allow a release — 2b-ii's.
#[derive(Debug)]
pub struct Accepted<R> {
    old: Vec<R>,
    new: Vec<R>,
}

/// An explicit ioctl rejection, or a refusal before any IPC. The new state
/// was never current, so its resources leave the ledger by value. The **old**
/// state is still what the hardware is scanning out, so it must leave too —
/// back to the caller, as still-current — rather than being dropped with the
/// record. Revision 2 kept it inside `Rejected` and then dropped the record,
/// which would destroy an in-use framebuffer, BO or pin (`spec:1929-1939`,
/// `spec:2127-2128`).
#[derive(Debug)]
pub struct Rejected<R> {
    old: Vec<R>,
}

impl<R> Rejected<R> {
    /// The old state, handed back as still current. Called exactly once, when
    /// the record retires.
    pub fn into_current(self) -> Vec<R> {
        self.old
    }
}

/// Acceptance neither established nor disproved. Both sets are held until
/// section 10's teardown barrier, which is stage 3's.
#[derive(Debug)]
pub struct Quarantined<R> {
    held: Vec<R>,
}

impl<R> Submitted<R> {
    pub fn new(old: Vec<R>, new: Vec<R>) -> Self {
        Self { old, new }
    }

    pub fn accepted(self) -> Accepted<R> {
        Accepted {
            old: self.old,
            new: self.new,
        }
    }

    /// The released new-state resources leave **by value**: the caller owns
    /// them and the ledger cannot hand them out a second time. The old state
    /// stays in `Rejected` until `into_current` hands it back at retirement —
    /// it is still current and must outlive the record.
    pub fn rejected(self) -> (Rejected<R>, Vec<R>) {
        (Rejected { old: self.old }, self.new)
    }

    pub fn unknown(self) -> Quarantined<R> {
        let mut held = self.old;
        held.extend(self.new);
        Quarantined { held }
    }
}

impl<R> Accepted<R> {
    pub fn unknown(self) -> Quarantined<R> {
        let mut held = self.old;
        held.extend(self.new);
        Quarantined { held }
    }
}

impl<R> Quarantined<R> {
    pub fn held(&self) -> &[R] {
        &self.held
    }
}

#[derive(Debug)]
pub enum LedgerState<R> {
    Submitted(Submitted<R>),
    Accepted(Accepted<R>),
    Rejected(Rejected<R>),
    Quarantined(Quarantined<R>),
    /// Only observable if a transition panicked mid-move. Every read treats
    /// it as quarantined: a ledger whose state is unknown releases nothing.
    Poisoned,
}

impl<R> LedgerState<R> {
    /// True when nothing may be released from this state. Used by the record
    /// so `Poisoned` is never treated as an opportunity to free something.
    pub fn releases_nothing(&self) -> bool {
        matches!(
            self,
            Self::Quarantined(_) | Self::Poisoned | Self::Accepted(_)
        )
    }
}

/// Uninhabited on purpose. **2b-i converts no call site, so it owns no KMS
/// resource** — `Vec<NeverResource>` is provably empty and every ledger
/// transition is trivially correct. 2c replaces this parameter with an enum
/// whose variants own real RAII guards, and no code in this sub-stage
/// changes when it does. That is what the type parameter is for.
///
/// Naming a handle-shaped placeholder instead would claim an ownership this
/// sub-stage cannot deliver, which is exactly the contract violation the
/// generic exists to avoid.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum NeverResource {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    /// A resource that reports its own destruction, so "exactly once" is
    /// observable rather than asserted. 2c substitutes real framebuffers.
    #[derive(Debug)]
    struct Tracked(Rc<Cell<usize>>);
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn a_rejection_hands_the_new_state_out_by_value_exactly_once() {
        // spec:2146-2148 — new KMS state is not current after an explicit
        // rejection. Handing it out by value is what makes a second release
        // impossible: the ledger no longer has it.
        let drops = Rc::new(Cell::new(0));
        let ledger = Submitted::new(
            vec![Tracked(Rc::clone(&drops))],
            vec![Tracked(Rc::clone(&drops))],
        );
        let (rejected, released) = ledger.rejected();
        assert_eq!(released.len(), 1);
        assert_eq!(drops.get(), 0, "handing over is not dropping");
        drop(released);
        assert_eq!(drops.get(), 1, "the never-current new state is destroyed");

        // The old state is still what the hardware is scanning out. It leaves by
        // value too; dropping `Rejected` must not destroy it.
        let still_current = rejected.into_current();
        assert_eq!(still_current.len(), 1);
        assert_eq!(
            drops.get(),
            1,
            "retiring the ledger did not destroy the old state"
        );
        drop(still_current);
        assert_eq!(
            drops.get(),
            2,
            "it is destroyed only when its new owner drops it"
        );
    }

    #[test]
    fn acceptance_releases_nothing_and_keeps_both_states() {
        // spec:2149-2151 — the pending record owns all possible old/new state
        // after acceptance; release waits for PriorBufferReleased, which is
        // 2b-ii's.
        let drops = Rc::new(Cell::new(0));
        let accepted = Submitted::new(
            vec![Tracked(Rc::clone(&drops))],
            vec![Tracked(Rc::clone(&drops))],
        )
        .accepted();
        assert_eq!(drops.get(), 0);
        let quarantined = accepted.unknown();
        assert_eq!(
            quarantined.held().len(),
            2,
            "both states survive into quarantine"
        );
        assert_eq!(drops.get(), 0);
    }

    #[test]
    fn quarantine_holds_both_states_and_offers_no_way_out() {
        // spec:2160-2162. This test is a statement about the API surface: there
        // is no method on Quarantined<R> that yields a resource, so a later
        // explicit result cannot release one. If someone adds one, this stops
        // being true and the reviewer should ask why.
        let drops = Rc::new(Cell::new(0));
        let q = Submitted::new(
            vec![Tracked(Rc::clone(&drops))],
            vec![Tracked(Rc::clone(&drops))],
        )
        .unknown();
        assert_eq!(q.held().len(), 2);
        assert_eq!(drops.get(), 0);
    }

    #[test]
    fn the_ledger_state_enum_treats_poisoned_as_holding_everything() {
        let state: LedgerState<Tracked> = LedgerState::Poisoned;
        assert!(
            state.releases_nothing(),
            "an unknown ledger state releases nothing"
        );
    }
}
