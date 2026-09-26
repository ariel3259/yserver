# Stage 3c design — codex review, round 4

**Target:** revision 4 (`773da90c`), fourth review (prior: round 3).

**Result:** 3 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24; connector-uevent device mapping and worker-fd registration unassessed). Round 3: three findings TRADED, one PARTIAL.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-26):** all four CONFIRMED (B-2 against `process_request.rs` publication rebuilding from backend state; B-3 against `run_suspend`s synchronous `dpms_set_outputs_active(false)`).

**User decision (2026-09-26):** the fixes kept trading one edge for another, so the edges are closed with simple rules in revision 5 and the design moves to the plans without a fifth round; plan review is the safety net. Rev 5: urgent withdrawal = own publication that only subtracts from the published projection (B-1, B-2); mixed-server synchronous Legacy calls are a named limitation (B-3); stuck probe worker keeps its fd in the incarnation fd set and fails the next probe fast (M-1).

## Review as received

## Verdict

**3 blocking, 1 major, 0 minor**  
Coverage: **INCOMPLETE**

This is a design review. It does not establish that the code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Round-3 finding | Status | Revision 4 |
| --- | --- | --- |
| B-1 — acquire probe failure aborts and continues | **TRADED** | The per-participant rule lets healthy devices reinstall, but an immediate failed-device publication conflicts with the promised single episode publication. See B-1. |
| B-2 — urgent withdrawal waits behind the gate | **TRADED** | Withdrawal now bypasses the FIFO, but rebuilding from the current backend model can expose another mutation before its gate publication. See B-2. |
| B-3 — Legacy suspend consumes the hand-off budget | **PARTIAL** | Owner dispatch and an absolute deadline are specified; capping waits leaves a synchronous Legacy KMS call on the core. See B-3. |
| M-1 — publication waits only for `Applied` | **TRADED** | §4.4 now uses terminal outcomes and covers partial timestamps, but the new urgent path can publish during an episode. See B-1. |

## Findings

### Blocking

**B-1 — Acquire failure requires both an immediate and a single publication.** [Plan §3.3:194–212](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:194) promises one publication after every acquire participant is terminal. [Plan §4.1:294–315](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:294) instead publishes a failed Owner probe’s withdrawal at once. If device A’s probe fails while B’s probe is outstanding, A must be published before B can reinstall and reach terminal outcome. The later B change needs another publication. Define urgent withdrawal as a separate publication and revise the episode rule and mixed-success test accordingly, or give the failed-probe withdrawal a terminal timing rule consistent with C.0’s logical-withdrawal requirement ([C.0:2015–2020](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2015)).

**B-2 — A full current-model withdrawal bypass leaks an in-flight mutation.** Revision 4 requires an urgent publication to bypass the gate and rebuild from the backend’s current model ([plan §4.1:294–309](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:294)). Device A’s modeset can already be promoted in the backend while its gate publication is pending; then removal of B produces a full-model publication containing **A’s new state** before A’s publication and reply. The core’s later publication rebuilds from backend state again ([3b §4.2:335–350](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:335), [source:5262–5295](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:5262)). This defeats the gate’s dispatch/publication order ([umbrella:294–297](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:294), [3b §7.5:746–765](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:746)). Specify a withdrawal-only update to the *published* projection that excludes A’s pending change, plus the explicit urgent-order exception needed by the authoritative gate contract. Test intermediate client-visible state, not only the final state.

**B-3 — The mixed release can still miss its absolute deadline.** The revision caps Legacy’s bounded waits, then relies on the core’s `next_wakeup` to hand off by one second ([plan §3.1:121–136](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:121)). Legacy suspend also executes `dpms_set_outputs_active(false)` synchronously on that core ([source:14876–14890](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:14876)). If that ioctl blocks past the deadline, the core cannot process the wakeup, drop master, or send `VT_RELDISP(1)`. C.0 requires VT release to proceed without waiting past the drain deadline ([C.0:2050](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2050)). Give the mixed path a hand-off mechanism independent of every potentially blocking Legacy call. A test that only makes the one-second event drain consume its budget cannot establish this property.

### Major

**M-1 — Probe expiry does not free the path for the required retry.** A probe runs on a worker with its own DRM fd; expiry invalidates its result, and a forced reprobe timeout arms a background rebuild ([plan §4.1:259–277](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:259), [plan §4.1:317–331](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:317)). If the worker remains inside `GETCONNECTOR`, invalidating its *result* neither frees that worker for the rebuild nor accounts for its fd during device retirement. State who owns and retires a timed-out worker and fd, and how a later probe runs while the old call remains blocked. C.0 requires tracked fd and lease ownership for an incarnation ([C.0:1919–1932](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1919)). The named timeout test needs to keep the real worker occupied; a scripted late reply alone can hide this failure.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** All four round-3 findings were compared with revision 4; none is claimed fully resolved above.
- **Architecture and safety:** Checked gate ownership, intermediate publication state, mixed release ordering, deadlines, and probe-worker handoff against targeted source and C.0 excerpts. Device-number mapping for connector uevents and detailed worker-fd registration remain unassessed.
- **Spec and evidence:** The named scenarios are directed through production entries by [plan §6:431–440](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:431), but their future fixture wiring was not checked individually. Stubs can establish ordering and disposition only if they preserve pending publications, blocked workers, and intermediate client reads; they cannot prove kernel `ACTIVE=0`, master hand-off, or actual udev delivery. Implementation inherits the format, clippy, test, and hardware gates in [umbrella §5.3:444–449](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:444). None was run.
- **Reading limit:** 24/24 spec/source selections used; one umbrella selection was 125 lines, exceeding the requested 120-line cap. Investigation stopped at the budget. The unassessed areas are not judged sound.