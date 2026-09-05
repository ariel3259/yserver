# Phase C.0 Stage 2b-i — The commit record and the device slot

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every KMS device an owner that computes a request's exact CRTC closure, installs a commit record that uncertainty-owns both possible resource states, reserves the one device slot before IPC, and drives that record to exactly one terminal state from the executor's typed outcome.

**Architecture:** Stage 2a completed the executor: it carries a real atomic request, returns its out-fences, and never blocks the core. It has no caller — the backend logs its `HostCallEvent`s and throws them away. This sub-stage builds the caller. Four things in order: a pure closure computation that decides which CRTCs a request affects and which of them owe completion evidence; a commit record whose milestones are independently typed and whose resource ledger survives an unknown outcome; a single device slot that a `Submitting` record reserves before IPC and that `ValidationOnly` never touches; and an outcome stream that replaces the backend's log-and-discard queue with the owner. The record ends at exactly one of `Completed`, `FailedBeforeSubmit` or `CompletionUnknown`, decided only by evidence this sub-stage can actually observe: the executor reply.

**Tech Stack:** Rust (stable toolchain), `libc`. No new dependency. The owner is pure state plus the 2a executor API; it performs no ioctl of its own.

**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md` (Approved, revision 2). This plan implements the first half of section 18 **stage 2b**: request construction and the atomic CRTC closure, commit records with their owned resource ledger, and the single device slot with its typed outcome stream. The second half — the epoch-local clock record and its probe, page-event correlation and MSC/UST normalization, the completion deadlines and the qualification gate — is **2b-ii** and is not started here.

**Predecessor:** `2026-09-04-phase-c0-stage-2a-executor-substrate.md`, complete at `25ee0237` plus the two race repairs recorded in its "What replaces it" section.

**Why 2b is split.** Section 18 sizes each sub-stage near stage 1, which returned 2 blocking findings at 14 tasks where the 21- and 23-task stage 2 monoliths returned 24 and 26. The eight normative concerns section 18 assigns to 2b do not fit that size in one document. They divide cleanly at the point where evidence stops coming from the executor reply and starts coming from the kernel: everything here is decided by an IPC outcome the owner already correlates, and everything in 2b-ii is decided by a fence, an event or a clock. That seam is also the one place the two halves have a single narrow interface, listed in "What stage 2b-ii consumes".

---

## Global Constraints

Copied from the spec. Every task's requirements implicitly include this section.

- **`COMMIT-5`** — the X11 core never executes or waits synchronously for a potentially blocking KMS ioctl. The owner built here calls `send`/`poll_reply`/`tick` and never `dispatch_blocking_at_boundary` outside cold start or final offline.
- **`COMMIT-6`** — before sending IPC the owner installs a `Submitting` record and reserves the device slot. After send, **only an explicit ioctl rejection proves `FailedBeforeSubmit`**; missing or invalid reply, helper exit, IPC failure and watchdog expiry are acceptance-unknown. No second ioctl may be dispatched on the device while this record or its executor lease exists.
- **`ValidationOnly`** — `TEST_ONLY` omits `NONBLOCK`, touches no hardware, creates no out-fence, and **does not occupy the submitted-commit slot**. It holds an exclusive owner validation lease, not a `Submitting` record.
- **`ID-3`** — every executor request and reply carries the lifecycle epoch. A reply is current only when incarnation, lifecycle epoch, optional transition id and commit id all match.
- **Closure (`spec:541-556`)** — `AtomicCrtcClosure` = every CRTC with a persistent CRTC-property entry, union every non-zero old or new `CRTC_ID` binding of each connector or plane having a persistent property entry. `ExpectedCompletionCrtcs` = every CRTC in the closure where `old.active || new.active`.
- **Out-fence placement (`spec:565-568`)** — exactly one `OUT_FENCE_PTR` property for every member of `ExpectedCompletionCrtcs` and **none outside it**. Ephemeral out-fence entries may not enlarge `AtomicCrtcClosure`.
- **Off-to-off (`spec:583-595`)** — construction fails before submit for every inactive-to-inactive closure member if the global `PAGE_FLIP_EVENT` flag is set or an out-fence pointer was assigned to that CRTC. An inactive-to-inactive member is permissible only with neither signaling source.
- **Event sets (`spec:571-574`)** — when `PAGE_FLIP_EVENT` is set, `KernelEventCrtcs = ExpectedCompletionCrtcs`; `PresentEventCrtcs` is the subset with a Present consumer.
- **Re-scan (`spec:569-570`)** — the final serialized request is re-scanned before dispatch; if its kernel-visible CRTC closure differs from the recorded set, construction fails before submit.
- **`user_data` (`spec:1673-1678`)** — `drm_mode_atomic.user_data` carries the commit's `EventToken` verbatim. Raw CRTC ids are never event identities.
- **Tombstones (`spec:1697-1704`)** — the owner keeps the last 64 identity-only tombstones for terminalized commits. They retain kernel-event, Present-event and observed CRTC sets plus terminal state, and own no KMS resource. Eviction merely changes a very old duplicate from `tombstoned` to `unknown`.
- **Identity allocation** — checked increment; never wraps or reuses a token within an incarnation.
- Portable builds must compile on glibc, musl and FreeBSD.
- Format is `cargo +nightly fmt --check`. Tests are `cargo test -p yserver`. Lint is `cargo clippy --all-targets -- -D warnings`, exactly as CI runs it.

### What this sub-stage does not do

- **No production `atomic_commit` call site is converted.** The six live sites stay on the Phase A+B path; conversion is 2c's. The owner is reachable in this sub-stage only through its own API and its tests, exactly as 2a's executor was.
- **No intents, no admission, no fairness, no ordering classes.** Section 9.1, 9.2 and 9.2.1 are 2c's. The owner here accepts a fully-formed commit description and does not choose between two of them.
- **No completion evidence.** Out-fence *slots* are specified in the request because the closure decides them, but no fence is adopted, queried or closed here. `HardwareComplete`, `Presented` and `PriorBufferReleased` exist as typed milestones that nothing in this sub-stage can set. 2b-ii sets them.
- **No kernel event handling.** The raw parser in `drm/event_stream.rs` keeps its current callers. Owner-exclusive drain, correlation and MSC/UST normalization are 2b-ii.
- **No clock record and no probe.** `HostCallOutcome::ProbeAccepted` stays unconsumed. The stage-1 `SequenceSupport` map at `kms/render/backend.rs:1044` is **not** moved — spec lines 1755-1763 require it inside 2b-ii's epoch-local clock record, which does not exist yet. Task 7 greps for it so an executor of this plan does not start that migration.
- **No recovery, no poison, no quarantine teardown.** `CompletionUnknown` marks the ledger quarantined; what releases a quarantine is section 10's fd-set barrier, which belongs to stage 3.
- **No coordinate transport.** The section 7.1 `CoordinateSubmitting` reservation is cursor work and belongs to stage 4; the slot type here has no variant for it.

---

## File Structure

**New — `yserver`:**
- `crates/yserver/src/kms/owner/closure.rs` — `AtomicObject`, `PersistentEntry`, `CrtcPower`, `AtomicCrtcClosure`, `ClosureError`, and the serialized re-scan.
- `crates/yserver/src/kms/owner/record.rs` — `CommitRecord`, `Milestones`, `TerminalState`, `RecordState`.
- `crates/yserver/src/kms/owner/ledger.rs` — `ResourceLedger`, `LedgerDisposition`, `Quarantine`.
- `crates/yserver/src/kms/owner/slot.rs` — `DeviceSlot`, `SlotError`, and the production producers for `SubmittingProof` and `ValidationLease`.
- `crates/yserver/src/kms/owner/build.rs` — `CommitDescription` and `build_atomic_request`.
- `crates/yserver/src/kms/owner/device.rs` — `DeviceCommitOwner`: the slot, the live record, the tombstone ring, and `apply_host_call_event`.
- `crates/yserver/tests/owner_commit_record.rs` — integration coverage that drives a real stub helper through the owner.

**Modified — `yserver`:**
- `crates/yserver/src/kms/owner/mod.rs` — declares the six new modules.
- `crates/yserver/src/kms/executor/mod.rs` — `SubmittingProof` and `ValidationLease` gain `pub(crate)` production constructors alongside their `for_tests` ones.
- `crates/yserver/src/kms/render/platform.rs:1990-2000` — `KmsDevice` carries a `DeviceCommitOwner` beside its executor.
- `crates/yserver/src/kms/render/backend.rs:14755-14761` — `record_host_call_events` routes each event into the owning device's owner instead of only logging it.

**Explicitly out of scope:**
- `crates/yserver/src/drm/page_flip.rs`, `crates/yserver/src/drm/modeset.rs` — the live commit paths. 2c converts them.
- `crates/yserver/src/present/event_loop.rs` — as in stage 1 and 2a: its `run_loop` has no caller in the workspace.

---

## The record and closure contract

**This section is normative. Every task implements it; no task re-derives it.** Revision 4 of the 2a plan established why: when a type model is restated in five tasks, a change lands in one and the neighbours keep speaking the old language, which is where five of that round's ten blocking findings came from. A model change here is one edit.

### Closure vocabulary

```rust
// crates/yserver/src/kms/owner/closure.rs

/// A DRM object appearing in the final serialized persistent property list.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum AtomicObject {
    Crtc(u32),
    Connector(u32),
    Plane(u32),
}

/// One object's participation in the persistent property list.
///
/// `old_crtc_id`/`new_crtc_id` are the object's `CRTC_ID` bindings. They are
/// meaningful for a connector or a plane and are both `None` for a CRTC, whose
/// own id is its binding. Zero means unbound and is not a closure member.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct PersistentEntry {
    pub object: AtomicObject,
    pub old_crtc_id: Option<u32>,
    pub new_crtc_id: Option<u32>,
}

/// Powered state of one CRTC across the request.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CrtcPower {
    pub crtc_id: u32,
    pub old_active: bool,
    pub new_active: bool,
}

/// The four CRTC sets, each sorted and deduplicated.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AtomicCrtcClosure {
    closure: Vec<u32>,
    expected_completion: Vec<u32>,
    kernel_event: Vec<u32>,
    present_event: Vec<u32>,
}
```

`AtomicCrtcClosure` exposes `closure()`, `expected_completion()`, `kernel_event()` and `present_event()` as `&[u32]`. There is no public constructor other than `AtomicCrtcClosure::compute`, so a set cannot be assembled by hand and then disagree with the request it describes.

### The computation

`compute(entries: &[PersistentEntry], power: &[CrtcPower], page_flip_event: bool, present_consumers: &[u32]) -> Result<AtomicCrtcClosure, ClosureError>`

1. `closure` = every `Crtc(id)` entry's `id`, union every non-zero `old_crtc_id` and `new_crtc_id` of every `Connector`/`Plane` entry. Sort, dedup.
2. Every member must have exactly one `CrtcPower` row, else `ClosureError::UnknownPower(crtc)`. A duplicate row is `ClosureError::DuplicatePower(crtc)`.
3. `expected_completion` = members where `old_active || new_active`.
4. If `page_flip_event`, every member **not** in `expected_completion` — that is, every inactive-to-inactive member — is `ClosureError::OffToOffWithPageEvent(crtc)`. This is the section 6.3 rule stated in the form construction can check: with the global flag set, the kernel creates event state for every closure member and then rejects the off-to-off one.
5. `kernel_event` = `expected_completion` when `page_flip_event`, else empty.
6. `present_event` = `present_consumers` ∩ `kernel_event`. A consumer outside `kernel_event` is `ClosureError::PresentConsumerOutsideEventSet(crtc)` — silently dropping it would create a Present that can never complete.

An empty `expected_completion` is **legal and not an error**: a request whose exact closure affects no old-or-new active CRTC has no completion evidence to manufacture. It is 2b-ii's qualification gate that refuses to let such a request qualify an incarnation, not this computation.

### The re-scan

`AtomicCrtcClosure::verify_serialized(&self, props: &AtomicPropertyList, kinds: &ObjectKinds, ids: &PropertyIds) -> Result<(), ClosureError>` recomputes the closure from the bytes that are actually about to be sent and compares it to the recorded one, returning `ClosureError::SerializedClosureDiffers { recorded, serialized }` on any difference.

`ObjectKinds` maps an object id to `AtomicObject`; `PropertyIds` carries the device's `CRTC_ID`, `ACTIVE` and `OUT_FENCE_PTR` property ids. Both are supplied by the caller because property ids are per-device and this module performs no ioctl. The re-scan also enforces that an `OUT_FENCE_PTR` entry exists for exactly `expected_completion` and for nothing else, which is the check that catches an ephemeral out-fence smuggled onto an off-to-off CRTC.

### Record vocabulary

```rust
// crates/yserver/src/kms/owner/record.rs

/// The six section 10.2 milestones. Each is set only by its own evidence;
/// no setter infers another.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct Milestones {
    pub producer_ready: bool,
    pub dispatched: bool,
    pub accepted: bool,
    pub hardware_complete: bool,
    pub presented: bool,
    pub prior_buffer_released: bool,
}

/// The three section 10 terminal states. A dispatched transaction reaches
/// exactly one.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TerminalState {
    Completed,
    FailedBeforeSubmit(FailureCause),
    CompletionUnknown(UnknownCause),
}

/// Why a record never reached the kernel, or was proven rejected by it.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FailureCause {
    /// Cancelled before IPC dispatch.
    CancelledBeforeDispatch,
    /// An explicit ioctl rejection. This is the only post-dispatch proof of
    /// `FailedBeforeSubmit` that `COMMIT-6` permits.
    IoctlRejected { errno: i32 },
}

/// Why acceptance could not be established or disproved.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UnknownCause {
    HostCall(UnknownReason),
    /// A reply whose correlation is not this record's.
    Uncorrelated,
    /// An outcome whose shape contradicts the request class.
    ContradictoryEvidence,
}

#[derive(Debug)]
pub enum RecordState {
    Submitting,
    Terminal(TerminalState),
}
```

**`UnknownReason` is 2a's**, re-exported rather than redefined: `WatchdogExpired`, `HelperExited`, `IpcFailure`, `MalformedReply`. `UnknownCause::HostCall` wraps it so the owner's vocabulary stays one enum wider than the executor's without duplicating it.

`CommitRecord` holds the identities (`CommitId`, `EventToken`, `IncarnationId`, `LifecycleEpochId`, `Option<LifecycleTransitionId>`, `topology_generation: u64`), the `AtomicCrtcClosure`, the `HostCallCorrelation` it dispatched under, the `Milestones`, the `ResourceLedger`, the observed-CRTC set (empty here; 2b-ii fills it), and the `RecordState`.

### Terminal classification

This is the one table the outcome stream implements. It is the whole of `COMMIT-6` for this sub-stage:

| `HostCallOutcome` | Record outcome | Slot |
| --- | --- | --- |
| `Accepted { .. }` | `accepted = true`, stays `Submitting` — the fences it carries are 2b-ii's evidence and are adopted-and-quarantined here | held |
| `Rejected { errno, .. }` | `Terminal(FailedBeforeSubmit(IoctlRejected { errno }))` | released |
| `Unknown(reason)` | `Terminal(CompletionUnknown(HostCall(reason)))`, ledger quarantined | **held** |
| `ValidationAbandoned(reason)` | not a record outcome — a validation lease has no record; the lease is dropped | never held |
| `ProbeAccepted { .. }` | not a record outcome — a clock probe has no record | never held |
| any of the above under a correlation that is not the live record's | `LateReply` handling: fds adopted and closed into quarantine, record untouched | unchanged |

The `Unknown` row holding the slot is the point of the whole sub-stage. `COMMIT-6` forbids a second ioctl while the record or its executor lease exists, and an unknown outcome proves neither that the kernel took the request nor that it did not. Releasing the slot there is exactly the defect the single-slot rule exists to prevent.

`Accepted` **not** being terminal is the second. Acceptance is `Accepted`, not `Completed`; the section 6.3 row for every class in C.0 requires successful canonical out-fence status on top of it. Since this sub-stage cannot query a fence, an accepted record here stays `Submitting` forever and the slot stays held. That is not a stall to work around — it is the honest state until 2b-ii supplies the missing evidence, and Task 6's test pins it so that no one "fixes" it by completing on acceptance.

### Slot vocabulary

```rust
// crates/yserver/src/kms/owner/slot.rs

/// The one dispatched-or-submitted live atomic transaction per DRM device.
#[derive(Debug, Default)]
pub struct DeviceSlot {
    occupant: Option<CommitId>,
    validation: Option<CommitId>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SlotError {
    #[error("the device slot is already held by commit {0:?}")]
    AlreadyOccupied(CommitId),
    #[error("an exclusive validation lease is already outstanding")]
    ValidationOutstanding,
    #[error("the device slot is not held by commit {0:?}")]
    NotHeld(CommitId),
}
```

`reserve(commit) -> Result<SubmittingProof, SlotError>` and `release(commit) -> Result<(), SlotError>` are the only ways the occupant changes. `acquire_validation(commit) -> Result<ValidationLease, SlotError>` and `release_validation(commit)` take the separate exclusive lease and never touch `occupant`, which is `ValidationOnly`'s whole point. Validation is refused while another validation is outstanding, and permitted while a commit occupies the slot — `TEST_ONLY` touches no hardware and does not occupy the submitted-commit slot.

`SubmittingProof` and `ValidationLease` are 2a's types. This sub-stage adds their production constructors, which are `pub(crate)` and reachable only from `slot.rs`, so the only way to obtain one outside tests is to have actually reserved.

---

## Task 1: The atomic CRTC closure

**Files:**
- Create: `crates/yserver/src/kms/owner/closure.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: nothing from other tasks. `AtomicPropertyList` from `kms::executor::protocol` for the re-scan.
- Produces: `AtomicObject`, `PersistentEntry`, `CrtcPower`, `AtomicCrtcClosure`, `ClosureError`, `ObjectKinds`, `PropertyIds`, and `AtomicCrtcClosure::{compute, closure, expected_completion, kernel_event, present_event, verify_serialized}`.

- [ ] **Step 1: Write the failing closure tests**

```rust
// crates/yserver/src/kms/owner/closure.rs  (at the bottom, #[cfg(test)] mod tests)

fn crtc(id: u32) -> PersistentEntry {
    PersistentEntry { object: AtomicObject::Crtc(id), old_crtc_id: None, new_crtc_id: None }
}
fn plane(id: u32, old: u32, new: u32) -> PersistentEntry {
    PersistentEntry {
        object: AtomicObject::Plane(id),
        old_crtc_id: Some(old),
        new_crtc_id: Some(new),
    }
}
fn on(id: u32) -> CrtcPower { CrtcPower { crtc_id: id, old_active: true, new_active: true } }
fn off(id: u32) -> CrtcPower { CrtcPower { crtc_id: id, old_active: false, new_active: false } }
fn enabling(id: u32) -> CrtcPower {
    CrtcPower { crtc_id: id, old_active: false, new_active: true }
}
fn disabling(id: u32) -> CrtcPower {
    CrtcPower { crtc_id: id, old_active: true, new_active: false }
}

#[test]
fn a_plane_move_includes_both_powered_endpoints() {
    // spec:557-560 — a plane/connector move includes both powered endpoints;
    // detach retains the old CRTC and attach retains the new one.
    let c = AtomicCrtcClosure::compute(
        &[plane(31, 1, 2)],
        &[on(1), on(2)],
        false,
        &[],
    )
    .expect("closure");
    assert_eq!(c.closure(), &[1, 2]);
    assert_eq!(c.expected_completion(), &[1, 2]);
}

#[test]
fn an_unbound_endpoint_is_not_a_closure_member() {
    // Zero means unbound. An attach from nothing has one endpoint, not two.
    let c = AtomicCrtcClosure::compute(&[plane(31, 0, 2)], &[on(2)], false, &[])
        .expect("closure");
    assert_eq!(c.closure(), &[2]);
}

#[test]
fn a_disable_still_owes_completion_evidence() {
    // spec:1873-1876 — the set includes disable and is never empty merely
    // because a disable makes new.active false.
    let c = AtomicCrtcClosure::compute(&[crtc(1)], &[disabling(1)], false, &[])
        .expect("closure");
    assert_eq!(c.expected_completion(), &[1]);
}

#[test]
fn an_inactive_to_inactive_member_owes_nothing_and_is_not_an_error() {
    let c = AtomicCrtcClosure::compute(&[crtc(1)], &[off(1)], false, &[])
        .expect("closure");
    assert_eq!(c.closure(), &[1]);
    assert!(c.expected_completion().is_empty());
}

#[test]
fn an_off_to_off_member_with_a_page_event_fails_construction() {
    // spec:583-591 — prepare_signaling() creates event state for every closure
    // member when the global flag is set, and the atomic check then rejects the
    // off-to-off one. Construction must fail before submit, not at the kernel.
    let err = AtomicCrtcClosure::compute(&[crtc(1), crtc(2)], &[on(1), off(2)], true, &[])
        .expect_err("off-to-off plus page event must not construct");
    assert_eq!(err, ClosureError::OffToOffWithPageEvent(2));
}

#[test]
fn the_kernel_event_set_is_the_expected_set_only_when_the_flag_is_set() {
    let with = AtomicCrtcClosure::compute(&[crtc(1)], &[on(1)], true, &[]).expect("closure");
    assert_eq!(with.kernel_event(), &[1]);
    let without = AtomicCrtcClosure::compute(&[crtc(1)], &[on(1)], false, &[]).expect("closure");
    assert!(without.kernel_event().is_empty());
}

#[test]
fn the_present_set_is_the_consumer_subset_of_the_event_set() {
    // spec:573-574 — events in the set difference are correlated and drained
    // but create no protocol completion.
    let c = AtomicCrtcClosure::compute(&[crtc(1), crtc(2)], &[on(1), on(2)], true, &[1])
        .expect("closure");
    assert_eq!(c.kernel_event(), &[1, 2]);
    assert_eq!(c.present_event(), &[1]);
}

#[test]
fn a_present_consumer_outside_the_event_set_is_rejected_not_dropped() {
    // Dropping it would create a Present that can never complete.
    let err = AtomicCrtcClosure::compute(&[crtc(1)], &[on(1)], true, &[9])
        .expect_err("a consumer with no event must not construct");
    assert_eq!(err, ClosureError::PresentConsumerOutsideEventSet(9));
}

#[test]
fn a_closure_member_with_no_power_row_is_an_error_not_an_assumption() {
    let err = AtomicCrtcClosure::compute(&[crtc(7)], &[], false, &[])
        .expect_err("an unknown powered state must not default to off");
    assert_eq!(err, ClosureError::UnknownPower(7));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::closure`
Expected: FAIL — the module does not exist, so this is a compile error naming `closure`.

- [ ] **Step 3: Write the closure computation**

```rust
// crates/yserver/src/kms/owner/closure.rs

//! The section 6.3 atomic CRTC closure.
//!
//! This module performs no ioctl and holds no device handle. Property ids and
//! object kinds are per-device facts the caller supplies, which is what keeps
//! the computation a pure function that a test can exercise without hardware.

use crate::kms::executor::protocol::AtomicPropertyList;
use std::collections::{BTreeMap, BTreeSet};

// ... the four types from the contract section, verbatim ...

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ClosureError {
    #[error("closure member CRTC {0} has no powered-state row")]
    UnknownPower(u32),
    #[error("CRTC {0} has more than one powered-state row")]
    DuplicatePower(u32),
    #[error("inactive-to-inactive CRTC {0} cannot carry the global page-event flag")]
    OffToOffWithPageEvent(u32),
    #[error("present consumer CRTC {0} is outside the kernel event set")]
    PresentConsumerOutsideEventSet(u32),
    #[error("serialized closure {serialized:?} differs from recorded {recorded:?}")]
    SerializedClosureDiffers { recorded: Vec<u32>, serialized: Vec<u32> },
    #[error("OUT_FENCE_PTR coverage {found:?} differs from expected {expected:?}")]
    OutFenceCoverageDiffers { expected: Vec<u32>, found: Vec<u32> },
    #[error("object {0} in the serialized list has no known kind")]
    UnknownObject(u32),
    #[error("the serialized property list is malformed: {0}")]
    MalformedPropertyList(&'static str),
}

impl AtomicCrtcClosure {
    pub fn compute(
        entries: &[PersistentEntry],
        power: &[CrtcPower],
        page_flip_event: bool,
        present_consumers: &[u32],
    ) -> Result<Self, ClosureError> {
        let mut powered: BTreeMap<u32, CrtcPower> = BTreeMap::new();
        for row in power {
            if powered.insert(row.crtc_id, *row).is_some() {
                return Err(ClosureError::DuplicatePower(row.crtc_id));
            }
        }

        let mut closure: BTreeSet<u32> = BTreeSet::new();
        for entry in entries {
            match entry.object {
                AtomicObject::Crtc(id) => {
                    closure.insert(id);
                }
                AtomicObject::Connector(_) | AtomicObject::Plane(_) => {
                    for binding in [entry.old_crtc_id, entry.new_crtc_id].into_iter().flatten() {
                        if binding != 0 {
                            closure.insert(binding);
                        }
                    }
                }
            }
        }
        let closure: Vec<u32> = closure.into_iter().collect();

        let mut expected_completion = Vec::new();
        for id in &closure {
            let row = powered.get(id).ok_or(ClosureError::UnknownPower(*id))?;
            if row.old_active || row.new_active {
                expected_completion.push(*id);
            } else if page_flip_event {
                return Err(ClosureError::OffToOffWithPageEvent(*id));
            }
        }

        let kernel_event = if page_flip_event { expected_completion.clone() } else { Vec::new() };

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

        Ok(Self { closure, expected_completion, kernel_event, present_event })
    }

    pub fn closure(&self) -> &[u32] { &self.closure }
    pub fn expected_completion(&self) -> &[u32] { &self.expected_completion }
    pub fn kernel_event(&self) -> &[u32] { &self.kernel_event }
    pub fn present_event(&self) -> &[u32] { &self.present_event }
}
```

Add to `crates/yserver/src/kms/owner/mod.rs`:

```rust
#[doc(hidden)]
pub mod closure;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::closure`
Expected: PASS, 9 tests.

- [ ] **Step 5: Write the failing re-scan tests**

```rust
#[test]
fn the_rescan_accepts_a_list_matching_the_recorded_closure() {
    let (kinds, ids) = fixture_device();
    let recorded = AtomicCrtcClosure::compute(&[crtc(1)], &[on(1)], false, &[]).expect("closure");
    let props = serialized_list(&[(1, &[(ids.active, 1), (ids.out_fence_ptr, 0)])]);
    recorded.verify_serialized(&props, &kinds, &ids).expect("matching list");
}

#[test]
fn the_rescan_rejects_a_crtc_that_appeared_after_the_closure_was_recorded() {
    // spec:569-570 — the final serialized request is re-scanned before
    // dispatch; a differing kernel-visible closure fails construction.
    let (kinds, ids) = fixture_device();
    let recorded = AtomicCrtcClosure::compute(&[crtc(1)], &[on(1)], false, &[]).expect("closure");
    let props = serialized_list(&[
        (1, &[(ids.active, 1), (ids.out_fence_ptr, 0)]),
        (2, &[(ids.active, 1)]),
    ]);
    let err = recorded.verify_serialized(&props, &kinds, &ids).expect_err("must differ");
    assert!(matches!(err, ClosureError::SerializedClosureDiffers { .. }));
}

#[test]
fn the_rescan_rejects_an_out_fence_on_a_crtc_outside_the_expected_set() {
    // spec:565-568 — one OUT_FENCE_PTR for every member of
    // ExpectedCompletionCrtcs and none outside it. This is the check that
    // catches an ephemeral out-fence smuggled onto an off-to-off CRTC.
    let (kinds, ids) = fixture_device();
    let recorded =
        AtomicCrtcClosure::compute(&[crtc(1), crtc(2)], &[on(1), off(2)], false, &[])
            .expect("closure");
    assert_eq!(recorded.expected_completion(), &[1]);
    let props = serialized_list(&[
        (1, &[(ids.active, 1), (ids.out_fence_ptr, 0)]),
        (2, &[(ids.active, 0), (ids.out_fence_ptr, 0)]),
    ]);
    let err = recorded.verify_serialized(&props, &kinds, &ids).expect_err("must differ");
    assert!(matches!(err, ClosureError::OutFenceCoverageDiffers { .. }));
}

#[test]
fn the_rescan_rejects_a_missing_out_fence_on_an_expected_crtc() {
    let (kinds, ids) = fixture_device();
    let recorded = AtomicCrtcClosure::compute(&[crtc(1)], &[on(1)], false, &[]).expect("closure");
    let props = serialized_list(&[(1, &[(ids.active, 1)])]);
    let err = recorded.verify_serialized(&props, &kinds, &ids).expect_err("must differ");
    assert!(matches!(err, ClosureError::OutFenceCoverageDiffers { .. }));
}

#[test]
fn the_rescan_rejects_a_list_whose_counts_do_not_describe_its_values() {
    let (kinds, ids) = fixture_device();
    let recorded = AtomicCrtcClosure::compute(&[crtc(1)], &[on(1)], false, &[]).expect("closure");
    let mut props = serialized_list(&[(1, &[(ids.active, 1), (ids.out_fence_ptr, 0)])]);
    props.count_props[0] = 9; // claims nine properties, carries two
    let err = recorded.verify_serialized(&props, &kinds, &ids).expect_err("must be malformed");
    assert!(matches!(err, ClosureError::MalformedPropertyList(_)));
}
```

`fixture_device()` returns an `ObjectKinds` declaring objects 1 and 2 as CRTCs and 31 as a plane, plus a `PropertyIds { crtc_id: 20, active: 21, out_fence_ptr: 22 }`. `serialized_list(&[(object, &[(prop, value)])])` assembles the four parallel vectors of an `AtomicPropertyList`. Write both helpers in the test module; they are three lines each and repeating them beats a shared fixture module for one consumer.

- [ ] **Step 6: Run the re-scan tests to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::closure`
Expected: FAIL with "no method named `verify_serialized`".

- [ ] **Step 7: Write the re-scan**

```rust
/// Per-device object-kind and property-id maps. Supplied by the caller
/// because both are discovered per device and this module issues no ioctl.
#[derive(Debug, Clone, Default)]
pub struct ObjectKinds(BTreeMap<u32, AtomicObject>);

impl ObjectKinds {
    pub fn from_pairs(pairs: impl IntoIterator<Item = (u32, AtomicObject)>) -> Self {
        Self(pairs.into_iter().collect())
    }
    fn kind(&self, object: u32) -> Result<AtomicObject, ClosureError> {
        self.0.get(&object).copied().ok_or(ClosureError::UnknownObject(object))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PropertyIds {
    pub crtc_id: u32,
    pub active: u32,
    pub out_fence_ptr: u32,
}

impl AtomicCrtcClosure {
    pub fn verify_serialized(
        &self,
        props: &AtomicPropertyList,
        kinds: &ObjectKinds,
        ids: &PropertyIds,
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
        let mut fenced: BTreeSet<u32> = BTreeSet::new();
        let mut cursor = 0usize;
        for (index, object) in props.objects.iter().enumerate() {
            let count = props.count_props[index] as usize;
            let kind = kinds.kind(*object)?;
            if let AtomicObject::Crtc(id) = kind {
                serialized.insert(id);
            }
            for offset in 0..count {
                let prop = props.props[cursor + offset];
                let value = props.values[cursor + offset];
                if prop == ids.crtc_id
                    && matches!(kind, AtomicObject::Connector(_) | AtomicObject::Plane(_))
                    && value != 0
                {
                    // Only the new binding is visible in a serialized list; the
                    // old one is kernel state. The recorded closure carries
                    // both, so a detach shows up as a recorded member absent
                    // here and is caught by the comparison below rather than
                    // being silently forgiven.
                    serialized.insert(value as u32);
                }
                if prop == ids.out_fence_ptr {
                    if let AtomicObject::Crtc(id) = kind {
                        fenced.insert(id);
                    }
                }
            }
            cursor += count;
        }

        let serialized: Vec<u32> = serialized.into_iter().collect();
        if !serialized.iter().all(|c| self.closure.contains(c)) {
            return Err(ClosureError::SerializedClosureDiffers {
                recorded: self.closure.clone(),
                serialized,
            });
        }

        let fenced: Vec<u32> = fenced.into_iter().collect();
        if fenced != self.expected_completion {
            return Err(ClosureError::OutFenceCoverageDiffers {
                expected: self.expected_completion.clone(),
                found: fenced,
            });
        }
        Ok(())
    }
}
```

The closure comparison is containment, not equality: a serialized list cannot show an old `CRTC_ID` binding, so a detach legitimately serializes fewer members than were recorded. What it must never do is show a member that was *not* recorded, which is the direction that changes what the kernel will touch. The out-fence check is equality, because that set is entirely ours to construct.

- [ ] **Step 8: Run the re-scan tests to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::closure`
Expected: PASS, 14 tests.

- [ ] **Step 9: Commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/owner/closure.rs crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): compute the section 6.3 atomic CRTC closure and re-scan it"
```

---

## Task 2: The commit record and its milestones

**Files:**
- Create: `crates/yserver/src/kms/owner/record.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: `AtomicCrtcClosure` (Task 1); `CommitId`, `EventToken`, `IncarnationId`, `ClockEpochId` from `kms::owner::identity`; `LifecycleEpochId`, `LifecycleTransitionId` from `kms::owner::lifecycle`; `HostCallCorrelation` from `kms::executor::protocol`; `UnknownReason` from `kms::executor`.
- Produces: `Milestones`, `TerminalState`, `FailureCause`, `UnknownCause`, `RecordState`, `CommitRecord`, `Tombstone`, and `CommitRecord::{new, milestones, closure, correlation, state, mark_dispatched, mark_accepted, terminalize, tombstone}`.

- [ ] **Step 1: Write the failing milestone and terminal tests**

```rust
#[test]
fn a_new_record_is_submitting_with_no_milestone_but_producer_ready() {
    // spec:2114-2118 — ProducerReady precedes admission; a record only exists
    // once every source dependency completed.
    let r = fixture_record();
    assert!(matches!(r.state(), RecordState::Submitting));
    assert!(r.milestones().producer_ready);
    assert!(!r.milestones().dispatched);
    assert!(!r.milestones().accepted);
}

#[test]
fn dispatch_and_acceptance_are_independently_typed_milestones() {
    // spec:2124-2130 — code records both and may not infer either from the
    // other. `Dispatched` belongs at send time, `Accepted` at ioctl success.
    let mut r = fixture_record();
    r.mark_dispatched();
    assert!(r.milestones().dispatched);
    assert!(!r.milestones().accepted, "send is not acceptance");
    r.mark_accepted();
    assert!(r.milestones().accepted);
}

#[test]
fn acceptance_is_not_a_terminal_state() {
    // spec:1996-2000 — Completed requires the section 6.3 evidence for the
    // class, which for every C.0 class is successful out-fence status on top
    // of acceptance. 2b-i can observe none of it.
    let mut r = fixture_record();
    r.mark_dispatched();
    r.mark_accepted();
    assert!(matches!(r.state(), RecordState::Submitting));
}

#[test]
fn acceptance_never_sets_a_completion_milestone() {
    // spec:2137-2139 — a page event never closes an out-fence and an
    // out-fence never completes or timestamps Present. Neither is implied by
    // the ioctl returning success.
    let mut r = fixture_record();
    r.mark_dispatched();
    r.mark_accepted();
    assert!(!r.milestones().hardware_complete);
    assert!(!r.milestones().presented);
    assert!(!r.milestones().prior_buffer_released);
}

#[test]
fn a_record_reaches_exactly_one_terminal_state() {
    let mut r = fixture_record();
    r.mark_dispatched();
    r.terminalize(TerminalState::CompletionUnknown(UnknownCause::HostCall(
        UnknownReason::WatchdogExpired,
    )));
    let first = match r.state() {
        RecordState::Terminal(t) => *t,
        other => panic!("expected terminal, got {other:?}"),
    };
    // A later explicit rejection is accepted-stale and must not rewrite it.
    r.terminalize(TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected {
        errno: libc::EBUSY,
    }));
    assert_eq!(
        match r.state() { RecordState::Terminal(t) => *t, other => panic!("{other:?}") },
        first,
        "a terminalized record must not be terminalized a second time"
    );
}

#[test]
fn a_tombstone_keeps_identity_and_sets_but_owns_no_resource() {
    // spec:1699-1701 — tombstones retain kernel-event, Present-event and
    // observed CRTC sets plus terminal state, but own no KMS resource.
    let mut r = fixture_record();
    r.mark_dispatched();
    r.terminalize(TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected {
        errno: libc::EINVAL,
    }));
    let t = r.tombstone().expect("a terminalized record tombstones");
    assert_eq!(t.commit, r.commit_id());
    assert_eq!(t.event_token, r.event_token());
    assert_eq!(t.kernel_event_crtcs, r.closure().kernel_event().to_vec());
    assert_eq!(t.present_event_crtcs, r.closure().present_event().to_vec());
    assert!(matches!(t.terminal, TerminalState::FailedBeforeSubmit(_)));
}

#[test]
fn a_record_that_never_dispatched_cannot_tombstone_as_accepted() {
    let mut r = fixture_record();
    r.terminalize(TerminalState::FailedBeforeSubmit(FailureCause::CancelledBeforeDispatch));
    let t = r.tombstone().expect("tombstone");
    assert!(matches!(
        t.terminal,
        TerminalState::FailedBeforeSubmit(FailureCause::CancelledBeforeDispatch)
    ));
}
```

`fixture_record()` builds a `CommitRecord` over a single-CRTC closure with `page_flip_event = true` and one Present consumer, using `CommitId::for_tests(1)` and `EventToken::tagged_for_tests(1)`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::record`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the record**

```rust
// crates/yserver/src/kms/owner/record.rs

//! The section 10 commit record.
//!
//! A record is created only after `ProducerReady`, which is why that milestone
//! is set at construction rather than by a setter: section 10.2's table says
//! an intent becomes eligible for device admission at producer success, and a
//! record that could exist without it would be a queued intent, which is 2c's.

// ... the vocabulary types from the contract section, verbatim ...

#[derive(Debug, Clone)]
pub struct Tombstone {
    pub commit: CommitId,
    pub event_token: EventToken,
    pub incarnation: IncarnationId,
    pub lifecycle_epoch: LifecycleEpochId,
    pub kernel_event_crtcs: Vec<u32>,
    pub present_event_crtcs: Vec<u32>,
    pub observed_crtcs: Vec<u32>,
    pub terminal: TerminalState,
}

#[derive(Debug)]
pub struct CommitRecord {
    commit: CommitId,
    event_token: EventToken,
    incarnation: IncarnationId,
    lifecycle_epoch: LifecycleEpochId,
    transition: Option<LifecycleTransitionId>,
    topology_generation: u64,
    closure: AtomicCrtcClosure,
    correlation: HostCallCorrelation,
    milestones: Milestones,
    ledger: ResourceLedger,
    observed_crtcs: Vec<u32>,
    /// Built by `begin`, consumed by `send_on`.
    pending_request: Option<(AtomicRequest, SubmittingProof)>,
    /// Adopted from an `Accepted` reply. 2b-ii's evidence; held here so the
    /// record's `Drop` is the single exactly-once close site.
    adopted_fences: Vec<OwnedFd>,
    state: RecordState,
}

impl CommitRecord {
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
        ledger: ResourceLedger,
    ) -> Self {
        Self {
            commit,
            event_token,
            incarnation,
            lifecycle_epoch,
            transition,
            topology_generation,
            closure,
            correlation,
            milestones: Milestones { producer_ready: true, ..Milestones::default() },
            ledger,
            observed_crtcs: Vec::new(),
            pending_request: None,
            adopted_fences: Vec::new(),
            state: RecordState::Submitting,
        }
    }

    pub fn commit_id(&self) -> CommitId { self.commit }
    pub fn event_token(&self) -> EventToken { self.event_token }
    pub fn closure(&self) -> &AtomicCrtcClosure { &self.closure }
    pub fn correlation(&self) -> HostCallCorrelation { self.correlation }
    pub fn milestones(&self) -> &Milestones { &self.milestones }
    pub fn ledger(&self) -> &ResourceLedger { &self.ledger }
    pub fn ledger_mut(&mut self) -> &mut ResourceLedger { &mut self.ledger }
    pub fn state(&self) -> &RecordState { &self.state }

    /// Set at `send` return, not at reply. The `Submitting` record and event
    /// identity are installed and IPC was sent; nothing about the kernel's
    /// answer is known yet.
    pub fn mark_dispatched(&mut self) { self.milestones.dispatched = true; }

    /// Set only by an explicit `HostCallOutcome::Accepted`. It never implies
    /// `hardware_complete` or `presented`, which 2b-ii sets from a fence and
    /// an event respectively.
    pub fn mark_accepted(&mut self) { self.milestones.accepted = true; }

    /// The first terminal state wins. A later explicit rejection is
    /// accepted-stale under spec:2205-2210 and reconciles the ledger without
    /// rewriting the outcome.
    pub fn terminalize(&mut self, terminal: TerminalState) {
        if matches!(self.state, RecordState::Terminal(_)) {
            return;
        }
        if matches!(terminal, TerminalState::CompletionUnknown(_)) {
            self.ledger.quarantine();
        }
        self.state = RecordState::Terminal(terminal);
    }

    /// Hold the built request and its proof between `begin` and `send_on`.
    /// The request is immutable from here: spec line 578 forbids mutating it
    /// after the record is installed.
    pub fn attach_request(&mut self, request: AtomicRequest, proof: SubmittingProof) {
        self.pending_request = Some((request, proof));
    }

    /// Consume it, exactly once. A second `send_on` finds `None` and errors
    /// rather than sending the same request twice under one reservation.
    pub fn take_request(&mut self) -> Option<(AtomicRequest, SubmittingProof)> {
        self.pending_request.take()
    }

    /// Adopt the out-fences an accepted reply carried, without querying or
    /// closing them. 2b-ii replaces the body with fence-status evidence; until
    /// then they are held, because closing an unqueried fence would discard
    /// the only proof of hardware completion, and dropping the record would
    /// close them through `OwnedFd` at an arbitrary point.
    pub fn quarantine_fences(&mut self, fences: Vec<OwnedFd>) {
        self.adopted_fences.extend(fences);
    }

    /// What 2b-ii queries. Empty until an `Accepted` outcome arrives.
    pub fn adopted_fences(&self) -> &[OwnedFd] {
        &self.adopted_fences
    }

    /// An identity-only tombstone. It carries no ledger, which is the point:
    /// the ring may evict it and an evicted duplicate becomes `unknown`, which
    /// is telemetry-only and must never drop a resource.
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

`transition` and `topology_generation` are stored and not yet read. They are in the record because spec lines 1687-1692 require the record to supply `device_generation` and the lifecycle identities to a resolved event, and 2b-ii's correlation reads exactly these fields. Adding them later would mean touching every construction site.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::record`
Expected: PASS, 7 tests. `ResourceLedger` does not exist yet — declare it in Task 3 and land Tasks 2 and 3 in one commit if the borrow does not compile on its own; do not stub it.

Because `CommitRecord` cannot compile without `ResourceLedger`, **write Task 3's module before running this step**, then return here. This is the one place in this plan where two tasks share a compile unit, and it is called out rather than hidden behind a placeholder type that would then survive into the tree.

---

## Task 3: The owned-resource ledger

**Files:**
- Create: `crates/yserver/src/kms/owner/ledger.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `ResourceLedger`, `LedgerDisposition`, `OwnedResource`, and `ResourceLedger::{new, own_old, own_new, disposition, quarantine, resolve_accepted, resolve_rejected, released_now}`.

- [ ] **Step 1: Write the failing ledger tests**

```rust
#[test]
fn a_submitting_ledger_uncertainty_owns_both_possible_states() {
    // spec:2141-2143 — the record uncertainty-owns every possible old/new KMS,
    // framebuffer, blob, BO, pin, descriptor and external-ownership state.
    let l = fixture_ledger();
    assert_eq!(l.disposition(), LedgerDisposition::UncertaintyOwned);
    assert!(l.released_now().is_empty());
}

#[test]
fn an_explicit_rejection_releases_the_new_state_and_keeps_the_old_current() {
    // spec:2146-2148 — new KMS state is not current; copied/direct BO state
    // follows atomic-rejected recovery.
    let mut l = fixture_ledger();
    l.resolve_rejected();
    assert_eq!(l.disposition(), LedgerDisposition::RejectedNewReleased);
    assert_eq!(l.released_now(), &[OwnedResource::NewFramebuffer(77)]);
}

#[test]
fn acceptance_releases_nothing_yet() {
    // spec:2149-2151 — the pending record owns all possible old/new state
    // after acceptance; release waits for the class-specific replacement rule
    // and PriorBufferReleased, neither of which 2b-i can observe.
    let mut l = fixture_ledger();
    l.resolve_accepted();
    assert_eq!(l.disposition(), LedgerDisposition::AcceptedBothOwned);
    assert!(l.released_now().is_empty());
}

#[test]
fn quarantine_releases_nothing_and_is_not_reversible() {
    // spec:2160-2162 — quarantine both possible state/resource sets and all
    // external ownership ledgers until the section 10 teardown barrier.
    let mut l = fixture_ledger();
    l.quarantine();
    assert_eq!(l.disposition(), LedgerDisposition::Quarantined);
    assert!(l.released_now().is_empty());
    l.resolve_rejected();
    assert_eq!(
        l.disposition(),
        LedgerDisposition::Quarantined,
        "a later rejection must not un-quarantine an unknown outcome"
    );
    assert!(l.released_now().is_empty());
}

#[test]
fn a_quarantined_ledger_lists_every_resource_it_still_holds() {
    let mut l = fixture_ledger();
    l.quarantine();
    let held = l.quarantined();
    assert!(held.contains(&OwnedResource::OldFramebuffer(66)));
    assert!(held.contains(&OwnedResource::NewFramebuffer(77)));
}
```

`fixture_ledger()` owns `OwnedResource::OldFramebuffer(66)` and `OwnedResource::NewFramebuffer(77)`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::ledger`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the ledger**

```rust
// crates/yserver/src/kms/owner/ledger.rs

//! Section 10.2's ownership table as state.
//!
//! The ledger names resources; it does not close descriptors or drop
//! framebuffers. `released_now()` is the list a caller must act on, which
//! keeps the exactly-once cleanup rule enforceable at one site instead of
//! spread across every terminal path.

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum OwnedResource {
    OldFramebuffer(u32),
    NewFramebuffer(u32),
    OldGammaBlob(u32),
    NewGammaBlob(u32),
    CursorFramebuffer(u32),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LedgerDisposition {
    /// Between dispatch and a typed outcome. Cancellation can no longer
    /// classify the request as never-submitted.
    UncertaintyOwned,
    /// An explicit ioctl rejection: new state was never current.
    RejectedNewReleased,
    /// Accepted: both possible states remain owned until the replacement rule
    /// and `PriorBufferReleased` allow a release. 2b-ii owns that transition.
    AcceptedBothOwned,
    /// Acceptance could not be established or disproved.
    Quarantined,
}

#[derive(Debug, Default)]
pub struct ResourceLedger {
    old: Vec<OwnedResource>,
    new: Vec<OwnedResource>,
    disposition: Option<LedgerDisposition>,
    released_now: Vec<OwnedResource>,
}

impl ResourceLedger {
    pub fn new() -> Self {
        Self { disposition: Some(LedgerDisposition::UncertaintyOwned), ..Self::default() }
    }
    pub fn own_old(&mut self, r: OwnedResource) { self.old.push(r); }
    pub fn own_new(&mut self, r: OwnedResource) { self.new.push(r); }

    pub fn disposition(&self) -> LedgerDisposition {
        self.disposition.unwrap_or(LedgerDisposition::UncertaintyOwned)
    }
    pub fn released_now(&self) -> &[OwnedResource] { &self.released_now }

    pub fn quarantined(&self) -> Vec<OwnedResource> {
        if self.disposition() != LedgerDisposition::Quarantined {
            return Vec::new();
        }
        self.old.iter().chain(self.new.iter()).copied().collect()
    }

    /// Quarantine is absorbing. A later explicit result is accepted-stale
    /// under spec:2205-2210: it reconciles telemetry, it does not release a
    /// resource whose reachability was never disproved.
    pub fn quarantine(&mut self) {
        self.disposition = Some(LedgerDisposition::Quarantined);
        self.released_now.clear();
    }

    pub fn resolve_accepted(&mut self) {
        if self.disposition() == LedgerDisposition::Quarantined { return; }
        self.disposition = Some(LedgerDisposition::AcceptedBothOwned);
    }

    pub fn resolve_rejected(&mut self) {
        if self.disposition() == LedgerDisposition::Quarantined { return; }
        self.disposition = Some(LedgerDisposition::RejectedNewReleased);
        self.released_now = self.new.clone();
    }
}
```

- [ ] **Step 4: Run the ledger and record tests to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::`
Expected: PASS — 5 ledger tests plus Task 2's 7 record tests.

- [ ] **Step 5: Commit tasks 2 and 3 together**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/owner/record.rs crates/yserver/src/kms/owner/ledger.rs \
        crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): install commit records that uncertainty-own both resource states"
```

---

## Task 4: The single device slot

**Files:**
- Create: `crates/yserver/src/kms/owner/slot.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs`, `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: `CommitId`; `SubmittingProof` and `ValidationLease` from `kms::executor`.
- Produces: `DeviceSlot`, `SlotError`, and `DeviceSlot::{reserve, release, acquire_validation, release_validation, occupant, validation_outstanding}`.

- [ ] **Step 1: Write the failing slot tests**

```rust
#[test]
fn one_commit_may_hold_the_device_slot() {
    // spec:1328-1331 — exactly one dispatched-or-submitted live atomic
    // transaction per DRM device, not one per CRTC.
    let mut slot = DeviceSlot::default();
    let _proof = slot.reserve(CommitId::for_tests(1)).expect("first reservation");
    let err = slot.reserve(CommitId::for_tests(2)).expect_err("second must be refused");
    assert_eq!(err, SlotError::AlreadyOccupied(CommitId::for_tests(1)));
}

#[test]
fn the_slot_is_not_released_by_a_late_result() {
    // spec:1330-1332 — the slot is not released merely because the ioctl
    // result is late. Only the holder releases it, by id.
    let mut slot = DeviceSlot::default();
    let _proof = slot.reserve(CommitId::for_tests(1)).expect("reserve");
    let err = slot.release(CommitId::for_tests(2)).expect_err("a stranger cannot release");
    assert_eq!(err, SlotError::NotHeld(CommitId::for_tests(2)));
    assert_eq!(slot.occupant(), Some(CommitId::for_tests(1)));
}

#[test]
fn validation_does_not_occupy_the_submitted_commit_slot() {
    // ValidationOnly: TEST_ONLY holds an exclusive owner validation lease, not
    // a Submitting record, and does not occupy the submitted-commit slot.
    let mut slot = DeviceSlot::default();
    let _lease = slot.acquire_validation(CommitId::for_tests(1)).expect("validation lease");
    assert_eq!(slot.occupant(), None);
    let _proof = slot.reserve(CommitId::for_tests(2)).expect("a commit is still admissible");
}

#[test]
fn the_validation_lease_is_exclusive() {
    let mut slot = DeviceSlot::default();
    let _lease = slot.acquire_validation(CommitId::for_tests(1)).expect("first lease");
    let err = slot
        .acquire_validation(CommitId::for_tests(2))
        .expect_err("a second lease must be refused");
    assert_eq!(err, SlotError::ValidationOutstanding);
}

#[test]
fn releasing_and_reserving_again_is_permitted() {
    let mut slot = DeviceSlot::default();
    let _proof = slot.reserve(CommitId::for_tests(1)).expect("reserve");
    slot.release(CommitId::for_tests(1)).expect("release");
    assert_eq!(slot.occupant(), None);
    let _proof = slot.reserve(CommitId::for_tests(2)).expect("re-reserve");
}

#[test]
fn a_submitting_proof_cannot_be_obtained_without_reserving() {
    // The production constructor is pub(crate) and lives in this module, so
    // this is a compile-level guarantee. The test documents where the
    // guarantee comes from; `compile_fail.rs` is where it is enforced.
    let mut slot = DeviceSlot::default();
    let proof = slot.reserve(CommitId::for_tests(1)).expect("reserve");
    drop(proof);
    // Dropping the proof does not release the slot: only `release` does,
    // because a dropped proof is not evidence the kernel finished.
    assert_eq!(slot.occupant(), Some(CommitId::for_tests(1)));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::slot`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Add the production constructors to 2a's proof types**

In `crates/yserver/src/kms/executor/mod.rs`, beside each existing `for_tests`:

```rust
impl SubmittingProof {
    /// Produced only by `DeviceSlot::reserve`. Crate-visible so the slot is
    /// the sole issuer: a caller that has not reserved cannot name one.
    pub(crate) const fn from_reservation() -> Self { Self(()) }
}

impl ValidationLease {
    /// Produced only by `DeviceSlot::acquire_validation`.
    pub(crate) const fn from_reservation() -> Self { Self(()) }
}
```

- [ ] **Step 4: Write the slot**

```rust
// crates/yserver/src/kms/owner/slot.rs

//! The section 9 single device slot.
//!
//! Release is by `CommitId` rather than by dropping a guard. A guard would
//! release on unwind and on every early return, and the one thing this slot
//! must not do is free itself because a result was late — spec:1330-1332.

use crate::kms::executor::{SubmittingProof, ValidationLease};
use crate::kms::owner::identity::CommitId;

// ... DeviceSlot and SlotError from the contract section, verbatim ...

impl DeviceSlot {
    pub fn reserve(&mut self, commit: CommitId) -> Result<SubmittingProof, SlotError> {
        if let Some(held) = self.occupant {
            return Err(SlotError::AlreadyOccupied(held));
        }
        self.occupant = Some(commit);
        Ok(SubmittingProof::from_reservation())
    }

    pub fn release(&mut self, commit: CommitId) -> Result<(), SlotError> {
        match self.occupant {
            Some(held) if held == commit => {
                self.occupant = None;
                Ok(())
            }
            _ => Err(SlotError::NotHeld(commit)),
        }
    }

    pub fn acquire_validation(&mut self, commit: CommitId) -> Result<ValidationLease, SlotError> {
        if self.validation.is_some() {
            return Err(SlotError::ValidationOutstanding);
        }
        self.validation = Some(commit);
        Ok(ValidationLease::from_reservation())
    }

    pub fn release_validation(&mut self, commit: CommitId) -> Result<(), SlotError> {
        match self.validation {
            Some(held) if held == commit => {
                self.validation = None;
                Ok(())
            }
            _ => Err(SlotError::NotHeld(commit)),
        }
    }

    pub fn occupant(&self) -> Option<CommitId> { self.occupant }
    pub fn validation_outstanding(&self) -> bool { self.validation.is_some() }
}
```

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::slot`
Expected: PASS, 6 tests.

- [ ] **Step 6: Commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/owner/slot.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/src/kms/executor/mod.rs
git commit -m "feat(kms): reserve the one device slot and issue its submitting proof"
```

---

## Task 5: Request construction

**Files:**
- Create: `crates/yserver/src/kms/owner/build.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: `AtomicCrtcClosure`, `ObjectKinds`, `PropertyIds`, `ClosureError` (Task 1); `AtomicRequest`, `AtomicPropertyList`, `OutFenceSlot`, `HostCallCorrelation`, `HostCallClass` from `kms::executor::protocol`.
- Produces: `CommitDescription`, `BuildError`, and `build_atomic_request(desc, correlation, class) -> Result<(AtomicRequest, AtomicCrtcClosure), BuildError>`.

A `CommitDescription` is the persistent property list plus the powered-state rows plus the Present consumers — everything the closure needs and nothing else. It is what 2c's converted call sites will produce; here its only producers are tests.

- [ ] **Step 1: Write the failing construction tests**

```rust
// crates/yserver/src/kms/owner/build.rs  (#[cfg(test)] mod tests)

#[test]
fn one_out_fence_property_is_added_for_each_expected_completion_crtc() {
    // spec:565-568 — exactly one OUT_FENCE_PTR for every member of
    // ExpectedCompletionCrtcs and none outside it.
    let desc = two_crtcs_one_off();
    let (req, closure) =
        build_atomic_request(&desc, atomic_correlation(1), HostCallClass::SeatActiveNonblock)
            .expect("build");
    assert_eq!(closure.expected_completion(), &[1]);
    assert_eq!(req.out_fence_slots.len(), 1);
    assert_eq!(req.out_fence_slots[0].crtc_id, 1);
}

#[test]
fn every_out_fence_slot_indexes_the_value_the_helper_will_overwrite() {
    // 2a's helper writes the returned fd into properties.values[value_index].
    // An index that does not point at this CRTC's OUT_FENCE_PTR value would
    // corrupt an unrelated property rather than fail.
    let desc = single_active_crtc();
    let (req, _) =
        build_atomic_request(&desc, atomic_correlation(1), HostCallClass::SeatActiveNonblock)
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
fn a_validation_request_carries_no_out_fence_at_all() {
    // ValidationOnly creates no out-fence. Its holder storage would be
    // returned as -1, which spec:1958-1962 makes valid only after a rejected
    // ioctl or TEST_ONLY — so it must not be requested in the first place.
    let desc = single_active_crtc();
    let (req, _) = build_atomic_request(
        &desc,
        atomic_correlation(1),
        HostCallClass::SeatActiveValidation,
    )
    .expect("build");
    assert!(req.out_fence_slots.is_empty());
    assert_eq!(req.flags & crate::kms::executor::protocol::DRM_MODE_ATOMIC_TEST_ONLY, 
               crate::kms::executor::protocol::DRM_MODE_ATOMIC_TEST_ONLY);
}

#[test]
fn a_seat_active_commit_carries_nonblock_and_a_validation_never_does() {
    // COMMIT-5 — during seat-active service every live commit uses NONBLOCK.
    let desc = single_active_crtc();
    let (live, _) =
        build_atomic_request(&desc, atomic_correlation(1), HostCallClass::SeatActiveNonblock)
            .expect("build");
    assert_ne!(live.flags & crate::kms::executor::protocol::DRM_MODE_ATOMIC_NONBLOCK, 0);
    let (test, _) = build_atomic_request(
        &desc,
        atomic_correlation(2),
        HostCallClass::SeatActiveValidation,
    )
    .expect("build");
    assert_eq!(test.flags & crate::kms::executor::protocol::DRM_MODE_ATOMIC_NONBLOCK, 0);
}

#[test]
fn the_page_event_flag_reaches_the_request_when_the_description_asks_for_it() {
    let mut desc = single_active_crtc();
    desc.page_flip_event = true;
    let (req, closure) =
        build_atomic_request(&desc, atomic_correlation(1), HostCallClass::SeatActiveNonblock)
            .expect("build");
    assert_ne!(req.flags & crate::kms::executor::protocol::DRM_MODE_PAGE_FLIP_EVENT, 0);
    assert_eq!(closure.kernel_event(), &[1]);
}

#[test]
fn construction_fails_when_the_serialized_list_disagrees_with_the_closure() {
    // spec:569-570 — the re-scan runs on the bytes about to be sent, so a
    // description whose property list mentions a CRTC its entries omit cannot
    // reach the executor.
    let desc = description_with_a_smuggled_crtc();
    let err = build_atomic_request(
        &desc,
        atomic_correlation(1),
        HostCallClass::SeatActiveNonblock,
    )
    .expect_err("the re-scan must refuse it");
    assert!(matches!(err, BuildError::Closure(ClosureError::SerializedClosureDiffers { .. })));
}

#[test]
fn construction_fails_before_submit_on_an_off_to_off_crtc_with_a_page_event() {
    let mut desc = two_crtcs_one_off();
    desc.page_flip_event = true;
    let err = build_atomic_request(
        &desc,
        atomic_correlation(1),
        HostCallClass::SeatActiveNonblock,
    )
    .expect_err("off-to-off plus page event must not build");
    assert!(matches!(err, BuildError::Closure(ClosureError::OffToOffWithPageEvent(2))));
}

#[test]
fn the_request_carries_the_commits_event_token_as_user_data() {
    // spec:1673-1678 — user_data carries the commit's EventToken verbatim.
    // 2a puts the correlation on the wire, and the helper writes its
    // event_token into drm_mode_atomic.user_data; this asserts the tuple the
    // owner hands over is the record's, not a fresh one.
    let desc = single_active_crtc();
    let correlation = atomic_correlation(7);
    let (req, _) =
        build_atomic_request(&desc, correlation, HostCallClass::SeatActiveNonblock)
            .expect("build");
    assert_eq!(req.correlation, correlation);
}

#[test]
fn a_property_list_over_the_wire_limit_fails_construction_not_encoding() {
    let desc = description_with_too_many_properties();
    let err = build_atomic_request(
        &desc,
        atomic_correlation(1),
        HostCallClass::SeatActiveNonblock,
    )
    .expect_err("an oversized list must fail here");
    assert!(matches!(err, BuildError::Protocol(_)));
}
```

Helpers in the test module: `single_active_crtc()` returns a `CommitDescription` over CRTC 1 with `ACTIVE=1` and one bound plane; `two_crtcs_one_off()` adds CRTC 2 with `old_active=false, new_active=false`; `description_with_a_smuggled_crtc()` puts CRTC 2 in `serialized` without an entry for it; `description_with_too_many_properties()` exceeds `MAX_ATOMIC_PROPS`; `flatten(&AtomicPropertyList)` returns a `Vec<(object, prop)>` parallel to `values`; `atomic_correlation(commit)` builds a `HostCallCorrelation::Atomic` with `CommitId::for_tests(commit)` and `EventToken::tagged_for_tests(commit)`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::build`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the builder**

```rust
// crates/yserver/src/kms/owner/build.rs

//! Turning a commit description into the exact request 2a puts on the wire.
//!
//! Order matters and is not an implementation detail. The closure is computed
//! from the persistent entries *before* any completion property exists, then
//! the out-fence entries are appended, then the serialized list is re-scanned.
//! Computing the closure after appending would let an ephemeral out-fence
//! entry enlarge it, which spec:592-595 forbids by name.

use crate::kms::executor::protocol::{
    self, AtomicPropertyList, AtomicRequest, HostCallClass, HostCallCorrelation, OutFenceSlot,
    ProtocolError,
};
use crate::kms::owner::closure::{
    AtomicCrtcClosure, ClosureError, CrtcPower, ObjectKinds, PersistentEntry, PropertyIds,
};

/// One object's serialized persistent properties, in wire order.
#[derive(Debug, Clone)]
pub struct SerializedObject {
    pub object: u32,
    pub props: Vec<(u32, u64)>,
}

/// Everything the owner needs to build one request. 2c's converted call sites
/// produce this; here its only producers are tests.
#[derive(Debug, Clone)]
pub struct CommitDescription {
    pub entries: Vec<PersistentEntry>,
    pub power: Vec<CrtcPower>,
    pub present_consumers: Vec<u32>,
    pub page_flip_event: bool,
    pub serialized: Vec<SerializedObject>,
    pub object_kinds: ObjectKinds,
    pub property_ids: PropertyIds,
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
    let closure = AtomicCrtcClosure::compute(
        &desc.entries,
        &desc.power,
        desc.page_flip_event,
        &desc.present_consumers,
    )?;

    // A validation request touches no hardware and creates no out-fence, so it
    // gets neither the fence properties nor the slot table.
    let wants_fences = !class.is_validation();

    let mut objects: Vec<u32> = Vec::new();
    let mut count_props: Vec<u32> = Vec::new();
    let mut props: Vec<u32> = Vec::new();
    let mut values: Vec<u64> = Vec::new();
    let mut out_fence_slots: Vec<OutFenceSlot> = Vec::new();

    for object in &desc.serialized {
        let is_expected_crtc = closure.expected_completion().contains(&object.object)
            && matches!(
                desc.object_kinds.kind(object.object),
                Ok(crate::kms::owner::closure::AtomicObject::Crtc(_))
            );
        let extra = usize::from(wants_fences && is_expected_crtc);

        objects.push(object.object);
        count_props.push((object.props.len() + extra) as u32);
        for (prop, value) in &object.props {
            props.push(*prop);
            values.push(*value);
        }
        if extra == 1 {
            // The holder is initialized to -1 and the helper overwrites this
            // exact index with the returned sync-file fd. spec:1955-1957.
            out_fence_slots.push(OutFenceSlot {
                crtc_id: object.object,
                value_index: values.len() as u32,
            });
            props.push(desc.property_ids.out_fence_ptr);
            values.push(u64::MAX); // encodes -1 as an s32 holder
        }
    }

    let properties = AtomicPropertyList { objects, count_props, props, values };
    properties.validate().map_err(BuildError::Protocol)?;
    closure.verify_serialized(&properties, &desc.object_kinds, &desc.property_ids)?;

    let mut flags = match class {
        HostCallClass::SeatActiveNonblock => protocol::DRM_MODE_ATOMIC_NONBLOCK,
        HostCallClass::SeatActiveValidation | HostCallClass::ColdStartOrOfflineValidation => {
            protocol::DRM_MODE_ATOMIC_TEST_ONLY
        }
        HostCallClass::ColdStartOrOfflineBlocking => 0,
    };
    if desc.page_flip_event && wants_fences {
        flags |= protocol::DRM_MODE_PAGE_FLIP_EVENT;
    }

    Ok((
        AtomicRequest { correlation, class, flags, properties, out_fence_slots },
        closure,
    ))
}
```

`ObjectKinds::kind` must become `pub` for this call; change its visibility in Task 1's module rather than duplicating the map here.

If `protocol::DRM_MODE_PAGE_FLIP_EVENT` does not already exist beside the two flags 2a defined, add it as `pub const DRM_MODE_PAGE_FLIP_EVENT: u32 = 0x01;` with the same doc-comment style as its neighbours, and extend 2a's decoder flag check to accept it for the two non-validation classes only. A validation request carrying a page-event flag is a protocol error, not a tolerated combination.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::build`
Expected: PASS, 9 tests.

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/owner/build.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/src/kms/owner/closure.rs crates/yserver/src/kms/executor/protocol.rs
git commit -m "feat(kms): build the atomic request its closure describes and re-scan it"
```

---

## Task 6: The device owner and its typed outcome stream

**Files:**
- Create: `crates/yserver/src/kms/owner/device.rs`
- Create: `crates/yserver/tests/owner_commit_record.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: every type from Tasks 1-5; `KmsIoExecutor`, `HostCallEvent`, `HostCallOutcome`, `UnknownReason`, `HostCallReservation`, `SendError` from `kms::executor`.
- Produces: `DeviceCommitOwner`, `DispatchError`, and `DeviceCommitOwner::{new, begin, send_on, dispatch, dispatch_validation, apply_host_call_event, slot, live_record, tombstones}`.

**Why `begin` and `send_on` are separate.** `COMMIT-6` orders the work: install the `Submitting` record and reserve the slot, *then* send IPC. Splitting the call at exactly that boundary makes the order a signature rather than a comment, and it lets every state test in this task run without spawning a helper process. `dispatch` is `begin` followed by `send_on` and is what production calls. The built `AtomicRequest` lives in the record between the two, which is also where spec line 578's "later request mutation is forbidden" wants it.

- [ ] **Step 1: Write the failing outcome-stream tests**

```rust
// crates/yserver/src/kms/owner/device.rs  (#[cfg(test)] mod tests)

#[test]
fn dispatch_reserves_the_slot_before_it_sends() {
    // COMMIT-6 — the owner installs a Submitting record and reserves the
    // device slot *before* executor IPC, so a send that fails still leaves a
    // record that owns the uncertainty.
    let mut owner = fixture_owner();
    let commit = owner.begin(&single_active_crtc(), fixture_ledger()).expect("begin");
    assert_eq!(owner.slot().occupant(), Some(commit));
    assert!(
        !owner.live_record().expect("record").milestones().dispatched,
        "the record and the slot exist before IPC, and `dispatched` marks the send"
    );
}

#[test]
fn a_second_dispatch_is_refused_while_a_record_lives() {
    // spec:1330-1334 — no second ioctl may be dispatched on the device while
    // this record or its executor lease exists.
    let mut owner = fixture_owner();
    owner.begin(&single_active_crtc(), fixture_ledger()).expect("first");
    let err = owner
        .begin(&single_active_crtc(), fixture_ledger())
        .expect_err("second must be refused");
    assert!(matches!(err, DispatchError::Slot(SlotError::AlreadyOccupied(_))));
}

#[test]
fn an_explicit_rejection_is_the_only_proof_of_failed_before_submit() {
    let mut owner = fixture_owner();
    let commit = owner.begin(&single_active_crtc(), fixture_ledger()).expect("begin");
    owner.apply_host_call_event(rejected_event(commit, libc::EBUSY));
    let t = owner.tombstones().last().expect("tombstoned");
    assert!(matches!(
        t.terminal,
        TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected { errno })
            if errno == libc::EBUSY
    ));
    assert_eq!(owner.slot().occupant(), None, "a proven rejection releases the slot");
}

#[test]
fn every_acceptance_unknown_reason_keeps_the_slot_held() {
    // COMMIT-6 — missing or invalid reply, helper exit, IPC failure and
    // watchdog expiry are acceptance-unknown. Releasing the slot on any of
    // them is exactly the defect the single-slot rule exists to prevent.
    for reason in [
        UnknownReason::WatchdogExpired,
        UnknownReason::HelperExited,
        UnknownReason::IpcFailure,
        UnknownReason::MalformedReply,
    ] {
        let mut owner = fixture_owner();
        let commit = owner.begin(&single_active_crtc(), fixture_ledger()).expect("begin");
        owner.apply_host_call_event(unknown_event(commit, reason));
        assert_eq!(
            owner.slot().occupant(),
            Some(commit),
            "{reason:?} released the device slot"
        );
        let t = owner.tombstones().last().expect("tombstoned");
        assert!(matches!(
            t.terminal,
            TerminalState::CompletionUnknown(UnknownCause::HostCall(r)) if r == reason
        ));
    }
}

#[test]
fn an_unknown_outcome_quarantines_the_ledger() {
    let mut owner = fixture_owner();
    let commit = owner.begin(&single_active_crtc(), fixture_ledger()).expect("begin");
    owner.apply_host_call_event(unknown_event(commit, UnknownReason::WatchdogExpired));
    let record = owner.live_record().expect("an unknown record is retained, not dropped");
    assert_eq!(record.ledger().disposition(), LedgerDisposition::Quarantined);
    assert!(!record.ledger().quarantined().is_empty());
}

#[test]
fn acceptance_is_recorded_and_does_not_complete_or_release() {
    // The whole point of the 2b split: acceptance is Accepted, not Completed.
    // 2b-ii supplies the fence status that section 6.3 requires on top of it.
    let mut owner = fixture_owner();
    let commit = owner.begin(&single_active_crtc(), fixture_ledger()).expect("begin");
    owner.apply_host_call_event(accepted_event(commit));
    let record = owner.live_record().expect("still live");
    assert!(record.milestones().accepted);
    assert!(!record.milestones().hardware_complete);
    assert!(matches!(record.state(), RecordState::Submitting));
    assert_eq!(owner.slot().occupant(), Some(commit));
    assert!(owner.tombstones().is_empty());
}

#[test]
fn an_uncorrelated_outcome_never_touches_the_live_record() {
    // ID-3 — a reply is current only when incarnation, lifecycle epoch,
    // optional transition id and commit id all match.
    let mut owner = fixture_owner();
    let commit = owner.begin(&single_active_crtc(), fixture_ledger()).expect("begin");
    owner.apply_host_call_event(rejected_event(CommitId::for_tests(999), libc::EINVAL));
    assert!(matches!(owner.live_record().expect("untouched").state(), RecordState::Submitting));
    assert_eq!(owner.slot().occupant(), Some(commit));
}

#[test]
fn a_late_reply_is_adopted_and_never_revives_a_terminalized_record() {
    // spec:2205-2210 — a later success remains accepted-stale and quarantined.
    let mut owner = fixture_owner();
    let commit = owner.begin(&single_active_crtc(), fixture_ledger()).expect("begin");
    owner.apply_host_call_event(unknown_event(commit, UnknownReason::WatchdogExpired));
    owner.apply_host_call_event(late_accepted_event(commit));
    let record = owner.live_record().expect("still retained");
    assert_eq!(record.ledger().disposition(), LedgerDisposition::Quarantined);
    assert!(
        !record.milestones().accepted,
        "a late acceptance must not promote a terminalized record"
    );
}

#[test]
fn the_tombstone_ring_keeps_the_last_sixty_four() {
    // spec:1697-1704 — the ring holds 64 identity-only tombstones; eviction
    // merely changes a very old duplicate from tombstoned to unknown.
    let mut owner = fixture_owner();
    let mut created = Vec::new();
    for _ in 0..70 {
        let commit = owner.begin(&single_active_crtc(), fixture_ledger()).expect("begin");
        created.push(commit);
        owner.apply_host_call_event(rejected_event(commit, libc::EINVAL));
    }
    assert_eq!(owner.tombstones().len(), 64);
    assert_eq!(
        owner.tombstones()[0].commit,
        created[6],
        "the ring drops the oldest six, not an arbitrary window"
    );
}

#[test]
fn a_validation_outcome_takes_no_record_and_no_slot() {
    // ValidationOnly — it holds an exclusive lease, not a Submitting record.
    let mut owner = fixture_owner();
    let commit = owner.begin_validation(&single_active_crtc()).expect("validate");
    assert_eq!(owner.slot().occupant(), None);
    assert!(owner.slot().validation_outstanding());
    owner.apply_host_call_event(validation_abandoned_event(commit));
    assert!(!owner.slot().validation_outstanding(), "the lease is released");
    assert!(owner.tombstones().is_empty(), "validation leaves no commit tombstone");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::device`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the owner**

```rust
// crates/yserver/src/kms/owner/device.rs

//! The device-local commit owner.
//!
//! It owns the slot, at most one live record, and a bounded tombstone ring.
//! It performs no ioctl: every kernel interaction goes through the 2a
//! executor's `send`/`poll_reply`/`tick`, which is what keeps `COMMIT-5`
//! structural rather than a convention.

const TOMBSTONE_RING_CAPACITY: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("slot: {0}")]
    Slot(#[from] SlotError),
    #[error("build: {0}")]
    Build(#[from] BuildError),
    #[error("send: {0:?}")]
    Send(SendError),
    #[error("identity space exhausted within this incarnation")]
    IdentityExhausted,
    #[error("no live record to send")]
    NoLiveRecord,
    #[error("this record's request was already sent")]
    AlreadySent,
}

#[derive(Debug)]
pub struct DeviceCommitOwner {
    slot: DeviceSlot,
    live: Option<CommitRecord>,
    /// The built-but-unsent validation request and its lease. A validation
    /// installs no record, so it cannot live in `live`.
    pending_validation: Option<(CommitId, AtomicRequest, ValidationLease)>,
    tombstones: VecDeque<Tombstone>,
    identities: IdentityAllocator,
    lifecycle_epoch: LifecycleEpochId,
    transition: Option<LifecycleTransitionId>,
    topology_generation: u64,
    next_seq: u64,
}

impl DeviceCommitOwner {
    /// Install the record and reserve the slot. No IPC happens here.
    pub fn begin(
        &mut self,
        desc: &CommitDescription,
        ledger: ResourceLedger,
    ) -> Result<CommitId, DispatchError> {
        let commit = self.identities.checked_next_commit().ok_or(DispatchError::IdentityExhausted)?;
        let event_token = self
            .identities
            .checked_next_event_token()
            .ok_or(DispatchError::IdentityExhausted)?;
        self.next_seq += 1;
        let correlation = HostCallCorrelation::Atomic {
            // `from_raw`, not `for_tests`: this is the production allocator.
            seq: RequestSeq::from_raw(self.next_seq),
            incarnation: self.identities.incarnation(),
            lifecycle_epoch: self.lifecycle_epoch,
            transition: self.transition,
            commit,
            event_token,
        };

        // Build first: a description that cannot produce a valid request must
        // not have consumed the slot. Construction failure is
        // FailedBeforeSubmit by definition — nothing was ever installed.
        let (request, closure) =
            build_atomic_request(desc, correlation, HostCallClass::SeatActiveNonblock)?;

        // Then reserve, then install, then send. COMMIT-6's order exactly: a
        // transport error must never find a device with no record.
        let proof = self.slot.reserve(commit)?;
        let mut record = CommitRecord::new(
            commit,
            event_token,
            self.identities.incarnation(),
            self.lifecycle_epoch,
            self.transition,
            self.topology_generation,
            closure,
            correlation,
            ledger,
        );

        record.attach_request(request, proof);
        self.live = Some(record);
        Ok(commit)
    }

    /// Send the request `begin` built. `dispatched` is set on return whether
    /// or not the send succeeded: 2a installs its `InFlight` before the write
    /// and queues a terminal event on transport error, so a failed send has
    /// still consumed the request and `apply_host_call_event` is what
    /// terminalizes it. Treating a failed send as never-dispatched would
    /// classify it `FailedBeforeSubmit`, which `COMMIT-6` permits only for an
    /// explicit ioctl rejection.
    pub fn send_on(&mut self, executor: &mut KmsIoExecutor) -> Result<(), DispatchError> {
        let record = self.live.as_mut().ok_or(DispatchError::NoLiveRecord)?;
        let (request, proof) = record.take_request().ok_or(DispatchError::AlreadySent)?;
        let result = executor.send(&request, HostCallReservation::Submitting(proof));
        record.mark_dispatched();
        result.map_err(DispatchError::Send)
    }

    /// `begin` then `send_on`. What production calls.
    pub fn dispatch(
        &mut self,
        desc: &CommitDescription,
        ledger: ResourceLedger,
        executor: &mut KmsIoExecutor,
    ) -> Result<CommitId, DispatchError> {
        let commit = self.begin(desc, ledger)?;
        self.send_on(executor)?;
        Ok(commit)
    }

    /// A `TEST_ONLY` request. It takes the exclusive validation lease, installs
    /// no record, and never touches the commit slot. The returned `CommitId`
    /// names the lease so `release_validation` can match it; it identifies no
    /// commit record because a validation has none.
    pub fn begin_validation(
        &mut self,
        desc: &CommitDescription,
    ) -> Result<CommitId, DispatchError> {
        let commit = self
            .identities
            .checked_next_commit()
            .ok_or(DispatchError::IdentityExhausted)?;
        let event_token = self
            .identities
            .checked_next_event_token()
            .ok_or(DispatchError::IdentityExhausted)?;
        self.next_seq += 1;
        let correlation = HostCallCorrelation::Atomic {
            seq: RequestSeq::from_raw(self.next_seq),
            incarnation: self.identities.incarnation(),
            lifecycle_epoch: self.lifecycle_epoch,
            transition: self.transition,
            commit,
            event_token,
        };
        let (request, _closure) =
            build_atomic_request(desc, correlation, HostCallClass::SeatActiveValidation)?;
        let lease = self.slot.acquire_validation(commit)?;
        self.pending_validation = Some((commit, request, lease));
        Ok(commit)
    }

    pub fn apply_host_call_event(&mut self, event: HostCallEvent) {
        let (correlation, outcome, late) = match event {
            HostCallEvent::Outcome { correlation, outcome } => (correlation, outcome, false),
            HostCallEvent::LateReply { correlation, outcome } => (correlation, outcome, true),
        };

        // A clock probe has no record. 2b-ii is its consumer; here it is
        // logged and dropped rather than mistaken for a commit.
        let HostCallCorrelation::Atomic { commit, .. } = correlation else {
            log::debug!("owner: clock probe outcome with no consumer yet: {outcome:?}");
            return;
        };

        if !self.is_current(&correlation) {
            log::warn!("owner: uncorrelated host-call outcome {correlation:?}");
            Self::adopt_and_drop_fences(outcome);
            return;
        }

        // A validation has a correlation and a lease but no record, so it is
        // resolved before the record lookup rather than falling through it.
        if let Some((pending, _, _)) = &self.pending_validation {
            if *pending == commit {
                self.pending_validation = None;
                let _ = self.slot.release_validation(commit);
                if let HostCallOutcome::Accepted { out_fences, .. } = outcome {
                    // ValidationOnly creates no out-fence, so a reply carrying
                    // one contradicts its class. Close them and say so.
                    log::warn!("owner: a TEST_ONLY reply carried {} fds", out_fences.len());
                }
                return;
            }
        }

        let Some(record) = self.live.as_mut() else { return };
        if late || matches!(record.state(), RecordState::Terminal(_)) {
            // spec:2205-2210 — accepted-stale. Its fds are adopted and closed
            // into quarantine; it promotes nothing.
            Self::adopt_and_drop_fences(outcome);
            return;
        }

        match outcome {
            HostCallOutcome::Accepted { out_fences, .. } => {
                record.mark_accepted();
                record.ledger_mut().resolve_accepted();
                // 2b-ii adopts these as evidence. Until it exists they are
                // quarantined rather than closed: closing an unqueried fence
                // would discard the only proof of hardware completion.
                record.quarantine_fences(out_fences);
            }
            HostCallOutcome::Rejected { errno, .. } => {
                record.ledger_mut().resolve_rejected();
                record.terminalize(TerminalState::FailedBeforeSubmit(
                    FailureCause::IoctlRejected { errno },
                ));
                self.retire_live(commit);
            }
            HostCallOutcome::Unknown(reason) => {
                record.terminalize(TerminalState::CompletionUnknown(UnknownCause::HostCall(
                    reason,
                )));
                // The slot stays held: acceptance was neither established nor
                // disproved, so a second ioctl is forbidden. The record is
                // tombstoned for identity but not dropped — its ledger is the
                // quarantine.
                self.tombstone_live_without_releasing();
            }
            HostCallOutcome::ValidationAbandoned(_) => {
                log::warn!("owner: validation outcome arrived under a commit record");
            }
            HostCallOutcome::ProbeAccepted { .. } => {
                log::warn!("owner: clock probe outcome arrived under a commit correlation");
            }
        }
    }
}
```

`retire_live(commit)` tombstones the record, releases the slot and clears `live`. `tombstone_live_without_releasing()` pushes the tombstone and keeps both the record and the slot. Both push through `push_tombstone`, which pops the front once the ring exceeds `TOMBSTONE_RING_CAPACITY`.

`is_current(&correlation)` implements `ID-3`: incarnation, lifecycle epoch, optional transition id and commit id must all equal the live record's. `adopt_and_drop_fences(outcome)` moves any `out_fences` out of the outcome and drops them, which closes each exactly once through `OwnedFd` — the one place this sub-stage closes a descriptor.

`dispatch_validation(desc, executor)` builds with `HostCallClass::SeatActiveValidation`, takes `acquire_validation`, and installs **no record**. Its outcome path releases the lease.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::device`
Expected: PASS, 10 tests.

- [ ] **Step 5: Write the failing integration test against a real helper**

```rust
// crates/yserver/tests/owner_commit_record.rs
//! The owner driven by a real stub helper process, so the correlation the
//! record matches against is one that actually made a round trip.

#[test]
fn a_rejecting_helper_drives_the_record_to_failed_before_submit() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::RejectWith(libc::EBUSY)).expect("spawn");
    let mut owner = DeviceCommitOwner::new_for_tests();
    let commit = owner
        .dispatch(&single_active_crtc(), ResourceLedger::new(), &mut executor)
        .expect("dispatch");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    let event = executor.poll_reply().expect("reply");
    owner.apply_host_call_event(event);
    assert_eq!(owner.slot().occupant(), None);
    assert!(matches!(
        owner.tombstones().last().expect("tombstone").terminal,
        TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected { errno })
            if errno == libc::EBUSY
    ));
    let _ = commit;
}

#[test]
fn a_helper_that_dies_leaves_the_slot_held_and_the_ledger_quarantined() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    let mut owner = DeviceCommitOwner::new_for_tests();
    let commit = owner
        .dispatch(&single_active_crtc(), ResourceLedger::new(), &mut executor)
        .expect("dispatch");
    test_support::kill_helper(&mut executor);
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    owner.apply_host_call_event(executor.poll_reply().expect("terminal event"));
    assert_eq!(
        owner.slot().occupant(),
        Some(commit),
        "COMMIT-6: a dead helper proves nothing about acceptance"
    );
    assert_eq!(
        owner.live_record().expect("retained").ledger().disposition(),
        LedgerDisposition::Quarantined
    );
}

#[test]
fn a_watchdog_expiry_reaches_the_owner_through_tick_without_sleeping() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    let mut owner = DeviceCommitOwner::new_for_tests();
    let commit = owner
        .dispatch(&single_active_crtc(), ResourceLedger::new(), &mut executor)
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

`kill_helper` and `wait_readable` are 2a's `test_support` helpers, already used by `executor_async.rs`. `DeviceCommitOwner::new_for_tests()` builds an owner over `IdentityAllocator::new(IncarnationId::first())` with `LifecycleEpochId::first()` and topology generation 1.

- [ ] **Step 6: Run to verify they pass**

Run: `cargo test -p yserver --test owner_commit_record`
Expected: PASS, 3 tests.

- [ ] **Step 7: Commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/owner/device.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/tests/owner_commit_record.rs
git commit -m "feat(kms): drive commit records from the executor's typed outcome stream"
```

---

## Task 7: Backend integration, portable gates and the stage reviewability check

**Files:**
- Modify: `crates/yserver/src/kms/render/platform.rs:1990-2000,2555-2565`
- Modify: `crates/yserver/src/kms/render/backend.rs:14755-14761`

**Interfaces:**
- Consumes: `DeviceCommitOwner` (Task 6).
- Produces: an owner reachable from the backend, and the greps that prove what this sub-stage deliberately did not do.

- [ ] **Step 1: Write the failing routing test**

```rust
// crates/yserver/src/kms/render/backend.rs  (beside
// on_executor_readable_drains_more_than_one_queued_event)

#[test]
fn a_host_call_event_reaches_the_owning_devices_owner_not_only_the_log() {
    // Helper names are the ones this module already uses for the 2a executor
    // tests: `backend_with_stub_executors_with_behaviour_for_tests`,
    // `wait_executor_readable_for_tests`, and `ServerState::new()`. Do not
    // introduce parallel fixtures.
    let mut backend = backend_with_stub_executors_with_behaviour_for_tests(
        1,
        crate::kms::executor::test_support::StubBehaviour::RejectWith(libc::EBUSY),
    );
    let mut state = yserver_core::server::ServerState::new();
    let commit = backend.begin_on_first_device_for_tests().expect("begin");
    backend.send_on_first_device_for_tests().expect("send");
    wait_executor_readable_for_tests(&backend, std::time::Duration::from_secs(5));
    yserver_core::backend::Backend::on_executor_readable(&mut backend, &mut state);
    assert_eq!(
        backend.first_device_owner_for_tests().slot().occupant(),
        None,
        "an explicit EBUSY rejection is the one outcome that releases the slot"
    );
    assert_eq!(
        backend
            .first_device_owner_for_tests()
            .tombstones()
            .last()
            .expect("tombstoned")
            .commit,
        commit
    );
}

#[test]
fn the_test_only_event_queue_is_still_fed_so_2a_coverage_keeps_working() {
    // record_host_call_events gains a consumer; it does not lose one. The 2a
    // tests that read drained_host_call_events_for_tests must keep passing,
    // and this asserts the tee rather than leaving it to them.
    let mut backend = backend_with_stub_executors_with_behaviour_for_tests(
        1,
        crate::kms::executor::test_support::StubBehaviour::RejectWith(libc::EBUSY),
    );
    let mut state = yserver_core::server::ServerState::new();
    backend.begin_on_first_device_for_tests().expect("begin");
    backend.send_on_first_device_for_tests().expect("send");
    wait_executor_readable_for_tests(&backend, std::time::Duration::from_secs(5));
    yserver_core::backend::Backend::on_executor_readable(&mut backend, &mut state);
    assert!(!backend.drained_host_call_events_for_tests().is_empty());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p yserver --lib kms::render::backend`
Expected: FAIL with "no method named `first_device_owner_for_tests`".

- [ ] **Step 3: Carry the owner on the device and route events to it**

In `crates/yserver/src/kms/render/platform.rs`, beside `pub(crate) executor: Option<KmsIoExecutor>` at line 1995:

```rust
    /// The device-local commit owner. Present whenever the executor is: the
    /// two are created together at `platform_init` and neither is meaningful
    /// without the other.
    pub(crate) owner: Option<crate::kms::owner::device::DeviceCommitOwner>,
```

Every `KmsDevice` construction site — the six at lines 2561, 2989, 7302, 7751, 7772, 8362 — gains `owner: None` except the real one at 2561, which builds a `DeviceCommitOwner` over the device's identity allocator. Add `pub(crate) fn owner_for(&mut self, key: DrmDeviceKey) -> Option<&mut DeviceCommitOwner>` and `pub(crate) fn owner_for_event(&mut self, event: &HostCallEvent) -> Option<&mut DeviceCommitOwner>` beside `drain_executor_events`.

`backend.rs` gains three `#[cfg(test)]` helpers beside the existing `send_rejected_host_call_for_tests`, which is the fixture this module already uses for 2a's executor tests:

```rust
#[cfg(test)]
impl KmsBackend {
    pub(crate) fn begin_on_first_device_for_tests(&mut self) -> Result<CommitId, DispatchError> {
        let desc = crate::kms::owner::build::single_active_crtc_for_tests();
        let device = self.platform.devices.first_mut().expect("a device");
        device.owner.as_mut().expect("an owner").begin(&desc, ResourceLedger::new())
    }

    pub(crate) fn send_on_first_device_for_tests(&mut self) -> Result<(), DispatchError> {
        let device = self.platform.devices.first_mut().expect("a device");
        let executor = device.executor.as_mut().expect("an executor");
        device.owner.as_mut().expect("an owner").send_on(executor)
    }

    pub(crate) fn first_device_owner_for_tests(&self) -> &DeviceCommitOwner {
        self.platform.devices.first().expect("a device").owner.as_ref().expect("an owner")
    }
}
```

`send_on_first_device_for_tests` borrows `device.owner` and `device.executor` from the same `KmsDevice`, which the borrow checker refuses through two `as_mut()` calls on one binding. Destructure the device once — `let KmsDevice { owner, executor, .. } = device;` — rather than reaching for `RefCell`. `single_active_crtc_for_tests()` is Task 5's fixture promoted from its test module to a `#[cfg(test)] pub(crate)` function so both call sites share one description.

In `crates/yserver/src/kms/render/backend.rs`, `record_host_call_events` gains the routing while keeping the test queue:

```rust
    fn record_host_call_events(&mut self, events: Vec<crate::kms::executor::HostCallEvent>) {
        for event in events {
            log::debug!("kms executor host call event: {event:?}");
            // The owner is the real consumer; the queue remains a tee so 2a's
            // coverage of the transport keeps observing what crossed it.
            self.host_call_events_for_tests.lock().unwrap().push(event.clone());
            if let Some(owner) = self.platform.owner_for_event(&event) {
                owner.apply_host_call_event(event);
            }
        }
    }
```

`HostCallEvent` and `HostCallOutcome` must therefore derive `Clone`. `OwnedFd` is not `Clone`, so instead of deriving it, give `HostCallEvent` an `identity_only()` method returning a fd-free copy and push **that** into the test queue. Cloning an outcome that owns descriptors would duplicate them and break the exactly-once close rule that 2a's `an_accepted_reply_adopts_its_out_fence_and_releases_it_on_drop` pins. Do not derive `Clone`.

`owner_for_event(&event)` resolves the event's correlation incarnation to a device. With one executor per device and one incarnation per device in this sub-stage, that is a linear scan over `self.devices`; a map is not yet warranted and would need invalidation the reopen path does not exist to trigger.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p yserver --lib kms::render::backend`
Expected: PASS, including the two new tests and 2a's `on_executor_readable_drains_more_than_one_queued_event`.

- [ ] **Step 5: Run the portable compile gates**

```bash
cargo check -p yserver --target x86_64-unknown-linux-gnu
cargo check -p yserver --target x86_64-unknown-linux-musl
cargo check -p yserver --target x86_64-unknown-freebsd
```

Expected: all three succeed. If a target is not installed, `rustup target add` it; do not skip a gate. Nothing in this sub-stage issues an ioctl, so the only portability surface is `libc::EBUSY`/`EINVAL` in tests and `OwnedFd` in the outcome — but the gate is what proves that, not the reasoning.

- [ ] **Step 6: Run the reviewability greps**

```bash
# 1. The stage-1 SequenceSupport map is deliberately still device-keyed.
grep -n 'HashMap<(crate::platform::drm::DrmDeviceKey, ClockEpochId), SequenceSupport>' \
     crates/yserver/src/kms/render/backend.rs
```
Expected: still present at `kms/render/backend.rs:1044`. This sub-stage does **not** move it — spec lines 1755-1763 require it inside 2b-ii's epoch-local clock record, which does not exist yet. The grep is here so an executor of this plan does not "helpfully" start that migration, and so a reviewer sees the omission is deliberate.

```bash
# 2. No production atomic_commit call site was converted.
grep -rn 'atomic_commit' crates/yserver/src/drm/ | grep -v '^.*://'
```
Expected: the six live sites in `page_flip.rs` and `modeset.rs` are unchanged. Converting one is 2c's.

```bash
# 3. Nothing outside slot.rs can mint a reservation proof.
grep -rn 'from_reservation' crates/yserver/src --include=*.rs
```
Expected: exactly three lines — the two constructors in `executor/mod.rs` and their uses in `owner/slot.rs`. A fourth means the type-state has a hole.

```bash
# 4. No completion milestone is set anywhere in this sub-stage.
grep -rn 'hardware_complete = true\|presented = true\|prior_buffer_released = true' \
     crates/yserver/src
```
Expected: no matches. These are 2b-ii's to set; a match here means someone inferred completion from acceptance.

```bash
# 5. The owner performs no ioctl.
grep -rn 'libc::ioctl\|drm_mode_atomic\|SYNC_IOC' crates/yserver/src/kms/owner/
```
Expected: no matches. Every kernel interaction goes through the 2a executor.

- [ ] **Step 7: Run the full suite and both gates**

```bash
cargo +nightly fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p yserver
```

Expected: all clean. Run `cargo test -p yserver` **five times**: this sub-stage adds tests that spawn helper processes, and 2a shipped two races that a single green run hid — the substrate suite failed 11 runs in 12 while `cargo clippy` and one `cargo test` both reported success. One green run is not evidence.

- [ ] **Step 8: Commit**

```bash
git add crates/yserver/src/kms/render/platform.rs crates/yserver/src/kms/render/backend.rs \
        crates/yserver/src/kms/executor/mod.rs
git commit -m "feat(kms): route executor outcomes to the device commit owner"
```

---

## What this sub-stage proves

- **A request's CRTC closure is computed once, from the persistent entries, and re-checked against the bytes that are actually sent.** An out-fence cannot enlarge it, an off-to-off member cannot carry a page event, and a Present consumer without an event set cannot be created.
- **`COMMIT-6`'s asymmetry is structural.** Only `Rejected` releases the slot. All four acceptance-unknown reasons hold it and quarantine the ledger, and a test iterates the enum so a fifth reason cannot be added without deciding which side it falls on.
- **Acceptance is not completion.** The owner records `Accepted` and stops. Nothing in this sub-stage can reach `Completed`, and Task 6's test and Task 7's grep both pin that, because "the slot never frees" looks like a bug to anyone who has not read section 6.3.
- **A reservation proof cannot be forged.** `SubmittingProof` and `ValidationLease` have exactly one production issuer each, and `grep from_reservation` is the check.
- **Validation never occupies the commit slot.** It takes its own exclusive lease, and a commit remains admissible while one is outstanding.

## What stage 2b-ii consumes

- `CommitRecord` and its `Milestones`: 2b-ii sets `hardware_complete` from successful canonical out-fence status, and `presented` from a correlated page event. It never sets one from the other.
- `CommitRecord::quarantine_fences(Vec<OwnedFd>)` is where 2b-ii's fence adoption replaces this sub-stage's hold-and-drop. The fds are already owned by the record, so 2b-ii adds the `SYNC_IOC_FILE_INFO` query and the `Completed` transition, not a new ownership path.
- `AtomicCrtcClosure::{kernel_event, present_event}` are the sets 2b-ii's correlation matches an event's `crtc_id` against, and `expected_completion` is the set whose fences must all report signalled before `Completed`.
- `Tombstone` and the 64-entry ring are what 2b-ii resolves a delayed event against before deciding it is `unknown`.
- `DeviceCommitOwner::apply_host_call_event`'s `ProbeAccepted` arm is the placeholder 2b-ii replaces with the epoch-local clock record's decision between `KernelSequence` and `Unresolved`.
- **A prerequisite, not a handover: the `SequenceSupport` migration.** `kms/render/backend.rs:1044` is still device-keyed. 2b-ii builds the epoch-local clock record it must move into, and that move is 2b-ii's first task, not an afterthought at its end.
- **A prerequisite inherited from 2a: core poll-source churn.** `run_core` collects `Backend::poll_fds()` once and never refreshes it (`run.rs:1045-1059`). Neither 2a nor this sub-stage ever replaces an executor, so neither is affected. The first reopen path — stage 3's — cannot bring a replacement executor's control fd into a running loop until that mechanism exists.

## What this sub-stage deliberately leaves broken-looking

- **An accepted commit never completes and the slot never frees.** That is the honest state without fence evidence. Do not add a completion path here.
- **An unknown commit holds the slot forever.** Recovery — the one automatic attempt, the `RecoveryId`, the fd-set barrier — is section 10's table and belongs to stage 3. Until then a device that reaches `CompletionUnknown` stops accepting commits, which is precisely what `COMMIT-6` asks for.
- **The tombstone ring is written and never read.** Its reader is 2b-ii's event correlation. It is built here because the record that populates it is built here, and retrofitting a ring onto an existing terminal path is how a duplicate event ends up resolving against a live record.
