# Stage 3d spec — design review round 2

**Result:** 0 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`; model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.159.2`.

**Coordinator verification:** M-2 verified in yserver-core/src/core_loop/run.rs (episodes block mutations and
forced reprobes; episode grant needs no request in flight; the forced turn is released after its reply). Design
(internal mechanics; resolved by the coordinator without a new user decision): recovery takes no gate
episode, parked mutations hold their own turn, administrative-reprobe delivery is recorded before the gate, a
forced reprobe from RecoveryFailed is answered first. Revision 3, section 3.3.

## Verdict

**0 blocking, 1 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result. It does not establish that implementation compiles, tests pass, or execution is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| M-1 — Invisibility conflates stable topology with unchanged request outcomes | **APPLIED** | [Design:39–56](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:39) scopes invisibility to topology and names delayed RANDR replies and terminalized Presents as exceptions. [Design:209–215](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:209) adds requests during recovery and failure against withdrawn state. This addresses the original contract defect; implementing parking introduces the separate integration question below. |

The prior review’s late-result coverage question was not a finding. The revision states the required disposition at design lines 130–138, but the complete resource-transfer path remains unassessed.

## Findings

### Blocking

None demonstrated.

### Major

**M-2 — Recovery parking lacks a RANDR gate ownership and administrative-reprobe handoff contract**

[Design:48–52](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:48) requires mutations to wait in the existing gate, while [design:118–127](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:118) allows `AdministrativeReprobe` to supersede recovery. The design does not define how the gate holds recovery, admits that higher-priority input, or releases its turn.

The existing gate makes this consequential: active or requested topology episodes block **both mutations and forced reprobes** ([run.rs:963–1000](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:963)). If recovery uses an episode to park mutations, an arriving `RRGetScreenResources` cannot reach the backend to become `AdministrativeReprobe`. A recovery paused by DPMS-off or awaiting reap can therefore keep the promised superseding input outside the arbiter. C.0 requires higher-priority events to supersede the current transition ([C.0:633–663](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:633)); the authoritative umbrella requires prompt driver execution on the core loop ([umbrella:117–130](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:117)).

The reverse direction also needs a decision. A forced reprobe already owns the gate. If its fresh recovery attempt waits for a separate acquire-style episode while its reply waits for recovery, neither can finish: episode grant requires no in-flight request ([run.rs:873–884](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:873)), and the forced turn is released after reply completion ([run.rs:1938–1964](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1938)).

**Smallest correction:** specify the gate owner and event-delivery contract for automatic and administrative recovery. Preserve administrative-event delivery while mutations are parked; either reuse the forced request’s existing turn or explicitly complete its reply before granting a recovery episode. Define release/wake behavior on failure, stall and supersession. Add core-loop scenarios covering a forced reprobe during paused recovery and a forced reprobe initiating recovery from `RecoveryFailed`.

### Minor

None.

## Coverage and implementation checks

1. **Incorporation:** Complete for the sole prior finding. The revised topology contract and named protocol exceptions satisfy [umbrella:360–365](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:360). Gate integration is a new concern.

2. **Architecture and cross-task contracts:** Checked coordinator/arbiter/driver responsibilities, `REC-4..6` precedence and incident fate, device-scoped acquire reuse, and actual core gate behavior. Acquire preparation currently selects `VTAcquire` explicitly ([admission.rs:4305–4343](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:4305)); extending the preparation under the current recovery transition belongs to implementation. M-2 concerns ownership and delivery, not method names.

3. **Safety, ownership and failure semantics:** Checked the specified barrier before reopen, same-incident stalled resume, supersession, DPMS restrictions, logical withdrawal and shutdown ordering against C.0 §6.4/§10. Source confirms that reap requires an actual wait status ([executor/mod.rs:1226–1251](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/executor/mod.rs:1226)), and family closure additionally requires detached submitters and discharged aliases ([drm_cleanup.rs:563–632](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:563)). Late-result fd adoption/closure is visible in [device.rs:2149–2194](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:2149).

   **Unassessed:** complete transfer of both resource sets from the old Owner/CommitConsumer into the winning quarantine, including owner replacement, and the complete Vulkan/GBM/dma-buf destruction graph. Those paths are not established sound by this review.

4. **Spec compliance and verification:** Checked the recovery requirements, acknowledged administrative amendment, client exceptions and proposed A/B/C/F evidence. Exhaustive arbiter checks and protocol-order differential evidence remain required by [umbrella:376–395](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:376) and [424–434](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:424). M-2 needs integration evidence beyond incident-count assertions.

**Reading budget:** **24/24 bounded excerpts**, beyond one read each of the target and prior review. Investigation stopped at the limit.

Implementation must perform formatting, `cargo clippy --all-targets -- -D warnings` in the required configurations, full required suites and mutations under CPU load, hardware cycles, and applicable Linux glibc/musl/FreeBSD checks. No builds, tests, installations or hardware runs were performed.