# Leak-fix branch regression + game-start transparency bug — findings (2026-08-11)

Status: **Investigation snapshot — mechanism confirmed by test, fix implemented in working tree (`dri3-syncobj-drm-signal`).**

## 1. Scope and evidence source

- Evidence log: `issues_logs/yserver-leak-cinnamon.log` (25 MB, 53,071 lines,
  session 21:48:28→22:56:32, ~68 min, Cinnamon/Muffin, RADV/AMD, 3440×1440@165 +
  1920×1080@60).
- The build under test is the **leak-fix branch** — commit `87922fcf`
  (`origin/diag/115-resource-population`), which carries all of:
  `8aca93ec` cursor refcounts, `a405e4a1` cursor retirement, `8bf3e5da` pixmap
  pool global cap, `74781f79` pool LRU, `856f0593` store-scan instrumentation,
  `24d97cd6` GC-pixmap orphan release, `87922fcf` measurement.
- User report (Victor): *"this time was way worse, performance was bad from the
  start, animations not smooth, starting a game got the transparency bug,
  alt-tab froze the desktop, could hear sounds but everything else not working,
  had to force quit."*

## 2. Performance regression — confirmed root cause

**Perpetual full-screen-redraw busy loop.** Structural in the KMS renderer,
present in base and leak-fix alike; the leak-fix did not fix it and the log
confirms it end-to-end.

- `damage_fraction=1.000` in **100 % of 4056 pacing windows** and
  `full_redraw_fallback/s > 0` in 100 % — every compose is a full-framebuffer
  redraw. `pick_repaint_region` (`kms/render/scene.rs`) returns
  `Repaint::Full(extent)` unconditionally (buffer-age logic disabled/commented).
- **~7.8× over-composition** (up to 13.6×): `submit_group_flush_reason_scene_compose/s`
  (~1500/s) averages 7.8× the `composite_submits/s` (~145/s).
- `iter/s` 1500–2700 with `page_flip/s` only ~145. `next_wakeup` returns
  `Some(now)` whenever `scene_wants_compose() && has_output_ready_for_submit()`
  → poll timeout 0 → busy-spin doing full scene builds that mostly produce nothing.
- **Recurring ~1.033 s page-flip stalls in 41.7 % of pacing windows** (median
  max-gap 683 ms). Stall windows correlate with higher over-composition ratio
  (9.5× vs 5.8×). This is the *"froze the desktop, could hear sounds, everything
  else not working"* signature. Present from the very first seconds (21:48:31).
- `GetImage` (op73) **500–770 ms CPU-fence readbacks** — 4 discrete core-loop
  freezes (21:56:23, 22:17:25, 22:37:44, 22:49:26), each a `wait_for_fences`
  blocking the single-threaded loop. Matches the leak-fix doc's known
  "three discrete core-loop freezes" (it also noted an isolated 675 ms op73 at
  game start).
- **Input freezes of minutes** — `gap_max` up to 319 s while the loop stays
  alive (e.g. 22:36:11).
- **Client outbound flood** — 17.5 MB pending → `client_io: outbound cap
  exceeded` → disconnect (22:49:26).
- **The leak itself is not fixed**: drawable store still grows to ~1950 (max),
  1242 xid-bound pixmaps, nominal retention 244 MB. Pool cap held
  (`live_entries` 1785 < 2048, 0 global evictions) but the store keeps growing.

### Why the leak-fix didn't help (and this is not primarily a leak bug)
The busy-loop gating, unconditional `Repaint::Full`, and `scene_wants_compose`
predicate are **identical in base, leak-fix, and current working tree**. The
visible degradation is the render pipeline doing ~8–13× the work it needs while
the loop never sleeps. The GC-orphan-release added O(N) per-GC-op scans
(pixmaps+windows+gcs) but the store-scan cost is small (~0.23–0.35 % wall);
the leak is real but not the dominant cost driver.

## 3. Game-start transparency bug — confirmed mechanism

**The GC-pixmap orphan-release (`24d97cd6`) can destroy a backing that a Render
Picture still references**, when the Picture did not take a store ref at
create time.

### Mechanism (static-verified in code)

1. `handle_free_pixmap` keeps a pixmap alive only when
   `host_xid_referenced_by_window_bg || host_xid_referenced_by_gc`. Render
   Picture references (`PictureState.host_owned_pixmap` / `drawable`) are
   **not** consulted.
2. `orphaned_host_pixmaps` (leak-fix only) runs on every `ChangeGC` / `CopyGC` /
   `SetClipRectangles` / `FreeGC` / XFixes `SetGCClipRegion`. It releases a GC's
   dropped clip/tile/stipple host pixmap when no pixmap resource / window-bg /
   GC references it — again without consulting `pictures`.
3. The only backstop is the KMS store refcount: `render_create_picture`
   (`kms/render/backend.rs`) does `store.lookup(drawable_xid) → store.incref`.
   That incref is **conditional on the drawable already being in the store at
   picture-create time**.
4. **Hole**: if the backing is not yet materialized in the store when the
   Picture wraps it (game start: window map + redirect before backing alloc, or
   GLX-TFP / Present / DRI3 import), the Picture holds no store ref. A later
   GC-churn orphan-release calls `backend.free_pixmap`, the store decref hits
   zero, and the drawable is destroyed out from under the Picture →
   **transparent window**.
5. Log trigger window: game start at 22:16–22:17 shows op55 (CreateGC) /
   op59 (SetClipRectangles) / op60 (FreeGC) at **2000–4000/s**, windows
   313→361, store population →1112 drawables. The per-GC-op orphan scan runs
   hot exactly then.

### Distinction (important)
- The orphan-release path **exists only on the leak-fix branch**. The current
  working tree (`dri3-syncobj-drm-signal`) has `change_gc`/`free_gc` returning
  `()` — no GC-time host-pixmap release. So **this specific transparency
  trigger is leak-fix-specific**, matching *"after the changes and attempts made
  to solve the problem"*.
- The deeper pre-existing gap (FreePixmap not consulting Picture refs) exists in
  both, but only the leak-fix's GC-orphan-release turns GC churn into a live
  free.

## 4. Open questions / next steps

- Confirm the picture-before-backing timing hole with a focused workspace test
  (cross-layer: `yserver-core::resources` orphan gate vs `kms::render` store
  refcount). Needs a real run or a crafted unit test; a live Cinnamon/game run
  would also show the transparency on screen.
- Plan the fix:
  - Gate the orphan-release on Render Picture references (`pictures` map,
    `host_owned_pixmap`, and `store.lookup` present at release time).
  - Or make `render_create_picture` retain the store ref regardless of store
    materialization timing (defer/backfill).
  - Separately: the busy-loop (over-composition + unconditional
    `Repaint::Full` + `scene_wants_compose` never settling) is the primary perf
    defect; the leak-fix did not address it.

### 4a. UPDATE (2026-08-11, resumed on tty2): mechanism confirmed + fix landed

**The timing hole is confirmed and fixed in the working tree.**

- New test `picture_before_backing_pins_backing_on_late_materialization`
  (`kms/render/backend.rs`): a Picture over a host xid with no store entry yet
  takes **no** incref; when the backing materializes later, `free_pixmap` drops
  the owning ref to 0 and destroys the drawable under the live Picture. It fails
  with the pre-fix code (`apply_pending_picture_refs` no-op → refcount stays 1)
  and passes with the fix.
- Fix (structural, branch-agnostic): `render_create_picture` now records a
  **deferred store ref** (`pending_picture_drawable_refs`, picture_xid →
  drawable_xid) when the backing is not yet in the store. Every production
  backing-materialization path goes through the new `store_alloc` wrapper, which
  calls `apply_pending_picture_refs(xid)` after `store.allocate` to incref the
  entry on behalf of any picture that wrapped the xid early. `render_free_picture`
  drops the pending entry (no decref) if the backing never materialized.
- Because the orphan-release (leak-fix) and `handle_free_pixmap` both funnel into
  `backend.free_pixmap` → `store.decref`, a picture that now ALWAYS holds a store
  ref (as soon as the backing exists) survives both paths: refcount 2→1 on free,
  destroyed only on `render_free_picture`. This covers the leak-fix GC-churn
  trigger as well if/when the fix is ported to that branch.
- Companion test `picture_freed_before_materialization_leaves_no_pending_ref`
  pins the no-leak path: a picture freed before materialization leaves no pending
  entry and the later `free_pixmap` still destroys normally.
- `host_owned_pixmap` is dead: never assigned `Some` anywhere in the tree; the
  `free_picture` host-owned-pixmap branch (process_request.rs:1702) never fires.
  The store-ref path is the real lifetime mechanism.
- Verification: `cargo test -p yserver --lib` 782 pass, `yserver-core --lib`
  1141 pass, `cargo clippy --all-targets -- -D warnings` clean, `cargo +nightly fmt`.
- **Outstanding**: the busy-loop perf defect (over-composition + unconditional
  `Repaint::Full` + `scene_wants_compose` never settling) is unchanged and is the
  dominant user-visible issue; port the store-ref fix to `origin/diag/115-resource-population`
  if that branch is still in play; a live Cinnamon/game run to see transparency
  gone on screen.

## 5. Files touched by the analysis (reference points)

- `crates/yserver-core/src/resources.rs` — `orphaned_host_pixmaps`,
  `host_xid_referenced_by_gc`, `handle_free_pixmap` call sites.
- `crates/yserver-core/src/core_loop/process_request.rs` —
  `release_orphaned_host_pixmaps`, GC handlers (leak-fix branch).
- `crates/yserver/src/kms/render/backend.rs` — `render_create_picture`
  (conditional `store.incref`), `free_pixmap`.
- `crates/yserver/src/kms/render/store.rs` — refcount, `decref` / `incref`,
  xid detach on PendingFence.
- `crates/yserver/src/kms/render/scene.rs` — `pick_repaint_region` returns
  `Repaint::Full` unconditionally.
- `crates/yserver/src/kms/render/backend.rs` — `next_wakeup` busy-loop gate.
- `docs/status.md` / leak-fix branch docs — prior #115 narrative.
