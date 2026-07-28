# Present Completion Vblank Pacing Implementation Plan (v2)

> **⚠️ STATUS 2026-07-27 — IMPLEMENTED, HW-tested, ONE OPEN REGRESSION. Read this before reviewing.**
>
> The plan below (Tasks 1–8) is fully implemented on this branch and codex-reviewed clean pre-HW. HW results on bee (AMD 680M / RADV, KDE Plasma):
> - ✅ **Spinner fixed** (ksplashqml no longer free-runs; over-render storm gone) and Vulkan/GL clients render at refresh. Faster login.
> - ❌ **Regression: unredirected fullscreen video (mpv, Vulkan-WSI / PixmapSynced) stalls intermittently** (~1–2s, bursty). *Windowed* mpv and `mpv --x11-bypass-compositor=no` (KWin keeps compositing) are **smooth** — so it's specific to the KWin-unredirected fullscreen path. General interactive drag "stutter" also reported.
>
> **What's solid (ms-timestamped instrumentation, see the TEMP top commit):** mpv presents *steadily* at 25–40 ms with **zero gaps** — so mpv and the completion pipeline are fine; the stall is **downstream, on the scanout/display side**.
>
> **Leading hypothesis (needs confirmation, likely the crux):** parking a completion for *every* present makes `present_complete_gate`/`present_pending_complete` always non-empty during video, so `arm_idle_vblanks` now fires on ~every iteration — driving the completion clock off synthetic **`CRTC_QUEUE_SEQUENCE`** ticks rather than **real pageflip retirements**. Those synthetic msc ticks are decoupled from actual scanout, so mpv gets "vblank" completions that don't correspond to frames hitting the panel. `arm_idle_vblanks` was designed as an *idle-only fallback* (its own comment: "stay flip-driven … no idle arming"); this change makes it fire during active flipping. Xorg paces every present via a real per-present `present_queue_vblank(exec_msc)` (`../xserver/present/present_scmd.c:865`) tied to actual vblanks and does NOT gate on redirect (`present_scmd.c:114/499` redirect checks only pick copy-vs-flip).
>
> **Recommended direction:** drive present-completion pacing off **real pageflip retirements / real vblank events**, not synthetic `CRTC_QUEUE_SEQUENCE` arming during active flipping (restore arming to idle-only fallback). Verify with scanout-side instrumentation (msc source: sequence-event vs pageflip-retire; real per-flip cadence during a stall). Do NOT descope to Pixmap-only or gate pacing on redirect — those are bandaids (a fullscreen GLX game would hit the same path; Xorg doesn't gate on redirect).
>
> **Cross-hardware signal:** the bee stall does NOT reproduce on air (Asahi/`apple_drm`) — code-confirmed `apple_drm` rejects `DRM_IOCTL_CRTC_QUEUE_SEQUENCE` with `EOPNOTSUPP` (`platform.rs:1470-1484` comment), permanently latching `crtc_queue_sequence_unsupported=true`, which forces the MSC clock to be real-flip-driven only (the software fallback ticks once per completed flip). That's consistent with the hypothesis: on air the decoupling this branch might cause simply cannot occur, by construction. Not yet re-tested against a driver that *does* support the ioctl (e.g. i915) to confirm the stall reproduces there too — that would be the strongest remaining confirmation.
>
> **⚠️ Do NOT conflate with a second, unrelated symptom found on fuji (Intel i915/cinnamon):** fullscreen mpv (`PixmapSynced`) plays audio but shows nothing (background wallpaper) until fullscreen is toggled off. **Confirmed via direct A/B: this ALSO reproduces on stock `master`**, so it is a **pre-existing bug, unrelated to this branch** — out of scope here. Root cause (code-confirmed): the `PixmapSynced` handler (`process_request.rs`, `PIXMAP_SYNCED` arm) calls `backend.copy_area(...)` immediately with **no wait on `req.acquire_syncobj`/`req.acquire_value`** before copying — matches the known `project_steam_pbuffer_render_regression` pattern (copies a possibly-not-yet-rendered frame; nothing correct shows until an unrelated recomposite). This code path is entirely upstream of what this branch changes (which only affects post-copy completion/release timing). Worth fixing separately; do not let it block or muddy review of the bee stall above.
>
> **Branch state:** top commit `TEMP(present): ms-timestamped pacing instrumentation` is diagnostic only (`present_pace` log target, `PACE-INSTR` prefix) — **revert before merge**. The 8 commits below it are the implementation (Tasks 1–8). Master is untouched/clean; nothing is in production.
>
> ---

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pace `PresentPixmap` **and** `PresentPixmapSynced` (Copy path) completions to the display vblank at the request's `target_msc`, so vsync clients (Qt/Mesa `loader_dri3`) are throttled to the refresh rate instead of free-running (KDE Plasma ksplashqml spins at ~500 fps into 60 Hz today).

**Architecture:** The real client wake primitive (the DRI3 xshmfence / release syncobj) is signalled **inside the backend** at GPU-copy completion (`KmsBackend::fire_pending_present_entry`, `backend.rs:5016`). That is the true throttle Mesa waits on, so pacing must control *that signal*, not core-side bookkeeping. We:
1. Give every enqueued completion a server-generated monotonic `present_id: u64` (correlation key — client `serial` and the two overloaded `dst_host_xid` values are all non-unique/mismatched).
2. Change the backend to **retain** the pinned wake (unsignalled) keyed by `present_id` and expose `signal_present_wake(present_id)`.
3. In core, compute the Xorg `effective_target_msc` at request time; park the returned completion until the display MSC reaches it, then call `signal_present_wake` + fire `IdleNotify`/`CompleteNotify`. Reuse the proven `NotifyMSC` park/`arm_idle_vblanks`/disconnect-purge pattern.

The GPU copy stays eager (compositor keeps fresh content); only the client-visible wake+events are paced.

**Tech Stack:** Rust, yserver-core (`present_scheduler`, `core_loop::run`, `core_loop::process_request`, `core_loop::process_disconnect`, `server`, `backend`), yserver KMS backend (`kms::render::backend`, `kms::render::present_completion`), X11 Present extension.

---

## Background — Root Cause (confirmed, 4 signals + codex review)

ksplashqml free-runs at ~500 fps into 60 Hz (`yserver-hw-plasma.log`, `yserver-plasma.submit.tsv`): ~500 `PresentPixmap`/s → ~500 `CompleteNotify`+`IdleNotify`/s, ~500 `copy_area` submits/s, while `page_flip/s` and `scene_compose/s` hold at ~60.

**Why:** `fire_pending_present_entry` triggers the real xshmfence (`dri3_trigger_fence_via_handle`, `backend.rs:5024`) / release syncobj (`dri3_signal_syncobj_via_handle`, `backend.rs:5029`) at GPU-copy completion, before the event reaches core. Mesa's `dri3_find_back` waits on that xshmfence, so it never blocks → free-run. Neither `PresentPixmap` nor the scheduler currently honors `target_msc`.

**Xorg contract (spec):**
- `present_get_target_msc` (`../xserver/present/present.c:155`): a **synced** (`!(options & PresentAllAsyncOptions)`) present whose `target_msc` already passed is bumped to the next field (`crtc_msc + 1` when `divisor == 0`). That ≥1-vblank delay caps a swap-interval-1 client at refresh.
- `present_execute_copy` (`../xserver/present/present_execute.c:101`): at the target vblank, copy → `present_pixmap_idle` (IdleNotify / release fence) → `present_execute_post` → `present_vblank_notify` (CompleteNotify, mode `Copy`), stamped with that vblank's `(ust, crtc_msc)`.
- `present_pixmap` validation (`../xserver/present/present_request.c:141`): `BadValue` for `divisor == 0 && remainder != 0` and `divisor != 0 && remainder >= divisor`.

**Codex review blockers folded into this v2:** (1) real wake signalled in backend not core; (2) gate key `dst.host_xid()` ≠ completion `dst_host_xid == req.window`; (3) `(window,serial)` non-unique + source-wait reorders completions; (4) gate insertion out-of-scope + leaks; (5) AsyncMayTear silent-clear uses bit `0x8` (Suboptimal) instead of `0x10`; (6) `PresentPixmapSynced` also needs pacing; (7) missing divisor/remainder validation on the Pixmap handlers.

## File Structure

- `crates/yserver-core/src/present_scheduler.rs` — **add** option constants + pure `msc_is_after` / `effective_target_msc` (Xorg `present_get_target_msc` port), fully unit-tested.
- `crates/yserver-core/src/backend/trait_def.rs` — **add** `present_id: u64` to `CompletedPresentEvent`; **add** `Backend::signal_present_wake` (default no-op).
- `crates/yserver/src/kms/render/present_completion.rs` — (no struct change required; `wake_pin` is moved out at retain time).
- `crates/yserver/src/kms/render/backend.rs` — **change** `fire_pending_present_entry` to *retain* the pin (keyed by `present_id`) instead of signalling; **add** `retained_present_wakes` field + `signal_present_wake` impl.
- `crates/yserver-core/src/server.rs` — **add** `present_next_id`, `present_complete_gate`, `present_pending_complete` + `next_present_id()`; new `PendingPresentComplete` / `PresentCompleteGate` types.
- `crates/yserver-core/src/core_loop/process_request.rs` — **fix** AsyncMayTear bit; **add** validation to both Pixmap handlers; **thread** `present_id` + `effective_target_msc` through `PendingPresentPixmap` → `execute_present_pixmap_copy` and the `PixmapSynced` handler; **add** `complete_present_now` + `fire_due_present_completions`.
- `crates/yserver-core/src/core_loop/run.rs` — **modify** the completion drain (park-or-signal), call `fire_due_present_completions`, extend `arm_idle_vblanks`.
- `crates/yserver-core/src/core_loop/process_disconnect.rs` — **purge** (signal+drop) parked/gated presents on disconnect.

---

## Task 1: Pure pacing math (`effective_target_msc`)

**Files:**
- Modify: `crates/yserver-core/src/present_scheduler.rs` (constants near line 95; free functions after `schedule_satisfied`; tests in `mod tests`)

- [ ] **Step 1: Write the failing tests** (vectors taken directly from Xorg's `present_get_target_msc` doc comment — external ground truth)

```rust
    #[test]
    fn msc_is_after_handles_wrap() {
        assert!(super::msc_is_after(11, 10));
        assert!(!super::msc_is_after(10, 10));
        assert!(!super::msc_is_after(9, 10));
        assert!(super::msc_is_after(0, u64::MAX));
        assert!(!super::msc_is_after(u64::MAX, 0));
    }

    #[test]
    fn effective_target_future_used_as_is() {
        assert_eq!(super::effective_target_msc(100, 10, 0, 0, 0), 100);
        assert_eq!(super::effective_target_msc(100, 10, 0, 0, super::PRESENT_OPTION_ASYNC), 100);
    }

    #[test]
    fn effective_target_divisor0_synced_past_bumps_to_next_vblank() {
        assert_eq!(super::effective_target_msc(5, 10, 0, 0, 0), 11);
        assert_eq!(super::effective_target_msc(10, 10, 0, 0, 0), 11);
        assert_eq!(super::effective_target_msc(0, 10, 0, 0, 0), 11);
    }

    #[test]
    fn effective_target_divisor0_async_past_is_now() {
        assert_eq!(super::effective_target_msc(5, 10, 0, 0, super::PRESENT_OPTION_ASYNC), 10);
        assert_eq!(super::effective_target_msc(5, 10, 0, 0, super::PRESENT_OPTION_ASYNC_MAY_TEAR), 10);
    }

    #[test]
    fn effective_target_divisor_modulo_examples() {
        // Xorg example: crtc_msc=10, divisor=4.
        assert_eq!(super::effective_target_msc(0, 10, 4, 3, 0), 11);
        assert_eq!(super::effective_target_msc(0, 10, 4, 2, 0), 14);
        assert_eq!(super::effective_target_msc(0, 10, 4, 2, super::PRESENT_OPTION_ASYNC), 10);
        assert_eq!(super::effective_target_msc(0, 10, 4, 1, 0), 13);
        assert_eq!(super::effective_target_msc(0, 10, 4, 0, 0), 12);
    }
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p yserver-core present_scheduler::tests::effective_target`
Expected: FAIL — functions/constants not found.

- [ ] **Step 3: Implement constants (next to `PRESENT_OPTION_COPY`, ~line 95) and functions (module level, after `schedule_satisfied`)**

```rust
/// `PresentOptionAsync` bit per `presentproto`.
pub const PRESENT_OPTION_ASYNC: u32 = 0x1;
/// `PresentOptionAsyncMayTear` bit per `presentproto` (presenttokens.h).
pub const PRESENT_OPTION_ASYNC_MAY_TEAR: u32 = 0x10;
/// Xorg `PresentAllAsyncOptions` = Async | AsyncMayTear. Any of these bits
/// means the present is not synced-to-vblank and is never bumped forward.
pub const PRESENT_ALL_ASYNC_OPTIONS: u32 = PRESENT_OPTION_ASYNC | PRESENT_OPTION_ASYNC_MAY_TEAR;
```

```rust
/// MSC comparison with 64-bit wraparound, matching Xorg `msc_is_after`
/// (`(int64_t)(a - b) > 0`). True when `a` is strictly after `b`.
#[must_use]
pub fn msc_is_after(a: u64, b: u64) -> bool {
    (a.wrapping_sub(b) as i64) > 0
}

/// Effective target MSC for a Present request — a port of Xorg
/// `present_get_target_msc` (`../xserver/present/present.c:155`). A synced
/// present whose target already passed defers to the next field; that is
/// the throttle. Caller must reject invalid divisor/remainder first
/// (Task 3) so the modulo arithmetic is well-defined.
#[must_use]
pub fn effective_target_msc(
    target_msc_arg: u64,
    crtc_msc: u64,
    divisor: u64,
    remainder: u64,
    options: u32,
) -> u64 {
    let synced = (options & PRESENT_ALL_ASYNC_OPTIONS) == 0;
    if msc_is_after(target_msc_arg, crtc_msc) {
        return target_msc_arg;
    }
    if divisor == 0 {
        let mut target = crtc_msc;
        if synced {
            target += 1;
        }
        return target;
    }
    let mut target = crtc_msc - (crtc_msc % divisor) + remainder;
    if msc_is_after(target, crtc_msc) {
        return target;
    }
    if synced || msc_is_after(crtc_msc, target) {
        target += divisor;
    }
    target
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p yserver-core present_scheduler::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver-core/src/present_scheduler.rs
git commit -m "feat(present): port Xorg present_get_target_msc pacing math"
```

---

## Task 2: Fix the AsyncMayTear silent-clear bit (blocker 5)

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs:8736` and `:9038`

Both sites define `const PRESENT_OPTION_ASYNC_MAY_TEAR: u32 = 0x8;` — but `0x8` is `PresentOptionSuboptimal`; AsyncMayTear is `0x10`. Today this wrongly strips Suboptimal and leaves AsyncMayTear set, so an unsupported-tear request bypasses pacing.

- [ ] **Step 1: Add a failing test for the mask** (new test module or inline unit test near the handler; if a handler-level harness is impractical, assert the constant instead):

```rust
    #[test]
    fn present_async_may_tear_bit_is_0x10() {
        // presenttokens.h: PresentOptionAsyncMayTear = (1 << 4).
        assert_eq!(0x10u32, 1u32 << 4);
        // Stripping AsyncMayTear must NOT strip Suboptimal (0x8) or Async (0x1).
        let opts = 0x1 | 0x8 | 0x10;
        assert_eq!(opts & !0x10u32, 0x1 | 0x8);
    }
```

- [ ] **Step 2: Run to verify fail** (only if you introduced a symbol that doesn't exist yet; otherwise this documents intent). Run: `cargo test -p yserver-core present_async_may_tear_bit_is_0x10`

- [ ] **Step 3: Fix both constants**

At `:8736` and `:9038` change:

```rust
                const PRESENT_OPTION_ASYNC_MAY_TEAR: u32 = 0x8;
```
to:
```rust
                const PRESENT_OPTION_ASYNC_MAY_TEAR: u32 = 0x10;
```

(Leave the surrounding `req.options & !PRESENT_OPTION_ASYNC_MAY_TEAR` logic; `masked_options` is what Task 5 uses for async detection.)

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p yserver-core present`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver-core/src/core_loop/process_request.rs
git commit -m "fix(present): AsyncMayTear silent-clear used wrong bit (0x8->0x10)"
```

---

## Task 3: Validate divisor/remainder on both Pixmap handlers (blocker 7)

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs` — `PIXMAP` handler (near the parse, before the copy dispatch ~8850) and `PIXMAP_SYNCED` handler (~9040).

NotifyMSC already does this at ~8966 (`divisor != 0 && remainder >= divisor → BadValue`). Xorg also rejects `divisor == 0 && remainder != 0`. Add both checks to each Pixmap handler, before any pacing math or copy.

- [ ] **Step 1: Add the validation** (mirror the existing NotifyMSC block; use the request's parsed `req.divisor` / `req.remainder`):

```rust
            if (req.divisor == 0 && req.remainder != 0)
                || (req.divisor != 0 && req.remainder >= req.divisor)
            {
                return emit_x11_error_with_minor(
                    state,
                    client_id,
                    sequence,
                    x11::error::BAD_VALUE,
                    u32::try_from(req.remainder).unwrap_or(u32::MAX),
                    u16::from(header.data),
                    PRESENT_MAJOR_OPCODE,
                );
            }
```

Place it in both the `PIXMAP` and `PIXMAP_SYNCED` arms. Confirm `PRESENT_MAJOR_OPCODE` is in scope (the NotifyMSC arm uses it) — if not, use the local `const PRESENT_MAJOR_OPCODE: u8 = 145;`.

- [ ] **Step 2: Build**

Run: `cargo build -p yserver-core`
Expected: builds.

- [ ] **Step 3: Commit**

```bash
git add crates/yserver-core/src/core_loop/process_request.rs
git commit -m "feat(present): reject invalid divisor/remainder on PresentPixmap[Synced]"
```

---

## Task 4: Backend — retain the wake, add `present_id` + `signal_present_wake` (blocker 1)

**Files:**
- Modify: `crates/yserver-core/src/backend/trait_def.rs` (`CompletedPresentEvent` ~160; `Backend` trait method)
- Modify: `crates/yserver/src/kms/render/backend.rs` (`fire_pending_present_entry` ~5016; a new field on `KmsBackend`; trait impl)
- Modify every other `CompletedPresentEvent { .. }` literal (compiler-guided).

- [ ] **Step 1: Add `present_id` to `CompletedPresentEvent`**

In `trait_def.rs`, add to the struct (~line 165, before `wake`):

```rust
    /// Server-generated monotonic correlation id (from
    /// `ServerState::next_present_id`). Keys the backend's retained-wake
    /// map and core's pacing gate. Never 0 for a real completion.
    pub present_id: u64,
```

Fix the constructor sites (compiler lists them): `execute_present_pixmap_copy` (Task 5), the `PixmapSynced` handler (Task 5), the test in `present_completion.rs:89` (`present_id: 0`), and any `CompletedPresentEvent` in `trait_def.rs` tests (`present_id: 0`).

- [ ] **Step 2: Add the `Backend` trait method (default no-op)**

In `trait_def.rs`, next to `drain_completed_present_events`:

```rust
    /// Signal (and release) the pinned wake primitive for a previously
    /// drained completion. Core calls this once the display MSC has reached
    /// the request's target, or during teardown to release buffers. The
    /// backend triggers the retained xshmfence / release syncobj and drops
    /// its pin. Unknown / already-signalled ids are a no-op. Default no-op
    /// so non-v2 backends opt out.
    fn signal_present_wake(&mut self, _present_id: u64) {}
```

- [ ] **Step 3: Add the retained-wake map to `KmsBackend`**

Add a field (near other present-completion state, e.g. beside `pending_present_batches`):

```rust
    /// Pinned wakes drained to core but not yet signalled — held here so
    /// core can pace when the real xshmfence / syncobj fires. Keyed by
    /// `CompletedPresentEvent::present_id`.
    retained_present_wakes: std::collections::HashMap<u64, crate::kms::render::present_completion::PinnedWake>,
```

Initialise it to `HashMap::new()` in the `KmsBackend` constructor(s).

- [ ] **Step 4: Change `fire_pending_present_entry` to retain instead of signal**

Replace the body (`backend.rs:5016-5044`):

```rust
    fn fire_pending_present_entry(
        &mut self,
        entry: crate::kms::render::present_completion::PendingPresentEntry,
    ) -> yserver_core::backend::CompletedPresentEvent {
        // Do NOT signal here. Retain the pin keyed by present_id so core can
        // release it at the target vblank (frame pacing). PinnedWake::None
        // entries need no retention.
        // Destructure so `wake_pin` is moved exactly once (PinnedWake is not
        // Copy — a `matches!(entry.wake_pin, ..)` guard would move it first and
        // fail to compile).
        use crate::kms::render::present_completion::{PendingPresentEntry, PinnedWake};
        let PendingPresentEntry { wake_pin, event } = entry;
        if !matches!(wake_pin, PinnedWake::None) {
            self.retained_present_wakes.insert(event.present_id, wake_pin);
        }
        event
    }
```

- [ ] **Step 5: Implement `signal_present_wake` on the KMS `Backend` impl**

Add to the `impl Backend for KmsBackend` block (near `drain_completed_present_events` ~17588):

```rust
    fn signal_present_wake(&mut self, present_id: u64) {
        use crate::kms::render::present_completion::PinnedWake;
        let Some(pin) = self.retained_present_wakes.remove(&present_id) else {
            return;
        };
        match pin {
            PinnedWake::Pixmap(h) => {
                if let Err(e) = self.dri3_trigger_fence_via_handle(&h) {
                    log::warn!("signal_present_wake: dri3_trigger_fence_via_handle failed: {e}");
                }
            }
            PinnedWake::PixmapSynced { handle, value } => {
                if let Err(e) = self.dri3_signal_syncobj_via_handle(&handle, value) {
                    log::warn!("signal_present_wake: dri3_signal_syncobj_via_handle failed: {e}");
                }
            }
            PinnedWake::None => {}
        }
    }
```

Confirm `dri3_trigger_fence_via_handle` / `dri3_signal_syncobj_via_handle` take `&Arc<dyn ..>` (they did in `fire_pending_present_entry`); adjust `&h` / `&handle` to match their existing signatures.

- [ ] **Step 6: Handle the shutdown force-fire path**

`drain_completed_present_events_impl` on `renderer_failed` (and `take_shutdown_present_events`) previously signalled via `fire_pending_present_entry`. Now those entries retain instead. At shutdown, after draining, signal any still-retained wakes so buffers are released. Add, in the shutdown block that already calls `take_shutdown_present_events` / drains completions (search `shutdown_destroy_drawables` caller in `lib.rs`), a drain of `retained_present_wakes` (signal all). Minimal form — a helper:

```rust
    pub fn signal_all_retained_present_wakes(&mut self) {
        let ids: Vec<u64> = self.retained_present_wakes.keys().copied().collect();
        for id in ids {
            self.signal_present_wake(id);
        }
    }
```

Call it in the shutdown sequence before `shutdown_destroy_drawables`.

- [ ] **Step 7: Build**

Run: `cargo build -p yserver && cargo build -p yserver-core`
Expected: builds (all `CompletedPresentEvent` literals now set `present_id`).

- [ ] **Step 8: Commit**

```bash
git add crates/yserver-core/src/backend/trait_def.rs crates/yserver/src/kms/render/backend.rs crates/yserver/src/kms/render/present_completion.rs
git commit -m "feat(present): backend retains wake by present_id; add signal_present_wake"
```

---

## Task 5: Thread `present_id` + effective target through the request paths (blockers 2,3,4,6)

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs` — `PendingPresentPixmap` struct (~8892 usage; find its definition), `PIXMAP` handler, `execute_present_pixmap_copy` (8489), `PIXMAP_SYNCED` handler (~9040-9179).
- Depends on Task 6 (server state) for `state.next_present_id()`, `state.present_complete_gate`, `PresentCompleteGate`. Implement Task 6 first or in the same branch step.

- [ ] **Step 1: Widen `PendingPresentPixmap`**

Add two fields to the `PendingPresentPixmap` struct definition:

```rust
    /// Server-generated correlation id for this present's completion.
    pub present_id: u64,
    /// `Some(msc)` when the completion must be paced to that vblank
    /// (synced present, target in the future). `None` = complete asap.
    pub effective_target_msc: Option<u64>,
```

- [ ] **Step 2: Compute id + effective target in the `PIXMAP` handler**

Inside the valid `if let (Some(src), Some(dst))` block, after `masked_options` is known and before constructing `PendingPresentPixmap` (~8892), add:

```rust
                let present_id = state.next_present_id();
                let effective_target_msc = {
                    // Use the freshest kernel MSC: the mirror can lag a vblank
                    // that fired since the last drain (codex stale-clock edge).
                    // `0` means no vblank clock yet — pre-first-flip on KMS, or a
                    // nested/headless backend whose `present_get_ust_msc` is
                    // `(0,0)`. In that case leave the present UNPACED (complete
                    // asap): a nested client would otherwise park forever because
                    // `arm_idle_vblanks` is a no-op there. The residual pre-first-
                    // flip window on KMS is sub-frame (see Non-blocking notes).
                    let (m, u) = backend.present_get_ust_msc();
                    let current_msc = if m > 0 {
                        state.present_kernel_msc = m;
                        state.present_kernel_ust = u;
                        m
                    } else {
                        state.present_kernel_msc
                    };
                    if current_msc > 0 {
                        let eff = crate::present_scheduler::effective_target_msc(
                            req.target_msc,
                            current_msc,
                            req.divisor,
                            req.remainder,
                            masked_options,
                        );
                        // Pace only when the target is genuinely in the future
                        // (wrap-safe). async / already-satisfied → complete asap.
                        crate::present_scheduler::msc_is_after(eff, current_msc).then_some(eff)
                    } else {
                        None
                    }
                };
```

Set `present_id` and `effective_target_msc` in the `PendingPresentPixmap { .. }` literal.

- [ ] **Step 3: Set `present_id` + insert the gate in `execute_present_pixmap_copy`**

Destructure the two new fields at the top of `execute_present_pixmap_copy` (add `present_id,` and `effective_target_msc,` to the `let PendingPresentPixmap { .. } = pending;`). Set `present_id` in the `CompletedPresentEvent { .. }` literal (8567). Immediately **before** the `backend.enqueue_present_completion(` call (so the gate is created transactionally with the enqueue — never for a request that errors out earlier), add:

```rust
    if let Some(eff) = effective_target_msc {
        state.present_complete_gate.insert(
            present_id,
            crate::server::PresentCompleteGate {
                effective_target_msc: eff,
                owner: client_id,
                dst_window_xid: req.window,
            },
        );
    }
```

- [ ] **Step 4: Mirror for `PIXMAP_SYNCED`**

In the `PIXMAP_SYNCED` handler, inside its valid block, compute `present_id` + `effective_target_msc` the same way (Step 2). Set `present_id` in the `CompletedPresentEvent { .. }` literal at `:9168`, and insert the gate immediately before `backend.enqueue_present_completion(` at `:9167`, identical to Step 3.

- [ ] **Step 5: Build**

Run: `cargo build -p yserver-core`
Expected: builds (Task 6 must be present for `next_present_id`/`present_complete_gate`).

- [ ] **Step 6: Commit**

```bash
git add crates/yserver-core/src/core_loop/process_request.rs
git commit -m "feat(present): correlate completions by present_id; record pacing gate"
```

---

## Task 6: Server state — id counter, gate map, parked list

**Files:**
- Modify: `crates/yserver-core/src/server.rs` (types near `PendingNotifyMsc` ~804; fields ~1016; constructor ~1278; a helper method on `ServerState`)

- [ ] **Step 1: Add the types**

```rust
/// A Present completion whose wake+events are deferred until the display
/// reaches `effective_target_msc`. `event.present_id` correlates the
/// backend-retained wake primitive to signal at fire time.
#[derive(Debug, Clone)]
pub struct PendingPresentComplete {
    pub event: crate::backend::CompletedPresentEvent,
    pub effective_target_msc: u64,
}

/// Recorded at request time; consumed when the GPU copy completes to decide
/// park-vs-fire. Keyed by `present_id` in `present_complete_gate`.
#[derive(Debug, Clone, Copy)]
pub struct PresentCompleteGate {
    pub effective_target_msc: u64,
    pub owner: yserver_protocol::x11::ClientId,
    /// Destination window xid (`req.window`, == `event.dst_host_xid`), so a
    /// window-destroy purge can drop a gate whose copy has not completed yet.
    pub dst_window_xid: u32,
}
```

- [ ] **Step 2: Add fields (after `present_pending_msc`, ~1016)**

```rust
    /// Monotonic Present completion id source (starts at 1; 0 is "unset").
    pub present_next_id: u64,
    /// Per-in-flight-present pacing target, set at request time, consumed at
    /// GPU-copy completion. Keyed by `present_id`.
    pub present_complete_gate: std::collections::HashMap<u64, PresentCompleteGate>,
    /// Completions parked until their target-msc vblank.
    pub present_pending_complete: Vec<PendingPresentComplete>,
```

- [ ] **Step 3: Init in constructor (after `present_pending_msc: Vec::new(),`)**

```rust
            present_next_id: 1,
            present_complete_gate: std::collections::HashMap::new(),
            present_pending_complete: Vec::new(),
```

- [ ] **Step 4: Add the id helper (impl ServerState)**

```rust
    /// Allocate the next monotonic Present completion id (never 0).
    pub fn next_present_id(&mut self) -> u64 {
        let id = self.present_next_id;
        self.present_next_id = self.present_next_id.wrapping_add(1).max(1);
        id
    }
```

- [ ] **Step 5: Build**

Run: `cargo build -p yserver-core`
Expected: builds.

- [ ] **Step 6: Commit**

```bash
git add crates/yserver-core/src/server.rs
git commit -m "feat(present): server state for present_id gate + parked completions"
```

---

## Task 7: Pace at vblank — drain rewrite + fire helpers (blocker 1 core side)

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs` (add helpers near `fire_due_present_notify_msc` ~9600)
- Modify: `crates/yserver-core/src/core_loop/run.rs` (drain 1293-1318; arm 1327)

- [ ] **Step 1: Add `complete_present_now` + `fire_due_present_completions`**

```rust
/// Signal the client's retained wake (real xshmfence/syncobj) via the
/// backend, update X11 fence bookkeeping, then emit IdleNotify+CompleteNotify
/// (idle-before-complete, per Xorg). Stamps the complete event with the
/// current `present_kernel_msc/ust`. Used for both the immediate path and
/// the vblank drain.
pub(crate) fn complete_present_now(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    event: &crate::backend::CompletedPresentEvent,
) {
    use crate::backend::PresentWake;
    backend.signal_present_wake(event.present_id);
    if let PresentWake::Pixmap { idle_fence_xid } = event.wake
        && idle_fence_xid != 0
        && let Some(f) = state.sync_fences.get_mut(&idle_fence_xid)
    {
        f.triggered = true;
    }
    fire_present_completion_events(state, event);
}

/// Fire parked completions whose target MSC has been reached. Called from the
/// MSC-advance drain, alongside `fire_due_present_notify_msc`.
pub(crate) fn fire_due_present_completions(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    msc: u64,
    _ust: u64,
) {
    if msc == 0 || state.present_pending_complete.is_empty() {
        return;
    }
    let mut still_pending = Vec::new();
    for p in std::mem::take(&mut state.present_pending_complete) {
        // Due when msc has reached/passed the target (wrap-safe): NOT (target after msc).
        if !crate::present_scheduler::msc_is_after(p.effective_target_msc, msc) {
            complete_present_now(state, backend, &p.event);
        } else {
            still_pending.push(p);
        }
    }
    state.present_pending_complete = still_pending;
}
```

- [ ] **Step 2: Rewrite the completion drain in `run.rs`**

Replace `run.rs:1293-1305` with:

```rust
    let completed = backend.drain_completed_present_events();
    for entry in completed {
        // Pace: if this completion recorded a future target-msc gate, park the
        // whole thing (wake NOT signalled yet) until that vblank. Otherwise
        // (async / no clock / target already reached) complete now.
        // `present_kernel_msc` here is the previous iteration's value; the MSC
        // refresh + fire_due_present_completions below release anything due
        // this iteration.
        match state.present_complete_gate.remove(&entry.present_id) {
            Some(gate)
                if crate::present_scheduler::msc_is_after(
                    gate.effective_target_msc,
                    state.present_kernel_msc,
                ) =>
            {
                state
                    .present_pending_complete
                    .push(crate::server::PendingPresentComplete {
                        event: entry,
                        effective_target_msc: gate.effective_target_msc,
                    });
            }
            _ => {
                crate::core_loop::process_request::complete_present_now(state, backend, &entry);
            }
        }
    }
```

(The old `sync_fences[..].triggered = true` block is removed here — it now lives in `complete_present_now`, fired together with the real wake.)

- [ ] **Step 3: Call `fire_due_present_completions` after the MSC refresh**

Extend `run.rs:1313-1318`:

```rust
    let (msc, ust) = backend.present_get_ust_msc();
    if msc > 0 {
        state.present_kernel_msc = msc;
        state.present_kernel_ust = ust;
        crate::core_loop::process_request::fire_due_present_notify_msc(state, msc, ust);
        crate::core_loop::process_request::fire_due_present_completions(state, backend, msc, ust);
    }
```

- [ ] **Step 4: Extend `arm_idle_vblanks` (run.rs:1327)**

```rust
    if !state.present_pending_msc.is_empty() || !state.present_pending_complete.is_empty() {
        let mut targets: Vec<u64> = state
            .present_pending_msc
            .iter()
            .map(|p| p.target_msc)
            .collect();
        targets.extend(
            state
                .present_pending_complete
                .iter()
                .map(|p| p.effective_target_msc),
        );
```

(Rest of the block — `backend.arm_idle_vblanks(&targets)` + logging — unchanged.)

- [ ] **Step 5: Build + existing present tests**

Run: `cargo build -p yserver-core && cargo test -p yserver-core present`
Expected: builds; existing wire/notify tests green.

- [ ] **Step 6: Commit**

```bash
git add crates/yserver-core/src/core_loop/process_request.rs crates/yserver-core/src/core_loop/run.rs
git commit -m "feat(present): defer wake+completion to target-msc vblank"
```

---

## Task 8: Lifecycle purge (blocker 4 addendum)

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_disconnect.rs` (~299)
- Modify: the window-destroy path that already calls `state.present_scheduler.drain_window` (`process_request.rs:1370`).

- [ ] **Step 1: Purge on client disconnect**

Next to `state.present_pending_msc.retain(|p| p.owner != client_id);` (`process_disconnect.rs:299`), add — signalling the retained wakes first so the (dying) client's buffers are released, then dropping the entries:

```rust
    // Release + drop any parked/gated Present completions this client owns.
    for p in state
        .present_pending_complete
        .iter()
        .filter(|p| p.event.client_id == client_id)
    {
        backend.signal_present_wake(p.event.present_id);
    }
    state
        .present_pending_complete
        .retain(|p| p.event.client_id != client_id);
    for (&id, g) in state.present_complete_gate.iter() {
        if g.owner == client_id {
            backend.signal_present_wake(id);
        }
    }
    state.present_complete_gate.retain(|_, g| g.owner != client_id);
```

`backend: &mut dyn Backend` is in scope in `process_disconnect` (it already calls `finish_present_source_wait`). Pre-copy producer-wait presents are **already** purged on disconnect (`process_disconnect.rs:303-311` removes the `pending_present_pixmaps` entry + `finish_present_source_wait`); no idle-fence trigger is needed there because the client — and its fence resource — is gone. So disconnect only needs the gate + parked purge above; the window-destroy path (Step 2) is where the pre-copy wait needs the extra fence release (client still alive).

- [ ] **Step 1b: Regression test (deferred wait → destroy → producer ready)**

Add a test asserting that a `PresentPixmap` parked in `pending_present_pixmaps` whose destination window is then destroyed: (a) is removed from `pending_present_pixmaps`, (b) its idle fence is triggered, and (c) when the producer later becomes ready, `drain_ready_present_pixmaps` creates **no** copy and **no** gate (no orphan). Model it on the existing `pending_present_pixmaps` disconnect test (`process_request.rs:35103`).

- [ ] **Step 2: Purge on window destroy — single canonical release path**

The existing teardown loop at `process_request.rs:1369-1401` drains the (dead) `present_scheduler` queue and **directly signals each frame's idle fence/syncobj by XID** (`backend.dri3_trigger_fence` / `dri3_signal_syncobj`, lines 1374-1394). That is a *second* signal path that would double-fire with the new `signal_present_wake`. Make the `present_id` mechanism the only release path:

1. **Delete the legacy per-frame signalling block** (the `for frame in &drained { match frame.idle { .. } }` body at lines 1374-1394). Keep the `state.present_scheduler.drain_window(*window)` call so the dead accumulated queue is still cleared, but do not signal from it.

2. **For every destroyed window**, release+drop the new-mechanism state, placed **before** the `if drained.is_empty() { continue; }` early-out so it runs even for windows with no scheduler entries:

```rust
    for window in &order {
        let win_xid = window.0;
        // Pre-copy producer-wait presents (async source wait, `PresentPixmap`
        // only) for this window: the client is alive and may be blocked on the
        // idle fence, so release it by xid; drop the source-wait pin; and remove
        // the entry so it cannot execute + insert an orphan gate AFTER this
        // purge (codex r3 P1). These have no present_id gate / retained wake yet,
        // so the by-xid trigger is their sole release — no double-signal.
        let stale_waits: Vec<u64> = state
            .pending_present_pixmaps
            .iter()
            .filter_map(|(&wid, p)| (p.request.window == win_xid).then_some(wid))
            .collect();
        for wid in stale_waits {
            if let Some(p) = state.pending_present_pixmaps.remove(&wid) {
                let fence = p.request.idle_fence;
                if fence != 0 {
                    if let Err(e) = backend.dri3_trigger_fence(fence) {
                        log::warn!("PRESENT teardown: trigger idle fence 0x{fence:x} failed: {e}");
                    }
                }
            }
            backend.finish_present_source_wait(wid);
        }
        // Release + drop parked completions for this window (pin is retained;
        // signal fires the real wake so Mesa's WSI thread is not stuck).
        for p in state
            .present_pending_complete
            .iter()
            .filter(|p| p.event.dst_host_xid == win_xid)
        {
            backend.signal_present_wake(p.event.present_id);
        }
        state
            .present_pending_complete
            .retain(|p| p.event.dst_host_xid != win_xid);
        // Drop gates for in-flight copies not yet drained. signal_present_wake is
        // a no-op here (the pin is retained only once the copy completes), but
        // removing the gate means the later backend completion takes the
        // gate-absent arm in run.rs -> completes + signals exactly once, never
        // re-parking against a destroyed window.
        for (&id, g) in state.present_complete_gate.iter() {
            if g.dst_window_xid == win_xid {
                backend.signal_present_wake(id);
            }
        }
        state
            .present_complete_gate
            .retain(|_, g| g.dst_window_xid != win_xid);

        let drained = state.present_scheduler.drain_window(*window);
        if drained.is_empty() {
            continue;
        }
        // (legacy per-frame idle signalling removed — see step 2.1)
        log::debug!(
            "PRESENT teardown: cleared {} stale scheduler entry(ies) for destroyed window 0x{:x}",
            drained.len(),
            window.0
        );
    }
```

**Do NOT** add any `FreePixmap`-driven cancellation: an in-flight Present must retain its source until release; the backend already refcount-pins the source drawable.

- [ ] **Step 3: Build + clippy + fmt (CI-exact)**

```bash
cargo +nightly fmt
cargo build -p yserver-core && cargo build -p yserver
cargo clippy --all-targets -- -D warnings
cargo test -p yserver-core
```
Expected: all green, no warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/yserver-core/src/core_loop/process_disconnect.rs crates/yserver-core/src/core_loop/process_request.rs
git commit -m "feat(present): release+purge parked completions on disconnect/window destroy"
```

---

## Task 9: Full workspace verification

- [ ] **Step 1: fmt + clippy + tests across the workspace**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```
Expected: green, no warnings. (If workspace test runtime is prohibitive, run `-p yserver-core` and `-p yserver` first.)

- [ ] **Step 2: Commit any fmt-only churn**

```bash
git add -A && git commit -m "chore(present): fmt"
```

---

## Task 10: Hardware verification (the real gate)

Unit-green is not proof; the fix is validated on-box (memory `feedback_tests_are_not_visible_evidence`, `feedback_vng_pass_not_hw_pass`).

- [ ] **Step 1: Re-capture the Plasma splash on bee** with the same `YSERVER_LOOP_TELEMETRY` + submit-tsv env that produced `yserver-hw-plasma.log` / `yserver-plasma.submit.tsv`.

- [ ] **Step 2: Confirm the pacing collapse** during the splash:
- splash `PresentPixmap` in-rate and `CompleteNotify`/`IdleNotify` out-rate drop from ~500/s toward `page_flip/s` (~60/s).
- `copy_area` submits drop from ~500/s toward `scene_compose` (~60/s).
- spinner visibly spins at normal speed.
- **If the rate lands at a small multiple of 60** (e.g. 120–240) rather than ~60, that is the "no Skip-collapse" tail (see Non-blocking below): Mesa released several same-target frames per vblank. Record the measured multiple; the follow-up is same-window/same-target supersession + `CompleteNotify{Skip}` (`present_scmd.c:797`).

- [ ] **Step 3: Regression checks** (deferring the real idle_fence/syncobj is the risk surface):
- Vulkan WSI client (vkcube/vkgears or the in-sandbox vulkan acceptance path): renders past frame 2 without hanging, vsync-paced (this is the `vkAcquireNextImage` concern — now that idle is idle-before-complete AND paced, it must still not stall).
- Warframe windowed + fullscreen, Marco compositing on/off: renders, updates, no cursor-lag regression (`project_warframe_cursor_lag`).
- Steam (uses `PresentPixmapSynced`): library/pages render, no black-until-damaged regression (`project_steam_pbuffer_render_regression`).
- An `AsyncMayTear`/vsync-off client is still NOT paced (free-runs) — `effective_target_msc` returns `None`.
- cinnamon/xfce dogfood: normal compositing, no added input lag (`feedback_lag_decoupled_from_render_correctness`).

- [ ] **Step 4: Update status + memory**
- Append verification note to `docs/status.md`.
- Refresh memory `project_yserver_compose_responsiveness` (this subsumes the splash singleton-submit storm) and add a memory for this fix.

---

## Non-blocking notes (tracked, not silently dropped)

- **Startup pre-first-flip window:** when the backend has no MSC yet (`present_get_ust_msc() == (0,0)` — pre-first-flip on KMS, or always on nested/headless) a synced present completes immediately (unpaced). This is required: parking without a vblank clock would hang nested clients (their `arm_idle_vblanks` is a no-op). On KMS the window is sub-frame (the first pageflip retires almost immediately). Fully closing it would need an explicit "await first MSC" gate state — deferred; it cannot regress nested. (Task 5 queries the freshest MSC, which also closes codex's stale-clock edge.)
- **Skip-collapse not implemented:** every eager copy still gets a Copy completion at its target vblank. Mesa's `loader_dri3` spreads `target_msc` across frames (`priv->msc + interval*(send_sbc - recv_sbc)`), so steady state should approach 1/vblank; but a multi-buffer client that collapses targets can release N/vblank. Task 10 Step 2 measures this; the remedy (same-window/same-target supersession + `Skip`) is a follow-up, not part of this fix.
- **Pre-existing dead `present_scheduler` enqueue:** `process_request.rs:8936` enqueues every present into `present_scheduler` with no normal drain (`:8919` comment), so it accumulates until window destroy — wasteful at the current 500/s. This fix does not depend on it; once pacing lands the rate drops to ~60/s. Consider removing the dead enqueue in a follow-up (kept out of scope to keep this change focused).

## Self-Review

- **Blocker coverage:** 1→Task 4+7 (backend retains, core signals at vblank); 2/3→Task 5+6 (`present_id` correlation, not `dst_host_xid`/`serial`); 4→Task 5 (transactional gate at enqueue) + Task 8 (purge); 5→Task 2; 6→Task 5 (`PixmapSynced` handler mirrored); 7→Task 3.
- **Three disjoint completion populations, each released exactly once** (a present is only ever in one at a time): (1) **pre-copy producer-wait** — in `pending_present_pixmaps`, no gate/retained-wake yet → released by-xid (`dri3_trigger_fence`) only on teardown (disconnect already; window-destroy added in Task 8 Step 2); (2) **in-flight/gated** — copy submitted, gate keyed by `present_id`, pin not yet retained → released via `signal_present_wake` (no-op until the pin is retained; the gate is dropped so the later drain completes+signals once); (3) **parked** — pin retained, awaiting vblank → `signal_present_wake` at vblank / on purge. The legacy by-XID teardown at `process_request.rs:1374-1394` is **removed** (Task 8 Step 2.1), so post-copy presents have exactly one signal path. `signal_present_wake` is idempotent (remove→None); the by-xid pre-copy path never overlaps it (disjoint populations).
- **Type consistency:** gate keyed by `u64 present_id` at insert (Task 5) and lookup (Task 7); `PendingPresentComplete.event.present_id` feeds `signal_present_wake`; `PresentCompleteGate.owner: ClientId` matches disconnect purge; `PresentCompleteGate.dst_window_xid: u32` matches `event.dst_host_xid` (both `req.window`) for window-destroy purge.
- **Wrap-safety:** all MSC comparisons in the request/gate/drain paths use `msc_is_after` (Task 5/7), consistent with the Xorg-faithful `effective_target_msc` math.
- **Window-destroy race (codex r2):** a gate for a copy still in flight when its window is destroyed is dropped by `dst_window_xid` (Task 8 Step 2), so the late backend completion takes the gate-absent arm → completes+signals once, never re-parking against a dead window.
- **Ordering (codex-confirmed):** drain reads previous-iteration `present_kernel_msc`, then MSC refresh + `fire_due_present_completions` run same iteration → a completion whose target is this vblank fires this iteration, not one late.
