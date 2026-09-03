# Phase C.0 Stage 2 plan — adversarial review

**Date:** 2026-09-03
**Subject:** `docs/superpowers/plans/2026-09-03-phase-c0-stage-2-device-owner.md`
(21 tasks, 105 steps, reviewed before execution)
**Spec:** `2026-08-26-phase-c0-atomic-kms-migration-design.md` (Approved, revision 2)
**Reviewer:** `codex exec --sandbox read-only`, three independent slices
**Raw output:** `2026-09-03-phase-c0-stage-2-plan-review/slice-{a,b,c}-*.md`
**Disposition:** open. No finding applied yet; no task executed.

## Result

| Slice | Tasks | Blocking | Major | Minor |
| --- | --- | --- | --- | --- |
| A | 1–8 | 9 | 9 | 3 |
| B | 9–15 | 9 | 8 | 2 |
| C | 16–21 | 6 | 7 | 2 |
| **Total** | | **24** | **24** | **7** |

For calibration, the stage 1 plan review found 2 blocking and 3 major. This plan
is not ready for execution by anyone.

The slices agree on what is sound: every file and line anchor in the plan was
checked against the merged tree and confirmed accurate, the six `atomic_commit`
sites and three submission helpers are correctly identified, and the damage
tasks preserve `scanout_damage.rs`'s API without needing to change its data
layout. The defects are in the substance of the owner's integration, not in the
plan's scaffolding.

## The one finding two slices reached independently

**`KmsDeviceOwner::submit` blocks the X11 core.** Slice A B-1 and slice C B-1
are the same defect, found without shared context.

Task 6 calls `self.executor.dispatch(&wire, proof)` synchronously and handles
the outcome before returning. Stage 1's implemented `dispatch`
(`kms/executor/mod.rs:293-380`) polls the helper socket until reply or watchdog
expiry — up to two seconds. Tasks 16 and 17 then route live render and
event-loop paths through that call.

This violates `COMMIT-5` verbatim, and it reintroduces at the integration point
precisely the X11-core stall that section 4.1's process-isolated executor exists
to remove. Process isolation contains a stuck ioctl; it does not make a
synchronous wait on the reply nonblocking. It also prevents the owner from
draining page events while the helper is inside the ioctl, which is exactly what
the `Submitting` interval is specified to do.

It would pass every unit test in the plan and surface on hardware as desktop
freezes.

The fix is structural: `submit` sends and returns, and reply, EOF and watchdog
become event-loop sources that call `on_host_call_outcome` asynchronously. This
is not a patch to one task — it changes the owner's API and therefore tasks 6,
7, 8, 9, 11, 16, 17 and 18.

## Systemic causes

Reading the 55 findings together, five patterns account for most of them.

**1. The plan was designed around the API stage 1 happened to expose, rather
than around the requirement.** The synchronous-dispatch defect above is the
severe case. Slice B B-1 is the same shape: a clock probe has no legal way to
construct the `SubmittingProof` stage 1 demands, because that proof was scoped
to atomic commits and the probe is not one.

**2. Task APIs discard information later tasks require.** `submit` returns
`Result<(), SubmitError>`, so no caller can learn the `CommitId` or the ioctl
outcome — yet task 16 branches on rejection, task 17 needs it before installing
`awaiting_outputs`, and task 18 keys its damage stage by exactly that id
(slice A B-2, slice C B-2). The executor reply carries no incarnation, lifecycle
epoch, transition or commit id, while task 9 validates identities the protocol
does not transport (slice A B-2). `AdmissionContext` reduces admission
compatibility to `fn(MaintenanceIdentity, u32) -> bool`, which cannot inspect
the closure, generations or completion coverage the tiers are defined over
(slice B B-3).

**3. Ownership was written in prose and never in types.** The plan says the
record "transfers every possible old/new resource" before IPC, but `submit`
receives only a `SerializedRequest` — no framebuffer, BO, pin, descriptor or
external-ownership reference reaches it, and task 5 explicitly leaves the
backend as the real owner (slice A B-4). A handle keeps nothing alive. The
spec's uncertainty ledger has no representation.

**4. Tests assert against mocks and injected events instead of driving the real
transition.** The damage tests deliver `DamageEvent` variants by hand, so they
pass even if ioctl acceptance emits nothing, `Presented` emits
`HardwareComplete`, or poison never reaches the backend (slice C M-2). The
round-robin test sets `owed_crtc` manually between selections and nothing in
production ever updates it (slice B B-4). The exactly-once fd test cannot
distinguish one close from two (slice C M-7).

**5. Internal inconsistencies a compiler would have caught.** The atomic frame
head is declared 56 bytes and its fields total 68 (slice A B-3).
`AdmissionChoice::Maintenance` is matched as a tuple variant in one test and a
struct variant in another (slice B B-3, tier 4). This is inherent to a document
containing uncompiled code, and it is exactly what the plan's "run the test to
verify it fails" steps exist to catch — but a plan whose types do not agree
cannot be executed task-by-task without stalling.

## Findings that change the design, not just the code

Beyond the API restructuring, six findings require decisions rather than edits.

- **Tier 3's round-robin rule is implemented backwards** (slice B B-3). The plan
  blocks a successor whose CRTC *is* owed; the spec blocks it when a *different*
  CRTC is owed. Tiers 2, 5, 6 and 7 are also not fully representable in the
  current types.
- **Qualification uses the wrong commit** (slice B B-2). Task 12 opens readiness
  on any completed record with a non-empty expected set, so an ordinary primary
  commit can qualify an incarnation after an unowned legacy modeset. §10.1
  requires the specific install/restore commit. Deferring the real gate to
  stage 3 while claiming stage 2 readiness is not sound.
- **A multi-CRTC Present completes after the first page event** (slice A B-5).
- **`install_validated` accepts an arbitrary request** (slice C B-4), so
  `TEST_ONLY` can validate request A and the owner install request B while
  generations match. §5 requires identical persistent state.
- **The device lock is held by the parent, not the executor** (slice C B-6). If
  the parent dies while a helper is wedged, the lock releases and a new server
  installs state underneath — the exact window `COMMIT-7` exists to close. This
  is inherited from stage 1's placement, not introduced by stage 2.
- **The parked-producer path is unimplementable as written** (slice C B-3). It
  returns `Ok(())`, which the real caller reads as "KMS flip pending" and wedges
  the frame; `transition_to_awaiting_producer` does not exist in `BoState`; and
  task 16 does not list `kms/vk/scanout.rs` among its modified files.

## Two findings against the spec, not the plan

- **§12.1's restore row has no consumer** (slice C M-1). The mapping table
  names "a post-accept failure whose prior state is proven still current →
  restore", but tasks 18 and 19 define only `Accepted`, `HardwareComplete` and
  `Unknown`, so `retire_failure` is never called. Either the plan gains that
  disposition or the spec row is unreachable and should say so.
- **`scanout_damage.rs`'s module documentation would become false** (slice C
  m-2). Its header still describes applying at page-flip retirement and the old
  two-outcome lifecycle. The plan's exit criterion forbids modifying the file.
  Keeping the implementation unchanged is right; keeping obsolete normative
  comments is not, so the criterion should permit a documentation-only update.

## Recommended disposition

Do not execute this plan. Do not delegate it.

The scaffolding is sound and the spec fidelity of the non-owner tasks was
confirmed, so this is a rework rather than a discard. The order that minimises
churn is:

1. Settle the owner's asynchronous submission API and its outcome/identity
   surface first. Tasks 6 and 9 are rewritten; 7, 8, 11, 16, 17 and 18 are
   re-anchored onto the new API.
2. Give submission a typed owned-resource ledger, and give the executor reply
   the full correlation tuple. These are protocol and type changes that
   invalidate the frame layout in task 2, so fix the 56-versus-68 arithmetic in
   the same pass.
3. Rebuild task 14's admission candidate as a typed, generation-carrying value
   produced by the request builder, and move round-robin state into the
   scheduler. Re-derive all seven tiers from the spec rather than editing the
   current encoding.
4. Redo the affected tests so each drives a real transition and would fail if
   the mapping were wrong.
5. Apply the remaining major and minor findings, and re-review the rewritten
   tasks before execution.

The review cost roughly one hour of wall time. Finding the synchronous stall
during execution would have cost the integration of every task built on top
of it.
