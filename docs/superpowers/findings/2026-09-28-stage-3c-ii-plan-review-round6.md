# Stage 3c-ii plan — codex review, round 6

**Target:** `docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md` revision 6 (`28b75b24`), prior: rounds 1–5.

**Result:** 3 blocking, 0 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (24/24). Incorporation: all carried findings APPLIED; round 5 M-1 TRADED (see B-3).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.157.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification and triage (2026-09-28):** all three CONFIRMED; all three
are **edge cases** under the user's convergence criterion (no cross-task or
cross-crate contract, no ownership or task-order defect). The review loop is
closed; each is carried into the plan as an implementation note with its
invariant and test:

- **B-1** → **IN-2** (Task 4): a separate renderer's card or render node removal must request exit; `c0_3cii_separate_renderer_removal_exits_vulkan` (H69).
- **B-2** → **IN-1** (Task 1): a Legacy failure in a mixed acquire exits on receipt; `c0_3cii_mixed_acquire_legacy_failure_on_receipt_vulkan` (H68).
- **B-3** → **IN-3** (Task 5): a hotplug answer older than a published forced-reprobe result is invalidated and re-probed; `c0_3cii_forced_reprobe_invalidates_older_hotplug_answer_vulkan` (H70).

## Review as received

## Verdict

**3 blocking, 0 major, 0 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design review. It does not establish that the implementation compiles, passes tests, or is approved.

## Incorporation audit

| Prior finding | Status | Audit |
| --- | --- | --- |
| Round 1 B-1 — episode overlaps a mutation | APPLIED | Task 2 requires a core grant before staging or dispatch. |
| Round 1 B-2 — retry spent on a stuck worker | APPLIED | The device fails promptly; retry follows worker reaping. |
| Round 1 M-1 — `ENODEV` waits for peers | APPLIED | Removal is handled on receipt. |
| Round 1 M-2 — release leaves probes outstanding | APPLIED | Release invalidates the episode and discards late answers. |
| Round 1 M-3 — hardware missing at task boundaries | APPLIED | The coordinator schedule covers each real-path task. |
| Round 2 B-1 — worker wake precedes consumption | APPLIED | Result consumption sends a further core wake for queued work. |
| Round 2 B-2 — next edge waits for a stuck worker | APPLIED | It fails promptly and retries after reaping. |
| Round 2 M-1 — half-switched route | APPLIED | Acquire and hotplug each switch with their application. |
| Round 2 M-2 — forced reprobe has two turn owners | APPLIED | Its request owns the turn; it opens no topology episode. |
| Round 3 B-1 / round 4 B-1 — forced probe hides KMS work | APPLIED | Installed routes govern KMS work, and revision 6 uses the same preservation predicate for disables. |
| Round 3 M-1 — unreachable second probe after failed acquire | APPLIED | Task 1 checks closure and reaping; Task 2 checks the next edge. |
| Round 4 M-1 — released query has no response | APPLIED | Task 5 probes and replies while released. |
| Round 5 B-1 — failed Owner acquire waits for peers | APPLIED | `EIO` and `ENODEV` close and withdraw on receipt. |
| Round 5 B-2 — zero-mode route remains in the commit | APPLIED | The commit disables every output failing `preserves_active_output`. |
| Round 5 B-3 — released reply differs from Legacy | APPLIED | The released-seat forced probe now uses the worker and tests parity. |
| Round 5 M-1 — forced query overlaps a probe episode | **TRADED** | Waiting prevents concurrent probes, but permits a newer forced result to publish before an older hotplug result; see B-3. |

## Findings

### Blocking

**B-1 — Removal of a separate renderer has no event-to-exit path.** Task 4 maps `remove` only for an *open KMS card*, although it promises exit when the renderer’s device or render node disappears ([plan:477](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:477), [plan:503](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:503)). The selected renderer can be a separate endpoint whose primary node is not an open KMS device ([source:11056](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:11056)). Remove that renderer’s card or render node: the proposed classifier finds no open-card match, so it never requests exit, contrary to [spec §4.5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:422). Correlate removal records with the selected renderer’s primary and render-node identities as well as open KMS cards; test the separate-renderer case.

**B-2 — A failed Legacy acquire answer is deferred behind an unrelated worker.** Task 1 places acquire dispositions in the continuation at episode resolution, then explicitly makes only failed *Owner* answers immediate ([plan:297](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:297)). On a mixed server, a Legacy probe can answer `EIO` while an Owner worker remains blocked; the described continuation delays Legacy’s exit until that worker answers or times out. [Spec §3.3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:209) assigns *every* participant its disposition when its answer arrives. Specify Legacy failure exit on receipt, and test it with a blocked Owner peer.

**B-3 — A newer forced result can be overwritten by an older hotplug snapshot.** Revision 6 lets a forced request wait for a hotplug *probe*, then run and publish while the hotplug topology grant waits behind its turn ([plan:173](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:173), [plan:552](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:552)). If hotplug probes an unplug, then the connector returns before the forced probe, the forced request publishes the newer connected state first. The waiting hotplug episode can subsequently stage its older unplug snapshot and disable the route. That reverses the observed state and notifications; Legacy processes its synchronous hotplug rescan before the later request, and the [spec requires parity outside named exceptions](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:449). Define an ordering or invalidation rule so the older answer cannot apply after the newer publication. Extend Task 5’s overlap test with divergent probe answers; its current scenario ([plan:587](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:587)) does not establish that rule.

### Major

None.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** Audited all carried findings and all four round 5 findings against task text.
- **Architecture:** Checked episode and gate ownership, result delivery, task order, and the removal classifier. The three findings identify missing or conflicting production contracts.
- **Safety and failure:** Checked receipt timing, deadlines, release invalidation, stale-result handling, worker reaping, and renderer identity.
- **Spec and evidence:** Checked the in-scope §§3–6 requirements, task-local test dependencies, runtime probe seam, core-entry/executor test rules, and hardware schedule. The plan assigns formatting, regular CI clippy, tests, portability, and hardware gates to implementation.

Used **24/24 bounded spec/source excerpts**. Actual udev delivery, kernel behavior on this hardware, and the future implementation remain unassessed; they are not judged sound. No build, test, or hardware command was run.