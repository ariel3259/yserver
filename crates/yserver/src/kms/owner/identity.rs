//! Typed C.0 identities.
//!
//! Every identity is incarnation-scoped and monotonic. The kernel echoes
//! `user_data` verbatim, so the owner must be able to tell its own live token
//! from a stale one, from another purpose's token, and from zero — the spec
//! requires a zero or unknown token on a current tagged event to poison the
//! incarnation rather than to be accepted.

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct IncarnationId(u64);

impl IncarnationId {
    #[allow(dead_code)] // Will be consumed in Task 7
    pub(crate) const fn first() -> Self {
        Self(1)
    }

    #[allow(dead_code)] // Will be consumed in Task 7
    pub(crate) const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    #[allow(dead_code)] // Will be consumed in Task 7
    pub(crate) fn next(self) -> Self {
        self.checked_next().expect("incarnation id exhausted")
    }

    #[allow(dead_code)] // Will be consumed in Task 7
    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct CommitId(u64);

impl CommitId {
    #[doc(hidden)]
    #[allow(dead_code)] // Will be consumed in Task 7
    pub(crate) const fn for_tests(raw: u64) -> Self {
        Self(raw)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct EventToken(u64);

impl EventToken {
    #[allow(dead_code)] // Will be consumed in Task 7
    pub(crate) const fn as_user_data(self) -> u64 {
        self.0
    }

    /// Rejects zero, and rejects any value whose purpose tag is not this
    /// token's own. The module doc has always claimed a token is
    /// distinguishable "from another purpose's token"; before this check it
    /// was not, and each decoder happily accepted the other's tokens.
    #[allow(dead_code)] // Will be consumed in Task 7
    pub(crate) const fn from_user_data(raw: u64) -> Option<Self> {
        if raw == 0 || (raw >> PURPOSE_SHIFT) != PURPOSE_EVENT {
            None
        } else {
            Some(Self(raw))
        }
    }

    #[doc(hidden)]
    #[allow(dead_code)] // Will be consumed in Task 7
    pub(crate) const fn for_tests(raw: u64) -> Self {
        Self(raw)
    }

    /// A token carrying the correct purpose tag, for tests that put one on
    /// the wire. `for_tests` stores its argument verbatim, so a small literal
    /// built with it is rejected by `from_user_data` above.
    #[doc(hidden)]
    #[allow(dead_code)] // Will be consumed in Task 2.
    pub(crate) const fn tagged_for_tests(counter: u64) -> Self {
        Self((PURPOSE_EVENT << PURPOSE_SHIFT) | (counter & COUNTER_MASK))
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct SequenceArmToken(u64);

impl SequenceArmToken {
    #[allow(dead_code)] // Will be consumed in Task 5
    pub(crate) const fn as_user_data(self) -> u64 {
        self.0
    }

    /// Rejects zero, and rejects any value whose purpose tag is not this
    /// token's own — including an echoed event token whose counter happens
    /// to match.
    #[allow(dead_code)] // Will be consumed in Task 5
    pub(crate) const fn from_user_data(raw: u64) -> Option<Self> {
        if raw == 0 || (raw >> PURPOSE_SHIFT) != PURPOSE_SEQUENCE_ARM {
            None
        } else {
            Some(Self(raw))
        }
    }

    #[doc(hidden)]
    #[allow(dead_code)] // Will be consumed in Task 5
    pub(crate) const fn for_tests(raw: u64) -> Self {
        Self(raw)
    }

    #[doc(hidden)]
    #[allow(dead_code)] // Will be consumed in Task 2.
    pub(crate) const fn tagged_for_tests(counter: u64) -> Self {
        Self((PURPOSE_SEQUENCE_ARM << PURPOSE_SHIFT) | (counter & COUNTER_MASK))
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct ClockEpochId(u64);

impl ClockEpochId {
    #[allow(dead_code)] // Will be consumed in Task 5
    pub(crate) const fn first() -> Self {
        Self(1)
    }

    #[allow(dead_code)] // Will be consumed in Task 6
    pub(crate) const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    #[allow(dead_code)] // Will be consumed in Task 6
    pub(crate) fn next(self) -> Self {
        self.checked_next().expect("clock epoch exhausted")
    }

    #[allow(dead_code)] // Will be consumed in Task 6
    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// Purpose tags occupy the top bits so an event token and a sequence-arm
/// token can never be mistaken for each other in an echoed `user_data`.
///
/// Both purposes draw from ONE counter, not one per purpose. Section 6.1
/// allocates from a single monotonic namespace across the complete device
/// incarnation; two independent counters would still pass a per-type
/// uniqueness test while quietly breaking that property, and nothing
/// downstream would notice until two tokens of different purposes shared a
/// counter value in a log.
#[allow(dead_code)] // Will be consumed in Task 8
const PURPOSE_SHIFT: u32 = 62;
#[allow(dead_code)] // Will be consumed in Task 8
const PURPOSE_EVENT: u64 = 1;
#[allow(dead_code)] // Will be consumed in Task 8
const PURPOSE_SEQUENCE_ARM: u64 = 2;
#[allow(dead_code)] // Will be consumed in Task 8
const COUNTER_MASK: u64 = (1 << PURPOSE_SHIFT) - 1;

pub(crate) struct IdentityAllocator {
    #[allow(dead_code)] // Will be consumed in Task 8
    incarnation: IncarnationId,
    #[allow(dead_code)] // Will be consumed in Task 8
    next_commit: u64,
    #[allow(dead_code)] // Will be consumed in Task 8
    next_counter: u64,
}

impl IdentityAllocator {
    #[allow(dead_code)] // Will be consumed in Task 8
    pub(crate) fn new(incarnation: IncarnationId) -> Self {
        // Seeding the counter from the incarnation keeps a fresh incarnation
        // from reissuing a token the previous one may still see echoed.
        Self {
            incarnation,
            next_commit: 1,
            next_counter: incarnation.get() << 32 | 1,
        }
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub(crate) fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }

    /// An allocator whose counters are already past their last usable value,
    /// so exhaustion is reachable in a test without issuing 2^62 tokens.
    #[doc(hidden)]
    #[cfg(test)]
    pub(crate) fn at_limit_for_tests() -> Self {
        Self {
            incarnation: IncarnationId::first(),
            next_commit: u64::MAX,
            next_counter: COUNTER_MASK + 1,
        }
    }

    /// `None` once the commit counter can no longer advance. Exhaustion is
    /// unreachable in a process lifetime, so `next_commit` unwraps; the
    /// checked form exists because the spec forbids silent wrapping.
    #[allow(dead_code)] // Will be consumed in Task 8
    pub(crate) fn checked_next_commit(&mut self) -> Option<CommitId> {
        let next = self.next_commit.checked_add(1)?;
        let id = CommitId(self.next_commit);
        self.next_commit = next;
        Some(id)
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub(crate) fn next_commit(&mut self) -> CommitId {
        self.checked_next_commit()
            .expect("commit id space exhausted")
    }

    /// The tagged counter is bounded by `COUNTER_MASK`, not `u64::MAX`,
    /// because the purpose tag occupies the top two bits. Advancing past the
    /// mask would silently wrap a counter back into a value already issued.
    #[allow(dead_code)] // Will be consumed in Task 8
    fn checked_next_tagged(&mut self, purpose: u64) -> Option<u64> {
        if self.next_counter > COUNTER_MASK {
            return None;
        }
        let counter = self.next_counter;
        self.next_counter += 1;
        Some((purpose << PURPOSE_SHIFT) | counter)
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub(crate) fn checked_next_event_token(&mut self) -> Option<EventToken> {
        self.checked_next_tagged(PURPOSE_EVENT).map(EventToken)
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub(crate) fn checked_next_sequence_arm(&mut self) -> Option<SequenceArmToken> {
        self.checked_next_tagged(PURPOSE_SEQUENCE_ARM)
            .map(SequenceArmToken)
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub(crate) fn next_event_token(&mut self) -> EventToken {
        self.checked_next_event_token()
            .expect("event token space exhausted")
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub(crate) fn next_sequence_arm(&mut self) -> SequenceArmToken {
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
    fn a_new_incarnation_does_not_reissue_the_previous_incarnations_tokens() {
        let mut first = IdentityAllocator::new(IncarnationId::first());
        let stale = first.next_event_token();
        let mut second = IdentityAllocator::new(IncarnationId::first().next());
        let fresh = second.next_event_token();
        assert_ne!(
            stale, fresh,
            "a fresh incarnation must not collide with the old one"
        );
    }

    #[test]
    fn sequence_arm_tokens_are_distinct_from_event_tokens() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let event = ids.next_event_token();
        let arm = ids.next_sequence_arm();
        assert_ne!(event.as_user_data(), arm.as_user_data());
    }

    #[test]
    fn identity_allocation_is_checked_at_the_counter_limit() {
        let mut alloc = IdentityAllocator::at_limit_for_tests();
        assert_eq!(alloc.checked_next_commit(), None);
        assert_eq!(alloc.checked_next_event_token(), None);
        assert_eq!(alloc.checked_next_sequence_arm(), None);
    }

    #[test]
    fn the_purpose_tag_never_collides_with_the_counter() {
        let mut alloc = IdentityAllocator::new(IncarnationId::first());
        let event = alloc.checked_next_event_token().expect("token");
        let arm = alloc.checked_next_sequence_arm().expect("arm");
        assert_ne!(event.as_user_data(), arm.as_user_data());
        assert!(EventToken::from_user_data(arm.as_user_data()).is_none());
        assert!(SequenceArmToken::from_user_data(event.as_user_data()).is_none());
    }

    #[test]
    fn a_tagged_test_token_survives_its_own_decoder() {
        // `for_tests` stores a raw value verbatim, so a small literal built
        // with it is rejected once the decoder checks the purpose tag. Wire
        // tests must use `tagged_for_tests`.
        assert!(EventToken::from_user_data(EventToken::for_tests(0x66).as_user_data()).is_none());
        let tagged = EventToken::tagged_for_tests(0x66);
        assert_eq!(
            EventToken::from_user_data(tagged.as_user_data()),
            Some(tagged)
        );
        let tagged_arm = SequenceArmToken::tagged_for_tests(0x66);
        assert_eq!(
            SequenceArmToken::from_user_data(tagged_arm.as_user_data()),
            Some(tagged_arm)
        );
    }

    #[test]
    fn incarnation_and_clock_epoch_increments_are_checked() {
        // The global constraint says identity allocation is checked and cannot
        // wrap; these two were still unchecked `+ 1`.
        assert_eq!(IncarnationId::from_raw(u64::MAX).checked_next(), None);
        assert_eq!(ClockEpochId::from_raw(u64::MAX).checked_next(), None);
        assert_eq!(IncarnationId::first().next().get(), 2);
        assert_eq!(ClockEpochId::first().next().get(), 2);
    }
}
