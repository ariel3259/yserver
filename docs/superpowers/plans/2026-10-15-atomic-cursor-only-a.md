# Atomic Cursor Only A: Capability and Framebuffer Foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Execute inline; the controller's instruction forbids subagents.

**Goal:** Establish per-CRTC eligibility, owned sprite framebuffers, and a working full-state atomic cursor path before changing scene transition ownership.

**Architecture:** Keep the scene's existing conservative, two-phase retirement handoff (including C1's external handoff) during this sub-plan. Replace its KMS show/hide implementation with full atomic transactions, using the same per-CRTC scheduler and explicit framebuffer leases; the callbacks remain a temporary adapter until plan B. A successful checkpoint has working cursors and lifecycle teardown, without depending on unimplemented B/C tasks.

**Tech Stack:** Rust workspace, existing drm/drm-ffi, dumb buffers/AddFB2, yserver-render scene/output seam, recording atomic executor, controlled DRM events, Vulkan/lavapipe for ignored tests.

**Spec:** `docs/superpowers/specs/2026-10-15-atomic-cursor-only-design.md` (approved by the controller; its stale “draft” label does not reopen design). Read `/home/ariel_santangelo/Projects/claude-scratch/atomic-cursor/spec-review-r{1,2,3,4}.md` for the failure cases behind the requirements.

**Revalidation baseline:** joske/master `9524c80d`, including C1 `653d7801` (#249), inspected on `feat/atomic-cursor-only`. Paths below are repository-relative; abbreviated `kms/*.rs` in evidence means `crates/yserver/src/kms/render/backend/kms/*.rs`, `portable/*.rs` means `crates/yserver/src/kms/render/backend/portable/*.rs`; `platform/*.rs` means `crates/yserver/src/kms/render/platform/*.rs`; `drm/*.rs` means `crates/yserver/src/drm/*.rs`; and `scene/*.rs` means `crates/yserver-render/src/kms/render/scene/*.rs`. Symbol anchors describe the current code; new interfaces are explicitly task-produced. C1 is already merged, so its adaptation is required in each affected task, with C10 auditing the result. No implementation, git writes, hardware runs or new checkout are authorized by this plan revision.

## Global Constraints

- Execute A → [B](2026-10-15-atomic-cursor-only-b.md) → [C](2026-10-15-atomic-cursor-only-c.md), on `feat/atomic-cursor-only`. Each task is one independently gated commit. Do not merge without the user's confirmation. This document authorizes no execution during plan drafting.
- “A commit carries only the cursor properties that differ from the applied state.” Applied means the last **accepted** state; visible old framebuffer ownership survives until replacement retirement.
- “A buffer is rewritten, and its framebuffer removed (RMFB), only at user count zero.” Count every distinct CRTC with built, submitted, or still-on-screen use. Keep the old and new references across a replacement.
- “The pool starts with two buffers per device and may grow to one per CRTC plus one.” No free buffer means retain only the newest wanted image, with no live-buffer write.
- `DRM_CLIENT_CAP_ATOMIC` precedes `DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT`, which precedes plane enumeration. Linux 6.8 is historical context, never an eligibility predicate. Physical-driver cap rejection is normal.
- Virtual driver names are exactly `virtio_gpu`, `vmwgfx`, `qxl`, `vboxvideo`; require cap acceptance and both hotspot properties. Missing/unassigned planes use SW, including Apple DCP. Preserve #246 maximum matching and exclusive-plane preference.
- Final ownership rules are binding: frame-owned HW↔SW, cursor-only HW changes/held Vsync direct, blocking lifecycle detach; plan A's retirement adapter is explicitly removed by B. No new legacy fallback is introduced.
- No `cfg(test)` branches inside production function bodies, and no new `YSERVER_*` behavior environment variables. Injectable production dependencies have actual non-test callers. Keep portable code free of DRM/platform types and yserver-render free of KMS dependencies.
- `tools/check-layout.sh` sets ceilings of **5,000 code lines / 6,000 test lines**. Create focused modules and `tests/atomic_cursor_only/round*.rs`; do not enlarge `backend/tests/atomic_cursor.rs` into a catch-all. C1 already has `backend/tests/tearing.rs`, `tearing/round2.rs` and `tearing/round3.rs`; preserve that split and use focused new rounds for this suite.
- Preserve Linux glibc, Linux musl, and FreeBSD ioctl buildability; do not introduce `libc::Ioctl` request typing. No new dependency is required. Gamma/LUT, overlay policy, scaling and new cursor protocol features are out of scope.
- Update `docs/status.md` during implementation checkpoints, accurately distinguishing interim behavior, deterministic evidence, skips, and hardware validation. Drafting itself writes only plan documents.
- Rules A/B: every behavioral test advances **only through `Backend` trait entries or `core_loop::run_core`**. Construction may inject device/allocator/event dependencies, but may not seed accepted cursor state, fake scene modes/in-flight flags, call a submission/retirement helper, or invoke a matching function as the tested action. End each scenario through Backend-driven hide/disable/drain: no actionable pending intents, staged leases, event obligations, or live framebuffer allocations. Expected retained HW ownership after an injected failure must be asserted before a later successful teardown.
- Rules C/F: no hardware in implementation tasks. Run each new test 5 times under 16 busy loops; tests waiting on GPU work run 20 times. Vulkan-fed scanout tests are ignored tests and skip **before allocation/submission** when `VkContext::supports_sync_fd()` is false, following `crates/yserver/tests/common/mod.rs::skip_without_sync_fd`; report skips separately from executed passes. Never emulate successful SYNC_FD export.
- Every task ends with the literal gate below. Format first with `cargo +nightly fmt` as AGENTS.md requests; retain the controller's `cargo fmt --all` gate too. Run the additional exact default-feature CI clippy command before committing.

- Module registration belongs to the same task and commit as each new file: test rounds modify `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/mod.rs`; platform modules modify `crates/yserver/src/kms/render/platform.rs`; cursor-plane submodules modify `crates/yserver/src/kms/cursor_plane.rs`; DRM modules modify `crates/yserver/src/drm/mod.rs`. Treat these exact parent paths as part of each applicable Files block.

## Review Focus

1. Two cards reuse raw CRTC/plane/FB handles: scope every cache, lease and event by device; A3/A7 test this.
2. A cursor shrinks after a large image, with padded pitch: transparent padding and full-buffer geometry remain correct; A4 tests this.
3. Allocation/AddFB2/map failure during a live image update: no partially published sprite or lost old binding; A4/A5 test this.
4. An event arrives immediately after an accepted retirement-time show: it retires exactly that transaction, once; A5 tests this.
5. Headless → active → zero outputs → active: reevaluate eligibility without resurrecting old state or losing initialization retry; A3/A8 test this.

---

## Production-path evidence and test discipline

The following were grepped **before** choosing the tests. Recheck these caller edges when implementing; a new helper alone is not proof of a reachable path.

| Evidence | Non-test caller chain inspected | Tests that use it |
|---|---|---|
| P-open | `crates/yserver/src/platform/drm.rs` and `crates/yserver/src/kms/outputs.rs` call `Device::open`; `drm/device.rs::Device::open` negotiates UniversalPlanes/Atomic; `platform/init.rs::open_with_commit` calls `core_platform_init`, creates pools and initializes cursor state | A1–A3 construction traces, followed by Backend hotplug/poll/compose |
| P-hotplug | `crates/yserver/src/kms/hotplug.rs::DrmHotplugMonitor` drains udev; `kms/randr.rs::backend_randr_on_display_hotplug` arms debounce; `kms/session.rs::backend_session_poll_deferred_input` services it via `fire_randr_changes` | A1/A3; C6/C8/C9 |
| P-pointer | `portable/pointer.rs::update_pointer_window` → `portable/cursor.rs::refresh_effective_cursor` → `display_cursor_by_handle`; `tick_cursor_animation` also calls display | A1, A4–A7, B4/B7 |
| P-scene | `kms/scanout.rs::backend_scanout_maybe_composite` → `compose_tick` → `platform/scene_sink.rs::finish_frame`/`submit_shared_scanout_frame`; copied route calls `submit_copied` → `platform/scanout_bos.rs::submit_copied_scanout` | A4–A8, B1–B3 |
| P-cursor | `kms/scanout.rs::backend_scanout_maybe_composite` submits the idle/held-direct cursor lane; `backend_scanout_on_page_flip_ready` calls `record_atomic_cursor_commit_retirement` before direct/scene retirement | A5–A8, B4–B6 |
| P-direct | `kms/scanout.rs::{submit_direct_frame_live,submit_composed_unflip,retire_direct_output,bind_direct_cursor_on_all_outputs,refresh_direct_cursor_on_all_outputs,sync_direct_cursor_scene_state}`; `kms/platform_hooks.rs::{refresh_direct_cursor,hide_cursor_under_direct_scanout}`; `scene/cursor.rs::record_external_cursor_plane_bindings` records observed bindings | A4–A6; B5/B6/B8 replace direct operations and observation sync |
| P-tearing | `kms/present.rs::{note_tearing_present,exit_tearing}`; `kms/scanout.rs::{prepare_tearing_scene_pass,direct_frame_flip_timing,tearing_cursor_compose_cap_applies}`; `scene/cursor.rs::retire_external_cursor_handoff`; `drm/page_flip.rs::submit_scene_flip_with_fences_tearing` and `drm/modeset.rs::submit_direct_scanout` filter Async cursor fields | A1/A5/A6, B2/B4/B6/B8, C1/C3/C5/C10 |
| P-life | `kms/session.rs::{disable_output,run_suspend,run_resume,backend_session_set_dpms_power}`; `kms/crtc_config.rs`; `platform/connectors.rs`; `platform/power.rs`; resume invokes `Platform::recover_hidden_cursor_on_resume` independently of direct ownership | A5/A8, C7–C9 |
| P-backend | `backend/trait_impl.rs` delegates pointer/events/scheduler/VT/hotplug/hide/warp/DPMS through `backend/seam.rs::Platform`; `yserver-core/src/core_loop/run.rs` supplies live callers | All rounds |

New tests use prefix `atomic_cursor_only_`. The fixture owns a unified operation log: device initialization, allocation/write/AddFB2/RMFB, atomic attempts/results/flags/events, legacy cursor operations while they exist, and accepted primary content (software cursor present/absent). Kernel binding in the oracle changes only on an accepted non-TEST_ONLY request; a recorded attempt alone is not acceptance. The framebuffer oracle independently tracks bytes and live users, rather than trusting the implementation's count.

For red/green runs select the names in each task using `cargo test -p yserver --locked --features xdmcp atomic_cursor_only_ -- --nocapture`; use the lavapipe command in the gate with the same filter and `--ignored` for GPU tests. An absent test/symbol is an initial red, but before committing each stated mutation must produce the stated behavioral assertion failure and be reverted.

Stress execution: start exactly 16 background shell `while :; do :; done` workers, save their PIDs and install an EXIT/INT/TERM cleanup trap. Invoke the selected non-GPU command five times and the selected ignored lavapipe command twenty times, aborting on the first failure; terminate and wait for all workers even on failure. Keep the repetitions and skip counts in the task evidence. Do not add a stress behavior switch to the server.

## Shared interface decisions

Names below are proposed interfaces, not implementation bodies. Use existing `CrtcKey { device_key, crtc }`; raw DRM handles alone are never ownership identities.

- `drm/client_caps.rs` defines `CursorDeviceCaps { driver_name: String, hotspot_accepted: bool }`. `kms/cursor_plane/topology.rs`: `CursorPlaneProperties` contains plane handle and handles for `FB_ID`, `CRTC_ID`, `CRTC_X/Y/W/H`, `SRC_X/Y/W/H`, plus `hotspot: Option<(property::Handle, property::Handle)>`. `CursorPlane::properties_for(crtc: crtc::Handle) -> Option<&CursorPlaneProperties>` is the per-CRTC eligibility query.
- `kms/cursor_plane/sprites.rs`: `CursorSpriteId(u64)`, `CursorUse::{Built, Submitted, OnScreen}`, and a non-Copy `CursorSpriteLease` identify a device-local framebuffer, owning CRTC and use role. `CursorSpritePool::prepare(&mut self, key: CrtcKey, record: &CursorRecord) -> io::Result<Option<CursorSpriteLease>>`: `None` is pool pressure, not driver rejection. `CursorSpriteLease::framebuffer(&self) -> framebuffer::Handle`. Pool metadata deduplicates users by CRTC across roles; dropping one role cannot drop another's user.
- `platform/cursor_state.rs`: `CursorBinding::{Detached, Attached { sprite: CursorSpriteId, x: i32, y: i32, hot_x: u16, hot_y: u16 }}`. `CursorCrtcState` separates `applied: Option<CursorBinding>` (`None` means invalid), wanted record/position, built/submitted leases, and on-screen/retiring leases. Accepted state is not an on-screen release boundary.
- `drm/atomic.rs`: `CursorPlaneUpdate { device_key: DrmDeviceKey, crtc: crtc::Handle, plane: plane::Handle, properties: Vec<AtomicPropertyUpdate>, motion_timestamp: Option<Instant> }`; `AtomicCommitRequest::add_cursor_update(&mut self, update: &CursorPlaneUpdate)` tags **every** cursor field for stripping/attribution. Preserve the existing executor/sink and commit flags.
- `platform/cursor_commit.rs`: `prepare_cursor_commit(&mut self, output_idx: usize) -> io::Result<Option<PreparedCursorCommit>>`; `accept_cursor_commit(&mut self, commit: PreparedCursorCommit)`; `reject_cursor_commit(&mut self, commit: PreparedCursorCommit)`; `retire_cursor_commit(&mut self, key: CrtcKey)`. `PreparedCursorCommit` owns the delta, target binding, built lease, prior binding identity, and intent generation. Empty/equal work returns `None` and is consumed. Accepting an older generation cannot erase a newer wanted generation.
- `platform/cursor_lifecycle.rs`: `cursor_disable_updates(&self, key: CrtcKey) -> Vec<CursorPlaneUpdate>` and `cursor_disabled_and_drained(&mut self, key: CrtcKey)`. The first does not mutate ownership; the second requires successful blocking disable **and** drained old events. Invalidation preserves leases until proven safe.

## Task A1: Make complete production cursor operations observable

**Files:**
- Modify: `crates/yserver/src/drm/atomic.rs`, `crates/yserver/src/drm/mod.rs`, `crates/yserver/src/drm/modeset.rs`, `crates/yserver/src/kms/cursor_plane.rs`, `crates/yserver/src/kms/hotplug.rs`, `crates/yserver/src/kms/outputs.rs`, `crates/yserver/src/kms/render/backend/for_tests.rs`, `crates/yserver/src/kms/render/backend/kms/randr.rs`, `crates/yserver/src/kms/render/backend/kms/scanout.rs`, `crates/yserver/src/kms/render/backend/kms/session.rs`, `crates/yserver/src/kms/render/backend/kms/state.rs`, `crates/yserver/src/kms/render/backend/tests/mod.rs`, `crates/yserver/src/kms/render/backend/tests/present_scanout.rs`, `crates/yserver/src/kms/render/backend/tests/tearing.rs`, `crates/yserver/src/kms/render/platform.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/kms/render/platform/flip_events.rs`, `crates/yserver/src/kms/render/platform/init.rs`, `crates/yserver/src/kms/render/platform/tests.rs`.
- Create: `crates/yserver/src/drm/io.rs`, `crates/yserver/src/kms/cursor_plane/ops.rs`, `crates/yserver/src/kms/render/platform/services.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/mod.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/support.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_a.rs`.

**Interfaces:**
- Consumes: the existing Vk-backed `KmsBackend::for_tests_with_vk*` fixtures, `AtomicCommitExecutor`, `PageFlipEventSource`, injected cursor operations, and `Backend::{warp_pointer_root, maybe_composite, on_page_flip_ready, set_cursor_hidden, on_display_hotplug, poll_deferred_input}`. Behavioral tests advance through Backend or `run_core`; fixture helpers only configure existing test state and expose operation recordings.
- Produces: `AtomicCursorFixture` in `tests/atomic_cursor_only/support.rs`, layered on the existing Vk fixtures with a recording atomic executor, controllable flip events, and recording cursor/mode-blob operations selected by `KmsServices::for_tests`. Production continues through the same cursor, scene, event, hotplug, and shared modeset paths. The test service factory selects recording cursor, atomic, and mode-blob operations; the fixture does not inject startup or scanout operations or start a server on a non-DRM fd.
- Produces: cursor buffer raw operations in `kms/cursor_plane/ops.rs` (`create_dumb`, `map_dumb`, `add_argb_framebuffer`, `remove_framebuffer`, `unmap_dumb`, `destroy_dumb`), plus legacy bind/move/hide. Cursor-plane resources, properties, and driver dimensions remain direct DRM queries in production; the fixture supplies synthetic cursor topology at its test-constructor boundary. `CursorBuffer::allocate`, upload, rollback and release remain shared production algorithms; the recorder stores raw resource identities and an independent live-resource ledger.
- Produces: `ModeBlobIo::{create_property_blob, destroy_property_blob}` remains selected through `KmsServices`. Since this task does not construct or modeset outputs, the fixture asserts recorder selection and an empty mode-blob ledger at its boundary; it does not claim request-identity or release-after-rejection coverage.
- Produces: `DisplayHotplugSource::{raw_fd, drain_changes}` and controlled page-flip events remain constructor-selected. The hotplug test asserts that an empty recording source leaves Backend without a debounce deadline and that a queued edge arms the 150 ms deadline. It does not expire the deadline or reprobe connectors, because KMS discovery is outside this fixture.
- Do not add device-open/control injection, KMS resources/connectors/encoders/planes/properties discovery injection, primary-buffer or scanout-slot injection, GBM or PRIME import injection, scanout AddFB2/RMFB, GEM close, or a fake KMS startup path. `KmsServices::for_tests` uses recording cursor, atomic, mode-blob, hotplug, and page-flip operations only.
- The direct-successor migration uses Backend direct admission with an explicitly prequalified synthetic M1 fixture entry, then asserts the accepted shared atomic request references that qualified framebuffer. The test does not claim M1 qualification/import coverage.

- [ ] **Step 1: Add `atomic_cursor_only_trace_covers_backend_motion_and_retirement`, `atomic_cursor_only_hotplug_source_arms_backend_debounce`, and `atomic_cursor_only_mode_blob_lifetime_uses_shared_modeset`.** The cursor trace composes and retires through Backend, then moves through Backend and asserts the exact recorded atomic sequence, legacy bind/move/hide sequence, no pending cursor intent or in-flight event obligation, and an empty cursor-resource ledger after teardown. The hotplug test dispatches with no source edge, queues one recording edge, then verifies Backend advertises the debounce deadline. The mode-blob test verifies service selection at the fixture boundary and no fixture-owned blobs. Each test comment names its mutation: bypass the atomic executor; omit legacy move recording; ignore the injected hotplug source; or select production mode-blob operations.
- [ ] **Step 2: Run each new test red before implementation.** Capture the failing operation assertion, not an incidental setup/ioctl failure. Keep the command and failing output in the A1 report.
- [ ] **Step 3: Complete the fixture and remove the direct-frame submitter bypass.** Keep raw cursor operations, atomic commits, mode blobs, hotplug events and page-flip events behind construction-selected services. Leave cursor allocation/upload, request construction, scheduling, retirement, and shared modeset behavior in their existing production algorithms. Remove `direct_frame_submitter`, `submit_direct_frame_for_tests`, and `for_tests_with_direct_submitter`; migrate `async_fullscreen_direct_successors_coalesce_until_flip_retirement` through `Backend::try_present_direct` and Backend page-flip entries. Do not extend A1 into KMS startup/discovery or scanout OS-operation injection.
- [ ] **Step 4: Run green, each named cursor/hotplug/service-selection mutation once and revert it, then stress each test under 16 busy workers.** Run five loaded repetitions per test and twenty for the GPU-waiting Vk cursor trace. The brief's `MODE_ID=0` and missing-release-after-atomic-rejection mutations require a modeset path, which this ruling excludes; report them as not executable and assert recording-service selection plus an empty fixture blob ledger instead.
- [ ] **Step 5: Run the task gate.** Format with `cargo +nightly fmt`; then build, run the default and whole ignored workspace suites under lavapipe, related integration tests and ignored render acceptance, workspace clippy with `-D warnings`, and `tools/check-layout.sh`. No hardware runs or git writes.

---

## Task A2: Negotiate and retain hotspot capability before discovery

**Files:**
- Modify: `crates/yserver/src/drm/device.rs`, `crates/yserver/src/platform/drm.rs`, `crates/yserver/src/kms/outputs.rs`, `crates/yserver/src/kms/render/platform/init.rs`.
- Create: `crates/yserver/src/drm/client_caps.rs`, `crates/yserver/src/kms/cursor_plane/topology.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_a.rs`.

**Interfaces:**
- Consumes: A1's construction I/O trace, `Device::open`, existing device driver-name query; evidence P-open/P-backend.
- Produces: `Device::cursor_caps(&self) -> Option<&CursorDeviceCaps>`, populated at real KMS open; render-node/inherited-probe constructors do not negotiate KMS capabilities; `negotiate_client_caps(device: &Device, io: &dyn DeviceControlIo) -> io::Result<CursorDeviceCaps>`. Extend A1's existing `DeviceControlIo` with `driver_name(&self, device: &Device) -> io::Result<String>`; its existing `set` method submits capability ioctls. No trait-object downcast or test-only capability branch is needed. Handle the hotspot uAPI value with the existing dependency's supported API or a small portable request wrapper; verify local headers/dependency source at implementation time.

- [ ] **Step 1: Add `atomic_cursor_only_caps_precede_plane_enumeration`.** Inspect the production construction trace, then advance through Backend reprobe/compose. Assert Atomic → hotspot-cap → first plane enumeration, one negotiated result per fd, no kernel-version query, and normal physical-driver rejection. Parameterize accepted/rejected cap and construction of an inherited probe fd (which must not negotiate again). Mutations: swap Atomic/hotspot, enumerate first, or renegotiate an inherited probe fd; each corresponding trace/eligibility assertion fails.
- [ ] **Step 2: Run red and confirm the missing hotspot request/order assertion.** Construction is the real production initialization with injected syscalls, not a direct test call to the negotiation helper.
- [ ] **Step 3: Persist the result and wire all device-open callers.** Query the actual kernel driver name, not a PCI vendor string. Do not make a render-node opener acquire master or set KMS caps. Cap rejection does not fail physical-device startup. The active server retains its cached result for eligibility. Inherited disposable-probe fds reuse kernel capabilities without renegotiation and return `None` from the new getter; those helpers do not discover/program cursor planes. Keep their existing constructor signature. A cursor discovery caller without a negotiated result is ineligible, never an optimistic HW probe.
- [ ] **Step 4: Run green, each order/duplicate-negotiation mutation, and F stress.** Compile the new ioctl wrapper on glibc, musl and FreeBSD using the repository's existing cross-build/CI target setup; record unavailable local targets as unexecuted, never as passes.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "feat: negotiate atomic cursor hotspot capability"`. Do not batch this commit with the next task.

## Task A3: Select atomic eligibility independently for each CRTC

**Files:**
- Modify: `crates/yserver/src/kms/cursor_plane.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/kms/render/platform.rs`, `crates/yserver/src/kms/render/platform/connectors.rs`.
- Modify: `crates/yserver/src/kms/cursor_plane/topology.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_a.rs`.

**Interfaces:**
- Consumes: A2 `CursorDeviceCaps`; existing #246 matcher; evidence P-open/P-life.
- Produces: `CursorPlaneProperties`, `CursorPlane::properties_for`, and output-local availability/fits queries. Replace the device-wide `atomic_cursor`/`topology_blocked` decision with per-CRTC qualification; keep headless initialization retry state device-local. Add device-local `cursor_topology_epoch: u64`, advanced on an actual cursor/CRTC topology change; a lightweight probe or unchanged DPMS/VT cycle is not a new epoch.

- [ ] **Step 1: Add `atomic_cursor_only_eligibility_matrix` and `atomic_cursor_only_topology_rechecks_only_affected_device`.** Queue changed raw discovery snapshots **and** an edge in A1's injected `DisplayHotplugSource`, dispatch `Backend::on_display_hotplug`, wait for its advertised real debounce deadline, then call `Backend::poll_deferred_input` and compose. Assert rediscovery occurs only after the consumed edge/deadline; snapshots alone and ordinary `reprobe_connectors` do not trigger plane enumeration. Remove each of the ten required properties individually; include no plane, unassigned CRTC, all four virtual names with exposed plane + rejected cap, cap accepted + either hotspot property missing, virtual positive with both, and physical rejected cap with all ordinary properties. Assert SW only on affected outputs; HW-eligible peers remain usable, matching is unique/maximum and exclusive planes retain preference. Include two devices with identical raw handles and headless → active → zero → active. Mutations: require only X/Y, use a device-wide all-or-none gate, allow virtual rejected-cap planes, or reuse stale discovery; each fails its row.
- [ ] **Step 2: Run red with named failing matrix rows.** Do not test the matcher directly: snapshots must be consumed by Backend lifecycle/reprobe entries before observed cursor assignment.
- [ ] **Step 3: Rediscover property handles and matching at every topology epoch.** Preserve `effective_cursor_hidden` (missing/transparent static cursor and all-transparent animation are hidden; one opaque animation frame makes the animation visible) before eligibility decisions. No no-plane optimistic legacy probe and no non-atomic legacy motion route may remain reachable for an ineligible output. Leave legacy definitions for later deletion, not as fallback. Do not discard a live old assignment merely because desired matching changes; until C8 introduces active transfers, use the existing safe all-off topology path.
- [ ] **Step 4: Run green, matrix mutations and F stress.** Verify negative rows paint SW through an accepted scene, rather than merely returning a false capability getter.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "feat: qualify atomic cursor planes per crtc"`. Do not batch this commit with the next task.

## Task A4: Allocate and stage full-size ARGB sprite framebuffers

**Files:**
- Create: `crates/yserver/src/kms/cursor_plane/sprites.rs`.
- Modify: `crates/yserver/src/kms/cursor_plane.rs`, `crates/yserver/src/kms/cursor_plane/ops.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/kms/render/backend/kms/scanout.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver-render/src/kms/render/scene/sink.rs`, `crates/yserver-render/src/kms/render/scene/cursor.rs`, `crates/yserver-render/src/kms/render/scene/tests/mod.rs`, `crates/yserver/src/surface/platform.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_a.rs`.

**Interfaces:**
- Consumes: A1 buffer I/O, A3 dimensions/eligibility, production `CursorRecord`; evidence P-pointer/P-scene and the current `scene/cursor.rs::apply_cursor_transition_on_retire` upload caller and C1's direct refresh upload in `kms/scanout.rs::refresh_direct_cursor_on_all_outputs`.
- Produces: pool/lease interfaces in the shared decisions; `CursorUploadOutcome::{Ready, Deferred}` and `CursorPlaneSink::upload(&mut self, output_idx: usize, version: u64, width: u32, height: u32, bgra_bytes: &[u8]) -> io::Result<CursorUploadOutcome>` replacing the old success-only return. A deferred upload preserves its newest wanted record and current binding; it does not qualify as a driver rejection or a successful show.
- Compatibility invariant: migrate every `CursorPlaneSink` implementation and upload caller in A4, including KMS `platform/scene_sink.rs`, `SurfacePlatform`, and the render test sink in `scene/tests/mod.rs`. Migrate `PlatformBackend::cursor_plane_upload_image_for_output` and C1's direct refresher too: only Ready authorizes a bind; Deferred retains old actual plane/scene mode and newest wanted record, and is retried through Backend scheduling without pretending a new sprite was shown or disqualifying HW. Immediate successful uploads return Ready; only actual pool pressure returns Deferred. Preserve surface native cursor behavior. Verify all implementors/callers with `rg` before this task's compile/full-suite gate; no signature migration is deferred to A5 or B.

- [ ] **Step 1: Add `atomic_cursor_only_full_buffer_argb_upload_and_cleanup`, `atomic_cursor_only_failed_sprite_allocation_preserves_prior_image`, and `atomic_cursor_only_malformed_cursor_source_stops_at_request_validation`.** Valid Backend cursor creation/definition and compose/events invoke the production upload path. Assert two initial allocations per active device, ARGB8888 AddFB2, driver cursor-cap dimensions (including valid values below 64, with 64×64 fallback only when queries are unavailable/invalid), top-left pixels, transparent remainder and padded rows, including a smaller second sprite. Inject raw create failure (nothing to release), map failure (destroy dumb), and AddFB2 failure (unmap then destroy dumb), including failure of the second initial allocation; assert previously completed candidates and partial acquisitions are released once and wanted data remains retryable. Mutations: allocate sprite-sized buffers, skip transparent clear, publish before AddFB2 success, omit unmap after AddFB2 failure, or omit destroy after map failure; each executes in the shared allocation/upload algorithm and fails the independent byte/resource ledger.
- [ ] **Step 1a: Keep malformed-input evidence at its reachable boundary.** Through a client served by `core_loop::run_core`, create a depth-32 2×2 pixmap and compatible GC. First send a valid ZPixmap PutImage (`left_pad=0`, destination `(0,0)`, 16 known pixel bytes); assert the Backend PutImage call and exact readback, proving drawable/GC/target/depth validation succeeds. Reuse those IDs and metadata for a short request with 12 different pixel bytes; its wire length is 9 four-byte units (4-byte header + 20-byte fixed body + 12-byte payload), matching the bytes actually sent, so framing/minimum-length validation passes. Exercise `core_loop/process_request/drawing.rs::handle_put_image`'s existing ZPixmap length guard: after a subsequent reply-producing synchronization request completes, assert no X11 error for the short PutImage's sequence/opcode 72, no additional Backend PutImage call, unchanged source pixmap/current cursor, and no cursor upload/pool write. Assert the actual ignored-request behavior rather than changing protocol policy. A subsequent normal RENDER CreateCursor/readback of that pixmap yields complete, unchanged pixels; do not define that new cursor as a side effect of the invalid update. Zero-size PutImage is likewise a request-validation/no-op case. A well-formed source larger than the cursor cap is tested through Backend admission/fallback, before pool upload. Valid ARGB readback yields exactly width × height × 4 bytes and `CursorRecord::new` asserts that invariant; do not fabricate a truncated record, call pool preparation directly, or claim malformed source requests exercised pool byte-length validation. These validation/admission checks are distinct from the valid-record pool tests above.
- [ ] **Step 1b: Demonstrate the malformed test's own length-guard mutation.** Temporarily mutate only the `if request.data.len() < expected_len` branch in the ZPixmap arm of `drawing.rs::handle_put_image`: replace its silent `return Ok(RequestOutcome::Handled)` with `return emit_x11_error(state, client_id, sequence, x11::error::BAD_LENGTH, 0, 72)`. Run `atomic_cursor_only_malformed_cursor_source_stops_at_request_validation` alone (using its guarded ignored command if GPU-fed); require its no-X11-error assertion to fail on BadLength for that exact short-request sequence/opcode, with the valid control upload still succeeding. An earlier drawable/GC rejection, timeout, panic, unrelated test failure or capability skip does not demonstrate this mutation. Restore the guard and require the same test to pass; record both outputs. The allocation/upload mutations in Step 1 cannot substitute for this proof; the temporary protocol-error mutation is reverted, with no permanent drawing-policy change.
- [ ] **Step 2: Run red on geometry/AddFB2/lifetime assertions.** Existing request validation may already pass; its required behavioral red is the isolated Step 1b mutation. GPU-fed cases use the guarded ignored suite; allocator-only failures still enter via Backend operations.
- [ ] **Step 3: Make pool preparation the real upload implementation and migrate all upload implementations/callers together.** In this intermediate task the existing legacy binding and its existing upload path remain authoritative; pool preparation stages an independently allocated candidate at that same real upload boundary. Use A1's shared `CursorBuffer::allocate` algorithm with raw I/O injection for acquisition/rollback; the fixture never performs cleanup on the algorithm's behalf. AddFB2 candidate allocations are released on cancellation and do not replace a visible image merely by being prepared. Keep preparation idempotent so repeated retirement retries do not allocate unbounded candidates. A5 replaces the old single-buffer upload/binding together; A4 does not claim to fix the existing legacy on-screen lifetime defect by itself.
- [ ] **Step 4: Run green, each allocation/upload mutation plus the isolated length-guard mutation, and F stress.** Revert all mutations before the gate. Teardown asserts allocations = removals + rolled-back partial creations; no remaining mappings/dumb handles. A4 does not claim submitted/on-screen lease coverage; that begins in A5.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "feat: stage owned cursor sprite framebuffers"`. Do not batch this commit with the next task.

## Task A5: Replace KMS cursor binding with owned full-state atomic commits

**Files:**
- Create: `crates/yserver/src/kms/render/platform/cursor_state.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs`, `crates/yserver/src/kms/render/platform/cursor_lifecycle.rs`.
- Modify: `crates/yserver/src/drm/atomic.rs`, `crates/yserver/src/drm/modeset.rs`, `crates/yserver/src/kms/render/platform.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/kms/render/platform/power.rs`, `crates/yserver/src/kms/render/platform/connectors.rs`, `crates/yserver/src/kms/render/backend/kms/session.rs`, `crates/yserver/src/kms/render/backend/kms/scanout.rs`.
- Modify: `crates/yserver/src/kms/render/backend/kms/platform_hooks.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver-render/src/kms/render/scene/cursor.rs`, `crates/yserver/src/kms/render/backend/tests/atomic_cursor.rs`, `crates/yserver/src/kms/render/backend/tests/tearing.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round2.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round3.rs`, and the existing cursor-plane/platform test fixtures affected by the binding change.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_a.rs`.

**Interfaces:**
- Consumes: A3 properties, A4 pool and leases, existing sink/event source; evidence P-scene/P-cursor/P-life.
- Produces: `CursorBinding`, `CursorCrtcState`, `CursorPlaneUpdate`, `PreparedCursorCommit`, prepare/accept/reject/retire methods and lifecycle helpers from the shared decisions. Existing `cursor_plane_show_on_crtc`/hide adapters now return success only for an **accepted atomic** operation, and still describe retained visibility correctly on rejection/defer. All visibility queries (including `any_cursor_plane_visible`, direct timing and scene sync) now read actual accepted `CursorCrtcState`, never a legacy visible set or desired detach. C1 hooks stay as temporary adapters here; B5/B6/B8 replace them with receipts.

- [ ] **Step 1: Add `atomic_cursor_only_atomic_binding_has_full_geometry_and_retirement` and `atomic_cursor_only_atomic_hide_preserves_ownership_on_reject`.** Establish a cursor through Backend compose/events, observe the temporary retirement adapter's atomic show, then hide via Backend. Assert attach has FB/CRTC IDs, zero SRC_X/Y, SRC_W/H = whole buffer << 16, CRTC_W/H = whole buffer, signed output-local X/Y = pointer − hotspot, and HOTSPOT when available. Detach has FB_ID=CRTC_ID=0. Assert NONBLOCK|PAGE_FLIP_EVENT, no legacy operations, accepted state at return, old/new ownership until the event, and no second CRTC commit before it. Reject attach/detach and allocation deferral separately: retain actual binding; never expose SW beside HW. Mutations: omit a geometry property, release the old lease at accept, mark a rejected commit in flight, or declare a rejected detach hidden; fail the corresponding assertion.
- [ ] **Step 1a: Add `atomic_cursor_only_c1_adapters_publish_only_accepted_bindings`.** Backend-establish held Vsync direct while effectively hidden, then define/show a visible cursor, move, animate and hide with controlled accept/reject/busy results. Keep scene mode consistent with actual bound planes after accepted adapter work; rejected/deferred work cannot claim Hidden/Hw or `cursor_bound_all` prematurely. While On, sprite producers never attach; a pending/failed detach keeps Async disallowed until the detach's event drains, with old leases retained. Run existing C1 tests listed in sibling plan B's B5/B6 at this checkpoint. Adapt their legacy show/hide fixture to A1's recorder in this same task; never satisfy their expectations by mutating accepted state. Existing synchronous-timing assertions may be superseded by equivalent Backend scheduling/event assertions, with an explicit old→new test mapping before deletion.
- [ ] **Step 2: Run red on full atomic show/hide through the Backend path.** Include immediate-after-accept event delivery to expose duplicate/misattributed retirement.
- [ ] **Step 3: Wire full atomic binding and ownership together.** Show/hide retirement callbacks are temporary callers of the new commit service, not legacy ioctls. Register obligations only after success and retire by device/CRTC. At event dispatch, snapshot/consume the pre-existing obligation before invoking scene retirement adapters, so a newly accepted show from that callback cannot be retired by the old event. Implement a conservative safe error baseline: rejected show retains old binding; rejected hide retains HW and requests a cursorless scene retry. Retain C1's scene-sync guard against unresolved scene acknowledgements and request unflip on actual failure; busy/deferred adapter work remains scheduled until acceptance, with no speculative sync. The external tearing handoff remains a temporary retirement adapter until B6, now issuing an owned Vsync atomic detach and awaiting its own event before any async primary or SW reveal. Preserve `scene_cursor_hide_failed`/once-only diagnostic behavior and cursorless Vsync fallback if detach fails. Detailed final retries arrive in C. Integrate the blocking cursor detach primitive into every current disable caller **in this task**, so newly bound FBs are never freed on DPMS/VT/removal while still live. On successful blocking disable, release old on-screen/submitted users only after old events drain; failure preserves users and today's fail-stop/quarantine path. Disable requests use the recording atomic sink, not a separate accepting test bypass.
- [ ] **Step 4: Run green, each mutation and F stress.** Confirm production no longer calls the legacy bind/move/hide executor on an eligible path. Legacy symbols and the old scene timing remain for B/C cleanup.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "feat: submit owned full cursor state atomically"`. Do not batch this commit with the next task.

## Task A6: Diff accepted state and serialize every commit producer

**Files:**
- Modify: `crates/yserver/src/kms/render/platform/cursor_commit.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`, `crates/yserver/src/kms/render/platform/scanout_bos.rs`, `crates/yserver/src/drm/page_flip.rs`, `crates/yserver/src/drm/modeset.rs`, `crates/yserver/src/kms/render/backend/kms/scanout.rs`, `crates/yserver/src/kms/render/backend/kms/present.rs`, `crates/yserver/src/kms/render/platform/flip_events.rs`, `crates/yserver/src/drm/async_flip.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_a.rs`.

**Interfaces:**
- Consumes: A5 prepared commits and applied state; evidence P-cursor/P-scene/P-direct.
- Produces: `submit_atomic_cursor_intent_for_output(&mut self, output_idx: usize) -> io::Result<CursorMoveOutcome>` replacing the move-only submitter. Scene/direct/unflip builders consume `&[CursorPlaneUpdate]` in place of position-only slices; all accept/reject paths settle the same prepared leases. Produce `has_eligible_atomic_cursor_intent(&self) -> bool` in place of the visible/move-only readiness query; `scene_wants_compose`, `backend_scanout_next_wakeup` and both idle/held-direct scheduler branches consume it, so a first attach or stationary image/hide progresses without primary damage. Route the event-path `cursor_plane_drain_pending_move_for_output` through the same full-intent service, preserving per-device event arbitration. Preserve `FlipTiming`, `AsyncOutcome`, `AsyncFlipLedger`, `TearingCommitContext` and real producer flags/fences. Async filtering occurs before preparation: no discarded cursor delta can be accepted merely because its primary was accepted.

- [ ] **Step 1: Add `atomic_cursor_only_equal_state_consumes_without_ioctl`, `atomic_cursor_only_busy_stream_keeps_latest_intent`, and `atomic_cursor_only_all_builders_use_accepted_deltas`.** Via Backend, accept B, enqueue C→B behind an actual outstanding flip, and deliver its event; also drive fractional host pointer motion whose integer coordinates equal B. Assert no ioctl, no event obligation or next-wakeup spin. Exercise shared, copied, direct and composed-unflip paths through their actual Backend entries; assert motion-only X/Y deltas, unchanged fields omitted, correct sink device, one outstanding commit per CRTC, latest generation retained if a newer intent arrives before acceptance. Script EBUSY: no fallback/attribution probes, no new event owed, no retry until a real existing flip event. Mutations: submit empty deltas, compare to last requested instead of accepted, clear newer intent on acceptance, omit a producer's cursor updates, or retry EBUSY each tick; each fails its trace/state assertion.
- [ ] **Step 1a: Add `atomic_cursor_only_c1_async_filters_before_cursor_preparation`.** Backend-enter On through real Async Present probation for both hidden-direct and shared composed routes; queue motion/image/hotspot while a flip is pending. Assert every Async attempt has PAGE_FLIP_ASYNC, no OUT_FENCE_PTR and zero properties on any cursor plane (not just no X/Y), no cursor-only attach while On, and no cursor state/lease settlement from a filtered-out delta. Inject async EINVAL→Vsync fallback, EBUSY, shape suspension/recovery and exit. Vsync fallback while On keeps detached intent; failed/pending detach keeps primaries Vsync/cursorless until safe, with no async event obligation for a rejected attempt. Mutations: append FB_ID to Async, accept a filtered delta, reattach on Vsync fallback, or classify async EINVAL as cursor failure; fail request/lease/attribution assertions. Preserve `tearing.rs::{async_direct_request_has_flag_no_out_fence_no_cursor,async_scene_request_has_flag_no_out_fence_no_cursor,composed_async_accept_keeps_cursor_move_unsent}`; C10 only audits this completed migration.
- [ ] **Step 2: Run red and identify the failing producer, not just a builder unit test.** A direct probe uses `Backend::try_present_direct`, and completion uses Backend events.
- [ ] **Step 3: Replace the motion caches with full-state deltas and shared settlement.** Prepare positions when the request is built. Preserve #244 arbitration: direct successor then scene then cursor-only; work already in flight waits for its event. TEST_ONLY requests cannot publish applied state or acquire event obligations. Cursor-only has no already-visible-plane guard for an otherwise owned full intent; scene ownership is tightened in B.
- [ ] **Step 4: Run green, producer-specific mutations and F stress.** Assert repeated idle scheduling stays quiescent after equal-state cancellation.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "feat: diff and serialize atomic cursor intents"`. Do not batch this commit with the next task.

## Task A7: Prove shared framebuffer retirement, pressure, and hotspot identity

**Files:**
- Modify: `crates/yserver/src/kms/cursor_plane/sprites.rs`, `crates/yserver/src/kms/render/platform/cursor_state.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs`.
- Create: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_a_lifetime.rs`.

**Interfaces:**
- Consumes: A4–A6 pool roles and Backend animation/definition/event paths; evidence P-pointer/P-cursor/P-scene.
- Produces: per-device newest-wanted coalescing and pool wakeup when the final user disappears; hotspot is part of prepared sprite identity even with identical pixels/version.

- [ ] **Step 1: Add `atomic_cursor_only_shared_fb_survives_staggered_replacement`, `atomic_cursor_only_pool_pressure_keeps_only_newest_animation`, and `atomic_cursor_only_hotspot_only_switches_framebuffer`.** Establish F0 on outputs A/B, accept replacing commits, retire A before B and skip B's next scene by the A1 allocator dependency's scripted no-slot result. Send animation while all slots are used. Independently assert F0's bytes unchanged and no RMFB after acceptance or A's first event; only B's final replacing event permits reuse. With N CRTCs, assert initial two buffers and maximum max(2,N+1), no live rewrite, dropped intermediate wanted images and progress for the newest without fresh pointer motion. Same pixels + changed hotspot must bind a distinct FB, update X/Y and hotspot, and retain the old FB through retirement. Include identical raw handles on separate devices. Mutations: release at acceptance; release shared FB at first event; overwrite busy slot; grow beyond bound; retain oldest wanted; dedup hotspot only by version/pixels. Each must fail the independent trace/byte assertion.
- [ ] **Step 2: Run red for each mutation-sensitive scenario.** Accepted replacement before its event is a separately asserted checkpoint; eventual cleanup alone is insufficient.
- [ ] **Step 3: Complete pool pressure and user aggregation.** User count is the union of CRTC roles, not a global current FB or the first retiring output. Retry newest wanted on resource release and schedule it even without motion; pressure is not EINVAL and cannot disqualify HW. Test built-but-unsent references in B3, where a real copied frame holds them across a scheduling boundary.
- [ ] **Step 4: Run green, both explicit §4.4 mutations plus the other listed mutations, then F stress.** Finish with Backend hides, all events and successful disable; require zero leases and allocation balance across both cards.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "fix: preserve cursor framebuffers through all users"`. Do not batch this commit with the next task.

## Task A8: Close the foundation checkpoint across modesets and lifecycle

**Files:**
- Modify: `crates/yserver/src/kms/render/platform/cursor_lifecycle.rs`, `crates/yserver/src/kms/render/platform/cursor.rs`, `crates/yserver/src/kms/render/backend/kms/session.rs`, `crates/yserver/src/kms/render/backend/kms/crtc_config.rs`, `crates/yserver/src/kms/render/platform/connectors.rs`, `crates/yserver/src/kms/render/backend/seam.rs`, `crates/yserver/src/kms/render/backend/kms/platform_hooks.rs`, `crates/yserver/src/kms/render/backend/tests/tearing/round2.rs`, `docs/status.md`.
- Create: `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_a_lifecycle.rs`.

**Interfaces:**
- Consumes: A5 blocking detach/drain and A6 invalid/equal state; evidence P-life/P-backend.
- Produces: `invalidate_cursor_applied(&mut self, key: CrtcKey)` invalidates only comparison state, preserving any real ownership; all modeset/master-loss/resume callers use it. Desired topology and actual owners remain distinct.

- [ ] **Step 1: Add `atomic_cursor_only_lifecycle_invalidates_and_drains` and `atomic_cursor_only_failed_disable_retains_users`.** Through Backend DPMS off/on, VT release/acquire, reprobe removal and actual CRTC configuration, assert cursor detach in the same blocking disable as primary/CRTC off; no replacing event expected; old cursor events drain before RMFB. Assert the first post-resume/modeset binding sends every applicable property even if position/sprite match pre-suspend values. Fail disable and assert unchanged ownership/resources before a successful recovery teardown. Include zero-output activation and no legacy rearm. Add the C1 resume regression: hide/define transparent while VT-suspended, clear direct ownership through its real teardown, then resume and DPMS-wake. In a separate failed lifecycle-detach case assert retained ownership and the existing failure path first, then recover via a successful disable/drain before resume; never force a resume after failed all-off. `recover_hidden_cursor_on_resume` must reconcile any stale actual binding without depending on `scanout_m2.active()`, queue no attach for an effectively hidden cursor, and retain failed-detach resources until safe. Supersede `tearing/round2.rs::hidden_cursor_while_vt_suspended_retries_failed_plane_hide_on_resume`'s best-effort legacy-hide failure with these Backend-only sequences: successful same-request disable already detaches; failed disable retains owners/follows the error path; hidden resume sends full Detached state even with no direct frame. Require that invalidated hidden resume issues both zero IDs. Mutations: bypass resume reconciliation when direct is inactive (and omit the full hidden state), or clear users after failed disable. Also mutate pre-drain cache/reference clearing, omit invalidation at any modeset entry, or rearm by legacy operation; each fails its checkpoint. Do not seed a stale visible plane after successful atomic all-off to preserve an unreachable old fixture stimulus.
- [ ] **Step 2: Run red using actual Backend lifecycle dispatch and the atomic recorder.** Do not install an output-power executor that returns success without assembling the real request.
- [ ] **Step 3: Complete invalidation coverage and checkpoint documentation.** Migrate `Platform::recover_hidden_cursor_on_resume` in `backend/seam.rs` / `kms/platform_hooks.rs` to the common lifecycle/invalidation path, separately from the direct-only hide adapter; active master and enabled outputs remain mandatory. Record that atomic properties/pool lifetime are active, the conservative retirement-time scene adapter remains, frame coupling/#244 proof are B, and final recovery/reassignment/legacy deletion are C. Do not claim full spec completion at A.
- [ ] **Step 4: Run green, lifecycle mutations and F stress; review the entire A checkpoint.** It must display/animate/move/hide and suspend safely without depending on B/C.
- [ ] **Step 5: Per-task gate and one commit.** After the targeted red/green, mutation checks (reverted) and F stress, run all commands:

```bash
cargo +nightly fmt
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
```

Expected: every command exits 0, clippy has no warnings, and ordinary/ignored suites have no failures; report capability skips explicitly. Check formatter changes and run the relevant targeted tests again only if formatting changed behavior-sensitive code. Stage only this task's named files and commit once with `git commit -m "fix: reset atomic cursor state at lifecycle boundaries"`. Do not batch this commit with the next task.

## Spec coverage table

This is the complete cross-plan ledger, repeated so this plan can be reviewed independently. Task IDs prefixed **A** are owned here; other prefixes refer to the ordered sibling plans. A/B checkpoints are working increments; final ownership/error/deletion requirements are complete only after their listed later tasks. Each §1–§4 bullet/sub-bullet and every numbered §4 test has an explicit owner; §5 is included for the final controller task.

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
