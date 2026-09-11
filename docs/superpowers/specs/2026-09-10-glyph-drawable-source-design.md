# CompositeGlyphs with a drawable source — design

**Issue:** #137 — Java AWT/Swing text is invisible under yserver, cured by
`-Dsun.java2d.xrender=false`. Reported by kaaduu on an AMD 780M; reproduced
on silence (RX 6800) with the reporter's standalone Swing probe.

## The defect

`KmsBackend::render_composite_glyphs` accepts only two kinds of source
picture. Anything else is dropped:

```rust
ResolvedSource::Drawable(_) | ResolvedSource::None => {
    log::debug!("render composite_glyphs gap: src 0x{host_src:x} is not
                 SolidFill / Gradient (plan §3d v1-parity scope)");
    self.telemetry.record_composite_glyphs_dropped_unsupported();
    return Ok(Vec::new());
}
```

Java2D's XRender pipeline paints text through `XRSolidSrcPict`, which is a
**1×1 pixmap picture with `repeat=Normal`** — not `CreateSolidFill`. So it
resolves to `ResolvedSource::Drawable` and every text draw is discarded.
Nothing is painted, no error is returned, and the client cannot tell.

Measured on the failing run: 32 `CompositeGlyphs` requests from the Java
client, 32 gap hits, all naming the same source picture, and zero rectangles
painted. No other client on the desktop trips it — cairo/pango and the MATE
stack use `CreateSolidFill`, which resolves to `ResolvedSource::Solid`.

**A correlation to discard:** Java uses `CompositeGlyphs32` exclusively, and
in the log 8/16 were 3429/3429 healthy while 32 was 33/33 dead. That is
coincidence — the 32-bit id path decodes correctly. The discriminator is the
**source picture kind**, not the glyph-id width. Do not "fix" the 32-bit
path.

## What Xorg does

`miGlyphs` (`../xserver/render/glyph.c:575`) never collapses the source to a
colour. It has two branches, and the source is a sampled picture in both:

- **`maskFormat != 0`** — allocate a mask pixmap over the glyph extents,
  accumulate every glyph into it with `PictOpAdd`, then issue a single
  `Composite(op, pSrc, pMask, pDst, ...)`.
- **`maskFormat == 0`** — no intermediate. Composite each glyph directly as
  `Composite(op, pSrc, glyphPicture, pDst, ...)`: the glyph is the mask and
  the source is sampled per glyph.

**Java sends `mask_format = 0`**, so the reference behaviour for this bug is
the second branch. Our own path is shaped like neither: it reduces the source
to `foreground_rgba: [f32; 4]` and multiplies the glyph coverage by it
(`RenderEngine::composite_glyphs`, `engine.rs:6183`). That is a valid
optimisation *only* when the source really is one colour.

## Design

Two tiers. Tier 1 fixes #137 exactly and is not a workaround; tier 2 closes
the stub.

### Tier 1 — a uniform source is a colour, and may be treated as one

A picture whose **sampled domain** is a single pixel, repeated, is by
definition a constant colour over the whole destination. Collapsing it to
`foreground_rgba` is then **exact**, not an approximation — the same answer
Xorg computes, by a cheaper route.

Admit a `ResolvedSource::Drawable` when its sampled domain is 1×1 and the
repeat makes that pixel cover the plane. Read that pixel, convert to
premultiplied f32 in the engine's existing convention, and proceed down the
current path unchanged.

**"1×1" means the sampled domain, not the backing storage.**
`SourceDrawable` (`engine.rs:8244`) is `{ id, offset, domain:
Option<Extent2D> }`: `whole()` samples the storage from its origin, while
`content()` carries a content origin *inside* a larger storage plus the
window's own extent. A window picture can therefore resolve to a 1×1 logical
domain sitting inside a much larger redirected backing. The test must be on
the logical domain — `domain` when present, the storage extent when it is
`None` — and the pixel must be read at `SourceDrawable::offset()`, not at
(0,0). Testing the storage extent instead would reject the redirected case,
and reading at the origin would sample the wrong pixel of the backing.
Resolution happens in `resolve_picture_for_render` (`backend.rs:6126`).

**Which repeats qualify.** `Normal`, `Pad` and `Reflect` all map a one-pixel
domain onto the whole plane, so all three are exact. `Repeat::None` is
**not** admissible even at 1×1: outside the single pixel the source reads as
transparent, so the correct result paints only the glyph area overlapping
that pixel. Collapsing it to a colour would paint every glyph. Keep dropping
`None`.

**The read must be X11-ordered, and taken in backing space.** Use
`RenderEngine::get_image` (`engine.rs:5486`), the synchronous readback path,
which performs the flush/close/wait sequence and so observes a repaint of the
source pixmap the client submitted just before the text. A direct image map
or raw read offers no such guarantee and would race Java's own recolouring.

Read through **`Src::server_internal(backing_id)`** (`target.rs:108`) with a
1×1 rectangle at `SourceDrawable::offset()`. That handle is the privileged,
unclipped, backing-space read. A client-bounded `Src` carries its own
`bounds` and would reapply a coordinate space on top of the offset we have
already resolved — sampling the wrong pixel precisely in the redirected case
this section exists to get right.

**Caching, if measurement shows it is needed.** A per-draw readback sits on
the hottest RENDER path we have, so the plan must measure it before
accepting the naive form.

The key is **`(DrawableId, content_version, offset)`** — identity plus
version plus sampled origin, following the shape the clip-mask cache already
uses. `content_version` (`store.rs:702`, bumped by `mark_contents_modified`)
is *not* sufficient alone: versions are local to an allocation, so they
collide across drawables, and the same backing can legitimately be sampled at
several content offsets at the same version. Entries are populated only
*after* the ordered readback above, never at `CreatePicture` time. Java repaints the same 1×1 pixmap to change text
colour and reuses the picture, so a value cached at creation renders every
subsequent run of text in the previous colour. That converts a total,
obvious failure into an intermittent wrong-colour one, which is strictly
worse.

### Tier 2 — general drawable sources (FUTURE WORK, not part of #137)

Deliberately **not** designed here, and not to be attempted alongside tier 1.
Sketching it is useful only to show tier 1 does not paint us into a corner.

The obvious move — "just route glyphs through
`RenderEngine::try_append_render_batch` (`engine.rs:4789`), which already
takes `src: ResolvedSource` with `src_repeat`" — does not survive contact.
That API expects the **mask** to be a `SourceDrawable` too, whereas glyph
coverage lives in the glyph atlas, in atlas coordinates. Closing the stub
therefore requires designing, at minimum:

- an adapter presenting atlas-resident glyph coverage as a maskable source,
  including the atlas-to-destination coordinate mapping; and
- for `mask_format != 0`, a real temporary A8 target with a defined
  lifecycle and ownership — Xorg allocates and frees a mask pixmap per call
  — rather than a batch entry.

Both are engine-interface changes with blast radius beyond #137, since
`composite_glyphs` is shared with `image_text`. A separate spec.

## Invariants

1. A source that is genuinely one colour renders identically before and
   after, by whichever tier handles it. Tier 1 must not regress the
   overwhelmingly common `CreateSolidFill` path, which stays on the existing
   fast route untouched.
2. No `CompositeGlyphs` is ever discarded silently. If a case remains
   unsupported it must log *and* count; see the visibility note below.
3. The glyph-id width (8/16/32) has no bearing on which source kinds are
   supported. The two are independent and must stay so.
4. Colour is read per composite, never cached across a repaint of the source
   drawable. Any cache is keyed on `content_version` and filled only after
   the ordered readback.
5. "1×1" is always the sampled *domain* at its *offset*, never the backing
   storage extent at its origin. The pixel is read in backing space through
   a server-internal handle, so no second coordinate space is applied.
6. Any cache is keyed on `(DrawableId, content_version, offset)`. Version
   alone collides across drawables and across offsets into one backing.

## Why this hid for so long

The path was already instrumented: it logs at debug and bumps
`composite_glyphs_dropped_unsupported`. Nobody looked, because nothing
surfaces the counter and the debug line sits in `yserver::kms::render::backend`,
which the usual `RUST_LOG` filters exclude. An entire class of client — every
Java/AWT application — rendered no text at all, and the server's own
telemetry knew.

Worth doing alongside the fix, cheaply: promote a *first* occurrence of any
`composite_glyphs` drop to `warn!`, so a silently-unsupported paint path
announces itself once rather than never.

## Proof

**Tier 1, this change.**

- A 1×1 `repeat=Normal` drawable source paints the same pixels as the
  equivalent `CreateSolidFill`. Same for `Pad` and `Reflect`.
- **Negative:** `repeat=None` at 1×1 still drops, and a drawable source
  whose sampled domain is larger than 1×1 **still drops** — tier 1 does not
  make it paint, and a test asserting otherwise would be asserting tier 2.
- **Domain, not storage:** a window picture with a 1×1 content domain inside
  a larger redirected backing is admitted, and samples the pixel at the
  content offset — not the backing's (0,0).
- **Ordering / staleness:** repaint the source pixmap, then draw text, and
  assert the new colour is used. This is the trap in invariant 4, asserted
  rather than assumed.
- **Hardware:** the reporter's Swing probe with `-Dsun.java2d.xrender=true`
  shows all five sample strings, the accented Czech line, and the direct
  `Graphics2D.drawString()` panel. `xrender=false` must remain correct.
- **No regression** in ordinary desktop text: xts `Xlib9` A/B per
  `feedback_xts_ab_gates_pixel_changes`, zero PASS→FAIL.

**Tier 2, when it happens.** A non-1×1 drawable source paints, sampled per
pixel, matching Xorg for both `mask_format == 0` and `!= 0`. Not a gate on
this change.

## Risks

- **Readback cost on the hottest path.** Tier 1 adds a GPU read to every
  glyph draw unless cached. Measure before committing to the naive form.
- **Stale colour** if the read is cached wrongly — see invariant 4. This
  turns a total failure into an intermittent wrong-colour one, which is
  harder to notice and harder to report.
- **Tier 2 is unscoped on purpose.** It needs an atlas-to-maskable-source
  adapter and, for `mask_format != 0`, a temporary A8 target with a real
  lifecycle. Both change the engine's glyph interface, which `image_text`
  shares. Treating it as "route through the existing batch API" would
  understate it — see the tier 2 section.
