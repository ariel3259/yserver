# Same-target render-pass coalescing (Stage 5 Task 3 continuation)

Status: 🟢 measured + ready to implement (slice 1) — codex review the
cross-kind extension before that step.
Branch: `perf/same-target-renderpass-coalesce` (pushed; 2 telemetry
commits `ac07bd35`, `14367e7a`).

## CONFIRMED RESULT (2026-06-22, air / Asahi M1, xfce + menu hover + drag)

Two telemetry iterations on hardware nailed down where the cost is and
how much is recoverable:

1. **`PendingRenderBatch` is dead for xfce.** The `vk renderpass flush
   src` counters came back **all-zero** (`self_sample=0`, every
   `rpflush_*=0`) while `begin_rendering=41,557` / `barrier2=114,942`.
   So `try_append_render_batch` is not on the hot path — do NOT optimise
   it.

2. **The hot path is the frame-builder close-replay.** `vk frame
   coalescing` on a fresh run:

   ```
   pass_ops    64,726     (≈ begin_rendering; slight over-count — tallies
                           recorded ops incl. rolled-back frames)
   coalescable 34,480     ← 53% of passes a same-dst session removes
   self_sample 0          ← NOTHING blocks the merge
   ```

   Offline submit-trace cross-check agrees: 58% consecutive-same-dst.
   `begin_rendering=50,509` / `barrier2=138,620` / `queue_submit2=8,317`
   that run (submits already fine — frame builder batches them; passes
   are the problem).

**Bottom line:** keeping one `begin_rendering` open across consecutive
same-dst ops in the replay removes ~34k of ~50k render passes and a
large share of the ~139k barriers, with **no self-sample caveat** — the
full ~53–58% is achievable. This is THE tiler win for #48.

## NEXT SESSION — start here

Implement in `close_open_frame`'s replay loop (engine.rs ~1852):
group consecutive same-dst pass-ops, emit ONE
`to_color`+`begin_rendering` … `end_rendering`+`to_read` per group,
rebinding pipeline/descriptors between ops inside it.

Recommended order (decided 2026-06-22):
- **Slice 1 (lowest risk, do first):** merge consecutive same-dst
  `RecordedOp::RenderComposite` only. `ops/render.rs` already has the
  `open`/`draws`/`close` split, so NO recorder refactor — just hoist
  open/close around a run in the replay. Largest single kind; validates
  the approach end-to-end on HW; watch `coalescable` + `begin_rendering`
  drop to confirm.
- **Slice 2 (after codex review):** extend to fill/glyph/traps. These
  recorders (`ops/fill.rs`, `ops/text.rs`, `ops/traps.rs`) are monolithic
  (own begin/end_rendering inside) and must be split into open/draws/
  close so open/close can be hoisted. `cmd_clear_attachments` (fills)
  composes mid-pass — no "scratch clear lift-out" needed (retires the
  old Task 3 deferral note).

Validation (per slice): `cargo test`; Vulkan validation layers clean on
ynest (no render-pass/layout VUIDs) under menu-hover + drag; HW A/B on
air comparing `vk frame coalescing` + `begin_rendering/s` + `barrier2/s`
and confirming NO visual regression (interactive — buffer-age taught us
GPU correctness here is only judgeable by eye). Re-capture on a RADV box
to confirm no submit-rate regression.

Telemetry to watch is already wired: `vk frame coalescing [1s]`
(`pass_ops` / `coalescable` / `self_sample`) — `coalescable` should fall
toward zero as slices land.

## Why now (the new data the original deferral lacked)

Stage 5 Task 3 framed paint aggregation around **`vkQueueSubmit2`
count** — bee/silence are RADV (immediate-mode) and were ioctl-rate
bound (~2k submits/s). Against that metric, `render_fill` coalescing
was scored "8k savings on silence, deferred" and the Solid `render_fill`
extension was parked because it "would need scratch clear lifted out of
the render pass" (plan §Task 3, 2026-05-20).

The #48 xfce captures add a hardware class the original analysis did not
have: **Apple/Asahi tiler (`air`).** On a TBDR GPU the binding cost is
not submits — it is **render passes and barriers**:

| metric (44s xfce capture, air) | count | note |
|---|---|---|
| `queue_submit2` | 4,601 | already fine — Task 3 POC working |
| `begin_rendering`/`end_rendering` | 30,369 | **render passes** |
| `barrier2` | 83,318 | ~1 barrier per draw (79,874 draws) |

Each `end_rendering` on a tiler is a full tile store; each
`begin_rendering` a reload; each `COLOR→SHADER_READ` / `SHADER_READ→COLOR`
barrier a cache flush. The busiest menu-hover second did **2,805 render
passes + 8,438 barriers** for a tiny menu region — that is what drops
compositing to ~23 fps and makes the highlight lag the cursor.

So the deferred fill/glyph coalescing is **re-prioritised**: on tilers
each avoided pass saves a tile store/load round-trip, not just an ioctl.
The metric to drive this work is `begin_rendering/s` + `barrier2/s`, not
just `queue_submit2/s`.

## Course correction (2026-06-22 air run — read this first)

The flush-reason telemetry (`vk renderpass flush src`) came back
**all-zero** on a live xfce capture while `begin_rendering=41,557` /
`barrier2=114,942`. That decisively shows the `PendingRenderBatch`
coalescing path (`try_append_render_batch`) is **dead for xfce**:
`self_sample=0` too, so that entry point isn't even called.

The real hot path is the **frame builder**. Ops are recorded as
`RecordedOp` into an `OpenFrame` and **replayed at `close_open_frame`**,
where each op calls its `record_*_open/draws/close` and emits its own
`begin_rendering` + `to_color`/`to_read` barrier pair. The frame
builder already solved Task 3's *submit* problem (queue_submit2=6,257,
low — one CB per frame) but **not** the *render-pass* problem (one pass
per op). On a tiler that is the entire cost.

**So the coalescing must happen in the close-replay loop**, not in
`PendingRenderBatch`. The `vk frame coalescing` telemetry line
(`fb_pass_ops` / `fb_pass_coalescable` / `fb_self_sample`) measures the
headroom directly on that path; size the phases from it.

## Root cause in code

In the frame-builder replay (`close_open_frame`), every `RecordedOp`
emits its own pass. Outside the frame builder, only `render_composite`
coalesces (engine.rs `PendingRenderBatch`); every other op kind calls
`flush_render_batch` at its entry guard and runs its own pass with its
own `to_color` + `to_read` barrier pair:

- `fill.rs::record_fill_rectangles` — `cmd_clear_attachments` in its own
  LOAD/STORE pass (no pipeline). Backs `PolyFillRectangle` / `ClearArea`.
- `ops/render.rs` RenderFill (Solid) — own pass.
- `ops/text.rs` `composite_glyphs` — own pass per flush.
- `ops/traps.rs` — own pass.

A single menu-item repaint is `clear-fill → src-fills → glyphs →
composite`, **all to the same dst pixmap**, so each kind-transition
flushes the batch and round-trips the dst `COLOR↔SHADER_READ` between
ops that never sample it. That round-trip is pure waste whenever the
next op writes the same dst and does not read it.

## Design: a per-dst "render-pass session"

Generalise `PendingRenderBatch` from "consecutive same-`RenderBatchKey`
composites" to **"consecutive ops writing the same dst that do not
sample the dst"**, holding one `begin_rendering` open across them and
rebinding pipeline between ops (legal in dynamic rendering).

Keep the dst in `COLOR_ATTACHMENT_OPTIMAL` across the session; emit the
`to_read` barrier + `end_rendering` lazily, only when:

1. the next op targets a **different** dst, or
2. the next op **samples** this dst (src or mask id == dst), or
3. the dst is about to be **presented / composited / read back**
   (`copy_area` src, `get_image`, scene compose, frame close), or
4. an op in the run genuinely needs a pipeline/attachment change that
   dynamic rendering can't express mid-pass.

Within a session, ops just rebind pipeline + descriptors and draw
(composites) or `cmd_clear_attachments` (fills — legal mid-pass, honours
the bound scissor). This is the key realisation that retires the
"scratch clear lifted out of the render pass" blocker: `clear_attachments`
does **not** need to be lifted out — it composes inside the same dynamic
render pass, interleaved with `cmd_draw`, as long as the color attachment
is unchanged.

### Correctness constraints (the hazards that sank prior GPU work)

- **Self-sample = hard flush.** If any op reads the dst (RENDER ops where
  src/mask id == dst, or `copy_area` with dst as src), close the session
  first (`end_rendering` + `to_read`) so the read sees committed pixels.
  This is the load-bearing invariant; default to flushing when unsure.
- **Layout bookkeeping.** `DrawableImage.current_layout` /
  `Drawable::storage.current_layout` must reflect "still COLOR" while a
  session is open. The existing `record_render_composite_open_with_old_layout`
  + `commit_close_success` layout commit is the model to extend; do not
  let two code paths disagree on the open dst's layout.
- **Fence/ticket + descriptor-pool lifetime.** A longer-lived CB must
  keep every touched drawable + staging buffer + atlas ticket alive to
  retirement, exactly as `PendingRenderBatch.touched_drawables` /
  `SubmittedOp` do today. Extend, don't fork.
- **Damage + telemetry.** Accumulate dst damage across the session
  (`PendingRenderBatch.dst_damage`); emit one flush record with
  `coalesced_count`. Add `begin_rendering/s` + `barrier2/s` to the
  per-second telemetry so the win is measurable (they exist in `vk call
  rate` already; surface them on the v2 line).

## Phasing (each phase independently shippable + HW-validated)

- **Phase 1 — `render_fill` (Solid) into the composite session.**
  Smallest slice that needs no new session abstraction: let a Solid
  `render_fill` append into an open `PendingRenderBatch` whose dst
  matches, via `cmd_clear_attachments` mid-pass (or the solid pipeline).
  Reverts the explicit Task 3 deferral. Expected: removes the per-fill
  pass+barrier pair for runs to one dst. ~8k submit savings on silence;
  proportionally larger pass/barrier savings on `air`.
- **Phase 2 — keep dst in COLOR across kind transitions.** Defer
  `to_read`/`end_rendering` at flush when the next op writes the same
  dst and doesn't sample it. This is the bulk of the 30k→? reduction.
- **Phase 3 — fold `composite_glyphs` into the session** (same dst, no
  self-sample). Glyph runs are the densest menu-hover op.
- **Phase 4 — traps + revisit put_image span bursts** if data warrants.

## Validation gate (per phase)

1. `cargo test -p yserver` green; new unit tests on the session
   open/append/flush state machine + the self-sample-forces-flush
   invariant.
2. Vulkan validation layers clean on `ynest` (no
   layout/render-pass VUIDs) under a menu-hover + drag workload.
3. HW A/B on `air` via `just yserver-xfce-hw-telemetry`: compare
   `begin_rendering/s` + `barrier2/s` (target: large drop on menu hover)
   and confirm no visual regression (the buffer-age spike taught us GPU
   correctness here is only judgeable interactively).
4. Re-capture on a RADV box (bee/silence) to confirm no submit-rate
   regression — the original Task 3 metric must not move the wrong way.

## Non-goals

- Buffer-age clipped repaint (separate, shelved — see
  `perf/reenable-buffer-age` + dbf093f). This plan speeds up producing
  each full frame; it does not try to skip frames.
- Allocation churn (Stage 5 Task 5 / pixmap-pool oversize) — independent
  lever, separate work.
