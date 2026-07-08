# Scope: direct-scanout page-flip for fullscreen unredirected windows (#7)

**Date:** 2026-07-08
**Source:** `docs/superpowers/findings/2026-07-08-xorg-render-optimization-gaps.md` #7
(= make-v2-fast Task 7). **Status:** scope only — not started. HW-only work; test
machine = **silence** (i9-13900K / RX 580, amdgpu+RADV).

## What #7 actually wins (precise)

yserver **already page-flips**. Every frame, `scene.rs::tick_one_output` acquires a
triple-buffered `ScanoutBo`, runs `record_compose_v2` to **sample every scene window
into that BO** (one full-output GPU blit), then `submit_flip_with_fences` atomic-commits
it (`scene.rs:2762-2856`). The win of #7 is narrow and specific: **when the scene reduces
to a single opaque window covering the output, skip the compose blit and flip that
window's own backing directly** — zero GPU compositing that frame. This is the issue-#82
case (fullscreen video / maximized player / single terminal / wallpaper-only desktop).

It is **not** a new presentation path — the atomic-commit/fence/retirement machinery is
built and battle-tested. It is: (a) make an eligible window's backing scannable, and
(b) route the scene to flip it instead of compositing.

## As-built map (from a full code survey — file:line load-bearing)

- **Output scanout = full Vk→dma-buf→DRM-FB pipeline, reusable.** Each output owns 3
  `ScanoutBo`s; each is a `VkImage` with `ExternalMemoryImageCreateInfo(DMA_BUF_EXT)` +
  `ExportMemoryAllocateInfo`, exported via `vkGetMemoryFdKHR`
  (`scanout.rs:1105-1118`), imported with `prime_fd_to_buffer` (PRIME_FD_TO_HANDLE,
  `scanout.rs:340`), registered via `add_planar_framebuffer`→`drmModeAddFB2`
  (`scanout.rs:354`). Format fixed `B8G8R8A8_UNORM`; modifier from the KMS↔Vulkan
  intersection (`allocate_vk_scanout_image`, `scanout.rs:948-1123`).
- **Present = atomic commit with fences.** `submit_flip_with_fences` sets plane
  `FB_ID`/`CRTC_ID` + `IN_FENCE_FD` (GPU-done SYNC_FD) + `OUT_FENCE_PTR`, commits
  `PAGE_FLIP_EVENT|NONBLOCK` (`page_flip.rs:152-213`). Completion → `drain_events`
  → per-CRTC MSC/UST (`platform.rs:1428-1478`). **One pending commit per CRTC**
  (`tick_one_output` gates on `pending_acks`, `scene.rs:1405-1426`).
- **Window backings are NOT scannable.** `allocate_drawable_storage`
  (`platform.rs:1661-1774`) = `OPTIMAL` tiling, plain `DEVICE_LOCAL`, **no external-memory
  export, no modifier**, usage `COLOR_ATTACHMENT|TRANSFER_SRC/DST|SAMPLED`, format
  `format_for_depth(depth)`. Cannot be handed to `prime_fd_to_buffer`/AddFB2 as-is.
- **Fullscreen predicate already exists** (`scene.rs:1972-1986`): `covers` (window rect ⊇
  output) && `opaque` (`depth != 32`) && `participating` (`scene_participating`,
  server-drawn). Today it only **suppresses the COW overlay** — it does not flip. This is
  the exact predicate to reuse; note `opaque = depth != 32` ⟹ depth-24 BGRX, which **is**
  scanout-format-compatible with the fixed BGRA8 scanout FB.
- **DRI3 client pixmaps already arrive as dma-bufs** (`dri3.rs:370 import_dmabuf` →
  `DrawableImage::from_dmabuf`, `backend.rs:17081`), with importable/exportable modifier
  sets computed (`dri3.rs:39/151`). These are the one window class that is **already
  potentially scannable** without re-allocation.

## Machinery inventory + reframe (code survey 2026-07-08 — CHANGES the size)

The buffer-handoff question turned up a structural reframe. The fullscreen **DRI3
game/video** case (the solid-win workload) does NOT flow through the scene compositor's
window-backing flip — it flows through the **X Present extension**, whose flip path was
**pre-built and then shelved**. So case (C) = *revive the dormant Present flip path* (the
"alien-BO scanout integration" the code comments repeatedly defer to), not "add an
eligibility check to `scene.rs`". Case (B) (non-DRI3) is the `scene.rs` backing-flip.

Inventory of the Present path:
- **WIRED + correct (reusable safety shape):** the Copy path withholds `IdleNotify` until
  the client buffer's GPU read genuinely completes — a double fence-gate in
  `pending_present_batch_ready` (`backend.rs:5180-5208`). Idle-fence / release-syncobj
  signal-back (`fire_pending_present_entry`, `backend.rs:5086-5106`), `CompleteNotify`
  MSC/UST off the real kernel vblank clock — all present and correct. **This proves the
  server already controls client buffer reuse; the handoff contract is enforceable in
  principle.**
- **DORMANT (built, unit-tested, never called in production):** the scheduler FIFO
  `pick_at_vblank`, and the buffer-identity primitives `record_flipped` (returns the
  *previous* handle so its IdleNotify can be withheld until the replacement retires) +
  `last_flipped` (`present_scheduler.rs:237-281`). This is exactly the "previous vs current
  buffer" primitive M2 needs — dormant, not missing.
- **MISSING / STUBBED:** `flip_path` hardcoded `false` (`backend.rs:17592`); the client
  **acquire/wait fence is parsed but never waited on** — v1.0 does a bounded 50ms implicit
  dma-buf read-wait (`wait_present_source_ready`), the **v1.4 synced path does neither**
  (`process_request.rs:8122-8146`); `build_path_selector_inputs` stubs
  `pixmap_format/modifier=0` + `output_scanout_format_set=&[]` (so DirectScanout can't even
  be selected); no KMS `IN_FENCE`-based scanout of a client buffer; flip-complete never
  drives a previous-buffer IdleNotify.

**Effort implication:** M2 is bigger than "reuse the output FB machinery + eligibility". It
is: revive the Present flip scheduler drain + populate buffer-identity tracking + **honor
the client acquire fence** (correctness prereq — scanning out contents-not-ready = garbage)
+ extend retention "hold-until-copied" → "hold-until-replacement-flip-retires" + plumb the
alien-BO dma-buf metadata + kernel scanout-compat probe + wire the acquire fence as the KMS
`IN_FENCE`. The output-side Vk→dma-buf→FB→atomic-flip half is done; this Present-side half
is the real work. **M1 is unaffected and still the right safe first probe.**

## The crux decision: how does an eligible window backing become scannable?

Three options, increasing blast radius:

- **(C) DRI3-only, no re-allocation** — support only windows whose backing is already a
  dma-buf (DRI3/Present clients: fullscreen GL/Vulkan games, video via VA-API/DRI3). If
  the import modifier is in the output's scanout-modifier set, register an FB and flip.
  **Narrowest, lowest-risk, no render-perf change, and hits the highest-value workload
  (fullscreen games/video).** Misses xterm/wallpaper (non-DRI3).
- **(B) Lazy re-allocation on fullscreen transition** — when a server-allocated opaque
  window becomes a covering candidate, re-allocate its backing through the exportable
  path (`allocate_vk_scanout_image` shape) and register an FB; revert when it stops
  covering. General (covers issue-#82's terminal/wallpaper) but adds reallocation churn +
  a backing-swap dance + possible render-perf hit if the scanout modifier forces LINEAR.
- **(A) Always allocate backings exportable** — rejected: blast radius = all windows,
  render-perf risk, most memory pressure, for a fast path that fires rarely.

**Recommendation: build (C) first** (validates the entire flip path end-to-end with
minimal new allocation and no render-perf risk), then **(B)** as a follow-up for the
non-DRI3 fullscreen cases if the win proves out. Do **not** do (A).

> **Codex reframing — the MVP is not "fullscreen + AddFB2 succeeded".** The load-bearing
> constraint is **buffer ownership across the whole frame**, not FB registration. The
> current `ScanoutBo` model is safe *because* compose copies into a dedicated BO the client
> can't touch; reusing a window backing removes that safety. The real MVP is: **"direct
> scanout for DRI3 buffers with provable front-buffer pinning + multi-buffering"** — the
> scanned image must be immutable until pageflip retirement, and the client's *next* render
> must land in a *different* buffer. DRI3 clients normally multi-buffer (each present is a
> distinct dma-buf), which is why (C) is viable — but the server must **verify** the client
> has moved to a different buffer, and **fall back to compose** for any single-buffered
> producer. To keep (B) an extension not a rewrite, build around a generic `ScanoutSource`
> abstraction + FB cache keyed on `(dma-buf planes, modifier, size, format)` +
> atomic-test-based eligibility + explicit pin/unpin — **not** a `DRI3-window` special-case
> threaded through scheduling and lifetime.

## Milestones (each gated on HW validation on silence)

### M1 — Register FB + ATOMIC-TEST it, NO FLIP (the safety-first step)
> **Codex correction:** the original "compose FROM the FB" idea does NOT prove scanout
> safety — Vulkan sampling only proves the GPU can read it as a texture, not that the
> *display engine* accepts it on this plane/CRTC with this modifier/stride/offset. The
> faithful zero-risk validation is a **`drmModeAtomicCommit(..., ATOMIC_TEST_ONLY)`** — it
> exercises the exact plane+CRTC+FB acceptance the real commit would, and changes nothing.
- Add eligibility: in `tick_one_output`, after `build_scene`, detect "scene == single
  opaque covering participating window" (reuse `scene.rs:1972-1986`) AND its backing is
  scannable (case C: dma-buf-backed with an output-compatible modifier).
- Register a DRM FB for the eligible window's backing (reuse `add_planar_framebuffer`).
  Build the **exact** atomic state the real direct-scanout flip would use, and run it with
  `AtomicCommitFlags::TEST_ONLY`. **Never a real commit in M1.**
- **Exit gate:** AddFB2 accepts the backing AND `ATOMIC_TEST_ONLY` succeeds for the exact
  plane state. If either fails, that window is ineligible → the (untouched) compose path
  runs. Zero risk to the live CRTC. This is the de-risking milestone: a wrong
  modifier/stride/format is caught by the kernel's test, not by a GPU fault.
- **M1 is safe for the CRTC but NOT side-effect-free (codex re-review).** `PRIME_FD_TO_HANDLE`
  creates a GEM-handle ref and `AddFB2` creates an FB object (possibly with dma-buf
  pins) that **outlive the test**. M1 must have explicit teardown on *every* path —
  `drmModeRmFB` + GEM-handle close + cache eviction — or repeated per-frame eligibility
  checks leak/pin kernel resources. `TEST_ONLY` is a dry run: use the same property values
  but do **not** attach a real `OUT_FENCE_PTR` or expect a page-flip event under it.

### M2 — Flip the window backing, with load-bearing fallback
- Replace compose+flip with a direct flip of the eligible window's FB in `tick_one_output`:
  bind that FB in `submit_flip_with_fences` instead of the composed `ScanoutBo`.
- **Fallback is mandatory and never removed:** any of {not eligible, no registered FB,
  modifier mismatch, AddFB2 failure, atomic-commit error} → fall straight through to the
  existing `record_compose_v2` + BO flip. Direct scanout is purely additive.
- **Fence discipline:** the window's last render must complete before scanout — reuse the
  IN_FENCE_FD handshake (the window backing's render-completion semaphore exported as a
  SYNC_FD), mirroring `record_compose_v2`'s `bo.vk_semaphore` export (`scene.rs:2826-2838`).
- **Retention:** per presentproto, the previously-scanned buffer stays live until the next
  flip completes. The window backing is now doubling as a scanout FB — its FB registration
  must survive until the flip that replaces it retires, and be torn down on
  unmap/resize/destroy/damage-driven-realloc. Mirror `last_flipped` retention
  (`present_scheduler.rs:180`) + the per-CRTC `pending_acks` gate.
- **Exit gate on silence:** fullscreen DRI3 client (e.g. `vkcube`/`glxgears -fullscreen`/a
  fullscreen video) flips with **zero `record_compose_v2` submits** (confirm via
  telemetry `composite_submits/s` dropping to ~0 while the app presents), no tearing, no
  corruption, and un-fullscreening cleanly reverts to the compose path.

### M3 — (optional) Broaden to server-allocated backings (case B)
Only if M2's win justifies it: lazy re-allocation of opaque covering windows through the
exportable path, covering xterm/video/wallpaper. Separate scope; do not bundle.

## Implementation sequence (third-pass review — M2 is a project, not a step)
1. **M1** — eligibility + FB import + `ATOMIC_TEST_ONLY` + teardown + instrumentation
   (log accept/reject AND whether the client rotates distinct buffers). Ready now, HW-safe.
2. **Acquire-fence wait** — land as a **standalone correctness milestone**, independent of
   scanout. It's a **pre-existing bug** (the client's acquire fence is parsed but never
   waited; v1.4 synced doesn't even implicit-wait), so it's worth fixing on its own merits
   regardless of #7, and it's the front-edge safety M2 needs.
3. **Scheduler/flip-path revival + revoke/fallback state machine** — wire the dormant FIFO
   drain + `record_flipped`/`last_flipped` into production; build the revoke/fallback
   transitions + FB/GEM lifetime accounting; align Present queue vs one-pending-commit.
4. **M2** — the real direct-scanout flip, gated on 2+3, with mandatory Copy fallback.

## DESIGN-OPEN (must resolve before M2 — codex re-review's headline gap)

The unresolved crux is **NOT** KMS eligibility (the atomic test settles that) — it's
**proving the client buffer handoff** so the scanned image is immutable while on screen.
Two independent gates are required; the atomic test only covers the first:
1. **KMS acceptance** — `ATOMIC_TEST_ONLY` (M1 covers this).
2. **Producer-side safety** — the scanned dma-buf must not be re-rendered until its
   replacement flip retires. `TEST_ONLY` cannot prove this.

Concrete requirements for gate 2 (design work before M2):
- Track **per-Present dma-buf identity** — which exact image each Present presents.
- Direct-scan only when a **genuine buffer rotation** is observed (client cycles distinct
  dma-bufs), never when one image is reused frame-to-frame. Present serials / MSC / "window
  got another frame" / a per-window semaphore are **all insufficient**.
- **Pin** the scanned image server-side until its replacement flip retires.
- Require a **per-image-per-frame acquire fence** (the final write to *that* dma-buf for
  *this* flip). If one can't be produced for the image, the frame is **ineligible** →
  fallback. (Where this fence comes from on the DRI3/Present path is itself an open item.)
- **FB cache key must include dma-buf object identity / FD**, not just
  `(planes, modifier, size, format)` — two distinct imports can share that metadata.

If yserver can't observe a reliable per-image handoff contract from DRI3/Present for these
clients, **even case (C) is not safe** and this needs a design answer, not just code. This
is the gate on whether M2 is buildable as scoped.

**RESOLUTION (code survey 2026-07-08): resolvable, with a clear shape — but it's a
subsystem revival, not a checkbox.** The handoff contract IS enforceable: the server
already withholds `IdleNotify` until a buffer is GPU-safe (proven, fence-gated, in the Copy
path). M2 extends that from "hold until copied" to "hold until the replacement flip
retires," using the **dormant** `record_flipped`→returns-previous primitive to fire the
retiring buffer's IdleNotify at flip-complete. Buffer rotation is observable because the
server gates reuse — a well-behaved DRI3 client cannot re-render the pinned buffer until
the server releases it. **Two hard prerequisites that are currently stubbed and MUST land
first:** (1) actually **wait on the client acquire fence** before scanout (today unwaited;
v1.4 not even implicit) — else you scan contents-not-ready; (2) revive the scheduler FIFO
drain + buffer-identity population (dead code today). Until (1)+(2) are wired, M2 is unsafe. **Third-pass review adds (3):** a **revoke/fallback
state machine** for "was eligible, then ceased to be" — fullscreen loss / occlusion /
mode change / hotplug / TEST_ONLY-starts-failing / commit-fail — with **correct FB+GEM
lifetime accounting across pending/current/retiring buffers on every error path**, and
explicit alignment of Present's own queue semantics with KMS's one-pending-commit-per-CRTC
gate. (Async/tearing flips NOT required — fallback-to-Copy suffices.)

## Risk register (this is the machine-lock-risk zone)

- **★ Buffer ownership across the frame (codex #1/#3 — the load-bearing hazard).** KMS reads
  the scanned FB for the entire refresh interval until the next flip retires. If the client
  re-renders into that same backing before retirement → tearing/corruption/undefined
  KMS↔Vulkan sync, and on some stacks a display-engine wedge. **Rule:** only direct-scan a
  buffer that is provably immutable until pageflip-out (client has moved to a different
  dma-buf). Single-buffered producer → fall back to compose. This — not FB registration —
  is what makes M2 hard.
- **Eligibility predicate is necessary but NOT sufficient (codex #6).** "single opaque
  covering window" ignores output scaling, per-output transform/rotation, gamma/color-mgmt
  /HDR, viewport/src-crop mismatch, and cursor/overlay-plane interaction — any of which
  forces composition. **The real gate is the atomic TEST**, not the heuristic: build the
  exact plane state and let the kernel accept/reject. Decline (fall back) on any test
  failure rather than reasoning about which property tripped it.
- **GPU fault → unrecoverable reset latch.** A wrong modifier/fourcc/pitch such that the
  GPU or display engine faults trips the known `renderer_failed` latch
  (`project_gpu_reset_no_recovery`) — corruption-until-zap. **Mitigation: M1 validates the
  import path with zero flips; M2 keeps the compose fallback.** Never scan out a BO whose
  AddFB2 hasn't already succeeded.
- **One pending commit per CRTC** — already enforced (`pending_acks`); the direct-flip path
  must respect the same gate or risk `-EBUSY`/stalls.
- **Mid-flip client damage.** A fullscreen client rendering into a backing that is *also*
  the live scanout FB races the display. Reuse the IN_FENCE handshake; if the client
  double-buffers via DRI3 (they do), each present is a distinct BO — retention handles it.
  For single-buffered backings, fall back to compose.
- **Misbehaving client (codex 3rd pass — don't oversell blast radius).** If a client
  re-renders the pinned dma-buf while KMS is scanning it, that's a client bug — but the
  corruption is **not** confined to its own window; it can garble the whole CRTC frame.
  It will **not** wedge the kernel as long as the memory object stays valid. Acceptable
  contract, but state it honestly.
- **Multi-output.** `window_covers_output` is a single-output approximation
  (`present_scheduler.rs:122-124`). Gate the first cut to single-output-covered; a window
  spanning/covering one of several outputs needs care.
- **HW-only iteration.** No lavapipe. Every milestone is a silence HW session. Budget for
  it; keep a serial console / SSH-from-another-box ready in case of a lock (REISUB).

## Verification
- **M1:** HW on silence — AddFB2 accepts window backing + composed frame pixel-correct.
  Unit-testable pieces: the eligibility predicate (pure fn over scene state) — TDD it.
- **M2:** HW on silence — fullscreen DRI3 app flips with `composite_submits/s → ~0`, no
  tear/corruption, clean revert. Regression: windowed/multi-window desktop unchanged
  (compose path untouched). Run the existing xts/rendercheck + a WM smoke.
- Cannot be validated on lavapipe (needs real `/dev/dri` + display).

## Open questions for codex
1. Is (C)-first the right MVP, or does starting DRI3-only bake in assumptions that make (B)
   a rewrite rather than an extension?
2. M1's "validate without flipping" — is composing-from the registered window FB a faithful
   proof that a *flip* of it would display correctly, or does flip exercise paths
   (scanout-engine format/modifier constraints) that compose-sampling doesn't? If the
   latter, what's the safest first flip?
3. Retention/lifecycle: does reusing a window backing as a scanout FB while it's still a
   normal render target (client keeps drawing into it) violate any presentproto or KMS
   invariant that the current BO-owns-the-FB model avoids?
4. Fence: is the window backing's render-completion semaphore exportable as a SYNC_FD the
   same way `ScanoutBo::vk_semaphore` is, or is there a per-image-vs-per-BO mismatch?
5. Anything that makes this fault the GPU rather than fail cleanly to fallback.

## Codex review verdict (2026-07-08)
Scope judged sound; **(C)-first is correct** and does not force a (B) rewrite *provided*
it's built on a generic `ScanoutSource` + dma-buf-keyed FB cache + atomic-test gate (not a
DRI3-window special-case). Corrections folded in above: **M1 uses `ATOMIC_TEST_ONLY`, not
compose-from-FB** (Vulkan-sampling ≠ scanout-acceptance); **the real hazard is buffer
ownership/immutability across the frame, not FB registration** — the MVP is "DRI3 +
provable multi-buffering/front-buffer pinning," and single-buffered producers must fall
back; **the fullscreen predicate is insufficient — the atomic test is the gate** (scaling/
transform/gamma/cursor-overlay can force compose); **IN_FENCE must be per-image-per-frame**,
representing the final write to *that exact dma-buf for this flip*, never a reused
per-window/per-BO semaphore. Bottom line (codex): "the architectural hazard is not
registration; it is ownership of the scanned image across the whole frame."

## Codex RE-review verdict (2026-07-08, revised scope)
"The revision is materially better and M1 is a safe first step." Confirmed the corrections
were incorporated correctly (not cargo-culted). Two residuals folded in above: (1) **M1 is
CRTC-safe but not resource-side-effect-free** — needs explicit RmFB/GEM-close/cache-evict
teardown on every path (the `TEST_ONLY` still imports + registers kernel objects). (2) The
**headline unresolved gap is the client buffer-handoff/immutability proof** (new
DESIGN-OPEN section) — "if that stays vague, M2 remains the real risk, not plane
eligibility." M1 is unblocked and safe to build now; **M2 is gated on resolving
DESIGN-OPEN.**

## Codex 3rd-pass verdict (2026-07-08, reframed scope) — CONVERGED
"**M1: ready now. M2: not ready until acquire-fence wait, scheduler revival, and
revoke/fallback transitions are real. Reframe: correct — this is a Present-path revival
project, not a compositor eligibility tweak.**" Confirmed (C)=revive-Present-flip is the
right vehicle for fullscreen DRI3; the IdleNotify-retention handoff is sound for
well-behaved clients (with the acquire-fence wait for front-edge safety); added the third
blocker (revoke/fallback state machine + FB/GEM lifetime + Present-queue/commit alignment);
and recommended splitting acquire-fence-wait out as a standalone correctness fix (it's a
pre-existing bug). See "Implementation sequence" above. **Scope is READY: M1 is green-lit
to build on silence; M2 is a defined 3-prerequisite project, not an afternoon.**
