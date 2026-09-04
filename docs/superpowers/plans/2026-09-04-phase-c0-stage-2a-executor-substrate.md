# Phase C.0 Stage 2a — Executor substrate completion

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the process-isolated executor able to carry a real atomic request, return its out-fences, and do so without ever making the X11 core wait — and put the `COMMIT-7` device lock where only the executor's death releases it.

**Architecture:** Stage 1 built an executor that performs an *empty* atomic ioctl through a blocking poll loop. This stage completes it in three moves. The wire gains a bounded variable property payload and a correlation tuple echoed in every reply. The helper materializes those arrays, owns the `OUT_FENCE_PTR` holder storage the kernel writes into, and returns the resulting descriptors. And the host call splits into send, poll and watchdog so the core event loop drives it, which is what `COMMIT-5` requires and what stage 1's `dispatch` cannot provide.

**Tech Stack:** Rust (stable toolchain), `libc`, `std::os::unix` sockets. No serialization crate: framing stays hand-rolled, extended from stage 1's fixed frames to one fixed head plus a bounded variable payload.

**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md` (Approved, revision 2). This plan implements section 18 **stage 2a only**.

**Predecessor:** `2026-09-02-phase-c0-stage-1-executor-substrate.md`, complete at `83b47700`.

**Why this plan is small.** A single stage 2 plan was written twice and reviewed twice, returning 24 and then 26 blocking findings, while the 14-task stage 1 plan returned 2. The reviews are preserved at `2026-09-03-phase-c0-stage-2-plan-adversarial-review.md` and `-round2.md`; every finding in their scope is incorporated here. Sub-stages 2b and 2c are planned separately, after this one is reviewed.

---

## Global Constraints

Copied from the spec. Every task's requirements implicitly include this section.

- **`COMMIT-5`** — the X11 core never executes or waits synchronously for a potentially blocking KMS ioctl. During seat-active service every live commit uses `NONBLOCK`. Blocking atomic calls are restricted to cold startup before service, or final offline/shutdown work after prompt lifecycle obligations have ended.
- **`COMMIT-6`** — before sending IPC the owner installs a `Submitting` record and reserves the device slot. After send, only an explicit ioctl rejection proves `FailedBeforeSubmit`; missing or invalid reply, helper exit, IPC failure and watchdog expiry are acceptance-unknown.
- **`ID-3`** — every executor request and reply carries the lifecycle epoch. A reply is current only when incarnation, lifecycle epoch, optional transition id and commit id all match.
- **`COMMIT-7`** — sending a termination signal, closing the IPC channel, `PR_SET_PDEATHSIG` or a watchdog expiry is a request, not reap proof. The guarantee that no later incarnation installs state underneath a still-live helper comes from a device-scoped advisory lock taken by the executor **for as long as it lives, released only by its death**.
- Host-call watchdog: 2 seconds for seat-active `NONBLOCK` work and for seat-active `ValidationOnly`, 30 seconds for a permitted cold-start or final-offline blocking ioctl.
- Message transport is message-boundary-preserving. Atomic success returns every out-fence through fd passing.
- The executor never reads the DRM event fd; drain is owner-exclusive for the incarnation.
- Identity allocation uses checked increment and never wraps or reuses a token within an incarnation.
- Portable builds must compile on glibc, musl and FreeBSD.
- Format is `cargo +nightly fmt --check`. Tests are `cargo test -p yserver`. Lint is `cargo clippy --all-targets -- -D warnings`, exactly as CI runs it.

### What this sub-stage does not do

- **No owner exists yet.** Nothing here installs a commit record, reserves a device slot or classifies a commit. `SubmittingProof` keeps stage 1's test-only constructor; 2b adds its production producer.
- **No production `atomic_commit` call site is converted.** The six live sites are 2c's.
- **No damage, admission, clock or completion logic.** Those are 2b and 2c.

---

## File Structure

**Modified:**
- `crates/yserver/src/kms/owner/lifecycle.rs` — created here: `LifecycleEpochId`, `LifecycleTransitionId`.
- `crates/yserver/src/kms/owner/identity.rs` — checked allocation.
- `crates/yserver/src/kms/executor/protocol.rs` — protocol version 2: the correlation tuple, the explicit host-call class, the bounded variable payload, the out-fence slot table.
- `crates/yserver/src/kms/executor/transport.rs` — variable-length frames, non-blocking receive.
- `crates/yserver/src/kms/executor/helper.rs` — property materialization, holder ownership, the lock-fd adoption.
- `crates/yserver/src/kms/executor/mod.rs` — the split host-call API, the inherited lock slot.
- `crates/yserver/src/kms/executor/device_lock.rs` — the destructor stops unlocking.
- `crates/yserver/src/kms/backend.rs:844` — real device open takes the lock and hands it to the executor.
- `crates/yserver/src/present/event_loop.rs` — **out of scope**, as in stage 1: its `run_loop` has no caller in the workspace.

**New:**
- `crates/yserver/tests/executor_async.rs` — integration tests that spawn real helper processes.
- `crates/yserver/tests/common/fd_ledger.rs` — the descriptor-ownership instrumentation tasks 3 and 4 assert on.

---

### Task 1: Lifecycle identities, the explicit host-call class, and checked allocation

**Files:**
- Create: `crates/yserver/src/kms/owner/lifecycle.rs`
- Modify: `crates/yserver/src/kms/owner/mod.rs`, `crates/yserver/src/kms/owner/identity.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs:153-181` (`HostCallClass`)

**Interfaces:**
- Consumes: `IncarnationId`, `CommitId`, `EventToken`, `IdentityAllocator` from stage 1.
- Produces:
  - `LifecycleEpochId::{first, next, get, from_raw}` and `LifecycleTransitionId::{from_raw, get}`
  - `HostCallClass::{SeatActiveNonblock, SeatActiveValidation, ColdStartOrOfflineBlocking}` with `watchdog()` and `wire_tag()`
  - `IdentityAllocator` allocation that cannot wrap

- [ ] **Step 1: Write the failing tests**

```rust
// crates/yserver/src/kms/owner/lifecycle.rs
#[cfg(test)]
mod tests {
    use super::{LifecycleEpochId, LifecycleTransitionId};

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
        assert_eq!(e.get(), t.get());
        // Compile-fail case in tests/compile_fail: passing one where the other
        // is expected must not compile.
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
// crates/yserver/src/kms/executor/protocol.rs
#[test]
fn validation_only_carries_the_seat_active_watchdog() {
    // TEST_ONLY never sets NONBLOCK, so deriving the class from the flag bit
    // gives a seat-active validation the 30-second cold-start watchdog.
    assert_eq!(HostCallClass::SeatActiveValidation.watchdog(), Duration::from_secs(2));
    assert_eq!(HostCallClass::SeatActiveNonblock.watchdog(), Duration::from_secs(2));
    assert_eq!(HostCallClass::ColdStartOrOfflineBlocking.watchdog(), Duration::from_secs(30));
}

#[test]
fn the_class_round_trips_through_its_wire_tag() {
    for class in [
        HostCallClass::SeatActiveNonblock,
        HostCallClass::SeatActiveValidation,
        HostCallClass::ColdStartOrOfflineBlocking,
    ] {
        assert_eq!(HostCallClass::from_wire_tag(class.wire_tag()), Some(class));
    }
    assert_eq!(HostCallClass::from_wire_tag(0), None);
    assert_eq!(HostCallClass::from_wire_tag(4), None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver lifecycle identity host_call_class`
Expected: FAIL — the module and the checked allocators do not exist.

- [ ] **Step 3: Write the implementation**

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

`LifecycleTransitionId` is the same newtype without `next`. Neither derives
`From<u64>`, so one cannot be passed where the other is expected; add the
compile-fail case beside stage 1's existing ones.

`IdentityAllocator` gains `checked_next_commit`, `checked_next_event_token` and
`checked_next_sequence_arm` returning `Option`, with the existing infallible
wrappers implemented as `.expect(...)` over them. The tagged counter checks
against `COUNTER_MASK`, not `u64::MAX`, because the purpose tag occupies the top
two bits.

`HostCallClass` gains `SeatActiveValidation` and stops deriving itself from the
`NONBLOCK` flag bit: the class is a declared field of the request, added to the
wire in task 2.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver lifecycle identity host_call_class`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/lifecycle.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/src/kms/owner/identity.rs crates/yserver/src/kms/executor/mod.rs
git commit -m "feat(kms): add lifecycle identities and checked identity allocation"
```

---

### Task 2: The atomic property payload and the reply correlation tuple

Stage 1's helper submits `count_objs = 0` with null pointers, and its reply
carries only a per-socket sequence number. `ID-3` requires every reply to carry
the full correlation tuple, and `COMMIT-6` requires a late success whose
lifecycle tag is stale to remain **accepted** rather than be mistaken for a
rejection — which a sequence number cannot express.

**Files:**
- Modify: `crates/yserver/src/kms/executor/protocol.rs`
- Modify: `crates/yserver/src/kms/executor/transport.rs`

**Interfaces:**
- Consumes: `LifecycleEpochId`, `LifecycleTransitionId`, `HostCallClass` (task 1); `IncarnationId`, `CommitId`, `EventToken`, `ClockEpochId` (stage 1).
- Produces:
  - `AtomicPropertyList { objects: Vec<u32>, count_props: Vec<u32>, props: Vec<u32>, values: Vec<u64> }` with `validate()`
  - `OutFenceSlot { crtc_id: u32, value_index: u32 }`
  - `HostCallCorrelation` — **one type, two variants**, echoed verbatim in every reply
  - `MAX_ATOMIC_OBJECTS = 256`, `MAX_ATOMIC_PROPS = 1024`, `REQUEST_HEAD_LEN = 68`, `MAX_REQUEST_FRAME_LEN = 32 * 1024`
  - `encode_request`, `decode_request`, `encode_reply`, `decode_reply`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_declared_head_length_equals_the_sum_of_its_fields() {
    // Six u64 = 48, presence/class/pad = 4, four u32 = 16. Total 68, so the
    // body starts at 80. An encoder and decoder sharing one wrong offset pass
    // a round-trip test, so assert the constant independently.
    const FIELD_SUM: usize = 6 * 8 + 1 + 1 + 2 + 4 * 4;
    assert_eq!(REQUEST_HEAD_LEN, FIELD_SUM);
    assert_eq!(REQUEST_HEAD_LEN, 68);
    assert_eq!(HEADER_LEN + REQUEST_HEAD_LEN, 80);
}

#[test]
fn a_golden_frame_places_every_field_at_its_documented_offset() {
    let request = AtomicRequest {
        correlation: atomic_correlation_for_tests(),
        class: HostCallClass::SeatActiveNonblock,
        flags: 0x0200,
        properties: AtomicPropertyList {
            objects: vec![0x31],
            count_props: vec![1],
            props: vec![0x07],
            values: vec![0x1111_2222_3333_4444],
        },
        out_fence_slots: vec![],
    };
    let f = encode_request(&HostCallRequest::Atomic(request));
    assert_eq!(&f[0..4], b"YSKX");
    assert_eq!(u64::from_le_bytes(f[12..20].try_into().unwrap()), 7);      // seq @0
    assert_eq!(u64::from_le_bytes(f[20..28].try_into().unwrap()), 3);      // incarnation @8
    assert_eq!(u64::from_le_bytes(f[28..36].try_into().unwrap()), 9);      // lifecycle @16
    assert_eq!(f[60], 0);                                                  // transition_present @48
    assert_eq!(f[61], HostCallClass::SeatActiveNonblock.wire_tag());       // class @49
    assert_eq!(u32::from_le_bytes(f[64..68].try_into().unwrap()), 0x0200); // flags @52
    assert_eq!(u32::from_le_bytes(f[68..72].try_into().unwrap()), 1);      // object_count @56
    assert_eq!(f.len(), 80 + 4 + 4 + 4 + 8);                               // body @80
}

#[test]
fn every_reply_echoes_the_request_correlation() {
    // ID-3: a reply is current only when incarnation, lifecycle epoch,
    // optional transition id and commit id all match. A per-socket sequence
    // number cannot classify a late success against a changed lifecycle.
    let correlation = atomic_correlation_for_tests();
    for reply in [
        HostCallReply::Accepted { correlation, helper_duration_ns: 10, out_fence_present: 0b11 },
        HostCallReply::Rejected { correlation, errno: libc::EINVAL, helper_duration_ns: 10,
                                  unexpected_fence_output: false },
    ] {
        assert_eq!(decode_reply(&encode_reply(&reply)).expect("decode"), reply);
        assert_eq!(reply.correlation(), correlation);
    }
}

#[test]
fn a_clock_probe_reply_echoes_the_probe_correlation_not_the_atomic_one() {
    // The two request kinds carry different identities, so one reply tuple
    // cannot serve both.
    let correlation = probe_correlation_for_tests();
    let reply = HostCallReply::ClockProbe { correlation, sequence: 42, helper_duration_ns: 10 };
    assert_eq!(decode_reply(&encode_reply(&reply)).expect("decode"), reply);
    assert!(matches!(reply.correlation(), HostCallCorrelation::ClockProbe { .. }));
}

#[test]
fn a_reply_whose_correlation_differs_is_detectable_by_the_caller() {
    let sent = atomic_correlation_for_tests();
    let stale = HostCallCorrelation::Atomic {
        lifecycle_epoch: LifecycleEpochId::from_raw(9999),
        ..match sent { HostCallCorrelation::Atomic(a) => a, _ => unreachable!() }
    };
    assert_ne!(sent, stale);
}

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
}

#[test]
fn an_out_fence_slot_index_must_be_inside_the_value_array() {
    let request = AtomicRequest {
        properties: AtomicPropertyList { objects: vec![42], count_props: vec![1],
                                         props: vec![9], values: vec![0] },
        out_fence_slots: vec![OutFenceSlot { crtc_id: 42, value_index: 1 }],
        ..atomic_request_shell_for_tests()
    };
    let frame = encode_request(&HostCallRequest::Atomic(request));
    assert_eq!(decode_request(&frame), Err(ProtocolError::Field("out fence slot index")));
}

#[test]
fn truncation_at_every_length_is_a_length_error_not_a_short_read() {
    let frame = encode_request(&HostCallRequest::Atomic(small_atomic_request_for_tests()));
    for cut in 0..frame.len() {
        assert!(decode_request(&frame[..cut]).is_err(), "truncation at {cut} decoded");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver kms::executor::protocol`
Expected: FAIL — `HostCallCorrelation`, `AtomicPropertyList` and the 68-byte head do not exist.

- [ ] **Step 3: Write the implementation**

```rust
pub(crate) const PROTOCOL_VERSION: u16 = 2;
pub(crate) const REQUEST_HEAD_LEN: usize = 68;
pub(crate) const MAX_ATOMIC_OBJECTS: usize = 256;
pub(crate) const MAX_ATOMIC_PROPS: usize = 1024;
pub(crate) const MAX_REQUEST_FRAME_LEN: usize = 32 * 1024;

const _: () = assert!(REQUEST_HEAD_LEN == 6 * 8 + 1 + 1 + 2 + 4 * 4);

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
        hardware_crtc: u32,
        clock_epoch: ClockEpochId,
        probe: ClockProbeId,
    },
}
```

Atomic request frame layout, little-endian, all offsets relative to the end of
the 12-byte envelope:

```text
head @0..68  seq u64            @0    incarnation u64     @8
             lifecycle_epoch u64 @16  transition u64      @24
             commit u64          @32  event_token u64     @40
             transition_present u8 @48 class u8           @49
             pad u16             @50  flags u32           @52
             object_count u32    @56  prop_count u32      @60
             slot_count u32      @64
body @68     objects[object_count] u32
             count_props[object_count] u32
             props[prop_count] u32
             values[prop_count] u64
             slots[slot_count] { crtc_id u32, value_index u32 }
```

Frame offsets in the golden test are 12 higher because they include the
envelope. `encode_request` calls `properties.validate()` and panics on a
violation: the owner must never construct an invalid list, and a panic in the
parent is preferable to handing a short array to a helper that passes it to the
kernel. `decode_request` re-runs `validate()`, checks `payload_len` equals the
exact computed body length, checks `slot_count <= MAX_OUT_FENCES`, and checks
every `value_index < values.len()`.

The probe request keeps a fixed body and uses the `ClockProbe` correlation.
`transport.rs` receives into a `[u8; MAX_REQUEST_FRAME_LEN]`, and `send_frame`
returns `InvalidInput` above that bound.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver kms::executor`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/protocol.rs crates/yserver/src/kms/executor/transport.rs
git commit -m "feat(kms): carry an atomic property payload and a reply correlation tuple"
```

---

### Task 3: Helper-side materialization and `OUT_FENCE_PTR` holder ownership

`§10.2`: "The executor owns stable `OUT_FENCE_PTR` holder memory until the ioctl has returned and transfers one terminal reply plus every resulting fd in one message-boundary-preserving IPC operation." The holder must live in the helper's address space; an owner-side pointer is meaningless across processes.

**Files:**
- Modify: `crates/yserver/src/kms/executor/helper.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs` (reply validation)
- Modify: `crates/yserver/src/kms/executor/test_support.rs`
- Create: `crates/yserver/tests/common/fd_ledger.rs`

**Interfaces:**
- Consumes: `AtomicRequest`, `AtomicPropertyList`, `OutFenceSlot`, `HostCallCorrelation` (task 2).
- Produces:
  - helper behaviour only, plus `TestDevice::{open_stub, open_real_drm_or_ignore}`
  - `FdLedger` — parent-side descriptor ownership instrumentation
  - `HostCallOutcome::Accepted { correlation, helper_duration_ns, round_trip_ns, out_fences, out_fence_present }` and `Rejected { correlation, errno, helper_duration_ns, round_trip_ns, unexpected_fence_output }`

- [ ] **Step 1: Write the failing tests**

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
    assert_eq!(prepared.values[1], prepared.holder_addresses[0]);
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
    let ledger = FdLedger::install();
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
    assert!(matches!(reply, HostCallReply::Accepted { out_fence_present: 0b01, .. }));
}
```

Parent-side integration tests, in `crates/yserver/tests/executor_async.rs`:

```rust
#[test]
fn the_helper_submits_the_property_list_and_reports_the_kernel_errno() {
    // Object id 0 is never a valid DRM object, so the kernel must reject
    // rather than the helper silently submitting count_objs = 0.
    let device = TestDevice::open_stub();
    let mut executor = spawn_test_executor(&device);
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
    for (slot_count, present, fds) in [(0usize, 0b1000_0000_0000_0000_0000_0000_0000_0000u32, 1usize),
                                       (1, 0b10, 1)] {
        let mut executor = spawn_scripted_helper_for_tests(ScriptedReply::Accepted { present, fds });
        let outcome = dispatch_and_wait_for_tests(&mut executor, request_with_slots_for_tests(slot_count));
        assert!(matches!(outcome, HostCallOutcome::Unknown(UnknownReason::MalformedReply)),
                "slot_count={slot_count} present={present:#b}");
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

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver --test executor_async` and `cargo test -p yserver kms::executor::helper`
Expected: FAIL — the helper still submits an empty request and `FdLedger` does not exist.

- [ ] **Step 3: Write the implementation**

Split preparation from execution so the pointer discipline is unit-testable
without an ioctl:

```rust
struct PreparedAtomic {
    objects: Vec<u32>,
    count_props: Vec<u32>,
    props: Vec<u32>,
    values: Vec<u64>,
    holders: Vec<i32>,
}

fn prepare_atomic(atomic: &AtomicRequest) -> PreparedAtomic {
    let mut prepared = PreparedAtomic {
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
        prepared.values[slot.value_index as usize] = holder as usize as u64;
    }
    prepared
}
```

`execute_atomic` calls `prepare_atomic`, builds `DrmModeAtomic` from the
prepared pointers, runs the ioctl, and then:

- **rc == 0**: for each holder `>= 0`, set its bit in `out_fence_present` and
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
if present & !valid_mask != 0 { return malformed(); }
if fds.len() as u32 != present.count_ones() { return malformed(); }
```

`TestDevice` has two constructors and no skip. `open_stub` opens a helper stub
target that is always available and answers with scripted results; it is the
default for this suite. `open_real_drm_or_ignore` returns `Option` and is used
only by `#[ignore]`d hardware tests, which the suite reports separately.

`FdLedger` (in `tests/common/fd_ledger.rs`, and mirrored as a `#[cfg(test)]`
helper inside `helper.rs`) wraps descriptor creation and close in the process
under test and counts transitions. It is explicitly **not** claimed to observe
another process: helper-side exactness is asserted by the helper's own unit
tests and reported to the parent through `unexpected_fence_output`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/helper.rs crates/yserver/src/kms/executor/mod.rs \
        crates/yserver/src/kms/executor/test_support.rs crates/yserver/tests/
git commit -m "feat(kms): materialize atomic property arrays and own the out-fence holders"
```

---

### Task 4: The asynchronous host-call API and its event-loop integration

The keystone. Stage 1's `dispatch` is a `libc::poll` loop that waits up to the watchdog and can `std::thread::sleep` for 100 ms deciding whether a child died. Calling it from a live path stalls the X11 core for two seconds, which `COMMIT-5` forbids and which is the exact stall section 4.1's process isolation exists to remove. The blocking form is not deleted — it is the correct call at a cold-start or final-offline boundary — but it becomes unreachable by accident.

**Files:**
- Modify: `crates/yserver/src/kms/executor/mod.rs:293-400`
- Modify: `crates/yserver/src/kms/render/backend.rs` — register the control fd and drive the tick
- Test: `crates/yserver/tests/executor_async.rs`

**Interfaces:**
- Consumes: everything from tasks 1–3.
- Produces:
  - `KmsIoExecutor::{send, control_fd, poll_reply, tick, dispatch_blocking_at_boundary}`
  - `HostCallEvent::{Outcome, LateReply}`
  - `SendError::{AlreadyInFlight, Stalled, Ipc}`
  - `BoundaryWitness` — constructible only at a `COMMIT-5` boundary
  - `KmsBackend::{register_executor_source, on_executor_readable, tick_executors}`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn send_returns_before_the_helper_replies() {
    let mut executor = spawn_slow_helper_for_tests(Duration::from_millis(400));
    let started = Instant::now();
    executor.send(&small_atomic_request_for_tests(), SubmittingProof::for_tests()).expect("send");
    assert!(started.elapsed() < Duration::from_millis(50), "send waited {:?}", started.elapsed());
    assert!(executor.poll_reply().is_none());
}

#[test]
fn poll_reply_never_blocks_while_the_helper_is_busy() {
    // Timing-robust: assert that each individual call returns promptly, not
    // that some iteration count is reached, which is scheduler-sensitive.
    let mut executor = spawn_slow_helper_for_tests(Duration::from_millis(300));
    executor.send(&small_atomic_request_for_tests(), SubmittingProof::for_tests()).expect("send");
    for _ in 0..200 {
        let started = Instant::now();
        assert!(executor.poll_reply().is_none());
        assert!(started.elapsed() < Duration::from_millis(10), "poll_reply blocked");
    }
}

#[test]
fn a_readable_control_fd_yields_the_outcome_with_its_correlation() {
    let mut executor = spawn_test_executor(&TestDevice::open_stub());
    let request = rejecting_request_for_tests();
    let sent = request.correlation();
    executor.send(&request, SubmittingProof::for_tests()).expect("send");
    wait_readable_for_tests(executor.control_fd().expect("fd"), Duration::from_secs(2));
    match executor.poll_reply() {
        Some(HostCallEvent::Outcome { correlation, outcome: HostCallOutcome::Rejected { .. } }) => {
            assert_eq!(correlation, sent);
        }
        other => panic!("expected a correlated rejection, got {other:?}"),
    }
}

#[test]
fn only_one_host_call_may_be_in_flight() {
    // This is what serializes host calls now that send returns immediately.
    let mut executor = spawn_slow_helper_for_tests(Duration::from_millis(300));
    executor.send(&small_atomic_request_for_tests(), SubmittingProof::for_tests()).expect("first");
    assert_eq!(
        executor.send(&small_atomic_request_for_tests(), SubmittingProof::for_tests()).unwrap_err(),
        SendError::AlreadyInFlight
    );
}

#[test]
fn a_send_failure_still_produces_exactly_one_terminal_event() {
    // COMMIT-6 installs the record before the send, so a failed send must not
    // leave the caller with a record and no outcome. The parent cannot prove
    // the helper did not act, so the conservative classification is unknown.
    let mut executor = spawn_dead_helper_for_tests();
    let request = small_atomic_request_for_tests();
    let sent = request.correlation();
    let err = executor.send(&request, SubmittingProof::for_tests()).unwrap_err();
    assert_eq!(err, SendError::Ipc);
    match executor.poll_reply() {
        Some(HostCallEvent::Outcome { correlation, outcome: HostCallOutcome::Unknown(_) }) => {
            assert_eq!(correlation, sent);
        }
        other => panic!("a failed send must terminalize its request, got {other:?}"),
    }
}

#[test]
fn the_watchdog_fires_from_tick_without_sleeping_to_reach_it() {
    let mut executor = spawn_slow_helper_for_tests(Duration::from_secs(30));
    executor.send(&small_atomic_request_for_tests(), SubmittingProof::for_tests()).expect("send");
    assert!(executor.tick(Instant::now()).is_none());
    let started = Instant::now();
    let event = executor.tick(Instant::now() + Duration::from_secs(3));
    assert!(started.elapsed() < Duration::from_millis(10), "tick slept to reach the deadline");
    assert!(matches!(
        event,
        Some(HostCallEvent::Outcome { outcome: HostCallOutcome::Unknown(UnknownReason::WatchdogExpired), .. })
    ));
}

#[test]
fn watchdog_expiry_does_not_release_serialization_before_reap() {
    // COMMIT-6: after the watchdog the device enters ExecutorStalled and no
    // retry, reopen or release is permitted until reap proves the alias gone.
    let mut executor = spawn_slow_helper_for_tests(Duration::from_secs(30));
    executor.send(&small_atomic_request_for_tests(), SubmittingProof::for_tests()).expect("send");
    executor.tick(Instant::now() + Duration::from_secs(3));
    assert_eq!(executor.state(), ExecutorState::Stalled);
    assert_eq!(
        executor.send(&small_atomic_request_for_tests(), SubmittingProof::for_tests()).unwrap_err(),
        SendError::Stalled
    );
    executor.force_reap_for_tests();
    assert!(executor.send(&small_atomic_request_for_tests(), SubmittingProof::for_tests()).is_ok());
}

#[test]
fn a_reply_arriving_after_the_watchdog_is_delivered_as_a_late_reply_with_its_fds() {
    // Its descriptors must be adopted and closed exactly once into quarantine
    // rather than dropped on the floor.
    let ledger = FdLedger::install();
    let mut executor = spawn_slow_helper_for_tests(Duration::from_millis(400));
    executor.send(&fence_returning_request_for_tests(), SubmittingProof::for_tests()).expect("send");
    executor.tick(Instant::now() + Duration::from_secs(3));
    wait_readable_for_tests(executor.control_fd().expect("fd"), Duration::from_secs(2));
    match executor.poll_reply() {
        Some(HostCallEvent::LateReply { outcome: HostCallOutcome::Accepted { out_fences, .. }, .. }) => {
            assert_eq!(out_fences.len(), 1, "the late fd is adopted, not leaked");
            drop(out_fences);
            assert_eq!(ledger.closes(), 1);
        }
        other => panic!("expected a late reply, got {other:?}"),
    }
}

#[test]
fn helper_death_while_in_flight_is_unknown_not_rejection() {
    let mut executor = spawn_slow_helper_for_tests(Duration::from_secs(10));
    executor.send(&small_atomic_request_for_tests(), SubmittingProof::for_tests()).expect("send");
    executor.kill_helper_for_tests();
    wait_readable_for_tests(executor.control_fd().expect("fd"), Duration::from_secs(2));
    assert!(matches!(
        executor.poll_reply(),
        Some(HostCallEvent::Outcome { outcome: HostCallOutcome::Unknown(UnknownReason::HelperExited), .. })
    ));
}

#[test]
fn the_backend_registers_the_control_fd_and_drains_it() {
    // The asynchronous design needs a consumer, or it has none.
    let mut backend = backend_with_executor_for_tests();
    assert!(backend.registered_sources_for_tests().contains(&SourceKind::ExecutorControl));
    backend.send_scripted_host_call_for_tests();
    backend.deliver_readability_for_tests(SourceKind::ExecutorControl);
    assert_eq!(backend.drained_host_call_events_for_tests().len(), 1);
}

#[test]
fn the_backend_tick_drives_the_watchdog() {
    let mut backend = backend_with_executor_for_tests();
    backend.send_scripted_host_call_for_tests();
    backend.tick_executors(Instant::now() + Duration::from_secs(3));
    assert_eq!(backend.drained_host_call_events_for_tests().len(), 1);
}

#[test]
fn the_blocking_form_needs_a_boundary_witness_and_is_the_only_polling_site() {
    let src = include_str!("../src/kms/executor/mod.rs");
    assert_eq!(src.matches("libc::poll").count(), 1, "exactly one polling site");
    assert!(!src.contains("std::thread::sleep"), "no sleep on any host-call path");
    assert!(src.contains("fn dispatch_blocking_at_boundary(\n        &mut self,\n        witness: BoundaryWitness,"));
    // BoundaryWitness has no public constructor; the compile-fail case proves
    // a seat-active caller cannot fabricate one.
    assert!(compile_fail_case_exists("fabricate_boundary_witness.rs"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver --test executor_async`
Expected: FAIL — `send`, `poll_reply`, `tick` and the backend integration do not exist.

- [ ] **Step 3: Write the implementation**

The executor owns its in-flight state, so no caller can hold a token that
desynchronizes from it:

```rust
struct InFlight {
    correlation: HostCallCorrelation,
    class: HostCallClass,
    started: Instant,
    deadline: Instant,
    /// Set by `tick` on expiry. The entry is retained so a later reply is
    /// recognised as late rather than unknown.
    watchdog_expired: bool,
}

pub(crate) enum HostCallEvent {
    Outcome { correlation: HostCallCorrelation, outcome: HostCallOutcome },
    /// Arrived after its watchdog expired. Its fds are adopted so the owner
    /// can close them exactly once into quarantine.
    LateReply { correlation: HostCallCorrelation, outcome: HostCallOutcome },
}
```

`send` refuses when `in_flight.is_some()` (`AlreadyInFlight`) or when the state
is `Stalled` or `ShutdownStalled` (`Stalled`). It encodes, sends, and on a
transport error queues `Outcome { Unknown(IpcFailure) }` for that correlation
before returning `Err(SendError::Ipc)` — so a caller that installed a record
under `COMMIT-6` always receives exactly one terminal event, whether or not the
send succeeded. The `Dispatched` milestone belongs at send time, not at reply
time; 2b's owner sets it when `send` returns.

`control_fd` returns `None` once reaped, and `Some(self.control.as_fd())`
otherwise. The socket is `O_NONBLOCK` from construction.

`poll_reply` performs one non-blocking `recv_frame`. `WouldBlock` yields `None`.
EOF yields `HelperExited` if `check_child_exited()`, otherwise `IpcFailure`;
stage 1's 100 ms sleep loop is deleted, because a parent must never sleep to
decide whether a child died. A decoded reply is validated against
`in_flight.correlation`, the slot mask and the descriptor count, then emitted as
`Outcome` or, when `watchdog_expired`, as `LateReply` with its adopted fds. Only
then is `in_flight` cleared.

`tick(now)` returns `None` unless an unexpired in-flight call is past its
deadline. On expiry it sets `watchdog_expired`, moves the executor to
`ExecutorState::Stalled`, calls `request_termination`, and emits
`Unknown(WatchdogExpired)`. It **does not** clear `in_flight` and **does not**
leave `Stalled` — only a successful `try_reap` does, which is what keeps the
executor serialized until reap proves the alias gone.

`dispatch_blocking_at_boundary(&mut self, witness: BoundaryWitness, ...)` is
stage 1's body minus the sleep loop. `BoundaryWitness` is a unit struct whose
only constructors are `BoundaryWitness::cold_start()` and
`BoundaryWitness::final_offline()`, both `pub(crate)` and documented as callable
solely from the two `COMMIT-5` boundaries; the type has no public constructor,
so a seat-active caller cannot reach the blocking path even by name.

In `kms/render/backend.rs`, register `executor.control_fd()` with the existing
event loop alongside the DRM and input sources, route readability to
`on_executor_readable()`, and call `tick_executors(now)` from the loop's
existing timer path. Both return the drained `HostCallEvent`s; in this
sub-stage the backend logs and discards them, and 2b routes them into the owner.
Registering the source here rather than in 2b is deliberate: an asynchronous API
with no consumer is not an implementation.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/mod.rs crates/yserver/src/kms/render/backend.rs \
        crates/yserver/tests/executor_async.rs
git commit -m "feat(kms): split the host call into send, poll and watchdog and drive it from the loop"
```

---

### Task 5: The `COMMIT-7` device lock, held by the executor

`COMMIT-7` says the lock is "taken by the executor for as long as it lives and released only by its death". The case it exists for is the parent dying while a helper is wedged: a parent-held lock is released by the parent's exit, and a new server then installs state underneath the still-live helper.

Two facts make the handoff possible and one makes the obvious implementation wrong. `flock` is associated with the open file description, survives `execve`, and duplicated descriptors share the lock, which is released only "when **all** such file descriptors have been closed" — but also by "an explicit `LOCK_UN` operation on **any** of these duplicate file descriptors". `DeviceLock::drop` currently calls `flock(fd, LOCK_UN)`, so the parent dropping its guard would release the helper's lock too.

**Files:**
- Modify: `crates/yserver/src/kms/executor/device_lock.rs:184-191`
- Modify: `crates/yserver/src/kms/executor/mod.rs:32-33,589-663` — the inherited `LOCK_FD` slot
- Modify: `crates/yserver/src/kms/executor/helper.rs:72-76` — adopt it
- Modify: `crates/yserver/src/kms/backend.rs:844`

**Interfaces:**
- Consumes: `may_install_state`, `DeviceLock`, `DrmDeviceKey`.
- Produces: `LOCK_FD: RawFd = 200`; `DeviceLock::{into_inheritable, release_explicitly}`; the opened device carries no lock guard after handoff.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn dropping_a_device_lock_does_not_unlock_a_shared_description() {
    // The bug that makes the naive handoff wrong: LOCK_UN through any
    // descriptor sharing the open file description releases it globally.
    let key = DrmDeviceKey { major: 226, minor: 250 };
    let lock = may_install_state(&key).expect("first holder");
    let duplicate = lock.duplicate_for_tests();
    drop(lock);
    assert!(
        may_install_state(&key).is_err(),
        "the surviving duplicate must still hold the lock"
    );
    drop(duplicate);
    assert!(may_install_state(&key).is_ok());
}

#[test]
fn an_explicit_release_is_still_available_and_still_global() {
    let key = DrmDeviceKey { major: 226, minor: 251 };
    let lock = may_install_state(&key).expect("holder");
    let duplicate = lock.duplicate_for_tests();
    lock.release_explicitly();
    assert!(may_install_state(&key).is_ok(), "explicit release is global by design");
    drop(duplicate);
}

#[test]
fn the_lock_survives_the_parent_and_is_released_only_by_helper_death() {
    let key = DrmDeviceKey { major: 226, minor: 252 };
    let mut opened = open_kms_device_for_tests(&key).expect("open");
    let helper = opened.take_executor_for_tests();
    drop(opened);   // every parent-side reference gone
    assert!(
        may_install_state(&key).is_err(),
        "the orphaned helper must still hold the device lock"
    );
    helper.kill_and_reap_for_tests();
    assert!(may_install_state(&key).is_ok(), "released only by its death");
}

#[test]
fn the_lock_is_never_free_between_acquisition_and_handoff() {
    // Deterministic rather than racy: the probe attempts an acquisition at
    // each instrumented handoff step, so a window is observed rather than
    // hoped against.
    let key = DrmDeviceKey { major: 226, minor: 253 };
    let probe = LockProbe::attach_for_tests(&key);
    let opened = open_kms_device_for_tests_with_probe(&key, &probe).expect("open");
    assert_eq!(
        probe.successful_acquisitions(),
        0,
        "acquired at steps: {:?}",
        probe.successful_steps()
    );
    drop(opened);
}

#[test]
fn a_start_while_the_lock_is_held_refuses_rather_than_installing() {
    let key = DrmDeviceKey { major: 226, minor: 254 };
    let held = may_install_state(&key).expect("holder");
    let err = open_kms_device_for_tests(&key).unwrap_err();
    assert!(matches!(err, OpenError::LockUnavailable { .. }));
    assert_eq!(err.installs_attempted(), 0, "no state may be installed on refusal");
    drop(held);
}

#[test]
fn discovery_probing_takes_no_install_lock() {
    // discover_kms_candidates opens every card read-only to enumerate
    // connectors and installs nothing.
    let key = DrmDeviceKey { major: 226, minor: 255 };
    let held = may_install_state(&key).expect("holder");
    assert!(discover_kms_candidates_for_tests().is_ok());
    drop(held);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver device_lock`
Expected: FAIL — `Drop` still unlocks, and there is no inherited lock slot.

- [ ] **Step 3: Write the implementation**

Replace the destructor:

```rust
impl Drop for DeviceLock {
    /// Deliberately does NOT `LOCK_UN`. `flock` releases on last close of the
    /// open file description, and an explicit unlock through any duplicate
    /// releases it for all of them — including the executor's inherited copy,
    /// which COMMIT-7 requires to outlive this process. Closing our descriptor
    /// is exactly the semantics we want.
    fn drop(&mut self) {
        // `self.file` closes here; no explicit unlock.
    }
}

impl DeviceLock {
    /// The only remaining explicit release, for the single-process paths that
    /// never handed the lock to a helper.
    pub(crate) fn release_explicitly(self) { /* LOCK_UN then close */ }
}
```

The handoff mirrors `CONTROL_FD` and `KMS_FD` exactly:

1. `kms/backend.rs` calls `may_install_state` before installing anything and
   holds the returned `DeviceLock`. A refusal names the device and says an
   earlier incarnation's helper may still mutate it; it is not retried in a
   loop, and no state is installed on that path.
2. `DeviceLock::into_inheritable` yields the raw descriptor; `spawn_internal`
   duplicates it into `LOCK_FD` with the existing
   `duplicate_to_inherited_slot`, which uses `dup2` and therefore clears
   `FD_CLOEXEC` so it survives the exec.
3. `run_executor_helper` adopts `LOCK_FD` with `take_inherited_fd` and holds it
   for the process lifetime. It re-asserts `LOCK_EX | LOCK_NB` on the inherited
   descriptor as a liveness assertion — the same open file description, so it is
   a no-op conversion that cannot fail — and stores the fd so it is closed only
   at process exit.
4. The parent drops its `DeviceLock` after the helper's first reply proves the
   helper is running. With the destructor above, that closes one descriptor of a
   shared description and releases nothing.

There is no window: from step 1 the parent holds it, from step 2 both hold the
same description, and from step 4 only the helper does.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p yserver` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/device_lock.rs crates/yserver/src/kms/executor/mod.rs \
        crates/yserver/src/kms/executor/helper.rs crates/yserver/src/kms/backend.rs
git commit -m "feat(kms): hand the device lock to the executor and stop unlocking on drop"
```

---

### Task 6: Portable gates and the stage reviewability check

- [ ] **Step 1: Run the full local gate**

```bash
cargo +nightly fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p yserver
```
Expected: all clean. `--all-targets` is required or lints in the new test modules are missed.

- [ ] **Step 2: Run the three portable builds**

```bash
cargo build -p yserver --target x86_64-unknown-linux-gnu
cargo build -p yserver --target x86_64-unknown-linux-musl
cargo build -p yserver --target x86_64-unknown-freebsd
```
Expected: all compile. Every new ioctl goes through `platform/ioctl.rs`'s `iowr`, never a `libc::Ioctl` alias.

- [ ] **Step 3: Verify the removals and the invariants**

```bash
rg -n 'std::thread::sleep' crates/yserver/src/kms/executor/
rg -c 'libc::poll' crates/yserver/src/kms/executor/mod.rs
rg -n 'LOCK_UN' crates/yserver/src/kms/executor/device_lock.rs
rg -n 'open_any_drm_or_skip' crates/yserver/src/
```
Expected: no sleep on any host-call path; exactly one polling site; `LOCK_UN`
only inside `release_explicitly`; no skip-shaped test helper anywhere.

- [ ] **Step 4: Update the status document**

Record that the executor substrate is complete and asynchronous, that the device
lock is executor-held, and that no owner or call-site conversion exists yet.

- [ ] **Step 5: Commit**

```bash
git add docs/status.md
git commit -m "docs(kms): record the stage 2a executor substrate"
```

---

## Stage exit criteria

- The three portable builds pass, `cargo clippy --all-targets -- -D warnings` is clean, and the suite is green.
- A real property list crosses the wire, the helper owns the holder storage, and every returned descriptor is adopted and closed exactly once.
- Every reply echoes its request's correlation tuple, and a mismatch is `MalformedReply` — never a rejection.
- **No seat-active path waits on a host call.** `send` returns after the frame is sent, replies arrive through `poll_reply`, the watchdog fires from `tick`, `libc::poll` appears exactly once, and no `std::thread::sleep` remains on any host-call path.
- The core event loop registers the control fd and drains it: the asynchronous API has a consumer.
- Exactly one host call is in flight at a time; a failed send still produces exactly one terminal event; watchdog expiry keeps the executor `Stalled` until reap; and a reply arriving after expiry is delivered as `LateReply` with its descriptors adopted.
- `DeviceLock::drop` does not unlock, the lock is inherited by the helper across the re-exec, and a start attempted while an orphaned helper holds it refuses without installing state.
- Identity allocation is checked and cannot wrap.

## What stage 2b consumes

- `KmsIoExecutor::{send, control_fd, poll_reply, tick}` and `HostCallEvent`. 2b's owner is the consumer the backend currently logs and discards.
- `SubmittingProof` keeps its test-only constructor here; 2b adds the production producer at record installation, and 2b's clock probe adds the non-atomic host-call reservation that also produces one.
- `HostCallCorrelation` is what 2b's records match a reply against.
- The `Dispatched` milestone is set when `send` returns, not when the reply arrives.

## Self-review notes

- **Scope.** Five implementation tasks plus the gate, against stage 1's fourteen. Every task delivers something testable without an owner: the wire, the helper, the async API with its loop integration, and the lock.
- **Every finding in scope is incorporated.** From round 1: the reply correlation tuple, the 68-byte head, the bitmap slot mask, the deterministic test target, the holder-address assertion, checked identity allocation. From round 2: the event-loop integration that had no consumer, the send-failure terminal path, the late reply and its descriptors, serialization retained until reap, `Dispatched` at send time, the boundary witness, scheduler-insensitive timing tests, and the `LOCK_UN` destructor.
- **One deliberate conservatism.** A failed `send` on a `SOCK_SEQPACKET` did not enqueue the datagram, so `FailedBeforeSubmit` would arguably be provable. The plan classifies it `Unknown` anyway: `COMMIT-6`'s default is unknown, the cost is one quarantine on a path where the helper is usually dead regardless, and a wrong `FailedBeforeSubmit` would release resources the kernel might still own.
- **One thing this stage cannot prove.** `FdLedger` observes only the process it is installed in. Helper-side exactly-once closing is asserted by the helper's own unit tests and reported through `unexpected_fence_output`; the plan says so rather than implying cross-process coverage it does not have.
