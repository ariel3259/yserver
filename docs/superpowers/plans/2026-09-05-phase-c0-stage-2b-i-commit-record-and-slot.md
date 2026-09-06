# Phase C.0 Stage 2b-i — The commit record and the device slot

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every KMS device an owner that computes a request's exact CRTC closure, installs a commit record that owns both possible resource states, reserves the one device slot before IPC, and drives that record to exactly one terminal state from the executor's typed outcome.

**Architecture:** Stage 2a completed the executor: it carries a real atomic request, returns its out-fences, and never blocks the core. It has no caller — the backend logs its `HostCallEvent`s and tees them into a test queue. This sub-stage builds the caller. Four things in order: a pure closure computation over one serialized property list that decides which CRTCs a request affects and which of them owe completion evidence; a commit record that owns its resources through a type-state ledger whose transitions consume `self`; a single device slot that a `Submitting` record reserves before IPC and that `ValidationOnly` never takes; and one typed `OwnerEvent` stream that is the owner's only output. The record ends at exactly one of `Completed`, `FailedBeforeSubmit` or `CompletionUnknown`, decided only by evidence this sub-stage can actually observe: the executor reply.

**Tech Stack:** Rust (stable toolchain), `libc`. No new dependency. The owner is pure state plus the 2a executor API; it performs no ioctl of its own.

**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md` (Approved, revision 2). This plan implements the first half of section 18 **stage 2b**: request construction and the atomic CRTC closure, commit records with their owned resource ledger, and the single device slot with its typed outcome stream. The second half — the epoch-local clock record and its probe, page-event correlation and MSC/UST normalization, the completion deadlines and the qualification gate — is **2b-ii** and is not started here.

**Predecessor:** `2026-09-04-phase-c0-stage-2a-executor-substrate.md`, complete at `ddb86da4`, which includes two race repairs and one lying test helper found while reviewing it.

**Revision 3, after two adversarial reviews.** Round 2 returned 10 blocking, 4 major and 1 minor (`2026-09-05-phase-c0-stage-2b-i-plan-review-round2.md`), down from 14/5/1, and **every one is resolved here** — see "Round-2 finding disposition". Two of its blockers were defects revision 2 introduced: the "one source of truth" redesign forced an `ACTIVE` property onto every closure CRTC, which `spec:551-556` forbids because the persistent list is minimal; and `Rejected<R>` retained the still-current *old* state and then let the record drop it, which would destroy an in-use framebuffer, BO or pin. Power is now retained metadata cross-checked against the wire, and old state is handed back as still-current at retirement.

**Revision 2, after one adversarial review.** The draft returned 14 blocking, 5 major and 1 minor at `docs/superpowers/findings/2026-09-05-phase-c0-stage-2b-i-plan-adversarial-review.md`. **Every one is resolved here.** Ten of the fourteen were design defects no compiler could have found, which is the opposite of stage 2a's round 4 and the reason this round was folded in whole rather than sampled.

**What changed structurally, and why.** Four blockers (B-1, B-2, B-3, and half of M-1) were consequences of one mistake: the draft described a request three times over — `entries`, `power` and `serialized` — then tried to verify the copies against each other. Revision 2 gives `CommitDescription` **one source of truth**, the serialized object list, each object carrying its own old/new `CRTC_ID` binding and, for a CRTC, its own old/new `ACTIVE` value. The closure is computed from it and re-checked against it. There is no second description to disagree with, so the spec's equality re-scan (`spec:569-570`) becomes checkable where the draft had to weaken it to containment.

Two further changes come from recorded project decisions the draft contradicted, both reaffirmed by the user: the owner emits **one typed `OwnerEvent` stream** and never a second parallel event type, and `ResourceLedger` **owns resources** through transitions that consume `self`, generic over the resource type so 2c instantiates it with real framebuffers without rewriting it or its call sites.


## Round-2 finding disposition

Every finding from `docs/superpowers/findings/2026-09-05-phase-c0-stage-2b-i-plan-review-round2.md`. Nothing is knowingly outstanding.

| # | Finding | Resolved by |
|---|---|---|
| B-1 | The closure model makes minimal plane/connector requests impossible | `CrtcPower` is retained metadata on `CommitDescription.crtc_state`, not a serialized property; `SerializedObject` no longer carries `old_active` and a CRTC need not serialize `ACTIVE`. Test `a_plane_only_request_needs_no_crtc_property_at_all`. The re-scan's `ActiveContradictsPower` keeps the metadata honest |
| B-2 | Rejection and pre-dispatch refusal destroy the still-current old resources | `Rejected<R>::into_current` hands the old state back at retirement; `retire_live` emits it as `ResourcesStillCurrent` alongside `ResourcesReleased`, and the refusal path takes the same route. The record never drops a resource |
| B-3 | The concrete `KmsResource` does not own the required resources | It is gone. Production instantiates `DeviceCommitOwner<NeverResource>` over an uninhabited enum, because 2b-i converts no call site and owns nothing. The crate has no RAII owner for a framebuffer, BO or pin — `DirectPresentFrame` holds pins as `u64` and `DirectScanoutProbeFramebuffer` (`drm/modeset.rs:1395`) is the only `Drop` — so building them is 2c's conversion work, and the type parameter is the seam |
| B-4 | The validation lease ends before the interval it exists to protect | The lease survives the `TEST_ONLY` reply and is released only by `consume_validation` (at the live call, refusing unless the description matches what was validated) or `abandon_validation`. `acquire_validation` now also refuses while the slot is occupied |
| B-5 | Validation resolution bypasses the full `ID-3` currency check | Resolution compares the whole stored `HostCallCorrelation` for equality, not `CommitId` alone — `CommitId` is device-generation-local and can collide across incarnations |
| B-6 | The "unforgeable" proofs remain publicly forgeable | The false claim is withdrawn. `for_tests` must stay public: stage 2a's integration tests are a separate crate, so no `cfg` gate admits them alone, and closing it needs a dev-dependency cargo feature this stage does not bundle. The accurate claim is one private production issuer and one named seam |
| B-7 | `send_validation_on` is promised but never specified | Specified in full in Task 6: lease into `HostCallReservation::Validation`, lease released on a pre-IPC refusal, correlation retained on `SendError::Ipc`, `AlreadySent` on a second call |
| B-8 | Task 7 references backend interfaces that do not exist | `owner_for` is written out in Task 7; `begin_on_device_for_tests` builds `Submitted::<NeverResource>::new(Vec::new(), Vec::new())` rather than the wrongly-typed `test_ledger`; the `KmsDevice` literal at `backend.rs:24229` is enumerated, with a `grep -rn 'KmsDevice {'` instruction to reconcile against |
| B-9 | The routing test waits on the executor that received no request | New `wait_device_executor_readable_for_tests(backend, index, timeout)`; the original hard-codes `.devices.first()` and is kept for the single-device 2a test |
| B-10 | The shown external integration test does not compile | Its import block now names `Duration`, `UnknownReason`, `DeviceCommitOwner`, `OwnerEvent`, `FailureCause`, `TerminalState` and `UnknownCause` |
| M-1 | Malformed bindings and duplicate persistent properties survive construction | `CrtcIdOutOfRange` replaces the `u64`→`u32` truncation; `DuplicateProperty`, `DuplicateObject` and `DuplicatePower` reject every ambiguous description. Test `a_crtc_id_value_that_does_not_fit_u32_is_refused_not_truncated`, `duplicate_properties_and_duplicate_object_rows_are_refused` |
| M-2 | Consumes/Produces declarations still omit required names | Each task's list was rewritten against its final body, including `terminalize_rejected`, `TEST_PROPERTY_IDS`, the fixtures, `NeverResource`, `HostCallObservation`, `ObservedOutcome` and `owner_for` |
| M-3 | The fd-free observation conversion is invoked but never specified | `HostCallObservation::of(&HostCallEvent)` is written out in Task 7; it borrows, so the event still reaches the owner |


## Round-1 finding disposition

Every finding from `docs/superpowers/findings/2026-09-05-phase-c0-stage-2b-i-plan-adversarial-review.md`, and where it is resolved. Nothing is knowingly outstanding.

| # | Finding | Resolved by |
|---|---|---|
| B-1 | Every active `ValidationOnly` fails its own out-fence re-scan | `FencePolicy::{Required,Forbidden}` in the contract; `verify_serialized` requires emptiness under `Forbidden`; Task 1 step 5 test `a_validation_rescan_requires_no_fences_and_therefore_passes`, Task 5 test `a_validation_request_carries_no_out_fence_and_still_builds` |
| B-2 | The re-scan accepts a different serialized closure (containment, unused `active`) | One source of truth: `SerializedObject` carries `old_crtc_id` and `old_active`, so power is derived from the same objects and equality is demanded against `closure() - old_binding_only()`; Task 1 tests `the_rescan_demands_equality_not_containment`, `the_rescan_subtracts_exactly_the_old_binding_only_members` |
| B-3 | "Exactly one `OUT_FENCE_PTR`" is not enforced | The re-scan counts fences as a multiset (`DuplicateOutFence`), and `compute` refuses a caller-supplied fence (`UnsolicitedOutFence`); Task 1 tests `the_rescan_rejects_a_duplicate_out_fence_on_one_crtc`, `an_out_fence_the_caller_supplied_is_refused` |
| B-4 | The ledger owns no resources and performs no cleanup | Task 2: `Submitted<R>`/`Accepted<R>`/`Rejected<R>`/`Quarantined<R>` own `R` by value; `rejected()` returns the freed set by value; `Quarantined<R>` has no exit; `OwnerEvent::ResourcesReleased` hands ownership to the consumer. Drop-counting test `a_rejection_hands_the_new_state_out_by_value_exactly_once` |
| B-5 | Wrong executor types (`HostCallClass` path, `&AtomicRequest`) | "Using the 2a executor correctly" in the contract; Task 5 imports `HostCallClass` from `kms::executor`; Task 6 wraps in `HostCallRequest::Atomic` |
| B-6 | Move and trait errors (`RecordState` not `Copy`, `IdentityAllocator` not `Debug`) | `RecordState` derives `Copy`; Task 4 step 1 adds `#[derive(Debug)]` to `IdentityAllocator` |
| B-7 | Pre-dispatch refusals falsely marked `Dispatched` and stranded | `RefusalCause` and `FailureCause::NeverDispatched`; Task 6's `send_on` separates `SendError::Ipc` from the five pre-install refusals; test `an_executor_refusal_before_ipc_is_never_dispatched_not_acceptance_unknown` |
| B-8 | Validation lease neither exclusive nor resolvable | `reserve` refuses while a lease is outstanding (`spec:324-325`); `apply_host_call_event` resolves the validation **before** `is_current`; Task 3 test `an_outstanding_validation_lease_blocks_a_new_commit`, Task 6 test `a_validation_resolves_its_own_lease_though_it_has_no_record` |
| B-9 | Partial fence output promoted to `Accepted`; slot→CRTC mapping lost | `UnknownCause::IncompleteFenceOutput`; `FenceEvidence` retains slots and mask; Task 6 test `a_short_out_fence_mask_is_completion_unknown_not_acceptance`, Task 4 test `fence_evidence_maps_each_descriptor_back_to_its_crtc` |
| B-10 | Contradictory outcomes leave a record nonterminal and unquarantined | `ContradictoryEvidence` is produced, not just declared; Task 6 test `a_validation_outcome_under_a_commit_record_is_contradictory_and_terminal` |
| B-11 | `pub(crate)` constructors make proofs forgeable | Both proof types **move** to `owner/slot.rs`; `fn issue()` is private to that module; Task 3 step 3. Grep 5 in Task 7 reads matches instead of asserting a count |
| B-12 | Incarnation-only routing cannot identify a device | `drain_executor_events`/`tick_executors` return `(DrmDeviceKey, HostCallEvent)`; Task 7 test `an_outcome_reaches_the_owner_of_the_device_that_produced_it` uses two devices |
| B-13 | The promised owner API is not specified or produced | Task 6's Produces list matches its body exactly, including `new`, `begin`, `send_on`, `dispatch`, `begin_validation`; `DeviceCommitOwner::new` takes the incarnation, lifecycle epoch and topology generation |
| B-14 | Both integration layers' fixtures are unusable | `owner/test_fixtures.rs` is `#[doc(hidden)] pub`, not `#[cfg(test)]`, so the integration crate reaches it; Task 7 step 3 sets `owner: Some(..)` in the stub-executor platform fixture |
| M-1 | Interface declarations disagree with bodies | Every task's Consumes/Produces was rewritten against its final body; `Quarantine` is gone from the file structure |
| M-2 | The fd-free test tee has no sound representation | `HostCallObservation` / `ObservedOutcome`, a separate type with a specified `of` constructor; neither event type becomes `Clone`, and grep 6 checks it |
| M-3 | The claimed five-second timeout is really thirty | Fixed in the tree, not the plan: `wait_readable` now honors its parameter (commit `ddb86da4`) |
| M-4 | The unknown-reason loop is not exhaustive | `UnknownReason::{COUNT, ALL, index}` with a compile-time index round-trip; Task 6 step 1 |
| M-5 | The proof grep has an impossible expected count | Task 7 grep 5 asserts no count and says what to read for |
| m-1 | Two descriptions of current behavior are inaccurate | The architecture paragraph says the backend "logs and tees"; Task 5's `value_index` test now explains that the helper writes a **pointer to holder storage** into that value and the kernel writes the fd into the holder |

---

## Global Constraints

Copied from the spec. Every task's requirements implicitly include this section.

- **`COMMIT-5`** — the X11 core never executes or waits synchronously for a potentially blocking KMS ioctl. The owner calls `send`/`poll_reply`/`tick` and never `dispatch_blocking_at_boundary` outside cold start or final offline.
- **`COMMIT-6`** — before sending IPC the owner installs a `Submitting` record and reserves the device slot. After send, **only an explicit ioctl rejection proves `FailedBeforeSubmit`**; missing or invalid reply, helper exit, IPC failure and watchdog expiry are acceptance-unknown. No second ioctl may be dispatched on the device while this record or its executor lease exists.
- **`ValidationOnly` (`spec:320-329`)** — `TEST_ONLY` omits `NONBLOCK`, touches no hardware, transfers no live resource ownership, **creates no out-fence**, and does not occupy the submitted-commit slot. A final serialized validation holds an exclusive owner validation lease **so no persistent generation can change before the live call**.
- **`ID-3`** — every executor request and reply carries the lifecycle epoch. A reply is current only when incarnation, lifecycle epoch, optional transition id and commit id all match.
- **Closure (`spec:541-556`)** — `AtomicCrtcClosure` = every CRTC with a persistent CRTC-property entry, union every non-zero old or new `CRTC_ID` binding of each connector or plane having a persistent property entry. `ExpectedCompletionCrtcs` = every CRTC in the closure where `old.active || new.active`.
- **Out-fence placement (`spec:565-568`)** — **exactly one** `OUT_FENCE_PTR` property for every member of `ExpectedCompletionCrtcs` and **none outside it**. Ephemeral out-fence entries may not enlarge `AtomicCrtcClosure`.
- **Off-to-off (`spec:583-595`)** — construction fails before submit for every inactive-to-inactive closure member if the global `PAGE_FLIP_EVENT` flag is set or an out-fence pointer was assigned to that CRTC.
- **Event sets (`spec:571-574`)** — when `PAGE_FLIP_EVENT` is set, `KernelEventCrtcs = ExpectedCompletionCrtcs`; `PresentEventCrtcs` is the subset with a Present consumer.
- **Re-scan (`spec:569-570`)** — the final serialized request is re-scanned before dispatch; if its kernel-visible CRTC closure **differs** from the recorded set, construction fails before submit.
- **Fence completeness (`spec:1955-1962`, `spec:2129`)** — a successful live ioctl must replace every expected holder with a non-negative sync-file fd. `-1` is valid only after a rejected ioctl or `TEST_ONLY`. Live success plus a still-`-1` holder, or partial output, **is not success**: it enters `CompletionUnknown`.
- **`user_data` (`spec:1673-1678`)** — `drm_mode_atomic.user_data` carries the commit's `EventToken` verbatim. Raw CRTC ids are never event identities.
- **Tombstones (`spec:1697-1704`)** — the owner keeps the last 64 identity-only tombstones. They retain kernel-event, Present-event and observed CRTC sets plus terminal state, and own no KMS resource.
- **Identity allocation** — checked increment; never wraps or reuses a token within an incarnation.
- Portable builds must compile on glibc, musl and FreeBSD.
- Format is `cargo +nightly fmt --check`. Tests are `cargo test -p yserver`. Lint is `cargo clippy --all-targets -- -D warnings`, exactly as CI runs it.

### A known pre-existing flake, so it is not mistaken for this work

`cargo test -p yserver --lib` fails about **10-20% of runs** on this tree *before* any change from this plan. Measured 2026-09-05: 3 failures in 30 runs at `25ee0237`, against 6 in 30 after the stage-2a race repairs — a difference that is not statistically significant, so the flake is pre-existing and neither repair caused it.

Three tests are involved: `kms::executor::tests::early_take_reap_proof_returns_none_and_does_not_invalidate_future_proof` and two in `kms::executor::device_lock::tests`. The mechanism is the fork/exec window: `File::open` gives the lock fd `O_CLOEXEC`, so an unrelated `Command::spawn` does not *inherit* it past `exec`, but between `fork` and `exec` the child holds a duplicate of every open descriptor — including a device lock another test thread is about to drop. Same class as commit `752df3d0`, "stabilize executor_lock_handoff tests against concurrent fork races". It is a test-concurrency artifact, not a production defect: nothing in production forks unrelated processes while juggling device locks across threads.

**Therefore: clean full-suite runs are not this stage's gate.** Task 7 says what to require instead. Do not "fix" it by relaxing an assertion — that is what hid stage 2a's two real races. Serializing the fork-heavy tests would be a genuine improvement, but it is a separate change with its own commit, not this stage's work.

### What this sub-stage does not do

- **No production `atomic_commit` call site is converted.** The six live sites stay on the Phase A+B path; conversion is 2c's. The owner is reachable here only through its own API and its tests, exactly as 2a's executor was.
- **No intents, no admission, no fairness, no ordering classes.** Sections 9.1, 9.2 and 9.2.1 are 2c's. The owner accepts a fully-formed commit description and does not choose between two of them.
- **No completion evidence.** Out-fence *slots* are specified because the closure decides them, and a returned fence's *presence* is checked because the reply carries it — but no fence is queried, and `HardwareComplete`, `Presented` and `PriorBufferReleased` are typed milestones nothing here can set.
- **No kernel event handling.** `drm/event_stream.rs` keeps its current callers. Owner-exclusive drain, correlation and MSC/UST normalization are 2b-ii.
- **No clock record and no probe.** `HostCallOutcome::ProbeAccepted` under a *probe* correlation is logged and dropped. The stage-1 `SequenceSupport` map at `kms/render/backend.rs:1044` is **not** moved — `spec:1755-1763` requires it inside 2b-ii's epoch-local clock record. Task 7 greps for it.
- **No recovery, no poison, no quarantine release.** `CompletionUnknown` quarantines the ledger; what *releases* a quarantine is section 10's fd-set barrier, which is stage 3's.
- **No coordinate transport.** The section 7.1 `CoordinateSubmitting` reservation is cursor work and belongs to stage 4; the slot type has no variant for it.

---

## File Structure

**New — `yserver`:**
- `crates/yserver/src/kms/owner/closure.rs` — `ObjectKind`, `SerializedObject`, `PropertyIds`, `FencePolicy`, `AtomicCrtcClosure`, `ClosureError`.
- `crates/yserver/src/kms/owner/ledger.rs` — the generic type-state ledger: `Submitted<R>`, `Accepted<R>`, `Rejected<R>`, `Quarantined<R>`, `LedgerState<R>`.
- `crates/yserver/src/kms/owner/slot.rs` — `DeviceSlot`, `SlotError`, and the **definitions** of `SubmittingProof` and `ValidationLease`, whose issuing constructors are private to this module.
- `crates/yserver/src/kms/owner/record.rs` — `CommitRecord<R>`, `Milestones`, `TerminalState`, `FailureCause`, `RefusalCause`, `UnknownCause`, `RecordState`, `FenceEvidence`, `Tombstone`.
- `crates/yserver/src/kms/owner/build.rs` — `CommitDescription`, `BuildError`, `build_atomic_request`.
- `crates/yserver/src/kms/owner/device.rs` — `DeviceCommitOwner<R>`, `OwnerEvent<R>`, `ValidationOutcome`, `DispatchError`.
- `crates/yserver/src/kms/owner/test_fixtures.rs` — `#[doc(hidden)] pub` descriptions and constructors shared by unit tests, the integration test and the backend fixtures. **Not `#[cfg(test)]`:** an integration-test crate links the library built *without* `cfg(test)`, which is exactly why the draft's fixtures were unreachable.
- `crates/yserver/tests/owner_commit_record.rs` — integration coverage driving a real stub helper through the owner.

**Modified — `yserver`:**
- `crates/yserver/src/kms/owner/mod.rs` — declares the seven new modules.
- `crates/yserver/src/kms/owner/identity.rs:182` — `IdentityAllocator` gains `#[derive(Debug)]`.
- `crates/yserver/src/kms/executor/mod.rs:222-250` — `SubmittingProof` and `ValidationLease` are **imported from `owner::slot`** instead of defined here. `HostCallReservation` and `send` signatures are unchanged.
- `crates/yserver/src/kms/render/platform.rs:1990-2000,3975-3995` — `KmsDevice` carries a `DeviceCommitOwner`; `drain_executor_events` and `tick_executors` return the **device key** with each event.
- `crates/yserver/src/kms/render/backend.rs:14755-14771` — `record_host_call_events` routes each event to its device's owner by key.

**Explicitly out of scope:**
- `crates/yserver/src/drm/page_flip.rs`, `crates/yserver/src/drm/modeset.rs` — the live commit paths. 2c converts them.
- `crates/yserver/src/present/event_loop.rs` — as in stage 1 and 2a: its `run_loop` has no caller in the workspace.

---

## The normative contract

**This section is the single model. Every task implements it; no task re-derives it.** Revision 4 of the 2a plan established why: when a type model is restated per task, a change lands in one and the neighbours keep speaking the old language.

### One description, not three

```rust
// crates/yserver/src/kms/owner/closure.rs

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ObjectKind { Crtc, Connector, Plane }

/// One DRM object's serialized persistent properties, plus the one thing the
/// wire provably cannot show: its binding before this request.
///
/// A serialized atomic request carries only the *new* `CRTC_ID`, so a detach
/// is invisible in the bytes. Recording it here — beside the properties it
/// describes, not in a parallel array — is what lets the re-scan demand
/// equality instead of the containment the first draft settled for.
#[derive(Debug, Clone)]
pub struct SerializedObject {
    pub object: u32,
    pub kind: ObjectKind,
    /// `CRTC_ID` before this request. `None` for a CRTC (its own id is its
    /// binding); `Some(0)` means it was unbound.
    pub old_crtc_id: Option<u32>,
    /// `(property id, value)` in wire order — **the minimal list**. It
    /// contains only what actually changes plus what the kernel requires for
    /// that change (`spec:551-556`). A CRTC appearing here need not carry
    /// `ACTIVE`, and a plane-only request need not name its CRTC at all.
    pub props: Vec<(u32, u64)>,
}

/// Powered state of one CRTC across the request. **Retained metadata, not a
/// property.**
///
/// Revision 2 folded this into `SerializedObject` and required every closure
/// member to carry an `ACTIVE` entry. That made a minimal plane-only request
/// impossible: it would have had to restate an unchanged `ACTIVE` for each
/// bound CRTC purely to satisfy the closure computation, which `spec:551-556`
/// forbids — the persistent list contains only objects whose generation
/// changes. Power is something the owner *knows*; it is not something the
/// request must *say*.
///
/// This is separate from `SerializedObject` and therefore could disagree with
/// it. `verify_serialized` closes that: whenever a CRTC does serialize an
/// `ACTIVE` value, it must equal this row's `new_active`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CrtcPower {
    pub crtc_id: u32,
    pub old_active: bool,
    pub new_active: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PropertyIds { pub crtc_id: u32, pub active: u32, pub out_fence_ptr: u32 }

/// Whether this request's class may carry out-fences at all.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FencePolicy {
    /// Every live class: one `OUT_FENCE_PTR` per `ExpectedCompletionCrtcs`.
    Required,
    /// `ValidationOnly`: none, on any CRTC. `spec:320-323`.
    Forbidden,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AtomicCrtcClosure {
    closure: Vec<u32>,
    /// Members contributed only by an `old_crtc_id`. A serialized list cannot
    /// show these, so the re-scan subtracts them before demanding equality.
    old_binding_only: Vec<u32>,
    expected_completion: Vec<u32>,
    kernel_event: Vec<u32>,
    present_event: Vec<u32>,
}
```

`AtomicCrtcClosure` exposes `closure()`, `old_binding_only()`, `expected_completion()`, `kernel_event()` and `present_event()` as `&[u32]`. Its only constructor is `compute`, so a set cannot be assembled by hand and then disagree with the request it describes.

`compute(objects: &[SerializedObject], power: &[CrtcPower], ids: &PropertyIds, page_flip_event: bool, present_consumers: &[u32]) -> Result<AtomicCrtcClosure, ClosureError>`:

1. **Closure.** Every `Crtc` object's id; every non-zero `old_crtc_id`; every non-zero `CRTC_ID` *value* in a `Connector`/`Plane` object's `props`. Sorted, deduped. `old_binding_only` records the members contributed solely by an `old_crtc_id`.
2. **Powered state.** Each closure member's `CrtcPower` row supplies both values. A member with no row is `ClosureError::UnknownPower(crtc)`; two rows for one CRTC are `ClosureError::DuplicatePower(crtc)`; a row for a CRTC outside the closure is `ClosureError::PowerOutsideClosure(crtc)`. **Nothing is assumed off**, and no `ACTIVE` property is required in order to know.
3. **`ExpectedCompletionCrtcs`** = members where `old_active || new_active`.
4. **Off-to-off with a page event** is `ClosureError::OffToOffWithPageEvent(crtc)`.
5. **`KernelEventCrtcs`** = `expected_completion` when `page_flip_event`, else empty. **`PresentEventCrtcs`** = `present_consumers` ∩ `kernel_event`, deduped and sorted; a consumer outside it is `ClosureError::PresentConsumerOutsideEventSet(crtc)`.
6. **A pre-existing `OUT_FENCE_PTR`** in any object's `props` is `ClosureError::UnsolicitedOutFence(object)`. The builder is the only thing allowed to add one, which is how "exactly one" stays enforceable.
7. **Ambiguous input fails rather than being interpreted.** A `CRTC_ID` value above `u32::MAX` is `ClosureError::CrtcIdOutOfRange(value)` — truncating it would record CRTC 1 while the kernel receives `0x1_0000_0001`. Two `CRTC_ID` entries on one object are `ClosureError::DuplicateProperty { object, prop }`; so are two `ACTIVE` entries on one CRTC. Two rows for the same `object` are `ClosureError::DuplicateObject(object)`. Every one of these is a description that means two different things, and picking the first or the last is how a request quietly stops matching the closure recorded for it.

An empty `expected_completion` is **legal**. It is 2b-ii's qualification gate that refuses to let such a request qualify an incarnation, not this computation.

```rust
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ClosureError {
    #[error("closure member CRTC {0} has no powered-state row")]
    UnknownPower(u32),
    #[error("CRTC {0} has more than one powered-state row")]
    DuplicatePower(u32),
    #[error("powered-state row for CRTC {0} is outside the closure")]
    PowerOutsideClosure(u32),
    #[error("object {0} appears more than once")]
    DuplicateObject(u32),
    #[error("object {object} carries property {prop} more than once")]
    DuplicateProperty { object: u32, prop: u32 },
    #[error("CRTC_ID value {0:#x} does not fit a u32")]
    CrtcIdOutOfRange(u64),
    #[error("object {0} supplied its own OUT_FENCE_PTR")]
    UnsolicitedOutFence(u32),
    #[error("inactive-to-inactive CRTC {0} cannot carry the global page-event flag")]
    OffToOffWithPageEvent(u32),
    #[error("present consumer CRTC {0} is outside the kernel event set")]
    PresentConsumerOutsideEventSet(u32),
    #[error("serialized closure {serialized:?} differs from recorded {recorded:?}")]
    SerializedClosureDiffers { recorded: Vec<u32>, serialized: Vec<u32> },
    #[error("OUT_FENCE_PTR coverage {found:?} differs from expected {expected:?}")]
    OutFenceCoverageDiffers { expected: Vec<u32>, found: Vec<u32> },
    #[error("CRTC {0} carries more than one OUT_FENCE_PTR")]
    DuplicateOutFence(u32),
    #[error("OUT_FENCE_PTR on non-CRTC object {0}")]
    OutFenceOnNonCrtc(u32),
    #[error("CRTC {crtc} serializes ACTIVE={serialized} against recorded {recorded}")]
    ActiveContradictsPower { crtc: u32, serialized: bool, recorded: bool },
    #[error("object {0} in the serialized list has no known kind")]
    UnknownObject(u32),
    #[error("the serialized property list is malformed: {0}")]
    MalformedPropertyList(&'static str),
}
```

### The re-scan

`verify_serialized(&self, props: &AtomicPropertyList, kinds: &BTreeMap<u32, ObjectKind>, ids: &PropertyIds, fences: FencePolicy) -> Result<(), ClosureError>` runs on the bytes about to be sent, **after** the builder appended out-fence properties:

- Validate the parallel-array shape first: `count_props.len() == objects.len()`, `props.len() == values.len()`, and the declared counts summing to the payload length. Otherwise `ClosureError::MalformedPropertyList(&'static str)`.
- Recompute the serialized closure — CRTC objects plus non-zero `CRTC_ID` values — and require it to equal `closure()` **minus** `old_binding_only()` exactly. Anything else is `ClosureError::SerializedClosureDiffers { recorded, serialized }`. This is equality, as `spec:569-570` demands; the subtraction is the one thing a serialized list provably cannot show.
- Collect `OUT_FENCE_PTR` entries as a **multiset**, so a duplicate is visible: a repeat on one CRTC is `ClosureError::DuplicateOutFence(crtc)`. Then require the set to equal `expected_completion` under `FencePolicy::Required`, and to be **empty** under `Forbidden`; a mismatch is `ClosureError::OutFenceCoverageDiffers { expected, found }`.
- **Whenever a CRTC serializes an `ACTIVE` value, it must equal that CRTC's recorded `new_active`**, else `ClosureError::ActiveContradictsPower`. Power is retained metadata rather than a required property, so this is the check that keeps it from drifting away from the request it describes. A CRTC that serializes no `ACTIVE` is checked by nothing here, and correctly so: it is not changing power.

Under `Forbidden` the requirement is emptiness, which is what lets a `ValidationOnly` request pass its own re-scan — the defect that made every active validation in the draft fail construction.

### The ledger: owning, generic, self-consuming

```rust
// crates/yserver/src/kms/owner/ledger.rs

/// Between dispatch and a typed outcome. Owns both possible states;
/// cancellation can no longer classify the request as never-submitted.
#[derive(Debug)]
pub struct Submitted<R> { old: Vec<R>, new: Vec<R> }

/// Accepted: both possible states stay owned until the class-specific
/// replacement rule and `PriorBufferReleased` allow a release — 2b-ii's.
#[derive(Debug)]
pub struct Accepted<R> { old: Vec<R>, new: Vec<R> }

/// An explicit ioctl rejection, or a refusal before any IPC. The new state
/// was never current, so its resources leave the ledger by value. The **old**
/// state is still what the hardware is scanning out, so it must leave too —
/// back to the caller, as still-current — rather than being dropped with the
/// record. Revision 2 kept it inside `Rejected` and then dropped the record,
/// which would destroy an in-use framebuffer, BO or pin (`spec:1929-1939`,
/// `spec:2127-2128`).
#[derive(Debug)]
pub struct Rejected<R> { old: Vec<R> }

impl<R> Rejected<R> {
    /// The old state, handed back as still current. Called exactly once, when
    /// the record retires.
    pub fn into_current(self) -> Vec<R> { self.old }
}

/// Acceptance neither established nor disproved. Both sets are held until
/// section 10's teardown barrier, which is stage 3's.
#[derive(Debug)]
pub struct Quarantined<R> { held: Vec<R> }

impl<R> Submitted<R> {
    pub fn new(old: Vec<R>, new: Vec<R>) -> Self { Self { old, new } }
    pub fn accepted(self) -> Accepted<R> { Accepted { old: self.old, new: self.new } }
    /// The released new-state resources leave **by value**: the caller owns
    /// them and the ledger cannot hand them out a second time. The old state
    /// stays in `Rejected` until `into_current` hands it back at retirement —
    /// it is still current and must outlive the record.
    pub fn rejected(self) -> (Rejected<R>, Vec<R>) { (Rejected { old: self.old }, self.new) }
    pub fn unknown(self) -> Quarantined<R> {
        let mut held = self.old;
        held.extend(self.new);
        Quarantined { held }
    }
}

impl<R> Accepted<R> {
    pub fn unknown(self) -> Quarantined<R> {
        let mut held = self.old;
        held.extend(self.new);
        Quarantined { held }
    }
}

impl<R> Quarantined<R> { pub fn held(&self) -> &[R] { &self.held } }

#[derive(Debug)]
pub enum LedgerState<R> {
    Submitted(Submitted<R>),
    Accepted(Accepted<R>),
    Rejected(Rejected<R>),
    Quarantined(Quarantined<R>),
    /// Only observable if a transition panicked mid-move. Every read treats
    /// it as quarantined: a ledger whose state is unknown releases nothing.
    Poisoned,
}
```

There is **no transition out of `Quarantined`**. That is how `spec:2205-2210`'s accepted-stale rule becomes a type rather than an `if`: a later explicit result cannot release a resource whose reachability was never disproved, because no such method exists.

**What `R` is in production, and why it is uninhabited here.** `spec:2127-2132` requires the record to own every old/new framebuffer, blob, BO, pin, descriptor and external-ownership state. This crate has no RAII owner for any of them: `DirectPresentFrame` holds `source_pin: u64` and `fallback_target_pin: u64` by identifier, and the only `impl Drop` in the KMS resource path is `DirectScanoutProbeFramebuffer` (`drm/modeset.rs:1395`). Building those owners is the conversion work of 2c, which is where the first producer appears.

So 2b-i instantiates the production owner as `DeviceCommitOwner<NeverResource>` over

```rust
/// Uninhabited on purpose. **2b-i converts no call site, so it owns no KMS
/// resource** — `Vec<NeverResource>` is provably empty and every ledger
/// transition is trivially correct. 2c replaces this parameter with an enum
/// whose variants own real RAII guards, and no code in this sub-stage
/// changes when it does. That is what the type parameter is for.
///
/// Naming a handle-shaped placeholder instead would claim an ownership this
/// sub-stage cannot deliver, which is exactly the contract violation the
/// generic exists to avoid.
#[derive(Debug)]
pub enum NeverResource {}
```

The ledger's own tests instantiate `R` with a drop-counting `Tracked`, so the transitions are exercised over a type that can actually observe destruction.

Transitions on a record use `std::mem::replace(&mut self.ledger, LedgerState::Poisoned)`, consume the extracted value and install the result. `R` is a plain type parameter: this sub-stage's tests instantiate it with a small `TestResource`, and **2c instantiates it with real framebuffer, BO and pin handles without editing this module or any call site**. That is why it is generic now rather than later.

### The record

```rust
// crates/yserver/src/kms/owner/record.rs

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct Milestones {
    pub producer_ready: bool,
    pub dispatched: bool,
    pub accepted: bool,
    pub hardware_complete: bool,     // 2b-ii
    pub presented: bool,             // 2b-ii
    pub prior_buffer_released: bool, // 2b-ii
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TerminalState {
    Completed,
    FailedBeforeSubmit(FailureCause),
    CompletionUnknown(UnknownCause),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FailureCause {
    /// Cancelled, or refused by the executor, before any IPC crossed the
    /// uncertainty boundary.
    NeverDispatched(RefusalCause),
    /// An explicit ioctl rejection — the only post-dispatch proof of
    /// `FailedBeforeSubmit` that `COMMIT-6` permits.
    IoctlRejected { errno: i32 },
}

/// Why the executor refused *before* installing `InFlight`. Each maps to a
/// `SendError` that `executor/mod.rs:665-692` returns before it writes
/// anything, so no IPC occurred and nothing is uncertain.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RefusalCause { Reaped, Stalled, AlreadyInFlight, ReservationMismatch, BoundaryViolation }

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UnknownCause {
    HostCall(UnknownReason),
    /// Live success whose out-fence output is short of
    /// `ExpectedCompletionCrtcs`. `spec:1955-1962`, `spec:2129`.
    IncompleteFenceOutput { expected: usize, returned: usize },
    /// An outcome whose shape contradicts the request class.
    ContradictoryEvidence,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RecordState { Submitting, Terminal(TerminalState) }
```

`RecordState` is `Copy`, which is what lets `tombstone(&self)` read it out of a shared reference; the draft's version could not compile.

**`FenceEvidence`** is what an `Accepted` outcome leaves for 2b-ii:

```rust
#[derive(Debug)]
pub struct FenceEvidence {
    /// The slot table the request was built with, in slot order.
    slots: Vec<OutFenceSlot>,
    /// Bit *i* set means slot *i* produced a descriptor. The helper sets it
    /// only when holder *i* came back non-negative (`helper.rs:263-270`).
    mask: u32,
    /// One descriptor per set bit, in ascending bit order.
    fences: Vec<OwnedFd>,
}

impl FenceEvidence {
    /// `(crtc_id, fd)` pairs. Without the slot table this mapping is lost and
    /// 2b-ii cannot tell which CRTC a descriptor proves.
    pub fn by_crtc(&self) -> Vec<(u32, BorrowedFd<'_>)>;
    pub fn returned(&self) -> usize { self.mask.count_ones() as usize }
}
```

`CommitRecord<R>` holds the identities (`CommitId`, `EventToken`, `IncarnationId`, `LifecycleEpochId`, `Option<LifecycleTransitionId>`, `topology_generation: u64`), the `AtomicCrtcClosure`, the `HostCallCorrelation`, `Milestones`, `LedgerState<R>`, the observed-CRTC set (empty here; 2b-ii fills it), `Option<FenceEvidence>`, the built-but-unsent `Option<(HostCallRequest, SubmittingProof)>`, and `RecordState`.

### The typed outcome stream

The owner's **only** output. There is never a second parallel event type.

```rust
// crates/yserver/src/kms/owner/device.rs

#[derive(Debug)]
pub enum OwnerEvent<R> {
    Dispatched { commit: CommitId },
    Accepted { commit: CommitId },
    Terminal { commit: CommitId, terminal: TerminalState },
    /// Resources proven never-current, handed over **by value**. The receiver
    /// owns them; the ledger cannot yield them twice.
    ResourcesReleased { commit: CommitId, resources: Vec<R> },
    /// The old state, handed back **by value** as still current, when a record
    /// retires without its new state ever becoming current. The receiver must
    /// keep these alive: the hardware is still scanning them out. Dropping the
    /// record instead — which is what revision 2 did — destroys an in-use
    /// framebuffer, BO or pin (`spec:1929-1939`, `spec:2127-2128`).
    ResourcesStillCurrent { commit: CommitId, resources: Vec<R> },
    Quarantined { commit: CommitId },
    ValidationResolved { commit: CommitId, outcome: ValidationOutcome },
    /// An accepted-stale or uncorrelated reply. Its descriptors were adopted
    /// and closed exactly once.
    StaleReply { correlation: HostCallCorrelation },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ValidationOutcome { Passed, Rejected { errno: i32 }, Abandoned(UnknownReason) }
```

`apply_host_call_event(&mut self, event: HostCallEvent) -> Vec<OwnerEvent<R>>`; `begin`, `send_on` and `begin_validation` return theirs the same way.

### Terminal classification

The whole of `COMMIT-6` for this sub-stage. The outcome stream implements exactly this table:

| Outcome, under the live commit correlation | Record outcome | Slot |
| --- | --- | --- |
| `Accepted`, `mask.count_ones() == expected_completion.len()` | `accepted = true`, `FenceEvidence` retained, stays `Submitting` | held |
| `Accepted`, **short** mask | `Terminal(CompletionUnknown(IncompleteFenceOutput))`, ledger quarantined, fds retained in quarantine | **held** |
| `Rejected { errno }` | `Terminal(FailedBeforeSubmit(IoctlRejected))`, ledger → `Rejected`, new resources emitted as `ResourcesReleased` | released |
| `Unknown(reason)` | `Terminal(CompletionUnknown(HostCall(reason)))`, ledger quarantined | **held** |
| `ValidationAbandoned` or `ProbeAccepted` | `Terminal(CompletionUnknown(ContradictoryEvidence))`, ledger quarantined | **held** |

| Outcome, otherwise | Handling |
| --- | --- |
| any, under the outstanding **validation** correlation | no record; lease released; `ValidationResolved` |
| `ProbeAccepted`/`ProbeRejected` under a **probe** correlation | logged and dropped; 2b-ii is its consumer |
| any `LateReply`, or any correlation that is not current | fds adopted and closed; `StaleReply` |

Four rows are load-bearing, and each was a blocking finding in the draft:

- **A short mask is not success.** The helper sets bit *i* only when holder *i* came back non-negative, and the executor's two consistency checks (`mod.rs:781-789`) only prove the fd count matches the mask — a mask narrower than the slot table passes them both. `spec:2129` is explicit that a holder still at `-1` after live success is missing completion evidence. Deciding this needs only the reply, so it belongs here even though *querying* a fence belongs to 2b-ii.
- **An unknown outcome holds the slot.** `COMMIT-6` forbids a second ioctl while acceptance is unproven, and none of these outcomes proves or disproves it.
- **A contradictory outcome is terminal, not a log line.** A validation or probe result arriving under a live commit correlation means correlation is unreliable; leaving the record `Submitting` would strand it with no future event. `spec:612-617` requires any dispatched result that is neither an explicit rejection nor a normally consumed success to become `CompletionUnknown`.
- **`Accepted` is not terminal.** Section 6.3 requires successful out-fence *status* for every C.0 class, which this sub-stage cannot query. An accepted record stays `Submitting` and the slot stays held. That is the honest state, not a stall to work around; Task 6's test and Task 7's grep both pin it.

**Correlation currency (`ID-3`).** `is_current` compares incarnation, lifecycle epoch, optional transition id **and** commit id. It is consulted **after** the correlation has been matched against the outstanding validation and against the probe family, because a validation deliberately has no record and would otherwise be rejected as uncorrelated and never release its lease.

### The slot and its proofs

```rust
// crates/yserver/src/kms/owner/slot.rs

/// Linear proof that the one device slot was reserved for this commit.
///
/// Defined **here**, not in `executor/`, and its issuing constructor `issue`
/// is private to this module — so no other module can mint one *through the
/// production path*.
///
/// **This is not an absolute guarantee, and the plan does not claim one.**
/// `for_tests` below is `pub`, because stage 2a's integration tests construct
/// proofs from a separate crate (`tests/executor_async.rs` uses it throughout)
/// and an integration-test crate links the library built without `cfg(test)`,
/// so no `cfg` gate can admit them and exclude everyone else. A cargo feature
/// enabled only by dev-dependencies would close it; that is a workspace-wide
/// change and is deliberately not bundled into this stage. What is true is:
/// one private issuer, one named public seam, and Task 7's grep to read.
#[derive(Debug)]
pub struct SubmittingProof(());

/// Linear proof of the exclusive owner validation lease.
#[derive(Debug)]
pub struct ValidationLease(());

impl SubmittingProof {
    fn issue() -> Self { Self(()) }
    /// The one deliberate seam: stage 2a's executor tests construct a proof
    /// with no owner in play. Task 7's grep bounds its use.
    #[doc(hidden)]
    pub const fn for_tests() -> Self { Self(()) }
}
// `ValidationLease` mirrors it exactly.

#[derive(Debug, Default)]
pub struct DeviceSlot { occupant: Option<CommitId>, validation: Option<CommitId> }

#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SlotError {
    #[error("the device slot is already held by commit {0:?}")]
    AlreadyOccupied(CommitId),
    #[error("an exclusive validation lease is outstanding for commit {0:?}")]
    ValidationOutstanding(CommitId),
    #[error("the device slot is not held by commit {0:?}")]
    NotHeld(CommitId),
    #[error("no validation lease is outstanding for commit {0:?}")]
    NoValidationLease(CommitId),
}
```

`reserve(commit)` refuses while a validation lease is outstanding, and `acquire_validation` refuses while the slot is occupied. Both directions follow from `spec:305-325`: the lease exists *"so no persistent generation can change before the live call"*, so neither a new commit during a lease nor a lease during an unresolved commit is admissible. `TEST_ONLY` not occupying the submitted-commit slot is a statement about which slot it takes, not a licence to run alongside arbitrary work.

**The lease outlives the `TEST_ONLY` reply.** Revision 2 released it as soon as the validation outcome arrived, which ends the exclusive interval at exactly the moment it is supposed to begin protecting: the gap between a passed validation and the live call it validated. The lease is released by exactly one of

- `DeviceSlot::consume_validation(commit) -> Result<SubmittingProof, SlotError>`, which releases the lease and reserves the slot in one step, so no window exists between them; or
- `DeviceSlot::abandon_validation(commit)`, for a failed or abandoned validation, or a caller that decides not to proceed.

The slot never learns what a description is. **The owner** performs the equality check before calling `consume_validation`, refusing with `DispatchError::ValidationDoesNotMatch` unless the new description serializes identically to the validated one — otherwise the lease would certify a request nobody checked.

So `DeviceSlot` records `validation: Option<CommitId>` and the owner records the validated description beside it. `begin` on a device with an outstanding lease is refused; `begin_validated` is the path that consumes one.

**Resolution uses the whole `ID-3` tuple.** A validation is matched by comparing the stored `HostCallCorrelation` for equality — incarnation, lifecycle epoch, transition, commit id, sequence and event token — not by `CommitId` alone. `CommitId` is device-generation-local, so a stale reply from another incarnation can collide on it, and matching on it alone would let that reply release the current owner's lease.

`executor/mod.rs` imports both proof types from `crate::kms::owner::slot`; `HostCallReservation` and `send` are otherwise unchanged.

### Using the 2a executor correctly

Three signatures the draft got wrong. They are stated once, here:

- `HostCallClass` is defined in **`kms::executor`** (`mod.rs:167`), not in `kms::executor::protocol`.
- `KmsIoExecutor::send` takes **`&HostCallRequest`** (`mod.rs:660-664`). An `AtomicRequest` must be wrapped: `HostCallRequest::Atomic(request)`.
- `RequestSeq::from_raw` is the production constructor; `for_tests` is not.

---

## Task 1: The atomic CRTC closure and its re-scan

**Files:**
- Create: `crates/yserver/src/kms/owner/closure.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: `AtomicPropertyList` from `kms::executor::protocol`.
- Produces: `ObjectKind`, `SerializedObject`, `PropertyIds`, `FencePolicy`, `ClosureError`, `AtomicCrtcClosure`, and `AtomicCrtcClosure::{compute, closure, old_binding_only, expected_completion, kernel_event, present_event, verify_serialized}`.

- [ ] **Step 1: Write the failing closure tests**

```rust
// crates/yserver/src/kms/owner/closure.rs  (#[cfg(test)] mod tests)

const IDS: PropertyIds = PropertyIds { crtc_id: 20, active: 21, out_fence_ptr: 22 };

/// A CRTC that serializes its own `ACTIVE` — an enable, disable or modeset.
fn crtc(id: u32, new_active: bool) -> SerializedObject {
    SerializedObject {
        object: id,
        kind: ObjectKind::Crtc,
        old_crtc_id: None,
        props: vec![(IDS.active, u64::from(new_active))],
    }
}
fn plane(id: u32, old: u32, new: u32) -> SerializedObject {
    SerializedObject {
        object: id,
        kind: ObjectKind::Plane,
        old_crtc_id: Some(old),
        props: vec![(IDS.crtc_id, u64::from(new))],
    }
}
/// Retained metadata, never serialized.
fn pw(id: u32, old: bool, new: bool) -> CrtcPower {
    CrtcPower { crtc_id: id, old_active: old, new_active: new }
}

#[test]
fn a_plane_only_request_needs_no_crtc_property_at_all() {
    // spec:551-556 — the persistent list is minimal. A plane move must not
    // have to restate an unchanged ACTIVE for its bound CRTCs just to let the
    // closure be computed. Revision 2 required exactly that; this is the
    // regression guard.
    let c = AtomicCrtcClosure::compute(
        &[plane(31, 1, 2)],
        &[pw(1, true, true), pw(2, true, true)],
        &IDS, false, &[],
    ).expect("closure");
    assert_eq!(c.closure(), &[1, 2]);
    assert_eq!(c.expected_completion(), &[1, 2]);
    assert_eq!(c.old_binding_only(), &[1], "CRTC 1 appears only as an old binding");
}

#[test]
fn a_power_row_for_a_crtc_outside_the_closure_is_refused() {
    let err = AtomicCrtcClosure::compute(
        &[crtc(1, true)], &[pw(1, true, true), pw(7, true, true)], &IDS, false, &[],
    ).expect_err("must refuse");
    assert_eq!(err, ClosureError::PowerOutsideClosure(7));
}

#[test]
fn a_crtc_id_value_that_does_not_fit_u32_is_refused_not_truncated() {
    // Truncation would record CRTC 1 while the kernel receives
    // 0x1_0000_0001 — the closure would describe a different request than
    // the one being sent.
    let mut p = plane(31, 0, 1);
    p.props = vec![(IDS.crtc_id, 0x1_0000_0001)];
    let err = AtomicCrtcClosure::compute(&[p], &[pw(1, true, true)], &IDS, false, &[])
        .expect_err("must refuse");
    assert_eq!(err, ClosureError::CrtcIdOutOfRange(0x1_0000_0001));
}

#[test]
fn duplicate_properties_and_duplicate_object_rows_are_refused() {
    let mut c1 = crtc(1, true);
    c1.props.push((IDS.active, 0));
    assert_eq!(
        AtomicCrtcClosure::compute(&[c1], &[pw(1, true, true)], &IDS, false, &[])
            .expect_err("must refuse"),
        ClosureError::DuplicateProperty { object: 1, prop: IDS.active }
    );
    assert_eq!(
        AtomicCrtcClosure::compute(
            &[crtc(1, true), crtc(1, false)], &[pw(1, true, true)], &IDS, false, &[],
        ).expect_err("must refuse"),
        ClosureError::DuplicateObject(1)
    );
}

#[test]
fn a_plane_move_includes_both_powered_endpoints() {
    // spec:557-560 — detach retains the old CRTC, attach retains the new one.
    let c = AtomicCrtcClosure::compute(
        &[crtc(1, true), crtc(2, true), plane(31, 1, 2)],
        &[pw(1, true, true), pw(2, true, true)], &IDS, false, &[],
    ).expect("closure");
    assert_eq!(c.closure(), &[1, 2]);
    assert_eq!(c.expected_completion(), &[1, 2]);
    assert!(c.old_binding_only().is_empty(), "both CRTCs also appear as objects");
}

#[test]
fn a_detach_records_the_old_endpoint_as_serialization_invisible() {
    // The old binding is the one thing the wire cannot show. It must be a
    // closure member and must be listed so the re-scan can subtract it.
    let c = AtomicCrtcClosure::compute(
        &[crtc(2, true), plane(31, 1, 2)],
        &[pw(2, true, true)], &IDS, false, &[],
    ).expect_err("CRTC 1 has no power row");
    assert_eq!(c, ClosureError::UnknownPower(1));

    let c = AtomicCrtcClosure::compute(
        &[crtc(1, false), crtc(2, true), plane(31, 1, 2)],
        &[pw(1, true, false), pw(2, true, true)], &IDS, false, &[],
    ).expect("closure");
    assert_eq!(c.closure(), &[1, 2]);
}

#[test]
fn an_unbound_endpoint_is_not_a_closure_member() {
    let c = AtomicCrtcClosure::compute(
        &[crtc(2, true), plane(31, 0, 2)], &[pw(2, false, true)], &IDS, false, &[],
    ).expect("closure");
    assert_eq!(c.closure(), &[2]);
}

#[test]
fn a_disable_still_owes_completion_evidence() {
    // spec:1873-1876 — never empty merely because a disable makes
    // new.active false.
    let c = AtomicCrtcClosure::compute(&[crtc(1, false)], &[pw(1, true, false)], &IDS, false, &[])
        .expect("closure");
    assert_eq!(c.expected_completion(), &[1]);
}

#[test]
fn an_inactive_to_inactive_member_owes_nothing_and_is_not_an_error() {
    let c = AtomicCrtcClosure::compute(&[crtc(1, false)], &[pw(1, false, false)], &IDS, false, &[])
        .expect("closure");
    assert_eq!(c.closure(), &[1]);
    assert!(c.expected_completion().is_empty());
}

#[test]
fn an_off_to_off_member_with_a_page_event_fails_construction() {
    // spec:583-591 — prepare_signaling() creates event state for every
    // closure member when the global flag is set, and the atomic check then
    // rejects the off-to-off one. Fail here, not at the kernel.
    let err = AtomicCrtcClosure::compute(
        &[crtc(1, true), crtc(2, false)], &[pw(1, true, true), pw(2, false, false)], &IDS, true, &[],
    ).expect_err("must not construct");
    assert_eq!(err, ClosureError::OffToOffWithPageEvent(2));
}

#[test]
fn the_kernel_event_set_is_the_expected_set_only_when_the_flag_is_set() {
    let with = AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, true, &[])
        .expect("closure");
    assert_eq!(with.kernel_event(), &[1]);
    let without = AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
        .expect("closure");
    assert!(without.kernel_event().is_empty());
}

#[test]
fn the_present_set_is_the_consumer_subset_of_the_event_set() {
    // spec:573-574 — events in the set difference are drained but create no
    // protocol completion.
    let c = AtomicCrtcClosure::compute(
        &[crtc(1, true), crtc(2, true)], &[pw(1, true, true), pw(2, true, true)], &IDS, true, &[1],
    ).expect("closure");
    assert_eq!(c.kernel_event(), &[1, 2]);
    assert_eq!(c.present_event(), &[1]);
}

#[test]
fn a_present_consumer_outside_the_event_set_is_rejected_not_dropped() {
    let err = AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, true, &[9])
        .expect_err("a consumer with no event must not construct");
    assert_eq!(err, ClosureError::PresentConsumerOutsideEventSet(9));
}

#[test]
fn a_closure_member_with_no_power_row_is_an_error_not_an_assumption() {
    // Power is retained metadata, so an absent ACTIVE property is fine — an
    // absent power ROW is not, and must never default to off.
    let err = AtomicCrtcClosure::compute(&[crtc(1, true)], &[], &IDS, false, &[])
        .expect_err("an unknown powered state must not default");
    assert_eq!(err, ClosureError::UnknownPower(1));
}

#[test]
fn an_out_fence_the_caller_supplied_is_refused() {
    // The builder is the only thing permitted to add one; that is what makes
    // "exactly one per expected CRTC" enforceable at all.
    let mut c1 = crtc(1, true);
    c1.props.push((IDS.out_fence_ptr, 0));
    let err = AtomicCrtcClosure::compute(&[c1], &[pw(1, true, true)], &IDS, false, &[])
        .expect_err("a caller-supplied out-fence must not construct");
    assert_eq!(err, ClosureError::UnsolicitedOutFence(1));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::closure`
Expected: FAIL — the module does not exist, so this is a compile error naming `closure`.

- [ ] **Step 3: Write the closure computation**

Write the vocabulary types and `ClosureError` exactly as the contract section gives them, then:

```rust
impl AtomicCrtcClosure {
    pub fn compute(
        objects: &[SerializedObject],
        power_rows: &[CrtcPower],
        ids: &PropertyIds,
        page_flip_event: bool,
        present_consumers: &[u32],
    ) -> Result<Self, ClosureError> {
        let mut power: BTreeMap<u32, CrtcPower> = BTreeMap::new();
        for row in power_rows {
            if power.insert(row.crtc_id, *row).is_some() {
                return Err(ClosureError::DuplicatePower(row.crtc_id));
            }
        }

        let mut from_objects: BTreeSet<u32> = BTreeSet::new();
        let mut from_old_binding: BTreeSet<u32> = BTreeSet::new();
        let mut seen: BTreeSet<u32> = BTreeSet::new();

        for object in objects {
            if !seen.insert(object.object) {
                return Err(ClosureError::DuplicateObject(object.object));
            }
            // Each property may appear at most once on an object: two values
            // for one property mean two different requests, and picking one
            // silently decouples the closure from what is actually sent.
            let mut props_seen: BTreeSet<u32> = BTreeSet::new();
            for (prop, _) in &object.props {
                if *prop == ids.out_fence_ptr {
                    return Err(ClosureError::UnsolicitedOutFence(object.object));
                }
                if !props_seen.insert(*prop) {
                    return Err(ClosureError::DuplicateProperty {
                        object: object.object,
                        prop: *prop,
                    });
                }
            }
            match object.kind {
                ObjectKind::Crtc => {
                    from_objects.insert(object.object);
                    // No ACTIVE is required here. Power is retained metadata,
                    // so the persistent list stays minimal (spec:551-556).
                }
                ObjectKind::Connector | ObjectKind::Plane => {
                    if let Some(old) = object.old_crtc_id.filter(|b| *b != 0) {
                        from_old_binding.insert(old);
                    }
                    for (prop, value) in &object.props {
                        if *prop == ids.crtc_id && *value != 0 {
                            let id = u32::try_from(*value)
                                .map_err(|_| ClosureError::CrtcIdOutOfRange(*value))?;
                            from_objects.insert(id);
                        }
                    }
                }
            }
        }

        let mut closure: Vec<u32> =
            from_objects.union(&from_old_binding).copied().collect();
        closure.sort_unstable();
        let old_binding_only: Vec<u32> = from_old_binding
            .difference(&from_objects)
            .copied()
            .collect();

        for id in power.keys() {
            if !closure.contains(id) {
                return Err(ClosureError::PowerOutsideClosure(*id));
            }
        }

        let mut expected_completion = Vec::new();
        for id in &closure {
            let row = *power.get(id).ok_or(ClosureError::UnknownPower(*id))?;
            if row.old_active || row.new_active {
                expected_completion.push(*id);
            } else if page_flip_event {
                return Err(ClosureError::OffToOffWithPageEvent(*id));
            }
        }

        let kernel_event =
            if page_flip_event { expected_completion.clone() } else { Vec::new() };

        let mut present_event = Vec::new();
        for id in present_consumers {
            if !kernel_event.contains(id) {
                return Err(ClosureError::PresentConsumerOutsideEventSet(*id));
            }
            if !present_event.contains(id) {
                present_event.push(*id);
            }
        }
        present_event.sort_unstable();

        Ok(Self { closure, old_binding_only, expected_completion, kernel_event, present_event })
    }
}
```

Note the ordering: a member reachable *only* through an old binding still needs a power row, because `ExpectedCompletionCrtcs` includes an old-active CRTC. That is why `a_detach_records_the_old_endpoint_as_serialization_invisible` asserts `UnknownPower(1)` for the description that omits CRTC 1 — a detach must describe the CRTC it is detaching from.

Add `#[doc(hidden)] pub mod closure;` to `crates/yserver/src/kms/owner/mod.rs`.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::closure`
Expected: PASS, 11 tests.

- [ ] **Step 5: Write the failing re-scan tests**

```rust
fn kinds() -> BTreeMap<u32, ObjectKind> {
    BTreeMap::from([(1, ObjectKind::Crtc), (2, ObjectKind::Crtc), (31, ObjectKind::Plane)])
}
/// Assemble the four parallel vectors from `(object, &[(prop, value)])`.
fn list(objs: &[(u32, &[(u32, u64)])]) -> AtomicPropertyList {
    let mut l = AtomicPropertyList {
        objects: Vec::new(), count_props: Vec::new(), props: Vec::new(), values: Vec::new(),
    };
    for (object, entries) in objs {
        l.objects.push(*object);
        l.count_props.push(entries.len() as u32);
        for (p, v) in *entries { l.props.push(*p); l.values.push(*v); }
    }
    l
}

#[test]
fn the_rescan_accepts_a_list_matching_the_recorded_closure() {
    let c = AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
        .expect("closure");
    let props = list(&[(1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)])]);
    c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required).expect("matches");
}

#[test]
fn the_rescan_demands_equality_not_containment() {
    // spec:569-570. A CRTC that appeared after the closure was recorded
    // changes what the kernel will touch.
    let c = AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
        .expect("closure");
    let props = list(&[
        (1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)]),
        (2, &[(IDS.active, 1)]),
    ]);
    let err = c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
        .expect_err("must differ");
    assert!(matches!(err, ClosureError::SerializedClosureDiffers { .. }));
}

#[test]
fn the_rescan_subtracts_exactly_the_old_binding_only_members() {
    // A detach's old endpoint cannot appear in the bytes, so equality is
    // demanded against the closure minus those members — not waived.
    let c = AtomicCrtcClosure::compute(
        &[crtc(1, false), crtc(2, true), plane(31, 1, 2)], &[pw(1, true, false), pw(2, true, true)], &IDS, false, &[],
    ).expect("closure");
    assert_eq!(c.closure(), &[1, 2]);
    let props = list(&[
        (1, &[(IDS.active, 0), (IDS.out_fence_ptr, u64::MAX)]),
        (2, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)]),
        (31, &[(IDS.crtc_id, 2)]),
    ]);
    c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required).expect("matches");
}

#[test]
fn the_rescan_rejects_a_duplicate_out_fence_on_one_crtc() {
    // A set would collapse these and pass. spec:564-566 says exactly one.
    let c = AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
        .expect("closure");
    let props = list(&[(
        1,
        &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX), (IDS.out_fence_ptr, u64::MAX)],
    )]);
    let err = c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
        .expect_err("must reject");
    assert_eq!(err, ClosureError::DuplicateOutFence(1));
}

#[test]
fn the_rescan_rejects_an_out_fence_outside_the_expected_set() {
    let c = AtomicCrtcClosure::compute(
        &[crtc(1, true), crtc(2, false)], &[pw(1, true, true), pw(2, false, false)], &IDS, false, &[],
    ).expect("closure");
    assert_eq!(c.expected_completion(), &[1]);
    let props = list(&[
        (1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)]),
        (2, &[(IDS.active, 0), (IDS.out_fence_ptr, u64::MAX)]),
    ]);
    let err = c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
        .expect_err("must reject");
    assert!(matches!(err, ClosureError::OutFenceCoverageDiffers { .. }));
}

#[test]
fn a_validation_rescan_requires_no_fences_and_therefore_passes() {
    // The draft made every active ValidationOnly request fail its own
    // re-scan: it omitted the fences and then demanded them.
    let c = AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
        .expect("closure");
    assert_eq!(c.expected_completion(), &[1]);
    let props = list(&[(1, &[(IDS.active, 1)])]);
    c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Forbidden)
        .expect("a validation carries no fences and that is correct");
}

#[test]
fn a_validation_rescan_rejects_a_fence_that_slipped_in() {
    let c = AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
        .expect("closure");
    let props = list(&[(1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)])]);
    let err = c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Forbidden)
        .expect_err("must reject");
    assert!(matches!(err, ClosureError::OutFenceCoverageDiffers { .. }));
}

#[test]
fn the_rescan_rejects_a_list_whose_counts_do_not_describe_its_values() {
    let c = AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
        .expect("closure");
    let mut props = list(&[(1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)])]);
    props.count_props[0] = 9;
    let err = c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
        .expect_err("must be malformed");
    assert!(matches!(err, ClosureError::MalformedPropertyList(_)));
}

#[test]
fn the_rescan_rejects_an_object_of_unknown_kind() {
    let c = AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
        .expect("closure");
    let props = list(&[
        (1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)]),
        (99, &[(IDS.crtc_id, 1)]),
    ]);
    let err = c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
        .expect_err("must reject");
    assert_eq!(err, ClosureError::UnknownObject(99));
}
```

- [ ] **Step 6: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::closure`
Expected: FAIL with "no method named `verify_serialized`".

- [ ] **Step 7: Write the re-scan**

```rust
impl AtomicCrtcClosure {
    pub fn verify_serialized(
        &self,
        props: &AtomicPropertyList,
        kinds: &BTreeMap<u32, ObjectKind>,
        ids: &PropertyIds,
        fences: FencePolicy,
    ) -> Result<(), ClosureError> {
        if props.count_props.len() != props.objects.len() {
            return Err(ClosureError::MalformedPropertyList("count_props length"));
        }
        if props.props.len() != props.values.len() {
            return Err(ClosureError::MalformedPropertyList("props/values length"));
        }
        let declared: u64 = props.count_props.iter().map(|c| u64::from(*c)).sum();
        if declared != props.props.len() as u64 {
            return Err(ClosureError::MalformedPropertyList("declared count vs payload"));
        }

        let mut serialized: BTreeSet<u32> = BTreeSet::new();
        // A multiset, so a duplicate is visible rather than collapsed.
        let mut fenced: BTreeMap<u32, usize> = BTreeMap::new();
        let mut cursor = 0usize;
        for (index, object) in props.objects.iter().enumerate() {
            let count = props.count_props[index] as usize;
            let kind = *kinds.get(object).ok_or(ClosureError::UnknownObject(*object))?;
            if kind == ObjectKind::Crtc {
                serialized.insert(*object);
            }
            for offset in 0..count {
                let prop = props.props[cursor + offset];
                let value = props.values[cursor + offset];
                if prop == ids.crtc_id
                    && matches!(kind, ObjectKind::Connector | ObjectKind::Plane)
                    && value != 0
                {
                    serialized.insert(value as u32);
                }
                if prop == ids.out_fence_ptr {
                    if kind != ObjectKind::Crtc {
                        return Err(ClosureError::OutFenceOnNonCrtc(*object));
                    }
                    *fenced.entry(*object).or_insert(0) += 1;
                }
            }
            cursor += count;
        }

        let expected_serialized: Vec<u32> = self
            .closure
            .iter()
            .filter(|c| !self.old_binding_only.contains(c))
            .copied()
            .collect();
        let serialized: Vec<u32> = serialized.into_iter().collect();
        if serialized != expected_serialized {
            return Err(ClosureError::SerializedClosureDiffers {
                recorded: expected_serialized,
                serialized,
            });
        }

        if let Some((crtc, _)) = fenced.iter().find(|(_, n)| **n > 1) {
            return Err(ClosureError::DuplicateOutFence(*crtc));
        }
        let found: Vec<u32> = fenced.keys().copied().collect();
        let expected: &[u32] = match fences {
            FencePolicy::Required => self.expected_completion(),
            FencePolicy::Forbidden => &[],
        };
        if found != expected {
            return Err(ClosureError::OutFenceCoverageDiffers {
                expected: expected.to_vec(),
                found,
            });
        }
        Ok(())
    }
}
```

- [ ] **Step 8: Run to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::closure`
Expected: PASS, 20 tests.

- [ ] **Step 9: Commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/owner/closure.rs crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): compute the section 6.3 atomic CRTC closure and re-scan it exactly"
```

---

## Task 2: The owning, generic resource ledger

**Files:**
- Create: `crates/yserver/src/kms/owner/ledger.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `Submitted<R>`, `Accepted<R>`, `Rejected<R>`, `Quarantined<R>`, `LedgerState<R>`, and their transitions.

- [ ] **Step 1: Write the failing ledger tests**

```rust
// crates/yserver/src/kms/owner/ledger.rs  (#[cfg(test)] mod tests)

/// A resource that reports its own destruction, so "exactly once" is
/// observable rather than asserted. 2c substitutes real framebuffers.
#[derive(Debug)]
struct Tracked(Rc<Cell<usize>>);
impl Drop for Tracked {
    fn drop(&mut self) { self.0.set(self.0.get() + 1); }
}

#[test]
fn a_rejection_hands_the_new_state_out_by_value_exactly_once() {
    // spec:2146-2148 — new KMS state is not current after an explicit
    // rejection. Handing it out by value is what makes a second release
    // impossible: the ledger no longer has it.
    let drops = Rc::new(Cell::new(0));
    let ledger = Submitted::new(
        vec![Tracked(Rc::clone(&drops))],
        vec![Tracked(Rc::clone(&drops))],
    );
    let (rejected, released) = ledger.rejected();
    assert_eq!(released.len(), 1);
    assert_eq!(drops.get(), 0, "handing over is not dropping");
    drop(released);
    assert_eq!(drops.get(), 1, "the never-current new state is destroyed");

    // The old state is still what the hardware is scanning out. It leaves by
    // value too; dropping `Rejected` must not destroy it.
    let still_current = rejected.into_current();
    assert_eq!(still_current.len(), 1);
    assert_eq!(drops.get(), 1, "retiring the ledger did not destroy the old state");
    drop(still_current);
    assert_eq!(drops.get(), 2, "it is destroyed only when its new owner drops it");
}

#[test]
fn acceptance_releases_nothing_and_keeps_both_states() {
    // spec:2149-2151 — the pending record owns all possible old/new state
    // after acceptance; release waits for PriorBufferReleased, which is
    // 2b-ii's.
    let drops = Rc::new(Cell::new(0));
    let accepted = Submitted::new(
        vec![Tracked(Rc::clone(&drops))],
        vec![Tracked(Rc::clone(&drops))],
    ).accepted();
    assert_eq!(drops.get(), 0);
    let quarantined = accepted.unknown();
    assert_eq!(quarantined.held().len(), 2, "both states survive into quarantine");
    assert_eq!(drops.get(), 0);
}

#[test]
fn quarantine_holds_both_states_and_offers_no_way_out() {
    // spec:2160-2162. This test is a statement about the API surface: there
    // is no method on Quarantined<R> that yields a resource, so a later
    // explicit result cannot release one. If someone adds one, this stops
    // being true and the reviewer should ask why.
    let drops = Rc::new(Cell::new(0));
    let q = Submitted::new(
        vec![Tracked(Rc::clone(&drops))],
        vec![Tracked(Rc::clone(&drops))],
    ).unknown();
    assert_eq!(q.held().len(), 2);
    assert_eq!(drops.get(), 0);
}

#[test]
fn the_ledger_state_enum_treats_poisoned_as_holding_everything() {
    let state: LedgerState<Tracked> = LedgerState::Poisoned;
    assert!(state.releases_nothing(), "an unknown ledger state releases nothing");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::ledger`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the ledger**

Write the four state types, their transitions and `LedgerState<R>` exactly as the contract section gives them, plus:

```rust
impl<R> LedgerState<R> {
    /// True when nothing may be released from this state. Used by the record
    /// so `Poisoned` is never treated as an opportunity to free something.
    pub fn releases_nothing(&self) -> bool {
        matches!(self, Self::Quarantined(_) | Self::Poisoned | Self::Accepted(_))
    }
}
```

Add `#[doc(hidden)] pub mod ledger;` to `owner/mod.rs`.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::ledger`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/owner/ledger.rs crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): own commit resources through a type-state ledger"
```

---

## Task 3: The single device slot and its reservation proofs

**Files:**
- Create: `crates/yserver/src/kms/owner/slot.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs`, `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: `CommitId` from `kms::owner::identity`. Nothing from the executor: this task is where both proof types are *defined*.
- Produces: `SubmittingProof`, `ValidationLease`, `DeviceSlot`, `SlotError`, and `DeviceSlot::{reserve, release, acquire_validation, consume_validation, abandon_validation, occupant, validation_outstanding}`.

`release_validation` from the earlier revision is gone: a lease now ends by being consumed into the slot or abandoned, and a bare release was what let the exclusive interval end before the call it protects.

This task **moves** `SubmittingProof` and `ValidationLease` out of `executor/mod.rs`. The executor imports them instead. Its `HostCallReservation` enum, its `send` signature and every 2a test keep working unchanged, because only the definition site moves and both types keep their `#[doc(hidden)] pub const fn for_tests()`.

- [ ] **Step 1: Write the failing slot tests**

```rust
// crates/yserver/src/kms/owner/slot.rs  (#[cfg(test)] mod tests)

fn c(n: u64) -> CommitId { CommitId::for_tests(n) }

#[test]
fn one_commit_may_hold_the_device_slot() {
    // spec:1328-1331 — exactly one dispatched-or-submitted live atomic
    // transaction per DRM device, not one per CRTC.
    let mut slot = DeviceSlot::default();
    let _proof = slot.reserve(c(1)).expect("first reservation");
    assert_eq!(slot.reserve(c(2)).expect_err("refused"), SlotError::AlreadyOccupied(c(1)));
}

#[test]
fn the_slot_is_not_released_by_a_stranger_or_by_a_late_result() {
    // spec:1330-1332 — the slot is not released merely because the ioctl
    // result is late. Only its holder releases it, by id.
    let mut slot = DeviceSlot::default();
    let _proof = slot.reserve(c(1)).expect("reserve");
    assert_eq!(slot.release(c(2)).expect_err("refused"), SlotError::NotHeld(c(2)));
    assert_eq!(slot.occupant(), Some(c(1)));
}

#[test]
fn dropping_the_proof_does_not_release_the_slot() {
    // A guard would release on unwind and on every early return. The one
    // thing this slot must never do is free itself because a result was late.
    let mut slot = DeviceSlot::default();
    drop(slot.reserve(c(1)).expect("reserve"));
    assert_eq!(slot.occupant(), Some(c(1)));
}

#[test]
fn validation_does_not_occupy_the_submitted_commit_slot() {
    // spec:322-323 — TEST_ONLY does not occupy the submitted-commit slot.
    let mut slot = DeviceSlot::default();
    let _lease = slot.acquire_validation(c(1)).expect("lease");
    assert_eq!(slot.occupant(), None);
}

#[test]
fn an_outstanding_validation_lease_blocks_a_new_commit() {
    // spec:324-325 — the exclusive lease exists so no persistent generation
    // can change before the live call. Admitting another commit is exactly
    // such a change. The first draft of this plan asserted the opposite.
    let mut slot = DeviceSlot::default();
    let _lease = slot.acquire_validation(c(1)).expect("lease");
    assert_eq!(
        slot.reserve(c(2)).expect_err("refused"),
        SlotError::ValidationOutstanding(c(1))
    );
    slot.abandon_validation(c(1)).expect("abandon");
    let _proof = slot.reserve(c(2)).expect("admissible once the lease is gone");
}

#[test]
fn an_unresolved_commit_blocks_a_new_validation() {
    // The other half of spec:305-325. Revision 2 only blocked the commit; a
    // validation could still begin while a live commit was unresolved and
    // might yet change a persistent generation.
    let mut slot = DeviceSlot::default();
    let _proof = slot.reserve(c(1)).expect("reserve");
    assert_eq!(
        slot.acquire_validation(c(2)).expect_err("refused"),
        SlotError::AlreadyOccupied(c(1))
    );
}

#[test]
fn consuming_a_lease_takes_the_slot_with_no_window_in_between() {
    // The exclusive interval ends by becoming the live commit, not by
    // releasing and hoping to re-reserve.
    let mut slot = DeviceSlot::default();
    let _lease = slot.acquire_validation(c(1)).expect("lease");
    let _proof = slot.consume_validation(c(1), c(2)).expect("consume");
    assert_eq!(slot.validation_outstanding(), None);
    assert_eq!(
        slot.occupant(),
        Some(c(2)),
        "the LIVE commit takes the slot; the lease's id was a different allocation"
    );
}

#[test]
fn abandoning_a_lease_leaves_the_device_admissible() {
    let mut slot = DeviceSlot::default();
    let _lease = slot.acquire_validation(c(1)).expect("lease");
    slot.abandon_validation(c(1)).expect("abandon");
    assert_eq!(slot.validation_outstanding(), None);
    let _proof = slot.reserve(c(2)).expect("admissible");
}

#[test]
fn a_stranger_can_neither_consume_nor_abandon_a_lease() {
    let mut slot = DeviceSlot::default();
    let _lease = slot.acquire_validation(c(1)).expect("lease");
    assert_eq!(
        slot.consume_validation(c(2), c(3)).expect_err("refused"),
        SlotError::NoValidationLease(c(2))
    );
    assert_eq!(
        slot.abandon_validation(c(2)).expect_err("refused"),
        SlotError::NoValidationLease(c(2))
    );
    assert_eq!(slot.validation_outstanding(), Some(c(1)));
}

#[test]
fn the_validation_lease_is_exclusive() {
    let mut slot = DeviceSlot::default();
    let _lease = slot.acquire_validation(c(1)).expect("first lease");
    assert_eq!(
        slot.acquire_validation(c(2)).expect_err("refused"),
        SlotError::ValidationOutstanding(c(1))
    );
}

#[test]
fn releasing_and_reserving_again_is_permitted() {
    let mut slot = DeviceSlot::default();
    let _proof = slot.reserve(c(1)).expect("reserve");
    slot.release(c(1)).expect("release");
    assert_eq!(slot.occupant(), None);
    let _proof = slot.reserve(c(2)).expect("re-reserve");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::slot`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Move the proof types and write the slot**

Delete the `SubmittingProof` and `ValidationLease` definitions from `crates/yserver/src/kms/executor/mod.rs` and add there:

```rust
pub use crate::kms::owner::slot::{SubmittingProof, ValidationLease};
```

Write `owner/slot.rs` with both types as the contract section gives them — `fn issue()` private, `for_tests` `#[doc(hidden)] pub` — and:

```rust
impl DeviceSlot {
    pub fn reserve(&mut self, commit: CommitId) -> Result<SubmittingProof, SlotError> {
        if let Some(held) = self.occupant {
            return Err(SlotError::AlreadyOccupied(held));
        }
        if let Some(validating) = self.validation {
            return Err(SlotError::ValidationOutstanding(validating));
        }
        self.occupant = Some(commit);
        Ok(SubmittingProof::issue())
    }

    pub fn release(&mut self, commit: CommitId) -> Result<(), SlotError> {
        match self.occupant {
            Some(held) if held == commit => { self.occupant = None; Ok(()) }
            _ => Err(SlotError::NotHeld(commit)),
        }
    }

    pub fn acquire_validation(&mut self, commit: CommitId) -> Result<ValidationLease, SlotError> {
        if let Some(validating) = self.validation {
            return Err(SlotError::ValidationOutstanding(validating));
        }
        // spec:305-325 — the lease exists so no persistent generation changes
        // before the live call. An unresolved commit may still change one.
        if let Some(held) = self.occupant {
            return Err(SlotError::AlreadyOccupied(held));
        }
        self.validation = Some(commit);
        Ok(ValidationLease::issue())
    }

    /// End the exclusive interval by proceeding to the live call. Releasing
    /// and reserving in one step is the point: two calls would leave a window
    /// in which neither is held and another commit could be admitted.
    /// `lease` is the id the lease was taken under; `commit` is the live
    /// commit that now takes the slot. They differ: a validation and the call
    /// it validated are two allocations, and the slot must end up holding the
    /// one whose record exists.
    pub fn consume_validation(
        &mut self,
        lease: CommitId,
        commit: CommitId,
    ) -> Result<SubmittingProof, SlotError> {
        match self.validation {
            Some(held) if held == lease => {
                if let Some(occupied) = self.occupant {
                    return Err(SlotError::AlreadyOccupied(occupied));
                }
                self.validation = None;
                self.occupant = Some(commit);
                Ok(SubmittingProof::issue())
            }
            _ => Err(SlotError::NoValidationLease(lease)),
        }
    }

    /// End the exclusive interval without proceeding.
    pub fn abandon_validation(&mut self, commit: CommitId) -> Result<(), SlotError> {
        match self.validation {
            Some(held) if held == commit => { self.validation = None; Ok(()) }
            _ => Err(SlotError::NoValidationLease(commit)),
        }
    }

    pub fn occupant(&self) -> Option<CommitId> { self.occupant }
    pub fn validation_outstanding(&self) -> Option<CommitId> { self.validation }
}
```

- [ ] **Step 4: Run the whole suite — this move touches stage 2a's tests**

Run: `cargo test -p yserver`
Expected: every 2a executor test still passes. The move is source-compatible: `HostCallReservation::Submitting(SubmittingProof::for_tests())` resolves through the re-export.

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/owner/slot.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/src/kms/executor/mod.rs
git commit -m "feat(kms): reserve the one device slot behind a single proof issuer"
```

---

## Task 4: The commit record

**Files:**
- Create: `crates/yserver/src/kms/owner/record.rs`
- Modify: `crates/yserver/src/kms/owner/identity.rs:182`, `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: `AtomicCrtcClosure` (Task 1); `LedgerState<R>`, `Submitted<R>` (Task 2); `SubmittingProof` (Task 3); `CommitId`, `EventToken`, `IncarnationId` from `kms::owner::identity`; `LifecycleEpochId`, `LifecycleTransitionId` from `kms::owner::lifecycle`; `HostCallCorrelation`, `HostCallRequest`, `OutFenceSlot` from `kms::executor::protocol`; `UnknownReason` from `kms::executor`; `OwnedFd`, `BorrowedFd` from `std::os::fd`.
- Produces: `Milestones`, `TerminalState`, `FailureCause`, `RefusalCause`, `UnknownCause`, `RecordState`, `FenceEvidence`, `Tombstone`, `CommitRecord<R>`, and `CommitRecord::{new, commit_id, event_token, closure, correlation, milestones, ledger, state, attach_request, take_request, mark_dispatched, mark_accepted, adopt_fences, fence_evidence, terminalize, tombstone}`.

- [ ] **Step 1: Give `IdentityAllocator` a `Debug` impl**

`crates/yserver/src/kms/owner/identity.rs:182` declares `pub struct IdentityAllocator` with no derive. `DeviceCommitOwner` in Task 6 must derive `Debug` and holds one, so add `#[derive(Debug)]` to it now. Nothing else changes.

- [ ] **Step 2: Write the failing record tests**

```rust
// crates/yserver/src/kms/owner/record.rs  (#[cfg(test)] mod tests)

#[derive(Debug, PartialEq)]
struct TestResource(u32);

fn record() -> CommitRecord<TestResource> { /* see fixtures below */ }

#[test]
fn a_new_record_is_submitting_and_already_producer_ready() {
    // spec:2114-2118 — a record exists only after every source dependency
    // completed; a thing that could exist without it is a queued intent,
    // which is 2c's.
    let r = record();
    assert_eq!(*r.state(), RecordState::Submitting);
    assert!(r.milestones().producer_ready);
    assert!(!r.milestones().dispatched);
}

#[test]
fn dispatch_and_acceptance_are_independently_typed() {
    // spec:2124-2130 — code records both and may not infer either from the
    // other.
    let mut r = record();
    r.mark_dispatched();
    assert!(r.milestones().dispatched);
    assert!(!r.milestones().accepted, "send is not acceptance");
    r.mark_accepted();
    assert!(r.milestones().accepted);
}

#[test]
fn acceptance_is_not_terminal_and_sets_no_completion_milestone() {
    // spec:1996-2000 and 2137-2139. Completed needs the section 6.3 evidence
    // for the class; 2b-i can observe none of it.
    let mut r = record();
    r.mark_dispatched();
    r.mark_accepted();
    assert_eq!(*r.state(), RecordState::Submitting);
    assert!(!r.milestones().hardware_complete);
    assert!(!r.milestones().presented);
    assert!(!r.milestones().prior_buffer_released);
}

#[test]
fn the_first_terminal_state_wins() {
    // spec:2205-2210 — a later explicit result is accepted-stale.
    let mut r = record();
    r.mark_dispatched();
    r.terminalize(TerminalState::CompletionUnknown(UnknownCause::HostCall(
        UnknownReason::WatchdogExpired,
    )));
    let first = *r.state();
    r.terminalize(TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected {
        errno: libc::EBUSY,
    }));
    assert_eq!(*r.state(), first, "a terminalized record does not terminalize again");
}

#[test]
fn an_unknown_terminal_quarantines_the_ledger() {
    let mut r = record();
    r.mark_dispatched();
    r.terminalize(TerminalState::CompletionUnknown(UnknownCause::ContradictoryEvidence));
    assert!(matches!(r.ledger(), LedgerState::Quarantined(_)));
    assert!(r.ledger().releases_nothing());
}

#[test]
fn a_rejection_yields_the_new_state_once_and_never_again() {
    let mut r = record();
    r.mark_dispatched();
    let released = r.terminalize_rejected(libc::EINVAL);
    assert_eq!(released, vec![TestResource(77)]);
    let again = r.terminalize_rejected(libc::EINVAL);
    assert!(again.is_empty(), "a terminalized record releases nothing a second time");
}

#[test]
fn the_request_is_taken_exactly_once() {
    let mut r = record();
    r.attach_request(request_for_tests(), SubmittingProof::for_tests());
    assert!(r.take_request().is_some());
    assert!(r.take_request().is_none(), "a second send must not reuse one reservation");
}

#[test]
fn fence_evidence_maps_each_descriptor_back_to_its_crtc() {
    // A bare Vec<OwnedFd> loses this: an Accepted reply returns only the
    // descriptors that came back, so position alone does not name a CRTC.
    let mut r = record();
    r.adopt_fences(
        vec![OutFenceSlot { crtc_id: 5, value_index: 1 },
             OutFenceSlot { crtc_id: 9, value_index: 3 }],
        0b10,                 // only slot 1 produced a descriptor
        vec![pipe_read_end()],
    );
    let ev = r.fence_evidence().expect("evidence");
    assert_eq!(ev.returned(), 1);
    assert_eq!(ev.by_crtc().len(), 1);
    assert_eq!(ev.by_crtc()[0].0, 9, "the set bit is slot 1, whose CRTC is 9");
}

#[test]
fn a_tombstone_keeps_identity_and_sets_but_owns_no_resource() {
    // spec:1699-1701.
    let mut r = record();
    r.mark_dispatched();
    let _ = r.terminalize_rejected(libc::EINVAL);
    let t = r.tombstone().expect("a terminalized record tombstones");
    assert_eq!(t.commit, r.commit_id());
    assert_eq!(t.event_token, r.event_token());
    assert_eq!(t.kernel_event_crtcs, r.closure().kernel_event().to_vec());
    assert_eq!(t.present_event_crtcs, r.closure().present_event().to_vec());
    assert!(matches!(t.terminal, TerminalState::FailedBeforeSubmit(_)));
}

#[test]
fn a_live_record_does_not_tombstone() {
    assert!(record().tombstone().is_none());
}
```

`record()` builds a `CommitRecord<TestResource>` over a single active CRTC with `page_flip_event = true` and one Present consumer, whose ledger is `Submitted::new(vec![TestResource(66)], vec![TestResource(77)])`. `pipe_read_end()` returns an `OwnedFd` from `libc::pipe`, so the descriptor is real and its close is observable.

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::record`
Expected: FAIL — the module does not exist.

- [ ] **Step 4: Write the record**

Write the vocabulary types exactly as the contract section gives them, then:

```rust
impl<R> CommitRecord<R> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        commit: CommitId,
        event_token: EventToken,
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        transition: Option<LifecycleTransitionId>,
        topology_generation: u64,
        closure: AtomicCrtcClosure,
        correlation: HostCallCorrelation,
        ledger: Submitted<R>,
    ) -> Self {
        Self {
            commit, event_token, incarnation, lifecycle_epoch, transition,
            topology_generation, closure, correlation,
            milestones: Milestones { producer_ready: true, ..Milestones::default() },
            ledger: LedgerState::Submitted(ledger),
            observed_crtcs: Vec::new(),
            fences: None,
            pending_request: None,
            state: RecordState::Submitting,
        }
    }

    /// Set at `send` return, not at reply.
    pub fn mark_dispatched(&mut self) { self.milestones.dispatched = true; }

    /// Set only by an explicit `HostCallOutcome::Accepted` whose fence output
    /// is complete. It implies nothing about hardware completion.
    pub fn mark_accepted(&mut self) {
        self.milestones.accepted = true;
        self.advance_ledger(|l| match l {
            LedgerState::Submitted(s) => LedgerState::Accepted(s.accepted()),
            other => other,
        });
    }

    pub fn adopt_fences(&mut self, slots: Vec<OutFenceSlot>, mask: u32, fences: Vec<OwnedFd>) {
        self.fences = Some(FenceEvidence { slots, mask, fences });
    }

    /// The one place a resource leaves a record. Returns the never-current
    /// new state by value; a terminalized record returns an empty vector.
    pub fn terminalize_rejected(&mut self, errno: i32) -> Vec<R> {
        if matches!(self.state, RecordState::Terminal(_)) { return Vec::new(); }
        let mut released = Vec::new();
        self.advance_ledger(|l| match l {
            LedgerState::Submitted(s) => {
                let (rejected, freed) = s.rejected();
                released = freed;
                LedgerState::Rejected(rejected)
            }
            other => other,
        });
        self.state = RecordState::Terminal(TerminalState::FailedBeforeSubmit(
            FailureCause::IoctlRejected { errno },
        ));
        released
    }

    pub fn terminalize(&mut self, terminal: TerminalState) {
        if matches!(self.state, RecordState::Terminal(_)) { return; }
        if matches!(terminal, TerminalState::CompletionUnknown(_)) {
            self.advance_ledger(|l| match l {
                LedgerState::Submitted(s) => LedgerState::Quarantined(s.unknown()),
                LedgerState::Accepted(a) => LedgerState::Quarantined(a.unknown()),
                other => other,
            });
        }
        self.state = RecordState::Terminal(terminal);
    }

    fn advance_ledger(&mut self, f: impl FnOnce(LedgerState<R>) -> LedgerState<R>) {
        let taken = std::mem::replace(&mut self.ledger, LedgerState::Poisoned);
        self.ledger = f(taken);
    }

    pub fn tombstone(&self) -> Option<Tombstone> {
        let RecordState::Terminal(terminal) = self.state else { return None };
        Some(Tombstone {
            commit: self.commit,
            event_token: self.event_token,
            incarnation: self.incarnation,
            lifecycle_epoch: self.lifecycle_epoch,
            kernel_event_crtcs: self.closure.kernel_event().to_vec(),
            present_event_crtcs: self.closure.present_event().to_vec(),
            observed_crtcs: self.observed_crtcs.clone(),
            terminal,
        })
    }
}
```

`terminalize_rejected` is separate from `terminalize` because it is the only terminal transition that produces a value, and folding it into the general path would mean either returning `Vec<R>` from every terminalization or dropping the released resources on the floor.

`transition` and `topology_generation` are stored and not yet read: `spec:1687-1692` requires the record to supply the lifecycle identities and device generation to a resolved event, and 2b-ii's correlation reads exactly these fields. Adding them later would touch every construction site.

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::record`
Expected: PASS, 10 tests.

- [ ] **Step 6: Commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/owner/record.rs crates/yserver/src/kms/owner/identity.rs \
        crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): install commit records that own both possible resource states"
```

---

## Task 5: Request construction

**Files:**
- Create: `crates/yserver/src/kms/owner/build.rs`, `crates/yserver/src/kms/owner/test_fixtures.rs`
- Modify: `crates/yserver/src/kms/executor/protocol.rs`, `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: everything Task 1 produces; `AtomicRequest`, `AtomicPropertyList`, `OutFenceSlot`, `HostCallCorrelation`, `ProtocolError` from `kms::executor::protocol`; **`HostCallClass` from `kms::executor`** — it is defined in `mod.rs:167` and `protocol.rs` does not re-export it.
- Produces: `CommitDescription`, `BuildError`, `build_atomic_request`, `same_persistent_properties`; and in `test_fixtures`, `TEST_PROPERTY_IDS` plus `#[doc(hidden)] pub fn` `single_active_crtc()`, `two_crtcs_one_off()`, `two_active_crtcs()`, `atomic_correlation_for_tests(n)` and `request_for_tests()`.

- [ ] **Step 1: Add the page-event flag to the protocol**

`protocol.rs:43-44` defines `DRM_MODE_ATOMIC_TEST_ONLY` and `DRM_MODE_ATOMIC_NONBLOCK`. Add beside them, in the same style:

```rust
pub(crate) const DRM_MODE_PAGE_FLIP_EVENT: u32 = 0x0001;
```

Then extend the flag check at `protocol.rs:514-520`: the page-event bit is accepted for `SeatActiveNonblock` and `ColdStartOrOfflineBlocking`, and is a `ProtocolError` for either validation class. A validation that asks for a page event is a protocol error, not a tolerated combination — `spec:320-323` says `TEST_ONLY` touches no hardware and creates no completion evidence.

- [ ] **Step 2: Write the failing construction tests**

```rust
// crates/yserver/src/kms/owner/build.rs  (#[cfg(test)] mod tests)
use crate::kms::executor::HostCallClass;
use crate::kms::executor::protocol::{
    DRM_MODE_ATOMIC_NONBLOCK, DRM_MODE_ATOMIC_TEST_ONLY, DRM_MODE_PAGE_FLIP_EVENT,
};

#[test]
fn one_out_fence_is_added_for_each_expected_completion_crtc_and_none_beyond() {
    let desc = super::super::test_fixtures::two_crtcs_one_off();
    let (req, closure) =
        build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
            .expect("build");
    assert_eq!(closure.expected_completion(), &[1]);
    assert_eq!(req.out_fence_slots.len(), 1);
    assert_eq!(req.out_fence_slots[0].crtc_id, 1);
}

#[test]
fn every_slot_indexes_the_value_the_helper_will_overwrite() {
    // The helper replaces properties.values[value_index] with a POINTER to
    // its own holder storage before the ioctl (helper.rs:216-220), and the
    // kernel writes the fd into that holder (helper.rs:263-270). A misaimed
    // index therefore overwrites an unrelated property's value with a
    // pointer — which is why this assertion is load-bearing rather than
    // cosmetic.
    let desc = super::super::test_fixtures::single_active_crtc();
    let (req, _) =
        build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
            .expect("build");
    let slot = req.out_fence_slots[0];
    let flat = flatten(&req.properties);
    assert_eq!(
        flat[slot.value_index as usize],
        (1u32, desc.property_ids.out_fence_ptr),
        "the slot must index this CRTC's OUT_FENCE_PTR entry"
    );
}

#[test]
fn a_validation_request_carries_no_out_fence_and_still_builds() {
    // The first draft made every active ValidationOnly request fail its own
    // re-scan. This is the regression guard.
    let desc = super::super::test_fixtures::single_active_crtc();
    let (req, closure) =
        build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveValidation)
            .expect("a validation over an active CRTC must build");
    assert_eq!(closure.expected_completion(), &[1], "the closure is unchanged");
    assert!(req.out_fence_slots.is_empty());
    assert_ne!(req.flags & DRM_MODE_ATOMIC_TEST_ONLY, 0);
    assert_eq!(req.flags & DRM_MODE_ATOMIC_NONBLOCK, 0, "spec:320-322");
    assert_eq!(req.flags & DRM_MODE_PAGE_FLIP_EVENT, 0);
}

#[test]
fn a_validation_never_carries_a_page_event_even_when_asked() {
    let mut desc = super::super::test_fixtures::single_active_crtc();
    desc.page_flip_event = true;
    let (req, _) =
        build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveValidation)
            .expect("build");
    assert_eq!(req.flags & DRM_MODE_PAGE_FLIP_EVENT, 0);
}

#[test]
fn a_seat_active_commit_carries_nonblock() {
    let desc = super::super::test_fixtures::single_active_crtc();
    let (req, _) =
        build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
            .expect("build");
    assert_ne!(req.flags & DRM_MODE_ATOMIC_NONBLOCK, 0);
}

#[test]
fn the_page_event_flag_and_the_kernel_event_set_agree() {
    let mut desc = super::super::test_fixtures::single_active_crtc();
    desc.page_flip_event = true;
    let (req, closure) =
        build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
            .expect("build");
    assert_ne!(req.flags & DRM_MODE_PAGE_FLIP_EVENT, 0);
    assert_eq!(closure.kernel_event(), &[1]);
}

#[test]
fn construction_fails_before_submit_on_an_off_to_off_crtc_with_a_page_event() {
    let mut desc = super::super::test_fixtures::two_crtcs_one_off();
    desc.page_flip_event = true;
    let err = build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
        .expect_err("must not build");
    assert!(matches!(err, BuildError::Closure(ClosureError::OffToOffWithPageEvent(2))));
}

#[test]
fn the_request_carries_the_records_correlation_verbatim() {
    // spec:1673-1678 — user_data carries the commit's EventToken. 2a's
    // helper reads it out of the correlation, so the tuple handed over must
    // be the record's.
    let desc = super::super::test_fixtures::single_active_crtc();
    let c = correlation(7);
    let (req, _) =
        build_atomic_request(&desc, c, HostCallClass::SeatActiveNonblock).expect("build");
    assert_eq!(req.correlation, c);
}

#[test]
fn an_oversized_property_list_fails_construction_not_encoding() {
    let desc = description_with_too_many_properties();
    let err = build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
        .expect_err("must fail here");
    assert!(matches!(err, BuildError::Protocol(_)));
}
```

`flatten(&AtomicPropertyList)` returns a `Vec<(object, prop)>` parallel to `values`; `correlation(n)` builds a `HostCallCorrelation::Atomic` with `CommitId::for_tests(n)` and `EventToken::tagged_for_tests(n)` — **`tagged_for_tests`, never `for_tests`**, because both token decoders check the purpose tag and an untagged token is rejected on arrival. `description_with_too_many_properties()` exceeds `MAX_ATOMIC_PROPS`.

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::build`
Expected: FAIL — the module does not exist.

- [ ] **Step 4: Write the builder and the shared fixtures**

```rust
// crates/yserver/src/kms/owner/build.rs

//! Turning a commit description into the exact request 2a puts on the wire.
//!
//! Order is normative, not stylistic. The closure is computed from the
//! description *before* any completion property exists, the out-fence
//! entries are appended, and only then is the serialized list re-scanned.
//! Computing the closure afterwards would let an ephemeral out-fence entry
//! enlarge it, which spec:592-595 forbids by name.

#[derive(Debug, Clone)]
pub struct CommitDescription {
    /// The minimal persistent property list — `spec:551-556`.
    pub objects: Vec<SerializedObject>,
    /// Retained powered state for every closure member. Not serialized; see
    /// `CrtcPower`. The re-scan cross-checks it against any `ACTIVE` that is.
    pub crtc_state: Vec<CrtcPower>,
    pub present_consumers: Vec<u32>,
    pub page_flip_event: bool,
    pub property_ids: PropertyIds,
}

impl CommitDescription {
    /// The object-kind map the re-scan needs, derived from the same objects
    /// the closure was computed from. There is deliberately no way to supply
    /// a different one.
    fn kinds(&self) -> BTreeMap<u32, ObjectKind> {
        self.objects.iter().map(|o| (o.object, o.kind)).collect()
    }
}

/// Do two built requests carry the same persistent properties?
///
/// Compares the four parallel arrays with every `OUT_FENCE_PTR` entry and its
/// value removed, because that is exactly and only how a live request differs
/// from the `TEST_ONLY` that validated it. Flags are excluded for the same
/// reason. Anything else differing means the lease would be certifying a
/// request nobody checked.
pub fn same_persistent_properties(
    a: &AtomicRequest,
    b: &AtomicRequest,
    out_fence_ptr: u32,
) -> bool {
    fn persistent(r: &AtomicRequest, out_fence_ptr: u32) -> Vec<(u32, u32, u64)> {
        let mut flat = Vec::new();
        let mut cursor = 0usize;
        for (index, object) in r.properties.objects.iter().enumerate() {
            let count = r.properties.count_props[index] as usize;
            for offset in 0..count {
                let prop = r.properties.props[cursor + offset];
                if prop != out_fence_ptr {
                    flat.push((*object, prop, r.properties.values[cursor + offset]));
                }
            }
            cursor += count;
        }
        flat
    }
    // The caller passes the id from the live description's `PropertyIds`;
    // both sides were built against the same device, so one id serves for
    // both. Ordering is preserved rather than sorted: two lists that carry
    // the same properties in a different order are different requests.
    persistent(a, out_fence_ptr) == persistent(b, out_fence_ptr)
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("closure: {0}")]
    Closure(#[from] ClosureError),
    #[error("protocol: {0:?}")]
    Protocol(ProtocolError),
}

pub fn build_atomic_request(
    desc: &CommitDescription,
    correlation: HostCallCorrelation,
    class: HostCallClass,
) -> Result<(AtomicRequest, AtomicCrtcClosure), BuildError> {
    let fences =
        if class.is_validation() { FencePolicy::Forbidden } else { FencePolicy::Required };
    // A validation never carries a page event, so the closure it is checked
    // against must be computed without one. spec:320-323.
    let page_flip_event = desc.page_flip_event && fences == FencePolicy::Required;

    let closure = AtomicCrtcClosure::compute(
        &desc.objects,
        &desc.crtc_state,
        &desc.property_ids,
        page_flip_event,
        &desc.present_consumers,
    )?;

    let mut objects = Vec::new();
    let mut count_props = Vec::new();
    let mut props = Vec::new();
    let mut values = Vec::new();
    let mut out_fence_slots = Vec::new();

    for object in &desc.objects {
        let wants_fence = fences == FencePolicy::Required
            && object.kind == ObjectKind::Crtc
            && closure.expected_completion().contains(&object.object);

        objects.push(object.object);
        count_props.push((object.props.len() + usize::from(wants_fence)) as u32);
        for (prop, value) in &object.props {
            props.push(*prop);
            values.push(*value);
        }
        if wants_fence {
            out_fence_slots.push(OutFenceSlot {
                crtc_id: object.object,
                value_index: values.len() as u32,
            });
            props.push(desc.property_ids.out_fence_ptr);
            // The holder is initialized to -1 by the helper; this value is
            // overwritten with a pointer to it before the ioctl.
            values.push(u64::MAX);
        }
    }

    let properties = AtomicPropertyList { objects, count_props, props, values };
    properties.validate().map_err(BuildError::Protocol)?;
    closure.verify_serialized(&properties, &desc.kinds(), &desc.property_ids, fences)?;

    let mut flags = match class {
        HostCallClass::SeatActiveNonblock => DRM_MODE_ATOMIC_NONBLOCK,
        HostCallClass::SeatActiveValidation | HostCallClass::ColdStartOrOfflineValidation => {
            DRM_MODE_ATOMIC_TEST_ONLY
        }
        HostCallClass::ColdStartOrOfflineBlocking => 0,
    };
    if page_flip_event {
        flags |= DRM_MODE_PAGE_FLIP_EVENT;
    }

    Ok((AtomicRequest { correlation, class, flags, properties, out_fence_slots }, closure))
}
```

Then `crates/yserver/src/kms/owner/test_fixtures.rs`:

```rust
//! Descriptions shared by unit tests, the integration test and the backend
//! fixtures.
//!
//! **Not `#[cfg(test)]`.** An integration-test crate links the library built
//! *without* `cfg(test)`, so a `#[cfg(test)]` fixture is invisible to it and
//! a crate-private one is inaccessible. `#[doc(hidden)] pub` is the same seam
//! stage 2a used for `executor::test_support`, and Task 7's grep bounds it.

#[doc(hidden)]
pub const TEST_PROPERTY_IDS: PropertyIds =
    PropertyIds { crtc_id: 20, active: 21, out_fence_ptr: 22 };

/// CRTC 1 active before and after, with a plane bound to it.
#[doc(hidden)]
pub fn single_active_crtc() -> CommitDescription { /* ... */ }

/// CRTC 1 active, CRTC 2 inactive before and after.
#[doc(hidden)]
pub fn two_crtcs_one_off() -> CommitDescription { /* ... */ }

/// CRTCs 1 and 2 both active before and after, so
/// `expected_completion == [1, 2]` and a request over it needs two
/// out-fences. Used by the short-mask test, which needs a request that
/// expects more fences than the reply returns.
#[doc(hidden)]
pub fn two_active_crtcs() -> CommitDescription { /* ... */ }

/// A built `HostCallRequest` for record-level tests that only need something
/// to attach and take back. It is never sent, so its contents are
/// irrelevant beyond being well-formed.
#[doc(hidden)]
pub fn request_for_tests() -> HostCallRequest {
    let (request, _closure) = build_atomic_request(
        &single_active_crtc(),
        atomic_correlation_for_tests(1),
        HostCallClass::SeatActiveNonblock,
    )
    .expect("the fixture description builds");
    HostCallRequest::Atomic(request)
}

/// A `HostCallCorrelation::Atomic` over `CommitId::for_tests(n)` and
/// `EventToken::tagged_for_tests(n)`. **Tagged, never `for_tests`:** both
/// token decoders check the purpose tag, so an untagged token is rejected on
/// arrival and the helper answers with a protocol error instead of a reply.
#[doc(hidden)]
pub fn atomic_correlation_for_tests(n: u64) -> HostCallCorrelation { /* ... */ }

/// An executor whose child has already exited and been reaped, so `send`
/// returns `SendError::Reaped` before installing `InFlight` — the pre-IPC
/// refusal the `NeverDispatched` test needs.
///
/// Built from 2a's stub: spawn `ExitBeforeReply`, then drive `try_reap` until
/// it reports `Reaped`, bounded, because reaping is asynchronous and asserting
/// it at an instant is the race that cost stage 2a two defects.
#[doc(hidden)]
pub fn reaped_executor_for_tests() -> KmsIoExecutor {
    let mut executor =
        crate::kms::executor::test_support::spawn_stub_helper(StubBehaviour::ExitBeforeReply)
            .expect("spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if matches!(executor.try_reap(), ReapState::Reaped(_)) {
            return executor;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("the stub helper did not become reapable within 5s");
}
```

Both return fully-populated descriptions using `TEST_PROPERTY_IDS`; write them out in full rather than deriving one from the other, so a change to one cannot silently retune the other's meaning.

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::build`
Expected: PASS, 9 tests.

- [ ] **Step 6: Commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/owner/build.rs crates/yserver/src/kms/owner/test_fixtures.rs \
        crates/yserver/src/kms/owner/mod.rs crates/yserver/src/kms/executor/protocol.rs
git commit -m "feat(kms): build the atomic request its closure describes"
```

---

## Task 6: The device owner and its typed outcome stream

**Files:**
- Create: `crates/yserver/src/kms/owner/device.rs`, `crates/yserver/tests/owner_commit_record.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs`, `crates/yserver/src/kms/owner/{mod.rs,test_fixtures.rs}`

**Interfaces:**
- Consumes: everything Tasks 1-5 produce; `KmsIoExecutor`, `HostCallEvent`, `HostCallOutcome`, `HostCallReservation`, `HostCallRequest`, `SendError`, `UnknownReason`, `HostCallClass` from `kms::executor`; `IdentityAllocator`, `IncarnationId`, `CommitId` from `kms::owner::identity`; `RequestSeq::from_raw`.
- Produces: `OwnerEvent<R>`, `ValidationOutcome`, `DispatchError<R>`, `DeviceCommitOwner<R>`, and `DeviceCommitOwner::{new, begin, begin_validated, send_on, dispatch, begin_validation, send_validation_on, abandon_validation, apply_host_call_event, slot, live_record, tombstones, mark_dispatched_for_tests}`; in `test_fixtures`, `#[doc(hidden)] pub enum TestResource`, `#[doc(hidden)] pub fn owner_for_tests() -> DeviceCommitOwner<TestResource>`, `ledger() -> Submitted<TestResource>`, `reaped_executor_for_tests()`, and the event constructors the tests use: `accepted(commit, mask, fence_count)`, `late_accepted(..)`, `rejected(commit, errno)`, `unknown(commit, reason)`, `validation_abandoned(commit, reason)`, `probe_accepted_event(sequence)` and `off_to_off_crtc(id)`. All are `#[doc(hidden)] pub` for the same reason as the descriptions: the integration crate needs them.

**Why `begin` and `send_on` are separate.** `COMMIT-6` orders the work: install the `Submitting` record and reserve the slot, *then* send IPC. Splitting the call at exactly that boundary makes the order a signature rather than a comment, and lets every state test run without spawning a helper process. `dispatch` is `begin` followed by `send_on` and is what production calls. The built request lives in the record between the two, which is also where `spec:578`'s "later request mutation is forbidden" wants it.

- [ ] **Step 1: Make `UnknownReason` enumerable in a way a new variant cannot escape**

In `crates/yserver/src/kms/executor/mod.rs`, beside `UnknownReason`:

```rust
impl UnknownReason {
    /// Bump this and extend `ALL` when a variant is added. `index` below is
    /// what forces you to: adding a variant makes its match non-exhaustive,
    /// which is a compile error, and `ALL`'s length is checked against this
    /// constant at compile time.
    pub const COUNT: usize = 4;

    #[doc(hidden)]
    pub const ALL: [Self; Self::COUNT] = [
        Self::WatchdogExpired,
        Self::HelperExited,
        Self::IpcFailure,
        Self::MalformedReply,
    ];

    const fn index(self) -> usize {
        match self {
            Self::WatchdogExpired => 0,
            Self::HelperExited => 1,
            Self::IpcFailure => 2,
            Self::MalformedReply => 3,
        }
    }
}

// Every entry of ALL sits at its own index, so ALL cannot drift out of sync
// with `index` without failing to compile.
const _: () = {
    let mut i = 0;
    while i < UnknownReason::COUNT {
        assert!(UnknownReason::ALL[i].index() == i);
        i += 1;
    }
};
```

The draft claimed a fifth variant could not compile without being classified; it manually listed four in a test array and that claim was false. This makes it true.

- [ ] **Step 2: Write the failing owner tests**

```rust
// crates/yserver/src/kms/owner/device.rs  (#[cfg(test)] mod tests)

#[test]
fn begin_installs_the_record_and_reserves_the_slot_before_any_ipc() {
    // COMMIT-6's ordering, expressed as an API: a send that fails still finds
    // a device with a record that owns the uncertainty.
    let mut o = owner_for_tests();
    let (commit, events) = o.begin(&single_active_crtc(), ledger()).expect("begin");
    assert_eq!(o.slot().occupant(), Some(commit));
    assert!(!o.live_record().expect("record").milestones().dispatched);
    assert!(events.is_empty(), "begin emits nothing; send_on emits Dispatched");
}

#[test]
fn a_second_begin_is_refused_while_a_record_lives() {
    // spec:1330-1334.
    let mut o = owner_for_tests();
    o.begin(&single_active_crtc(), ledger()).expect("first");
    let err = o.begin(&single_active_crtc(), ledger()).expect_err("refused");
    assert!(matches!(err, DispatchError::Slot(SlotError::AlreadyOccupied(_))));
}

#[test]
fn a_construction_failure_never_consumes_the_slot() {
    // Building precedes reserving, so a description that cannot produce a
    // valid request leaves the device admissible.
    let mut o = owner_for_tests();
    let mut bad = single_active_crtc();
    bad.page_flip_event = true;
    bad.objects.push(off_to_off_crtc(2));
    assert!(o.begin(&bad, ledger()).is_err());
    assert_eq!(o.slot().occupant(), None);
    assert!(o.live_record().is_none());
}

#[test]
fn an_executor_refusal_before_ipc_is_never_dispatched_not_acceptance_unknown() {
    // spec:612-617 — a refusal before send is cancellation, not uncertainty.
    // `send` returns Reaped / Stalled / AlreadyInFlight / ReservationMismatch
    // / BoundaryViolation *before* installing InFlight (executor/mod.rs:665-692)
    // and queues no terminal event, so treating every Err as acceptance-unknown
    // would strand a slot-holding record forever.
    let mut o = owner_for_tests();
    let mut executor = reaped_executor_for_tests();
    o.begin(&single_active_crtc(), ledger()).expect("begin");
    let events = o.send_on(&mut executor).expect_err("refused").into_events();
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                RefusalCause::Reaped
            )),
            ..
        }
    )));
    // Both halves of the ledger must come back out. Revision 2 dropped the
    // record here, destroying the old state the hardware is still scanning.
    assert!(events.iter().any(|e| matches!(e, OwnerEvent::ResourcesReleased { .. })));
    assert!(events.iter().any(|e| matches!(e, OwnerEvent::ResourcesStillCurrent { .. })));
    assert_eq!(o.slot().occupant(), None, "nothing crossed the boundary");
}

#[test]
fn an_explicit_rejection_is_the_only_proof_of_failed_before_submit() {
    let mut o = owner_for_tests();
    let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
    o.mark_dispatched_for_tests();
    let events = o.apply_host_call_event(rejected(commit, libc::EBUSY));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected { errno }),
            ..
        } if *errno == libc::EBUSY
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::ResourcesReleased { resources, .. } if resources.len() == 1
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::ResourcesStillCurrent { resources, .. } if resources.len() == 1
    )));
    assert_eq!(o.slot().occupant(), None, "a proven rejection releases the slot");
}

#[test]
fn every_acceptance_unknown_reason_keeps_the_slot_held() {
    // COMMIT-6. `UnknownReason::ALL` is compile-checked complete, so a fifth
    // reason cannot be added without deciding which side of this it falls on.
    for reason in UnknownReason::ALL {
        let mut o = owner_for_tests();
        let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
        o.mark_dispatched_for_tests();
        o.apply_host_call_event(unknown(commit, reason));
        assert_eq!(o.slot().occupant(), Some(commit), "{reason:?} released the slot");
        assert!(matches!(o.live_record().expect("retained").ledger(), LedgerState::Quarantined(_)));
    }
}

#[test]
fn a_complete_acceptance_is_recorded_and_does_not_complete_or_release() {
    // The whole point of the 2b split: acceptance is Accepted, not Completed.
    let mut o = owner_for_tests();
    let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
    o.mark_dispatched_for_tests();
    let events = o.apply_host_call_event(accepted(commit, 0b1, 1));
    assert!(events.iter().any(|e| matches!(e, OwnerEvent::Accepted { .. })));
    assert!(!events.iter().any(|e| matches!(e, OwnerEvent::Terminal { .. })));
    let r = o.live_record().expect("still live");
    assert!(r.milestones().accepted);
    assert!(!r.milestones().hardware_complete);
    assert_eq!(*r.state(), RecordState::Submitting);
    assert_eq!(o.slot().occupant(), Some(commit));
}

#[test]
fn a_short_out_fence_mask_is_completion_unknown_not_acceptance() {
    // spec:1955-1962, 2129 — a holder still at -1 after live success is
    // missing completion evidence. The helper sets bit i only when holder i
    // came back non-negative, and the executor's consistency checks accept a
    // mask narrower than the slot table, so this decision is the owner's.
    let mut o = owner_for_tests();
    let (commit, _) = o.begin(&two_active_crtcs(), ledger()).expect("begin");
    o.mark_dispatched_for_tests();
    let events = o.apply_host_call_event(accepted(commit, 0b01, 1)); // 2 expected, 1 back
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::CompletionUnknown(UnknownCause::IncompleteFenceOutput {
                expected: 2, returned: 1
            }),
            ..
        }
    )));
    assert!(!o.live_record().expect("retained").milestones().accepted);
    assert_eq!(o.slot().occupant(), Some(commit), "acceptance is unproven");
}

#[test]
fn a_validation_outcome_under_a_commit_record_is_contradictory_and_terminal() {
    // spec:612-617 — a dispatched result that is neither an explicit
    // rejection nor a normally consumed success becomes CompletionUnknown.
    // The draft only logged a warning and left the record stranded.
    let mut o = owner_for_tests();
    let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
    o.mark_dispatched_for_tests();
    o.apply_host_call_event(validation_abandoned(commit, UnknownReason::WatchdogExpired));
    assert!(matches!(
        o.live_record().expect("retained").state(),
        RecordState::Terminal(TerminalState::CompletionUnknown(UnknownCause::ContradictoryEvidence))
    ));
    assert_eq!(o.slot().occupant(), Some(commit));
}

#[test]
fn an_uncorrelated_outcome_never_touches_the_live_record() {
    // ID-3.
    let mut o = owner_for_tests();
    let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
    o.mark_dispatched_for_tests();
    let events = o.apply_host_call_event(rejected(CommitId::for_tests(999), libc::EINVAL));
    assert!(events.iter().any(|e| matches!(e, OwnerEvent::StaleReply { .. })));
    assert_eq!(*o.live_record().expect("untouched").state(), RecordState::Submitting);
    assert_eq!(o.slot().occupant(), Some(commit));
}

#[test]
fn a_late_reply_never_revives_a_terminalized_record() {
    // spec:2205-2210 — a later success is accepted-stale and quarantined.
    let mut o = owner_for_tests();
    let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
    o.mark_dispatched_for_tests();
    o.apply_host_call_event(unknown(commit, UnknownReason::WatchdogExpired));
    let events = o.apply_host_call_event(late_accepted(commit, 0b1, 1));
    assert!(events.iter().any(|e| matches!(e, OwnerEvent::StaleReply { .. })));
    let r = o.live_record().expect("retained");
    assert!(!r.milestones().accepted);
    assert!(matches!(r.ledger(), LedgerState::Quarantined(_)));
}

#[test]
fn a_validation_resolves_its_own_lease_though_it_has_no_record() {
    // The draft consulted `is_current` — which compares against the live
    // record — before looking for the outstanding validation. A validation
    // deliberately has no record, so its outcome was rejected as
    // uncorrelated and its lease never released.
    let mut o = owner_for_tests();
    let commit = o.begin_validation(&single_active_crtc()).expect("validate");
    assert_eq!(o.slot().occupant(), None);
    assert_eq!(o.slot().validation_outstanding(), Some(commit));
    let events = o.apply_host_call_event(accepted(commit, 0, 0));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::ValidationResolved { outcome: ValidationOutcome::Passed, .. }
    )));
    assert_eq!(
        o.slot().validation_outstanding(),
        Some(commit),
        "spec:305-325: the lease protects the gap between the validation and \
         the live call, so the TEST_ONLY reply does not end it"
    );
    assert!(o.tombstones().is_empty(), "a validation leaves no commit tombstone");
}

#[test]
fn the_lease_ends_at_the_live_call_and_only_for_the_request_it_validated() {
    let mut o = owner_for_tests();
    let desc = single_active_crtc();
    let commit = o.begin_validation(&desc).expect("validate");
    o.apply_host_call_event(accepted(commit, 0, 0));

    // A different description cannot ride a lease taken for this one.
    let err = o.begin_validated(&two_active_crtcs(), ledger()).expect_err("refused");
    assert!(matches!(err, DispatchError::ValidationDoesNotMatch));
    assert_eq!(o.slot().validation_outstanding(), Some(commit), "the lease survives");

    let (live, _) = o.begin_validated(&desc, ledger()).expect("the validated request proceeds");
    assert_eq!(o.slot().validation_outstanding(), None);
    assert_eq!(o.slot().occupant(), Some(live));
}

#[test]
fn an_abandoned_validation_frees_the_device() {
    let mut o = owner_for_tests();
    let commit = o.begin_validation(&single_active_crtc()).expect("validate");
    o.apply_host_call_event(rejected(commit, libc::EINVAL));
    o.abandon_validation(commit).expect("abandon");
    assert_eq!(o.slot().validation_outstanding(), None);
    o.begin(&single_active_crtc(), ledger()).expect("admissible again");
}

#[test]
fn a_probe_outcome_under_a_probe_correlation_is_dropped_not_misread() {
    let mut o = owner_for_tests();
    let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
    o.mark_dispatched_for_tests();
    let events = o.apply_host_call_event(probe_accepted_event(42));
    assert!(events.is_empty(), "2b-ii is the probe's consumer");
    assert_eq!(*o.live_record().expect("untouched").state(), RecordState::Submitting);
    let _ = commit;
}

#[test]
fn the_tombstone_ring_keeps_the_last_sixty_four() {
    // spec:1697-1704.
    let mut o = owner_for_tests();
    let mut created = Vec::new();
    for _ in 0..70 {
        let (commit, _) = o.begin(&single_active_crtc(), ledger()).expect("begin");
        created.push(commit);
        o.mark_dispatched_for_tests();
        o.apply_host_call_event(rejected(commit, libc::EINVAL));
    }
    assert_eq!(o.tombstones().len(), 64);
    assert_eq!(o.tombstones()[0].commit, created[6], "the oldest six are evicted");
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::device`
Expected: FAIL — the module does not exist.

- [ ] **Step 4: Write the owner**

```rust
// crates/yserver/src/kms/owner/device.rs

const TOMBSTONE_RING_CAPACITY: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("slot: {0}")] Slot(#[from] SlotError),
    #[error("build: {0}")] Build(#[from] BuildError),
    #[error("identity space exhausted within this incarnation")] IdentityExhausted,
    #[error("no live record to send")] NoLiveRecord,
    #[error("this record's request was already sent")] AlreadySent,
    #[error("this description is not the one the outstanding lease validated")]
    ValidationDoesNotMatch,
    /// The executor refused before any IPC. Carries the events the caller
    /// must still drain, because the record was terminalized here.
    #[error("executor refused before dispatch: {cause:?}")]
    Refused { cause: RefusalCause, events: Vec<OwnerEvent<TestResourcePlaceholder>> },
}
```

`DispatchError` must be generic in `R` to carry `OwnerEvent<R>`; declare it as `DispatchError<R>` and give it `into_events(self) -> Vec<OwnerEvent<R>>` returning an empty vector for every non-`Refused` variant. That is the shape the refusal test uses.

```rust
#[derive(Debug)]
pub struct DeviceCommitOwner<R> {
    slot: DeviceSlot,
    live: Option<CommitRecord<R>>,
    /// A built-but-unsent validation and its lease. A validation installs no
    /// record, so it cannot live in `live`.
    pending_validation: Option<(CommitId, HostCallRequest, ValidationLease)>,
    /// A sent validation awaiting its reply: the commit id and the full
    /// correlation the reply must equal under `ID-3`.
    validation_in_flight: Option<(CommitId, HostCallCorrelation)>,
    /// The description an outstanding lease certifies. `begin_validated`
    /// refuses anything that does not serialize identically to it, so the
    /// lease cannot vouch for a request nobody checked.
    validated_description: Option<(CommitId, CommitDescription)>,
    tombstones: VecDeque<Tombstone>,
    identities: IdentityAllocator,
    lifecycle_epoch: LifecycleEpochId,
    transition: Option<LifecycleTransitionId>,
    topology_generation: u64,
    next_seq: u64,
}

impl<R> DeviceCommitOwner<R> {
    pub fn new(
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        topology_generation: u64,
    ) -> Self {
        Self {
            slot: DeviceSlot::default(),
            live: None,
            pending_validation: None,
            tombstones: VecDeque::new(),
            identities: IdentityAllocator::new(incarnation),
            lifecycle_epoch,
            transition: None,
            topology_generation,
            next_seq: 0,
        }
    }

    fn next_correlation(&mut self) -> Result<(CommitId, EventToken, HostCallCorrelation), DispatchError<R>> {
        let commit = self.identities.checked_next_commit().ok_or(DispatchError::IdentityExhausted)?;
        let event_token =
            self.identities.checked_next_event_token().ok_or(DispatchError::IdentityExhausted)?;
        self.next_seq += 1;
        Ok((commit, event_token, HostCallCorrelation::Atomic {
            seq: RequestSeq::from_raw(self.next_seq),  // production, not for_tests
            incarnation: self.identities.incarnation(),
            lifecycle_epoch: self.lifecycle_epoch,
            transition: self.transition,
            commit,
            event_token,
        }))
    }

    /// Install the record and reserve the slot. No IPC happens here.
    /// Building precedes reserving, so a description that cannot produce a
    /// valid request never consumes the slot.
    pub fn begin(
        &mut self,
        desc: &CommitDescription,
        ledger: Submitted<R>,
    ) -> Result<(CommitId, Vec<OwnerEvent<R>>), DispatchError<R>> {
        let (commit, event_token, correlation) = self.next_correlation()?;
        let (request, closure) =
            build_atomic_request(desc, correlation, HostCallClass::SeatActiveNonblock)?;
        let proof = self.slot.reserve(commit)?;
        let mut record = CommitRecord::new(
            commit, event_token, self.identities.incarnation(), self.lifecycle_epoch,
            self.transition, self.topology_generation, closure, correlation, ledger,
        );
        record.attach_request(HostCallRequest::Atomic(request), proof);
        self.live = Some(record);
        Ok((commit, Vec::new()))
    }

    /// Send the request `begin` built.
    ///
    /// A refusal from `send` before it installs `InFlight` — `Reaped`,
    /// `Stalled`, `AlreadyInFlight`, `ReservationMismatch`,
    /// `BoundaryViolation` — means no IPC crossed the uncertainty boundary
    /// and no terminal event was queued, so the record is `NeverDispatched`
    /// and the slot is released here. Only `SendError::Ipc` means the write
    /// was attempted, and 2a has already queued the terminal event for it.
    pub fn send_on(&mut self, executor: &mut KmsIoExecutor) -> Result<Vec<OwnerEvent<R>>, DispatchError<R>> {
        let record = self.live.as_mut().ok_or(DispatchError::NoLiveRecord)?;
        let commit = record.commit_id();
        let (request, proof) = record.take_request().ok_or(DispatchError::AlreadySent)?;
        match executor.send(&request, HostCallReservation::Submitting(proof)) {
            Ok(()) => {
                record.mark_dispatched();
                Ok(vec![OwnerEvent::Dispatched { commit }])
            }
            Err(SendError::Ipc) => {
                // The write was attempted; 2a queued a terminal event that
                // `apply_host_call_event` will deliver. Dispatched is correct.
                record.mark_dispatched();
                Ok(vec![OwnerEvent::Dispatched { commit }])
            }
            Err(other) => {
                let cause = match other {
                    SendError::Reaped => RefusalCause::Reaped,
                    SendError::Stalled => RefusalCause::Stalled,
                    SendError::AlreadyInFlight => RefusalCause::AlreadyInFlight,
                    SendError::ReservationMismatch => RefusalCause::ReservationMismatch,
                    SendError::BoundaryViolation => RefusalCause::BoundaryViolation,
                    SendError::Ipc => unreachable!("handled above"),
                };
                let terminal =
                    TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(cause));
                record.terminalize(terminal);
                let events = self.retire_live(commit, terminal);
                Err(DispatchError::Refused { cause, events })
            }
        }
    }

    pub fn dispatch(
        &mut self,
        desc: &CommitDescription,
        ledger: Submitted<R>,
        executor: &mut KmsIoExecutor,
    ) -> Result<(CommitId, Vec<OwnerEvent<R>>), DispatchError<R>> {
        let (commit, mut events) = self.begin(desc, ledger)?;
        events.extend(self.send_on(executor)?);
        Ok((commit, events))
    }

    /// Map a pre-install `SendError` to the refusal it represents. `Ipc` is
    /// deliberately absent: it is the one variant meaning the write was
    /// attempted, so it is acceptance-unknown rather than a refusal, and both
    /// send paths handle it before reaching here.
    fn refusal_cause(err: SendError) -> RefusalCause {
        match err {
            SendError::Reaped => RefusalCause::Reaped,
            SendError::Stalled => RefusalCause::Stalled,
            SendError::AlreadyInFlight => RefusalCause::AlreadyInFlight,
            SendError::ReservationMismatch => RefusalCause::ReservationMismatch,
            SendError::BoundaryViolation => RefusalCause::BoundaryViolation,
            SendError::Ipc => unreachable!("handled by the caller"),
        }
    }

    /// Proceed from a passed validation to the live call it validated.
    ///
    /// Refuses unless `desc` serializes identically to the description the
    /// outstanding lease was taken for: a lease that certified one request
    /// must not admit another. On success the lease becomes the slot
    /// reservation in one step, so nothing can be admitted in between.
    pub fn begin_validated(
        &mut self,
        desc: &CommitDescription,
        ledger: Submitted<R>,
    ) -> Result<(CommitId, Vec<OwnerEvent<R>>), DispatchError<R>> {
        let (lease_commit, validated) = self
            .validated_description
            .as_ref()
            .ok_or(DispatchError::ValidationDoesNotMatch)?;
        let (lease_commit, validated) = (*lease_commit, validated.clone());
        let (commit, event_token, correlation) = self.next_correlation()?;
        let (request, closure) =
            build_atomic_request(desc, correlation, HostCallClass::SeatActiveNonblock)?;
        let (reference, _) = build_atomic_request(
            &validated,
            correlation,
            HostCallClass::SeatActiveValidation,
        )?;
        // Compare the serialized persistent properties, not the descriptions:
        // the live request legitimately differs from the validation by its
        // out-fence entries and its flags, and by nothing else.
        if !same_persistent_properties(&request, &reference, desc.property_ids.out_fence_ptr) {
            return Err(DispatchError::ValidationDoesNotMatch);
        }
        let proof = self.slot.consume_validation(lease_commit, commit)?;
        self.validated_description = None;
        let mut record = CommitRecord::new(
            commit, event_token, self.identities.incarnation(), self.lifecycle_epoch,
            self.transition, self.topology_generation, closure, correlation, ledger,
        );
        record.attach_request(HostCallRequest::Atomic(request), proof);
        self.live = Some(record);
        Ok((commit, Vec::new()))
    }

    /// End the exclusive interval without proceeding: a failed or abandoned
    /// validation, or a caller that decides not to submit.
    pub fn abandon_validation(&mut self, commit: CommitId) -> Result<(), DispatchError<R>> {
        self.slot.abandon_validation(commit)?;
        self.pending_validation = None;
        self.validation_in_flight = None;
        self.validated_description = None;
        Ok(())
    }

    /// Send the validation `begin_validation` built. Mirrors `send_on`:
    /// the stored lease moves into `HostCallReservation::Validation`, a
    /// pre-IPC refusal releases the lease and clears `pending_validation`
    /// (nothing crossed the boundary, so nothing is uncertain), a
    /// `SendError::Ipc` keeps both — 2a queued a terminal event that
    /// `apply_host_call_event` will deliver under the stored correlation —
    /// and a second call finds the lease already taken and returns
    /// `DispatchError::AlreadySent`.
    ///
    /// The lease is **not** released on a successful send: it is released by
    /// `consume_validation` at the live call, or by `abandon_validation`.
    pub fn send_validation_on(
        &mut self,
        executor: &mut KmsIoExecutor,
    ) -> Result<Vec<OwnerEvent<R>>, DispatchError<R>> {
        let (commit, request, lease) =
            self.pending_validation.take().ok_or(DispatchError::AlreadySent)?;
        let correlation = request.correlation();
        match executor.send(&request, HostCallReservation::Validation(lease)) {
            Ok(()) | Err(SendError::Ipc) => {
                // The lease moved into the executor; the owner keeps the
                // correlation so the reply can be matched under ID-3.
                self.validation_in_flight = Some((commit, correlation));
                Ok(vec![OwnerEvent::Dispatched { commit }])
            }
            Err(other) => {
                let cause = Self::refusal_cause(other);
                self.slot.abandon_validation(commit)?;
                Err(DispatchError::Refused { cause, events: Vec::new() })
            }
        }
    }

    /// A `TEST_ONLY` request. Takes the exclusive validation lease, installs
    /// no record, never touches the commit slot.
    pub fn begin_validation(
        &mut self,
        desc: &CommitDescription,
    ) -> Result<CommitId, DispatchError<R>> {
        let (commit, _token, correlation) = self.next_correlation()?;
        let (request, _closure) =
            build_atomic_request(desc, correlation, HostCallClass::SeatActiveValidation)?;
        let lease = self.slot.acquire_validation(commit)?;
        self.pending_validation = Some((commit, HostCallRequest::Atomic(request), lease));
        // Recorded here, at the moment the lease is taken, so `begin_validated`
        // has something to compare against. A lease with no recorded
        // description could vouch for anything.
        self.validated_description = Some((commit, desc.clone()));
        Ok(commit)
    }

    pub fn apply_host_call_event(&mut self, event: HostCallEvent) -> Vec<OwnerEvent<R>> {
        let (correlation, outcome, late) = match event {
            HostCallEvent::Outcome { correlation, outcome } => (correlation, outcome, false),
            HostCallEvent::LateReply { correlation, outcome } => (correlation, outcome, true),
        };

        // Order matters. A probe has no record; a validation has no record
        // either, so both must be resolved before `is_current`, which
        // compares against the live record and would otherwise reject them
        // as uncorrelated and leak the validation lease forever.
        let HostCallCorrelation::Atomic { commit, .. } = correlation else {
            log::debug!("owner: probe outcome with no consumer until 2b-ii: {outcome:?}");
            return Vec::new();
        };

        if self.pending_validation.as_ref().is_some_and(|(c, _, _)| *c == commit) {
            return self.resolve_validation(commit, outcome);
        }

        if late || !self.is_current(&correlation) {
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        }

        let Some(record) = self.live.as_mut() else { return Vec::new() };
        if matches!(record.state(), RecordState::Terminal(_)) {
            Self::adopt_and_close(outcome);
            return vec![OwnerEvent::StaleReply { correlation }];
        }
        self.apply_to_live(commit, outcome)
    }
}
```

`apply_to_live` implements the terminal-classification table verbatim: it compares `mask.count_ones()` against `closure.expected_completion().len()` before calling `mark_accepted`, terminalizes `IncompleteFenceOutput` on a shortfall while adopting the fds into the record's quarantine, uses `terminalize_rejected` for `Rejected` and emits its `ResourcesReleased`, terminalizes `ContradictoryEvidence` for a validation or probe outcome, and holds the slot on every `CompletionUnknown`. `resolve_validation` records the outcome, clears `validation_in_flight` and emits one `ValidationResolved` — **it does not release the lease**, which survives until `consume_validation` at the live call or `abandon_validation`. Releasing it here would end the exclusive interval at exactly the moment it exists to protect: the gap between a passed validation and the call it validated. `retire_live(commit, terminal)` takes the record out of `live`, drains its ledger before dropping it — `Rejected<R>::into_current` yields the still-current old state as `ResourcesStillCurrent`, and a `Submitted<R>` reached through a pre-IPC refusal is split by `rejected()` into `ResourcesReleased` and `ResourcesStillCurrent` the same way — then pushes the tombstone and releases the slot. **A record is never dropped while its ledger still holds anything**, which is the invariant the drop-counting test in Task 2 and the refusal test in Task 6 both check; `tombstone_live_holding_slot` pushes the tombstone and keeps both. Both go through `push_tombstone`, which pops the front once the ring exceeds `TOMBSTONE_RING_CAPACITY`. `is_current` compares the whole `HostCallCorrelation` for equality — incarnation, lifecycle epoch, transition, commit id, sequence and event token — not a subset. `CommitId` is device-generation-local, so a stale reply from another incarnation can collide on it. `adopt_and_close(outcome)` moves any `out_fences` out and drops them, closing each exactly once through `OwnedFd`.

`mark_dispatched_for_tests()` is a `#[doc(hidden)]` shim that sets the milestone without an executor, so the state tests need no helper process.

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::device`
Expected: PASS, 14 tests.

- [ ] **Step 6: Write the failing integration tests against a real helper**

```rust
// crates/yserver/tests/owner_commit_record.rs
//! The owner driven by real stub helper processes, so the correlation the
//! record matches against actually made a round trip.
//!
//! Every fixture here comes from `yserver::kms::owner::test_fixtures`, which
//! is `#[doc(hidden)] pub` rather than `#[cfg(test)]` precisely so this crate
//! can reach it.

use std::time::Duration;
use yserver::kms::executor::test_support::{self, StubBehaviour};
use yserver::kms::executor::UnknownReason;
use yserver::kms::owner::device::{DeviceCommitOwner, OwnerEvent};
use yserver::kms::owner::record::{FailureCause, TerminalState, UnknownCause};
use yserver::kms::owner::test_fixtures::{ledger, owner_for_tests, single_active_crtc};

// Every one of these must be imported: none is in the prelude, and revision 2
// used all six unqualified.

#[test]
fn a_rejecting_helper_drives_the_record_to_failed_before_submit() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::RejectWith(libc::EBUSY)).expect("spawn");
    let mut owner = owner_for_tests();
    let (commit, _) = owner
        .dispatch(&single_active_crtc(), ledger(), &mut executor)
        .expect("dispatch");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    let events = owner.apply_host_call_event(executor.poll_reply().expect("reply"));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::Terminal {
            terminal: TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected { .. }),
            ..
        }
    )));
    assert_eq!(owner.slot().occupant(), None);
    assert_eq!(owner.tombstones().last().expect("tombstone").commit, commit);
}

#[test]
fn a_helper_that_dies_leaves_the_slot_held_and_the_ledger_quarantined() {
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    let mut owner = owner_for_tests();
    let (commit, _) = owner
        .dispatch(&single_active_crtc(), ledger(), &mut executor)
        .expect("dispatch");
    test_support::kill_helper(&mut executor);
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    owner.apply_host_call_event(executor.poll_reply().expect("terminal event"));
    assert_eq!(
        owner.slot().occupant(),
        Some(commit),
        "COMMIT-6: a dead helper proves nothing about acceptance"
    );
}

#[test]
fn a_watchdog_expiry_reaches_the_owner_through_tick_without_sleeping() {
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    let mut owner = owner_for_tests();
    let (commit, _) = owner
        .dispatch(&single_active_crtc(), ledger(), &mut executor)
        .expect("dispatch");
    let past = executor.next_deadline().expect("a deadline exists") + Duration::from_millis(1);
    owner.apply_host_call_event(executor.tick(past).expect("the watchdog fires"));
    assert_eq!(owner.slot().occupant(), Some(commit));
    assert!(matches!(
        owner.tombstones().last().expect("tombstone").terminal,
        TerminalState::CompletionUnknown(UnknownCause::HostCall(UnknownReason::WatchdogExpired))
    ));
}
```

Add `ledger()` and `owner_for_tests()` to `test_fixtures.rs` as `#[doc(hidden)] pub`, over a `#[doc(hidden)] pub enum TestResource { OldFramebuffer(u32), NewFramebuffer(u32) }`.

- [ ] **Step 7: Run to verify they pass**

Run: `cargo test -p yserver --test owner_commit_record`
Expected: PASS, 3 tests.

- [ ] **Step 8: Commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/owner/device.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/src/kms/owner/test_fixtures.rs \
        crates/yserver/src/kms/executor/mod.rs crates/yserver/tests/owner_commit_record.rs
git commit -m "feat(kms): drive commit records from one typed owner event stream"
```

---

## Task 7: Backend integration, portable gates and the stage reviewability check

**Files:**
- Modify: `crates/yserver/src/kms/render/platform.rs:1990-2000,3975-3995`, `crates/yserver/src/kms/render/backend.rs:14755-14771`

**Interfaces:**
- Consumes: `DeviceCommitOwner`, `OwnerEvent` (Task 6).
- Produces: a device-keyed routing path, a fd-free observation type for the test tee, and the greps that prove what this sub-stage deliberately did not do.

- [ ] **Step 1: Write the failing routing tests**

```rust
// crates/yserver/src/kms/render/backend.rs, beside
// on_executor_readable_drains_more_than_one_queued_event.
// The fixture helpers are the ones this module already uses for 2a:
// backend_with_stub_executors_with_behaviour_for_tests,
// wait_executor_readable_for_tests, ServerState::new(). Do not add parallel ones.

#[test]
fn an_outcome_reaches_the_owner_of_the_device_that_produced_it() {
    let mut backend = backend_with_stub_executors_with_behaviour_for_tests(
        2,
        crate::kms::executor::test_support::StubBehaviour::RejectWith(libc::EBUSY),
    );
    let mut state = yserver_core::server::ServerState::new();
    // Dispatch on the SECOND device only. Every device is opened with
    // IncarnationId::first() (kms/backend.rs:875), so an incarnation-keyed
    // scan would deliver this to the first device's owner.
    let commit = backend.begin_on_device_for_tests(1).expect("begin");
    backend.send_on_device_for_tests(1).expect("send");
    // Device-indexed: the original helper hard-codes `.devices.first()`, and
    // device 0 has nothing in flight, so it would time out instead of proving
    // anything.
    wait_device_executor_readable_for_tests(&backend, 1, std::time::Duration::from_secs(5));
    yserver_core::backend::Backend::on_executor_readable(&mut backend, &mut state);
    assert_eq!(
        backend.device_owner_for_tests(1).tombstones().last().expect("tombstoned").commit,
        commit
    );
    assert!(
        backend.device_owner_for_tests(0).tombstones().is_empty(),
        "the first device's owner must not have seen the second device's reply"
    );
}

#[test]
fn the_observation_queue_is_still_fed_so_2a_coverage_keeps_working() {
    let mut backend = backend_with_stub_executors_with_behaviour_for_tests(
        1,
        crate::kms::executor::test_support::StubBehaviour::RejectWith(libc::EBUSY),
    );
    let mut state = yserver_core::server::ServerState::new();
    backend.begin_on_device_for_tests(0).expect("begin");
    backend.send_on_device_for_tests(0).expect("send");
    wait_device_executor_readable_for_tests(&backend, 0, std::time::Duration::from_secs(5));
    yserver_core::backend::Backend::on_executor_readable(&mut backend, &mut state);
    assert!(!backend.drained_host_call_events_for_tests().is_empty());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::render::backend`
Expected: FAIL with "no method named `begin_on_device_for_tests`".

- [ ] **Step 3: Carry the device key with every event, and the owner on every device**

`platform.rs:1995` gains, beside `pub(crate) executor: Option<KmsIoExecutor>`:

```rust
    /// The device-local commit owner. Present whenever the executor is: the
    /// two are created together and neither is meaningful without the other.
    pub(crate) owner: Option<crate::kms::owner::device::DeviceCommitOwner<KmsResource>>,
```

`NeverResource` is the uninhabited type from the contract section. **2b-i converts no call site, so the owner genuinely owns no KMS resource**, and naming a handle-shaped stand-in would claim an ownership `spec:2127-2132` requires and this sub-stage cannot deliver. `LedgerState<R>` is generic precisely so 2c substitutes an owning enum here without touching any of this code.

`platform.rs` gains, beside the existing immutable `device_for_key` at `3836-3841`:

```rust
    pub(crate) fn owner_for(
        &mut self,
        key: DrmDeviceKey,
    ) -> Option<&mut crate::kms::owner::device::DeviceCommitOwner<NeverResource>> {
        self.devices.iter_mut().find(|d| d.key == key)?.owner.as_mut()
    }
```

and a device-indexed wait helper in `backend.rs`, because the existing
`wait_executor_readable_for_tests` hard-codes `.devices.first()`
(`backend.rs:39659-39671`) and would poll a device with nothing in flight:

```rust
#[cfg(test)]
pub(crate) fn wait_device_executor_readable_for_tests(
    backend: &KmsBackend,
    index: usize,
    timeout: std::time::Duration,
) {
    use std::os::fd::AsFd;
    let fd = backend.platform.devices[index]
        .executor.as_ref().and_then(|e| e.control_fd()).expect("control fd");
    crate::kms::executor::test_support::wait_readable(fd.as_fd(), timeout);
}
```

Keep the original helper; the 2a test that uses it is single-device and unaffected.

`drain_executor_events` and `tick_executors` change their return type from `Vec<HostCallEvent>` to `Vec<(DrmDeviceKey, HostCallEvent)>`, tagging each event with the key of the device whose executor produced it. **This is the fix for the routing defect, and it is not optional:** every device is opened with `IncarnationId::first()` (`kms/backend.rs:875`) and `HostCallEvent` carries no device key, so reconstructing the source device from the correlation is impossible.

`record_host_call_events` in `backend.rs` becomes:

```rust
    fn record_host_call_events(&mut self, events: Vec<(DrmDeviceKey, crate::kms::executor::HostCallEvent)>) {
        for (key, event) in events {
            log::debug!("kms executor host call event on {key}: {event:?}");
            // A fd-free observation, not a clone: `HostCallOutcome` owns
            // `OwnedFd`s and cloning one would duplicate a descriptor and
            // break the exactly-once close that 2a's
            // `an_accepted_reply_adopts_its_out_fence_and_releases_it_on_drop`
            // pins. Do not derive Clone on either type.
            self.host_call_events_for_tests
                .lock()
                .unwrap()
                .push(HostCallObservation::of(&event));
            if let Some(owner) = self.platform.owner_for(key) {
                let owner_events = owner.apply_host_call_event(event);
                // 2b-i has no consumer for the stream yet; 2c's
                // terminalization is the first. Logging keeps it observable
                // and keeps the single-stream shape honest.
                for owner_event in owner_events {
                    log::debug!("kms owner event on {key}: {owner_event:?}");
                }
            }
        }
    }
```

Add to `executor/mod.rs`:

```rust
/// What crossed the transport, without its descriptors. The test queue holds
/// these; `HostCallEvent` itself is never cloned.
#[derive(Debug, Clone, Eq, PartialEq)]
#[doc(hidden)]
pub struct HostCallObservation {
    pub correlation: HostCallCorrelation,
    pub late: bool,
    pub kind: ObservedOutcome,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub enum ObservedOutcome {
    Accepted { out_fence_mask: u32, fence_count: usize },
    ProbeAccepted { sequence: u64 },
    Rejected { errno: i32 },
    Unknown(UnknownReason),
    ValidationAbandoned(UnknownReason),
}
```

`HostCallObservation::of` is the constructor the routing body calls, and it borrows rather than consumes so the event can still be handed to the owner:

```rust
impl HostCallObservation {
    #[doc(hidden)]
    pub fn of(event: &HostCallEvent) -> Self {
        let (correlation, outcome, late) = match event {
            HostCallEvent::Outcome { correlation, outcome } => (*correlation, outcome, false),
            HostCallEvent::LateReply { correlation, outcome } => (*correlation, outcome, true),
        };
        let kind = match outcome {
            HostCallOutcome::Accepted { out_fence_mask, out_fences, .. } => {
                ObservedOutcome::Accepted {
                    out_fence_mask: *out_fence_mask,
                    fence_count: out_fences.len(),
                }
            }
            HostCallOutcome::ProbeAccepted { sequence, .. } => {
                ObservedOutcome::ProbeAccepted { sequence: *sequence }
            }
            HostCallOutcome::Rejected { errno, .. } => ObservedOutcome::Rejected { errno: *errno },
            HostCallOutcome::Unknown(r) => ObservedOutcome::Unknown(*r),
            HostCallOutcome::ValidationAbandoned(r) => ObservedOutcome::ValidationAbandoned(*r),
        };
        Self { correlation, late, kind }
    }
}
```

`drained_host_call_events_for_tests` returns `Vec<HostCallObservation>`. Update 2a's `on_executor_readable_drains_more_than_one_queued_event`, which asserts only on `len()`, so it needs no other change.

The three `#[cfg(test)]` backend helpers, beside the existing `send_rejected_host_call_for_tests`:

```rust
#[cfg(test)]
impl KmsBackend {
    pub(crate) fn begin_on_device_for_tests(&mut self, index: usize) -> Result<CommitId, ...> {
        let desc = crate::kms::owner::test_fixtures::single_active_crtc();
        let device = self.platform.devices.get_mut(index).expect("device");
        // `NeverResource` is uninhabited, so the only ledger a real device's
        // owner can take is the empty one — which is the truthful ledger for
        // a sub-stage that converts no call site.
        let ledger = Submitted::<NeverResource>::new(Vec::new(), Vec::new());
        device.owner.as_mut().expect("owner").begin(&desc, ledger).map(|(c, _)| c)
    }

    pub(crate) fn send_on_device_for_tests(&mut self, index: usize) -> Result<(), ...> {
        // Destructure once: two `as_mut()` calls on one binding borrow the
        // same `KmsDevice` twice and the borrow checker refuses it.
        let KmsDevice { owner, executor, .. } =
            self.platform.devices.get_mut(index).expect("device");
        owner.as_mut().expect("owner").send_on(executor.as_mut().expect("executor"))?;
        Ok(())
    }

    pub(crate) fn device_owner_for_tests(&self, index: usize) -> &DeviceCommitOwner<KmsResource> {
        self.platform.devices[index].owner.as_ref().expect("owner")
    }
}
```

`platform_with_stub_executors_with_behaviour_for_tests` (`platform.rs:8867-8882`) currently sets only `executor`. It must also set `owner: Some(DeviceCommitOwner::new(IncarnationId::first(), LifecycleEpochId::first(), 1))`, or the helpers above panic on `expect("owner")`.

**Every `KmsDevice` literal must gain the field**, and a missed one is a hard compile error rather than a silent omission. They are at `platform.rs:2561` (the real path — builds an owner from the same incarnation the executor was spawned with), `platform.rs:2989,7302,7751,7772,8362`, and **`backend.rs:24229`**, which revision 2's list missed. Before writing the field, run `grep -rn 'KmsDevice {' crates/yserver/src` and reconcile against that list rather than trusting either.

`begin_on_device_for_tests` builds its ledger as `Submitted::<NeverResource>::new(Vec::new(), Vec::new())` — the owner on a real `KmsDevice` is `DeviceCommitOwner<NeverResource>`, so Task 6's `ledger()` fixture, which yields `Submitted<TestResource>`, cannot be passed here. That is not an inconvenience to work around: an empty ledger is the truthful one, because this sub-stage owns nothing.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p yserver --lib kms::render::backend`
Expected: PASS, including 2a's `on_executor_readable_drains_more_than_one_queued_event`.

- [ ] **Step 5: Run the portable compile gates**

```bash
cargo check -p yserver --target x86_64-unknown-linux-gnu
cargo check -p yserver --target x86_64-unknown-linux-musl
cargo check -p yserver --target x86_64-unknown-freebsd
```

Expected: all three succeed. If a target is missing, `rustup target add` it; do not skip a gate.

- [ ] **Step 6: Run the reviewability greps**

```bash
# 1. The stage-1 SequenceSupport map is deliberately still device-keyed.
grep -n 'HashMap<(crate::platform::drm::DrmDeviceKey, ClockEpochId), SequenceSupport>' \
     crates/yserver/src/kms/render/backend.rs
```
Expected: **one** match, at `backend.rs:1044`. `spec:1755-1763` requires it inside 2b-ii's epoch-local clock record, which does not exist yet. The grep is here so an executor of this plan does not start that migration, and so a reviewer sees the omission is deliberate.

```bash
# 2. No production atomic_commit call site was converted.
grep -rn 'device.atomic_commit\|\.atomic_commit(' crates/yserver/src/drm/
```
Expected: **six** matches, in `page_flip.rs` and `modeset.rs`, unchanged. Converting one is 2c's.

```bash
# 3. No completion milestone is set anywhere in this sub-stage.
grep -rn 'hardware_complete = true\|presented = true\|prior_buffer_released = true' \
     crates/yserver/src
```
Expected: **no** matches. These are 2b-ii's; a match means someone inferred completion from acceptance.

```bash
# 4. The owner performs no ioctl.
grep -rn 'libc::ioctl\|drm_mode_atomic\|SYNC_IOC' crates/yserver/src/kms/owner/
```
Expected: **no** matches. Every kernel interaction goes through the 2a executor.

```bash
# 5. A reservation proof has exactly one issuing site each, and one test seam.
grep -rn 'fn issue()\|SubmittingProof::for_tests\|ValidationLease::for_tests' \
     crates/yserver/src crates/yserver/tests
```
Expected: two `fn issue()` definitions in `owner/slot.rs`, and `for_tests` uses only under `crates/yserver/tests/` and `#[cfg(test)]` modules. **No count is asserted** — the draft's grep demanded "exactly three" against an implementation that necessarily produces four, so a conforming tree failed its own check. What matters is that no production module outside `owner/slot.rs` constructs one; read the matches rather than counting them.

```bash
# 6. Neither event type became Clone.
grep -n 'derive(.*Clone.*)' crates/yserver/src/kms/executor/mod.rs | \
  grep -i 'hostcallevent\|hostcalloutcome'
```
Expected: **no** matches. Cloning an outcome that owns descriptors would duplicate them.

- [ ] **Step 7: Verify, against the pre-existing flake rather than through it**

```bash
cargo +nightly fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p yserver --lib kms::owner
cargo test -p yserver --test owner_commit_record
```

Those two targeted runs must be **clean twelve times running**:

```bash
for i in $(seq 1 12); do
  cargo test -p yserver --lib kms::owner 2>&1 | grep -E '^test result:' | grep -q ' 0 failed' \
    || echo "OWNER FLAKE on run $i"
  cargo test -p yserver --test owner_commit_record 2>&1 | grep -E '^test result:' \
    | grep -q ' 0 failed' || echo "INTEGRATION FLAKE on run $i"
done
```

Expected: no output. This stage's tests spawn helper processes, and stage 2a shipped two real races that a single green run hid — one failed 11 runs in 12 while `clippy` and one `cargo test` both reported success. One green run is not evidence.

The **full** `cargo test -p yserver` is expected to fail roughly one run in five for reasons documented under "A known pre-existing flake": three fork/exec-window tests unrelated to this work. Run it, and confirm that any failure is one of those three named tests. **A failure anywhere else is yours.** Do not relax an assertion to make it pass; that is exactly what concealed stage 2a's races.

- [ ] **Step 8: Commit**

```bash
git add crates/yserver/src/kms/render/platform.rs crates/yserver/src/kms/render/backend.rs \
        crates/yserver/src/kms/executor/mod.rs crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): route each executor outcome to its own device's owner"
```

---

## What this sub-stage proves

- **A request's CRTC closure is computed once, from one description, and re-checked against the bytes that are actually sent.** An out-fence cannot enlarge it, a duplicate out-fence cannot hide in a set, an off-to-off member cannot carry a page event, and a `ValidationOnly` request passes the same re-scan under a policy that forbids fences rather than demanding them.
- **`COMMIT-6`'s asymmetry is structural.** Only an explicit rejection releases the slot. Every acceptance-unknown reason holds it, and `UnknownReason::ALL` is compile-checked complete, so a fifth reason cannot be added without deciding which side it falls on.
- **A short out-fence mask is not acceptance.** The one completion decision this sub-stage can make from the reply alone, it makes.
- **A refusal before IPC is not uncertainty.** `send` can refuse five ways before installing `InFlight`; those release the slot as `NeverDispatched`, and only an attempted write is acceptance-unknown.
- **Resources are owned, not named.** A rejection hands the never-current state out by value; quarantine has no exit; and `Quarantined<R>` exposes no method that yields a resource, so the accepted-stale rule is a type rather than an `if`.
- **A reservation proof has exactly one production issuer.** Both proof types live in `owner/slot.rs` with private `issue` constructors. The `for_tests` seam stays public because stage 2a's integration tests are a separate crate and no `cfg` gate can admit them alone; closing it needs a dev-dependency cargo feature, which is a workspace change this stage does not bundle. The claim is "one production issuer and one named seam", not "unforgeable".
- **Acceptance is not completion.** Nothing here can reach `Completed`; a test and a grep both pin it, because "the slot never frees" looks like a bug to anyone who has not read section 6.3.

## What stage 2b-ii consumes

- `CommitRecord::{milestones, fence_evidence}`: 2b-ii sets `hardware_complete` from successful canonical out-fence status and `presented` from a correlated page event, never one from the other. `FenceEvidence::by_crtc()` is the mapping it needs and the reason the slot table and mask are retained beside the descriptors.
- `AtomicCrtcClosure::{kernel_event, present_event}` are the sets 2b-ii matches an event's `crtc_id` against; `expected_completion` is the set whose fences must all report signalled before `Completed`.
- `Tombstone` and the 64-entry ring are what 2b-ii resolves a delayed event against before deciding it is `unknown`.
- `OwnerEvent<R>` is the stream 2b-ii extends — with `HardwareComplete`, `Presented` and `Completed` variants — never a second parallel event type.
- The `ProbeAccepted` drop in `apply_host_call_event` is the placeholder 2b-ii replaces with the epoch-local clock record's decision between `KernelSequence` and `Unresolved`.
- **A prerequisite, not a handover: the `SequenceSupport` migration.** `backend.rs:1044` is still device-keyed. 2b-ii builds the epoch-local clock record it must move into, and that move is 2b-ii's **first** task.
- **A prerequisite inherited from 2a: core poll-source churn.** `run_core` collects `Backend::poll_fds()` once and never refreshes it (`run.rs:1045-1054`). Neither 2a nor this sub-stage replaces an executor, so neither is affected. The first reopen path — stage 3's — cannot bring a replacement executor's control fd into a running loop until that mechanism exists.

## What this sub-stage deliberately leaves looking broken

- **An accepted commit never completes and the slot never frees.** That is the honest state without fence *status*. Do not add a completion path here.
- **An unknown commit holds the slot forever.** Recovery — the one automatic attempt, the `RecoveryId`, the fd-set barrier — is section 10's table and stage 3's work. Until then a device that reaches `CompletionUnknown` stops accepting commits, which is precisely what `COMMIT-6` asks for.
- **The tombstone ring is written and never read**, and **the `OwnerEvent` stream is emitted and only logged.** Their readers are 2b-ii's correlation and 2c's terminalization. Both are built here because the record that populates them is built here, and retrofitting either onto an existing terminal path is how a duplicate event ends up resolving against a live record.
