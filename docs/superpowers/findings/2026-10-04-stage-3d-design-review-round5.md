# Stage 3d spec — design review round 5

**Result:** 0 blocking, 0 major, 0 minor; coverage INCOMPLETE (24/24). **Design converged.**

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`; model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.159.2`.

**Coordinator classification:** M-1..M-4 applied. The two unassessed integration questions (who services the
old incarnation's resource service and cleanup contexts after a successful recovery with unresolved GPU
obligations; publication vs a gate-holding mutation's revalidation when recovery changes topology) are
implementation wiring, resolved as explicit plan tasks with tests (user's convergence rule, 2026-09-28 /
2026-10-03).

## Verdict

**0 blocking, 0 major, 0 minor**

**Coverage: INCOMPLETE**

No new defect was demonstrated within this pass. Two integration questions remain unassessed below; this is not a clean review or a claim that implementation is approved, compiles, or passes tests.

## Incorporation audit

The prior review contains formal finding M-4 and carries forward M-1 through M-3.

| Prior finding | Status | Assessment |
| --- | --- | --- |
| M-4 — Shutdown lacks disposition for unresolved GPU/shared proofs after fd holders are released | **APPLIED** | [Plan lines 258–269](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:258) bound the whole teardown, prohibit unbounded waits, and retain unproven owners **and contexts** until process exit. [Lines 311–315](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:311) require the zero-surviving-holder GPU scenario. |
| M-3 — Shutdown predicates omit connector-probe threads | **APPLIED** | [Plan lines 220–235](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:220) count children and probe threads, transfer the worker ledger with its incarnation, and require asynchronous joining. Shutdown and recovery evidence includes blocked workers. |
| M-2 — Recovery parking lacks gate ownership and administrative handoff | **APPLIED** | [Plan lines 154–182](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:154) specify mutation-owned turns, the active entry’s `Q` deadline, administrative delivery before gate waiting, reply-before-attempt from `RecoveryFailed`, and terminal-path wakes. Changed-topology publication ordering remains a separate coverage limitation. |
| M-1 — Invisibility conflates topology with request outcomes | **APPLIED** | [Plan lines 44–61](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:44) scope invisibility to topology and name delayed mutations and terminalized Presents as exceptions, consistent with [umbrella lines 360–365](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:360). |

The device-scoped acquire correction is present at plan lines 110–115. “APPLIED” describes the design text, not verified implementation.

## Findings

### Blocking

None demonstrated.

### Major

None demonstrated.

### Minor

None demonstrated.

## Coverage and implementation checks

1. **Incorporation audit — complete.** Checked the formal prior finding and every carried-forward finding against the current text. The shutdown correction covers independent obligations, rather than merely extending the lease-holder predicate.

2. **Architecture and cross-task contracts — bounded coverage.** Checked the design against the authoritative coordinator/arbiter/driver boundaries: the effectful driver owns resource movement and acknowledges actions; supersession must progress on the core loop without waiting for host calls ([umbrella lines 101–129](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:101)). Source confirms acquire preparation reads desired outputs and current DPMS, discovers properties, and registers fresh pools. Inspected incarnation bundles, their handoff router, and the incarnation-bound resource service. This establishes relevant mechanisms, not complete production handoff.

3. **Safety, ownership and failure semantics — bounded coverage.** Checked barrier-before-reopen, same-incident stalled resume, supersession, DPMS restrictions, stale-result quarantine, logical withdrawal, and seat-before-wait shutdown against C.0 `REC-1..6` and §10. The family-closure registry independently checks detached submitters, helper reap, control closure, and remaining aliases ([source lines 579–633](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:579)). The revised shutdown rule respects the separate GPU/shared proofs required by [C.0 lines 1710–1716](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1710).

4. **Spec compliance and verification strategy — bounded coverage.** Checked the umbrella’s 3d exit and inherited handoff requirements ([lines 325–336](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:325)), protocol differential, exhaustive arbiter requirements, and A/B/C/F evidence. Proposed scenarios cover the prior corrections, recovery failure, stalls, supersession, and late results. Qualification remains subject to the nonempty expected-completion set and canonical fence evidence in [C.0 lines 1726–1743](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1726).

**Excerpts used: 24/24**, beyond one read each of the target and prior review. Investigation stopped at the limit.

**Unassessed questions for a focused follow-up:**

- After recovery reaches `Ready` while old GPU/read/FOREIGN obligations remain unresolved, which durable owner retains and services the old incarnation’s resource service, cleanup contexts, and completion ingress? Verify that cleanup remains executable without the replaced backend, as required by the incorporated [handoff contract, lines 244–272](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:244). The full Vulkan/GBM/dma-buf destruction graph was not assessed.
- When recovery changes topology while a mutation owns the gate turn, what orders core publication before that mutation’s revalidation? Ordinary publications pass through a gate-dependent drain ([source lines 2172–2187](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:2172)); this pass did not establish the complete terminal handoff.

Implementation must run the prescribed formatting, exact all-targets clippy gates and feature configurations, suites and mutations under CPU load, repeated helper suites, hardware evidence per real-path task, and applicable Linux glibc/musl/FreeBSD checks. No builds, tests, installs, benchmarks, or hardware experiments were performed.