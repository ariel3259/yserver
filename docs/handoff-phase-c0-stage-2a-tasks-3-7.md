# Handoff — Phase C.0 stage 2a, tasks 3-7

**Date:** 2026-09-05
**For:** Antigravity CLI (`agy`)
**Branch:** `feat/phase-c0-atomic-kms-migration`
**Plan:** `docs/superpowers/plans/2026-09-04-phase-c0-stage-2a-executor-substrate.md`
**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`

Tasks 1 and 2 are implemented and merged on this branch. Tasks 3-7 are yours.

## Start here

Read `AGENTS.md` first — it is the project's instructions for all agents.
Then invoke `superpowers:executing-plans` (or, if you dispatch subagents via
`invoke_subagent`, `superpowers:subagent-driven-development`) against the plan
above. Do **not** read the whole plan into context at once: it is 3200 lines.
Read the task you are executing, plus the two sections before Task 1 —
**Global Constraints** and **The wire and API contract**. The contract is
normative: a task whose shown code contradicts it is wrong, and the contract
wins.

## State

| Task | Status | Commit |
| --- | --- | --- |
| 1 — lifecycle identities, host-call classes, checked allocation | done | `6f951850` |
| 2 — atomic property payload, reply correlation, protocol v2 | done | `ecd76f1c` |
| 3 — helper materialization and `OUT_FENCE_PTR` holders | **yours** | — |
| 4 — asynchronous host-call API | **yours** | — |
| 5 — executor as a core-loop source | **yours** | — |
| 6 — `COMMIT-7` device lock | **yours** | — |
| 7 — portable gates | **yours** | — |

What already exists that the remaining tasks build on:

- `kms/owner/lifecycle.rs`: `LifecycleEpochId`, `LifecycleTransitionId`, `ClockProbeId`.
- `HostCallClass` has **four** variants with `watchdog()`, `wire_tag()`, `from_wire_tag()`, `is_validation()`.
- Both token decoders check the purpose tag. **Anything crossing the wire must use `EventToken::tagged_for_tests`, never `for_tests`** — `for_tests` stores a raw value the decoder rejects. This already broke two stage-1 integration tests once.
- Protocol v2: variable-length requests, fixed-length replies with a shared 56-byte correlation block, `HostCallReply` with four variants in two families, and a separate handshake frame family carrying incarnation + lifecycle epoch.
- `HostCallReply::family()` is derived from the **variant**, never the correlation.
- Test seam widened behind `#[doc(hidden)]`; `crates/yserver/tests/wire_external_surface.rs` proves it compiles from an external crate. Keep that test passing — it is the only real check on the visibility contract.
- Request builders live in `kms::executor::test_support` (`small_atomic_request_for_tests`, `probe_request_for_tests`, `validation_request_for_tests(class)`, …). External tests must use these, never build wire types by hand.

## The gate — run all of it before every commit

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo test -p yserver
cargo test -p yserver-core          # tasks 5 and 7 only
```

CI fails on any clippy warning and `--all-targets` is required. Stage 1's
`crates/yserver/tests/executor_substrate.rs` (6 tests) is the regression net
for the outcome classification — it must keep passing **unchanged**.

## Rulings — read these before starting

The plan went through four adversarial review rounds. Round 4
(`docs/superpowers/findings/2026-09-05-phase-c0-stage-2a-plan-review-round4.md`)
left ten findings open in tasks 3-7. Rather than make you rediscover them,
here is what to do about each. These rulings **override the plan text** where
they conflict.

**R1 — `dispatch_and_wait_for_tests` takes the request by reference.**
Signature is `(&mut KmsIoExecutor, &HostCallRequest) -> HostCallOutcome`. Task 3's
shown call sites pass by value; add the `&`.

**R2 — the ioctl-capture test is a unit test, not an integration test.**
Task 3 shows `the_submitted_ioctl_argument_is_the_prepared_arrays` in
`tests/executor_async.rs`, but the capture seam is `#[cfg(test)]` and a library
is compiled without that cfg for integration tests. Put the test inside
`helper.rs`'s `#[cfg(test)] mod tests`. And `capture_submitted_ioctl_for_tests`
must return an **owned snapshot** (the objects, count_props, props and values it
would have submitted, copied out), not a raw `DrmModeAtomic` whose pointers
dangle once the `PreparedAtomic` drops.

**R3 — `KmsDevice.executor` is `Option<KmsIoExecutor>`.**
Task 5 adds a mandatory field, which breaks eight existing struct literals
(`platform.rs:2555,2982,7262,7710,7727,8319`, `backend.rs:24196`, plus the
production conversion). Making those fixtures each spawn a real helper process
is worse than the weaker invariant. So: `Option`, `None` in test fixtures,
always `Some` in `platform_init`. `poll_fds`, `executor_deadline` and
`tick_executors` skip `None`. Document at the field why it is optional.

**R4 — bound every test that could hang.**
Revision 4 removed the wall-clock ceilings and claimed "the harness's own
timeout" would catch a blocking implementation. Rust's test harness has **no
per-test timeout**, so a regression hangs CI instead of failing. Give
`test_support::wait_readable` and every `poll_reply` wait a generous bound
(30 s) that **panics with a clear message** on expiry. That is a hang guard,
not a timing assertion: it cannot false-positive on scheduling, but it turns an
infinite hang into a named failure.

**R5 — `tick` must not reap in the call that terminalizes.**
The plan has `tick` terminalize an expired call and then independently attempt
`try_reap` every call. With a helper that dies on the termination signal,
whether `try_wait` observes it in that same call is scheduling-dependent, so
tests asserting `ExecutorState::Stalled` immediately afterwards are flaky.
Terminalize and return; attempt reap only on **subsequent** ticks.

**R6 — the handoff entry point needs an inner closure.**
`run_lock_handoff_if_requested` returns `Option<io::Result<()>>`, and the plan's
shown body applies `?` to `Result` values inside it, which does not compile.
Wrap the body in `Some((|| -> io::Result<()> { … })())`. Also: the plan contains
a literal `map_err(...)` placeholder — write real error handling there.

**R7 — the drain test needs two replies on ONE socket.**
Task 5's `on_executor_readable` test uses two devices with one reply each, which
an implementation calling `poll_reply` once per device passes. The requirement
is drain-to-exhaustion on a single readable source, so queue two replies on one
executor's control socket.

**R8 — delete Task 7's `#[doc(hidden)]` grep.**
The proposed `rg -B1 … | rg -v 'doc(hidden)' | rg 'pub '` is line-oriented: it
strips the attribute line and keeps the declaration, so it prints correctly
annotated items and never fails on a bare `pub`. `tests/wire_external_surface.rs`
already proves the seam compiles from outside the crate, which is the property
that matters. Remove the grep rather than keeping a check that cannot fail.

**R9 — the `libc::poll` gate counts one site, two callers.**
Task 6's `await_helper_ready` needs a bounded wait on a socket that task 4 makes
non-blocking. Do **not** add a second `libc::poll`, and do not retry-sleep
(the sleep gate forbids it). Extract `wait_readable_bounded(fd, deadline)` in
`executor/mod.rs` and have both `dispatch_blocking_at_boundary` and
`await_helper_ready` call it. Task 7's "exactly one `libc::poll`" then still
holds and still means something.

**R10 — scope Task 7's greps to the host-call path.**
`mod.rs`, `helper.rs`, `transport.rs`, `protocol.rs` only. `test_support.rs` is a
helper-process simulator whose whole job is to sleep, and `device_lock.rs` holds
the lock-holder subprocess plus `#[cfg(test)]` tests that legitimately `wait()`.
A directory-wide gate forbids code the plan itself requires and can never pass.

## What executing tasks 1-2 taught

Two things worth carrying forward.

**The compiler is the fastest reviewer you have.** Seven of round 4's ten
blocking findings were compile errors — a non-exhaustive match, private types
used from an external test, unresolved names, `?` in the wrong return type,
struct literals missing a field. Tasks 1 and 2 hit the same class and rustc
named the exact line in seconds. Write the code and let it tell you; do not
reason about whether the plan's shown code compiles.

**Expect the file list to be incomplete.** Task 2's plan named `mod.rs`, but
changing the request shape broke `helper.rs`, `test_support.rs`, `transport.rs`
and its own tests — 31 errors in the first build. That is normal. Follow the
compiler to the real blast radius rather than trusting the list.

## Fold your work back into the plan

After each task, in a **separate commit** from the code:

- mark the task's steps `- [x]` and add `**Status: EXECUTED at `<sha>`.**`
- correct the shown code to what actually compiled
- record what the task required beyond its written text

Tasks 1 and 2 did this at `b39af438` and `0905cbef` — follow that shape. The
point is that the plan converges toward truth instead of drifting further from
it with every revision, which is what four review rounds failed to achieve.

## Commit conventions

From `CLAUDE.md` and `AGENTS.md`: conventional-commit subjects, feature branch,
squash merge only when asked. **Never put a session URL in a commit message.**
Sign commits if you can — `CONTRIBUTING.md` requires Verified commits to merge.
