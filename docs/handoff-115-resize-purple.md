# Handoff: issue #115 — resize glitches (purple flashes) + the perf half

Date: 2026-07-31, ~01:00. Machine: **air** (M1 MBA, Asahi, MESA_AGXV, single
eDP-1 2560x1600@60). Session under test: **xfce** (xfwm4, compositing on).
Branch: `fix/115-resize-content-preservation`, cut from `master` @ `468e4f25`.

Reporter's machine (issue #115) is different and matters for the perf half:
**RX 9070 XT / Ryzen 9 5900X, dual monitor DP-1 3440x1440@165 + HDMI-1
1920x1080@60**, on `544d459c` (= `v1.4.0-13`).

**Purple is NOT fixed**, and the final run rules out the leading theory. Three
real bugs were found and fixed along the way. Read "Result of the final run"
first — it says where the remaining cause must be, and which whole class of
cause is now excluded.

---

## Issue #115 has two complaints; only one was worked

1. **Graphical glitches on fast window resize** — worked on, partially fixed,
   purple still reproduces.
2. **Poor performance, "very noticeable in games"** — **not actionable yet.**
   Both reporter logs contain **zero GLX/DRI3 client traffic**, so no game was
   running during either capture. The perf half has never been measured. Next
   step is to ask for a capture with a game running plus which API it uses
   (native GL/Vulkan vs Proton) — not to keep analysing the existing logs.

What the reporter's logs *do* show (desktop, not games): ~10k
`queue_submit2/s` peak, 42 us per RENDER request, and request dispatch eating
73% of a core. That is the already-known submit-churn item — see the memory
note `yserver_mate_terminal_perf_render_submit_churn`, which also records that
the real lever is the sync-boundary/hazard rework, not the submit-group cap.
His logs reproduce it on RADV; they do not reveal anything new.

---

## Commits on this branch

| commit | what | state |
|---|---|---|
| `f4676057` | **fix 1** — carry-over copy moved ahead of the alias-retarget loop | HW-verified: 103 → 0 dropped copies |
| `5e23460d` | **fix 3** — clear the newly-exposed strip on the re-grow fast path, plus 5 diagnostics | fires, but did not fix purple |
| `c66fe993` | **gravity fix** — honour bit-gravity in leaf-storage resize | regressed on its own; fine once `218368b8` is under it |
| `ae5e2344` | revert of `c66fe993` | superseded by `99e09824` |
| `218368b8` | **store fix** — retiring drawable must not steal a re-allocated xid | **HW-verified: 318 → 0 orphaned windows, desktop sane** |
| `99e09824` | reapply of the gravity fix, now on top of the store fix | **HW-verified: 309 preserves, 0 copy failures** |

`c66fe993` alone caused a grey screen; it needs `218368b8` beneath it, because
it is the first caller to hold a store ref across a re-allocate. `ae5e2344` was
a revert created accidentally mid-interrupt and is now undone — the pair is the
intended state.

---

## The purple flash — diagnosis is solid, fix is not

### Symptom

Purple flashes on the part of an xfwm4 titlebar that becomes **wider than it
has previously been**, during resize. Reproduces continuously when the resize
handle is dragged **in a fast circle**. Stops permanently once the window has
been grown to maximum screen size and cannot grow further.

Not catchable with a hand-fired dump signal — far too brief. Every manual
capture attempt got the healed frame. (Same wall the idle-compositor spec hit;
its answer was an auto-dump fired from inside the loop.)

### Evidence chain (all measured, one xfce drag)

1. xfwm4's decoration windows are **depth 32** and set **non-Forget
   bit-gravity** — observed `1` (NorthWest) and `10` (Static). Both are
   explicit requests that contents survive a resize.
2. `sync_window_leaf_storage_to_geometry` **ignores bit-gravity entirely**: it
   reallocates storage on any size change and wipes the whole window
   `(0,0,new_w,new_h)` to its background. **540 wipes** of
   preservation-requesting windows in one drag; `355` still in the last run.
3. The wipe is **transparent, not black**: `bg_pixel=0x00000000` on a depth-32
   ARGB visual decodes to `rgba=[0,0,0,0]`. **532 of the 540** were alpha-zero.
   Identical `bg_pixel` on the 240 depth-24 windows gives opaque black and is
   invisible — which is why only the ARGB decorations show it.
4. One window took 210 of the wipes while growing `673x483 → 837x552` — the
   frame of the window being dragged.

This accounts for every observation, including the two that killed earlier
theories: the hue is **consistent** (a deterministic transparent fill, not
random recycled bytes), and it **stops at maximum size** (the function
early-returns once the storage extent already matches, so a window that stops
changing size stops being wiped).

### Ruled out, from source — do not re-litigate

- No debug magenta constant exists in the tree.
- Pixmap pool takes are **exact-size** (`PixmapPoolKey` is
  `{width,height,format}`), so it is not an oversize recycled region.
- `create_pixmap` **does** zero-fill the full extent (Stage 3f.14) and its
  failure path logged **zero** hits with debug enabled — the fill is not
  failing.
- `decode_x11_pixel_bgra` is **correct** for standard TrueColor
  (`0x00RRGGBB`, blue in the low byte), and a channel swap cannot turn grey
  into purple anyway.
- `init_clear_cursor` / `init_clear_window` / `init_clear_pixmap` are **dead
  counters** — declared, zeroed, swapped and printed every second, never
  incremented anywhere. Every `vk init_clear src` line reads
  `cursor=0 window=0 pixmap=0` and always will. This cost one wrong inference
  during the session. Wire them or delete them.

### Why the gravity fix (`c66fe993`) regressed

Symptom: grey screen, content rendered under it.

`DrawableStore::decref`'s `PendingFence` branch cleared `by_xid[xid]`
unconditionally, while `destroy_now` guards the same removal and its comment
explains exactly why. That divergence was harmless because every caller
decref'd *before* re-allocating the xid. The gravity fix holds a ref **across**
the re-allocate (it must, or its copy has no source), so its final decref ran
while the window's *new* drawable owned the xid and deleted that mapping —
orphaning the window. Its own copy also leaves the render ticket unsignaled,
making `PendingFence` the common path instead of a rare one, hence the
severity: 318 dropped client CopyAreas (`dst_store=gone`, all `op=62`).

`218368b8` fixes that store bug, and the 01:07 run validated it against the
original symptom: with the gravity fix reapplied on top, `dst unresolvable`
went 318 → **0** and the desktop stayed sane. (An earlier clean run proved
nothing about it — that one was on the reverted build.)

---

## Result of the final run (01:07) — gravity was NOT the cause

With both fixes in: desktop sane, `dst unresolvable` = 0, **309 preserves with
0 copy failures** — and **purple still reproduces, with additional corruption,
during a fast circular resize storm.**

So the bit-gravity wipe was real and is now fixed, but it is not what you see.
The decisive number is **`ALPHA ZERO` = 303**: the newly-exposed strip is still
filled fully **transparent**, and that strip is exactly where the purple
appears.

**That fill is correct.** A depth-32 ARGB window whose `bg_pixel` is
`0x00000000` genuinely has a transparent background; Xorg would store the same
thing. Every pixel-init site is now instrumented and accounted for, so the
purple is **not** something writing wrong pixels into the window.

### The one remaining lead

**What gets composited beneath a transparent region of a redirected window.**
The window is correctly see-through in that strip; something behind it is
garbage. That is a scene/compositing question. Specifically worth checking:

- Whether a transparent region of a redirected window's backing blends against
  the actual layers beneath it, or against undefined/stale scanout content.
- The `alpha_passthrough=true` mode the scene compositor uses for window draws
  (see the L1 server-alpha note near `decode_x11_pixel_server_alpha` in
  `engine.rs`, which states outright that "a paint that leaves alpha=0 in
  storage renders as a fully-transparent window — the layer underneath leaks
  through").
- `kms/vk/dst_readback.rs:458` documents a *different* case of undefined
  content surfacing as magenta on some drivers — worth reading for the pattern,
  which is a shader sampling texels nothing ever wrote.

**Do not go back to the fill sites.** They are exonerated: instrumented,
counted, and all correct.

### Also note

- `Static` (10) gravity is implemented as offset `(0,0)`. Correct only while
  the window origin is unchanged; a combined move+resize needs the position
  delta threaded in. Known gap, documented at the function.
- "Corruption" beyond the purple during the storm is unexplained and may be a
  second effect — the strip being transparent would let *whatever* is beneath
  show, including stale frames, which would read as corruption rather than a
  flat hue.

## Still open, unstarted

- **fix 2 — `composite_named_pixmaps` is never pruned.** No
  `.retain`/`.remove` anywhere in the tree; it is only ever pushed to
  (`process_request.rs`, NameWindowPixmap). Reached **36 entries for one
  window** under xfwm4, which re-Names on every resize step. Consequences: the
  alias-retarget loop's per-alias `drop_backing_storage(OLD)` count no longer
  matches the refs OLD holds (this is what fix 1 works around rather than
  cures), and every resize step does O(N) bookkeeping that grows for the life
  of the window. Pruning must decref whatever backing the entry held, or a
  premature free is traded for a leaked hold — and note the retarget comment
  records a picom-under-openbox regression from getting this area wrong.
- **Perf half of #115** — needs a game capture (see top).
- **Idle spin on the reporter's machine** — separate finding from this
  session, not an #115 regression. `6e1ba09b` ("make yserver truly idle") is
  intact and verified here: fvwm3 idle on air gives `iter/s=0`,
  `page_flip/s=0.2` over three 15 s windows, and **cinnamon** (compositing,
  single output) idles too — a 38.79 s window at `iter/s=0 req/s=0`. So
  compositing is not the trigger. But the reporter's dual-monitor capture has
  **output/0 composing 839 times in a 24 s idle window at a fixed 30.73 ms
  period** (p50 = p90 = max, i.e. a metronome, not vblank free-run) while
  output/1 correctly quiesces with an 8 s gap. The acceptance criteria for
  `6e1ba09b` covered dual-monitor on *silence* (two identical panels), so the
  untested axis is **mixed refresh rates** (his are 165 + 60, and the fast one
  is the one spinning). Reproducing needs a second display on air at a
  different refresh than eDP-1.

## Diagnostics available (all off by default)

| gate | what it gives |
|---|---|
| `YSERVER_BACKING_REFCOUNT_LOG=1` | retain/release/drop/allocate of redirected backings with refcounts. Invariant: OLD stays `freed=false` until after the rotate copy. |
| `YSERVER_DEBUG_INIT_COLORS=1` | each pixel-init site paints a distinct colour — green `create_pixmap`, blue `allocate_window_storage`, cyan window background, yellow re-grow strip. Turns "which path wrote these pixels?" into a question you answer by looking at the screen. **Note:** a window *with* a `bg_pixel` bypasses this and fills with the decoded client pixel — that blind spot is what hid the titlebar for two rounds. |
| `RUST_LOG=…backend=debug` | `window-bg-fill` (depth, bg_pixel, bit-gravity, decoded RGBA, alpha-zero flag), `bit-gravity` forwards, `leaf-resize preserved`. Compare the non-Forget `window-bg-fill` count against `leaf-resize preserved`: **equal counts mean preservation is working**, a shortfall means windows are still being wiped. |
| always on | `copy_area dropped` now reports `src_alias=`/`src_store=` and the originating request. `alias=none store=gone` ⇒ freed before the read; `alias=rc:N store=gone` ⇒ store bookkeeping; `op=12` ⇒ internal rotate, `op=62` ⇒ client CopyArea. |

## Repro recipe

```
just yserver-xfce-hw-telemetry
# grab a window corner and drag it in fast circles for ~15 s
grep -c "SPEC VIOLATION"     yserver-hw-xfce.log   # wipes of preserve-requesting windows
grep -c "ALPHA ZERO"         yserver-hw-xfce.log   # of those, transparent
grep -c "dst unresolvable"   yserver-hw-xfce.log   # orphaned windows (must be 0)
grep    "leaf-resize preserved" yserver-hw-xfce.log
```

Growth is what matters: `redirected_backing_can_fit` short-circuits on shrink,
so only growing forces the realloc paths. Circular dragging maximises size
changes per second, which is what turned an occasional flash into a constant
one.

**The `Justfile` change in the working tree is deliberately uncommitted** — it
bakes `YSERVER_DEBUG_INIT_COLORS=1`, `YSERVER_BACKING_REFCOUNT_LOG=1` and a
debug log level into the xfce recipe. Handy while chasing this, wrong to land.
