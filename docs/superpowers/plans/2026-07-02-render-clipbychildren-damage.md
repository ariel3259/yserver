# RENDER ClipByChildren parity (Composite / CompositeGlyphs / Trapezoids / Triangles) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the four RENDER paint ops (Composite, CompositeGlyphs, Trapezoids, Triangles/TriStrip/TriFan) full X.Org `ClipByChildren` parity on both paint and damage: clip each op — and the damage it generates — to the destination window's `clipList` (geometry ∩ picture clip, minus mapped non-redirected children), gated on the **destination picture's** `subWindowMode`.

**Architecture:** A single clipList region is computed once, backend-side, in destination-window-local coordinates. It drives both the paint scissors (after shifting into backing space) and the damage the core accumulates (returned un-shifted). Paint and damage use the same region by construction, so they can't disagree; the mode source is the picture record (X.Org-correct), and damage becomes precise to what was painted.

**Tech Stack:** Rust; `yserver` (KMS/Vulkan backend) + `yserver-core` (protocol core loop) + `yserver-protocol` crates; existing region helpers (`intersect_rect_with_clip`, `subtract_one_rect_clip`, `compute_copy_area_dst_rects`, `compute_render_composite_clip`).

---

## Deviations from the spec (`docs/superpowers/specs/2026-07-02-render-clipbychildren-damage-design.md`)

Two forced corrections were found while grounding the spec in the code. Both are baked into the tasks below.

1. **Return type is `Vec<xfixes::RegionRect>`, not `Vec<Rectangle16>`.** `Rectangle16` is defined in the **`yserver`** crate (`crates/yserver/src/kms/cpu_types.rs`). The backend trait lives in **`yserver-core`**, which depends only on `yserver-protocol` — it *cannot* name `Rectangle16` without a circular dependency. `yserver_protocol::x11::xfixes::RegionRect` has identical fields (`x: i16, y: i16, width: u16, height: u16`), is already the trait's rectangle currency (`mapped_child_clip_rects`, `set_shape_rectangles`, etc.), and is what the plan returns from the trait. The private backend helper still computes `Vec<Rectangle16>` (matching all surrounding backend code); each op converts to `Vec<RegionRect>` at its `Ok(...)`.

2. **`subWindowMode` lives on the *picture* record, keyed by the picture xid** (`PictureRecord::Drawable { subwindow_mode: u8, .. }`, `crates/yserver/src/kms/core.rs:1391`). The spec's helper signature only receives the *window* xid, which cannot look up the picture mode. So the mode is resolved in each op body (which holds `host_dst`, the picture xid) via a small free function `dst_picture_clip_by_children(core, host_dst) -> bool`, and passed to the clipList helper as a `clip_by_children: bool` parameter.

Value convention: `subwindow_mode` is stored as the raw X wire byte — `0 = ClipByChildren` (default), `1 = IncludeInferiors` (`RenderChangePicture` `CPSubwindowMode` handler at `backend.rs:8846`).

**Damage excludes the src/mask picture-clip fold.** The returned region is `picture_clip ∩ dst_extent − children ∩ op_bbox` — it does *not* fold in source/mask `clientClip`. That fold refines the *paint* scissors only (Composite path). Damage may therefore be marginally larger than the painted pixels when a src/mask clip is set — safe over-damage, and consistent with the spec's helper signature (which takes only the dst picture clip). *(Codex review MINOR-3: acknowledged; kept as accepted over-damage. Tightening it — folding src/mask into the returned region in local coords — is a recorded follow-up, not part of this plan.)*

**Empty picture clip (`Some([])`) means paint NOTHING, not "no clip"** *(codex review MAJOR-1 fix)*. Only a `None` picture clip means "no clip → base is the full extent". An empty rect list is an empty clip region: X RENDER clients (marco-with-compositing) use `SetPictureClipRectangles` with zero rects as a "stop painting until I set a real clip" gate — the load-bearing comment at `backend.rs:16409-16435` stores it as `Some(vec![])` and this was a previously-fixed "shadow only" regression. The helper routes every `Some(..)` through `intersect_rect_with_clip` (which returns empty for an empty list), so `Some([])` correctly yields an empty region. Guarded by `render_dst_cliplist_empty_picture_clip_paints_nothing` (Task 1).

**CompositeGlyphs damage is a single bounding box, matching X.Org** *(codex review MAJOR-2, resolved as no-change with evidence)*. Codex suggested a per-glyph multi-rect union. X.Org's `GlyphExtents` (`render/glyph.c:499`) computes a single min/max box over all resolved glyphs and composites/damages within it, so the single-box approach *is* the X.Org behaviour (`feedback_xorg_is_the_de_facto_spec`). Per-glyph rects would diverge from X.Org and risk a scissor/damage-rect explosion for long text. The plan's box is over rendered glyphs only (`parsed[]`), so it is the rendered extent — not the request envelope — which satisfies the spec's intent. See Task 6.

---

## File Structure

- `crates/yserver/src/kms/v2/backend.rs` — the real work: two new free/impl helpers (`render_dst_cliplist_local`, `dst_picture_clip_by_children`, `dst_local_extent`, `local_rects_to_region`) + the four op bodies rewired to compute-clip-paint-return + backend unit tests.
- `crates/yserver-core/src/backend/trait_def.rs` — four return-type changes.
- `crates/yserver-core/src/backend/recording.rs` — configurable `render_return_region` field + four render methods return it (incl. an explicit `render_triangles_op` override).
- `crates/yserver-core/src/host_x11/trait_impl.rs` — four methods return `Ok(Vec::new())`.
- `crates/yserver-core/src/core_loop/process_request.rs` — three arms damage the returned region + per-arm core plumbing tests.
- `crates/yserver/tests/v2_acceptance.rs` — verify existing call sites still compile (expected: no edits needed).

---

## Task 1: Backend clipList helper (pure logic, TDD)

Add the private clipList computation + the picture-mode reader, with pure unit tests. No trait changes, no op wiring yet — this is the standalone core the four ops will call.

**Files:**
- Modify: `crates/yserver/src/kms/v2/backend.rs` (add helpers near `clip_fill_rects_by_subwindow_mode` `:7130` and free fns near `compute_render_composite_clip` `:17978`)
- Test: `crates/yserver/src/kms/v2/backend.rs` (the `#[cfg(test)] mod tests` block, near `clip_fill_rects_by_subwindow_mode_subtracts_mapped_child` `:22584`)

- [ ] **Step 1: Write the failing tests**

Add to the tests module. These mirror the existing `clip_fill_rects_by_subwindow_mode_*` fixtures (`seed_window` helper at `:22579`, `KmsBackendV2::for_tests()`).

```rust
    // --- render_dst_cliplist_local ---

    fn rect(x: i16, y: i16, width: u16, height: u16) -> Rectangle16 {
        Rectangle16 { x, y, width, height }
    }

    fn as_set(v: Vec<Rectangle16>) -> std::collections::BTreeSet<(i16, i16, u16, u16)> {
        v.into_iter().map(|r| (r.x, r.y, r.width, r.height)).collect()
    }

    #[test]
    fn render_dst_cliplist_subtracts_mapped_child() {
        // dst 40x40 window, mapped automatic child at (10,20) size 15x10.
        // op paints the whole window; no picture clip. Result = window − child.
        let mut b = KmsBackendV2::for_tests();
        let _parent = seed_window(&mut b, 0x100, None, 0, 0);
        let _child = seed_window(&mut b, 0x200, Some(0x100), 10, 20);
        let child = b.windows_v2.get_mut(&0x200).expect("child geom");
        child.width = 15;
        child.height = 10;

        let out = b.render_dst_cliplist_local(
            0x100,
            true, // clip_by_children (ClipByChildren)
            None, // no picture clip
            rect(0, 0, 40, 40),
            rect(0, 0, 40, 40),
        );
        assert_eq!(
            as_set(out),
            std::collections::BTreeSet::from([
                (0, 0, 40, 20),
                (0, 30, 40, 10),
                (0, 20, 10, 10),
                (25, 20, 15, 10),
            ]),
        );
    }

    #[test]
    fn render_dst_cliplist_include_inferiors_keeps_children() {
        // clip_by_children=false ⇒ child NOT subtracted (guards F1).
        let mut b = KmsBackendV2::for_tests();
        let _parent = seed_window(&mut b, 0x100, None, 0, 0);
        let _child = seed_window(&mut b, 0x200, Some(0x100), 10, 20);
        let child = b.windows_v2.get_mut(&0x200).expect("child geom");
        child.width = 15;
        child.height = 10;

        let out = b.render_dst_cliplist_local(
            0x100,
            false, // IncludeInferiors
            None,
            rect(0, 0, 40, 40),
            rect(0, 0, 40, 40),
        );
        assert_eq!(as_set(out), std::collections::BTreeSet::from([(0, 0, 40, 40)]));
    }

    #[test]
    fn render_dst_cliplist_skips_manually_redirected_child() {
        // A manually-redirected child (scene_participating=false) is NOT
        // subtracted — the region covers under it (guards F2).
        let mut b = KmsBackendV2::for_tests();
        let _parent = seed_window(&mut b, 0x100, None, 0, 0);
        let child_id = seed_window(&mut b, 0x200, Some(0x100), 10, 20);
        {
            let child = b.windows_v2.get_mut(&0x200).expect("child geom");
            child.width = 15;
            child.height = 10;
        }
        // Flip the child's backing to manual-redirect semantics.
        b.store.set_scene_participating(child_id, false);

        let out = b.render_dst_cliplist_local(
            0x100,
            true,
            None,
            rect(0, 0, 40, 40),
            rect(0, 0, 40, 40),
        );
        assert_eq!(as_set(out), std::collections::BTreeSet::from([(0, 0, 40, 40)]));
    }

    #[test]
    fn render_dst_cliplist_intersects_picture_clip_and_op_bbox() {
        // Picture clip restricts to (0,0,20,40); op bbox restricts to
        // (5,5,30,30); no children. Result = clip ∩ bbox = (5,5,15,30).
        let mut b = KmsBackendV2::for_tests();
        let _parent = seed_window(&mut b, 0x100, None, 0, 0);

        let out = b.render_dst_cliplist_local(
            0x100,
            true,
            Some(&[rect(0, 0, 20, 40)]),
            rect(0, 0, 40, 40),
            rect(5, 5, 30, 30),
        );
        assert_eq!(as_set(out), std::collections::BTreeSet::from([(5, 5, 15, 30)]));
    }

    #[test]
    fn render_dst_cliplist_clamps_op_bbox_to_extent() {
        // op bbox spills past the 40x40 extent; result clamps to extent.
        let mut b = KmsBackendV2::for_tests();
        let _parent = seed_window(&mut b, 0x100, None, 0, 0);

        let out = b.render_dst_cliplist_local(
            0x100,
            true,
            None,
            rect(0, 0, 40, 40),
            rect(0, 0, 100, 100),
        );
        assert_eq!(as_set(out), std::collections::BTreeSet::from([(0, 0, 40, 40)]));
    }

    #[test]
    fn render_dst_cliplist_empty_picture_clip_paints_nothing() {
        // Some([]) is an EMPTY clip region (X RENDER "stop painting"
        // gate), NOT "no clip" — must yield an empty region. Guards the
        // already-fixed marco "shadow only" regression.
        let mut b = KmsBackendV2::for_tests();
        let _parent = seed_window(&mut b, 0x100, None, 0, 0);
        let out = b.render_dst_cliplist_local(
            0x100,
            true,
            Some(&[]),
            rect(0, 0, 40, 40),
            rect(0, 0, 40, 40),
        );
        assert!(out.is_empty());
    }

    #[test]
    fn render_dst_cliplist_fully_covered_is_empty() {
        // Child covers the whole window ⇒ empty region ⇒ no paint, no damage.
        let mut b = KmsBackendV2::for_tests();
        let _parent = seed_window(&mut b, 0x100, None, 0, 0);
        let _child = seed_window(&mut b, 0x200, Some(0x100), 0, 0);
        let child = b.windows_v2.get_mut(&0x200).expect("child geom");
        child.width = 40;
        child.height = 40;

        let out = b.render_dst_cliplist_local(
            0x100,
            true,
            None,
            rect(0, 0, 40, 40),
            rect(0, 0, 40, 40),
        );
        assert!(out.is_empty());
    }

    // --- dst_picture_clip_by_children (F1 mode source) ---

    #[test]
    fn dst_picture_clip_by_children_reads_picture_record() {
        let mut b = KmsBackendV2::for_tests();
        // ClipByChildren (default, subwindow_mode=0).
        b.core
            .pictures
            .insert(0xAA01, crate::kms::core::PictureRecord::drawable_default(0x100, 0));
        assert!(dst_picture_clip_by_children(&b.core, 0xAA01));

        // IncludeInferiors (subwindow_mode=1).
        if let Some(crate::kms::core::PictureRecord::Drawable { subwindow_mode, .. }) =
            b.core.pictures.get_mut(&0xAA01)
        {
            *subwindow_mode = 1;
        }
        assert!(!dst_picture_clip_by_children(&b.core, 0xAA01));

        // Missing picture ⇒ default ClipByChildren.
        assert!(dst_picture_clip_by_children(&b.core, 0xDEAD));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver --lib render_dst_cliplist 2>&1 | tail -20`
Expected: FAIL to compile — `no method named render_dst_cliplist_local`, `cannot find function dst_picture_clip_by_children`.

- [ ] **Step 3: Add the picture-mode reader (free fn)**

Insert next to `compute_render_composite_clip` (`crates/yserver/src/kms/v2/backend.rs:17978`):

```rust
/// True if the destination picture's `subWindowMode` is
/// `ClipByChildren` (the X RENDER default) — the X.Org-correct source
/// for gating child subtraction (`render/mipict.c:112`), stored per
/// picture (NOT the GC's `current_subwindow_mode`). Raw wire byte:
/// `0 = ClipByChildren`, `1 = IncludeInferiors`. A missing / non-Drawable
/// picture defaults to `ClipByChildren`.
fn dst_picture_clip_by_children(core: &KmsCore, host_pic: u32) -> bool {
    match core.pictures.get(&host_pic) {
        Some(PictureRecord::Drawable { subwindow_mode, .. }) => *subwindow_mode == 0,
        _ => true,
    }
}
```

- [ ] **Step 4: Add the clipList helper + extent + conversion helpers (impl methods)**

Insert as `impl KmsBackendV2` methods next to `clip_fill_rects_by_subwindow_mode` (`crates/yserver/src/kms/v2/backend.rs:7213`, just after it):

```rust
    /// Compute the destination window's RENDER `clipList` for one paint
    /// op, in **destination-window-local** coordinates (children in
    /// `windows_v2` are local too, so no offset juggling here). Order
    /// mirrors X.Org's `miComputeCompositeRegion` child clip:
    ///   1. base = picture clip ∩ dst extent (or the full extent if the
    ///      picture set no clip).
    ///   2. if `clip_by_children` and the dst is a window, subtract every
    ///      mapped child that is not manually-redirected (same child
    ///      enumeration + `scene_participating` skip as
    ///      `compute_copy_area_scissors`).
    ///   3. ∩ the op's bounding box.
    /// The result is the region the op actually paints; the caller shifts
    /// it by `dst_target.offset` for paint scissors and returns the
    /// un-shifted region for the core to damage. Empty ⇒ nothing painted.
    fn render_dst_cliplist_local(
        &self,
        dst_host_xid: u32,
        clip_by_children: bool,
        pre_shift_picture_clip: Option<&[Rectangle16]>,
        dst_local_extent: Rectangle16,
        op_bbox_local: Rectangle16,
    ) -> Vec<Rectangle16> {
        let extent = ash::vk::Rect2D {
            offset: ash::vk::Offset2D {
                x: i32::from(dst_local_extent.x),
                y: i32::from(dst_local_extent.y),
            },
            extent: ash::vk::Extent2D {
                width: u32::from(dst_local_extent.width),
                height: u32::from(dst_local_extent.height),
            },
        };
        // 1. base = picture clip ∩ extent, or the whole extent when the
        //    picture has NO clip (`None`). Per X RENDER semantics an
        //    EMPTY clip list (`Some([])`) is an *empty region* = paint
        //    NOTHING, NOT "no clip" — marco-with-compositing uses the
        //    empty-list form as a "stop painting until I set a real clip"
        //    gate (see the load-bearing comment in
        //    `render_set_picture_clip_rectangles`, `backend.rs:16409-16435`;
        //    `*clip = Some(rects)` where `rects` may be empty). Routing
        //    every `Some` through `intersect_rect_with_clip` yields the
        //    correct paint-nothing result because that helper returns
        //    empty for an empty clip list. Do NOT special-case
        //    `Some(empty)` as "no clip" — that reintroduces the
        //    already-fixed "shadow only" regression.
        let base: Vec<ash::vk::Rect2D> = match pre_shift_picture_clip {
            Some(clip) => {
                let clip_rects: Vec<ash::vk::Rect2D> = clip
                    .iter()
                    .filter(|r| r.width > 0 && r.height > 0)
                    .map(|r| ash::vk::Rect2D {
                        offset: ash::vk::Offset2D {
                            x: i32::from(r.x),
                            y: i32::from(r.y),
                        },
                        extent: ash::vk::Extent2D {
                            width: u32::from(r.width),
                            height: u32::from(r.height),
                        },
                    })
                    .collect();
                intersect_rect_with_clip(extent, &clip_rects)
            }
            None => {
                if extent.extent.width == 0 || extent.extent.height == 0 {
                    Vec::new()
                } else {
                    vec![extent]
                }
            }
        };
        if base.is_empty() {
            return Vec::new();
        }
        // 2. subtract mapped non-redirected children (ClipByChildren only,
        //    windows only — pixmaps have no windows_v2 entry).
        let after_children: Vec<ash::vk::Rect2D> =
            if clip_by_children && self.windows_v2.contains_key(&dst_host_xid) {
                let child_rects: Vec<ash::vk::Rect2D> = self
                    .windows_v2
                    .iter()
                    .filter_map(|(child_host_xid, geom)| {
                        if !(geom.parent == Some(dst_host_xid) && geom.mapped) {
                            return None;
                        }
                        let is_manually_redirected = self
                            .store
                            .lookup(*child_host_xid)
                            .and_then(|id| self.store.get(id))
                            .is_some_and(|d| !d.scene_participating);
                        if is_manually_redirected {
                            return None;
                        }
                        Some(ash::vk::Rect2D {
                            offset: ash::vk::Offset2D {
                                x: i32::from(geom.x),
                                y: i32::from(geom.y),
                            },
                            extent: ash::vk::Extent2D {
                                width: u32::from(geom.width.max(1)),
                                height: u32::from(geom.height.max(1)),
                            },
                        })
                    })
                    .collect();
                if child_rects.is_empty() {
                    base
                } else {
                    base.into_iter()
                        .flat_map(|r| compute_copy_area_dst_rects(r, &child_rects))
                        .collect()
                }
            } else {
                base
            };
        if after_children.is_empty() {
            return Vec::new();
        }
        // 3. ∩ op bounding box.
        let bbox = ash::vk::Rect2D {
            offset: ash::vk::Offset2D {
                x: i32::from(op_bbox_local.x),
                y: i32::from(op_bbox_local.y),
            },
            extent: ash::vk::Extent2D {
                width: u32::from(op_bbox_local.width),
                height: u32::from(op_bbox_local.height),
            },
        };
        intersect_rect_with_clip(bbox, &after_children)
            .into_iter()
            .filter_map(|r| {
                Some(Rectangle16 {
                    x: i16::try_from(r.offset.x).ok()?,
                    y: i16::try_from(r.offset.y).ok()?,
                    width: u16::try_from(r.extent.width).ok()?,
                    height: u16::try_from(r.extent.height).ok()?,
                })
            })
            .collect()
    }

    /// The destination drawable's own extent in local coords: window w×h
    /// from `windows_v2`, else the store's backing extent (pixmaps).
    fn dst_local_extent(
        &self,
        dst_host_xid: u32,
        dst_id: crate::kms::v2::store::DrawableId,
    ) -> Rectangle16 {
        if let Some(g) = self.windows_v2.get(&dst_host_xid) {
            Rectangle16 {
                x: 0,
                y: 0,
                width: g.width,
                height: g.height,
            }
        } else {
            let ext = self
                .store
                .get(dst_id)
                .map(|d| d.extent)
                .unwrap_or(ash::vk::Extent2D {
                    width: 0,
                    height: 0,
                });
            Rectangle16 {
                x: 0,
                y: 0,
                width: u16::try_from(ext.width).unwrap_or(u16::MAX),
                height: u16::try_from(ext.height).unwrap_or(u16::MAX),
            }
        }
    }
```

Add the conversion free fn next to `dst_picture_clip_by_children`:

```rust
/// Convert a local clipList (backend `Rectangle16`) into the trait's
/// wire rectangle currency (`xfixes::RegionRect`) for the RENDER op
/// return value the core damages. Fields are identical; this is the
/// crate-boundary hop (`Rectangle16` is `yserver`-local).
fn local_rects_to_region(rects: Vec<Rectangle16>) -> Vec<xfixes::RegionRect> {
    rects
        .into_iter()
        .map(|r| xfixes::RegionRect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        })
        .collect()
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p yserver --lib render_dst_cliplist dst_picture_clip_by_children 2>&1 | tail -20`
Expected: PASS (7 tests: 6 `render_dst_cliplist_*` + `dst_picture_clip_by_children_reads_picture_record`).

- [ ] **Step 6: Commit**

```bash
git add crates/yserver/src/kms/v2/backend.rs
git commit -m "feat(render): clipList helper for RENDER ClipByChildren parity

Pure backend helper computing dst clipList (picture clip ∩ extent −
children ∩ op bbox) in window-local coords, gated on a caller-supplied
clip_by_children bool sourced from the picture record. Not yet wired
into the ops.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Avkmce8W4mDeLap2Y3x1Tb"
```

---

## Task 2: Flip the trait return type (type-only refactor, behaviour unchanged)

Change all four RENDER methods to return `io::Result<Vec<xfixes::RegionRect>>` across the trait, the recording double, the nested host, and the KMS backend. KMS ops return `Ok(Vec::new())` transitionally (the core still ignores the value and full-damages, so behaviour is identical); the real regions land in Tasks 3–6. This is a mechanical seam-widening commit, not a shipped stub — the feature is completed within this plan.

**Files:**
- Modify: `crates/yserver-core/src/backend/trait_def.rs:1591,1609,1637,1656`
- Modify: `crates/yserver-core/src/backend/recording.rs:146` (struct), `:231` (new), `:1092,1111,1142` (methods) + add `render_triangles_op` override
- Modify: `crates/yserver-core/src/host_x11/trait_impl.rs:857,881,919,948`
- Modify: `crates/yserver/src/kms/v2/backend.rs:15151,15327,15806,15988` (four `-> io::Result<()>` → new type; every `return Ok(())` / trailing `Ok(())` → `Ok(Vec::new())`)

- [ ] **Step 1: Change the trait signatures**

In `crates/yserver-core/src/backend/trait_def.rs`, change the return type of `render_composite` (`:1606`), `render_composite_glyphs` (`:1623`), `render_trapezoids` (`:1649`) from `-> io::Result<()>;` to `-> io::Result<Vec<xfixes::RegionRect>>;`. For `render_triangles_op` (`:1669`) change the default body:

```rust
    ) -> io::Result<Vec<xfixes::RegionRect>> {
        Ok(Vec::new())
    }
```

(`xfixes` is already imported at `trait_def.rs:15`.)

- [ ] **Step 2: Update the RecordingBackend double**

In `crates/yserver-core/src/backend/recording.rs`, add a field to `struct RecordingBackend` (after `xkb_mods` at `:221`):

```rust
    /// Configurable region returned by the four RENDER paint methods so
    /// core-plumbing tests can drive an exact damage region without a
    /// real backend. Default empty ⇒ no damage.
    pub render_return_region: Vec<xfixes::RegionRect>,
```

Ensure `xfixes` is in scope in `recording.rs` — add `use yserver_protocol::x11::xfixes;` to the imports if not already present (check the top of the file; `set_shape_rectangles` at `:1247` already references `xfixes::RegionRect`, so it is imported).

In `RecordingBackend::new()` (`:232`), initialise it after `xkb_mods: (0, 0, 0, 0),`:

```rust
            render_return_region: Vec::new(),
```

Change the three overridden methods' return type and body:
- `render_composite` (`:1107`): `-> io::Result<()> {` → `-> io::Result<Vec<xfixes::RegionRect>> {`, body `Ok(())` → `Ok(self.render_return_region.clone())`.
- `render_composite_glyphs` (`:1125`): same change.
- `render_trapezoids` (`:1154`): same change.

Add an explicit `render_triangles_op` override (RecordingBackend currently inherits the trait default, which would silently return empty and drop triangle damage). Insert after `render_trapezoids` (`:1156`):

```rust
    fn render_triangles_op(
        &mut self,
        _origin: Option<OriginContext>,
        _minor: u8,
        _op: u8,
        _host_src: u32,
        _host_dst: u32,
        _host_mask_format: u32,
        _src_x: i16,
        _src_y: i16,
        _primitives: &[u8],
        _x_off: i16,
        _y_off: i16,
    ) -> io::Result<Vec<xfixes::RegionRect>> {
        Ok(self.render_return_region.clone())
    }
```

- [ ] **Step 3: Update the nested host_x11 impl**

In `crates/yserver-core/src/host_x11/trait_impl.rs`, for each of `render_composite` (`:857`), `render_composite_glyphs` (`:881`), `render_trapezoids` (`:919`), `render_triangles_op` (`:948`): change `-> io::Result<()> {` to `-> io::Result<Vec<xfixes::RegionRect>> {`, and wrap the delegated call to discard its `()` and return empty (nested damage is out of scope — ynest is unmaintained). Example for `render_composite`:

```rust
    ) -> io::Result<Vec<xfixes::RegionRect>> {
        self.with_active_origin(origin, |this| {
            HostX11Backend::render_composite(
                this, op, host_src, host_mask, host_dst, src_x, src_y, mask_x, mask_y, dst_x,
                dst_y, width, height,
            )
        })?;
        Ok(Vec::new())
    }
```

Apply the same `?;` + `Ok(Vec::new())` shape to the other three. Ensure `xfixes` is in scope (add `use yserver_protocol::x11::xfixes;` if the compiler flags it).

- [ ] **Step 4: Update the KMS backend signatures (transitional empty returns)**

In `crates/yserver/src/kms/v2/backend.rs`, for `render_composite` (`:15166`), `render_composite_glyphs` (`:15341`), `render_trapezoids` (`:15818`), `render_triangles_op` (`:16001`): change each `-> io::Result<()> {` to `-> io::Result<Vec<xfixes::RegionRect>> {`, and replace **every** `return Ok(());` and the trailing `Ok(())` inside those four methods with `return Ok(Vec::new());` / `Ok(Vec::new())`.

(There are several early-`return Ok(())` gap paths per method — e.g. `render_composite:15169,15176,15190,15200`; `render_trapezoids:15825,15829,15866,15872,15879,15899,15904`; `render_triangles_op` the `_ => return Ok(())` arms and bbox bails; `render_composite_glyphs:15364,15371,15397,15402,15408,15413,15588,15621`. Update all of them, plus each method's final `Ok(())`.)

- [ ] **Step 5: Verify the whole workspace compiles and existing tests pass**

Run: `cargo build --locked 2>&1 | tail -20`
Expected: clean build.

Run: `cargo test -p yserver --test v2_acceptance 2>&1 | tail -20`
Expected: PASS. The acceptance call sites use `.expect("...")` in statement position (`:275`, `:399`, `:815`, …) or `let g = ...; g.expect(...)` (`:4310`) — `.expect()` now yields `Vec<RegionRect>` which is dropped; `Vec` is not `#[must_use]`, so no edits are needed. If the compiler *does* flag any site, bind the result with `let _ = ...` there.

Run: `cargo test -p yserver-core render_composite_emits_damage_on_dst_drawable 2>&1 | tail -20`
Expected: PASS — the core still calls `accumulate_damage_full_to_state` (Task 7 changes this), so the existing assertion (`damage.rects` non-empty) still holds.

- [ ] **Step 6: Commit**

```bash
git add crates/yserver-core/src/backend/trait_def.rs \
  crates/yserver-core/src/backend/recording.rs \
  crates/yserver-core/src/host_x11/trait_impl.rs \
  crates/yserver/src/kms/v2/backend.rs
git commit -m "refactor(render): RENDER paint ops return Vec<RegionRect>

Widen the four RENDER paint methods to return a painted-region vector
(RegionRect — the trait's cross-crate rectangle type). KMS returns empty
transitionally; core still full-damages. No behaviour change.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Avkmce8W4mDeLap2Y3x1Tb"
```

---

## Task 3: Wire Composite (paint child-clip + return region)

**Files:**
- Modify: `crates/yserver/src/kms/v2/backend.rs` — `render_composite` body (`:15151`)
- Test: `crates/yserver/tests/v2_acceptance.rs` (new test)

- [ ] **Step 1: Write the failing acceptance test**

Add to `crates/yserver/tests/v2_acceptance.rs`. Model the picture/window seeding on `v2_render_composite_no_gc_clip_leak` (`:212`). This drives the real `render_composite` and asserts the returned region = (window − child) ∩ op bbox.

```rust
/// RENDER Composite over a window with a mapped automatic child must
/// return the ClipByChildren clipList (window − child ∩ op bbox), in
/// window-local coords — the region the core will damage.
#[test]
fn v2_render_composite_returns_child_clipped_region() {
    let mut b = KmsBackendV2::for_tests();
    // Parent 0x100 @ 100x100; child 0x200 @ (0,0) 40x40, mapped automatic.
    let _p = seed_window(&mut b, 0x100, None, 0, 0);
    {
        let g = b.windows_v2.get_mut(&0x100).unwrap();
        g.width = 100;
        g.height = 100;
    }
    let _c = seed_window(&mut b, 0x200, Some(0x100), 0, 0);
    {
        let g = b.windows_v2.get_mut(&0x200).unwrap();
        g.width = 40;
        g.height = 40;
    }
    // Source solid + dst picture wrapping the parent window.
    let src_pic = b
        .render_create_solid_fill(None, [0u8; 8])
        .expect("solid")
        .expect("handle");
    b.core.pictures.insert(
        0xD01,
        crate::kms::core::PictureRecord::drawable_default(0x100, 0),
    );
    let dst_pic = yserver_core::backend::PictureHandle::from_raw_for_test(0xD01);

    // Composite over the whole 100x100 parent.
    let region = b
        .render_composite(
            None, 3, src_pic.as_raw(), 0, dst_pic.as_raw(), 0, 0, 0, 0, 0, 0, 100, 100,
        )
        .expect("render_composite");

    let got: std::collections::BTreeSet<(i16, i16, u16, u16)> = region
        .into_iter()
        .map(|r| (r.x, r.y, r.width, r.height))
        .collect();
    // Window (0,0,100,100) minus child (0,0,40,40): the two Xorg-band
    // strips (bottom, then middle-right of the top band).
    assert_eq!(
        got,
        std::collections::BTreeSet::from([(0, 40, 100, 60), (40, 0, 60, 40)]),
    );
}
```

> **Note for the implementer:** confirm the `seed_window` helper and `KmsBackendV2` / `PictureRecord` / `render_create_solid_fill` symbols are importable in `v2_acceptance.rs` (the file already exercises `render_composite`, `render_create_solid_fill`, and `seed_window`-style seeding — reuse the exact imports/helpers already at the top of that file; if `seed_window` is test-module-private in `backend.rs`, seed `windows_v2`/`store` the same way the neighbouring acceptance tests do). The **expected region values above are derived from `subtract_one_rect_clip`'s Xorg band order** (top strip, bottom strip, then middle-band left/right). With child at the top-left corner there is no top strip and no middle-left strip, leaving the bottom strip `(0,40,100,60)` and the middle-band right strip `(40,0,60,40)`. If the band decomposition differs, read the actual output and reconcile against `subtract_one_rect_clip` (`:18031`) — do **not** hand-adjust the test to match a wrong impl.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p yserver --test v2_acceptance v2_render_composite_returns_child_clipped_region 2>&1 | tail -20`
Expected: FAIL — Task 2 returns `Ok(Vec::new())`, so `region` is empty and the assertion fails.

- [ ] **Step 3: Rewire `render_composite`**

In `render_composite` (`crates/yserver/src/kms/v2/backend.rs:15151`), replace the block from the `shift_dst_picture_clip` call (`:15201`) through the `compute_render_composite_clip` assignment (`:15230-15236`) so the clipList is computed in local coords first, then shifted for paint and folded with src/mask, and returned for damage. The current `:15201` line is:

```rust
        let dst_clip = Self::shift_dst_picture_clip(dst_clip, dst_target.offset);
```

Replace it with (keep `dst_clip` from `resolve_dst_picture_for_render` **un-shifted** as the local picture clip):

```rust
        // ClipByChildren clipList in dst-window-local coords: picture clip
        // ∩ extent − mapped children ∩ this op's dst rect. Gated on the
        // DESTINATION PICTURE's subWindowMode (not the GC's).
        let clip_by_children = dst_picture_clip_by_children(&self.core, host_dst);
        let dst_local_extent = self.dst_local_extent(dst_host_xid, dst_target.id);
        let op_bbox_local = Rectangle16 {
            x: dst_x,
            y: dst_y,
            width,
            height,
        };
        let cliplist_local = self.render_dst_cliplist_local(
            dst_host_xid,
            clip_by_children,
            dst_clip.as_deref(),
            dst_local_extent,
            op_bbox_local,
        );
        if cliplist_local.is_empty() {
            // Fully clipped away (e.g. dst covered by a child) — no paint,
            // no damage.
            return Ok(Vec::new());
        }
        // Shift the clipList into backing space for the paint scissors.
        let dst_clip =
            Self::shift_dst_picture_clip(Some(cliplist_local.clone()), dst_target.offset);
```

The existing `src_clip` / `mask_clip` / translation lines (`:15214-15229`) stay unchanged. The `compute_render_composite_clip` call (`:15230`) now folds src/mask on top of the shifted clipList — it already reads `dst_clip.as_deref()`, so no edit there.

At the method's end, change the final `Ok(Vec::new())` (from Task 2, at `:15324`) to return the painted region:

```rust
        Ok(local_rects_to_region(cliplist_local))
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p yserver --test v2_acceptance v2_render_composite_returns_child_clipped_region 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Guard against paint regression**

Run: `cargo test -p yserver --test v2_acceptance v2_render_composite 2>&1 | tail -30`
Expected: PASS — existing composite acceptance tests (no children present) still paint identically; with no children and no picture clip the clipList is just the op bbox clamped to extent, equivalent to the prior `None`/picture-clip scissor.

- [ ] **Step 6: Commit**

```bash
git add crates/yserver/src/kms/v2/backend.rs crates/yserver/tests/v2_acceptance.rs
git commit -m "feat(render): Composite clips paint + returns damage region by children

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Avkmce8W4mDeLap2Y3x1Tb"
```

---

## Task 4: Wire Trapezoids

**Files:**
- Modify: `crates/yserver/src/kms/v2/backend.rs` — `render_trapezoids` body (`:15806`)
- Test: `crates/yserver/tests/v2_acceptance.rs` (new test)

- [ ] **Step 1: Write the failing acceptance test**

Model on `v2_render_trapezoids_renders_filled_rect` (`:761`) for the trap encoding + seeding. One axis-aligned trapezoid covering a 40×40 box, dst window 100×100 with a mapped child at `(0,0,40,40)`; assert the returned region = trap bbox − child.

```rust
/// RENDER Trapezoids over a window with a mapped automatic child returns
/// the primitive-bbox ∩ (window − child) clipList in local coords.
#[test]
fn v2_render_trapezoids_returns_child_clipped_region() {
    let mut b = KmsBackendV2::for_tests();
    let _p = seed_window(&mut b, 0x100, None, 0, 0);
    {
        let g = b.windows_v2.get_mut(&0x100).unwrap();
        g.width = 100;
        g.height = 100;
    }
    let _c = seed_window(&mut b, 0x200, Some(0x100), 0, 0);
    {
        let g = b.windows_v2.get_mut(&0x200).unwrap();
        g.width = 20;
        g.height = 20;
    }
    let src_pic = b
        .render_create_solid_fill(None, [0xFFu8; 8])
        .expect("solid")
        .expect("handle");
    b.core.pictures.insert(
        0xD02,
        crate::kms::core::PictureRecord::drawable_default(0x100, 0),
    );
    let dst_pic = yserver_core::backend::PictureHandle::from_raw_for_test(0xD02);

    // One trapezoid = axis-aligned box (0,0)-(40,40), 16.16 fixed-point.
    let f = |v: i32| (v << 16).to_le_bytes();
    let mut traps = Vec::new();
    traps.extend_from_slice(&f(0)); // top
    traps.extend_from_slice(&f(40)); // bottom
    traps.extend_from_slice(&f(0)); // left.p1.x
    traps.extend_from_slice(&f(0)); // left.p1.y
    traps.extend_from_slice(&f(0)); // left.p2.x
    traps.extend_from_slice(&f(40)); // left.p2.y
    traps.extend_from_slice(&f(40)); // right.p1.x
    traps.extend_from_slice(&f(0)); // right.p1.y
    traps.extend_from_slice(&f(40)); // right.p2.x
    traps.extend_from_slice(&f(40)); // right.p2.y

    let region = b
        .render_trapezoids(None, 3, src_pic.as_raw(), dst_pic.as_raw(), 0, 0, 0, &traps, 0, 0)
        .expect("render_trapezoids");
    let got: std::collections::BTreeSet<(i16, i16, u16, u16)> = region
        .into_iter()
        .map(|r| (r.x, r.y, r.width, r.height))
        .collect();
    // Trap bbox (0,0,40,40) minus child (0,0,20,20): bottom strip +
    // middle-band right strip.
    assert_eq!(
        got,
        std::collections::BTreeSet::from([(0, 20, 40, 20), (20, 0, 20, 20)]),
    );
}
```

> **Note:** verify `trapezoid_bbox` yields `(0,0,40,40)` for this trap before trusting the expected values; if the fixed-point rounding produces a slightly different bbox, reconcile against `vk_traps::trapezoid_bbox` — do not fudge the test.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p yserver --test v2_acceptance v2_render_trapezoids_returns_child_clipped_region 2>&1 | tail -20`
Expected: FAIL — returns empty from Task 2.

- [ ] **Step 3: Rewire `render_trapezoids`**

In `render_trapezoids` (`:15806`), the early shift is at `:15881`:

```rust
        let dst_clip = Self::shift_dst_picture_clip(dst_clip, dst_target.offset);
```

Keep `dst_clip` un-shifted until after the bbox is known. Move/replace so the sequence is:
1. Leave `:15881`'s shift **removed** — do not shift yet.
2. After the bbox clamp block (`:15898-15909`, which yields `bx, by, bw, bh` in **backing** space because the `dx/dy` fold at `:15884-15897` already applied `dst_target.offset`), compute the **local** bbox by subtracting the offset, then compute the clipList and shift it:

Replace the removed shift with — inserted right after the `bh` computation (`:15909`):

```rust
        // Local op bbox = backing bbox − redirect offset (children in
        // windows_v2 are window-local). offset is (0,0) for unredirected
        // windows and pixmaps.
        let bbox_local = Rectangle16 {
            x: i16::try_from((bx - dst_target.offset.0).max(0)).unwrap_or(i16::MAX),
            y: i16::try_from((by - dst_target.offset.1).max(0)).unwrap_or(i16::MAX),
            width: u16::try_from(bw).unwrap_or(u16::MAX),
            height: u16::try_from(bh).unwrap_or(u16::MAX),
        };
        let clip_by_children = dst_picture_clip_by_children(&self.core, host_dst);
        let dst_local_extent = self.dst_local_extent(dst_host_xid, dst_target.id);
        let cliplist_local = self.render_dst_cliplist_local(
            dst_host_xid,
            clip_by_children,
            dst_clip.as_deref(),
            dst_local_extent,
            bbox_local,
        );
        if cliplist_local.is_empty() {
            return Ok(Vec::new());
        }
        let dst_clip =
            Self::shift_dst_picture_clip(Some(cliplist_local.clone()), dst_target.offset);
```

The engine call at `:15937` already passes `dst_clip.as_deref()` — now the child-clipped, shifted clipList. Change the method's trailing `Ok(Vec::new())` (Task 2, `:15985`) to:

```rust
        Ok(local_rects_to_region(cliplist_local))
```

> **Ordering caveat:** `dst_clip` is the value from `resolve_dst_picture_for_render` at `:15868`. Between there and the new insertion it must stay un-shifted — only the `dx`/`dy` *trap coordinate* fold (`:15882-15897`) runs in between, which does not touch `dst_clip`. Confirm no other read of `dst_clip` occurs before the new block.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p yserver --test v2_acceptance v2_render_trapezoids 2>&1 | tail -30`
Expected: the new test PASS + existing trap tests (no children) still PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/v2/backend.rs crates/yserver/tests/v2_acceptance.rs
git commit -m "feat(render): Trapezoids clip paint + return damage region by children

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Avkmce8W4mDeLap2Y3x1Tb"
```

---

## Task 5: Wire Triangles / TriStrip / TriFan

**Files:**
- Modify: `crates/yserver/src/kms/v2/backend.rs` — `render_triangles_op` body (`:15988`)
- Test: `crates/yserver/tests/v2_acceptance.rs` (new test)

- [ ] **Step 1: Write the failing acceptance test**

`render_triangles_op` minor 11 (Triangles), one triangle whose bbox is `(0,0,40,40)`, over a 100×100 window with a mapped child at `(0,0,20,20)`.

```rust
/// RENDER Triangles over a window with a mapped automatic child returns
/// the triangle-bbox ∩ (window − child) clipList in local coords.
#[test]
fn v2_render_triangles_returns_child_clipped_region() {
    let mut b = KmsBackendV2::for_tests();
    let _p = seed_window(&mut b, 0x100, None, 0, 0);
    {
        let g = b.windows_v2.get_mut(&0x100).unwrap();
        g.width = 100;
        g.height = 100;
    }
    let _c = seed_window(&mut b, 0x200, Some(0x100), 0, 0);
    {
        let g = b.windows_v2.get_mut(&0x200).unwrap();
        g.width = 20;
        g.height = 20;
    }
    let src_pic = b
        .render_create_solid_fill(None, [0xFFu8; 8])
        .expect("solid")
        .expect("handle");
    b.core.pictures.insert(
        0xD03,
        crate::kms::core::PictureRecord::drawable_default(0x100, 0),
    );
    let dst_pic = yserver_core::backend::PictureHandle::from_raw_for_test(0xD03);

    // One triangle: (0,0), (40,0), (0,40) — bbox (0,0,40,40). 16.16 fixed.
    let f = |v: i32| (v << 16).to_le_bytes();
    let mut prims = Vec::new();
    for (x, y) in [(0, 0), (40, 0), (0, 40)] {
        prims.extend_from_slice(&f(x));
        prims.extend_from_slice(&f(y));
    }

    let region = b
        .render_triangles_op(None, 11, 3, src_pic.as_raw(), dst_pic.as_raw(), 0, 0, 0, &prims, 0, 0)
        .expect("render_triangles_op");
    let got: std::collections::BTreeSet<(i16, i16, u16, u16)> = region
        .into_iter()
        .map(|r| (r.x, r.y, r.width, r.height))
        .collect();
    assert_eq!(
        got,
        std::collections::BTreeSet::from([(0, 20, 40, 20), (20, 0, 20, 20)]),
    );
}
```

> **Note:** confirm `triangle_bbox` gives `(0,0,40,40)` for these three points before trusting the expected region.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p yserver --test v2_acceptance v2_render_triangles_returns_child_clipped_region 2>&1 | tail -20`
Expected: FAIL — empty from Task 2.

- [ ] **Step 3: Rewire `render_triangles_op`**

Identical shape to Task 4. The early shift is at `:16083`; keep `dst_clip` un-shifted, and after the `bh` computation (`:16107`) insert the same block (the `dx`/`dy` fold `:16086-16095` runs between, touching only triangle coords):

```rust
        let bbox_local = Rectangle16 {
            x: i16::try_from((bx - dst_target.offset.0).max(0)).unwrap_or(i16::MAX),
            y: i16::try_from((by - dst_target.offset.1).max(0)).unwrap_or(i16::MAX),
            width: u16::try_from(bw).unwrap_or(u16::MAX),
            height: u16::try_from(bh).unwrap_or(u16::MAX),
        };
        let clip_by_children = dst_picture_clip_by_children(&self.core, host_dst);
        let dst_local_extent = self.dst_local_extent(dst_host_xid, dst_target.id);
        let cliplist_local = self.render_dst_cliplist_local(
            dst_host_xid,
            clip_by_children,
            dst_clip.as_deref(),
            dst_local_extent,
            bbox_local,
        );
        if cliplist_local.is_empty() {
            return Ok(Vec::new());
        }
        let dst_clip =
            Self::shift_dst_picture_clip(Some(cliplist_local.clone()), dst_target.offset);
```

Remove the original `let dst_clip = Self::shift_dst_picture_clip(dst_clip, dst_target.offset);` at `:16083`. The engine call (`:16131`) uses `dst_clip.as_deref()` unchanged. Change the method's trailing `Ok(Vec::new())` (Task 2 — the last statement of the method, after the telemetry block near `:16155+`) to:

```rust
        Ok(local_rects_to_region(cliplist_local))
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p yserver --test v2_acceptance v2_render_triangles 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/v2/backend.rs crates/yserver/tests/v2_acceptance.rs
git commit -m "feat(render): Triangles clip paint + return damage region by children

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Avkmce8W4mDeLap2Y3x1Tb"
```

---

## Task 6: Wire CompositeGlyphs (glyph-quad union bbox)

The op bbox here is the **single bounding box of the actually-rendered glyph destination quads** (local coords), not the request envelope. This matches X.Org's `GlyphExtents` (`/home/jos/Projects/xserver/render/glyph.c:499`), which likewise computes one min/max box over all resolved glyphs and composites/damages within it. **Do not** use a per-glyph multi-rect union — that would be *more* precise than X.Org and risk a damage/scissor-rect explosion for long text runs (one rect per glyph). Using only `parsed[]` (glyphs that actually resolved) already excludes missing/zero-size glyphs, so the box is the rendered extent, not the envelope — which is the spec's intent. The glyph path builds `parsed[]` with local `dst_x, dst_y, w, h` (`:15566-15574`) before the paint offset is folded into `inputs[]` (`:15614-15615`).

**Files:**
- Modify: `crates/yserver/src/kms/v2/backend.rs` — `render_composite_glyphs` body (`:15327`)
- Test: `crates/yserver/tests/v2_acceptance.rs` (new test)

- [ ] **Step 1: Write the failing acceptance test**

Model on `v2_composite_glyphs_*` seeding (glyphset creation + `render_add_glyphs`) used around `:3769-3862`. Seed a glyphset with one glyph large enough that its dst quad overlaps a child; assert the returned region = glyph-quad-union − child. Because glyph seeding is verbose, reuse the exact helper sequence the neighbouring glyph acceptance test uses.

```rust
/// RENDER CompositeGlyphs over a window with a mapped automatic child
/// returns the union of rendered glyph quads ∩ (window − child).
#[test]
fn v2_render_composite_glyphs_returns_child_clipped_region() {
    let mut b = KmsBackendV2::for_tests();
    let _p = seed_window(&mut b, 0x100, None, 0, 0);
    {
        let g = b.windows_v2.get_mut(&0x100).unwrap();
        g.width = 100;
        g.height = 100;
    }
    let _c = seed_window(&mut b, 0x200, Some(0x100), 0, 0);
    {
        let g = b.windows_v2.get_mut(&0x200).unwrap();
        g.width = 10;
        g.height = 10;
    }
    let src_pic = b
        .render_create_solid_fill(None, [0xFFu8; 8])
        .expect("solid")
        .expect("handle");
    b.core.pictures.insert(
        0xD04,
        crate::kms::core::PictureRecord::drawable_default(0x100, 0),
    );
    let dst_pic = yserver_core::backend::PictureHandle::from_raw_for_test(0xD04);

    // Glyphset with one A8 glyph, 20x20, origin (x,y)=(0,0), advance 0.
    // Reuse the glyphset-seed sequence from the neighbouring glyph test:
    //   create_glyphset (A8) → render_add_glyphs(one 20x20 glyph id=1).
    // <implementer: copy that helper block verbatim; call the resulting
    //  host glyphset xid `gs_raw`.>
    let gs_raw = seed_single_a8_glyph(&mut b, /*w*/ 20, /*h*/ 20, /*id*/ 1);

    // One element: count=1, dx=0, dy=0, id=1 (CompositeGlyphs8, minor 23).
    let mut items = Vec::new();
    items.extend_from_slice(&[1u8, 0, 0, 0]); // count + pad
    items.extend_from_slice(&i16::to_le_bytes(0)); // dx
    items.extend_from_slice(&i16::to_le_bytes(0)); // dy
    items.extend_from_slice(&[1u8, 0, 0, 0]); // id=1 + 3 pad (padded to 4)

    let region = b
        .render_composite_glyphs(
            None, 23, 3, src_pic.as_raw(), dst_pic.as_raw(), 0, gs_raw, 0, 0, &items, 0, 0,
        )
        .expect("render_composite_glyphs");
    let got: std::collections::BTreeSet<(i16, i16, u16, u16)> = region
        .into_iter()
        .map(|r| (r.x, r.y, r.width, r.height))
        .collect();
    // Glyph quad (0,0,20,20) minus child (0,0,10,10).
    assert_eq!(
        got,
        std::collections::BTreeSet::from([(0, 10, 20, 10), (10, 0, 10, 10)]),
    );
}
```

> **Note for the implementer:** there is no existing `seed_single_a8_glyph` helper — write one (or inline the glyphset create + `render_add_glyphs` byte-packing the neighbouring glyph acceptance test already does; see `render_add_glyphs` parser at `backend.rs`), returning the host glyphset xid. The glyph's `x`/`y` (bearing) must be 0 so `dst_x = pen_x - glyph.x = 0`. Verify the resulting quad is `(0,0,20,20)` before trusting the expected region.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p yserver --test v2_acceptance v2_render_composite_glyphs_returns_child_clipped_region 2>&1 | tail -20`
Expected: FAIL — empty from Task 2.

- [ ] **Step 3: Rewire `render_composite_glyphs`**

The early shift is at `:15409`:

```rust
        let dst_clip = Self::shift_dst_picture_clip(dst_clip, dst_target.offset);
```

Keep `dst_clip` un-shifted here (remove that line). After `parsed` is filled and the `if parsed.is_empty()` guard (`:15584-15589`), compute the local glyph-quad union bbox, then the clipList, then shift. Insert after `:15589`:

```rust
        // op bbox = union of the rendered glyph dst quads (local coords —
        // parsed[].dst_x/dst_y are pre-offset; the paint offset is folded
        // into `inputs` below).
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for p in &parsed {
            min_x = min_x.min(p.dst_x);
            min_y = min_y.min(p.dst_y);
            max_x = max_x.max(p.dst_x + i32::try_from(p.w).unwrap_or(0));
            max_y = max_y.max(p.dst_y + i32::try_from(p.h).unwrap_or(0));
        }
        let glyph_union_local = Rectangle16 {
            x: i16::try_from(min_x.max(0)).unwrap_or(i16::MAX),
            y: i16::try_from(min_y.max(0)).unwrap_or(i16::MAX),
            width: u16::try_from((max_x - min_x).max(0)).unwrap_or(u16::MAX),
            height: u16::try_from((max_y - min_y).max(0)).unwrap_or(u16::MAX),
        };
        let clip_by_children = dst_picture_clip_by_children(&self.core, host_dst);
        let dst_local_extent = self.dst_local_extent(dst_host_xid, dst_target.id);
        let cliplist_local = self.render_dst_cliplist_local(
            dst_host_xid,
            clip_by_children,
            dst_clip.as_deref(),
            dst_local_extent,
            glyph_union_local,
        );
        if cliplist_local.is_empty() {
            return Ok(Vec::new());
        }
        let dst_clip =
            Self::shift_dst_picture_clip(Some(cliplist_local.clone()), dst_target.offset);
```

The `dst_clip` used by the engine call at `:15630` is now the shifted clipList. Change the method's trailing `Ok(Vec::new())` (Task 2, `:15699`) — and the earlier `if inputs.is_empty() { return Ok(Vec::new()); }` (`:15620-15622`) stays returning empty (nothing painted) — to return the region at the successful end:

```rust
        Ok(local_rects_to_region(cliplist_local))
```

> **Caveat:** `dst_clip` (from `resolve_dst_picture_for_render` at `:15399`) must stay un-shifted through the entire item-parse loop. The parse loop does not read `dst_clip`, so this is safe; confirm by grepping the method body for `dst_clip` between `:15399` and the new block.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p yserver --test v2_acceptance v2_render_composite_glyphs 2>&1 | tail -20`
Expected: the new test PASS + existing glyph tests still PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/v2/backend.rs crates/yserver/tests/v2_acceptance.rs
git commit -m "feat(render): CompositeGlyphs clip paint + return damage region by children

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Avkmce8W4mDeLap2Y3x1Tb"
```

---

## Task 7: Core damages the returned region (all three arms) + plumbing tests

Replace `accumulate_damage_full_to_state` with per-rect `accumulate_damage_to_state` over the region returned by each backend call, at the Composite arm (`:1628`), the Traps/Tris arm (`:1675`), and the CompositeGlyphs arm (`:1806`).

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs` — three arms (`:1662`, `:1716`/`:1730`, `:1841`)
- Test: `crates/yserver-core/src/core_loop/process_request.rs` — update `render_composite_emits_damage_on_dst_drawable` (`:40856`) + add three per-arm tests

- [ ] **Step 1: Update the existing plumbing test + write three failing per-arm tests**

Rewrite `render_composite_emits_damage_on_dst_drawable` (`:40856`) to set `RecordingBackend.render_return_region` to a known region and assert the core damages **exactly** it. Change the backend construction (`:40873`) and the final assertion (`:40954-40960`):

```rust
        let mut backend = RecordingBackend::new();
        backend.render_return_region = vec![
            yserver_protocol::x11::xfixes::RegionRect { x: 0, y: 0, width: 100, height: 80 },
        ];
```

```rust
        let damage = state.damage_objects.get(&DAMAGE_XID).unwrap();
        // Core must damage exactly the backend-returned region.
        assert_eq!(
            damage.rects.len(),
            1,
            "composite must damage exactly the backend-returned region",
        );
        assert_eq!(
            (damage.rects[0].x, damage.rects[0].y, damage.rects[0].width, damage.rects[0].height),
            (0, 0, 100, 80),
        );
```

> **Note:** confirm the `DamageObject.rects` element type/field names (`crates/yserver-core/src/server.rs` `DamageObject`) — adjust the tuple accessors if the stored rect type differs. If damage rects are clipped/merged by `accumulate_damage_to_state`, assert on the merged extent instead; read the helper (`damage_fanout.rs:213`) to confirm.

Add three per-arm tests modelled on the same fixture — one drives minor 8 (Composite, already covered above so this is the rename), one drives minor 10 (Trapezoids) **and** a second asserting minor 11 (Triangles), one drives minor 23 (CompositeGlyphs). Each sets `render_return_region` to a distinct known region and asserts exact damage; each also asserts **empty region ⇒ no damage** (set `render_return_region = vec![]`, drive the op, assert `damage.rects` unchanged/empty). Name them:
- `render_trapezoids_damages_returned_region`
- `render_triangles_damages_returned_region`
- `render_composite_glyphs_damages_returned_region`

For the traps arm, the RENDER body layout is: `op(1) pad(3) src(4) dst(4) mask_format(4) src_x(2) src_y(2)` then primitives (`process_request.rs:1682-1688`); use minor 10 with a ≥40-byte trap body (one trapezoid) or any ≥20-byte body — the backend double ignores the bytes and returns `render_return_region`. For glyphs use minor 23 with a non-empty `items` body (the arm gates on `!req.items.is_empty()`, `:1845`). For triangles use minor 11 with a ≥20-byte body.

> **Important:** these tests exercise the **core-side plumbing** (does the arm capture the return value and damage each rect?), independent of GPU clip logic. The backend double returns whatever `render_return_region` holds, so a bad return-capture in any arm fails its test even while Composite stays green — that's the point of covering all four arms.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver-core damages_returned_region render_composite_emits_damage 2>&1 | tail -30`
Expected: FAIL — the arms still call `accumulate_damage_full_to_state` (damages the whole 800×600 window, not the returned region), so the exact-region assertions fail; the new arm tests fail to find their capture path.

- [ ] **Step 3: Rewire the Composite arm**

In `process_request.rs`, the Composite arm currently (`:1662-1672`):

```rust
                let _ = backend.render_composite(
                    origin, req.op, host_src, host_mask, host_dst, req.src_x, req.src_y,
                    req.mask_x, req.mask_y, req.dst_x, req.dst_y, req.width, req.height,
                );
                if req.width > 0
                    && req.height > 0
                    && let Some(dst_drawable) =
                        state.resources.picture(req.dst).and_then(|p| p.drawable)
                {
                    let _dropped = accumulate_damage_full_to_state(state, dst_drawable);
                }
```

Replace with — capture the region and damage each rect:

```rust
                let painted = backend
                    .render_composite(
                        origin, req.op, host_src, host_mask, host_dst, req.src_x, req.src_y,
                        req.mask_x, req.mask_y, req.dst_x, req.dst_y, req.width, req.height,
                    )
                    .unwrap_or_default();
                if let Some(dst_drawable) =
                    state.resources.picture(req.dst).and_then(|p| p.drawable)
                {
                    for r in &painted {
                        let _dropped = accumulate_damage_to_state(
                            state, dst_drawable, r.x, r.y, r.width, r.height,
                        );
                    }
                }
```

- [ ] **Step 4: Rewire the Traps/Tris arm**

In the `10..=13` arm, the calls are `let _ = backend.render_trapezoids(...)` (`:1717`) / `let _ = backend.render_triangles_op(...)` (`:1730`), followed by the damage block (`:1744-1746`). Capture the region from whichever branch runs, then damage it:

```rust
                let painted = if minor == 10 {
                    backend
                        .render_trapezoids(
                            origin, op, host_src, host_dst, host_mask_fmt, src_x, src_y,
                            primitives, 0, 0,
                        )
                        .unwrap_or_default()
                } else {
                    backend
                        .render_triangles_op(
                            origin, minor, op, host_src, host_dst, host_mask_fmt, src_x, src_y,
                            primitives, 0, 0,
                        )
                        .unwrap_or_default()
                };
                if let Some(dst_drawable) = state.resources.picture(dst).and_then(|p| p.drawable) {
                    for r in &painted {
                        let _dropped = accumulate_damage_to_state(
                            state, dst_drawable, r.x, r.y, r.width, r.height,
                        );
                    }
                }
```

- [ ] **Step 5: Rewire the CompositeGlyphs arm**

In the `23..=25` arm, the call is `let _ = backend.render_composite_glyphs(...)` (`:1841`), followed by the damage block (`:1845-1850`). Replace:

```rust
                let painted = backend
                    .render_composite_glyphs(
                        origin, minor, req.op, host_src, host_dst, mask_fmt, host_gs, req.src_x,
                        req.src_y, &req.items, 0, 0,
                    )
                    .unwrap_or_default();
                if let Some(dst_drawable) =
                    state.resources.picture(req.dst).and_then(|p| p.drawable)
                {
                    for r in &painted {
                        let _dropped = accumulate_damage_to_state(
                            state, dst_drawable, r.x, r.y, r.width, r.height,
                        );
                    }
                }
```

Ensure `accumulate_damage_to_state` is imported in `process_request.rs` (check the `use` block — `accumulate_damage_full_to_state` is already used, and both live in `damage_fanout`; add `accumulate_damage_to_state` to that import if absent). If `accumulate_damage_full_to_state` becomes unused after these edits, remove it from the import to avoid a warning (it may still be used elsewhere in the file — grep first).

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p yserver-core damages_returned_region render_composite_emits_damage 2>&1 | tail -30`
Expected: PASS — all four arm tests (Composite, Trapezoids, Triangles, CompositeGlyphs) damage exactly the returned region, and empty region ⇒ no damage.

- [ ] **Step 7: Commit**

```bash
git add crates/yserver-core/src/core_loop/process_request.rs
git commit -m "feat(render): damage exactly the backend-returned clipList region

Composite/Trapezoids/Triangles/CompositeGlyphs now damage the region the
backend reports it painted (child-clipped) instead of the whole drawable.
Closes the RENDER over-damage class for these ops.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Avkmce8W4mDeLap2Y3x1Tb"
```

---

## Task 8: Full verification

- [ ] **Step 1: Format**

Run: `cargo fmt`
Expected: no diff (or only the new code reformatted).

- [ ] **Step 2: Clippy (plain — per `feedback_clippy_pedantic_default`)**

Run: `cargo clippy --all-targets 2>&1 | tail -30`
Expected: no new warnings. Fix any that the new code introduces (e.g. `clippy::needless_range_loop`, `too_many_lines` — add a scoped `#[allow]` only if it matches the existing style in these methods).

- [ ] **Step 3: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: all pass. Pay attention to the RENDER rendercheck-adjacent unit tests and the `compute_render_composite_clip_*` free-fn tests (unaffected) and the `clip_fill_rects_by_subwindow_mode_*` tests (unaffected — that path is untouched).

- [ ] **Step 4: (Recommended, not a landing blocker) bee/MATE visible smoke**

Per `feedback_commit_after_testing`, a paint-path + backend-signature change like this warrants a visible check even though the unit/plumbing tests prove the invariants. On bee: `just startx` into MATE, confirm (a) the mate-panel systray icons still render and do not drive a recomposite self-loop (the `FillRectangles` fix's sibling case — `[[project_tray_damage_self_loop]]`), and (b) xfce-decoration / titlebar buttons paint correctly (child-clipping must not clip away legitimate paint). Note the active GTK theme in any report (`feedback_gtk_theme_workload`).

---

## Self-Review

**Spec coverage:**
- Backend trait return-type change (spec §"Backend trait change") → Task 2. ✓ (type corrected to `RegionRect`.)
- `render_dst_cliplist_local` helper, local coords, 3-step order, picture-mode gating (spec §"Paint side") → Task 1 (helper) + Tasks 3–6 (per-op wiring, incl. per-op bbox: Composite dst rect, Traps/Tris primitive bbox, Glyphs quad union). ✓
- Damage side: 3 arms damage returned region (spec §"Damage side") → Task 7. ✓
- F1 (mode from picture record) → `dst_picture_clip_by_children` + its unit test (Task 1) + IncludeInferiors helper test. ✓
- F2 (paint/damage single region; manually-redirected child not subtracted) → single `cliplist_local` drives both; `render_dst_cliplist_skips_manually_redirected_child` test (Task 1). ✓
- RecordingBackend configurable region + explicit `render_triangles_op` override (spec §"Impls") → Task 2. ✓
- host_x11 returns empty (spec §"Impls") → Task 2. ✓
- v2_acceptance mechanical return-type update (spec §"Impls") → Task 2 Step 5 (expected: no edits; `.expect()` value dropped). ✓
- Pixmap destinations (no children) → `dst_local_extent` store branch + helper's `windows_v2.contains_key` gate. ✓
- Tests 1–6 (per-op child clip, IncludeInferiors, manually-redirected, src/mask fold, coord/offset, full-cover) → Task 1 helper tests cover the logic; Tasks 3–6 cover per-op return + the src/mask fold survives in Composite (Task 3 Step 5); coord/offset covered by `bbox_local = backing − offset` derivation (verified by unredirected acceptance tests). ✓
- Core plumbing: one test per arm (spec §"Core plumbing tests") → Task 7. ✓
- Op 22 (FreeGlyphs) untouched. ✓ (not in scope of any task.)

**Placeholder scan:** No "TBD"/"handle edge cases"; the two implementer-`Note`s (glyph seed helper, damage-rect field names) point at concrete existing code to copy and demand value verification, not hand-waving. The transitional `Ok(Vec::new())` in Task 2 is an explicit type-flip step superseded by Tasks 3–7, not a shipped stub.

**Type consistency:** `render_dst_cliplist_local` returns `Vec<Rectangle16>` (backend-local) throughout Tasks 1/3/4/5/6; `local_rects_to_region` converts to `Vec<xfixes::RegionRect>` at each op's `Ok(...)`; the trait + recording + host + core all speak `xfixes::RegionRect`; the core reads `r.x/r.y/r.width/r.height` which both `RegionRect` and the damage helper accept. `dst_picture_clip_by_children` → `bool` → helper's `clip_by_children` param, consistently. ✓

**Open item flagged for review:** damage deliberately excludes the src/mask picture-clip fold (Composite) — over-damage-safe, per the helper signature. If a reviewer wants damage to match painted pixels exactly under a src/mask clip, that's a follow-up (fold src/mask into the returned region too), not part of this plan's scope.
