# Stage 3b — client modeset and the RANDR protocol on the Owner: accepted

**Tip:** `68cffa47` (pushed to `fork/feat/phase-c0-atomic-kms-migration`).
Design `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`
revision 12 (seven design review rounds, then plan-driven revisions 9–12).
Three plans:

- 3b-i-1 modeset execution, revision 11 (nine tasks + hardware addenda A1–A3);
- 3b-i-2 modeset routes, revision 5 (Tasks 1, 2, H, 3, 4, 5);
- 3b-ii RANDR protocol, revision 9 (five tasks; review loop closed at round 7).

Implemented by codex `gpt-6-luna` (xhigh; max on send-backs); every task
verified by the coordinator, who re-ran the suites itself (codex twice reported
a suite green that was not) and spot-checked mutations by line.

## What exists now

A client `SetCrtcConfig` on an Owner device is one atomic `ALLOW_MODESET`
transaction per device (no Legacy all-off): it takes the per-device
client-modeset slot on `Tier::Topology`, is validated with `TEST_ONLY`, keeps
the old topology lit while the new pools are prepared, and promotes
infallibly once the kernel accepts it. Output state is keyed by identity
(`OutputInstanceId`); the scene is rebuilt per device; a retired output keeps
its scene state and pool in a bundle until its KMS proof arrives, including
the dark-CRTC displacement proof. A modeset under DPMS-off installs
`ACTIVE=0`. Failures are typed (`ClientModesetFailure`, one log line each),
`EBUSY` is never retried, and errnos are classified. `REC-4` events supersede
a modeset before dispatch. A direct frame unflips through the ordinary Ciii
unflip before the modeset dispatches; the copied route and position-only
changes (a logical transaction with no KMS call, moving the output in place)
are covered; a Legacy modeset in a mixed server is scoped to the Legacy
devices, and a Legacy-only server (all of production today) is unchanged.

On the protocol side, `RandrMutationGate` serializes RANDR mutations
server-wide (FIFO; queries never wait), with the queue deadline `Q` = 30 s,
publication that outlives its requester, ordered requester-less
publications, and reset/terminate/XDMCP waits for an install-capable
mutation.

## Evidence

| Unit | Commits |
| --- | --- |
| 3b-i-1 Tasks 1–9 | `8d3b4c51`, `ba83e54f`, `a8508efe`, `65dc5f38`, `d3102297`, `6b66b221`, `a4799d80`, `91da07b0`, `c0309f88` |
| 3b-i-1 addenda (found on hardware) | A1 `bba09dd9` retired bundles serviced without composition; A2 `eb85bfa1` + `079d1b7e` bundles release their pools, a disable clears current resources, the retired front BO leaves `OnScreen`; A3 `94c00628` never-submitted composed work of a retired output is terminalized (withdrawal keyed by generation **and** output instance) |
| 3b-i-2 Task 1 unflip, Task 2 copied route | `fe59cbc3`, `7651636b` |
| 3b-i-2 Task H end-state check + core-entry driver | `79af64df`, WIPs `44e6749d`, `cdd64e98`, `f0300653`, `dede8405`; closed by user decision (plan rev 4) |
| 3b-i-2 Task 3 position-only | `c39479fd` |
| 3b-i-2 Task 4 mixed server | `f3ab490c` |
| 3b-i-2 Task 5 two devices, coverage, hw additions | `e9cfcbef` |
| 3b-ii Tasks 1–5 | `6afd252f`, `c4c0ff06`, `47cd42b2`, `5f802204`, `68cffa47` |

Product defects found by Task H and the merges, beyond the addenda:
`index_shift` — `CompletionRetired` missed owner buffers held by retired
bundles and kept resource groups current for an absent CRTC (`dede8405`);
`retire_owner_current` starved releasable buffers behind one blocked on its
`KmsRelease` (found by the `ad71461c` merge, fixed in `d069a29c`).

**Hardware (card1, RTX 5060 Ti, NVIDIA 615.71.09, kernel 7.2.6,
`nvidia-drm vblank=Y`, run from tty2):** `c0_hw_3b_modeset_owner_on_card1_drm`
passes **3/3** at the tip — advertised mode and back ×4, disable/enable ×4,
DPMS-off/mode/on ×4, position change ×4, each lit step composed and each
displaced pool discharged; two `SetCrtcConfig` requests through the core with
their reply and Screen/CRTC/Output event bytes checked. The second-device step
skips with its reason logged (card0 amdgpu has no lit output).

**Gate at the tip:** fmt; clippy `-D warnings` in default, `tcp-transport`,
`xdmcp`; `c0_3bi_` 76, `c0_3bii_` 5, `c0_3aii_` 36, `c0_3a_` 37, `c0_2b_add_`
14, `c0_conv_ciii_` 39, `c0_conv_cii_` 26, `c0_conv_cfb_` 35, `c0_conv_cp_` 34,
`c0_adm` 127, `c0_merge_` 1; `--lib -- --skip c0_2ci` 1935/0/287;
`c0_2ci` 172; `yserver-core` 1458 + 2; every integration file green;
`render_acceptance --include-ignored` 180/180 (after the `ad71461c` merge).

## Upstream merged during 3b

`fc0917be` (BIG-REQUESTS), `ad71461c` (window storage only while viewable,
composite backing fixes, XKB ChangeKeyboardMapping), `d5db7ccb` (xmodmap
reaches XKB). Rows in `docs/phase-c0-upstream-fixes-revalidation.md`; the
window-storage lifecycle is **high** exposure (`c0_merge_unmap_direct_window_unflips_vulkan`
is its software half).

## Decisions recorded during 3b

- Option A: one atomic transaction per device; the split into 3b-i/3b-ii;
  `ACTIVE=0` under DPMS-off; per-device rebuild by `OutputKey`; typed failure
  + one log line; one RANDR mutation in flight server-wide.
- Task H closed as delivered; the shared core-loop iteration export dropped
  (user, 2026-09-25).
- Tests that need the assembled server on Owner are not simulated: they are
  listed in `docs/phase-c0-deferred-real-server-tests.md` (user, 2026-09-26).

## Known intermittent failures

Under suite concurrency, `c0_3bi_`: `direct_hold_released_on_every_end`
(possibly the starvation defect fixed in `d069a29c`; re-measure),
`each_failure_has_its_cause`, `retired_copied_frame_stages`,
`a3_desired_frame_of_a_retired_output_is_released`,
`position_only_updates_in_place`. `render_acceptance`'s
`present_pixmap_enqueues_pending_and_defers_emission` (< 50 ms bound) under
load. A gate failing only on these is re-run once.

## Carried

- **Deferred to the real Owner server** (`docs/phase-c0-deferred-real-server-tests.md`):
  3b-ii Task 5's gate scripts through the real loop; 3b-i-2 F15; the
  second-device hardware step.
- **Activation (stage 4/5):** one `ResourceService` and `DrmCleanupRegistry`
  per Owner device (`install_resource_service` has no production caller yet).
- **3c:** the arbiter defers prompt obligations during a dispatched modeset;
  the RANDR reprobe moves off the core thread; the requester-less timestamp
  rule versus Legacy hotplug.
- **Before the PR:** fold the WIP commits into one commit per plan (user
  confirms; the branch is pushed, so it needs a force push).
