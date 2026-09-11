# Component-alpha (subpixel) glyphs — design

**Found by:** #137. Java AWT/Swing text became visible when tier 1 admitted
a drawable glyph source, and most of it renders as **solid blocks** — one
filled rectangle per glyph, correctly positioned and advanced, with no
letterform. Reproduced on hardware (silence, MATE) and in a vng guest.

**Not a regression from #137.** `parse_add_glyphs`, the glyph atlas and the
mask path are byte-identical to `a06cf0e0` on that branch. The blocks were
always there; nothing painted at all before, so nobody could see them.

## The defect

`parse_add_glyphs` (`kms/backend.rs:1063`) reduces an `ARGB32` glyph to its
**alpha byte** and records the stored glyph as `A8`:

```rust
GlyphSetFormat::Argb32 => {
    // memory order [B, G, R, A]
    pixels[row * w + col] = wire[row_off + col * 4 + 3];
    (pixels, GlyphSetFormat::A8)
}
```

Measured on OpenJDK 25's LCD glyphs (temporary instrumentation, one vng run):

```
argb32 glyph 0x1  9x9: alpha distinct=1  [(255, 81)]   max(rgb) distinct=28
argb32 glyph 0x2 11x9: alpha distinct=1  [(255, 99)]   max(rgb) distinct=34
argb32 glyph 0x6  9x9: alpha distinct=1  [(255, 81)]   max(rgb) distinct=46
```

**Alpha is 255 across the entire glyph box** — one distinct value covering
all 81 pixels of a 9×9 glyph. The coverage lives in R, G and B, one channel
per subpixel. So the extracted "A8" glyph is a solid `0xFF` rectangle, which
is exactly what lands on screen.

**The discriminator is the glyphset format, not the source kind.** Java
creates two glyphsets and picks by the desktop's text-antialiasing setting:

```
create_glyphset 0x400012 ynest_format=2 -> A8       (grayscale AA)
create_glyphset 0x400013 ynest_format=4 -> Argb32   (subpixel AA)
```

`-Dawt.useSystemAAFontSettings=lcd` reproduces the blocks; `gasp` renders
correctly. That is why the first vng runs looked perfect while hardware did
not: a bare guest has no settings daemon, so its default was grayscale.
`tools/vng-scenarios/java-xrender-text.sh` takes `YS_JAVA_AA` to force it.

**Xorg is the oracle and it renders this correctly**, with visible subpixel
fringing, from the identical scenario under `vng-shot --server xorg`.

Affects every toolkit that asks for subpixel AA, not just Java — which is
the default on MATE, GNOME and KDE.

## What Xorg does

```c
/* render/render.c:996 */
#define NeedsComponent(f) (PICT_FORMAT_A(f) != 0 && PICT_FORMAT_RGB(f) != 0)
```

At `AddGlyphs` (`render/render.c:1031`) Xorg computes
`component_alpha = NeedsComponent(glyphSet->format->format)` **once per
glyphset**, and creates every glyph's picture with `CPComponentAlpha`
(`render/render.c:1133-1136`). Any format carrying both alpha and RGB —
`ARGB32` — is therefore a component-alpha picture, per glyph, always.

Then, for `maskFormat == 0` (Java's case), `render/glyph.c:653` composites
each glyph as the **mask**:

```c
CompositePicture(op, pSrc, pPicture /* the glyph */, pDst, ...);
```

A component-alpha mask contributes its R, G and B as **per-channel**
coverage; a normal mask contributes only its alpha. Same source picture,
same op — the whole difference is that one flag on the mask.

For `maskFormat != 0` (`render/glyph.c:610`) Xorg derives component-alpha
from the *mask* format, accumulates every glyph into an `ARGB32` mask
picture with `PictOpAdd`, and issues one component-alpha composite. That is
cairo/Pango's subpixel route. **Out of scope here — see the last section.**

Component-alpha semantics, per channel:

```
result.C = src.C * mask.C + dst.C * (1 - src.A * mask.C)
```

## What we already have

Most of this is built. The gap is entirely on the glyph side.

- **The blend state.** `PictOp::blend_factors`
  (`vk/render_pipeline.rs:257-275`) already swaps the `SRC_ALPHA` family for
  `SRC1_COLOR` / `ONE_MINUS_SRC1_COLOR` when `component_alpha` is set, and
  it is `pub(crate)` *specifically* so the text pipeline derives from the
  same table — its own doc comment says so. Nothing there needs to change.
- **The maths.** `shaders/render.frag.glsl:257-263` already implements the
  semantics above, including the per-channel Disjoint/Conjoint variant:
  ```glsl
  s          = vec4(src.rgb * mask_sample.rgb, src.a * mask_sample.a);
  src_alpha  = vec4(src.a   * mask_sample.rgb, src.a * mask_sample.a);
  ```
  with `src_alpha` emitted to output location 1 for the `SRC1_*` factors.
- **The capability and its fallback are already declared.**
  `vk/device.rs:482-506`: `dualSrcBlend` is an optional Vulkan 1.0 feature
  (Broadcom V3D ships without it), the device layer enables only what is
  supported, and it already warns that "a missing dualSrcBlend degrades
  component-alpha (subpixel text AA) to grayscale AA". That promise is
  currently unimplemented for glyphs — today it degrades to *blocks*.
- **The sampler is already exact for this.** The glyph sampler is `NEAREST`
  with `CLAMP_TO_EDGE` (`vk/text_pipeline.rs:207-215`) and glyphs are 1:1
  with the destination, so no filtering can smear across packed channels.

What is missing:

| where | today |
|---|---|
| `kms/backend.rs:1063` | throws R, G, B away at the wire |
| `vk/glyph.rs:46,130` | atlas is a fixed 4096² **`R8_UNORM`**, alpha-only by construction |
| `vk/text_pipeline.rs:350` | hardcodes `component_alpha = false`, "glyph path has no dual-source output" |
| `shaders/text.frag.glsl` | samples `.r` only, one output |

## Design

### Stage 0 — stop destroying the information

An `ARGB32` glyph stays `ARGB32` in `StoredGlyph`. Today `parse_add_glyphs`
rewrites `stored_format` to `A8`; that line is the bug's origin. `A8` and
`A1` are untouched, and `GlyphSetFormat::Argb32` stops being unreachable in
`render_composite_glyphs` (`render/backend.rs:23314` currently treats it as
a defensive "cannot happen" and skips the glyph).

### Stage 1 — where the four coverages live

**The decision this spec exists to make.** Three options:

- **(a) A second atlas image**, `R8G8B8A8_UNORM`, allocated lazily on the
  first component-alpha glyph, with its own view and descriptor. Clean
  separation; conventional. Costs a second descriptor binding (or a second
  set) and a second residency/eviction budget. At the current 4096² an RGBA
  atlas is 64 MiB, so it also needs a size decision the A8 atlas never had
  to make.
- **(b) Pack the four channels as four horizontally adjacent texels in the
  existing R8 atlas** — a `w × h` glyph occupies `4w × h`. No new image, no
  new descriptor, no new memory budget. The shelf packer already takes an
  arbitrary `(w, h)` (`vk/glyph.rs:338`), so the pack call site is a
  one-line change, and the `NEAREST` sampler makes four `texelFetch`es
  exact.
- **(c) One `RGBA` atlas for everything.** Simplest shader — one sample —
  but 4× the memory for the overwhelmingly common A8 case and a rewrite of
  the existing upload path for no benefit to it.

**Four channels, not three.** An earlier draft of this section packed only
R, G and B, on the grounds that Java's alpha byte is 255 and therefore
carries nothing. That is wrong twice over: stage 2 below needs `cov_a` for
the alpha channel's own blend factor, and Xorg's rule
(`NeedsComponent`) applies component-alpha to **every** `ARGB32` glyphset,
not only to the ones whose alpha happens to be constant. Dropping alpha
would work for this JVM and break on the next client. The cost of a
component-alpha glyph is therefore **4×** its area, not 3×.

**Recommend (b), at `4w`.** It reuses the atlas, its packer, its residency
discipline, its eviction and its single descriptor, and the glyph path never
filters so the packing is lossless. Note honestly that at four channels its
*memory* advantage over (a) is gone — both cost 4 bytes per glyph pixel —
and what remains is that no second image, view, descriptor binding or
residency budget has to exist, and that A8 glyphs keep byte-identical
behaviour in the atlas they already live in. (c) is rejected: it taxes every
A8 glyph for a minority case.

**Channel order is part of the contract.** Incoming `ARGB32` wire bytes are
`[B, G, R, A]` (little-endian CARD32, alpha at bits 24-31 — the order
`parse_add_glyphs` already assumes at `kms/backend.rs:1065`). The packed
atlas texels must hold **logical R, G, B, A in that order**. State it in the
code at both the upload and the sample site: this is the same trap that cost
a round in #137 — a channel swap paints plausible-looking text in the wrong
colour and nothing errors.

`AtlasEntry` (`vk/glyph.rs:66`) then needs to carry the **logical** width
alongside the packed width — the dst quad is `w` wide while the atlas
footprint is `4w`. Store them separately rather than deriving one from a
`channels` count: a derived value invites one of the two call sites to use
the wrong one, and the failure mode is a subtly misaligned glyph rather than
an error.

**Rename both fields; do not add one beside `w`.** `AtlasEntry.w` currently
drives *both* meanings, a few lines apart in the same loop: the upload's
copy region (`render/engine.rs:6661`) wants the **packed** width, while the
damage extent and the instance data (`render/engine.rs:6629`, `:6637`) want
the **logical** one. Leaving the name `w` and adding a second field means
every existing use silently keeps whichever meaning it had, and half of them
would be wrong. Rename to `packed_w` and `logical_w` so that each existing
consumer has to be visited and misuse either fails to compile or reads as
obviously wrong. `RecordedTextGlyph.w` and `RecordedGlyphUpload.w`
(`render/frame_builder.rs:457`) are separate copies of the same value and
each must take the correct one of the two.

Audit every consumer of the field as part of this, not afterwards. There is
**no active eviction or LRU path** in the atlas today, so there is no
eviction-side footprint question to answer — worth writing down so nobody
goes looking for one.

Alongside the two widths, `AtlasEntry` carries a **layout enum** —
`A8` | `ComponentAlpha`. That is sufficient, and a separate channel-count
field is **not** wanted: the layout is what selects the pipeline and what
tells the shader whether the explicit plane stride is meaningful, and a
count beside it would be a second encoding of the same fact, free to
disagree with it.

`GlyphKey` is `{font_xid, codepoint}` (`vk/glyph.rs:55`) and `font_xid` is
the glyphset xid, so an A8 and an ARGB32 glyph can never collide on a key.
No key change is needed — but the entry must record which layout it has, or
the shader cannot know how to sample it.

**One caveat on that key.** The same field carries a *core font* host xid
when the glyph came from `image_text` and a *glyphset* host xid when it came
from `composite_glyphs`; both paths share this one atlas. Until now a
collision between the two namespaces would have been merely a wrong glyph
image. Once entries carry a layout, a collision means a core-text glyph
sampling a 4-plane entry as if it were 1-plane. Confirm the two host-xid
namespaces cannot collide (single allocator ⇒ they cannot) and assert it,
rather than leaving it as a property nobody has written down.

### Stage 1b — the coordinate path

Underspecified in the first draft, and the failure modes are silent in both
directions. `text.vert.glsl:46` interpolates **one** atlas rectangle across
the destination quad:

```glsl
v_uv = (atlas_xy + quad * atlas_wh) / pc.atlas_extent;
```

So for a `4w × h` allocation:

- passing `4w` as `atlas_wh` **stretches one glyph across all four planes**;
- passing `w` with no further information **samples only the first plane**.

The contract:

- `atlas_wh` stays the **logical** `(w, h)`. The A8 path's vertex and
  fragment maths then does not change at all, which is invariant 1.
- The **packed allocation width `4w` never reaches the shader.** It exists
  only in the packer (`vk/glyph.rs:338`) and the uploader. `AtlasEntry`
  carries both; only the logical one goes into instance data.
- Add a per-instance **plane stride** in texels and the glyph's **atlas
  origin** in texels, both delivered `flat` to the fragment shader. The
  fragment shader for `COMPONENT_ALPHA` then fetches
  `atlas_origin + ivec2(local.x + k * stride, local.y)` for `k` in `0..4`.
- **The local offset must be spelled out, not gestured at.** `quad *
  dst_size` is integral *at the vertices only* — it is interpolated before
  the fragment shader ever sees it, so "use the integral quantity" is not an
  instruction anyone can implement. Concretely: emit it as a varying
  `v_local` (`quad * dst_size`, so `0..w` × `0..h` across the quad) and take
  `ivec2(floor(v_local))` in the fragment shader. That is exact because the
  quad is pixel-aligned — `dst_origin` and `dst_size` are whole pixels — so
  fragment centres sample `v_local` at `i + 0.5` and `floor` recovers `i`.
  The alternative is a `flat` `dst_origin` plus
  `ivec2(gl_FragCoord.xy) - dst_origin`, which `render.frag.glsl:281`
  already does in spirit for its dst readback; either is fine, but the spec
  picks one and the code comments why.
- What must **not** happen is recovering the offset by multiplying `v_uv`
  back up by `atlas_extent`. That round trip through a normalised divide and
  back lands half a texel out at glyph edges, and the symptom is a
  one-pixel colour fringe on some glyphs and not others.
- Carry the stride explicitly even though it currently equals `dst_size.x`.
  That identity holds only because glyph blitting is 1:1; deriving from it
  couples the sampling to an unrelated invariant, and the failure mode is a
  glyph sampling its neighbour's plane.

This changes the vertex input description and the instance buffer stride.
`TextPushConsts` has compile-time asserts locking its size and field offsets
(`text.vert.glsl:19-23` documents them); the instance layout deserves the
same treatment rather than an unchecked `repr(C)`.

### Stage 2 — the pipeline and the shader

`text.frag.glsl` gains a `COMPONENT_ALPHA` specialization constant. When
set, it gathers the **four** packed coverages — logical R, G, B, A from four
adjacent texels — and emits **exactly what `render.frag.glsl:262-263`
emits**, with the packed coverage standing in for `mask_sample`:

```glsl
// Dual-source: SECOND INDEX of location 0, not a second location —
// exactly as render.frag.glsl:66 declares it. The SRC1_* blend
// factors reference index 1 of attachment 0; a `location = 1`
// output is a second attachment and would not link.
layout(location = 0)          out vec4 out_color;
layout(location = 0, index = 1) out vec4 out_color1;

// cov.rgb = logical R,G,B coverage; cov_a = the glyph's own alpha
out_color  = vec4(fg.rgb * cov.rgb, fg.a * cov_a);
out_color1 = vec4(fg.a   * cov.rgb, fg.a * cov_a);
```

Sampling is by `texelFetch` at `atlas_x + k*w + u` for `k` in `0..4`, not by
UV interpolation across the packed run: the four planes are distinct images
that happen to be adjacent, and any filtering between them is meaningless.
The sampler is already `NEAREST` (`vk/text_pipeline.rs:207-215`), so this is
belt-and-braces rather than a change, but it should be `texelFetch` so the
intent survives a future sampler change.

The two shaders must be read side by side when this is written; a divergence
here is a per-channel blend that is subtly wrong on one path only.

`text_pipeline.rs` stops passing `false` and keys its pipeline cache on
`component_alpha`, as `RenderPipelineCache` already does
(`render_pipeline.rs:438`). Because both derive from `blend_factors`, the
two paths then agree on blend state by construction rather than by review.

### Stage 2b — one request can mix both glyph formats

**The gap that would have broken this in the field.** A single
`CompositeGlyphs` request may switch glyphsets mid-stream: the items parser
tracks a mutable `active_gs_xid` that the inline `count == 255` element
rewrites (`render/backend.rs:23270-23274`). Different glyphsets may have
different formats. So **one request can interleave A8 and ARGB32 glyphs.**

Pipeline state is immutable, and it is chosen **once per recorded glyph
op** — `text_pipelines.get(&(cg.op, format, dst_has_alpha))`
(`render/engine.rs:9089-9091`), after which a single `vkCmdDraw` covers all
`cg.instance_count` instances. Adding `component_alpha` to that key makes a
run homogeneous by construction, which means the mixed request must be
**split**.

**Split into contiguous runs in request order**, each recorded as its own op
with its own pipeline.

**The split belongs in the engine, not the parser.** The runs are formed in
`composite_glyphs_via_frame_builder`, which already owns pipeline
construction, the instance buffer, frame ordering and pin accounting.
Splitting in `render_composite_glyphs` would push all four of those up into
the backend for no gain.

**Ownership of the two facts is split, deliberately.** The backend parser
tags each `CompositeGlyphInput` with the glyph's **source format** — that is
protocol state, read from the glyphset. The **engine** derives the
**effective `GlyphLayout`** from it, because that mapping depends on
`component_alpha_supported`, which is device state the backend has no
business consulting. Keeping the derivation in the engine puts the fallback
decision in the same place as atlas allocation and pipeline choice, so the
three cannot disagree.

What must stay **request-wide**, one per request, not one per run:

- the returned clip region the backend computes and hands back
  (`render/backend.rs:23438` — one `composite_glyphs` call, one region);
- the append-time damage union (`render/engine.rs:6698`, computed from
  `damage_min/max` over the whole call);
- the single `mark_contents_modified`.

A client sees one request; it must see one damage event and one region for
it, however many draws we happened to issue.

**`first_instance` does not exist yet and must be plumbed end to end.**
`RecordedCompositeGlyphs` carries only `instance_pin` and `instance_count`
(`render/frame_builder.rs:463`), and `record_text_run_scissored` hardcodes
the draw's `firstInstance` to zero — `cmd_draw(cb, 4, instance_count, 0, 0)`
(`vk/ops/text.rs:212`). Three sites, all of which must change together:
the recorded op gains the field, emit passes it through
(`render/engine.rs:9089-9103`), and the draw call uses it. A partial job
here draws run 0's glyphs for every run, which looks like duplicated text
rather than like an unplumbed parameter.

**Split on the EFFECTIVE layout, not the glyphset format.** Where
`dualSrcBlend` is absent, stage 3 reduces ARGB32 to A8 at upload — those
glyphs are then genuinely A8 in the atlas and belong in an A8 run. Keying
the splitter on the original glyphset format would manufacture runs that
differ in nothing, splitting draws for no reason. It also gives the fallback
a clean property worth stating: on a device without `dualSrcBlend` every
glyph has the same effective layout, so there is exactly **one** run and the
splitter is inert.

**Do not regroup all the A8 glyphs together.** Grouping by format is the
obvious optimisation and it is wrong: PictOps are not commutative in
general, so reordering changes pixels wherever glyph quads overlap — and
overlap is ordinary, not exotic (kerning, italics, combining marks). The
existing comment at the head of `render_composite_glyphs` already makes the
same point from the other side: the per-glyph and accumulate forms differ
precisely for *overlapping* glyph quads, and are identical only for the
associative `Add`.

The cost is bounded and worth stating: a stream that alternates format every
glyph degrades to one draw per glyph. That is exactly what Xorg does anyway
on this path — `render/glyph.c:653` issues one `CompositePicture` per glyph
for `maskFormat == 0` — so our worst case is Xorg's normal case. Real
clients switch glyphsets per font or AA change, so the common case stays a
single run.

### Stage 2c — the run split must be inside the pin ceiling

**A hard ceiling that the split can walk straight through.** The pre-pass
budgets prospective glyph *uploads* only —
`pending_pins_before_call + prospective_misses > ceiling` forces a
close+reopen before anything is allocated (`render/engine.rs:6429-6436`) —
and the instance buffer is pinned **afterwards**, one `pin_staging` per
call (`render/engine.rs:6762`). One instance pin per call is invisible in
that arithmetic today because it is always exactly one. With N runs it is N,
taken after the check, so a request that splits can exceed a ceiling the
pre-pass has already declared satisfied.

**One shared instance buffer, per-run ranges.** All runs' instance data goes
into a single allocation; each recorded op carries a
`(first_instance, count)` range into it. That keeps the pin count at exactly
one per call regardless of how many runs there are, which is the property
the rest of this section depends on. The alternative — budget `run_count`
pins — needs the run count *before* the pre-pass, so the pre-pass would have
to move or run twice.

**But "one pin per call, arithmetic unchanged" is not a fix.** An earlier
draft of this section stopped there, and that was wrong: the arithmetic is
already off by one, so preserving it preserves a ceiling violation. The
pre-pass compares `pending_pins_before_call + prospective_misses` against
the ceiling and the per-glyph rule compares
`new_uploads.len() + 1 + pending_pins_before_call` — **neither reserves
anything for the instance buffer**, which is then pinned unconditionally.
A call that exactly fills the ceiling with uploads therefore ends the frame
at `ceiling + 1` pins. It is invisible today only because nobody has looked;
the run split does not cause it, but a design that adds draws next to it
must not leave it.

**Reserve the draw buffer's pin in both places.** One pin, budgeted:

- the pre-pass becomes
  `pending_pins_before_call + prospective_misses + 1 > ceiling`;
- the single-call overflow branch (`render/engine.rs:6488`) becomes
  `prospective_misses + 1 > ceiling`;
- the per-glyph admission rule (`render/engine.rs:6567`) becomes
  `new_uploads.len() + 1 + pending_pins_before_call + 1 > ceiling`, i.e.
  when uploads and the draw buffer cannot both fit, **drop uploads to leave
  room for the draw**. Dropping a glyph loses one glyph; losing the draw
  loses the whole run.
- and `note_pin_ceiling_hit_once` (`render/engine.rs:6436`) reports the
  **reserved** total, `pending_pins_before_call + prospective_misses + 1`.
  Its argument is the attempted pin count; leaving it at the unreserved sum
  would have the one diagnostic that exists for this ceiling understate the
  very number the new rule is there to make truthful.

**`image_text` has the identical hole** (`render/engine.rs:5954`, same rule,
same unconditional instance pin) and takes the same reservation. Fixing only
the glyph path would leave core text able to exceed the same ceiling, which
is a worse state to be in than the current uniform bug: two paths that look
alike and behave differently.

**Test the pin count, not the outcome.** A low ceiling plus an alternating
A8/ARGB32 stream, asserting the frame's **actual** `pins.len()` never
exceeds `max_pinned_resources_per_frame()`. "It rendered" passes under the
off-by-one; only counting the pins distinguishes budgeted from lucky.

`cov_a` is the glyph's own alpha, read from the fourth packed texel — the
one place the alpha byte is still used, deliberately. Xorg uses `mask.a` for
the alpha channel's factor and `mask.rgb` for the colour channels' factors
(`render.frag.glsl:262-263` reproduces exactly that), so `cov_a` must come
from the glyph and must **not** be assumed to be 1. It is 1 for this JVM;
that is a property of this client, not of the format.

### Stage 3 — the no-`dualSrcBlend` fallback

Not optional: `device.rs` already promises grayscale AA there, and today it
delivers blocks.

**The reduction happens at upload, not in the shader.** When
`component_alpha_supported` is false, `ARGB32` glyph wire bytes are reduced
to a single A8 plane — coverage = the mean of logical R, G and B,
`(r + g + b + 1) / 3`, rounded to nearest — and packed at **logical width
`w`**, exactly like a real A8 glyph. The existing single-output shader and
pipeline then serve it unchanged.

Note that the mean is **symmetric in its arguments**, so no test of the
reduction can detect an R↔B transposition; only the packed path's
exact-texel assertion can. Do not claim otherwise in the reduction's own
tests — the mistake they actually guard against is reading bytes `1..=3`
(including alpha, dropping blue).

Saying "the shader computes the mean" would not have worked, and this is the
hole the first draft left: with `COMPONENT_ALPHA = 0` the existing shader
samples **one** R8 texel, so against a 4-wide packed glyph it would read the
R plane alone and silently render red-channel-only coverage — not a mean, and
not obviously wrong on screen.

Deciding at upload is also the right shape for what the capability *is*:
`component_alpha_supported` is fixed for the life of the device
(`vk/device.rs:502`), so there is never mixed state in the atlas, and the
reduction is a pure function of the wire bytes — which is what lets it be
tested directly instead of behind a runtime switch
(`feedback_no_feature_kill_switches`).

The consequence is named: no colour fringing, and stroke weight very
slightly different from a true grayscale-AA rasterisation, because the
client rasterised for subpixel geometry. It is a large improvement on blocks
and it is what the device layer already tells the user we do.

Reducing at upload also keeps `image_text` structurally out of this: on a
device without `dualSrcBlend` nothing in the atlas is ever 4-plane, so the
core-text path cannot encounter one.

### Stage 3b — the `image_text` boundary

`image_text` takes `&[PreparedGlyph]` (`engine.rs:8798`), not
`CompositeGlyphInput` — `PreparedGlyph.pixels` is a `Vec<u8>` the server
rasterised itself from a core font, always A8. The two paths are *not*
coupled through the input type.

What they **do** share is the glyph atlas and the recorded text-run op, and
that is where the boundary has to be asserted:

- an `image_text` glyph must always intern as a **single-plane A8** entry;
- the atlas key namespaces must not collide (stage 1's caveat);
- `image_text` must never select a component-alpha pipeline.

State it and test it. `CompositeGlyphInput`'s own doc comment currently
promises "dense A8 (native a8 / ARGB32-preconverted) or raw A1 wire"
(`engine.rs:8807-8815`) — that sentence becomes false under this design and
is the natural place to record the new rule.

Both lavapipe and RADV report `dualSrcBlend = true`, so CI exercises the
**real** path, not the fallback — the fallback needs its own coverage. Make
the reduction a pure function and unit-test it directly; do **not** add a
switch to force the fallback at runtime (`feedback_no_feature_kill_switches`).

## Invariants

1. An `A8` or `A1` glyphset renders **byte-identically** before and after.
   That is the overwhelmingly common path and it must not move.
2. **Colour** coverage for a component-alpha glyphset comes from R, G and B,
   never from the alpha byte. Reading the alpha byte for coverage *is* the
   current defect, and on Java's glyphs it is 255 everywhere, so the failure
   is silent and total.
3. The glyph's alpha is nonetheless preserved and used, for the alpha
   channel's own blend factor. A client whose `ARGB32` glyphs carry a real
   alpha must render correctly; that Java's is constant 255 is a property of
   Java, not of the format.
4. Channel order is fixed and stated at both ends: wire `[B, G, R, A]` →
   packed texels **logical R, G, B, A**. A swap here paints wrong-coloured
   text and errors nothing.
5. `PictOp::blend_factors` stays the single source of blend state for both
   the render and the text pipeline.
6. Where `dualSrcBlend` is absent the result is grayscale-antialiased
   letterforms — never blocks, never nothing, and never one channel
   mistaken for a mean.
7. A recorded glyph run is homogeneous in **effective** layout, and runs
   preserve **request order**. Glyphs are never regrouped by format:
   PictOps are not commutative and glyph quads overlap.
7a. One request produces one clip region, one damage union and one
    `mark_contents_modified`, however many runs it split into. The client
    issued one request and must observe one.
7b. One instance-buffer pin per call, **and that pin is budgeted** — in the
    pre-pass, in the single-call overflow branch and in the per-glyph
    admission rule. A frame never ends above
    `max_pinned_resources_per_frame()`. When uploads and the draw cannot
    both fit, uploads are dropped.
7c. The close+reopen decision stays **before** the per-glyph walk, exactly
    where it is now. The engine routine is entered once per request and
    forms runs internally; it is never invoked once per run. That is what
    keeps 7a true and the frame ordering intact.
8. `image_text` only ever forms A8/A1 glyphs and only ever interns
   single-plane atlas entries. It shares the atlas and the recorded text-run
   op with `composite_glyphs`, so this is a boundary to assert, not a
   property to assume.
9. The packed allocation width never reaches the shader. Instance data
   carries the logical width and an explicit plane stride.
10. Xorg is the oracle for pixels, run from the same scenario in the same
    guest, not from a reading of the spec
    (`feedback_check_xorg_fb_impl_for_render_conventions`).

## Proof

- **Pure, no GPU:** the wire → packed-atlas conversion, over a glyph with
  **four mutually distinct, asymmetric channel values** (say
  `B=0x20 G=0x60 R=0xA0 A=0xE0`), asserting the packed texels are exactly
  `[0xA0, 0x60, 0x20, 0xE0]`. Deliberately not a `r != g != b` style check:
  that passes with red and blue reversed, which is the single most likely
  mistake here. A real measured Java glyph goes in alongside as a second
  fixture (alpha constant 255, `max(rgb)` spanning 28 distinct values), so a
  regression to the alpha byte fails loudly instead of producing a
  plausible-looking solid block.
- **Pure:** the stage-3 upload-time reduction — mean of logical R, G, B, at
  logical width `w` — and that a component-alpha glyph and an A8 glyph each
  take their own path and never the other's.
- **Pure, and this one is the new gap:** the run splitter. Feed an items
  stream that switches glyphsets mid-element via the `count == 255` inline
  form, alternating A8 and ARGB32, and assert the resulting runs are
  (a) homogeneous, (b) **in the original glyph order**, and (c) that no
  glyph is lost or duplicated at a boundary. A test that only checks
  homogeneity would pass a splitter that regroups, which is the wrong
  answer for every op but `Add`.
- **Pure:** the splitter keys on effective layout — with the
  no-`dualSrcBlend` reduction in force, an alternating stream yields
  **one** run, not many.
- **Live Vulkan, `#[ignore]`, at a lowered pin ceiling:** an alternating
  A8/ARGB32 stream, asserting the frame's **actual `pins.len()`** never
  exceeds `max_pinned_resources_per_frame()`. Assert the count, not that it
  rendered — "it rendered" passes under the current off-by-one, so only the
  count distinguishes budgeted from lucky. A case that exactly fills the
  ceiling with uploads is the one that catches the missing reservation.
- **The same test for `image_text`**, which carries the identical rule and
  the identical hole.
- **Live Vulkan, `#[ignore]`:** a split request still reports **one**
  damage union and **one** returned region. Assert the region, not just
  the pixels — a per-run region is the kind of change that looks harmless
  and breaks a compositor's damage tracking.
- **Live Vulkan, `#[ignore]`:** the mixed-format request end to end — an A8
  glyph and an ARGB32 glyph in ONE `CompositeGlyphs`, with **overlapping**
  quads and a non-commutative op, so a regrouping splitter produces
  visibly different pixels and fails.
- **Live Vulkan, `#[ignore]`:** the coordinate path in isolation — a
  component-alpha glyph whose four planes hold four distinct constants, so
  a stretched `atlas_wh`, a first-plane-only sample and a half-texel offset
  each produce a different, recognisable wrong answer.
- **Pure or live:** an `image_text` run interns single-plane entries only,
  and the core-font / glyphset atlas key namespaces do not overlap.
- **Live Vulkan, `#[ignore]`:** an ARGB32 component-alpha glyphset paints
  per-channel coverage. Seed the glyph with known asymmetric per-channel
  coverage and assert the destination's R, G and B against **exact expected
  values** derived from the component-alpha equation — not merely that they
  differ, which a red/blue swap satisfies. A grayscale result has
  `r == g == b`, so this assertion separates a real component-alpha
  composite from a convincing approximation of one *and* pins the mapping.
  Only an absent ICD may skip.
- **Live Vulkan, `#[ignore]`:** an A8 glyphset is byte-identical to its
  pre-change output (invariant 1).
- **vng A/B:** `java-xrender-text.sh` with `YS_JAVA_AA=lcd`, yserver vs
  `--server xorg`, in the same guest.
- **Hardware:** MATE, whose default is subpixel AA, plus the reporter's
  probe. Blocks must be gone and the accented line must be legible.
- **No regression:** xts `Xlib9` A/B, zero PASS→FAIL
  (`feedback_xts_ab_gates_pixel_changes`) — this changes pixels on the text
  path.

## Risks

- **The pack/UV arithmetic of option (b) fails as wrong-coloured fringes**,
  which reads as "subpixel AA looks a bit off" rather than as a bug. The
  per-channel live assertion above exists because an eyeball will not catch
  it; a screenshot of correct-looking text is not evidence here.
- **Two shaders implementing one equation.** `text.frag.glsl` and
  `render.frag.glsl` would both carry component-alpha maths. They cannot
  share code today, so they must be reviewed against each other explicitly.
- **`image_text` shares the atlas and the recorded text-run op** with
  `composite_glyphs` (not the input type — see stage 3b). Core X11 text is
  A8 and must stay on the untouched path; those two shared surfaces are
  where a component-alpha change can leak into core text.
- **Run splitting turns one draw into several**, which interacts with the
  glyph draw-batching work the text pipeline exists to serve. The bound is
  known (one draw per glyph, i.e. Xorg's own behaviour) but the telemetry
  worth watching on the first hardware run is the draw count per
  `CompositeGlyphs`, not just the pixels.
- **Atlas pressure.** Component-alpha glyphs are **4×** the area. A desktop
  where every client uses subpixel AA — which is the default on MATE, GNOME
  and KDE — fills the atlas ~4× faster, and `glyphs_dropped_atlas_full` is
  the counter that would show it. Worth watching on the first hardware run
  rather than assuming 4096² still suffices; a glyph dropped for a full
  atlas is invisible text again, i.e. #137's symptom by another route.
- **A 4-wide glyph shelf-packs differently.** The packer is width-first
  within a shelf (`vk/glyph.rs:343-345`); quadrupling every component-alpha
  glyph's width changes which glyphs share a shelf and how much of each
  shelf is wasted. Not a correctness risk, but it is why the pressure above
  cannot be predicted from the 4× factor alone.

## Out of scope

- **`mask_format != 0`** — cairo/Pango's subpixel route, where Xorg
  accumulates glyphs into an `ARGB32` mask with `PictOpAdd` and composites
  once (`render/glyph.c:600-624`). We ignore `mask_format` entirely today
  (`render_composite_glyphs` takes it as `_mask_fmt`), so this needs the
  real temporary-target lifecycle that the #137 design already named as
  future work for masks. Java does not need it: all 21 requests carried
  `mask_format = 0`, measured in **both** AA modes.
- **Colour (emoji) glyphs.** `NeedsComponent` makes Xorg treat *any* A+RGB
  glyphset as component-alpha, so a colour-emoji glyphset gets the same
  treatment there — which is why colour emoji through RENDER glyphs is a
  known mess everywhere. This design follows Xorg rather than trying to
  detect intent, and says so; a real colour-glyph path is separate work.
- **Tier 2 of #137** (general drawable glyph sources) is unrelated and
  stays where its own spec left it.
