# Atomic Cursor Only B: Commit Ownership and Window Entry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Execute inline; no subagents.

**Goal:** Couple HW↔SW changes to their actual submitted frames, generalize HW-only cursor intent, and remove the retirement-time #244 jump.

**Architecture:** Build on [plan A](2026-10-15-atomic-cursor-only-a.md)'s applied state, atomic deltas and framebuffer leases. Give each rendered frame an OS-neutral cursor plan and an acceptance receipt, carrying that plan across copied-render completion without publishing it prematurely. HW-only producers use the same per-CRTC stream; retirement releases resources and presentation bookkeeping, never issues cursor operations.

**Tech Stack:** Existing Rust workspace, yserver-render `OutputSink`, KMS scene/direct/cursor-only scheduler, full-state atomic sink, controlled events, existing Vulkan test renderer.

**Spec:** `docs/superpowers/specs/2026-10-15-atomic-cursor-only-design.md`, approved and binding despite its draft label; review rationale in `/home/ariel_santangelo/Projects/claude-scratch/atomic-cursor/spec-review-r{1,2,3,4}.md`.

**Revalidation baseline:** joske/master `9524c80d`, including C1 `653d7801` (#249), inspected on `feat/atomic-cursor-only`. Paths below are repository-relative; abbreviated `kms/*.rs` in evidence means `crates/yserver/src/kms/render/backend/kms/*.rs`, `portable/*.rs` means `crates/yserver/src/kms/render/backend/portable/*.rs`; `platform/*.rs` means `crates/yserver/src/kms/render/platform/*.rs`; `drm/*.rs` means `crates/yserver/src/drm/*.rs`; and `scene/*.rs` means `crates/yserver-render/src/kms/render/scene/*.rs`. Symbol anchors describe the current code; new interfaces are explicitly task-produced. C1 is already merged, so its adaptation is required in each affected task, with C10 auditing the result. No implementation, git writes, hardware runs or new checkout are authorized by this plan revision.

## Global Constraints

- Prerequisite: all eight A tasks and their gates. Order A → B → [C](2026-10-15-atomic-cursor-only-c.md). One commit per task on `feat/atomic-cursor-only`; merge requires user confirmation. No implementation or git writes during drafting.
- “A frame that is not submitted (no BO, skipped, failed) publishes nothing, and its assignment is not applied by any other commit.” HW↔SW intent is scoped to the exact frame, not an output-global pending slot.
- HW-only motion/image/animation/hotspot/entry/exit belongs to cursor-only commits when idle; it can ride an unbuilt scene/direct commit or wait for its event. Held Vsync direct show/hide/animation/first bind must progress without a successor primary frame.
- “Per-output cursor mode (HW, SW, Mixed while outputs differ) changes when the commit that carried the transition is accepted, and its flip event retires it as today, so Mixed bookkeeping is kept.”
- “ShowOnRetire / HideOnRetire and their stored coordinates are removed. Every position is read when a commit is built.” SW cursor pixels still describe their rendered frame; do not reinterpret that SW snapshot as live HW position.
- “A commit carries only the cursor properties that differ from the applied state.” Equal work causes no ioctl or event obligation. Invalidated state sends the full state next time. Keep old framebuffer users until replacement retirement; blocking disable additionally requires the old-event drain.
- Full unscaled driver-size ARGB8888 buffers; cursor X/Y = output-local pointer − hotspot; hotspot-only updates change FB_ID; two initial buffers with at most max(2, CRTC count + 1). Never overwrite/remove a live buffer; coalesce newest under pressure.
- Keep A's eligibility and #246 matching. Physical hotspot-cap rejection does not disqualify; virtual drivers require accepted cap plus HOTSPOT_X/Y. No legacy fallback for ineligible outputs.
- Tearing On means detached intent; async commits have **zero** cursor properties. Do not add scaling/gamma/overlay/protocol policy. C1 is merged: A5/A6 and B2/B4/B5/B6/B8 own the adaptation where they change producers/consumers; C10 is the final audit, never the first integration step. Detached intent does not assert that a failed detach removed an actually bound plane; keep that actual ownership and Vsync safety.
- No function-body `cfg(test)` branches, no new `YSERVER_*` behavior vars. Keep DRM/device/lease types out of portable backend and yserver-render. Update `SurfacePlatform` and test sink implementations when changing shared traits.
- Preserve the surface backend's native host cursor. The neutral Hardware mode includes a cursor owned by the host window system, with Cursorless primary content; Software describes pixels actually drawn into a primary. Cursor motion on a surface continues to require neither composition nor repeated `ShellUi::set_cursor` calls.
- Respect `tools/check-layout.sh`: 5,000 code / 6,000 test lines. Use focused `round_b*.rs` modules, not one enlarged atomic test file. Preserve glibc/musl/FreeBSD buildability.
- Rule B: all scenario progress through `Backend` or `core_loop::run_core`; no manual `scene.tick`, `display_cursor_by_handle`, commit builders, matching calls, fake in-flight flags or direct retirement helpers. Constructor injection and read-only observations are allowed. Inject no-BO/render/atomic failures at real dependencies, not branches that bypass the path under test.
- Rule A: inspect every accepted intermediate primary/cursor state; end through Backend hide/disable/drain with no actionable intent, staged leases, events or framebuffer leaks. Preserve expected live ownership before cleanup after injected failures.
- Rule F: 16 busy loops, each new non-GPU test 5 repetitions; GPU-waiting tests 20 repetitions. Use A's PID/trap stress procedure. Vulkan-fed scanout tests skip before submission on missing `supports_sync_fd()`, matching `crates/yserver/tests/common/mod.rs`; skipped cases are not passes. Use actual Vulkan rendering, not a fake successful export.
- Rule C: implementation never accesses hardware. Every task ends with the literal full workspace/lavapipe gate; `cargo +nightly fmt` precedes the required `cargo fmt --all`, and the exact default CI clippy command runs before the task commit.

- Module registration belongs to the same task and commit as each new file: test rounds modify `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/mod.rs`; platform modules modify `crates/yserver/src/kms/render/platform.rs`; cursor-plane submodules modify `crates/yserver/src/kms/cursor_plane.rs`; DRM modules modify `crates/yserver/src/drm/mod.rs`. Treat these exact parent paths as part of each applicable Files block.

## Review Focus

1. A copied frame waits while live SW cursor state changes; independent outputs finish in reverse order, or a job is cancelled before its fd signals: the submitted frame keeps its own plan/lease. B1/B3 use only these reachable schedules. One output's pending ack prevents a second concurrent frame, and cancellation removes its completion entry; no same-output out-of-order or delivered-after-cancellation coverage is claimed.
2. A frame renders HW but is rejected, while a cursor-only scheduler pass runs: it cannot steal that frame's assignment; B2/B3 test this.
3. A stationary direct cursor changes image/hotspot while no Present arrives: progress without unrelated damage or a hidden “visible plane” prerequisite; B4/B5 test this.
4. An output gap or unequal output heights changes footprint membership: attach/detach current HW peers and repaint only required SW peers; B6 tests this.
5. Window entry changes sprite, then motion arrives both before submission and before retirement: the complete operation trace must never restore the old position; B7 tests this.

- **Controller rulings carried from sub-plan A (2026-10-17).** A1 was narrowed: there is no `ScanoutSlotAllocator` and no full discovery/GBM/PRIME injection. Where a step says "A1's `ScanoutSlotAllocator`", produce no-slot through the real path — withhold that output's recorded page-flip events (the A1 per-device flip-event fixture) so its scanout BOs stay busy — or, only if unreachable, a construction-selected fixture option configured before the scenario (A7 precedent). Modesets go through Backend RandR with the injected property/blob services from A8. Real mode-blob growth, the re-exec qualification worker and hotplug rediscovery stay in C11. Never write backend state mid-scenario (`for_tests_*` setters are construction-time only).
---

## Production-path evidence

Current symbol anchors: `backend/trait_impl.rs` dispatches input/scheduler/events/cursors via `backend/seam.rs::Platform`; `portable/pointer.rs::update_pointer_window` → `portable/cursor.rs::refresh_effective_cursor` → `display_cursor_by_handle` drives window entry, and `tick_cursor_animation` also calls display. `kms/scanout.rs::backend_scanout_maybe_composite` calls compose and the held/idle cursor lane; `platform/scene_sink.rs::{finish_frame,submit_shared_scanout_frame,submit_copied}` and `platform/scanout_bos.rs::submit_copied_scanout` implement frame handoff. Direct paths are `kms/scanout.rs::{submit_direct_frame_live,submit_composed_unflip,retire_direct_output,bind_direct_cursor_on_all_outputs,refresh_direct_cursor_on_all_outputs}` and `kms/platform_hooks.rs::{refresh_direct_cursor,hide_cursor_under_direct_scanout}`. The latter direct operations synchronize `scene/cursor.rs::record_external_cursor_plane_bindings` through `sync_direct_cursor_scene_state`. `scene/flip.rs::handle_page_flip_complete` applies stored transitions today; C1 adds another transition operation through `scene/cursor.rs::retire_external_cursor_handoff` at grouped-unflip retirement. B6/B8 must remove that path too.

C1's consumers include `direct_cursor_eligible`, `direct_frame_flip_timing`, `prepare_tearing_scene_pass`, `scene_wants_compose`, `backend_scanout_next_wakeup`, `Backend::mark_dirty`, and portable pointer/display/hidden paths. They consume accepted mode/visibility, pending transitions, wake causes or actual plane occupancy. Preserve their contracts as receipts replace observations and retirement actions. A1 supplies C1's constructor-selected clock; B4 extends its use to animation/debounce.

Before adding each B test, repeat `rg -n` for its named caller(s) under `crates/` and verify at least one non-test call. New functions below must be wired into these chains in the same task. The tests never advance them by direct invocation.

## File and interface decisions

- New neutral contract file: `crates/yserver-render/src/kms/render/scene/cursor_frame.rs`, re-exported by `scene.rs`. Define `CursorFrameId(u64)` with no reuse within one output lifetime; `CursorFrameAssignment::{Hardware, Software { pos: (i32,i32) }, Hidden}`; `CursorFramePlan { frame: CursorFrameId, assignment: CursorFrameAssignment, sprite: Option<CursorEntry> }`. No HW coordinates are stored in the plan. `CursorEntry` is the existing version/hotspot/bytes record and has no root position.
- `CursorAcceptedMode::{Hardware, Software, Hidden}` describes actual accepted ownership. Hardware covers both a KMS cursor plane and the surface backend's existing external/native host cursor, without exposing either OS representation. `CursorPrimaryContent::{Cursorless, Software { pos: (i32,i32), record_version: u64 }}` describes accepted pixels; `CursorCommitReceipt { frame: Option<CursorFrameId>, mode: CursorAcceptedMode, primary_cursor: CursorPrimaryContent, needs_recompose: bool }`; `None` frame identifies acceptance of an external cursor update alone, never permission to apply a rendered frame assignment. Visible native surface cursors use Hardware + Cursorless; a hidden native cursor uses Hidden + Cursorless. Never report Software for a cursorless surface primary merely to adapt the seam.
- Change `OutputSink::finish_frame(&mut self, output_idx: usize, bo_idx: usize, cursor: CursorFramePlan, record: &mut RecordFrame<'_>) -> FinishOutcome`. `FrameHandoff::FlipPending { submit: ComposeSubmit, cursor: CursorCommitReceipt }`; copied `RenderCompletion` handoff stays pending without acceptance. Change `OutputSink::submit_copied(&mut self, output_idx: usize, bo_idx: usize, fd: Option<OwnedFd>) -> io::Result<CursorCommitReceipt>`. Existing unavailable/error results remain unaccepted. Preserve C1's `FlipTiming`/`AsyncOutcome` independently of the new cursor receipt: a cursor-free Async primary may report Software/Hidden according to its actual pixels, but cannot accept a filtered cursor delta. Copied output remains Vsync, as in C1.
- `SceneCompositor::note_cursor_commit_accepted(&mut self, output_idx: usize, receipt: CursorCommitReceipt)` updates accepted mode and requests recompose when specified. `PendingAck` stores the receipt and rendered SW metadata; it contains no cursor operation. Retirement applies only presentation metadata and resource release. A pending transition still participates in aggregate Mixed bookkeeping.
- New KMS file `platform/cursor_frames.rs`: `stage_cursor_frame(&mut self, output_idx: usize, bo_idx: usize, plan: CursorFramePlan) -> io::Result<()>`; `cancel_cursor_frame(&mut self, output_idx: usize, bo_idx: usize, frame: CursorFrameId)`; `prepare_frame_cursor_commit(&mut self, output_idx: usize, bo_idx: usize, frame: CursorFrameId) -> io::Result<Option<PreparedCursorCommit>>`. Built leases belong to `(CrtcKey, bo_idx, frame)` and are released exactly once on cancellation/rejection or promoted on acceptance. A copied job retains that exact identity across the fd wait.
- B2 introduces `Platform::note_cursor_pointer(b: &mut XBackend<Self>, root_pos: (f32,f32))`, an unconditional position-observation hook independent of cursor mode/visibility. KMS stores it separately and samples/translates/rounds it at atomic build; it never publishes an assignment or queues a show by itself. Surface uses a no-op observation hook and keeps its existing host-warp behavior. The existing `move_hw_cursor` hook retains mode-specific wake/membership behavior; it is no longer the only source of current position.
- B4's `Platform::queue_hw_cursor_sprite(b: &mut XBackend<Self>, record: &Arc<CursorRecord>)` and `Platform::queue_hw_cursor_visibility(b: &mut XBackend<Self>, hidden: bool)` use the platform's existing associated-type seam. KMS records current intent without committing a frame assignment. Surface accepts the image/hotspot/visibility into its native cursor state and flushes through `ShellUi`; it continues using an external cursor rather than forcing SW registration/rendering. B2 already supplies live pointer observations before either shared or copied retirement coordinates are removed.
- `platform/cursor_commit.rs` continues A's `PreparedCursorCommit`; add `CursorCommitOwner::{Scene(CursorFrameId), HardwareOnly, Lifecycle}` to preparation metadata. Owner classification is semantic: it cannot be changed simply because a scene frame failed or has no BO.

## Task B1: Carry cursor plans and receipts across the output seam

**Files:**
- Create: `crates/yserver-render/src/kms/render/scene/cursor_frame.rs`, `crates/yserver/src/kms/render/platform/cursor_frames.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_frames.rs`.
- Modify: `crates/yserver-render/src/kms/render/scene.rs`, `crates/yserver-render/src/kms/render/scene/sink.rs`, `crates/yserver-render/src/kms/render/scene/tick.rs`, `crates/yserver-render/src/kms/render/scene/flip.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/surface/platform.rs`, `crates/yserver/src/surface/swapchain.rs`, `crates/yserver/src/surface/tests.rs`; update all existing trait implementors, including `crates/yserver-render/src/kms/render/scene/tests/mod.rs`.

**Interfaces:**
- Consumes: A's prepared commits and lease roles; P-scene; existing `CursorEntry`, `ComposeSubmit`, `RecordFrame`.
- Produces: neutral types and `finish_frame`/handoff/receipt signatures above. This task carries metadata while retaining A's conservative behavior; it does not switch HW↔SW ownership until B2/B3.
- Compatibility invariant: migrate direct `FrameHandoff::FlipPending` constructors as well as trait implementations. In particular, thread the surface plan/receipt through `surface/swapchain.rs::finish_frame` and settle successful native cursor acceptance in `SurfacePlatform` before publishing that receipt to the scene; unavailable/error handoffs publish no receipt or speculative native state. While KMS still uses A's adapter, its receipt reports actual retained ownership separately from that frame's pixels, not an unaccepted requested mode.

- [ ] **Step 1: Add `atomic_cursor_only_frame_receipts_name_the_accepted_frame`.** Through Backend render/compose/events, create serial distinguishable frames F0 and F1, retiring F0 before F1; inject a failed/unavailable handoff between them. Assert each successful receipt's frame ID and primary-content snapshot match that request's recorded FB/render, unavailable/error paths publish none, TEST_ONLY publishes none, and no cancelled frame changes accepted cursor mode. Mutation: return F0's cached frame ID/content receipt for F1; F1's exact ID/content assertion fails. This serial schedule proves receipt identity and rejection boundaries; it does not claim to detect a global-latest-assignment mutation (B3 supplies a held-frame schedule for that).
- [ ] **Step 2: Run red on missing/incorrect receipts.** Confirm the recorded primary framebuffer and frame identity agree; a state getter alone is insufficient.
- [ ] **Step 3: Thread the contract through KMS shared/copied and surface sinks, including the swapchain's direct handoff constructor.** Keep OS-native identifiers behind the neutral seam. Preserve native image/hotspot/show/hide delivery through `ShellUi`, with Hardware/Hidden + Cursorless receipts. Extend the existing surface tests to assert receipt/content agreement and retain `surface_pointer_motion_composes_nothing_and_sends_no_cursor` in this task's gate. A copied render handoff has no accepted receipt until its final KMS submit succeeds. Keep C1's `scene_flip_timings`, async ledger/fallback result and tearing copy-receipt timestamps intact; publish a cursor receipt only for the request actually accepted, including Vsync fallback.
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
- Modify: `crates/yserver-render/src/kms/render/scene/tick.rs`, `crates/yserver-render/src/kms/render/scene/cursor.rs`, `crates/yserver-render/src/kms/render/scene/flip.rs`, `crates/yserver/src/kms/render/platform/cursor_frames.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs`, `crates/yserver/src/kms/render/backend/seam.rs`, `crates/yserver/src/kms/render/backend/portable/pointer.rs`, `crates/yserver/src/kms/render/backend/kms/platform_hooks.rs`, `crates/yserver/src/kms/render/backend/kms/session.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/surface/platform.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_frames.rs`.

**Interfaces:**
- Consumes: B1 plan/receipt, A applied states/deltas and buffer leases; P-scene/P-cursor.
- Produces: shared route's stage/prepare/cancel methods and acceptance notification; `CursorCommitOwner::Scene(frame)` plans are invisible to cursor-only preparation.
- Produces **before removing the old coordinate path**: the unconditional `Platform::note_cursor_pointer` hook above, KMS storage and constructor initialization from the actual startup pointer. Every authoritative position update (host/relative input, root/window warp, core/XTEST pointer notification and configuration clamp) reports the final confined root position even while Software or Hidden, including while C1 owns cursor policy; observation alone must not attach HW or turn `wake_for_cursor_damage` into ordinary damage. Preserve `effective_cursor_hidden` and the `tearing_active` → `cursor_motion_during_tearing` branch before normal HW motion. Audit all writes to the portable pointer fields, excluding temporary coordinate substitutions used only to serialize floating-device events. Surface observation has no repaint/host-cursor side effects. B4 consumes this completed path rather than introducing it later.

- [ ] **Step 1: Add `atomic_cursor_only_shared_transition_is_one_primary_commit`, `atomic_cursor_only_no_bo_does_not_publish_assignment`, and `atomic_cursor_only_failed_frame_cannot_be_stolen_by_cursor_only`.** Use Backend definition/eligibility change to request SW→HW and HW→SW, inject an unavailable BO then a rejected frame, and run idle scheduling between retries. Assert accepted SW→HW contains a cursorless primary plus plane attach; accepted HW→SW contains SW pixels plus detach in the **same request**. Before acceptance, mode and primary stay old; after acceptance mode changes and retirement resolves Mixed metadata. No-BO and failed frames publish neither plan nor mode through cursor-only. Mutations: retain the two-phase cursorless HW→SW frame, apply mode before acceptance, or copy the scene plan to global HW-only intent; fail the content/owner trace assertions.
- [ ] **Step 1a: Add `atomic_cursor_only_first_hw_attach_samples_motion_while_software`.** Establish SW using a valid oversized cursor on an otherwise eligible output, retire that frame, then move P0→P1 through Backend while still SW. Define an eligible cursor without another motion event and accept its first shared SW→HW frame. Assert CRTC_X/Y are current output-local P1 minus the new hotspot, not P0/startup position. Repeat with hidden motion followed by show. Mutation: observe pointer updates only in HW/Mixed mode; the first-attach coordinate assertion must fail at this task's gate.
- [ ] **Step 2: Run red through Backend scheduling.** Use A1's `ScanoutSlotAllocator` dependency at real `acquire_scanout_bo` to inject no-slot, then availability; never set BO/in-flight state by hand.
- [ ] **Step 3: Wire unconditional position observation, then switch only the shared route to frame-owned transitions.** Initialize the position store and connect all authoritative updates before removing its retirement-time show/hide calls; stop stripping the SW draw on normal HW→SW. Sample HW position at final request build. C1's `tearing_on` / scene `hw_strategy_enabled` remains the policy override: On builds detached target state, while effective-transparent/XFIXES-hidden cursors stay Hidden rather than becoming SW sprites. A composed visible HW→SW entry is a Vsync scene frame with SW pixels plus atomic detach; do not select Async based only on desired mode or accepted detach before its event. Preserve `cursor_transition_pending` as an outstanding transition-receipt predicate for C1's probation gate, including copied render waits and accepted-but-unretired frame changes; B8 removes old transition storage, not this safety query. Until C's detailed recovery arrives, reject/abandon a failed transition frame safely with its old primary and actual binding retained; do not accept a software-cursor frame beside bound HW. Recover and cancel staged leases on every no-device/no-slot/render/submit failure.
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

- [ ] **Deferred from A6.** Add a fixture failure seam at the cursor-lease reservation boundary (scripted before the scenario) and Backend-driven regressions for Shared/composed-unflip reservation rollback and accepted-Shared-settlement failure; mutations: drop each rollback → leases/reservations do not return to zero.

## Task B3: Preserve copied-frame ownership across deferred submission

**Files:**
- Modify: `crates/yserver/src/kms/render/platform/cursor_frames.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/kms/render/platform/scanout_bos.rs`, `crates/yserver/src/kms/render/platform/flip_events.rs`, `crates/yserver/src/kms/render/platform/services.rs`, `crates/yserver/src/kms/render/platform/init.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/support.rs`, `crates/yserver-render/src/kms/render/scene/tick.rs`, `crates/yserver-render/src/kms/render/scene/flip.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_frames.rs`.

**Interfaces:**
- Consumes: B1 copied receipt contract, B2 shared staging/settlement; P-scene's real render-completion path.
- Produces: copied completion associates its job with `(CrtcKey,bo_idx,CursorFrameId)`; `submit_copied` returns a receipt only after final atomic acceptance. Cancellation invalidates a job without transferring its assignment elsewhere.
- Produces: `RenderCompletionReadiness::is_ready(&self, job_id: u64, fd: Option<BorrowedFd<'_>>) -> io::Result<bool>` at the existing completion poll boundary in `platform/flip_events.rs`. `KmsServices` carries the dependency through platform construction. The production implementation uses the existing zero-timeout fd poll; the recording implementation may withhold an actually ready job until released, but cannot declare an unsignalled GPU fd ready. Register, drain and cancellation still use the production pending-job queue and fd ownership. This controls delivery of real completed work without replacing Vulkan rendering/copying or inventing stale jobs.
- Scheduling invariant / coverage limit: one output has at most one pending scene ack/job. Concurrent jobs may exist on different outputs and complete in either order. Cancellation unregisters/removes the old job before a new job on that output; signalling its old fd later does not deliver a stale completion. Preserve these existing rules and add no rendering pipeline or stale-completion injection seam. Matching by output index alone is not claimed as a killed mutation under this scheduler; identity is checked against reachable submitted-frame content and cancellation/resource accounting.

- [ ] **Step 1: Add `atomic_cursor_only_copied_frame_keeps_assignment_until_actual_submit` and `atomic_cursor_only_built_sprite_is_not_reused_before_submit`.** Drive Backend compose with a held render-completion dependency, move P0→P1 before signalling completion, and invoke `Backend::on_scanout_render_completion`. Assert no plane/mode publication before the final KMS request; HW attachment uses P1 from B2's pointer store. In a separate SW case keep the output ineligible for HW with both V0 and V1, render F0 with SW cursor V0 at P0, hold its completion, then move to P1 and define V1 through Backend while F0's ack blocks another frame. Assert final F0 receipt/content still names SW(V0,P0) and matches its pixels; only a later rendered F1 may report SW(V1,P1). This creates an observed difference from current global assignment without concurrent jobs on one output. For a built HW sprite lease, pressure the pool with newer records and assert no write/RMFB until the frame submits or cancels.
- [ ] **Step 1a: Cover reachable completion order and cancellation.** Hold one job on A and one on B, release B then A through the production completion poller/Backend dispatch and compare each request/receipt/FB with its own rendered content. Separately cancel/remove A while its fd is held, assert unregister/removal and release of its built lease once, then signal that old fd and call Backend completion dispatch: no old request or receipt appears. Re-enable A and run its new job normally; do not deliver a fabricated stale completion or bypass the pending-ack gate. Include already-signalled completion and copy failure. Mutations and witnesses: publish at render acceptance → a receipt appears before signalling; recompute F0's assignment from global state → SW(V1,P1) mismatches F0 pixels; drop a built lease early → a pressured slot is rewritten while F0 still owns it; omit cancelled-plan release → the per-job lease ledger remains live after cancellation/teardown. No mutation is credited merely because a test name mentions identity.
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

- [ ] **Deferred from A6.** Add a fixture hook delivering a second copied-render completion while the output is ready; assert the copied `SceneWithCursor` motion delta through Backend; mutation: omit the delta in `submit_copied_scanout`.

## Task B4: Route every HW-only producer into current intent

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/seam.rs`, `crates/yserver/src/kms/render/backend/portable/cursor.rs`, `crates/yserver/src/kms/render/backend/portable/pointer.rs`, `crates/yserver/src/kms/render/backend/kms/platform_hooks.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs`, `crates/yserver/src/surface/platform.rs`, `crates/yserver/src/surface/tests.rs`, `crates/yserver/src/kms/render/platform.rs`, `crates/yserver/src/kms/render/platform/services.rs`, `crates/yserver/src/kms/render/platform/init.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/support.rs`, `crates/yserver/src/kms/render/backend/kms/session.rs`, `crates/yserver/src/kms/render/backend/kms/randr.rs`.
- Modify: `crates/yserver/src/kms/render/backend/kms/state.rs`, `crates/yserver/src/kms/render/backend/kms/scanout.rs`, `crates/yserver/src/kms/render/backend/trait_impl.rs`, `crates/yserver-render/src/kms/render/scene/damage.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round2.rs`.
- Create: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_producers.rs`.

**Interfaces:**
- Consumes: A full intent scheduler, B2/B3 frame-owner exclusion, P-pointer and portable animation non-test callers.
- Consumes: A1's `KmsServices::monotonic_clock: Rc<dyn Fn() -> Instant>` wired to C1's existing `KmsBackendState::monotonic_clock` and `KmsBackend::monotonic_now`. Produces: neutral `Platform::monotonic_now(b: &XBackend<Self>) -> Instant` (default `Instant::now`, KMS forwards to that existing method), used for animation/debounce deadline creation **and** evaluation. Reuse one constructor-selected clock for C1 cap and new tests; do not create a second clock trait/module or reset last-compose/deferred-wake state during migration. A1's injected hotplug source remains the event origin; advancing time alone never creates a hotplug edge. Migrate clock-controlled C1 fixture callers in the same task; the old test setter cannot become the only way this dependency reaches production.
- Produces: platform seam queue hooks above; `CursorCommitOwner::HardwareOnly`. HW sprite/visibility/motion intent is distinct from `CursorFramePlan`; no captured HW coordinates in pending frame metadata.
- Consumes B2's already wired unconditional pointer delivery. Implement the native surface sprite/visibility hooks here too, with successful native updates represented by Hardware/Hidden + Cursorless receipts and deduplicated `ShellUi` notifications. Do not add SW repaint on native pointer motion.

- [ ] **Step 1: Add `atomic_cursor_only_stationary_hw_image_and_animation_make_progress` and `atomic_cursor_only_hw_visibility_and_hotspot_need_no_scene_frame`.** Establish HW via accepted Backend frames, keep primary undamaged, then drive image replacement, animation deadline, hotspot-only change and hide/show through Backend. Assert current FB/hotspot/attach/detach deltas go via cursor-only when idle; no new primary required. While a real frame/direct flip is pending, coalesce newest data and wait; before a frame is built, let its commit carry the HW-only change without changing its composition owner. Mutation: keep the visible-plane/position-only guard, require scene damage for sprite upload, or reset animation pixels to an older record at retirement; each fails progress or identity assertions.
- [ ] **Step 1a: Add `atomic_cursor_only_effective_visibility_overrides_hw_intent` and `atomic_cursor_only_tearing_cursor_cap_survives_all_motion_sources`.** Through Backend select transparent core/RENDER/glyph cursors, all-transparent animation, mixed transparent/opaque animation and XFIXES-hidden visible cursors. Assert the inherited classification (mixed animation remains visible), Hidden cursorless primaries, and no attach of transparent records, including held direct. Enter On via real Presents; use one constructor clock to drive host/relative motion, root/window warp, core/XTEST motion and generic request wakes through Backend/run_core. At elapsed time below `1_000_000_000 / max(active_nonzero_vrefresh)` nanoseconds (60 Hz fallback), cursor-motion-only recomposition waits; at its deadline it progresses once with latest position. Client Present/Copy and structural damage or owed repaint bypass the cap, never lose damage and never wait for the cursor deadline. Exercise A5's temporary external handoff interval here; B6 reruns the same cap scenario after replacing that handoff, so this task does not depend on future B6 code. Mutations: use only `cursor_hidden`, route tearing motion to normal HW intent, convert cursor wake to ordinary damage in `mark_dirty`, reset the clock/cap on each observation, or cap client damage; fail the visibility/request-count/deadline assertion. Retain `tearing/round2.rs::task6_present_and_xtest_motion_keep_cursor_compose_cap_under_run_core_load` as a regression gate; its old seeded fixture is not new rules-A/B evidence.
- [ ] **Step 2: Run red; advance animation via Backend scheduling and a controlled deadline dependency.** Do not call `tick_cursor_animation` or change `active_cursor_anim` fields to fake the path. Use the shared constructor clock and `Platform::monotonic_now` hook defined here, with the same default `Instant::now` behavior; the test controls time, not animation state.
- [ ] **Step 3: Queue record bytes/hotspot/visibility before deciding whether a software sprite view is needed.** A HW-only update must not be silently blocked by the portable `display_cursor_by_handle` SW image-view readiness guard. Retain SW allocation/registration for outputs that paint the cursor. Evaluate effective visibility before queueing; while On retain newest sprite/position as observations for later policy restoration, but normalize actionable HW intent to Detached, never attaching even on a Vsync fallback. That override does not authorize a HardwareOnly detach for a visible HW→SW transition: B2/B3's scene frame or B6's handoff owns it (A5's external adapter until B6), with C2's explicit error recovery separate. Until B5 removes the temporary direct adapters, they consume this same prepared intent/lease service; queueing cannot create a second bind authority or duplicate I/O. Leave C1's `cursor_motion_during_tearing`, `tearing_cursor_compose_cap_applies`, `wake_for_cursor_damage` and `Backend::mark_dirty` damage-cause distinctions active. Queueing never authorizes a skipped scene transition. Hold newest pending data under pool pressure and wake on release.
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

- Modify: `crates/yserver/src/kms/render/backend/seam.rs`, `crates/yserver/src/kms/render/backend/kms/present.rs`, `crates/yserver/src/kms/render/backend/kms/state.rs`, `crates/yserver-render/src/kms/render/scene/cursor.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round2.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round3.rs`.

**Interfaces:**
- Consumes: B4 HW intent and A's per-CRTC arbitration; P-direct's first-bind, direct refresh, hidden hook and held scheduling callers.
- Produces: direct show/hide/first-bind producers only queue HardwareOnly intent; replace `refresh_direct_cursor_on_all_outputs`, `bind_direct_cursor_on_all_outputs` and `Platform::{refresh_direct_cursor,hide_cursor_under_direct_scanout}` side effects. Replace `sync_direct_cursor_scene_state` / `record_external_cursor_plane_bindings` with B1's `note_cursor_commit_accepted(output_idx, CursorCommitReceipt { frame: None, ... })` after actual atomic acceptance, using actual retained ownership on error. Retire or derive `cursor_bound_all` from accepted per-output bindings; it cannot suppress a later queued attach. First bind is scheduled at direct admission/build, or after its outstanding event when necessary; it never replays a captured retirement-time position. Existing direct-unflip request remains the fallback when the plane cannot take the sprite. Direct admission requires verified Cursorless retained-composed content per output before holding a direct frame that may acquire HW; if a retained primary still contains SW, complete the normal scene handoff first, then retry admission. Preserve `recover_hidden_cursor_on_resume` as A8's independent lifecycle hook.

- [ ] **Step 1: Add `atomic_cursor_only_held_direct_services_show_hide_animation_and_first_bind`.** Through `Backend::try_present_direct`, establish one accepted held Vsync direct frame and retire it; send no successor. Exercise first attach (including initially hidden direct), stationary show, hide, image, animation and hotspot updates. Assert each necessary cursor-only commit appears, primary FB stays the held source, and event obligations serialize normally. Make the plane ineligible/oversized; assert direct unflip through normal composed recovery instead of SW pixels being invented in the direct primary. Mutations: rely on next Present, bind at direct retirement, keep the visible-only guard, or omit the hide producer; the corresponding no-successor trace fails.
- [ ] **Step 1a: Add `atomic_cursor_only_vsync_direct_rebind_receipt_routes_motion_to_hardware`.** Reproduce C1's Plasma sequence solely through Backend: transparent cursor gives Hidden scene mode; establish/retire held Vsync direct; DefineCursor or XFIXES Show selects an opaque sprite; schedule/accept atomic first attach and move again with no successor Present. Assert accepted receipt changes that output to Hardware with Cursorless primary, actual plane remains bound, and subsequent motion sends current X/Y instead of only waking a scene held by direct. Repeat visible→transparent→visible, with accepted hides and staggered events; rejected/EBUSY attach cannot publish Hardware. Pending scene acks reached through real composition must prevent direct admission/receipt overwrite and lead to normal wait/unflip. Mutations: omit external receipt, publish at intent, retain Hidden after accepted rebind, or guard first bind on existing visibility; fail mode/request/progress assertions. This explicitly supersedes the synchronous legacy bind timing in `tearing/round2.rs::{define_cursor_rebind_routes_motion_to_bound_plane_during_vsync_direct_scanout,xfixes_show_rebind_routes_motion_and_transparent_roundtrip_during_vsync_direct_scanout}` and `tearing/round3.rs::define_visible_cursor_rebinds_held_vsync_direct_scanout`. Preserve their scenarios in migrated tests, green in this task; do not keep an obsolete `record_external_cursor_plane_bindings` stub just for fixtures.
- [ ] **Step 2: Run red through Backend direct admission/events.** Use A1's shared `submit_direct_frame_live` path and raw import/TEST_ONLY dependencies; no `direct_frame_submitter` success shortcut or manually installed current direct frame.
- [ ] **Step 3: Remove synchronous direct bind/refresh/hide side effects.** Keep queued direct successor priority over cursor-only, and wait for every grouped CRTC before submitting an overlapping transaction. Mixed bookkeeping cannot by itself force an unnecessary unflip for a healthy HW-only update. While On no direct hook may bind; `direct_cursor_eligible`/`direct_frame_flip_timing` read accepted occupancy and outstanding detach events, not wanted Detached. A pending or rejected detach cannot make an async direct request eligible.
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

- Modify: `crates/yserver/src/kms/render/backend/kms/present.rs`, `crates/yserver/src/kms/render/backend/kms/state.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/kms/render/platform/scanout_bos.rs`, `crates/yserver/src/kms/render/platform/cursor_frames.rs`, `crates/yserver/src/kms/render/backend/tests/tearing.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round2.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round3.rs`.

**Interfaces:**
- Consumes: B2/B3 acceptance receipts, B4 membership intent, B5 direct ownership; P-pointer/P-direct/P-tearing.
- Produces: output-local accepted cursor modes plus aggregate Mixed during pending or heterogeneous output states. HW footprint entry/exit while staying HW uses cursor-only attach/detach; actual HW↔SW composition changes remain frame-owned. This task replaces C1's `retire_external_cursor_handoff` operation and its `retire_direct_output` call with frame-owned detach and receipt-only retirement. C1's probation/owner/exit policy, newest-source shadow copy and grouped CRTC event barriers stay in use.

- [ ] **Step 1: Add `atomic_cursor_only_cross_output_membership_preserves_mixed_mode` and `atomic_cursor_only_direct_probe_and_grouped_unflip_keep_cursor_owners`.** Backend motion crosses HW/HW then HW/SW outputs with offsets, gaps, unequal heights, shared sprite and staggered events. Assert current local positions, HW-only attach/detach without a new primary, and SW peer repaint without replaying another output's transition. In a grouped direct successor/unflip, hold a cursor-only flip on one CRTC: group waits; after event, TEST_ONLY and actual direct requests carry the same applicable cursor deltas, but probing publishes no state. Rendered SW transitions are never attached to a retained composed framebuffer whose pixels do not match them; recompose first when necessary. Mutations: route by raw CRTC alone, classify mixed as all-HW, attach during an outstanding peer event, omit a cursor field in the probe, or consume a pending scene plan in unflip; each fails its trace/content assertion.
- [ ] **Step 1a: Add `atomic_cursor_only_tearing_handoff_keeps_hw_until_software_detach_frame`.** Backend-drive On entry from visible-HW held direct and hidden→visible while On, including a dead-zone return and multiple grouped outputs. Preserve the newest Present/Copy fallback source in the materialized backing for the following recompose, rather than replacing it with the stale retained direct source. For initially bound HW, the grouped Vsync unflip restores verified cursorless primaries and **retains** actual HW ownership; the following real per-output Vsync scene frame paints SW and detaches in that same request. For hidden→visible with HW already detached, the following SW frame needs no cursor delta and may be cursor-free Async after the group event, subject to C1's existing gates; never invent a redundant detach or a new async restriction for that case. Assert no detach on grouped retirement, no SW publication from cursorless retained content, one stream per CRTC and no Async until all old HW/detach events drain. Inject no BO, render failure, EBUSY and detach rejection: retain any old HW with a cursorless Vsync primary (or retain the actually hidden state when no HW was bound), no double cursor, no filtered-delta acceptance, and keep On without rebinding; C3 later adds exact retry recovery. Hidden cursor with no HW can remain/re-enter direct Async; visible cursor uses composed path, and Show/animation cannot rebind while On. Mutations: retain C1 retirement hide, report Software on cursorless unflip, reveal SW before accepted detach, allow Async at intent or detach acceptance before drain, or drop newest fallback source; fail intermediate trace/content assertions.
- [ ] **Step 1b: Preserve C1 pacing across the changed handoff.** Keep `Tearing::note_composed_submit` on accepted grouped unflip and later composed submissions, `last_cursor_compose`/`deferred_cursor_wake`, the in-flight handoff cap, pending-transition probation guard and client-damage bypass. Run B4's clock-controlled cap test during grouped wait and after SW acceptance, and preserve `tearing/round2.rs::task5_handoff_unflip_starts_cursor_compose_interval` with receipt-based expectations. Keep `tearing.rs::{entry_from_held_direct_with_visible_hw_cursor_runs_handoff,show_cursor_during_held_direct_hidden_tearing_makes_cursor_visible,show_cursor_does_not_rebind_plane_while_on,hide_failure_keeps_visible_cursor_frames_vsync,cursor_toggle_cycles_never_async_with_bound_plane}` green or mapped to the new Backend-only scenario before removal. The old hide-at-retirement and mandatory reveal-gap assertions are superseded by the same-request SW+detach and drain assertions, not silently deleted.
- [ ] **Step 2: Run red using Backend motion/direct/unflip entry paths.** No direct calls to request builders or mode classifiers.
- [ ] **Step 3: Replace the old membership-dirty scene wake for HW-only crossings with current intent.** Preserve necessary damage for SW peers, inverse transforms/current transform SW policy, and existing direct topology restrictions. Retained composed FBs need truthful content/receipt metadata; a group cannot publish a speculative assignment. Keep C1's `materialize_direct_shadow_for_unflip` null-image guards, fence readiness and newest-source copy into the fallback drawable backing; that operation does **not** render/refresh retained scanout BOs. Verify retained Cursorless content from its last accepted frame receipt (B5 admission establishes this), never infer it from shadow-copy success or relabel a SW-containing FB. If no safe retained target exists or preparation fails, retain direct/HW and use the existing degraded per-output scene fallback with B2's real SW+detach frames and direct-source pins held through replacement retirement; never accept SW+HW. The group publishes retained Hardware/Hidden + Cursorless receipts, then ordinary scene rendering after group retirement owns SW+detach. Remove `retire_external_cursor_handoff` and its direct-retirement caller here; B8 removes remaining scene retirement machinery. Update the handoff cap predicate to track the real grouped wait/receipts, not the deleted transition enum.
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

- [ ] **Deferred from A6.** Assert the grouped direct-primary successor carries the motion delta; mutation: omit it in the direct successor producer.

## Task B7: Prove the #244 window-entry regression against master

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/portable/pointer.rs`, `crates/yserver/src/kms/render/backend/portable/cursor.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs` as needed for live-position production wiring.
- Create: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_jump.rs`.
- Modify: `docs/status.md` with reproduction evidence.

**Interfaces:**
- Consumes: A1 unified legacy+atomic operation trace, B3 controllable copied completion, B4 current-position intent; P-pointer's **actual** window hit test → crossing → refresh → sprite update.
- Produces: `atomic_cursor_only_window_enter_never_restores_p0`, a reproducible failure on base master and a pass with B's implementation. No transition-state seeding is part of the fixture.

- [ ] **Step 1: Build two mapped windows with distinct cursors through core requests or Backend window/cursor entries plus the authoritative server tree.** Establish the initial cursor through accepted frames. Warp/host-motion into the second window at P0=(120,80), defer the real frame's copied submission, move inside that window to P1=(180,130), release completion, hold retirement, then deliver the event. Record every bind/move/hide/atomic operation and accepted result from entry through final drain. Assert every post-motion cursor position operation uses P1 (adjusted by that sprite's hotspot/output origin), never P0, including during and after retirement. Cover a second schedule where motion lands after acceptance but before retirement. Assert the cursor XID/version actually changed through the enter path.
- [ ] **Step 2: Demonstrate red on master.** Apply only fixture/recording-seam changes to a controller-provided isolated checkout of `9524c80d` (the C1-inclusive rebase base; keep the stimulus on the composed/copied route so direct rebind does not mask the stale scene coordinates), without bringing the fix or changing cursor behavior. The complete trace must contain the stale legacy move to P0 after the P1 motion. Do not count a missing new interface, build failure, seeded HW state or assertion on atomic properties alone as reproduction. If this exact scenario does not fail, refine the production-path schedule before proceeding.
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
- Modify: `crates/yserver-render/src/kms/render/scene.rs`, `crates/yserver-render/src/kms/render/scene/cursor.rs`, `crates/yserver-render/src/kms/render/scene/tick.rs`, `crates/yserver-render/src/kms/render/scene/flip.rs`, `crates/yserver-render/src/kms/render/scene/lifecycle.rs`, `crates/yserver-render/src/kms/render/scene/sink.rs`, `crates/yserver-render/src/kms/render/scene/tests/mod.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/surface/platform.rs`, `crates/yserver/src/surface/tests.rs`, `docs/status.md`.
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs`, `crates/yserver/src/kms/render/backend/kms/present.rs`, `crates/yserver/src/kms/render/backend/kms/platform_hooks.rs`, `crates/yserver/src/kms/render/backend/seam.rs`, `crates/yserver/src/kms/render/backend/tests/tearing.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round2.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round3.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_b_frames.rs`, `crates/yserver/src/surface/tests.rs` and relevant existing scene tests.

**Interfaces:**
- Consumes: B1 receipts and B2–B7 owners/trace coverage; the non-test `scene/flip.rs::handle_page_flip_complete` and `scene/lifecycle.rs::drain_all` transition calls are removal targets; B5/B6 have already replaced C1's external sync and external-retirement operation.
- Produces: no `CursorTransition`, `ShowOnRetire`, `HideOnRetire`, retire-time upload/show/hide or stored HW show coordinates. `PendingAck` and accepted mode remain truthful through receipt-only retirement. Keep `cursor_transition_pending()` as a receipt-based predicate for C1 probation; it includes staged/copied/accepted-but-unretired changes and is not an operation. Neither `record_external_cursor_plane_bindings` nor `retire_external_cursor_handoff` remains as a second authority. `CursorPlaneSink` retains eligibility queries but loses operation methods that only the old retirement path used.

- [ ] **Step 1: Add `atomic_cursor_only_retirement_is_cursor_operation_free` and `atomic_cursor_only_surface_cursor_preserves_native_ownership`.** Backend-driven scene/cursor/direct transitions accept and then retire while another cursor generation is queued; With no actionable newer intent, assert retirement adds zero cursor I/O. Separately queue a newer generation and permit Backend event dispatch to schedule it: any new I/O must be owned by that newest live intent and use current position, never a stored retirement action; old retirement releases only its own leases/metadata and cannot erase the newer intent. Through Surface Backend definition/show/image/hotspot/hide entries, assert `ShellUi` receives the native image/hotspot or hide, accepted mode is Hardware/Hidden, and primary pixels/receipts stay Cursorless. Retain the existing motion regression: no compose wake, extra primary frame or redundant `ShellUi` cursor image on motion; warps still move the host pointer. Mutations: keep a retirement operation → extra I/O after retirement; clear a newer generation on old ack → latest native/KMS update is lost; force surface Software or paint SW → receipt/pixel/no-compose assertions fail; omit native notification → expected `ShellUi` call is absent. Surface Hardware is expected external ownership, not a mutation.
- [ ] **Step 2: Establish the mutation red before deleting the old machinery.** Earlier B tasks may already make the new retirement test pass. Temporarily restore one old production retirement operation (and separately the old generation-clear behavior), verify the behavioral assertion fails, then revert the mutation. Observe operations, not merely type-name grep results.
- [ ] **Step 3: Delete the retired transition machinery and update all sink/test implementations.** Remove `force_show_retry_version` behavior that replays old records; C6 supplies final key/epoch qualification. Scene `drain_all` must not submit a hide after lifecycle ownership has already disabled the CRTC. Audit C1 callers of `cursor_mode`, `cursor_transition_pending`, direct visibility and `scene_cursor_hide_failed`: consume receipts/actual bindings; move the once-only hide-failure signal to submit/recovery rather than the deleted sink hide method. Keep tearing readback/compose `hw_strategy_enabled` overrides, window exit hooks, generic-wake cap and resume hook functional. A8/C7 own resume/lifecycle safety; do not route recovery through the now-removed direct hide hook. Keep fail-closed intermediate recovery from B2/B3 until C installs the exact §3 retry policies. Update status to distinguish completed ownership/#244 work from remaining C recovery/reassignment/removal.
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
| §2.2 owner row 1 | HW↔SW belongs to exact frame that changes SW pixels, including C1 handoff; missing/skipped/failed frame publishes nothing | B1, B2, B3, B6 |
| §2.2 owner row 2 | HW-only motion/image/animation/hotspot/HW-output entry/exit; idle cursor-only, ride unbuilt request or await event | A6, B4, B6 |
| §2.2 owner row 3 | Held Vsync direct show/hide/animation/first bind without successor; unavailable plane unflips | B4, B5 |
| §2.2 owner row 4 | DPMS/VT/output removal detaches in same blocking disable | A5, A8, C7 |
| §2.2 mode bullet | Mode changes at acceptance (including external direct receipts); flip retirement and heterogeneous/pending Mixed retained | B1, B2, B3, B5, B6, B8 |
| §2.2 retirement bullet | Remove ShowOnRetire/HideOnRetire/stored HW coordinates; unconditional pointer delivery precedes frame-path removal; every HW position sampled at build | A6, B2, B3, B5, B6, B7, B8, C10 |
| §2.3 bullet 1 | Per-device cap-size dumb buffers; ARGB8888 AddFB2; top-left transparent padding; full-buffer unscaled SRC/CRTC; pointer−hotspot | A4, A5 |
| §2.3 users: on-screen | Old FB remains a user until its replacing flip event, not acceptance | A5, A7 |
| §2.3 users: submitted | Submitted in-flight new and old references cannot be reclaimed | A5, A7, C7 |
| §2.3 users: built | Built but unsent request retains its sprite across deferred submission/cancellation; B3 uses one pending job per output and real cancellation | A5, B3 |
| §2.3 reclaim paragraph | Write/RMFB only at all-user zero across outputs; blocking return AND old-event drain | A7, A8, C7, C9 |
| §2.3 image/hotspot bullet | Image and hotspot-only change use free buffer and switch FB_ID for virtio host update | A7, B4, B5, C11 |
| §2.3 pressure bullet | Newest wanted pending, older pending dropped, wake at free user; two initial / one-per-CRTC-plus-one bound | A4, A7, B3, B4 |
| §2.4 bullet 1 success | One blocking lifecycle disable+detach; applied detached; last-user reclaim after old events, no replacing event | A5, A8, C7 |
| §2.4 bullet 1 failure | Failed disable preserves previous ownership/resources and current error path | A5, A8, C7, C9 |
| §2.4 bullet 2 | Resume/modeset starts invalidated | A8, C7 |
| §2.5 | Merged C1: detached wanted state while On, retained actual HW on failed detach; direct/shared async requests contain no cursor properties or OUT_FENCE_PTR; no false lease acceptance | A5, A6, B2, B5, B6, C1, C3, C5, C10 |
| §2.5 inherited C1 visibility/direct | Transparent means hidden; held Vsync direct rebind updates scene at atomic acceptance; subsequent motion moves bound plane (Plasma regression) | A3, A4, A5, B4, B5, B8, C10 |
| §2.5 inherited C1 pacing/lifecycle | Cap only cursor-motion recomposition at fastest active refresh, including handoff; never delay client damage; restore policy/clock/wakes after exit and resume | A1, A8, B2, B4, B6, B8, C7, C10 |
| §3 attribution bullet | Rejected cursor-carrying frame TEST_ONLY full vs without cursor fields | A5, C1 |
| §3 cursor-only EINVAL | Applied unchanged; SW-pending; explicit detach acceptance before SW/recompose; rejected detach retains HW/log once | C2 |
| §3 frame HW→SW hide | Retry SW frame with explicit detach; if rejected, abandon frame/retain old primary+HW; cursorless replacement without cursor fields | C3 |
| §3 frame SW→HW show | Retry cursorless primary without cursor props; one cursorless accepted frame; SW ownership + immediate SW recompose | C4 |
| §3 frame HW-only riding | Retry primary without cursor props then cursor-only-style detach recovery | C5 |
| §3 retry fails | All rejected retry variants preserve current flip error paths and resource ownership | C1, C3, C4, C5 |
| §3 EBUSY | #244 event wait, never cursor failure, no fake event obligation | A6, C1 |
| §3 requalification | (width,height,hotspot) key; no pixel/animation requal; once per new key per CRTC/epoch; rejected revisits wait topology | C6 |
| §4 rules A/B/C/F | Backend/run_core only (C7 shutdown uses a shared Backend trait entry); complete startup/request trace and independent leak oracle; final controller hardware; 5x/16 loops, GPU 20x | A1 and every task gate; C7, C11 |
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

## SPEC QUESTIONS

- Editorial: the approved atomic spec header still names pre-C1 base `8b8b1364` and says draft. Update its baseline/status when the spec owner next revises it; these plans use `9524c80d` and retain approval.
- Cross-spec alignment: C1's `2026-10-14-tearing-on-demand-design.md` entry-handoff/test prose requires a retirement-time hide and a subsequent reveal frame. Atomic spec §§2.2/2.5 supersede that mechanism: B6 keeps actual HW through a verified cursorless grouped unflip, then uses the real Vsync scene frame that paints SW to atomically detach; cursor-free async work waits for retirement. Update the companion spec to describe that equivalent behavior and failed-detach safety. This is the already-authorized C1 adaptation, not a new tearing policy or a reason to edit either spec during this task.
