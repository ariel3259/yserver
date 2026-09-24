> **Author verification (coordinator, 2026-09-23):** B-1 PARTIAL CONFIRMED as a
> plan gap, not a code gap: the executor already terminates and reaps any
> timed-out host call generically (`terminalize_unknown` → `Stalled` +
> `request_termination`; `tick` → `Reaped` + `ReapProof`, `kms/executor/mod.rs`),
> probes included. Rev 3 names that barrier and its owner, separates it from
> the logical `Poisoned`, and makes the timeout test assert it.

## Verdict

**1 blocking, 0 major, 0 minor.** Coverage: **COMPLETE FOR DECLARED SCOPE**. This is a design review, not a claim that the implementation compiles, passes tests, or is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| B-1 — uncertain probe needs an `ExecutorStalled` handoff | **PARTIAL** | The plan separates `Unknown` from explicit rejection and keeps the slot, but its specified production route establishes `Poisoned` without assigning the executor lease and reap handoff. |
| M-1 — clock absent on first activation | **APPLIED** | I-2 requires a record for every served Owner CRTC before lifecycle dispatch, including without a RANDR query, and forbids a partial clock map ([plan, lines 60–74](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:60>)). |
| M-2 — unscheduled probe can lose the slot | **APPLIED** | I-3 retains a waiting key and gives it priority when the slot frees, including after an epoch replacement; the occupied-slot test exercises that contract ([plan, lines 76–87](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:76>)). |

## Findings

### Blocking

**B-1 — Logical poisoning does not establish the timed-out probe’s lease barrier.** The plan claims the prior finding is applied and sends `ClockProbeResolved::Unknown` through `lifecycle_report_completion_loss → Table U → Poisoned` ([plan, lines 5–12](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:5>), [50–58](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:50>)). That route reports `CompletionUnknown` to the lifecycle coordinator ([coordinator.rs, lines 393–437](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/lifecycle/coordinator.rs:393>)); the 3a rule describes logical `Poisoned` behavior ([3a spec, lines 329–334](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:329>)). Neither stated handoff identifies who requests executor termination, retains its alias and lease, or waits for reap proof. The current probe resolver marks the clock failed and emits `Unknown` while retaining the slot ([device.rs, lines 2254–2261](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:2254>)).

If a probe times out while its helper remains in the ioctl, the proposed test can observe `Poisoned` and a held slot yet leave the helper’s fd lifetime unaccounted for. C.0 requires timeout, IPC loss, and executor failure to follow `COMMIT-5`/`ExecutorStalled`, including host-call watchdog and reap rules ([C.0 spec, lines 643–654](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:643>), [1769–1784](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1769>)). Specify the executor-state and lease owner for this probe outcome, the termination/reap barrier, and its relationship to logical `Poisoned`; make the timeout test establish that barrier, not only the DPMS disposition ([plan, lines 149–150](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:149>)).

### Major

None.

### Minor

None.

## Coverage and implementation checks

All four checks were performed: prior findings were audited; clock installation, slot scheduling, lifecycle submission, and event routing were checked against targeted code; uncertain-outcome ownership was checked against C.0 and 3a; and the named tests and implementation gate were assessed. **24/24 bounded excerpts** were used, including the required owner-clock finding. No build, test, or review script was run.

The exact production setup site, every installer, and the full executor teardown path were not verified within this reading limit; they are **unassessed**, not deemed sound. The specific unresolved design question is who owns a timed-out probe’s executor lease through termination and reap. The plan assigns formatting, Clippy, tests, and build checks to implementation.