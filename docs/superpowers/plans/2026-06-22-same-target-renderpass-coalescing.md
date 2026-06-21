# Same-target render-pass coalescing (Stage 5 Task 3 continuation)

Status: 🟡 draft plan — awaiting codex review before execute.
Branch: `perf/same-target-renderpass-coalesce`.

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

## Root cause in code

Only `render_composite` coalesces (engine.rs `PendingRenderBatch`,
one `begin_rendering`/`end_rendering` per batch). Every other op kind
calls `flush_render_batch` at its entry guard and then runs its own
pass with its own `to_color` + `to_read` barrier pair:

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
