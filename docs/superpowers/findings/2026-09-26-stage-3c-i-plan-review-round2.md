# Stage 3c-i plan — codex review, round 2

**Target:** revision 2 (`1c3b7f3d`), second review (prior: round 1).

**Result:** 2 blocking, 3 major, 0 minor; coverage INCOMPLETE (24/24). Round 1: B-1, M-2 applied; B-2, M-1 partial.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-26):** all five CONFIRMED.

- **B-1** — rev 2 Task 1 left today's release sequence (direct all-off, then drop master) running for Owner devices until Task 3. Rev 3: the whole release is Task 2.
- **B-2** — only the release commit's own unknown outcome was specified; a client modeset occupying the slot at the deadline was not. Rev 3: any non-terminal commit in the slot closes the device; the client token resolves `Failed`, its gate publication is withdrawn; test V30 through `on_vt_release`.
- **M-1** — `core_loop/run.rs` receives requester-less publications only when drained; no episode start/end. Rev 3: `EpisodeBegin`/`EpisodeEnd` in the core (Task 1), used by `AcquireEpisode` (Task 4), with an intervening-mutation test.
- **M-2** — Task 4 needed Task 6's reinstall; Task 5 started acquire transitions before their physical work existed. Rev 3: the whole acquire is Task 4.
- **M-3** — the hardware test was written in the last task but scheduled earlier. Rev 3: written in Task 2 (release), extended in Task 4 (acquire), runs after 2, 4, 6.

## Review as received

## Verdict

**2 blocking, 3 major, 0 minor**  
**Coverage: INCOMPLETE**

This is a design review. It does not establish that the plan compiles, passes tests, or is approved for execution.

## Incorporation audit

| Round-one finding | Status | Revision 2 |
| --- | --- | --- |
| B-1 — combined probe loses per-device outcomes | **APPLIED** | Task 5 specifies a result keyed by device and continues probing after an error ([plan:194](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:194)). |
| B-2 — unsafe task boundaries on release | **PARTIAL** | Task 3 combines hand-off, closure, and withdrawal, and Task 1 scopes Legacy suspend. Task 1 still exposes an unsafe Owner release path; the deadline case with a client commit occupying the slot remains unspecified (B-1, B-2). |
| M-1 — no acquire episode owner | **PARTIAL** | Task 6 names a backend `AcquireEpisode` and terminal barrier, but does not define how it acquires and holds the core gate turn (M-1). |
| M-2 — gate tests lack a gate-bearing path | **APPLIED** | Task 2 adds core-loop gate tests with retained state and a separate backend emitter test ([plan:119](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:119)). |

## Findings

### Blocking

**B-1 — Task 1 exposes an Owner release before its commit path exists.**  
Task 1 adds prompt obligations but leaves the rest of today’s release sequence running until Task 3 ([plan:82](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:82)). Today that sequence makes a direct all-off KMS call, then drops master and acknowledges release ([backend.rs:14858](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:14858), [backend.rs:25386](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:25386)). With a client modeset dispatched but unanswered, the direct call can overlap the Owner executor; scoping that call away from Owner instead leaves no Owner release commit before hand-off. The spec requires the commit to wait in the device slot and hand-off to wait for terminal completion or the deadline ([spec:124](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:124), [spec:148](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:148)). Make Task 1 a safe preparatory change with no active Owner release route, or deliver that route together with Task 3.

**B-2 — The deadline outcome does not cover a client commit ahead of release.**  
The plan queues release behind a dispatched client modeset, but its unknown-outcome test holds the *release commit* in flight ([plan:134](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:134), [plan:170](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:170)). If the client commit never answers, the release commit is still queued when the 1 s hand-off occurs. The plan does not say how that client token and its pending core gate publication become unknown, receive `Failed`, and become unable to publish a late result. The core currently publishes a ready client result through that gate ([run.rs:1627](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1627)); the spec requires dead-epoch results to change nothing and a dispatched commit on a withdrawn device to fail ([spec:156](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:156), [spec:333](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:333)). Specify closure for **any occupied device slot at the deadline**, including its requester and core gate token, and test that sequence through the production VT entry.

### Major

**M-1 — `AcquireEpisode` has no contract with the core gate.**  
Task 6 says a backend record holds one requester-less gate turn from first reinstall dispatch through publication ([plan:230](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:230)). The existing core gate receives requester-less *publications* only when drained; it has no episode start or terminal signal ([run.rs:817](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:817), [run.rs:897](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:897)). Thus a client mutation can take a turn while A’s reinstall is in flight and before B is terminal, contrary to the required episode turn ([spec:305](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:305)). Define the episode identity, core reservation and release signals, and abort behavior; test an intervening client mutation.

**M-2 — Tasks 4 and 5 cannot each satisfy their stated gate.**  
Task 4 requires a test in which acquire reinstalls `ACTIVE=0`, but the reinstall is delivered in Task 6 ([plan:187](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:187), [plan:217](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:217)). Task 5 then starts healthy Owner acquire transitions before Task 6 supplies their physical work ([plan:201](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:201)). An acquire at that boundary can reach `Active` with Owner admission closed and no reinstall path. Move the dependent test and route to the task that delivers reinstall, or provide a safe, testable interim acquire path.

**M-3 — The scheduled hardware checks precede their test.**  
The plan says the implementer writes the card1 test in Task 8, while the coordinator runs it after Tasks 3, 6, and 8 ([plan:261](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:261)). It cannot provide evidence after Tasks 3 or 6 as scheduled. The standing rule calls for hardware after each task touching a real path ([spec:467](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:467)). Put an executable check before its first scheduled run and state which intermediate behavior each run verifies.

### Minor

None.

## Coverage and implementation checks

The audit compared all four prior findings with revision 2. Architecture checks covered the VT entry, Legacy suspend/resume, coordinator dispatch, lifecycle action match, core gate, requester-less publication, combined probe, and existing test driver. Safety checks focused on occupied slots, deadline closure, publication ordering, and intermediate tasks. Spec checks covered §3, the relevant §4.1 publication rules, §5 exceptions, and §6 evidence.

**Reading:** 24/24 spec and source excerpts used. Two source excerpts inadvertently exceeded the 120-line limit; no further investigation was performed. Exact fd-alias retirement, console seam behavior, and the implementation’s late-token invalidation remain unassessed, not sound. The focused unresolved question is how an in-flight client CRTC token is retired across an unknown VT release. Formatting, clippy, tests, portability, and hardware results belong to implementation.