# Stage 3b design — codex review, round 3

**Target:** `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`
revision 3 (`514bc0a5`), prior review round 2.

**Result:** 1 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24; copied-route
source/sink plumbing and the disabled pool's full release path unassessed).
Trend: r1 2B 3M, r2 2B, r3 1B 1M.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):**

- **B-1 — CONFIRMED, bounded rather than removed.** A Legacy mutation executes
  synchronously on the core loop; no timer can run during it. That is Legacy
  behaviour C.0 keeps for Legacy devices. Revision 4 services queued deadlines
  first after it returns and adds `L` (the synchronous Legacy time ahead) to
  the mixed-server bound; `L = 0` without a Legacy device.
- **M-1 — CONFIRMED as a gap in the text; the mechanism exists.** 2c already
  registers `KmsRelease` for each allocation a displacing owner commit
  displaces (`register_kms`, `resources/mod.rs:998`) and discharges it on that
  commit's `CompletionRetired` (test at `backend.rs:54313`). A disable displaces
  the old framebuffer (plane `FB_ID` = 0) exactly as a flip does. Revision 4
  names this, keeps the GPU/FOREIGN proofs separate, and states that COMMIT-3's
  device-level teardown barrier is not what releases a buffer.

## Verdict

**1 blocking, 1 major, 0 minor.** Coverage: **INCOMPLETE**. This is a design review; it does not establish that code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| Round 2 B-1 — scene resource handoff | **APPLIED** | Revision 3 retains the replaced scene state and drains resources on their own proofs, including the deferred queues serviced by the named drain site ([3b §5.1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:340)). |
| Round 2 B-2 — validation at gate expiry | **APPLIED** | Expiry now performs only stateless checks before returning `GateExpired` ([3b §7.3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:530)). |
| Round 1 B-1 — atomic `EBUSY` retry | **APPLIED** | The real-commit rejection closes readiness and forbids retry ([3b §6](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:413)). |
| Round 1 B-2 — whole-request bound | **PARTIAL** | Probe, unflip, commit and queue deadlines are stated; a synchronous Legacy predecessor can still prevent a queued request’s deadline from being serviced (B-1 below). |
| Round 1 M-1 — position-only request | **APPLIED** | It is an ordered logical transaction without a KMS commit ([3b §3.3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:189)). |
| Round 1 M-2 — fallible promotion | **APPLIED** | Scene construction precedes acceptance; promotion moves staged state and retains the replaced state ([3b §§4.1–4.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:277)). |
| Round 1 M-3 — requester-less publication | **APPLIED** | The event reaches the arbiter immediately; its publication passes through the gate ([3b §7.5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:564)). |

## Findings

### Blocking

**B-1 — A synchronous Legacy mutation can prevent gate expiry.** The gate queues mutations across Legacy and Owner devices and promises a timeout wake plus a `Q + E` answer bound for parked requests ([3b §§7.1, 7.3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:449), [umbrella obligation 5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:298)). Consider Owner request A in flight while Legacy request B and Owner request C enter the gate FIFO. When A completes, B is admitted. Legacy `begin_crtc_config` can call `apply_crtc_config` synchronously, including its quiesce, on the core loop ([backend](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:24424), [core dispatch](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:875)). If B blocks past C’s `Q`, the core cannot run C’s timeout wake. C remains parked beyond the stated bound. Specify how the gate services queued deadlines while a synchronous Legacy mutation runs, and test this three-request sequence with B stalled.

### Major

**M-1 — Disable lacks a named proof for retiring its old scanout pool.** Promotion retains the old pool until “completion evidence” proves the kernel no longer scans it, while scene retirement ties composed generations to that pool’s KMS retirement ([3b §§4.2, 5.1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:305)). For a disable, an out-fence can establish `HardwareComplete`, but C.0 says that milestone does **not** prove full disable or teardown; `PriorBufferReleased` is separate ([C.0 §6.3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:627), [§10.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2126)). After a successful disable fence, the design leaves the release producer and retention handoff unspecified: treating that fence as proof releases too early; never obtaining further proof retains the pool indefinitely. Name the resource-appropriate barrier or later teardown owner for a disabled output, and test that its old pool survives `HardwareComplete` until that proof.

### Minor

None.

## Coverage and implementation checks

**24/24 bounded spec/source excerpts used; no builds or tests run.** Incorporation covered both round 2 findings and its five round 1 carry-forwards. Architecture checked the gate against core dispatch and the lifecycle result boundary. Safety checked scene queues, commit milestones, clocks and pool retirement. Compliance checked the six umbrella RANDR obligations and the proposed evidence.

The excerpt limit left copied-route source/sink resource plumbing and the full release path after a disabled output’s pool handoff unassessed; neither is judged sound. The focused follow-up question is which owner supplies and records the disabled pool’s teardown proof. Rust compilation, real tests, formatting, CI clippy and applicable portability checks belong to implementation.