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
3. Two `Output` owners at once (game on one monitor, fullscreen video on the other), each completing in its own CRTC domain; a tearing game on one is not disturbed by the other — Tasks 1–2.
4. `Root`↔`Output` change (a window goes from spanning the screen to one monitor, or a monitor is unplugged under it): one unflip, one completion per Present, no stale frame — Task 1.
5. GetImage of the game's region while it is direct on an output at a nonzero origin returns the game's pixels — Task 3.

---

### Task 1: Scope classification, per-scope admission, transitions

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/kms/present.rs` (`direct_candidate_authoritative_root` ~25, `direct_candidate_gate` ~70, `note_tearing_present` ~151 only as far as Task 1's authority change requires — the arbitration itself is Task 2 — the live admission/successor path including the "scene page flip pending" fallback ~924, probation streak)
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs` (`scanout_m2_is_authoritative_root` ~27, `scanout_m1_probe_eligible` ~245, `scanout_m1_outputs_cover_root` ~302, `direct_shape_chain_covers_root`/`hides_root` free fns ~114/~162 and wrappers ~2863, `unredirected_direct_scene_eligible` ~2927, `direct_present_crtc_eligible` ~2543, `direct_scanout_topology_eligible` ~2522, `direct_cursor_eligible` ~464/~484, `maybe_probe_scanout_m1` ~2560 incl. cache lookup ~2595 / insert ~2787, every other probe-cache lookup: direct submission, framebuffer-property retries, async timing)
- Modify: `crates/yserver-render/src/kms/render/scene/cursor.rs` (cursor-mode query ~178 takes the outputs)
- Modify: `crates/yserver/src/kms/render/backend/kms/state.rs` (`ScanoutM1ProbeCache` ~194: key, `remove` ~205 and every caller that evicts a drawable; `DirectScope` derives `Hash`); probe test seam next to `DirectFrameSubmitter` (~248) and its `for_tests` installer next to `for_tests_with_direct_submitter` (`backend/for_tests.rs` ~619)
- Modify: fixtures that insert probe-cache entries (`tests/commit_identity/recording_scene.rs` ~218 and any other insertion the compiler reports)
- Test: `tests/present_scanout.rs`; two-output fixtures extended so outputs can have different refresh rates and a nonzero second-output origin (`tests/commit_identity/direct_unflip.rs` `submit_two_output_owner_successors` ~283, `tests/tearing.rs` `live_scene_backend_with_outputs` ~2706); `tests/randr.rs` (~3754, A1's teardown scenario)

**Interfaces:**
- Produces:
  - `fn direct_candidate_scope(&self, candidate: PresentScanoutCandidate, target: ScanoutM0Target) -> Option<DirectScope>` on `KmsBackend` (in `present.rs`): `Root` when today's root-authority test holds; `Output(i)` when the window rect and source extent equal output *i* (`observe_scanout_m0`'s existing `Output(idx)` classification is the reference) and target is Unredirected/COW/COW descendant; the hidden-COW rule (#299) applies to the scope's rectangle (`hides_root` takes a rectangle).
  - `fn direct_scope_rect(&self, scope: DirectScope) -> vk::Rect2D` (root rect or output *i*'s rect).
  - `fn direct_cursor_eligible_for(&self, outputs: &[usize]) -> bool` — the existing `direct_cursor_eligible` becomes `direct_cursor_eligible_for(&all outputs)`; admission uses the scope's outputs. Its scene dependency, the cursor-mode query (`crates/yserver-render/src/kms/render/scene/cursor.rs` ~178, aggregate over every output today), gains the outputs to consider (`&[usize]`) in this task; Task 2 uses it for the remaining consumers.
  - `direct_candidate_gate` evaluates every term against the scope: shape chain covers `direct_scope_rect(scope)`; `unredirected_direct_scene_eligible` for that rectangle (the scene's per-output top-level/COW decision, `yserver-render` `scene/build.rs` ~310–328, is the reference for "topmost on output *i*"); root overlay empty **within the scope's outputs** (`root_overlay.rs` `apply_list_for_output` ~108); `direct_cursor_eligible_for(scope outputs)`; CRTC eligibility: `Root` → today's `direct_present_crtc_eligible` + topology rule, `Output(i)` → output *i*'s CRTC on the primary device, live epoch, the candidate's selected CRTC resolved through the RANDR route (`present_crtc_output`) is output *i* (never compare a protocol CRTC XID with a raw DRM handle), and output *i*'s RANDR size equal to its mode size (the per-output part of `scanout_m1_outputs_cover_root` ~302, which stays whole for `Root`).
  - Probe cache keyed by `(DrawableId, DirectScope)`; `remove(drawable)` drops every scope's entry for that drawable; every lookup/insert migrates. M1 probe for `Output(i)`: TEST_ONLY with only CRTC *i*'s primary plane, source (0,0) and output *i*'s size; modifier support checked on that plane only.
  - **Probe test seam:** the probe's PRIME import + ADDFB + TEST_ONLY step goes through a function pointer installed like `DirectFrameSubmitter`: `type ScanoutM1Prober = fn(&KmsBackend, DrawableId, DirectScope, &[DirectScanoutPlaneState<'_>]) -> io::Result<ScanoutM1ProbeEntry>` — an immutable backend reference, because the plane states borrow `&Output` from `platform.outputs` (`drm/modeset.rs` ~1963, `kms/scanout.rs` ~2730); the cache insertion stays in `maybe_probe_scanout_m1` after the call. The live value is today's import/ADDFB/TEST_ONLY code; if any of it needs `&mut` state, take it before building the plane states. Tests install a recorder (it records `(scope, planes)` into a test-owned cell) that returns an accepting or rejecting entry. This is the only new seam; everything else is driven through Presents.
  - Live admission: the scope picks the owner slot (`DirectOwners::get_mut(scope)`); probation streak per slot; the "scene page flip pending" fallback (~924) and successor arbitration look only at `direct_scope_outputs(scope)`. A candidate whose kind (Root vs Output) differs from existing owners is not admitted for this Present and requests the existing owners' unflip (`request_direct_unflip`); later eligible Presents enter after probation. A second `Output` owner on another output never disturbs the first.
- [ ] **Step 1: Failing tests** (all through Presents; the probe recorder replaces only the import/ADDFB/TEST_ONLY step):
  - `output_present_enters_direct_on_its_output_only`: output 0 at +0+0 75 Hz, output 1 at +1920+0 240 Hz, root 3840×1080; a window covering output 1 presents an output-sized pixmap with `crtc_id` = output 1's CRTC repeatedly → after probation the direct commit carries only output 1's CRTC/plane with `SRC_X/Y == 0`, CompleteNotify is stamped from output 1's clock, output 0 keeps composed flips. Mutation: keep the homogeneous-topology rule for `Output`.
  - `output_present_selecting_other_crtc_is_composed`: same window, `crtc_id` = output 0's CRTC → never direct. Mutation: drop the `candidate.crtc_id` check.
  - `mixed_refresh_root_present_still_rejected`: same topology, a root-covering window → composed. Mutation: apply the `Output` CRTC rule to `Root`.
  - `output_mode_size_mismatch_is_rejected`: output 1's RANDR size differs from its mode size → not admitted. Mutation: drop the per-output size check.
  - `output_probe_per_scope`: two equal-sized same-refresh outputs; one pixmap presented by a window covering output 0, then the same window moved to cover output 1 (no topology change) → the recorder saw two TEST_ONLY probes, `(id, Output(0))` with only output 0's plane and `(id, Output(1))` with only output 1's plane; drawable retirement removes both entries. Mutation: key the cache by drawable only.
  - `root_overlay_on_other_output_does_not_block_output_admission`: draw an XOR/Invert root-overlay rectangle on output 0 through the Backend drawing path (assert the root overlay is non-empty) → output 1 still admitted; an overlay rectangle on output 1 → rejected. Mutation: keep `root_overlay.is_empty()`.
  - `two_output_owners_enter_and_complete_independently`: two windows, each covering one output with its own CRTC selected; both enter direct; each Present completes once in its own CRTC's domain. Mutation: entering a second `Output` owner unflips the first.
  - `root_to_output_transition_unflips_once` (same-refresh topology): a root-covering window is direct; resized to cover output 1 only → the `Root` owner unflips through the grouped path, its last Present completes once, pins released, then `Output(1)` enters after probation. Mutation: admit `Output(1)` while the `Root` owner is active.
  - `output_to_root_transition_unflips_every_output_owner`: two `Output` owners; a root-covering Present → both unflip, each completes once, then `Root` enters after probation. Mutation: unflip only the owner on the new Present's selected CRTC.
  - Extend A1's RANDR teardown test (`tests/randr.rs` ~3754) to enter the `Output` owner through Presents instead of seeding.
  - Single-output and same-refresh dual-`Root` regressions: existing tests unchanged.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Gate**; new tests 20× under load.
- [ ] **Step 5: Commit** `feat(scanout): admit output-covering Presents as per-output direct owners`.

### Task 2: Cursor and tearing scoped to the owner's outputs

**Files:**
- Modify: `kms/scanout.rs` — `bind_direct_cursor_on_outputs` ~588 and the external cursor synchronisation it records ~585, `refresh_direct_cursor_on_all_outputs` ~740, the scheduler's aggregate cursor check (~3876), `direct_frame_flip_timing` (~1586: async is refused while **any** output shows a hardware cursor → only the owner's outputs count), `tearing_cursor_compose_cap_applies` ~3315, `prepare_tearing_scene_pass` ~3610
- Modify: `kms/present.rs` — `tearing_gate_ignoring_cursor` ~94 (async capability of the scope's outputs only), `tearing_scene_still_owned`, `note_tearing_present` ~151 (a Present whose scope does not overlap the tearing owner's outputs neither exits nor replaces the owner; an overlapping one behaves as today)
- Modify: `backend/portable/compose.rs` (~116 `tearing_active`), `backend/portable/cursor.rs` (~694: the early return while any direct owner exists → skip only for outputs that are direct, so composed outputs refresh their cursor image)
- Modify: pointer routing — `kms/platform_hooks.rs` `move_hw_cursor` (~294: cross-CRTC membership detection is disabled whenever any direct owner exists → disabled only for direct outputs, reusing the existing output-membership check, so entering an idle composed output binds/shows its cursor plane; `platform/cursor.rs` ~1367 moves only bound planes) and `backend/portable/pointer.rs` (~622: a tearing owner routes all pointer motion away from hardware movement → only motion on the tearing owner's outputs)
- Modify: `crates/yserver-render/src/kms/render/scene.rs` (`SceneInput::hw_cursor_suppressed` ~132 becomes per output, indexed like the skip mask) and `scene/tick.rs` (~259 consumer); `scene/cursor.rs` (external-cursor sync pending-acknowledgement check ~206 takes the outputs; the mode query already did in Task 1)
- Test: `tests/cursor_sprites/scene_attach.rs` (`held_direct_with_two_outputs` ~2775), `tests/tearing.rs`

**Interfaces:**
- Consumes: Task 1 (`direct_cursor_eligible_for`, scopes).
- Produces: cursor-mode / external-sync queries take `&[usize]`; the direct path passes `direct_scope_outputs(scope)`. Hardware-cursor suppression and async composed timing apply only to the tearing owner's outputs. One tearing owner at a time (user decision), and an unrelated scope never disturbs it.
- [ ] **Step 1: Failing tests:**
  - `cursor_image_change_with_pending_ack_on_other_output_keeps_owner`: `Output(1)` owner held; output 0 has a scene frame whose acknowledgement is still pending; the client changes the cursor image (Backend `set_cursor`/DefineCursor path), which triggers the direct bind/refresh for owner 1 → owner 1 not revoked (`cursor_scene_sync_failed` not requested), output 1's cursor plane gets the new image, and output 0's cursor plane gets it once its frame retires. Mutations: keep the all-output pending-acknowledgement check; restore the portable early return for any direct owner.
  - `pointer_enters_idle_composed_output_while_other_is_direct`: `Output(1)` owner held, output 0 idle (no damage) with its cursor plane hidden; pointer motion from output 1 into output 0 → output 0's cursor plane is bound, shown and moved; repeat while owner 1 is the tearing owner. Mutations: keep `move_hw_cursor`'s any-owner gate; keep `pointer.rs`'s all-motion tearing route.
  - `tearing_on_output_owner_scopes_cursor_and_async_timing`: an `Output(1)` tearing owner → output 1 async flips with suppressed hardware cursor; output 0 keeps its hardware cursor and vsync composed timing; async is not refused because output 0 shows a hardware cursor. Mutation: keep `any_cursor_plane_visible()` over all outputs in `direct_frame_flip_timing`.
  - `second_output_tearing_request_does_not_disturb_owner`: tearing owner on output 1; a second client on output 0 presents async **past the tearing probation** → output 0 is direct with synchronised flips, output 1 still tears (owner unchanged). Mutation: `note_tearing_present` exits the owner on any other authoritative window.
- [ ] **Step 2–5:** run/fail, implement, gate + 20× load, commit `feat(scanout): direct cursor and tearing scoped to the owner's outputs`.

### Task 3: Readback rebase for `Output` owners

**Files:**
- Modify: `kms/readback.rs` (`direct_scanout_route_for_rect` ~36)
- Test: `tests/root_scanout.rs` (model `split_root_scanout_reads_spanning_two_outputs` ~928)

**Interfaces:**
- Consumes: Task 1.
- Produces: the root→source rebase subtracts the owner's paint-target origin (for `Output(i)`, output *i*'s origin) as well as `x_off/y_off`.
- [ ] **Step 1: Failing test** `get_image_of_output_owner_at_nonzero_origin_reads_client_pixels`: an `Output(1)` owner at +1920+0 entered through Presents, source filled with a known pattern; GetImage of a rect inside output 1 returns the pattern; a rect spanning both outputs returns output 0's composed pixels for its part and the client's for the rest. Mutation: rebase by `x_off/y_off` only.
- [ ] **Step 2–5:** run/fail, implement, gate + 20× load, commit `fix(scanout): readback rebases output-owner reads by the output origin`.
