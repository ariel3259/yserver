# Stage 3c-ii plan — codex review, round 5

**Target:** `docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md` revision 5 (`039eccf7`), prior: rounds 1–4.

**Result:** 3 blocking, 1 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (20/24). Incorporation: eleven carried findings APPLIED; rounds 3/4 B-1 PARTIAL (see B-2); round 4 M-1 TRADED (see B-3).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.157.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-28):** all four CONFIRMED.

- **B-1** — spec §3.3 gives each acquire participant its disposition once its probe answers; rev 5 deferred an `EIO` answer to episode resolution. Rev 6: a failed Owner answer is closed and withdrawn on receipt (H6b extended to `EIO`).
- **B-2** — rev 5 decided KMS work with `preserves_active_output` but the commit disabled only connectors that were gone. Rev 6: the disable set uses the same predicate.
- **B-3** — rev 5's released-seat reply from the published state was an unnamed parity difference. Rev 6 removes it: while released the worker's dup'd fd is not master, so the kernel demotes the forced reprobe to the same cached read Legacy's synchronous probe gets; the forced reprobe runs on the worker then too (`c0_3cii_forced_reprobe_while_released_matches_legacy_vulkan`, H65 redefined). The question put to the user in rev 5 is withdrawn.
- **M-1** — a forced reprobe arriving during an open hotplug probe episode had no arbitration. Rev 6: it parks, waits for that episode, then probes; deadline from the request; no deadlock (`c0_3cii_forced_reprobe_waits_for_open_hotplug_probe_vulkan`, H66, H67).

## Review as received

## Verdict

**3 blocking, 1 major, 0 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design review. It does not establish that an implementation compiles, passes tests, or is approved.

## Incorporation audit

| Prior finding | Status | Audit |
| --- | --- | --- |
| Round 1 B-1 — episode overlaps a client mutation | **APPLIED** | Task 2 requires a core grant before staging or dispatch (plan 350–355). |
| Round 1 B-2 — retry spent on a stuck worker | **APPLIED** | The episode fails that device at once; retry waits for reaping (149–159, 416). |
| Round 1 M-1 — `ENODEV` waits for peers | **APPLIED** | `ENODEV` causes withdrawal on receipt (199–205, 482). |
| Round 1 M-2 — release leaves probes outstanding | **APPLIED** | Release invalidates every outstanding probe (206–216, 422). |
| Round 1 M-3 — missing hardware at task boundaries | **APPLIED** | The schedule assigns hardware after each real-path task (581–590). |
| Round 2 B-1 — worker wake precedes result consumption | **APPLIED** | Consumption sends a further wake for core-drained work (131–148). |
| Round 2 B-2 — next edge waits for a stuck worker | **APPLIED** | The edge fails promptly and retries after reaping (149–159, 416). |
| Round 2 M-1 — half-switched route | **APPLIED** | Acquire and hotplug each switch with their application (271–290, 314–388). |
| Round 2 M-2 — forced reprobe has two turn owners | **APPLIED** | Its parked request owns the turn; it sends no `EpisodeBegin` (217–225, 506–525). |
| Round 3 B-1 / round 4 B-1 — forced reprobe hides later KMS work | **PARTIAL** | Revision 5 fixes the KMS-work predicate and adds zero-mode tests (319–332, 410, 552), but the commit description still disables only connectors that are gone. See B-2. |
| Round 3 M-1 — unreachable second probe after failed acquire | **APPLIED** | Task 1 tests closure and reaping; Task 2 tests the reachable next edge (299, 416). |
| Round 4 M-1 — query begun while released has no response | **TRADED** | Task 5 now gives an immediate response (531–555), but that response can differ from Legacy outside a named spec exception. See B-3. |

## Findings

### Blocking

**B-1 — A failed acquire probe waits for an unrelated worker.** [Task 1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:260) runs the acquire continuation only when *every* device answers or the deadline passes (lines 271–285); its EIO test explicitly defers the failed disposition to resolution (line 300). If Owner B answers EIO promptly while Owner A remains blocked, B’s closure and urgent withdrawal wait for A’s deadline. The [spec requires each acquire participant’s disposition when its probe answers](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:209), and a failed Owner’s withdrawal is immediate (lines 223–228). Give failed Owner results their terminal disposition on receipt while keeping the episode open for the remaining participants; test EIO with a blocked peer.

**B-2 — The zero-mode KMS decision has no matching disable rule.** Task 2 correctly declares KMS work when an installed output fails `preserves_active_output`, including a connector with zero modes ([plan:323](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:323)). Its [commit description](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:367), however, disables outputs only when the connector is *gone*. For a present, zero-mode connector, the staged logical state drops the route but the described commit leaves its CRTC scanning out. Legacy’s [preservation predicate](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:3103) and [snapshot application](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:9387) both drop that route; the spec requires Legacy parity and retirement of lost active outputs ([spec:55](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:55), [spec:381](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:381)). Define the commit’s disable set with the same preservation predicate used to decide KMS work.

**B-3 — The released-seat reply is an unapproved parity exception.** Revision 5 replies from published state without probing when `GetScreenResources` begins while released ([plan:531](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:531)). The plan itself notes that Legacy’s cached connector probe may already see an HPD change (lines 537–540). Thus, after that change, Legacy can return the new resource list while Owner returns the old one, without a probe timeout. The [named exception](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:449) permits stale contents *after a deadline*; §5 says other differences are defects (line 463). The existing request path [calls the Legacy reprobe](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:3173). Specify a bounded released-seat path that preserves its cached-state reply, or obtain an explicit spec change before execution.

### Major

**M-1 — A forced query can arrive while a hotplug probe owns the sole probe slot.** The plan permits only one probe episode at a time ([plan:149](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:149)), yet Task 5 says an Owner forced query starts one and parks its reply ([plan:519](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:519)). Before a hotplug probe finishes, it has no topology gate turn; the current gate therefore allows the forced query to run ([source:817](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:817)). With a hotplug worker still answering, the plan gives no rule for whether the query waits, shares that answer, or supersedes it—and no deadline or turn rule for the wait. Define that arbitration and test this overlap through the core-entry driver.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** audited all 11 carried findings and both round 4 findings; the earlier grant, wake, retry, release, and hardware corrections remain stated.
- **Architecture:** checked probe and gate ownership, result delivery, task order, and relevant core-entry behavior. The forced-query overlap remains unresolved.
- **Safety:** checked deadlines, stale results, urgent withdrawal, worker reaping, and released-seat handoff. The failed-acquire timing remains unresolved.
- **Spec and evidence:** checked the in-scope §§3–6 rules, named scenarios, mutations, and hardware schedule. The zero-mode commit rule and released-seat parity remain unresolved.

Used **20/24 bounded spec/source excerpts**. Verified source ground includes Legacy’s zero-mode route rule, the current forced-reprobe request path, and the gate’s admission behavior. Actual udev delivery and the proposed implementation remain unassessed, not judged sound. Compiler, formatting, clippy, portability, tests, and hardware runs belong to implementation; none were run.