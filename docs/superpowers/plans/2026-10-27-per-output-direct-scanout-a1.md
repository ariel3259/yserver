# Per-output direct scanout — Sub-plan A1: owners and output-scoped mechanics

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The direct-scanout state can hold either today's one grouped `Root` owner or up to one `Output` owner per output, and the submit/retire/unflip/compose machinery acts only on the outputs an owner covers — with admission still `Root`-only, so behaviour is unchanged.

**Architecture:** `ScanoutM2State` stays the owner record. `KmsPstate::scanout_m2` becomes a `DirectOwners` container (one root slot + one slot per output). Every path that today acts on "all outputs" for a direct frame acts on the outputs of the owner's `DirectScope`. Sub-plan A2 opens `Output` admission (probe, CRTC selection, transitions, cursor, tearing, readback); A3 scopes the unflip causes and does the hardware validation.

**Tech Stack:** Rust, yserver KMS backend (`crates/yserver/src/kms/render/backend/kms/`).

**Spec:** `docs/superpowers/specs/2026-10-27-per-output-direct-scanout-design.md` (design items 1, 3, 5; Reuse).

## Global Constraints

- Reuse first: `ScanoutM2State`, `DirectPresentFrame`, the probe, the per-output compose path, the per-output commit gate and the plane-slice direct submission are reused; new code only for the owner container and the scope parameter. Every new type/function is justified in the report.
- No `YSERVER_*` environment variables that change behaviour; no `cfg(test)` in production bodies.
- Single-monitor behaviour unchanged; `Root` grouped submission and grouped composed unflip unchanged (AMD `ENOSPC`, `kms/scanout.rs` ~1767).
- One Present → exactly one completion; pins released only after every CRTC that scanned the frame out has been replaced.
- Tests: A — assert end state (owner slots, recorded commits per CRTC, completions, pins), not only calls; B — drive backend entry points (Present, scheduling pass `backend_scanout_maybe_composite`, page-flip/retirement handlers, next_wakeup); a test may seed an `Output` owner through the existing `for_tests` direct-frame helpers because admission arrives in A2, and must then drive everything else through entry points; C — hardware is A3; F — new tests 20 runs under CPU load (16 busy loops).
- Gate per task: `cargo +nightly fmt --all`; `cargo build --bin yserver`; `cargo test --workspace --locked --features xdmcp`; lavapipe `--ignored` workspace suite; `render_acceptance --ignored`; `cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings`; `tools/check-layout.sh`; FreeBSD clippy cross-check.
- Licence: wlroots (MIT) and Xorg (MIT) behaviour only as cited in the spec; no KWin/Mutter/Muffin source.
- Commits signed (`-S`) with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. Single-monitor direct scanout (mpv/CS2, VRR, tearing, Cinnamon COW cases of #299): every existing test passes unchanged in meaning after the migration (Task 1) and the scoping (Tasks 2–3).
2. `Root` on two same-refresh outputs: grouped direct, grouped unflip, one completion — `scanout_m1_accepts_exact_dual_head_root_tiling` and the dual-head tests pass unchanged.
3. A RANDR topology change while an owner exists resizes the per-output slots without leaking a frame, a pin or a queued notification — Task 1 test.
4. An `Output` owner's retirement/unflip never touches another output's CRTC and completes exactly once with its own clock sample — Task 2 tests.
5. Composition on output *j* proceeds while output *i* has an `Output` owner pending, held or unflipping — Task 3 tests.

---

### Task 1: `DirectOwners` container (pure migration)

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/kms/state.rs` (`ScanoutM2State` ~261, field `scanout_m2` ~812)
- Modify: every production and test site using `pstate.scanout_m2` (~185 production, ~300 test; `kms/scanout.rs`, `kms/present.rs`, `kms/session.rs`, `kms/platform_hooks.rs`, `kms/randr.rs`, `kms/crtc_config.rs`, `tests/**`)

**Interfaces:**
- Produces, in `kms/state.rs`: `pub(in crate::kms::render::backend) struct DirectOwners { root: ScanoutM2State, outputs: Vec<ScanoutM2State> }` replacing the field `scanout_m2` (new field name: `direct`), with `fn root(&self) -> &ScanoutM2State`, `fn root_mut(&mut self) -> &mut ScanoutM2State`, and `fn sync_output_count(&mut self, count: usize)`. Only these; the scope API arrives in Task 2 with its first consumers (no dead code at the clippy gate).
- `sync_output_count` grows `outputs` with fresh states; before shrinking it moves the removed slots' queued notifications (`completed`, `deferred_successor_skips`, `idled`) into the root slot's queues so none is lost (`active()` ignores those queues, `state.rs` ~337, and a stopped owner keeps them, `scanout.rs` ~1533). A removed slot must not hold a pending/current/queued frame (callers unflip first; today's topology paths already do for the root).

- [ ] **Step 1: Migrate.** Replace the field and every `pstate.scanout_m2` access with `pstate.direct.root()` / `root_mut()`. Call `sync_output_count(self.platform.outputs.len())` wherever `platform.outputs` is rebuilt (find them by the `scanout_m1` topology-signature / `clear("output topology changed")` sites, RANDR apply, rescan, VT acquire).
- [ ] **Step 2: Test** `direct_owners_follow_output_count_on_topology_change`: on the dual-head simulated fixture (`tests/randr.rs` `simulated_kms_dual_head`) drive a RANDR change from two outputs to one → `direct.outputs.len() == 1`; then with a root owner active, the change unflips first and leaves no pin and exactly one completion queued/drained. Mutation: never call `sync_output_count`.
- [ ] **Step 3: Gate**; all existing tests unchanged in meaning.
- [ ] **Step 4: Commit** `refactor(scanout): direct-scanout owners container (root slot + per-output slots)`.

### Task 2: Owner scope through submit, retirement, unflip, notifications and the scheduler's owner dispatch

**Files:**
- Modify: `kms/state.rs` (`DirectScope`, scope API on `DirectOwners`; the `DirectFrameSubmitter` fn-pointer type ~248)
- Modify: `kms/scanout.rs` — `submit_direct_frame_live` (~946), `submit_direct_frame_for_tests`, `release_direct_frame`, `queue_direct_successor`, `submit_queued_direct_successor`, `stop_direct_after_scanout_replaced`, `direct_scanout_frame_for_output` (~1637), `materialize_direct_shadow_for_unflip`, `submit_composed_unflip`, `fresh_composed_unflip_eligible`, `submit_fresh_composed_unflip`, `commit_fresh_composed_unflip`, `record_accepted_degraded_unflip_replacements`, `retire_direct_output` (~2072), `request_direct_unflip` (~427), `direct_frames_are_under_cow` (~408), `live_direct_vrr_wanted_by_output` (~1171), `current_primary_framebuffer_for_output`; in `backend_scanout_maybe_composite` the successor dispatch (~3560) and unflip processing (~3617) iterate the active owners
- Modify: `kms/present.rs` callers; `kms/platform_hooks.rs` completion/Idle drain (~795) drains every slot, and its fallback-materialization call (~329) passes the scope; lifecycle teardown callers `kms/session.rs` (~538) and `kms/randr.rs` (~1349); `backend/for_tests.rs` `for_tests_with_direct_submitter` (~619) and the injected test submitters
- Test: `tests/commit_identity/direct_unflip.rs` (recording fixture), `tests/present_scanout.rs`

**Interfaces:**
- Consumes: Task 1's `DirectOwners`.
- Produces:
  - `pub(in crate::kms::render::backend) enum DirectScope { Root, Output(usize) }` (`Clone, Copy, Debug, PartialEq, Eq`).
  - On `DirectOwners`: `fn get(&self, scope: DirectScope) -> Option<&ScanoutM2State>`, `fn get_mut(&mut self, scope: DirectScope) -> Option<&mut ScanoutM2State>`, `fn active_scopes(&self) -> Vec<DirectScope>` (root if active, else active output slots in index order), `fn scope_for_output(&self, output_idx: usize) -> Option<DirectScope>`. Invariant: root and output owners never active together.
  - On `KmsBackend`: `fn direct_scope_outputs(&self, scope: DirectScope) -> Vec<usize>` (`Root` → all outputs, `Output(i)` → `[i]`).
  - The submit/unflip/retire functions take the owner's scope (or derive it from `output_idx` via `scope_for_output`) and build plane states / replacements only for `direct_scope_outputs(scope)`. `Root` keeps the grouped transaction and grouped unflip exactly as today; `Output(i)` submits and unflips on CRTC *i* alone. `DirectFrameSubmitter` gains the scope (or reads it from the frame) — migrate every injected submitter.
  - `request_direct_unflip(reason)` keeps its meaning "every active owner" (global causes; teardown in `session.rs`/`randr.rs` uses it); `fn request_direct_unflip_for(&mut self, scope: DirectScope, reason: &'static str)` is added for one owner and gets its production callers in A3 (until then its caller is the scheduler's per-owner failure paths: successor-submit failures for that owner use it).
  - `retire_direct_output(output_idx, event_id, clock)` routes to `scope_for_output(output_idx)`.
  - The completion/Idle/skip drain collects from the root slot and every output slot.

- [ ] **Step 1: Failing tests** (recording fixture with two outputs; owners seeded through the existing direct-frame `for_tests` helpers, everything else through entry points):
  - `output_owner_submits_and_unflips_only_its_crtc`: an `Output(1)` owner's recorded direct commit and composed-unflip commit carry plane/CRTC properties for output 1's CRTC only. Mutation: build plane states for all outputs.
  - `two_output_owners_retire_independently`: `Output(0)` and `Output(1)` owners with distinct event identities and distinct clock samples; retiring output 1 completes owner 1 exactly once with output 1's sample and leaves owner 0 pending; then output 0 completes owner 0 with its own sample. Mutation: route retirement to the first active owner instead of `scope_for_output`.
  - `global_teardown_unflips_every_output_owner`: two `Output` owners held; drive the VT-release entry point → both owners stopped, both completions drained once, no pin left; repeat with a RANDR topology teardown. Mutation: the stop-all paths (`session.rs` ~567/~588 VT release, `scanout.rs` ~1621 topology teardown) stop only the root slot.
  - `stopped_output_owner_notifications_are_drained`: an `Output(1)` owner retires its unflip; the completion and Idle it queued are delivered through the normal drain. Mutation: drain the root slot only.
  - Root regression: the existing `p1_grouped_unflip_ignores_late_direct_or_duplicate_cookie` (`tests/commit_identity/direct_unflip.rs` ~693) passes unchanged.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement** as in Interfaces; pins and completions stay inside each owner record (unchanged code).
- [ ] **Step 4: Gate**; new tests 20× under load.
- [ ] **Step 5: Commit** `feat(scanout): direct submit, retirement, unflip and notifications act on the owner's outputs`.

### Task 3: Composition skips only outputs that are direct

**Files:**
- Modify: `backend/portable/compose.rs` (`compose_tick` ~94, generic over `XBackend<P>` ~11, which supplies every output), `crates/yserver-render/src/kms/render/scene/tick.rs` (`SceneCompositor::tick`, the per-output visit ~236) and their callers: `surface/platform.rs` ~1053 (passes an all-false mask), `kms/session.rs` ~419, `crates/yserver-render/src/kms/render/scene/tests/tick.rs` ~355
- Modify: `kms/scanout.rs` — `backend_scanout_maybe_composite` early returns (~3569–3612) and the held cursor-only loop (~3586); scene readiness (`scene_wants_compose` ~3029); held-direct wakeup (~3346); `backend_scanout_next_wakeup`; the reentry barrier (`reentry_blocked_until_composed`)
- Modify: `kms/scanout.rs` queued-successor dispatch (`submit_queued_direct_successor` ~1429): its scene-flip-pending check considers only the owner's outputs. (The Present-time admission check, `kms/present.rs` ~924, is scoped in A2 together with `Output` admission, since A1 Presents can only be `Root`.)
- Test: models `tests/tearing.rs` `live_scene_backend_with_outputs(2)`, `tests/cursor_sprites/scene_attach.rs` `held_direct_with_two_outputs`; `tests/present_scanout.rs`

**Interfaces:**
- Consumes: Task 2's scope API.
- Produces:
  - `fn output_is_direct(&self, output_idx: usize) -> bool` on `KmsBackend`: an owner of that output is pending, held, or awaiting its unflip replacement there.
  - **Fresh/degraded unflip passes render only the owner's outputs.** The fresh composed unflip (~1909) sets the global scene-flip deferral and settles every deferred frame with the replacement's accepted identity (~1911, ~1945; `platform/scene_sink.rs` ~138), so that pass uses the inverse mask — compose exactly `direct_scope_outputs(scope)` and skip every other output; unrelated outputs compose in a normal pass (the same tick after the replacement commit is submitted is not allowed — they compose on the next pass, as today's "do not fall through" rule). The degraded unflip (~3645) likewise renders only the owner's outputs.
  - Output selection seam: the mask is passed explicitly — `compose_tick(…, skip_outputs: &[bool])` on the generic `XBackend<P>` and `SceneCompositor::tick(…, skip_outputs: &[bool])` (indexed by the platform output index, same length as the outputs; callers without direct scanout pass all `false`); the scene tick skips masked outputs (no render, no flip, damage kept for later) and visits the rest exactly as today. The mask is `output_is_direct` per output. Output indices are preserved (no compaction).
  - The early returns in `backend_scanout_maybe_composite` stop only work on owned outputs; every other output composes and flips. Readiness, wakeups and the reentry barrier are evaluated per owner's outputs; held cursor-only commits iterate the owner's outputs only. The existing per-output commit gate (`platform/gamma.rs` ~202) still decides CRTC busyness.

- [ ] **Step 1: Failing tests:**
  - `composed_output_flips_while_other_output_owner_held`: `Output(1)` held; damage a window on output 0; scheduling pass → a composed flip on output 0 only; owner 1 still current. Mutation: restore the root-wide early return.
  - `composed_output_flips_while_other_output_owner_pending_and_unflipping`: same with owner 1 pending, then unflipping: the fresh-unflip pass renders and commits only output 1's replacement and output 0 gets no pending acknowledgement from it; output 0 composes and flips on the next pass. Mutation: the fresh-unflip pass uses the normal mask (composes output 0 into the deferral).
  - `queued_successor_not_blocked_by_scene_flip_on_other_output`: a composed flip pending on output 0; owner 1's queued successor is submitted by `submit_queued_direct_successor` on the next pass, not deferred. Mutation: keep the global "any scene flip pending" check in `submit_queued_direct_successor`.
  - `next_wakeup_reflects_composed_output_while_other_is_direct`: owner 1 held, damage on output 0 → `next_wakeup` is due. Mutation: held-direct wakeup considers cursor work only.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Gate**; new tests 20× under load.
- [ ] **Step 5: Commit** `feat(scanout): composition and scheduling skip only outputs that are direct`.
