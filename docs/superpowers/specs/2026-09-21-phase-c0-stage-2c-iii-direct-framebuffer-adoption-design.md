# Phase C.0 stage 2c-iii — direct framebuffer adoption and retention

**Status:** design, revision 3 (2026-09-21). Its three design sections (the
ownership boundary, the handoff and ownership table, the evidence) were
approved one by one with the user in brainstorming, together with the scope
decision of section 1.1. Revision 2 incorporates codex round 1
(`../findings/2026-09-21-stage-2c-iii-direct-fb-adoption-review-round1.md`:
2 blocking, 2 major, all verified against the tree and accepted): the reuse
identity is the service's `AllocationKey` behind a live-checked index that is
removed when its position retires (B-1); the adoption transaction has an
intermediate owner and a defined failure exit (B-2); managed source storage is
a prerequisite, adopted in production at preparation (M-1, new section 2.5);
the evidence covers every consume arm and seam exit (M-2).
Revision 3 incorporates codex round 2
(`../findings/2026-09-21-stage-2c-iii-direct-fb-adoption-review-round2.md`:
1 blocking, 2 major, all verified and accepted): a failed FB/GEM cleanup keeps a
retry-capable owner (B-1, 2.6); **storage is not adopted at preparation** —
the source is retained by the present pin the frame already holds and the
index uses a store-side backing identity (M-1, 2.5 rewritten); the index is
removed on the allocation's direct-lease count reaching zero, kept by one
component (M-2, 2.3). The implementation plan follows the next review.

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
   and its F8 finding, whose seven required points section 6 maps.

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
the same source finds the adoption) and its `Drop` destroys nothing. The
transaction's order, its intermediate owner and its two failure exits are
fixed in 2.6; the source retention it needs is 2.5's.

**2.3. Stable physical identity (round-1 B-1).** The identity of a managed
framebuffer **is the service's `AllocationKey`** — device, incarnation and
generation (`resources/mod.rs:398`) — nothing else. The cache index maps
`(source DrawableId, the source's backing serial, topology generation)` to a
**live-checked token** of that allocation (a weak reference the service can
refuse to upgrade), never to a strong owner. The **backing serial** is a
store-side identity of the drawable's storage that changes on every storage
replacement (relayout, re-import, migration) and never repeats; if the store
does not carry one today, the plan adds it to the drawable and bumps it at
every site that replaces `storage`. Lookup succeeds only if the token upgrades
to a live allocation of the **current** incarnation; a mismatch, a failed
upgrade or a different topology generation forces a fresh probe and import,
which never aliases the previous one.

**Removal (round-2 M-2).** One component owns the allocation-correlated count:
the commit consumer, which already sees every direct-role transition, keeps a
count of live direct-role leases **per `AllocationKey`**. The index entry is
removed on that count's `1 → 0` transition (the allocation's last direct role
retired — 2c-i: "remove the index when its position retires"), on incarnation
loss, and when the source drawable is dropped (the existing removal site). A
`2 → 1` transition (a same-source successor terminalized while the current
commit still scans the allocation) keeps the index, so the next same-source
successor still reuses the current allocation. The capacity's role slots
(`capacity.rs:29`) record no allocation identity and are not extended for
this. A second candidate whose lookup succeeds reuses the same allocation with
a new lease. This is what makes P3-3 observable: the successor carries the same
`AllocationKey` as the current commit.

**2.4. The cache is shared by both routes.** The probe runs before the fork, so
Legacy sees the same entries. Invariants: a `Managed` entry serves no raw
framebuffer handle to the Legacy submit path (which borrows one today,
`submit_direct_frame`); on an Owner device that path does not run, and if it
were reached it fails closed. Evicting a `Managed` entry
(`MAX_M1_PROBE_CACHE_ENTRIES`, `backend.rs:541`) removes the index and touches
the allocation not at all.

**2.5. The source is retained by the frame's present pin; storage is not
adopted (round-1 M-1, round-2 M-1).** `into_managed` today takes a
non-optional source `AllocationLease` (`modeset.rs:1426`), but the
`DirectFramebufferAllocation` it builds stores it as `Option`
(`drm_cleanup.rs:548`). Adopting the source storage at preparation is **not**
the answer: `Storage::into_managed` (`store.rs:606`) turns the drawable's
backing `Managed`, and consumers that still dereference raw storage — the
root extent lookup (`backend.rs:14005`), the direct-read/GetImage routing
(`backend.rs:17814`) — panic on a `Managed` backing (`store.rs:132`). The
managed-storage access adaptation is a separate item; this document records
it as **open** (2c-i's managed storage contract, owed before the Owner route
is production-active) and does not depend on it.

Instead: the right that keeps the imported dma-buf alive while the framebuffer
scans it out is the **present-source pin** the direct frame already holds by
value from preparation to release (2c-iii §5.4, F13b-D1), the same right the
Legacy route relies on today. The plan gives `into_managed` an entry that
takes `Option<AllocationLease>`: `Some` when the source drawable's backing is
already managed by this device's service (then the lease is the
`share_storage_read` that `pin_direct_source`, `backend.rs:2521`, already
takes), `None` otherwise. A `None` never weakens retention: the pin is
released only by the ledger's release path, after the framebuffer lease. The
fallback target is treated the same way. Legacy devices change nothing.

**Multi-device.** The framebuffer allocation belongs to the service of the
device the candidate's CRTC is on (2c-iii §6.2); the source drawable is
device-agnostic and is only *pinned*, never adopted, so no other device's
service is ever asked for a lease on it.

**2.6. The adoption transaction (round-1 B-2).** `into_managed` registers the
cleanup right in the registry **before** the service adopts the payload, and
service adoption can fail and return the payload (`adopt_unchecked`,
`resources/mod.rs:368`). The order and the intermediate owner are therefore
fixed:

1. the service's adoptability is checked first (not exhausted, capacity for
   the `Preparing` role reserved) — a refusal here rejects the candidate with
   the cache still the strong owner, as in 3.3's "adoption fails" row;
2. `into_managed` runs: from this point the cache entry is `Managed` and the
   returned `DirectFramebufferAllocation` is the **sole** owner of the
   framebuffer, the GEM handle and the registered right;
3. the service adopts it; on success the candidate holds the lease and the
   index of 2.3 is written.

If step 3 fails despite step 1, the returned payload is not dropped and the
cache is **not** reconstructed as a strong owner (that would be two cleanup
authorities). The payload is handed to the registry's cleanup, which releases
FB, GEM and right exactly once on success; the index is not written; the
candidate is rejected; the next Present of that source re-probes. That failure
is a service-level fault, so it also closes admission on the device, as a lock
mismatch does (2c-ii §7).

**Cleanup itself is fallible (round-2 B-1).** `discharge_file_owned`
(`drm_cleanup.rs:613`) puts the right back into the payload when `consume`
fails — frozen family, `RMFB` or `GEM_CLOSE` error — and today nothing retains
such a payload. So the registry gains a bounded **pending-cleanup owner**: a
payload whose cleanup failed is retained there, with its right and its
`Preparing` charge, until a later cleanup attempt succeeds (the registry's own
R3 retry, resuming from `FramebufferRemoved`) or the existing teardown barrier
takes ownership of the whole family. It is never dropped and never
reconstructed as a cache owner; the capacity charge is released only with the
payload. This is the 2c-i rule that uncertain cleanup retains its position
while the transport closes.

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
| Adoption refused before `into_managed` (2.6 step 1) | cache (strong) | cache (strong); candidate rejected, `Preparing` cancelled | as today |
| Adoption | cache (strong) | service; cache = index; lease in `Preparing` | never while a lease exists |
| Service adoption fails after `into_managed` (2.6 step 3) | payload (sole) | registry cleanup (sole), index absent, admission closed | the registry, exactly once |
| Cleanup of that payload fails (`RMFB`/`GEM_CLOSE`/frozen) | registry cleanup | registry pending-cleanup owner, `Preparing` charge kept | a later successful retry, or the teardown barrier |
| Successor replacement | lease in `Successor` | lease dropped (never-submitted path); the allocation persists if another lease holds it | the last lease release |
| Capacity / `begin` / pre-IPC refusal | lease on the candidate | lease back on the candidate → `Desired`, exactly once | — |
| Kernel rejection | commit's `allocations` | `ResourcesStillCurrent` returns the old; the new goes to `rejected` and its lease is released | lease release |
| `Completed` + retirement | old in `Current` | old → releasing with its `KmsRelease` obligation; new → `Current` | `KmsRelease` discharged **and** no lease |
| Reuse (same-source successor) | same key in `Current` | the retained allocation registers **no** obligation (P3-3) | — |
| `CompletionUnknown` / quarantine | commit | retained in quarantine, never released here | stage 3's teardown barrier |
| Cache eviction | `Managed` index | index gone; allocation unchanged | — |
| Direct-lease count `1 → 0` / incarnation lost / source drawable dropped | live index | index removed; the allocation follows its own release | — |
| Direct-lease count `2 → 1` (successor terminalized, current still scanning) | live index | index kept | — |
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
| The framebuffer's source retention is the frame's present pin; storage is never adopted at preparation; a managed source passes its read lease (2.5) | release the pin before the framebuffer lease; adopt the storage at preparation (a raw-storage consumer must then panic in the test) |
| The index key uses the backing serial: a relayout/re-import of the source changes it (2.3) | key by DrawableId alone |
| The index holds a live-checked token; a stale or foreign-incarnation entry forces a fresh probe (2.3) | upgrade the token without checking the incarnation |
| The index is removed on the direct-lease count's `1 → 0` and kept on `2 → 1` (2.3) | remove on `2 → 1`; never remove |
| Service adoption failure after `into_managed` releases exactly once through the registry and closes admission (2.6) | drop the returned payload; reconstruct the cache as strong owner |
| A failed cleanup (forced `RMFB` and forced `GEM_CLOSE` failure, each) retains the payload in the pending-cleanup owner with its charge, and a later retry releases exactly once (2.6) | drop the payload on cleanup failure; release the `Preparing` charge before cleanup succeeds |
| Both completion orders keep one lease owner: `HardwareComplete` before and after `CompletionRetired` (round-1 M-2) | discharge the framebuffer's obligation at `HardwareComplete` when no retirement recorded it |
| `ResourcesStillCurrent` returns the framebuffer lease to current; `ResourcesReleased` releases it (M-2) | drop the lease on `ResourcesStillCurrent` |
| Unknown / quarantine retains the lease, never releases it (M-2) | release it on `CompletionUnknown` |
| Dispatch undo (`managed_undo_direct_dispatch`) and successor terminalization return the lease once (M-2) | release it twice |
| Unflip exit retirement moves the framebuffer lease with the `ExitRetirement` role (M-2) | leave it in `Current` |

**4.3. Re-entry into Ciii.** With this plan accepted, plan Ciii Task 6 resumes
as written: composed → direct → same-source direct → unflip on card1, with T32
(displaced registration) and T33 (retained non-registration) as hardware
mutations. No hand-built `CommitResources`, synthetic allocation or direct call
to the registration helper substitutes for that route.

## 5. Plan

One plan, five tasks, reviewed by codex before implementation and implemented by
codex unsandboxed (the Vulkan fixture needs the GPU): (1) the registry beside
the service and its pending-cleanup owner; (2) the backing serial and the
live-checked index with its count-driven removal (2.3); (3) framebuffer
adoption with the optional source lease and the transaction of 2.6; (4) the
handoff into the ledger and every row of section 3.3; (5) the tests of
section 4.2.

## 6. Mapping to the F8 finding's required points

| Required point | Section |
| --- | --- |
| 1 Authoritative ownership boundary | 2.2, 2.4, 2.6 |
| 2 Stable physical identity | 2.3, 2.5 |
| 3 Producer-to-record handoff | 3.1 |
| 4 Every rollback and terminal path | 3.3, 3.4 |
| 5 Registry and service access | 2.1 |
| 6 Bounded retention | 3.2 |
| 7 Executable evidence and mutations | 4.2, 4.3 |
