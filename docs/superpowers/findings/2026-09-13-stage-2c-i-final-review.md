# Stage 2c-i final stage review — full sub-stage verification

## Verdict

**ACCEPTED.** All ten tasks of Phase C.0 stage 2c-i (`76a93356..HEAD`) have been
implemented, corrected across fix sessions F-1 through F-12, independently
verified through adversarial mutation checks, and proven sound against
`docs/handoff-phase-c0-stage-2c-i.md` rulings R1–R12, `docs/handoff-phase-c0-stage-2c-i-fix.md`
rules F1–F8, and the stage design contracts.

All 16 blocking findings (B-1..B-16) and 24 major findings (M-1..M-24) from
the round-1 implementation review (`2026-09-11-stage-2c-i-implementation-review-round1.md`)
are fully resolved with concrete, non-vacuous tests. The physical ownership regress
is closed: exactly one closer per kernel object (R3); two independently tracked
payload halves (`file_owned` and `shared`, R4); reachable fd-family barrier via
caller-driven payload alias discharge (R5); displacing commit's `HardwareComplete`
as `KmsRelease` producer (R6); transport gate enforced beneath every real DRM write
sink (R7, R11); zero raw `.storage.` derefs across the repository (M-19); and live
hardware/Vulkan validation tests passing cleanly without fabricated proofs (R8, R9, R12).

- **Reviewed range:** `76a93356..HEAD` (103 commits on `feat/phase-c0-atomic-kms-migration`).
- **Pre-commit verification gate:**
  - `cargo +nightly fmt --check`: Clean (0 diffs).
  - `cargo clippy --all-targets -- -D warnings`: Clean (0 warnings, 0 errors).
  - `cargo test -p yserver --lib c0_2ci`: 121 passed; 0 failed; 13 ignored.
  - Flake loop: 12 consecutive runs with 0 flakes.
  - Hardware test suite (`cargo test -p yserver --lib c0_2ci -- --ignored`): 13 passed; 0 failed (live DRM primary node + NVIDIA/RADV Vulkan ICDs with validation layers).
  - Full crate suite (`cargo test -p yserver --lib`): 1659 passed; 0 failed; 85 ignored.
  - Portable targets (`x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-unknown-freebsd`): Clean.

---

## Detailed Review by Scope

### Scope 1: Tasks 1–4 (Leases, Availability, Cleanup, Storage & Scanout)

1. **Rooted Allocation Leases & Availability Ledger (Task 1, R1, R9, R10):**
   - Implemented in `crates/yserver/src/kms/render/resources/{mod,availability,lease}.rs`.
   - Single authoritative availability state machine (`can_destroy`, `Retain`, `Read`, `Write`, `Kms` reservations).
   - Core-thread only (R10); zero `Send`/`Sync` leaks.
   - Vocabulary enforces private `apply_validated_proof` (`pub(in crate::kms::render::resources)` with test shims, closing M-17).
   - Proven via deterministic ledger tests: `c0_2ci_kms_release_does_not_complete_gpu_work`, `c0_2ci_reverse_order_gpu_before_kms`, `c0_2ci_two_retain_leases_require_both_drops_for_destruction`, `c0_2ci_drop_write_lease_with_pending_gpu_blocks_reuse`.

2. **Consuming DRM Cleanup & Reachable FD-Family Barrier (Task 2, R3, R5, B-1, B-2):**
   - Implemented in `crates/yserver/src/kms/render/resources/drm_cleanup.rs` and `drm/modeset.rs`.
   - `DrmCleanupRegistry` tracks `payload_alias_keys: BTreeSet<AllocationKey>`.
   - `try_mint_file_family_closed` takes a caller-supplied `discharge_payload_alias` closure, walking and closing all registered payload aliases before dropping its own `device` alias and minting `FileFamilyClosed`.
   - Barrier preconditions (`FamilyInventory`: `submitters_detached`, `helper_reaped`, `control_closed`, `non_payload_aliases`) are real, unconditional struct fields defaulting to unsatisfied, closing F1-B1.
   - `DrmCleanupRight` enforces `GemOwner::Right` (closes GEM on discharge) vs `GemOwner::Gbm` (never closes GEM; libgbm drops it), closing R3.
   - `DirectScanoutProbeFramebuffer` uses `ProbeFbOwnership { Legacy, Managed }`, eliminating legacy destructor races on managed adoptions.
   - Proven via `c0_2ci_drm_cleanup_fd_family_barrier_discharges_payload_alias`, `c0_2ci_drm_cleanup_fake_family_barrier_requires_all_closed`, `c0_2ci_drm_cleanup_fd_family_barrier_discharge_failure_retries`, and hardware test `c0_2ci_fd_family_barrier_real_gbm_payload_drm`.

3. **Storage Generations, DRI3 Buffers & Elimination of Raw Accessors (Task 3, B-14, M-18, M-19, M-20, M-21, F3-M1):**
   - Implemented in `crates/yserver/src/kms/render/store.rs`, `resources/storage.rs`, `scene.rs`, `frame_builder.rs`, `target.rs`, `engine.rs`, and `backend.rs`.
   - Physical fields extracted into `StorageAllocation`; logical facade operates via `StorageBacking { Legacy, Managed, Detached }`.
   - `StorageBacking::Detached` ensures destroyed managed drawables drop their leases immediately without fabricating an inert legacy stub (closing F3-m1).
   - Safe accessor methods implemented on `Storage` and `Drawable` (`extent()`, `format()`, `image()`, `image_view()`, `sample_view()`, `has_image_view()`, `current_layout()`, `set_current_layout()`, `imported_drawable()`, `export_metadata()`, `is_exportable()`).
   - Every single raw `.storage.` field deref across the repository eliminated (down from 202 unmigrated sites to exactly 0).
   - F3-M1 resolved: `is_exportable` and `record_layout_transition` safely handle `Managed` storage leases via `svc.with_storage_read` and guarded barriers without panicking.
   - DRI3 buffer identity preserved: modifier, stride, offset, and implicit/explicit layout preserved across adoption and export.
   - Promotion retirement (M-20) safely handles `RetiredPromotionPayload::Managed(StorageLease)` under fence tickets.
   - Proven via `c0_2ci_storage_managed_destroy_transitions_to_detached`, `c0_2ci_storage_managed_destroy_detaches_before_drop`, `c0_2ci_storage_is_exportable_managed_reserves_read_and_refuses_when_written`, `c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan`, `c0_2ci_storage_dri3_lease_regressions_vulkan`, `c0_2ci_storage_no_premature_pool_return_vulkan`, and `c0_2ci_engine_promote_drawable_exportable_managed_vulkan`.

4. **Shared/Copied Scanout Backing & Single-Closer Semantics (Task 4, R3, R4, B-12, B-13, M-22):**
   - Implemented in `crates/yserver/src/kms/render/resources/scanout.rs` and `kms/vk/scanout.rs`.
   - `ScanoutAllocation` splits ownership into `file_owned` (`Option<FileOwnedBacking>`) and `shared` (`Arc<SharedBacking>`).
   - Barrier discharge releases `file_owned` without destroying `shared` Vulkan images (R4).
   - `FileOwnedBacking::discharge` drops `gbm_bo` before `device` alias, matching physical order (M-22).
   - Tested single `CloseGem` per payload via counting `MockCleanupIo` on both success and retry paths.
   - Proven via `c0_2ci_scanout_file_owned_discharge_right_exactly_one_close_gem`, `c0_2ci_scanout_discharging_file_owned_leaves_shared_intact`, `c0_2ci_scanout_file_owned_pairing_gbm_and_right`, `c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan`.

---

### Scope 2: Tasks 5–7 (GPU/Read Adapters, Transport Gate, Commit Resources)

1. **GPU, Descriptors & Readback Adapters (Task 5, B-11, B-15, M-16, F-4d):**
   - Implemented in `crates/yserver/src/kms/render/resources/mod.rs`, `adapter_tests.rs`, and `backend.rs`.
   - Replaced simulator fixtures with real DRM and Vulkan execution paths.
   - Read source regressions run on real primary DRM nodes without master (`PermissiveDump` contract, F-4b).
   - Per-batch serviced-time deadlines implemented (`serviced_elapsed` tracked per batch, pausing during seat-inactive and DPMS-off, closing B-11 and M-16).
   - Managed scanout write path and retirement batches wired and tested (session F-4d).
   - Proven via `c0_2ci_read_source_scratch_regression_vulkan`, `c0_2ci_scene_managed_shared_compose_vulkan`, `c0_2ci_serviced_deadline_is_per_batch_not_global`, `c0_2ci_serviced_time_pauses_during_seat_inactive_and_expires`.

2. **Transport Permission Boundary at Real Sinks (Task 6, R7, R11, B-10, M-13, M-14):**
   - Implemented in `crates/yserver/src/kms/render/resources/transport.rs` and integrated across all DRM entry points.
   - Transport gate enforces `Legacy`, `Quiescing`, `Owner`, and `Closed` states.
   - `begin_quiescing` queries live `DirectOwnershipState` (M-13) and refuses while direct units are busy or unflip is pending.
   - `close()` refuses while owner writes are outstanding (M-14).
   - Transport gate authorization wired beneath all 7 real DRM sink paths:
     1. Legacy page flip (`page_flip.rs`)
     2. Direct atomic flip (`modeset.rs`)
     3. Composed unflip (`modeset.rs`)
     4. Modeset install (`modeset.rs`)
     5. Output disable / rollback (`modeset.rs`)
     6. Cursor plane set/move (`cursor_plane.rs`)
     7. Gamma ramp set (`backend.rs`)
   - Proven via `c0_2ci_transport_gate_vocabulary_and_table`, `c0_2ci_transport_gate_direct_scanout_precondition`, `c0_2ci_transport_gate_close_refuses_outstanding_grants`, `c0_2ci_sink_gamma_gate_four_states_drm`, and deterministic four-state tests for all six other sinks.

3. **Concrete Commit Resources & Present Dispositions (Task 7, R6, B-8, B-9, M-2, M-3, M-4, M-5, M-6):**
   - Implemented in `crates/yserver/src/kms/render/resources/commit.rs`, `backend.rs`, and `present_completion.rs`.
   - Replaced uninhabited `NeverResource` with `CommitResources`.
   - Displaced-pair producer: `HardwareComplete` registers and discharges `KmsRelease` obligations strictly for the displaced `old` set, never for the `new` set (R6).
   - Keyed discharge: obligations matched by `GroupMember` (CRTC key + generation + epoch) without dual bookkeeping (M-2, M-3).
   - `Terminal` event handles causes correctly: only `CompletionUnknown` freezes resources; `Completed` discharges and releases normally (B-8).
   - Atomic validation before application (M-4): mid-drain errors do not leave orphaned obligations.
   - Present dispositions wired: `PresentRelease` safely consumes present sources, wakes deferred waiters, and manages COW overlay release (M-6).
   - Proven via `c0_2ci_commit_hardware_complete_discharges_old_only`, `c0_2ci_commit_grouped_skip_and_duplicate_protection`, `c0_2ci_present_release_consumption_and_completion_suppression`, `c0_2ci_cow_deferred_release_and_reclaim_with_physical_contracts`.

---

### Scope 3: Tasks 8–10 (Roles, Handoff, Fixture Matrix & Validation Smoke)

1. **Six Physical Roles & Release-Safe Exit (Task 8, M-1, M-7, M-8):**
   - Implemented in `crates/yserver/src/kms/render/resources/capacity.rs` and `commit.rs`.
   - `DirectCapacity` manages the six physical roles: `DirectScanoutA`, `DirectScanoutB`, `Preparing`, `Successor`, `OrdinaryRetirement`, `ExitRetirement`.
   - Strict occupancy enforcement: `finish_role` rejects merely `Reserved` tokens (`ResourceError::InvalidProof`), closing M-7.
   - `on_available` uses staged rollback: on error, all popped resources are restored to `releasing_resources`/`rejected_resources`, admission closes, and no resource is dropped (M-1).
   - Bounded probe cache: `ScanoutM1ProbeCache` bounded to 32 entries with FIFO eviction.
   - Unflip re-entry contract: verifies both retirement roles are vacant before direct scanout re-entry.
   - Proven via `c0_2ci_capacity_transitions_and_move_into_reserved`, `c0_2ci_capacity_finish_role_rejects_merely_reserved_token`, `c0_2ci_capacity_on_available_error_restores_all_resources_safely`, `c0_2ci_backend_managed_prepare_direct_candidate_implicit_layout_rejection`, `c0_2ci_backend_managed_unflip_and_reentry_contracts`.

2. **Reserved Teardown Recipient & Late-Completion Handoff (Task 9, R7, R8, B-3, B-4, B-5, B-6, B-7, M-9, M-10, M-11, M-12):**
   - Implemented in `crates/yserver/src/kms/render/resources/handoff.rs`.
   - `DeviceBarrier` sealed: private constructors (`from_file_family_closed`, `device_lost`) prevent fabricated enum literals in tests or crate code (B-4).
   - `TeardownRelease` sealed: requires validated proof before minting (B-5).
   - `RecipientReservation`: production constructor eliminated; test fixture only under `#[cfg(test)]` (B-6).
   - Handoff transfer revokes owner writes (`revoke_owner_writes`) and quarantines pending grants (`Quarantined`, B-7).
   - `apply_teardown_release` validates `file_owned == None`, preventing ioctl-after-close (M-10).
   - Error propagation: `HandoffRouter` propagates errors instead of discarding with `let _` (M-9).
   - Proven via `c0_2ci_handoff_complete_fd_family_barrier_deterministic`, `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines`, `c0_2ci_handoff_unresolved_kms_rejects_teardown_release`, `c0_2ci_handoff_failure_returns_bundle_and_slot_by_value`.

3. **Concrete Adapters, Hardware Tests & Documentation (Task 10, R8, R12, B-16, M-23, M-24):**
   - Implemented in `crates/yserver/src/kms/render/resources/adapter_tests.rs`, `tests.rs`, `docs/status.md`.
   - Fixture matrix: all 12 scenarios implemented with real payload types, eliminating Spy substitutes in integration tests.
   - Live hardware smoke under validation layers: `c0_2ci_live_lifetime_adapters_vulkan` and `c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan` execute with `VK_LAYER_KHRONOS_validation` enabled, proving destruction order and zero validation errors/warnings on real GPU ICDs.
   - Real DRM tests: `c0_2ci_fd_family_barrier_real_gbm_payload_drm` and `c0_2ci_sink_gamma_gate_four_states_drm` execute on real DRM nodes.
   - Environmental reporting honesty (R12): missing ICD or node panics rather than reporting a fake pass; lavapipe claims removed.
   - All test-only constructors gated under `#[cfg(test)]` (M-23).
   - Production route strictly `Legacy` (R8): zero production activation of Owner writers.

---

## Adversarial Mutation Checks

To verify that the decisive mechanisms are genuinely enforced by the test suite,
five independent mutation checks were performed and verified across the codebase:

1. **Registry FD-Family Barrier (Scope 1, Task 2):**
   - *Mutation:* Comment out `self.device = None` in `DrmCleanupRegistry::try_mint_file_family_closed`.
   - *Result:* Test `c0_2ci_drm_cleanup_fd_family_barrier_discharges_payload_alias` failed immediately with:
     `panicked at crates/yserver/src/kms/render/resources/tests.rs:707:5: assertion failed: weak.upgrade().is_none()`
   - *Restored:* Cleanly reverted.

2. **Transport Gate Enforcement (Scope 2, Task 6):**
   - *Mutation:* Mutate `TransportGate::allows_legacy` to return `true` unconditionally regardless of state.
   - *Result:* Test `c0_2ci_transport_gate_vocabulary_and_table` failed immediately with:
     `panicked at crates/yserver/src/kms/render/resources/tests.rs:2457:9: assertion failed: !gate.allows_legacy(class)`
   - *Restored:* Cleanly reverted.

3. **Strict Direct Role Occupancy (Scope 3, Task 8):**
   - *Mutation:* Mutate `DirectCapacity::finish_role` to accept `RoleState::Reserved` in addition to `Occupied`.
   - *Result:* Test `c0_2ci_capacity_finish_role_rejects_merely_reserved_token` failed immediately with:
     `panicked at crates/yserver/src/kms/render/resources/tests.rs:4293:59: called Result::unwrap_err() on an Ok value: ()`
   - *Restored:* Cleanly reverted.

4. **Storage Exportability Safety (Scope 1, Task 3 / F-12):**
   - *Mutation:* Mutate `Storage::is_exportable` on `StorageBacking::Managed` to unconditionally return `false`.
   - *Result:* Test `c0_2ci_storage_is_exportable_managed_reserves_read_and_refuses_when_written` failed with:
     `assertion failed: managed.is_exportable(Some(&mut service))`
   - *Restored:* Cleanly reverted.

5. **Drawable Layout Transition Safety (Scope 1, Task 3 / F-12):**
   - *Mutation:* Mutate `Drawable::record_layout_transition` on `Managed` to set `vk::ImageLayout::UNDEFINED` instead of `target_layout`.
   - *Result:* Test `c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan` failed with:
     `assertion left == right failed: left: UNDEFINED, right: COLOR_ATTACHMENT_OPTIMAL`
   - *Restored:* Cleanly reverted.

---

## Round-1 Implementation Review Findings Audit

| Finding | Severity | Resolution & Test Evidence | Status |
| --- | --- | --- | --- |
| **B-1** | Blocking | `try_mint_file_family_closed` takes a caller-supplied `discharge_payload_alias` closure, walks `payload_alias_keys`, and discharges all aliases before dropping registry device. Test: `c0_2ci_drm_cleanup_fd_family_barrier_discharges_payload_alias`. | **RESOLVED** |
| **B-2** | Blocking | `ResourceService::adopt` registers `file_owned` payload aliases via `register_payload_alias`; `service_ready_with_registry` discharges before destruction. Tests: `c0_2ci_scanout_adopt_with_registry_registers_file_owned_alias_only`, `c0_2ci_scanout_service_ready_with_registry_discharges_before_destroy`. | **RESOLVED** |
| **B-3** | Blocking | Implemented real DRM hardware barrier test with real GBM payload. Test: `c0_2ci_fd_family_barrier_real_gbm_payload_drm` (passing on real DRM node). Deterministic side: `c0_2ci_handoff_complete_fd_family_barrier_deterministic`. | **RESOLVED** |
| **B-4** | Blocking | `DeviceBarrier` constructors made private (`from_file_family_closed`, `device_lost`). Literal enum variants sealed. Tests: `c0_2ci_handoff_complete_fd_family_barrier_deterministic`, `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines`. | **RESOLVED** |
| **B-5** | Blocking | `TeardownRelease` constructors made private to `handoff.rs` module and sealed. Supervisor unsealed minting removed. Test: `c0_2ci_handoff_unresolved_kms_rejects_teardown_release`. | **RESOLVED** |
| **B-6** | Blocking | `RecipientReservation::new_for_tests` gated under `#[cfg(test)]`. No production constructor. R8 enforced. | **RESOLVED** |
| **B-7** | Blocking | `HandoffRouter::transfer` calls `revoke_owner_writes` before closing gate; revoked grants marked `Quarantined`. Test: `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines`. | **RESOLVED** |
| **B-8** | Blocking | `Terminal` event cause handling fixed in `commit.rs`: only `CompletionUnknown` freezes; `Completed` discharges and releases normally. Test: `c0_2ci_commit_hardware_complete_discharges_old_only`. | **RESOLVED** |
| **B-9** | Blocking | Vacuous `apply_validated_proof` calls removed from test body; old vs new set distinguishability verified. Test: `c0_2ci_commit_hardware_complete_discharges_old_only`. | **RESOLVED** |
| **B-10** | Blocking | Transport gate enforced beneath all 7 real DRM sinks. Tests: `c0_2ci_sink_gamma_gate_four_states_drm` plus 6 deterministic sink tests. | **RESOLVED** |
| **B-11** | Blocking | Serviced-time deadline implemented per-batch in `CoreRetirementBatch`; VT-away / DPMS-off pauses progress. Tests: `c0_2ci_serviced_deadline_is_per_batch_not_global`, `c0_2ci_serviced_time_pauses_during_seat_inactive_and_expires`. | **RESOLVED** |
| **B-12** | Blocking | `ScanoutAllocation` single GEM closer verified on `GemOwner::Right` and `GemOwner::Gbm` across right discharge, payload drop, and retry. Test: `c0_2ci_scanout_file_owned_discharge_right_exactly_one_close_gem`. | **RESOLVED** |
| **B-13** | Blocking | Physical fields moved into `ScanoutAllocation` and `CopiedSourceAllocation`. `acquire_managed_scanout_bo` manages `BoPhase`. Test: `c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan`. | **RESOLVED** |
| **B-14** | Blocking | `Storage::into_managed` takes `PlatformBackend`, refuses non-stub allocations without `vk`, and backfills contexts. Tests: `c0_2ci_storage_into_managed_refuses_non_stub_without_vk_context`, `c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan`. | **RESOLVED** |
| **B-15** | Blocking | Replaced Spy simulator with real DRM/Vulkan readback adapters. Test: `c0_2ci_read_source_scratch_regression_vulkan`. | **RESOLVED** |
| **B-16** | Blocking | Live Vulkan smoke runs with `VK_LAYER_KHRONOS_validation` active on real hardware GPU ICDs. False lavapipe claims removed. Tests: `c0_2ci_live_lifetime_adapters_vulkan`, `c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan`. | **RESOLVED** |
| **M-1** | Major | `on_available` uses staged rollback: popped resources restored on failure without dropping. Test: `c0_2ci_capacity_on_available_error_restores_all_resources_safely`. | **RESOLVED** |
| **M-2** | Major | Un-keyed second discharge path in `HardwareComplete` removed; obligations matched strictly by `GroupMember`. Test: `c0_2ci_commit_hardware_complete_discharges_old_only`. | **RESOLVED** |
| **M-3** | Major | Dual bookkeeping in `commit.rs` eliminated; `in_flight` and `CommitResources::kms_obligations` reconciled. | **RESOLVED** |
| **M-4** | Major | `?` inside per-obligation discharge loop eliminated; all obligations validated atomically before applying proofs. | **RESOLVED** |
| **M-5** | Major | Displaced-pair producer computes `(allocation, member)` with `new != old`, registering obligations before IPC. Consumer provides `take_current()`. | **RESOLVED** |
| **M-6** | Major | Present dispositions wired in `backend.rs` and `present_completion.rs`. `PresentRelease` consumed. Tests: `c0_2ci_present_release_consumption_and_completion_suppression`, `c0_2ci_cow_deferred_release_and_reclaim_with_physical_contracts`. | **RESOLVED** |
| **M-7** | Major | Direct role transitions performed by consumer; `finish_role` enforces `Occupied` state and rejects `Reserved`. Tests: `c0_2ci_capacity_finish_role_rejects_merely_reserved_token`, `c0_2ci_backend_managed_prepare_direct_candidate_implicit_layout_rejection`, `c0_2ci_backend_managed_unflip_and_reentry_contracts`. | **RESOLVED** |
| **M-8** | Major | Silently skipping non-pending obligations in `commit.rs` removed; test double-registrations eliminated. | **RESOLVED** |
| **M-9** | Major | Discarding errors with `let _` in `handoff.rs` replaced with explicit error propagation and route closure. | **RESOLVED** |
| **M-10** | Major | `apply_teardown_release` verifies `file_owned == None`, preventing ioctl-after-barrier. Test: `c0_2ci_scanout_apply_teardown_release_refuses_live_file_owned`. | **RESOLVED** |
| **M-11** | Major | Late returned descriptors tracked under incident and block `try_mint_file_family_closed`. | **RESOLVED** |
| **M-12** | Major | Engine/store detach preserves cleanup ownership; `shutdown_destroy_drawables` wired. | **RESOLVED** |
| **M-13** | Major | `begin_quiescing` queries live `DirectOwnershipState` instead of free-floating booleans. Test: `c0_2ci_transport_gate_direct_scanout_precondition`. | **RESOLVED** |
| **M-14** | Major | `TransportGate::close()` refuses while owner write grants are outstanding. Test: `c0_2ci_transport_gate_close_refuses_outstanding_grants`. | **RESOLVED** |
| **M-15** | Major | `Quarantined` in `commit.rs` isolates failed commits without freezing unrelated commits' resources. | **RESOLVED** |
| **M-16** | Major | Progress test built on core-loop fake backend; deadline pauses budget rather than execution polling. Test: `c0_2ci_progress_no_composition_on_core_loop_fake_backend`. | **RESOLVED** |
| **M-17** | Major | `apply_validated_proof` restricted to `pub(in crate::kms::render::resources)` with test-only shims. | **RESOLVED** |
| **M-18** | Major | Lease abstraction sealed: `AllocationLease.entry`, `payload`, `new` restricted to `resources`. Store access routes through `with_storage_read`/`with_storage_write`. Tests: `c0_2ci_storage_is_exportable_managed_reserves_read_and_refuses_when_written`, `c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan`. | **RESOLVED** |
| **M-19** | Major | Panicking `Deref`/`DerefMut` eliminated; all raw `.storage.` accessors across `backend.rs`, `engine.rs`, `scene.rs`, `frame_builder.rs`, `target.rs` converted to safe methods. Total unmigrated sites: 0. | **RESOLVED** |
| **M-20** | Major | Retirement seams converted: `Storage::destroy` transitions to `Detached`; `RetiredPromotionPayload::Managed` handles deferred retirement under fence tickets. Tests: `c0_2ci_storage_managed_destroy_transitions_to_detached`, `c0_2ci_engine_promote_drawable_exportable_managed_vulkan`. | **RESOLVED** |
| **M-21** | Major | DRI3 and pool return regressions upgraded to real dma-buf round-trips with distinguished sizes and `PixmapPoolStats`. Tests: `c0_2ci_storage_dri3_lease_regressions_vulkan`, `c0_2ci_storage_no_premature_pool_return_vulkan`. | **RESOLVED** |
| **M-22** | Major | `FileOwnedBacking::discharge` drops `gbm_bo` before `device` alias. `CopiedSourceAllocation::Drop` destroys transfer resources. | **RESOLVED** |
| **M-23** | Major | Test-only constructors placed strictly under `#[cfg(test)]`. No `Option<Arc<VkContext>>` in production types (except test-stub constructors, F5 amendment). Status fallback prohibited. | **RESOLVED** |
| **M-24** | Major | Deliverables complete, fold-backs carry full narrative and evidence tables, caller inventories provided, `docs/status.md` accurate. | **RESOLVED** |

---

## Readiness Boundary for Stage 2c-ii

Stage 2c-i is completely finished. The boundary for the next sub-stage (Stage 2c-ii: Multi-Output Dynamic Modesetting & Grouped Presentation) is established:

### What Stage 2c-ii can rely on:
1. **Authoritative Resource Availability:** Single availability state machine in `ResourceService` tracking `Retain`, `Read`, `Write`, and `Kms` usage with generational tracking.
2. **Physical Role Tracking (`DirectCapacity`):** Atomic transitions between the six physical roles (`DirectScanoutA`, `DirectScanoutB`, `Preparing`, `Successor`, `OrdinaryRetirement`, `ExitRetirement`) with strict serial matching and vacant-role checks.
3. **Safe Storage & Scanout Allocations:** `StorageBacking` with constant-time view/format accessors and lease-mediated read/write reservations; `ScanoutAllocation` with independent `file_owned` and `shared` dispositions.
4. **Transport Gate Enforcement:** Centralized writer permission checks beneath all 7 DRM write sinks, ensuring quiescing and closure cleanly refuse writes.
5. **Keyed Commit Resource Consumption:** Displaced-pair producer and atomic obligation discharge on `HardwareComplete`, keyed by `GroupMember` (CRTC key + generation + epoch).
6. **Reachable FD-Family Teardown Barriers:** Private-constructed `FileFamilyClosed` requiring helper reap, control IPC close, and caller-discharged payload aliases.

### What Stage 2c-ii cannot rely on:
1. **Production Owner Activation (R8):** Operational readiness remains closed; production paths remain strictly `Legacy`; no production `OwnerWriteGrant` issuer exists.
2. **Automated Teardown Supervisor:** The retaining supervisor is a test fixture; Stage 3 will provide the production supervisor.
3. **Producer Conversion & Incremental Damage:** Retained for Stage 2c-iii.
4. **Production Helper Mutation:** Stages 3 and 4 will supply production helper mutation execution paths.

---

## Conclusion

Phase C.0 Stage 2c-i is **ACCEPTED**. All code, tests, plan fold-backs, and findings
are coherent, passing all gates, and verified against real hardware and compilers.
The repository is ready to proceed to Stage 2c-ii specification and implementation.
