# Stage 3c-ii plan — codex review, round 3

**Target:** `docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md` revision 3 (`89b804f2`, a rewrite), prior: rounds 1 and 2.

**Result:** 1 blocking, 1 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (23/24). Incorporation: all nine findings of rounds 1–2 APPLIED.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.157.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-28):** both CONFIRMED.

- **B-1** — Legacy's rescan drops every installed output absent from the snapshot even when a forced probe already marked its connector disconnected (`render/platform.rs`, the comment in `apply_connector_snapshot_for_devices`); rev 3 said "discovered topology", which read as the registry would miss the unplug. Rev 4: KMS work judged against installed and remembered routes, logical change against the registry; `c0_3cii_forced_reprobe_then_edge_still_detaches_and_relights_vulkan` (H63, Task 5).
- **M-1** — an acquire probe past its deadline closes the Owner device (spec §3.3) and closed devices are not probed again, so Task 1's second-probe path was unreachable. Rev 4: Task 1 asserts closure, exclusion and reaping (H61, H62); H5 moves to Task 2's hotplug retry test.

## Review as received

## Verdict

**1 blocking, 1 major, 0 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design review, not a claim that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior finding | Status | Audit |
| --- | --- | --- |
| Round 1 B-1 — episode overlaps a client mutation | **APPLIED** | Decision 10 requires a core grant before staging or dispatch; Task 2 tests same-device and cross-device ordering ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:170)). |
| Round 1 B-2 — timeout retry spent on a stuck worker | **APPLIED** | Decision 6 retains one retry until the worker is joined ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:134)). |
| Round 1 M-1 — `ENODEV` waits for peer probes | **APPLIED** | Decision 11 acts on receipt; Tasks 1 and 4 test withdrawal before a blocked peer answers ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:184)). |
| Round 1 M-2 — release leaves probes outstanding | **APPLIED** | Decision 12 invalidates all probe causes and resolves a parked forced request ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:191)). |
| Round 1 M-3 — missing hardware at task boundaries | **APPLIED** | The schedule assigns hardware after each real-path task ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:540)). |
| Round 2 B-1 — worker wake precedes result consumption | **APPLIED** | Decision 5 requires a second wake after consumption enqueues core-drained work; Task 1 names an isolated-answer test ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:116)). |
| Round 2 B-2 — next edge waits for a stuck worker | **APPLIED** | The edge starts an episode and fails that device at once; the join triggers a retry ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:134)). |
| Round 2 M-1 — half-switched hotplug route | **APPLIED** | Task 1 uses the existing acquire application; Task 2 switches the hotplug route with its application ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:232), [Task 2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:299)). |
| Round 2 M-2 — forced reprobe has two gate owners | **APPLIED** | Decision 13 gives the parked request the sole turn and forbids `EpisodeBegin` ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:202)). |

## Findings

### Blocking

**B-1 — A forced reprobe can erase the change that admits the later hotplug transaction.** Task 5 applies a forced result to the connector registry while deliberately leaving an enabled output alone; Task 2 admits a hotplug topology episode only when the new snapshot differs from “discovered topology” ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:304), [plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:321), [plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:489)). If `GetScreenResources` first records an unplug in that registry, the later udev probe can return the same connector state. The plan’s “unchanged → none” rule then grants no turn, leaving the active route enabled, contrary to the required detach and relight behavior ([spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:381)). The current heavy rescan explicitly handles a connector already marked disconnected by a forced probe by checking the active output separately ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:9429)). Require Task 2’s classification and grant condition to include unreconciled installed or remembered routes, or retain a separate heavy-topology baseline. Test forced reprobe **then** udev edge for both unplug and relight.

### Major

**M-1 — Task 1’s stuck-worker test cannot reach its claimed second probe.** The test hangs B during acquire until that acquire’s deadline, then releases and acquires again expecting B to fail fast as a stuck participant ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:284)). But Task 1 closes an Owner device whose acquire probe misses the deadline and excludes closed devices from later probe episodes ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:245), [plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:263)); that closure is required by [spec §3.3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:209). B therefore cannot take the test’s second-probe path. Make Task 1 assert closure, exclusion and eventual worker reaping; attach its no-second-worker mutation to Task 2’s still-open hotplug retry scenario.

### Minor

None.

## Coverage and implementation checks

The incorporation audit covered all nine findings across both rounds. Architecture checks covered task order, gate ownership, result delivery and the forced-reprobe/hotplug handoff. Safety checks covered deadlines, worker retention, release, removal and urgent withdrawal. Spec and evidence checks covered the in-scope §3–§6 requirements and the per-task hardware schedule.

I used **23/24 bounded spec/source excerpts**. Verified baseline ground includes the core’s drain order, the forced probe’s registry update, and the heavy rescan’s active-output check. Actual connector sub-device udev metadata and implementation details beyond those paths remain unassessed, not judged sound. Formatting, clippy, compilation, tests, portability and hardware runs are implementation checks; none were run.