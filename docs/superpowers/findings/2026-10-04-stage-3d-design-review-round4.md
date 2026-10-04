# Stage 3d spec — design review round 4

**Result:** 0 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`; model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.159.2`.

**Coordinator verification:** M-4 verified (live `VkContext` destruction waits `device_wait_idle`,
kms/vk/device.rs ~90-126 and ~838-852, unbounded). Design. Rounds 3 and 4 found successive cases in the same
area (shutdown teardown), so revision 5 states the rule generally: the deadline bounds the whole teardown, every
unproven obligation is retained rather than destroyed, and no unbounded wait runs during shutdown.

## Verdict

**0 blocking, 1 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result. It does not establish that code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| M-3 — Shutdown predicates omit retained connector-probe threads | **APPLIED** | [Plan lines 218–255](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:218) count children and probe threads, transfer the worker ledger with the incarnation, require asynchronous join, and define deadline disposition for either holder. [Lines 293–299](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:293) require blocked-worker shutdown and recovery scenarios. |
| M-2 — Recovery parking lacks gate ownership and administrative-reprobe handoff, carried forward by round 3 | **APPLIED** | [Plan lines 152–180](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:152) specify mutation-owned turns, administrative delivery before gate waiting, reply-before-recovery from `RecoveryFailed`, and terminal-path wakes. The active entry’s `Q` deadline is now explicitly assigned. |
| M-1 — Invisibility conflates topology with request outcomes, carried forward by round 3 | **APPLIED** | [Plan lines 42–59](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:42) scope invisibility to topology and name delayed RANDR replies and terminalized Presents as exceptions, as permitted by [umbrella lines 360–365](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:360). |

The additional device-scoped acquire correction is present at plan lines 108–113. The prior review’s incomplete resource-transfer assessment remains a coverage limitation, not a finding claimed fixed.

## Findings

### Blocking

None demonstrated.

### Major

**M-4 — Shutdown lacks a deadline disposition for unavailable GPU/shared-resource proof after fd holders are released**

[Plan lines 243–253](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:243) define two supervisor outcomes: every fd-family holder is released, permitting subsequent teardown; or a holder remains at the deadline, requiring retention and process exit. Neither outcome specifies what happens when all those holders are released but an independent GPU/read/FOREIGN obligation remains unresolved.

Those obligations are explicitly independent of family closure: [C.0 lines 1710–1714](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1710). The handoff contract incorporated by [umbrella lines 330–334](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:330) requires separate GPU/shared proofs and retention through the deadline when proof never arrives, forbidding ordinary container destruction as fallback: [terminalization lines 262–272](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:262).

Concrete sequence:

1. Off-screen rendering leaves a GPU submission pending.
2. Shutdown releases the seat, reaps every child, joins every probe worker, and closes the DRM family.
3. The GPU submission still cannot be proven finished.
4. Respecting its lifetime requires retaining the resource/context, but the deadline branch has no surviving fd holder to represent this case. Entering ordinary teardown can instead block indefinitely: live `VkContext` destruction calls `device_wait_idle`, and live contexts always retain that wait ([device.rs lines 90–126](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/vk/device.rs:90), [838–852](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/vk/device.rs:838)).

“Device-loss safe order” does not define this unavailable-proof disposition. The proposed shutdown tests likewise exercise surviving children/workers only.

**Smallest correction:** specify how the supervisor retains unresolved GPU/read/FOREIGN/shared owners after family closure, keeps its deadline operational, and exits without running unproven or unbounded cleanup. Add a core-loop scenario with zero surviving fd-family holders and an unresolved GPU obligation, checking retained ownership and bounded exit.

### Minor

None.

## Coverage and implementation checks

1. **Incorporation audit:** Complete for the prior formal finding and carried-forward findings. Thread supervision, active-gate deadline ownership, and device-scoped acquire reuse are stated in the actual design text.

2. **Architecture and cross-task contracts:** Checked coordinator/arbiter/driver responsibilities, current-transition validation, recovery gate exclusion, and administrative handoff against the umbrella. Source confirms acquire preparation is device scoped and reads current desired outputs and DPMS state. The design assigns fresh master, executor, pools, registrations, and rediscovery before reinstall.

3. **Safety, ownership and failure semantics:** Checked barrier-before-reopen, same-incident stalled resume, supersession, DPMS restrictions, stale-result disposition, withdrawal, and seat-before-wait ordering. Inspected the retained incarnation bundle and handoff router, including late-event/descriptor delivery, and the registry’s family-closure preconditions. M-4 identifies an unresolved shutdown contract.

   **Unassessed:** complete production transfer of Owner/CommitConsumer resources and cleanup contexts during replacement; the full Vulkan/GBM/dma-buf destruction graph. These are not established sound. A focused follow-up should determine whether every retained cleanup obligation remains executable after replacement and which independent proofs can remain unavailable after fd-family closure.

4. **Spec compliance and verification strategy:** Checked relevant umbrella architecture, 3d exit, client differential, and evidence requirements; C.0 §6.4, `REC-1..6`, and relevant §10 recovery/shutdown rules; and 3c acquire reuse. A/B/C/F requirements and the proposed gate/failure scenarios are present. The exhaustive arbiter matrix and protocol-byte differential remain binding. No execution evidence was collected.

**Excerpts used: 24/24**, beyond one read each of the target and prior review. Investigation stopped at the limit.

Implementation must run `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings` in the required configurations, required suites and mutations under CPU load, hardware evidence per real-path task, and applicable Linux glibc/musl/FreeBSD checks. No builds, tests, installs, or hardware experiments were performed.