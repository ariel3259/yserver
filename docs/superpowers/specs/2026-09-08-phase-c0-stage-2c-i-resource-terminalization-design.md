# Phase C.0 stage 2c-i — resource ownership and terminalization

**Status:** Revised after user approval of the Xorg/wlroots-aligned direction
on 2026-09-08: retain actual allocations, bound physical resources, and preserve
Present semantics without introducing protocol credits. The [round-3 review](../findings/2026-09-09-stage-2c-i-adversarial-review-round3.md)
reports 0 blocking, 0 major, 0 minor and COMPLETE FOR DECLARED SCOPE.
The design is ready for writing the 2c-i implementation plan; code and hardware
activation remain pending.

**Implementation plan:** [2c-i task plan](../plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md)
is drafted; its own adversarial review and task execution remain pending.

**Baseline:** Integration `67bf3491` includes upstream `a06cf0e0` (DRI3/overlay fixes),
on top of the prior v1.5.0 integration `14dd92d8` / upstream `99d02b16`. See the
[latest integration comparison](../findings/2026-09-09-stage-2c-v150-integration.md).
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
| DRM destruction context for that import | `DirectScanoutProbeFramebuffer::device: Rc<Device>` | Converted allocations retain an incarnation-bound cleanup context with registry-controlled DRM access, not an untracked strong fd owner. |
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

v1.5.0 adds allocation-layout metadata that the adapter must not lose:
`Drawable::content_offset` describes the layout pixels actually occupy, not
the current window border width. Capture that offset, allocation extent and
generation in any lease/intent that will interpret pixels later. Preserve
`PaintTarget`/`Src`/`Dst` bounds and offsets separately from the allocation's
raw handles, including the original logical X11 depth (`PaintTarget::x11_depth`),
which can differ from backing depth. Preserve the complete typed target or an
equivalent captured identity; never reconstruct depth from backing storage.
A storage reallocation creates a new identity; releasing the old
drawable must not remove the new `by_xid` mapping, matching master's guarded
`decref` path.

Border relayout may move pixels in place as well as allocate-and-copy. A lease
must therefore protect layout/use compatibility, not just object destruction:
if an outstanding KMS/read lease can still interpret the old layout, defer
in-place relayout or use a separately acquired allocation under existing pool
constraints. Changing an offset field or incrementing a generation alone does
not make overwriting the leased pixels safe. Preserve migration source storage
and GPU tickets until the copy dependency resolves; invalidate old unsent
eligibility and wake the scene for the new layout.

Root IncludeInferiors Composite now has a temporary source pixmap populated
from assembled current scanout. Its normal free operation may defer storage
destruction behind a GPU ticket. Keep that lifetime in the store/GPU ledger;
do not classify the scratch pixmap as a KMS-submitted direct generation. Any
scanout-read lease retains its actual current allocation through readback and
uses the same release/FOREIGN dependencies as other readers. In particular,
the completed-resource handoff cannot make a BO reusable while a snapshot read
still holds it. This adds no new direct import position or notification quota.

### Upstream a06cf0e0 integration — 2026-09-10

The physical imported owner must preserve the client's original dma-buf FD,
`DrawableImage::drm_modifier`, `import_plane0` and `import_size`, plus
`ImportedDmabufMetadata::implicit_layout`. An implicit DRI3 import is not an
explicit LINEAR allocation: the server's current Vulkan view is a guess, and
managed adoption must not promote that guess into a verified layout. Imported
re-export duplicates the original client FD with original plane description
and legacy stated size, without seeking or replacing it with a Vulkan re-export.
Keep INVALID reporting for implicit imports and explicit modifiers verbatim.
Implicit layouts remain rejected by the direct-scanout gate; preserve the
single-plane modifier advertisement restriction. This upstream fix repairs
Chrome's client round trip, not arbitrary server sampling of implicit layouts.

Overlay logical claims now belong exclusively to `ServerState::cow_claims`.
Backend get/release methods see only 0→1/1→0 claim edges. A retained physical COW
or fallback lease must neither resurrect a logical claim nor recreate the
removed `KmsCore::cow_refcount`. Preserve safe deferred unflip/release, re-claim
of the retained identity, request-failure claim retention, and disconnect's
sticky `cow_teardown_failed` policy. Test those transitions with actual resource
leases separately from the core's claim ledger.

These requirements are incorporated into the plan after design round 3 and the
first plan review; neither prior verdict certifies this new baseline. See the
[integration record](../findings/2026-09-10-stage-2c-master-dri3-overlay-integration.md).

## 3. Recommended ownership direction

Use typed leases backed by a narrowly scoped resource owner. Keep the generic
`DeviceCommitOwner<R>` independent of `DrawableStore` and the Vulkan engine.
Perform backend-dependent cleanup at an explicit backend service boundary,
where mutable access to those subsystems is already available.

A converted direct framebuffer uses shared strong references to an allocation
with the proof-gated cleanup contract below; the baseline destructor must be
adapted first. Cache indexing does not retain additional imports outside the
bounded role positions in §6. Removing an index cannot destroy an active
import. An allocation reference alone does not authorize release of the source
drawable or its Present wake.

For store/pool resources, the lease must retain a backing owner that can keep
the allocation alive until cleanup is serviced. Dropping a lease may record a
release obligation; it must not silently discard that obligation, signal a
release point, or free potentially current storage. The backing owner must
outlive outstanding obligations. A queue holding only numeric identifiers
while its store/pool can be destroyed is not an acceptable implementation.

Separate allocation lifetime from mutable logical state. For a drawable, the
store retains protocol identity, damage and current storage selection; its
storage allocation has a separately retained owner. For a scanout BO, the pool
retains acquisition policy while the resource service owns authoritative
availability state (§4); the lease retains its exact backing allocation. A new storage generation gets a new allocation identity; it cannot
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

### Allocation availability and completion routing (round-2 B-1)

The backend resource service owns one availability entry per incarnation and
allocation generation, shared by all aliases. Pool state and direct-resource
role occupancy are consumers of this entry, never independent release
authorities. Each entry retains the backing/context, live usage reservations,
KMS obligations, GPU tickets, read tickets, resource-specific FOREIGN state,
and completion subscriptions. Acquisition reserves the intended usage before
exposing handles; a conflicting reader, relayout or writer cannot slip between
the availability check and reservation on the serialized service path.

For a conflicting reuse or cleanup, eligibility is the conjunction of no
incompatible live reservation, every applicable KMS obligation discharged,
every applicable GPU/read ticket completed, and the required FOREIGN ownership
transition proven. Conditions are resource-specific: an allocation never used
by KMS has no KMS obligation; a copied-route renderer source does not acquire
the display sink's external transport. Its paired copy dependency still applies.
`PriorBufferReleased` satisfies only the matching KMS obligation. Neither
Present completion, reference drop, timeout nor fd closure fabricates a GPU,
read or FOREIGN proof. Unknown submission retains its reservation and closes
the affected transport until its specified recovery proof arrives.

The commit owner delivers generation-tagged KMS release evidence; the GPU
completion service delivers ticket completion and validated ownership-return
evidence; the read adapter completes its ticket only after the actual read
has finished. A lease drop relinquishes a usage reservation, not an unfinished
GPU ticket. Each producer posts to the same resource service inbox, identifying
incarnation, allocation generation and obligation/ticket. Duplicate evidence
is idempotent; stale evidence cannot release a newer allocation at the same
pool index or XID. Register the obligation and its completion route before
starting work; a failure to establish the route prevents dispatch.

Inbox consumption recomputes eligibility and publishes changes to the pool
acquisition path and direct-role/admission consumer. Those consumers may then
return a slot or retry eligible work; they cannot mark storage reusable merely
from page-flip handling. Register a waiter and recheck eligibility on the same
serialized path to avoid lost wakes. GPU/fence sources use the existing core
poll/completion service; ticket updates schedule a service wake, with deadlines
included in the normal wake calculation. No readiness loop blocks or spins.
The release-safe composed return reservation remains usable independently of
ordinary direct retirement, subject to its own resource dependencies.

During teardown, the supervisor receives these entries, retained contexts,
inbox, pending evidence, subscriptions and wake registrations in the same
move-by-value incarnation bundle as the commit records. Completion routing
switches atomically on the serialized path to that sole recipient; callbacks
must not capture the old backend/store/pool. The detached pool cannot acquire
again. The supervisor consumes late evidence for cleanup or quarantine and
does not publish availability into a new incarnation. If transfer cannot be
established, keep the existing retaining service alive and transport closed.
Full fd-family closure discharges only the file-owned obligations described
below; shared GPU/GBM allocations still need their independent proofs.

The current IncludeInferiors source read uses `read_scanout_region` and
`run_one_shot_op_with_wait`: successful readback returns owned CPU bytes after
GPU completion. Its source ticket can finish at that boundary; the subsequent
scratch upload and Composite retain the scratch allocation with separate GPU
tickets. They do not extend the source BO's read ticket. For a future deferred
read or a read whose submission outcome is uncertain, retain the source and
staging/context dependencies until completion or the applicable recovery
proof. This contract does not assert a demonstrated asynchronous snapshot race
in today's successful synchronous path.

### Teardown handoff contract (round-1 B-1)

The receiving owner is the process-lifetime teardown supervisor required by
C.0 §10, separate from the replaceable render backend. Before any ordinary
store/platform/engine destruction, close admission and alias creation, detach
normal event consumers, and move one incarnation-keyed ownership bundle into
that supervisor. The bundle contains the live/quarantined records, current and
delayed-release leases, allocation/device contexts, pinned release obligations,
GPU/FOREIGN dependencies, and pending cleanup work. Transfer the registered fd
family and outstanding helper leases with the same incident. An outstanding
reply must route to the retained incident, never the destroyed backend.

The move has no intermediate unowned state: until the supervisor accepts the
bundle, the backend retains it and its required contexts. Once accepted, the
backend holds no independently releasing copy. Cleanup obligations must be
executable from retained contexts after handoff; a callback requiring a mutable
reference to the old backend is forbidden. Normal cache/view cleanup either
finishes before handoff when safe or moves its required ownership into the
bundle as well.

The supervisor does not release a resource merely because a lease drops.
It consumes the resource-appropriate proof from §10: actual helper reap before
fd-family retirement, then the complete fd-family barrier for file-owned
quarantine, plus the separate GPU/FOREIGN/shared-allocation requirements before
destroying those resources. No expired watchdog substitutes for these proofs.
Registered DRM aliases retained by allocation contexts participate in that
barrier; they must not become hidden references that prevent or falsely certify
closure. The adapter plan must define their post-close/device-loss-safe cleanup
without issuing stale-handle ioctls. If proof never arrives, the supervisor
retains the bundle through the specified shutdown deadline and process-exit
policy; ordinary container destruction is not a fallback release mechanism.

The KMS obligation is one of these resource-appropriate proofs, not a byproduct
of the others. An entry whose file-owned, GPU/read and FOREIGN dispositions are
all satisfied — including the vacuous case of an entry with no exclusively
file-owned handles left — still holds its `KmsRelease` obligation until either
correlated `PriorBufferReleased` evidence for that exact commit/CRTC generation
arrives, or a device-scoped barrier supersedes it: complete `IncarnationFdSet`
closure for the producing device after every submitter stopped and the
incarnation detached from event dispatch, or proven device removal/loss. That
barrier supersedes the KMS obligation only, per §10's rule that fd-family
closure is a barrier solely for exclusively file-owned resources; a barrier for
one device never supersedes another device's obligation. Where no such proof
arrives — an unreaped helper holding a device alias, `ExecutorStalled` or
`ShutdownExecutorStalled` — the entries stay quarantined and rooted under the
shutdown deadline and process-exit policy. Ending quarantine on the remaining
dispositions alone is forbidden, and so is treating the unresolved obligation
as an unavoidable leak.

Stage 2c-i defines and tests this ownership transfer with a retaining supervisor
fixture, including late replies and unavailable proofs. Stage 3 implements the
real supervision/barrier process. Converted production traffic remains disabled
until that receiver exists. Round 3 accepted this ownership/handoff contract
within the declared design scope.

#### Allocation cleanup and fd closure (2026-09-09 clarification)

The baseline `DirectScanoutProbeFramebuffer::Drop` unconditionally calls
`destroy_framebuffer` and `close_buffer` through `Rc<Device>`. That destructor
cannot be reused unchanged for converted quarantined resources. The converted
allocation separates immutable FB/GEM identity from an incarnation-bound
cleanup context. Registered fd entries, rather than allocation references,
own the closable DRM access. The context exposes an explicit open/closing/closed
state and never recovers access by opening a new device with the same path.

On proven ordinary release, a typed cleanup operation consumes FB/GEM cleanup
rights once against the still-open original incarnation. On quarantine, freeze
those normal cleanup rights. The supervisor retains the allocation but can
retire the registered fd family after helper reap without waiting for each
allocation's reference count to reach zero. Only actual complete-family closure
discharges exclusively file-owned handles; it marks their cleanup state closed
so a later allocation destructor cannot run RMFB/GEM-close on stale/reused ids.
Shared dma-buf, GBM and Vulkan allocations and their contexts remain retained
until their independent release/device-loss-safe teardown rules are satisfied.
Closing an IPC fd or one alias is not complete-family closure.

Create the supervisor's retaining incident slot before enabling converted
traffic for an incarnation. Handoff moves existing ownership into that slot;
it must not depend on allocating a new recipient after shutdown has begun.
If the transfer cannot complete, keep the old owner/contexts alive and the
transport closed. The supervisor's identity-indexed ingress also receives late
executor replies and returned descriptors after ordinary backend detachment.
Failure or deadline exit retains the recorded uncertain ownership; it does
not run ordinary cleanup as stack unwinding of an unproven bundle.

The concrete adapter implementation must enforce these operations as consuming
state transitions. A raw `Rc<Device>` left inside an imported framebuffer,
hidden cleanup callback, or unconditional destructor ioctl would violate this
contract even if the surrounding record is called a lease.

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

Use the existing finite scanout pools for composed allocations. Direct imports
use the explicit role-bound capacity below; the old probe cache is not a bound.
A BO retained by current/submitted/delayed-release ownership cannot be acquired
again merely because the atomic slot is free. Keep desired composed work as
scene/damage state when no reusable BO exists; do not allocate a spill pool or
queue rendered frames. Retiring entries refer to retained allocations and are
removed when their actual dependencies finish, without a second copy of the
underlying resource.

### Direct physical-resource invariant (round-1 B-2, 2026-09-09)

For each direct primary ownership unit, allocate six frame-resource positions
whose roles are fixed below. A position owns one source allocation generation,
its framebuffer/import, source and fallback leases, pinned release state and
cleanup obligations. Several positions may reference the same allocation, but
that does not allow extra positions or independent untracked release duties.
The currently supported full-root grouped path has one unit per DRM device,
covering its exact homogeneous output set. It does not multiply the allowance
by CRTC for a shared source. Future partial/disjoint ownership needs its own
bounded representation before admission, as required by C.0 §9.1.

| Role | Capacity | Lifetime |
| --- | --- | --- |
| Current | 1 | Source that may still be scanned out. |
| Submitted | 1 | New direct state in the sole device atomic transaction. |
| Successor | 1 | Latest eligible never-submitted frame. |
| Preparing | 1 | Candidate import/validation before installing or replacing the successor. |
| Ordinary retirement | 1 | Replaced direct state awaiting resource-specific release. |
| Exit retirement | 1 | Reserved exclusively for the current direct state displaced by composed unflip/ownership exit. |

This is a count bound on server-retained direct frame-resource generations,
not on client-created pixmaps, all Present memory, or allocation bytes. The
two retirement positions bound delayed resource obligations even after the
atomic slot becomes free. The preparing position accounts for the transient
old-successor/new-candidate overlap rather than assuming instantaneous import.

Reserve `Preparing` before creating a framebuffer/GEM import or retaining
candidate resource leases. At most one such operation is outstanding per unit.
An unsuccessful candidate follows proven never-submitted cleanup and leaves
the existing successor unchanged. A successful replacement idles the old
successor once, releases its never-submitted resource references and transfers
the candidate to `Successor`; only completion metadata remains deferred.
No second candidate can enter until the preparing position is free. If cleanup
cannot prove release, retain that position and close the transport into the
existing failure/quarantine path instead of opening an overflow queue.

The converted direct probe cache keeps no additional strong framebuffer/import
references outside these positions. Reuse may index occupied positions by exact
allocation and topology generation; remove the index when its position retires.
Pure eligibility metadata must follow existing drawable/topology lifetimes and
cannot authorize a new live submission without current validation. The legacy
cache may remain on the legacy-only route, but cannot coexist as an uncounted
resource owner after handover.

A normal direct replacement reserves the empty ordinary-retirement position
for its old current state before dispatch. If that position is still occupied,
the successor remains latest-wins but is not resource-ready for dispatch.
The atomic slot stays free for other eligible work. Release evidence wakes
admission; capacity pressure alone must not schedule an immediate retry loop.
Rejection returns old current and releases the reservation; unknown retains
all occupied positions with the record and closes transport. On completion,
old current moves into the reserved retirement position and submitted becomes
current. If release proof is already complete, service it in the same wake so
ordinary retirement creates no artificial frame delay.

An unflip cancels never-submitted direct work using the existing ordered Skip
rules, waits for any submitted atomic work to resolve, and reserves the distinct
exit-retirement position for the remaining current direct state. It does not
need the ordinary-retirement position to be free. Its composed replacement
uses the already retained composed framebuffer on each affected output:
preserve those allocation leases throughout direct ownership and exclude them
from normal pool reuse, as the baseline `retire_direct_output` does. Required
shadow materialization/source waits and output evidence remain mandatory; the
reservation never licenses displaying stale contents or ignoring a dependency.

Before entering direct mode, establish this composed-return resource path for
every included output. If it cannot be established, remain composed. A failed
unflip does not release the current direct source. If the ordinary slot holds
A and the exit slot later holds B, composed operation can continue while both
await release; direct re-entry waits until both retirement positions are free.
This prevents successive direct-entry/exit cycles from accumulating imports.
Quarantined positions remain charged to the old incarnation, and no fresh
incarnation bypasses the stage-3 resource/fd barrier.

These reservations are physical ownership bookkeeping, not new Present credit
limits or admission priority tiers. Source/core wait queues retain their own
pre-existing semantics; work not admitted to direct must not secretly import
resources outside the positions. The 2c-ii readiness predicate consumes these
reservations while retaining all seven normative ordering tiers.

Reserve bookkeeping needed to handle the chosen transaction's success,
rejection or unknown before IPC. This is local transaction preparation, not
a new per-client protocol quota. Reuse the existing release-safe composed
fallback/acquisition path for unflip; test it under exhausted normal acquisition.
The role reservations above must preserve that existing ordering and dependency
contract; they do not add a protocol credit budget.

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

### Blocking-finding regression matrix

| Sequence | Required observation |
| --- | --- |
| Unknown -> detach backend -> late reply -> delayed helper reap | Supervisor remains the sole owner; descriptors route to the same incident; no cleanup requires the old backend. |
| Allocation references remain after complete fd-family retirement | File-owned cleanup is already discharged; final reference drop issues no DRM ioctl. Shared allocation cleanup still waits for its own proof. |
| Shutdown recipient unavailable or helper never reaps | Transport stays closed and the retaining owner survives; no drop-count or wake implies a release proof. |
| A replaced by B, A release delayed, newest successors C/D/E arrive | Only B, A, the latest successor and at most one preparing candidate remain; no second ordinary replacement dispatches while A occupies retirement. All victim Skips remain ordered. |
| Unflip while A is in ordinary retirement and B is current | Existing composed-return resources can replace B using exit retirement; both old direct sources remain retained until independently releasable. |
| Repeated direct entry/exit with both retirements unresolved | Direct entry stays resource-ineligible; no seventh position or import outside the role table appears. |
| Probe/replacement cleanup fails or an atomic outcome is unknown | Occupied positions remain retained, transport closes, and later arrivals cannot grow a new import queue. |
| Border relayout while old-layout KMS/read lease exists | In-place relocation is excluded; defer or allocate separately within capacity. Old offset/extent remain valid until release, and new scene eligibility is refreshed. |
| Depth-24 target backed by depth-32 redirected storage | Deferred paint/read retains original depth, offset and typed bounds; no backing-depth reconstruction. |
| Old storage generation released after the XID selects new storage | Old storage is cleaned once; the new XID mapping, contents and damage state survive. |
| KMS release before read/GPU completion; then reversed order | No conflicting reuse until all applicable proofs and FOREIGN return hold; final proof wakes acquisition once. Exercise real adapter routing as well as synthetic timing. |
| Successful synchronous root snapshot followed by scratch Composite | Source read ticket ends after CPU bytes are owned; scratch survives its own GPU work and is freed once, without retaining the source solely for scratch usage. |
| Handoff with pending/uncertain read; late, duplicate and stale completion | Supervisor retains source, staging and contexts, receives completion without backend callbacks, and cannot release a new generation. |
| Completion arrives during waiter registration or handoff | Recheck/inbox transfer prevents a lost wake; only the current recipient services evidence. |
| Normal stream with immediately satisfied release dependencies | Retirement clears in the same wake and ImmediateOnRetirement introduces no additional delay. |

Instrument position high-water marks and actual allocation/destruction counts
in the tests. Include repeated pixmap identities with new storage generations,
multi-output shared sources, rejection and reordered fence/page evidence. The
finite-state count is derived from the role table, not from elapsed time or a
notification quota. These are required implementation tests, not tests already
run against the currently unimplemented adapters.

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

The [concrete adapter inventory](2026-09-09-phase-c0-stage-2c-i-resource-adapter-inventory.md)
now maps retained fields, destruction/replacement sites, completion producers,
acquisition consumers and teardown boundaries against `14dd92d8`. Round 3
assessed this inventory within its declared scope. It also requires completion-only progress with scene
submission suspended, and old-generation lease tests for promotion/pool return.
Round 3 accepted the inventory, §4 availability contract and the main design's
previously uncovered M-2 gate as a basis for writing the executable plan.
Concrete implementation and its validation remain outstanding.

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
