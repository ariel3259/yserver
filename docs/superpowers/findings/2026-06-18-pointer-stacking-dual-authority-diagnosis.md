# Pointer / Focus / Stacking bugs — root-cause diagnosis: dual sources of truth

Date: 2026-06-18
Status: **Diagnosis + recommendation. No code change proposed yet — read before attempting fix #N.**
Author: investigation session on bee (cinnamon) + Xorg source cross-reference.
Review: codex-reviewed 2026-06-18 — verdict "findings mostly hold; architectural conclusion
agreed." Corrections incorporated: the "subwindow siblings aren't ordered in v2 yet" claim was
stale (v2 now orders them via `stack_rank`/`restack_subwindow` — but as a parallel projection,
not a derivation); current-branch producer state clarified. Empty-input-shape diagnosis and
Xorg comparison confirmed against source.

---

## TL;DR (conclusion first)

The recurring "sloppy-focus / click-lands-on-wrong-window / window-below-comes-to-front"
family of bugs is **not a focus bug and not fixable by tweaking the pointer path.** It is an
**architectural flaw: yserver maintains two independent sources of truth for window stacking,
pointer-window resolution, and input shape** — the core resource tree
(`yserver-core`) and the KMS v2 backend (`yserver/src/kms/v2`). They cannot even represent the
same facts (empty input shape; subwindow sibling order), so they continually drift, and every
fix to date has only relocated the disagreement.

**Xorg — the de-facto spec — has exactly one source of truth (the `WindowRec` tree); the
framebuffer is *derived* from it, never a co-authority.** That is why Xorg does not have this
bug class.

Three prior fix attempts (each with design docs + codex reviews) failed catastrophically
because they were *half right*: they fixed one path (pointer direction, or the COW model)
while leaving the backend as a parallel authority for the others. Unifying one path leaves the
drift in the others; flipping authority while both stores are live produces catastrophe.

**Recommendation:** do not attempt redesign #4. Instead, incrementally **demote the KMS v2
backend from an authority to a pure projection of the core tree** — delete one parallel store
at a time, each replaced by a read-through to the core tree, behind characterization tests that
pin current-correct behavior *first*. Start with input shape (the proven villain), then
subwindow sibling order. The COW is already modeled correctly (matches Xorg).

---

## 1. The observable bugs

There are (at least) **two distinct user-facing bugs**, plus a separate fix/focus-only churn:

| | **Issue 1 — wrong click DELIVERY** | **Issue 2 — wrong RAISE** |
|---|---|---|
| Repro seen | silence, pamac (front) + Firefox (back) | bee, Nautilus (front) + Nemo (back) |
| Visual | top window stays in front | top window stays in front, then loses it |
| Click | **lands on the window below** (the back app reacts) | **lands correctly** on the top window |
| After | stacking unchanged | **the window below gets raised** to the top |
| yserver trace | hit-test resolves the *lower* window (stale order) | hit-test resolves the *top* window correctly, then a restack raises the lower one |

Both are the same theme — **yserver's stacking/pointer model disagreeing with the
compositor's visual order** — but with different symptoms and code paths. Both reproduce on
`master` (see §3).

**Separate, fix/focus-only:** a restack *storm* — 144 real `AboveSibling` restacks of a single
Nautilus helper window (`0x1f000d4`) in ~2 s — caused by the focus-fix producer's crossing
feedback loop. Absent on master (max 5). It is churn, **not** the cause of Issue 1/2, and is
parked.

---

## 2. Evidence (captured traces, bee/cinnamon, 2026-06-18)

Instrumentation added (read-only, gated): `yserver::input::clickhit=trace` logs each
ButtonPress with the full hit-test stack, the delivered clients, and
`_NET_CLIENT_LIST_STACKING`; `yserver::input::restack=trace` logs every ROOT-children-order
mutation (create/reparent/configure-restack) with before/after order.

### Issue 1 — wrong delivery (stale hit-test order)
ButtonPress, cursor over the overlap; yserver's children order has the **lower** window on top:
```
hit-explain at root=(449,78):
  ...
  0xf002c4[cinnamon] geom=(-14,29 1378x857) mapped geom_inside shape_ok=true  <== FIRST HIT
  => deepest target 0x220000e[nemo] top_level=0xf002c4[cinnamon]
```
Nautilus was visually on top, but `root_pointer_target_at` resolved Nemo's frame because it
sat above Nautilus in yserver's `children`. The click is faithfully delivered to Nemo.

### Issue 2 — wrong raise (click correct, lower window raised immediately after)
```
19:01:44  BUTTON-PRESS → resolved Nautilus, delivered c55[Nautilus]   (click landed correctly)
          _NET_CLIENT_LIST_STACKING: [plank, NAUTILUS, nemo, nemo, nemo-desktop]  (muffin: Nautilus on top)
19:01:44  RESTACK 0xf00128(nemo-frame) BelowSibling(plank)            (Nemo raised above Nautilus)
          root after: [... plank, nemo-frame, NAUTILUS, ...]
          _NET_CLIENT_LIST_STACKING flips → Nemo above Nautilus
19:01:44  crossing Enter → nemo (child=0x2200086)                     (producer now resolves Nemo: it is on top)
```
The click landed on Nautilus, but a restack immediately raised Nemo's frame, and
`_NET_CLIENT_LIST_STACKING` flipped — i.e. the WM itself changed its mind. Open sub-question:
whether muffin sent that restack (off a signal yserver gave it) or Nemo self-raised — not yet
pinned, and subsumed by the architectural cause below.

### Calibration: when it works, the two orders agree
```
press → resolved Nautilus
  yserver hit-test: Nautilus FIRST HIT
  _NET_CLIENT_LIST_STACKING: [plank, Nautilus, nemo, nemo, nemo-desktop]   (agree → correct click)
```

---

## 3. The focus fix (#49) is exonerated

`fix/focus` (#49) was suspected, but the bug **reproduces on `master` (`a6f20f68`, the commit
fix/focus branches from, with no focus fix)** — verified by applying only the read-only restack
trace to master and reproducing Nautilus+Nemo. Master shows **no restack storm** (max 5
restacks vs 144 on fix/focus), yet still misbehaves. Since button *delivery* code
(`root_hit = root_pointer_target_at`) is byte-identical on both branches, the bug cannot be the
focus fix. It is **pre-existing** (the #34-revert symptom). Caveat: it is condition-sensitive
(app mix / timing) — pamac+FF on silence did not trip it in casual testing; Nautilus+Nemo on
bee does.

Current-branch state (codex-confirmed): on `fix/focus` the *producer* already resolves via
`server_state.root_pointer_target_at(...)` (`yserver/src/kms/v2/backend.rs:5385`), so the
historical **producer-vs-delivery split is already closed on this branch** (that is what #49
did, and why it exists). On `master` the producer still used backend `window_under_cursor`, so
the split is accurate there. The remaining *live* drift on `fix/focus` is therefore the input
shape store (DRIFT 1) and the backend stacking projection (`stack_rank`) — which is exactly
where the recommendation in §8 starts.

---

## 4. Retrospective: why three prior attempts failed

| Branch | Approach | Failure mode |
|---|---|---|
| `revert-34-host-first-hittest` (#34) | single authority via **backend** host-stack (`window_under_cursor` as producer + delivery) | backend host stack **lags** the protocol tree → resolves unmapped/stale host_xid → `xid_map` lookup fails → **"no clicks."** Reverted. |
| `revert-34…` commit `39740228` | single authority via **core tree** (`root_pointer_target_at` everywhere; delete backend shape store) | builds clean, but **HW-failed on cinnamon**: the COW captures all input during its `region=0` (opaque) phases → clicks funnel into cinnamon's stage → rubberband / dead sloppy-focus. Commit note: *"the core-first DIRECTION is right; the blocker is COW region=0 input semantics."* |
| `feat/cow-structural` / `fix/cow-structural-rebase` | COW becomes a **real structural root child** (Xorg-faithful: `cow_aware_top_index`, `compCheckRedirect` guard, empty input shape, strict hit-test) | model is correct, but needed **four** course-corrections; load-bearing one: `RedirectSubwindows(root)` redirected the COW → blank desktop, fixed by a `compCheckRedirect` guard. HW-green on bee but left the backend shape/stacking stores as parallel authorities. |

**The pattern:** each attempt was *half right*.
- cow-structural got the **COW model** Xorg-correct.
- `39740228` got the **pointer direction** right (core-first).
- **But all three left the KMS v2 backend as a parallel authority for input shape and
  subwindow stacking.** So unifying the pointer path still drifted on the COW's *dynamic*
  empty-shape (39740228's exact failure), and flipping authority while both stores were live
  produced catastrophe.

Design docs + codex reviews catch *logic* errors. They do **not** catch *behavioral
regressions* in an entangled subsystem — only tests do. Every catastrophe surfaced on HW
dogfooding far from the change. That is the signature of refactoring without a characterization
safety net.

---

## 5. The gold standard: what Xorg does (verified in `/home/jos/Projects/xserver`)

Xorg answers stacking, hit-test, input-shape, and rendering from **one structure — the
`WindowRec` tree**:

- **Stacking = the sibling links themselves.** `WindowRec.{firstChild,lastChild,nextSib,prevSib,parent}`
  (`include/windowstr.h:128-168`); `firstChild` = topmost. `MoveWindowInStack` (`dix/window.c:1632`)
  is the *sole* mutator. **There is no second stacking list.**
- **Hit-test walks that same tree.** `miSpriteTrace` (`mi/miwindow.c:749`) descends
  `firstChild`/`nextSib` top-to-bottom, gated per-window by `mapped` + geometry + shape. No
  separate store. `XYToWindow` (`dix/events.c:3053`) → `miSpriteTrace`.
- **Input shape = one region per window, with the empty-vs-absent distinction built in.**
  `wInputShape(w)` → `w->optional->inputShape` (`include/windowstr.h:80-96,197`). The hit-test
  line (`mi/miwindow.c:768`):
  `(!wInputShape(pWin) || RegionContainsPoint(wInputShape(pWin), ...))` —
  **NULL = use whole window (opaque); empty region = click-through.** Same in
  `PointInWindowIsVisible` (`dix/window.c:2994`).
- **The COW is a normal window in the one tree.** `compCreateOverlayWindow`
  (`composite/compoverlay.c:127`) creates it as an override-redirect child of root and maps it.
  Its *only* two specialties:
  1. **always-on-top**, via `CompositeRealChildHead` (`composite/compwindow.c:762`) used by
     restack/circulate so real children never go above it;
  2. **never redirected**, via `compCheckRedirect` (`composite/compwindow.c:156`): *"Never
     redirect the overlay window."*
  Click-through is just an empty input shape (the standard mechanism above).
- **Rendering derives from the tree.** `miValidateTree` (`mi/mivaltree.c:549`) walks sibling
  order to compute clip lists; redirected windows render to per-window pixmaps the compositor
  draws. **The framebuffer/scanout is the output of painting in tree order — never an
  authority for stacking or hit-test.**

---

## 6. yserver vs Xorg — the divergence

| Concern | Xorg | yserver |
|---|---|---|
| Stacking | one tree (sibling links) | core `children` **+** backend `top_level_order` + per-window `stack_rank` — an independently maintained projection, **not** derived from `ResourceTable.children` (so it can still drift) |
| Input shape | one region; **empty ≠ absent** | core `shape_windows` keeps `Some([])` = click-through **vs** backend `set_shape_rectangles` **deletes** empty → absent reads as **opaque** |
| Pointer-window | derived from the tree | producer (`resource_pointer_host_xid`/old `window_under_cursor`) vs delivery (`root_pointer_target_at`) historically read *different* stores |
| Render / scanout | derived from the tree | `windows_v2` / scene is a **parallel authority** |

**Drift points (most load-bearing first), with refs:**
1. **Empty-input-shape asymmetry (DRIFT 1).** Core `window_input_contains`
   (`yserver-core/src/server.rs` ~1972): absent → opaque, `Some([])` → click-through. Backend
   `set_shape_rectangles` (`yserver/src/kms/v2/backend.rs` ~16236) deletes the entry on empty
   rects; `cursor_inside_shape` reads absent as opaque. **They cannot represent the COW's
   empty-input-shape identically.** This is exactly what broke `39740228`.
2. **Subwindow sibling order — now a parallel projection, not a gap (codex-corrected).**
   v2 *does* order subwindow siblings: `restack_subwindow` (`yserver/src/kms/v2/backend.rs:844`)
   maintains a per-window `stack_rank`, and the scene recurses children in `stack_rank` order
   (`yserver/src/kms/v2/scene.rs:2525`); covered by the passing test
   `restack_subwindow_updates_sibling_order`. The remaining issue is *not* "unordered" — it is
   that `stack_rank` is **backend-maintained parallel state, not derived from
   `ResourceTable.children`**, so it can still drift from core's order.
3. **COW materialization atomicity.** Core resource insert + input-shape materialization are
   separate handler steps; a scene tick between them can see COW-in-order without its shape.

---

## 7. Why this keeps producing catastrophe (and what's actually missing)

- As long as the backend holds *any* authoritative stacking/shape/pointer state, fixing one
  path leaves the drift in the others. The bug is structural, not local.
- Big-bang authority flips require **both** stores to stay in sync *during* the transition —
  impossible when they can't represent the same facts — so they break distant cases
  catastrophically.
- There is **no characterization-test net** for this subsystem, so regressions are found only
  by HW dogfooding, after the fact. Reviews cannot substitute for that net.

---

## 8. Recommendation

**Do not attempt redesign #4.** Instead, converge yserver onto Xorg's single-source model by
**demoting the KMS v2 backend from an authority to a pure projection of the core resource
tree**, incrementally and test-first:

1. **Build the characterization net first.** Golden/unit tests that pin *current-correct*
   behavior across the hard cases: overlapping CSD windows, COW empty-input-region
   (click-through), framed-vs-unframed top-levels, focus-follows-mouse raise, the wezterm
   intermediate-window case, reparent-into-frame. These must pass on master/fix/focus before
   any change, and gate every step.
2. **Step 1 — input shape (DRIFT 1).** Make the scene and any backend hit-test read the core
   `shape_windows` directly (preserving `Some([])` = click-through vs absent = opaque); delete
   the backend `shape_input`/`shape_bounding` as an authority. This removes the asymmetry that
   broke `39740228`.
3. **Step 2 — stacking projection.** v2 already *orders* subwindow siblings (`stack_rank`,
   `restack_subwindow`, scene recurses in rank order), so this is not a missing-feature step —
   it is a *demotion*: make backend `top_level_order` + `stack_rank` a pure *derivation*
   recomputed from core `children`, never independently mutated, so the two orders cannot drift.
4. **COW** stays as `cow-structural` already modeled it — it already matches Xorg
   (real tree node; always-on-top; `compCheckRedirect` never-redirected). Reuse that work.

Each step *removes* a source of truth rather than adding a reconciliation. That is why it can
succeed where unification/big-bang attempts failed.

**Process:** codex-review this diagnosis and the per-step plan before any code (project
convention). Treat the HW catastrophes of the three prior branches as the acceptance criteria:
the net must cover the cases each of them broke.

---

## Appendix A — reproduction & instrumentation

Recipe (bee, VT): `just yserver-cinnamon-hw log="warn,yserver::input::clickhit=trace,yserver::input::restack=trace"`.
Repro: open two overlapping file managers (e.g. Nautilus over Nemo); raise the top one; click
it. Issue 1 = click reacts on the lower window; Issue 2 = lower window raises afterward.
Intermittent / condition-sensitive.

Diagnostic code (read-only, currently uncommitted on `fix/focus`):
- `yserver-core/src/server.rs`: `debug_window_label`, `debug_client_label`,
  `debug_explain_pointer_hit`, `debug_net_client_list_stacking`, `AtomTable::id_for`.
- `yserver-core/src/core_loop/pointer_fanout.rs`: the `clickhit` ButtonPress trace block.
- `yserver-core/src/resources.rs`: the `restack` trace in
  `restack_window`/`reparent_window`/`create_window` + `debug_root_order`.
- `Justfile`: `yserver-cinnamon-hw` default log includes clickhit.
Note: `git diff` here uses difftastic — use `git diff --no-ext-diff` to produce applyable patches.

## Appendix B — prior art

Branches: `revert-34-host-first-hittest`, `fix/unified-pointer-sprite`,
`fix/cow-structural-rebase`, `feat/cow-structural`.
Docs: `docs/superpowers/plans/2026-04-28-clipping-and-pointer-events.md`,
`2026-05-20-cow-authoritative-mode.md`, `2026-06-08-cow-structural.md`;
`docs/superpowers/findings/2026-06-09-cow-structural-results.md` (on the cow-structural branch).
Xorg source: `/home/jos/Projects/xserver`.
