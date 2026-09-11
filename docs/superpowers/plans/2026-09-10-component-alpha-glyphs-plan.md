# Component-alpha (subpixel) glyphs — implementation plan

Implements `../specs/2026-09-10-component-alpha-glyphs-design.md` (agreed
complete over five review rounds, `78e36738`). Read that first; this plan
does not restate its reasoning.

**Branch:** `fix/137-glyph-drawable-source` — the same branch, because this
is the second half of making Java text actually render
(`feedback_one_branch_per_issue`).

**Deliverable:** on a subpixel-AA desktop (MATE's default), the reporter's
Swing probe renders letterforms, not blocks, with subpixel fringing that
matches Xorg's on the same scenario.

## Ordering principle

**Fix the pre-existing bug first, then make blocks legible, then make them
correct.**

Two things drive the order. The pin-ceiling off-by-one is a pre-existing
defect that this work would otherwise sit on top of, so it lands alone where
a regression is unambiguous. And the upload-time grayscale reduction — the
spec's `dualSrcBlend` fallback — is a **complete, shippable fix for the
blocks** on its own, needing no atlas, pipeline or shader change. Landing it
third means the risky work (4-plane packing, a second shader path, run
splitting, `first_instance`) all happens on top of a known-good, visually
verified baseline that each later step can be A/B'd against.

The alternative — keep ARGB32 glyphs as ARGB32 first — would take LCD text
from blocks to *nothing* mid-branch, because `render_composite_glyphs`
currently skips a stored `Argb32` glyph defensively
(`render/backend.rs:23314`). Not acceptable even transiently.

## Prerequisites

- `cargo +nightly fmt -- --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test --workspace` clean before each commit — exactly
  CI's lines (`.github/workflows/ci.yml:31-38`). A clippy run without the
  deny only *looks* clean.
- The live suite on lavapipe:
  `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test -p
  yserver --test render_acceptance -- --ignored`. Baseline is **144/145**;
  `window_storage_init_covers_the_whole_allocation` fails on this branch and
  on `a06cf0e0` alike (known uninit-storage defect). Do not re-derive that.
- The reproducer:
  `tools/vng-shot.sh --scenario tools/vng-scenarios/java-xrender-text.sh
  --env YS_JAVA_AA=lcd`, and the same with `--server xorg` for the oracle.
  `YS_JAVA_AA=gasp` is the A8 control that must never change.
- lavapipe and RADV both report `dualSrcBlend = true`, so CI exercises the
  real component-alpha path. The reduction path is pure and unit-tested; do
  not add a switch to force it (`feedback_no_feature_kill_switches`).
- Hardware for step 6 only.

---

## Step 1 — the pin-ceiling reservation (pre-existing bug, alone)

Reserve the instance buffer's pin so a frame cannot end above
`max_pinned_resources_per_frame()`. Four edits, all in
`render/engine.rs`, and the same rule in **both** glyph paths:

- pre-pass `:6430` → `pending + prospective_misses + 1 > ceiling`;
- `note_pin_ceiling_hit_once` `:6436` → reports that same reserved total;
- single-call overflow `:6488` → `prospective_misses + 1 > ceiling`;
- per-glyph admission `:6567` (and `image_text`'s identical `:5954`) →
  `new_uploads.len() + 1 + pending + 1 > ceiling`, dropping uploads to
  leave room for the draw.

Nothing about component-alpha yet.

**Proof.** Red first is legitimate here and the oracle is not a hypothesis:
the ceiling is a limit the code itself declares, so "a frame's `pins.len()`
never exceeds `max_pinned_resources_per_frame()`" is read off the existing
contract, not guessed (`feedback_red_first_needs_verified_oracle`). Write
that assertion at a lowered ceiling against a call that exactly fills it,
watch it fail on today's code, then fix. One test per path — the
`composite_glyphs` and `image_text` holes are separate code.

Assert the **count**, never "it rendered": the off-by-one renders fine.

**Expected behaviour change:** a call that exactly fills the ceiling now
drops one more glyph than before. That is the correct trade — see the spec.
`stats.glyphs_dropped` moves by one in that case and nowhere else.

## Step 2 — `AtlasEntry`: rename the width, add the layout

Pure refactor, behaviour-neutral by construction:

- `AtlasEntry.w` → `packed_w` **and** `logical_w`, both equal today;
- add `layout: GlyphLayout` (`A8` | `ComponentAlpha`), always `A8` today;
- visit every consumer the compiler surfaces. The upload copy region
  (`:6661`) takes `packed_w`; the damage extent (`:6629`) and the instance
  data (`:6637`) take `logical_w`. `RecordedTextGlyph.w` and
  `RecordedGlyphUpload.w` (`render/frame_builder.rs:457`) are separate
  copies and each takes the one it means.

The rename is the point: a field added beside `w` would leave every existing
use silently keeping whichever meaning it had, and half of them would be
wrong.

**Proof.** The existing suite cannot carry this step. While
`packed_w == logical_w`, **either field produces identical pixels**, so a
green suite and a green 144/145 say nothing about whether each consumer took
the field it means — which is the entire reason for the rename.

So: a targeted test with **deliberately asymmetric synthetic widths** on an
`AtlasEntry`, asserting that

- `RecordedGlyphUpload` receives `packed_w`;
- `RecordedTextGlyph`, the damage extent and the instance geometry all
  receive `logical_w`.

That proves the rename's premise directly, and it is the test that makes
step 5 safe — step 5 is the first step where the two widths differ, and by
then this must already be pinned. Run the full and live suites too, but as
a regression net rather than as the proof.

**Carry-forward, found while implementing this step.** The `logical_w` half
is assertable here, through production, by shrinking a cached entry's
`logical_w` under a live draw. The **`packed_w` half is not**: production
constructs `packed_w == logical_w` from the same variable at every upload
site, so no real call path can produce an asymmetric *upload* until step 5's
`pack(4 * logical_w, h)` exists. That is a structural limit, not a
Vulkan-availability one.

So **step 5 must carry the missing assertion**: that `RecordedGlyphUpload`
receives `packed_w` and not `logical_w`. It is the first step where that is
reachable, and it is the half whose failure mode is an upload copying a
quarter of the glyph.

## Step 3 — make the blocks legible: reduce at upload

The spec's stage 3, landed here as the **universal** behaviour so it is a
shippable fix on its own. `parse_add_glyphs` stops rewriting an `ARGB32`
glyph's `stored_format` to `A8`; the engine's upload path reduces the four
channels to one A8 coverage plane — the mean of logical R, G, B — at logical
width `w`. The existing single-output shader and pipeline serve it unchanged.

Wire order is `[B, G, R, A]`; the reduction reads logical R, G and B. The
mean is `(r + g + b + 1) / 3` — rounded to nearest, fixed here so step 5's
fallback cannot quietly disagree.

After this step **LCD text renders as grayscale-antialiased letterforms**.
Blocks are gone. Step 5 narrows this code's condition to
`!component_alpha_supported` rather than replacing it.

**Proof.**

- **Pure:** the reduction over a glyph with four mutually distinct
  asymmetric channels (`B=0x20 G=0x60 R=0xA0 A=0xE0`), asserting the exact
  expected coverage.

  **But be honest about what this fixture can and cannot catch.** The mean
  is *symmetric in its arguments*, so an R↔B transposition is
  mathematically undetectable at this step — no fixture can catch it, this
  one included. What it does catch, and what the realistic form of the trap
  is here, is the **off-by-one**: reading bytes `1..=3` as if they were
  R, G, B, which includes alpha and drops blue. Verify that by mutation
  rather than assertion — replace the reduction with the old alpha read and
  with the `1..=3` mean, and confirm each fails.

  Channel **order** only becomes observable in step 5, where the exact
  packed-texel assertion `[0xA0, 0x60, 0x20, 0xE0]` does pin it. So the
  guard against a swap is real, one step later than this.
- **Pure:** the real measured Java glyph as a second fixture (alpha constant
  255, `max(rgb)` spanning 28 distinct values), so a regression to the alpha
  byte fails loudly instead of producing a plausible solid block.
- **vng:** `YS_JAVA_AA=lcd` — letterforms, and the Czech accented line
  legible. `YS_JAVA_AA=gasp` unchanged.
- **Live suite** still 144/145.

## Step 4a — `first_instance`, end to end, with one run

Plumb the field before anything depends on it:
`RecordedCompositeGlyphs` gains `first_instance`
(`render/frame_builder.rs:463`), emit passes it through
(`render/engine.rs:9089-9103`), and `record_text_run_scissored` uses it in
`cmd_draw`'s fourth argument instead of the hardcoded `0`
(`vk/ops/text.rs:212`).

Also switch to one shared instance buffer with per-run ranges, still with
exactly one run per call. Behaviour-neutral.

**This step needs a seam, or its proof is unreachable.** Before step 4b,
`composite_glyphs_via_frame_builder` forms exactly one run, so "record it as
two runs" is not something a test can ask for. Introduce the run-recording
helper here: a small internal function taking `(first_instance, count)`
ranges over the shared buffer, called by production with a single range and
by the test with two. It must be the **same** function production uses for
the one-run case — a test-only parallel path would prove nothing about the
code that ships, and step 4b consumes this helper anyway, so it is a seam
rather than scaffolding.

**Proof.** An all-A8 stream recorded as **two ranges** through that helper
renders identically to the same stream as one range. A `first_instance` left
at zero draws run 0's glyphs twice, which this catches and which no later
test would distinguish from a splitter bug.

Complement it with a direct unit test of `record_text_run_scissored`
(`vk/ops/text.rs:212`) at a nonzero `first_instance`, so the draw-call
argument is pinned at its own level rather than only through the helper.

**Carry-forward from 4a — a latent trap already fixed, but untested.** Runs
after the first must record `SHADER_READ_ONLY_OPTIMAL` as `dst_old_layout`,
not the request's pre-op layout: that is where the previous run leaves the
image, since `record_text_run_scissored` ends there. Carrying the pre-op
layout on run 2+ declares a wrong `oldLayout` in its barrier, and a pre-op
`UNDEFINED` would license the driver to **discard the earlier runs' pixels**.

4a fixed it inside the seam but could not test it: it is inert with one run,
and an `UNDEFINED` `oldLayout` is *permitted* to preserve contents, so a test
on lavapipe would be a coin flip rather than an oracle. **4b is the first
place it is assertable**, because multi-run becomes reachable through
production. The seam it needs is a way to force a
non-`SHADER_READ_ONLY_OPTIMAL` pre-frame layout on a destination that then
takes two runs.

**Correction to an earlier note here:** step 2's `RecordedGlyphUpload` /
`packed_w` assertion is **step 5's** carry-forward, not 4b's — `packed_w`
only diverges from `logical_w` when the 4-plane packing lands. 4b owns the
layout trap above and nothing else inherited.

**And the oracle for it is white-box, not pixels.** 4a declined to test it
because an `UNDEFINED` `oldLayout` is *permitted* to preserve contents, so a
pixel assertion on lavapipe passes whether the barrier is right or wrong.
Assert the **recorded value** instead: drive `record_glyph_runs` with two
runs and assert that run 2's `RecordedCompositeGlyphs.dst_old_layout` is
`SHADER_READ_ONLY_OPTIMAL` while run 1's carries the request's pre-op layout.
That oracle is exact and driver-independent, and it is reachable now through
the seam rather than waiting for production multi-run.

## Step 4b — the run splitter

The backend parser tags each `CompositeGlyphInput` with the glyph's **source
format** — protocol state, read from the glyphset. The **engine** derives
the **effective `GlyphLayout`** from it, because that mapping depends on
`component_alpha_supported`, which is device state the backend should not be
reading. That keeps the fallback decision alongside atlas allocation and
pipeline choice, where it cannot drift from either.

Runs are then formed over the effective layout in
`composite_glyphs_via_frame_builder`, in request order, through the step-4a
helper. One clip region, one damage union, one `mark_contents_modified` per
request; close+reopen stays before the per-glyph walk; the engine routine is
entered once per request.

Inert in production at this point — step 3 reduces everything, so every
glyph has the same effective layout and there is exactly one run.

**Proof.** Pure, and it must assert **order**:

- an items stream that switches glyphsets mid-element via the `count == 255`
  inline form, alternating formats, yields runs that are homogeneous, **in
  the original glyph order**, with no glyph lost or duplicated at a
  boundary. A test that checks only homogeneity passes a splitter that
  regroups — the wrong answer for every op but `Add`.
- with the reduction in force, that same alternating stream yields **one**
  run.

**Carry-forwards into step 5 — three, all earned by earlier steps.**

1. **`effective_glyph_layout` must gain the device parameter, here and
   nowhere else.** 4b placed the derivation in the engine as designed, but
   deliberately did **not** thread `component_alpha_supported` into it,
   because it cannot yet mean anything: step 3's reduction is unconditional,
   so answering `ComponentAlpha` would tag entries with a layout the
   single-plane upload does not produce, and would break 4b's own inertness
   proof. Threading an unused parameter, or hedging behind a constant-`true`
   flag, would have been a kill-switch in disguise. Step 5 is where that
   function starts answering `ComponentAlpha`, and its doc comment names the
   insertion point.
2. **Assert `RecordedGlyphUpload` receives `packed_w`, not `logical_w`**
   (step 2's carry-forward). Step 5 is the first step where the two widths
   differ, so it is the first step where this is reachable. The failure mode
   is an upload copying a quarter of the glyph.
3. **Fold in a simplification 4b identified:** `CompositeGlyphInput.source_format`
   is structurally redundant with the `GlyphPixels` variant — the mapping is
   total and 1:1. 4b built both from a single `match` so they cannot drift,
   but a `GlyphPixels::source_format()` accessor would remove the field
   outright with no behaviour change. Step 5 already touches this area.

**Two coverage gaps 4b reported honestly, to be closed here or explicitly
accepted.**

- A mutation **survived**: making the `draw_layouts` push conditional on
  `logical_w != 1` breaks nothing, because no live test uses a one-pixel-wide
  glyph. The lockstep between `glyphs_to_draw` and `draw_layouts` is
  otherwise guarded only by a `debug_assert`.
- The pre-existing test `composite_glyphs_inline_glyphset_change_parsed`
  **survives** a parse that ignores the inline `count == 255` glyphset
  change — i.e. it does not test what its name claims. 4b's new tests do
  catch it. Worth fixing or renaming while nearby.

## Step 5 — the real component-alpha path

The payload. These are mutually dependent and land together:

- `packed_w = 4 * logical_w` and `layout = ComponentAlpha` when
  `component_alpha_supported`; the upload writes four planes in logical
  R, G, B, A order.
- `text.frag.glsl` gains `COMPONENT_ALPHA`, four `texelFetch`es at
  `atlas_origin + ivec2(local.x + k*stride, local.y)`, `local` from a
  `v_local` varying via `ivec2(floor(v_local))`, and a second output
  declared `layout(location = 0, index = 1)` — the second **index** of
  attachment 0, as `render.frag.glsl:66` does. A `location = 1` output is a
  second attachment and will not link.
- instance data carries `logical_w` and an explicit `flat` plane stride;
  the `4w` allocation width never reaches the shader.
- `text_pipeline.rs:350` stops passing `false` and keys its cache on
  `component_alpha`, deriving blend state from the shared
  `PictOp::blend_factors`.
- step 3's reduction narrows to `!component_alpha_supported`.

Read `text.frag.glsl` and `render.frag.glsl:257-263` side by side while
writing this; the two then carry one equation in two places.

**Proof.**

- **Live, `#[ignore]`:** per-channel coverage — seed known asymmetric
  per-channel coverage and assert the destination's R, G and B against
  **exact expected values** from the component-alpha equation. Not "they
  differ": a red/blue swap satisfies that. A grayscale result has
  `r == g == b`, so this separates a real component-alpha composite from a
  convincing approximation *and* pins the mapping.
- **Live, `#[ignore]`:** the coordinate path alone — four planes holding
  four distinct constants, so a stretched `atlas_wh`, a first-plane-only
  sample and a half-texel offset each produce a different recognisable
  wrong answer.
- **Live, `#[ignore]`:** a mixed-format request — one A8 and one ARGB32
  glyph in ONE `CompositeGlyphs`, **overlapping** quads, non-commutative
  op, so a regrouping splitter fails.
- **Live, `#[ignore]`:** a split request still reports **one** damage union
  and **one** returned region. Assert the region, not just the pixels: a
  per-run region looks harmless and breaks compositor damage tracking.
- **Live, `#[ignore]`:** an A8 run interns single-plane entries only, and
  the core-font / glyphset atlas key namespaces do not overlap.
- **Live, lowered ceiling:** the alternating stream's `pins.len()` still
  within the limit, now that runs and 4× uploads both exist.
- **vng A/B:** `YS_JAVA_AA=lcd` on yserver vs `--server xorg`, same guest.
  Fringing present on both.
- **Live suite** 144/145 throughout.

## Step 6 — hardware

- MATE (subpixel AA is its default) plus the reporter's Swing probe: all
  five sample strings, the Czech accented line, the `Graphics2D.drawString()`
  panel. Blocks gone; fringing present.
- A drawables dump (`Ctrl+Alt+F12`) so the probe's own storage can be read
  independently of compositing, as the diagnosis run did.
- `glyphs_dropped_atlas_full` and the per-`CompositeGlyphs` draw count from
  the telemetry rollup: component-alpha glyphs are 4× the area and runs can
  multiply draws, and those are the two counters that would show either
  going wrong.
- xts `Xlib9` A/B, zero PASS→FAIL
  (`feedback_xts_ab_gates_pixel_changes`) — this changes pixels on the text
  path. vng is **not** valid for the xts gate (`reference_xts`).

## Hazards

- **Renaming `AtlasEntry.w` in step 2 is where a silent wrong-field pick can
  enter**, because every consumer moves at once and the compiler accepts
  either field — and while the two widths are equal, so do the pixels. Only
  the asymmetric-metadata test catches it, and it must exist before step 5
  makes the widths differ.
- **The coordinate path fails as wrong-coloured fringes**, which read as
  "subpixel AA looks a bit off" rather than as a bug. A screenshot of
  correct-looking text is not evidence; the per-channel assertion is.
- **A half-texel error appears on some glyphs and not others**, depending on
  quad alignment — so a single-glyph test can pass while text is wrong.
- **Step 3 is a shippable stopping point.** If step 5 proves harder than
  expected, stopping after step 3 leaves legible grayscale text rather than
  a half-built component-alpha path. Say so rather than pressing on.
- **Do not regroup glyphs by format** for a draw-count win. It is the
  obvious optimisation and it is wrong for every op but `Add`.
- **`image_text` shares the atlas and the recorded op**, not the input type.
  Those two surfaces are where a component-alpha change can leak into core
  X11 text.
