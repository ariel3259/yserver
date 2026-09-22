# Direct framebuffer adoption design — codex review, round 2

**Target:** revision 2 (`79f6f72a`), with round 1 as the prior review.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
Coverage INCOMPLETE (24/24): `managed_handle_direct_unflip`'s body, exit-retirement
completion and `register_commit_dependencies` were not fully reopened.

**Author verification (2026-09-21), every finding checked against the tree:**

- **B-1 — CONFIRMED.** `discharge_file_owned` (`drm_cleanup.rs:613`) returns the
  right into the payload on a failed `consume`; nothing retained such a payload.
  Fixed: a registry pending-cleanup owner, charge kept, forced-failure evidence.
- **M-1 — CONFIRMED, and it reverses revision 2's answer to round-1 M-1.**
  `Storage::into_managed` makes raw dereference panic (`store.rs:132`) and
  `root_storage_extent` (`backend.rs:14005`) and the GetImage routing
  (`backend.rs:17814`) still dereference raw storage. Fixed: storage is NOT
  adopted; the present pin retains the source; `into_managed` takes an
  `Option<AllocationLease>` (the field already is one). Managed-storage access
  adaptation recorded as an open item.
- **M-2 — CONFIRMED.** Capacity slots record no allocation identity
  (`capacity.rs:29`); service destruction is not "last direct role". Fixed: the
  commit consumer keeps a per-`AllocationKey` direct-lease count; removal on
  `1 → 0`, kept on `2 → 1`.
- Incorporation: round-1 B-1 PARTIAL and B-2/M-1 TRADED are closed by the above;
  M-2 APPLIED.

Revision 3 incorporates all three.

---

## Verdict

**1 blocking, 2 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result only; it does not claim that code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Audit |
| --- | --- | --- |
| B-1 — identity and stale-index contract | **PARTIAL** | Section 2.3 now uses the service `AllocationKey`, incarnation checking, and a non-owning live-check. However, it still lacks an authoritative hook for detecting the last direct-role retirement when Current and Successor share the allocation. |
| B-2 — owner after conversion failure | **TRADED** | Section 2.6 defines the payload as intermediate owner and avoids reconstructing the cache. It introduces a new unowned state when registry cleanup itself fails; see B-1. |
| M-1 — managed source prerequisite | **TRADED** | Section 2.5 supplies a real source lease through storage adoption, but globally changing the drawable to `Managed` is incompatible with existing consumers and lacks a multi-device ownership contract; see M-1. |
| M-2 — terminal/ordering evidence | **APPLIED** | Section 4.2 adds both completion orders, rejection dispositions, quarantine, dispatch undo, successor terminalization, and exit-retirement evidence. Exact test implementation remains compiler/test work. |

## Findings

### Blocking

#### B-1 — Cleanup failure after failed service adoption leaves the framebuffer payload without a retry owner

Section 2.6 says that after `into_managed` succeeds but service adoption fails, the returned payload is handed to registry cleanup, released exactly once, and admission is closed ([target lines 137–158](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:137)). This handles successful cleanup only.

Cleanup is fallible. `DirectFramebufferAllocation::discharge_file_owned` restores the cleanup right into the payload when registry consumption fails ([drm_cleanup.rs lines 608–627](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:608)); `consume` can fail because the family is frozen/closed, `RMFB` fails, or `GEM_CLOSE` fails, and returns the still-live right ([drm_cleanup.rs lines 330–364](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:330)). The registry stores no returned `DirectFramebufferAllocation`, and the payload-alias index is registered only after successful service adoption ([resources/mod.rs lines 421–438](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:421)).

Concrete sequence: `into_managed` consumes the cache owner; service adoption fails; `RMFB` fails; cleanup returns the right to the payload; the stated handoff has nowhere to retain that payload. Dropping it leaks the registered right/DRM objects, while closing admission merely prevents new work. This violates the authoritative fallible-transfer rule requiring resources to be handed back ([conversion design lines 185–191](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:185)) and the six-role rule that uncertain cleanup retains its position while closing transport ([2c-i design lines 415–423](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:415)).

Required correction: define a retry-capable owner that retains the returned payload, cleanup right, and `Preparing` charge until cleanup succeeds or the existing teardown barrier takes ownership. Add forced `RMFB` and `GEM_CLOSE` failure evidence; testing only service-adoption failure is insufficient.

### Major

#### M-1 — Preparation-time storage adoption mutates the shared drawable into a backing existing consumers cannot use

Section 2.5 globally replaces legacy source and fallback storage with managed storage during Owner preparation ([target lines 122–135](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:122)). `Storage::into_managed` indeed transfers the allocation into the device-scoped service ([store.rs lines 594–674](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/store.rs:594)), after which ordinary dereference of that storage deliberately panics ([store.rs lines 132–160](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/store.rs:132)).

Existing non-direct consumers still dereference raw storage fields: root/fallback extent lookup does so at [backend.rs lines 14005–14010](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:14005), and direct scanout read/GetImage routing does so at [backend.rs lines 17814–17835](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:17814). Thus an accepted preparation can make a later topology/layout query or scanout read panic. The mutation is also drawable-global while services and conductors are per DRM device ([authoritative lines 496–500](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:496)); the design does not say how another device obtains a lease from storage keyed to the first service.

Required correction: either complete the managed-storage access adaptation before enabling preparation-time adoption, or retain a logical-store indirection that preserves all existing consumers. Specify cross-device ownership explicitly. Evidence must exercise rendering/sampling, GetImage/readback, composed-scene use, topology queries, and shared Owner/Legacy-device access after adoption.

#### M-2 — “Remove on last direct role” has no allocation-correlated transition contract

The design permits the same allocation to occupy Current and Successor simultaneously, while requiring index removal only when its last direct role retires ([target lines 99–112, 170–175](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:99)). Current capacity state records only role and reservation serial, not allocation identity ([capacity.rs lines 29–40](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/capacity.rs:29)); `finish_role` likewise receives no key ([capacity.rs lines 190–203](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/capacity.rs:190)). Service destruction detects zero total uses/obligations, which is not the same event as zero direct roles ([resources/mod.rs lines 824–911](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:824)).

Concrete sequence: Current and Successor share key K; successor terminalization vacates only Successor ([backend.rs lines 2850–2857](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2850)). Removing K on that retirement prevents another same-source successor from reusing still-current K; waiting for service destruction can leave the index beyond the last direct role when other uses remain. The former defeats the retaining commit required by [authoritative lines 520–538](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:520); the latter violates the binding position-retirement rule ([2c-i lines 425–431](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:425)).

Required correction: assign one component responsibility for allocation-correlated direct-role counts, removing the index on the `1 → 0` transition and on incarnation loss. Test both `2 → 1` retention and `1 → 0` removal.

### Minor

None.

## Coverage and implementation checks

- Incorporation: all four prior findings audited.
- Architecture: checked service identity/live lookup feasibility, shared-store conversion, six-role reuse, and per-device ownership.
- Safety: checked adoption/cleanup failure, stale identity, two-lease reuse, successor terminalization, dispatch undo, and the start of exit retirement.
- Compliance/evidence: checked authoritative §§3.2, 5.4, 6.4; C.0 §§10.2/10.4; 2c-i resource/capacity rules; debt §§9.2/9.5; and 2c-ii §7.

**Excerpts used: 24/24.** The budget was exhausted. The complete body of `managed_handle_direct_unflip`, downstream exit-retirement completion, and the current `register_commit_dependencies` implementation were not fully reopened; they are unassessed, not sound. The specific unresolved follow-up question is whether every exit-retirement completion/failure removes an allocation-correlated index only after its final direct role.

Builds, tests, formatting, clippy, portability, Rust borrowing, and exact API/test shapes remain deferred to implementation.