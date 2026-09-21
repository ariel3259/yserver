# Phase C.0 stage 2c-iii — direct framebuffer adoption and retention

**Status:** design, revision 1 (2026-09-21). Its three design sections (the
ownership boundary, the handoff and ownership table, the evidence) were
approved one by one with the user in brainstorming, together with the scope
decision of section 1.1. Not yet reviewed; the implementation plan follows the
codex review of this document.

**Why this document exists.** Plan Ciii's Task 6 stopped with an F8
(`../findings/2026-09-21-stage-2c-iii-plan-ciii-task6-f8.md`): the real direct
producer reaches `register_commit_dependencies`, but hands it `CommitResources`
whose `allocations` is empty (`take_direct_owner_resources`,
`kms/render/backend.rs:2567`). The KMS framebuffer/GEM import a direct commit
scans out is owned by the M1 probe cache (`ScanoutM1ProbeEntry`,
`backend.rs:520`), and `DirectScanoutProbeFramebuffer::into_managed`
(`drm/modeset.rs:1426`) has no production caller. So the framebuffer never
enters the resource ledger, a same-source successor cannot carry the same
`AllocationKey`, and P3-3 — a retained buffer registers no `KmsRelease`
obligation — is unprovable on any device. This is the item the 2c-i resource
plan left open ("production adoption, payload alias registration and weak
cache indexing" for the later producer conversion), which Cii converted the
direct producer without closing.

**Authority**, most general first. This document elaborates one missing piece of
the 2c-i ownership contract on the Owner route; it does not replace or relax
any of them.

1. [C.0 specification](2026-08-26-phase-c0-atomic-kms-migration-design.md): §10.2
   (retirement milestones; `PriorBufferReleased` item 6), §10.4 (Present and
   release terminalization).
2. [Stage 2c-i design](2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md):
   the resource table (direct framebuffer/GEM import must be retained by active
   and quarantined records independently of cache membership; the converted cache
   holds only weak indices outside the six bounded direct roles) and §6 (the six
   direct-resource roles).
3. [Stage 2c-i debt design](2026-09-15-phase-c0-stage-2c-i-debt-design.md) §9.2
   and §9.5: P3-2 / P3-3 and their F8.
4. [Stage 2c-iii design](2026-09-19-phase-c0-stage-2c-iii-conversion-design.md)
   §3.2 (registration is a production caller, non-deferrable), §5.4 (leases by
   value), §6.4 (the hardware test and its retained-allocation step).
5. Plan Ciii (`../plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md`)
   and its F8 finding, whose seven required points section 8 maps.

## 1. Scope

**1.1. Owner route only (user, 2026-09-21).** Adoption happens on the Owner
route, at direct-candidate preparation; the M1 probe cache becomes an index for
adopted entries. **The Legacy route does not change**: on a device without an
active conductor the probe, the cache's strong ownership, `submit_direct_frame`
and cache eviction behave exactly as today. Rejected: converting the whole M1
cache (Legacy included) to weak indices — Legacy is migration scaffolding that
C.0 deletes before its squash (C.0 §1, §18), so that work would land on code
that is removed.

**1.2. Delivers.** A direct commit dispatched on the Owner route carries its
scanned framebuffer as a managed `DirectFramebufferAllocation` lease in
`CommitResources::allocations`; a valid same-source successor carries the
**same** `AllocationKey`; the registry that destroys FB/GEM imports has a
production home beside the resource service; every rollback and terminal path
has one owner. With it, plan Ciii Task 6 resumes unchanged and P3-2/P3-3 are
proven on card1.

**1.3. Does not.** Touch `register_commit_dependencies`'s comparison (it keeps
comparing `allocations` only — comparing `source` is explicitly not a repair,
per the F8 finding); add a seventh direct role; convert the copied route; change
the Present pins (`source`/`fallback`, spec 2c-iii §5.4); activate anything in
production (C0-R8).

## 2. Ownership boundary, identity and registry

**2.1. The registry lives beside the service.** A `DrmCleanupRegistry` is
installed together with the `ResourceService` (`install_resource_service`,
`backend.rs:2289`), both bound to the same device incarnation, through the same
entry that installs the service today. Production installs neither yet (C0-R8:
`platform.rs:6658` records that no registry reaches the platform); the Owner
fixtures stop constructing a registry of their own and hold it through this
entry. The producer reaches both with device scope and without a long-lived
backend borrow (2c-ii §11's constraint). A registry of another incarnation is
never reachable from a producer.

**2.2. Adoption at preparation.** On an Owner device,
`managed_prepare_direct_candidate` (`backend.rs:20800`), at the point that
today only checks `fb_ready` (`backend.rs:20849`), consumes the accepted M1
`DirectScanoutProbeFramebuffer` with `into_managed` into a
`DirectFramebufferAllocation`, registers it in the service through the registry,
and the prepared candidate holds an `AllocationLease` on it from then on. The
cache entry becomes **`Managed`**: it stays as an index (so a later Present of
the same source finds the adoption) and its `Drop` destroys nothing. Adoption
failure rejects the candidate exactly as `framebuffer_missing` does today, with
the `Preparing` reservation cancelled and nothing half-adopted.

**2.3. Stable physical identity.** The reuse key of a managed framebuffer is
`(source_id, storage allocation generation, topology generation)`. A second
candidate whose key matches reuses the **same** managed allocation with a new
lease; a different key — relayout, new topology, new incarnation — is a new
import that never aliases the previous one. This is what makes P3-3 observable:
the successor carries the same `AllocationKey` as the current commit.

**2.4. The cache is shared by both routes.** The probe runs before the fork, so
Legacy sees the same entries. Invariants: a `Managed` entry serves no raw
framebuffer handle to the Legacy submit path (which borrows one today,
`submit_direct_frame`); on an Owner device that path does not run, and if it
were reached it fails closed. Evicting a `Managed` entry
(`MAX_M1_PROBE_CACHE_ENTRIES`, `backend.rs:541`) removes the index and touches
the allocation not at all.

## 3. Handoff and the ownership table

**3.1. Producer → record.** The prepared Owner candidate carries the framebuffer
`AllocationLease` from adoption. In the ledger closure, `direct_owner::resources`
(`kms/render/direct_owner.rs:142`) moves it **by value** into
`CommitResources::allocations`, beside the `source`/`fallback` storage leases it
already moves. They are different rights and stay separate:
`register_commit_dependencies` (`resources/commit.rs:650`) keeps deciding
retained versus displaced from `allocations` alone, where the framebuffer now is.

**3.2. The six roles.** The framebuffer lease travels with the role the
candidate already occupies (`DirectRole`, `resources/capacity.rs:7`):
`Preparing` → `Successor` → `Submitted` → `Current` → `OrdinaryRetirement` /
`ExitRetirement`. No seventh owner and no extra release duty. When a successor
reuses the current commit's allocation, two leases coexist on one allocation,
one in `Current` and one in `Successor`, both inside the existing capacity.

**3.3. Ownership table** — the sole owner before and after each transition, and
what permits FB/GEM cleanup:

| Transition | Before | After | Cleanup permitted by |
| --- | --- | --- | --- |
| Probe rejected | nobody | nobody | — |
| Adoption fails | cache (strong) | cache (strong); candidate rejected, `Preparing` cancelled | as today |
| Adoption | cache (strong) | service; cache = index; lease in `Preparing` | never while a lease exists |
| Successor replacement | lease in `Successor` | lease dropped (never-submitted path); the allocation persists if another lease holds it | the last lease release |
| Capacity / `begin` / pre-IPC refusal | lease on the candidate | lease back on the candidate → `Desired`, exactly once | — |
| Kernel rejection | commit's `allocations` | `ResourcesStillCurrent` returns the old; the new goes to `rejected` and its lease is released | lease release |
| `Completed` + retirement | old in `Current` | old → releasing with its `KmsRelease` obligation; new → `Current` | `KmsRelease` discharged **and** no lease |
| Reuse (same-source successor) | same key in `Current` | the retained allocation registers **no** obligation (P3-3) | — |
| `CompletionUnknown` / quarantine | commit | retained in quarantine, never released here | stage 3's teardown barrier |
| Cache eviction | `Managed` index | index gone; allocation unchanged | — |
| Topology change / device loss / teardown | live leases | new imports get a different key; live leases follow the existing fd-family barrier | `Superseded` by the barrier |

**3.4. Cleanup exactly once.** A managed framebuffer is destroyed only when the
service reports the allocation releasable and no lease holds it, by the
incarnation's registry. Neither the cache entry nor the direct frame destroys
it.

## 4. Verification

**4.1. Legacy is unchanged**, byte for byte, on a device without a conductor:
the existing software and hardware gates pin it.

**4.2. Hardware-free evidence through the real producer** (the Cii Owner
fixture; tests `c0_conv_cfb_*`), each criterion with the mutation that must
break its named test:

| Invariant | Mutation that must fail it |
| --- | --- |
| A dispatched direct commit has non-empty `allocations` holding the adopted framebuffer | pass `Vec::new()` again |
| A valid same-source successor carries the same `AllocationKey` | mint a new allocation on every preparation |
| After a relayout or a topology change the key differs | ignore the generation in the reuse key |
| Every pre-submit refusal returns the lease to the candidate, exactly once | drop it on refusal; return it twice |
| Cache eviction cannot destroy a current or submitted allocation | keep the cache as a strong owner |
| FB/GEM cleanup happens exactly once, by the registry | also destroy in the entry's `Drop` |
| Legacy gets no handle from a `Managed` entry | serve the handle anyway |
| Registry and service are installed together, same incarnation | install the service alone |

**4.3. Re-entry into Ciii.** With this plan accepted, plan Ciii Task 6 resumes
as written: composed → direct → same-source direct → unflip on card1, with T32
(displaced registration) and T33 (retained non-registration) as hardware
mutations. No hand-built `CommitResources`, synthetic allocation or direct call
to the registration helper substitutes for that route.

## 5. Plan

One plan, four tasks, reviewed by codex before implementation and implemented by
codex unsandboxed (the Vulkan fixture needs the GPU): (1) the registry beside
the service; (2) adoption at preparation, the cache as index; (3) the handoff into
the ledger and every rollback of section 3.3; (4) the tests of section 4.2.

## 6. Mapping to the F8 finding's required points

| Required point | Section |
| --- | --- |
| 1 Authoritative ownership boundary | 2.2, 2.4 |
| 2 Stable physical identity | 2.3 |
| 3 Producer-to-record handoff | 3.1 |
| 4 Every rollback and terminal path | 3.3, 3.4 |
| 5 Registry and service access | 2.1 |
| 6 Bounded retention | 3.2 |
| 7 Executable evidence and mutations | 4.2, 4.3 |
