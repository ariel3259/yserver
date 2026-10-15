# Atomic Cursor Only B: Commit Ownership and Window Entry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Execute inline; no subagents.

**Goal:** Couple HW↔SW changes to their actual submitted frames, generalize HW-only cursor intent, and remove the retirement-time #244 jump.

**Architecture:** Build on [plan A](2026-10-15-atomic-cursor-only-a.md)'s applied state, atomic deltas and framebuffer leases. Give each rendered frame an OS-neutral cursor plan and an acceptance receipt, carrying that plan across copied-render completion without publishing it prematurely. HW-only producers use the same per-CRTC stream; retirement releases resources and presentation bookkeeping, never issues cursor operations.

**Tech Stack:** Existing Rust workspace, yserver-render `OutputSink`, KMS scene/direct/cursor-only scheduler, full-state atomic sink, controlled events, existing Vulkan test renderer.

**Spec:** `docs/superpowers/specs/2026-10-15-atomic-cursor-only-design.md`, approved and binding despite its draft label; review rationale in `/home/ariel_santangelo/Projects/claude-scratch/atomic-cursor/spec-review-r{1,2,3,4}.md`.

## Global Constraints

- Prerequisite: all eight A tasks and their gates. Order A → B → [C](2026-10-15-atomic-cursor-only-c.md). One commit per task on `feat/atomic-cursor-only`; merge requires user confirmation. No implementation or git writes during drafting.
- “A frame that is not submitted (no BO, skipped, failed) publishes nothing, and its assignment is not applied by any other commit.” HW↔SW intent is scoped to the exact frame, not an output-global pending slot.
- HW-only motion/image/animation/hotspot/entry/exit belongs to cursor-only commits when idle; it can ride an unbuilt scene/direct commit or wait for its event. Held Vsync direct show/hide/animation/first bind must progress without a successor primary frame.
- “Per-output cursor mode (HW, SW, Mixed while outputs differ) changes when the commit that carried the transition is accepted, and its flip event retires it as today, so Mixed bookkeeping is kept.”
- “ShowOnRetire / HideOnRetire and their stored coordinates are removed. Every position is read when a commit is built.” SW cursor pixels still describe their rendered frame; do not reinterpret that SW snapshot as live HW position.
- “A commit carries only the cursor properties that differ from the applied state.” Equal work causes no ioctl or event obligation. Invalidated state sends the full state next time. Keep old framebuffer users until replacement retirement; blocking disable additionally requires the old-event drain.
- Full unscaled driver-size ARGB8888 buffers; cursor X/Y = output-local pointer − hotspot; hotspot-only updates change FB_ID; two initial buffers with at most max(2, CRTC count + 1). Never overwrite/remove a live buffer; coalesce newest under pressure.
- Keep A's eligibility and #246 matching. Physical hotspot-cap rejection does not disqualify; virtual drivers require accepted cap plus HOTSPOT_X/Y. No legacy fallback for ineligible outputs.
- Tearing On means detached intent; async commits have **zero** cursor properties. Do not add scaling/gamma/overlay/protocol policy. C10 owns the C1 rebase adaptation and final gate.
- No function-body `cfg(test)` branches, no new `YSERVER_*` behavior vars. Keep DRM/device/lease types out of portable backend and yserver-render. Update `SurfacePlatform` and test sink implementations when changing shared traits.
- Respect `tools/check-layout.sh`: 5,000 code / 6,000 test lines. Use focused `round_b*.rs` modules, not one enlarged atomic test file. Preserve glibc/musl/FreeBSD buildability.
- Rule B: all scenario progress through `Backend` or `core_loop::run_core`; no manual `scene.tick`, `display_cursor_by_handle`, commit builders, matching calls, fake in-flight flags or direct retirement helpers. Constructor injection and read-only observations are allowed. Inject no-BO/render/atomic failures at real dependencies, not branches that bypass the path under test.
- Rule A: inspect every accepted intermediate primary/cursor state; end through Backend hide/disable/drain with no actionable intent, staged leases, events or framebuffer leaks. Preserve expected live ownership before cleanup after injected failures.
- Rule F: 16 busy loops, each new non-GPU test 5 repetitions; GPU-waiting tests 20 repetitions. Use A's PID/trap stress procedure. Vulkan-fed scanout tests skip before submission on missing `supports_sync_fd()`, matching `crates/yserver/tests/common/mod.rs`; skipped cases are not passes. Use actual Vulkan rendering, not a fake successful export.
- Rule C: implementation never accesses hardware. Every task ends with the literal full workspace/lavapipe gate; `cargo +nightly fmt` precedes the required `cargo fmt --all`, and the exact default CI clippy command runs before the task commit.

- Module registration belongs to the same task and commit as each new file: test rounds modify `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/mod.rs`; platform modules modify `crates/yserver/src/kms/render/platform.rs`; cursor-plane submodules modify `crates/yserver/src/kms/cursor_plane.rs`; DRM modules modify `crates/yserver/src/drm/mod.rs`. Treat these exact parent paths as part of each applicable Files block.

## Review Focus

1. Two copied frames on one output complete out of order or after cancellation: no assignment or lease migrates between jobs; B1/B3 test this.
2. A frame renders HW but is rejected, while a cursor-only scheduler pass runs: it cannot steal that frame's assignment; B2/B3 test this.
3. A stationary direct cursor changes image/hotspot while no Present arrives: progress without unrelated damage or a hidden “visible plane” prerequisite; B4/B5 test this.
4. An output gap or unequal output heights changes footprint membership: attach/detach current HW peers and repaint only required SW peers; B6 tests this.
5. Window entry changes sprite, then motion arrives both before submission and before retirement: the complete operation trace must never restore the old position; B7 tests this.

---

## Production-path evidence

Grepped before defining these tests: `backend/trait_impl.rs:135–166,1834,1867` dispatches the input/scheduler/event/cursor operations; `portable/pointer.rs:520 → :500 → portable/cursor.rs:409` drives real window entry; animation calls display at `portable/cursor.rs:549`. `kms/scanout.rs:2183` calls compose; `platform/scene_sink.rs:132,158,237` reaches shared rendering, copied rendering and copied final submission; `platform/scanout_bos.rs:128` builds copied KMS requests. Direct/held paths have non-test callers at `kms/scanout.rs:524,899,1024,2127`, and direct cursor hooks at `kms/platform_hooks.rs:68–91`. `scene/flip.rs:198` calls the stale retirement transition today. These are A's P-pointer/P-scene/P-direct/P-cursor chains, not helper-only tests.

Before adding each B test, repeat `rg -n` for its named caller(s) under `crates/` and verify at least one non-test call. New functions below must be wired into these chains in the same task. The tests never advance them by direct invocation.

## File and interface decisions

- New neutral contract file: `crates/yserver-render/src/kms/render/scene/cursor_frame.rs`, re-exported by `scene.rs`. Define `CursorFrameId(u64)` with no reuse within one output lifetime; `CursorFrameAssignment::{Hardware, Software { pos: (i32,i32) }, Hidden}`; `CursorFramePlan { frame: CursorFrameId, assignment: CursorFrameAssignment, sprite: Option<CursorEntry> }`. No HW coordinates are stored in the plan. `CursorEntry` is the existing version/hotspot/bytes record and has no root position.
- `CursorAcceptedMode::{Hardware, Software, Hidden}` describes actual accepted content/ownership. `CursorPrimaryContent::{Cursorless, Software { pos: (i32,i32), record_version: u64 }}` describes accepted pixels; `CursorCommitReceipt { frame: Option<CursorFrameId>, mode: CursorAcceptedMode, primary_cursor: CursorPrimaryContent, needs_recompose: bool }`; `None` frame identifies cursor-only acceptance, never permission to apply a rendered frame assignment.
- Change `OutputSink::finish_frame(&mut self, output_idx: usize, bo_idx: usize, cursor: CursorFramePlan, record: &mut RecordFrame<'_>) -> FinishOutcome`. `FrameHandoff::FlipPending { submit: ComposeSubmit, cursor: CursorCommitReceipt }`; copied `RenderCompletion` handoff stays pending without acceptance. Change `OutputSink::submit_copied(&mut self, output_idx: usize, bo_idx: usize, fd: Option<OwnedFd>) -> io::Result<CursorCommitReceipt>`. Existing unavailable/error results remain unaccepted.
- `SceneCompositor::note_cursor_commit_accepted(&mut self, output_idx: usize, receipt: CursorCommitReceipt)` updates accepted mode and requests recompose when specified. `PendingAck` stores the receipt and rendered SW metadata; it contains no cursor operation. Retirement applies only presentation metadata and resource release. A pending transition still participates in aggregate Mixed bookkeeping.
- New KMS file `platform/cursor_frames.rs`: `stage_cursor_frame(&mut self, output_idx: usize, bo_idx: usize, plan: CursorFramePlan) -> io::Result<()>`; `cancel_cursor_frame(&mut self, output_idx: usize, bo_idx: usize, frame: CursorFrameId)`; `prepare_frame_cursor_commit(&mut self, output_idx: usize, bo_idx: usize, frame: CursorFrameId) -> io::Result<Option<PreparedCursorCommit>>`. Built leases belong to `(CrtcKey, bo_idx, frame)` and are released exactly once on cancellation/rejection or promoted on acceptance. A copied job retains that exact identity across the fd wait.
- KMS producer hook `Platform::queue_hw_cursor_sprite(b: &mut XBackend<Self>, record: &Arc<CursorRecord>)` uses the platform's existing associated-type seam; the surface implementation can decline HW and keep SW registration. `Platform::queue_hw_cursor_visibility(b: &mut XBackend<Self>, hidden: bool)` and the existing `move_hw_cursor` hook record current intent without committing a frame assignment. The platform stores latest pointer separately and reads it at atomic build.
- `platform/cursor_commit.rs` continues A's `PreparedCursorCommit`; add `CursorCommitOwner::{Scene(CursorFrameId), HardwareOnly, Lifecycle}` to preparation metadata. Owner classification is semantic: it cannot be changed simply because a scene frame failed or has no BO.

## Task B1: Carry cursor plans and receipts across the output seam

**Files:**
- Create: `crates/yserver-render/src/kms/render/scene/cursor_frame.rs`, `crates/yserver/src/kms/render/platform/cursor_frames.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_frames.rs`.
- Modify: `crates/yserver-render/src/kms/render/scene.rs`, `crates/yserver-render/src/kms/render/scene/sink.rs`, `crates/yserver-render/src/kms/render/scene/tick.rs`, `crates/yserver-render/src/kms/render/scene/flip.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/surface/platform.rs`; update all existing trait implementors, including `crates/yserver-render/src/kms/render/scene/tests/mod.rs`.

**Interfaces:**
- Consumes: A's prepared commits and lease roles; P-scene; existing `CursorEntry`, `ComposeSubmit`, `RecordFrame`.
- Produces: neutral types and `finish_frame`/handoff/receipt signatures above. This task carries metadata while retaining A's conservative behavior; it does not switch HW↔SW ownership until B2/B3.

- [ ] **Step 1: Add `atomic_cursor_only_frame_receipts_name_the_accepted_frame`.** Through Backend render/compose/events, create two distinguishable frames and inject a failed/unavailable handoff between them. Assert successful receipts identify exactly the submitted frame, unavailable/error paths publish none, TEST_ONLY publishes none, and no cancelled frame changes accepted cursor mode. Mutation: use an output-global “latest assignment” instead of the submitted frame's identity; the receipt assertion fails.
- [ ] **Step 2: Run red on missing/incorrect receipts.** Confirm the recorded primary framebuffer and frame identity agree; a state getter alone is insufficient.
- [ ] **Step 3: Thread the contract through KMS shared/copied and surface sinks.** Keep OS-native identifiers in KMS only. Surface paths continue producing truthful Software/Hidden receipts. A copied render handoff has no accepted receipt until its final KMS submit succeeds.
- [ ] **Step 4: Run green, the identity mutation and F stress.** Keep existing modes/retirement behavior working at this intermediate checkpoint.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "refactor: carry frame cursor plans and acceptance receipts"`. Do not batch this commit with the next task.

## Task B2: Put shared-frame HW↔SW handoffs in the frame commit

**Files:**
- Modify: `crates/yserver-render/src/kms/render/scene/tick.rs`, `crates/yserver-render/src/kms/render/scene/cursor.rs`, `crates/yserver-render/src/kms/render/scene/flip.rs`, `crates/yserver/src/kms/render/platform/cursor_frames.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_frames.rs`.

**Interfaces:**
- Consumes: B1 plan/receipt, A applied states/deltas and buffer leases; P-scene/P-cursor.
- Produces: shared route's stage/prepare/cancel methods and acceptance notification; `CursorCommitOwner::Scene(frame)` plans are invisible to cursor-only preparation.

- [ ] **Step 1: Add `atomic_cursor_only_shared_transition_is_one_primary_commit`, `atomic_cursor_only_no_bo_does_not_publish_assignment`, and `atomic_cursor_only_failed_frame_cannot_be_stolen_by_cursor_only`.** Use Backend definition/eligibility change to request SW→HW and HW→SW, inject an unavailable BO then a rejected frame, and run idle scheduling between retries. Assert accepted SW→HW contains a cursorless primary plus plane attach; accepted HW→SW contains SW pixels plus detach in the **same request**. Before acceptance, mode and primary stay old; after acceptance mode changes and retirement resolves Mixed metadata. No-BO and failed frames publish neither plan nor mode through cursor-only. Mutations: retain the two-phase cursorless HW→SW frame, apply mode before acceptance, or copy the scene plan to global HW-only intent; fail the content/owner trace assertions.
- [ ] **Step 2: Run red through Backend scheduling.** Use A1's `ScanoutSlotAllocator` dependency at real `acquire_scanout_bo` to inject no-slot, then availability; never set BO/in-flight state by hand.
- [ ] **Step 3: Switch only the shared route to frame-owned transitions.** Remove its retirement-time show/hide calls and stop stripping the SW draw on normal HW→SW. Sample HW position at final request build. Until C's detailed recovery arrives, reject/abandon a failed transition frame safely with its old primary and actual binding retained; do not accept a software-cursor frame beside bound HW. Recover and cancel staged leases on every no-device/no-slot/render/submit failure.
- [ ] **Step 4: Run green, every mutation and F stress.** Assert every accepted intermediate frame's primary SW content against the oracle's bound plane; inspect transition acceptance and event retirement separately.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "feat: couple shared frame cursor handoffs to atomic submission"`. Do not batch this commit with the next task.

## Task B3: Preserve copied-frame ownership across deferred submission

**Files:**
- Modify: `crates/yserver/src/kms/render/platform/cursor_frames.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/kms/render/platform/scanout_bos.rs`, `crates/yserver/src/kms/render/platform/flip_events.rs`, `crates/yserver-render/src/kms/render/scene/tick.rs`, `crates/yserver-render/src/kms/render/scene/flip.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_frames.rs`.

**Interfaces:**
- Consumes: B1 copied receipt contract, B2 shared staging/settlement; P-scene's real render-completion path.
- Produces: copied completion associates its job with `(CrtcKey,bo_idx,CursorFrameId)`; `submit_copied` returns a receipt only after final atomic acceptance. Cancellation invalidates a job without transferring its assignment elsewhere.

- [ ] **Step 1: Add `atomic_cursor_only_copied_frame_keeps_assignment_until_actual_submit` and `atomic_cursor_only_built_sprite_is_not_reused_before_submit`.** Drive Backend compose with a held render-completion dependency, move P0→P1 before signaling completion, and invoke `Backend::on_scanout_render_completion`. Assert no plane/mode publication before the final KMS request; that request uses P1 and the original frame's composition assignment. Keep a built sprite lease held, pressure the pool with newer records, and assert no write/RMFB until the frame submits or cancels. Exercise cancellation/removal, already-signalled completion, copy failure and two differently identified ready jobs; no stale job may attach or release another's lease. Mutations: publish at render acceptance, recalculate assignment from current global state, drop the built lease early, or match completions only by output index; each fails its boundary assertion.
- [ ] **Step 2: Run red with genuinely deferred Backend completion.** Do not call `submit_copied_scanout` or a page-flip builder directly.
- [ ] **Step 3: Make final copied submission use the frame plan and current HW position.** Preserve rendered SW pixels' snapshot. Route accepted/rejected/cancelled leases exactly once; honor source/sink device ownership. Keep a waiting transition out of the cursor-only lane even when final submission is skipped or fails.
- [ ] **Step 4: Run green, all mutations and the 20-repeat GPU stress.** Guard missing SYNC_FD before starting; record any skipped coverage as pending execution on a capable software/test device.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "feat: preserve copied frame cursor ownership until kms acceptance"`. Do not batch this commit with the next task.

## Task B4: Route every HW-only producer into current intent

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/seam.rs`, `crates/yserver/src/kms/render/backend/portable/cursor.rs`, `crates/yserver/src/kms/render/backend/portable/pointer.rs`, `crates/yserver/src/kms/render/backend/kms/platform_hooks.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs`, `crates/yserver/src/surface/platform.rs`, `crates/yserver/src/kms/render/platform.rs`, `crates/yserver/src/kms/render/backend/kms/session.rs`, `crates/yserver/src/kms/render/backend/kms/randr.rs`.
- Create: `crates/yserver/src/kms/render/platform/clock.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_producers.rs`.

**Interfaces:**
- Consumes: A full intent scheduler, B2/B3 frame-owner exclusion, P-pointer and portable animation non-test callers.
- Produces: `MonotonicClock::now(&self) -> Instant` with a production `SystemMonotonicClock` in `platform/clock.rs`; `Platform::monotonic_now(b: &XBackend<Self>) -> Instant` exposes it without platform types in portable code. Use it in production animation/debounce deadline creation and evaluation; test constructors inject a manually advanced clock.
- Produces: platform seam queue hooks above; `CursorCommitOwner::HardwareOnly`. HW sprite/visibility/motion intent is distinct from `CursorFramePlan`; no captured HW coordinates in pending frame metadata.

- [ ] **Step 1: Add `atomic_cursor_only_stationary_hw_image_and_animation_make_progress` and `atomic_cursor_only_hw_visibility_and_hotspot_need_no_scene_frame`.** Establish HW via accepted Backend frames, keep primary undamaged, then drive image replacement, animation deadline, hotspot-only change and hide/show through Backend. Assert current FB/hotspot/attach/detach deltas go via cursor-only when idle; no new primary required. While a real frame/direct flip is pending, coalesce newest data and wait; before a frame is built, let its commit carry the HW-only change without changing its composition owner. Mutation: keep the visible-plane/position-only guard, require scene damage for sprite upload, or reset animation pixels to an older record at retirement; each fails progress or identity assertions.
- [ ] **Step 2: Run red; advance animation via Backend scheduling and a controlled deadline dependency.** Do not call `tick_cursor_animation` or change `active_cursor_anim` fields to fake the path. Use the `MonotonicClock`/`Platform::monotonic_now` dependency defined here, with the same default `Instant::now` behavior; the test controls time, not animation state.
- [ ] **Step 3: Queue record bytes/hotspot/visibility before deciding whether a software sprite view is needed.** A HW-only update must not be silently blocked by the portable `display_cursor_by_handle` SW image-view readiness guard. Retain SW allocation/registration for outputs that paint the cursor. Queueing never authorizes a skipped scene transition. Hold newest pending data under pool pressure and wake on release.
- [ ] **Step 4: Run green, producer mutations and F stress.** Require one accepted operation per necessary delta and no idle retry loop after hidden/equal state settles.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "feat: queue all hardware cursor producers on the atomic stream"`. Do not batch this commit with the next task.

## Task B5: Service a held direct frame without a successor

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs`, `crates/yserver/src/kms/render/backend/kms/platform_hooks.rs`, `crates/yserver/src/kms/render/backend/portable/cursor.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_producers.rs`.

**Interfaces:**
- Consumes: B4 HW intent and A's per-CRTC arbitration; P-direct's first-bind, direct refresh, hidden hook and held scheduling callers.
- Produces: direct cursor hooks only queue HardwareOnly intent; first bind is driven by scheduler intent, never by a cursor ioctl at direct retirement. Existing direct-unflip request remains the fallback when the plane cannot take the sprite.

- [ ] **Step 1: Add `atomic_cursor_only_held_direct_services_show_hide_animation_and_first_bind`.** Through `Backend::try_present_direct`, establish one accepted held Vsync direct frame and retire it; send no successor. Exercise first attach (including initially hidden direct), stationary show, hide, image, animation and hotspot updates. Assert each necessary cursor-only commit appears, primary FB stays the held source, and event obligations serialize normally. Make the plane ineligible/oversized; assert direct unflip through normal composed recovery instead of SW pixels being invented in the direct primary. Mutations: rely on next Present, bind at direct retirement, keep the visible-only guard, or omit the hide producer; the corresponding no-successor trace fails.
- [ ] **Step 2: Run red through Backend direct admission/events.** The fixture must not enable `test_submit_direct_without_drm` or manually install a current direct frame.
- [ ] **Step 3: Remove synchronous direct bind/refresh/hide side effects.** Keep queued direct successor priority over cursor-only, and wait for every grouped CRTC before submitting an overlapping transaction. Mixed bookkeeping cannot by itself force an unnecessary unflip for a healthy HW-only update.
- [ ] **Step 4: Run green, direct producer mutations and F stress.** Teardown unflips/disable through Backend before source and cursor framebuffer cleanup.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "feat: drive held direct scanout cursor changes atomically"`. Do not batch this commit with the next task.

## Task B6: Preserve HW membership, Mixed mode and grouped unflip ownership

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/kms/platform_hooks.rs`, `crates/yserver/src/kms/render/backend/kms/scanout.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/drm/modeset.rs`, `crates/yserver-render/src/kms/render/scene/cursor.rs`.
- Create: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_outputs.rs`.

**Interfaces:**
- Consumes: B2/B3 acceptance receipts, B4 membership intent, B5 direct ownership; P-pointer/P-direct.
- Produces: output-local accepted cursor modes plus aggregate Mixed during pending or heterogeneous output states. HW footprint entry/exit while staying HW uses cursor-only attach/detach; actual HW↔SW composition changes remain frame-owned.

- [ ] **Step 1: Add `atomic_cursor_only_cross_output_membership_preserves_mixed_mode` and `atomic_cursor_only_direct_probe_and_grouped_unflip_keep_cursor_owners`.** Backend motion crosses HW/HW then HW/SW outputs with offsets, gaps, unequal heights, shared sprite and staggered events. Assert current local positions, HW-only attach/detach without a new primary, and SW peer repaint without replaying another output's transition. In a grouped direct successor/unflip, hold a cursor-only flip on one CRTC: group waits; after event, TEST_ONLY and actual direct requests carry the same applicable cursor deltas, but probing publishes no state. Rendered SW transitions are never attached to a retained composed framebuffer whose pixels do not match them; recompose first when necessary. Mutations: route by raw CRTC alone, classify mixed as all-HW, attach during an outstanding peer event, omit a cursor field in the probe, or consume a pending scene plan in unflip; each fails its trace/content assertion.
- [ ] **Step 2: Run red using Backend motion/direct/unflip entry paths.** No direct calls to request builders or mode classifiers.
- [ ] **Step 3: Replace the old membership-dirty scene wake for HW-only crossings with current intent.** Preserve necessary damage for SW peers, inverse transforms/current transform SW policy, and existing direct topology restrictions. Retained composed FBs need truthful content/receipt metadata; a group cannot publish a speculative assignment.
- [ ] **Step 4: Run green, all owner/membership mutations and F stress.** End with each output drained independently and all shared sprite users gone.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "fix: retain cursor ownership across outputs and grouped commits"`. Do not batch this commit with the next task.

## Task B7: Prove the #244 window-entry regression against master

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/portable/pointer.rs`, `crates/yserver/src/kms/render/backend/portable/cursor.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs` as needed for live-position production wiring.
- Create: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_jump.rs`.
- Modify: `docs/status.md` with reproduction evidence.

**Interfaces:**
- Consumes: A1 unified legacy+atomic operation trace, B3 controllable copied completion, B4 current-position intent; P-pointer's **actual** window hit test → crossing → refresh → sprite update.
- Produces: `atomic_cursor_only_window_enter_never_restores_p0`, a reproducible failure on base master and a pass with B's implementation. No transition-state seeding is part of the fixture.

- [ ] **Step 1: Build two mapped windows with distinct cursors through core requests or Backend window/cursor entries plus the authoritative server tree.** Establish the initial cursor through accepted frames. Warp/host-motion into the second window at P0=(120,80), defer the real frame's copied submission, move inside that window to P1=(180,130), release completion, hold retirement, then deliver the event. Record every bind/move/hide/atomic operation and accepted result from entry through final drain. Assert every post-motion cursor position operation uses P1 (adjusted by that sprite's hotspot/output origin), never P0, including during and after retirement. Cover a second schedule where motion lands after acceptance but before retirement. Assert the cursor XID/version actually changed through the enter path.
- [ ] **Step 2: Demonstrate red on master.** Apply only fixture/recording-seam changes to a controller-provided isolated checkout of `8b8b1364` (or a separately documented equivalent master with #244), without bringing the fix or changing cursor behavior. The complete trace must contain the stale legacy move to P0 after the P1 motion. Do not count a missing new interface, build failure, seeded HW state or assertion on atomic properties alone as reproduction. If this exact scenario does not fail, refine the production-path schedule before proceeding.
- [ ] **Step 3: Enforce current-position sampling for every HW request.** Never replay the x/y captured by a scene show request. Keep the same test stimulus and oracle on the implementation branch. Mutation: store P0 in pending HW transition data and apply it at retirement; both schedule variants must fail at the trace assertion. A second mutation omitting legacy trace observation must invalidate the master proof.
- [ ] **Step 4: Run green and 20-repeat GPU stress.** Record baseline revision, old/new trace excerpt and assertion location in status. Baseline reproduction belongs to future execution: this drafting task creates no checkout and performs no git writes.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "test: prove atomic cursor window entry does not jump on retirement"`. Do not batch this commit with the next task.

## Task B8: Remove retirement operations and close the ownership checkpoint

**Files:**
- Modify: `crates/yserver-render/src/kms/render/scene.rs`, `crates/yserver-render/src/kms/render/scene/cursor.rs`, `crates/yserver-render/src/kms/render/scene/tick.rs`, `crates/yserver-render/src/kms/render/scene/flip.rs`, `crates/yserver-render/src/kms/render/scene/lifecycle.rs`, `crates/yserver-render/src/kms/render/scene/sink.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/surface/platform.rs`, `docs/status.md`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_frames.rs` and relevant existing scene tests.

**Interfaces:**
- Consumes: B1 receipts and B2–B7 owners/trace coverage; the existing non-test `scene/flip.rs:198` and `scene/lifecycle.rs:470` legacy calls are the removal targets.
- Produces: no `CursorTransition`, `ShowOnRetire`, `HideOnRetire`, retire-time upload/show/hide or stored HW show coordinates. `PendingAck` and accepted mode remain truthful through receipt-only retirement. `CursorPlaneSink` retains eligibility queries but loses operation methods that only the old retirement path used.

- [ ] **Step 1: Add `atomic_cursor_only_retirement_is_cursor_operation_free` and `atomic_cursor_only_surface_cursor_stays_software_after_seam_change`.** Backend-driven scene/cursor/direct transitions accept and then retire while another cursor generation is queued; assert retirement adds zero cursor I/O and preserves newer intent, while releasing only its own old leases/metadata. Surface backend remains Software/Hidden with correct repaint. Mutations: keep a retirement operation, clear a newer generation on old ack, or incorrectly publish surface Hardware; each fails a receipt/log assertion.
- [ ] **Step 2: Establish the mutation red before deleting the old machinery.** Earlier B tasks may already make the new retirement test pass. Temporarily restore one old production retirement operation (and separately the old generation-clear behavior), verify the behavioral assertion fails, then revert the mutation. Observe operations, not merely type-name grep results.
- [ ] **Step 3: Delete the retired transition machinery and update all sink/test implementations.** Remove `force_show_retry_version` behavior that replays old records; C6 supplies final key/epoch qualification. Scene `drain_all` must not submit a hide after lifecycle ownership has already disabled the CRTC. Keep fail-closed intermediate recovery from B2/B3 until C installs the exact §3 retry policies. Update status to distinguish completed ownership/#244 work from remaining C recovery/reassignment/removal.
- [ ] **Step 4: Run green, mutations and F stress, then `rg -n 'ShowOnRetire|HideOnRetire' crates`.** Expected: no matches. Verify all B behavior works without any retirement adapter; C is refinement/completion, not a prerequisite for displaying cursors safely.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "refactor: retire cursor metadata without cursor operations"`. Do not batch this commit with the next task.

## Spec coverage table

This is the complete cross-plan ledger, repeated so this plan can be reviewed independently. Task IDs prefixed **B** are owned here; other prefixes refer to the ordered sibling plans. A/B checkpoints are working increments; final ownership/error/deletion requirements are complete only after their listed later tasks. Each §1–§4 bullet/sub-bullet and every numbered §4 test has an explicit owner; §5 is included for the final controller task.

| Spec item | Required invariant / evidence | Task(s) |
|---|---|---|
| Goal / non-goals | Atomic-only HW, SW fallback, #244 fix; retain gamma/overlay/scaling/protocol boundaries | A3, A5, B7, C10 |
| §1 bullet 1 | Per-CRTC #246 assignment; all ten required FB/CRTC/SRC properties; open + every topology change | A2, A3, C8 |
| §1 bullet 2 | Atomic then hotspot cap before enumeration; Linux 6.8 historical fact; physical rejection normal | A2, C11 |
| §1 bullet 3 | Four virtual names; exposed plane + rejected cap => SW; accepted cap AND both hotspots; feature/backport detection | A2, A3 |
| §1 bullet 4 | No cursor planes / Apple DCP / unmatched CRTC => SW | A3, C10 |
| §1 bullet 5 | Delete legacy show/hide/move ioctls and non-atomic fallback branches | A3, A5, C10 |
| §2.1 bullet 1 | Per-CRTC intent versus last accepted applied FB/position/hotspot/detached state | A5, A6, B1, C1 |
| §2.1 bullet 2 | Only changed properties; equal/coalesced B→C→B/integer-equal work consumed without ioctl or event | A6 |
| §2.1 bullet 3 | Invalidate after master loss/resume, every modeset, every reassignment; next request full | A8, C7, C8, C9 |
| §2.1 reassignment parent | Never directly switch bound plane A→B; accepted detach before separate attach | C8, C9 |
| §2.1 reassignment disabled A | Source detach rides its blocking lifecycle disable | A5, C7, C9 |
| §2.1 reassignment active A | Source detach rides scene frame painting SW; no-BO/skipped/rejected frame retains A HW | B2, B3, C8 |
| §2.1 reassignment final | Drain A outstanding event before B attach; B SW until attach accepted; no transfer in one request | C8, C9 |
| §2.2 owner row 1 | HW↔SW belongs to exact frame that changes SW pixels; missing/skipped/failed frame publishes nothing | B1, B2, B3 |
| §2.2 owner row 2 | HW-only motion/image/animation/hotspot/HW-output entry/exit; idle cursor-only, ride unbuilt request or await event | A6, B4, B6 |
| §2.2 owner row 3 | Held Vsync direct show/hide/animation/first bind without successor; unavailable plane unflips | B4, B5 |
| §2.2 owner row 4 | DPMS/VT/output removal detaches in same blocking disable | A5, A8, C7 |
| §2.2 mode bullet | Mode changes at acceptance; flip retirement and heterogeneous/pending Mixed bookkeeping retained | B1, B2, B3, B6, B8 |
| §2.2 retirement bullet | Remove ShowOnRetire/HideOnRetire/stored HW coordinates; every HW position sampled at build | A6, B3, B7, B8, C10 |
| §2.3 bullet 1 | Per-device cap-size dumb buffers; ARGB8888 AddFB2; top-left transparent padding; full-buffer unscaled SRC/CRTC; pointer−hotspot | A4, A5 |
| §2.3 users: on-screen | Old FB remains a user until its replacing flip event, not acceptance | A5, A7 |
| §2.3 users: submitted | Submitted in-flight new and old references cannot be reclaimed | A5, A7, C7 |
| §2.3 users: built | Built but unsent request retains its sprite across deferred submission/cancellation | A4, A5, B3 |
| §2.3 reclaim paragraph | Write/RMFB only at all-user zero across outputs; blocking return AND old-event drain | A7, A8, C7, C9 |
| §2.3 image/hotspot bullet | Image and hotspot-only change use free buffer and switch FB_ID for virtio host update | A7, B4, B5, C11 |
| §2.3 pressure bullet | Newest wanted pending, older pending dropped, wake at free user; two initial / one-per-CRTC-plus-one bound | A4, A7, B3, B4 |
| §2.4 bullet 1 success | One blocking lifecycle disable+detach; applied detached; last-user reclaim after old events, no replacing event | A5, A8, C7 |
| §2.4 bullet 1 failure | Failed disable preserves previous ownership/resources and current error path | A5, A8, C7, C9 |
| §2.4 bullet 2 | Resume/modeset starts invalidated | A8, C7 |
| §2.5 | Tearing On detached; async requests cursor-free; second merger adapts C1 rebase | C10 |
| §3 attribution bullet | Rejected cursor-carrying frame TEST_ONLY full vs without cursor fields | A5, C1 |
| §3 cursor-only EINVAL | Applied unchanged; SW-pending; explicit detach acceptance before SW/recompose; rejected detach retains HW/log once | C2 |
| §3 frame HW→SW hide | Retry SW frame with explicit detach; if rejected, abandon frame/retain old primary+HW; cursorless replacement without cursor fields | C3 |
| §3 frame SW→HW show | Retry cursorless primary without cursor props; one cursorless accepted frame; SW ownership + immediate SW recompose | C4 |
| §3 frame HW-only riding | Retry primary without cursor props then cursor-only-style detach recovery | C5 |
| §3 retry fails | All rejected retry variants preserve current flip error paths and resource ownership | C1, C3, C4, C5 |
| §3 EBUSY | #244 event wait, never cursor failure, no fake event obligation | A6, C1 |
| §3 requalification | (width,height,hotspot) key; no pixel/animation requal; once per new key per CRTC/epoch; rejected revisits wait topology | C6 |
| §4 rules A/B/C/F | Backend/run_core only, full trace and independent leak oracle; final controller hardware; 5x/16 loops, GPU 20x | A1 and every task gate; C11 |
| §4 test 1 | Show/hide/image/motion correct full FB/CRTC/SRC/HOTSPOT and frame-coupled show/hide | A5, A6, B2, B3, B4 |
| §4 test 2 | #244 real window entry, deferred submission/retirement, P0→P1 whole atomic+legacy trace, demonstrated master red | A1, B7 |
| §4 test 3 | Capability matrix including rejected cap AND exposed virtual plane and eligible physical rejection | A2, A3 |
| §4 test 4 | Two outputs staggered, skipped output, exhausted animation; fail first-event-release and acceptance-release mutations | A7, B3 |
| §4 test 5 | Bound plane A→B ordering/B SW; active A no-BO retains HW; detach coupled to SW frame | C8, C9 |
| §4 test 6 | Held direct no-successor all producers/first bind; skipped frame no publication; cross-output and Mixed | B2, B3, B5, B6 |
| §4 test 7 | DPMS/VT/removal disable+detach/drain, failure ownership, full resume state | A8, C7 |
| §4 test 8 | B→C→B and integer-equal no ioctl/event obligation | A6 |
| §4 test 9 | Hotspot-only switches FB identity | A7, B4, C11 |
| §4 test 10 | Every §3 case and every accepted intermediate frame; rejected hide/no double; failed show cursorless then SW; animation retry bound | C1, C2, C3, C4, C5, C6 |
| §4 test 11 | No legacy ioctl name under crates; CI grep detects return of dead/comment/test references too | C10 |
| §5 NVIDIA | Approved card1 motion/drag/animation, zero core stalls and #244 EBUSY comparison | C11 |
| §5 AMD | Approved card0 same + visible Vsync direct + Plasma X11 drags | C11 |
| §5 virtio | Approved ≥6.8 VM host/guest hotspot; explicitly pending if VM unavailable | C11 |

