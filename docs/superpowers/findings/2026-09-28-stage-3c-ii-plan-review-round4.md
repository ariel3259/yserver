# Stage 3c-ii plan — codex review, round 4

**Target:** `docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md` revision 4 (`aa08ed76`), prior: rounds 1–3.

**Result:** 1 blocking, 1 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (19/24). Incorporation: ten of eleven carried findings APPLIED; round 3 B-1 PARTIAL.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.157.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-28):** both CONFIRMED.

- **B-1** — `ConnectorSnapshot::preserves_active_output` (`render/platform.rs`) requires the same connector **and** a non-empty mode list; rev 4's predicate looked at presence only. Rev 5: KMS work uses that predicate; `c0_3cii_zero_mode_connector_loses_its_route_vulkan` (H64) and a zero-mode case in the forced-reprobe-then-edge test.
- **M-1** — a `GetScreenResources` issued while the seat is released had no rule (no worker may be spawned). Rev 5: it replies at once from the published state, holds no turn; the acquire publishes the change; named as a narrow extension of spec §5 exception 2 for the user; `c0_3cii_forced_reprobe_while_released_replies_at_once_vulkan` (H65).

## Review as received

## Verdict

**1 blocking, 1 major, 0 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design review, not a claim that the implementation compiles, passes tests, or is approved.

## Incorporation audit

| Prior finding | Status | Audit |
| --- | --- | --- |
| Round 1 B-1 — episode overlaps a client mutation | **APPLIED** | Task 2 waits for a core grant before staging or dispatch ([plan:340](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:340)). |
| Round 1 B-2 — retry spent on a stuck worker | **APPLIED** | A stuck device fails at once; a retry waits for worker reaping ([plan:142](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:142)). |
| Round 1 M-1 — `ENODEV` waits for peer probes | **APPLIED** | Withdrawal occurs when the answer is consumed ([plan:192](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:192)). |
| Round 1 M-2 — release leaves probes outstanding | **APPLIED** | Release invalidates every probe cause ([plan:199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:199)). |
| Round 1 M-3 — missing hardware at task boundaries | **APPLIED** | The schedule assigns hardware after each real-path task ([plan:560](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:560)). |
| Round 2 B-1 — worker wake precedes result consumption | **APPLIED** | Consumption sends another wake after enqueueing core-drained work ([plan:124](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:124)). |
| Round 2 B-2 — next edge waits for a stuck worker | **APPLIED** | Task 2 starts the edge episode immediately and tests the later retry ([plan:405](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:405)). |
| Round 2 M-1 — half-switched hotplug route | **APPLIED** | Task 1 switches acquire with its continuation; Task 2 switches hotplug with its application ([plan:264](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:264), [plan:307](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:307)). |
| Round 2 M-2 — forced reprobe has two gate owners | **APPLIED** | The parked request owns its turn; forced reprobe sends no `EpisodeBegin` ([plan:210](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:210)). |
| Round 3 B-1 — forced reprobe hides later KMS work | **PARTIAL** | The unplug and relight cases now use installed and remembered routes, but the installed-output predicate omits a present connector with no modes. See B-1. |
| Round 3 M-1 — unreachable second probe after failed acquire | **APPLIED** | Task 1 now tests closure, exclusion and reaping; Task 2 carries the reachable no-second-worker mutation ([plan:292](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:292), [plan:405](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:405)). |

## Findings

### Blocking

**B-1 — A present connector with no modes leaves an installed output active.** Task 2 admits KMS work when an installed output’s connector is *absent*, or a remembered route can be relit; a mode-list change alone can take the logical-only path ([plan:312](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:312), [plan:153](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:153)). If an active connector remains present but its probe returns zero usable modes, neither KMS condition holds. The current heavy rescan treats that output as lost and drops its active scanout ([source:3103](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:3103), [source:9387](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:9387)); leaving it installed breaks Legacy parity and the hotplug detach contract ([spec:55](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:55), [spec:381](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:381)). Define KMS work using the existing *preserves active output* predicate, and test the zero-mode case, including after a forced reprobe has updated the registry.

### Major

**M-1 — Forced resource queries begun while the VT is released have no response contract.** Decision 12 forbids spawning a worker while released, but Task 5 says every Owner `GetScreenResources` starts a forced probe episode and returns `Pending`; its only release case begins the request *before* release ([plan:199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:199), [plan:495](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:495), [plan:534](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:534)). A client can issue this request after release. The stated paths then either spawn a forbidden probe or leave the reply and gate turn without a resolution rule. Specify the released-seat response and turn release, and test a request initiated in that state; account for any client-visible parity difference under the spec’s named exceptions ([spec:444](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:444)).

### Minor

None.

## Coverage and implementation checks

The incorporation audit covered all 11 carried findings; revision 4 did not undo the nine round-1/2 corrections inspected. Architecture checks covered gate ownership, worker delivery and task order. Safety checks covered deadlines, stale answers, release, removal and worker reaping. Spec and evidence checks covered the in-scope §§3–6 requirements, named tests and hardware schedule.

I used **19/24 spec/source retrievals**. One platform retrieval inadvertently spanned 126 lines, six over the per-excerpt cap. Verified ground includes the core drain order and Legacy’s active-output preservation rule. Actual udev connector sub-device delivery and the implementation of the new paths remain unassessed, not judged sound. Compilation, formatting, clippy, portability, tests and hardware runs remain implementation checks; none were run.