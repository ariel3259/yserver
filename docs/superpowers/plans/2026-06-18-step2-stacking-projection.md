# Step 2 — demote backend `top_level_order` to a pure projection of core children

Date: 2026-06-18
Base: `fix/focus` after Step 1a (`b67130a9`). Part of the DRIFT-2 fix in
`docs/superpowers/findings/2026-06-18-pointer-stacking-dual-authority-diagnosis.md` §8 Step 2.

## Problem (the two RED gates)

The KMS-v2 backend maintains `core.top_level_order: Vec<u32>` (host xids, bottom→top) as an
**independent authority** for top-level z-order, mutated by its own logic that drifts from the
core resource tree:

- `restack_top_level` (backend.rs:6081) treats `TopIf`/`BottomIf`/`Opposite` as **unconditional**
  Above/Below — it ignores geometry, so a `TopIf` that core correctly treats as a no-op still
  reorders the backend. → `drift2_topif_noop_keeps_backend_order_in_sync_with_core` (RED).
- `restack_top_level` (and `create`/`reparent`) have **no COW awareness** — a raise-to-top pushes
  a normal window *above* the Composite Overlay Window, whereas core caps it just below the COW
  (`cow_aware_top_index`, `restack_above_cow_caps_to_just_below_cow`). →
  `drift2_raise_keeps_cow_on_top` (RED).

Core already computes the correct order (full X11 occlusion + COW cap, covered by ~15 resources.rs
tests). The bug is that the backend recomputes it independently and badly.

## Approach (doc-faithful: derive, don't re-implement)

Make `top_level_order` a **pure projection of `state.resources.children(ROOT_WINDOW)`**, never
independently mutated. Core stays the single source of truth; the backend stores a derived copy.

### New Backend trait method

```rust
/// Reproject the backend's top-level z-order from the core resource tree.
/// `top_level_order` becomes children(ROOT_WINDOW), filtered to host-backed
/// windows, mapped to host xids, in core's bottom→top order. Default: no-op
/// (host_x11 / recording don't keep this projection).
fn sync_top_level_order(&mut self, state: &ServerState) {}
```

Precedent: `on_window_became_top_level(&mut self, state: &ServerState, …)` is already a
`ServerState`-taking trait method (default no-op, v2-overridden).

v2 impl:
```rust
fn sync_top_level_order(&mut self, state: &ServerState) {
    self.core.top_level_order = state
        .resources
        .children(ROOT_WINDOW)
        .iter()
        .filter_map(|id| state.resources.window(*id).and_then(|w| w.host_xid).map(|h| h.as_raw()))
        .collect();
    self.scene.mark_scene_structure_dirty();
}
```
Because core children already caps the COW on top and applies conditional TopIf/BottomIf, the
projection inherits both for free.

### Call sites (core handlers, all have `state`)

Call `backend.sync_top_level_order(state)` after any operation that changes root's child order:
1. `ConfigureWindow` with `stack_mode` (process_request.rs ~15486, after `configure_window`).
2. Window create as a root child.
3. Destroy of a root-child top-level.
4. Reparent to/from root.
5. COW materialize / teardown (so the COW enters/leaves the projection).

### Remove the now-dead independent mutations

- `restack_top_level`'s top-level reorder body → delete (subwindow `restack_subwindow`/`stack_rank`
  is a SEPARATE concern, left for Step 2b — not gated yet).
- Backend `create`/`reparent`/`destroy`/COW `top_level_order.push/retain` (backend.rs:10858,
  11122, 11126, 11214, 11706-7, 11741) → replaced by `sync_top_level_order`.

## Scope decision for review

- **Option A (minimal, gated):** wire `sync_top_level_order` only into the `ConfigureWindow`
  restack path. Turns both gates green. Leaves create/reparent/COW on their existing (latently
  COW-buggy but un-gated) push/retain logic → residual drift for freshly-created windows under a COW.
- **Option B (full demotion, recommended):** wire `sync_top_level_order` into ALL five call sites
  and delete every independent mutation. Eliminates the dual authority entirely (the doc's goal),
  but larger blast radius on a load-bearing z-order path.

Leaning B (it's the actual fix; A leaves a known latent COW drift on create). Want codex's read on
A vs B and on the projection/call-site design.

## Tests (test-first)

- Rewrite the two RED gates to drive `sync_top_level_order(&state)` after building core children via
  `configure_window` (TopIf no-op; COW-present raise), asserting `top_level_order` == the core
  projection. Non-vacuous: projection must filter host-backed + map xids + preserve core order.
- Add: create-under-COW → projection keeps COW last (Option B).
- Existing resources.rs restack tests already pin core's correctness; keep green.
- The existing v2 `restack_*`/`reparent_*`/`window_under_cursor` tests that assert on
  `top_level_order` will need updating to the projection model.

## Risk

Global window z-order. Mitigation: the projection is a trivial, total function of core children
(already test-covered); the change removes logic rather than adding it. HW dogfood (overlapping
file managers; create-under-compositor) is the user's acceptance step once green.

## Codex review outcome (2026-06-18, gpt-5.5, read the actual code)

**Verdict: Option B, done as a real demotion.** Refinements folded in:

1. **Projection shape:** `children(ROOT_WINDOW)` → host-backed → host_xids, in core order.
   - **Do NOT filter on `mapped`** — X11 stacking order includes unmapped children; order must
     survive unmap/remap (scene/hit-test already skip unmapped via geometry/state).
   - **Do NOT filter on `windows_v2` membership** — that hides backend lifecycle drift. Project
     all host-backed root children; debug-assert / log loudly if a projected xid is missing from
     `windows_v2`.
   - Correctly included: override-redirect root children, reparented WM frames, unmapped
     host-backed root children, the COW. Correctly excluded: client windows reparented under
     frames (reached via descendant traversal), non-host-backed resource children.
2. **6th drift source — `apply_top_level_stack_hint` (backend.rs:6203)**, invoked from
   `on_window_became_top_level` (9422/9426). It reorders `top_level_order` from EWMH hints
   (`_NET_WM_WINDOW_TYPE_*`, `_NET_WM_STATE_ABOVE/BELOW/FOCUSED`, `WM_TRANSIENT_FOR`) via
   `restack_top_level`. Once order is a projection this mutation is overwritten by the next sync,
   so it must be removed. **Xorg does NOT do server-side EWMH stacking — the WM does it via
   ConfigureWindow (→ core → projection).** `_NET_WM_STATE_FOCUSED → raise-to-top` here is a
   prime suspect for Issue 2 (wrong-raise). Decision: drop the backend's EWMH/focus-based
   reordering (neutralize `apply_top_level_stack_hint`); stacking is owned by the WM via core.
3. **COW sync timing:** `GetOverlayWindow` is backend-first then core `materialize_cow_resource`.
   Sync must run in the **core handler** after `materialize_cow_resource`/`materialize_cow_input_shape`
   (0→1) and after `destroy_cow_resource`/`destroy_cow_input_shape` (final release) — NOT inside the
   backend COW hook (core has no COW child yet there).
4. **Hot path:** rebuilding the `Vec` per restack is O(#top-levels), trivial vs scene compose. Fine.
5. **Surgical deletion** — keep each method's non-order duties, delete only the order mutation:
   `register_top_level` (keep storage+xid map), `reparent_subwindow` (keep parent update),
   `destroy_subwindow` (keep windows_v2/storage removal), COW hooks (keep windows_v2 insert/remove),
   `configure_subwindow` (remove the top-level `restack_top_level` branch; keep `restack_subwindow`
   for Step 2b).

### Revised call sites (core handlers, all have `state`)
After: ConfigureWindow restack · create root-child · destroy root-child · reparent to/from root ·
**COW materialize/teardown (core handler, post-materialize)**.

### Revised tests
Rewrite the two gates to drive `sync_top_level_order(&state)` after core `configure_window`
(TopIf no-op; COW raise). Add: create-under-COW, reparent-to-root-under-COW, reparent-from-root,
final COW release, and **EWMH/`_NET_WM_STATE_FOCUSED` no longer reorders independently** (the most
likely hidden drift path).

## Second codex pass (2026-06-18, gpt-5.5) — verdict: GO-WITH-CHANGES

Architecture confirmed correct; EWMH-stacking removal confirmed Xorg-faithful (DIX restacks only via
ConfigureWindow/CirculateWindow; `_NET_WM_STATE` never restacks in DIX). Required changes:

1. **Neutralize BOTH hooks**, not just `on_window_became_top_level` (backend.rs:9425):
   `on_window_property_changed` (backend.rs:9416) ALSO calls `apply_top_level_stack_hint` — invoked
   from ChangeProperty / DeleteProperty / GetProperty(delete=true) (process_request.rs:22659).
   `on_window_became_top_level` has NO other v2 work, so making it inert breaks nothing else.
2. **Tests to rewrite/remove** (assert old independent ordering):
   `restack_below_no_sibling_moves_to_bottom`, `restack_above_no_sibling_moves_to_top`,
   `desktop_window_type_moves_to_bottom`, `dialog_hint_raises_when_window_becomes_top_level`,
   `reparent_into_container_removes_from_top_level_order`, `reparent_to_root_re_adds_to_top_level_order`,
   the COW tests `get_overlay_window_first_claim_materializes_full_backend_state` /
   `release_overlay_window_final_release_tears_down_full_backend_state` (drop their direct
   `top_level_order` assertions), and the two ignored DRIFT gates (drive the sync, not `restack_top_level`).
3. **Sync call-site ordering (precise):**
   - CreateWindow root child: AFTER successful `register_top_level` (not merely after setting `host_xid`).
   - Reparent to/from root: AFTER backend `reparent_subwindow` + `register_top_level`/`register_subwindow`.
   - DestroyWindow: AFTER `state.resources.destroy_window(root)` and the pending backend teardown loop.
   - ConfigureWindow: AFTER `configure_window` (once the top-level branch is removed from `configure_subwindow`).
   - COW: core handler AFTER `materialize_cow_resource` / `destroy_cow_resource`.
4. **LOG, do not debug-assert**, when a projected host-backed root child is missing from `windows_v2`:
   benign/failure-path transients exist (core child has `host_xid` before backend registration/storage
   completes); scene + hit-test already skip missing `windows_v2`, so a debug-build panic would turn
   benign diagnostic drift into a crash.
5. **Step 2b partial state is safe:** leaving `restack_subwindow`/`stack_rank` for descendants does not
   reintroduce top-level drift — top-level order comes from core, per-parent sibling order stays on
   `stack_rank`, and scene recursion uses them independently.
