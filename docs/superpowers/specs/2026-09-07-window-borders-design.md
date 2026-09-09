# Server-side window borders — design

## Status

Design only. Nothing here is implemented.

Fixes issue #133 (BergmannAtmet, "awesome borders are not drawn"), and folds in
the re-land of `7e1484b4` (reverted by `d08d6933`), which is the same root cause
surfacing as a second symptom.

Reviewed by codex 2026-09-07, **two rounds**, all findings verified against
`../xserver` and applied.

Round 1: six blocking findings. The central correction is recorded in §The five
spaces, which reshaped §Design; it also fixed two misreadings of my own (the
border tile origin, and child clipping against the parent's outer rather than
inner region).

Round 2: three blocking findings — the content clip had no enforcement
*mechanism* (and, worse, P4 as written made P5 impossible), the scene walk's
coordinate recurrence was unspecified, and P8's unchanged-extent case still
misplaced content. Two smaller corrections: `SetWinSize` intersects **both**
bounding and clip shapes, and outer space was described in a way that read as
licensing client draws into the ring. The border-pixmap phasing question is now
**decided** (P5) rather than deferred.

Round 3: no blockers. One design clarification — the redirection exception to
P6's child-clipping rule, now §The redirection exception, which is a *reset* of
the descendant clip rather than an intersect (`mi/mivaltree.c:233`) and
propagates to descendants. Two small improvements: the `bw == 0` invariant is
observable-identical rather than byte-identical, and P8's unchanged-extent case
is in §Verification rather than prose only. Codex considers the spec ready to
turn into an implementation plan.

## Goal

Paint server-side window borders, place a window's content at
`outer_origin + border_width`, and make the border participate in child
clipping and input hit-testing.

Scope is core protocol + resources + the KMS backend's window geometry, storage
sizing, scene placement and input hit-testing. Out of scope: the `maim`
root-capture bug also mentioned in #133 (separate issue).

`CWBackPixmap` semantics were out of scope in the first draft. They are now
partly in scope: the border tile origin depends on the background state (P5),
and codex correctly noted that excluding it conflicts with complete
`CWBorderPixmap` support.

## Evidence

Measured 2026-09-07 on silence (RX 6800, awesome 4.x, `border_width = 16`,
`border_normal = "#ff0000"`, `border_focus = "#00ff00"`, two tiled xterms).
Artefacts are gitignored and not committed; numbers quoted.

**Awesome sends the border colour, identically to both servers.** Tallied over
`awesome-xorg.xtrace` (5596 lines) and `awesome.xtrace` (5116 lines):

| | Xorg | yserver |
|---|---|---|
| `border-pixel=0x00000000` | 11 | 11 |
| `border-pixel=0xffff0000` (red) | 4 | 4 |
| `border-pixel=0xff00ff00` (green) | 3 | 3 |
| `border-pixmap=…` | 0 | 0 |
| `border-width=16` occurrences | 27 | 27 |
| errors on any border request | none | none |

Same seven `ChangeWindowAttributes` requests, same shapes, same order; only the
XID base differs. Each is a 16-byte request with **only** the `CWBorderPixel`
bit set. So awesome takes no different path on yserver, and yserver rejects
nothing — it accepts all seven and does nothing with them.

Both servers also see awesome compute the same client root origin:

```
yserver: SendEvent → ConfigureNotify window=0x0030000c x=16 y=50 ... border-width=16
Xorg:    SendEvent → ConfigureNotify window=0x0080000c x=16 y=50 ... border-width=16
```

frame outer `(0,17)` + border `16` → `(16,33)` + 17 px titlebar → `(16,50)`.
yserver's protocol layer is therefore correct: it echoes `border-width=16` in
ConfigureNotify identically and awesome derives the right answer from it.

**But nothing is painted, and the content is 16 px off.** From
`yserver-drawable-0-windows.txt`, the two frames are `(0,17 1244x1389)` and
`(1276,17 1244x1389)`; `1276 − 1244 = 32 = 2 × 16`, so awesome allowed exactly
two border widths between the tiles. Sampling `yserver-scanout-0-out0.ppm`
(2560×1440) at `y=700`: `x 0..1243` white (xterm content), `x 1244..1275` greys
92–145 (**the wallpaper**, 32 px of it), `x 1276..` white again. Column `x=600`:
`y 17..33` = `(34,34,34)` (awesome's 17 px titlebar), `y 34..1406` white,
`y 1408+` = `(79,79,79)` wallpaper.

Two conclusions, each independently decisive:

1. Scanning all 3 686 400 pixels of the output: **0 pixels of exact `#ff0000`
   and 0 of exact `#00ff00`.** Roughly 90 k should be one or the other. The ring
   is not painted in the wrong place or the wrong colour — it is not painted.
2. The titlebar renders at `y=17` and content at `x=0`, where the server itself
   told the client `(16,50)`. The `+bw` term is missing exactly once. yserver's
   render path and its protocol layer disagree with each other by one border
   width.

Note the `border-pixmap` row: **zero uses on either server.** Awesome exercises
only the pixel path. This is the measurement behind the phasing option in P5,
and it also means the awesome smoke test cannot validate tiled borders.

## Current behaviour on master (`a1e33aa8`)

1. **The colour is never parsed.** `create_window_request`
   (`crates/yserver-protocol/src/x11/mod.rs:912`) and
   `change_window_attributes_request` (:941) both read value index 0, 1, then
   jump to 4. Index 2 is `CWBorderPixmap`, index 3 is `CWBorderPixel`. Neither
   request struct has a field for them. `values.value(N)` is bit-indexed, so the
   later attributes are *not* misparsed — the two are simply dropped, and a
   request carrying only `CWBorderPixel` parses to a struct in which every
   optional field is `None`.
2. **Nowhere to store it.** `grep -rn border_pixel --include=*.rs` returns
   nothing. `Window` (`crates/yserver-core/src/resources.rs:2875`) has
   `border_width` and a vestigial `border_pixmap_host_xid` (:2892) only ever
   assigned `None`.
3. **The backend drops the width.** `WindowGeometry`
   (`crates/yserver/src/kms/render/backend.rs:90`) has no border field.
   `create_subwindow` takes it as `_border_width` (:17204); `configure_subwindow`
   (:17325) never reads `config.border_width`, although the field exists
   (`crates/yserver-core/src/host_x11/pump.rs:313`).
4. **Storage is sized and placed without the ring.**
   `sync_window_leaf_storage_to_geometry` (backend.rs:3301) sizes storage
   `geom.width × geom.height`; the scene walk places it at `geom.x, geom.y` with
   content at storage `(0,0)` — hence the 16 px shift.
5. **`window_absolute_position` omits `+bw`** (resources.rs:~2707), reverted by
   `d08d6933`.
6. **Input ignores borders entirely.** `hit_test_child`
   (`crates/yserver-core/src/server.rs:2409`) rejects `child_x < 0 ||
   child_y < 0` and tests only `child.width`/`child.height`, so no point on a
   left or top border can hit, and right/bottom border points fall outside the
   extent. A second implementation, `child_containing_point`
   (resources.rs:2763), ignores borders the same way.
7. **Storage bounds are the only draw clip.** `resolve_paint_target` returns a
   `PaintTarget` with an `offset` and no extent (backend.rs:510), and
   destination ops such as `put_image` (backend.rs:19632) add the offset and
   submit against the storage extent.

`border_width` *is* otherwise tracked and correct: `window_bounding_box`
(resources.rs:2857) uses it for stacking-overlap tests, `configure_window`
(:837) stores it, and ConfigureNotify reports it faithfully (27 = 27 above).

## Reference: how Xorg does it

Read from `../xserver`, not from spec prose. Citations verified 2026-09-07.

**Representation** (`include/windowstr.h:146`): `PixUnion border` plus
`borderIsPixel:1` — a discriminated union, parallel to `background` /
`backgroundState`. A border is *either* a pixel *or* a pixmap, never both.

**The ring lives inside the window's own storage**
(`composite/compalloc.c:610`, `compAllocPixmap`):

```c
int bw = (int) pWin->borderWidth;
int x = pWin->drawable.x - bw;
int y = pWin->drawable.y - bw;
int w = pWin->drawable.width  + (bw << 1);
int h = pWin->drawable.height + (bw << 1);
PixmapPtr pPixmap = compNewPixmap(pWin, x, y, w, h);
...
compSetPixmap(pWin, pPixmap, bw);
```

Bordered size, outer origin, and `compSetPixmap` records `bw` as the content
offset within it. `compSetParentPixmap` (:672) passes `pWin->borderWidth` the
same way when a window reverts to drawing into its parent's pixmap.

**Two regions, not one** (`mi/mivaltree.c:384`), the review's central point:

```c
/*
 * To get the right clipList for the parent, and to make doubly sure
 * that no child overlaps the parent's border, we remove the parent's
 * border from the universe before proceeding.
 */
RegionIntersect(universe, universe, &pParent->winSize);
```

`borderClip` (outer, border-inclusive) is what the window itself draws with and
occludes siblings by; `winSize` (inner) is what clips descendants. Children
cannot overlap their parent's border.

**Shaped variants** (`dix/window.c:1735` `SetWinSize`, :1747 `SetBorderSize`).
`SetWinSize` intersects with **both** shapes, not just the clip shape — the
first draft of this spec said `wClipShape` only, which was wrong:

```c
if (wBoundingShape(pWin) || wClipShape(pWin)) {
    RegionTranslate(&pWin->winSize, -pWin->drawable.x, -pWin->drawable.y);
    if (wBoundingShape(pWin))
        RegionIntersect(&pWin->winSize, &pWin->winSize, wBoundingShape(pWin));
    if (wClipShape(pWin))
        RegionIntersect(&pWin->winSize, &pWin->winSize, wClipShape(pWin));
    RegionTranslate(&pWin->winSize, pWin->drawable.x, pWin->drawable.y);
}
```

Note the translation to **window-local** coordinates before intersecting and
back after: shapes are window-relative, `winSize` is absolute. `SetBorderSize`
expands by `bw` and, under COMPOSITE, gives a redirected window a clip equal to
its own geometry rather than one clipped to its parent. Bounding, clip and input
shapes have three different effects and must not be collapsed into one "∩
shape".

**Painting** (`mi/miexpose.c:449`, `miPaintWindow` with `PW_BORDER`): paints
into the window's own pixmap (`pixmap = GetWindowPixmap(drawable)`),
`solid = pWin->borderIsPixel`. Solid uses `FillSolid` with `fill.pixel`; tiled
uses `FillTiled`. **The tile origin is not unconditionally the window's inner
origin** — `miexpose.c:458` walks up while the background is ParentRelative and
uses *that ancestor's* drawable origin:

```c
while (pWin->backgroundState == ParentRelative)
    pWin = pWin->parent;
tile_x_off = pWin->drawable.x;
tile_y_off = pWin->drawable.y;
```

The first draft of this spec stated the inner origin unconditionally. That was
a misreading.

**Depth-32 alpha rule** (`mi/miexpose.c`, under `#ifdef COMPOSITE`): for a
depth-32 drawable, walk up the parent chain; if the effective depth is 24,
`fill.pixel |= 0xff000000`. "Make sure alpha will sample as 1.0 for opaque
windows."

**Input tests the outer region but reports inner coordinates**
(`dix/window.c:2986`, `PointInWindowIsVisible`):

```c
if (RegionContainsPoint(&pWin->borderClip, x, y, &box)
    && (!wInputShape(pWin) ||
        RegionContainsPoint(wInputShape(pWin),
                            x - pWin->drawable.x,
                            y - pWin->drawable.y, &box)))
```

`borderClip` is border-inclusive; the input shape and the reported coordinates
are relative to `drawable.x/y`, the **inner** origin. **A pointer event on a left
or top border therefore has negative window-relative coordinates.**

**A border-width change preserves content** (`composite/compalloc.c:676`,
`compReallocPixmap`):

```c
/*
 * Make sure the pixmap is the right size and offset.  Allocate a new
 * pixmap to change size, adjust origin to change offset, leaving the
 * old pixmap in cw->pOldPixmap so bits can be recovered
 */
```

Two things follow: the old pixmap is retained in `cw->pOldPixmap` so valid
pixels can be recovered, and reallocation happens **only if `w + 2bw` or
`h + 2bw` actually changed**.

The `else` branch (`compalloc.c:706`) is worth reading closely, because it is
narrower than "just adjusts the origin" suggests:

```c
else {
    pNew = pOld;
    cw->pOldPixmap = 0;
}
pNew->screen_x = pix_x;
pNew->screen_y = pix_y;
```

It reuses the storage, clears `pOldPixmap`, updates only the screen origin —
and notably does **not** call `compSetPixmap`, so it does not itself relocate
any bytes within the storage. Whatever moves content when the *content offset*
changes lives in the surrounding mark/validate/`compCopyWindow` machinery, not
here. I have not traced that full path, so this spec does not claim what Xorg
does about it; P8 states the requirement independently and locates the
relocation deliberately.

**`CWBorderPixmap` validation** (`dix/window.c:1251`): value `CopyFromParent`
(0) → BadMatch if there is no parent or `depth != parent depth`; then if the
parent `borderIsPixel`, copy the parent's *pixel* and switch representation,
else adopt the parent's border pixmap xid. Otherwise look the pixmap up:
BadMatch if `pPixmap->drawable.depth != pWin->drawable.depth` or a different
screen; BadPixmap if it does not exist. Refcount the new pixmap; destroy the old
one if the old representation was a pixmap.

**`CWBorderPixel` validation** (`dix/window.c:1292`): sets `border.pixel`,
`borderIsPixel = TRUE`, and — quoting — *"border pixel overrides border pixmap,
so don't let the ddx layer see both bits"*: `vmaskCopy &= ~CWBorderPixmap`. With
both bits set the pixel wins, which also falls out of mask order.

**CreateWindow inherits the border** (`dix/window.c:879`):

```c
pWin->borderIsPixel = pParent->borderIsPixel;
pWin->border = pParent->border;
if (pWin->borderIsPixel == FALSE)
    pWin->border.pixmap->refcnt++;
```

Not "pixel 0" — CopyFromParent.

**CreateWindow requires a border when the depth differs**
(`dix/window.c:818`): neither `CWBorderPixmap` nor `CWBorderPixel` supplied,
class not InputOnly, `depth != pParent->drawable.depth` → BadMatch. This is why
awesome passes `border-pixel=0x00000000` on every depth-32 `CreateWindow`, and
accounts for the 11 zeros above.

## The five spaces

The first draft treated a border as "larger storage plus an offset". Codex's
review showed that is the error underneath four of its six blocking findings: a
border creates five distinct spaces, and conflating any two of them is a bug.

| space | extent | used for | Xorg name |
|---|---|---|---|
| **outer** | `(x, y, w+2bw, h+2bw)` ∩ bounding-shape-expanded | **server** border painting; scene-node sampling; sibling occlusion; stacking overlap | `borderSize` / `borderClip` |
| **content** | `(x+bw, y+bw, w, h)` | GetGeometry `w`/`h`; the logical drawable; **the only space client drawing may touch** | `winSize` |
| **child clip** | content ∩ bounding ∩ clip shape | clipping descendants — children may **not** overlap the parent's border | `winSize` |
| **input** | outer, ∩ input shape (applied in content coords) | hit-testing; coordinates reported relative to the **content** origin, hence possibly negative | `borderClip` + `wInputShape` |
| **backing** | storage-local | draw translation, per level of the redirect chain | `compSetPixmap` offset |

Client drawing belongs exclusively to content space. Outer space is reached only
by server-internal painting (P5) and by the scene walk sampling the node —
never by a client request. An earlier draft of this table said outer space was
for "the window's own draw", which read as licensing client draws into the ring
and contradicted the content-clip invariant.

Every piece below names which space it operates in.

## Design

**Storage-inclusive borders**, mirroring `compAllocPixmap`: the ring lives
inside the window's own storage, placed at the outer origin.

### Why not a solid-colour draw op

`CompositeDraw` (`crates/yserver/src/kms/vk/compositor.rs:62`) is
texture-sample-only — `image_view`, `src_origin`, `src_size`,
`alpha_passthrough`. `CompositeScene` carries a single `bg_color` clear for the
whole scanout. A ring as its own scene node would need a third pipeline
alongside the opaque/passthrough pair.

Storage-inclusive needs none: `emit_node`
(`crates/yserver/src/kms/render/scene.rs:5772`) and `piece_draw` (:5714) keep
sampling one texture per node, unchanged. It matches Xorg, so the coordinate
arithmetic has a reference implementation rather than a reinvention.

Codex accepted this reasoning, and the alternative of painting the ring into the
*parent* is additionally ruled out by `mivaltree.c:384`: the border belongs to
the window's own clip, not the parent's, and painting it into the parent would
invert the occlusion relationship with siblings.

The catch, and the reason the piece list grew: storage bounds currently *are*
the draw clip (§Current behaviour 7). Enlarging storage without adding an
explicit content clip lets client drawing scribble on the border.

### Pieces

**P1 — protocol (wire).** Parse value indices 2 and 3 in both parsers. Add
`border_pixmap: Option<ResourceId>` and `border_pixel: Option<u32>` to
`CreateWindowRequest` and `ChangeWindowAttributesRequest`. Keep `value_mask`
available so the handler can apply pixel-overrides-pixmap.

**P2 — resources (state + validation).** Replace the vestigial
`border_pixmap_host_xid` with a discriminated union mirroring `PixUnion` +
`borderIsPixel`:

```rust
enum BorderSource {
    Pixel(u32),
    Pixmap { id: ResourceId, host_xid: Option<PixmapHandle> },
}
```

`CreateWindow` default is the parent's `BorderSource` (`dix/window.c:879`), not
`Pixel(0)`. Implement `CopyFromParent` resolution, the depth/screen BadMatch,
the BadPixmap path, pixel-overrides-pixmap, and pixmap refcount/destroy. Retain
the host xid at attrs-change time for the same reason
`background_pixmap_host_xid` is retained.

**The `dix/window.c:818` BadMatch moves in scope.** The first draft deferred it.
Codex is right that it cannot be deferred alongside P2's inheritance: without
it, a depth-32 child under a depth-24 parent inherits a border pixmap whose
depth is invalid for the child, and P2 has no legal state to fall back to.

**P3 — core→backend plumbing.** The first draft said only "`WindowGeometry`
gains the resolved border source", which had no delivery mechanism:
`create_subwindow` (`crates/yserver-core/src/backend/trait_def.rs:1013`) carries
only `border_width` and the background attributes, and
`handle_change_window_attributes` forwards to the backend only when a background
changes (`process_request.rs:20660`). Cover explicitly:

- creation-time border-source propagation (extend the `create_subwindow` args or
  fold into a struct);
- CWA border-source propagation (a new forward path, not the background one);
- the KMS storage repaint it triggers;
- host-X11 / ynest forwarding (`nested.rs` already reads `w.border_width` at
  :779);
- the recording and test backends (`backend/recording.rs:1143`);
- border-pixmap orphan / reference tracking, parallel to the existing background
  helpers.

`create_subwindow` stops discarding `_border_width`; `configure_subwindow`
honours `config.border_width`.

**P4 — backing space: storage sizing, translation, and the content clip.**
Storage becomes `(w + 2bw) × (h + 2bw)`.

**The choke point must be an API, not a data field.** An earlier draft said
`PaintTarget` gains a content extent and enforcement happens "at `PaintTarget`
resolution". That does not work: resolution returns metadata and cannot clip
anything. Rectangles are clipped later, and CPU read-modify-write paths, source
reads, RENDER operations and direct engine calls each reach the storage by their
own route. A field every call site is expected to consult is exactly the
enforcement model that fails silently the next time a drawing op is added.

Replace it with an **opaque drawable-target handle** that is the only way to
reach window storage, exposing:

- **destination clipping** in drawable-local (content) coordinates;
- **source clipping / read bounds** for the reads that are content-only.
  ⚠ **`GetImage` is NOT one of them — this was wrong in the first draft.** X11
  `GetImage` on a window is deliberately **border-inclusive**: Xorg validates
  `x >= -wBorderWidth(pWin)` through
  `x + width <= wBorderWidth(pWin) + pDraw->width`
  (`dix/dispatch.c:2179-2183`) and reads the bounding drawable. Worse, the reply
  length is computed up front from the *requested* rectangle
  (`PixmapBytePad(width, depth) * height`, `:2227`), so clamping the read
  **shortens the reply** while its header still advertises the full size — a
  protocol violation that segfaults clients inside Xlib. Whatever the read
  bounds are, a reply must always describe exactly the rectangle asked for;
- **translation** into backing coordinates;
- **separate, privileged backing-space operations**, used only by the server.

That last item is load-bearing and its absence was a contradiction in the
earlier draft: once P4 correctly confines the client path to content space, P5
**cannot** paint the ring through that path. The ring fill must be a privileged
backing-space operation — see P5.

Give an explicit coordinate formula per case rather than the flat `(bw, bw)` an
earlier draft implied, which is only the unredirected-leaf case.
`resolve_paint_target` (backend.rs:4855) accumulates child `x`/`y` up to the
redirected ancestor; it must also accumulate content offsets. For a child C
painting into redirected ancestor W the one-level horizontal translation is
`W.border_width + C.x + C.border_width`, and a redirected C still has its
content starting at `C.border_width`. Spell out: leaf storage, ancestor backing,
root redirection, nested descendants.

**P5 — the ring fill (privileged backing space).** Fill the ring on creation,
border source change, `border_width` change, and geometry change.

This is a **server-internal backing-space fill**, not `fill_rectangle` and not
any client-facing drawing path — those are confined to content space by P4. It
receives exactly `outer − content` (the ring, four rects or a region) in backing
coordinates and bypasses the content clip by construction.

The primitive to model it on already exists: `clear_window_area_with_background`
(`crates/yserver/src/kms/render/backend.rs:3407`) takes
`(background_pixel, background_pixmap_host_xid, x, y, w, h, tile_origin)` and
already handles the pixel-or-pixmap choice with tile alignment, resolving
through `resolve_paint_target`. The ring fill should be its sibling — same
shape, border source instead of background source, ring rects instead of a
cleared area — which also means P4's target API must expose the privileged
backing view that both of them need.

Tile origin comes from the **ParentRelative walk** (`miexpose.c:458`), not the
inner origin. Apply the depth-32 alpha rule.

**Phasing: pixmap borders are IN phase 1.** An earlier draft left this to be
decided during implementation, which codex correctly objected leaves the plan's
API, tests and completion criteria indeterminate. Deciding it now: the tiled
primitive exists (above), already carries a `tile_origin`, and
`resources.rs:1771` already documents the parent-origin tile alignment rule for
the ParentRelative background case. The marginal cost over pixel-only is
therefore small, and it avoids shipping a validated-but-unpainted attribute
(`feedback_no_protocol_stubs`). The zero-`border-pixmap` measurement is still
relevant, but only to §Verification: the awesome smoke cannot exercise this
path, so tiled borders need a purpose-written client.

**P6 — outer, content and child-clip spaces in the scene walk.** The first
draft said "clip children to the parent's outer rect". **That is backwards** and
is corrected here: children clip to the parent's *inner* (`winSize`) region per
`mivaltree.c:384`. The walk needs both regions separately — outer for the node's
own draw and sibling occlusion, inner for descendant clipping — plus shaped
variants of each, with the `SetBorderSize` rule: intersect the expanded border
with the bounding shape, then union the inner `winSize`.

**The scene walk needs its own explicit coordinate recurrence.** P4's formula
covers paint targets; it does not tell the walk where nodes go, and the KMS
scene carries its own geometry tree, so this must be stated independently of
`window_absolute_position` (P7). Today the walk keeps a single absolute per
node (`scene.rs:6141`):

```rust
let abs_x = parent_abs_x + i32::from(geom.x);
let abs_y = parent_abs_y + i32::from(geom.y);
```

then clips against `own_w`/`own_h` = `geom.width`/`geom.height`. With borders it
needs two absolutes per node:

```
child_outer_abs   = parent_content_abs + child.x/y
child_content_abs = child_outer_abs + child.border_width
```

The **outer** absolute is where the node samples and how it occludes siblings;
the **content** absolute is what descends as the child-clip origin. Without
this, a nested bordered window stays displaced by every ancestor's border even
with P4 and P7 both correct — the walk would keep adding `x` to a content
absolute while treating the result as an outer one.

The current walk uses one `width`/`height` rectangle for both the node and the
descendant clip (`scene.rs:6141`); simply widening it would let children cover
their parent's border. This also interacts with
`project_parent_bounding_shape_not_clipping_children`, which records that the
walk clips children by the parent's rect only — that latent defect is now on the
critical path rather than adjacent to it.

#### The redirection exception

"Children clip to the parent's inner region" is **not** universal, and P6 must
not apply the normal rule silently to redirected windows. Xorg's rule is a
*reset*, not an intersect (`mi/mivaltree.c:233`):

```c
/*
 * In redirected drawing case, reset universe to borderSize
 */
if (pParent->redirectDraw != RedirectDrawNone) {
    ...
    RegionCopy(universe, &pParent->borderSize);
}
```

and `TreatAsTransparent(w)` is `w->redirectDraw == RedirectDrawManual`
(`mivaltree.c:171`) — manual redirect additionally does not subtract from
siblings' universe. Three rules, stated separately because they are not the same
thing in yserver's model:

1. **Client / backing clipping — both redirect modes.** A redirected window
   draws into backing sized to its **own** bordered geometry and is clipped to
   its **own** content extent only, never to its parent. Xorg keys this off
   `redirectDraw != RedirectDrawNone` with no manual/automatic distinction
   (`SetWinSize` :1720, `SetBorderSize` :1747, both quoted in §Reference). So
   P4's target API derives the content clip from the window itself, with no
   ancestor term, whenever the window owns its own backing.

2. **Scene visibility clipping when yserver samples that backing.** The
   accumulated ancestor clip must be **reset** to the window's own outer region
   for a window owning its own `redirected_target`, not intersected with what
   descended. Today the walk only ever narrows: `clip_x0 = clip_x0.max(abs_x)`
   (`scene.rs:6157`). That is correct for unredirected windows and wrong for
   redirected ones. yserver already distinguishes the modes —
   `is_manual_redirected = has_own_redirected_target && !d.scene_participating`
   (`scene.rs:6264`), with manual-redirect windows skipped from the scene and
   their descendants pruned, automatic ones sampled through
   `redirected_target` — so this rule attaches to the existing
   `has_own_redirected_target` branch rather than needing new mode detection.
   Manual redirect's transparency for sibling occlusion is the separate
   `TreatAsTransparent` behaviour and is already what "skipped from the scene"
   achieves; state it explicitly so it is not re-derived.

3. **Descendant inheritance: yes, descendants inherit the un-parent-clipped
   region.** Xorg resets the universe to `borderSize` *before* the
   `RegionIntersect(universe, universe, &pParent->winSize)` at :390, so a
   descendant of a redirected window clips to that window's own content region
   regardless of what its parent would have permitted. This is the point of
   redirection — the window owns a full-size pixmap — and it means the reset in
   rule 2 propagates down, it does not apply only to the redirected node itself.

**P7 — origin.** Re-land `7e1484b4` in `window_absolute_position`:
`ax += x + bw`, `ay += y + bw`.

**P8 — border-width change: preserve content, damage both rings.** The first
draft said "treat a border-width change as `size_changed`", which is wrong:
`sync_window_leaf_storage_to_geometry` (backend.rs:3301) detaches the old
backing, allocates a new one and initialises it from the background, **erasing
an otherwise unchanged client drawable**. Mirror `compReallocPixmap`:

- reallocate **only** if `w + 2bw` or `h + 2bw` actually changed;
- **whenever the content offset changes, relocate the content — reallocation or
  not.** An earlier draft said the unchanged-extent case may "just move the
  content origin". That loses and misplaces pixels. Worked counter-example, one
  ConfigureWindow:

  ```
  old: w=100, bw=2  → outer = 104
  new: w= 98, bw=3  → outer = 104
  ```

  No reallocation is needed, yet content must move from storage offset 2 to
  offset 3. Reinterpreting the existing bytes makes the new content sample old
  border pixels along one edge and leaves stale content inside the new ring.
  Note Xorg's no-realloc branch does **not** do this relocation itself
  (§Reference), so it must be located deliberately here rather than assumed to
  fall out of the port.
- the relocation must be **overlap-safe** (the source and destination regions
  overlap by construction) — either an in-place overlap-aware copy or a
  temporary/retained old backing;
- when reallocating, retain the old storage and copy the intersection of old and
  new content from `old_bw` to `new_bw` before painting the newly exposed ring;
- **distinguish this from a pure `x`/`y` move**, which changes the screen origin
  only and must relocate nothing storage-local;
- damage `old outer ∪ new outer`, not merely the shrinking case.

**P9 — input space (new).** P7 alone does not fix input; §Current behaviour 6 is
a separate defect. Cover:

- border-inclusive hit regions (outer space);
- event coordinates relative to the **content** origin, which means **negative
  values on the left and top borders** must be representable and must not be
  rejected;
- input shapes applied in content coordinates (`window.c:2986`);
- all four sides and the four corners;
- **both** hit-test implementations — `hit_test_child` (server.rs:2409) and
  `child_containing_point` (resources.rs:2763) — since divergence between the
  two authoritative trees is a known bug class here.

### Invariants

- **`bw == 0` must be observably and performance-identical to today.** MATE,
  e16, Cinnamon and every WM in the current smoke set use `border_width = 0` —
  exactly why #133 went unnoticed and why `7e1484b4` looked inert. Every
  storage-size, placement, clip, translation and hit-test change must collapse
  to the current arithmetic at `bw == 0`. Stated as *observable* rather than
  byte-identical deliberately: new internal metadata (a second absolute per
  scene node, a content extent on the target handle) legitimately changes
  in-memory state while pixels, event coordinates, draw counts and painted
  fractions must not move.
- **A border is either a pixel or a pixmap, never both** (`borderIsPixel`).
- **Client drawing can never touch border pixels.** Reads are a different
  matter and must follow X11 per request: `GetImage` on a window is
  border-inclusive by design (see P4), so "reads never return ring pixels" is
  NOT an invariant — it was an error in the first draft that reached the
  implementation and one of its tests before Xorg settled it.
  The content clip in P4 is a correctness gate, not an optimisation.
- **A child can never overlap its parent's border** (`mivaltree.c:386`).
- **Input coordinates are content-relative and may be negative.**
- **InputOnly windows have no border.** `CWBorderPixel`/`CWBorderPixmap` are
  already outside `INPUTONLY_LEGAL_MASK` and BadMatch'd
  (`process_request.rs:20530`); keep `border_width` 0 for them.
- **The root window has no border.**
- **GetGeometry reports content `w`/`h` with `border_width` separately** — never
  the bordered extent.
- **Depth-32 border pixels sample α = 1 when the effective depth is 24.** Check
  which pipeline the ring lands on: the opaque shader already forces α = 1 on
  some paths (see
  `project_xfce_menu_black_borders_after_disabling_compositor`), so this may be
  satisfied on one path and not another.

### Risks

- **Storage sizing touches every window.** The `bw == 0` invariant is the
  mitigation and needs a measurement, not an argument.
- **The content clip is a new cross-cutting constraint.** It must be enforced by
  the drawable-target API of P4 — the only route to window storage — not by a
  field that call sites are trusted to consult, and not per-operation. The
  failure mode to design against is the next drawing op added silently
  bypassing it. The privileged backing path (P5) is the deliberate exception and
  should be narrow enough to audit by grep.
- **Re-landing `7e1484b4` re-introduces whatever caused `d08d6933`.** The revert
  message is empty and the reason unknown. Oracle: xts5
  `XI/GrabDeviceButton-4`, which measured x_root 103 against an expected 104.
  Diff against the eiger baseline; never eyeball pass counts
  (`feedback_xts_vacuous_passes`, `reference_xorg_not_100pct_on_xts_xi`).
- **Direct scanout assumes content at the storage origin.** Phase 1 **rejects
  direct scanout for `bw > 0`** — a hard rule, not "if the audit is unclear",
  per codex. It costs nothing on real desktops (`bw == 0` everywhere in the
  current smoke set). Lifting it later requires proving the bordered storage and
  source crop valid; see `project_video_high_gpu_no_direct_scanout` and
  `project_scanout_m2_cowdescendant_unflip_fatal`.
- **Border pixmap lifetime.** Retained independent of client refs; a wrong
  refcount leaks or use-after-frees. Mirror the background helpers.
- **Two hit-test implementations and two geometry trees.** Fixing one and not
  the other is the most likely partial failure.

### Verification

- **The bug's own measurement, re-run:** awesome HW smoke at `bw = 16` with
  red/green borders, then pixel-scan the scanout for exact `#ff0000` /
  `#00ff00` counts. Currently 0 and 0; expect ~90 k split between them, the
  32 px inter-tile gap filled, the titlebar at `y = 33` and content at `x = 16`.
- **Targeted tests**, per codex: draws clipped to the inner content rect;
  `GetImage` on a window returning border pixels rather than clamping —
  **corrected 2026-09-08**, this bullet previously said "reads never returning
  border pixels", which contradicts the invariant above and was the first
  draft's error; nested bordered children under a
  redirected ancestor; a child unable to overlap its parent's border; border
  hit-testing including negative event coordinates on all four sides and the
  corners; a `border_width` change preserving client content — **including the
  unchanged-outer-extent case `w=100,bw=2 → w=98,bw=3` (outer stays 104, content
  must still migrate from offset 2 to 3)**, which is the case prose alone would
  let an implementer skip; a redirected window's clip reset and its descendants
  inheriting the un-parent-clipped region, per §The redirection exception, in
  both redirect modes; bounding and input
  shapes combined with borders; `CopyFromParent`; both mask bits in one request;
  invalid pixmap xid and depth mismatch; pixmap lifetime; direct-scanout
  rejection at `bw > 0`; and **`bw == 0` across every one of
  these paths**. (`host-X11 parity` was on this list; **dropped 2026-09-08** —
  jos: "ynest is not a supported use-case ATM", and the `ynest` binary is gone,
  so the nested path is neither shipped nor runnable.)
- **`bw == 0` regression:** MATE and e16 idle A/B, painted fractions and walk
  counts identical to master.
- **xts5:** diff against `docs/superpowers/findings/2026-06-18-xorg-xts-baseline.tsv`
  with `tools/xts-vs-baseline.py`. Runs are not deterministic; compare against
  our own previous run in `docs/test-status.md`, never against Xorg.
- **Xorg comparison:** already captured; awesome's request stream is identical
  on both servers, so any remaining divergence after the fix is ours.

Note the awesome smoke cannot validate tiled borders (zero `border-pixmap` uses)
— that path needs a purpose-written client.

No commit before a HW smoke (`feedback_no_commit_before_smoke`).

## Adjacent gaps found while reading, not in scope

- `CreateWindowRequest` has no `cursor` field although `CWCursor` (index 14) is
  legal on CreateWindow — `create_window_request` stops at index 13.
