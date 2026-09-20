# Stage 2c-iii, plan Ci-refactor — one owner buffer state machine, one dispatch path

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for hardware work) with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator commits); of the `#[ignore]` tests run only this plan's filters (`c0_conv_ci_`, `c0_adm`, `c0_2ci` without `--ignored`), never `_drm`, `render_acceptance`, unfiltered `--ignored`, or anything that modesets or takes DRM master; no deletes outside the worktree. **You write the implementation**; this plan gives the interfaces, the invariants and the checks. Execute tasks in order, one per run. Do not ask for approval; a real design choice the plan leaves open, or a claim here that does not hold in the code, is an F8 stop you report.

**Revision 4 (2026-09-19)** — incorporates codex round 2 (`../findings/2026-09-19-stage-2c-iii-plan-ci-refactor-review-round2.md`: 1 blocking, 1 major, both verified and accepted; round 1: B-2, M-1 and M-2 APPLIED, B-1 TRADED into this round's B-1).
- **B-1:** the spec's §8.3 defined this plan as behaviour-preserving, so Task 4 had no authority for any policy change. **Spec §8.3 is amended first** (`a1adc3b1`, user's decision): the refactor may collapse dispatch-failure policies that production cannot reach, provided every reachable row keeps today's behaviour, the unreachable ones are identified by recorded measurement, and the acceptance numbers, the mutation parity and the hardware gate are unchanged.
- **M-1:** the reachable-only matrix cannot witness a mutation of the collapsed fallback, so Task 4 adds a policy-level test on the real policy function and owns both mutations itself.

**Revision 3 (2026-09-19)** — Task 4 replaced, on measured evidence (user's decision after Task 3).
Task 3 measured the reachability of decision 4's policy table through production entries: **only 3 of its 11 rows are reachable** — primary `Ledger`/restore-succeeded, primary `Refused`, and direct `Ledger`/undo-succeeded together with direct `Refused`. The other eight are F8 (recorded in Task 3's commit `bd429355` and in the matrix test): a `Cleanup` error needs `slot.release` to fail on the commit just reserved, and the direct missing/extra-resource rows cannot be produced by a real preparation. Making those cells "consistent", as revision 2's Task 4 proposed, would change behaviour no test driven through production can observe. Instead Task 4 now **collapses them**: one fail-closed path, and the table keeps only the rows that describe observable behaviour.

**Revision 2 (2026-09-19)** — incorporates codex round 1 (`../findings/2026-09-19-stage-2c-iii-plan-ci-refactor-review-round1.md`: 2 blocking, 2 major, all verified against the tree and accepted).
- **B-1:** the dispatch failure arms are not uniform today; decision 4 now carries the exact current policy matrix, Tasks 1–3 preserve it, and the three inconsistencies it exposes are fixed separately, as an explicit behaviour change, in Task 4.
- **B-2:** `Quarantined` is a minimal poison marker, not a payload holder: today `CompletionUnknown` drops the scene record.
- **M-1:** each `OwnerBuffer` carries immutable identity and a total order (generation), and restoration keeps the original position.
- **M-2:** the membership test is renamed and joined by an exact-state sequence test; R19 is re-applied on the new release helper instead of inheriting its old status.

**Goal:** Remove two duplications plan Ci left behind without changing behaviour (Tasks 1–3), then fix the dispatch-failure inconsistencies the refactor exposed (Task 4, the only behaviour change): (b) an owner-route scanout buffer's state lives in two places that must agree — eight `BoPhase::Owner*` variants and four `PreparedComposed` queues in the scene; (d) the primary and direct dispatches each carry their own copy of the ledger-failure and restore arms, and the owner has fallible and infallible `begin` entries with duplicated bodies.

**Spec:** `docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md` §8.3 ("Plan Ci-refactor"). Authoritative behaviour: plan Ci's decision 10 (the owner buffer lifecycle table) and decision 11 (member identity), in `2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md`. Read the acceptance finding `../findings/2026-09-19-stage-2c-iii-plan-ci-accepted.md` and the Ci commit messages (`git log 914f2818^..f153c3a5`) before Task 1.

## Global Constraints

- **Tasks 1–3 are behaviour-preserving.** No outcome, event, ordering, resource movement or damage action changes. **Task 4 is the only behaviour change**, limited to decision 4's table. Production (`Legacy`) is untouched throughout.
- **Tests are the specification.** In Tasks 1–3 no assertion of an existing test changes. A test helper may change how it *reads* state (because the state moved), never what it asserts. If an assertion itself would have to change, that is an F8 stop, not an edit.
- New owner code lives in its own module, reached from the existing fork points; no new branches inside legacy functions (the Cii/Ciii constraint applies here too).
- No side effect inside `debug_assert!`; fail closed, never panic, in non-test code; no test-only hook that bypasses a path.

## Design decisions this plan fixes

1. **One owner of an owner buffer's state.** A new module, `crates/yserver/src/kms/render/owner_buffer.rs`, holds `OwnerBuffer`: one value per owner-held scanout buffer, an enum whose variants are decision 10's states (Rendering, Desired, Displaced, Submitted, Accepted, Current, Releasing, Quarantined), each carrying exactly the data valid in that state (the captured `PendingAck`, the allocation lease when the state owns one, the `CommitId` from Submitted to Current, the descriptor slot while it is held). Transitions consume the value and return the next state, or refuse and hand the value back unchanged. Illegal transitions cannot produce a value.
   - **Identity, immutable across states (round-1 M-1):** `OutputKey`, CRTC id, buffer index, generation and managed allocation key, as `PreparedComposed` carries them today. Every lookup the scene performs today by output key, commit id or generation keeps matching on the same fields.
   - **`Quarantined` is a poison marker (round-1 B-2):** it keeps only the identity above. The transition into it drops the `PendingAck`, any lease, the descriptor slot and the `CommitId` — exactly what `CompletionUnknown` drops today when it discards the scene record (`scene.rs`, the "no quarantine queue" comment). It has no exit in this stage.
2. **`BoPhase` keeps one owner variant.** The eight `Owner*` variants collapse into a single variant meaning "held by the owner route; ask the owner machine". The pool's acquisition, which only needs "is this buffer `Free`", is unaffected; every place that inspected an `Owner*` phase asks the `OwnerBuffer` instead.
3. **One place per output.** The scene's four queues (`owner_prepared`, `owner_submitted`, `owner_current`, `owner_displaced`) become one per-output collection of `OwnerBuffer` keyed by buffer index, **ordered by generation** (round-1 M-1). That total order replaces the queues' FIFO positions: `owner_submitted` and `owner_displaced` are appended in generation order today, and a failed resolution that currently re-inserts a member at its original queue index keeps its place because its generation does not change. "The newest prepared generation" is the highest-generation Rendering/Desired entry, and "the current buffer" is the single Current entry per output; a second Current entry for one output is refused by the collection. Inserting and removing an entry happens through one helper that also sets and clears the buffer's owner `BoPhase`, so the two cannot disagree.
4. **One dispatch failure path, parameterised by today's policy (round-1 B-1).** `FallibleBeginError::{Ledger, Cleanup, Refused}` handling in `admission_dispatch_primary` and `admission_dispatch_direct` goes through one shared mechanism (restore old state to current, return the new state to its source — composed → scene, direct → `managed_undo_direct_dispatch` — close the gate or not, abort the token), driven by an explicit policy table. **Tasks 1–3 reproduced the pre-collapse table exactly**, as measured at `bdc44848`; Task 4 replaces its unreachable rows under the amended §8.3:

   | Dispatch | Error | Restore of the new state | Outcome | Gate closed |
   | --- | --- | --- | --- | --- |
   | primary | Ledger | succeeds | `BeginRefused` | no |
   | primary | Refused | not attempted | `BeginRefused` | no |
   | direct | Ledger | undo succeeds | `BeginRefused` | no |
   | direct | Refused | resource present; undo succeeds or fails (result ignored) | `BeginRefused` | no |
   | either | any Cleanup, failed restore other than direct Refused, or missing/extra resource | — | `TransportClosed` | yes |

   The shared fail-closed fallback handles all collapsed cells, but the behavior-changing collapse covers only production-unreachable cells. `Cleanup` needs the owner to fail releasing the slot reserved by that same `begin`; the slot is exclusively owned during the synchronous ledger closure, so no production entry can produce it. Primary restoration receives only the state just moved and its production inverse is exact; direct preparation supplies exactly one resource, so missing/extra resources are not production outputs. The kept primary `Ledger`/restore-succeeded row has no capacity reservation, serial or frozen-allocation failure in its restore operation; its inverse either succeeds as measured or falls into the old fail-closed policy. The kept primary `Refused` row does not attempt restoration. The kept direct `Ledger`/undo-succeeded row can, in principle, see an already-closed capacity ledger or an exhausted serial during undo; a role mismatch is excluded by the synchronous ownership interval, and frozen allocations do not participate in undo. Ci already mapped a direct-Ledger undo failure to `TransportClosed` with the gate closed, so that shared result preserves behavior. The only changed-action cells are the production-unreachable Cleanup combinations, direct-Ledger missing-resource case, and direct-Refused missing/extra-resource cases. Finally, direct `Refused` is deliberately not collapsed: preparation moves the queued `Successor` reservation to `Submitted` and `attach` occupies that exact token, while the owner returns `Refused` before invoking the ledger closure. The capacity ledger can nevertheless already be closed by a prior production consumer error, making `reserve(Successor)` return `Busy`; the matrix drives that real consumer error path and records the failed undo. Ci ignores either undo result, so the complete row—resource present, undo succeeds or fails—remains `BeginRefused` with the gate open.
5. **One owner `begin` body.** `begin_with_ledger` and `begin_with_context_and_ledger` are expressed through their fallible counterparts (an infallible closure is a fallible one that never fails), so each refusal check exists once.

## Checks every task must keep green

Run by the implementer and re-run by the coordinator:

```bash
cargo build -p yserver --bin yserver
cargo build --release -p yserver --bin yserver
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features tcp-transport -- -D warnings
cargo clippy --all-targets --features xdmcp -- -D warnings
for i in 1 2 3 4 5; do cargo test -p yserver --lib c0_conv_ci_; done
cargo test -p yserver --lib c0_conv_ci_ -- --include-ignored --test-threads=1
cargo test --release -p yserver --lib c0_conv_ci_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_adm
cargo test -p yserver --lib c0_2ci
cargo test -p yserver --lib
```

Baseline before Task 1 (commit `5fe87a75`): `c0_conv_ci_` 38/38 with `--include-ignored` in debug and release; `c0_adm` 129/0; `c0_2ci` 180/0/21; `--lib` 1921/0/121. Every task ends at exactly these numbers plus its own new deterministic tests.

---

### Task 1: The `OwnerBuffer` type and its transitions

**Files:** new `crates/yserver/src/kms/render/owner_buffer.rs` (registered in `render/mod.rs`); `scene.rs` only to move the types it needs (`PreparedComposed` and whatever it carries) if they belong with the new module.

**Invariants:** every transition of decision 10's table exists as a consuming method; every other transition is refused without losing the value; a state owns an allocation lease only where decision 10 says the owner route holds one; `Submitted` and later carry their `CommitId`. Not yet wired into the scene: dead-code allowances naming Task 2 are acceptable if clippy requires them.

**Named tests (deterministic):** `c0_conv_cir_owner_buffer_legal_transitions` (every row of decision 10, and the identity fields unchanged across each), `c0_conv_cir_owner_buffer_refuses_illegal_transitions` (at least: Rendering → Submitted, Desired → Current, Current → Free without Releasing, Quarantined → anything; each returns the value unchanged), and `c0_conv_cir_quarantine_drops_the_payload` (entering Quarantined leaves no `PendingAck`, lease, descriptor slot or `CommitId` owned by the marker). Building the payloads through test constructors is fine here: this task tests the type, not the route.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 2: Migrate the scene and platform to `OwnerBuffer`

**Files:** `scene.rs`, `platform.rs`, `vk/scanout.rs`, and the `c0_conv_ci_` test helpers in `backend.rs` that read the removed queues or phases.

**Invariants:** decision 2 and decision 3 hold; the four queues and the eight `Owner*` `BoPhase` variants are gone; one helper inserts/removes an `OwnerBuffer` together with the buffer's owner `BoPhase`; every transition the scene, the drain, the admission closure, the owner events and the release pass performed before goes through `OwnerBuffer`'s methods. Behaviour, including every Ci F8 and every characterisation test, is unchanged.

**Checks:** the full list above, with the GPU ones. Add two tests (round-1 M-2):
- `c0_conv_cir_owner_membership_agrees_vulkan`: through a full owner cycle (tick, drain, admission, Accepted, HardwareComplete, completion, the next generation's retirement, release), after every step each buffer is `BoPhase` owner-held if and only if the scene holds an `OwnerBuffer` for it;
- `c0_conv_cir_owner_state_sequence_vulkan`: the same cycle, asserting after every milestone the **exact** logical state and identity of each involved buffer (Rendering → Desired → Submitted → Accepted → Current, the previous one Current → Releasing → gone), plus a displacement, a pre-IPC refusal and a `CompletionUnknown` branch.

- [ ] Steps: migrate; checks; stop dirty and report the old → new mapping of every removed field and phase.

---

### Task 3: One dispatch failure path, one owner `begin` body

**Files:** `admission.rs`, `owner/device.rs`.

**Invariants:** decisions 4 and 5; decision 4's table holds cell for cell (the same `AdmissionOutcome`, the same resources returned to the same owners, the same gate state, the same token abort); every owner refusal check exists once.

**Named test:** `c0_conv_cir_dispatch_failure_policy_matrix` — one case per row of decision 4's table, driven through the real dispatches (forcing the ledger error, the cleanup failure and the refusal the way the existing Ci and 2c-ii tests do; say how each row is reached, F8 for a row that cannot be reached through production entries), asserting outcome, gate state and where every resource ended up.

**Checks:** the full list above. The existing `c0_conv_ci_registration_failure_aborts_the_token`, `c0_conv_ci_owner_*`, and the 2c-ii `c0_adm_conductor_*` refusal tests are the evidence; no new test is required unless a path had none — then add it and say why.

- [ ] Steps: refactor; checks; stop dirty and report.

---

### Task 4: Collapse the unreachable failure rows into one fail-closed path (behaviour change)

**Files:** `admission.rs`; the matrix test; decision 4's table in this plan.

Revision 3's decision, from Task 3's measurement: eight of the eleven rows cannot be reached through production entries, and three of them claim distinct policies for situations that cannot occur. A table that describes unobservable behaviour is where the next reader's wrong assumption comes from.

**Invariants:**
- The policy keeps exactly the reachable rows, each with today's behaviour unchanged: primary `Ledger` with a successful restore → `BeginRefused`, gate open; primary `Refused` → `BeginRefused`, gate open; direct `Ledger` with a successful undo → `BeginRefused`, gate open; direct `Refused` with its resource present, whether undo succeeds or fails → `BeginRefused`, gate open. Direct `Refused` is the explicit exception to the collapse: the capacity ledger may already be closed by a prior production consumer error, causing undo's `reserve(Successor)` to return `Busy`; Ci ignores that result.
- Every failure outside the explicit open rows — any `Cleanup`, any restore failure, or any resource missing or unexpected, on either dispatch — takes **one** fail-closed path: close the gate, abort the token, return `TransportClosed`, and log which condition it was. `Cleanup` is production-unreachable because the owner exclusively owns its newly reserved slot during the synchronous closure; primary restoration receives only the state just moved; and direct preparation supplies exactly one resource. The behavior-changing Cleanup and missing-resource cells are F8; primary/direct-Ledger restore failures that were already fail-closed in Ci retain that same result. Direct-Ledger undo failures from an already-closed ledger or exhausted serial therefore remain safe under the shared result. Frozen allocations can make resource registration or release fail, but cannot make the direct undo's capacity operations fail.
- No unreachable condition keeps a policy of its own, and the mechanism has no cell that no caller can produce.
- Decision 4's table in this plan is rewritten to the new policy in the same change, with a line naming the rows that were collapsed and why.

**Named tests:**
- `c0_conv_cir_dispatch_failure_policy_matrix`, updated: one case per reachable row, driven through the real dispatches, asserting outcome, gate state and where every resource ended up, plus a note naming the collapsed conditions and why they are unreachable. Mutation that must fail it: give a reachable row the fail-closed policy.
- `c0_conv_cir_dispatch_failure_fallback_is_fail_closed` (round-2 M-1): a policy-level test calling the **real** policy function with a representative collapsed condition per dispatch, asserting closed gate and `TransportClosed`. The test states in one line that it proves the mapping only — not reachability, not end-to-end restoration. Mutation that must fail it: give the fail-closed path a reachable row's policy (gate left open, `BeginRefused`).

Both mutations are part of this task's own evidence, not only of the plan's acceptance step.

- [ ] Steps: update the test and the plan's table; collapse the policy; checks; stop dirty and report.

---

## What the coordinator does

After each task: reads the diff against the invariants and the constraints (no assertion changed, no legacy change, no owner branch inside a legacy function), re-runs every check outside codex, and commits with `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)` and `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.

After Task 3, the acceptance:

1. **Mutation parity.** Re-apply, by line on the new code, every mutation Ci's acceptance lists as caught, and confirm each still fails its test: R1, R2, R4, R5, R6, R7, R9, R10, R12, R14, R15, R16, R17, R21, R24, R27, R28, R29, R30, R31 (quarantine, mechanism failure, topology), R33, the `add_damage` skip, never freeing a displaced buffer, F-T5-2..5, no quarantine on unknown, both dormancy route mutations, and R22, R23, R25, R26. Plus new mutations: a path that changes an `OwnerBuffer` without the owner `BoPhase` (or the reverse) must fail `c0_conv_cir_owner_membership_agrees_vulkan`; a transition to the wrong logical state (for example Desired straight to Accepted) must fail `c0_conv_cir_owner_state_sequence_vulkan`; **R19 is re-applied on the new release helper** (let it free an `OwnerBuffer` that is not Releasing) and its new result recorded — it does not inherit "equivalent", because the phase guard it relied on is gone (round-1 M-2). The other equivalent and structural ones (R3, R32, R8, R11, R13, R18, R20, R34, F-T5-1) keep their recorded status.
2. The hardware gate (all three suites), with the user's go-ahead.
3. An acceptance finding and a `docs/status.md` entry.
