# Phase C.0 Stage 2 — Device commit owner and merged-primary integration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the device-local atomic commit owner — one slot, canonical out-fence completion evidence, tagged page-event correlation, bounded admission with a starvation bound — and route every merged primary submission (composed flip, copied scanout, direct scanout, composed replacement, direct successor promotion) through it.

**Architecture:** Stage 1 delivered an executor that can perform an *empty* atomic ioctl. Stage 2 first teaches the wire protocol to carry a real serialized property list plus helper-owned `OUT_FENCE_PTR` holder storage, then builds `KmsDeviceOwner` on top: it constructs requests through one builder that computes `AtomicCrtcClosure`/`ExpectedCompletionCrtcs` and enforces the off-to-off signaling rule, installs a `Submitting` record before IPC, adopts out-fences and queries canonical sync-file status for `HardwareComplete`, resolves tagged page events to `Presented`, and admits work through the seven-tier fair-admission function. The three merged primary submission helpers stop calling `Device::atomic_commit` and become request builders the owner submits.

**Tech Stack:** Rust (stable toolchain), `drm` 0.15 / `drm-ffi` 0.9, `libc`, `std::os::unix` sockets. No serialization crate: framing stays hand-rolled, extended from stage 1's fixed frames to one fixed head plus a bounded variable payload.

**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md` (Approved, revision 2, 2026-09-03). This plan implements section 18 **stage 2 only**, plus section 12.1, whose damage-transaction mapping belongs to this stage because this stage owns the milestones it maps. Stages 3 and 4 (lifecycle/modeset/DPMS/VT/topology; cursor and gamma conversion) are planned separately.

**Predecessor:** `docs/superpowers/plans/2026-09-02-phase-c0-stage-1-executor-substrate.md`, complete at `83b47700`. Its "What stage 2 consumes" section names the three products this stage spends: the six real `atomic_commit` call sites, the `SubmittingProof` producer, and the `may_install_state` production caller.

---

## Revision 2 — corrected architecture

**Status: rework in progress.** The pre-execution review
(`docs/superpowers/findings/2026-09-03-phase-c0-stage-2-plan-adversarial-review.md`)
found 24 blocking, 24 major and 7 minor defects in revision 1. Several are
structural, so this section states the corrected architecture normatively. **A
task that contradicts this section is wrong and has not been reworked yet.**
Tasks are being rewritten in the order listed under "Corrected task order"; the
heading of each reworked task carries `[r2]`.

### The submission path is asynchronous

Revision 1's `KmsDeviceOwner::submit` called stage 1's `KmsIoExecutor::dispatch`
and handled the outcome before returning. That dispatch polls the control socket
until reply or watchdog expiry (`kms/executor/mod.rs:293-400`), so a live render
path could stall the X11 core for two seconds — violating `COMMIT-5` verbatim
and reintroducing the exact stall section 4.1 exists to remove. It would also
prevent the owner from draining page events while the helper is inside the
ioctl, which is precisely what the `Submitting` interval must do.

The executor gains a split API and the blocking one is confined by its name:

```rust
/// A sent host call awaiting its reply. Non-`Clone`: exactly one is
/// outstanding per executor, which is what serializes host calls.
pub(crate) struct InFlightHostCall {
    seq: RequestSeq,
    class: HostCallClass,
    started: Instant,
    deadline: Instant,
}

impl KmsIoExecutor {
    /// Encodes and sends. Never waits for a reply.
    pub(crate) fn send(&mut self, request: &HostCallRequest, proof: SubmittingProof)
        -> Result<InFlightHostCall, SendError>;

    /// Registered with the core event loop for readability.
    pub(crate) fn control_fd(&self) -> BorrowedFd<'_>;

    /// Called on readability. Returns `None` if the frame is incomplete.
    /// Never blocks and never sleeps.
    pub(crate) fn poll_reply(&mut self, in_flight: &InFlightHostCall)
        -> Option<HostCallOutcome>;

    /// Called from the event loop's timer tick.
    pub(crate) fn check_watchdog(&mut self, in_flight: &InFlightHostCall, now: Instant)
        -> Option<HostCallOutcome>;

    /// The ONLY blocking form. `COMMIT-5` permits a blocking ioctl solely at a
    /// cold-start-before-service or final-offline boundary; the name is the
    /// enforcement, so no seat-active caller can reach it by accident.
    pub(crate) fn dispatch_blocking_at_permitted_boundary(
        &mut self, request: &HostCallRequest, proof: SubmittingProof,
    ) -> HostCallOutcome;
}
```

### The owner publishes one typed outcome stream

Revision 1's `submit` returned `Result<(), SubmitError>`, discarding both the
`CommitId` and the ioctl outcome that tasks 16, 17 and 18 all require. It is
replaced by:

```rust
pub(crate) fn submit(
    &mut self,
    request: SerializedRequest,
    class: CommitClass,
    resources: ResourceLedger,
) -> Result<CommitId, SubmitError>;

/// Drains everything that became true since the last call. The core event loop
/// calls these two and routes the events; nothing polls owner internals.
pub(crate) fn on_control_readable(&mut self) -> Vec<OwnerEvent>;
pub(crate) fn tick(&mut self, now: Instant) -> Vec<OwnerEvent>;
```

```rust
pub(crate) enum OwnerEvent {
    Accepted(CommitId),
    Rejected { commit: CommitId, errno: i32 },
    CompletionUnknown { commit: CommitId, reason: UnknownReason },
    HardwareComplete(CommitId),
    Presented { commit: CommitId, crtc: u32, sample: ClockSample },
    Completed(CommitId),
    PriorBufferReleased { commit: CommitId, buffer: BufferRef },
    DamageInvalidate { outputs: Vec<usize>, cause: DamageInvalidateCause },
}
```

This one stream replaces revision 1's separate `DamageEvent` type: the damage
tasks consume `OwnerEvent` like every other consumer, which closes the gap where
only the host-call unknown arm emitted an invalidation while fence failure,
deadline expiry and poison emitted nothing.

### Submission consumes owned resources, not handles

`COMMIT-6` requires the record to uncertainty-own every possible old and new
resource before IPC. Revision 1 passed only a `SerializedRequest` and left the
backend as the real owner, so nothing kept a framebuffer, pin or external
ownership alive across acceptance uncertainty. `submit` now consumes:

```rust
pub(crate) struct ResourceLedger {
    old: OwnedResourceSet,
    new: OwnedResourceSet,
}
```

where `OwnedResourceSet` holds strong RAII references — framebuffers, blobs,
pins, descriptors and external-ownership tokens. Rejection, completion and
quarantine consume it through typed transitions; nothing else can release it.

### Admission candidates carry generations, not a boolean

Revision 1 reduced admission compatibility to
`fn(MaintenanceIdentity, u32) -> bool`, which cannot inspect the closure,
generations, completion coverage or synchronous class the seven tiers are
defined over. The scheduler now receives values produced by the request builder:

```rust
pub(crate) struct AdmissionCandidate {
    crtc: u32,
    kind: PrimaryKind,               // Composed | DirectSuccessor | Unflip
    generation: PrimaryGeneration,
    closure: BTreeSet<u32>,
    completion_covered: bool,
    absorbs: Vec<(MaintenanceIdentity, MaintenanceGeneration)>,
    offered: AdmissionTicket,        // gives tier 6 its "oldest"
}
```

Round-robin state moves inside `AdmissionState` and is advanced by `select`
itself; no caller supplies `owed_crtc`. All seven tiers are re-derived from
spec section 9.2.1 rather than edited, because tier 3's rule was inverted in
revision 1.

### Corrected task order

Revision 1's order was not implementable: task 5 declared an "opaque"
`FenceSlotState` that task 7 redefined (impossible for a Rust enum) and placed a
`StagedPageEvent` that task 8 had not yet defined. Foundational types now
precede their consumers.

| # | Task | Status |
| --- | --- | --- |
| 1 | Lifecycle identities and explicit host-call class | carried from r1 |
| 2 | Atomic property payload, reply correlation tuple, 68-byte head | rework |
| 3 | Helper materialization, holder ownership, bitmap validation | rework |
| 4 | **Asynchronous host-call API** | new |
| 5 | Owner request builder, closure derived from the serialized payload | rework |
| 6 | Resource ledger, commit records, milestones, tombstones | rework |
| 7 | Device slot, asynchronous submit, the `OwnerEvent` stream | rewrite |
| 8 | Out-fence adoption and canonical sync-file status | carried, tests reworked |
| 9 | Page-event correlation, per-CRTC `Presented`, wrong-type poison | rework |
| 10 | **Migrate the `SequenceSupport` cache into the clock record** | new |
| 11 | Clock probe with its own host-call reservation | rework |
| 12 | Sequence normalization, integrated into event handling | rework |
| 13 | Deadlines: checked arithmetic and the unvalidated-cohort disposition | rework |
| 14 | Qualification via an explicit install/restore record property | rework |
| 15 | Bounded intents, tickets, aged-behind-submitted, displaced ownership | rework |
| 16 | Admission candidate and the seven tiers re-derived | rebuild |
| 17 | Terminalization with unique keys, FIFO and liveness | rework |
| 18 | Composed conversion and the producer wait as a real `BoState` | rework |
| 19 | Direct conversion and a snapshot bound to the exact request | rework |
| 20 | Damage transaction on `OwnerEvent` | rework |
| 21 | Damage unknown/poison/bundle and the restore disposition | rework |
| 22 | Device lock held by the executor, not the parent | rework |
| 23 | Portable gates and the section 16.3 evidence manifest | rework |

Task 10 exists because the review found an unmet **stage 1** requirement:
`kms/render/backend.rs:1042` still holds
`HashMap<(DrmDeviceKey, ClockEpochId), SequenceSupport>`, read at `:9246`,
`:9297` and written at `:9382`. Stage 1 removed the old
`crtc_queue_sequence_unsupported_devices` name but not the separate cache, and
spec section 10 requires that decision to live in the epoch-local CRTC clock
record keyed by hardware CRTC. Revision 1's task 9 asserted the removal had
already happened and built on it.

---

## Global Constraints

Copied from the spec. Every task's requirements implicitly include this section.

- The X11 core never executes or waits synchronously for a potentially blocking KMS ioctl (`COMMIT-5`).
- The owner installs `Submitting` and the applicable fd lease **before** IPC dispatch (`COMMIT-6`). After dispatch, explicit rejection, success and acceptance-unknown remain distinct; IPC loss, helper exit or watchdog expiry can never be rewritten as rejection.
- Exactly one dispatched-or-submitted live atomic transaction per DRM device, **not** one per CRTC (`SingleSlotMultiCrtcCeiling`). The slot is reserved before IPC and is not released because a result is late.
- Host-call watchdog: 2 seconds for seat-active `NONBLOCK` work and for seat-active `ValidationOnly`, 30 seconds for a permitted cold-start/final-offline blocking ioctl.
- `COMMIT-4` — no unresolved C.0 input fence. Every producer dependency finishes successfully **before** admission; the live request omits `IN_FENCE_FD` or supplies `-1`.
- For every class except C.1 async direct, the owner adds exactly one `OUT_FENCE_PTR` property for every member of `ExpectedCompletionCrtcs`, and none outside it.
- Construction fails before submit for every inactive-to-inactive CRTC in `AtomicCrtcClosure` if the global page-event flag is set or an out-fence pointer was assigned to that CRTC. Adding an out-fence to an off-to-off CRTC "for symmetry" is forbidden.
- `COMMIT-2` — `ProducerReady`, `Dispatched`, `Accepted`, `HardwareComplete`, `Presented` and `PriorBufferReleased` are independent typed facts. Observing one never fabricates another.
- `COMMIT-3` — an out-fence proves the CRTC flip/scanout milestone, not full teardown.
- Live success plus a `-1` out-fence holder, a non-sync-file fd, partial output, poll error or deadline expiry latches the mechanism failed and enters `CompletionUnknown`. `-1` is valid only after a rejected ioctl or `TEST_ONLY`.
- The executor never reads the DRM event fd; drain is owner-exclusive for the incarnation.
- Atomic `EBUSY` with no owner-tracked live record is an explicit pre-submit rejection and an invariant failure: no retry, no spin (`§9.4`).
- Deadlines use `CLOCK_MONOTONIC`/`Instant`. `FastHardwareCompletionDeadline = clamp(3 * slowest_affected_mode_period, 100 ms, 2 s)`, unknown mode period is `16.667 ms` before clamping. `deadline[crtc] = hardware_complete_observed_at + clamp(2 * mode_period[crtc], 50 ms, 500 ms)`.
- The latency recorder performs no filesystem write, allocation, flush or additional supervisor IPC on the measured path, never wraps, and makes an exhausted row `EvidenceInsufficient`.
- Portable builds must compile on glibc, musl and FreeBSD.
- Format check is `cargo +nightly fmt --check`. Tests are `cargo test -p yserver`. Lint is `cargo clippy --all-targets -- -D warnings`, exactly as CI runs it.

### Deliberate stage boundaries

Recorded so this stage is not judged against another stage's outcome.

- **Cursor and gamma payload construction is stage 4.** Stage 2 models a maintenance identity as an opaque `(CRTC, class)` with a generation counter, so the ticket, aging and starvation bound in `§9.2.1` are built and tested now and stage 4 fills in the payload. No cursor or gamma property is emitted by this stage.
- **Modeset, DPMS, VT, topology and the `REC-4`/`REC-5`/`REC-6` arbiter are stage 3.** Stage 2 implements `Unqualified`, `Ready`, `Quiescing` and `Poisoned` only, and the qualification commit is the first converted primary commit rather than a converted install/restore modeset. `modeset.rs:1144` (`disable_output`) and `modeset.rs:1305` (`modeset_with_flags`) keep their direct `atomic_commit` in this stage.
- **The C.1 async-direct commit class is out of scope.** `CommitClass` has no async variant here; C.1 adds it.
- **`crates/yserver/src/present/event_loop.rs` is out of scope.** Its `run_loop` has no caller anywhere in the workspace — it is a standalone presenter demo, not a live KMS mutation path — exactly as `kms/console.rs` was excluded in stage 1. Do not convert its `submit_flip` calls.

---

## File Structure

**New:**
- `crates/yserver/src/kms/owner/lifecycle.rs` — `LifecycleEpochId`, `LifecycleTransitionId`, `DeviceLifecycleState`.
- `crates/yserver/src/kms/owner/request.rs` — `AtomicRequestBuilder`, `AtomicCrtcClosure`, `ExpectedCompletionCrtcs`, off-to-off signaling rule, final serialized re-scan, `SerializedRequest`.
- `crates/yserver/src/kms/owner/commit.rs` — `CommitRecord`, `CommitClass`, `CommitState`, `Milestones`, `ResourceLedger`, `TombstoneRing`.
- `crates/yserver/src/kms/owner/fence.rs` — out-fence adoption, canonical `SYNC_IOC_FILE_INFO` status query, `FenceSlotState`.
- `crates/yserver/src/kms/owner/events.rs` — page-event correlation and its normative dispositions.
- `crates/yserver/src/kms/owner/clock.rs` — per-`(incarnation, hardware CRTC, clock epoch)` clock record, probe dispatch, UST and `u32`→`u64` sequence normalization.
- `crates/yserver/src/kms/owner/deadline.rs` — the three post-dispatch monotonic timers.
- `crates/yserver/src/kms/owner/admission.rs` — bounded intents, `AdmissionTicket`, aging, the seven tiers.
- `crates/yserver/src/kms/owner/terminalize.rs` — `§10.4` Present/idle/release terminalization ledger.
- `crates/yserver/src/kms/owner/device_owner.rs` — `KmsDeviceOwner`: the slot, dispatch, terminalization, qualification.
- `crates/yserver/tests/device_owner.rs` — integration tests that spawn real helper processes and submit real property lists against a stub target.

**Modified:**
- `crates/yserver/src/kms/owner/mod.rs` — module declarations.
- `crates/yserver/src/kms/executor/protocol.rs` — protocol version 2: explicit host-call class, `LifecycleEpochId`, bounded variable atomic payload, out-fence slot table, reply presence bitmap.
- `crates/yserver/src/kms/executor/helper.rs` — materialize the property arrays, own the `OUT_FENCE_PTR` holder storage, patch holder addresses, return fds in slot order.
- `crates/yserver/src/kms/executor/transport.rs` — variable-length request frames.
- `crates/yserver/src/kms/executor/mod.rs` — `dispatch` takes the owned request; `HostCallClass` comes from the request field, not from the `NONBLOCK` bit.
- `crates/yserver/src/drm/page_flip.rs` — `submit_flip_with_fences` becomes `build_composed_flip_request`; no `atomic_commit` remains.
- `crates/yserver/src/drm/modeset.rs:1562,1635,1690` — the direct-scanout `TEST_ONLY` probe, `submit_direct_scanout` and `submit_composed_scanout` become request builders.
- `crates/yserver/src/kms/render/platform.rs:5163` — `submit_copied_scanout` waits for its copy fence before admission and submits through the owner.
- `crates/yserver/src/kms/render/backend.rs:1831,1843,1892,2234` — direct submission, successor promotion and composed replacement route through the owner.
- `crates/yserver/src/kms/render/scene.rs:6769` — the per-output composed flip routes through the owner.
- `crates/yserver/src/kms/render/scene.rs:1978,4344` — the damage transaction's apply and stage sites move onto owner milestones (`DMG-1`, `DMG-2`). The defensive `invalidate()` on platform/scene divergence a few lines above the apply site stays untouched.
- `crates/yserver/src/kms/render/backend.rs:2273,17273` — the existing damage invalidations gain typed causes and are joined by the acceptance-unknown, poison and lifecycle ones.

`crates/yserver/src/kms/render/scanout_damage.rs` itself is **not** modified. Its
transactional contract is already the right one; stage 2 changes only which
events drive it. A task that finds itself editing that module has almost
certainly mapped a milestone wrongly and should re-read section 12.1.
- `crates/yserver/src/kms/backend.rs:844` — real device open takes the `COMMIT-7` device lock.

---

### Task 1: Lifecycle identities and an explicit host-call class on the wire

Stage 1 put `ClockEpochId` in `AtomicRequest`'s lifecycle-epoch field and derived the watchdog from the `NONBLOCK` bit. Both are wrong for stage 2: `§6.1` makes the lifecycle epoch a distinct identity, and `§5` gives seat-active `ValidationOnly` the two-second watchdog even though `TEST_ONLY` never sets `NONBLOCK`. Fix both before any real payload exists.

**Files:**
- Create: `crates/yserver/src/kms/owner/lifecycle.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`
- Modify: `crates/yserver/src/kms/executor/protocol.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs:160-181` (`HostCallClass::from_request`)

**Interfaces:**
- Consumes: `IncarnationId`, `ClockEpochId`, `CommitId`, `EventToken` from `kms/owner/identity.rs`; `HostCallClass` from `kms/executor/mod.rs`.
- Produces: `LifecycleEpochId::{first, next, get, from_raw}`, `LifecycleTransitionId::{from_raw, get}`, `DeviceLifecycleState`, and an `AtomicRequest.class: HostCallClass` field carrying `HostCallClass::{SeatActiveNonblock, ColdStartOrOfflineBlocking, SeatActiveValidation}`.

- [ ] **Step 1: Write the failing test**

In `crates/yserver/src/kms/owner/lifecycle.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{DeviceLifecycleState, LifecycleEpochId};

    #[test]
    fn lifecycle_epoch_is_monotonic_and_starts_at_one() {
        let first = LifecycleEpochId::first();
        assert_eq!(first.get(), 1);
        assert_eq!(first.next().get(), 2);
        assert!(first.next() > first);
    }

    #[test]
    fn a_fresh_device_is_unqualified_not_ready() {
        assert_eq!(DeviceLifecycleState::default(), DeviceLifecycleState::Unqualified);
        assert!(!DeviceLifecycleState::Unqualified.admits_ordinary_primary());
        assert!(DeviceLifecycleState::Ready.admits_ordinary_primary());
        assert!(!DeviceLifecycleState::Quiescing.admits_ordinary_primary());
        assert!(!DeviceLifecycleState::Poisoned.admits_ordinary_primary());
    }
}
```

In `crates/yserver/src/kms/executor/protocol.rs` tests:

```rust
#[test]
fn validation_only_requests_carry_the_seat_active_watchdog() {
    // TEST_ONLY never sets NONBLOCK, so deriving the class from the flag bit
    // would give a ValidationOnly call the 30-second cold-start watchdog.
    let request = atomic_request_for_tests(0 /* no NONBLOCK */, HostCallClass::SeatActiveValidation);
    assert_eq!(
        HostCallClass::from_request(&HostCallRequest::Atomic(request)),
        HostCallClass::SeatActiveValidation
    );
    assert_eq!(HostCallClass::SeatActiveValidation.watchdog(), Duration::from_secs(2));
}

#[test]
fn atomic_frame_round_trips_the_lifecycle_epoch_and_class() {
    let request = AtomicRequest {
        seq: RequestSeq::for_tests(7),
        incarnation: IncarnationId::from_raw(3),
        lifecycle_epoch: LifecycleEpochId::from_raw(9),
        transition: Some(LifecycleTransitionId::from_raw(4)),
        commit: CommitId::for_tests(11),
        event_token: EventToken::for_tests(0x4000_0000_0000_0001),
        class: HostCallClass::SeatActiveNonblock,
        flags: 0x0200,
        ..atomic_request_shell_for_tests()
    };
    let frame = encode_request(&HostCallRequest::Atomic(request.clone()));
    let decoded = decode_request(&frame).expect("decode");
    assert_eq!(decoded, HostCallRequest::Atomic(request));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver lifecycle -- --nocapture` and `cargo test -p yserver protocol::tests`
Expected: FAIL — `LifecycleEpochId` and `HostCallClass::SeatActiveValidation` do not exist.

- [ ] **Step 3: Write the implementation**

`crates/yserver/src/kms/owner/lifecycle.rs`:

```rust
//! Lifecycle identities and the C.0 device lifecycle states this stage drives.
//!
//! `LifecycleEpochId` is always present, including during ordinary `Ready`
//! traffic (spec 6.1). A transition id is optional: ordinary cursor/gamma/
//! primary commits carry `None`, never a fabricated or previous id.

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct LifecycleEpochId(u64);

impl LifecycleEpochId {
    pub(crate) const fn first() -> Self {
        Self(1)
    }

    pub(crate) const fn next(self) -> Self {
        Self(self.0 + 1)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct LifecycleTransitionId(u64);

impl LifecycleTransitionId {
    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// The subset of the section 6.4 device lifecycle matrix this stage drives.
/// Stage 3 adds `Recovering(RecoveryId)` and `RecoveryFailed`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub(crate) enum DeviceLifecycleState {
    /// C.0 install/restore only; never ordinary traffic and never C.1.
    #[default]
    Unqualified,
    Ready,
    Quiescing,
    Poisoned,
}

impl DeviceLifecycleState {
    pub(crate) const fn admits_ordinary_primary(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// `Unqualified` still admits the mandatory install/restore commit that
    /// qualifies the incarnation; every other state that is not `Ready` is
    /// closed to live KMS admission.
    pub(crate) const fn admits_qualification_commit(self) -> bool {
        matches!(self, Self::Unqualified | Self::Ready)
    }
}
```

In `crates/yserver/src/kms/owner/mod.rs`:

```rust
pub(crate) mod identity;
pub(crate) mod lifecycle;
```

In `crates/yserver/src/kms/executor/mod.rs`, replace the `HostCallClass` enum and its derivation:

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub enum HostCallClass {
    SeatActiveNonblock,
    SeatActiveValidation,
    ColdStartOrOfflineBlocking,
}

impl HostCallClass {
    pub const fn watchdog(self) -> Duration {
        match self {
            Self::SeatActiveNonblock | Self::SeatActiveValidation => Duration::from_secs(2),
            Self::ColdStartOrOfflineBlocking => Duration::from_secs(30),
        }
    }

    /// The class is a declared field of the request, never re-derived from a
    /// flag bit: `TEST_ONLY` carries no `NONBLOCK` and would otherwise inherit
    /// the 30-second cold-start watchdog while seat-active.
    pub(crate) fn from_request(request: &HostCallRequest) -> Self {
        match request {
            HostCallRequest::Atomic(atomic) => atomic.class,
            HostCallRequest::ClockProbe(_) => Self::SeatActiveNonblock,
        }
    }

    pub(crate) const fn wire_tag(self) -> u8 {
        match self {
            Self::SeatActiveNonblock => 1,
            Self::SeatActiveValidation => 2,
            Self::ColdStartOrOfflineBlocking => 3,
        }
    }

    pub(crate) const fn from_wire_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::SeatActiveNonblock),
            2 => Some(Self::SeatActiveValidation),
            3 => Some(Self::ColdStartOrOfflineBlocking),
            _ => None,
        }
    }
}
```

In `crates/yserver/src/kms/executor/protocol.rs`, bump `PROTOCOL_VERSION` to `2`, replace `AtomicRequest`'s `epoch: ClockEpochId` with `lifecycle_epoch: LifecycleEpochId`, its `transition: Option<u64>` with `Option<LifecycleTransitionId>`, and add `class: HostCallClass`. Encode the class as one `u8` and the transition as a presence byte plus a `u64`. `ClockProbeRequest` keeps `ClockEpochId` for both its `clock_epoch` and gains `lifecycle_epoch: LifecycleEpochId` in place of its `epoch` field.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver lifecycle protocol`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/lifecycle.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/src/kms/executor/protocol.rs crates/yserver/src/kms/executor/mod.rs
git commit -m "feat(kms): add lifecycle identities and an explicit host-call class"
```

---

### Task 2 `[r2]`: Carry a real atomic property payload over the executor wire

Stage 1's helper submits `count_objs = 0` with null pointers. Nothing in the protocol can express a property list or an out-fence holder. This task makes the request frame `fixed head + bounded variable payload` and adds the out-fence slot table; task 3 makes the helper act on it.

**Files:**
- Modify: `crates/yserver/src/kms/executor/protocol.rs`
- Modify: `crates/yserver/src/kms/executor/transport.rs`
- Test: `crates/yserver/src/kms/executor/protocol.rs` (unit tests in-module)

**Interfaces:**
- Consumes: `AtomicRequest` from task 1.
- Produces:
  - `AtomicPropertyList { objects: Vec<u32>, count_props: Vec<u32>, props: Vec<u32>, values: Vec<u64> }` with `AtomicPropertyList::validate(&self) -> Result<(), ProtocolError>`.
  - `OutFenceSlot { crtc_id: u32, value_index: u32 }` — `value_index` indexes `AtomicPropertyList::values`, the entry the helper overwrites with its own holder address.
  - `MAX_ATOMIC_OBJECTS: usize = 256`, `MAX_ATOMIC_PROPS: usize = 1024`, `MAX_REQUEST_FRAME_LEN: usize = 32 * 1024`.
  - `encode_request(&HostCallRequest) -> Vec<u8>`, `decode_request(&[u8]) -> Result<HostCallRequest, ProtocolError>`.
  - `ReplyCorrelation { seq, incarnation, lifecycle_epoch, transition, commit, event_token }`, echoed verbatim in **every** reply.
  - `HostCallReply::Accepted { correlation, helper_duration_ns, out_fence_present: u32 }` — a bitmap over `out_fence_slots` — and `HostCallReply::Rejected { correlation, errno, helper_duration_ns, unexpected_fence_output: bool }`.

`RequestSeq` alone is not enough. `ID-3` says "a reply is current only when incarnation, lifecycle epoch, optional transition id, and commit id all match", and `COMMIT-6` requires a late success whose lifecycle tag is stale to remain **accepted** rather than be mistaken for a rejection. A per-socket sequence number cannot distinguish those cases, and task 11's clock probe validates identities that a seq-only reply does not carry.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn property_list_round_trips_with_out_fence_slots() {
    let request = AtomicRequest {
        properties: AtomicPropertyList {
            objects: vec![31, 42],
            count_props: vec![2, 1],
            props: vec![7, 8, 9],
            values: vec![0x1111, 0x2222, 0],
        },
        out_fence_slots: vec![OutFenceSlot { crtc_id: 42, value_index: 2 }],
        ..atomic_request_shell_for_tests()
    };
    let frame = encode_request(&HostCallRequest::Atomic(request.clone()));
    assert!(frame.len() <= MAX_REQUEST_FRAME_LEN);
    assert_eq!(decode_request(&frame).expect("decode"), HostCallRequest::Atomic(request));
}

#[test]
fn property_list_counts_must_agree() {
    // sum(count_props) must equal props.len() and values.len(), and
    // objects.len() must equal count_props.len(). A helper that trusted a
    // mismatched list would hand the kernel a short array.
    let bad = AtomicPropertyList {
        objects: vec![31, 42],
        count_props: vec![2],
        props: vec![7, 8, 9],
        values: vec![1, 2, 3],
    };
    assert_eq!(bad.validate(), Err(ProtocolError::Field("count_props length")));

    let bad = AtomicPropertyList {
        objects: vec![31],
        count_props: vec![2],
        props: vec![7, 8, 9],
        values: vec![1, 2, 3],
    };
    assert_eq!(bad.validate(), Err(ProtocolError::Field("prop count sum")));

    let bad = AtomicPropertyList {
        objects: vec![31],
        count_props: vec![3],
        props: vec![7, 8, 9],
        values: vec![1, 2],
    };
    assert_eq!(bad.validate(), Err(ProtocolError::Field("value count")));
}

#[test]
fn oversized_property_lists_are_rejected_before_the_wire() {
    let request = AtomicRequest {
        properties: AtomicPropertyList {
            objects: vec![1],
            count_props: vec![(MAX_ATOMIC_PROPS + 1) as u32],
            props: vec![1; MAX_ATOMIC_PROPS + 1],
            values: vec![0; MAX_ATOMIC_PROPS + 1],
        },
        ..atomic_request_shell_for_tests()
    };
    assert_eq!(
        request.properties.validate(),
        Err(ProtocolError::Field("prop count limit"))
    );
}

#[test]
fn out_fence_slot_index_must_be_inside_the_value_array() {
    let request = AtomicRequest {
        properties: AtomicPropertyList {
            objects: vec![42],
            count_props: vec![1],
            props: vec![9],
            values: vec![0],
        },
        out_fence_slots: vec![OutFenceSlot { crtc_id: 42, value_index: 1 }],
        ..atomic_request_shell_for_tests()
    };
    let frame = encode_request(&HostCallRequest::Atomic(request));
    assert_eq!(decode_request(&frame), Err(ProtocolError::Field("out fence slot index")));
}

#[test]
fn truncated_payload_is_a_length_error_not_a_short_read() {
    let request = AtomicRequest {
        properties: AtomicPropertyList {
            objects: vec![31],
            count_props: vec![1],
            props: vec![7],
            values: vec![5],
        },
        ..atomic_request_shell_for_tests()
    };
    let frame = encode_request(&HostCallRequest::Atomic(request));
    for cut in 1..frame.len() {
        assert!(
            decode_request(&frame[..cut]).is_err(),
            "truncation at {cut} decoded"
        );
    }
}

#[test]
fn accepted_reply_round_trips_the_out_fence_presence_bitmap() {
    let reply = HostCallReply::Accepted {
        seq: RequestSeq::for_tests(3),
        helper_duration_ns: 1234,
        out_fence_present: 0b101,
    };
    assert_eq!(decode_reply(&encode_reply(&reply)).expect("decode"), reply);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::executor::protocol`
Expected: FAIL — `AtomicPropertyList`, `OutFenceSlot` and the new reply shape do not exist.

- [ ] **Step 3: Write the implementation**

In `protocol.rs`:

```rust
pub(crate) const MAX_ATOMIC_OBJECTS: usize = 256;
pub(crate) const MAX_ATOMIC_PROPS: usize = 1024;
pub(crate) const MAX_REQUEST_FRAME_LEN: usize = 32 * 1024;

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(crate) struct AtomicPropertyList {
    pub(crate) objects: Vec<u32>,
    pub(crate) count_props: Vec<u32>,
    pub(crate) props: Vec<u32>,
    pub(crate) values: Vec<u64>,
}

impl AtomicPropertyList {
    pub(crate) fn validate(&self) -> Result<(), ProtocolError> {
        if self.objects.len() != self.count_props.len() {
            return Err(ProtocolError::Field("count_props length"));
        }
        if self.objects.len() > MAX_ATOMIC_OBJECTS {
            return Err(ProtocolError::Field("object count limit"));
        }
        if self.props.len() > MAX_ATOMIC_PROPS {
            return Err(ProtocolError::Field("prop count limit"));
        }
        let mut sum: usize = 0;
        for count in &self.count_props {
            sum = sum
                .checked_add(*count as usize)
                .ok_or(ProtocolError::Field("prop count sum"))?;
        }
        if sum > MAX_ATOMIC_PROPS {
            return Err(ProtocolError::Field("prop count limit"));
        }
        if sum != self.props.len() {
            return Err(ProtocolError::Field("prop count sum"));
        }
        if self.values.len() != self.props.len() {
            return Err(ProtocolError::Field("value count"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct OutFenceSlot {
    pub(crate) crtc_id: u32,
    /// Index into `AtomicPropertyList::values`. The helper replaces this entry
    /// with the address of its own holder storage immediately before the
    /// ioctl; the value the owner encoded there is ignored.
    pub(crate) value_index: u32,
}
```

`AtomicRequest` gains `properties: AtomicPropertyList` and `out_fence_slots: Vec<OutFenceSlot>` and loses `payload_len` (the frame header's `payload_len` is now authoritative). `HostCallRequest` and `AtomicRequest` become `Clone` rather than `Copy`.

Encoding layout for `KIND_ATOMIC_REQUEST`, all little-endian:

```text
header (12) : magic | version | kind | payload_len
head   (68) : seq u64            @0    incarnation u64     @8
              lifecycle_epoch u64 @16   transition u64      @24
              commit u64          @32   event_token u64     @40
              transition_present u8 @48 class u8            @49
              pad u16             @50   flags u32           @52
              object_count u32    @56   prop_count u32      @60
              slot_count u32      @64
body @80    : objects[object_count] u32
            | count_props[object_count] u32
            | props[prop_count] u32
            | values[prop_count] u64
            | slots[slot_count] { crtc_id u32, value_index u32 }
```

Six `u64` are 48 bytes, the presence/class/pad group is 4, and four `u32` are 16: the head is **68** bytes, so with the 12-byte envelope the body starts at byte **80**. Declare `const REQUEST_HEAD_LEN: usize = 68;` with a `const` assertion against the sum of its field sizes, use checked cursor arithmetic, and add a golden-byte test that independently asserts the body offset and total frame length — an encoder and decoder sharing one wrong offset would pass a round-trip test.

`encode_request` calls `properties.validate()` and panics on a violation — the owner must never construct an invalid list, and a panic in the parent is preferable to sending a short array to a helper that will hand it to the kernel. `decode_request` re-runs `validate()` on the decoded list, checks `payload_len` matches the exact computed body length, checks `slot_count <= MAX_OUT_FENCES`, and checks every `value_index < values.len()` returning `ProtocolError::Field("out fence slot index")`.

`transport.rs`: replace the fixed `REQUEST_FRAME_LEN` receive buffer with `recv_frame` into a `[u8; MAX_REQUEST_FRAME_LEN]`; `ReceivedFrame::len` already reports the real length so the helper slices `&buf[..len]`. Add a `send_frame` guard that returns `io::ErrorKind::InvalidInput` for a frame above `MAX_REQUEST_FRAME_LEN`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::executor`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/protocol.rs crates/yserver/src/kms/executor/transport.rs
git commit -m "feat(kms): carry a bounded atomic property payload over the executor wire"
```

---

### Task 3 `[r2]`: Helper-side property materialization and `OUT_FENCE_PTR` holder ownership

`§10.2`: "The executor owns stable `OUT_FENCE_PTR` holder memory until the ioctl has returned and transfers one terminal reply plus every resulting fd in one message-boundary-preserving IPC operation." The holder must live in the helper's address space — an owner-side pointer is meaningless across processes.

**Files:**
- Modify: `crates/yserver/src/kms/executor/helper.rs`
- Test: `crates/yserver/tests/device_owner.rs` (new file; spawns a real helper)

**Interfaces:**
- Consumes: `AtomicRequest`, `AtomicPropertyList`, `OutFenceSlot` from task 2.
- Produces: helper behaviour only. `KmsIoExecutor::dispatch` keeps its shape but takes `&HostCallRequest` by reference with an owned payload inside.

- [ ] **Step 1: Write the failing test**

In `crates/yserver/tests/device_owner.rs`:

```rust
//! Integration tests that drive a real re-exec helper process with real
//! property payloads. They never require a KMS device: an invalid object id
//! makes the kernel reject the request, which is exactly the reject path
//! these tests pin.

mod common;

use yserver::kms::executor::test_support::{spawn_test_executor, TestDevice};

#[test]
fn helper_submits_the_property_list_and_reports_the_kernel_errno() {
    let device = TestDevice::open_any_drm_or_skip();
    let mut executor = spawn_test_executor(&device);

    // Object id 0 is never a valid DRM object: the kernel must reject with
    // ENOENT/EINVAL rather than the helper silently submitting count_objs=0.
    let outcome = executor.dispatch_atomic_for_tests(
        AtomicPropertyList {
            objects: vec![0],
            count_props: vec![1],
            props: vec![0],
            values: vec![0],
        },
        &[],
    );
    match outcome {
        HostCallOutcome::Rejected { errno, .. } => {
            assert!(errno != 0, "a rejected commit must carry a real errno");
        }
        other => panic!("expected an explicit rejection, got {other:?}"),
    }
}

#[test]
fn a_rejected_commit_returns_no_out_fence_and_reports_no_unexpected_output() {
    let device = TestDevice::open_any_drm_or_skip();
    let mut executor = spawn_test_executor(&device);
    let outcome = executor.dispatch_atomic_for_tests(
        AtomicPropertyList {
            objects: vec![0],
            count_props: vec![1],
            props: vec![0],
            values: vec![0],
        },
        &[OutFenceSlot { crtc_id: 0, value_index: 0 }],
    );
    match outcome {
        HostCallOutcome::Rejected { unexpected_fence_output, .. } => {
            assert!(!unexpected_fence_output, "kernel wrote a fence into a rejected commit");
        }
        other => panic!("expected rejection, got {other:?}"),
    }
}

#[test]
fn the_helper_patches_every_out_fence_slot_with_its_own_holder_address() {
    // The helper must not submit the owner's placeholder value. Pin it by
    // asking the helper to echo the patched value array back in a debug reply.
    let device = TestDevice::open_any_drm_or_skip();
    let mut executor = spawn_test_executor(&device);
    let echoed = executor.echo_patched_values_for_tests(
        AtomicPropertyList {
            objects: vec![0],
            count_props: vec![2],
            props: vec![0, 1],
            values: vec![0xdead_beef, 0],
        },
        &[OutFenceSlot { crtc_id: 0, value_index: 1 }],
    );
    assert_eq!(echoed[0], 0xdead_beef, "untouched entries must survive verbatim");
    // Not merely "nonzero": any constant would pass that. The patched value
    // must be the address of the holder that is live at ioctl time, which is
    // what a later reallocation of `holders` would break.
    assert_eq!(
        echoed[1],
        executor.holder_addresses_for_tests()[0],
        "the slot must carry the address of the live holder, not a placeholder"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p yserver --test device_owner`
Expected: FAIL — `dispatch_atomic_for_tests` and `echo_patched_values_for_tests` do not exist and the helper still submits an empty request.

- [ ] **Step 3: Write the implementation**

Replace `execute_host_call`'s atomic arm in `helper.rs`:

```rust
fn execute_atomic(
    kms_fd: BorrowedFd<'_>,
    atomic: &AtomicRequest,
) -> (HostCallReply, Vec<OwnedFd>) {
    // Local, stable copies. The kernel reads the arrays and writes the
    // holders; both must outlive the ioctl and neither may be reallocated
    // while the pointers are live.
    let objects = atomic.properties.objects.clone();
    let count_props = atomic.properties.count_props.clone();
    let props = atomic.properties.props.clone();
    let mut values = atomic.properties.values.clone();
    let mut holders: Vec<i32> = vec![-1; atomic.out_fence_slots.len()];

    for (slot_idx, slot) in atomic.out_fence_slots.iter().enumerate() {
        let holder: *mut i32 = &mut holders[slot_idx];
        values[slot.value_index as usize] = holder as usize as u64;
    }

    let mut req = DrmModeAtomic {
        flags: atomic.flags,
        count_objs: objects.len() as u32,
        objs_ptr: objects.as_ptr() as usize as u64,
        count_props_ptr: count_props.as_ptr() as usize as u64,
        props_ptr: props.as_ptr() as usize as u64,
        prop_values_ptr: values.as_ptr() as usize as u64,
        reserved: 0,
        user_data: atomic.event_token.as_user_data(),
    };

    let started = Instant::now();
    // SAFETY: every pointer above refers to a live local allocation that is
    // not moved or reallocated until after the ioctl returns, and the counts
    // were validated by `AtomicPropertyList::validate` on decode.
    let rc = unsafe {
        libc::ioctl(
            kms_fd.as_raw_fd(),
            DRM_IOCTL_MODE_ATOMIC,
            std::ptr::addr_of_mut!(req),
        )
    };
    let helper_duration_ns = elapsed_ns(started);

    if rc == 0 {
        let mut fences = Vec::with_capacity(holders.len());
        let mut present: u32 = 0;
        for (slot_idx, raw) in holders.iter().copied().enumerate() {
            if raw >= 0 {
                present |= 1u32 << slot_idx;
                // SAFETY: the kernel wrote a freshly allocated sync-file fd
                // that this process now owns exactly once.
                fences.push(unsafe { OwnedFd::from_raw_fd(raw) });
            }
        }
        // A live success with a `-1` holder is NOT repaired here. The reply
        // reports the exact bitmap and the owner classifies the gap as
        // `CompletionUnknown` (spec 10, "Live success plus -1").
        (
            HostCallReply::Accepted {
                seq: atomic.seq,
                helper_duration_ns,
                out_fence_present: present,
            },
            fences,
        )
    } else {
        let errno = io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO);
        let mut unexpected_fence_output = false;
        for raw in holders.iter().copied() {
            if raw >= 0 {
                unexpected_fence_output = true;
                // SAFETY: an unexpected non-negative output is still a fd this
                // process owns; close it exactly once (spec 10.2).
                unsafe { libc::close(raw) };
            }
        }
        (
            HostCallReply::Rejected {
                seq: atomic.seq,
                errno,
                helper_duration_ns,
                unexpected_fence_output,
            },
            Vec::new(),
        )
    }
}
```

`HostCallOutcome::Accepted` gains `out_fence_present: u32` alongside its `out_fences: Vec<OwnedFd>` so the owner can map each adopted fd back to its slot, and `HostCallOutcome::Rejected` gains `unexpected_fence_output: bool`.

`KmsIoExecutor` validates the bitmap in two steps, in this order:

```rust
let valid = if slot_count == 0 { 0u32 } else { u32::MAX >> (32 - slot_count) };
if present & !valid != 0 {
    return HostCallOutcome::Unknown(UnknownReason::MalformedReply);
}
if fds.len() as u32 != present.count_ones() {
    return HostCallOutcome::Unknown(UnknownReason::MalformedReply);
}
```

The count check alone is insufficient: a reply declaring zero slots could set bit 31 and carry one fd, pass the count, and leave the owner treating an empty expected set as complete while an adopted descriptor is dropped. Test zero-slot-with-a-high-bit and one-slot-with-bit-31 explicitly.

The reply's `ReplyCorrelation` is checked against the in-flight request's before any of this; a mismatch is `MalformedReply` and never a rejection.

Add to `kms/executor/test_support.rs`:

```rust
/// A deterministic ioctl target for the helper tests.
///
/// Rust's test harness has no runtime skip: printing a message and returning
/// early reports a PASS for a test that exercised nothing, which is how the
/// only real coverage of property materialization would disappear silently in
/// CI. So the default target is a stub the helper can always open, and the
/// hardware path is a separately reported `#[ignore]` test.
pub(crate) struct TestDevice {
    fd: OwnedFd,
    kind: TestDeviceKind,
}

pub(crate) enum TestDeviceKind {
    /// Always available: the helper's stub mode answers with a scripted errno
    /// and, when asked, scripted holder writes.
    Stub,
    /// A real `/dev/dri/cardN`. Used only by `#[ignore]`d hardware tests.
    RealDrm,
}

impl TestDevice {
    /// Never skips. Fails loudly if even the stub cannot be created.
    pub(crate) fn open_any_drm_or_fail() -> Self { /* stub by default */ }
    pub(crate) fn open_real_drm_or_ignore() -> Option<Self> { /* card0..card3 */ }
}
```

and a `KmsIoExecutor::dispatch_atomic_for_tests(&mut self, properties, slots) -> HostCallOutcome` plus, behind `#[cfg(feature = "executor-echo-debug")] `— no. Use a dedicated request kind instead: add `KIND_ECHO_REQUEST: u16 = 4` guarded by `#[cfg(debug_assertions)]` on both sides, so a release helper cannot be asked to echo. The echo reply returns the patched `values` array and performs no ioctl.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver --test device_owner`
Expected: PASS (or SKIP on a host with no DRM node — the skip message must name the reason).

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/helper.rs crates/yserver/src/kms/executor/mod.rs \
        crates/yserver/src/kms/executor/test_support.rs crates/yserver/tests/device_owner.rs
git commit -m "feat(kms): materialize atomic property arrays and out-fence holders in the helper"
```

---

### Task 4 `[r2]`: The asynchronous host-call API

This is the keystone of revision 2. Stage 1 built `dispatch` as a blocking poll
loop; revision 1 of this plan called it from the owner and therefore from live
render paths. `COMMIT-5` forbids that. The blocking form is not deleted — it is
still the correct call at a cold-start or final-offline boundary — but it is
renamed so no seat-active caller reaches it by accident.

**Files:**
- Modify: `crates/yserver/src/kms/executor/mod.rs:293-400`
- Test: `crates/yserver/tests/device_owner.rs`

**Interfaces:**
- Consumes: `HostCallRequest`, `SubmittingProof`, `HostCallOutcome`, `HostCallClass` (tasks 1–3).
- Produces:
  - `InFlightHostCall` — non-`Clone`, non-`Copy`, one per executor.
  - `KmsIoExecutor::{send, control_fd, poll_reply, check_watchdog, dispatch_blocking_at_permitted_boundary}`
  - `SendError::{HelperExited, Ipc, AlreadyInFlight}`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn send_returns_before_the_helper_replies() {
    let device = TestDevice::open_any_drm_or_fail();
    let mut executor = spawn_slow_helper_for_tests(&device, Duration::from_millis(400));
    let started = Instant::now();
    let in_flight = executor
        .send(&atomic_request_for_tests(), SubmittingProof::for_tests())
        .expect("send");
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "send must not wait for the reply; took {:?}",
        started.elapsed()
    );
    assert!(executor.poll_reply(&in_flight).is_none(), "no reply can have arrived yet");
}

#[test]
fn the_core_stays_responsive_while_a_host_call_is_unresolved() {
    // COMMIT-5. The whole point of the executor. This is the test the plan's
    // revision 1 could not have passed.
    let device = TestDevice::open_any_drm_or_fail();
    let mut executor = spawn_slow_helper_for_tests(&device, Duration::from_millis(300));
    let in_flight = executor.send(&atomic_request_for_tests(), SubmittingProof::for_tests()).expect("send");

    let mut core_iterations = 0u32;
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        // Stand-in for the core's ordinary work. It must run freely.
        core_iterations += 1;
        assert!(executor.poll_reply(&in_flight).is_none());
    }
    assert!(core_iterations > 1000, "the core ran only {core_iterations} times");
}

#[test]
fn poll_reply_returns_none_rather_than_blocking_when_nothing_arrived() {
    let device = TestDevice::open_any_drm_or_fail();
    let mut executor = spawn_slow_helper_for_tests(&device, Duration::from_millis(500));
    let in_flight = executor.send(&atomic_request_for_tests(), SubmittingProof::for_tests()).expect("send");
    for _ in 0..100 {
        let started = Instant::now();
        assert!(executor.poll_reply(&in_flight).is_none());
        assert!(started.elapsed() < Duration::from_millis(5), "poll_reply blocked");
    }
}

#[test]
fn a_readable_control_fd_yields_the_outcome() {
    let device = TestDevice::open_any_drm_or_fail();
    let mut executor = spawn_test_executor(&device);
    let in_flight = executor.send(&rejecting_request_for_tests(), SubmittingProof::for_tests()).expect("send");
    wait_readable_for_tests(executor.control_fd(), Duration::from_secs(2));
    match executor.poll_reply(&in_flight) {
        Some(HostCallOutcome::Rejected { errno, .. }) => assert!(errno != 0),
        other => panic!("expected a rejection, got {other:?}"),
    }
}

#[test]
fn the_watchdog_is_a_deadline_check_not_a_wait() {
    let device = TestDevice::open_any_drm_or_fail();
    let mut executor = spawn_slow_helper_for_tests(&device, Duration::from_secs(30));
    let in_flight = executor.send(&atomic_request_for_tests(), SubmittingProof::for_tests()).expect("send");
    assert!(executor.check_watchdog(&in_flight, Instant::now()).is_none());
    let started = Instant::now();
    let outcome = executor.check_watchdog(&in_flight, in_flight.deadline_for_tests() + Duration::from_millis(1));
    assert!(
        started.elapsed() < Duration::from_millis(5),
        "check_watchdog must not sleep to reach the deadline"
    );
    assert!(matches!(
        outcome,
        Some(HostCallOutcome::Unknown(UnknownReason::WatchdogExpired))
    ));
}

#[test]
fn the_watchdog_deadline_matches_the_declared_class() {
    let device = TestDevice::open_any_drm_or_fail();
    let mut executor = spawn_test_executor(&device);
    for (class, expected) in [
        (HostCallClass::SeatActiveNonblock, Duration::from_secs(2)),
        (HostCallClass::SeatActiveValidation, Duration::from_secs(2)),
        (HostCallClass::ColdStartOrOfflineBlocking, Duration::from_secs(30)),
    ] {
        let in_flight = executor
            .send(&request_with_class_for_tests(class), SubmittingProof::for_tests())
            .expect("send");
        assert_eq!(in_flight.watchdog_for_tests(), expected);
        executor.abandon_for_tests(in_flight);
    }
}

#[test]
fn only_one_host_call_may_be_in_flight_at_a_time() {
    // This is what serializes host calls now that send returns immediately.
    let device = TestDevice::open_any_drm_or_fail();
    let mut executor = spawn_slow_helper_for_tests(&device, Duration::from_millis(300));
    let _first = executor.send(&atomic_request_for_tests(), SubmittingProof::for_tests()).expect("first");
    assert_eq!(
        executor
            .send(&atomic_request_for_tests(), SubmittingProof::for_tests())
            .unwrap_err(),
        SendError::AlreadyInFlight
    );
}

#[test]
fn helper_death_while_in_flight_is_unknown_not_rejection() {
    let device = TestDevice::open_any_drm_or_fail();
    let mut executor = spawn_slow_helper_for_tests(&device, Duration::from_secs(10));
    let in_flight = executor.send(&atomic_request_for_tests(), SubmittingProof::for_tests()).expect("send");
    executor.kill_helper_for_tests();
    wait_readable_for_tests(executor.control_fd(), Duration::from_secs(2));
    assert!(matches!(
        executor.poll_reply(&in_flight),
        Some(HostCallOutcome::Unknown(UnknownReason::HelperExited))
    ));
}

#[test]
fn the_blocking_form_is_reachable_only_by_its_explicit_name() {
    // COMMIT-5 permits blocking solely at cold start or final offline. The name
    // is the enforcement; assert no other executor entry point blocks.
    let src = include_str!("../src/kms/executor/mod.rs");
    assert_eq!(src.matches("libc::poll").count(), 1, "exactly one polling site");
    let blocking = &src[src.find("fn dispatch_blocking_at_permitted_boundary").expect("named fn")..];
    assert!(blocking[..blocking.find("\n    pub").unwrap_or(blocking.len())].contains("libc::poll"));
    assert!(!src.contains("std::thread::sleep"), "no sleep may remain on any host-call path");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver --test device_owner`
Expected: FAIL — `send`, `poll_reply`, `check_watchdog` and `control_fd` do not exist, and `dispatch` still blocks.

- [ ] **Step 3: Write the implementation**

Split the existing loop. `send` performs the encode and the `send_frame`, computes the deadline, and stores `Some(seq)` in a new `in_flight: Option<RequestSeq>` field so a second send is refused:

```rust
pub(crate) fn send(
    &mut self,
    request: &HostCallRequest,
    _proof: SubmittingProof,
) -> Result<InFlightHostCall, SendError> {
    if self.in_flight.is_some() {
        return Err(SendError::AlreadyInFlight);
    }
    let class = HostCallClass::from_request(request);
    let started = Instant::now();
    let deadline = started
        .checked_add(class.watchdog())
        .ok_or(SendError::Ipc)?;
    let frame = encode_request(request);
    if send_frame(&self.control, &frame).is_err() {
        return Err(if self.check_child_exited() {
            SendError::HelperExited
        } else {
            SendError::Ipc
        });
    }
    let seq = request.seq();
    self.in_flight = Some(seq);
    Ok(InFlightHostCall { seq, class, started, deadline })
}
```

`poll_reply` does one non-blocking `recv_frame` on a socket the constructor now
sets `O_NONBLOCK` on. `WouldBlock` returns `None`; EOF returns
`HelperExited` if `check_child_exited()`, otherwise `IpcFailure` — the
100 ms sleep loop is deleted, because the parent must never sleep to decide
whether a child died. It clears `self.in_flight` on every terminal outcome and
computes `round_trip_ns` from `in_flight.started`.

`check_watchdog(&in_flight, now)` compares `now >= in_flight.deadline`; on
expiry it sets `ExecutorState::Stalled`, calls `request_termination`, clears
`in_flight` and returns `Unknown(WatchdogExpired)`. It performs no I/O.

`control_fd` returns `self.control.as_fd()` for event-loop registration.

`dispatch_blocking_at_permitted_boundary` is the old body verbatim except for
its name, minus the `std::thread::sleep` EOF loop, which is replaced by a single
`check_child_exited()`. It keeps the only `libc::poll` in the module.

The caller contract, stated once here so tasks 7–23 inherit it: the core
registers `control_fd()` for readability and calls the owner's
`on_control_readable()`; the core's existing timer tick calls the owner's
`tick(now)`. Nothing on the seat-active path waits.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver --test device_owner` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/mod.rs crates/yserver/tests/device_owner.rs
git commit -m "feat(kms): split the executor host call into send, poll and watchdog"
```

---

### Task 5 `[r2]`: The owner request builder, atomic CRTC closure and the off-to-off signaling rule

This is spec test 53 and the construction half of `§6.3`. It must exist before any call site is converted, because the conversion's whole point is that requests stop being hand-assembled `AtomicModeReq` values with no closure knowledge.

**Files:**
- Create: `crates/yserver/src/kms/owner/request.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: `AtomicPropertyList`, `OutFenceSlot`, `ProtocolError` from task 2.
- Produces:
  - `AtomicRequestBuilder::{new, add_crtc_property, add_connector_property, add_plane_property, declare_crtc_active, atomic_crtc_closure, expected_completion_crtcs, finish}`
  - `Signaling { page_flip_event: bool }`
  - `SerializedRequest { properties, out_fence_slots, closure, expected_completion, kernel_event_crtcs, present_event_crtcs, flags }`
  - `RequestError::{OffToOffSignaled, MissingActiveDeclaration, MissingOutFenceProp, ClosureMutated, Payload}`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const CRTC_A: u32 = 40;
    const CRTC_B: u32 = 41;
    const CONN_A: u32 = 60;
    const PLANE_A: u32 = 80;
    const PROP_ACTIVE: u32 = 1;
    const PROP_CRTC_ID: u32 = 2;
    const PROP_FB_ID: u32 = 3;
    const PROP_OUT_FENCE: u32 = 4;

    fn out_fence_props() -> HashMap<u32, u32> {
        HashMap::from([(CRTC_A, PROP_OUT_FENCE), (CRTC_B, PROP_OUT_FENCE)])
    }

    #[test]
    fn a_crtc_property_pulls_its_crtc_into_the_closure() {
        let mut b = AtomicRequestBuilder::new();
        b.add_crtc_property(CRTC_A, PROP_ACTIVE, 1);
        b.declare_crtc_active(CRTC_A, false, true);
        assert_eq!(b.atomic_crtc_closure(), BTreeSet::from([CRTC_A]));
        assert_eq!(b.expected_completion_crtcs(), BTreeSet::from([CRTC_A]));
    }

    #[test]
    fn a_plane_move_pulls_both_powered_endpoints_into_the_closure() {
        let mut b = AtomicRequestBuilder::new();
        b.add_plane_property(PLANE_A, PROP_CRTC_ID, u64::from(CRTC_B), Some(CRTC_A), Some(CRTC_B));
        b.declare_crtc_active(CRTC_A, true, true);
        b.declare_crtc_active(CRTC_B, true, true);
        assert_eq!(b.atomic_crtc_closure(), BTreeSet::from([CRTC_A, CRTC_B]));
        assert_eq!(b.expected_completion_crtcs(), BTreeSet::from([CRTC_A, CRTC_B]));
    }

    #[test]
    fn detach_retains_the_old_endpoint() {
        let mut b = AtomicRequestBuilder::new();
        b.add_plane_property(PLANE_A, PROP_CRTC_ID, 0, Some(CRTC_A), None);
        b.declare_crtc_active(CRTC_A, true, true);
        assert_eq!(b.atomic_crtc_closure(), BTreeSet::from([CRTC_A]));
    }

    #[test]
    fn a_disable_still_produces_completion_evidence() {
        // old.active is enough: the expected set is never empty merely because
        // new.active is false.
        let mut b = AtomicRequestBuilder::new();
        b.add_crtc_property(CRTC_A, PROP_ACTIVE, 0);
        b.declare_crtc_active(CRTC_A, true, false);
        assert_eq!(b.expected_completion_crtcs(), BTreeSet::from([CRTC_A]));
        let req = b
            .finish(Signaling { page_flip_event: false }, &out_fence_props())
            .expect("disable must serialize");
        assert_eq!(req.out_fence_slots.len(), 1);
    }

    #[test]
    fn inactive_to_inactive_may_be_empty_and_takes_no_fence() {
        let mut b = AtomicRequestBuilder::new();
        b.add_crtc_property(CRTC_A, PROP_ACTIVE, 0);
        b.declare_crtc_active(CRTC_A, false, false);
        assert_eq!(b.atomic_crtc_closure(), BTreeSet::from([CRTC_A]));
        assert!(b.expected_completion_crtcs().is_empty());
        let req = b
            .finish(Signaling { page_flip_event: false }, &out_fence_props())
            .expect("off-to-off with no signaling source is legal");
        assert!(req.out_fence_slots.is_empty());
    }

    #[test]
    fn an_off_to_off_closure_member_rejects_the_global_page_event() {
        let mut b = AtomicRequestBuilder::new();
        b.add_crtc_property(CRTC_A, PROP_ACTIVE, 1);
        b.declare_crtc_active(CRTC_A, true, true);
        b.add_crtc_property(CRTC_B, PROP_ACTIVE, 0);
        b.declare_crtc_active(CRTC_B, false, false);
        assert_eq!(
            b.finish(Signaling { page_flip_event: true }, &out_fence_props()),
            Err(RequestError::OffToOffSignaled(CRTC_B))
        );
    }

    #[test]
    fn an_out_fence_is_never_added_for_symmetry_outside_the_expected_set() {
        let mut b = AtomicRequestBuilder::new();
        b.add_crtc_property(CRTC_A, PROP_ACTIVE, 1);
        b.declare_crtc_active(CRTC_A, true, true);
        b.add_crtc_property(CRTC_B, PROP_ACTIVE, 0);
        b.declare_crtc_active(CRTC_B, false, false);
        let req = b
            .finish(Signaling { page_flip_event: false }, &out_fence_props())
            .expect("serialize");
        assert_eq!(
            req.out_fence_slots.iter().map(|s| s.crtc_id).collect::<Vec<_>>(),
            vec![CRTC_A]
        );
    }

    #[test]
    fn ephemeral_out_fence_entries_cannot_enlarge_the_closure() {
        let mut b = AtomicRequestBuilder::new();
        b.add_plane_property(PLANE_A, PROP_FB_ID, 7, Some(CRTC_A), Some(CRTC_A));
        b.declare_crtc_active(CRTC_A, true, true);
        let before = b.atomic_crtc_closure();
        let req = b
            .finish(Signaling { page_flip_event: true }, &out_fence_props())
            .expect("serialize");
        assert_eq!(req.closure, before);
    }

    #[test]
    fn the_final_rescan_catches_a_mutated_serialized_binding() {
        // Mutate the REAL payload, not a synthetic override: corrupting a
        // recorded field would pass even against a re-scan that reads its own
        // metadata back, which is what revision 1's test did.
        let mut b = AtomicRequestBuilder::new();
        b.add_plane_property(PLANE_A, PROP_CRTC_ID, u64::from(CRTC_A), Some(CRTC_A), Some(CRTC_A));
        b.declare_crtc_active(CRTC_A, true, true);
        b.declare_crtc_active(CRTC_B, true, true);
        b.mutate_serialized_value_for_tests(PLANE_A, PROP_CRTC_ID, u64::from(CRTC_B));
        assert_eq!(
            b.finish(Signaling { page_flip_event: false }, &out_fence_props()),
            Err(RequestError::ClosureMutated)
        );
    }

    #[test]
    fn replacing_a_property_leaves_no_obsolete_binding_behind() {
        let mut b = AtomicRequestBuilder::new();
        b.add_plane_property(PLANE_A, PROP_CRTC_ID, u64::from(CRTC_A), Some(CRTC_A), Some(CRTC_A));
        b.add_plane_property(PLANE_A, PROP_CRTC_ID, 0, Some(CRTC_A), None);
        b.declare_crtc_active(CRTC_A, true, true);
        assert_eq!(
            b.atomic_crtc_closure(),
            BTreeSet::from([CRTC_A]),
            "the replaced binding must not leave CRTC_A's successor in the closure"
        );
    }

    #[test]
    fn a_closure_member_without_an_active_declaration_fails_construction() {
        let mut b = AtomicRequestBuilder::new();
        b.add_crtc_property(CRTC_A, PROP_ACTIVE, 1);
        assert_eq!(
            b.finish(Signaling { page_flip_event: false }, &out_fence_props()),
            Err(RequestError::MissingActiveDeclaration(CRTC_A))
        );
    }

    #[test]
    fn kernel_event_crtcs_equal_the_expected_set_only_when_the_page_event_is_set() {
        let mut b = AtomicRequestBuilder::new();
        b.add_plane_property(PLANE_A, PROP_FB_ID, 7, Some(CRTC_A), Some(CRTC_A));
        b.declare_crtc_active(CRTC_A, true, true);
        let with_event = b
            .clone()
            .finish(Signaling { page_flip_event: true }, &out_fence_props())
            .expect("serialize");
        assert_eq!(with_event.kernel_event_crtcs, with_event.expected_completion);
        let without = b
            .finish(Signaling { page_flip_event: false }, &out_fence_props())
            .expect("serialize");
        assert!(without.kernel_event_crtcs.is_empty());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::request`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Write the implementation**

```rust
//! Owner-side atomic request construction.
//!
//! Every live C.0 request is built here so the atomic CRTC closure, the
//! expected completion set and the off-to-off signaling rule are computed from
//! the same property list the helper will submit — not from a caller's belief
//! about which CRTCs it touched.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::kms::executor::protocol::{AtomicPropertyList, OutFenceSlot, ProtocolError};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct Signaling {
    pub(crate) page_flip_event: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct ActiveState {
    old: bool,
    new: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub(crate) enum RequestError {
    #[error("CRTC {0} is inactive in both old and new state but carries a signaling source")]
    OffToOffSignaled(u32),
    #[error("CRTC {0} is in the atomic closure with no declared active state")]
    MissingActiveDeclaration(u32),
    #[error("CRTC {0} needs an OUT_FENCE_PTR property id and none is known")]
    MissingOutFenceProp(u32),
    #[error("the serialized request's CRTC closure differs from the recorded set")]
    ClosureMutated,
    #[error("serialized property list is invalid: {0:?}")]
    Payload(ProtocolError),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AtomicRequestBuilder {
    /// Persistent properties only, ordered so serialization is deterministic.
    entries: BTreeMap<u32, BTreeMap<u32, u64>>,
    crtc_objects: BTreeSet<u32>,
    /// Old/new CRTC bindings contributed by connector and plane entries.
    bindings: Vec<(Option<u32>, Option<u32>)>,
    active: BTreeMap<u32, ActiveState>,
    /// Set by `corrupt_recorded_closure_for_tests` only.
    recorded_closure_override: Option<BTreeSet<u32>>,
    present_event_crtcs: BTreeSet<u32>,
}

impl AtomicRequestBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn add_crtc_property(&mut self, crtc: u32, prop: u32, value: u64) {
        self.crtc_objects.insert(crtc);
        self.entries.entry(crtc).or_default().insert(prop, value);
    }

    pub(crate) fn add_connector_property(
        &mut self,
        connector: u32,
        prop: u32,
        value: u64,
        old_crtc: Option<u32>,
        new_crtc: Option<u32>,
    ) {
        self.entries.entry(connector).or_default().insert(prop, value);
        self.bindings.push((old_crtc, new_crtc));
    }

    pub(crate) fn add_plane_property(
        &mut self,
        plane: u32,
        prop: u32,
        value: u64,
        old_crtc: Option<u32>,
        new_crtc: Option<u32>,
    ) {
        self.entries.entry(plane).or_default().insert(prop, value);
        self.bindings.push((old_crtc, new_crtc));
    }

    pub(crate) fn declare_crtc_active(&mut self, crtc: u32, old: bool, new: bool) {
        self.active.insert(crtc, ActiveState { old, new });
    }

    /// Mark a CRTC as having a Present consumer, so its page event creates
    /// protocol completion rather than only being drained.
    pub(crate) fn declare_present_consumer(&mut self, crtc: u32) {
        self.present_event_crtcs.insert(crtc);
    }

    pub(crate) fn atomic_crtc_closure(&self) -> BTreeSet<u32> {
        let mut closure = self.crtc_objects.clone();
        for (old, new) in &self.bindings {
            closure.extend(old.filter(|id| *id != 0));
            closure.extend(new.filter(|id| *id != 0));
        }
        closure
    }

    pub(crate) fn expected_completion_crtcs(&self) -> BTreeSet<u32> {
        self.atomic_crtc_closure()
            .into_iter()
            .filter(|crtc| {
                self.active
                    .get(crtc)
                    .is_some_and(|state| state.old || state.new)
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn corrupt_recorded_closure_for_tests(&mut self, closure: BTreeSet<u32>) {
        self.recorded_closure_override = Some(closure);
    }

    pub(crate) fn finish(
        self,
        signaling: Signaling,
        out_fence_props: &HashMap<u32, u32>,
    ) -> Result<SerializedRequest, RequestError> {
        let closure = self.atomic_crtc_closure();
        let recorded = self.recorded_closure_override.clone().unwrap_or_else(|| closure.clone());

        for crtc in &closure {
            if !self.active.contains_key(crtc) {
                return Err(RequestError::MissingActiveDeclaration(*crtc));
            }
        }

        let expected = self.expected_completion_crtcs();

        // Off-to-off: kernel `prepare_signaling()` creates event state for
        // every CRTC in the atomic state when either the global page-event
        // flag is set or that CRTC carries OUT_FENCE_PTR, and the later check
        // rejects it when both old and new are inactive.
        for crtc in &closure {
            let state = self.active[crtc];
            if !state.old && !state.new {
                let would_be_fenced = expected.contains(crtc);
                if signaling.page_flip_event || would_be_fenced {
                    return Err(RequestError::OffToOffSignaled(*crtc));
                }
            }
        }

        // Ephemeral out-fence entries are added only after the closure is
        // fixed, and only for members of the expected set.
        let mut entries = self.entries.clone();
        let mut fenced: Vec<u32> = Vec::with_capacity(expected.len());
        for crtc in &expected {
            let prop = *out_fence_props
                .get(crtc)
                .ok_or(RequestError::MissingOutFenceProp(*crtc))?;
            entries.entry(*crtc).or_default().insert(prop, 0);
            fenced.push(*crtc);
        }

        let (properties, index_of) = serialize(&entries);
        properties.validate().map_err(RequestError::Payload)?;

        let out_fence_slots = fenced
            .iter()
            .map(|crtc| OutFenceSlot {
                crtc_id: *crtc,
                value_index: index_of[&(*crtc, out_fence_props[crtc])],
            })
            .collect();

        // Final re-scan of the serialized request: recompute the closure from
        // what is actually about to be dispatched, ignoring the ephemeral
        // out-fence entries, and refuse a mismatch.
        // Derived from the SERIALIZED payload, not from the metadata the
        // builder accumulated. Passing `self.bindings` back in would make the
        // check tautological: replacing a serialized CRTC_ID value would not
        // change the result, which is precisely the mutation this exists to
        // catch (spec 6.3: "the owner first computes the closure from the final
        // serialized persistent property list").
        let rescanned = rescan_closure(&properties, &self.object_kinds, &self.old_bindings, &crtc_id_prop_ids);
        if rescanned != recorded {
            return Err(RequestError::ClosureMutated);
        }

        let kernel_event_crtcs = if signaling.page_flip_event {
            expected.clone()
        } else {
            BTreeSet::new()
        };
        let present_event_crtcs = kernel_event_crtcs
            .intersection(&self.present_event_crtcs)
            .copied()
            .collect();

        Ok(SerializedRequest {
            properties,
            out_fence_slots,
            closure,
            expected_completion: expected,
            kernel_event_crtcs,
            present_event_crtcs,
            page_flip_event: signaling.page_flip_event,
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct SerializedRequest {
    pub(crate) properties: AtomicPropertyList,
    pub(crate) out_fence_slots: Vec<OutFenceSlot>,
    pub(crate) closure: BTreeSet<u32>,
    pub(crate) expected_completion: BTreeSet<u32>,
    pub(crate) kernel_event_crtcs: BTreeSet<u32>,
    pub(crate) present_event_crtcs: BTreeSet<u32>,
    pub(crate) page_flip_event: bool,
}
```

`serialize` walks the `BTreeMap` in key order producing `objects`, `count_props`, `props`, `values` plus a `HashMap<(u32, u32), u32>` from `(object, prop)` to its `values` index.

`rescan_closure` takes the serialized arrays, the recorded `object_kinds`, the recorded **old** bindings, and the set of property ids that mean `CRTC_ID` per object kind. It recovers each object's **new** binding by reading the serialized `CRTC_ID` value rather than trusting the caller's metadata, unions it with the recorded old binding, and adds every object of kind `Crtc`. Old bindings stay caller-supplied because they describe kernel state that is not in the request; new bindings must come from the payload, because that is the half a mutation can change.

The builder therefore stores `object_kinds: BTreeMap<u32, ObjectKind>` and `old_bindings: BTreeMap<u32, Option<u32>>` keyed by object, replacing revision 1's positional `bindings: Vec<(Option<u32>, Option<u32>)>`, which silently unioned inconsistent metadata supplied for different properties of the same object and left obsolete entries behind when a property was replaced.

`SerializedRequest` also carries the state-affecting flags the builder was given — `allow_modeset: bool` alongside `page_flip_event: bool` — because task 7's `atomic_flags` needs `ALLOW_MODESET` and revision 1 exposed no field or builder method for it. `AtomicRequestBuilder::declare_allow_modeset()` sets it, and the final `TEST_ONLY` in task 19 must carry the same value.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner::request`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/request.rs crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): add the owner atomic request builder and CRTC closure rules"
```

---

### Task 6: Commit records, typed milestones, terminal states and the tombstone ring

**Files:**
- Create: `crates/yserver/src/kms/owner/commit.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: `SerializedRequest` (task 4), `CommitId`, `EventToken`, `IncarnationId`, `LifecycleEpochId`, `LifecycleTransitionId`.
- Produces:
  - `CommitClass::{NonblockingPrimaryPresent, NonblockingNonPresent, BlockingOrdinary, BlockingQualification}`
  - `CommitState::{Submitting, Accepted, Completed, FailedBeforeSubmit, CompletionUnknown}`
  - `Milestones` with independent `bool` fields and `Milestones::completed_for(class) -> bool`
  - `CommitRecord::{new, terminalize, is_terminal}`
  - `TombstoneRing::{new, push, resolve}` with `Resolution::{Live, Tombstoned, Unknown}` and capacity 64.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nonblocking_present_needs_both_hardware_complete_and_presented() {
        let mut m = Milestones::default();
        m.accepted = true;
        m.hardware_complete = true;
        assert!(!m.completed_for(CommitClass::NonblockingPrimaryPresent));
        m.presented = true;
        assert!(m.completed_for(CommitClass::NonblockingPrimaryPresent));
    }

    #[test]
    fn a_nonblocking_non_present_commit_cannot_manufacture_presentation() {
        let mut m = Milestones::default();
        m.accepted = true;
        m.hardware_complete = true;
        assert!(m.completed_for(CommitClass::NonblockingNonPresent));
        assert!(!m.presented, "hardware completion never fabricates Presented");
    }

    #[test]
    fn observing_one_milestone_never_fabricates_another() {
        let mut m = Milestones::default();
        m.presented = true;
        assert!(!m.hardware_complete);
        assert!(!m.accepted);
        assert!(!m.completed_for(CommitClass::NonblockingPrimaryPresent));
    }

    #[test]
    fn a_record_reaches_exactly_one_terminal_state() {
        let mut record = record_for_tests();
        record.terminalize(CommitState::CompletionUnknown);
        assert_eq!(record.state, CommitState::CompletionUnknown);
        record.terminalize(CommitState::Completed);
        assert_eq!(
            record.state,
            CommitState::CompletionUnknown,
            "a terminal record must not be re-terminalized"
        );
    }

    #[test]
    fn a_terminalized_commit_becomes_a_tombstone_that_advances_nothing() {
        let mut owner = accepted_present_owner_for_tests(40);
        let token = owner.pending_token_for_tests();
        owner.complete_pending_for_tests();
        assert!(owner.slot_is_free(), "a terminal record must leave the slot");
        assert!(matches!(owner.resolve_token(token), Resolution::Tombstoned(_)));
        let before = owner.milestone_snapshot_for_tests();
        assert_eq!(
            owner.on_drm_event(page_flip(40, token.as_user_data())),
            EventDisposition::TelemetryOnly(TelemetryReason::Tombstoned)
        );
        assert_eq!(owner.milestone_snapshot_for_tests(), before);
    }

    #[test]
    fn tombstones_are_bounded_and_evict_oldest_first() {
        let mut ring = TombstoneRing::new();
        for raw in 1..=70u64 {
            ring.push(Tombstone::identity_only(EventToken::for_tests(raw), CommitState::Completed));
        }
        assert_eq!(ring.len(), 64);
        // Eviction downgrades a very old duplicate from `Tombstoned` to
        // `Unknown`; both are telemetry-only, so this is safe.
        assert_eq!(ring.resolve(EventToken::for_tests(1)), Resolution::Unknown);
        assert!(matches!(ring.resolve(EventToken::for_tests(70)), Resolution::Tombstoned(_)));
    }

    #[test]
    fn into_tombstone_hands_the_ledger_back_rather_than_dropping_it() {
        // Revision 1 asserted `size_of_val(terminal_state) == 1`, which proves
        // nothing about resource ownership. Count drops instead.
        let counter = DropCounter::new();
        let record = record_with_counted_framebuffer_for_tests(&counter);
        let (tombstone, ledger) = record.into_tombstone();
        assert_eq!(counter.dropped(), 0, "tombstoning must not release resources");
        assert_eq!(tombstone.kernel_event_crtcs, BTreeSet::from([40]));
        drop(ledger.release_new());
        assert_eq!(counter.dropped(), 1, "released exactly once, by the ledger");
    }

    #[test]
    fn a_ledger_transition_consumes_it_so_double_release_cannot_compile() {
        // Compile-fail case, kept in tests/compile_fail alongside stage 1's:
        //   let l = ledger_for_tests();
        //   let _ = l.release_new();
        //   let _ = l.quarantine();   // ERROR: use of moved value
        assert!(compile_fail_case_exists("ledger_double_release.rs"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::commit`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Write the implementation**

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CommitClass {
    NonblockingPrimaryPresent,
    NonblockingNonPresent,
    BlockingOrdinary,
    BlockingQualification,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CommitState {
    Submitting,
    Accepted,
    Completed,
    FailedBeforeSubmit,
    CompletionUnknown,
}

impl CommitState {
    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::FailedBeforeSubmit | Self::CompletionUnknown)
    }
}

/// Section 6.3 COMMIT-2: independent typed facts. Never derive one from
/// another; every field is set only by the evidence that proves it.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub(crate) struct Milestones {
    pub(crate) producer_ready: bool,
    pub(crate) dispatched: bool,
    pub(crate) accepted: bool,
    pub(crate) hardware_complete: bool,
    pub(crate) presented: bool,
    pub(crate) prior_buffer_released: bool,
}

impl Milestones {
    pub(crate) const fn completed_for(self, class: CommitClass) -> bool {
        match class {
            CommitClass::NonblockingPrimaryPresent => {
                self.accepted && self.hardware_complete && self.presented
            }
            CommitClass::NonblockingNonPresent | CommitClass::BlockingQualification => {
                self.accepted && self.hardware_complete
            }
            CommitClass::BlockingOrdinary => self.accepted,
        }
    }
}
```

Every type the record contains is defined **here**, in this task. Revision 1
declared `FenceSlotState` as an "opaque enum" for task 8 to complete and placed
a `StagedPageEvent` that task 9 defined; a Rust enum cannot be declared in one
module and redefined in another, so that order was unimplementable.

```rust
pub(crate) struct CommitRecord {
    // identity — spec 10 requires all of these in the record before dispatch
    pub(crate) commit: CommitId,
    pub(crate) token: EventToken,
    pub(crate) incarnation: IncarnationId,
    pub(crate) device_generation: DeviceGeneration,
    pub(crate) topology_generation: TopologyGeneration,
    pub(crate) lifecycle_epoch: LifecycleEpochId,
    pub(crate) transition: Option<LifecycleTransitionId>,
    pub(crate) class: CommitClass,
    /// True only for the mandatory install/restore commit of section 10.1.
    /// Orthogonal to `class`, because that commit may be nonblocking.
    pub(crate) is_qualification: bool,
    // the exact sets, recorded before dispatch and never recomputed
    pub(crate) closure: BTreeSet<u32>,
    pub(crate) expected_completion: BTreeSet<u32>,
    pub(crate) kernel_event_crtcs: BTreeSet<u32>,
    pub(crate) present_event_crtcs: BTreeSet<u32>,
    pub(crate) observed_event_crtcs: BTreeSet<u32>,
    pub(crate) fences: BTreeMap<u32, FenceSlotState>,
    pub(crate) staged_events: Vec<StagedPageEvent>,
    pub(crate) milestones: Milestones,
    pub(crate) state: CommitState,
    pub(crate) resources: ResourceLedger,
    pub(crate) outputs: Vec<usize>,
}

#[derive(Debug)]
pub(crate) enum FenceSlotState {
    /// Expected but not returned. Valid only after rejection or TEST_ONLY.
    Missing,
    Adopted(OwnedFd),
    Signalled,
    /// Retained with a `CompletionUnknown` record until the teardown barrier.
    Quarantined(OwnedFd),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StagedPageEvent {
    pub(crate) crtc: u32,
    pub(crate) sequence: u32,
    pub(crate) tv_sec: u32,
    pub(crate) tv_usec: u32,
}
```

`DeviceGeneration` and `TopologyGeneration` are newtypes introduced here.
Without them no reply or event handler can prove a result belongs to the current
generation, which spec section 10 requires before it may mutate a record.

`outputs` is the renderer output-index set the request paints. It is recorded at
submission because the commit record stores hardware CRTC ids, and nothing may
cast a CRTC id to an output index.

**`ResourceLedger` owns, it does not reference.** `COMMIT-6` requires the record
to uncertainty-own every possible old and new resource before IPC; a handle
keeps no Vulkan, GBM or dma-buf owner alive.

```rust
pub(crate) struct ResourceLedger {
    old: OwnedResourceSet,
    new: OwnedResourceSet,
    quarantined: bool,
}

pub(crate) struct OwnedResourceSet {
    framebuffers: Vec<FramebufferRef>,   // strong, RAII
    blobs: Vec<BlobRef>,
    pins: Vec<ScanoutPin>,
    external: Vec<ExternalOwnership>,
}

impl ResourceLedger {
    /// Explicit rejection: KMS acquired nothing, so the never-submitted new
    /// set is released and the old set is handed back to the caller.
    pub(crate) fn release_new(self) -> OwnedResourceSet;
    /// Acceptance-unknown: neither set is proven, so both are retained.
    pub(crate) fn quarantine(self) -> QuarantinedResources;
    /// Hardware completion: the new set becomes current and the old set is
    /// released only when the class-specific replacement rules also allow it.
    pub(crate) fn complete(self) -> (OwnedResourceSet, OwnedResourceSet);
}
```

Each transition consumes `self`, so a ledger cannot be released twice or
released and quarantined.

`terminalize` is a no-op once `state.is_terminal()`. `into_tombstone` consumes
the record, returns the ledger to the caller for its typed transition, and keeps
only `token`, `kernel_event_crtcs`, `present_event_crtcs`,
`observed_event_crtcs` and `terminal_state`.

`TombstoneRing` is a `VecDeque<Tombstone>` with `const CAPACITY: usize = 64`,
plus `clear_after_proven_drain()`. Its lookup returns
`TombstoneLookup::{Tombstoned(&Tombstone), Unknown}` — it stores only terminal
records and so can never report `Live`. `Resolution::{Live, Tombstoned, Unknown}`
belongs to `KmsDeviceOwner::resolve_token`, which checks the slot first and then
consults the ring.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner::commit`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/commit.rs crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): add commit records, typed milestones and the tombstone ring"
```

---

### Task 7 `[r2]`: The device slot, asynchronous submit, and the `OwnerEvent` stream

Rewritten from revision 1, which called the executor synchronously, returned
`Result<(), SubmitError>` so no caller could learn the `CommitId` or the ioctl
outcome, admitted every commit class while `Unqualified`, promoted atomic
`EBUSY` to incarnation poison, and never moved a terminal record out of the
slot.

**Files:**
- Create: `crates/yserver/src/kms/owner/device_owner.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs` (`SubmittingProof::new` becomes `pub(crate)`)

**Interfaces:**
- Consumes: `KmsIoExecutor::{send, poll_reply, check_watchdog, control_fd}` (task 4); `SerializedRequest` (task 5); `CommitRecord`, `ResourceLedger`, `TombstoneRing` (task 6).
- Produces:
  - `KmsDeviceOwner::{submit, on_control_readable, tick, control_fd, resolve_token, lifecycle_state, poison}`
  - `OwnerEvent` as defined in the revision 2 architecture section
  - `SubmitError::{SlotBusy, AdmissionClosed, ClockUnresolved, Construction}`
  - `ServicePhase::{ColdStart, SeatActive, FinalOffline}`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn submit_returns_the_commit_id_without_waiting() {
    let mut owner = owner_with_slow_helper_for_tests(Duration::from_millis(300));
    let started = Instant::now();
    let commit = owner
        .submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests())
        .expect("submit");
    assert!(started.elapsed() < Duration::from_millis(50), "submit waited");
    assert_eq!(owner.pending_state(), Some(CommitState::Submitting));
    assert_eq!(owner.pending_commit_for_tests(), Some(commit));
}

#[test]
fn the_record_and_its_resources_exist_before_the_frame_is_sent() {
    // COMMIT-6. Ordering is observed from the executor's own send hook.
    let mut owner = owner_with_send_observer_for_tests(|view| {
        assert!(!view.slot_is_free());
        assert_eq!(view.pending_state(), Some(CommitState::Submitting));
        assert!(view.pending_owns_resources_for_tests());
    });
    owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests())
        .expect("submit");
}

#[test]
fn one_device_never_has_two_submitted_commits_even_for_disjoint_crtcs() {
    let mut owner = owner_with_slow_helper_for_tests(Duration::from_millis(300));
    owner.submit(request_for_crtc_for_tests(40), CommitClass::NonblockingNonPresent, ledger_for_tests())
        .expect("first");
    assert_eq!(
        owner.submit(request_for_crtc_for_tests(41), CommitClass::NonblockingNonPresent, ledger_for_tests()),
        Err(SubmitError::SlotBusy)
    );
}

#[test]
fn outcomes_reach_the_caller_as_events_not_as_a_return_value() {
    let mut owner = scripted_owner_for_tests(&[ScriptedOutcome::Accepted]);
    let commit = owner
        .submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests())
        .expect("submit");
    assert!(owner.on_control_readable().is_empty(), "nothing readable yet");
    owner.deliver_scripted_reply_for_tests();
    assert_eq!(owner.on_control_readable(), vec![OwnerEvent::Accepted(commit)]);
}

#[test]
fn only_an_explicit_rejection_reaches_failed_before_submit() {
    for (outcome, expected) in [
        (ScriptedOutcome::Rejected(libc::EINVAL), CommitState::FailedBeforeSubmit),
        (ScriptedOutcome::Unknown(UnknownReason::HelperExited), CommitState::CompletionUnknown),
        (ScriptedOutcome::Unknown(UnknownReason::IpcFailure), CommitState::CompletionUnknown),
        (ScriptedOutcome::Unknown(UnknownReason::MalformedReply), CommitState::CompletionUnknown),
        (ScriptedOutcome::Unknown(UnknownReason::WatchdogExpired), CommitState::CompletionUnknown),
    ] {
        let mut owner = scripted_owner_for_tests(&[outcome]);
        owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests())
            .expect("submit");
        owner.deliver_scripted_reply_for_tests();
        owner.on_control_readable();
        assert_eq!(owner.last_terminal_state_for_tests(), Some(expected));
    }
}

#[test]
fn a_rejected_record_leaves_the_slot_and_becomes_a_tombstone() {
    // Revision 1 terminalized in place, so an ordinary rejection wedged the
    // device forever: submit refuses whenever the slot is occupied.
    let mut owner = scripted_owner_for_tests(&[ScriptedOutcome::Rejected(libc::EINVAL)]);
    let commit = owner
        .submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests())
        .expect("submit");
    owner.deliver_scripted_reply_for_tests();
    let events = owner.on_control_readable();
    assert!(events.contains(&OwnerEvent::Rejected { commit, errno: libc::EINVAL }));
    assert!(owner.slot_is_free(), "a rejection must free the device slot");
    assert!(matches!(owner.resolve_token(owner.token_of_for_tests(commit)), Resolution::Tombstoned(_)));
    assert!(owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests()).is_ok());
}

#[test]
fn an_unknown_record_keeps_the_slot_and_quarantines_both_sets() {
    let mut owner = scripted_owner_for_tests(&[ScriptedOutcome::Unknown(UnknownReason::IpcFailure)]);
    owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests())
        .expect("submit");
    owner.deliver_scripted_reply_for_tests();
    owner.on_control_readable();
    assert!(!owner.slot_is_free(), "an unknown record still owns the slot");
    assert!(owner.pending_resources_quarantined_for_tests());
}

#[test]
fn atomic_ebusy_closes_readiness_and_enters_recovery_without_poisoning() {
    // Section 9.4: EBUSY with no owner-tracked live record is an explicit
    // pre-submit rejection and an invariant failure. It is NOT a
    // completion-mechanism breach, and section 10's poison list does not
    // contain it, so it must not retire the fd family.
    let mut owner = scripted_owner_for_tests(&[ScriptedOutcome::Rejected(libc::EBUSY)]);
    owner.force_ready_for_tests();
    owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests())
        .expect("submit");
    owner.deliver_scripted_reply_for_tests();
    owner.on_control_readable();
    assert!(!owner.readiness_open());
    assert_ne!(owner.lifecycle_state(), DeviceLifecycleState::Poisoned);
    assert_eq!(owner.recovery_requested_for_tests(), Some(RecoveryCause::ForeignBusy));
    assert_eq!(owner.dispatch_count_for_tests(), 1, "EBUSY is never retried");
}

#[test]
fn the_admission_matrix_is_closed() {
    // Revision 1's predicate was `admits_ordinary_primary() ||
    // admits_qualification_commit()`, which admitted every class while
    // Unqualified — including a blocking one — and admitted blocking classes
    // while Ready and seat-active.
    use CommitClass::*;
    use DeviceLifecycleState::*;
    use ServicePhase::*;
    let cases = [
        // (lifecycle, phase, class, is_qualification, admitted)
        (Unqualified, ColdStart,    BlockingQualification,     true,  true),
        (Unqualified, SeatActive,   NonblockingNonPresent,     true,  true),
        (Unqualified, SeatActive,   NonblockingNonPresent,     false, false),
        (Unqualified, SeatActive,   NonblockingPrimaryPresent, false, false),
        (Unqualified, SeatActive,   BlockingOrdinary,          false, false),
        (Ready,       SeatActive,   NonblockingPrimaryPresent, false, true),
        (Ready,       SeatActive,   BlockingOrdinary,          false, false),
        (Ready,       FinalOffline, BlockingOrdinary,          false, true),
        (Quiescing,   SeatActive,   NonblockingNonPresent,     false, false),
        (Poisoned,    SeatActive,   NonblockingNonPresent,     false, false),
        (Poisoned,    FinalOffline, BlockingOrdinary,          false, false),
    ];
    for (state, phase, class, qual, admitted) in cases {
        assert_eq!(
            admission_permits(state, phase, class, qual),
            admitted,
            "{state:?}/{phase:?}/{class:?}/qual={qual}"
        );
    }
}

#[test]
fn a_seat_active_commit_never_carries_the_blocking_flags() {
    let mut owner = ready_owner_for_tests();
    owner.set_service_phase_for_tests(ServicePhase::SeatActive);
    owner.submit(primary_request_for_tests(), CommitClass::NonblockingPrimaryPresent, ledger_for_tests())
        .expect("submit");
    let sent = owner.last_sent_request_for_tests();
    assert_ne!(sent.flags & DRM_MODE_ATOMIC_NONBLOCK, 0);
    assert_eq!(sent.class, HostCallClass::SeatActiveNonblock);
    assert_eq!(sent.flags & DRM_MODE_PAGE_FLIP_ASYNC, 0, "C.0 never uses PAGE_FLIP_ASYNC");
}

#[test]
fn a_stale_reply_is_neither_consumed_nor_mistaken_for_a_rejection() {
    let mut owner = scripted_owner_for_tests(&[ScriptedOutcome::AcceptedWithStaleCorrelation]);
    let commit = owner
        .submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests())
        .expect("submit");
    owner.deliver_scripted_reply_for_tests();
    let events = owner.on_control_readable();
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::CompletionUnknown { commit: c, reason: UnknownReason::MalformedReply } if *c == commit
    )));
    assert!(!events.iter().any(|e| matches!(e, OwnerEvent::Rejected { .. })));
}

#[test]
fn the_watchdog_fires_from_tick_not_from_a_wait() {
    let mut owner = owner_with_slow_helper_for_tests(Duration::from_secs(30));
    let commit = owner
        .submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests())
        .expect("submit");
    assert!(owner.tick(Instant::now()).is_empty());
    let events = owner.tick(Instant::now() + Duration::from_secs(3));
    assert!(events.iter().any(|e| matches!(
        e,
        OwnerEvent::CompletionUnknown { commit: c, reason: UnknownReason::WatchdogExpired } if *c == commit
    )));
}

#[test]
fn every_dispatch_records_one_latency_sample_at_the_reply() {
    let mut owner = scripted_owner_for_tests(&[ScriptedOutcome::Accepted]);
    owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests())
        .expect("submit");
    owner.deliver_scripted_reply_for_tests();
    owner.on_control_readable();
    let samples = owner.export_evidence_for_tests().expect("evidence");
    assert_eq!(samples.len(), 1);
    assert!(samples[0].round_trip_ns >= samples[0].helper_duration_ns);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::device_owner`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Write the implementation**

```rust
pub(crate) struct KmsDeviceOwner {
    incarnation: IncarnationId,
    device_generation: DeviceGeneration,
    topology_generation: TopologyGeneration,
    lifecycle_epoch: LifecycleEpochId,
    transition: Option<LifecycleTransitionId>,
    state: DeviceLifecycleState,
    phase: ServicePhase,
    identities: IdentityAllocator,
    executor: KmsIoExecutor,
    in_flight: Option<InFlightHostCall>,
    fd_set: IncarnationFdSet,
    slot: Option<CommitRecord>,
    tombstones: TombstoneRing,
    clocks: BTreeMap<u32, CrtcClockRecord>,
    recorder: LatencyRecorder,
    pending_events: Vec<OwnerEvent>,
    next_request_seq: u64,
}
```

`submit` refuses on an occupied slot, on a closed admission matrix, and on an
unresolved clock for any CRTC in `kernel_event_crtcs`. It then allocates the
identities, builds the record **including the resource ledger it consumed**,
installs it in the slot, constructs `SubmittingProof`, calls
`executor.send(...)` and stores the returned `InFlightHostCall`. It returns the
`CommitId` and never waits.

```rust
fn admission_permits(
    state: DeviceLifecycleState,
    phase: ServicePhase,
    class: CommitClass,
    is_qualification: bool,
) -> bool {
    use CommitClass::*;
    use DeviceLifecycleState::*;
    use ServicePhase::*;
    // COMMIT-5: blocking is legal only at a cold-start or final-offline
    // boundary, whatever the lifecycle state.
    let blocking_ok = matches!(phase, ColdStart | FinalOffline);
    match (state, class) {
        (Poisoned | Quiescing, _) => false,
        (_, BlockingOrdinary | BlockingQualification) if !blocking_ok => false,
        // Unqualified admits ONLY the mandatory install/restore commit, of
        // whatever class section 10.1 permits — including a nonblocking one.
        (Unqualified, _) => is_qualification,
        (Ready, _) => true,
    }
}
```

`on_control_readable` calls `executor.poll_reply(&in_flight)`; `None` returns an
empty vector. A reply whose `ReplyCorrelation` does not match the record's
identities is `MalformedReply` — never a rejection — per `COMMIT-6`. Otherwise
it drives one central terminalization routine:

```rust
fn resolve_outcome(&mut self, outcome: HostCallOutcome) {
    let Some(record) = self.slot.as_mut() else { return };
    record.milestones.dispatched = true;
    match outcome {
        HostCallOutcome::Accepted { helper_duration_ns, round_trip_ns, out_fences, out_fence_present } => {
            record.milestones.accepted = true;
            self.record_sample(record.commit, round_trip_ns, helper_duration_ns);
            self.adopt_out_fences(out_fences, out_fence_present);   // task 8
            self.replay_staged_events();                            // task 9
            self.emit(OwnerEvent::Accepted(record.commit));
        }
        HostCallOutcome::Rejected { errno, helper_duration_ns, round_trip_ns, .. } => {
            self.record_sample(record.commit, round_trip_ns, helper_duration_ns);
            if record.has_staged_events() {
                // An event plus an explicit rejection is contradictory active
                // evidence; it poisons and is never reported as a rejection.
                self.terminalize(CommitState::CompletionUnknown, PoisonOn::Yes);
                return;
            }
            let commit = record.commit;
            self.terminalize(CommitState::FailedBeforeSubmit, PoisonOn::No);
            self.emit(OwnerEvent::Rejected { commit, errno });
            if errno == libc::EBUSY {
                // Section 9.4: an invariant/ownership failure, not a
                // completion-mechanism breach. Close readiness and enter the
                // bounded recovery path; do NOT poison the incarnation, which
                // would require retiring the whole fd family.
                self.close_readiness(ReadinessClosure::ForeignBusy);
                self.request_recovery(RecoveryCause::ForeignBusy);
            }
        }
        HostCallOutcome::Unknown(reason) => {
            let commit = record.commit;
            self.terminalize(CommitState::CompletionUnknown, PoisonOn::Yes);
            self.emit(OwnerEvent::CompletionUnknown { commit, reason });
        }
    }
}
```

`terminalize` is the single routine every terminal path uses. It sets the state
once, applies the ledger's typed transition (`release_new` for
`FailedBeforeSubmit`, `quarantine` for `CompletionUnknown`, `complete` for
`Completed`), pushes the tombstone, emits the `DamageInvalidate` event when the
disposition requires one, and **frees the slot unless the record is
`CompletionUnknown`** — an unknown record keeps the slot because neither state
is proven. Nothing else may mutate `self.slot`.

`tick(now)` calls `executor.check_watchdog`, then the fence poll and the
completion deadlines from tasks 8 and 13, draining `pending_events`.

`SubmittingProof::new()` becomes `pub(crate)` in `kms/executor/mod.rs`, with a
doc comment naming `KmsDeviceOwner::submit` and task 11's probe reservation as
its only callers.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/device_owner.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/src/kms/executor/mod.rs
git commit -m "feat(kms): add the asynchronous device commit owner and its event stream"
```

---

### Task 8: Out-fence adoption and canonical sync-file status

`§10`: "Readability is only a wakeup: the owner queries canonical sync-file status (for example `SYNC_IOC_FILE_INFO`) and counts only successful signalled status toward `HardwareComplete`."

**Files:**
- Create: `crates/yserver/src/kms/owner/fence.rs`
- Modify: `crates/yserver/src/kms/owner/device_owner.rs`
- Modify: `crates/yserver/src/platform/ioctl.rs` (add the `SYNC_IOC_FILE_INFO` request code beside the existing DRM ones)

**Interfaces:**
- Consumes: `IoctlReq`, `iowr` from `platform/ioctl.rs`; `OutFenceSlot` ordering from task 3.
- Produces:
  - `FenceSlotState::{Missing, Adopted(OwnedFd), Signalled, Quarantined(OwnedFd)}`
  - `FenceStatus::{Pending, Signalled, Error(i32), Unqueryable}`
  - `sync_file_status(fd: BorrowedFd<'_>) -> FenceStatus`
  - `KmsDeviceOwner::{adopt_out_fences, poll_fences}`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_live_success_with_a_missing_holder_is_completion_unknown_not_success() {
    let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
        // Two expected CRTCs, but the helper reports only slot 0 populated.
        ScriptedOutcome::AcceptedWithFences { present: 0b01, fences: 1 },
    ]);
    owner.submit(two_crtc_request_for_tests(), CommitClass::NonblockingNonPresent).expect("submit");
    assert_eq!(owner.pending_state(), Some(CommitState::CompletionUnknown));
    assert!(!owner.pending_record_for_tests().unwrap().milestones.hardware_complete);
}

// The `TEST_ONLY` counterpart of this rule is exercised in task 19, which
// introduces the validation lease. Task 8 owns only the live-commit rule:
// a `-1` holder on a live success is missing evidence, never success.

#[test]
fn a_non_sync_file_fd_enters_completion_unknown() {
    let (read, _write) = std::os::unix::net::UnixStream::pair().expect("pair");
    assert_eq!(sync_file_status(read.as_fd()), FenceStatus::Unqueryable);
}

#[test]
fn a_multi_crtc_commit_stays_pending_until_the_complete_set_signals() {
    let mut owner = owner_with_two_expected_crtcs_and_adopted_fences();
    owner.set_fence_status_for_tests(40, FenceStatus::Signalled);
    owner.poll_fences();
    assert!(!owner.pending_record_for_tests().unwrap().milestones.hardware_complete);
    owner.set_fence_status_for_tests(41, FenceStatus::Signalled);
    owner.poll_fences();
    assert!(owner.pending_record_for_tests().unwrap().milestones.hardware_complete);
    assert_eq!(owner.hardware_complete_count_for_tests(), 1, "retires exactly once");
}

#[test]
fn one_error_in_a_mixed_fence_set_prevents_every_hardware_retirement() {
    let mut owner = owner_with_two_expected_crtcs_and_adopted_fences();
    owner.set_fence_status_for_tests(40, FenceStatus::Signalled);
    owner.set_fence_status_for_tests(41, FenceStatus::Error(-libc::EIO));
    owner.poll_fences();
    assert!(!owner.pending_record_for_tests().unwrap().milestones.hardware_complete);
    assert_eq!(owner.pending_state(), Some(CommitState::CompletionUnknown));
}

#[test]
fn a_pending_fence_stays_armed_rather_than_advancing_anything() {
    let mut owner = owner_with_two_expected_crtcs_and_adopted_fences();
    owner.set_fence_status_for_tests(40, FenceStatus::Pending);
    owner.poll_fences();
    assert_eq!(owner.pending_state(), Some(CommitState::Accepted));
    assert!(owner.fence_is_registered_for_tests(40));
}

#[test]
fn every_adopted_fence_is_closed_exactly_once_on_hardware_completion() {
    // Revision 1 simply dropped the owner, so it passed even if the
    // hardware-completion path leaked, provided the destructor cleaned up.
    let counter = FdCloseCounter::install_for_tests();
    let mut owner = owner_with_two_expected_crtcs_and_adopted_fences();
    owner.set_fence_status_for_tests(40, FenceStatus::Signalled);
    owner.set_fence_status_for_tests(41, FenceStatus::Signalled);
    owner.poll_fences();
    assert!(owner.pending_record_for_tests().unwrap().milestones.hardware_complete);
    assert_eq!(counter.closes(), 2, "each adopted fence closed exactly once");
    drop(owner);
    assert_eq!(counter.closes(), 2, "drop must not close them again");
}

#[test]
fn a_rejected_ioctl_that_wrote_a_holder_closes_it_exactly_once() {
    // Spec 10.2: "Diagnose any defensively unexpected non-negative output and
    // close it exactly once." Revision 1 never exercised this branch.
    let counter = FdCloseCounter::install_for_tests();
    let mut owner = scripted_owner_for_tests(&[
        ScriptedOutcome::RejectedWithUnexpectedFence(libc::EINVAL),
    ]);
    owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests())
        .expect("submit");
    owner.deliver_scripted_reply_for_tests();
    owner.on_control_readable();
    assert_eq!(counter.closes(), 1);
    assert!(owner.last_reply_reported_unexpected_fence_for_tests());
}

#[test]
fn quarantined_fences_survive_with_their_unknown_record() {
    let mut owner = owner_with_two_expected_crtcs_and_adopted_fences();
    owner.set_fence_status_for_tests(41, FenceStatus::Error(-libc::EIO));
    owner.poll_fences();
    let record = owner.pending_record_for_tests().expect("record");
    assert!(record.fences.values().any(|slot| matches!(slot, FenceSlotState::Quarantined(_))));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::fence`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Write the implementation**

```rust
//! Canonical out-fence evidence.
//!
//! Poll readability is only a wakeup. The single source of truth is the
//! sync-file status query: only a successful signalled status counts toward
//! `HardwareComplete`, and a signalled error never promotes state or releases
//! a resource.

const SYNC_IOC_MAGIC: u8 = b'>';

#[repr(C)]
struct SyncFileInfo {
    name: [u8; 32],
    status: i32,
    flags: u32,
    num_fences: u32,
    pad: u32,
    sync_fence_info: u64,
}

const SYNC_IOC_FILE_INFO: IoctlReq =
    iowr(SYNC_IOC_MAGIC, 4, std::mem::size_of::<SyncFileInfo>());

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum FenceStatus {
    Pending,
    Signalled,
    Error(i32),
    /// The fd could not be queried at all: it is not a sync file, or the
    /// ioctl failed. This is a completion-mechanism breach, not "pending".
    Unqueryable,
}

pub(crate) fn sync_file_status(fd: BorrowedFd<'_>) -> FenceStatus {
    // SAFETY: `info` is a correctly sized, zero-initialized SyncFileInfo and
    // the ioctl writes only into it.
    let mut info: SyncFileInfo = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::ioctl(fd.as_raw_fd(), SYNC_IOC_FILE_INFO, std::ptr::addr_of_mut!(info))
    };
    if rc != 0 {
        return FenceStatus::Unqueryable;
    }
    match info.status {
        1 => FenceStatus::Signalled,
        0 => FenceStatus::Pending,
        negative => FenceStatus::Error(negative),
    }
}

#[derive(Debug)]
pub(crate) enum FenceSlotState {
    /// Expected but not returned. Valid only after a rejection or TEST_ONLY.
    Missing,
    Adopted(OwnedFd),
    Signalled,
    /// Retained with its `CompletionUnknown` record until the teardown barrier.
    Quarantined(OwnedFd),
}
```

`adopt_out_fences` walks `record.expected_completion` in the same order the builder emitted `out_fence_slots`, consuming one fd per set bit of `out_fence_present`. Any expected CRTC whose bit is clear stays `Missing`; if the class is live (not `ValidationOnly`) and the outcome was `Accepted`, a `Missing` slot immediately terminalizes the record as `CompletionUnknown` and poisons the incarnation.

`poll_fences` queries every `Adopted` slot. `Signalled` closes the fd exactly once (dropping the `OwnedFd`) and marks the slot `Signalled`; `Error` or `Unqueryable` moves the slot to `Quarantined`, terminalizes the record as `CompletionUnknown` and poisons. `HardwareComplete` is set only when every member of `expected_completion` is `Signalled`, and only once — the `Milestones` field is checked before the transition so a repeated poll retires nothing twice.

The owner registers each `Adopted` fd with the event loop for readability; readability calls `poll_fences` and never advances state by itself.

Add `pub(crate) const fn iowr(magic: u8, nr: u8, size: usize) -> IoctlReq` reuse from stage 1's `platform/ioctl.rs` — it already takes the magic byte as its first argument, so `SYNC_IOC_FILE_INFO` needs no new construction machinery, only the new magic and nr.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/fence.rs crates/yserver/src/kms/owner/device_owner.rs \
        crates/yserver/src/platform/ioctl.rs crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): adopt out-fences and gate HardwareComplete on canonical sync status"
```

---

### Task 9 `[r2]`: Migrate the `SequenceSupport` cache into the clock record

Added by the review. Stage 1 removed the name
`crtc_queue_sequence_unsupported_devices` and satisfied its exit criterion
literally, but the decision still lives in a separate map in the backend:

```rust
// crates/yserver/src/kms/render/backend.rs:1042
HashMap<(crate::platform::drm::DrmDeviceKey, ClockEpochId), SequenceSupport>
```

read at `:9246` and `:9297`, written at `:9382`, and consulted at `:16052`.
Spec section 10 requires the decision to live in the epoch-local CRTC clock
record: "The owner stores that decision directly in the epoch-local CRTC clock
record as `Unresolved` or `KernelSequence`; there is no separate device-keyed
unsupported cache." This map is separate, and its key omits the hardware CRTC,
so two CRTCs of one device in one epoch cannot disagree — which they must be
able to, because the probe is per `(incarnation, hardware CRTC, clock epoch)`.

Task 10 depends on this: revision 1 asserted the removal had already happened.

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:1037-1042,9246,9297,9382-9405,16052`
- Modify: `crates/yserver/src/kms/owner/clock.rs` (created by task 10 — this task lands first and defines the record it will fill)

**Interfaces:**
- Consumes: `ClockEpochId` (stage 1).
- Produces: `CrtcClockRecord { hardware_crtc, epoch, source, probe_attempted }` and `ClockSource::{Unresolved, KernelSequence { reference: u64 }}`, keyed per `(IncarnationId, hardware CRTC, ClockEpochId)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn two_crtcs_of_one_device_hold_independent_clock_decisions() {
    // The removed cache keyed only (device, epoch), so this was unrepresentable.
    let mut clocks = CrtcClockTable::new(IncarnationId::first());
    clocks.record_probe_result(40, ClockProbeResult::Supported { reference: 7 });
    clocks.record_probe_result(41, ClockProbeResult::Unsupported(libc::EOPNOTSUPP));
    assert_eq!(clocks.source(40), Some(ClockSource::KernelSequence { reference: 7 }));
    assert_eq!(clocks.source(41), Some(ClockSource::Unresolved));
}

#[test]
fn a_new_incarnation_starts_every_crtc_unresolved() {
    let mut clocks = CrtcClockTable::new(IncarnationId::first());
    clocks.record_probe_result(40, ClockProbeResult::Supported { reference: 7 });
    let fresh = CrtcClockTable::new(IncarnationId::first().next());
    assert_eq!(fresh.source(40), None, "no decision survives an incarnation");
}

#[test]
fn no_device_keyed_sequence_cache_remains_in_the_backend() {
    let src = include_str!("../render/backend.rs");
    assert!(!src.contains("SequenceSupport"), "the separate cache must be gone");
    assert!(!src.contains("crtc_queue_sequence_unsupported_devices"));
}

#[test]
fn the_backend_consults_the_owner_record_rather_than_its_own_map() {
    let backend = backend_with_unresolved_clock_for_tests(40);
    assert!(!backend.may_arm_sequence_for_tests(40));
    backend.owner_record_probe_for_tests(40, ClockProbeResult::Supported { reference: 1 });
    assert!(backend.may_arm_sequence_for_tests(40));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver sequence_support`
Expected: FAIL — `CrtcClockTable` does not exist and `SequenceSupport` is still in `backend.rs`.

- [ ] **Step 3: Write the implementation**

Create `CrtcClockTable` in `kms/owner/clock.rs`, keyed
`BTreeMap<(u32 /* hardware CRTC */, ClockEpochId), CrtcClockRecord>` and scoped
to one `IncarnationId` by construction, so a fresh incarnation is a fresh table
rather than an invalidation pass.

Delete `SequenceSupport`, the `HashMap` field at `backend.rs:1042`, the
`sequence_support` accessor at `:9395` and the insert at `:9382`. The three
read sites (`:9246`, `:9297`, `:16052`) call
`owner.clock_source(hardware_crtc)` and treat anything but
`ClockSource::KernelSequence` as "cannot arm".

The `UnsupportedForEpoch` state disappears rather than being renamed: an
unsupported probe result leaves the record `Unresolved` with
`probe_attempted = true`, which is what task 10's no-retry rule reads. Two
states for one fact is how the stale cache survived stage 1's removal.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/clock.rs crates/yserver/src/kms/render/backend.rs
git commit -m "refactor(kms): move the sequence-support decision into the clock record"
```

---

### Task 10 `[r2]`: The owner-serialized clock probe and the epoch-local clock record

`§10`: no event-bearing commit may be admitted on a newly installed active hardware CRTC or clock epoch until one `DRM_IOCTL_CRTC_GET_SEQUENCE` probe, serialized through the executor, returns a current success.

**Files:**
- Create: `crates/yserver/src/kms/owner/clock.rs`
- Modify: `crates/yserver/src/kms/owner/device_owner.rs`

**Interfaces:**
- Consumes: `HostCallRequest::ClockProbe`, `HostCallReply::ClockProbe`; `ClockEpochId`.
- Produces:
  - `ClockSource::{Unresolved, KernelSequence { reference: u64 }}`
  - `CrtcClockRecord { hardware_crtc: u32, epoch: ClockEpochId, source: ClockSource }`
  - `ClockProbeOutcome::{Selected, QualificationFailed(i32), Stalled}`
  - `KmsDeviceOwner::{probe_crtc_clock, clock_record, invalidate_crtc_clock_epoch, admits_event_bearing_commit}`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn an_event_bearing_commit_is_refused_until_the_clock_probe_succeeds() {
    let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[]);
    assert!(!owner.admits_event_bearing_commit(40));
    assert_eq!(
        owner.submit(present_request_for_tests(40), CommitClass::NonblockingPrimaryPresent),
        Err(SubmitError::ClockUnresolved(40))
    );
}

#[test]
fn a_successful_probe_selects_kernel_sequence_with_the_trusted_reference() {
    let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
        ScriptedOutcome::ClockProbe(0x1_0000_0005),
    ]);
    assert_eq!(owner.probe_crtc_clock(40), ClockProbeOutcome::Selected);
    assert_eq!(
        owner.clock_record(40).map(|r| r.source),
        Some(ClockSource::KernelSequence { reference: 0x1_0000_0005 })
    );
    assert!(owner.admits_event_bearing_commit(40));
}

#[test]
fn eopnotsupp_closes_qualification_and_permits_no_same_epoch_retry() {
    let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
        ScriptedOutcome::Rejected(libc::EOPNOTSUPP),
    ]);
    assert_eq!(
        owner.probe_crtc_clock(40),
        ClockProbeOutcome::QualificationFailed(libc::EOPNOTSUPP)
    );
    assert_eq!(owner.clock_record(40).map(|r| r.source), Some(ClockSource::Unresolved));
    // A second probe in the same epoch must not be attempted at all.
    assert_eq!(owner.probe_crtc_clock(40), ClockProbeOutcome::QualificationFailed(libc::EOPNOTSUPP));
    assert_eq!(owner.clock_probe_dispatch_count_for_tests(), 1);
}

#[test]
fn a_new_clock_epoch_starts_unresolved_even_for_the_same_raw_handle() {
    let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
        ScriptedOutcome::ClockProbe(7),
    ]);
    owner.probe_crtc_clock(40);
    let first_epoch = owner.clock_record(40).unwrap().epoch;
    owner.invalidate_crtc_clock_epoch(40);
    let record = owner.clock_record(40).unwrap();
    assert_ne!(record.epoch, first_epoch);
    assert_eq!(record.source, ClockSource::Unresolved);
    assert!(!owner.admits_event_bearing_commit(40));
}

#[test]
fn a_stale_probe_reply_is_discarded_rather_than_selecting_a_source() {
    let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
        ScriptedOutcome::ClockProbeWithStaleEpoch(7),
    ]);
    assert_eq!(owner.probe_crtc_clock(40), ClockProbeOutcome::QualificationFailed(0));
    assert_eq!(owner.clock_record(40).map(|r| r.source), Some(ClockSource::Unresolved));
}

#[test]
fn probe_timeout_follows_the_executor_stall_path_and_creates_no_software_clock() {
    let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
        ScriptedOutcome::Unknown(UnknownReason::WatchdogExpired),
    ]);
    assert_eq!(owner.probe_crtc_clock(40), ClockProbeOutcome::Stalled);
    assert_eq!(owner.clock_record(40).map(|r| r.source), Some(ClockSource::Unresolved));
    assert!(!owner.admits_event_bearing_commit(40));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::clock`
Expected: FAIL.

- [ ] **Step 3: Write the implementation**

The clock record is stored directly per `(hardware CRTC, clock epoch)` in a `BTreeMap<u32, CrtcClockRecord>` on the owner — there is no separate device-keyed unsupported cache, which stage 1 already removed. A record whose `source` is `Unresolved` and whose `probe_attempted` flag is set never re-probes in that epoch; `invalidate_crtc_clock_epoch` bumps `ClockEpochId`, clears `probe_attempted`, discards the extension reference and resets the source.

A probe is a host call, so it needs a `SubmittingProof` — but it installs no
`CommitRecord`, and revision 1 scoped that proof's constructor to
`KmsDeviceOwner::submit`, leaving the probe no legal way to build one. The owner
therefore holds a second, narrower reservation:

```rust
/// A non-atomic host-call reservation. It occupies no atomic device slot, so
/// an accepted commit may still be pending, but it does occupy the executor,
/// which is what serializes host calls per COMMIT-5.
pub(crate) struct HostCallReservation {
    probe: ClockProbeId,
    incarnation: IncarnationId,
    lifecycle_epoch: LifecycleEpochId,
    hardware_crtc: u32,
    clock_epoch: ClockEpochId,
}

impl HostCallReservation {
    pub(crate) fn proof(&self) -> SubmittingProof;
}
```

`probe_crtc_clock` installs the reservation, builds
`HostCallRequest::ClockProbe`, and calls `executor.send`. It returns
immediately; the result arrives through `on_control_readable` like every other
outcome. While the reservation is outstanding the owner dispatches no other host
call — atomic, validation or probe — and `submit` returns `SubmitError::SlotBusy`.

A reply whose incarnation, lifecycle epoch, hardware CRTC, clock epoch or probe
id are not all current is a **neutral discard**: it is dropped with a telemetry
counter and mutates nothing. Revision 1 mapped it to `QualificationFailed(0)`,
which is not a qualification failure — errno zero means no error — and worse,
would mark the newly winning epoch as attempted, permanently blocking the
replacement probe that the winning lifecycle transition owns.

`CrtcClockRecord` carries `probe_attempted: bool` explicitly, declared in
task 9. It gates the no-retry rule: an `Unresolved` record with
`probe_attempted = true` never re-probes in that epoch, and
`invalidate_crtc_clock_epoch` clears it.

`admits_event_bearing_commit(crtc)` returns true only for `ClockSource::KernelSequence`, and `submit` checks it for every commit whose `kernel_event_crtcs` is non-empty, returning `SubmitError::ClockUnresolved(crtc)`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/clock.rs crates/yserver/src/kms/owner/device_owner.rs \
        crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): serialize the CRTC clock probe through the owner"
```

---

### Task 11: `KernelSequence` page-event normalization

**Files:**
- Modify: `crates/yserver/src/kms/owner/clock.rs`

**Interfaces:**
- Consumes: `ClockSource::KernelSequence { reference }`.
- Produces:
  - `normalize_ust(tv_sec: u32, tv_usec: u32) -> Option<u64>`
  - `extend_sequence(reference: u64, raw: u32) -> Option<u64>`
  - `CrtcClockRecord::normalize_page_event(&mut self, tv_sec, tv_usec, raw_sequence) -> Result<ClockSample, NormalizeError>` with `ClockSample { msc: u64, ust_us: u64 }` and `NormalizeError::{InvalidUsec, UstOverflow, AmbiguousSequence, NoRepresentative}`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn ust_conversion_accepts_the_maximum_valid_microsecond() {
    assert_eq!(normalize_ust(0, 999_999), Some(999_999));
    assert_eq!(normalize_ust(1, 0), Some(1_000_000));
}

#[test]
fn a_microsecond_field_of_one_million_is_invalid() {
    assert_eq!(normalize_ust(0, 1_000_000), None);
}

#[test]
fn maximum_u32_seconds_converts_without_overflow() {
    let expected = u64::from(u32::MAX) * 1_000_000 + 999_999;
    assert_eq!(normalize_ust(u32::MAX, 999_999), Some(expected));
}

#[test]
fn sequence_extension_picks_the_representative_within_half_the_range() {
    assert_eq!(extend_sequence(0x1_0000_0000, 0x0000_0005), Some(0x1_0000_0005));
    // A raw value just below the reference's low half stays in the same block.
    assert_eq!(extend_sequence(0x1_0000_0005, 0x0000_0000), Some(0x1_0000_0000));
}

#[test]
fn sequence_extension_follows_a_u32_wrap_forwards() {
    let mut reference = 0x1_ffff_fffe_u64;
    for raw in [0xffff_fffe_u32, 0xffff_ffff, 0x0000_0000, 0x0000_0001] {
        let extended = extend_sequence(reference, raw).expect("representative");
        assert!(extended >= reference, "the clock never moves backwards");
        reference = extended;
    }
    assert_eq!(reference, 0x2_0000_0001);
}

#[test]
fn an_exact_half_range_distance_is_ambiguous_and_rejected() {
    // Both candidates sit exactly 2^31 away from the reference.
    assert_eq!(extend_sequence(0x1_0000_0000, 0x8000_0000), None);
}

#[test]
fn a_reference_with_no_non_negative_representative_is_rejected() {
    assert_eq!(extend_sequence(0, 0x8000_0001), None);
}

#[test]
fn a_raw_zero_is_ordinary_sequence_data_and_never_switches_the_source() {
    let mut record = kernel_sequence_record_for_tests(0xffff_ffff);
    let sample = record.normalize_page_event(1, 2, 0).expect("sample");
    assert_eq!(sample.msc, 0x1_0000_0000);
    assert_eq!(record.source, ClockSource::KernelSequence { reference: 0x1_0000_0000 });
}

#[test]
fn a_late_sample_is_classified_but_cannot_move_the_clock_backwards() {
    let mut record = kernel_sequence_record_for_tests(100);
    let sample = record.normalize_page_event(1, 2, 90).expect("sample");
    assert_eq!(sample.msc, 90);
    assert_eq!(
        record.source,
        ClockSource::KernelSequence { reference: 100 },
        "a late sample never advances the trusted reference"
    );
}

#[test]
fn clock_records_are_isolated_per_crtc_and_per_epoch() {
    let mut owner = owner_with_two_probed_crtcs_for_tests(40, 41);
    owner.clock_record_mut(40).unwrap().normalize_page_event(1, 2, 500).expect("sample");
    assert_eq!(
        owner.clock_record(41).unwrap().source,
        ClockSource::KernelSequence { reference: 0 },
        "advancing one CRTC's clock must not touch another's"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::clock`
Expected: FAIL.

- [ ] **Step 3: Write the implementation**

```rust
pub(crate) fn normalize_ust(tv_sec: u32, tv_usec: u32) -> Option<u64> {
    if tv_usec >= 1_000_000 {
        return None;
    }
    u64::from(tv_sec)
        .checked_mul(1_000_000)?
        .checked_add(u64::from(tv_usec))
}

/// Choose the unique non-negative value congruent to `raw (mod 2^32)` whose
/// modular distance from `reference` is strictly less than `2^31`. An exact
/// half-range tie, or the absence of a non-negative representative, is
/// invalid — the spec forbids guessing which side of the wrap the sample is on.
pub(crate) fn extend_sequence(reference: u64, raw: u32) -> Option<u64> {
    const RANGE: u64 = 1 << 32;
    const HALF: u64 = 1 << 31;

    let low = u64::from(raw);
    let base = reference & !(RANGE - 1);
    let candidates = [
        base.checked_sub(RANGE).map(|b| b + low),
        Some(base + low),
        base.checked_add(RANGE).map(|b| b + low),
    ];

    let mut chosen = None;
    for candidate in candidates.into_iter().flatten() {
        let distance = candidate.abs_diff(reference);
        if distance < HALF {
            if chosen.is_some() {
                // Two representatives inside the half range is impossible for
                // a 2^32-periodic value; treat it as ambiguity rather than
                // silently preferring one.
                return None;
            }
            chosen = Some(candidate);
        } else if distance == HALF {
            return None;
        }
    }
    chosen
}
```

`normalize_page_event` computes the UST, extends the sequence, and advances `reference` only when the extended value is greater than the current reference — a valid late sample is returned to the caller for classification but never moves the clock backwards. A matching active Present event whose normalized sample contradicts its clock epoch terminalizes that record as `CompletionUnknown` in the caller rather than inventing an MSC/UST; that check lives in `events.rs` and is exercised by the `Presented` path from task 8.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner::clock`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/clock.rs
git commit -m "feat(kms): normalize KernelSequence page-event MSC and UST"
```

---

### Task 12: Tagged page-event correlation and its poison rules

**Files:**
- Create: `crates/yserver/src/kms/owner/events.rs`
- Modify: `crates/yserver/src/kms/owner/device_owner.rs`

**Interfaces:**
- Consumes: `DrmEventRecord` from `drm/event_stream.rs`; `TombstoneRing`, `CommitRecord` (task 5).
- Produces:
  - `EventDisposition::{Presented, ObservedNonConsumer, ClockSampleOnly, TelemetryOnly(TelemetryReason), Poison(PoisonCause)}`
  - `KmsDeviceOwner::on_drm_event(&mut self, record: DrmEventRecord) -> EventDisposition`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_multi_crtc_present_is_not_presented_until_every_consumer_event_arrives() {
    // Revision 1 made `presented` one bool set by the first event, so a
    // two-CRTC Present completed after one. Spec 6.3: the page event is
    // "required for each Present CRTC".
    let mut owner = accepted_present_owner_with_crtcs_for_tests(&[40, 41]);
    let token = owner.pending_token_for_tests();
    owner.on_drm_event(page_flip(40, token.as_user_data()));
    let record = owner.pending_record_for_tests().unwrap();
    assert_eq!(record.milestones.presented_crtcs, BTreeSet::from([40]));
    assert!(!record.milestones.completed_for(CommitClass::NonblockingPrimaryPresent));
    owner.on_drm_event(page_flip(41, token.as_user_data()));
    assert!(owner.pending_record_for_tests().unwrap()
        .milestones.completed_for(CommitClass::NonblockingPrimaryPresent));
}

#[test]
fn an_event_arriving_before_acceptance_has_its_own_disposition() {
    // Neither `Presented` (it is not, yet) nor `ObservedNonConsumer` (that
    // would misreport its consumer class).
    let mut owner = submitting_present_owner_for_tests(40);
    let token = owner.pending_token_for_tests();
    assert_eq!(
        owner.on_drm_event(page_flip(40, token.as_user_data())),
        EventDisposition::StagedPendingAcceptance
    );
}

#[test]
fn a_token_delivered_with_the_wrong_event_type_poisons() {
    // Spec 10: "an active token delivered with the wrong event type is a
    // completion-mechanism contradiction and poisons the incarnation."
    let mut owner = accepted_present_owner_for_tests(40);
    let token = owner.pending_token_for_tests();
    assert_eq!(
        owner.on_drm_event(vblank(40, token.as_user_data())),
        EventDisposition::Poison(PoisonCause::WrongEventTypeForToken)
    );

    let mut owner = accepted_present_owner_for_tests(40);
    let arm = owner.arm_sequence_for_tests(40);
    assert_eq!(
        owner.on_drm_event(page_flip(40, arm.as_user_data())),
        EventDisposition::Poison(PoisonCause::WrongEventTypeForToken)
    );
}

#[test]
fn a_crtc_sequence_record_is_never_read_for_a_crtc_id_it_does_not_have() {
    // `DrmEventRecord::CrtcSequence` has no `crtc_id` field. Revision 1's
    // algorithm read one off every record. Dispatch by variant first.
    let mut owner = accepted_present_owner_for_tests(40);
    let arm = owner.arm_sequence_for_tests(40);
    assert_eq!(
        owner.on_drm_event(crtc_sequence(arm.as_user_data(), /* sequence */ 9)),
        EventDisposition::ClockSampleOnly
    );
}

#[test]
fn a_matching_present_event_stages_presented_for_a_present_consumer() {
    let mut owner = accepted_present_owner_for_tests(/* crtc */ 40);
    let token = owner.pending_token_for_tests();
    let d = owner.on_drm_event(page_flip(40, token.as_user_data()));
    assert_eq!(d, EventDisposition::Presented);
    assert!(owner.pending_record_for_tests().unwrap().milestones.presented);
}

#[test]
fn presented_is_not_protocol_authoritative_before_explicit_ioctl_success() {
    // A page event that arrives while the record is still `Submitting` is
    // staged, not consumed, and consumed only after acceptance.
    let mut owner = submitting_present_owner_for_tests(40);
    let token = owner.pending_token_for_tests();
    owner.on_drm_event(page_flip(40, token.as_user_data()));
    assert!(!owner.pending_record_for_tests().unwrap().milestones.presented);
    assert_eq!(owner.staged_event_count_for_tests(), 1);
    owner.deliver_scripted_acceptance_for_tests();
    assert!(owner.pending_record_for_tests().unwrap().milestones.presented);
}

#[test]
fn a_kernel_event_outside_present_event_crtcs_is_observed_but_never_presented() {
    let mut owner = accepted_owner_with_two_kernel_event_crtcs_one_consumer(40, 41);
    let token = owner.pending_token_for_tests();
    let d = owner.on_drm_event(page_flip(41, token.as_user_data()));
    assert_eq!(d, EventDisposition::ObservedNonConsumer);
    assert!(!owner.pending_record_for_tests().unwrap().milestones.presented);
}

#[test]
fn zero_unknown_and_tombstoned_tokens_are_telemetry_only() {
    let mut owner = accepted_present_owner_for_tests(40);
    assert_eq!(
        owner.on_drm_event(page_flip(40, 0)),
        EventDisposition::TelemetryOnly(TelemetryReason::ZeroToken)
    );
    assert_eq!(
        owner.on_drm_event(page_flip(40, 0x4000_0000_dead_beef)),
        EventDisposition::TelemetryOnly(TelemetryReason::UnknownToken)
    );
    assert_eq!(owner.lifecycle_state(), DeviceLifecycleState::Ready);
}

#[test]
fn a_duplicate_for_an_already_observed_crtc_advances_nothing_and_warns() {
    let mut owner = accepted_present_owner_for_tests(40);
    let token = owner.pending_token_for_tests();
    owner.on_drm_event(page_flip(40, token.as_user_data()));
    assert_eq!(
        owner.on_drm_event(page_flip(40, token.as_user_data())),
        EventDisposition::TelemetryOnly(TelemetryReason::Duplicate)
    );
    assert_eq!(owner.lifecycle_state(), DeviceLifecycleState::Ready);
}

#[test]
fn the_current_token_with_zero_crtc_id_poisons_immediately() {
    let mut owner = accepted_present_owner_for_tests(40);
    let token = owner.pending_token_for_tests();
    assert_eq!(
        owner.on_drm_event(page_flip(0, token.as_user_data())),
        EventDisposition::Poison(PoisonCause::ZeroCrtcForCurrentToken)
    );
    assert_eq!(owner.lifecycle_state(), DeviceLifecycleState::Poisoned);
}

#[test]
fn the_current_token_on_a_crtc_outside_the_kernel_event_set_poisons() {
    let mut owner = accepted_present_owner_for_tests(40);
    let token = owner.pending_token_for_tests();
    assert_eq!(
        owner.on_drm_event(page_flip(99, token.as_user_data())),
        EventDisposition::Poison(PoisonCause::EventCrtcOutsideKernelSet)
    );
}

#[test]
fn an_event_paired_with_an_explicit_rejection_is_contradictory_and_poisons() {
    let mut owner = submitting_present_owner_for_tests(40);
    let token = owner.pending_token_for_tests();
    owner.on_drm_event(page_flip(40, token.as_user_data()));
    owner.deliver_scripted_rejection_for_tests(libc::EINVAL);
    assert_eq!(owner.lifecycle_state(), DeviceLifecycleState::Poisoned);
    assert_eq!(owner.pending_state(), Some(CommitState::CompletionUnknown));
}

#[test]
fn a_delayed_old_generation_event_cannot_match_a_newer_commit_after_evictions() {
    let mut owner = accepted_present_owner_for_tests(40);
    let old_token = owner.pending_token_for_tests();
    owner.complete_pending_for_tests();
    for _ in 0..70 {
        owner.cycle_one_commit_for_tests(40);
    }
    // The old token's tombstone is evicted, but the token was never reused,
    // so the delayed event resolves to `Unknown`, never to the live commit.
    assert_eq!(
        owner.on_drm_event(page_flip(40, old_token.as_user_data())),
        EventDisposition::TelemetryOnly(TelemetryReason::UnknownToken)
    );
    assert!(!owner.pending_record_for_tests().unwrap().milestones.presented);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::events`
Expected: FAIL.

- [ ] **Step 3: Write the implementation**

`on_drm_event` follows spec `§10`'s classification list in order:

1. `EventToken::from_user_data(user_data)` — `None` is `TelemetryOnly(ZeroToken)`, **except** that a zero `crtc_id` is checked first only when the token *does* resolve to the live record; a zero token with any CRTC is telemetry-only.
2. Resolve the token: live record, tombstone, or unknown. Unknown and tombstoned are `TelemetryOnly`.
3. Live record: `crtc_id == 0` → `Poison(ZeroCrtcForCurrentToken)`. Not in `kernel_event_crtcs` → `Poison(EventCrtcOutsideKernelSet)`. Already in `observed_event_crtcs` → `TelemetryOnly(Duplicate)` plus `log::warn!`.
4. Record the CRTC as observed. If the record is `Submitting`, push a `StagedPageEvent` and return `EventDisposition::Presented` only after acceptance — the staged events are replayed inside `on_host_call_outcome`'s `Accepted` arm. A staged event replayed against a `Rejected` outcome is the contradiction case: `Poison(EventPlusRejection)` and `CompletionUnknown`.
5. If the CRTC is in `present_event_crtcs`, set `milestones.presented` (once) and hand the normalized MSC/UST from task 10 to the Present consumer; otherwise `ObservedNonConsumer`, which still updates the general CRTC clock.

`poison(cause)` sets `DeviceLifecycleState::Poisoned`, closes readiness, terminalizes the live record as `CompletionUnknown` if it is not already terminal, and logs the cause. `§10`: incarnation poison stops all live KMS submission on that fd, including primary work that omits the failing state — `submit` already refuses in `Poisoned`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/events.rs crates/yserver/src/kms/owner/device_owner.rs \
        crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): correlate tagged page events and pin their poison rules"
```

---

### Task 13: The three post-dispatch monotonic deadlines

**Files:**
- Create: `crates/yserver/src/kms/owner/deadline.rs`
- Modify: `crates/yserver/src/kms/owner/device_owner.rs`

**Interfaces:**
- Consumes: `Milestones`, `CommitRecord`.
- Produces:
  - `fast_hardware_deadline(slowest_mode_period: Option<Duration>) -> Duration`
  - `lifecycle_hardware_deadline(observed_max: Duration) -> Duration`
  - `present_event_deadline(mode_period: Option<Duration>) -> Duration`
  - `CommitDeadlines::{arm_hardware, arm_present_events, expired}` returning `Expiry::{None, Hardware, PresentEvent(u32)}`
  - `KmsDeviceOwner::tick_deadlines(&mut self, now: Instant)`

- [ ] **Step 1: Write the failing tests**

```rust
const UNKNOWN_PERIOD: Duration = Duration::from_nanos(16_667_000);

#[test]
fn the_fast_hardware_deadline_applies_the_exact_clamp() {
    // 3 * 16.667 ms = 50 ms, below the 100 ms floor.
    assert_eq!(fast_hardware_deadline(None), Duration::from_millis(100));
    assert_eq!(fast_hardware_deadline(Some(UNKNOWN_PERIOD)), Duration::from_millis(100));
    // 3 * 200 ms = 600 ms, inside the range.
    assert_eq!(fast_hardware_deadline(Some(Duration::from_millis(200))), Duration::from_millis(600));
    // 3 * 1 s = 3 s, above the 2 s ceiling.
    assert_eq!(fast_hardware_deadline(Some(Duration::from_secs(1))), Duration::from_secs(2));
}

#[test]
fn the_lifecycle_hardware_deadline_applies_min_max_and_the_representable_margin() {
    assert_eq!(lifecycle_hardware_deadline(Some(Duration::from_secs(1))), Ok(Duration::from_secs(10)));
    assert_eq!(lifecycle_hardware_deadline(Some(Duration::from_secs(12))), Ok(Duration::from_secs(14)));
    // Revision 1 clamped 40 s to 30 s, silently validating a cohort the spec
    // requires to stay unvalidated.
    assert_eq!(
        lifecycle_hardware_deadline(Some(Duration::from_secs(40))),
        Err(CohortUnvalidated::AboveRepresentableMargin(Duration::from_secs(40)))
    );
    assert_eq!(lifecycle_hardware_deadline(None), Err(CohortUnvalidated::MissingEvidence));
}

#[test]
fn an_unvalidated_cohort_is_not_a_completion_timeout_and_poisons_nothing() {
    let mut owner = ready_owner_for_tests();
    owner.set_lifecycle_observed_max_for_tests(None);
    assert_eq!(
        owner.submit(modeset_request_for_tests(), CommitClass::NonblockingNonPresent, ledger_for_tests()),
        Err(SubmitError::CohortUnvalidated)
    );
    assert_ne!(owner.lifecycle_state(), DeviceLifecycleState::Poisoned);
}

#[test]
fn deadline_arithmetic_never_panics_on_an_absurd_mode_period() {
    assert_eq!(fast_hardware_deadline(Some(Duration::MAX)), Duration::from_secs(2));
    assert_eq!(present_event_deadline(Some(Duration::MAX)), Duration::from_millis(500));
}

#[test]
fn the_present_event_deadline_applies_the_exact_clamp() {
    assert_eq!(present_event_deadline(None), Duration::from_millis(50));
    assert_eq!(present_event_deadline(Some(Duration::from_millis(100))), Duration::from_millis(200));
    assert_eq!(present_event_deadline(Some(Duration::from_secs(1))), Duration::from_millis(500));
}

#[test]
fn the_present_timer_starts_at_hardware_complete_not_at_dispatch() {
    let mut deadlines = CommitDeadlines::default();
    let dispatch = Instant::now();
    deadlines.arm_hardware(dispatch, Duration::from_millis(100));
    let hw = dispatch + Duration::from_millis(80);
    deadlines.arm_present_events(hw, &[(40, Duration::from_millis(200))]);
    assert_eq!(deadlines.expired(hw + Duration::from_millis(199)), Expiry::None);
    assert_eq!(deadlines.expired(hw + Duration::from_millis(201)), Expiry::PresentEvent(40));
}

#[test]
fn an_event_that_already_arrived_arms_no_present_timer() {
    let mut deadlines = CommitDeadlines::default();
    let hw = Instant::now();
    deadlines.arm_present_events(hw, &[]);
    assert_eq!(deadlines.expired(hw + Duration::from_secs(10)), Expiry::None);
}

#[test]
fn a_producer_timeout_is_classified_separately_and_occupies_no_slot() {
    let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[]);
    owner.fail_producer_for_tests(ProducerFailure::Timeout);
    assert!(owner.slot_is_free(), "a never-submitted intent occupies no slot");
    assert_eq!(owner.lifecycle_state(), DeviceLifecycleState::Unqualified, "no poison");
}

#[test]
fn a_hardware_deadline_expiry_after_dispatch_enters_completion_unknown_and_poisons() {
    let mut owner = accepted_owner_with_two_expected_crtcs_for_tests();
    owner.tick_deadlines(Instant::now() + Duration::from_secs(5));
    assert_eq!(owner.pending_state(), Some(CommitState::CompletionUnknown));
    assert_eq!(owner.lifecycle_state(), DeviceLifecycleState::Poisoned);
}

#[test]
fn a_multi_crtc_present_arms_one_deadline_per_required_crtc_and_expires_once() {
    let mut owner = accepted_present_owner_with_crtcs_for_tests(&[40, 41]);
    owner.reach_hardware_complete_for_tests();
    owner.on_drm_event(page_flip(40, owner.pending_token_for_tests().as_user_data()));
    owner.tick_deadlines(Instant::now() + Duration::from_secs(1));
    assert_eq!(owner.pending_state(), Some(CommitState::CompletionUnknown));
    assert_eq!(owner.completion_unknown_count_for_tests(), 1);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::deadline`
Expected: FAIL.

- [ ] **Step 3: Write the implementation**

```rust
const UNKNOWN_MODE_PERIOD: Duration = Duration::from_nanos(16_667_000);
/// Spec 10.3: an observation above the representable margin leaves the cohort
/// unvalidated rather than being clamped into a deadline.
const LIFECYCLE_OBSERVED_MAX_CEILING: Duration = Duration::from_secs(28);

pub(crate) fn fast_hardware_deadline(slowest_mode_period: Option<Duration>) -> Duration {
    let period = slowest_mode_period.unwrap_or(UNKNOWN_MODE_PERIOD);
    // checked_mul, not `*`: Duration multiplication panics on overflow, and a
    // mode period is discovered data.
    period
        .checked_mul(3)
        .unwrap_or(Duration::from_secs(2))
        .clamp(Duration::from_millis(100), Duration::from_secs(2))
}

/// `Err(CohortUnvalidated)` is NOT a live-completion timeout and must not
/// poison anything. It means this release has no usable lifecycle timing
/// evidence for the cohort, so no lifecycle commit may be admitted under a
/// fabricated deadline.
pub(crate) fn lifecycle_hardware_deadline(
    observed_max: Option<Duration>,
) -> Result<Duration, CohortUnvalidated> {
    let observed = observed_max.ok_or(CohortUnvalidated::MissingEvidence)?;
    if observed > LIFECYCLE_OBSERVED_MAX_CEILING {
        return Err(CohortUnvalidated::AboveRepresentableMargin(observed));
    }
    let candidate = observed
        .checked_add(Duration::from_secs(2))
        .ok_or(CohortUnvalidated::Unrepresentable)?;
    Ok(Duration::from_secs(30).min(Duration::from_secs(10).max(candidate)))
}

pub(crate) fn present_event_deadline(mode_period: Option<Duration>) -> Duration {
    let period = mode_period.unwrap_or(UNKNOWN_MODE_PERIOD);
    period
        .checked_mul(2)
        .unwrap_or(Duration::from_millis(500))
        .clamp(Duration::from_millis(50), Duration::from_millis(500))
}
```

`CommitDeadlines::arm_*` uses `Instant::checked_add` and treats `None` as an
immediately-expired deadline rather than panicking.

`CommitDeadlines` holds `hardware: Option<Instant>` and `present: BTreeMap<u32, Instant>`. `arm_hardware` is called from the `Accepted` arm of `on_host_call_outcome`; `arm_present_events` is called at the moment `milestones.hardware_complete` flips true, and only for required Present CRTCs whose event has not already arrived. `expired` returns the hardware expiry first, then the lowest-numbered expired Present CRTC. `tick_deadlines` terminalizes the record as `CompletionUnknown` exactly once (the record's `is_terminal` guard makes the second expiry a no-op), closes readiness and poisons the incarnation.

The producer/acquire timer is deliberately *not* in this type: it runs entirely before device admission, inherits the existing source-specific policy already in the backend, and its failure completes a never-submitted intent locally. `ProducerFailure` therefore lives on the intent, not on `CommitRecord`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/deadline.rs crates/yserver/src/kms/owner/device_owner.rs \
        crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): arm the host-call, hardware and present-event deadlines"
```

---

### Task 14: The qualification gate and readiness

`§10.1`: no synthetic probe. The first required real install/restore commit whose `ExpectedCompletionCrtcs` is non-empty is the qualification commit, and readiness stays closed until it reaches `Completed` with the complete fence evidence.

**Files:**
- Modify: `crates/yserver/src/kms/owner/device_owner.rs`

**Interfaces:**
- Consumes: `DeviceLifecycleState`, `CommitClass::BlockingQualification`, `Milestones`.
- Produces:
  - `KmsDeviceOwner::{qualification_complete, readiness_open, on_commit_completed}`
  - `SubmitError::ClockUnresolved(u32)` (added in task 9) and `SubmitError::AdmissionClosed` are the only refusals a caller sees.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn readiness_stays_closed_until_the_first_real_commit_qualifies_the_incarnation() {
    let mut owner = probed_owner_for_tests(40);
    assert!(!owner.readiness_open());
    assert_eq!(owner.lifecycle_state(), DeviceLifecycleState::Unqualified);
    owner.submit_and_complete_for_tests(request_for_crtc_for_tests(40));
    assert!(owner.readiness_open());
    assert_eq!(owner.lifecycle_state(), DeviceLifecycleState::Ready);
}

#[test]
fn no_synthetic_transition_is_inserted_to_qualify() {
    let mut owner = probed_owner_for_tests(40);
    assert_eq!(owner.dispatch_count_for_tests(), 0, "opening a device submits nothing");
}

#[test]
fn an_empty_expected_set_cannot_qualify_vacuously() {
    let mut owner = probed_owner_for_tests(40);
    owner.submit_and_complete_for_tests(off_to_off_request_for_tests(40));
    assert!(!owner.readiness_open());
    assert_eq!(owner.lifecycle_state(), DeviceLifecycleState::Unqualified);
}

#[test]
fn qualification_requires_every_returned_fence_to_signal() {
    let mut owner = probed_owner_for_tests(40);
    owner.submit_for_tests(request_for_crtc_for_tests(40));
    owner.set_fence_status_for_tests(40, FenceStatus::Error(-libc::EIO));
    owner.poll_fences();
    assert!(!owner.readiness_open());
    assert_eq!(owner.lifecycle_state(), DeviceLifecycleState::Poisoned);
}

#[test]
fn a_completion_breach_after_qualification_closes_readiness_again() {
    let mut owner = qualified_owner_for_tests(40);
    assert!(owner.readiness_open());
    owner.poison(PoisonCause::AcceptanceUnknown);
    assert!(!owner.readiness_open());
}

#[test]
fn a_failed_before_submit_result_never_rewrites_an_advertised_capability_bit() {
    let mut owner = qualified_owner_for_tests(40);
    let advertised = owner.advertised_structural_capability_for_tests();
    owner.submit_and_reject_for_tests(request_for_crtc_for_tests(40), libc::EINVAL);
    assert_eq!(owner.advertised_structural_capability_for_tests(), advertised);
    assert!(!owner.readiness_open() || owner.lifecycle_state() == DeviceLifecycleState::Ready);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::device_owner`
Expected: FAIL — `readiness_open` and the qualification transition do not exist.

- [ ] **Step 3: Write the implementation**

`on_commit_completed(record)` runs when
`record.milestones.completed_for(record.class)` first becomes true. Readiness
opens only when **all** of these hold:

```rust
self.state == DeviceLifecycleState::Unqualified
    && record.is_qualification                       // the explicit property
    && !record.expected_completion.is_empty()
    && record.expected_completion.iter()
           .all(|c| matches!(record.fences[c], FenceSlotState::Signalled))
```

`is_qualification` is set by the caller that issues the mandatory install or
restore commit, and is orthogonal to `CommitClass` because section 10.1 permits
that commit to be nonblocking. Revision 1 opened readiness on any completed
record with a non-empty expected set, so an ordinary cursor or primary commit
could qualify an incarnation after an unowned legacy modeset — which is not the
qualification section 10.1 defines. The admission matrix in task 7 reads the
same property: while `Unqualified`, only a record with `is_qualification` is
admitted at all.

Nothing else opens readiness: there is no bootstrap path, no synthetic flip, no
gamma or cursor transition.

`readiness_open()` is `self.state == DeviceLifecycleState::Ready`. `poison` sets `Poisoned` and therefore closes readiness, but never touches the advertised structural-capability value — that one is computed once during protocol-domain construction and stored outside the owner, which is what `advertised_structural_capability_for_tests` reads (`CAP-1`). `atomic_kms_cursor_policy` is outside this stage entirely: the spec's revision 2 makes it runtime-derived per device identity and consumed by cursor work in stage 4, never by the primary path built here.

Until stage 3 converts modeset, the qualification commit in production is the first converted primary commit after the existing `commit_modeset` path lights the CRTC. Record that with a comment at the transition so stage 3's move of the gate to the real install/restore commit is an obvious, single-site change.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/device_owner.rs
git commit -m "feat(kms): gate readiness on the first real qualification commit"
```

---

### Task 15: Bounded intents, admission tickets and aging

`§9.1` and the ticket half of `§9.2.1`. Cursor and gamma payloads are stage 4, so a maintenance identity here is an opaque `(CRTC, class)` with a generation counter — enough to build and prove the starvation bound now.

**Files:**
- Create: `crates/yserver/src/kms/owner/admission.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks except `SerializedRequest` for the primary payload.
- Produces:
  - `AdmissionTicket(u64)` with `Ord`
  - `MaintenanceClass::{Cursor, Gamma}`, `MaintenanceIdentity { crtc: u32, class: MaintenanceClass }`
  - `MaintenanceIntent { ticket: AdmissionTicket, generation: u64, aged: bool }`
  - `PrimaryIntents { composed: Option<ComposedIntent>, direct_successor: Option<DirectIntent>, barrier: Option<BarrierIntent> }`
  - `AdmissionState::{new, offer_maintenance, offer_composed, offer_direct_successor, offer_barrier, age_unselected, take}`
  - `Displaced { idle_now: Option<PresentSerial>, deferred_skip: Option<PresentSerial> }`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_ready_maintenance_identity_receives_a_ticket_even_with_an_idle_slot() {
    let mut state = AdmissionState::new();
    let ticket = state.offer_maintenance(cursor_on(40), 1);
    assert_eq!(ticket, AdmissionTicket::first());
    assert!(!state.intent_for(cursor_on(40)).unwrap().aged);
}

#[test]
fn latest_wins_replacement_preserves_the_ticket_and_its_original_age() {
    let mut state = AdmissionState::new();
    let first = state.offer_maintenance(cursor_on(40), 1);
    state.offer_maintenance(gamma_on(41), 1);
    let second = state.offer_maintenance(cursor_on(40), 2);
    assert_eq!(first, second, "a newer payload keeps the identity's ticket");
    assert_eq!(state.intent_for(cursor_on(40)).unwrap().generation, 2);
}

#[test]
fn a_maintenance_identity_becomes_aged_without_changing_its_ticket() {
    let mut state = AdmissionState::new();
    let ticket = state.offer_maintenance(cursor_on(40), 1);
    state.age_unselected(&[cursor_on(40)]);
    let intent = state.intent_for(cursor_on(40)).unwrap();
    assert!(intent.aged);
    assert_eq!(intent.ticket, ticket);
}

#[test]
fn admitting_an_identity_consumes_its_ticket_exactly_once() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1);
    assert!(state.take(cursor_on(40)).is_some());
    assert!(state.take(cursor_on(40)).is_none());
    // A newer desired update arriving after admission gets a NEW ticket.
    let fresh = state.offer_maintenance(cursor_on(40), 2);
    assert!(fresh > AdmissionTicket::first());
    assert!(!state.intent_for(cursor_on(40)).unwrap().aged);
}

#[test]
fn a_direct_successor_slot_is_latest_wins_and_hands_back_the_displaced_intent() {
    let mut state = AdmissionState::new();
    assert!(matches!(
        state.offer_direct_successor(direct_intent(PresentSerial(1), /* async */ false)),
        OfferOutcome::Inserted
    ));
    let OfferOutcome::Replaced(old) =
        state.offer_direct_successor(direct_intent(PresentSerial(2), /* async */ true))
    else {
        panic!("the older never-submitted successor must come back owned");
    };
    assert_eq!(old.serial(), PresentSerial(1));
    assert!(old.owns_buffer_and_pins(), "the caller can now release them");
    assert_eq!(state.direct_successor_serial(), Some(PresentSerial(2)));
}

#[test]
fn an_intent_rejected_by_a_barrier_comes_back_owned() {
    let mut state = AdmissionState::new();
    state.offer_barrier(BarrierIntent::Unflip);
    let OfferOutcome::RejectedByBarrier(incoming) =
        state.offer_direct_successor(direct_intent(PresentSerial(7), false))
    else {
        panic!("revision 1 returned None here and leaked the Present");
    };
    assert_eq!(incoming.serial(), PresentSerial(7));
}

#[test]
fn maintenance_offered_behind_a_submitted_commit_is_born_aged() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1, SlotState::Occupied);
    assert!(state.intent_for(cursor_on(40)).unwrap().aged, "spec 9.2.1");
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1, SlotState::Idle);
    assert!(!state.intent_for(cursor_on(40)).unwrap().aged);
}

#[test]
fn an_unflip_barrier_supersedes_unsent_direct_work_and_is_never_superseded() {
    let mut state = AdmissionState::new();
    state.offer_direct_successor(direct_intent(PresentSerial(1), false));
    let displaced = state.offer_barrier(BarrierIntent::Unflip).expect("displaces direct work");
    assert_eq!(displaced.idle_now, Some(PresentSerial(1)));
    assert!(state.direct_successor_serial().is_none());
    // A later primary intent cannot displace the barrier.
    assert!(state.offer_direct_successor(direct_intent(PresentSerial(2), false)).is_none());
    assert!(state.barrier_is_pending());
}

#[test]
fn composed_state_accumulates_damage_rather_than_queueing_frames() {
    let mut state = AdmissionState::new();
    state.offer_composed(composed_intent(scene_generation(1), damage_rect(0, 0, 10, 10)));
    state.offer_composed(composed_intent(scene_generation(2), damage_rect(5, 5, 20, 20)));
    let composed = state.composed_for_tests().expect("one composed desired state");
    assert_eq!(composed.scene_generation, scene_generation(2));
    assert_eq!(composed.accumulated_damage, damage_rect(0, 0, 25, 25));
    assert_eq!(state.composed_queue_len_for_tests(), 1, "never a frame queue");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::admission`
Expected: FAIL.

- [ ] **Step 3: Write the implementation**

`AdmissionState` holds `maintenance: BTreeMap<MaintenanceIdentity, MaintenanceIntent>`, `primary: BTreeMap<u32, PrimaryIntents>`, the round-robin cursor, and `next_ticket: u64` allocated with `checked_add` — exhaustion is an invariant failure, never a wrap, because a wrapped ticket reverses oldest-first order.

`offer_maintenance(identity, generation, slot_state)` takes the owner's slot
state. Spec section 9.2.1: "a cursor or gamma intent is also aged when it
arrives **behind an already submitted commit**." Revision 1 always created a
non-aged intent and aged it only after losing an admission, so maintenance
offered during an in-flight commit could lose one more admission than the bound
permits. An identity offered while the slot is occupied is created **already
aged**. Latest-wins replacement still preserves the ticket and the aged flag.

`take` removes the identity and returns its intent, which is what "consumes its
ticket exactly once" means.

Displacement returns **ownership**, not a serial. `§10.4` requires releasing the
displaced intent's buffer, pins and wake and emitting `IdleNotify` exactly once;
a caller cannot do that from a `PresentSerial`:

```rust
pub(crate) enum OfferOutcome {
    Inserted,
    /// The caller now owns the displaced intent and must release it.
    Replaced(DirectIntent),
    /// A barrier is pending, so this intent was never stored. The caller owns
    /// it back and must terminalize it; revision 1 returned `None` here, which
    /// was indistinguishable from a successful insertion with no displacement
    /// and leaked the incoming Present.
    RejectedByBarrier(DirectIntent),
}
```

`Displaced { idle_now, deferred_skip }` survives only as the *protocol* half
returned alongside the intent, so task 17's ledger keeps its ordering rule.

`ComposedIntent` carries `scene_generation` and `accumulated_damage`; a second offer unions the damage and takes the newer generation rather than appending.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner::admission`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/admission.rs crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): add bounded primary intents and admission tickets"
```

---

### Task 16 `[r2]`: The seven admission tiers, re-derived

Rebuilt, not edited. Revision 1 reduced admission compatibility to
`fn(MaintenanceIdentity, u32) -> bool`, which cannot inspect the closure,
generations, completion coverage or synchronous class the tiers are defined
over; implemented tier 3's round-robin rule backwards; had no primary age for
tiers 4, 6 and 7; used `AdmissionChoice::Maintenance` as a tuple variant in one
test and a struct variant in another; and left the round-robin cursor to the
caller, so production could admit one CRTC forever.

`DispatchTimingPolicy::ImmediateOnRetirement` is fixed for C.0: when retirement
makes work eligible the owner runs admission in that wake and dispatches with no
retention timer.

**Files:**
- Modify: `crates/yserver/src/kms/owner/admission.rs`
- Modify: `crates/yserver/src/kms/owner/device_owner.rs`

**Interfaces:**
- Consumes: everything from task 15; `SerializedRequest` and the builder from task 5.
- Produces:
  - `AdmissionCandidate` as defined in the revision 2 architecture section
  - `AdmissionChoice` — **all struct variants**, no tuples
  - `AdmissionState::select(&mut self) -> Option<AdmissionChoice>` — takes no context; the scheduler owns its state
  - `HomogeneousGroup` and `RoundRobin`

- [ ] **Step 1: Write the failing tests**

```rust
// Every candidate is built by the request builder, so it carries the real
// closure and generations rather than a caller's opinion.
fn candidate(crtc: u32, kind: PrimaryKind, absorbs: &[MaintenanceIdentity]) -> AdmissionCandidate;

#[test]
fn a_waiting_topology_barrier_wins_every_other_tier() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1, SlotState::Idle);
    state.offer_primary(candidate(40, PrimaryKind::Composed, &[]));
    state.offer_barrier(BarrierIntent::Topology);
    assert!(matches!(state.select(), Some(AdmissionChoice::Barrier { .. })));
}

#[test]
fn tier_two_admits_an_unflip_or_software_cursor_recovery_before_any_primary() {
    for barrier in [BarrierIntent::Unflip, BarrierIntent::SoftwareCursorRecovery] {
        let mut state = AdmissionState::new();
        state.offer_primary(candidate(40, PrimaryKind::Composed, &[]));
        state.offer_barrier(barrier);
        assert!(matches!(state.select(), Some(AdmissionChoice::Recovery { .. })));
    }
}

#[test]
fn tier_three_is_blocked_when_a_DIFFERENT_crtc_is_owed_the_turn() {
    // Revision 1 had this backwards: it blocked the successor whose own CRTC
    // was owed. Spec 9.2.1: the successor is eligible when its CRTC is
    // permitted, and yields when another CRTC is owed.
    let mut state = AdmissionState::new();
    state.offer_primary(candidate(40, PrimaryKind::Composed, &[]));
    state.offer_primary(candidate(41, PrimaryKind::Composed, &[]));
    let first = state.select().expect("first");
    let first_crtc = primary_crtc(&first);
    let other = if first_crtc == 40 { 41 } else { 40 };

    state.offer_direct_successor_candidate(candidate(first_crtc, PrimaryKind::DirectSuccessor, &[]));
    // `other` is owed, so the successor on `first_crtc` may not take tier 3.
    assert!(!matches!(state.select(), Some(AdmissionChoice::RetirementSuccessor { .. })));

    let mut state = AdmissionState::new();
    state.offer_direct_successor_candidate(candidate(40, PrimaryKind::DirectSuccessor, &[]));
    // Nothing else is ready, so no CRTC is owed and tier 3 applies.
    assert!(matches!(state.select(), Some(AdmissionChoice::RetirementSuccessor { .. })));
}

#[test]
fn tier_three_requires_absorbing_every_aged_identity_that_would_otherwise_win() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1, SlotState::Occupied);   // born aged
    state.offer_direct_successor_candidate(candidate(40, PrimaryKind::DirectSuccessor, &[]));
    assert!(matches!(
        state.select(),
        Some(AdmissionChoice::AgedMaintenance { identity, .. }) if identity == cursor_on(40)
    ));

    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1, SlotState::Occupied);
    state.offer_direct_successor_candidate(
        candidate(40, PrimaryKind::DirectSuccessor, &[cursor_on(40)]),
    );
    let Some(AdmissionChoice::RetirementSuccessor { absorbed, .. }) = state.select() else {
        panic!("an absorbing successor takes tier 3");
    };
    assert_eq!(absorbed, vec![cursor_on(40)]);
    assert!(state.intent_for(cursor_on(40)).is_none(), "the absorbed ticket is consumed");
}

#[test]
fn a_candidate_that_lacks_completion_coverage_is_never_admitted() {
    let mut state = AdmissionState::new();
    let mut c = candidate(40, PrimaryKind::Composed, &[]);
    c.completion_covered = false;
    state.offer_primary(c);
    assert!(state.select().is_none(), "no canonical completion, no admission");
}

#[test]
fn n_incompatible_aged_identities_meet_the_specified_bound() {
    // Each of N is admitted after at most the already-submitted commit plus
    // N-1 older-ticket maintenance admissions. Model the submitted commit.
    let identities = [cursor_on(40), gamma_on(40), cursor_on(41), gamma_on(41)];
    let mut state = AdmissionState::new();
    for id in identities {
        state.offer_maintenance(id, 1, SlotState::Occupied);
    }
    let mut intervening = std::collections::HashMap::new();
    let mut seen = 0usize;
    while let Some(choice) = state.select() {
        for id in identities {
            if state.intent_for(id).is_some() {
                *intervening.entry(id).or_insert(0usize) += 1;
            }
        }
        assert!(matches!(choice, AdmissionChoice::AgedMaintenance { .. }));
        seen += 1;
    }
    assert_eq!(seen, identities.len());
    for id in identities {
        assert!(intervening[&id] <= identities.len() - 1, "{id:?} waited too long");
    }
}

#[test]
fn maintenance_absorbs_a_compatible_ready_primary_on_the_same_crtc() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1, SlotState::Idle);
    state.offer_primary(candidate(40, PrimaryKind::Composed, &[cursor_on(40)]));
    let Some(AdmissionChoice::Maintenance { absorbed_primary: Some(crtc), .. }) = state.select()
    else {
        panic!("symmetric absorption");
    };
    assert_eq!(crtc, 40);
}

#[test]
fn maintenance_absorption_never_crosses_an_unflip_barrier() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1, SlotState::Idle);
    state.offer_primary(candidate(40, PrimaryKind::Composed, &[cursor_on(40)]));
    state.offer_barrier(BarrierIntent::Unflip);
    assert!(matches!(state.select(), Some(AdmissionChoice::Recovery { .. })));
}

#[test]
fn tier_five_includes_every_ready_crtc_of_the_group_not_merely_two() {
    let mut state = AdmissionState::with_group(HomogeneousGroup::of(&[40, 41, 42]));
    for crtc in [40, 41, 42] {
        state.offer_primary(candidate(crtc, PrimaryKind::Composed, &[]));
    }
    let Some(AdmissionChoice::Bundle { crtcs, .. }) = state.select() else {
        panic!("tier 5");
    };
    assert_eq!(crtcs, vec![40, 41, 42], "spec 9.2.1: every ready CRTC in the group");
}

#[test]
fn one_ready_crtc_never_waits_on_a_timer_for_a_missing_bundle_member() {
    let mut state = AdmissionState::with_group(HomogeneousGroup::of(&[40, 41]));
    state.offer_primary(candidate(40, PrimaryKind::Composed, &[]));
    assert!(matches!(state.select(), Some(AdmissionChoice::Primary { crtc: 40, .. })));
}

#[test]
fn tier_six_takes_the_oldest_ready_primary() {
    let mut state = AdmissionState::new();
    state.offer_primary(candidate(41, PrimaryKind::Composed, &[]));   // older ticket
    state.offer_primary(candidate(40, PrimaryKind::Composed, &[]));
    assert!(matches!(state.select(), Some(AdmissionChoice::Primary { crtc: 41, .. })));
}

#[test]
fn the_scheduler_advances_its_own_round_robin() {
    // Revision 1's test set `owed_crtc` by hand between selections, so it
    // proved only that select obeys a correctly prepared mock.
    let mut state = AdmissionState::new();
    let mut admitted = Vec::new();
    for _ in 0..4 {
        state.offer_primary(candidate(40, PrimaryKind::Composed, &[]));
        state.offer_primary(candidate(41, PrimaryKind::Composed, &[]));
        admitted.push(primary_crtc(&state.select().expect("admission")));
    }
    assert!(
        admitted.windows(2).all(|w| w[0] != w[1]),
        "a continuously ready CRTC took two successive slots: {admitted:?}"
    );
}

#[test]
fn a_topology_transition_keeps_remappable_ticket_age_and_drops_the_rest() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1, SlotState::Idle);
    let ticket = state.intent_for(cursor_on(40)).unwrap().ticket;
    state.offer_maintenance(cursor_on(99), 1, SlotState::Idle);
    state.remap_topology(&TopologyRemap::keeping(&[40]));
    assert_eq!(state.intent_for(cursor_on(40)).unwrap().ticket, ticket);
    assert!(state.intent_for(cursor_on(99)).is_none(), "unremappable tickets drop");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::admission`
Expected: FAIL — `AdmissionCandidate`, `HomogeneousGroup` and the context-free `select` do not exist.

- [ ] **Step 3: Write the implementation**

```rust
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum AdmissionChoice {
    Barrier { intent: BarrierIntent },
    Recovery { intent: BarrierIntent },
    RetirementSuccessor { crtc: u32, candidate: AdmissionCandidate, absorbed: Vec<MaintenanceIdentity> },
    AgedMaintenance { identity: MaintenanceIdentity, absorbed_primary: Option<u32> },
    Bundle { crtcs: Vec<u32>, candidates: Vec<AdmissionCandidate> },
    Primary { crtc: u32, candidate: AdmissionCandidate },
    Maintenance { identity: MaintenanceIdentity, absorbed_primary: Option<u32> },
}
```

Every variant is a struct variant, so a test and an implementation cannot
disagree about its shape.

`select(&mut self)` evaluates the tiers strictly in order and returns on the
first match. It consults only its own state — the homogeneous group and the
round-robin cursor are fields, not arguments — and it **advances** the cursor
whenever it admits a primary, singular or bundled.

The three predicates the tiers are defined over come from the candidate, not
from a caller's boolean:

- `candidate.absorbs` lists the exact `(identity, generation)` pairs the
  serialized request already contains, produced by the request builder when it
  merged those generations. Tier 3's eligibility is
  `aged_that_would_otherwise_win ⊆ candidate.absorbs`.
- `candidate.completion_covered` is false when any CRTC in `candidate.closure`
  lacks canonical out-fence coverage. Such a candidate is never admitted in any
  tier.
- `candidate.offered` is the ticket the primary received when offered, giving
  tiers 6 and 7 their "oldest".

The round-robin rule, stated once because revision 1 inverted it: a CRTC is
*owed* when it has a ready primary and was not the most recently admitted
primary CRTC. Tier 3 is eligible when **no other** CRTC is owed; it is not
blocked by its own CRTC being owed. Tiers 5 and 6 win when another CRTC is owed.

Aging happens in exactly one place: when `select` admits something while ready
maintenance identities remain unsent, it marks those identities aged before
returning. Their tickets are untouched.

`remap_topology` keeps every ticket whose identity survives the remap, with its
original value, and drops only the unremappable ones — surviving desired
protocol state retains its relative age.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/admission.rs crates/yserver/src/kms/owner/device_owner.rs
git commit -m "feat(kms): re-derive the seven admission tiers from the spec"
```

---

### Task 17: Present and release terminalization

`§10.4`. The merged baseline already has the shape of this in `ScanoutM2State` (`deferred_successor_skips`, `idled`); this task lifts the rules into the owner so they hold for every terminal path, not only for the direct-successor one.

**Files:**
- Create: `crates/yserver/src/kms/owner/terminalize.rs`
- Modify: `crates/yserver/src/kms/owner/device_owner.rs`

**Interfaces:**
- Consumes: `Displaced` (task 13), `CommitRecord`, `ClockSample` (task 10).
- Produces:
  - `TerminalizationLedger::{new, record_displaced_successor, complete_accepted_without_presented, publish_deferred_skips, note_prior_buffer_released, is_terminalized}`
  - `ProtocolCompletion::{Flip { msc, ust_us }, Skip { msc, ust_us }}`
  - `ReleasePoint::{PriorBufferReleased, TeardownBarrier}`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_displaced_successor_idles_immediately_and_defers_only_its_skip() {
    let mut ledger = TerminalizationLedger::new();
    ledger.record_displaced_successor(PresentSerial(1), PresentSerial(0) /* in-flight */);
    assert_eq!(ledger.idle_events_for_tests(), vec![PresentSerial(1)]);
    assert!(ledger.publish_deferred_skips(PresentSerial(9)).is_empty(), "wrong predecessor");
    assert_eq!(
        ledger.publish_deferred_skips(PresentSerial(0)),
        vec![PresentSerial(1)]
    );
}

#[test]
fn the_idle_event_is_not_re_emitted_with_the_deferred_skip() {
    let mut ledger = TerminalizationLedger::new();
    ledger.record_displaced_successor(PresentSerial(1), PresentSerial(0));
    ledger.take_idle_events_for_tests();
    ledger.publish_deferred_skips(PresentSerial(0));
    assert!(ledger.take_idle_events_for_tests().is_empty());
}

#[test]
fn repeated_replacement_cannot_duplicate_either_half() {
    let mut ledger = TerminalizationLedger::new();
    for serial in 1..=5u64 {
        ledger.record_displaced_successor(PresentSerial(serial), PresentSerial(0));
    }
    assert_eq!(ledger.idle_events_for_tests().len(), 5);
    let skips = ledger.publish_deferred_skips(PresentSerial(0));
    assert_eq!(skips.len(), 5);
    assert!(ledger.publish_deferred_skips(PresentSerial(0)).is_empty());
}

#[test]
fn an_accepted_present_without_presented_completes_once_as_skip_with_the_last_sample() {
    let mut ledger = TerminalizationLedger::new();
    let last = ClockSample { msc: 4242, ust_us: 99 };
    let completion = ledger
        .complete_accepted_without_presented(PresentSerial(7), Some(last))
        .expect("one completion");
    assert_eq!(completion, ProtocolCompletion::Skip { msc: 4242, ust_us: 99 });
    assert!(
        ledger.complete_accepted_without_presented(PresentSerial(7), Some(last)).is_none(),
        "an accepted pending predecessor cannot be completed a second time"
    );
}

#[test]
fn a_skip_without_a_validated_sample_reports_no_clock_rather_than_zero() {
    // Zero is a fabricated MSC/UST and is also a legal real value, so a client
    // cannot distinguish it from a genuine sample.
    let mut ledger = TerminalizationLedger::new();
    let t = ledger
        .complete_accepted_without_presented(protocol_key_for_tests(7), None)
        .expect("one completion");
    assert_eq!(t.completion, ProtocolCompletion::SkipWithoutClock);
    assert!(t.unpark_fifo, "the FIFO unparks either way");
}

#[test]
fn a_suppressed_notification_still_unparks_the_client_fifo() {
    let mut ledger = TerminalizationLedger::new();
    ledger.mark_drawable_dead_for_tests(protocol_key_for_tests(7));
    let t = ledger
        .complete_accepted_without_presented(protocol_key_for_tests(7), Some(sample_for_tests()))
        .expect("completion");
    assert_eq!(t.notify, NotifyDisposition::SuppressDeadDrawable);
    assert!(t.unpark_fifo);
}

#[test]
fn a_reused_buffer_handle_cannot_match_an_older_generations_release() {
    let mut ledger = TerminalizationLedger::new();
    ledger.record_accepted(protocol_key_for_tests(7), CommitId::for_tests(1), BufferRef(11));
    ledger.invalidate_generation();
    ledger.record_accepted(protocol_key_for_tests(8), CommitId::for_tests(2), BufferRef(11));
    ledger.note_prior_buffer_released(CommitId::for_tests(1), BufferRef(11));
    assert!(
        ledger.released_buffers_for_tests().is_empty(),
        "the old generation's release must not signal the new entry"
    );
}

#[test]
fn an_accepted_commit_withholds_idle_and_release_until_prior_buffer_released() {
    let mut ledger = TerminalizationLedger::new();
    ledger.record_accepted(PresentSerial(7), BufferRef(11));
    ledger.complete_accepted_without_presented(PresentSerial(7), None);
    assert!(ledger.released_buffers_for_tests().is_empty(), "Present completion is not idleness");
    ledger.note_prior_buffer_released(BufferRef(11));
    assert_eq!(ledger.released_buffers_for_tests(), vec![BufferRef(11)]);
}

#[test]
fn protocol_completion_idle_release_and_quarantine_are_keyed_separately() {
    let mut ledger = TerminalizationLedger::new();
    ledger.record_accepted(PresentSerial(7), BufferRef(11));
    ledger.complete_accepted_without_presented(PresentSerial(7), None);
    // A device rebuild must not inherit or signal the old release point.
    ledger.invalidate_generation();
    ledger.note_prior_buffer_released(BufferRef(11));
    assert!(ledger.released_buffers_for_tests().is_empty());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::terminalize`
Expected: FAIL.

- [ ] **Step 3: Write the implementation**

`TerminalizationLedger` holds three independently keyed maps. The keys are the
ones that are actually unique: a client-supplied `PresentSerial` is not, and a
`BufferRef` can be reused by several Presents and generations, so revision 1's
keys could match a late release against a newer entry.

```rust
/// Unique across the server: the monotonic present id the completion carrier
/// already carries, plus the client and window lifetime it belongs to.
#[derive(Hash, Eq, PartialEq, Clone, Copy)]
pub(crate) struct ProtocolKey {
    client: ClientId,
    present_id: u64,
    window_generation: u64,
}

/// Release is keyed by the commit that owned the buffer, not by the handle.
#[derive(Hash, Eq, PartialEq, Clone, Copy)]
pub(crate) struct ReleaseKey {
    commit: CommitId,
    device_generation: DeviceGeneration,
    buffer: BufferRef,
}

protocol: HashMap<ProtocolKey, ProtocolCompletion>,
deferred_skips: Vec<(ProtocolKey, ProtocolKey)>,   // (waiter, predecessor)
release: HashMap<ReleaseKey, ReleasePoint>,
```

`record_displaced_successor(displaced: DirectIntent, predecessor: ProtocolKey)`
takes the owned intent from task 15's `OfferOutcome::Replaced`, releases its
buffer, pins and wake, emits `IdleNotify` once, and records the deferred `Skip`
behind `predecessor`. Revision 1's interface said it consumed `Displaced` while
its tests passed two serials and its prose described a third shape; there is one
signature and the tests use it.

`record_displaced_successor(displaced, predecessor)` pushes the idle event immediately (the buffer, pins and wake are released by the caller at the same moment) and records the deferred `Skip` behind `predecessor`. `publish_deferred_skips(predecessor)` drains only the entries waiting on that predecessor.

`complete_accepted_without_presented` inserts into `protocol` only if absent,
returning `None` on the second call. Its clock comes from the last validated
CRTC sample. With **no** validated sample it returns
`ProtocolCompletion::SkipWithoutClock`, never `Skip { msc: 0, ust_us: 0 }`:
`§10.4` says such a completion "never fabricates a new MSC/UST", and zero is
both fabricated and a legal real clock value, so a client cannot tell it from a
genuine sample.

Every terminal path also reports two things the spec requires and revision 1
omitted:

```rust
pub(crate) struct Terminalization {
    pub(crate) completion: ProtocolCompletion,
    /// Follows normal drawable/client-liveness rules. Suppression does not
    /// change the next field.
    pub(crate) notify: NotifyDisposition,   // Send | SuppressDeadDrawable
    /// The per-client FIFO is unparked in EITHER case (§10.4).
    pub(crate) unpark_fifo: bool,           // always true
}
```

`ReleasePoint::TeardownBarrier` gains its implementation here rather than being
a named variant with no producer: `release_at_teardown_barrier(generation)`
drops every release record of that generation and reports which buffers it
freed, so a device rebuild cannot inherit or signal an old generation's release
point.

In `device_owner.rs`, every terminal path — `CompletionUnknown`, poison, quiesce, and the primary-event deadline — routes through this ledger so a Present intent always reaches a protocol terminal state even when its KMS commit does not.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/terminalize.rs crates/yserver/src/kms/owner/device_owner.rs \
        crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): terminalize Present, idle and release through one owner ledger"
```

---

### Task 18: Convert composed primary submission and remove the live input fence

Three things happen here. `submit_flip_with_fences` and `submit_composed_scanout` stop calling `Device::atomic_commit` and become request builders. And `COMMIT-4` is enforced: the copied-scanout path currently hands its copy-completion fence to KMS as `IN_FENCE_FD`, which C.0 forbids — the producer must complete in an asynchronous pre-submit wait, before admission.

**Files:**
- Modify: `crates/yserver/src/drm/page_flip.rs:126-186` (`submit_flip_with_fences` → `build_composed_flip_request`)
- Modify: `crates/yserver/src/drm/modeset.rs:1690` (`submit_composed_scanout` → `build_composed_scanout_request`)
- Modify: `crates/yserver/src/kms/render/platform.rs:5163` (`submit_copied_scanout`)
- Modify: `crates/yserver/src/kms/render/scene.rs:6769`
- Modify: `crates/yserver/src/kms/render/backend.rs:2234`

**Interfaces:**
- Consumes: `AtomicRequestBuilder`, `Signaling`, `SerializedRequest` (task 4); `KmsDeviceOwner::submit` (task 6); `CommitClass` (task 5).
- Produces:
  - `drm::page_flip::build_composed_flip_request(output: &Output, fb_id: framebuffer::Handle) -> AtomicRequestBuilder`
  - `drm::modeset::build_composed_scanout_request(planes: &[ComposedScanoutPlaneState<'_>]) -> Result<AtomicRequestBuilder, io::Error>`
  - `PlatformBackend::submit_copied_scanout` keeps its signature but waits on `render_completion` before admission.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_composed_flip_request_carries_no_in_fence_property() {
    let output = output_for_tests();
    let builder = build_composed_flip_request(&output, framebuffer_for_tests());
    let request = builder
        .finish(Signaling { page_flip_event: true }, &out_fence_props_for(&output))
        .expect("serialize");
    assert!(
        !request_contains_prop(&request, output.plane_in_fence_fd_prop.unwrap()),
        "C.0 hands the kernel no unresolved producer fence"
    );
}

#[test]
fn a_composed_flip_request_carries_exactly_one_out_fence_for_its_crtc() {
    let output = output_for_tests();
    let request = build_composed_flip_request(&output, framebuffer_for_tests())
        .finish(Signaling { page_flip_event: true }, &out_fence_props_for(&output))
        .expect("serialize");
    assert_eq!(
        request.out_fence_slots.iter().map(|s| s.crtc_id).collect::<Vec<_>>(),
        vec![u32::from(output.crtc)]
    );
    assert_eq!(request.expected_completion, BTreeSet::from([u32::from(output.crtc)]));
}

#[test]
fn a_composed_flip_request_carries_a_nonzero_event_token_not_zero_user_data() {
    let mut owner = qualified_owner_for_tests(40);
    owner.submit_for_tests(composed_flip_request_for_tests(40));
    let dispatched = owner.last_dispatched_request_for_tests();
    assert_ne!(dispatched.event_token.as_user_data(), 0);
    assert_eq!(dispatched.event_token, owner.pending_token_for_tests());
}

#[test]
fn producer_success_releases_the_wait_exactly_once_before_admission() {
    let mut backend = platform_backend_for_tests();
    let fence = signalled_fence_for_tests();
    let raw = fence.as_raw_fd();
    backend.submit_copied_scanout_for_tests(0, 0, Some(fence));
    assert_eq!(unsafe { libc::close(raw) }, -1, "the producer fence was closed exactly once");
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
    assert_eq!(backend.owner_dispatch_count_for_tests(), 1);
}

#[test]
fn a_producer_error_never_calls_the_atomic_ioctl_and_occupies_no_slot() {
    let mut backend = platform_backend_for_tests();
    let result = backend.submit_copied_scanout_for_tests(0, 0, Some(errored_fence_for_tests()));
    assert!(result.is_err());
    assert_eq!(backend.owner_dispatch_count_for_tests(), 0);
    assert!(backend.owner_slot_is_free_for_tests());
}

#[test]
fn an_atomic_rejection_preserves_the_released_but_atomic_rejected_recovery() {
    let mut backend = platform_backend_with_rejecting_owner_for_tests(libc::EINVAL);
    let _ = backend.submit_copied_scanout_for_tests(0, 0, None);
    assert_eq!(
        backend.copied_destination_state_for_tests(0, 0),
        CopiedDestinationState::ReleasedButAtomicRejected,
        "KMS returning nothing is not an ownership return"
    );
}

#[test]
fn no_composed_submission_path_calls_device_atomic_commit() {
    // Guard against a future reintroduction: the two converted helpers must
    // not name the crate wrapper at all.
    let page_flip = include_str!("../../drm/page_flip.rs");
    let modeset = include_str!("../../drm/modeset.rs");
    assert!(!page_flip.contains("atomic_commit"));
    assert!(!modeset[modeset.find("fn build_composed_scanout_request").unwrap()..].contains("atomic_commit"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver composed_flip`
Expected: FAIL — the builders do not exist and `submit_copied_scanout` still passes `IN_FENCE_FD`.

- [ ] **Step 3: Write the implementation**

`build_composed_flip_request` keeps the existing property set — plane `FB_ID`, plane `CRTC_ID` — but declares each through `add_plane_property` with its old and new CRTC binding so the closure is computed, calls `declare_crtc_active(crtc, true, true)` and `declare_present_consumer(crtc)`. It never adds `IN_FENCE_FD` and never adds `OUT_FENCE_PTR` itself; `finish` owns the out-fence entries.

`submit_copied_scanout` changes shape:

```rust
// COMMIT-4: the copy fence is resolved here, before admission. The old path
// handed it to KMS as IN_FENCE_FD; C.0 forbids an unresolved producer fence
// crossing the ioctl, and the pre-submit wait must not block the core.
let copy_completion = copied.submit_copy(bo_idx, render_completion)?;
match self.producer_wait.poll(copy_completion) {
    ProducerPoll::Pending(wait) => {
        // Park the intent; the event loop re-enters this function when the
        // fence becomes readable. No device slot is taken and no KMS call
        // has been made.
        destination.state.transition_to_awaiting_producer(wait);
        return Ok(());
    }
    ProducerPoll::Ready => {
        // The wait is released exactly once here and `ProducerReady` is set
        // on the intent before it is offered to admission.
    }
    ProducerPoll::Failed(error) => {
        destination.state.transition_to_recording_after_producer_failure();
        copied.recover_copy_failure(bo_idx)?;
        return Err(error);
    }
}
```

then builds the request and calls `owner.submit(request, CommitClass::NonblockingPrimaryPresent)`. The rejection arm keeps the existing `ReleasedButAtomicRejected` transition verbatim — `§10.2` requires that copied/direct BO state follow the atomic-rejected recovery and not be reset as though KMS returned FOREIGN ownership. The out-fence handling that used to live here is gone: the owner adopts the fences.

`ProducerWait` is a small helper on `PlatformBackend` reusing the existing readability registration the backend already performs for scanout fences; it inherits the source-specific policy and introduces no universal 200 ms timeout.

`build_composed_scanout_request` is the same transformation applied to `submit_composed_scanout`'s per-plane loop, declaring `(old, new)` bindings for every plane and `declare_crtc_active(crtc, true, true)` for every affected CRTC.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/drm/page_flip.rs crates/yserver/src/drm/modeset.rs \
        crates/yserver/src/kms/render/platform.rs crates/yserver/src/kms/render/scene.rs \
        crates/yserver/src/kms/render/backend.rs
git commit -m "feat(kms): route composed primary submission through the owner without an input fence"
```

---

### Task 19: Convert direct scanout, its `TEST_ONLY` validation, and retirement-time successor promotion

The last three of the six baseline call sites. `§12`: `submit_direct_scanout` becomes a `§6.3` owner transaction with exact event identity and canonical out-fence evidence, and retirement-time successor promotion enters through tier 3 rather than issuing an atomic commit from the event handler.

**Files:**
- Modify: `crates/yserver/src/drm/modeset.rs:1562` (direct-scanout `TEST_ONLY` probe) and `:1635` (`submit_direct_scanout`)
- Modify: `crates/yserver/src/kms/render/backend.rs:1831,1843,1892,1915`
- Modify: `crates/yserver/src/kms/owner/device_owner.rs` (validation lease)

**Interfaces:**
- Consumes: everything from tasks 4, 6, 13, 14, 15.
- Produces:
  - `drm::modeset::build_direct_scanout_request(fb, planes) -> Result<AtomicRequestBuilder, io::Error>`
  - `drm::modeset::build_direct_scanout_validation(fb, planes) -> Result<AtomicRequestBuilder, io::Error>`
  - `KmsDeviceOwner::{validate, take_validation_lease, release_validation_lease}` with `ValidationLease` and `AtomicSnapshotId`
  - `KmsBackend::on_direct_retirement(&mut self)` driving tier 3 promotion.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_direct_scanout_request_carries_a_fresh_token_and_the_canonical_out_fence_set() {
    let mut owner = qualified_owner_for_tests(40);
    owner.submit_for_tests(direct_scanout_request_for_tests(&[40, 41]));
    let request = owner.last_dispatched_request_for_tests();
    assert_ne!(request.event_token.as_user_data(), 0);
    assert_eq!(
        request.out_fence_slots.iter().map(|s| s.crtc_id).collect::<BTreeSet<_>>(),
        BTreeSet::from([40, 41])
    );
}

#[test]
fn validation_only_creates_no_live_record_and_holds_the_exclusive_lease() {
    let mut owner = qualified_owner_for_tests(40);
    let lease = owner.take_validation_lease().expect("lease");
    assert!(owner.take_validation_lease().is_none(), "the lease is exclusive");
    owner.validate(direct_validation_request_for_tests(40), &lease).expect("validate");
    assert!(owner.slot_is_free(), "ValidationOnly occupies no submitted slot");
    assert!(owner.pending_record_for_tests().is_none());
    owner.release_validation_lease(lease);
    assert!(owner.take_validation_lease().is_some());
}

#[test]
fn validation_uses_the_seat_active_watchdog_and_timeout_is_not_acceptance_unknown() {
    let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
        ScriptedOutcome::Unknown(UnknownReason::WatchdogExpired),
    ]);
    let lease = owner.take_validation_lease().expect("lease");
    let result = owner.validate(direct_validation_request_for_tests(40), &lease);
    assert_eq!(result, Err(ValidateError::SnapshotInvalidated));
    assert_ne!(
        owner.lifecycle_state(),
        DeviceLifecycleState::Poisoned,
        "no live mutation was requested, so hardware state is not unknown"
    );
    assert_eq!(owner.last_host_call_class_for_tests(), HostCallClass::SeatActiveValidation);
}

#[test]
fn an_unchanged_topology_generation_cannot_authorize_a_live_install_after_a_generation_change() {
    let mut owner = qualified_owner_for_tests(40);
    let lease = owner.take_validation_lease().expect("lease");
    let snapshot = owner.validate(direct_validation_request_for_tests(40), &lease).expect("validate");
    owner.bump_primary_generation_for_tests(40);
    assert_eq!(
        owner.install_validated(snapshot, direct_scanout_request_for_tests(&[40])),
        Err(InstallError::SnapshotStale)
    );
}

#[test]
fn retirement_promotion_enters_the_owner_tier_and_never_commits_from_the_event_handler() {
    let mut backend = kms_backend_with_direct_pending_and_successor_for_tests();
    backend.on_direct_retirement();
    assert_eq!(backend.owner_admission_choices_for_tests(), vec![AdmissionChoiceKind::RetirementSuccessor]);
    assert_eq!(backend.direct_commits_issued_outside_the_owner_for_tests(), 0);
}

#[test]
fn retirement_promotion_preserves_the_immediate_dispatch_instant() {
    let mut backend = kms_backend_with_direct_pending_and_successor_for_tests();
    let retired_at = Instant::now();
    backend.on_direct_retirement_at(retired_at);
    assert_eq!(
        backend.owner_dispatch_instant_for_tests(),
        Some(retired_at),
        "ImmediateOnRetirement adds no retention margin"
    );
}

#[test]
fn the_predecessor_completion_is_published_before_the_deferred_successor_skips() {
    let mut backend = kms_backend_with_displaced_successor_for_tests();
    backend.on_direct_retirement();
    let published = backend.published_present_events_for_tests();
    assert_eq!(published[0].kind, PresentEventKind::Complete);
    assert!(published[1..].iter().all(|e| e.kind == PresentEventKind::Skip));
}

#[test]
fn a_never_submitted_successor_releases_and_idles_immediately_on_replacement() {
    let mut backend = kms_backend_with_direct_pending_and_successor_for_tests();
    let first_pin = backend.queued_successor_pin_for_tests().expect("pin");
    backend.offer_direct_successor_for_tests(newer_direct_frame_for_tests());
    assert!(!backend.pin_is_held_for_tests(first_pin));
    assert_eq!(backend.idle_events_for_tests().len(), 1);
    assert!(backend.published_present_events_for_tests().is_empty(), "the Skip is deferred");
}

#[test]
fn warframe_shaped_producer_pressure_does_not_exhaust_client_buffers() {
    let mut backend = kms_backend_with_direct_pending_and_successor_for_tests();
    for _ in 0..1000 {
        backend.offer_direct_successor_for_tests(newer_direct_frame_for_tests());
    }
    assert_eq!(backend.queued_successor_count_for_tests(), 1);
    assert_eq!(backend.held_pin_count_for_tests(), 2, "one in flight, one queued");
}

#[test]
fn direct_entry_attaches_the_current_cursor_state_or_proves_the_submitted_state_valid() {
    let backend = kms_backend_with_bound_cursor_for_tests();
    assert!(
        backend.direct_entry_cursor_precondition_for_tests(),
        "direct entry must not drop the cursor plane"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver direct_scanout`
Expected: FAIL.

- [ ] **Step 3: Write the implementation**

`build_direct_scanout_request` mirrors the existing per-plane loop but through `add_plane_property` with `(old, new)` bindings, `declare_crtc_active(crtc, true, true)` and `declare_present_consumer(crtc)` per affected output. `build_direct_scanout_validation` builds the identical persistent property set and is finished with `Signaling { page_flip_event: false }` and an empty out-fence map — `§5`: the final `TEST_ONLY` and live request must contain identical DRM objects, framebuffer ids, routing, modes and geometry, while the ephemeral synchronization properties are freshly built for the live ioctl.

`ValidationLease` is a non-`Clone` token; `take_validation_lease` returns `None` while one is outstanding. `validate` dispatches with `HostCallClass::SeatActiveValidation` and `AtomicCommitFlags::TEST_ONLY`, allocates no `CommitId`, installs no record, reserves no slot and adopts no fence. On success it returns an `AtomicSnapshotId` carrying the device, lifecycle, topology, primary, cursor, gamma, connector and CRTC desired generations; `install_validated` refuses with `InstallError::SnapshotStale` if any of them changed. A watchdog expiry invalidates the snapshot and does **not** poison, because no live mutation was requested.

In `backend.rs`, the direct submission at `:1831` builds a request and calls `owner.submit(request, CommitClass::NonblockingPrimaryPresent)`. `promote_queued_successor` at `:1843` no longer submits: it offers the successor to `AdmissionState::offer_direct_successor` and lets `on_direct_retirement` run `select` in the retirement wake. The existing supersession at `:1892` calls `TerminalizationLedger::record_displaced_successor` instead of hand-managing `deferred_successor_skips`, and `:1915` (`take` on invalidation) routes both halves through the same ledger so topology invalidation completes or rejects every never-submitted intent through the same split before dropping resources.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/drm/modeset.rs crates/yserver/src/kms/render/backend.rs \
        crates/yserver/src/kms/owner/device_owner.rs
git commit -m "feat(kms): route direct scanout, its validation and successor promotion through the owner"
```

---

### Task 20: Drive the damage transaction from owner milestones

The merged damage tracker stages "after the submit succeeded" and applies at `on_page_flip_complete`. Under C.0 neither event exists in that form: submission crosses IPC, and retirement splits into `HardwareComplete` and `Presented`. `DMG-1` and `DMG-2` make the re-anchoring normative.

**Files:**
- Modify: `crates/yserver/src/kms/owner/device_owner.rs`
- Modify: `crates/yserver/src/kms/render/scene.rs:1978` (apply site) and `:4344` (stage site)
- Modify: `crates/yserver/src/kms/render/backend.rs`

**Interfaces:**
- Consumes: `Milestones`, `CommitState`, `CommitId` (task 5); `KmsDeviceOwner` milestones (tasks 6, 7, 8).
- Produces:
  - `DamageEvent::{Accepted(CommitId), HardwareComplete(CommitId), Unknown(CommitId)}`
  - `KmsDeviceOwner::take_damage_events(&mut self) -> Vec<DamageEvent>`
  - `DamageStageEntry { output_idx: usize, bo_idx: usize, repaint: Region, painted: Region, complete: bool }`
  - `PendingDamageStage { commit: CommitId, entries: Vec<DamageStageEntry> }`
  - `KmsBackend::{hold_damage_stage, resolve_damage_events}`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn nothing_stages_while_the_commit_is_only_submitting() {
    let mut backend = damage_backend_for_tests();
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0)]));
    backend.owner_install_submitting_for_tests(CommitId::for_tests(1));
    backend.resolve_damage_events();
    assert!(!backend.damage_has_staged_frame_for_tests(0), "Submitting must stage nothing");
}

#[test]
fn staging_happens_at_accepted_not_at_dispatch() {
    let mut backend = damage_backend_for_tests();
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0)]));
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert!(backend.damage_has_staged_frame_for_tests(0));
}

#[test]
fn an_explicit_rejection_leaves_nothing_to_roll_back() {
    let mut backend = damage_backend_for_tests();
    let before = backend.damage_missing_area_for_tests(0, 0);
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0)]));
    backend.owner_reject_for_tests(CommitId::for_tests(1), libc::EINVAL);
    backend.resolve_damage_events();
    assert!(!backend.damage_has_staged_frame_for_tests(0));
    assert_eq!(
        backend.damage_missing_area_for_tests(0, 0),
        before,
        "a rejected commit recomputes an identical repaint next tick"
    );
}

#[test]
fn applying_happens_at_hardware_complete() {
    let mut backend = damage_backend_for_tests();
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0)]));
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert!(backend.damage_has_staged_frame_for_tests(0));
    backend.owner_deliver_for_tests(DamageEvent::HardwareComplete(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert!(!backend.damage_has_staged_frame_for_tests(0), "the staged frame was applied");
}

#[test]
fn presented_without_hardware_complete_applies_nothing() {
    // Presentation is protocol completion. It is absent for whole commit
    // classes that still change what is displayed, so it must never drive the
    // damage transaction.
    let mut backend = damage_backend_for_tests();
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0)]));
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.owner_mark_presented_for_tests(CommitId::for_tests(1));
    backend.resolve_damage_events();
    assert!(backend.damage_has_staged_frame_for_tests(0), "Presented applies nothing");
}

#[test]
fn a_cursor_only_commit_with_no_page_event_still_applies_its_damage() {
    let mut backend = damage_backend_for_tests();
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0)]));
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.owner_deliver_for_tests(DamageEvent::HardwareComplete(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert!(!backend.damage_has_staged_frame_for_tests(0));
}

#[test]
fn an_incomplete_compose_invalidates_instead_of_staging() {
    // Preserved from the merged base: a truncated submit painted less than it
    // claims, so recording it would bake a hole into that buffer permanently.
    let mut backend = damage_backend_for_tests();
    let mut stage = stage_for_tests(CommitId::for_tests(1), &[(0, 0)]);
    stage.entries[0].complete = false;
    backend.hold_damage_stage(stage);
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert!(!backend.damage_has_staged_frame_for_tests(0));
    assert_eq!(backend.damage_missing_area_for_tests(0, 0), backend.full_output_area_for_tests(0));
}

#[test]
fn damage_arriving_between_accept_and_hardware_complete_survives() {
    let mut backend = damage_backend_for_tests();
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0)]));
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    backend.add_damage_for_tests(0, rect_for_tests(100, 100, 50, 50));
    backend.owner_deliver_for_tests(DamageEvent::HardwareComplete(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert!(
        backend.damage_missing_area_for_tests(0, 0) > 0,
        "damage that arrived in flight is not cleared by the apply"
    );
}

#[test]
fn no_damage_transition_is_driven_by_an_ioctl_return() {
    let scene = include_str!("../../render/scene.rs");
    let stage_site = &scene[scene.find("fn stage_submitted_frame").unwrap()..];
    assert!(
        !stage_site[..2000].contains("atomic_commit"),
        "DMG-1: the transaction is driven by owner milestones, not by the ioctl"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver damage_milestone`
Expected: FAIL — `DamageEvent` and the held-stage plumbing do not exist.

- [ ] **Step 3: Write the implementation**

`KmsDeviceOwner` records milestone transitions as they happen and hands them to the backend in one drain, so the scene never inspects owner internals:

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum DamageEvent {
    /// The ioctl returned success. This is C.0's "the submit actually
    /// succeeded" and the only point at which staging is permitted.
    Accepted(CommitId),
    /// Every expected out-fence reported successful signalled status, so the
    /// buffer's content is on screen.
    HardwareComplete(CommitId),
    /// Neither possible buffer state is proven.
    Unknown(CommitId),
}

impl KmsDeviceOwner {
    pub(crate) fn take_damage_events(&mut self) -> Vec<DamageEvent> {
        std::mem::take(&mut self.damage_events)
    }
}
```

`on_host_call_outcome` pushes `Accepted` in its accept arm and `Unknown` in its unknown arm; `FailedBeforeSubmit` pushes nothing, because nothing was staged. `poll_fences` pushes `HardwareComplete` at the same instant it sets `milestones.hardware_complete`, and `Presented` pushes nothing at all — `DMG-2`.

The backend holds the composed regions from compose until the milestones resolve them:

```rust
pub(crate) struct DamageStageEntry {
    pub(crate) output_idx: usize,
    pub(crate) bo_idx: usize,
    pub(crate) repaint: Region,
    pub(crate) painted: Region,
    /// False when the compose was truncated; invalidates instead of staging.
    pub(crate) complete: bool,
}

pub(crate) struct PendingDamageStage {
    pub(crate) commit: CommitId,
    pub(crate) entries: Vec<DamageStageEntry>,
}

impl KmsBackend {
    pub(crate) fn resolve_damage_events(&mut self) {
        for event in self.owner.take_damage_events() {
            match event {
                DamageEvent::Accepted(commit) => {
                    let Some(stage) = self.held_damage_stage_for(commit) else { continue };
                    for entry in &stage.entries {
                        let damage = self.scene.scanout_damage_mut(entry.output_idx);
                        if entry.complete {
                            damage.commit_submitted(entry.bo_idx, &entry.repaint, &entry.painted);
                        } else {
                            damage.invalidate();
                        }
                    }
                }
                DamageEvent::HardwareComplete(commit) => {
                    let Some(stage) = self.take_held_damage_stage(commit) else { continue };
                    for entry in &stage.entries {
                        self.scene.scanout_damage_mut(entry.output_idx).retire_success();
                    }
                }
                DamageEvent::Unknown(commit) => { /* task 19 */ }
            }
        }
    }
}
```

At `scene.rs:4344` the compose stops calling `stage_submitted_frame` directly; it builds a `DamageStageEntry` and hands it to `KmsBackend::hold_damage_stage` keyed by the `CommitId` the owner allocated for that submission. At `scene.rs:1978` the `retire_success()` call is removed from the page-flip ack path: retirement still acks Present state there, but the damage transaction is now driven by `HardwareComplete`. The defensive `state.damage.invalidate()` on platform/scene divergence a few lines above stays exactly as it is.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/device_owner.rs crates/yserver/src/kms/render/scene.rs \
        crates/yserver/src/kms/render/backend.rs
git commit -m "feat(kms): drive the damage transaction from owner milestones"
```

---

### Task 21: Unknown, poison and bundle damage handling

`DMG-3`, `DMG-4` and `DMG-5`. This is the task that keeps a stale pixel off the screen when C.0 cannot prove which buffer state is current.

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs`
- Modify: `crates/yserver/src/kms/owner/device_owner.rs`

**Interfaces:**
- Consumes: `DamageEvent` (task 18); `ExpectedCompletionCrtcs` from the commit record (task 5); `AdmissionChoice::Bundle` (task 14).
- Produces: `KmsBackend::invalidate_damage_for_outputs(&mut self, outputs: &[usize], cause: DamageInvalidateCause)` and `DamageInvalidateCause::{AcceptanceUnknown, IncarnationPoison, Recovery, TopologyInvalidation, VtRelease, DeviceLoss, DirectEntry, ComposedUnflip}`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn acceptance_unknown_invalidates_rather_than_choosing() {
    let mut backend = damage_backend_for_tests();
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0)]));
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    backend.owner_deliver_for_tests(DamageEvent::Unknown(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert!(!backend.damage_has_staged_frame_for_tests(0), "the staged frame is dropped");
    assert_eq!(
        backend.damage_missing_area_for_tests(0, 0),
        backend.full_output_area_for_tests(0),
        "every buffer owes the whole output"
    );
}

#[test]
fn acceptance_unknown_is_neither_apply_nor_restore() {
    // Applying would clear pixels that may never have reached the screen;
    // restoring would claim the flip did not land when it may have.
    let mut backend = damage_backend_for_tests();
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0)]));
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.owner_deliver_for_tests(DamageEvent::Unknown(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert_eq!(backend.damage_retire_success_calls_for_tests(), 0);
    assert_eq!(backend.damage_retire_failure_calls_for_tests(), 0);
    assert_eq!(backend.damage_invalidate_calls_for_tests(), 1);
}

#[test]
fn every_unproven_lifecycle_cause_invalidates() {
    for cause in [
        DamageInvalidateCause::IncarnationPoison,
        DamageInvalidateCause::Recovery,
        DamageInvalidateCause::TopologyInvalidation,
        DamageInvalidateCause::VtRelease,
        DamageInvalidateCause::DeviceLoss,
    ] {
        let mut backend = damage_backend_for_tests();
        backend.add_damage_for_tests(0, rect_for_tests(0, 0, 10, 10));
        backend.invalidate_damage_for_outputs(&[0], cause);
        assert_eq!(
            backend.damage_missing_area_for_tests(0, 0),
            backend.full_output_area_for_tests(0),
            "{cause:?} must invalidate"
        );
    }
}

#[test]
fn a_bundle_stages_one_buffer_per_included_output_and_applies_to_exactly_that_set() {
    let mut backend = damage_backend_with_outputs_for_tests(3);
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0), (1, 0)]));
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert!(backend.damage_has_staged_frame_for_tests(0));
    assert!(backend.damage_has_staged_frame_for_tests(1));
    assert!(!backend.damage_has_staged_frame_for_tests(2), "output 2 was not in the bundle");

    backend.owner_deliver_for_tests(DamageEvent::HardwareComplete(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert!(!backend.damage_has_staged_frame_for_tests(0));
    assert!(!backend.damage_has_staged_frame_for_tests(1));
    assert_eq!(backend.damage_retire_success_calls_for_output_for_tests(2), 0);
}

#[test]
fn a_bundle_that_becomes_unknown_invalidates_every_included_output() {
    let mut backend = damage_backend_with_outputs_for_tests(3);
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0), (1, 0)]));
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.owner_deliver_for_tests(DamageEvent::Unknown(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    for idx in [0, 1] {
        assert_eq!(
            backend.damage_missing_area_for_tests(idx, 0),
            backend.full_output_area_for_tests(idx)
        );
    }
    assert_eq!(backend.damage_invalidate_calls_for_output_for_tests(2), 0);
}

#[test]
fn staging_an_output_twice_without_an_intervening_apply_is_refused() {
    let mut backend = damage_backend_for_tests();
    backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(1), &[(0, 0)]));
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert_eq!(
        backend.hold_damage_stage(stage_for_tests(CommitId::for_tests(2), &[(0, 0)])),
        Err(DamageStageError::OutputAlreadyStaged(0)),
        "the single device slot makes this unreachable; refuse rather than corrupt"
    );
}

#[test]
fn a_direct_transaction_applies_to_no_composed_buffer() {
    let mut backend = damage_backend_for_tests();
    backend.add_damage_for_tests(0, rect_for_tests(0, 0, 10, 10));
    let before = backend.damage_missing_area_for_tests(0, 0);
    backend.submit_direct_for_tests(CommitId::for_tests(1), 0);
    backend.owner_deliver_for_tests(DamageEvent::Accepted(CommitId::for_tests(1)));
    backend.owner_deliver_for_tests(DamageEvent::HardwareComplete(CommitId::for_tests(1)));
    backend.resolve_damage_events();
    assert_eq!(
        backend.damage_missing_area_for_tests(0, 0),
        before,
        "DMG-5: a direct commit clears nothing from a composed buffer"
    );
}

#[test]
fn direct_entry_and_composed_unflip_both_invalidate_the_affected_outputs() {
    let mut backend = damage_backend_for_tests();
    backend.enter_direct_for_tests(0);
    assert_eq!(
        backend.damage_missing_area_for_tests(0, 0),
        backend.full_output_area_for_tests(0)
    );
    backend.paint_and_apply_for_tests(0);
    backend.composed_unflip_retire_for_tests(0);
    assert_eq!(
        backend.damage_missing_area_for_tests(0, 0),
        backend.full_output_area_for_tests(0)
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver damage_unknown damage_bundle`
Expected: FAIL — the `Unknown` arm is a stub and the bundle set is not tracked.

- [ ] **Step 3: Write the implementation**

The `DamageEvent::Unknown` arm takes the held stage, drops it, and invalidates every output it named:

```rust
DamageEvent::Unknown(commit) => {
    // DMG-3: neither possible buffer state is proven. Applying would clear
    // pixels that may never have reached the screen; restoring would claim
    // the flip did not land when it may have. One full repaint is the only
    // truthful answer.
    let outputs = match self.take_held_damage_stage(commit) {
        Some(stage) => stage.entries.iter().map(|e| e.output_idx).collect(),
        // No held stage: the transaction painted nothing, but its CRTCs may
        // still have changed. Fall back to the record's completion set.
        None => self.owner.expected_completion_outputs(commit),
    };
    self.invalidate_damage_for_outputs(&outputs, DamageInvalidateCause::AcceptanceUnknown);
}
```

`invalidate_damage_for_outputs` calls `ScanoutDamage::invalidate` for each named output and logs the cause once per invalidation, so a poison storm is visible in telemetry without becoming a log storm.

`hold_damage_stage` returns `Result<(), DamageStageError>` and refuses an output that already has a staged frame. `DMG-4` makes that unreachable in production — the single device slot means only one transaction can be in flight — so the refusal is a guard against a future change rather than a path production takes.

`PendingDamageStage` carries the whole bundle's entries under one `CommitId`, so both the apply and the invalidate naturally scope to exactly the outputs the transaction included; nothing needs to consult `ExpectedCompletionCrtcs` except the `None` fallback above.

Wire the existing invalidation sites to the typed causes: `backend.rs:2273` becomes `DamageInvalidateCause::ComposedUnflip` and the direct-entry path gains `DamageInvalidateCause::DirectEntry`. Task 12's `poison`, task 15's terminalization and stage 3's lifecycle transitions each call `invalidate_damage_for_outputs` with their own cause.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs crates/yserver/src/kms/owner/device_owner.rs
git commit -m "feat(kms): invalidate damage on acceptance-unknown and scope bundle staging"
```

---

### Task 22: Take the `COMMIT-7` device lock at real device open

Stage 1 built `may_install_state` and proved it; its production caller is this stage's.

**Files:**
- Modify: `crates/yserver/src/kms/backend.rs:844`
- Modify: `crates/yserver/src/kms/executor/device_lock.rs` (drop the `#[allow(dead_code)]`)

**Interfaces:**
- Consumes: `may_install_state(&DrmDeviceKey) -> Result<DeviceLock, LockUnavailable>`; `DrmDeviceKey` from `platform/drm.rs:35`.
- Produces: the opened KMS device carries its `DeviceLock` for the life of the incarnation.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn opening_a_kms_device_consults_the_device_lock_before_installing_state() {
    let device = DrmDeviceKey { major: 226, minor: 250 };
    let held = may_install_state(&device).expect("first holder");
    assert!(
        open_kms_device_for_tests(&device).is_err(),
        "a start must wait or refuse while an earlier incarnation's helper holds the lock"
    );
    drop(held);
    assert!(open_kms_device_for_tests(&device).is_ok());
}

#[test]
fn the_lock_is_held_for_the_life_of_the_incarnation() {
    let device = DrmDeviceKey { major: 226, minor: 251 };
    let opened = open_kms_device_for_tests(&device).expect("open");
    assert!(may_install_state(&device).is_err(), "the open device still holds the lock");
    drop(opened);
    assert!(may_install_state(&device).is_ok());
}

#[test]
fn discovery_probing_does_not_take_the_install_lock() {
    // `discover_kms_candidates` opens every card read-only to enumerate
    // connectors; only the device that becomes the KMS owner installs state.
    let device = DrmDeviceKey { major: 226, minor: 252 };
    let held = may_install_state(&device).expect("holder");
    assert!(discover_kms_candidates_for_tests().is_ok());
    drop(held);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p yserver device_lock`
Expected: FAIL — nothing acquires the lock at open.

- [ ] **Step 3: Write the implementation**

In `kms/backend.rs`, after `drm::Device::open` succeeds and before any state is installed, resolve the `DrmDeviceKey` from the opened fd's `fstat` rdev and call `may_install_state`. `LockUnavailable` is a refusal with a message naming the device and the fact that an earlier incarnation's helper may still be able to mutate it; it is not an error to be retried in a loop. The returned `DeviceLock` is stored alongside the `drm::Device` in the per-device record so it lives exactly as long as the incarnation and is released by that record's drop.

Discovery in `platform/drm.rs::discover_kms_candidates` is unchanged: it opens each card only to enumerate connectors and installs nothing, so it takes no lock.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p yserver device_lock`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/backend.rs crates/yserver/src/kms/executor/device_lock.rs
git commit -m "feat(kms): take the device install lock when opening a real KMS device"
```

---

### Task 23: Portable gates and the stage reviewability check

Same gate stage 1 established: the three builds plus a green suite are what make this stage reviewable.

**Files:**
- Modify: `.github/workflows/portable-build.yml` (extend the stage gate list only if the workflow enumerates modules)
- Modify: `docs/status.md`

- [ ] **Step 1: Run the full local gate**

```bash
cargo +nightly fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p yserver
```
Expected: all three clean. `clippy` must run with `--all-targets` exactly as CI does — a crate-scoped run misses lints in the new test modules.

- [ ] **Step 2: Run the three portable builds**

```bash
cargo build -p yserver --target x86_64-unknown-linux-gnu
cargo build -p yserver --target x86_64-unknown-linux-musl
cargo build -p yserver --target x86_64-unknown-freebsd
```
Expected: all three compile. The new `SYNC_IOC_FILE_INFO` request code is the stage's only new ioctl and is the likeliest portability break — it must go through `platform/ioctl.rs`'s `iowr`, never through a `libc::Ioctl` alias.

- [ ] **Step 3: Verify the removals**

```bash
rg -n 'atomic_commit' crates/yserver/src/drm/page_flip.rs
rg -n 'IN_FENCE_FD' crates/yserver/src/kms/render/platform.rs
rg -n 'submit_direct_scanout|submit_composed_scanout|submit_flip_with_fences' crates/yserver/src
```
Expected: `page_flip.rs` no longer names `atomic_commit`; no live `IN_FENCE_FD` value crosses a C.0 submission; the three old submission helpers exist only as request builders under their new names.

- [ ] **Step 4: Update the status document**

Add a Phase C.0 stage 2 line to `docs/status.md` recording that the device owner exists end to end, that the three primary submission families are converted, and that modeset/DPMS/VT/topology and cursor/gamma remain on the merged Phase A+B path until stages 3 and 4.

- [ ] **Step 5: Commit**

```bash
git add docs/status.md .github/workflows/portable-build.yml
git commit -m "docs(kms): record the stage 2 device owner and primary conversion"
```

---

## Stage exit criteria

Stage 2 is reviewable when all of the following hold.

- The three portable builds pass, `cargo clippy --all-targets -- -D warnings` is clean and the full suite is green.
- A real property list crosses the executor wire with helper-owned `OUT_FENCE_PTR` holder storage, and every returned fd is adopted, status-queried and closed exactly once.
- `page_flip.rs` and the three converted `modeset.rs` sites contain no `Device::atomic_commit` call, and no live C.0 request carries an `IN_FENCE_FD` value other than `-1`.
- One device never has two dispatched-or-submitted atomic transactions, including for disjoint CRTCs.
- No path converts an `Unknown` outcome into a `Rejected` one, and no `EBUSY` is retried.
- `HardwareComplete` comes only from successful canonical sync-file status for the complete `ExpectedCompletionCrtcs`; `Presented` comes only from a correlated tagged page event; neither is inferred from the other.
- Readiness is closed until the first commit with a non-empty `ExpectedCompletionCrtcs` completes with full fence evidence, and no synthetic transition is inserted to reach it.
- Every ready maintenance identity gets a ticket immediately, keeps it across latest-wins replacement, and is admitted within the `§9.2.1` bound.
- The device install lock is held for the life of every real KMS incarnation.
- The damage transaction is driven only by owner milestones: nothing stages at
  `Submitting` or at dispatch, applying happens at `HardwareComplete` and never
  at `Presented`, and `CompletionUnknown` invalidates rather than choosing
  between the two possible buffer states. A bundle stages one buffer per
  included output and applies to exactly that set. `scanout_damage.rs` is
  unmodified.

## What stage 3 consumes

- `modeset.rs:1144` (`disable_output`) and `modeset.rs:1305` (`modeset_with_flags`) are the two remaining direct `atomic_commit` sites; stage 3 converts them into owner-held lifecycle intents together with DPMS, VT, hotplug and topology.
- `DeviceLifecycleState` gains `Recovering(RecoveryId)` and `RecoveryFailed`, and `LifecycleTransitionId` gains its producer — stage 2 always dispatches with `transition: None` because it owns no transition.
- The qualification gate's single transition site moves from "the first converted primary commit" to the real install/restore modeset commit.
- `AdmissionContext::homogeneous_group` is supplied as an empty set in stage 2, so tier 5 is built and unit-tested but never selected in production until stage 3 discovers the group's exact mode-derived refresh rationals.
- `AdmissionState`'s maintenance identities carry an opaque generation; stage 4 attaches the cursor and gamma payloads and the absorption compatibility predicate that currently comes from `AdmissionContext`.

## Self-review notes

Checked against the spec after writing.

- **Coverage.** Every item named in section 18 stage 2 maps to a task: the device-local atomic slot (6), canonical completion evidence (7, 8, 10, 11), bounded admission and fairness (13, 14), direct-primary submission (17), `ScanoutM2State::queued_successor` promotion (17), composed primary replacement (16), and the exact buffer/Present retirement rules (15). The three prerequisites the stage cannot skip are tasks 2–4 (a real payload and a real closure) and tasks 9–10 (no event-bearing commit may be admitted without a resolved clock), plus tasks 12 and 18 for the two products stage 1 handed over.
- **Two stage-1 defects fixed early.** `AtomicRequest` used `ClockEpochId` for the lifecycle epoch, and `HostCallClass` was derived from the `NONBLOCK` bit — which gives seat-active `TEST_ONLY` the 30-second watchdog. Task 1 fixes both before anything depends on them.
- **One behavioural change called out explicitly.** The copied-scanout path currently hands KMS an unresolved `IN_FENCE_FD`. `COMMIT-4` forbids it, so task 16 converts it to an asynchronous pre-submit producer wait. This is the only place where stage 2 changes what the kernel is asked to do beyond the ownership move, and it is spec-mandated rather than incidental.
- **Type consistency.** `SerializedRequest` is produced by task 4 and consumed unchanged by tasks 6, 16 and 17. `Milestones` field names are identical in tasks 5, 7, 8 and 11. `FenceSlotState` is declared in task 5 and defined in task 7 — the declaration is an opaque enum so task 5's tests compile without the ioctl. `Displaced { idle_now, deferred_skip }` is produced by task 13 and consumed by task 15 with the same field names. `AdmissionChoice` variants named in task 14's tests match the ones task 14 defines.
- **A second behavioural coupling found after the master merge.** The
  damage-clipped repaint work that landed at `02bafec3` keys a two-phase
  transaction to KMS submit/retire. That model has exactly two post-submit
  outcomes; C.0 has three, and splits retirement into `HardwareComplete` and
  `Presented`. Tasks 18 and 19 re-anchor it under the spec's new section 12.1.
  The merged base already provides the correct escape hatch — an `invalidate()`
  whose comment argues precisely the case `CompletionUnknown` needs — so this is
  a rewiring, not a new mechanism.
- **Known gaps closed deliberately, not silently.** Tier 5 has no production selector until stage 3, the maintenance payload is opaque until stage 4, and the qualification commit is the first primary commit until stage 3 — all three are recorded in "Deliberate stage boundaries" and again in "What stage 3 consumes" so neither can be mistaken for an omission.
