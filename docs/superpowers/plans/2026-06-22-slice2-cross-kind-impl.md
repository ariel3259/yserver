# Slice 2 Cross-Kind Render-Pass Session — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Hold one `begin_rendering` open across consecutive same-dst ops of any kind in the frame-builder close-replay, eliminating the per-op tile store/load that costs ~34% of render passes on tilers (the #48 menu-hover lag).

**Architecture:** Refactor every `emit_recorded_*_into_cb` to a uniform `open / draws / close` split around two shared helpers (`open_dst_color_pass` / `close_dst_color_pass`), then add a `DstPassSession` to the replay loop that opens once, replays each eligible same-dst op's draws-half, and closes lazily on a hazard. Mirrors Mesa GL's implicit FBO batching. Design + codex review: `docs/superpowers/plans/2026-06-22-slice2-cross-kind-renderpass-session.md`.

**Tech Stack:** Rust, ash (Vulkan), dynamic rendering (`cmd_begin_rendering`/`cmd_end_rendering`), `cmd_pipeline_barrier2`.

**Gate (load-bearing, from CLAUDE.md):** render changes are NOT committed before hardware smoke. Phase 1 (pure refactor) is verifiable by `cargo test` + Vulkan validation layers and is HW-pixel-identical; Phases 2–5 change pass boundaries and are correctness-by-eye — each ends at "build+test+validation-layer green", and the COMMIT waits on HW A/B on eiger via `just yserver-xfce-hw-telemetry`.

---

## File Structure

- `crates/yserver/src/kms/v2/engine.rs` — Modify. Add the two shared pass helpers near the emit fns (~`engine.rs:8300`); in Phase 1 refactor ONLY `emit_recorded_fill_rect_into_cb` (9249) and `emit_recorded_logic_fill_into_cb` (9355) to `open + draws + close`. The glyph paths (`emit_recorded_image_text_into_cb` 9473 → `record_text_run`; `CompositeGlyphs` arm 8331 → `record_text_run_scissored`) own their `begin_rendering`/`end_rendering` inside `text.rs` and CANNOT be split until Phase 4 (codex round-1) — they stay standalone through Phase 3. Add `DstPassSession` + its pure step fn near `coalescing_counts` (~`engine.rs:250`); rewire the replay loop (~`engine.rs:2000`).
- `crates/yserver/src/kms/vk/ops/render.rs` — Reference only in Phase 1 (the composite open/draws/close at :214/:282/:385 is the template; do not change in Phase 1).
- `crates/yserver/src/kms/vk/ops/text.rs` — Modify in Phase 4 only (split `record_text_run_scissored` into a draws-half; today it owns begin/end — codex §2).

---

## Phase 1 — Uniform open/draws/close refactor for FILL + LOGIC_FILL only (NO merging)

**Scope (codex round-1):** Phase 1 refactors ONLY `fill_rect` and `logic_fill`. Composite is already split in `render.rs` (left untouched). The glyph/image_text paths own their pass inside `text.rs` and are deferred to Phase 4. So `open_dst_color_pass`/`close_dst_color_pass` are exercised by fill/logic in Phase 1 and by the session in Phase 2+.

**Rendering-identical, NOT telemetry-identical (codex round-1):** the emitted Vulkan *rendering* commands and order are unchanged for fill/logic. But fill/logic today emit `cmd_begin_rendering`/`cmd_set_viewport` WITHOUT `vk_count!` macros (engine.rs:9314/9325), whereas the helper counts them (matching composite's `render.rs` open). So after Phase 1 the `vk call rate` `begin_rendering`/`viewport` counters RISE — this is a counter FIX (fill/logic passes were previously uncounted, so our measured `begin_rendering` rate *undercounts* real passes), not a regression. Consequence: telemetry A/B must compare Phase-1-baseline vs later phases, NOT pre-Phase-1 captures. State this in the Phase 1 commit message.

**Do NOT unify per-kind pre-barrier access masks** (codex round-2 of design) — `open_dst_color_pass` takes the producer `src_access` as a parameter; fill/logic pass their CURRENT superset mask. Mask unification with composite's narrow mask is Phase 3.

Regression guard = existing 563 lib tests stay green + validation layers clean + HW pixel-identical for fill/logic.

### Task 1: Shared pass helpers

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs` (add near the emit fns, after `emit_recorded_op_into_cb`)

- [ ] **Step 1: Add `open_dst_color_pass` + `close_dst_color_pass`**

```rust
/// Open a dynamic-rendering color pass on `dst`: pre-barrier from
/// `old_layout` → COLOR_ATTACHMENT_OPTIMAL with the caller's producer
/// `src_access` mask (kept per-kind — fill/logic pass the superset), then
/// `cmd_begin_rendering` (LOAD/STORE, full-extent render area) + viewport.
/// Does NOT bind a pipeline or scissor — those are per-op (draws half).
/// Emits the SAME rendering commands+order as the fill/logic open
/// prologues (engine.rs:9268-9325 / 9380-9424) — the only difference is
/// that this counts `begin_rendering`/`set_viewport` via `vk_count!`,
/// which the inline fill/logic code does NOT today (telemetry fix, see
/// Phase 1 header). It is NOT a drop-in for composite's open
/// (`render.rs:214`), which also binds the pipeline + counts it; composite
/// keeps using its own `render.rs` open in Phase 1.
fn open_dst_color_pass(
    vk: &crate::kms::vk::VkContext,
    cb: vk::CommandBuffer,
    dst_image: vk::Image,
    dst_view: vk::ImageView,
    dst_extent: vk::Extent2D,
    old_layout: vk::ImageLayout,
    src_access: vk::AccessFlags2,
) {
    barrier_to_layout(
        &vk.device,
        cb,
        dst_image,
        old_layout,
        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        vk::PipelineStageFlags2::ALL_COMMANDS,
        src_access,
        vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
        vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
    );
    let render_area = vk::Rect2D {
        offset: vk::Offset2D::default(),
        extent: dst_extent,
    };
    let color_attachment = [vk::RenderingAttachmentInfo::default()
        .image_view(dst_view)
        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .load_op(vk::AttachmentLoadOp::LOAD)
        .store_op(vk::AttachmentStoreOp::STORE)];
    let rendering_info = vk::RenderingInfo::default()
        .render_area(render_area)
        .layer_count(1)
        .color_attachments(&color_attachment);
    #[allow(clippy::cast_precision_loss)]
    let viewport = [vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: dst_extent.width as f32,
        height: dst_extent.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    }];
    unsafe {
        crate::vk_count!(cmd_begin_rendering);
        vk.device.cmd_begin_rendering(cb, &rendering_info);
        crate::vk_count!(cmd_set_viewport);
        vk.device.cmd_set_viewport(cb, 0, &viewport);
    }
}

/// Close a pass opened by `open_dst_color_pass`: `cmd_end_rendering` +
/// post-barrier COLOR_ATTACHMENT_OPTIMAL → SHADER_READ_ONLY_OPTIMAL.
/// Emits exactly the commands the per-kind close halves emit today.
fn close_dst_color_pass(vk: &crate::kms::vk::VkContext, cb: vk::CommandBuffer, dst_image: vk::Image) {
    unsafe {
        crate::vk_count!(cmd_end_rendering);
        vk.device.cmd_end_rendering(cb);
    }
    barrier_to_layout(
        &vk.device,
        cb,
        dst_image,
        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
        vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        vk::PipelineStageFlags2::FRAGMENT_SHADER,
        vk::AccessFlags2::SHADER_SAMPLED_READ,
    );
}
```

- [ ] **Step 2: Build**

Run: `cargo build --release --bin yserver`
Expected: compiles (helpers unused yet → `#[allow(dead_code)]` if clippy complains, removed when wired in Task 2).

- [ ] **Step 3: Commit**

```bash
git add crates/yserver/src/kms/v2/engine.rs
git commit -m "refactor(v2): add open/close dst-color-pass helpers (Slice 2 phase 1)"
```

### Task 2: Refactor `emit_recorded_fill_rect_into_cb` to use the helpers

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs:9249`

- [ ] **Step 1: Replace the inline pre-barrier + begin_rendering + end_rendering + post-barrier** with `open_dst_color_pass(... src_access = SHADER_SAMPLED_READ | TRANSFER_WRITE | COLOR_ATTACHMENT_WRITE ...)`, the existing scissor+`cmd_clear_attachments` body, then `close_dst_color_pass(...)`. The `cmd_set_scissor(render_area)` + `cmd_clear_attachments` stay (the draws half). Resolve `vk` via `inner.vk.clone()` (the existing pattern) to avoid aliasing `&inner.vk` against `&mut`.

- [ ] **Step 2: Run tests** — `cargo test -p yserver --lib` → 563 pass.
- [ ] **Step 3: Build release** — `cargo build --release --bin yserver`.
- [ ] **Step 4: Commit** — `git commit -am "refactor(v2): fill_rect emit via pass helpers (byte-identical)"`.

### Task 3: Refactor `emit_recorded_logic_fill_into_cb` (engine.rs:9355)

- [ ] Same transformation: `open_dst_color_pass(src_access = superset)` → existing `bind_pipeline` + per-rect scissor/push/draw loop → `close_dst_color_pass`. The viewport is now set by the helper; remove the duplicate `cmd_set_viewport` from the body. Verify the bind_pipeline still happens AFTER begin_rendering (it does — body runs after the helper).
- [ ] `cargo test -p yserver --lib` → 563 pass; build release; commit.

### Task 4: (Deferred to Phase 4) glyph + image_text

`emit_recorded_image_text_into_cb` calls `record_text_run` → `record_text_run_scissored` (text.rs:97/110), which OWNS its `begin_rendering`/`end_rendering`. Likewise the `CompositeGlyphs` arm. Both CANNOT be split until `text.rs` is split (Phase 4, Task 9). No Phase 1 work; leave both standalone. Add a one-line `// SLICE2: glyph pass-split deferred to Phase 4` comment at engine.rs:8331 and :9473 so the deferral is discoverable in-code.

### Task 5: Phase-1 validation gate

- [ ] **Step 1:** `cargo +nightly fmt`
- [ ] **Step 2:** `cargo clippy -p yserver --lib` → 0 warnings.
- [ ] **Step 3:** `cargo test -p yserver --lib` → 563 pass.
- [ ] **Step 4 (HW-gated, hand to user):** Vulkan validation layers clean on ynest under menu-hover + drag; HW pixel-identical on eiger. Until then, Phase 1 is build/test-green but UNCOMMITTED-as-verified per the gate. Report to user for smoke.

---

## Phase 2 — `DstPassSession` scaffold + same-dst fill merge (first real merge)

### Task 6: `DstPassSession` + pure step fn + unit tests

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs` (near `coalescing_counts`, ~250)
- Test: same file, `mod tests::session`

- [ ] **Step 1: Write failing tests** for a pure session-decision fn that says, per op, whether to `Close` first, then `Open`-or-`Continue`, then (after loop) a final close. Reuse the `CoalesceClass`-style classification.

```rust
#[derive(Debug, PartialEq, Eq)]
enum SessionStep { OpenNew, Continue, FlushThenStandalone, FlushThenOpenNew }

// session_step(open_dst: Option<DrawableId>, op: &CoalesceClass) -> (SessionStep, Option<DrawableId> /*new open_dst*/)
#[test]
fn first_eligible_op_opens() {
    let (s, open) = session_step(None, &comp(1));
    assert_eq!(s, SessionStep::OpenNew);
    assert_eq!(open, Some(DrawableId::for_tests(1)));
}
#[test]
fn same_dst_eligible_continues() {
    let (s, open) = session_step(Some(DrawableId::for_tests(1)), &comp(1));
    assert_eq!(s, SessionStep::Continue);
    assert_eq!(open, Some(DrawableId::for_tests(1)));
}
#[test]
fn different_dst_flushes_then_opens() {
    let (s, _) = session_step(Some(DrawableId::for_tests(1)), &comp(2));
    assert_eq!(s, SessionStep::FlushThenOpenNew);
}
#[test]
fn ineligible_op_flushes_then_standalone() {
    // NonPass / traps / layout_transition / copy
    let (s, open) = session_step(Some(DrawableId::for_tests(1)), &CoalesceClass::NonPass);
    assert_eq!(s, SessionStep::FlushThenStandalone);
    assert_eq!(open, None);
}
```

- [ ] **Step 2:** Run → fail (fn undefined).
- [ ] **Step 3:** Implement `session_step` mirroring `coalescing_counts`'s eligibility. ELIGIBLE: fold-clean composite, fill, logic_fill (glyph only after Phase 4). INELIGIBLE → flush: traps, copy_area, **masked_copy_area** (codex round-1 — it's in the enum at frame_builder.rs:907 + dispatch at engine.rs:8407), put_image, glyph_upload, clip_snapshot_refresh, **layout_transition** (codex round-2 — transitions an arbitrary drawable incl. the open dst), and any `NonPass`. Self-sample / different dst / dst-read ⇒ flush. Reuse the `CoalesceClass` carrier; extend `classify_recorded_op` to flag eligibility explicitly. Add a unit test asserting `masked_copy_area` and `layout_transition` both yield `FlushThenStandalone`.
- [ ] **Step 4:** Run → 4 new pass + existing 12 coalescing pass.
- [ ] **Step 5:** Commit (pure logic, no GPU change) — `git commit -am "feat(v2): DstPassSession decision fn + tests (Slice 2 phase 2)"`.

### Task 7: Wire the session into the replay loop for FILLS ONLY

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs:~2000` (the `for op in &open_frame.ops` record pass)

- [ ] **Step 1:** Thread `let mut session: Option<DstPassSession> = None;`. For each op: classify; consult `session_step`; on `Flush*` call `close_dst_color_pass(&vk, cb, session.dst_image)` and clear `session`; on `OpenNew`/`FlushThenOpenNew` call `open_dst_color_pass(vk, cb, fr.dst_image, fr.dst_image_view, fr.dst_extent, fr.dst_old_layout, FILL_SRC_ACCESS)` — **the open MUST pass the current op's recorded `fr.dst_old_layout`/`lf.dst_old_layout`** (codex round-1), not a constant — and set `session = Some(DstPassSession{ dst_id, dst_image, dst_view, dst_extent })`; on `Continue` (or just after an open) call the fill/logic **draws-half only** (fill: `set_scissor(render_area)` + `cmd_clear_attachments`; logic: bind_pipeline + per-rect scissor/push/draw — no open/close, no extra viewport). All ops the session marks `FlushThenStandalone` keep going through the unchanged `emit_recorded_op_into_cb` standalone path. After the loop, if `session` is open call `close_dst_color_pass`.
- [ ] **Step 2:** No env flag — the merge IS the behavior. A/B is this branch vs `master` (or `git revert` of the commit); the feature branch already provides instant revert. Do NOT add a `YSERVER_*` kill-switch (see `feedback_no_feature_kill_switches`).
- [ ] **Step 3:** `cargo test -p yserver --lib` → all pass.
- [ ] **Step 4 (HW-gated):** validation layers clean on ynest; HW A/B (this branch vs master) on eiger — `cross_kind`/`begin_rendering`/`barrier2` drop on fill-heavy seconds, NO visual regression. Commit only after smoke.

---

## Phase 3 — Composite + logic_fill into the session

### Task 8: Extend eligibility to composite (fold-clean) + logic_fill

- [ ] In the session loop, when the op is a fold-clean composite or a logic_fill to the open session's dst, call its draws-half (composite: `record_render_composite_draws`; logic_fill: its bind+loop) instead of standalone. A solid-clear composite (`dirty_clear`) flushes, runs its pre-pass clear, opens a NEW session (codex: clear is pre-pass, illegal mid-pass). Now widen the shared open barrier to the superset mask uniformly (this is where the Phase-1-deferred mask unification lands — codex gap #2 — because cross-kind is intentionally on).
- [ ] `cargo test` green; **HW-gated** validation + eiger A/B; commit after smoke.

---

## Phase 4 — Glyph into the session

### Task 9: Split `record_text_run_scissored` (text.rs) into open/draws/close

- [ ] Per codex §2: hoist its `old_layout` capture, both barriers, `cmd_begin_rendering`, viewport, `cmd_end_rendering`, and final `set_current_layout` OUT; keep pipeline bind, descriptor bind, scissor, push constants, draws in a `record_text_run_scissored_draws` half. Keep `record_text_run_scissored` as a wrapper = open+draws+close for the standalone path.
- [ ] `cargo test` green (text behaviour identical standalone).

### Task 10: Fold glyph draws into the session

- [ ] In the session loop, a same-dst `CompositeGlyphs` calls the draws-half. Glyph uploads (`RecordedOp::GlyphUpload`) are non-pass → already flush the session (uploads are transfer, must precede the pass; verify append-order puts uploads before the glyph draw).
- [ ] `cargo test` green; **HW-gated** validation + eiger A/B (menu-hover is glyph-dense — the key #48 second); commit after smoke.

---

## Phase 5 (optional) — Traps tail

### Task 11: Let a trap's dst-composite half rejoin a session

- [ ] Traps stay a flush boundary for the mask-raster pass (different attachment), but after the mask raster, the dst-composite half MAY open/continue a session. Only do this if Phase 3/4 eiger data shows residual `cross_kind` worth it. Otherwise leave traps as a hard boundary (codex: safe first cut, not required).

---

## Self-Review notes

- **Spec coverage:** Phase 1 covers the design's "split each kind" + "byte-for-byte" (codex gap #2). Phase 2–4 cover §2 session + §3 eligibility incl. `layout_transition` (codex gap #1, handled as ineligible→flush in `session_step`). §5 layout bookkeeping is preserved because the session only omits intermediate barriers and always closes at SHADER_READ on a hazard/end.
- **Type consistency:** `open_dst_color_pass` / `close_dst_color_pass` / `session_step` / `DstPassSession` names used consistently across tasks.
- **A/B without a flag:** compare this branch vs `master` on HW; no runtime kill-switch. Record the commit SHA with each capture so a run is attributable to a known state.
