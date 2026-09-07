//! Typed C.0 identities.
//!
//! Every identity is incarnation-scoped and monotonic. The kernel echoes
//! `user_data` verbatim, so the owner must be able to tell its own live token
//! from a stale one, from another purpose's token, and from zero — the spec
//! requires a zero or unknown token on a current tagged event to poison the
//! incarnation rather than to be accepted.

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct IncarnationId(u64);

impl IncarnationId {
    #[allow(dead_code)] // Will be consumed in Task 7
    pub const fn first() -> Self {
        Self(1)
    }

    #[allow(dead_code)] // Will be consumed in Task 7
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    #[allow(dead_code)] // Will be consumed in Task 7
    pub fn next(self) -> Self {
        self.checked_next().expect("incarnation id exhausted")
    }

    #[allow(dead_code)] // Will be consumed in Task 7
    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct CommitId(u64);

impl CommitId {
    #[doc(hidden)]
    #[allow(dead_code)] // Will be consumed in Task 7
    pub const fn for_tests(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct EventToken(u64);

impl EventToken {
    #[allow(dead_code)] // Will be consumed in Task 7
    pub const fn as_user_data(self) -> u64 {
        self.0
    }

    /// Rejects zero. Target kind comes from the live/tombstoned owner record,
    /// never raw-bit decoding.
    #[allow(dead_code)] // Will be consumed in Task 7
    pub const fn from_user_data(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    #[doc(hidden)]
    pub const fn for_tests(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SequenceArmToken(u64);

impl SequenceArmToken {
    #[allow(dead_code)] // Will be consumed in Task 5
    pub const fn as_user_data(self) -> u64 {
        self.0
    }

    /// Rejects zero. Target kind comes from the live/tombstoned owner record,
    /// never raw-bit decoding.
    #[allow(dead_code)] // Will be consumed in Task 5
    pub const fn from_user_data(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    #[doc(hidden)]
    pub const fn for_tests(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ClockEpochId(u64);

impl ClockEpochId {
    #[allow(dead_code)] // Will be consumed in Task 5
    pub const fn first() -> Self {
        Self(1)
    }

    #[allow(dead_code)] // Will be consumed in Task 6
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    #[allow(dead_code)] // Will be consumed in Task 6
    pub fn next(self) -> Self {
        self.checked_next().expect("clock epoch exhausted")
    }

    #[allow(dead_code)] // Will be consumed in Task 6
    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug)]
pub struct IdentityAllocator {
    #[allow(dead_code)] // Will be consumed in Task 8
    incarnation: IncarnationId,
    #[allow(dead_code)] // Will be consumed in Task 8
    next_commit: u64,
    #[allow(dead_code)] // Will be consumed in Task 8
    next_token: u64,
}

impl IdentityAllocator {
    #[allow(dead_code)] // Will be consumed in Task 8
    pub fn new(incarnation: IncarnationId) -> Self {
        Self {
            incarnation,
            next_commit: 1,
            next_token: 0,
        }
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }

    /// An allocator whose counters are already past their last usable value,
    /// so exhaustion is reachable in a test without issuing 2^64 tokens.
    #[doc(hidden)]
    #[cfg(test)]
    pub fn at_limit_for_tests() -> Self {
        Self {
            incarnation: IncarnationId::first(),
            next_commit: u64::MAX,
            next_token: u64::MAX,
        }
    }

    /// `None` once the commit counter can no longer advance. Exhaustion is
    /// unreachable in a process lifetime, so `next_commit` unwraps; the
    /// checked form exists because the spec forbids silent wrapping.
    #[allow(dead_code)] // Will be consumed in Task 8
    pub fn checked_next_commit(&mut self) -> Option<CommitId> {
        let id = self.next_commit;
        self.next_commit = self.next_commit.checked_add(1)?;
        Some(CommitId(id))
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub fn next_commit(&mut self) -> CommitId {
        self.checked_next_commit()
            .expect("commit id space exhausted")
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    fn checked_next_token(&mut self) -> Option<u64> {
        self.next_token = self.next_token.checked_add(1)?;
        Some(self.next_token)
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub fn checked_next_event_token(&mut self) -> Option<EventToken> {
        self.checked_next_token().map(EventToken)
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub fn checked_next_sequence_arm(&mut self) -> Option<SequenceArmToken> {
        self.checked_next_token().map(SequenceArmToken)
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub fn next_event_token(&mut self) -> EventToken {
        self.checked_next_event_token()
            .expect("event token space exhausted")
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub fn next_sequence_arm(&mut self) -> SequenceArmToken {
        self.checked_next_sequence_arm()
            .expect("sequence arm token space exhausted")
    }
}

#[cfg(test)]
mod tests {
    use super::{ClockEpochId, EventToken, IdentityAllocator, IncarnationId, SequenceArmToken};

    #[test]
    fn commit_ids_are_monotonic_within_an_incarnation() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let a = ids.next_commit();
        let b = ids.next_commit();
        assert!(b > a, "commit ids must increase");
    }

    #[test]
    fn event_tokens_never_repeat_within_an_incarnation() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(ids.next_event_token()), "event token reused");
        }
    }

    #[test]
    fn a_zero_user_data_is_not_a_valid_event_token() {
        assert!(EventToken::from_user_data(0).is_none());
        assert!(SequenceArmToken::from_user_data(0).is_none());
    }

    #[test]
    fn an_event_token_round_trips_through_user_data() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let token = ids.next_event_token();
        assert_eq!(
            EventToken::from_user_data(token.as_user_data()),
            Some(token)
        );
    }

    #[test]
    fn alternating_atomic_and_sequence_allocations_yield_consecutive_raw_values() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let event1 = ids.next_event_token();
        let arm = ids.next_sequence_arm();
        let event2 = ids.next_event_token();
        assert_eq!(event1.as_user_data(), 1);
        assert_eq!(arm.as_user_data(), 2);
        assert_eq!(event2.as_user_data(), 3);
    }

    #[test]
    fn identity_allocation_is_checked_at_the_counter_limit() {
        let mut alloc = IdentityAllocator::at_limit_for_tests();
        assert_eq!(alloc.checked_next_commit(), None);
        assert_eq!(alloc.checked_next_event_token(), None);
        assert_eq!(alloc.checked_next_sequence_arm(), None);
    }

    #[test]
    fn two_incarnations_with_equal_raw_values_selected_by_source() {
        let mut first = IdentityAllocator::new(IncarnationId::first());
        let mut second = IdentityAllocator::new(IncarnationId::first().next());
        assert_eq!(first.next_event_token().as_user_data(), 1);
        assert_eq!(second.next_event_token().as_user_data(), 1);
    }

    #[test]
    fn incarnation_and_clock_epoch_increments_are_checked() {
        assert_eq!(IncarnationId::from_raw(u64::MAX).checked_next(), None);
        assert_eq!(ClockEpochId::from_raw(u64::MAX).checked_next(), None);
        assert_eq!(IncarnationId::first().next().get(), 2);
        assert_eq!(ClockEpochId::first().next().get(), 2);
    }
}
