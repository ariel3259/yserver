# Stage 3c-ii — hotplug and device removal on the Owner: accepted

**Tip:** `b2682dcf`. Plan `docs/superpowers/plans/2026-09-28-phase-c0-stage-3c-ii-plan-hotplug.md`
revision 6 (six tasks; review loop closed after round 6). Design
`docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md`
revision 5 (§4). Implemented by codex `gpt-6-luna` (xhigh; max on send-backs)
inside a `systemd-run --user --scope -p MemoryMax=16G` scope; every task
verified by the coordinator, who re-ran the suites itself.

## What exists now

- **Probes leave the core thread.** One probe episode covers every open device
  (hotplug edge, forced reprobe, acquire); each probe runs on its own worker
  with its own DRM fd, tagged by device, incarnation and epoch. The episode
  applies nothing until every device has answered, and applies and publishes
  nothing if any probe fails or misses the 2 s deadline. A device whose worker
  is stuck fails its next probe at once; the retry starts when that worker is
  joined. A forced reprobe parks behind its episode and expires with it.
- **The Owner hotplug route.** Classification, the per-device transaction,
  the gate turn and the episode; publication parity with Legacy, including
  timestamps (`lastConfigTime`).
- **Typed udev events.** `remove` of an open card node is `DeviceRemoved`: no
  KMS call, executor terminated and reaped, in-flight commits terminate
  `CompletionUnknown` into quarantine, outputs withdrawn logically at once and
  published requester-less, the arbiter enters `Removed`, the server
  continues. `add` of a card that is not open is `DeviceAddedOrReplaced`: one
  log line, the card stays unopened, a removed device stays `Removed`.
- **A failed acquire** closes the device and withdraws its scene outputs as
  well as its RANDR outputs (`b47b64cf`), so its parked repaint no longer
  keeps the shared scene dirty while the other Owner devices continue.

## Evidence

| Unit | Commits |
| --- | --- |
| Task 1 probe worker, episode, acquire probe | `f329c66b`; hardware fixes `f7224d15`, `192bd295`, `521ea596` |
| Task 2 Owner hotplug route | `80e3eed4`; hardware fixes `370b5529`, `2c142273`, `8be0c620`, `e583cea1` |
| Task 3 publication parity | `170d6dd5` |
| Task 4 typed udev events, removal, add | `894be4f9` |
| Task 5 forced reprobe off the core thread | `cfdf50d1` |
| Task 6 vkms device hotplug acceptance test | `3122732b`, `f94d8bbe`, first hardware run `62d631b1` |
| Load robustness of the 3c-ii suite | `b47b64cf` |

**Hardware (card1, RTX 5060 Ti, HDMI-2 1920x1080@60, kernel 7.2.8):**

- `c0_hw_3cii_vkms_device_hotplug_drm` — the acceptance gate (user,
  2026-10-01): a configfs vkms device as a second, live Owner (real modeset,
  `KernelSequence` clock, executor, adopted pool), destroyed and re-created
  through the real kernel/udev `remove`/`add`. 6/6 consecutive at `62d631b1`,
  3/3 at `b47b64cf`, 2/2 at `b2682dcf`.
- `c0_hw_3c_hotplug_on_card1_drm` (`~/hotplug.sh`, the user pulling and
  re-plugging the HDMI cable, 4 cycles per run): 3/3 at `b2682dcf`; the user
  saw nothing abnormal.
- `c0_hw_3c_vt_switch_on_card1_drm` (`~/vt.sh`, six real VT hand-offs per
  run): 3/3 at `b2682dcf`. Observed: the monitor goes black, as if off, for
  the length of each ~2 s run and comes back at once. Expected: every release
  commits `ACTIVE=0` (design §9, Legacy's all-off at the same point), so six
  hand-offs in two seconds never let the HDMI sink lock; the image returns
  with the final reinstall.
- `c0_hw_3cii_forced_reprobe_on_card1_drm`, `c0_hw_3cii_probe_worker_on_card1_drm`,
  `c0_hw_3b_modeset_owner_on_card1_drm`: 2/2 each at `b2682dcf`.

**Gate at the tip:** fmt; clippy `--workspace --all-targets -D warnings`;
`cargo test --workspace` 20 binaries, 4303 passed; helper suites `c0_3cii_` 67,
`c0_3ci_` 36, `c0_3bii_` 5, `c0_3aii_` 36, `c0_3a_` 37, `c0_2b_add_` 14,
`c0_conv_ciii_` 39, `c0_conv_cii_` 26, `c0_conv_cfb_` 35, `c0_conv_cp_` 34,
`c0_adm` 127, `c0_merge_` 1. **Under CPU load** (one busy loop per core, the
new criterion): `c0_3cii_` 30/30 consecutive at `b47b64cf`, 12/12 at
`b2682dcf`.

## What the hardware and the load found

1. **vkms test, first run (`62d631b1`) — five test defects, none in the
   server:** the module was checked before the helper loaded it; vkms sits on
   the faux bus on kernel 7.x, so it is identified by its DRM driver name after
   udev makes the node openable; the composed-commit helper ran one iteration
   past its condition; the test swapped the single installed `ResourceService`
   while card1 kept composing (`WrongIncarnation`); the end state did not
   expect the logically withdrawn output to stay installed.
2. **The 3c-ii suite flaked under load** (4/8 runs on the untouched tree, idle
   runs hid it). Fixture synchronization fixed most cases without relaxing an
   assertion; one production defect (the failed acquire's scene outputs, above).
   A proposed early close of a failed probe episode was rejected: it lost a
   hotplug edge arriving while another device's worker was still running.
3. One fix is not shown by its mutation: `composed_readiness_waits_for_clock_probe`
   now waits, bounded, for the render source; restoring the old assertion
   never failed in 40 loaded runs.

## Carried

- **Stages 4/5 (activation):** one `ResourceService` per Owner device; then
  the vkms test's composing-on-both variant
  (`docs/phase-c0-deferred-real-server-tests.md`).
- **3d:** teardown of a `Removed` device's quarantined resources; a fresh
  incarnation for a closed device.
- **Before stage 4:** the other `c0_` suites flake under load (3b-i fails
  every loaded run, 3c-i, conv, merge); a load-robustness task classifies each
  as test timing or production race before stage 4 builds on those paths.
- Out of scope for C.0: opening a hot-added card.
