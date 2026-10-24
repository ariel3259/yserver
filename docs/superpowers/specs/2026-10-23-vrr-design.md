# Variable refresh rate (VRR / adaptive sync) — design

**Date:** 2026-10-23 · **Status:** approved by the user (2026-10-23), reviewed once (sol); gamma (#293) merged as 6d83dcb9; plan `docs/superpowers/plans/2026-10-24-vrr.md` · **Scope:** KMS backend,
one output covered by a client window in direct scanout; cursor behaviour while VRR is active.

## Why

A game that renders at a rate the monitor cannot divide evenly (50 fps on a 60 Hz panel, 170 fps on a 240 Hz panel)
either waits for the next fixed vblank (stutter, latency) or tears. A VRR monitor can instead extend its vertical front
porch until the next frame arrives, so each frame is shown as soon as it is ready. Linux exposes this through two KMS
properties: the connector's `vrr_capable` (read-only) and the CRTC's `VRR_ENABLED` (set by userspace).

yserver has no VRR today: no code reads `vrr_capable` or writes `VRR_ENABLED`.

Xorg modesetting can only enable VRR while it flips one window that covers the whole X screen, and it enables it on
every CRTC at once. yserver composes and commits per output, so VRR can be enabled on the one monitor that shows the
game while the others keep their own fixed rate. This document covers the first step: one output, direct scanout.

## Goals

1. **VRR on the output a client window covers.** While the direct-scanout owner of an output is a client window, and
   that output's connector reports `vrr_capable` = 1, the output's CRTC runs with `VRR_ENABLED` = 1. Otherwise it runs
   with `VRR_ENABLED` = 0.
2. **Frames reach KMS when they are ready.** In direct scanout a client Present is already submitted when the buffer is
   ready, with at most one flip in flight and one queued successor. With VRR that flip ends the front porch, so no new
   frame clock is needed. This goal is "keep it that way": nothing in this design may make a direct Present wait for a
   fixed vblank boundary while VRR is on.
3. **Cursor motion does not drive the refresh rate.** While VRR is on for a CRTC, a new cursor position rides the
   client's next frame commit on that CRTC. If no frame commit arrives within **33 ms** of the first position that is
   still unsent, the position goes out in its own commit. Cursor image changes keep today's path (see The cursor while
   VRR is on).
4. **On by default.** No configuration switch and no environment variable (`YSERVER_*` variables that change behaviour
   are not allowed).
5. **Logs that let a tester confirm it works** (see Logging).

## Non-goals

- Several outputs: VRR on an output composed by the per-output scene path (backlog step 3), and per-output direct
  scanout (backlog step 4). Direct scanout today requires the window to cover the whole root, so with several monitors
  this design does not activate VRR unless one window covers all of them.
- Reading `_VARIABLE_REFRESH` or any other window property (decided, see below).
- HDR, colour management, `CTM` / `DEGAMMA_LUT`.
- Changing Present's MSC/UST semantics. MSC still counts flips/vblanks as today; UST still comes from the flip event.
- Window rules or per-application exceptions.
- Tearing (async flips) combined with VRR. The tearing mode keeps working as today; see Interaction with tearing.

## Anti-goals (a failure even if VRR technically works)

1. **The desktop never runs in VRR.** Without a client window owning direct scanout of an output, that output keeps its
   fixed refresh. Panels can change brightness with the refresh rate; normal desktop use must not show that.
2. **No change without VRR.** On an output whose connector lacks `vrr_capable`, or with no client window in direct
   scanout, yserver behaves exactly as before: same commits, same cursor commits, same timing.
3. **Turning VRR on or off is invisible:** no black frame and no full modeset.
4. **Cursor motion never holds the monitor at its maximum refresh** while a client controls the refresh rate. A single
   cursor commit sent at the 33 ms deadline may happen to precede a client frame closely; what must not happen is a
   sustained stream of cursor-driven refreshes.

## Decisions (approved in conversation, 2026-10-23)

- **Activation model: a client window that covers the output** (the behaviour KWin uses for fullscreen windows; see
  Provenance). Mesa and NVIDIA clients take the same single route.
- **`_VARIABLE_REFRESH` is not read.** The property is a convention between Mesa and Xorg's drivers, not part of any
  X11 protocol, and the server never answers it. Mesa's Vulkan WSI sets it to 1 when `drirc` allows adaptive sync and
  otherwise deletes it (it never sets 0). NVIDIA's userspace libraries never set it. Reading it would add a second
  input that, in practice, only ever repeats "covers the output". Cost accepted: an application that Mesa's `drirc`
  excludes (a browser playing fullscreen video, for example) also gets VRR while it covers the output, as under KWin.
- **"Client window", not the compositor's image.** Only a direct-scanout owner classified as `ScanoutM0Target::Unredirected`
  activates VRR. `Cow` and `CowDescendant` (a compositing manager's overlay window and its children, such as Muffin's
  stage) never do: that buffer is the whole desktop. With a compositing manager, VRR therefore follows whether the
  manager unredirects the fullscreen game.
- **Cursor deadline 33 ms.** NVIDIA's own cursor path in NVKMS and KWin both use 1000/30 ms for the same problem.
- **On by default.**

## Kernel and driver behaviour this design relies on

Read in the sources (behaviour only; see Provenance). The items marked *by reading* are not yet measured on hardware.

1. **`VRR_ENABLED` is not a modeset property.** The DRM core only stores it in the CRTC state; it is not one of the
   conditions that make a commit need `ALLOW_MODESET`.
2. **amdgpu (Linux 7.1.9):** a change of `VRR_ENABLED` is processed in atomic check without a modeset and maps to the
   FreeSync state (variable or inactive); it takes effect with the next flip of the primary plane, through a stream
   update. Since commit 3477c1b0972d (2025, also in stable), an atomic commit on a CRTC with VRR active pulls the
   primary plane in and starts a refresh cycle, including cursor-only commits. Legacy cursor ioctls are excluded from
   that, but yserver no longer has them (#290).
3. **nvidia-drm / NVKMS 615.78.08:** every modeset through DRM prepares VRR on the head without activating it;
   `VRR_ENABLED` decides per flip whether the flip is a VRR flip (*by reading*). A cursor change in an atomic commit is
   part of an NVKMS flip, and a flip triggers an immediate refresh (*by reading*). On NVIDIA, VRR needs DisplayPort.
4. **Kernel UAPI documentation:** the front porch is extended until a page flip or timeout; large, frequent changes of
   the front porch can worsen perceived stutter and cause brightness flicker.

Consequences: VRR can be switched with the frame commits yserver already sends (no modeset, goal 3 of the anti-goals),
and every standalone cursor commit on an active VRR CRTC is a refresh on both drivers, which is why goal 3 exists.

## Design

### State

Per output, two facts and one decision:

- **capable:** the connector exposes `vrr_capable` with value 1 and the CRTC exposes `VRR_ENABLED`. The property ids
  are cached where the other optional properties are cached when an output is built (the pattern of
  `crtc_gamma_lut_prop`, `plane_in_fence_fd_prop`, `crtc_out_fence_ptr_prop` on `Output`). The `vrr_capable` value must
  also be refreshed on the connector rescan path, which keeps the live `Output` across a disconnect/reconnect and only
  updates connector metadata (`platform/connectors.rs`, rescan); a monitor swap on the same connector must not keep a
  stale value. A missing property means not capable.
- **wanted:** the direct transaction being submitted for this output is owned by a client window (`Unredirected`). It
  follows the owner of the transaction being submitted, not the retained direct record: first entry submits before the
  pending record is installed, and the previous direct record survives until a composed replacement retires.
- **committed:** the `VRR_ENABLED` value the kernel holds for this CRTC. It starts from the kernel's current value, read
  when the output is built, and is updated when a commit carrying the property is accepted.

`VRR_ENABLED` is written only on a CRTC that exposes it, and only when the wanted value (*capable* and *wanted*) differs
from *committed*. An output whose kernel value is already 0 and that never qualifies never receives the property
(anti-goal 2). A value of 1 inherited from a previous DRM master (at startup or on VT acquire) differs from the wanted
0 and is cleared by the next vsync commit of that CRTC, so the desktop does not run in VRR (anti-goal 1).

### Turning VRR on and off

- **On:** when direct scanout of an output starts with a client-window owner (after the existing direct-entry probation;
  no second probation), *wanted* becomes true.
- **Off:** *wanted* becomes false when direct scanout of that output ends for any reason (unflip, owner changes to the
  compositor's image, the window is unmapped/destroyed/reconfigured, topology change, per-CRTC RandR config, DPMS off,
  VT release, shutdown), or when the output is no longer capable.
- **Transport:** the new value rides the next vsync commit of that CRTC, the same way a pending gamma LUT does
  (`feat/gamma-atomic-lut`): scene flip, direct submit or composed commit. Async (tearing) flips cannot carry CRTC
  properties, so the value waits for the next vsync commit. The pending-CRTC-property mechanism added for gamma is
  generalized to carry this value too, rather than duplicated.
- **Before giving up the device** (VT release, shutdown), a CRTC whose *committed* value is 1 gets one commit with
  `VRR_ENABLED` = 0 so the next DRM master (fbcon or another server) does not inherit VRR. If no vsync commit is due at
  that point, a standalone commit carries it, as the gamma-only commit does. This is one attempt: if it fails, the
  failure is logged (warn) and the release proceeds; the VT switch or shutdown never waits on it.
- **Failure:** if a commit carrying `VRR_ENABLED` = 1 is rejected with EINVAL, VRR is latched off for that CRTC (warn
  once) and the frame is resubmitted without the property. Other errors keep the change pending. This is the gamma
  EINVAL latch, reused with gamma's lifetime: it survives an `Output` replacement on the same kernel CRTC
  (`platform/connectors.rs`, where gamma's latch and pending state are carried over) and ends with that CRTC.

Whether VRR on/off needs the primary plane in the same commit (amdgpu applies it on a primary flip) is satisfied by
every frame path: the on change rides a direct frame, and a composed unflip carries a primary-plane replacement
(`drm/modeset.rs`, composed commit). The one commit that may carry no primary flip is the standalone clear before
releasing the device; the builder must check on amdgpu that it takes effect, and if it does not, include the current
primary framebuffer in that commit.

### The cursor while VRR is on

Today a cursor change between frames goes out as its own `cursor-only` commit as soon as the CRTC has nothing in
flight; in held direct scanout this is explicit (`scanout.rs`, the branch that sends cursor-only commits while there is
no queued direct successor). With VRR on, each of those commits is a refresh (driver behaviour 2 and 3).

While VRR is on for a CRTC:

- A cursor **position** is recorded as pending, as today, but no standalone `cursor-only` commit is sent for that CRTC.
  The pending position rides the client's next direct commit, which already carries every pending position ("its
  grouped direct transaction gets every later position").
- A cursor **image** change (shape change, animation frame, show/hide) keeps today's path: a full cursor-state commit
  that can go out immediately. Direct frames carry position deltas, not image bindings, so batching images would need
  a new mechanism that moves sprite ownership into the direct transaction. Image changes are far less frequent than
  motion (an animated cursor changes a few times per second), so each one costing a refresh is accepted for this step.
- The first unsent position starts a **33 ms** deadline. The deadline is exposed through the existing
  `next_wakeup` (`backend_scanout_next_wakeup`), as the gamma deadline and the tearing `deferred_cursor_wake` already
  are; no new timer. A later change before the deadline does not move it.
- When the deadline expires with the change still unsent, the existing standalone `cursor-only` path sends it. A client
  frame that carries it first cancels the deadline. The tearing `deferred_cursor_wake` serves a different purpose (it
  caps software-cursor composition while tearing, with its own reset rules) and must keep its semantics if any
  representation is shared.
- With VRR off nothing changes.

Reuse: prefer one deadline shape shared with gamma's pending deadline over a third copy; the builder decides whether
the tearing `deferred_cursor_wake` can be the same field or must stay separate, and justifies the choice.

### Interaction with tearing

Tearing mode (async flips for a direct-eligible window that asked for `PresentOptionAsync`) and VRR are independent.
A window can be in both; async flips carry no CRTC properties, so `VRR_ENABLED` changes wait for the next vsync commit,
and the tearing cursor rules keep applying while tearing is on. Combined tearing+VRR behaviour is not tuned here.

### Logging

So a tester can confirm VRR on a real game:

- One info line per change: `kms: vrr on` / `kms: vrr off` with output, CRTC, owning window, and reason (the off reason
  uses the same reason strings the direct/tearing exits already log).
- One info line per output at build: whether it is VRR-capable (`vrr_capable` value and `VRR_ENABLED` present).
- In the existing per-second present latency summary, while VRR is on for an output: the VRR state and the minimum,
  median and maximum interval between retired flips on that CRTC in that second. A game running below the panel's
  maximum must show intervals that follow its frame time instead of multiples of the fixed refresh period.
- The count of standalone cursor commits sent because the 33 ms deadline expired, in the same summary.

## Testing

Unit and backend tests, driven through the Backend paths (no direct calls into internals), with the end state checked:

- capable output, client-window direct owner: the first direct commit after entry carries `VRR_ENABLED` = 1; the commit
  after unflip carries 0. Mutation: never write the property.
- compositor image (`Cow`, `CowDescendant`) in direct scanout: no `VRR_ENABLED` in any commit. Mutation: accept any
  direct owner.
- output without `vrr_capable` (or with value 0), or without `VRR_ENABLED`: no commit carries the property, and the
  commit stream is identical to one from before this change. Mutation: write 0 unconditionally.
- async (tearing) flip while a change is pending: the flip carries no CRTC property; the next vsync commit does.
- VT release and shutdown with VRR on: a commit with `VRR_ENABLED` = 0 is accepted before the device is released.
- EINVAL on a commit carrying `VRR_ENABLED` = 1: latched off for that CRTC, the frame still goes out.
- cursor with VRR on: motion with a direct frame inside 33 ms rides that frame (0 `cursor-only` commits); motion with no
  frame gives exactly one `cursor-only` commit at the deadline, driven through `next_wakeup`. Mutation: send cursor-only
  immediately (today's behaviour). An image change still commits as today. With VRR off, cursor-only commits are
  unchanged.
- capable state follows the connector rescan: reconnecting a non-capable monitor on the same connector turns VRR off.
- inherited `VRR_ENABLED` = 1 at startup with no client window: the next vsync commit carries 0.
- the release-time clear failing: logged, and the VT release still completes.

The usual gate applies: workspace tests, the lavapipe ignored suite, `render_acceptance`, clippy, `tools/check-layout.sh`,
the FreeBSD cross-check, and the affected suites under CPU load.

Hardware (the user validates with a game and logs):

- MSI MAG 245F X24 (48–240 Hz) on DisplayPort, NVIDIA RTX 5060 Ti; and on AMD Raphael if a DP or FreeSync-capable HDMI
  path is available. The monitor's own refresh read-out or OSD, the yserver logs above, and the game's frame-time log.
- Check while entering and leaving VRR: no black frame, no modeset, nothing visible on the panel.
- Check that the desktop (no fullscreen client, and a compositing manager's image) never logs `vrr on`.
- Check cursor motion over a game capped below the panel maximum: the flip intervals keep following the game.

## Open items for the builder

- Whether the off path needs an explicit primary-plane flip on amdgpu (see Transport).
- Whether gamma's pending-property and pending-deadline pieces can be shared as-is for `VRR_ENABLED` and the cursor
  deadline, or need a small generalization (reuse first; justify any new piece).
- Interaction with gamma's active-CRTC rule: with VRR the CRTC may flip slower than two fixed refresh periods, so a
  gamma change can go out standalone and trigger a refresh. Gamma changes are rare; note, do not tune, unless trivial.

## Provenance

Sources consulted, by static read only, for behaviour:

- yserver at `feat/gamma-atomic-lut` (`db07d171`): direct-scanout ownership (`ScanoutM0Target`, `state.rs`), direct
  Present submission (`present.rs`), cursor motion and cursor-only commits (`platform/cursor.rs`, `scanout.rs`), the
  tearing state and its cursor deferral (`state.rs`, `platform_hooks.rs`), gamma's pending property and deadline.
- Linux 7.1.9 (GPL): DRM core atomic UAPI and the variable-refresh documentation in `drm_connector.c`; amdgpu display
  manager (`amdgpu_dm.c`, `amdgpu_dm_crtc.c`) and the message of commit 3477c1b0972d.
- NVIDIA open kernel modules 615.78.08 (GPL/MIT dual): nvidia-drm connector and CRTC, NVKMS kapi, modeset, flip and
  VRR code.
- Xorg server modesetting driver (MIT): `driver.c` and `present.c`, for the `VariableRefresh` option and the
  `_VARIABLE_REFRESH` handling.
- Mesa main (MIT): Vulkan WSI X11 (`wsi_common_x11.c`), for how `_VARIABLE_REFRESH` is set or deleted.
- wlroots (MIT): output cursor and frame scheduling, which has no VRR-specific cursor rule.
- KWin 6.7.5 (GPL-2.0): only its observable behaviour is used: a fullscreen window enables adaptive sync on its output,
  and while a window controls the refresh rate, repaints caused by anything else (the cursor included) are delayed by
  up to 1000/30 ms and go out with that window's next frame. No KWin code, expressions or structure are used; the
  extracted sources were removed after reading.

This document specifies behaviour in original prose. It contains no copyleft code excerpts; property names, constants
and file names identify public interfaces.
