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
- At device open, before plane enumeration, yserver sets `DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT` on every device.
  Virtualized drivers (virtio-gpu, vmwgfx, qxl, vboxvideo) hide their cursor plane from atomic clients without it.
  If the plane exposes `HOTSPOT_X`/`HOTSPOT_Y`, every commit that shows the plane or changes the sprite sets them.
  If the kernel rejects the cap (< 6.6), yserver continues; such drivers then expose no cursor plane and those
  CRTCs use the software cursor (accepted outcome, user decision "option a").
- Devices without cursor planes (Apple DCP) and CRTCs left unassigned by the matching use the software cursor, as
  today.
- Deleted: `set_cursor2`, `move_cursor`, the legacy hide operation, and every "no atomic positions → legacy move"
  branch. Devices that relied on them use the software cursor.

## 2. Cursor plane intent on the commit stream

- Each CRTC holds the *intended* cursor plane state: `Visible { fb, x, y, hotspot }` or `Hidden`.
- Every atomic commit on that CRTC carries the current intent: scene flips, Vsync direct scanout and cursor-only
  commits when the CRTC is idle. A commit adds only the properties that differ from the last *applied* state.
  The applied state advances when the commit is accepted.
- **Show and hide share the frame's commit.** The scene's per-output `CursorAssignment` (HW vs SW) and the frame that
  stops (or starts) painting the software cursor travel in the same atomic commit as the plane's show (or hide).
  `ShowOnRetire` / `HideOnRetire` and their stored coordinates are removed. Positions are read when each commit is
  built, so the #244 jump cannot occur.
- Pointer motion only updates the intent's position. The #244 cursor-only scheduling pass (CRTC idle →
  `NONBLOCK | PAGE_FLIP_EVENT`; after EBUSY, wait for the CRTC's next flip event) is unchanged.
- **Sprite framebuffers.** The sprite goes into a per-device dumb buffer sized to `DRM_CAP_CURSOR_WIDTH/HEIGHT`,
  image at the top-left corner, as today. Each buffer gets an ARGB8888 framebuffer (AddFB2). `SRC_*` and
  `CRTC_W/H` cover the whole buffer (no scaling); `CRTC_X/Y` = pointer − hotspot. An image change writes the
  alternate buffer (two per device) and switches `FB_ID`. A replaced framebuffer is removed, and its buffer reused,
  only after the page-flip event of the commit that replaced it.
- **Tearing (C1).** While tearing is On the intent is `Hidden` and async commits carry no cursor properties, as C1
  specifies. Whichever of C1 and this work merges second adapts to the other in its rebase.

## 3. Errors

- **EINVAL on a cursor-only commit:** that CRTC switches to the software cursor (logged once), intent `Hidden`. The
  scene paints the cursor from its next frame.
- **EINVAL on a scene or direct commit that carried a cursor change:** retry the same frame without cursor
  properties and switch that CRTC to the software cursor. If the retry also fails, it is today's flip error path.
- **EBUSY:** unchanged from #244, never a cursor failure.
- A CRTC switched to software by EINVAL retries the plane only on the next topology change or sprite shape/size
  change. This matches today's `set_cursor2` backoff.

## 4. Tests (rules A/B/C/F)

1. Through `Backend` / `run_core` with the recording atomic sink: show, hide, image change and motion. Every commit
   carries the right `FB_ID`/`CRTC_ID`/`SRC`/`CRTC`/`HOTSPOT` values. Show and hide share the commit of the frame
   that stops or starts painting the software cursor. End state: no pending intents, no leaked framebuffers.
2. #244 jump regression: crossing into a window with another cursor while the pointer moves between the request
   and the commit. The committed position equals the current pointer. Must fail on master.
3. Capability matrix: missing plane or any required property → software cursor; `HOTSPOT_X/Y` present → set;
   hotspot cap rejected → virtual driver CRTCs use software.
4. Framebuffer lifetime: a replaced framebuffer is removed only after its replacing commit's event. Mutation: remove
   it at replacement → the test fails.
5. Gate: no `set_cursor2` / `move_cursor` left under `crates/`.
6. Error paths: EINVAL cursor-only → SW; EINVAL on a frame commit → retried without cursor, then SW; EBUSY → no SW.

## 5. Hardware validation (with the user's approval, per card)

- NVIDIA (card1): motion, drag, animated cursor; 0 core stalls (#244's criterion); EBUSY within #244's numbers.
- AMD iGPU (card0): the same, plus Vsync direct scanout with a visible cursor and Plasma X11 window drags.
- virtio-gpu VM on kernel ≥ 6.6: the guest hotspot matches the host cursor. If no VM is available, the PR lists it
  as pending validation.
