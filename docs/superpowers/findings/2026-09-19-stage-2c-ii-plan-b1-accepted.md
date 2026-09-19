# Stage 2c-ii, plan B1 (maintenance in the decider) — accepted

**Plan:** `docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md`
revision 3, with three corrections made during implementation (below).
**Implemented by:** codex, model `gpt-5.6-luna`, reasoning effort `xhigh`,
`--sandbox workspace-write`, one task per run; the coordinating session
verified each task outside the sandbox and committed it.

| Task | Commit | Sent back |
| --- | --- | --- |
| 1 — maintenance storage, tickets, rejection re-entry, cursor barrier | `7f37f5e2` | once: F-B1T1-1, every replacement while submitted allocated a new ticket |
| 2 — decision shape, tiers 2/4/7, symmetric absorption, ageing on confirm | `5259b644` | once: F-B1T2-1, tier 7 evaluated before tier 6 — to satisfy a plan test whose scenario (a tier-7 winner combining a primary) is unreachable; **plan error**, fixed in `bd9abe18` |
| 3 — tier 3, absorption into primaries, tier 5 | `63d7a725` | once: F-B1T3-1, A2's retirement promotion switched off and its evidence rewritten, following the plan's constraint that A2 abort every tier-3 decision; **plan error**, fixed before re-dispatch |
| 4 — the checked bound | `3d3f8174` | no |
| mutation survivor P20 | `(this commit's parent)` | the A2 guard test bypassed the guard; fixed with one shared guard/dispatch step |

## Mutations (coordinator, against the implemented code)

Applied by unique anchor, one at a time, each run confirmed compiled, files
restored from memory. P18 duplicates P7 (the direct-stream case) and was not
run separately.

| Mutation | Failing tests |
| --- | --- |
| P1 reset the ticket on replacement | 2 |
| P2 reuse the consumed ticket | 1 |
| P3 queue the current generation | 1 |
| P4 skip ageing on loss | 4 |
| P5 a barrier resets relative age | 4 |
| P6 bundle before aged maintenance | 1 |
| P7 tier 3 ignores unabsorbable aged maintenance | 1 |
| P8 combine an incompatible primary | 1 |
| P9 composed on a barrier CRTC competes | 3 |
| P10 absorb an incompatible generation | 1 |
| P11 drop one ready bundle member | 1 |
| P12 skip the round-robin in tier 5 | 1 |
| P13 a collision resets the count | 2 |
| P14 count an unknown | 1 |
| P15 a primary overtakes a cursor recovery | 1 |
| P16 barriers count toward the bound | 2 |
| P17 never report a violation | 2 |
| P19 tier 3 on ordinary wakes | 4 |
| P20 A2 dispatches carried maintenance | 2 (after the fix; survived before) |
| P21 symmetric absorption carries only the winner | 1 |
| P22 absorb a `Waiting` generation | 1 |
| P23 reset the count on drop | 3 |
| P24 freeze the allowance | 2 |
| P25 skip the increment in `confirm` | 2 |
| P26 `Waiting` aged identities block tier 3 | 2 |
| P27 a successor over stale required maintenance | 1 |
| P28 `confirm` keeps the combined primary | 1 |
| P29 no readiness guard in tier 4 | 4 |
| P30 no readiness guard on the cursor recovery | 2 |

## Gate (coordinator, outside the sandbox)

fmt; clippy default, `tcp-transport`, `xdmcp` clean; `cargo check --workspace`
gnu/musl/freebsd clean; `c0_adm` 106/0 three times in debug and once in
release; `c0_2ci` 180/0/21; full `--lib` 1884 passed, 0 failed.

## What this round taught

- **Two of three sent-back tasks were the plan's errors**, both followed
  literally by the implementer: an unreachable test scenario (tier-7
  combination) and a constraint wider than its intent (abort every tier 3).
  Each fix of a plan rule should be checked against the tier order and against
  the earlier plans' evidence it could disable.
- **A test-only hook can make a test vacuous.** The mutation run is what caught
  it: the hook bypassed the very guard the test was named after.
- **codex asks for approval in non-interactive runs** when a finding invites a
  design choice; pre-approve the design in the prompt.
