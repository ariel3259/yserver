# Gamma through atomic `GAMMA_LUT`, legacy `SETGAMMA` as capability fallback

Short plan (no spec: the gamma cache, sizes and reapply points stay; only the last step, how the LUT reaches the CRTC, changes).
It revises one decision of `specs/2026-06-18-randr-gamma-design.md` (#47), which chose the legacy ioctl and listed atomic
`GAMMA_LUT` as a non-goal.

## Why

`apply_gamma_to_live_output` (`kms/render/backend/kms/randr.rs:1554`) ends in the legacy `DRM_IOCTL_MODE_SETGAMMA`. On
amdgpu, i915/xe and nvidia-drm (615.78.08) none of the drivers has its own `gamma_set` hook, so the kernel
(`drm_crtc_legacy_gamma_set`, `drm_color_mgmt.c`) turns that ioctl into its own **blocking** atomic commit carrying a
`GAMMA_LUT` blob of the legacy size, and clears `DEGAMMA_LUT` and `CTM` in the same commit. That is the second commit per
CRTC that #47 wanted to avoid, made synchronously by the kernel outside our per-CRTC commit stream, and it blocks the
core loop while a flip is pending.

#47's reason for legacy (a separate atomic commit EBUSY-collides with scanout flips) no longer holds: the cursor series
(#272, #276, #287, #288, #290) put cursor state on the same per-CRTC stream, and this change does the same for gamma.

Driver history (when each driver gained `GAMMA_LUT`): i915 v4.7 (2016), amdgpu DC v4.17 (2018), nouveau v5.4 (2019),
nvidia-drm 545.23.06 (2023, legacy and atomic in the same release), msm dpu v7.0 (2025, both in the same release); Apple
DCP has no gamma. Every one predates its driver's Vulkan 1.2 support, or arrived together with legacy gamma.

**Legacy stays as a capability fallback:** a CRTC with a legacy gamma size but no `GAMMA_LUT` property keeps `SETGAMMA`.

## Out of scope

- **The RANDR-visible gamma size.** It stays the legacy `gamma_length` (256 on amdgpu/i915, 1024 on NVIDIA). Xorg
  modesetting always reports 1024 (`xf86RandR12InitGamma(pScrn, 1024)` from `xf86HandleColormaps`); that change belongs
  with #263 (VidMode gamma) or its follow-up, which reworks the same LUT code.
- `CTM`, `DEGAMMA_LUT` as client features, HDR, VRR.

## Design

- **Blob size = the legacy `gamma_length`.** That is exactly the blob the kernel builds today inside `SETGAMMA`, so every
  driver that accepts the legacy ioctl accepts it (amdgpu and i915 take a 256-entry `GAMMA_LUT` as their legacy mode:
  `amdgpu_dm_verify_lut_sizes`, `lut_is_legacy`; nvidia-drm's two sizes are both 1024). No client-visible change.
- **Same side effects as today:** each gamma commit also sets `DEGAMMA_LUT` = 0 and `CTM` = 0 where the CRTC exposes them,
  as the kernel's legacy path does.
- **Transport:** the pending LUT rides the next **vsync** commit of that CRTC: scene flip (`drm/page_flip.rs:235`), direct
  scanout (`drm/modeset.rs:1736`) and composed scanout (`drm/modeset.rs:1801`). Async (tearing) flips cannot carry CRTC
  properties, so the LUT waits for the next vsync commit, as cursor positions already do (`drm/page_flip.rs:231`).
- **Nothing in flight:** when no commit is pending or queued for that CRTC, the LUT goes in its own nonblocking commit
  with a page-flip event, through the same per-CRTC stream and commit identity as the other commits, so the scheduler
  sees the CRTC busy until its event, and EBUSY retries on the CRTC's next event.
- **Failure:** EINVAL on a commit carrying `GAMMA_LUT` latches that CRTC to the legacy fallback and reapplies through
  `SETGAMMA`; frame handling is otherwise unchanged. Other errors keep the LUT pending.

## Reuse inventory

- **Property lookup and caching:** `PropMap::for_object(..).id(..)` (`drm/modeset.rs:1130`) and the cached optional
  properties on `Output` (`plane_in_fence_fd_prop` / `crtc_out_fence_ptr_prop`, `drm/modeset.rs:203-209`): add
  `crtc_gamma_lut_prop`, `crtc_degamma_lut_prop` and `crtc_ctm_prop` the same way. `None` for `GAMMA_LUT` = legacy fallback.
- **Blob lifecycle:** `create_property_blob` / `destroy_property_blob` as the mode blob (`drm/modeset.rs:1365-1414`); the
  user handle is destroyed after submit, the kernel keeps its reference in the CRTC state.
- **The LUT itself:** the per-output cache `gamma_luts`, `cached_gamma_for_current_size`, `resample_channel` and the size
  query (`randr.rs:1474-1552`) stay unchanged.
- **Reapply points stay** (modeset `crtc_config.rs:1570`, DPMS-on / VT restore `crtc_config.rs:1163`, composed re-light
  `scanout.rs:1049`); they now mark the LUT pending instead of calling the ioctl.
- **Standalone commit:** `AtomicCommitSink::submit_with_commit_id` with `PAGE_FLIP_EVENT | NONBLOCK`, as the cursor-only
  commit does (`kms/render/platform/cursor_atomic.rs:136`), including its EBUSY handling pattern; and the existing
  in-flight checks the scanout scheduler already consults (`cursor_commit_in_flight_for_outputs`, `scanout.rs:908`,
  `present.rs:922`) so that a scene flip is not submitted over a pending gamma commit.

## Tasks

### Task 1: atomic gamma transport

Capability on `Output`, pending LUT per output, the LUT riding vsync scene/direct/composed commits, the standalone
commit when nothing is in flight, the EINVAL latch, legacy fallback.

Tests (Backend-driven, rule B; end state checked, rule A):
- a CRTC with `GAMMA_LUT`: `SetCrtcGamma` issues 0 `SETGAMMA`, the next vsync commit carries `GAMMA_LUT` with a blob of the
  legacy size, `DEGAMMA_LUT` = 0 and `CTM` = 0, and the pending LUT is cleared after it is accepted. Mutation: keep
  calling `set_gamma`.
- idle CRTC (no flip in flight or queued): a standalone commit with a page-flip event carries the LUT; a scene flip
  submitted before that event is held until it. Mutation: submit the scene flip immediately.
- async (tearing) flip: carries no CRTC property; the LUT goes on the next vsync commit. Mutation: add it to the async flip.
- EBUSY on the standalone commit: retried on the CRTC's next event, not dropped.
- a CRTC without `GAMMA_LUT` but with a legacy size: `SETGAMMA`, as today. EINVAL on a gamma-carrying commit: that CRTC
  latches to `SETGAMMA`. Mutation: drop the latch.
- reapply after a modeset and after VT resume still reaches the hardware (now through the commit).
- `GetCrtcGammaSize` unchanged (legacy size).

Hardware (rule C), NVIDIA and AMD, release build, baseline on master first (this is also #47's pending smoke):
gammastep (or redshift) changing gamma while mpv plays fullscreen in direct scanout (mpv rendering on the same GPU via
`--vulkan-device`; by default it picks the first Vulkan device and a cross-GPU buffer never reaches direct scanout) and while idle; strace from server start: `SETGAMMA` count (0 on the branch), commits carrying
`GAMMA_LUT`, mpv dropped frames, and that the colour visibly changes and survives a VT switch.

### Task 2: docs

`docs/status.md` entry; the #47 spec gets a note pointing to this plan for the transport decision.

Gate (every task): fmt, `cargo build --bin yserver`, workspace + lavapipe ignored suite, clippy `-D warnings`,
`tools/check-layout.sh`, FreeBSD cross-clippy; load runs per rule F.
