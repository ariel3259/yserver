# Atomic-only hardware cursor — design

Status: draft for review (2026-10-15). Branch `feat/atomic-cursor-only`, base joske/master `8b8b1364`.

## Goal

Drive every hardware cursor plane through atomic commits and delete the legacy cursor ioctls
(`set_cursor2`, `move_cursor`). Any CRTC without a usable atomic cursor plane uses the software cursor.
Fix the #244 cursor jump: when the pointer crosses into a window with another cursor, the plane briefly shows
the cursor at a stale position.

## Background

PR #244 (`e118bc3e`) moved cursor *positions* onto one atomic commit stream per CRTC (scene flips, direct scanout,
cursor-only `NONBLOCK | PAGE_FLIP_EVENT` commits, at most one commit in flight). Showing, hiding, image and hotspot
changes still use legacy `set_cursor2` (`kms/cursor_plane.rs`), and devices without atomic position properties
still move the cursor with `move_cursor`. The scene sequences plane transitions with `ShowOnRetire` / `HideOnRetire`
(`yserver-render` `scene/cursor.rs`, `scene/tick.rs`). `ShowOnRetire` captures x,y when the show is requested
and applies it at retirement, which causes the #244 jump.

## Non-goals

Gamma/LUT, overlay planes, tearing (C1 keeps its own cursor policy), cursor scaling, new cursor protocol features.

## 1. Eligibility (per CRTC)

- A CRTC uses a hardware cursor plane only if the #246 matching assigns it a cursor plane that exposes all of
  `FB_ID`, `CRTC_ID`, `CRTC_X`, `CRTC_Y`, `CRTC_W`, `CRTC_H`, `SRC_X`, `SRC_Y`, `SRC_W`, `SRC_H`. Otherwise that
  CRTC uses the software cursor. Evaluated at device open and on every topology change.
- **Hotspot cap.** At device open yserver sets `DRM_CLIENT_CAP_ATOMIC` first, then
  `DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT`, before plane enumeration. The cap exists since Linux 6.8 (the uAPI header's
  "6.6" comment is wrong). A rejection on a physical driver is normal and changes nothing.
- **Virtualized drivers** (driver name `virtio_gpu`, `vmwgfx`, `qxl`, `vboxvideo`): a cursor plane is eligible only
  if the cap was accepted AND the plane exposes `HOTSPOT_X`/`HOTSPOT_Y`. This is decided by feature detection, not
  by kernel version, so backports are handled. Kernels before 6.8 can still expose the plane without hotspot
  support, and there the CRTC uses the software cursor (accepted outcome, user decision "option a").
- Devices without cursor planes (Apple DCP) and CRTCs left unassigned by the matching use the software cursor, as
  today.
- Deleted: `set_cursor2`, `move_cursor`, the legacy hide operation, and every "no atomic positions → legacy move"
  branch. Devices that relied on them use the software cursor.

## 2. Cursor plane state and its owners

### 2.1 Applied state, intent, no-ops

- Per CRTC, yserver tracks the **applied** cursor plane state, which is what the last *accepted* commit set (`fb`,
  position, hotspot, or detached), and the **intent**, which is what it should be.
- A commit carries only the cursor properties that differ from the applied state. When the intent equals the applied
  state (for example motion coalesced B→C→B, or positions equal after integer rounding), the pending work is
  consumed without an ioctl and without an in-flight or flip-event obligation.
- The applied state is invalidated (the next commit sends the full cursor state) after DRM master loss and resume,
  every modeset, and every cursor-plane reassignment.
- **Reassignment.** DRM rejects moving a bound plane straight from CRTC A to CRTC B (it requires an intermediate
  off state). When the matching moves a bound plane, yserver first waits for A's outstanding commit and event,
  sends a detach-only commit on A and waits for it to be accepted. Only then does a commit on B attach the plane.
  Until the plane is attached on B, B uses the software cursor.

### 2.2 Who submits what

| Change | Owner |
|---|---|
| HW↔SW transition of an output (the scene's `CursorAssignment` changes) | the **scene frame** for that output that stops or starts painting the software cursor: plane show or hide goes in that frame's commit. A frame that is not submitted (no BO, skipped, failed) publishes nothing, and its assignment is not applied by any other commit. |
| HW-only change while the output stays HW: motion, image/animation frame, hotspot, crossing into or out of an output whose cursor is HW | the **cursor-only commit**, generalized from #244: when the CRTC is idle it sends whatever HW-only intent differs from the applied state (position, `FB_ID`, hotspot, attach/detach), not only positions of an already visible plane. While a frame or direct flip is in flight, the change rides that commit if it has not been built yet, or waits for its event (#244's EBUSY rule). |
| Show, hide, animation and first bind during **held Vsync direct scanout** | the cursor-only commit (a direct frame never paints a software cursor). This replaces today's direct hooks (`refresh_direct_cursor`, `hide_cursor_under_direct_scanout`, first bind at direct retirement). If the plane cannot take the cursor, direct scanout unflips as today. |
| DPMS off, VT leave, output removal | the **lifecycle disable commit**: it detaches the cursor plane (`FB_ID=0`, `CRTC_ID=0`) in the same blocking commit that disables the CRTC (§2.4). |

- Per-output cursor mode (HW, SW, Mixed while outputs differ) changes when the commit that carried the transition
  is accepted, and its flip event retires it as today, so Mixed bookkeeping is kept.
- `ShowOnRetire` / `HideOnRetire` and their stored coordinates are removed. Every position is read when a commit is
  built.

### 2.3 Sprite framebuffers

- Sprites live in per-device dumb buffers sized to `DRM_CAP_CURSOR_WIDTH/HEIGHT`, image at the top-left corner, each
  with an ARGB8888 framebuffer (AddFB2). `SRC_*` and `CRTC_W/H` cover the whole buffer (no scaling); `CRTC_X/Y` =
  pointer − hotspot.
- A framebuffer has a **user count**. Each CRTC counts as a user while the framebuffer is:
  - *on screen*, until the flip event of the commit that replaced it (accepting the replacement is not enough,
    because the hardware may still be reading it);
  - referenced by a submitted, in-flight commit;
  - referenced by a commit built but not yet sent.

  A buffer is rewritten, and its framebuffer removed (RMFB), only at user count zero. This covers several outputs
  showing the same sprite and retiring at different times. For a blocking commit that has no flip event, the *on
  screen* reference ends when the commit returns successfully and the old events have drained.
- An image change, and also a **hotspot-only change**, writes a free buffer and switches `FB_ID`. Virtio forwards
  the hotspot to the host only on a framebuffer change (`virtgpu_plane.c`).
- When no buffer is free, the newest wanted image stays pending (older pending images are dropped) and is written
  when a buffer's user count reaches zero. A live buffer is never overwritten. The pool starts with two buffers per
  device and may grow to one per CRTC plus one.

### 2.4 Lifecycle

- DPMS off, VT leave and output removal send one blocking commit that disables the CRTC and detaches its cursor
  plane. On success the applied state is detached, and framebuffers whose last user was that CRTC are reclaimed
  after the old flip events have drained (blocking all-off commits produce no replacing event). On failure the
  previous ownership is kept and today's error path runs.
- Resume and modeset start from an invalidated applied state (§2.1).

### 2.5 Tearing (C1)

While tearing is On the intent is detached, and async commits carry no cursor properties, as C1 specifies. Whichever
of C1 and this work merges second adapts to the other in its rebase.

## 3. Errors

- **Attribution.** As today, a rejected commit that carried cursor properties is tested with `TEST_ONLY` without
  them, to tell a cursor rejection from a frame rejection (`drm/atomic.rs`).
- **Cursor-only commit rejected (EINVAL):** the applied state is unchanged (the kernel kept the previous binding).
  The CRTC is marked *SW-pending*. yserver sends a detach-only commit (`FB_ID=0`, `CRTC_ID=0`). When that is
  accepted, the output becomes SW and the scene recomposes with the software cursor. If the detach is also rejected,
  ownership stays HW and the plane is left as it is (logged once). The cursor never shows twice.
- **Frame commit rejected because of its cursor properties:**
  - *HW→SW frame (hide):* the frame already paints the software cursor. Retry it with an explicit detach. If that
    is rejected too, that frame is **not** submitted: it would show the software cursor while the hardware cursor is
    still bound. The output stays HW, the previous primary framebuffer stays on screen, and the scene renders a
    replacement frame without the software cursor. That frame is submitted without cursor properties.
  - *SW→HW frame (show):* the frame was rendered without the software cursor. Retry it without cursor properties
    (one frame without a cursor), keep the output SW and request an immediate recompose with the software cursor.
  - *HW-only change riding a frame:* retry without cursor properties; the change is handled as a rejected cursor-only
    change above.
  - If the retry also fails, it is today's flip error path.
- **EBUSY:** unchanged from #244, never a cursor failure.
- **Re-qualification.** The key is (width, height, hotspot), as today's backoff signature. Pixel or animation-frame
  changes never re-qualify. After a rejection, a CRTC retries the plane once per new key per topology epoch. A
  rejected key is not retried until the next topology change.

## 4. Tests (rules A/B/C/F)

All through `Backend` / `run_core` with the recording atomic sink and controllable flip events. End state for each:
no pending intents, no in-flight obligations, no leaked framebuffers.

1. Show, hide, image change and motion: each commit carries the right `FB_ID`/`CRTC_ID`/`SRC`/`CRTC`/`HOTSPOT`.
   Show and hide share the commit of the frame that stops or starts painting the software cursor.
2. **#244 jump regression:** drive the real window-enter path (pointer crosses into a window with another cursor),
   defer submission and retirement, move the pointer from P0 to P1 in between, and record the **whole** cursor
   operation trace: atomic properties, and on master the legacy ioctl seam. Assert that nothing after the motion
   sets P0. Must fail on master.
3. Capability matrix: missing plane or any required property → SW. Physical driver with the cap rejected → HW still
   eligible. Virtual driver with the cap rejected **and the plane exposed** → SW. Virtual driver with the cap
   accepted and `HOTSPOT_X/Y` present → HW with hotspot set.
4. Framebuffer lifetime: two outputs share a sprite and retire at staggered times; one output skips frames; an
   animation arrives while every buffer is in use. No buffer is rewritten or removed while any user remains. In
   particular, an animation frame must not rewrite F0 after the commit replacing F0 is accepted but before its flip
   event. Mutations: release at the first replacing event, or at acceptance instead of the event → fails.
5. Reassignment: the matching moves a bound plane from CRTC A to B. The commits are: A's detach accepted first,
   then B's attach. B uses the software cursor until its attach is accepted. No commit both detaches and attaches.
6. Ownership: a held direct frame with no successor gets show, hide, animation and first bind through cursor-only
   commits. A skipped (no-BO) frame does not publish its assignment. Cross-output motion. Mixed mode.
7. Lifecycle: DPMS off/on, VT leave/resume and output removal detach the plane in the disable commit and reclaim
   after the drain. A failed disable keeps ownership. Resume sends the full state.
8. No-op: B→C→B coalescing and integer-equal motion produce no ioctl and no event obligation.
9. Hotspot-only change switches `FB_ID` (virtio needs it).
10. Errors: every §3 case, checking **every accepted intermediate frame** and not only the final recovery. This
   includes a rejected detach: the software-cursor frame is never submitted while the plane is bound, so no accepted
   frame shows two cursors. It also includes a SW→HW show failure (one cursorless
   frame, then a SW recompose), and repeated rejected animation frames (no retry loop: one retry per key per epoch).
11. Gate: no `set_cursor2` / `move_cursor` left under `crates/`.

## 5. Hardware validation (with the user's approval, per card)

- NVIDIA (card1): motion, drag, animated cursor; 0 core stalls (#244's criterion); EBUSY within #244's numbers.
- AMD iGPU (card0): the same, plus Vsync direct scanout with a visible cursor and Plasma X11 window drags.
- virtio-gpu VM on kernel ≥ 6.8: the guest hotspot matches the host cursor. If no VM is available, the PR lists it
  as pending validation.
