#![allow(dead_code)]
use std::collections::{BTreeSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ResourceConsumer {
    Pool,
    DirectCapacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ResourceWaiter {
    pub(crate) generation: u64,
    pub(crate) consumer: ResourceConsumer,
}

impl ResourceWaiter {
    pub(crate) fn new(generation: u64, consumer: ResourceConsumer) -> Self {
        Self {
            generation,
            consumer,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct WaiterRegistry {
    waiters: BTreeSet<ResourceWaiter>,
    pending_wakes: VecDeque<ResourceConsumer>,
    enqueued_wakes: BTreeSet<ResourceConsumer>,
}

impl WaiterRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register(&mut self, waiter: ResourceWaiter) {
        self.waiters.insert(waiter);
    }

    pub(crate) fn is_registered(&self, waiter: &ResourceWaiter) -> bool {
        self.waiters.contains(waiter)
    }

    /// On an eligibility edge for `generation`, enqueues a wake for each registered waiter,
    /// clears their registration, and coalesces the wakes so each consumer is queued once.
    pub(crate) fn notify_eligible(&mut self, generation: u64) {
        let matching: Vec<ResourceWaiter> = self
            .waiters
            .iter()
            .copied()
            .filter(|w| w.generation == generation)
            .collect();

        for waiter in matching {
            self.waiters.remove(&waiter);
            if self.enqueued_wakes.insert(waiter.consumer) {
                self.pending_wakes.push_back(waiter.consumer);
            }
        }
    }

    pub(crate) fn pop_wake(&mut self) -> Option<ResourceConsumer> {
        let consumer = self.pending_wakes.pop_front()?;
        self.enqueued_wakes.remove(&consumer);
        Some(consumer)
    }

    pub(crate) fn has_pending_wakes(&self) -> bool {
        !self.pending_wakes.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.waiters.clear();
        self.pending_wakes.clear();
        self.enqueued_wakes.clear();
    }
}
