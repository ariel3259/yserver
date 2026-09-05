# Stage 2 plan review round 2 — slice A, tasks 1-8

Raw output of `codex exec --sandbox read-only`, 2026-09-03, against plan revision 2.

## Regression check

Slice A B-1 — TRADED — `submit` is now asynchronous (`plan:2195-2200`), but watchdog terminalization clears executor occupancy before reap and the production event loop never registers or drains the new control fd.

Slice A B-2 — NOT FIXED — Task 2 specifies `ReplyCorrelation` (`plan:509-512`), but its own test and Task 3 still construct replies with only `seq` (`plan:619-624`, `895-919`), while clock-probe replies need a different tuple that Task 2 never defines.

Slice A B-3 — FIXED — The atomic head is correctly declared as 68 bytes, the body starts at byte 80, and an independent golden-layout test is required (`plan:692-710`).

Slice A B-4 — PARTIAL — `submit` now accepts `ResourceLedger` (`plan:83-88`), but the proposed ledger omits descriptors, uses undefined placeholder ownership types, and no path preserves or returns its old/new sets after completion (`plan:1893-1915`).

Slice A B-5 — NOT FIXED — Task 12 tests `milestones.presented_crtcs` (`plan:2950-2958`), but Task 6 defines only one `presented: bool` (`plan:1803-1811`) and Task 12 still says to set that boolean on an individual consumer event (`plan:3123`).

Slice A B-6 — NOT FIXED — The matrix itself is improved (`plan:2080-2110`), but `submit(request, class, resources)` carries no `is_qualification` argument and `SerializedRequest` has no such field, so the predicate at `plan:2203-2223` cannot receive the value it requires.

Slice A B-7 — PARTIAL — The prose now requires rescanning serialized `CRTC_ID` values (`plan:1585-1596`), but the shown builder still stores positional `bindings`, lacks `object_kinds`/`old_bindings`, and defines neither the test mutation helper nor `crtc_id_prop_ids` (`plan:1450-1461`, `1358`, `1594`).

Slice A B-8 — NOT FIXED — Task 6 tests depend on the Task 7 owner and Task 12 event API (`plan:1718-1731`), and Task 8 redeclares `FenceSlotState` after Task 6 already defined it (`plan:1861-1869`, `2475-2483`).

Slice A B-9 — NOT FIXED — The test expects `EventDisposition::StagedPendingAcceptance` (`plan:2961-2970`), but the produced enum omits that variant (`plan:2937-2940`) and the implementation algorithm does not add it (`plan:3117-3124`).

Slice A M-1 — NOT FIXED — `LifecycleEpochId::next` still uses unchecked addition (`plan:369-371`), while the implemented `IdentityAllocator` also increments unchecked at `identity.rs:163-173`; the spec requires checked, non-reusing allocation (`spec:1663-1672`).

Slice A M-2 — FIXED — Task 3 rejects bits outside the declared slot mask before checking descriptor cardinality and requires high-bit tests (`plan:928-940`).

Slice A M-3 — FIXED — Task 12 dispatches by event variant and adds both atomic-token/sequence-event and sequence-token/page-event contradiction tests (`plan:2973-3001`).

Slice A M-4 — FIXED — Atomic `EBUSY` now closes readiness and requests bounded recovery without poisoning or retrying (`plan:2062-2077`, `2251-2261`).

Slice A M-5 — TRADED — Explicit rejection now frees the slot (`plan:2035-2048`), but the common terminalizer simultaneously promises to tombstone every terminal record and retain the unknown record in the slot (`plan:2272-2278`), which cannot be represented by consuming `into_tombstone`.

Slice A M-6 — FIXED — `CommitRecord` now includes device generation, topology generation, lifecycle identity, exact closure, and completion/event sets (`plan:1833-1859`).

Slice A M-7 — NOT FIXED — Tests still call `open_any_drm_or_skip` (`plan:758`, `782`, `805`) and the task still expects “PASS (or SKIP)” (`plan:978-979`), contradicting its later deterministic `open_any_drm_or_fail` design (`plan:944-971`).

Slice A M-8 — PARTIAL — The completion test now drives signalled fences (`plan:2377-2390`), but `FdCloseCounter` is never defined, and a parent-process counter cannot observe the rejected-holder close performed inside the helper process (`plan:903-919`, `2392-2405`).

Slice A M-9 — FIXED — Task 6 now completes a real record, resolves its actual token as tombstoned, and verifies the delayed event advances nothing (`plan:1718-1731`).

Slice C B-1 — TRADED — The converted live submit API no longer waits, but neither the core poll source nor the watchdog timer integration needed to deliver its asynchronous result is planned.

Slice C B-2 — PARTIAL — `submit` now returns `CommitId` and declares typed events (`plan:82-106`), but downstream tasks still invoke the old two-argument form (`plan:4317`, `4569`) and several consumers continue using the deleted `DamageEvent` API (`plan:4770-4795`, `4827-4829`).

## Blocking

### B-1. No production path services executor replies or watchdogs

Task 4 states that “the core registers `control_fd()`” and calls owner callbacks (`plan:1199-1202`), but no task modifies the core poll-source API or dispatch loop. The real `BackendFdKind` has no executor-control variant (`crates/yserver-core/src/backend/trait_def.rs:57-82`), and its exhaustive dispatch only handles DRM, hotplug, input, host X11, Present completion, and scanout-render completion (`crates/yserver-core/src/core_loop/run.rs:1209-1257`). `PlatformBackend::poll_fds` likewise exposes none (`crates/yserver/src/kms/render/platform.rs:3936-3958`). Consequently a production commit can remain `Submitting` forever: neither a reply nor the two-second watchdog is guaranteed to reach the owner.

The same omission affects Task 8’s assertion that adopted fences are registered with the event loop (`plan:2490`); no poll-source or callback integration is assigned to a file or task.

### B-2. Watchdog expiry releases executor serialization before helper reap

`check_watchdog` marks the executor stalled, requests termination, then clears `self.in_flight` immediately (`plan:1189-1191`). Yet `send` refuses only when that option is populated (`plan:1160-1162`); it does not reject `ExecutorState::Stalled`. A second call can therefore be sent while the timed-out helper may still be executing the first ioctl. This violates the one-host-call rule and the spec’s requirement that an uncertain host call retain its reservation until actual helper reap (`spec:401-402`, `1941-1949`).

The test at `plan:1106-1118` covers only two sends before timeout, not a send after timeout but before reap.

### B-3. Late replies and fds are lost after watchdog expiry

After clearing `in_flight`, Task 4 defines no state that can receive a late reply. This conflicts directly with the synchronization table: a watchdog-unknown record must adopt any later delivered fd into quarantine (`spec:2127-2132`), and channel loss/truncation is acceptance-unknown rather than permission to stop accounting for descriptors (`spec:2143-2150`).

The borrowed-token API aggravates this. `poll_reply(&InFlightHostCall)` and `check_watchdog(&InFlightHostCall)` do not consume the token (`plan:60-65`). A stale token can therefore be called again after another request begins unless every operation authenticates it against the executor’s current internal identity; the implementation instructions specify no such check. It could clear or consume the newer call.

### B-4. Send failure leaves an installed owner record with no terminal path

Task 7 installs the slot and ledger before calling `executor.send` (`plan:2195-2200`), correctly satisfying pre-dispatch ordering, but neither Task 4 nor Task 7 specifies what happens when `send` returns `SendError`. `SubmitError` does not even contain a send/IPC variant (`plan:1963-1966`). The record may remain `Submitting` with no `InFlightHostCall`, no future reply, and no watchdog handle.

The plan must distinguish failure proven before message dispatch from an ambiguous transport failure, terminalize or quarantine accordingly, emit the corresponding owner event, and leave no unreachable slot/resource state.

### B-5. The reply protocol has mutually incompatible definitions

Task 2 normatively changes accepted/rejected replies to carry `correlation` (`plan:509-510`), but:

- Its test still constructs `Accepted { seq, ... }` (`plan:617-624`).
- Task 3’s helper constructs both variants with `seq` rather than `correlation` (`plan:895-919`).
- Task 10 consumes `HostCallReply::ClockProbe` (`plan:2620-2622`), but Task 2’s produced reply list specifies only accepted and rejected variants and its `ReplyCorrelation` contains atomic-only `CommitId` and `EventToken`.
- A clock reply instead needs topology generation, hardware CRTC, clock epoch, and probe id under `COMMIT-5` (`spec:641-645`).

This cannot be implemented task-by-task without inventing a second, undocumented protocol design.

### B-6. Tasks 5–8 remain uncompilable in their stated order

The claimed correction at `plan:156-173` is contradicted by the tasks:

- Task 5 calls undefined `mutate_serialized_value_for_tests` (`plan:1358`) and accesses fields/values absent from its shown struct (`plan:1450-1461`, `1594`).
- Its `SerializedRequest` interface promises `flags` (`plan:1231`), the struct contains only `page_flip_event` (`plan:1621-1630`), and later prose adds a different `allow_modeset` field (`plan:1639`).
- Task 6 tests instantiate Task 7’s owner and Task 12’s event disposition before either module exists (`plan:1718-1731`).
- Task 8 redefines Task 6’s `FenceSlotState` (`plan:1861-1869`, `2475-2483`).

The stale self-review explicitly repeats the revision-1 architecture—claiming `SerializedRequest` comes from Task 4 and that an opaque fence enum is later completed (`plan:5294-5297`)—despite revision 2 saying that architecture is invalid.

### B-7. Unknown terminalization cannot preserve the required live quarantine

`CommitRecord::into_tombstone` consumes the record and returns its ledger (`plan:1921-1924`). The central terminalizer then claims that it:

1. consumes the ledger through `quarantine`,
2. pushes a tombstone, and
3. keeps the `CompletionUnknown` record in the slot (`plan:2272-2278`).

Those operations are mutually exclusive with the declared `CommitRecord { resources: ResourceLedger }` shape (`plan:1833-1859`). A tombstone must not own resources (`spec:1698-1704`), while the unknown slot must retain both state/resource sets and late fds (`spec:2127-2132`). A distinct quarantined-record state and ownership type is required.

Completed ownership is also unfinished: `ResourceLedger::complete` returns both old and new sets (`plan:1912-1915`), but no task stores the new current set or transfers the old set into the later `PriorBufferReleased` ledger. Dropping either result would violate `spec:2098-2114` and `2131`.

### B-8. The multi-CRTC Present fix exists only in a non-compiling test

The spec requires a page event for every Present CRTC (`spec:594-600`). Task 12 expects `presented_crtcs`, but Task 6 defines only a boolean and completion tests that boolean (`plan:1803-1823`, `2946-2958`). Task 12’s algorithm sets it on a single Present event (`plan:3123`). A two-CRTC Present can therefore still complete after the first event.

### B-9. Clock probes and validation still use synchronous result APIs

The corrected architecture says every seat-active host call is asynchronous, but later tasks retain synchronous interfaces:

- `probe_crtc_clock` tests expect an immediate `ClockProbeOutcome::Selected` or `Stalled` (`plan:2641-2649`, `2692-2699`), while its implementation prose says `executor.send` returns immediately (`plan:2734-2737`). `OwnerEvent` contains no clock-probe result.
- `validate` tests synchronously return a snapshot or watchdog error (`plan:4371-4405`), but Task 4 provides only asynchronous send/poll/watchdog. `OwnerEvent` contains no validation result either.

Implementing the tests would block the core; implementing the prose leaves callers without their outcomes. Both contradict `COMMIT-5` (`spec:635-653`).

### B-10. Qualification cannot be expressed and knowingly uses the wrong commit

Task 14 requires `record.is_qualification`, but neither `submit` nor `SerializedRequest` carries it. The statement that “the caller” sets it (`plan:3414-3421`) names no API.

More fundamentally, the plan knowingly labels the first converted primary after a legacy modeset as qualification (`plan:3428`, `5284-5286`). The spec allows qualification only from the mandatory real install/restore commit (`spec:431-435`); an ordinary primary cannot prove the persistent installation it did not perform. Calling this a stage boundary does not make the claimed Stage 2 readiness spec-compliant.

## Major

### M-1. The blocking API is restricted only by its name

`dispatch_blocking_at_permitted_boundary` accepts any `HostCallRequest` and ordinary `SubmittingProof` (`plan:67-72`). The plan explicitly says its name is the enforcement (`plan:67-69`, `1133-1141`). There is no boundary token, `ServicePhase` argument, class check, or visibility separation preventing a seat-active request from calling it. `COMMIT-5` requires an enforced cold-start/final-offline restriction (`spec:646-653`), not a naming convention or source-text test.

### M-2. `Dispatched` is recorded at reply time instead of send time

The spec defines `Dispatched` as becoming true once the record is installed and IPC was sent (`spec:2084-2089`). Task 7 instead sets `record.milestones.dispatched = true` inside `resolve_outcome` (`plan:2231-2236`). During the complete send-to-reply interval, the record therefore falsely says it was not dispatched—the exact interval in which cancellation must no longer use never-submitted cleanup.

### M-3. Non-`EBUSY` rejection recovery is missing

The only errno-specific transition is `EBUSY` (`plan:2251-2261`). The spec separately requires `EACCES`, removed-object `ENOENT`, `EINVAL`, and device loss to cancel invalid-generation work, preserve desired state, and rebuild topology or revoke readiness (`spec:1954-1958`). The plan otherwise frees the record and emits `Rejected`, leaving a Ready owner able to submit again against stale or unauthorized state.

### M-4. Readiness collapses the capability contract to one enum comparison

Task 14 defines `readiness_open()` as only `state == Ready` (`plan:3426`). Section 6.2 makes readiness per incarnation, lifecycle epoch, protocol CRTC, and topology generation and additionally gates it on structural capability, cursor policy, qualification, seat/output state, recovery, generations, and cursor state (`spec:429-435`). No step implements those conjunctions, and the actual tree has no existing `atomic_kms_pipeline_*` state for this task merely to read.

### M-5. The owner cannot return rejected resources to existing BO state machines

An explicit rejection consumes `ResourceLedger::release_new` inside owner terminalization (`plan:1906-1915`, `2272-2275`), but `OwnerEvent::Rejected` carries only commit and errno (`plan:97-106`). Later code must preserve `ReleasedButAtomicRejected` (`plan:4230-4238`, `4317`), yet it receives neither the relevant ownership object nor a typed disposition from the consumed ledger. The owner cannot both exclusively own the ledger and leave backend BO ownership transitions implicit.

### M-6. Exact-close coverage is not implementable as written

`FdCloseCounter` appears in several tests but no task defines it. More seriously, the unexpected rejected holder is closed in the re-executed helper (`plan:903-919`), so a process-local counter installed by the parent at `plan:2396` cannot observe it. The deterministic stub protocol needs explicit shared instrumentation or an acknowledgement carrying a close count; otherwise the test cannot prove its assertion.

### M-7. The deterministic helper-test rewrite contradicts itself

Task 3 introduces an always-available stub and says runtime skip is unacceptable (`plan:944-971`), but all three tests still call the nonexistent/old `open_any_drm_or_skip`, and the expected result remains “PASS (or SKIP)” (`plan:756-805`, `978-979`). Also, the echo-only request proves address patching without exercising the raw ioctl; the real materialization/ioctl path remains hardware-dependent unless the stub executes that same pointer-building code before scripting the result.

### M-8. The owner-event migration is incomplete

Revision 2 declares that `OwnerEvent` replaces `DamageEvent` (`plan:97-112`), but Tasks 20–21 still consume, construct, and match `DamageEvent` (`plan:4717-4718`, `4770-4784`, `4827-4829`, `4838-4841`). Conversely, Task 20 introduces `OwnerEvent::PriorStateProven` (`plan:4746-4749`) without adding it to the normative enum (`plan:97-106`). The damage mapping therefore has neither one stable event type nor one exhaustive producer surface.

### M-9. Downstream submit calls still use the old signature

The normative API requires `(request, class, resources)` (`plan:82-88`), but the production instructions for composed and direct scanout call only `(request, class)` (`plan:4317`, `4569`). These are precisely the paths that must construct and transfer the real ownership ledger; treating the omission as shorthand would leave the central `COMMIT-6` integration unspecified.

## Minor

### m-1. One prescribed Cargo command is invalid

`cargo test -p yserver lifecycle protocol` supplies two positional test filters (`plan:478-481`). Cargo accepts one filter; these must be separate commands or one common filter.

### m-2. The nonblocking timing tests are scheduler-sensitive

Assertions that calls complete within 5 ms or 50 ms and that a busy loop exceeds 1,000 iterations (`plan:1021-1045`, `1047-1056`, `1077-1086`) can fail on loaded CI without detecting a blocking syscall. Deterministic nonblocking sockets plus readiness/state assertions would test the property directly.

### m-3. Task numbering and interface references were not updated consistently

Examples include Task 6 consuming `SerializedRequest` from “task 4” (`plan:1661-1663`), Tasks 18–21 referring to old task numbers and types (`plan:4124-4129`, `4827-4829`), and the self-review repeating revision-1 ordering (`plan:5294-5297`). These errors make dependency interpretation unreliable even where the intended design is recoverable.

## Notes on the rest

- I verified the round-1 false claim against the current tree: both production `queue_crtc_sequence` callers pass `token.as_user_data()` at `crates/yserver/src/kms/render/backend.rs:9342-9347` and `16082-16087`; I did not revive that finding.
- The 68-byte atomic head and byte-80 body offset are now arithmetically sound.
- The out-fence bitmap mask fix is sound.
- The record’s generation and closure fields are now substantively present.
- The revised atomic-`EBUSY` classification is faithful to §9.4.
- Canonical sync-file status interpretation in Task 8 is otherwise sound: readability is treated only as a wakeup, pending remains armed, and negative/unqueryable status does not promote hardware completion.
- The plan is not ready for execution. Task 4 needs a consuming state-machine API whose reservation survives uncertainty until reap, plus explicit core-loop wiring; the reply protocol, record/quarantine ownership, qualification marker, per-CRTC presentation state, and downstream call shapes then need to be made consistent with it.
