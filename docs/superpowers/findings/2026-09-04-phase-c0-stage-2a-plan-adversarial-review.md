# Phase C.0 Stage 2a plan — adversarial review

**Date:** 2026-09-04
**Subject:** `docs/superpowers/plans/2026-09-04-phase-c0-stage-2a-executor-substrate.md` (6 tasks, 30 steps, 1149 lines)
**Reviewer:** `codex exec --sandbox read-only`, single pass
**Result:** 8 blocking, 9 major, 2 minor
**Disposition:** open. Not executed, not delegated.

## On the size hypothesis

The split was justified by a measured claim: that defect density rises with plan
size. This plan is the first test of it, and **the claim is not supported**.

| Plan | Tasks | Blocking | Per task |
| --- | --- | --- | --- |
| Stage 1 | 14 | 2 | 0.14 |
| Stage 2 monolith, revision 1 | 21 | 24 | 1.14 |
| Stage 2 monolith, revision 2 | 23 | 26 | 1.13 |
| **Stage 2a** | **6** | **8** | **1.33** |

Density did not improve. Two confounds matter more than the arithmetic, and both
argue that the stage 1 baseline was never comparable:

- **Different reviewer, different brief.** Stage 1's plan was reviewed by an
  independent Opus session against a general brief. Every stage 2 review was
  `codex exec` against a brief that explicitly demanded cross-task interface
  checking, verification of every claim about existing code, and an
  incorporation spot-check. A harsher instrument finds more.
- **Different material.** Stage 1's fourteen tasks included several
  straightforward ones. All six of 2a's are FFI, descriptor ownership, IPC
  framing and process lifecycle — the densest material in the phase.

What did change is the **character** of the findings, and that is worth more
than the count. The monolith's reviews rejected whole subsystems: the admission
tiers could not express the spec, submission was synchronous, resources were
never owned. This review confirms the design and rejects gaps in it — the
arithmetic, the holder allocation order, the `flock` reasoning, the inherited-fd
mechanism and every cited source anchor were checked and found sound. No task
was called unimplementable as a whole.

The honest conclusion is that splitting did not reduce defect count, and the
memory entry claiming a measured size effect has been corrected accordingly.
Whether the split still pays for itself through reviewability and convergence is
undecided; 2b will be the second data point.

## Findings

## Blocking

### B-1. The clock-probe correlation is incomplete and references an undefined type

`HostCallCorrelation::ClockProbe` omits `topology_generation` (`plan:399-406`). The spec requires clock-probe messages to carry “incarnation, `LifecycleEpochId`, topology generation, hardware CRTC, CRTC clock epoch, and a monotonic `ClockProbeId`” (`spec:641-645`).

This would regress the implemented Stage 1 request, which already contains `topology_generation` (`protocol.rs:87-95`). `ClockProbeId` is also used at `plan:405` but is neither produced by Task 1 nor listed among Task 2’s consumed Stage 1 identities (`plan:68-73`, `239`). The declared code therefore does not compile and cannot provide the required correlation tuple.

### B-2. The event-loop integration cannot be implemented in the files or interfaces named

Task 4 promises `SourceKind::ExecutorControl` behavior (`plan:804-812`) while declaring changes only to `executor/mod.rs` and `kms/render/backend.rs` (`plan:660-673`). There is no `SourceKind` in the real tree. The relevant type is `BackendFdKind`, which has no executor variant (`crates/yserver-core/src/backend/trait_def.rs:57-84`), and the core loop exhaustively dispatches its existing variants (`crates/yserver-core/src/core_loop/run.rs:1209-1257`).

Adding a real source therefore also requires changes to at least:

- `crates/yserver-core/src/backend/trait_def.rs`;
- `crates/yserver-core/src/core_loop/run.rs`;
- likely forwarding/test implementations of `Backend`;
- `kms/render/platform.rs`, whose `poll_fds()` owns the actual source inventory (`platform.rs:3936-3958`).

The watchdog integration is independently incomplete. The core blocks until `Backend::next_wakeup()` (`run.rs:1121-1148`), but the plan adds no executor-deadline accessor and never requires the executor deadline to participate in `KmsBackend::next_wakeup()` (`backend.rs:15201-15250`). Merely calling `tick_executors` after an unrelated wake does not guarantee the two-second watchdog ever fires.

This leaves the central round-2 finding—no production consumer—unfixed despite the claim at `plan:1147`.

### B-3. IPC failure, EOF, and malformed replies do not retain serialization until reap

The spec says:

> “No second ioctl may be dispatched on the device while this record or its executor lease exists.” (`spec:685-686`)

The plan preserves this only for watchdog expiry (`plan:882-887`). Its other acceptance-unknown paths do not specify the same transition:

- send failure queues `Unknown(IpcFailure)` and returns `SendError::Ipc`, without requiring `Stalled`, termination, or retention of `in_flight` (`plan:863-869`);
- EOF and receive failure yield unknown outcomes without those requirements (`plan:874-880`);
- malformed correlation/bitmap/fd replies likewise have no stall/reap transition (`plan:620-628`, `874-880`).

If these paths clear `in_flight`, another ioctl can reach a helper whose previous acceptance is unknown. If they retain it without entering a reap path, the executor wedges permanently.

There is also an exactly-once contradiction after watchdog expiry. `tick` already emits a terminal `Unknown(WatchdogExpired)` (`plan:882-885`), but a subsequent EOF is specified to yield `HelperExited` (`plan:874-876`), producing a second terminal outcome instead of only reap progress. No test covers EOF-after-watchdog or retry-after-malformed/send-failure.

### B-4. `KmsIoExecutor::Drop` still synchronously waits for an uninterruptible helper

The implemented destructor kills and then calls `Child::wait()` synchronously (`executor/mod.rs:496-502`). No task removes or replaces it. Consequently dropping an executor on the core thread can block indefinitely when the helper is stuck in an uninterruptible kernel call.

That directly contradicts both:

- “The X11 core never executes or waits synchronously for a potentially blocking KMS ioctl” (`spec:646-650`);
- the bounded `ShutdownExecutorStalled`/orphan fallback (`spec:699-710`).

The source scan at `plan:1103-1110` checks only `libc::poll` and `std::thread::sleep`, so it would declare this path clean while the blocking `wait()` remained.

### B-5. The reap and late-reply tests cannot pass with the specified executor

`watchdog_expiry_does_not_release_serialization_before_reap` calls `force_reap_for_tests()` and then expects `send()` on the same executor to succeed (`plan:758-770`). But the plan itself says a reaped executor has no control fd (`plan:871-872`), and it defines no helper-respawn operation. The implemented reap state is `ExecutorState::Reaped` (`executor/mod.rs:446-472`). A dead helper and closed socket cannot accept the asserted send.

The late-reply test is also inconsistent with the real stub mechanism. It uses an ordinary slow helper (`plan:774-789`), then `tick` sends a termination request (`plan:882-885`). Stage 1’s `AcceptAfter` helper does not ignore `SIGTERM`; `IgnoreTermination` is a separate behavior (`test_support.rs:232-263`). The helper will normally die instead of sending the expected late reply.

### B-6. The production device-lock handoff has no representable ownership path

Task 5 says `DeviceLock::into_inheritable` “yields the raw descriptor” (`plan:1050-1053`) and later says the parent drops its `DeviceLock` after a first reply (`plan:1059-1061`). A consuming `into_*` method cannot leave that same guard available to drop. Returning an unowned `RawFd` is also insufficient to define exactly who closes it on spawn failure.

The real production structures make the file list incomplete:

- `PlatformInitDevice` contains only `key` and `device` (`kms/backend.rs:677-681`);
- it is converted into `KmsDevice`, which also contains no executor or lock (`kms/render/platform.rs:1990-1994`, `2550-2559`);
- Task 5 does not modify `kms/render/platform.rs`, while Task 4 already assumes the long-lived backend owns executors.

No startup-ready protocol is defined either, despite the proposed “first reply proves the helper is running” handoff point. Because 2a converts no host-call site, such a reply need not occur.

Finally, `release_explicitly(self)` remains callable on a guard sharing the helper’s open file description (`plan:1037-1041`). Without a type-state transition that removes this operation after handoff, it can globally unlock the executor-held lock—the exact behavior Task 5 is meant to eliminate.

### B-7. Host-call classes do not enforce `COMMIT-5` or validation semantics

The decoder validates payload sizes and slots but never requires the flags and payload to agree with `HostCallClass` (`plan:428-434`). Thus it permits:

- `SeatActiveNonblock` without `NONBLOCK`;
- `SeatActiveValidation` without `TEST_ONLY`, or with `NONBLOCK`;
- validation with out-fence slots;
- a live request labelled as validation to obtain the two-second class.

The spec explicitly requires seat-active live commits to use `NONBLOCK` (`spec:646-650`) and says `TEST_ONLY` omits `NONBLOCK`, creates no out-fence, and is not a submitted record (`spec:320-329`, `2126`).

The API is also unusable for production validation: every `send` requires `SubmittingProof` (`plan:681`, `863-869`), while the spec says validation “is neither a live blocking commit nor a submitted record” (`spec:651-653`). Line 1140 promises future proof producers for commit records and clock probes but names no validation-lease proof.

`BoundaryWitness` does not repair this. Its constructors are `pub(crate)` (`plan:889-894`), so every seat-active module in the crate can fabricate one. An external compile-fail test cannot prove an internal caller is at cold startup or final offline.

### B-8. Multiple shown tests do not compile against the shown types

The stale-correlation test treats a named-field enum variant as a tuple and attempts struct-update syntax on it (`plan:316-319`):

```rust
HostCallCorrelation::Atomic(a)
```

That does not match the declared named variant at `plan:390-398`, and enum variants cannot be used as the base of struct update syntax in that form.

The holder-address test reads `prepared.holder_addresses[0]` (`plan:477-490`), but `PreparedAtomic` has no `holder_addresses` field (`plan:583-590`) and no method producing it.

These are exactly the cross-task/interface inconsistencies the split claimed to eliminate.

## Major

### M-1. `FdLedger` still cannot observe the asserted `OwnedFd` closes

The late-reply test installs `FdLedger`, drops a `Vec<OwnedFd>`, and expects the ledger to count the close (`plan:773-787`). But the outcome type contains ordinary `OwnedFd` values (`plan:469`); their destructor closes through the standard library, not through a project wrapper. The prose that the ledger “wraps descriptor creation and close” (`plan:635-639`) supplies no interception path.

The helper-local mirror addresses the prior cross-process objection only for the rejected-holder unit test. It does not make the parent-side `OwnedFd` assertion observable. `common/fd_ledger.rs` is also added without requiring `common/mod.rs` to declare the module.

### M-2. The purported property-submission integration test asserts on a stub

`the_helper_submits_the_property_list_and_reports_the_kernel_errno` says object zero forces “the kernel” to reject (`plan:528-539`), but it uses `TestDevice::open_stub()`. The same task defines that target as a scripted helper (`plan:630-633`). A scripted rejection can pass even if the real helper still submits `count_objs = 0`.

The pointer/materialization unit tests are useful, but this integration test does not connect them to the raw ioctl path.

### M-3. Malformed-frame arithmetic and allocation order are under-specified

The plan says the decoder checks the exact computed length and later reruns `validate()` (`plan:428-434`), but it does not require caps and checked size arithmetic before allocating vectors from wire-provided `u32` counts. A decoder that allocates first can attempt enormous allocations before `validate()` rejects the frame.

It also does not reject duplicate `OutFenceSlot.value_index` or duplicate CRTC slots. Duplicate indices cause later pointer patches to overwrite earlier holder addresses, breaking the one-holder-per-slot model.

### M-4. The “every field” golden test does not check every field

The test named `a_golden_frame_places_every_field_at_its_documented_offset` checks magic, three initial identities, two one-byte fields, flags, object count, and total length (`plan:261-285`). It does not verify transition value, commit, event token, prop count, slot count, or any body field. Encoder and decoder offset mistakes in those fields can still round-trip and pass.

The underlying 68-byte calculation and byte-80 body start are correct.

### M-5. The compile-fail checks are not meaningful or wired into the existing runner

Task 1 says to add a compile-fail case “beside stage 1’s existing ones” (`plan:197-199`), but the real runner compiles only `reap_proof_cannot_clone.rs` (`tests/compile_fail.rs:29-43`). No step modifies that runner.

Moreover, lifecycle types live in the private `kms::owner` module (`kms/mod.rs:18`). An external compile-fail case would fail on privacy before testing epoch/transition type distinction. The boundary test merely asserts that a file exists (`plan:823-831`), not that the file is compiled for the intended diagnostic.

### M-6. The timing tests remain scheduler-sensitive

The plan claims to incorporate scheduler-insensitive tests (`plan:1147`), but it still asserts 50 ms and 10 ms wall-clock ceilings (`plan:678-696`, `744-750`). A preempted CI process can fail these despite a nonblocking implementation.

`helper_death_while_in_flight` is also racy (`plan:792-801`): socket EOF does not guarantee `try_wait()` observes the child as reaped at that exact instant, so the planned implementation may return `IpcFailure` rather than `HelperExited`. Both are acceptance-unknown under the spec.

### M-7. The parent-death lock property is not actually tested

`the_lock_survives_the_parent...` merely drops an `opened` value while the test process remains alive and retains the helper handle (`plan:964-975`). It proves parent-side descriptor closure, not parent process death, reparenting, or restart refusal—the actual `COMMIT-7` threat model (`spec:712-719`).

The deterministic no-gap probe and lock-refusal tests are otherwise useful.

### M-8. The nonblocking socket requirement is ambiguous and dangerous if followed literally

The plan states “The socket is `O_NONBLOCK` from construction” (`plan:871-872`). If `SOCK_NONBLOCK` is applied to both ends of the socketpair, the helper’s blocking receive loop will immediately see `WouldBlock` and exit. Stage 1’s helper expects a blocking endpoint (`helper.rs:79-103`).

The plan must require only the parent control endpoint to become nonblocking, after the pair is created, while preserving the helper endpoint’s blocking semantics.

### M-9. The self-review overstates incorporation

Several claimed incorporations at `plan:1147` remain only partial:

- event-loop integration lacks the real poll-source and deadline plumbing;
- send-failure terminalization lacks the required stall/reap exclusion;
- the late-reply test uses a helper that termination kills;
- the boundary witness is forgeable throughout the crate;
- the timing tests still use scheduler-sensitive thresholds;
- holder-address coverage references a nonexistent field;
- deterministic property coverage asserts on a scripted result rather than the ioctl path.

## Minor

### m-1. Task 1’s prescribed Cargo commands are invalid

`cargo test` accepts one positional test filter. Both commands at `plan:160` and `213` supply three: `lifecycle identity host_call_class`. They must be separate invocations or use one common filter.

### m-2. Several source-text tests are brittle substitutes for behavioral checks

Counting the string `libc::poll`, searching for an exact multiline function signature, and checking only that a compile-fail file exists (`plan:823-831`) can fail after harmless formatting or pass while the boundary remains callable. The final `rg` gate has the same limitation (`plan:1101-1110`), notably missing `Child::wait()`.

## Notes on the rest

- The cited Stage 1 anchors are accurate: `HostCallClass` is at `executor/mod.rs:153-181`, synchronous `dispatch` at `:293-433`, inherited slots and `dup2` plumbing at `:589-663`, helper adoption at `helper.rs:72-76`, and device open at `kms/backend.rs:844`.
- `DeviceLock::drop` really does call `LOCK_UN` (`device_lock.rs:185-191`). The plan is correct that this globally releases a lock shared through duplicated descriptors and must be removed for handoff.
- The `flock` open-file-description premise, inherited fixed-fd mechanism, and `dup2`/`FD_CLOEXEC` reasoning are sound.
- `present/event_loop.rs::run_loop` has no caller in the workspace; leaving it out of scope is accurate.
- The 68-byte atomic head, documented offsets, byte-80 body start, and maximum payload size are arithmetically sound.
- The shown holder allocation order is sound: holders reach final length before addresses are installed, and none of the shown operations reallocates them before the ioctl.
- The slot-mask expression is safe under the existing `MAX_OUT_FENCES = 16`, provided the promised count-limit check runs first.
- I did not treat the deliberate absence of the owner, production call-site conversion, admission, clock-record logic, damage, or completion handling as defects. Those are explicitly assigned to 2b/2c by `spec:3868-3880`.
