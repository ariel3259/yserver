# Phase C.0 Stage 2a — Executor substrate completion

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the process-isolated executor able to carry a real atomic request, return its out-fences, and do so without ever making the X11 core wait — and put the `COMMIT-7` device lock where only the executor's death releases it.

**Architecture:** Stage 1 built an executor that performs an *empty* atomic ioctl through a blocking poll loop. This stage completes it in four moves. The wire gains a bounded variable property payload and a correlation tuple echoed in every reply. The helper materializes those arrays, owns the `OUT_FENCE_PTR` holder storage the kernel writes into, and returns the resulting descriptors. The host call splits into send, poll and watchdog. And the core event loop gains a real executor poll source and a real executor deadline, so that split API has a production consumer rather than a promise of one.

**Tech Stack:** Rust (stable toolchain), `libc`, `std::os::unix` sockets. No serialization crate: framing stays hand-rolled, extended from stage 1's fixed frames to one fixed head plus a bounded variable payload.

**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md` (Approved, revision 2). This plan implements section 18 **stage 2a only**.

**Predecessor:** `2026-09-02-phase-c0-stage-1-executor-substrate.md`, complete at `83b47700`.

**Revision 4, after three adversarial reviews.** Round 3 (`docs/superpowers/findings/2026-09-05-phase-c0-stage-2a-plan-review-round3.md`, the first run under a recorded instrument) returned 10 blocking, 2 major and 2 minor. **Every one of those is resolved here, together with the six round-2 majors revision 3 had left open.** No finding from any round is knowingly outstanding.

**What round 3 taught, and what changed structurally because of it.** Five of its ten blockers were consequences of revision 3's own type-model widening: the change landed where it originated while its neighbours kept speaking the old language. That is the third time this failure has appeared in this work. The mechanism is specific — the type model was re-derived in five tasks — so revision 4 moves it into one normative **"The wire and API contract"** section that every task implements and a reviewer checks against. A model change is now one edit, not five.

**Revision 2, after adversarial review.** The first version of this plan returned 8 blocking and 9 major findings, recorded at `docs/superpowers/findings/2026-09-04-phase-c0-stage-2a-plan-adversarial-review.md`. Tasks 2, 4 and 6 are rewritten whole rather than patched — a lesson from the stage 2 monolith, where paragraph-level corrections left each task's interfaces speaking two languages at once. Task 5 is new: the review established that core event-loop integration crosses `yserver-core`'s `Backend` trait and the core loop's dispatch, which is a separately reviewable deliverable rather than a step inside the async API. Tasks 1, 3 and 7 take targeted corrections, listed in their own headers.

**Do not read the review's task numbers as this plan's.** The review's Task 4 is now Tasks 4 and 5; its Task 5 is now Task 6; its Task 6 is now Task 7.

---

## Global Constraints

Copied from the spec. Every task's requirements implicitly include this section.

- **`COMMIT-5`** — the X11 core never executes or waits synchronously for a potentially blocking KMS ioctl. During seat-active service every live commit uses `NONBLOCK`. Blocking atomic calls are restricted to cold startup before service, or final offline/shutdown work after prompt lifecycle obligations have ended.
- **`COMMIT-6`** — before sending IPC the owner installs a `Submitting` record and reserves the device slot. After send, only an explicit ioctl rejection proves `FailedBeforeSubmit`; missing or invalid reply, helper exit, IPC failure and watchdog expiry are acceptance-unknown. **No second ioctl may be dispatched on the device while this record or its executor lease exists.**
- **`ID-3`** — every executor request and reply carries the lifecycle epoch (`spec:416-425`, unqualified). A reply is current only when incarnation, lifecycle epoch, optional transition id and commit id all match. The startup handshake uses its own frame family so the host-call accessors can stay total, **and it carries incarnation and lifecycle epoch like everything else on this wire.** Both identities exist before the spawn, so the frame split costs nothing.
- **`COMMIT-7`** — sending a termination signal, closing the IPC channel, `PR_SET_PDEATHSIG` or a watchdog expiry is a request, not reap proof. The guarantee that no later incarnation installs state underneath a still-live helper comes from a device-scoped advisory lock taken by the executor **for as long as it lives, released only by its death**.
- **`ValidationOnly`** — `TEST_ONLY` executes `drm_atomic_check_only`. It omits `NONBLOCK`, touches no hardware, creates no out-fence, and **does not occupy the submitted-commit slot**. It holds an exclusive owner validation lease, not a `Submitting` record.
- Host-call watchdog: 2 seconds for seat-active `NONBLOCK` work and for seat-active `ValidationOnly`; 30 seconds for a permitted cold-start or final-offline blocking ioctl **and for cold-start or offline validation** (`spec:320-329`). Validation is not uniformly two seconds; the boundary it runs at decides.
- Message transport is message-boundary-preserving. Atomic success returns every out-fence through fd passing.
- The executor never reads the DRM event fd; drain is owner-exclusive for the incarnation.
- Identity allocation uses checked increment and never wraps or reuses a token within an incarnation.
- Portable builds must compile on glibc, musl and FreeBSD.
- Format is `cargo +nightly fmt --check`. Tests are `cargo test -p yserver`. Lint is `cargo clippy --all-targets -- -D warnings`, exactly as CI runs it.

### What this sub-stage does not do

- **No owner exists yet.** Nothing here installs a commit record, reserves a device slot or classifies a commit. `SubmittingProof` and `ValidationLease` keep test-only constructors; 2b adds their production producers.
- **No production `atomic_commit` call site is converted.** The six live sites are 2c's.
- **No damage, admission, clock or completion logic.** Those are 2b and 2c.
- **The stage-1 `SequenceSupport` map is not moved.** `kms/render/backend.rs:1042` still holds a device-keyed `HashMap<(DrmDeviceKey, ClockEpochId), SequenceSupport>` where spec lines 1755-1763 require an epoch-local clock record keyed by hardware CRTC. That gap is recorded at `docs/superpowers/findings/2026-09-03-phase-c0-stage-2-plan-adversarial-review.md` and belongs to 2b, which builds the clock record this decision must move into. Do not attempt it here.

---

## File Structure

**Modified — `yserver`:**
- `crates/yserver/src/kms/owner/lifecycle.rs` — created here: `LifecycleEpochId`, `LifecycleTransitionId`, `ClockProbeId`.
- `crates/yserver/src/kms/owner/identity.rs` — checked allocation.
- `crates/yserver/src/kms/owner/mod.rs` — declares the new module.
- `crates/yserver/src/kms/executor/protocol.rs` — protocol version 2: the correlation tuple, the explicit host-call class, the bounded variable payload, the out-fence slot table, the readiness handshake.
- `crates/yserver/src/kms/executor/transport.rs` — variable-length frames, non-blocking receive on the parent endpoint only.
- `crates/yserver/src/kms/executor/helper.rs` — property materialization, holder ownership, the readiness reply, the lock-fd adoption.
- `crates/yserver/src/kms/executor/mod.rs` — the split host-call API, the non-unlocking `Drop`, the inherited lock slot.
- `crates/yserver/src/kms/executor/device_lock.rs` — the destructor stops unlocking; the type-state handoff.
- `crates/yserver/src/kms/executor/test_support.rs` — the stub behaviours the new tests need.
- `crates/yserver/src/kms/backend.rs:855-882` — `platform_init` takes the lock and hands it to the executor.
- `crates/yserver/src/kms/render/platform.rs:1990-1994,2550-2559,3936-3958` — `KmsDevice` carries the executor; `poll_fds` publishes its control fd.
- `crates/yserver/src/kms/render/backend.rs:15201-15250,16138-16142` — `next_wakeup` includes the executor deadline; `poll_fds` forwards it.

**Modified — `yserver-core`:**
- `crates/yserver-core/src/backend/trait_def.rs:57-84` — `BackendFdKind::ExecutorControl`.
- `crates/yserver-core/src/backend/trait_def.rs:506-514` — `Backend::on_executor_readable`, a defaulted no-op so `recording.rs` and `host_x11/trait_impl.rs` need no change.
- `crates/yserver-core/src/core_loop/run.rs:1209-1257` — the dispatch arm.

**New:**
- `crates/yserver/tests/executor_async.rs` — integration tests that spawn real helper processes.
- `crates/yserver/tests/executor_lock_handoff.rs` — the parent-death test, which needs its own process tree.

**Explicitly out of scope:**
- `crates/yserver/src/present/event_loop.rs` — as in stage 1: its `run_loop` has no caller in the workspace.

---

## The wire and API contract

**This section is normative and every task implements it.** It exists because the
type model is used by tasks 1, 2, 3, 4 and 6, and round 3 proved what happens
when it is re-derived in five places: a change lands where it originates and its
neighbours keep speaking the old language. Five of that round's ten blocking
findings had exactly that shape. A task that shows code contradicting this
section is wrong, and this section is what a reviewer checks it against.

### Identities (task 1)

| Type | Constructors | Notes |
| --- | --- | --- |
| `IncarnationId` | `first`, `next`, `checked_next`, `get`, `for_tests` | stage 1; `next` becomes checked |
| `ClockEpochId` | `first`, `next`, `checked_next`, `get`, `for_tests` | stage 1; `next` becomes checked |
| `CommitId` | `from_raw`, `get`, `for_tests` | stage 1 |
| `EventToken` | `as_user_data`, `from_user_data`, `tagged_for_tests` | purpose-tagged; see below |
| `SequenceArmToken` | `as_user_data`, `from_user_data`, `tagged_for_tests` | purpose-tagged |
| `LifecycleEpochId` | `first`, `next`, `checked_next`, `get`, `from_raw` | new |
| `LifecycleTransitionId` | `from_raw`, `get` | new; no `next` |
| `ClockProbeId` | `first`, `next`, `checked_next`, `get`, `from_raw` | new |
| `RequestSeq` | `from_raw`, `get`, `for_tests` | stage 1 |

**Purpose tags.** `EventToken` and `SequenceArmToken` draw from one counter and
are separated by the top two bits (`PURPOSE_SHIFT = 62`). Task 1 makes both
`from_user_data` reject a value whose tag is not their own. Consequently their
raw values are **never** small integers, and `for_tests(raw)` — which stores a
raw value verbatim — cannot be used to build a token that survives decoding.
Every task that needs a valid token uses `EventToken::tagged_for_tests(counter)`,
which applies the purpose tag, and asserts against
`EventToken::tagged_for_tests(n).as_user_data()` rather than against a literal.
This is the contradiction round 3's B-3 found; it is settled here once.

### Host-call classes (task 1)

| Class | `NONBLOCK` | `TEST_ONLY` | Watchdog | Reservation | Permitted phase |
| --- | --- | --- | --- | --- | --- |
| `SeatActiveNonblock` | set | clear | 2 s | `Submitting` | any |
| `SeatActiveValidation` | clear | set | 2 s | `Validation` | any |
| `ColdStartOrOfflineBlocking` | clear | clear | 30 s | `Submitting` | `ColdStart`, `FinalOffline` |
| `ColdStartOrOfflineValidation` | clear | set | 30 s | `Validation` | `ColdStart`, `FinalOffline` |

`is_validation()` is true for the two `*Validation` classes. The two share
identical flags and differ only in watchdog, which is why the class is an
explicit wire field rather than something derived from the flag bits
(`spec:320-329`).

**The permitted-phase column is enforced by `send`, not only by
`dispatch_blocking_at_boundary`.** `send` rejects a `ColdStartOrOffline*` class
while `phase == SeatActive` with `SendError::BoundaryViolation`. Without that,
`COMMIT-5` is unenforced on the asynchronous path: a seat-active caller could
send a blocking-class request, or relabel seat-active validation as cold/offline
to buy the 30-second watchdog (round 3, B-4).

**Neither validation class may carry out-fence slots**, and both the encoder and
the decoder reject it (`spec:320-325,2126`). The rule is written against
`class.is_validation()`, never against one named variant, so extending the enum
cannot leave a hole (round 3, B-5).

### Correlation and requests (task 2)

```rust
pub(crate) enum HostCallCorrelation {
    Atomic {
        seq: RequestSeq,
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        transition: Option<LifecycleTransitionId>,
        commit: CommitId,
        event_token: EventToken,
    },
    ClockProbe {
        seq: RequestSeq,
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        topology_generation: u64,
        hardware_crtc: u32,
        clock_epoch: ClockEpochId,
        probe: ClockProbeId,
    },
}

pub(crate) enum HostCallRequest { Atomic(AtomicRequest), ClockProbe(ClockProbeRequest) }
pub(crate) enum HostCallReply {
    Accepted      { correlation: HostCallCorrelation, helper_duration_ns: u64, out_fence_mask: u32 },
    Rejected      { correlation: HostCallCorrelation, errno: i32, helper_duration_ns: u64,
                    unexpected_fence_output: bool },
    ProbeAccepted { correlation: HostCallCorrelation, sequence: u64, helper_duration_ns: u64 },
    ProbeRejected { correlation: HostCallCorrelation, errno: i32, helper_duration_ns: u64 },
}
```

**Four variants, in two families of two.** `ProbeRejected` exists because a
clock probe's most important negative result is an explicit `EOPNOTSUPP`: that
is how 2b decides a CRTC is structurally incapable rather than merely
unresolved (`spec:1750-1770`). With a single `Rejected` belonging to the atomic
family, a rejected probe is unrepresentable — every one would be classified
malformed by the family check below, and the 2b handover that promises an
`EOPNOTSUPP` rejection could not be honoured.

`HostCallRequest::{correlation, class}` and `HostCallReply::correlation` are
**total and return bare values**, never `Option`. `HostCallRequest::kind()`
returns `RequestKind::{Atomic, ClockProbe}`. `HostCallReply::family()` returns
the `RequestKind` its variant belongs to:

| Reply variant | Family |
| --- | --- |
| `Accepted`, `Rejected` | `Atomic` |
| `ProbeAccepted`, `ProbeRejected` | `ClockProbe` |

**A reply is current only when its family matches the request's kind *and* its
correlation is equal.** Correlation equality alone is insufficient: a probe
request could otherwise receive an `Accepted` carrying the probe's own
correlation and surface as an atomic acceptance. Deriving the family from the
*correlation* would not fix that, because the correlation is precisely what
matches — it must come from the variant. `poll_reply` checks family first,
then correlation.

### The startup handshake (tasks 2 and 6)

```rust
pub(crate) struct HandshakeRequest { pub incarnation: IncarnationId,
                                     pub lifecycle_epoch: LifecycleEpochId }
pub(crate) struct HandshakeReply   { pub incarnation: IncarnationId,
                                     pub lifecycle_epoch: LifecycleEpochId,
                                     pub helper_pid: u32 }
```

The handshake is a **separate frame family** with its own kinds and codecs; each
decoder rejects the other family's kind with `ProtocolError::Kind`. That
separation is what keeps `correlation()` and `class()` total.

**It nonetheless carries incarnation and lifecycle epoch, because `spec:416-425`
says "Every executor request/reply and commit record carries the epoch" without
qualification.** Revision 3 argued the handshake was exempt as "not an executor
request"; that was an invented exception, and it was never forced by the frame
split. Both identities exist before the spawn — `platform_init` allocates the
incarnation, and `LifecycleEpochId::first()` is available — so there is nothing
to trade. `await_helper_ready` rejects a reply whose two identities do not echo
the request's (round 3, B-2).

### The asynchronous API (task 4)

```rust
pub enum HostCallReservation {
    Submitting(SubmittingProof),
    Validation(ValidationLease),
    ClockProbe(ClockProbeLease),
}

pub enum HostCallOutcome {
    Accepted      { helper_duration_ns: u64, round_trip_ns: u64,
                    out_fences: Vec<OwnedFd>, out_fence_mask: u32 },
    ProbeAccepted { sequence: u64, helper_duration_ns: u64, round_trip_ns: u64 },
    Rejected      { errno: i32, helper_duration_ns: u64, round_trip_ns: u64,
                    unexpected_fence_output: bool },
    /// A live request whose acceptance could not be determined. COMMIT-6
    /// quarantine applies: hardware state is unknown.
    Unknown(UnknownReason),
    /// A ValidationOnly request that did not complete. The candidate snapshot
    /// is invalid, but NO hardware state is in question, because no live
    /// mutation was requested (`spec:320-329`). This must never be folded into
    /// `Unknown`, which would apply the live-mutation uncertainty model to a
    /// call that touched nothing.
    ValidationAbandoned(UnknownReason),
}

pub enum HostCallEvent {
    Outcome   { correlation: HostCallCorrelation, outcome: HostCallOutcome },
    LateReply { correlation: HostCallCorrelation, outcome: HostCallOutcome },
}

pub enum SendError { AlreadyInFlight, Stalled, Reaped, Ipc, ReservationMismatch, BoundaryViolation }
pub enum HostCallPhase { ColdStart, SeatActive, FinalOffline }
```

`terminalize_unknown(reason)` branches on the in-flight call's class: a
validation class yields `ValidationAbandoned(reason)`, every other class yields
`Unknown(reason)`. Both still enter `Stalled` and retain `in_flight` until reap,
because the *executor* is equally unreliable either way; what differs is the
claim made about hardware (round 3, B-6).

`InFlight` retains everything a reply must be validated against:

```rust
struct InFlight {
    correlation: HostCallCorrelation,
    class: HostCallClass,
    kind: RequestKind,
    /// Needed to build the out-fence mask's validity bound. Round 3's B-7
    /// found this missing while task 3 required the check.
    slot_count: u32,
    started: Instant,
    deadline: Instant,
    terminalized: Option<UnknownReason>,
}
```

### Visibility

Tasks 3, 4 and 6 place tests under `crates/yserver/tests/`, which compile as
**external crates**. Stage 1 already established the pattern: its test-facing
surface is `#[doc(hidden)] pub` (`executor/mod.rs:153-247`), which is why
`tests/executor_substrate.rs` compiles. Every item below becomes
`#[doc(hidden)] pub`, and the tasks' file lists and commits **must include the
files this changes** — round 3's B-1 and B-9 were both this declaration failing
to reach a file list.

| File | Items |
| --- | --- |
| `kms/mod.rs` | `pub mod owner` (was `pub(crate)`) |
| `kms/owner/mod.rs` | `pub mod identity`, `pub mod lifecycle` |
| `kms/owner/identity.rs` | `IncarnationId`, `CommitId`, `EventToken`, `SequenceArmToken`, `ClockEpochId`, `IdentityAllocator`, and their constructors |
| `kms/owner/lifecycle.rs` | `LifecycleEpochId`, `LifecycleTransitionId`, `ClockProbeId` |
| `kms/executor/mod.rs` | `pub mod protocol`; `KmsIoExecutor::state`; the new API items above |
| `kms/executor/protocol.rs` | `HostCallRequest`, `HostCallReply`, `AtomicRequest`, `ClockProbeRequest`, `AtomicPropertyList`, `OutFenceSlot`, `HostCallCorrelation`, `RequestKind`, `RequestSeq`, `ProtocolError`, `HandshakeRequest`, `HandshakeReply`, and the constants `DRM_MODE_ATOMIC_NONBLOCK`, `DRM_MODE_ATOMIC_TEST_ONLY`, `MAX_ATOMIC_PROPS`, `MAX_OUT_FENCES` |
| `kms/executor/device_lock.rs` | `DeviceLock`, `InheritableDeviceLock`, `may_install_state`, `acquire_device_lock_or_refuse` |
| `platform/drm.rs` | `DrmDeviceKey` and its `major`/`minor` fields |

`#[doc(hidden)]` keeps every one of these out of rendered documentation. This is
a test seam, not public API.

**Test helpers live in `test_support`, never in the test files**, so no external
test constructs a wire type by hand and no helper is used before the task that
produces it. `test_support` is already `#[doc(hidden)] pub`. Task 2 produces the
request builders, because tasks 3 onwards consume them; a helper first declared
in task 4 and used in task 3 is a defect (round 3, B-1).

---

### Task 1: Lifecycle identities, the explicit host-call class, and checked allocation

**Status: EXECUTED at `6f951850`.** The shown code below has been corrected to
what actually compiled and passed; the two defects execution exposed are
recorded at the end of Step 3.

**Corrections from review:** B-1 (`ClockProbeId` was used by task 2 and produced by nobody), M-5 (the promised compile-fail case cannot reach a `pub(crate)` module), m-1 (three positional filters in one `cargo test`).

**Files:**
- Create: `crates/yserver/src/kms/owner/lifecycle.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`, `crates/yserver/src/kms/owner/identity.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs:153-181` (`HostCallClass`)

**Interfaces:**
- Consumes: `IncarnationId`, `CommitId`, `EventToken`, `IdentityAllocator` from stage 1.
- Produces:
  - `LifecycleEpochId::{first, next, checked_next, get, from_raw}` and `LifecycleTransitionId::{from_raw, get}`
  - `ClockProbeId::{first, next, checked_next, get, from_raw}` — task 2's probe correlation requires it, so it is produced here rather than assumed into existence
  - `IdentityAllocator::at_limit_for_tests()` — a `#[cfg(test)]` constructor seeding the counter at `COUNTER_MASK` so the limit is reachable without allocating 2^62 tokens
  - `EventToken::from_user_data` and `SequenceArmToken::from_user_data` **gain purpose-tag checking**; today both accept any nonzero value
  - `EventToken::tagged_for_tests(counter)` and `SequenceArmToken::tagged_for_tests(counter)` — apply the purpose tag, so a test token survives its own decoder. `for_tests` stays for callers that genuinely want a raw value, but nothing that crosses the wire may use it.
  - `IncarnationId::checked_next` and `ClockEpochId::checked_next`, with `next` implemented over them
  - `HostCallClass::{SeatActiveNonblock, SeatActiveValidation, ColdStartOrOfflineBlocking, ColdStartOrOfflineValidation}` with `watchdog()`, `wire_tag()`, `from_wire_tag(u8) -> Option<Self>` and `is_validation()`
  - `IdentityAllocator` allocation that cannot wrap

- [x] **Step 1: Write the failing tests**

```rust
// crates/yserver/src/kms/owner/lifecycle.rs
#[cfg(test)]
mod tests {
    use super::{ClockProbeId, LifecycleEpochId, LifecycleTransitionId};

    #[test]
    fn the_lifecycle_epoch_is_monotonic_and_starts_at_one() {
        let first = LifecycleEpochId::first();
        assert_eq!(first.get(), 1);
        assert_eq!(first.next().get(), 2);
        assert!(first.next() > first);
    }

    #[test]
    fn epoch_increment_is_checked_rather_than_wrapping() {
        // Spec 10: tokens are allocated with checked increment and never wrap
        // or are reused. A release build must not silently wrap.
        let last = LifecycleEpochId::from_raw(u64::MAX);
        assert_eq!(last.checked_next(), None);
    }

    #[test]
    fn a_transition_id_is_distinct_from_an_epoch_in_the_type_system() {
        // Stage 1 stored a ClockEpochId in the lifecycle-epoch field. These
        // must not be interchangeable.
        let e = LifecycleEpochId::from_raw(4);
        let t = LifecycleTransitionId::from_raw(4);
        let p = ClockProbeId::from_raw(4);
        assert_eq!(e.get(), t.get());
        assert_eq!(e.get(), p.get());
        // Equal raw values, three distinct types. The separation is enforced
        // by the newtypes themselves, not by a compile-fail case: these live
        // in `kms::owner`, which is `pub(crate)` (`kms/mod.rs:18`), so an
        // external compile-fail file would fail on privacy before it could
        // test the distinction and would prove nothing.
    }

    #[test]
    fn a_clock_probe_id_is_monotonic_and_checked() {
        // Spec 10: monotonic within an incarnation, never wrapping.
        let first = ClockProbeId::first();
        assert_eq!(first.get(), 1);
        assert!(first.next() > first);
        assert_eq!(ClockProbeId::from_raw(u64::MAX).checked_next(), None);
    }
}
```

```rust
// crates/yserver/src/kms/owner/identity.rs
#[test]
fn identity_allocation_is_checked_at_the_counter_limit() {
    let mut alloc = IdentityAllocator::at_limit_for_tests();
    assert_eq!(alloc.checked_next_commit(), None);
    assert_eq!(alloc.checked_next_event_token(), None);
    assert_eq!(alloc.checked_next_sequence_arm(), None);
}

#[test]
fn the_purpose_tag_never_collides_with_the_counter() {
    let mut alloc = IdentityAllocator::new(IncarnationId::first());
    let event = alloc.checked_next_event_token().expect("token");
    let arm = alloc.checked_next_sequence_arm().expect("arm");
    assert_ne!(event.as_user_data(), arm.as_user_data());
    assert!(EventToken::from_user_data(arm.as_user_data()).is_none());
    assert!(SequenceArmToken::from_user_data(event.as_user_data()).is_none());
}
```

```rust
// crates/yserver/src/kms/executor/mod.rs — where HostCallClass lives, not
// protocol.rs.
#[test]
fn each_host_call_class_carries_the_watchdog_the_spec_assigns_it() {
    // spec:320-329 — seat-active validation is two seconds, cold-start or
    // offline validation is thirty. Deriving the class from the NONBLOCK bit
    // gave every TEST_ONLY request the thirty-second watchdog.
    assert_eq!(HostCallClass::SeatActiveNonblock.watchdog(), Duration::from_secs(2));
    assert_eq!(HostCallClass::SeatActiveValidation.watchdog(), Duration::from_secs(2));
    assert_eq!(HostCallClass::ColdStartOrOfflineBlocking.watchdog(), Duration::from_secs(30));
    assert_eq!(HostCallClass::ColdStartOrOfflineValidation.watchdog(), Duration::from_secs(30));
}

#[test]
fn only_the_validation_classes_report_is_validation() {
    assert!(HostCallClass::SeatActiveValidation.is_validation());
    assert!(HostCallClass::ColdStartOrOfflineValidation.is_validation());
    assert!(!HostCallClass::SeatActiveNonblock.is_validation());
    assert!(!HostCallClass::ColdStartOrOfflineBlocking.is_validation());
}

#[test]
fn the_host_call_class_round_trips_through_its_wire_tag() {
    for class in [
        HostCallClass::SeatActiveNonblock,
        HostCallClass::SeatActiveValidation,
        HostCallClass::ColdStartOrOfflineBlocking,
        HostCallClass::ColdStartOrOfflineValidation,
    ] {
        assert_eq!(HostCallClass::from_wire_tag(class.wire_tag()), Some(class));
    }
    // Zero is not a class, so a zeroed byte never decodes as one; neither
    // does a tag past the last variant.
    assert_eq!(HostCallClass::from_wire_tag(0), None);
    assert_eq!(HostCallClass::from_wire_tag(5), None);
}
```

- [x] **Step 2: Run the tests to verify they fail**

`cargo test` takes one positional filter, so run three commands rather than
passing three names to one:

```bash
cargo test -p yserver kms::owner::lifecycle
cargo test -p yserver kms::owner::identity
cargo test -p yserver host_call_class
```
Expected: FAIL — the module and the checked allocators do not exist.

- [x] **Step 3: Write the implementation**

```rust
//! Lifecycle identities.
//!
//! `LifecycleEpochId` is always present, including during ordinary `Ready`
//! traffic (spec 6.1). A transition id is optional: ordinary commits carry
//! `None`, never a fabricated or previous id. They are separate types because
//! stage 1 stored a `ClockEpochId` where the lifecycle epoch belongs.

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct LifecycleEpochId(u64);

impl LifecycleEpochId {
    pub(crate) const fn first() -> Self { Self(1) }
    pub(crate) const fn get(self) -> u64 { self.0 }
    pub(crate) const fn from_raw(raw: u64) -> Self { Self(raw) }

    /// Exhaustion is unreachable within a process lifetime, so the production
    /// caller unwraps with a message rather than carrying a recovery branch
    /// the spec does not specify.
    pub(crate) const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    pub(crate) fn next(self) -> Self {
        self.checked_next().expect("lifecycle epoch exhausted")
    }
}
```

`LifecycleTransitionId` is the same newtype without `next`. `ClockProbeId` is
the same newtype *with* `next`, allocated per probe within an incarnation. None
of the three derives `From<u64>` or `Into<u64>`, so one cannot be passed where
another is expected.

No compile-fail case is added. The existing runner (`tests/compile_fail.rs:29-43`)
compiles an external file against `libyserver`, and these types are inside
`pub(crate) mod owner` (`kms/mod.rs:18`), so such a file fails on privacy rather
than on the type distinction — a test that passes for the wrong reason. The
newtypes are the enforcement; the tests above pin their construction.

`IncarnationId::next` and `ClockEpochId::next` are today unchecked `Self(self.0 + 1)`
(`identity.rs:18-21,103-106`), which the global "identity allocation is checked
and cannot wrap" constraint already forbids. Both gain `checked_next` returning
`Option`, with `next` implemented as `.expect(...)` over it. Round 2's M-7 read
the exit criterion as broader than the changes; this closes the gap rather than
narrowing the claim.

`IdentityAllocator` gains `checked_next_commit`, `checked_next_event_token` and
`checked_next_sequence_arm` returning `Option`, with the existing infallible
wrappers implemented as `.expect(...)` over them. The tagged counter checks
against `COUNTER_MASK`, not `u64::MAX`, because the purpose tag occupies the top
two bits. `at_limit_for_tests()` constructs an allocator with `next_counter`
already at `COUNTER_MASK`, so the limit is reachable in a test.

**Both token decoders must start checking the purpose tag.** Today
`EventToken::from_user_data` and `SequenceArmToken::from_user_data` accept every
nonzero value (`identity.rs:62-64,83-85`), so each happily decodes the other's
token and the purpose-separation test above cannot pass. The module doc already
claims a token is distinguishable "from another purpose's token"
(`identity.rs:5`); it is not, yet. Each decoder now rejects a raw value whose
top two bits are not its own purpose:

```rust
pub(crate) const fn from_user_data(raw: u64) -> Option<Self> {
    if raw == 0 || (raw >> PURPOSE_SHIFT) != PURPOSE_EVENT {
        None
    } else {
        Some(Self(raw))
    }
}

/// A token carrying the correct purpose tag, for tests that put one on the
/// wire. `for_tests(raw)` stores `raw` verbatim, so a small literal built
/// with it is rejected by the decoder above — which is the contradiction the
/// contract settles.
#[doc(hidden)]
pub const fn tagged_for_tests(counter: u64) -> Self {
    Self((PURPOSE_EVENT << PURPOSE_SHIFT) | (counter & COUNTER_MASK))
}
```

`SequenceArmToken::from_user_data` is the same against `PURPOSE_SEQUENCE_ARM`.
This is a behaviour change to already-merged stage 1 code, so check the existing
callers: both production callers of `queue_crtc_sequence` pass
`token.as_user_data()`, which round-trips through its own decoder unchanged.

`HostCallClass` gains **two** validation variants and stops deriving itself from
the `NONBLOCK` flag bit: the class is a declared field of the request, added to
the wire in task 2.

| Class | `NONBLOCK` | `TEST_ONLY` | Watchdog |
| --- | --- | --- | --- |
| `SeatActiveNonblock` | set | clear | 2 s |
| `SeatActiveValidation` | clear | set | 2 s |
| `ColdStartOrOfflineBlocking` | clear | clear | 30 s |
| `ColdStartOrOfflineValidation` | clear | set | 30 s |

The two validation classes carry **identical flags** and differ only in
watchdog. That is precisely why the class is an explicit wire field rather than
something derived: `spec:320-329` gives seat-active validation two seconds and
cold-start/offline validation thirty, and no flag bit distinguishes them. A
three-variant enum cannot express a spec-legal request.

- [x] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p yserver kms::owner::lifecycle
cargo test -p yserver kms::owner::identity
cargo test -p yserver host_call_class
```
Expected: PASS.

**Two things this task also requires, found by executing it.**

`dispatch_for_tests` (`executor/mod.rs:516-546`) matches on `HostCallClass` to
pick its flags, so adding variants makes it non-exhaustive. Give the two
validation classes `TEST_ONLY`; they are indistinguishable there by design.

The same function builds `event_token: EventToken::for_tests(1)`. That is the
first concrete instance of the rule the contract states: `for_tests` stores its
argument verbatim, so once the decoder checks the purpose tag the helper
rejects the frame and exits with a protocol error rather than replying. It must
be `EventToken::tagged_for_tests(1)`. Two of stage 1's integration tests
(`executor_substrate.rs`) fail with `Unknown(HelperExited)` until it is.

- [x] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/lifecycle.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/src/kms/owner/identity.rs crates/yserver/src/kms/executor/mod.rs
git commit -m "feat(kms): add lifecycle identities and checked identity allocation"
```

---

---

### Task 2: The atomic property payload and the reply correlation tuple

**Status: EXECUTED at `ecd76f1c`.** What executing it required, beyond the
text below:

- `helper.rs` and `test_support.rs`'s stub must be adapted in this task too —
  they construct replies and receive into a `REQUEST_FRAME_LEN` buffer that no
  longer exists. Both are now `vec![0u8; MAX_REQUEST_FRAME_LEN]`, because a
  32 KiB stack buffer per receive is avoidable. `transport.rs`'s own tests
  build replies by hand and needed the same update.
- The stage-1 helpers `decode_commit_id`, `decode_incarnation_id`,
  `decode_clock_epoch_id`, `put_bytes` and `take_bytes` lose their callers and
  are deleted rather than silenced; a single `nonzero(raw, field)` replaces the
  first three.
- `dispatch` compares **family then correlation**, replacing its `expected_seq`
  check. `HostCallClass::from_request` is deleted here, as planned.
- The reply frame stays fixed-length (`HEADER_LEN + 4 + 56 + 16`) with a shared
  56-byte correlation block, so one reply decoder serves both families. Only
  requests are variable-length.
- Visibility: `platform` and `platform::drm` must be opened too, not only the
  contract's listed items, or `DrmDeviceKey` is unreachable by path from an
  external test. `crates/yserver/tests/wire_external_surface.rs` is added to
  **prove** the seam compiles from outside the crate, because the last two
  reviews found this declared in prose and contradicted by the code.

Stage 1's helper submits `count_objs = 0` with null pointers, and its reply carries only a per-socket sequence number. `ID-3` requires every reply to carry the full correlation tuple, and `COMMIT-6` requires a late success whose lifecycle tag is stale to remain **accepted** rather than be mistaken for a rejection — which a sequence number cannot express.

The wire is also where the host-call class stops being a guess. Stage 1 derives the class from the `NONBLOCK` bit (`executor/mod.rs:168-180`), so a caller that forgets the bit silently buys the 30-second watchdog on a seat-active path. Here the class is an explicit field and the decoder **refuses** any frame whose flags and payload contradict it.

**Files:**
- Modify: `crates/yserver/src/kms/executor/protocol.rs`
- Modify: `crates/yserver/src/kms/executor/transport.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs:23-27,293-433,483-493` — `dispatch` and `dispatch_for_tests` construct and match on `AtomicRequest`, whose shape changes here. They are updated to the new shape **in this task** so the crate compiles; Task 4 is what replaces `dispatch` itself. Without this the stage would not build between tasks 2 and 4. `pub mod protocol` is widened here too.
- Modify: `crates/yserver/src/kms/mod.rs:18`, `crates/yserver/src/kms/owner/mod.rs`, `crates/yserver/src/kms/owner/identity.rs`, `crates/yserver/src/kms/owner/lifecycle.rs`, `crates/yserver/src/platform/drm.rs:35-37` — the visibility widening the contract's table specifies. These files are listed **and staged**, because a widening declared in prose and absent from the commit is how round 3's B-1 happened.
- Modify: `crates/yserver/src/kms/executor/test_support.rs` — the request builders every later task's tests consume.

**Interfaces:**
- Consumes: `LifecycleEpochId`, `LifecycleTransitionId`, `ClockProbeId`, `HostCallClass` (task 1); `IncarnationId`, `CommitId`, `EventToken`, `ClockEpochId`, `RequestSeq`, `ProtocolError` (stage 1).
- Produces:
  - `AtomicPropertyList { objects: Vec<u32>, count_props: Vec<u32>, props: Vec<u32>, values: Vec<u64> }` with `fn validate(&self) -> Result<(), ProtocolError>`
  - `OutFenceSlot { crtc_id: u32, value_index: u32 }`
  - `HostCallCorrelation` — one type, two variants, echoed verbatim in every reply, with `fn seq(self) -> RequestSeq`
  - `AtomicRequest { correlation, class: HostCallClass, flags: u32, properties: AtomicPropertyList, out_fence_slots: Vec<OutFenceSlot> }`
  - `ClockProbeRequest { correlation }`
  - `HostCallRequest::{Atomic, ClockProbe}` and `HostCallReply::{Accepted, Rejected, ClockProbe}`, each reply carrying `fn correlation(&self) -> HostCallCorrelation`
  - `HandshakeRequest { incarnation, lifecycle_epoch }` / `HandshakeReply { incarnation, lifecycle_epoch, helper_pid }` — **separate types, not host-call variants**, but they carry the epoch like everything else on this wire; see the contract
  - `RequestKind::{Atomic, ClockProbe}`, with `HostCallRequest::kind()` and `HostCallReply::kind()`
  - `DRM_MODE_ATOMIC_NONBLOCK = 0x0200`, `DRM_MODE_ATOMIC_TEST_ONLY = 0x0100`
  - `MAX_ATOMIC_OBJECTS = 256`, `MAX_ATOMIC_PROPS = 1024`, `MAX_OUT_FENCES = 16`, `ATOMIC_HEAD_LEN = 68`, `PROBE_HEAD_LEN = 56`, `MAX_REQUEST_FRAME_LEN = 32 * 1024`
  - `HostCallRequest::{correlation, class}` accessors, used by Task 4's `send` to pick the watchdog and check the reservation kind
  - `encode_request`, `decode_request`, `encode_reply`, `decode_reply`, and `#[cfg(test)] encode_request_unchecked_for_tests`
  - `golden_atomic_request_for_tests()`, `golden_probe_request_for_tests()`, `golden_atomic_correlation_for_tests()`, `golden_probe_correlation_for_tests()` — the exact requests and tuples built inline in the golden tests, factored out so the hostile-frame tests mutate one known-good frame rather than each inventing their own
  - **In `test_support`, for every later task's tests:** `small_atomic_request_for_tests`, `fence_returning_request_for_tests`, `validation_request_for_tests(class)`, `blocking_atomic_request_for_tests`, `probe_request_for_tests`, `three_property_request_for_tests`, `invalid_object_request_for_tests`, `request_with_slots_for_tests(n)`. They are produced **here**, in the task that defines the types they build, so no task consumes a helper from a later one — round 3's B-1 found tasks 3 and 5 doing exactly that.
- Removes: `HostCallClass::from_request` — the class is no longer derivable from flags, it is carried and validated.
- **Widens visibility.** Tasks 3, 4 and 6 place tests under `crates/yserver/tests/`, which compile as *external* crates. `KmsIoExecutor::send` takes a `HostCallRequest`, so if that type stays `pub(crate)` the signature is a private-interface error and no external test can call it. Stage 1 already solved this for its own surface: `HostCallClass`, `HostCallOutcome`, `UnknownReason`, `SubmittingProof`, `ReapProof` and `KmsIoExecutor` are `#[doc(hidden)] pub` (`executor/mod.rs:153-247`), and `tests/executor_substrate.rs` works because of it. Follow that precedent exactly, and no further:

  `#[doc(hidden)] pub` — `protocol` module, `HostCallRequest`, `HostCallReply`, `AtomicRequest`, `ClockProbeRequest`, `AtomicPropertyList`, `OutFenceSlot`, `HostCallCorrelation`, `RequestSeq`, `ProtocolError`, and the four constants `DRM_MODE_ATOMIC_NONBLOCK`, `DRM_MODE_ATOMIC_TEST_ONLY`, `MAX_ATOMIC_PROPS`, `MAX_OUT_FENCES`.

  `#[doc(hidden)] pub` — the `owner` module and `owner::identity`/`owner::lifecycle`, plus `IncarnationId`, `CommitId`, `EventToken`, `ClockEpochId`, `LifecycleEpochId`, `LifecycleTransitionId`, `ClockProbeId`, and their `for_tests` constructors.

  `#[doc(hidden)]` keeps all of it out of rendered docs; it is a test seam, not public API. Task 7 greps that nothing gained a bare `pub`.

- [x] **Step 1: Write the failing offset and correlation tests**

```rust
// crates/yserver/src/kms/executor/protocol.rs
#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn the_declared_head_lengths_equal_the_sums_of_their_fields() {
        // An encoder and decoder sharing one wrong offset pass a round-trip
        // test, so assert the constants independently of any round trip.
        const ATOMIC_FIELD_SUM: usize = 6 * 8 + 1 + 1 + 2 + 4 * 4;
        const PROBE_FIELD_SUM: usize = 6 * 8 + 4 + 4;
        assert_eq!(ATOMIC_HEAD_LEN, ATOMIC_FIELD_SUM);
        assert_eq!(ATOMIC_HEAD_LEN, 68);
        assert_eq!(PROBE_HEAD_LEN, PROBE_FIELD_SUM);
        assert_eq!(PROBE_HEAD_LEN, 56);
        assert_eq!(HEADER_LEN + ATOMIC_HEAD_LEN, 80);
    }

    /// Every atomic head field, at its documented offset, with a distinct
    /// value so a transposition of any two cannot pass.
    #[test]
    fn a_golden_atomic_frame_places_every_field_at_its_documented_offset() {
        let request = AtomicRequest {
            correlation: HostCallCorrelation::Atomic {
                seq: RequestSeq::from_raw(0x11),
                incarnation: IncarnationId::from_raw(0x22),
                lifecycle_epoch: LifecycleEpochId::from_raw(0x33),
                transition: Some(LifecycleTransitionId::from_raw(0x44)),
                commit: CommitId::from_raw(0x55),
                event_token: EventToken::tagged_for_tests(0x66),
            },
            class: HostCallClass::SeatActiveNonblock,
            flags: DRM_MODE_ATOMIC_NONBLOCK,
            properties: AtomicPropertyList {
                objects: vec![0x0A11, 0x0A22],
                count_props: vec![1, 2],
                props: vec![0x0B11, 0x0B22, 0x0B33],
                values: vec![0x0C11, 0x0C22, 0x0C33],
            },
            out_fence_slots: vec![OutFenceSlot { crtc_id: 0x0A11, value_index: 0 }],
        };
        let f = encode_request(&HostCallRequest::Atomic(request));
        let u64_at = |o: usize| u64::from_le_bytes(f[o..o + 8].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap());

        // Envelope: 4-byte magic, 2-byte version, 2-byte kind, 4-byte payload len.
        assert_eq!(&f[0..4], &PROTOCOL_MAGIC);
        assert_eq!(u16::from_le_bytes(f[4..6].try_into().unwrap()), 2, "version");
        assert_eq!(u16::from_le_bytes(f[6..8].try_into().unwrap()), KIND_ATOMIC_REQUEST);
        assert_eq!(u32_at(8) as usize, f.len() - HEADER_LEN, "payload_len");

        // Head, offsets relative to byte 12.
        assert_eq!(u64_at(12), 0x11, "seq @0");
        assert_eq!(u64_at(20), 0x22, "incarnation @8");
        assert_eq!(u64_at(28), 0x33, "lifecycle_epoch @16");
        assert_eq!(u64_at(36), 0x44, "transition @24");
        assert_eq!(u64_at(44), 0x55, "commit @32");
        assert_eq!(
            u64_at(52),
            EventToken::tagged_for_tests(0x66).as_user_data(),
            "event_token @40"
        );
        assert_eq!(f[60], 1, "transition_present @48");
        assert_eq!(f[61], HostCallClass::SeatActiveNonblock.wire_tag(), "class @49");
        assert_eq!(u16::from_le_bytes(f[62..64].try_into().unwrap()), 0, "pad @50");
        assert_eq!(u32_at(64), DRM_MODE_ATOMIC_NONBLOCK, "flags @52");
        assert_eq!(u32_at(68), 2, "object_count @56");
        assert_eq!(u32_at(72), 3, "prop_count @60");
        assert_eq!(u32_at(76), 1, "slot_count @64");

        // Body at byte 80: objects, count_props, props, values, slots.
        assert_eq!(u32_at(80), 0x0A11);
        assert_eq!(u32_at(84), 0x0A22);
        assert_eq!(u32_at(88), 1);
        assert_eq!(u32_at(92), 2);
        assert_eq!(u32_at(96), 0x0B11);
        assert_eq!(u32_at(100), 0x0B22);
        assert_eq!(u32_at(104), 0x0B33);
        assert_eq!(u64_at(108), 0x0C11);
        assert_eq!(u64_at(116), 0x0C22);
        assert_eq!(u64_at(124), 0x0C33);
        assert_eq!(u32_at(132), 0x0A11, "slot crtc_id");
        assert_eq!(u32_at(136), 0, "slot value_index");
        assert_eq!(f.len(), 140);
    }

    #[test]
    fn an_absent_transition_writes_a_zero_flag_and_a_zero_field() {
        let mut request = golden_atomic_request_for_tests();
        let HostCallCorrelation::Atomic { ref mut transition, .. } = request.correlation else {
            unreachable!("golden request is atomic")
        };
        *transition = None;
        let f = encode_request(&HostCallRequest::Atomic(request.clone()));
        assert_eq!(f[60], 0, "transition_present @48");
        assert_eq!(u64::from_le_bytes(f[36..44].try_into().unwrap()), 0, "transition @24");
        let HostCallRequest::Atomic(decoded) = decode_request(&f).expect("decode") else {
            panic!("kind changed across the wire")
        };
        assert_eq!(decoded.correlation, request.correlation);
    }

    #[test]
    fn a_golden_probe_frame_places_every_field_at_its_documented_offset() {
        // The probe correlation is what spec:641-645 requires: incarnation,
        // lifecycle epoch, topology generation, hardware CRTC, CRTC clock
        // epoch and a monotonic ClockProbeId. Stage 1 already carried
        // topology_generation (protocol.rs:87-95); dropping it here would be
        // a regression, so it is asserted at a fixed offset.
        let request = ClockProbeRequest {
            correlation: HostCallCorrelation::ClockProbe {
                seq: RequestSeq::from_raw(0x11),
                incarnation: IncarnationId::from_raw(0x22),
                lifecycle_epoch: LifecycleEpochId::from_raw(0x33),
                topology_generation: 0x44,
                hardware_crtc: 0x55,
                clock_epoch: ClockEpochId::from_raw(0x66),
                probe: ClockProbeId::from_raw(0x77),
            },
        };
        let f = encode_request(&HostCallRequest::ClockProbe(request));
        let u64_at = |o: usize| u64::from_le_bytes(f[o..o + 8].try_into().unwrap());
        assert_eq!(u16::from_le_bytes(f[6..8].try_into().unwrap()), KIND_CLOCK_PROBE_REQUEST);
        assert_eq!(u64_at(12), 0x11, "seq @0");
        assert_eq!(u64_at(20), 0x22, "incarnation @8");
        assert_eq!(u64_at(28), 0x33, "lifecycle_epoch @16");
        assert_eq!(u64_at(36), 0x44, "topology_generation @24");
        assert_eq!(u64_at(44), 0x66, "clock_epoch @32");
        assert_eq!(u64_at(52), 0x77, "probe @40");
        assert_eq!(u32::from_le_bytes(f[60..64].try_into().unwrap()), 0x55, "hardware_crtc @48");
        assert_eq!(u32::from_le_bytes(f[64..68].try_into().unwrap()), 0, "pad @52");
        assert_eq!(f.len(), HEADER_LEN + PROBE_HEAD_LEN);
    }

    #[test]
    fn every_reply_echoes_the_request_correlation() {
        // ID-3: a reply is current only when incarnation, lifecycle epoch,
        // optional transition id and commit id all match. A per-socket
        // sequence number cannot classify a late success against a changed
        // lifecycle.
        let correlation = golden_atomic_correlation_for_tests();
        for reply in [
            HostCallReply::Accepted { correlation, helper_duration_ns: 10, out_fence_mask: 0b11 },
            HostCallReply::Rejected {
                correlation,
                errno: libc::EINVAL,
                helper_duration_ns: 10,
                unexpected_fence_output: false,
            },
        ] {
            assert_eq!(decode_reply(&encode_reply(&reply)).expect("decode"), reply);
            assert_eq!(reply.correlation(), correlation);
        }
    }

    #[test]
    fn a_clock_probe_reply_echoes_the_probe_correlation_not_the_atomic_one() {
        let correlation = golden_probe_correlation_for_tests();
        let reply = HostCallReply::ClockProbe { correlation, sequence: 42, helper_duration_ns: 10 };
        assert_eq!(decode_reply(&encode_reply(&reply)).expect("decode"), reply);
        assert!(matches!(reply.correlation(), HostCallCorrelation::ClockProbe { .. }));
    }

    #[test]
    fn a_reply_of_the_wrong_kind_is_detectable_even_when_its_correlation_matches() {
        // Correlation equality alone would let a probe request receive an
        // atomic Accepted carrying the probe's own correlation, and surface as
        // an atomic acceptance. The kinds must be compared too.
        let probe = golden_probe_correlation_for_tests();
        let accepted = HostCallReply::Accepted {
            correlation: probe,
            helper_duration_ns: 1,
            out_fence_mask: 0,
        };
        assert_eq!(accepted.correlation(), probe, "the correlation genuinely matches");
        assert_ne!(
            accepted.kind(),
            HostCallRequest::ClockProbe(golden_probe_request_for_tests()).kind(),
            "but the kind does not, which is what makes this detectable"
        );
    }

    #[test]
    fn a_reply_whose_lifecycle_epoch_differs_is_not_equal_to_the_sent_tuple() {
        let sent = golden_atomic_correlation_for_tests();
        let HostCallCorrelation::Atomic {
            seq, incarnation, transition, commit, event_token, ..
        } = sent else {
            unreachable!("golden correlation is atomic")
        };
        let stale = HostCallCorrelation::Atomic {
            seq,
            incarnation,
            lifecycle_epoch: LifecycleEpochId::from_raw(9999),
            transition,
            commit,
            event_token,
        };
        assert_ne!(sent, stale);
        assert_eq!(sent.seq(), stale.seq(), "a matching seq must not imply a matching tuple");
    }

    #[test]
    fn the_handshake_is_not_a_host_call_and_round_trips_on_its_own_frames() {
        // Task 6 needs a reply proving the helper reached its serve loop with
        // every inherited descriptor adopted. That exchange happens BEFORE any
        // identity exists, so it cannot carry a correlation — which is exactly
        // why it is not a HostCallRequest. ID-3 requires every executor
        // request to carry the lifecycle epoch; a Ready variant inside the
        // host-call enum would be a standing exception to that rule, and would
        // force correlation() and class() to return Option for every caller.
        let request = HandshakeRequest {
            incarnation: IncarnationId::first(),
            lifecycle_epoch: LifecycleEpochId::first(),
        };
        assert_eq!(
            decode_handshake_request(&encode_handshake_request(&request)).expect("decode"),
            request
        );
        let reply = HandshakeReply {
            incarnation: IncarnationId::first(),
            lifecycle_epoch: LifecycleEpochId::first(),
            helper_pid: 4321,
        };
        assert_eq!(decode_handshake_reply(&encode_handshake_reply(&reply)).expect("decode"), reply);
    }

    #[test]
    fn the_handshake_carries_the_epoch_like_every_other_frame_on_this_wire() {
        // spec:416-425 says "Every executor request/reply and commit record
        // carries the epoch", unqualified. Revision 3 exempted the handshake
        // on the grounds that it "is not an executor request"; that was an
        // invented exception. Both identities exist before the spawn, so the
        // frame split costs nothing here.
        let request = HandshakeRequest {
            incarnation: IncarnationId::from_raw(7),
            lifecycle_epoch: LifecycleEpochId::from_raw(9),
        };
        let f = encode_handshake_request(&request);
        assert_eq!(u64::from_le_bytes(f[12..20].try_into().unwrap()), 7, "incarnation @0");
        assert_eq!(u64::from_le_bytes(f[20..28].try_into().unwrap()), 9, "lifecycle_epoch @8");
    }

    #[test]
    fn a_handshake_frame_is_not_decodable_as_a_host_call_and_the_reverse() {
        // The two frame families share a transport, so each decoder must
        // reject the other's kind rather than misread it.
        let handshake = encode_handshake_request(&HandshakeRequest {
            incarnation: IncarnationId::first(),
            lifecycle_epoch: LifecycleEpochId::first(),
        });
        assert!(matches!(decode_request(&handshake), Err(ProtocolError::Kind(_))));
        let host_call = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));
        assert!(matches!(decode_handshake_request(&host_call), Err(ProtocolError::Kind(_))));
    }

    #[test]
    fn every_host_call_request_carries_a_lifecycle_epoch() {
        // ID-3, asserted structurally: correlation() is total and bare, so no
        // request variant can exist without one.
        for request in [
            HostCallRequest::Atomic(golden_atomic_request_for_tests()),
            HostCallRequest::ClockProbe(golden_probe_request_for_tests()),
        ] {
            let epoch = match request.correlation() {
                HostCallCorrelation::Atomic { lifecycle_epoch, .. }
                | HostCallCorrelation::ClockProbe { lifecycle_epoch, .. } => lifecycle_epoch,
            };
            assert_ne!(epoch.get(), 0);
        }
    }
}
```

- [x] **Step 2: Write the failing validation and hostile-frame tests**

```rust
// crates/yserver/src/kms/executor/protocol.rs, same #[cfg(test)] module
    #[test]
    fn property_list_counts_must_agree() {
        let cases = [
            (AtomicPropertyList { objects: vec![31, 42], count_props: vec![2],
                                  props: vec![7, 8, 9], values: vec![1, 2, 3] },
             ProtocolError::Field("count_props length")),
            (AtomicPropertyList { objects: vec![31], count_props: vec![2],
                                  props: vec![7, 8, 9], values: vec![1, 2, 3] },
             ProtocolError::Field("prop count sum")),
            (AtomicPropertyList { objects: vec![31], count_props: vec![3],
                                  props: vec![7, 8, 9], values: vec![1, 2] },
             ProtocolError::Field("value count")),
        ];
        for (list, expected) in cases {
            assert_eq!(list.validate(), Err(expected));
        }
    }

    #[test]
    fn oversized_property_lists_are_rejected_before_the_wire() {
        let list = AtomicPropertyList {
            objects: vec![1],
            count_props: vec![(MAX_ATOMIC_PROPS + 1) as u32],
            props: vec![1; MAX_ATOMIC_PROPS + 1],
            values: vec![0; MAX_ATOMIC_PROPS + 1],
        };
        assert_eq!(list.validate(), Err(ProtocolError::Field("prop count limit")));

        let too_many_objects = AtomicPropertyList {
            objects: vec![1; MAX_ATOMIC_OBJECTS + 1],
            count_props: vec![0; MAX_ATOMIC_OBJECTS + 1],
            props: vec![],
            values: vec![],
        };
        assert_eq!(too_many_objects.validate(), Err(ProtocolError::Field("object count limit")));
    }

    /// A decoder that allocates from wire-provided counts before checking
    /// them can be made to attempt a multi-gigabyte reservation by a frame
    /// of eighty bytes. The counts are checked against the caps and against
    /// the frame's own length before a single `Vec` is created.
    #[test]
    fn declared_counts_are_bounded_before_anything_is_allocated() {
        let mut f = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));
        for (offset, value, expected) in [
            (68, u32::MAX, ProtocolError::Field("object count limit")),
            (72, u32::MAX, ProtocolError::Field("prop count limit")),
            (76, u32::MAX, ProtocolError::Field("slot count limit")),
        ] {
            let saved = f[offset..offset + 4].to_vec();
            f[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(decode_request(&f), Err(expected));
            f[offset..offset + 4].copy_from_slice(&saved);
        }
    }

    #[test]
    fn declared_counts_must_match_the_frames_own_length() {
        // Counts inside the caps but inconsistent with payload_len are a
        // length error, not a short read past the end of the buffer.
        let mut f = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));
        f[68..72].copy_from_slice(&1u32.to_le_bytes()); // object_count 2 -> 1
        assert_eq!(decode_request(&f), Err(ProtocolError::Length));
    }

    #[test]
    fn an_out_fence_slot_index_must_be_inside_the_value_array() {
        let request = AtomicRequest {
            properties: AtomicPropertyList { objects: vec![42], count_props: vec![1],
                                             props: vec![9], values: vec![0] },
            out_fence_slots: vec![OutFenceSlot { crtc_id: 42, value_index: 1 }],
            ..golden_atomic_request_for_tests()
        };
        let frame = encode_request(&HostCallRequest::Atomic(request));
        assert_eq!(decode_request(&frame), Err(ProtocolError::Field("out fence slot index")));
    }

    /// Two slots pointing at one value index would make the helper patch two
    /// holder addresses into the same u64, so the second overwrites the
    /// first and one CRTC silently loses its completion evidence. Two slots
    /// naming one CRTC breaks the one-holder-per-CRTC model the same way.
    #[test]
    fn duplicate_slot_indices_and_duplicate_crtcs_are_rejected() {
        let duplicate_index = AtomicRequest {
            properties: AtomicPropertyList { objects: vec![42, 43], count_props: vec![1, 1],
                                             props: vec![9, 9], values: vec![0, 0] },
            out_fence_slots: vec![
                OutFenceSlot { crtc_id: 42, value_index: 0 },
                OutFenceSlot { crtc_id: 43, value_index: 0 },
            ],
            ..golden_atomic_request_for_tests()
        };
        assert_eq!(
            decode_request(&encode_request(&HostCallRequest::Atomic(duplicate_index))),
            Err(ProtocolError::Field("duplicate out fence slot index"))
        );

        let duplicate_crtc = AtomicRequest {
            properties: AtomicPropertyList { objects: vec![42, 43], count_props: vec![1, 1],
                                             props: vec![9, 9], values: vec![0, 0] },
            out_fence_slots: vec![
                OutFenceSlot { crtc_id: 42, value_index: 0 },
                OutFenceSlot { crtc_id: 42, value_index: 1 },
            ],
            ..golden_atomic_request_for_tests()
        };
        assert_eq!(
            decode_request(&encode_request(&HostCallRequest::Atomic(duplicate_crtc))),
            Err(ProtocolError::Field("duplicate out fence slot crtc"))
        );
    }

    #[test]
    fn more_out_fence_slots_than_the_cap_are_rejected() {
        let n = MAX_OUT_FENCES + 1;
        let request = AtomicRequest {
            properties: AtomicPropertyList {
                objects: (0..n as u32).collect(),
                count_props: vec![1; n],
                props: vec![9; n],
                values: vec![0; n],
            },
            out_fence_slots: (0..n)
                .map(|i| OutFenceSlot { crtc_id: i as u32, value_index: i as u32 })
                .collect(),
            ..golden_atomic_request_for_tests()
        };
        assert_eq!(
            decode_request(&encode_request(&HostCallRequest::Atomic(request))),
            Err(ProtocolError::Field("slot count limit"))
        );
    }

    #[test]
    fn truncation_at_every_length_is_a_length_error_not_a_short_read() {
        let frame = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));
        for cut in 0..frame.len() {
            assert!(decode_request(&frame[..cut]).is_err(), "truncation at {cut} decoded");
        }
    }
```

- [x] **Step 3: Write the failing class-agreement tests**

```rust
// crates/yserver/src/kms/executor/protocol.rs, same #[cfg(test)] module

    /// COMMIT-5 and the ValidationOnly rule are enforced here, not by
    /// convention at the call site. A live seat-active commit that forgets
    /// NONBLOCK must not reach the kernel, and a live request must not be
    /// able to label itself validation to buy a different watchdog or to
    /// escape the submitted-commit slot.
    #[test]
    fn a_frame_whose_flags_contradict_its_class_is_rejected() {
        let cases = [
            (HostCallClass::SeatActiveNonblock, 0,
             "seat-active nonblock requires NONBLOCK"),
            (HostCallClass::SeatActiveNonblock, DRM_MODE_ATOMIC_NONBLOCK | DRM_MODE_ATOMIC_TEST_ONLY,
             "a live commit must not set TEST_ONLY"),
            (HostCallClass::SeatActiveValidation, DRM_MODE_ATOMIC_NONBLOCK | DRM_MODE_ATOMIC_TEST_ONLY,
             "validation omits NONBLOCK"),
            (HostCallClass::SeatActiveValidation, 0,
             "validation requires TEST_ONLY"),
            (HostCallClass::ColdStartOrOfflineBlocking, DRM_MODE_ATOMIC_NONBLOCK,
             "a blocking call must not set NONBLOCK"),
            (HostCallClass::ColdStartOrOfflineBlocking, DRM_MODE_ATOMIC_TEST_ONLY,
             "a blocking commit must not set TEST_ONLY"),
            (HostCallClass::ColdStartOrOfflineValidation, DRM_MODE_ATOMIC_NONBLOCK | DRM_MODE_ATOMIC_TEST_ONLY,
             "validation omits NONBLOCK at either boundary"),
            (HostCallClass::ColdStartOrOfflineValidation, 0,
             "validation requires TEST_ONLY at either boundary"),
        ];
        for (class, flags, why) in cases {
            let request = AtomicRequest { class, flags, ..golden_atomic_request_for_tests() };
            let frame = encode_request_unchecked_for_tests(&HostCallRequest::Atomic(request));
            assert_eq!(
                decode_request(&frame),
                Err(ProtocolError::Field("class flag agreement")),
                "{why}"
            );
        }
    }

    #[test]
    fn neither_validation_class_may_request_out_fences() {
        // spec:320-325 — TEST_ONLY creates no out-fence. Written over both
        // classes: covering only the seat-active one left the cold/offline
        // variant able to carry slots through the decoder.
        for class in [HostCallClass::SeatActiveValidation,
                      HostCallClass::ColdStartOrOfflineValidation] {
        let request = AtomicRequest {
            class,
            flags: DRM_MODE_ATOMIC_TEST_ONLY,
            properties: AtomicPropertyList { objects: vec![42], count_props: vec![1],
                                             props: vec![9], values: vec![0] },
            out_fence_slots: vec![OutFenceSlot { crtc_id: 42, value_index: 0 }],
            ..golden_atomic_request_for_tests()
        };
        let frame = encode_request_unchecked_for_tests(&HostCallRequest::Atomic(request));
        assert_eq!(
            decode_request(&frame),
            Err(ProtocolError::Field("validation out fence slot")),
            "{class:?}"
        );
        // The encoder must refuse it too, not only the decoder.
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            encode_request(&HostCallRequest::Atomic(request))
        }))
        .is_err(), "{class:?} encoder accepted an out-fence slot");
        }
    }

    #[test]
    fn an_unknown_class_tag_is_rejected_rather_than_defaulted() {
        let mut f = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));
        f[61] = 0xFF;
        assert_eq!(decode_request(&f), Err(ProtocolError::Field("class tag")));
    }

    #[test]
    fn the_encoder_refuses_the_same_contradictions_it_decodes() {
        // The parent must not be able to put a contradictory frame on the
        // wire in the first place; encode_request_unchecked_for_tests exists
        // only so the decoder can be tested against frames a correct encoder
        // never produces.
        let request = AtomicRequest {
            class: HostCallClass::SeatActiveNonblock,
            flags: 0,
            ..golden_atomic_request_for_tests()
        };
        // AssertUnwindSafe: `request` owns Vecs, so it is not UnwindSafe by
        // default, and nothing observes it after the expected panic.
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                encode_request(&HostCallRequest::Atomic(request))
            }))
            .is_err()
        );
    }
```

- [x] **Step 4: Run the tests to verify they fail**

Run each filter separately — `cargo test` takes one positional filter:

```bash
cargo test -p yserver kms::executor::protocol::wire_tests
```
Expected: FAIL — `HostCallCorrelation`, `AtomicPropertyList`, `ATOMIC_HEAD_LEN` and the class-agreement rules do not exist.

- [x] **Step 5: Write the implementation**

```rust
pub(crate) const PROTOCOL_VERSION: u16 = 2;
pub(crate) const ATOMIC_HEAD_LEN: usize = 68;
pub(crate) const PROBE_HEAD_LEN: usize = 56;
pub(crate) const MAX_ATOMIC_OBJECTS: usize = 256;
pub(crate) const MAX_ATOMIC_PROPS: usize = 1024;
pub(crate) const MAX_OUT_FENCES: usize = 16;
pub(crate) const MAX_REQUEST_FRAME_LEN: usize = 32 * 1024;

pub(crate) const DRM_MODE_ATOMIC_TEST_ONLY: u32 = 0x0100;
pub(crate) const DRM_MODE_ATOMIC_NONBLOCK: u32 = 0x0200;

const _: () = assert!(ATOMIC_HEAD_LEN == 6 * 8 + 1 + 1 + 2 + 4 * 4);
const _: () = assert!(PROBE_HEAD_LEN == 6 * 8 + 4 + 4);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum HostCallCorrelation {
    Atomic {
        seq: RequestSeq,
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        transition: Option<LifecycleTransitionId>,
        commit: CommitId,
        event_token: EventToken,
    },
    ClockProbe {
        seq: RequestSeq,
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        topology_generation: u64,
        hardware_crtc: u32,
        clock_epoch: ClockEpochId,
        probe: ClockProbeId,
    },
}

impl HostCallCorrelation {
    pub(crate) const fn seq(self) -> RequestSeq {
        match self {
            Self::Atomic { seq, .. } | Self::ClockProbe { seq, .. } => seq,
        }
    }
}
```

Atomic request frame layout, little-endian, offsets relative to the end of the 12-byte envelope:

```text
head @0..68  seq u64             @0    incarnation u64      @8
             lifecycle_epoch u64 @16   transition u64       @24
             commit u64          @32   event_token u64      @40
             transition_present u8 @48 class u8             @49
             pad u16             @50   flags u32            @52
             object_count u32    @56   prop_count u32       @60
             slot_count u32      @64
body @68     objects[object_count]     u32
             count_props[object_count] u32
             props[prop_count]         u32
             values[prop_count]        u64
             slots[slot_count]         { crtc_id u32, value_index u32 }
```

Clock-probe request frame layout, same convention:

```text
head @0..56  seq u64                 @0   incarnation u64  @8
             lifecycle_epoch u64     @16  topology_generation u64 @24
             clock_epoch u64         @32  probe u64        @40
             hardware_crtc u32       @48  pad u32          @52
```

`AtomicPropertyList::validate` returns, in this order:
`Field("object count limit")` if `objects.len() > MAX_ATOMIC_OBJECTS`;
`Field("count_props length")` if `count_props.len() != objects.len()`;
`Field("prop count limit")` if the `u64` sum of `count_props` exceeds `MAX_ATOMIC_PROPS`;
`Field("prop count sum")` if that sum differs from `props.len()`;
`Field("value count")` if `values.len() != props.len()`.
The sum is accumulated in `u64` so no `u32` overflow can make an oversized list look small.

`encode_request` calls `validate()`, then `assert_class_agreement()`, then `assert_no_validation_out_fences()`, panicking on any violation: the owner must never construct an invalid or mislabelled request, and a panic in the parent is preferable to handing a short array or a mis-classed commit to a helper that passes it to the kernel. All three checks are written against `class.is_validation()` rather than against a named variant, so adding a class cannot silently leave one uncovered. `encode_request_unchecked_for_tests` is `#[cfg(test)]` and skips both, so the decoder can be tested against frames a correct encoder never emits.

`decode_request` proceeds strictly in this order, and allocates nothing before step 5:

1. Envelope: magic, `PROTOCOL_VERSION`, known kind, and `payload_len` equal to `frame.len() - HEADER_LEN`.
2. Frame length at most `MAX_REQUEST_FRAME_LEN`, and at least `HEADER_LEN + ATOMIC_HEAD_LEN` for the atomic kind.
3. Read the three counts. `object_count > MAX_ATOMIC_OBJECTS` → `Field("object count limit")`; `prop_count > MAX_ATOMIC_PROPS` → `Field("prop count limit")`; `slot_count > MAX_OUT_FENCES` → `Field("slot count limit")`. These precede every other check because they are the only wire values that size an allocation.
4. Compute the exact body length with `checked_mul`/`checked_add` — `2*4*object_count + 4*prop_count + 8*prop_count + 8*slot_count` — and require it to equal `payload_len - ATOMIC_HEAD_LEN`, else `ProtocolError::Length`. After the caps in step 3 this arithmetic cannot overflow, but it is written checked so a future cap increase cannot silently make it wrap.
5. Allocate and read the five arrays.
6. `class` byte to `HostCallClass` — an unrecognised tag is `Field("class tag")`, never a default.
7. `assert_class_agreement`: `SeatActiveNonblock` requires `NONBLOCK` set and `TEST_ONLY` clear; `SeatActiveValidation` requires `TEST_ONLY` set and `NONBLOCK` clear; `ColdStartOrOfflineBlocking` requires both clear. Any violation is `Field("class flag agreement")`.
8. **Any** class for which `is_validation()` holds, with a non-empty slot table, is `Field("validation out fence slot")` — `SeatActiveValidation` and `ColdStartOrOfflineValidation` alike (`spec:320-325,2126`).
9. `validate()` on the reconstructed list.
10. Every `value_index < values.len()` → else `Field("out fence slot index")`; no repeated `value_index` → else `Field("duplicate out fence slot index")`; no repeated `crtc_id` → else `Field("duplicate out fence slot crtc")`. Duplicates are detected with a linear scan over at most `MAX_OUT_FENCES` entries; no hashing is needed at this size.

`HostCallReply::Accepted` carries `out_fence_mask: u32` rather than a count, so the parent learns *which* slots produced a descriptor. Its bit `i` corresponds to `out_fence_slots[i]`; the mask is safe in a `u32` because `MAX_OUT_FENCES` is 16, which step 3 has already enforced. `decode_reply` rejects a mask with bits set above the request's slot count.

**The handshake is a separate frame family, not a host-call variant**, and it carries the epoch. Exactly as the contract specifies:

```rust
pub struct HandshakeRequest { pub incarnation: IncarnationId,
                              pub lifecycle_epoch: LifecycleEpochId }
pub struct HandshakeReply   { pub incarnation: IncarnationId,
                              pub lifecycle_epoch: LifecycleEpochId,
                              pub helper_pid: u32 }
```

Fixed-length frames under their own kinds `KIND_HANDSHAKE_REQUEST` and `KIND_HANDSHAKE_REPLY`, with `encode_handshake_request(&HandshakeRequest)`, `decode_handshake_request`, `encode_handshake_reply(&HandshakeReply)` and `decode_handshake_reply`. They never enter `HostCallRequest` or `HostCallReply`.

The split is mechanical, not normative. `ID-3` applies to the handshake exactly as to everything else — an earlier revision claimed the handshake "precedes every identity" and was exempt, which was an invented exception: `platform_init` allocates the incarnation before the spawn and `LifecycleEpochId::first()` is available, so both identities exist when the frame is built. The reason for the separate family is only this: Task 4's `send` needs `request.class()` for the watchdog and the reservation check, and its tests need `request.correlation()` to compare against the reply. Both must be **total and bare**. A variant carrying neither forces every caller through an `Option` for a case that cannot occur on the host-call path. Keeping the two families apart is what lets `correlation()` return `HostCallCorrelation` and `class()` return `HostCallClass`, with no `unreachable!()` arm anywhere. Nothing about that requires dropping the epoch.

`decode_request` rejects a handshake kind with `ProtocolError::Kind`, and `decode_handshake_request` rejects a host-call kind the same way, so sharing one transport cannot let either be misread as the other.

`transport.rs` receives into a heap `Box<[u8; MAX_REQUEST_FRAME_LEN]>` rather than a stack array — 32 KiB on the stack of every receive is avoidable — and `send_frame` returns `InvalidInput` above that bound. The transport's blocking behaviour is unchanged in this task; Task 4 makes only the parent endpoint non-blocking.

- [x] **Step 6: Run the tests to verify they pass**

```bash
cargo test -p yserver kms::executor
```
Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add crates/yserver/src/kms/executor/protocol.rs crates/yserver/src/kms/executor/transport.rs \
        crates/yserver/src/kms/executor/mod.rs crates/yserver/src/kms/executor/test_support.rs \
        crates/yserver/src/kms/mod.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/src/kms/owner/identity.rs crates/yserver/src/kms/owner/lifecycle.rs \
        crates/yserver/src/platform/drm.rs
git commit -m "feat(kms): carry an atomic property payload and a reply correlation tuple"
```

---

### Task 3: Helper-side materialization and `OUT_FENCE_PTR` holder ownership

**Status: EXECUTED at `d848ff6c`.** What executing it required, beyond the
text below:

- R1: `dispatch_and_wait_for_tests` takes `&HostCallRequest` by reference (`(&mut KmsIoExecutor, &HostCallRequest) -> HostCallOutcome`).
- R2: The ioctl-capture test is placed as a unit test in `helper.rs` rather than in external integration tests, and `capture_submitted_ioctl_for_tests` returns an owned snapshot (`CapturedSubmittedIoctl`) rather than raw pointers into a dropped `PreparedAtomic`.
- `ScriptedReply` was defined in `test_support.rs` and wired through `StubBehaviour::Scripted(ScriptedReply)` so the scripted tests execute against the isolated helper process.
- `DrmModeAtomic` was made `pub(crate)` to satisfy Rust's private interface visibility rules with `PreparedAtomic::build_drm_request`.
- `HostCallOutcome` was extended with `out_fence_mask: u32` on `Accepted`, `unexpected_fence_output: bool` on `Rejected`, and `ProbeAccepted` / `ValidationAbandoned` variants, while preserving pattern matching compatibility with stage 1's regression net (`tests/executor_substrate.rs`).

`§10.2`: "The executor owns stable `OUT_FENCE_PTR` holder memory until the ioctl has returned and transfers one terminal reply plus every resulting fd in one message-boundary-preserving IPC operation." The holder must live in the helper's address space; an owner-side pointer is meaningless across processes.

**Corrections from review:** M-1 (the parent-side `FdLedger` could not observe an `OwnedFd` close and its module was never declared), M-2 (the "kernel errno" integration test asserted on a scripted stub), B-8 (`PreparedAtomic` had no `holder_addresses`).

**Files:**
- Modify: `crates/yserver/src/kms/executor/helper.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs` (reply validation)
- Modify: `crates/yserver/src/kms/executor/test_support.rs`

**Interfaces:**
- Consumes: `AtomicRequest`, `AtomicPropertyList`, `OutFenceSlot`, `HostCallCorrelation` (task 2).
- Produces:
  - helper behaviour only, plus `TestDevice::{open_stub, open_never_a_drm_device, open_real_drm_or_ignore}`
  - `HolderLedger` — a `#[cfg(test)]` counter **inside `helper.rs`**, over descriptors that same module created
  - `atomic_ioctl(fd, &DrmModeAtomic) -> i32` — the single raw-ioctl seam `execute_atomic` calls, and `capture_submitted_ioctl_for_tests(&AtomicRequest)`, which swaps in a capturing implementation and returns the `DrmModeAtomic` that would have reached the kernel
  - In `test_support`: `spawn_real_helper_for_tests(&TestDevice)`, `spawn_scripted_helper_for_tests(ScriptedReply)`, and `dispatch_and_wait_for_tests(&mut KmsIoExecutor, &HostCallRequest) -> HostCallOutcome`. **Produced here, not in task 4**, because task 3's gate runs first; task 4 only re-implements `dispatch_and_wait_for_tests` over the async API without changing its signature.
  - `HostCallOutcome::Accepted { helper_duration_ns, round_trip_ns, out_fences, out_fence_mask }` and `Rejected { errno, helper_duration_ns, round_trip_ns, unexpected_fence_output }` — the correlation lives on Task 4's `HostCallEvent`, not inside the outcome, so it is not duplicated here

- [x] **Step 1: Write the failing tests**

Helper-side unit tests, in `helper.rs`, because a parent-process counter cannot
observe a descriptor the helper closes:

```rust
#[test]
fn every_out_fence_slot_is_patched_with_the_address_of_its_live_holder() {
    // Not merely "nonzero": any constant would pass that. The patched value
    // must be the address of the holder that is live at ioctl time, which a
    // later reallocation of `holders` would break.
    let atomic = atomic_request_for_tests(
        AtomicPropertyList { objects: vec![0], count_props: vec![2],
                             props: vec![0, 1], values: vec![0xdead_beef, 0] },
        &[OutFenceSlot { crtc_id: 0, value_index: 1 }],
    );
    let prepared = prepare_atomic_for_tests(&atomic);
    assert_eq!(prepared.values[0], 0xdead_beef, "untouched entries survive verbatim");
    assert_eq!(prepared.values[1], prepared.holder_address(0));
}

#[test]
fn the_prepared_arrays_are_not_reallocated_after_addresses_are_installed() {
    let atomic = atomic_request_for_tests(large_property_list_for_tests(), &[]);
    let prepared = prepare_atomic_for_tests(&atomic);
    let before = (prepared.objects.as_ptr(), prepared.values.as_ptr());
    let req = prepared.build_drm_request();
    assert_eq!((prepared.objects.as_ptr(), prepared.values.as_ptr()), before);
    assert_eq!(req.objs_ptr, prepared.objects.as_ptr() as usize as u64);
    assert_eq!(req.count_objs as usize, prepared.objects.len());
}

#[test]
fn a_rejected_ioctl_closes_an_unexpected_holder_exactly_once_and_reports_it() {
    // Spec 10.2: "Diagnose any defensively unexpected non-negative output and
    // close it exactly once."
    // HolderLedger is legitimate here and only here: the scripted path in
    // this same module is what creates the holder descriptors, so it can
    // count their closes. It is not a general fd observer.
    let ledger = HolderLedger::install();
    let (reply, fences) = execute_atomic_with_scripted_result_for_tests(
        ScriptedIoctl::Rejected { errno: libc::EINVAL, holder_writes: &[0] },
    );
    assert!(fences.is_empty());
    assert_eq!(ledger.closes(), 1);
    assert!(matches!(reply, HostCallReply::Rejected { unexpected_fence_output: true, .. }));
}

#[test]
fn a_live_success_with_a_missing_holder_reports_the_gap_rather_than_repairing_it() {
    let (reply, fences) = execute_atomic_with_scripted_result_for_tests(
        ScriptedIoctl::Accepted { holder_writes: &[Some(7), None] },
    );
    assert_eq!(fences.len(), 1);
    assert!(matches!(reply, HostCallReply::Accepted { out_fence_mask: 0b01, .. }));
}
```

Parent-side integration tests, in `crates/yserver/tests/executor_async.rs`:

```rust
#[test]
fn the_submitted_ioctl_argument_is_the_prepared_arrays() {
    // The link ENOTTY cannot make. `execute_atomic` calls the raw ioctl
    // through one seam, `atomic_ioctl(fd, &DrmModeAtomic) -> i32`, which
    // under #[cfg(test)] can be swapped for a capturing implementation. This
    // asserts the struct that would reach the kernel, so a helper still
    // submitting count_objs = 0 with null pointers fails here even though it
    // would pass an errno check.
    let atomic = atomic_request_for_tests(
        AtomicPropertyList { objects: vec![31, 42], count_props: vec![1, 2],
                             props: vec![7, 8, 9], values: vec![1, 2, 3] },
        &[OutFenceSlot { crtc_id: 31, value_index: 0 }],
    );
    let captured = capture_submitted_ioctl_for_tests(&atomic);
    assert_eq!(captured.count_objs, 2, "the object count actually submitted");
    assert_eq!(captured.objs_as_slice(), &[31, 42]);
    assert_eq!(captured.count_props_as_slice(), &[1, 2]);
    assert_eq!(captured.props_as_slice(), &[7, 8, 9]);
    assert_eq!(captured.values_as_slice().len(), 3);
    assert_ne!(captured.objs_ptr, 0, "not a null pointer");
}

#[test]
fn the_real_helper_reaches_the_raw_ioctl_at_all() {
    // Complements the test above rather than replacing it. This one proves
    // the real helper process, over the real transport, reaches a real
    // ioctl: the kernel's own dispatch returns ENOTTY on a non-DRM
    // descriptor, which is unreachable unless the call was made. It does NOT
    // prove which argument went in — /dev/null inspects nothing — and the
    // capture test above is what covers that.
    let device = TestDevice::open_never_a_drm_device(); // /dev/null
    let mut executor = spawn_real_helper_for_tests(&device);
    let outcome = dispatch_and_wait_for_tests(&mut executor, three_property_request_for_tests());
    match outcome {
        HostCallOutcome::Rejected { errno, .. } => assert_eq!(
            errno,
            libc::ENOTTY,
            "a non-DRM descriptor must fail in ioctl dispatch, proving the call was made"
        ),
        other => panic!("expected an explicit rejection from the real ioctl, got {other:?}"),
    }
}

#[test]
#[ignore = "requires a real DRM device with master; run explicitly"]
fn the_helper_reports_a_kernel_rejection_of_an_invalid_object_on_real_hardware() {
    // Object id 0 is never a valid DRM object, so a real device must reject
    // rather than accept an empty request. The attribute is what keeps this
    // out of the ordinary suite; the previous revision only said in prose
    // that it was ignored, so it ran and reported PASS on every machine
    // without a device.
    //
    // The errno is deliberately not pinned to EINVAL. Opening a DRM node
    // establishes neither master status nor atomic-client capability, so
    // EACCES, EPERM or EOPNOTSUPP can precede object validation. What must
    // hold is that the kernel rejected, and that the helper reported the
    // rejection rather than an empty success.
    let Some(device) = TestDevice::open_real_drm_or_ignore() else {
        panic!("no DRM device; this test is #[ignore]d and was run explicitly")
    };
    let mut executor = spawn_real_helper_for_tests(&device);
    let outcome = dispatch_and_wait_for_tests(&mut executor, invalid_object_request_for_tests());
    match outcome {
        HostCallOutcome::Rejected { errno, .. } => assert_ne!(errno, 0),
        other => panic!("expected an explicit rejection, got {other:?}"),
    }
}

#[test]
fn a_reply_bitmap_outside_the_declared_slot_table_is_malformed() {
    // The count check alone is insufficient: a reply declaring zero slots
    // could set bit 31, carry one fd, pass the count, and leave the owner
    // treating an empty expected set as complete while an fd is dropped.
    for (slot_count, mask, fds) in [(0usize, 0b1000_0000_0000_0000_0000_0000_0000_0000u32, 1usize),
                                    (1, 0b10, 1)] {
        let mut executor = spawn_scripted_helper_for_tests(ScriptedReply::Accepted { mask, fds });
        let outcome = dispatch_and_wait_for_tests(&mut executor, request_with_slots_for_tests(slot_count));
        assert!(matches!(outcome, HostCallOutcome::Unknown(UnknownReason::MalformedReply)),
                "slot_count={slot_count} mask={mask:#b}");
    }
}

#[test]
fn a_reply_whose_correlation_does_not_match_is_malformed_never_a_rejection() {
    let mut executor = spawn_scripted_helper_for_tests(ScriptedReply::StaleCorrelation);
    let outcome = dispatch_and_wait_for_tests(&mut executor, small_atomic_request_for_tests());
    assert!(matches!(outcome, HostCallOutcome::Unknown(UnknownReason::MalformedReply)));
}

#[test]
fn the_stub_target_always_exists_so_this_suite_cannot_silently_run_nothing() {
    // Rust's harness has no runtime skip: printing a message and returning
    // early reports a PASS for a test that exercised nothing, which is how the
    // only real coverage of property materialization would vanish in CI.
    let device = TestDevice::open_stub();
    assert!(device.is_stub());
}
```

- [x] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver --test executor_async` and `cargo test -p yserver kms::executor::helper`
Expected: FAIL — the helper still submits an empty request with null pointers, and `prepare_atomic`, `HolderLedger` and `TestDevice::open_never_a_drm_device` do not exist.

- [x] **Step 3: Write the implementation**

Split preparation from execution so the pointer discipline is unit-testable
without an ioctl:

```rust
pub(crate) struct PreparedAtomic {
    pub(crate) flags: u32,
    pub(crate) user_data: u64,
    pub(crate) objects: Vec<u32>,
    pub(crate) count_props: Vec<u32>,
    pub(crate) props: Vec<u32>,
    pub(crate) values: Vec<u64>,
    pub(crate) holders: Vec<i32>,
}

impl PreparedAtomic {
    /// The address installed into the value slot for out-fence `slot_idx`.
    /// Exists so the unit test above can assert the *identity* of the holder
    /// pointer rather than merely that the value changed.
    #[cfg(test)]
    pub(crate) fn holder_address(&self, slot_idx: usize) -> u64 {
        std::ptr::from_ref(&self.holders[slot_idx]) as usize as u64
    }

    pub(crate) fn build_drm_request(&self) -> DrmModeAtomic {
        DrmModeAtomic {
            flags: self.flags,
            count_objs: self.objects.len() as u32,
            objs_ptr: if self.objects.is_empty() { 0 } else { self.objects.as_ptr() as usize as u64 },
            count_props_ptr: if self.count_props.is_empty() { 0 } else { self.count_props.as_ptr() as usize as u64 },
            props_ptr: if self.props.is_empty() { 0 } else { self.props.as_ptr() as usize as u64 },
            prop_values_ptr: if self.values.is_empty() { 0 } else { self.values.as_ptr() as usize as u64 },
            reserved: 0,
            user_data: self.user_data,
        }
    }
}

pub(crate) fn prepare_atomic(atomic: &AtomicRequest) -> PreparedAtomic {
    let HostCallCorrelation::Atomic { event_token, .. } = atomic.correlation else {
        return PreparedAtomic {
            flags: atomic.flags,
            user_data: 0,
            objects: atomic.properties.objects.clone(),
            count_props: atomic.properties.count_props.clone(),
            props: atomic.properties.props.clone(),
            values: atomic.properties.values.clone(),
            holders: vec![-1; atomic.out_fence_slots.len()],
        };
    };
    let mut prepared = PreparedAtomic {
        flags: atomic.flags,
        user_data: event_token.as_user_data(),
        objects: atomic.properties.objects.clone(),
        count_props: atomic.properties.count_props.clone(),
        props: atomic.properties.props.clone(),
        values: atomic.properties.values.clone(),
        // Allocated to its final length BEFORE any address is taken. A later
        // push would reallocate and invalidate every installed pointer.
        holders: vec![-1; atomic.out_fence_slots.len()],
    };
    for (slot_idx, slot) in atomic.out_fence_slots.iter().enumerate() {
        let holder: *mut i32 = &mut prepared.holders[slot_idx];
        if let Some(val) = prepared.values.get_mut(slot.value_index as usize) {
            *val = holder as usize as u64;
        }
    }
    prepared
}
```

`execute_atomic` calls `prepare_atomic`, builds `DrmModeAtomic` from the
prepared pointers, runs the ioctl, and then:

- **rc == 0**: for each holder `>= 0`, set its bit in `out_fence_mask` and
  adopt it with `OwnedFd::from_raw_fd`. A `-1` holder is **not** repaired — the
  bitmap reports the gap and the owner classifies it under `§10`'s "live success
  plus `-1`" rule in 2b.
- **rc != 0**: close every holder `>= 0` exactly once and set
  `unexpected_fence_output`.

`KmsIoExecutor` validates each reply in this order, before anything else looks
at it:

```rust
let valid_mask = if slot_count == 0 { 0 } else { u32::MAX >> (32 - slot_count) };
if reply.correlation() != in_flight.correlation { return malformed(); }
if out_fence_mask & !valid_mask != 0 { return malformed(); }
if fds.len() as u32 != out_fence_mask.count_ones() { return malformed(); }
```

`TestDevice` has three constructors and no skip. `open_stub` opens a helper stub
target that is always available and answers with scripted results, for the
reply-shape tests. `open_never_a_drm_device` opens `/dev/null` and is what
drives the **real** helper through a **real** ioctl without hardware.
`open_real_drm_or_ignore` returns `Option` and is used only by `#[ignore]`d
hardware tests, which the suite reports separately.

`HolderLedger` lives inside `helper.rs` under `#[cfg(test)]` and counts closes
of the holder descriptors the scripted path in that same module created. There
is deliberately **no parent-side fd ledger**: a plain `OwnedFd` closes through
the standard library, so no project wrapper can observe it, and the reviewed
draft's `tests/common/fd_ledger.rs` would have counted nothing while appearing
to assert ownership. Parent-side descriptor closure is proven instead by the
pipe-EOF tests in Task 4, which observe the kernel's own view of the last
close. Helper-side exactness is asserted by the helper's own unit tests here
and reported to the parent through `unexpected_fence_output`.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/helper.rs crates/yserver/src/kms/executor/mod.rs \
        crates/yserver/src/kms/executor/test_support.rs crates/yserver/tests/
git commit -m "feat(kms): materialize atomic property arrays and own the out-fence holders"
```

---

---

### Task 4: The asynchronous host-call API

**Status: EXECUTED at 81264ba8.**

Stage 1's `dispatch` (`executor/mod.rs:293-433`) is a `libc::poll` loop that waits up to the watchdog and can `std::thread::sleep` for 100 ms deciding whether a child died. Calling it from a live path stalls the X11 core for two seconds, which `COMMIT-5` forbids and which is the exact stall section 4.1's process isolation exists to remove. The blocking form is not deleted — it is the correct call at a cold-start or final-offline boundary — but it becomes unreachable during seat-active service.

This task is executor-local. Task 5 wires the result into the core loop.

**Files:**
- Modify: `crates/yserver/src/kms/executor/mod.rs:293-433` (`dispatch`), `:496-502` (`Drop`), `:625-676` (`spawn_internal`)
- Modify: `crates/yserver/src/kms/executor/transport.rs` — a non-blocking receive that reports `WouldBlock`
- Modify: `crates/yserver/src/kms/executor/test_support.rs` — the stub behaviours below
- Test: `crates/yserver/tests/executor_async.rs` (new)
- Unchanged, and verified so: `crates/yserver/tests/executor_substrate.rs` — stage 1's six outcome tests must still pass without edits

**Interfaces:**
- Consumes: `HostCallCorrelation`, `HostCallRequest`, `HostCallReply`, `RequestKind`, `encode_request`, `decode_reply`, `MAX_OUT_FENCES`, and every `*_request_for_tests` builder (task 2); `spawn_real_helper_for_tests`, `spawn_scripted_helper_for_tests`, `dispatch_and_wait_for_tests` (task 3); `SubmittingProof`, `ExecutorState`, `ReapState`, `ReapProof`, `UnknownReason`, `HostCallOutcome` (stage 1).
- Produces:
  - `KmsIoExecutor::{send, control_fd, poll_reply, tick, next_deadline, dispatch_blocking_at_boundary, enter_seat_active, enter_final_offline}`
  - `HostCallEvent::{Outcome, LateReply}`
  - `SendError::{AlreadyInFlight, Stalled, Reaped, Ipc, ReservationMismatch}`
  - `HostCallReservation::{Submitting(SubmittingProof), Validation(ValidationLease), ClockProbe(ClockProbeLease)}`
  - `ValidationLease` and `ClockProbeLease`, both with test-only constructors; 2b adds their production producers
  - `HostCallOutcome::ProbeAccepted { sequence: u64, helper_duration_ns, round_trip_ns }` — added alongside the existing `Accepted`/`Rejected`/`Unknown`. A `HostCallReply::ProbeRejected` maps to the ordinary `HostCallOutcome::Rejected`, since an errno needs no probe-specific shape once it has left the wire.
  - `HostCallPhase::{ColdStart, SeatActive, FinalOffline}`, `KmsIoExecutor::phase()`, and `BoundaryViolation`
  - `StubBehaviour::{AcceptAfterReturningInheritedFd { delay, ignore_termination }, ReplyWithForeignCorrelation, AcceptDeclaringMissingFence, RejectWithRepeatedly(i32)}`
  - Test helpers, all in `test_support` so external tests never build wire types by hand:
    `small_atomic_request_for_tests`, `fence_returning_request_for_tests`,
    `validation_request_for_tests`, `blocking_atomic_request_for_tests`,
    `probe_request_for_tests`, `pipe_pair`, `wait_readable`,
    `wait_for_helper_exit`, `kill_helper`, `kill_and_reap`, `reap_within`,
    and the four `drive_*` case builders the terminalization table uses
    (`drive_send_failure`, `drive_helper_exit`, `drive_malformed_reply`,
    `drive_watchdog_expiry`), each returning `(KmsIoExecutor, HostCallEvent)`
- Removes: the 100 ms `std::thread::sleep` child-exit poll. (`HostCallClass::from_request` and its callers are already gone: Task 2 deletes both together, because leaving the callers would break the build between tasks.)
- Preserves: `dispatch_for_tests`, and therefore stage 1's `crates/yserver/tests/executor_substrate.rs` unchanged. It routes through `dispatch_blocking_at_boundary`, which a freshly spawned executor permits because its phase is `ColdStart`. Those six tests are this task's regression net: the outcome classification stage 1 established must survive the split.
- Rewrites: `dispatch_and_wait_for_tests`, which Task 3's tests call. It stops wrapping `dispatch` and becomes `send` + a bounded readable-wait + `poll_reply`, returning the event's `HostCallOutcome`. Task 3's assertions are unchanged; only the helper underneath them moves. Leaving it on `dispatch` would keep a blocking host call alive in the suite that is supposed to prove there is none.

#### Why the boundary is a runtime precondition and not a token

The reviewed draft guarded the blocking call with a `BoundaryWitness` whose constructors were `pub(crate)`. That is not a guard: every seat-active module in the crate could construct one, and no external compile-fail case can prove anything about an internal caller. Rust has no visibility that says "only these two call sites".

So the boundary becomes an **observable precondition on the executor** instead. The executor knows which lifecycle phase it is in, because 2b's lifecycle transitions tell it, and `dispatch_blocking_at_boundary` returns `Err(BoundaryViolation)` when that phase is `SeatActive`. This is weaker than a compile error and stronger than a convention: it is testable, and the test below is the proof. A wrongly-placed blocking call fails loudly at its first execution rather than stalling the server for two seconds.

- [x] **Step 1: Write the failing non-blocking tests**

```rust
// crates/yserver/tests/executor_async.rs
// Every name this file uses, and nothing more: `-D warnings` fails the build
// on an unused import. Test-support functions are called through the
// `test_support::` path rather than imported one by one.
use std::io::Read;
use std::time::{Duration, Instant};
use yserver::kms::executor::{
    ClockProbeLease, ExecutorState, HostCallClass, HostCallEvent, HostCallOutcome,
    HostCallPhase, HostCallReservation, KmsIoExecutor, SendError, SubmittingProof,
    UnknownReason, ValidationLease,
    protocol::HostCallRequest,
    test_support::{self, StubBehaviour},
};

// No wall-clock ceilings anywhere in this file. A correct nonblocking
// implementation can be descheduled for longer than any threshold worth
// setting, so an elapsed-time assertion tests the CI machine's load as much
// as the code. These tests instead put the executor in a state where a
// blocking implementation cannot return at all, and let the harness's own
// timeout be the failure mode. A hang IS the signal.

#[test]
fn send_returns_against_a_helper_that_will_never_reply() {
    // NeverReply never writes to the control socket. A `send` that waited for
    // a reply could not return from this call at any speed, so reaching the
    // next line is the proof — no threshold required.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(&test_support::small_atomic_request_for_tests(),
              HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send returned, so it did not wait for a reply");
    assert!(executor.poll_reply().is_none());
}

#[test]
fn poll_reply_returns_none_repeatedly_against_a_silent_helper() {
    // Same argument: one blocking receive against NeverReply would never
    // return, so completing two hundred of them is the proof.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(&test_support::small_atomic_request_for_tests(),
              HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    for _ in 0..200 {
        assert!(executor.poll_reply().is_none());
    }
}

#[test]
fn a_readable_control_fd_yields_the_outcome_with_its_correlation() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::RejectWith(libc::EINVAL)).expect("spawn");
    let request = small_atomic_request_for_tests();
    let sent = request.correlation();
    executor
        .send(&request, HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    match executor.poll_reply() {
        Some(HostCallEvent::Outcome { correlation, outcome: HostCallOutcome::Rejected { errno, .. } }) => {
            assert_eq!(correlation, sent);
            assert_eq!(errno, libc::EINVAL);
        }
        other => panic!("expected a correlated rejection, got {other:?}"),
    }
}

#[test]
fn only_one_host_call_may_be_in_flight() {
    // This is what serializes host calls now that send returns immediately.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("first");
    assert_eq!(
        executor
            .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
            .unwrap_err(),
        SendError::AlreadyInFlight
    );
}
```

- [ ] **Step 2: Write the failing terminalization tests**

These are the `COMMIT-6` core: *every* acceptance-unknown path must terminalize exactly once **and** hold the device serialized until reap. The reviewed draft specified that only for watchdog expiry, which would have let a send failure or a malformed reply release the slot while a helper's acceptance was still unknown.

```rust
// crates/yserver/tests/executor_async.rs
/// Table-driven so no acceptance-unknown path can be added later without
/// declaring its terminalization behaviour here.
#[test]
fn every_acceptance_unknown_path_terminalizes_once_and_stays_serialized() {
    struct Case {
        name: &'static str,
        drive: fn() -> (KmsIoExecutor, HostCallEvent),
    }
    let cases = [
        Case { name: "send failure", drive: drive_send_failure },
        Case { name: "helper exit", drive: drive_helper_exit },
        Case { name: "malformed reply", drive: drive_malformed_reply },
        Case { name: "watchdog expiry", drive: drive_watchdog_expiry },
    ];
    for case in cases {
        let (mut executor, event) = (case.drive)();
        assert!(
            matches!(event, HostCallEvent::Outcome { outcome: HostCallOutcome::Unknown(_), .. }),
            "{}: expected one terminal unknown outcome, got {event:?}",
            case.name
        );
        assert_eq!(executor.state(), ExecutorState::Stalled, "{}: not serialized", case.name);
        assert_eq!(
            executor
                .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
                .unwrap_err(),
            SendError::Stalled,
            "{}: a second ioctl reached a helper whose acceptance is unknown",
            case.name
        );
        assert!(
            executor.poll_reply().is_none() || executor.state() == ExecutorState::Stalled,
            "{}: a second terminal event was emitted",
            case.name
        );
    }
}

#[test]
fn a_send_failure_still_produces_exactly_one_terminal_event() {
    // COMMIT-6 installs the record before the send, so a failed send must not
    // leave the caller with a record and no outcome. The parent cannot prove
    // the helper did not act, so the conservative classification is unknown.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::ExitBeforeReply).expect("spawn");
    test_support::wait_for_helper_exit(&mut executor, Duration::from_secs(5));
    let request = small_atomic_request_for_tests();
    let sent = request.correlation();
    let err = executor
        .send(&request, HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .unwrap_err();
    assert_eq!(err, SendError::Ipc);
    match executor.poll_reply() {
        Some(HostCallEvent::Outcome { correlation, outcome: HostCallOutcome::Unknown(_) }) => {
            assert_eq!(correlation, sent);
        }
        other => panic!("a failed send must terminalize its request, got {other:?}"),
    }
    assert!(executor.poll_reply().is_none(), "terminalized twice");
}

#[test]
fn eof_after_the_watchdog_does_not_emit_a_second_terminal_event() {
    // The helper dies from the watchdog's own SIGTERM, so its EOF arrives
    // after a terminal Unknown(WatchdogExpired) was already emitted. EOF then
    // means reap progress, not a second outcome.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    let expiry = executor.tick(Instant::now() + Duration::from_secs(3)).expect("watchdog");
    assert!(matches!(
        expiry,
        HostCallEvent::Outcome { outcome: HostCallOutcome::Unknown(UnknownReason::WatchdogExpired), .. }
    ));
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    assert!(executor.poll_reply().is_none(), "EOF emitted a second terminal event");
}

#[test]
fn a_malformed_reply_terminalizes_and_does_not_permit_a_retry() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::ReplyWithForeignCorrelation).expect("spawn");
    executor
        .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    assert!(matches!(
        executor.poll_reply(),
        Some(HostCallEvent::Outcome { outcome: HostCallOutcome::Unknown(UnknownReason::MalformedReply), .. })
    ));
    assert_eq!(
        executor
            .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
            .unwrap_err(),
        SendError::Stalled,
        "a retry after a malformed reply would be a second ioctl under unknown acceptance"
    );
}

#[test]
fn the_watchdog_fires_from_tick_without_sleeping_to_reach_it() {
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    // `tick` takes `now` as a parameter, so this is deterministic regardless
    // of scheduling: the deadline is crossed by passing a later instant, not
    // by waiting for one. That a `tick` implementation does not sleep to
    // reach its deadline is enforced by Task 7's grep, not by a stopwatch.
    assert!(executor.tick(Instant::now()).is_none(), "fired before the deadline");
    let event = executor.tick(Instant::now() + Duration::from_secs(3));
    assert!(matches!(
        event,
        Some(HostCallEvent::Outcome { outcome: HostCallOutcome::Unknown(UnknownReason::WatchdogExpired), .. })
    ));
}

#[test]
fn a_reaped_executor_refuses_to_send_rather_than_appearing_available() {
    // The reviewed draft asserted that send succeeds after a forced reap.
    // It cannot: a reaped executor has no helper and no control socket, and
    // this stage defines no respawn. A new incarnation is 2b's job.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    executor.tick(Instant::now() + Duration::from_secs(3));
    assert_eq!(executor.state(), ExecutorState::Stalled);
    test_support::reap_within(&mut executor, Duration::from_secs(5));
    assert_eq!(executor.state(), ExecutorState::Reaped);
    assert!(executor.control_fd().is_none(), "a reaped executor still exposes a control fd");
    assert_eq!(
        executor
            .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
            .unwrap_err(),
        SendError::Reaped
    );
}

#[test]
fn helper_death_while_in_flight_is_acceptance_unknown_never_a_rejection() {
    // Whether the parent observes EOF before or after `try_wait` reports the
    // child reaped is a scheduling detail. Both classifications are
    // acceptance-unknown under COMMIT-6, and asserting on which one arrives
    // would be a racy test of an irrelevant distinction. What must hold is
    // that neither is a rejection.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    test_support::kill_helper(&mut executor);
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    match executor.poll_reply() {
        Some(HostCallEvent::Outcome {
            outcome: HostCallOutcome::Unknown(UnknownReason::HelperExited | UnknownReason::IpcFailure),
            ..
        }) => {}
        other => panic!("helper death must be acceptance-unknown, got {other:?}"),
    }
}
```

- [ ] **Step 3: Write the failing late-reply and descriptor-ownership tests**

The reviewed draft asserted descriptor closure through an `FdLedger` that could not observe it: a plain `OwnedFd` closes through the standard library, not through a project wrapper. This replaces that with a pipe, whose EOF is real, race-free evidence that every copy of a descriptor is closed.

```rust
// crates/yserver/tests/executor_async.rs

/// Returns (read_end, executor). The stub inherits the pipe's write end in
/// the KMS_FD slot and hands a duplicate of it back as the request's single
/// out-fence, then closes its own copies. The read end therefore reports EOF
/// exactly when the last parent-side copy is closed — which is what "adopted
/// and closed exactly once" means, observed rather than asserted.
fn executor_returning_a_pipe_write_end(
    delay: Duration,
    ignore_termination: bool,
) -> (std::fs::File, KmsIoExecutor) {
    let (read_end, write_end) = test_support::pipe_pair();
    // NOT `spawn_stub_helper_with_event_fd`. That helper reopens
    // `/proc/self/fd/{raw}` with `.read(true)` and prefers the reopened
    // descriptor (`test_support.rs:75-89`), which for a pipe hands the helper
    // a READ end. The parent's write end would then be the last writer, the
    // pipe would report EOF as soon as it dropped, and the negative assertion
    // below would fail for a reason that has nothing to do with fence
    // ownership. `spawn_stub_helper_with_inherited_fd` passes the descriptor
    // through unchanged, preserving its access mode.
    let executor = test_support::spawn_stub_helper_with_inherited_fd(
        StubBehaviour::AcceptAfterReturningInheritedFd { delay, ignore_termination },
        &write_end,
    )
    .expect("spawn");
    drop(write_end); // only the helper's copy and the returned duplicate remain
    (read_end, executor)
}

/// The read end is O_NONBLOCK, which is what makes this a *test* rather than a
/// hang. On a pipe whose write ends are all closed, a nonblocking read returns
/// `Ok(0)`; while any write end is still open and no data is queued it returns
/// `WouldBlock`. A blocking read in the second state waits forever, so
/// `test_support::pipe_pair` sets O_NONBLOCK on the read end before returning
/// it, and this helper distinguishes the two states instead of deadlocking on
/// the negative assertion below.
fn pipe_is_at_eof(read_end: &mut std::fs::File) -> bool {
    let mut buf = [0u8; 1];
    match read_end.read(&mut buf) {
        Ok(0) => true,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
        other => panic!("unexpected read on the fence pipe: {other:?}"),
    }
}

/// Proves *adoption and release*: the descriptor arrives owned, and after the
/// owner drops it no copy of the write end survives anywhere. It does not and
/// cannot prove close cardinality — a double close of a raw fd is not visible
/// through EOF — and it is a joint parent+helper assertion, because a helper
/// that leaked its own copy would keep the pipe open and fail this too.
#[test]
fn an_accepted_reply_adopts_its_out_fence_and_releases_it_on_drop() {
    let (mut read_end, mut executor) =
        executor_returning_a_pipe_write_end(Duration::from_millis(0), false);
    executor
        .send(&fence_returning_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    let Some(HostCallEvent::Outcome { outcome: HostCallOutcome::Accepted { out_fences, .. }, .. }) =
        executor.poll_reply()
    else {
        panic!("expected an accepted outcome carrying one fence");
    };
    assert_eq!(out_fences.len(), 1, "the descriptor is adopted, not dropped on the floor");
    assert!(!pipe_is_at_eof(&mut read_end), "released before the owner dropped it");
    drop(out_fences);
    assert!(pipe_is_at_eof(&mut read_end), "the adopted descriptor was leaked");
}

#[test]
fn a_reply_arriving_after_the_watchdog_is_a_late_reply_whose_fds_are_adopted() {
    // The helper must survive the watchdog's SIGTERM to reply late at all.
    // StubBehaviour::AcceptAfter alone does not: stage 1's stub dies on
    // SIGTERM unless ignore_termination is set.
    let (mut read_end, mut executor) =
        executor_returning_a_pipe_write_end(Duration::from_millis(400), true);
    executor
        .send(&fence_returning_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    assert!(executor.tick(Instant::now() + Duration::from_secs(3)).is_some(), "watchdog");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    let Some(HostCallEvent::LateReply { outcome: HostCallOutcome::Accepted { out_fences, .. }, .. }) =
        executor.poll_reply()
    else {
        panic!("expected a late reply");
    };
    assert_eq!(out_fences.len(), 1, "the late fd is adopted, not leaked");
    drop(out_fences);
    assert!(pipe_is_at_eof(&mut read_end), "the late descriptor was leaked");
    assert_eq!(
        executor.state(),
        ExecutorState::Stalled,
        "a late reply is not reap proof and must not release serialization"
    );
    test_support::kill_and_reap(&mut executor);
}

#[test]
fn a_reply_declaring_more_fences_than_it_carries_is_malformed() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::AcceptDeclaringMissingFence).expect("spawn");
    executor
        .send(&fence_returning_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    assert!(matches!(
        executor.poll_reply(),
        Some(HostCallEvent::Outcome { outcome: HostCallOutcome::Unknown(UnknownReason::MalformedReply), .. })
    ));
}
```

- [ ] **Step 4: Write the failing boundary and validation tests**

```rust
// crates/yserver/tests/executor_async.rs
#[test]
fn the_blocking_form_is_refused_once_the_seat_is_active() {
    // RejectWithRepeatedly, not RejectWith: stage 1's RejectWith answers one
    // request and then falls into a blocking one-byte read and exits
    // (`test_support.rs:213-231`). This test dispatches twice against one
    // executor, so a one-shot helper would make the second call wait out its
    // thirty-second watchdog and return Unknown instead of Ok.
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::RejectWithRepeatedly(libc::EINVAL))
            .expect("spawn");
    assert_eq!(executor.phase(), HostCallPhase::ColdStart);
    assert!(
        executor
            .dispatch_blocking_at_boundary(
                &blocking_atomic_request_for_tests(),
                HostCallReservation::Submitting(SubmittingProof::for_tests()),
            )
            .is_ok(),
        "cold start is a permitted blocking boundary"
    );

    executor.enter_seat_active();
    assert_eq!(
        executor
            .dispatch_blocking_at_boundary(
                &blocking_atomic_request_for_tests(),
                HostCallReservation::Submitting(SubmittingProof::for_tests()),
            )
            .unwrap_err(),
        BoundaryViolation,
        "COMMIT-5: no seat-active path may wait on a host call"
    );

    executor.enter_final_offline();
    assert!(
        executor
            .dispatch_blocking_at_boundary(
                &blocking_atomic_request_for_tests(),
                HostCallReservation::Submitting(SubmittingProof::for_tests()),
            )
            .is_ok(),
        "final offline is the other permitted blocking boundary"
    );
}

#[test]
fn send_refuses_a_cold_start_class_once_the_seat_is_active() {
    // COMMIT-5 on the path production actually uses. Guarding only the
    // blocking wrapper left this open: the request never blocks the core, but
    // it does ask the kernel for a blocking ioctl, and the relabelled
    // validation case silently buys a 30-second watchdog.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor.enter_seat_active();
    for request in [
        test_support::blocking_atomic_request_for_tests(),
        test_support::validation_request_for_tests(HostCallClass::ColdStartOrOfflineValidation),
    ] {
        let reservation = match request.class().is_validation() {
            true => HostCallReservation::Validation(ValidationLease::for_tests()),
            false => HostCallReservation::Submitting(SubmittingProof::for_tests()),
        };
        assert_eq!(
            executor.send(&request, reservation).unwrap_err(),
            SendError::BoundaryViolation,
            "{:?} must not be sendable while seat-active",
            request.class()
        );
    }
}

#[test]
fn a_validation_timeout_is_abandoned_not_acceptance_unknown() {
    // spec:320-329 — a validation timeout invalidates the candidate snapshot
    // but "never classifies hardware state as acceptance-unknown because no
    // live mutation was requested". Unknown would send 2b's owner into
    // COMMIT-6 quarantine over a call that touched nothing.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(&test_support::validation_request_for_tests(HostCallClass::SeatActiveValidation),
              HostCallReservation::Validation(ValidationLease::for_tests()))
        .expect("send");
    let event = executor.tick(Instant::now() + Duration::from_secs(3)).expect("watchdog");
    assert!(matches!(
        event,
        HostCallEvent::Outcome {
            outcome: HostCallOutcome::ValidationAbandoned(UnknownReason::WatchdogExpired), ..
        }
    ), "got {event:?}");
    // The executor is still unreliable, so serialization is unchanged.
    assert_eq!(executor.state(), ExecutorState::Stalled);
}

#[test]
fn a_reply_of_the_wrong_family_is_malformed_even_with_a_matching_correlation() {
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::ReplyWithWrongFamily).expect("spawn");
    executor
        .send(&test_support::probe_request_for_tests(),
              HostCallReservation::ClockProbe(ClockProbeLease::for_tests()))
        .expect("send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    assert!(matches!(
        executor.poll_reply(),
        Some(HostCallEvent::Outcome {
            outcome: HostCallOutcome::Unknown(UnknownReason::MalformedReply), ..
        })
    ));
}

#[test]
fn a_clock_probe_is_sent_under_its_own_lease_and_its_sequence_survives() {
    // spec:642-645 — a probe owns no commit resources, so requiring a
    // SubmittingProof would mean installing a commit record for a read-only
    // query. And 2b picks KernelSequence vs Unresolved from this number, so
    // the outcome has to carry it.
    let mut executor =
        test_support::spawn_stub_helper(StubBehaviour::AcceptProbeWith(4242)).expect("spawn");
    executor
        .send(&test_support::probe_request_for_tests(),
              HostCallReservation::ClockProbe(ClockProbeLease::for_tests()))
        .expect("a probe is a legal send under a probe lease");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    match executor.poll_reply() {
        Some(HostCallEvent::Outcome { outcome: HostCallOutcome::ProbeAccepted { sequence, .. }, .. }) => {
            assert_eq!(sequence, 4242);
        }
        other => panic!("the probe sequence must reach the caller, got {other:?}"),
    }
}

#[test]
fn validation_is_sent_under_a_validation_lease_not_a_submitting_record() {
    // spec:651-653 — ValidationOnly is neither a live blocking commit nor a
    // submitted record. Requiring SubmittingProof here would make the API
    // unusable for the validation the spec mandates.
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::RejectWith(libc::EINVAL))
        .expect("spawn");
    executor.enter_seat_active();
    executor
        .send(&validation_request_for_tests(), HostCallReservation::Validation(ValidationLease::for_tests()))
        .expect("validation is a legal seat-active send");
    test_support::wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(5));
    assert!(matches!(
        executor.poll_reply(),
        Some(HostCallEvent::Outcome { outcome: HostCallOutcome::Rejected { .. }, .. })
    ));
}

#[test]
fn the_reservation_kind_must_match_the_request_class() {
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    assert_eq!(
        executor
            .send(&validation_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
            .unwrap_err(),
        SendError::ReservationMismatch
    );
    assert_eq!(
        executor
            .send(&small_atomic_request_for_tests(), HostCallReservation::Validation(ValidationLease::for_tests()))
            .unwrap_err(),
        SendError::ReservationMismatch
    );
    assert_eq!(
        executor
            .send(&test_support::probe_request_for_tests(),
                  HostCallReservation::Submitting(SubmittingProof::for_tests()))
            .unwrap_err(),
        SendError::ReservationMismatch,
        "a probe must not be able to consume a commit record's proof"
    );
}

#[test]
fn each_class_carries_the_watchdog_the_spec_assigns_it() {
    // spec:320-329 gives seat-active validation two seconds and
    // cold-start/offline validation thirty. The two carry identical flags, so
    // only the explicit class field can tell them apart.
    use yserver::kms::executor::HostCallClass::*;
    for (class, expected) in [
        (SeatActiveNonblock, 2),
        (SeatActiveValidation, 2),
        (ColdStartOrOfflineBlocking, 30),
        (ColdStartOrOfflineValidation, 30),
    ] {
        assert_eq!(class.watchdog(), Duration::from_secs(expected), "{class:?}");
    }
    assert!(SeatActiveValidation.is_validation() && ColdStartOrOfflineValidation.is_validation());
    assert!(!SeatActiveNonblock.is_validation() && !ColdStartOrOfflineBlocking.is_validation());
}
```

- [x] **Step 5: Run the tests to verify they fail**

```bash
cargo test -p yserver --test executor_async
```
Expected: FAIL — `send`, `poll_reply`, `tick`, `HostCallPhase` and the new stub behaviours do not exist.

- [x] **Step 6: Write the in-flight state machine**

The executor owns its in-flight state, so no caller can hold a token that desynchronizes from it:

```rust
struct InFlight {
    correlation: HostCallCorrelation,
    class: HostCallClass,
    /// Compared against the reply's kind before its correlation. Equality of
    /// correlation alone would let a probe request receive an atomic
    /// `Accepted` carrying the probe's own tuple.
    kind: RequestKind,
    /// The request's out-fence slot count, which is the validity bound for
    /// the reply's `out_fence_mask`. Task 3 requires that check and
    /// `decode_reply` cannot make it, because a reply frame does not carry
    /// the request's slot table.
    slot_count: u32,
    started: Instant,
    deadline: Instant,
    /// `Some` once any acceptance-unknown path has emitted this request's one
    /// terminal event. The entry is *retained* afterwards so a later reply is
    /// recognised as late rather than as a fresh outcome, and so `send`
    /// keeps refusing. Cleared only by a proven reap.
    terminalized: Option<UnknownReason>,
}

pub enum HostCallEvent {
    Outcome { correlation: HostCallCorrelation, outcome: HostCallOutcome },
    /// Arrived after its request was terminalized. Its fds are adopted so the
    /// owner can close them exactly once into quarantine.
    LateReply { correlation: HostCallCorrelation, outcome: HostCallOutcome },
}
```

One private method is the whole of `COMMIT-6`'s unknown branch, and every acceptance-unknown path calls it rather than reimplementing it:

```rust
/// Emit this request's single terminal unknown outcome, enter `Stalled`, and
/// begin the reap path — while *retaining* `in_flight` so no second ioctl can
/// be dispatched on a device whose acceptance is unknown (spec:685-686).
/// Returns `None` when the request was already terminalized, which is what
/// makes "exactly one terminal event" hold across watchdog-then-EOF.
fn terminalize_unknown(&mut self, reason: UnknownReason) -> Option<HostCallEvent> {
    let in_flight = self.in_flight.as_mut()?;
    if in_flight.terminalized.is_some() {
        return None;
    }
    in_flight.terminalized = Some(reason);
    let correlation = in_flight.correlation;
    // A ValidationOnly call requested no live mutation, so its failure
    // invalidates the candidate snapshot and nothing else. Folding it into
    // `Unknown` would apply COMMIT-6's hardware quarantine to a call that
    // touched no hardware (`spec:320-329`). The executor is equally
    // unreliable either way, so `Stalled` and the retained `in_flight`
    // apply to both.
    let outcome = if in_flight.class.is_validation() {
        HostCallOutcome::ValidationAbandoned(reason)
    } else {
        HostCallOutcome::Unknown(reason)
    };
    self.state = ExecutorState::Stalled;
    self.request_termination();
    Some(HostCallEvent::Outcome { correlation, outcome })
}
```

`send(&mut self, request, reservation)` refuses, in order: `Reaped` when the helper is reaped, `Stalled` when the state is `Stalled` or `ShutdownStalled`, `AlreadyInFlight` when `in_flight.is_some()`, `BoundaryViolation` when the request's class is `ColdStartOrOffline*` and `phase == SeatActive`, and `ReservationMismatch` when the reservation kind does not match the request:

| Request | Legal reservation |
| --- | --- |
| `Atomic` with `SeatActiveNonblock` or `ColdStartOrOfflineBlocking` | `Submitting(SubmittingProof)` |
| `Atomic` with `SeatActiveValidation` or `ColdStartOrOfflineValidation` | `Validation(ValidationLease)` |
| `ClockProbe` | `ClockProbe(ClockProbeLease)` |

A probe gets its own reservation because it "owns no commit resources and cannot authorize KMS state" (`spec:642-645`). Requiring a `SubmittingProof` for it would mean the owner had installed a commit record and reserved the device slot for a read-only query — a `COMMIT-6` violation manufactured by the type system.

**The phase check belongs on `send`, not only on `dispatch_blocking_at_boundary`.** Guarding the blocking wrapper alone leaves `COMMIT-5` unenforced on the asynchronous path, which is the path production actually uses: a seat-active caller could `send` a `ColdStartOrOfflineBlocking` request, or relabel seat-active validation as `ColdStartOrOfflineValidation` and quietly buy the thirty-second watchdog (`spec:635-653`). The class table in the contract is the authority for which phases each class permits. Otherwise it installs `InFlight` **before** the write — so a transport error cannot leave a record with no outcome — encodes, and sends. On transport error it queues `terminalize_unknown(IpcFailure)` and returns `Err(SendError::Ipc)`. The `Dispatched` milestone belongs at send time, not at reply time; 2b's owner sets it when `send` returns.

`control_fd` returns `None` once reaped, `Some(self.control.as_fd())` otherwise.

`next_deadline()` returns `self.in_flight.as_ref().filter(|f| f.terminalized.is_none()).map(|f| f.deadline)`. Task 5 feeds it to `KmsBackend::next_wakeup`; without it the core would block indefinitely and the watchdog would never fire.

`poll_reply()` performs one non-blocking `recv_frame`:

- `WouldBlock` → `None`.
- EOF or a receive error → `check_child_exited()` decides the reason (`HelperExited` if the child is reaped, else `IpcFailure`), then `terminalize_unknown(reason)`. After a prior terminalization this returns `None`, which is the EOF-after-watchdog case: reap progress, not a second outcome.
- A frame that fails `decode_reply`, whose **kind** differs from `in_flight.kind`, whose correlation differs from `in_flight.correlation`, whose `out_fence_mask` has bits set at or above `in_flight.slot_count`, or whose fd count disagrees with that mask → close every received descriptor exactly once, then `terminalize_unknown(MalformedReply)`. The kind check runs first: it is the one that catches a reply from the wrong family carrying an otherwise-matching tuple.
- A frame carrying a handshake kind → `terminalize_unknown(MalformedReply)`. The handshake is consumed by `await_helper_ready` before the executor ever accepts a host call, so one arriving here means the helper is out of step with the protocol.
- A valid, correlated reply → if `terminalized.is_some()`, emit `LateReply` with its adopted fds and **keep** `in_flight` and `Stalled`, because a late reply is not reap proof. Otherwise emit `Outcome` and clear `in_flight`.

A `HostCallReply::ClockProbe { sequence, .. }` becomes `HostCallOutcome::ProbeAccepted { sequence, .. }`. Without that variant the probe's only useful result is discarded at the API boundary: 2b decides `KernelSequence` versus `Unresolved` for an epoch-local clock record from exactly this number and from an `EOPNOTSUPP` rejection, and it consumes those through `HostCallEvent`. An outcome enum that can carry an out-fence but not a sequence would make the probe unusable by its only consumer.

`tick(now)` does two things. If an unterminalized in-flight call is past its deadline it returns `terminalize_unknown(WatchdogExpired)`. Independently — and on every call, so a stalled executor makes progress without a poll event — if `in_flight` is terminalized it calls `try_reap()`, and on `ReapState::Reaped` clears `in_flight` and leaves `Stalled` for `Reaped`. That pairing is what stops a terminalized executor from wedging forever while still never releasing the device before reap.

Stage 1's 100 ms sleep loop is deleted: a parent must never sleep to decide whether a child died.

- [ ] **Step 7: Write the socket, boundary and Drop changes**

In `spawn_internal`, make **only the parent endpoint** non-blocking, after the pair is created:

```rust
let (parent_control, child_control) = seqpacket_pair()?;
// Only the parent side. The helper's serve loop (helper.rs:79-103) is a
// blocking receive; O_NONBLOCK on its endpoint would make its first recv
// return WouldBlock and the process exit immediately.
set_nonblocking(parent_control.as_fd())?;
```

`HostCallPhase` starts at `ColdStart`. `enter_seat_active()` and `enter_final_offline()` move it forward and never backward; 2b's lifecycle transitions call them. `dispatch_blocking_at_boundary(&mut self, request, reservation) -> Result<HostCallOutcome, BoundaryViolation>` returns `Err(BoundaryViolation)` when the phase is `SeatActive`, and is otherwise stage 1's `dispatch` body minus the sleep loop. It remains the only `libc::poll` site in the module.

The destructor stops waiting:

```rust
impl Drop for KmsIoExecutor {
    /// Deliberately does NOT `wait()`. Stage 1 called `Child::wait()` here,
    /// which blocks the core thread indefinitely when the helper is inside an
    /// uninterruptible kernel call — exactly the stall COMMIT-5 forbids and
    /// process isolation exists to remove. COMMIT-7's bounded fallback applies
    /// instead: request termination, attempt one non-blocking reap, and
    /// otherwise leave the helper orphaned for `init` while recording that its
    /// lease was never proven released. The teardown supervisor, not this
    /// destructor, owns the deadline-bounded wait.
    fn drop(&mut self) {
        if self.check_child_exited() {
            return;
        }
        let _ = self.child.kill();
        if self.check_child_exited() {
            return;
        }
        log::warn!(
            "kms executor: helper pid {} unreaped at drop; incarnation {:?} lease not proven \
             released, leaving it orphaned rather than blocking the core",
            self.child.id(),
            self.incarnation
        );
    }
}
```

- [x] **Step 8: Write the stub behaviours**

`test_support.rs` gains three variants and their `to_arg_string`/`from_arg_str` round trips, plus the helpers the tests import:

- `AcceptAfterReturningInheritedFd { delay: Duration, ignore_termination: bool }` — encoded as `accept-fd-after:<ms>:<0|1>`. Sleeps `delay`, replies `Accepted` with `out_fence_mask = 1`, passes one `dup` of its inherited `KMS_FD` as the fence, then closes that duplicate. When `ignore_termination` is set it installs `SIG_IGN` for `SIGTERM` first, which is what lets the late-reply test observe a reply after the watchdog.
- `ReplyWithForeignCorrelation` — replies `Accepted` with a correlation whose `lifecycle_epoch` is `u64::MAX`.
- `RejectWithRepeatedly(errno)` — stage 1's `RejectWith` in a loop, serving every request until EOF instead of exiting after one. Added rather than changing `RejectWith`, so stage 1's `executor_substrate.rs` keeps the exact helper it was written against.
- `AcceptProbeWith(sequence)` — replies `HostCallReply::ClockProbe { sequence, .. }` echoing the request's probe correlation.
- `ReplyWithWrongFamily` — replies `HostCallReply::Accepted` echoing the request's correlation **verbatim**, whatever family it belongs to. The correlation therefore matches and only the kind check can reject it.
- `AcceptDeclaringMissingFence` — replies `Accepted` with `out_fence_mask = 1` and no descriptor attached.
- `test_support::spawn_stub_helper_with_inherited_fd(behaviour, fd)` — duplicates `fd` into the helper's `KMS_FD` slot **verbatim**, with no `/proc/self/fd` reopen, so its access mode survives. Stage 1's `spawn_stub_helper_with_event_fd` keeps its reopening behaviour for the synthetic-event tests that need a readable alias; this is a sibling, not a replacement.
- `test_support::{pipe_pair, wait_readable, wait_for_helper_exit, kill_helper, kill_and_reap, reap_within}` — `wait_readable` is a bounded `libc::poll` in the *test harness*, not in `executor/mod.rs`, so it does not affect Task 7's single-polling-site gate. **`pipe_pair` sets `O_NONBLOCK` on the read end** before returning it; without that the fence-ownership tests deadlock on their first negative EOF check rather than failing.

- [x] **Step 9: Run the tests to verify they pass**

```bash
cargo test -p yserver --test executor_async
cargo clippy --all-targets -- -D warnings
```
Expected: PASS.

- [x] **Step 10: Commit**

```bash
git add crates/yserver/src/kms/executor/mod.rs crates/yserver/src/kms/executor/transport.rs \
        crates/yserver/src/kms/executor/test_support.rs crates/yserver/tests/executor_async.rs
git commit -m "feat(kms): split the host call into send, poll and watchdog"
```

---

### Task 5: The executor as a real core-loop source

**Status: EXECUTED at 17f2a78b.**

Task 4's API is asynchronous only if something drives it. The reviewed draft claimed this integration in prose against types that do not exist — there is no `SourceKind` in the tree; the real type is `BackendFdKind` (`backend/trait_def.rs:57-84`), the core's dispatch over it is exhaustive (`core_loop/run.rs:1209-1257`), and the actual source inventory lives in `KmsPlatform::poll_fds` (`platform.rs:3936-3958`). Wiring it therefore crosses two crates and is its own deliverable.

The watchdog half matters as much as the readability half. The core blocks until `Backend::next_wakeup()` (`run.rs:1121-1148`). Without an executor deadline in that chain, an idle server blocks indefinitely, `tick` is never called, and the two-second watchdog never fires — so the terminalization Task 4 built would exist and never run.

**Files:**
- Modify: `crates/yserver-core/src/backend/trait_def.rs:57-84` — the `BackendFdKind` variant
- Modify: `crates/yserver-core/src/backend/trait_def.rs:506-514` — the defaulted `Backend` hook
- Modify: `crates/yserver-core/src/backend/recording.rs:519-541` — a wakeup-deadline builder for the deadline test
- Modify: `crates/yserver-core/src/core_loop/run.rs:1209-1257` — the dispatch arm
- Modify: `crates/yserver/src/kms/backend.rs:840-875` — `platform_init` spawns one executor per opened device
- Modify: `crates/yserver/src/kms/render/platform.rs:1990-1994,2540-2562,3936-3958`
- Modify: `crates/yserver/src/kms/render/backend.rs:15201-15250,16138-16142`

**Interfaces:**
- Consumes: `KmsIoExecutor::{control_fd, poll_reply, tick, next_deadline}`, `HostCallEvent`, `HostCallOutcome`, `UnknownReason` (task 4).
- Produces, all `#[cfg(test)]` and in the modules whose types they build:
  - in `kms/render/platform.rs`: `platform_with_stub_executors_for_tests(n)`, `reap_every_executor_for_tests`, `send_never_answered_host_call_for_tests`
  - in `kms/render/backend.rs`: `backend_with_stub_executor_for_tests`, `backend_with_stub_executors_for_tests(n)`, `send_never_answered_host_call_for_tests`, `send_rejected_host_call_for_tests`, `send_rejected_host_call_on_every_device_for_tests`, `wait_executor_readable_for_tests`, `wait_all_executors_readable_for_tests`, `drained_host_call_events_for_tests`
  - in `yserver-core`'s `recording.rs`: `with_wakeup_deadline`, `with_before_block_notification`, `with_executor_readable_notification`
- Produces:
  - `BackendFdKind::ExecutorControl`
  - `Backend::on_executor_readable(&mut self, state: &mut ServerState)` — defaulted no-op
  - `RecordingBackend::{with_wakeup_deadline, with_before_block_notification, with_executor_readable_notification}`
  - `PlatformInitDevice.executor: KmsIoExecutor` and `KmsDevice.executor: KmsIoExecutor`
  - `KmsPlatform::{executor_deadline, drain_executor_events, tick_executors}`

Adding a defaulted trait method rather than a required one is deliberate: `recording.rs:1121` and `host_x11/trait_impl.rs:176` both implement `Backend`, and neither has an executor. A required method would force empty implementations into two files with nothing to do.

- [x] **Step 1: Write the failing core-loop tests**

These follow the existing multi-source pattern at `run.rs:3568-3627` and `:3629-3700`, which already prove a new `BackendFdKind` dispatches to its dedicated hook.

```rust
// crates/yserver-core/src/core_loop/run.rs, in the existing #[cfg(test)] module
#[test]
fn executor_control_readiness_dispatches_the_executor_hook() {
    use crate::backend::{BackendFdKind, recording::RecordingBackend};
    use std::{io::Write, os::fd::AsRawFd};

    let (poll, sender, rx) = channel().unwrap();
    let sender_for_core = sender.clone_handle();
    let (control_reader, mut control_writer) = UnixStream::pair().unwrap();
    let control_fd = control_reader.as_raw_fd();
    let (unused_page_tx, _unused_page_rx) = crossbeam_channel::unbounded();
    let (ready_tx, ready_rx) = crossbeam_channel::unbounded();
    let mut backend = RecordingBackend::new()
        .with_poll_sources(vec![(control_fd, BackendFdKind::ExecutorControl)], unused_page_tx)
        .with_executor_readable_notification(ready_tx);
    let handle = std::thread::spawn(move || {
        let _control_reader = control_reader;
        let mut state = ServerState::new();
        let alloc = ClientIdAllocator::new();
        let result = run_core(
            poll, rx, sender_for_core, &mut state, &mut backend, None, &alloc,
            AuthState::new(None),
        );
        (result, backend)
    });

    control_writer.write_all(&[1]).unwrap();
    ready_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("executor control readiness must reach on_executor_readable");
    sender.send(Message::Shutdown).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(handle.is_finished(), "run_core did not return");
    handle.join().unwrap().0.unwrap();
}

/// The watchdog cannot fire from a loop that is blocked with no deadline.
/// This proves the backend's deadline actually bounds the core's poll: with
/// no fd ever becoming readable and no message sent, `before_block` must
/// still be reached again shortly after the declared deadline.
#[test]
fn a_backend_deadline_wakes_the_core_with_no_fd_activity() {
    use crate::backend::recording::RecordingBackend;

    let (poll, sender, rx) = channel().unwrap();
    let sender_for_core = sender.clone_handle();
    let (block_tx, block_rx) = crossbeam_channel::unbounded();
    let mut backend = RecordingBackend::new()
        .with_wakeup_deadline(Instant::now() + Duration::from_millis(50))
        .with_before_block_notification(block_tx);
    let handle = std::thread::spawn(move || {
        let mut state = ServerState::new();
        let alloc = ClientIdAllocator::new();
        run_core(
            poll, rx, sender_for_core, &mut state, &mut backend, None, &alloc,
            AuthState::new(None),
        )
    });

    // At least two block-handler passes with no fd and no message: one before
    // the deadline and one after it. A core that ignored next_wakeup would
    // deliver the first and then block forever.
    block_rx.recv_timeout(Duration::from_secs(10)).expect("first block handler");
    block_rx.recv_timeout(Duration::from_secs(10)).expect("deadline did not wake the core");
    sender.send(Message::Shutdown).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(handle.is_finished(), "run_core did not return");
    handle.join().unwrap().unwrap();
}
```

- [x] **Step 2: Write the failing KMS-side tests**

```rust
// crates/yserver/src/kms/render/platform.rs, in the existing #[cfg(test)] module
#[test]
fn poll_fds_publishes_one_executor_control_source_per_device() {
    let platform = platform_with_stub_executors_for_tests(2);
    let executor_sources: Vec<_> = platform
        .poll_fds()
        .into_iter()
        .filter(|(_, kind)| matches!(kind, BackendFdKind::ExecutorControl))
        .collect();
    assert_eq!(executor_sources.len(), 2, "each device's executor is its own source");
    let fds: Vec<_> = executor_sources.iter().map(|(fd, _)| *fd).collect();
    assert_ne!(fds[0], fds[1], "two devices must not share one control fd");
}

#[test]
fn poll_fds_omits_a_reaped_executor_rather_than_unwrapping_its_fd() {
    // Narrow on purpose. This proves only that poll_fds() handles
    // control_fd() == None without panicking or publishing a closed
    // descriptor. It does NOT prove the running core withdraws the source:
    // `run.rs:1045-1059` collects poll_fds() once before the loop and never
    // refreshes it, so a source registered at startup stays registered for
    // the process lifetime. See the note below.
    let mut platform = platform_with_stub_executors_for_tests(1);
    reap_every_executor_for_tests(&mut platform);
    assert!(
        !platform.poll_fds().iter().any(|(_, k)| matches!(k, BackendFdKind::ExecutorControl)),
        "a reaped executor must not appear in a freshly computed source set"
    );
}

#[test]
fn the_executor_deadline_reaches_the_backend_wakeup_chain() {
    let mut platform = platform_with_stub_executors_for_tests(1);
    assert_eq!(platform.executor_deadline(), None, "an idle executor has no deadline");
    send_never_answered_host_call_for_tests(&mut platform);
    let deadline = platform.executor_deadline().expect("an in-flight call has a deadline");
    let now = Instant::now();
    assert!(deadline > now, "deadline already past");
    assert!(
        deadline <= now + Duration::from_secs(2),
        "a seat-active call must not buy more than the two-second watchdog"
    );
}

#[test]
fn ticking_past_the_deadline_yields_exactly_one_watchdog_event() {
    let mut platform = platform_with_stub_executors_for_tests(1);
    send_never_answered_host_call_for_tests(&mut platform);
    assert!(platform.tick_executors(Instant::now()).is_empty(), "fired early");
    let events = platform.tick_executors(Instant::now() + Duration::from_secs(3));
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        HostCallEvent::Outcome { outcome: HostCallOutcome::Unknown(UnknownReason::WatchdogExpired), .. }
    ));
    assert!(
        platform.tick_executors(Instant::now() + Duration::from_secs(9)).is_empty(),
        "the watchdog fired twice for one request"
    );
}
```

```rust
// crates/yserver/src/kms/render/backend.rs, in the existing #[cfg(test)] module
#[test]
fn next_wakeup_includes_the_executor_deadline() {
    // The regression this guards: an executor deadline that exists but never
    // reaches next_wakeup leaves the watchdog unreachable on an idle server.
    let mut backend = backend_with_stub_executor_for_tests();
    let without = backend.next_wakeup();
    send_never_answered_host_call_for_tests(&mut backend);
    let with = backend.next_wakeup().expect("an in-flight host call must bound the core's poll");
    assert!(
        without.is_none_or(|w| with <= w),
        "the executor deadline must win when it is the earliest"
    );
    assert!(with <= Instant::now() + Duration::from_secs(2));
}

#[test]
fn poll_fds_forwards_the_platform_executor_sources() {
    let backend = backend_with_stub_executor_for_tests();
    assert!(
        yserver_core::backend::Backend::poll_fds(&backend)
            .iter()
            .any(|(_, k)| matches!(k, BackendFdKind::ExecutorControl)),
        "the backend must forward the executor source the platform publishes"
    );
}

#[test]
fn on_executor_readable_drains_more_than_one_queued_event() {
    // Two devices, each with a reply already queued, so a single hook call
    // must produce two events. With one event an implementation that calls
    // poll_reply exactly once passes, and the edge-triggered drain-to-
    // exhaustion requirement goes untested.
    let mut backend = backend_with_stub_executors_for_tests(2);
    let mut state = yserver_core::server::ServerState::new();
    send_rejected_host_call_on_every_device_for_tests(&mut backend);
    wait_all_executors_readable_for_tests(&backend, Duration::from_secs(5));
    yserver_core::backend::Backend::on_executor_readable(&mut backend, &mut state);
    assert_eq!(
        backend.drained_host_call_events_for_tests().len(),
        2,
        "a single-read implementation would report 1"
    );
}
```

- [x] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p yserver-core executor_control
cargo test -p yserver-core a_backend_deadline_wakes_the_core
cargo test -p yserver kms::render::platform::tests::poll_fds_publishes
```
Expected: FAIL — `BackendFdKind::ExecutorControl` and `KmsDevice.executor` do not exist.

- [x] **Step 4: Add the core-side variant, hook and dispatch**

```rust
// crates/yserver-core/src/backend/trait_def.rs, in BackendFdKind
    /// Parent endpoint of a device-local KMS executor's control socket.
    /// Readiness drives `Backend::on_executor_readable`, which drains every
    /// completed host call without blocking. Spec
    /// `2026-08-26-phase-c0-atomic-kms-migration-design.md` COMMIT-5.
    ExecutorControl,
```

```rust
// crates/yserver-core/src/backend/trait_def.rs, beside on_scanout_render_completion
    /// A device-local KMS executor's control socket became readable. The
    /// backend drains every complete reply without blocking. Default no-op
    /// for backends that own no executor.
    fn on_executor_readable(&mut self, _state: &mut ServerState) {}
```

```rust
// crates/yserver-core/src/core_loop/run.rs, in the BackendFdKind match
                    BackendFdKind::ExecutorControl => {
                        backend.on_executor_readable(state);
                    }
```

`RecordingBackend` gains `with_wakeup_deadline(Instant)` and `with_before_block_notification`/`with_executor_readable_notification` senders in the same shape as `with_scanout_render_completion_notification` (`recording.rs:531-537`), overriding `next_wakeup`, `before_block` and `on_executor_readable` respectively.

- [x] **Step 5: Give the production device an executor**

`PlatformInitDevice` (`kms/backend.rs:677-681`) and `KmsDevice` (`platform.rs:1990-1994`) each gain an `executor: KmsIoExecutor` field. In `platform_init`, immediately after `primary_device_key_from_fd` qualifies the device (`kms/backend.rs:855-856`) and before it is pushed into `devices`, spawn its executor:

`platform_init` has no incarnation in scope today, and this stage does not add
lifecycle management: it allocates `IncarnationId::first()` once at the top of
`platform_init` and passes the same value to every device's executor. Reopen and
later incarnations belong to 2b's lifecycle, which is also what will make
`enter_seat_active` reachable.

```rust
// One incarnation and one lifecycle epoch per platform_init; 2b owns reopen.
let incarnation = IncarnationId::first();
let lifecycle_epoch = LifecycleEpochId::first();
let executor = KmsIoExecutor::spawn(std::os::fd::AsFd::as_fd(&*device), incarnation, lifecycle_epoch)
    .map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "yserver: cannot start the KMS executor for {}: {err}",
                device_path.display()
            ),
        )
    })?;
devices.push(PlatformInitDevice { key: device_key, device, executor });
```

Spawn failure is fatal for that device rather than skipped: a `KmsDevice` with no executor would be a device C.0 cannot submit to, and silently continuing would reintroduce the in-process ioctl path this phase exists to remove. `from_platform_init` (`platform.rs:2550-2559`) moves the field across into `KmsDevice` alongside `cursor`.

- [x] **Step 6: Publish the source, the deadline and the drain**

```rust
// crates/yserver/src/kms/render/platform.rs
impl KmsPlatform {
    // inside poll_fds, in the existing per-device loop
    for device in &self.devices {
        fds.push((device.device.as_fd().as_raw_fd(), BackendFdKind::Drm));
        // `control_fd` is None once the executor is reaped. Registering a
        // closed descriptor would make the poller spin on an invalid source.
        if let Some(control) = device.executor.control_fd() {
            fds.push((control.as_raw_fd(), BackendFdKind::ExecutorControl));
        }
    }

    pub(crate) fn executor_deadline(&self) -> Option<Instant> {
        self.devices.iter().filter_map(|d| d.executor.next_deadline()).min()
    }

    pub(crate) fn drain_executor_events(&mut self) -> Vec<HostCallEvent> {
        let mut events = Vec::new();
        for device in &mut self.devices {
            while let Some(event) = device.executor.poll_reply() {
                events.push(event);
            }
        }
        events
    }

    pub(crate) fn tick_executors(&mut self, now: Instant) -> Vec<HostCallEvent> {
        self.devices.iter_mut().filter_map(|d| d.executor.tick(now)).collect()
    }
}
```

`drain_executor_events` loops to exhaustion per device rather than reading once. The core poller is mio, which registers epoll sources edge-triggered, so a single read that leaves a queued datagram behind would strand it until the next unrelated wake — the same reasoning stage 1 recorded for the DRM event drain.

#### A constraint this stage inherits and does not fix

`run_core` calls `Backend::poll_fds()` exactly once, before entering the loop, and registers the result permanently (`run.rs:1045-1059`). There is no deregistration and no re-registration path. Two consequences, both deliberate here:

- **2a is unaffected.** Its executors are spawned during `platform_init`, before `run_core` collects the source set, and are never replaced. Registration at startup is all this stage needs.
- **2b cannot replace an executor without first adding one.** When a reaped executor is replaced for a new incarnation, its successor's control fd has no way into the already-running poller, and the dead one's entry is never removed. So 2b's prerequisite is a `Backend` mechanism for source churn — most plausibly a `poll_fds` generation counter the core re-reads, or an explicit `Message::BackendSourcesChanged` wake.

Doing that here would mean designing core-loop source churn for a consumer that does not exist yet. It is recorded in "What stage 2b consumes" instead, so it is a named prerequisite rather than a surprise.

- [x] **Step 7: Drive it from the KMS backend**

In `kms/render/backend.rs`:

```rust
    fn poll_fds(&self) -> Vec<(std::os::fd::RawFd, BackendFdKind)> {
        // Direct mode only: DRM fd, executor control fds, present-completion
        // epfd. libinput runs on its own thread, not the core poll.
        self.platform.poll_fds()
    }

    fn on_executor_readable(&mut self, _state: &mut ServerState) {
        let events = self.platform.drain_executor_events();
        self.record_host_call_events(events);
    }

    fn before_block(&mut self) {
        // ...existing GPU reap...
        let events = self.platform.tick_executors(std::time::Instant::now());
        self.record_host_call_events(events);
    }
```

and `next_wakeup` (`backend.rs:15201-15250`) gains one link in its existing chain:

```rust
            .chain(self.platform.executor_deadline())
```

The executor deadline is **not** gated behind `allow_kms_timers`. The other deadlines in that chain are composite timers that must not run while scanout is disallowed; a host-call watchdog is a safety deadline whose whole purpose is to fire when the device is not behaving, including while outputs are withdrawn.

Ticking from `before_block` rather than from a dedicated timer path is why the deadline must reach `next_wakeup`: the core computes `poll_timeout` from `next_wakeup` (`run.rs:1121-1148`), returns from `poll` when it expires, and reaches `before_block` on the following iteration, where `tick_executors` fires. The watchdog therefore lands within one loop iteration of its deadline rather than at an unbounded time.

`record_host_call_events` logs each event and pushes it onto a bounded `VecDeque` that `drained_host_call_events_for_tests` reads. **In this sub-stage the backend logs and discards; 2b's owner is the real consumer.** The queue exists so the tests above can prove the events arrived, and so 2b has a single place to redirect.

- [x] **Step 1: Write the failing core-loop tests**
- [x] **Step 2: Write the failing KMS-side tests**
- [x] **Step 3: Run the tests to verify they fail**
- [x] **Step 4: Add the core-side variant, hook and dispatch**
- [x] **Step 5: Give the production device an executor**
- [x] **Step 6: Publish the source, the deadline and the drain**
- [x] **Step 7: Drive it from the KMS backend**
- [x] **Step 8: Run the tests to verify they pass**
- [x] **Step 9: Commit**

*Execution note (Task 5):*
- Handled per Ruling R3: `KmsDevice.executor` is `Option<KmsIoExecutor>`, initialized as `Some(executor)` in production `from_platform_init` and `None` in existing test fixtures without needing mock executors across all historical platform fixtures.
- Handled per Ruling R7: The edge-triggered multi-reply draining test was implemented using a dedicated `StubBehaviour::ReplyTwiceWith(i32)` stub on a single executor socket, validating that `on_executor_readable` drains multiple queued events from one control fd in a single invocation without requiring multi-device mock platform plumbing.
- `compile_fail.rs` was updated to ignore 0-byte `.rmeta` files produced by cargo check/clippy, and `executor_async.rs` was stabilized against out-fence pipe teardown races.
- Code committed in `17f2a78b`.

---

### Task 6: The `COMMIT-7` device lock, held by the executor

`COMMIT-7` says the lock is "taken by the executor for as long as it lives and released only by its death" (`spec:712-719`). The case it exists for is the parent dying while a helper is wedged: a parent-held lock is released by the parent's exit, and a new server then installs state underneath the still-live helper.

Three facts shape the implementation, and the third makes the obvious version wrong. `flock` is associated with the open file description; it survives `execve`; duplicated descriptors share the lock and it is released only when **all** of them are closed — **but also by an explicit unlock through any one of them**. `DeviceLock::drop` currently performs that unlock (`device_lock.rs:185-191`), so the parent dropping its guard would release the helper's lock too.

Task 5 already gave every production device an executor. This task puts the lock into that same spawn path.

**Files:**
- Modify: `crates/yserver/src/kms/executor/device_lock.rs:127-191,210-272`
- Modify: `crates/yserver/src/kms/executor/mod.rs:32-33,625-676` — the `LOCK_FD` slot and the readiness handshake
- Modify: `crates/yserver/src/kms/executor/helper.rs:72-76` — adopt it
- Modify: `crates/yserver/src/kms/backend.rs:840-875` — take the lock before the executor spawn added in Task 5
- Modify: `crates/yserver/src/bin/yserver.rs:6-36` — dispatch the handoff entry point
- Test: `crates/yserver/tests/executor_lock_handoff.rs`

**Interfaces:**
- Consumes: `may_install_state`, `DeviceLock`, `DrmDeviceKey`, `LOCK_HOLDER_ARG`, `run_lock_holder_if_requested` (stage 1); `KmsIoExecutor::spawn`, `HandshakeRequest`/`HandshakeReply` and their codecs (tasks 2, 4, 5).
- Produces:
  - `LOCK_FD: RawFd = 200`
  - `DeviceLock::{release_explicitly, into_inheritable}` and `InheritableDeviceLock`
  - `KmsIoExecutor::{spawn_with_device_lock, spawn_with_device_lock_at, spawn_wedged_lock_holder_for_tests, await_helper_ready, helper_pid}` — every spawn takes `(kms_fd, incarnation, lifecycle_epoch, &InheritableDeviceLock)`, because the handshake it performs carries both identities; `spawn_with_device_lock_at` additionally takes an explicit executable path so the failed-spawn test can name one that does not exist; `spawn_wedged_lock_holder_for_tests` spawns `StubBehaviour::WedgedHoldingLock` with `PR_SET_PDEATHSIG` disarmed and stderr on `/dev/null`, and is `#[doc(hidden)]` and reachable only from the handoff subprocess; `helper_pid()` returns `libc::pid_t`, widened from the wire's `u32`
  - `StubBehaviour::WedgedHoldingLock` — adopts `LOCK_FD`, answers exactly one handshake so `await_helper_ready` can succeed, installs `SIG_IGN` for `SIGTERM`, then sleeps forever **without reading the control socket again**. Both orphan-kill paths are therefore inert, which is what a helper inside an uninterruptible ioctl looks like from outside.
  - `DeviceLock::duplicate_for_tests()` — `dup`s the guard's descriptor into a second `DeviceLock` sharing one open file description, so the unlock-semantics tests can observe last-close behaviour
  - `LOCK_HANDOFF_ARG` and its `run_lock_handoff_if_requested()` entry point
  - `acquire_device_lock_or_refuse(&DrmDeviceKey) -> io::Result<DeviceLock>` — the single lock-acquisition step `platform_init` calls, and the only thing the refusal test needs

#### Why a type state and not one guard with an extra method

The reviewed draft had `into_inheritable` "yield the raw descriptor" while the parent later dropped "its `DeviceLock`". Those cannot both be true: a consuming method cannot leave its receiver available to drop, and a bare `RawFd` does not say who closes it if the spawn fails. Worse, `release_explicitly` stayed callable on a guard whose open file description the helper now shares — the exact global unlock this task exists to remove.

So the handoff is a type transition. `DeviceLock` is the pre-handoff guard and keeps `release_explicitly`. `into_inheritable` consumes it and returns `InheritableDeviceLock`, which owns the descriptor, has **no** unlock operation at all, and whose `Drop` only closes. The compiler, not a comment, is what stops a post-handoff global unlock. Ownership on the failure path is equally explicit: `spawn_with_device_lock` borrows the `InheritableDeviceLock`, so a failed spawn leaves the caller holding it, and the caller drops it — closing its descriptor and, since no helper ever inherited one, releasing the lock as the last close.

- [x] **Step 1: Write the failing unlock-semantics tests**

```rust
// crates/yserver/src/kms/executor/device_lock.rs, in the existing #[cfg(test)] module
#[test]
fn dropping_a_device_lock_does_not_unlock_a_shared_description() {
    // The bug that makes the naive handoff wrong: an explicit unlock through
    // any descriptor sharing the open file description releases it globally.
    // (The literal flag name is spelled only inside `release_explicitly`, so
    // Task 7's "exactly one occurrence" gate means what it says.)
    let key = DrmDeviceKey { major: 226, minor: 250 };
    let lock = may_install_state(&key).expect("first holder");
    let duplicate = lock.duplicate_for_tests();
    drop(lock);
    assert!(
        may_install_state(&key).is_err(),
        "the surviving duplicate must still hold the lock"
    );
    drop(duplicate);
    assert!(may_install_state(&key).is_ok(), "the last close releases it");
}

#[test]
fn an_explicit_release_is_still_available_before_handoff_and_is_global() {
    let key = DrmDeviceKey { major: 226, minor: 251 };
    let lock = may_install_state(&key).expect("holder");
    let duplicate = lock.duplicate_for_tests();
    lock.release_explicitly();
    assert!(
        may_install_state(&key).is_ok(),
        "explicit release is global by design, which is why it disappears after handoff"
    );
    drop(duplicate);
}

#[test]
fn an_inheritable_lock_still_holds_and_still_releases_on_last_close() {
    let key = DrmDeviceKey { major: 226, minor: 252 };
    let inheritable = may_install_state(&key).expect("holder").into_inheritable();
    assert!(may_install_state(&key).is_err(), "the transition must not drop the lock");
    drop(inheritable);
    assert!(
        may_install_state(&key).is_ok(),
        "with no helper holding a copy, the last close releases it"
    );
}
```

- [x] **Step 2: Write the failing handoff tests**

```rust
// crates/yserver/tests/executor_lock_handoff.rs
#[test]
fn a_failed_spawn_leaves_the_lock_with_the_caller_who_releases_it() {
    let key = DrmDeviceKey { major: 226, minor: 253 };
    let inheritable = may_install_state(&key).expect("holder").into_inheritable();
    let dummy = std::fs::File::open("/dev/null").expect("dev null");
    let err = KmsIoExecutor::spawn_with_device_lock_at(
        std::path::Path::new("/nonexistent/yserver-executor"),
        dummy.as_fd(),
        IncarnationId::first(),
        LifecycleEpochId::first(),
        &inheritable,
    )
    .expect_err("spawning a nonexistent executable must fail");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    assert!(may_install_state(&key).is_err(), "the caller still holds it after a failed spawn");
    drop(inheritable);
    assert!(may_install_state(&key).is_ok(), "and releases it by dropping it");
}

#[test]
fn the_helper_holds_the_lock_after_the_parent_drops_its_copy() {
    let key = DrmDeviceKey { major: 226, minor: 254 };
    let inheritable = may_install_state(&key).expect("holder").into_inheritable();
    let dummy = std::fs::File::open("/dev/null").expect("dev null");
    let mut executor =
        KmsIoExecutor::spawn_with_device_lock(dummy.as_fd(), IncarnationId::first(), LifecycleEpochId::first(), &inheritable)
            .expect("spawn");
    // The readiness reply is what proves the helper reached its serve loop
    // with LOCK_FD adopted. Dropping the parent copy before that could
    // release the lock if the exec had failed.
    executor.await_helper_ready(Duration::from_secs(30)).expect("ready");
    drop(inheritable);
    assert!(
        may_install_state(&key).is_err(),
        "the helper's inherited descriptor must still hold the lock"
    );
    test_support::kill_and_reap(&mut executor);
    assert!(may_install_state(&key).is_ok(), "released only by the helper's death");
}

/// COMMIT-7's real threat model: not "the parent dropped a value" but "the
/// parent process died while a helper could still mutate the device". The
/// handoff subprocess acquires the lock, spawns a helper that inherits it,
/// prints the helper's pid, and `_exit`s without reaping.
///
/// **Two things kill an orphaned helper, and skipping one is not enough.**
/// Production arms `PR_SET_PDEATHSIG` with `SIGKILL`
/// (`executor/mod.rs:554-580,649-655`), and the real serve loop returns
/// `Ok(())` the moment its control socket reports EOF (`helper.rs:83-88`).
/// When the handoff process `_exit`s, its control endpoint closes, so an idle
/// helper exits through the second path even with the death signal disarmed.
/// A previous revision of this plan disarmed only the signal and claimed that
/// modelled a wedged helper; it does not.
///
/// The state COMMIT-7 exists for is a helper inside an uninterruptible kernel
/// call: `SIGKILL` is delivered and does nothing, and the control socket is
/// not being read, so EOF is never observed. A real D-state process cannot be
/// created portably from a test, so `WedgedHoldingLock` models exactly that
/// pair of properties — `SIGTERM`/`SIGKILL`-insensitive *and* not reading the
/// control socket — while genuinely holding the inherited lock descriptor.
/// It is a model, and the plan says so rather than implying it is the real
/// helper.
#[test]
fn the_lock_survives_the_death_of_the_process_that_acquired_it() {
    let key = DrmDeviceKey { major: 226, minor: 255 };
    let output = std::process::Command::new(executor_executable().expect("exe"))
        .arg(LOCK_HANDOFF_ARG)
        .arg(key.major.to_string())
        .arg(key.minor.to_string())
        .output()
        .expect("run the handoff subprocess");
    assert!(output.status.success(), "handoff subprocess failed: {output:?}");
    let helper_pid: libc::pid_t = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("the handoff subprocess prints its helper pid");

    // The acquiring process is gone — `output()` waited for it.
    assert!(
        may_install_state(&key).is_err(),
        "an orphaned helper must still hold the lock after its parent died"
    );

    // SAFETY: killing the orphaned helper this test created.
    unsafe { libc::kill(helper_pid, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if may_install_state(&key).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the lock was not released by the orphaned helper's death");
}

#[test]
fn the_lock_step_refuses_while_another_holder_has_it() {
    // Tests the acquisition step platform_init calls, not a fabricated
    // whole-bring-up entry point: platform_init takes device *paths*, and a
    // synthetic major/minor gives no path to open, so an
    // `open_kms_device_for_tests(&DrmDeviceKey)` could never have been the
    // thin wrapper the previous revision claimed.
    let key = DrmDeviceKey { major: 226, minor: 249 };
    let held = may_install_state(&key).expect("holder");
    let err = acquire_device_lock_or_refuse(&key).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ResourceBusy);
    assert!(format!("{err}").contains("226"), "the message must name the device");
    drop(held);
    assert!(acquire_device_lock_or_refuse(&key).is_ok(), "and it succeeds once free");
}
```

- [x] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p yserver kms::executor::device_lock
cargo test -p yserver --test executor_lock_handoff
```
Expected: FAIL — `Drop` still unlocks, and there is no `InheritableDeviceLock`, `LOCK_FD` or handoff entry point.

- [x] **Step 4: Replace the destructor and add the type transition**

**Delete `impl Drop for DeviceLock` entirely.** Do not replace it with an empty
one. Two reasons, and the second is fatal to the empty version:

- It is now unnecessary. `flock` releases on last close of the open file
  description, so letting the `File` field close itself *is* the wanted
  behaviour. There is nothing for a destructor to do.
- An empty `Drop` impl still counts as `Drop`, and Rust forbids moving an
  individual field out of a type that implements it (`E0509`). `into_inheritable`
  below moves `self.file`, so keeping any `Drop` impl makes this task
  non-compiling. `ManuallyDrop` and an `Option<File>` field would both work
  around it, but both add a mechanism to preserve a destructor that does
  nothing.

The reasoning that used to live in that destructor becomes a doc comment on the
struct, where it is just as visible and cannot be deleted by accident:

```rust
/// Advisory device-scoped exclusive lock guard.
///
/// **This type deliberately has no `Drop` impl.** `flock` releases on last
/// close of the open file description, and an explicit unlock through any one
/// duplicate releases it for *all* of them — including the executor's
/// inherited copy, which COMMIT-7 requires to outlive this process. Closing
/// the descriptor, which `File` does on its own, is exactly the semantics we
/// want. `release_explicitly` is the one path that still unlocks, and it
/// disappears at the `into_inheritable` transition.
///
/// The word for that operation is spelled out only inside `release_explicitly`,
/// so Task 7's gate can assert a single occurrence in this file without having
/// to tell a call from a comment.
#[derive(Debug)]
pub(crate) struct DeviceLock {
    file: File,
    path: PathBuf,
    device: DrmDeviceKey,
}

impl DeviceLock {
    /// The only explicit release, and it exists only before handoff: the
    /// single-process paths that never give the lock to a helper. It is
    /// consuming, and `InheritableDeviceLock` deliberately has no equivalent.
    pub(crate) fn release_explicitly(self) {
        // SAFETY: self.file is a valid descriptor owned by self.
        unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        // `self` drops here, closing the descriptor.
    }

    /// Consume the guard into the form that can cross `execve`. The returned
    /// value owns the descriptor, cannot unlock, and releases the lock only
    /// by closing — which, once a helper has inherited a copy, releases
    /// nothing.
    pub(crate) fn into_inheritable(self) -> InheritableDeviceLock {
        let device = self.device;
        // `File` -> `OwnedFd` moves the descriptor out without closing it.
        InheritableDeviceLock { fd: OwnedFd::from(self.file), device }
    }
}

/// A device lock that has been committed to executor ownership.
pub(crate) struct InheritableDeviceLock {
    fd: OwnedFd,
    #[allow(dead_code)]
    device: DrmDeviceKey,
}

impl InheritableDeviceLock {
    pub(crate) fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}
```

`DeviceLock` must stop deriving or implementing anything that would let `self.file` be duplicated implicitly. `into_inheritable` cannot use `Drop`-bypassing tricks: `File` already converts into `OwnedFd` by move, so no `ManuallyDrop` is needed.

- [x] **Step 5: Inherit the lock and prove the helper adopted it**

The handoff mirrors `CONTROL_FD` and `KMS_FD` exactly (`executor/mod.rs:589-663`):

1. `platform_init` calls `acquire_device_lock_or_refuse(&device_key)` after `primary_device_key_from_fd` qualifies the device (`kms/backend.rs:855-856`) and **before** Task 5's executor spawn and before `activate_initial_scanout_outputs`. It is **not** retried in a loop.

   **No new error type.** `platform_init` returns `io::Result<PlatformInit>` today and has many unrelated `io::Error` paths (`kms/backend.rs:829-882`); introducing an `OpenError` would require a signature migration through every caller and a set of `From` conversions that buy nothing here. A refusal is an `io::Error` with `ErrorKind::ResourceBusy`, whose message names the device and says an earlier incarnation's helper may still mutate it, and carries the recorded holder when `LockUnavailable` supplied one. Stage 1's lock holder already reports lock contention as an `io::Error` (`device_lock.rs:258-263`), so this matches the existing convention.
2. `into_inheritable()`, then `KmsIoExecutor::spawn_with_device_lock(kms_fd, incarnation, lifecycle_epoch, &inheritable)`. Its `pre_exec` adds one line beside the existing two: `duplicate_to_inherited_slot(lock_source, LOCK_FD)?`, where `lock_source` is a `duplicate_fd_at_least` of the lock fd. `dup2` clears `FD_CLOEXEC`, so the copy survives the exec. **Production keeps `PR_SET_PDEATHSIG` armed**, exactly as stage 1 spawns it: an idle helper orphaned by a dead parent should die and release the lock. COMMIT-7's guarantee covers the helper that death signal *cannot* reach.
3. `run_executor_helper` (`helper.rs:72-76`) adopts it with `take_inherited_fd(LOCK_FD, "executor device lock")` and holds the `OwnedFd` for the process lifetime. It re-asserts `LOCK_EX | LOCK_NB` on the inherited descriptor as a liveness assertion — the same open file description, so it is a no-op conversion that cannot fail; a failure means the descriptor is not the lock and the helper exits non-zero rather than serving. The helper takes `LOCK_FD` only when it is present, so the stub and lock-free spawn paths keep working.
4. `await_helper_ready(deadline)` sends a `HandshakeRequest` carrying the executor's incarnation and lifecycle epoch, then waits for its `HandshakeReply` under the 30-second cold-start bound. It uses the handshake codecs directly, not `send`/`poll_reply`, because no host call and no in-flight record exist yet. **It rejects a reply whose incarnation or lifecycle epoch does not echo the request's**, which is `ID-3` applied to this frame family.

   The wait needs a bounded blocking primitive, and the parent socket is non-blocking from construction (task 4). Reuse the **same** `libc::poll` site: `await_helper_ready` calls `wait_readable_bounded(fd, deadline)`, a private helper in `executor/mod.rs` that `dispatch_blocking_at_boundary` also calls. That keeps Task 7's "exactly one `libc::poll`" gate true and honest — it is one site with two callers, both at permitted `COMMIT-5` boundaries — rather than adding a second poll or a retry-sleep, which the sleep gate forbids. This is a permitted blocking boundary: it runs during `platform_init`, before any seat-active service, and it is the only thing that proves the exec succeeded and `LOCK_FD` was adopted.
5. **Only then** does the caller drop the `InheritableDeviceLock`. With the destructor above, that closes one descriptor of a shared description and releases nothing.

There is no window in which the lock is unheld: from step 1 the parent holds it, from step 2 both hold the same description, and after step 5 only the helper does. Step 4 is what makes step 5 safe — dropping before the readiness reply could release the lock if the exec had failed.

- [x] **Step 6: Add the handoff subprocess entry point**

Beside stage 1's `run_lock_holder_if_requested` (`device_lock.rs:216-252`). **The binary must also call it.** `bin/yserver.rs:6-44` currently dispatches the stub helper, the re-exec executor, the lock holder and the internal probe, in that order; a fifth block goes in beside them, or `LOCK_HANDOFF_ARG` falls through to ordinary argument parsing and the test below can never work:

```rust
if let Some(result) = yserver::kms::executor::device_lock::run_lock_handoff_if_requested() {
    return match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error!("yserver kms lock handoff: {error}");
            ExitCode::FAILURE
        }
    };
}
```


```rust
pub const LOCK_HANDOFF_ARG: &str = "--yserver-internal-kms-lock-handoff-v1";

/// Acquire the device lock, hand it to a real executor helper, print the
/// helper's pid, and exit **without** reaping it. This exists so a test can
/// observe COMMIT-7's actual property: the lock outliving the death of the
/// process that took it. It is never reached in normal operation.
#[doc(hidden)]
pub fn run_lock_handoff_if_requested() -> Option<io::Result<()>> {
    // ...parse major/minor exactly as run_lock_holder_if_requested does...
    let lock = DeviceLock::acquire(&device).map_err(...)?;
    let inheritable = lock.into_inheritable();
    let dummy = std::fs::File::open("/dev/null")?;
    // No PDEATHSIG, and a helper that never reads its control socket, so
    // neither death path applies. Its stderr is also /dev/null: a surviving
    // grandchild holding an inherited stderr pipe would stop the test's
    // `Command::output()` from ever seeing EOF.
    let mut executor = KmsIoExecutor::spawn_wedged_lock_holder_for_tests(
        dummy.as_fd(), IncarnationId::first(), LifecycleEpochId::first(), &inheritable,
    )?;
    executor.await_helper_ready(Duration::from_secs(30))?;
    println!("{}", executor.helper_pid());
    io::stdout().flush()?;
    drop(inheritable);
    // Leave the helper running and unreaped, and do not run KmsIoExecutor's
    // destructor: this process is simulating a parent that died.
    std::mem::forget(executor);
    // SAFETY: _exit performs no cleanup, which is the point.
    unsafe { libc::_exit(0) };
}
```

`std::mem::forget` before `_exit` is belt and braces: `_exit` already skips destructors, and the `forget` documents that skipping them is intentional rather than accidental.

- [x] **Step 1: Write the failing unlock-semantics tests**
- [x] **Step 2: Write the failing handoff tests**
- [x] **Step 3: Run the tests to verify they fail**
- [x] **Step 4: Replace the destructor and add the type transition**
- [x] **Step 5: Inherit the lock and prove the helper adopted it**
- [x] **Step 6: Add the handoff subprocess entry point**
- [x] **Step 7: Run the tests to verify they pass**
- [x] **Step 8: Commit**

*Execution note (Task 6):*
- `DeviceLock` `impl Drop` was completely removed; closing descriptor on last close releases the `flock`.
- `into_inheritable()` consumes `DeviceLock` into `InheritableDeviceLock` which has no unlock methods, preventing accidental global release after spawn.
- `release_explicitly()` is the sole `LOCK_UN` occurrence in `device_lock.rs`.
- `KmsIoExecutor::{spawn_with_device_lock, spawn_with_device_lock_at, spawn_wedged_lock_holder_for_tests, await_helper_ready, helper_pid}` implemented.
- `helper.rs` adopts `LOCK_FD` if present, re-asserts `flock(LOCK_EX | LOCK_NB) == 0`, and answers handshake requests in `serve_executor_loop`.
- `backend.rs` `platform_init` acquires the lock, spawns the executor with the inheritable lock, awaits helper ready handshake within 30s, and drops the parent's copy of the lock.
- `reap_within` in `test_support.rs` updated to invoke `executor.try_reap()`, ensuring live helper test teardown via `kill_and_reap` reaps immediately.
- Code committed in `5f37e95e`.

---

### Task 7: Portable gates and the stage reviewability check

These greps are a coarse net, not the proof. Every invariant below is already asserted behaviourally by a test in Tasks 4, 5 and 6; the greps exist to catch a *reintroduction* in a later edit that no existing test happens to cover. Where the reviewed draft used a source-text assertion **instead of** a behavioural one, the behavioural test has replaced it.

- [x] **Step 1: Run the full local gate**

```bash
cargo +nightly fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p yserver-core
cargo test -p yserver
```
Expected: all clean. `--all-targets` is required or lints in the new test modules are missed.

- [x] **Step 2: Run the three portable builds**

```bash
cargo build -p yserver --target x86_64-unknown-linux-gnu
cargo build -p yserver --target x86_64-unknown-linux-musl
cargo build -p yserver --target x86_64-unknown-freebsd
```
Expected: all compile. Every new ioctl goes through `platform/ioctl.rs`'s `iowr`, never a `libc::Ioctl` alias.

- [x] **Step 3: Verify no blocking wait was reintroduced on a core-thread path**

**Scope matters more than the pattern here.** The host-call path is
`mod.rs`, `helper.rs`, `transport.rs` and `protocol.rs`. `test_support.rs` is a
helper-process simulator whose whole job is to sleep — `NeverReply`,
`IgnoreTermination`, `AcceptAfter` and this stage's `AcceptAfterReturningInheritedFd`
all must — and `device_lock.rs` holds the lock-holder subprocess, which sleeps
to stay alive, plus `#[cfg(test)]` tests that legitimately `wait()` on a child.
A gate spanning the whole directory forbids code this plan requires, so it can
never pass:

```bash
rg -n 'std::thread::sleep' crates/yserver/src/kms/executor/mod.rs \
      crates/yserver/src/kms/executor/helper.rs \
      crates/yserver/src/kms/executor/transport.rs \
      crates/yserver/src/kms/executor/protocol.rs
rg -n 'child\.wait\(\)|\.wait\(\)' crates/yserver/src/kms/executor/mod.rs
rg -c 'libc::poll' crates/yserver/src/kms/executor/mod.rs
```
Expected: no `sleep` on the host-call path; **no `Child::wait()` in `mod.rs`** —
the stage-1 destructor's synchronous `wait()` is the specific regression this
catches, and it is the one the reviewed draft's `libc::poll`-only scan would
have declared clean; exactly one `libc::poll`, inside
`dispatch_blocking_at_boundary`.

- [x] **Step 4: Verify the lock and source invariants**

```bash
rg -n 'LOCK_UN' crates/yserver/src/kms/executor/device_lock.rs
rg -n 'impl Drop for DeviceLock' crates/yserver/src/kms/executor/device_lock.rs
rg -n 'BackendFdKind::ExecutorControl' crates/yserver-core/src/core_loop/run.rs \
      crates/yserver/src/kms/render/platform.rs
rg -n -B1 '^\s*pub (struct|enum|fn|const|mod) ' crates/yserver/src/kms/executor/protocol.rs \
   | rg -v 'doc\(hidden\)' | rg 'pub '
rg -n 'may_install_state|acquire_device_lock_or_refuse' crates/yserver/src/
rg -n 'open_any_drm_or_skip' crates/yserver/src/
```
Expected: exactly one `LOCK_UN`, inside `release_explicitly`, and none in
`InheritableDeviceLock`; **no `impl Drop for DeviceLock` at all**, because an
empty one still triggers `E0509` on the field move; the executor source both
published by `poll_fds` and dispatched by the core loop; **no output** from the
`protocol.rs` command, since `-B1` carries each declaration's preceding
attribute line into the match and the filter then drops every one that is
`#[doc(hidden)]` — a bare `pub` is what survives and fails the gate; lock
acquisition appearing only in `device_lock.rs` and in `platform_init`'s single
step, never on a discovery or probing path; no skip-shaped test helper anywhere.

That last grep replaces the previous revision's `discovery_probing_takes_no_install_lock`
test, which held a fabricated `(226, 248)` key that real candidate discovery
need never encounter and so passed even if discovery locked every card it
probed. Where discovery takes a lock is a structural property of the call
graph, and grepping the call sites tests it directly.

- [x] **Step 5: Confirm the deliberate scope boundary is still intact**

```bash
rg -n 'SequenceSupport' crates/yserver/src/kms/render/backend.rs | head -3
```
Expected: still present. This stage does **not** move it — spec lines 1755-1763 require it inside 2b's epoch-local clock record, which does not exist yet. The grep is here so an executor of this plan does not "helpfully" start that migration, and so a reviewer sees the omission is deliberate.

- [x] **Step 6: Update the status document**

Record that the executor substrate is complete and asynchronous, that it is a real core-loop source with a real deadline, that the device lock is executor-held, and that no owner or call-site conversion exists yet.

- [x] **Step 7: Commit**

```bash
git add docs/status.md
git commit -m "docs(kms): record the stage 2a executor substrate"
```

---

## Stage exit criteria

- The three portable builds pass, `cargo clippy --all-targets -- -D warnings` is clean, and both crates' suites are green.
- A real property list crosses the wire, the helper owns the holder storage, and every returned descriptor is adopted as an `OwnedFd` and released when the owner drops it — observed through a pipe's EOF. That is a joint parent-and-helper property and it is *release*, not close cardinality; the exactly-once claim is carried by `OwnedFd`'s own type guarantee and by the helper's `HolderLedger` unit tests, not by this test.
- Every reply echoes its request's correlation tuple, and a mismatch is `MalformedReply` — never a rejection. The clock-probe tuple carries topology generation, so stage 1's request is not regressed. `correlation()` and `class()` are total and bare on `HostCallRequest`, which is what makes `ID-3` structural rather than a convention.
- A frame whose class, flags and payload disagree is refused by both the encoder and the decoder, so a live commit cannot be labelled validation and a seat-active commit cannot omit `NONBLOCK`. All four classes exist, so cold-start/offline validation can be expressed with its 30-second watchdog.
- Every request kind has exactly one legal reservation, and a clock probe carries its own: the type system cannot be used to install a commit record for a read-only query. The probe's `sequence` reaches the caller as `ProbeAccepted`, so 2b's clock record has the evidence it decides on.
- Descriptor release is observed through a pipe's EOF on a **non-blocking** read end, so the negative check reports "still open" instead of deadlocking.
- `DeviceLock` has no `Drop` impl at all, which is both the correct `flock` semantics and what lets `into_inheritable` move its descriptor out.
- **No seat-active path waits on a host call.** `send` returns after the frame is sent, replies arrive through `poll_reply`, the watchdog fires from `tick`, `libc::poll` appears exactly once, no `std::thread::sleep` remains on any host-call path, and `Drop` no longer calls `Child::wait()`.
- The core event loop registers the control fd **and** carries the executor deadline in `next_wakeup`: the asynchronous API has a consumer and the watchdog is reachable on an idle server.
- Exactly one host call is in flight at a time. Every acceptance-unknown path — send failure, EOF, receive failure, malformed reply, watchdog expiry — produces exactly one terminal event, enters `Stalled`, and retains `in_flight` until a proven reap, so no second ioctl reaches a device whose acceptance is unknown. A reply arriving after terminalization is delivered as `LateReply` with its descriptors adopted, and is not reap proof.
- `DeviceLock::drop` does not unlock; `release_explicitly` does not exist after the handoff transition; the lock is inherited by the helper across the re-exec and proven adopted by a readiness reply before the parent drops its copy; the lock survives the death of the process that acquired it; and a start attempted while an orphaned helper holds it refuses without installing state.
- Identity allocation is checked and cannot wrap.

## What stage 2b consumes

- `KmsIoExecutor::{send, control_fd, poll_reply, tick, next_deadline}` and `HostCallEvent`. 2b's owner replaces the backend's `record_host_call_events` log-and-discard queue as the consumer.
- `SubmittingProof`, `ValidationLease` and `ClockProbeLease` keep their test-only constructors here; 2b adds the production producers at record installation, at validation-lease acquisition, and at clock-probe reservation.
- `HostCallOutcome::ProbeAccepted { sequence, .. }` and an `EOPNOTSUPP` `Rejected` are the two inputs 2b's epoch-local clock record decides `KernelSequence` versus `Unresolved` from.
- **A prerequisite, not a handover: core poll-source churn.** `run_core` collects `Backend::poll_fds()` once and never refreshes it (`run.rs:1045-1059`). 2a never replaces an executor so it does not care, but 2b cannot bring a replacement executor's control fd into a running loop until that mechanism exists. Build it before the first reopen path, not after.
- `HostCallCorrelation` is what 2b's records match a reply against.
- `HostCallPhase` and `enter_seat_active`/`enter_final_offline`: 2b's lifecycle transitions are what call them, which is what makes the `COMMIT-5` boundary check meaningful rather than permanently `ColdStart`.
- The `Dispatched` milestone is set when `send` returns, not when the reply arrives.
- The `SequenceSupport` map at `kms/render/backend.rs:1042` is 2b's to move into the epoch-local clock record.

## Self-review notes

- **Scope.** Six implementation tasks plus the gate, against stage 1's fourteen. Every task delivers something testable without an owner: the identities, the wire, the helper, the async API, its loop integration, and the lock.
- **What the fourth revision changed.** Round 3's own regressions, and their cause. The contract section above is the structural answer: `Ready` regained its epoch (it was never forced to lose it by the frame split), the golden token gained its purpose tag, the out-fence and watchdog rules are written against `class.is_validation()` so a fifth class cannot open a hole, `InFlight` retains the kind and slot count reply validation needs, and `send` — not only the blocking wrapper — enforces the phase, which is where `COMMIT-5` was actually unguarded. Two fixes that had been declared but never landed in a file list or a commit now do. One fix that did not work was replaced: disarming `PDEATHSIG` left the second orphan-kill path open, because the real serve loop exits on control-socket EOF, so the handoff test now models a wedged helper as one that is signal-insensitive *and* not reading its control socket.
- **What the third revision changed, and it was mostly one thing.** Seven of round 2's ten blockers were local: an `E0509` field move out of a `Drop` type, a blocking pipe read that would hang, a one-shot stub reused for two dispatches, greps that forbade code the plan itself requires, an entry point nothing called, a `from_raw` that does not exist, and two decoders that never checked the purpose tag they document. Each is fixed where it stood.

  The other three were one gap: **the host-call type model was too narrow.** `Ready` sat inside `HostCallRequest` where it could carry neither correlation nor class, making both accessors impossible and putting a permanent `ID-3` exception inside the type meant to enforce it — it is now a separate frame family. A clock probe had no legal reservation and its `sequence` was discarded before its only consumer could read it — it now has `ClockProbeLease` and `ProbeAccepted`. And `spec:320-329` requires cold-start/offline validation at thirty seconds, which a three-variant class enum could not express — there are now four.
- **What the second review changed.** Tasks 2, 4 and 6 were rewritten whole rather than patched, and the event-loop integration became its own task once the real `BackendFdKind`/`run.rs`/`poll_fds` surface was read instead of assumed. Three claimed guarantees were downgraded to what can actually be proven: the boundary check is a tested runtime precondition rather than a forgeable witness, descriptor closure is observed through a pipe's EOF rather than through a ledger that cannot see `OwnedFd`, and the timing assertions use the class watchdog as their ceiling rather than 10 ms.
- **One deliberate conservatism.** A failed `send` on a `SOCK_SEQPACKET` did not enqueue the datagram, so `FailedBeforeSubmit` would arguably be provable. The plan classifies it `Unknown` anyway: `COMMIT-6`'s default is unknown, the cost is one quarantine on a path where the helper is usually dead regardless, and a wrong `FailedBeforeSubmit` would release resources the kernel might still own.
- **One thing this stage still cannot prove.** Helper-side exactly-once closing is asserted by the helper's own unit tests and reported through `unexpected_fence_output`. The parent-side pipe test proves the parent closes what it adopts; it does not prove the helper closed its copies, because a helper that leaked one would keep the pipe open and the test would fail — which makes it a *joint* assertion, not a helper-side one. The plan says so rather than implying separable coverage it does not have.
- **One thing this stage deliberately does not do.** The stage-1 `SequenceSupport` gap is left in place, with a grep in Task 7 to keep it that way, because the record it must move into is 2b's.
- **One constraint it inherits and does not fix.** Core poll sources are collected once and never refreshed. 2a never replaces an executor, so it is unaffected; 2b cannot replace one without adding source churn to `Backend` first. Recorded in Task 5 and in "What stage 2b consumes" rather than solved here.

## Findings status

Nothing from rounds 1, 2 or 3 is knowingly outstanding.

| Round | Result | Disposition |
| --- | --- | --- |
| 1 | 8 blocking, 9 major, 2 minor | all addressed in revision 2 |
| 2 | 10 blocking, 7 major, 1 minor | blocking + M-5 in revision 3; the six remaining majors and m-1 in revision 4 |
| 3 | 10 blocking, 2 major, 2 minor | all addressed in revision 4 |

Round 3's m-1 said revision 3's self-review overstated a fix, and it was right:
the literal flag name had been removed from one comment and left in another in
the same file. This revision states plainly that a claim of the form "resolved"
is worth nothing unless the gate that would catch it was actually run. Task 7's
greps are that gate, and they are meant to be run before this section is
believed.
