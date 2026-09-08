use super::{
    identity::ClockEpochId,
    lifecycle::{ClockProbeId, LifecycleEpochId},
};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct ClockKey {
    pub hardware_crtc: u32,
    pub epoch: ClockEpochId,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ClockSource {
    Unresolved,
    KernelSequence,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProbeState {
    NotStarted,
    InFlight(ClockProbeId),
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ClockSample {
    pub msc: u64,
    pub ust: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ClockError {
    Unresolved,
    BadMicroseconds,
    Overflow,
    HalfRange,
    NoRepresentative,
    NegativeTimestamp,
    Regression,
}

#[derive(Debug)]
pub struct CrtcClock {
    pub key: ClockKey,
    pub lifecycle_epoch: LifecycleEpochId,
    pub topology_generation: u64,
    pub source: ClockSource,
    pub probe: ProbeState,
    pub queue_failed: bool,
    pub reference: Option<u64>,
    pub latest: Option<ClockSample>,
}

impl CrtcClock {
    pub fn new(key: ClockKey, lifecycle_epoch: LifecycleEpochId, topology_generation: u64) -> Self {
        Self {
            key,
            lifecycle_epoch,
            topology_generation,
            source: ClockSource::Unresolved,
            probe: ProbeState::NotStarted,
            queue_failed: false,
            reference: None,
            latest: None,
        }
    }

    pub fn install_reference(&mut self, sequence: u64) {
        self.source = ClockSource::KernelSequence;
        self.probe = ProbeState::Succeeded;
        self.reference = Some(sequence);
    }

    pub fn page_sample(&self, raw: u32, sec: u32, usec: u32) -> Result<ClockSample, ClockError> {
        let reference = self.reference.ok_or(ClockError::Unresolved)?;
        Ok(ClockSample {
            msc: extend_sequence(reference, raw)?,
            ust: page_ust(sec, usec)?,
        })
    }

    pub fn observe(&mut self, sample: ClockSample) -> bool {
        if let Some(latest) = self.latest {
            if sample.msc < latest.msc || sample.ust < latest.ust {
                return false;
            }
            if sample == latest {
                return false;
            }
        }
        self.latest = Some(sample);
        true
    }
}

pub fn extend_sequence(reference: u64, raw: u32) -> Result<u64, ClockError> {
    let delta = raw.wrapping_sub(reference as u32);
    if delta == 0x8000_0000 {
        return Err(ClockError::HalfRange);
    }
    let candidate = if delta < 0x8000_0000 {
        reference.checked_add(u64::from(delta))
    } else {
        reference.checked_sub(u64::from(0u32.wrapping_sub(delta)))
    };
    candidate.ok_or(ClockError::NoRepresentative)
}

pub fn page_ust(sec: u32, usec: u32) -> Result<u64, ClockError> {
    if usec >= 1_000_000 {
        return Err(ClockError::BadMicroseconds);
    }
    u64::from(sec)
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(u64::from(usec)))
        .ok_or(ClockError::Overflow)
}

#[derive(Debug)]
#[allow(dead_code)] // Fields are consumed when Task 7 issues the checked drain proof.
pub(crate) struct LegacyDrainPermit {
    pub(super) incarnation: super::identity::IncarnationId,
    pub(super) lifecycle: LifecycleEpochId,
}

impl LegacyDrainPermit {
    pub(super) fn new(
        incarnation: super::identity::IncarnationId,
        lifecycle: LifecycleEpochId,
    ) -> Self {
        Self {
            incarnation,
            lifecycle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [ID-1..3, CAP-1..4, COMMIT-2] Catches treating raw zero as fallback,
    /// choosing an ambiguous half-range representative, or unchecked UST math.
    #[test]
    fn raw_zero_is_a_real_wrap_not_a_source_switch() {
        assert_eq!(extend_sequence(0xffff_ffff, 0), Ok(0x1_0000_0000));
        assert_eq!(extend_sequence(0x1_0000_0001, 0xffff_ffff), Ok(0xffff_ffff));
        assert_eq!(extend_sequence(0, 0x8000_0000), Err(ClockError::HalfRange));
        assert_eq!(
            extend_sequence(0, 0xffff_ffff),
            Err(ClockError::NoRepresentative)
        );
        assert_eq!(page_ust(11, 22), Ok(11_000_022));
        assert_eq!(page_ust(11, 1_000_000), Err(ClockError::BadMicroseconds));
        assert_eq!(page_ust(u32::MAX, 999_999), Ok(4_294_967_295_999_999));
        let mut reference = 0xffff_fffe;
        for (raw, expected) in [
            (0xffff_ffff, 0xffff_ffff),
            (0, 0x1_0000_0000),
            (1, 0x1_0000_0001),
        ] {
            reference = extend_sequence(reference, raw).unwrap();
            assert_eq!(reference, expected);
        }
    }

    /// [ID-1..3, CAP-1..4, COMMIT-2] Catches a GET reference incorrectly
    /// manufacturing a timestamped page sample.
    #[test]
    fn installing_a_reference_does_not_manufacture_a_sample() {
        let mut clock = test_clock(1, 1);
        clock.install_reference(44);
        assert_eq!(clock.source, ClockSource::KernelSequence);
        assert_eq!(clock.probe, ProbeState::Succeeded);
        assert_eq!(clock.reference, Some(44));
        assert_eq!(clock.latest, None);
    }

    /// [ID-1..3, CAP-1..4, COMMIT-2] Catches a late sample regressing either
    /// coordinate of the trusted general clock.
    #[test]
    fn observe_never_regresses_msc_or_ust() {
        let mut clock = test_clock(1, 1);
        assert!(clock.observe(ClockSample { msc: 10, ust: 100 }));
        assert!(!clock.observe(ClockSample { msc: 9, ust: 90 }));
        assert_eq!(clock.latest, Some(ClockSample { msc: 10, ust: 100 }));
    }

    fn test_clock(hardware_crtc: u32, epoch: u64) -> CrtcClock {
        CrtcClock::new(
            ClockKey {
                hardware_crtc,
                epoch: crate::kms::owner::identity::ClockEpochId::from_raw(epoch),
            },
            crate::kms::owner::lifecycle::LifecycleEpochId::first(),
            1,
        )
    }
}
