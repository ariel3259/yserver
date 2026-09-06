# Handoff — Phase C.0 stage 2b-i, all seven tasks

**Date:** 2026-09-05
**For:** Antigravity CLI (`agy`)
**Branch:** `feat/phase-c0-atomic-kms-migration`
**Plan:** `docs/superpowers/plans/2026-09-05-phase-c0-stage-2b-i-commit-record-and-slot.md`
**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`

Stage 2a is complete and merged on this branch. **Nothing in 2b-i is
implemented.** All seven tasks are yours.

## Start here

Read `AGENTS.md` first — it is the project's instructions for all agents.
Then invoke `superpowers:executing-plans` (or, if you dispatch subagents via
`invoke_subagent`, `superpowers:subagent-driven-development`) against the plan
above.

Do **not** read the whole plan into context at once: it is 3250 lines. Read the
task you are executing, plus three sections before Task 1 — **Global
Constraints**, **A known pre-existing flake**, and **The normative contract**
(plan lines 74-591). The contract is normative: **a task whose shown code
contradicts it is wrong, and the contract wins.** See R1.

The two finding-disposition tables at the top are history, not instructions.
Skip them unless you want to know why something is shaped the way it is.

## State

| Task | Deliverable | Status |
| --- | --- | --- |
| 1 | The atomic CRTC closure and its re-scan | **yours** |
| 2 | The owning, generic resource ledger | **yours** |
| 3 | The single device slot and its reservation proofs | **yours** |
| 4 | The commit record | **yours** |
| 5 | Request construction | **yours** |
| 6 | The device owner and its typed outcome stream | **yours** |
| 7 | Backend integration, portable gates, reviewability | **yours** |

**Execute them in order.** The dependency chain is real: Task 4's record holds
Task 2's ledger and Task 3's proof, and cannot compile before both. Revision 2
had this backwards and had to tell the executor to skip ahead; revision 3
reordered it so each task compiles on its own.

What already exists that every task builds on — all of it real, readable and
tested at `crates/yserver/src/kms/executor/`:

- The process-isolated `KmsIoExecutor` with `send`, `poll_reply`, `tick`,
  `next_deadline`, `control_fd`, `try_reap`. It performs the ioctl; the owner
  you are building never does.
- `HostCallRequest`/`HostCallReply` protocol v2, `HostCallCorrelation`,
  `HostCallEvent`, `HostCallOutcome`, `UnknownReason`, `HostCallClass`,
  `HostCallPhase`, `OutFenceSlot`, `AtomicPropertyList`.
- `kms/owner/identity.rs`: `IncarnationId`, `CommitId`, `EventToken`,
  `SequenceArmToken`, `ClockEpochId`, `IdentityAllocator`.
- `kms/owner/lifecycle.rs`: `LifecycleEpochId`, `LifecycleTransitionId`,
  `ClockProbeId`.
- `kms::executor::test_support`: `spawn_stub_helper`, `StubBehaviour`,
  `kill_helper`, `wait_readable`.

## The gate — run all of it before every commit

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo test -p yserver
```

CI fails on any clippy warning and `--all-targets` is required — a crate-scoped
or non-`--all-targets` run misses lints in test code and they surface on GH.

**Read R2 before you interpret a failing `cargo test -p yserver`.**

## Rulings — read these before starting

These **override the plan text** where they conflict.

**R1 — where the plan contradicts itself, the contract section wins.**
This plan was revised three times under adversarial review, and three separate
times a fix landed in the prose while the code block it described kept the old
behaviour: the validation lease, `test_ledger`, and half of the slot exclusion.
All three are fixed. If you find a fourth, it is that same pattern — **do not
try to reconcile them and do not guess.** The normative contract (plan lines
139-591) is the model; the task body is an implementation of it. Fix the task
body, and say so when you fold the task back.

**R2 — the full suite is already flaky, and it is not yours.**
`cargo test -p yserver --lib` fails about **10-20% of runs on a clean tree**.
Measured 2026-09-05: 3 failures in 30 runs at `25ee0237`. Three tests are
involved — `kms::executor::tests::early_take_reap_proof_returns_none_and_does_not_invalidate_future_proof`
and two in `kms::executor::device_lock::tests`. The mechanism is the fork/exec
window: `File::open` gives the lock fd `O_CLOEXEC`, so an unrelated
`Command::spawn` does not inherit it past `exec`, but between `fork` and `exec`
the child holds a duplicate of every open descriptor — including a device lock
another test thread is about to drop.

So: **run the suite, and confirm any failure is one of those three named
tests.** A failure anywhere else is yours. Gate your own work on twelve clean
runs of your own targets instead:

```bash
for i in $(seq 1 12); do
  cargo test -p yserver --lib kms::owner 2>&1 | grep -E '^test result:' | grep -q ' 0 failed' \
    || echo "OWNER FLAKE on run $i"
  cargo test -p yserver --test owner_commit_record 2>&1 | grep -E '^test result:' \
    | grep -q ' 0 failed' || echo "INTEGRATION FLAKE on run $i"
done
```

**Never relax an assertion to make a test pass.** That is exactly what hid two
real races in stage 2a — one of them failed 11 runs in 12 while `clippy` and a
single `cargo test` both reported success.

**R3 — Task 3 moves two types out of `executor/mod.rs`. Expect fallout.**
`SubmittingProof` and `ValidationLease` are *defined* in `owner/slot.rs` and
re-exported from `executor/mod.rs`. Stage 2a's tests construct them through
`for_tests` from an external crate (`tests/executor_async.rs` uses it
throughout), so the re-export must keep those paths working. Run the whole
suite after Task 3, not just `kms::owner`.

The plan says `for_tests` stays `pub` and explains why. **Do not try to close
that seam** — no `cfg` gate can admit an integration-test crate and exclude
everyone else, and the fix is a dev-dependency cargo feature, which is a
workspace change this stage deliberately does not bundle.

**R4 — `NeverResource` is uninhabited, and that is the point.**
Task 7 instantiates the production owner as `DeviceCommitOwner<NeverResource>`
over `pub enum NeverResource {}`. `Vec<NeverResource>` is provably empty, so
every ledger transition is trivially correct in production. This is truthful:
2b-i converts no `atomic_commit` call site, so it owns no KMS resource.

**Do not "fix" this by inventing a handle-shaped resource type.** The first
draft did, and it claimed an ownership `spec:2127-2132` requires and this
sub-stage cannot deliver — this crate has no RAII owner for a framebuffer, BO
or pin (`DirectPresentFrame` holds pins as `u64`; `DirectScanoutProbeFramebuffer`
at `drm/modeset.rs:1395` is the only `Drop` in that path). Building those owners
is 2c's conversion work. The type parameter is the seam.

**R5 — four fixture bodies are elided and you must write them.**
`test_fixtures.rs` shows `single_active_crtc()`, `two_crtcs_one_off()`,
`two_active_crtcs()` and `atomic_correlation_for_tests(n)` as
`{ /* ... */ }`. The prose above each says exactly what it contains; write them
out in full rather than deriving one from another, so a change to one cannot
silently retune another's meaning.

`atomic_correlation_for_tests` must use **`EventToken::tagged_for_tests`, never
`for_tests`** — both token decoders check the purpose tag, and an untagged token
is rejected on arrival so the helper answers with a protocol error instead of a
reply. This already broke two stage-1 integration tests once.

**R6 — `test_fixtures.rs` is `#[doc(hidden)] pub`, not `#[cfg(test)]`.**
An integration-test crate links the library built *without* `cfg(test)`, so a
`#[cfg(test)]` fixture is invisible to `tests/owner_commit_record.rs` and a
crate-private one is inaccessible. This is the same seam stage 2a used for
`executor::test_support`. If you find yourself adding a parallel `#[cfg(test)]`
fixture to work around a visibility error, you have hit this — widen the real
one instead.

**R7 — the generic parameter propagates further than the plan shows.**
`R` flows through `CommitRecord<R>`, `LedgerState<R>`, `DeviceCommitOwner<R>`,
`OwnerEvent<R>` **and `DispatchError<R>`**, because the refusal variant carries
`Vec<OwnerEvent<R>>`. Expect it to reach signatures the plan writes without it.
Add the parameter; do not reach for a trait object to avoid it.

**R8 — expect the file list to be incomplete.**
Stage 2a's Task 2 named one file and broke four others — 31 errors in the first
build. That is normal. Task 3's type move and Task 7's `KmsDevice` field are the
two here most likely to spread. Task 7 already tells you to run
`grep -rn 'KmsDevice {' crates/yserver/src` and reconcile rather than trusting
its list, because revision 2's list missed `backend.rs:24229`. **Follow the
compiler to the real blast radius.**

**R9 — the compiler is the fastest reviewer you have.**
This plan had two adversarial review rounds (14 then 10 blocking, all closed).
Deliberately **not** spent on compilability: stage 2a burned four rounds where 7
of the last 10 blockers were compile errors — a non-exhaustive match, private
types used from an external test, unresolved names, `?` in the wrong return
type, struct literals missing a field. Expect the shown code to have that class
of defect and let rustc name the line. Do not reason about whether a block
compiles; build it.

A mechanical sweep before handoff found 3 real dangling symbols in 158 call
sites and fixed them, so the residue should be small — but it is not zero.

**R10 — Task 1 is the probe. Stop if it surprises you.**
Task 1 is pure: no executor, no backend, 20 tests, and it exercises the contract
section directly. If it lands clean, the contract is sound and the rest follows.
If it produces **design** surprises — not compile errors, but the contract
turning out not to describe a workable closure — stop and say so before doing
Tasks 2-7. That costs one task instead of seven.

## Fold your work back into the plan

After each task, in a **separate commit** from the code:

- mark the task's steps `- [x]` and add `**Status: EXECUTED at `<sha>`.**`
- correct the shown code to what actually compiled
- record what the task required beyond its written text

Stage 2a did this at `b39af438`, `0905cbef`, `88821ec3`, `d882dc30`, `a7fa5320`,
`f0ab2f0d` and `25ee0237` — follow that shape. The point is that the plan
converges toward truth instead of drifting further from it with every revision.

## What 2b-ii will need from you

Do not build these, but do not make them harder either. 2b-ii consumes
`CommitRecord::{milestones, fence_evidence}`, `FenceEvidence::by_crtc()`,
`AtomicCrtcClosure::{kernel_event, present_event, expected_completion}`, the
64-entry tombstone ring, and the `OwnerEvent` stream — which it **extends**
with `HardwareComplete`, `Presented` and `Completed`, never a second parallel
event type.

Two things in this plan look broken and are deliberate, each with a stated
reason in "What this sub-stage deliberately leaves looking broken": an accepted
commit never reaches `Completed`, and the device slot is never released after an
acceptance-unknown outcome. Section 6.3 requires fence *status* evidence 2b-i
cannot query, and `COMMIT-6` forbids a second ioctl while acceptance is
unproven. **Do not add a completion path to make them look finished.**

## Commit conventions

From `CLAUDE.md` and `AGENTS.md`: conventional-commit subjects, feature branch,
squash merge only when asked. **Never put a session URL in a commit message.**
Sign commits if you can — `CONTRIBUTING.md` requires Verified commits to merge.
