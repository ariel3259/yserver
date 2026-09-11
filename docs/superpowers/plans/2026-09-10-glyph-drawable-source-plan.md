# CompositeGlyphs with a drawable source — implementation plan (tier 1)

Implements tier 1 of `../specs/2026-09-10-glyph-drawable-source-design.md`.
Read that first; this plan does not restate its reasoning.

> **Status 2026-09-10 — steps 1-4 landed; 5 and 6 need hardware.**
>
> | step | state |
> |---|---|
> | 1 recognise | `49b84374` |
> | 2 read | `4c50ad2b` — Java text paints |
> | 3 staleness regression | `e9c6f057` |
> | 4 loud remaining drops | `08fa79f8` |
> | 5 measure, then cache | measured in vng (below); the DECISION is open and is jos's |
> | 6 hardware | Swing probe passes in vng; real hardware + xts `Xlib9` A/B still owed |
>
> **No cache exists yet, by design** (step 5 measures first). The
> readback is attributed to `GetImageSite::GlyphSource`, which prints as
> `get_image_by_site/s[… glyphsrc=N]`, so the measurement is a matter of
> reading one field of the per-second telemetry line.
>
> Gates run so far: `cargo +nightly fmt --check`, `cargo clippy
> --all-targets -- -D warnings`, `cargo test --workspace`, and the full
> live acceptance suite on lavapipe — 144 of 145 pass, and the one
> failure (`window_storage_init_covers_the_whole_allocation`) fails
> identically with the branch's changes stashed. It is the known
> uninit-storage defect, not this work.
>
> Step 2's live proof was checked for vacuity: with recognition forced
> off, `uniform_drawable_glyph_source_paints_like_a_solid_fill` fails
> with the destination still at its background colour, and the byte
> order in that diff is what pins the channel order.
>
> **vng A/B, 2026-09-10 — the fix works; step 6's visual deliverable is
> met in a guest, not on hardware.** `tools/vng-scenarios/java-xrender-text.sh`
> runs the reporter's Swing probe under `tools/vng-shot.sh`, with no WM so
> every glyph on screen came from Java. Same scenario, same 21 requests,
> only the binary differs (`49b84374` = recognise-but-drop, i.e.
> behaviourally pre-fix):
>
> | | pre-fix | fixed |
> |---|---|---|
> | `CompositeGlyphs32` dispatches | 21 | 21 |
> | painted rects | 21 x **0** | 21 x **1** |
> | `composite_glyphs gap` lines | 21 | **0** |
>
> Pre-fix the window, its borders and its Close button all paint and not
> one glyph appears; fixed, every sample string renders including the
> accented Czech line and the `Graphics2D.drawString()` panel. The
> `xrender=false` control is correct on both, so the core-X11 text path is
> untouched. Step 1's recognition log names the source exactly as the spec
> predicted: `repeat=Normal mask_fmt=0 domain: None` over a 1x1 storage.
>
> **This is a guest, not hardware.** Pixel behaviour in vng has been
> faithful before (it reproduced the wezterm white band bit-for-bit), so
> the visual result should hold; timings must not be read across.
>
> **Step 5 input — the cost is the frame close, not the readback.** The
> churn scenario (32 strings recoloured every 10ms, the worst case for a
> per-draw read) gives, per second:
>
> - `glyphsrc=122..255` readbacks, and it is the ONLY `get_image` site
>   firing;
> - phase split `drain=104..274ms` vs `wait=17..57ms` vs `copyout=1.5ms` —
>   the drain is 4-8x the fence wait;
> - `frame_builder_opens=237 closes=238` with
>   `close_reasons[sync_wait=179]`: **~75% of all frame closes are this
>   readback's sync-wait**, and `ops/frame_avg=1.6` (max 5). The batching
>   `composite_glyphs_via_frame_builder` exists to provide is gone.
>
> That reframes the step 5 decision rather than settling it, and the
> reframing is the part worth keeping. A cache keyed
> `(DrawableId, content_version, offset)` **misses by construction**
> whenever the client recolours the 1x1 pixmap per string — which is
> exactly what Java does — so it would buy nothing in this workload and
> would only help a client painting many runs in one colour. The
> alternative lever is the ordering itself: not closing the frame per
> read. The spec requires `get_image`'s ordering for a reason — it is what
> stops us racing Java's recolouring — so this is a real tension and needs
> a decision, not a patch. Absolute timings still need real hardware; the
> counts and the close-reason split do not.
>
> **Colour correctness across 32 concurrent colours.** In one churn frame
> all 32 lines have distinct ink colours (32/32), the green channel is
> bit-identical within each group of four and steps 31 per group exactly
> as the probe's formula requires, and red/blue step 20/100 per line to
> within the ±1 of sampling an antialiased pixel. A stale colour would
> show as two lines sharing one; none do. Byte exactness stays pinned by
> the unit tests, not by this.

**Branch:** `fix/137-glyph-drawable-source`, off master. Closes #137.

**Scope:** tier 1 only — a source whose *sampled domain* is one pixel under a
repeat that covers the plane. Tier 2 (general drawable sources) is future
work and must not be started here; the spec says why routing through the
existing batch API does not survive contact.

**Deliverable:** the reporter's Swing probe run with
`-Dsun.java2d.xrender=true` shows every sample string, including the accented
Czech line and the direct `Graphics2D.drawString()` panel.

## Ordering principle

**Recognise, then read, then prove, then measure, then optimise.**

Recognition is pure and testable with no GPU. The read is the part with
ordering and coordinate-space hazards, and is worth landing alone so a
failure is unambiguous. The staleness test comes before any caching, so the
cache is added against a test that already fails when it goes wrong. Caching
is last and only on evidence — the spec requires measuring first.

## Prerequisites

- `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` clean before each commit. The clippy line must be exactly
  CI's (`.github/workflows/ci.yml:35`, and AGENTS.md says so): CI fails on
  any warning, and a run without `-D warnings` only *looks* clean.
- Hardware for steps 5 and 6 only.
- The instrumentation from `3af701db` stays: each step's evidence comes from
  those log lines, and step 4 asserts against the drop counter.

---

## Step 1 — recognise an admissible source

A predicate over `(SourceDrawable, Repeat, storage extent)`, no I/O:

- **`mask_format == 0`.** Tier 1 is exact for Java's direct per-glyph
  route, which is what `mask_format == 0` selects in Xorg
  (`../xserver/render/glyph.c`). For `mask_format != 0` Xorg accumulates an
  A8 mask and composites once; we do not — `render_composite_glyphs` takes
  the parameter as **`_mask_fmt`** and ignores it, always taking the
  per-glyph shortcut. That is a known deviation, and admitting a new class
  of source into it would broaden incorrect behaviour rather than fix
  anything. Keep dropping non-zero mask formats with a drawable source.
- Sampled domain = `SourceDrawable::domain()` when `Some`, else the storage
  extent. Admissible only when it is exactly 1×1.
- Repeat ∈ `{Normal, Pad, Reflect}` — all three map one pixel onto the
  plane. `None` is **not** admissible and must keep dropping.

Nothing else changes yet: an admissible source still falls through to the
existing drop, so this step is behaviour-neutral by construction.

**Proof.** Table test over the cross product of {1×1, 1×2, 2×1, larger} ×
{None, Normal, Pad, Reflect} × {mask_format 0, non-zero}, plus the
redirected shape the spec calls out:
a `SourceDrawable::content(id, offset, 1×1)` whose *storage* is much larger
must be admitted, and a `whole()` over a 1×1 storage must be admitted. A test
that only exercises `whole()` would pass while the window case stays broken.

## Step 2 — read the pixel, ordered and in backing space

Replace the drop for an admissible source with a read:

- `RenderEngine::get_image(store, platform, Src::server_internal(backing_id),
  rect, out_depth)` where `rect` is 1×1 at `SourceDrawable::offset()`.
- `Src::server_internal` (`target.rs:108`), never a client-bounded `Src` —
  the offset is already resolved and a bounded handle would reapply a
  coordinate space.
- `get_image` (`engine.rs:5486`) for the flush/close/wait ordering, never a
  direct map.

Convert the returned pixel to the `ResolvedSource::Solid([f32; 4])`
convention: **premultiplied** RGBA. Two conversions to get right, and both
are silent when wrong:

- `get_image` packs to wire format via `pack_from_storage` keyed on
  `out_depth`. Verify the channel order it produces rather than assuming
  BGRA; a swap turns blue text red and nothing errors.
- A depth-24 source has no alpha and must yield `a = 1.0`. Taking the stored
  byte would give `a = 0` and paint nothing — the same symptom as the bug
  being fixed, which would be maximally confusing.

**Proof, in two parts** — the recording backend cannot serve this. `get_image`
is a Vulkan synchronisation and readback path, so a recording-backend test
would assert nothing about the thing most likely to be wrong.

- **Pure, no GPU:** the byte→premultiplied-`[f32; 4]` conversion, as a free
  function over a pixel buffer. Depth-32 and depth-24 (which must yield
  `a = 1.0`), and a non-grey colour so a channel swap cannot pass. This is
  where the two silent conversions above are actually pinned.
- **Live Vulkan, `#[ignore]`:** the real ordered readback — a 1×1
  `repeat=Normal` drawable source paints pixels identical to the equivalent
  `CreateSolidFill`. **Only an absent required capability may skip**, and it
  must say which one. Once the ICD is up and the capability is present,
  every subsequent stage — seed allocation, painting, readback, the
  assertions — must **fail**, never skip. Skipping on any error is how a
  live proof becomes vacuous, and CI runs the ignored tests on lavapipe, so
  the distinction is load-bearing rather than theoretical. Negative
  cases belong here too: `repeat=None` at 1×1 still drops, a 2×2 source
  still drops, and a non-zero `mask_format` still drops — under tier 1 none
  of those may paint.

## Step 3 — the staleness regression

Java repaints the same 1×1 pixmap to change colour and reuses the picture.
Write this test **before** any caching exists, so the cache in step 5 is
added against a test that already fails when it is wrong.

**Proof.** Paint text; repaint the source drawable a different colour; paint
text again; assert the second run uses the new colour. With no cache this
passes trivially — that is the point: it must be in place and green before a
cache can make it fail.

## Step 4 — make the remaining drops loud

The spec's visibility note. A `CompositeGlyphs` that still cannot be served
— `repeat=None`, a domain larger than 1×1, or a non-zero `mask_format` —
logs at `warn!` on its **first** occurrence, then reverts to debug so a
pathological client cannot flood the log.

**Once per process, explicitly** — not "per generation". A bare backend
flag does not reset itself at a generation boundary, and wiring one into the
reset lifecycle means depending on the #121 reset work, which is unmerged
and parked. Say once-per-process in the code comment so nobody reads
per-generation intent into a flag that does not implement it; revisit if and
when reset lands.

**Proof.** One unsupported request warns; a hundred more do not. Assert the
existing `composite_glyphs_dropped_unsupported` counter still advances on
every one of them — the log is rate-limited, the counter is not.

## Step 5 — measure, then cache only if the measurement says so

A per-draw readback sits on the hottest RENDER path we have. Measure before
deciding:

- `yserver-mate-hw-telemetry` on a text-heavy workload, comparing glyph-path
  cost before and after step 2. The spec's own risk section flags this; do
  not skip to caching because it "obviously" needs it, and do not skip
  caching because one run looked fine.

If the numbers justify a cache, key it on **`(DrawableId, content_version,
offset)`** — never `content_version` alone, which collides across drawables
and across offsets into one backing. Populate only *after* the ordered read
of step 2, never at `CreatePicture`.

**Proof.** Step 3 stays green. Plus: two different drawables at the same
`content_version` do not share an entry, and two different offsets into one
backing do not share an entry. Both are the collisions the key exists to
prevent, and neither is visible without an explicit test.

## Step 6 — hardware

- The reporter's Swing probe (`target/diag137/`), `-Dsun.java2d.xrender=true`:
  all five sample strings, the accented Czech line, the
  `Graphics2D.drawString()` panel. `xrender=false` must remain correct.
- Confirm from the log that the gap no longer fires for the Java client and
  that its requests now report a nonzero painted-rect count.
- xts `Xlib9` A/B per `feedback_xts_ab_gates_pixel_changes`, zero PASS→FAIL,
  since this changes pixels on the text path.

## Hazards

- **Testing the storage extent instead of the sampled domain** rejects the
  redirected window case, which is the one that looks fine in a naive test.
  Step 1's proof exists specifically to catch it.
- **Reading at the storage origin instead of `offset()`** samples the wrong
  pixel of a larger backing, and only in the redirected case — so it will
  look correct in every simple test and wrong on a real desktop.
- **Alpha and channel order** (step 2) both fail silently: wrong alpha paints
  nothing, wrong order paints the wrong colour.
- **Caching before measuring** is the temptation the spec explicitly guards
  against; caching wrongly turns a total failure into an intermittent
  wrong-colour one, which is harder to notice and harder to report than the
  bug being fixed.
- **Do not touch the `CreateSolidFill` fast path.** It serves every other
  client on the desktop; tier 1 adds a branch beside it, not through it.
