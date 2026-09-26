# Handoff — Phase C.0 stage 3b (2026-09-26)

Branch `feat/phase-c0-atomic-kms-migration`, tip `730606b4` (plus this file).
Pushed to `fork` up to `e9cfcbef` (2026-09-26); push later work after asking the user.

## Where 3b stands

Spec: `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md` (rev 12).
3b = client modeset and RANDR on the Owner route (VT/hotplug are 3c).

| Plan | State |
| --- | --- |
| 3b-i-1 modeset execution (`...-3b-i-1-plan-modeset-execution.md`, rev 11) | done, incl. hardware addenda A1, A2, A3 (`94c00628`) |
| 3b-i-2 modeset routes (`...-3b-i-2-plan-modeset-routes.md`, rev 5) | done (F15 carried to activation) |
| 3b-ii RANDR protocol (`...-3b-ii-plan-randr-protocol.md`, rev 9) | done; Task 5's gate scripts deferred to the real Owner server (`docs/phase-c0-deferred-real-server-tests.md`) |

**Next:** the 3b acceptance finding, `docs/status.md`, then fold the WIP
commits into one commit per plan (user confirms history rewrites), then push
(user confirms).

**Task H is closed by user decision** (plan rev 4): the end-state check
(`c0_3bi_assert_end_state`), the core-entry driver (`c0_3bi_core_driver_until`)
and the kernel-faithful stub (`StubBehaviour::AcceptKernelCalls`) stay as they
are. The "shared run-one-iteration" refactor of `core_loop::run` was
**dropped** — do not re-propose it. Defects found later go to addenda. An
unverified flake-hunt patch is kept outside the repo at
`~/Projects/yserver-patches/taskH-flakes-partial-on-dede8405.patch` (probably
obsolete, see below).

## Last merge from upstream

`d069a29c` merges joske/master up to `ad71461c` (window storage only while
viewable, composite backing fixes, XKB). Integration notes are in the merge
message; revalidation rows added to `docs/phase-c0-upstream-fixes-revalidation.md`.
It fixed a real product defect found by the merge: `retire_owner_current`
starved releasable buffers behind one blocked on `KmsRelease`.
Gate green; `c0_hw_3b_modeset_owner_on_card1_drm` 3/3 on card1.
`render_acceptance --include-ignored`: 180/180 on the 2026-09-26 tip (a first run
failed only `present_pixmap_enqueues_pending_and_defers_emission`, its < 50 ms
enqueue-time bound under load; 3/3 alone and 2/2 full runs after).
`730606b4` then merged `d5db7ccb` (XKB #171, low exposure; gate green).

## Known intermittent failures (`c0_3bi_`, under suite concurrency)

Plan rev 4 lists `direct_hold_released_on_every_end`, `each_failure_has_its_cause`,
`retired_copied_frame_stages`, `a3_desired_frame_of_a_retired_output_is_released`.
New since Task 3: `position_only_updates_in_place` (1 in 3 runs).
The starvation fix in the merge may have been the cause of the `direct_hold`
flake; re-measure before assuming. Rule: a gate failing only on these is
re-run once; a repeat or any other failure is a finding.

## How the work is run

- **Codex implements** (`gpt-6-luna`, `xhigh`; `max` on send-backs):
  `codex exec -m gpt-6-luna -c model_reasoning_effort=xhigh --sandbox danger-full-access "<prompt>" < /dev/null`.
  Resuming a session: `codex exec resume -m ... -c sandbox_mode=danger-full-access <SESSION_ID> "<prompt>"`
  (`resume` rejects `--sandbox`). Every prompt restates: no git writes; never
  `_drm` tests, `render_acceptance`, `c0_hw_*`, DRM master or modesets; never
  edit `docs/status.md`; never weaken an assertion; the gate with exact counts
  per filter; the known-flakes rule. Plans give interfaces, invariants, tests
  and mutations — not code.
- **Codex reviews specs/plans** via `docs/superpowers/review/review.sh`
  (`gpt-6-sol` xhigh); findings go to `docs/superpowers/findings/`.
- **The coordinator verifies**: re-runs the suites itself (codex has
  misreported passing suites), spot-checks mutations, commits, and runs the
  hardware test after every task that touches a real KMS path.
- **Hardware** (`c0_hw_3b_modeset_owner_on_card1_drm`, card1 = RTX 5060 Ti,
  HDMI-A-2): the box is also the user's VR/gaming rig — ask before GPU work.
  Preflight must gate, not just print: the active VT
  (`/sys/class/tty/tty0/active`) must not be the graphical session's, and no
  VR/game process may run. Hyprland on tty1 with Xwayland holding card1 open is
  fine while the user is on another tty (logind drops its master). Never exit
  Hyprland: it kills the other sessions.
- Gate: `cargo +nightly fmt --check`; clippy `--all-targets -D warnings` in
  default, `tcp-transport`, `xdmcp`; each `c0_*` filter with
  `--include-ignored --skip _drm`; `--lib`; `c0_2ci`; `yserver-core`; each
  integration file except `render_acceptance`.

## Deferred to the real Owner server

`docs/phase-c0-deferred-real-server-tests.md` (user, 2026-09-26) lists every
test that needs the assembled server on Owner; stages 4 and 5 run them.

## Carried to the activation stage (4/5)

- **One `ResourceService` + `DrmCleanupRegistry` per Owner device.** Today the
  backend holds one, and `install_resource_service` has no production caller;
  a client modeset on a second Owner device fails closed at preparation
  (`Stage::Allocation`). The stage that installs services in production owns
  it, together with 3b-i-2's carried test `c0_3bi_enable_on_b_unflips_a_vulkan`
  (F15, plan rev 5).

## Carried to 3c

- The arbiter defers prompt obligations during a dispatched modeset.
- The RANDR reprobe must move off the core thread.
- The requester-less publication timestamp rule versus Legacy hotplug.
