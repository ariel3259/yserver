//! X11 Present extension MSC arithmetic and due-classification helpers.
//!
//! `msc_is_after` and `effective_target_msc` port the Xorg MSC-comparison
//! and target-MSC computation used to schedule Present requests.
//! `classify_msc_due` decides whether a parked Present's deferred Copy
//! execution is due now against the general vblank clock (spec §msc-due).

/// `PresentOptionAsync` bit per `presentproto`.
pub const PRESENT_OPTION_ASYNC: u32 = 0x1;
/// `PresentOptionAsyncMayTear` bit per `presentproto` (presenttokens.h).
pub const PRESENT_OPTION_ASYNC_MAY_TEAR: u32 = 0x10;
/// Xorg `PresentAllAsyncOptions` = Async | AsyncMayTear. Any of these bits
/// means the present is not synced-to-vblank and is never bumped forward.
pub const PRESENT_ALL_ASYNC_OPTIONS: u32 = PRESENT_OPTION_ASYNC | PRESENT_OPTION_ASYNC_MAY_TEAR;

/// MSC comparison with 64-bit wraparound, matching Xorg `msc_is_after`
/// (`(int64_t)(a - b) > 0`). True when `a` is strictly after `b`.
#[must_use]
pub fn msc_is_after(a: u64, b: u64) -> bool {
    (a.wrapping_sub(b) as i64) > 0
}

/// Effective target MSC for a Present request — a port of Xorg
/// `present_get_target_msc` (`../xserver/present/present.c:155`). A synced
/// present whose target already passed defers to the next field; that is
/// the throttle. Caller must reject invalid divisor/remainder first
/// (Task 3) so the modulo arithmetic is well-defined.
#[must_use]
pub fn effective_target_msc(
    target_msc_arg: u64,
    crtc_msc: u64,
    divisor: u64,
    remainder: u64,
    options: u32,
) -> u64 {
    let synced = (options & PRESENT_ALL_ASYNC_OPTIONS) == 0;
    if msc_is_after(target_msc_arg, crtc_msc) {
        return target_msc_arg;
    }
    if divisor == 0 {
        let mut target = crtc_msc;
        if synced {
            target += 1;
        }
        return target;
    }
    let mut target = crtc_msc - (crtc_msc % divisor) + remainder;
    if msc_is_after(target, crtc_msc) {
        return target;
    }
    if synced || msc_is_after(crtc_msc, target) {
        target += divisor;
    }
    target
}

/// Outcome of [`classify_msc_due`]: whether a parked Present's Copy
/// execution should happen now or stay parked for a later evaluation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MscDue {
    /// Execute the Copy now.
    ExecuteNow,
    /// Park (or stay parked) — the msc-due fallback ladder (spec §msc-due)
    /// decides how a still-parked entry eventually becomes due.
    Park,
}

/// Classify whether a Present's deferred Copy execution is due now, per
/// spec §msc-due. `eff` is the request's `effective_target_msc` (`None` for
/// async presents and no-clock environments — nested/headless, pre-first-
/// flip KMS — which collapse the whole due rule to "always now", spec
/// "Unified pending-present store"). `clock_msc` MUST be the **general**
/// vblank clock (`Backend::present_get_ust_msc`), never
/// `present_get_completion_clock` — spec "Loop-order and clock contract"
/// item 2: the two intentionally diverge, and classifying against the
/// completion clock would reclassify a present arriving between an
/// active-display sequence sample and a flip retire as future-target,
/// adding a spurious extra period of latency on exactly the hardware the
/// 2026-07-27 pacing fix was validated on. `flip_in_flight` is
/// `Backend::present_flip_in_flight()`.
///
/// - `eff <= clock_msc` (wrap-safe via [`msc_is_after`]): late or already
///   due — execute now (Xorg completes a late present at the actual MSC
///   rather than dropping it).
/// - `eff == clock_msc + 1`: the very next vblank. Execute now iff no flip
///   is already in flight for it (the compose that would carry this copy
///   has not been submitted yet); otherwise this copy cannot make that
///   compose and must park for the flip-retirement wakeup.
/// - `eff > clock_msc + 1`: future target — park. The fallback ladder
///   (absolute vblank arm / idle-display / blackout) decides how it
///   becomes due.
#[must_use]
pub fn classify_msc_due(eff: Option<u64>, clock_msc: u64, flip_in_flight: bool) -> MscDue {
    let Some(eff) = eff else {
        // Async present (PresentOptionAsync): cannot flip before the current
        // in-flight flip retires. Park to the next vblank so a no-vsync flood
        // supersedes instead of shedding every present onto the per-present
        // Copy path (spec 2026-08-11-async-present-defer-supersession §1).
        // Nested/headless runs always report flip_in_flight == false, so the
        // no-clock "always now" behavior is preserved there.
        return if flip_in_flight { MscDue::Park } else { MscDue::ExecuteNow };
    };
    if !msc_is_after(eff, clock_msc) {
        return MscDue::ExecuteNow;
    }
    if eff == clock_msc.wrapping_add(1) {
        return if flip_in_flight {
            MscDue::Park
        } else {
            MscDue::ExecuteNow
        };
    }
    MscDue::Park
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msc_is_after_handles_wrap() {
        assert!(super::msc_is_after(11, 10));
        assert!(!super::msc_is_after(10, 10));
        assert!(!super::msc_is_after(9, 10));
        assert!(super::msc_is_after(0, u64::MAX));
        assert!(!super::msc_is_after(u64::MAX, 0));
    }

    #[test]
    fn effective_target_future_used_as_is() {
        assert_eq!(super::effective_target_msc(100, 10, 0, 0, 0), 100);
        assert_eq!(
            super::effective_target_msc(100, 10, 0, 0, super::PRESENT_OPTION_ASYNC),
            100
        );
    }

    #[test]
    fn effective_target_divisor0_synced_past_bumps_to_next_vblank() {
        assert_eq!(super::effective_target_msc(5, 10, 0, 0, 0), 11);
        assert_eq!(super::effective_target_msc(10, 10, 0, 0, 0), 11);
        assert_eq!(super::effective_target_msc(0, 10, 0, 0, 0), 11);
    }

    #[test]
    fn effective_target_divisor0_async_past_is_now() {
        assert_eq!(
            super::effective_target_msc(5, 10, 0, 0, super::PRESENT_OPTION_ASYNC),
            10
        );
        assert_eq!(
            super::effective_target_msc(5, 10, 0, 0, super::PRESENT_OPTION_ASYNC_MAY_TEAR),
            10
        );
    }

    #[test]
    fn effective_target_divisor_modulo_examples() {
        // Xorg example: crtc_msc=10, divisor=4.
        assert_eq!(super::effective_target_msc(0, 10, 4, 3, 0), 11);
        assert_eq!(super::effective_target_msc(0, 10, 4, 2, 0), 14);
        assert_eq!(
            super::effective_target_msc(0, 10, 4, 2, super::PRESENT_OPTION_ASYNC),
            10
        );
        assert_eq!(super::effective_target_msc(0, 10, 4, 1, 0), 13);
        assert_eq!(super::effective_target_msc(0, 10, 4, 0, 0), 12);
    }

    // Task 7 Step 1 — pure classifier (spec §msc-due).

    #[test]
    fn classify_msc_due_async_parked_while_flip_in_flight() {
        // Async present with a flip in flight: park to the next vblank so a
        // no-vsync flood supersedes instead of shedding onto the Copy path.
        assert_eq!(classify_msc_due(None, 12345, true), MscDue::Park);
        // No flip in flight: execute now (also the nested/headless no-clock path).
        assert_eq!(classify_msc_due(None, 0, false), MscDue::ExecuteNow);
    }

    #[test]
    fn classify_msc_due_async_parked_when_flip_in_flight() {
        assert_eq!(super::classify_msc_due(None, 10, true), super::MscDue::Park);
    }

    #[test]
    fn classify_msc_due_async_executes_now_without_flip_in_flight() {
        assert_eq!(super::classify_msc_due(None, 10, false), super::MscDue::ExecuteNow);
    }

    #[test]
    fn classify_msc_due_late_or_due_executes_now() {
        // eff <= clock_msc: late (Xorg completes a late present at the
        // actual MSC rather than dropping it) or exactly due.
        assert_eq!(classify_msc_due(Some(9), 10, false), MscDue::ExecuteNow);
        assert_eq!(classify_msc_due(Some(10), 10, false), MscDue::ExecuteNow);
        // flip_in_flight is irrelevant once the target has already passed.
        assert_eq!(classify_msc_due(Some(10), 10, true), MscDue::ExecuteNow);
    }

    #[test]
    fn classify_msc_due_immediate_target_gated_on_flip_in_flight() {
        // eff == clock_msc + 1: the next vblank. Executes now iff the
        // compose that would carry it hasn't already been submitted.
        assert_eq!(classify_msc_due(Some(11), 10, false), MscDue::ExecuteNow);
        assert_eq!(classify_msc_due(Some(11), 10, true), MscDue::Park);
    }

    #[test]
    fn classify_msc_due_future_target_parks_regardless_of_flip() {
        // eff > clock_msc + 1: parks either way — the fallback ladder
        // decides how it becomes due, not the flip bit.
        assert_eq!(classify_msc_due(Some(12), 10, false), MscDue::Park);
        assert_eq!(classify_msc_due(Some(100), 10, true), MscDue::Park);
    }

    #[test]
    fn classify_msc_due_wraps_near_u64_max() {
        // clock at u64::MAX: "next vblank" wraps to 0.
        assert_eq!(
            classify_msc_due(Some(0), u64::MAX, false),
            MscDue::ExecuteNow
        );
        assert_eq!(classify_msc_due(Some(0), u64::MAX, true), MscDue::Park);
        // clock at 0: eff == u64::MAX is "before" 0 in wrapped order — late.
        assert_eq!(
            classify_msc_due(Some(u64::MAX), 0, false),
            MscDue::ExecuteNow
        );
        // A genuinely future target past the wrap still parks.
        assert_eq!(classify_msc_due(Some(1), u64::MAX, false), MscDue::Park);
    }
}
