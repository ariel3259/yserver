# Stage 3b design — codex review, round 2

**Target:** `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`
revision 2 (`c39f0605`), prior review round 1.

**Result:** 2 blocking, 0 major, 0 minor; coverage INCOMPLETE (24/24; the
latch key, RANDR opcode coverage and mixed Legacy synchronous deadlines
unassessed). Trend: r1 2B 3M, r2 2B.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):**

- **B-1 — CONFIRMED.** `OutputSceneState.pending_pool_releases`
  (`scene.rs:508`) holds descriptor slots and managed allocation keys behind
  fence tickets, drained by `drain_deferred_scene_resources` (`:394`); a fresh
  state starts empty (`:1623`). Revision 3 rebuilds only the target output
  and moves its replaced state to a scene retirement list freed on each
  resource's own proof.
- **B-2 — CONFIRMED.** Revision 3 runs only stateless checks on `Q` expiry and
  answers `Failed`; the named exception now states that Legacy might have
  answered anything.
- **Unassessed latch key** — made explicit in revision 3:
  `(installed topology generation, requested configuration)`.
- **Opcode coverage** is left to the plan, which enumerates it from
  `handle_randr_request`; **mixed Legacy synchronous deadlines**: a Legacy
  mutation runs synchronously once admitted, so it adds no parked stage.

## Verdict

**2 blocking, 0 major, 0 minor.** Coverage: **INCOMPLETE**. This is a design review, not a claim that the design compiles, tests pass, or is approved for implementation.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| B-1 — atomic `EBUSY` retry | **APPLIED** | [§6](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:390) now closes readiness, records the evidence, and forbids retry, matching [C.0 §9.4](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1657). |
| B-2 — whole-request bound | **TRADED** | The design adds an unflip outcome and gate deadline, but expiry can validate against state an earlier mutation has yet to publish (B-2 below). |
| M-1 — position-only request | **APPLIED** | [§3.3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:188) defines an ordered logical transaction with no KMS commit. |
| M-2 — fallible scene promotion | **TRADED** | Scene construction moves before acceptance, but the proposed swap lacks a lifetime handoff for the old scene’s live resources (B-1 below). |
| M-3 — requester-less event ordering | **APPLIED** | [§3.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:144) and [§7.5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:537) let `REC-4` reach the arbiter before gate-ordered publication. |

The prior review’s *unassessed* dark-CRTC question is addressed in [§3.5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:203). Its inactive-CRTC premise is supported by the [kernel `GET_SEQUENCE` path](/home/ariel_santangelo/Projects/linux/drivers/gpu/drm/drm_vblank.c:2033) and [C.0’s event-bearing clock rule](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1769); the required Owner change remains implementation work.

## Findings

### Blocking

**B-1 — Scene promotion has no handoff for live GPU resources.** The design builds fresh scene states and [swaps them in at promotion](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:276), while rebuilding [every output on the device](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:329). An existing `OutputSceneState` owns `pending_pool_releases`, whose fence ticket deliberately keeps a descriptor-pool slot alive **after page-flip retirement** until GPU work finishes ([scene.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:485)); a freshly built state has empty queues ([scene.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:1599)). A composed flip can retire while its GPU fence is unsignaled, then a modeset can promote and discard that old state. The plan’s old *scanout pool* retirement rule does not retain this scene-owned descriptor pool. That permits release before the independent resource proof required by [C.0’s typed milestones and teardown rule](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:627). Specify who retains or migrates each old scene-owned queue and ring until its own fence or retirement proof, and test promotion with a pending deferred release.

**B-2 — Gate expiry validates against an unpublished predecessor.** The gate promises serial validation against [the previous mutation’s published state](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:445), yet at `Q` it [runs normal validation immediately](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:507) while that predecessor may still install. Suppose A expands the root through an Owner modeset and B queues a position valid in the expanded root but outside the currently published root. If A remains in flight past `Q`, B receives a validation error; serial Legacy would validate after A and could succeed. This exceeds the named `GateExpired` exception and contradicts the [umbrella’s Legacy byte-parity obligation](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:271). Make expiry independent of mutable validation and explicitly define its wire result as a named exception, or defer validation until predecessor publication with a bound that can still be met. Test a predecessor that changes B’s validation outcome.

### Major

None.

### Minor

None.

## Coverage and implementation checks

**24/24 bounded spec/source excerpts used; no builds or tests run.** Incorporation: all five prior findings checked. Architecture: checked the gate, result boundary, scene swap, and sampled core completion path. Safety: checked clock bootstrap against C.0, the kernel, and Owner readiness; checked scene resource lifetime and expiry ordering. Compliance: checked the six umbrella RANDR obligations and the proposed evidence for the two findings.

The excerpt limit left the candidate-versus-installed topology-generation latch contract, complete RANDR opcode coverage, and mixed Legacy synchronous deadline behavior unassessed. Those areas are **not** judged sound. Real compilation, tests, formatting, CI clippy, and applicable ioctl portability checks belong to implementation.