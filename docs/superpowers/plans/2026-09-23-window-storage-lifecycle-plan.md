# Window storage only while viewable (A) — implementation plan

Implements part A of `../specs/2026-09-23-window-storage-lifecycle-design.md`
(reviewed by codex, `3ffb770a`). Read that first; this plan does not restate
its reasoning. Option 2 is out of scope.

**Branch:** `feat/window-storage-lifecycle`.

**Deliverable:** a window owns storage only while it is viewable. On the
reporter's awesome session, windows on hidden tags and minimised apps cost
no VRAM; `vram by use` shows `window=` tracking the visible set.

## Ordering principle

**Make every consumer cope with "no storage" before anything stops
allocating it.** Steps 1–4 each change one thing while every window still
owns storage, so a regression in any of them is attributable and none of
them can crash on a missing image. Step 5 is the only step that frees
storage, and by then it is mostly deletion of the unconditional allocation.

## Prerequisites

- Gates before every commit, exactly CI's lines: `cargo +nightly fmt --
  --check`, `cargo clippy --all-targets -- -D warnings` (also with
  `--features tcp-transport` and `--features xdmcp`), `cargo test`.
- Lavapipe ignored suite: `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json
  cargo test -p yserver --lib -- --ignored`, plus `--test render_acceptance
  -- --ignored`.
- xts A/B per step (Xlib4 + Xlib9, zero PASS→FAIL), measured against our own
  previous run on this branch, not against Xorg.
- `feat/vram-by-category` merged first (HW-checked on bee): step 6 measures
  with its `vram by use` line.

## Step 1 — Core: `ViewabilityDelta` (no consumer yet)

Pure refactor; no behaviour change.

- `resources.rs`: `map_window` (`:1257`), `unmap_window` (`:1374`) and
  `reparent_window` (`:1687`) return a `ViewabilityDelta { became_viewable,
  became_unviewable }` instead of discarding the lists built by
  `promote_unviewable_descendants` (`:1302`) and `demote_viewable_descendants`
  (`:1342`); see the discarded vectors at `:1398-1400` and `:1753-1758`.
  Fold `map_window_with_promoted_descendants` (`:1262`) into it.
- `process_request.rs`: `handle_map_window` (`:24563`),
  `handle_map_subwindows` (`:24765`), `handle_unmap_window` (`:24851`),
  `handle_unmap_subwindows` (`:24935`) and `handle_reparent_window`
  (`:20397`) receive the delta. The Subwindows handlers union the per-child
  deltas. **Fix while here:** `handle_map_subwindows` calls plain
  `map_window` per child (`:24791`), so grandchild promotions never surface
  today.
- The existing damage reset that uses `promoted_descendants`
  (`:24740-24743`) switches to `delta.became_viewable`.
- Tests (core, no Vulkan): map/unmap/reparent/MapSubwindows/UnmapSubwindows
  of a frame with nested children, some unmapped, yield the exact sets. An
  already-viewable remap and an unmap of an unviewable window yield empty
  deltas.

## Step 2 — Core: protocol results for unviewable windows

Correct today already, with the storage still present.

- **CopyArea / CopyPlane:** `copy_area_source_split` (`process_request.rs:26752`)
  treats an unviewable source *window* as wholly missing, so `emit_copy_exposures`
  (`:26828`, called at `:27032`) sends GraphicsExpose for the whole
  destination area and no NoExpose. Same shape in `handle_copy_plane`
  (`:27278`, `:27333-27347`). Pixmap sources are unaffected.
- **Present to an unviewable window:** before `try_present_direct`
  (`:10514`), skip direct scanout and the copy (`:10543`, `:10556`); record no
  damage; still `enqueue_present_completion` (`:10603`) with the completion
  built at `:10479-10494` (CompleteModeCopy).
- **Drawing damage:** already gated on `map_state != Viewable` in
  `accumulate_damage_to_state` (`damage_fanout.rs:315-319`). Add the test
  only.
- Tests: CopyArea from an unmapped window with graphics-exposures on →
  GraphicsExpose covering the destination rect; Present to an unmapped
  window → idle + complete, no damage, no backend copy; PolyFill on an
  unmapped window → no DamageNotify.
- Check the Present behaviour against `../xserver/present/` before
  committing; the design records Xorg's result.

## Step 3 — Pictures on windows

- Backend: `render_create_picture` (`backend.rs:24307`) stops increfing the
  window's leaf (`:24327-24329`) and the `pending_picture_drawable_refs`
  deferral (`:24337`, `apply_pending_picture_refs` `:9208`) goes away for
  window drawables. A window Picture resolves its window at each use through
  `resolve_paint_target`; `None` clips the operation away.
  `render_free_picture` (`:24422`) no longer decrefs for window Pictures.
- Core destruction: `destroy_window_subtree` (`process_request.rs:1748`)
  already snapshots the subtree into `pending` (`:1769-1792`) before
  `resources.destroy_window` (`:1839`). Extend the snapshot with every Picture
  whose `drawable` (`resources.rs:178` `PictureState`) is in the subtree, for
  **all** clients, and free those Picture resources and their backend
  records. Same in the disconnect path (`process_disconnect.rs:180-264`
  snapshot; teardown `:519-527`, `:728`). Core has no window→Picture index;
  a scan of `resources.pictures` filtered against the snapshot is enough to
  start.
- Tests: the design's cross-client test (client B's Picture on client A's
  window → BadPicture after A's DestroyWindow, and after A disconnects; a new
  window reusing the host storage does not revive it); a window Picture
  composites nothing while the window is unmapped and works after remap.

## Step 4 — Composite: unrealize is not UnredirectWindow

- Split `teardown_redirect_for_window` (`process_disconnect.rs:598`), which
  today releases the backing (`:613`) *and* restores scene participation
  (`:636`):
  - `unrealize_redirect_backing`: release the backing only; the
    `composite_redirects` record (`server.rs:1135`, `RedirectRecord`
    `:817-820`) and the window's non-participation stay.
  - the full teardown stays for UnredirectWindow, UnredirectSubwindows, the
    reparent REVOKE (`process_request.rs:20568`) and destruction.
- Consume the step-1 delta: `became_unviewable` of a redirected window →
  `unrealize_redirect_backing`; `became_viewable` → allocate a fresh backing
  for the existing redirect (reuse the path `reapply_redirect_mode_after_map`
  and `maybe_activate_child_under_redirected_parent` already take,
  `:24668-24669`, `:24803-24804`).
- Named pixmaps keep a released backing alive through the `AliasRegistry`
  (`kms/core.rs:1743`; `release_redirected_backing` `backend.rs:21508`,
  decref `:21557-21559`).
- Tests: unmap of a manually redirected window frees the backing, keeps the
  redirect record and keeps it out of the scene; remap re-creates a backing;
  a NameWindowPixmap taken before the unmap stays valid and readable after
  it.
- HW: awesome + picom, open/close windows (fade-outs read named pixmaps).

## Step 5 — The switch: window storage follows viewability

- `create_subwindow` (`backend.rs:20635`) no longer calls
  `allocate_window_storage` (`:20668`); it records geometry, depth,
  background and border only.
- New backend entry points driven by the delta (core calls them for every
  member, never inferring a subtree):
  - `realize_window_storage(xid)`: allocate at the current bordered size
    through `allocate_window_storage` (`:15452`), fill the background, paint
    the border ring, and for bg None seed from the parent with
    `seed_backing_from_parent` (`:7669`) (accepted deviation: no lower
    siblings).
  - `release_window_storage(xid)`: drop the window's store reference; the
    store's fence-deferred decref keeps a pinned or in-flight image alive.
- Exemptions: the root and the COW (`get_overlay_window` `:21594`,
  `cow_id` `:1297`, `finish_cow_release` `:1962`) never go through these.
- `register_top_level` (`:21075`) and `register_subwindow` (`:21102`)
  allocate 1×1 placeholders for unknown xids; make them follow the same rule
  (no storage unless viewable).
- `configure_subwindow` of an unviewable window updates geometry only; the
  border-attribute path records the border without painting
  (`border_ring_thickness` `:4829` must handle a missing leaf).
- `map_subwindow` / `unmap_subwindow` (`:20716`, `:20758`) and
  `window_viewable` / `collect_viewable_bg_paint_targets` (`:5066`, `:5086`)
  become delta consumers; keep `window_viewable` only as a debug assertion
  that agrees with core.
- Audit every window-path `store.lookup(host_xid)` in `backend.rs` (128
  occurrences including tests) for a missing leaf being an error, warning or
  panic. The redirect route flip (`:21357`, `:21526`, `:21539`) must accept
  a redirected window without a leaf.
- Tests: storage absent after an ancestor's unmap and present after its
  map; a window Picture rebinding to the new leaf; drawing to an unmapped
  window writes nothing; a pool hit on remap of the same size.

## Step 6 — Measure and smoke

- `vram by use` on awesome (air or bee): open large windows on tag 1, switch
  to an empty tag, `window=` drops to the visible set; switch back, it
  returns. Then ask the reporter for one run.
- HW smoke: Cinnamon (Muffin, direct scanout; lock screen), awesome + picom
  (fade-outs), MATE, and a menu/tooltip-heavy app for map/unmap churn.
- Full xts A/B against the step-5 baseline.

## Atomic-switch check

At every step boundary the tree must run: after steps 1–4 every window
still owns storage and the new paths only add correct behaviour, so a
partial landing is safe. Step 5 changes allocation and every consumer in
one commit; it must not be split into "stop allocating" and "cope with
absence".
