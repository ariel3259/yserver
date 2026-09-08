## Verdict

6 blocking, 7 major, 2 minor

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `38aee673`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Provenance clarification:** This file was produced successfully by the outer
`review.sh` invocation (exit 0). The reviewer's final note refers to its redundant
attempt to invoke the instrument recursively inside the read-only reviewer.
That nested failure does not invalidate the outer run or the provenance above.
The raw reviewer text below is preserved, including that mistaken characterization.

**Author verification:** All 15 findings verified against the reviewed draft and
baseline `28b34dba`. In particular, spec §4.1 explicitly includes QUEUE_SEQUENCE;
the rejection stub emits the atomic reply family; and `run.rs` computes its
timeout before `before_block`. These are real defects, not deferred 2c scope.
The next plan revision addresses each finding; the second review must audit those
changes rather than treating this verification as proof that they are resolved.

## Incorporation audit

| Prior finding | Status |
|---|---|
| No prior review exists for this plan. | Skipped check 1 as instructed. |

## Findings

### Blocking

#### B-1 — `QUEUE_SEQUENCE` bypasses the mandatory executor

The plan explicitly keeps `queue_crtc_sequence` as a synchronous in-process ioctl boundary (`plan:166`, `plan:412`). The current wrapper calls `libc::ioctl` directly on the caller thread (`crates/yserver/src/drm/page_flip.rs:81-109`).

The spec assigns both `GET_SEQUENCE` and `QUEUE_SEQUENCE` to `KmsIoExecutor` (`spec:161-168`) and requires the X11 core never to execute potentially blocking KMS host calls (`spec:635-647`). Sequence arms are required to use that model (`spec:1772-1780`). An implementation following the plan remains spec-violating and can block the core.

#### B-2 — The prescribed probe-rejection test cannot produce a rejection

Task 2 requires `StubBehaviour::RejectWith(EOPNOTSUPP)` to leave the probe `Failed/Unresolved` (`plan:373`). That stub always sends `HostCallReply::Rejected` (`crates/yserver/src/kms/executor/test_support.rs:288-301`), whose protocol family is Atomic, not ClockProbe (`crates/yserver/src/kms/executor/protocol.rs:301-302`).

For a clock-probe request, `poll_reply` detects the family mismatch and returns `Unknown(MalformedReply)` (`crates/yserver/src/kms/executor/mod.rs:831-839`), not an explicit rejection. The shown test requirement cannot pass without changing the stub to emit `ProbeRejected` or adding a probe-specific behavior—neither is specified.

#### B-3 — A non-event qualification commit can qualify without successful GET_SEQUENCE

The plan gates clocks only for event-bearing work (`plan:135`) and allows an install/restore candidate with no Present event (`plan:322-324`). Its enumerated structural evidence contains CRTC-in-vblank, monotonic timestamps, and out-fence coverage, but omits successful current GET_SEQUENCE.

Consequently, a non-Present install can reach `Qualified` while every relevant clock remains `Unresolved`. The spec makes usable GET_SEQUENCE part of structural capability (`spec:389-395`, `spec:429-435`) and requires probe failure to close qualification as applicable, with no software substitute (`spec:1737-1753`, `spec:3065-3073`).

#### B-4 — Blocking qualification is required by a test but unrepresentable by the interfaces

Task 6 requires a “permitted blocking qualification fixture” (`plan:468`), but:

- `CompletionClass` has no blocking/nonblocking dimension (`plan:175-184`).
- `begin_install_restore` has no host-call-class or lifecycle-boundary argument (`plan:322`).
- The existing `begin` and `begin_validated` both hard-code `SeatActiveNonblock` (`crates/yserver/src/kms/owner/device.rs:159-166`, `260-277`).

The spec explicitly requires testing blocking qualification at an allowed cold-start/final-offline boundary (`spec:2892-2901`, `spec:2929-2936`). No earlier task produces an API capable of constructing that case.

#### B-5 — `before_block` can consume a fence wake after poll timeout was calculated

Task 7 says `before_block` performs canonical fence observation and then deadline processing (`plan:481`). In the actual loop, `backend.next_wakeup()` is consulted and `poll_timeout` fixed first (`crates/yserver-core/src/core_loop/run.rs:1121-1148`); only afterward is `backend.before_block()` called (`run.rs:1149-1156`).

If `before_block` observes the last fence, it closes/unregisters that descriptor and starts the shorter per-CRTC Present deadline. The aggregate readiness can disappear before the outer poll sees it, while the already-calculated timeout still reflects the old hardware deadline. The loop can therefore oversleep the new 50–500 ms Present deadline. This violates the exact timer start/expiry requirements (`spec:2184-2190`, `2201-2204`). Completion servicing must happen before timeout calculation, in the post-poll stateful hook, or force timeout recomputation.

#### B-6 — `LegacyDrainPermit` is consumed before any task defines or produces it

`LegacyDrainPermit` does not exist in the baseline. The plan describes it in prose (`plan:328`) and Task 3 immediately requires behavior “under `LegacyDrainPermit`” (`plan:412`), but neither Task 1 nor Task 2 produces the type, issuer, owner methods, or revocation transition. Task 7 later tests it (`plan:479`) as though it already exists.

This violates the plan’s own task-isolation rule: a Task 3 implementer has an undeclared type and no enforceable API for the critical non-coexistence/one-way-handover invariant.

### Major

#### M-1 — Qualification-cap evidence has no production wiring

The plan says the owner defaults to no caps and production installs evidence from discovery (`plan:324`), but Task 6’s files and interfaces define no capability-evidence type, setter, constructor argument, or platform handoff (`plan:446-448`). Task 7’s wiring steps likewise never populate it.

Today production constructs owners with only incarnation, lifecycle epoch, and topology generation (`crates/yserver/src/kms/render/platform.rs:2555-2571`). The only nearby driver-capability query is for timeline syncobj (`platform.rs:2366-2374`). Following the listed tasks leaves the qualification gate permanently closed in production, or forces an implementer to invent an unreviewed trust boundary.

#### M-2 — Raw-parser failure has no owner-side ingress interface

Task 7 must convert a final drain/parse error into the owner’s monotonic failure latch (`plan:479`, `482`). Task 4 produces `apply_drm_event` for successfully decoded records but declares no API for reporting a stream-level `MalformedEvent` (`plan:419-421`). The existing drain emits valid records and then returns `io::Error` for a malformed tail (`crates/yserver/src/drm/event_stream.rs:193-215`).

Task 7 therefore cannot use the “complete owner evidence APIs” it claims to consume; it must invent an undeclared failure-injection method.

#### M-3 — The sequence-arm interface leaves required identity and consumer signatures undefined

The normative text calls `take_sequence(token, time_ns, sequence, identity)` (`plan:168`) without defining the type or contents of `identity`. Task 3’s interface list omits signatures for `reserve_arm`, `arm_queued`, `take_sequence`, and the new consumer-bearing backend seam (`plan:408-413`).

This is material because the current backend traits accept only target arrays, not consumer identities (`crates/yserver-core/src/backend/trait_def.rs:975-991`), while current core call sites discard the mapping when constructing those arrays (`crates/yserver-core/src/core_loop/run.rs:1593-1606`, `1622-1639`, `1707-1720`). Deduplication plus exact cancellation cannot be implemented consistently without specifying the target/consumer association and return contract.

#### M-4 — The plan does not provide required requirement-group traceability

The spec requires every plan task and test name to cite at least one requirement group and says uncited behavior is not acceptance evidence (`spec:2677-2683`). Tasks 1–8 and their test descriptions contain no systematic `[ID-*]`, `[COMMIT-*]`, `[CAP-*]`, or `[MULTI]` citations. A few global prose references do not satisfy per-task and per-test traceability.

#### M-5 — The final gate omits the spec’s workspace-wide locked test command

Task 8 runs crate-specific tests (`plan:495-503`) but never runs:

```text
cargo test --all-targets --locked
```

That command is an explicit C.0 acceptance criterion (`spec:3832-3834`). The proposed gates can miss workspace crates, examples/benches, target-specific test compilation, and lockfile drift.

#### M-6 — Normalization tests omit normative boundary cases

The plan tests basic UST conversion and selected wrap cases (`plan:348-360`, `425`), but the required evidence also includes:

- `tv_usec = 999_999`;
- maximum `u32` seconds;
- the complete `0xffff_fffe → 0xffff_ffff → 0 → 1` extension sequence;
- resulting client-visible MSC/UST behavior.

These are explicitly required by `spec:3052-3060`. The listed tests do not prove the full arithmetic and projection contract.

#### M-7 — Task 8 misclassifies page-before-reply ordering as a contradiction

Task 8 asks for a “page-before-reply contradiction” (`plan:492`). A matching page before a successful reply is valid and must be staged (`plan:216`, `425`; `spec:2987-2990`). Only page evidence followed by explicit rejection is contradictory.

As written, the integration matrix directs an implementer to assert the opposite of the state-machine contract. It must say “page-before-rejection contradiction” or separately test valid page-before-success staging.

### Minor

#### m-1 — The `ClockEpochId` baseline claim is false

The plan says `ClockEpochId` may need `Ord` and `PartialOrd` added (`plan:96`). It already derives both (`crates/yserver/src/kms/owner/identity.rs:132-134`). This is harmless but violates the requested baseline-claim accuracy.

#### m-2 — `ProbeAccepted` does not carry “only” a sequence

The baseline table says `HostCallOutcome::ProbeAccepted` carries only `u64 sequence` (`plan:51`). It also carries `helper_duration_ns` and `round_trip_ns` (`crates/yserver/src/kms/executor/mod.rs:237-240`). The intended conclusion—that it carries no kernel timestamp—is correct, but the field claim is not.

## Notes on the rest

- Incorporation audit: there was no prior review, so check 1 was explicitly skipped.
- Existing-code verification: I opened every cited baseline anchor. The anchors at `backend.rs:1044`, `backend.rs:1514-1558`, `platform.rs:4150`, `platform.rs:4243`, and `core_loop/run.rs:1251` are accurate. The owner split API, 64-entry tombstone bound, fence slot mapping, device-keyed executor routing, and stable poll-source snapshot were also verified.
- Cross-task consistency: the clock, completion-state, fence-status, owner-event, and accepted-ledger types are otherwise consistently named. Public visibility for `DrmEventRecord` and the integration fixtures is explicitly addressed.
- Compile/test review: the shown clock arithmetic, deadline formulas, `SyncFileInfo` 56-byte layout, offsets, and Linux request value `0xc0383e04` are sound. The probe code example is type-correct once Task 1/2 APIs exist. B-2 and B-4 are the concrete impossible-test cases.
- Spec compliance: the independent Accepted/HardwareComplete/Presented milestones, complete per-CRTC fence set, page/fence separation, pre-accept staging, 64 commit tombstones, monotonic deadlines, empty-set nonqualification, accepted-ledger handoff, FreeBSD fail-closed query, and `NeverResource` production boundary are sound.
- I attempted the repository-mandated pinned reviewer (`docs/superpowers/review/ @ 38aee673`, `gpt-5.6-sol`, medium), but `codex exec` failed before review initialization because this session’s filesystem is read-only. No findings file or provenance-producing instrument result was created; this is a manual application of the frozen five-check brief and is not count-comparable to instrumented reviews.
