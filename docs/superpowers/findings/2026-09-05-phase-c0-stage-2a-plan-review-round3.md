# Phase C.0 Stage 2a plan — adversarial review, round 3

**Date:** 2026-09-05
**Subject:** `docs/superpowers/plans/2026-09-04-phase-c0-stage-2a-executor-substrate.md` revision 3 (`a85ec624`, 7 tasks, ~2750 lines)
**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `38aee673`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.152.0`.
Counts are comparable only to other reviews citing this same instrument SHA.
**Disposition:** open. Not executed, not delegated.

## Verification status

Spot-checked against the tree before filing. **Everything checked reproduces.**

- `spec:416-425` reads "Every executor request/reply and commit record carries
  the epoch", unqualified. B-2 is right: revision 3's "the handshake is not an
  executor request" was an invented exception, and the reviewer's counter holds
  — the incarnation exists before spawn and `LifecycleEpochId::first()` is
  available, so the handshake can carry both. The separate frame family was the
  correct half of the fix; the missing epoch was not forced by it.
- `serve_executor_loop` returns `Ok(())` on control-socket EOF
  (`helper.rs:83-88`). B-8 is right and it invalidates revision 3's PDEATHSIG
  reasoning entirely: when the handoff process `_exit`s, its control endpoint
  closes, the idle helper sees EOF and exits cleanly, and the lock releases.
  Skipping the death signal changes nothing. An idle helper is **not**
  observationally equivalent to a wedged one.
- `EventToken::for_tests(0x66)` at `plan:386` with the byte assertion at
  `plan:414`, against Task 1's new purpose-tag checking. B-3 reproduces.
- `send` never consults `HostCallPhase` (`plan:1777-1790`). B-4 reproduces.
- Task 6's commit omits `bin/yserver.rs` and Task 2's omits `executor/mod.rs`
  (`plan:2606-2611,941-944`). B-9 reproduces.
- The literal `LOCK_UN` remains in the unit-test comment at `plan:2293`. m-1
  reproduces, and **revision 3's self-review claim that it was resolved is
  false** — only the struct doc comment was changed, and the file was never
  re-grepped.

## On the numbers

| Round | Instrument | Blocking | Major | Minor |
| --- | --- | --- | --- | --- |
| 1 | 3-check brief, lost | 8 | 9 | 2 |
| 2 | 5-check brief, lost | 10 | 7 | 1 |
| **3** | **`review/` @ `38aee673`** | **10** | **2** | **2** |

**None of these rows is comparable to another.** Rounds 1 and 2 ran under briefs
that no longer exist. Round 3 is the first with a recorded instrument, so it is
a baseline, not a comparison; comparability begins with round 4.

The major count is additionally depressed by this round's `--context`, which told
the reviewer that six of round 2's majors are deliberately open and to classify
them NOT APPLIED without repeating them. That was the right instruction — they
are known — but it means 2 is not the number of major defects in the plan.

## What the character says

Round 2's blockers were mostly one structural gap: the host-call type model was
too narrow. Revision 3 widened it, and **most of round 3's blockers are
consequences of that widening**. B-2, B-3, B-5, B-6 and B-7 all trace to the
same change: the handshake moved but lost its epoch; purpose-tag checking landed
without reconciling the golden token that must survive it; the out-fence rule
was extended to one validation class and not the other; watchdog expiry was left
uniform across a class split that the spec says must not be uniform; and
`InFlight` was never widened to retain what reply validation now needs.

That is the monolith's failure shape at a smaller scale: a change applied at its
point of origin while its neighbours keep speaking the old language. The two
remaining blockers are of a different and simpler kind — declarations that never
reached a file list or a commit (B-1, B-9), and one fix that does not work
(B-8).

The design is not in question. Nothing here says a subsystem is unimplementable.

---

**Result:** 10 blocking, 2 major, 2 minor

## Incorporation audit

| Prior finding | Status | Audit |
|---|---|---|
| B-1 | PARTIAL | Task 2 recognizes that external integration tests require public test seams (`plan:346-352`), but the actual task files, shown declarations, commits, helper ordering, and later tests remain inconsistent. Task 3 still consumes helpers first produced by Task 4; Task 5 has undeclared helpers; Task 6 directly uses private lock/device APIs. See B-1. |
| B-2 | APPLIED | Task 1 imports `ClockProbeId`, produces `IdentityAllocator::at_limit_for_tests`, and requires purpose-tag checking (`plan:92-94,103,152-165`). Task 2 no longer calls the nonexistent `EventToken::from_raw`. A new incompatibility with the golden token remains; see B-3. |
| B-3 | TRADED | Removing `Ready` from `HostCallRequest` makes `correlation()` and `class()` total, but reclassifying the readiness exchange as a separate frame family does not satisfy the spec’s requirement that every executor request and reply carry the lifecycle epoch. See B-2. |
| B-4 | APPLIED | Clock probes now have `ClockProbeLease` and yield `HostCallOutcome::ProbeAccepted { sequence, .. }` (`plan:1214-1216,1641-1660,1783,1799`). |
| B-5 | TRADED | Four classes now exist, including cold/offline validation, but the encoder/decoder and phase rules still permit spec-invalid uses of them. See B-4 through B-6. |
| B-6 | APPLIED | Task 6 now deletes `impl Drop for DeviceLock` before moving `self.file`, eliminating E0509 (`plan:2459-2472,2512-2516`). |
| B-7 | TRADED | The binary entry-point edit and an unsupervised spawn mode are described, but the parent-death test’s idle helper exits on control-socket EOF, and the binary edit is omitted from the task’s commit. See B-8 and B-9. |
| B-8 | APPLIED | `pipe_pair` now makes the read end nonblocking, and `pipe_is_at_eof` handles `WouldBlock` (`plan:1512-1525,1858`). |
| B-9 | APPLIED | The two-dispatch boundary test now uses `RejectWithRepeatedly` (`plan:1597-1605,1855`). |
| B-10 | APPLIED | The sleep/wait greps are narrowed to the host-call production files and `mod.rs` respectively (`plan:2639-2662`). |
| M-1 | NOT APPLIED | Explicitly retained as open at `plan:2747`. |
| M-2 | NOT APPLIED | Explicitly retained as open at `plan:2748`. |
| M-3 | NOT APPLIED | Explicitly retained as open at `plan:2749`. |
| M-4 | NOT APPLIED | Explicitly retained as open at `plan:2750`. |
| M-5 | APPLIED | The test’s claim is narrowed to freshly recomputed sources, and one-shot core registration is recorded as a 2b prerequisite (`plan:2003-2016,2191-2198,2721`). |
| M-6 | NOT APPLIED | Explicitly retained as open at `plan:2752`. |
| M-7 | NOT APPLIED | Explicitly retained as open at `plan:2753`. |
| m-1 | TRADED | The struct documentation avoids the literal token, but the prescribed unit-test comment still contains `LOCK_UN`, so Task 7’s “exactly one” grep still fails. See m-1. |

## Findings

### Blocking

#### B-1. The external-test API and helper dependency graph still cannot compile

Revision 3 does not actually resolve the prior cross-task test-surface blocker.

Task 3 calls `dispatch_and_wait_for_tests`, `spawn_real_helper_for_tests`, `three_property_request_for_tests`, `invalid_object_request_for_tests`, `spawn_scripted_helper_for_tests`, `request_with_slots_for_tests`, and `small_atomic_request_for_tests` (`plan:1024-1081`). Its `Produces` list contains none of them (`plan:959-964`). `dispatch_and_wait_for_tests` and `small_atomic_request_for_tests` are first produced by Task 4 (`plan:1219-1229`), after Task 3’s required passing gate at `plan:1178-1181`. None exists in the current tree.

Task 5 likewise uses `platform_with_stub_executors_for_tests`, `reap_every_executor_for_tests`, `send_never_answered_host_call_for_tests`, `backend_with_stub_executor_for_tests`, `send_rejected_host_call_for_tests`, `wait_executor_readable_for_tests`, and `drained_host_call_events_for_tests` (`plan:1990-2087`), but declares none under `Produces` (`plan:1893-1900`).

Visibility is internally contradictory:

- Task 2 promises `#[doc(hidden)] pub` protocol and identity surfaces (`plan:346-352`), but the shown implementation declares the constants and `HostCallCorrelation` as `pub(crate)` (`plan:826-843`).
- Task 2’s file list and commit omit `kms/mod.rs`, `owner/mod.rs`, `owner/identity.rs`, and `owner/lifecycle.rs`, even though those are the files whose visibility it says it widens (`plan:325-350,941-944`).
- Task 4’s external test calls `executor.state()`, but the existing method is `pub(crate)` (`crates/yserver/src/kms/executor/mod.rs:247-251`), and Task 4 never says it becomes public.
- The same test imports only `test_support::{self, StubBehaviour}` and then calls test-support functions unqualified (`plan:1242-1258,1332-1336`). `ClockProbeLease` is likewise used without being imported (`plan:1594,1647-1652`).
- Its `AsRawFd` and `OwnedFd` imports are unused in the shown file (`plan:1488-1509`), which fails the required `-D warnings` gate.
- Task 6’s external test directly uses `DrmDeviceKey`, `may_install_state`, `DeviceLock::into_inheritable`, `IncarnationId::first`, and `executor_executable` (`plan:2335-2448`). Those are currently crate-private: `DrmDeviceKey` and its fields at `platform/drm.rs:35-37`, `DeviceLock` and `may_install_state` at `device_lock.rs:117-129,194-198`, `IncarnationId::first` at `identity.rs:9-15`, and `executor_executable` at `executor/mod.rs:505`.

It also calls `DeviceLock::duplicate_for_tests()` twice (`plan:2295-2312`), but that method does not exist and is absent from Task 6’s `Produces` list and implementation.

#### B-2. Moving the handshake to another enum does not satisfy ID-3

The plan declares that the startup handshake “is not an executor request” because it uses a separate frame family (`plan:29,922-928`). That is an invented exception to the authoritative rule: “Every executor request/reply … carries the epoch” (`spec:416-425`).

`HandshakeRequest` carries nothing, and `HandshakeReply` carries only `helper_pid` (`plan:339,537-550`). It is still a request sent to the executor and a reply returned by it. Separating it from `HostCallRequest` fixes the total-accessor problem but not the wire invariant.

The stated rationale that no epoch exists yet is not forced by the tree or spec. Task 5 already creates an `IncarnationId` before spawn (`plan:2131-2139`), and `LifecycleEpochId::first()` is available from Task 1. Following the plan produces executor traffic that violates `spec:422-425`.

#### B-3. Task 1’s tag validation makes Task 2’s golden request invalid

Task 1 changes `EventToken::from_user_data` to reject values lacking the event purpose tag (`plan:258-276`). The current `EventToken::for_tests(raw)` stores the raw value unchanged (`identity.rs:66-70`).

Task 2 nevertheless constructs its golden request with `EventToken::for_tests(0x66)` and asserts that byte-for-byte value at the wire offset (`plan:379-414`). It later decodes a frame built from the same golden request and expects success (`plan:439-452`).

`0x66` has no event purpose tag in bits 62–63. A decoder using the newly required `from_user_data` rejects it. Changing `for_tests(0x66)` to add the tag would instead break the fixed-offset assertion expecting exactly `0x66`. The shown tests cannot all pass.

#### B-4. `send` permits seat-active blocking ioctls and the wrong validation watchdog

The runtime boundary check protects only `dispatch_blocking_at_boundary` (`plan:1231-1235,1817`). The ordinary asynchronous `send` accepts:

- `ColdStartOrOfflineBlocking` with `SubmittingProof`;
- `ColdStartOrOfflineValidation` with `ValidationLease`;

without consulting `HostCallPhase` (`plan:1777-1785`).

Therefore a seat-active caller can invoke `send` with a blocking atomic request, or label seat-active validation as cold/offline and receive a 30-second watchdog. That violates the restriction that every live seat-active commit uses `NONBLOCK` and blocking atomics occur only at cold-start/final-offline boundaries (`spec:635-653`), plus the validation watchdog split at `spec:320-329`.

The only boundary test calls the blocking wrapper (`plan:1596-1639`); it does not exercise direct `send`, so it cannot catch this hole.

#### B-5. Normal encoding permits validation requests with out-fence payloads

The spec says every `ValidationOnly` request creates no out-fence (`spec:320-325,2126`). The plan’s normal encoder calls only `AtomicPropertyList::validate` and `assert_class_agreement` (`plan:897-905`). Neither described operation rejects an out-fence slot for validation.

The decoder’s explicit payload rule covers only `SeatActiveValidation` (`plan:915-918`), not `ColdStartOrOfflineValidation`. The sole test also uses only the seat-active variant and deliberately bypasses encoder validation (`plan:768-783`).

Thus:

- the normal encoder can emit either validation class with out-fence slots;
- cold/offline validation can pass the described decoder with such slots;
- the exit claim that encoder and decoder both reject class/flag/payload disagreement (`plan:2706`) is false.

#### B-6. Validation watchdog expiry is incorrectly classified as acceptance-unknown hardware state

Task 4 routes every expired in-flight call through `terminalize_unknown(WatchdogExpired)`, enters `Stalled`, and describes watchdog expiry as an acceptance-unknown path (`plan:1316-1319,1756-1774,1801`).

That behavior is not valid for `ValidationOnly`. The spec explicitly says validation timeout invalidates the candidate snapshot but “never classifies hardware state as acceptance-unknown because no live mutation was requested” (`spec:320-329`).

No validation-specific timeout outcome or test exists. `HostCallOutcome::Unknown` and the generic terminalization language direct an implementer to apply the live-mutation uncertainty model to validation.

#### B-7. The asynchronous state discards data required to validate replies

Task 3 requires reply validation against the original request’s `slot_count` (`plan:1151-1159`) and claims masks outside that request’s slot table are malformed. Task 4’s definitive `InFlight` structure retains only correlation, class, timestamps, and terminalization state (`plan:1735-1746`). It retains neither the request nor its slot count.

Consequently `poll_reply` cannot calculate the required `valid_mask`. The pure `decode_reply` also cannot know the original request’s slot count, despite the claim at `plan:920`.

The reply model has a second family-consistency hole. Every reply variant accepts the general `HostCallCorrelation` enum (`plan:335-338,488-513`), while `poll_reply` checks only correlation equality (`plan:1795-1799`). A probe request can therefore receive `HostCallReply::Accepted` carrying its matching `ClockProbe` correlation and be exposed as an atomic `Accepted`; conversely an atomic request can receive `ClockProbe` with its matching atomic correlation and become `ProbeAccepted`. No request-kind/reply-kind check is specified.

This fails the requirement that the executor return the complete typed result for the corresponding atomic or clock-probe message (`spec:635-645`).

#### B-8. The parent-death test’s “orphaned” helper exits when its parent dies

Skipping `PDEATHSIG` does not keep the helper alive. The handoff subprocess owns the parent endpoint of the executor control socket. `_exit` closes that endpoint despite `mem::forget` (`plan:2579-2594`).

The real helper is idle in `serve_executor_loop`; on control EOF it immediately returns successfully (`crates/yserver/src/kms/executor/helper.rs:79-99`). Its inherited lock descriptor then closes, releasing the lock before the test’s assertion at `plan:2411-2415`.

An idle helper without `PDEATHSIG` is therefore not observationally equivalent to a helper stuck in an uninterruptible ioctl. To model that state, the test helper must already be inside a non-returning host call or otherwise retain the lock despite control EOF.

There is also a latent process-harness problem if the helper is changed to remain alive: `spawn_internal` redirects stdout but leaves stderr inherited (`executor/mod.rs:641-648`). `Command::output()` captures the handoff process’s stderr (`plan:2399-2404`), and a surviving grandchild inheriting that pipe can prevent `output()` from observing EOF.

#### B-9. The required handoff entry-point edit is omitted from the commit

Task 6 correctly adds `crates/yserver/src/bin/yserver.rs` to its file list and explicitly shows the dispatch block (`plan:2263-2269,2547-2561`). But its commit stages only executor, helper, backend, and test files (`plan:2605-2611`); `bin/yserver.rs` is omitted.

The current binary has no handoff dispatch (`crates/yserver/src/bin/yserver.rs:6-44`). Following the task’s commit instructions leaves the committed implementation unwired, reproducing the essential operational half of prior B-7.

Task 2 has the same commit-discipline problem on a smaller scale: it says `executor/mod.rs` must change in that task to keep the intermediate build working (`plan:328`), but its commit stages only protocol and transport (`plan:941-944`).

#### B-10. `OpenError` and the production lock-refusal API have no coherent implementation path

Task 6 promises `OpenError::LockUnavailable { device, recorded_holder }` and `installs_attempted()` (`plan:2277-2279`) and later says production `platform_init` returns that error (`plan:2537-2540`). No file is assigned to define `OpenError`, no other variants or `From<io::Error>` behavior are specified, and no signature migration is described.

The current `platform_init` returns `io::Result<PlatformInit>` and has many unrelated `io::Error` paths (`crates/yserver/src/kms/backend.rs:829-882`). Changing it to a new error requires corresponding changes to callers and conversions that the plan neither lists nor defines.

The proposed `open_kms_device_for_tests(&DrmDeviceKey)` is also not a thin entry point over `platform_init`: the production function accepts device paths, and a fabricated major/minor key provides no path to open (`plan:2279,2430-2436`; `backend.rs:829-844`). An implementer cannot realize the declared helper or production error behavior from the stated interface.

### Major

#### M-1. The “drains every pending event” test queues only one event

`on_executor_readable_drains_every_pending_event` sends one rejected host call and asserts that one event was recorded (`plan:2079-2087`). A broken implementation that calls `poll_reply` exactly once passes.

The production requirement is a loop to exhaustion because the poll source is edge-triggered (`plan:2173-2189`). The test needs at least two queued replies before one hook invocation to distinguish draining from single-read behavior.

#### M-2. The public-visibility grep cannot prove `#[doc(hidden)]`

Task 7 runs:

```bash
rg -n 'pub [a-z]' crates/yserver/src/kms/executor/protocol.rs
```

and claims this verifies that every widened item carries `#[doc(hidden)]` rather than bare `pub` (`plan:2666-2679`).

The matching declaration line is identical in both cases; `#[doc(hidden)]` is on a preceding line. The grep neither fails on bare `pub` nor associates an attribute with the following item. It cannot enforce the invariant it claims to check.

### Minor

#### m-1. The `LOCK_UN` gate still has more than one match

Task 7 expects exactly one `LOCK_UN`, inside `release_explicitly` (`plan:2666-2675`). The prescribed unit-test comment also contains the literal token:

```rust
// The bug that makes the naive handoff wrong: LOCK_UN through any
```

at `plan:2292-2294`, in the same `device_lock.rs` file. The self-review’s claim that revision 3 resolved this is overstated (`plan:2754`).

#### m-2. Two existing-code anchors do not identify the stated locations

`plan:2129` anchors “immediately after `primary_device_key_from_fd`” at `kms/backend.rs:844`, but line 844 is the device open; the identity call is at `crates/yserver/src/kms/backend.rs:855-856`.

`plan:2549` says `bin/yserver.rs:6-36` contains all four existing internal dispatch blocks. The internal probe block starts at line 36 and continues through line 44 (`crates/yserver/src/bin/yserver.rs:36-44`). The same paragraph calls the proposed handoff “a fourth block,” although it is a fifth dispatch block.

## Notes on the rest

I performed all five requested checks:

1. **Incorporation audit:** Classified all 18 findings from round 2. I treated the explicitly retained M-1, M-2, M-3, M-4, M-6, and M-7 as NOT APPLIED without repeating them.
2. **Existing-code verification:** Opened every cited Stage 1 surface: executor state and spawn paths, protocol, helper serve loop, stub behavior, identity allocator, device lock, binary dispatch, platform initialization/device structures, KMS/core poll sources, wakeup calculation, backend trait/implementations, core dispatch tests, candidate discovery, and the compile-fail runner. Apart from the anchors in m-2 and false behaviors reported above, the cited locations resolve correctly.
3. **Cross-task interface consistency:** Traced every `Consumes`/`Produces` declaration, including external-test visibility, test-only helpers, commits, correlation shapes, reservations, and task ordering. The failures are concentrated in B-1, B-7, B-9, and B-10.
4. **Shown code/tests:** Checked name resolution, privacy, imports, enum-family matching, retained state, descriptor lifetime, `Drop` field moves, subprocess EOF behavior, blocking reads, one-shot stubs, and source gates. Definite compile/test failures are reported above.
5. **Spec compliance:** Checked the fixed executor model, `ID-3`, `COMMIT-5`, `COMMIT-6`, `COMMIT-7`, validation semantics/watchdogs, clock-probe ownership/results, synchronization ownership, and section 18’s 2a boundary. I did not report any explicitly excluded 2b/2c work or core poll-source churn.

The following areas are sound:

- Atomic/probe head sizes, documented offsets, body arithmetic, and the 140-byte golden atomic frame arithmetic are correct.
- Hostile count checks precede allocation and use bounded, checked size calculations.
- Duplicate out-fence slot indices and duplicate CRTC ids are explicitly rejected.
- Holder storage reaches final length before addresses are installed, so moving `PreparedAtomic` does not move its heap allocations.
- Clock probes now have a non-commit reservation and preserve their returned sequence.
- The pipe EOF helper is genuinely nonblocking.
- The repeated-rejection stub fixes the former two-request/one-shot contradiction.
- Deleting `DeviceLock`’s `Drop` implementation correctly fixes both flock semantics and E0509.
- Parent-only nonblocking socket configuration is consistent with the existing blocking helper receive loop.
- The actual `BackendFdKind`, trait hook, core dispatch, `poll_fds`, and deadline chain are the correct production integration surfaces.
- The executor deadline is correctly kept outside `allow_kms_timers`.
- The stage-1 `SequenceSupport` retention and the owner/call-site/admission/completion omissions match the stated 2a/2b/2c boundary at `spec:3865-3880`.