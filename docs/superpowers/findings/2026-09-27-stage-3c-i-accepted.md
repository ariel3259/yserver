# Stage 3c-i — VT switching on the Owner: accepted

**Tip:** `a461096b`. Plan `docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md`
revision 5 (six tasks; review loop closed by the user after round 4). Design
`docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md`
revision 5 (§3). Implemented by codex `gpt-6-luna` (xhigh; max on send-backs)
inside a `systemd-run --user --scope -p MemoryMax=16G` scope; every task
verified by the coordinator, who re-ran the suites itself and audited removed
assertions.

## What exists now

On a server with an Owner device a VT switch is a `VTRelease`/`VTAcquire`
lifecycle transition:

- **Release.** Prompt obligations at once, never behind the executor or a
  dispatched commit (input paused, held keys released, DPMS protocol 0,
  admission closed, Presents answered by blackout). One `ACTIVE=0` commit per
  Owner device, queued behind dispatched work; a mixed server's Legacy suspend
  is scoped to its devices with its waits capped at the remaining budget. The
  hand-off (`drmDropMaster` + `VT_RELDISP(1)`) happens when every slot is
  terminal or at the absolute 1 s deadline (a `next_wakeup`). An unknown
  outcome follows C.0 §10's VT-release row (`ExecutorStalled`, helper
  termination, quarantine, urgent withdrawal); a client token pending at the
  deadline resolves `Failed` through the ordinary ready-token path.
- **While released.** Legacy parity: `SetCrtcConfig` → `Failed`
  (`SeatReleased`), DPMS kept as `Deferred(SeatReleased)`, hotplug edges
  recorded, lower-precedence lifecycle events deferred and coalesced.
- **Acquire.** The `AcquireEpisode` begins first and the core reserves its gate
  turn before draining pending requests; a per-device probe; the scoped Legacy
  resume stages its RANDR difference; per-participant dispositions (a failed
  Owner probe or a kernel-rejected reinstall closes and urgently withdraws that
  device — `RecoveryFailed` — while the server continues); input resumes
  without waiting for Owner; a from-scratch `ALLOW_MODESET` reinstall per
  healthy device with the current DPMS level; one publication only if the
  topology changed.
- **Rapid switching.** A release supersedes an undispatched acquire; after
  dispatch it waits inside its own bound; the device stays ready for the next
  acquire.
- **Core.** Urgent withdrawal publications bypass the gate FIFO and only
  subtract from the published projection (every later publication is filtered
  against withdrawn ids); `EpisodeBegin`/`EpisodeEnd` hold the gate turn.
- `vt` writer coverage proven; the deferred real-server row stays unclaimed.

## Evidence

| Unit | Commits |
| --- | --- |
| Task 1 urgent withdrawal, episode turn | `b0797971` |
| Task 2 release | `034b78b0`, test fixes `e406342c`, `9410cf6e` |
| Task 3 while released | `a4fe838e` |
| Task 4 acquire | `8133b1e5`; hardware fixes `949bccca`, `b994a227`, `9fcd42e0`, `74e10018`, `891fcdef`, `2f0756dd` |
| Task 5 rapid switching | `a9c6c191`; hardware fix `a461096b` |
| Task 6 coverage, rapid hw cycle | `a241b4ae` |

**Hardware (card1, RTX 5060 Ti, HDMI-2 1920x1080@60, NVIDIA 615.71.09,
nvidia-drm vblank=Y, kernel 7.2.8; run by the user from a text VT with
`~/vt.sh`):** `c0_hw_3c_vt_switch_on_card1_drm` passes **3/3** at `a461096b`,
each run six real VT hand-offs (four ordinary cycles, one rapid
acquire-then-release before the reinstall is dispatched, one final return),
~2 s per run, clean end state. `c0_hw_3b_modeset_owner_on_card1_drm` 3/3 at the
same tip (regression).

**Gate at the tip (coordinator run):** fmt; clippy `-D warnings` default,
`tcp-transport`, `xdmcp`: 0; `c0_3ci_` 34 (+7 core), `c0_3bi_` 76, `c0_3bii_` 5,
`c0_3aii_` 36, `c0_3a_` 37, `c0_2b_add_` 14, `c0_conv_ciii_` 39, `c0_conv_cii_`
26, `c0_conv_cfb_` 35, `c0_conv_cp_` 34, `c0_adm` 127, `c0_merge_` 1; `--lib`
1939/0/317; `c0_2ci` 172; `yserver-core` 1465; every integration file green.

## What the hardware found

Fixtures passed every time; card1 found six defects, each fixed with a test
that failed before its fix:

1. **The test ran the test branch.** A `#[cfg(test)]` fork in production code
   gave the acquire reinstall synthetic property ids; the hardware test is
   compiled with `cfg(test)`, so the kernel answered `ENOENT` while production
   was correct. Live-KMS fixtures now take the production discovery path.
2. **A rejected reinstall hung the device** in `Quiescing` with no transition;
   it now reaches `RecoveryFailed` (C.0 §10 `VTAcquire` row).
3. **The displaced pool leaked:** the reinstall did not register the old pool's
   `KmsRelease`.
4. **A Displaced composed frame leaked** intermittently: its render
   notification was drained while scanout was inactive.
5. **After a superseded acquire the next acquire did nothing:** the arbiter
   deferred the newer `VTAcquire` as `ReadinessClosed` against the wrong
   transition kind.
6. **Test harness:** `SIGUSR2` killed the process through libtest's
   pre-existing threads (process-wide handlers now); a killed run left the tty
   in `KD_GRAPHICS`/`K_OFF` (`~/vt.sh` restores it after every run; recovery
   without root is `loginctl activate` to another session).

Topology validation outcomes are now logged (one line each, errno included).

## Carried

- **3c-ii:** connector hotplug, the off-core-thread probe (3c-i's acquire uses
  the synchronous per-device probe), `DeviceRemoved`,
  `c0_3ci_acquire_skips_a_removed_device` and
  `c0_3ci_acquire_probe_no_reply_withdraws_owner_device`.
- **3d:** the fresh incarnation for a device closed by an unknown release or a
  `RecoveryFailed` acquire.
- **Stage 4:** the cursor detach in the release commit.
- **Open, unrelated to 3c:** a Vulkan validation error (an image sampled in
  `SHADER_READ_ONLY_OPTIMAL` while still `UNDEFINED`) logged before the first
  release in the hardware test's initial compose; to investigate separately.
- **Review lesson:** grep a real-path test's code for `cfg(test)` forks.
