# Stage 3d spec — design review round 3

**Result:** 0 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`; model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.159.2`.

**Coordinator verification:** M-3 verified (the connector-probe worker is an in-process thread owning a
duplicated incarnation fd until joined, backend.rs ~2503 and ~26953). Design. Also folded in: the parked
mutation's `Q` deadline on the active gate entry (run.rs ~930), and device-scoped acquire reuse (the acquire
episode takes a gate episode). Revision 4, section 4.2a.

## Verdict

**0 blocking, 1 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result. It does not establish that code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| M-2 — Recovery parking lacks gate ownership and administrative-reprobe handoff | **APPLIED** | [Design:149–175](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:149) specifies no recovery episode, ownership by the parked mutation, administrative delivery before gate waiting, reply-before-recovery from `RecoveryFailed`, and wakes on every terminal path. [Design:253–259](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:253) assigns corresponding core-loop evidence. These resolve the design contract raised previously; implementation must establish the behavior. |
| M-1 — Invisibility conflates topology with request outcomes, carried forward as applied in round 2 | **APPLIED** | [Design:40–57](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:40) retains the scoped topology guarantee and names delayed RANDR replies and terminalized Presents as exceptions, consistent with [umbrella:360–365](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:360). |

The prior review’s incomplete resource-transfer assessment was a coverage limitation, not a resolved finding. It remains partly unassessed below.

## Findings

### Blocking

None demonstrated.

### Major

**M-3 — Shutdown’s completion and deadline predicates omit retained connector-probe threads**

The inventory explicitly includes stuck probe-worker fds ([design:187–199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:187)), but shutdown advances after collecting every **child’s wait/reap status**, and its deadline branch applies to a child still unreaped ([design:219–227](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:219)).

The production connector worker is an **in-process thread**. Its ledger retains a `JoinHandle`, and a timed-out worker remains there until it returns and can be joined ([backend.rs:2503–2553](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2503)). The thread receives a duplicated DRM fd and owns it while executing the probe ([backend.rs:26953–26982](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:26953)). Executor child reap cannot establish that this alias has closed.

Concrete sequence:

1. A connector probe remains blocked inside its host call, retaining the incarnation’s duplicated fd.
2. Shutdown releases the seat and successfully reaps every process child.
3. The specified child predicate now permits teardown, although complete fd-family closure remains impossible.
4. Respecting the barrier requires waiting for the thread, but the stated `ShutdownExecutorStalled` deadline branch has no unreaped child to represent this case. Proceeding instead would claim an unproven family barrier.

This leaves a material gap between the inventory and the shutdown supervisor. The authoritative umbrella requires transfer of retained owners and a total ownership matrix ([umbrella:330–335](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:330)); C.0 requires complete family closure and retention through unavailable teardown proof ([C.0:1708–1714](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1708)). The inherited handoff contract also forbids ordinary container destruction as fallback release ([resource-terminalization:262–272](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:262)).

**Smallest correction:** define a supervisor completion predicate covering both reaped children and returned/joined connector workers. Transfer the worker ledger and fd accounting with the incarnation; specify asynchronous return/join handling and the deadline disposition when a worker remains blocked after every child is reaped. Require a core-loop scenario with a retained probe thread, proving no premature family closure or resource destruction and the intended bounded-exit behavior.

### Minor

None.

## Coverage and implementation checks

1. **Incorporation audit:** Complete for the prior formal finding and its carried-forward invisibility correction. The revision’s gate ownership and administrative handoff claims are supported by its actual section 3.3.

2. **Architecture and cross-task contracts:** Checked the authoritative coordinator/arbiter/driver separation, recovery identity and precedence, gate episode exclusion, and acquire reuse. Existing acquire preparation is device scoped and derives installation from desired outputs and current DPMS state. Fresh master, executor, pools, registrations and rediscovery are explicitly assigned by the design. The existing acquire episode cannot be reused wholesale: it creates a gate episode, whereas recovery expressly forbids one.

3. **Safety, ownership and failure semantics:** Checked barrier-before-reopen, same-incident stalled resume, supersession, DPMS restrictions, stale-result disposition, withdrawal and shutdown ordering. Source confirms actual child wait status is required for reap proof. Resource-family closure additionally checks detached submitters, control closure and non-payload aliases; executor lease count alone does not prove it. M-3 identifies the missing thread-supervision contract.

   **Unassessed:** the complete transfer of both resource sets from Owner/CommitConsumer during owner replacement, and the complete Vulkan/GBM/dma-buf destruction graph. These are not established sound. An authorized follow-up should specifically trace whether those retained contexts remain sufficient to execute cleanup after the old owner/backend is replaced.

4. **Spec compliance and verification strategy:** Checked C.0 §6.4, `REC-1..6`, relevant §10 rows, the acknowledged administrative amendment, client exceptions and proposed A/B/C/F evidence. The umbrella’s exhaustive arbiter matrix and protocol-byte differential remain binding. The proposed shutdown hardware case exercises a delayed helper; it does not establish the connector-thread contract in M-3. The active parked mutation’s deadline handoff also remains unverified: the inspected core gate derives deadlines from waiting entries ([run.rs:930–940](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:930)).

**Excerpts used: 24/24**, beyond one read each of target and prior review. Investigation stopped at the limit.

Implementation must run `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings` in the required configurations, required suites and mutations under CPU load, hardware evidence per real-path task, and applicable Linux glibc/musl/FreeBSD checks. No builds, tests, installs or hardware experiments were performed.