# Phase C0 stage 2c-i — concrete resource adapter inventory

Status: reviewed in [round 3](../findings/2026-09-09-stage-2c-i-adversarial-review-round3.md),
with 0 blocking, 0 major, 0 minor and COMPLETE FOR DECLARED SCOPE. Ready as input
to the executable implementation plan; implementation remains pending. Baseline: `14dd92d8`, incorporating upstream
`99d02b16` (v1.5.0). This is a normative companion to
[resource terminalization](2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md).
Names below identify existing code; the proposed retained owners/service are
not already implemented. Paths are relative to `crates/yserver/src/`.

## 1. Retained fields and cleanup boundaries

| Existing allocation family | Retained allocation boundary | Logical state and replacement/cleanup entry points |
| --- | --- | --- |
| `kms/render/store.rs::Storage` | Image, memory, attachment and sample views, extent/format/storage depth, tracked Vulkan layout, imported `DrawableImage` and dma-buf metadata, exportable flag/stride/size/modifier, exact Vulkan context and cleanup/pool-return rights. Imported alias handles must have exactly one owner. | `DrawableStore` keeps XID, damage/dormancy/content version and current allocation selection. Capture `content_offset`, extent, original paint depth and generation for deferred interpretation. `decref`, `destroy_now`, `poll_pending_retire`, `shutdown_destroy_all` detach or submit cleanup obligations; they cannot directly destroy or pool-return a leased allocation. Preserve invalidation before image cleanup and the guarded `by_xid` removal. |
| `Storage::adopt_exportable` / `RetiredImage` | The displaced image/memory/views plus its exact context and all usage tickets become an old-generation retained allocation. | `kms/render/engine.rs::retire_image_after`, `destroy_retired_image` and `retired_promoted_images` must consume service-authorized cleanup. A ready engine fence alone cannot override an old KMS/read lease. Promotion publishes a distinct generation; old aliases cannot retarget. |
| `drm/modeset.rs::DirectScanoutProbeFramebuffer` | FB and GEM handles with consuming cleanup rights bound to the DRM incarnation registry. Replace the destructor's untracked `Rc<Device>` access for converted allocations. | Backend probe-cache eviction drops an index. The six resource roles retain accepted imports; rejected/preparing cleanup remains charged until resolved. `Drop` must not issue RMFB/GEM close after registry closure or while retained. |
| `kms/vk/scanout.rs::ScanoutBo` | Image/memory/view, FB/GEM, transfer command pool/buffer, staging buffer/memory/mapping, timestamp pool, GBM BO and required GBM-device lifetime, Vulkan context, registry-bound DRM cleanup rights. Retain live fence descriptors as obligations. | `BoState`, pool selection, content age and telemetry remain logical/service state. `ScanoutBo::drop`, pool replacement/drain and final disarm cannot bypass the resource service. Transfer resources cannot be reset while their GPU/read tickets remain live. Preserve view/image/memory and GBM teardown ordering. |
| `CopiedRenderSource` and `CopiedScanoutPool` | Retain renderer optimal target, exported transport, sink import, transfer resources, completion/wait semaphores, retained return sync file, both exact Vulkan contexts, and destination allocation leases. | Preserve `CopiedSourceOwnership`, `CopiedDestinationOwnership`, export-semaphore reuse state and target-content state as authoritative dependency substates of the service entries. `release_completed_source`, `note_kms_retired`, copy-failure recovery and both Drop paths must provide/consume the appropriate proof. No duplicate ledger may independently declare availability. |
| `kms/render/scene.rs::PendingAck` / pending pool releases | Retain the referenced allocations, render `FenceTicket`, command/descriptor pool slot and its cleanup dependency until GPU use ends. | Damage snapshots, submitted participants and Present milestones stay with their existing logical consumers. `drain_pending_pool_releases` becomes a GPU-evidence producer plus service-authorized slot return; hardware completion alone cannot return GPU-used descriptors. |
| Backend Present pins and wakes | Convert `present_source_pins` to leases owning actual source/fallback references; retain each `PinnedWake` object, not an XID lookup. | `release_present_source` preserves `store_decref_with_invalidate` semantics through the service boundary. `retained_present_wakes` follows independent release evidence. Ordered Skip metadata has no allocation ownership solely for notification retention. |

A pool lease covers one exact allocation generation, not an output/BO index.
Aliased source/fallback or grouped-output references share an allocation entry;
usage reservations remain distinct so releasing one cannot authorize another's
reuse. Pool capacity/role reservations remain required before allocating or
importing. Returning an allocation to `PixmapPool` ends the old lease generation
only after all dependencies; subsequent checkout creates a new generation.

## 2. Completion and acquisition adapters

| Existing entry point | Required converted behavior |
| --- | --- |
| `PlatformBackend::acquire_scanout_bo` | Replace the `BoPhase::Free`-only decision with a service check-and-reserve; returned token includes the exact allocation lease. Validate both destination and paired renderer resources on copied routes. No additional overflow allocation. |
| `cancel_scanout_bo_recording` | Relinquish only the recording reservation. If GPU work started, its ticket still gates reuse; cancellation is not completion. |
| `on_page_flip_complete` | Keep legacy behavior only on the Legacy route. Converted evidence passes through `DeviceCommitOwner`; service receives its matching KMS release obligation rather than immediately transitioning old storage to Free. Do not equate page events with the governing canonical release evidence. |
| `register_scanout_render_completion` / `drain_scanout_render_completions` | Register before dispatch and route readiness to exact incarnation/generation/ticket. FD readiness alone retains the existing Vulkan import/wait and ownership transitions. Cancellation/removal transfers unresolved obligations instead of implying GPU completion. |
| `FenceTicket::poll_signaled_result`, store pending retirement and engine retirement | Service owns outstanding ticket registrations and polls without blocking. Errors retain obligations and follow renderer-failure policy. Logical store removal cannot cancel pending cleanup. |
| `service_owner_completions`, executor-control handling | Deliver owner-validated evidence to the service inbox. Keep submitted/current/retiring/quarantined references until their independent obligations resolve. |
| `KmsBackend::next_wakeup`, `PlatformBackend::poll_fds` | Include resource-service completion FDs and the earliest positive retry deadline for tickets without an FD. Reuse the existing core poll integration; service deadlines must remain active with VT-away, DPMS-off or composition idle, like owner/executor deadlines. Recheck on waiter registration. A pending ticket must not force an immediate busy loop. |
| `read_scanout_region` / `include_inferiors_root_snapshot` | Reserve actual read source and staging resources before GPU work; successful one-shot wait followed by CPU copy ends that read. Copied-route read uses the renderer target, not an external transport acquire. Scratch upload/Composite has separate store/GPU tickets and one cleanup. Uncertain read submission retains its dependencies. |
| Border relayout / `configure_subwindow`, promotion | In-place pixel movement requires compatible exclusive usage. Otherwise defer or separately allocate within existing capacity; copy retains old and new generations until its ticket resolves. Publish offset and new selection together and invalidate queued direct eligibility. |
| `retire_direct_output` and shadow materialization | Preserve composed-return allocation leases and dependencies before direct entry. Use reserved exit retirement for unflip; do not require free ordinary retirement or overwrite held source/composed pixels. |

Non-exportable Vulkan fence tickets need a finite scheduled poll in the core
service, not a fictitious ready FD. The implementation plan may choose the
positive retry interval using the existing polling conventions; it must test
progress with no new damage or input and no spin on unsignaled/error tickets.
Resource-service wake handling must run independently of scene submission gates.

## 3. Replacement and teardown integration

`PlatformBackend::reset_scanout_bos_for_suspend`, `drain_scanout_pool_at`, and
installed-pool replacement must detach old generations into the resource
service. Successful synchronous modeset/disable supplies only the evidence
allowed by the governing spec; it does not erase read/GPU/FOREIGN obligations.
`OutputScanout::drain_all_pending`, `ScanoutBoPool::drain_all_pending` and
`CopiedScanoutPool::drain_all_pending` are existing cleanup boundaries to adapt,
not proof that a dropped container is safe. The final-exit disarm path cannot
serve as a runtime retaining supervisor.

`KmsBackend::shutdown_destroy_drawables` and `DrawableStore::shutdown_destroy_all`
first detach logical entries. Move retained resources, pending promoted images,
scene pool slots, ticket registrations and cache invalidation/cleanup rights
into the incarnation bundle before their ordinary engine/store/platform owners
are destroyed. Cleanup must retain the necessary cache/resource slice, or have
completed its invalidation under valid dependencies before handoff; it must
never call back into the destroyed engine. Supervisor and backend service run
on the same core thread: mapped staging and GBM resources remain `!Send`.

Use the companion §4 supervisor contract for atomic inbox/wake transfer. The
transport gate in the main design remains mandatory: all writer classes must
be owner-mediated or disabled before publishing Owner. The 2c-i fixtures must
exercise actual lease adapters and a mock retaining recipient; production
activation still requires the real stage-3 supervisor and later writer work.

## 4. Evidence and readiness

The companion regression matrix is required for each applicable family,
including imported/server-owned/promoted storage, shared/copied scanout,
cache eviction, layout migration and pending read handoff. Add completion-only
progress while VT/DPMS suppress composition, and promotion/pool-return with an
old allocation lease. Generic integer/drop-count fixtures alone are inadequate.

This inventory identifies the concrete ownership seams for planning; it does
not certify every producer call site as converted. The executable plan must
map tasks to these seams, and implementation must inspect callers of each
replacement/cleanup API before changing it. Any additional direct destructor
or raw-handle escape discovered there must be routed through the same contract
before that adapter is considered complete. No production Owner route can use
an unconverted writer or allocation lifetime path.

Round 2 retains its historical INCOMPLETE verdict. Round 3 accepted the revised
availability design, this inventory and the previously uncovered M-2 writer
gate within its declared scope. This establishes design readiness for writing
the plan, not implementation completion or hardware validation.


## Upstream integration addendum — 2026-09-10

Baseline additions from `a06cf0e0` are not covered by design round 3:

- Imported allocation ownership includes the original client dma-buf FD,
  `DrawableImage::drm_modifier`, `import_plane0`, `import_size`, and
  `ImportedDmabufMetadata::implicit_layout`. Re-export duplicates that client
  buffer and preserves its request metadata, not a guessed Vulkan layout.
  Implicit is distinct from explicit LINEAR; implicit exports report INVALID
  and remain ineligible for direct scanout. Preserve single-plane advertisement.
- `ServerState::cow_claims` owns overlay claims. Backend overlay calls are
  0→1/1→0 edges; retained physical COW/source/fallback leases do not create
  logical claims. Preserve deferred release/reclaim and the disconnect failure
  latch `cow_teardown_failed`; never recreate the removed backend counter.
- Concrete lease tests must include metadata-preserving imported-buffer round
  trips and final overlay release/disconnect while direct resources are retained.

The plan's first review findings and local dispositions are recorded separately;
these additions require their own reassessment before plan execution.
