# Atomic Cursor Only C: Recovery, Lifecycle and Reassignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Execute inline; no subagents. The last task is controller-led hardware validation with the user.

**Goal:** Complete safe rejection recovery, bounded requalification, lifecycle/reassignment, and deletion of every legacy cursor ioctl path.

**Architecture:** Build on [A](2026-10-15-atomic-cursor-only-a.md)'s owned atomic state and [B](2026-10-15-atomic-cursor-only-b.md)'s frame-scoped ownership/acceptance receipts. Classify rejection by the actual request and the rendered frame's cursor content, so recovery never accepts a software cursor while HW stays bound. Track desired plane matching separately from actual ownership, with a separate accepted detach and event drain before attachment elsewhere.

**Tech Stack:** Rust, existing DRM atomic sink and TEST_ONLY attribution, dumb-buffer sprite pool, scene acceptance receipts, Backend/core-loop fixtures, existing lifecycle and hotplug dispatch.

**Spec:** `docs/superpowers/specs/2026-10-15-atomic-cursor-only-design.md`, approved/binding; rationale `/home/ariel_santangelo/Projects/claude-scratch/atomic-cursor/spec-review-r{1,2,3,4}.md`.

**Revalidation baseline:** joske/master `9524c80d`, including C1 `653d7801` (#249), inspected on `feat/atomic-cursor-only`. Paths below are repository-relative; abbreviated `kms/*.rs` in evidence means `crates/yserver/src/kms/render/backend/kms/*.rs`, `portable/*.rs` means `crates/yserver/src/kms/render/backend/portable/*.rs`; `platform/*.rs` means `crates/yserver/src/kms/render/platform/*.rs`; `drm/*.rs` means `crates/yserver/src/drm/*.rs`; and `scene/*.rs` means `crates/yserver-render/src/kms/render/scene/*.rs`. Symbol anchors describe the current code; new interfaces are explicitly task-produced. C1 is already merged, so its adaptation is required in each affected task, with C10 auditing the result. No implementation, git writes, hardware runs or new checkout are authorized by this plan revision.

## Global Constraints

- Execute after all eight A and eight B tasks. Each C task is one independently tested commit on `feat/atomic-cursor-only`; user confirmation precedes merge. Drafting performs no git writes, hardware operations, or implementation changes.
- Rejection never changes applied state. TEST_ONLY is attribution only, never acceptance, retirement, or an event obligation. EBUSY follows #244, is never cursor failure, and waits for an actual outstanding flip before retrying.
- “When that is accepted, the output becomes SW and the scene recomposes with the software cursor.” For cursor-only EINVAL this means an **accepted explicit detach first**. A rejected detach retains HW, leaves the plane as it is, and logs once.
- HW→SW frame hide failure retries that software-cursor frame with explicit detach. If detach fails, **do not submit that frame**; retain the old primary/HW binding, then render a cursorless replacement and submit it without cursor properties.
- SW→HW frame show failure retries the cursorless frame without cursor properties, accepts one cursorless frame, retains SW mode and immediately recomposes with SW. HW-only riding-frame failure retries without cursor fields and then follows cursor-only detach recovery. Failed retry uses today's flip error path.
- “The key is (width, height, hotspot), as today's backoff signature. Pixel or animation-frame changes never re-qualify.” Each CRTC retries once per new key per topology epoch; revisiting an already rejected key in that epoch is not new qualification.
- “A's detach must be accepted before B's attach is sent.” Active A detaches in its SW-cursor scene frame; skipped/rejected frames defer transfer and retain A's HW. Disabled A uses its lifecycle disable. Drain A's outstanding event before a **separate** B attach; B remains SW until its own attach is accepted.
- Blocking lifecycle disable detaches cursor and disables primary/CRTC together. Success plus drained old events releases users; failure keeps ownership and today's error handling. Master loss/resume, every modeset and reassignment invalidate the next applied-state comparison without prematurely dropping references.
- Keep driver-size unscaled ARGB8888 buffers, accepted-state deltas, hotspot FB identity switching, user-count lifetime and newest-wanted bounded pool from A/B. Capability is per CRTC, feature-detected, with Atomic-before-hotspot-before-enumeration ordering. Ineligible devices use SW.
- Tearing On has detached intent and async commits have zero cursor properties. C1 (#249) is already merged; A/B integrate its producers and consumers before this checkpoint, and C1/C3/C5/C7 below preserve its recovery/lifecycle behavior. “Task C1” in this plan means attribution; “C1 (#249)” means the merged tearing feature. No scaling/gamma/overlay/new protocol policy changes.
- No function-body `cfg(test)` branches or new `YSERVER_*` behavior vars; production seams must have non-test callers. Keep portable/yserver-render free of KMS types. 5,000 code / 6,000 test line ceiling; split `round_c*.rs`. Preserve Linux glibc/musl/FreeBSD ioctl typing and buildability.
- Rules A/B: advance only Backend trait entries or `core_loop::run_core`, through recorded atomic requests and controllable events. C7 adds `Backend::shutdown_outputs`, used by the real `lib.rs` post-loop caller and the shutdown scenario; merely returning from `run_core` does not perform that teardown. Never invoke rejection/lifecycle/matching helpers directly or seed accepted bindings/in-flight flags. Assert **every accepted intermediate frame**, plus actual applied ownership after each rejected attempt. Cleanup via Backend hide/disable/drain leaves no actionable intents, staged/submitted/on-screen users, event obligations, or live FB allocations. After expected failure, assert preserved ownership before injecting successful teardown.
- Rule F: every new test runs 5 times under 16 busy loops; GPU-waiting tests run 20 times, with cleanup traps for worker PIDs as specified in A. Vulkan-fed scanout skips before submit on missing SYNC_FD export, using `VkContext::supports_sync_fd()` and master's `crates/yserver/tests/common/mod.rs` pattern. Record skips separately; do not fake export success.
- Rule C: no hardware before C11. Hardware requires per-card user approval and is executed with the controller. Every task, including C11, has the complete formatter/clippy/workspace/ignored-lavapipe gate before its single commit. Status documents distinguish executed results, skips and pending hardware.

- Module registration belongs to the same task and commit as each new file: test rounds modify `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/mod.rs`; platform modules modify `crates/yserver/src/kms/render/platform.rs`; cursor-plane submodules modify `crates/yserver/src/kms/cursor_plane.rs`; DRM modules modify `crates/yserver/src/drm/mod.rs`. Treat these exact parent paths as part of each applicable Files block.

## Review Focus

1. Detach rejection followed by another render tick: no accepted intermediate double cursor, even for one frame; C2/C3 test this.
2. A primary/bandwidth failure or commit-phase failure looks like cursor EINVAL: preserve correct attribution and recovery target; C1/C5 test this.
3. Unsupported animation alternates among previously rejected parameter keys: no repeated qualification storm; C6 tests this.
4. One CRTC disables successfully but a peer fails or its old event is delayed: preserve each remaining owner and do not attach a reassigned plane prematurely; C7/C9 test this.
5. Live output A loses the matched plane while no BO is available: A keeps HW until the SW-cursor frame detaches it; C8 tests this.

---

## Production-path evidence

Current non-test anchors: `drm/atomic.rs::{submit_with_tearing_context,cursor_caused_rejection}` owns cursor attribution, and `submit_tearing` supplies C1 (#249) timing/telemetry context. `platform/scene_sink.rs::{finish_frame,submit_shared_scanout_frame}`, `platform/scanout_bos.rs::submit_copied_scanout`, and `kms/scanout.rs::{submit_direct_frame_live,submit_composed_unflip}` consume rejection. The shared/direct paths first handle cursor-free Async EINVAL through their own `AsyncFlipLedger` Vsync retry; this is not cursor blame. The copied route remains Vsync. `kms/session.rs::{disable_output,run_suspend,run_resume,backend_session_set_dpms_power}`, `platform/power.rs` and `platform/connectors.rs` own lifecycle. `lib.rs` calls `run_core`, then the inherent `KmsBackend::disable_output`; C7 routes that actual post-loop caller through a Backend trait entry. `kms/crtc_config.rs::quiesce_before_topology_mutation` exits tearing before changing topology and drains old events. `portable/cursor.rs::display_cursor_by_handle` invokes `Platform::note_cursor_sprite` → `platform/cursor.rs::cursor_plane_note_sprite_hotspot`; `tick_cursor_animation` invokes display. `platform/connectors.rs` and `platform/cursor.rs::{refresh_cursor_topology_for_devices,refresh_cursor_topology,rearm_cursor}` are topology/resume seams; A8 has already replaced legacy rearm with atomic lifecycle reconciliation.

Hotplug is **not** a synchronous topology mutation: `kms/randr.rs::backend_randr_on_display_hotplug` arms a deadline; `kms/scanout.rs::backend_scanout_next_wakeup` advertises it; `kms/session.rs::backend_session_poll_deferred_input` services it through `fire_randr_changes`. Forced `Backend::reprobe_connectors` currently publishes lightweight connector probes only (`kms/randr.rs::backend_randr_reprobe_connectors`). C8 wires cursor topology reconciliation into the actual debounced hotplug/configuration path, including inventory-only changes; it cannot use a probe-only entry as a fake matching driver. C1 (#249)'s exit clears `last_cursor_compose`, `deferred_cursor_wake` and async-shape ledger; VT/DPMS/resume and window map/unmap/disconnect hooks must retain that policy and use actual cursor bindings/receipts.

Repeat `rg -n` for each named non-test caller before writing its test. Fixture observations may inspect receipt/state snapshots, but only Backend entries cause recovery, modeset, reassignment or retirement. This is especially necessary for C8's new live-reassignment route: its task is incomplete if it exists only as a helper exercised by tests.

## Shared recovery interfaces

- New `platform/cursor_recovery.rs`: `CursorFailureClass::{Busy, CursorProperties, Frame}`; `CursorFailureContext::{CursorOnly, SceneToSoftware, SceneToHardware, SceneHardwareOnly, DirectHardwareOnly}`. `CursorRecoveryAction::{WaitForFlip, DetachThenSoftware, RetryFrameWithDetach, RetryFrameWithoutCursor, DiscardAndRecomposeWithoutSoftware, FlipError}`. `classify_cursor_failure(error: &io::Error, cursor_attributed: bool, context: CursorFailureContext) -> CursorRecoveryAction`. These are internal KMS types, consumed by the production submit paths, not exported through the portable seam.
- Consume B's neutral receipt field `primary_cursor: CursorPrimaryContent`, where `CursorPrimaryContent::{Cursorless, Software { pos: (i32,i32), record_version: u64 }}` describes pixels actually accepted. It is independent of accepted ownership mode. Failed show's cursorless retry has Software ownership mode, Cursorless primary, and `needs_recompose=true`.
- C1 owns `CursorRecoveryReporter::retained_after_detach_rejection(&self, key: CrtcKey, epoch: u64, error: &io::Error)` in `cursor_recovery.rs`, its production `LogCursorRecoveryReporter`, and the new `KmsServices::cursor_recovery_reporter: Rc<dyn CursorRecoveryReporter>` dependency. Normal service construction installs the logger; `open_with_services` propagates it through platform initialization into platform-owned recovery state. C1 extends A1's fixture constructor to inject a counting reporter at that same production call site; A1 itself has no dependency on C's later type. C2 adds/tests once-per-rejected-key/epoch suppression in the recovery state, before calling the reporter; a logger that suppresses duplicates by itself cannot make the mutation pass.
- `CursorFallbackState::{Qualified, SoftwarePendingDetach, Software, RetainHardware}` per CRTC. RetainHardware is not permission to paint SW, clear leases, or loop detach attempts on motion/animation. Keep the rejected desired key as a suppressed observation rather than actionable unsent work.
- `CursorQualificationKey { width: u16, height: u16, hot_x: u16, hot_y: u16 }`; `CursorQualification { epoch: u64, rejected: HashSet<CursorQualificationKey> }`, stored per CRTC/device. A3's device-local `cursor_topology_epoch` owns qualification epoch changes; ordinary DPMS/VT/modeset invalidation alone cannot clear rejected keys. Pixel/version changes, transparent/hidden toggles and C1 (#249) On/Off changes do not clear the set; a new parameter key gets one attempt, including after a prior accepted key.
- New `platform/cursor_reassign.rs`: `CursorPlaneKey { device_key: DrmDeviceKey, plane: plane::Handle }`; `CursorReassignment { plane: CursorPlaneKey, from: CrtcKey, to: CrtcKey, detach_accepted: bool, old_events_drained: bool }`. `reconcile_cursor_assignments(&mut self, device_key: DrmDeviceKey) -> io::Result<()>` updates desired matching while retaining actual owners. `cursor_reassignment_can_attach(&self, plane: CursorPlaneKey) -> bool` requires both boundaries. It is queried by normal **B's scene frame** attachment, not a test attach API.

## Task C1: Attribute every cursor property without corrupting accepted state

**Files:**
- Modify: `crates/yserver/src/drm/atomic.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/kms/render/platform/scanout_bos.rs`, `crates/yserver/src/kms/render/backend/kms/scanout.rs`, `crates/yserver/src/kms/render/platform/services.rs`, `crates/yserver/src/kms/render/platform/init.rs`, `crates/yserver/src/kms/render/platform.rs`, `crates/yserver/src/kms/render/backend/kms/session.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/support.rs`.
- Create: `crates/yserver/src/kms/render/platform/cursor_recovery.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_c_attribution.rs`.

**Interfaces:**
- Consumes: A tagged `CursorPlaneUpdate`, B frame composition identity, existing `AtomicCommitSink::submit/test_only`; production callers listed above.
- Produces: failure context/action types and classification interface; request snapshots preserve every tagged cursor field and preserve all unrelated primary/fence fields when stripping. Cursor-free Async rejection never enters this classifier as cursor-attributed: C1 (#249) Async EINVAL fallback/shape suspension runs first, with unchanged `TearingCommitContext` and flags. A subsequent Vsync request is attributed only for cursor fields it actually included; a wanted/prepared-but-filtered update is not evidence.
- Produces: the reporter trait, logger default, `KmsServices` field, constructor propagation and counting-fixture injection specified above, all in this gated commit. Route the existing retained-HW detach-failure warning through this production dependency; C2 consumes it rather than adding an unwired test-only counter.

- [ ] **Step 1: Add `atomic_cursor_only_attribution_strips_every_cursor_field` and `atomic_cursor_only_ebusy_is_never_cursor_failure`.** Drive each shared/copied/direct/unflip path through Backend; reject requests selectively on FB_ID, CRTC_ID, each SRC/CRTC geometry field and each hotspot property. Assert failed live request → identical full TEST_ONLY rejection → cursor-stripped TEST_ONLY acceptance attributes cursor; if stripped also fails, or full TEST_ONLY succeeds, it is the frame/commit path. EBUSY issues no attribution probe, no fallback or owned event and waits for a real flip. TEST_ONLY never changes state/users. Mutations: tag X/Y only, remove a primary/fence field when stripping, blame all EINVAL, count TEST_ONLY as accepted, or flag cursor-only EBUSY as cursor failure; each fails its exact trace/state assertion.
- [ ] **Step 1a: Add `atomic_cursor_only_async_einval_is_shape_fallback_not_cursor_blame`.** Through Backend enter On on direct and shared composed routes; reject an Async request with EINVAL and accept its Vsync fallback. Assert the Async request has no cursor fields/OUT_FENCE_PTR, no cursor-attribution TEST_ONLY probe, no rejected cursor qualification key, and no cursor lease/receipt applied from work omitted by timing. Vsync fallback keeps detached target, preserves its primary/input fence and C1 (#249) async-fallback telemetry; repeated same-shape EINVAL follows existing bounded shape suspension, while EBUSY neither suspends nor disqualifies HW. Mutations: blame all EINVAL, mark Async as SceneWithCursor, or settle an omitted prepared sprite; fail attribution/ownership assertions.
- [ ] **Step 2: Run red on the real submit path's classification.** A direct call to the sink/classifier does not count as this test.
- [ ] **Step 3: Dispatch structured recovery by actual owner/composition and wire the reporter end to end.** Install the default logger in normal services, retain the injected reporter through both backend/platform constructors, and extend `AtomicCursorFixture` construction to accept the counter. Preserve original failure if the retry fails, including current per-producer flip error handling and GPU/BO quarantine. Do not classify allocation pressure as a rejected atomic request. Maintain independent state for C1 (#249) async-shape rejection and atomic-cursor qualification; tearing exit/topology reset of the former cannot silently clear the latter.
- [ ] **Step 4: Run green, each property/EBUSY mutation and F stress.** Verify failed attempts leave applied state and old on-screen users unchanged.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "fix: attribute full atomic cursor request failures"`. Do not batch this commit with the next task.

## Task C2: Detach safely after a cursor-only EINVAL

**Files:**
- Modify: `crates/yserver/src/kms/render/platform/cursor_recovery.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs`, `crates/yserver/src/kms/render/platform/cursor_state.rs`, `crates/yserver/src/kms/render/backend/kms/scanout.rs`.
- Create: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_c_cursor_only.rs`.

**Interfaces:**
- Consumes: C1 CursorOnly action **and its fully wired recovery reporter/counting fixture**, A applied/users, B receipts/scheduler.
- Produces: `CursorFallbackState` and detach-only recovery in the normal cursor lane; successful detach emits Software receipt + scene recompose, rejected detach enters RetainHardware once.

- [ ] **Step 1: Add `atomic_cursor_only_einval_detaches_before_software` and `atomic_cursor_only_rejected_detach_keeps_hardware_without_retry_loop`.** Construct through C1's services with the counting reporter, then Backend establishes HW, requests motion/image/hotspot, scripts EINVAL then detach accepted/rejected variants, and schedules repeated motion/animation/compose ticks. Assert applied FB unchanged after first rejection; detach request contains only FB_ID=0/CRTC_ID=0; Software mode/recompose begins only on detach acceptance. Old FB remains owned until the detach event. If detach fails, every accepted primary is cursorless, old HW stays bound, exactly one production reporter call is counted and repeated ticks cause neither new calls nor a new detach loop. Inject a later successful lifecycle disable for cleanup. Mutations: switch SW on initial EINVAL, omit explicit detach, free the old FB at rejection/acceptance, retry every animation tick, or call the reporter on every suppressed observation; each fails a boundary, content, or raw call-count assertion independently of logger filtering.
- [ ] **Step 2: Run red with both accepted and rejected detach scripts.** Include latest wanted image arriving during recovery; it cannot resurrect an already rejected key.
- [ ] **Step 3: Implement pending-detach versus actual ownership separately.** The detach obeys the single in-flight stream and creates an obligation only if accepted. A failed detach leaves the plane as it is; suppress the rejected work without pretending desired and applied state are equal. Do not permanently disable healthy peer CRTCs. If recovery overlaps On/exit, C1 (#249)'s detached policy still wins; accepted detach/receipt must not requeue an attach from a stale pre-On record. On retains Vsync/cursorless safety if actual HW remains bound, and C3 owns any SW-containing frame needed for a handoff.
- [ ] **Step 4: Run green, mutations and F stress.** Assert the intentionally retained owner before the successful cleanup disable.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "fix: retain hardware ownership until atomic detach succeeds"`. Do not batch this commit with the next task.

## Task C3: Reject a software-cursor frame if its plane cannot detach

**Files:**
- Modify: `crates/yserver/src/kms/render/platform/cursor_recovery.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/kms/render/platform/scanout_bos.rs`, `crates/yserver-render/src/kms/render/scene/tick.rs`, `crates/yserver-render/src/kms/render/scene/cursor_frame.rs`, `crates/yserver-render/src/kms/render/scene/flip.rs`.
- Create: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_c_frames.rs`.

**Interfaces:**
- Consumes: C1 SceneToSoftware classification, B2/B3 frame IDs, content and cancellation paths.
- Produces: `RetryFrameWithDetach` followed on failure by `DiscardAndRecomposeWithoutSoftware`; the existing B1 `CursorPrimaryContent` receipt field with truthful replacement content. The renderer can request a cursorless replacement while retaining Hardware ownership, without making a speculative SW assignment eligible.

- [ ] **Step 1: Add `atomic_cursor_only_hide_failure_never_accepts_double_cursor`.** For shared and copied outputs, render a real HW→SW primary containing the software cursor. Reject hide, attribute to cursor, then parameterize explicit-detach retry success/failure. Success accepts that same primary with detach. Failure accepts **neither** attempt, leaves the old primary/HW FB, and renders a new cursorless primary that is submitted without cursor fields. Inspect each accepted intermediate primary's pixel/cursor-content record against oracle binding; there is never SW content + bound HW. Repeat with B6's actual visible-direct→tearing handoff: cursorless grouped unflip has retired, HW is still bound, and the next scene frame paints SW+detach. Failed detach retains On with cursorless Vsync output and logs once; even a desired Detached state cannot enable Async until the actual binding is gone and its event drained. Preserve/supersede C1 (#249)'s `hide_failure_keeps_visible_cursor_frames_vsync` here with exact §3 retry assertions. Verify per-frame lease and GPU retirement cleanup. Mutations: retry original SW frame without cursor properties, claim SW while binding remains, or reuse its SW metadata on the cursorless replacement; each fails before final recovery.
- [ ] **Step 2: Run red at the first unsafe accepted frame.** A final-state-only assertion is insufficient.
- [ ] **Step 3: Keep the rejected software frame out of the accepted stream.** Explicit detach carries both zero IDs even if the original hide delta happened to look identical. If it fails too, cancel its plan, preserve primary and old users, and request an actual cursorless render before a cursor-free retry. Never edit the content annotation to pretend the old software framebuffer is cursorless.
- [ ] **Step 4: Run green, mutations and 20-repeat GPU stress.** Force both shared and deferred copied paths; cancellation must not reuse a BO before its GPU work is quiescent.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "fix: prevent double cursors after rejected frame detach"`. Do not batch this commit with the next task.

## Task C4: Recompose software immediately after a failed show

**Files:**
- Modify: `crates/yserver/src/kms/render/platform/cursor_recovery.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/kms/render/platform/scanout_bos.rs`, `crates/yserver-render/src/kms/render/scene/cursor_frame.rs`, `crates/yserver-render/src/kms/render/scene/tick.rs`, `crates/yserver-render/src/kms/render/scene/flip.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_c_frames.rs`.

**Interfaces:**
- Consumes: C1 SceneToHardware action, C3 truthful `primary_cursor`, B receipts/Mixed bookkeeping.
- Produces: failed-show retry receipt with Software ownership, Cursorless primary and immediate repaint; it cannot retire metadata that claims a SW cursor was painted in that primary.

- [ ] **Step 1: Add `atomic_cursor_only_failed_show_accepts_one_cursorless_frame_then_software`.** Through Backend establish SW, request HW, reject/attribute the show, and accept retry without cursor fields. Assert the accepted primary is the original cursorless render, mode stays SW, no cursor-plane attach occurred, and an immediate compose wake produces the next accepted SW-cursor frame without pointer motion/client damage. Include shared/copied and a pending peer HW output so aggregate mode remains truthful. Mutations: label the retry HW, claim it already contains SW, omit the immediate wake, or replay show at retirement; each fails the intermediate receipt/content/progress assertion.
- [ ] **Step 2: Run red at the cursorless retry and its next scheduling opportunity.** Do not count a later unrelated redraw as recovery.
- [ ] **Step 3: Retry the same primary without cursor properties, then recompose SW.** Preserve its real content snapshot in PendingAck; retirement cannot install an unpainted SW position/version. Keep built target leases until rejection/cancellation settlement, then release without RMFB while any other user remains.
- [ ] **Step 4: Run green, mutations and 20-repeat GPU stress.** Assert final quiescence after both cursorless and SW events drain.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "fix: recompose software after atomic cursor show rejection"`. Do not batch this commit with the next task.

## Task C5: Recover riding HW changes and preserve ordinary flip errors

**Files:**
- Modify: `crates/yserver/src/kms/render/platform/cursor_recovery.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/kms/render/platform/scanout_bos.rs`, `crates/yserver/src/kms/render/backend/kms/scanout.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_c_frames.rs`, `round_c_attribution.rs`.

**Interfaces:**
- Consumes: C1 SceneHardwareOnly/DirectHardwareOnly contexts and C2 detach recovery; B scene/direct/unflip receipts.
- Produces: retry of a rejected HW-only riding change without cursor properties, followed by normal pending-detach recovery; rejected primary retry follows the existing flip-error/BO recovery path with no false acceptance.

- [ ] **Step 1: Add `atomic_cursor_only_riding_failure_retries_primary_then_detaches` and `atomic_cursor_only_retry_failure_keeps_original_flip_error_path`.** Parameterize shared/copied/direct successor/composed unflip. Reject attributed HW update, accept cursor-stripped primary retry, then process detach. Assert old hardware binding persists across the primary retry and SW is painted only after detach acceptance. For each §3 retry (cursor-stripped show, explicit hide detach, riding frame), also fail the retry: no receipt/event obligation for rejected work, original primary/FB retained, expected current per-producer error outcome and no newly leaked BO/FB. Include unattributed bandwidth rejection and commit-phase ENOMEM. Mutations: mark retry accepted before result, enter SW immediately after stripping fields, erase primary/fence fields, or swallow the retry failure as successful presentation; each fails its first intermediate assertion.
- [ ] **Step 1a: Add `atomic_cursor_only_tearing_exit_restores_latest_intent_after_async_retirement`.** Queue newer sprite/motion intent while On and exit through a qualifying real Backend policy change with an outstanding async event. Assert no overlapping cursor work, drain that event, restore HW via an actual accepted SW→HW scene frame if needed and retire it, then request and reject a Vsync HW-only update. Assert no cursor fields/lease settlement on the old Async request, no overlapping commit before retirement, exactly-once settlement only for accepted Vsync work, and recovery uses the latest live sprite/position. Mutations: settle omitted cursor work on Async acceptance or restore a cached pre-On sprite; fail the binding/lease/identity assertion.
- [ ] **Step 2: Run red separately for success and failed-retry variants.** The direct path must actually pass through its production builder/sink.
- [ ] **Step 3: Apply owner-specific recovery while retaining the rendered primary's truth.** Do not convert a scene-owned transition to HardwareOnly to make a failed frame progress. Keep the Async→Vsync shape fallback from A6/task C1 separate from this cursor-stripped retry. While On, HW-only riding attach is suppressed before build, and an Async primary cannot reach this cursor-failure context; any Vsync transition detach recovery is still owned by its SW frame. Existing device-lost/quarantine errors remain authoritative when GPU cleanup itself fails.
- [ ] **Step 4: Run green, mutations and F stress.** Every §3 frame case has both retry accepted and retry failed evidence.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "fix: recover cursor updates without losing flip error ownership"`. Do not batch this commit with the next task.

## Task C6: Bound qualification by parameter key and topology epoch

**Files:**
- Modify: `crates/yserver/src/kms/render/platform/cursor_state.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/kms/render/platform/cursor_recovery.rs`, `crates/yserver/src/kms/render/backend/portable/cursor.rs` through its neutral sprite hook.
- Create: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_c_qualification.rs`.

**Interfaces:**
- Consumes: B4's neutral sprite hook in `portable/cursor.rs::display_cursor_by_handle`, animation display in `tick_cursor_animation`, task C1/C2 rejection results. `note_cursor_sprite` is a current-code anchor, not a requirement to keep the old hook after B4 replaces it.
- Produces: per-CRTC `CursorQualificationKey`/`CursorQualification`; topology refresh increments epoch and clears rejected-key history only for that device's new epoch. Remove retire-count/exponential requalification as the source of eligibility.

- [ ] **Step 1: Add `atomic_cursor_only_requalification_is_once_per_key_per_epoch`.** Backend submits rejected K1, many pixel/animation versions of K1, new dimensions K2, new hotspot K3, then K1→K2 revisits. Assert exactly one rejected plane attempt per distinct key, no attempts for animation/version-only updates or already-rejected revisits, even after many SW retirements. Cycle DPMS/VT and tearing On/Off with unchanged topology, plus transparent/hidden toggles, and assert no new attempt; then trigger an actual changed topology epoch via Backend hotplug/config and assert each key gets one new opportunity. Reject on output A while B accepts the same key; assert B remains eligible. Mutations: reset on record version, forget older rejected keys on a new signature, requalify after N SW frames, use device-global rejection, reset on unchanged DPMS/VT, or fail to reset on a changed epoch; each fails its attempt-count row.
- [ ] **Step 2: Run red on counts, not just final SW mode.** Keep animation progressing through Backend scheduler so the test would expose a retry loop.
- [ ] **Step 3: Replace the single current backoff signature with a rejected-key set.** Parameter changes may requalify exactly once; pixel animation cannot. Suppressed desired records are not actionable pending ioctl work, and rejected detach ownership remains truthful.
- [ ] **Step 4: Run green, key/epoch mutations and F stress.** Include no idle wakeup spin while all current keys are rejected.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "fix: bound cursor qualification retries by key and epoch"`. Do not batch this commit with the next task.

## Task C7: Complete lifecycle success, failure and partial-device accounting

**Files:**
- Modify: `crates/yserver/src/kms/render/platform/cursor_lifecycle.rs`, `crates/yserver/src/drm/modeset.rs`, `crates/yserver/src/kms/render/platform/power.rs`, `crates/yserver/src/kms/render/platform/connectors.rs`, `crates/yserver/src/kms/render/platform/flip_events.rs`, `crates/yserver/src/kms/render/backend/kms/session.rs`, `crates/yserver/src/kms/render/backend/kms/crtc_config.rs`, `crates/yserver-core/src/backend/trait_def.rs`, `crates/yserver/src/kms/render/backend/trait_impl.rs`, `crates/yserver/src/kms/render/backend/seam.rs`, `crates/yserver/src/kms/render/backend/kms/platform_hooks.rs`, `crates/yserver/src/kms/render/backend/kms/present.rs`, `crates/yserver/src/kms/render/backend/portable/windows.rs`, `crates/yserver/src/kms/render/backend/tests/tearing.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round2.rs`, `crates/yserver/src/lib.rs`.
- Create: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_c_lifecycle.rs`.

**Interfaces:**
- Consumes: A lifecycle primitives plus C recovery/qualification and actual `pending_pageflip_crtcs`/old-event drain callers.
- Produces: all disable/enable/removal/shutdown callers settle cursor state through the common sink. No separate legacy hide or accepting power-executor bypass exists in the exercised path. Consume A8's independent `recover_hidden_cursor_on_resume` hook; it cannot require direct scanout still owning a frame, since resume already clears it.
- Produces: `Backend::shutdown_outputs(&mut self) -> io::Result<()>` with a default no-op for backends without this post-loop KMS teardown. `XBackend<P>` delegates through `Platform::shutdown_outputs`; the KMS platform hook calls the existing complete `KmsBackend::disable_output` sequence (engine flush/shutdown, scene drain, platform disable/drain and direct-source settlement), rather than just the lower-level platform method. Replace `lib.rs`'s actual post-`run_core` inherent call with this trait entry, preserving subsequent Present/wake/drawable cleanup and error handling. The test calls this same trait entry after `run_core` returns; it does not call the inherent/lower-level teardown helper directly. No change to the spec's Backend/run_core rule is needed.

- [ ] **Step 1: Add `atomic_cursor_only_all_lifecycle_paths_detach_and_drain` and `atomic_cursor_only_partial_disable_failure_retains_remaining_owners`.** Parameterize Backend DPMS off/on, VT release/acquire, hotplug output removal, CRTC disable/modeset and post-loop shutdown. In the shutdown case establish a real live cursor and outstanding event via Backend, terminate the core loop, assert loop return alone emitted no shutdown disable, then call `Backend::shutdown_outputs` exactly as `lib.rs` does. Assert this route emits blocking ALLOW_MODESET detach + primary/CRTC off, no new replacing event, accepted detached applied state, and no RMFB/destruction while old events are still draining. Delay a previously accepted cursor flip into disable completion; the resource ledger must still show its live owners when the event source is first asked to drain, with reclaim only after the event is consumed. Make A's disable succeed and B's fail, with shared F0: A's success alone cannot reclaim B's user or claim total all-off. Missing old event follows current bounded fail-stop with owners retained, not forced cleanup. After Backend-driven exit from On by DPMS/VT/removal/shutdown, assert async ledger/probation and deferred cap wakes are cleared, normal current HW/SW/Hidden policy is restored only after valid resume, and fresh Async probation is required. Repeat A8's hidden-on-suspend stale-binding recovery with direct ownership cleared, and preserve `tearing/round2.rs::{hidden_cursor_while_vt_suspended_retries_failed_plane_hide_on_resume,task5_exit_restores_cursor_policy_per_state,task5_tearing_window_destroy_exits_and_restores_cursor_policy}` plus `tearing.rs::{dpms_off_exits_tearing_and_on_requires_fresh_probation,vt_release_exits_tearing_and_resume_needs_probation,topology_change_exits_tearing}`. Preserve portable `exit_tearing_for_window` callers on window map/unmap/destroy/disconnect; an unrelated window must not exit another owner. Resume/modeset sends the full state and no operation occurs while master is lost/DPMS off. Mutations: omit the Backend shutdown delegation → no shutdown disable trace; detach separately → wrong blocking request; reclaim on blocking return before drain → ledger drops live resources early; erase failed peers' users, clear expected-event sets, or omit one invalidation site → the corresponding owner/event/full-state assertion fails.
- [ ] **Step 2: Run red through Backend lifecycle dispatch.** Queue delayed events in the dependency before calling a blocking lifecycle entry, or have the event dependency release them at the controlled disable return; do not block a single-thread test on an event it cannot deliver.
- [ ] **Step 3: Wire the actual post-loop caller and consolidate lifecycle settlement.** Add the trait/platform delegation and update `lib.rs` in this commit; shutdown still executes the full existing teardown body. Route its disable, connector disable and all modeset/resume paths through the same ownership rules, preserving resources through successful blocking disable **and** old-event drain. Retain uncertain resources in existing fail-stop/quarantine paths on error; the tests then supply a successful disable/drain for leak accounting rather than pretending failed hardware released them.
- [ ] **Step 4: Run green, lifecycle/partial-failure mutations and F stress.** Check two devices with identical handles and old events so one cannot release the other's state.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "fix: preserve atomic cursor owners across lifecycle failures"`. Do not batch this commit with the next task.

## Task C8: Reassign a plane while its old output stays active

**Files:**
- Create: `crates/yserver/src/kms/render/platform/cursor_reassign.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_c_reassign.rs`.
- Modify: `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/kms/render/platform/cursor_frames.rs`, `crates/yserver/src/kms/render/backend/kms/randr.rs`, `crates/yserver/src/kms/render/backend/kms/scanout.rs`, `crates/yserver/src/kms/render/platform/connectors.rs`.

**Interfaces:**
- Consumes: A maximum matching and applied owners, B frame-owned HW↔SW, C7 lifecycle; actual hotplug deadline chain documented above.
- Produces: `CursorPlaneKey`, `CursorReassignment`, `reconcile_cursor_assignments`, attach barrier above. Desired matching no longer overwrites actual ownership.

- [ ] **Step 1: Add `atomic_cursor_only_active_reassignment_waits_for_software_frame`.** Through Backend host-hotplug notification and its normal `Backend::poll_deferred_input` timed rescan (advance B4's injected monotonic clock past the debounce deadline), present a discovery change that moves bound plane P from active A to B while retaining both primary routes. Inject A's next BO unavailable, then a primary-attributed rejected transition frame (both full and cursor-stripped TEST_ONLY fail, so it is not a new cursor-key qualification failure); run B's scheduling throughout. Assert A keeps its hardware cursor/old primary, no detach-only transaction is sent for active A, and B paints SW. Let A submit an actual SW-cursor frame: its accepted request detaches P. Before A's event, B still cannot attach. After that event, B's distinct SW→HW frame removes B's SW pixels and attaches P; only acceptance changes B's mode. No single request both transfers P from A and attaches it to B. Mutations: eagerly detach on matching, publish after skipped/rejected A frame, attach B at A acceptance before event, directly switch CRTC_ID A→B, or attach B without erasing SW; each fails cursor-coverage/order/content assertions.
- [ ] **Step 2: Run red using a production-reachable live topology route.** Existing connector reprobe alone does not run matching. Wire fresh plane discovery/reconciliation into the real debounced hotplug/configuration path, including a cursor-plane inventory change with unchanged connector/primary routes. Call the existing tearing-exit/topology invalidation contract before reconciliation, even for a cursor-inventory-only epoch; do not carry an On owner/async eligibility across reassignment. Retain cursor rejected-key history on unaffected devices and B4's shared clock for debounce. Preserve active scanout in that cursor-only topology case; do not force all CRTCs off to sidestep this required scenario. The non-test caller must be visible in `rg` before the test is accepted.
- [ ] **Step 3: Retain old actual owner until A's detach frame is accepted and drained.** The old plane's properties and leases remain available while desired matching points to B. B attaches via B's own frame-owned SW→HW plan, never a cursor-only plan stolen from a skipped frame. Invalidate applied comparisons at the reassignment boundary and preserve independent HW peers.
- [ ] **Step 4: Run green, every transfer mutation and 20-repeat GPU stress.** End by disabling both outputs through Backend and assert no transfer record, pending plan, event or sprite user remains.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "feat: defer live cursor plane reassignment until scene handoff"`. Do not batch this commit with the next task.

## Task C9: Reassign disabled outputs and drain old identities before reuse

**Files:**
- Modify: `crates/yserver/src/kms/render/platform/cursor_reassign.rs`, `crates/yserver/src/kms/render/platform/cursor_lifecycle.rs`, `crates/yserver/src/kms/render/platform/flip_events.rs`, `crates/yserver/src/kms/render/platform/connectors.rs`, `crates/yserver/src/kms/render/backend/kms/crtc_config.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_c_reassign.rs`.

**Interfaces:**
- Consumes: C8 transfer barriers, C7 successful-disable + drained-event proof, stable `CrtcKey` identity.
- Produces: disabled-A transfer completion through its lifecycle commit, including cancellation of a transfer when B disappears or a newer matching supersedes it. Settling a stale event cannot release the new owner's leases.

- [ ] **Step 1: Add `atomic_cursor_only_disabled_reassignment_uses_lifecycle_detach` and `atomic_cursor_only_reassignment_cancellation_keeps_device_identity`.** Through Backend config/removal, disable A while P is destined for B. Assert P detach in A's blocking disable, then drain old events before separate B attach. Fail A disable: retain A ownership and forbid B attach. Exercise B removal and matching retarget while waiting, plus a second card with equal raw CRTC/plane/FB handles. Assert no stale lease/event crosses device or topology lifetime, full state on the eventual new attachment, and no transfer leak. Mutations: mark transfer ready on desired topology alone, forget old event barrier on blocking success, route event by reused index/raw handle, or attach an obsolete destination; each fails its event/ordering/resource assertion.
- [ ] **Step 2: Run red through Backend configuration/hotplug and controlled old events.** No direct call to the reassignment planner, attach gate or drain helper.
- [ ] **Step 3: Unify lifecycle and live transfer settlement.** Cancellation releases only built destination references, not live source users. A successful source disable changes applied state but does not free old resources before the drain. A newer desired matching can replace a pending destination without manufacturing an intermediate accepted detach.
- [ ] **Step 4: Run green, all stale-identity mutations and F stress.** Check the next ordinary scheduling pass is idle after cleanup, rather than endlessly retrying a vanished destination.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "fix: drain old cursor plane owners before reassignment"`. Do not batch this commit with the next task.

## Task C10: Delete legacy cursor paths and finish the software gate

**Files:**
- Modify: `crates/yserver/src/kms/cursor_plane.rs`, `crates/yserver/src/kms/cursor_plane/ops.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/drm/atomic.rs`, `crates/yserver/src/kms/render/backend/tests/mod.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor.rs`, `crates/yserver/src/kms/render/backend/kms/scanout.rs`, `crates/yserver/src/drm/modeset.rs`, `docs/status.md`.
- Modify: obsolete existing cursor/scene tests and A1 legacy recording adapter.
- Create: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_c_gate.rs`.
- Modify: `crates/yserver/src/drm/page_flip.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/kms/render/backend/kms/present.rs`, `crates/yserver/src/kms/render/backend/kms/platform_hooks.rs`, `crates/yserver/src/kms/render/backend/seam.rs`, `crates/yserver-render/src/kms/render/scene/cursor.rs`, `crates/yserver/src/kms/render/backend/tests/tearing.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round2.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round3.rs`.
- Modify: `tools/check-layout.sh`, `.github/workflows/ci.yml` (its existing source-layout step already invokes the script).

**Interfaces:**
- Consumes: A–C9 production paths and evidence. Baseline literal legacy matches were grepped in `crates/yserver/src/kms/cursor_plane.rs`, `platform/cursor.rs`, and `crates/yserver-render/src/kms/render/scene/cursor.rs`; A's temporary trace seam is also a removal target.
- Produces: no legacy bind/move/hide executor, ioctl, legacy telemetry path or optimistic non-atomic branch under `crates/`. Preserve external master reproduction evidence outside the source tree.

- [ ] **Step 1: Add `atomic_cursor_only_ineligible_outputs_never_attempt_hardware` and `atomic_cursor_only_tearing_keeps_cursor_detached_and_async_clean`.** Backend-drive missing-property/no-plane/virtual-rejected-cap outputs through motion/show/hide/animation and direct admission: SW pixels with no HW cursor requests. Tearing tests drive the actual C1 Backend policy path: visible-HW entry to On uses B6's cursorless grouped-unflip then frame-owned SW+detach route; every direct/shared Async attempt has no property on a cursor plane and no OUT_FENCE_PTR, even with newer sprite/motion/hotspot intent queued; failed or pending detach keeps actual HW leases and prohibits Async. Leaving On clears pacing/async-shape state and returns to normal eligibility/frame ownership using the newest position/sprite. Include transparent/opaque cursor toggles, no-successor Vsync direct rebind+motion, and C1's cursor-only cap/client-damage bypass. These are end-to-end audit tests, not deferred first ownership implementations. Mutations: restore legacy fallback, append cursor fields to async requests, skip the successful handoff's frame-owned detach, or allow Async with a still-bound cursor; fail the trace/content checks. Use static gate proof as well as behavioral coverage.
- [ ] **Step 2: Run red on each surviving legacy branch and the merged-C1 mutations.** C1 (#249) and `backend/tests/tearing.rs`, `tearing/round2.rs`, `tearing/round3.rs` are present. Drive their real state machine, request builders and clocks through Backend/run_core; no test-only tearing mode. Require a passing migrated test or a named equivalent in A5/A6/A8/B4/B5/B6/task C1/C3/C7 for every changed legacy timing assertion before removal. Run the full existing tearing regression module in both ordinary and guarded ignored gates; report capability skips as unexecuted behavior, never inferred passes.
- [ ] **Step 3: Delete the legacy code and obsolete assertions, then enforce the gate.** `rg -n 'set_cursor2|move_cursor' crates` and `rg -n 'ShowOnRetire|HideOnRetire' crates` must both exit 1 (no matches), including comments, fixtures and dead code. A grep execution error is a failure. Put both automated no-match checks in `tools/check-layout.sh` (use its portable shell tooling; do not assume the CI host has ripgrep); `.github/workflows/ci.yml` already invokes it, so preserve that path and adjust its explanatory step text only if needed. All new/replacement cursor scenarios use Backend/core-loop progress; preserved unrelated old fixture tests are regression gates and are not credited as new rules-A/B evidence. Remove A's legacy seam after B7 baseline evidence is saved. Confirm B5/B6 removed `record_external_cursor_plane_bindings`, `sync_direct_cursor_scene_state`, `retire_external_cursor_handoff` and synchronous direct bind/refresh/hide operations; any residual references require removal or an explicitly documented non-operating replacement in their owning task. Keep the independent resume hook, receipt-based pending-transition query, transparent classification and C1 pacing hooks. The grep gates delete legacy cursor API names, not C1 behavior. Audit new code for function-body test cfg branches and new behavior env variables; run `tools/check-layout.sh` and check module sizes.
- [ ] **Step 4: Run green, mutations and F stress; update status and review the full coverage table.** Recheck the final accepted-intermediate-frame tests, all life/transfer cases and the master-red #244 evidence. Run existing Linux glibc/musl/FreeBSD CI build checks; list targets not locally available. Software completion does not claim C11 hardware validation.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "refactor: remove legacy cursor ioctls and enforce atomic-only gate"`. Do not batch this commit with the next task.

- [ ] **Deferred from A6.** Async audit: accepted-binding hidden-direct entry, and an accepted-binding EBUSY attempt with fault scripts preloaded before scenario entry; assert no cursor properties on Async and latest-intent rebind on exit.

## Task C11: Controller-led hardware validation with per-card user approval

**Files:**
- Modify: `docs/status.md`.
- Record the complete evidence in `docs/status.md`; no extra plan artifact or code change is required for this task's commit.

**Interfaces:**
- Consumes: gated C10 revision, `tools/cursor-bench/README.md`, the existing #244 benchmark procedure and logs; production executable and atomic telemetry.
- Produces: revision/card/kernel/workload-specific validation results and explicit pending coverage, recorded in one evidence commit. Hardware is a separate final controller task, not an implementer gate silently run on host devices.

- [ ] **Step 1: Prepare the concrete commands and expected observations without opening a DRM node.** Read the local benchmark recipe and retain the same workload/duration/baseline as #244. Identify each requested card by its approved device node. Present the exact session/commands, expected disruption and log location to the user before running each card. This permission comes from spec §5 and rule C; approval for a prior card does not authorize another.
- [ ] **Step 2: With approval, run manual scenario `nvidia_atomic_cursor_smoke` on card1.** Motion, drag, animated cursor, real window-entry sprite changes and a held direct frame. Assert zero core round trips over 8 ms (the #244 core-stall criterion); compare EBUSY against #244's measured **0**, and retain NONBLOCK ioctl latency numbers (baseline p99 under 0.4 ms). Record any difference instead of changing the benchmark threshold. Mutation/detection control: replay the B7 stale-position trace and a trace with an introduced >8 ms core round trip against the same acceptance criteria during report review; the controller must classify each altered trace as a failure. Never introduce a live hardware fault as a mutation.
- [ ] **Step 3: With separate approval, run manual scenario `amd_atomic_cursor_direct_plasma` on card0.** Same workloads/zero core stalls/EBUSY comparison, plus Vsync direct scanout with visible cursor and Plasma X11 window drags. Compare the same #244 motion-to-visible measure (documented AMD p99 20–30 ms) without inventing a new pass threshold. Exercise approved DPMS/VT cycles and capture complete detach/resume evidence. Detection mutation: a report missing the visible-direct or Plasma row must fail completeness review.
- [ ] **Step 4: With separate approval and an available VM, run manual scenario `virtio_atomic_cursor_host_hotspot` on kernel ≥6.8.** Confirm guest and host hotspot alignment, including hotspot-only change with unchanged pixels. Retain the capability/properties/FB-switch trace. Detection mutation: a trace with unchanged FB_ID for hotspot-only update must be rejected. If no VM is available, mark this explicitly **pending validation** in the PR/status; do not substitute an eligibility unit test for host alignment.
- [ ] **Step 5: Record evidence and run the per-task software gate.** Any observed defect returns to its responsible implementation task for a new tested fix before repeating the affected hardware row. If the user defers any card, record pending and stop hardware actions; do not claim the deferred row passed. Create exactly one documentation/evidence commit for this task after the gate; do not merge without confirmation.
- [ ] **Step 6: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "docs: record approved atomic cursor hardware validation"`. Do not batch this commit with the next task.


**Deferred from sub-plan A (controller rulings; not coverable by the narrowed fixture — must be checked here):**
- [ ] **Mode blob through a real modeset:** on each card, a RANDR mode change (and one rejected mode, if the card
  offers an invalid combination) creates the `MODE_ID` blob, commits with it, and releases it after both the accepted
  and the rejected commit. Evidence: the yserver log (blob create/destroy lines), no growing blob count after 10 mode
  changes.
- [ ] **Scanout qualification worker services:** the re-exec'd qualification worker runs its TEST_ONLY commits and
  mode blobs through the production services (startup log shows the qualification result per CRTC; no fallback
  caused by a service mismatch).
- [ ] **Hotplug rediscovery:** unplug and replug one monitor. After the 150 ms debounce yserver rediscovers exactly
  once (one rescan log line per edge), the cursor plane re-qualifies, and the cursor reappears on the replugged output.

## Spec coverage table

This is the complete cross-plan ledger, repeated so this plan can be reviewed independently. Task IDs prefixed **C** are owned here; other prefixes refer to the ordered sibling plans. A/B checkpoints are working increments; final ownership/error/deletion requirements are complete only after their listed later tasks. Each §1–§4 bullet/sub-bullet and every numbered §4 test has an explicit owner; §5 is included for the final controller task.

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
