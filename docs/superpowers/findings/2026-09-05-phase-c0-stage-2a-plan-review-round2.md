# Phase C.0 Stage 2a plan — adversarial review, round 2

**Date:** 2026-09-05
**Subject:** `docs/superpowers/plans/2026-09-04-phase-c0-stage-2a-executor-substrate.md` revision 2 (`4189656e`, 7 tasks, ~2470 lines)
**Reviewer:** `codex exec --sandbox read-only`, single pass
**Result:** 10 blocking, 7 major, 1 minor
**Disposition:** open. Not executed, not delegated.

## Verification status

Every blocking finding was checked against the tree by the plan's author before
this document was filed. **All ten reproduce.** None is refuted, which is itself
a departure from the previous rounds, where reviewer claims about existing code
sometimes did not survive checking. Spot-checks recorded:

- `EventToken` has only `as_user_data`/`from_user_data`/`for_tests`, no
  `from_raw` (`identity.rs:55-72`). B-2 holds.
- `StubBehaviour::RejectWith` answers exactly one request, then performs a
  single blocking one-byte read and returns (`test_support.rs:213-231`). B-9
  holds.
- `bin/yserver.rs:6-36` dispatches the stub, re-exec executor, lock holder and
  internal probe. Nothing calls a handoff entry point. B-7 holds.
- `spawn_internal`'s `pre_exec` arms `PR_SET_PDEATHSIG`/`PROC_PDEATHSIG_CTL`
  with `SIGKILL` (`executor/mod.rs:554-580,649-655`). B-7's second half holds.
- `poll_fds()` is called once at `run.rs:1045-1059` and the resulting
  `backend_poll_sources` is never refreshed. M-5 holds.
- `IncarnationId::next` and `ClockEpochId::next` are unchecked `self.0 + 1`
  (`identity.rs:18-21,103-106`). M-7 holds.

## On the numbers, and on the instrument

| Plan | Tasks | Blocking | Per task |
| --- | --- | --- | --- |
| Stage 1 | 14 | 2 | 0.14 |
| Stage 2 monolith, rev 1 | 21 | 24 | 1.14 |
| Stage 2 monolith, rev 2 | 23 | 26 | 1.13 |
| Stage 2a, rev 1 | 6 | 8 | 1.33 |
| **Stage 2a, rev 2** | **7** | **10** | **1.43** |

**Read this table with the confound stated first.** The brief was not held
constant. Round 1's brief asked for three things: cross-task interface checking,
verification of claims about existing code, and an incorporation spot-check.
This round's brief asked for five, including a finding-by-finding incorporation
audit of all 19 prior items, an explicit out-of-scope list, and a fixed output
format. It is a harsher instrument, so the count is not comparable to round 1's
and no conclusion about the plan getting worse can be drawn from `1.33 → 1.43`.
This is the second time the instrument has drifted between rounds. Until the
brief is frozen, these counts measure the reviewer at least as much as the plan.

**What the character says, which is the part worth keeping.** Round 1 rejected
structure: the event-loop integration named types that did not exist, the
async API had no consumer, `Drop` still blocked. This round confirms all of
that repaired — B-2, B-3, B-4, B-5, M-3, M-4, M-5, M-8 are APPLIED — and the
new blockers split into two groups:

- **Seven are local and fixable in place**, with no redesign: one Rust rule
  (`E0509`), one blocking read, one stub reused across two requests, one grep
  scope, one unwired entry point, missing imports and a missing constructor.
- **Three are one coherent design gap**, and it is a gap the rewrite introduced
  while fixing the old ones: the host-call type model is too narrow. `Ready`
  cannot satisfy the total `correlation()`/`class()` accessors the same task
  promises, a clock probe has no legal reservation and its `sequence` result
  is dropped on the floor before 2b can read it, and there is no
  `ColdStartOrOfflineValidation` class even though `spec:320-329` requires
  cold-start/offline validation to carry the 30-second watchdog.

Plus one architectural finding neither round had reached before: **core poll
sources are registered once and never refreshed** (M-5), so a reaped executor
stays registered and 2b's replacement executor cannot enter the running poller
through `poll_fds()` at all.

The honest reading is that the rewrite did what rewriting is for — it closed
the structural objections — and then under-specified a type model in the new
material. That is a different failure from the monolith's, where corrections
regressed. Nothing here asks for a third rewrite of the same tasks.

---

## Incorporation audit

| Prior finding | Status | Audit |
|---|---|---|
| B-1 | APPLIED | Task 1 now produces `ClockProbeId` (plan:88-90), and the probe tuple includes `topology_generation` (plan:772-780). |
| B-2 | APPLIED | Task 5 now changes the real `BackendFdKind`, core dispatch, `KmsPlatform::poll_fds`, and `next_wakeup` chain (plan:1722-1739, 1934-2057). Its reaped-source test still does not prove production deregistration; see M-5. |
| B-3 | APPLIED | `terminalize_unknown` retains `in_flight`, enters `Stalled`, emits once, and starts reaping for all named unknown paths (plan:1606-1642). EOF-after-watchdog and retry-after-malformed are covered. |
| B-4 | APPLIED | Task 4 removes synchronous `Child::wait()` from `Drop` and uses nonblocking status checks (plan:1659-1686). |
| B-5 | APPLIED | Reaped executors now refuse sends (plan:1345-1365), and the late-reply helper explicitly ignores termination (plan:1442-1468, 1691-1695). |
| B-6 | TRADED | The ownership model is repaired with `InheritableDeviceLock`, readiness, and fields on production devices, but the shown consuming conversion cannot compile because `DeviceLock` implements `Drop`; the subprocess is also unwired. See B-6 and B-7. |
| B-7 | TRADED | Flag/class agreement and `ValidationLease` are added, but the class model cannot represent cold/offline validation and clock probes still require the wrong reservation. See B-4 and B-5. |
| B-8 | APPLIED | Named-variant construction and holder access are corrected (plan:470-488, 1028-1035). New unrelated compilation failures remain. |
| M-1 | TRADED | The impossible parent-side ledger is removed and replaced by a kernel-observed pipe, but the replacement test blocks on its first negative EOF check. See B-8. |
| M-2 | PARTIAL | The scripted errno assertion was replaced, but `/dev/null` returning `ENOTTY` still cannot prove the materialized arrays reached the ioctl. See M-1. |
| M-3 | APPLIED | Caps, preallocation checks, checked arithmetic, and duplicate slot/CRTC rejection are specified in the correct order (plan:828-839). |
| M-4 | APPLIED | The golden tests now check every documented atomic and probe head/body field (plan:326-436). |
| M-5 | APPLIED | The meaningless external compile-fail case was removed with an accurate privacy explanation (plan:241-245). |
| M-6 | PARTIAL | The helper-death race now accepts both unknown reasons, and ceilings increased to two seconds. The wall-clock assertions remain scheduler-sensitive and therefore do not justify the self-review’s claim. See M-3. |
| M-7 | TRADED | A real parent-process-death test was added, but ordinary executor spawning arms `PDEATHSIG=SIGKILL`, so the idle helper used by that test dies with its parent. See B-7. |
| M-8 | APPLIED | Only the parent endpoint becomes nonblocking after socketpair construction (plan:1645-1655). |
| M-9 | NOT APPLIED | The new self-review again overstates incorporation: property submission, timing robustness, parent-death behavior, and descriptor exactness remain unproven or broken (plan:2470-2473). |
| m-1 | APPLIED | Task 1 now uses one Cargo filter per command (plan:190-199, 257-263). |
| m-2 | TRADED | Behavioral tests now accompany the greps, but the final grep expectations contradict helper/test code the plan itself requires. See B-10 and m-1. |

## Findings

### Blocking

#### B-1. The external integration tests have no compilable API and depend on unproduced helpers

Tasks 3, 4, and 6 place tests under `crates/yserver/tests`, meaning they compile as external crates (plan:939-1007, 1145-1572, 2160-2258). Yet the request and identity types needed by those tests remain inside private modules: `protocol` is `pub(crate)` (`crates/yserver/src/kms/executor/mod.rs:23-27`), `owner` is `pub(crate)` (`crates/yserver/src/kms/mod.rs:18`), and Stage 1 identities are themselves `pub(crate)` (`identity.rs:9-10,33-34,52-53,94-95`).

The shown tests call `KmsIoExecutor::send` with `HostCallRequest`, `AtomicRequest`, and private identity values. Widening only `send` would trigger a private-interface error; widening the whole wire/owner surface is neither declared nor justified.

The cross-task helper inventory is also incomplete:

- Task 3 calls `dispatch_and_wait_for_tests`, but Task 4 is where it is first said to be rewritten/produced (plan:953, 971, 985-995 versus 1135).
- `small_atomic_request_for_tests`, `drive_send_failure`, `drive_helper_exit`, `drive_malformed_reply`, `drive_watchdog_expiry`, `fence_returning_request_for_tests`, `blocking_atomic_request_for_tests`, and `validation_request_for_tests` are used throughout Task 4 but appear in no preceding `Produces` section.
- Task 5 similarly assumes `platform_with_stub_executors_for_tests`, `reap_every_executor_for_tests`, `backend_with_stub_executor_for_tests`, and several send/wait helpers without producing them (plan:1828-1921).
- Task 6 promises external helpers taking `DrmDeviceKey`, but that type lives under the crate-private `platform` module (`crates/yserver/src/lib.rs:9`).

An implementer following the declared interfaces cannot make the intermediate Task 3 gate—or the final suite—compile.

#### B-2. Task 1 and Task 2 contain immediate compile and behavior failures

The lifecycle test imports only `LifecycleEpochId` and `LifecycleTransitionId` (plan:99) but uses `ClockProbeId` at plan:123 and 136-139. That name is not in scope.

Task 2 constructs `EventToken::from_raw(...)` (plan:337), but the existing type has only `as_user_data`, `from_user_data`, and `for_tests` (`identity.rs:55-70`). No earlier task produces `from_raw`.

Task 1’s purpose-separation test also cannot pass with the described changes. It expects:

```rust
EventToken::from_user_data(arm.as_user_data()).is_none()
```

(plan:155-161), while the actual decoder accepts every nonzero value without checking the purpose tag (`identity.rs:62-64`). Task 1 only instructs changes to allocation and `COUNTER_MASK` handling (plan:247-251), not to either token decoder.

`IdentityAllocator::at_limit_for_tests()` is likewise used at plan:148 without being declared in the interface or implementation instructions.

#### B-3. `Ready` makes the request accessors internally impossible and violates the plan’s own ID-3 constraint

Task 2 promises `HostCallRequest::{correlation, class}` accessors (plan:300) while also adding `HostCallRequest::Ready`, which intentionally has neither correlation nor class (plan:491-501, 843).

Task 4 assumes `request.correlation()` returns a bare `HostCallCorrelation`:

```rust
let sent = request.correlation();
assert_eq!(correlation, sent);
```

(plan:1191-1200). It therefore cannot return `Option`. But no total bare-return accessor can handle `Ready`. The same contradiction applies to `class()`, which `send` uses for watchdog and reservation matching.

The handshake also contradicts both the plan’s global statement that every executor request/reply carries the lifecycle epoch (plan:27) and ID-3’s normative requirement at spec:422-425. `Ready` explicitly carries none (plan:491-501, 843).

#### B-4. The asynchronous API discards clock-probe results and has no legal reservation for a probe

`HostCallReservation` has only `Submitting` and `Validation` (plan:1129). The specified matching rule requires `Submitting` for both non-validation classes (plan:1627). A clock probe therefore either consumes a `SubmittingProof` or cannot be sent.

That violates COMMIT-5: a clock probe “owns no commit resources” (spec:642-645). The plan acknowledges that 2b will add a non-atomic reservation later (plan:2462), leaving the 2a API knowingly incapable of correctly sending one of its own `HostCallRequest` variants.

The result is also lost. `HostCallReply::ClockProbe` carries `sequence` (plan:460-463), but the produced `HostCallOutcome` variants contain no clock sequence (plan:876-879), and `HostCallEvent` merely wraps that outcome (plan:1598-1603). Stage 2b is told to consume `HostCallEvent` (plan:2461), through which it cannot recover the probe result required to select `KernelSequence`.

#### B-5. The host-call class model forbids a normative validation mode

The plan defines only:

- `SeatActiveNonblock`
- `SeatActiveValidation`
- `ColdStartOrOfflineBlocking`

(plan:88-90), and explicitly rejects `TEST_ONLY` on `ColdStartOrOfflineBlocking` (plan:673-676, 836).

The spec requires two validation watchdog modes: seat-active validation uses two seconds, while cold-start/offline validation uses thirty seconds (spec:320-329). The plan’s global rule incorrectly assigns two seconds to all validation (plan:30). There is no `ColdStartOrOfflineValidation`, so an engineer cannot express a required spec-valid request.

#### B-6. `DeviceLock::into_inheritable` cannot move `self.file` because `DeviceLock` implements `Drop`

The shown implementation does this:

```rust
pub(crate) fn into_inheritable(self) -> InheritableDeviceLock {
    InheritableDeviceLock { fd: OwnedFd::from(self.file), ... }
}
```

(plan:2292-2300), while retaining a `Drop` implementation for `DeviceLock` (plan:2270-2280).

Rust forbids moving an individual field out of a type that implements `Drop` (`E0509`). The plan explicitly rejects `ManuallyDrop` and provides no `Option<File>`/`into_inner` representation (plan:2317). Following the shown code produces a non-compiling task.

#### B-7. The parent-death handoff is unwired and conflicts with the existing `PDEATHSIG` spawn path

Task 6 adds `run_lock_handoff_if_requested` but does not modify `crates/yserver/src/bin/yserver.rs` in its file list or commit (plan:2089-2094, 2331-2360, 2374-2380). The binary currently calls only the stub, executor, and old lock-holder entry points (`bin/yserver.rs:6-34`). Invoking `LOCK_HANDOFF_ARG` therefore reaches ordinary argument parsing rather than the new subprocess function.

Even if wired, the test’s process model contradicts Stage 1. `spawn_internal` arms `PR_SET_PDEATHSIG`/`PROC_PDEATHSIG_CTL` with `SIGKILL` before exec (`executor/mod.rs:554-580,649-655`). Task 6 says its handoff mirrors that spawn path (plan:2321-2326). When the handoff process `_exit`s, the idle helper is killed and releases the lock; the test expects it to remain orphaned and keep the lock held (plan:2201-2235).

Thus the claimed M-7 repair cannot reliably pass or demonstrate the uninterruptible-helper threat model.

#### B-8. The pipe EOF tests block before reaching their assertions

`pipe_is_at_eof` performs a normal blocking `read` (plan:1418-1421). The pipe is never made nonblocking.

In `an_accepted_reply_adopts_its_out_fence...`, the first call is intentionally made while a writer remains open and no data exists:

```rust
assert!(!pipe_is_at_eof(&mut read_end), ...);
```

(plan:1436-1439). A blocking pipe read in that state waits indefinitely; it does not return “not EOF.” The test hangs. It must use `O_NONBLOCK`, `poll`, or an explicit sentinel protocol.

#### B-9. The boundary test reuses a one-shot stub for a second blocking dispatch

`the_blocking_form_is_refused_once_the_seat_is_active` performs one cold-start dispatch and later a final-offline dispatch using the same `RejectWith` helper (plan:1491-1527).

The current `RejectWith` stub reads and answers exactly one request, then performs a single-byte blocking read and exits without decoding or replying to another request (`test_support.rs:213-230`). Nothing in Task 4 says it becomes a multi-request serve loop.

The final-offline call therefore waits for the thirty-second watchdog and returns an unknown outcome, contradicting `.is_ok()`. The test needs a fresh executor or a looping stub.

#### B-10. Task 7’s required source gates cannot pass after implementing Tasks 4 and 6

Task 7 requires no `std::thread::sleep` and no `.wait()` anywhere under `kms/executor` (plan:2408-2415).

That directly contradicts required code:

- `AcceptAfterReturningInheritedFd` is specified to sleep for its delay (plan:1691-1695).
- `NeverReply` and `IgnoreTermination` already sleep in their helper loops (`test_support.rs:204-207,232-239`).
- `AcceptAfter` sleeps to simulate ioctl duration (`test_support.rs:241-250`).
- The lock-holder subprocess sleeps while retaining the lock (`device_lock.rs:254-272`).
- Existing lock tests call `child.wait()` (`device_lock.rs:291-307`).
- Task 4’s test-support reap helpers will themselves need some bounded waiting mechanism.

These are helper/test-process waits, not X11-core waits. The grep is scoped too broadly and makes the declared exit gate impossible without deleting test behavior the plan depends upon.

### Major

#### M-1. The replacement property-submission test still cannot prove arrays reached the ioctl

The test uses `/dev/null` and treats `ENOTTY` as proof that the real helper submitted the materialized property arrays (plan:942-961).

`ENOTTY` proves only that `ioctl(fd, DRM_IOCTL_MODE_ATOMIC, arg)` was invoked on a non-DRM fd. `/dev/null` does not inspect `count_objs` or any pointer field. The test passes equally if the helper still submits `count_objs = 0` and null pointers—the exact regression it claims to catch.

The preparation unit tests prove `prepare_atomic` can construct correct arrays; this integration test does not prove `execute_atomic` actually uses the prepared result when forming the raw ioctl request.

#### M-2. The hardware-only test is not ignored and assumes an errno the setup cannot guarantee

The prose says `the_helper_reports_a_kernel_rejection...` is `#[ignore]`d (plan:964-969), but the shown code has only `#[test]`.

It will therefore run in the ordinary integration suite. `open_real_drm_or_ignore()` returning `None` silently reports PASS despite the surrounding claim that hardware coverage is “reported separately.” When a device is available, opening a DRM node does not establish master status, atomic-client capability, or permissions, so `EACCES`, `EPERM`, or `EOPNOTSUPP` can precede validation of object id zero. The unconditional `EINVAL` assertion (plan:969-975) is not portable hardware validation.

#### M-3. The timing tests remain scheduler-sensitive despite the self-review claim

Correct nonblocking code can be descheduled for more than two seconds between `started` and the assertion. Both `send_returns_without_waiting...` and the `poll_reply` loop then fail despite never blocking (plan:1153-1185). The same issue exists in the `tick` duration assertion (plan:1330-1343).

The larger ceiling reduces likelihood but does not make the tests scheduler-insensitive, contrary to plan:2471. A deterministic test needs an instrumented transport/state transition, not elapsed wall time.

#### M-4. Pipe EOF proves eventual last-close, not “closed exactly once”

Once fixed to avoid blocking, pipe EOF can prove that no write-end descriptor remains open after the returned `OwnedFd`s are dropped. It cannot distinguish one correct close from an erroneous raw double-close, nor attribute closure separately between helper and parent.

The plan partially admits this is joint coverage at plan:2473, but its test name and exit criterion still claim exact-once parent closure (plan:1423-1440, 2450). The test proves ownership/adoption and absence of a surviving duplicate, not exact close cardinality.

#### M-5. The reaped-source test does not prove withdrawal from the real core poller

The production core snapshots `Backend::poll_fds()` once before entering the loop and never refreshes it (`run.rs:1039-1059`). Task 5’s test merely calls `platform.poll_fds()` again after reaping (plan:1841-1852).

That can pass while the core still retains the original `backend_poll_sources` entry. The plan adds no deregistration/re-registration mechanism. This matters when 2b replaces a reaped executor: the new control fd cannot enter the already-running core poller through `poll_fds()` alone.

The existing anchors are accurate; the behavioral conclusion drawn from them is not.

#### M-6. The discovery-lock test is vacuous

`discovery_probing_takes_no_install_lock` holds a fabricated key `(226, 248)` and then runs global candidate discovery (plan:2248-2256). Candidate discovery opens whatever actual card nodes exist (`platform/drm.rs:187-225`); it need not encounter that fabricated device identity.

The test therefore passes even if discovery incorrectly locks every real device it probes. It needs injected candidate paths/identities or instrumentation recording lock attempts.

#### M-7. “Identity allocation is checked” is broader than the changes actually specified

The plan globally requires checked identity allocation and claims this as an exit criterion (plan:33, 2457), but Task 1 changes only `IdentityAllocator` plus the two newly introduced monotonic types (plan:88-91, 247-251).

Existing `IncarnationId::next` and `ClockEpochId::next` still use unchecked `+ 1` (`identity.rs:18-21,103-106`), and no task lists changing them. If the intended claim is only commit/event/sequence allocation, the global and exit language must say so; otherwise the stated invariant remains false.

### Minor

#### m-1. The `LOCK_UN` source gate contradicts the comments in the prescribed implementation

Task 7 says `LOCK_UN` should appear only inside `release_explicitly` (plan:2417-2425), but the prescribed `Drop` documentation itself contains `LOCK_UN` several lines before that method (plan:2271-2275). A raw `rg` cannot establish the claimed syntactic location without filtering comments or inspecting the result manually.

## Notes on the rest

I performed all five requested checks:

1. **Incorporation audit:** all 19 prior findings are classified above. I checked the per-task correction headers and the self-review claims against the actual task bodies.
2. **Existing-code verification:** I opened every cited Stage 1 surface. The numeric anchors themselves are accurate: `HostCallClass`, synchronous `dispatch`, `Drop`, inherited-fd plumbing, protocol topology generation, helper adoption, device-lock destructor, platform device structures, `poll_fds`, `next_wakeup`, the `Backend` trait, core dispatch, recording backend, host-X11 backend, and compile-fail runner all resolve to the claimed existing code. The false behavioral conclusions are reported above.
3. **Cross-task interface consistency:** I traced every `Consumes`/`Produces` section, including test-only helpers. The missing helpers, privacy conflicts, `Ready` accessor contradiction, clock-probe reservation/result gap, and `OpenError`/test-surface ambiguity are captured primarily in B-1 through B-5.
4. **Shown-code and test analysis:** I checked every Rust block for name resolution, visibility, ownership, moves, blocking behavior, enum shapes, and stub behavior. Definite failures include the missing imports/methods, `E0509` move, blocking pipe read, one-shot helper reuse, unwired subprocess, and impossible grep gates.
5. **Spec compliance:** I checked COMMIT-5, COMMIT-6, COMMIT-7, ID-3, ValidationOnly, and section 18. I did not report the explicitly excluded owner, production call-site conversion, admission, clock-record, damage, completion, or retained `SequenceSupport` map.

The following parts are sound:

- The atomic 68-byte head, probe 56-byte head, byte-80 atomic body start, and 140-byte golden frame arithmetic are correct.
- The hostile-frame cap checks occur before allocation, use checked size arithmetic, and reject duplicate slot indices and CRTC ids.
- The holder vector reaches final length before addresses are installed; the shown preparation does not subsequently reallocate it.
- Seat-active live/validation flag agreement is correctly specified, apart from the missing cold/offline validation class.
- `terminalize_unknown` centralizes exactly-once terminal event emission and retains serialization until reap.
- Parent-only nonblocking socket configuration matches the existing blocking helper loop.
- Removing `Child::wait()` from `Drop` is necessary and correctly motivated.
- The executor deadline is correctly threaded into `next_wakeup` and is not incorrectly gated by `allow_kms_timers`.
- The conceptual `DeviceLock` → `InheritableDeviceLock` type-state design is appropriate; only its shown field-move implementation is invalid.
- The stated 2a/2b/2c scope boundary matches spec:3868-3880.
