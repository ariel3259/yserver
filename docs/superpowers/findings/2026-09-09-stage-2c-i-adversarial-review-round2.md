## Verdict

**1 blocking, 1 major, 1 minor**

**Coverage: INCOMPLETE**

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `13637318`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.

Reviewed HEAD: `14dd92d818e619357c903ad64e5357f47619111e`.
Target SHA256 before local corrections:
`f7597855a44b41b034d570a44c24e318441cc519498bba1922c0c92491addf24`.
Observed reviewer usage: **61,362 tokens**; author work excluded.
Raw local log: `/tmp/yserver-stage2ci-review-round2.log`.


The resource design is **not ready for writing the 2c-i implementation plan** within the assessed scope. Physical capacity is now adequately designed, but the production availability bridge for retained pool/read leases remains deliberately unresolved. M-2 incorporation could not be independently confirmed within the exhausted excerpt budget.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1 — teardown handoff | **PARTIAL** | The revision adds a pre-established process-lifetime supervisor, atomic move-by-value handoff, late-reply routing, consuming cleanup rights, complete-fd-family closure, and no post-close destructor ioctl ([plan lines 146–217](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L146)). However, it still postpones how actual pool/direct-source dependencies feed availability and survive teardown ([lines 412–418](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L412)). That is an event-delivery/ownership contract, not merely adapter spelling. |
| B-2 — bounded physical resources | **APPLIED** | Six fixed roles cover current, submitted, successor, preparing, ordinary retirement, and exit retirement; reservation precedes import/dispatch; failures close transport instead of overflowing; unflip has an independent exit slot; re-entry waits for both retirements ([lines 275–359](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L275)). The regression matrix exercises probe, supersession, retirement, unflip, unknown, and re-entry. |
| M-1 — Skip metadata growth | **NOT APPLIED** | Correctly retained as an explicit compatibility limitation rather than claimed fixed ([lines 363–375](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L363)). Per the authorized policy, it is not treated as a new 2c-i planning blocker. |
| M-2 — transport exclusion | **PARTIAL** | This companion keeps converted traffic disabled until a receiver exists ([lines 178–182](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L178)), but does not itself establish the per-device writer gate. The claimed correction resides in the context document, which was not reached before the 12-excerpt budget was exhausted; incorporation therefore remains unconfirmed, not sound. |

## Findings

### Blocking

#### B-1 — Read and pool leases have no defined availability-feedback owner

The design requires a scanout-read lease to retain the current allocation through readback and forbids BO reuse while that lease exists ([plan lines 78–85](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L78)), but explicitly leaves unresolved how pool and direct-source dependencies “feed readiness” across teardown ([lines 412–418](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L412)). The generic statement that lease usage prevents reuse does not identify the authoritative availability state, which component receives GPU/read completion, or how that wake reaches the pool after backend handoff.

Concrete sequence: IncludeInferiors snapshots the current composed BO; KMS replacement later supplies `PriorBufferReleased`; ordinary retirement marks the BO reusable; a renderer acquires and overwrites it while the snapshot GPU read is still outstanding. Conversely, retaining it without a completion route can permanently exhaust the finite pool. The spec requires `PriorBufferReleased` to remain independently gated by Vulkan/FOREIGN ownership ([spec lines 2098–2114](2026-08-26-phase-c0-atomic-kms-migration-design.md#L2098)) and states fd closure is not proof for shared Vulkan/GBM resources ([lines 2025–2031](2026-08-26-phase-c0-atomic-kms-migration-design.md#L2025)).

Smallest correction: define one authoritative per-allocation availability ledger, the conjunction of KMS-release, GPU/read-ticket, and FOREIGN conditions, the completion producer and wake consumer, and how those states move into the supervisor. Exact Rust method names can remain for planning.

### Major

#### M-1 — Verification does not exercise the new layout or read-lease contracts

The proposed tests cover destruction counts, generations, aliasing, capacity, and reordered KMS evidence ([plan lines 392–410](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L392)), but none covers border relayout while an old-layout lease exists, in-place relocation exclusion, IncludeInferiors snapshot read concurrent with KMS retirement, or reuse in both GPU-first and KMS-first completion orders.

These are new contracts, not incidental implementation details. Baseline `content_offset` may only change together with pixel relocation ([store.rs lines 697–721](../../../crates/yserver/src/kms/render/store.rs#L697)), while ordinary destruction currently destroys storage directly ([store.rs lines 1126–1136](../../../crates/yserver/src/kms/render/store.rs#L1126)). Generic drop-count tests cannot establish layout interpretation or non-reuse.

Smallest correction: add state-machine and concrete-adapter cases for both completion orders, relayout deferral/separate allocation, late old-generation destruction preserving the new XID mapping, and supervisor handoff during an outstanding snapshot.

### Minor

#### m-1 — Deferred paint identity omits original X11 depth

The design names captured offset, extent, generation, and typed bounds ([plan lines 60–76](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L60)), but does not require preserving `PaintTarget::x11_depth`. Baseline explicitly records original target depth because it may differ from backing-storage depth ([target.rs lines 163–176](../../../crates/yserver/src/kms/render/target.rs#L163)).

A deferred operation reconstructed from a depth-32 redirect backing can therefore lose the original depth-24 target semantics while remaining type-correct. Require deferred paint/read identity to retain original X11 depth, or explicitly state that the complete typed `PaintTarget` value is preserved.

## Coverage and implementation checks

- **Incorporation:** all four prior findings classified; M-2 independently unconfirmed.
- **Architecture/contracts:** examined teardown ownership, fd closure, physical roles, retirement, unflip/re-entry, layout identity, and read-lease availability.
- **Safety/ownership:** verified baseline unconditional DRM and storage destructors and the spec’s independent KMS/GPU/FOREIGN barriers.
- **Spec/verification:** checked §§9.1, 10, 10.2, 10.4, 12, and 18. Capacity aligns materially; availability feedback and its evidence remain incomplete.

Used **12/12 bounded excerpts** beyond the target and prior review: authoritative spec 6, source 6. The main-document M-2 route state, actual scanout pool transitions, and full IncludeInferiors acquisition path remain unassessed—not sound. Builds, formatting, clippy, portability, exact APIs, and executable test behavior remain properly deferred to implementation.

## Author verification and local dispositions — 2026-09-09

The verdict and INCOMPLETE coverage above are preserved. The following changes
were made after the reviewed input; they have not received another external
pass and do not establish readiness for an executable 2c-i plan.

- **B-1: design omission verified; local contract added.** §4 now assigns a
  single per-allocation availability entry to the resource service, defines
  resource-specific KMS/GPU/read/FOREIGN conjunctions, completion producers,
  serialized waiter registration, and atomic transfer of evidence routing to
  the supervisor. Baseline `platform.rs::on_page_flip_complete` directly
  transitions prior BO state toward Free, so converted adapters must route
  through this authority. Concrete field and call-site inventory remains open.
  The review's snapshot example is not a demonstrated baseline race:
  `backend.rs::read_scanout_region` calls
  `ops/mod.rs::run_one_shot_op_with_wait`, which waits for the GPU fence before
  returning CPU bytes. `include_inferiors_root_snapshot` uploads those bytes
  into a separate scratch pixmap. The source and scratch tickets must therefore
  be independent. Pending/uncertain reads still require the new contract.
- **M-1: verified and locally corrected.** The companion matrix now explicitly
  requires relayout exclusion, separate generations and guarded XID retirement,
  original-depth preservation, both KMS/GPU completion orders, synchronous
  snapshot versus scratch lifetime, and late completion across supervisor
  handoff. These are future implementation tests, not executed evidence.
- **m-1: verified and locally corrected.** `target.rs::PaintTarget` explicitly
  captures original X11 depth; §2 now requires retaining that depth alongside
  bounds, offset and generation.
- **Prior M-2: locally checked, external coverage still incomplete.** The main
  design's transport gate covers all writer classes, revoke-before-drain,
  helper permissions, the installed teardown receiver and no legacy re-entry
  after unknown. This is present as a design contract, not implemented or
  independently accepted by round 2.

Remaining readiness work: complete the concrete retained-field/cleanup and
completion-adapter inventory; independently assess the new availability
contract and the previously uncovered M-2 writer gate. Any further external
pass requires explicit authorization under `review/README.md`. No 2c-i
production code or implementation plan was added. Documentation validation:
`git diff --check`; previous integration test results are not new evidence for
these unimplemented contracts.

### Subsequent local adapter elaboration

The [concrete inventory](../specs/2026-09-09-phase-c0-stage-2c-i-resource-adapter-inventory.md)
now maps the retained fields and cleanup/acquisition/completion seams on
`14dd92d8`, including promotion retirement, copied transport dependencies,
scene descriptor slots, pool replacement and shutdown. It specifies service
progress independently of VT/DPMS composition gates and keeps non-Send resource
handoff on the core thread. This addresses the pending design inventory locally;
it is not an exhaustive converted-call-site audit or external acceptance.
Independent coverage of the new contracts and prior M-2 remains outstanding.
