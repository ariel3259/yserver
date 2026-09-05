# Stage 2 plan review — slice A, tasks 1-8

Raw output of `codex exec --sandbox read-only`, 2026-09-03. Scope: the executor
wire protocol, the atomic property payload, helper-side materialization and
out-fence holder ownership, the owner request builder, commit records, the
device slot and dispatch, out-fence adoption, and page-event correlation.

## Blocking

### B-1. `submit` synchronously blocks the X11 core on executor IPC

The plan calls `self.executor.dispatch(&wire, proof)` synchronously and handles the result before returning (`plan:1537-1540`). The implemented Stage 1 `dispatch` polls the helper socket until reply or watchdog expiry (`crates/yserver/src/kms/executor/mod.rs:293-380`), potentially blocking for two seconds.

This directly violates:

> “The X11 core never executes or waits synchronously for a potentially blocking KMS ioctl.” (`spec:646-647`)

Process isolation contains a stuck ioctl but does not make this synchronous wait nonblocking. It also prevents the owner from processing page events while the helper is inside the ioctl, undermining the requirement that such events be staged during `Submitting`.

Concrete fix: split dispatch into nonblocking send and later reply handling registered with the core event loop. `submit` must return after the frame is sent; reply, EOF, and watchdog events must call `on_host_call_outcome` asynchronously.

### B-2. Version-2 replies still lack the identities required for stale-result validation

Task 2 defines replies containing only `seq`, duration, bitmap, errno, and an unexpected-output flag (`plan:326`, `plan:696-720`). There is no incarnation, lifecycle epoch, transition id, commit id, or event token.

The spec requires:

> “Every executor request/reply and commit record carries the epoch. A reply is current only when incarnation, lifecycle epoch, optional transition id … and commit id all match.” (`spec:422-425`)

It also requires atomic messages to carry incarnation, lifecycle epoch, transition, commit, and event token (`spec:635-645`). This becomes an explicit inter-task contradiction in Task 9, which proposes rejecting a reply whose incarnation/lifecycle/clock/probe identities are stale (`plan:2052`) even though the reply protocol contains none of those fields.

A per-socket `RequestSeq` is not equivalent: it cannot classify a late success against a changed lifecycle or transition, especially since `COMMIT-6` requires stale success to remain accepted rather than be mistaken for rejection.

Concrete fix: echo the complete request correlation tuple in every atomic reply and the complete probe tuple in every clock-probe reply, then validate it before mutating the current record.

### B-3. The documented atomic frame head is 68 bytes, not 56

The layout says `head (56)` (`plan:510`) but its fields total 68 bytes:

- six `u64` values: 48 bytes;
- presence/class/padding: 4 bytes;
- four `u32` values: 16 bytes.

Total: 68 bytes. With the 12-byte envelope, the body starts at byte 80, not byte 68. An implementation following the declared constant will panic while encoding, overlap fields with the variable body, or reject its own frames.

The round-trip test (`plan:331-346`) is not sufficient guidance because an encoder and decoder can share the same mistaken offsets.

Concrete fix: declare the head as 68 bytes, document exact offsets, use checked cursor arithmetic, and add a golden-byte test that independently asserts the body start and total frame length.

### B-4. The owner API cannot transfer any old/new resources into `Submitting`

`KmsDeviceOwner::submit` accepts only `SerializedRequest` and `CommitClass` (`plan:1483-1487`). `SerializedRequest` contains properties and CRTC/event sets only (`plan:1139-1148`). Nevertheless, the plan claims that `CommitRecord::new(..., &request)` transfers “every possible old/new resource” before IPC (`plan:1502-1513`).

There is no framebuffer, BO, pin, descriptor, external-ownership token, or resource ledger in that call. Task 5 compounds this by saying the record merely stores handles while the backend remains the real owner (`plan:1316`).

The spec requires the opposite:

> “Before executor dispatch, the owner installs the pending record with … cursor framebuffer, gamma blob, primary framebuffer … and both possible old/new ownership ledgers.” (`spec:1687-1693`)

> “The record uncertainty-owns every possible old/new KMS, framebuffer, blob, BO, pin, descriptor, and external-ownership state.” (`spec:2127-2130`)

A handle does not keep a Rust/Vulkan/GBM owner alive and cannot prevent release during acceptance uncertainty.

Concrete fix: make submission consume an owned resource/ownership ledger containing strong RAII references and both possible ownership states. Install that ledger in the record before sending IPC; rejection, completion, and quarantine must consume it through typed transitions.

### B-5. Multi-CRTC Present completes after the first Present event

`Milestones` represents `presented` as one boolean, and `completed_for(NonblockingPrimaryPresent)` checks only that boolean (`plan:1289-1304`). Task 8 says to set it whenever an event CRTC belongs to `present_event_crtcs` (`plan:1932-1933`).

For a multi-CRTC Present, the first correlated consumer event therefore satisfies `Presented`, even if other required Present CRTCs have not produced events. The spec requires:

> “Page event … Required for each Present CRTC.” (`spec:594-597`)

`observed_event_crtcs` exists, but the proposed completion predicate does not compare it with `PresentEventCrtcs`.

Concrete fix: track Presented per CRTC, or set the aggregate milestone only when `present_event_crtcs ⊆ observed_event_crtcs`. Add a test in which one of two Present events arrives and completion remains pending.

### B-6. Admission permits ordinary and blocking commits in forbidden lifecycle states

The proposed predicate is:

```rust
_ => self.state.admits_ordinary_primary()
    || self.state.admits_qualification_commit()
```

(`plan:1491-1494`)

Because `admits_qualification_commit` is true in `Unqualified` and `Ready` (`plan:229-234`), this admits every non-qualification class while unqualified, including `BlockingOrdinary`. It also permits both blocking classes while `Ready`, and maps them to `ColdStartOrOfflineBlocking` (`plan:1516-1523`).

The spec restricts blocking calls to cold startup before service or final offline/shutdown work (`spec:647-653`). During normal seat-active service even install/recovery uses `NONBLOCK` (`spec:648-650`).

Task 12 then consumes `BlockingQualification` for the first converted primary commit (`plan:2370-2377`), even though that commit occurs after the existing modeset has lit the CRTC (`plan:2447`), making a seat-active blocking ioctl likely.

Concrete fix: make live/offline phase explicit and validate `(lifecycle state, service phase, commit class, flags)` as one closed matrix. Ordinary traffic must require `Ready`; seat-active qualification must use an appropriate nonblocking live class.

### B-7. The “final re-scan” does not derive closure from the final serialized request

The builder records old/new bindings separately in `self.bindings` whenever a caller adds any plane or connector property (`plan:997-1019`). `rescan_closure` is then passed those same recorded bindings (`plan:1109-1114`, `plan:1151`). It does not recover bindings from the serialized `CRTC_ID` property values.

Consequences include:

- changing or replacing the serialized `CRTC_ID` value need not change the re-scanned closure;
- inconsistent bindings supplied for different properties on the same object are silently unioned;
- duplicate property replacement in the `BTreeMap` leaves obsolete entries in `bindings`;
- correctness depends on callers truthfully supplying metadata rather than on the final kernel-visible list.

That violates:

> “The owner first computes the closure from the final serialized persistent property list.” (`spec:538-550`)

> “The final serialized request is re-scanned before dispatch.” (`spec:567-569`)

The test corrupts only a synthetic `recorded_closure_override` (`plan:889-899`), so it passes without detecting any real serialized-payload mutation.

Concrete fix: retain typed object metadata and authoritative old bindings, but derive each new binding from the final serialized `CRTC_ID` entry. Test mutation/replacement of an actual serialized binding value.

### B-8. Tasks 5–8 cannot be implemented in the stated order

Several types and APIs are consumed before they exist:

- Task 5 requires `FenceSlotState`, then says to declare an “opaque enum with a `Missing` variant” (`plan:1314`); Task 7 separately declares that it produces and defines the complete enum in another module (`plan:1621`, `plan:1765-1773`). Rust enums cannot later be extended or independently redefined.
- Task 5 places `Vec<StagedPageEvent>` in `CommitRecord` (`plan:1314`), but that type is not defined until Task 8.
- Task 7’s test calls `validate_for_tests` (`plan:1640-1647`), while validation and its lease are not introduced until Task 17 (`plan:3055-3062`).
- Task 8 says it hands normalized MSC/UST from Task 10 to Present (`plan:1933`), but Task 10 has not yet created the normalization API.
- Task 4 promises `SerializedRequest.flags` (`plan:771`), but its shown struct has only `page_flip_event` (`plan:1139-1148`). Task 6 then says `atomic_flags` uses caller-declared `ALLOW_MODESET` (`plan:1592`), for which no field or builder method exists.

Concrete fix: reorder the foundational types before their consumers, or define their final shapes in the earlier task. Remove forward-dependent tests or introduce the required API in the task where it is first exercised.

### B-9. Staging a page event has no representable disposition

Task 8’s result enum has `Presented`, `ObservedNonConsumer`, `ClockSampleOnly`, telemetry, and poison (`plan:1808`). For a Present event arriving while `Submitting`, the plan requires staging it without setting `Presented` (`plan:1823-1834`, `plan:1932`).

No enum variant represents “valid matching Present event staged pending ioctl result.” Returning `Presented` contradicts the test and spec; returning `ObservedNonConsumer` falsely changes its consumer classification.

Concrete fix: add an explicit `StagedPendingAcceptance` disposition and test its return value, not just the record’s side effects.

## Major

### M-1. Identity increment can wrap and reuse lifecycle/event identities

`LifecycleEpochId::next` uses unchecked `self.0 + 1` (`plan:181-188`). Task 6 also relies on the implemented Stage 1 `IdentityAllocator`, whose commit and event counters use unchecked increments (`crates/yserver/src/kms/owner/identity.rs:163-173`).

The spec requires tokens to be allocated with checked increment and never wrap or be reused (`spec:1663-1672`). A release build can wrap silently.

Concrete fix: use `checked_add` and make exhaustion a nonrecoverable invariant failure. Add boundary tests at `u64::MAX` and at the tagged counter limit.

### M-2. The bitmap is not validated against the declared slot table

Task 3 checks only:

```text
received fd count == out_fence_present.count_ones()
```

(`plan:727`)

It does not require `out_fence_present` to have no bits at or above `out_fence_slots.len()`. An accepted reply for zero expected slots could carry bit 31 plus one fd and pass the count check. The owner would ignore/drop that fd while potentially treating the empty expected set as complete.

Concrete fix: reject unless `bitmap & !valid_slot_mask == 0`, then verify descriptor count. Add zero-slot/extraneous-bit and one-slot/high-bit tests.

### M-3. Event-type contradictions are not specified in Task 8

`on_drm_event` accepts a general `DrmEventRecord` (`plan:1806-1809`), but its algorithm treats every record as though it had page-flip `crtc_id` and `user_data` fields (`plan:1927-1933`). Actual `DrmEventRecord::CrtcSequence` has no CRTC field (`crates/yserver/src/drm/event_stream.rs:45-49`).

The spec requires an active token delivered with the wrong event type to poison the incarnation (`spec:1772-1780`). There is no test for an atomic token arriving as Vblank/sequence or a sequence-arm token arriving as PageFlip.

Concrete fix: dispatch by record variant and token target type before applying page-event logic; add both wrong-type poison tests.

### M-4. Atomic `EBUSY` is incorrectly promoted to incarnation poison

Task 6 calls `self.poison(PoisonCause::ForeignBusy)` on atomic `EBUSY` (`plan:1557-1568`). Incarnation poison requires complete fd-family retirement and blocks all submission.

Section 9.4 instead says to close qualification/readiness, retain newest desired state, record the invariant evidence, and enter bounded topology/recovery (`spec:1625-1634`). The completion-poison list does not classify atomic `EBUSY` as a completion-mechanism breach (`spec:1960-1969`).

Concrete fix: represent this as an explicit rejection plus qualification/readiness failure feeding recovery, not as completion-mechanism poison.

### M-5. Terminal records are never moved out of the slot or into tombstones

Task 6 terminalizes rejection records in place (`plan:1555-1572`), but `submit` rejects whenever `self.slot.is_some()` (`plan:1488-1490`). No production step in Tasks 5–8 takes a terminal record, pushes its tombstone, and frees the slot. The only search hits for `into_tombstone` and terminal extraction are tests.

This can permanently block the device after an ordinary explicit rejection and leaves Task 8’s tombstone behavior unspecified.

Concrete fix: define one owner terminalization routine that atomically applies cleanup/quarantine, inserts the tombstone, and either frees the slot or deliberately retains an unknown record.

### M-6. Commit records omit required generation and closure identity

Task 5 lists identities and event sets but does not require device generation, topology generations, or `AtomicCrtcClosure` in the record (`plan:1314`). No `DeviceGeneration` appears anywhere in the plan.

The spec explicitly requires the pending record to contain the exact closure, device/topology generations, lifecycle identity, and resource identities before dispatch (`spec:1687-1697`). Without these fields, later event/reply handling cannot prove that a result belongs to the current generation.

Concrete fix: define the complete record schema explicitly and test stale device, topology, lifecycle, transition, and commit identities independently.

### M-7. The helper integration tests can silently provide no hardware coverage

`TestDevice::open_any_drm_or_skip() -> Self` is left as a comment and tests are declared “PASS (or SKIP)” (`plan:729-746`). Rust’s standard test harness has no runtime skip result. Printing a message and returning early would report a passing test that exercised nothing.

The tests also depend on a DRM node being openable in CI, so the only real helper/property-materialization test can disappear silently.

Concrete fix: use a deterministic stub ioctl target/helper mode for CI, and keep hardware-dependent coverage as a separately reported ignored/hardware test. If absence is permitted, return `Option` and make the suite explicitly report whether zero cases ran.

### M-8. The exact-close test exercises drop, not hardware completion

`every_adopted_fence_is_closed_exactly_once_on_hardware_completion` never marks either fence signalled or calls `poll_fences`; it simply drops the owner (`plan:1686-1695`). It therefore passes if the hardware-completion path leaks or mishandles descriptors, provided destructor cleanup works.

The rejected-output test similarly initializes holders to `-1` and expects no unexpected result (`plan:588-607`); it never exercises the required close of an unexpected nonnegative rejected output.

Concrete fix: instrument descriptor ownership/close transitions, drive fences through successful status and `poll_fences`, and inject a rejected ioctl result with a nonnegative holder.

### M-9. The tombstone test named for tombstones never tests one

`zero_unknown_and_tombstoned_tokens_are_telemetry_only` checks only zero and a fabricated unknown token (`plan:1845-1857`). It never completes a record, obtains its tombstoned token, or asserts `TelemetryReason::Tombstoned`.

Concrete fix: terminalize a commit, verify the live record was replaced with a tombstone, then deliver its exact token and assert no milestone, clock, or resource state changes.

## Minor

### m-1. The “tombstone owns no resource” assertion is tautological

The test asserts that `terminal_state` has size one byte (`plan:1243-1252`). That neither proves the tombstone lacks a resource ledger nor prevents a future resource-owning field from being added.

A compile-time shape claim should be enforced through visibility/type design or a drop-counted resource test around `into_tombstone`.

### m-2. `TombstoneRing::resolve` cannot itself return `Resolution::Live`

The interface assigns `TombstoneRing::resolve` the results `Live`, `Tombstoned`, and `Unknown` (`plan:1179-1180`), but the ring stores only terminal tombstones (`plan:1318`). Live resolution belongs to `KmsDeviceOwner`, which must check its slot before consulting the ring.

Separate the owner-level resolution API from the ring lookup result.

### m-3. The helper echo test proves replacement, not pointer validity or lifetime

The debug echo test only asserts that the patched value is nonzero (`plan:609-626`). Any nonzero constant would pass. The shown implementation’s allocation ordering is sound, but the test does not protect it from later reallocating `holders` after addresses are installed.

A helper-local unit test can assert that every patched value equals the corresponding holder address after final allocation and immediately before ioctl invocation.

## Notes on the rest

- The current source anchors were checked against the merged tree. The important anchors are accurate: `HostCallClass::from_request` is at `executor/mod.rs:168`; modeset atomic calls are at `modeset.rs:1144`, `1305`, `1562`, `1635`, and `1690`; composed submissions are at `platform.rs:5163` and `scene.rs:6769`; direct submission/successor/unflip sites are at `backend.rs:1831`, `1843`, `1892`, and `2234`; damage sites are at `scene.rs:1978`, `4344` and `backend.rs:2273`, `17273`; device open is at `kms/backend.rs:844`.
- The helper-side pointer lifetime shown in `execute_atomic` is otherwise sound: all arrays and holders are fully allocated before raw addresses are installed, none are reallocated before the ioctl returns, and the pointer-width-to-`u64` conversion matches the DRM UAPI field (`plan:638-678`).
- Accepted returned fds are converted exactly once into `OwnedFd`; after a successful `SCM_RIGHTS` send the helper’s local copies are dropped, and a send failure also drops them. The rejected branch explicitly closes unexpected nonnegative outputs once (`plan:681-723`). The missing issue is test coverage, not the shown ownership path.
- Property count validation correctly checks object/count cardinality, checked count summation, the property bound, and values/property equality (`plan:464-491`). With the stated maxima, the variable-body size is comfortably below 32 KiB.
- The powered old-or-new definition of `ExpectedCompletionCrtcs`, detach retention, disable handling, and prohibition on fences outside the expected set are represented correctly in the Task 4 tests (`plan:803-875`).
- Task 7 correctly treats readability only as a wakeup and uses `SYNC_IOC_FILE_INFO` status as authoritative (`plan:1715-1780`). Its `SyncFileInfo` field layout matches the Linux UAPI shape, and successful, pending, negative, and unqueryable outcomes are distinguished.
- Task 8 correctly distinguishes consumer and non-consumer CRTC events, stages events that precede acceptance, treats duplicates as telemetry-only, and poisons a current token paired with zero or an out-of-set CRTC (`plan:1814-1900`). The missing cases are multi-consumer completeness, tombstone execution, and wrong event types.
