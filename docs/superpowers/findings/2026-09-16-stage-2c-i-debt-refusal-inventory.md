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

A census of `mod.rs` with the same method (34 enumerated sites) was started on
2026-09-16; its result is appended below when it completes.
