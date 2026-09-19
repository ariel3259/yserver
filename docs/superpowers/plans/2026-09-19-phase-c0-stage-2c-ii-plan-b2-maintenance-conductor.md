# Stage 2c-ii, plan B2 — maintenance in the conductor

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), `--sandbox workspace-write`, run with `< /dev/null`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time. Tick steps (`- [ ]` → `- [x]`) only with the evidence each one names. Before writing code, read `AGENTS.md` and, as plain markdown, the Superpowers skills `executing-plans/SKILL.md` and `test-driven-development/SKILL.md` under `~/.claude/plugins/cache/claude-plugins-official/superpowers/*/skills/`. **The implementer never commits**: this worktree's git directory is read-only inside the sandbox. Stop with the tree dirty after each task; the coordinating session verifies and commits. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, stop and report it (F8).

**Revision 1 (2026-09-19).**

**Goal:** Give the conductor what 2c-ii's maintenance needs. That covers the routing of host-call outcomes, the maintenance store, dispatch of the decisions B1 now produces, the admission receipt and the terminal outcomes, the consequences of a drop, and closing the transport on a bound violation. With it, 2c-ii's admission is complete at fixture level.

**Architecture:** Everything is in A2's conductor (`crates/yserver/src/kms/render/admission.rs`) and its hooks in `backend.rs`. B1's decider (`crates/yserver/src/kms/owner/admission/`) is used as implemented; it gains no new state. Production is unchanged (R8): every new path acts only for a device whose conductor is active, meaning installed and with the transport in `Owner`.

**Spec:** `docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md` revision 4 — section 7 (the maintenance store, the receipt, the terminal-outcome table, the collision rule), section 11.1 (rejections per identity, the post-drop state), section 5 (the bound). Read the implemented A1/A2/B1 code before Task 1. The accepted findings `2026-09-18-stage-2c-ii-plan-a2-accepted.md` and `2026-09-19-stage-2c-ii-plan-b1-accepted.md` record what those plans left as limits.

## Design decisions this plan fixes (user-approved, 2026-09-19)

1. **Host-call outcomes reach the conductor.** `record_host_call_events` today logs the owner events that `apply_host_call_event` returns and drops them. So a kernel rejection (`Terminal { FailedBeforeSubmit(IoctlRejected) }`, with its `ResourcesReleased` / `ResourcesStillCurrent`) and a `CompletionUnknown` never reach anyone. That is a latent gap in A2 too: a primary rejected after dispatch never gets its ledger back. For a device whose conductor is active, those events now go through `route_owner_event`. For every other device, nothing changes.
2. **The maintenance payload is opaque bytes, owned by the store.** In `MaintenancePayload { generation, data: Arc<[u8]> }`, the bytes are a cursor image or a LUT, supplied by the source. The conductor's store keeps each `(CRTC, class)`'s desired, submitted and current payload. In 2c-ii maintenance carries **no** `CommitResources`. The cursor buffer is stage 4's, and so is extending 2c-i's consumer to track it. What goes into the commit is the description the source builds.
3. **A maintenance-only commit has an empty ledger, and its retirement leaves the primary's current state alone.** `CommitResourceConsumer::consume` replaces `current_resources` unconditionally on `CompletionRetired`. For a commit the receipt marks as maintenance-only (tier 4 or 7 with no combined primary), the conductor does not forward `CompletionRetired` to `consume`.
4. **The source grows.** `describe` receives the whole `AdmissionDecision`: the primary, the carried maintenance, a bundle's members, and the combined primary. New methods report:
   - maintenance readiness;
   - maintenance/primary compatibility;
   - the homogeneous group;
   - cursor-recovery readiness;
   - the per-commit resources of a bundle's members (as `composed_resources` does for one).
5. **Terminal outcomes go through the receipt** (spec §7 table):
   - `Completed`: submitted becomes current, and `note_completed`.
   - Kernel rejection: submitted goes back to desired, with the collision rule, and `reenter(Rejected)`. If that returns `Dropped`, a cursor raises `request_cursor_recovery` and a gamma records a per-CRTC failure for stage 4.
   - `CompletionUnknown`: the payload is parked dormant and is **not** re-offered in 2c-ii. Re-offering it is C.0 §10 recovery's job.
6. **`bound_violation()` is checked after every `confirm`**; `Some` closes the device's transport, like a lock mismatch.
7. **Cursor recovery, topology and unflip admissions stay `Unsupported`.** The software cursor is stage 4's; topology and unflip dispatch are outside 2c-ii.

## Limits stated

- Fixture-level only; no production caller (R8).
- No real cursor buffer, LUT blob or compatibility predicate: the source stands in (stage 4).
- A `CompletionUnknown` payload stays dormant until C.0 §10 recovery exists.
- One device, as in A2.

## Global Constraints

- Every new path is **inert unless the device's conductor is active** (installed and `Owner`). Production behaviour is byte-for-byte unchanged: no conductor is installed there.
- Every token is consumed exactly once; confirmation is at the send; no retry on refusal (A2's rules).
- Resources travel by value; nothing is bare-dropped. A maintenance payload has exactly one home at a time: desired, submitted or current.
- No side effect inside `debug_assert!`; fail closed, never panic, in non-test code; no invented state; no test-only hook that bypasses the path it is named after.
- Test names start with `c0_adm_conductor_` (conductor) or `c0_adm_maint_` (if a decider-level test is needed).
- In your sandbox the full `--lib` suite's helper, device-lock and socket tests may fail or hang. Report what you see without retrying in a loop; the coordinator runs it outside.
- **Honesty rule (F8).** Unreachable scenario, a seam that does not behave as stated, or a real design choice the plan left open: stop and report.

## Exit criteria

| Criterion | Tests | Mutation that must fail them |
| --- | --- | --- |
| Host-call outcomes reach the conductor for an active device only | `c0_adm_conductor_kernel_rejection_returns_the_primary_ledger`, `c0_adm_conductor_host_call_events_unchanged_without_a_conductor` | Q1: skip the routing; Q2: route for every device |
| The store keeps one home per payload | `c0_adm_conductor_offer_maintenance_fills_the_store_and_the_decider` | Q3: leave the payload in desired after confirm |
| Snapshot reports maintenance readiness, compatibility, group and recovery readiness from the source | `c0_adm_conductor_snapshot_reports_maintenance_inputs` | Q4: report maintenance ready without asking the source |
| Decisions carrying maintenance dispatch, with the source describing the whole decision | `c0_adm_conductor_tier6_carrying_gamma_dispatches`, `c0_adm_conductor_tier3_carrying_cursor_dispatches`, `c0_adm_conductor_maintenance_only_dispatches_with_an_empty_ledger`, `c0_adm_conductor_bundle_dispatches_every_member` | Q5: A2's old `Unsupported` guard left in place; Q6: `describe` given only `admitted` |
| Cursor recovery, topology, unflip stay `Unsupported` | `c0_adm_conductor_cursor_recovery_is_unsupported` | Q7: dispatch a cursor recovery |
| A maintenance-only retirement leaves the primary's current state | `c0_adm_conductor_maintenance_only_retirement_keeps_the_primary_current` | Q8: forward its `CompletionRetired` to `consume` |
| `Completed` promotes exactly the carried generation and closes the receipt | `c0_adm_conductor_completed_promotes_the_carried_generation` | Q9: promote the desired generation instead; Q10: leave the receipt open |
| A kernel rejection re-enters with the original ticket; the collision rule; a second rejection drops, with its consequence | `c0_adm_conductor_rejection_reenters_the_carried_payload`, `c0_adm_conductor_rejection_collision_keeps_the_newer_payload`, `c0_adm_conductor_second_rejection_drops_a_cursor_into_recovery`, `c0_adm_conductor_second_rejection_records_a_gamma_failure` | Q11: re-enter with a new ticket; Q12: keep the older payload on collision; Q13: no recovery barrier on a dropped cursor |
| `CompletionUnknown` parks the payload dormant, uncounted | `c0_adm_conductor_unknown_parks_the_payload` | Q14: re-offer it; Q15: count it as a rejection |
| A bound violation closes the transport | `c0_adm_conductor_bound_violation_closes_the_transport` | Q16: ignore `bound_violation()` |

---

### Task 1: Route host-call outcomes for an active conductor

**Files:** `backend.rs` (`record_host_call_events`, and `route_owner_event` if needed), tests.

**Invariant:** for each `(device, HostCallEvent)` that `record_host_call_events` applies to a device's owner, if that device's conductor is active, every `OwnerEvent` returned by `apply_host_call_event` is passed to `route_owner_event(device, event, now)`, in order. For any other device the events are only logged, as today. `route_owner_event` must already handle the event kinds that arrive: resource events go to `commit_consumer.consume`; `Terminal` is recorded; `Quarantined` and the rest fall to the default arm. Check that each kind is handled correctly rather than assuming it, and report what you found.

**Named tests:**

- `c0_adm_conductor_kernel_rejection_returns_the_primary_ledger` — active conductor. A composed admission dispatched over a non-empty current (A's `Current` resources), then the executor delivers a kernel rejection for that commit (a stub behaviour or a crafted `HostCallEvent` — say which). Afterwards `current_resources` holds A again, the new resources are in `rejected_resources`, and nothing was dropped.
- `c0_adm_conductor_host_call_events_unchanged_without_a_conductor` — the same event on a device with no conductor: `commit_consumer` is untouched, which is today's behaviour.

- [ ] Steps: tests; red state; implement; gate; stop dirty and report.

---

### Task 2: The store, the offers, the snapshot's maintenance inputs, the source extension

**Files:** `admission.rs`, `backend.rs` tests.

**Interfaces:**

```rust
pub(crate) struct MaintenancePayload { pub(crate) generation: u64, pub(crate) data: std::sync::Arc<[u8]> }

// AdmissionSource changes:
//   fn describe(&mut self, decision: &AdmissionDecision) -> CommitDescription;   // was &Admitted
//   fn maintenance_readiness(&self, key: MaintenanceKey, generation: u64) -> Readiness;
//   fn compatible(&self, key: MaintenanceKey, generation: u64, primary: IntentKey) -> bool;
//   fn homogeneous_group(&self) -> BTreeSet<CrtcId>;
//   fn cursor_recovery_ready(&self, crtc: CrtcId) -> bool;

impl KmsBackend {
    /// Store the payload as desired and queue it in the decider
    /// (`set_maintenance(key, generation, behind_commit)`, where `behind_commit`
    /// is "the owner's slot is occupied"). Inert unless active.
    pub(crate) fn admission_offer_maintenance(&mut self, device: DrmDeviceKey,
        key: MaintenanceKey, payload: MaintenancePayload) -> Result<(), AdmissionError>;
}
```

**Invariants:**

1. The store's desired entry and the decider's slot change together, like A2's direct offer. If the decider refuses (`StaleGeneration`), the store is unchanged. If the decider clears the slot (unchanged omission), the store drops that desired payload too.
2. The snapshot reports, for every desired maintenance generation, `IntentKey::Maintenance` readiness from `maintenance_readiness`. It reports a compatibility for every pair of that generation with each primary candidate intent the snapshot reports, from `compatible`. It also reports `homogeneous_group` and `IntentKey::CursorRecovery` for each pending recovery, from `cursor_recovery_ready`.
3. Existing A2 tests keep their meaning; the fixture source implements the new methods with defaults that change none of them (report that).

**Named tests:** `c0_adm_conductor_offer_maintenance_fills_the_store_and_the_decider`, `c0_adm_conductor_snapshot_reports_maintenance_inputs`.

- [ ] Steps as above.

---

### Task 3: Dispatch what B1 decides, the receipt, the bound check

**Files:** `admission.rs`, `backend.rs` tests.

**Invariants:**

1. `decision_requires_unsupported` now refuses only `CursorRecovery`, `Topology` and `Unflip` (the last two as today). Every other decision is dispatched through the shared step, `admission_dispatch_decision`.
2. The request is `source.describe(&decision)`. The ledger is built inside `begin_with_ledger`'s builder, as A2 does:
   - **primary with or without carried maintenance:** `old` = `take_current()`, `new` = the primary's resources (composed: `composed_resources`; direct: the prepared seam; bundle: `composed_resources` for each member, in member order);
   - **maintenance-only:** both sides empty.
3. On a confirmed dispatch the conductor builds the receipt `AdmissionReceipt { commit, carried: Vec<CarriedMaintenance>, maintenance_only: bool }`, keyed by `CommitId` (spec §7, round-3 m-1), and moves each carried payload from desired to submitted in the store. A combined primary is treated as its `admitted` kind for resources.
4. Right after `confirm`, if `admission.bound_violation()` is `Some`, close the device's transport (the lock-mismatch close) and return `TransportClosed`.
5. At most one receipt exists per device (one live commit).

**Named tests:** `c0_adm_conductor_tier6_carrying_gamma_dispatches`, `c0_adm_conductor_tier3_carrying_cursor_dispatches` (a retirement wake), `c0_adm_conductor_maintenance_only_dispatches_with_an_empty_ledger`, `c0_adm_conductor_bundle_dispatches_every_member`, `c0_adm_conductor_cursor_recovery_is_unsupported`, `c0_adm_conductor_bound_violation_closes_the_transport`. That last one reaches a violation through a sequence the decider allows, or through a hook that changes **only** the decider's counters, not the conductor's check. Name which; if you use a hook, say why no legal sequence exists.

A2's and B1's existing tests that asserted `Unsupported` for maintenance-carrying decisions (`c0_adm_conductor_maintenance_carrying_tier6_is_unsupported`, `..._tier3_is_unsupported`) now assert dispatch. Rewrite them under the new names above, and list them.

- [ ] Steps as above.

---

### Task 4: Terminal outcomes through the receipt

**Files:** `backend.rs` (`route_owner_event`), `admission.rs`, tests.

**Invariants** (spec §7 table, §11.1):

1. On a `Terminal` for a commit with a receipt:
   - `Completed` — each carried payload moves submitted → current (the previous current payload is released), then `note_completed(key, generation)`, and the receipt closes.
   - `FailedBeforeSubmit(IoctlRejected)` — each carried payload moves submitted → desired, with the collision rule: if a newer desired payload exists, keep it and release the older one. Then `reenter(key, generation, ticket, Rejected)`. On `Dropped`: remove the store's desired entry for that key; a cursor raises `admission.request_cursor_recovery(crtc)`; a gamma adds `crtc` to the conductor's gamma-failure set. The receipt closes.
   - `CompletionUnknown` — each carried payload moves submitted → a **dormant** set in the store; it is not re-offered. `reenter` is not called, so the rejection count is untouched. The receipt closes.
   - A pre-IPC `NeverDispatched` never reaches this point: the token was aborted and no receipt exists.
2. For a maintenance-only commit's `CompletionRetired`, do **not** call `commit_consumer.consume`. Everything else about the retirement hook (A2 Task 4) is unchanged.
3. A `Terminal` for a commit with no receipt changes nothing in the store.

**Named tests:** `c0_adm_conductor_completed_promotes_the_carried_generation`, `c0_adm_conductor_maintenance_only_retirement_keeps_the_primary_current`, `c0_adm_conductor_rejection_reenters_the_carried_payload`, `c0_adm_conductor_rejection_collision_keeps_the_newer_payload`, `c0_adm_conductor_second_rejection_drops_a_cursor_into_recovery` (then a wake: `Unsupported(Tier::Unflip)` for the recovery), `c0_adm_conductor_second_rejection_records_a_gamma_failure`, `c0_adm_conductor_unknown_parks_the_payload`. The rejection and unknown outcomes arrive through Task 1's routing, from a stub or crafted host-call event, not by calling the handler directly.

- [ ] Steps as above; Task 4 also runs `cargo check --workspace --target` for the three targets.

---

## Gate — the one authoritative list

Every task:

```bash
cargo build -p yserver --bin yserver
cargo build --release -p yserver --bin yserver
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features tcp-transport -- -D warnings
cargo clippy --all-targets --features xdmcp -- -D warnings
for i in 1 2 3 4 5; do cargo test -p yserver --lib c0_adm; done
cargo test --release -p yserver --lib c0_adm
cargo test -p yserver --lib c0_2ci
cargo test -p yserver --lib
```

Task 4 adds `cargo check --workspace --target <t>` for `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-unknown-freebsd`. The hardware gate (spec §10.4) is the coordinator's, after Task 4, with the user's go-ahead.

## What the coordinator does after each task

1. Reads the diff against the invariants; checks each named test sets up its scenario and goes through the path it is named after.
2. Reruns the gate outside the sandbox.
3. After Task 4: applies Q1–Q16 to your code; a survivor goes back as a finding.
4. Commits each task with `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)` and `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
