//! Latency and evidence recording for KMS host-calls.
//!
//! The evidence recorder preallocates a fixed-capacity buffer for the whole
//! declared arm. It is single-writer, never wraps, and performs no allocation,
//! filesystem write, or buffer flush on the measured path. If capacity is exceeded,
//! the recorder records nothing further, marks itself exhausted, and exports as
//! [`EvidenceInsufficient::RecorderExhausted`].

use crate::kms::owner::identity::{CommitId, IncarnationId};

/// A single latency measurement sample collected across a host-call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Will be consumed in subsequent stages
pub(crate) struct HostCallSample {
    pub(crate) round_trip_ns: u64,
    #[allow(dead_code)] // Will be consumed in subsequent stages
    pub(crate) helper_duration_ns: u64,
    #[allow(dead_code)] // Will be consumed in subsequent stages
    pub(crate) commit: CommitId,
    #[allow(dead_code)] // Will be consumed in subsequent stages
    pub(crate) incarnation: IncarnationId,
}

/// Outcome of recording a sample into [`LatencyRecorder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Will be consumed in subsequent stages
pub(crate) enum RecordOutcome {
    Recorded,
    Exhausted,
}

/// Reasons why evidence collected for a run arm is insufficient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[allow(dead_code)] // Will be consumed in subsequent stages
pub(crate) enum EvidenceInsufficient {
    #[error("recorder buffer exhausted")]
    RecorderExhausted,
}

/// Fixed-capacity, non-wrapping latency sample recorder.
#[allow(dead_code)] // Will be consumed in subsequent stages
pub(crate) struct LatencyRecorder {
    samples: Vec<HostCallSample>,
    capacity: usize,
    exhausted: bool,
}

impl LatencyRecorder {
    /// Creates a new recorder with preallocated capacity for `capacity` samples.
    #[allow(dead_code)] // Will be consumed in subsequent stages
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
            capacity,
            exhausted: false,
        }
    }

    /// Records a single sample.
    ///
    /// If capacity has been reached or the recorder is already exhausted,
    /// marks the recorder exhausted and returns [`RecordOutcome::Exhausted`].
    /// Does not allocate or overwrite previous samples.
    #[allow(dead_code)] // Will be consumed in subsequent stages
    pub(crate) fn record(&mut self, sample: HostCallSample) -> RecordOutcome {
        if self.exhausted {
            return RecordOutcome::Exhausted;
        }
        if self.samples.len() < self.capacity {
            self.samples.push(sample);
            RecordOutcome::Recorded
        } else {
            self.exhausted = true;
            RecordOutcome::Exhausted
        }
    }

    /// Exports the recorded samples in order.
    ///
    /// If the recorder was exhausted at any point, returns
    /// [`EvidenceInsufficient::RecorderExhausted`].
    #[allow(dead_code)] // Will be consumed in subsequent stages
    pub(crate) fn export(self) -> Result<Vec<HostCallSample>, EvidenceInsufficient> {
        if self.exhausted {
            Err(EvidenceInsufficient::RecorderExhausted)
        } else {
            Ok(self.samples)
        }
    }

    /// Peeks a sample at index `idx` for tests.
    #[doc(hidden)]
    #[allow(dead_code)]
    pub(crate) fn peek_for_tests(&self, idx: usize) -> &HostCallSample {
        &self.samples[idx]
    }

    /// Returns the buffer capacity for tests.
    #[doc(hidden)]
    #[allow(dead_code)]
    pub(crate) fn capacity_for_tests(&self) -> usize {
        self.samples.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::owner::identity::{CommitId, IncarnationId};

    fn sample(value: u64) -> HostCallSample {
        HostCallSample {
            round_trip_ns: value,
            helper_duration_ns: value,
            commit: CommitId::for_tests(1),
            incarnation: IncarnationId::first(),
        }
    }

    #[test]
    fn the_recorder_never_wraps_and_reports_exhaustion() {
        let mut recorder = LatencyRecorder::with_capacity(2);
        assert_eq!(recorder.record(sample(1)), RecordOutcome::Recorded);
        assert_eq!(recorder.record(sample(2)), RecordOutcome::Recorded);
        assert_eq!(recorder.record(sample(3)), RecordOutcome::Exhausted);
        // The third sample must not have overwritten the first.
        assert_eq!(recorder.peek_for_tests(0).round_trip_ns, 1);
    }

    #[test]
    fn an_exhausted_arm_exports_as_evidence_insufficient() {
        let mut recorder = LatencyRecorder::with_capacity(1);
        recorder.record(sample(1));
        recorder.record(sample(2));
        assert!(matches!(
            recorder.export(),
            Err(EvidenceInsufficient::RecorderExhausted)
        ));
    }

    #[test]
    fn a_complete_arm_exports_every_sample_in_order() {
        let mut recorder = LatencyRecorder::with_capacity(3);
        for value in 1..=3 {
            recorder.record(sample(value));
        }
        let exported = recorder.export().expect("complete arm");
        assert_eq!(
            exported.iter().map(|s| s.round_trip_ns).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn recording_does_not_allocate_after_construction() {
        // Capacity is fixed at construction; `record` only writes into it.
        let mut recorder = LatencyRecorder::with_capacity(64);
        let before = recorder.capacity_for_tests();
        for value in 0..64 {
            recorder.record(sample(value));
        }
        assert_eq!(
            recorder.capacity_for_tests(),
            before,
            "the buffer must not grow"
        );
    }
}
