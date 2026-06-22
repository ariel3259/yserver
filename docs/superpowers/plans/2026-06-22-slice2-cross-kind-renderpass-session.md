# Slice 2 — cross-kind render-pass session (frame-builder replay)

Status: 🟢 design codex-reviewed (round 2) — verdict "tractable extension
of the deferred-recording infra, not a hazard pit," after the two patches
below. Ready to implement Phase 1 on greenlight.
Branch: `perf/same-target-renderpass-coalesce`.

## Codex review outcome (2026-06-22)

Per-area: layout bookkeeping §5 SOUND (overlay drives append-time
`old_layout`; close-success commits / failure rolls back once; no
mid-session divergence path). Glyph mid-pass SOUND *iff* the recorder is
genuinely split (hoist barriers/begin/end/viewport/layout-writeback;
keep pipeline+descriptor+scissor+push+draw in the draws half). Fill
mid-pass SOUND (`cmd_clear_attachments` already runs in-pass). Resource
lifetime SOUND (no new retirement path; pins/ticket/frame_generation
already cover it). Two FIXABLE GAPS, both patched into this doc:
1. **`LayoutTransition` op kind** was missing from the flush enumeration
   — now §3 (hard flush; can target the session dst).
2. **Phase 1 must stay byte-for-byte** — do not unify the pre-barrier
   access masks until Phase 3 — now §Phasing.

## Why (measured)

Three-machine telemetry (air M1, eiger M2, silence RX580) on xfce
menu-hover + drag, with the bucket-split counter (`ba558e1a`):

| bucket | eiger (tiler) | silence (RADV) | reachable by |
|---|---|---|---|
| `mergeable` | 27% of coalescable / 16% of passes | 30% / 16% | Slice 1 (composite-only) |
| `dirty_clear` | 16% / 9% of passes | 18% / 10% | per-op solid scratch |
| **`cross_kind`** | **57% / 34% of passes** | **52% / 30%** | **this slice** |

Per-second mix shows the busiest seconds (the menu-hover/repaint storms
that ARE the #48 felt lag) are **61–84% `cross_kind`**. Slice 1 / 1.5
barely touch those seconds. `self_sample=0` everywhere. So the cross-kind
session is the only lever that addresses the reported lag.

## The spec: replicate Mesa GL's implicit render-pass batching

Xorg (glamor → GL) is smooth on this workload because the GL driver keeps
the destination FBO's tile resident across consecutive same-dst draws and
flushes (stores) lazily on a hazard. yserver pays a tile store+load per X
op because its Vulkan dynamic-rendering translation emits one
`begin_rendering`+`end_rendering`+barrier-pair per op. Slice 2 makes the
frame-builder replay do what the GL driver does: **hold one
`begin_rendering` open across consecutive same-dst ops of ANY kind, flush
lazily.** This is not new architecture — the frame-builder already defers
and replays; this changes only how the replay emits passes.

## Enabling observation: the emit skeleton is uniform

Every render-pass-emitting op's `emit_recorded_*_into_cb` has the
identical shape today:

```
pre-barrier:  barrier_to_layout(dst, dst_old_layout → COLOR_ATTACHMENT_OPTIMAL)
open:         cmd_begin_rendering(LOAD/STORE) ; cmd_set_viewport
draws:        bind pipeline ; per-(clip×rect): set_scissor + push_const + draw
              (FillRect uses cmd_clear_attachments instead of a pipeline draw)
close:        cmd_end_rendering
post-barrier: barrier_to_layout(dst, COLOR → SHADER_READ_ONLY_OPTIMAL)
```

Confirmed at:
- composite: `record_render_composite_open_with_old_layout` /
  `record_render_composite_draws` / `record_render_composite_close`
  (`vk/ops/render.rs:214/282/385`) — ALREADY split.
- fill: `emit_recorded_fill_rect_into_cb` (`engine.rs:9249`) —
  `cmd_clear_attachments`, legal mid-pass.
- logic_fill: `emit_recorded_logic_fill_into_cb` (`engine.rs:9355`).
- glyph: `emit_recorded_op_into_cb` `CompositeGlyphs` arm
  (`engine.rs:8331`) via `record_text_run_scissored`; atlas is a
  separate SHADER_READ image; glyph uploads are separate
  `RecordedOp::GlyphUpload` (non-pass) ops.
- image_text: `emit_recorded_image_text_into_cb` (`engine.rs:9473`).
- traps: `emit_recorded_render_traps_or_tris_into_cb` (`engine.rs:9565`)
  — SPECIAL: rasterizes a coverage mask into `mask_scratch` (a DIFFERENT
  attachment) in its own pass, then composites scratch→dst. See §Traps.

The draw bodies and pipelines differ; the open/close (pre-barrier +
begin_rendering + viewport / end_rendering + post-barrier) are nearly
identical. The only open-barrier difference is the producer access mask
on the pre-barrier — unify to the superset already used by fill/logic
(`SHADER_SAMPLED_READ | TRANSFER_WRITE | COLOR_ATTACHMENT_WRITE`).

## Design

### 1. Split each kind into `open` / `draws` / `close`

Refactor (no behaviour change) each monolithic `emit_recorded_*_into_cb`
into three pieces, mirroring the composite split that already exists:

- `emit_<kind>_open(inner, cb, dst_image, dst_view, dst_extent, dst_old_layout)`
  → pre-barrier + `begin_rendering` + viewport. Identical across kinds
  (one shared helper `open_dst_color_pass`).
- `emit_<kind>_draws(inner, cb, op)` → pipeline bind + scissors + draws
  (or `cmd_clear_attachments`). Kind-specific.
- `close_dst_color_pass(inner, cb, dst_image)` → `end_rendering` +
  post-barrier → SHADER_READ. Identical across kinds (one shared helper).

The standalone `emit_recorded_*_into_cb` becomes
`open ; draws ; close` and stays the fallback for unmerged ops, so the
refactor is provably equivalent before any merging is switched on.

### 2. A `DstPassSession` in the replay loop

In `close_open_frame`'s record pass (`engine.rs:2000`, currently
`for op in &open_frame.ops { emit_recorded_op_into_cb(...) }`), thread a
session:

```
struct DstPassSession { dst_id, dst_image, dst_view, dst_extent }   // None = closed
```

For each op:
1. Compute `eligible` (see §Eligibility) and `dst` (None for non-pass).
2. If a session is open and (`!eligible` || `dst != session.dst` ||
   op reads session.dst) → `close_dst_color_pass`; session = None.
3. If `eligible`:
   - if session is None → run the op's **pre-pass work** (composite
     solid clears / readback copies — see §Pre-pass), then
     `open_dst_color_pass(dst_old_layout = op.dst_old_layout)`; session = open.
   - emit the op's `draws` only (no per-op begin/end/barrier).
4. else (`!eligible`, e.g. traps, copy, put_image, glyph-upload) → emit
   via the unchanged standalone `emit_recorded_*_into_cb` (which does its
   own open+draws+close), session stays None.
5. After the loop, if a session is open → `close_dst_color_pass`.

`open` uses the FIRST op's `dst_old_layout`; `close` always terminates at
SHADER_READ. Intermediate ops contribute draws only.

### 3. Eligibility (which ops may join a session)

Session-eligible op = writes one dst as a color attachment via
mid-pass-legal commands, does NOT read its own dst, needs NO pre-pass
transfer once the session is open:

- **fill / logic_fill** — always eligible (`clear_attachments` / pipeline
  draw, mid-pass-legal).
- **glyph / image_text** — eligible (samples atlas, a separate image;
  uploads are separate non-pass ops).
- **composite** — eligible iff fold-clean: no `src_clear_color` /
  `mask_clear_color`, no `src_alias_view` / `needs_dst_readback`, not
  self-sampling (== current Slice-1 `folder_clean`). A solid-clear
  composite can still OPEN a session (its clear runs as pre-pass before
  `open`) but cannot JOIN one mid-pass. (Slice 1.5 — per-op solid scratch
  — would let it join; out of scope here.)
- **traps** — NOT eligible (see §Traps). Forces a flush.
- **copy_area / masked_copy_area / put_image / glyph_upload /
  clip_snapshot_refresh** — NOT pass ops; force a flush.
- **layout_transition** (`RecordedOp::LayoutTransition`, replayed at
  `engine.rs:8382` via `record_layout_transition`) — NOT a pass op and a
  HARD flush: it transitions an arbitrary drawable's layout (GPU + the
  CPU-tracked layout) and that drawable CAN be the session's open dst. The
  session must close (post-barrier → SHADER_READ) BEFORE this op so the
  transition's append-time `old_layout` (SHADER_READ from the overlay)
  matches the GPU. Codex round-2 caught this — it was missing from the
  v1 enumeration. Default classify maps it to "not eligible" → flush,
  but it must be called out so the `else` arm explicitly handles it
  (it does: `emit_recorded_op_into_cb`'s `LayoutTransition` arm runs
  standalone once the session is closed).

### 4. Flush rules (GL's implicit store triggers)

Close the open session before any of:
1. next op writes a **different dst**;
2. next op **reads this dst** — composite with src/mask view == dst view,
   `copy_area` with this dst as src, `get_image`, scene compose, present
   (the dst must be committed to SHADER_READ first);
3. next op is **not session-eligible** (traps, copy, put_image, …);
4. **end of frame** replay.

Default to flushing when unsure. Rule 2 is the load-bearing self-sample
invariant; `self_sample=0` in telemetry says it is rarely hit, but it
must be correct.

### 5. Layout bookkeeping (the crux — where prior GPU work sank)

Today each op pre-barriers from its append-time `dst_old_layout` and
post-barriers to SHADER_READ; the overlay records the post-op layout, and
`commit_close_success` (`engine.rs:~9948`) writes the overlay's final
per-drawable layout into `storage.current_layout` on submit success.

Under a session, dst stays in `COLOR_ATTACHMENT_OPTIMAL` across the
group. Why this stays correct:

- The session emits ONE pre-barrier (from the first op's `dst_old_layout`
  — the overlay-resolved layout before the group) and ONE post-barrier to
  SHADER_READ. Intermediate ops emit NO barrier, so their (now-stale)
  recorded `dst_old_layout` is simply unused — there is no wrong barrier,
  just an omitted one.
- Storage is committed once at frame close from the FINAL overlay entry,
  which is SHADER_READ for dst (the last op in the group recorded that).
  The intermediate overlay entries (each op individually recorded
  "SHADER_READ after me") are irrelevant because storage is never written
  mid-frame — `commit_close_success` only takes the last value.
- A later same-frame op that reads dst hits flush-rule 2, so the session
  closes (post-barrier → SHADER_READ on the GPU) before that read; the
  reader's pre-barrier then transitions from SHADER_READ as it already
  expects.

Invariant to assert: the GPU dst layout the session believes (COLOR while
open, SHADER_READ after close) must never diverge from what a later op's
pre-barrier declares as its `old_layout`. Because every later op's
recorded `dst_old_layout` was resolved assuming the prior op left dst in
SHADER_READ, and the session DOES leave dst in SHADER_READ at close, the
two agree at every session boundary. The only place they could diverge is
mid-session — and mid-session no op emits a barrier, so there is nothing
to diverge.

### 6. Pre-pass work

`open_dst_color_pass` must be preceded (while NO pass is open) by any
transfer/clear the opening op needs:
- composite solid 1×1 clears (`record_solid_color_clear` →
  `cmd_clear_color_image`, illegal in a pass);
- composite dst-readback / src-alias copies (`record_copy_from`).

These only ever run for the session's OPENING op (a joining op is
eligibility-gated to need none). So: when opening, run the opening op's
pre-pass, then `open`, then its draws. Followers contribute draws only.

### Traps (deferred within Slice 2 / to Slice 4)

`emit_recorded_render_traps_or_tris_into_cb` rasterizes coverage into
`mask_scratch` — a DIFFERENT color attachment — in its own
`begin_rendering`, then composites scratch→dst. Two passes to two
attachments cannot both be open (one `begin_rendering` per CB at a time),
so a trap op must run with no dst session open: it forces a flush, does
its mask-raster pass + dst-composite pass standalone, and MAY leave dst
open for followers afterwards (optional later optimization). Treat traps
as a hard flush boundary for now.

## Phasing within Slice 2 (each independently HW-validated)

1. **Refactor only** — split fill/logic_fill/glyph/image_text into
   open/draws/close; standalone emit = open+draws+close. No merging.
   Prove `cargo test` + validation-layer + HW pixel-identical (zero
   behaviour change). This de-risks the mechanical part alone.
   **Keep it byte-for-byte equivalent**: do NOT unify the per-kind
   pre-barrier producer access masks in this phase. Composite's open
   barrier uses a narrow `SHADER_SAMPLED_READ` source mask
   (`render.rs:226`) while fill/logic use the broader
   `SHADER_SAMPLED_READ | TRANSFER_WRITE | COLOR_ATTACHMENT_WRITE`
   (`fill.rs:59`); collapsing them to a superset is a real
   synchronization change (conservative, likely correct, but NOT
   no-op). Widen to the superset only in Phase 3 when cross-kind
   batching is intentionally switched on. (Codex round-2.)
2. **Same-kind fill runs** — merge consecutive same-dst fills. Smallest
   real merge; fills are pure `clear_attachments`, no pipeline/descriptor
   state to interleave.
3. **Composite + fill + logic** into one session (the bulk of cross_kind).
4. **Glyph into the session** — densest menu-hover op; verify atlas
   sampling mid-pass + glyph-upload ordering (uploads precede the pass).
5. **(optional) traps tail** — let a trap's dst-composite half rejoin.

Watch `cross_kind` fall and `begin_rendering/s` + `barrier2/s` drop on
eiger between phases.

## Correctness constraints (carried from the parent plan + codex round 1)

- **Self-sample = hard flush** (rule 2). Default to flush when unsure.
- **One pre-barrier / one post-barrier per session**, never per joined op.
- **Descriptor-set + staging + atlas-ticket lifetime**: a longer-lived
  open pass changes nothing about retirement — every touched resource is
  already pinned to the frame ticket via `open_frame.pins` /
  `touched_drawables`; the CB is the same single frame CB. Confirm no op's
  descriptor set is freed before frame retire (it isn't — pins hold them).
- **Damage**: each op already contributes its dst damage; the session
  changes pass boundaries, not damage rects. No change needed.
- **Scissor / render_area**: `open` uses full dst extent as render_area;
  each op sets its own `cmd_set_scissor` before its draws (already true).
  A joining op must re-set scissor + re-bind its pipeline/descriptors
  (dynamic rendering allows pipeline rebind mid-pass).
- **Validation layers**: expect ZERO new render-pass / layout VUIDs on
  ynest under menu-hover + drag. A `SYNC-HAZARD` on dst would mean a
  missing flush (rule 2) — treat as a correctness bug, not a tuning knob.

## Test plan (per phase)

1. Unit: a `DstPassSession` state-machine test (pure, like
   `coalescing_counts`): feed op-classification sequences, assert the
   emitted (open, draws*, close) grouping — same-dst run → 1 open/1 close;
   dst change / read / ineligible → flush; end-of-frame → close.
2. `cargo test -p yserver` green.
3. Validation layers clean on ynest (menu-hover + drag), no layout VUIDs.
4. HW A/B on eiger via `just yserver-xfce-hw-telemetry`: `cross_kind`
   falls, `begin_rendering/s` + `barrier2/s` drop on the storm seconds,
   NO visual regression (GPU correctness here is eye-only — buffer-age
   taught us that).
5. Re-capture on silence (RADV): no submit-rate regression.

## Open questions for codex

1. Is the layout-bookkeeping argument in §5 airtight, or is there a path
   where a mid-session op's omitted barrier leaves dst's GPU layout
   diverged from a later op's declared `old_layout`?
2. Glyph mid-pass: does `record_text_run_scissored` assume it owns the
   begin/end (e.g. its own render_area / load-op), such that calling only
   its draws-half inside a foreign open pass is unsafe?
3. Fill `cmd_clear_attachments` mid-pass honours the bound scissor and the
   session's `render_area` — any interaction with a prior op's draw state
   (viewport/scissor) we must reset?
4. Is forcing a full flush on every traps op (rather than rejoining its
   dst-composite half) leaving meaningful headroom on the table for xfce?
5. Any resource-lifetime hazard from the longer-lived open pass that the
   pin-set / ticket model does NOT already cover?
