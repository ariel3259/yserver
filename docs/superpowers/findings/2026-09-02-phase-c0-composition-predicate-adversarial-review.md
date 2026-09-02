# Phase C.0 — adversarial review: composition predicate, slow-path detection, and evidence reachability

**Date:** 2026-09-02
**Subject:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
(Draft of 2026-09-02, driver-eligibility review incorporated)
**Scope:** the `CursorCompositionKey` / `CoordinateCompositionPredicate`
machinery added to sections 5, 6.1, 7.1 and 9.4, the stock-driver cohort policy
in `CAP-4`, and their interaction with the section 16.3 gates.

This finding supersedes the per-key slow-return suspension and one-shot
requalification disposition recorded in the earlier 2026-09-02
driver-coordinate-eligibility review.

All kernel claims below were verified by the reviewer against the pinned tree
`torvalds/linux@77cb8f24c2381a8abb7272d7bbdec548d6426a8a` and against
`NVIDIA/open-gpu-kernel-modules` main. The disposition below was independently
checked against the pinned local Linux tree before incorporation.

## Settled correctly

The stock-driver-only cohort policy in `CAP-4` and section 7.1 is the right
call and is stated without hedging. The helper call-chain correction is
accurate: `drm_atomic_helper_check()` stores internal async-check rejection as
`async_update = false` while returning the earlier plane-check result, so it
does not expose that rejection as a userspace errno. Section 9.4 and tests
16.2 #14/#27 match that behavior.

The Navi 21 predicate conditions in section 7.1 also match
`dm_crtc_get_cursor_mode()`: YUV/video below-cursor format, active plane color
pipeline, scale mismatch, and absence of full-CRTC coverage. The DCN 4.0.1 /
4.2.0 exemption does not apply to Navi 21.

## Blocking findings

### B-1. Slow-return detection costs up to a full frame and conflicts with the soak

When the async cursor hook is rejected, the legacy cursor ioctl reaches
`drm_atomic_helper_commit(dev, state, nonblock = false)`. Its blocking path
runs `drm_atomic_helper_wait_for_dependencies()` before the commit tail and
waits for old CRTC, connector and plane commits. With a same-CRTC primary
commit in flight, the detection call can therefore block until that commit
reaches `hw_done`, on the same scale as the recorded 11.5 ms mean / 16.3 ms
maximum NVIDIA legacy-HW regression.

Section 16.3 permits zero in-process host calls above their section 4.1 class
maximum. A slow-return detection event would itself invalidate
`InProcessQualified`, making the production state machine and release budget
inconsistent.

### B-2. AMD cursor mode is hysteretic

`dm_crtc_get_cursor_mode()` initializes the candidate mode from the current
driver mode. It recomputes only when plane enablement, framebuffer format,
scale ratio, z-order, or color-pipeline state changes. Full-CRTC coverage is a
decision input but is not independently a recomputation trigger. Destination
movement or resize that changes coverage while preserving scale ratio can
therefore leave the driver's prior mode installed.

A pure predicate over current installed shape cannot exactly reproduce this
history. Coverage loss may conservatively reject while the driver remains
native; later coverage restoration may look eligible while the driver remains
overlay. The predicate must account for both recomputation triggers and
decision inputs and must be described as conservative rather than exact.

## Major finding

### M-1. The old production suspension path had no hardware evidence

Production deliberately issues zero coordinate ioctls for predicate-ineligible
keys, while the raw-KMS slow-path call is nonmergeable mechanism
characterization. Consequently the old per-key slow-return suspension path
would not execute on hardware during the merge campaign and had only unit-test
evidence.

## Minor findings

### m-1. `EBUSY` during requalification was undefined

An eligible concurrent `EBUSY` is expected beside an accepted cursor-disjoint
primary. It must preserve established qualification and carry the newest point
to the single deferred post-completion retry rather than disabling the fast
path.

### m-2. NVIDIA's expected result should be explicit

Stock NVIDIA cannot select `OwnerMediatedLegacyMove`; its only C.0 hardware
coordinate path is vblank-paced `SynchronousAtomicMove`, which occupies the
sole atomic device slot. Meeting both the FPS and cursor-maintenance-admission
gates therefore depends on tier-3 absorption, constraining hardware cursor
updates to client cadence. The expected result is that low-frame-rate drag
fails the software-baseline comparison, leaving software cursor selected and
`atomic_kms_cohort_validated=false`. Confirming that prediction is a valid
cohort-local result.

## Approved disposition

1. Delete per-key slow-return suspension and one-shot composition
   requalification.
2. Keep `CursorCompositionKey` as installed-shape identity, but make
   composition eligibility a history-bearing conservative latch. The cohort
   predicate consumes the previous-to-current key transition, its audited
   driver recomputation triggers, and all cursor-mode decision inputs.
3. Any incompatible current decision input makes the latch ineligible. When
   all decision inputs permit native mode, an audited recomputation-trigger
   transition may establish eligibility; without one, the prior latch is
   retained. Coverage-only restoration therefore cannot rehabilitate a latch
   rejected by coverage loss.
4. Any otherwise valid coordinate call returning above the latency-critical
   one-millisecond maximum is a cohort/predicate defect. Close
   `OwnerMediatedLegacyMove` for the complete plane incarnation, emit key and
   transition telemetry, and fall back. It has no special soak allowance and
   invalidates `InProcessQualified` if observed there.
5. Eligible concurrent `EBUSY` changes neither the eligibility latch nor
   transport qualification; it coalesces the newest point into the existing
   single deferred post-completion retry.
6. State explicitly that retaining software cursor with cohort validation
   false is the expected stock-NVIDIA result; the hardware arm attempts to
   refute that prediction.
