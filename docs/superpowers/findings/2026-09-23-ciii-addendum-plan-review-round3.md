# 2c-iii addendum plan — codex review, round 3

**Target:** revision 3 (`cd635368`). **Result:** 0 blocking, 2 major, 0 minor;
coverage INCOMPLETE (24/24). Incorporation: all four round-2 findings APPLIED.
Trend: r1 1B 2M, r2 1B 3M, r3 0B 2M.

**Reviewer:** `codex exec --sandbox read-only`, single pass; instrument
`docs/superpowers/review/` @ `0245f96b`; `gpt-6-sol` `xhigh`; `codex-cli 0.155.1`.

**Author verification (2026-09-23):** both CONFIRMED by reading revision 3 —
the gate's `c0_conv_ --include-ignored` is broader than the header's permitted
families (M-1), and the every-output clause could finish unproved (M-2). Fixed
in revision 4. The review also confirmed the rewrite's premise: refusal returns
before `hold_direct` (`backend.rs:22794`) and the core then takes Present Copy
(`process_request.rs:10522`).

---

## Verdict

**0 blocking, 2 major, 0 minor**  
**Coverage: INCOMPLETE**

This is a design review. It does not establish that the implementation compiles, passes tests, or is approved.

## Incorporation audit

| Round-2 finding | Status | Revision 3 |
| --- | --- | --- |
| B-1 — a prepared direct frame prevents the composition it awaits | **APPLIED** | The gate moves before direct preparation ([plan:14](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:14>)). Refusal returns before `hold_direct` is set ([backend.rs:22794](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:22794>)); the core then takes Present Copy ([process_request.rs:10522](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:10522>)). |
| M-1 — bootstrap requests restart with replacement offers | **APPLIED** | The bootstrap request and generation counter are removed ([plan:14](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:14>)). |
| M-2 — device scoping loses the global retirement-capacity guard | **APPLIED** | `has_current_direct` remains global ([plan:70](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:70>)); current topology permits direct on the primary device only ([backend.rs:3877](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:3877>)). |
| M-3 — no named test for the one-return-present intermediate state | **APPLIED** | The plan names a two-output test and its first-output-only mutation ([plan:110](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:110>)). Its optional exit remains a separate finding below. |

## Findings

### Blocking

None.

### Major

**M-1 — The test gate selects ignored tests outside its allowed filters.** The plan permits ignored tests only under five named filters ([plan:3](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:3>)), but its gate runs `c0_conv_` with `--include-ignored` ([plan:128](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:128>)). An implementer following that command selects any ignored `c0_conv_` test outside the permitted families, defeating the stated execution boundary. Replace the broad ignored-test command with separate commands for the permitted filters; run the ordinary library suite separately.

**M-2 — The every-output return requirement may finish without proof.** The unflip requires a retained composed framebuffer on *every affected output* ([spec:481](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:481>)). The plan makes the two-output test an exit criterion, then allows it to be unwritten and its mutation unrun if the fixture cannot create the intermediate state ([plan:102](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:102>), [plan:118](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:118>)). The other entry tests cannot distinguish “all outputs” from “first output.” Thus an implementation could report the central clause unproved while completing the task; the spec calls for named tests and mutations that break exit criteria ([spec:576](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:576>)). Make inability to establish that state an F8 stop, or specify another production-entry proof before treating the task as complete.

### Minor

None.

## Coverage and implementation checks

**24/24 excerpts used.** One excerpt printed 126 lines, exceeding the 120-line limit; coverage is therefore **incomplete**. I stopped investigating at the budget.

Architecture: the shared predicate feeds both Present entry and successor rechecks; an ineligible queued successor is withdrawn ([admission.rs:903](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:903>), [admission.rs:1022](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1022>)). Safety and ownership: with no direct unit active, `request_direct_unflip` returns without setting a hold ([backend.rs:2633](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2633>)); the ineligible path resets entry probation, whose counter does not gate scene composition ([backend.rs:1002](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:1002>), [backend.rs:22807](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:22807>)). Spec and verification: the shared all-output predicate was located, but the complete production Copy → composed admission → retirement sequence was not traced. Its behavior under a continuing Present stream remains unassessed, not established as sound.

Formatting, clippy, tests, fixture reachability, and mutation results belong to implementation. None were run here.
