# Present direct-scanout M2 live plan

**Goal:** Display the M1-proven compositor-authoritative framebuffer directly
on every active primary plane, while preserving Present buffer ownership,
ordered completion, and a composed fallback.

**Branch:** `feat/direct-scanout-scope`.

## Hardware basis

On silence/RX 580, seven distinct 5120x1440 linear Muffin stage buffers each
passed one exact dual-head atomic `TEST_ONLY` transaction. The two source crops
were 2560x1440 at x=0 and x=2560. There were no rejects, errors, or repeated
per-frame probes. Eiger/AGXV exposed the same authoritative root shape at
2560x1600, but correctly issued no M1 ioctl because apple_drm rejected the
hardware cursor and yserver fell back to software cursor composition.

## M2a: live flip with a shadow COW copy

The first hardware build keeps one deliberate safety cost: it records the
existing Present Copy into the COW as a shadow fallback, but suppresses scene
composition when the direct atomic commit succeeds. This should remove the two
full-output compositions per Muffin frame on silence while ensuring an overlay,
failed successor, or other unflip can immediately compose current desktop
content. M2b may remove the shadow Copy only after live enter/leave and unflip
ownership are proven.

- Make M1 probing and M2 live commits the feature branch's default behavior;
  keep Present flip capability unadvertised.
- Reuse only an M1-accepted framebuffer for the exact same stable source and
  topology.
- Require every M1 eligibility condition, hardware cursor mode, no root
  overlay, no composed flip in flight, and no direct transaction in flight.
- Initially exclude `PresentPixmapSynced`; its acquire point must eventually
  be handed to KMS as `IN_FENCE_FD`, not merely waited in userspace.
- Record the normal source-to-COW Copy first. On successful direct submission,
  preserve that shadow but do not arm scene composition. On any failure before
  commit, retain today's Copy, damage, completion, and compose path unchanged.
- Commit every affected primary plane in one atomic
  `PAGE_FLIP_EVENT | NONBLOCK` transaction with the M1-proven crops.

## Ownership and retirement

- A successful submit transfers a dedicated backend source pin and pinned wake
  into a `DirectPresent` record. The core's parked-entry pin may then release;
  the two pins overlap across the commit.
- Wait for page-flip events from every affected CRTC before completing the new
  frame with `CompleteNotify { mode: Flip }`.
- Do not signal or idle the newly displayed buffer at its own completion.
- When a replacement direct transaction has retired on every CRTC, signal and
  emit `IdleNotify` for the previous direct buffer, then release its source
  pin/framebuffer lifetime.
- Never attempt independent per-output scene flips while another CRTC still
  scans the shared direct framebuffer. AMD rejects that mixed state with
  `ENOSPC`. Replace every primary plane with its retained per-output composed
  pool framebuffer in one non-modesetting atomic page flip, keep the direct
  source pinned until every CRTC replacement event retires, and then repaint
  the COW shadow normally. Block direct re-entry until one composed frame has
  submitted on the complete output set, so a fast Present stream cannot starve
  the fallback. Do not disable/re-enable the CRTCs: hardware showed that path
  as a visible blackout on pointer-triggered unflip.
- Treat submission of the all-output replacement as an immediate tick boundary;
  per-output scene flips before its CRTC events retire are invalid (`EBUSY`).
- A non-root child/video/game Present updates the retained COW shadow but does
  not invalidate the compositor-authoritative direct root frame. Only an
  ineligible whole-root compositor successor requests a Present-driven unflip.
- On direct entry, bind the uploaded hardware cursor to every participating
  CRTC once. Cursor moves then use CRTC-local coordinates and kernel clipping;
  normal scene composition restores precise cursor membership after unflip.
- Split backend-to-core direct completion from retired-buffer Idle delivery;
  they refer to different Present requests and cannot be represented as one
  Copy-style completion.

## Lifecycle and rollback

- Atomic failure before ownership transfer falls back to the ordinary Copy and
  does not create direct state.
- VT loss, DPMS off, topology change, drawable invalidation, and shutdown must
  first stop/replace hardware scanout, then release every direct source and
  wake exactly once.
- A software cursor, root overlay, ineligible successor, window teardown, or
  geometry change forces the normal composed path. No output may remain on a
  direct framebuffer while another output from the same root transaction has
  unflipped.

## Validation gates

- [ ] Unit tests: commit eligibility, Copy fallback, pin transfer, split
  Complete/Idle ordering, multi-CRTC retirement, and unflip retirement.
- [ ] Lifecycle tests: failed commit, drawable teardown, DPMS, VT, topology,
  and shutdown release each source/wake once.
- [ ] Silence ordinary Cinnamon: clean direct enter/replace/unflip with cursor
  (first run found and fixed a missing second-CRTC cursor bind),
  notifications, Alt-Tab, workspaces, and independent second-head activity.
- [ ] Silence Warframe: `scene_compose` collapses while direct frames retire at
  Muffin cadence and cursor/input remain responsive.
- [ ] Eiger remains safely composed because its cursor mode is software.
- [ ] Only after M2a is stable, scope M2b removal of the shadow COW Copy and
  advertise no protocol capability until full flip semantics are validated.

## M2b: lazy composed fallback

- Attempt direct ownership before recording the ordinary Present Copy. Direct
  success skips Copy and its scene-damage path; decline or submission error
  removes the provisional completion gate and executes the historical Copy
  fallback unchanged.
- Retain the exact direct source plus resolved COW target for the lifetime of
  each direct frame. This survives source/target XID teardown until KMS has
  replaced the frame.
- When an ineligible authoritative successor requests unflip, its normal Copy
  is the current fallback. For overlay, cursor-strategy, or lifecycle unflip
  without such a successor, record and submit one lazy full-root source-to-COW
  Copy from the retained current frame before the all-output atomic replacement.
- Keep redirected child/game Presents on their normal Copy path. Their cost is
  proportional to window area and cannot be removed by whole-root stage
  scanout; a later overlay/per-window scanout design would be required.
- Require eight consecutive eligible authoritative root Presents before direct
  entry. Reset the probation on an ineligible authoritative root Present so
  compositors such as E27, which alternate region-limited and full-root frames,
  remain composed instead of repeatedly entering and atomically unflipping.
