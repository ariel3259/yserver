# Phase C.0 Stage 2 plan — adversarial review, round 2

**Date:** 2026-09-03
**Subject:** `docs/superpowers/plans/2026-09-03-phase-c0-stage-2-device-owner.md`
(revision 2: 23 tasks, 116 steps, 5310 lines)
**Round 1:** `2026-09-03-phase-c0-stage-2-plan-adversarial-review.md`
**Reviewer:** `codex exec --sandbox read-only`, three independent slices
**Raw output:** `2026-09-03-phase-c0-stage-2-plan-review-round2/slice-{a,b,c}-*.md`
**Disposition:** open. Do not execute, do not delegate.

## Result

| | Round 1 | Round 2 |
| --- | --- | --- |
| Blocking | 24 | **26** |
| Major | 24 | **26** |
| Minor | 7 | **9** |

Revision 2 has more defects than revision 1.

Regression check over the 54 round-1 findings re-examined:

| Verdict | Count |
| --- | --- |
| FIXED | 10 |
| PARTIAL | 21 |
| TRADED — fixed by introducing a different defect | 10 |
| NOT FIXED | 13 |

## What actually got fixed

Ten findings closed cleanly, and they share a shape: each was a **local,
self-contained fact** with no consumers elsewhere in the plan.

- The atomic frame head, corrected to 68 bytes with explicit offsets and a
  golden-layout test (A B-3).
- The out-fence bitmap validated against the declared slot mask before
  descriptor cardinality (A M-2).
- Event dispatch by record variant, with both wrong-type contradiction tests
  (A M-3).
- Atomic `EBUSY` closing readiness and requesting bounded recovery instead of
  poisoning (A M-4).
- The commit record's device generation, topology generation, closure and
  completion sets (A M-6).
- A tombstone test that actually terminalizes a record and resolves its token
  (A M-9).
- Round-robin state moved inside the scheduler and advanced by `select`
  (B B-4).
- Maintenance created already aged when offered behind a submitted commit
  (B B-5).
- Producer-fd exactly-once coverage by close counting rather than an `EBADF`
  probe (C M-7).
- The invalid two-filter Cargo command (C m-1).

## Why the rest did not close

Almost every PARTIAL and TRADED verdict says the same thing in different words:
the paragraph the finding pointed at was corrected and the rest of its task was
left speaking the old API.

- Task 13 declares `lifecycle_hardware_deadline(Duration) -> Duration` while its
  implementation is `Option<Duration> -> Result<_, CohortUnvalidated>` (B B-7).
- Task 12 tests `milestones.presented_crtcs`; task 6 defines `presented: bool`;
  task 12's implementation still sets the boolean (A B-5, A B-8).
- Task 12's test returns `EventDisposition::StagedPendingAcceptance`, which the
  declared enum omits (A B-9).
- Tasks 20 and 21 still define, inject and match `DamageEvent`, which the
  revision 2 architecture section normatively removed, and task 20 invents
  `OwnerEvent::PriorStateProven`, which the normative enum does not contain
  (C B-5).
- Task 17 calls its own four methods in incompatible shapes despite claiming one
  signature (B M-8).
- Task 3's tests still call `open_any_drm_or_skip` and still say "PASS (or
  SKIP)" beside the deterministic design that replaced it (A M-7).
- Task 8 redeclares `FenceSlotState` after task 6 defines it (A B-8).

This is a direct consequence of the rework method: targeted replacements on
individual paragraphs, without re-reading each task as a whole afterwards. The
residue scan that should have caught it grepped for `DamageEvent`, saw the first
two hits in the architecture section, judged them intentional, and never looked
at the rest because the output was truncated to five lines.

## Defects the rework introduced

The asynchronous rewrite is sound in principle and incomplete in practice.

- **No production path services replies or watchdogs** (A B-1). Task 4 states
  the caller contract in prose — the core registers `control_fd()` and calls
  `tick(now)` — but no task modifies the core event loop to do it. The
  asynchronous design has no consumer.
- **Watchdog expiry releases executor serialization before helper reap**
  (A B-2), contradicting `ExecutorStalled`.
- **Late replies and their fds are lost after watchdog expiry** (A B-3). A
  delivered out-fence must be adopted and closed exactly once into quarantine;
  nothing does.
- **Send failure leaves an installed record with no terminal path** (A B-4). The
  record is installed before the frame is sent, by design; the failure branch
  never terminalizes it.
- **The probe and `TEST_ONLY` validation became synchronous again** (B B-1,
  C B-3). Both were rewritten to send-and-return, and both kept tests expecting
  an immediate terminal result. This is round 1's top finding reintroduced in
  two places.
- **`CompletionUnknown` damage is emitted twice** (C B-6), once by the event and
  once by `DamageInvalidate`, contradicting the exactly-once test written in the
  same batch.
- **The parent's lock handoff unlocks the helper's lock** (C B-7). `flock` is
  associated with the open file description and survives `execve`, which is why
  the handoff is possible — but `DeviceLock::drop` calls
  `flock(fd, LOCK_UN)` explicitly, and `LOCK_UN` through any descriptor sharing
  that description releases it globally. This document's author verified the
  close semantics in the manual page and did not check the destructor already
  read earlier in the same session.
- **The exit criterion contradicts the architecture section** on qualification
  (C B-8): one requires the explicit install/restore commit, the other accepts
  the first commit with a non-empty expected set.

## The size signal

The most useful number here is not the defect count but its relationship to plan
size, across three data points from this project:

| Plan | Tasks | Lines | Blocking |
| --- | --- | --- | --- |
| Stage 1 | 14 | 1999 | 2 |
| Stage 2 revision 1 | 21 | 3784 | 24 |
| Stage 2 revision 2 | 23 | 5310 | 26 |

Defect density rises far faster than size. Stage 1 was written the same way, by
the same author, against the same spec, and reviewed by the same mechanism; it
came back nearly clean. The plausible reading is that a plan of this size cannot
be held internally consistent in one pass by this workflow, and that no number
of correction rounds fixes that, because each round edits a document too large
to re-verify as a whole.

## Recommended disposition

Do not execute. Do not delegate. Do not attempt a third round of patches — that
method produced this state and there is no reason to expect the third
application to converge when the second regressed.

Two viable paths:

1. **Split stage 2 into three separately planned stages** along the seams the
   review itself keeps drawing: the executor and wire substrate (tasks 1–4), the
   owner and its completion evidence (5–14), and the conversions plus damage
   (15–23). Each lands near stage 1's size, where this process demonstrably
   worked. The spec's section 18 already permits stage boundaries that are not
   merge boundaries, so this needs no spec change.
2. **Rewrite each task as a whole unit** — heading, interfaces, tests and
   implementation together — verifying internal consistency before moving to the
   next, and re-reviewing per group rather than per plan.

Path 1 is preferred on the evidence. Path 2 is the same workflow that has now
failed twice at this size.
