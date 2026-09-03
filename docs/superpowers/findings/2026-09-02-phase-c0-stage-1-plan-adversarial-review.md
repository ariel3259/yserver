# Phase C.0 Stage 1 plan — adversarial review

**Date:** 2026-09-02
**Subject:** `docs/superpowers/plans/2026-09-02-phase-c0-stage-1-executor-substrate.md`
(14 tasks, reviewed before execution)
**Spec:** `2026-08-26-phase-c0-atomic-kms-migration-design.md` (Approved 2026-09-02)
**Reviewer:** independent Opus session
**Disposition:** all findings applied 2026-09-02 before any execution; see the
disposition note at the end.

## The two decisions submitted for review

**`ReapProof` as a newtype: approve.** Turning "a signal is not reap proof" into
a type error is the right instinct, and it is the one invariant in `COMMIT-7`
that is easiest to violate by accident during a refactor. One constraint to make
explicit in task 11: `ReapProof` must not be `Copy` or `Clone`, and `release`
must take it by value. The spec says a lease is released "exactly once"; a
copyable proof would let one wait status release several leases, which is
precisely the accounting error the type exists to prevent.

**Purpose tag in the token high bits: approve.** It closes a real
misattribution path at zero runtime cost. One constraint: section 6.1 defines
the event token as allocated "from one monotonic namespace across the complete
device incarnation." Tag the purpose, but keep a single shared counter behind
both token types rather than two independent per-purpose counters. Two counters
would satisfy the uniqueness test in task 5 while quietly breaking the
one-namespace property the spec relies on, and nothing downstream would notice
until two tokens of different purposes shared a counter value in a log.

## Blocking

### B-1. The stage exit criteria contradict their own note

The criterion asserted that every KMS host call in `drm/page_flip.rs` and
`drm/modeset.rs` reaches the device through `KmsIoExecutor`, then withdrew
exactly that work in a note in the same bullet. Those six calls *are* the KMS
host calls in those two files. An agent executing this plan cannot determine
whether stage 1 is finished, and a reviewer reading only the criteria will
conclude the stage failed.

The deferral itself is right and well argued — the call sites need the owner's
admission and completion model. Only the criterion is wrong.

**Requested disposition.** Rewrite the bullet to what stage 1 delivers, and move
the conversion statement to a "stage 2 consumes" section so it is not phrased as
a stage-1 outcome at all.

### B-2. The reply frame carries no correlation identity

`AtomicRequest` carries `commit` and `event_token`. `HostCallReply` carries
`helper_duration_ns`, `errno` and `out_fence_count`, and nothing identifying
which request it answers. Section 6.3's rules for recognising a stale result
presuppose that the parent can tell which call a reply belongs to; as specified
it can only assume the reply answers whatever is outstanding.

The one-in-flight discipline makes this hard to hit today, which is exactly why
it should be closed before stage 2 adds recovery paths that respawn helpers and
reuse sockets.

**Requested disposition.** Put the `CommitId` — or a monotonic per-executor
request sequence, which also covers the clock-probe class — in every reply
frame, and reject a mismatched reply as `UnknownReason::MalformedReply`.

## Major

### M-1. `COMMIT-6`'s ordering is enforced by nothing in stage 1

The owner installs `Submitting` and the fd lease before IPC dispatch, but the
owner is stage 2 and `dispatch` can be called by anything holding an executor.
The invariant lives only in prose for the whole of stage 1 and lands in stage 2,
where it is easiest to get wrong because that is where the call sites move.
Apply the `ReapProof` idiom: have `dispatch` require a token only the owner's
`Submitting` installation can construct, defined in stage 1 even though its
producer arrives in stage 2.

### M-2. Nothing cross-checks the declared fence count against the fds received

Task 8 treats `MSG_CTRUNC` as an error but does not require
`received.fds.len() == out_fence_count`. A helper that miscounts produces a short
fence list with no flag set — and the plan's own rationale says that surfaces
later as a missing completion, the symptom hardest to trace to its cause.

### M-3. The watchdog is a free `Duration` chosen by the caller

Both values are normative and class-derived: 2 seconds seat-active `NONBLOCK`,
30 seconds cold-start or final-offline blocking. A caller-chosen `Duration` lets
a wrong call site produce a wrong bound silently, and the failure then looks like
a driver problem rather than a plumbing one. Derive it from the request class
inside the executor.

## Minor

### m-1. `LockAvailability::HeldByLiveHelper` names a conclusion the evidence does not support

`flock` returning `EWOULDBLOCK` proves the lock is held. It does not prove the
holder is a yserver helper, or that it is wedged. Naming the variant
`HeldByLiveHelper` asserts provenance the mechanism cannot establish — the same
error the spec spent several rounds removing from
`AuditedCursorExpansionHazard`. Rename to `Held` and record separately whatever
the start-time check actually knows.

Second, `check_available` and `acquire` as separate operations is a
check-then-act race. `may_install_state` should acquire and hold, returning the
guard.

### m-2. Task 3 does not state the drain's termination condition

The test drives the drain from a pipe closed by the writer, so it terminates on
EOF. A real DRM fd is non-blocking and terminates on `EAGAIN`. Whether a
single-read drain is safe depends on trigger mode: level-triggered epoll
re-wakes; edge-triggered does not, and the residue sits until an unrelated event
wakes the fd, presenting as a permanently missing completion. State the
termination condition and which mode the four call sites use.

## Notes on the rest

The reuse of `internal_probe.rs` as the mould is the right call and is the
single biggest risk reduction in the plan. The finding that there are four
`drain_events` call sites rather than one, that two live in
`present/event_loop.rs`, that the six real `atomic_commit` sites are a small
surface and that `drm/` is 3,126 lines materially changes how risky stage 2
looks compared to the 39,000-line figure that has been shaping this
conversation. Single-closure `drain_device_events` making the drain
single-reader by construction is a good structural choice.

---

## Disposition

All findings applied before execution.

**B-1 — applied.** The exit criterion now states what stage 1 delivers and says
explicitly that no production `atomic_commit` call site is converted. A new
"What stage 2 consumes" section carries the conversion, `SubmittingProof`'s
producer and `may_install_state`'s production call site.

**B-2 — applied.** A `RequestSeq` is carried by every request and echoed in
every reply, covering the clock-probe class as well as the atomic one, with a
round-trip test and a mismatch mapped to `UnknownReason::MalformedReply`.

**M-1 — applied.** `dispatch` takes `SubmittingProof`, defined in stage 1 with a
test-only constructor and given its real producer in stage 2.

**M-2 — applied.** `adopt_reply` requires the declared count to equal the fds
received, with a test that sends a reply declaring two fences carrying one and
no `MSG_CTRUNC`.

**M-3 — applied.** The watchdog is gone from the `dispatch` signature.
`HostCallClass` derives 2 s and 30 s from the request class.

**m-1 — applied.** `check_available` is removed; `may_install_state` acquires
and returns the guard. The variant naming argument is recorded in the task, and
whatever the check knows travels separately as an explicitly advisory
`HolderRecord`. A third test was added for the property `COMMIT-7` actually
rests on: a `SIGKILL`ed holder still releases the lock.

**m-2 — applied, and the answer is stricter than the finding assumed.** The two
call-site families are on different pollers. `present/event_loop.rs:58`
registers the DRM fd with raw epoll and `EPOLLIN` alone, with no `EPOLLET`, so
it is level-triggered. The KMS backend fd goes through the core poller at
`core_loop/run.rs:1058`, which is mio 1.x (`Cargo.toml:29`), and mio registers
epoll sources edge-triggered. So one of the two paths genuinely cannot recover a
residue. The drain now loops to `EAGAIN` unconditionally rather than depending
on the poller a call site uses, the early `read < DRAIN_BUFFER_LEN` return was
deleted, and a test writes more events than one buffer read can return.

**Both approvals applied with their constraints.** `ReapProof` is neither `Copy`
nor `Clone`, `release` takes it by value, and a compile-fail test pins that.
Both token purposes draw from one counter, with the reason recorded in the
doc comment.
