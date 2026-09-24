# Finding — NVIDIA's default `nvidia-drm` config has no `GET_SEQUENCE`

**Date:** 2026-09-23. **Found by:** `c0_hw_3a_dpms_owner_on_card1_drm` on card1,
run from tty2 after the stage 2b clock-probe addendum (`8dd9fd19`,
`3d2ab55b`) made the Owner probe its CRTC clocks in production.
**Device:** RTX 5060 Ti, NVIDIA open kernel modules 615.71.09, kernel
7.2.6-gentoo-dist, HDMI-2 1920x1080@60, CRTC 205.

## Observation

The production clock probe resolved
`clock=(Unresolved, Rejected { errno: 95 })`, executor outcome
`Rejected { errno: 95 }` — `DRM_IOCTL_CRTC_GET_SEQUENCE` returned
`EOPNOTSUPP` for CRTC 205. The DPMS sequence was not reached.

## Cause (read in the 615.71.09 source)

- `nvidia-drm` has a module parameter `vblank`
  (`nvidia-drm-linux.c:47`: "Enable drm vblank notification support (1 =
  enable, 0 = disable (default))"). On this machine
  `/sys/module/nvidia_drm/parameters/vblank = N`; `/etc/modprobe.d/nvidia.conf`
  does not set it.
- `nvidia-drm-drv.c` ~894: `drm_vblank_init` runs only when the parameter is
  set, or on kernels whose CRTC state lacks `no_vblank`
  (`#if !defined(NV_DRM_CRTC_STATE_HAS_NO_VBLANK)`). Kernel 7.2 has
  `no_vblank`, so with the default the device has no DRM vblank support and the
  core answers `GET_SEQUENCE` with `EOPNOTSUPP`. Page-flip events still arrive
  (nvidia-drm synthesizes them; `nv_drm_crtc_send_vblank_event`).

This is the driver's default configuration, not a yserver defect. It is
consistent with the pre-C.0 server: Legacy already records
"sequence unsupported" and falls back.

## Consequence for C.0

C.0's clock-source row (§6) and §10 clock probing: `EOPNOTSUPP` leaves the
clock `Unresolved`, "closes C.0 qualification", software protocol-clock
synthesis is "deferred beyond C.0", and "No event-bearing commit is admitted
until … a current result". With the addendum's I-4, a lifecycle DPMS on this
device is correctly `Deferred(ReadinessClosed)` — it can never dispatch.

That rule assumed a device that fails qualification keeps a working route.
The user's stage 5 removes the Legacy route entirely. On NVIDIA with the
default `nvidia-drm` configuration — this project's primary hardware — the
Owner route would then have no DPMS (and no other lifecycle-class commit, nor
Present MSC timing) at all. **This is a C.0 spec-level decision, not an
implementation defect**, and goes to the user.

## Measured with `nvidia-drm.vblank=1` (2026-09-23)

The user added `options nvidia-drm vblank=1` to `/etc/modprobe.d/nvidia.conf`
and rebooted (`/sys/module/nvidia_drm/parameters/vblank = Y`). Same driver
615.71.09, same kernel. `c0_hw_3a_dpms_owner_on_card1_drm` then **passed three
runs in a row** (~1.1 s each):

- the production `GET_SEQUENCE` probe succeeds: the clock is `KernelSequence`;
- four `ACTIVE`-only off/on cycles: each off accepted with an out-fence that
  is observed signalled, no vblank follows it, and the retained framebuffer
  and allocation are unchanged; after each on a composed frame is admitted and
  reaches `HardwareComplete` — the fourth cycle as well as the first.

So with vblank enabled this device is structurally capable under C.0 and
NVIDIA accepts the `ACTIVE`-only DPMS shape. Two test defects surfaced on the
way and were fixed (`c0_hw_3a_…`: a held `RefCell` borrow; a bounded
damage-history length used as a progress signal).

## Open decision (user)

With the driver default (`vblank=N`) the device remains structurally
incapable under C.0 and admits no Owner traffic, which conflicts with stage 5
removing the Legacy route. Options discussed: keep Legacy as the route of
structurally incapable devices; require `nvidia-drm.vblank=1` (detected at
startup, documented); or amend C.0 with a flip-driven clock (contradicts C.0's
explicit no-software-clock decision).
