# Phase C.0 stage 2c — conversions and damage

**Status:** Three-block elaboration approved by the user on 2026-09-08.
The user also approved the reference-aligned direction: allocation leases and
physical resource limits, without new Present protocol credits. The detailed
adapter contracts remain design proposals, not an implementation plan
or an activation/merge authorization. No adversarial review has run.

**Baseline inspected:** Feature integration `d4c30877` combined with upstream
master `99d02b16` (including v1.5.0 at `e2d17ec5`), on
`feat/phase-c0-atomic-kms-migration`. The latest integration review is in
[the v1.5.0 comparison](../findings/2026-09-09-stage-2c-v150-integration.md);
the [previous comparison](../findings/2026-09-08-stage-2c-master-integration.md)
records the earlier `f6c79967` baseline.
Historical 2b-ii validation and fresh integration validation are recorded
separately in `docs/status.md`.

**Authority:** [C.0 revision-2 specification](2026-08-26-phase-c0-atomic-kms-migration-design.md),
especially §§9–10.4, 12–12.1 and 18. This document elaborates its stage boundary;
it does not replace or relax the governing contracts.

## 1. Outcome and observed baseline

Stage 2c connects the device owner to actual primary resources and producers.
It adds bounded admission, independent Present/release terminalization,
composed/direct primary conversion, and damage transactions driven by owner
milestones. Operational C.0 readiness remains closed until the remaining
lifecycle, cursor/gamma and qualification requirements are implemented.

The baseline already supplies `DeviceCommitOwner<R>`, a device-wide slot,
correlated executor outcomes, completion evidence, clocks and deadlines.
`PlatformBackend` still instantiates `DeviceCommitOwner<NeverResource>`.
`OwnerEvent::CompletionRetired` transfers `Accepted<R>` by value: this is a
resource handoff, not proof that every old resource can be destroyed.

Concrete integration sites inspected:

| Existing site | Required change |
| --- | --- |
| `kms/render/platform.rs`: device owner and owner event service | Carry real ownership; dispatch events to the resource/protocol/damage consumers by device and commit identity. |
| `kms/render/backend.rs`: `submit_direct_frame` | Replace synchronous `submit_direct_scanout` on the converted path with owner admission and asynchronous dispatch. |
| `kms/render/backend.rs`: `submit_queued_direct_successor` | Promote through the shared admission function, preserving immediate dispatch policy. |
| `kms/render/backend.rs`: `defer_direct_successor_skip` | Preserve immediate idle and predecessor-before-Skip publication with independent terminalization records. |
| `kms/render/backend.rs`: `submit_composed_unflip` | Represent primary restoration as an ordered barrier; distinguish it from lifecycle conversions assigned to stage 3. |
| `kms/render/scene.rs`: `handle_page_flip_complete` | Separate damage/BO hardware progress from protocol presentation and delayed resource reuse. |
| `kms/render/platform.rs`: `issue_legacy_drained`, plus backend disposition handling | Preserve the checked handover and exact-once disposition of all drained events. |

## 2. Approach and review boundaries

Elaborate three dependent blocks, each with explicit input/output contracts
and its own implementation plan after its design is reviewed:

1. **2c-i — Resource ownership and terminalization.** Real resource guards, current /
   submitted / delayed-retirement ownership, and independent protocol ledgers.
2. **2c-ii — Bounded intents and admission.** Storage bounds, source readiness, seven
   tiers, maintenance tickets, primary fairness and retirement-time dispatch.
3. **2c-iii — Primary conversion and damage.** Composed, direct and composed-unflip
   producers, shared event consumption, homogeneous bundles and regression tests.

These are approved elaboration boundaries inside 2c, not new merge units.
Develop 2c-i first; freeze its ownership interfaces before writing the dependent
2c-ii plan, and freeze admission before writing the 2c-iii plan. Each block
gets a focused design and implementation plan, using the governing C.0 spec
and this document as shared context. Do not duplicate the full C.0 spec in each.

| Block | Inputs | Deliverable consumed by the next block | Exit evidence |
| --- | --- | --- | --- |
| 2c-i | 2b-ii owner outcomes; existing BO, framebuffer, pin and wake ownership | Real resource leases; current/submitted/retiring dispositions; exact-once protocol ledger; physical acquisition readiness | Resource lifetime and terminalization tests across rejection, completion, unknown and grouped outputs |
| 2c-ii | 2c-i resource reservations and terminalization; ready source descriptions | Bounded intents; one deterministic admission decision including the exact absorbed generations; fairness bookkeeping | Seven-tier tests, supersession bounds, no dispatch before source readiness, retirement promotion ordering |
| 2c-iii | Owner evidence; 2c-i leases; 2c-ii admission | Converted composed/direct/unflip paths and commit-bound damage transactions behind an exclusive transport boundary | End-to-end producer/owner tests, damage milestone matrix, grouped retirement and legacy exclusion checks |

2c-i does not dispatch production primary work. 2c-ii does not add a second
resource ledger or consume damage. 2c-iii does not choose its own admission
order or release resources through the old page-event shortcut.

Two alternatives were considered. A single monolithic plan makes the contracts
visible together but repeats the review-size problem documented in §18.
Converting direct submission first yields an earlier end-to-end path, but its
resource release and successor promotion would depend on admission and
terminalization rules not yet established. The recommended order makes those
dependencies explicit before changing producers.

## 3. Resource and protocol contract

Replace the production uninhabited parameter with a type owning actual guards
or leases. Integer framebuffer, BO, pin or descriptor identifiers alone are
not ownership. Each guard must keep its backing allocation and destruction
context alive; moving a record or removing a producer cache entry cannot free
storage that the record may still expose to KMS.

The implementation plan must map each existing composed BO, direct framebuffer,
source/fallback pin, descriptor allocation and external-ownership obligation
to exactly one authoritative owner through these transitions:

| Boundary | Disposition |
| --- | --- |
| Waiting for producer | Intent owns resources and the asynchronous source wait; no atomic slot reservation. |
| Ready and dispatched | Record owns both possible old/new states before IPC; cancellation cannot use never-submitted cleanup. |
| Proven refusal/rejection | Return still-current resources separately; clean up new resources using their actual Vulkan/FOREIGN state, including `ReleasedButAtomicRejected`. |
| Accepted | Retain possible old/new ownership; stage composed damage exactly once. |
| HardwareComplete | Apply composed damage and establish hardware state; do not infer Present or unconditional BO reuse. |
| CompletionRetired | Move resources to current-state and bounded delayed-retirement ownership; release the device slot under the existing completion contract. |
| PriorBufferReleased | Finish resource-specific replacement and FOREIGN return obligations; only then idle/release accepted buffers. |
| Unknown | Retain both states in quarantine and invalidate damage; protocol terminalization does not free resources. |

Track protocol completion, idle/release and quarantine separately by Present
serial/commit identity. A never-submitted displaced successor idles and releases
pins/wakes immediately; retain only its deferred Skip notification behind the
predecessor. An accepted Present lacking validated presentation terminalizes as
Skip using the last validated clock sample, never a fabricated Flip timestamp.
Client/drawable disappearance suppresses delivery as appropriate but still
unparks the FIFO. Teardown cannot terminalize the same predecessor twice.

## 4. Admission contract

Retain one desired composed scene/damage state, one latest direct successor and
one non-supersedable unflip/recovery barrier per primary ownership unit. Do not
add a rendered-frame FIFO or a second async successor slot. Keep the core's
existing Present equivalence rule unchanged.

Use exactly the §9.2.1 tiers: topology barrier; visible-desktop restoration;
fairness-qualified direct successor; oldest aged maintenance; compatible
homogeneous primary bundle; round-robin primary; oldest non-aged maintenance.
Tickets survive latest-wins payload replacement. Maintenance ages after losing
an admission or arriving behind submitted work. A continuously ready CRTC may
not take consecutive slots while another ready CRTC is owed service.

All producers and retirement promotion enter the same admission function.
Enqueue the predecessor's completion and deferred Skips first, then admit and
dispatch in that wake, then let core publish protocol events in order.
`ImmediateOnRetirement` introduces no retention timer or bundle wait.

The scheduler must support the full maintenance/bundle contract even though
stage 4 supplies production cursor/gamma producers. Tests must exercise aged
incompatible maintenance, symmetric absorption and unchanged-cursor omission;
an empty maintenance queue is not evidence that fairness is implemented.
No live maintenance payload may be synthesized from stale legacy state.

## 5. Damage contract

Identify the transaction by commit and the exact included output/BO generation.
Capture the submitted snapshots without consuming damage arriving afterwards.
Retain transaction metadata before dispatch, but do not stage damage until
`Accepted`.

| Owner outcome | Damage action |
| --- | --- |
| Submitting or FailedBeforeSubmit | None. |
| Accepted | Stage each painted buffer once. |
| HardwareComplete | Apply every staged buffer in that transaction once. |
| Presented | None. |
| CompletionUnknown or incarnation/topology/VT/device invalidation | Invalidate. |
| Post-accept failure with proven prior state still current | Restore. |

A homogeneous bundle is one transaction: stage and apply the same included
outputs, with apply following the complete canonical fence set. Never retire an
unrepresented output. Direct commits do not apply composed damage; direct entry
and return through composed unflip invalidate affected composed buffers.
Keep descriptor/GPU and FOREIGN release dependencies separate from damage ack.

### Current-master scene contracts carried into 2c-iii

The `8d448583` damage follow-ups are part of the implementation baseline.
Converting the retirement trigger must preserve their snapshot selection and
wake behavior; copying the old pre-follow-up handler is insufficient.

- An invalidated/failed transaction owes a repaint independently of fresh
  producer damage. Feed `ScanoutDamage::owes_repaint()` through the scene and
  backend wake predicates. While a device is quarantined this desired work
  remains retained, but cannot dispatch or create an immediate retry spin.
- Preserve `DormantReason::NoPieces` versus `HiddenDamage`: paint re-arms the
  latter, while a structural change is needed to expose the former. These are
  logical visibility decisions, not buffer lifetime or completion milestones.
- Keep `walk_needed`, per-output `pending_presentation_for_output`, and retained
  `last_pieces`. A device-owner wake does not by itself require walking every
  output. Reconcile dormancy with `dormancy_inputs`, including retained pieces
  from outputs that skipped this tick; a pending output is not evidence that
  its drawable is invisible.
- Carry the scene builder's exact per-output `drawable_snapshots` into the
  transaction. Non-empty `Hidden`, `OtherOutput` and `OffOutput` snapshots are
  excluded. `OffOutput` may force a compose but does not authorize an ack.
  Do not reconstruct a global drawable ack set from a bundle's CRTC set.
- Preserve damage epoch checks and per-output snapshot attribution at
  `HardwareComplete`. Capture participating outputs before acknowledgment; do
  not ack one output and then discover another output's damage from the
  already-mutated store. Separately scheduled spanning damage remains an
  explicit validation case, not a claimed fix inherited from master.
- Storage/redirect/direct/topology changes must invalidate the retained walk
  assumptions and wake the scene through existing structural notification
  paths. Preserve exact shape rectangles, presence signatures and the restack
  comparison against rank-unchanged participants in `scene_diff`.

The implementation tests must combine these rules with the owner milestone
matrix: invalidation without fresh paint, skipped-output dormancy, off-output
damage not acked, new paint between capture and hardware completion, and
two-output completion permutations with both bundled and separate scheduling.

### v1.5.0 implementation requirements by block

The border and root-Picture fixes are implemented baseline behavior to retain,
not additional features to reimplement. Carry these requirements into each
block's implementation plan and review scope:

| Block | Required adaptation | Concrete integration and regression coverage |
| --- | --- | --- |
| 2c-i | Keep physical layout identity with the leased allocation generation, including content offset, extent and the resolved source domain. Preserve old storage while migration/render/snapshot consumers still need it. | `store.rs`, `target.rs`, backend storage relayout and decref paths: old pinned storage plus a new drawable under the same XID; border change with live lease; scratch snapshot freed after GPU use. |
| 2c-ii | Preserve border eligibility and resource readiness in every direct admission path, including retirement-promoted successors. A geometry/layout change invalidates an earlier decision. | `scanout_direct_eligible`, `try_present_direct` and the new scheduler: bordered ancestor rejection, changed border while successor queued, ordered unflip, no bypass from tier 3. |
| 2c-iii | Preserve `PaintTarget`/`Src`/`Dst` coordinate and clip semantics in composed and unflip paths; keep root IncludeInferiors source snapshots and their GPU lifetime separate from KMS damage acknowledgment. | `backend.rs`, `engine.rs`, `scene.rs`, `frame_builder.rs`, `target.rs`: root-picture captures under composed/direct/unflip transitions; content versus border clipping; snapshot before Composite with aliased source/destination; zero-size and error cleanup. |

The resolved paint chain's `has_border_clip()` is the current direct gate, not
just the leaf window's border width. Keep that conservative exclusion; this
stage does not qualify cropped direct scanout of bordered storage. A stale
eligible snapshot cannot survive a border/layout change without revalidation.

Client-facing drawing uses `PaintTarget::dst()` / `src()` with their content
bounds; server backing access remains explicit and narrowly scoped. Do not
strip those bounds when moving resource handles into an owner intent. Reading
semantics remain request-specific: window GetImage can include the border,
and a named window pixmap exposes its bordered backing intentionally.

The new root IncludeInferiors source Picture path captures before Composite,
using assembled OnScreenOnly scanout, then frees its temporary pixmap through
the ordinary fence-aware store path at one cleanup site. It is not a canonical
scene image. The owner conversion must provide the corresponding safely
readable scanout identity without reading pending/unknown storage, acquiring
an unresolved FOREIGN buffer, acknowledging scene damage, or synthesizing
Present evidence. Retain a read lease through the readback operation and GPU
consumers; record any extra release dependency in the existing physical role
or BO ledger rather than an uncounted snapshot-owned direct import.

Preserve current source transform/repeat/component-alpha behavior when
substituting the snapshot. No-output/readback-unavailable fallback and
zero-area Composite behavior remain as implemented; lifecycle invalidation
must not leave a cached read pointer into retired output storage.

`2026-09-01-canonical-scene-copy-design.md` and its plan were restored as
unimplemented design history. They neither authorize replacing this snapshot
path nor introduce a canonical image into the 2c resource model. The documented
general resize/gravity preservation gap also remains outside this integration.
In contrast, preserve the landed border-width migration and SHAPE distinction
between an unset clip region and an explicitly empty region.

## 6. Activation and later-stage interfaces

Stage 2c must not activate owner primary traffic on a device whose legacy
lifecycle or maintenance writers can still issue conflicting KMS calls.
`LegacyDrained` proves event-drain completion; it is not by itself proof that
all future legacy writers are excluded. The existing terminal handover-failure
latch and valid-prefix event dispositions remain mandatory.

Round-1 M-2 is addressed by defining a single per-device, incarnation-bound
transport state in `PlatformBackend`: `Legacy`, `Quiescing`, `Owner`, or
`Closed`. These are transport permissions, not substitutes for the owner's
lifecycle/qualification states. Every mutating transport entry checks this
authority before dispatch; checking only the primary producer is insufficient.
The gated classes include composed/direct primary and unflip, modeset/routing,
DPMS, VT, topology/reprobe installations, cursor load/move/show/hide/detach,
gamma, and helper paths that mutate device state. The future qualified
coordinate exception is owner-authorized and cannot bypass the transport gate.

`Legacy -> Quiescing` revokes new legacy submissions before draining already
submitted work. New owner submissions remain blocked. After legacy work has
resolved, all drained events have final dispositions, and the checked
`LegacyDrained` proof is consumed, the platform may publish `Owner` only when
all writer classes are either owner-mediated or disabled and the teardown
receiver is installed. Publish this transition on the core's serialized path;
any helper permission issued earlier must be resolved/revoked before it.
Handover failure or unknown closes the transport; there is no return to legacy
within that incarnation. A later fresh-incarnation route is selected only by
the stage-3 lifecycle boundary after old-resource/fd barriers.

Until stages 3/4 supply the remaining production writers, the production route
stays `Legacy`; fixtures may establish `Owner` only with explicit disabled/mock
writer coverage. Test each gated class during quiescing, after handover and
after unknown, including a legacy attempt made after event-drain success.
The call-site inventory remains implementation work, but none may retain an
unguarded mutating entry on an owner-active device. This contract correction
has not received a second adversarial review.

The recommended intermediate state keeps operational readiness closed and
preserves the Phase A+B production route while converted paths are exercised
through deterministic integration tests. The implementation plan must identify
the concrete selection boundary and enumerate every writer it excludes before
any live activation. A fallback to legacy after an unknown owner commit is
forbidden: unknown retains the slot and resources.

Stage 3 owns lifecycle installation, DPMS/VT/topology, recovery and quarantine
release. Stage 4 owns cursor/gamma producer conversion and coordinate transport.
Stage 2c provides their admission, cancellation and terminalization interfaces;
it neither implements a competing recovery loop nor certifies their hardware
behavior. Composed unflip needed for primary restoration belongs here; the
remaining lifecycle modeset/unflip callers stay in stage 3 per §18.

## 7. Verification and questions to resolve in planning

Required deterministic scenarios include source-wait failure without dispatch;
pre-IPC refusal; rejection after external ownership release; page/fence/reply
reordering; hardware completion without Present; missing Present after hardware
completion; exact-once idle versus deferred Skip; client disappearance;
multi-output shared-buffer retirement; unknown retaining both states; damage
arriving after capture; bundle damage identity; and fairness under a continuous
direct successor stream. Retest the existing legacy handover failure matrix.

Implementation retains the repository gates: `cargo +nightly fmt`,
`cargo clippy --all-targets -- -D warnings`, relevant tests and Linux glibc,
Linux musl and FreeBSD checks. Hardware qualification and final C.0 readiness
are not established by deterministic tests.

Before this proposal becomes an executable plan, resolve these source-level
design questions explicitly:

- Which concrete owners can supply transferable guards without borrow cycles
  between platform, scene, drawable store and render engine?
- How do existing pool/source constraints retain delayed releases after the
  atomic slot becomes free? The 2026-09-09 B-2 correction in the 2c-i design
  now specifies six direct-resource roles per ownership unit, including
  ordinary and exit retirement, with normal admission gated on release
  capacity. Composed buffers retain existing pool constraints. Deferred
  protocol metadata keeps the baseline's
  ordering and growth behavior; a new hard protocol bound is separate
  compatibility work, not a 2c-i requirement.
- Where is the exclusive production-route selection, and how will stages 3/4
  complete it without a legacy writer racing an owner commit?
- How do current grouped direct frames map to per-device admission and
  per-output evidence without releasing a shared source at the first output?

These are remaining design work, not implementation discretion. Review any
resulting spec/plan through `docs/superpowers/review/review.sh` with declared
scope and its printed provenance; no clean verdict is claimed here.
