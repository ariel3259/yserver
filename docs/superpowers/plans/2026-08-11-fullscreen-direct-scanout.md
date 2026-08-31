# No-Vsync Fullscreen Game Stutter — Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A no-vsync fullscreen game keeps `page_flip/s` at refresh without
throttling an uncapped async producer to refresh. Phase A preserves Xorg-style
target identity and target-scoped supersession; Phase B sends fullscreen games
to KMS direct scanout.

**Architecture:** Two independent subsystems, sequenced.
- **Phase A:** every clocked Present retains its Xorg-style effective target.
  Immediate async Presents use `Some(current_msc)` and execute into the
  backend's bounded successor path even while a flip is in flight. `None` is
  reserved for genuinely unclocked domains. Supersession continues to require
  the same CRTC and known effective target MSC; the async option bit alone
  never establishes equivalence. Spec:
  `docs/superpowers/specs/2026-08-11-async-present-defer-supersession.md`.
- **Phase B (EFFICIENCY, the user's stated direct-scanout goal):** fullscreen unredirected windows never direct-scanout because of three eligibility checks (`authoritative_root`, `try_present_direct`, and `maybe_probe_scanout_m1`). Relax them for authoritative-root candidates.

**Tech Stack:** Rust, KMS/DRM atomic, Vulkan, yserver-core `Backend` trait, `present_scheduler`.

## Global Constraints

- TDD: failing test → run → implement → run again. CI gate at the end:
  `cargo clippy --all-targets -- -D warnings` and `cargo +nightly fmt`.
- Feature branch off `dri3-syncobj-drm-signal`: `git checkout -b fix/fullscreen-novsync-stutter`.
- **Synced-present behavior must be bit-for-bit unchanged** — both phases touch only the async / authoritative-root paths.
- Test fixtures have no Vk/DRM: `for_tests()` cursor is `Sw` (scene.rs:896-899) and `dri3_syncobjs` is empty (backend.rs:3251). **Never write fixture-integration tests for scanout-m1 or syncobj wiring** — test pure predicates, verify the rest on hardware (Phase A Task 3, Phase B Task 8).
- Commit after each task. Reference: `docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md` (§4a, §4b), spec `docs/superpowers/specs/2026-08-11-async-present-defer-supersession.md`.

---

## Phase A — Async target identity + supersession (PRIMARY)

> **Correction, 2026-08-31:** Warframe hardware validation invalidated the
> original Task 1 below. Parking clocked async requests as `None` held one
> client buffer per vblank and limited the game to exactly 60 FPS. The
> replacement is to retain `Some(current_msc)` for clocked immediate async
> requests, execute them immediately into the backend successor slot, and use
> `None` only for no-clock domains. The original task is retained below as
> historical implementation context and MUST NOT be re-applied.

### Task 1 (superseded): Park async presents while a flip is in flight

**Files:**
- Modify: `crates/yserver-core/src/present_scheduler.rs:93-96` (`classify_msc_due`)
- Test: same file, near `classify_msc_due_no_clock_always_executes_now` (~167)

**Interfaces:**
- Consumes: `classify_msc_due(eff: Option<u64>, clock_msc: u64, flip_in_flight: bool) -> MscDue` (unchanged signature).
- Produces: `classify_msc_due(None, _, true) == MscDue::Park`; `classify_msc_due(None, _, false) == MscDue::ExecuteNow`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn classify_msc_due_async_parked_when_flip_in_flight() {
    assert_eq!(super::classify_msc_due(None, 10, true), super::MscDue::Park);
}

#[test]
fn classify_msc_due_async_executes_now_without_flip_in_flight() {
    assert_eq!(super::classify_msc_due(None, 10, false), super::MscDue::ExecuteNow);
}
```

- [ ] **Step 2: Run them, expect FAIL**

Run: `cargo test -p yserver-core --lib classify_msc_due_async`
Expected: FAIL — `classify_msc_due(None, 10, true)` returns `ExecuteNow`.

- [ ] **Step 3: Implement**

```rust
pub fn classify_msc_due(eff: Option<u64>, clock_msc: u64, flip_in_flight: bool) -> MscDue {
    let Some(eff) = eff else {
        // Async present (PresentOptionAsync): cannot flip before the current
        // in-flight flip retires. Park until that dependency clears instead
        // of submitting another Copy immediately.
        // Nested/headless runs always report flip_in_flight == false, so the
        // no-clock "always now" behavior is preserved there.
        return if flip_in_flight { MscDue::Park } else { MscDue::ExecuteNow };
    };
    // ...existing eff Some(_) body unchanged...
}
```

- [ ] **Step 4: Update the existing test and run**

`classify_msc_due_no_clock_always_executes_now` (present_scheduler.rs:167-172) asserts **two** cases:
- line 170 `classify_msc_due(None, 0, false) == ExecuteNow` — survives (no flip in flight);
- line 171 `classify_msc_due(None, 12345, true) == ExecuteNow` — **now FAILS** (flip in flight → `Park`).

Update it to pin the new behavior and rename:

```rust
#[test]
fn classify_msc_due_async_parked_while_flip_in_flight() {
    // Async present with a flip in flight: park until the dependency clears.
    assert_eq!(classify_msc_due(None, 12345, true), MscDue::Park);
    // No flip in flight: execute now (also the nested/headless no-clock path).
    assert_eq!(classify_msc_due(None, 0, false), MscDue::ExecuteNow);
}
```

Run: `cargo test -p yserver-core --lib classify_msc_due`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver-core/src/present_scheduler.rs
git commit -m "feat(present): park async presents while a flip is in flight"
```

---

### Task 2: Preserve target-scoped supersession

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs`
- Test: same file, using `present_pending_entry_with` and
  `RecordingBackend.present_skip_count`.

The 2026-08-28 review corrected the original plan: Xorg's scrap identity
requires both CRTC and target MSC equality. Keep the early return for
`effective_target_msc = None`, require the predecessor to share the same CRTC,
CRTC epoch, and `Some(target)`, and leave the existing conservative coverage
predicate in place. The async option bit is not a grouping key.

- [ ] Add an async/async regression where equal `Some(target)` requests
  supersede.
- [ ] Add an async/async regression where distinct `Some(target)` requests do
  not supersede.
- [ ] Add an async/async regression where both targets are `None` and verify no
  supersession occurs.
- [ ] Run `cargo test -p yserver-core supersede`.

---

### Task 3: Phase A validation on hardware

**Files:**
- Create: append result to `docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md`.

- [ ] **Step 1: Rebuild and launch the telemetry session**

```bash
cargo build --release --bin yserver
setsid nohup env RUST_LOG=info RUST_BACKTRACE=1 YSERVER_LOOP_TELEMETRY=1 \
  YSERVER_SUBMIT_TRACE=yserver-cinnamon.submit.tsv \
  target/release/yserver > yserver-hw-cinnamon.log 2>&1 &
# + dbus-run-session cinnamon-session (DISPLAY=:7), per yserver-cinnamon-hw recipe
```

- [ ] **Step 2: Play CS2 fullscreen with vsync OFF, moving the mouse, ~3 min**

- [ ] **Step 3: Verify the collapse is gone (PRIMARY acceptance)**

Run: `grep "loop telemetry" yserver-hw-cinnamon.log | grep -oE "page_flip/s=[0-9.]+" | tail -40`
Expected: `page_flip/s` holds ≈ refresh (60) through the no-vsync window.

Also confirm supersession now fires:
Run: `grep -c "present_skips/s=[1-9]" yserver-hw-cinnamon.log` (or grep the render telemetry for skips)
Expected: `> 0` during the flood.

- [ ] **Step 4: Confirm vsync-on is unchanged (synced path regression check)**

Play ~1 min with vsync ON. Expected: fluid, `page_flip/s` = refresh, and no new supersession of synced presents (`present_skips/s` ≈ 0 in that window).

- [ ] **Step 5: Record results in the findings doc (Phase A row), commit**

```bash
git add docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md
git commit -m "docs(scanout): record async-defer validation result"
```

**Gate:** if Phase A holds `page_flip/s` at refresh, proceed to Phase B. If it does NOT, STOP and re-investigate the classify/supersession path before touching scanout gates.

---

## Phase B — Fullscreen direct scanout (EFFICIENCY)

### Task 4: Accept `Unredirected` targets as authoritative root

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:117-122` (`scanout_m2_is_authoritative_root`)
- Test: same file, `scanout_m2_authoritative_root_accepts_unredirected_fullscreen` (new) + update `scanout_m2_only_authoritative_root_present_invalidates_direct_frame` (~32069).

- [ ] **Step 1: Add a failing test and update the existing one**

```rust
#[test]
fn scanout_m2_authoritative_root_accepts_unredirected_fullscreen() {
    use super::ScanoutM0Target;
    assert!(super::scanout_m2_is_authoritative_root(
        ScanoutM0Target::Unredirected,
        true
    ));
    assert!(!super::scanout_m2_is_authoritative_root(
        ScanoutM0Target::Unredirected,
        false
    ));
}
```

In `scanout_m2_only_authoritative_root_present_invalidates_direct_frame` (~32084), flip the existing assertion:
`assert!(!scanout_m2_is_authoritative_root(Unredirected, true))` → `assert!(...)`.

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p yserver --lib scanout_m2_authoritative_root_accepts_unredirected_fullscreen`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
fn scanout_m2_is_authoritative_root(target: ScanoutM0Target, root_coverage: bool) -> bool {
    matches!(
        target,
        ScanoutM0Target::Cow
            | ScanoutM0Target::CowDescendant
            | ScanoutM0Target::Unredirected
    ) && root_coverage
}
```

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p yserver --lib scanout_m2_authoritative_root`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs
git commit -m "feat(scanout): treat fullscreen Unredirected windows as authoritative root"
```

---

### Task 5: Drop explicit-sync / update-region gates in `try_present_direct`

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:13000-13013` (`eligible` predicate)
- Test: same file, new test for the factored pure predicate.

**Interfaces:**
- Consumes: `scanout_direct_eligible` (new, below); `try_present_direct` locals.
- Produces: `scanout_direct_eligible(...) -> bool` — pure predicate that does NOT consult `explicit_sync`, `update_region_xid`, or `update_is_full` (they are intentionally ignored for authoritative-root fullscreen presents; the acquire fence is awaited upstream via `source_ready`).

- [ ] **Step 1: Add the pure predicate + failing test**

```rust
fn scanout_direct_eligible(
    scanout_allowed: bool,
    kms_outputs_active: bool,
    cursor_hw: bool,
    root_overlay_empty: bool,
    authoritative_root: bool,
    x_off: i16,
    y_off: i16,
    valid_region_xid: u32,
) -> bool {
    scanout_allowed
        && kms_outputs_active
        && cursor_hw
        && root_overlay_empty
        && authoritative_root
        && x_off == 0
        && y_off == 0
        && valid_region_xid == 0
        // explicit_sync and update_region/update_is_full are intentionally NOT
        // consulted: an authoritative-root (fullscreen) present replaces the
        // whole scanout buffer, and the acquire fence is already awaited
        // (source_ready) before try_present_direct runs.
}

#[test]
fn scanout_direct_eligible_accepts_fullscreen_game_candidate() {
    assert!(super::scanout_direct_eligible(true, true, true, true, true, 0, 0, 0));
    assert!(!super::scanout_direct_eligible(true, true, true, true, false, 0, 0, 0));
    assert!(!super::scanout_direct_eligible(true, true, false, true, true, 0, 0, 0));
    assert!(!super::scanout_direct_eligible(true, true, true, true, true, 1, 0, 0));
}
```

- [ ] **Step 2: Run, expect FAIL (function undefined)**

Run: `cargo test -p yserver --lib scanout_direct_eligible`
Expected: FAIL — `scanout_direct_eligible` not found.

- [ ] **Step 3: Implement + wire the call site**

Add `scanout_direct_eligible` next to `scanout_m2_is_authoritative_root`. Replace the inline `eligible` expression at backend.rs:13000-13013:

```rust
let eligible = scanout_direct_eligible(
    self.scanout_allowed(),
    self.kms_outputs_active,
    matches!(
        self.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw
    ),
    self.scene.root_overlay.is_empty(),
    authoritative_root,
    candidate.x_off,
    candidate.y_off,
    candidate.valid_region_xid,
);
```

- [ ] **Step 4: Run tests, expect PASS**

Run: `cargo test -p yserver --lib scanout_direct_eligible && cargo test -p yserver --lib scanout_m2`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs
git commit -m "feat(scanout): allow fullscreen explicit-sync presents on the direct path"
```

---

### Task 6: Probe m1 for `Unredirected` fullscreen sources

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:1437-1454` (`maybe_probe_scanout_m1` guard)
- Test: same file, new test for the factored pure guard predicate.

**Interfaces:**
- Consumes: `scanout_m1_probe_eligible` (new, below); `ScanoutM0Target`, `ScanoutM0Coverage`.
- Produces: `scanout_m1_probe_eligible(...) -> bool` — accepts `Cow | CowDescendant | Unredirected` targets with `Root` coverage; does NOT consult `update_region_xid`/`update_is_full`.

- [ ] **Step 1: Add the pure guard predicate + failing test**

```rust
fn scanout_m1_probe_eligible(
    scanout_allowed: bool,
    kms_outputs_active: bool,
    cursor_hw: bool,
    root_overlay_empty: bool,
    target: ScanoutM0Target,
    coverage: ScanoutM0Coverage,
    x_off: i16,
    y_off: i16,
    valid_region_xid: u32,
) -> bool {
    scanout_allowed
        && kms_outputs_active
        && cursor_hw
        && root_overlay_empty
        && matches!(
            target,
            ScanoutM0Target::Cow
                | ScanoutM0Target::CowDescendant
                | ScanoutM0Target::Unredirected
        )
        && matches!(coverage, ScanoutM0Coverage::Root)
        && x_off == 0
        && y_off == 0
        && valid_region_xid == 0
}

#[test]
fn scanout_m1_probe_eligible_accepts_unredirected_fullscreen() {
    use super::{ScanoutM0Coverage, ScanoutM0Target};
    assert!(super::scanout_m1_probe_eligible(
        true, true, true, true,
        ScanoutM0Target::Unredirected, ScanoutM0Coverage::Root, 0, 0, 0,
    ));
    assert!(!super::scanout_m1_probe_eligible(
        true, true, true, true,
        ScanoutM0Target::Other, ScanoutM0Coverage::Root, 0, 0, 0,
    ));
    assert!(!super::scanout_m1_probe_eligible(
        true, true, true, true,
        ScanoutM0Target::Unredirected, ScanoutM0Coverage::None, 0, 0, 0,
    ));
    assert!(!super::scanout_m1_probe_eligible(
        true, true, false, true,
        ScanoutM0Target::Unredirected, ScanoutM0Coverage::Root, 0, 0, 0,
    ));
}
```

- [ ] **Step 2: Run, expect FAIL (function undefined)**

Run: `cargo test -p yserver --lib scanout_m1_probe_eligible`
Expected: FAIL — not found.

- [ ] **Step 3: Implement + wire the call site**

Add `scanout_m1_probe_eligible` next to the other scanout predicates. In `maybe_probe_scanout_m1`, replace the guard block (backend.rs:1437-1454) with:

```rust
if !scanout_m1_probe_eligible(
    self.scanout_allowed(),
    self.kms_outputs_active,
    matches!(
        self.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw
    ),
    self.scene.root_overlay.is_empty(),
    target,
    coverage,
    candidate.x_off,
    candidate.y_off,
    candidate.valid_region_xid,
) {
    return;
}
```

(Remove the `candidate.update_region_xid != 0` and `!candidate.update_is_full` guard lines — they are no longer in the predicate. The probe does not consult `explicit_sync` either, matching today.)

- [ ] **Step 4: Run, expect PASS; keep the existing m1 guard test green**

Run: `cargo test -p yserver --lib scanout_m1_probe_eligible`
Run: `cargo test -p yserver --lib scanout_m1` (the existing guard test at ~32023 uses `Other` target / `Output` coverage / `valid_region_xid != 0`, which still guard)
Expected: new PASS, existing PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs
git commit -m "feat(scanout): pre-probe Unredirected fullscreen sources for direct scanout"
```

---

### Task 7: Verify explicit-sync fence ordering (no new plumbing expected)

**Files:**
- Test: run existing suites; no new fixture test (fixture has no DRM syncobj).

**Interfaces:**
- Consumes: the acquire-before-execute ordering described in the findings §4b.

- [ ] **Step 1: Confirm the ordering is pinned by existing tests**

`try_present_direct` has a single production call site: `execute_present_pixmap_copy` (process_request.rs:9144, the `match backend.try_present_direct(...)` at 9238), reached only via `execute_present_pixmap_copy_or_reroute` (9352) — which every execute path gates on `source_ready` (arrival on `PresentSourceWait::Ready` only; wait resolution; due-pass filter at 9046). Run the present suites that exercise parking/resolution:

Run: `cargo test -p yserver-core --lib present_pending_exec`
Run: `cargo test -p yserver-core --lib drain_due_present_pending_exec`
Run: `cargo test -p yserver-core --lib source_wait`
Expected: PASS. (These pin that a not-ready PixmapSynced present parks and does NOT reach execution/direct.)

- [ ] **Step 2: Note the release-wake coverage boundary**

The release syncobj signal (`signal_present_wake` → `dri3_signal_syncobj_via_handle`, backend.rs:19656) needs a real DRM syncobj — the fixture's `dri3_syncobjs` is empty. Hardware coverage happens in Task 8 (watch for missed release syncobj signals while a fullscreen direct game runs). Do NOT add a fixture-integration test here.

- [ ] **Step 3: Commit (tests already committed above; if nothing changed, skip)**

```bash
git status --short   # nothing expected; if a test was added, commit it
```

---

### Task 8: Phase B validation on hardware

**Files:**
- Create: append Phase B result to the findings doc.

- [ ] **Step 1: Rebuild release, relaunch telemetry session (as Task 3)**

- [ ] **Step 2: Play CS2 fullscreen vsync OFF ~3 min; also drag a desktop window to check composed path**

- [ ] **Step 3: Verify direct scanout engaged**

Run: `grep -c "scanout_m2: live direct submit" yserver-hw-cinnamon.log`
Expected: `> 0`.
Run: `grep -oE "m1_probe_pass=[0-9]+" yserver-hw-cinnamon.log | tail -1`
Expected: `> 0`.
Run: `grep "loop telemetry" yserver-hw-cinnamon.log | grep -oE "page_flip/s=[0-9.]+" | tail -40`
Expected: `page_flip/s` still at refresh (Phase A + B both active).

- [ ] **Step 4: Check the desktop still composes (no unflip thrash)**

Run: `grep -c "scanout_m2: stopped after scanout replacement" yserver-hw-cinnamon.log`
Expected: small number (cursor/overlay exits), not a flood. Dragging a window must work without stutter.

- [ ] **Step 5: Watch for release-fence regressions**

If the game later shows recycled/garbage frames after direct scanout engaged, the release syncobj is not being signaled after the direct flip retires — record it in the findings doc; do not claim success.

- [ ] **Step 6: Record Phase B result in the findings doc, commit**

```bash
git add docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md
git commit -m "docs(scanout): record fullscreen direct-scanout validation"
```

**Known blockers this task may reveal (do NOT silently skip):**
- **Modifier rejected by kernel**: if `m1_probe_pass` stays 0 because the game's `XR24` / `0x300000000606014` tiling is not scanout-capable on the primary plane, direct scanout cannot engage on this GPU. Phase A already fixes the user-visible stutter; record this as a follow-up (a different scanout format / plane is out of scope).
- **Software cursor / overlay**: if the CS2 session runs with cursor `Sw` or a root overlay, both `eligible` and the m1 guard block — record it; direct scanout needs the Hw-cursor precondition.

---

### Task 9: CI gate + branch finish

- [ ] **Step 1: Clippy exactly as CI**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 2: Format**

Run: `cargo +nightly fmt`

- [ ] **Step 3: Full test suites**

Run: `cargo test -p yserver --lib && cargo test -p yserver-core --lib`
Expected: all pass.

- [ ] **Step 4: Ask the user for confirmation to squash-merge** (AGENTS.md)

```bash
git log origin/dri3-syncobj-drm-signal..HEAD --oneline
```
