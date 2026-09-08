//! Commit completion deadlines and timing bounds.

use std::time::{Duration, Instant};

pub const UNKNOWN_MODE_PERIOD: Duration = Duration::from_micros(16_667);

#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum DeadlineError {
    #[error("deadline calculation overflowed")]
    Overflow,
    #[error("lifecycle duration unvalidated")]
    LifecycleUnvalidated,
}

pub fn fast_hardware(
    periods: impl IntoIterator<Item = Option<Duration>>,
) -> Result<Duration, DeadlineError> {
    let period = periods
        .into_iter()
        .map(|p| p.unwrap_or(UNKNOWN_MODE_PERIOD))
        .max()
        .unwrap_or(UNKNOWN_MODE_PERIOD);
    Ok(period
        .checked_mul(3)
        .ok_or(DeadlineError::Overflow)?
        .clamp(Duration::from_millis(100), Duration::from_secs(2)))
}

pub fn primary_event(period: Option<Duration>) -> Result<Duration, DeadlineError> {
    Ok(period
        .unwrap_or(UNKNOWN_MODE_PERIOD)
        .checked_mul(2)
        .ok_or(DeadlineError::Overflow)?
        .clamp(Duration::from_millis(50), Duration::from_millis(500)))
}

pub fn lifecycle_hardware(observed: Option<Duration>) -> Result<Duration, DeadlineError> {
    let observed = observed.ok_or(DeadlineError::LifecycleUnvalidated)?;
    if observed > Duration::from_secs(28) {
        return Err(DeadlineError::LifecycleUnvalidated);
    }
    Ok(observed
        .checked_add(Duration::from_secs(2))
        .ok_or(DeadlineError::Overflow)?
        .clamp(Duration::from_secs(10), Duration::from_secs(30)))
}

pub fn checked_deadline(now: Instant, duration: Duration) -> Result<Instant, DeadlineError> {
    now.checked_add(duration).ok_or(DeadlineError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_and_event_windows_are_independent() {
        assert_eq!(fast_hardware([None]), Ok(Duration::from_millis(100)));
        assert_eq!(
            fast_hardware([Some(Duration::from_millis(400))]),
            Ok(Duration::from_millis(1200))
        );
        assert_eq!(
            fast_hardware([Some(Duration::from_secs(1))]),
            Ok(Duration::from_secs(2))
        );
        assert_eq!(primary_event(None), Ok(Duration::from_millis(50)));
        assert_eq!(
            primary_event(Some(Duration::from_secs(1))),
            Ok(Duration::from_millis(500))
        );
        assert_eq!(
            lifecycle_hardware(None),
            Err(DeadlineError::LifecycleUnvalidated)
        );
        assert_eq!(
            lifecycle_hardware(Some(Duration::from_secs(28))),
            Ok(Duration::from_secs(30))
        );
        assert_eq!(
            lifecycle_hardware(Some(Duration::from_secs(29))),
            Err(DeadlineError::LifecycleUnvalidated)
        );
    }

    #[test]
    fn checked_deadline_overflow() {
        let now = Instant::now();
        assert_eq!(
            checked_deadline(now, Duration::MAX),
            Err(DeadlineError::Overflow)
        );
    }
}
