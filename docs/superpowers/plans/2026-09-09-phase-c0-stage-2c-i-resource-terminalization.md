# Phase C0 Stage 2c-i Resource Ownership and Terminalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Do not dispatch additional agents without the applicable authorization.

**Goal:** Retain real allocation generations across commit outcomes, separate Present completion from release, and provide bounded resource and teardown interfaces for subsequent C0 stages.

**Architecture:** Keep `DeviceCommitOwner<R>` backend-independent and instantiate its backend boundary with `CommitResources`. A core-thread resource service roots concrete allocations, arbitrates usage and proof-gated cleanup, and survives by-value handoff into an incarnation bundle. Legacy production submission remains active; converted resource adapters are exercised through real backing objects and controlled completion/transport fixtures until later stages supply activation prerequisites.

**Tech Stack:** Existing Rust 2024 workspace, Vulkan/ash, DRM/GBM, existing owner/executor and core poll facilities. No new dependency or ioctl is required by this plan.

**Spec:** Read [resource design](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md), [concrete adapter inventory](../specs/2026-09-09-phase-c0-stage-2c-i-resource-adapter-inventory.md), [2c decomposition](../specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md), and governing [C0 specification](../specs/2026-08-26-phase-c0-atomic-kms-migration-design.md), especially §§9.1, 10–10.4, 12 and 18.

**Status:** Executable-plan draft; no task executed. Its [first plan review](../findings/2026-09-09-stage-2c-i-implementation-plan-review-round1.md) reported 1 blocking, 2 major, 1 minor and its [second plan review](../findings/2026-09-10-stage-2c-i-implementation-plan-review-round2.md) reported 1 blocking, 2 major, 0 minor, both with complete declared coverage; round 2 recorded all four round-1 corrections as applied. Round-2 B-1 (KMS disposition at teardown release), M-1 (owner-writer authority) and M-2 (overlay physical-retirement transition) are corrected below. The [third plan review](../findings/2026-09-10-stage-2c-i-implementation-plan-review-round3.md) — run through `review-claude.sh` and not comparable to the codex rounds — reported 1 blocking, 2 major, 2 minor with complete declared coverage and recorded all three round-2 corrections as applied; its B-1 (registry-rooted GBM device holders block the fd-family barrier), M-1 (no producer for the old set's `KmsRelease` proof), M-2 (quiescing precondition), m-1 and m-2 are corrected below. Those corrections are local and have not received another external pass. The [round-3 review](../findings/2026-09-09-stage-2c-i-adversarial-review-round3.md) accepted the design as a basis for writing this plan. It did not review this plan. Original baseline `14dd92d818e619357c903ad64e5357f47619111e`; this revision accounts for integration of upstream `a06cf0e00c5ba41431966732b71df802b7d0a51b`, including Chrome DRI3 and Composite overlay ownership fixes. Earlier design/plan verdicts do not certify these additions.

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
}
impl AllocationLease {
    pub(crate) fn key(&self) -> AllocationKey;
}
```

The returned `adopt` lease is `Retain`; the registry owns payload even after that lease drops. Cleanup on `service_ready` is explicit. `service_ready` returns availability transitions for waiting consumers, not a license to acquire without another serialized reserve.

- [ ] **1.1 Write the first failing test and the minimal fixture.** In `resources/tests.rs`, define the fixture below. `AllocationPayload::Spy` contains the object, not just its ID.

```rust
#[derive(Debug)]
struct SpyAllocation { drops: Rc<Cell<usize>> }
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

- [ ] **1.2 Run** `cargo test -p yserver --lib c0_2ci_kms_release_does_not_complete_gpu_work`. Record the missing module/API failure before implementing.
- [ ] **1.3 Implement the entry state and atomic reserve.** Each entry tracks live uses, unresolved obligations, frozen state and payload. Use `BTreeMap`/`BTreeSet` for deterministic tests; a pending set empties only on matching evidence. Read/write compatibility is evaluated inside the same mutable service operation that inserts `UseId`.

```rust
fn can_destroy(entry: &AllocationEntry) -> bool {
    !entry.frozen()
        && entry.live_use_count() == 0
        && entry.pending_obligation_count() == 0
}
```

Implement the three accessors against that entry's single availability state. `Retain` prevents destruction but does not alone license access. `Write` excludes live Read/Write/Kms uses and pending GPU/read/KMS/FOREIGN work. `Read` excludes writers and requires its adapter's route-specific ownership/readiness check. `Kms` use is registered before dispatch and cannot be released by a CPU reference drop. A read of an already scanning-out image is permitted only through the read adapter's explicit route checks; do not make a generic pending-KMS prohibition that breaks successful synchronous snapshots.

- [ ] **1.4 Add reverse-order, alias, stale-key, duplicate-proof and cancellation tests.** Reuse the first test with GPU before KMS; hold two `Retain` leases and verify only the second drop permits destruction; repeat the same proof without a second cleanup; allocate a new generation and deliver old evidence; drop a Write lease after registering GPU work and verify reuse stays blocked. Frozen entries remain retained after all normal proofs.
- [ ] **1.5 Run** `cargo test -p yserver --lib c0_2ci_`, format, run required clippy, and commit these Task-1 files with `feat(kms): add allocation leases and availability ledger`.

## Task 2: Consuming DRM cleanup and real direct framebuffer retention

**Files:** Create `resources/drm_cleanup.rs`; modify `drm/modeset.rs` and `resources/mod.rs`.

**Consumes:** Task-1 allocation entries; existing `DirectScanoutProbeFramebuffer` handles.

**Produces:** `DrmCleanupRegistry`, `DrmCleanupRight`, `FileFamilyClosed`, and `DirectFramebufferAllocation`. These are core-thread types. The right contains registry identity plus FB/GEM identity, never an untracked `Rc<Device>`. `FileFamilyClosed` has a private constructor: only the registry can mint it after full closure, and Task 2 supplies fake descriptor-family closure tests; Task 9 adds helper-reap and late-reply integration. No public boolean claims closure.

```rust
impl DrmCleanupRegistry {
    pub(crate) fn consume(&mut self, right: DrmCleanupRight)
        -> Result<(), (std::io::Error, DrmCleanupRight)>;
    pub(crate) fn freeze_incarnation(&mut self);
    pub(crate) fn retire_closed_family(&mut self, proof: FileFamilyClosed);
}
```

- [ ] **2.1 Write a counting-transport cleanup test.** Define a module-private `CleanupIo` trait with `remove_fb(u32) -> io::Result<()>` and `close_gem(u32) -> io::Result<()>`. Its real implementation borrows the registry's original device only for the call. Its test implementation appends `RemoveFb(id)`/`CloseGem(id)` to `Rc<RefCell<Vec<CleanupCall>>>`; define that two-variant enum in the same test module. Register FB 11/GEM 12, consume its right, then service/drop all references and assert the exact log:

```rust
assert_eq!(calls.borrow().as_slice(), &[CleanupCall::RemoveFb(11), CleanupCall::CloseGem(12)]);
```

- [ ] **2.2 Run** `cargo test -p yserver --lib c0_2ci_drm_cleanup` and observe missing cleanup API/test failures.
- [ ] **2.3 Implement consuming cleanup stages.** Use `Registered`, `FramebufferRemoved`, `Discharged`, `Frozen` states. On RMFB success/GEM-close failure return a right at `FramebufferRemoved`, so retry cannot issue RMFB twice. On error retain rights and close converted admission. Complete-family closure discharges only file-owned rights and prevents later ioctls; shared Vulkan/GBM payload remains in Task-1 availability. Do not reopen the device by path. Registry-held aliases and helper aliases must be accounted for; a single closed control FD cannot mint `FileFamilyClosed`. Every `Rc<drm::Device>` held inside a registry-rooted payload context — the baseline `GbmDevice = gbm::Device<Rc<drm::Device>>` is one — is a **counted alias** of the same inherited open file description, registered at adoption, not a hidden reference (plan-review round-3 B-1).
- [ ] **2.4 Add `DirectScanoutProbeFramebuffer::into_managed`** as an ownership-consuming conversion that extracts FB/GEM and transfers original-device ownership into the registry. Preserve the legacy destructor for legacy values by replacing inner ownership with an explicit `Legacy`/`Managed` representation; moved managed values have no destructor capable of issuing old ioctls. `DirectFramebufferAllocation` retains its Task-1 source allocation leases; cache entries for managed imports become weak indices.
- [ ] **2.5 Test cache eviction with a live lease, frozen rights, partial cleanup failure and complete-family closure followed by final lease drop.** Count actual payload destruction as well as transport calls. Injecting `FileFamilyClosed` in tests uses the registry's fake family inventory and closure steps, not a free proof constructor. Add a case where a GBM/Vulkan dependency remains pending after all file-owned rights discharge.
- [ ] **2.6 Run** the focused tests, the three target checks from Task 10 because this task changes DRM cleanup typing, formatting and required clippy. Commit only these files with `feat(kms): make managed framebuffer cleanup proof gated`.

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

- [ ] **3.1 Add a real-storage retirement test.** In the existing store test module, extend `decref_then_realloc_then_retire_keeps_new_xid_mapping` to retain a managed allocation from the old drawable before reallocation. Keep its original offset/depth in `PixelIdentity`. Assert old destruction is delayed, new lookup still selects the new ID, and old cleanup does not reset the new drawable's content/damage state. Use existing null-storage tests for logical ordering and a live Vulkan version in Task 10 for actual image/view lifetime.
- [ ] **3.2 Run** `cargo test -p yserver --lib c0_2ci_storage` before adding the adapter.
- [ ] **3.3 Extract physical fields into `StorageAllocation`.** Move image/memory/views, format/depth/extent/current Vulkan layout, imported owner/metadata and promoted-export metadata listed in the inventory. Keep drawable identity, scene damage, dormancy and current selection in `DrawableStore`. Provide `Storage::into_managed(self, service: &mut ResourceService, target: PaintTarget, content_offset: (i32, i32)) -> Result<StorageLease, (ResourceError, Storage)>` at the owning boundary; on failure reconstruct the original storage and return it. A live logical drawable retains its own allocation lease when its backing is managed; do not consume the drawable's only reference to create an intent. Introduce `StorageBacking { Legacy(StorageAllocation), Managed(StorageLease) }` beneath the store's logical facade, updating its field accessors and all affected engine consumers in this task. The payload contains `StorageAllocation`, never that facade, so there is no recursive ownership. Adoption captures extent from the allocation and target/offset from the resolved drawable, then assigns the new allocation key. `ResourceService::retain_storage(&mut self, source: &StorageLease) -> Result<StorageLease, ResourceError>` creates another Retain use with the same captured identity; no implicit `Clone` issues a new usage.

For managed storage, `destroy_now`, `poll_pending_retire` and `shutdown_destroy_all` detach logical references and submit invalidation/cleanup work. The service retains actual image owners and their contexts. Invalidation must occur before image cleanup; its job retains the necessary cache entries, not a closure capturing `&mut KmsBackend`. Imported image aliases remain single-owned; sample view is destroyed before dropping the imported owner. Preserve the legacy storage constructor and cleanup route until a producer adopts managed storage explicitly.

- [ ] **3.4 Implement scoped access and relayout exclusion.** Define `ResourceService::with_storage_read<T>(&mut self, lease: &StorageLease, f: impl FnOnce(&StorageAllocation) -> T) -> Result<T, ResourceError>` and the corresponding `with_storage_write<T>` with a `&mut StorageAllocation` closure. Write access first reserves compatible usage and cannot escape a raw reference. GPU work records an obligation before ending the CPU access scope. Preserve full `PaintTarget`, including `x11_depth`, when capturing deferred work.

```rust
let pixels = &lease.pixels;
assert_eq!(pixels.target.x11_depth(), 24);
assert_eq!(pixels.allocation, lease.allocation.key());
```

Use this assertion in the depth-24 target/depth-32 backing regression. For border relayout, first attempt exclusive layout/write reservation. If busy, take the existing separately allocated copy path within pool limits; if no capacity, defer layout publication and request the existing scene retry. Never change `content_offset` before moving pixels, and never bump a generation to justify overwriting held storage.

- [ ] **3.5 Convert promotion retirement.** `adopt_exportable` publishes a new allocation generation. Old `RetiredImage` becomes a retained payload guarded by every old usage plus the existing render ticket. `retire_image_after` and `destroy_retired_image` feed the service for managed payloads. Returning ordinary storage to `PixmapPool` is authorized only after all uses/tickets; promoted/imported storage remains pool-ineligible. Pool checkout establishes a new generation.
- [ ] **3.5a Preserve upstream DRI3 buffer identity.** Extend the retained imported owner with the original `ImageBacking::Imported::dma_buf_fd`, `DrawableImage::drm_modifier`, `import_plane0`, `import_size`, and `ImportedDmabufMetadata::implicit_layout`. Preserve `Dri3ImportModifier::Implicit` versus `Explicit(m)` through import; do not reinterpret the server's guessed Vulkan view as a verified client layout. Imported re-export duplicates the original client FD with its original stride/offset and stated legacy size; do not replace this with `vkGetMemoryFdKHR` or `lseek`. Implicit export reports `DRM_FORMAT_MOD_INVALID`; explicit export retains its supplied modifier. Copying or moving a lease cannot relabel the metadata. Retain the window modifier list's `drmFormatModifierPlaneCount == 1` constraint; multi-plane import is still out of scope.
- [ ] **3.5b Add DRI3 lease regressions.** Extend upstream's imported-buffer metadata/export test across managed adoption, logical FreePixmap and deferred retirement; use a distinguishable client size so a Vulkan-derived substitute fails. Assert no client FD offset change, explicit/implicit modifier preservation and once-only FD ownership. No-ICD or export-not-supported conditions are reported as such; do not hide arbitrary fixture failure as a successful test.

- [ ] **3.6 Test** in-place relayout exclusion, allocate-and-copy retaining both allocations, promotion with an old read/KMS lease, new XID preservation, depth semantics and no premature pool return. Run `cargo test -p yserver --lib c0_2ci_storage`, existing border-width/promotion tests, format and required clippy. Commit with `feat(kms): retain storage generations across deferred use`.

## Task 4: Shared/copied scanout backing and pool reuse

**Files:** Create `resources/scanout.rs`; modify `kms/vk/scanout.rs`, `kms/render/platform.rs` and the resource payload enum.

**Consumes:** Tasks 1–3; existing `ScanoutBo`, `OutputScanout`, `CopiedRenderSource` and copied ownership state machines.

**Produces:** `ScanoutAllocation`, `CopiedSourceAllocation`, `ManagedScanoutToken`. Extract backing fields at their defining module boundary to preserve private invariants; do not make every BO field public.

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

- [ ] **4.2 Run** `cargo test -p yserver --lib c0_2ci_scanout` and confirm missing managed acquisition behavior.
- [ ] **4.3 Move physical fields into retained payloads.** Shared payload includes image/memory/view, FB/GEM rights, transfer/staging/query resources, exact Vulkan context and GBM BO/device retention. The retained `Rc<GbmDevice>` is registered with the Task-2 registry as a counted alias of the DRM description at adoption; Task 9 defines how the barrier discharge closes it (round-3 B-1). Copied payload additionally retains renderer target, exported transport, sink import, semaphores, return sync file and both Vulkan contexts. Move the existing `CopiedSourceOwnership`, `CopiedDestinationOwnership` and semaphore-reuse state into the authoritative entries or delegate to them; do not maintain independent writable copies in the pool and service.
- [ ] **4.4 Implement all-or-nothing pair acquisition.** Reserve all required entries before returning a token. On the second reservation's failure, release the first reservation without clearing its GPU obligations. A `BoPhase::Free` check alone is insufficient. Validate stable output key/topology generation before touching indices.

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

- [ ] **4.5 Adapt managed cancellation, replacement and cleanup.** `cancel_scanout_bo_recording` ends recording but not in-flight work. `note_kms_retired` supplies the copied destination's matching replacement evidence; `release_completed_source` must also prove the sink/renderer semaphore dependencies before consuming them. Preserve `ReleasedButAtomicRejected`; ioctl rejection is not ownership return. `reset_scanout_bos_for_suspend`, `drain_scanout_pool_at` and installed-pool replacement detach managed entries. Managed Drop cannot issue legacy RMFB, reset active fences to Free or destroy transfer resources.
- [ ] **4.6 Test** copied rejection after external release, partial grouped replacement, topology reuse of a BO index, alias acquisition, cancellation after GPU dispatch and cleanup ordering including GBM. Run focused tests, format and clippy; commit with `feat(kms): retain scanout allocations and gate pool reuse`.

## Task 5: GPU, descriptors and readback lifetime

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

- [ ] **5.1 Write the source/scratch regression.** Extend the existing root IncludeInferiors snapshot test path with managed source and scratch allocations. Observe that successful readback produces owned CPU bytes before scratch upload; source-read completion is recorded then, whereas scratch cleanup remains behind its own upload/Composite ticket. Assert source retention is not extended solely by scratch use. Add the opposite failure case: uncertain read submission leaves source/staging retained and closes the converted route.
Instrument the real adapter's cleanup boundary with source/scratch destruction counters. After snapshot readback returns and after scratch GPU completion, respectively, the test body must contain:

```rust
assert_eq!(source_read_pending, 0);
assert_eq!(scratch_drops.get(), 0);
assert!(!scratch_ticket.poll_signaled_result(&vk).unwrap());
```

Here `source_read_pending` is the service's count for the source read obligation, not all KMS uses of the source; `scratch_ticket` is the scratch's actual ticket. Submit/signify completion with the fixture's controlled GPU path, service the batch, release the scratch's final logical lease and assert `scratch_drops.get() == 1`. A separate pending-read case asserts the source and staging destruction counters remain zero across backend detach. Declare test-only count accessors on the real entries; do not replace the adapter with an event-log simulator.

- [ ] **5.2 Run** `cargo test -p yserver --lib c0_2ci_read` and `cargo test -p yserver --lib c0_2ci_gpu` before the new adapters.
- [ ] **5.3 Register dependencies before dispatch.** At managed frame submission, enumerate every read/write allocation, reserve use and register its obligation before the GPU can use raw handles. Move descriptors/command slots into `CoreRetirementBatch`. Bind the returned ticket on success; on proven pre-submit failure cancel only those obligations that provably never reached GPU execution. On uncertainty freeze them.
- [ ] **5.4 Use real ticket status for completion.** Implement `ResourceService::poll_gpu(&mut self, now: Instant) -> Result<(), ResourceError>` over registered batches. Match the existing API without converting errors to success:

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

## Task 6: Completion progress and transport permission boundary

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
- **Consumption.** One grant authorizes exactly one dispatch, consumed by value through `consume_owner_write`. Serials are never reissued, so a replayed or reconstructed grant is rejected. A grant that is dropped instead of consumed authorizes nothing and does not decrement the outstanding count; the gate keeps the charge and closes admission, mirroring the lost-role-token rule in Task 8. Dropping is never the release path.
- **Revocation and ordering.** `begin_quiescing`, `close` and handover refuse while `outstanding_owner_writes() != 0`; `revoke_owner_writes` is the explicit resolution and returns the count it invalidated. This extends the existing rule that Owner cannot publish while a helper permission or disposition is outstanding. `WriterClass::HelperMutation` grants additionally resolve or revoke the issued helper permission before handover.
- **Scope.** Concrete stage-3/4 producers of these grants remain deferred; this stage supplies the vocabulary, the validation site and the tests only.

- [ ] **6.1 Add a no-composition progress test.** Use the existing core-loop fake backend completion tests. Register one unsignaled ticket, set VT-away/DPMS-off and no damage, then signal it through the adapter and verify the service runs, its allocation becomes available and no scene submission occurs. Assert a pending ticket schedules a future deadline, and a failed ticket closes the route without repeated immediate deadlines.
- [ ] **6.2 Run** `cargo test -p yserver --lib c0_2ci_progress` and the core-loop completion tests.
- [ ] **6.3 Move service polling outside composition gates.** Invoke service work from the established completion callback and `before_block`, before any scene/VT/DPMS early return. Chain `ResourceService::next_deadline()` unconditionally in `next_wakeup`. For tickets without exportable FDs use a **1 ms** positive retry interval, coalesced to one service deadline; successful evidence services availability in the same wake. Each such ticket carries a bounded pending deadline derived with checked arithmetic (round-3 m-1); on expiry the batch is frozen, converted admission closes and the retry is not re-armed. Expiry is never a completion proof: the batch stays rooted for teardown like a failed ticket. Checked time overflow closes managed admission and retains work. Failed/device-lost tickets are retained for teardown rather than polled forever. Keep existing FD pollers; the serialized inbox does not need a new OS thread or synthetic ready FD.
- [ ] **6.4 Register waiter before rechecking availability.** A waiter is keyed by allocation generation and consumer (`Pool`, `DirectCapacity`); define this two-variant enum in `completion.rs`. Store a set to coalesce wake notifications. On reserve failure register the consumer and recheck in the same service turn. On an eligibility edge enqueue one consumer wake and clear its registration; consumer retries reserve, not an unchecked index acquisition. Completion arriving during registration must either be observed by recheck or produce the wake.
- [ ] **6.5 Implement and test the gate.** Gate initial state is Legacy. Quiescing revokes all new legacy writer permissions before issuing the drain; Owner cannot publish while any helper permission or disposition is outstanding. **Precondition (round-3 M-2):** `begin_quiescing` returns `ResourceError::Busy` while any direct ownership unit is `Current`, `Submitted` or `Successor`, or while an unflip is requested and not retired. Exiting direct scanout is the last legacy write and precedes quiescing; `Unflip` is not a class `Quiescing` permits, so the all-classes-false assertions stand unchanged and no sink gains a bypass. Add a test that `begin_quiescing` under active direct scanout refuses without changing state, and succeeds after the unflip retires. Closed cannot return to Legacy on the same incarnation. Add table-driven tests:

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

- [ ] **6.6 Run** focused and existing handover/core completion tests, format and clippy. Commit with `feat(kms): service resource completions independently of composition`.

## Task 7: Concrete commit resources and Present dispositions

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

- [ ] **7.1 Add an owner integration test with actual leases.** Use existing `kms/owner/test_fixtures.rs` request/outcome fixtures but instantiate `DeviceCommitOwner<CommitResources>`. Submit old A/new B, generate HardwareComplete before Presented, then finish presentation and transfer the accepted resource event. Drop the event vector after consumption and assert A and B are still retained by their correct owner until each proof. Test the same sequence using both supported orders of page/fence evidence.
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

- [ ] **7.2 Run** `cargo test -p yserver --lib c0_2ci_commit` before switching the backend boundary type.
- [ ] **7.3 Replace backend/platform `NeverResource` parameterizations with `CommitResources`.** Include return signatures, `DispatchError` signatures, device constructors and test helpers. Retain the generic owner's resource-agnostic API and existing empty-resource Legacy behavior. Search both `kms::owner::NeverResource` and `owner::ledger::NeverResource`; the baseline contains separate declarations, so do not accidentally migrate only one spelling or delete a declaration still used by generic tests.
- [ ] **7.4 Register commit dependencies before IPC and handle every resource event by value.** Register one correlated KMS obligation for each displaced (`old`) allocation and member and retain all source/fallback uses before constructing `Submitted::new(old, new)` or sending the executor request; store the `(AllocationKey, ObligationId, GroupMember)` triples in `CommitResources::kms_obligations` so the consumer can correlate without a side table (round-3 M-1). Dependency identity includes commit/CRTC, not only `ObligationKind`. Pre-IPC failure returns the exact resources and registration ownership for proven cancellation. Preserve request-time `CompletedPresentEvent::crtc_id`, `crtc_epoch`, `msc_offset`, `window_generation` and reference `completion_clock`; do not reconstruct them from current topology.

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

- [ ] **7.5 Split source/fallback pin ownership from numeric protocol handles.** For managed pins the table entry owns `StorageLease` and an invalidation-aware logical decref obligation. `release_present_source` removes that entry once and lets the service run the appropriate cleanup. `retained_present_wakes` moves the actual pinned object into `PresentRelease`. Never look up a reused XID to reconstruct it. Completion suppression consumes FIFO bookkeeping but cannot signal release.
- [ ] **7.5a Preserve core-owned overlay claims.** `ServerState::cow_claims` is the only logical claim authority; `KmsCore::cow_refcount` no longer exists. Backend overlay methods receive only 0→1 and 1→0 edges. Managed COW/source/fallback leases retain physical allocation after a final logical release until safe unflip/replacement. Do not infer a protocol claim from a surviving lease, and do not clear the sticky failure or duplicate claim counts in the resource consumer.

**Physical-retirement transition (plan-review round-2 M-2).** The baseline already owns this edge in `KmsBackend::deferred_cow_release` ([backend.rs:1164](../../../crates/yserver/src/kms/render/backend.rs:1164)); keep it as the single transition, keyed by the retained identity (`cow_id`/allocation key) rather than by a generation counter, and add no second one in `CommitResourceConsumer`:

- **1→0 with `scanout_m2.active()`.** `materialize_direct_shadow_for_unflip` must succeed before anything is released; on `Err` the COW and its direct pins are untouched and core keeps the caller's claim ([backend.rs:20755](../../../crates/yserver/src/kms/render/backend.rs:20755)). On success request the unflip, set `deferred_cow_release`, and keep the retained `StorageLease` (plus any COW source/fallback lease) rooted. The logical release arms no KMS release obligation and discharges none.
- **0→1 while `deferred_cow_release` holds.** Reuse the retained `cow_id`/`StorageLease` identity: clear the flag, allocate no storage, perform no second import, create no second physical owner and no new allocation generation ([backend.rs:20632](../../../crates/yserver/src/kms/render/backend.rs:20632)). Only the protocol resource is materialized again, through `materialize_cow_resource`. Preserve the existing invariant that a live `cow_id` on this edge implies a deferred release.
- **Pending versus already-dispatched unflip.** Neither is cancelled by a re-claim. The unflip runs to its stop path, which calls `finish_deferred_cow_release` ([backend.rs:1853](../../../crates/yserver/src/kms/render/backend.rs:1853), invoked at [backend.rs:2110](../../../crates/yserver/src/kms/render/backend.rs:2110)); with the flag cleared it retires nothing. The retirement decision is therefore read at stop time, not at request time, so no cancelable/dispatched branch is introduced and no armed obligation can retire a reclaimed resource.
- **Stop path never runs.** On device loss or unknown completion the retained lease stays quarantined and rooted and moves in the Task 9 bundle; core's claim state is unaffected.
- **After a completed `finish_cow_release`.** The identity is gone, so a later 0→1 allocates fresh storage with a new allocation generation; stale evidence for the old generation cannot retire it.
- **Failure routes.** Preserve request-failure claim retention, and disconnect's release-claims-anyway plus sticky `cow_teardown_failed` ([composite_overlay.rs:122](../../../crates/yserver-core/src/core_loop/composite_overlay.rs:122)).

Test final release/disconnect with a direct frame, delayed physical retirement and re-claim: assert the deferred release drops no lease and decrefs no storage; the re-claim performs zero new imports/allocations and keeps the same allocation key and generation with exactly one logical claim in `cow_claims`; the later stop path frees nothing and the COW survives; the no-re-claim ordering decrefs exactly once; after a completed `finish_cow_release` and a fresh 0→1 allocation, late stop-path or unflip evidence for the old identity retires nothing (round-3 m-2); and both failure routes behave as above.

- [ ] **7.6 Add grouped/Skip tests.** A two-output frame completes only with all required evidence; select the recorded reference CRTC sample even if the other arrives last or has larger MSC. Replace only one output and assert shared source stays retained. Supersede never-submitted successor: idle/drop its resource leases now, keep Skip metadata ordered after predecessor and `emit_idle=false`; accepted work ending as Skip retains the stronger release rule. Repeated stale/duplicate notifications cannot double-complete or double-release. Add the round-3 M-1 regression: the **only** KMS proof reaches the service through the owner's `HardwareComplete` event — the test body calls no `apply_validated_proof` — and asserts the `old` set's obligations are discharged, the `new` set's are untouched, a partial grouped replacement discharges only the matching `GroupMember`, and a rejected commit cancels rather than discharges.
- [ ] **7.7 Run** focused owner/backend Present tests, format and clippy. Commit with `feat(kms): consume concrete commit resources and split present release`.

## Task 8: Six physical roles and release-safe exit

**Files:** Create `resources/capacity.rs`; extend `commit.rs` and the managed backend preparation boundary. Preserve legacy direct scheduling until 2c-ii/iii conversion.

**Consumes:** Task-7 resource sets and service eligibility. Defines capacity only, not seven-tier scheduler policy.

**Produces:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectRole {
    Current, Submitted, Successor, Preparing, OrdinaryRetirement, ExitRetirement,
}
pub(crate) struct RoleReservation { role: DirectRole, serial: u64 }
impl DirectCapacity {
    pub(crate) fn new() -> Self;
    pub(crate) fn reserve(&mut self, role: DirectRole)
        -> Result<RoleReservation, ResourceError>;
    pub(crate) fn attach(&mut self, slot: RoleReservation, value: CommitResources)
        -> Result<CommitResources, (ResourceError, RoleReservation, CommitResources)>;
    pub(crate) fn occupied(&self) -> usize;
    pub(crate) fn can_enter_direct(&self) -> bool;
}
```

Task 8 puts the sole `DirectCapacity` inside `CommitResourceConsumer`. Its existing `consume(event, service)` signature therefore has direct access to capacity and cannot orphan a returned token. Add `CommitResourceConsumer::on_available(&mut self, keys: &[AllocationKey], service: &mut ResourceService) -> Result<(), ResourceError>` and invoke it in the same serialized resource-service turn that publishes eligibility. It rechecks the waiting old/new resource sets, consumes releasable role tokens with `finish_role`, and schedules direct admission once. Rejection restores Current and disposes the rejected token only when its remaining dependencies finish; ordinary and exit retirements each finish their own token; unknown retains all charges. Any transition error keeps resources/tokens rooted and closes admission. The handoff transfers this consumer including its capacity, never a second independent capacity table (plan-review M-1).

`DirectCapacity` stores six non-owning `RoleState` values: `Vacant`, `Reserved(serial)`, or `Occupied(serial)`. Task 8 adds `direct_role: Option<RoleReservation>` to `CommitResources`; `attach` puts the reservation there and returns the resource set to its sole owner. The consumer/commit owner/release ledger owns resources, never the capacity table as well. A serial is checked monotonic. Define `cancel_reservation(&mut self, slot: RoleReservation) -> Result<(), (ResourceError, RoleReservation)>` for proven pre-import cancellation and `move_role(&mut self, slot: &mut RoleReservation, to: DirectRole) -> Result<(), ResourceError>` for role changes. Invalid operations preserve source ownership. For a pre-reserved retirement destination, define `move_into_reserved(&mut self, occupied: &mut RoleReservation, reserved: RoleReservation) -> Result<(), (ResourceError, RoleReservation)>`: validate both slots before changing either, then vacate the source and adopt the destination serial atomically. A plain move to an already reserved slot is invalid. End occupied accounting through `finish_role(&mut self, slot: RoleReservation) -> Result<(), (ResourceError, RoleReservation)>` only after the consumer has discharged the role's release obligations. Dropping a token cannot free capacity; an unexpected lost token closes admission with its slot charged. Resources moved into the generic owner carry their tokens, so accounting remains charged without a second owning copy.

- [ ] **8.1 Add the full-capacity test.** Reserve each role once and reject another Preparing reservation:

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

- [ ] **8.2 Run** `cargo test -p yserver --lib c0_2ci_capacity` before implementation.
- [ ] **8.3 Reserve before retaining/importing.** Managed direct preparation rejects `implicit_layout` before any direct import/validation, preserving upstream's `m1_gate_reject_import` behavior; a guessed LINEAR modifier is never scanout qualification. It then takes Preparing, then pins source/fallback and imports/tests FB. On proven failure clean up the candidate and cancel the role, retaining the existing successor. On successful validation atomically replace Successor, idle/release the victim and retain only its ordered Skip metadata. If victim/preparing cleanup is uncertain, charge its role and close admission instead of proceeding with another import. Managed probe cache stores no additional strong framebuffer leases.
- [ ] **8.4 Reserve retirement before replacement.** An ordinary replacement cannot dispatch with occupied OrdinaryRetirement. Keep the latest Successor while waiting and register a service wake. At commit dispatch, move Current/New leases into `Submitted<CommitResources>` by value while retaining role accounting; on acceptance return new to Current and old to the pre-reserved retirement role, on rejection restore Current and dispose new, on unknown retain both in owner quarantine without freeing capacity.
- [ ] **8.5 Implement exit-resource accounting.** Before managed direct entry, retain one release-safe composed return allocation per affected output. Unflip cancels unsent direct work, waits for submitted work and uses ExitRetirement for Current even if OrdinaryRetirement is occupied. Materialize shadow and resolve source/GPU dependencies before using the composed return resources. Do not display stale pixels. Re-entry requires both retirement roles vacant; no additional pool/frame allocation is allowed to work around a blocked exit.
- [ ] **8.6 Test** A retired/B current/C-D-E successors; B unflip while A awaits release; repeated entry/exit; failed cleanup; partial grouped release; and immediate-ready retirement. Observe a maximum of six charged positions, bounded live imports and once-only ordered victim completions. Drive the real `consume` → delayed `on_available` → `finish_role` path for rejected, ordinary-retirement and exit-retirement tokens, and verify each makes a previously blocked reservation available without dropping a token to free its charge. Run focused tests, format and clippy; commit with `feat(kms): bound managed direct resource roles`.

## Task 9: Reserved teardown recipient and late-completion handoff

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
pub(crate) enum DeviceBarrier { FileFamilyClosed, DeviceLost }
```

`Discharged` requires correlated `PriorBufferReleased` for that exact commit/CRTC generation; a reused CRTC number at another generation does not qualify. `DeviceBarrier` has exactly two private constructors, both scoped to the producing `DrmDeviceKey`: the registry's `FileFamilyClosed` for the complete `IncarnationFdSet` of that device, issued only after every submitter has stopped and the incarnation is detached from event dispatch (Task 9.5), and a proven device removal/loss for that device key. Nothing else constructs one — not an expired deadline or watchdog, not control-IPC or single-alias closure, not helper reap alone, not an `ExecutorStalled`/`ShutdownExecutorStalled` bounded exit, and not GPU/read/FOREIGN evidence. The barrier supersedes the KMS obligation only: per governing §10 it is a barrier solely for resources owned exclusively by that open file description, so shared dma-buf/GBM/Vulkan state keeps its independent proofs. A barrier for one device never supersedes an obligation registered against another device key.

**Reaching the barrier with registry-rooted device holders (plan-review round-3 B-1).** Quarantined GBM-backed payloads retain `Rc<GbmDevice>`, which is a duplicate of the incarnation's open file description (`from_inherited_kms_fd`, `MasterOwnership::InheritedDuplicate`). If those holders merely waited for `FileFamilyClosed`, the description could never close and the barrier could never be minted — the leak outcome the spec forbids. They are therefore counted aliases whose closure is performed by the barrier discharge itself, in this order on the serialized recipient path:

1. `FileFamilyClosed` becomes **mintable** when every submitter and dispatch path is detached, the helper is reaped, the control alias and every non-payload alias are closed, and the only remaining holders of the description are registry-rooted payload contexts.
2. Discharge then destroys those contexts in dependency order: GBM BOs first (their GEM close runs against the still-open description and is not a stale-handle ioctl), then the `GbmDevice`, whose drop releases the payload's duplicate.
3. The registry closes its own last alias as the final step and mints `FileFamilyClosed`; only then does the `Superseded(FileFamilyClosed)` disposition exist and `apply_teardown_release` proceed.

Step 2 is safe with respect to the unresolved KMS obligation because a DRM framebuffer holds its own kernel reference to the GEM object: closing the userspace handle frees nothing while the framebuffer exists, and the kernel removes the framebuffer and disables the plane when the description's last reference goes. No userspace ioctl touches the buffer after step 3. Shared dma-buf and Vulkan contexts are not part of this order; they keep their independent proofs.

`apply_teardown_release` returns `ResourceError::InvalidProof` while any entry named by the proof still holds an `Outstanding` KMS obligation, even when every other disposition is satisfied — including the vacuous case where the entry has no exclusively-file-owned handles left. Unresolved KMS ownership without a barrier is not a leak decision the capability may take: entries stay quarantined and rooted through the shutdown deadline and process-exit policy (governing §10 shutdown row), which is the specified outcome for an unreaped helper holding a device alias.

- [ ] **9.1 Write the detached-backend test.** Start a managed owner with A/B resources and dispatch uncertainty. Reserve a recipient, transfer, destroy the backend fixture, then deliver late reply descriptors and a delayed GPU completion to the router. Count resource destruction, fd cleanup calls and wake deliveries. Assert the same incarnation consumes the evidence, no current state is resurrected, and no old backend callback occurs.
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

- [ ] **9.2 Run** `cargo test -p yserver --lib c0_2ci_handoff` before creating transfer support.
- [ ] **9.3 Implement atomic move and routing.** On the serialized core path: close transport, stop acquisition, freeze uncertain entries, move the owner/current/retirement roles and service plus cleanup contexts into the pre-reserved slot, then publish that slot as the sole ingress target. Queue evidence racing the switch in the stable inbox and drain it under the new recipient. No fallible allocation is required after the transfer begins. Allocation failure when reserving the recipient prevents managed activation in the first place.
- [ ] **9.4 Preserve cleanup ownership during engine/store detach.** Transfer pending promoted images, descriptor slots, pinned wakes and invalidation work before destroying ordinary containers. An invalidation job must own the cache-entry slice it will destroy; it cannot reference the old engine. Managed `shutdown_destroy_drawables` detaches logical entries into service ownership. Keep the root router alive until final process exit policy, even if helper reap or GPU proof never arrives.
- [ ] **9.5 Test the complete fd-family barrier.** Register original device descriptors, helper/child aliases and pending reply-owned descriptors. Close control IPC alone: no proof. Reap helper but retain one device alias: no proof. Close the last registered file-description alias: issue the registry's private `FileFamilyClosed`, discharge file-owned FB/GEM rights, and verify late final lease drop issues no ioctl. Add a case with a **real** `Rc<drm::Device>`-owning payload (a `GbmDevice` over the fixture's inherited duplicate), not the fake family inventory: after reap and control-alias closure the barrier must still be mintable, the discharge must destroy the GBM BO before the device drop, the description's last close must be the registry's, and no ioctl may follow it (round-3 B-1). Keep shared Vulkan/GBM cleanup blocked until its independent proof. A late returned alias must be registered/closed under the same incident before closure can be certified.
- [ ] **9.5a Test that unresolved KMS ownership rejects teardown release (plan-review round-2 B-1).** Quarantine an acceptance-unknown direct commit whose old allocation carries a registered `KmsRelease` obligation for a known commit/`GroupMember`. Satisfy every other listed predicate: discharge or exhaust the entry's file-owned rights, complete its GPU and read tickets, and prove its FOREIGN return, while one helper alias keeps the fd family open (`ExecutorStalled`). Assert `apply_teardown_release` returns `ResourceError::InvalidProof`, the payload is not destroyed and no ioctl is issued. Then take each discharge route separately and assert exactly-once cleanup: correlated `PriorBufferReleased` for the exact commit/CRTC generation; and, from the reset fixture, complete-family closure after reap. Assert a `FileFamilyClosed` barrier for a second device key does not supersede this entry's obligation, and that a stale-generation `PriorBufferReleased` at a reused CRTC number does not either.
- [ ] **9.6 Test unavailable recipient, duplicate transfer, pending read at handoff and identity reuse.** Failure returns the intact bundle. A old-generation completion cannot free a new allocation/output at the same numeric index. Service deadlines keep running in the retaining recipient while backend composition is gone. Unknown never transitions back to ordinary Current after a late success. Run focused tests, format and clippy; commit with `feat(kms): define retaining incarnation handoff`.

## Task 10: Concrete adapters, integration evidence and handoff to 2c-ii

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

- [ ] **10.4 Run final software and portability checks.** These commands are required once the tasks are implemented; their presence here is not a claim they ran during drafting.

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
- [ ] **10.6 Run formatting and required clippy before the final task commit.** Stage only tests/documentation from this task and commit with `test(kms): verify resource terminalization adapters`. Do not squash merge or push as part of execution.

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

Self-review performed during drafting (execution checkboxes above remain unchecked):

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
