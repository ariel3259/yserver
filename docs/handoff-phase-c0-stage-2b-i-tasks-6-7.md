# Handoff — Phase C.0 stage 2b-i, tasks 6-7

**Date:** 2026-09-05
**For:** Codex CLI (`codex exec`), `codex-cli 0.153.4`
**Branch:** `feat/phase-c0-atomic-kms-migration`
**Plan:** `docs/superpowers/plans/2026-09-05-phase-c0-stage-2b-i-commit-record-and-slot.md`
**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`

**Supersedes** `docs/handoff-phase-c0-stage-2b-i.md`, which handed all seven
tasks to the Antigravity CLI. That agent executed tasks 1-5 and ran out of
budget. Its rulings still hold and are restated here; read this document, not
that one.

## How to invoke

Unlike the review path, implementation needs write access. `review.sh` pins
`--sandbox read-only`; that is correct for a reviewer and wrong here.

```bash
cd /home/ariel_santangelo/Projects/yserver-phase-b
codex exec --sandbox workspace-write \
  "Read docs/handoff-phase-c0-stage-2b-i-tasks-6-7.md and execute tasks 6 and 7." \
  < /dev/null
```

`< /dev/null` is required on this machine — without it `codex exec` waits on
stdin and hangs.

Codex reads `AGENTS.md` natively, which is the half of the previous handoff's
bootstrap that carries over. The other half does not: the Superpowers skills
are a Claude Code plugin and nothing loads them for you. They are readable
files, so if you want the execution discipline the previous agent followed:

- `~/.claude/plugins/cache/claude-plugins-official/superpowers/6.3.0/skills/executing-plans/SKILL.md`
- `~/.claude/plugins/cache/claude-plugins-official/superpowers/6.3.0/skills/using-superpowers/references/codex-tools.md`
  — Codex-specific notes, including that subagent dispatch needs
  `[features] multi_agent = true` in `~/.codex/config.toml`.

You do not need subagents for two tasks. Single-threaded is fine and cheaper.

**One caveat worth knowing:** codex is also this repo's *review* instrument —
`docs/superpowers/review/review.sh` pins it to adversarially review plans, and
it produced the two review rounds this plan survived. You are now implementing
a plan your own model family reviewed. For 2b-i that is mostly moot, since
those rounds are closed and their findings are folded in. But do not treat the
plan as pre-validated by a peer: it was reviewed as a *document*, and no line
of tasks 6-7 has ever been compiled.

## State

| Task | Deliverable | Status | Commit |
| --- | --- | --- | --- |
| 1 | The atomic CRTC closure and its re-scan | done, 25 tests | `7a28f69d` |
| 2 | The owning, generic resource ledger | done, 4 tests | `9fd6721e` |
| 3 | The single device slot and its reservation proofs | done, 11 tests | `a5acdd40` |
| 4 | The commit record | done, 10 tests | `72498c2f` |
| 5 | Request construction | done, 11 tests | `d7aebe21` |
| 6 | The device owner and its typed outcome stream | **yours** | — |
| 7 | Backend integration, portable gates, reviewability | **yours** | — |

The tree is clean, `cargo clippy --all-targets -- -D warnings` passes, and
`cargo test -p yserver --lib kms::owner` is 75 green. Task 5 was implemented by
the previous agent but committed here after verification, which is why its
commit is the only one not in that agent's own sequence.

Read the plan's Task 6 and Task 7 sections, plus three sections before Task 1 —
**Global Constraints**, **A known pre-existing flake**, and **The normative
contract** (plan lines 74-591). Do not read all 3250 lines. The two
finding-disposition tables at the top are history, not instructions.

## What tasks 1-5 already built, and that you must not re-derive

All of it is real and readable under `crates/yserver/src/kms/owner/`:

- `closure.rs` — `AtomicCrtcClosure::{compute, verify_serialized}`, `ObjectKind`,
  `SerializedObject`, `CrtcPower`, `PropertyIds`, `FencePolicy`, `ClosureError`.
- `ledger.rs` — `Submitted<R>`, `Accepted<R>`, `Rejected<R>`, `Quarantined<R>`,
  `LedgerState<R>`. Transitions consume `self`; `Rejected::into_current` is how
  the still-current old state leaves without being dropped.
- `slot.rs` — `DeviceSlot::{reserve, release, acquire_validation,
  consume_validation, abandon_validation}`, and the definitions of
  `SubmittingProof` / `ValidationLease`, re-exported from `executor/mod.rs`.
- `record.rs` — `CommitRecord<R>`, `Milestones`, `TerminalState`,
  `FailureCause`, `RefusalCause`, `UnknownCause`, `RecordState`,
  `FenceEvidence`, `Tombstone`.
- `build.rs` — `CommitDescription`, `build_atomic_request`,
  `same_persistent_properties`.
- `test_fixtures.rs` — `#[doc(hidden)] pub`, not `#[cfg(test)]`. Already holds
  `single_active_crtc`, `two_crtcs_one_off`, `two_active_crtcs`,
  `request_for_tests`, `atomic_correlation_for_tests`. **Task 6 adds the rest
  here** (`TestResource`, `ledger`, `owner_for_tests`,
  `reaped_executor_for_tests`, the event constructors) — see R5.

And from stage 2a, at `crates/yserver/src/kms/executor/`: `KmsIoExecutor` with
`send`/`poll_reply`/`tick`/`next_deadline`/`control_fd`/`try_reap`, protocol v2,
and `test_support` with `spawn_stub_helper`, `StubBehaviour`, `kill_helper`,
`wait_readable`.

## The gate — run all of it before every commit

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo test -p yserver
```

CI fails on any clippy warning and `--all-targets` is required — a crate-scoped
or non-`--all-targets` run misses lints in test code and they surface on GH.

**Read R2 before interpreting a failing `cargo test -p yserver`.**

## Rulings — these override the plan text

**R1 — where the plan contradicts itself, the normative contract wins.**
Three times during revision, a fix landed in the prose while the code block it
described kept the old behaviour. Task 1 hit exactly one instance and resolved
it this way (`ActiveContradictsPower`), which is recorded in its fold-back. If
you find another: the contract (plan lines 139-591) is the model, the task body
is an implementation of it. **Fix the task body and say so when you fold back.**
Do not try to satisfy both.

**R2 — the full suite is already flaky, and it is not yours.**
`cargo test -p yserver --lib` fails about **10-20% of runs on a clean tree**.
Measured: 3 failures in 30 runs at `25ee0237`. Three tests are involved —
`kms::executor::tests::early_take_reap_proof_returns_none_and_does_not_invalidate_future_proof`
and two in `kms::executor::device_lock::tests`. The mechanism is the fork/exec
window: `File::open` gives the lock fd `O_CLOEXEC`, so an unrelated
`Command::spawn` does not inherit it past `exec`, but between `fork` and `exec`
the child holds a duplicate of every open descriptor — including a device lock
another test thread is about to drop.

Run the suite and **confirm any failure is one of those three named tests. A
failure anywhere else is yours.** Gate your own work on twelve clean runs of
your own targets:

```bash
for i in $(seq 1 12); do
  cargo test -p yserver --lib kms::owner 2>&1 | grep -E '^test result:' | grep -q ' 0 failed' \
    || echo "OWNER FLAKE on run $i"
  cargo test -p yserver --test owner_commit_record 2>&1 | grep -E '^test result:' \
    | grep -q ' 0 failed' || echo "INTEGRATION FLAKE on run $i"
done
```

**Never relax an assertion to make a test pass.** That is what hid two real
races in stage 2a — one failed 11 runs in 12 while clippy and a single
`cargo test` both reported success.

**R3 — `NeverResource` is uninhabited, and that is the point.**
Task 7 instantiates the production owner as `DeviceCommitOwner<NeverResource>`
over `pub enum NeverResource {}`. `Vec<NeverResource>` is provably empty, so
every ledger transition is trivially correct in production. This is truthful:
2b-i converts no `atomic_commit` call site, so it owns no KMS resource.

**Do not "fix" this by inventing a handle-shaped resource type.** The first
draft did, and it claimed an ownership `spec:2127-2132` requires and this
sub-stage cannot deliver — this crate has no RAII owner for a framebuffer, BO
or pin (`DirectPresentFrame` holds pins as `u64`; `DirectScanoutProbeFramebuffer`
at `drm/modeset.rs:1395` is the only `Drop` in that path). Building those owners
is 2c's work. The type parameter is the seam.

**R4 — the generic parameter propagates further than the plan shows.**
`R` flows through `CommitRecord<R>`, `LedgerState<R>`, `DeviceCommitOwner<R>`,
`OwnerEvent<R>` **and `DispatchError<R>`**, because the refusal variant carries
`Vec<OwnerEvent<R>>`. Expect it to reach signatures the plan writes without it.
Add the parameter; do not reach for a trait object to avoid it.

**R5 — `test_fixtures.rs` is `#[doc(hidden)] pub`, never `#[cfg(test)]`.**
An integration-test crate links the library built *without* `cfg(test)`, so a
`#[cfg(test)]` fixture is invisible to `tests/owner_commit_record.rs` and a
crate-private one is inaccessible. This is the same seam stage 2a used for
`executor::test_support`, and tasks 1-5 already established the file. If you hit
a visibility error and reach for a parallel `#[cfg(test)]` fixture, you have hit
this — widen the real one.

`atomic_correlation_for_tests` uses **`EventToken::tagged_for_tests`, never
`for_tests`** — both token decoders check the purpose tag, and an untagged token
is rejected on arrival so the helper answers with a protocol error instead of a
reply. Follow that for any new event constructor that crosses the wire.

**R6 — expect the file list to be incomplete, and Task 7 especially.**
Task 7 adds a field to `KmsDevice`, which breaks every struct literal. The task
already tells you to run `grep -rn 'KmsDevice {' crates/yserver/src` and
reconcile rather than trusting its list, because an earlier revision's list
missed `backend.rs:24229`. Do that. Follow the compiler to the real blast
radius.

**R7 — the compiler is the fastest reviewer you have.**
This plan had two adversarial rounds (14 then 10 blocking, all closed),
deliberately **not** spent on compilability: stage 2a burned four rounds where 7
of the last 10 blockers were compile errors. Expect the shown code to have that
class of defect and let rustc name the line. A mechanical sweep before handoff
found 3 dangling symbols in 158 call sites and fixed them, so the residue should
be small — not zero.

**R8 — two things look broken and are deliberate.**
An accepted commit never reaches `Completed`, and the device slot is never
released after an acceptance-unknown outcome. Section 6.3 requires fence
*status* evidence 2b-i cannot query, and `COMMIT-6` forbids a second ioctl while
acceptance is unproven. Task 6's tests and Task 7's greps both pin this.
**Do not add a completion path to make it look finished.** The plan's closing
section "What this sub-stage deliberately leaves looking broken" is the
reference.

## Fold your work back into the plan

After each task, in a **separate commit** from the code:

- mark the task's steps `- [x]` and add
  `**Status: EXECUTED at `<sha>`.**` immediately under the task heading
- correct the shown code to what actually compiled
- record what the task required beyond its written text

Tasks 1-5 did exactly this at `3d68b799`, `ceafb358`, `4c257e1d`, `18ebebdf`
and `3f33849f` — read one and follow its shape. The point is that the plan
converges toward truth instead of drifting further from it.

## When you finish

Task 7 ends the sub-stage. Do **not** squash-merge: `AGENTS.md` requires asking
first, and stage 2b-ii and 2c are still unwritten — section 18 makes all of C.0
one squashed commit, so no stage is independently mergeable. Report what you
did, what the greps showed, and anything you had to fix under R1.

## Commit conventions

From `CLAUDE.md` and `AGENTS.md`: conventional-commit subjects, feature branch,
squash merge only when asked. **Never put a session URL in a commit message.**
Sign commits if you can — `CONTRIBUTING.md` requires Verified commits to merge.
