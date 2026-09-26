# Stage 3c-i plan — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md` revision 1, first review.

**Result:** 2 blocking, 2 major, 0 minor; coverage INCOMPLETE (24/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-26):** all four CONFIRMED.

- **B-1** — `render/platform.rs` `probe_connector_snapshot` returns one `io::Result` and stops at the first error. Rev 2 adds a per-device probe entry (Task 5).
- **B-2** — rev 1 dispatched and timed out release commits before the unknown-outcome closure existed, and scoped the mixed server only in Task 9. Rev 2: scoping in Task 1, urgent withdrawal as Task 2, commit + hand-off + outcomes + budget co-delivered as Task 3; 8 tasks.
- **M-1** — no owner for the acquire episode barrier. Rev 2: `AcquireEpisode` (Task 6) with its test.
- **M-2** — the core-entry driver builds a fresh `ServerState` and has no gate. Rev 2: urgent-withdrawal gate tests at the yserver-core layer (`c0_3bii_` core-loop harness) plus a backend emitter test.

## Review as received

## Verdict

**2 blocking, 2 major, 0 minor**  
**Coverage: INCOMPLETE**

This is a design review. It does not establish that the plan compiles, passes tests, or is approved for execution.

## Incorporation audit

| Prior findings | Status |
| --- | --- |
| None | First review; check 1 skipped. |

## Findings

### Blocking

**B-1 — The acquire probe cannot produce the required per-device dispositions.**  
The plan retains the existing synchronous combined `probe_connector_snapshot` but requires one failed Owner probe to withdraw only that device while healthy Owners reinstall ([plan:48–50, 164–179, 199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:48); [spec:209–228](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:209)). Today that probe returns one `io::Result<Vec<_>>` and stops at the first error ([platform.rs:5037–5057](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:5037)). If device B fails, its result discards A’s successful snapshot and leaves later devices unprobed. Task 6 needs a synchronous *per-device* result, keyed by device and collected across all participants, before Task 7 can implement mixed success. This does not require 3c-ii’s asynchronous worker.

**B-2 — The task boundaries expose an unsafe release before unknown-outcome handling exists.**  
Task 3 requires a deadline hand-off even when the executor never answers; Task 4, later, supplies the dead-epoch closure, fd retention, quarantine, and withdrawal required for that outcome ([plan:109–145](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:109); [spec:148–171](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:148)). At the end of Task 3 alone, a release can time out, drop master, and acknowledge the VT while the old executor remains unresolved; a subsequent acquire can still reach the existing resume route ([backend.rs:25463–25464](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:25463)). That conflicts with the plan’s per-task green gate and hardware rule. Co-deliver the timeout hand-off and unknown-outcome closure as one independently safe task. The mixed-server release route likewise needs its scoped Legacy suspend before that route is considered ready, rather than first receiving it in Task 9 ([plan:215–228](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:215)).

### Major

**M-1 — No owner is specified for the acquire episode’s terminal barrier.**  
The plan calls for “one publication for the acquire episode” at `Applied`, but assigns no component to track all participants, hold the gate turn, and publish only after every remaining participant is terminal ([plan:182–200](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:182)). The spec requires precisely that barrier, while a failed Owner probe withdraws separately and leaves the episode ([spec:225–228, 305–318, 340–341](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:225)). With two healthy Owners, A can reach `Applied` while B is still in flight; publishing A then exposes a partial episode. Define the episode owner, participant terminal accounting, gate lifetime, and single publication trigger. Add a test that holds B in flight after A applies.

**M-2 — The named gate-bypass tests lack a gate-bearing test path.**  
Task 4 requires a client read between publications and a withdrawal while an unrelated mutation holds `RandrMutationGate` ([plan:141–146](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:141)). The prescribed backend driver creates a fresh `ServerState` for each run and drives backend entries without a retained gate ([backend.rs:65973–65984](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:65973)); the production gate owns the in-flight mutation and queues requester-less publications ([run.rs:715–719, 1720–1746](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:715)). A backend-only test could observe a withdrawal request yet miss the gate ordering or client-visible projection defect. Specify an existing core-loop gate test path that retains the mutation, state, and client reads, alongside the backend lifecycle test; keep executor replies on the prescribed driver path.

## Coverage and implementation checks

- **Check 1:** No prior review.
- **Checks 2–3:** Verified the production VT entry, combined probe, lifecycle action handling, requester-less queue, and core driver. Exact fd-alias retirement ownership and later publication filtering remain unassessed.
- **Check 4:** Compared the in-scope VT and urgent-withdrawal requirements. The plan assigns formatting, regular clippy in three configurations, suites, and coordinator-run hardware checks; their results belong to implementation.
- **Reading:** 24/24 spec and source excerpts used. One source read inadvertently exceeded the 120-line excerpt limit; investigation stopped at the budget. A targeted follow-up, if needed, should examine only the unknown-release fd/alias barrier and whether later queued publications can restore a withdrawn device.