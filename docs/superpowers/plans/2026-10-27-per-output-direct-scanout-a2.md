# Per-output direct scanout — Sub-plan A2: `Output` admission

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Present whose window covers exactly one output enters, holds and leaves direct scanout on that output alone (an `Output` owner), independently of the other outputs' refresh, device and activity; `Root` behaves as today.

**Architecture:** One classification, `direct_candidate_scope`, decides `Root` / `Output(i)` / none for a candidate. Every admission gate that today measures "the root" measures the scope's rectangle and outputs instead (authority, scene eligibility, CRTC/topology, root overlay, cursor eligibility, M1 probe, async-flip capability). A1's owner container, scoped submit/retire/unflip and per-output composition carry the rest. A3 scopes the unflip causes and does the hardware validation.

**Tech Stack:** Rust, yserver KMS backend.

**Spec:** `docs/superpowers/specs/2026-10-27-per-output-direct-scanout-design.md` (design items 1, 2, 5, 6, 7, 9). Sub-plan A1 (done, f2baedad) produced `DirectScope`, `DirectOwners` (`root()`, `get`, `get_mut`, `active_scopes`, `scope_for_output`, `sync_output_count`), `KmsBackend::direct_scope_outputs`, `request_direct_unflip_for`, `output_is_direct` and the per-output compose skip mask.

## Global Constraints

- Reuse first: extend the existing gates (`direct_candidate_authoritative_root`, `direct_candidate_gate`, `direct_shape_chain_covers_root`, `direct_shape_chain_hides_root`, `unredirected_direct_scene_eligible`, `direct_present_crtc_eligible`, `direct_scanout_topology_eligible`, `direct_cursor_eligible`, `maybe_probe_scanout_m1`, `tearing_gate_ignoring_cursor`) with a scope/rectangle parameter instead of adding parallel ones. Every new item justified in the report.
- `Root` admission, grouped submission and grouped unflip unchanged, including the homogeneous-topology rule (primary device, refresh within 0.5%) and "outputs tile the root". Single-monitor behaviour unchanged.
- An `Output(i)` candidate requires: window rect == output *i*'s rect, source extent == output *i*'s size, target Unredirected/COW/COW descendant, output *i*'s CRTC on the primary DRM device, **the Present's selected CRTC == output *i*'s CRTC** (otherwise composed), and nothing about other outputs.
- Root and Output owners never coexist; entering one kind while the other exists first unflips the existing owner(s) through the existing composed-unflip path.
- One Present → one completion in the selected CRTC's domain; pins released after the last consumer.
- Tearing has one owner (user decision); its effects are scoped to the owner's outputs.
- No `YSERVER_*` env vars; no `cfg(test)` in production bodies.
- Tests A/B/F as in A1 (drive Present / scheduling / page-flip entry points; A2 tests reach `Output` owners through real Presents, no seeding); 20 loaded runs for new tests.
- Gate per task as in A1 (fmt, build, workspace tests, lavapipe `--ignored`, `render_acceptance --ignored`, clippy, layout, FreeBSD clippy).
- Licence: no KWin/Mutter/Muffin sources. Commits signed with the Co-Authored-By trailer.

## Review Focus

1. Mixed refresh (240 Hz + 75 Hz): `Output` admitted on the 240 Hz output while `Root` stays rejected — Task 1.
2. The Present selected the other output's CRTC (window dragged between monitors, or a client choosing a CRTC): composed, never direct, no mis-stamped completion — Task 1.
3. Two `Output` owners at once (game on one monitor, fullscreen video on the other), each completing in its own CRTC domain — Task 2.
4. `Root`↔`Output` change (a window goes from spanning the screen to one monitor, or a monitor is unplugged under it): one unflip, one completion per Present, no stale frame — Task 2.
5. GetImage of the game's region while it is direct on an output at a nonzero origin returns the game's pixels — Task 4.

---

### Task 1: Scope classification and per-scope admission

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/kms/present.rs` (`direct_candidate_authoritative_root` ~25, `direct_candidate_gate` ~70, the live admission/successor path including the "scene page flip pending" fallback ~924, probation streak)
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs` (`scanout_m2_is_authoritative_root` ~27, `scanout_m1_probe_eligible` ~245, `direct_shape_chain_covers_root`/`hides_root` free fns ~114/~162 and wrappers ~2863, `unredirected_direct_scene_eligible` ~2927, `direct_present_crtc_eligible` ~2543, `direct_scanout_topology_eligible` ~2522, `direct_cursor_eligible` ~464, `maybe_probe_scanout_m1` ~2560, `observe_scanout_m0` coverage diagnostics)
- Test: `tests/present_scanout.rs`; two-output fixtures to extend: `tests/commit_identity/direct_unflip.rs` `submit_two_output_owner_successors` (~283) and `tests/tearing.rs` `live_scene_backend_with_outputs` (~2706); they must be able to give the outputs different refresh rates and a nonzero second-output origin.

**Interfaces:**
- Produces:
  - `fn direct_candidate_scope(&self, candidate: PresentScanoutCandidate, target: ScanoutM0Target) -> Option<DirectScope>` on `KmsBackend` (in `present.rs`): `Root` when today's root-authority test holds; `Output(i)` when the window rect and source extent equal output *i* (`observe_scanout_m0`'s existing `Output(idx)` classification is the reference) and target is Unredirected/COW/COW descendant; the hidden-COW rule (#299) applies to the scope's rectangle (`hides_root` takes a rectangle).
  - `fn direct_scope_rect(&self, scope: DirectScope) -> vk::Rect2D` (root rect or output *i*'s rect).
  - `direct_candidate_gate` evaluates every term against the scope: shape chain covers `direct_scope_rect(scope)`; `unredirected_direct_scene_eligible` for that rectangle (the scene's per-output top-level/COW decision, `yserver-render` `scene/build.rs` ~310–328, is the reference for "topmost on output *i*"); root overlay empty **within the scope's outputs** (`root_overlay.rs` `apply_list_for_output` ~108); cursor eligible on the scope's outputs; CRTC eligibility: `Root` → today's `direct_present_crtc_eligible` + topology rule, `Output(i)` → output *i*'s CRTC on the primary device, live epoch, and `candidate.crtc_id ==` output *i*'s CRTC.
  - M1 probe for `Output(i)`: TEST_ONLY with only CRTC *i*'s primary plane, source (0,0) and output *i*'s size; modifier support checked on that plane only; "outputs tile the root" only for `Root`; probe cache key `(source_id, scope)`.
  - Live admission: the scope picks the owner slot (`DirectOwners::get_mut(scope)`); probation streak per slot; the "scene page flip pending" fallback (~924) and successor arbitration look only at `direct_scope_outputs(scope)`. A candidate whose scope differs in kind from existing owners (Root vs Output) is rejected for this Present and requests the existing owners' unflip (`request_direct_unflip`), so the next eligible Presents enter cleanly.
- [ ] **Step 1: Failing tests:**
  - `output_present_enters_direct_on_its_output_only`: two outputs (output 0 at +0+0 75 Hz, output 1 at +1920+0 240 Hz, root 3840×1080); a client window covering output 1 presents an output-sized pixmap with `crtc_id` = output 1's CRTC repeatedly → after the probation run the direct commit carries only output 1's CRTC/plane with `SRC_X/Y == 0`, CompleteNotify is stamped from output 1's clock, and output 0 keeps composed flips. Mutation: keep the homogeneous-topology rule for `Output`.
  - `output_present_selecting_other_crtc_is_composed`: same window, `crtc_id` = output 0's CRTC → never direct; completions from output 0's domain as today. Mutation: drop the `candidate.crtc_id` check.
  - `mixed_refresh_root_present_still_rejected`: same topology, a root-covering window → composed (unchanged). Mutation: apply the `Output` CRTC rule to `Root`.
  - `output_probe_tests_only_its_crtc`: the recorded TEST_ONLY request for the `Output(1)` probe contains only output 1's plane; a second probe of the same buffer as `Root` (same-refresh topology) is a separate cache entry. Mutation: key the cache by `source_id` only.
  - `root_overlay_on_other_output_does_not_block_output_admission`: a software-cursor/overlay sprite on output 0 only → output 1 still admitted; a sprite on output 1 → rejected. Mutation: keep `root_overlay.is_empty()`.
  - Single-output and same-refresh dual-`Root` regressions: existing tests unchanged.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Gate**; new tests 20× under load.
- [ ] **Step 5: Commit** `feat(scanout): admit output-covering Presents as per-output direct owners`.

### Task 2: Transitions and concurrent owners

**Files:**
- Modify: `kms/present.rs` (live admission path from Task 1), `kms/scanout.rs` (owner entry after an unflip of the other kind)
- Test: `tests/present_scanout.rs`, `tests/commit_identity/direct_unflip.rs`

**Interfaces:**
- Consumes: Task 1's `direct_candidate_scope`, per-scope admission.
- Produces no new API; pins the transition behaviour of Task 1's "different kind → unflip existing owners first" rule and concurrent `Output` owners.
- [ ] **Step 1: Failing tests:**
  - `two_output_owners_enter_and_complete_independently`: two windows, each covering one output with its own CRTC selected; both enter direct; each Present completes once in its own CRTC's domain; a retirement on one output never completes the other's Present. Mutation: a second `Output` owner unflips the first.
  - `root_to_output_transition_unflips_once`: same-refresh topology; a root-covering window is direct (`Root`); it is resized to cover output 1 only → the `Root` owner unflips through the grouped path, its last Present completes once, its pins are released, and after the probation run output 1 enters as `Output(1)`. Mutation: admit `Output(1)` while the `Root` owner is still active.
  - `output_to_root_transition_unflips_every_output_owner`: two `Output` owners; a root-covering Present arrives on a same-refresh topology → both owners unflip, each completes once, then `Root` enters after probation. Mutation: unflip only the owner overlapping the new Present's selected CRTC.
  - `output_unplugged_under_output_owner`: RANDR removes output 1 while it has an owner → it unflips/stops, no pin left, `DirectOwners` shrinks (A1's `sync_output_count`). Mutation: skip the stop for `Output` owners on topology change.
- [ ] **Step 2: Run, verify they fail** (or, if already green from Task 1, show the mutation fails and say so in the report).
- [ ] **Step 3: Implement** what the tests require.
- [ ] **Step 4: Gate**; new tests 20× under load.
- [ ] **Step 5: Commit** `feat(scanout): Root/Output transitions and concurrent output owners`.

### Task 3: Cursor and tearing scoped to the owner's outputs

**Files:**
- Modify: `kms/scanout.rs` (`direct_cursor_eligible` ~464, `bind_direct_cursor_on_outputs` ~588 and the external cursor synchronisation it records ~585, `refresh_direct_cursor_on_all_outputs` ~740, `tearing_cursor_compose_cap_applies` ~3315, `prepare_tearing_scene_pass` ~3610), `kms/present.rs` (`tearing_gate_ignoring_cursor` ~94, `tearing_scene_still_owned`), `backend/portable/compose.rs` (~116 `tearing_active` use), `crates/yserver-render/src/kms/render/scene/cursor.rs` (external-cursor sync pending-acknowledgement check ~206, cursor-mode query ~178)
- Test: `tests/cursor_sprites/scene_attach.rs` (`held_direct_with_two_outputs` ~2775), `tests/tearing.rs`

**Interfaces:**
- Consumes: Tasks 1–2.
- Produces: the cursor-mode / external-sync queries take the outputs to consider (`&[usize]`); the direct path passes `direct_scope_outputs(scope)`. Tearing: the async-flip capability check in `tearing_gate_ignoring_cursor` covers the candidate's scope outputs only; hardware-cursor suppression and async composed timing apply only to the tearing owner's outputs. One tearing owner at a time (unchanged).
- [ ] **Step 1: Failing tests:**
  - `composed_cursor_transition_on_other_output_keeps_direct_cursor`: `Output(1)` owner with its cursor bound; the cursor crosses into output 0 and output 0 has a pending scene frame → owner 1 is not revoked (`cursor_scene_sync_failed` not requested), output 0's cursor composes/updates normally. Mutation: keep the all-output pending-acknowledgement check.
  - `cursor_image_change_on_composed_output_does_not_unflip_other`: cursor on output 0 changes image while owner 1 is held → owner 1 stays current. Mutation: aggregate cursor mode across all outputs.
  - `tearing_on_output_owner_scopes_cursor_and_async_timing`: an `Output(1)` tearing owner (async Presents) → output 1 uses async flips and suppressed hardware cursor; output 0 keeps its hardware cursor and vsync composed timing. Mutation: apply tearing effects to all outputs.
  - `second_output_tearing_request_flips_synchronised`: a tearing owner on output 1; a second client on output 0 asks for async → it is direct with synchronised flips (no async flag on output 0's commits). Mutation: allow a second tearing owner.
- [ ] **Step 2–5:** run/fail, implement, gate + 20× load, commit `feat(scanout): direct cursor and tearing scoped to the owner's outputs`.

### Task 4: Readback rebase for `Output` owners

**Files:**
- Modify: `kms/readback.rs` (`direct_scanout_route_for_rect` ~36)
- Test: `tests/root_scanout.rs` (models `split_root_scanout_reads_spanning_two_outputs` ~928)

**Interfaces:**
- Consumes: Tasks 1–2.
- Produces: the root→source rebase subtracts the owner's paint-target origin (the window's absolute origin; for `Output(i)` that is output *i*'s origin) as well as `x_off/y_off`, so a root-absolute rect on output 1 maps to the source's own coordinates.
- [ ] **Step 1: Failing test** `get_image_of_output_owner_at_nonzero_origin_reads_client_pixels`: `Output(1)` owner at +1920+0 with a source filled with a known pattern; GetImage of a rect inside output 1 returns that pattern; a rect spanning both outputs returns output 0's composed pixels for its part and the client's for the rest. Mutation: rebase by `x_off/y_off` only.
- [ ] **Step 2–5:** run/fail, implement, gate + 20× load, commit `fix(scanout): readback rebases output-owner reads by the output origin`.
