# Part 3 plan — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md` revision 1 @ `f24113b6`
**Result:** 2 blocking, 3 major, 0 minor; coverage complete for declared scope (12/12 excerpts).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `783089b4`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.154.0`.
Counts are comparable only to other reviews citing this same instrument SHA.

## Author verification (2026-09-17)

All five verified; all five taken. Revision 2 carries them.

- **B-1 — CONFIRMED.** The plan said only "drops master in `Drop`" and never
  carried over what the feasibility probe itself did: restore the CRTC first.
  With mutations that deliberately panic on a live display, that is the
  difference between a console that comes back and one that does not. Revision
  2 states the snapshot and the teardown order: restore, then destroy test
  resources, then drop master, with a failed restore reported loudly.
- **B-2 — CONFIRMED against the tree.** `submit_flip_with_fences` hands the fd
  to `bo.state.transition_to_pending`, which stores it as `release_fence_fd`
  (`vk/scanout.rs:393`). A test that closed it too would leave a dead
  descriptor and could double-close it later. Revision 2 makes the BO state the
  sole owner; an independent observation `dup`s.
- **M-1 — CONFIRMED.** `cargo test` runs in parallel by default, master is
  per-fd and the CRTC is shared, so two live-KMS fixtures would contend.
  Revision 2 pins `--test-threads=1` **and** takes a process-wide guard.
- **M-2 — CONFIRMED against the tree.** The retained branch is
  `if !retained_in_new { register_kms(...) }` (`resources/commit.rs`), which is
  not a refusal guard, and the census tool only forces conditions to `false`.
  The mutation P3-3 needs is the opposite. Revision 2 adds Task 1: a
  `--named-mutation` mode in the tool itself, so spec 9.3 step 3 is satisfied
  literally rather than by an ad-hoc edit.
- **M-3 — CONFIRMED, and the author's overstatement.** The feasibility finding
  said plainly that the fixtures open the node without master, and the plan
  then claimed 9.2's last paragraph was "already answered". Running the suite
  while master was merely *available* is not running it *with* master. Revision
  2 adds Task 6, which drives the gamma path on a master-holding fd and records
  what the kernel does.

No implementation has been dispatched.

---

## Verdict

**2 blocking, 3 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result only. It does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status |
|---|---|
| None | No prior review exists; check 1 was skipped as directed. |

## Findings

### Blocking

#### B-1 — The fixture has no display-state restoration contract

The fixture modesets a live connector and installs test scanout state, but its only specified recovery action is dropping DRM master ([plan lines 58–64](docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md#L58), [line 28](docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md#L28)). The feasibility probe explicitly restored the original CRTC before dropping master ([feasibility lines 44–57](docs/superpowers/findings/2026-09-17-part-3-drm-master-feasibility.md#L44)).

Concrete failure sequence:

1. The fixture takes master and modesets a test framebuffer.
2. A test panics, including an expected mutation failure.
3. `Drop` relinquishes master without restoring the previous connector/CRTC/framebuffer state.
4. Backend fields subsequently destroy test framebuffer resources while the active VT has not been restored to its prior display state.

This can leave the console blank, showing stale test contents, or dependent on driver cleanup behavior. It is especially unsafe because mutation runs deliberately cause panics.

Required correction: make the fixture snapshot the prior KMS state and specify an unwind-safe teardown order: while master and test resources remain alive, restore the original connector/CRTC/framebuffer state; then destroy test resources; finally drop master. Restoration failure must be prominently reported rather than hidden by an existing panic.

#### B-2 — P3-4 assigns the out-fence to two incompatible owners

Task 4 states that the test owns and closes the out-fence fd ([plan lines 158–163](docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md#L158)). In the production submission path, however, the fd returned through `OUT_FENCE_PTR` is immediately transferred into BO state by `transition_to_pending(out_fence)` ([scene.rs lines 7893–7907](crates/yserver/src/kms/render/scene.rs#L7893)). That state is therefore the canonical owner.

If the test extracts and closes that same fd, the BO state retains a dead descriptor and can later double-close it; descriptor reuse can make the later close affect an unrelated resource. If instead the test submits directly and owns the fd, it no longer establishes that the production submission/ownership path feeds the canonical status query.

Required correction: select one ownership model explicitly. Prefer driving the production submission path, observing fence status through BO state, and leaving closure to that owner. If independent observation needs an fd, duplicate it and state which owner closes each descriptor.

### Major

#### M-1 — The run contract does not serialize tests sharing one DRM device and output

Task 5 runs all part-3 tests under one filtered `cargo test` command ([plan lines 178–184](docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md#L178)), but does not require `--test-threads=1` or an equivalent process-wide lock.

Each test independently opens the same primary node, takes master, selects the same connected output, and modesets it. With Rust’s default parallel test execution, fixtures can contend for master, consume one another’s DRM events, or overwrite live CRTC state. The result can be environmental failures or, worse, observations attributed to the wrong test.

Required correction: mandate serial execution in the command contract and add an in-process exclusive guard around the live-KMS fixture so alternative filters or future invocations cannot accidentally run these tests concurrently.

#### M-2 — The prescribed census tool cannot perform the two named mutations

The plan requires the script to run semantic mutations through `tools/guard-census.py` ([plan lines 180–194](docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md#L180)); spec 9.3 likewise requires the named mutations with the census tool ([spec lines 478–482](docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md#L478)).

The tool is a refusal-guard neutralizer over resource-service files ([guard-census.py lines 1–18](tools/guard-census.py#L1)). P3-3’s named mutation must make retained allocations enter the registration branch, but the actual branch registers only under `if !retained_in_new` ([commit.rs lines 606–617](crates/yserver/src/kms/render/resources/commit.rs#L606)); neutralizing a refusal guard cannot invert that condition. P3-2’s “discharge on submission” mutation is likewise a lifecycle relocation, not a refusal-guard deletion.

Consequently, the promised exact script cannot currently produce the required mutants through this tool, so mutation evidence would be absent or test unrelated guards.

Required correction: extend the tool with explicit, reversible named mutation modes for P3-2 and P3-3, including restoration on failure/Ctrl-C and a compile-success check, or provide an equally safe custom-mutation interface within it.

#### M-3 — The claimed master-held audit of existing tests never gives their DRM fd master

Spec 9.2 requires existing hardware tests that encode the no-master result to be identified by **running them with master** and recording the result ([spec lines 464–468](docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md#L464)). The plan declares this already answered and forbids master from reaching existing fixtures ([plan lines 27 and 202–205](docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md#L202)).

The cited measurement says the ignored suite ran while master was merely available; its fixtures deliberately opened separate non-master fds, so master “never reaches them” ([feasibility lines 72–85](docs/superpowers/findings/2026-09-17-part-3-drm-master-feasibility.md#L72)). Thus the gamma test’s master-held behavior was inferred, not executed.

Required correction: add a scoped master-held execution of the identified gamma path using the same DRM fd on which its ioctl runs, while preserving the ordinary fixture’s no-master semantics, and record the observed changed outcome or F8 stop.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** no prior review; correctly skipped.
- **Architecture/contracts:** checked fixture ownership, device selection, event evidence, shared-hardware execution, and mutation producer/consumer contracts.
- **Safety/ownership:** checked master lifetime, display recovery, fd ownership, panic teardown, and concurrent access.
- **Compliance/verification:** mapped all of spec 9.2–9.4 and checked the real-evidence, no-master, mutation, logging, and boundary claims.
- **Excerpts used:** **12/12**. Verified ground includes the full plan, spec sections 3.0 and 9, the feasibility audit, live-scene fixture behavior, page-flip retirement, production out-fence transfer, retained-member registration, and census-tool scope.
- **Unassessed:** exact connector/encoder/plane selection APIs, implementation signatures, and detailed test-body mechanics. These are not deemed sound; they were outside the bounded excerpt budget.
- **Deferred to implementation:** Rust type/borrow correctness, ioctl portability, formatting, `cargo clippy --all-targets -- -D warnings`, deterministic counts, hardware execution, mutation compilation, and actual kernel/driver results.