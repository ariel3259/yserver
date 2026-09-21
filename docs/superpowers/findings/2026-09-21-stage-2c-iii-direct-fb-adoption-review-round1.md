# Direct framebuffer adoption design — codex review, round 1

**Target:** `docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md`
revision 1 (`0cd8d81b`), against the 2c-iii conversion design as the passed parent.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
Coverage INCOMPLETE (24/24): C.0 §§10.2/10.4, debt §§9.2/9.5, the F8 document and
the exact `managed_undo_direct_dispatch`/terminalize/unflip seams were not reopened.

**Author verification (2026-09-21), every finding checked against the tree:**

- **B-1 — CONFIRMED.** The key had no device/incarnation; `AllocationKey`
  carries both (`resources/mod.rs:398`); 2c-i says "remove the index when its
  position retires". Fixed: 2.3 rewritten around a live-checked token.
- **B-2 — CONFIRMED.** `into_managed` (`modeset.rs:1426`) registers the right
  before the service adopts; `adopt_unchecked` (`resources/mod.rs:368`) can
  return the payload. Fixed: new 2.6, transaction order and failure owner.
- **M-1 — CONFIRMED.** `pin_direct_source` (`backend.rs:2521`) has a lease only
  for `Managed` backing; `Storage::into_managed` (`store.rs:606`) has test
  callers only. Fixed: new 2.5, storage adoption at preparation.
- **M-2 — CONFIRMED.** Fixed: five evidence rows for the consume arms and seam exits.

Revision 2 incorporates all four.

---

## Verdict

**2 blocking, 2 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status |
| --- | --- |
| None | Skipped as directed. The Task-6 F8 is the design’s input, not a prior review of this document. |

## Findings

### Blocking

#### B-1 — Managed reuse is not scoped to the DRM device/incarnation and has no stale-index contract

The reuse key is `(source_id, storage allocation generation, topology generation)`, yet the design asserts that a new device incarnation necessarily produces a different key ([target lines 92–97](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:92)). Neither device identity nor incarnation appears in that tuple.

This conflicts with the authoritative design’s per-`DrmDeviceKey` isolation requirement ([authoritative spec lines 496–500](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:496)) and 2c-i’s incarnation-bound cleanup requirement ([resource design lines 36–40](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:36)). The service’s real `AllocationKey` already contains device and incarnation ([resources/mod.rs lines 398–402](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:398)), while the existing M1 cache is keyed only by `DrawableId` ([backend.rs lines 543–569](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:543)).

The design also leaves a `Managed` index until ordinary eviction ([target lines 99–105](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:99)), whereas 2c-i requires removing the index when its occupied role retires ([resource design lines 425–431](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:425)).

Concrete failure: the device is recreated with the same numeric topology generation while the drawable/storage generation remains unchanged, or the allocation’s last role lease retires while the cache index remains. A later candidate can resolve the index to an allocation belonging to the old fd family or to already-cleaned FB/GEM handles.

Required correction: include `DrmDeviceKey` and incarnation in the reuse identity, store only a weak/live-checked allocation token, remove or poison it on final role retirement and incarnation loss, and specify that mismatch or failed weak upgrade forces a fresh probe/import.

#### B-2 — Adoption failure has no valid owner transition after conversion begins

The design says preparation consumes the cached probe with `into_managed`, registers it in the service, and on any adoption failure restores the cache as strong owner with “nothing half-adopted” ([target lines 81–90](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:81), [ownership table line 130](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:130)). It does not define how that rollback is possible.

`into_managed` first replaces `Legacy` with `Managed`, consumes the framebuffer/GEM owner, and registers a cleanup right ([modeset.rs lines 1426–1448](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/drm/modeset.rs:1426)). Subsequent service adoption can fail and return the payload—for example on generation exhaustion ([resources/mod.rs lines 368–402](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:368)); the registry payload alias is installed only after successful adoption ([lines 428–438](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:428)).

Concrete failure: `into_managed` succeeds, service adoption fails, and candidate rejection follows the table. Dropping the returned payload leaks its file-owned right; reconstructing the legacy cache while that right remains registered creates competing cleanup authority. This violates the authoritative failure rule that resources are handed back on fallible ownership transfer ([authoritative spec lines 185–191](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:185)).

Required correction: define the transaction’s intermediate owner and rollback. Either explicitly undo the registry right and reconstruct exactly one legacy owner, or retain the returned managed payload in a bounded Preparing/quarantine owner and close admission. It cannot simply be reported as ordinary candidate rejection.

### Major

#### M-1 — Preparation does not define how the storage generation and required source lease are obtained

The key depends on the storage allocation generation, and `into_managed` requires an `AllocationLease`, but the design never states the prerequisite or producer for either.

Existing storage exposes a lease only for `Managed` backing ([store.rs lines 190–202](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/store.rs:190)); `pin_direct_source` therefore records an optional lease ([backend.rs lines 2521–2535](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2521)). By contrast, framebuffer conversion requires a non-optional source lease ([modeset.rs lines 1426–1430](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/drm/modeset.rs:1426)). The authoritative contract requires present-pin leases by value, with empty lease construction rejected ([authoritative spec lines 455–460](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:455)).

Concrete failure: an otherwise valid Owner candidate reaches preparation while its imported storage remains legacy-backed. The implementation must either reject the required real path, invent a generation from mutable drawable identity, or call conversion without its required source lifetime.

Required correction: mandate managed storage adoption before framebuffer adoption, derive the storage generation from that exact allocation lease/key, and define the bounded failure handoff when that prerequisite cannot be established.

#### M-2 — Verification does not establish the ownership table’s terminal and ordering claims

The design claims every rollback and terminal path has one owner ([target lines 55–60](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:55)), but its evidence matrix covers pre-submit refusal, cache eviction, and cleanup only ([lines 151–165](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:151)).

The real consumer has distinct `HardwareComplete`/`CompletionRetired` ordering states ([commit.rs lines 285–355](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/commit.rs:285)) and separate `ResourcesStillCurrent`, `ResourcesReleased`, and unknown/quarantine handling ([lines 358–419](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/commit.rs:358)). None has framebuffer-lease-specific mutation evidence, nor do unflip exit retirement and dispatch undo.

An implementation could therefore pass every listed test while losing or duplicating the framebuffer lease on rejection, late hardware completion, quarantine, or exit retirement.

Required correction: add real-producer evidence and mutations for both completion orderings, both rejection resource events, unknown/quarantine, dispatch undo, and unflip exit retirement.

## Coverage and implementation checks

- Incorporation: skipped because there was no prior review.
- Architecture: assessed service/registry reachability, identity, producer-to-record handoff, six-role accounting, cache sharing, and per-device isolation.
- Safety: assessed adoption rollback, cache liveness, incarnation correlation, rejection/completion ownership, and cleanup authority.
- Compliance/evidence: assessed authoritative §§3.2, 5.4, and 6.4 plus the relevant 2c-i resource table and six-role contract.

**Excerpts used: 24/24.** The budget was exhausted. C.0 §§10.2/10.4, debt §§9.2/9.5, the F8 document itself, and the exact `managed_undo_direct_dispatch`/terminalize/unflip implementations were not reopened; they are unassessed, not sound. A bounded follow-up should inspect only whether those terminalization texts or seam implementations introduce obligations beyond B-2/M-2.

Builds, tests, formatting, clippy, portability, Rust borrowing, and exact API/test names remain deferred to implementation.