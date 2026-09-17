# Stage 2c-i debt, part 3 — the flip-accepted path, with DRM master

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), `--sandbox workspace-write`, run with `< /dev/null`. Execute tasks in order, one at a time; tick steps (`- [ ]` → `- [x]`) only with the evidence each names. Before writing code, read `AGENTS.md` and, as plain markdown, the Superpowers skills `executing-plans/SKILL.md` and `test-driven-development/SKILL.md` under `~/.claude/plugins/cache/claude-plugins-official/superpowers/*/skills/`. **The implementer never commits**: this worktree's git directory is read-only inside the sandbox. Stop with the tree dirty; the coordinating session verifies and commits.

**Revision 3 (2026-09-17)** — incorporates codex round 2 (`…-part-3-plan-review-round2.md`: 0 blocking, 2 major; four of round 1's five audited APPLIED, M-3 PARTIAL). See *Corrections from review round 2*.

**Revision 2 (2026-09-17)** — incorporates codex round 1 (`docs/superpowers/findings/2026-09-17-part-3-plan-review-round1.md`: 2 blocking, 3 major), all five verified against the tree. See *Corrections from review round 1*.

**Goal:** Put a managed scanout buffer on screen through a page flip the kernel actually accepts, and prove the stage 2c-i ledger against the kernel's own completion and out-fence instead of a synthesized event.

**Architecture:** A second hardware fixture, beside the existing ones, that takes DRM master and builds its output from the live card through the production probe. Four `_drm` tests over it, one per invariant of spec section 9.2. Nothing existing changes shape: the current fixtures keep opening the node without master, so the tests that encode the no-master outcome keep meaning what they mean.

**Tech Stack:** Rust (`cargo test`), libdrm through the `drm` crate, a live Vulkan ICD, a real DRM card with a connected output.

**Spec:** `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` section 9. Sections 9.2 (invariants), 9.3 (how it is run) and 9.4 (what it is not) govern this plan.

**Prior measurement:** `docs/superpowers/findings/2026-09-17-part-3-drm-master-feasibility.md` — read it before Task 1. It establishes, on this box, that master is reachable from an active VT, that `card1` (NVIDIA) drives the connected output and is the node Vulkan reports, and that a modeset, a page flip, a kernel completion and an atomic `OUT_FENCE_PTR` all work there with dumb buffers.

## Why this plan states invariants instead of supplying code

Session 2's plan handed the implementer verbatim blocks, because the coordinating session had prototyped and measured every one of them first. That is impossible here: these tests only run with DRM master, which logind grants to the **active** session on the seat, so neither the implementer's sandbox nor the coordinating session can execute them while writing them. Prescribing unrunnable code would be prescribing guesses. Spec 9.2 says so too — "the shape of the tests is the implementer's call".

So this plan fixes the **fixture contract**, the **invariants**, and the **commands the user runs**, and leaves the test bodies to the implementer. What the implementer can still verify mechanically is stated per task: it compiles, it lints, the deterministic suite is unaffected, and the new tests **fail** rather than pass when run without master.

## Corrections from review round 2

| Finding | Disposition |
| --- | --- |
| **M-1** — Task 6 substituted a *new* test for running the identified existing one with master, so round 1's M-3 was only PARTIAL | **Fixed.** Verified: `c0_2ci_sink_gamma_gate_four_states_drm` opens its own node through `TestDevice::open_real_drm_or_ignore` — without master — and drives **four** gate states. A new test over a different fixture would prove something else. Task 6 now parameterises that test's own body into a helper taking the device, leaves the existing test calling it with its current non-master device (unchanged in meaning), and adds a master-held invocation of **the same helper over the same four states**. The record names the existing test and carries both outcomes. |
| **M-2** — no bounded, ordered completion contract: a flip is asynchronous, so "read the CRTC afterwards" is timing-dependent, and a lost event or stuck fence would hang the run *and* skip the display restoration | **Fixed.** A new Global Constraint: every hardware observation waits for the **matching** completion under a deadline, P3-1's readback and P3-2's discharge assertion happen only after that completion is correlated to the submitted output and BO, and P3-4 polls the canonical `sync_file::query_status` until `Success`, `Error(_)` or the deadline. A deadline or error **panics through normal unwinding**, so the fixture's restoration runs; the record distinguishes a timeout from a fence error. |

## Corrections from review round 1

| Finding | Disposition |
| --- | --- |
| **B-1** — the fixture had no display-restoration contract, only "drops master on drop"; a panicking test (which the mutations deliberately cause) could leave the console on test contents | **Fixed.** The fixture snapshots the prior CRTC state and tears down in a stated, unwind-safe order: restore the original connector/CRTC/framebuffer **while master and the test resources are still alive**, then destroy the test resources, then drop master. A failed restoration is reported loudly rather than swallowed by the panic in flight. This is what the feasibility probe already did; the plan had not carried it over. |
| **B-2** — P3-4 gave the out-fence two owners | **Fixed.** Verified in the tree: `submit_flip_with_fences` hands the fd to `bo.state.transition_to_pending(out_fence)`, which keeps it as `release_fence_fd` — the BO state is the canonical owner. The test observes fence status through that owner and closes nothing; if it needs an independent fd it `dup`s one and closes only its own copy. |
| **M-1** — nothing serialised tests sharing one device, one master and one CRTC | **Fixed.** The run command pins `--test-threads=1`, and the fixture additionally takes a process-wide exclusive guard, so a future filter or a stray parallel invocation cannot make two fixtures contend for master or consume each other's DRM events. |
| **M-2** — the census tool cannot produce the two named mutations | **Fixed, by extending the instrument.** Verified: the P3-3 mutation must make `if !retained_in_new` register anyway, and the tool only forces conditions to `false` and only enumerates refusal guards. Task 1 adds a `--named-mutation` mode carrying those two edits explicitly, reusing the tool's restore-from-memory and compile-check discipline, so spec 9.3 step 3 is satisfied literally — the mutations still run through the census tool. |
| **M-3** — the master-held audit of the existing tests was inferred, not executed | **Fixed.** Correct: the suite ran while master was *available*, but the fixtures' own fds never held it, so the gamma test's behaviour under master was never observed. Task 6 adds a scoped, master-held execution of that exact path on the fixture's master-holding fd, and records what the kernel actually does. The ordinary fixture keeps its no-master semantics untouched. |

## Global Constraints

- **R12, sharpened.** Every test here carries the `_drm` suffix and `#[ignore]`, and **detects whether it holds master**. Without master it reports an environmental failure — `panic!` with a message naming the cause — and never passes. A test that cannot tell "no master" from "passed" is the one defect this part exists to avoid.
- **R8:** nothing here becomes production-active. The fixture is `#[cfg(test)]`.
- **F3:** do not mock `ResourceService`; the tests drive the real service.
- **F8:** if an invariant cannot be proven because the code or the driver does not do what the spec assumes, **stop and report it**. A part-3 test that cannot pass is a finding about the ledger or the driver, which is the point of the exercise; it is never something to weaken into a pass.
- **The existing fixtures keep opening the node without master.** `KmsBackend::for_tests_with_vk*` and `PlatformBackend::for_tests` are not touched. `c0_2ci_sink_gamma_gate_four_states_drm` asserts the gamma ioctl is *reached and fails* — true only without master (see the prior measurement). If a change here would put master under it, that is an F8 stop, not a silent edit of its assertion.
- **The console comes back, even from a panic (round-1 B-1).** The fixture snapshots the CRTC configuration it found and tears down in this order, in `Drop` as well as on the success path: **restore** the original connector/CRTC/framebuffer, with master and the test framebuffers still alive; **then** destroy the test resources; **then** drop master. A restoration that fails is reported loudly — a panicking test must not hide it.
- **One live-KMS fixture at a time (round-1 M-1).** Master is per-fd and the CRTC is shared, so two fixtures in one process would fight. The fixture takes a process-wide exclusive guard for its lifetime, and the run command pins `--test-threads=1`. Both, not either.
- **Every hardware observation is bounded and correlated (round-2 M-2).** A flip is accepted asynchronously: `on_page_flip_complete` is what retires the pending BO, and `sync_file::query_status` distinguishes `Pending`, `Success` and `Error(_)`. So no test reads state "afterwards": it waits for the **matching** completion — correlated to the output and BO it submitted — under an explicit deadline, and only then asserts. P3-4 polls the canonical status until `Success`, `Error(_)` or that deadline. **A timeout or a fence error panics**, through ordinary unwinding, so the fixture's restoration runs; a test that blocks forever would strand the console, which is the outcome this constraint exists to prevent. The record says which of timeout or error occurred.
- **`OUT_FENCE_PTR` writes an `s32`.** Read it into `i32`, as `scene.rs` already does. A wider destination reads a bogus fd — measured: `0xFFFFFFFF00000004` for fd `4`.
- **The out-fence has exactly one owner (round-1 B-2).** `submit_flip_with_fences` hands the fd to `bo.state.transition_to_pending`, which keeps it as `release_fence_fd`. Tests observe it **through that owner** and close nothing; an independent observation `dup`s the fd and closes only the duplicate. A test that closes the BO's fd leaves a dead descriptor behind and can later double-close it.
- Every new test name starts with `c0_2ci_` (the census runs `cargo test -p yserver --lib c0_2ci`).
- Gate before each hand-off: `cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings`; `cargo test -p yserver --lib c0_2ci`.
- The implementer does not run `git commit`, `git add`, `git checkout`, `git stash`, `git apply` or `rm -f`, and does not claim any `_drm` test passes: its sandbox has no `/dev/dri`. The coordinating session commits each task, with the trailer `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)`. Never a session URL in a commit message.
- Nobody pushes, squashes, rebases or amends.

## What the implementer cannot do, and who does it

| Step | Who | Why |
| --- | --- | --- |
| Write the fixture and the tests, compile, lint, run the deterministic suite | implementer | no GPU or DRM node in its sandbox |
| Confirm the new tests fail without master | coordinating session | has GPU and DRM, but runs inside a session that is not the seat's active one |
| Run the `_drm` tests with master, and the mutations | **the user**, from an active VT (tty2) | logind grants master only to the active session |

## File Structure

- Modify: `crates/yserver/src/kms/executor/test_support.rs` — master acquisition on an already-open node, with the R12 failure message (Task 1).
- Modify: `crates/yserver/src/kms/render/backend.rs` — the live-KMS fixture beside the existing ones (Task 1).
- Create: `crates/yserver/src/kms/render/part3_tests.rs` — the `_drm` tests (Tasks 3–6), declared from `render/mod.rs` under `#[cfg(test)]`.
- Modify: `tools/guard-census.py` — a `--named-mutation` mode for the two lifecycle mutations spec 9.3 step 3 names (Task 1).
- Create ([H], user + coordinator): `docs/superpowers/findings/2026-09-17-part-3-flip-accepted.md` (Task 7).

---

### Task 1: The mutation instrument for part 3's two named mutations

**Files:**
- Modify: `tools/guard-census.py`

**Why this comes first.** Spec 9.3 step 3 requires part 3's named mutations to
be run "with the census tool from section 3.0". Round-1 M-2 showed the tool
cannot express them: it enumerates **refusal guards** (`return Err/false/None`)
and forces their conditions to `false`, while P3-2's mutation relocates a
lifecycle step and P3-3's must make `if !retained_in_new` register **anyway**.
The instrument is extended rather than bypassed, so the discipline that makes a
mutation trustworthy -- restore from memory even on failure or Ctrl-C, and count
nothing that did not compile -- still applies.

**Interfaces:**
- Produces: `tools/guard-census.py --named-mutation {p3-2,p3-3} --filter <cargo test filter> [--dry-run]`, which refuses unless the unmutated filtered suite is green; applies the one edit its name carries; **refuses to count a mutant that did not compile**; runs the filtered suite; reports which tests failed; and restores the file from memory in a `finally`, as the existing census already does. `--dry-run` applies the edit, confirms it compiles, prints the diff and restores, without running the suite.
- The two edits, by name:
  - **`p3-2`** -- discharge the `KmsRelease` obligation at submission instead of at the kernel completion, so "not released before the completion" becomes false;
  - **`p3-3`** -- register a KMS obligation for a **retained** member, by making `commit.rs`'s `if !retained_in_new` branch register regardless.
- Each edit is anchored by the exact source text it replaces, never by line number, and the tool **fails loudly when its anchor no longer matches**: a silently skipped mutation is the failure mode this whole stage exists to prevent.

- [ ] **Step 1: Extend the tool**

- [ ] **Step 2: Prove both anchors still match**

Run: `python3 tools/guard-census.py --named-mutation p3-3 --filter c0_2ci_commit --dry-run`, then the same for `p3-2`.
Expected: each edit applies and compiles, and the printed diff is the edit its name promises. A non-matching anchor is an F8 stop, not a guess at the intended site.

- [ ] **Step 3: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`
Expected: clean; **180 passed, 18 ignored** -- this task touches no Rust source.

```bash
git add tools/guard-census.py
git commit -m "tools(census): named mutations for part 3

Spec 9.3 step 3 requires part 3's two mutations to run through the
census tool, but the tool only neutralises refusal guards: P3-2's
mutation relocates a lifecycle step and P3-3's must make a branch
register where it currently skips (review round 1, M-2).
--named-mutation carries those two edits explicitly, anchored by source
text, and reuses the tool's restore-from-memory and compile-check
discipline.

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 2: The live-KMS fixture

**Files:**
- Modify: `crates/yserver/src/kms/executor/test_support.rs`, `crates/yserver/src/kms/render/backend.rs`

**Interfaces:**
- Produces: `KmsBackend::for_tests_with_live_kms() -> Result<LiveKmsFixture, io::Error>`, where `LiveKmsFixture` owns the backend and drops master on drop. Its contract:
  0. takes the process-wide exclusive guard (round-1 M-1) and holds it for its whole lifetime;
  1. builds the `VkContext` first, and opens **the primary node that `VK_EXT_physical_device_drm` reports for the physical device Vulkan selected** — the existing `TestDevice::open_real_drm_matching` path, not a blind `card0`;
  2. takes DRM master on it. On `EACCES` it **panics** with a message that names the cause: master is granted to the seat's active session, so the test must run from an active VT. Never a skip, never a pass;
  3. probes that device's connectors, picks the first **connected** one with modes, resolves its encoder, CRTC and primary plane, and builds the output through the production path `crate::drm::modeset::output_for_exact_probe_assignment`, with the connector's preferred mode. A box with no connected output panics with that reason;
  4. installs that `ActiveOutput`, and allocates the scanout pool at the mode's real resolution;
  5. **reports, on stdout, the card path, the connector name and the mode it selected** — spec 9.3 step 2 requires the run to say which card was used;
  6. **snapshots the CRTC configuration it found before touching it** and, on teardown — success or panic alike — restores it **while master and the test framebuffers are still alive**, then destroys the test resources, then drops master (round-1 B-1). A failed restoration is reported loudly, never swallowed by the panic in flight.
- Consumes: `TestDevice::open_real_drm_matching`, `output_for_exact_probe_assignment`, `ActiveOutput::new`, and whatever the existing `for_tests_with_vk_live_scene` uses to attach the Vulkan context and build pools — reuse it rather than duplicating it.

- [ ] **Step 1: Write the master helper**

In `test_support.rs`, add master acquisition over an already-open node (`DRM_IOCTL_SET_MASTER` / `DRM_IOCTL_DROP_MASTER`, or the `drm` crate's equivalents), returning a guard that drops master when dropped. The `EACCES` message must name the active-session requirement.

- [ ] **Step 2: Write the fixture**

Beside the existing fixtures in `backend.rs`, per the contract above. Do not modify them.

- [ ] **Step 3: Compile and lint**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings`
Expected: clean. The fixture is unused so far, so give it `#[cfg(test)]` and, if clippy asks, the narrowest `allow` that keeps it honest — or, better, land it in the same commit as Task 2's first caller if that avoids the allow entirely.

- [ ] **Step 4: The deterministic suite is unaffected**

Run: `cargo test -p yserver --lib c0_2ci`
Expected: **180 passed, 18 ignored** — exactly the session-2 acceptance numbers. Any change here means an existing fixture was touched: F8 stop.

- [ ] **Step 5: Hand off for commit**

The coordinator verifies, then commits:

```bash
git add crates/yserver/src/kms/executor/test_support.rs crates/yserver/src/kms/render/backend.rs
git commit -m "test(kms): a hardware fixture that holds DRM master

Part 3 of the stage 2c-i debt stage (spec section 9). Opens the primary
node Vulkan reports for the device it selected, takes DRM master,
probes the connected connector and builds its output through the
production probe, then allocates the scanout pool at the real mode. It
reports the card, connector and mode it used, and drops master on drop.
Without master it panics naming the active-session requirement (R12):
it never skips and never passes.

The existing fixtures are untouched and keep opening the node without
master, so the tests that encode the no-master outcome keep meaning
what they mean.

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 3: P3-1 — a managed buffer goes on screen through an accepted flip

**Files:**
- Create: `crates/yserver/src/kms/render/part3_tests.rs`; modify `crates/yserver/src/kms/render/mod.rs` to declare it under `#[cfg(test)]`.

**Invariant (spec 9.2, P3-1):** a managed scanout buffer submitted in a flip the kernel **accepts** becomes the current buffer — the flip-accepted path actually executes.

**What the test must establish, not how:**
- the buffer is **managed**: converted through `PlatformBackend::register_managed_scanout_bo`, rooted by its lease, exactly as `c0_2ci_scene_managed_shared_compose_vulkan` does today — that test is the closest existing relative and the right starting point;
- the frame goes through the **real** submission path, the one that calls `crate::drm::page_flip::submit_flip_with_fences`, not a hand-rolled commit;
- the flip is **accepted**: the submission returns `Ok`, and the assertion says so in terms of the kernel's answer, not of a fixture flag;
- the buffer is the CRTC's current one **after the matching completion has been consumed** (round-2 M-2), read back from the device rather than from the backend's own bookkeeping. Reading before that observes the old framebuffer and proves nothing.

- [ ] **Step 1: Write the test**

- [ ] **Step 2: Compile, lint, and confirm it does not run here**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`
Expected: clean; **180 passed, 19 ignored** — the new test is ignored, and no existing count moves.

- [ ] **Step 3: Hand off.** The coordinator additionally runs the test **without** master and confirms it **fails** with the fixture's message (R12), then commits.

---

### Task 4: P3-2 and P3-3 — the ledger against real kernel evidence

**Files:**
- Modify: `crates/yserver/src/kms/render/part3_tests.rs`

**Invariants:**
- **P3-2:** the displaced buffer's `KmsRelease` obligation is discharged by the **real** kernel completion, and the buffer is **not released before it**. Both halves matter: the test must observe the entry still rooted while the completion has not arrived, and released after it has. The completion must come from the kernel's event, read through the backend's own completion path — not a synthesized `OwnerEvent` — and must be the completion **matching** this submission, awaited under the deadline the Global Constraints impose (round-2 M-2).
- **P3-3:** a buffer **retained** across the flip registers no obligation and is not released — R6's retained-member clause, on real evidence.

- [ ] **Step 1: Write both tests**

- [ ] **Step 2: Compile, lint, deterministic suite**

Expected: clean; **180 passed, 21 ignored**.

- [ ] **Step 3: Hand off.** Same no-master check by the coordinator, then commit.

---

### Task 5: P3-4 — the out-fence resolves

**Files:**
- Modify: `crates/yserver/src/kms/render/part3_tests.rs`

**Invariant (P3-4):** the flip's out-fence resolves through its canonical status query.

- The fence comes back through `submit_flip_with_fences`'s `out_fence` parameter, which is an `i32` — see the Global Constraints; a wider type reads a bogus fd.
- "Canonical status query" means the same query the production code uses to decide a fence is signalled — `crate::platform::sync_file::query_status`, whose `FenceStatus` is `Pending | Success | Error(i32)`. Poll it until `Success`, `Error(_)` or the deadline; never invent a poll, and never treat `Pending` at the deadline as anything but a failure to report (round-2 M-2).
- **The test does not own the fd** (round-1 B-2): `submit_flip_with_fences` hands it to `bo.state.transition_to_pending`, which keeps it as `release_fence_fd`. Observe the fence through that owner. If an independent observation is genuinely needed, `dup` the fd and close only the duplicate, saying in the test which owner closes which descriptor.

- [ ] **Step 1: Write the test**

- [ ] **Step 2: Compile, lint, deterministic suite**

Expected: clean; **180 passed, 22 ignored**.

- [ ] **Step 3: Hand off for commit.**

---

### Task 6: What the gamma path does when its own fd holds master

**Files:**
- Modify: `crates/yserver/src/kms/render/part3_tests.rs`

**Why (round-1 M-3).** Spec 9.2's last paragraph asks which existing hardware
tests encode the no-master outcome, answered **by running them with master**.
The feasibility measurement ran the suite while master was merely *available*;
the fixtures' own fds never held it, so
`c0_2ci_sink_gamma_gate_four_states_drm`'s Legacy arm -- which asserts the gamma
ioctl is reached **and fails** -- was only ever inferred to be master-dependent.

**Invariant:** on a fd that **does** hold master, the same gamma path either
succeeds or fails for a reason that is not "no master", and the result is
recorded. This test does not change the existing one; it measures what the
existing one's assertion rests on.

**How, precisely (round-2 M-1).** Do **not** write a new test that merely
drives the same entry point over a different fixture: that would change the
fixture, the output selection and possibly which arms run, and would prove
something other than what the existing test's assertion rests on.

- [ ] **Step 1: Parameterise the existing test's body** into a helper that
  takes the DRM device (and whatever else it opens today) as arguments, leaving
  `c0_2ci_sink_gamma_gate_four_states_drm` calling it with its current
  `open_real_drm_or_ignore` device. Its meaning must not change: same four gate
  states, same assertions, same name.

- [ ] **Step 2: Add the master-held invocation** of that same helper, over the
  same four states, with the live-KMS fixture's master-holding device, as a new
  `_drm` test. Record what each state does. If the Legacy arm's outcome
  contradicts what the existing test encodes, **do not edit the existing
  test**: report it as an F8 stop for Task 7's record and a decision for the
  stage owner.

- [ ] **Step 3: Compile, lint, deterministic suite**

Expected: clean; **180 passed, 23 ignored** — the refactor must not move the deterministic count, and `c0_2ci_sink_gamma_gate_four_states_drm` must still exist under that name.

- [ ] **Step 4: Hand off for commit.**

---

### Task 7: The run protocol, the mutations, and the record

This task is **not** the implementer's to execute. The implementer writes Step 1's command file; the user runs it from an active VT; the coordinator records the result.

- [ ] **Step 1 (implementer): supply the exact commands**

Write `docs/superpowers/plans/part-3-run.sh` — the exact, copy-pasteable sequence spec 9.3 step 2 requires, which:
- refuses to start unless `loginctl show-seat seat0 -p ActiveSession` names this session (the check the user would otherwise forget, and the one that turns a confusing failure into a clear refusal);
- runs `cargo test -p yserver --lib c0_2ci -- --ignored --nocapture --test-threads=1` filtered to the part-3 tests, so the fixture's card/connector/mode report is visible and no two of them hold the device at once (round-1 M-1);
- then runs the mutations of Step 3 below, under the same filter, through `tools/guard-census.py --named-mutation` (Task 1);
- writes everything to `/tmp/part3-run.log` and prints where it put it.

- [ ] **Step 2 (user): run it**

From a VT that is the seat's **active** session — on this box tty2, with the desktop logged out — run `bash docs/superpowers/plans/part-3-run.sh` and hand back `/tmp/part3-run.log`.

- [ ] **Step 3 (user, inside that script): the named mutations**

Each must fail its named test, and each must be confirmed to have compiled:
- **`--named-mutation p3-2`:** discharge the `KmsRelease` obligation on submission instead of on the kernel completion — the test must fail on the "not released before" half, the half that distinguishes real evidence from a synthesized event;
- **`--named-mutation p3-3`:** register an obligation for the retained buffer — the test must fail on R6's retained-member clause.

Neither mutation counts unless the tool reports that it compiled.

- [ ] **Step 4 (coordinator): record**

Create `docs/superpowers/findings/2026-09-17-part-3-flip-accepted.md` with: the card, connector and mode the fixture reported; for every observation, whether it completed within its deadline, and for any that did not, whether it was a timeout or a fence `Error(_)` (round-2 M-2); the named existing gamma test and what its four states did with and without master (round-2 M-1); the four invariants and what was observed for each; both mutations and the test that caught each; the full log; and, prominently, spec 9.4's boundary — **part 3 is not the bounded delivery check of the C.0 design's section 16.3, satisfies none of its requirements, and is never reported as doing so**. Any F8 stop goes here too, including the one this plan expects most: whether a **PRIME-imported Vulkan image**, rather than a dumb buffer, can be flipped on this NVIDIA driver at all. The feasibility probe did not answer that, and a negative answer is a finding about the driver, not a failure of the plan.

Then add to the spec's status line: "Part 3 executed: …", naming what was proven and what was not.

## Self-review notes

- **Spec coverage.** 9.2's four invariants map to Tasks 3-5; its last paragraph — which existing tests encode the no-master outcome — maps to Task 6, which **executes** it on a master-holding fd instead of inferring it (round-1 M-3). 9.3's four steps map to Task 7, whose mutations run through the instrument Task 1 adds (round-1 M-2). 9.4 is carried into Task 7's record.
- **The one thing this plan cannot promise.** Whether P3-1 is achievable at all on this driver. The feasibility probe proved the kernel path with dumb buffers; the managed path imports a Vulkan image through PRIME and flips *that*. If NVIDIA refuses it, the honest output of this plan is an F8 report, and the stage still gains the fixture and the three other invariants.
- **Why no verbatim code.** Stated above, under "Why this plan states invariants instead of supplying code". The trade is deliberate: unrunnable prescribed code would be worse than an invariant the implementer must satisfy and the user must witness.
- **What round 1 changed about the risk profile.** Two of its five findings were about what happens when a test *fails*: the console left on test contents (B-1) and an fd closed twice (B-2). Both matter more here than in an ordinary stage, because part 3's mutations deliberately make tests panic on a live display. The teardown order and the fd ownership are now stated as constraints, not left to the test body.
