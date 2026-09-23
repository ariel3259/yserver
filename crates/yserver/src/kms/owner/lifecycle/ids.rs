//! Lifecycle identities.
//!
//! `LifecycleEpochId` is always present, including during ordinary `Ready`
//! traffic (spec 6.1). A transition id is optional: ordinary commits carry
//! `None`, never a fabricated or previous id. They are separate types because
//! stage 1 stored a `ClockEpochId` where the lifecycle epoch belongs.

/// Monotonic lifecycle epoch. `ID-3` requires every executor request and
/// reply, and every commit record, to carry it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct LifecycleEpochId(u64);

impl LifecycleEpochId {
    #[allow(dead_code)] // Consumed by the wire in task 2.
    pub const fn first() -> Self {
        Self(1)
    }

    #[allow(dead_code)] // Consumed by the wire in task 2.
    pub const fn get(self) -> u64 {
        self.0
    }

    #[allow(dead_code)] // Consumed by the wire in task 2.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Exhaustion is unreachable within a process lifetime, so the production
    /// caller unwraps with a message rather than carrying a recovery branch
    /// the spec does not specify.
    #[allow(dead_code)] // Consumed by the owner in 2b.
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    #[allow(dead_code)] // Consumed by the owner in 2b.
    pub fn next(self) -> Self {
        self.checked_next().expect("lifecycle epoch exhausted")
    }
}

/// Identifies one lifecycle transition. Deliberately has no `next`: the owner
/// publishes a transition id, it is not allocated by counting. An ordinary
/// `Ready` commit carries `None`, never a fabricated or previous id.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct LifecycleTransitionId(u64);

impl LifecycleTransitionId {
    #[allow(dead_code)] // Consumed by the wire in task 2.
    pub const fn get(self) -> u64 {
        self.0
    }

    #[allow(dead_code)] // Consumed by the wire in task 2.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// Monotonic per-probe identity within an incarnation, carried by the
/// clock-probe correlation tuple (spec 6.1).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ClockProbeId(u64);

impl ClockProbeId {
    #[allow(dead_code)] // Consumed by the wire in task 2.
    pub const fn first() -> Self {
        Self(1)
    }

    #[allow(dead_code)] // Consumed by the wire in task 2.
    pub const fn get(self) -> u64 {
        self.0
    }

    #[allow(dead_code)] // Consumed by the wire in task 2.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    #[allow(dead_code)] // Consumed by the clock record in 2b.
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    #[allow(dead_code)] // Consumed by the clock record in 2b.
    pub fn next(self) -> Self {
        self.checked_next().expect("clock probe id exhausted")
    }
}

#[cfg(test)]
mod tests {
    use super::{ClockProbeId, LifecycleEpochId, LifecycleTransitionId};

    #[test]
    fn the_lifecycle_epoch_is_monotonic_and_starts_at_one() {
        let first = LifecycleEpochId::first();
        assert_eq!(first.get(), 1);
        assert_eq!(first.next().get(), 2);
        assert!(first.next() > first);
    }

    #[test]
    fn epoch_increment_is_checked_rather_than_wrapping() {
        // Spec 10: tokens are allocated with checked increment and never wrap
        // or are reused. A release build must not silently wrap.
        let last = LifecycleEpochId::from_raw(u64::MAX);
        assert_eq!(last.checked_next(), None);
    }

    #[test]
    fn a_transition_id_is_distinct_from_an_epoch_in_the_type_system() {
        // Stage 1 stored a ClockEpochId in the lifecycle-epoch field. These
        // must not be interchangeable. Equal raw values, three distinct
        // types; the newtypes are the enforcement.
        let e = LifecycleEpochId::from_raw(4);
        let t = LifecycleTransitionId::from_raw(4);
        let p = ClockProbeId::from_raw(4);
        assert_eq!(e.get(), t.get());
        assert_eq!(e.get(), p.get());
    }

    #[test]
    fn a_clock_probe_id_is_monotonic_and_checked() {
        // Spec 10: monotonic within an incarnation, never wrapping.
        let first = ClockProbeId::first();
        assert_eq!(first.get(), 1);
        assert!(first.next() > first);
        assert_eq!(ClockProbeId::from_raw(u64::MAX).checked_next(), None);
    }
}
