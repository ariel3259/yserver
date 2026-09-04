# Stage 2 plan review round 2 — slice B, tasks 9-17

Raw output of `codex exec --sandbox read-only`, 2026-09-03, against plan revision 2.

## Regression check

Slice B B-1 — TRADED — `HostCallReservation` supplies a legal proof and serializes host calls (plan:2712-2738), but `probe_crtc_clock` is still tested as synchronously returning `Selected`/failure (2630-2699) while the implementation says it sends and returns before any reply (2734-2737).

Slice B B-2 — NOT FIXED — the plan still designates “the first converted primary commit after the existing `commit_modeset`” as qualification (3428), contradicting spec §10.1’s exact install/restore commit requirement (spec:2041-2052).

Slice B B-3 — PARTIAL — `AdmissionCandidate` adds generations, closure, coverage, absorption, and age, but omits the synchronous/async class promised by Task 16 (plan:140-147, 3626-3629) and cannot prove that independently serialized candidates form one compatible tier-5 atomic request.

Slice B B-4 — FIXED — round-robin state is now internal, advanced by `select`, and tested through repeated selections without caller mutation (3855-3858, 3803-3817).

Slice B B-5 — FIXED — `offer_maintenance(..., SlotState::Occupied)` creates the intent already aged and preserves that state across replacement (3534-3542, 3577-3583).

Slice B B-6 — PARTIAL — direct replacement now returns the owned `DirectIntent` (3592-3602), but barrier supersession still tests/returns serial-only `Displaced` data and barrier rejection still expects `None` (3544-3553).

Slice B B-7 — TRADED — checked arithmetic and the unvalidated-cohort result fix the substantive deadline rule (3173-3201, 3263-3306), but the task’s declared interface still says `lifecycle_hardware_deadline(Duration) -> Duration` (3151-3153), disagreeing with its `Option<Duration> -> Result<...>` implementation.

Slice B B-8 — PARTIAL — zero timestamps are removed, but `SkipWithoutClock` is absent from the declared `ProtocolCompletion` enum and no wire-level notification disposition is defined for it (3911-3913, 3965-3975, 4067-4073).

Slice B B-9 — PARTIAL — the proposed `ProtocolKey` and `ReleaseKey` are generation-safe (4035-4055), but most Task 17 tests still use `PresentSerial` or `BufferRef` as keys and incompatible old method signatures (3919-3961, 4001-4019).

Slice B M-1 — PARTIAL — the finding’s premise remains false: both production callers already pass `token.as_user_data()` and `SequenceArm` exists (`backend.rs:1520-1535,9342-9347,16082-16087`); however, no task moves the existing arm table or producers into the owner that Task 12 assumes resolves them.

Slice B M-2 — PARTIAL — Task 9 correctly identifies the real map and production accesses, confirmed at `backend.rs:1041-1042,9246,9297,9381-9405,16052`, but does not provide a realizable backend-to-owner ownership path and omits numerous initializers/tests that still name `SequenceSupport`.

Slice B M-3 — TRADED — the implementation prose correctly makes stale probe replies neutral (2740-2750), but the retained test still expects `QualificationFailed(0)` (2683-2690), and the synchronous return shape conflicts with asynchronous dispatch.

Slice B M-4 — PARTIAL — Task 12 nominally integrates normalization, but still writes the obsolete Boolean milestone and references “task 10” for normalization (3122-3123); it neither stores separate non-regressing general/completion clocks nor connects the production DRM drain.

Slice B M-5 — PARTIAL — topology ticket remapping is added (3820-3829, 3882-3884), but the owner-mediated coordinate lane and its post-`EBUSY` retry before atomic tiers remain absent.

Slice B M-6 — PARTIAL — starvation and round-robin tests are substantially stronger, but qualification still uses a generic primary helper (3343-3350), and the advertised-capability assertion remains tautological (3385-3392).

Slice B M-7 — PARTIAL — liveness, FIFO-unpark, and teardown APIs are described (3977-3986, 4075-4093), but their tests and ledger signatures disagree, so no coherent implementation is specified.

Slice B M-8 — NOT FIXED — Task 17 still calls `record_displaced_successor`, `record_accepted`, `complete_accepted_without_presented`, and `note_prior_buffer_released` in multiple incompatible shapes (3919-3961, 3988-4019), despite claiming one signature at 4058-4063.

Slice A B-5 — NOT FIXED — Task 6 still defines one `milestones.presented: bool` and completes on it (1803-1824); Task 12’s new `presented_crtcs` test refers to a field no task defines, while its implementation still sets the Boolean (2946-2958, 3123).

Slice A B-9 — PARTIAL — the test uses `StagedPendingAcceptance` (2961-2970), but the declared `EventDisposition` omits it (2936-2940), and the implementation never explicitly says to return that variant (3117-3123).

Slice A M-1 — NOT FIXED — revision 2 still specifies unchecked `LifecycleEpochId(self.0 + 1)` (369-371), while the implemented allocator still increments commit and tagged counters unchecked (`identity.rs:163-173`).

Slice A M-3 — PARTIAL — wrong-type tests were added and acknowledge that `CrtcSequence` has no CRTC field (2973-3001), matching the actual enum (`event_stream.rs:45-49`), but the algorithm still begins generically and no owner-side `SequenceArmTable` is defined.

## Blocking

### B-1. The normative corrected task order contradicts the actual plan

The “corrected architecture” normatively assigns task 9 to page-event correlation, task 10 to cache migration, task 11 to probing, and task 12 to normalization (plan:112-128). The executable body instead makes task 9 cache migration, task 10 probing, task 11 normalization, and task 12 correlation (2509, 2612, 2769, 2930). This causes pervasive incorrect dependencies: Task 9 modifies a clock file “created by task 10” while claiming to land first (2530-2533), Task 10 again says it creates that file (2616-2618), and later tasks consume artifacts under the wrong task numbers.

### B-2. The clock-probe API cannot be both asynchronous and return its terminal result

Tests require `probe_crtc_clock(40)` to return `Selected`, `QualificationFailed`, or `Stalled` immediately (2641-2699). The normative implementation sends through `executor.send` and “returns immediately,” with the reply arriving later through `on_control_readable` (2734-2737). No probe-result `OwnerEvent` exists in the architecture’s outcome stream (plan:74-84). The task therefore defines no implementable way to produce its advertised result.

### B-3. Multi-CRTC presentation remains unrepresentable

`Milestones` has only `presented: bool` (1803-1811), and `completed_for` accepts that Boolean (1813-1824). Task 12 tests `presented_crtcs`, which is undefined (2946-2958), while its algorithm still sets `milestones.presented` once (3123). The first page event can therefore still complete a multi-CRTC Present, contrary to spec:2095-2111.

### B-4. The seven-tier candidate cannot construct the selected request

The candidate contains metadata but no `SerializedRequest` or merge proof (plan:140-147). Tier 5 returns several independently built candidates (3846), yet nothing proves their property lists are mutually compatible or constructs the required single atomic transaction. Tiers 4 and 7 reduce symmetric primary absorption to `absorbed_primary: Option<u32>` (3845, 3848), discarding the selected primary generation, request, and resources. These choices cannot be dispatched faithfully.

Tier-by-tier:

1. Tier 1 is represented correctly by `Barrier`.
2. Tier 2 is represented correctly by `Recovery`.
3. Tier 3’s round-robin direction is corrected, but synchronous class and exact snapshot/request currency are absent.
4. Tier 4 selects the oldest maintenance ticket, but loses the absorbed primary candidate.
5. Tier 5 includes every ready group CRTC in its test, but cannot prove or build one compatible atomic request.
6. Tier 6 has primary age and internal round-robin state, though the precedence between “oldest” and “owed” remains underspecified.
7. Tier 7 selects oldest non-aged maintenance, but has the same lossy symmetric-absorption representation as tier 4.

### B-5. Qualification still deliberately uses a non-qualification commit

Task 14 correctly introduces `record.is_qualification` in its predicate (3402-3421), then explicitly marks the first converted primary after legacy modesetting as the production qualification commit until stage 3 (3428). That is precisely the ordinary-primary qualification defect found in round 1 and directly contradicts spec:2041-2058.

Additionally, neither `SerializedRequest` (1622-1630) nor `KmsDeviceOwner::submit(request, class, resources)` carries `is_qualification`; no specified API can set the record field safely.

### B-6. Task 17 is internally unimplementable

Its interface declares only `Flip` and `Skip` (3911-3913), but tests and prose require `SkipWithoutClock` (3965-3975, 4067-4073). Early tests use two `PresentSerial`s for displacement (3921-3948), while the implementation consumes `DirectIntent` plus `ProtocolKey` (4058-4063). Release tests alternate among `(CommitId, BufferRef)`, `BufferRef`, and the proposed generation-bearing `ReleaseKey` (3988-4019, 4045-4055). No single Rust API satisfies these steps.

### B-7. Sequence-arm ownership required by Task 12 is never migrated

The implemented tree keeps `SequenceArmTable` and both sequence producers in `KmsBackend` (`backend.rs:1030-1042,1520-1553,9337-9371,16070-16105`). Task 9 migrates only `SequenceSupport`; Task 12 nevertheless expects `KmsDeviceOwner::arm_sequence_for_tests` and owner token resolution (2984-3001). No task transfers the live arm table, consumer sets, cancellation, or producer event routing to the owner.

### B-8. Later integration still calls `submit` without the mandatory resource ledger

The corrected architecture requires `submit(request, class, resources)` (plan:60-65), and Task 7 uses that shape. Tasks 18 and 19 still instruct production callers to call `owner.submit(request, CommitClass::NonblockingPrimaryPresent)` with only two arguments (4317, 4569). Besides failing to compile, this drops the mandatory uncertainty-owned ledger.

## Major

### M-1. Task 9 cannot actually make the backend consult an owner record

Task 9 tells existing backend sites to call `owner.clock_source(hardware_crtc)` (2587-2591), but it neither adds an owner field/reference to `KmsBackend` nor defines how the existing synchronous sequence-arm paths access the device-local owner. Its source-string test also requires every `SequenceSupport` occurrence to disappear (2560-2563), while the task’s inventory omits constructors, imports, and tests containing that name throughout `backend.rs`.

### M-2. Clock normalization does not preserve both monotonic clock domains

Task 11 advances only the kernel sequence reference (2914). It defines no last validated general-clock sample and no completion-clock sample, especially no UST monotonic guard. Returning a late normalized sample to Task 12 can therefore regress a consumer clock even though the extension reference itself stays monotonic, contrary to spec:1824-1827.

### M-3. The production DRM event drain is never connected to the owner

Task 12 modifies only `owner/events.rs` and `device_owner.rs` (2932-2935). No step replaces or routes the existing production `receive_events()` drain through `KmsDeviceOwner::on_drm_event`, despite spec:1710-1729 requiring a single owner-exclusive typed parser. Unit helpers invoking `on_drm_event` cannot establish production ownership.

### M-4. Task 15 retains incompatible APIs after adding slot-aware aging

The first four tests call `offer_maintenance(identity, generation)` (3464-3502), while later tests and the implementation require a third `SlotState` argument (3534-3542, 3577-3583). The task cannot compile as written.

### M-5. Barrier displacement still loses owned cleanup state

`offer_direct_successor` returns owned intents, but `offer_barrier` is tested as returning `Displaced { idle_now, ... }` containing only serials (3544-3553). Superseding direct work via an unflip/topology barrier therefore still lacks the buffer, pin, wake, client, and window ownership required by spec:1423-1428.

### M-6. Primary intent eligibility omits the §9.1 coverage invariant

Neither `DirectIntent` nor `AdmissionCandidate` is required to carry or prove authoritative-root/full-output coverage. Closure membership is not equivalent to Present coverage. Thus the direct successor slot can admit a request that does not replace the complete plane state, contrary to spec:1407-1421.

### M-7. The coordinate lane’s priority is absent

Task 16 implements only atomic barriers, recovery, primary, and maintenance. It has no representation for `OwnerMediatedLegacyMove`, its bounded per-plane reservation, or its one post-`EBUSY` retry before atomic tiers (spec:1475-1481, 1595-1602, 1636-1649). Topology remapping alone does not fix round-1 M-5.

### M-8. Idle-scene maintenance has no dispatch trigger

The plan gives a ready maintenance identity a ticket while the slot is idle (3464-3471), but never specifies that `offer_maintenance` immediately invokes owner admission and dispatch. Task 16 explicitly discusses retirement wakes (3634-3636). Without an offer-time wake, cursor/gamma can still wait for later scene or retirement activity, violating spec:1595-1610.

### M-9. `EventDisposition` and implementation disagree

The interface omits `StagedPendingAcceptance` (2936-2940), yet a required test returns it (2961-2970). The implementation instead says a staged event returns `Presented` “only after acceptance” without specifying the immediate disposition (3122). The enum and algorithm need one explicit shape.

### M-10. Deadline integration bypasses the architecture’s single event stream

The corrected architecture exposes `tick(now) -> Vec<OwnerEvent>` as the timer entry point (plan:65-70). Task 13 instead adds a separate `tick_deadlines` method (3154-3155) and never states that `tick` invokes it or emits the resulting `CompletionUnknown`/damage events.

## Minor

### m-1. Task 13’s public signature is stale

The declared `lifecycle_hardware_deadline(observed_max: Duration) -> Duration` at 3152 conflicts with every test and implementation at 3173-3184 and 3283-3294.

### m-2. Task references remain shifted

Task 12 says it consumes `CommitRecord` from task 5 although it is defined in task 6 (2937), and Task 17 says it consumes `Displaced` from task 13 and `ClockSample` from task 10 although those belong to tasks 15 and 11 (3908-3910).

### m-3. `SkipWithoutClock` lacks a notification policy

The plan creates a protocol-terminal value that cannot carry Present timestamps but only suppresses notification for dead drawables (3965-3985). It never states whether a live drawable receives no notification, a different protocol event, or an invalid timestamp-free completion.

## Notes on the rest

Task 9’s factual claims about the existing cache are substantially verified: the separate map is at `backend.rs:1041-1042`, reads are at 9246, 9297, and 16052, and the write/accessor are at 9381-9405. The hardware CRTC is indeed absent from its key.

The round-1 raw-identity claim remains refuted. `SequenceArm`, `SequenceArmPurpose`, and `SequenceArmTable` exist at `backend.rs:1520-1553`, and both production queue calls pass `token.as_user_data()` at 9342-9347 and 16082-16087. The stale comment in `drm/page_flip.rs:70-71` is documentation residue, not production encoding.

The sequence-extension arithmetic and UST boundary remain sound. Task 13’s checked multiplication, checked `Instant` addition, exact clamps, and non-poisoning unvalidated-cohort disposition are substantively correct. Ticket allocation now has an explicit checked-overflow policy, and topology remapping preserves surviving ticket age.
