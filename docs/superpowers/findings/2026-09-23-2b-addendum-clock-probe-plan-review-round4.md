> **Author verification (coordinator, 2026-09-23):** M-1 CONFIRMED —
> `ValidationResolved(Passed)` keeps the lease and `consume_validation` hands
> it to the live commit (`owner/device.rs` ~1820). Applied in rev 5 from
> C.0's rule (probe before admitting work on a new clock epoch): a passed
> validation proceeds only with every needed clock ready, else it is
> abandoned (a real release) and re-validated after the probe.

## Verdict

**0 blocking, 1 major, 0 minor.** Coverage: **COMPLETE FOR DECLARED SCOPE**. This is a design review, not a claim that the implementation compiles, passes tests, or is approved.

## Incorporation audit

| Round 3 finding | Status | Assessment |
| --- | --- | --- |
| B-1 — stale probe timeout | **APPLIED** | The plan limits stale discard to results and explicitly routes an uncertain outcome from a superseded probe through executor stall, reap, and logical completion loss. Its epoch-replacement test includes that timeout sequence ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:113)). |
| M-1 — promotion after validation | **PARTIAL** | The plan names validation resolution and adds a queued-successor test, but treats a *passed* validation as a slot release. The owner retains that lease until the validated live call consumes it or validation is abandoned ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:120), [owner](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1930)). The remaining contract gap is below. |

## Findings

### Blocking

None.

### Major

**M-1 — A passed validation is not a probe promotion point.** The plan requires a waiting probe to run at validation resolution before another commit or validation begins, including when validation passes ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:120)). In the current owner, `ValidationResolved(Passed)` retains the exclusive lease; the lifecycle driver immediately submits the validated topology, and `consume_validation` transfers that lease directly to the live commit ([owner](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:2447), [driver](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1391), [owner](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1820)).

If a new clock epoch is installed while validation holds the slot, the waiting probe cannot acquire it merely because the validation reply passed. The current validated call may take the lease first. The named test checks priority before a *successor’s validation*, but can pass after the current validated call has already begun ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:189)). Specify whether waiting-probe priority requires abandoning and requeuing a passed validation for fresh validation after the probe. If an exception is intended, narrow I-3 to actual slot releases and state how the direct consume path preserves C.0’s current-clock admission rule ([spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1769)). Test the passed-validation handoff itself.

### Minor

None.

## Coverage and implementation checks

All four checks were performed. The incorporation audit assessed both round 3 findings; architecture and ownership were checked against the owner slot, probe resolution, validation handoff, and event batch; failure semantics were compared with C.0 §10 and stage 3a §3.7; and the named tests and implementation gate were assessed. **24/24 logical spec/source excerpts** were used. One command printed 131 contiguous source lines, exceeding the requested 120-line per-excerpt cap; that output is counted as two excerpts. Investigation stopped at the budget.

The exact production setup site, complete installer inventory, and other slot-release paths remain **unassessed**, not deemed sound. The plan assigns formatting, Clippy, build, portability, and test gates to implementation. No build, test, benchmark, or review script was run.