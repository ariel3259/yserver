# Atomic-only hardware cursor — design

Status: approved (2026-10-15). Branch `feat/atomic-cursor-only`, rebased on joske/master `9524c80d` (includes C1, #249).

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

**Why the legacy ioctls can go (user, 2026-10-16).** GPUs whose drivers cannot do atomic cursor commits also have no
Vulkan driver, and yserver renders only through Vulkan, so the legacy cursor path only serves hardware that cannot
run yserver. This also holds for the copied route: there the other GPU composes, but the display device imports the
frame and copies it into its own scanout buffers through its own Vulkan transfer context
(`VkContext::new_transfer_for_device`, `platform/init.rs`). A display device without Vulkan therefore cannot be a sink
either.

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
  off state). When the matching moves a bound plane, A's detach must be accepted before B's attach is sent:
  - If A is being disabled, the detach rides A's lifecycle disable commit (§2.4).
  - If A stays active, losing the plane is A's HW→SW transition. The detach rides **A's scene frame that starts
    painting its software cursor** (§2.2). A skipped or rejected frame defers the reassignment, and A keeps its
    hardware cursor meanwhile.
  - Only after A's detach is accepted (and A's outstanding event has drained) does a separate commit on B attach the
    plane. Until then, B uses the software cursor.

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

While tearing is On the intent is detached, and async commits carry no cursor properties, as C1 specifies. C1
merged first (#249); this work adapts to it. C1's tearing spec (`2026-10-14-tearing-on-demand-design.md`) says the
cursor plane is hidden at the unflip retirement. Under this design that detach is owned by the frame that starts
painting the software cursor (§2.2), and this spec supersedes that wording.

### 2.6 When a cursor-only commit is sent (C11 addendum)

Found in C11 on amdgpu, where flip events arrive at vblank and only one commit per CRTC fits in a vblank. While the
pointer moves over a 60 fps FIFO Vulkan client (mpv), the branch alternates `SceneWithCursor` and `CursorOnly`
commits one per vblank, so the client drops to 30 fps. Master shows 60 `SceneWithCursor`. A µs probe on the AMD
showed what happens after each scene flip event. The cursor-only commit is submitted at p50 19 µs (max 40) after the
event. The client's next `PresentPixmap` is processed at p50 28 µs (max 64). The Present was already queued in the
server when the event was handled: the flip-event handler sends the cursor-only commit before the core loop reads
that iteration's client requests.

Rules:

- **One output-local gate.** After a flip retires on output O while O has eligible cursor intent, O is
  *awaiting a frame*. While O is awaiting, no ordinary cursor-only commit for O is sent. The rule covers every lane:
  the flip-event handler, the held-direct lane, independent peer-output updates and the post-compose fallback. A
  scene or direct commit built for O in that time carries the intent (`SceneWithCursor`, or the direct successor's
  cursor delta). The C11 r1–r3 exceptions keep going cursor-only at once:
  - the recovery detach (§3);
  - a failed scene submit, which releases its cursor intents;
  - a retry that would otherwise park cursor intent.
- **When the wait ends.** The wait ends at the first *decision point* where O has no scene or direct work, and the
  core reports that its bounded dispatch (request count and time budget) left no request queued. It also ends when
  2 ms have passed since the retired flip event. In both cases one cursor-only commit carries the intent.
  - Decision points are the loop's composition tail and `before_block`.
  - The core reports its backlog as one boolean, before composition, after each dispatch. It never says which
    requests are queued, and the fairness budgets do not change.
- **How the 2 ms bound is measured.** It is measured in loop time. The core loop does not preempt a request, and
  the pointer motion itself is processed by the same loop. So an expired wait is serviced at the first decision
  point after expiry, and the deadline counts towards the poll timeout. A request flood cannot starve the cursor
  past the first decision point after the bound.
- **Idle scene.** With no backlog and an idle scene, the cursor-only commit goes out in the iteration that
  retired the flip, never a vblank later.
- **Other drivers.** NVIDIA and other drivers that accept a scene commit and a cursor-only commit in the same vblank
  keep working as today. This ordering must not reduce their cursor rate.

Acceptance on AMD, with the pointer moving at 250 Hz over a windowed 60 fps FIFO client: scene commits ≈60/s, and
cursor-only commits ≈0 while the scene runs every vblank. The motion→accept p99 must not regress beyond one dispatch
iteration (tens of µs).

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
5. Reassignment: the matching moves a bound plane from CRTC A to B. A's detach is accepted first, then B's attach.
   B uses the software cursor until its attach is accepted, and no commit both detaches and attaches. With A
   remaining active, A's detach rides the scene frame that paints A's software cursor. If that frame is skipped (no
   BO), the reassignment is deferred and A keeps its hardware cursor, so A is never left without a cursor.
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
12. Flip-time ordering (§2.6), through `run_core`. The model is vblank-locked: one commit per CRTC in flight, and the
   event is delivered once per vblank. A Present client works like Mesa FIFO: it presents its next frame as soon as
   it sees its COMPLETE, so that Present is already queued when the next flip event is handled. Continuous pointer
   motion runs throughout. Over 30 vblanks, every vblank carries a scene commit with the cursor
   (`SceneWithCursor` ≥ 28), and no vblank is spent on a cursor-only commit while the client has a frame queued.
   With the client stopped, the cursor-only lane still moves the cursor every vblank. A third case queues the
   PresentPixmap behind more requests than the dispatch budget allows: the decision waits for the backlog and the
   cursor rides the scene. A fourth case keeps a backlog of unrelated requests queued for the whole run, with the
   injected clock advancing during dispatch and across expiry: the cursor-only commit goes out at the first decision
   point after 2 ms. A fifth case holds a direct (fullscreen) frame of a FIFO client: the held-direct lane honours
   the gate, so the direct successor carries the cursor. Mutations:
   - drain the cursor in the flip handler again → fails;
   - evaluate scene work before the queued client requests → fails;
   - delay the cursor-only commit to the next vblank when the scene is idle → fails the idle half;
   - ignore the backlog report → fails the third case;
   - drop the 2 ms bound → fails the fourth case;
   - let the held-direct or post-compose lane bypass the gate → fails the fifth or third case.

## 5. Hardware validation (with the user's approval, per card)

- NVIDIA (card1): motion, drag, animated cursor; 0 core stalls (#244's criterion); EBUSY within #244's numbers.
- AMD iGPU (card0): the same, plus Vsync direct scanout with a visible cursor and Plasma X11 window drags.
- virtio-gpu VM on kernel ≥ 6.8: the guest hotspot matches the host cursor. If no VM is available, the PR lists it
  as pending validation.
