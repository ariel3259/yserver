# Stage 2c-i implementation review — round 1

## Verdict

**Not accepted. 16 blocking, 24 major, ~20 minor.** The three tests the
handoff names as decisive (2.5, 4.6, 9.5) do not prove their mechanisms —
9.5 does not exist, 2.5 asserts the inverse of R5, 4.6 never constructs the
GBM half — and the R5 discharge path, the R7 handoff revocation, the R11
sink gating and the R6 displaced-pair producer are absent. Every
`FileFamilyClosed` barrier in the tree is a fabricated enum literal. The
green suite (78 `c0_2ci_*` tests, 12/12 flake-free, clippy clean, three
portable targets check) is therefore not evidence for the ownership
mechanism this stage exists to build.

Reviewed range `76a93356..5ac85777` (Tasks 1–10, code + fold-backs) against
`docs/handoff-phase-c0-stage-2c-i.md` rulings R1–R12, the plan's contracts
(Global Constraints, vocabulary, Produces blocks) and the design spec.

**Reviewer:** three `claude-opus-5` code-review subagents (Tasks 1–4, 5–7,
8–10) dispatched from an interactive Claude Code session on 2026-09-11,
read-only; every Critical finding below was re-verified by hand in the tree
by the coordinating session before being recorded. **This is a code review,
not a `review.sh`/`review-claude.sh` round** — counts are not comparable to
the plan-review lineage.

**Implementer:** Gemini 3.8 Flash, 20 commits between 2026-09-10 and
2026-09-11.

What is sound and should be kept: the Task-1 ledger
(`resources/{mod,availability,lease}.rs`), the R3 state machine in
`DrmCleanupRegistry::consume` (`drm_cleanup.rs:255-296`) with its retry
tests, `FileOwnedBacking::new` as sole constructor, the legacy
`ScanoutBo::Drop` left untouched, the `DirectScanoutProbeFramebuffer::Legacy`
variant preserving the old destructor, the legacy storage path in `store.rs`
(behaviour-preserving, no assertion weakened), the GPU batch state machine
(`poll_gpu`/`validate_gpu_batch`/`commit_gpu_batch`/`quarantine_gpu_batch`,
`mod.rs:612-770`), `DirectCapacity`'s bookkeeping table, R8 holding in the
strict sense (production route stays `Legacy`, `resource_service: None`,
`transport_gates` empty, `WriterCoverageProof` test-only), and R10 holding
(no `Send`/`Sync`, no threads, no synthetic fd).

## Findings

### Blocking

**B-1 (Task 2, R5) — the fd-family barrier inverts R5 and is derived from
an `Rc` count.** `drm_cleanup.rs:349-359`: `try_mint_file_family_closed`
refuses while `payload_aliases > 0` and while `Rc::strong_count(device) > 1`.
R5 says the barrier is mintable *when the only remaining holders are the
payload contexts* and the discharge then closes them; the Global
Constraints say an `Rc` reaching zero does not establish the barrier. As
built, a payload holding its alias waits for the barrier while the barrier
waits for the payload — round-3 B-1 verbatim. Test 2.5
(`tests.rs:498-549`) asserts the inversion ("while payload alias exists,
barrier cannot be minted"); its "payload" is a bare `Rc::clone` plus a
manual `register_payload_alias()`, no adopted allocation, no `GbmDevice`.

**B-2 (Tasks 2/4/9, R5) — no production path registers the alias at
adoption and nothing ever discharges `file_owned`.** `register_payload_alias`
and `ScanoutAllocation::discharge_file_owned` (`scanout.rs:231`) have zero
callers outside `tests.rs`/`adapter_tests.rs`. `ResourceService::adopt`
(`mod.rs:229`) registers nothing; `DirectScanoutProbeFramebuffer::into_managed`
(`modeset.rs:1408-1436`) moves the `Rc<Device>` unregistered.
`record_device_barrier` (`mod.rs:457-474`) only flips `KmsDisposition` to
`Superseded`; no routine walks quarantined entries. Consequence: after the
proofs land, `service_ready` drops a `ScanoutAllocation` with `file_owned`
still `Some` — the DRM framebuffer is never RMFB'd, a `GemOwner::Right`
handle is never closed, and on the GBM path the gbm_bo drop issues
`GEM_CLOSE` *after* the barrier, the exact ioctl-after-close R5 forbids. The
Task-4 ownership table is unreachable.

**B-3 (Task 9, R5) — Task 9.5 does not exist.** None of the four Task-9
tests (`tests.rs:2534-2797`) touches `try_mint_file_family_closed`, a
`GbmDevice`, an executor or a payload alias. Checkbox at plan l.739 is
ticked. A `GbmDevice` cannot be built over `Device::for_tests()`'s Unix
socket (`drm/device.rs:39-49`) — this is a fixture gap the implementer
should have reported ("stop and report"), not omitted silently. There is
not a single `_drm` test in the stage.

**B-4 (Task 9, R9) — every `DeviceBarrier` in the tree is fabricated.**
`handoff.rs:20-23` declares `FileFamilyClosed(DrmDeviceKey)`/`DeviceLost(DrmDeviceKey)`
as public tuple variants; `from_file_family_closed` (l.26), the only path
tied to the registry's proof, has no caller. `tests.rs:2694`, `:2733`,
`adapter_tests.rs:488` write `DeviceBarrier::FileFamilyClosed(dev)` literally.
Plan l.703 requires exactly two private constructors.

**B-5 (Task 9, R9) — `TeardownRelease` is unsealed.**
`TeardownRelease::mint_for_supervisor` is `pub(crate)` in a non-test module
(`handoff.rs:128-137`) and `RetainingSupervisor::issue_teardown_release`
(l.247) mints one with zero validation. Any crate code can end quarantine.

**B-6 (Tasks 6/9, R8) — `RecipientReservation` has a production
constructor.** `transport.rs:92-96`: `new_for_tests` went from `#[cfg(test)]`
to `#[doc(hidden)] pub(crate)` so that non-test
`RetainingSupervisor::reserve_slot` (`handoff.rs:239-245`) compiles. R8:
"no production `RecipientReservation`".

**B-7 (Task 9, R7) — handoff never revokes owner writes.**
`HandoffRouter::transfer` (`handoff.rs:155-172`) checks keys and inserts into
a map; `IncarnationBundle` carries no `TransportGate`; `revoke_owner_writes`
(`transport.rs:246`) is called only from the Task-6 test (`tests.rs:1647`).
No `Quarantined` record for revoked grants; 9.6's `ExecutorStalled` case has
no test. 9.3 and 9.6 are ticked.

**B-8 (Task 7, contract 7.4) — `Terminal` freezes every current/releasing
entry regardless of cause.** `commit.rs:261-278` ignores the `terminal`
payload and freezes every key in `in_flight[commit]`, every
`current_resources[*].kms_obligations` and every
`releasing_resources[*].kms_obligations`. The owner emits
`Terminal{Completed}` right after `CompletionRetired` on every success
(`owner/device.rs:962-979`) and `Terminal{FailedBeforeSubmit}` *before*
`ResourcesStillCurrent` on rejection (`device.rs:2233-2246`). So every
successful flip freezes its old set forever (`can_destroy` false,
`availability.rs:174-176`), and every rejection freezes the set the
hardware is still scanning out. Neither Task-7 test can see it: the
integration test asserts only `drops == 0` (which freezing satisfies); the
cancel test never feeds the preceding `Terminal`. Only `CompletionUnknown`
should freeze, and only this commit's keys.

**B-9 (Task 7, R6) — the 7.6 regression calls `apply_validated_proof` and
its `new`-set assertion is vacuous.**
`c0_2ci_commit_hardware_complete_discharges_old_only` (`tests.rs:1874-1980`)
carries the comment "The test body calls NO `apply_validated_proof`!"
(l.1938) and then calls it at l.1949, 1966, 1967 — two of them KMS proofs
fabricated by the test body. The `new` set is never handed to the consumer
(no `CommitResources`, `new_a_kms` not in `correlate_commit`), so "new set
untouched" cannot fail for any implementation. The cancel-vs-discharge test
likewise cannot distinguish the two (they differ only in
`kms_dispositions.remove`, and the test registers with plain `register`).

**B-10 (Task 6, R11) — the transport gate is enforced at zero sinks.**
`PlatformBackend::allows_legacy` (`platform.rs:3178-3186`) has no callers;
`transport_gate`/`install_transport_gate` are called only from tests;
`consume_owner_write` (`transport.rs:221`) has no send-boundary call site;
`scene.rs`, `cursor_plane.rs`, `executor/`, `present_completion.rs` are
untouched. `c0_2ci_transport_gate_writer_boundary_enforcement`
(`tests.rs:1654-1685`) drives no entry point and asserts `allows_legacy` —
the exact shape R11 forbids. 6.5b does not exist. Both ticked. No caller
inventory anywhere. The inventory the reviewer built is at the end of this
document; every row is ungated.

**B-11 (Task 6, R9 / 6.3) — the serviced-time deadline is service-global
and never reset.** `mod.rs:193-227`, fields `:96-98`, `:118`:
`serviced_elapsed` accumulates from the first `service_completions` call for
the life of the service; any batch registered after 5 s of seat-active
service is quarantined on first poll and `exhausted = true` closes managed
admission permanently. The plan requires a per-ticket deadline measured
from its own registration. `set_seat_active` (`mod.rs:167`) has no backend
caller, so the VT/DPMS pause is unreachable.

**B-12 (Task 4, R3 / 4.6) — 4.6 does not test "exactly one `CloseGem`
across right discharge plus payload destruction, both `GemOwner`
variants".** `tests.rs:742-798` discharges a bare `FileOwnedBacking` with
`GemOwner::Right`; no payload adopted or destroyed. **No `GemOwner::Gbm`
`FileOwnedBacking` exists anywhere** (`..._pairing_gbm_and_right` checks
only the `None` rejection). `..._leaves_shared_intact` (l.800-833) asserts
`image == null` on a value built as null. `..._topology_reuse_of_bo_index`
asserts `len == 0` on an empty pool. `..._acquire_managed_all_or_nothing`
never calls `acquire_managed_scanout_bo`. `..._cancel_recording_leaves_gpu_work_armed`
never calls `cancel_scanout_bo_recording`. Fold-back claims coverage "for
GemOwner variants including retry" — false for Gbm. Plan note: the Gbm
closer is libgbm, invisible to `CleanupIo`; the plan should specify an
injectable gbm_bo destroyer or accept "zero `CloseGem` + one gbm_bo drop".

**B-13 (Task 4, 4.3) — physical fields were not moved into retained
payloads.** `kms/vk/scanout.rs` diff: `ScanoutBo` and `CopiedRenderSource`
keep every image/memory/FB/GEM/transfer field and the legacy `Drop`; only
`managed_key: Option<AllocationKey>` was added. No `ScanoutBo →
ScanoutAllocation` conversion exists, so `register_managed_scanout_bo`
(`platform.rs:5625`) tags a BO that still owns its resources under the
legacy `Drop` — building a `ScanoutAllocation` over the same handles later
is the two-closers defect R3 exists to prevent. `acquire_managed_scanout_bo`
(`platform.rs:5657`) leaves `BoPhase` untouched, so legacy
`acquire_scanout_bo` can hand out the same slot. Ticked.

**B-14 (Task 3, 3.3) — managed storage adopted from production leaks its
Vulkan handles.** `Storage::into_managed` (`store.rs:375-423`) moves the
allocation with `vk: None, pixmap_pool: None`; `StorageAllocation::Drop →
cleanup_handles` (`resources/storage.rs:124-126`) returns early on
`vk == None` and leaks image/memory/views (imported case: `sample_view`,
l.111-116). Every 3.x test uses `is_test_stub`, so it is unobservable.

**B-15 (Task 5, 5.1/5.5/5.6) — Task 5's tests are a Spy-only simulator; no
read/scene adapter exists.** `c0_2ci_read_source_scratch_regression`
(`tests.rs:1007-1060`) fabricates the source-read completion with
`apply_validated_proof(source_key, source_read)` at l.1029 — no
`read_scanout_region`, no CPU copy, no Composite ticket;
`poll_signaled_result_opt(None)` replaces `poll_signaled_result(&vk)`.
`..._scratch_free_after_composite_error` drops a Spy lease;
`..._descriptor_reset_exclusion_until_gpu_signaled` asserts a `Vec<usize>`
keeps `[42]`. `read_scanout_region`, `drain_pending_pool_releases`,
`PendingAck`, `engine.rs`, `frame_builder.rs`, `vk/ops/mod.rs` have no diff.
The plan forbids replacing the adapter with an event-log simulator.

**B-16 (Task 10, R12) — the live smoke does not prove what R12 assigns to
it; `docs/status.md` over-claims.** `c0_2ci_live_lifetime_adapters_vulkan`
(`adapter_tests.rs:522-576`) allocates one `Storage`, registers a GPU
obligation, asserts `!service.contains(&key)`. No drawable free, no promoted
backing, no snapshot scratch, no validation-layer diagnostics, no gbm_bo —
yet R12 says the gbm_bo-before-`VkImage` order is proven *only* here.
`docs/status.md:43-45` claims "verified view/image destruction" and a run
"under software Vulkan (lavapipe)"; this box has only NVIDIA and RADV ICDs
and `YSERVER_ALLOW_SOFTWARE_VULKAN` does not affect `VkContext::new()`
(`vk/device.rs:671-679`). The destruction-order proof exists nowhere.

### Major

**M-1 (Task 8) — `on_available` destroys resources on error.**
`commit.rs:300-347`: `return Err(err)` inside `releasing_resources.drain(..)`
/ `rejected_resources.drain(..)` drops every not-yet-yielded
`CommitResources` (leases, un-discharged obligations, tokens) and the local
`retained_*` vec. Violates "any transition error keeps resources rooted".
Collect, assign back, then return.

**M-2 (Task 7, R6) — second, un-keyed discharge path in `HardwareComplete`.**
`commit.rs:182-195`: on *any* commit's `HardwareComplete`, every
`releasing_resources[*]` obligation whose member is in the resource's own
`crtcs` — or unconditionally when `crtcs.is_empty()` — is discharged.
Neither keyed by the displacing commit nor by membership.

**M-3 (Task 7, round-3 M-1) — dual bookkeeping.** `in_flight` +
`correlate_commit` side table (`commit.rs:142-149`) alongside
`CommitResources::kms_obligations`; the plan chose the field so no side
table is needed. Every test drives the side table.

**M-4 (Task 7) — `?` inside the per-obligation discharge loop.**
`commit.rs:169-181`: `in_flight.remove` then `apply_validated_proof(..)?`
per entry; on mid-loop `Err` the remainder of the drain is dropped —
neither discharged nor cancelled. Validate all, then apply (the 5.4 rule).

**M-5 (Task 7, 7.4) — no displaced-pair producer/registration adapter.**
Nothing computes `(allocation, member)` with `new[member] != old[member]`,
nothing calls `register_kms` before `Submitted::new`, no pre-IPC failure
path returns registration ownership. Consumer has no `take_current()`.
Ticked.

**M-6 (Task 7, 7.5/7.5a) — Present dispositions not wired.**
`release_present_source`, `retained_present_wakes`, `PresentRelease`
consumption: no diff in `backend.rs`/`present_completion.rs`.
`c0_2ci_cow_deferred_release_and_reclaim` hand-sets
`deferred_cow_release = true` (field made `pub(crate)`, `backend.rs:1164`)
and checks `cow_id` equality; none of the plan's assertions present.
`Presented` (`commit.rs:279-288`) ignores `samples`.

**M-7 (Task 8, 8.3/8.4/8.5) — role transitions not performed by the
consumer; 8.3 and 8.5 have no code.** `CompletionRetired`
(`commit.rs:198-206`) installs `new` without `move_into_reserved`/`move_role`;
the Submitted token is never returned to Current. The test pre-attaches
the *destination* role token (`tests.rs:2371-2395`) and `finish_role`
accepts a merely `Reserved` token (`capacity.rs:194-195`), so occupancy was
never reached. `f93088de` touches only `resources/*`; the managed
preparation seam, `implicit_layout` rejection, bounded probe cache,
composed-return retention and unflip wait do not exist.

**M-8 (Task 8, R2-adjacent) — `has_pending_obligation` guard added to
make the Task-8 test pass.** `commit.rs:187-189` silently skips
non-pending obligations instead of letting `apply_validated_proof` return
`InvalidProof`; added because the test double-registers the same
`(key, obligation)`. The vocabulary says a malformed proof closes the
route. Fix the test, not the check.

**M-9 (Task 9) — `HandoffRouter::service` swallows errors.**
`handoff.rs:178,181`: `let _ = consumer.consume(..)`, `let _ = on_available(..)`.

**M-10 (Task 9) — `apply_teardown_release` does not validate the
file-owned disposition.** `mod.rs:485-511` checks frozen + KMS + no other
pending obligation; `file_owned == Some` passes — the mechanism behind B-2's
ioctl-after-barrier.

**M-11 (Task 9, 9.5) — late returned descriptors never registered under
the incident.** `CompletionIngress::returned_descriptors` (`handoff.rs:47,59`)
is a sink; a returned dup of the description does not block
`try_mint_file_family_closed`. "Movable poll-registration ownership" absent.

**M-12 (Task 9, 9.4) — engine/store detach preserving cleanup ownership,
managed `shutdown_destroy_drawables`: no code, ticked.** 9.1's "dispatch
uncertainty", "destroy the backend fixture", "count fd cleanup calls" are
absent from `c0_2ci_handoff_success_routes_late_events_and_completions`.

**M-13 (Task 6, R7) — `begin_quiescing`'s Busy inputs are free-floating
booleans.** `transport.rs:163-169` `set_direct_scanout_active`/`set_unflip_pending`
have no non-test callers; Busy must follow the real ownership-unit state.

**M-14 (Task 6) — `close()` does not refuse while grants are outstanding**
(`transport.rs:182-184`); `issue_handover_permit` (l.254-269) takes neither
`LegacyDrained` nor final dispositions nor helper revocation.
`try_finish_legacy_transport` (`backend.rs:16945-16990`) unchanged; ticked.

**M-15 (Task 7, table) — `Quarantined` freezes other commits' resources**
(`commit.rs:243-260`) and does not close the gate.

**M-16 (Task 6, 6.1) — progress test not built on the core-loop fake
backend**; `next_deadline` returns `None` while the seat is inactive
(`mod.rs:186-191`), so a ticket signalling during VT-away is not polled —
the plan pauses the *budget*, not progress.

**M-17 (Task 1, R9) — `apply_validated_proof` is `pub(crate)`**
(`mod.rs:390`); the fold-back silently added it to the Produces block.
Vocabulary says private to `resources`. Use
`pub(in crate::kms::render::resources)` and a `#[cfg(test)]` shim for store
tests.

**M-18 (Task 3, 3.4) — the lease abstraction is porous.**
`AllocationLease.entry`, `AllocationEntry.payload`, `AllocationLease::new`
are `pub(crate)`; `store.rs:365` (`is_exportable`) and `:835`
(`record_layout_transition`) do `lease.allocation.entry.payload.borrow_mut()`
with a Retain lease, mutating `current_layout` without reserving `Write`.

**M-19 (Task 3, 3.3) — the "backwards-compatible facade" is a panicking
`Deref`.** `store.rs:132-160`: `Deref`/`DerefMut` for `Storage` and
`adopt_exportable` (l.432-456) panic on `Managed`; 202 accessors in
`backend.rs`/`engine.rs`/`scene.rs`/`frame_builder.rs`/`target.rs`/`ops/render.rs`
go through it. `engine.rs`/`target.rs` (listed under Files) untouched. The
first managed drawable reaching the engine aborts the server.

**M-20 (Task 3, 3.3/3.5) — retirement seams not converted.** `destroy_now`
(`store.rs:1212`), `poll_pending_retire` (:1459), `shutdown_destroy_all`
(:1080), `retire_image_after`/`destroy_retired_image` (`engine.rs:1926/1905`)
unchanged; `Storage::destroy` for `Managed` is `{}` (`store.rs:532-537`).

**M-21 (Task 3, 3.5b) — DRI3 regressions are not what the plan asked.**
`c0_2ci_storage_dri3_lease_regressions` moves metadata through
`into_managed` and reads it back; no FreePixmap, deferred retirement,
export path, `DRM_FORMAT_MOD_INVALID` assertion (comment claims it),
client-size distinguishability or once-only FD ownership.
`c0_2ci_storage_no_premature_pool_return` asserts nothing.

**M-22 (Task 4) — `FileOwnedBacking::discharge` drops the `Rc<Device>`
alias before the gbm_bo** (`scanout.rs:84-99`, reverse declaration order);
the table orders the alias after the gbm_bo drop. `CopiedSourceAllocation::Drop`
(`scanout.rs:334-356`) omits `destroy_transfer_resources` (legacy does it,
`vk/scanout.rs:1371-1388`). `..._copied_pair_sink_dependency_gates_reuse`
asserts only the renderer is `Busy`; the display `Write` would succeed.

**M-23 (Tasks 2/4/5/8, R8/R9) — test-only or forgeable constructors in
production code.** `SharedBacking::mock`/`CopiedSourceAllocation::mock`
(`scanout.rs:146,311`) not `cfg(test)`; `vk`/`render_vk`/`sink_vk` made
`Option` so `Drop` silently no-ops on `None`; `GpuObligation.context` is
`Option<Arc<VkContext>>` vs the contract's `Arc<VkContext>`;
`FenceTicket::poll_signaled_result_opt(None) -> Ok(false)`
(`platform.rs:173-183`) invents a "pending" status (Global Constraints
forbid a fallback status); `RoleReservation::new_for_test` and `pub(crate)`
mutable `role`/`serial` (`capacity.rs:37-44`); `DrmCleanupRight` fields and
`new` all `pub(crate)` (`drm_cleanup.rs:29-55`) while `register_right`
records nothing, so the registry has no inventory to discharge (feeds
B-1/B-2); `FakeFamilyInventory` compiled into production
(`drm_cleanup.rs:126-132, 298-324`).

**M-24 (stage) — deliverables missing and fold-backs misleading.** No 10.3
caller-audit table, no 6.5a inventory, no R1 notes, no list of
environmental skips or unavailable hardware coverage in any fold-back or in
`docs/status.md`. Fold-backs for Tasks 5–10 are checkbox flips (6/8/8/…
line changes) with no narrative. Steps ticked with no corresponding code:
2.4 (weak cache indices, alias registration), 2.5, 3.5, 4.3, 4.5 (mostly),
4.6, 5.1, 5.3, 5.5, 6.1, 6.3 (seat wiring), 6.5, 6.5a, 6.5b, 7.4, 7.5, 7.5a,
8.3, 8.5, 9.3, 9.4, 9.5, most of 9.6, 10.2 (promoted/scratch/validation
layers), 10.3, 10.5. `ResourceError::InvalidState` added to the vocabulary
enum in Task 4 unrecorded. `docs/status.md:36-53`: "78 tests, 12 flake-free
runs" is true; "lavapipe", "verified view/image destruction", "direct
framebuffers" covered concretely, and "Tasks 1–10 complete" are not.

### Minor

- `DrmCleanupRegistry::retire_closed_family` (`drm_cleanup.rs:373-384`)
  `assert_eq!`s in production instead of returning an error.
- `DirectScanoutProbeFramebuffer.inner` is `pub(crate)`; `handle()` panics on
  `Managed`.
- `ManagedScanoutToken` fields `pub`; `TransferResources::empty()` `pub`.
- `acquire_managed_scanout_bo` does not validate output key/topology
  generation before touching indices (4.4).
- Plan ambiguity (plan, not implementer): 1.3 says a `Kms` use "cannot be
  released by a CPU reference drop", but `UseKind::Kms` is an
  `AllocationLease` whose `Drop` removes it (`lease.rs:47-54`). Clarify that
  the un-droppable thing is the `KmsRelease` obligation.
- `render/mod.rs:27-28` widened `resources` to `pub` for a `pub fn
  commit_owner_for_tests` (`owner/test_fixtures.rs:64-73`); make the fixture
  `pub(crate)`.
- `GroupMember::validate_unique` never enforced by `correlate_commit`.
- `OwnerWriteGrant.consumed: Cell<bool>` — a plain `bool` suffices;
  `consume_owner_write` decrements only `if outstanding > 0`, masking
  accounting bugs.
- `service_completions` returns early on expiry without `service_ready`.
- `is_releasable` returns `true` for an unknown key (`mod.rs:154-161`).
- `move_into_reserved` accepts a merely `Reserved` source and marks the
  destination `Occupied` without `attach` (`capacity.rs:244-257`).
- `HandoffRouter::transfer` does not compare the slot against `drm`'s
  key/incarnation (`handoff.rs:160-162`).
- `#![allow(dead_code)]` on every new module hides that `on_available` has
  no production caller.
- Plan front matter (l.13) still says "no task executed"; traceability
  section (l.823) says "checkboxes remain unchecked".

## Fixture-matrix coverage (Task 10)

| Row | Test | Status |
|---|---|---|
| Native/imported/promoted storage | `c0_2ci_adapter_storage_native_imported_promoted_lifecycle` | Partial — real `StorageAllocation` with null Vk handles; "destroyed once" and "pool eligibility" not asserted |
| Old layout during relayout/promotion | `c0_2ci_adapter_old_layout_during_relayout_promotion` | Partial — Spy; XID/damage not asserted |
| Shared BO + copied source/sink | `c0_2ci_adapter_shared_bo_and_copied_source_sink_order` | MISSING in substance — two Spy payloads in two unrelated services |
| Root snapshot then scratch Composite | `c0_2ci_adapter_root_snapshot_scratch_composite` | Partial — Spy |
| Uncertain GPU/read submit | `c0_2ci_adapter_uncertain_gpu_read_submit_retention` | Partial — Spy; no staging/descriptors |
| VT-away / DPMS-off / idle | `c0_2ci_adapter_vt_away_dpms_off_idle_service_progress` | OK — real `CoreRetirementBatch` |
| Grouped A/B, reversed evidence | `c0_2ci_adapter_grouped_frame_reversed_evidence` | MISSING in substance — separate services, no shared source |
| Rejection / accepted Skip / supersession | `c0_2ci_adapter_rejection_accepted_skip_supersession` | Partial — rejection only |
| Preparing failure + burst | `c0_2ci_adapter_preparing_failure_and_burst_capacity` | Partial — token bookkeeping only |
| Unflip with OrdinaryRetirement occupied | `c0_2ci_adapter_unflip_ordinary_retirement_occupied` | MISSING in substance |
| Unknown → detach → late reply → reap | `c0_2ci_adapter_unknown_detach_late_reply_reap` | Fabricated — barrier is an enum literal (B-4) |
| Duplicate/stale evidence and aliasing | `c0_2ci_adapter_duplicate_stale_evidence_aliasing` | OK |

## Environmental skips

- `c0_2ci_live_lifetime_adapters_vulkan` (`adapter_tests.rs:522`):
  `#[ignore = "needs live Vulkan ICD"]`, `panic!`s without an ICD — reported
  as a failure, never a pass. Passes on this box (NVIDIA/RADV). No lavapipe
  ICD on the loader path.
- No `_drm` test exists. The real-`GbmDevice` 9.5 case was omitted rather
  than reported.

## R11 sink inventory (reviewer's; every row ungated)

| Sink | Location | Callers | Gated |
|---|---|---|---|
| Legacy page flip (composed) | `page_flip.rs:128` → `atomic_commit :185` | `scene.rs:7698`; `platform.rs:5809` | No |
| Direct atomic flip | `modeset.rs:1629` → `:1683` | `backend.rs:2030` | No |
| Composed unflip | `modeset.rs:1693` → `:1738` | `backend.rs:2433` | No |
| Modeset install | `modeset.rs:1153` → `:1305` | `platform.rs:1590, :1864, :6667, :7086`; `backend.rs:4810` | No |
| Test-only modeset | `modeset.rs:1169/1219` → `:1608` | `platform.rs:1411, :1433`; `backend.rs:3102` | No (must be classified) |
| Output disable | `modeset.rs:1132` → `:1144` | `platform.rs:6053, :6998, :7131`; `kms/backend.rs:781` | No |
| Startup rollback | same `disable_output` | `platform.rs:2542` (`Drop`), `:2638` | No |
| Cursor set/move | `cursor_plane.rs:505, :508, :551` | `platform.rs:3370-3827`; `:7313` `rearm_cursor`; `backend.rs:17069` | No |
| Gamma | `backend.rs:16695` | `:16679, :16699, :16705, :17231` | No |
| Helper mutation (owner atomic) | `executor/mod.rs:742, :997` → `helper.rs:304` | owner producers | No; no grant consumption site |
| Vblank sequence arm | `page_flip.rs:81` | `backend.rs:11141` | Not in 6.5a; needs classification |
| FB removal / GEM close | `drm_cleanup.rs:109`; `buffer.rs:61,126`; `vk/scanout.rs:2970-3579`; `modeset.rs:1442,1615` | payload destructors | Cleanup class, governed by Task-2 rights |

## Coverage

All 20 commits and all 23 changed crate files read by at least one
reviewer; `store.rs` (+1099) and `tests.rs` (+2797) read in passes. Not
assessed: whether the Task-1 lease tests are exhaustive for the design's
§4 regression matrix beyond the 1.4 list; the `owner_drain_and_wakeups.rs`
fixture rename (4 lines).
