## Verdict

14 blocking, 5 major, 1 minor

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `38aee673`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.152.0`.
Counts are comparable only to other reviews citing this same instrument SHA.

> The reviewer's closing note claims `review.sh` "failed before review because
> this environment is read-only". That describes a *nested* dispatch it
> attempted on its own; the outer dispatch this document came from ran under
> the pinned instrument above and produced these findings. The provenance is
> valid; the note is the reviewer misreporting its own recursion.

## Verification against the tree

Per `docs/superpowers/review/README.md`, blocking findings were checked against
the tree before filing. All claims about existing code that were checked are
**confirmed**:

| Finding | Claim checked | Result |
|---|---|---|
| B-5.1 | `HostCallClass` lives in `executor/mod.rs:167`, not re-exported by `protocol.rs` | confirmed |
| B-5.2 | `send` takes `&HostCallRequest`, not `&AtomicRequest` (`executor/mod.rs:660-664`) | confirmed |
| B-6 | `IdentityAllocator` has no `Debug` impl (`owner/identity.rs:182`) | confirmed |
| B-8 | spec:324-325 — the exclusive lease exists "so no persistent generation can change before the live call", which the plan's "a commit is still admissible" test contradicts | confirmed |
| B-9 | the helper sets `out_fence_mask` bit *i* only when `holders[i] >= 0` (`executor/helper.rs:263-270`), and the executor's two consistency checks (`mod.rs:781-789`) accept a mask narrower than `slot_count`. A partial result therefore reaches `Accepted` | confirmed, and stronger than filed |
| B-11 | `pub(crate)` is crate-wide; the plan's "only `slot.rs` can call it" is false | confirmed |
| B-12 | every device is opened with `IncarnationId::first()` (`kms/backend.rs:875`), so incarnation cannot identify a device | confirmed |
| M-3 | `wait_readable` ignores its `_timeout` parameter and hard-codes 30 s (`executor/test_support.rs:778-780`) | confirmed — a latent 2a defect, not only a plan defect |
| M-5 | `from_reservation` yields four source lines, not three | confirmed |
| m-1 | `record_host_call_events` also stores into `host_call_events_for_tests`; and the helper overwrites `values[value_index]` with a **pointer to holder storage** before the ioctl (`helper.rs:216-220`), the kernel writing the fd into the holder | confirmed. The plan's explanation is wrong; its test is right and more load-bearing than stated — a misaimed `value_index` would overwrite an unrelated property's value with a pointer |

Not separately re-verified, because they are claims about the plan's own text
rather than about the tree: B-1, B-2, B-3, B-4, B-7, B-10, B-13, B-14, M-1, M-2,
M-4. Each was read against the plan and accepted.

**Classification for planning purposes.** Four of the fourteen blockers (B-5,
B-6, and the fixture halves of B-13 and B-14) plus M-1 are defects `rustc` names
in seconds; [[phase-c0-status]] records that spending review rounds on that class
is what made 2a's rounds 3 and 4 worthless. **The other ten are design defects a
compiler cannot see** — a validation request that fails its own re-scan, a
re-scan that checks containment where the spec demands equality, partial fence
output promoted to `Accepted`, a lease that never releases, forgeable proofs,
and routing that cannot tell two devices apart. That ratio is the opposite of
2a round 4's and is why this round is worth folding in whole.


## Incorporation audit

| Prior finding | Status | Evidence |
|---|---|---|
| None | N/A | This is the first review; check 1 was skipped as instructed. |

## Findings

### Blocking

#### B-1 — Every active `ValidationOnly` request fails its own out-fence re-scan

Task 5 correctly omits out-fences for validation at plan lines 1513-1516, but then unconditionally calls `verify_serialized` at lines 1549-1551. That method requires the discovered fence set to equal `expected_completion` at lines 631-636. Thus `a_validation_request_carries_no_out_fence_at_all` (lines 1345-1359), whose active CRTC makes `expected_completion == [1]`, returns `OutFenceCoverageDiffers` rather than succeeding.

This directly contradicts spec lines 320-325 and 2126: validation creates no out-fence. The re-scan needs class-aware fence expectations, or validation must have a separate persistent-list verification path.

#### B-2 — The purported exact re-scan accepts a different serialized closure

The plan promises equality at lines 31 and 137, matching spec lines 567-569. The shown implementation instead accepts any serialized subset:

- Plan lines 623-629 test only `serialized ⊆ recorded`.
- Plan line 643 explicitly declares containment intentional.
- `PropertyIds.active` is never used, so serialized `ACTIVE` values are not checked against `CrtcPower`.
- Closure and power are taken from parallel caller metadata (`entries` and `power`) rather than derived and validated against `serialized` (plan lines 1483-1491, 1506-1511).

An off-to-off CRTC present only in `entries`, or a serialized `ACTIVE=0` described as active, passes despite changing both the exact closure and off-to-off decision. This violates spec lines 538-550 and 580-592. The legitimate old-binding/detach problem requires richer retained metadata; it does not authorize replacing equality with unchecked containment.

#### B-3 — “Exactly one `OUT_FENCE_PTR`” is not enforced

The re-scan stores fenced CRTCs in a `BTreeSet` (plan lines 591-617), so duplicate `OUT_FENCE_PTR` properties collapse to one and pass. The builder also blindly appends a property without rejecting one already present in `SerializedObject.props` (lines 1523-1546).

Duplicating a serialized CRTC produces duplicate slots. `build_atomic_request` still returns success because it calls only `AtomicPropertyList::validate`; duplicate slot validation occurs later in `check_atomic_invariants` at `executor/protocol.rs:514-549`, when encoding can panic. Spec lines 564-566 require exactly one property, and construction—not IPC encoding—must reject the malformed request.

#### B-4 — `ResourceLedger` owns no resources and cannot perform the required cleanup

Plan lines 1028-1033 admit that the ledger merely names resources. `OwnedResource` contains only copyable numeric IDs (lines 1035-1042); it has no BO references, pins, descriptors, external-ownership state, framebuffer owners, or cleanup callbacks. Dropping a record therefore preserves none of the resources it supposedly uncertainty-owns.

Furthermore:

- `resolve_rejected` only clones names into `released_now` (lines 1098-1102).
- `retire_live` then clears/drops the record (lines 1969-1975, 1997).
- No owner path actually performs the promised release.
- `released_now()` is repeatable, so it cannot enforce exactly-once cleanup.

This violates spec lines 1687-1692 and 2127-2132, which require the record itself to retain every possible old/new resource and external ownership. This is not deferred call-site conversion; stage 2b expressly owns the resource-ledger model.

#### B-5 — Task 5 and Task 6 use the existing executor API with the wrong types

Two independent compile errors exist:

1. Task 5 imports `HostCallClass` through `kms::executor::protocol` at plan lines 1465-1468. It is defined in `executor/mod.rs:164-172`; `protocol.rs:10-16` only privately imports it and does not re-export it.
2. Task 6 passes `&AtomicRequest` to `executor.send` at plan lines 1866-1871. The real signature accepts `&HostCallRequest` (`executor/mod.rs:660-664`). The request must be wrapped in `HostCallRequest::Atomic`, a type Task 6 does not consume or mention.

Following the shown code does not compile.

#### B-6 — The shown record/owner definitions have move and trait errors

`CommitRecord::tombstone(&self)` destructures `self.state` by value at plan line 920. `RecordState` is not `Copy` (lines 187-191), so this attempts to move a field out of a shared reference.

Separately, `DeviceCommitOwner` derives `Debug` at plan lines 1795-1808, but it contains `IdentityAllocator`, which has no `Debug` implementation (`owner/identity.rs:182-189`).

Both prevent compilation as shown.

#### B-7 — Pre-dispatch executor refusals are falsely marked `Dispatched` and then stranded

`send_on` marks the record dispatched regardless of `KmsIoExecutor::send`’s result (plan lines 1859-1871). Its justification applies only to transport failure after the executor installs `InFlight`.

The real executor can return `Reaped`, `Stalled`, `AlreadyInFlight`, `ReservationMismatch`, or `BoundaryViolation` before installing `InFlight` or sending IPC (`executor/mod.rs:665-692`). Those paths queue no terminal event. The plan therefore leaves a slot-holding `Submitting` record marked dispatched forever even though no IPC crossed the uncertainty boundary.

Spec lines 612-617 and 678-686 distinguish cancellation/refusal before send from acceptance-unknown failure after send. `send_on` must classify pre-install failures separately, rather than assume every `Err` produced a queued outcome.

#### B-8 — The validation lease is neither exclusive nor resolvable

`DeviceSlot::reserve` ignores `validation`, and `acquire_validation` ignores `occupant` (plan lines 1238-1263). The test at lines 1157-1165 positively requires a live commit to remain admissible during validation, repeated as a claimed property at line 2297.

That contradicts spec lines 320-325: the exclusive validation lease exists so no persistent generation can change before the live call. Coexistence also drives the `AlreadyInFlight` stranding defect in B-7 once validation is actually sent.

The outcome path is independently broken: `apply_host_call_event` calls `is_current` before inspecting `pending_validation` (plan lines 1931-1949), while line 1999 defines `is_current` against the live record. A validation deliberately has no live record, so its outcome is rejected as uncorrelated and its lease never releases. The validation test at lines 1747-1756 cannot pass under the described implementation.

#### B-9 — Partial successful fence output is incorrectly promoted to `Accepted`

The real executor intentionally preserves `out_fence_mask` and can return `Accepted` with a partial mask (`executor/mod.rs:761-792`). Task 6 discards the mask and unconditionally calls `mark_accepted` for every `Accepted` outcome (plan lines 1960-1967).

Spec line 2129 requires one non-negative fd per expected CRTC and says any holder left at `-1` enters `CompletionUnknown`. That decision comes from the IPC reply and belongs at the stated 2b-i/2b-ii seam; querying fence status may be deferred, but detecting a missing returned fence may not.

The record also retains only `Vec<OwnedFd>` after consuming the request (plan lines 898-914), losing the slot-to-CRTC mapping and stable holder specification required by spec lines 1687-1691. Stage 2b-ii cannot reliably associate evidence with the CRTC whose slot produced it.

#### B-10 — Contradictory current outcomes leave a dispatched record nonterminal and unquarantined

`UnknownCause::ContradictoryEvidence` is declared at plan lines 177-185, but `ValidationAbandoned` and `ProbeAccepted` under a live atomic record merely log warnings (lines 1986-1991). The record remains `Submitting`, its ledger remains unquarantined, and no future event is guaranteed.

Spec `COMMIT-1`, lines 612-617, requires any dispatched result that is neither an explicit rejection nor a normally consumed success to become `CompletionUnknown`. The plan’s own interface advertises the correct cause but never produces it.

#### B-11 — `pub(crate)` constructors make both reservation proofs forgeable

Plan lines 1210-1219 add `pub(crate) fn from_reservation`, then claim only `slot.rs` can call it (lines 238-240 and 2296). Rust’s `pub(crate)` visibility permits every module in the crate to call it. The grep at lines 2253-2256 detects current textual uses but supplies no type-system guarantee.

An owner-side caller can mint `SubmittingProof` or `ValidationLease` without reserving anything, defeating the structural precondition behind spec `COMMIT-6` lines 678-686.

#### B-12 — Incarnation-only event routing cannot identify a device

Task 7 says `owner_for_event` resolves an event’s incarnation by scanning devices (plan lines 2220-2221). In the current tree every opened device is initialized with `IncarnationId::first()` (`kms/backend.rs:875-881`), and the executor exposes no device key in `HostCallEvent`.

Consequently, two devices can carry the same numeric incarnation—and independently seeded owners can also issue identical early identities. A scan routes the second device’s reply to the first matching owner. This violates the device-local identity model and spec `ID-1` at lines 407-409. Event collection must preserve the source device identity instead of attempting to reconstruct it from a device-scoped number.

#### B-13 — The promised owner API is not actually specified or produced

Task 6 declares `DeviceCommitOwner::new`, accessors, and `dispatch_validation` at plan lines 1599-1601, but the shown implementation supplies none of them. It instead introduces `begin_validation`, which is absent from the produced interface, and only prose mentions `dispatch_validation` at line 2001. There is no specified constructor capable of pairing an owner’s incarnation/lifecycle epoch with the executor.

Task 7 then says the real device uses “the device’s identity allocator” (line 2175), but `PlatformInitDevice` has only `key`, `device`, and `executor` (`kms/backend.rs:677-682`); no such allocator exists.

Following the task-local interfaces leaves Task 7 unable to construct the owner and the tests unable to call the promised accessors.

#### B-14 — Both integration layers’ proposed fixtures are unusable

The external integration test calls `single_active_crtc()` and `DeviceCommitOwner::new_for_tests()` at plan lines 2015-2064. Neither is produced by an earlier task. Task 5’s helper is local to its unit-test module (line 1445); Task 7 later proposes a different `#[cfg(test)] pub(crate) single_active_crtc_for_tests` (line 2200), which is unavailable to an external integration crate because the library is built without `cfg(test)` and crate-private items are inaccessible.

The backend routing tests also panic: Task 7 directs every test `KmsDevice` construction to use `owner: None` (line 2175). The existing stub fixture creates a normal test device and changes only `executor` to `Some` (`render/platform.rs:8867-8882`), while the new helpers immediately call `device.owner.as_mut().expect("an owner")` (plan lines 2182-2195).

The described three integration tests and two backend tests therefore cannot pass.

### Major

#### M-1 — The task interface declarations materially disagree with their bodies

Examples include:

- Task 2 omits `ResourceLedger`, `AtomicRequest`, `SubmittingProof`, and `OwnedFd`, despite storing or accepting all four at plan lines 816-822 and 892-914.
- Task 2’s produced-method list omits `commit_id`, `event_token`, ledger accessors, request transfer, and fence adoption, all used later.
- Task 2 claims to consume `ClockEpochId` (line 668) but never stores or uses one.
- Task 3 omits `quarantined`, although its own test calls it.
- Task 5 omits the produced `SerializedObject`.
- Task 6 advertises `dispatch_validation` but tests and code use `begin_validation`.
- The file structure promises a `Quarantine` type at line 55, but no task defines or produces it.

This defeats the plan’s stated rule that an implementer sees only their task.

#### M-2 — The fd-free test tee has no sound representation

The shown routing code clones `HostCallEvent` (plan lines 2204-2214), then prose correctly notes that `OwnedFd` prevents this and asks for an `identity_only()` method returning a “fd-free copy” (lines 2218-2219). No return type or semantics are supplied.

Keeping the queue typed as `Vec<HostCallEvent>` forces an accepted snapshot to fabricate `Accepted { out_fences: vec![], out_fence_mask: nonzero }`, an internally contradictory outcome. A separate observation type is needed if the queue must retain correlation/outcome metadata without owning fds.

#### M-3 — The claimed five-second integration-test timeout is actually thirty seconds

The plan repeatedly passes `Duration::from_secs(5)` to `wait_readable` (lines 2023, 2044 and 2123). The existing helper ignores its timeout parameter and hard-codes 30 seconds (`executor/test_support.rs:778-789`).

Failures can therefore block six times longer than the tests claim, degrading race diagnostics and making the plan’s timing assertions false.

#### M-4 — The unknown-reason loop is not exhaustive

Plan lines 1655-1660 manually list the four current `UnknownReason` variants, yet line 2294 claims adding a fifth variant cannot compile without classifying it. Adding another enum variant leaves this array and test perfectly compilable; the new reason is simply untested.

Use an exhaustive classifier/match shared with production code, or an enum iteration mechanism whose completeness is checked.

#### M-5 — The reservation-proof review grep has an impossible expected count

The plan adds two definitions named `from_reservation` and two calls—one for `SubmittingProof`, one for `ValidationLease` (lines 1210-1219 and 1238-1263). The grep at lines 2253-2256 therefore returns at least four source lines, not “exactly three,” even before comments or tests.

The stage’s own reviewability check reports a conforming implementation as defective.

### Minor

#### m-1 — Two descriptions of current behavior are inaccurate

Plan line 7 says backend events are logged and thrown away, but the current implementation also stores them in `host_call_events_for_tests` (`render/backend.rs:14755-14771`); Task 7 later acknowledges this queue.

Likewise, plan lines 1328-1331 say the helper writes the returned fd into `properties.values[value_index]`. The helper actually replaces the cloned value with a pointer to stable holder storage before the ioctl (`executor/helper.rs:198-220`); the kernel writes the fd into that holder (`executor/helper.rs:263-270`). The proposed index test remains useful, but its explanation is wrong.

## Notes on the rest

1. Incorporation audit: there was no prior review, so no incorporation claims were assessed.

2. Existing-code verification: I opened every explicit file anchor. The `KmsDevice` anchor at `render/platform.rs:1990-2000`, `record_host_call_events` at `render/backend.rs:14755-14761`, the `SequenceSupport` map at `render/backend.rs:1044`, and the one-time poll-source collection at `yserver-core/src/core_loop/run.rs:1045-1059` are accurate. The six existing `atomic_commit` sites and the cited executor outcome/proof/helper types also exist. False behavioral and type claims are reported above.

3. Cross-task consistency: I traced every declared Consumes/Produces interface through later tasks, including test-only helpers and external integration-test visibility. The material mismatches are B-5, B-13, B-14, and M-1.

4. Compile/test audit: I read every shown Rust block against the current executor signatures and trait implementations. The definite type/move/trait failures, logically impossible validation test, missing fixtures, fixture panic, and misleading timeout are reported above. No plan code exists in the tree to run directly.

5. Spec compliance: I checked the plan against sections 5, 6.3, 9, 10/10.2, and 18. The accepted-without-completion behavior and retained slot after acceptance-unknown are sound and were not reported. I also excluded every 2b-ii/2c/recovery/call-site-conversion item named by the user.

The repository-prescribed `review.sh` was attempted with the supplied context and exclusions. It reached the pinned invocation (`docs/superpowers/review/ @ 38aee673`, `gpt-5.6-sol`, medium, `codex-cli 0.152.0`) but the nested Codex client failed before review because this environment is read-only, so no findings artifact or valid dispatcher provenance block was produced.