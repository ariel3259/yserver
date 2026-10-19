# Cursor Sprite State P2b Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL at implementation time: use superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. The implementer is a small model: where this plan gives an interface, an invariant, a test table or a mutation, follow it exactly; where it names a reference file and line range, read that range first and port it, stripping what is listed. Do not write more than the plan asks for.

**Goal:** Carry the cursor image, hotspot, attach and detach on the existing per-CRTC atomic stream as full-state cursor-plane properties backed by P2a's sprite pool, retire them by P1's commit IDs, make the lifecycle safe on every disable path, and enable the path only for devices that qualify, leaving master's legacy path complete as the fallback.

**Architecture:** The scene keeps its `ShowOnRetire`/`HideOnRetire` sequencing. For devices on `CursorStatePath::Atomic`, the `upload`/`show`/`hide` sink adapters stage a sprite and submit one cursor-only commit each; scene, direct, copied and unflip commits keep carrying only position deltas, computed against the applied binding. Leases are reserved before submit, become users on acceptance, and are released at the replacing commit's event (matched by ID) or at a successful blocking atomic detach (lifecycle and rollback), the only other release point. The path is decided once per plane from stored caps (P2a), so production devices stay Legacy until Task 5 supplies caps from `Device::open`; before that the new code is exercised only by tests whose snapshot planes carry caps.

**Tech Stack:** Rust workspace; P1's `AtomicCommitSink::submit_with_commit_id`, `AtomicEventFixture`, `RecordingSceneFixture`; P2a's `SpritePool`, `CursorPlane` properties, `CursorStatePath`, capability negotiation; lavapipe for the ignored scene tests.

**Spec:** [docs/superpowers/specs/2026-10-19-cursor-sprite-state-design.md](../specs/2026-10-19-cursor-sprite-state-design.md). Read it before executing any task. Prerequisite: [P2a](2026-10-19-cursor-sprite-state-a.md) complete.

## Global Constraints

- Everything on the legacy path is unchanged when the device does not qualify; master's `set_cursor2`/`move_cursor` executors, hide-recovery and transient-fallback state and the software-cursor policy keep their tests. Every existing master cursor test passes without semantic edits (only the mechanical signature edits listed in the tasks).
- Devices with no universal cursor plane (Apple DCP / Asahi) keep the optimistic legacy probe and the permanent software-cursor latch.
- Preserve Linux glibc, Linux musl and FreeBSD ioctl request typing. No new `YSERVER_*` behavior environment variables; no `cfg(test)` branches inside production function bodies.
- Respect `tools/check-layout.sh` (5,000 code / 6,000 test lines; `yserver-render` free of drm/gbm/input/udev/`yserver-core`; `backend/portable` free of KMS names). Do not edit the script.
- The path never changes during a plane's lifetime and the assignment is immutable (spec R3, R24); every predicate meaning "hardware cursor bound" goes through `KmsCursorState::hw_bound_on` (spec R18).
- Do not change scheduling, admission, retry timing, async qualification or the EBUSY/EINVAL attribution policy. Async (tearing) commits carry no cursor property of any kind.
- Image, hotspot, attach and detach never ride scene, direct, copied or unflip commits in P2 (spec R20). Do not pull any P3-P5 behavior or maintainer finding forward (spec Non-goals).
- The scene-crate seam already landed in P2a Task 6; P2b changes no `yserver-render` file.

This document is planning only. Its preparation authorizes writes only to `docs/superpowers/specs/` and `docs/superpowers/plans/` of `/home/ariel_santangelo/Projects/yserver-cursor-sprites`, with no git writes, cargo runs, device access, hardware or subagents. Tasks below describe future implementation; their source writes and commits require a later implementation instruction. Do not merge as part of this plan.

Branch `feat/cursor-sprite-state`, P2a complete (six tasks). Read-only porting source: the reference `/home/ariel_santangelo/Projects/yserver-cursor` at `9023da14`; its first atomic-cursor commit `14fc3e47` (`git show 14fc3e47 -- <path>`) is the closest state to P2 (legacy removed there, kept here) and is the best porting source for the scene seam and platform adapters. Reference line numbers refer to HEAD `9023da14` unless a commit is named. Never copy whole reference files.

## Review Focus

1. Every "hardware cursor bound" predicate must use the one accessor, or an idle desktop on the Atomic path never moves its cursor: Task 1 `hw_bound_on` conversion and Task 3 `p2_idle_pointer_motion_wakes_the_scheduler_and_sends_one_cursor_only_move`, `p2_motion_inside_one_output_does_not_set_membership_dirty`.
2. A pointer that returns to the applied position must leave no stale pending move and no stale latest position: Task 3 `p2_returning_to_the_applied_position_clears_pending_and_latest`.
3. A replacing commit's event must release its predecessor exactly once, and a rejection or lifecycle detach must remove the cursor from the plane the accepted state names (never legacy `crtc->cursor`) and release only after the blocking commit: Tasks 2-4 (`p2_image_replacement_keeps_the_old_sprite_until_the_replacing_event`, `p2_rollback_detaches_the_applied_plane_not_crtc_cursor`, `p2_vt_suspend_detaches_even_with_a_replacement_in_flight`).
4. Every path that ends a CRTC's scanout (VT, DPMS, connector removal, shutdown) must have a release owner, or leases leak and the pool starves: Task 4 `p2_connector_disable_reenable_three_times_leaks_nothing`, `p2_shutdown_disable_output_releases_every_cursor_resource`.
5. A device that does not qualify must behave exactly as master, and the path must not change under live atomic state: Task 5 `p2_legacy_path_device_uses_masters_ioctls_and_allocates_no_sprites`; P2a `topology_refresh_never_changes_the_path_or_the_assignment`.

## Verification rules for all tasks

- **A (end state):** every lifecycle test ends, through normal completion (accepted events delivered by ID, `Backend::set_cursor_hidden`, `disable_output`), with no sink pending-flip entry, no in-flight cursor record, no pending position, `plane.sprites.all_users_zero()`, and `CursorBufferLedger::end_state() == (0, 0, 0)` after the plane is dropped. Assert intermediate retention before the release event. Destroying a fixture, clearing a map or forcing a drain is not evidence.
- **B (production drivers):** tests install topology (a snapshot plane through `for_tests_install_cursor_plane_from_snapshot`), recording buffer I/O, recording legacy show/hide executors, P1's recording atomic executor and P1's explicit-cookie injector (`AtomicEventFixture::queue_commit_completion`, `accepted_event_id`), then advance only through `Backend` trait entries (`define_cursor`, `warp_pointer_root`, `clear_area`, `maybe_composite`, `try_present_direct`, `on_page_flip_ready`, `on_scanout_render_completion`, `set_cursor_hidden`, `set_dpms_power`, `begin_vt_release`/`finish_vt_release`/`on_vt_acquire`, `before_block`, `next_wakeup`) and the inherent `KmsBackend::disable_output` that `lib.rs:931` calls at shutdown or `core_loop::run_core`. Never call `cursor_plane_show_on_crtc`, `cursor_plane_hide_on_crtc`, `submit_atomic_cursor_move_for_output`, `retire_atomic_cursor_commit_for_output`, `detach_cursor_blocking`, lease methods, or `record_commit` from a lifecycle test. `prepare_cursor_commit`, the attribution builder and the retirement equality check on a constructed in-flight record are stateless boundary tests, say so, and are the only exception (spec Test requirements B).
- **C (hardware):** Task 6 is the separate final controller; it needs the user's authorization at that time (the 2026-10-19 unattended authorization covers P1 only). Recording tests never claim to execute the real ioctls.
- **F (repetition):** every new test runs 5 times under exactly 16 busy loops; any test that waits on GPU work (the `RecordingSceneFixture` scenes, ignored) runs 20 times. Start one Bash session, save the 16 busy-loop PIDs in an array, `trap` EXIT/INT/TERM to kill and reap only those PIDs, use `--list` first so a filter cannot select zero tests, run `cargo test --workspace --locked --features xdmcp <full-test-name> -- --exact --nocapture` (add `--ignored` for GPU tests) with the lavapipe environment sourced, and kill the loops before reporting. Each mutation named below is applied once, must fail the named assertion (not a build error or timeout), and is reverted before the next case and before the gate. Report executed and skipped cases separately; a skipped copied/SYNC_FD case is not lavapipe coverage.
- **Skips:** every fixture skip prints exactly one line naming its reason (for example `SKIP p2_...: no Vulkan test device`), from the deciding constructor only. Unexpected setup errors fail.
- One commit per task after its gate; every commit buildable; interface changes update their consumers and tests in the same task. Remove each `#[allow(dead_code, reason = "...")]` that P2a added once its consumer lands.

Per-task gate. Every test command is prefixed by `source /home/ariel_santangelo/Projects/claude-scratch/atomic-cursor-split/sdd-p1/lvp-env.sh &&` (without it tests would use the real NVIDIA/AMD drivers); never run tests without it:

```sh
cargo +nightly fmt --all
source /home/ariel_santangelo/Projects/claude-scratch/atomic-cursor-split/sdd-p1/lvp-env.sh && cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings
source /home/ariel_santangelo/Projects/claude-scratch/atomic-cursor-split/sdd-p1/lvp-env.sh && cargo test --workspace --locked --features xdmcp
source /home/ariel_santangelo/Projects/claude-scratch/atomic-cursor-split/sdd-p1/lvp-env.sh && cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
tools/check-layout.sh
```

## Production caller audit (performed before choosing the tests)

Read-only `rg -n -g '*.rs' -g '!**/tests/**' -g '!**/for_tests.rs'` on the P2a tree. A reference-only caller is not a reason to port a path.

| Production path relied on | Non-test caller verified | Driver used by planned tests |
| --- | --- | --- |
| `show`/`hide`/`upload` adapters | `scene/cursor.rs:544` `apply_cursor_transition_on_retire` (called from `scene/flip.rs:218` and `scene/cursor.rs:112` `retire_external_cursor_handoff`; `ShowOnRetire` built at `scene/tick.rs:761`); direct hooks `backend/kms/scanout.rs:379-470` and `backend/kms/platform_hooks.rs:149-183`; resume `platform/cursor.rs:1546` `rearm_cursor` from `session.rs:698,995` | Scene retirement via `Backend::maybe_composite` + `on_page_flip_ready`; direct hooks via `Backend::try_present_direct`; resume via the VT/DPMS Backend entries |
| Cursor-only lane and its gates | `backend/kms/scanout.rs:2522,2631` call `submit_atomic_cursor_move_for_output` (gate `platform/cursor.rs:504`); `has_eligible_atomic_cursor_move` (`:395`, gate `:410`) from `scanout.rs:2122`; `cursor_crtc_membership_dirty` (`:1281`) from the pointer path | `Backend::warp_pointer_root`, `next_wakeup`, `before_block`, `maybe_composite` |
| Event retirement | `backend/kms/scanout.rs:2201,2229,2290` `record_atomic_cursor_commit_retirement` (P1 supplies `matched_event_id`; explicit unmatched cookies are dropped at `:2198,2226`) | `Backend::on_page_flip_ready` with injected cookies |
| Position carriers | `platform/scene_sink.rs:114,213`, `platform/scanout_bos.rs:81,181`, `backend/kms/scanout.rs:570-665,1090-1109,1517-1531` call `cursor_plane_atomic_positions_for_outputs` and `cursor_plane_scene_commit_submitted` | `maybe_composite`, `try_present_direct`, `on_scanout_render_completion` |
| Lifecycle hides and releases | `backend/kms/session.rs:486` `cursor_plane_hide_all` (VT suspend), `:511,1037` and `crtc_config.rs:111` `clear_atomic_cursor_state_after_old_events_discard`; `platform/power.rs:109,137,268` and `platform/connectors.rs:204-240` `disable_connector` (then `remove_connector_at` -> `refresh_cursor_topology_for_devices`, no discard runs); `session.rs:371` `disable_output` (`scene.drain_all` -> `hide_all`, then `platform.disable_output`, no discard) | VT/DPMS/connector/shutdown Backend entries with `for_tests_accept_output_power_commits` |
| Device capability wiring | `kms/outputs.rs:308`, `platform/drm.rs:181` (candidate probe, also negotiates) call `Device::open` | Recording `DeviceControl` through the P2a open-sequence function; hardware in Task 6 |

## File and interface map

`Y` means `crates/yserver/src/`.

| Responsibility | Files |
| --- | --- |
| State types, commit assembly, request tagging, `hw_bound_on` | `Y/kms/render/platform/{cursor_state,cursor_commit}.rs`, `Y/drm/atomic.rs`, `Y/kms/render/platform.rs`, `Y/kms/render/platform/cursor.rs` |
| Atomic adapters, leases, retirement by ID, rollback | `Y/kms/render/platform/cursor_atomic.rs` (new), `Y/kms/render/platform/cursor.rs`, `Y/kms/cursor_plane.rs` |
| Motion, producers, recovery outcomes | `Y/kms/render/platform/{cursor,cursor_atomic}.rs` |
| Lifecycle | `Y/kms/render/platform/{cursor_atomic,cursor,power,connectors}.rs`, `Y/kms/render/backend/kms/{session,crtc_config}.rs` |
| Enabling | `Y/drm/device.rs`, `Y/drm/client_caps.rs` |
| Fixtures and scenarios | `Y/kms/render/backend/tests/cursor_sprites/{mod,support,scene_attach,motion,lifecycle,legacy}.rs` |
| Documentation | `docs/status.md`, `docs/superpowers/plans/2026-10-19-cursor-sprite-state-validation.md` |

### Task 1: State types, commit assembly, request tagging and the `hw_bound_on` accessor (no behavior change)

**Files:**

- Create: `crates/yserver/src/kms/render/platform/cursor_state.rs`, `crates/yserver/src/kms/render/platform/cursor_commit.rs`.
- Modify: `crates/yserver/src/kms/render/platform.rs` (declare both modules; `KmsCursorState` gains `crtc_states: HashMap<crtc::Handle, CursorCrtcState>` default empty; the value type of `atomic_cursor_commits_in_flight` becomes `InFlightCursorCommit`).
- Modify: `crates/yserver/src/drm/atomic.rs` (`AtomicCommitRequest::add_cursor_property`).
- Modify: `crates/yserver/src/kms/render/platform/cursor.rs` (accessor; the eight `is_visible_on` sites; the sites that insert into or read `atomic_cursor_commits_in_flight`; the retire signature).
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs` (`record_atomic_cursor_commit_retirement` takes the matched ID: call sites `:2201`, `:2229`, definition `:2290`).
- Test: `#[cfg(test)] mod tests` inside `cursor_commit.rs` and `cursor_state.rs`; the attribution test in `crates/yserver/src/drm/atomic/tests.rs` (P1 created that file); existing master tests updated mechanically.

**Interfaces:**

- Produces in `cursor_state.rs` (port reference `platform/cursor_state.rs` lines 1-60 and 128-177; **strip** `owner`, `scene_owned_intent`, `topology_epoch`, `sprite_key`, `CursorQualification*`, `CursorFallbackState`, `retry_backoff_cursor_suppressed`, `discarded_software_scene_frame`, `intent*`, `lifecycle_pending`, `CursorPlaneUpdate`; P2 has no queued intent, no frame owner and no lifecycle-pending owner):
  - `CursorBinding { properties: CursorPlaneProperties, lease: SpriteBufferLease, framebuffer: u32, x: i32, y: i32, hot_x: u16, hot_y: u16 }` (`Copy`, `Eq`, `Debug`).
  - `CursorPropertyUpdate { object: RawResourceHandle, property: property::Handle, value: u64 }` with `atomic_update(self) -> AtomicPropertyUpdate`.
  - `PreparedCursorCommit { prior: Option<CursorBinding>, next: Option<CursorBinding>, updates: Vec<CursorPropertyUpdate> }` (`Clone`, `Eq`).
  - `InFlightCursorCommit { commit_id: Option<u64>, prepared: Option<PreparedCursorCommit> }`: `None`/`None` for master's position-only commits (retired by CRTC membership, unchanged), `Some(id)`/`Some(prepared)` for Atomic-path commits (retired only by `id`, spec R11). `prepared.prior` is the lease still owed a release; release code sets it to `None` once released (exactly once).
  - `CursorCrtcState { applied: Option<CursorBinding>, valid: bool, resume_visible: bool }` (`Default`; `valid` starts `false`: an untouched CRTC's kernel state is not trusted).
- Produces in `platform/cursor.rs`: `KmsCursorState::hw_bound_on(&self, crtc: crtc::Handle) -> bool`: `crtc_states.get(&crtc).is_some_and(|s| s.applied.is_some())` when `state_path() == Atomic`, else the plane's `is_visible_on(crtc)`. **Replace every `plane.is_visible_on` in `platform/cursor.rs` with it** (master lines 383 position carrier, 410 `has_eligible_atomic_cursor_move`, 504 the `submit_atomic_cursor_move_for_output` gate, 1143 motion/hide-recovery, 1281 `cursor_crtc_membership_dirty`, 1328 `cursor_plane_visible_for_output`, 1512 `cursor_plane_hide_all`, 1558 `rearm_cursor`); where a borrow of `device.cursor.plane` conflicts, bind `let bound = device.cursor.hw_bound_on(crtc)` first. After this task the only remaining `is_visible_on` call in the file is inside `hw_bound_on`'s Legacy branch (show `rg -n "is_visible_on" crates/yserver/src/kms/render/platform/cursor.rs` in the report). Behavior is unchanged because no device is Atomic yet.
- Produces in `cursor_commit.rs` (port reference `platform/cursor_commit.rs` lines 56-157; **strip** receipt functions, `prepare_detach_commit`, `add_cursor_plane_updates`, `add_vsync_scene_cursor_updates`, `request_for_cursor_updates`):
  - `prepare_cursor_commit(properties: CursorPlaneProperties, crtc: crtc::Handle, applied: Option<CursorBinding>, valid: bool, next: Option<CursorBinding>, width: u32, height: u32) -> PreparedCursorCommit`: spec R16 exactly: `attach = !valid || applied.is_none()`; with `next`: `FB_ID` when attach or lease/framebuffer differs, `CRTC_ID` when attach, `CRTC_X`/`CRTC_Y` when `!valid` or the coordinate differs, on attach the geometry six (`SRC_X = 0`, `SRC_Y = 0`, `SRC_W = width << 16`, `SRC_H = height << 16`, `CRTC_W = width`, `CRTC_H = height`), `HOTSPOT_X`/`HOTSPOT_Y` when the plane exposes them and (`!valid` or the value differs); without `next`: `FB_ID = 0` and `CRTC_ID = 0` only when `!valid || applied.is_some()`; coordinates are `i64::from(v) as u64`. `width`/`height` are the pool capacity, never the image size.
  - `request_for_cursor_commit(prepared: &PreparedCursorCommit) -> AtomicCommitRequest`: one `add_cursor_property` per update in order.
- Produces in `atomic.rs`: `AtomicCommitRequest::add_cursor_property(&mut self, object: RawResourceHandle, property: property::Handle, value: u64)`: duplicate-property replacement as `add_raw_property`, writes the raw request, pushes `(object, property)` onto `cursor_properties` so `attribution_snapshot().request(false)` strips it. `add_cursor_position` uses it (behavior identical).
- Changes (mechanical): `atomic_cursor_commits_in_flight: HashMap<crtc::Handle, InFlightCursorCommit>`; master's cursor-only insertions store `{ commit_id: None, prepared: None }`; `retire_atomic_cursor_commit_for_output(&mut self, output_idx: usize, event_id: Option<u64>) -> bool`: removes the record unless it has `Some(id)` with `event_id != Some(id)` (then it is kept); `record_atomic_cursor_commit_retirement(&mut self, output_idx: usize, matched_event_id: Option<u64>)` forwards it. No release is performed in this task.

- [ ] **Step 1: Write the tests first.**

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `prepare_cursor_commit_truth_table` (stateless boundary) | Property map as a set of `(property, value)`: attach with hotspot (ten + two, `SRC_W = cap_w << 16`, `CRTC_H = cap_h`); attach without hotspot properties; replace new lease only (`{FB_ID}`); replace + moved X + changed hotspot; move only X (`{CRTC_X}`); identical binding (empty); detach with applied (`{FB_ID: 0, CRTC_ID: 0}`); detach with `applied == None && valid` (empty); detach with `applied == None && !valid` (the pair); `valid == false` with applied equal to next (full attach set resent); negative coordinates as `i64 as u64`. | `SRC_W` shifted by image size; treat `valid == false` as valid; detach a detached valid state; zero-extend negatives. |
| `cursor_request_tags_every_full_state_property_for_attribution` (boundary for P3, since no P2 scene commit carries full-state fields) | Build a `SceneWithCursor` request of scene properties plus `request_for_cursor_commit(attach)`; recorder script `[EINVAL, EINVAL, Ok]`: `take_last_cursor_rejection()` is true, the stripped `TEST_ONLY` record contains none of the attach properties, `stripped.len() + attach_count == live.len()`. | Add through `add_raw_property`; strip only `CRTC_X`/`CRTC_Y`. |
| `retire_keeps_an_id_tagged_record_on_a_different_event_id` (boundary exception: constructs the record because no Atomic-path producer exists yet; Task 2 re-proves it through the Backend) | `{commit_id: Some(7)}`: `retire_atomic_cursor_commit_for_output(0, Some(8))` and `(0, None)` return false and keep it; `Some(7)` removes it; `{commit_id: None}` is removed by any `event_id`. | Retire an ID-tagged record on any event; retire `None` against `Some(7)`. |
| `hw_bound_on_reads_the_path_specific_state` (boundary) | A Legacy snapshot plane: `hw_bound_on` follows `for_tests_set_visible`; an Atomic snapshot plane: it follows `crtc_states[crtc].applied` and ignores the legacy map. | Atomic branch reads the map (or the reverse). |
| master's existing cursor suites | `cargo test -p yserver --locked --features xdmcp kms::render::backend::tests::atomic_cursor`, `kms::render::platform::tests` and `kms::cursor_plane` unchanged except mechanical argument updates (list every edited test). | n/a |

- [ ] **Step 2: Run red, implement, run green.**
- [ ] **Step 3: Mutation and repetition protocol** (5x under 16 busy loops; none waits on GPU).
- [ ] **Step 4: Run the per-task gate; commit only after every command passes.** Subject: `feat(kms): add atomic cursor state types, full-state commit assembly and hw_bound_on`.

### Task 2: Atomic-path adapters: stage, attach, replace, detach, rollback and retirement by commit ID

**Files:**

- Create: `crates/yserver/src/kms/render/platform/cursor_atomic.rs` (`impl PlatformBackend` for the Atomic path; declared in `platform.rs`).
- Modify: `crates/yserver/src/kms/render/platform/cursor.rs` (branch the public adapters on `state_path()`; retire releases leases), `crates/yserver/src/kms/cursor_plane.rs` (pool-facing wrappers).
- Create/Test: `crates/yserver/src/kms/render/backend/tests/cursor_sprites/{mod,support,scene_attach}.rs`.

**Interfaces:**

- Platform adapters. `cursor_plane_upload_image_for_output(output_idx, version, hot_x, hot_y, width, height, bgra_bytes) -> io::Result<CursorUploadOutcome>`: the Atomic branch calls `plane.sprites_mut().prepare(device, buffer_io, version, hot_x, hot_y, width, height, bytes)` and maps `SpriteStage::{Ready, Deferred}`; an `Err` is classified by `apply_cursor_operation_result` only for real driver/allocation failures, never for `Deferred`; the Legacy branch is P2a's. `cursor_plane_uploaded_image_matches_for_output`: Atomic branch `candidate_lease().is_some_and(|l| l.matches(version, hot_x, hot_y))`.
- `cursor_atomic.rs` (port reference `platform/cursor.rs:2766-2828` show, `2966-2995` hide, `1537-1597` reserve/reject, `1606-1712` accept, `1713-1750` retire; **strip** `CursorCommitOwner`, `scene_owned_intent`, `intent`, qualification, `CursorFallbackState`, reassignment barriers, primary-successor handling, the intent update on upload, `note_detach_accepted`), `impl PlatformBackend`:
  - `pub(super) fn atomic_show(&mut self, output_idx: usize, hot_x: u16, hot_y: u16, x: i32, y: i32) -> Result<(), CursorShowError>` and `pub(super) fn atomic_hide(&mut self, output_idx: usize) -> io::Result<()>`; the public `cursor_plane_show_on_crtc`/`cursor_plane_hide_on_crtc` branch on `state_path() == Atomic` as their first line and otherwise run master's code.
  - **What `show` binds** (the scene's upload ran just before in the same transition; `show` carries no version): the device's *current candidate lease* (`sprites().candidate_lease()`), required only to have the requested hotspot (`matches(candidate.version(), hot_x, hot_y)`). No candidate, or a hotspot mismatch -> `Deferred { WouldBlock }`. Callers: scene retirement (upload immediately before), `refresh_direct_cursor_on_all_outputs` (uploads, then shows), `bind_direct_cursor_on_all_outputs` and `rearm_cursor` (reuse the candidate of the earlier upload).
  - Route and local coordinates as master (`cursor_output_route`, `cursor_root_to_crtc_local(x, y, layout_x, layout_y, hot_x, hot_y)`); `available_on(crtc)` false -> `Unbound`. An in-flight record or a wait mark -> `Deferred` (insert the wait mark), `remains_visible = applied.is_some()`. `prepared = prepare_cursor_commit(properties, crtc, applied, valid, Some(binding), plane.width(), plane.height())`; empty `updates` -> `Ok(())`, no ioctl, no record.
  - **Reservation iff the lease changes:** reserve only when `prior.lease != next.lease`; submit through `sink.submit_with_commit_id(&device, PAGE_FLIP_EVENT | NONBLOCK, AtomicCommitKind::CursorOnly, request_for_cursor_commit(&prepared), &[AtomicCommitEvent { crtc, motion_timestamp: None }])`. On success, `can_accept_reserved` validation and `accept_reserved` run **if and only if a reservation was taken** (a same-lease rebind has nothing to validate). If validation fails after the ioctl succeeded (cannot happen single-threaded): log an error, `valid = false`, still insert the in-flight record so the event is consumed, transfer nothing, return `Unbound`.
  - `EBUSY`: reject the reservation, insert the wait mark, clear `take_last_cursor_rejection()`, return `Deferred { remains_visible: applied.is_some(), .. }`; no state change, no lease consumed (R12).
  - Any other error: reject the reservation, clear the attribution flag, call `atomic_rollback(device_idx, crtc, &error)` (below), return `Unbound`/`StillVisible` from its outcome.
  - Success with ID `k`: `applied = Some(binding)` (`None` for detach), `valid = true`, `resume_visible = false` when attached, insert `InFlightCursorCommit { commit_id: Some(k), prepared: Some(prepared) }`, remove the wait mark, `note_cursor_success`; for a show set the latest position cache to the new `(cx, cy)` and **remove any pending move** for the CRTC (latest == applied, spec R16; do not call `remember_atomic_position`, which always queues one); for a detach `clear_atomic_position(crtc)`.
  - `hide`: `applied == None && valid` -> `Ok(())`; a permanently disabled device with nothing bound -> `Ok(())` (master's rule); in-flight -> `Err(WouldBlock)`; otherwise the detach commit as above with `next = None`; failures classified by `apply_cursor_operation_result`.
  - `pub(super) fn detach_cursor_blocking(&mut self, device_idx: usize, crtc: crtc::Handle) -> io::Result<()>` (shared by rollback here, motion recovery in Task 3 and lifecycle in Task 4): requires `applied.is_some()` or `valid == false`; builds `prepare_cursor_commit(applied.properties, crtc, applied, valid, None, ..)` on **the applied plane** (`applied.properties`; never `crtc->cursor`, never the legacy hide executor); submits with `AtomicCommitSink::submit(&device, AtomicCommitFlags::empty(), AtomicCommitKind::CursorOnly, request, &[])` (no `NONBLOCK`, no `PAGE_FLIP_EVENT`, no events: no completion association, P1 R1) **even when an ID-tagged commit is in flight** (the kernel serializes it); clears the attribution flag. Success (the blocking commit returned after the hardware stopped scanning): release the applied lease user, release the in-flight record's `prior` lease (if different) and set that record's `prepared.prior = None`, set `applied = None`, `valid = true`, `resume_visible = true` when a binding was detached, `clear_atomic_position(crtc)`. Failure: keep `applied` and every lease, `valid = false`, return the error. Each lease is released exactly once whichever path runs first.
  - `atomic_rollback(device_idx, crtc, error)` implements master's *policy* with the Atomic executor (spec R14): clear the cached position; if `applied.is_some()` run `detach_cursor_blocking`; `Ok` (or nothing applied) -> `cursor_hide_recovery.remove`, `note_einval`, `fallback_changed`, master's log text; `Err` -> insert master's `CursorHideRecovery`, `retry_required`, keep ownership.
  - Retirement (extends Task 1's function): a removed record with `commit_id: Some(k)` and `prepared.prior = Some(p)` where `next` is `None` or `p.lease != next.lease` -> `release_user(p.lease)`; wait mark cleared as master.
- Fixture additions (`support.rs`): `AtomicPathFixture` wrapping P1's `RecordingSceneFixture` (Vulkan; `RecordingSceneFixture::new(1)` prints its own skip line) with `new(kind: PathKind) -> Result<Option<Self>, String>`, `PathKind::{Physical, Paravirtualized}`; installs a complete-plane snapshot on every device through `for_tests_install_cursor_plane_from_snapshot` (physical: `amdgpu`, cap rejected; paravirtualized: `virtio_gpu`, cap accepted, hotspot properties) with one shared `RecordingCursorBufferIo::new(256)` ledger, installs `RecordingLegacyCursor { show_calls, move_calls, hide_calls }` as the plane's legacy show and hide executors, asserts `state_path() == Atomic` for each device. Helpers: `cursor_properties_for_output(backend, output_idx) -> CursorPlaneProperties`; `applied_cursor_binding(backend, output_idx) -> Option<CursorBinding>`; `sprite_users(backend, output_idx, lease) -> Option<usize>`; `establish_hardware_cursor(fx, delivered, cursor, pointer) -> HardwareCursor { output_idx, device_fd, crtc, attach_commit_id }`: define the cursor on the root, warp, `Backend::clear_area` on the root, `maybe_composite`, deliver accepted events one at a time with `retire_pending_output` and compose again, asserting the exact recorded kind sequence after each step, until a `CursorOnly` record carrying `FB_ID` was accepted and retired (**first run the same scene on a Legacy-path device to learn the event sequence, then assert the exact observed Atomic-path sequence in the helper's own test**; at most 4 steps, then panic `hardware cursor not established`). Finish scenes through `finish_fixture` then P1's `RecordingSceneFixture::finish_after_normal_drain` (it handles the root's sticky default cursor).

- [ ] **Step 1: Write the tests first** (GPU-waiting scenarios are `#[ignore]`d, 20 repetitions; one skip line when there is no Vulkan device):

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `p2_first_hardware_cursor_attaches_with_full_state_and_retires_by_id` | `AtomicPathFixture::new(Physical)`, `establish_hardware_cursor` with a 4x4 ARGB cursor hotspot (1, 2) at pointer (100, 90). Attach record: kind `CursorOnly`, flags exactly `NONBLOCK | PAGE_FLIP_EVENT`, nonzero `user_data`, properties exactly `{FB_ID, CRTC_ID = crtc, CRTC_X/Y = 99 / 88, SRC_X = 0, SRC_Y = 0, SRC_W = cap_w << 16, SRC_H = cap_h << 16, CRTC_W = cap_w, CRTC_H = cap_h}`, no hotspot entries (physical); `RecordingLegacyCursor` zero calls. Unretired: `any_cursor_commit_in_flight()`; the correct ID retires it. `applied_cursor_binding` is `Some` at (99, 88); `sprite_users == Some(1)`. | Use `set_cursor2` for the show; drop the `SRC_W` shift. |
| `p2_paravirtualized_attach_carries_hotspot_properties` | `PathKind::Paravirtualized`: the attach also has `HOTSPOT_X = 1`, `HOTSPOT_Y = 2`; a second cursor with identical pixels and hotspot (3, 4) (a different record) switches `FB_ID` and the hotspot values. | Omit hotspot properties when exposed. |
| `p2_image_replacement_keeps_the_old_sprite_until_the_replacing_event` | After `establish_hardware_cursor` (cursor A), define cursor B and compose: one `CursorOnly` replace record with properties exactly `{FB_ID = B}`. Before the event: `sprite_users(A) == Some(1)` and `(B) == Some(1)`, A's bytes in the ledger unchanged, no `RemoveFb` for A. The replacing ID: `sprite_users(A) == Some(0)`. Re-inject the same ID (duplicate): dropped by P1, counts unchanged. Hide + detach event + `finish_fixture`: all zero. | Release A at acceptance; release A on any event; underflow on the duplicate. |
| `p2_wrong_explicit_id_is_dropped_before_cursor_retirement` | With a replacement in flight, inject a cookie that matches no pending entry for the CRTC: still in flight, A keeps its user, B's count unchanged. Through `Backend` this guarantee is P1's filter (`scanout.rs:2198,2226`); the local equality is covered by Task 1's boundary test. | Remove P1's unmatched-cookie `continue` **and** the local equality (either alone survives, by design). |
| `p2_zero_cookie_event_retires_the_pending_replacement_exactly_once` | A zero cookie for the CRTC: P1's FIFO attributes the oldest pending entry (the replacement); A is released once. A second zero cookie with nothing pending retires nothing and underflows nothing. | Release without checking that a record is in flight. |
| `p2_accepted_show_leaves_no_pending_move` | After `establish_hardware_cursor` and a retired attach, with no motion: `for_tests_atomic_cursor_pending_position` is `None` and `finish_fixture` sees no further `CursorOnly` record. | Keep master's `remember_atomic_position` insert after a show. |
| `p2_hide_detaches_and_the_event_releases_the_sprite` | `Backend::set_cursor_hidden(true)` + compose: `HideOnRetire` accepts a `CursorOnly {FB_ID = 0, CRTC_ID = 0}`; `cursor_plane_visible_for_output` is false immediately; the user remains until the event; after it `all_users_zero()`. | Mark hidden without a commit; release at acceptance. |
| `p2_ebusy_show_defers_without_consuming_a_lease` | Script `EBUSY` for the attach: no in-flight record, `prepared_user_count() == 0`, no user, scene mode unchanged (no software reveal, no `Hidden`); after the next injected event the forced repaint retries and the attach is accepted with a fresh ID. | Count the lease as a user on EBUSY; set Hw on a deferred show. |
| `p2_rejected_attach_rolls_back_without_touching_legacy` | Script `EINVAL` for the first attach: reservation rejected (`prepared_user_count() == 0`), no in-flight, no blocking detach record (nothing applied), `RecordingLegacyCursor` zero calls, `for_tests_cursor_fallback_remaining` shows the transient backoff. | Call the legacy hide executor; leave the reservation held. |
| `p2_rejected_replace_detaches_the_applied_plane_and_releases_after_it` | With A applied, script `EINVAL` for the replace: an event-less, non-`NONBLOCK` `{FB_ID: 0, CRTC_ID: 0}` record on **A's plane** appears; `applied_cursor_binding` is `None`; `sprite_users(A) == Some(0)` only after that record succeeded; legacy executors zero calls. With the detach scripted to fail: A and its user retained, `for_tests_cursor_hide_recovery_attempts == Some(0)`. | Use the legacy hide; release before the blocking commit succeeds; clear `applied` on failure. |
| `p2_pool_pressure_defers_replacement_and_keeps_only_the_newest_wanted_image` | One CRTC (max 2): attach A; define B and compose (A and B have users, pool full); define C then D before B's event: both `Deferred`, mode stays Hw, A/B bytes unchanged; after B's event the retry stages D (never C) into A's slot and the next retirement replaces B with D; recorded `FB_ID` sequence A, B, D. | Keep the oldest wanted; overwrite A while it has a user. |
| `p2_direct_hook_treats_deferred_as_failure_and_unflips` | Held direct scanout, visible-cursor refresh with the pool full: `refresh_direct_cursor_on_all_outputs` reports failure and `request_direct_unflip` is taken (master's failure path); no lease leaks. | Treat `Deferred` as success. |

- [ ] **Step 2: Run red, implement in this order:** upload/matches, `atomic_show`/`atomic_hide`, `detach_cursor_blocking` and `atomic_rollback`, retirement release, fixtures. Remove the P2a `allow(dead_code)` attributes now consumed.
- [ ] **Step 3: Audit.** Re-run the Rule-B grep; confirm `rg -n "set_cursor2|\.hide\(" crates/yserver/src/kms/render/platform/cursor_atomic.rs` is empty.
- [ ] **Step 4: Mutation and repetition protocol** (GPU scenarios 20x under 16 loops).
- [ ] **Step 5: Run the per-task gate; commit only after every command passes.** Subject: `feat(kms): attach, replace and detach the hardware cursor with atomic commits`.

### Task 3: Motion, producers, visibility gates and recovery outcomes on the Atomic path

**Files:**

- Modify: `crates/yserver/src/kms/render/platform/cursor.rs` (`try_cursor_plane_move_for_device`, `cursor_plane_atomic_positions_for_outputs`, `cursor_plane_scene_commit_submitted`, `submit_atomic_cursor_move_for_output`, `reject_atomic_cursor_commit`), `cursor_atomic.rs`.
- Create/Test: `crates/yserver/src/kms/render/backend/tests/cursor_sprites/motion.rs`.

**Interfaces:**

- **Motion invariant (spec R16; Atomic-path device only, the Legacy branch is untouched):** in `try_cursor_plane_move_for_device` a CRTC with `hw_bound_on` false is skipped; otherwise the local position uses the **applied binding's hotspot**. After the call the latest cache equals the current pointer position, and a pending move exists if and only if latest != `applied.x/y`: a motion equal to applied **removes** any existing pending move (and updates latest) without creating a request; otherwise it behaves as master (`remember_atomic_position`).
- Gates: because Task 1 routed every predicate through `hw_bound_on`, `has_eligible_atomic_cursor_move`, the `submit_atomic_cursor_move_for_output` gate and `cursor_crtc_membership_dirty` work on `applied`; no other change is needed in them. `cursor_plane_atomic_positions_for_outputs` keeps its other gates (`atomic_cursor`, `permanently_disabled`, `cursor_hide_recovery`).
- Applied-state upkeep: after an accepted position-carrying commit - `cursor_plane_scene_commit_submitted(positions)` and the success branch of `submit_atomic_cursor_move_for_output` - set `applied.x/y` to the accepted position for positions matching the latest cache whose plane equals `applied.properties.plane`. On an Atomic-path device `submit_atomic_cursor_move_for_output` uses `submit_with_commit_id` and stores `InFlightCursorCommit { commit_id: Some(k), prepared: None }`; the Legacy branch keeps `submit` and `commit_id: None`.
- Rejection and recovery (spec R14, Atomic-path device): `reject_atomic_cursor_commit` per rejected CRTC clears the cached position then rolls back with `atomic_rollback` (Task 2: blocking atomic detach of the applied plane; **no** `plane.hide`), preserving master's `note_einval`/`fallback_changed`/`CursorHideRecovery` policy and logs. The motion-led hide recovery branch (`cursor.rs:1149-1190`) calls `detach_cursor_blocking` instead of `plane.hide`: on success it clears the recovery entry and positions and enters transient fallback as master, with `applied = None` and the lease released by the detach; on failure it counts attempts, and when the 3-attempt latch fires it sets `permanently_disabled` and keeps `applied` and the lease (still bound; released by any later successful detach, spec R14). `cursor_plane_hide_on_crtc` for a permanently disabled device that still has `hw_bound_on` true runs the detach (master's "still owns a plane its failed hides left bound").

- [ ] **Step 1: Write the tests first** (GPU scenarios `#[ignore]`d, 20x; `AtomicPathFixture` from Task 2):

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `p2_idle_pointer_motion_wakes_the_scheduler_and_sends_one_cursor_only_move` | Established cursor, no damage, all events retired. `warp_pointer_root` to a new point: `Backend::next_wakeup()` is due; `before_block`/`maybe_composite` emit exactly one `CursorOnly` record with properties `{CRTC_X, CRTC_Y}` only. | Restore `plane.is_visible_on` in `has_eligible_atomic_cursor_move` (no wakeup) or in the submit gate (no record). |
| `p2_motion_inside_one_output_does_not_set_membership_dirty` | Cursor attached on a one-output scene; motion within the output: `cursor_crtc_membership_dirty(..)` is false and the scene does not recompose (`for_tests_scene_wants_scene()` false). | Restore `is_visible_on` in `cursor_crtc_membership_dirty` (always dirty). |
| `p2_returning_to_the_applied_position_clears_pending_and_latest` | Warp P1 (accepted, applied = P1); while in flight warp P2; then back to P1: after the event no further `CursorOnly` record carries P2; the pending move is `None`; the next scene commit carries `CRTC_X/Y` = P1; idle passes add no record; `next_wakeup` not due. | Keep the pending P2; leave latest at P2. |
| `p2_scene_direct_copied_and_unflip_commits_carry_only_the_applied_position` | After `establish_hardware_cursor`: shared scene (`maybe_composite`), direct (`try_present_direct`), composed unflip (authoritative-root Present then ineligible Present), copied (`RecordingSceneFixture::with_route(1, CopiedSameDevice)` + the same snapshot install, `on_scanout_render_completion`). Each accepted record's cursor-plane properties are a subset of `{CRTC_X, CRTC_Y}` valued pointer minus the applied hotspot; sprite users unchanged by the commit. | Append `FB_ID` to a carrier; use the staged image's hotspot. |
| `p2_staged_replacement_hotspot_does_not_shift_the_applied_plane` | Applied A hotspot (0, 0); define B hotspot (8, 8), stage it, move the pointer before the replacement lands: position = pointer - (0, 0); after the replacement lands the next move uses (8, 8). | Use the effective cursor's hotspot. |
| `p2_accepted_scene_position_updates_the_applied_state` | A scene commit carries X; a following replace omits `CRTC_X`/`CRTC_Y` when equal to that carried position and includes a position the scene did not carry. | Do not update applied on scene acceptance. |
| `p2_busy_stream_keeps_latest_intent_and_waits_for_the_event` | Cursor-only move in flight, three more warps: one record before the event; after it exactly one commit with the latest position; scripted EBUSY on it sets `cursor_retry_waits_for_flip(0)`, sends nothing more, one retry after the next event. | Retry EBUSY every tick; send intermediate positions. |
| `p2_rollback_detaches_the_applied_plane_not_crtc_cursor` | Cursor-caused rejection of a `SceneWithCursor` commit (script `[EINVAL, EINVAL, Ok]`): an event-less blocking `{FB_ID: 0, CRTC_ID: 0}` record on the applied plane; `RecordingLegacyCursor.hide_calls == 0`; `applied_cursor_binding` is `None`; `sprite_users == 0` only after that record succeeded. With the detach scripted to fail: binding and user retained, `for_tests_cursor_hide_recovery_attempts == Some(0)`. | Use the legacy hide; release before success. |
| `p2_motion_led_hide_recovery_settles_the_applied_state` | After the failed rollback, motion: a successful recovery detach leaves `!cursor_plane_visible_for_output(0)`, `sprite_users == 0`, transient fallback set. Three failing detaches latch `permanently_disabled` and keep `applied` and the user; a later `set_cursor_hidden` + successful detach releases it. | Leave `applied` set after a successful recovery; release at the latch. |
| `p2_legacy_motion_is_unchanged_on_a_legacy_path_device` (headless) | Master's `legacy_path_pointer_motion_keeps_one_legacy_move_and_no_atomic_commit` (`backend/tests/atomic_cursor.rs:646`) passes unmodified; with a snapshot plane deciding `Legacy` the same motion yields the same legacy-move count. | Route a Legacy device through the Atomic branch. |

- [ ] **Step 2: Run red, implement:** motion invariant first, then upkeep, then rejection/recovery.
- [ ] **Step 3: Mutation and repetition protocol.**
- [ ] **Step 4: Run the per-task gate; commit only after every command passes.** Subject: `feat(kms): drive cursor motion, rejection and recovery from the applied atomic state`.

### Task 4: Lifecycle safety: blocking detach on every disable path and a release owner for every lease

**Files:**

- Modify: `crates/yserver/src/kms/render/platform/cursor.rs` (`cursor_plane_hide_all`, `clear_atomic_cursor_state_after_old_events_discard`, `rearm_cursor`, `refresh_cursor_topology_for_devices_with`), `crates/yserver/src/kms/render/backend/kms/platform_hooks.rs` (`recover_hidden_cursor_on_resume`, which reaches the Atomic path through `cursor_plane_hide_on_crtc`), `cursor_atomic.rs`.
- Modify: `crates/yserver/src/kms/render/platform/power.rs` (`dpms_set_outputs_active(false)`, `disable_output`), `crates/yserver/src/kms/render/platform/connectors.rs` (`disable_connector`, `remove_connector_at`), `crates/yserver/src/kms/render/backend/kms/session.rs` and `crtc_config.rs` (invalidation points only).
- Create/Test: `crates/yserver/src/kms/render/backend/tests/cursor_sprites/lifecycle.rs`.

**Interfaces:**

- Blocking detach callers (Task 2's `detach_cursor_blocking`, which detaches **even with a commit in flight**, spec R22): `cursor_plane_hide_all` (Atomic branch: for every output with `hw_bound_on`; log per-CRTC failures, never return `Err` for an individual failure, `Err` only when no device has a plane, as master), `dpms_set_outputs_active(false)` and `PlatformBackend::disable_output` (both: first collect the output indices and detach each, then run the existing disable loops, which borrow `self.outputs`), and `disable_connector` (before `modeset::disable_output` of that output). Legacy branches are untouched.
- Release owners (spec R22/R23): every lease user has exactly one of: its replacing event (R11); a successful `detach_cursor_blocking` (releases the applied lease and the in-flight record's prior, setting `prior = None`); `clear_atomic_cursor_state_after_old_events_discard` (for each removed in-flight record whose `prior` is still `Some` and differs from `next`: release it); CRTC removal. `remove_connector_at` (`connectors.rs`, the single choke point after the caller's successful disable) removes the CRTC's `crtc_states` entry and `atomic_cursor_commits_in_flight` record on its device, releasing any lease they still own. At shutdown `scene.drain_all` -> `hide_all` and `platform.disable_output` both detach, so no user remains.
- Pool size: `refresh_cursor_topology_for_devices_with` keeps P2a's `set_active_crtc_count` update; it does **not** clear Atomic-path in-flight records (those carrying an ID) nor leases of live CRTCs; master's position maps and wait marks are cleared as before.
- Invalidation (spec R23): `invalidate_cursor_applied(key)` sets `valid = false`; called for every live CRTC at VT/DRM-master loss (`run_suspend`), `run_resume`, DPMS-on and after a full modeset (where master clears its maps); never releases a lease.
- Resume: `rearm_cursor` routes on `resume_visible || hw_bound_on` and calls `atomic_show` with the current hotspot/position (full state because `valid == false` or `applied == None`; the pool candidate is reused, no new allocation); `recover_hidden_cursor_on_resume` for an Atomic CRTC calls `atomic_hide` (full detach pair because `valid == false`).

- [ ] **Step 1: Write the tests first** (GPU scenarios `#[ignore]`d, 20x; `for_tests_accept_output_power_commits` for the all-off commits):

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `p2_vt_suspend_detaches_even_with_a_replacement_in_flight` | Established cursor A with a replacement B accepted and unretired; `Backend::begin_vt_release`/`finish_vt_release` (the entries `drive_vt_event` uses; pattern in `backend/tests/vt_session.rs`): a blocking event-less `{FB_ID: 0, CRTC_ID: 0}` record on B's plane precedes the all-off commit; afterwards `applied_cursor_binding` is `None`, **both** A and B have zero users (each released exactly once), and the discard later releases nothing again (`all_users_zero()`, no underflow). | Skip the detach when a commit is in flight; release A twice; release before the blocking commit succeeds. |
| `p2_failed_blocking_detach_keeps_ownership_and_invalidates_the_comparison` | Script the detach to fail: the suspend error path runs as master's; `applied_cursor_binding` still `Some`, users unchanged, nothing released at the discard; resume sends the full state; a later successful detach releases normally. | Clear `applied` on failure; release on discard after a failed detach. |
| `p2_resume_sends_full_state_after_invalidation` | Suspend + resume (`on_vt_acquire`): the re-attach record has the complete attach set with a fresh ID; the ledger shows no new allocation; the attach is accepted (no validation failure) although the lease equals the previous one. | Treat `valid` as true after resume; validate acceptance when no reservation was taken. |
| `p2_dpms_off_on_detaches_then_reattaches` | `set_dpms_power` off then on: detach before the disable, full-state attach after the enable; users zero after the detach and one after the re-attach. | Skip the detach before DPMS off. |
| `p2_connector_disable_reenable_three_times_leaks_nothing` | Three cycles of `disable_connector` (through the Backend connector entry master's connector tests use) and re-enable, re-establishing the cursor each time: after each disable `all_users_zero()` and `has_available_buffer()`; no `crtc_states` entry or in-flight record for the removed CRTC; staging after the cycles returns `Ready`, not `Deferred`. | Skip the release in `remove_connector_at` (the pool starves by the third cycle). |
| `p2_shutdown_disable_output_releases_every_cursor_resource` | After an established cursor, `KmsBackend::disable_output()` (the inherent entry `lib.rs:931` calls): blocking detach + disable, `finish_fixture`, `all_users_zero()`, `end_state() == (0, 0, 0)` after the plane drops; also with the cursor hidden first, and with a failed `hide_all` detach followed by the platform's own detach. | Skip the detach in `PlatformBackend::disable_output`. |
| `p2_discard_releases_the_prior_of_a_discarded_in_flight_commit` | A replacement in flight whose event is discarded by an all-off (detach scripted to fail, all-off succeeds): `clear_atomic_cursor_state_after_old_events_discard` releases A once; B stays owned (applied). | Drop the record without releasing; release twice. |
| `p2_double_detach_releases_each_lease_once` | DPMS off (detach) then shutdown without an intervening discard: `all_users_zero()` and no underflow. | Keep a second owner slot that a later detach overwrites. |

- [ ] **Step 2: Run red, implement:** detach callers, `remove_connector_at`, discard release, invalidation, resume.
- [ ] **Step 3: Audit.** Grep every blocking-disable site (`modeset::disable_output`, `output_power_executor.disable`) for a missed detach; document why `kms/outputs.rs:244` (startup rollback before any cursor exists) needs none. Confirm no test calls `detach_cursor_blocking`.
- [ ] **Step 4: Mutation and repetition protocol.**
- [ ] **Step 5: Run the per-task gate; commit only after every command passes.** Subject: `feat(kms): detach and release the atomic cursor on every disable path`.

### Task 5: Enable the path: the cursor step of the KMS open sequence and the legacy proofs

**Files:**

- Modify: `crates/yserver/src/drm/device.rs` (`configure_kms_device` gains the cursor step), `crates/yserver/src/drm/client_caps.rs` (test control extensions only if needed).
- Create/Test: `crates/yserver/src/kms/render/backend/tests/cursor_sprites/legacy.rs`; update P2a's open-sequence test.

**Interfaces:**

- `configure_kms_device(&mut self, control: &dyn DeviceControl)`: after the Atomic request and before P1's CRTC-event check, `self.cursor_caps = Some(negotiate_cursor_caps(self, control))` (infallible, spec R1). `Device::open` already calls it; the probe fd (`platform/drm.rs:181`) negotiates too, which is harmless. This is the only change that lets production planes see caps, hence the only one that lets `state_path()` be `Atomic` in production; the `render cursor: ... cursor_state_path=...` log line is the operator-visible indicator. Remove P2a's remaining `allow(dead_code)` on the cursor-step items.

- [ ] **Step 1: Write the tests first:**

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `open_sequence_requests_universal_atomic_hotspot_name_then_checks_crtc_events_once` (updates P2a's test; boundary on the recording control) | `Device::for_tests_with_control`: calls exactly `[Set(UniversalPlanes), Set(Atomic), Set(CursorPlaneHotspot), DriverName, CrtcInVBlankEvent]`, each once; `cursor_caps()` is `Some` with the cached facts. | Negotiate twice; put the cursor step before the Atomic request or after the CRTC check; keep a second `enable_atomic_capabilities`. |
| `p2_legacy_path_device_uses_masters_ioctls_and_allocates_no_sprites` (headless, then GPU) | Snapshots: a plane missing `FB_ID`; a paravirtualized plane with the cap rejected; a plane with `caps == None`; define a cursor and warp through `Backend`: the legacy show executor sees master's call counts, position-only atomic records are exactly `{CRTC_X, CRTC_Y}`, the buffer ledger has no record, `sprites()` is empty. Master's whole cursor suite passes unmodified. | Allocate the pool for a Legacy device; issue `AddFB2`. |
| `p2_apple_dcp_topology_keeps_the_software_latch` (headless) | Zero-plane snapshot: `ENXIO` through `note_unbound_cursor_failure` latches `hw_cursor_disabled_for_device`; no Atomic code runs. | Route a no-plane device to the Atomic path. |
| `p2_paravirtualized_with_accepted_cap_but_incomplete_plane_is_legacy` (boundary) | `virtio_gpu`, cap accepted, `HOTSPOT_Y` missing -> `Legacy/HotspotSupportMissing` (the R25 exception: such a device now sees its cursor planes); recorded in the validation note. | Accept the plane. |

- [ ] **Step 2: Run red, wire the cursor step last, run green.** Re-run the full ignored suite.
- [ ] **Step 3: Mutation and repetition protocol.**
- [ ] **Step 4: Run the per-task gate; commit only after every command passes.** Subject: `feat(kms): request the cursor hotspot capability at KMS open and enable the atomic cursor path`.

### Task 6: Documentation and the hardware controller

**Files:**

- Modify: `docs/status.md` (new behavior-focused P2 entry in the recent-work list).
- Create: `docs/superpowers/plans/2026-10-19-cursor-sprite-state-validation.md` during implementation/controller execution, containing the software/repetition/mutation/skip and hardware result matrix. Do not fabricate it during planning.
- Read only: this design and both plans, `.github/workflows/ci.yml`, `docs/setup.md`, `tools/cursor-bench`, the implementation commits and their test output.

**Interfaces:**

- Consumes: the twelve buildable implementation commits, per-test mutation and repetition results, executed/skipped lavapipe cases, glibc/musl/FreeBSD build evidence.
- Produces: coordinator-run AMD (`card0`) and NVIDIA (`card1`) results with kernel/driver/GPU/output topology, build revision, the `cursor_state_path` log line per device, scenario, commit-ID/event relation, visible behavior, quiescence evidence; the final `docs/status.md` entry; intended single commit: `docs: describe the atomic cursor state path and its validation`.

- [ ] **Step 1: Prepare the controller packet after software tasks pass.** Include the build revision, startup instructions from `docs/setup.md`, new test names, software/mutation/repeat results, all fixture skip reasons, and a diff review against the P1 base confirming: scene/copied/direct/unflip builders unchanged, master's legacy code unchanged except `hw_bound_on`/state-path branches, no change to `drm/modeset.rs`, scheduling, async policy or maintainer-finding code. The implementer accesses no `/dev/dri`; the coordinator runs Step 2.
- [ ] **Step 2: The coordinator runs this matrix on AMD and NVIDIA.** It requires the user's authorization at that time (the 2026-10-19 P1 authorization does not extend to P2); check before each run that no other client holds the card (DRM master, Vulkan/GL users); start yserver from `tools/cursor-bench` (master binary versus branch binary) on a free display; drive clients by script (mpv, xdotool, `tools/anim-cursor-probe.c`). No new behavior environment variable or policy change to make a case run.

| Coordinator-run case (both vendors) | What to execute and observe | Required result and defect it detects |
| --- | --- | --- |
| Path selection and startup | Start the branch; read the `cursor_state_path` and capability log lines | `atomic` or a documented `legacy` reason; no startup regression |
| Static and shape-changing cursor | Move over windows with several cursors (arrow, I-beam, themed ARGB); change shape while moving | Correct image, no flicker or torn frame, hotspot-correct clicks; matches master visually |
| Animated cursor (scene) | Animated cursor over a window for 60 s | No torn images, no dropped HW cursor |
| Scene frame rate under an animated cursor | A 60 fps FIFO client (windowed mpv) with an animated cursor over it, branch vs master | Scene commits/s and presented fps against master; a drop of at most one scene frame per animation frame is the documented cost (spec "Expected cost") and is recorded, not hidden |
| Animated cursor over a fullscreen direct client | Fullscreen mpv (direct scanout) with an animated cursor, branch vs master | Count of unflips/direct exits against master; repeated unflips are the known cost and are recorded |
| Sprite pressure | Fast animation across two outputs when dual-head exists | Newest frame shown, no stall |
| Tearing | The existing `tools/cursor-bench/tests/tearing.sh` windows | Async commits carry no cursor property; numbers as master |
| VT switch and DPMS | Switch VT away/back; DPMS off/on | Cursor reappears with the right image; no stale-plane EINVAL; no leaked framebuffers |
| Connector disable/re-enable x3 | Disable and re-enable one output three times (`xrandr --output ... --off/--auto`) with a visible cursor | Cursor returns each time; lease/ledger counters (debug log or `/sys/kernel/debug/dri/*/framebuffer`) show no growth; the hardware cursor image still updates after the third cycle |
| Output hot-unplug (when topology allows) | Unplug one output | Remaining output keeps its cursor; removed output's resources released |
| Legacy fallback (when a legacy-decided device exists) | A device that decides `legacy` | Master's behavior and call pattern |
| Paravirtualized driver (virtio-gpu, kernel >= 6.8) | Not available unless a VM is provided | Listed as pending validation in the PR if not run |

- [ ] **Step 3: Record the hardware results and close coverage gaps honestly.** Persist vendor results, failed/skipped/unexecuted cases and exact repetition counts in the validation note. Return any implementation defect to its owning task, rerun affected tests/gates, refresh the packet. A physical scenario unsupported by the topology stays an explicit limit.
- [ ] **Step 4: Write the final status entry as behavior with the actual verification scope.** State: on qualifying devices the cursor image, hotspot, attach and detach are full-state atomic commits on the existing per-CRTC stream, backed by a per-device pool of full-capacity ARGB sprite framebuffers released at the replacing commit's event (matched by ID) or at a successful blocking detach; position deltas still ride scene/direct/copied/unflip commits; devices that do not qualify keep the legacy ioctls and software fallback unchanged; lifecycle events detach with a separate blocking commit; the hotspot capability is requested at KMS open; the measured cost of image changes on the stream; CI runs the ignored suite on lavapipe but fixtures may report capability skips. Describe actual AMD/NVIDIA results and remaining limits (paravirtualized validation); omit split-history narrative.
- [ ] **Step 5: Run the per-task gate; commit this documentation task only after every command passes.** No merge in this task.

## Coverage table

| Spec rule / constraint | Task and decisive test |
| --- | --- |
| R1 open sequence, cursor step, exact names | P2a T4 tests; P2b T5 `open_sequence_requests_universal_atomic_hotspot_name_then_checks_crtc_events_once` |
| R2-R4 discovery, static path, no-plane | P2a T5; P2b T5 `p2_legacy_path_device_uses_masters_ioctls_and_allocates_no_sprites`, `p2_apple_dcp_topology_keeps_the_software_latch` |
| R5-R9 pool | P2a T2/T3; P2b T2 `p2_pool_pressure_defers_replacement_and_keeps_only_the_newest_wanted_image`, `p2_paravirtualized_attach_carries_hotspot_properties` |
| R10 release at the replacing event or blocking detach | P2b T2 `p2_image_replacement_keeps_the_old_sprite_until_the_replacing_event`; T4 detach tests |
| R11 retirement by commit ID | P2b T1 `retire_keeps_an_id_tagged_record_on_a_different_event_id`; T2 `p2_wrong_explicit_id_is_dropped_before_cursor_retirement`, `p2_zero_cookie_event_retires_the_pending_replacement_exactly_once`, `p2_first_hardware_cursor_attaches_with_full_state_and_retires_by_id` |
| R12 one commit in flight, EBUSY | P2b T2 `p2_ebusy_show_defers_without_consuming_a_lease`; T3 `p2_busy_stream_keeps_latest_intent_and_waits_for_the_event` |
| R13 reservation bracket (only when a lease changes) | P2a T3 pool half; P2b T2 `p2_rejected_attach_rolls_back_without_touching_legacy`; T4 `p2_resume_sends_full_state_after_invalidation` |
| R14 rejection: master's policy, atomic detach of the applied plane | P2b T2 `p2_rejected_replace_detaches_the_applied_plane_and_releases_after_it`; T3 `p2_rollback_detaches_the_applied_plane_not_crtc_cursor`, `p2_motion_led_hide_recovery_settles_the_applied_state` |
| R15 teardown never frees a used framebuffer | P2a T3 drop test; P2b T4 `p2_failed_blocking_detach_keeps_ownership_and_invalidates_the_comparison` |
| R16 diff, position invariant, no redundant move after show | P2b T1 `prepare_cursor_commit_truth_table`; T2 `p2_accepted_show_leaves_no_pending_move`; T3 `p2_returning_to_the_applied_position_clears_pending_and_latest` |
| R17 attribution tagging, async carries none | P2b T1 `cursor_request_tags_every_full_state_property_for_attribution` (boundary for P3); master's `async_*_request_has_flag_no_out_fence_no_cursor` unchanged |
| R18 one accessor for "bound" | P2b T1 `hw_bound_on_reads_the_path_specific_state`; T3 `p2_idle_pointer_motion_wakes_the_scheduler_and_sends_one_cursor_only_move`, `p2_motion_inside_one_output_does_not_set_membership_dirty` |
| R19 scene seam | P2a T6; P2b T2 `p2_ebusy_show_defers_without_consuming_a_lease`, `p2_direct_hook_treats_deferred_as_failure_and_unflips` |
| R20 carriers carry only the applied-hotspot position; upkeep | P2b T3 `p2_scene_direct_copied_and_unflip_commits_carry_only_the_applied_position`, `p2_staged_replacement_hotspot_does_not_shift_the_applied_plane`, `p2_accepted_scene_position_updates_the_applied_state` |
| R21 deferred work retries through the scene | P2b T2 deferred and pressure tests |
| R22 blocking detach on every disable path | P2b T4 `p2_vt_suspend_detaches_even_with_a_replacement_in_flight`, `p2_dpms_off_on_detaches_then_reattaches`, `p2_connector_disable_reenable_three_times_leaks_nothing`, `p2_shutdown_disable_output_releases_every_cursor_resource` |
| R23 invalidation and release owners | P2b T4 `p2_discard_releases_the_prior_of_a_discarded_in_flight_commit`, `p2_double_detach_releases_each_lease_once`, `p2_resume_sends_full_state_after_invalidation` |
| R24 assignment fixed | P2a T5 `topology_refresh_never_changes_the_path_or_the_assignment` |
| R25 legacy is master (with the stated exception) | P2b T3 `p2_legacy_motion_is_unchanged_on_a_legacy_path_device`; T5 legacy tests |
| Expected cost (spec) | P2b T6 matrix cases |
| A end-state | `finish_fixture` / `assert_no_cursor_resources` in every lifecycle scenario; ledger `end_state() == (0, 0, 0)` |
| B Backend/core-only drivers; master non-test callers | Caller audit; fixtures; explicit bans |
| C coordinator-run AMD + NVIDIA, separate final controller | P2b T6 matrix and note |
| F 5x/20x under 16 busy loops, one-line skips | Verification rules; each task's protocol |
| Shared helpers first and small; no large reference fixtures | P2a T1, P2b T2 `AtomicPathFixture` over P1's `RecordingSceneFixture` |
| No cfg(test) in production bodies; no env vars; layout ceilings | Every gate and source review |
| Final behavior-focused status | P2b T6 `docs/status.md` entry |

## Open questions and handoff

Rulings of revision round 1 are applied: path per device, decided once per plane and static; a paravirtualized driver without the capability stays Legacy; rollback uses a blocking atomic detach of the applied plane; every disable path has a release owner; R24 dropped; six plus six tasks.

New questions for the user (defaults shown):

1. **Direct-scanout cost.** An animated cursor over a fullscreen direct client can cause repeated unflips until P3 (a deferred show in held direct scanout is a hook failure, as master treats its errors). Default: accept and measure in Task 6. Alternative: make an in-flight deferral non-fatal in the direct hook and retry at the next flip, which adds scheduling.
2. **Release at the blocking detach.** The plan releases leases directly after a successful blocking detach (no wait for the old-event drain), on the grounds that a blocking atomic commit returns after hardware completion. Confirm, or ask for the stricter release after the existing discard (which needs a lifecycle owner on the paths with no discard: connector removal and shutdown).
3. **Hardware authorization** for Task 6 (AMD `card0`, NVIDIA `card1`); a virtio-gpu check needs a VM.

This plan has six tasks and one commit per task. It contains no implementation results.
