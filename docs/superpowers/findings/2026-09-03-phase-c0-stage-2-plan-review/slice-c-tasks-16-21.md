# Stage 2 plan review — slice C, tasks 16-21

Raw output of `codex exec --sandbox read-only`, 2026-09-03. Scope: the composed
and direct primary conversions, the damage-transaction re-anchoring, the device
lock, and the portable gates.

## Blocking

### B-1. The converted paths still block the X11 core on executor replies

The plan’s global constraint says, “The X11 core never executes or waits synchronously for a potentially blocking KMS ioctl” (plan lines 21–22), matching `COMMIT-5`: “The X11 core never executes or waits synchronously for a potentially blocking KMS ioctl” (spec lines 635–650).

But Task 6 defines `KmsDeviceOwner::submit` as a synchronous call to `self.executor.dispatch(&wire, proof)` (plan lines 1483–1540). The already-implemented executor then polls synchronously until reply or watchdog expiry ([executor/mod.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/executor/mod.rs:293), especially lines 321–380). Tasks 16 and 17 route live render/event-loop paths directly through this API (plan lines 3026 and 3190), allowing a two-second core stall.

This also contradicts the plan’s earlier interface statement that executor `dispatch` “takes the owned request” (plan line 69); Task 6 passes `&wire` and waits for an outcome.

The owner needs a nonblocking enqueue API: install the record, send IPC, return a `CommitId`, and consume replies from the event loop. Watchdog and reap processing must likewise be event-driven. Add a delayed-helper test proving input, VT, and device-loss work continues while a request is unresolved.

### B-2. `KmsDeviceOwner::submit` cannot provide either the identity or outcome required by Tasks 16–18

Task 6 defines:

```rust
fn submit(...) -> Result<(), SubmitError>
```

and returns `Ok(())` after every executor outcome, including explicit rejection and acceptance-unknown (plan lines 1483–1540). Later tasks require information this API discards:

- Task 16 says its “rejection arm” preserves `ReleasedButAtomicRejected` (line 3026).
- Task 17 must distinguish a submitted direct frame from a rejected one before installing `awaiting_outputs` (line 3190).
- Task 18 keys `PendingDamageStage` by “the `CommitId` the owner allocated for that submission” (line 3412), although callers cannot obtain that ID.

This is not just an asynchronous-design issue: even with the current synchronous executor, `SubmitError` describes owner refusal/construction, not the eventual `HostCallOutcome`.

Replace it with an API that returns the installed `CommitId` after dispatch/enqueue and register typed per-commit callbacks/events for `Accepted`, `Rejected`, `CompletionUnknown`, `HardwareComplete`, and release transitions. The copied/direct BO state machines and damage ledger must consume those outcomes rather than interpreting `Ok(())` as ioctl acceptance.

### B-3. The copied-scanout parked-intent path reports a nonexistent KMS submission and cannot resume safely

Task 16 keeps `submit_copied_scanout`’s `io::Result<()>` signature (line 2908). Its proposed pending arm parks the wait and returns `Ok(())` (lines 3005–3013). The actual caller interprets `Ok(())` as “KMS flip pending,” sets `InFlightStage::KmsFlipPending`, and waits for a page event ([scene.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2136)). No KMS request was sent, so that frame wedges.

The continuation is also undefined. “The event loop re-enters this function” (plan lines 3008–3010) would execute `copied.submit_copy(...)` again before polling, duplicating the Vulkan copy and losing the original wait. The proposed `transition_to_awaiting_producer(wait)` does not exist in the real `BoState`; its phases are only `Free`, `Recording`, `Submitted`, `Pending`, `OnScreen`, and `Retiring` ([scanout.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/vk/scanout.rs:109)). Task 16 does not even list `kms/vk/scanout.rs` as a modified file.

This violates the section 10.2 ownership rows requiring the pre-ioctl intent to own the wait and new resources, with no slot or live sync property, and requiring timeout/error/cancel to release the wait exactly once and perform never-submitted cleanup (spec lines 2121–2127).

Introduce an explicit pending-producer object containing the `OwnedFd`, output/BO identity, framebuffer, copy ownership state, Present/damage resources, and one-shot continuation. Return a distinct `PendingProducer` result so the caller retains `WaitingForProducer`, and resume after canonical fence-status validation without resubmitting the copy. Define cancellation, coalescing, output removal, timeout, and callback-after-removal cleanup.

### B-4. The final `TEST_ONLY` result does not authorize the live request actually installed

The spec requires the final test and live request to contain identical persistent objects, framebuffer/blob IDs, routing, modes, geometry, and state-affecting flags, with no generation change between them (spec lines 298–329).

Task 17’s `AtomicSnapshotId` contains generations only (plan line 3188). `install_validated(snapshot, direct_scanout_request)` accepts an arbitrary separately built request (test lines 3109–3117), so request B can be installed after validating request A as long as generations match. Nothing compares the persistent property list, framebuffer, geometry, or `ALLOW_MODESET` state.

Production is internally inconsistent as well: the tests use `install_validated` (line 3115), while the implementation instructions say direct submission calls ordinary `owner.submit` (line 3190), bypassing the validated snapshot entirely.

The validated snapshot must bind a canonical digest or owned immutable copy of the exact persistent request and relevant state-affecting flags. The live install should consume both that snapshot and the exclusive lease, add only permitted ephemeral synchronization/event properties, recheck all generations, and submit that bound request. Add negative tests changing framebuffer, one geometry property, routing, and `ALLOW_MODESET` without changing generations.

### B-5. Rejection and several `CompletionUnknown` paths leave damage stages permanently held

Task 18 calls `hold_damage_stage` before owner resolution (lines 3231, 3249, and 3367–3412). It then states that `FailedBeforeSubmit` pushes no event (line 3365). Therefore explicit ioctl rejection never removes the held `PendingDamageStage`. The rejection test checks only `ScanoutDamage::has_staged_frame` and missing area (lines 3246–3259); it never checks that the separate held stage was discarded. Task 19’s next submission can consequently fail with `OutputAlreadyStaged`.

The unknown wiring is also incomplete. The plan explicitly pushes `Unknown` only from the host-call unknown arm (line 3365). Hardware-fence failure, hardware deadline expiry, primary-event deadline expiry, and direct calls to `poison` also enter `CompletionUnknown`/poison, but no plan step emits a corresponding damage event. The synthetic tests manually inject `DamageEvent::Unknown`, bypassing those real transitions.

This violates the exhaustive mapping: “`FailedBeforeSubmit` | none” and “`CompletionUnknown` | invalidate,” plus “Incarnation poison … | invalidate” (spec lines 2427–2436), and test requirement 84 (spec lines 3177–3189).

Add a terminal damage disposition emitted centrally whenever a commit first terminalizes. Rejection must discard the held pre-accept stage without touching `ScanoutDamage`; every route to `CompletionUnknown` must consume it and invalidate exactly once. Poison without a current staged commit must carry the affected output set explicitly.

### B-6. Task 20 leaves the `COMMIT-7` lock in the parent, not the potentially orphaned executor

The spec says the lock is “taken by the executor for as long as it lives and released only by its death” (spec lines 712–719). Task 20 instead stores `DeviceLock` “alongside the `drm::Device` in the per-device record” (plan lines 3672–3676). If the parent exits or is killed while an executor remains wedged, the parent-held lock is released and a new server can install state under the old helper—the exact window `COMMIT-7` exists to close.

The tests prove only parent-record lifetime (plan lines 3634–3654). They never keep an executor alive after dropping/killing its parent-side guard.

Pass the acquired lock file description into the re-executed helper without a handoff gap, or have the executor acquire it under a coordinated parent-held bridge. Add the required cross-process test: executor remains alive, parent guard disappears, and a new `may_install_state` still refuses until actual helper death.

## Major

### M-1. The section 12.1 mapping omits the normative restore transition

The spec includes:

> “A post-accept failure whose prior state is proven still current | restore” (spec line 2436).

Tasks 18–19 define only `Accepted`, `HardwareComplete`, and `Unknown` events (plan lines 3219 and 3345–3356). No event or implementation step invokes `ScanoutDamage::retire_failure`, even though the real API provides it specifically for post-submit failures ([scanout_damage.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scanout_damage.rs:237)).

Add a typed proven-prior-current disposition and a test that starts with non-full partial damage, stages it, drives the real owner failure classification, and verifies `retire_failure` restores the submitted pending region without full invalidation.

### M-2. The damage tests bypass the owner mapping they claim to verify

Most Task 18/19 tests call `owner_deliver_for_tests(DamageEvent::Accepted/HardwareComplete/Unknown)` directly (plan lines 3241, 3265–3268, 3448–3451, and 3498–3517). These tests can all pass if:

- ioctl acceptance emits no `Accepted`;
- out-fence success emits the wrong event;
- `Presented` emits `HardwareComplete`;
- a deadline terminalizes without emitting `Unknown`;
- poison never reaches the backend.

The source-text test at lines 3325–3333 searches only a 2,000-byte substring and merely checks for the token `atomic_commit`; it does not verify milestone mapping.

Drive tests through scripted executor outcomes, real `poll_fences`, tagged page events, deadline expiry, and `poison`. Assert the resulting `ScanoutDamage` state. Include a table-driven test covering every row of spec lines 2427–2436.

### M-3. `expected_completion_outputs` has no defined CRTC-to-output mapping

Task 19’s no-stage fallback calls:

```rust
self.owner.expected_completion_outputs(commit)
```

(plan lines 3588–3594), but no earlier task defines this API. The commit record stores hardware CRTC IDs, not renderer `output_idx` values. Task 19’s interface says it consumes `ExpectedCompletionCrtcs` (line 3438), silently treating that as an output-index set.

Record the affected output identity/index mapping with the submission or resolve CRTC IDs through a generation-checked topology snapshot. Never cast or assume a hardware CRTC ID is a renderer index.

### M-4. The validation lease can be leaked and is not consumed by installation

`ValidationLease` is merely non-`Clone`; callers must manually invoke `release_validation_lease` (plan lines 3062 and 3188). Error, timeout, early return, or panic can leave the owner permanently leased. The stale-snapshot test performs `install_validated` while the lease remains outstanding (lines 3109–3117), and installation does not consume or authenticate it.

Make validation/install a state machine owned internally by `KmsDeviceOwner`, or make live installation consume a lease tied to owner/incarnation/validation identity on every terminal path. A watchdog-expired validation must also retain host-call exclusion until helper reap even though it does not poison live hardware state.

### M-5. Direct cursor preservation is asserted through a test-only boolean, not construction evidence

Section 12 requires: “Direct entry must attach the current cursor state atomically or prove that the already-submitted cursor plane state remains valid” (spec lines 2361–2364).

Task 17’s only test asserts `direct_entry_cursor_precondition_for_tests()` (plan lines 3169–3176). That accessor can return `true` independently of the serialized request and installed generations. The implementation merely builds the primary-plane loop (line 3186) and does not state how validity is proven.

Have the production request/record carry a typed cursor-state proof or include the required cursor properties. Test the serialized object set and generation relationship directly, including a changed-cursor negative case. The cursor/gamma payload work may remain deferred to stage 4, but stage 2 cannot replace the existing direct path with an unsubstantiated boolean.

### M-6. Task 21 does not account for the slice’s required section 16.3 evidence

Task 21 runs formatting, lint, unit tests, and portable builds only (plan lines 3700–3725). Section 16.3 additionally requires, for these paths:

- Warframe-shaped successor pressure (spec lines 3324–3326);
- continuous synchronized successor promotion and maintenance absorption (lines 3327–3329);
- accept/reject and fence/event reordering with exact ownership (lines 3351–3355);
- producer/out-fence/page-event stalls (lines 3381–3383);
- delayed executors proving core responsiveness (lines 3420–3429);
- restart refusal while an orphaned helper holds the lock (lines 3430–3437).

Section 18 permits physical evidence collection after all implementation stages, so these campaigns need not run at the stage-2 checkpoint. The plan must nevertheless add them to the final-tip evidence manifest and identify the stage-2 source paths that invalidate each row. Currently they are absent rather than explicitly deferred.

### M-7. The exactly-once producer-fd test cannot distinguish one close from multiple closes

The test saves the raw fd, calls the helper, then expects another `close(raw)` to return `EBADF` (plan lines 2948–2957). That proves only that the descriptor is no longer open. It also passes after an accidental double close. Worse, if the fd number is reused during the call, the final `close` can close an unrelated descriptor.

Use ownership instrumentation or a controlled fd wrapper/drop counter. Separately test each terminal path—ready, source error, timeout, cancellation, coalescing, output removal—and assert one release transition.

## Minor

### m-1. Task 19’s focused test command is invalid Cargo syntax

`cargo test -p yserver damage_unknown damage_bundle` (plan line 3575) supplies two positional test filters; Cargo accepts one. Run separate commands or use one common module/filter name.

### m-2. `scanout_damage.rs` is API-compatible but its contract documentation would become false

The real API has exactly the needed operations: `commit_submitted`, `retire_success`, `retire_failure`, and `invalidate` ([scanout_damage.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scanout_damage.rs:184)). However, its module documentation still says applying occurs at page-flip retirement and describes the old two-outcome lifecycle (lines 27–44). The plan’s exit criterion requires the file to remain unmodified (plan lines 3753–3758).

Keeping the implementation unchanged is sound; keeping obsolete normative comments is not. Permit a documentation-only update describing `Accepted`, `HardwareComplete`, proven-prior restore, and unknown invalidation.

## Notes on the rest

The cited live-code anchors were checked against the current tree and are accurate as mutation sites:

- `page_flip.rs:126–186` contains `submit_flip_with_fences` and its atomic call.
- `modeset.rs:1562`, `:1635`, and `:1690` are the direct validation, direct live commit, and composed commit ioctl sites.
- `platform.rs:5163`, `scene.rs:6769`, and `backend.rs:2234` are the corresponding live composed/copy/unflip call sites.
- `scene.rs:1978` and `:4344` are the current damage apply/stage sites.
- `backend.rs:1831`, `:1843`, `:1892`, `:1915`, and `:2273` match the direct submission, successor, supersession, invalidation, and composed-unflip paths.
- `kms/backend.rs:844` is the real device-open site.

The direct request’s canonical out-fence and nonzero event-token tests are appropriately non-vacuous. The retirement ordering, immediate-dispatch instant, single queued successor, pin-pressure, bundle output-set, incomplete-compose invalidation, late-damage survival, direct-bypass, and portable glibc/musl/FreeBSD gates are also well targeted once the submission and damage-event plumbing above is corrected.

The plan correctly preserves `scanout_damage.rs`’s underlying state-machine API: staging at `Accepted`, applying with `retire_success` at `HardwareComplete`, restoring with `retire_failure`, and conservative `invalidate` are all representable without changing its data layout.
