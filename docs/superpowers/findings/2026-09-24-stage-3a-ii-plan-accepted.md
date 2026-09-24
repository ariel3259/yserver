# Stage 3a-ii — DPMS execution on the Owner: accepted

**Tip:** `1f252e52`. Plan revision 9
(`docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md`),
nine tasks in eight units, plus the stage 2b clock-probe addendum
(`docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md`,
revision 6, two tasks) that the hardware run made necessary. Implemented by
codex `gpt-6-luna` (xhigh); every task verified by the coordinator, who re-ran
the gate and re-applied at least one mutation by line per task.

## What exists now

The pure lifecycle layer of 3a-i now drives production on an Owner device:
a per-device, run-to-completion driver queue; a typed `Tier::Topology` that
carries its transition tag through validation, dispatch and result, with
freshness checks before `TEST_ONLY` and before submission; DPMS as an
`ACTIVE`-only lifecycle commit (MODE_ID and the primary plane untouched, every
turned-off CRTC in `ExpectedCompletionCrtcs` with a required out-fence) under
the C.0 revision-4 30 s bootstrap deadline; the `set_dpms_power` fork (Legacy
path byte-for-byte unchanged); while off, frames wait in admission and a
direct buffer stays reserved; blackout per target CRTC in both core Present
sweeps; every Owner-reachable read of `kms_outputs_active` routed per gate;
the failure edges (rejection, completion loss → `Poisoned`, logical-only DPMS
while poisoned, the seat observed read-only once the VT commits `Active`); and
the Legacy/Owner differential for backend state and client bytes.

The 2b addendum makes the Owner probe each CRTC clock through the executor at
one activation step every conductor installation goes through, promotes a
waiting probe at every slot release, makes a DPMS wait for its clock, routes
an uncertain probe to Table U, and stops reporting the owner's own refusals as
kernel rejections.

## Evidence

| Unit | Commit | Notes |
| --- | --- | --- |
| Tasks 1–2 driver + typed tier | `1ff55e10` | merged into one unit after two F8 stops (plan revs 5, 6) |
| Task 3 `ACTIVE`-only DPMS | `5d03cd12` | fixed a stage-2b defect: dispatch refused an unmeasured lifecycle cohort, against C.0 rev 4's bootstrap |
| Task 4 `set_dpms_power` fork | `11698781` | Legacy server-wide steps scoped per device |
| Task 5 while off / back on | `8a56a74b` | upstream timestamp fix interaction checked clean |
| Task 6 per-CRTC blackout | `814ad9de` | sent back once: a not-ready entry closed its window, breaking Legacy parity (D40) |
| Task 7 `kms_outputs_active` | `b128e49d` | two F8 stops (plan revs 7, 8): four topology reads are value-dead on Owner; composition must keep the off-screen scene |
| Task 8 failure edges + seat | `bfbbcbb6` | one F8 (rev 9: the advertised capability of item 39 does not exist before stage 4); sent back twice: the seat feed observed `Owned` before the VT was `Active`, and test assertions sat in the production VT loop |
| Task 9 differentials + hw test | `627846ea` | D33(a) re-applied unconditionally: caught by the absolute count |
| 2b addendum Task 1 | `8dd9fd19` | one F8 (rev 6: no production Owner activation site exists yet) |
| 2b addendum Task 2 | `3d2ab55b` | codex found and fixed its own wake-recursion stack overflow during the gate |
| hw test fixes | `21f66606`, `c0e054bb` | probe before compose; a held `RefCell` borrow; a bounded history length used as progress |

**Hardware (card1, RTX 5060 Ti, NVIDIA 615.71.09, kernel 7.2.6, tty2, user
present):** `c0_hw_3a_dpms_owner_on_card1_drm` passes **3/3** with
`nvidia-drm vblank=1` — the production `GET_SEQUENCE` probe resolves
`KernelSequence`; four `ACTIVE`-only off/on cycles, each off's out-fence
observed signalled with no later vblank, the retained framebuffer and
allocation unchanged, a composed frame admitted and completed after every on,
the fourth cycle as well as the first.

**Gate at the tip** (addendum Task 2, coordinator run): fmt; clippy
`-D warnings` in default, `tcp-transport`, `xdmcp`; `c0_2b_add_` 14,
`c0_3aii_` 36, `c0_3a_` 37, `c0_conv_ciii_` 39, `c0_conv_cii_` 26,
`c0_conv_cfb_` 35, `c0_conv_cp_` 34, `c0_adm` 129; `--lib -- --skip c0_2ci`
1873/0/217; `c0_2ci` 201 ×3 without a hang; `yserver-core` 1384; every
integration file green (`owner_completion_evidence` 43,
`owner_drain_and_wakeups` 8, `executor_async` 26, `executor_substrate` 6,
`executor_lock_handoff` 4, `owner_commit_record` 3, `wire_external_surface` 1,
`compile_fail` 3).

## What the hardware run found

The first real run failed for reasons no fixture could reach, each fixed in
its own commit and recorded:

1. **No Owner clock was ever probed in production** (stage 2b): the DPMS
   commit was refused by the owner and reported as a kernel `EINVAL`. `drm.debug`
   proved the kernel never saw it. Finding
   `2026-09-23-owner-clock-never-probed.md`; fixed by the 2b addendum.
2. **NVIDIA's default `nvidia-drm` has no DRM vblank:** `GET_SEQUENCE` returns
   `EOPNOTSUPP` unless `vblank=1` (parameter only on driver branch 610+, off by
   default; Maxwell/Pascal/Volta are on the legacy 580 branch and can never
   enable it). Finding `2026-09-23-nvidia-get-sequence-eopnotsupp.md`. **User
   decision (2026-09-24):** stage 5 becomes *activation by capability* and
   Legacy stays as the permanent fallback route (C.0 §18 and umbrella §6,
   `4d4b5396`); `docs/setup.md` documents `vblank=1` for Turing and newer.
3. **Three integration-test regressions hid behind a `--lib`-only gate**
   (`62838d58`): two tests kept the pre-rev-4 lifecycle contract; the handover
   test broke when `Device::for_tests` became `/dev/null` (bisected to
   `7762a560`). Every gate now runs each integration file.

## Carried

- **3b/3c:** call the output-reconciliation hook at their installation sites
  before lighting an output; route the four value-dead topology reads of
  `kms_outputs_active` when those paths first proceed on Owner; start a new
  epoch's clock probe at VT reacquire, hotplug, client modeset and
  administrative reprobe (C.0 §10).
- **3d:** the return to `Ready` — a `Deferred(ReadinessClosed)` DPMS parks until
  a newer request, since nothing in 3a produces that input.
- **Stage 4:** the cursor detach in the DPMS off commit; extend the item-39
  capability-stability test to the advertised values once they exist; keep
  Legacy cursor and gamma working for fallback devices.
- **Stage 5 (activation by capability):** characterize the mixed Owner/Legacy
  server (the Task 7 note on Legacy outputs off becomes an obligation); the
  scene-wakeup no-spin covers "every output off" only, so a multi-device
  transient is uncovered; the per-device "why not qualified" log.
- **Pre-existing, unchanged:** the `c0_2ci` intermittent parallel hang
  (`docs/known-issues.md`); `has_current_direct` is device-blind (latent while
  direct scanout is single-device).
