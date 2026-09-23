# Stage 3 umbrella design — codex review, round 2

**Target:** `docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md`
revision 2 (`be13d34d`), against the C.0 specification (with its 2026-09-22
§18 amendment), prior review round 1.

**Result:** 1 blocking, 2 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE
(24/24 excerpts). Round 1: 2B 5M INCOMPLETE. Incorporation: 5 APPLIED, B-1
PARTIAL, B-2 TRADED (its new async path exposed round 2's M-1 and M-2).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-22):**

- **B-1 — CONFIRMED.** C.0 §10 makes an explicit ioctl rejection
  `FailedBeforeSubmit` with the previous state authoritative (C.0 lines
  1961–1968), and only completion-mechanism breaches poison the incarnation
  (lines 1996–1998); revision 2 grouped them. Separately, the owner-event route
  for `MechanismFailed` calls `request_exit()` (`backend.rs:21183`), so the
  claimed wait in `Poisoned` was unreachable. Fixed in 3a: two distinct
  outcomes, and on an Owner device the exit is replaced by the driver's
  `Poisoned` entry.
- **M-1 — CONFIRMED.** `drain_ready_crtc_configs` removes a parked request
  only on a ready wake or cancellation (`core_loop/run.rs:1055`); a superseded
  transition would park its client forever. Fixed: a disposition → reply →
  publication table in 3b; `Deferred` must match Legacy's measured behavior
  and never park without bound. Note: Legacy's `apply_crtc_config` has no seat
  gate, so what a modeset does while the VT is released today is what the
  user's VT golden capture will show.
- **M-2 — CONFIRMED.** `set_time` is captured at dispatch
  (`process_request.rs:4657`) and written as the RANDR timestamp at completion
  (`rebuild_randr_state`, `backend.rs:10652`). Fixed: RANDR outcomes publish
  in dispatch order across devices (publication only, never hardware work),
  which is what Legacy's synchronous path does by construction; added to the
  protocol-order gate.

---

## Verdict

**1 blocking, 2 major, 0 minor**  
**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result. It does not establish that the design compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Round-1 finding | Status | Revision-2 assessment |
| --- | --- | --- |
| B-1: failure path before live DPMS | **PARTIAL** | [3a now specifies poison entry, terminalization, dispositions, and retained quarantine](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:201). It conflates explicit rejection with completion loss; see B-1 below. Recovery exits remain assigned to 3d. That is a contained intermediate debt while Owner is fixture-only, but it is not a completed `REC-1` path. |
| B-2: publication tied to requester | **TRADED** | [Publication is now requester-independent](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:233). The new asynchronous path leaves superseded requests and cross-device timestamp order unresolved; see M-1 and M-2. |
| M-1: no effectful actor | **APPLIED** | [The device-local lifecycle driver applies actions and reports acknowledgements](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:114). It is specified to use the existing device slot, so the design does not introduce a second commit owner. |
| M-2: DPMS inheritance | **APPLIED** | [Topology installation refreshes projections before installation and invalidates removed outputs](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:213). |
| M-3: typed tag claimed freshness | **APPLIED** | [The driver now checks incarnation, epoch, and transition at one result-disposition boundary](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:129). |
| M-4: backend test cannot observe wire order | **APPLIED** | [The exit gate now drives the core request path and observes requester and listener connections](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:307). |
| M-5: stage-3 parent requirement | **APPLIED** | [C.0 §18.3 now limits stage-3 conversion to Owner paths and assigns Legacy removal to stage 5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:4056). |

## Findings

### Blocking

**B-1 — 3a treats a proven rejection as incarnation poison.** [The 3a failure edge groups a rejected DPMS commit with lost completion evidence and a breached deadline](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:201). C.0 instead makes explicit ioctl rejection `FailedBeforeSubmit`, with the previous state authoritative and a classified retry, topology-scoped latch, or readiness closure; completion-mechanism breaches poison the incarnation ([§10:1955](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1955), [§10:1992](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1992)). For example, an explicit `EINVAL` on a DPMS transition proves that no new state was submitted, yet the proposed path closes the whole Owner incarnation and retains an unnecessary quarantine until 3d. Separate rejection from acceptance-unknown and completion failure in 3a’s transition outcomes. The Owner failure route must also reach that handler: the current `MechanismFailed` route calls `request_exit()` ([backend.rs:21183](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:21183)), which would prevent the claimed fixture-only wait in `Poisoned`.

### Major

**M-1 — A superseded parked RANDR request has no defined completion.** The revision defines publication at `Applied` and a failure reply for rejection or acceptance-unknown, but gives no reply/publication rule for `SupersededBy`, `Invalidated`, or an absorbed target ([plan:233](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:233), [plan:292](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:292)). A client can park a modeset, then VT release supersedes its transition. A late accepted-stale success cannot publish installed state under [C.0 `REC-4`](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:801), while the client still needs its parked request resolved; the present core removes that request only on a ready wake or cancellation ([run.rs:1055](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1055)). Specify how every terminal `REC-5` disposition wakes and answers a connected requester, and which disposition, if any, publishes a successful RANDR change. A stale result must never publish the displaced transition.

**M-2 — Independent device completions can move RANDR `lastSetTime` backward.** The proposed Owner path parks every real mutation and publishes when each device reaches `Applied` ([plan:239](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:239)); C.0 keeps commits per device ([§13:2521](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2521)). Today the core captures `set_time` when dispatching a request, then successful completion writes that value directly as the RANDR timestamp ([process_request.rs:4657](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:4657), [backend.rs:10652](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:10652)). If device A’s earlier request finishes after device B’s later request, A’s publication restores an older `lastSetTime`. Define an Xorg-compatible protocol publication order or timestamp policy for concurrent device transitions, while preserving the distinct `lastConfigTime` rule. Include that ordering in the core-level evidence gate.

## Coverage and implementation checks

- **Incorporation:** All seven round-1 findings were checked against revision 2. The 3d recovery exits are deferred within the same, non-mergeable stage sequence; production activation follows in stage 5 ([C.0 §18:4003](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:4003)).
- **Architecture:** Checked coordinator, arbiter, lifecycle driver, device slot, owner-event routing, and the parked core continuation. The driver is an orchestrator in the stated design; exact handoff to the existing owner and resource consumer remains an implementation integration check.
- **Safety and specification:** Checked `REC-1..6`, ordering classes, the §10 rejection and quarantine rules, multi-device ownership, and the §18 amendment. B-1 and M-1 identify the unresolved failure and supersession contracts.
- **Verification:** Checked the proposed core protocol gate and implementation gates. **24/24 bounded spec/source excerpts** were used beyond the once-read design and prior review. Physical Owner-fixture capability and detailed sub-stage evidence remain unassessed. Formatting, clippy, builds, portability, tests, and hardware runs belong to implementation; none ran here.
