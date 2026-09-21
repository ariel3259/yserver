# Stage 2c-iii, plan Cfb — direct framebuffer adoption and retention

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for hardware work) with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters (`c0_conv_cfb_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_ci_`, `c0_conv_cir_`) with `--include-ignored`, never `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time. Stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code, stop and report it (F8); never silently substitute a test shape.

**Revision 1 (2026-09-21)** — not yet reviewed.

**Goal:** Close plan Ciii's Task 6 F8. On the Owner route a direct commit carries its scanned framebuffer as a managed `DirectFramebufferAllocation` lease in `CommitResources::allocations`, a valid same-source successor carries the same `AllocationKey`, the registry that destroys FB/GEM imports lives beside the resource service, and every rollback and terminal path has exactly one owner. With it, Ciii Task 6 resumes as written.

**Architecture:** Five tasks, each a coherent unit: the registry's production home and its pending-cleanup owner (`resources/drm_cleanup.rs`, `backend.rs`); the backing serial and the live-checked index (`store.rs`, `backend.rs`'s M1 cache, `resources/mod.rs`); adoption at preparation with the transaction of spec §2.6 (`backend.rs` `managed_prepare_direct_candidate`, `drm/modeset.rs`); the handoff into the ledger and every ownership row (`direct_owner.rs`, `resources/commit.rs`, the unflip seam); the evidence. Owner-route code goes in its own functions, reached from the existing fork points (the Cii/Ciii constraint); nothing is interleaved into a Legacy function body. Production is unchanged (C0-R8): no registry or service is installed there.

**Spec:** `docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md` at revision 4 (`ec97fc88`), whole. Read first: its "Why this document exists", §2 (2.1–2.6), §3 (3.1–3.4, the ownership table), §4.2 (every row is an exit criterion here). Then the F8 that motivates it, `../findings/2026-09-21-stage-2c-iii-plan-ciii-task6-f8.md`, and the Ciii plan's Task 6 (`2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md`), which this plan unblocks. Read the Cii commit messages for the direct producer (`git log --grep 'Cii task'` is read-only and allowed).

## Design decisions this plan fixes

Items 1–7 come from the spec (user-approved); items 8–10 are this plan's.

1. **Owner route only** (spec 1.1). Legacy's probe, cache ownership, `submit_direct_frame` and eviction are byte-for-byte unchanged.
2. **The registry lives beside the service** (2.1): installed by the same entry as the service, same incarnation; the Owner fixtures stop building their own.
3. **Identity is the service's `AllocationKey`** behind a live-checked, non-owning token in the M1 cache index, keyed by `(source DrawableId, backing serial, topology generation)` (2.3).
4. **The count lives in the service** (2.3): only direct-frame holders lease a framebuffer allocation, so the service's live-use count on the key is the direct-lease count; index removal on `1 → 0`, on incarnation loss, on source-drawable drop.
5. **Storage is never adopted at preparation** (2.5): the present pin retains the source; `into_managed` takes `Option<AllocationLease>`.
6. **The adoption transaction** (2.6): adoptability check → `into_managed` (payload is sole owner) → service adopt; a service failure goes to registry cleanup and closes admission; a cleanup failure goes to a keyless, alias-counted pending-cleanup entry with retry / freeze-handoff / family-closed dispositions.
7. **Every ownership row of 3.3**, including the four unflip rows and the rejected-unflip `ExitRetirement → Current` restore.
8. **The backing serial** is a `u64` on the store's `Drawable`, bumped at every site that replaces the drawable's `storage` (relayout, migration, re-import, `adopt_exportable_managed`). Task 2's first step is the implementer's enumeration of those sites; the plan does not close the list.
9. **The live-checked token** is a service-issued handle that carries the `AllocationKey` and can be asked to upgrade to a lease only while the allocation is live in the current incarnation; a stale token upgrades to nothing. Its shape is yours; it must not keep the allocation alive.
10. **Test names start with `c0_conv_cfb_`**; Vulkan tests end in `_vulkan` with `#[ignore = "needs live Vulkan ICD"]`, on the Owner live fixture (`for_tests_with_vk_live_scene_real_drm`) with a managed pool. Deterministic tests use the stub-executor fixtures where no Vulkan is needed.

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
| Legacy gets no handle from a `Managed` entry; Legacy cache behaviour unchanged (2.4, 1.1) | `c0_conv_cfb_managed_entry_serves_no_legacy_handle`, `c0_conv_cfb_legacy_probe_cache_unchanged` | F2: serve the handle anyway |
| Cache eviction cannot destroy a current or submitted allocation (2.4) | `c0_conv_cfb_eviction_leaves_a_live_allocation_vulkan` | F3: keep the cache as a strong owner |
| The backing serial changes on every storage replacement (2.3, decision 8) | `c0_conv_cfb_backing_serial_changes_on_relayout` | F4: key by `DrawableId` alone (leave the serial out of the key) |
| The index holds a live-checked token; stale or foreign-incarnation forces a fresh probe (2.3) | `c0_conv_cfb_stale_token_forces_a_fresh_probe_vulkan` | F5: upgrade the token without checking the incarnation |
| Index removed on the service's `1 → 0`, kept on `2 → 1`, including a never-dispatched allocation (2.3) | `c0_conv_cfb_index_removed_on_last_lease_drop_vulkan`, `c0_conv_cfb_index_kept_while_current_scans_vulkan`, `c0_conv_cfb_never_dispatched_allocation_reaches_zero_vulkan` | F6: remove on `2 → 1`; F7: never remove; F8: count only committed leases |
| Only direct-frame holders lease a framebuffer allocation (2.3) | `c0_conv_cfb_only_direct_holders_lease_the_framebuffer_vulkan` | F9: take a lease on it from a non-direct consumer (the count must go wrong) |
| A dispatched direct commit has non-empty `allocations` with the adopted framebuffer (3.1) | `c0_conv_cfb_dispatch_carries_the_framebuffer_vulkan` | F10: pass `Vec::new()` again |
| A valid same-source successor carries the same `AllocationKey` (2.3, 3.1) | `c0_conv_cfb_same_source_successor_reuses_the_key_vulkan` | F11: mint a new allocation on every preparation |
| After a relayout or topology change the key differs (2.3) | `c0_conv_cfb_relayout_or_topology_gives_a_new_key_vulkan` | F12: ignore the backing serial and topology generation in the lookup |
| Source retention is the present pin; storage never adopted; a managed source passes its read lease (2.5) | `c0_conv_cfb_pin_retains_the_source_vulkan`, `c0_conv_cfb_storage_is_not_adopted_at_preparation_vulkan` | F13: release the pin before the framebuffer lease; F14: adopt the storage at preparation (a raw-storage consumer panics in the test) |
| Every pre-submit refusal returns the lease to the candidate exactly once (3.3) | `c0_conv_cfb_refusal_returns_the_lease_once_vulkan` | F15: drop it on refusal; F16: return it twice |
| Service adoption failure after `into_managed` releases once through the registry and closes admission (2.6) | `c0_conv_cfb_service_adoption_failure_releases_once` | F17: drop the returned payload; F18: reconstruct the cache as strong owner |
| A failed cleanup (forced `RMFB`, forced `GEM_CLOSE`) retains the payload with its charge; a later retry releases once (2.6) | `c0_conv_cfb_failed_cleanup_is_retained_and_retried` (two cases) | F19: drop the payload on cleanup failure; F20: release the `Preparing` charge before cleanup succeeds |
| A pending-cleanup entry counts as an incarnation alias, moves into the bundle at freeze, closes with the family without a stale ioctl (2.6) | `c0_conv_cfb_pending_cleanup_follows_the_freeze` | F21: leave it out of the alias count; F22: retry `consume` after the freeze; F23: issue `RMFB` on a closed family |
| FB/GEM cleanup happens exactly once, by the registry (3.4) | `c0_conv_cfb_cleanup_exactly_once_vulkan` | F24: also destroy in the entry's `Drop` |
| Both completion orders keep one lease owner (3.3) | `c0_conv_cfb_completion_orders_keep_one_owner_vulkan` | F25: discharge the framebuffer's obligation at `HardwareComplete` when no retirement recorded it |
| `ResourcesStillCurrent` returns the lease to current; `ResourcesReleased` releases it (3.3) | `c0_conv_cfb_rejection_returns_or_releases_the_lease_vulkan` | F26: drop the lease on `ResourcesStillCurrent` |
| Unknown / quarantine retains the lease (3.3) | `c0_conv_cfb_unknown_retains_the_lease_vulkan` | F27: release it on `CompletionUnknown` |
| Dispatch undo and successor terminalization return the lease once (3.3) | `c0_conv_cfb_undo_and_terminalize_return_once_vulkan` | F28: release it twice |
| Unflip exit retirement moves the lease with `ExitRetirement`; a rejected unflip restores `Current` with index and count intact (3.3) | `c0_conv_cfb_unflip_moves_the_lease_vulkan`, `c0_conv_cfb_rejected_unflip_restores_current_vulkan` | F29: leave it in `Current`; F30: append the returned record without restoring its role |

---

### Task 1: The registry's production home and the pending-cleanup owner

**Files:** `backend.rs` (`install_resource_service`, the Owner fixtures `owner_live_fixture*` and the `c0_adm`/`c0_conv` fixtures that build a `DrmCleanupRegistry` by hand), `resources/drm_cleanup.rs` (`DrmCleanupRegistry`, `discharge_file_owned`, the R3 retry, `freeze_incarnation`, the alias count); tests.

**Invariants (spec 2.1, 2.6).**
- The entry that installs the `ResourceService` installs the `DrmCleanupRegistry` with it, both for the same device incarnation; there is no way to install one without the other, and no fixture keeps a registry of its own. The producer reaches both with device scope and no long-lived backend borrow.
- The registry gains **pending-cleanup entries**: keyless, each holding a `DirectFramebufferAllocation` whose cleanup failed, its right, its device alias and its `Preparing` charge; each counts in the incarnation's alias total exactly like an adopted payload alias (`payload_aliases()`).
- Dispositions: while live, the R3 retry may succeed → right spent, alias dropped, charge released, entry gone; at `freeze_incarnation` the entries move into the incarnation bundle with the rest of pending cleanup and are not retried; after the family's closure each entry's right is marked closed without any ioctl, alias dropped, charge released. Never dropped, never reconstructed as a cache owner.
- Legacy: nothing changes (no registry is installed on a Legacy device).

**Named tests:** `c0_conv_cfb_registry_installed_with_the_service` (deterministic; installing the service alone is impossible or refused, say which); `c0_conv_cfb_failed_cleanup_is_retained_and_retried` (deterministic, using the registry's mock cleanup I/O to force `RMFB` failure and, separately, `GEM_CLOSE` failure: the entry exists, counts as an alias, keeps its charge; a later successful retry releases exactly once); `c0_conv_cfb_pending_cleanup_follows_the_freeze` (cleanup fails, then freeze: the entry is in the bundle, `consume` is not retried, after family closure it is closed with no ioctl and the charge is released); `c0_conv_cfb_legacy_probe_cache_unchanged` (a Legacy device's probe, cache and eviction behave as at `ec97fc88`; pin the observable you use).

- [ ] Steps: tests; red; implement; checks; stop dirty and report which fixtures lost their private registry.

---

### Task 2: The backing serial and the live-checked index

**Files:** `store.rs` (`Drawable`, every site that replaces `storage`), `backend.rs` (`ScanoutM1ProbeCache` and `ScanoutM1ProbeEntry`: the `Managed` entry, the index, its removal sites), `resources/mod.rs` (the token, the `1 → 0` observation); tests.

**Invariants (spec 2.3, 2.4; decisions 3, 4, 8, 9).**
- **First step: enumerate** every site that replaces a drawable's `storage` (relayout, migration, re-import, `adopt_exportable_managed`, and whatever else exists); each bumps the backing serial. Report the enumeration.
- A `Managed` cache entry holds the index key `(source DrawableId, backing serial, topology generation)` and a live-checked token; it serves no raw framebuffer handle to the Legacy submit path (fail closed if reached); eviction removes the index and touches the allocation not at all.
- Lookup succeeds only when the token upgrades to a live allocation of the current incarnation and the key matches exactly.
- The service observes the last-lease drop of a `DirectFramebufferAllocation` and the index is removed on it, on incarnation loss, and on the source drawable's drop (the existing `scanout_m1.remove` site). Only direct-frame holders lease a framebuffer allocation: the service refuses (or the plan's mutation F9 makes the count observably wrong) any other lease on it — say which shape you chose.

**Named tests:** `c0_conv_cfb_backing_serial_changes_on_relayout` (deterministic: every enumerated site bumps it); `c0_conv_cfb_managed_entry_serves_no_legacy_handle` (deterministic). The Vulkan ones of this task's rows (`stale_token_forces_a_fresh_probe`, `index_removed_on_last_lease_drop`, `index_kept_while_current_scans`, `never_dispatched_allocation_reaches_zero`, `only_direct_holders_lease_the_framebuffer`, `eviction_leaves_a_live_allocation`) need Task 3's adoption to produce entries; write them in Task 3 against the index built here, and say so in this task's report.

- [ ] Steps: enumeration; tests; red; implement; checks; stop dirty and report.

---

### Task 3: Adoption at preparation

**Files:** `backend.rs` (`managed_prepare_direct_candidate` and the Owner-only function it forks into — the Owner adoption lives in its own function, e.g. in `direct_owner.rs`, not interleaved into the Legacy body), `drm/modeset.rs` (`into_managed` with `Option<AllocationLease>`), `resources/mod.rs` (adoptability check); tests.

**Invariants (spec 2.2, 2.5, 2.6).**
- On an Owner device, at the point that today only checks `fb_ready`: (1) adoptability check — service not exhausted, `Preparing` reserved — a refusal rejects the candidate as `framebuffer_missing` does, cache still the strong owner; (2) `into_managed` with the source's lease if the source backing is already managed by this device's service (the `share_storage_read` that `pin_direct_source` takes) or `None` otherwise — **storage is never adopted here**; (3) service adoption — on success the candidate holds the lease and the index is written; on failure the payload goes to registry cleanup (Task 1's owner on a further failure), the index is not written, the candidate is rejected, admission is closed on the device.
- A same-source candidate whose lookup succeeds reuses the allocation with a new lease; a mismatched key or a stale token probes and imports afresh.
- The pin is released only after the framebuffer lease, by the ledger's release path.
- Legacy devices: nothing runs.

**Named tests** (`_vulkan`, on the Owner live fixture, through `managed_prepare_direct_candidate` / the direct offer): `c0_conv_cfb_pin_retains_the_source_vulkan`, `c0_conv_cfb_storage_is_not_adopted_at_preparation_vulkan` (after preparation the source drawable's backing is what it was, and a raw-storage consumer such as the root extent lookup still works), `c0_conv_cfb_same_source_successor_reuses_the_key_vulkan`, `c0_conv_cfb_relayout_or_topology_gives_a_new_key_vulkan`, `c0_conv_cfb_stale_token_forces_a_fresh_probe_vulkan`, `c0_conv_cfb_service_adoption_failure_releases_once` (deterministic if a service failure can be forced without Vulkan, say which), `c0_conv_cfb_eviction_leaves_a_live_allocation_vulkan`, `c0_conv_cfb_only_direct_holders_lease_the_framebuffer_vulkan`, and Task 2's index tests (`index_removed_on_last_lease_drop`, `index_kept_while_current_scans`, `never_dispatched_allocation_reaches_zero`).

- [ ] Steps: tests; red; implement; checks; stop dirty and report the exact fork point and the Owner function it reaches.

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
- **The Ciii re-entry check (spec 4.3):** through the production producer on the Owner live fixture, a composed → direct → same-source direct sequence yields two direct commits whose `allocations` carry the **same** `AllocationKey` for the same `GroupMember`, and `register_commit_dependencies` registers no `KmsRelease` for it (P3-3's shape, hardware-free), while a different-source successor's old allocation does get one (P3-2's shape). Named test: `c0_conv_cfb_ciii_task6_shape_is_reachable_vulkan`. If it cannot be built through production entries, F8 — that is exactly what this plan exists to make reachable.
- A `docs/status.md` entry: what changed, that Ciii Task 6 is unblocked, and that the managed-storage access adaptation stays open.
- The three portability `cargo check` targets.

- [ ] Steps: tests; checks incl. portability; status entry; stop dirty and report.

---

## What the coordinator does

After each task: read the diff, grep for `owner_route`-style branches inside Legacy bodies, re-run the checks outside codex, apply this plan's mutations F1–F30 **by line** against the recorded mutation text, confirm each is caught by its named test (or record it equivalent with the reason), then commit with `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)`. `_vulkan` tests and their mutations run on the GPU with the user's go-ahead, from tty when the user asks for it.

After Task 5, with the user's go-ahead and the GPU free: the full hardware gate (`render_acceptance -- --ignored`, `c0_2ci -- --ignored`, the library's other ignored tests, `c0_conv_ -- --include-ignored` — 359/359 at `aaac1a8f` plus this plan's tests), the acceptance finding, and then plan Ciii Task 6 resumes: `c0_hw_ciii_owner_route_on_card1_drm` from tty2 with T32 and T33.
