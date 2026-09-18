# Stage 2c-ii, plan A1 — the pure admission decider

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), `--sandbox workspace-write`, run with `< /dev/null`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time. Tick steps (`- [ ]` → `- [x]`) only with the evidence each one names. Before writing code, read `AGENTS.md` and, as plain markdown, the Superpowers skills `executing-plans/SKILL.md` and `test-driven-development/SKILL.md` under `~/.claude/plugins/cache/claude-plugins-official/superpowers/*/skills/`. **The implementer never commits**: this worktree's git directory is read-only inside the sandbox. Stop with the tree dirty after each task; the coordinating session verifies and commits.

**Revision 2 (2026-09-18)** — revision 1 carried the whole implementation as blocks to copy, written by the coordinating session. The user's decision: the implementation is codex's. This revision keeps the design, the interfaces, the invariants, the test scenarios and the mutations, and removes every implementation block. Revision 1's codex review was stopped before it finished and produced no findings.

**Revision 3 (2026-09-18)** — incorporates codex round 1 (`../findings/2026-09-18-stage-2c-ii-plan-a1-review-round1.md`: 1 blocking, 3 major, 1 minor, all verified and accepted). B-1: the round-robin exempted an owed CRTC *inside* a grouped candidate; the rule is restated in Task 4, with an "owed" definition that also cannot deadlock. M-1: confirmation of direct, unflip and topology admissions is now tested. M-2: foreign tokens are tested on `abort` too. M-3: unflip widening is tested. m-1: the barrier test fixes its queue order. Five mutations added (M13–M17), and the per-task counts moved to 9/16/25/36.

**Goal:** A pure, deterministic decider for the primary side of 2c-ii admission — bounded intent storage, `PrimaryOrdinal`, the readiness snapshot, tiers 1, 2 and 6, the per-CRTC round-robin, and the `lock`/`confirm`/`abort` token — with no resource, no I/O and no caller yet.

**Architecture:** A new module `crates/yserver/src/kms/owner/admission/`: storage and bounds, the snapshot the conductor reports, the tiers and round-robin, and two-phase confirmation. `Admission` is one per DRM device. It never touches the owner, the executor or 2c-i's resources: plan A2 builds the conductor that does, and plan B adds maintenance (tickets, tiers 3, 4, 5 and 7, the receipt).

**Tech Stack:** Rust, `std::collections::{BTreeMap, BTreeSet}`, `thiserror` (already a dependency of `yserver`).

**Spec:** `docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md`, revision 3. Sections 3 (units, intents, bounds, `PrimaryOrdinal`), 4 (readiness), 5 (tiers 1, 2, 6 and the round-robin), 6 (the token) and 10.2 (exit criteria) govern this plan; read them before Task 1. **The split into A1 and A2** was the user's decision on 2026-09-18, by the plan-size rule: A1 is the decider, which needs no helper process; A2 is the conductor over `KmsBackend`.

## What was checked before handing this over

For revision 1, the coordinating session confirmed in a scratch worktree, since removed, that this design **can** be built in this crate as specified: the interfaces below compile together, the four tasks can each pass the full gate on their own in this order, and M1–M12 are each caught by a test with the scenario given. That is evidence that the plan is consistent, not an implementation to reproduce: nothing from it is in this plan or in the tree. **Revision 3's changes were not checked that way**: the restated round-robin rule, the six new tests and M13–M17 come from the review and are argued in the text. The counts below are what **your** tests should reach, not a target to pad towards.

## Global Constraints

- The decider owns no resource and performs no I/O; `decide` takes `&self` and mutates nothing (spec §2).
- An intent the snapshot does not report is **not ready**: the decider fails closed (spec §4).
- A report is keyed by the descriptor's **exact generation**: a report about another generation of the same slot does not make the queued one ready.
- Fairness is accounted **per CRTC**; a multi-CRTC admission serves **every** CRTC it covers (spec §5, round-2 B-1).
- The confirmation boundary is the owner's **send**, not `begin`: `lock` consumes nothing, `abort` restores exactly, only `confirm` consumes (spec §6).
- Test names start with `c0_adm_`. **Not** `c0_2cii_`: `cargo test c0_2ci` is a substring filter and would pull them into the 2c-i counts.
- `cargo test -p yserver --lib` needs the `yserver` binary built: the executor tests spawn it as their helper. If helper tests fail with `HelperExited` or a device-lock timeout, run `cargo build -p yserver --bin yserver` and rerun before reporting anything.
- No production caller (R8). Nothing outside `kms/owner/admission/` changes except registering the module in `kms/owner/mod.rs`, in the same `#[doc(hidden)] pub mod` style as its siblings.
- **Honesty rule (F8).** If a test scenario below turns out unreachable through these interfaces, or an interface cannot carry an invariant, **stop and report it**. A silently substituted test is the defect, not the deviation.

## Exit criteria covered by this plan

Each row: the invariant, the test(s) that prove it, and the mutation that **must** make at least one of them fail. After Task 4 the coordinator applies each mutation to your code and runs the tests; a surviving mutation sends the task back.

| Spec 10.2 criterion (A1's part) | Tests | Mutation that must fail them |
| --- | --- | --- |
| Tiers 1, 2 and 6 in order | `c0_adm_tiers_topology_then_unflip_then_primary` | M1: an unflip overtakes a waiting topology barrier |
| Supersession bounds: one slot per category | `c0_adm_second_direct_successor_displaces_the_first_and_keeps_the_ordinal`, `c0_adm_unflip_displaces_the_successor_and_refuses_later_direct_work` | M12: setting the unflip barrier leaves the queued successor in place |
| The unflip barrier is never replaced, only widened | `c0_adm_a_second_unflip_request_widens_the_barrier` | M17: a second `request_unflip` overwrites the barrier's CRTC set |
| No dispatch before readiness | `c0_adm_waiting_or_unreported_intent_is_never_admitted` | M2: any report, `Waiting` included, counts as ready |
| `PrimaryOrdinal` orders across shapes; survives replacement and `Waiting` | `c0_adm_oldest_ready_primary_wins_across_shapes`, `c0_adm_ordinal_survives_replacement_and_waiting`, `c0_adm_composed_newest_wins_and_keeps_its_ordinal` | M5: replacing a composed generation gives the slot a new ordinal |
| A direct successor queued under another layout/topology generation is never admitted | `c0_adm_direct_with_a_stale_layout_or_topology_generation_is_not_admitted`, `c0_adm_lock_rejects_a_direct_successor_whose_layout_changed` | M3: the layout/topology comparison is dropped |
| A later composed intent does not overtake a pending unflip barrier on its CRTC | `c0_adm_composed_on_an_unflip_crtc_does_not_overtake_the_barrier` | M4: composed on a barrier CRTC competes in tier 6 |
| Two-phase: `lock` and `abort` consume nothing; `confirm` consumes exactly the admitted intent, of every kind | `c0_adm_abort_leaves_the_decider_exactly_as_before`, `c0_adm_confirm_consumes_the_admitted_intent_only`, `c0_adm_confirm_consumes_a_direct_unflip_or_topology_admission_exactly` | M6: `lock` advances the admission sequence; M11: `abort` removes the admitted intent; M14: `confirm` of a direct admission leaves the successor queued; M15: `confirm` of an unflip leaves the barrier pending |
| A token only acts on the decider that issued it | `c0_adm_a_foreign_token_is_refused`, `c0_adm_a_foreign_token_cannot_abort` | M16: `abort` skips the token-identity check |
| A generation mismatch is caught at `lock` | `c0_adm_lock_detects_a_generation_that_changed_since_decide` | M7: `lock` accepts a decision without re-deciding |
| Round-robin per CRTC, grouped→composed and composed→grouped, including an owed CRTC *inside* the grouped set | `c0_adm_grouped_then_composed_on_its_crtcs_yields_to_the_owed_crtc`, `c0_adm_composed_then_grouped_yields_to_the_owed_crtc`, `c0_adm_a_grouped_candidate_yields_to_an_owed_crtc_it_contains`, `c0_adm_a_multi_crtc_unflip_serves_every_crtc_it_covers` | M8: the round-robin always permits; M9: `confirm` marks only one CRTC of a multi-CRTC admission as served; M13: a candidate is refused only for owed CRTCs **outside** it (revision 2's rule) |
| The round-robin cannot deadlock | `c0_adm_when_every_ready_crtc_was_just_served_the_oldest_wins`, `c0_adm_a_lone_grouped_candidate_is_not_held_back_by_itself` | — (a deadlock shows as `decide` returning `None` with ready work; both tests assert a decision) |
| Fairness under a continuous direct-successor stream; the retirement preference | `c0_adm_a_retirement_successor_stream_cannot_starve_another_crtc`, `c0_adm_retirement_preference_needs_no_other_crtc_owed` | M10: the retirement preference ignores CRTCs owed the turn |

The receipt, tickets, ageing, tiers 3/4/5/7 and the post-rejection rules are plan B's; the conductor, retirement ordering, the refusal disposition and invalidation as a *wake* are plan A2's.

## File structure

The split below is the intended one; small deviations are your call if you report them.

| File | Responsibility | Task |
| --- | --- | --- |
| `crates/yserver/src/kms/owner/mod.rs` | registers `pub mod admission` | 1 |
| `admission/mod.rs` | `Admission`, `AdmissionError`, `CrtcId`, re-exports | 1–4 |
| `admission/intents.rs` | bounded storage, `PrimaryOrdinal` | 1 |
| `admission/snapshot.rs` | `ReadinessSnapshot`, `IntentKey`, `Readiness`, `WaitReason` | 2 |
| `admission/decide.rs` | `decide`, the tiers, the round-robin | 2, 4 |
| `admission/token.rs` | `lock`, `confirm`, `abort` | 3, 4 |
| `admission/tests.rs` (`#[cfg(test)]`) | all `c0_adm_` tests | 1–4 |

## Gate (every task)

Run in this order and report each result:

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo test -p yserver --lib c0_adm
cargo test -p yserver --lib
```

Task 4 additionally runs `cargo clippy --all-targets --features tcp-transport -- -D warnings`, the same with `--features xdmcp`, and `cargo check --workspace --target <t>` for `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl` and `x86_64-unknown-freebsd`.

---

### Task 1: The module and bounded intent storage

**Files:** `kms/owner/mod.rs` (register), `admission/mod.rs`, `admission/intents.rs`, `admission/tests.rs`.

**Interfaces (public; later tasks and plan A2 rely on these names and types):**

```rust
pub type CrtcId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    StaleGeneration { queued: u64, offered: u64 },
    UnflipPending,
    EmptyCrtcSet,
    // Task 3 adds: AlreadyLocked, DecisionMismatch, TokenMismatch
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrimaryOrdinal(/* private */);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposedIntent { pub generation: u64, pub ordinal: PrimaryOrdinal }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectSuccessor {
    pub source_generation: u64,
    pub layout_generation: u64,
    pub topology_generation: u64,
    pub crtcs: BTreeSet<CrtcId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedDirect { pub successor: DirectSuccessor, pub ordinal: PrimaryOrdinal }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnflipBarrier { pub crtcs: BTreeSet<CrtcId> }

#[derive(Debug, Default)]
pub struct Admission { /* private */ }

impl Admission {
    pub fn new() -> Self;
    pub fn set_composed(&mut self, crtc: CrtcId, generation: u64) -> Result<(), AdmissionError>;
    pub fn set_direct_successor(&mut self, successor: DirectSuccessor)
        -> Result<Option<DirectSuccessor>, AdmissionError>;          // Ok(Some(displaced))
    pub fn withdraw_direct(&mut self, source_generation: u64) -> Option<DirectSuccessor>;
    pub fn request_unflip(&mut self, crtcs: BTreeSet<CrtcId>)
        -> Result<Option<DirectSuccessor>, AdmissionError>;          // Ok(Some(displaced))
    pub fn request_topology(&mut self, generation: u64) -> Result<(), AdmissionError>;
    pub fn composed(&self, crtc: CrtcId) -> Option<ComposedIntent>;
    pub fn direct(&self) -> Option<&QueuedDirect>;
    pub fn unflip(&self) -> Option<&UnflipBarrier>;
    pub fn topology(&self) -> Option<u64>;
}

#[doc(hidden)]
pub fn crtcs(ids: &[CrtcId]) -> BTreeSet<CrtcId>;   // test/fixture convenience
```

**Invariants:**

1. At most one composed intent per CRTC, one direct successor, one unflip barrier and one waiting topology request per `Admission` — enforced by the storage shape, not by a check.
2. A new generation must be strictly newer than the queued one of the same slot, or `StaleGeneration { queued, offered }` and nothing changes. Topology requests follow the same rule.
3. `PrimaryOrdinal` is device-monotonic, assigned when a primary slot (composed or direct) goes from empty to occupied, **kept** across latest-wins replacement, and released with the slot. Overflow panics with a message; it is not silently wrapped.
4. `set_direct_successor` returns the displaced successor so the caller can terminalize it. It refuses an empty CRTC set (`EmptyCrtcSet`) and refuses anything while an unflip barrier is pending (`UnflipPending`), storing nothing.
5. `request_unflip` sets the barrier, or **widens** an existing one to the union of both CRTC sets — it is never replaced (spec §3) — and **displaces** the queued direct successor, returning it. An empty set is `EmptyCrtcSet`.
6. `withdraw_direct(g)` removes the successor only if `g` is its exact `source_generation`; otherwise `None` and nothing changes. Withdrawal consumes no fairness state.

**Named tests (you write them; scenario and what each asserts):**

- `c0_adm_composed_newest_wins_and_keeps_its_ordinal` — set CRTC 1 to generation 10, then 11: the queued generation is 11 and the ordinal is unchanged.
- `c0_adm_composed_refuses_a_stale_generation` — offering the queued generation again is `StaleGeneration { queued: 10, offered: 10 }` and the queued one stays.
- `c0_adm_second_direct_successor_displaces_the_first_and_keeps_the_ordinal` — the first set returns `Ok(None)`; a newer one returns the first as `Some` and keeps the ordinal.
- `c0_adm_unflip_displaces_the_successor_and_refuses_later_direct_work` — `request_unflip` returns the queued successor, the direct slot is empty, a later successor is `UnflipPending`, and the barrier holds the requested CRTCs.
- `c0_adm_withdraw_only_matches_the_queued_generation` — withdrawing another generation is `None` and leaves the slot; the exact one returns it.
- `c0_adm_empty_crtc_sets_are_refused` — both for a successor and for an unflip.
- `c0_adm_ordinals_are_device_monotonic_across_shapes` — composed on CRTC 2, then a direct successor, then composed on CRTC 1: their ordinals are strictly increasing in that order.
- `c0_adm_topology_requests_are_monotonic` — a repeated generation is `StaleGeneration`; a newer one replaces it.
- `c0_adm_a_second_unflip_request_widens_the_barrier` — `request_unflip({1})` then `request_unflip({2})`: the barrier holds `{1, 2}`.

- [ ] **Step 1:** Register the module and write the tests against the interfaces above.
- [ ] **Step 2:** Run `cargo test -p yserver --lib c0_adm` and record the failure (a compile error, since the storage does not exist yet).
- [ ] **Step 3:** Implement the storage.
- [ ] **Step 4:** Run the gate. Expected: fmt and clippy clean; `c0_adm` **9 passed, 0 failed**; full `--lib` 0 failed.
- [ ] **Step 5:** Stop dirty and report: files touched, gate output, and anything this task's text did not settle.

---

### Task 2: The readiness snapshot and tiers 1, 2 and 6

**Files:** `admission/snapshot.rs`, `admission/decide.rs`, `admission/mod.rs` (modules, re-exports), `admission/tests.rs` (append).

**Interfaces:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IntentKey {
    Unflip,
    Composed { crtc: CrtcId, generation: u64 },
    Direct { source_generation: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    SourceWaits, NoReusableBuffer, OrdinaryRetirementOccupied,
    ExitRetirementOccupied, ComposedReturnNotEstablished, NotDirectEligible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness { Ready, Waiting(WaitReason) }

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadinessSnapshot {
    pub layout_generation: u64,
    pub topology_generation: u64,
    pub retirement_wake: bool,
    /* private: the reports */
}
impl ReadinessSnapshot {
    pub fn new(layout_generation: u64, topology_generation: u64) -> Self; // retirement_wake = false
    pub fn report(&mut self, key: IntentKey, readiness: Readiness);
    pub fn readiness(&self, key: IntentKey) -> Option<Readiness>;
    pub fn is_ready(&self, key: IntentKey) -> bool;                      // unreported ⇒ false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier { Topology = 1, Unflip = 2, Primary = 6 }   // plan B adds 3, 4, 5, 7

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admitted {
    Topology { generation: u64 },
    Unflip { crtcs: BTreeSet<CrtcId> },
    Composed { crtc: CrtcId, generation: u64 },
    Direct { successor: DirectSuccessor },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionDecision { pub tier: Tier, pub admitted: Admitted }
impl AdmissionDecision {
    pub fn primary_crtcs(&self) -> BTreeSet<CrtcId>; // topology: empty; unflip: its CRTCs;
                                                       // composed: its CRTC; direct: its set
}

impl Admission {
    pub fn decide(&self, snapshot: &ReadinessSnapshot) -> Option<AdmissionDecision>;
}
```

There is no `IntentKey` for topology: a waiting topology request is ready by definition (spec §4 table).

**Invariants:**

1. Tier 1: a waiting topology request wins over everything.
2. Tier 2: a pending unflip barrier wins over any primary, but only when the snapshot reports `IntentKey::Unflip` as `Ready`.
3. Tier 6: among ready primaries, the **lowest `PrimaryOrdinal`** wins — a total order across composed and direct.
4. A direct successor is a tier-6 candidate only if the snapshot reports its exact `source_generation` `Ready` **and** its `layout_generation` and `topology_generation` equal the snapshot's.
5. A composed intent on a CRTC covered by a pending unflip barrier is not a candidate: the barrier carries that CRTC's return (C.0 §9.1). Composed intents on other CRTCs are unaffected, even while the barrier waits.
6. `decide` is pure: calling it twice gives the same answer and admits nothing.

**Named tests:**

- `c0_adm_tiers_topology_then_unflip_then_primary` — with a ready composed, a ready unflip and a waiting topology request, tier 1 wins; without the topology request, tier 2 wins over the (older) composed.
- `c0_adm_waiting_or_unreported_intent_is_never_admitted` — with composed, unflip and direct intents queued: an empty snapshot admits nothing; `Waiting` reports admit nothing; a `Ready` report for a *different* generation of a queued composed admits nothing.
- `c0_adm_oldest_ready_primary_wins_across_shapes` — a direct successor queued before a composed intent wins when both are ready.
- `c0_adm_ordinal_survives_replacement_and_waiting` — CRTC 1 queued first but `Waiting`, CRTC 2 ready: 2 is chosen. Then CRTC 1 is replaced with a newer generation and both are ready: 1 is chosen, because it kept its age.
- `c0_adm_direct_with_a_stale_layout_or_topology_generation_is_not_admitted` — a ready successor is not admitted when the snapshot's layout generation differs, nor when its topology generation differs; it is admitted when both match.
- `c0_adm_composed_on_an_unflip_crtc_does_not_overtake_the_barrier` — barrier on CRTC 1 not ready; composed on 1 queued **before** composed on 2, both ready: composed 2 is chosen. The order matters: with composed 1 older, only the barrier's suppression can make 2 win, so M4 is caught.
- `c0_adm_decide_is_pure` — two calls, same result; the composed intent is still queued.

- [ ] **Step 1:** Append the tests.
- [ ] **Step 2:** Run `cargo test -p yserver --lib c0_adm` and record the failure (compile errors: the snapshot and `decide` do not exist yet).
- [ ] **Step 3:** Implement the snapshot and `decide`.
- [ ] **Step 4:** Run the gate. Expected: `c0_adm` **16 passed, 0 failed**; full `--lib` 0 failed.
- [ ] **Step 5:** Stop dirty and report.

---

### Task 3: The `lock`/`confirm`/`abort` token

**Files:** `admission/token.rs`, `admission/mod.rs` (new error variants, new private state, re-exports), `admission/tests.rs` (append).

**Interfaces:**

```rust
// AdmissionError gains:
    AlreadyLocked,
    DecisionMismatch,
    TokenMismatch,

#[derive(Debug)]
#[must_use = "a locked admission must be confirmed or aborted"]
pub struct AdmissionToken { /* private */ }          // not Clone
impl AdmissionToken { pub fn decision(&self) -> &AdmissionDecision; }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed { pub decision: AdmissionDecision, pub sequence: u64 }

impl Admission {
    pub fn lock(&mut self, decision: AdmissionDecision, snapshot: &ReadinessSnapshot)
        -> Result<AdmissionToken, AdmissionError>;
    pub fn confirm(&mut self, token: AdmissionToken) -> Result<Confirmed, AdmissionError>;
    pub fn abort(&mut self, token: AdmissionToken) -> Result<(), AdmissionError>;
    pub fn is_locked(&self) -> bool;
    pub fn sequence(&self) -> u64;                    // confirmed admissions so far
}
```

**Invariants:**

1. `lock` refuses with `AlreadyLocked` while a token is outstanding.
2. `lock` refuses with `DecisionMismatch` unless the decision equals what `decide` returns for the snapshot **now**. That one comparison is spec §7's generation-mismatch check, including a direct successor's layout and topology generations. A refused `lock` leaves the decider unlocked.
3. `lock` consumes nothing: no intent is removed and the sequence does not move.
4. `confirm` removes exactly the admitted intent (topology request, barrier, that CRTC's composed intent, or the successor), advances the admission sequence by one, returns it in `Confirmed`, and unlocks.
5. `abort` unlocks and changes nothing else: afterwards `decide` returns the same decision and every ordinal is unchanged.
6. A token only matches the decider that issued it — on **both** `confirm` and `abort`. Tokens from another `Admission`, or stale ones, get `TokenMismatch` and leave the current lock in place. Lock serials must therefore be unique across deciders, not per instance.
7. Dropping a token without confirming or aborting leaves the decider **locked**: it fails closed.

**Named tests:**

- `c0_adm_lock_refuses_a_second_lock_while_a_token_exists`
- `c0_adm_abort_leaves_the_decider_exactly_as_before` — same decision after abort, same ordinal, `sequence() == 0`.
- `c0_adm_confirm_consumes_the_admitted_intent_only` — two composed intents; confirming one empties its slot, leaves the other, and returns `sequence == 1`.
- `c0_adm_lock_detects_a_generation_that_changed_since_decide` — decide, then replace that composed generation, then lock the old decision against a snapshot of the new one: `DecisionMismatch`, and not locked.
- `c0_adm_lock_rejects_a_direct_successor_whose_layout_changed` — decide a direct admission, then lock it against a snapshot with a moved layout generation: `DecisionMismatch`.
- `c0_adm_a_dropped_token_keeps_the_decider_locked`
- `c0_adm_a_foreign_token_is_refused` — two deciders each lock; confirming decider B's token on decider A is `TokenMismatch`, A stays locked, and A's own token then confirms.
- `c0_adm_a_foreign_token_cannot_abort` — the same with `abort`: B's token on A is `TokenMismatch` and A stays locked.
- `c0_adm_confirm_consumes_a_direct_unflip_or_topology_admission_exactly` — three scenarios, each with an unrelated composed intent on another CRTC also queued: confirming a direct admission empties the successor slot; confirming an unflip clears the barrier; confirming a topology admission clears the topology request. In each, the unrelated composed intent is still queued afterwards.

- [ ] **Step 1:** Append the tests.
- [ ] **Step 2:** Run and record the failure (compile errors: no `lock`, `confirm`, `abort`, `is_locked`, `sequence`, and no new variants yet).
- [ ] **Step 3:** Implement the token.
- [ ] **Step 4:** Run the gate. Expected: `c0_adm` **25 passed, 0 failed**; full `--lib` 0 failed.
- [ ] **Step 5:** Stop dirty and report.

---

### Task 4: The per-CRTC round-robin and the retirement preference

**Files:** `admission/decide.rs`, `admission/token.rs`, `admission/mod.rs` (new private state), `admission/tests.rs` (append).

**Interfaces:** no new public items. `confirm` starts recording fairness state, and tier 6 starts filtering on it.

**The rule (spec §5), stated so it can be implemented and tested exactly:**

- A CRTC was **served last** when the immediately previous confirmed admission carried a primary for it (`primary_crtcs()` of that decision; a topology admission carries none).
- A CRTC is **owed** when some ready tier-6 candidate covers it **and none of that candidate's CRTCs was served last** — a CRTC is owed only if something could actually serve it next without breaking the rule.
- A tier-6 candidate is **refused** when any of its CRTCs was served last **and** at least one CRTC is owed. An owed CRTC is never served last, so it is always "another CRTC" in C.0's sense — whether or not the candidate also covers it. Revision 2 refused only for owed CRTCs *outside* the candidate; the review showed that lets a grouped `{A, B}` give A two successive slots while composed B waits (M13).
- The rule cannot deadlock. If every ready candidate touches a CRTC served last, nothing is owed and nothing is refused; the oldest wins.
- **Retirement preference:** on a snapshot with `retirement_wake`, the direct successor is chosen over older candidates if it passes the round-robin **and** every owed CRTC lies inside its CRTC set. Otherwise the oldest eligible candidate wins.

**Invariants:**

1. No CRTC takes two successive admissions while another CRTC with a ready primary is owed (C.0 §9.2.1).
2. A multi-CRTC admission — a grouped successor, or an unflip over several CRTCs — marks **every** CRTC it covers as served.
3. An intervening admission of any kind, including a topology one, ends a CRTC's successive run.
4. The preference never lets a direct stream take a slot that an owed CRTC outside it is waiting for.

**Named tests:**

- `c0_adm_grouped_then_composed_on_its_crtcs_yields_to_the_owed_crtc` — confirm a grouped successor over {1, 2}; then composed intents on 1, 2 and 3, queued in that order and all ready: composed 3 wins. It must use **both** 1 and 2, so that marking only one of the grouped CRTCs as served (M9) is caught.
- `c0_adm_composed_then_grouped_yields_to_the_owed_crtc` — confirm composed 1; then an (older) grouped successor over {1, 2} and a composed on 3: composed 3 wins; confirming it, the grouped successor wins next.
- `c0_adm_when_every_ready_crtc_was_just_served_the_oldest_wins` — after a grouped {1, 2} admission, composed intents on 2 then 1: composed 2 wins (no deadlock).
- `c0_adm_an_intervening_admission_ends_the_successive_run` — composed 1 admitted, then a topology admission, then composed 1 (newer) and composed 2: composed 1 wins, because it is older and no longer successive.
- `c0_adm_retirement_successor_is_preferred_when_no_other_crtc_is_owed` — composed 1 queued before a successor over {1, 2}, both ready: an ordinary snapshot picks composed 1, and the same snapshot with `retirement_wake` picks the successor.
- `c0_adm_retirement_successor_yields_to_an_owed_crtc` — after a grouped {1, 2} admission, a new successor over {1, 2} and a composed on 3 are ready on a retirement wake: composed 3 wins.
- `c0_adm_a_retirement_successor_stream_cannot_starve_another_crtc` — stage 2c §7's continuous stream: a successor over {1} re-queued on every retirement wake, with composed 2 ready throughout. Composed 2 takes the **second** admission.
- `c0_adm_a_grouped_candidate_yields_to_an_owed_crtc_it_contains` — round-1 B-1's sequence: confirm composed 1; then an (older) grouped successor over {1, 2} and a composed on 2, both ready: composed 2 wins. Revision 2's rule admits the grouped one (M13).
- `c0_adm_a_lone_grouped_candidate_is_not_held_back_by_itself` — confirm composed 1; then only a grouped successor over {1, 2} is ready: it is admitted. CRTC 2 is covered only by a candidate that touches CRTC 1, so nothing is owed. This is the deadlock a naive "owed" definition would cause.
- `c0_adm_a_multi_crtc_unflip_serves_every_crtc_it_covers` — confirm an unflip over {1, 2}; then composed intents on 1, 2 and 3, queued in that order and ready: composed 3 wins.
- `c0_adm_retirement_preference_needs_no_other_crtc_owed` — after a topology admission (so nothing is successive), a composed on 3 queued before a successor over {1}, both ready on a retirement wake: composed 3 wins. This isolates M10: the successor passes the round-robin here, so only the preference's own "owed" condition can hold it back.

- [ ] **Step 1:** Append the tests.
- [ ] **Step 2:** Run and record which fail. Expected: at least the two round-robin transition tests and both retirement-successor tests fail. The stream test and `c0_adm_retirement_preference_needs_no_other_crtc_owed` may already pass on age alone; they exist to catch M10 once the preference is in. Report the exact list.
- [ ] **Step 3:** Implement the round-robin and the preference.
- [ ] **Step 4:** Run the gate, including Task 4's additions. Expected: clippy clean in all three configurations; `cargo check` clean on the three targets; `c0_adm` **36 passed, 0 failed**; full `--lib` 0 failed.
- [ ] **Step 5:** Stop dirty and report.

---

## What the coordinator does after each task

1. Reads the diff against the task's interfaces and invariants, and checks every named test exists and exercises its stated scenario.
2. Reruns the task's gate.
3. After Task 4: applies each mutation M1–M17 to **your** code, one at a time, confirms each run compiled, and records which tests fail. Any survivor goes back to you as a finding naming the invariant and the mutation, not a prescribed edit.
4. Commits each task with `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)` and `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
