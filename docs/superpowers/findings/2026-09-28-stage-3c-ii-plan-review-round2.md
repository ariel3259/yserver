# Stage 3c-ii plan — codex review, round 2

**Target:** `docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md` revision 2 (`986b3649`), prior: round 1.

**Result:** 2 blocking, 2 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (21/24). Incorporation: round 1's B-1, M-1, M-2, M-3 APPLIED; B-2 TRADED.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.157.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-28):** all four CONFIRMED.

- **B-1** — `drain_requesterless_publications` runs only in the core's `CrtcConfigReady` handling and after the VT entries (`core_loop/run.rs`); `poll_deferred_input` runs in `run_iteration_tail` with no drain after it, so a result consumed there is not delivered until another wake. Rev 3: decision 5 — the backend sends `CrtcConfigReady` after enqueueing anything the core drains (the producer contract the trait already documents); `c0_3cii_worker_answer_alone_reaches_clients_vulkan` (H57).
- **B-2** — rev 2's decision 6 held a new edge until the stuck worker returned, contradicting spec §4.1's fast failure. Rev 3: episodes are never delayed; the stuck device fails at once and one retry runs when the worker is joined; `c0_3cii_edge_fails_stuck_device_fast_then_retries_vulkan` (H51, H58).
- **M-1** — rev 2's Task 1 switched the Owner hotplug route to a log-only consumer before Task 2's application existed. Rev 3 (rewritten as one unit, not patched): the worker's first consumer is the acquire (3c-i already delivers its application; rev 2's Task 5 folded into Task 1), and the hotplug route switches in Task 2 with classification, grant, transaction and episode together. Six tasks.
- **M-2** — rev 2 named `ForcedReprobe` as a topology-episode cause while Task 6 gave the parked request the turn. Rev 3: decision 13 separates probe episodes (worker level) from topology episodes (gate level); the forced reprobe never signals `EpisodeBegin`; `c0_3cii_forced_reprobe_owns_one_turn_vulkan` (H60).

## Review as received

## Verdict

**2 blocking, 2 major, 0 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design review. It makes no claim that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior finding | Status | Audit |
| --- | --- | --- |
| B-1 — episode overlaps a client mutation | **APPLIED** | The core grant now waits for any in-flight mutation, and the backend stages nothing before the grant. The task names both same-device and cross-device ordering tests ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:294)). A separate delivery defect remains below. |
| B-2 — timeout retry is spent on a stuck worker | **TRADED** | The background retry now waits for the worker to be joined, but the same rule also holds a new hotplug edge instead of giving it the specified immediate failure ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:110)). See B-2. |
| M-1 — `ENODEV` waits for peer probes | **APPLIED** | Task 4 withdraws on receipt and tests a blocked or failed peer ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:364)). Task 1’s interim production route remains a separate task-boundary defect below. |
| M-2 — release leaves probes outstanding | **APPLIED** | The release rule covers all probe causes; Task 6 resolves a parked forced reprobe and discards its late answer ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:159)). |
| M-3 — missing hardware at task boundaries | **APPLIED** | The schedule assigns real-path runs after Tasks 1, 4 and 6 ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:510)). The Task 1 run cannot detect M-1. |

## Findings

### Blocking

**B-1 — The worker wake precedes consumption of its result.** The plan has the worker send `CrtcConfigReady`, then has `poll_deferred_input` consume its result ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:98)). In the current loop, that message drains ready work and episode events *before* the iteration tail calls `poll_deferred_input`; the tail has no subsequent event drain ([message handler](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:2377), [tail](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:2749)). Thus an isolated worker answer can be consumed after its sole wake has been serviced. Its grant request, publication, or `ENODEV` urgent withdrawal then waits for another loop wake; the proposed check that polling ran after the wake does not establish delivery. This violates the terminal publication and urgent withdrawal contracts ([spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:305), [spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:320)). Consume results before the core drain, or explicitly drain the resulting events in that iteration. Test an isolated worker completion with no second stimulus.

**B-2 — A new edge loses the specified fast-failure path.** The spec says a new probe of a device with a stuck worker fails at once, and the next edge starts a new episode ([spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:280), [spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:293)). Decision 6 instead keeps an edge pending until that worker returns; Task 1 simultaneously requires a new episode to fail fast and an edge to start no episode ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:110), [tests](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:238)). If the worker never returns, the next edge never gets the promised immediate disposition. Preserve the retry for when the worker joins, while allowing the new edge’s probe episode to fail that device at once; make the two tests exercise those distinct events.

### Major

**M-1 — Task 1 switches the production hotplug route before its application exists.** Task 1 replaces the Owner rescan with a probe whose consumer only logs; it changes no registry, transition or publication. Task 2 nevertheless says the route switches only when its transaction and publication are co-delivered ([Task 1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:225), [Task 2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:253)). A physical unplug after Task 1 takes the new route and leaves the old topology published, contrary to the required episode outcome ([spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:280)). Task 1’s no-cable hardware run cannot catch that, and its retry test requires an answer to *apply* before Task 2 supplies application ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:240)). Co-deliver Tasks 1 and 2 as one real-path boundary, with its tests and hardware run after application exists.

**M-2 — Forced reprobe has two possible owners of one gate turn.** Task 2 says the topology episode type gains a `ForcedReprobe` cause; Task 6 also makes the core’s parked forced-reprobe request hold the gate turn ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:282), [plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:449)). The spec assigns that request its own turn ([spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:348)). If the backend also sends `EpisodeBegin`, the new grant rule queues it behind the forced-reprobe flight that is waiting for the probe: a self-wait, with unclear publication and release ownership. Specify that the forced probe uses the request’s existing turn, with no second grant request, or define a single alternative owner. Pin the one-turn ordering in a test.

### Minor

None.

## Coverage and implementation checks

The incorporation audit covered all five prior findings. Architecture checks covered the gate and worker-result delivery, task boundaries, and forced-reprobe ownership. Safety checks covered stuck-worker ordering, release invalidation and urgent removal timing. Spec and evidence checks covered the relevant §3–§6 rules, named tests and hardware schedule.

I used **21/24 spec/source excerpts**; two grouped spec reads exceeded the requested 120-line excerpt limit. Verified baseline ground includes the current core drain order, iteration tail, gate state and synchronous hotplug paths. Exact udev metadata on connector sub-devices and kernel behavior beyond the prior review’s established findings remain unassessed, not judged sound. Compilation, tests, portability gates and hardware execution remain implementation checks; none were run.