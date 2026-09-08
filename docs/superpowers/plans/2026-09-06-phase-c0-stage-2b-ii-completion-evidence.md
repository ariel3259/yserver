# Phase C.0 stage 2b-ii: completion evidence implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Read the normative contract before any individual task. This document is a plan, not permission to merge C.0.

**Goal:** Extend the existing device commit owner with correlated clock/event and canonical fence evidence, independent completion deadlines, and an exact install/restore qualification gate.

**Architecture:** Keep the one generic `DeviceCommitOwner<R>`, the one device atomic slot, and the one `OwnerEvent<R>` stream. The owner owns epoch-local clock/arm records and completion state; a stable platform readiness aggregator wakes it for dynamically adopted out-fences. A narrow compatibility adapter preserves the still-unconverted Phase A+B producers without allowing their untagged events to become C.0 evidence.

**Tech Stack:** Rust, the process-isolated KMS executor (baseline protocol v2, extended to v3 for asynchronous QUEUE_SEQUENCE), raw DRM event parser, canonical sync-file status, and the existing epoll/kqueue readiness abstraction.

**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`, approved revision 2, especially §§6.3, 10, 10.1–10.3 and 18.

**Baseline:** `28b34dbab6db2dd835704e252a13e051aa714fcc`, branch `feat/phase-c0-atomic-kms-migration`. Stage 2b-i code commits are `04a52dec` and `617bfb02`; their separate plan fold-backs are `5802eafa` and `28b34dba`.

**Status:** Revision 5's remaining handover finding is closed by `../findings/2026-09-07-phase-c0-stage-2b-ii-handover-failure-review.md`: zero findings, COMPLETE FOR DECLARED SCOPE. Together with the preceding scoped review's three accepted corrections, the known design findings are resolved. Ready for task-by-task implementation on user authorization; no production implementation or tests have been performed. Scoped verdicts do not certify every unchanged contract or replace implementation gates.

## Global constraints

- `COMMIT-5`: send and return; never synchronously wait for executor replies, clock probes or fences on the X11 core. The helper never reads the DRM event fd.
- `COMMIT-6`: reserve/install before IPC; only proven rejection releases a never-accepted submission. Unknown retains the atomic slot and both possible resource states. No recovery or quarantine release is added here.
- `COMMIT-2`: `Accepted`, `HardwareComplete`, `Presented` and `PriorBufferReleased` remain independent. This stage never sets `prior_buffer_released`.
- `ID-3`: match the entire executor correlation, including request sequence and event token. Kernel events do not transport lifecycle/generation fields: resolve those from their live record, never invent them.
- Every C.0 nonblocking class requires successful canonical status for every expected out-fence; every Present consumer also requires its own validated page event. A page event cannot close a fence.
- Validation has no record, live resources, page events or out-fences. Its lease protects the passed-validation/live-call interval; it may not be consumed by a different persistent request.
- All identity increments and deadline arithmetic are checked. No token reuse within an incarnation; exactly 64 commit tombstones.
- Production remains `DeviceCommitOwner<NeverResource>` until 2c supplies real RAII framebuffer/BO/pin owners. Never introduce handle-shaped resources to pretend that ownership exists.
- No new dependency is needed. Use `cargo +nightly fmt`; before each implementation commit run `cargo clippy --all-targets -- -D warnings` and `cargo test -p yserver`.
- Portable gates: `cargo check -p yserver --target x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, and `x86_64-unknown-freebsd`. An unsupported runtime status query fails closed; compiling on FreeBSD is not proof of runtime sync-file support there.

## Scope and continuity

Read both `2026-09-03-phase-c0-stage-2-plan-adversarial-review.md` and `2026-09-03-phase-c0-stage-2-plan-review-round2.md` under `docs/superpowers/findings/`. Their scope-independent lessons apply; their historical finding counts are not comparable to the pinned review instrument.

The following are **in scope**: moving the still-device-keyed `SequenceSupport` decision into hardware-CRTC/epoch clock records; asynchronous GET_SEQUENCE probes; sequence-arm identity, cancellation and clock samples; exclusive raw-event drain and correlation; MSC/UST normalization; sync-file status and ownership; completion/qualification state; deadline integration; deterministic and real-stub-helper tests.

The following remain **out of scope**: six production atomic call-site conversions, admission tiers/intents/fairness, `ScanoutM2State` successor and BO transitions, Present Flip/Skip/Idle/release terminalization, applying damage, real resource ownership, recovery/reopen/fd-set retirement, cursor transport, gamma conversion, C.1 async-direct, and hardware performance/soak qualification. These belong to 2c, stage 3, stage 4, or C.1. This plan neither claims C.0 operational readiness for the existing server nor qualifies an incarnation from an ordinary Phase A+B commit.

### Verified baseline and consequences

| Existing code | Consequence for this plan |
| --- | --- |
| `kms/owner/device.rs`: split `begin`/`send_on`, passed-validation gate, full correlation, 64 tombstones | Extend it; do not create another owner or submission facade. |
| `kms/owner/record.rs`: slot table survives request transfer; `FenceEvidence::by_crtc`; no completed-ledger extraction | Add evidence state and consuming accepted-ledger transfer. |
| `kms/executor/mod.rs`: `ClockProbeLease` has only `for_tests()` | A real private production issuer is required before production probes. |
| `kms/render/backend.rs:1044`: `HashMap<(DrmDeviceKey, ClockEpochId), SequenceSupport>` | Remove the map and all construction/read/write sites in Task 1. |
| `backend.rs:1514–1558`: arm table omits device ownership in each arm and has no bounded consumers | Move arms into each device owner; add full clock identity and cancellation. |
| `platform.rs:4243`: drain discards flip `user_data`, synthesizes MSC when raw sequence is zero | Preserve raw fields until owner classification. Legacy synthesis cannot reach C.0 clocks. |
| `platform.rs:4150`: old-event discard is a second raw-reader call site | Route it through the same owner drain; preserve its existing legacy boundary until stage 3 converts lifecycle waits. |
| `core_loop/run.rs:1251`: stable backend fd kinds dispatch callbacks | Add one stable owner-completion source; do not append per-commit fds to the one-time core source snapshot. |
| `kms/render/completion_poller.rs`: register/unregister, no waiting API | Reuse as an aggregator; scan owner-held fds with zero-timeout poll. |
| `HostCallOutcome::ProbeAccepted` carries sequence and helper/round-trip durations, but no kernel timestamp | It seeds the extension reference, not a timestamped protocol sample. |
| `drm/page_flip.rs::queue_crtc_sequence` calls ioctl on the caller thread | Task 3 moves this syscall into the helper, with its own request/reply family. |
| `core_loop/run.rs` calculates poll timeout before `before_block` | Task 7 moves servicing before timeout calculation. |

## Normative contract: define once, consume everywhere

All new owner modules are `#[doc(hidden)] pub` when their types appear in public owner APIs or integration tests. `DrmEventRecord`, `EventParseError` and `parse_event_buffer_partial` become `#[doc(hidden)] pub`; do not create a second event parser for tests. Low-level fd status querying remains in `platform/`, not in the state machine.

### Clock identity, state and normalization

Create `kms/owner/clock.rs`. A device owner supplies incarnation; its map is keyed by `(hardware_crtc, ClockEpochId)`, never RANDR XID alone. Store lifecycle epoch and topology generation in each active clock identity. A changed identity invalidates probes, arms, trusted reference and timestamp samples before any replacement is admitted.

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct ClockKey {
    pub hardware_crtc: u32,
    pub epoch: ClockEpochId,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ClockSource { Unresolved, KernelSequence }

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProbeState { NotStarted, InFlight(ClockProbeId), Succeeded, Failed }

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ClockSample { pub msc: u64, pub ust: u64 }

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ClockError {
    Unresolved, BadMicroseconds, Overflow, HalfRange, NoRepresentative,
    NegativeTimestamp, Regression,
}

#[derive(Debug)]
pub struct CrtcClock {
    pub key: ClockKey,
    pub lifecycle_epoch: LifecycleEpochId,
    pub topology_generation: u64,
    pub source: ClockSource,
    pub probe: ProbeState,
    pub queue_failed: bool,
    pub reference: Option<u64>,
    pub latest: Option<ClockSample>,
}
```

`ClockEpochId` already derives `Ord, PartialOrd`; preserve it. `CrtcClock::new(key, lifecycle_epoch, topology_generation)` initializes Unresolved/NotStarted/false/None/None. `install_reference(sequence)` sets KernelSequence/Succeeded/reference but leaves `latest` absent. `page_sample(raw, sec, usec)` is a pure checked normalization, returning `Result<ClockSample, ClockError>` without mutation. `observe(sample)` advances neither MSC nor UST backwards and returns whether the general clock changed. Older otherwise-valid samples are drained as late; an active Present sample whose epoch reference cannot normalize, or whose timestamp moves backwards while MSC advances, is contradictory evidence.

Use this arithmetic, not a cast of `u32` to `u64` and not zero-triggered software fallback:

```rust
pub fn extend_sequence(reference: u64, raw: u32) -> Result<u64, ClockError> {
    let delta = raw.wrapping_sub(reference as u32);
    if delta == 0x8000_0000 { return Err(ClockError::HalfRange); }
    let candidate = if delta < 0x8000_0000 {
        reference.checked_add(u64::from(delta))
    } else {
        reference.checked_sub(u64::from(0u32.wrapping_sub(delta)))
    };
    candidate.ok_or(ClockError::NoRepresentative)
}

pub fn page_ust(sec: u32, usec: u32) -> Result<u64, ClockError> {
    if usec >= 1_000_000 { return Err(ClockError::BadMicroseconds); }
    u64::from(sec).checked_mul(1_000_000)
        .and_then(|v| v.checked_add(u64::from(usec)))
        .ok_or(ClockError::Overflow)
}
```

A sequence sample accepts `time_ns >= 0`, converts to microseconds by division by 1000, validates current arm identity and target first, then may advance reference and the general clock. Use the existing `yserver_core::present_scheduler::msc_is_after` comparison for protocol target ordering. A sample below the arm's scheduled target cannot fulfill that arm or advance the clock; a contradictory current arm poisons, an unknown/cancelled arm is telemetry only.

### Probe serialization

Move the definition of `ClockProbeLease` from executor to `owner/slot.rs`, re-export it from executor, preserving the public `for_tests` seam. Add a private `issue()` plus `DeviceSlot::acquire_probe(id)` / `release_probe(id)`. Probe reservation excludes submitted commit and validation reservation in both directions. `SlotError::ProbeOutstanding(ClockProbeId)` and `NoProbeLease(ClockProbeId)` describe failures.

Owner methods:

```rust
pub fn begin_clock_probe(&mut self, key: ClockKey)
    -> Result<ClockProbeId, DispatchError<R>>;
pub fn send_clock_probe_on(&mut self, executor: &mut KmsIoExecutor)
    -> Result<Vec<OwnerEvent<R>>, DispatchError<R>>;
```

The owner stores one pending request/lease and one full in-flight `HostCallCorrelation::ClockProbe`; it uses the same checked request sequence allocator as atomics and a checked `ClockProbeId`. At most one probe is in flight per device. Build identity from the actual clock record, not caller-supplied correlation. `begin`/`begin_validated` refuse event-bearing work unless every kernel-event CRTC has a current KernelSequence clock and no queue failure. Do not require incarnation qualification here: the designated real install may itself carry events while qualification is closed.

Dispatch refusal before IPC abandons the probe lease and leaves its clock NotStarted; no ioctl attempt occurred. Successful send makes InFlight. A current `ProbeAccepted` installs the reference exactly once. Current explicit rejection or contradictory shape makes Failed/Unresolved and closes qualification; no same-epoch retry. Unknown terminalizes the probe as Failed and retains probe exclusion while the executor may still execute. Late/stale replies are disposed before looking up probes. Invalidation cancels the logical probe but cannot release an unresolved executor lease; stage 3 owns reap/replacement. No `.poll_reply()` loop inside a probe method.

Add `OwnerEvent::ClockProbeResolved { key, outcome }`, with `ProbeOutcome::{Ready { reference: u64 }, Rejected { errno: i32 }, Unknown(UnknownReason), Contradictory}`. Probe events cannot carry commit resources or mark commit milestones.

### Sequence arms and consumers

Create `owner/sequence.rs`. Task 3 also corrects the baseline identity representation to spec §10: one raw nonzero-u64 counter per device incarnation, initialized to zero so its first allocation returns 1. Commit-event and sequence-arm allocations both increment that same counter with checked_add; remove purpose high bits, incarnation seeding, COUNTER_MASK and range partitioning. Keep distinct EventToken/SequenceArmToken Rust newtypes, but both from_user_data functions validate only nonzero. Target kind comes from the live/tombstoned owner record, never raw-bit decoding. Incarnation remains independently supplied by the fd owner and full executor correlation; a new incarnation may start at 1 only under the existing fresh-incarnation rules. Remove backend ids after moving all sequence callers to their device owner.

Update protocol goldens and every tagged_for_tests use in the same protocol-v3 task: remove tagged fixture constructors, build fixtures with nonzero raw values or the actual live record correlation, and replace tests of bit-tag rejection with tests of wrong request/reply family and wrong current record event type. Alternating atomic/sequence/atomic allocations must yield raw 1/2/3; test exhaustion without wrap/reuse and two incarnations with equal raw values selected by their source fd. No old raw token can change type while that incarnation survives. This corrects an existing spec mismatch; it is not a new token-policy choice.

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum SequencePurpose { IdleClockWake, PresentTargetWake }

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct SequenceConsumer(pub u64);

#[derive(Debug)]
pub struct SequenceArm {
    pub token: SequenceArmToken,
    pub key: ClockKey,
    pub lifecycle_epoch: LifecycleEpochId,
    pub topology_generation: u64,
    pub purpose: SequencePurpose,
    pub requested_target: u64,
    pub scheduled_target: Option<u64>,
    pub consumers: BTreeSet<SequenceConsumer>,
}
```

Use a device-owned `SequenceArms` with a token map and a `(ClockKey, purpose, target)` dedup index. Limit active arms to 256 per device and logical consumers to 4096 per device; exceeding either returns an explicit capacity error and cancels no existing consumer. These are storage bounds, not admission priority tiers. Reuse a target's arm while any consumer remains; remove the arm and its index as soon as its last consumer leaves. Keep 64 identity-only sequence tombstones for diagnostics; eviction cannot create a match because tokens never repeat.

`reserve_arm` installs before IPC and deduplicates by immutable requested target, never returned scheduled target. Relative requests send sequence 1; absolute requests send requested target. A full correlated QueueAccepted reply supplies the absolute scheduled target. The parent never executes QUEUE_SEQUENCE. An event may precede that reply: retain one raw staged sample, publish nothing until explicit success validates scheduled target, and treat an observed event followed by explicit rejection as contradictory evidence. A duplicate cannot overwrite the staged sample.

`apply_sequence_event` resolves incarnation, key, lifecycle, topology, token and current event type from the arm, not invented wire fields. After success, a valid sample at/after scheduled target consumes the arm once. Last-consumer cancellation removes active logical storage immediately; in-flight correlation/lease and any already-observed evidence remain separately retained until resolution. Cancellation cannot admit another host call while QUEUE_SEQUENCE may still execute. Unknown retains that lease and closes readiness. `invalidate_clock` cancels matching arms and probe decisions. `PresentTargetWake` may update completion clock with a flip pending; `IdleClockWake` does so only under the existing idle predicate. Neither affects atomic milestones.

The retained exchange carries a monotonic `publishable: bool`, initially true.
Last-consumer cancellation sets it false before removing the arm/index; neither
a reply nor a new consumer may restore it. A new arm for the same target gets a
fresh token and cannot inherit retained evidence. Both event ingress and reply
reconciliation check this latch before publishing staged evidence. A later
QueueAccepted may reconcile/dispose evidence, release the resolved exchange
lease and retain an identity-only tombstone, but emits no ClockSample,
LegacyClockSample or Present wake and advances neither clock. Cancellation does
not suppress contradiction/error handling or turn an uncertain exchange into a
resolved one. An unsent cancelled arm needs no retained host-call exchange.

### QUEUE_SEQUENCE executor extension (Task 3)

Use the existing helper/control socket. Bump `PROTOCOL_VERSION` to 3 on both sides; mixed versions fail during handshake. Preserve the 12-byte header, 56-byte correlation and existing atomic/probe layouts. Add request kind 6, correlation tag 3 and reply tags 5/6. Define these new variants/types in protocol.rs:

```rust
// HostCallCorrelation gains this variant; RequestKind gains SequenceQueue.
SequenceQueue {
    seq: RequestSeq, incarnation: IncarnationId,
    lifecycle_epoch: LifecycleEpochId, topology_generation: u64,
    hardware_crtc: u32, clock_epoch: ClockEpochId, token: SequenceArmToken,
}
pub struct SequenceQueueRequest {
    pub correlation: HostCallCorrelation,
    pub relative: bool,
    pub sequence: u64,
}
// HostCallRequest::SequenceQueue(SequenceQueueRequest)
// HostCallReply gains:
QueueAccepted { correlation: HostCallCorrelation, sequence: u64, helper_duration_ns: u64 },
QueueRejected { correlation: HostCallCorrelation, errno: i32, helper_duration_ns: u64 },
// HostCallOutcome gains (rejection uses Rejected after family validation):
QueueAccepted { sequence: u64, helper_duration_ns: u64, round_trip_ns: u64 },
```

The new request payload is 72 bytes: standard correlation, relative byte (0/1), seven zero padding bytes, sequence u64. Correlation encodes tag=3, presence=0, u16 padding=0, CRTC u32, then seq/incarnation/lifecycle/topology/clock epoch/token as six u64. Require nonzero seq/incarnation/lifecycle/topology/CRTC/epoch/token; reject other correlation families, nonzero padding and malformed lengths. Relative mode requires sequence=1. QUEUE replies retain 76-byte payload / 88-byte frame: tag at payload 0, zero padding 1..3, correlation 4..59, helper_duration_ns at 60..67, and sequence at 68..75 for success or signed errno at 68..71 plus zero padding 72..75 for rejection. All multibyte fields are little-endian. Add fixed-byte goldens with unequal duration/sequence; round trips alone do not prove order. No QUEUE request/reply carries SCM_RIGHTS. Any ancillary fd or family mismatch produces Unknown and closes received fds once. Update all request/reply/correlation/class/observation matches, transport assertions, version goldens and helper dispatch; family derives from reply variant, not echoed correlation.

Add `SequenceQueueLease` in slot.rs, privately issued and re-exported by executor, plus `HostCallReservation::SequenceQueue`. The 2-second watchdog applies. A QUEUE may coexist with an **explicitly accepted** atomic record waiting for evidence, preserving Phase A absolute wakes. It cannot coexist with an unresolved atomic host call, validation interval, probe or another queue exchange. Track `atomic_reply_resolved` in DeviceSlot, set only after atomic success; a separate sequence reservation survives atomic completion. Atomic begin/validation cannot start while a sequence exchange remains unresolved. Unknown blocks both kinds until stage 3 teardown.

Pending logical arms form a FIFO inside the bounded 256-arm table. `send_next_sequence_on` sends at most one eligible arm and returns immediately; reply callbacks and `before_block` retry eligible arms without spinning while busy. Count a queued/in-flight/shared arm as covered while its asynchronous result is pending. On explicit failure remove its coverage, latch the exact clock's queue failure and emit `SequenceArmFailed { key, consumers: BTreeSet<SequenceConsumer>, errno: Option<i32> }`. Backend consumes this before the next core arm/due pass so parked work re-evaluates the existing failure path instead of retaining fictitious coverage. No software C.0 clock is created.

Add `StubBehaviour::{AcceptQueueWith(u64), RejectQueueWith(i32)}` and scripted equivalents encoding QueueAccepted/QueueRejected. Change the low-level wrapper to `queue_crtc_sequence(fd: BorrowedFd<'_>, crtc_id: u32, relative: bool, sequence: u64, user_data: u64) -> io::Result<u64>`, consuming no ownership. helper.rs::execute_host_call already receives BorrowedFd and passes it directly; do not manufacture a Device/OwnedFd alias. Return the kernel-updated sequence; no production parent-side caller remains. Extend the event-fd stub to inject a sequence event before its reply and verify staging, rejection contradiction and cancellation while in flight.

### Record evidence and one stream

Create `owner/completion.rs` and extend `CommitRecord<R>` with one `CompletionState`. No second externally consumed event stream is introduced.

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CompletionClass { FastUpdate, LifecycleInstallRestore }

#[derive(Debug, Clone)]
pub struct CompletionContext {
    pub class: CompletionClass,
    pub host_class: HostCallClass,
    pub allow_modeset: bool,
    pub clocks: BTreeMap<u32, ClockKey>,
    pub mode_periods: BTreeMap<u32, Option<Duration>>,
    pub lifecycle_observed_max: Option<Duration>,
}

#[derive(Debug, Default)]
pub struct CompletionState {
    pub observed: BTreeSet<u32>,
    pub staged_general: BTreeMap<u32, ClockSample>,
    pub staged_present: BTreeMap<u32, ClockSample>,
    pub successful_fences: BTreeSet<u32>,
    pub accepted_at: Option<Instant>,
    pub hardware_complete_at: Option<Instant>,
    pub hardware_deadline: Option<Instant>,
    pub present_deadlines: BTreeMap<u32, Instant>,
}
```

Retain one observed set: migrate the current `observed_crtcs` field into `CompletionState.observed` and have tombstones derive their observed set from it. Store the immutable context in the record. Supply it at construction through new owner entry points `begin_with_context(desc, ledger, context)` and `begin_validated_with_context(desc, ledger, context)`; retain existing convenience `begin`/`begin_validated` only for FastUpdate non-event-bearing descriptions, deriving mode defaults. They reject event-bearing descriptions without explicit clock context. `dispatch_with_context` is begin-with-context then existing send; tests use actual stored record correlation instead of guessing counters after probes.

Before reserve, require context clock keys for every kernel-event CRTC, keys matching their CRTC, and a mode row (possibly None) for every expected completion CRTC. Reject extra or missing rows. Lifecycle class requires valid measured timing before dispatch. New errors are `DispatchError::{ClockNotReady(u32), InvalidCompletionContext, LifecycleUnvalidated}`.

For an install/restore candidate, clock rows cover the union of kernel-event and expected-completion CRTCs, including non-Present and old-active disable CRTCs. Every one must have a current successful GET reference and no queue failure. Ordinary contexts cover only kernel-event CRTCs. `host_class` accepts SeatActiveNonblock, or ColdStartOrOfflineBlocking only for LifecycleInstallRestore; validation classes are invalid in a live context. Pass it to the existing builder and use asynchronous `send_on`; the executor's existing phase check refuses blocking requests during SeatActive before IPC. No owner method waits for a blocking ioctl. A cold-start/final-offline helper may execute it under its 30s watchdog while the parent returns immediately.

Task 6 extends build.rs with `build_atomic_request_with_modeset(desc: &CommitDescription, correlation: HostCallCorrelation, class: HostCallClass, allow_modeset: bool) -> Result<(AtomicRequest, AtomicCrtcClosure), BuildError>`. Existing `build_atomic_request` delegates with false. Add `DRM_MODE_ATOMIC_ALLOW_MODESET = 0x0400` to protocol constants and set it only from this explicit argument. `begin_validation_with_options(desc, class, allow_modeset)` accepts only the two validation classes, stores class/options with the validated description, and sends through existing `send_validation_on`. A validated live install requires the same allow_modeset and corresponding boundary family in addition to persistent equality. The old begin_validation wrapper selects SeatActiveValidation/false. Thus the required real modeset and its exact TEST_ONLY are representable without fabricated fixture-only requests.

Extend `OwnerEvent<R>` with:

```rust
HardwareComplete { commit: CommitId },
Presented { commit: CommitId, samples: BTreeMap<u32, ClockSample> },
ClockSample { key: ClockKey, sample: ClockSample, origin: ClockSampleOrigin },
CompletionRetired { commit: CommitId, resources: Accepted<R> },
CompletionQualificationChanged { qualified: bool },
MechanismFailed { reason: MechanismFailure },
LegacyPageFlip { crtc_id: u32, sequence: u32, tv_sec: u32, tv_usec: u32 },
LegacyClockSample { key: ClockKey, sample: ClockSample, purpose: SequencePurpose },
```

`MechanismFailure` is a typed enum with `MalformedEvent`, `ActiveEventContradiction`, `ClockContradiction`, `FenceInvalid`, `FenceError`, `FencePollError`, `HardwareTimeout`, `PresentTimeout`, `DeadlineOverflow`, `HostCallUnknown`. Add matching variants to `UnknownCause` via `UnknownCause::Mechanism(MechanismFailure)`. The per-incarnation owner failure latch is monotonic and has no clear method. It rejects new C.0 work and closes qualification; stage 3 consumes this state to perform actual recovery. Fault with no live record still latches and emits `MechanismFailed` once.

Task 3 defines `ClockSampleOrigin::{PageFlip, Sequence(SequencePurpose)}`. Task 4 defines `report_stream_failure(&mut self, incarnation: IncarnationId, now: Instant) -> Vec<OwnerEvent<R>>`: a current source latches MalformedEvent even without a live record; an old source does nothing. Other read errors also fail closed through this ingress with logged I/O detail. A malformed tail never discards the valid prefix already delivered. Store all normalized pre-accept samples in staged_general; Present members also enter staged_present. On acceptance publish each staged general sample at most once, using origin PageFlip, without forcing an older sample over a newer sequence clock.

`apply_host_call_event_at(event, now)` is the deterministic implementation; existing `apply_host_call_event(event)` calls it with `Instant::now()`. The instant is parent observation time, never helper duration or wall clock. Current successful atomic reply adopts/validates complete slots, sets Accepted, publishes any already staged Present set, and starts hardware timing. Current rejection with **any observed kernel-event CRTC**, including a non-Present consumer, becomes contradictory CompletionUnknown; it may not call `terminalize_rejected` or release resources. Rejection with no observed evidence keeps the 2b-i behavior.

`HardwareComplete` is emitted once only after Accepted and every expected fence is successfully signalled. Empty expected set may complete an ordinary inactive transaction after acceptance, but never qualifies. `Presented` is emitted once after Accepted and every required Present CRTC has a valid staged sample; no event is emitted for an empty Present set. Complete when HardwareComplete and all required Present samples are authoritative. Clear the device slot then; do not wait for PriorBufferReleased.

At `Completed`, unregister/close remaining successful fence descriptors, extract `LedgerState::Accepted(Accepted<R>)` by value, emit `CompletionRetired` and one `Terminal { Completed }`, push tombstone, then release the slot. The recipient owns both old and new guards and decides later retirement; never emit `ResourcesReleased` or drop that ledger as a side effect of completion. In production the returned `Accepted<NeverResource>` is empty; 2c installs the bounded real retirement consumer. Unknown keeps its record and ledger and never reaches this transfer path.

### Page-event algorithm

`DeviceCommitOwner::apply_drm_event(incarnation, record, now)` performs these ordered checks:

1. Wrong incarnation or raw token zero: telemetry only (except the explicit legacy permit). Look up raw token in typed owner records; never decode kind from token bits or fabricate CRTC for CrtcSequence.
2. Resolve against current commit, current sequence arms, then tombstones. Unknown/zero/tombstoned/cancelled tokens do not touch C.0 state. Duplicate observed CRTC is warning/telemetry only and cannot move a clock.
3. Current atomic token with Vblank/CrtcSequence, or active sequence token with PageFlip/Vblank, is a mechanism contradiction. A current atomic PageFlip with CRTC zero or outside KernelEventCrtcs also poisons immediately.
4. Verify the immutable clock context still matches the active clock identity. Validate UST and extend raw MSC. Stage the exact CRTC sample and observed membership before acceptance; do not publish Presented, general-clock changes or completion until acceptance resolves.
5. After acceptance, publish valid non-regressing general samples. Present consumers contribute to the full Presented set. Non-consumer page events are accounted but missing ones never create a deadline or block completion.
6. A successful correlated sequence event updates only ClockSample/PresentDueWake behavior. It can advance the trusted extension reference but cannot satisfy a missing page event.

Pre-accept samples are normalized when observed and retained as such. A subsequent independent sequence advance cannot invalidate a legitimate earlier staged sample; publish it as that commit's Present timestamp while leaving the newer general clock untouched. An invalid active Present sample becomes CompletionUnknown, not a synthetic clock.

### Canonical fences and portable query

Task 5 first replaces the baseline platform ioctl alias, as required by spec §10. `platform::ioctl::iowr` returns raw u32 request bits, preserving the existing per-target size mask. Remove that module's IoctlReq alias and all its imports/consumers in helper.rs and drm/page_flip.rs. One reviewed wrapper performs the final call-site-inferred request cast:

```rust
/// Safety: request must describe T's initialized allocation, including every
/// pointed-to allocation the kernel may access for the duration of this ioctl.
pub(crate) unsafe fn ioctl_readwrite<T>(
    fd: BorrowedFd<'_>, request: u32, arg: *mut T,
) -> io::Result<()> {
    let rc = unsafe { libc::ioctl(fd.as_raw_fd(), request as _, arg) };
    if rc < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}
```

Import std io/AsRawFd/BorrowedFd in that module. The cast's target is inferred from the actual target libc function signature (musl c_int, glibc/FreeBSD unsigned long); no public or private alias re-encodes the old split. Atomic/GET/QUEUE request constants become u32, and their callers use this unsafe wrapper with their existing lifetime safety proofs. Update numeric ABI assertions without target-width casts. Unrelated console/syncobj wrappers are not consumers of this module and are not gratuitously rewritten. Run all three compilation gates with these actual wrapper calls. Create platform/sync_file.rs using iowr/ioctl_readwrite. The Linux UAPI checked locally is include/uapi/linux/sync_file.h and drivers/dma-buf/sync_file.c. With num_fences=0, FILE_INFO returns aggregate dma_fence_get_status: zero pending, positive success, negative error; no array or second ioctl is needed.

```rust
#[repr(C)]
#[derive(Default)]
struct SyncFileInfo {
    name: [u8; 32], status: i32, flags: u32,
    num_fences: u32, pad: u32, sync_fence_info: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FenceStatus { Pending, Success, Error(i32) }

pub fn query_status(fd: BorrowedFd<'_>) -> io::Result<FenceStatus> {
    #[cfg(target_os = "linux")]
    {
        let mut info = SyncFileInfo::default();
        let request = iowr(b'>', 4, std::mem::size_of::<SyncFileInfo>());
        unsafe { ioctl_readwrite(fd, request, std::ptr::addr_of_mut!(info))?; }
        Ok(match info.status {
            0 => FenceStatus::Pending,
            n if n > 0 => FenceStatus::Success,
            n => FenceStatus::Error(n),
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = fd;
        Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP))
    }
}
```

Import std io/fd traits and gate Linux-only struct/imports to avoid unused warnings on FreeBSD. Verify 56-byte size and offsets status=32, num_fences=40, pointer=48; Linux request is `0xc0383e04`. Do not cast every target to `c_ulong`. FreeBSD stays fail-closed until its runtime canonical ABI is independently supported; this stage promises portable compilation, not a fabricated successful status.

Create `owner/fences.rs` with `FenceQuery::status(BorrowedFd) -> io::Result<FenceStatus>`; `CanonicalFenceQuery` delegates to the platform wrapper. Tests inject the query result **at the syscall boundary**, while real owner transitions, fd ownership and readiness registration execute unchanged. A readable pipe is never called a real sync-file in a runtime integration test.

The concrete traits are `FenceQuery::status(&mut self, fd: BorrowedFd<'_>) -> io::Result<FenceStatus>` and `FencePollSet::{register(&mut self, fd: BorrowedFd<'_>, token: u64), unregister(&mut self, fd: BorrowedFd<'_>)}` returning `io::Result<()>`. `observe_fences(&mut self, query: &mut impl FenceQuery, poll_set: &mut impl FencePollSet, now: Instant) -> Vec<OwnerEvent<R>>` owns status/registration transitions. Host-outcome ingress adopts descriptors without querying; every production reply-service batch calls observe_fences immediately afterward, before timers or returning to poll. Newly adopted descriptors are queried regardless of readiness. Tests that call ingress alone assert acceptance only; tests claiming hardware evidence must call observation with explicit query/poll adapters. No implicit second production status path is permitted.

Each slot retains `{ crtc_id, fd: Option<OwnedFd>, registered: bool, succeeded: bool }` in slot order. Immediately query new descriptors regardless of readiness. Only Pending enters the poll set. POLLERR/HUP/NVAL fails; POLLIN merely triggers status query. On Success, unregister **iff registered**, then close and mark success. Immediate success closes without any unregister call; immediate query failure likewise cannot delete a nonexistent registration. On later fault unregister only registered slots and retain their owned fds in quarantine. No successful slot rescues an unknown record. Registration failure fails closed without marking registered. Unregister failure requests backend shutdown while retaining the descriptor and its registration state; no blind retry/spin. Task 5's exact-close tests distinguish immediate Success from Pending → registered → Success.

### Deadlines

Create `owner/deadlines.rs`. Functions return `Result<Duration, DeadlineError>` with `DeadlineError::{Overflow, LifecycleUnvalidated}`. `checked_deadline(now, duration)` uses `Instant::checked_add`. Never silently saturate an unrepresentable deadline.

Duration validation occurs before reservation/dispatch. Missing, overflowing or
above-28s lifecycle measurements return LifecycleUnvalidated and leave the
cohort unqualified without poisoning; other unrepresentable duration inputs
return InvalidCompletionContext before admission. Store validated durations in
the record. The observation-time helper has the exact contract
`checked_deadline(now: Instant, duration: Duration) -> Result<Instant, DeadlineError>`.
If its checked addition fails after Accepted or when constructing a missing
Present timer at HardwareComplete, immediately enter
`UnknownCause::Mechanism(MechanismFailure::DeadlineOverflow)`. Latch incarnation
failure, close qualification and emit the existing exact-once unknown/failure
events; retain the slot and ledger in quarantine. Do not call try_complete,
transfer CompletionRetired, omit the timer and continue, panic, or saturate.
Construct the missing-CRTC deadline map transactionally before installing it;
failure of one entry fails the record, not just that CRTC. Already established
typed milestones are not fabricated or revoked, but cannot rescue the unknown
record. Use the same fault cleanup for registered fence descriptors as other
mechanism failures. Commits needing no missing-Present timers perform no such
addition.

```rust
pub const UNKNOWN_MODE_PERIOD: Duration = Duration::from_micros(16_667);

pub fn fast_hardware(periods: impl IntoIterator<Item = Option<Duration>>)
    -> Result<Duration, DeadlineError>
{
    let period = periods.into_iter().map(|p| p.unwrap_or(UNKNOWN_MODE_PERIOD))
        .max().unwrap_or(UNKNOWN_MODE_PERIOD);
    Ok(period.checked_mul(3).ok_or(DeadlineError::Overflow)?
        .clamp(Duration::from_millis(100), Duration::from_secs(2)))
}

pub fn primary_event(period: Option<Duration>) -> Result<Duration, DeadlineError> {
    Ok(period.unwrap_or(UNKNOWN_MODE_PERIOD).checked_mul(2)
        .ok_or(DeadlineError::Overflow)?
        .clamp(Duration::from_millis(50), Duration::from_millis(500)))
}

pub fn lifecycle_hardware(observed: Option<Duration>) -> Result<Duration, DeadlineError> {
    let observed = observed.ok_or(DeadlineError::LifecycleUnvalidated)?;
    if observed > Duration::from_secs(28) { return Err(DeadlineError::LifecycleUnvalidated); }
    Ok(observed.checked_add(Duration::from_secs(2)).ok_or(DeadlineError::Overflow)?
        .clamp(Duration::from_secs(10), Duration::from_secs(30)))
}
```

The four timers remain separate: producer waits stay in existing source owners before admission; executor owns its 2s/30s watchdog; hardware starts at acceptance; missing Present-CRTC deadlines start at observed HardwareComplete. No universal producer timer, no Present timer at submission, no timer for non-consumer events. Platform drains queued events and observes fences before invoking tick_completion(now), which only decides expiry at now >= deadline. Valid evidence already queued for that service turn wins. Each missing CRTC has its own period/deadline. completion_deadline returns the minimum live evidence deadline, including while seat/output service is disabled.

### Exact qualification and failure latch

**Scope of this fact:** CompletionQualification is only the exact install/restore
**completion-mechanism qualification** fact. Its Qualified variant and
CompletionQualificationChanged event do NOT mean
atomic_kms_incarnation_qualified, atomic_kms_pipeline_structurally_capable or
atomic_kms_pipeline_ready. Stage 2c must combine this exact commit/generation
evidence with required primary/cursor properties and all relevant latches before
publishing incarnation qualification, and with simultaneous cursor coverage,
coordinate transport, homogeneous domain membership, policy and per-submit gates
before readiness. Those structural producers are not fabricated here. No 2b-ii
accessor/event writes any of those three complete bits. All uses of “qualifies”
below and in its tests mean this strictly narrower completion fact. Tests explicitly
assert that it cannot independently open a complete readiness bit.

Create `owner/qualification.rs`:

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CompletionQualification {
    Unqualified { topology_generation: u64 },
    Awaiting { topology_generation: u64, commit: CommitId },
    Qualified { topology_generation: u64, commit: CommitId },
}
```

`begin_install_restore(desc, ledger, context)` is the explicit first-real-install API, returning the same `(CommitId, Vec<OwnerEvent<R>>)` as begin. It requires LifecycleInstallRestore context, no qualified/current candidate, valid structural caps and current successful clocks for the full expected set before recording Awaiting. Empty off-to-off install may run but leaves Unqualified; it cannot be made a candidate. Ordinary `begin_with_context` never arms qualification. `begin_validated_install_restore` consumes passed validation with persistent/options equality and records the exact new live commit. No public `mark_qualified(bool)` or `qualify_next_commit` setter.

Only that Awaiting commit's Completed transition, with its complete successful fence set and current topology identity, changes Qualified. Rejection closes the candidate without poisoning solely for an explicit topology rejection; staged-event contradiction poisons. Epoch/topology invalidation closes qualification. The owner latches structural caps from actual per-incarnation discovery (`DRM_CAP_CRTC_IN_VBLANK_EVENT`, `DRM_CAP_TIMESTAMP_MONOTONIC`, and OUT_FENCE_PTR coverage); default construction has none and cannot qualify. Tests install explicit cap evidence through a documented fixture; production discovers it through the existing DRM device capability query interface. No advertised capability bit is rewritten by this gate.

Every proven-pre-IPC terminal path, including send_on refusal or cancellation
before dispatch, resets the matching Awaiting candidate to Unqualified in the
same terminalization operation, before retiring its record/releasing the slot.
Match both commit and topology generation; stale retirement cannot clear a
new candidate. This reset alone neither poisons nor retries, clears an existing
failure latch, nor opens readiness. A later explicit begin_install_restore must
pass all admission gates again. Reaped, Stalled, AlreadyInFlight,
ReservationMismatch and BoundaryViolation are proven send refusals in the
baseline. SendError::Ipc means a write was attempted: retain dispatched
reconciliation and the unknown/poison rules, never apply this reset as proof of
non-submission. Construction failure before candidate installation leaves
qualification unchanged.

Task 6 produces CompletionCaps with private fields `{ incarnation: IncarnationId, topology_generation: u64, atomic_enabled: bool, crtc_in_event: bool, monotonic: bool, out_fence_crtcs: BTreeSet<u32> }`. Baseline Device::enable_atomic_capabilities warns and returns Ok even on failure; successful Device construction is NOT evidence. Add a private Device atomic_client_cap_enabled: bool field and `pub(crate) fn atomic_client_cap_enabled(&self) -> bool`. In Device::open, record the actual Atomic SET_CLIENT_CAP result; preserve the baseline warning/continue behavior for legacy callers, but leave this field false on failure. UniversalPlanes success cannot set it. Test/render-node/inherited wrappers initialize false; inherited helpers need no discovery claim. Update every Device literal/constructor. Platform must use this accessor, not retry SET_CLIENT_CAP or infer capability from driver name.

The concrete issuer is `pub(crate) fn discover_completion_caps(&self, key: DrmDeviceKey) -> io::Result<CompletionCaps>` on PlatformBackend: resolve key to its KmsDevice or return NotFound, get incarnation/topology from that paired owner, read the stored atomic result, query Device::get_driver_capability(CRTCInVBlankEvent/MonotonicTimestamp) requiring exactly 1, and discover nonzero OUT_FENCE_PTR on every usable CRTC. Query failure yields no positive evidence. Construct the platform device list first, then collect cap results under immutable borrowing and install them in a separate mutable-owner pass. `pub(crate) fn install_completion_caps(&mut self, caps: CompletionCaps) -> Result<(), DispatchError<R>>` requires matching identities and at most one installation per generation. The CompletionCaps constructor is `pub(in crate::kms)` in qualification.rs, used only by this discovery path and named fixtures; fields have read-only accessors, no public setters. Task 7 wires this pass at construction; a new topology needs new discovery. Hidden fixtures have exact signatures `completion_caps_for_tests(incarnation: IncarnationId, generation: u64, atomic: bool, crtc_cap: bool, monotonic_cap: bool, crtcs: BTreeSet<u32>) -> CompletionCaps` and `install_test_completion_caps<R>(owner: &mut DeviceCommitOwner<R>, caps: CompletionCaps) -> Result<(), DispatchError<R>>`. GET readiness remains independently checked from clock rows at dispatch/completion.

### Interim production boundary

No ordinary Phase A+B caller obtains C.0 readiness here. Task 1 produces `LegacyDrainPermit` in owner/clock.rs with private incarnation/lifecycle fields and no public constructor/Clone. Default `DeviceCommitOwner::new` is owner-only; crate-private `new_legacy(incarnation, lifecycle_epoch, topology_generation)` issues and stores the permit during platform construction, before any owner work. All C.0 begin/validation/probe entry points reject while that permit exists. Legacy sequence enqueue is allowed only through Task 3's bounded asynchronous owner path. No API can return an owner-only instance to legacy mode.

Task 1 also defines crate-private `LegacyDrained` proof and `finish_legacy_transport(&mut self, proof: LegacyDrained) -> Result<(), DispatchError<R>>`. Task 7's platform adapter can issue this proof only after legacy producer admission is stopped, scene/direct pending flips are empty, all sequence exchanges resolve, logical arms are cancelled, and the owner-exclusive fd drain reaches EAGAIN without parse error. Proof carries exact incarnation/lifecycle and is consumed once to remove the permit; old clocks/references/arms are invalidated. No C.0 record/probe can exist during this handover. Production does not call it in 2b-ii: stopping/converting legacy producers is 2c. Hidden test fixtures construct the stopped/drained platform state and exercise the actual checks, including refusal on each outstanding source. A bare caller bool cannot stand in for these checks.

Concrete proof boundary: define LegacyDrained in render/platform.rs in Task 1,
with private fields, no Clone, no constructor outside that module, and a
crate-private `matches(incarnation: IncarnationId, lifecycle: LifecycleEpochId)
-> bool` accessor. Owner consumes it by value; it cannot mint one. In Task 7 add
`pub(crate) fn try_finish_legacy_transport(&mut self, key: DrmDeviceKey,
now: Instant) -> io::Result<()>`
on KmsBackend, where scene/direct pending state is actually visible. It checks
the stopped-admission state and pending flips itself, then calls private platform
`issue_legacy_drained(&mut self, key: DrmDeviceKey, now: Instant)
-> (Vec<OwnerEvent<NeverResource>>, io::Result<LegacyDrained>)` after owner
probe/validation/queue checks and cancellation/drain. The platform issuer has
`pub(super)` visibility for the backend only; the backend does not pass booleans
asserting completion. Stopped admission is a backend state initialized false and
set only by the future 2c producer-stop transition; 2b-ii exposes only a hidden
fixture setter, never a production handover call. The issuer independently
checks owner idleness, and the wrapper never calls it with legacy flips pending.
The tuple always transfers all normalized events already produced, even when
the proof result is Err (including malformed tail, EOF or an I/O failure).
The wrapper must consume that batch before inspecting/propagating the proof
result; no early `?` may drop a valid prefix. A pre-drain refusal returns an
empty batch. The backend owns the batch until every event has a final disposition.

Replace fallible batch application with private backend contracts:

```rust
enum LegacyEventDisposition {
    Applied,
    Cancelled(LegacyEventCancellation),
}
enum LegacyEventCancellation {
    RecipientGone,
    StaleOrAlreadyTerminal,
    BackendFailure,
}
// Infallible disposition, not a claim that every internal operation succeeds.
fn dispose_legacy_drain_event(
    &mut self, key: DrmDeviceKey, event: OwnerEvent<NeverResource>,
) -> LegacyEventDisposition;
```

Process the batch in stream order, consuming each event exactly once. Reuse the
normal legacy clock/page-flip bookkeeping and notification rules from Task 7;
do not introduce different MSC/UST, Present or buffer-release semantics.
Applied means the event's normal effects have been accounted. Cancelled means
an explicit final disposition under existing recipient-liveness, stale-event
or backend-failure handling, not silent dropping or rollback of effects.
Validate identity/liveness before effects. A missing notification recipient
does not skip required internal completion bookkeeping. Cancellation never
fabricates hardware success, buffer idleness, a clock sample or permission to
release an unproven resource. No event is returned for retry after partial
effects; internal failure must finish its failure bookkeeping once and retain
unproven resource ownership under the existing failure path.

Add a private backend `legacy_handover_failed` latch per device incarnation,
initially false. An unexpected event/state or internal application failure
sets it, requests the existing backend shutdown path and returns BackendFailure
after local failure bookkeeping. Continue disposing the rest of the already
drained batch (normal proven effects or explicit failure dispositions); never
return early, silently drop the suffix or replay the prefix. This latch has no
same-incarnation reset. Subsequent handover calls refuse before draining or
issuing proof. RecipientGone/StaleOrAlreadyTerminal alone are not such failures.
This is a terminal fallback, not a new resource recovery or retry subsystem.

Only after all dispositions, a successful proof result and an unset failure
latch may the wrapper consume the proof and remove the legacy permit. Otherwise
drop any proof, retain legacy mode with admission stopped, and return the drain
error or InvalidData for terminal application failure. The wrapper still returns
`io::Result<()>`, never a batch requiring deferred delivery. No event-loop yield
or reopening of producer admission occurs between drain, disposition and proof
consumption. The prohibition on new KMS/QUEUE submissions is specific to this
stopped handover phase: separate normal completion bookkeeping from scheduling
new work, leaving normal event-loop rearming and Xorg-compatible Present behavior
unchanged. Stopped/cancelled consumers cannot be resurrected by a callback.
Stage 2c must complete handover before admitting C.0 producers; no production
handover is enabled here.

Task 1 adds DispatchError::{LegacyTransportActive, InvalidLegacyDrainProof};
begin/validation/probe reject LegacyTransportActive before allocating identity or
consuming a ledger. finish_legacy_transport checks proof identity and permit,
returning InvalidLegacyDrainProof for wrong/repeated proof. To distinguish real
empty drain from EOF, Task 7 changes raw drain result to `io::Result<DrainStop>`
with `DrainStop::{WouldBlock, EndOfFile}`; only WouldBlock permits a proof.
Update drain_device_events and its callers/tests; normal EOF fails the current
stream rather than masquerading as a successful handover. This adds no second
reader. Handover removes legacy-only rows before first C.0 probe; it cannot reset
any already-attempted C.0 probe or reuse a prior C.0 epoch.

Under this permit only, zero `user_data` page flips take the old backend retirement path (including its existing legacy software clock behavior). Nonzero unknown tokens never get that fallback. C.0 tokens always go through exact owner correlation. Legacy samples never seed the owner clock or qualify anything. Moving a legacy-only device's sequence support into clock records may retain a legacy enqueue-failure bit there, but it must not select KernelSequence without the GET probe. The old backend helper that assumes Unknown means Supported must not open C.0 admission.

Legacy sequence success emits only LegacyClockSample; it does not set source/reference/latest in CrtcClock. This distinct event preserves baseline general/completion-clock projection without leaking trusted evidence into C.0. The same epoch-local queue_failed bit may suppress repeated legacy enqueue attempts, and handover resets it with the clock identity. Missing permission always means telemetry-only for zero-token flips, never implicit legacy selection.

Production probe/owner submission is not started automatically alongside unconverted legacy KMS mutations. 2c performs the one-way transport handover after pending legacy flips and old event bytes are drained. This stage implements and tests that boundary but does not claim it has converted the six atomic callers.

## File structure and task order

New: `kms/owner/{clock,sequence,completion,fences,deadlines,qualification}.rs`, `platform/sync_file.rs`, `tests/owner_completion_evidence.rs`.

All `kms/`, `drm/`, and `platform/` paths in this plan are relative to `crates/yserver/src/`; `tests/` means `crates/yserver/tests/`. Core paths are relative to `crates/`.

Modified: `kms/owner/{device,record,ledger,slot,identity,build,mod,test_fixtures}.rs`; `kms/executor/{mod,protocol,helper,test_support}.rs`; `drm/{event_stream,page_flip,device}.rs`; `platform/mod.rs`; `kms/render/{platform,backend,completion_poller}.rs`; `yserver-core/src/backend/{trait_def,recording}.rs`; `yserver-core/src/core_loop/{run,process_request,process_disconnect}.rs`; `yserver-core/src/server.rs`; `docs/status.md`. Preserve unrelated changes and reconcile every affected struct literal using `rg` plus the compiler.

### Cross-task signatures

These declarations are the API contract, not additional production facades. Types come from the normative sections and existing owner modules. Owner methods below belong to `impl<R> DeviceCommitOwner<R>`; preserve existing begin/send methods as the documented wrappers.

```rust
// Task 1. Invalid identity returns InvalidCompletionContext; no row on read.
pub fn install_clock(&mut self, key: ClockKey, lifecycle: LifecycleEpochId,
    generation: u64) -> Result<(), DispatchError<R>>;
pub fn clock(&self, key: ClockKey) -> Option<&CrtcClock>;
pub fn invalidate_clock(&mut self, key: ClockKey) -> Vec<OwnerEvent<R>>;
// Task 3. SequenceError additionally has Busy and TransportUnavailable.
pub fn reserve_arm(&mut self, key: ClockKey, purpose: SequencePurpose,
    target: u64, consumers: &[SequenceConsumer]) -> Result<SequenceArmToken, SequenceError>;
pub fn send_next_sequence_on(&mut self, executor: &mut KmsIoExecutor)
    -> Result<Vec<OwnerEvent<R>>, DispatchError<R>>;
pub fn cancel_consumer(&mut self, consumer: SequenceConsumer);
pub fn apply_sequence_event(&mut self, incarnation: IncarnationId,
    token: u64, time_ns: i64, sequence: u64, now: Instant) -> Vec<OwnerEvent<R>>;
// Task 4. All four begin entry points (ordinary/validated/install/validated-install)
// have this same input/output shape; install variants are produced in Task 6.
pub fn begin_with_context(&mut self, desc: &CommitDescription, ledger: Submitted<R>,
    context: CompletionContext) -> Result<(CommitId, Vec<OwnerEvent<R>>), DispatchError<R>>;
pub fn begin_validated_with_context(&mut self, desc: &CommitDescription, ledger: Submitted<R>,
    context: CompletionContext) -> Result<(CommitId, Vec<OwnerEvent<R>>), DispatchError<R>>;
pub fn apply_drm_event(&mut self, incarnation: IncarnationId,
    event: DrmEventRecord, now: Instant) -> Vec<OwnerEvent<R>>;
pub fn apply_host_call_event_at(&mut self, event: HostCallEvent,
    now: Instant) -> Vec<OwnerEvent<R>>;
// Task 6. This method does NOT drain fds itself: platform orders evidence before it.
pub fn completion_deadline(&self) -> Option<Instant>;
pub fn tick_completion(&mut self, now: Instant) -> Vec<OwnerEvent<R>>;
pub fn begin_validation_with_options(&mut self, desc: &CommitDescription,
    class: HostCallClass, allow_modeset: bool) -> Result<CommitId, DispatchError<R>>;
```

Task 3's `arm_queued`/`arm_rejected` are private transition helpers selected only after full host correlation/family validation, not independently callable public ingress. An arm keeps phase PendingDispatch/InFlight/Armed plus optional staged raw event. `apply_drm_event` delegates sequence records to apply_sequence_event; no caller supplies a free-form identity argument. install_clock must match the owner's current lifecycle/topology and reject zero/reused/regressing epochs; actual topology/lifecycle transition issuance remains stage 3. Invalidating a clock revokes qualification and cancels arms; a dispatched dependent live commit becomes unknown, never silently adopts the new key.

The core seam lives in trait_def.rs and is independent of KMS types:

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct PresentSequenceTarget { pub consumer: u64, pub target: u64 }
// New Backend methods; default implementations extract target values and delegate
// to the existing corresponding arm_* method, preserving nested backends.
fn arm_idle_vblanks_for_consumers(&mut self, crtc: u32, epoch: u64,
    targets: &[PresentSequenceTarget]) -> std::io::Result<usize>;
fn arm_present_absolute_vblank_for_consumers(&mut self, crtc: u32, epoch: u64,
    targets: &[PresentSequenceTarget]) -> std::io::Result<usize>;
fn arm_present_completion_idle_vblanks_for_consumers(&mut self, crtc: u32, epoch: u64,
    targets: &[PresentSequenceTarget]) -> std::io::Result<usize>;
fn cancel_present_sequence_consumer(&mut self, id: u64) {}
```

KMS overrides all three consumer-bearing methods and verifies exact epoch before resolution to (device, hardware CRTC, ClockKey). Return the number of **input rows covered**, including duplicates sharing an arm, pending sends and prior arms—not newly sent ioctl count. Admission into the logical table is all-or-nothing per call: capacity/error cannot leave a partially attached input batch. Preserve existing core target computation, including eff-1 for pending execution and eff for completion. The absolute/relative purposes retain separate dedup keys. Cancellation is idempotent and removes the id from all device tables. Old KMS trait methods with target-only inputs are compatibility wrappers for tests/other direct callers only; they cannot mint guessed persistent consumer identities in production.

### Requirement/test traceability

The bracket groups in each task heading apply to every named test and case in that task. Copy those groups into each implemented test's doc comment (including parameterized cases) and record the actual test names in the task fold-back. Task 8 integration tests cite their row's original task groups as well as its own. Hardware qualification is explicitly not claimed by these deterministic tests.

## Task 1: Epoch-local clock record and the SequenceSupport migration [ID-1..3, CAP-1..4, COMMIT-2]

**EXECUTED 2026-09-07:** code `60738727`, review fix `ccae6e2e`.
Task-scoped spec/quality review found missing/stale queue evidence could create
a clock row; the fix updates existing rows only and the scoped re-review is
clean. Full `cargo test -p yserver`: 1391 unit passed, 64 ignored, integration
and doctests passed (`/tmp/yserver-task1-round1-root-tests.log`). Exact clippy,
nightly format and Linux glibc/musl/FreeBSD cargo checks passed. No runtime
hardware qualification claimed.

Compiled owner clock APIs match this task; support seams are `clock_mut`,
`clock_context`, `new_legacy` and consuming `finish_legacy_transport`.
Tests include arithmetic/wrap, reference-without-sample, nonregression,
two-CRTC isolation, newer-epoch non-reuse, legacy refusal/proof consumption,
`queue_failure_for_a_missing_clock_row_is_telemetry_only` and
`queue_failure_for_a_stale_epoch_does_not_touch_the_replacement`.
Exact test names and RED/GREEN evidence are retained in the local task report;
the commit contains their requirement-tagged definitions. Probe/arm hooks and
the checked production proof issuer remain Tasks 2/3/7.

**Files:** create `owner/clock.rs`; modify owner module/identity/device and backend/platform clock accessors and fixtures.

**Interfaces:** consumes `ClockEpochId`, `LifecycleEpochId`, current hardware `CrtcKey`; produces ClockKey/CrtcClock/ClockSource/ProbeState/ClockSample/ClockError, `extend_sequence`, `page_ust`, clock methods in the signature contract and LegacyDrainPermit/LegacyDrained/new_legacy/finish_legacy_transport. Construction refuses coexistence before Task 2 adds probing. Task 7 produces the platform proof issuer. install_clock is idempotent only for identical identity; clock does not manufacture a row.

- [x] **Step 1: Add clock arithmetic and isolation tests.** Put the following in the new module's unit tests, importing `super::*`.

```rust
#[test]
fn raw_zero_is_a_real_wrap_not_a_source_switch() {
    assert_eq!(extend_sequence(0xffff_ffff, 0), Ok(0x1_0000_0000));
    assert_eq!(extend_sequence(0x1_0000_0001, 0xffff_ffff), Ok(0xffff_ffff));
    assert_eq!(extend_sequence(0, 0x8000_0000), Err(ClockError::HalfRange));
    assert_eq!(extend_sequence(0, 0xffff_ffff), Err(ClockError::NoRepresentative));
    assert_eq!(page_ust(11, 22), Ok(11_000_022));
    assert_eq!(page_ust(11, 1_000_000), Err(ClockError::BadMicroseconds));
    assert_eq!(page_ust(u32::MAX, 999_999), Ok(4_294_967_295_999_999));
    let mut reference = 0xffff_fffe;
    for (raw, expected) in [(0xffff_ffff, 0xffff_ffff), (0, 0x1_0000_0000), (1, 0x1_0000_0001)] {
        reference = extend_sequence(reference, raw).unwrap();
        assert_eq!(reference, expected);
    }
}
```

Add tests with two CRTCs on the same device/epoch where a queue failure on CRTC 1 leaves CRTC 2 untouched, and with the same raw handle in a new epoch where source/reference/sample reset to Unresolved/None/None. Observe `(msc=10, ust=100)` then `(9,90)` and assert latest stays `(10,100)`. A GET reference alone must leave latest absent.

- [x] **Step 2: Run `cargo test -p yserver --lib kms::owner::clock`; observe failure for the absent implementation.**
- [x] **Step 3: Implement the clock contract above and migrate the map in the same task.** Replace every backend `sequence_support(device, epoch)` access with exact `CrtcKey` resolution and that device owner's clock row. Remove `SequenceSupport`, its backend field and initializer entries, and its device-wide unsupported helper. Legacy queue errors update only the exact clock row's queue-failure evidence. Remove the O(epoch) loop in `clock_epoch_for_raw`; use `ClockEpochId::from_raw` with explicit rejection of raw zero for a C.0 context.
- [x] **Step 4: Run clock tests and existing backend sequence-support/epoch tests.** Update their setup to name hardware CRTC explicitly; preserve tests proving a different device is unaffected and add same-device/different-CRTC coverage.
- [x] **Step 5: Run the global gate and commit `feat(kms): store sequence capability in epoch-local CRTC clocks`.**

## Task 2: Asynchronous probe and production reservation [ID-3, COMMIT-5, CAP-1..4]

**Files:** modify owner slot/device/clock/test_fixtures and executor mod/test_support; add probe integration cases to `tests/owner_completion_evidence.rs`.

**Interfaces:** consumes Task 1 clocks and existing protocol ClockProbeRequest/ClockProbe correlation; produces private probe lease issuer, `begin_clock_probe`, `send_clock_probe_on`, ProbeOutcome and ClockProbeResolved event defined above. No wire change.

- [x] **Step 1: Add failing probe tests.** Real stub AcceptProbeWith must leave source Unresolved after send and set KernelSequence only after its polled reply. Add `StubBehaviour::RejectProbeWith(i32)` (including CLI encode/decode and child match) that emits existing `HostCallReply::ProbeRejected`, not atomic Rejected; EOPNOTSUPP must leave Failed/Unresolved and refuse same-epoch retry. Keep RejectWith as a deliberate wrong-family test yielding Unknown(MalformedReply). Use NeverReply plus executor.next_deadline()+1ms for watchdog; assert retained probe exclusion. Vary each full correlation field and LateReply; none resolves the current probe. Test mutual exclusion with validation/atomic and refusal in legacy mode.

```rust
#[test]
fn a_probe_round_trip_selects_a_reference_without_a_timestamp() {
    use std::time::Duration;
    use yserver::kms::executor::test_support::{self, StubBehaviour};
    use yserver::kms::owner::test_fixtures::owner_for_tests;
    use yserver::kms::owner::clock::{ClockKey, ClockSource};
    use yserver::kms::owner::identity::ClockEpochId;
    use yserver::kms::owner::lifecycle::LifecycleEpochId;
    let key = ClockKey { hardware_crtc: 1, epoch: ClockEpochId::first() };
    let mut owner = owner_for_tests();
    owner.install_clock(key, LifecycleEpochId::first(), 1).unwrap();
    let mut executor = test_support::spawn_stub_helper(
        StubBehaviour::AcceptProbeWith(0x1_0000_0000)).unwrap();
    owner.begin_clock_probe(key).unwrap();
    owner.send_clock_probe_on(&mut executor).unwrap();
    assert_eq!(owner.clock(key).unwrap().source, ClockSource::Unresolved);
    test_support::wait_readable(executor.control_fd().unwrap(), Duration::from_secs(5));
    owner.apply_host_call_event(executor.poll_reply().unwrap());
    assert_eq!(owner.clock(key).unwrap().reference, Some(0x1_0000_0000));
    assert_eq!(owner.clock(key).unwrap().latest, None);
}
```

- [x] **Step 2: Run `cargo test -p yserver --test owner_completion_evidence`; verify missing owner probe API.**
- [x] **Step 3: Implement the normative probe state machine.** Move ClockProbeLease definition and preserve imports/re-exports used by existing executor tests. Extend slot error precedence tests in both directions. DispatchError gains the probe/context variants named by this contract; refusal uses the existing `Refused` shape. Probe resolution precedes atomic matching, but all late results are stale first. Do not match probe id alone or seed UST from `Instant`.
- [x] **Step 4: Run owner and integration tests, including stage-2a clock-probe wire tests.**
- [x] **Step 5: Run the global gate and commit `feat(kms): probe CRTC clocks through the asynchronous owner`.**

## Task 3: Asynchronous bounded sequence arms and consumer cancellation [ID-1..3, COMMIT-5, CAP-1..4, MULTI]

**Files:** create owner sequence module; modify owner device/slot/identity, executor protocol/mod/helper/test_support and drm/page_flip; convert backend sequence producers; modify core trait/recording/server/run/process_request/process_disconnect for consumer lifetime notifications.

**Interfaces:** consumes Task 1 clocks/legacy permit and Task 2's asynchronous ingress; produces the protocol-v3 QUEUE family/lease, SequenceArm/SequenceArms/SequencePurpose/SequenceConsumer, public methods in the signature contract, private reply transitions, ClockSampleOrigin and ClockSample/LegacyClockSample/SequenceArmFailed events. Define `SequenceError::{ClockNotReady, Capacity, IdentityExhausted, UnknownArm, InvalidIdentity, InvalidSample, Busy, TransportUnavailable}`. reserve_arm returns SequenceError; send uses DispatchError because refusal carries owner events.

- [x] **Step 1: Add failing unit tests against the real arm table.** Two consumers for `(CRTC=1,epoch=1,target=50)` share one token. Cancel one: event still resolves for the other. Cancel both: delayed event changes no clock. Fill 256 distinct arms and assert arm 257 returns Capacity without eviction. Reuse raw CRTC 1 after epoch invalidation and prove old token cannot consume a new arm. Cross-device tokens with equal raw values cannot consume one another because the device owner is selected before token lookup. Wrong-type current arm event poisons; unknown wrong-type token does not.
- [x] **Step 2: Run `cargo test -p yserver --lib kms::owner::sequence`; verify the missing implementation.**
- [x] **Step 2a: Add the staged cancellation permutation.** Send QUEUE, stage its event, cancel the last consumer, then deliver QueueAccepted. Assert unchanged general/completion clocks, no clock or Present-wake output, and lease retention until reply resolution followed by exact-once disposal. Also cancel before event, and create a fresh same-target arm while the old exchange resolves: the new token cannot revive old evidence. Repeat the no-publication assertion for legacy output; cancellation does not hide a contradictory rejection.
- [x] **Step 3: Implement protocol/lease and table, then convert both queue producers.** Follow the QUEUE_SEQUENCE extension and signatures above. Add wire round trips, bad-family/fd rejection, mixed-version handshake, early-event staging and 2s watchdog tests before implementation. Remove backend SequenceArmTable/target maps when their callers use the owner. Keep immutable dedup target distinct from scheduled target. Require GET success for C.0 arms; legacy mode emits only LegacyClockSample. Move the sole production low-level queue call into helper.rs. Preserve absolute wakes while an accepted atomic awaits fences/events and test that an unresolved atomic host call excludes QUEUE.
- [x] **Step 4: Connect real logical consumers.** ServerState::next_present_id currently WRAPS: replace its increment with checked_add(...).expect("Present identity exhausted"), retaining the existing nonzero initial value, and add an exhaustion test. Allocate PendingNotifyMsc's new sequence_consumer from that shared namespace; Pixmap/complete use present_id. Implement all consumer-bearing trait methods above, preserving existing target transforms. Call cancellation before each removal in fire_due_present_notify_msc_for_domain, purge_present_for_destroyed_windows, supersede_covered_pending_presents, execute_parked_present_ids, drain_ready_present_pixmaps, shutdown_drain_present_pending_exec, fire_present_completions_sweep, discard_stale_present_event and process_disconnect's three pending-store cleanup paths. NotifyMSC delivery helpers currently lack backend: add `backend: &mut dyn Backend` and update all callers, including run.rs, so cancellation is synchronous in that turn. Cancelling an execution-stage arm before the same id enters completion is safe; the completion stage explicitly rearms its new target. Reconcile every PendingNotifyMsc literal and every pending-store removal with rg. RecordingBackend must record calls from actual delivery/destruction paths, not only direct owner-table tests.
- [x] **Step 5: Verify both relative and absolute wakes retain Phase A behavior.** An absolute wake advances completion clock while a page flip is pending, without retiring that flip. A relative idle wake clears only its own arm. Cancellation must occur before another raw event is serviced in the same event-loop turn. Tests drive actual core scrap/purge paths, not direct mutation of the owner consumer set alone.
- [x] **Step 5a: Verify shutdown cancellation.** Populate two execution-stage consumers sharing a target, call actual shutdown_drain_present_pending_exec with RecordingBackend, assert both ids are cancelled before the store is emptied, then deliver the old arm event and assert no clock/milestone change. Include an unresolved queue reply so logical cancellation cannot release its host-call lease.
- [x] **Step 6: Run the global gate plus relevant `cargo test -p yserver-core` tests and commit `feat(kms): correlate bounded sequence arms with their consumers`.**

## Task 4: Page-event correlation and pre-accept staging [ID-1..3, COMMIT-2, COMMIT-6, MULTI]

**Files:** create owner completion module; modify owner record/device/clock/mod/test_fixtures and drm event_stream visibility.

**Interfaces:** consumes Tasks 1–3 and existing DrmEventRecord; produces CompletionContext/CompletionState/CompletionClass, `begin_with_context`, `begin_validated_with_context`, `apply_drm_event`, `apply_host_call_event_at`, MechanismFailure and the staged Presented/clock portion of OwnerEvent. Completion finalization follows in Task 6; do not invent fence success in this task.

- [x] **Step 1: Add parser-to-owner tests with actual 32-byte wire records.** Extend public test_fixtures with `page_event_bytes(crtc, raw_sequence, sec, usec, user_data) -> [u8;32]` using the parser's documented native-endian offsets, and `event_for_current_record(owner, crtc, raw_sequence, sec, usec) -> DrmEventRecord` by parsing those bytes. The helper takes `owner.live_record().event_token()`; it never guesses the allocator counter. Keep fixtures visible to the integration crate.

Cases: valid page before ioctl success stages but emits no Presented; explicit success publishes staged samples; explicit rejection after a consumer or non-consumer event quarantines both resource sets. Two Present CRTCs require two distinct events; duplicate first event does not complete the set. Raw zero wraps from a trusted high reference. Invalid microseconds, exact half-range, wrong current CRTC, zero current CRTC and wrong current event type poison; unknown/tombstoned/old-incarnation events do not. A sequence advance after staging cannot replace the stored Present timestamp.

- [x] **Step 2: Run `cargo test -p yserver --lib kms::owner::completion`; verify missing transitions.**
- [x] **Step 3: Implement the ordered page algorithm and record changes from the contract.** Public `apply_drm_event` takes explicit source incarnation from the fd's owning device; do not accept caller-supplied CommitId. Add `record.observed` before processing later ioctl replies. Replace the unconditional rejected branch in current `apply_to_live` with the observed-evidence contradiction check. Do not let its duplicate-acceptance guard drop legitimate later DRM/fence events; that guard applies only to repeated host outcomes. Tombstones resolve tokens without changing records or clocks.
- [x] **Step 4: Update stage-2b-i tests to provide clock context only when they request page events.** Preserve every old refusal/unknown assertion. New evidence-state tests must assert resource contents and slot occupancy after contradictory rejection, not just an event enum.
- [x] **Step 5: Run the global gate and commit `feat(kms): stage and correlate owner page-event evidence`.**

## Task 5: Canonical fence query and owned descriptors [COMMIT-2, COMMIT-6, MULTI]

**Files:** create platform sync_file and owner fences modules; replace platform/ioctl boundary and its consumers/tests in executor/helper and drm/page_flip; modify record/owner, CompletionPoller adapter and fixtures.

**Interfaces:** consumes existing out-fence slot table and CompletionState; produces FenceStatus/FenceQuery/CanonicalFenceQuery, owned slot status, `observe_fences(query, poll_set, now) -> Vec<OwnerEvent<R>>`. Define `FencePollSet::{register(BorrowedFd, u64), unregister(BorrowedFd)}` returning `io::Result<()>`; implement it for CompletionPoller. The trait allows tests to fail registration without changing state-machine semantics.

- [x] **Step 1: Add failing canonical-query and ownership tests.** Real pipe and `/dev/null` descriptors must fail the real `query_status`; readiness cannot be called Success. ABI tests assert size, offsets and request code. State tests inject Pending/Success/Error at FenceQuery, retaining actual owned fds and a real CompletionPoller. Query immediately after adoption. A partial mask quarantines; a full mask with one error quarantines; two successful slots emit HardwareComplete exactly once, never Presented. An already-signalled descriptor must progress without a new readiness edge. Readable-but-Pending stays pending. Force register/unregister errors through the adapter and assert fail-closed behavior.
- [x] **Step 2: Run `cargo test -p yserver --lib sync_file` and `cargo test -p yserver --lib kms::owner::fences`; verify expected failures.**
- [x] **Step 3: Implement the canonical wrapper and descriptor lifecycle exactly as specified.** Migrate `FenceEvidence` to per-slot Option<OwnedFd> storage without duplicating descriptors. Preserve `by_crtc()` for currently retained descriptors; succeeded slots remain represented by successful_fences after their fd closes. On fault, unregister all pending slots before retaining quarantine. Poll each fd with timeout zero and handle EINTR by leaving it pending for this service turn; do not loop unboundedly. A later owner service turn queries pending state again.
- [x] **Step 4: Add exact-close tests using pipe writer EOF, not fd-number reuse.** Immediate fake Success closes the sole writer with ZERO register/unregister calls and the reader observes EOF. Pending → Success first registers, then unregisters before close; a tracking adapter asserts that order. Pending/Unknown retains the writer until owner teardown. Never dup an observation tee. A hidden public `test_fixtures::fence_poll_set_for_tests() -> io::Result<impl FencePollSet + AsRawFd>` returns the real private CompletionPoller for integration tests without leaking its private type.
- [x] **Step 5: Run the global and all three portable gates, then commit `feat(kms): require canonical sync-file status for hardware completion`.**

## Task 6: Deadlines, exact completion and install qualification [COMMIT-2, COMMIT-5, CAP-1..4, MULTI]

**Files:** create owner deadlines/qualification; modify owner build/record/ledger/device/completion, executor protocol constants, drm/device atomic-result storage, platform discovery and fixtures.

**Interfaces:** consumes Tasks 1–5; produces deadline functions, owner `completion_deadline`, `tick_completion`, CompletionQualification, install/restore entry points, full Completed retirement and failure latch. Add consuming `CommitRecord::into_completed() -> (Tombstone, Accepted<R>)`, guarded by the required milestones, and no transition out of Quarantined. Use `LedgerState::Poisoned` only as a move sentinel, never as a substitute for returning accepted resources.

- [x] **Step 1: Write exact deadline tests.**

```rust
#[test]
fn hardware_and_event_windows_are_independent() {
    assert_eq!(fast_hardware([None]), Ok(Duration::from_millis(100)));
    assert_eq!(fast_hardware([Some(Duration::from_millis(400))]), Ok(Duration::from_millis(1200)));
    assert_eq!(fast_hardware([Some(Duration::from_secs(1))]), Ok(Duration::from_secs(2)));
    assert_eq!(primary_event(None), Ok(Duration::from_millis(50)));
    assert_eq!(primary_event(Some(Duration::from_secs(1))), Ok(Duration::from_millis(500)));
    assert_eq!(lifecycle_hardware(None), Err(DeadlineError::LifecycleUnvalidated));
    assert_eq!(lifecycle_hardware(Some(Duration::from_secs(28))), Ok(Duration::from_secs(30)));
    assert_eq!(lifecycle_hardware(Some(Duration::from_secs(29))), Err(DeadlineError::LifecycleUnvalidated));
}
```

Add no-sleep owner tests at deadline minus 1ns / exactly deadline. Before acceptance no hardware timer; before HardwareComplete no Present timer. After HardwareComplete create timers only for still-missing Present CRTCs, each from its own mode. Arrival on that service turn precedes expiry. Producer waiting never reserves the slot or enters this timer table.

- [x] **Step 2: Add completion/qualification tests and observe failure.** Ordinary fast commit with all evidence reaches Completed but leaves qualification false. Exact install candidate with nonempty expected set and full evidence qualifies; empty expected set cannot. Successful ioctl with pending fences cannot qualify, including a permitted blocking qualification fixture. Failed/missing caps or missing lifecycle timing prevents qualification dispatch. Topology invalidation closes a formerly qualified gate and cancels clocks/arms. Error fence after one success yields no CompletionRetired. Completing a tracked-resource commit frees the slot but destroys neither old nor new resource; drop counts stay zero until the receiving test deliberately drops CompletionRetired resources.
- [x] **Step 3: Implement the deadline and qualification contracts.** Before each eligible dispatch compute durations and reject unvalidated lifecycle timing. At Accepted record parent now; at full successful fence set record hardware now and create missing-event deadlines. `try_complete` is invoked after acceptance, page, and fence transitions, never as a guessed polling shortcut. Drain Accepted resources by value into the one stream. Failed latch has no reset. A topology update while a dispatched record lives makes it unknown and preserves its old clock context for diagnostic correlation; it does not silently replace its generations.
- [x] **Step 3a: Cover pre-IPC candidate retirement.** Begin a valid install candidate, force a proven BoundaryViolation refusal before IPC, and assert matching Awaiting becomes Unqualified with the slot released and no new poison. Correct the executor boundary and explicitly begin a fresh valid candidate; full evidence qualifies it. Cover pre-dispatch cancellation, construction failure and stale commit/generation retirement separately. An attempted-write Ipc outcome instead follows unknown reconciliation and cannot authorize a fresh candidate on the poisoned incarnation.
- [x] **Step 3b: Cover deadline-construction failures without sleeps.** Inject a checked-add failure at the arithmetic boundary while running the real acceptance/fence owner transitions. Test hardware construction at acceptance and one failed entry in a two-CRTC missing-Present map at HardwareComplete. Both produce one DeadlineOverflow failure/unknown terminal, retain slot/ledger, close qualification and emit no CompletionRetired; later evidence cannot complete them. Test checked_deadline's real overflow result separately. Missing or above-28s lifecycle measurements must instead refuse before dispatch with no poison. Keep the injection private to tests; do not invent a production clock or timer fallback.
- [x] **Step 4: Add full ordering matrix.** Drive real parser → owner and real stub helper reply → owner with a fake syscall query only for fence status. Cover page/reply/fence permutations and two-CRTC partial sets. Assert exactly one Accepted, HardwareComplete, Presented, Completed terminal and completed-ledger transfer when each is required; no Presented for non-Present. Sequence-only events never satisfy these counts. Every failure yields one unknown terminal and retains the slot.
- [x] **Step 5: Run owner tests twelve consecutive times, the global gate, and commit `feat(kms): complete exact evidence sets and gate install qualification`.**

## Task 7: Exclusive drain, stable wakeups and compatibility boundary [INV, ID-3, COMMIT-5, MULTI]

**Files:** modify render platform/backend, owner device, core backend trait/recording/run, and relevant backend fixtures.

**Interfaces:** consumes complete owner evidence APIs; produces platform `drain_owner_events(drm_fd, now)`, `service_owner_completions(now)`, `owner_completion_deadline`, stable OwnerCompletion fd kind and callback. Both drain/service functions return `Vec<(DrmDeviceKey, OwnerEvent<NeverResource>)>`; do not log/drop the stream inside platform before backend consumers receive it.

- [x] **Step 1: Add failing integration tests before wiring.** Second device's raw event updates only its owner even when raw token/incarnation numbers collide. A fence adopted after startup wakes the same initially registered aggregate fd. Backend callback drives canonical observation; register-only tests are insufficient. With seat inactive, an expired owner deadline still runs. Partial raw buffer containing one valid event then malformed tail delivers the valid event and still latches the malformed-stream failure; do not lose it behind `?` on drain error. Legacy zero-token flip reaches the old scene/direct retire path only with LegacyDrainPermit; a C.0 token never reaches that path.
- [x] **Step 2: Run targeted backend/core-loop tests and verify missing callback/source behavior.**
- [x] **Step 2a: Assert final dispositions and handover order.** Drive the actual stopped/drained fixture with events A/B/C. First use three live recipients: assert all three are Applied in order, exactly once, and the permit is removed only after C. Then remove B's recipient: assert B receives RecipientGone with required internal bookkeeping preserved, A/C are Applied, no fabricated clock/release occurs, and proof is consumed only after C. Force an internal failure on B: assert its final BackendFailure disposition, C still receives a final disposition, the failure latch and shutdown request are set, and no proof is consumed. A second handover call must refuse without drain/proof issuance and without replaying any event. Throughout disposition, the legacy permit remains present and C.0 admission is rejected. Also inject malformed tail/EOF after a valid A/B/C prefix: the returned tuple must retain that prefix, all three receive dispositions, and the proof result remains an error. Test pending-work refusal separately. Assert zero new KMS/QUEUE sends during handover and unchanged normal-mode sequence rearming/Present behavior; a proof-identity-only test is insufficient.
- [x] **Step 3: Install the stable aggregate and correct loop ordering.** Create owner_completion_poller in every PlatformBackend constructor, even with zero devices; expose BackendFdKind::OwnerCompletion and Backend::on_owner_completion_ready(&mut self, state: &mut ServerState) default no-op. Dispatch it in run.rs. Move the single backend.before_block() call BEFORE calculating poll_timeout, not merely before poll.poll. It services host replies, queued raw events, newly adopted/canonical fences, deadlines, then eligible sequence sends. next_wakeup includes owner timers outside allow_kms_timers. No later callback may consume evidence/start a timer between next_wakeup and poll without timeout recomputation. Add a core regression test where before_block replaces a 2s hardware deadline with a 50ms event deadline and removes the last ready fd; assert the actual timeout computation uses the new deadline. Discovery/install of CompletionCaps also runs at owner construction, failing closed without changing advertised capabilities.
- [x] **Step 4: Establish nonblocking fd ownership, then move both raw-reader sites.** Baseline Device opens do not explicitly set O_NONBLOCK. At the KMS primary-device open boundary in drm/device.rs, preserve existing status flags and set/verify O_NONBLOCK with F_GETFL/F_SETFL before spawning its helper or registering/draining the fd; fail initialization on error. Do not change unrelated render-node opens. The shared open-file description retains this status across aliases; the helper never reads it. Test nonblocking empty drain and flag preservation using a pipe fixture with bounded readiness, without running a deliberately blocking read. drain_page_flip_events becomes an adapter around drain_owner_events. discard_old_drm_events_after_all_off uses the same owner reader and counts returned legacy CRTCs; keep its bounded legacy lifecycle wait until stage 3. Accumulate valid-prefix owner events even when the final drain fails, then call report_stream_failure. The helper and dormant present/event_loop.rs gain no new reader. Implement the checked legacy-drain proof issuer described in Task 1, without invoking production handover before 2c.
- [x] **Step 5: Wire outputs deliberately.** ClockSample projects the trusted clock through hardware-CRTC resolution and existing per-window offsets; add tests asserting client-visible MSC/UST across raw fffe/ffff/0/1, maximum sec/usec and legitimate epoch remap. LegacyClockSample uses only legacy projection; LegacyPageFlip alone invokes old direct/scene retirement until 2c. Evidence events never reach legacy BO/protocol terminalization. CompletionRetired carries only empty Accepted<NeverResource> in production. CompletionQualificationChanged never rewrites advertised caps. SequenceArmFailed invalidates coverage before the next core due/arm pass. MechanismFailed closes owner admission; unregister failure calls KmsBackend::request_exit (Message::Shutdown via input_sender), retains quarantine and detaches the aggregate from further servicing to avoid spin. Test shutdown delivery with a real test CoreSender; no-sender fixture logs are not proof of shutdown. Stage 3 later supplies recovery.
- [x] **Step 6: Run all backend/core tests and commit after the global gate: `feat(kms): drive owner evidence from exclusive drains and stable poll sources`.**

## Task 8: Real-helper coverage, portable gates and handoff [INV, ID-1..3, COMMIT-1..7, CAP-1..4, MULTI]

**Files:** extend `tests/owner_completion_evidence.rs`, public owner test_fixtures, `docs/status.md`; correct this plan after implementation in separate commits.

**Interfaces:** consumes the final implementation; produces reproducible evidence, no new production API solely for testing.

- [x] **Step 1: Complete the real-helper integration matrix.** Use ScriptedReply masks/fds to prove complete/partial output reaches owner over IPC; real status with these non-sync descriptors must fail closed. Fake canonical queries belong only in named deterministic state tests. Include GET and QUEUE success/rejection/death/watchdog, valid page-before-success staging, page-before-rejection contradiction, queue-event-before-reply staging/contradiction, stale full correlations, late-fd disposal, and second-device routing. Bound helper waits/reap; no sleeps for simulated deadlines.
- [x] **Step 2: Run exact global gates.**

```bash
cargo +nightly fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p yserver
cargo test -p yserver-core
cargo test --all-targets --locked
cargo check -p yserver --target x86_64-unknown-linux-gnu
cargo check -p yserver --target x86_64-unknown-linux-musl
cargo check -p yserver --target x86_64-unknown-freebsd
```

On this machine full socket/process tests need execution outside the Codex sandbox. Do not misclassify sandbox denial as a code defect. The known fork/exec-window flake remains confined to `kms::executor::tests::early_take_reap_proof_returns_none_and_does_not_invalidate_future_proof` and the two documented device-lock tests. Identify the exact failing test, retain the output, and treat any other failure as this change's responsibility. No weakened assertions.

- [x] **Step 3: Run twelve clean targeted repetitions, preserving command exit codes.**

```bash
for run in $(seq 1 12); do
  cargo test -p yserver --lib kms::owner || exit 1
  cargo test -p yserver --test owner_commit_record || exit 1
  cargo test -p yserver --test owner_completion_evidence || exit 1
done
```

- [x] **Step 4: Review structural searches by reading every match.**

```bash
rg -n 'SequenceSupport|sequence_support' crates/yserver/src/kms/render/backend.rs
rg -n 'drain_device_events|drain_fd_events' crates/yserver/src
rg -n 'queue_crtc_sequence\(' crates/yserver/src
rg -n 'hardware_complete = true|presented = true|prior_buffer_released = true' crates/yserver/src
rg -n 'atomic_commit\(' crates/yserver/src/drm
rg -n 'ClockProbeLease::for_tests|fn issue\(' crates/yserver/src crates/yserver/tests
rg -n 'IoctlReq|libc::Ioctl' crates/yserver/src/platform/ioctl.rs crates/yserver/src/drm/page_flip.rs crates/yserver/src/kms/executor/helper.rs
rg -n 'PURPOSE_|COUNTER_MASK|tagged_for_tests' crates/yserver/src/kms/owner/identity.rs crates/yserver/src/kms/executor crates/yserver/tests
```

Expected: no backend SequenceSupport cache; one owner KMS drain (parser tests/dormant present loop separately identified); only helper calls queue syscall wrapper; evidence-only hardware/present writes, no new prior_buffer_released write; six atomic sites remain unconverted; private probe/queue issuers and named fixtures. No old platform IoctlReq alias/import or purpose-tag/counter-mask fixture remains in the searched code. Unrelated console/syncobj aliases are distinct boundaries. HostCallEvent/HostCallOutcome remain non-Clone.

- [x] **Step 5: Update status and commit `test(kms): verify stage 2b-ii completion evidence and portable gates`.** After each task, separately fold the code SHA, actual compiled signatures, test counts and necessary deviations back into this plan. Do not write EXECUTED on a task until its work and checks are complete. No session URL in commit messages; sign commits where available.

## Self-review and required execution boundaries

### Revision 2 incorporation audit

All findings refer to `../findings/2026-09-06-phase-c0-stage-2b-ii-plan-review-round1.md`. These are author dispositions, subject to the next pinned review—not a clean-review claim.

| Finding | Correction in this revision |
| --- | --- |
| B-1 | Task 3 adds protocol-v3 QUEUE request/reply family and helper-only syscall; includes asynchronous early-event staging and separate host-call lease. |
| B-2 | Task 2 adds explicit RejectProbeWith emitting ProbeRejected; old RejectWith remains a wrong-family test. |
| B-3 | CompletionQualification checks current GET success for every expected-completion CRTC, even without page/Present events. |
| B-4 | CompletionContext includes host_class; asynchronous blocking install at permitted executor phases, matching validation options and ALLOW_MODESET builder support are defined. |
| B-5 | Task 7 moves before_block before actual timeout calculation and tests shortened-deadline wakeup after removing readiness. |
| B-6 | Task 1 produces legacy permit/proof and one-way APIs; Task 7 defines checked platform proof issuance. |
| M-1 | CompletionCaps has a private production discovery boundary and explicit owner-construction wiring. |
| M-2 | Task 4 produces report_stream_failure; Task 7 preserves valid prefix before reporting the error. |
| M-3 | Exact sequence/consumer signatures and coverage semantics replace untyped identity and implicit target arrays. |
| M-4 | Every task/test case inherits explicit requirement groups; implemented test names must carry them in doc comments/fold-back. |
| M-5 | Final gate includes cargo test --all-targets --locked. |
| M-6 | Arithmetic tests add maximum seconds/usec and full four-sample wrap; Task 7 tests actual protocol projection/offsets. |
| M-7 | Integration matrix separates valid page-before-success from contradictory page-before-rejection. |
| m-1/m-2 | Corrected ClockEpochId derives and ProbeAccepted's existing duration fields. |

Additional author checks: nonblocking DRM fd precondition, checked core consumer allocation, disconnect cancellation, immutable requested-versus-scheduled targets, distinct legacy clock output, non-consumer pre-accept sample storage and immediate post-reply canonical observation are explicit. The extracted arithmetic examples were compiled in an isolated Rust test and passed boundary assertions; this does not establish that the planned production interfaces compile or that implementation gates passed.

### Revision 3 incorporation audit

The second review is `../findings/2026-09-06-phase-c0-stage-2b-ii-plan-review-round2.md`.

| Finding | Correction |
| --- | --- |
| B-1 | CompletionQualification is explicitly a completion-mechanism fact, not full incarnation/structural/readiness qualification; 2c's required conjunction is stated. |
| B-2 | Task 5 removes the old platform alias and replaces actual atomic/GET/QUEUE/status callers with one u32-code, call-signature-inferred wrapper and three-target gates. |
| B-3 | Task 3 removes raw purpose bits/incarnation seeding: shared counter starts at 1 and target kind comes from owner records; wire-v3 goldens and fixtures change together. |
| M-1 | Task 6 stores the actual Atomic SET_CLIENT_CAP result in Device, defines a receiver-based discovery method and separate immutable-discovery/mutable-install passes. |
| M-2 | Unregister only registered fds; immediate Success test asserts no registration/deletion, Pending→Success asserts unregister-before-close. |
| M-3 | Exact reply payload offsets and fixed-byte goldens prescribe duration before sequence/errno. |
| M-4 | Named backend/platform proof methods, visibility, proof location/accessor, refusal variants and EAGAIN-versus-EOF drain result are explicit. |
| M-5 | Named shutdown_drain_present_pending_exec cancellation and its actual core-path regression test are included. |

The two substrate repairs in B-2/B-3 are prerequisites within their owning tasks,
not evidence that 2b-i was reimplemented or that its historical test results changed.

The new plan carries forward the old reviews' defects as explicit checks: no synchronous probe/submit API, no reply without a real event-loop consumer, no clock seeded from a raw u32, no single-CRTC shortcut for a multi-CRTC Present, no lease consumed before validation passes, no handle-only resource ledger, no qualification from an arbitrary commit, no event/fence conflation, and no code that drops old state during rejected/completed retirement.

The four timers are covered without relocating producer waits into the atomic owner. The one-time core fd snapshot is handled with a stable aggregator; executor replacement remains stage 3. The completed Accepted<R> transfer is a handoff to 2c, not a release assertion. Full fd-set poison clearing, reopening and final lifecycle integration are not implemented here. Legacy compatibility forwarding is deleted by the producer conversion work, not accepted as a final C.0 path.

Before execution, run the pinned adversarial reviewer with this scope stated explicitly and verify findings against the baseline. The plan is ready only after its in-scope blocking/major findings are resolved and the concrete final document is reviewed. Keep the existing feature branch and worktree; all of C.0 remains one eventual squash merge requiring confirmation.

### Revision 4 incorporation audit — 2026-09-07

Source: `../findings/2026-09-06-phase-c0-stage-2b-ii-design-review-v2-round1.md`,
including the local supplementary verification. These are author-applied plan
corrections, not a new reviewer verdict or executed tests.

| Finding | Contract and regression coverage |
| --- | --- |
| B-1 | Irreversible publication revocation survives logical arm removal; Task 3 step 2a covers event/cancellation/reply ordering and fresh-token isolation. |
| M-1 | Matching candidate resets on proven pre-IPC retirement only; Task 6 step 3a separates refusal/cancellation from attempted-write uncertainty. |
| M-2 | Post-acceptance checked-add failure enters typed DeadlineOverflow unknown/poison; Task 6 step 3b covers both timer origins, retaining pre-admission lifecycle refusal. |
| M-3 | Handover returns unit after synchronous backend delivery then proof consumption; Task 7 step 2a asserts observable ordering and failure behavior. |

No automatic review rerun is authorized by this audit. Request approval before
another external pass; compilation and full implementation gates remain for
execution, not design-review simulation.

### Revision 5 fold-back — final disposition contract

The final scoped review's B-1 identified loss of an unapplied suffix through a
consuming, fallible batch API. Revision 5 removes that API. Each event receives
Applied or an explicit final Cancelled disposition; an internal failure is
terminal for handover, never a partial-event retry. The drain returns the valid
event prefix independently of its proof/error, and Task 7 step 2a verifies both
middle-event failure and malformed-tail behavior. No resource release is inferred
from cancellation. The stopped-admission restriction is local to migration.

Reference comparison: local Xorg modesetting at `5541a5c`,
`hw/xfree86/drivers/modesetting/vblank.c` (sequence dispatch/abort), and wlroots
at `bd75ebfe`, `backend/drm/drm.c` (handle_page_flip/handle_drm_event), use
callbacks without per-event error returns and explicit cancellation/failure
handling. This motivates final dispositions; it is not evidence that their
callbacks perform no fallible operations or that either implements our
LegacyDrained migration barrier. These are author-verified design edits, not a
new independent review or a claim that the planned callbacks have been tested.
