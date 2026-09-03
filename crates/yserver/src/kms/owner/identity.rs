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
    pub(crate) const fn next(self) -> Self {
        Self(self.0 + 1)
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

    #[allow(dead_code)] // Will be consumed in Task 7
    pub(crate) const fn from_user_data(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    #[doc(hidden)]
    #[allow(dead_code)] // Will be consumed in Task 7
    pub(crate) const fn for_tests(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct SequenceArmToken(u64);

impl SequenceArmToken {
    #[allow(dead_code)] // Will be consumed in Task 5
    pub(crate) const fn as_user_data(self) -> u64 {
        self.0
    }

    #[allow(dead_code)] // Will be consumed in Task 5
    pub(crate) const fn from_user_data(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    #[doc(hidden)]
    #[allow(dead_code)] // Will be consumed in Task 5
    pub(crate) const fn for_tests(raw: u64) -> Self {
        Self(raw)
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
    pub(crate) const fn next(self) -> Self {
        Self(self.0 + 1)
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

    #[allow(dead_code)] // Will be consumed in Task 8
    pub(crate) fn next_commit(&mut self) -> CommitId {
        let id = CommitId(self.next_commit);
        self.next_commit += 1;
        id
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    fn next_tagged(&mut self, purpose: u64) -> u64 {
        let counter = self.next_counter & COUNTER_MASK;
        self.next_counter += 1;
        (purpose << PURPOSE_SHIFT) | counter
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub(crate) fn next_event_token(&mut self) -> EventToken {
        EventToken(self.next_tagged(PURPOSE_EVENT))
    }

    #[allow(dead_code)] // Will be consumed in Task 8
    pub(crate) fn next_sequence_arm(&mut self) -> SequenceArmToken {
        SequenceArmToken(self.next_tagged(PURPOSE_SEQUENCE_ARM))
    }
}

#[cfg(test)]
mod tests {
    use super::{EventToken, IdentityAllocator, IncarnationId};

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
}
