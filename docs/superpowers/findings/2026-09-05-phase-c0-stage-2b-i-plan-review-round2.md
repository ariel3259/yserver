## Verdict

10 blocking, 4 major, 1 minor

## Incorporation audit

| Prior finding | Status | Audit result |
|---|---|---|
| B-1 | APPLIED | `FencePolicy::Forbidden` makes validation require an empty fence set (plan:153-160, 783-802), and Task 5 builds validations without slots (plan:1666-1670). |
| B-2 | TRADED | Equality replaced containment (plan:886-898), but the redesign forces `ACTIVE` onto every closure CRTC, violating the spec’s minimal persistent-list rule. See B-1. |
| B-3 | APPLIED | Caller-supplied fences are rejected during `compute`, and the re-scan counts duplicates (plan:619-623, 858-901). |
| B-4 | TRADED | The ledger owns `R` by value, but rejection/refusal drops the still-current old state, while `KmsResource` contains only numeric handles. See B-2 and B-3. |
| B-5 | APPLIED | Task 5 uses `kms::executor::HostCallClass`, and Task 6 sends `HostCallRequest::Atomic` (plan:1486-1488, 2141, 2157-2158), matching `executor/mod.rs:167,660-664`. |
| B-6 | APPLIED | `RecordState` is `Copy` (plan:309-313), and Task 4 adds `Debug` to `IdentityAllocator` (plan:1224-1226). |
| B-7 | PARTIAL | `send_on` distinguishes the five pre-install refusals from `SendError::Ipc` (plan:2146-2183), but the refusal path destroys both resource sets and emits no release event. See B-2. |
| B-8 | NOT APPLIED | The validation lease is released before the live call, can coexist with an occupied slot, and is resolved by commit id alone. See B-4 and B-5. |
| B-9 | APPLIED | Short masks become `IncompleteFenceOutput`, and `FenceEvidence` retains slots, mask, and fds (plan:299-334, 1936-1957, 2246). |
| B-10 | APPLIED | Contradictory validation/probe-shaped outcomes under a live record become `CompletionUnknown` (plan:371-390, 1959-1973, 2246). |
| B-11 | NOT APPLIED | Both proof types retain an unconditional public `for_tests()` constructor (plan:411-418, 1062). `#[doc(hidden)]` is not an access restriction. See B-6. |
| B-12 | PARTIAL | Device-keyed event transport is specified, but its two-device proof test waits on the wrong executor. See B-9. |
| B-13 | PARTIAL | Most owner methods are specified, but `send_validation_on` appears only in Produces. See B-7. |
| B-14 | NOT APPLIED | Public fixtures are proposed, but the external test lacks imports; backend code uses nonexistent or wrong-typed helpers; a `KmsDevice` literal is omitted; and the routing test waits on device 0. See B-8 through B-10. |
| M-1 | NOT APPLIED | Consumes/Produces lists still omit methods, constants, helpers, and concrete types used later. See M-2. |
| M-2 | PARTIAL | A separate observation type exists, but `HostCallObservation::of` is invoked without being produced or specified. See M-3. |
| M-3 | APPLIED | Fixed in the tree: `wait_readable` uses its `timeout` at `executor/test_support.rs:778-787`. |
| M-4 | APPLIED | `UnknownReason::index` is exhaustive and `ALL` is const-checked against it (plan:1786-1820). |
| M-5 | APPLIED | Grep 5 no longer asserts the impossible count (plan:2545-2550). |
| m-1 | APPLIED | The event-tee and holder-pointer descriptions now match the tree (plan:4, 1521-1527). |

## Findings

### Blocking

#### B-1 — The closure model makes minimal plane/connector requests impossible

`compute` requires every closure member to have a `Crtc` object containing an `ACTIVE` persistent property (plan:178-180, 625-648). The detach test repairs `UnknownPower(1)` by adding `crtc(1, ...)` to the serialized objects (plan:497-510). Task 5 then copies every such property onto the wire (plan:1685-1695).

Consequently, a plane-only change must gratuitously serialize unchanged `ACTIVE` properties for its bound CRTCs. The spec instead says bindings contribute CRTCs to the closure while the persistent list remains minimal, containing only objects whose generation changes or which the kernel requires (spec:538-557). Current CRTC power must be retained metadata; it cannot require another persistent property entry.

#### B-2 — Rejection and pre-dispatch refusal destroy the still-current old resources

`Submitted::rejected` retains `old` in `Rejected` (plan:222-231), but `terminalize_rejected` stores that value in the record (plan:1407-1423), after which `retire_live` clears and drops the record (plan:2246). The drop-counting test explicitly expects the old resource to be destroyed with `Rejected` (plan:968-974).

The pre-dispatch refusal path is worse: `terminalize(NeverDispatched)` leaves the ledger `Submitted`, then `retire_live` drops both states without emitting `ResourcesReleased` (plan:2178-2183).

For cancellation or explicit rejection, only unreferenced new resources may be released; the old state remains current (spec:1929-1939, 2127-2128). Dropping its RAII owners can destroy an in-use framebuffer, BO, or pin.

#### B-3 — The concrete `KmsResource` does not own the required resources

Task 7 calls `KmsResource` the real 2c resource type but specifies only `Framebuffer(framebuffer::Handle)` and `GammaBlob(u32)` (plan:2410-2416). These are identifiers, not owners. They retain no framebuffer object, BO, pin, descriptor, cursor state, or external-ownership ledger.

The record must own cursor/gamma/primary resources before dispatch (spec:1687-1693), and uncertainty ownership includes every old/new framebuffer, blob, BO, pin, descriptor, and external-ownership state (spec:2127-2132). The proposed type cannot satisfy that contract.

#### B-4 — The validation lease ends before the interval it exists to protect

The final serialized validation must hold an exclusive lease so no persistent generation changes before the corresponding live call (spec:305-325). The plan instead makes `resolve_validation` release the lease immediately upon the TEST_ONLY outcome (plan:2246). Only afterward can `begin` accept an arbitrary new `CommitDescription`; no snapshot or equality check binds it to the validated request.

Moreover, `acquire_validation` checks only for another validation, not for `occupant` (plan:1177-1183). A validation can therefore begin while an unresolved live commit may still change persistent state.

#### B-5 — Validation resolution bypasses the full ID-3 currency check

`pending_validation` stores the request and its complete correlation (plan:2077-2088), but `apply_host_call_event` resolves it when only `CommitId` matches, before checking `late` or `is_current` (plan:2212-2233).

A stale result with a colliding device-scoped commit id but different incarnation, lifecycle epoch, transition, sequence, or token can release the current owner’s validation lease. ID-3 and accepted-stale handling require the full relevant identity tuple to be current (spec:416-420, 2205-2210).

#### B-6 — The “unforgeable” proofs remain publicly forgeable

Both proof types expose an unconditional public `for_tests()` constructor (plan:411-418), which Task 3 explicitly preserves (plan:1062). `#[doc(hidden)]` affects documentation only. Production modules and downstream crates can mint proofs without reserving the slot.

The current tree explains why this seam is unconditional: external tests invoke it throughout `crates/yserver/tests/executor_async.rs:92-748`. The claimed type-system guarantee at plan:399-416 and 2600 is false.

#### B-7 — `send_validation_on` is promised but never specified

Task 6 produces `DeviceCommitOwner::send_validation_on` (plan:1776-1778), but that is its only occurrence. The shown implementation ends after `apply_host_call_event` (plan:2075-2244).

Nothing specifies:

- Moving the stored lease into `HostCallReservation::Validation`.
- Releasing the lease after a pre-IPC refusal.
- Preserving pending correlation after `SendError::Ipc`.
- Preventing a second send.

An implementer confined to Task 6 cannot implement the advertised validation dispatch consistently with the real API at `executor/mod.rs:660-692`. Existing tests never actually send a validation.

#### B-8 — Task 7 references backend interfaces that do not exist

The routing body calls `self.platform.owner_for(key)` (plan:2435), but the tree exposes only immutable `device_for_key` at `render/platform.rs:3836-3841`; no task produces `owner_for`.

The backend helper calls `test_ledger()` (plan:2480-2483). Task 6 instead provides `ledger()` returning `Submitted<TestResource>` (plan:2266-2268, 2324), which cannot be passed to `DeviceCommitOwner<KmsResource>`.

Adding `owner` also requires updating every `KmsDevice` literal. Plan:2500 claims to enumerate them but misses `render/backend.rs:24229-24234`, which will fail with a missing-field error.

#### B-9 — The routing test waits on the executor that received no request

The test sends only on device index 1 and then calls `wait_executor_readable_for_tests(&backend, ...)` (plan:2362-2375). The existing helper always selects `.devices.first()` and polls device 0 (`render/backend.rs:39659-39671`).

Device 0 has no in-flight request, so the test times out instead of proving device-keyed routing.

#### B-10 — The shown external integration test does not compile

The purported file imports only `test_support`, `StubBehaviour`, and three fixtures (plan:2257-2268), but then uses unqualified `Duration`, `OwnerEvent`, `TerminalState`, `FailureCause`, `UnknownCause`, and `UnknownReason` (plan:2269-2320). None is in Rust’s prelude or supplied by those imports.

### Major

#### M-1 — Malformed bindings and duplicate persistent properties survive construction

Both `compute` and `verify_serialized` truncate non-zero `CRTC_ID` values from `u64` to `u32` (plan:640-643, 870-875). For example, `0x1_0000_0001` is recorded as CRTC 1 while the kernel receives the original invalid value.

The plan also:

- Uses only the first `ACTIVE` value (plan:629-635).
- Accepts multiple `CRTC_ID` properties on one object.
- Permits duplicate connector/plane object rows.
- Silently keeps the last duplicate in `CommitDescription::kinds()` (plan:1644-1650).

These ambiguous inputs should fail construction.

#### M-2 — Consumes/Produces declarations still omit required names

Examples:

- Task 4 omits `terminalize_rejected`, used by Task 6 (plan:1220-1222, 2246).
- Task 5 omits `TEST_PROPERTY_IDS` (plan:1486-1488, 1738-1740).
- Task 6 omits fixture `ledger`, `mark_dispatched_for_tests`, and numerous event fixtures used by its tests (plan:1776-1778, 1830-2044, 2248, 2324).
- Task 7 introduces `KmsResource`, `HostCallObservation`, `ObservedOutcome`, `owner_for`, and `test_ledger` without a coherent Produces contract (plan:2349-2351, 2410-2497).

This still violates the plan’s rule that an implementer sees only their own task.

#### M-3 — The fd-free observation conversion is invoked but never specified

Task 7 defines `HostCallObservation` and `ObservedOutcome` (plan:2448-2470), then calls `HostCallObservation::of(&event)` (plan:2431-2434) without declaring or showing that method.

Its exhaustive mapping is material: probe rejection is represented as `HostCallOutcome::Rejected` in the existing executor (`executor/mod.rs:809-840`), late status comes from the outer event variant, and accepted fence count must be inspected without moving descriptors.

#### M-4 — The “neither event type became Clone” grep cannot detect `Clone`

Grep 6 first selects only derive-attribute lines and then searches those same lines for the type names (plan:2552-2557). Rust puts the derive attribute and type declaration on separate lines, as at `executor/mod.rs:237-240,358-361`.

The command therefore reports no matches even if either event type derives `Clone`.

### Minor

#### m-1 — Several existing-tree anchors are inaccurate or incomplete

- Plan:106 places both proof types at `executor/mod.rs:222-250`; `ValidationLease` is actually at 296-306.
- Plan:1498 cites the flag match as `protocol.rs:514-520`, but it continues through line 525.
- Plan:2500 names lines containing `executor` fields rather than complete literals and omits the backend literal entirely.

Other behavioral anchors were confirmed.

## Notes on the rest

1. **Incorporation audit:** I checked all 20 prior findings against both the disposition table and the actual revised tasks.

2. **Existing-code verification:** I opened every cited tree anchor. Confirmed sound were the `HostCallClass` location, `send` signature and refusal ordering, partial-mask behavior, corrected timeout helper, identity APIs, device event-drain paths, `SequenceSupport` map, one-time core poll-source collection, and all six live `atomic_commit` calls.

3. **Cross-task consistency:** I traced production and test-only interfaces, including external-test visibility. The corrected executor imports, request wrapping, `RequestSeq::from_raw`, fence-evidence fields, and public fixture module are sound. Remaining breaks are reported above.

4. **Compile/test audit:** I checked every shown Rust block against the current executor API. The definite compile failures and impossible test are B-8 through B-10. `RecordState: Copy`, generic `DispatchError<R>`, const-exhaustive `UnknownReason`, validation fence policy, and slot-to-CRTC mapping are otherwise type-consistent.

5. **Spec compliance:** I checked spec sections 5, 6.1, 6.3, 10.2, and 18. I excluded every explicitly deferred 2b-ii, 2c, recovery, cursor/gamma/coordinate, call-site-conversion, poll-source-churn, and known-flake item. The deliberately retained accepted/unknown slots, unconsumed owner stream, and unread tombstone ring were not reported.

The findings could not be written to `docs/superpowers/findings/2026-09-05-phase-c0-stage-2b-i-plan-review-round2.md` because this session’s filesystem is read-only; the attempted patch was rejected.