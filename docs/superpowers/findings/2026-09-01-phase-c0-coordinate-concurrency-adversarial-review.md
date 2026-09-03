# Phase C.0 coordinate-concurrency adversarial review

**Date:** 2026-09-01
**Reviewer:** Opus
**Reviewed spec:** the 2026-08-31 kernel-path-revised Draft of
`2026-08-26-phase-c0-atomic-kms-migration-design.md`
**Disposition:** Incorporated after maintainer/user approval

## Verification basis

The review was checked against merged Phase A+B and local Linux source at
`77cb8f24c2381a8abb7272d7bbdec548d6426a8a`, especially
`drm_atomic_helper_setup_commit()`, `drm_atomic_helper_async_check()`, the legacy
cursor-to-atomic helper path, and nonblocking `drm_atomic_helper_commit()`.

The kernel records a `drm_crtc_commit` on every plane explicitly present in an
atomic request. The legacy cursor async check rejects movement with `-EBUSY`
while that plane's prior `hw_done` remains incomplete. Therefore restating an
unchanged cursor plane in every primary request defeats the coordinate fast
transport even though the cursor content did not change.

## Accepted corrections

1. Primary commits omit an unchanged cursor plane. Only changed persistent
   image, visibility, binding, hotspot or other non-coordinate generations may
   be absorbed; coordinate-only intent is never absorbed.
2. `CoordinateSubmitting` is a per-cursor-plane mutation reservation rather
   than the atomic device slot. It may overlap exactly one accepted primary
   commit whose recorded closure omitted the cursor and contains no modeset,
   connector, topology, unflip, cursor or lifecycle change. The unresolved
   coordinate host call still excludes every new KMS host-call dispatch.
3. Eligible concurrent coordinate `EBUSY` coalesces the newest point and permits
   one retry after primary completion. A second consecutive result or an
   impossible-context `EBUSY` closes the fast transport without a retry loop.
4. The hardware spike compares cursor-omitting and unchanged-cursor-absorbing
   primary requests under continuous composed and direct traffic, records
   `EBUSY` ratio/runs, and applies the legacy no-regression gate there rather
   than on an idle desktop.
5. Section 9.2.1 has an explicit combined-primary admission tier for a
   qualified equal-refresh `HomogeneousCompletionGroup`. Mixed-refresh or
   unknown-period topology cannot satisfy the single-slot throughput contract;
   it retains the merged Phase A+B backend path and advertises C.0 multi-CRTC
   false.
6. Executor selection remains one global binary choice. Measurement limits are
   split by latency-critical, interactive-maintenance and interactive-lifecycle
   call class. The provisional in-process probe selects the architecture before
   the eight-hour soak, which then validates the chosen design.
7. `FlipDrivenSoftware` has one client-visible protocol MSC. Timer wakes and
   real page events advance that domain; a real event publishes the maximum of
   `protocol_msc + 1` and the elapsed-period candidate, replaces the anchor, and
   remains the only source of KMS presentation/completion evidence.

## Deliberate scope

C.0 does not add concurrent atomic commits for disjoint DRM object sets. That
would invalidate the single atomic device-slot invariant and requires a
successor design. It also does not introduce per-class executor architectures:
every seat-active class is measured against its own bound, but failure of any
row selects the one global executor architecture.
