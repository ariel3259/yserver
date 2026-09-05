# Phase C.0 Stage 2a — Executor substrate completion

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the process-isolated executor able to carry a real atomic request, return its out-fences, and do so without ever making the X11 core wait — and put the `COMMIT-7` device lock where only the executor's death releases it.

**Architecture:** Stage 1 built an executor that performs an *empty* atomic ioctl through a blocking poll loop. This stage completes it in four moves. The wire gains a bounded variable property payload and a correlation tuple echoed in every reply. The helper materializes those arrays, owns the `OUT_FENCE_PTR` holder storage the kernel writes into, and returns the resulting descriptors. The host call splits into send, poll and watchdog. And the core event loop gains a real executor poll source and a real executor deadline, so that split API has a production consumer rather than a promise of one.

**Tech Stack:** Rust (stable toolchain), `libc`, `std::os::unix` sockets. No serialization crate: framing stays hand-rolled, extended from stage 1's fixed frames to one fixed head plus a bounded variable payload.

**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md` (Approved, revision 2). This plan implements section 18 **stage 2a only**.

**Predecessor:** `2026-09-02-phase-c0-stage-1-executor-substrate.md`, complete at `83b47700`.

**Revision 2, after adversarial review.** The first version of this plan returned 8 blocking and 9 major findings, recorded at `docs/superpowers/findings/2026-09-04-phase-c0-stage-2a-plan-adversarial-review.md`. Tasks 2, 4 and 6 are rewritten whole rather than patched — a lesson from the stage 2 monolith, where paragraph-level corrections left each task's interfaces speaking two languages at once. Task 5 is new: the review established that core event-loop integration crosses `yserver-core`'s `Backend` trait and the core loop's dispatch, which is a separately reviewable deliverable rather than a step inside the async API. Tasks 1, 3 and 7 take targeted corrections, listed in their own headers.

**Do not read the review's task numbers as this plan's.** The review's Task 4 is now Tasks 4 and 5; its Task 5 is now Task 6; its Task 6 is now Task 7.

---

## Global Constraints

Copied from the spec. Every task's requirements implicitly include this section.

- **`COMMIT-5`** — the X11 core never executes or waits synchronously for a potentially blocking KMS ioctl. During seat-active service every live commit uses `NONBLOCK`. Blocking atomic calls are restricted to cold startup before service, or final offline/shutdown work after prompt lifecycle obligations have ended.
- **`COMMIT-6`** — before sending IPC the owner installs a `Submitting` record and reserves the device slot. After send, only an explicit ioctl rejection proves `FailedBeforeSubmit`; missing or invalid reply, helper exit, IPC failure and watchdog expiry are acceptance-unknown. **No second ioctl may be dispatched on the device while this record or its executor lease exists.**
- **`ID-3`** — every executor request and reply carries the lifecycle epoch. A reply is current only when incarnation, lifecycle epoch, optional transition id and commit id all match.
- **`COMMIT-7`** — sending a termination signal, closing the IPC channel, `PR_SET_PDEATHSIG` or a watchdog expiry is a request, not reap proof. The guarantee that no later incarnation installs state underneath a still-live helper comes from a device-scoped advisory lock taken by the executor **for as long as it lives, released only by its death**.
- **`ValidationOnly`** — `TEST_ONLY` executes `drm_atomic_check_only`. It omits `NONBLOCK`, touches no hardware, creates no out-fence, and **does not occupy the submitted-commit slot**. It holds an exclusive owner validation lease, not a `Submitting` record.
- Host-call watchdog: 2 seconds for seat-active `NONBLOCK` work and for seat-active `ValidationOnly`, 30 seconds for a permitted cold-start or final-offline blocking ioctl.
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
- `crates/yserver/src/kms/backend.rs:844` — `platform_init` takes the lock and hands it to the executor.
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

### Task 1: Lifecycle identities, the explicit host-call class, and checked allocation

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
  - `HostCallClass::{SeatActiveNonblock, SeatActiveValidation, ColdStartOrOfflineBlocking}` with `watchdog()`, `wire_tag()` and `from_wire_tag(u8) -> Option<Self>`
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

`cargo test` takes one positional filter, so run three commands rather than
passing three names to one:

```bash
cargo test -p yserver kms::owner::lifecycle
cargo test -p yserver kms::owner::identity
cargo test -p yserver host_call_class
```
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

`LifecycleTransitionId` is the same newtype without `next`. `ClockProbeId` is
the same newtype *with* `next`, allocated per probe within an incarnation. None
of the three derives `From<u64>` or `Into<u64>`, so one cannot be passed where
another is expected.

No compile-fail case is added. The existing runner (`tests/compile_fail.rs:29-43`)
compiles an external file against `libyserver`, and these types are inside
`pub(crate) mod owner` (`kms/mod.rs:18`), so such a file fails on privacy rather
than on the type distinction — a test that passes for the wrong reason. The
newtypes are the enforcement; the tests above pin their construction.

`IdentityAllocator` gains `checked_next_commit`, `checked_next_event_token` and
`checked_next_sequence_arm` returning `Option`, with the existing infallible
wrappers implemented as `.expect(...)` over them. The tagged counter checks
against `COUNTER_MASK`, not `u64::MAX`, because the purpose tag occupies the top
two bits.

`HostCallClass` gains `SeatActiveValidation` and stops deriving itself from the
`NONBLOCK` flag bit: the class is a declared field of the request, added to the
wire in task 2.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p yserver kms::owner::lifecycle
cargo test -p yserver kms::owner::identity
cargo test -p yserver host_call_class
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/lifecycle.rs crates/yserver/src/kms/owner/mod.rs \
        crates/yserver/src/kms/owner/identity.rs crates/yserver/src/kms/executor/mod.rs
git commit -m "feat(kms): add lifecycle identities and checked identity allocation"
```

---

---

### Task 2: The atomic property payload and the reply correlation tuple

Stage 1's helper submits `count_objs = 0` with null pointers, and its reply carries only a per-socket sequence number. `ID-3` requires every reply to carry the full correlation tuple, and `COMMIT-6` requires a late success whose lifecycle tag is stale to remain **accepted** rather than be mistaken for a rejection — which a sequence number cannot express.

The wire is also where the host-call class stops being a guess. Stage 1 derives the class from the `NONBLOCK` bit (`executor/mod.rs:168-180`), so a caller that forgets the bit silently buys the 30-second watchdog on a seat-active path. Here the class is an explicit field and the decoder **refuses** any frame whose flags and payload contradict it.

**Files:**
- Modify: `crates/yserver/src/kms/executor/protocol.rs`
- Modify: `crates/yserver/src/kms/executor/transport.rs`
- Modify: `crates/yserver/src/kms/executor/mod.rs:293-433,483-493` — `dispatch` and `dispatch_for_tests` construct and match on `AtomicRequest`, whose shape changes here. They are updated to the new shape **in this task** so the crate compiles; Task 4 is what replaces `dispatch` itself. Without this the stage would not build between tasks 2 and 4.

**Interfaces:**
- Consumes: `LifecycleEpochId`, `LifecycleTransitionId`, `ClockProbeId`, `HostCallClass` (task 1); `IncarnationId`, `CommitId`, `EventToken`, `ClockEpochId`, `RequestSeq`, `ProtocolError` (stage 1).
- Produces:
  - `AtomicPropertyList { objects: Vec<u32>, count_props: Vec<u32>, props: Vec<u32>, values: Vec<u64> }` with `fn validate(&self) -> Result<(), ProtocolError>`
  - `OutFenceSlot { crtc_id: u32, value_index: u32 }`
  - `HostCallCorrelation` — one type, two variants, echoed verbatim in every reply, with `fn seq(self) -> RequestSeq`
  - `AtomicRequest { correlation, class: HostCallClass, flags: u32, properties: AtomicPropertyList, out_fence_slots: Vec<OutFenceSlot> }`
  - `ClockProbeRequest { correlation }`
  - `HostCallRequest::{Atomic, ClockProbe, Ready}` and `HostCallReply::{Accepted, Rejected, ClockProbe, Ready}`, each reply carrying `fn correlation(&self) -> Option<HostCallCorrelation>`
  - `DRM_MODE_ATOMIC_NONBLOCK = 0x0200`, `DRM_MODE_ATOMIC_TEST_ONLY = 0x0100`
  - `MAX_ATOMIC_OBJECTS = 256`, `MAX_ATOMIC_PROPS = 1024`, `MAX_OUT_FENCES = 16`, `ATOMIC_HEAD_LEN = 68`, `PROBE_HEAD_LEN = 56`, `MAX_REQUEST_FRAME_LEN = 32 * 1024`
  - `HostCallRequest::{correlation, class}` accessors, used by Task 4's `send` to pick the watchdog and check the reservation kind
  - `encode_request`, `decode_request`, `encode_reply`, `decode_reply`, and `#[cfg(test)] encode_request_unchecked_for_tests`
  - `golden_atomic_request_for_tests()` and `golden_atomic_correlation_for_tests()` / `golden_probe_correlation_for_tests()` — the exact request and tuples built inline in the golden tests, factored out so the hostile-frame tests mutate one known-good frame rather than each inventing their own
- Removes: `HostCallClass::from_request` — the class is no longer derivable from flags, it is carried and validated.

- [ ] **Step 1: Write the failing offset and correlation tests**

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
                event_token: EventToken::from_raw(0x66),
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
        assert_eq!(u64_at(52), 0x66, "event_token @40");
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
            assert_eq!(reply.correlation(), Some(correlation));
        }
    }

    #[test]
    fn a_clock_probe_reply_echoes_the_probe_correlation_not_the_atomic_one() {
        let correlation = golden_probe_correlation_for_tests();
        let reply = HostCallReply::ClockProbe { correlation, sequence: 42, helper_duration_ns: 10 };
        assert_eq!(decode_reply(&encode_reply(&reply)).expect("decode"), reply);
        assert!(matches!(
            reply.correlation(),
            Some(HostCallCorrelation::ClockProbe { .. })
        ));
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
    fn the_readiness_handshake_round_trips_and_carries_no_correlation() {
        // Task 6 needs a reply that proves the helper reached its serve loop
        // with every inherited descriptor adopted. It precedes any identity,
        // so it carries none.
        assert_eq!(
            decode_request(&encode_request(&HostCallRequest::Ready)).expect("decode"),
            HostCallRequest::Ready
        );
        let reply = HostCallReply::Ready { helper_pid: 4321 };
        assert_eq!(decode_reply(&encode_reply(&reply)).expect("decode"), reply);
        assert_eq!(reply.correlation(), None);
    }
}
```

- [ ] **Step 2: Write the failing validation and hostile-frame tests**

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

- [ ] **Step 3: Write the failing class-agreement tests**

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
    fn validation_may_not_request_out_fences() {
        // spec:320-329 — TEST_ONLY creates no out-fence.
        let request = AtomicRequest {
            class: HostCallClass::SeatActiveValidation,
            flags: DRM_MODE_ATOMIC_TEST_ONLY,
            properties: AtomicPropertyList { objects: vec![42], count_props: vec![1],
                                             props: vec![9], values: vec![0] },
            out_fence_slots: vec![OutFenceSlot { crtc_id: 42, value_index: 0 }],
            ..golden_atomic_request_for_tests()
        };
        let frame = encode_request_unchecked_for_tests(&HostCallRequest::Atomic(request));
        assert_eq!(
            decode_request(&frame),
            Err(ProtocolError::Field("validation out fence slot"))
        );
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

- [ ] **Step 4: Run the tests to verify they fail**

Run each filter separately — `cargo test` takes one positional filter:

```bash
cargo test -p yserver kms::executor::protocol::wire_tests
```
Expected: FAIL — `HostCallCorrelation`, `AtomicPropertyList`, `ATOMIC_HEAD_LEN` and the class-agreement rules do not exist.

- [ ] **Step 5: Write the implementation**

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

`encode_request` calls `validate()` and then `assert_class_agreement()`, panicking on a violation: the owner must never construct an invalid or mislabelled request, and a panic in the parent is preferable to handing a short array or a mis-classed commit to a helper that passes it to the kernel. `encode_request_unchecked_for_tests` is `#[cfg(test)]` and skips both, so the decoder can be tested against frames a correct encoder never emits.

`decode_request` proceeds strictly in this order, and allocates nothing before step 5:

1. Envelope: magic, `PROTOCOL_VERSION`, known kind, and `payload_len` equal to `frame.len() - HEADER_LEN`.
2. Frame length at most `MAX_REQUEST_FRAME_LEN`, and at least `HEADER_LEN + ATOMIC_HEAD_LEN` for the atomic kind.
3. Read the three counts. `object_count > MAX_ATOMIC_OBJECTS` → `Field("object count limit")`; `prop_count > MAX_ATOMIC_PROPS` → `Field("prop count limit")`; `slot_count > MAX_OUT_FENCES` → `Field("slot count limit")`. These precede every other check because they are the only wire values that size an allocation.
4. Compute the exact body length with `checked_mul`/`checked_add` — `2*4*object_count + 4*prop_count + 8*prop_count + 8*slot_count` — and require it to equal `payload_len - ATOMIC_HEAD_LEN`, else `ProtocolError::Length`. After the caps in step 3 this arithmetic cannot overflow, but it is written checked so a future cap increase cannot silently make it wrap.
5. Allocate and read the five arrays.
6. `class` byte to `HostCallClass` — an unrecognised tag is `Field("class tag")`, never a default.
7. `assert_class_agreement`: `SeatActiveNonblock` requires `NONBLOCK` set and `TEST_ONLY` clear; `SeatActiveValidation` requires `TEST_ONLY` set and `NONBLOCK` clear; `ColdStartOrOfflineBlocking` requires both clear. Any violation is `Field("class flag agreement")`.
8. `SeatActiveValidation` with a non-empty slot table is `Field("validation out fence slot")`.
9. `validate()` on the reconstructed list.
10. Every `value_index < values.len()` → else `Field("out fence slot index")`; no repeated `value_index` → else `Field("duplicate out fence slot index")`; no repeated `crtc_id` → else `Field("duplicate out fence slot crtc")`. Duplicates are detected with a linear scan over at most `MAX_OUT_FENCES` entries; no hashing is needed at this size.

`HostCallReply::Accepted` carries `out_fence_mask: u32` rather than a count, so the parent learns *which* slots produced a descriptor. Its bit `i` corresponds to `out_fence_slots[i]`; the mask is safe in a `u32` because `MAX_OUT_FENCES` is 16, which step 3 has already enforced. `decode_reply` rejects a mask with bits set above the request's slot count.

`HostCallRequest::Ready` and `HostCallReply::Ready { helper_pid: u32 }` are fixed-length frames under new kinds `KIND_READY_REQUEST` and `REPLY_TAG_READY`. They carry no correlation, so `HostCallReply::correlation()` returns `Option<HostCallCorrelation>` and yields `None` for `Ready`.

`transport.rs` receives into a heap `Box<[u8; MAX_REQUEST_FRAME_LEN]>` rather than a stack array — 32 KiB on the stack of every receive is avoidable — and `send_frame` returns `InvalidInput` above that bound. The transport's blocking behaviour is unchanged in this task; Task 4 makes only the parent endpoint non-blocking.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test -p yserver kms::executor
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/yserver/src/kms/executor/protocol.rs crates/yserver/src/kms/executor/transport.rs
git commit -m "feat(kms): carry an atomic property payload and a reply correlation tuple"
```

---

### Task 3: Helper-side materialization and `OUT_FENCE_PTR` holder ownership

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
  - `HostCallOutcome::Accepted { helper_duration_ns, round_trip_ns, out_fences, out_fence_mask }` and `Rejected { errno, helper_duration_ns, round_trip_ns, unexpected_fence_output }` — the correlation lives on Task 4's `HostCallEvent`, not inside the outcome, so it is not duplicated here

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
fn the_real_helper_reaches_the_raw_ioctl_with_the_materialized_arrays() {
    // The reviewed draft ran this against a scripted stub, where a rejection
    // proves nothing about whether the helper still submits count_objs = 0.
    // This uses the REAL helper with a real `DRM_IOCTL_MODE_ATOMIC` on a
    // descriptor that is definitely not a DRM device. The kernel's own ioctl
    // dispatch returns ENOTTY, which is reachable only if the helper actually
    // performed the ioctl — and the request it performed it with is the one
    // carrying the property arrays. No hardware and no stub.
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
fn the_helper_reports_a_kernel_rejection_of_an_invalid_object_on_real_hardware() {
    // Object id 0 is never a valid DRM object, so a real device must reject
    // with EINVAL rather than accept an empty request. Hardware-only, so it
    // is #[ignore]d and reported separately rather than silently skipped.
    let Some(device) = TestDevice::open_real_drm_or_ignore() else { return };
    let mut executor = spawn_real_helper_for_tests(&device);
    let outcome = dispatch_and_wait_for_tests(&mut executor, invalid_object_request_for_tests());
    match outcome {
        HostCallOutcome::Rejected { errno, .. } => assert_eq!(errno, libc::EINVAL),
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

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver --test executor_async` and `cargo test -p yserver kms::executor::helper`
Expected: FAIL — the helper still submits an empty request with null pointers, and `prepare_atomic`, `HolderLedger` and `TestDevice::open_never_a_drm_device` do not exist.

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

impl PreparedAtomic {
    /// The address installed into the value slot for out-fence `slot_idx`.
    /// Exists so the unit test above can assert the *identity* of the holder
    /// pointer rather than merely that the value changed.
    #[cfg(test)]
    fn holder_address(&self, slot_idx: usize) -> u64 {
        std::ptr::from_ref(&self.holders[slot_idx]) as usize as u64
    }
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

---

### Task 4: The asynchronous host-call API

Stage 1's `dispatch` (`executor/mod.rs:293-433`) is a `libc::poll` loop that waits up to the watchdog and can `std::thread::sleep` for 100 ms deciding whether a child died. Calling it from a live path stalls the X11 core for two seconds, which `COMMIT-5` forbids and which is the exact stall section 4.1's process isolation exists to remove. The blocking form is not deleted — it is the correct call at a cold-start or final-offline boundary — but it becomes unreachable during seat-active service.

This task is executor-local. Task 5 wires the result into the core loop.

**Files:**
- Modify: `crates/yserver/src/kms/executor/mod.rs:293-433` (`dispatch`), `:496-502` (`Drop`), `:625-676` (`spawn_internal`)
- Modify: `crates/yserver/src/kms/executor/transport.rs` — a non-blocking receive that reports `WouldBlock`
- Modify: `crates/yserver/src/kms/executor/test_support.rs` — the stub behaviours below
- Test: `crates/yserver/tests/executor_async.rs` (new)
- Unchanged, and verified so: `crates/yserver/tests/executor_substrate.rs` — stage 1's six outcome tests must still pass without edits

**Interfaces:**
- Consumes: `HostCallCorrelation`, `HostCallRequest`, `HostCallReply`, `encode_request`, `decode_reply`, `MAX_OUT_FENCES` (task 2); the helper's reply behaviour (task 3); `SubmittingProof`, `ExecutorState`, `ReapState`, `ReapProof`, `UnknownReason`, `HostCallOutcome` (stage 1).
- Produces:
  - `KmsIoExecutor::{send, control_fd, poll_reply, tick, next_deadline, dispatch_blocking_at_boundary, enter_seat_active, enter_final_offline}`
  - `HostCallEvent::{Outcome, LateReply}`
  - `SendError::{AlreadyInFlight, Stalled, Reaped, Ipc, ReservationMismatch}`
  - `HostCallReservation::{Submitting(SubmittingProof), Validation(ValidationLease)}`
  - `ValidationLease` with a test-only constructor; 2b adds its production producer
  - `HostCallPhase::{ColdStart, SeatActive, FinalOffline}`, `KmsIoExecutor::phase()`, and `BoundaryViolation`
  - `StubBehaviour::AcceptAfterReturningInheritedFd { delay, ignore_termination }`
- Removes: the 100 ms `std::thread::sleep` child-exit poll. (`HostCallClass::from_request` and its callers are already gone: Task 2 deletes both together, because leaving the callers would break the build between tasks.)
- Preserves: `dispatch_for_tests`, and therefore stage 1's `crates/yserver/tests/executor_substrate.rs` unchanged. It routes through `dispatch_blocking_at_boundary`, which a freshly spawned executor permits because its phase is `ColdStart`. Those six tests are this task's regression net: the outcome classification stage 1 established must survive the split.
- Rewrites: `dispatch_and_wait_for_tests`, which Task 3's tests call. It stops wrapping `dispatch` and becomes `send` + a bounded readable-wait + `poll_reply`, returning the event's `HostCallOutcome`. Task 3's assertions are unchanged; only the helper underneath them moves. Leaving it on `dispatch` would keep a blocking host call alive in the suite that is supposed to prove there is none.

#### Why the boundary is a runtime precondition and not a token

The reviewed draft guarded the blocking call with a `BoundaryWitness` whose constructors were `pub(crate)`. That is not a guard: every seat-active module in the crate could construct one, and no external compile-fail case can prove anything about an internal caller. Rust has no visibility that says "only these two call sites".

So the boundary becomes an **observable precondition on the executor** instead. The executor knows which lifecycle phase it is in, because 2b's lifecycle transitions tell it, and `dispatch_blocking_at_boundary` returns `Err(BoundaryViolation)` when that phase is `SeatActive`. This is weaker than a compile error and stronger than a convention: it is testable, and the test below is the proof. A wrongly-placed blocking call fails loudly at its first execution rather than stalling the server for two seconds.

- [ ] **Step 1: Write the failing non-blocking tests**

```rust
// crates/yserver/tests/executor_async.rs
use std::time::{Duration, Instant};
use yserver::kms::executor::{
    HostCallEvent, HostCallOutcome, HostCallReservation, KmsIoExecutor, SendError,
    SubmittingProof, UnknownReason, test_support::{self, StubBehaviour},
};

/// The class watchdog, and therefore the longest a *correct* blocking
/// implementation could take. Asserting against it rather than against
/// 10 ms makes these tests insensitive to CI scheduling while still
/// failing any implementation that actually waits for the helper.
const SEAT_ACTIVE_WATCHDOG: Duration = Duration::from_secs(2);

#[test]
fn send_returns_without_waiting_for_a_reply_that_never_comes() {
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    let started = Instant::now();
    executor
        .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    // A helper that never replies cannot have replied. Returning at all
    // proves send did not wait for one; the ceiling catches an
    // implementation that waited out the watchdog instead.
    assert!(started.elapsed() < SEAT_ACTIVE_WATCHDOG, "send waited {:?}", started.elapsed());
    assert!(executor.poll_reply().is_none());
}

#[test]
fn poll_reply_returns_none_while_the_helper_is_busy_and_never_blocks() {
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(&small_atomic_request_for_tests(), HostCallReservation::Submitting(SubmittingProof::for_tests()))
        .expect("send");
    let started = Instant::now();
    for _ in 0..200 {
        assert!(executor.poll_reply().is_none());
    }
    // 200 blocking receives against a silent helper would take 200 watchdogs.
    assert!(started.elapsed() < SEAT_ACTIVE_WATCHDOG, "poll_reply blocked");
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
use yserver::kms::executor::ExecutorState;

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
    assert!(executor.tick(Instant::now()).is_none(), "fired before the deadline");
    let started = Instant::now();
    let event = executor.tick(Instant::now() + Duration::from_secs(3));
    assert!(started.elapsed() < SEAT_ACTIVE_WATCHDOG, "tick slept to reach the deadline");
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
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};

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
    let executor = test_support::spawn_stub_helper_with_event_fd(
        StubBehaviour::AcceptAfterReturningInheritedFd { delay, ignore_termination },
        &write_end,
    )
    .expect("spawn");
    drop(write_end); // only the helper's copy and the returned duplicate remain
    (read_end, executor)
}

fn pipe_is_at_eof(read_end: &mut std::fs::File) -> bool {
    let mut buf = [0u8; 1];
    matches!(read_end.read(&mut buf), Ok(0))
}

#[test]
fn an_accepted_reply_adopts_its_out_fence_and_closes_it_exactly_once() {
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
    assert!(!pipe_is_at_eof(&mut read_end), "closed before the owner dropped it");
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
use yserver::kms::executor::{BoundaryViolation, HostCallPhase, ValidationLease};

#[test]
fn the_blocking_form_is_refused_once_the_seat_is_active() {
    let mut executor = test_support::spawn_stub_helper(StubBehaviour::RejectWith(libc::EINVAL))
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
}

#[test]
fn validation_and_seat_active_commits_share_the_two_second_watchdog() {
    for request in [validation_request_for_tests(), small_atomic_request_for_tests()] {
        assert_eq!(request.class().watchdog(), Duration::from_secs(2));
    }
    assert_eq!(blocking_atomic_request_for_tests().class().watchdog(), Duration::from_secs(30));
}
```

- [ ] **Step 5: Run the tests to verify they fail**

```bash
cargo test -p yserver --test executor_async
```
Expected: FAIL — `send`, `poll_reply`, `tick`, `HostCallPhase` and the new stub behaviours do not exist.

- [ ] **Step 6: Write the in-flight state machine**

The executor owns its in-flight state, so no caller can hold a token that desynchronizes from it:

```rust
struct InFlight {
    correlation: HostCallCorrelation,
    class: HostCallClass,
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
    self.state = ExecutorState::Stalled;
    self.request_termination();
    Some(HostCallEvent::Outcome { correlation, outcome: HostCallOutcome::Unknown(reason) })
}
```

`send(&mut self, request, reservation)` refuses, in order: `Reaped` when the helper is reaped, `Stalled` when the state is `Stalled` or `ShutdownStalled`, `AlreadyInFlight` when `in_flight.is_some()`, and `ReservationMismatch` when the reservation kind does not match the request class (`Validation` for `SeatActiveValidation`, `Submitting` for the two live classes). Otherwise it installs `InFlight` **before** the write — so a transport error cannot leave a record with no outcome — encodes, and sends. On transport error it queues `terminalize_unknown(IpcFailure)` and returns `Err(SendError::Ipc)`. The `Dispatched` milestone belongs at send time, not at reply time; 2b's owner sets it when `send` returns.

`control_fd` returns `None` once reaped, `Some(self.control.as_fd())` otherwise.

`next_deadline()` returns `self.in_flight.as_ref().filter(|f| f.terminalized.is_none()).map(|f| f.deadline)`. Task 5 feeds it to `KmsBackend::next_wakeup`; without it the core would block indefinitely and the watchdog would never fire.

`poll_reply()` performs one non-blocking `recv_frame`:

- `WouldBlock` → `None`.
- EOF or a receive error → `check_child_exited()` decides the reason (`HelperExited` if the child is reaped, else `IpcFailure`), then `terminalize_unknown(reason)`. After a prior terminalization this returns `None`, which is the EOF-after-watchdog case: reap progress, not a second outcome.
- A frame that fails `decode_reply`, whose correlation differs from `in_flight.correlation`, or whose fd count disagrees with `out_fence_mask` → close every received descriptor exactly once, then `terminalize_unknown(MalformedReply)`.
- A valid `Ready` reply outside the handshake → `terminalize_unknown(MalformedReply)`.
- A valid, correlated reply → if `terminalized.is_some()`, emit `LateReply` with its adopted fds and **keep** `in_flight` and `Stalled`, because a late reply is not reap proof. Otherwise emit `Outcome` and clear `in_flight`.

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

- [ ] **Step 8: Write the stub behaviours**

`test_support.rs` gains three variants and their `to_arg_string`/`from_arg_str` round trips, plus the helpers the tests import:

- `AcceptAfterReturningInheritedFd { delay: Duration, ignore_termination: bool }` — encoded as `accept-fd-after:<ms>:<0|1>`. Sleeps `delay`, replies `Accepted` with `out_fence_mask = 1`, passes one `dup` of its inherited `KMS_FD` as the fence, then closes that duplicate. When `ignore_termination` is set it installs `SIG_IGN` for `SIGTERM` first, which is what lets the late-reply test observe a reply after the watchdog.
- `ReplyWithForeignCorrelation` — replies `Accepted` with a correlation whose `lifecycle_epoch` is `u64::MAX`.
- `AcceptDeclaringMissingFence` — replies `Accepted` with `out_fence_mask = 1` and no descriptor attached.
- `test_support::{pipe_pair, wait_readable, wait_for_helper_exit, kill_helper, kill_and_reap, reap_within}` — `wait_readable` is a bounded `libc::poll` in the *test harness*, not in `executor/mod.rs`, so it does not affect Task 7's single-polling-site gate.

- [ ] **Step 9: Run the tests to verify they pass**

```bash
cargo test -p yserver --test executor_async
cargo clippy --all-targets -- -D warnings
```
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/yserver/src/kms/executor/mod.rs crates/yserver/src/kms/executor/transport.rs \
        crates/yserver/src/kms/executor/test_support.rs crates/yserver/tests/executor_async.rs
git commit -m "feat(kms): split the host call into send, poll and watchdog"
```

---

### Task 5: The executor as a real core-loop source

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
- Consumes: `KmsIoExecutor::{control_fd, poll_reply, tick, next_deadline}`, `HostCallEvent` (task 4).
- Produces:
  - `BackendFdKind::ExecutorControl`
  - `Backend::on_executor_readable(&mut self, state: &mut ServerState)` — defaulted no-op
  - `RecordingBackend::{with_wakeup_deadline, with_before_block_notification, with_executor_readable_notification}`
  - `PlatformInitDevice.executor: KmsIoExecutor` and `KmsDevice.executor: KmsIoExecutor`
  - `KmsPlatform::{executor_deadline, drain_executor_events, tick_executors}`

Adding a defaulted trait method rather than a required one is deliberate: `recording.rs:1121` and `host_x11/trait_impl.rs:176` both implement `Backend`, and neither has an executor. A required method would force empty implementations into two files with nothing to do.

- [ ] **Step 1: Write the failing core-loop tests**

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

- [ ] **Step 2: Write the failing KMS-side tests**

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
fn a_reaped_executor_is_withdrawn_from_the_poll_set() {
    // Registering a closed fd would make the poller spin on an invalid
    // source. control_fd() returns None once reaped, and poll_fds must
    // honour that rather than unwrapping it.
    let mut platform = platform_with_stub_executors_for_tests(1);
    reap_every_executor_for_tests(&mut platform);
    assert!(
        !platform.poll_fds().iter().any(|(_, k)| matches!(k, BackendFdKind::ExecutorControl)),
        "a reaped executor must not remain registered"
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
fn on_executor_readable_drains_every_pending_event() {
    let mut backend = backend_with_stub_executor_for_tests();
    let mut state = yserver_core::server::ServerState::new();
    send_rejected_host_call_for_tests(&mut backend);
    wait_executor_readable_for_tests(&backend, Duration::from_secs(5));
    yserver_core::backend::Backend::on_executor_readable(&mut backend, &mut state);
    assert_eq!(backend.drained_host_call_events_for_tests().len(), 1);
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p yserver-core executor_control
cargo test -p yserver-core a_backend_deadline_wakes_the_core
cargo test -p yserver kms::render::platform::tests::poll_fds_publishes
```
Expected: FAIL — `BackendFdKind::ExecutorControl` and `KmsDevice.executor` do not exist.

- [ ] **Step 4: Add the core-side variant, hook and dispatch**

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

- [ ] **Step 5: Give the production device an executor**

`PlatformInitDevice` (`kms/backend.rs:677-681`) and `KmsDevice` (`platform.rs:1990-1994`) each gain an `executor: KmsIoExecutor` field. In `platform_init` (`kms/backend.rs:844`), immediately after `primary_device_key_from_fd` qualifies the device and before it is pushed into `devices`, spawn its executor:

`platform_init` has no incarnation in scope today, and this stage does not add
lifecycle management: it allocates `IncarnationId::first()` once at the top of
`platform_init` and passes the same value to every device's executor. Reopen and
later incarnations belong to 2b's lifecycle, which is also what will make
`enter_seat_active` reachable.

```rust
let incarnation = IncarnationId::first(); // one per platform_init; 2b owns reopen
let executor = KmsIoExecutor::spawn(std::os::fd::AsFd::as_fd(&*device), incarnation)
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

- [ ] **Step 6: Publish the source, the deadline and the drain**

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

- [ ] **Step 7: Drive it from the KMS backend**

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

- [ ] **Step 8: Run the tests to verify they pass**

```bash
cargo test -p yserver-core
cargo test -p yserver
cargo clippy --all-targets -- -D warnings
```
Expected: PASS. `recording.rs` and `host_x11/trait_impl.rs` need no edits, because `on_executor_readable` is defaulted.

- [ ] **Step 9: Commit**

```bash
git add crates/yserver-core/src/backend/trait_def.rs crates/yserver-core/src/backend/recording.rs \
        crates/yserver-core/src/core_loop/run.rs crates/yserver/src/kms/backend.rs \
        crates/yserver/src/kms/render/platform.rs crates/yserver/src/kms/render/backend.rs
git commit -m "feat(kms): drive the executor from the core event loop and its wakeup chain"
```

---

### Task 6: The `COMMIT-7` device lock, held by the executor

`COMMIT-7` says the lock is "taken by the executor for as long as it lives and released only by its death" (`spec:712-719`). The case it exists for is the parent dying while a helper is wedged: a parent-held lock is released by the parent's exit, and a new server then installs state underneath the still-live helper.

Three facts shape the implementation, and the third makes the obvious version wrong. `flock` is associated with the open file description; it survives `execve`; duplicated descriptors share the lock and it is released only when **all** of them are closed — **but also by an explicit `LOCK_UN` on any one of them**. `DeviceLock::drop` currently calls `flock(fd, LOCK_UN)` (`device_lock.rs:185-191`), so the parent dropping its guard would release the helper's lock too.

Task 5 already gave every production device an executor. This task puts the lock into that same spawn path.

**Files:**
- Modify: `crates/yserver/src/kms/executor/device_lock.rs:127-191,210-272`
- Modify: `crates/yserver/src/kms/executor/mod.rs:32-33,625-676` — the `LOCK_FD` slot and the readiness handshake
- Modify: `crates/yserver/src/kms/executor/helper.rs:72-76` — adopt it
- Modify: `crates/yserver/src/kms/backend.rs:840-875` — take the lock before the executor spawn added in Task 5
- Test: `crates/yserver/tests/executor_lock_handoff.rs`

**Interfaces:**
- Consumes: `may_install_state`, `DeviceLock`, `DrmDeviceKey`, `LOCK_HOLDER_ARG`, `run_lock_holder_if_requested` (stage 1); `KmsIoExecutor::spawn`, `HostCallRequest::Ready` (tasks 2, 4, 5).
- Produces:
  - `LOCK_FD: RawFd = 200`
  - `DeviceLock::{release_explicitly, into_inheritable}` and `InheritableDeviceLock`
  - `KmsIoExecutor::{spawn_with_device_lock, spawn_with_device_lock_at, await_helper_ready, helper_pid}` — `spawn_with_device_lock_at` takes an explicit executable path so the failed-spawn test can name one that does not exist; `helper_pid()` returns `libc::pid_t`, widened from the wire's `u32`
  - `LOCK_HANDOFF_ARG` and its `run_lock_handoff_if_requested()` entry point
  - `OpenError::LockUnavailable { device, recorded_holder }` with `installs_attempted() -> usize`
  - `open_kms_device_for_tests` and `discover_kms_candidates_for_tests` — thin test entry points over `platform_init` and the existing candidate discovery, so the refusal path is exercised without a full bring-up

#### Why a type state and not one guard with an extra method

The reviewed draft had `into_inheritable` "yield the raw descriptor" while the parent later dropped "its `DeviceLock`". Those cannot both be true: a consuming method cannot leave its receiver available to drop, and a bare `RawFd` does not say who closes it if the spawn fails. Worse, `release_explicitly` stayed callable on a guard whose open file description the helper now shares — the exact global unlock this task exists to remove.

So the handoff is a type transition. `DeviceLock` is the pre-handoff guard and keeps `release_explicitly`. `into_inheritable` consumes it and returns `InheritableDeviceLock`, which owns the descriptor, has **no** unlock operation at all, and whose `Drop` only closes. The compiler, not a comment, is what stops a post-handoff global unlock. Ownership on the failure path is equally explicit: `spawn_with_device_lock` borrows the `InheritableDeviceLock`, so a failed spawn leaves the caller holding it, and the caller drops it — closing its descriptor and, since no helper ever inherited one, releasing the lock as the last close.

- [ ] **Step 1: Write the failing unlock-semantics tests**

```rust
// crates/yserver/src/kms/executor/device_lock.rs, in the existing #[cfg(test)] module
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

- [ ] **Step 2: Write the failing handoff tests**

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
        KmsIoExecutor::spawn_with_device_lock(dummy.as_fd(), IncarnationId::first(), &inheritable)
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

/// M-7's real threat model: not "the parent dropped a value" but "the parent
/// process died". The handoff subprocess acquires the lock, spawns a helper
/// that inherits it, prints the helper's pid, and `_exit`s without reaping.
/// The helper is then reparented to init while this test still runs.
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
fn a_start_while_the_lock_is_held_refuses_rather_than_installing() {
    let key = DrmDeviceKey { major: 226, minor: 249 };
    let held = may_install_state(&key).expect("holder");
    let err = open_kms_device_for_tests(&key).unwrap_err();
    assert!(matches!(err, OpenError::LockUnavailable { .. }));
    assert_eq!(err.installs_attempted(), 0, "no state may be installed on refusal");
    drop(held);
}

#[test]
fn discovery_probing_takes_no_install_lock() {
    // discover_kms_candidates opens every card read-only to enumerate
    // connectors and installs nothing, so it must not be blocked by a lock
    // an earlier incarnation's helper still holds.
    let key = DrmDeviceKey { major: 226, minor: 248 };
    let held = may_install_state(&key).expect("holder");
    assert!(discover_kms_candidates_for_tests().is_ok());
    drop(held);
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p yserver kms::executor::device_lock
cargo test -p yserver --test executor_lock_handoff
```
Expected: FAIL — `Drop` still unlocks, and there is no `InheritableDeviceLock`, `LOCK_FD` or handoff entry point.

- [ ] **Step 4: Replace the destructor and add the type transition**

```rust
impl Drop for DeviceLock {
    /// Deliberately does NOT `LOCK_UN`. `flock` releases on last close of the
    /// open file description, and an explicit unlock through any duplicate
    /// releases it for all of them — including the executor's inherited copy,
    /// which COMMIT-7 requires to outlive this process. Closing our
    /// descriptor is exactly the semantics we want.
    fn drop(&mut self) {
        // `self.file` closes here; no explicit unlock.
    }
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

- [ ] **Step 5: Inherit the lock and prove the helper adopted it**

The handoff mirrors `CONTROL_FD` and `KMS_FD` exactly (`executor/mod.rs:589-663`):

1. `platform_init` calls `may_install_state(&device_key)` before installing anything and before Task 5's executor spawn. A refusal names the device, says an earlier incarnation's helper may still mutate it, is **not** retried in a loop, and returns `OpenError::LockUnavailable` with no state installed.
2. `into_inheritable()`, then `KmsIoExecutor::spawn_with_device_lock(kms_fd, incarnation, &inheritable)`. Its `pre_exec` adds one line beside the existing two: `duplicate_to_inherited_slot(lock_source, LOCK_FD)?`, where `lock_source` is a `duplicate_fd_at_least` of the lock fd. `dup2` clears `FD_CLOEXEC`, so the copy survives the exec.
3. `run_executor_helper` (`helper.rs:72-76`) adopts it with `take_inherited_fd(LOCK_FD, "executor device lock")` and holds the `OwnedFd` for the process lifetime. It re-asserts `LOCK_EX | LOCK_NB` on the inherited descriptor as a liveness assertion — the same open file description, so it is a no-op conversion that cannot fail; a failure means the descriptor is not the lock and the helper exits non-zero rather than serving. The helper takes `LOCK_FD` only when it is present, so the stub and lock-free spawn paths keep working.
4. `await_helper_ready` sends `HostCallRequest::Ready` and blocks for the reply under the 30-second cold-start watchdog. This is a permitted blocking boundary: it runs during `platform_init`, before any seat-active service, and it is the only thing that proves the exec succeeded and `LOCK_FD` was adopted.
5. **Only then** does the caller drop the `InheritableDeviceLock`. With the destructor above, that closes one descriptor of a shared description and releases nothing.

There is no window in which the lock is unheld: from step 1 the parent holds it, from step 2 both hold the same description, and after step 5 only the helper does. Step 4 is what makes step 5 safe — dropping before the readiness reply could release the lock if the exec had failed.

- [ ] **Step 6: Add the handoff subprocess entry point**

Beside stage 1's `run_lock_holder_if_requested` (`device_lock.rs:216-252`), which the `yserver` binary already calls before argument parsing:

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
    let mut executor = KmsIoExecutor::spawn_with_device_lock(
        dummy.as_fd(), IncarnationId::first(), &inheritable,
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

- [ ] **Step 7: Run the tests to verify they pass**

```bash
cargo test -p yserver kms::executor::device_lock
cargo test -p yserver --test executor_lock_handoff
cargo clippy --all-targets -- -D warnings
```
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/yserver/src/kms/executor/device_lock.rs crates/yserver/src/kms/executor/mod.rs \
        crates/yserver/src/kms/executor/helper.rs crates/yserver/src/kms/backend.rs \
        crates/yserver/tests/executor_lock_handoff.rs
git commit -m "feat(kms): hand the device lock to the executor and stop unlocking on drop"
```

---

### Task 7: Portable gates and the stage reviewability check

These greps are a coarse net, not the proof. Every invariant below is already asserted behaviourally by a test in Tasks 4, 5 and 6; the greps exist to catch a *reintroduction* in a later edit that no existing test happens to cover. Where the reviewed draft used a source-text assertion **instead of** a behavioural one, the behavioural test has replaced it.

- [ ] **Step 1: Run the full local gate**

```bash
cargo +nightly fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p yserver-core
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

- [ ] **Step 3: Verify no blocking wait was reintroduced on a core-thread path**

```bash
rg -n 'std::thread::sleep' crates/yserver/src/kms/executor/
rg -n '\.wait\(\)' crates/yserver/src/kms/executor/
rg -c 'libc::poll' crates/yserver/src/kms/executor/mod.rs
```
Expected: no `sleep` anywhere under `executor/`; **no `Child::wait()` anywhere under `executor/`** — the stage-1 destructor's synchronous `wait()` is the specific regression this catches, and it is the one the reviewed draft's `libc::poll`-only scan would have declared clean; exactly one `libc::poll`, inside `dispatch_blocking_at_boundary`.

- [ ] **Step 4: Verify the lock and source invariants**

```bash
rg -n 'LOCK_UN' crates/yserver/src/kms/executor/device_lock.rs
rg -n 'BackendFdKind::ExecutorControl' crates/yserver-core/src/core_loop/run.rs \
      crates/yserver/src/kms/render/platform.rs
rg -n 'open_any_drm_or_skip' crates/yserver/src/
```
Expected: `LOCK_UN` appears only inside `release_explicitly`, and nowhere in `InheritableDeviceLock`'s implementation; the executor source is both published by `poll_fds` and dispatched by the core loop; no skip-shaped test helper anywhere.

- [ ] **Step 5: Confirm the deliberate scope boundary is still intact**

```bash
rg -n 'SequenceSupport' crates/yserver/src/kms/render/backend.rs | head -3
```
Expected: still present. This stage does **not** move it — spec lines 1755-1763 require it inside 2b's epoch-local clock record, which does not exist yet. The grep is here so an executor of this plan does not "helpfully" start that migration, and so a reviewer sees the omission is deliberate.

- [ ] **Step 6: Update the status document**

Record that the executor substrate is complete and asynchronous, that it is a real core-loop source with a real deadline, that the device lock is executor-held, and that no owner or call-site conversion exists yet.

- [ ] **Step 7: Commit**

```bash
git add docs/status.md
git commit -m "docs(kms): record the stage 2a executor substrate"
```

---

## Stage exit criteria

- The three portable builds pass, `cargo clippy --all-targets -- -D warnings` is clean, and both crates' suites are green.
- A real property list crosses the wire, the helper owns the holder storage, and every returned descriptor is adopted and closed exactly once — observed through a pipe's EOF, not asserted through instrumentation that cannot see an `OwnedFd` close.
- Every reply echoes its request's correlation tuple, and a mismatch is `MalformedReply` — never a rejection. The clock-probe tuple carries topology generation, so stage 1's request is not regressed.
- A frame whose class, flags and payload disagree is refused by both the encoder and the decoder, so a live commit cannot be labelled validation and a seat-active commit cannot omit `NONBLOCK`.
- **No seat-active path waits on a host call.** `send` returns after the frame is sent, replies arrive through `poll_reply`, the watchdog fires from `tick`, `libc::poll` appears exactly once, no `std::thread::sleep` remains on any host-call path, and `Drop` no longer calls `Child::wait()`.
- The core event loop registers the control fd **and** carries the executor deadline in `next_wakeup`: the asynchronous API has a consumer and the watchdog is reachable on an idle server.
- Exactly one host call is in flight at a time. Every acceptance-unknown path — send failure, EOF, receive failure, malformed reply, watchdog expiry — produces exactly one terminal event, enters `Stalled`, and retains `in_flight` until a proven reap, so no second ioctl reaches a device whose acceptance is unknown. A reply arriving after terminalization is delivered as `LateReply` with its descriptors adopted, and is not reap proof.
- `DeviceLock::drop` does not unlock; `release_explicitly` does not exist after the handoff transition; the lock is inherited by the helper across the re-exec and proven adopted by a readiness reply before the parent drops its copy; the lock survives the death of the process that acquired it; and a start attempted while an orphaned helper holds it refuses without installing state.
- Identity allocation is checked and cannot wrap.

## What stage 2b consumes

- `KmsIoExecutor::{send, control_fd, poll_reply, tick, next_deadline}` and `HostCallEvent`. 2b's owner replaces the backend's `record_host_call_events` log-and-discard queue as the consumer.
- `SubmittingProof` and `ValidationLease` keep their test-only constructors here; 2b adds the production producers at record installation and at validation-lease acquisition, and 2b's clock probe adds the non-atomic host-call reservation.
- `HostCallCorrelation` is what 2b's records match a reply against.
- `HostCallPhase` and `enter_seat_active`/`enter_final_offline`: 2b's lifecycle transitions are what call them, which is what makes the `COMMIT-5` boundary check meaningful rather than permanently `ColdStart`.
- The `Dispatched` milestone is set when `send` returns, not when the reply arrives.
- The `SequenceSupport` map at `kms/render/backend.rs:1042` is 2b's to move into the epoch-local clock record.

## Self-review notes

- **Scope.** Six implementation tasks plus the gate, against stage 1's fourteen. Every task delivers something testable without an owner: the identities, the wire, the helper, the async API, its loop integration, and the lock.
- **What the second review changed.** Tasks 2, 4 and 6 were rewritten whole rather than patched, and the event-loop integration became its own task once the real `BackendFdKind`/`run.rs`/`poll_fds` surface was read instead of assumed. Three claimed guarantees were downgraded to what can actually be proven: the boundary check is a tested runtime precondition rather than a forgeable witness, descriptor closure is observed through a pipe's EOF rather than through a ledger that cannot see `OwnedFd`, and the timing assertions use the class watchdog as their ceiling rather than 10 ms.
- **One deliberate conservatism.** A failed `send` on a `SOCK_SEQPACKET` did not enqueue the datagram, so `FailedBeforeSubmit` would arguably be provable. The plan classifies it `Unknown` anyway: `COMMIT-6`'s default is unknown, the cost is one quarantine on a path where the helper is usually dead regardless, and a wrong `FailedBeforeSubmit` would release resources the kernel might still own.
- **One thing this stage still cannot prove.** Helper-side exactly-once closing is asserted by the helper's own unit tests and reported through `unexpected_fence_output`. The parent-side pipe test proves the parent closes what it adopts; it does not prove the helper closed its copies, because a helper that leaked one would keep the pipe open and the test would fail — which makes it a *joint* assertion, not a helper-side one. The plan says so rather than implying separable coverage it does not have.
- **One thing this stage deliberately does not do.** The stage-1 `SequenceSupport` gap is left in place, with a grep in Task 7 to keep it that way, because the record it must move into is 2b's.
