# Handoff — Phase C.0 stage 2c-i, all ten tasks

**Date:** 2026-09-10
**For:** Gemini 3.8 Flash as the implementing model, in whichever harness
runs it (Gemini CLI or Antigravity)
**Branch:** `feat/phase-c0-atomic-kms-migration`, worktree
`/home/ariel_santangelo/Projects/yserver-phase-b`
**Baseline:** `76a93356` — the tree is clean and every document below is
committed
**Plan:** `docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md`
**Design it implements:** `docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md`
plus `docs/superpowers/specs/2026-09-09-phase-c0-stage-2c-i-resource-adapter-inventory.md`
**Governing spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
(revision 3), sections 9.1, 10–10.4, 12 and 18

Stages 1, 2a, 2b-i and 2b-ii are complete on this branch. **Nothing in 2c-i is
implemented.** All ten tasks are yours, in order.

## Start here

1. Read `AGENTS.md`. Your harness may look for `GEMINI.md`; there is none, and
   `AGENTS.md` is the project's instructions for every agent.
2. Read this document to the end. The **Rulings** section overrides the plan
   text wherever they disagree.
3. Read the plan's front matter — **Global Constraints** (lines 15–35), the
   **File and dependency map** (36–54) and the **Shared interface vocabulary**
   (55–90). Those three are contracts. Then read only the task you are
   executing. The plan is about 800 lines; do not hold all of it in context,
   and do not read the two review-history sections at the end (754 onward)
   unless you need to know why something is shaped the way it is.
4. The Superpowers skills are Claude Code plugins and nothing loads them for
   you, but they are plain files. Read these two before Task 1:
   - `~/.claude/plugins/cache/claude-plugins-official/superpowers/6.3.0/skills/executing-plans/SKILL.md`
   - `~/.claude/plugins/cache/claude-plugins-official/superpowers/6.3.0/skills/test-driven-development/SKILL.md`

Work **one task per session** if your context is limited. Each task ends with
its own commit and a fold-back commit (below), so a fresh session can resume
from the plan's `Status:` lines without this document changing.

## State

| Task | Deliverable | Status |
| --- | --- | --- |
| 1 | Rooted allocation leases and authoritative availability | **yours** |
| 2 | Consuming DRM cleanup and real direct framebuffer retention | **yours** |
| 3 | Storage generations, layout and promotion | **yours** |
| 4 | Shared/copied scanout backing and pool reuse | **yours** |
| 5 | GPU, descriptors and readback lifetime | **yours** |
| 6 | Completion progress and transport permission boundary | **yours** |
| 7 | Concrete commit resources and Present dispositions | **yours** |
| 8 | Six physical roles and release-safe exit | **yours** |
| 9 | Reserved teardown recipient and late-completion handoff | **yours** |
| 10 | Concrete adapters, integration evidence, handoff to 2c-ii | **yours** |

The dependency chain is 1 → 2 → … → 10 and it is real: Task 2's registry
consumes Task 1's leases, Task 4's payload halves carry Task 2's rights, Task 7
instantiates the generic owner with Task 4–5's types, Task 9 moves everything
Tasks 1–8 built. Do not skip ahead.

## What already exists that you build on

All real, readable and tested. Do not re-derive or re-implement any of it.

- `kms/owner/device.rs` — `DeviceCommitOwner<R>` and the single typed
  `OwnerEvent<R>` stream (`Dispatched`, `Accepted`, `HardwareComplete`,
  `Presented`, `Terminal`, `CompletionRetired`, `ResourcesReleased`,
  `ResourcesStillCurrent`, `Quarantined`, …). Production instantiates it with
  the uninhabited `NeverResource`; Task 7 replaces that with `CommitResources`.
- `kms/owner/ledger.rs` — `Submitted<R>`, `Accepted<R>::into_parts()`,
  `Rejected<R>`. The ledger owns resources by value; transitions consume `self`.
- `kms/owner/qualification.rs`, `fences.rs`, `deadlines.rs` — `CompletionCaps`,
  canonical `FenceStatus`, and the bounded deadlines. These are the runtime
  qualification gate the governing spec's revision 3 relies on.
- `kms/executor/` — `KmsIoExecutor`, the process-isolated helper, and
  `dispatch_blocking_at_boundary`. The helper owns the DRM open file
  description; the parent's `drm::Device` is an **inherited duplicate** of it
  (`drm/device.rs`, `from_inherited_kms_fd`, `MasterOwnership::InheritedDuplicate`).
- `kms/vk/scanout.rs` — `ScanoutBo` (line ~480 onward), the pool, and the GBM
  path: `type GbmDevice = gbm::Device<Rc<drm::Device>>` (line 75); on the
  GBM-allocated path `PRIME_FD_TO_HANDLE` returns the gbm_bo's **existing** GEM
  handle (comment at line ~3115). `ScanoutBo::Drop` is at line ~3442.
- `drm/modeset.rs` — `DirectScanoutProbeFramebuffer` (~line 1395), the only
  `Drop` in the crate that issues `RMFB`/GEM close. Task 2.4 converts it.
- `kms/render/backend.rs` — `deferred_cow_release` (line 1164) and the
  overlay edges (`get_overlay_window` ~20632, `release_overlay_window` ~20755,
  `finish_deferred_cow_release` ~1853, its stop-path call ~2110).
- `yserver-core/src/server.rs` — `ServerState::cow_claims` and
  `cow_teardown_failed`: core owns the logical overlay claim; the backend sees
  only 0→1 / 1→0 edges. `KmsCore::cow_refcount` no longer exists.

## The gate — run all of it before every commit

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo test -p yserver --lib <this task's test prefix>   # c0_2ci_* names
```

And for the tasks the plan marks (2, 6, 10), the three portable checks:

```bash
cargo check -p yserver --target x86_64-unknown-linux-gnu
cargo check -p yserver --target x86_64-unknown-linux-musl
cargo check -p yserver --target x86_64-unknown-freebsd
```

CI fails on any clippy warning; `--all-targets` is required or lints in test
code surface on GitHub instead. A failing check blocks the commit. Stage only
the files the task names.

**Read R2 before interpreting a failing full suite.**

## Rulings — these override the plan text

**R1 — where the plan contradicts itself, the contract wins.** The contracts
are: Global Constraints, the Shared interface vocabulary, each task's
**Produces** block, and the design spec. A task step whose prose or shown code
contradicts one of those is wrong. Fix the step, implement the contract, and
say so in your fold-back. Do not try to satisfy both. This has been necessary
in every prior stage.

**R2 — the full suite is already flaky, and it is not yours.**
`cargo test -p yserver --lib` fails 10–20% of clean runs because of the
fork/exec window in three named tests:
`kms::executor::tests::early_take_reap_proof_returns_none_and_does_not_invalidate_future_proof`
and two in `kms::executor::device_lock::tests`. Confirm any failure is one of
those three. **A failure anywhere else is yours.** Gate your own targets on
twelve clean runs:

```bash
for i in $(seq 1 12); do
  cargo test -p yserver --lib c0_2ci 2>&1 | grep -E '^test result:' | grep -q ' 0 failed' \
    || echo "FLAKE on run $i"
done
```

**Never relax an assertion to make a test pass.** That hid two real races in
stage 2a; one failed 11 runs in 12 while a single green `cargo test` reported
success.

**R3 — one closer per kernel object. This was the round-4 blocker.**
Task 4's *Physical ownership of one scanout allocation* table is normative. A
GEM handle is not refcounted by the kernel. On the GBM path the Task-2 right
and the gbm_bo drop would both close the same handle, and a second close after
handle-number reuse destroys another live buffer's handle. Therefore:

- every `DrmCleanupRight` carries `GemOwner::Gbm` or `GemOwner::Right`;
- `GemOwner::Gbm`: the right records the handle and **never** closes it; the
  gbm_bo drop, ordered after `RMFB`, is the sole `GEM_CLOSE`;
- `GemOwner::Right`: the right closes it, as the legacy `Drop` does today;
- `FileOwnedBacking::new` is the only constructor and rejects any other
  pairing;
- Task 4.6 asserts exactly one `CloseGem(h)` per payload with a counting
  transport, including the `FramebufferRemoved` retry path.

The baseline `ScanoutBo::Drop` closes the handle twice today. That is benign
there (both closes are consecutive on one thread, nothing can reuse the number
in between) and **it is not yours to fix** — leave the legacy `Drop` alone. It
becomes a defect only where the plan separates the two closers in time, which
is exactly what R3 prevents in managed payloads.

**R4 — the payload has two halves with separate dispositions.** `file_owned`
(right, gbm_bo, `Rc<drm::Device>` alias) and `shared` (VkImage, memory, view,
transfer, `VkContext`, dma-buf). The Task-9 barrier discharge takes only
`file_owned`; `file_owned == None` afterwards is a legal, fully described
state. `shared` is released only by GPU/read/FOREIGN proofs. Never destroy a
`VkImage` because file-owned rights discharged.

**R5 — the fd-family barrier must be reachable.** Every `Rc<drm::Device>`
inside a registry-rooted payload is a **counted alias** of the incarnation's
open file description, registered at adoption. `FileFamilyClosed` becomes
mintable when every submitter is detached, the helper is reaped, and the only
remaining holders are those payload contexts; the discharge then closes them
in the R3/R4 order and the registry performs the description's last close. A
payload that merely waits for the barrier while holding the alias is the leak
the design forbids. Task 2.5 tests this with a **real** `Rc<drm::Device>`
payload, not the fake inventory — do not weaken that test.

**R6 — the `KmsRelease` producer is the displacing commit's own
`HardwareComplete`, for its `old` set only.** The canonical out-fence of the
flip that installed B proves A left the plane. Register one obligation per
*displaced pair* `(allocation, member)` with `new[member] != old[member]`; a
member retained across a grouped commit registers nothing. Discharge is keyed
by `GroupMember` (CRTC key + topology generation + CRTC epoch), never by
current topology. Nothing is ever discharged for the `new` set: **a current
buffer's own presentation never proves it idle.** A rejected commit
(`ResourcesStillCurrent`) *cancels* its registrations; it does not discharge
them. The Task 7.6 regression's only proof path is the owner event — the test
body must not call `apply_validated_proof`.

**R7 — the transport gate.** `begin_quiescing` returns `Busy` while any direct
ownership unit is `Current`/`Submitted`/`Successor` or an unflip is requested
and not retired: exiting direct scanout is the last legacy write and precedes
quiescing. `Quiescing` permits **no** writer class; the all-classes-false
assertions stand. `OwnerWriteGrant` is consumed at the serialized send
boundary when the executor accepts the request; unknown outcome thereafter
belongs to the owner's quarantine, not the gate count. Handoff (9.3) calls
`revoke_owner_writes` before `close` and treats every revoked grant as possibly
dispatched (`Quarantined`, never "cancelled").

**R8 — nothing you build is production-active.** Replacing
`NeverResource` with `CommitResources` in Task 7 keeps the production route
`Legacy`. There is no production issuer for `OwnerWriteGrant`, no production
`RecipientReservation`, no production writer-coverage proof, and no environment
switch that enables Owner. Tests establish Owner only with explicit mock
coverage for every `WriterClass`. If you find yourself adding a flag to reach
a converted path in production, stop: that is 2c-iii and stage 3/4 work.

**R9 — proofs are never fabricated.** An expired deadline, an `Rc` reaching
zero, fd readiness, a Present completion, a closed IPC fd or a single closed
alias is never a KMS, GPU, read or FOREIGN proof. `apply_validated_proof` is
private to `resources`; producers correlate real evidence before calling it.
Failed and timed-out tickets are retained for teardown, not polled forever and
not released. The pending deadline on non-exportable tickets counts
**serviced** time and pauses while the seat is inactive.

**R10 — core-thread only.** Allocations, mapped pointers and GBM state stay on
the core thread. Do not add `Send`/`Sync` to fit `BatchResource`; use the
core-thread retirement lane the plan describes. Do not add a thread or a
synthetic ready fd for the service inbox.

**R11 — the transport gate is enforced at real sinks.** Task 6.5a's table is a
list of anchors, not a boundary: follow each sink's callers, including startup
rollback and cursor restoration, and classify every discovered path. The
counting transport goes *beneath* the real entry point; do not replace the
entry point with a mock that merely calls `allows_legacy`. Report the caller
inventory in your fold-back.

**R12 — hardware and Vulkan tests report honestly.** Deterministic tests use
the `c0_2ci_` prefix and run in ordinary `cargo test`. Real Vulkan/DRM cases
carry the repository's hardware annotations, end in `_vulkan` or `_drm`, and
must report an environmental skip as a skip — never as a pass. The
gbm_bo-before-`VkImage` destruction order is proven only by the live-Vulkan
smoke under validation layers (Task 10.2), not by a fixture.

## Where the sharp edges are, and why

The plan survived four adversarial reviews (two codex, two claude; see the
findings under `docs/superpowers/findings/2026-09-*-stage-2c-i-implementation-plan-review-round*.md`).
Three consecutive rounds found that the previous round's correction created
the next blocker, each one layer deeper into physical ownership: KMS
disposition at teardown (round 2) → the fd-family cycle through `GbmDevice`
(round 3) → two closers of one GEM handle and an undivided payload (round 4).
R3, R4 and R5 are the result. Tasks 2.5, 4.6 and 9.5 are the tests that decide
whether that chain is finally closed; they are the most important tests in
this stage. If one of them cannot be made to pass as specified, **stop and
report** — that is a design question, not something to work around.

## Fold your work back into the plan

After each task, in a **separate commit** from the code:

- mark the task's steps `- [x]` and add
  `**Status: EXECUTED at `<sha>`.**` directly under the task heading;
- correct the shown code to what actually compiled;
- record what the task required beyond its written text, and any R1 fix.

Stage 2b-i did exactly this (see `git show 3d68b799`); follow that shape. The
plan must converge toward the truth of the tree.

## When you finish

Task 10 ends the sub-stage. Update `docs/status.md` with a dated entry in the
existing style. Do **not** push, squash-merge or activate anything: section 18
of the governing spec makes all of C.0 one squashed commit, and 2c-ii, 2c-iii
and stages 3–4 are still unwritten. Report what you did, the 6.5a caller
inventory, every environmental skip, and everything you had to fix under R1.

## Commit conventions

From `CLAUDE.md` and `AGENTS.md`: conventional-commit subjects (the plan gives
each task's), feature branch only, stage only the files the task names, no
session URLs of any tool in commit messages. Sign commits if you can —
`CONTRIBUTING.md` requires Verified commits to merge. Run the gate before every
commit; a failing check blocks it.
