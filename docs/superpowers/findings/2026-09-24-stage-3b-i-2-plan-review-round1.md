# Stage 3b-i-2 plan — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-2-plan-modeset-routes.md`
revision 1, first review.

**Result:** 0 blocking, 3 major, 0 minor; coverage INCOMPLETE.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):** M-1, M-2 and M-3 CONFIRMED and applied in
plan revision 2 (kept-output repaint on a root storage change, a request-owned
direct hold released on every end, a KMS-submitted copied frame test). No
blocking finding: the plan is ready once 3b-i-1 is accepted.

## Verdict

**0 blocking, 3 major, 0 minor**  
Coverage: **INCOMPLETE**

This is a design review, not a claim that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior review | Status |
| --- | --- |
| None | First review; check 1 skipped. |

## Findings

### Blocking

None.

### Major

**M-1 — Kept outputs lack a root-change repaint contract.** The spec requires outputs on other devices to receive full damage and repaint when root storage identity changes ([spec §5.2, lines 501–506](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:501)). Task 3 names full damage for the moved output while updating the root extent; Task 4 keeps Owner scene states during a Legacy modeset. Neither states how those kept outputs are damaged ([plan lines 107–116](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-2-plan-modeset-routes.md:107), [127–140](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-2-plan-modeset-routes.md:127)). If moving A grows and reallocates the root while B has a composed flip in flight, B can finish that flip and retain its old damage history without a repaint against the new root. Specify full damage and a class-2 repaint for every kept output when root storage identity changes, and test that sequence without a lifecycle commit on B.

**M-2 — The direct-entry hold has no failure-terminal release.** Task 1 holds direct re-entry until modeset promotion, yet a rejected unflip or a superseding event ends the request without promotion ([plan lines 59–76](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-2-plan-modeset-routes.md:59); [spec §3.2, lines 156–165](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:156)). The current request helper sets `unflip_requested` and clears `hold_direct` ([backend.rs:2709](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2709)). For example, an unflip can retire, then `TEST_ONLY` can reject the modeset: the old topology remains authoritative, but the stated hold has no promotion to release it. Define who clears or transfers the hold on every pre-dispatch terminal path, after any pending unflip is safe. Test a failed or superseded request followed by a direct-eligible Present.

**M-3 — Copied-retirement tests omit the KMS-submitted stage.** The spec’s last table row retains a B-submitted frame through the displacing commit’s `KmsRelease` and the FOREIGN return ([spec §5.1, lines 481–490](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:481)). Task 2 promises that rule, but its staged test lists only A in flight through B completed *without* KMS submission; the basic copied mode-change test asserts route and resulting state ([plan lines 87–103](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-2-plan-modeset-routes.md:87)). Destroying a submitted frame’s pool after B’s fence could therefore pass the named evidence while KMS or FOREIGN still owns it. Add a submitted-frame case that delays each proof separately and asserts retention until both arrive.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** no prior review.
- **Architecture:** checked the plan against the spec’s transaction, per-output rebuild, direct, copied, and mixed Legacy contracts. Targeted source excerpts confirmed the current direct request, Legacy all-off path, scoped DPMS precedent, and copied-completion routing.
- **Safety and verification:** checked the named failure and ordering scenarios against those contracts. The plan assigns formatting, lint, test, and integration gates to implementation; none were run.
- **Reading limit:** **24/24 bounded excerpts** used. The full event-loop handoff, 3b-i-1 promotion implementation, and mixed Legacy recovery path remain unassessed. In particular, follow-up should establish whether the base promotion already provides the root-change damage behavior required by M-1. Unassessed code is not treated as sound; compiler and test results remain for implementation.