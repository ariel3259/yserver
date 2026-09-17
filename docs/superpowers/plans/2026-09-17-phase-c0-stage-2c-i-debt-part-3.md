# Stage 2c-i debt, part 3 — the flip-accepted path, with DRM master

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), `--sandbox workspace-write`, run with `< /dev/null`. Execute tasks in order, one at a time; tick steps (`- [ ]` → `- [x]`) only with the evidence each names. Before writing code, read `AGENTS.md` and, as plain markdown, the Superpowers skills `executing-plans/SKILL.md` and `test-driven-development/SKILL.md` under `~/.claude/plugins/cache/claude-plugins-official/superpowers/*/skills/`. **The implementer never commits**: this worktree's git directory is read-only inside the sandbox. Stop with the tree dirty; the coordinating session verifies and commits.

**Goal:** Put a managed scanout buffer on screen through a page flip the kernel actually accepts, and prove the stage 2c-i ledger against the kernel's own completion and out-fence instead of a synthesized event.

**Architecture:** A second hardware fixture, beside the existing ones, that takes DRM master and builds its output from the live card through the production probe. Four `_drm` tests over it, one per invariant of spec section 9.2. Nothing existing changes shape: the current fixtures keep opening the node without master, so the tests that encode the no-master outcome keep meaning what they mean.

**Tech Stack:** Rust (`cargo test`), libdrm through the `drm` crate, a live Vulkan ICD, a real DRM card with a connected output.

**Spec:** `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` section 9. Sections 9.2 (invariants), 9.3 (how it is run) and 9.4 (what it is not) govern this plan.

**Prior measurement:** `docs/superpowers/findings/2026-09-17-part-3-drm-master-feasibility.md` — read it before Task 1. It establishes, on this box, that master is reachable from an active VT, that `card1` (NVIDIA) drives the connected output and is the node Vulkan reports, and that a modeset, a page flip, a kernel completion and an atomic `OUT_FENCE_PTR` all work there with dumb buffers.

## Why this plan states invariants instead of supplying code

Session 2's plan handed the implementer verbatim blocks, because the coordinating session had prototyped and measured every one of them first. That is impossible here: these tests only run with DRM master, which logind grants to the **active** session on the seat, so neither the implementer's sandbox nor the coordinating session can execute them while writing them. Prescribing unrunnable code would be prescribing guesses. Spec 9.2 says so too — "the shape of the tests is the implementer's call".

So this plan fixes the **fixture contract**, the **invariants**, and the **commands the user runs**, and leaves the test bodies to the implementer. What the implementer can still verify mechanically is stated per task: it compiles, it lints, the deterministic suite is unaffected, and the new tests **fail** rather than pass when run without master.

## Global Constraints

- **R12, sharpened.** Every test here carries the `_drm` suffix and `#[ignore]`, and **detects whether it holds master**. Without master it reports an environmental failure — `panic!` with a message naming the cause — and never passes. A test that cannot tell "no master" from "passed" is the one defect this part exists to avoid.
- **R8:** nothing here becomes production-active. The fixture is `#[cfg(test)]`.
- **F3:** do not mock `ResourceService`; the tests drive the real service.
- **F8:** if an invariant cannot be proven because the code or the driver does not do what the spec assumes, **stop and report it**. A part-3 test that cannot pass is a finding about the ledger or the driver, which is the point of the exercise; it is never something to weaken into a pass.
- **The existing fixtures keep opening the node without master.** `KmsBackend::for_tests_with_vk*` and `PlatformBackend::for_tests` are not touched. `c0_2ci_sink_gamma_gate_four_states_drm` asserts the gamma ioctl is *reached and fails* — true only without master (see the prior measurement). If a change here would put master under it, that is an F8 stop, not a silent edit of its assertion.
- **Master is released on drop.** The fixture drops master in its `Drop`, so a panicking test still returns the console.
- **`OUT_FENCE_PTR` writes an `s32`.** Read it into `i32`, as `scene.rs` already does. A wider destination reads a bogus fd — measured: `0xFFFFFFFF00000004` for fd `4`.
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
- Create: `crates/yserver/src/kms/render/part3_tests.rs` — the four `_drm` tests (Tasks 2–4), declared from `render/mod.rs` under `#[cfg(test)]`.
- Create ([H], user + coordinator): `docs/superpowers/findings/2026-09-17-part-3-flip-accepted.md` (Task 5).

---

### Task 1: The live-KMS fixture

**Files:**
- Modify: `crates/yserver/src/kms/executor/test_support.rs`, `crates/yserver/src/kms/render/backend.rs`

**Interfaces:**
- Produces: `KmsBackend::for_tests_with_live_kms() -> Result<LiveKmsFixture, io::Error>`, where `LiveKmsFixture` owns the backend and drops master on drop. Its contract:
  1. builds the `VkContext` first, and opens **the primary node that `VK_EXT_physical_device_drm` reports for the physical device Vulkan selected** — the existing `TestDevice::open_real_drm_matching` path, not a blind `card0`;
  2. takes DRM master on it. On `EACCES` it **panics** with a message that names the cause: master is granted to the seat's active session, so the test must run from an active VT. Never a skip, never a pass;
  3. probes that device's connectors, picks the first **connected** one with modes, resolves its encoder, CRTC and primary plane, and builds the output through the production path `crate::drm::modeset::output_for_exact_probe_assignment`, with the connector's preferred mode. A box with no connected output panics with that reason;
  4. installs that `ActiveOutput`, and allocates the scanout pool at the mode's real resolution;
  5. **reports, on stdout, the card path, the connector name and the mode it selected** — spec 9.3 step 2 requires the run to say which card was used;
  6. drops master in `Drop`.
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

### Task 2: P3-1 — a managed buffer goes on screen through an accepted flip

**Files:**
- Create: `crates/yserver/src/kms/render/part3_tests.rs`; modify `crates/yserver/src/kms/render/mod.rs` to declare it under `#[cfg(test)]`.

**Invariant (spec 9.2, P3-1):** a managed scanout buffer submitted in a flip the kernel **accepts** becomes the current buffer — the flip-accepted path actually executes.

**What the test must establish, not how:**
- the buffer is **managed**: converted through `PlatformBackend::register_managed_scanout_bo`, rooted by its lease, exactly as `c0_2ci_scene_managed_shared_compose_vulkan` does today — that test is the closest existing relative and the right starting point;
- the frame goes through the **real** submission path, the one that calls `crate::drm::page_flip::submit_flip_with_fences`, not a hand-rolled commit;
- the flip is **accepted**: the submission returns `Ok`, and the assertion says so in terms of the kernel's answer, not of a fixture flag;
- the buffer is the CRTC's current one afterwards, read back from the device rather than from the backend's own bookkeeping.

- [ ] **Step 1: Write the test**

- [ ] **Step 2: Compile, lint, and confirm it does not run here**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`
Expected: clean; **180 passed, 19 ignored** — the new test is ignored, and no existing count moves.

- [ ] **Step 3: Hand off.** The coordinator additionally runs the test **without** master and confirms it **fails** with the fixture's message (R12), then commits.

---

### Task 3: P3-2 and P3-3 — the ledger against real kernel evidence

**Files:**
- Modify: `crates/yserver/src/kms/render/part3_tests.rs`

**Invariants:**
- **P3-2:** the displaced buffer's `KmsRelease` obligation is discharged by the **real** kernel completion, and the buffer is **not released before it**. Both halves matter: the test must observe the entry still rooted while the completion has not arrived, and released after it has. The completion must come from the kernel's event, read through the backend's own completion path — not a synthesized `OwnerEvent`.
- **P3-3:** a buffer **retained** across the flip registers no obligation and is not released — R6's retained-member clause, on real evidence.

- [ ] **Step 1: Write both tests**

- [ ] **Step 2: Compile, lint, deterministic suite**

Expected: clean; **180 passed, 21 ignored**.

- [ ] **Step 3: Hand off.** Same no-master check by the coordinator, then commit.

---

### Task 4: P3-4 — the out-fence resolves

**Files:**
- Modify: `crates/yserver/src/kms/render/part3_tests.rs`

**Invariant (P3-4):** the flip's out-fence resolves through its canonical status query.

- The fence comes back through `submit_flip_with_fences`'s `out_fence` parameter, which is an `i32` — see the Global Constraints; a wider type reads a bogus fd.
- "Canonical status query" means the same query the production code uses to decide a fence is signalled; find it and use it, rather than inventing a poll.
- The test owns the fd it receives and closes it.

- [ ] **Step 1: Write the test**

- [ ] **Step 2: Compile, lint, deterministic suite**

Expected: clean; **180 passed, 22 ignored**.

- [ ] **Step 3: Hand off for commit.**

---

### Task 5: The run protocol, the mutations, and the record

This task is **not** the implementer's to execute. The implementer writes Step 1's command file; the user runs it from an active VT; the coordinator records the result.

- [ ] **Step 1 (implementer): supply the exact commands**

Write `docs/superpowers/plans/part-3-run.sh` — the exact, copy-pasteable sequence spec 9.3 step 2 requires, which:
- refuses to start unless `loginctl show-seat seat0 -p ActiveSession` names this session (the check the user would otherwise forget, and the one that turns a confusing failure into a clear refusal);
- runs `cargo test -p yserver --lib c0_2ci -- --ignored --nocapture` filtered to the part-3 tests, so the fixture's card/connector/mode report is visible;
- then runs the mutations of Step 3 below, under the same filter, through `tools/guard-census.py`;
- writes everything to `/tmp/part3-run.log` and prints where it put it.

- [ ] **Step 2 (user): run it**

From a VT that is the seat's **active** session — on this box tty2, with the desktop logged out — run `bash docs/superpowers/plans/part-3-run.sh` and hand back `/tmp/part3-run.log`.

- [ ] **Step 3 (user, inside that script): the named mutations**

Each must fail its named test, and each must be confirmed to have compiled:
- **for P3-2:** discharge the `KmsRelease` obligation on submission instead of on the kernel completion — the test must fail on the "not released before" half, which is the half that distinguishes real evidence from a synthesized event;
- **for P3-3:** register an obligation for the retained buffer — the test must fail on R6's retained-member clause.

- [ ] **Step 4 (coordinator): record**

Create `docs/superpowers/findings/2026-09-17-part-3-flip-accepted.md` with: the card, connector and mode the fixture reported; the four invariants and what was observed for each; both mutations and the test that caught each; the full log; and, prominently, spec 9.4's boundary — **part 3 is not the bounded delivery check of the C.0 design's section 16.3, satisfies none of its requirements, and is never reported as doing so**. Any F8 stop goes here too, including the one this plan expects most: whether a **PRIME-imported Vulkan image**, rather than a dumb buffer, can be flipped on this NVIDIA driver at all. The feasibility probe did not answer that, and a negative answer is a finding about the driver, not a failure of the plan.

Then add to the spec's status line: "Part 3 executed: …", naming what was proven and what was not.

## Self-review notes

- **Spec coverage.** 9.2's four invariants map to Tasks 2–4; 9.2's last paragraph (which existing tests encode the no-master outcome) was already answered by the prior measurement and is carried into the Global Constraints as a "do not touch". 9.3's four steps map to Task 5. 9.4 is carried into Task 5's record.
- **The one thing this plan cannot promise.** Whether P3-1 is achievable at all on this driver. The feasibility probe proved the kernel path with dumb buffers; the managed path imports a Vulkan image through PRIME and flips *that*. If NVIDIA refuses it, the honest output of this plan is an F8 report, and the stage still gains the fixture and the three other invariants.
- **Why no verbatim code.** Stated above, under "Why this plan states invariants instead of supplying code". The trade is deliberate: unrunnable prescribed code would be worse than an invariant the implementer must satisfy and the user must witness.
