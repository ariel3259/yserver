# Stage 2 plan review round 2 — slice C, tasks 18-23

Raw output of `codex exec --sandbox read-only`, 2026-09-03, against plan revision 2.

## Regression check

C B-3 — TRADED — The rewrite adds `AwaitingProducer`, but places an owned `PendingProducer` inside a phase model whose real `BoPhase` is `Clone + Copy` and whose `BoState` separately owns raw fence fields (`scanout.rs:109-137,356-369`); it also depends on `DamageStageEntry`, which Task 20 defines only after Task 18 (`plan:4270-4284,4753-4766`).

C B-4 — NOT FIXED — `ValidatedSnapshot` binds the tested property list, but the production instruction still builds a separate request and calls ordinary `owner.submit` instead of `install_validated(snapshot)` (`plan:4539-4560,4569`).

C B-5 — PARTIAL — Rejection and unknown dispositions are now described, but Tasks 20–21 still use the removed `DamageEvent` API and duplicate unknown invalidation through both `CompletionUnknown` and `DamageInvalidate` (`plan:4598-4602,4724-4749,5027-5044`).

C B-6 — TRADED — Moving the lock toward the helper is correct in principle, but dropping the parent’s current `DeviceLock` executes `LOCK_UN`, which releases the shared open-file-description lock even while the helper retains a duplicate (`device_lock.rs:185-191`; `plan:5163-5166`).

C M-1 — TRADED — The restore row gains a proposed producer, but `OwnerEvent::PriorStateProven` is absent from the normative `OwnerEvent` enum and Task 7’s interface (`plan:97-106,1963-1965,4746-4749`).

C M-2 — PARTIAL — One new table-driven test exercises real unknown routes (`plan:4851-4872`), but most damage tests still inject the nonexistent `DamageEvent` directly (`plan:4620,4644-4647,4838-4840,4945-4951`).

C M-3 — NOT FIXED — `CommitRecord.outputs` was added, but neither `SerializedRequest` nor `submit(request, class, resources)` supplies renderer output indices, and no task defines `record_outputs` (`plan:83-87,1231,1833-1859,1884-1886,5041`).

C M-4 — TRADED — Snapshot ownership is intended to make the lease RAII, but Task 19 presents incompatible `validate` signatures and a synchronous result for an asynchronous host call (`plan:4372-4380,4384-4390,4400-4404,4553-4567`).

C M-5 — PARTIAL — The tests now demand a typed `CursorProof`, but the implementation section never creates, records, or checks that proof; it only describes primary-plane properties and ordinary submission (`plan:4493-4519,4530-4569`).

C M-6 — PARTIAL — Task 23 lists the missing campaigns, but gives task numbers rather than exact source paths and does not name or create an evidence-manifest artifact containing the source tip, hashes, identities, workload, and sensitivity data required by §18 (`plan:5221-5238`; `spec:3879-3897`).

C M-7 — FIXED — The EBADF probe was replaced by close-count instrumentation and covers success, error, timeout, cancellation, coalescing, and output removal (`plan:4169-4218`).

C m-1 — FIXED — The focused command now uses one Cargo filter: `cargo test -p yserver damage_` (`plan:5020-5023`).

C m-2 — PARTIAL — The exit criteria now permit a documentation-only update, but neither damage task lists or instructs modification of `scanout_damage.rs`, whose current contract still says apply at page-flip retirement (`plan:4590-4593,4823-4825,5271-5272`; `scanout_damage.rs:27-44`).

## Blocking

### B-1. Task 18’s `AwaitingProducer` state cannot be implemented as written

The real model is:

- `BoPhase`, a fieldless `Clone + Copy` enum (`scanout.rs:109-137`);
- `BoState`, a struct with `phase`, `in_fence_fd`, and `release_fence_fd` (`scanout.rs:356-369`).

The plan inserts `AwaitingProducer(PendingProducer)` as though the phase itself could own a non-`Copy` ledger, framebuffer, `OwnedFd`, protocol key, and damage stage (`plan:4263-4284`). It says neither whether this replaces `BoPhase`, becomes storage in `BoState`, nor which derives and every exhaustive match must change.

It also creates an impossible task ordering: `PendingProducer.damage` uses `DamageStageEntry` in Task 18 (`plan:4283`), but that type is not introduced until Task 20 (`plan:4753-4766`).

Finally, Task 18 says `submit_copied_scanout` “keeps its signature” (`plan:4129`) and later changes its return type to `CopiedSubmission` (`plan:4287-4298`). This task cannot compile incrementally.

### B-2. Copied-scanout ownership still has no asynchronous owner-outcome transition

After the producer becomes ready, `owner.submit` returns only a `CommitId`; acceptance or rejection arrives later through `OwnerEvent`. Nevertheless, Task 18 still speaks of a local “rejection arm” immediately after `owner.submit` (`plan:4317`), and its rejection test inspects `ReleasedButAtomicRejected` immediately after calling `submit_copied_scanout` without draining an owner reply (`plan:4230-4238`).

The current state machine transitions synchronously from `Submitted` to either `Pending` or `Recording` around `atomic_commit` (`platform.rs:5160-5185`). The rewrite defines no replacement handlers that:

- keep the destination uncertainty-owned while `Submitting`;
- move it on `OwnerEvent::Accepted`;
- restore `ReleasedButAtomicRejected` on `OwnerEvent::Rejected`;
- quarantine it on `CompletionUnknown`;
- reconcile the owner-held out-fence with `BoState`, which currently stores a release-fence fd.

Thus the producer wait is improved, but the subsequent asynchronous BO transition is missing.

### B-3. Task 19 turns seat-active validation back into a synchronous API

The corrected architecture requires host-call replies to arrive through `on_control_readable`; the X11 core must not wait. Task 19 nevertheless writes `owner.validate(...).expect("validate")` and immediately obtains a `ValidatedSnapshot` (`plan:4400-4416`). Its implementation says validation “dispatches” a `SeatActiveValidation` host call (`plan:4562-4567`) without defining an in-progress validation state or an eventual validation-result event.

A delayed `TEST_ONLY` therefore either blocks inside `validate`, violating `COMMIT-5` (`spec:635-653`), or cannot return the advertised snapshot. The timeout test has the same synchronous assumption (`plan:4383-4397`).

The APIs also disagree within the task: one test manually acquires a lease and calls `validate(request, &lease)` (`plan:4372-4379`), while later tests call `validate(request)` (`plan:4400-4404,4412-4416`).

### B-4. Production direct submission bypasses the request-bound validation snapshot

The snapshot design itself is sound: it owns the exact persistent list and `install_validated(snapshot)` permits no alternate request argument (`plan:4539-4560`). But the production instruction immediately contradicts it:

> “builds a request and calls `owner.submit(request, ...)`” (`plan:4569`).

That bypasses the snapshot, lease, digest, generation recheck, and exact tested/live equality required by §5 (`spec:298-329`). Task 19 must specify that the direct path consumes the validated snapshot through `install_validated`, or explicitly establish that this `TEST_ONLY` is merely candidate qualification and cannot authorize the live install. It currently claims both models.

### B-5. Tasks 20–21 contradict the normative one-stream architecture

Revision 2 normatively removes `DamageEvent` and defines one `OwnerEvent` stream (`plan:92-110`). Task 20’s interface recreates `DamageEvent` and `take_damage_events` (`plan:4595-4602`), its tests inject `DamageEvent`, and its implementation matches a mixture of `DamageEvent` and `OwnerEvent` (`plan:4770-4796`). Task 21 continues to consume and inject `DamageEvent` (`plan:4827-4829,4838-4840`).

Conversely, Task 20 adds `OwnerEvent::PriorStateProven` (`plan:4746-4749`), but that variant does not exist in the normative enum (`plan:97-106`) or Task 7’s declared output (`plan:1963-1965`).

This is not merely stale test spelling: it leaves no coherent routing contract for the core to drain once and distribute events without one consumer stealing events from another.

### B-6. Completion-unknown damage is emitted twice

Task 7 says central `terminalize` emits `DamageInvalidate` when required (`plan:2272-2277`) and its unknown arm also emits `OwnerEvent::CompletionUnknown` (`plan:2263-2266`). Task 20 maps both `CompletionUnknown` and `DamageInvalidate` to invalidation (`plan:4733-4740`), while Task 21 directly invalidates on `CompletionUnknown` (`plan:5030-5044`).

A single unknown can therefore call `ScanoutDamage::invalidate` twice, contradicting the new exactly-once test (`plan:4851-4872`) and the stage exit criterion (`plan:5266-5270`). One event must own the damage disposition; the other must not independently repeat it.

### B-7. Task 22’s parent close explicitly unlocks the helper’s lock

The plan’s factual premise is correct: on Linux, `flock` is associated with the open file description, survives `execve`, and duplicated descriptors share it. That same rule is why the proposed handoff fails with the existing guard.

`DeviceLock::drop` calls `flock(fd, LOCK_UN)` before closing (`device_lock.rs:185-191`). `LOCK_UN` through any descriptor sharing the open file description releases the lock globally. Therefore “the parent closes its copy” after the helper reply (`plan:5163-5166`) unlocks the file unless the plan explicitly consumes the guard without running this destructor or removes explicit unlocking and relies on last-close semantics.

The task also cannot implement the promised inheritance with its file list. Passing another descriptor across the current re-exec requires changes analogous to `CONTROL_FD`/`KMS_FD` in `executor/mod.rs:32-33,589-609,625-663` and adoption in `executor/helper.rs:72-76`; Task 22 lists only `kms/backend.rs` and `device_lock.rs` (`plan:5077-5080`).

Consequently there is either an unlock at parent handoff or no inherited lock fd at all.

### B-8. The stage exit criterion knowingly permits false qualification

The normative architecture and §10.1 require the specific real install/restore commit to qualify readiness (`plan:1844-1846`; `spec:2041-2057`). Yet the exit criterion accepts “the first commit with a non-empty `ExpectedCompletionCrtcs`” (`plan:5263`), and “What stage 3 consumes” explicitly defers correcting that behavior from the first primary commit (`plan:5284-5287`).

An ordinary primary cannot stand in for the required installation. This directly contradicts both the spec and revision 2’s own corrected architecture.

## Major

### M-1. `CommitRecord.outputs` has no producer

Task 6 adds `outputs: Vec<usize>` and explains why it is needed (`plan:1858,1884-1886`), but:

- `SerializedRequest` has no output-index field (`plan:1231`);
- `KmsDeviceOwner::submit` accepts only request, class, and resources (`plan:83-87`);
- `ResourceLedger` has no output mapping (`plan:1892-1904`);
- no task defines `record_outputs(commit)`, although Task 21 calls it (`plan:5041`).

The fallback for poison/unknown therefore remains unimplementable, and §12.1’s exact output set is not enforced.

### M-2. The cursor proof remains test-only prose

The two tests require `CursorProof` and generation invalidation (`plan:4493-4519`), but the production instructions never define the enum, attach it to `SerializedRequest` or `CommitRecord`, build it from installed cursor state, or recheck it before dispatch (`plan:4530-4569`).

Section 12’s direct-entry cursor guarantee (`spec:2361-2364`) is therefore not implemented by any plan step.

### M-3. The evidence manifest step is not actionable

Section 18 requires an evidence manifest containing exact source tip, tested paths, dependency/build hashes, module/kernel identities, workload, invalidation rationale, and sensitivity class (`spec:3879-3897`). Task 23 supplies only a table of campaign names and task numbers (`plan:5228-5238`), does not name a manifest file in its file list (`plan:5190-5192`), and its commit command broadly stages `docs/superpowers/findings/` without specifying any artifact (`plan:5244-5248`).

This only appears to fix C M-6; an executor following the plan has nowhere defined to write the required evidence metadata.

### M-4. Task 18’s commit omits its essential state-machine edit

Task 18 correctly lists `kms/vk/scanout.rs` as modified (`plan:4116-4121`), but its commit command omits that file (`plan:4330-4334`). Executing tasks commit-by-commit would leave the foundational `AwaitingProducer` change outside the task’s commit and make review/bisect evidence misleading.

### M-5. The device-lock no-gap test is underspecified and racy

`LockContentionProbe::spawn_for_tests` starts before the parent acquires the lock (`plan:5117-5122`). Without an explicit barrier ensuring the parent has acquired before the probe begins attempts, the probe can win first and make `open_kms_device_for_tests` fail instead of measuring a handoff gap.

The test must synchronize acquisition, continuously contend specifically during fork/exec/handoff, and stop only after helper ownership is acknowledged.

### M-6. Lifecycle damage invalidation is declared but not enforced in this stage

Task 21 says Task 12 poison, Task 15 terminalization, and “stage 3’s lifecycle transitions” will call the invalidation helper (`plan:5057`). Tests for recovery/topology/VT/device-loss merely call `invalidate_damage_for_outputs` directly (`plan:4921-4938`); they do not verify that any real lifecycle path emits or routes `OwnerEvent::DamageInvalidate`.

Thus §12.1’s poison/recovery/topology/VT/device-loss row (`spec:2434-2436`) remains unconnected at the end of Stage 2.

### M-7. The portable gate is weaker than the spec’s final gate

Task 23 runs `cargo test -p yserver` and un-locked builds (`plan:5194-5209`). The spec’s final acceptance explicitly requires `cargo test --all-targets --locked` (`spec:3830-3834`). The plan’s gate can miss target-specific tests and dependency-lock drift.

## Minor

### m-1. Task 20 still names a removed test target

Its expected failure says "`DamageEvent` and the held-stage plumbing do not exist" (`plan:4715-4718`) even though the normative rewrite says no `DamageEvent` should ever exist. This directs implementation toward the obsolete API.

### m-2. `scanout_damage.rs` documentation is permitted but never scheduled

The exit criteria now allow the update (`plan:5271-5272`), but no task lists the file or describes the replacement text. Its current documentation still states that state is applied at page-flip retirement and describes only the old two-outcome model (`scanout_damage.rs:27-44`).

### m-3. The self-review repeats a now-false type-order claim

The self-review says `FenceSlotState` is declared in Task 5 and defined in Task 7 (`plan:5297`), while revision 2 actually defines it completely in Task 6 (`plan:1828-1831,1861-1869`). This is documentary residue, but it undermines the claimed type-consistency audit.

## Notes on the rest

The `flock` open-file-description premise itself is sound: duplicate descriptors share the lock, and it survives `execve`. The defect is the existing explicit `LOCK_UN` destructor and the missing third inherited-fd plumbing, not the kernel premise.

The request-bound snapshot shape—immutable persistent list, state-affecting flags, digest, generations, and snapshot-owned lease—is a sound correction in isolation (`plan:4539-4560`). It fails only because the task’s async API and production call path do not use that design consistently.

The §6.3 composed/direct builder instructions correctly use old/new plane bindings, declare active CRTCs, omit C.0 input fences, and leave canonical out-fence insertion to `finish` (`plan:4257-4321,4530`). No additional closure defect was found in Tasks 18–19.

The §10.4 predecessor/successor ordering and separate Present-versus-release rules are handled by Task 17, and Tasks 18–23 introduce no clear new contradiction there beyond the missing asynchronous BO-outcome wiring identified above.

The replacement close-count tests, the corrected single Cargo filter, and the explicit enumeration of deferred §16.3 campaigns are genuine improvements, even though the evidence manifest remains incomplete.
