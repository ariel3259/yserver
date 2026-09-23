# Plan Cp (copied scanout route) — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md`
revision 1 (`6fa59538`), against the copied-route design at revision 4 (`80089003`).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
Coverage COMPLETE FOR DECLARED SCOPE, 24/24 excerpts used; `CompletionPoller`
internals, the remaining device-loss clear caller, the owner-ledger gates,
`FenceTicket` sourcing, copied-fixture construction, transport encoding and
hardware reachability are recorded as unassessed.

**Author verification (2026-09-22), every finding checked against the tree:**

- **M-1 — CONFIRMED.** Revision 1's Task 2 required five route tests to pass
  while stating that Tasks 3 to 6 would supply the receipt producer, and
  permitted an interim test seam. That is plan Ciii's round-1 M-1 repeated by
  its own author: evidence demanded before the route is reachable. The tree
  agrees there is no earlier producer — the Owner completion finalises A and
  queues an offer (`scene.rs:3892`, `:3971`), and the copied branch otherwise
  calls the Legacy submit (`scene.rs:4006`, `platform.rs:6443`). Fixed: Tasks 2
  and 3 are swapped, the preparation carries the wake registration so the
  promotion is reachable through production, and the seam permission is gone.
- **M-2 — CONFIRMED.** The spec's §8.2 row "a failed copy submission offers
  nothing and discharges its leases through the service" (spec line 428) had no
  counterpart in the plan: rewriting CP-4b around `gpu_submitted` replaced it
  with a resource-disposition row, so an implementation could dispose of the
  resources correctly and promote the generation anyway. Restored as its own
  criterion with Q36 and Q37.
- **M-3 — CONFIRMED, and the caller count is worse than the finding states.**
  `clear_scanout_render_completions` (`platform.rs:4987`) is a distinct
  operation from the per-output cancellation (`platform.rs:4968`) and has
  **three** production callers, not one: VT suspend (`platform.rs:6634`),
  `platform.rs:7664` and `scene.rs:3128`. Revision 1 named only output removal.
  Decision 11 now binds the wake-container choice to both operations, with Q38
  forcing a stale completion to survive a VT suspend.

**Assessment.** First plan review under this instrument to return zero blocking.
All three majors are the same class — a criterion or a caller that the plan
named once and the tree has twice — which is what the review is for, and none
required changing the architecture the design settled over three rounds.

Revision 2 incorporates all three.

---

## Verdict

**0 blocking, 3 major, 0 minor**

Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design-review result only; it does not claim compilation, test success, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| None | N/A | This is the first review of the implementation plan; check 1 was skipped as instructed. The three required design-review records were read and used as verified seam context. |

## Findings

### Blocking

None.

### Major

#### M-1 — Task 2 requires production-route evidence before any production path can produce its receipt

Task 2 explicitly says Tasks 3–6 will provide the receipt producer, yet requires five Vulkan route tests to pass immediately and permits an interim “test seam” ([plan:121](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:121), [plan:127](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:127)). That conflicts with the plan’s rule that state must be reached through production entries ([plan:43](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:43)) and the spec’s requirement for real producers ([spec:400](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:400)).

The current production fork confirms the problem: an Owner completion finalizes A and queues an offer directly ([scene.rs:3892](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:3892), [scene.rs:3971](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:3971)); otherwise the copied path calls the Legacy copy/KMS operation ([scene.rs:4006](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:4006), [platform.rs:6443](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:6443)). Before Task 3, no production Owner path can create a destination receipt or copy-wait state.

Concrete failure: Task 2’s seam injects a receipt and all Q3–Q9 tests pass, while the later production producer is absent or incorrectly connected. The evidence proves the predicate in isolation, not the claimed route.

Smallest correction: combine Tasks 2 and 3, or first land enough of the production preparation/submission/registration path to produce the receipt, then run Task 2’s acceptance tests through that path. A test seam may develop the predicate but cannot satisfy the task’s exit evidence.

#### M-2 — Failed submission can still offer while every listed Task 4 mutation passes

The spec requires a failed copy submission to make the generation `Displaced`, offer nothing, and discharge leases through the service ([spec:281](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:281)). Its verification table explicitly requires mutations for offering after failure and releasing leases by hand ([spec:428](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:428)).

The plan substitutes only Q17–Q19, which test uncertain-dispatch cancellation, freezing and gate closure ([plan:82](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:82)). Task 4 likewise specifies resource disposition but not the required `Displaced`/no-offer transition, and its named tests only mention cancellation and uncertain dispatch ([plan:166](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:166)).

Concrete failure: an implementation correctly cancels or freezes the prepared entries and closes the gate, then erroneously promotes the generation or queues an offer. Both named tests can satisfy their stated scenarios, and the successful end-to-end test does not exercise this failure.

Smallest correction: explicitly require `Displaced` and no offer for both `gpu_submitted` outcomes, and add mutations/observations for offer-after-failure and bypassing service-owned lease discharge.

#### M-3 — Output-removal evidence does not cover the separate VT/device-loss clear operation

The spec requires cancellation for output removal, device loss and VT release ([spec:286](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:286)). Task 4 repeats that contract but names only an output-removal test and mutation ([plan:169](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:169), [plan:172](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:172)) while allowing either the existing poller or a sibling ([plan:164](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:164)).

These are distinct production operations: per-output cancellation is at [platform.rs:4968](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:4968), whereas whole-queue clearing is separate at [platform.rs:4987](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:4987), and VT suspend invokes the latter ([platform.rs:6616](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:6616)).

Concrete failure: a sibling poller is wired into per-output removal, so Q25 passes, but is omitted from VT’s whole-queue clear. Its waiter survives suspension and can later deliver a stale copied completion.

Smallest correction: make Decision 11 specify both cancellation operations. If a sibling is chosen, add VT/global-clear evidence and mutation; if the existing poller is chosen, require evidence that copy waits inhabit the same container cleared by both paths.

### Minor

None.

## Coverage and implementation checks

- Incorporation: no prior plan review; all three mandated spec-review records were read.
- Architecture/contracts: checked task ordering, eligibility, the current completion fork, Legacy copied submission, receipt production/consumption and producer reachability.
- Safety/failure: checked preparation/unwind helpers, uncertain dispatch, read-obligation ownership, wake failure and cancellation scopes.
- Compliance/verification: compared every §8.2 row with the plan’s named tests and Q1–Q35 mappings; verified the implementation and portability gates are assigned.

Excerpts used: **24/24** beyond the target plan: four design-review-record excerpts, six authoritative-spec excerpts and fourteen source excerpts.

Unassessed and not deemed sound: CompletionPoller registration internals, the remaining device-loss clear caller, full owner-ledger gate implementation, FenceTicket sourcing, copied-fixture construction, transport encoding, and hardware reachability. Exact Rust signatures, borrowing, compilation, formatting, clippy, cross-target builds, Vulkan execution and hardware mutation runs remain deferred to implementation.