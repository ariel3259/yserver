# Phase C.0 Stage 2 — Device commit owner and merged-primary integration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the device-local atomic commit owner — one slot, canonical out-fence completion evidence, tagged page-event correlation, bounded admission with a starvation bound — and route every merged primary submission (composed flip, copied scanout, direct scanout, composed replacement, direct successor promotion) through it.

**Architecture:** Stage 1 delivered an executor that can perform an *empty* atomic ioctl. Stage 2 first teaches the wire protocol to carry a real serialized property list plus helper-owned `OUT_FENCE_PTR` holder storage, then builds `KmsDeviceOwner` on top: it constructs requests through one builder that computes `AtomicCrtcClosure`/`ExpectedCompletionCrtcs` and enforces the off-to-off signaling rule, installs a `Submitting` record before IPC, adopts out-fences and queries canonical sync-file status for `HardwareComplete`, resolves tagged page events to `Presented`, and admits work through the seven-tier fair-admission function. The three merged primary submission helpers stop calling `Device::atomic_commit` and become request builders the owner submits.

**Tech Stack:** Rust (stable toolchain), `drm` 0.15 / `drm-ffi` 0.9, `libc`, `std::os::unix` sockets. No serialization crate: framing stays hand-rolled, extended from stage 1's fixed frames to one fixed head plus a bounded variable payload.

**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md` (Approved 2026-09-02). This plan implements section 18 **stage 2 only**. Stages 3 and 4 (lifecycle/modeset/DPMS/VT/topology; cursor and gamma conversion) are planned separately.

**Predecessor:** `docs/superpowers/plans/2026-09-02-phase-c0-stage-1-executor-substrate.md`, complete at `83b47700`. Its "What stage 2 consumes" section names the three products this stage spends: the six real `atomic_commit` call sites, the `SubmittingProof` producer, and the `may_install_state` production caller.

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

### Task 2: Carry a real atomic property payload over the executor wire

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
  - `HostCallReply::Accepted { seq, helper_duration_ns, out_fence_present: u32 }` — a bitmap over `out_fence_slots`, and `HostCallReply::Rejected { seq, errno, helper_duration_ns, unexpected_fence_output: bool }`.

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
head   (56) : seq u64 | incarnation u64 | lifecycle_epoch u64 | transition_present u8
              | class u8 | pad u16 | transition u64 | commit u64 | event_token u64
              | flags u32 | object_count u32 | prop_count u32 | slot_count u32
body        : objects[object_count] u32
            | count_props[object_count] u32
            | props[prop_count] u32
            | values[prop_count] u64
            | slots[slot_count] { crtc_id u32, value_index u32 }
```

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

### Task 3: Helper-side property materialization and `OUT_FENCE_PTR` holder ownership

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
    assert_ne!(echoed[1], 0, "the out-fence slot must carry a holder address");
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

`HostCallOutcome::Accepted` gains `out_fence_present: u32` alongside its `out_fences: Vec<OwnedFd>` so the owner can map each adopted fd back to its slot, and `HostCallOutcome::Rejected` gains `unexpected_fence_output: bool`. In `KmsIoExecutor`, reject a reply whose adopted fd count differs from `out_fence_present.count_ones()` as `HostCallOutcome::Unknown(UnknownReason::MalformedReply)`.

Add to `kms/executor/test_support.rs`:

```rust
/// Open any DRM primary node the test host has, or skip. The tests here only
/// need a device that answers ioctls with an errno; they never install state.
pub(crate) struct TestDevice { /* OwnedFd */ }

impl TestDevice {
    pub(crate) fn open_any_drm_or_skip() -> Self { /* /dev/dri/card0..card3, else eprintln + skip */ }
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

### Task 4: The owner request builder, atomic CRTC closure and the off-to-off signaling rule

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
    fn the_final_rescan_catches_a_closure_that_no_longer_matches() {
        let mut b = AtomicRequestBuilder::new();
        b.add_crtc_property(CRTC_A, PROP_ACTIVE, 1);
        b.declare_crtc_active(CRTC_A, true, true);
        // Simulate a mutation between closure calculation and serialization.
        b.corrupt_recorded_closure_for_tests(BTreeSet::from([CRTC_B]));
        assert_eq!(
            b.finish(Signaling { page_flip_event: false }, &out_fence_props()),
            Err(RequestError::ClosureMutated)
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
        let rescanned = rescan_closure(&properties, &self.crtc_objects, &self.bindings);
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

`serialize` walks the `BTreeMap` in key order producing `objects`, `count_props`, `props`, `values` plus a `HashMap<(u32, u32), u32>` from `(object, prop)` to its `values` index. `rescan_closure` recomputes the CRTC-object union and the binding union from the serialized arrays and the recorded object kinds; it exists so a mutation between the closure calculation and serialization is caught, which is exactly test 53's requirement.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner::request`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/request.rs crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): add the owner atomic request builder and CRTC closure rules"
```

---

### Task 5: Commit records, typed milestones, terminal states and the tombstone ring

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
    fn a_tombstone_owns_no_kms_resource() {
        let mut record = record_for_tests();
        record.resources.push_new_framebuffer(7);
        let tombstone = record.into_tombstone();
        assert_eq!(tombstone.kernel_event_crtcs, BTreeSet::from([40]));
        // The type carries no resource ledger at all; this is a compile-time
        // guarantee reasserted here for the reader.
        assert_eq!(std::mem::size_of_val(&tombstone.terminal_state), 1);
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

`CommitRecord` holds the identities, the class, `expected_completion`/`kernel_event_crtcs`/`present_event_crtcs`/`observed_event_crtcs`, `fences: BTreeMap<u32, FenceSlotState>` (task 7 fills the type — declare it now as an opaque enum with a `Missing` variant), `milestones`, `state`, `staged_events: Vec<StagedPageEvent>` and `resources: ResourceLedger`. `terminalize` is a no-op once `state.is_terminal()`. `into_tombstone` drops the ledger and keeps only `token`, `kernel_event_crtcs`, `present_event_crtcs`, `observed_event_crtcs` and `terminal_state`.

`ResourceLedger` records both possible old/new sets: `old_framebuffers`, `new_framebuffers`, `old_pins`, `new_pins`, and a `quarantined: bool`. Its job in stage 2 is to keep the uncertainty ledger truthful; the Vulkan/GBM owners it points at are already tracked by the backend and are only referenced by handle here.

`TombstoneRing` is a `VecDeque<Tombstone>` with `const CAPACITY: usize = 64`, plus `clear_after_proven_drain()`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner::commit`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/commit.rs crates/yserver/src/kms/owner/mod.rs
git commit -m "feat(kms): add commit records, typed milestones and the tombstone ring"
```

---

### Task 6: The single device slot, dispatch, and the three-outcome boundary

`COMMIT-6`: the `Submitting` record and its fd lease exist *before* IPC send; after send only an explicit rejection proves `FailedBeforeSubmit`. This task also implements `§9.4`'s `EBUSY` rule, and it is where `SubmittingProof` finally gets its production producer.

**Files:**
- Create: `crates/yserver/src/kms/owner/device_owner.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs` (`SubmittingProof::new` becomes `pub(crate)` and constructible only from the owner module)

**Interfaces:**
- Consumes: `KmsIoExecutor::dispatch`, `HostCallOutcome`, `SubmittingProof`, `IncarnationFdSet`, `LatencyRecorder`, `HostCallSample`; `SerializedRequest` (task 4); `CommitRecord` (task 5).
- Produces:
  - `KmsDeviceOwner::{new, slot_is_free, submit, on_host_call_outcome, lifecycle_state, poison}`
  - `SubmitError::{SlotBusy, AdmissionClosed, Construction(RequestError)}`
  - `DispatchOutcome::{Accepted, Rejected { errno }, Unknown(UnknownReason)}`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submitting_occupies_the_slot_before_ipc_is_sent() {
        let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
            ScriptedOutcome::AcceptedAfterObserving(|owner_view| {
                // The scripted executor runs this while the IPC is notionally
                // in flight: the record must already be installed.
                assert!(!owner_view.slot_is_free());
                assert_eq!(owner_view.pending_state(), Some(CommitState::Submitting));
            }),
        ]);
        owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent).expect("submit");
    }

    #[test]
    fn one_device_never_has_two_submitted_commits_even_for_disjoint_crtcs() {
        let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[ScriptedOutcome::Accepted]);
        owner.submit(request_for_crtc_for_tests(40), CommitClass::NonblockingNonPresent).expect("first");
        assert_eq!(
            owner.submit(request_for_crtc_for_tests(41), CommitClass::NonblockingNonPresent),
            Err(SubmitError::SlotBusy),
            "the C.0 slot is per device, not per CRTC"
        );
    }

    #[test]
    fn the_slot_is_not_released_because_a_result_is_late() {
        let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
            ScriptedOutcome::Unknown(UnknownReason::WatchdogExpired),
        ]);
        owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent).expect("submit");
        assert_eq!(owner.pending_state(), Some(CommitState::CompletionUnknown));
        assert!(!owner.slot_is_free(), "an unknown record still owns the slot");
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
            let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[outcome]);
            owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent).expect("submit");
            assert_eq!(owner.pending_state(), Some(expected));
        }
    }

    #[test]
    fn atomic_ebusy_without_a_live_record_closes_readiness_and_never_retries() {
        let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
            ScriptedOutcome::Rejected(libc::EBUSY),
        ]);
        owner.force_ready_for_tests();
        owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent).expect("submit");
        assert_eq!(owner.lifecycle_state(), DeviceLifecycleState::Poisoned);
        assert_eq!(owner.dispatch_count_for_tests(), 1, "EBUSY must not be retried");
    }

    #[test]
    fn a_rejected_commit_releases_only_the_never_submitted_ledger() {
        let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
            ScriptedOutcome::Rejected(libc::EINVAL),
        ]);
        owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent).expect("submit");
        let record = owner.take_terminal_record_for_tests().expect("terminal record");
        assert!(record.resources.new_resources_released);
        assert!(!record.resources.quarantined, "a proven rejection is not a quarantine");
    }

    #[test]
    fn an_unknown_outcome_quarantines_both_possible_resource_sets() {
        let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
            ScriptedOutcome::Unknown(UnknownReason::IpcFailure),
        ]);
        owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent).expect("submit");
        let record = owner.pending_record_for_tests().expect("record");
        assert!(record.resources.quarantined);
        assert!(!record.resources.new_resources_released);
    }

    #[test]
    fn every_dispatch_records_one_latency_sample() {
        let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[ScriptedOutcome::Accepted]);
        owner.submit(primary_request_for_tests(), CommitClass::NonblockingNonPresent).expect("submit");
        let samples = owner.export_evidence_for_tests().expect("evidence");
        assert_eq!(samples.len(), 1);
        assert!(samples[0].round_trip_ns >= samples[0].helper_duration_ns);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::device_owner`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Write the implementation**

```rust
pub(crate) struct KmsDeviceOwner {
    incarnation: IncarnationId,
    lifecycle_epoch: LifecycleEpochId,
    transition: Option<LifecycleTransitionId>,
    state: DeviceLifecycleState,
    identities: IdentityAllocator,
    executor: ExecutorHandle,
    fd_set: IncarnationFdSet,
    slot: Option<CommitRecord>,
    tombstones: TombstoneRing,
    recorder: LatencyRecorder,
    next_request_seq: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub(crate) enum SubmitError {
    #[error("the device atomic slot is occupied")]
    SlotBusy,
    #[error("live KMS admission is closed in {0:?}")]
    AdmissionClosed(DeviceLifecycleState),
    #[error("request construction failed: {0}")]
    Construction(RequestError),
}

impl KmsDeviceOwner {
    pub(crate) fn submit(
        &mut self,
        request: SerializedRequest,
        class: CommitClass,
    ) -> Result<(), SubmitError> {
        if self.slot.is_some() {
            return Err(SubmitError::SlotBusy);
        }
        let admits = match class {
            CommitClass::BlockingQualification => self.state.admits_qualification_commit(),
            _ => self.state.admits_ordinary_primary() || self.state.admits_qualification_commit(),
        };
        if !admits {
            return Err(SubmitError::AdmissionClosed(self.state));
        }

        let commit = self.identities.next_commit();
        let token = self.identities.next_event_token();

        // COMMIT-6: install the record, reserve the slot, register the event
        // identity and transfer every possible old/new resource BEFORE IPC.
        let record = CommitRecord::new(
            commit,
            token,
            self.incarnation,
            self.lifecycle_epoch,
            self.transition,
            class,
            &request,
        );
        self.slot = Some(record);
        let proof = SubmittingProof::new();

        let host_class = match class {
            CommitClass::NonblockingPrimaryPresent | CommitClass::NonblockingNonPresent => {
                HostCallClass::SeatActiveNonblock
            }
            CommitClass::BlockingOrdinary | CommitClass::BlockingQualification => {
                HostCallClass::ColdStartOrOfflineBlocking
            }
        };
        let wire = HostCallRequest::Atomic(AtomicRequest {
            seq: self.next_seq(),
            incarnation: self.incarnation,
            lifecycle_epoch: self.lifecycle_epoch,
            transition: self.transition,
            commit,
            event_token: token,
            class: host_class,
            flags: atomic_flags(&request, host_class),
            properties: request.properties.clone(),
            out_fence_slots: request.out_fence_slots.clone(),
        });

        let started = Instant::now();
        let outcome = self.executor.dispatch(&wire, proof);
        self.on_host_call_outcome(outcome, started);
        Ok(())
    }

    fn on_host_call_outcome(&mut self, outcome: HostCallOutcome, started: Instant) {
        let Some(record) = self.slot.as_mut() else {
            debug_assert!(false, "a host-call outcome arrived with no live record");
            return;
        };
        record.milestones.dispatched = true;
        match outcome {
            HostCallOutcome::Accepted { helper_duration_ns, round_trip_ns, out_fences, out_fence_present } => {
                record.milestones.accepted = true;
                self.record_sample(record.commit, round_trip_ns, helper_duration_ns);
                self.adopt_out_fences(out_fences, out_fence_present);
            }
            HostCallOutcome::Rejected { errno, helper_duration_ns, round_trip_ns, .. } => {
                self.record_sample(record.commit, round_trip_ns, helper_duration_ns);
                if errno == libc::EBUSY {
                    // Section 9.4: the owner never dispatches while its own
                    // record occupies the slot, so EBUSY cannot mean "wait for
                    // our commit". It is a driver/ownership invariant failure.
                    log::error!(
                        "kms owner: atomic EBUSY with no foreign live record on commit {}",
                        record.commit.get()
                    );
                    record.terminalize(CommitState::FailedBeforeSubmit);
                    record.resources.release_new_resources();
                    self.poison(PoisonCause::ForeignBusy);
                    return;
                }
                record.terminalize(CommitState::FailedBeforeSubmit);
                record.resources.release_new_resources();
            }
            HostCallOutcome::Unknown(reason) => {
                // Never rewritten as rejection. Both possible states are
                // quarantined and the slot stays occupied.
                record.terminalize(CommitState::CompletionUnknown);
                record.resources.quarantine();
                log::warn!(
                    "kms owner: commit {} acceptance-unknown ({reason:?})",
                    record.commit.get()
                );
                self.poison(PoisonCause::AcceptanceUnknown);
            }
        }
        let _ = started;
    }
}
```

`SubmittingProof::new()` becomes `pub(crate)` inside `kms/executor/mod.rs` with a doc comment stating that only `KmsDeviceOwner::submit` may call it, immediately after installing the record. `ExecutorHandle` is an enum over the real `KmsIoExecutor` and a `#[cfg(test)] Scripted(VecDeque<ScriptedOutcome>)` so the state machine is testable without a helper process; the scripted arm exists only under `cfg(test)`, satisfying spec test 78.

`atomic_flags` sets `DRM_MODE_ATOMIC_NONBLOCK` for the two nonblocking classes, `DRM_MODE_PAGE_FLIP_EVENT` when `request.page_flip_event`, and `DRM_MODE_ATOMIC_ALLOW_MODESET` when the caller declared it. It never sets `DRM_MODE_PAGE_FLIP_ASYNC`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/device_owner.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/src/kms/executor/mod.rs
git commit -m "feat(kms): add the device commit owner slot and three-outcome dispatch"
```

---

### Task 7: Out-fence adoption and canonical sync-file status

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

#[test]
fn a_test_only_request_legitimately_returns_no_fence() {
    let mut owner = KmsDeviceOwner::for_tests_with_scripted_executor(&[
        ScriptedOutcome::AcceptedWithFences { present: 0, fences: 0 },
    ]);
    owner.validate_for_tests(validation_request_for_tests()).expect("validate");
    assert!(owner.slot_is_free(), "ValidationOnly occupies no submitted slot");
}

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
    let owner = owner_with_two_expected_crtcs_and_adopted_fences();
    let raw = owner.raw_fence_fds_for_tests();
    drop(owner);
    for fd in raw {
        // A second close must fail with EBADF: the owner already closed it.
        assert_eq!(unsafe { libc::close(fd) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
    }
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

### Task 8: Tagged page-event correlation and its poison rules

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

### Task 9: The owner-serialized clock probe and the epoch-local clock record

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

`probe_crtc_clock` builds `HostCallRequest::ClockProbe` with the current incarnation, lifecycle epoch, topology generation, hardware CRTC, clock epoch and a monotonic `ClockProbeId`, and dispatches it through the same executor path. It owns no commit resources and never touches the atomic slot: `submit` remains callable afterwards. A reply whose incarnation/lifecycle epoch/clock epoch/probe id are not all current is discarded as `QualificationFailed(0)`.

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

### Task 10: `KernelSequence` page-event normalization

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

### Task 11: The three post-dispatch monotonic deadlines

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
    assert_eq!(lifecycle_hardware_deadline(Duration::from_secs(1)), Duration::from_secs(10));
    assert_eq!(lifecycle_hardware_deadline(Duration::from_secs(12)), Duration::from_secs(14));
    assert_eq!(lifecycle_hardware_deadline(Duration::from_secs(40)), Duration::from_secs(30));
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

pub(crate) fn fast_hardware_deadline(slowest_mode_period: Option<Duration>) -> Duration {
    let period = slowest_mode_period.unwrap_or(UNKNOWN_MODE_PERIOD);
    (period * 3).clamp(Duration::from_millis(100), Duration::from_secs(2))
}

pub(crate) fn lifecycle_hardware_deadline(observed_max: Duration) -> Duration {
    let candidate = observed_max.saturating_add(Duration::from_secs(2));
    Duration::from_secs(30).min(Duration::from_secs(10).max(candidate))
}

pub(crate) fn present_event_deadline(mode_period: Option<Duration>) -> Duration {
    let period = mode_period.unwrap_or(UNKNOWN_MODE_PERIOD);
    (period * 2).clamp(Duration::from_millis(50), Duration::from_millis(500))
}
```

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

### Task 12: The qualification gate and readiness

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

`on_commit_completed(record)` runs when `record.milestones.completed_for(record.class)` first becomes true. If `self.state == Unqualified` and `!record.expected_completion.is_empty()` and every fence slot in that set is `Signalled`, the state moves to `Ready` and readiness opens. Nothing else opens readiness: there is no bootstrap path, no synthetic flip, no gamma or cursor transition.

`readiness_open()` is `self.state == DeviceLifecycleState::Ready`. `poison` sets `Poisoned` and therefore closes readiness, but never touches the structural-capability or cohort-validation values — those are computed once during protocol-domain construction and stored outside the owner, which is what `advertised_structural_capability_for_tests` reads (`CAP-1`).

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

### Task 13: Bounded intents, admission tickets and aging

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
fn a_direct_successor_slot_is_latest_wins_for_both_present_option_bits() {
    let mut state = AdmissionState::new();
    let displaced = state.offer_direct_successor(direct_intent(PresentSerial(1), /* async */ false));
    assert!(displaced.is_none());
    let displaced = state
        .offer_direct_successor(direct_intent(PresentSerial(2), /* async */ true))
        .expect("the older never-submitted successor is displaced");
    assert_eq!(displaced.idle_now, Some(PresentSerial(1)));
    assert_eq!(displaced.deferred_skip, Some(PresentSerial(1)));
    assert_eq!(state.direct_successor_serial(), Some(PresentSerial(2)));
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

`AdmissionState` holds `maintenance: BTreeMap<MaintenanceIdentity, MaintenanceIntent>`, `primary: BTreeMap<u32, PrimaryIntents>` and `next_ticket: u64`. `offer_maintenance` inserts with a fresh ticket if the identity is absent, otherwise replaces `generation` and leaves `ticket` and `aged` untouched. `age_unselected` sets `aged = true` for every listed identity that is still present. `take` removes the identity and returns its intent, which is what "consumes its ticket exactly once" means.

`offer_direct_successor` replaces the slot and returns `Displaced { idle_now, deferred_skip }` naming the displaced Present serial: `§10.4` requires releasing its buffer/pins and emitting `IdleNotify` immediately while withholding its `Skip` behind the in-flight predecessor. `offer_barrier` clears the direct successor (returning the same `Displaced`) and marks the barrier pending; a later `offer_direct_successor` while a barrier is pending returns `None` and is not stored.

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

### Task 14: The seven admission tiers, absorption and the starvation bound

`§9.2.1`. `DispatchTimingPolicy::ImmediateOnRetirement` is fixed for C.0: when retirement makes work eligible, admission runs in that wake and dispatches without a retention timer.

**Files:**
- Modify: `crates/yserver/src/kms/owner/admission.rs`
- Modify: `crates/yserver/src/kms/owner/device_owner.rs`

**Interfaces:**
- Consumes: everything from task 13.
- Produces:
  - `AdmissionChoice::{Barrier, Recovery, RetirementSuccessor, AgedMaintenance, Bundle, Primary, Maintenance}`, each carrying the identities it consumes
  - `AdmissionState::select(&mut self, ctx: &AdmissionContext) -> Option<AdmissionChoice>`
  - `AdmissionContext { homogeneous_group: BTreeSet<u32>, owed_crtc: Option<u32>, absorbable: fn(MaintenanceIdentity, u32) -> bool }`
  - `DispatchTimingPolicy::ImmediateOnRetirement` as a unit type documenting the fixed policy.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_waiting_topology_barrier_wins_every_other_tier() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1);
    state.offer_composed_ready(40);
    state.offer_barrier(BarrierIntent::Topology);
    assert!(matches!(state.select(&ctx()), Some(AdmissionChoice::Barrier(_))));
}

#[test]
fn maintenance_alone_is_selected_immediately_rather_than_waiting_for_a_primary() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(gamma_on(40), 1);
    assert!(matches!(
        state.select(&ctx()),
        Some(AdmissionChoice::Maintenance(id)) if id == gamma_on(40)
    ));
}

#[test]
fn a_direct_successor_takes_tier_three_only_when_it_absorbs_every_aged_identity() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1);
    state.age_unselected(&[cursor_on(40)]);
    state.offer_direct_successor(direct_intent(PresentSerial(1), false));

    // Incompatible: tier 4 wins and the primary yields.
    let incompatible = ctx_with_absorbable(|_, _| false);
    assert!(matches!(
        state.select(&incompatible),
        Some(AdmissionChoice::AgedMaintenance(id)) if id == cursor_on(40)
    ));

    // Compatible: tier 3 wins and consumes the aged ticket in the same commit.
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1);
    state.age_unselected(&[cursor_on(40)]);
    state.offer_direct_successor(direct_intent(PresentSerial(1), false));
    let compatible = ctx_with_absorbable(|_, _| true);
    match state.select(&compatible) {
        Some(AdmissionChoice::RetirementSuccessor { absorbed, .. }) => {
            assert_eq!(absorbed, vec![cursor_on(40)]);
        }
        other => panic!("expected tier 3, got {other:?}"),
    }
    assert!(state.intent_for(cursor_on(40)).is_none(), "the absorbed ticket is consumed");
}

#[test]
fn n_incompatible_aged_identities_are_each_admitted_within_the_specified_bound() {
    // Each of N identities is admitted after at most the already-submitted
    // commit plus N-1 older-ticket maintenance admissions.
    let mut state = AdmissionState::new();
    let identities = [cursor_on(40), gamma_on(40), cursor_on(41), gamma_on(41)];
    for id in identities {
        state.offer_maintenance(id, 1);
    }
    state.age_unselected(&identities);
    let ctx = ctx_with_absorbable(|_, _| false);
    let mut order = Vec::new();
    while let Some(AdmissionChoice::AgedMaintenance(id)) = state.select(&ctx) {
        order.push(id);
    }
    assert_eq!(order, identities.to_vec(), "strict oldest-ticket order");
}

#[test]
fn maintenance_absorbs_a_compatible_ready_primary_on_the_same_crtc() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1);
    state.offer_composed_ready(40);
    match state.select(&ctx_with_absorbable(|_, _| true)) {
        Some(AdmissionChoice::Maintenance { absorbed_primary: Some(crtc), .. }) => {
            assert_eq!(crtc, 40);
        }
        other => panic!("expected symmetric absorption, got {other:?}"),
    }
}

#[test]
fn maintenance_absorption_never_crosses_an_unflip_barrier() {
    let mut state = AdmissionState::new();
    state.offer_maintenance(cursor_on(40), 1);
    state.offer_composed_ready(40);
    state.offer_barrier(BarrierIntent::Unflip);
    assert!(matches!(state.select(&ctx_with_absorbable(|_, _| true)), Some(AdmissionChoice::Barrier(_))));
}

#[test]
fn two_ready_crtcs_in_a_qualified_group_enter_the_bundle_before_a_singular_round_robin() {
    let mut state = AdmissionState::new();
    state.offer_composed_ready(40);
    state.offer_composed_ready(41);
    let ctx = ctx_with_group(&[40, 41]);
    match state.select(&ctx) {
        Some(AdmissionChoice::Bundle { crtcs }) => assert_eq!(crtcs, vec![40, 41]),
        other => panic!("expected tier 5, got {other:?}"),
    }
}

#[test]
fn one_ready_crtc_never_waits_on_a_timer_for_a_missing_bundle_member() {
    let mut state = AdmissionState::new();
    state.offer_composed_ready(40);
    let ctx = ctx_with_group(&[40, 41]);
    assert!(matches!(state.select(&ctx), Some(AdmissionChoice::Primary { crtc: 40, .. })));
}

#[test]
fn a_continuously_ready_crtc_cannot_take_two_successive_slots_while_another_is_ready() {
    let mut state = AdmissionState::new();
    state.offer_composed_ready(40);
    state.offer_composed_ready(41);
    let mut ctx = ctx();
    let first = state.select(&ctx).expect("first admission");
    let first_crtc = primary_crtc(&first);
    ctx.owed_crtc = Some(if first_crtc == 40 { 41 } else { 40 });
    state.offer_composed_ready(first_crtc);
    let second = state.select(&ctx).expect("second admission");
    assert_ne!(primary_crtc(&second), first_crtc, "round-robin must yield the turn");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::owner::admission`
Expected: FAIL.

- [ ] **Step 3: Write the implementation**

`select` evaluates the tiers strictly in the spec's order and returns on the first match:

1. a pending topology/ownership barrier;
2. a pending unflip or software-cursor recovery barrier;
3. a ready synchronous direct successor whose closure absorbs **every** aged maintenance identity that would otherwise win, and whose CRTC is not the one currently owed a round-robin turn;
4. the aged identity with the oldest ticket, tie-broken by `(CRTC, class)`, absorbing a compatible ready primary on the same CRTC when no unflip/topology barrier intervenes;
5. one bundle when at least two CRTCs of `ctx.homogeneous_group` have ready synchronous generations, no barrier intervenes, and every changed aged maintenance required by tiers 3–4 is absorbed or already serviced;
6. the oldest ready primary replacement, round-robin across CRTCs, preferring the retirement successor when no different CRTC is owed the turn;
7. the ready non-aged identity with the oldest ticket, with the same symmetric absorption.

Every returned choice carries the identities whose tickets it consumes, and `select` removes them from the map before returning so a ticket is spent exactly once. When a higher-priority item is admitted while a ready identity remains unsent, `select` calls `age_unselected` for exactly those identities before returning — this is the only place aging happens, so "aged without changing its ticket" cannot drift.

In `device_owner.rs`, `on_retirement` runs `select` in the same wake and dispatches immediately; the deferred `Skip` events queued by `§10.4` are published by the caller after the retirement handler returns, preserving the client-visible predecessor-before-`Skip` order.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::owner`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/admission.rs crates/yserver/src/kms/owner/device_owner.rs
git commit -m "feat(kms): add the seven-tier fair admission function and its starvation bound"
```

---

### Task 15: Present and release terminalization

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
fn a_skip_never_fabricates_a_new_msc_or_ust() {
    let mut ledger = TerminalizationLedger::new();
    assert_eq!(
        ledger.complete_accepted_without_presented(PresentSerial(7), None),
        Some(ProtocolCompletion::Skip { msc: 0, ust_us: 0 }),
        "with no validated sample the completion reports no clock, never an invention"
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

`TerminalizationLedger` holds three independently keyed maps — `protocol: HashMap<PresentSerial, ProtocolCompletion>`, `deferred_skips: Vec<(PresentSerial, PresentSerial)>` keyed by the predecessor they wait behind, and `release: HashMap<BufferRef, ReleasePoint>` — plus a `generation: u64` that `invalidate_generation` bumps so an old generation's release point can never be signalled after a device rebuild.

`record_displaced_successor(displaced, predecessor)` pushes the idle event immediately (the buffer, pins and wake are released by the caller at the same moment) and records the deferred `Skip` behind `predecessor`. `publish_deferred_skips(predecessor)` drains only the entries waiting on that predecessor.

`complete_accepted_without_presented` inserts into `protocol` only if absent, returning `None` on the second call. Its clock comes from the last validated CRTC sample; with no sample it reports zeroes rather than inventing a value, and the caller logs that case.

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

### Task 16: Convert composed primary submission and remove the live input fence

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

### Task 17: Convert direct scanout, its `TEST_ONLY` validation, and retirement-time successor promotion

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

### Task 18: Take the `COMMIT-7` device lock at real device open

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

### Task 19: Portable gates and the stage reviewability check

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
- **Known gaps closed deliberately, not silently.** Tier 5 has no production selector until stage 3, the maintenance payload is opaque until stage 4, and the qualification commit is the first primary commit until stage 3 — all three are recorded in "Deliberate stage boundaries" and again in "What stage 3 consumes" so neither can be mistaken for an omission.
