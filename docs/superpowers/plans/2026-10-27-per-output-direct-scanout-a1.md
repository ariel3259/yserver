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
3. A RANDR topology change while an owner exists resizes the per-output slots without leaking a frame or a pin — Task 1 test.
4. An `Output` owner's retirement/unflip never touches another output's CRTC and completes exactly once — Task 2 tests.
5. Composition on output *j* proceeds while output *i* has an `Output` owner pending, held or unflipping — Task 3 tests.

---

### Task 1: `DirectOwners` container (pure migration)

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/kms/state.rs` (`ScanoutM2State` ~261, `scanout_m2` field ~812)
- Modify: every production and test site using `pstate.scanout_m2` (~185 production, ~300 test; `kms/scanout.rs`, `kms/present.rs`, `kms/session.rs`, `kms/platform_hooks.rs`, `kms/randr.rs`, `kms/crtc_config.rs`, `tests/**`)

**Interfaces:**
- Produces, in `kms/state.rs`:
  - `pub(in crate::kms::render::backend) enum DirectScope { Root, Output(usize) }` (`Clone, Copy, Debug, PartialEq, Eq`)
  - `pub(in crate::kms::render::backend) struct DirectOwners { root: ScanoutM2State, outputs: Vec<ScanoutM2State> }` replacing the field `scanout_m2` (field name: `direct`)
  - `fn root(&self) -> &ScanoutM2State`, `fn root_mut(&mut self) -> &mut ScanoutM2State`
  - `fn get(&self, scope: DirectScope) -> Option<&ScanoutM2State>`, `fn get_mut(&mut self, scope: DirectScope) -> Option<&mut ScanoutM2State>` (`Output(i)` out of range → `None`)
  - `fn active_scopes(&self) -> Vec<DirectScope>` (root if `root.active()`, else each active output slot in index order)
  - `fn scope_for_output(&self, output_idx: usize) -> Option<DirectScope>` (`Root` if the root owner is active, else `Output(output_idx)` if that slot is active)
  - `fn sync_output_count(&mut self, count: usize)` — grows/shrinks `outputs`; a removed slot must be inactive (callers unflip first; see test)
- Invariant: the root owner and any output owner are never active at the same time.

- [ ] **Step 1: Migrate.** Replace the field and every `pstate.scanout_m2` access with `pstate.direct.root()` / `root_mut()`. No other change. Call `sync_output_count(self.platform.outputs.len())` wherever `platform.outputs` is rebuilt (RANDR apply, rescan, VT acquire — find them by the existing `scanout_m1.clear("output topology changed")` / topology-signature sites).
- [ ] **Step 2: Tests.**
  - `direct_owners_root_and_output_scopes_are_exclusive` (unit, `state.rs` tests or `tests/present_scanout.rs`): a fresh container has no active scope; seeding the root makes `active_scopes() == [Root]` and `scope_for_output(1) == Some(Root)`; seeding output 1 only gives `[Output(1)]`, `scope_for_output(0) == None`. Mutation: `scope_for_output` ignores the root owner.
  - `direct_owners_follow_output_count_on_topology_change`: drive a RANDR change on the dual-head simulated fixture (`tests/randr.rs` `simulated_kms_dual_head`) from two outputs to one with no owner active → `outputs.len() == 1`; with a direct root owner active, the existing unflip happens first and no pin is left (assert the source/fallback pins are released and the completion queue holds exactly one completion). Mutation: never call `sync_output_count`.
- [ ] **Step 3: Gate**, all existing tests unchanged in meaning (only the access path changes).
- [ ] **Step 4: Commit** `refactor(scanout): direct-scanout owners container (root slot + per-output slots)`.

### Task 2: Owner-scoped submit, retirement and unflip

**Files:**
- Modify: `kms/scanout.rs` — `submit_direct_frame_live` (~946), `release_direct_frame`, `queue_direct_successor`, `submit_queued_direct_successor`, `stop_direct_after_scanout_replaced`, `direct_scanout_frame_for_output` (~1637), `materialize_direct_shadow_for_unflip`, `submit_composed_unflip`, `fresh_composed_unflip_eligible`, `submit_fresh_composed_unflip`, `commit_fresh_composed_unflip`, `record_accepted_degraded_unflip_replacements`, `retire_direct_output` (~2072), `request_direct_unflip` (~427), `direct_frames_are_under_cow` (~408), `live_direct_vrr_wanted_by_output` (~1171), `current_primary_framebuffer_for_output`
- Modify: `kms/present.rs` callers
- Test: `tests/commit_identity/direct_unflip.rs` (recording fixture), `tests/present_scanout.rs`

**Interfaces:**
- Consumes: Task 1's `DirectScope`, `DirectOwners`.
- Produces:
  - `fn direct_scope_outputs(&self, scope: DirectScope) -> Vec<usize>` on `KmsBackend` (`Root` → `0..outputs.len()`, `Output(i)` → `[i]`).
  - `fn request_direct_unflip(&mut self, reason: &'static str)` keeps its signature and meaning "every active owner" (global causes). New `fn request_direct_unflip_for(&mut self, scope: DirectScope, reason: &'static str)` for one owner. A3 moves causes from the former to the latter.
  - The submit/unflip/retire functions above take the owner's `scope` (or derive it from `output_idx` via `scope_for_output`) and build plane states / replacements only for `direct_scope_outputs(scope)`. `Root` keeps the grouped transaction and grouped unflip exactly as today; `Output(i)` submits and unflips on CRTC *i* alone.
  - `retire_direct_output(output_idx, event_id, clock)` routes to `scope_for_output(output_idx)`; an output with no owner returns `false` as today's "not ours".
  - `direct_frames_are_under_cow` and `live_direct_vrr_wanted_by_output` read the owner of each output (no behaviour change for `Root`).

- [ ] **Step 1: Failing tests** (recording fixture with two outputs; seed an `Output(1)` owner through the existing direct-frame `for_tests` helpers, then drive entry points):
  - `output_owner_submits_and_unflips_only_its_crtc`: the recorded direct commit and the composed-unflip commit carry plane/CRTC properties for output 1's CRTC only; output 0's CRTC never appears in them. Mutation: build plane states for all outputs.
  - `output_owner_completes_once_after_its_crtc_retires`: retirement on output 1 completes the Present exactly once with output 1's clock sample; a page-flip event on output 0 does not touch the owner. Mutation: route retirement by "any owner" instead of `scope_for_output`.
  - `request_direct_unflip_for_leaves_other_owner_held`: two `Output` owners (0 and 1); `request_direct_unflip_for(Output(0), "test")` → owner 0 unflips, owner 1 stays current with `unflip_requested == false`; `request_direct_unflip("global")` → both. Mutation: `request_direct_unflip_for` sets every owner.
  - `root_owner_grouped_unflip_unchanged` (two outputs, same refresh): root owner's unflip is one grouped commit over both CRTCs, as today. Mutation: unflip root per output.
- [ ] **Step 2: Run, verify they fail** for the missing scope parameter / routing.
- [ ] **Step 3: Implement** as in Interfaces. Pins and completions stay per owner record (unchanged code inside the record).
- [ ] **Step 4: Gate**, plus the new tests 20× under load.
- [ ] **Step 5: Commit** `feat(scanout): direct submit, retirement and unflip act on the owner's outputs`.

### Task 3: Output-scoped scheduling

**Files:**
- Modify: `kms/scanout.rs` — `backend_scanout_maybe_composite` (early returns ~3569–3660, held cursor-only loop ~3586), scene readiness (`scene_wants_compose` ~3029), held-direct wakeup (~3346), `backend_scanout_next_wakeup`, reentry barrier (`reentry_blocked_until_composed`); `phase_b_flip_in_flight_for_scheduler` callers
- Modify: `kms/present.rs` — admission's "scene page flip pending" fallback (~924) and successor arbitration
- Test: `tests/tearing.rs` model `live_scene_backend_with_outputs(2)`, `tests/cursor_sprites/scene_attach.rs` `held_direct_with_two_outputs`, `tests/present_scanout.rs`

**Interfaces:**
- Consumes: Task 1–2 (`scope_for_output`, `direct_scope_outputs`).
- Produces: `fn output_is_direct(&self, output_idx: usize) -> bool` (owner pending, held, or awaiting its unflip replacement on that output). The compose pass composes and flips every output for which it is false, and skips only outputs for which it is true; admission and successor arbitration of an owner consider only scene flips pending on that owner's outputs; readiness, wakeups and the reentry barrier are evaluated per owner's outputs; held cursor-only commits iterate the owner's outputs only. No new scheduler: the existing per-output commit gate decides CRTC busyness.

- [ ] **Step 1: Failing tests:**
  - `composed_output_flips_while_other_output_owner_held`: `Output(1)` owner held; damage a window on output 0; run the scheduling pass → output 0 gets a composed flip, output 1 gets none, owner 1 still current. Mutation: restore the root-wide early return.
  - `composed_output_flips_while_other_output_owner_pending_and_unflipping`: same with owner 1 pending (direct transaction not retired) and then unflipping. Mutation: early return while any owner pending.
  - `pending_scene_flip_on_other_output_does_not_block_admission`: a composed flip pending on output 0; a direct successor for `Output(1)` is submitted, not deferred/unflipped. Mutation: keep the global "any scene flip pending" check in admission.
  - `next_wakeup_reflects_composed_output_while_other_is_direct`: owner 1 held, damage on output 0 → `next_wakeup` is due (not only cursor-driven). Mutation: held-direct wakeup considers cursor work only.
  - Single-output regressions stay green (existing tests).
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Gate**, new tests 20× under load.
- [ ] **Step 5: Commit** `feat(scanout): composition and scheduling skip only outputs that are direct`.
