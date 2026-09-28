# Stage 3c-ii plan — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md` revision 1 (`ad9182f3`), first review.

**Result:** 2 blocking, 3 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (18/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.157.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-28):** all five CONFIRMED. The reviewer also
confirmed the plan's three deliberate corrections to the spec (worker fd is a
`dup` of the master fd — the kernel forces the connector probe only for the
current master; `IncarnationFdSet` has no production instance; the forced
reprobe keeps Legacy's no-modeset behaviour).

- **B-1** — `RandrMutationGate::begin_topology_episode` (`core_loop/run.rs`) sets the episode without checking `in_flight`, and `EpisodeEnd` publishes regardless; rev 1 waited only for a modeset on the changed device. Rev 2: decision 10 — the core grants the episode's turn only with no mutation in flight and notifies the backend; nothing is staged or dispatched before the grant (Task 2, core + A/B backend tests).
- **B-2** — the background rescan armed at a forced-reprobe expiry would fail at once on the still-stuck device, and no retry remained. Rev 2: decision 6 — a pending retry waits for the stuck worker to be joined (Task 1 and Task 6 tests).
- **M-1** — `ENODEV` was consumed only at episode resolution. Rev 2: decision 11 — consumed on receipt, independent of peers (Task 1 and Task 4 tests with a blocked or failed peer).
- **M-2** — a VT release with a hotplug or forced-reprobe probe outstanding had no rule. Rev 2: decision 12 — the release invalidates every outstanding probe; a parked forced reprobe resolves `Expired`; no background rescan while released (Task 1 and Task 6 tests).
- **M-3** — Tasks 1, 4 and 6 touched real paths with no hardware run. Rev 2: a probe-worker hardware test (Task 1), the real-monitor assertion moved into Task 4's hotplug run, a forced-reprobe hardware test (Task 6); the schedule names a run after every real-path task.

## Review as received

## Verdict

**2 blocking, 3 major, 0 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design review, not a claim that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior findings | Status |
| --- | --- |
| None. This is the first review; check 1 was skipped. | — |

## Findings

### Blocking

**B-1 — A hotplug episode can overlap an unrelated client modeset.** The plan waits for a dispatched client modeset only when it is on the *changed device*, then builds the episode publication from the backend model ([plan:220](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:220), [plan:230](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:230)). The existing gate accepts `EpisodeBegin` without checking its in-flight mutation, and publishes a matching `EpisodeEnd` even if that mutation remains in flight ([gate:723](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:723), [publication:1781](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1781)). If A’s client modeset is unanswered when B’s hotplug probe completes, B can commit and publish during A’s turn. That violates the spec’s single gate turn and terminal publication rule ([spec:305](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:305)). Require a gate grant before the episode’s first commit, serialized behind **any** in-flight mutation, and test the A/B sequence.

**B-2 — The promised retry can be consumed by the timed-out worker.** On forced-reprobe expiry, the plan immediately arms a background rescan ([plan:382](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:382)). Yet a new probe of a device with a stuck worker fails at once, and a failed episode applies nothing ([plan:149](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:149), [plan:157](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:157)). If the worker remains blocked through the 150 ms debounce, that background episode fails; when the worker later returns, its stale answer is discarded and no retry remains. The change need never reach the requester-less publication required by [spec:353](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:353). Keep the retry pending until the worker is joined, then start a fresh probe; test that ordering.

### Major

**M-1 — `ENODEV` removal has no per-result timing contract.** Task 1 resolves a probe episode only after all devices answer or its deadline, and reports `Removed` in that outcome; Task 4 replaces the interim failure treatment but does not say when it consumes the removal ([plan:152](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:152), [plan:289](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:289)). If A answers `ENODEV` while B hangs, A can remain published until B’s deadline—or be hidden by B’s failed outcome. The spec requires an urgent withdrawal when `ENODEV` establishes `DeviceRemoved` ([spec:320](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:320), [spec:422](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:422)). Specify immediate removal on receipt, independent of peer probe outcomes, and test `ENODEV` with a hung or failed peer.

**M-2 — VT release does not resolve an outstanding hotplug or forced probe.** The plan specifies cancellation when release interrupts an *acquire* probe, but gives no handoff for the other two probe causes ([plan:325](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:325), [plan:341](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:341)). An edge can start a worker, then VT release can occur before its answer. An accepted late answer could stage a hotplug transaction while released; an unresolved episode can also obstruct the acquire episode. The existing release path closes only the acquire episode ([source:24029](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:24029)); the spec records edges while released and defers their topology work to acquire ([spec:180](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:180)). Define release-time invalidation for every outstanding probe cause, including the forced request’s reply and gate turn.

**M-3 — The hardware schedule omits real-path task boundaries.** The standing rule requires hardware after every task touching a real path ([plan:36](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:36), [spec:467](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:467)). Tasks 1, 4 and 6 change the production probe, udev and forced-reprobe paths, but the explicit runs are scheduled after Tasks 2, 3, 5 and 7 ([plan:422](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md:422)). Assign a real-path hardware check after each affected task, or combine task boundaries so each required check has a runnable path.

## Coverage and implementation checks

Checks 2–4 covered episode ownership, gate delivery, worker lifetime, removal timing, VT ordering, parity and evidence. **18/24 bounded spec/source excerpts** were used; locator searches were bounded separately. The local kernel code confirms that a forced `GETCONNECTOR` probe calls `fill_modes` only for the current master ([kernel:3376](/home/ariel_santangelo/Projects/linux/drivers/gpu/drm/drm_connector.c:3376)). The production `IncarnationFdSet` absence and Legacy’s no-modeset reprobe were also confirmed ([fd set:110](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/executor/mod.rs:110), [reprobe:26541](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:26541)). Those three deliberate corrections are sound within the inspected ground.

Exact udev metadata on every connector sub-device and implementation details beyond the targeted paths remain unassessed, not judged sound. Compilation, tests, portability gates and hardware execution belong to implementation; none were run here.