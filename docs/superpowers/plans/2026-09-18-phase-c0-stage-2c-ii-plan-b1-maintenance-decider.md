# Stage 2c-ii, plan B1 — maintenance in the decider

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), `--sandbox workspace-write`, run with `< /dev/null`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time. Tick steps (`- [ ]` → `- [x]`) only with the evidence each one names. Before writing code, read `AGENTS.md` and, as plain markdown, the Superpowers skills `executing-plans/SKILL.md` and `test-driven-development/SKILL.md` under `~/.claude/plugins/cache/claude-plugins-official/superpowers/*/skills/`. **The implementer never commits**: this worktree's git directory is read-only inside the sandbox. Stop with the tree dirty after each task; the coordinating session verifies and commits.

**Revision 2 (2026-09-18)** — incorporates codex round 1 (`../findings/2026-09-18-stage-2c-ii-plan-b1-review-round1.md`: 2 blocking, 2 major, 1 minor, all verified and accepted).
- **B-1:** A2 aborts every decision that carries maintenance, a tier-6 primary included.
- **B-2:** the bound's allowance grows when an older-ticket identity ages later, instead of freezing when the identity first ages.
- **M-1:** negative scenarios for tier 3 on an ordinary wake, an incompatible symmetric primary, the extra compatible maintenance, and compatible-but-waiting maintenance.
- **M-2:** the counters are proven through `confirm`, and the rejection count survives a drop.
- **m-1:** there is one authoritative gate.

Mutations P19–P25 were added.

**Goal:** Extend A1's pure decider with maintenance: cursor and gamma intents with device-monotonic tickets, ageing, the per-identity rejection count, tiers 3, 4, 5 and 7, the software-cursor recovery barrier, absorption, the homogeneous bundle, and the starvation bound as a checked invariant.

**Architecture:** Everything lands in A1's module, `crates/yserver/src/kms/owner/admission/`. The decider stays pure: it holds descriptors and counters, never payloads. The payloads, the receipt and the terminal-outcome routing are plan B2's, in the conductor. B1 touches A2's conductor only as far as it must to keep compiling: each new admission kind is aborted there as `Unsupported`, and B2 implements it.

**Tech Stack:** Rust, `std::collections`, `thiserror`.

**Spec:** `docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md`, **revision 4**: sections 3 (maintenance, tickets), 4 (readiness, compatibility), 5 (tiers, absorption, ageing, bounds), 11.1 (rejections, per identity) and 10.2 (exit criteria). Its governing parent for these rules is C.0 §9.2.1, including the amendment marked 2026-09-18 (`docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`, around lines 1470–1615). Read A1's implemented module before Task 1.

## Design decisions this plan fixes (user-approved, 2026-09-18)

1. **B is split into B1 (the decider, this plan) and B2 (the conductor).** B1 needs no helper process.
2. **An admission carries a primary and maintenance together.** `AdmissionDecision` keeps `tier` and `admitted` (the tier's winner, as today) and gains three fields: the maintenance generations the commit carries, a primary combined into a maintenance admission (symmetric absorption), and the identities that age if the decision is confirmed. `Admitted` gains variants for a maintenance admission, a bundle and a cursor recovery.
3. **Compatibility and group membership are snapshot inputs.** Whether a maintenance generation can ride in a given primary, and which CRTCs form the qualified `HomogeneousCompletionGroup`, are reported by the snapshot. The decider does not compute them (spec §4). Stages 3 and 4 supply the real values.
4. **A software-cursor recovery barrier** joins tier 2. B1 stores and admits it; B2 cannot dispatch it (the software cursor is stage 4's) and aborts it as `Unsupported`, as A2 does for the unflip.
5. **The bound is checked.** The decider counts, per aged identity `X`, the older-ticket maintenance admissions `X` waited through. Barrier admissions are counted apart and do not count. `X`'s **allowance** is `2 ×` the number of **distinct identities with a ticket older than `X`'s that have been aged at any moment since `X` aged**. It is not frozen when `X` first ages (round-1 B-2): an older identity that ages later still adds its two admissions. The allowance only grows, and it is discarded when `X` is carried or dropped. `bound_violation()` reports an `X` whose count exceeds its allowance. B2 closes the transport on a violation.

## Semantics this plan pins down (read before Task 1)

These follow from C.0 §9.2.1 and spec revision 4; where C.0 left a choice, the choice is stated.

- **Identity and ticket.** A maintenance identity is a `(CRTC, class)`, where the class is `Cursor` or `Gamma`. Its desired slot holds one generation and one `AdmissionTicket`. Tickets are device-monotonic and unique, and are a **separate counter from `PrimaryOrdinal`**. A ticket is assigned when the slot goes from empty to occupied. It **survives latest-wins replacement** while the identity is not submitted. An update that arrives while the identity's previous generation is submitted gets a **new** ticket (C.0 §9.2.1).
- **Unchanged omission.** The decider records each identity's current generation (`note_completed`). Offering a generation equal to the current one clears the desired slot instead of queuing it: an unchanged cursor is never carried (C.0 §9.2.1).
- **Ageing.** An identity is aged, keeping its ticket, when:
  - it arrives while the device has a commit in flight (the caller says so);
  - it is ready, unsent and not carried when a different admission is confirmed (it "lost" that admission — this is the decision's `ages` set);
  - a barrier (topology, unflip, cursor recovery) is confirmed while it is ready and unsent.
- **Tier 3 is the retirement successor** ("the fairness-qualified version of submit-after-retirement", C.0 §9.2.1). It applies **only on a snapshot with `retirement_wake`**. It admits the queued direct successor if it is ready and eligible, the round-robin permits every CRTC it covers, and **every aged identity** is compatible with it and lies on its CRTCs, so the successor absorbs them all. Otherwise tier 3 does not apply. On ordinary wakes the direct successor competes in tier 6 as in A1.
- **Absorption into primaries (tiers 3, 5 and 6).** A primary admission carries every **ready, compatible** maintenance generation on the CRTCs it covers, so readiness implies changed. For tier 3, the aged ones are mandatory. Each carried generation's ticket is consumed.
- **Tier 4:** the aged, ready identity with the oldest ticket. **Tier 7:** the non-aged, ready identity with the oldest ticket. Tickets are unique, so the `(CRTC, class)` tie-break C.0 names never has to decide. It is still the order used when two identities are compared on equal tickets, which cannot happen.
- **Symmetric absorption (tiers 4 and 7).** The winner combines the **oldest** ready primary on its CRTC that is compatible with it. It never combines a primary on a CRTC with a pending barrier, and never one the round-robin would refuse. The winner's own generation is carried. So is any other ready, compatible maintenance on that CRTC.
- **Tier 5:** at least two CRTCs of the snapshot's homogeneous group have a ready composed primary. The bundle takes the oldest ready one for **every** ready group CRTC, the round-robin permits every CRTC in it, and no barrier is pending. Aged maintenance cannot be pending at tier 5, because tier 4 would have won first. The bundle also absorbs ready, compatible maintenance on its CRTCs. A direct successor is never part of a bundle.
- **Tier 2** now holds two barriers: the unflip (as in A1) and a cursor recovery per CRTC. A pending unflip goes before a cursor recovery. A cursor recovery is ready when the snapshot reports it ready, and it is never superseded.
- **Rejection re-entry (spec §11.1, revised).** `reenter(key, generation, ticket, kind)`:
  - `Rejected` bumps the identity's count. At 2, it drops the pending generation — the desired slot is emptied, even if it holds a newer generation that collided — and reports `Dropped`, so B2 can raise the cursor barrier or record the gamma failure.
  - Below 2, the generation re-enters aged with the original ticket.
  - If the slot already holds a newer generation, the slot keeps the newer generation with the older of the two tickets, aged, and the count is the identity's.
  - `Unknown` re-enters the same way without touching the count.
  - `note_completed` resets the count to 0.

## Global Constraints

- The decider stays pure: `decide` takes `&self`; nothing in B1 owns a payload or performs I/O.
- **No semantic change to A1 or A2 behaviour except tier 3**, which by C.0 moves a qualifying retirement-wake direct admission from `Tier::Primary` to the new tier 3. Every A1/A2 test assertion that changes because of that must be listed in your report with the reason. Everything else — adding the new decision fields to existing literals, new match arms — is mechanical and changes no expectation.
- A2's conductor must keep compiling and passing, and must never dispatch a decision it cannot carry in full. **Before** matching on `decision.admitted`, it aborts the token and returns `Unsupported(tier)`, with no dispatch and no state change, for any decision that has a non-empty `carried`, has a `combined_primary`, is one of the new `Admitted` variants, or has tier 3, 4, 5 or 7. This includes a tier-6 composed or direct decision that carries maintenance (round-1 B-1): dispatching its primary alone would confirm the whole decision, spend the maintenance tickets and lose the payload, which has no home until B2. Implementing those admissions is B2's job.
- No side effect inside `debug_assert!` (the gate runs `c0_adm` in release).
- Test names start with `c0_adm_` (for the decider: `c0_adm_maint_`).
- `cargo test -p yserver --lib` needs `target/debug/yserver` built, and the release run needs a fresh `target/release/yserver`. In your sandbox the full `--lib` suite's helper, device-lock and socket tests may fail or hang. Report what you observe and do not retry in a loop; the coordinator runs it outside the sandbox.
- **Honesty rule (F8).** If a scenario is unreachable through these interfaces, or an interface cannot carry an invariant, stop and report it.

## Exit criteria covered by this plan

| Spec criterion (B1's part) | Tests | Mutation that must fail them |
| --- | --- | --- |
| A ticket survives payload replacement; a new ticket after submission | `c0_adm_maint_ticket_survives_replacement`, `c0_adm_maint_update_while_submitted_gets_a_new_ticket` | P1: reset the ticket on replacement; P2: reuse the consumed ticket |
| Unchanged cursor omitted | `c0_adm_maint_unchanged_generation_is_never_carried` | P3: queue a generation equal to current |
| Ageing: on arrival behind a commit, on losing an admission, on a barrier | `c0_adm_maint_ages_on_arrival_behind_a_commit`, `c0_adm_maint_ages_after_losing_an_admission`, `c0_adm_maint_barrier_ages_overtaken_maintenance_without_resetting_tickets` | P4: skip ageing on loss; P5: reset relative age at a barrier |
| Tiers 1–7 in order, with the maintenance tiers | `c0_adm_maint_seven_tiers_in_order` | P6: swap tiers 4 and 5 |
| Tier 3 only on a retirement wake and only if it absorbs every aged identity | `c0_adm_maint_tier3_absorbs_every_aged_identity`, `c0_adm_maint_tier3_yields_to_unabsorbable_aged_maintenance`, `c0_adm_maint_tier3_never_applies_on_an_ordinary_wake` | P7: tier 3 ignores aged identities it cannot absorb; P19: tier 3 also on ordinary wakes |
| A2 never dispatches a decision carrying maintenance (round-1 B-1) | `c0_adm_conductor_maintenance_carrying_tier6_is_unsupported` | P20: A2 dispatches a tier-6 decision whose `carried` is non-empty |
| Symmetric absorption in tiers 4 and 7, never across a barrier or against the round-robin | `c0_adm_maint_symmetric_absorption_combines_the_oldest_compatible_primary`, `c0_adm_maint_symmetric_absorption_respects_barriers_and_round_robin`, `c0_adm_maint_symmetric_absorption_skips_an_incompatible_primary` | P8: combine an incompatible primary; P9: combine across a barrier; P21: carry only the winner, not the other ready compatible maintenance on that CRTC |
| Absorption into primaries carries only ready, compatible generations and consumes their tickets | `c0_adm_maint_primary_absorbs_compatible_maintenance`, `c0_adm_maint_incompatible_maintenance_is_not_absorbed`, `c0_adm_maint_waiting_maintenance_is_not_absorbed` | P10: absorb an incompatible generation; P22: absorb a compatible but `Waiting` generation |
| Tier 5: every ready group CRTC, the round-robin, no timer | `c0_adm_maint_bundle_takes_every_ready_group_crtc`, `c0_adm_maint_bundle_obeys_the_round_robin`, `c0_adm_maint_one_ready_group_crtc_is_tier6` | P11: drop one ready member; P12: skip the round-robin in tier 5 |
| Rejection re-entry per identity: original ticket, aged; second consecutive rejection drops; collision inherits the count; unknown not counted; completed resets; the count survives a drop | `c0_adm_maint_rejected_generation_reenters_aged_with_its_ticket`, `c0_adm_maint_second_consecutive_rejection_drops`, `c0_adm_maint_collision_keeps_older_ticket_and_inherits_the_count`, `c0_adm_maint_unknown_is_not_a_rejection`, `c0_adm_maint_completed_resets_the_count`, `c0_adm_maint_a_generation_after_a_drop_drops_on_its_first_rejection` | P13: count per generation (reset on collision); P14: count an unknown; P23: reset the count on drop |
| Cursor recovery barrier in tier 2, not superseded | `c0_adm_maint_cursor_recovery_is_a_tier2_barrier` | P15: let a primary on that CRTC overtake it |
| The bound: `1 + 2(N − 1)`, checked, barriers apart; continuous collision; the allowance grows with late-aged older identities; counting happens in `confirm` | `c0_adm_maint_two_identities_progress_under_continuous_collision`, `c0_adm_maint_bound_violation_is_reported`, `c0_adm_maint_barriers_do_not_count_against_the_bound`, `c0_adm_maint_allowance_grows_when_an_older_identity_ages_later`, `c0_adm_maint_confirm_counts_and_carry_clears_the_counter` | P16: count barrier admissions toward the bound; P17: never report a violation; P24: freeze the allowance at ageing; P25: skip the increment in `confirm` |
| Fairness under a continuous direct-successor stream, with maintenance | `c0_adm_maint_direct_stream_cannot_starve_maintenance` | P18: tier 3 ignores aged identities (same as P7, on the stream) |

---

### Task 1: Maintenance storage, tickets, rejection re-entry and the cursor barrier

**Files:** `admission/intents.rs` (or a new `admission/maintenance.rs` — your call, report it), `admission/mod.rs`, `admission/tests.rs`.

**Interfaces:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MaintenanceClass { Cursor, Gamma }

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaintenanceKey { pub crtc: CrtcId, pub class: MaintenanceClass }

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AdmissionTicket(/* private u64 */);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceIntent { pub generation: u64, pub ticket: AdmissionTicket, pub aged: bool }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReentryKind { Rejected, Unknown }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reentry { Reentered, Dropped }

impl Admission {
    /// Latest-wins desired generation. `behind_commit`: the device has a commit
    /// in flight right now (the conductor knows). Offering the current
    /// generation clears the slot instead of queuing it.
    pub fn set_maintenance(&mut self, key: MaintenanceKey, generation: u64, behind_commit: bool)
        -> Result<(), AdmissionError>;
    /// After a terminal outcome of a commit that carried `generation` with `ticket`.
    pub fn reenter(&mut self, key: MaintenanceKey, generation: u64, ticket: AdmissionTicket,
        kind: ReentryKind) -> Reentry;
    /// The commit carrying `generation` completed: it becomes current and the
    /// identity's rejection count resets.
    pub fn note_completed(&mut self, key: MaintenanceKey, generation: u64);
    pub fn request_cursor_recovery(&mut self, crtc: CrtcId);
    pub fn maintenance(&self, key: MaintenanceKey) -> Option<MaintenanceIntent>;
    pub fn rejection_count(&self, key: MaintenanceKey) -> u32;
    pub fn cursor_recovery(&self) -> &BTreeSet<CrtcId>;
}
```

A stale `generation` offered to `set_maintenance` (not newer than the queued one) is `StaleGeneration`, as for composed. To know that an update arrived "while submitted" (the new-ticket rule), the decider has to remember which identities have a generation in flight. That state is recorded when a decision carrying them is **confirmed**, which is Task 2's `confirm`, and cleared by `reenter` and `note_completed`. Keep it internal.

**Invariants:** the semantics section's identity/ticket, unchanged-omission and rejection-re-entry bullets, exactly.

**Named tests:**

- `c0_adm_maint_ticket_survives_replacement` — set a cursor generation, then a newer one before any admission: same ticket.
- `c0_adm_maint_update_while_submitted_gets_a_new_ticket` — Task 2 provides the confirm path. Here, simulate "submitted" through whatever internal hook Task 1 needs, and **name it in your report**. Better: write this test in Task 2 and state that here. Your call, but it must exist by the end of Task 2.
- `c0_adm_maint_unchanged_generation_is_never_carried` — `note_completed(key, 5)`, then `set_maintenance(key, 5, false)`: the slot is empty.
- `c0_adm_maint_ages_on_arrival_behind_a_commit` — `set_maintenance(key, g, true)`: the intent is aged.
- `c0_adm_maint_rejected_generation_reenters_aged_with_its_ticket` — `reenter(key, g, t, Rejected)` on an empty slot: the intent is `g`, ticket `t`, aged; count 1.
- `c0_adm_maint_second_consecutive_rejection_drops` — two rejections with no `note_completed` between them: the second returns `Dropped` and the slot is empty.
- `c0_adm_maint_collision_keeps_older_ticket_and_inherits_the_count` — a newer generation queued with ticket `t2`, then `reenter(key, older_g, t1 < t2, Rejected)`: the slot holds the newer generation with `t1`, aged. A second `Rejected` re-entry for another generation of the same identity returns `Dropped`.
- `c0_adm_maint_unknown_is_not_a_rejection` — `Unknown` re-entries do not move the count, however many there are.
- `c0_adm_maint_completed_resets_the_count` — rejected once, `note_completed`, rejected again: `Reentered`, not `Dropped`.
- `c0_adm_maint_cursor_recovery_is_stored_per_crtc` — two requests for the same CRTC keep one entry.
- `c0_adm_maint_a_generation_after_a_drop_drops_on_its_first_rejection` — two rejections drop the identity; a new generation is offered (new ticket, at the back); its first rejection returns `Dropped`.

- [ ] Step 1: tests. Step 2: record the red state. Step 3: implement. Step 4: gate. Step 5: stop dirty and report.

---

### Task 2: The decision shape, the snapshot inputs, tier 2's cursor barrier, tiers 4 and 7, symmetric absorption, ageing on confirm

**Files:** `admission/decide.rs`, `admission/snapshot.rs`, `admission/token.rs`, `admission/mod.rs`, `admission/tests.rs`, and `crates/yserver/src/kms/render/admission.rs` (A2's conductor: the `Unsupported` arms only).

**Interfaces:**

```rust
// Tier gains: DirectSuccessor = 3, AgedMaintenance = 4, Bundle = 5, Maintenance = 7.
// Admitted gains:
    Maintenance { key: MaintenanceKey, generation: u64 },
    Bundle { members: Vec<Admitted> },          // composed members only (Task 3)
    CursorRecovery { crtc: CrtcId },

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedMaintenance { pub key: MaintenanceKey, pub generation: u64, pub ticket: AdmissionTicket }

pub struct AdmissionDecision {
    pub tier: Tier,
    pub admitted: Admitted,
    pub carried: Vec<CarriedMaintenance>,      // every maintenance generation the commit carries
    pub combined_primary: Option<Admitted>,    // symmetric absorption (tiers 4, 7)
    pub ages: BTreeSet<MaintenanceKey>,        // ready, unsent, not carried: age on confirm
}

// Snapshot gains:
//   IntentKey::Maintenance { key: MaintenanceKey, generation: u64 }  and  IntentKey::CursorRecovery { crtc }
//   pub fn report_compatible(&mut self, maintenance: IntentKey, primary: IntentKey);
//   pub fn is_compatible(&self, maintenance: IntentKey, primary: IntentKey) -> bool; // unreported ⇒ false
//   pub homogeneous_group: BTreeSet<CrtcId>,                                         // Task 3 uses it
```

`primary_crtcs()` covers the new variants: a maintenance admission covers its combined primary's CRTCs, if any; a bundle covers its members'; a cursor recovery covers its CRTC. Existing `AdmissionDecision { tier, admitted }` literals in A1's tests gain the new fields empty — a mechanical change.

**Invariants:**

1. Tier 2 holds the unflip first, then the cursor recovery for the lowest CRTC whose `IntentKey::CursorRecovery` is ready. A composed intent on a CRTC with a pending cursor recovery does not compete, as with the unflip barrier.
2. Tier 4 and tier 7, with symmetric absorption, as the semantics section states.
3. **Confirm** consumes the admitted intent (as today) **and** every carried maintenance generation: its ticket is spent, its desired slot is emptied if the carried generation is still the desired one, and the identity is marked submitted with that generation. Confirm then ages every identity in `ages`. A barrier admission (topology, unflip, cursor recovery) ages every ready, unsent maintenance identity. Nothing resets a ticket.
4. `lock` keeps comparing the whole decision, the new fields included.
5. A2's conductor aborts the new kinds as `Unsupported` (Global Constraints); its existing tests keep passing unchanged.

**Named tests:** `c0_adm_maint_update_while_submitted_gets_a_new_ticket` (if not in Task 1), `c0_adm_maint_ages_after_losing_an_admission`, `c0_adm_maint_barrier_ages_overtaken_maintenance_without_resetting_tickets`, `c0_adm_maint_symmetric_absorption_combines_the_oldest_compatible_primary` (a cursor wins tier 7 and two composed generations on its CRTC are ready and compatible, queued at different times: the older one is combined, and the carried set holds the cursor), `c0_adm_maint_symmetric_absorption_respects_barriers_and_round_robin` (a compatible primary on a CRTC with a pending unflip is not combined; nor is one whose CRTC was just served while another is owed), `c0_adm_maint_cursor_recovery_is_a_tier2_barrier` (a ready cursor recovery wins over an older ready composed on another CRTC; a composed on its own CRTC does not compete), and a tier-4-over-tier-7 test of your naming. Also: `c0_adm_maint_symmetric_absorption_skips_an_incompatible_primary` — the older composed on the winner's CRTC is incompatible and a younger one is compatible, so the younger is combined; with none compatible, none is. The combines test gets a second case, where another ready, compatible maintenance identity on that CRTC rides along in `carried`. And `c0_adm_conductor_maintenance_carrying_tier6_is_unsupported` — in A2, a tier-6 composed decision that carries a gamma: the conductor returns `Unsupported`, the source's `describe` and `composed_resources` were never called, the owner's slot is free, and the decider is unlocked with the gamma still queued.

- [ ] Steps as above.

---

### Task 3: Tier 3, absorption into primaries, tier 5

**Files:** `admission/decide.rs`, `admission/tests.rs`, plus any A1/A2 test assertion that tier 3 changes (list each in your report).

**Invariants:** the semantics section's tier-3, absorption-into-primaries and tier-5 bullets. Tier 5 sits between tier 4 and tier 6; with fewer than two ready group CRTCs it does not apply and nothing waits.

**Named tests:** `c0_adm_maint_tier3_absorbs_every_aged_identity` (retirement wake, a ready successor over {1, 2}, aged cursors on 1 and 2 both compatible: tier 3, both carried, both tickets spent), `c0_adm_maint_tier3_yields_to_unabsorbable_aged_maintenance` (the same with the cursor on 2 incompatible: tier 4 wins), `c0_adm_maint_primary_absorbs_compatible_maintenance` (a tier-6 composed carries a ready, compatible gamma on its CRTC and spends its ticket), `c0_adm_maint_incompatible_maintenance_is_not_absorbed`, `c0_adm_maint_bundle_takes_every_ready_group_crtc` (group {1, 2, 3}, all three ready: one bundle with three members), `c0_adm_maint_bundle_obeys_the_round_robin` (CRTC 1 just served, 1 and 2 ready in the group: tier 6 serves 2 first), `c0_adm_maint_one_ready_group_crtc_is_tier6`, and `c0_adm_maint_seven_tiers_in_order` (one fixture per adjacent pair of tiers, each showing the higher one wins). Also: `c0_adm_maint_waiting_maintenance_is_not_absorbed` — compatible but reported `Waiting`: not carried, ticket unspent. And `c0_adm_maint_tier3_never_applies_on_an_ordinary_wake` — the tier-3 fixture without `retirement_wake`: the decision is tier 6 by age, or tier 4, never tier 3.

- [ ] Steps as above.

---

### Task 4: The checked bound and the starvation scenarios

**Files:** `admission/token.rs` (counting on confirm), `admission/decide.rs` or a new file, `admission/tests.rs`.

**Interface:**

```rust
impl Admission {
    /// An aged identity that has waited through more older-ticket maintenance
    /// admissions than `2 × (aged identities with older tickets when it aged)`.
    pub fn bound_violation(&self) -> Option<MaintenanceKey>;
}
```

**Invariants:** the design decision 5 definition. A confirmed admission that carries a ticket older than an aged identity's counts one toward that identity. Barrier admissions do not count. An identity's count is removed when it is carried or dropped.

**Evidence through `confirm`** (round-1 M-2): the counters must be driven by real `lock` / `confirm` sequences. A test-only hook may be used only where no legal sequence reaches the state, and the report must say where.

**Named tests:** `c0_adm_maint_allowance_grows_when_an_older_identity_ages_later` (round-1 B-2's sequence: A has the older ticket but is `Waiting` and not aged; B ages behind a commit; A becomes ready and a barrier ages it; A is admitted before B: no violation), `c0_adm_maint_confirm_counts_and_carry_clears_the_counter` (through real confirms: an older-ticket admission increments the younger aged identity's count; carrying that identity clears it), `c0_adm_maint_two_identities_progress_under_continuous_collision` (spec §11.1: cursor A and gamma B, both aged and both absorbable only alone. A is rejected on every admission, and before each re-entry a newer A generation arrives. B is admitted within `1 + 2(N − 1)` admissions, A is dropped at its second consecutive rejection, and `bound_violation()` stays `None` throughout), `c0_adm_maint_bound_violation_is_reported` (drive the counter past the bound through a test-only hook, or by a sequence the invariants allow if one exists — name which), `c0_adm_maint_barriers_do_not_count_against_the_bound`, `c0_adm_maint_direct_stream_cannot_starve_maintenance` (a direct successor re-queued on every retirement wake and an aged cursor on another CRTC that the successor cannot absorb: the cursor is admitted within the bound).

- [ ] Steps as above, then the gate below.

---

## Gate — the one authoritative list (round-1 m-1)

Every task:

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features tcp-transport -- -D warnings
cargo clippy --all-targets --features xdmcp -- -D warnings
for i in 1 2 3; do cargo test -p yserver --lib c0_adm; done
cargo build --release -p yserver --bin yserver
cargo test --release -p yserver --lib c0_adm
cargo test -p yserver --lib c0_2ci
cargo test -p yserver --lib
```

Task 4 also runs `cargo check --workspace --target <t>` for `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl` and `x86_64-unknown-freebsd`. The hardware gate (spec §10.4) is the coordinator's, after Task 4, and only with the user's go-ahead for GPU use.

## What the coordinator does after each task

1. Reads the diff against the task's interfaces, invariants and the semantics section, and checks every named test sets up its stated scenario and every tier-3 assertion change is justified.
2. Reruns the gate outside the sandbox.
3. After Task 4: applies P1–P25 to your code and records which tests fail; a survivor goes back as a finding.
4. Commits each task with `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)` and `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
