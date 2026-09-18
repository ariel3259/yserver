# Stage 2c-ii, plan A2 (the conductor) — accepted

**Plan:** `docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md`
revision 3, with the Task 4 correction (`c3adf03a`) and the release gate lines.
**Implemented by:** codex, model `gpt-5.6-luna`, reasoning effort `xhigh`,
`--sandbox workspace-write`, one task per run. The coordinating session
verified each task and committed it.

| Task | Commit | Sent back |
| --- | --- | --- |
| 1 — `begin_with_ledger`, split direct seam | `42a03c4e` | no (doc-comment placement fixed in task 2) |
| 2 — conductor, activation, intents, snapshot | `ab50c13e` | no |
| 3 — dispatch, confirm at the send, refusals | `da6a1fa6` | once: F-T3-1 side effect inside `debug_assert!` (release build never moved the frame), F-T3-2 panic on a consume error, F-T3-3 invented `ResourceService` |
| 4 — retirement ordering | `29193f4a` | once: F-T4-1 retired frame's pins released while on screen — **the plan's error**, followed literally |
| 5 — layout invalidation | `dbed3b57` | no |

## Mutations (coordinator, against the implemented code)

Each applied by exact line or unique anchor, one at a time, the run
confirmed compiled, the file restored from memory.

| Mutation | Caught by (failing tests) |
| --- | --- |
| N1 skip the `Owner` check | 1 (`is_inert_without_an_owner_transport`) |
| N2 ignore `OrdinaryRetirement` occupancy | 1 |
| N3 composed `Ready` without asking the source | 1 |
| N4 skip `set_direct_successor` on offer | 18 |
| N5 undo cancels the charge instead of restoring `Successor` | 2 |
| N6 confirm on a refusal | 4 |
| N7 drop the refusal's events | 5 |
| N8 leave a refused direct successor queued | 4 |
| N9 no `SlotBusy` check | 1 |
| N10 ignore a `lock` error | 1 |
| N11 admit before enqueueing | 1 (the operation trace) |
| N12 drain `completed` inside the handler | 4 |
| N13 layout bump without withdrawal | 3 |
| N14 call the builder before the slot reservation | **type-enforced**: on refusal `begin_with_ledger` returns the `FnOnce` builder by value, so it cannot have been called; not expressible as a compiling mutation |
| N15 unflip request leaves the frame | 1 |
| N16 eligibility forced true | 2 |
| N17 ignore `ResourcesStillCurrent` | 1 |
| N18 no abort on a preparation failure | 1 |
| N19 skip the end-of-entry-point publication | 7 |
| N20 fetch `composed_resources` before `begin` | 1 |

## Gate (coordinator, outside the sandbox)

`cargo +nightly fmt --check`; clippy default, `tcp-transport`, `xdmcp`
clean; `cargo check --workspace` gnu/musl/freebsd clean; `c0_adm` 64/0
five times in debug and once in release (after building a fresh
`target/release/yserver`); `c0_2ci` 180/0/21; full `--lib` 1842 passed,
0 failed, twice.

## What the round taught

- **Release matters.** A state change inside `debug_assert!` passed every
  debug run. The plan gate now includes a release run; so should plan B's.
- **A stale release helper binary** (`target/release/yserver` from
  2026-08-31) made the reaped-helper fixture time out at 5 s; the implementer
  blamed its sandbox. Build it before the release run.
- **The implementer's sandbox cannot run the full `--lib`** reliably
  (helper, device-lock and socket tests fail or hang there). The coordinator's
  run outside it is the evidence.
- **A plan error is followed literally.** Task 4's pin wording contradicted
  the legacy code it cited; the invariant should have been checked against
  that code before the plan went out.

## Limits carried to 2c-iii (unchanged from the plan)

Real `CommitDescription` builders and producer readiness; the real direct
eligibility predicate (extracted from `try_present_direct`); real
layout-change hook sites; F13b-D1's lease adoption; ready-unflip dispatch
(needs a retained composed framebuffer); multi-device conductor state.
