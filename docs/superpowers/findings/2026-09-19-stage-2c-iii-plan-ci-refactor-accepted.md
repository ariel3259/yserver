# Stage 2c-iii, plan Ci-refactor — mutation parity

**Plan:** `docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md`
revision 4, after two codex review rounds (2B 2M, then 1B 1M).
**Implemented by:** codex, model `gpt-5.6-luna`, reasoning effort `xhigh`, run
**without sandbox** (user-authorized, so it could run the `_vulkan` tests itself);
the coordinator verified each task outside it and committed.

| Task | Commit | Sent back |
| --- | --- | --- |
| 1 — the `OwnerBuffer` type and its transitions | `39a68bac` | no |
| 2 — the scene and platform on `OwnerBuffer` | `e82f632f` | no (interrupted mid-run by a reboot; finished in a second run) |
| 3 — one dispatch failure path, one `begin` body | `bd429355` | no |
| 4 — collapse the unreachable failure rows | `2bc520a0` | twice: F-T4R-1 (a reachable row split by the undo result), F-T4R-2 (the unreachability claim missed `DirectCapacity::reserve` refusing on a closed admission, `capacity.rs:136`) |

Task 4's authority: spec §8.3 was amended first (`a1adc3b1`, user's decision),
because the section defined this plan as behaviour-preserving.

## Mutation parity (coordinator, re-applied by line at `2bc520a0`)

Every mutation Ci's acceptance recorded as *caught*, re-applied on the refactored
code. The map of old → new sites was produced by a **read-only** codex run and
then checked by the coordinator while applying it; three of its rows were wrong
and were corrected (below).

| Mutation | Result at `2bc520a0` |
| --- | --- |
| R1, R2, R4, R5, R6, R7, R27, R28, dormancy (owner route, legacy route) | caught — deterministic tests |
| R9, R10, R12, R14, R15, R16, R17, R21, R22, R24, R29, R30, R31 (quarantine, mechanism failure, topology), R33, F-T4-1, never free a displaced buffer, F-T5-2, F-T5-3, F-T5-4, F-T5-5, no quarantine on unknown | caught — `_vulkan` tests on the real GPU |
| **R19** (free a buffer that is not `Releasing`) | **caught** — it was *equivalent* under Ci, where `BoState` refused it; the phase guard is gone, and `OwnerBuffer`'s transition now refuses it, failing `c0_conv_cir_owner_buffer_refuses_illegal_transitions` |
| R23, R25, R26 | carried with their Ci status: applied and reverted by codex on the GPU during Ci, not re-applied here |
| R3, R32, R8, R11, R13, R18, R20, R34, F-T5-1 | carried unchanged (equivalent, surviving-by-design, structural, or proven at the resource service) |

**Three corrections to the map, made while applying it:**

- **R2** pointed at `device.rs:1378`, the adapter of the *infallible* entry; the
  test drives the fallible one (`:1471`). At the right site it is caught.
- **R14** and **R15** had been restated ("stage at `Dispatched`", "add a
  `Presented` arm"); Ci recorded them as "never stage at `Accepted`" and "apply
  at `Presented` **instead of** `HardwareComplete`". As recorded, both are caught.
  The restated R15 does survive, for the reason the map gave: the transaction is
  removed at `HardwareComplete`, so a second apply cannot occur.
- **R12** pointed at the render-completion handler, which has no `store` and
  cannot ack; the ack site is the tick's displacement. There it is caught.

## Checks at `2bc520a0` (coordinator, outside codex, GPU idle)

fmt; clippy default, `tcp-transport`, `xdmcp`; `c0_conv_ci_` 38/38 with
`--include-ignored` in debug and release (Ci's baseline, unchanged);
`c0_conv_cir_` 7/7 in both; `c0_adm` 129/0; `c0_2ci` 180/0/21; `--lib`
1926/0/123 (Ci's 1921 plus this plan's five new tests).

## Hardware gate — 2026-09-19, ACCEPTED

Run by the coordinator with the user's go-ahead and the GPU free (no GPU
processes before or after): `render_acceptance -- --ignored` **164/164**;
`c0_2ci -- --ignored` **21/21**; the library's other ignored tests
(`--ignored --skip c0_2ci`) **102/102**. **287/287 in total**, against Ci's
285/285 plus this plan's two new `_vulkan` tests. Nothing was skipped and
nothing failed.

**Plan Ci-refactor is accepted**: behaviour unchanged where production can
observe it, the collapsed rows authorized by the amended §8.3, every Ci
mutation still caught (R19 now caught rather than equivalent), and the
hardware gate green.
