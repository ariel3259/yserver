# Phase C0 Stage 2c-i Resource Ownership and Terminalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Do not dispatch additional agents without the applicable authorization.

**Goal:** Retain real allocation generations across commit outcomes, separate Present completion from release, and provide bounded resource and teardown interfaces for subsequent C0 stages.

**Architecture:** Keep `DeviceCommitOwner<R>` backend-independent and instantiate its backend boundary with `CommitResources`. A core-thread resource service roots concrete allocations, arbitrates usage and proof-gated cleanup, and survives by-value handoff into an incarnation bundle. Legacy production submission remains active; converted resource adapters are exercised through real backing objects and controlled completion/transport fixtures until later stages supply activation prerequisites.

**Tech Stack:** Existing Rust 2024 workspace, Vulkan/ash, DRM/GBM, existing owner/executor and core poll facilities. No new dependency or ioctl is required by this plan.

**Spec:** Read [resource design](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md), [concrete adapter inventory](../specs/2026-09-09-phase-c0-stage-2c-i-resource-adapter-inventory.md), [2c decomposition](../specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md), and governing [C0 specification](../specs/2026-08-26-phase-c0-atomic-kms-migration-design.md), especially §§9.1, 10–10.4, 12 and 18.

**Status:** Executed once by Gemini 3.8 Flash (`38ce1eb4..5ac85777`, 2026-09-10/11) and **rejected in [implementation review round 1](../findings/2026-09-11-stage-2c-i-implementation-review-round1.md)** (16 blocking, 24 major): the type vocabulary, the Task-1 ledger, the R3 `consume` state machine, the GPU batch machine and `DirectCapacity` are sound and kept; the ownership mechanism (alias registration, `file_owned` discharge, real barriers, sink gating, displaced-pair producer, handoff revocation) is absent. Steps left unchecked below were found not done or not proven; the fix is dispatched through `docs/handoff-phase-c0-stage-2c-i-fix.md`. Original drafting history follows. Its [first plan review](../findings/2026-09-09-stage-2c-i-implementation-plan-review-round1.md) reported 1 blocking, 2 major, 1 minor and its [second plan review](../findings/2026-09-10-stage-2c-i-implementation-plan-review-round2.md) reported 1 blocking, 2 major, 0 minor, both with complete declared coverage; round 2 recorded all four round-1 corrections as applied. Round-2 B-1 (KMS disposition at teardown release), M-1 (owner-writer authority) and M-2 (overlay physical-retirement transition) are corrected below. The [third plan review](../findings/2026-09-10-stage-2c-i-implementation-plan-review-round3.md) — run through `review-claude.sh` and not comparable to the codex rounds — reported 1 blocking, 2 major, 2 minor with complete declared coverage and recorded all three round-2 corrections as applied; its B-1 (registry-rooted GBM device holders block the fd-family barrier), M-1 (no producer for the old set's `KmsRelease` proof), M-2 (quiescing precondition), m-1 and m-2 are corrected below. The [fourth plan review](../findings/2026-09-10-stage-2c-i-implementation-plan-review-round4.md) (same claude instrument) reported 1 blocking, 2 major, 2 minor with complete declared coverage; its B-1 (two owners of one GEM handle on the GBM path — also a latent baseline defect), M-1 (file-owned/shared payload halves), M-2 (grant consumption at send; handoff revokes before close), m-1 (displaced pairs only) and m-2 (serviced-time deadline) are corrected below. Those corrections are local and have not received another external pass. The [round-3 review](../findings/2026-09-09-stage-2c-i-adversarial-review-round3.md) accepted the design as a basis for writing this plan. It did not review this plan. Original baseline `14dd92d818e619357c903ad64e5357f47619111e`; this revision accounts for integration of upstream `a06cf0e00c5ba41431966732b71df802b7d0a51b`, including Chrome DRI3 and Composite overlay ownership fixes. Earlier design/plan verdicts do not certify these additions.

## Global Constraints

The following requirements are quoted from the resource design and apply to every task:

- “Keep the generic `DeviceCommitOwner<R>` independent of `DrawableStore` and the Vulkan engine.”
- “An `Rc` reaching zero does not establish GPU idleness or the teardown barrier.”
- “A current buffer's own presentation never proves it idle.”
- “Cross-device grouped direct is not introduced.”
- “Do not drop or merge distinct required notifications to impose a cap.”
- “Production continues using its existing route until 2c-iii and later-stage activation requirements are satisfied.”

Additional repository/execution constraints:

- Preserve Xorg-aligned Present semantics: no new protocol credits. Six direct frame-resource roles bound retained generations per supported grouped unit, not bytes or total Present metadata.
- Retain v1.5.0 layout offset, original X11 depth, typed bounds, guarded old-XID removal, dormant damage and scene wake semantics. Canonical scene-copy storage is not implemented and is not a dependency.
- Keep allocations, mapped pointers and GBM state on the core thread. Do not add `Send`/`Sync` to fit `BatchResource`, which currently requires `Send`; use a core-thread retirement lane for the new leases.
- Linux glibc, Linux musl and FreeBSD must compile. FD readiness is not proof of successful completion; errors fail closed. Never invent a platform status fallback.
- Work on the existing feature worktree. Preserve unrelated local documentation and review results. No push, squash merge or production activation is part of this plan.
- Before **every commit**, run `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`, and the task's focused tests. A failing check blocks the commit. Stage only named task files.
- Plan review must use `docs/superpowers/review/review.sh`; another external pass needs explicit user authorization under its README. This drafting task does not launch that pass.

## File and dependency map

Paths below are relative to `crates/yserver/src/` unless stated otherwise. New module names are proposed interfaces, not claims that symbols already exist.

| Task | Files created | Existing seams modified | Independently testable deliverable |
| --- | --- | --- | --- |
| 1 | `kms/render/resources/{mod,lease,availability,tests}.rs` | `kms/render/mod.rs` | Rooted allocation leases and serialized check/reserve/proof transitions |
| 2 | `kms/render/resources/drm_cleanup.rs` | `drm/modeset.rs` | Consuming FB/GEM cleanup rights, registry closure and direct import ownership |
| 3 | `kms/render/resources/storage.rs` | `kms/render/{store,engine,target}.rs` | Actual storage generations, layout protection and promotion retirement |
| 4 | `kms/render/resources/scanout.rs` | `kms/vk/scanout.rs`, `kms/render/platform.rs` | Retained shared/copied BOs and reuse-safe pool acquisition |
| 5 | `kms/render/resources/gpu.rs` | `kms/render/{backend,scene,engine,frame_builder}.rs`, `kms/vk/ops/mod.rs` | GPU/read/descriptor lifetime adapters and source/scratch separation |
| 6 | `kms/render/resources/{completion,transport}.rs` | `kms/render/{backend,platform}.rs` | Progress without composition and closed production activation boundary |
| 7 | `kms/render/resources/{commit,present}.rs` | `kms/render/{backend,platform,present_completion}.rs`; owner tests | Real `R`, by-value outcome consumption and independent Present dispositions |
| 8 | `kms/render/resources/capacity.rs` | `kms/render/resources/commit.rs`, backend direct-resource preparation seam | Six physical roles, ordinary/exit retirement and bounded probe ownership |
| 9 | `kms/render/resources/handoff.rs` | `kms/render/{backend,platform}.rs`; owner/executor test support | Reserved recipient and atomic transfer tested without a live stage-3 supervisor |
| 10 | `kms/render/resources/adapter_tests.rs` | All affected test modules; `docs/status.md` | Concrete adapter regression matrix and integration/portability evidence |

Dependency order: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10. Tasks 3–5 retain existing legacy allocation constructors; managed adoption is explicit, consumes backing ownership, and is unavailable to a production Owner producer in this stage. Do not replace legacy proof semantics with synthetic owner evidence.

## Shared interface vocabulary

Code blocks specify new interfaces, regression assertions and critical implementation fragments. Signature-only `impl` blocks are API contracts; provide the bodies in the owning task before running its green test. Standard-library types use ordinary imports from `std`; existing project types use the paths specified here. Real compiler/test failures during execution remain authoritative, and do not waive a resource contract merely to make a snippet compile.


Define these types in Task 1 and keep their spellings across tasks:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct AllocationKey {
    pub device: crate::platform::drm::DrmDeviceKey,
    pub incarnation: crate::kms::owner::identity::IncarnationId,
    pub generation: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct UseId(u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ObligationId(u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UseKind { Retain, Read, Write, Kms }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObligationKind { KmsRelease, Gpu, Read, ForeignReturn }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceError {
    Busy, WrongIncarnation, Exhausted, Frozen, Detached, InvalidProof,
}
```

`generation`, `UseId` and `ObligationId` use checked monotonic allocation. Exhaustion closes new managed admission; never wrap. Include the stable device key because incarnation counters alone are not a global device identity.

`AllocationLease` is non-Clone and owns an `Rc<AllocationEntry>` plus one `UseId`. `AllocationEntry` owns the payload and a single availability state; the service registry roots every entry until safe cleanup. Dropping a lease only ends that reservation and schedules service; it never destroys payload or discharges an unfinished obligation. Allocation entries contain no owner/backend/service strong reference. The stable inbox is separate from entries, preventing an ownership cycle. A backend-wide allocation index records the canonical service/key for each actual allocation. An alias on another output/device refers to that same entry and routes its completion to that entry's inbox; it never re-adopts the same backing into a second service. Evidence registrations retain their producer device/incarnation/commit in addition to the allocation key, so proving one output's release cannot satisfy another's obligation. Transferring an entry's service preserves all its aliases and routing; cross-device grouped direct remains unsupported.

`ResourceService` owns the registry, counters, pending evidence inbox and dirty-entry worklist. All work is serialized on the core thread. `AllocationPayload` is an enum expanded by Tasks 2–5 with real backing owners; a `#[cfg(test)] Spy` variant records destruction. A variant is added together with its concrete cleanup implementation, not as an empty placeholder.

Do not expose an unrestricted `complete(key, kind)` API to producers. Task 1 keeps `apply_validated_proof(&mut self, key: AllocationKey, obligation: ObligationId) -> Result<(), ResourceError>` private to `resources`; Tasks 5–7 create producer adapters that correlate and validate actual evidence before invoking it. Test modules can call it to control ordering. A malformed/foreign proof changes no allocation and closes the affected converted route.

## Task 1: Rooted leases and authoritative availability

**Status: EXECUTED at `38ce1eb4`.** **Review round 1 (2026-09-11): kept** — ledger sound; M-17 (`apply_validated_proof` visibility) open. The shown code below has been reconciled with
what compiled and passed all gate checks. **Fix round 1: `95f9dba6`.** M-17: **RESOLVED** (`apply_validated_proof` is now `pub(in crate::kms::render::resources)`; `store.rs`'s tests, which live outside `resources`, go through a new `#[cfg(test)] apply_validated_proof_for_tests` shim). See Task 2's fix-round entry for the fix session's full scope.

**Execution notes:**
- `ResourceService::cancel(&mut self, key: AllocationKey, obligation: ObligationId) -> Result<(), ResourceError>` was added to support pre-submit and un-displaced commit cancellation per R1 and line 170.
- `BTreeSet` in std does not provide `.drain()`; `std::mem::take(&mut *self.dirty_entries.borrow_mut())` was used for clean draining of the dirty worklist in `service_ready`.
- `SpyAllocation` and its fields were declared `pub(crate)` so `AllocationPayload::Spy(tests::SpyAllocation)` can be referenced across `mod.rs` and `tests.rs`.
- `#[allow(dead_code)]` attributes were placed on vocabulary types and methods scaffolded for Tasks 2–10 to satisfy `cargo clippy --all-targets -- -D warnings`.

**Files:** Create the four Task-1 files from the map; add `pub(crate) mod resources;` to `kms/render/mod.rs`.

**Consumes:** Existing `DrmDeviceKey` and `IncarnationId`.

**Produces:**

```rust
impl ResourceService {
    pub(crate) fn new(device: DrmDeviceKey, incarnation: IncarnationId) -> Self;
    pub(crate) fn adopt(&mut self, payload: AllocationPayload)
        -> Result<AllocationLease, (ResourceError, AllocationPayload)>;
    pub(crate) fn reserve(&mut self, key: AllocationKey, usage: UseKind)
        -> Result<AllocationLease, ResourceError>;
    pub(crate) fn register(&mut self, key: AllocationKey, kind: ObligationKind)
        -> Result<ObligationId, ResourceError>;
    pub(crate) fn freeze(&mut self, key: AllocationKey) -> Result<(), ResourceError>;
    pub(crate) fn service_ready(&mut self) -> Vec<AllocationKey>;
    pub(crate) fn cancel(&mut self, key: AllocationKey, obligation: ObligationId)
        -> Result<(), ResourceError>;
    pub(crate) fn apply_validated_proof(&mut self, key: AllocationKey, obligation: ObligationId)
        -> Result<(), ResourceError>;
}
impl AllocationLease {
    pub(crate) fn key(&self) -> AllocationKey;
    pub(crate) fn use_id(&self) -> UseId;
    pub(crate) fn kind(&self) -> UseKind;
}
```

The returned `adopt` lease is `Retain`; the registry owns payload even after that lease drops. Cleanup on `service_ready` is explicit. `service_ready` returns availability transitions for waiting consumers, not a license to acquire without another serialized reserve.

- [x] **1.1 Write the first failing test and the minimal fixture.** In `resources/tests.rs`, define the fixture below. `AllocationPayload::Spy` contains the object, not just its ID.

```rust
#[derive(Debug)]
pub(crate) struct SpyAllocation { pub(crate) drops: Rc<Cell<usize>> }
impl Drop for SpyAllocation {
    fn drop(&mut self) { self.drops.set(self.drops.get() + 1); }
}
fn spy_service() -> (ResourceService, AllocationLease, Rc<Cell<usize>>) {
    let drops = Rc::new(Cell::new(0));
    let mut service = ResourceService::new(
        DrmDeviceKey { major: 226, minor: 0 }, IncarnationId::first());
    let held = service.adopt(AllocationPayload::Spy(SpyAllocation {
        drops: Rc::clone(&drops),
    })).unwrap();
    (service, held, drops)
}
```

Use this regression:

```rust
#[test]
fn c0_2ci_kms_release_does_not_complete_gpu_work() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let kms = service.register(key, ObligationKind::KmsRelease).unwrap();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();
    drop(held);
    service.apply_validated_proof(key, kms).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 0);
    assert!(matches!(service.reserve(key, UseKind::Write), Err(ResourceError::Busy)));
    service.apply_validated_proof(key, gpu).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 1);
}
```

- [x] **1.2 Run** `cargo test -p yserver --lib c0_2ci_kms_release_does_not_complete_gpu_work`. Record the missing module/API failure before implementing.
- [x] **1.3 Implement the entry state and atomic reserve.** Each entry tracks live uses, unresolved obligations, frozen state and payload. Use `BTreeMap`/`BTreeSet` for deterministic tests; a pending set empties only on matching evidence. Read/write compatibility is evaluated inside the same mutable service operation that inserts `UseId`.

```rust
fn can_destroy(entry: &AllocationEntry) -> bool {
    !entry.frozen()
        && entry.live_use_count() == 0
        && entry.pending_obligation_count() == 0
}
```

Implement the three accessors against that entry's single availability state. `Retain` prevents destruction but does not alone license access. `Write` excludes live Read/Write/Kms uses and pending GPU/read/KMS/FOREIGN work. `Read` excludes writers and requires its adapter's route-specific ownership/readiness check. `Kms` use is registered before dispatch. Its `AllocationLease` may be dropped like any other (dropping only ends the reservation and schedules service, per the vocabulary); what a CPU reference drop can never do is discharge the `KmsRelease` **obligation** registered alongside it — only correlated `PriorBufferReleased` evidence or a device barrier does that, and the entry stays rooted until then. A read of an already scanning-out image is permitted only through the read adapter's explicit route checks; do not make a generic pending-KMS prohibition that breaks successful synchronous snapshots.

- [x] **1.4 Add reverse-order, alias, stale-key, duplicate-proof and cancellation tests.** Reuse the first test with GPU before KMS; hold two `Retain` leases and verify only the second drop permits destruction; repeat the same proof without a second cleanup; allocate a new generation and deliver old evidence; drop a Write lease after registering GPU work and verify reuse stays blocked. Frozen entries remain retained after all normal proofs.
- [x] **1.5 Run** `cargo test -p yserver --lib c0_2ci_`, format, run required clippy, and commit these Task-1 files with `feat(kms): add allocation leases and availability ledger`.

## Task 2: Consuming DRM cleanup and real direct framebuffer retention

**Status: EXECUTED at `2b0f4d3c`.** **Review round 1 (2026-09-11): REJECTED** — see the findings and `docs/handoff-phase-c0-stage-2c-i-fix.md`; unchecked steps below are not done or not proven. The shown code below has been reconciled with
what compiled and passed all gate checks.

**Fix round 1: `95f9dba6`** (session F-1, `docs/handoff-phase-c0-stage-2c-i-fix.md`). Verdicts for this row's findings:

| Finding | Verdict |
| --- | --- |
| B-1 (barrier inverted R5, derived from `Rc::strong_count`) | **RESOLVED** (test: `c0_2ci_drm_cleanup_fd_family_barrier_discharges_payload_alias`, `tests.rs`) |
| B-2, registry half (no inventory to discharge from) | **RESOLVED** (same test — the registry now keeps `payload_alias_keys: BTreeSet<AllocationKey>`, populated by `register_payload_alias(key)`, and walks it in `try_mint_file_family_closed`; the *production adoption* half — wiring `ResourceService::adopt` to call `register_payload_alias` for real `ScanoutAllocation`/`FileOwnedBacking` payloads — is F-2's, per the fix handoff's task table) |
| M-17 (`apply_validated_proof` visibility) | **RESOLVED** (`mod.rs`; now `pub(in crate::kms::render::resources)` with a `#[cfg(test)]` shim, `apply_validated_proof_for_tests`, for `store.rs`'s tests) |
| M-23, `DrmCleanupRight`/`FakeFamilyInventory` scope | **RESOLVED** (`DrmCleanupRight`'s fields and `new()` are private/`pub(in resources)`; `FakeFamilyInventory` and the registry methods that drive it — `init_fake_family`, `close_fake_control`, `reap_fake_helper`, `add_fake_alias`, `remove_fake_alias` — are `#[cfg(test)]`. The rest of M-23's list — `SharedBacking`/`CopiedSourceAllocation::mock`, `Option<Arc<VkContext>>` fields, `GpuObligation.context`, `poll_signaled_result_opt`, `RoleReservation::new_for_test` — belongs to Tasks 4/5/8 and is untouched here) |
| minor `retire_closed_family` assert | **RESOLVED** (returns `Result<(), ResourceError>` instead of `assert_eq!`; both call sites in `tests.rs` now `.unwrap()`) |

Pre-fix evidence: the deleted test `c0_2ci_drm_cleanup_round3_b1_counted_alias_real_device_barrier` (`tests.rs:507-559` before this fix, preserved in the fix commit's parent `2b0f4d3c`) asserted `registry.try_mint_file_family_closed().is_err()` *while the payload's device alias was still held* — the literal inversion B-1 names. That assertion could only pass under the pre-fix `try_mint_file_family_closed`, whose `payload_aliases > 0` and `Rc::strong_count(device) > 1` checks are exactly what this fix round removes; the new API shape (`try_mint_file_family_closed` now takes a discharge callback) makes the old call sites a compile error on the fixed tree, which is the strongest form of "does not still pass" available without a parallel implementation to run both against.

**Fix round 1b: `13c75265`** (session F-1b, closing `docs/superpowers/findings/2026-09-11-stage-2c-i-fix-F1-review.md`, which reviewed fix round 1 and found 1 blocking, 2 major, 2 minor). Verdicts:

| Finding | Verdict |
| --- | --- |
| F1-B1 (blocking: production `try_mint_file_family_closed` had no preconditions — the gate was `#[cfg(test)]`-only) | **RESOLVED** — `FakeFamilyInventory` replaced with a real `FamilyInventory` (`submitters_detached`, `helper_reaped`, `control_closed`, `non_payload_aliases`) that is always present on the registry (not `#[cfg(test)]`), defaults to fully unsatisfied via `#[derive(Default)]`, and is checked unconditionally in every build before the discharge loop. Only the `#[cfg(test)]` setters can satisfy it today; test: `c0_2ci_drm_cleanup_fake_family_barrier_requires_all_closed` now exercises the same unconditional path a production caller would hit |
| F1-M1 (major: 2.5 proved step 2 of the barrier, not step 3 — `weak.upgrade().is_none()` was satisfied by the payload's own drop, not the registry's) | **RESOLVED** — `c0_2ci_drm_cleanup_fd_family_barrier_discharges_payload_alias` rebuilt on `new_with_device_and_io` so the registry also holds its own alias; the discharge closure now asserts `weak.upgrade().is_some()` immediately after the payload's own alias drops (registry's alias is still live), and only after the mint returns does `weak.upgrade().is_none()` hold. Verified this catches the reviewer's named mutant: deleting `self.device = None` from `try_mint_file_family_closed` now fails this test (confirmed by hand before restoring) |
| F1-M2 (major: no test for a discharge failure mid-walk, no test that discharge is gated by preconditions) | **RESOLVED** — new test `c0_2ci_drm_cleanup_fd_family_barrier_discharge_failure_retries` (close_gem fails: `Err`, key still in `payload_alias_keys`, right at `FramebufferRemoved`, family not closed; retry re-issues only `CloseGem` and succeeds); `c0_2ci_drm_cleanup_fake_family_barrier_requires_all_closed` now registers a real payload alias up front and keeps a panicking closure through the three failing mints, switching to a counting closure for the successful one and asserting it ran exactly once |
| F1-m1 (minor: `consume` refuses after `freeze_incarnation`, which would block barrier discharge if quarantine ever freezes the registry) | **RESOLVED in Task 9 (F-8)** — quarantine freezes service entries (via consumer), never the registry itself; tested in `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines` |
| F1-m2 (minor: the discharge closures reach into `service.entries`/`entry.payload.borrow_mut()` directly — M-18's porous seam) | **DEFERRED to Task 4 (F-2)**, added explicitly to the B-2 adoption-half deferral already recorded above: F-2 must add a service-side `discharge_file_owned_for_barrier(&mut self, registry, key)` and the tests should switch to it |

Gate re-run for this round: `cargo +nightly fmt` clean, `cargo clippy --all-targets -- -D warnings` clean, `c0_2ci` 79/79 (78 plus the new failure-retry test) on twelve runs with no flakes, and all three portable targets (`x86_64-unknown-linux-gnu`, `-musl`, `-freebsd`) check clean.

**Execution notes:**
- `CleanupIo` trait with `remove_fb(u32) -> io::Result<()>` and `close_gem(u32) -> io::Result<()>` was implemented with `DeviceCleanupIo` (real DRM ioctls via `drm::control::Device` and `drm::Device`) and `MockCleanupIo` (counting transport in tests).
- `DrmCleanupRight` carries `GemOwner::Right` or `GemOwner::Gbm` across states `Registered`, `FramebufferRemoved`, `Discharged`, and `Frozen`. `GemOwner::Gbm` never issues GEM_CLOSE in the right per R3; on retry after partial GEM-close failure, RMFB is not re-issued.
- `DirectScanoutProbeFramebuffer` inner ownership was replaced with `ProbeFbOwnership { Legacy, Managed }`, and `into_managed` consumes ownership and registers with `DrmCleanupRegistry`, eliminating the legacy destructor.
- `FileFamilyClosed` can only be minted when all external handles and counted payload aliases are detached, proving the round-3 B-1 barrier with real `Rc<Device>`. **Fix round 1 correction:** this claim was false as originally implemented — see B-1 above. The barrier is now mintable once the fake-family/control/helper conditions hold, *regardless* of outstanding payload aliases, which the registry discharges itself as part of minting rather than waiting on. **Fix round 1b correction:** "fake-family/control/helper conditions hold" was itself only checked in `#[cfg(test)]` builds — see F1-B1 above. The gate is now `FamilyInventory`, always present, checked unconditionally.
- Verified against the three required platform targets: `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, and `x86_64-unknown-freebsd`.
- Fix round 1: `try_mint_file_family_closed` now takes `discharge_payload_alias: impl FnMut(&mut DrmCleanupRegistry, AllocationKey) -> Result<(), io::Error>` — the registry owns the closing order (walks `payload_alias_keys`) but has no access to the service's payloads, so the caller supplies the per-key discharge. `DirectFramebufferAllocation::discharge_file_owned` performs it (consume the right, then drop the device alias), mirroring `ScanoutAllocation::discharge_file_owned`.
- Fix round 1b: the registry's precondition state is `FamilyInventory { submitters_detached, helper_reaped, control_closed, non_payload_aliases }`, `#[derive(Default)]` (all-false/zero), checked unconditionally at the top of `try_mint_file_family_closed`. Renamed from `FakeFamilyInventory`; the `#[cfg(test)]` setters keep their `*_fake_*` names (`detach_fake_submitters`, `close_fake_control`, `reap_fake_helper`, `add_fake_alias`, `remove_fake_alias`) since production has no real setter yet.

**Files:** Create `resources/drm_cleanup.rs`; modify `drm/modeset.rs` and `resources/mod.rs`.

**Consumes:** Task-1 allocation entries; existing `DirectScanoutProbeFramebuffer` handles.

**Produces:** `DrmCleanupRegistry`, `DrmCleanupRight`, `FileFamilyClosed`, and `DirectFramebufferAllocation`. These are core-thread types. The right contains registry identity plus FB/GEM identity, never an untracked `Rc<Device>`. `FileFamilyClosed` has a private constructor: only the registry can mint it after full closure, and Task 2 supplies fake descriptor-family closure tests; Task 9 adds helper-reap and late-reply integration. No public boolean claims closure.

```rust
impl DrmCleanupRegistry {
    pub(crate) fn consume(&mut self, right: DrmCleanupRight)
        -> Result<(), (std::io::Error, DrmCleanupRight)>;
    pub(crate) fn freeze_incarnation(&mut self);
    // Fix round 1: returns Result instead of asserting on a mismatched proof.
    pub(crate) fn retire_closed_family(&mut self, proof: FileFamilyClosed)
        -> Result<(), ResourceError>;
    // Fix round 1: no longer refuses while a payload alias is outstanding (B-1);
    // instead it discharges every registered alias through the caller-supplied
    // callback -- the registry owns the closing order, the service owns the
    // payloads -- before minting.
    pub(crate) fn register_payload_alias(&mut self, key: AllocationKey);
    // Fix round 1b: preconditions (FamilyInventory) are real fields, always
    // present and checked unconditionally, defaulting to fully unsatisfied
    // (F1-B1) -- not a #[cfg(test)]-only check as fix round 1 shipped it.
    pub(crate) fn try_mint_file_family_closed(
        &mut self,
        discharge_payload_alias: impl FnMut(&mut DrmCleanupRegistry, AllocationKey) -> Result<(), std::io::Error>,
    ) -> Result<FileFamilyClosed, std::io::Error>;
}
```

- [x] **2.1 Write a counting-transport cleanup test.** Define a module-private `CleanupIo` trait with `remove_fb(u32) -> io::Result<()>` and `close_gem(u32) -> io::Result<()>`. Its real implementation borrows the registry's original device only for the call. Its test implementation appends `RemoveFb(id)`/`CloseGem(id)` to `Rc<RefCell<Vec<CleanupCall>>>`; define that two-variant enum in the same test module. Register FB 11/GEM 12, consume its right, then service/drop all references and assert the exact log:

```rust
assert_eq!(calls.borrow().as_slice(), &[CleanupCall::RemoveFb(11), CleanupCall::CloseGem(12)]);
```

- [x] **2.2 Run** `cargo test -p yserver --lib c0_2ci_drm_cleanup` and observe missing cleanup API/test failures.
- [x] **2.3 Implement consuming cleanup stages.** Use `Registered`, `FramebufferRemoved`, `Discharged`, `Frozen` states. On RMFB success/GEM-close failure return a right at `FramebufferRemoved`, so retry cannot issue RMFB twice. **One GEM closer per payload (plan-review round-4 B-1).** The right carries a `GemOwner` discriminant: `GemOwner::Right` for Vulkan-export payloads, where the right issues `GEM_CLOSE` as today; `GemOwner::Gbm` for GBM-allocated payloads, where `PRIME_FD_TO_HANDLE` on the same description returned the gbm_bo's *existing* handle ([scanout.rs:3115](../../../crates/yserver/src/kms/vk/scanout.rs:3115)) and the right records the handle without ever closing it — the gbm_bo drop, ordered after `RMFB`, is the sole `GEM_CLOSE`. A GEM handle is not refcounted: the baseline `ScanoutBo::Drop` already closes it twice on the GBM path ([scanout.rs:3442](../../../crates/yserver/src/kms/vk/scanout.rs:3442) then the trailing `gbm_bo` drop), and a second close after handle-number reuse destroys another live buffer's handle. The `FramebufferRemoved` retry re-issues only what its `GemOwner` permits. On error retain rights and close converted admission. Complete-family closure discharges only file-owned rights and prevents later ioctls; shared Vulkan/GBM payload remains in Task-1 availability. Do not reopen the device by path. Registry-held aliases and helper aliases must be accounted for; a single closed control FD cannot mint `FileFamilyClosed`. Every `Rc<drm::Device>` held inside a registry-rooted payload context — the baseline `GbmDevice = gbm::Device<Rc<drm::Device>>` is one — is a **counted alias** of the same inherited open file description, registered at adoption, not a hidden reference (plan-review round-3 B-1).
- [ ] **2.4 Add `DirectScanoutProbeFramebuffer::into_managed`** as an ownership-consuming conversion that extracts FB/GEM and transfers original-device ownership into the registry. Preserve the legacy destructor for legacy values by replacing inner ownership with an explicit `Legacy`/`Managed` representation; moved managed values have no destructor capable of issuing old ioctls. `DirectFramebufferAllocation` retains its Task-1 source allocation leases; cache entries for managed imports become weak indices. **Fix round 1 status: still open.** `into_managed`'s `Legacy`/`Managed` split exists and is exercised (`c0_2ci_direct_probe_framebuffer_into_managed`), but it does not call `registry.register_payload_alias` for the device it moves in, and "cache entries for managed imports become weak indices" has no code anywhere — this was not in session F-1's row (B-1/B-2 registry half/M-17/M-23/minor `retire_closed_family`) and is left for whichever session wires real production adoption against the registry (F-2, per the fix handoff's dependency note: "F-2 needs F-1's registry inventory").
- [x] **2.5 rewritten (fix rounds 1 and 1b).** Cache eviction, frozen rights and partial-cleanup-failure coverage predate this fix and are unaffected (`c0_2ci_drm_cleanup_frozen_rights_and_family_closed_reject_ioctls`, `c0_2ci_drm_cleanup_partial_failure_and_retry`); the GBM/Vulkan-pending-after-file-owned-discharge case is `c0_2ci_drm_cleanup_shared_gpu_dependency_persists_after_file_rights_discharge`. What fix round 1 rewrote is the round-3 B-1 mechanism test itself, per `docs/handoff-phase-c0-stage-2c-i-fix.md`'s F-1 section: `c0_2ci_drm_cleanup_fd_family_barrier_discharges_payload_alias` adopts a real `DirectFramebufferAllocation` (device from `Device::for_tests()`) into a `ResourceService`, registers its device alias with the registry, and proves the barrier is mintable **while the payload still holds the alias** — the inverse of what the pre-fix test asserted. After minting: the payload's right and device fields are both `None`, the counting transport shows exactly one `RemoveFb`+`CloseGem` pair, `Weak::upgrade()` on the device is `None`, and no further transport call happens on `retire_closed_family`. Fix round 1b hardened this same test (F1-M1): the registry now also holds its own alias (`new_with_device_and_io`), and the discharge closure asserts `weak.upgrade().is_some()` right after the payload's own alias drops — proving the registry's own alias, not the payload's, is the description's last close (R5 step 3), which the round-1 version could not distinguish. Fix round 1b also added `c0_2ci_drm_cleanup_fd_family_barrier_discharge_failure_retries` (a discharge failure mid-walk leaves the key registered and the right retryable) and reworked `c0_2ci_drm_cleanup_fake_family_barrier_requires_all_closed` to register a real payload alias and count its exactly-once discharge (F1-M2). The real-`GbmDevice`-over-a-render-node `_drm` variant of this mechanism (`Device::for_tests()`'s Unix socket cannot back a `GbmDevice`, per finding B-3) is explicitly deferred to Task 9.5/F-8, as the original plan text already anticipated ("9.5 later proves the same payload survives handoff").
- [x] **2.6 Run** the focused tests, the three target checks from Task 10 because this task changes DRM cleanup typing, formatting and required clippy. Commit only these files with `feat(kms): make managed framebuffer cleanup proof gated`.

## Task 3: Storage generations, layout and promotion

**Files:** Create `resources/storage.rs`; modify `store.rs`, `engine.rs`, `target.rs` and the resources module.

**Consumes:** Rooted entries and explicit cleanup rights. Keep actual Vulkan allocation ownership in this adapter, not in a new numeric pin table.

**Produces:** `StorageAllocation`, `PixelIdentity`, `StorageLease`, `StorageAccessError` and managed adoption/access methods. `StorageAccessError` wraps `ResourceError` or a Vulkan failure; callers propagate failure before mutating logical layout.

```rust
pub(crate) struct PixelIdentity {
    pub target: crate::kms::render::target::PaintTarget,
    pub allocation: AllocationKey,
    pub content_offset: (i32, i32),
    pub extent: ash::vk::Extent2D,
}
pub(crate) struct StorageLease {
    pub allocation: AllocationLease,
    pub pixels: PixelIdentity,
}
```

- [x] **3.1 Add a real-storage retirement test.** In the existing store test module, extend `decref_then_realloc_then_retire_keeps_new_xid_mapping` to retain a managed allocation from the old drawable before reallocation. Keep its original offset/depth in `PixelIdentity`. Assert old destruction is delayed, new lookup still selects the new ID, and old cleanup does not reset the new drawable's content/damage state. Use existing null-storage tests for logical ordering and a live Vulkan version in Task 10 for actual image/view lifetime.
- [x] **3.2 Run** `cargo test -p yserver --lib c0_2ci_storage` before adding the adapter.
- [ ] **3.3 Extract physical fields into `StorageAllocation`.** Move image/memory/views, format/depth/extent/current Vulkan layout, imported owner/metadata and promoted-export metadata listed in the inventory. Keep drawable identity, scene damage, dormancy and current selection in `DrawableStore`. Provide `Storage::into_managed(self, service: &mut ResourceService, target: PaintTarget, content_offset: (i32, i32)) -> Result<StorageLease, (ResourceError, Storage)>` at the owning boundary; on failure reconstruct the original storage and return it. A live logical drawable retains its own allocation lease when its backing is managed; do not consume the drawable's only reference to create an intent. Introduce `StorageBacking { Legacy(StorageAllocation), Managed(StorageLease) }` beneath the store's logical facade, updating its field accessors and all affected engine consumers in this task. The payload contains `StorageAllocation`, never that facade, so there is no recursive ownership. Adoption captures extent from the allocation and target/offset from the resolved drawable, then assigns the new allocation key. `ResourceService::retain_storage(&mut self, source: &StorageLease) -> Result<StorageLease, ResourceError>` creates another Retain use with the same captured identity; no implicit `Clone` issues a new usage.

For managed storage, `destroy_now`, `poll_pending_retire` and `shutdown_destroy_all` detach logical references and submit invalidation/cleanup work. The service retains actual image owners and their contexts. Invalidation must occur before image cleanup; its job retains the necessary cache entries, not a closure capturing `&mut KmsBackend`. Imported image aliases remain single-owned; sample view is destroyed before dropping the imported owner. Preserve the legacy storage constructor and cleanup route until a producer adopts managed storage explicitly.

- [x] **3.4 Implement scoped access and relayout exclusion.** Define `ResourceService::with_storage_read<T>(&mut self, lease: &StorageLease, f: impl FnOnce(&StorageAllocation) -> T) -> Result<T, ResourceError>` and the corresponding `with_storage_write<T>` with a `&mut StorageAllocation` closure. Write access first reserves compatible usage and cannot escape a raw reference. GPU work records an obligation before ending the CPU access scope. Preserve full `PaintTarget`, including `x11_depth`, when capturing deferred work.

```rust
let pixels = &lease.pixels;
assert_eq!(pixels.target.x11_depth(), 24);
assert_eq!(pixels.allocation, lease.allocation.key());
```

Use this assertion in the depth-24 target/depth-32 backing regression. For border relayout, first attempt exclusive layout/write reservation. If busy, take the existing separately allocated copy path within pool limits; if no capacity, defer layout publication and request the existing scene retry. Never change `content_offset` before moving pixels, and never bump a generation to justify overwriting held storage.

- [ ] **3.5 Convert promotion retirement.** `adopt_exportable` publishes a new allocation generation. Old `RetiredImage` becomes a retained payload guarded by every old usage plus the existing render ticket. `retire_image_after` and `destroy_retired_image` feed the service for managed payloads. Returning ordinary storage to `PixmapPool` is authorized only after all uses/tickets; promoted/imported storage remains pool-ineligible. Pool checkout establishes a new generation.
- [x] **3.5a Preserve upstream DRI3 buffer identity.** Extend the retained imported owner with the original `ImageBacking::Imported::dma_buf_fd`, `DrawableImage::drm_modifier`, `import_plane0`, `import_size`, and `ImportedDmabufMetadata::implicit_layout`. Preserve `Dri3ImportModifier::Implicit` versus `Explicit(m)` through import; do not reinterpret the server's guessed Vulkan view as a verified client layout. Imported re-export duplicates the original client FD with its original stride/offset and stated legacy size; do not replace this with `vkGetMemoryFdKHR` or `lseek`. Implicit export reports `DRM_FORMAT_MOD_INVALID`; explicit export retains its supplied modifier. Copying or moving a lease cannot relabel the metadata. Retain the window modifier list's `drmFormatModifierPlaneCount == 1` constraint; multi-plane import is still out of scope.
- [x] **3.5b Add DRI3 lease regressions.** Extend upstream's imported-buffer metadata/export test across managed adoption, logical FreePixmap and deferred retirement; use a distinguishable client size so a Vulkan-derived substitute fails. Assert no client FD offset change, explicit/implicit modifier preservation and once-only FD ownership. No-ICD or export-not-supported conditions are reported as such; do not hide arbitrary fixture failure as a successful test.

- [x] **3.6 Test** in-place relayout exclusion, allocate-and-copy retaining both allocations, promotion with an old read/KMS lease, new XID preservation, depth semantics and no premature pool return. Run `cargo test -p yserver --lib c0_2ci_storage`, existing border-width/promotion tests, format and required clippy. Commit with `feat(kms): retain storage generations across deferred use`.

**Status: EXECUTED at f43c64ed.** **Review round 1 (2026-09-11): REJECTED** — see the findings and `docs/handoff-phase-c0-stage-2c-i-fix.md`; unchecked steps below are not done or not proven. StorageAllocation extracted with PixelIdentity and StorageLease; StorageBacking (Legacy/Managed) implemented with backwards-compatible facade; ResourceService retain/with_storage_read/with_storage_write implemented with usage safety checks; promotion and layout transition handling integrated; DRI3 import metadata and modifier constraints preserved; 7 unit tests covering retirement, XID preservation, relayout exclusion, promotion with lease, depth semantics, and pool return constraints passing.

**Fix round 1: `12c5926c`.** Session F-3, closing B-14, M-18, M-19, M-20, M-21 (`docs/superpowers/findings/2026-09-11-stage-2c-i-implementation-review-round1.md`) and F2b-m1 (`docs/superpowers/findings/2026-09-12-stage-2c-i-fix-F2-review.md`), per `docs/handoff-phase-c0-stage-2c-i-fix.md`'s "F-3 — Task 3" section.

| Finding | Verdict |
| --- | --- |
| B-14 (managed storage adopted from production leaks its Vulkan handles) | **RESOLVED (test: `c0_2ci_storage_into_managed_refuses_non_stub_without_vk_context`, `c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan`)** — `Storage::into_managed` now takes `platform: &PlatformBackend`, refuses a non-stub allocation when `platform.vk` is `None` (`ResourceError::InvalidState`), and otherwise backfills `alloc.vk`/`alloc.pixmap_pool` from it before adopting. All 8 existing `into_managed` call sites (stub fixtures) updated to pass a `platform`. Mutation check: reverting `into_managed` to its pre-fix 3-argument form (no backfill) is exactly what the first test's `Err` assertion would no longer see — the non-stub, null-handle fixture would be silently adopted instead of refused |
| M-18 (lease abstraction porous: `AllocationLease.entry`/`AllocationEntry.payload`/`AllocationLease::new` were `pub(crate)`; `store.rs`'s `is_exportable`/`record_layout_transition` read/mutated the payload directly under a Retain lease, bypassing reservation) | **RESOLVED (test: `c0_2ci_storage_is_exportable_managed_reserves_read_and_refuses_when_written`, `c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan`)** — `entry`/`payload`/`new` are now `pub(in crate::kms::render::resources)`, which by itself forced `store.rs` off the direct-borrow path (it no longer compiles from outside `resources`). `Storage::is_exportable`/`Drawable::record_layout_transition` stay Legacy-only (panic on `Managed`, matching the already-accepted `adopt_exportable`/`adopt_exportable_managed` split — no production or test caller reaches Managed storage through either today, R8); `is_exportable_managed`/`record_layout_transition_managed` are the Managed-safe counterparts, routing through `with_storage_read`/`with_storage_write` so an incompatible live use is refused (`Busy`) instead of silently raced. Mutation check: reverting either `_managed` method to a direct `.entry.payload.borrow()/borrow_mut()` no longer compiles (the field is private outside `resources`), and prior to this fix there was no `Result` to return `Busy` from at all — the old `is_exportable` returned a bare `bool` |
| M-19 (panicking `Deref`/`DerefMut` facade; ~202 accessor call sites) | **NOT RESOLVED — F8 stop, see below.** Recounted per the dispatch's instruction: 180 external `.storage.` accessor sites today (`backend.rs` 91, `engine.rs` 70, `scene.rs` 17, `frame_builder.rs` 1, `target.rs` 1, `ops/render.rs` 0; `store.rs`'s own 7 are the type's own definition, not external consumers) — down from the review's 202, not up; the v1.5.1 merge did not add net accessor sites in the sampled files. The `Deref`/`DerefMut` panic-on-`Managed` is unchanged from the pre-fix tree (not shipped new by this session) and is unreachable in production/tests today (no producer creates Managed storage outside `into_managed`'s own test call sites, R8). 3.3 stays unticked |
| M-20 (retirement seams not converted: `destroy_now`/`poll_pending_retire`/`shutdown_destroy_all` via `Storage::destroy`'s Managed arm was `{}`; `retire_image_after`/`destroy_retired_image` Legacy-only) | **SPLIT.** `Storage::destroy`'s Managed arm — **RESOLVED (test: `c0_2ci_storage_managed_destroy_detaches_before_drop`, `c0_2ci_storage_managed_drawable_decref_reclaims_via_service`)**: it now overwrites `self.backing` with an inert Legacy stub, dropping the Managed lease (and releasing its Retain use) at the call, not merely whenever the caller later drops the whole `Storage`. The first test proves the timing distinction directly (calls `destroy()`, then `service_ready()`, while the `Storage` value is still alive); the second proves it through the real `DrawableStore::decref → destroy_now` seam. `retire_image_after`/`destroy_retired_image` (the promotion-specific half) — **DEFERRED TO the M-19 follow-up**: both only ever see the Legacy `RetiredImage`, because `RenderEngine::promote_drawable_exportable` (their only caller) has no Managed-aware branch at all — every field it touches (`d.storage.is_exportable()`, `.extent`, `.image`, `.format`, `Storage::adopt_exportable`) goes through the same panicking facade M-19 reports. Building the Managed promotion-retirement adapter first requires converting `promote_drawable_exportable` itself, which is squarely inside M-19's blast radius, not separable from it |
| M-21 (`c0_2ci_storage_dri3_lease_regressions` and `c0_2ci_storage_no_premature_pool_return` proved nothing real) | **RESOLVED (test: `c0_2ci_storage_dri3_lease_regressions_vulkan`, `c0_2ci_storage_no_premature_pool_return_vulkan`)** — the new DRI3 test drives a real dma-buf import/export round trip (via `dri3::export_backing`/`import_dmabuf_reporting`/`export_dmabuf`, mirroring `backend.rs`'s `dri3_imported_pixmap_exports_the_clients_own_description`) through managed adoption, asserts `DRM_FORMAT_MOD_INVALID` + the distinguishable client-stated size for the implicit case and the real modifier + measured size for the explicit case, a zero fd-offset round trip, once-only FD ownership (each case dups its own fd), and a logical FreePixmap (`DrawableStore::decref`) whose retirement is genuinely deferred by an outstanding KMS obligation and only completes once that obligation discharges. The pool-return test now wires a real `PixmapPool` and asserts `PixmapPoolStats::total_returns_accepted == 0` for a promoted allocation, instead of only "doesn't panic" |
| F2b-m1 (`register_managed_scanout_bo`'s Exhausted rollback path had no end-to-end test) | **RESOLVED (test: extended `c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan`)** — added `#[cfg(test)] ResourceService::force_exhausted_for_tests()` (`resources/mod.rs`); the extended test pushes a second bo, forces exhaustion, asserts `Err(Exhausted)`, the second bo's `fb_handle`/`gem_handle` untouched, `managed_key() == None`, `DrmCleanupRegistry::payload_aliases()` unchanged and no new ioctl. (Touches `resources/adapter_tests.rs`/`resources/mod.rs`, nominally Task 4/10 files, per the F2-review's explicit assignment of this finding to F-3.) |

**F8 stop — M-19 split proposal.** The recount stands at 180 sites across three files whose combined size (`backend.rs` 44455 lines, `engine.rs` 18321 lines, `scene.rs` 15662 lines) makes a single-session conversion unreviewable — the same "plan size drives defect density" pattern already measured on this stage (14 tasks→2 blocking, 21→24, 23→26). Proposed split, in dependency order (each session ends with the full gate green and its own fold-back):

1. **Read-mostly consumers** — `target.rs` (1) + `frame_builder.rs` (1) + `scene.rs` (17): smallest surface, lowest risk, and the right place to settle the accessor shape (named methods on `Storage` mirroring `extent()`/`depth()`/`content_offset()`, each dispatching Legacy directly and Managed through `with_storage_read`/`with_storage_write`, taking a `&mut ResourceService` where a caller needs one) before spending it across the two large files.
2. **`RenderEngine` (`engine.rs`, 70 sites)** — the paint/copy/promote/readback operations, including `promote_drawable_exportable` (which also closes M-20's deferred `retire_image_after`/`destroy_retired_image` half, since that adapter cannot exist before this file is Managed-aware).
3. **`KmsBackend` (`backend.rs`, 91 sites)** — the largest file; likely needs its own internal split by feature area (DRI3 import/export, GLX-TFP promotion, core paint entry points, cursor) once its own accessor inventory is taken, rather than being treated as one session.

Each session converts its own files' call sites to the accessor shape settled in session 1, ticks nothing on 3.3 until all three land, and reports its own recount so the next session starts from a verified number rather than trusting this one.

Gate for this round: `cargo +nightly fmt` clean; `cargo clippy --all-targets -- -D warnings` clean; `c0_2ci` 89 passed/0 failed/7 ignored (85 baseline passed + 4 new deterministic; 3 baseline ignored + 4 new `_vulkan`) on twelve consecutive runs, zero flakes; full `cargo test -p yserver --lib` 1626/1626 on a clean run (one run hit the pre-existing R2 flake in `kms::executor::device_lock::tests::an_inheritable_lock_still_holds_and_still_releases_on_last_close`, confirmed unrelated: this session touches no file under `kms/executor/`, and the same test passes standalone every time). Hardware run (`--ignored`, this box has a real DRM node and NVIDIA/RADV ICDs):

```
$ cargo test -p yserver --lib c0_2ci -- --ignored --nocapture
running 7 tests
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_no_premature_pool_return_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_dri3_lease_regressions_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1698 filtered out; finished in 0.52s
```

All four `_vulkan` tests this session added are new; the other three (`c0_2ci_fd_family_barrier_real_gbm_payload_drm`, `c0_2ci_live_lifetime_adapters_vulkan`, `c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan`) predate this session and are unaffected by it except the last, which this session extended for F2b-m1.

**Fix round 2: `36ab74a7`.** Session F-3b, closing F3-B1 (`docs/superpowers/findings/2026-09-12-stage-2c-i-fix-F3-review.md`), per that finding's "What F-3b must do" list.

| Finding | Verdict |
| --- | --- |
| F3-B1 (four new `_vulkan` tests in `store.rs` reported an environmental skip as a pass: `Err(e) => { eprintln!("skip: no Vk: {e}"); return; }` on a failed `VkContext::new()`, five occurrences total — `c0_2ci_storage_no_premature_pool_return_vulkan`, `c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan`, `c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan` each had one such arm, and `c0_2ci_storage_dri3_lease_regressions_vulkan` had two — one on `VkContext::new()`, one on `allocate_exportable`'s lavapipe/`FORMAT_NOT_SUPPORTED` case) | **RESOLVED** — all five arms now `panic!("environmental skip: no live Vulkan ICD available; not claiming pass")`, matching the shape already used by `c0_2ci_live_lifetime_adapters_vulkan` (`resources/adapter_tests.rs`). Nothing else in `store.rs` was touched |

Gate for this round: `cargo +nightly fmt` clean; `cargo clippy --all-targets -- -D warnings` clean; `c0_2ci` 89 passed/0 failed/7 ignored on twelve consecutive runs, zero flakes. Hardware run (`--ignored`, this box has a real DRM node and NVIDIA/RADV ICDs):

```
$ cargo test -p yserver --lib c0_2ci -- --ignored
running 7 tests
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::store::tests::c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_no_premature_pool_return_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_dri3_lease_regressions_vulkan ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1698 filtered out; finished in 0.51s
```

No steps ticked or unticked by this round; F3-B1 was procedural only.

## Task 4: Shared/copied scanout backing and pool reuse

**Files:** Create `resources/scanout.rs`; modify `kms/vk/scanout.rs`, `kms/render/platform.rs` and the resource payload enum.

**Consumes:** Tasks 1–3; existing `ScanoutBo`, `OutputScanout`, `CopiedRenderSource` and copied ownership state machines.

**Produces:** `ScanoutAllocation`, `CopiedSourceAllocation`, `ManagedScanoutToken`. Extract backing fields at their defining module boundary to preserve private invariants; do not make every BO field public.

**Physical ownership of one scanout allocation (plan-review round-4 B-1/M-1).** Every managed shared/copied payload is two halves with separately tracked dispositions, so the teardown barrier can discharge one without touching the other:

```rust
pub(crate) struct ScanoutAllocation {
    /// Owned exclusively by the DRM open file description. Discharged by the
    /// Task-2 right on ordinary release, or by the Task-9 barrier discharge.
    pub(crate) file_owned: Option<FileOwnedBacking>,
    /// Independent of the description; released only by GPU/read/FOREIGN proofs.
    pub(crate) shared: SharedBacking,
}
pub(crate) struct FileOwnedBacking {
    right: DrmCleanupRight,                 // FB id + GemOwner
    gbm_bo: Option<gbm::BufferObject<()>>,  // sole GEM closer when GemOwner::Gbm
    device: Rc<crate::drm::Device>,         // counted alias of the description
}
impl FileOwnedBacking {
    /// The only constructor. `GemOwner::Gbm` requires `Some(gbm_bo)` and
    /// `GemOwner::Right` requires `None`; any other pairing is rejected here,
    /// so two closers of one handle cannot be assembled by mistake.
    pub(crate) fn new(right: DrmCleanupRight, gbm_bo: Option<gbm::BufferObject<()>>,
        device: Rc<crate::drm::Device>) -> Result<Self, ResourceError>;
}
pub(crate) struct SharedBacking {
    image: ash::vk::Image, memory: ash::vk::DeviceMemory, view: ash::vk::ImageView,
    transfer: TransferResources, vk: Arc<VkContext>,
    dmabuf: Option<OwnedFd>,
}
```

| Kernel/userspace object | Sole closer | Ordered after |
| --- | --- | --- |
| DRM framebuffer | `right` (`RMFB`) | KMS obligation discharged or superseded |
| GEM handle, GBM-allocated | `gbm_bo` drop | `RMFB` |
| GEM handle, Vulkan-export | `right` (`GEM_CLOSE`) | `RMFB` |
| `Rc<drm::Device>` alias | `FileOwnedBacking` drop, or the barrier discharge | `gbm_bo` drop |
| `VkImage`/`VkDeviceMemory`/view | `SharedBacking` drop | every GPU/read ticket and FOREIGN return |
| dma-buf fd | `SharedBacking` drop | same as above |

The availability entry keeps one payload but records the file-owned pair (KMS + file-owned) and the shared triple (GPU/read/FOREIGN) as distinct dispositions; `file_owned == None` after discharge is a legal, fully described state, not a half-destroyed entry. The Vulkan import holds its own dma-buf reference, so the shared half may outlive the gbm_bo; because the baseline's field order assumed the opposite ([scanout.rs:552](../../../crates/yserver/src/kms/vk/scanout.rs:552)), Task 10.2's live-Vulkan smoke must run gbm_bo-before-`VkImage` destruction under validation layers rather than let fixtures assert driver behavior.

```rust
pub(crate) struct ManagedScanoutToken {
    pub display: AllocationLease,
    pub renderer: Option<AllocationLease>,
    pub output: crate::kms::render::platform::CrtcKey,
    pub topology_generation: u64,
    pub last_present_generation: u64,
    pub content_invalidated: bool,
}
```

`CrtcKey` is the existing stable CRTC key defined in `kms/render/platform.rs`; retain that definition. Add `PlatformBackend::acquire_managed_scanout_bo(&mut self, service: &mut ResourceService, output: CrtcKey) -> Result<ManagedScanoutToken, ResourceError>`. The managed token is non-Copy. Legacy `ScanoutBoToken` remains on legacy submission paths until conversion.

- [ ] **4.1 Add two pool tests.** With a concrete shared BO fixture, hold a read obligation after canonical KMS retirement and assert acquisition returns `Busy`; release the read and assert exactly that generation becomes eligible. With a concrete copied pair, retire the display BO while a renderer/sink dependency remains and assert neither conflicting destination write nor renderer transport reuse is allowed. Use the existing scanout test constructors; introduce injectable completion sources only, not replacement integer BOs.
Use this assertion body after the concrete shared fixture has supplied `service`, its pool-owned Retain lease `held` and a Read reservation `reader`. The pool's Retain lease keeps storage available for a subsequent acquisition instead of destroying it between assertions:

```rust
let key = held.key();
let kms = service.register(key, ObligationKind::KmsRelease).unwrap();
let read = service.register(key, ObligationKind::Read).unwrap();
service.apply_validated_proof(key, kms).unwrap();
service.service_ready();
assert!(matches!(service.reserve(key, UseKind::Write), Err(ResourceError::Busy)));
service.apply_validated_proof(key, read).unwrap();
drop(reader);
service.service_ready();
let acquired = service.reserve(key, UseKind::Write).unwrap();
assert_eq!(acquired.key(), held.key());
```

Repeat with read-before-KMS and keep the write blocked until KMS proof. The concrete copied fixture adds its FOREIGN/sink obligations; do not obtain availability merely by altering `BoPhase`.

- [x] **4.2 Run** `cargo test -p yserver --lib c0_2ci_scanout` and confirm missing managed acquisition behavior.
- [x] **4.3 rewritten (fix round 1).** Physical fields move via a consuming extraction (`take_physical_backing`) rather than duplication: `ScanoutAllocation`'s `file_owned`/`shared` and `CopiedSourceAllocation` are built from the *same* handles the legacy `ScanoutBo`/`CopiedRenderSource` held, which are then emptied. The retained `Rc<GbmDevice>`'s device alias is registered with the registry at adoption (`adopt_with_registry`, B-2). `CopiedSourceOwnership`/semaphore-reuse state moves with the extraction (`ownership`, `completion_semaphore_reuse` fields), not duplicated. `CopiedDestinationOwnership` (the sink/display-side state machine) is unaffected — it lives on the legacy `ScanoutBo`/pool side, which this task's managed conversion does not touch.
- [ ] **4.4 Implement all-or-nothing pair acquisition.** Reserve all required entries before returning a token. On the second reservation's failure, release the first reservation without clearing its GPU obligations. A `BoPhase::Free` check alone is insufficient. Validate stable output key/topology generation before touching indices. **Fix round 1 note:** the reserve/rollback shape here was already correct pre-fix and is not named in any F-2 finding; the `BoPhase::Free`-alone insufficiency this step warns about is B-13's, and is fixed there (`acquire_managed_scanout_bo` now transitions the phase). Left unchecked because "validate stable output key/topology generation before touching indices" still has no code — `acquire_managed_scanout_bo` reads `bo.managed_key` and `topology_gen` but never compares them.

```rust
let display = service.reserve(display_key, UseKind::Write)?;
let renderer = match renderer_key {
    Some(key) if key != display_key => match service.reserve(key, UseKind::Write) {
        Ok(lease) => Some(lease),
        Err(error) => { drop(display); return Err(error); }
    },
    _ => None,
};
```

The `display_key` and optional `renderer_key` are looked up from the managed pool's stable-generation mapping before this block. Shared aliases do not reserve an exclusive write twice. No spill allocation is introduced.

- [ ] **4.5 Adapt managed cancellation, replacement and cleanup.** `cancel_scanout_bo_recording` ends recording but not in-flight work. `note_kms_retired` supplies the copied destination's matching replacement evidence; `release_completed_source` must also prove the sink/renderer semaphore dependencies before consuming them. Preserve `ReleasedButAtomicRejected`; ioctl rejection is not ownership return. `reset_scanout_bos_for_suspend`, `drain_scanout_pool_at` and installed-pool replacement detach managed entries. Managed Drop cannot issue legacy RMFB, reset active fences to Free or destroy transfer resources. **Fix round 1: partial.** "`cancel_scanout_bo_recording` ends recording but not in-flight work" is proven for real (`c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan`: a registered GPU obligation survives the call). "Managed Drop cannot issue legacy RMFB..." holds by construction (`take_physical_backing` leaves a husk with nothing left to issue). `note_kms_retired`, `release_completed_source`, `ReleasedButAtomicRejected`, `reset_scanout_bos_for_suspend`, `drain_scanout_pool_at` are untouched — none were named in F-2's row (B-13/B-2 service half/B-12/M-22/M-23); left for whichever session wires managed replacement/suspend paths.
- [x] **4.6 rewritten (fix rounds 1).** Copied rejection after external release and partial grouped replacement predate this fix and are unaffected (`c0_2ci_scanout_copied_pair_sink_dependency_gates_reuse`, `c0_2ci_scanout_partial_grouped_replacement_leaves_shared_source_retained`). What this fix round closed is B-12 exactly as scoped to F-2: exactly one `CloseGem` across right discharge plus payload destruction for **both** `GemOwner` variants (`Right`'s `FramebufferRemoved` retry path was already covered in F-1b; `Gbm`'s zero-`CloseGem`-plus-real-gbm_bo-drop case is new, `c0_2ci_fd_family_barrier_real_gbm_payload_drm`), and that discharging `file_owned` leaves `shared` intact with its GPU ticket still pending (`c0_2ci_scanout_discharging_file_owned_leaves_shared_intact`, rewritten from a vacuous null-image assertion). "Alias acquisition" and "cancellation after GPU dispatch" are covered by the B-13 hardware test. Format and clippy clean; committed as `fix(kms): retain scanout physical ownership in managed payloads` (a `fix`, not the original `feat`, since this closes a rejected implementation's findings rather than adding new behavior).

**Status: EXECUTED at c56c78a3.** **Review round 1 (2026-09-11): REJECTED** — see the findings and `docs/handoff-phase-c0-stage-2c-i-fix.md`; unchecked steps below are not done or not proven. ScanoutAllocation defined with FileOwnedBacking and SharedBacking; CopiedSourceAllocation defined; ManagedScanoutToken created and PlatformBackend::acquire_managed_scanout_bo implemented with all-or-nothing pair acquisition; pool reuse gated by obligations/read leases; exactly one CloseGem verified across right discharge and payload destruction for GemOwner variants including retry; discharging file_owned leaves shared intact; 11 c0_2ci_scanout tests passing.

**Fix round 1: `fea5c043`** (session F-2, `docs/handoff-phase-c0-stage-2c-i-fix.md`), also closing F1b-m1 carried over from the F-1b review. Verdicts:

| Finding | Verdict |
| --- | --- |
| B-13 (`ScanoutBo`/`CopiedRenderSource` kept every field, legacy `Drop` intact — the two-closers shape; `acquire_managed_scanout_bo` left `BoPhase` untouched) | **RESOLVED** — `ScanoutBo`/`CopiedRenderSource::take_physical_backing` (`kms/vk/scanout.rs`) consume every physical field into a `ScanoutBoBacking`/`CopiedRenderSourceBacking`, leaving an emptied husk whose `Drop` is a no-op; `ScanoutAllocation::from_scanout_bo_backing`/`CopiedSourceAllocation::from_copied_render_source_backing` (`resources/scanout.rs`) mint the right and assemble the real payload; `register_managed_scanout_bo` (`platform.rs`) performs this for real instead of tagging a still-owning bo; `acquire_managed_scanout_bo` now calls `transition_to_recording()` on the winning bo before returning the token. Test: `c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan` (`adapter_tests.rs`, hardware-gated — `ScanoutBo.vk` is `Arc<VkContext>`, not `Option`) |
| B-2, service half (no adoption-side alias registration; `service_ready`/`apply_teardown_release` could drop or release a live file-owned right undischarged) | **RESOLVED** — `ResourceService::adopt_with_registry` (additive; `adopt`'s own signature and Tasks 1-3's call sites are untouched) registers the alias for a `Scanout` payload with `file_owned: Some`; `service_ready_with_registry` (additive alongside `service_ready`) discharges `file_owned` through the registry before dropping a destroyable `Scanout` entry, re-marking it dirty on discharge failure rather than a bare `let _ =` (F6); `apply_teardown_release` now refuses (`InvalidProof`) any entry whose `file_owned` is still `Some` (M-10). Tests: `c0_2ci_scanout_adopt_with_registry_registers_file_owned_alias_only`, `c0_2ci_scanout_service_ready_with_registry_discharges_before_destroy`, `c0_2ci_scanout_apply_teardown_release_refuses_live_file_owned` |
| B-12 (4.6 didn't test what it claimed; no `GemOwner::Gbm` `FileOwnedBacking` existed anywhere) | **RESOLVED** — `c0_2ci_fd_family_barrier_real_gbm_payload_drm` (real DRM render node + real `gbm_bo`) asserts zero `CloseGem` on the transport for `GemOwner::Gbm`; the `Right`-variant `FramebufferRemoved` retry path was already covered pre-fix (`c0_2ci_drm_cleanup_fd_family_barrier_discharge_failure_retries`, F-1b) and is unaffected. The four vacuous tests are replaced: `discharging_file_owned_leaves_shared_intact` now adopts into a real `ResourceService` and proves a pending GPU obligation on `shared` survives the discharge; `acquire_managed_all_or_nothing` and `cancel_recording_leaves_gpu_work_armed` are renamed to describe what they actually test (the `ResourceService`-level mechanism), with the platform functions themselves now exercised for real in the B-13 test above; `topology_reuse_of_bo_index`'s vacuous assertion (an always-empty test pool) is replaced by a real `managed_key` clear in that same test |
| 9.5, real-`GbmDevice` case (round-3 B-1; did not exist — B-3) | **RESOLVED**, moved here per the fix handoff ("9.5's real-GBM case lives here"). `c0_2ci_fd_family_barrier_real_gbm_payload_drm` opens a real DRM render node (`Device::for_tests()`'s Unix socket cannot back a `GbmDevice`), builds a real `GbmDevice`/`gbm_bo`, and proves the barrier is mintable while the payload holds its alias; the registry holds no separate device alias here (`new_with_io`), so the payload's clone is the only other holder and the gbm-before-device drop order is load-bearing, not merely asserted after the fact — a wrong order would touch the fd after close. `#[ignore = "requires a real DRM render node; run explicitly"]`; run on this box, see output below |
| M-22 (`FileOwnedBacking::discharge` dropped device before gbm_bo; `CopiedSourceAllocation::Drop` never destroyed transfer resources) | **RESOLVED** — explicit `drop(gbm_bo); drop(device);` in that order; `CopiedSourceAllocation::Drop` now also destroys transfer resources (guarded on a non-null `command_pool`, since an emptied husk or mock may legitimately have none) |
| M-23, Task-4 half (`SharedBacking::mock`/`CopiedSourceAllocation::mock` reachable outside tests; `vk`/`render_vk`/`sink_vk` as `Arc<VkContext>`) | **PARTIAL — stop and report.** `mock()` on both types is now `#[cfg(test)]`-only, so a production build can never construct a null-context entry (the more serious half of the finding: production could not previously distinguish "no test escape hatch exists" from "one exists but nothing calls it"). **Not done:** `vk`/`render_vk`/`sink_vk` stay `Option<Arc<VkContext>>`. There is no `VkContext` test fixture — `VkContext::new()` requires a live Vulkan ICD — so a deterministic `mock()` cannot supply a real one without either an unsound fabricated context or turning every `c0_2ci_scanout_*` test hardware-gated, which R12 and this stage's whole deterministic-test shape argue against. F8: this is a seam the finding's literal text assumes doesn't exist; flagging rather than working around it |
| F1b-m1 (fake-family gating test proved each precondition only in combination with the others already unsatisfied) | **RESOLVED** — `c0_2ci_drm_cleanup_fake_family_barrier_requires_all_closed` now has a from-fresh, one-condition-unsatisfied-at-a-time loop; verified it catches deleting the `submitters_detached` check (the mutant fails at the panicking closure, confirmed by hand, then reverted) |

Gate for this round: `cargo +nightly fmt` clean, `cargo clippy --all-targets -- -D warnings` clean, `c0_2ci` 81/81 (78 baseline + 3 new deterministic) on twelve runs with no flakes, all three portable targets check clean. Hardware runs (this box has a real DRM node and NVIDIA/RADV ICDs):

```
$ cargo test -p yserver --lib -- --ignored --nocapture
...
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
...
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
...
test result: ok. 75 passed; 0 failed; 0 ignored; 0 measured; 1618 filtered out; finished in 23.18s
```

(`c0_2ci_fd_family_barrier_real_gbm_payload_drm` also re-run standalone 5 times with no flakes.)

**Re-baseline verification (2026-09-12), no new commit needed.** Re-dispatched
against baseline `3183d8ca` (v1.5.1 merge: unredirect-restore #142, XI2 press
#141). `git diff 08cb7b91 3183d8ca -- <every file F-2 touches>` is empty — the
merge changed none of them, so `fea5c043`/`08cb7b91` are byte-identical on the
new baseline; there is nothing to redo or re-fold. Full gate re-run
unchanged: `cargo +nightly fmt --check` clean, clippy clean, `c0_2ci` 81/81 on
twelve runs with no flakes, all three portable targets clean. Hardware
re-run:

```
$ cargo test -p yserver --lib c0_2ci_fd_family_barrier_real_gbm_payload_drm -- --ignored --nocapture
running 1 test
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1692 filtered out; finished in 0.02s

$ cargo test -p yserver --lib -- --ignored --nocapture   (all hardware tests)
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test result: ok. 75 passed; 0 failed; 0 ignored; 0 measured; 1618 filtered out; finished in 23.21s
```

M-19's stale call-site count (memory note for F-3: #142 added more
`d.storage.extent`/`.image_view` accessor sites through the `Storage` `Deref`)
does not touch anything F-2 built or fixed. No code change; this entry
records the confirmation.

**Fix round 2: `05555b03`** (session F-2b, `docs/superpowers/findings/2026-09-12-stage-2c-i-fix-F2-review.md`, which reviewed fix round 1 and found 3 blocking, 3 major, 2 minor). Verdicts:

| Finding | Verdict |
| --- | --- |
| F2-B1 (blocking: a registered managed bo had no root; the next tick discharged it) | **RESOLVED** — `ScanoutBo`/`CopiedRenderSource` now store `managed: Option<AllocationLease>` (the lease itself, not a bare `AllocationKey`); `managed_key()` derives the key, `set_managed`/`take_managed` replace the old key-only setter, and `detach_managed_entries` drops the lease (the actual release) instead of clearing a field. Test: extended `c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan` with a tick right after registration (entry survives, no ioctl) and a tick right after `detach_managed_entries` (now discharges for real). Mutation-verified: reverting to a bare `drop(display_lease)` with no root fails the post-registration assertion |
| F2-B2 (blocking: `service_ready_with_registry` never unregistered the alias on normal release) | **RESOLVED** — unregisters on successful discharge. Tests: `c0_2ci_scanout_service_ready_with_registry_discharges_before_destroy` (extended with `payload_aliases() == 0`) and new `c0_2ci_scanout_service_ready_with_registry_unregisters_only_the_discharged_alias` (release one payload, mint the barrier with a second outstanding — callback runs exactly once, for the second key). Mutation-verified: dropping the `unregister_payload_alias` call fails both |
| F2-B3 (blocking, F6: a failed discharge destroyed the right it claimed to retain) | **RESOLVED** — `ScanoutAllocation::discharge_file_owned` now matches `DirectFramebufferAllocation`'s existing correct shape (keeps the backing in `self.file_owned` on failure, returns only `io::Error`). New test `c0_2ci_scanout_service_ready_with_registry_retries_failed_discharge` (fail `close_gem`, tick, assert `file_owned` survives at `FramebufferRemoved` and the alias stays registered; clear, tick, assert one more `CloseGem` and the entry gone). Mutation-verified against the exact reinstall-then-take-back-out shape the finding names |
| F2-M1 (major: the pre-fix leak paths still existed beside the fixes; `DirectFramebuffer` uncovered) | **RESOLVED** — `AllocationPayload::file_owned_alias_present`/`discharge_file_owned` are the single dispatch point `adopt` (now refuses), `adopt_with_registry`, `service_ready` (re-dirties instead of destroying), `service_ready_with_registry` and `apply_teardown_release` all key off, covering `DirectFramebuffer` as well as `Scanout`. Every existing test that adopted a file-owned `DirectFramebuffer`/`Scanout` payload via plain `adopt` (F-1's `c0_2ci_drm_cleanup_fd_family_barrier_discharges_payload_alias`/`..._discharge_failure_retries`, F-2's `..._discharging_file_owned_leaves_shared_intact`/`..._apply_teardown_release_refuses_live_file_owned`, and 9.5) switched to `adopt_with_registry`, dropping their now-redundant manual `register_payload_alias` calls |
| F2-M2 (major: admission checked after extraction; renderer adopted before display; no rollback) | **RESOLVED** — `ResourceService::is_exhausted` checked before extracting anything; adoption order reversed to display-first/renderer-second; `ScanoutBo`/`CopiedRenderSource::restore_physical_backing` (inverse of `take_physical_backing`) plus `ScanoutAllocation::into_scanout_bo_backing`/`CopiedSourceAllocation::into_copied_render_source_backing` restore full legacy ownership on any post-extraction failure; a renderer failure after the display succeeded releases the display adoption too via new `ResourceService::release_fresh_adoption` (all-or-nothing). Tests: `c0_2ci_release_fresh_adoption_reclaims_untouched_lease`, `c0_2ci_release_fresh_adoption_refuses_when_something_else_is_using_it` (the new primitive, deterministic); the full extraction/restoration round-trip through a real bo is exercised end-to-end by the B-13/F2-B1 hardware test (a dedicated failure-injection test through a live `PlatformBackend` was not written — see note below) |
| F2-M3 (major, repeats F1-M1: device-less registry couldn't show the registry performs the last close) | **RESOLVED** — `c0_2ci_fd_family_barrier_real_gbm_payload_drm` rebuilt on `new_with_device_and_io`, with `weak.upgrade().is_some()` asserted inside the discharge closure (after the payload's own alias drops) and `is_none()` only after the mint; the inverted justification comment is gone |
| F2-m1 (minor: the husk keeps a counted alias — `take_physical_backing` clones `drm`, so the pool husk still holds an `Rc<Device>`) | **RESOLVED in Task 9 (F-8)** — `register_pool_husk`/`unregister_pool_husk` added to registry; tested in `c0_2ci_handoff_complete_fd_family_barrier_deterministic` |
| F2-m2 (minor: `Option<Arc<VkContext>>` stays; ruling accepted as-is) | No further action, per the review's own ruling |

**Note on F2-M2 test coverage:** the fix's *mechanism* (`is_exhausted`, `restore_physical_backing`, `into_*_backing`, `release_fresh_adoption`) is real and the two new deterministic tests cover `release_fresh_adoption` directly, but a full `register_managed_scanout_bo` failure-injection test (drive `service` to exhaustion, or fail the renderer adopt with the display already committed, through a live `PlatformBackend`/hardware bo) was not written this session — doing so needs either a way to force `ResourceService::exhausted` from outside `resources` (not currently exposed) or a second real bo to construct a genuine renderer-adopt-fails-after-display-succeeds scenario, both of which are more fixture work than this round's remaining budget covered. Flagging rather than silently claiming full coverage.

Gate for this round: `cargo +nightly fmt` clean, `cargo clippy --all-targets -- -D warnings` clean, `c0_2ci` 85/85 (81 baseline + 4 new deterministic) on twelve runs with no flakes, all three portable targets check clean. Hardware re-run (per the dispatch's explicit ask, after F2-M3):

```
$ cargo test -p yserver --lib c0_2ci_fd_family_barrier_real_gbm_payload_drm -- --ignored --nocapture
running 1 test
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1696 filtered out; finished in 0.02s

$ cargo test -p yserver --lib -- --ignored --nocapture   (all hardware tests)
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test result: ok. 75 passed; 0 failed; 0 ignored; 0 measured; 1622 filtered out; finished in 23.45s
```

(also re-run standalone 5 times with no flakes.)

## Task 5: GPU, descriptors and readback lifetime

**Status: EXECUTED at `4dca7392`.** **Review round 1 (2026-09-11): REJECTED** — see the findings and `docs/handoff-phase-c0-stage-2c-i-fix.md`; unchecked steps below are not done or not proven.

**Files:** Create `resources/gpu.rs`; modify `backend.rs`, `scene.rs`, `engine.rs`, `frame_builder.rs` and `kms/vk/ops/mod.rs` only where a read submission needs its uncertainty classification preserved.

**Consumes:** Managed storage/scanout entries, existing `FenceTicket`, copied ownership state and synchronous readback.

**Produces:** `GpuObligation`, `ReadObligation`, `CoreRetirementBatch`. A GPU obligation contains exact allocation keys/obligation IDs, the original `FenceTicket` and Vulkan context. Read obligations also retain source/staging uses. They are not `Send`.

```rust
pub(crate) struct GpuObligation {
    pub(crate) entries: Vec<(AllocationKey, ObligationId)>,
    pub(crate) ticket: crate::kms::render::platform::FenceTicket,
    pub(crate) context: std::sync::Arc<crate::kms::vk::device::VkContext>,
}
```

`CoreRetirementBatch` retains command/descriptor slot ownership and all managed leases used by a submission, with `Option<GpuObligation>` set only after submission returns its ticket. Failed/uncertain dispatch retains that batch in the service. An empty ticket is not considered successful if dispatch may have occurred.

- [x] **5.1 Write the source/scratch regression.** Extend the existing root IncludeInferiors snapshot test path with managed source and scratch allocations. Observe that successful readback produces owned CPU bytes before scratch upload; source-read completion is recorded then, whereas scratch cleanup remains behind its own upload/Composite ticket. Assert source retention is not extended solely by scratch use. Add the opposite failure case: uncertain read submission leaves source/staging retained and closes the converted route.
Instrument the real adapter's cleanup boundary with source/scratch destruction counters. After snapshot readback returns and after scratch GPU completion, respectively, the test body must contain:

```rust
assert_eq!(source_read_pending, 0);
assert_eq!(scratch_drops.get(), 0);
assert!(!scratch_ticket.poll_signaled_result(&vk).unwrap());
```

Here `source_read_pending` is the service's count for the source read obligation, not all KMS uses of the source; `scratch_ticket` is the scratch's actual ticket. Submit/signify completion with the fixture's controlled GPU path, service the batch, release the scratch's final logical lease and assert `scratch_drops.get() == 1`. A separate pending-read case asserts the source and staging destruction counters remain zero across backend detach. Declare test-only count accessors on the real entries; do not replace the adapter with an event-log simulator.

- [x] **5.2 Run** `cargo test -p yserver --lib c0_2ci_read` and `cargo test -p yserver --lib c0_2ci_gpu` before the new adapters.
- [ ] **5.3 Register dependencies before dispatch.** At managed frame submission, enumerate every read/write allocation, reserve use and register its obligation before the GPU can use raw handles. Move descriptors/command slots into `CoreRetirementBatch`. Bind the returned ticket on success; on proven pre-submit failure cancel only those obligations that provably never reached GPU execution. On uncertainty freeze them.
- [x] **5.4 Use real ticket status for completion.** Implement `ResourceService::poll_gpu(&mut self, now: Instant) -> Result<(), ResourceError>` over registered batches. Match the existing API without converting errors to success:

```rust
match batch.ticket_status() {
    Ok(false) => pending.push(batch),
    Ok(true) => match self.validate_gpu_batch(batch) {
        Ok(prepared) => self.commit_gpu_batch(prepared),
        Err((error, retained)) => {
            self.quarantine_gpu_batch(retained, error);
            failed = true;
        }
    },
    Err(_) => {
        self.quarantine_gpu_batch(batch, ResourceError::Frozen);
        failed = true;
    }
}
```

In this loop `batch` is the full `CoreRetirementBatch`, including descriptor/command slots and leases, not only its ticket. `CoreRetirementBatch::ticket_status(&self) -> Result<bool, ash::vk::Result>` polls its actual ticket/context; a possibly dispatched batch without a ticket goes to quarantine. `pending` is the next full-batch vector; install it before returning any accumulated failure.

Define `ResourceService::validate_gpu_batch(&self, batch: CoreRetirementBatch) -> Result<ValidatedGpuBatch, (ResourceError, CoreRetirementBatch)>`. It checks the complete ticket/producer/incarnation/allocation/obligation set and prepares all bookkeeping without changing any availability entry. `ValidatedGpuBatch` is private and owns the entire batch. `commit_gpu_batch(&mut self, prepared: ValidatedGpuBatch)` is an infallible, non-reentrant serialized application of the validated set; it performs no allocation or callback until all proof mutations are committed. Duplicate already-consumed evidence is handled consistently for the complete set. Do not place a `?` inside the per-entry application loop.

`quarantine_gpu_batch(&mut self, batch: CoreRetirementBatch, reason: ResourceError)` first roots the intact batch in its reserved quarantine position, then freezes every still-correlated entry without early return and closes the route. Failure to find a stale entry cannot discard descriptors, remaining leases or other batches. Add `c0_2ci_gpu_batch_late_invalid_proof_is_atomic`: valid obligation A followed by invalid/stale B changes neither entry, keeps all actual allocation/descriptor counters at zero destruction, and emits no availability wake; cover the inverse order and a freeze lookup failure. This is the local correction for plan-review B-1.

- [ ] **5.5 Wire managed scene and read adapters.** `PendingAck` carries logical damage/Present metadata separately from the retained GPU batch. `drain_pending_pool_releases` returns descriptors only after service authorization. `read_scanout_region` takes managed source/staging reservations when handed a managed BO; its successful `run_one_shot_op_with_wait` followed by CPU byte copy ends the source read. Copied read selects the renderer target and preserves `validate_renderer_readback`, not sink external acquisition. The scratch follows ordinary managed storage retirement and is freed once on all Composite return paths.
- [ ] **5.6 Test** GPU-before-KMS and KMS-before-GPU, error after submit, dropped frame metadata with a live ticket, scratch free after Composite error, and descriptor reset exclusion. Run focused tests plus existing snapshot tests, format and clippy; commit with `feat(kms): retain gpu and read dependencies with allocation leases`.

**Fix round 1: `17384ae6`.** Session F-4, resuming a prior Sonnet session that was cut off mid-way by a rate limit with `gpu.rs`/`platform.rs`/`resources/mod.rs`/`resources/tests.rs`/`vk/ops/mod.rs` dirty. Closing B-15 and the Task-5 quarter of M-23 (`docs/superpowers/findings/2026-09-11-stage-2c-i-implementation-review-round1.md`), per `docs/handoff-phase-c0-stage-2c-i-fix.md`'s "F-4 — Task 5" section.

The inherited tree did not compile: `GpuObligation.context` had already been changed to `Arc<VkContext>` (non-`Option`) and `GpuObligation::for_tests_stub` deleted, but every test that bound a `GpuObligation` — 9 in `tests.rs`, 1 in `adapter_tests.rs` — still called the deleted constructor. Making a `GpuObligation` requires a real, live `Arc<VkContext>` now (there is no deterministic fixture: `VkContext::new()` is the only constructor and needs a real ICD), so every one of those became a `_vulkan`/`#[ignore]` test; determinism where the original relied on a stub ticket's own `signaled_cache` (`test_signal()`) now comes from flipping the existing `#[cfg(test)] test_ticket_status` field in place through a new `ResourceService::pending_batches_mut()` (`#[cfg(test)]`) — the mechanism under test never sees the difference, and this avoids ever calling `poll_signaled_result` against a null fence handle with a real device (invalid Vulkan usage). `FencePool::acquire` was widened to `pub(crate)` so a producer adapter/test outside `platform.rs` can obtain a genuine fence-backed ticket.

| Finding | Verdict |
| --- | --- |
| B-15 (Task 5's tests are a Spy-only simulator; no read/scene adapter exists) | **DEFERRED TO F-4b (wiring); read-proof adapter RESOLVED, environmentally unconfirmed on this box.** `backend::read_scanout_region_for_managed_source` now exists as a real, non-simulated producer adapter: it runs the real `read_scanout_region` (the root IncludeInferiors snapshot path — CPU copy off the real composited scanout) and correlates its actual `Ok`/`Err` with the source's registered `Read` obligation via the new `resources::gpu::record_read_outcome`, which applies the real proof on success and freezes (never fabricates) on failure. The decisive test, `kms::render::backend::tests::c0_2ci_read_source_scratch_regression_vulkan`, extends `root_get_image_reads_scanout_pixels_not_root_storage`'s exact fixture with a managed source and a managed scratch allocation, asserts the plan's literal snippet (`source_read_pending == 0`, `scratch_drops == 0`, `!scratch_ticket.poll_signaled_result(&vk).unwrap()` immediately after a REAL async submission via the newly-`pub(crate)` `FencePool::acquire` + `vk::ops::submit_one_shot_op_async`), then waits for the real fence and asserts the scratch frees exactly once through the existing, reviewed-sound `poll_gpu`. No `apply_validated_proof`/`test_signal` call appears in the test body for the proof under test (F3). The opposite failure case (5.1's second half) was already correctly covered by the pre-existing, still-passing deterministic `c0_2ci_read_uncertain_submission_leaves_source_and_staging_retained`, which needs no `Arc<VkContext>` (`ReadObligation` carries none) and was left untouched. **This test cannot be confirmed green on this box**: `KmsBackend::for_tests_with_vk_live_scene()` — the *only* fixture that produces a live scanout pool, required for any `read_scanout_region` call — fails during scanout BO allocation with `drm prime_fd_to_buffer: Inappropriate ioctl for device (os error 25)` on every GBM plan it tries. This is not new and not caused by this session: the two pre-existing tests in the same file that use the identical fixture (`root_get_image_reads_scanout_pixels_not_root_storage`, `root_overlay_xor_pass_reaches_scanout`) hit the exact same error, verified by running them directly — they simply hide it, using an `eprintln!("skipping: ..."); return;` pattern that reports a false "ok" instead of an honest skip (an R12 violation that predates this session and is out of Task 5's scope to fix). The box has real DRM nodes and Vulkan ICDs (16 other `_vulkan`/`_drm` tests in this session's own run passed against them), so the defect is specific to `for_tests_with_vk_live_scene`'s GBM scanout-pool allocation path — plausibly a mismatch between the DRM device `PlatformBackend::for_tests()`'s synthetic device opens and whichever GPU `VkContext::new()` enumerates for PRIME import — not a missing capability of the box in general. This is squarely outside Task 5's file map (`drm`/scanout-pool allocation is Task 2/4 territory) and outside this session's remaining scope; it is reported here per F8 rather than papered over. 5.3 (register-before-dispatch at the real managed frame submission call site) and 5.5 (wiring `drain_pending_pool_releases`/`PendingAck`/`engine.rs`/`frame_builder.rs`) are also deferred: the bookkeeping-only halves (`resources::gpu::prepare_retirement_batch`, `cancel_pre_submit_batch`, `freeze_uncertain_batch`) exist and compile but have no caller and no test (nothing exercises them beyond the compiler), and wiring them into the real paint/frame-submission pipeline in `engine.rs`/`frame_builder.rs`/`scene.rs` is a change to hot production rendering code that this session did not attempt given the compile-recovery work already required and the stage's own "plan size drives defect density" lesson — a rushed, unreviewed wiring into the live paint path is a worse outcome than an honest defer |
| M-23 — `GpuObligation.context: Arc<VkContext>` | **RESOLVED (test: `c0_2ci_gpu_batch_late_invalid_proof_is_atomic_vulkan`, passing on this box's hardware)** — kept from the inherited tree (was already fixed before the rate-limit cutoff): the field is `Arc<VkContext>` by value, `GpuObligation::new` is the sole constructor, and there is no `#[cfg(test)]` (or any other) shim that builds one with an absent context. This session's job was making every consumer compile against that invariant rather than reopening it |
| M-23 — delete `FenceTicket::poll_signaled_result_opt` | **RESOLVED** — kept from the inherited tree; confirmed absent (`grep -rn poll_signaled_result_opt crates/yserver/src` returns nothing) and `poll_signaled_result(&self, vk: &VkContext)` has no `Option`/status-fallback branch |
| M-23 — `drop_counter` under `#[cfg(test)]` | **RESOLVED** — kept from the inherited tree; `CoreRetirementBatch.drop_counter` and its `Drop` read are both `#[cfg(test)]`-gated in `gpu.rs` |
| M-23 — `ValidatedGpuBatch` private | **RESOLVED** — kept `pub(super)` (private to `resources`, gpu.rs's parent) from the inherited tree, and additionally fixed a knock-on `private_interfaces` warning (`-D warnings` failure) this created: `ResourceService::validate_gpu_batch`/`commit_gpu_batch` were still `pub(crate)`, wider than the type they return/take, so they are now `pub(in crate::kms::render::resources)` — an R1 fix, since nothing outside `resources` ever called either (verified by grep) and the plan's own text describes them as `poll_gpu`'s internal steps, not a public API |

**R1 fixes:** the `ValidatedGpuBatch` visibility mismatch above (mechanical — required for `-D warnings`, no behavior change). No other plan/contract contradictions found in Task 5's remaining text.

Gate for this round: `cargo +nightly fmt` clean; `cargo clippy --all-targets -- -D warnings` clean; `c0_2ci` 80 passed/0 failed/16 ignored on twelve consecutive runs, zero flakes; full `cargo test -p yserver --lib` 1617 passed/0 failed/88 ignored, no failures anywhere (R2's three named flaky executor tests did not fire this run). Hardware run (`--ignored`, this box has real DRM nodes and NVIDIA/RADV ICDs):

```
$ cargo test -p yserver --lib c0_2ci -- --ignored
running 16 tests
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::resources::tests::c0_2ci_progress_no_composition_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_no_premature_pool_return_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_batch_late_invalid_proof_is_atomic_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_dropped_frame_metadata_with_live_ticket_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_ticket_error_quarantines_batch_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_dri3_lease_regressions_vulkan ... ok
test kms::render::resources::tests::c0_2ci_descriptor_reset_exclusion_until_gpu_signaled_vulkan ... ok
test kms::render::backend::tests::c0_2ci_read_source_scratch_regression_vulkan ... FAILED (environmental: see B-15 verdict above)
test kms::render::resources::tests::c0_2ci_serviced_time_pauses_during_seat_inactive_and_expires_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_adapter_vt_away_dpms_off_idle_service_progress_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_batch_freeze_lookup_failure_handled_vulkan ... ok

test result: FAILED. 15 passed; 1 failed; 0 ignored; 0 measured; 1689 filtered out; finished in 1.12s
```

15 of 16 pass. The one failure is `c0_2ci_read_source_scratch_regression_vulkan`, reported honestly via `panic!("environmental skip: ...")` (R12) rather than a false pass — see the B-15 verdict for the root cause (a pre-existing, unrelated `for_tests_with_vk_live_scene` GBM/DRM fixture defect, independently reproduced against two already-existing tests in the same file). Five of the sixteen `_vulkan` tests above are renamed/converted by this session from pre-existing deterministic tests that no longer compile against `GpuObligation`'s non-`Option` context (`c0_2ci_gpu_batch_late_invalid_proof_is_atomic`, `c0_2ci_gpu_batch_freeze_lookup_failure_handled`, `c0_2ci_gpu_ticket_error_quarantines_batch`, `c0_2ci_gpu_dropped_frame_metadata_with_live_ticket`, `c0_2ci_descriptor_reset_exclusion_until_gpu_signaled`, `c0_2ci_progress_no_composition`, `c0_2ci_serviced_time_pauses_during_seat_inactive_and_expires` — the last two share one `_vulkan` test as before); one (`c0_2ci_adapter_vt_away_dpms_off_idle_service_progress`, Task 10's file) was converted for the same compile reason and is flagged here for Task 10's own fold-back to reconcile against its fixture-matrix row. `c0_2ci_read_source_scratch_regression_vulkan` is new.

**F8 stop — B-15's remaining wiring (5.3/5.5) and the `for_tests_with_vk_live_scene` environmental defect.** Both are reported rather than forced:

1. **5.3/5.5 wiring** needs a follow-up session (call it F-4b) to thread `prepare_retirement_batch`/`cancel_pre_submit_batch`/`freeze_uncertain_batch` and the read adapter into the real managed frame-submission path in `engine.rs`/`frame_builder.rs`/`scene.rs` — production paint code this session did not touch.
2. **`for_tests_with_vk_live_scene`'s scanout BO allocation** fails on this box independent of anything in Task 5; whichever session next needs a real composited scanout for a hardware test (this one, or Task 10's F-9) should investigate the DRM-device/Vulkan-device pairing this fixture assumes before relying on it again.

**Fix round 2: `f475b04c`.** Session F-4b, per `docs/handoff-phase-c0-stage-2c-i-fix.md`'s "F-4b" section and `docs/superpowers/findings/2026-09-12-stage-2c-i-fix-F4-review.md`'s "What F-4b must do" list (items 1–5). Closing F4-B1 in full; F4-B2 partially (the ENOTTY blocker it names is fixed, but a *second*, deeper blocker was found reaching the decisive test and is reported here under F8); F4-M1 and F4-M2 implemented and reviewed sound but not exercisable end-to-end because of F4-B2's remaining blocker.

| Finding | Verdict |
| --- | --- |
| F4-B1 (eight `c0_2ci_` tests regressed to `_vulkan`/`#[ignore]`) | **RESOLVED (tests: `c0_2ci_gpu_batch_late_invalid_proof_is_atomic`, `c0_2ci_gpu_batch_freeze_lookup_failure_handled`, `c0_2ci_gpu_ticket_error_quarantines_batch`, `c0_2ci_gpu_dropped_frame_metadata_with_live_ticket`, `c0_2ci_descriptor_reset_exclusion_until_gpu_signaled`, `c0_2ci_progress_no_composition`, `c0_2ci_serviced_time_pauses_during_seat_inactive_and_expires`, `c0_2ci_adapter_vt_away_dpms_off_idle_service_progress`, all green, 12/12 clean runs)**. `GpuObligation.context` is `Option<Arc<VkContext>>`; the only `None` constructor is `#[cfg(test)] GpuObligation::for_tests_stub`, gated exactly as F5's amendment (F2-m2) requires; `GpuObligation::new` still takes `Arc<VkContext>` by value and always stores `Some`; `CoreRetirementBatch::ticket_status` consults `test_ticket_status` first (unchanged) and otherwise `.expect()`s `Some` — a comment at the `expect` site says plainly that a `None` reaching it is a caller bug, never a status fallback. All eight tests dropped their `_vulkan` suffix and `#[ignore]` and now build their batches with `for_tests_stub`. Per F4-B1's instruction, kept an *additional* `_vulkan` variant, using a genuine `FencePool`/`OpsCommandPool` submission and no `test_ticket_status` override at all, for `c0_2ci_gpu_dropped_frame_metadata_with_live_ticket` and `c0_2ci_descriptor_reset_exclusion_until_gpu_signaled` only — both wait for the real ticket before polling, so they add real-device evidence without reintroducing F4-M1's race. Deterministic `c0_2ci` count: **88** (80 + these 8), not the ≥89 the handoff expected — see "Deterministic count discrepancy" below; this is not a shortfall, it is a miscount in the handoff's own bookkeeping. |
| F4-B2 (decisive-test fixture fails `ENOTTY` on every machine) | **PARTIALLY RESOLVED; second blocker found — F8 stop below.** `for_tests_with_vk_live_scene()` now opens a real `/dev/dri/cardN` **without master** and substitutes it (via the new `crate::kms::executor::test_support::TestDevice::open_real_drm_matching`, wrapped with `crate::drm::Device::from_file_for_tests`) for every `KmsDevice.device` the fixture seeded, fixing the exact `ENOTTY` this finding named: `PRIME_FD_TO_HANDLE`/`ADDFB2`/`RMFB` all now succeed (verified: pool allocation no longer errors, and the pool's ordinary `ScanoutBo::Drop` at test teardown runs real `RMFB`+`GEM_CLOSE` against the real device with no panic during unwind). This box is multi-GPU (NVIDIA discrete `card1` + AMD integrated `card0`); the finding's literal `TestDevice::open_real_drm_or_ignore()` (first enumerable node) pairs with the *wrong* GPU here and `ADDFB2` fails closed with `EINVAL` importing NVIDIA's PRIME export into AMD's driver — confirmed by testing it directly before adding the pairing fix. `open_real_drm_matching` instead opens the node whose `st_rdev` matches `vk.selected_drm_identity.primary` (`VK_EXT_physical_device_drm`, already computed by `VkContext::new()`), i.e. the node actually paired with the physical device Vulkan selected; falls back to `open_real_drm_or_ignore` when that extension is unavailable. **A second, deeper blocker surfaces once the first is fixed**: reaching `BoPhase::OnScreen` (what `select_scanout_bo_for_rect(OnScreenOnly)` — used by `read_scanout_region`, the decisive test, and both pre-existing sibling tests — requires) needs a real `DRM_IOCTL_MODE_ATOMIC` commit to actually land. See the F8 stop below; not resolved by this session. |
| F4-M1 (racy `poll_signaled_result` assertion) | **RESOLVED (code review only — see F8 stop; the test cannot currently run far enough to exercise this code path).** Deleted the named `assert!(!scratch_ticket.poll_signaled_result(&vk_ctx).unwrap())` and, for the same reason (a trivial no-op submission can already be signalled by the time anything polls it), also deleted the following "poll while genuinely unsignaled" step and its `assert_eq!(scratch_drops.get(), 0)`. The scratch's only remaining evidence is the deterministic `submit` → `wait` → `poll_gpu` → `service_ready` → `assert!(!service.contains(&scratch_key))` sequence, which makes no claim about the fence's state before the wait. |
| F4-M2 (`Spy` source/scratch; free-parameter `source_key`) | **RESOLVED (code review only — see F8 stop; the test cannot currently run far enough to exercise this code path).** The source is the live-scene pool's actual on-screen bo (selected the same way `read_scanout_region` selects it, via `select_scanout_bo_for_rect`), converted to a managed `ScanoutAllocation` through the real `PlatformBackend::register_managed_scanout_bo` over a real `DrmCleanupRegistry::new_with_device_and_io` (the fixture's real `Rc<drm::Device>`, paired with the counting `MockCleanupIo` from `resources::tests` — no real `RMFB`/`GEM_CLOSE` ioctl fires for this path, but the real fb/gem handles are what get recorded, and the assertion checks for exactly one `RemoveFb`+`CloseGem` pair). The scratch is a real `StorageAllocation` adopted via `Storage::into_managed(&mut service, &backend.platform, target, (0,0))`, exactly as Composite's own scratch would be. `read_scanout_region_for_managed_source` no longer takes `source_key` as a parameter: it re-runs `select_scanout_bo_for_rect` itself (the identical, deterministic selection `read_scanout_region` performs internally — nothing mutates the pools between the two calls) and takes the key from that slot's `managed_key()`, failing closed (`Err(ResourceError::InvalidProof)`) if the resolved bo is not a managed allocation. "Source retention is not extended by scratch use" is now asserted on the real allocation via `service.contains(&source_key)` after `detach_managed_entries()` + `service_ready_with_registry`, not a `Spy` drop counter. |

**F8 stop — `BoPhase::OnScreen` is unreachable in this fixture, with or without master.** `select_scanout_bo_for_rect(OnScreenOnly)` — the selection `read_scanout_region`, the decisive test, and both `root_get_image_reads_scanout_pixels_not_root_storage`/`root_overlay_xor_pass_reaches_scanout` all use — only matches a bo whose `BoPhase` is `OnScreen`. A bo reaches `OnScreen` only via `transition_to_on_screen` (a real pageflip-complete event) or `mark_on_screen_after_modeset` (a real synchronous modeset install); both require a real `DRM_IOCTL_MODE_ATOMIC`/`DRM_IOCTL_MODE_SETCRTC` to actually have landed. Two independent reasons that cannot happen here:

1. The kernel gates `DRM_IOCTL_MODE_ATOMIC` on `DRM_MASTER` unconditionally (`drivers/gpu/drm/drm_ioctl.c:714`, confirmed against this box's kernel tree). F4-B2 explicitly requires opening the substituted node **without** master (so the fixture can coexist with the live display server that holds it), so this ioctl is rejected for lack of master by design — acquiring master here would risk actually repainting the live, currently-in-use display out from under the user, which this session did not do and should not do.
2. Independent of master, `PlatformBackend::for_tests()` seeds exactly one synthetic `Output` (`connector`/`crtc`/`plane` all `Handle::from(1)`, `mode: mem::zeroed()`, and correspondingly no real `plane_fb_id_prop`/`plane_crtc_id_prop`/etc.) that names no real object on the substituted card. Observed directly (temporary instrumentation, since reverted): the real atomic-commit attempt fails with `Os { code: 2, kind: NotFound, message: "No such file or directory" }` — `ENOENT`, i.e. `drm_mode_object_find` cannot resolve the fabricated handles — not `EACCES`/`EPERM`, which is what a master-only rejection would look like. So even a fixture that somehow held master would still need a second, unrelated fix: pairing the fixture's one `Output` with a real CRTC/plane/property identity discovered from the substituted device (e.g. via the existing, production `crate::drm::modeset::discover_output`), which reason 1 makes moot anyway.

Neither of these is "a seam that does not exist yet" fixable by a small addition in the sense F4-B2 anticipated (that clause was written expecting the *ADDFB2* step to be the one that might need master — it doesn't, and is fixed). This is a hard, structural conflict between "must not hold master" and "must reach a state only master-gated ioctls produce," discovered only once the first (ADDFB2/ENOTTY) blocker was cleared. Reported per F8 rather than papered over. **5.1 stays unticked**; do not tick it in a future session without either resolving this conflict or changing what evidence 5.1 requires.

**The two pre-existing tests on the same fixture, as asked:** before this session, `root_get_image_reads_scanout_pixels_not_root_storage` and `root_overlay_xor_pass_reaches_scanout` both hit the fixture's `ENOTTY` and used `eprintln!("skipping: ..."); return;`, which `cargo test` reports as a false `ok` (an R12 violation predating this session, left alone per instructions — out of scope). With the fixture now getting past `ENOTTY`, both tests run further than before and then **genuinely FAIL** (not skip) at the exact same `OnScreen` blocker described above:

```
$ cargo test -p yserver --lib root_get_image_reads_scanout_pixels_not_root_storage -- --ignored --nocapture
panicked at crates/yserver/src/kms/render/backend.rs:40189:10:
scanout readback: Custom { kind: Other, error: "root screenshot rect has no on-screen scanout bo" }
test kms::render::backend::tests::root_get_image_reads_scanout_pixels_not_root_storage ... FAILED

$ cargo test -p yserver --lib root_overlay_xor_pass_reaches_scanout -- --ignored --nocapture
panicked at crates/yserver/src/kms/render/backend.rs:40553:10:
scanout readback (overlay): Custom { kind: Other, error: "root screenshot rect has no on-screen scanout bo" }
test kms::render::backend::tests::root_overlay_xor_pass_reaches_scanout ... FAILED
```

This is a real, newly-visible regression in what `cargo test -p yserver --lib <name> -- --ignored` reports for these two names specifically (false-`ok` → real `FAILED`), but it does not affect any gate this or prior sessions run: neither test carries the `c0_2ci_` prefix, so `cargo test -p yserver --lib c0_2ci -- --ignored` never selects them, and plain `cargo test -p yserver --lib` never runs `#[ignore]`d tests at all. Their skip idiom is unchanged, per instructions (out of scope); whoever next touches `for_tests_with_vk_live_scene` inherits both this report and the F8 stop above.

**Deterministic count discrepancy (89 vs 88).** The handoff's F4-B1 fix expected the deterministic `c0_2ci` count to return to "≥ 89" — the count *before* F-4's tree. `git show 17384ae6 -- crates/yserver/src/kms/render/resources/tests.rs` shows F-4 actually removed **nine** deterministic tests, not eight: the eight named in F4-B1, plus `c0_2ci_read_source_scratch_regression` — the *old*, `Spy`-based, non-`_vulkan` predecessor of the decisive test itself. That ninth removal was not a regression to reverse: it is B-15/5.1's own intentional, F3-mandated promotion from a synthetic `Spy` fixture to a real hardware test (`c0_2ci_read_source_scratch_regression_vulkan`), the exact conversion this whole handoff exists to make happen. Restoring a ninth `Spy`-based deterministic test to hit "89" would directly re-violate F3 ("no `Spy` where the real type exists") for the one test this stage cares most about. This session's **88** (80 + the eight F4-B1 restorations) is therefore the correct number under the contract, not a shortfall; the "≥ 89" figure in the fix handoff is the one that needs correcting.

Gate for this round: `cargo +nightly fmt` clean; `cargo clippy --all-targets -- -D warnings` clean; `c0_2ci` **88** passed / 0 failed / 10 ignored on twelve consecutive runs, zero flakes; full `cargo test -p yserver --lib` **1625** passed / 0 failed / 82 ignored, no failures anywhere (R2's three named flaky executor tests did not fire this run). Hardware run (`--ignored`, this box has real DRM nodes — NVIDIA `card1`/RADV+proprietary and AMD `card0`/amdgpu — and NVIDIA/RADV ICDs):

```
$ cargo test -p yserver --lib c0_2ci -- --ignored
running 10 tests
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::resources::tests::c0_2ci_descriptor_reset_exclusion_until_gpu_signaled_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_dropped_frame_metadata_with_live_ticket_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_no_premature_pool_return_vulkan ... ok
test kms::render::backend::tests::c0_2ci_read_source_scratch_regression_vulkan ... FAILED
test kms::render::store::tests::c0_2ci_storage_dri3_lease_regressions_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan ... ok

test result: FAILED. 9 passed; 1 failed; 0 ignored; 0 measured; 1697 filtered out; finished in 0.72s
```

9 of 10 pass; the one failure is `c0_2ci_read_source_scratch_regression_vulkan`, reported honestly via a real `panic!` (R12) — see the F8 stop above for the root cause. All eight F4-B1 tests that used to appear in this list are gone from it (they are deterministic now) except the two intentional real-fence `_vulkan` variants, both green.

**R1 fixes:** none beyond the mechanical `TestDevice::into_file`/`open_real_drm_matching` additions (test-support seams, not contract changes).

**Fix round 3: `4e870930`.** Session F-4c, resuming a Sonnet session cut off mid-way by a rate limit with `backend.rs`/`resources/mod.rs` dirty (the read half already largely written: `ResourceService::with_scanout_read`/`with_scanout_write`, `read_scanout_region` split into `scanout_copy_needed_bytes` + `submit_scanout_copy_to_staging`, `read_managed_scanout_region_bytes`, the decisive test reworked toward `PermissiveDump`). Per `docs/handoff-phase-c0-stage-2c-i-fix.md`'s "F-4c" instructions and `docs/superpowers/findings/2026-09-12-stage-2c-i-fix-F4b-review.md`'s "What F-4c must do" list (items 1-4), closing F4b-B1/F4-M3's read half, F4-m1 and F4b-m1; F4b-B1/F4-M3's write half is an F8 stop, precisely split below.

The inherited diff was read in full first (per instructions), found sound, and kept unmodified except for verification: `with_scanout_read`/`with_scanout_write` mirror `with_storage_read`/`with_storage_write` exactly (reserve a `Read`/`Write` use, borrow the entry's payload, match `AllocationPayload::Scanout`, run the closure, drop lease and borrow); `read_managed_scanout_region_bytes` takes image/staging/size from the `ScanoutAllocation` payload's `SharedBacking` under a `Read` lease rather than the pool's (possibly husked) `ScanoutBo` fields, scoped to `OutputScanout::Shared` (a managed `Copied`-route read fails closed, out of scope); `read_scanout_region_for_managed_source` derives `source_key` from the one `select_scanout_bo_for_rect` call it performs itself and calls the managed reader instead of the legacy one.

| Finding | Verdict |
| --- | --- |
| F4b-B1 (managed scanout bos are husks; `read_scanout_region` reads the husk) — read half | **RESOLVED (test: `c0_2ci_read_source_scratch_regression_vulkan`, hardware-green on this box; mutation-verified — see below)**. `with_scanout_read`/`with_scanout_write` added to `resources/mod.rs`; `read_managed_scanout_region_bytes` reads the payload under a `Read` lease instead of `bo.vk_image`/`bo.vk_transfer`. Verified by two independent mutations, each restored immediately after: (1) making the closure passed to `with_scanout_read` return `vk::Image::null()`/a zero-size staging buffer instead of the real `SharedBacking` fields (simulating "read the husk") makes the test fail with `"scanout staging buffer too small"` at the `result.expect("scanout readback")` line; (2) making `resources::gpu::record_read_outcome` a no-op that never calls `apply_validated_proof`/`freeze` makes the test fail at `"source_read_pending must be 0: the real read already discharged it"`. Both mutations were reverted (`git diff`/`git checkout` confirmed clean) before the commit above. |
| F4b-B1 (write half: composing into a managed bo through the legacy scene path) / F4-M3 (5.3/5.5 wiring) | **F8 stop — see below.** No code change attempted; the existing legacy path is untouched (byte-identical to the tree this session inherited), so R8 (production never takes the managed branch, and the branch does not exist to take) holds trivially. |
| F4-m1 (`#[allow(dead_code)]` on `read_managed_scanout_region_bytes`/`read_scanout_region_for_managed_source`) | **NOT APPLICABLE yet — verified still required.** Removed both attributes and ran `cargo clippy -p yserver --tests -- -D warnings`: fails `dead-code` on both functions (no non-test caller exists without the write-path wiring). Re-added; clippy clean again. Comes off only once F4b-B1's write half lands a real caller. |
| F4b-m1 (`base.platform.devices.len() == 1` assert before device substitution) | **RESOLVED (already present in the inherited tree, verified read and kept).** `for_tests_with_vk_live_scene` asserts this immediately before its device-substitution loop, with a message naming the multi-device hazard it guards against. |

**F8 stop — the scene-submission write branch (F4b-B1's write half / F4-M3's remaining 5.3/5.5 wiring).** Investigated the seam directly rather than estimating it:

- The call site is `submit_shared_scanout_frame` (`scene.rs`), invoked from `tick_one_output`'s `OutputScanout::Shared` arm (`scene.rs`, the `pool.bos.get_mut(token.bo_idx)` → `submit_shared_scanout_frame(&inner.vk, &drm_device, &layout.output, bo, ...)` call). It reads `bo.fb_handle`/`bo.state`, then calls `record_and_submit_render(vk, bo, ...)`, which uses the `ComposeRenderTarget for ScanoutBo` impl reading `self.vk_image`/`self.vk_image_view`/`self.vk_transfer.command_buffer`/`self.vk_transfer.timestamp_pool` directly off the pool struct. Once a bo is managed, those four fields are null/empty on the husk (`ScanoutBo::take_physical_backing` moved them into `ScanoutAllocation::shared`); `self.vk_semaphore`/`width`/`height`/`last_gpu_render_ns` are *not* moved and stay on the pool struct. A managed-route branch therefore needs a wrapper type implementing `ComposeRenderTarget` over two simultaneous mutable borrows — `&mut ScanoutBo` for the fields that stay on the pool, and `&mut SharedBacking` (obtained via `with_scanout_write`) for the fields that moved — not a single-field patch.
- That wrapper is the easy part. The hard part, verified by tracing the call chain: **no `ResourceService` reaches `scene.rs` today.** `KmsBackend.resource_service: Option<ResourceService>` exists and is threaded only into the Task-6/7 owner-event consumer (`backend.rs` ~17962/17988/18428/18439/18524); `tick`, `maybe_composite`, `tick_one_output`, `submit_shared_scanout_frame` and `drain_pending_pool_releases` (`scene.rs`) take no service parameter at all. Adding the managed branch as instructed — `acquire_managed_scanout_bo`/`with_scanout_write` at bo selection, `prepare_retirement_batch` before any handle reaches the GPU, the `CoreRetirementBatch` carried in a new `PendingAck` field, `cancel_pre_submit_batch`/`freeze_uncertain_batch` on the two failure exits, `drain_pending_pool_releases` consulting the service — means threading `Option<&mut ResourceService>` (it must stay optional: R8, production leaves it `None`) through that entire call chain, plus extending every `PendingAck` retirement path (flip-complete, render-completion, failed-submit recovery — `tick_one_output` and its siblings resolve `PendingAck` in several places) to resolve or freeze the new batch field on each exit.
- This is a materially different piece of work from the read half closed in this session's commit: it touches the generic hot compositor path (`ComposeRenderTarget`, shared with `CopiedRenderSource` and `DamageAuditTarget`) that every real frame renders through, across multiple functions and every `PendingAck` consumption site, none of which currently know about `ResourceService`. F-4's session deferred exactly this same piece for the same reason ("a rushed, unreviewed wiring into the live paint path is a worse outcome than an honest defer"); F-4b's session reached the same conclusion before even getting the read half green. Given two prior sessions independently reached this conclusion, and the repository's own recorded lesson that plan-sized changes done in one sitting drive defect density, this session did not attempt it and stopped here per F8 rather than paper over a real seam gap with a rushed patch to hot rendering code.
- **What the next session needs:** (1) add `resource_service: Option<&mut ResourceService>` (or restructure so `tick`/`maybe_composite` can borrow it from `KmsBackend` alongside `platform`) through `tick`/`maybe_composite`/`tick_one_output`/`submit_shared_scanout_frame`/`drain_pending_pool_releases`; (2) a `ManagedSharedComposeTarget<'a> { bo: &'a mut ScanoutBo, shared: &'a mut SharedBacking }` implementing `ComposeRenderTarget` (image/view/command_buffer/timestamp_pool from `shared`, semaphore/width/height/last_gpu_render_ns from `bo`, `record_post_compose`'s barrier using `shared.image`); (3) in `submit_shared_scanout_frame`, branch on `bo.managed_key()`: `None` keeps the exact existing code path (verify byte-for-byte unchanged — R8), `Some(key)` reserves via `with_scanout_write`, calls `resources::gpu::prepare_retirement_batch` for the write before `record_and_submit_render`, and on `record_and_submit_render`'s `Err` calls `cancel_pre_submit_batch` (proven never dispatched) or `freeze_uncertain_batch` (uncertain) as appropriate; (4) add `managed_batch: Option<CoreRetirementBatch>` to `PendingAck` and resolve/freeze it in each of `PendingAck`'s existing retirement sites; (5) `drain_pending_pool_releases` checks `bo.managed_key()` and, when present, consults `service` before returning the bo to `Free`. A fixture that calls `acquire_managed_scanout_bo` before a real tick and asserts the managed branch actually ran (not just compiled) is the decisive test for that session.

Gate for this round: `cargo +nightly fmt` clean (no changes); `cargo clippy --all-targets -- -D warnings` clean; `c0_2ci` **88** passed / 0 failed / 10 ignored on twelve consecutive runs, zero flakes; full `cargo test -p yserver --lib` **1625** passed / 0 failed / 82 ignored, no failures anywhere (R2's three named flaky executor tests did not fire). Hardware run (`--ignored`, this box has real DRM nodes — NVIDIA `card1`/RADV+proprietary and AMD `card0`/amdgpu — and NVIDIA/RADV ICDs), all ten green including the decisive test:

```
$ cargo test -p yserver --lib c0_2ci -- --ignored
running 10 tests
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_dri3_lease_regressions_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan ... ok
test kms::render::backend::tests::c0_2ci_read_source_scratch_regression_vulkan ... ok
test kms::render::resources::tests::c0_2ci_descriptor_reset_exclusion_until_gpu_signaled_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_dropped_frame_metadata_with_live_ticket_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_no_premature_pool_return_vulkan ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1697 filtered out; finished in 0.74s
```

Decisive test alone, `--nocapture`:

```
$ cargo test -p yserver --lib c0_2ci_read_source_scratch_regression_vulkan -- --ignored --nocapture
running 1 test
test kms::render::backend::tests::c0_2ci_read_source_scratch_regression_vulkan ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1706 filtered out; finished in 0.17s
```

The two pre-existing, out-of-scope sibling tests on the same fixture (`root_get_image_reads_scanout_pixels_not_root_storage`, `root_overlay_xor_pass_reaches_scanout`) were re-run unmodified and still fail exactly as F-4b's fold-back reported (`"root screenshot rect has no on-screen scanout bo"` at `OnScreenOnly` selection) — neither is `c0_2ci_`-prefixed, so neither gate command selects them; unrelated to this session's diff and not touched.

**Plan steps closed:** 5.1 ticked — proven by `c0_2ci_read_source_scratch_regression_vulkan` (hardware-green, mutation-verified as above). 5.3, 5.5 and 5.6 stay unticked: 5.5's read-adapter clause ("`read_scanout_region` takes managed source/staging reservations when handed a managed BO") is now true, but 5.5 as a whole also requires the `PendingAck`/`drain_pending_pool_releases`/scene wiring this session did not do, and F1 does not permit ticking a step on a partial proof; 5.3 and 5.6's remaining, unproven halves are exactly the F8 stop above.

## Task 6: Completion progress and transport permission boundary

**Status: EXECUTED at `72a91c27`.** **Review round 1 (2026-09-11): REJECTED** — see the findings and `docs/handoff-phase-c0-stage-2c-i-fix.md`; unchecked steps below are not done or not proven.

**Fix round 1 (F-5a): `5f025fed`.** Session F-5a, the smaller half of F-5,
closing B-11, M-16, M-13, M-14, B-6 (`docs/superpowers/findings/2026-09-11-
stage-2c-i-implementation-review-round1.md`) per
`docs/handoff-phase-c0-stage-2c-i-fix.md`'s "F-5 — Task 6" section, scoped
to `resources/{transport,completion,mod,tests}.rs`, `handoff.rs` (B-6 only)
and the VT/DPMS seams in `backend.rs`. B-10 (gating the twelve DRM sinks,
6.5a/6.5b) and the minor `consume_owner_write` accounting item are **NOT
this session's** — they are F-5b's, per the resume doc's split, and
`c0_2ci_transport_gate_writer_boundary_enforcement` was kept exactly as
instructed (only its `TransportGate::new_legacy` call site updated for the
new constructor signature; F-5b replaces its body).

| Finding | Verdict |
| --- | --- |
| B-11 (serviced-time deadline is service-global and never reset) | **RESOLVED (test: `c0_2ci_serviced_deadline_is_per_batch_not_global`, `c0_2ci_serviced_deadline_not_expired_on_first_poll_after_prior_service`, `c0_2ci_serviced_time_pauses_during_seat_inactive_and_expires`)** — `CoreRetirementBatch` now carries `serviced_deadline: Option<Duration>`, stamped by `ResourceService::register_batch` as `serviced_elapsed.checked_add(max_serviced_duration)` at registration time. `service_completions` filters `pending_batches` for entries whose own deadline has passed, quarantines only those, and never sets `exhausted`. `set_seat_active` is now driven from `on_vt_release`/`on_vt_acquire` and both `set_dpms_power` transitions in `backend.rs` (inert without a `resource_service`, per R8). Mutation check performed: reverting `register_batch`/`service_completions` to the pre-fix single `self.serviced_elapsed >= self.max_serviced_duration` global comparison makes `c0_2ci_serviced_deadline_is_per_batch_not_global` fail (batch 2 would expire alongside batch 1 at t=55ms instead of surviving to t=90ms) |
| M-16 (`next_deadline` returns `None` while the seat is inactive, suppressing progress rather than only the budget) | **RESOLVED (test: `c0_2ci_progress_no_composition` (updated), `c0_2ci_progress_no_composition_on_core_loop_fake_backend` in `backend.rs`)** — `next_deadline` no longer checks `seat_active`; only `service_completions`'s `serviced_elapsed` advance is gated on it. The new backend-level test drives the real `KmsBackend::for_tests()` fixture (the `Backend` implementor `run_core` actually calls) with VT Suspended, DPMS off and no scene damage, asserts `next_wakeup()` still schedules the pending ticket's deadline, drives the real `before_block` completion callback twice (unsignaled, then signalled), and asserts the allocation becomes available with no scene submission (`composite_and_flip`/`maybe_composite` never called; `scanout_allowed()`/`kms_outputs_active`/`scene_structure_dirty` all confirm every composition gate stayed closed throughout) |
| M-13 (`begin_quiescing`'s Busy inputs are free-floating setters) | **RESOLVED (test: `c0_2ci_transport_gate_direct_scanout_precondition`)** — `set_direct_scanout_active`/`set_unflip_pending` are deleted; `TransportGate` now takes a `Box<dyn DirectOwnershipState>` at construction (`new_legacy`'s third argument) and `begin_quiescing` queries `direct_ownership_busy()`/`unflip_outstanding()` live on every call. `#[cfg(test)] FakeDirectOwnershipState` records a query count per method so the test can assert the gate actually consulted live state rather than a cached value. The real, non-test implementor is `DirectOwnershipSignal` (shared `Rc<Cell<bool>>` pair) — a correctly-typed adapter with no production caller yet, exactly like `OwnerWriteGrant`'s issuer (R8): wiring its `set_direct_ownership_busy`/`set_unflip_outstanding` calls into the real direct-scanout/unflip transition sites (`backend.rs`'s `ScanoutM2State`) is DRM-sink territory and belongs to F-5b or later, not this session. Mutation check performed: reverting to the pre-fix setters would still pass the busy/unblocked assertions by construction, but `ownership.busy_query_count()`/`unflip_query_count()` would stay 0, failing the test |
| M-14 (`close()` doesn't refuse with outstanding grants; `issue_handover_permit` takes neither `LegacyDrained` nor final dispositions; `try_finish_legacy_transport` unconnected) | **RESOLVED (test: `c0_2ci_transport_gate_close_refuses_outstanding_grants`, `c0_2ci_transport_gate_handover_validates_proof_and_dispositions`, `legacy_transport_gate_permits_finish_inert_without_a_gate_and_refuses_past_legacy` in `platform.rs`)** — `close()` now returns `Result<(), ResourceError>` and refuses with `Busy` while `outstanding_owner_writes() != 0` (the foreign-proof emergency path in `consume_owner_write` uses a new private `force_close()` instead, since that closure must be unconditional). `issue_handover_permit` takes the real `platform::LegacyDrained` proof and `&[backend::LegacyEventDisposition]`, refusing `WrongIncarnation` for a foreign incarnation and `InvalidProof` for any `BackendFailure` disposition. `try_finish_legacy_transport` (`backend.rs`) now calls the new `PlatformBackend::legacy_transport_gate_permits_finish` before calling `owner.finish_legacy_transport`, refusing when an installed gate has already progressed past `Legacy`/`Quiescing`; inert under Legacy since no gate is installed in production (R8). Full end-to-end coverage of `try_finish_legacy_transport` itself (with a live `DeviceCommitOwner`/backend fixture) does not exist as a baseline and was not built this session — the new test instead proves the gate-check predicate directly and confirms by inspection that it gates the real call site; building the full integration fixture is judged out of proportion for this finding and is flagged here rather than silently left uncovered |
| B-6 (`RecipientReservation::new_for_tests` is a production constructor) | **RESOLVED (test: existing `c0_2ci_handoff_*` tests plus the whole `c0_2ci` suite compiling with `RetainingSupervisor` under `#[cfg(test)]`)** — `RecipientReservation::new_for_tests` is back under `#[cfg(test)]`; `RetainingSupervisor` (struct, `Default`, and its whole `impl` including `reserve_slot`/`issue_teardown_release`) moves under `#[cfg(test)]` in `handoff.rs`, per the plan's own description of it as a test fixture. Nothing else in `handoff.rs` was touched (Task 9 is F-8's). Mutation check performed: reverting `RetainingSupervisor`'s `#[cfg(test)]` gate alone does not fail a test by itself (it is a visibility fix, not a behavior change) — the decisive check is that `cargo check -p yserver --lib` (no `--tests`, no `cfg(test)`) still compiles with `RecipientReservation::new_for_tests` gone; it does, since nothing outside `#[cfg(test)]` code calls it any more |

Also fixed in this session: a pre-existing test-ordering bug in
`adapter_tests.rs`'s `c0_2ci_adapter_vt_away_dpms_off_idle_service_progress`,
surfaced (not introduced) by the B-11 change — it set
`max_serviced_duration` **after** `register_batch`, which the old
global-comparison code tolerated (it re-read the field on every poll) but
the new per-batch-deadline-at-registration design does not; reordered to
set the budget first, matching the fixed contract.

Ticked: 6.1 (`c0_2ci_progress_no_composition_on_core_loop_fake_backend`),
6.3 (per-batch serviced deadline + VT/DPMS `set_seat_active` wiring +
already-existing unconditional `before_block`/`next_wakeup` chaining,
proven by the B-11 tests above), 6.5
(`c0_2ci_transport_gate_direct_scanout_precondition`,
`c0_2ci_transport_gate_close_refuses_outstanding_grants`,
`c0_2ci_transport_gate_handover_validates_proof_and_dispositions`,
`legacy_transport_gate_permits_finish_inert_without_a_gate_and_refuses_past_legacy`).
6.5a/6.5b remain unticked — F-5b's.

Gate for this round: `cargo +nightly fmt --check` clean; `cargo clippy
--all-targets -- -D warnings` clean; `cargo test -p yserver --lib c0_2ci`
93 passed/0 failed/10 ignored on a clean run and on twelve consecutive
runs (zero flakes); `cargo test -p yserver --lib c0_2ci -- --ignored` 10
passed/0 failed (this box's real DRM node + NVIDIA/RADV ICDs); full
`cargo test -p yserver --lib` 1630 passed/1 failed — the one failure is
`kms::executor::device_lock::tests::the_lock_is_released_when_the_holder_dies`,
one of R2's three pre-existing executor flakes (unrelated file, this
session touches no code under `kms/executor/`); `cargo check -p yserver
--target x86_64-unknown-linux-musl` and `--target x86_64-unknown-freebsd`
both clean.

```
$ cargo test -p yserver --lib c0_2ci -- --ignored
running 10 tests
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::resources::tests::c0_2ci_descriptor_reset_exclusion_until_gpu_signaled_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_no_premature_pool_return_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_dri3_lease_regressions_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan ... ok
test kms::render::backend::tests::c0_2ci_read_source_scratch_regression_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_dropped_frame_metadata_with_live_ticket_vulkan ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1703 filtered out; finished in 0.77s
```

**Fix round 2 (F-5b): `842745a3`.** Session F-5b, the sinks half of F-5,
closing B-10 and F5a-M1 (`docs/superpowers/findings/2026-09-11-stage-2c-i-
implementation-review-round1.md`, `docs/superpowers/findings/2026-09-12-
stage-2c-i-fix-F5a-review.md`) per `docs/handoff-phase-c0-stage-2c-i-fix.md`'s
"F-5 — Task 6" section. This session resumed a prior attempt that left the
tree dirty and non-compiling (`KmsIoExecutor::send`/
`dispatch_blocking_at_boundary` had grown a third parameter naming the
`pub(crate)` `TransportGate`/`OwnerWriteGrant` types, but the two calls in
`tests/executor_async.rs` — an external integration-test crate — were
never updated, and the same types leaking into a publicly reachable
signature is itself a `private_interfaces` warning that `-D warnings`
turns into a build failure). Fixed by keeping `send`/
`dispatch_blocking_at_boundary` at their original 2-arg public signature
(every real and test call site outside this file is unchanged, including
`tests/executor_async.rs`) and moving the gate-carrying body to new
`pub(crate)` `send_authorized`/`dispatch_blocking_at_boundary_authorized`,
called with `None` from the public wrappers.

| Finding | Verdict |
| --- | --- |
| B-10 (Task 6, R11 — the transport gate is enforced at zero sinks) | **RESOLVED (tests: `c0_2ci_sink_legacy_page_flip_gate_four_states`, `c0_2ci_sink_direct_atomic_flip_gate_four_states`, `c0_2ci_sink_composed_unflip_gate_four_states`, `c0_2ci_sink_modeset_install_gate_four_states`, `c0_2ci_sink_output_disable_gate_four_states`, `c0_2ci_sink_cursor_gate_four_states` in `platform.rs`, `c0_2ci_sink_helper_mutation_gate_four_way`)** for six of the eight code-bearing rows in the R11 inventory below; **DEFERRED** for gamma (code done, test not built — see the inventory's Gamma row and the note beneath it) |
| F5a-M1 (`DirectOwnershipState`'s only implementor is a free-floating `Cell` pair nobody sets) | **RESOLVED (test: `c0_2ci_scanout_m2_ownership_handle_reflects_real_backend_state` in `backend.rs`)** — `DirectOwnershipSignal` deleted; `ScanoutM2OwnershipHandle` (a clone of `ScanoutM2State`'s own live cells) is the real implementor, kept in sync by `sync_ownership()` called at every `current`/`pending`/`queued_successor`/`unflip_requested` mutation site. `FakeDirectOwnershipState` (test-only) is unchanged |
| Minor (`consume_owner_write` masks accounting bugs with `if outstanding > 0 { -= 1 }`) | **RESOLVED (test: `c0_2ci_transport_gate_consume_owner_write_checked_subtraction`)** — checked subtraction, `ResourceError::InvalidState` on underflow instead of silently doing nothing |

Mutation checks performed and reverted: (1) deleting `disable_output`'s
`if !legacy_write_permitted { return Err(...) }` makes
`c0_2ci_sink_output_disable_gate_four_states` fail (Quiescing/Owner/Closed
all start reaching the real ioctl); (2) deleting the `authorize_write` call
in `send_authorized` makes `c0_2ci_sink_helper_mutation_gate_four_way` fail
(the Quiescing case dispatches instead of refusing); (3) making
`consume_owner_write` return `Ok(())` without decrementing
`outstanding_owner_writes` makes the same test's
"grant must be consumed at send" assertion fail (`left: 1, right: 0`).

### R11 sink inventory (this session's; supersedes the reviewer's table)

| Sink | Real entry point(s) | Class | Gated/Observational/Cleanup | Test |
| --- | --- | --- | --- | --- |
| Legacy page flip (composed) | `drm/page_flip.rs::submit_flip_with_fences` | `Primary` | **Gated** — checked immediately before `atomic_commit`; callers `scene.rs::submit_shared_scanout_frame`, `platform.rs::submit_copied_scanout` compute the real check via `PlatformBackend::allows_legacy` | `c0_2ci_sink_legacy_page_flip_gate_four_states` |
| Direct atomic flip | `drm/modeset.rs::submit_direct_scanout` | `Primary` | **Gated** — caller `backend.rs::submit_direct_frame` | `c0_2ci_sink_direct_atomic_flip_gate_four_states` |
| Composed unflip | `drm/modeset.rs::submit_composed_scanout` | `Unflip` | **Gated** — caller `backend.rs::submit_composed_unflip` | `c0_2ci_sink_composed_unflip_gate_four_states` |
| Modeset install | `drm/modeset.rs::commit_modeset` | `Modeset`/`Dpms` | **Gated** — callers: `platform.rs::replay_copy_free_scanout_plan`/`replay_copied_scanout_plan` (via `allocate_copy_free_scanout_pool`/`allocate_copied_scanout_pool`, reached with `commit_first_framebuffer=true` only from `enable_connector_inner`; the `PlatformBackend::new` bring-up and `prepare_qualified_connector_plan` call sites pass `commit_first_framebuffer=false`, so `commit_modeset` — and the `bool` value passed for it — is unreachable there, documented at each site), `enable_connector_inner`'s own direct commit, `dpms_set_outputs_active(true)` | `c0_2ci_sink_modeset_install_gate_four_states` |
| Test-only modeset | `drm/modeset.rs::{test_modeset, test_modeset_strict}` | — | **Observational, no gate** — `TEST_ONLY` atomic commits never latch hardware state (probe-only, per the plan's own 6.5a table: "read-only probe portions may run; installation cannot") | — |
| Output disable | `drm/modeset.rs::disable_output` | `Modeset`/`Dpms` | **Gated** — callers `platform.rs::disable_connector`, `PlatformBackend::disable_output` (post-loop teardown), `dpms_set_outputs_active(false)` | `c0_2ci_sink_output_disable_gate_four_states` |
| Startup rollback | same `disable_output` | — | **Gated at the sink** (identical check) — callers `kms/backend.rs::activate_initial_scanout_outputs`'s bring-up rollback loop, `PlatformBackend`'s `Drop` impl and `open_with_commit`'s `InitialScanoutRollbackGuard`: all three run before any `PlatformBackend` (hence any transport gate) exists, so they pass `true` literally, with a comment at each site citing R8 | covered by `disable_output`'s test above (same function; these callers are construction-time-only and cannot install a gate to exercise) |
| Cursor set/move | `kms/cursor_plane.rs::CursorPlane::{show,hide,move_to}`, gated at their `PlatformBackend` callers `cursor_plane_show_on_crtc`, `try_cursor_plane_move_for_device` (shared by `cursor_plane_move`/`cursor_plane_drain_pending_move_for_output`), `cursor_plane_hide_on_crtc`, `cursor_plane_hide_all` | `Cursor` | **Gated** — all four share the identical `self.allows_legacy(&device_key, WriterClass::Cursor)` guard immediately before their ioctl | `c0_2ci_sink_cursor_gate_four_states` drives `cursor_plane_hide_on_crtc` (the one entry point whose ioctl doesn't need a real dumb buffer or pre-marked-visible CRTC to reach a real fd) through all four states; show/move/hide_all are covered by code inspection of the identical guard, not a separate four-way run |
| Gamma | `backend.rs::apply_gamma_to_live_output` (called by `set_crtc_gamma`, `reapply_gamma_for_output`, `reapply_gamma_for_live_outputs`) | `Gamma` | **Gated in code**, immediately before `Device::set_gamma` — **no deterministic test**: `apply_gamma_to_live_output` requires `live_crtc_and_gamma_size` to succeed first, and that function's own `device.get_crtc(crtc)` read fails on `Device::for_tests()`'s socket fd (confirmed empirically: `Custom { kind: Other, error: "... Inappropriate ioctl for device (os error 25)" }`) *regardless of gate state*, so the gate check is unreachable through this fixture in any state, not just the refused ones. A `_drm` hardware test would need to acquire real DRM master on this box's live display to get `get_crtc` past that first read, which is unsafe/disruptive to attempt from an unattended fix session — **F8: stopping here rather than shipping a fabricated pass.** | none |
| Helper mutation (owner atomic) | `executor/mod.rs::send_authorized` (the `pub(crate)` body of the public `send`/`dispatch_blocking_at_boundary`) | `HelperMutation` | **Gated, and the only sink that actually consumes an `OwnerWriteGrant`** at the serialized send boundary (R7) — every real production call site (`owner/device.rs`'s four producers) passes `None`, per R8 | `c0_2ci_sink_helper_mutation_gate_four_way` |
| Vblank sequence arm | `drm/page_flip.rs::drm_crtc_queue_sequence` | — | **Observational, no gate** — `DRM_IOCTL_CRTC_QUEUE_SEQUENCE` is a read (queries a future vblank sequence number), not a state mutation; not in the plan's 6.5a table | — |
| FB removal / GEM close | `drm_cleanup.rs`, `buffer.rs`, `vk/scanout.rs`, `modeset.rs` payload destructors | — | **Cleanup class, governed by Task-2 rights** (`DrmCleanupRight`/`GemOwner`), not the transport gate — per R11's own text | — (Task 2's own tests) |

Left unticked (F1/F8): 6.5a and 6.5b stay `- [ ]` because gamma's proof is
deferred, not because the mechanism is missing — six of eight code-bearing
rows are proven with a test that would fail on the pre-fix tree, gamma's
code is done and its check is real (the row is not "ungated," it is
"unproven" for a fixture reason, not a design one) but has no test, and a
step with even one unproven row does not get ticked. 6.5b's content (the
`c0_2ci_sink_helper_mutation_gate_four_way` grant-consumption test above is
6.5b's four-way — "no grant"/"wrong class" collapse into the same
`ResourceError::InvalidProof` path since a class mismatch is the only way
this stage's fixtures can present "no valid grant"; a foreign device/
incarnation case is already covered by the pre-existing
`c0_2ci_transport_gate_owner_write_contract`, unchanged this session).

Gate for this round: `cargo +nightly fmt --check` clean; `cargo clippy
--all-targets -- -D warnings` clean; `cargo test -p yserver --lib c0_2ci`
101 passed/0 failed/10 ignored on a clean run and on twelve consecutive
runs (zero flakes); `cargo test -p yserver --lib c0_2ci -- --ignored` 10
passed/0 failed (this box's real DRM node + NVIDIA/RADV ICDs, unchanged
from F-5a's run); full `cargo test -p yserver --lib` 1639 passed/0 failed
(no R2 flake observed this run); `cargo check -p yserver --target
x86_64-unknown-linux-musl` and `--target x86_64-unknown-freebsd` both
clean.

```
$ cargo test -p yserver --lib c0_2ci -- --ignored
running 10 tests
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::resources::tests::c0_2ci_descriptor_reset_exclusion_until_gpu_signaled_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_no_premature_pool_return_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_dri3_lease_regressions_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan ... ok
test kms::render::backend::tests::c0_2ci_read_source_scratch_regression_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_dropped_frame_metadata_with_live_ticket_vulkan ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1711 filtered out; finished in 0.75s
```

**Files:** Create `resources/completion.rs` and `resources/transport.rs`; modify `backend.rs` and `platform.rs`. Extend `crates/yserver-core/src/core_loop/run.rs` tests only if needed to observe the existing completion callback.

**Consumes:** Service pending tickets; current `before_block`, `on_owner_completion_ready`, `next_wakeup`, `owner_completion_deadline`, `service_owner_completions`, scanout completion registrations and executor-control processing.

**Produces:** Service scheduling and per-device `TransportGate` with the following fixed vocabulary:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportState { Legacy, Quiescing, Owner, Closed }
/// Single-use authority for one owner-mediated mutating dispatch.
/// Not `Clone`, not `Copy`, no `Default`, no constructor outside `transport`.
#[derive(Debug)]
pub(crate) struct OwnerWriteGrant {
    device: DrmDeviceKey,
    incarnation: IncarnationId,
    class: WriterClass,
    serial: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriterClass {
    Primary, Unflip, Modeset, Dpms, Vt, Topology, Cursor, Gamma, HelperMutation,
}
impl TransportGate {
    pub(crate) fn new_legacy(device: DrmDeviceKey, incarnation: IncarnationId) -> Self;
    pub(crate) fn begin_quiescing(&mut self) -> Result<(), ResourceError>;
    pub(crate) fn close(&mut self);
    pub(crate) fn state(&self) -> TransportState;
    pub(crate) fn allows_legacy(&self, class: WriterClass) -> bool;
    pub(crate) fn authorize_owner_write(&mut self, class: WriterClass)
        -> Result<OwnerWriteGrant, ResourceError>;
    pub(crate) fn consume_owner_write(&mut self, grant: OwnerWriteGrant)
        -> Result<(), (ResourceError, OwnerWriteGrant)>;
    pub(crate) fn outstanding_owner_writes(&self) -> usize;
    pub(crate) fn revoke_owner_writes(&mut self) -> usize;
}
impl ResourceService {
    pub(crate) fn next_deadline(&self) -> Option<std::time::Instant>;
    pub(crate) fn service_completions(&mut self, now: std::time::Instant)
        -> Result<Vec<AllocationKey>, ResourceError>;
}
```

`TransportGate` stores device/incarnation, state and outstanding helper permissions. Define `new_legacy`, `begin_quiescing`, `close`, `state` and `allows_legacy(WriterClass) -> bool`. Publication of Owner requires the consumed existing `LegacyDrained` proof, all final event dispositions, a recipient reservation and a complete writer-coverage set. Declare the opaque `RecipientReservation` capability in this task, with a test constructor; Task 9 adds the only future supervisor-side issuer and transfer implementation. Do not add a production issuer in 2c-i. In 2c-i the constructor for production writer-coverage proof is absent; tests can construct it only after explicit mock/disabled coverage for every class. No environment switch enables production Owner. Existing `platform::LegacyDrained` is Copy and carries only incarnation/lifecycle identity; it is not the complete authority. Add a non-Clone `HandoverPermit` issued privately after the backend applies final drain dispositions and the gate validates writer coverage, recipient reservation and helper revocation. The serialized publication consumes that permit and changes both gate/owner route together. A copied `LegacyDrained` value cannot replay a prior transition or independently activate Owner.

**Owner-writer authority contract (plan-review round-2 M-1).** `HandoverPermit` authorizes publishing the route; it never authorizes an individual mutation, and `state() == TransportState::Owner` is not blanket permission. Every sink in the 6.5a table needs its own `OwnerWriteGrant` once the route is Owner, checked at the same final boundary where `allows_legacy` is checked today:

- **Issuer.** Only `TransportGate::authorize_owner_write`, on the serialized core path, and only while `state() == Owner`; `Legacy`, `Quiescing` and `Closed` return `ResourceError::Detached`. There is no production issuer in 2c-i: no production writer can reach an Owner route because publication itself stays test-only, so this defines the interface without activating it.
- **Binding.** The grant carries the gate's own `DrmDeviceKey` and `IncarnationId`, the requested `WriterClass`, and a checked-monotonic serial. The sink validates all four against its own device/incarnation/class before dispatch; a mismatch dispatches nothing, returns `ResourceError::WrongIncarnation`, and closes the transport, exactly as a foreign proof closes a converted route (Task 1).
- **Consumption.** One grant authorizes exactly one dispatch, consumed by value through `consume_owner_write` **at the serialized send boundary — when the executor accepts the request** (round-4 M-2). From that point the request's unknown outcome belongs to the owner's quarantine, not to the gate count; a stalled executor therefore never pins `outstanding_owner_writes()`. Serials are never reissued, so a replayed or reconstructed grant is rejected. A grant that is dropped instead of consumed authorizes nothing and does not decrement the outstanding count; the gate keeps the charge and closes admission, mirroring the lost-role-token rule in Task 8. Dropping is never the release path.
- **Revocation and ordering.** `begin_quiescing`, `close` and handover refuse while `outstanding_owner_writes() != 0`; `revoke_owner_writes` is the explicit resolution and returns the count it invalidated. This extends the existing rule that Owner cannot publish while a helper permission or disposition is outstanding. `WriterClass::HelperMutation` grants additionally resolve or revoke the issued helper permission before handover.
- **Scope.** Concrete stage-3/4 producers of these grants remain deferred; this stage supplies the vocabulary, the validation site and the tests only.

- [x] **6.1 Add a no-composition progress test.** Use the existing core-loop fake backend completion tests. Register one unsignaled ticket, set VT-away/DPMS-off and no damage, then signal it through the adapter and verify the service runs, its allocation becomes available and no scene submission occurs. Assert a pending ticket schedules a future deadline, and a failed ticket closes the route without repeated immediate deadlines.
- [x] **6.2 Run** `cargo test -p yserver --lib c0_2ci_progress` and the core-loop completion tests.
- [x] **6.3 Move service polling outside composition gates.** Invoke service work from the established completion callback and `before_block`, before any scene/VT/DPMS early return. Chain `ResourceService::next_deadline()` unconditionally in `next_wakeup`. For tickets without exportable FDs use a **1 ms** positive retry interval, coalesced to one service deadline; successful evidence services availability in the same wake. Each such ticket carries a bounded pending deadline derived with checked arithmetic (round-3 m-1), measured in **serviced** time: it pauses while the seat is inactive (VT-away, DPMS-off) and resumes on return, so a long VT switch is not a route-closing event (round-4 m-2). On expiry the batch is frozen, converted admission closes and the retry is not re-armed. Expiry is never a completion proof: the batch stays rooted for teardown like a failed ticket. Checked time overflow closes managed admission and retains work. Failed/device-lost tickets are retained for teardown rather than polled forever. Keep existing FD pollers; the serialized inbox does not need a new OS thread or synthetic ready FD.
- [x] **6.4 Register waiter before rechecking availability.** A waiter is keyed by allocation generation and consumer (`Pool`, `DirectCapacity`); define this two-variant enum in `completion.rs`. Store a set to coalesce wake notifications. On reserve failure register the consumer and recheck in the same service turn. On an eligibility edge enqueue one consumer wake and clear its registration; consumer retries reserve, not an unchecked index acquisition. Completion arriving during registration must either be observed by recheck or produce the wake.
- [x] **6.5 Implement and test the gate.** Gate initial state is Legacy. Quiescing revokes all new legacy writer permissions before issuing the drain; Owner cannot publish while any helper permission or disposition is outstanding. **Precondition (round-3 M-2):** `begin_quiescing` returns `ResourceError::Busy` while any direct ownership unit is `Current`, `Submitted` or `Successor`, or while an unflip is requested and not retired. Exiting direct scanout is the last legacy write and precedes quiescing; `Unflip` is not a class `Quiescing` permits, so the all-classes-false assertions stand unchanged and no sink gains a bypass. Add a test that `begin_quiescing` under active direct scanout refuses without changing state, and succeeds after the unflip retires. Closed cannot return to Legacy on the same incarnation. Add table-driven tests:

```rust
for class in [WriterClass::Primary, WriterClass::Unflip, WriterClass::Modeset,
    WriterClass::Dpms, WriterClass::Vt, WriterClass::Topology,
    WriterClass::Cursor, WriterClass::Gamma, WriterClass::HelperMutation] {
    assert!(!gate.allows_legacy(class));
}
```

Run this assertion after `begin_quiescing`, after test-only Owner publication and after `close`. Connect `try_finish_legacy_transport` to the gate: without all production prerequisites it returns a refusal before changing the owner route. Existing tests explicitly supply complete mock writer coverage. Full primary/lifecycle/cursor/gamma conversion remains in later stages; no partially converted production Owner is reachable.
- [ ] **6.5a Enforce the gate at actual writer boundaries (plan-review M-2).** The enum-only test above is necessary but insufficient. Wire authorization before the first transport/helper mutation in every row below, then call those real entry points under Quiescing, test-only Owner and Closed with a counting transport. Assert zero *legacy* ioctl/helper dispatch in all three states and an unchanged unrelated device. Test-only owner-mediated dispatch uses an `OwnerWriteGrant` of its own class under the contract above; production remains Legacy until the later implementations exist.

| Class | Concrete entry points in the integrated baseline | Transport sink / mandatory test observation |
| --- | --- | --- |
| Composed/direct primary and unflip | `scene.rs` shared submission to `submit_flip_with_fences`; `PlatformBackend::submit_copied_scanout`; `KmsBackend::submit_direct_frame`, `submit_composed_unflip` | Count page-flip/atomic writes, including queued retries; no ungated submission. |
| Modeset, connector and topology install | `PlatformBackend::disable_connector`, `enable_connector`, `enable_connector_with_qualified_plan`, `install_prepared_connector_plan`, `enable_connector_inner`, `apply_connector_snapshot`; backend `finish_crtc_config` / `apply_crtc_config` | Count `drm::modeset` commit/disable calls and helper dispatch. Read-only probe portions may run; installation cannot. |
| Shutdown / DPMS / VT | Platform `disable_output`, `dpms_set_outputs_active`, `reset_scanout_bos_for_suspend`; backend `drive_vt_event`, `on_vt_release`, `on_vt_acquire` | Guard actual KMS mutation and reset/install paths; preserve necessary terminal seat cleanup through its separately authorized lifecycle boundary. Closing a gate is not authorization for a legacy disable ioctl. |
| Cursor upload/show/move/hide/detach | Platform `cursor_plane_upload_image_for_output`, `cursor_plane_show_on_crtc`, `cursor_plane_move`, `cursor_plane_drain_pending_move_for_output`, `try_cursor_plane_move_for_device`, `cursor_plane_hide_on_crtc`, `cursor_plane_hide_all`; `kms/cursor_plane.rs` device operations | Exercise deferred moves, rollback and hide paths; no `set_cursor2`/`move_cursor` or mutable cursor backing access bypasses its applicable authority. |
| Gamma | Backend `set_crtc_gamma`, `apply_gamma_to_live_output`, `reapply_gamma_for_output`, `reapply_gamma_for_live_outputs` | Count `Device::set_gamma`; retries/reapplication must use the same gate. |
| Helper mutation | `KmsIoExecutor::send`, `dispatch_blocking_at_boundary` and platform/backend wrappers that construct those requests | Resolve or revoke issued helper permission before handover; block unpermitted mutating requests at dispatch, with no nested legacy fallback. |

These are baseline anchors, not permission to stop searching: follow each sink's callers, including startup rollback/cursor restoration, and classify all discovered paths. The test transport belongs beneath the real entry point; do not replace the entry point itself with a mock that merely calls `allows_legacy`. Keep signature adaptations within this task's files and add `kms/cursor_plane.rs` and the executor dispatch boundary where required.

- [ ] **6.5b Test owner-write authorization at the same sinks (plan-review round-2 M-1).** Reuse the 6.5a counting transport under test-only Owner and drive every row's real entry point four ways: with no grant, with a grant of a different `WriterClass`, with a grant carrying another device/incarnation, and with the correct grant. Assert zero dispatch in the first three, `ResourceError::WrongIncarnation` plus a closed transport for the mismatched bindings, and exactly one dispatch for the correct grant. Assert the consumed grant cannot authorize a second dispatch and that a reconstructed serial is rejected. Assert `begin_quiescing`, `close` and handover all refuse while a grant is outstanding, that a dropped grant leaves `outstanding_owner_writes()` charged and closes admission, and that `revoke_owner_writes` is the only path that clears it.

- [x] **6.6 Run** focused and existing handover/core completion tests, format and clippy. Commit with `feat(kms): service resource completions independently of composition`.

## Task 7: Concrete commit resources and Present dispositions

**Status: EXECUTED at `4363a9b2`.** **Review round 1 (2026-09-11): REJECTED** — see the findings and `docs/handoff-phase-c0-stage-2c-i-fix.md`; unchecked steps below are not done or not proven.

**Fix round 1 (F-6a): `c35af190`.** Session F-6a, consumer half of Task 7, closing B-8, B-9, M-2, M-3, M-4, M-5, M-15, and minor `GroupMember::validate_unique` (`docs/superpowers/findings/2026-09-11-stage-2c-i-implementation-review-round1.md`) per `docs/handoff-phase-c0-stage-2c-i-fix.md`'s "F-6 — Task 7" section and `docs/handoff-phase-c0-stage-2c-i-fix-resume.md`. Task 7's Present half (M-6: 7.5, 7.5a `release_present_source`, `retained_present_wakes`, COW tests in `backend.rs`) is deferred to session F-6b.

| Finding | Verdict |
| --- | --- |
| B-8 (`Terminal` blindly freezes regardless of cause) | **RESOLVED (tests: `c0_2ci_commit_terminal_completed_does_not_freeze_and_becomes_releasable`, `c0_2ci_commit_terminal_failed_before_submit_does_not_freeze_current_set`, `c0_2ci_commit_terminal_completion_unknown_freezes_only_that_commit`)** — `Terminal { commit, terminal }` matches on cause: `Completed` and `FailedBeforeSubmit` do not freeze; only `CompletionUnknown` freezes, and strictly for entries matching `commit_id == Some(commit)` |
| B-9 (Task 7.6 fabricated tests, no real `CommitResources` or owner-event proofs) | **RESOLVED (tests: `c0_2ci_commit_hardware_complete_discharges_old_only`, `c0_2ci_commit_resources_still_current_cancels_not_discharges`, `c0_2ci_commit_topology_replacement_reused_numeric_crtc`, `c0_2ci_commit_grouped_skip_and_duplicate_protection`, `c0_2ci_commit_presented_selects_reference_crtc_sample`)** — real `CommitResources` old/new driven through `CompletionRetired`, obligations registered via `register_kms`, zero `apply_validated_proof` calls in test bodies, verified `has_pending_obligations` on `new` set after `HardwareComplete`, verified `kms_disposition` after cancel is not `Discharged`, verified reference CRTC clock selection on `Presented` with two outputs, and verified Skip / duplicate notifications do not double-release |
| M-2 (Inversion between `HardwareComplete` and `CompletionRetired` drops obligations) | **RESOLVED (test: `c0_2ci_commit_hardware_complete_discharges_old_only`)** — `CommitResourceConsumer` tracks `commit_members: BTreeMap<CommitId, Vec<GroupMember>>` populated at registration / retired completion, supporting symmetric arrival order without dropping obligations |
| M-3 (`in_flight` and `correlate_commit` duplicate state; unkeyed `crtcs` loop) | **RESOLVED** — deleted `in_flight` and `correlate_commit` entirely. KMS obligations are tracked solely via `(AllocationKey, ObligationId, GroupMember)` triples on `CommitResources::kms_obligations` and discharged strictly by matching `GroupMember` |
| M-4 (`discharge_commit_kms_obligations` applies mid-loop with `?` without pre-validation) | **RESOLVED (test: `c0_2ci_commit_discharge_atomic_validate_then_apply_failure_rolls_back`)** — atomic two-pass check: `validate_proof_target` checks all obligations before `apply_validated_proof` applies any of them; failure rolls back cleanly without leaving partial discharges |
| M-5 (Missing displaced-pair registration adapter before IPC; missing `take_current()`) | **RESOLVED (test: `c0_2ci_commit_register_dependencies_and_pre_ipc_cancellation`)** — implemented `register_commit_dependencies` matching `(allocation, member)` where `new[member] != old[member]`, `cancel_pre_ipc_commit` for exact resource return and registration ownership cancellation on pre-IPC failure, and `take_current()` |
| M-6 (Task 7.5 / 7.5a Present release half) | **RESOLVED (tests: `c0_2ci_present_split_source_fallback_pin_ownership_and_release`, `c0_2ci_present_retained_wakes_move_into_present_release_and_signal`, `c0_2ci_present_release_consumption_and_completion_suppression`, `c0_2ci_cow_deferred_release_and_reclaim_with_physical_contracts`)** — `PresentPinEntry` owns `Option<StorageLease>` and `id: DrawableId`; `release_present_source` drops pin once with `store_decref_with_invalidate`; `retained_present_wakes` moves pinned wake into `PresentRelease` without XID re-lookup; `CommitResourceConsumer` wires `Presented` to emit completion with release retained, `FailedBeforeSubmit` suppresses completion with release retained, and `on_available` drains releasable `PresentRelease` as released; verified complete 6-part physical retirement and re-claim contract without boolean hand-setting |
| M-15 (`Quarantined` must freeze only this commit's entries and close gate) | **RESOLVED (test: `c0_2ci_commit_quarantined_closes_gate_and_freezes_only_that_commit`)** — `Quarantined { commit }` closes the consumer's `gate_handle` and freezes strictly the entries where `commit_id == Some(commit)` |
| Minor (`GroupMember::validate_unique` missing) | **RESOLVED (test: `c0_2ci_commit_group_member_validate_unique`)** — `GroupMember::validate_unique` validates CRTC key uniqueness within member sets |

**Fix round 2 (F-6b): `8b0e00d6`.** Session F-6b, Present release half of Task 7, closing M-6 (Tasks 7.5 and 7.5a) per `docs/handoff-phase-c0-stage-2c-i-fix.md` and `docs/superpowers/findings/2026-09-12-stage-2c-i-fix-F6a-review.md`.

Mutation checks performed and reverted in F-6b:
1. Mutating `release_present_source` by omitting `store_decref_with_invalidate` caused `c0_2ci_present_split_source_fallback_pin_ownership_and_release` to fail (`assertion left == right failed: left: 2, right: 1`).
2. Mutating `make_present_release` by returning `None` wake instead of removing from `retained_present_wakes` caused `c0_2ci_present_retained_wakes_move_into_present_release_and_signal` to fail (`assertion failed: release.wake.is_some()`).
3. Mutating `consume` for `Terminal::FailedBeforeSubmit` to set `disp.release = ReleaseDisposition::Released` caused `c0_2ci_present_release_consumption_and_completion_suppression` to fail (`assertion left == right failed: left: Released, right: Retained`).

Gate for F-6b: `cargo +nightly fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean; `cargo test -p yserver --lib c0_2ci` 112 passed/0 failed/10 ignored on clean run and twelve consecutive runs (zero flakes); `cargo test -p yserver --lib c0_2ci -- --ignored` 10 passed/0 failed (hardware run); full `cargo test -p yserver --lib` 1650 passed/0 failed/82 ignored; `cargo check -p yserver --target x86_64-unknown-linux-musl` and `--target x86_64-unknown-freebsd` both clean.

```
$ cargo test -p yserver --lib c0_2ci -- --ignored
running 10 tests
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::store::tests::c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_dropped_frame_metadata_with_live_ticket_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_no_premature_pool_return_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_dri3_lease_regressions_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan ... ok
test kms::render::resources::tests::c0_2ci_descriptor_reset_exclusion_until_gpu_signaled_vulkan ... ok
test kms::render::backend::tests::c0_2ci_read_source_scratch_regression_vulkan ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1722 filtered out; finished in 0.79s
```

Mutation checks performed and reverted in F-6a:
1. Mutating `Terminal::Completed` to freeze entries caused `c0_2ci_commit_terminal_completed_does_not_freeze_and_becomes_releasable` to fail (`assertion failed: !service.is_frozen(&old_key)`).
2. Mutating `Quarantined` to omit `gate.close_gate()` caused `c0_2ci_commit_quarantined_closes_gate_and_freezes_only_that_commit` to fail (`assertion failed: gate_handle.is_closed()`).
3. Mutating `HardwareComplete` to a no-op discharge caused `c0_2ci_commit_hardware_complete_discharges_old_only` to fail (`assertion failed: !service.has_pending_obligations(&old_a_key)`).
4. Mutating `ResourcesStillCurrent` to discharge instead of cancel caused `c0_2ci_commit_resources_still_current_cancels_not_discharges` to fail (`assertion left != right failed: left: Some(Discharged), right: Some(Discharged)`).
5. Mutating `GroupMember::validate_unique` to unconditionally return `true` caused `c0_2ci_commit_group_member_validate_unique` to fail (`assertion failed: !GroupMember::validate_unique(&[m1, m2, m1_dup])`).
6. Mutating `discharge_commit_kms_obligations` to apply mid-loop without pre-validation caused `c0_2ci_commit_discharge_atomic_validate_then_apply_failure_rolls_back` to fail (`assertion failed: service.has_pending_obligations(&old_key)` on error rollback).

Gate for this round: `cargo +nightly fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean; `cargo test -p yserver --lib c0_2ci` 109 passed/0 failed/10 ignored on clean run and twelve consecutive runs (zero flakes); `cargo test -p yserver --lib c0_2ci -- --ignored` 10 passed/0 failed (hardware run); full `cargo test -p yserver --lib` 1647 passed/0 failed/82 ignored; `cargo check -p yserver --target x86_64-unknown-linux-musl` and `--target x86_64-unknown-freebsd` both clean.

```
$ cargo test -p yserver --lib c0_2ci -- --ignored
running 10 tests
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::store::tests::c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_dri3_lease_regressions_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_dropped_frame_metadata_with_live_ticket_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan ... ok
test kms::render::resources::tests::c0_2ci_descriptor_reset_exclusion_until_gpu_signaled_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_no_premature_pool_return_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::backend::tests::c0_2ci_read_source_scratch_regression_vulkan ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1719 filtered out; finished in 0.75s
```


**Files:** Create `resources/commit.rs`, `resources/present.rs`; modify backend/platform owner-event types, `present_completion.rs` and owner tests without adding backend dependencies to `kms/owner`.

**Consumes:** Typed leases, service, gate and existing `OwnerEvent<R>`, `Submitted<R>`, `Accepted<R>::into_parts()`. Existing backend event handling currently has a wildcard that ignores resource events because `NeverResource` is empty: replace that behavior for the concrete type.

**Produces:**

```rust
pub(crate) struct CommitResources {
    pub(crate) allocations: Vec<AllocationLease>,
    pub(crate) source: Option<StorageLease>,
    pub(crate) fallback: Option<StorageLease>,
    pub(crate) present: Option<PresentRelease>,
    pub(crate) crtcs: Vec<GroupMember>,
    /// One entry per displaced allocation and member, registered at dispatch;
    /// discharged by this commit's `HardwareComplete` (round-3 M-1).
    pub(crate) kms_obligations: Vec<(AllocationKey, ObligationId, GroupMember)>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GroupMember {
    pub(crate) crtc: crate::kms::render::platform::CrtcKey,
    pub(crate) topology_generation: u64,
    pub(crate) crtc_epoch: u64,
}
pub(crate) struct PresentRelease {
    pub(crate) event: yserver_core::backend::CompletedPresentEvent,
    pub(crate) wake: Option<crate::kms::render::present_completion::PinnedWake>,
}
```

`GroupMember` is captured from the commit's immutable membership before dispatch and validated for uniqueness. Match release evidence against these exact identities; never resolve old members through the current output vector/topology. A reused CRTC number at another generation cannot release the old record. Add a same-incarnation topology-replacement regression with reused numeric CRTC ID (plan-review m-1).

`PresentDisposition` records completion and release independently with two enums: `CompletionDisposition { Pending, Emitted, Suppressed }` and `ReleaseDisposition { Retained, Released }`. Key records by device/incarnation/commit and Present ID, not output index. Add `CommitResourceConsumer` holding current resources per supported ownership unit and pending old-generation release sets. It consumes `OwnerEvent<CommitResources>` through `consume(&mut self, event, service) -> Result<(), ResourceError>`; the concrete full parameter types are `OwnerEvent<CommitResources>` and `&mut ResourceService`.

- [x] **7.1 Add an owner integration test with actual leases.** Use existing `kms/owner/test_fixtures.rs` request/outcome fixtures but instantiate `DeviceCommitOwner<CommitResources>`. Submit old A/new B, generate HardwareComplete before Presented, then finish presentation and transfer the accepted resource event. Drop the event vector after consumption and assert A and B are still retained by their correct owner until each proof. Test the same sequence using both supported orders of page/fence evidence.
For a deterministic by-value consumer test, build `CommitResources` with `allocations: vec![lease]`, `source: None`, `fallback: None`, `present: None`, `kms_obligations` holding one registered triple per `old` allocation and member, and `crtcs` containing two `GroupMember` values with the fixture's stable CRTC keys, topology generation and CRTC epochs for each `old`/`new` fixture allocation. Keep their drop counters outside the resources. Exercise the consuming API directly as well as through owner evidence fixtures:

```rust
let accepted = crate::kms::owner::ledger::Submitted::new(vec![old], vec![new]).accepted();
consumer.consume(
    crate::kms::owner::device::OwnerEvent::CompletionRetired {
        commit: crate::kms::owner::identity::CommitId::for_tests(1),
        resources: accepted,
    },
    &mut service,
).unwrap();
assert_eq!(old_drops.get(), 0);
assert_eq!(new_drops.get(), 0);
```

Register the old KMS/GPU dependencies and the consumer's commit correlation before this fragment. The fixture retains no extra allocation lease solely to force the expected zero counts. With no extra references, the counters prove the consumer/retirement owners actually retained the payloads. Task 8 adds `direct_role: None` to these non-direct test constructors.

- [x] **7.2 Run** `cargo test -p yserver --lib c0_2ci_commit` before switching the backend boundary type.
- [x] **7.3 Replace backend/platform `NeverResource` parameterizations with `CommitResources`.** Include return signatures, `DispatchError` signatures, device constructors and test helpers. Retain the generic owner's resource-agnostic API and existing empty-resource Legacy behavior. Search both `kms::owner::NeverResource` and `owner::ledger::NeverResource`; the baseline contains separate declarations, so do not accidentally migrate only one spelling or delete a declaration still used by generic tests.
- [x] **7.4 Register commit dependencies before IPC and handle every resource event by value.** Register one correlated KMS obligation for each **displaced pair** — `(allocation, member)` with `new[member] != old[member]`; a member retained across a grouped commit registers nothing (round-4 m-1) — and retain all source/fallback uses before constructing `Submitted::new(old, new)` or sending the executor request; store the `(AllocationKey, ObligationId, GroupMember)` triples in `CommitResources::kms_obligations` so the consumer can correlate without a side table (round-3 M-1). Dependency identity includes commit/CRTC, not only `ObligationKind`. Pre-IPC failure returns the exact resources and registration ownership for proven cancellation. Preserve request-time `CompletedPresentEvent::crtc_id`, `crtc_epoch`, `msc_offset`, `window_generation` and reference `completion_clock`; do not reconstruct them from current topology.

 Implement this disposition table; Rust matches must be exhaustive for resource-bearing variants:

| Owner event/outcome | Concrete action |
| --- | --- |
| `HardwareComplete` | Record observation for future damage consumers; never extract or clone a releasing owner from the live record. **This is the `PriorBufferReleased` producer for this commit's `old` set** (round-3 M-1): the canonical out-fence proves the displaced buffers left their planes. For each `(AllocationKey, ObligationId, GroupMember)` registered at this commit's dispatch, discharge the obligation through `apply_validated_proof` when its `GroupMember` is in the completed commit's membership; unmatched members stay `Outstanding`. Nothing is discharged for the `new` set — a current buffer's own presentation never proves it idle. Discharging changes the entry's KMS disposition only; the lease stays in the live record until `CompletionRetired`. |
| `CompletionRetired { resources, .. }` | Call `into_parts()` once; move new into current, old into release waiting. Their KMS obligations and independent GPU/read/FOREIGN state remain explicit. |
| `ResourcesStillCurrent` | Restore old current resources after rejection; do not signal idle. Cancel — do not discharge — this commit's `KmsRelease` registrations: the displacement never happened, so the old buffers hold no pending obligation until the next displacing commit registers one. |
| `ResourcesReleased` | Remove only the rejected/never-current KMS obligation justified by this outcome; route new resources through remaining GPU/FOREIGN cleanup. |
| `Quarantined` / unknown | Close gate, freeze corresponding managed entries, retain both possible sets inside owner and later handoff. |
| `Presented` | Consume completion disposition using selected/reference CRTC sample; do not release source, fallback or wake. |
| `Terminal` | Complete/suppress protocol bookkeeping exactly once according to cause; uncertainty keeps release retained. |

- [x] **7.5 Split source/fallback pin ownership from numeric protocol handles.** For managed pins the table entry owns `StorageLease` and an invalidation-aware logical decref obligation. `release_present_source` removes that entry once and lets the service run the appropriate cleanup. `retained_present_wakes` moves the actual pinned object into `PresentRelease`. Never look up a reused XID to reconstruct it. Completion suppression consumes FIFO bookkeeping but cannot signal release.
- [x] **7.5a Preserve core-owned overlay claims.** `ServerState::cow_claims` is the only logical claim authority; `KmsCore::cow_refcount` no longer exists. Backend overlay methods receive only 0→1 and 1→0 edges. Managed COW/source/fallback leases retain physical allocation after a final logical release until safe unflip/replacement. Do not infer a protocol claim from a surviving lease, and do not clear the sticky failure or duplicate claim counts in the resource consumer.

**Physical-retirement transition (plan-review round-2 M-2).** The baseline already owns this edge in `KmsBackend::deferred_cow_release` ([backend.rs:1164](../../../crates/yserver/src/kms/render/backend.rs:1164)); keep it as the single transition, keyed by the retained identity (`cow_id`/allocation key) rather than by a generation counter, and add no second one in `CommitResourceConsumer`:

- **1→0 with `scanout_m2.active()`.** `materialize_direct_shadow_for_unflip` must succeed before anything is released; on `Err` the COW and its direct pins are untouched and core keeps the caller's claim ([backend.rs:20755](../../../crates/yserver/src/kms/render/backend.rs:20755)). On success request the unflip, set `deferred_cow_release`, and keep the retained `StorageLease` (plus any COW source/fallback lease) rooted. The logical release arms no KMS release obligation and discharges none.
- **0→1 while `deferred_cow_release` holds.** Reuse the retained `cow_id`/`StorageLease` identity: clear the flag, allocate no storage, perform no second import, create no second physical owner and no new allocation generation ([backend.rs:20632](../../../crates/yserver/src/kms/render/backend.rs:20632)). Only the protocol resource is materialized again, through `materialize_cow_resource`. Preserve the existing invariant that a live `cow_id` on this edge implies a deferred release.
- **Pending versus already-dispatched unflip.** Neither is cancelled by a re-claim. The unflip runs to its stop path, which calls `finish_deferred_cow_release` ([backend.rs:1853](../../../crates/yserver/src/kms/render/backend.rs:1853), invoked at [backend.rs:2110](../../../crates/yserver/src/kms/render/backend.rs:2110)); with the flag cleared it retires nothing. The retirement decision is therefore read at stop time, not at request time, so no cancelable/dispatched branch is introduced and no armed obligation can retire a reclaimed resource.
- **Stop path never runs.** On device loss or unknown completion the retained lease stays quarantined and rooted and moves in the Task 9 bundle; core's claim state is unaffected.
- **After a completed `finish_cow_release`.** The identity is gone, so a later 0→1 allocates fresh storage with a new allocation generation; stale evidence for the old generation cannot retire it.
- **Failure routes.** Preserve request-failure claim retention, and disconnect's release-claims-anyway plus sticky `cow_teardown_failed` ([composite_overlay.rs:122](../../../crates/yserver-core/src/core_loop/composite_overlay.rs:122)).

Test final release/disconnect with a direct frame, delayed physical retirement and re-claim: assert the deferred release drops no lease and decrefs no storage; the re-claim performs zero new imports/allocations and keeps the same allocation key and generation with exactly one logical claim in `cow_claims`; the later stop path frees nothing and the COW survives; the no-re-claim ordering decrefs exactly once; after a completed `finish_cow_release` and a fresh 0→1 allocation, late stop-path or unflip evidence for the old identity retires nothing (round-3 m-2); and both failure routes behave as above.

- [x] **7.6 Add grouped/Skip tests.** A grouped commit that changes only one CRTC registers no obligation for the retained member and its `HardwareComplete` discharges nothing for it (round-4 m-1). A two-output frame completes only with all required evidence; select the recorded reference CRTC sample even if the other arrives last or has larger MSC. Replace only one output and assert shared source stays retained. Supersede never-submitted successor: idle/drop its resource leases now, keep Skip metadata ordered after predecessor and `emit_idle=false`; accepted work ending as Skip retains the stronger release rule. Repeated stale/duplicate notifications cannot double-complete or double-release. Add the round-3 M-1 regression: the **only** KMS proof reaches the service through the owner's `HardwareComplete` event — the test body calls no `apply_validated_proof` — and asserts the `old` set's obligations are discharged, the `new` set's are untouched, a partial grouped replacement discharges only the matching `GroupMember`, and a rejected commit cancels rather than discharges.
- [x] **7.7 Run** focused owner/backend Present tests, format and clippy. Commit with `feat(kms): consume concrete commit resources and split present release`.

## Task 8: Six physical roles and release-safe exit

**Status: EXECUTED at `f93088de`.** **Review round 1 (2026-09-11): REJECTED** — see the findings and `docs/handoff-phase-c0-stage-2c-i-fix.md`; unchecked steps below are not done or not proven.

**Fix round 1 (F-7): `f39a01c6`.** Fix session F-7, Task 8 role transitions and `on_available` safety, closing M-1, M-7, M-8, and 8.6 per `docs/handoff-phase-c0-stage-2c-i-fix.md`.

| Finding | Verdict |
| --- | --- |
| M-1 (`on_available` error preserves 100% of resources in `releasing_resources` without drops) | **RESOLVED (test: `c0_2ci_capacity_on_available_error_restores_all_resources_safely`)** — `on_available` uses staged processing: on any role transition failure (or proof error), all popped resources are restored to `releasing_resources`/`rejected_resources`, admission is closed, and `Err` is returned without dropping any resource |
| M-7 (`CompletionRetired` performs `move_into_reserved`/`move_role`, returns Submitted token to Current; `finish_role` rejects merely `Reserved` token; 8.3 managed candidate preparation seam and 8.5 unflip/composed retention) | **RESOLVED (tests: `c0_2ci_capacity_finish_role_rejects_merely_reserved_token`, `c0_2ci_backend_scanout_m1_probe_cache_strictly_bounded`, `c0_2ci_backend_managed_prepare_direct_candidate_implicit_layout_rejection`, `c0_2ci_backend_managed_unflip_and_reentry_contracts`)** — `finish_role` checks `RoleState::Occupied(serial)` and strictly rejects `Reserved`; `CompletionRetired` moves old Current into pre-reserved retirement slot and moves Submitted to Current; `ScanoutM1ProbeCache` is bounded to 32 entries via FIFO eviction (`VecDeque`); `managed_prepare_direct_candidate` rejects `implicit_layout` before import/reservation; `managed_handle_direct_unflip` unflip requests and clears dual retirement roles before re-entry |
| M-8 (remove `has_pending_obligation` skip; fix double registration) | **RESOLVED (tests: `c0_2ci_adapter_unflip_ordinary_retirement_occupied`, `c0_2ci_capacity_comprehensive_six_roles_and_contract_8_6`)** — cleaned up obligations and used `cancel_reservation` for unattached/reserved slots; test double registrations eliminated |
| 8.6 (Rewritten so destination token is not pre-attached and real occupancy is reached) | **RESOLVED (test: `c0_2ci_capacity_comprehensive_six_roles_and_contract_8_6`)** — comprehensive test covering A retired / B current / C-D-E successors with real capacity occupancy, B unflip into ExitRetirement while A is in OrdinaryRetirement, partial grouped release with matching/non-matching CRTC obligations, delayed `on_available` with `service_ready`, and clean direct re-entry only when both retirement roles are vacant |

Mutation checks performed and reverted in F-7:
1. Mutating `finish_role` to accept `RoleState::Reserved` caused `c0_2ci_capacity_finish_role_rejects_merely_reserved_token` to fail (`called Result::unwrap_err() on an Ok value: ()`).
2. Mutating `on_available` to omit restoring unconsumed resources back to `releasing_resources` on error caused `c0_2ci_capacity_on_available_error_restores_all_resources_safely` to fail (`assertion left == right failed: left: 1, right: 3`).
3. Mutating `managed_prepare_direct_candidate` to skip rejecting `implicit_layout` caused `c0_2ci_backend_managed_prepare_direct_candidate_implicit_layout_rejection` to fail (`assertion left == right failed: left: 0, right: 1`).

Gate for F-7: `cargo +nightly fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean; `cargo test -p yserver --lib c0_2ci` 118 passed/0 failed/10 ignored on clean run and twelve consecutive runs (zero flakes); `cargo test -p yserver --lib c0_2ci -- --ignored` 10 passed/0 failed (hardware run); full `cargo test -p yserver --lib` passed (1743 total, 0 failed); `cargo check -p yserver --target x86_64-unknown-linux-gnu`, `--target x86_64-unknown-linux-musl`, and `--target x86_64-unknown-freebsd` all clean.

```
$ cargo test -p yserver --lib c0_2ci -- --ignored
running 10 tests
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::store::tests::c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_dri3_lease_regressions_vulkan ... ok
test kms::render::resources::tests::c0_2ci_descriptor_reset_exclusion_until_gpu_signaled_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_no_premature_pool_return_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok
test kms::render::backend::tests::c0_2ci_read_source_scratch_regression_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_dropped_frame_metadata_with_live_ticket_vulkan ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1728 filtered out; finished in 0.77s
```

**Files:** Create `resources/capacity.rs`; extend `commit.rs` and the managed backend preparation boundary. Preserve legacy direct scheduling until 2c-ii/iii conversion.

**Consumes:** Task-7 resource sets and service eligibility. Defines capacity only, not seven-tier scheduler policy.

**Produces:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DirectRole {
    Current, Submitted, Successor, Preparing, OrdinaryRetirement, ExitRetirement,
}
pub(crate) struct RoleReservation {
    pub(crate) role: DirectRole,
    pub(crate) serial: u64,
    pub(crate) closed: std::rc::Rc<std::cell::Cell<bool>>,
    pub(crate) discharged: bool,
}
impl DirectCapacity {
    pub(crate) fn new() -> Self;
    pub(crate) fn reserve(&mut self, role: DirectRole)
        -> Result<RoleReservation, ResourceError>;
    #[allow(clippy::result_large_err)]
    pub(crate) fn attach(&mut self, slot: RoleReservation, value: CommitResources)
        -> Result<CommitResources, (ResourceError, RoleReservation, CommitResources)>;
    pub(crate) fn cancel_reservation(&mut self, slot: RoleReservation)
        -> Result<(), (ResourceError, RoleReservation)>;
    pub(crate) fn finish_role(&mut self, slot: RoleReservation)
        -> Result<(), (ResourceError, RoleReservation)>;
    pub(crate) fn move_role(&mut self, slot: &mut RoleReservation, to: DirectRole)
        -> Result<(), ResourceError>;
    pub(crate) fn move_into_reserved(&mut self, occupied: &mut RoleReservation, reserved: RoleReservation)
        -> Result<(), (ResourceError, RoleReservation)>;
    pub(crate) fn occupied(&self) -> usize;
    pub(crate) fn can_enter_direct(&self) -> bool;
}
```

Task 8 puts the sole `DirectCapacity` inside `CommitResourceConsumer`. Its existing `consume(event, service)` signature therefore has direct access to capacity and cannot orphan a returned token. Add `CommitResourceConsumer::on_available(&mut self, keys: &[AllocationKey], service: &mut ResourceService) -> Result<(), ResourceError>` and invoke it in the same serialized resource-service turn that publishes eligibility. It rechecks the waiting old/new resource sets, consumes releasable role tokens with `finish_role`, and schedules direct admission once. Rejection restores Current and disposes the rejected token only when its remaining dependencies finish; ordinary and exit retirements each finish their own token; unknown retains all charges. Any transition error keeps resources/tokens rooted and closes admission. The handoff transfers this consumer including its capacity, never a second independent capacity table (plan-review M-1).

`DirectCapacity` stores six non-owning `RoleState` values: `Vacant`, `Reserved(serial)`, or `Occupied(serial)`. Task 8 adds `direct_role: Option<RoleReservation>` to `CommitResources`; `attach` puts the reservation there and returns the resource set to its sole owner. The consumer/commit owner/release ledger owns resources, never the capacity table as well. A serial is checked monotonic. Define `cancel_reservation(&mut self, slot: RoleReservation) -> Result<(), (ResourceError, RoleReservation)>` for proven pre-import cancellation and `move_role(&mut self, slot: &mut RoleReservation, to: DirectRole) -> Result<(), ResourceError>` for role changes. Invalid operations preserve source ownership. For a pre-reserved retirement destination, define `move_into_reserved(&mut self, occupied: &mut RoleReservation, reserved: RoleReservation) -> Result<(), (ResourceError, RoleReservation)>`: validate both slots before changing either, then vacate the source and adopt the destination serial atomically. A plain move to an already reserved slot is invalid. End occupied accounting through `finish_role(&mut self, slot: RoleReservation) -> Result<(), (ResourceError, RoleReservation)>` only after the consumer has discharged the role's release obligations. Dropping a token cannot free capacity; an unexpected lost token closes admission with its slot charged. Resources moved into the generic owner carry their tokens, so accounting remains charged without a second owning copy.

- [x] **8.1 Add the full-capacity test.** Reserve each role once and reject another Preparing reservation:

```rust
let mut capacity = DirectCapacity::new();
let mut held = Vec::new();
for role in [DirectRole::Current, DirectRole::Submitted, DirectRole::Successor,
    DirectRole::Preparing, DirectRole::OrdinaryRetirement, DirectRole::ExitRetirement] {
    held.push(capacity.reserve(role).unwrap());
}
assert_eq!(capacity.occupied(), 6);
assert!(matches!(capacity.reserve(DirectRole::Preparing), Err(ResourceError::Busy)));
assert!(!capacity.can_enter_direct());
```

The `held` vector deliberately retains reservation tokens. Add real import/destruction counts in the managed probe fixture rather than inferring allocation count from this unit test.

- [x] **8.2 Run** `cargo test -p yserver --lib c0_2ci_capacity` before implementation.
- [x] **8.3 Reserve before retaining/importing.** Managed direct preparation rejects `implicit_layout` before any direct import/validation, preserving upstream's `m1_gate_reject_import` behavior; a guessed LINEAR modifier is never scanout qualification. It then takes Preparing, then pins source/fallback and imports/tests FB. On proven failure clean up the candidate and cancel the role, retaining the existing successor. On successful validation atomically replace Successor, idle/release the victim and retain only its ordered Skip metadata. If victim/preparing cleanup is uncertain, charge its role and close admission instead of proceeding with another import. Managed probe cache stores no additional strong framebuffer leases.
- [x] **8.4 Reserve retirement before replacement.** An ordinary replacement cannot dispatch with occupied OrdinaryRetirement. Keep the latest Successor while waiting and register a service wake. At commit dispatch, move Current/New leases into `Submitted<CommitResources>` by value while retaining role accounting; on acceptance return new to Current and old to the pre-reserved retirement role, on rejection restore Current and dispose new, on unknown retain both in owner quarantine without freeing capacity.
- [x] **8.5 Implement exit-resource accounting.** Before managed direct entry, retain one release-safe composed return allocation per affected output. Unflip cancels unsent direct work, waits for submitted work and uses ExitRetirement for Current even if OrdinaryRetirement is occupied. Materialize shadow and resolve source/GPU dependencies before using the composed return resources. Do not display stale pixels. Re-entry requires both retirement roles vacant; no additional pool/frame allocation is allowed to work around a blocked exit.
- [x] **8.6 Test** A retired/B current/C-D-E successors; B unflip while A awaits release; repeated entry/exit; failed cleanup; partial grouped release; and immediate-ready retirement. Observe a maximum of six charged positions, bounded live imports and once-only ordered victim completions. Drive the real `consume` → delayed `on_available` → `finish_role` path for rejected, ordinary-retirement and exit-retirement tokens, and verify each makes a previously blocked reservation available without dropping a token to free its charge. Run focused tests, format and clippy; commit with `feat(kms): bound managed direct resource roles`.

## Task 9: Reserved teardown recipient and late-completion handoff

**Status: EXECUTED at `ac3c94f7`.**

**Fix round 1 (F-8): `ac3c94f7`.** Fix session F-8, Task 9 sealed barriers, revocation, and late-completion handoff, closing B-3 (deterministic half), B-4, B-5, B-7, M-9, M-10, M-11, M-12, F2-m1, and F1-m1 per `docs/handoff-phase-c0-stage-2c-i-fix.md`.

| Finding | Verdict |
| --- | --- |
| B-3 (deterministic half: control alone: no, reap with one alias: no, last alias: mint and discharge) | **RESOLVED (test: `c0_2ci_handoff_complete_fd_family_barrier_deterministic`)** — deterministic ordering test exercising control closed alone (fails), submitters detached alone (fails), helper reaped with non-payload aliases active (fails), pool husk drained with returned descriptor active (fails), returned descriptor closed (succeeds, discharges payload alias, mints `FileFamilyClosed`, and rejects any post-barrier ioctl with `PermissionDenied`); real-GBM `_drm` case is `c0_2ci_fd_family_barrier_real_gbm_payload_drm` (from F-2) |
| B-4 (`DeviceBarrier` sealed with private fields, constructible only from `FileFamilyClosed` by value or proven device loss) | **RESOLVED (tests: `c0_2ci_handoff_complete_fd_family_barrier_deterministic`, `c0_2ci_handoff_unresolved_kms_rejects_teardown_release`, `c0_2ci_adapter_duplicate_stale_evidence_aliasing`)** — `DeviceBarrier` enum variants carry `_private: ()`, constructors are `from_file_family_closed(FileFamilyClosed)` taking proof by value and `from_device_loss(DrmDeviceKey, DeviceLossProof)`; zero enum literals in production or tests |
| B-5 (`TeardownRelease` constructible only by supervisor test fixture; `RetainingSupervisor` under `#[cfg(test)]`) | **RESOLVED (tests: `c0_2ci_handoff_*`)** — `TeardownRelease::mint_for_supervisor` is sealed under `#[cfg(test)]`; `RetainingSupervisor` and `reserve_slot` reside under `#[cfg(test)]` |
| B-7 (`IncarnationBundle` carries `TransportGate`; `transfer` calls `revoke_owner_writes` before `close`, records quarantined owner record, freezes uncertain entries) | **RESOLVED (test: `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines`)** — `IncarnationBundle` carries `gate: TransportGate`; `transfer` revokes owner writes before calling `close()`, quarantines live record via `owner.quarantine_live()`, emits `OwnerEvent::Quarantined`, freezes uncertain commit entries via `consumer.consume`, and closes capacity admission |
| M-9 (`HandoffRouter::service` propagates errors without discarding results) | **RESOLVED (tests: `c0_2ci_handoff_success_routes_late_events_and_completions`)** — `HandoffRouter::service` returns `Result<(), ResourceError>` and propagates consumer and registration errors rather than discarding with `let _ = ...` |
| M-10 (`apply_teardown_release` refuses `file_owned == Some`) | **RESOLVED (test: `c0_2ci_scanout_apply_teardown_release_refuses_live_file_owned`)** — `apply_teardown_release` validates that no file-owned right/gbm_bo/device alias remains, returning `ResourceError::InvalidProof` |
| M-11 (returned descriptors registered under incident and block mint until closed; movable poll registration) | **RESOLVED (tests: `c0_2ci_handoff_complete_fd_family_barrier_deterministic`, `c0_2ci_handoff_success_routes_late_events_and_completions`)** — `deliver_descriptor` registers descriptor into `drm.register_returned_descriptor(fd)`, which increments `non_payload_aliases` and blocks `try_mint_file_family_closed` until `close_returned_descriptors()` |
| M-12 (9.4 code: engine/store detach preserving cleanup ownership, managed `shutdown_destroy_drawables`; 9.1 test) | **RESOLVED (tests: `c0_2ci_storage_managed_destroy_detaches_before_drop`, `c0_2ci_handoff_failure_returns_bundle_and_slot_by_value`, `c0_2ci_handoff_success_routes_late_events_and_completions`)** — store logical drawables detached via `shutdown_destroy_drawables`; handoff returns intact bundle and slot by value on failure, and processes late events/completions |
| F2-m1 (pool husk holding `Rc<Device>` accounted for in `non_payload_aliases`) | **RESOLVED (test: `c0_2ci_handoff_complete_fd_family_barrier_deterministic`)** — `DrmCleanupRegistry` provides `register_pool_husk` and `unregister_pool_husk`; tested to block barrier mint until pool husk is drained |
| F1-m1 (quarantine freezes service entries, NOT the `DrmCleanupRegistry` itself) | **RESOLVED (test: `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines`)** — `quarantine_live` and `consumer.consume(Quarantined)` freeze service entries (`service.is_frozen(&key) == true`), leaving `bundle.drm.is_frozen() == false` so payload aliases can still be discharged during barrier mint |

**Decision on F1-m1:** Quarantine freezes service entries (`service.freeze`), NOT the `DrmCleanupRegistry` itself (`freeze_incarnation`). Freezing the registry would block `drm.consume` during payload alias discharge in `try_mint_file_family_closed`, causing an unrecoverable leak deadlock. `bundle.drm.is_frozen()` remains false across handoff and quarantine.

Mutation checks performed and reverted in F-8:
1. Mutating `DeviceBarrier::FileFamilyClosed` constructor to ignore the proof value or allowing public literal construction: prevented by compiler via `_private: ()`.
2. Mutating `HandoffRouter::transfer` to call `bundle.gate.close()` *before* `bundle.gate.revoke_owner_writes()`: causes `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines` to fail (`Busy` because outstanding write grants block close).
3. Mutating `HandoffRouter::transfer` to skip `bundle.owner.quarantine_live()` when `revoked > 0`: causes `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines` to fail (`bundle_ref.resources.is_frozen` remains false).
4. Mutating `try_mint_file_family_closed` to ignore `returned_descriptors` / `pool_husk`: causes `c0_2ci_handoff_complete_fd_family_barrier_deterministic` to fail (barrier mints prematurely before unregistering pool husk or closing returned descriptors).

Gate for F-8: `cargo +nightly fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean; `cargo test -p yserver --lib c0_2ci` 120 passed/0 failed/10 ignored on clean run and twelve consecutive runs (zero flakes); `cargo test -p yserver --lib c0_2ci -- --ignored` 10 passed/0 failed (hardware run); full `cargo test -p yserver --lib` passed (1745 total, 0 failed); `cargo check -p yserver --target x86_64-unknown-linux-gnu`, `--target x86_64-unknown-linux-musl`, and `--target x86_64-unknown-freebsd` all clean.

```
$ cargo test -p yserver --lib c0_2ci -- --ignored
running 10 tests
test kms::render::resources::tests::c0_2ci_fd_family_barrier_real_gbm_payload_drm ... ok
test kms::render::store::tests::c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_dri3_lease_regressions_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan ... ok
test kms::render::store::tests::c0_2ci_storage_no_premature_pool_return_vulkan ... ok
test kms::render::resources::tests::c0_2ci_gpu_dropped_frame_metadata_with_live_ticket_vulkan ... ok
test kms::render::backend::tests::c0_2ci_read_source_scratch_regression_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan ... ok
test kms::render::resources::tests::c0_2ci_descriptor_reset_exclusion_until_gpu_signaled_vulkan ... ok
test kms::render::resources::adapter_tests::c0_2ci_live_lifetime_adapters_vulkan ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1730 filtered out; finished in 0.79s
```

**Files:** Create `resources/handoff.rs`; modify platform/backend detach seams and test support. The stage-3 recovery process itself is not implemented here.

**Consumes:** All prior tasks, original `DeviceCommitOwner<CommitResources>` and `KmsIoExecutor` values, registry cleanup rights and existing completion registrations.

**Produces:** `IncarnationBundle`, `RecipientSlot`, `HandoffRouter`, `RetainingSupervisor` test fixture. The bundle owns the complete incident:

```rust
pub(crate) struct IncarnationBundle {
    pub(crate) owner: crate::kms::owner::device::DeviceCommitOwner<CommitResources>,
    pub(crate) resources: ResourceService,
    pub(crate) consumer: CommitResourceConsumer,
    pub(crate) drm: DrmCleanupRegistry,
    pub(crate) executor: Option<crate::kms::executor::KmsIoExecutor>,
    pub(crate) ingress: CompletionIngress,
    pub(crate) gate: TransportGate,
}
```

`CompletionIngress` owns pending `OwnerEvent<CommitResources>`, returned owned descriptors not yet assigned to records, and movable poll-registration ownership for that device/incarnation. Resource GPU tickets/inbox remain owned by `ResourceService`; do not duplicate them inside ingress. Extract per-incarnation registrations from shared pollers rather than moving a poller that serves unrelated devices. `RecipientSlot` reserves storage for one bundle before managed traffic and belongs to a process-root owner, not a backend-local temporary.

Define `HandoffRouter::transfer(&mut self, slot: RecipientSlot, bundle: IncarnationBundle) -> Result<(), (ResourceError, RecipientSlot, IncarnationBundle)>`. On failure return every owner unchanged; the caller keeps it alive with the transport closed. Define `HandoffRouter::service(&mut self, now: Instant)` to process the recipient's existing inbox/registrations without calling back into backend/store/pool. The test fixture owns the router outside the backend scope; production Owner remains impossible without a real recipient implementation. `RecipientSlot` carries Task 6's `RecipientReservation`, so successful handoff consumes the reservation made before activation. For frozen entries, normal proofs alone do not unfreeze the allocation. Add a sealed `TeardownRelease` capability and `ResourceService::apply_teardown_release(&mut self, proof: TeardownRelease) -> Result<(), ResourceError>`: it validates the exact incarnation/entries and independently recorded KMS, file-owned, GPU/read and FOREIGN dispositions before ending quarantine. Only the retaining-supervisor fixture issues it in this stage, after exercising those barriers; stage 3 supplies the real issuer. An expired deadline is never a proof constructor.

**KMS disposition at teardown (plan-review round-2 B-1).** The four dispositions are independent; the file-owned/GPU/read/FOREIGN set alone never fabricates the KMS one (resource design §4). Each quarantined entry therefore records a `KmsDisposition` for every registered `ObligationKind::KmsRelease`, keyed by the producing device/incarnation/commit and the exact `GroupMember` identity (CRTC key, topology generation, CRTC epoch):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsDisposition {
    Outstanding,
    Discharged,
    Superseded(DeviceBarrier),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceBarrier {
    FileFamilyClosed {
        device: DrmDeviceKey,
        incarnation: IncarnationId,
        _private: (),
    },
    DeviceLost {
        device: DrmDeviceKey,
        _private: (),
    },
}
```

`Discharged` requires correlated `PriorBufferReleased` for that exact commit/CRTC generation; a reused CRTC number at another generation does not qualify. `DeviceBarrier` has exactly two private constructors, both scoped to the producing `DrmDeviceKey`: the registry's `FileFamilyClosed` for the complete `IncarnationFdSet` of that device, issued only after every submitter has stopped and the incarnation is detached from event dispatch (Task 9.5), and a proven device removal/loss for that device key. Nothing else constructs one — not an expired deadline or watchdog, not control-IPC or single-alias closure, not helper reap alone, not an `ExecutorStalled`/`ShutdownExecutorStalled` bounded exit, and not GPU/read/FOREIGN evidence. The barrier supersedes the KMS obligation only: per governing §10 it is a barrier solely for resources owned exclusively by that open file description, so shared dma-buf/GBM/Vulkan state keeps its independent proofs. A barrier for one device never supersedes an obligation registered against another device key.

**Reaching the barrier with registry-rooted device holders (plan-review round-3 B-1).** Quarantined GBM-backed payloads retain `Rc<GbmDevice>`, which is a duplicate of the incarnation's open file description (`from_inherited_kms_fd`, `MasterOwnership::InheritedDuplicate`). If those holders merely waited for `FileFamilyClosed`, the description could never close and the barrier could never be minted — the leak outcome the spec forbids. They are therefore counted aliases whose closure is performed by the barrier discharge itself, in this order on the serialized recipient path:

1. `FileFamilyClosed` becomes **mintable** when every submitter and dispatch path is detached, the helper is reaped, the control alias and every non-payload alias are closed, and the only remaining holders of the description are registry-rooted payload contexts.
2. Discharge then takes only each entry's `file_owned` half (Task 4) and destroys it in the table's order: `RMFB` through the right, the gbm_bo drop as the sole `GEM_CLOSE` where `GemOwner::Gbm` (against the still-open description, so not a stale-handle ioctl), then the device alias. `shared` stays in the entry untouched.
3. The registry closes its own last alias as the final step and mints `FileFamilyClosed`; only then does the `Superseded(FileFamilyClosed)` disposition exist and `apply_teardown_release` proceed.

Step 2 is safe with respect to the unresolved KMS obligation because a DRM framebuffer holds its own kernel reference to the GEM object: closing the userspace handle frees nothing while the framebuffer exists, and the kernel removes the framebuffer and disables the plane when the description's last reference goes. No userspace ioctl touches the buffer after step 3. The `shared` half is not part of this order; it keeps its independent GPU/read/FOREIGN proofs and `file_owned == None` is its documented state thereafter (round-4 M-1).

`apply_teardown_release` returns `ResourceError::InvalidProof` while any entry named by the proof still holds an `Outstanding` KMS obligation, even when every other disposition is satisfied — including the vacuous case where the entry has no exclusively-file-owned handles left. Unresolved KMS ownership without a barrier is not a leak decision the capability may take: entries stay quarantined and rooted through the shutdown deadline and process-exit policy (governing §10 shutdown row), which is the specified outcome for an unreaped helper holding a device alias.

- [x] **9.1 Write the detached-backend test.** Start a managed owner with A/B resources and dispatch uncertainty. Reserve a recipient, transfer, destroy the backend fixture, then deliver late reply descriptors and a delayed GPU completion to the router. Count resource destruction, fd cleanup calls and wake deliveries. Assert the same incarnation consumes the evidence, no current state is resurrected, and no old backend callback occurs.
The failure-path test uses a mismatched reserved `slot` and a populated `bundle` from the managed fixture; capture `incarnation = bundle.owner.incarnation()` first. Assert the entire owner comes back by value:

```rust
let result = router.transfer(slot, bundle);
let Err((error, slot, bundle)) = result else { panic!("invalid recipient accepted"); };
assert_eq!(error, ResourceError::WrongIncarnation);
assert_eq!(bundle.owner.incarnation(), incarnation);
assert_eq!(old_drops.get(), 0);
assert_eq!(new_drops.get(), 0);
```

Keep the returned `slot` and `bundle` rooted in the test's outer supervisor fixture. In the success test, submit the same correctly matched ownership once, process late evidence through `router.service(now)`, and assert descriptor cleanup waits for the family barrier. A test must not pass by simply forgetting/leaking the returned bundle.

- [x] **9.2 Run** `cargo test -p yserver --lib c0_2ci_handoff` before creating transfer support.
- [x] **9.3 Implement atomic move and routing.** On the serialized core path: call `revoke_owner_writes` first and treat every revoked grant as possibly dispatched — its owner record is `Quarantined`, never cancelled (round-4 M-2) — then close transport, stop acquisition, freeze uncertain entries, move the owner/current/retirement roles and service plus cleanup contexts into the pre-reserved slot, then publish that slot as the sole ingress target. Queue evidence racing the switch in the stable inbox and drain it under the new recipient. No fallible allocation is required after the transfer begins. Allocation failure when reserving the recipient prevents managed activation in the first place.
- [x] **9.4 Preserve cleanup ownership during engine/store detach.** Transfer pending promoted images, descriptor slots, pinned wakes and invalidation work before destroying ordinary containers. An invalidation job must own the cache-entry slice it will destroy; it cannot reference the old engine. Managed `shutdown_destroy_drawables` detaches logical entries into service ownership. Keep the root router alive until final process exit policy, even if helper reap or GPU proof never arrives.
- [x] **9.5 Test the complete fd-family barrier.** Register original device descriptors, helper/child aliases and pending reply-owned descriptors. Close control IPC alone: no proof. Reap helper but retain one device alias: no proof. Close the last registered file-description alias: issue the registry's private `FileFamilyClosed`, discharge file-owned FB/GEM rights, and verify late final lease drop issues no ioctl. Add a case with a **real** `Rc<drm::Device>`-owning payload (a `GbmDevice` over the fixture's inherited duplicate), not the fake family inventory: after reap and control-alias closure the barrier must still be mintable, the discharge must destroy the GBM BO before the device drop, the description's last close must be the registry's, and no ioctl may follow it (round-3 B-1). **Fixture (review round 1, B-3):** `Device::for_tests()` is a Unix socket and cannot back a `GbmDevice`, so this case is a hardware test: open a render node with `Device::open_render_node` (no master needed; pick it through `kms::render_node::open_for_card` over `TestDevice::open_real_drm_or_ignore()` or scan `/dev/dri/renderD*`), build `GbmDevice::new(Rc::clone(&device))`, allocate one real gbm_bo, and put it in a real `FileOwnedBacking` with `GemOwner::Gbm` and a `DrmCleanupRight` whose transport is the counting `CleanupIo` (so `RMFB` is observed, not issued against the render node). Name it `c0_2ci_fd_family_barrier_real_gbm_payload_drm`, annotate `#[ignore = "requires a real DRM render node; run explicitly"]`, and `panic!` when no node is available — never return. The deterministic half of 9.5 (fake inventory, control/helper/alias ordering) stays a plain `c0_2ci_` test. Keep shared Vulkan/GBM cleanup blocked until its independent proof. A late returned alias must be registered/closed under the same incident before closure can be certified.
- [x] **9.5a Test that unresolved KMS ownership rejects teardown release (plan-review round-2 B-1).** Quarantine an acceptance-unknown direct commit whose old allocation carries a registered `KmsRelease` obligation for a known commit/`GroupMember`. Satisfy every other listed predicate: discharge or exhaust the entry's file-owned rights, complete its GPU and read tickets, and prove its FOREIGN return, while one helper alias keeps the fd family open (`ExecutorStalled`). Assert `apply_teardown_release` returns `ResourceError::InvalidProof`, the payload is not destroyed and no ioctl is issued. Then take each discharge route separately and assert exactly-once cleanup: correlated `PriorBufferReleased` for the exact commit/CRTC generation; and, from the reset fixture, complete-family closure after reap. Assert a `FileFamilyClosed` barrier for a second device key does not supersede this entry's obligation, and that a stale-generation `PriorBufferReleased` at a reused CRTC number does not either.
- [x] **9.6 Test unavailable recipient, duplicate transfer, pending read at handoff, identity reuse, and handoff under `ExecutorStalled` with a grant outstanding: revocation precedes close, close succeeds, and the revoked grant's record is `Quarantined` (round-4 M-2).** Failure returns the intact bundle. A old-generation completion cannot free a new allocation/output at the same numeric index. Service deadlines keep running in the retaining recipient while backend composition is gone. Unknown never transitions back to ordinary Current after a late success. Run focused tests, format and clippy; commit with `feat(kms): define retaining incarnation handoff`.

## Task 10: Concrete adapters, integration evidence and handoff to 2c-ii

**Status: EXECUTED at `d1aac6fd`.** **Review round 1 (2026-09-11): REJECTED** — see the findings and `docs/handoff-phase-c0-stage-2c-i-fix.md`; unchecked steps below are not done or not proven.

**Files:** Create `resources/adapter_tests.rs`; update affected tests and `docs/status.md`. Keep deterministic tests in ordinary `cargo test`; actual Vulkan/DRM cases use the repository's hardware annotations and must report environmental skips honestly.

**Consumes:** Tasks 1–9 and the design's regression matrix.

**Produces:** Executable coverage of actual adapter lifetimes, source inventory audit, validation record and explicit readiness boundary. No new scheduling/conversion implementation.

- [ ] **10.1 Finish the concrete fixture matrix.** Use actual constructors for each backing family. Fault injection substitutes completion timing/cleanup transport, not the allocation ownership path being tested. Prefix new deterministic tests with `c0_2ci_` and give hardware variants distinct names ending `_vulkan` or `_drm`.

| Family / sequence | Required assertions | Owning task |
| --- | --- | --- |
| Native, imported and promoted storage | Lease survives drawable destruction; exact views/images destroyed once; pool eligibility preserved | 3 |
| Old layout during relayout/promotion | No overwrite while incompatible use remains; old XID cleanup preserves new mapping/damage | 3 |
| Shared BO and copied source/sink pair | Real backing and both contexts retained; KMS/GPU/FOREIGN order cannot prematurely reuse | 4–5 |
| Root snapshot then scratch Composite | Successful source read ends at CPU copy; scratch lasts through its own GPU use and frees once | 5 |
| Uncertain GPU/read submit | Source/staging/descriptors retained, no normal release or retry spin | 5–6 |
| VT-away / DPMS-off / idle scene | Completion-only wake advances resource service, no scene submission required | 6 |
| Grouped A/B frame, reversed output evidence | Reference CRTC supplies sample; shared source retained until every required replacement | 7 |
| Rejection, accepted Skip and supersession | Restore old current or hold accepted resources correctly; ordered completions and once-only idle | 7 |
| Preparing failure and A/B/C-D-E burst | Maximum six roles, no strong import cache overflow; immediate victim lifetime release when proven safe | 8 |
| Unflip with ordinary retirement occupied | Composed return resource works with ExitRetirement; no stale pixels or extra allocation | 8 |
| Unknown → detach → late reply → helper reap | Recipient owns everything; full fd closure distinct from shared-resource cleanup | 9 |
| Duplicate/stale evidence and aliasing | No double signal/destruction or release of another generation | 1–9 |

- [ ] **10.2 Add the live Vulkan smoke.** Extend the existing ignored software-Vulkan test infrastructure: allocate native storage, retain a managed lease, free the drawable, poll, assert the allocation remains accessible through the lease; drop the lease after its ticket and poll to observe cleanup. Repeat for promoted backing and snapshot scratch. Use real engine/cache invalidation counters or validation-layer diagnostics to verify view-before-image cleanup. Do not treat a no-ICD early return as a passing lifetime test.
- [ ] **10.3 Audit all callers of changed ownership APIs.** Run:

```bash
rg -n 'destroy_now|shutdown_destroy_all|adopt_exportable|destroy_retired_image|retire_image_after' crates/yserver/src/kms
rg -n 'transition_to_free|release_completed_source|note_kms_retired|drain_all_pending|disarm' crates/yserver/src/kms
rg -n 'DirectScanoutProbeFramebuffer|present_source_pins|retained_present_wakes|CompletionRetired|ResourcesReleased|ResourcesStillCurrent' crates/yserver/src
rg -n 'try_finish_legacy_transport|finish_legacy_transport|NeverResource' crates/yserver/src/kms
rg -n 'atomic_commit|submit_flip|commit_modeset|disable_output|set_gamma|set_cursor2|move_cursor|dispatch_blocking_at_boundary' crates/yserver/src
rg -n 'cow_claims|cow_teardown_failed|implicit_layout|import_plane0|import_size|drm_modifier' crates/yserver/src crates/yserver-core/src
```

Classify each affected production caller as Legacy-only, managed-service mediated or rejected by the activation gate, and record the table in this plan's execution notes. Fix any managed raw-handle escape or unconditional destructor before completion. No new resource-bearing `OwnerEvent` may fall into a wildcard drop. This is the executable caller audit deferred by the bounded design review.

- [x] **10.4 Run final software and portability checks.** These commands are required once the tasks are implemented; their presence here is not a claim they ran during drafting.

```bash
cargo +nightly fmt
cargo +nightly fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo build --locked
cargo test --all-targets --locked
cargo check -p yserver --target x86_64-unknown-linux-gnu
cargo check -p yserver --target x86_64-unknown-linux-musl
cargo check -p yserver --target x86_64-unknown-freebsd
YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test -p yserver --lib --locked -- --ignored
```

Record actual passed/failed/ignored/skipped counts and tool/environment failures. Hardware scanout tests require supported DRM hardware and are separate from software-Vulkan tests; glibc/musl/FreeBSD compilation is not runtime fence support certification. A failing mandatory check or unverified managed lifetime path blocks completion. After a new fix rerun its affected checks; do not repeat unrelated broad suites without cause.

- [ ] **10.5 Update `docs/status.md` and this plan's checkboxes with actual evidence.** State which backing families passed concrete tests and any unavailable hardware coverage. Preserve operational readiness as closed and production as Legacy. Document the API handoff to 2c-ii: physical-role reservation, generation-bound lease acquisition, service readiness/wake subscription and typed resource outcome consumption. 2c-iii still owns producer conversion/damage integration; stage 3 owns the live teardown supervisor; stages 3/4 supply remaining owner-mediated writers. No C0 completion claim follows from finishing 2c-i.
- [x] **10.6 Run formatting and required clippy before the final task commit.** Stage only tests/documentation from this task and commit with `test(kms): verify resource terminalization adapters`. Do not squash merge or push as part of execution.

## Author self-review and traceability

This is drafting evidence, not task execution. The first plan review is recorded above; these 2026-09-10 corrections and upstream additions have not received a new external pass.

| Design contract / prior finding | Plan coverage |
| --- | --- |
| Real allocation ownership, no ID-only pins | Tasks 1–5, 7; concrete tests in 10 |
| Round-1 B-1 teardown and fd-family cleanup | Tasks 2, 6, 9 |
| Round-1 B-2 six physical roles and safe unflip | Task 8; actual import count in 10 |
| Round-2 B-1 authoritative availability and event delivery | Tasks 1, 4–6, 9 |
| Round-2 M-1 layout/read verification | Tasks 3, 5, 9–10 |
| Round-2 m-1 original X11 depth | Task 3 `PixelIdentity` retains complete `PaintTarget` |
| M-2 writer exclusion and activation prerequisites | Task 6; no production Owner without later-stage proofs |
| Rejection/unknown/hardware/Present/release distinctions | Tasks 1–2, 5, 7, 9 |
| Accepted unbounded Skip metadata | Task 7; no protocol credits introduced |
| Source/fallback aliases and grouped reference CRTC | Tasks 1, 4, 7–8 |
| Promotion, descriptor and scratch cleanup | Tasks 3, 5, 10 |
| Completion progress without scene/VT/DPMS | Task 6 and Task-9 recipient servicing |
| No production 2c-ii/iii or stage-3/4 implementation | Scope and task boundaries throughout |

Self-review performed during drafting (execution checkboxes above reflect the round-1 review, not the drafting state):

- [x] Compare each row above against the reviewed design and inventory; resolve omitted requirements in the responsible task.
- [x] Search for unfinished instructions and inconsistent type names; fix the text without simulating compilation.
- [x] Validate local file links, `git diff --check`, and whitespace in this new plan.

The next gate is an explicitly authorized adversarial reassessment of the corrected **implementation plan**, with the accepted design as authority and the three-block production boundary explicit. If execution is later requested, use `superpowers:executing-plans` in this worktree, task by task; an alternative delegation workflow requires the user's choice. No additional permission is needed merely to finish or correct this document.


## Local revision after first plan review and upstream a06cf0e0 — 2026-09-10

B-1: whole GPU batches validate before mutation and remain rooted on every failure.
M-1: the resource consumer now owns capacity and receives delayed-availability callbacks.
M-2: real writer entry points and transport-counting tests complement enum tests.
m-1: grouped release membership retains stable CRTC plus topology/CRTC generations.

Upstream additions: Task 3 preserves imported FD/layout/size metadata; Task 8 rejects
implicit layouts at direct preparation; Task 7 preserves core overlay claims and
independent physical COW retention. These are plan edits only, not implemented
fixes or another external review. The existing Chrome fix repairs client round
trip; general server sampling of unresolved implicit layouts remains a known
upstream limitation, not a capability supplied by this plan.
