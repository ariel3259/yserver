# Stage 2c-i debt — refusal inventory and census boundary

**Author:** Opus, 2026-09-16. Requested by codex round 1 finding M-1
(`2026-09-16-stage-2c-i-debt-design-review-round1.md`), derived by the author
as measurement before the design is revised, per the user's call.

M-1's point: a mutation census measures guards that **exist**. It cannot see a
refusal the contract requires and the code never implemented. This inventory
reads the governing contracts and checks each refusal against the code.

## Where the contracts actually live

Before mapping anything, one source correction. The debt design was reviewed
against `2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md` as
its authority. That document contains **zero** occurrences of
`publish_owner`, `HandoverPermit`, `Quiescing` or `LegacyDrained`. The Owner
handover contract — census group A, the one with no proven guard at all — is
specified in the **2c-i plan**, Task 6 (around line 1438), which takes it from
the stage-2c design's section 6 and ruling R7. So the inventory uses three
sources, by domain:

| Domain | Contract source |
| --- | --- |
| Transport gate and Owner handover | 2c-i plan Task 6; stage-2c design §6; R7, R11 |
| GPU batches, availability, completion routing | 2c-i design §4; R9 |
| Commit consumer and KMS release | 2c-i design §4–§5; R6 |

## Absent mechanisms — the contract requires it, no code does it

All three are **inert today under R8** (no production issuer, no production
gate). They are recorded because each is exactly the kind of gap that bites at
activation, and none is visible to a mutation census.

**AB-1 — `WriterCoverageProof` certifies nothing.** Plan Task 6: publication of
Owner requires "a complete writer-coverage set", and tests may construct the
proof "only after explicit mock/disabled coverage for every class". The type is
`struct WriterCoverageProof { _private: () }` with one constructor,
`#[cfg(test)] new_for_tests()`, taking **no arguments**, and
`issue_handover_permit` receives it as `_coverage` and never inspects it. It is
a placeholder capability, not the specified proof. The production proof is
stage 3/4 material by design; the test-side clause is simply unenforced.

**AB-2 — `RecipientReservation` carries no identity.** Also
`struct { _private: () }`, received by `issue_handover_permit` as
`_reservation` and never inspected. A reservation for any device satisfies any
gate's handover. This is the same identity-correlation class as round-1 B-2.
Notably the router's `RecipientSlot` *is* device/incarnation-bound and is
validated in `HandoffRouter::transfer` — two tokens, only one correlated.

**AB-3 — an uncertain GPU submission never closes the transport.** 2c-i design
§4: "Unknown submission retains its reservation **and closes the affected
transport** until its specified recovery proof arrives." The retention half
exists (`freeze_uncertain_batch` freezes; census/Q5 shows quarantine freezes).
The close half does not: no path in `gpu.rs`, `mod.rs`, `backend.rs`,
`platform.rs` or `scene.rs` closes the transport on uncertain GPU submission.

## Reclassified census survivors — not fixable with a test

The two `gpu.rs` survivors — the error arms of `cancel_pre_submit_batch` and
`freeze_uncertain_batch` — have exactly two production callers, both in
`scene.rs` (around 7970–7985), and both discard the result:

```rust
let _ = crate::kms::render::resources::gpu::freeze_uncertain_batch(service, &entries);
```

No test can observe those arms through their only caller, because the caller
swallows the error unconditionally. A test of the function in isolation would
pass while the production path still ignores the failure. **The defect is the
swallowed error at the call site**, not a missing test, and the debt design's
item list must say so. It is the same `let _ =` pattern round-1 B-2 showed
defeats `#[must_use]`, and it belongs with census group B (error propagation).

## Checked and not absent

- **Helper revocation before handover.** Plan Task 6 says the permit is issued
  after the gate validates helper revocation, and the gate has no separate
  helper-permission store — "helper" appears once in `transport.rs`, in a doc
  comment. Helper permissions are modelled as `OwnerWriteGrant`s of the helper
  writer class (`c0_2ci_sink_helper_mutation_gate_four_way` exercises it), so
  `outstanding_owner_writes != 0` *is* the check. It is already census survivor
  ~464; not double-counted here.
- The rest of §4 maps to guards that exist and are caught: the eligibility
  conjunction (`can_destroy`, and `is_resource_releasable`'s allocation and
  `kms_obligations` branches); `PriorBufferReleased` satisfying only the matching
  obligation (`record_kms_discharged`, keyed by full `GroupMember`); no proof
  fabricated from timeout or reference drop (private `apply_validated_proof`,
  quarantine on expiry); page-flip handling unable to mark storage reusable
  (the F-4d/F-13c scene gates).
- R6 in `commit.rs` was already worked through clause by clause by the F-14 and
  F-15 reviews; not re-derived.

## Census boundary — the author's error, the largest finding here

| File | Refusal points |
| --- | --- |
| `mod.rs` | **48** |
| `transport.rs` | 19 |
| `commit.rs` | 14 |
| `gpu.rs` | 5 |

`mod.rs` holds **more refusal points than the three censused files combined
(38)**. It is the ledger core and most of the GPU batch state machine —
`validate_gpu_batch`, `poll_gpu`, `apply_validated_proof`, `cancel`,
`apply_teardown_release`, `record_kms_discharged`,
`service_ready_with_registry`. The census was scoped to the three files the
author had named in the F-14 review's recommendation, without checking where
the guards actually live. That is why `gpu.rs` showed only five: the state
machine the name suggests is in `mod.rs`. The first census therefore left the
largest refusal surface — the one that implements R9 — unmeasured.

## `mod.rs` census result (2026-09-16)

Same method as the first census; the recreated tool enumerates exactly the
same 12 + 3 + 18 = 33 sites over `commit/gpu/transport`, so the numbers are
comparable. `mod.rs`: **34 sites, 15 caught, 19 survivors (56%).** No site
needed a hand-written mutation.

**Combined, all four files: 67 guard sites, 32 caught, 35 survivors — 52% of
the resource-service refusal surface is unproven.**

The `mod.rs` survivors, by mechanism (identity checks overlap the read path,
so the groups do not sum naively):

- **Cross-incarnation isolation — 10 of 12 identity checks survive.** Every
  `key.device != self.device || key.incarnation != self.incarnation` guard in
  `register`, `freeze`, `cancel`, `validate_proof_target`,
  `record_kms_discharged`, `apply_teardown_release` (two, one on the proof's
  incarnation) and `validate_gpu_batch` (three). The two caught ones —
  `reserve` and `apply_validated_proof` — are killed by a single test,
  `c0_2ci_stale_key_and_old_evidence_rejected`. The test pattern exists; it
  was never extended to the other ten entry points. No test presents a
  wrong-device or wrong-incarnation key to most of the ledger core, although
  R9's "stale evidence cannot release a newer allocation" rests on exactly
  these guards.
- **The read-obligation path in `validate_gpu_batch` — all 6 guards
  survive.** The function has three parallel check sets (identity, `frozen`,
  pending obligation): one for the GPU obligation's entries, one for the read
  obligation's source, one for its staging lease. Only the first set has any
  coverage. A GPU batch carrying a read obligation is validated by nothing
  proven.
- **Exhaustion — 3.** `adopt_unchecked`, `reserve` and `register` each refuse
  once the service is exhausted; none is proven.
- **`adopt` refusing a file-owned payload — 1** (F2-M1's guard: a payload with
  a live file-owned alias must go through `adopt_with_registry`).
- **`apply_teardown_release` requiring the entry frozen — 1.**

### Correction to round-1 review F14-M2

F14-M2 asked for "the frozen-entry refusal in `validate_gpu_batch`", singular.
There are three sibling `frozen` checks. F-15 closed the one the finding
named, correctly; the other two survive. The finding was under-scoped by its
author, not mis-implemented.

### A systemic pattern, named

This is the third instance of the same shape across two days:

| Finding | Guard proven on | Siblings left unproven |
| --- | --- | --- |
| F14-m1 | 1 cancelling path | 2 |
| F14-M2 | 1 `frozen` check set | 2 |
| `mod.rs` census | 2 identity entry points | 10 |

**Tests in this stage prove one instance of a guard and leave its siblings
unproven**, and sampling reviews found the siblings one at a time. The
invariant any fix must meet is per *family*: every ledger entry point that
accepts an `AllocationKey` refuses a foreign one; every check set in
`validate_gpu_batch` refuses a frozen entry; and so on — with each sibling's
deletion failing a named test.
