> **Author verification (coordinator, 2026-09-23):** B-1 CONFIRMED — rev 3's
> "discarded as stale" and I-1a contradicted each other for an old probe's
> timeout; rev 4 limits "stale" to results. M-1 CONFIRMED — `acquire_probe`
> also fails while a validation holds the slot (`owner/slot.rs` ~150); rev 4
> makes every slot release a promotion point, with a validation test.

## Verdict

**1 blocking, 1 major, 0 minor.** Coverage: **COMPLETE FOR DECLARED SCOPE**. This is a design review, not a claim that the implementation compiles, passes tests, or is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| Round 2 B-1 — timed-out probe needs an executor lease barrier | **APPLIED** | The plan assigns termination and reap to the generic executor path, keeps the owner’s probe slot held, separates `ReapProof` from logical `Poisoned`, and requires the timeout test to observe both ([plan, lines 58–79](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:58), [166–172](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:166)). The stale-epoch conflict below is a separate unresolved case. |
| Round 1 M-1 — clock absent on first activation | **APPLIED** | Every served Owner CRTC must have a record without a RANDR query; a missing record cannot yield a partial clock map ([plan, lines 81–95](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:81)). |
| Round 1 M-2 — probe loses the slot while waiting | **APPLIED** | The plan retains unscheduled keys and gives them priority when the slot frees, including across epoch replacement ([plan, lines 97–108](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:97)). Evidence for all slot-release paths remains incomplete, as M-1 explains. |

## Findings

### Blocking

**B-1 — A stale probe timeout has contradictory disposition.** The plan says an in-flight result for a superseded clock epoch is “discarded as stale,” while *any* uncertain probe must take the completion-loss route to logical `Poisoned` ([plan, lines 58–79](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:58), [97–108](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:97)). Its epoch-replacement test covers only an old reply, not an old timeout ([plan, line 170](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:170)).

A probe can be in flight when a new epoch replaces its clock, then time out. The executor correctly enters `Stalled` and requests termination ([executor, lines 723–740](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/executor/mod.rs:723)); discarding the resulting `Unknown` event as stale would suppress the lifecycle handoff while the probe slot stays held ([owner, lines 2254–2261](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:2254)). C.0 requires timeout to follow `COMMIT-5`/`ExecutorStalled` even as stale clock *results* are discarded ([spec, lines 1769–1784](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1769)). State explicitly that stale success or rejection cannot resolve the new clock, but an uncertain old host call still triggers the executor and logical failure handoffs. Add the stale-timeout sequence to the epoch-replacement test.

### Major

**M-1 — The occupied-slot test misses validation release.** `acquire_probe` can fail because a validation holds the slot, as well as because a composed commit does ([slot, lines 131–164](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/slot.rs:131)). The plan requires promotion at *every* slot release before new commit work, but its only occupied-slot test releases a composed commit ([plan, lines 97–105](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:97), [line 169](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:169)). The current event batch drains lifecycle work before its ordinary admission wake ([backend, lines 21242–21288](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:21242)), so a composed-retirement test cannot establish priority after validation releases the slot.

If a clock is installed during validation, then validation resolves while another lifecycle action is queued, the waiting probe needs an opportunity to claim the slot before that action begins. Name that release boundary in Task 1 and test validation resolution with a waiting probe and queued successor.

### Minor

None.

## Coverage and implementation checks

All four checks were performed. Incorporation was checked against the revised task text; architecture and failure ordering were checked against the owner slot, executor, lifecycle driver, and event batch; the findings were compared with C.0 §10 and stage 3a §§3.3 and 3.7; and the named tests and implementation gate were assessed. **24/24 bounded spec/source excerpts** were used. No build, test, benchmark, or review script was run.

The exact production setup site, complete installer inventory, and every slot-release path were not verified within this reading limit; they are **unassessed**, not deemed sound. The plan assigns formatting, Clippy, build, portability, and test checks to implementation.