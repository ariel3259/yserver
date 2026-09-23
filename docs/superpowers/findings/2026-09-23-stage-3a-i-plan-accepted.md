# Stage 3a-i — the pure lifecycle arbiter: accepted

**Tip:** `64d0bd68`. Plan revision 8
(`docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md`),
five tasks, `97e355aa` through `64d0bd68`. Implemented by codex `gpt-6-luna`
(xhigh; send-backs at max); every task verified by the coordinator, who
re-ran the gate and re-applied at least one mutation by line per task.

## What exists now

The lifecycle decision layer of stage 3, pure, under `kms/owner/lifecycle/`
(no import of `kms::render`, `kms::executor`, `drm` or `platform`, enforced by
a path-resolving source scan): identities and value types; the per-device
`LifecycleDesired` snapshot with C.0 `REC-5` coalescing and dispositions; the
two recovery tables — C.0 §10's "transition that encountered
`CompletionUnknown`" by active row (Table U) and `REC-6` (Table F), plus the
fate after `RecoveryFailed` from C.0's own §6.4 row; the per-device arbiter
(`REC-4` precedence, supersession with logical obligations at once and physical
advancement after the winner's own receipts, convergence, the item-65 epoch
rules, recovery-attempt outcomes); and the coordinator (global state, the only
event-id allocator, DPMS projection and aggregation). It has **no production
caller**, by design: plan 3a-ii wires it in.

## Evidence

| Task | Commit | Tests (`c0_3a_`) | Notes |
| --- | --- | --- | --- |
| 1 identities and types | `97e355aa`, `c08ed15b` | 4 + compile-fail | sent back once: the purity scan looked for non-existent paths (`crate::kms::drm`); a later fix stopped the compile-fail test littering the tree |
| 2 `REC-5` | `caf1ec0e` | 9 | — |
| 3 two recovery tables | `9676c850` | 18 | two F8 stops on real plan gaps (four boundaries, no `REC-6` row after `RecoveryFailed`) |
| 4 arbiter | `705d59c6` | 29 | sent back once: a factorial mixed-arrival test took the library suite from 2 s to 129 s; now 0.01 s with an order-dependence mutation caught |
| 5 coordinator | `64d0bd68` | 37 | one F8 stop: the arbiter had no recovery-attempt input (plan revision 8) |

Gate at the tip: `cargo +nightly fmt`; clippy `-D warnings` in default,
`tcp-transport` and `xdmcp`; `c0_3a_` 37 in 0.02 s; `compile_fail` 2;
`--lib` 1998/0/226 in 2.01 s. No coredumps.

## Process record

The plan took five codex review rounds (2B 4M 1m, 4B 1M, 3B 1M, 1B 1M, 1B)
and four implementer F8 stops. Two of the review rounds and all four F8s traced
to the same cause: the plan asserted "every row" of C.0 tables without checking
which rows C.0 defines. Two rewrites (Task 3 around both normative tables; R4-2a
as logical-now/physical-after-receipts) ended the trading. Two coordinator
process slips are recorded in memory: a `;`-chained commit that shipped only a
findings file (amended), and an unquoted heredoc that executed a backticked
command into a prompt.

## Carried to 3a-ii

- The DPMS level→target mapping exists in three places (two in the
  coordinator, one in the arbiter), each covered by a mutation; unify it when
  production consumes it.
- Everything the 3a design assigns to execution: the driver, typed
  `Tier::Topology` with pre-submit freshness checks, the `ACTIVE`-only DPMS
  commit, the Legacy/Owner fork of `set_dpms_power`, blackout per CRTC in both
  core sweeps, the `kms_outputs_active` inventory, the differential gate and the
  card1 hardware run.
