# Phase C.0 stage 2c-i — resource ownership and terminalization

**Status:** Revised after user approval of the Xorg/wlroots-aligned direction
on 2026-09-08: retain actual allocations, bound physical resources, and preserve
Present semantics without introducing protocol credits. Concrete adapter design
still requires review before an implementation plan. No code, hardware
activation or adversarial-review verdict is claimed.

**Baseline:** `523a96c7` combined with upstream master `f6c79967`.
See the [integration comparison](../findings/2026-09-08-stage-2c-master-integration.md).
Governing documents:
[C.0 specification](2026-08-26-phase-c0-atomic-kms-migration-design.md),
§§9.1, 10.2–10.4 and 12; and the
[2c decomposition](2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md).

## 1. Responsibility and boundary

Make the generic commit ledger capable of retaining actual production resources
and make protocol completion independent of resource release. Preserve the
existing evidence state machine. The next block must be able to reserve storage
for a prospective transaction, transfer ownership before IPC, and handle each
typed outcome without guessing whether dropping an identifier frees a resource.

This block supplies no primary producer conversion, damage application,
scheduler priority policy, lifecycle recovery or automatic quarantine release.
Production continues using its existing route until 2c-iii and later-stage
activation requirements are satisfied.

## 2. Observed ownership and required adaptations

| Resource | Baseline owner | Required guarantee |
| --- | --- | --- |
| Direct framebuffer/GEM import | `ScanoutM1ProbeEntry::_framebuffer`, removed by cache eviction/clear | Active and quarantined records retain the allocation independently of cache membership. |
| DRM destruction context for that import | `DirectScanoutProbeFramebuffer::device: Rc<Device>` | Preserve this strong reference and register its fd-lifetime implications with stage 3; do not duplicate raw fd ownership. |
| Direct source and fallback drawable | Store refcount plus `present_source_pins: HashMap<u64, DrawableId>` | A unique lease owns each retained reference; release still performs the backend's invalidation-aware decref. |
| Present release wake | `PinnedWake` and `retained_present_wakes` | Keep the pinned object/value, not a later XID lookup; protocol completion cannot signal it. |
| Composed scanout storage | Platform scanout pool and BO state machine | A lease keeps the backing allocation alive and excludes reuse through current, submitted and delayed-return states. |
| Scene submission allocations | `PendingAck` and GPU fence/pool retirement paths | KMS evidence cannot reset descriptors still used by the GPU. |
| Copied destination ownership | `CopiedDestinationOwnership` in `kms/vk/scanout.rs` | Preserve `ReleasedButAtomicRejected`; rejection is not a FOREIGN ownership-return proof. |

`DirectPresentFrame` currently owns numeric pins but borrows its framebuffer
through the cache. It cannot serve unchanged as the owner resource type.
`ScanoutBoToken` likewise needs an allocation-lifetime guarantee, not just an
output/BO index.

Current-master adapters must preserve `Drawable::dormant`, presentation-damage
epochs, content versions and redirect identity in the logical store. In
particular, removing/replacing an allocation must not reset `HiddenDamage` /
`NoPieces` as a side effect of a lease transition. Legitimate storage and
redirect changes must retain the backend's scene wake calls so `last_pieces`
and presence signatures are refreshed before walk-skipping relies on them.

Physical allocation context identity must include the selected Vulkan device
context, not just its DRM render node. Master now permits several physical
devices from one ICD to advertise that node; a node-keyed allocation/destructor
cache would conflate distinct contexts. This does not change the DRM device key
used to arbitrate KMS commits.

## 3. Recommended ownership direction

Use typed leases backed by a narrowly scoped resource owner. Keep the generic
`DeviceCommitOwner<R>` independent of `DrawableStore` and the Vulkan engine.
Perform backend-dependent cleanup at an explicit backend service boundary,
where mutable access to those subsystems is already available.

A direct framebuffer can use a shared strong reference to the existing RAII
allocation: the cache retains a reference, and each live lease retains one.
Removing the cache entry then cannot destroy an active import. This reference
alone does not authorize release of the source drawable or its Present wake.

For store/pool resources, the lease must retain a backing owner that can keep
the allocation alive until cleanup is serviced. Dropping a lease may record a
release obligation; it must not silently discard that obligation, signal a
release point, or free potentially current storage. The backing owner must
outlive outstanding obligations. A queue holding only numeric identifiers
while its store/pool can be destroyed is not an acceptable implementation.

Separate allocation lifetime from mutable logical state. For a drawable, the
store retains protocol identity, damage and current storage selection; its
storage allocation has a separately retained owner. For a scanout BO, the pool
retains acquisition/availability state while the lease retains its exact backing
allocation. A new storage generation gets a new allocation identity; it cannot
retarget an existing lease. A strong reference prevents destruction, while the
lease's usage state separately prevents reuse or writes inconsistent with the
existing explicit-synchronization contract.

Keep the dependency direction one-way: record/lease -> allocation -> required
device/allocation context. The context must not own records or the backend.
Normal destruction is serviced only after the resource-specific proof and GPU
dependencies are satisfied, with engine-view invalidation before image teardown.
The existing `DrawableStore::destroy_now` calls `Storage::destroy(platform)`;
it must stop unconditionally destroying leased allocations. This is a necessary
adapter change, not something a new wrapper around `DrawableId` fixes.

Cache removal, drawable destruction and pool replacement detach logical owners;
they cannot destroy backing allocations retained by a live or quarantined lease.
Stage 3 must transfer those retained owners and their device contexts into its
teardown supervision before destroying the ordinary platform/store containers.
An `Rc` reaching zero does not establish GPU idleness or the teardown barrier.
Keep unknown resources rooted until that barrier; do not implement quarantine
as a vector whose ordinary destruction bypasses it.

Before planning, enumerate the exact allocation fields/destructors moved into
each adapter and the existing cleanup call sites they replace. Avoid moving the
entire backend behind shared interior mutability solely to make a lease
destructor call it.

Alternatives: moving every allocation into records gives clear exclusive
ownership but is intrusive for pools and shared grouped sources; making the
whole backend reference-counted adds broad borrowing/destruction coupling.
The scoped lease approach is recommended because it confines that coupling to
the resources whose lifetime actually crosses commit boundaries.

## 4. Authoritative state and event consumption

There is one authoritative resource disposition for each lease:
desired, submitted, current, awaiting release, or quarantined. Record transitions
move leases by value. Shared allocations may have several leases, but each
reference and release obligation has exactly one owner.

`HardwareComplete` may establish the new hardware state before the record is
fully completed. Retain its resource ownership in the live record until
`CompletionRetired` hands it out; consumers may observe identity/state without
creating a second releasing owner. Damage consumers in 2c-iii need this earlier
hardware milestone, not ownership extraction.

At `CompletionRetired`, consume `Accepted<R>::into_parts()` exactly once:
install new current leases and transfer old leases into the release ledger.
The release ledger checks the applicable replacement and external ownership
dependencies. A current buffer's own presentation never proves it idle.

Proven rejection restores the old current set and routes the new set through
its source-specific cleanup. Unknown retains both sets in the owner's
quarantine; no normal release command is generated. Late events cannot move
those resources back into current or free storage.

Every event consumer keys work by device/incarnation and commit identity.
Outputs use stable device/CRTC and generation identities; an output-vector
index alone cannot survive topology replacement safely.

## 5. Present terminalization and grouped direct resources

The protocol ledger separately records completion disposition and release
disposition. Sending or suppressing a completion consumes that disposition
exactly once and unblocks the relevant FIFO. Release wakes, idle, source pins
and fallback pins remain pending until their own proof.

For an unsent successor displaced by newer full-plane work, emit idle and
release its never-submitted references immediately, retaining its Skip behind
the predecessor. Do not retain the framebuffer merely to remember notification
order. Accepted work uses the stronger release rule even when it terminalizes
as Skip after missing presentation evidence.

Preserve the baseline grouped-direct contract: one full-root frame targets a
homogeneous set on one DRM device, completes after all required output evidence,
and stamps Present with the selected/reference CRTC sample. Neither the last
arriving output nor the largest MSC replaces that reference. Its shared source
remains retained while any member can still scan it out; a partial replacement
cannot release the whole frame. Cross-device grouped direct is not introduced.

## 6. Physical capacity and protocol metadata passed to 2c-ii

Use the existing finite scanout pools and source-specific acquisition limits.
A BO retained by current/submitted/delayed-release ownership cannot be acquired
again merely because the atomic slot is free. Keep desired composed work as
scene/damage state when no reusable BO exists; do not allocate a spill pool or
queue rendered frames. Retiring entries refer to retained allocations and are
removed when their actual dependencies finish, without a second copy of the
underlying resource.

For direct scanout retain the existing current/pending/latest-successor model.
At supersession release the victim's never-submitted resource references and
keep only its protocol completion metadata. Do not introduce an arbitrary
64/256-entry import budget, notification credit window, or reader suspension
policy in this block. Retired imported sources may outlive an atomic slot:
the adapter design must map their existing source/release dependencies and
acquisition constraints, rather than falsely counting them as composed BOs.

Reserve bookkeeping needed to handle the chosen transaction's success,
rejection or unknown before IPC. This is local transaction preparation, not
a new per-client protocol quota. Reuse the existing release-safe composed
fallback/acquisition path for unflip; test it under exhausted normal acquisition.
Do not invent an extra admission tier or dedicated credit budget for it.

The baseline `deferred_successor_skips: Vec<_>` is not structurally bounded:
many displaced requests can accumulate behind one predecessor. Neither a
single successor slot nor a timeout proves a bound on notification count.
Preserve the existing notification ordering and liveness rules, retaining no
source/framebuffer references solely for a deferred Skip. Do not drop or merge
distinct required notifications to impose a cap.

The remaining metadata growth is an explicit inherited limitation. A future
hard protocol bound requires a separate Xorg-compatibility design covering
client progress, fence dependencies and request ordering. It is not a
prerequisite for 2c-i and this block must not claim to bound total Present
memory. Existing ingress byte credits are not completion credits and remain
unchanged.

## 7. Focused verification and remaining design work

Tests must observe actual reference/destruction and wake counts, not merely
integer membership. Cover cache eviction with live and quarantined framebuffers;
source destruction/client disappearance with pinned storage; rejection after
external release; hardware completion before Present; notification suppression;
duplicate/stale evidence; grouped partial replacement; source/fallback aliasing;
and pool exhaustion with a release-safe replacement path. Burst supersession
tests must prove that discarded framebuffer/pin lifetimes end immediately while
all required deferred completions survive in their original order.

Use synthetic resource backends for deterministic timing, but the implementation
also needs tests of the concrete lease adapters. A generic drop-count fixture
alone does not establish production lifetime safety.

The next design pass must resolve the concrete resource adapters before the
2c-i plan:

1. Which `Storage`/`ScanoutBo` allocation fields and destruction dependencies
   become retained owners, with an inventory of replacement and cleanup sites.
2. How existing pool and direct-source release dependencies feed readiness and
   retain allocations across teardown without blocking a release-safe unflip.

Once those are specified, review this block with the repository's frozen
review instrument. Approval of the three-block structure is not a claim that
these new ownership details have passed review.

## 8. Local reference comparison and accepted distinction

Compared the user-provided source trees on 2026-09-08: Xorg `5541a5c` at
`~/Projects/xserver` and wlroots `bd75ebfe` at `~/Projects/wlroots`.
The observations below concern those checked-out routes, not every backend.

| Source | Observed precedent |
| --- | --- |
| Xorg `present/present_vblank.c`, `present_vblank_init` and `present_vblank_scrap` | Hold a pixmap reference; release a scrapped pixmap while retaining completion bookkeeping. |
| Xorg `present/present_scmd.c`, `present_flip_notify` | Idle the prior pixmap, transfer the new reference into current screen state, then notify completion and try ready work. |
| wlroots `backend/drm/fb.c`, `drm_fb_lock` / `drm_fb_clear` | Retain/release the actual `wlr_buffer`, not just an FB integer. |
| wlroots `types/buffer/buffer.c`, `buffer_consider_destroy` | Outstanding locks prevent destruction after producer drop. |
| wlroots `backend/drm/drm.c`, `handle_page_flip` | Move queued framebuffer references to current on page flip and retire previous release state. |
| wlroots `render/swapchain.c`, `wlr_swapchain_acquire` | Use finite slots; no free slot returns failure rather than allocating unbounded overflow. |

These support retained allocation references and finite physical pools. They
do not establish equivalence for the previously proposed Present credit policy,
which is withdrawn. C.0's canonical out-fence evidence, independent milestones
and unknown quarantine remain intentional requirements of the governing spec;
the local reference comparison does not replace them with page-event retirement.
