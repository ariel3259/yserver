# Stage 2c-iii, plan Cfb — direct framebuffer adoption and retention

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for hardware work) with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters (`c0_conv_cfb_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_ci_`, `c0_conv_cir_`) with `--include-ignored`, never `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time. Stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code, stop and report it (F8); never silently substitute a test shape.

**Revision 3 (2026-09-21)** — incorporates codex round 2 (`../findings/2026-09-21-stage-2c-iii-plan-cfb-review-round2.md`: 1 blocking, 3 major, all verified and accepted; round 1: B-1, M-2, M-3 APPLIED; B-2 and M-1 PARTIAL, closed here).
- **B-1:** decision 11 becomes a two-phase protocol: the service records typed **zero edges** at lease drop; readiness returns zero edges and releasable keys **without destroying**; the backend step removes indices, then authorizes cleanup; it runs **after** `route_owner_event_batch` at the backend sites and after the scene's drain returns, before any managed lookup. F31 through all four sites.
- **M-1:** the role proof is an opaque **`DirectLeasePermit`** bound to the issuing capacity, device/incarnation, role state and service; foreign, stale and wrong-role permits are rejected (F9a–F9c).
- **M-2:** the post-conversion adoption failure is forced by seeding the service's generation counter at its last value, so preflight passes and `adopt` fails.
- **M-3:** no compliant producer existed — the Cii helpers insert fake M1 entries because the probe's `TEST_ONLY` needs DRM master. Decision 13: the probe is split into the real import (PRIME + `ADDFB2`, master-free) and the validation; Task 1 delivers a fixture helper that drives the real import on card1 and omits only the validation, which the Ciii hardware run exercises.

**Revision 2 (2026-09-21)** — incorporates codex round 1 (`../findings/2026-09-21-stage-2c-iii-plan-cfb-review-round1.md`: 2 blocking, 3 major, all verified against the tree and accepted).
- **B-1:** the task order could not honour the production-producer-only test rule (no `into_managed` producer until Task 3). Reordered: Task 1 installs the registry **and** the adoption producer; Task 2 the pending-cleanup owner; Task 3 the serial, the index and the service step; every task now has its own gate.
- **B-2:** nothing delivered a last-lease drop to index removal — both completion sites discard `service_completions`' keys (`backend.rs:21704`, `:21715`). Decision 11: one authoritative backend service step.
- **M-1:** framebuffer allocations are leased only through a role-proof entry; generic `reserve` refuses them; F9 is that refusal.
- **M-2:** F4 split: the store test kills a skipped bump; the fresh-key test kills an omitted serial (F12a) and an omitted topology generation (F12b).
- **M-3:** the serial is a checked increment; exhaustion permanently disables Owner direct adoption for that drawable, never wraps.

**Goal:** Close plan Ciii's Task 6 F8. On the Owner route a direct commit carries its scanned framebuffer as a managed `DirectFramebufferAllocation` lease in `CommitResources::allocations`, a valid same-source successor carries the same `AllocationKey`, the registry that destroys FB/GEM imports lives beside the resource service, and every rollback and terminal path has exactly one owner. With it, Ciii Task 6 resumes as written.

**Architecture:** Five tasks, each a coherent unit with its own gate: the registry's production home **and** adoption at preparation with the transaction of spec §2.6 (`backend.rs`, `direct_owner.rs`, `drm/modeset.rs`); the pending-cleanup owner (`resources/drm_cleanup.rs`); the backing serial, the live-checked index and the authoritative service step that removes it (`store.rs`, `backend.rs`'s M1 cache, `resources/mod.rs`); the handoff into the ledger and every ownership row (`direct_owner.rs`, `resources/commit.rs`, the unflip seam); the evidence. Owner-route code goes in its own functions, reached from the existing fork points (the Cii/Ciii constraint); nothing is interleaved into a Legacy function body. Production is unchanged (C0-R8): no registry or service is installed there.

**Spec:** `docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md` at revision 4 (`ec97fc88`), whole. Read first: its "Why this document exists", §2 (2.1–2.6), §3 (3.1–3.4, the ownership table), §4.2 (every row is an exit criterion here). Then the F8 that motivates it, `../findings/2026-09-21-stage-2c-iii-plan-ciii-task6-f8.md`, and the Ciii plan's Task 6 (`2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md`), which this plan unblocks. Read the Cii commit messages for the direct producer (`git log --grep 'Cii task'` is read-only and allowed).

## Design decisions this plan fixes

Items 1–7 come from the spec (user-approved); items 8–13 are this plan's.

1. **Owner route only** (spec 1.1). Legacy's probe, cache ownership, `submit_direct_frame` and eviction are byte-for-byte unchanged.
2. **The registry lives beside the service** (2.1): installed by the same entry as the service, same incarnation; the Owner fixtures stop building their own.
3. **Identity is the service's `AllocationKey`** behind a live-checked, non-owning token in the M1 cache index, keyed by `(source DrawableId, backing serial, topology generation)` (2.3).
4. **The count lives in the service** (2.3): only direct-frame holders lease a framebuffer allocation, so the service's live-use count on the key is the direct-lease count; index removal on `1 → 0`, on incarnation loss, on source-drawable drop.
5. **Storage is never adopted at preparation** (2.5): the present pin retains the source; `into_managed` takes `Option<AllocationLease>`.
6. **The adoption transaction** (2.6): adoptability check → `into_managed` (payload is sole owner) → service adopt; a service failure goes to registry cleanup and closes admission; a cleanup failure goes to a keyless, alias-counted pending-cleanup entry with retry / freeze-handoff / family-closed dispositions.
7. **Every ownership row of 3.3**, including the four unflip rows and the rejected-unflip `ExitRetirement → Current` restore.
8. **The backing serial** is a `u64` on the store's `Drawable`, advanced by a **checked** increment at every site that replaces the drawable's `storage` (relayout, migration, re-import, `adopt_exportable_managed`); it never wraps — on exhaustion the drawable is marked and Owner direct adoption of it is refused permanently (fail closed, candidate rejected). Task 3's first step is the implementer's enumeration of those sites; the plan does not close the list.
9. **The live-checked token** is a service-issued handle that carries the `AllocationKey` and can be asked to upgrade to a lease only while the allocation is live in the current incarnation; a stale token upgrades to nothing. Its shape is yours; it must not keep the allocation alive.
10. **Framebuffer allocations are leased only with a role proof** (round-1 M-1, round-2 M-1). The service issues a lease on a `DirectFramebuffer` payload only through a dedicated entry that takes an opaque **`DirectLeasePermit`**, minted by the capacity when a role is reserved or occupied and bound to the issuing capacity, the device/incarnation, the role's current state and the service that will verify it; `RoleReservation` alone is not a proof. The service rejects a foreign (other capacity/device), stale (role state moved on) or wrong-role permit; the generic `reserve(key, UseKind)` refuses a `DirectFramebuffer` payload. So the service's live-use count on such a key is the direct-lease count by construction.
11. **One authoritative service step, two phases** (round-1 B-2, round-2 B-1). *Phase A, in the service:* when a lease drop takes a key's live-use count to zero, the service records a typed **zero edge** for that key (distinct from the dirty queue, never lost if a new lease is minted afterwards — the edge stays recorded). `service_completions` and readiness **return** the zero edges and the releasable keys; readiness no longer destroys payloads on its own. *Phase B, in the backend:* exactly one function drives it — called **after** `route_owner_event_batch` at the two backend sites (`backend.rs:21704`, `:21715`, which today run before it), and by the backend right after the scene's drain returns (the scene keeps calling `service_completions` with its `&mut ResourceService`, `scene.rs:3915`/`:3965`, but destroys nothing) — which: (a) takes the zero edges and removes every M1 index token for them; (b) then authorizes the registry to clean up the releasable keys. It runs before any managed lookup in the same tick and before an incarnation's teardown handoff. F31 is applied at each of the four sites independently.
12. **Test names start with `c0_conv_cfb_`**; Vulkan tests end in `_vulkan` with `#[ignore = "needs live Vulkan ICD"]`, on the Owner live fixture (`for_tests_with_vk_live_scene_real_drm`) with a managed pool. Deterministic tests use the stub-executor fixtures where no Vulkan is needed.
13. **The probe is split; the fixture omits only the validation** (round-2 M-3). `probe_direct_scanout_test_only` (`drm/modeset.rs`) does two things: the real import (PRIME fd → GEM, `ADDFB2`) and the `TEST_ONLY` atomic commit. The validation needs DRM master, which no Vulkan fixture holds, which is why the Cii helpers insert fake accepted entries (`c0_conv_cii_try_candidate`, `managed_prepare_ready_candidate`). Task 1 splits the probe into an import function and a validation function (a pure refactor; both routes keep calling the pair) and delivers a fixture helper, `owner_direct_candidate_with_real_import`, that on the Owner live fixture drives the real import on the real node (a Vulkan-exported dma-buf of a real pixmap), inserts the accepted entry **through the production cache insert** with the real `DirectScanoutProbeFramebuffer`, and omits only the validation. Every later Vulkan test of this plan uses that helper; the fake-entry helpers are not used by any `c0_conv_cfb_` test. The full probe, validation included, is exercised only by Ciii Task 6's hardware run.

## Limits stated

- Fixture level; no production caller (C0-R8). The hardware run stays Ciii Task 6's.
- The managed-storage access adaptation (raw-storage consumers) is an open item recorded in spec 2.5; this plan does not depend on it and must not adopt storage.
- Stage 3's recovery of quarantined allocations; the copied route.

## Global Constraints

- **Production is byte-for-byte unchanged**: without an active conductor nothing here runs. Each task keeps a named Legacy characterisation test green (`c0_conv_cfb_legacy_probe_cache_unchanged`).
- Owner milestones reach consumers only through `route_owner_event_batch`; tests deliver them that way (a stub behaviour or crafted events handed to it — say which).
- No hand-built `CommitResources`, `DirectFramebufferAllocation`, `AllocationLease`, `DirectPresentFrame`, `PendingAck`, `BoPhase` or M1 cache entry in any test: everything comes from the production producer (`managed_prepare_direct_candidate` and the direct offer/dispatch path). A test that cannot reach its state through production entries is an F8, not a fixture.
- Resources travel by value; nothing is bare-dropped; every lease is released exactly once; no retry on refusal.
- No side effect inside `debug_assert!`; fail closed, never panic, in non-test code; no test-only hook that bypasses the path it is named after.
- Owner-route code lives in its own functions/modules reached from one fork point; no `owner_route`-style branch appears inside a Legacy function body (the Cii/Ciii constraint; the coordinator greps for it).
- **Honesty rule (F8).** An unreachable scenario, a seam that does not behave as stated, a fixture that cannot carry what a test needs, or a real design choice left open: stop and report.

## Checks every task must keep green

```bash
cargo build -p yserver --bin yserver
cargo build --release -p yserver --bin yserver
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features tcp-transport -- -D warnings
cargo clippy --all-targets --features xdmcp -- -D warnings
for i in 1 2 3 4 5; do cargo test -p yserver --lib c0_conv_cfb_; done
cargo test -p yserver --lib c0_conv_cfb_ -- --include-ignored --test-threads=1
cargo test --release -p yserver --lib c0_conv_cfb_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_adm
cargo test -p yserver --lib c0_2ci
cargo test -p yserver --lib
```

The last task also runs `cargo check --workspace --target <t>` for `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-unknown-freebsd`.

Baseline before Task 1 (commit `ec97fc88`, measured at `aaac1a8f`): `c0_conv_` 100/100 with `--include-ignored` in debug and release; `c0_adm` 129/0; `c0_2ci` 180/0/21; `--lib` 1945/0/163. Record the per-filter counts (`c0_conv_ci_`, `_cii_`, `_ciii_`, `_cir_`) at the start of Task 1 and keep them; every task ends at those numbers plus its own new tests. The full hardware gate is the coordinator's, after the last task, with the user's go-ahead.

## Exit criteria

Each row is a spec §4.2 row; mutations are applied by the coordinator by line, confirmed to compile and to remove the behaviour.

| Criterion (spec) | Tests | Mutation that must fail them |
| --- | --- | --- |
| Registry and service installed together, same incarnation (2.1) | `c0_conv_cfb_registry_installed_with_the_service` | F1: install the service alone |
| The probe split is behaviour-preserving and the fixture's accepted entry is a real import (decision 13) | `c0_conv_cfb_probe_split_keeps_legacy_probe_result`, `c0_conv_cfb_real_import_candidate_vulkan` | F34: reorder validation before import; F35: insert the fixture's entry without the production insert |
| Legacy gets no handle from a `Managed` entry; Legacy cache behaviour unchanged (2.4, 1.1) | `c0_conv_cfb_managed_entry_serves_no_legacy_handle`, `c0_conv_cfb_legacy_probe_cache_unchanged` | F2: serve the handle anyway |
| Cache eviction cannot destroy a current or submitted allocation (2.4) | `c0_conv_cfb_eviction_leaves_a_live_allocation_vulkan` | F3: keep the cache as a strong owner |
| The backing serial advances on every storage replacement, checked, never wraps (2.3, decision 8) | `c0_conv_cfb_backing_serial_changes_on_relayout`, `c0_conv_cfb_backing_serial_exhaustion_refuses_adoption` | F4: skip the bump at one enumerated site; F4b: wrap on overflow |
| The index holds a live-checked token; stale or foreign-incarnation forces a fresh probe (2.3) | `c0_conv_cfb_stale_token_forces_a_fresh_probe_vulkan` | F5: upgrade the token without checking the incarnation |
| Index removed on the service's `1 → 0`, kept on `2 → 1`, including a never-dispatched allocation (2.3) | `c0_conv_cfb_index_removed_on_last_lease_drop_vulkan`, `c0_conv_cfb_index_kept_while_current_scans_vulkan`, `c0_conv_cfb_never_dispatched_allocation_reaches_zero_vulkan` | F6: remove on `2 → 1`; F7: never remove; F8: count only committed leases |
| Only direct-frame holders lease a framebuffer allocation (2.3, decision 10) | `c0_conv_cfb_only_direct_holders_lease_the_framebuffer_vulkan`, `c0_conv_cfb_permit_rejections` | F9: let generic `reserve` accept a `DirectFramebuffer` payload (the test then takes a generic `Retain` lease and observes the index kept after the last direct drop); F9a: accept a foreign permit; F9b: accept a stale permit; F9c: accept a wrong-role permit |
| A dispatched direct commit has non-empty `allocations` with the adopted framebuffer (3.1) | `c0_conv_cfb_dispatch_carries_the_framebuffer_vulkan` | F10: pass `Vec::new()` again |
| A valid same-source successor carries the same `AllocationKey` (2.3, 3.1) | `c0_conv_cfb_same_source_successor_reuses_the_key_vulkan` | F11: mint a new allocation on every preparation |
| After a relayout or topology change the key differs (2.3) | `c0_conv_cfb_relayout_or_topology_gives_a_new_key_vulkan` | F12a: omit the serial from the index key; F12b: omit the topology generation |
| The service step delivers the zero edge and removes the index before cleanup, at every site (decision 11) | `c0_conv_cfb_service_step_removes_the_index_before_cleanup_vulkan` (four cases, one per site, including a lease dropped by an owner event routed at that site) | F31: at one site, run the step before `route_owner_event_batch` again / discard the zero edges (applied per site); F32: destroy in readiness before the index is removed; F33: lose a zero edge when a new lease is minted after the drop |
| Source retention is the present pin; storage never adopted; a managed source passes its read lease (2.5) | `c0_conv_cfb_pin_retains_the_source_vulkan`, `c0_conv_cfb_storage_is_not_adopted_at_preparation_vulkan` | F13: release the pin before the framebuffer lease; F14: adopt the storage at preparation (a raw-storage consumer panics in the test) |
| Every pre-submit refusal returns the lease to the candidate exactly once (3.3) | `c0_conv_cfb_refusal_returns_the_lease_once_vulkan` | F15: drop it on refusal; F16: return it twice |
| Service adoption failure after `into_managed` releases once through the registry and closes admission (2.6) | `c0_conv_cfb_service_adoption_failure_releases_once` (the service's generation counter seeded at its last value: preflight passes, `adopt` fails) | F17: drop the returned payload; F18: reconstruct the cache as strong owner |
| A failed cleanup (forced `RMFB`, forced `GEM_CLOSE`) retains the payload with its charge; a later retry releases once (2.6) | `c0_conv_cfb_failed_cleanup_is_retained_and_retried` (two cases) | F19: drop the payload on cleanup failure; F20: release the `Preparing` charge before cleanup succeeds |
| A pending-cleanup entry counts as an incarnation alias, moves into the bundle at freeze, closes with the family without a stale ioctl (2.6) | `c0_conv_cfb_pending_cleanup_follows_the_freeze` | F21: leave it out of the alias count; F22: retry `consume` after the freeze; F23: issue `RMFB` on a closed family |
| FB/GEM cleanup happens exactly once, by the registry (3.4) | `c0_conv_cfb_cleanup_exactly_once_vulkan` | F24: also destroy in the entry's `Drop` |
| Both completion orders keep one lease owner (3.3) | `c0_conv_cfb_completion_orders_keep_one_owner_vulkan` | F25: discharge the framebuffer's obligation at `HardwareComplete` when no retirement recorded it |
| `ResourcesStillCurrent` returns the lease to current; `ResourcesReleased` releases it (3.3) | `c0_conv_cfb_rejection_returns_or_releases_the_lease_vulkan` | F26: drop the lease on `ResourcesStillCurrent` |
| Unknown / quarantine retains the lease (3.3) | `c0_conv_cfb_unknown_retains_the_lease_vulkan` | F27: release it on `CompletionUnknown` |
| Dispatch undo and successor terminalization return the lease once (3.3) | `c0_conv_cfb_undo_and_terminalize_return_once_vulkan` | F28: release it twice |
| Unflip exit retirement moves the lease with `ExitRetirement`; a rejected unflip restores `Current` with index and count intact (3.3) | `c0_conv_cfb_unflip_moves_the_lease_vulkan`, `c0_conv_cfb_rejected_unflip_restores_current_vulkan` | F29: leave it in `Current`; F30: append the returned record without restoring its role |

---

### Task 1: The registry's production home, and adoption at preparation

**Files:** `backend.rs` (`install_resource_service`; the Owner fixtures `owner_live_fixture*` and the `c0_adm`/`c0_conv` fixtures that build a `DrmCleanupRegistry` by hand; `managed_prepare_direct_candidate` and the Owner-only function it forks into; the fixture helper of decision 13), `direct_owner.rs` (the Owner adoption function lives here, not in the Legacy body), `drm/modeset.rs` (the probe split of decision 13; `into_managed` with `Option<AllocationLease>`), `resources/capacity.rs` (`DirectLeasePermit`), `resources/mod.rs` (adoptability check, the permit-verified lease entry of decision 10, the generation-seeding test constructor); tests.

**Invariants (spec 2.1, 2.2, 2.5, 2.6 steps 1–3; decisions 5, 6, 10).**
- The entry that installs the `ResourceService` installs the `DrmCleanupRegistry` with it, same device incarnation; one cannot be installed without the other; no fixture keeps a registry of its own; the producer reaches both with device scope and no long-lived backend borrow.
- On an Owner device, at the point that today only checks `fb_ready`: (1) adoptability — service not exhausted, `Preparing` reserved — a refusal rejects the candidate as `framebuffer_missing` does, the cache still the strong owner; (2) `into_managed` with the source's lease when the source backing is already managed by this device's service (the `share_storage_read` `pin_direct_source` takes) or `None` otherwise — **storage is never adopted here**; (3) service adoption — on success the candidate holds a role-proof lease; on failure the payload goes to registry cleanup (Task 2 gives a further failure its owner; in this task a further failure is an F8 report, not silent), the candidate is rejected, admission is closed on the device.
- The cache entry becomes `Managed` and serves no raw handle to the Legacy submit path (fail closed if reached). The index of Task 3 does not exist yet: in this task a same-source second candidate probes and imports afresh; Task 3 makes it reuse.
- The permit-verified lease entry of decision 10: generic `reserve` refuses a `DirectFramebuffer` payload; a foreign, stale or wrong-role `DirectLeasePermit` is rejected.
- The probe split of decision 13 is a pure refactor: the Legacy route's probe result is unchanged, and the fixture helper `owner_direct_candidate_with_real_import` drives the real import on the real node and inserts through the production cache insert.
- The pin is released only after the framebuffer lease, by the ledger's release path (Task 4 proves the release; this task proves the order at preparation).
- Legacy devices: nothing runs.

**Named tests:** `c0_conv_cfb_registry_installed_with_the_service` (deterministic); `c0_conv_cfb_legacy_probe_cache_unchanged` (deterministic; pin the observable); `c0_conv_cfb_probe_split_keeps_legacy_probe_result` (deterministic); `c0_conv_cfb_managed_entry_serves_no_legacy_handle` (deterministic); `c0_conv_cfb_permit_rejections` (deterministic); `c0_conv_cfb_service_adoption_failure_releases_once` (deterministic: a service whose generation counter is seeded at its last value passes preflight and fails `adopt`); `c0_conv_cfb_real_import_candidate_vulkan` (the helper's entry holds a real FB/GEM on the real node); on the Owner live fixture, through `managed_prepare_direct_candidate`: `c0_conv_cfb_pin_retains_the_source_vulkan`, `c0_conv_cfb_storage_is_not_adopted_at_preparation_vulkan` (after preparation the source backing is what it was and `root_storage_extent` still works), `c0_conv_cfb_only_direct_holders_lease_the_framebuffer_vulkan` (generic `reserve` on the adopted key is refused; the role-proof entry succeeds).

- [ ] Steps: tests; red; implement; checks; stop dirty and report the fork point, the Owner function, and which fixtures lost their private registry.

---

### Task 2: The pending-cleanup owner

**Files:** `resources/drm_cleanup.rs` (`DrmCleanupRegistry`: pending entries, alias count, `freeze_incarnation`, the R3 retry, `MockCleanupIo`), `resources/mod.rs`; tests.

**Invariants (spec 2.6, "cleanup itself is fallible").**
- A payload whose cleanup failed (Task 1's step-3 failure path, or any later registry cleanup of a framebuffer allocation) is retained as a **pending-cleanup entry**: keyless, holding the payload with its right, device alias and `Preparing` charge, counted in the incarnation's alias total exactly like an adopted payload alias.
- Dispositions: while live, the R3 retry may succeed → right spent, alias dropped, charge released, entry gone; at `freeze_incarnation` the entries move into the incarnation bundle with the rest of pending cleanup and are not retried; after family closure each entry's right is marked closed with no ioctl, alias dropped, charge released. Never dropped, never reconstructed as a cache owner.

**Named tests** (deterministic where the mock cleanup I/O can force the failure; the payload comes from Task 1's producer on a fixture — say which): `c0_conv_cfb_failed_cleanup_is_retained_and_retried` (forced `RMFB` failure and, separately, forced `GEM_CLOSE` failure: entry present, counted as an alias, charge kept; a later successful retry releases exactly once); `c0_conv_cfb_pending_cleanup_follows_the_freeze` (cleanup fails, then freeze: entry in the bundle, `consume` not retried, closed with the family with no ioctl, charge released).

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 3: The backing serial, the live-checked index, and the service step

**Files:** `store.rs` (`Drawable`, every site that replaces `storage`), `backend.rs` (`ScanoutM1ProbeCache`/`ScanoutM1ProbeEntry`: the index and its removal sites; the authoritative service step of decision 11 at both completion sites), `scene.rs` (its two `service_completions` calls route through the step), `resources/mod.rs` (the token; the last-lease transition); tests.

**Invariants (spec 2.3, 2.4; decisions 3, 4, 8, 9, 11).**
- **First step: enumerate** every site that replaces a drawable's `storage`; each advances the serial with a checked increment; exhaustion marks the drawable and refuses Owner direct adoption of it permanently. Report the enumeration.
- A `Managed` entry holds the index key `(source DrawableId, backing serial, topology generation)` and a live-checked token; lookup succeeds only when the token upgrades to a live allocation of the current incarnation and the key matches exactly; a same-source candidate whose lookup succeeds reuses the allocation with a new role-proof lease; eviction removes the index and touches the allocation not at all.
- **The two-phase service step** (decision 11): the service records zero edges at lease drop and returns them without destroying; the backend step runs after `route_owner_event_batch` at both backend sites and after the scene's drain, removes the index for each zero edge (and on incarnation loss and on the source drawable's drop, the existing `scanout_m1.remove` site), then authorizes cleanup. A zero edge survives a lease minted after the drop.

**Named tests:** `c0_conv_cfb_backing_serial_changes_on_relayout` (deterministic; every enumerated site), `c0_conv_cfb_backing_serial_exhaustion_refuses_adoption` (deterministic); on the Owner live fixture: `c0_conv_cfb_same_source_successor_reuses_the_key_vulkan`, `c0_conv_cfb_relayout_or_topology_gives_a_new_key_vulkan`, `c0_conv_cfb_stale_token_forces_a_fresh_probe_vulkan`, `c0_conv_cfb_index_removed_on_last_lease_drop_vulkan`, `c0_conv_cfb_index_kept_while_current_scans_vulkan`, `c0_conv_cfb_never_dispatched_allocation_reaches_zero_vulkan`, `c0_conv_cfb_eviction_leaves_a_live_allocation_vulkan`, `c0_conv_cfb_service_step_removes_the_index_before_cleanup_vulkan`.

- [ ] Steps: enumeration; tests; red; implement; checks; stop dirty and report.

---

### Task 4: The handoff into the ledger and every ownership row

**Files:** `direct_owner.rs` (`resources`), `backend.rs` (`take_direct_owner_resources`, `managed_undo_direct_dispatch`, `managed_terminalize_queued_direct_successor`, the unflip seam that moves `Current` to `ExitRetirement`), `resources/commit.rs` (`consume`: `ResourcesStillCurrent` restores the role); tests.

**Invariants (spec 3.1–3.4).**
- `direct_owner::resources` moves the framebuffer lease by value into `CommitResources::allocations`, beside `source`/`fallback`; `register_commit_dependencies` is not touched.
- The lease travels with the role: `Preparing` → `Successor` → `Submitted` → `Current` → `OrdinaryRetirement` / `ExitRetirement`; two leases on one allocation may coexist in `Current` and `Successor`, inside existing capacity.
- Every row of spec 3.3 holds with one owner before and after: successor replacement, capacity/`begin`/pre-IPC refusal (lease back on the candidate, once), kernel rejection (`ResourcesStillCurrent` returns the record **and restores its role**; `ResourcesReleased` releases), completion in both orders, reuse (retained allocation registers no obligation), unknown/quarantine (retained), dispatch undo and terminalization (once), the four unflip rows (dispatched → `ExitRetirement`; rejected → `Current` restored, exit capacity released; completed → releasing; unknown → retained).
- Cleanup happens exactly once, by the registry, when the service reports the allocation releasable with no lease.

**Named tests** (`_vulkan`, milestones through `route_owner_event_batch`): `c0_conv_cfb_dispatch_carries_the_framebuffer_vulkan`, `c0_conv_cfb_refusal_returns_the_lease_once_vulkan` (each pre-submit refusal the 2c-ii fixtures can produce), `c0_conv_cfb_completion_orders_keep_one_owner_vulkan`, `c0_conv_cfb_rejection_returns_or_releases_the_lease_vulkan`, `c0_conv_cfb_unknown_retains_the_lease_vulkan`, `c0_conv_cfb_undo_and_terminalize_return_once_vulkan`, `c0_conv_cfb_unflip_moves_the_lease_vulkan`, `c0_conv_cfb_rejected_unflip_restores_current_vulkan`, `c0_conv_cfb_cleanup_exactly_once_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 5: Evidence closure and the Ciii re-entry check

**Files:** tests only, plus `docs/status.md`.

**Invariants.**
- Every row of the exit table has its named test green on the GPU in debug and release, and every earlier suite is at its baseline plus this plan's tests.
- **The Ciii re-entry check (spec 4.3):** through the production producer on the Owner live fixture, a composed → direct → same-source direct sequence yields two direct commits whose `allocations` carry the **same** `AllocationKey` for the same `GroupMember`, and `register_commit_dependencies` registers no `KmsRelease` for it (P3-3's shape, hardware-free), while a different-source successor's old allocation does get one (P3-2's shape). Named test: `c0_conv_cfb_ciii_task6_shape_is_reachable_vulkan`, built with Task 1's `owner_direct_candidate_with_real_import` (two candidates of the **same** source pixmap, the second while the first is current; the target stays the frontmost window per upstream #163). If it cannot be built that way, F8 — that is exactly what this plan exists to make reachable.
- A `docs/status.md` entry: what changed, that Ciii Task 6 is unblocked, and that the managed-storage access adaptation stays open.
- The three portability `cargo check` targets.

- [ ] Steps: tests; checks incl. portability; status entry; stop dirty and report.

---

## What the coordinator does

After each task: read the diff, grep for `owner_route`-style branches inside Legacy bodies, re-run the checks outside codex, apply this plan's mutations F1–F35 **by line** against the recorded mutation text, confirm each is caught by its named test (or record it equivalent with the reason), then commit with `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)`. `_vulkan` tests and their mutations run on the GPU with the user's go-ahead, from tty when the user asks for it.

After Task 5, with the user's go-ahead and the GPU free: the full hardware gate (`render_acceptance -- --ignored`, `c0_2ci -- --ignored`, the library's other ignored tests, `c0_conv_ -- --include-ignored` — 359/359 at `aaac1a8f` plus this plan's tests), the acceptance finding, and then plan Ciii Task 6 resumes: `c0_hw_ciii_owner_route_on_card1_drm` from tty2 with T32 and T33.
