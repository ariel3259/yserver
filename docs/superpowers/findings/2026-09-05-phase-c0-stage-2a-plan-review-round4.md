# Phase C.0 Stage 2a plan — adversarial review, round 4

**Date:** 2026-09-05
**Subject:** `docs/superpowers/plans/2026-09-04-phase-c0-stage-2a-executor-substrate.md` revision 4 (`3a721a34`, 7 tasks, 3213 lines)
**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `38aee673`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.152.0`.
**Disposition:** open. Not executed, not delegated.

## The first valid comparison in this project

Round 3 and round 4 ran under the **same instrument SHA**. That makes their
blocking counts comparable — the first time any two rounds in this work have
been.

| Round | Instrument | Blocking | Major | Minor |
| --- | --- | --- | --- | --- |
| 1 | 3-check brief, lost | 8 | 9 | 2 |
| 2 | 5-check brief, lost | 10 | 7 | 1 |
| 3 | `review/` @ `38aee673` | 10 | 2 | 2 |
| **4** | **`review/` @ `38aee673`** | **10** | **5** | **2** |

**10 → 10. No improvement.** Revision 4 closed round 3's ten blockers and
produced ten more.

The major counts are still not comparable to each other: round 3's `--context`
told the reviewer to classify six known-open majors as NOT APPLIED without
repeating them, and round 4's did not, because nothing was deliberately left
open. **That is a gap in the freezing mechanism** — `--context` and
`--out-of-scope` demonstrably move the numbers and are not covered by the
instrument SHA. Recorded here rather than fixed mid-comparison.

## Verification status

Spot-checked before filing. Everything checked reproduces.

- Task 2's handshake prose still declares "a unit struct" and
  `HandshakeReply { helper_pid: u32 }`, and still argues the handshake
  "precedes every identity ... before any epoch has been assigned to anything"
  (`plan:1219-1223`). The contract two hundred lines earlier says the opposite.
  **The document contradicts itself.** B-1 holds.
- The decoder's `assert_class_agreement` enumerates three classes
  (`plan:1211-1213`); `ColdStartOrOfflineValidation` is absent. B-8 holds.
- `map_err(...)` at `plan:3018` is a **literal placeholder**, which the plan's
  own authoring rules forbid outright. B-9 holds.
- Eight `KmsDevice { ... }` literal sites exist in the tree
  (`platform.rs:2555,2982,7262,7710,7727,8319`, `backend.rs:24196`, plus the
  production conversion). B-10 holds.
- Rust's standard test harness has no per-test timeout, so revision 4's
  "the harness's own timeout will fail it" is false and the tests hang CI
  instead. M-3 holds.

## What this round actually establishes

**The contract section did not work, and the reason is not the contract.**
Revision 4 added a normative type model and then *patched* the tasks instead of
regenerating them against it. So the model now lives in two places that
disagree: B-1, B-4 and B-8 are all contract-versus-task divergences inside one
document. Adding a second authoritative location without rewriting its
consumers made the consistency problem worse, not better. This project's own
recorded lesson — rewrite affected tasks whole rather than patching cited
paragraphs — was available and was not followed.

**Seven of the ten blockers are compile errors.** B-3 (a decoder signature that
cannot do what its prose promises), B-4 (`pub(crate)` types used from an
external test crate), B-5 (a `#[cfg(test)]` seam unavailable to integration
tests, returning dangling pointers), B-6 (unresolved names, wrong arity, an
unused import under `-D warnings`), B-8 (a non-exhaustive match), B-9 (`?` on a
`Result` inside a function returning `Option<Result<_>>`) and B-10 (struct
literals missing a new mandatory field) would all be reported by `cargo check`
in under a minute.

Only B-1 (the `ID-3` obligation on the handshake), B-2 (a reply-kind model that
makes an `EOPNOTSUPP` clock-probe rejection unrepresentable) and B-7 (a stage-1
test helper that reopens an inherited fd read-only, so the pipe test cannot
observe what it claims) are semantic findings a compiler cannot reach.

That ratio is the useful result of this round. Four review cycles have been
spent using a slow, expensive, imprecise reviewer to find defects that the Rust
compiler finds instantly and exactly. The plan's remaining value is in the three
findings of the second kind.

---

**Result:** 10 blocking, 5 major, 2 minor

Revision 4 does not resolve all prior findings. Its “all resolved” claims at `plan:15` and `plan:3200-3206` are materially overstated.

## Incorporation audit

### Round 3 findings

| Prior | Status | Audit |
|---|---|---|
| B-1 | PARTIAL | Request builders were moved to Task 2, file/commit lists improved, and `state` is included in the visibility contract. External tests remain non-compiling because task code still declares promised public types `pub(crate)`, uses unproduced/private helpers, mismatches helper signatures, and exposes a `#[cfg(test)]` capture seam to an integration test. See B-4 through B-6. |
| B-2 | PARTIAL | The normative contract gives both handshake frames incarnation and lifecycle epoch (`plan:185-204`), but Task 2 reverts to a unit request and pid-only reply (`plan:1219-1223`), and Tasks 5–6 never pass an epoch into spawn. See B-1. |
| B-3 | APPLIED | The golden atomic request now uses `EventToken::tagged_for_tests(0x66)` and asserts its tagged wire value (`plan:618-657`). |
| B-4 | APPLIED | `send` is explicitly required to reject cold/offline classes while seat-active (`plan:127-132,1984-2007,2204-2214`). |
| B-5 | APPLIED | Both validation classes are covered by encoder and decoder out-fence rejection (`plan:1054-1080,1202-1215`). |
| B-6 | APPLIED | Validation failures now produce `ValidationAbandoned`, not live-mutation `Unknown` (`plan:215-245,2010-2029,2180-2201`). |
| B-7 | TRADED | `InFlight` now retains `kind` and `slot_count`, but the reply-kind model makes every rejected clock probe appear to be the wrong family, and Task 2 still assigns an impossible request-dependent check to `decode_reply`. See B-2 and B-3. |
| B-8 | APPLIED | The handoff test now uses a signal-insensitive helper that does not read control EOF and redirects stderr (`plan:2718-2719,2821-2842,3021-3027`). |
| B-9 | APPLIED | Task 2 stages all visibility files (`plan:1238-1244`), and Task 6 now stages `bin/yserver.rs` (`plan:3053-3058`). |
| B-10 | APPLIED | The invented `OpenError` is gone. Lock refusal stays an `io::Error`/`ResourceBusy` step compatible with `platform_init`’s existing `io::Result` (`plan:2876-2890,2981-2984`; `backend.rs:829-882`). |
| M-1 | NOT APPLIED | The replacement uses two devices with one reply each. One `poll_reply` per device still passes, so it does not prove drain-to-exhaustion on any readable source. See M-1. |
| M-2 | NOT APPLIED | The revised `rg -B1` pipeline remains line-oriented and strips the attribute line while retaining the declaration. See M-2. |
| m-1 | APPLIED | The planned comments no longer spell `LOCK_UN`; only `release_explicitly` does (`plan:2734-2739,2919-2947`). |
| m-2 | PARTIAL | The primary-device anchor and “fifth block” language were corrected (`plan:2571,2993`), but Task 6’s file list still cites `bin/yserver.rs:6-36`, excluding the existing block through line 44 (`plan:2710`; `bin/yserver.rs:36-44`). |

### Carried round-2 majors

| Prior | Status | Audit |
|---|---|---|
| R2-M-1 | TRADED | A raw-ioctl capture seam was added, but it is unavailable to the external integration test under `#[cfg(test)]` and returns a raw pointer-bearing `DrmModeAtomic` without preserving its backing arrays. See B-5. |
| R2-M-2 | APPLIED | The hardware test is actually `#[ignore]` and no longer requires one exact errno (`plan:1373-1396`). |
| R2-M-3 | TRADED | Scheduler-sensitive elapsed ceilings were removed, but the replacement deliberately hangs forever on a regression and incorrectly claims the Rust harness supplies a timeout (`plan:1589-1594`). See M-3. |
| R2-M-4 | APPLIED | The pipe assertion is now accurately described as adoption/eventual last-close rather than close cardinality (`plan:1868-1872,3162`). |
| R2-M-5 | APPLIED | The fresh-`poll_fds` limitation remains accurately documented as a 2b prerequisite (`plan:2437-2451,2633-2640,3179`). |
| R2-M-6 | APPLIED | The vacuous fabricated-discovery test was replaced by a structural call-site inspection (`plan:3120-3137`). |
| R2-M-7 | APPLIED | Task 1 now changes `IncarnationId::next` and `ClockEpochId::next` to checked implementations (`plan:470-482`). |

## Findings

### Blocking

#### B-1. The handshake contract is immediately overwritten by the old epochless model

The contract requires:

```rust
HandshakeRequest { incarnation, lifecycle_epoch }
HandshakeReply { incarnation, lifecycle_epoch, helper_pid }
```

and correctly ties this to `ID-3` (`plan:183-204`; `spec:416-425`). But the global constraints still say the handshake “precedes every identity” (`plan:31`), Task 2 again specifies a unit request and pid-only reply (`plan:1219-1223`), and its test still calls `encode_handshake_request()` without the required argument (`plan:839-846`).

Production cannot satisfy the contract either. Task 5’s `KmsIoExecutor::spawn` call passes only `IncarnationId` (`plan:2579-2582`), and every Task 6 spawn signature and invocation likewise lacks `LifecycleEpochId` (`plan:2718,2787-2792,2805-2807,3025-3028`). `await_helper_ready(Duration)` has nowhere to obtain or validate the required epoch.

Following the task text either fails to compile against the contract or emits executor request/reply traffic forbidden by `spec:422-425`.

#### B-2. The reply-kind model makes explicit clock-probe rejection impossible

The contract permits every `Rejected` reply to carry the general `HostCallCorrelation`, but declares that `HostCallReply::kind()` returns `Atomic` for every `Rejected` (`plan:163-175`). `poll_reply` must reject a reply whose kind differs from the request kind before checking correlation (`plan:177-181,2220-2228`).

Therefore a failed clock probe represented as:

```rust
HostCallReply::Rejected {
    correlation: HostCallCorrelation::ClockProbe { ... },
    errno: EOPNOTSUPP,
    ...
}
```

is always classified as malformed because the request kind is `ClockProbe` while the reply kind is `Atomic`. This directly contradicts the plan’s own 2b handoff, which requires an `EOPNOTSUPP` `Rejected` probe result (`plan:3178`), and the spec’s requirement for a complete typed clock-probe result (`spec:635-645`).

The reply enum needs family-specific rejection variants or an explicit reply family independent of the correlation payload.

#### B-3. Task 2 assigns request-dependent validation to a one-argument reply decoder

Task 2 states that `decode_reply` rejects mask bits above the original request’s slot count (`plan:1217`). Its tests establish a one-argument API, `decode_reply(&frame)` (`plan:746,755`), and reply frames do not carry the request slot count.

Task 4 later explicitly admits that `decode_reply` cannot perform this check and stores `slot_count` in `InFlight` for `poll_reply` to use (`plan:2149-2154,2224`). Task 3 also presents the check as executor-side validation (`plan:1486-1494`).

These are mutually exclusive implementation instructions. Task 2 cannot implement its stated `decode_reply` guarantee with the shown signature and wire format.

#### B-4. The promised external-test visibility is still contradicted by the shown declarations

The contract says all listed test-facing items become `#[doc(hidden)] pub` (`plan:263-285`). Yet the contract’s own type definitions use `pub(crate)` (`plan:141-168,185-190`), and Task 2’s implementation again declares protocol constants, `HostCallCorrelation`, and `seq` as `pub(crate)` (`plan:1123-1165`).

Task 6 similarly shows `DeviceLock`, `InheritableDeviceLock`, and their methods as `pub(crate)` (`plan:2933-2971`) even though `executor_lock_handoff.rs` is an external crate and directly invokes them (`plan:2780-2890`). That test also calls the currently private `executor_executable` (`executor/mod.rs:505`) at `plan:2846`; neither the visibility table nor Task 6 widens it.

Thus round-3 B-1 remains reproducible despite the visibility prose.

#### B-5. The ioctl-capture test is both unavailable to integration tests and lifetime-unsound

Task 3 places `the_submitted_ioctl_argument_is_the_prepared_arrays` in external integration test `tests/executor_async.rs` (`plan:1327-1350`) while saying the ioctl seam is swapped “under `#[cfg(test)]`” (`plan:1332-1334`). A library dependency is compiled without its unit-test `cfg` for integration tests, so that seam is not available there.

The declared return is also the raw `DrmModeAtomic` (`plan:1265`). Its pointer fields refer into `PreparedAtomic` vectors. Returning only that raw structure after the capture call drops those vectors, yet the test dereferences the pointers through `objs_as_slice`, `count_props_as_slice`, and similar methods (`plan:1343-1348`). Unless an undeclared owner is retained, this is dangling-pointer access.

The seam must return an owned snapshot captured while the pointed-to arrays are alive, and it must be available through the normal test-support build used by integration tests.

#### B-6. The external tests still contain unresolved names and signature contradictions

Task 3 declares:

```text
dispatch_and_wait_for_tests(&mut KmsIoExecutor, &HostCallRequest)
```

at `plan:1266`, but every shown call passes the request by value (`plan:1362,1391,1406,1415`). `ScriptedReply`, `atomic_request_for_tests`, `large_property_list_for_tests`, and several helper-side test functions/types are used without being produced by Task 2 or Task 3’s interface.

Task 4 imports request helpers only through the `test_support` module (`plan:1579-1587`) but repeatedly calls them unqualified (`plan:1627,1647-1651,1673-1676,1688,1709,1731,1747,1756,1767,1788,1878,1900,1925,1953-1976,2078,2092-2098`). It also imports `protocol::HostCallRequest` without naming that type anywhere, producing an unused-import error under the mandatory `-D warnings` gate.

Finally, Task 2 defines `validation_request_for_tests(class)` (`plan:583`), while Task 4 calls both the one-argument and nonexistent zero-argument forms (`plan:1994,2017,2078,2092`).

The Task 3 and Task 4 gates cannot compile as shown.

#### B-7. The pipe test passes a reopened read end, not the promised write end

The ownership test calls `spawn_stub_helper_with_event_fd(..., &write_end)` and claims the helper inherits the pipe’s write end (`plan:1833-1848`). The existing helper explicitly reopens `/proc/self/fd/{raw}` with `.read(true)` and prefers that reopened descriptor (`test_support.rs:75-89`).

For a pipe, this gives the helper a read endpoint. Dropping the parent’s original `write_end` can therefore make the read side report EOF before the returned “fence” is dropped, contradicting the negative assertion at `plan:1886-1889`.

Task 4 does not specify changing this existing helper or producing a preserve-access-mode alternative. The test setup cannot demonstrate the descriptor lifetime it claims.

#### B-8. Task 2’s four-class contract is implemented as a three-class decoder

The normative table defines four classes and requires class/flag agreement for all four (`plan:113-137`). The hostile-frame tests include `ColdStartOrOfflineValidation` (`plan:1038-1041`).

But Task 2’s decoder instructions enumerate agreement rules for only:

- `SeatActiveNonblock`
- `SeatActiveValidation`
- `ColdStartOrOfflineBlocking`

at `plan:1211-1213`. `ColdStartOrOfflineValidation` is omitted. A direct exhaustive match will not compile; a conditional implementation can accept invalid cold/offline-validation flags. This contradicts both the contract and the stage exit claim at `plan:3164`.

#### B-9. The shown handoff entry point is not valid Rust

`run_lock_handoff_if_requested` returns `Option<io::Result<()>>` (`plan:3015-3016`) but applies `?` directly to `Result` values (`plan:3018-3028`). `Result` residuals cannot be propagated from a function returning `Option<Result<...>>`.

The block also contains the literal placeholder `map_err(...)` and calls `io::stdout().flush()` without showing `std::io::Write` in scope. Under the instruction to treat every shown block as compilable implementation, this entry point fails before reaching `_exit`.

It needs an inner `|| -> io::Result<()>` body wrapped in `Some(...)`, or explicit `Some(Err(...))` handling.

#### B-10. Adding a mandatory executor field leaves existing struct literals uncompilable

Task 5 adds `executor: KmsIoExecutor` to both `PlatformInitDevice` and `KmsDevice` (`plan:2333,2569-2594`) but does not instruct the implementer to update the existing `KmsDevice` literals.

The current tree has literals at:

- `kms/render/platform.rs:2555`
- `kms/render/platform.rs:2982`
- `kms/render/platform.rs:7262`
- `kms/render/platform.rs:7710`
- `kms/render/platform.rs:7727`
- `kms/render/platform.rs:8319`
- `kms/render/backend.rs:24196`

Only the production conversion at `plan:2591-2594` is discussed. Every unchanged test literal produces a missing-field error. Because `KmsIoExecutor` is process-owning and has no trivial default, this is not merely adding `executor: Default::default()`; the plan must specify how legacy fixtures obtain or represent an executor.

### Major

#### M-1. The revised drain test still does not test draining a source to exhaustion

The test queues one reply on each of two executors (`plan:2513-2529`). An implementation that loops over devices and calls `poll_reply` exactly once per device returns two events and passes.

It therefore does not distinguish the required `while let Some(...)` implementation at `plan:2615-2623` from one-read-per-source behavior. Round-3 M-1 is still open; two replies must be queued on the same readable control socket if that invariant is to be tested.

#### M-2. The `#[doc(hidden)]` grep is still line-oriented and proves nothing

The proposed pipeline is:

```bash
rg -n -B1 '^\s*pub ...' protocol.rs |
  rg -v 'doc\(hidden\)' |
  rg 'pub '
```

at `plan:3118-3119`.

`rg -v` removes only the attribute line, not the following declaration line, so the final `rg 'pub '` prints every correctly annotated public declaration. Conversely, the command is not negated, so finding bare `pub` is a successful shell exit rather than a failed gate.

The explanation at `plan:3126-3129` assumes record-aware filtering that this pipeline does not perform. Round-3 M-2 was not applied.

#### M-3. The nonblocking regression tests can hang the suite indefinitely

The plan removes timing ceilings and intentionally makes a blocking implementation “not return at all,” claiming “the harness’s own timeout” will fail it (`plan:1589-1594`). Rust’s standard test harness has no per-test timeout.

If `send`, `poll_reply`, or the test socket setup blocks, `cargo test` and CI hang rather than report a test failure. These cases need a bounded subprocess/thread protocol or another deterministic nonblocking seam. This trades round-2 scheduler sensitivity for an unbounded test runner.

#### M-4. `tick` may reap immediately, contradicting tests that require `Stalled`

The prescribed `tick` terminalizes an expired request, sends termination, and then independently attempts `try_reap` on every call; a successful reap clears `in_flight` and changes state to `Reaped` (`plan:2230`).

Several tests use a normal `NeverReply` helper that dies on termination and then require `ExecutorState::Stalled` immediately after that same `tick` (`plan:1679-1686,1786-1793,2015-2028`). Whether `try_wait` observes the just-killed child is scheduling-dependent. The described implementation may legitimately produce `Reaped`, making those tests flaky.

#### M-5. The readiness wait has no implementation compatible with the nonblocking socket and source gate

Task 4 makes the parent endpoint nonblocking immediately after socket creation (`plan:2234-2244`). Task 6 then requires `await_helper_ready` to block for up to 30 seconds waiting for a handshake (`plan:2986`).

No production bounded-readiness helper is produced. Calling `recv_frame` directly returns `WouldBlock`; retrying spins; sleeping violates the host-call sleep gate; adding another `libc::poll` in `executor/mod.rs` contradicts the “exactly one, inside `dispatch_blocking_at_boundary`” rule (`plan:3097-3109`).

A reusable bounded wait primitive and its permitted location must be specified.

### Minor

#### m-1. One corrected binary anchor remains stale

Task 6’s file list cites `crates/yserver/src/bin/yserver.rs:6-36` (`plan:2710`), but the fourth existing dispatch block begins at line 36 and continues through line 44 (`bin/yserver.rs:36-44`). The later explanation correctly uses `6-44` and calls the new block fifth (`plan:2993`).

#### m-2. The Task 1 wire-tag test omits the fourth class

`the_class_round_trips_through_its_wire_tag` exercises only three classes and then declares tag `4` unknown (`plan:399-410`). `ColdStartOrOfflineValidation` is absent even though Task 1 claims to produce it. The later Task 4 watchdog table mentions all four, but Task 1’s own wire-tag gate does not prove that the fourth class has any decodable tag.

## Notes on the rest

I performed all five requested checks:

1. **Incorporation audit:** Classified every round-3 finding and separately audited the seven round-2 majors referenced by the prior review, including the six revision 3 left open.
2. **Existing-code verification:** Opened all cited Stage 1 surfaces in `identity.rs`, `protocol.rs`, `executor/mod.rs`, `helper.rs`, `transport.rs`, `test_support.rs`, `device_lock.rs`, `kms/backend.rs`, `platform.rs`, render `backend.rs`, `trait_def.rs`, `recording.rs`, `run.rs`, `bin/yserver.rs`, `platform/drm.rs`, `platform/ioctl.rs`, and the compile-fail runner. Apart from m-1 and the false behavioral/API claims reported above, the current-tree anchors identify the stated code.
3. **Cross-task consistency:** Traced every Consumes/Produces declaration, the normative contract, helper ordering, public visibility, handshake identities, reply families, reservation forms, struct-field additions, and external-test access.
4. **Compile/test audit:** Checked the shown blocks for name resolution, argument borrowing, visibility, `cfg(test)` availability, enum/error consistency, pointer lifetime, nonblocking I/O, process/descriptor lifetime, immediate-reap races, and whether each setup can produce its asserted observation.
5. **Spec compliance:** Checked `ID-3`, `COMMIT-5`, `COMMIT-6`, `COMMIT-7`, validation watchdog/out-fence semantics, clock-probe results, helper ownership, and section 18’s 2a boundary. I did not report the explicitly excluded owner, call-site conversion, admission, clock-record, damage, completion, SequenceSupport, or poll-source-churn work.

The following parts are sound:

- The atomic/probe head arithmetic, fixed offsets, and 140-byte golden atomic frame.
- Count caps, checked body-size arithmetic, and allocation-after-validation ordering.
- Tagged event tokens in the golden frame.
- Duplicate slot-index and duplicate-CRTC rejection.
- Validation out-fence rejection expressed through `class.is_validation()`.
- The validation-specific `ValidationAbandoned` outcome.
- Retaining `kind` and `slot_count` in `InFlight`.
- Enforcing cold/offline phase restrictions in `send`.
- Stable helper-side holder allocation before pointer installation.
- Nonblocking pipe reads themselves.
- Removing `DeviceLock`’s `Drop` implementation before moving its `File`.
- The revised wedged-helper parent-death model and stderr redirection.
- The corrected Task 2 and Task 6 commit file lists.
- Keeping executor deadlines outside `allow_kms_timers`.
- Recording core poll-source churn and `SequenceSupport` as explicit 2b prerequisites.