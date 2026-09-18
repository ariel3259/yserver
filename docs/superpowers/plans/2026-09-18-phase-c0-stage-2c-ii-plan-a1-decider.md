# Stage 2c-ii, plan A1 — the pure admission decider

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), `--sandbox workspace-write`, run with `< /dev/null`. Execute tasks in order, one at a time. Tick steps (`- [ ]` → `- [x]`) only with the evidence each one names. Before writing code, read `AGENTS.md` and, as plain markdown, the Superpowers skills `executing-plans/SKILL.md` and `test-driven-development/SKILL.md` under `~/.claude/plugins/cache/claude-plugins-official/superpowers/*/skills/`. **The implementer never commits**: this worktree's git directory is read-only inside the sandbox. Stop with the tree dirty after each task; the coordinating session verifies and commits.

**Revision 1 (2026-09-18).**

**Goal:** A pure, deterministic decider for the primary side of 2c-ii admission — bounded intent storage, `PrimaryOrdinal`, the readiness snapshot, tiers 1, 2 and 6, the per-CRTC round-robin, and the `lock`/`confirm`/`abort` token — with no resource, no I/O and no caller yet.

**Architecture:** A new module `kms/owner/admission/`: `intents.rs` (storage and bounds), `snapshot.rs` (what the conductor reports), `decide.rs` (the tiers and the round-robin), `token.rs` (two-phase confirmation). `Admission` is one per DRM device. It never touches the owner, the executor or 2c-i's resources: plan A2 builds the conductor that does, and plan B adds maintenance (tickets, tiers 3, 4, 5 and 7, the receipt).

**Tech Stack:** Rust, `std::collections::{BTreeMap, BTreeSet}`, `thiserror` (already a dependency of `yserver`).

**Spec:** `docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md`, revision 3. Sections 3 (units, intents, bounds, `PrimaryOrdinal`), 4 (readiness), 5 (tiers 1, 2, 6 and the round-robin), 6 (the token) and 10.2 (exit criteria) govern this plan. **The split into A1 and A2** was the user's decision on 2026-09-18, measured against the plan-size rule: A1 is the decider, which needs no helper process, and A2 is the conductor over `KmsBackend`.

## How this plan was validated

The coordinating session wrote every code block below in a scratch worktree, **task by task in this order**, and ran each task's full gate before moving to the next: `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`, and the task's tests. The final state then passed clippy with `--features tcp-transport` and `--features xdmcp`, `cargo check --workspace` for `x86_64-unknown-linux-gnu`, `-musl` and `x86_64-unknown-freebsd`, and `cargo test -p yserver --lib` three times: **1808 passed, 0 failed, 95 ignored**, every run. The blocks are the ones that compiled, extracted from those commits, not retyped.

The red state of each task was measured the same way: that task's tests applied over the previous task's code. It is stated in each task's red step.

**The twelve named mutations in the table below were each applied by exact line to the final state, confirmed to compile, and run.** Every one failed at least the test listed. That table is the acceptance criterion for this plan's tests.

**Copy the blocks verbatim.** If a block does not compile or a step's expectation does not hold, stop and report it (F8): do not repair the plan silently. A compile error in your own transcription is yours to fix — compare against the block.

## Global Constraints

- The decider owns no resource and performs no I/O; it takes `&self` in `decide` (spec §2).
- An intent the snapshot does not report is **not ready** (spec §4: the decider fails closed).
- Fairness is accounted **per CRTC**; a multi-CRTC admission serves every CRTC it covers (spec §5, round-2 B-1).
- The confirmation boundary is the owner's **send**, not `begin`: `lock` consumes nothing, `abort` restores exactly, only `confirm` consumes (spec §6).
- Test names start with `c0_adm_`. **Not** `c0_2cii_`: `cargo test c0_2ci` is a substring filter and would pull them into the 2c-i counts.
- `cargo test -p yserver --lib` needs the `yserver` binary built (the executor tests spawn it as their helper). If helper tests fail with `HelperExited` or a device-lock timeout, run `cargo build -p yserver --bin yserver` and rerun before reporting anything.
- No production caller (R8). Nothing outside `kms/owner/admission/` changes except the one `pub mod admission;` line.

## Exit criteria covered by this plan

| Spec 10.2 criterion (A1's part) | Test | Mutation that must fail it (measured) |
| --- | --- | --- |
| Seven tiers in order — tiers 1, 2, 6 | `c0_adm_tiers_topology_then_unflip_then_primary` | M1: let an unflip overtake the topology barrier |
| Supersession bounds: one slot per category | `c0_adm_second_direct_successor_displaces_the_first_and_keeps_the_ordinal`, `c0_adm_unflip_displaces_the_successor_and_refuses_later_direct_work` | M12: unflip does not displace the successor |
| No dispatch before readiness | `c0_adm_waiting_or_unreported_intent_is_never_admitted` | M2: treat any report (including `Waiting`) as ready |
| `PrimaryOrdinal` orders across shapes; survives replacement and `Waiting` | `c0_adm_oldest_ready_primary_wins_across_shapes`, `c0_adm_ordinal_survives_replacement_and_waiting`, `c0_adm_composed_newest_wins_and_keeps_its_ordinal` | M5: give a replaced composed slot a new ordinal |
| A queued direct successor whose layout changed is never admitted | `c0_adm_direct_with_a_stale_layout_or_topology_generation_is_not_admitted`, `c0_adm_lock_rejects_a_direct_successor_whose_layout_changed` | M3: drop the layout/topology check |
| The barrier is not overtaken by a later primary | `c0_adm_composed_on_an_unflip_crtc_does_not_overtake_the_barrier` | M4: let composed on a barrier CRTC compete |
| Two-phase: `abort` consumes nothing | `c0_adm_abort_leaves_the_decider_exactly_as_before` | M6: consume state in `lock`; M11: consume state in `abort` |
| A generation mismatch is caught at `lock` | `c0_adm_lock_detects_a_generation_that_changed_since_decide` | M7: skip `lock`'s re-decision |
| Round-robin per CRTC, grouped→composed and composed→grouped | `c0_adm_grouped_then_composed_on_its_crtcs_yields_to_the_owed_crtc`, `c0_adm_composed_then_grouped_yields_to_the_owed_crtc` | M8: the round-robin always permits; M9: mark only one CRTC of a multi-CRTC admission as served |
| Fairness under a continuous direct-successor stream; the retirement preference | `c0_adm_a_retirement_successor_stream_cannot_starve_another_crtc`, `c0_adm_retirement_preference_needs_no_other_crtc_owed` | M10: the preference ignores owed CRTCs |

The receipt, tickets, ageing, tiers 3/4/5/7 and the post-rejection rules are plan B's; the conductor, retirement ordering, the refusal disposition and invalidation as a *wake* are plan A2's.

## File structure

| File | Responsibility | Task |
| --- | --- | --- |
| `crates/yserver/src/kms/owner/mod.rs` | registers `pub mod admission` | 1 |
| `crates/yserver/src/kms/owner/admission/mod.rs` | `Admission`, `AdmissionError`, `CrtcId`, re-exports | 1, 2, 3, 4 |
| `crates/yserver/src/kms/owner/admission/intents.rs` | bounded storage, `PrimaryOrdinal` | 1 |
| `crates/yserver/src/kms/owner/admission/snapshot.rs` | `ReadinessSnapshot`, `IntentKey`, `Readiness`, `WaitReason` | 2 |
| `crates/yserver/src/kms/owner/admission/decide.rs` | `decide`, tiers, round-robin | 2, 4 |
| `crates/yserver/src/kms/owner/admission/token.rs` | `lock`, `confirm`, `abort` | 3, 4 |
| `crates/yserver/src/kms/owner/admission/tests.rs` | all `c0_adm_` tests; each task appends | 1, 2, 3, 4 |

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

**Files:**
- Modify: `crates/yserver/src/kms/owner/mod.rs` (one module line)
- Create: `crates/yserver/src/kms/owner/admission/mod.rs`
- Create: `crates/yserver/src/kms/owner/admission/intents.rs`
- Create: `crates/yserver/src/kms/owner/admission/tests.rs`

**Interfaces:**
- Produces: `Admission::{new, set_composed, set_direct_successor, withdraw_direct, request_unflip, request_topology, composed, direct, unflip, topology}`; `AdmissionError::{StaleGeneration, UnflipPending, EmptyCrtcSet}`; `PrimaryOrdinal`, `ComposedIntent`, `DirectSuccessor`, `QueuedDirect`, `UnflipBarrier`; `CrtcId = u32`; `crtcs(&[CrtcId]) -> BTreeSet<CrtcId>` (a `#[doc(hidden)]` test helper).

- [ ] **Step 1: Register the module.** In `crates/yserver/src/kms/owner/mod.rs`, insert these two lines immediately **before** the existing `#[doc(hidden)]` line that precedes `pub mod build;`:

```rust
#[doc(hidden)]
pub mod admission;
```

- [ ] **Step 2: Write `admission/mod.rs`** with exactly:

```rust
//! Stage 2c-ii admission: the pure decider (2c-ii design §2).
//!
//! It holds only *descriptors* of the bounded primary intents — generations,
//! ages, turns — and, given a readiness snapshot the conductor supplies,
//! decides which one takes the device slot next. It owns no resource and
//! performs no I/O: resources stay with 2c-i's roles, pools and ledgers.

use std::collections::{BTreeMap, BTreeSet};

mod intents;
#[cfg(test)]
mod tests;

pub use intents::{ComposedIntent, DirectSuccessor, PrimaryOrdinal, QueuedDirect, UnflipBarrier};

/// A KMS CRTC object id.
pub type CrtcId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    #[error("generation {offered} is not newer than the queued {queued}")]
    StaleGeneration { queued: u64, offered: u64 },
    #[error("an unflip barrier is pending; direct work cannot supersede it")]
    UnflipPending,
    #[error("an intent must cover at least one CRTC")]
    EmptyCrtcSet,
}

/// The per-device decider. One per DRM device, like the owner's slot.
#[derive(Debug, Default)]
pub struct Admission {
    /// One composed desired state per CRTC (C.0 §9.1).
    composed: BTreeMap<CrtcId, ComposedIntent>,
    /// The grouped-direct unit's latest-wins successor: one per device.
    direct: Option<QueuedDirect>,
    /// The non-supersedable unflip/recovery barrier.
    unflip: Option<UnflipBarrier>,
    /// A waiting topology/lifecycle request, by its generation.
    topology: Option<u64>,
    next_ordinal: u64,
}

#[doc(hidden)]
pub fn crtcs(ids: &[CrtcId]) -> BTreeSet<CrtcId> {
    ids.iter().copied().collect()
}
```

- [ ] **Step 3: Write the tests**, `admission/tests.rs`, with exactly:

```rust
use super::*;

fn successor(source_generation: u64, ids: &[CrtcId]) -> DirectSuccessor {
    DirectSuccessor {
        source_generation,
        layout_generation: 1,
        topology_generation: 1,
        crtcs: crtcs(ids),
    }
}

#[test]
fn c0_adm_composed_newest_wins_and_keeps_its_ordinal() {
    let mut a = Admission::new();
    a.set_composed(1, 10).unwrap();
    let first = a.composed(1).unwrap().ordinal;
    a.set_composed(1, 11).unwrap();
    let queued = a.composed(1).unwrap();
    assert_eq!(
        queued.generation, 11,
        "one composed state per CRTC, newest wins"
    );
    assert_eq!(queued.ordinal, first, "replacement keeps the slot's age");
}

#[test]
fn c0_adm_composed_refuses_a_stale_generation() {
    let mut a = Admission::new();
    a.set_composed(1, 10).unwrap();
    assert_eq!(
        a.set_composed(1, 10),
        Err(AdmissionError::StaleGeneration {
            queued: 10,
            offered: 10
        })
    );
    assert_eq!(a.composed(1).unwrap().generation, 10);
}

#[test]
fn c0_adm_second_direct_successor_displaces_the_first_and_keeps_the_ordinal() {
    let mut a = Admission::new();
    assert_eq!(a.set_direct_successor(successor(5, &[1, 2])), Ok(None));
    let first = a.direct().unwrap().ordinal;
    let displaced = a.set_direct_successor(successor(6, &[1, 2])).unwrap();
    assert_eq!(
        displaced,
        Some(successor(5, &[1, 2])),
        "the victim goes back to the caller"
    );
    let queued = a.direct().unwrap();
    assert_eq!(
        queued.successor.source_generation, 6,
        "exactly one successor slot"
    );
    assert_eq!(queued.ordinal, first);
}

#[test]
fn c0_adm_unflip_displaces_the_successor_and_refuses_later_direct_work() {
    let mut a = Admission::new();
    a.set_direct_successor(successor(5, &[1, 2])).unwrap();
    let displaced = a.request_unflip(crtcs(&[1, 2])).unwrap();
    assert_eq!(displaced, Some(successor(5, &[1, 2])));
    assert!(a.direct().is_none());
    assert_eq!(
        a.set_direct_successor(successor(6, &[1, 2])),
        Err(AdmissionError::UnflipPending),
        "a later primary intent never supersedes the barrier"
    );
    assert_eq!(a.unflip().unwrap().crtcs, crtcs(&[1, 2]));
}

#[test]
fn c0_adm_withdraw_only_matches_the_queued_generation() {
    let mut a = Admission::new();
    a.set_direct_successor(successor(5, &[1])).unwrap();
    assert_eq!(a.withdraw_direct(4), None);
    assert!(a.direct().is_some());
    assert_eq!(a.withdraw_direct(5), Some(successor(5, &[1])));
    assert!(a.direct().is_none());
}

#[test]
fn c0_adm_empty_crtc_sets_are_refused() {
    let mut a = Admission::new();
    assert_eq!(
        a.set_direct_successor(successor(5, &[])),
        Err(AdmissionError::EmptyCrtcSet)
    );
    assert_eq!(
        a.request_unflip(crtcs(&[])),
        Err(AdmissionError::EmptyCrtcSet)
    );
}

#[test]
fn c0_adm_ordinals_are_device_monotonic_across_shapes() {
    let mut a = Admission::new();
    a.set_composed(2, 1).unwrap();
    a.set_direct_successor(successor(1, &[1])).unwrap();
    a.set_composed(1, 1).unwrap();
    let c2 = a.composed(2).unwrap().ordinal;
    let d = a.direct().unwrap().ordinal;
    let c1 = a.composed(1).unwrap().ordinal;
    assert!(
        c2 < d && d < c1,
        "arrival order, across composed and direct"
    );
}

#[test]
fn c0_adm_topology_requests_are_monotonic() {
    let mut a = Admission::new();
    a.request_topology(3).unwrap();
    assert_eq!(
        a.request_topology(3),
        Err(AdmissionError::StaleGeneration {
            queued: 3,
            offered: 3
        })
    );
    a.request_topology(4).unwrap();
    assert_eq!(a.topology(), Some(4));
}
```

- [ ] **Step 4: Run to see it fail.** `cargo test -p yserver --lib c0_adm` — expected: a compile error, because `intents.rs` does not exist yet (`file not found for module intents`).

- [ ] **Step 5: Write `admission/intents.rs`** with exactly:

```rust
//! Bounded intent storage (2c-ii design §3). Every slot is an `Option` or
//! one map entry per CRTC, so the bounds are structural.

use std::collections::BTreeSet;

use super::{Admission, AdmissionError, CrtcId};

/// Device-monotonic age of a primary slot (2c-ii design §3). Assigned when a
/// slot goes from empty to occupied; it survives latest-wins replacement and
/// periods of `Waiting`, and is released with the slot. Unique per device, so
/// "oldest" is a total order across composed and direct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrimaryOrdinal(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposedIntent {
    pub generation: u64,
    pub ordinal: PrimaryOrdinal,
}

/// What the conductor offers for the direct successor slot. `crtcs` is the
/// grouped unit's exact output set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectSuccessor {
    pub source_generation: u64,
    pub layout_generation: u64,
    pub topology_generation: u64,
    pub crtcs: BTreeSet<CrtcId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedDirect {
    pub successor: DirectSuccessor,
    pub ordinal: PrimaryOrdinal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnflipBarrier {
    pub crtcs: BTreeSet<CrtcId>,
}

impl Admission {
    pub fn new() -> Self {
        Self::default()
    }

    fn allocate_ordinal(&mut self) -> PrimaryOrdinal {
        let ordinal = PrimaryOrdinal(self.next_ordinal);
        self.next_ordinal = self
            .next_ordinal
            .checked_add(1)
            .expect("PrimaryOrdinal space exhausted");
        ordinal
    }

    /// Newest composed generation wins; never a queue of rendered frames
    /// (C.0 §9.1). Replacement keeps the slot's ordinal.
    pub fn set_composed(&mut self, crtc: CrtcId, generation: u64) -> Result<(), AdmissionError> {
        if let Some(queued) = self.composed.get_mut(&crtc) {
            if generation <= queued.generation {
                return Err(AdmissionError::StaleGeneration {
                    queued: queued.generation,
                    offered: generation,
                });
            }
            queued.generation = generation;
            return Ok(());
        }
        let ordinal = self.allocate_ordinal();
        self.composed.insert(
            crtc,
            ComposedIntent {
                generation,
                ordinal,
            },
        );
        Ok(())
    }

    /// Latest-wins direct successor (C.0 §9.1). Returns the displaced
    /// successor: the conductor runs it through 2c-i's never-submitted path.
    /// Replacement keeps the slot's ordinal. Refused while an unflip barrier
    /// is pending, which no later primary intent may supersede.
    pub fn set_direct_successor(
        &mut self,
        successor: DirectSuccessor,
    ) -> Result<Option<DirectSuccessor>, AdmissionError> {
        if successor.crtcs.is_empty() {
            return Err(AdmissionError::EmptyCrtcSet);
        }
        if self.unflip.is_some() {
            return Err(AdmissionError::UnflipPending);
        }
        if let Some(queued) = self.direct.as_mut() {
            if successor.source_generation <= queued.successor.source_generation {
                return Err(AdmissionError::StaleGeneration {
                    queued: queued.successor.source_generation,
                    offered: successor.source_generation,
                });
            }
            return Ok(Some(std::mem::replace(&mut queued.successor, successor)));
        }
        let ordinal = self.allocate_ordinal();
        self.direct = Some(QueuedDirect { successor, ordinal });
        Ok(None)
    }

    /// Removes the queued successor without admitting it — eligibility lost,
    /// or its leases terminalized by a pre-IPC refusal. Consumes no fairness
    /// state. `None` if `source_generation` is not the queued one.
    pub fn withdraw_direct(&mut self, source_generation: u64) -> Option<DirectSuccessor> {
        if self.direct.as_ref()?.successor.source_generation != source_generation {
            return None;
        }
        self.direct.take().map(|queued| queued.successor)
    }

    /// Sets (or widens) the unflip barrier and displaces the unsent direct
    /// successor, which the conductor terminalizes (C.0 §9.1).
    pub fn request_unflip(
        &mut self,
        crtcs: BTreeSet<CrtcId>,
    ) -> Result<Option<DirectSuccessor>, AdmissionError> {
        if crtcs.is_empty() {
            return Err(AdmissionError::EmptyCrtcSet);
        }
        match self.unflip.as_mut() {
            Some(barrier) => barrier.crtcs.extend(crtcs),
            None => self.unflip = Some(UnflipBarrier { crtcs }),
        }
        Ok(self.direct.take().map(|queued| queued.successor))
    }

    /// Records a waiting topology/lifecycle request.
    pub fn request_topology(&mut self, generation: u64) -> Result<(), AdmissionError> {
        if let Some(queued) = self.topology
            && generation <= queued
        {
            return Err(AdmissionError::StaleGeneration {
                queued,
                offered: generation,
            });
        }
        self.topology = Some(generation);
        Ok(())
    }

    pub fn composed(&self, crtc: CrtcId) -> Option<ComposedIntent> {
        self.composed.get(&crtc).copied()
    }

    pub fn direct(&self) -> Option<&QueuedDirect> {
        self.direct.as_ref()
    }

    pub fn unflip(&self) -> Option<&UnflipBarrier> {
        self.unflip.as_ref()
    }

    pub fn topology(&self) -> Option<u64> {
        self.topology
    }
}
```

- [ ] **Step 6: Run the gate.** Expected: fmt makes no change to the blocks; clippy clean; `c0_adm`: **8 passed, 0 failed**; full `--lib`: 0 failed.

- [ ] **Step 7: Stop dirty and report.** Do not commit.

---

### Task 2: The readiness snapshot and tiers 1, 2 and 6

**Files:**
- Modify: `crates/yserver/src/kms/owner/admission/mod.rs` (replace the whole file)
- Create: `crates/yserver/src/kms/owner/admission/snapshot.rs`
- Create: `crates/yserver/src/kms/owner/admission/decide.rs`
- Modify: `crates/yserver/src/kms/owner/admission/tests.rs` (append)

**Interfaces:**
- Consumes: Task 1's storage.
- Produces: `ReadinessSnapshot::{new(layout_generation, topology_generation), report, readiness, is_ready}` with pub fields `layout_generation`, `topology_generation`, `retirement_wake`; `IntentKey::{Unflip, Composed { crtc, generation }, Direct { source_generation }}`; `Readiness::{Ready, Waiting(WaitReason)}`; `WaitReason`; `Tier::{Topology = 1, Unflip = 2, Primary = 6}`; `Admitted::{Topology, Unflip, Composed, Direct}`; `AdmissionDecision { tier, admitted }` with `primary_crtcs()`; `Admission::decide(&self, &ReadinessSnapshot) -> Option<AdmissionDecision>`.

- [ ] **Step 1: Append the tests** to `admission/tests.rs`:

```rust
fn ready(snapshot: &mut ReadinessSnapshot, key: IntentKey) {
    snapshot.report(key, Readiness::Ready);
}

fn snapshot() -> ReadinessSnapshot {
    ReadinessSnapshot::new(1, 1)
}

#[test]
fn c0_adm_tiers_topology_then_unflip_then_primary() {
    let mut a = Admission::new();
    a.set_composed(1, 1).unwrap();
    a.request_unflip(crtcs(&[2])).unwrap();
    a.request_topology(7).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
    );
    ready(&mut s, IntentKey::Unflip);

    let d = a.decide(&s).unwrap();
    assert_eq!(d.tier, Tier::Topology);
    assert_eq!(d.admitted, Admitted::Topology { generation: 7 });

    let mut a = Admission::new();
    a.set_composed(1, 1).unwrap();
    a.request_unflip(crtcs(&[2])).unwrap();
    let d = a.decide(&s).unwrap();
    assert_eq!(
        d.tier,
        Tier::Unflip,
        "the barrier outranks an older primary"
    );
    assert_eq!(d.admitted, Admitted::Unflip { crtcs: crtcs(&[2]) });
}

#[test]
fn c0_adm_waiting_or_unreported_intent_is_never_admitted() {
    let mut a = Admission::new();
    a.set_composed(1, 1).unwrap();
    a.set_direct_successor(successor(1, &[2])).unwrap();
    a.request_unflip(crtcs(&[3])).unwrap();
    // The unflip displaced the successor; queue a composed on 2 too.
    a.set_composed(2, 1).unwrap();

    assert_eq!(a.decide(&snapshot()), None, "unreported is not ready");

    let mut s = snapshot();
    s.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
        Readiness::Waiting(WaitReason::NoReusableBuffer),
    );
    s.report(
        IntentKey::Unflip,
        Readiness::Waiting(WaitReason::ExitRetirementOccupied),
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 2,
            generation: 0,
        },
    );
    assert_eq!(
        a.decide(&s),
        None,
        "a report about another generation does not make this one ready"
    );
}

#[test]
fn c0_adm_oldest_ready_primary_wins_across_shapes() {
    let mut a = Admission::new();
    a.set_direct_successor(successor(1, &[1, 2])).unwrap();
    a.set_composed(3, 1).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Direct {
            source_generation: 1,
        },
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 3,
            generation: 1,
        },
    );
    let d = a.decide(&s).unwrap();
    assert_eq!(d.tier, Tier::Primary);
    assert_eq!(
        d.admitted,
        Admitted::Direct {
            successor: successor(1, &[1, 2])
        },
        "the direct slot is older by PrimaryOrdinal"
    );
}

#[test]
fn c0_adm_ordinal_survives_replacement_and_waiting() {
    let mut a = Admission::new();
    a.set_composed(1, 1).unwrap();
    a.set_composed(2, 1).unwrap();
    // CRTC 1 waits for a buffer while CRTC 2 is ready: 2 wins meanwhile.
    let mut s = snapshot();
    s.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
        Readiness::Waiting(WaitReason::NoReusableBuffer),
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 2,
            generation: 1,
        },
    );
    assert_eq!(
        a.decide(&s).unwrap().admitted,
        Admitted::Composed {
            crtc: 2,
            generation: 1
        }
    );
    // CRTC 1 is replaced and becomes ready: it is still the older slot.
    a.set_composed(1, 2).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 1,
            generation: 2,
        },
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 2,
            generation: 1,
        },
    );
    assert_eq!(
        a.decide(&s).unwrap().admitted,
        Admitted::Composed {
            crtc: 1,
            generation: 2
        }
    );
}

#[test]
fn c0_adm_direct_with_a_stale_layout_or_topology_generation_is_not_admitted() {
    let mut a = Admission::new();
    a.set_direct_successor(successor(1, &[1])).unwrap();
    for (layout, topology) in [(2, 1), (1, 2)] {
        let mut s = ReadinessSnapshot::new(layout, topology);
        ready(
            &mut s,
            IntentKey::Direct {
                source_generation: 1,
            },
        );
        assert_eq!(
            a.decide(&s),
            None,
            "a successor queued under another layout/topology is not direct-eligible now"
        );
    }
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Direct {
            source_generation: 1,
        },
    );
    assert!(a.decide(&s).is_some());
}

#[test]
fn c0_adm_composed_on_an_unflip_crtc_does_not_overtake_the_barrier() {
    let mut a = Admission::new();
    a.request_unflip(crtcs(&[1])).unwrap();
    a.set_composed(1, 1).unwrap();
    a.set_composed(2, 1).unwrap();
    let mut s = snapshot();
    s.report(
        IntentKey::Unflip,
        Readiness::Waiting(WaitReason::ComposedReturnNotEstablished),
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 2,
            generation: 1,
        },
    );
    assert_eq!(
        a.decide(&s).unwrap().admitted,
        Admitted::Composed {
            crtc: 2,
            generation: 1
        },
        "CRTC 1 waits for its barrier; CRTC 2 is unaffected"
    );
}

#[test]
fn c0_adm_decide_is_pure() {
    let mut a = Admission::new();
    a.set_composed(1, 1).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
    );
    let first = a.decide(&s);
    assert_eq!(a.decide(&s), first);
    assert_eq!(
        a.composed(1).unwrap().generation,
        1,
        "deciding admits nothing"
    );
}
```

- [ ] **Step 2: Run to see it fail.** `cargo test -p yserver --lib c0_adm` — expected: compile errors (`ReadinessSnapshot`, `IntentKey`, `decide` … not found).

- [ ] **Step 3: Replace `admission/mod.rs`** with exactly:

```rust
//! Stage 2c-ii admission: the pure decider (2c-ii design §2).
//!
//! It holds only *descriptors* of the bounded primary intents — generations,
//! ages, turns — and, given a readiness snapshot the conductor supplies,
//! decides which one takes the device slot next. It owns no resource and
//! performs no I/O: resources stay with 2c-i's roles, pools and ledgers.

use std::collections::{BTreeMap, BTreeSet};

mod decide;
mod intents;
mod snapshot;
#[cfg(test)]
mod tests;

pub use decide::{AdmissionDecision, Admitted, Tier};
pub use intents::{ComposedIntent, DirectSuccessor, PrimaryOrdinal, QueuedDirect, UnflipBarrier};
pub use snapshot::{IntentKey, Readiness, ReadinessSnapshot, WaitReason};

/// A KMS CRTC object id.
pub type CrtcId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    #[error("generation {offered} is not newer than the queued {queued}")]
    StaleGeneration { queued: u64, offered: u64 },
    #[error("an unflip barrier is pending; direct work cannot supersede it")]
    UnflipPending,
    #[error("an intent must cover at least one CRTC")]
    EmptyCrtcSet,
}

/// The per-device decider. One per DRM device, like the owner's slot.
#[derive(Debug, Default)]
pub struct Admission {
    /// One composed desired state per CRTC (C.0 §9.1).
    composed: BTreeMap<CrtcId, ComposedIntent>,
    /// The grouped-direct unit's latest-wins successor: one per device.
    direct: Option<QueuedDirect>,
    /// The non-supersedable unflip/recovery barrier.
    unflip: Option<UnflipBarrier>,
    /// A waiting topology/lifecycle request, by its generation.
    topology: Option<u64>,
    next_ordinal: u64,
}

#[doc(hidden)]
pub fn crtcs(ids: &[CrtcId]) -> BTreeSet<CrtcId> {
    ids.iter().copied().collect()
}
```

- [ ] **Step 4: Write `admission/snapshot.rs`** with exactly:

```rust
//! The readiness snapshot (2c-ii design §4). The conductor builds one per
//! wake; the decider queries nothing else. An intent the snapshot does not
//! report is not ready: the decider fails closed.

use std::collections::BTreeMap;

use super::CrtcId;

/// Identifies one queued descriptor by its exact generation, so a report
/// about an older generation can never make a newer one ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IntentKey {
    Unflip,
    Composed { crtc: CrtcId, generation: u64 },
    Direct { source_generation: u64 },
}

/// Why an intent is not ready. For telemetry and tests; never the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    SourceWaits,
    NoReusableBuffer,
    OrdinaryRetirementOccupied,
    ExitRetirementOccupied,
    ComposedReturnNotEstablished,
    NotDirectEligible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    Waiting(WaitReason),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadinessSnapshot {
    /// The current layout/eligibility generation: a direct successor queued
    /// under another one is not admissible (2c-ii design §4).
    pub layout_generation: u64,
    /// The current topology generation, likewise.
    pub topology_generation: u64,
    /// This wake is a retirement (`CompletionRetired`): tier 6 prefers the
    /// retirement successor (C.0 §9.2.1).
    pub retirement_wake: bool,
    entries: BTreeMap<IntentKey, Readiness>,
}

impl ReadinessSnapshot {
    pub fn new(layout_generation: u64, topology_generation: u64) -> Self {
        Self {
            layout_generation,
            topology_generation,
            retirement_wake: false,
            entries: BTreeMap::new(),
        }
    }

    pub fn report(&mut self, key: IntentKey, readiness: Readiness) {
        self.entries.insert(key, readiness);
    }

    pub fn readiness(&self, key: IntentKey) -> Option<Readiness> {
        self.entries.get(&key).copied()
    }

    /// Unreported is not ready.
    pub fn is_ready(&self, key: IntentKey) -> bool {
        self.readiness(key) == Some(Readiness::Ready)
    }
}
```

- [ ] **Step 5: Write `admission/decide.rs`** with exactly:

```rust
//! The admission function (2c-ii design §5): tiers 1, 2 and 6 of C.0
//! §9.2.1. Pure — `decide` takes `&self` and changes nothing.

use std::collections::BTreeSet;

use super::{Admission, CrtcId, DirectSuccessor, IntentKey, PrimaryOrdinal, ReadinessSnapshot};

/// C.0 §9.2.1's tier numbers. Plan B adds tiers 3, 4, 5 and 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Topology = 1,
    Unflip = 2,
    Primary = 6,
}

/// What is admitted, with the exact generations `lock` checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admitted {
    Topology { generation: u64 },
    Unflip { crtcs: BTreeSet<CrtcId> },
    Composed { crtc: CrtcId, generation: u64 },
    Direct { successor: DirectSuccessor },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionDecision {
    pub tier: Tier,
    pub admitted: Admitted,
}

impl AdmissionDecision {
    /// The CRTCs this admission carries a primary for. A topology barrier
    /// carries none.
    pub fn primary_crtcs(&self) -> BTreeSet<CrtcId> {
        match &self.admitted {
            Admitted::Topology { .. } => BTreeSet::new(),
            Admitted::Unflip { crtcs } => crtcs.clone(),
            Admitted::Composed { crtc, .. } => BTreeSet::from([*crtc]),
            Admitted::Direct { successor } => successor.crtcs.clone(),
        }
    }
}

/// A ready tier-6 candidate.
struct Candidate {
    ordinal: PrimaryOrdinal,
    admitted: Admitted,
}

impl Admission {
    /// Walks the tiers in order and returns the first that applies.
    pub fn decide(&self, snapshot: &ReadinessSnapshot) -> Option<AdmissionDecision> {
        // Tier 1: a topology barrier is ready whenever it is waiting (2c-ii
        // design §4).
        if let Some(generation) = self.topology {
            return Some(AdmissionDecision {
                tier: Tier::Topology,
                admitted: Admitted::Topology { generation },
            });
        }
        // Tier 2: unflip/recovery, once its exit retirement and composed
        // return path are established.
        if let Some(barrier) = &self.unflip
            && snapshot.is_ready(IntentKey::Unflip)
        {
            return Some(AdmissionDecision {
                tier: Tier::Unflip,
                admitted: Admitted::Unflip {
                    crtcs: barrier.crtcs.clone(),
                },
            });
        }
        // Tier 6: the oldest ready primary.
        let winner = self
            .primary_candidates(snapshot)
            .into_iter()
            .min_by_key(|candidate| candidate.ordinal)?;
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: winner.admitted,
        })
    }

    fn primary_candidates(&self, snapshot: &ReadinessSnapshot) -> Vec<Candidate> {
        let barrier_crtcs = self
            .unflip
            .as_ref()
            .map(|barrier| barrier.crtcs.clone())
            .unwrap_or_default();
        let mut candidates = Vec::new();
        for (&crtc, intent) in &self.composed {
            // A pending barrier carries these CRTCs' composed return; a later
            // composed intent does not overtake it (C.0 §9.1).
            if barrier_crtcs.contains(&crtc) {
                continue;
            }
            if snapshot.is_ready(IntentKey::Composed {
                crtc,
                generation: intent.generation,
            }) {
                candidates.push(Candidate {
                    ordinal: intent.ordinal,
                    admitted: Admitted::Composed {
                        crtc,
                        generation: intent.generation,
                    },
                });
            }
        }
        if let Some(queued) = &self.direct {
            let successor = &queued.successor;
            // Direct eligibility is re-proven on every admission, including
            // retirement promotion (stage 2c, v1.5.0 table).
            let current = successor.layout_generation == snapshot.layout_generation
                && successor.topology_generation == snapshot.topology_generation;
            if current
                && snapshot.is_ready(IntentKey::Direct {
                    source_generation: successor.source_generation,
                })
            {
                candidates.push(Candidate {
                    ordinal: queued.ordinal,
                    admitted: Admitted::Direct {
                        successor: successor.clone(),
                    },
                });
            }
        }
        candidates
    }
}
```

- [ ] **Step 6: Run the gate.** Expected: clippy clean; `c0_adm`: **15 passed, 0 failed**; full `--lib`: 0 failed.

- [ ] **Step 7: Stop dirty and report.**

---

### Task 3: The `lock`/`confirm`/`abort` token

**Files:**
- Modify: `crates/yserver/src/kms/owner/admission/mod.rs` (replace the whole file)
- Create: `crates/yserver/src/kms/owner/admission/token.rs`
- Modify: `crates/yserver/src/kms/owner/admission/tests.rs` (append)

**Interfaces:**
- Consumes: Task 2's `decide`.
- Produces: `Admission::{lock(&mut self, AdmissionDecision, &ReadinessSnapshot) -> Result<AdmissionToken, AdmissionError>, confirm(&mut self, AdmissionToken) -> Result<Confirmed, AdmissionError>, abort(&mut self, AdmissionToken) -> Result<(), AdmissionError>, is_locked, sequence}`; `AdmissionToken` (not `Clone`, `#[must_use]`, `decision()`); `Confirmed { decision, sequence }`; `AdmissionError::{AlreadyLocked, DecisionMismatch, TokenMismatch}`.

`lock` refuses unless the decision equals what `decide` returns for the snapshot **now** — that single comparison is the generation-mismatch check of spec §7, including a direct successor's layout and topology generations. Lock serials come from a process-wide counter, so a token can only match the decider that issued it.

- [ ] **Step 1: Append the tests** to `admission/tests.rs`:

```rust
fn one_ready_composed() -> (Admission, ReadinessSnapshot) {
    let mut a = Admission::new();
    a.set_composed(1, 1).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
    );
    (a, s)
}

#[test]
fn c0_adm_lock_refuses_a_second_lock_while_a_token_exists() {
    let (mut a, s) = one_ready_composed();
    let d = a.decide(&s).unwrap();
    let token = a.lock(d.clone(), &s).unwrap();
    assert_eq!(a.lock(d, &s).unwrap_err(), AdmissionError::AlreadyLocked);
    a.abort(token).unwrap();
}

#[test]
fn c0_adm_abort_leaves_the_decider_exactly_as_before() {
    let (mut a, s) = one_ready_composed();
    let before = a.decide(&s);
    let ordinal = a.composed(1).unwrap().ordinal;
    let token = a.lock(before.clone().unwrap(), &s).unwrap();
    a.abort(token).unwrap();
    assert!(!a.is_locked());
    assert_eq!(a.decide(&s), before, "the same decision is made again");
    assert_eq!(
        a.composed(1).unwrap().ordinal,
        ordinal,
        "the slot kept its age"
    );
    assert_eq!(a.sequence(), 0, "no admission was counted");
}

#[test]
fn c0_adm_confirm_consumes_the_admitted_intent_only() {
    let (mut a, s) = one_ready_composed();
    a.set_composed(2, 1).unwrap();
    let token = a.lock(a.decide(&s).unwrap(), &s).unwrap();
    let confirmed = a.confirm(token).unwrap();
    assert_eq!(
        confirmed.decision.admitted,
        Admitted::Composed {
            crtc: 1,
            generation: 1
        }
    );
    assert_eq!(confirmed.sequence, 1);
    assert!(a.composed(1).is_none(), "the admitted slot is empty");
    assert!(a.composed(2).is_some(), "the other slot is untouched");
    assert!(!a.is_locked());
}

#[test]
fn c0_adm_lock_detects_a_generation_that_changed_since_decide() {
    let (mut a, s) = one_ready_composed();
    let stale = a.decide(&s).unwrap();
    a.set_composed(1, 2).unwrap();
    let mut s2 = snapshot();
    ready(
        &mut s2,
        IntentKey::Composed {
            crtc: 1,
            generation: 2,
        },
    );
    assert_eq!(
        a.lock(stale, &s2).unwrap_err(),
        AdmissionError::DecisionMismatch
    );
    assert!(!a.is_locked(), "a refused lock holds nothing");
}

#[test]
fn c0_adm_lock_rejects_a_direct_successor_whose_layout_changed() {
    let mut a = Admission::new();
    a.set_direct_successor(successor(1, &[1])).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Direct {
            source_generation: 1,
        },
    );
    let d = a.decide(&s).unwrap();
    // A border appeared on an ancestor: the layout generation moved on.
    let mut moved = ReadinessSnapshot::new(2, 1);
    ready(
        &mut moved,
        IntentKey::Direct {
            source_generation: 1,
        },
    );
    assert_eq!(
        a.lock(d, &moved).unwrap_err(),
        AdmissionError::DecisionMismatch
    );
}

#[test]
fn c0_adm_a_dropped_token_keeps_the_decider_locked() {
    let (mut a, s) = one_ready_composed();
    let d = a.decide(&s).unwrap();
    drop(a.lock(d.clone(), &s).unwrap());
    assert!(a.is_locked(), "an unconsumed token fails closed");
    assert_eq!(a.lock(d, &s).unwrap_err(), AdmissionError::AlreadyLocked);
}

#[test]
fn c0_adm_a_foreign_token_is_refused() {
    let (mut a, s) = one_ready_composed();
    let (mut b, sb) = one_ready_composed();
    let theirs = b.lock(b.decide(&sb).unwrap(), &sb).unwrap();
    let ours = a.lock(a.decide(&s).unwrap(), &s).unwrap();
    assert_eq!(
        a.confirm(theirs).unwrap_err(),
        AdmissionError::TokenMismatch
    );
    assert!(
        a.is_locked(),
        "a refused token leaves our own lock in place"
    );
    a.confirm(ours).unwrap();
}
```

- [ ] **Step 2: Run to see it fail.** Expected: compile errors — no method `lock`, `confirm`, `abort`, `is_locked`, `sequence`; no variant `AlreadyLocked`, `DecisionMismatch`, `TokenMismatch`.

- [ ] **Step 3: Replace `admission/mod.rs`** with exactly:

```rust
//! Stage 2c-ii admission: the pure decider (2c-ii design §2).
//!
//! It holds only *descriptors* of the bounded primary intents — generations,
//! ages, turns — and, given a readiness snapshot the conductor supplies,
//! decides which one takes the device slot next. It owns no resource and
//! performs no I/O: resources stay with 2c-i's roles, pools and ledgers.

use std::collections::{BTreeMap, BTreeSet};

mod decide;
mod intents;
mod snapshot;
#[cfg(test)]
mod tests;
mod token;

pub use decide::{AdmissionDecision, Admitted, Tier};
pub use intents::{ComposedIntent, DirectSuccessor, PrimaryOrdinal, QueuedDirect, UnflipBarrier};
pub use snapshot::{IntentKey, Readiness, ReadinessSnapshot, WaitReason};
pub use token::{AdmissionToken, Confirmed};

/// A KMS CRTC object id.
pub type CrtcId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    #[error("generation {offered} is not newer than the queued {queued}")]
    StaleGeneration { queued: u64, offered: u64 },
    #[error("an unflip barrier is pending; direct work cannot supersede it")]
    UnflipPending,
    #[error("an intent must cover at least one CRTC")]
    EmptyCrtcSet,
    #[error("an admission is already locked; confirm or abort its token first")]
    AlreadyLocked,
    #[error("the decision is not what this decider decides now")]
    DecisionMismatch,
    #[error("this token is not the one the decider locked")]
    TokenMismatch,
}

/// The per-device decider. One per DRM device, like the owner's slot.
#[derive(Debug, Default)]
pub struct Admission {
    /// One composed desired state per CRTC (C.0 §9.1).
    composed: BTreeMap<CrtcId, ComposedIntent>,
    /// The grouped-direct unit's latest-wins successor: one per device.
    direct: Option<QueuedDirect>,
    /// The non-supersedable unflip/recovery barrier.
    unflip: Option<UnflipBarrier>,
    /// A waiting topology/lifecycle request, by its generation.
    topology: Option<u64>,
    next_ordinal: u64,
    /// The serial of the locked, not yet confirmed or aborted, admission.
    pending: Option<u64>,
    /// Confirmed admissions so far: the device admission sequence.
    sequence: u64,
}

#[doc(hidden)]
pub fn crtcs(ids: &[CrtcId]) -> BTreeSet<CrtcId> {
    ids.iter().copied().collect()
}
```

- [ ] **Step 4: Write `admission/token.rs`** with exactly:

```rust
//! Two-phase confirmation (2c-ii design §6). The confirmation boundary is
//! the owner's send, not `begin`: `lock` before `begin`, then exactly one of
//! `confirm` (the owner reported `Dispatched`) or `abort` (`begin` or a
//! pre-IPC `send_on` refused). Only `confirm` consumes admission state.

use std::sync::atomic::{AtomicU64, Ordering};

use super::{Admission, AdmissionDecision, AdmissionError, Admitted, ReadinessSnapshot};

/// Lock serials are unique per process, so a token can only ever match the
/// decider that issued it.
static NEXT_LOCK_SERIAL: AtomicU64 = AtomicU64::new(0);

/// A locked admission. Not `Clone`: it is consumed exactly once, by value.
/// Dropping it unconsumed leaves the decider locked — it fails closed.
#[derive(Debug)]
#[must_use = "a locked admission must be confirmed or aborted"]
pub struct AdmissionToken {
    serial: u64,
    decision: AdmissionDecision,
}

impl AdmissionToken {
    pub fn decision(&self) -> &AdmissionDecision {
        &self.decision
    }
}

/// A confirmed admission and its place in the device admission sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    pub decision: AdmissionDecision,
    pub sequence: u64,
}

impl Admission {
    /// Locks `decision` for dispatch. It must be exactly what `decide`
    /// returns for `snapshot` now, so a generation that changed since —
    /// including a direct successor's layout or topology generation — is
    /// caught here, before the owner holds anything (2c-ii design §7).
    /// Consumes no admission state.
    pub fn lock(
        &mut self,
        decision: AdmissionDecision,
        snapshot: &ReadinessSnapshot,
    ) -> Result<AdmissionToken, AdmissionError> {
        if self.pending.is_some() {
            return Err(AdmissionError::AlreadyLocked);
        }
        if self.decide(snapshot).as_ref() != Some(&decision) {
            return Err(AdmissionError::DecisionMismatch);
        }
        let serial = NEXT_LOCK_SERIAL.fetch_add(1, Ordering::Relaxed);
        self.pending = Some(serial);
        Ok(AdmissionToken { serial, decision })
    }

    /// The owner dispatched: `send` returned `Ok`, or `SendError::Ipc`.
    /// Consumes the admitted intent and advances the admission sequence.
    pub fn confirm(&mut self, token: AdmissionToken) -> Result<Confirmed, AdmissionError> {
        if self.pending != Some(token.serial) {
            return Err(AdmissionError::TokenMismatch);
        }
        match &token.decision.admitted {
            Admitted::Topology { .. } => self.topology = None,
            Admitted::Unflip { .. } => self.unflip = None,
            Admitted::Composed { crtc, .. } => {
                self.composed.remove(crtc);
            }
            Admitted::Direct { .. } => self.direct = None,
        }
        self.sequence += 1;
        self.pending = None;
        Ok(Confirmed {
            decision: token.decision,
            sequence: self.sequence,
        })
    }

    /// `begin` refused, or `send_on` refused before any IPC. Nothing is
    /// consumed: the decider is exactly as it was before `lock`.
    pub fn abort(&mut self, token: AdmissionToken) -> Result<(), AdmissionError> {
        if self.pending != Some(token.serial) {
            return Err(AdmissionError::TokenMismatch);
        }
        self.pending = None;
        Ok(())
    }

    pub fn is_locked(&self) -> bool {
        self.pending.is_some()
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}
```

- [ ] **Step 5: Run the gate.** Expected: clippy clean; `c0_adm`: **22 passed, 0 failed**; full `--lib`: 0 failed.

- [ ] **Step 6: Stop dirty and report.**

---

### Task 4: The per-CRTC round-robin and the retirement preference

**Files:**
- Modify: `crates/yserver/src/kms/owner/admission/mod.rs` (replace the whole file)
- Modify: `crates/yserver/src/kms/owner/admission/decide.rs` (replace the whole file)
- Modify: `crates/yserver/src/kms/owner/admission/token.rs` (replace the whole file)
- Modify: `crates/yserver/src/kms/owner/admission/tests.rs` (append)

**Interfaces:**
- Consumes: Task 3's `confirm` (which now records, per CRTC, the sequence of the last admission carrying a primary for it).
- Produces: no new public items. Tier 6 now filters by the round-robin and, on `retirement_wake`, prefers the direct successor when every owed CRTC is inside it.

The rule, from spec §5: a CRTC is **served last** when the immediately previous admission carried a primary for it; a CRTC is **owed** when it has a ready primary and was not served last. A candidate is refused if any of its CRTCs was served last **and** some owed CRTC lies outside it. When every ready CRTC was served last, nobody is owed and nothing is held back — the rule cannot deadlock.

- [ ] **Step 1: Append the tests** to `admission/tests.rs`:

```rust
/// Decide, lock and confirm whatever `snapshot` admits now.
fn admit(a: &mut Admission, s: &ReadinessSnapshot) -> Admitted {
    let d = a.decide(s).expect("something is admissible");
    let token = a.lock(d, s).unwrap();
    a.confirm(token).unwrap().decision.admitted
}

#[test]
fn c0_adm_grouped_then_composed_on_its_crtcs_yields_to_the_owed_crtc() {
    let mut a = Admission::new();
    a.set_direct_successor(successor(1, &[1, 2])).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Direct {
            source_generation: 1,
        },
    );
    admit(&mut a, &s);
    // Composed 1 and 2 are older than composed 3, but the grouped commit
    // served both CRTCs 1 and 2 while CRTC 3 is owed.
    a.set_composed(1, 1).unwrap();
    a.set_composed(2, 1).unwrap();
    a.set_composed(3, 1).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 2,
            generation: 1,
        },
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 3,
            generation: 1,
        },
    );
    assert_eq!(
        admit(&mut a, &s),
        Admitted::Composed {
            crtc: 3,
            generation: 1
        }
    );
}

#[test]
fn c0_adm_composed_then_grouped_yields_to_the_owed_crtc() {
    let mut a = Admission::new();
    a.set_composed(1, 1).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
    );
    admit(&mut a, &s);
    // The grouped successor over {1, 2} is older than composed 3, but it
    // would give CRTC 1 a second successive slot while CRTC 3 is owed.
    a.set_direct_successor(successor(1, &[1, 2])).unwrap();
    a.set_composed(3, 1).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Direct {
            source_generation: 1,
        },
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 3,
            generation: 1,
        },
    );
    assert_eq!(
        admit(&mut a, &s),
        Admitted::Composed {
            crtc: 3,
            generation: 1
        }
    );
    // CRTC 3 took the last slot, so now the grouped successor may go.
    assert_eq!(
        admit(&mut a, &s),
        Admitted::Direct {
            successor: successor(1, &[1, 2])
        }
    );
}

#[test]
fn c0_adm_when_every_ready_crtc_was_just_served_the_oldest_wins() {
    let mut a = Admission::new();
    a.set_direct_successor(successor(1, &[1, 2])).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Direct {
            source_generation: 1,
        },
    );
    admit(&mut a, &s);
    a.set_composed(2, 1).unwrap();
    a.set_composed(1, 1).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 2,
            generation: 1,
        },
    );
    assert_eq!(
        admit(&mut a, &s),
        Admitted::Composed {
            crtc: 2,
            generation: 1
        },
        "nobody is owed, so nothing deadlocks: the oldest goes"
    );
}

#[test]
fn c0_adm_an_intervening_admission_ends_the_successive_run() {
    let mut a = Admission::new();
    a.set_composed(1, 1).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
    );
    admit(&mut a, &s);
    a.request_topology(1).unwrap();
    assert_eq!(admit(&mut a, &s), Admitted::Topology { generation: 1 });
    a.set_composed(1, 2).unwrap();
    a.set_composed(2, 1).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 1,
            generation: 2,
        },
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 2,
            generation: 1,
        },
    );
    assert_eq!(
        admit(&mut a, &s),
        Admitted::Composed {
            crtc: 1,
            generation: 2
        },
        "the topology commit took the last slot, so CRTC 1 is not successive"
    );
}

#[test]
fn c0_adm_retirement_successor_is_preferred_when_no_other_crtc_is_owed() {
    let mut a = Admission::new();
    a.set_composed(1, 1).unwrap();
    a.set_direct_successor(successor(1, &[1, 2])).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
    );
    ready(
        &mut s,
        IntentKey::Direct {
            source_generation: 1,
        },
    );
    assert_eq!(
        a.decide(&s).unwrap().admitted,
        Admitted::Composed {
            crtc: 1,
            generation: 1
        },
        "an ordinary wake takes the oldest"
    );
    s.retirement_wake = true;
    assert_eq!(
        a.decide(&s).unwrap().admitted,
        Admitted::Direct {
            successor: successor(1, &[1, 2])
        },
        "a retirement wake prefers the retirement successor"
    );
}

#[test]
fn c0_adm_retirement_successor_yields_to_an_owed_crtc() {
    let mut a = Admission::new();
    a.set_direct_successor(successor(1, &[1, 2])).unwrap();
    let mut s = snapshot();
    ready(
        &mut s,
        IntentKey::Direct {
            source_generation: 1,
        },
    );
    admit(&mut a, &s);
    a.set_direct_successor(successor(2, &[1, 2])).unwrap();
    a.set_composed(3, 1).unwrap();
    let mut s = snapshot();
    s.retirement_wake = true;
    ready(
        &mut s,
        IntentKey::Direct {
            source_generation: 2,
        },
    );
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 3,
            generation: 1,
        },
    );
    assert_eq!(
        admit(&mut a, &s),
        Admitted::Composed {
            crtc: 3,
            generation: 1
        },
        "CRTC 3 is owed the turn"
    );
}

#[test]
fn c0_adm_a_retirement_successor_stream_cannot_starve_another_crtc() {
    // A direct successor over CRTC 1 is re-queued on every retirement wake
    // (stage 2c §7's continuous direct stream); composed CRTC 2 is ready.
    let mut a = Admission::new();
    a.set_direct_successor(successor(1, &[1])).unwrap();
    a.set_composed(2, 1).unwrap();
    let mut admitted = Vec::new();
    for source_generation in 1..=3 {
        if a.direct().is_none() {
            a.set_direct_successor(successor(source_generation, &[1]))
                .unwrap();
        }
        let queued = a.direct().unwrap().successor.source_generation;
        let mut s = snapshot();
        s.retirement_wake = true;
        ready(
            &mut s,
            IntentKey::Direct {
                source_generation: queued,
            },
        );
        if a.composed(2).is_some() {
            ready(
                &mut s,
                IntentKey::Composed {
                    crtc: 2,
                    generation: 1,
                },
            );
        }
        admitted.push(admit(&mut a, &s));
    }
    assert_eq!(
        admitted[1],
        Admitted::Composed {
            crtc: 2,
            generation: 1
        },
        "CRTC 2 takes the slot right after the stream's first: {admitted:?}"
    );
}

#[test]
fn c0_adm_retirement_preference_needs_no_other_crtc_owed() {
    // The successor is round-robin eligible (the last slot was a topology
    // commit, so nothing is successive), yet composed 3 is ready and owed:
    // the preference does not apply and the oldest goes.
    let mut a = Admission::new();
    a.request_topology(1).unwrap();
    admit(&mut a, &snapshot());
    a.set_composed(3, 1).unwrap();
    a.set_direct_successor(successor(1, &[1])).unwrap();
    let mut s = snapshot();
    s.retirement_wake = true;
    ready(
        &mut s,
        IntentKey::Composed {
            crtc: 3,
            generation: 1,
        },
    );
    ready(
        &mut s,
        IntentKey::Direct {
            source_generation: 1,
        },
    );
    assert_eq!(
        a.decide(&s).unwrap().admitted,
        Admitted::Composed {
            crtc: 3,
            generation: 1
        }
    );
}
```

- [ ] **Step 2: Run to see it fail.** Expected (measured): **26 passed, 4 failed** — `c0_adm_composed_then_grouped_yields_to_the_owed_crtc`, `c0_adm_grouped_then_composed_on_its_crtcs_yields_to_the_owed_crtc`, `c0_adm_retirement_successor_is_preferred_when_no_other_crtc_is_owed`, `c0_adm_retirement_successor_yields_to_an_owed_crtc`. The stream test and `c0_adm_retirement_preference_needs_no_other_crtc_owed` pass here already (ages alone order them without a preference); they exist to catch M10 once the preference is in.

- [ ] **Step 3: Replace `admission/mod.rs`** with exactly:

```rust
//! Stage 2c-ii admission: the pure decider (2c-ii design §2).
//!
//! It holds only *descriptors* of the bounded primary intents — generations,
//! ages, turns — and, given a readiness snapshot the conductor supplies,
//! decides which one takes the device slot next. It owns no resource and
//! performs no I/O: resources stay with 2c-i's roles, pools and ledgers.

use std::collections::{BTreeMap, BTreeSet};

mod decide;
mod intents;
mod snapshot;
#[cfg(test)]
mod tests;
mod token;

pub use decide::{AdmissionDecision, Admitted, Tier};
pub use intents::{ComposedIntent, DirectSuccessor, PrimaryOrdinal, QueuedDirect, UnflipBarrier};
pub use snapshot::{IntentKey, Readiness, ReadinessSnapshot, WaitReason};
pub use token::{AdmissionToken, Confirmed};

/// A KMS CRTC object id.
pub type CrtcId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    #[error("generation {offered} is not newer than the queued {queued}")]
    StaleGeneration { queued: u64, offered: u64 },
    #[error("an unflip barrier is pending; direct work cannot supersede it")]
    UnflipPending,
    #[error("an intent must cover at least one CRTC")]
    EmptyCrtcSet,
    #[error("an admission is already locked; confirm or abort its token first")]
    AlreadyLocked,
    #[error("the decision is not what this decider decides now")]
    DecisionMismatch,
    #[error("this token is not the one the decider locked")]
    TokenMismatch,
}

/// The per-device decider. One per DRM device, like the owner's slot.
#[derive(Debug, Default)]
pub struct Admission {
    /// One composed desired state per CRTC (C.0 §9.1).
    composed: BTreeMap<CrtcId, ComposedIntent>,
    /// The grouped-direct unit's latest-wins successor: one per device.
    direct: Option<QueuedDirect>,
    /// The non-supersedable unflip/recovery barrier.
    unflip: Option<UnflipBarrier>,
    /// A waiting topology/lifecycle request, by its generation.
    topology: Option<u64>,
    next_ordinal: u64,
    /// The serial of the locked, not yet confirmed or aborted, admission.
    pending: Option<u64>,
    /// Confirmed admissions so far: the device admission sequence.
    sequence: u64,
    /// Per CRTC, the sequence of the last admission that carried a primary
    /// for it (2c-ii design §5). Fairness is per CRTC, not per unit.
    last_primary: BTreeMap<CrtcId, u64>,
}

#[doc(hidden)]
pub fn crtcs(ids: &[CrtcId]) -> BTreeSet<CrtcId> {
    ids.iter().copied().collect()
}
```

- [ ] **Step 4: Replace `admission/decide.rs`** with exactly:

```rust
//! The admission function (2c-ii design §5): tiers 1, 2 and 6 of C.0
//! §9.2.1, with the per-CRTC round-robin. Pure — `decide` takes `&self` and
//! changes nothing.

use std::collections::BTreeSet;

use super::{Admission, CrtcId, DirectSuccessor, IntentKey, PrimaryOrdinal, ReadinessSnapshot};

/// C.0 §9.2.1's tier numbers. Plan B adds tiers 3, 4, 5 and 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Topology = 1,
    Unflip = 2,
    Primary = 6,
}

/// What is admitted, with the exact generations `lock` checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admitted {
    Topology { generation: u64 },
    Unflip { crtcs: BTreeSet<CrtcId> },
    Composed { crtc: CrtcId, generation: u64 },
    Direct { successor: DirectSuccessor },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionDecision {
    pub tier: Tier,
    pub admitted: Admitted,
}

impl AdmissionDecision {
    /// The CRTCs this admission carries a primary for. A topology barrier
    /// carries none.
    pub fn primary_crtcs(&self) -> BTreeSet<CrtcId> {
        match &self.admitted {
            Admitted::Topology { .. } => BTreeSet::new(),
            Admitted::Unflip { crtcs } => crtcs.clone(),
            Admitted::Composed { crtc, .. } => BTreeSet::from([*crtc]),
            Admitted::Direct { successor } => successor.crtcs.clone(),
        }
    }
}

/// A ready tier-6 candidate.
struct Candidate {
    ordinal: PrimaryOrdinal,
    crtcs: BTreeSet<CrtcId>,
    admitted: Admitted,
}

impl Admission {
    /// Walks the tiers in order and returns the first that applies.
    pub fn decide(&self, snapshot: &ReadinessSnapshot) -> Option<AdmissionDecision> {
        // Tier 1: a topology barrier is ready whenever it is waiting (2c-ii
        // design §4).
        if let Some(generation) = self.topology {
            return Some(AdmissionDecision {
                tier: Tier::Topology,
                admitted: Admitted::Topology { generation },
            });
        }
        // Tier 2: unflip/recovery, once its exit retirement and composed
        // return path are established.
        if let Some(barrier) = &self.unflip
            && snapshot.is_ready(IntentKey::Unflip)
        {
            return Some(AdmissionDecision {
                tier: Tier::Unflip,
                admitted: Admitted::Unflip {
                    crtcs: barrier.crtcs.clone(),
                },
            });
        }
        // Tier 6: the oldest ready primary the round-robin permits; on a
        // retirement wake the retirement successor first, when no CRTC
        // outside it is owed the turn (C.0 §9.2.1).
        let candidates = self.primary_candidates(snapshot);
        let owed = self.owed_crtcs(&candidates);
        let mut eligible: Vec<Candidate> = candidates
            .into_iter()
            .filter(|candidate| self.round_robin_permits(&candidate.crtcs, &owed))
            .collect();
        if snapshot.retirement_wake
            && let Some(index) = eligible.iter().position(|candidate| {
                matches!(candidate.admitted, Admitted::Direct { .. })
                    && owed.is_subset(&candidate.crtcs)
            })
        {
            return Some(AdmissionDecision {
                tier: Tier::Primary,
                admitted: eligible.swap_remove(index).admitted,
            });
        }
        let winner = eligible
            .into_iter()
            .min_by_key(|candidate| candidate.ordinal)?;
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: winner.admitted,
        })
    }

    /// True if the immediately previous admission carried a primary for
    /// `crtc`: admitting it again now would be a second successive slot.
    fn served_last(&self, crtc: CrtcId) -> bool {
        self.sequence > 0 && self.last_primary.get(&crtc) == Some(&self.sequence)
    }

    /// CRTCs with a ready primary that the previous admission did not serve.
    fn owed_crtcs(&self, candidates: &[Candidate]) -> BTreeSet<CrtcId> {
        candidates
            .iter()
            .flat_map(|candidate| candidate.crtcs.iter().copied())
            .filter(|&crtc| !self.served_last(crtc))
            .collect()
    }

    /// No CRTC takes a second successive slot while another CRTC with a
    /// ready primary is owed service (C.0 §9.2.1). When every ready CRTC was
    /// just served, nobody is owed and nothing is held back.
    fn round_robin_permits(&self, crtcs: &BTreeSet<CrtcId>, owed: &BTreeSet<CrtcId>) -> bool {
        let successive = crtcs.iter().any(|&crtc| self.served_last(crtc));
        let other_owed = owed.iter().any(|crtc| !crtcs.contains(crtc));
        !(successive && other_owed)
    }

    fn primary_candidates(&self, snapshot: &ReadinessSnapshot) -> Vec<Candidate> {
        let barrier_crtcs = self
            .unflip
            .as_ref()
            .map(|barrier| barrier.crtcs.clone())
            .unwrap_or_default();
        let mut candidates = Vec::new();
        for (&crtc, intent) in &self.composed {
            // A pending barrier carries these CRTCs' composed return; a later
            // composed intent does not overtake it (C.0 §9.1).
            if barrier_crtcs.contains(&crtc) {
                continue;
            }
            if snapshot.is_ready(IntentKey::Composed {
                crtc,
                generation: intent.generation,
            }) {
                candidates.push(Candidate {
                    ordinal: intent.ordinal,
                    crtcs: BTreeSet::from([crtc]),
                    admitted: Admitted::Composed {
                        crtc,
                        generation: intent.generation,
                    },
                });
            }
        }
        if let Some(queued) = &self.direct {
            let successor = &queued.successor;
            // Direct eligibility is re-proven on every admission, including
            // retirement promotion (stage 2c, v1.5.0 table).
            let current = successor.layout_generation == snapshot.layout_generation
                && successor.topology_generation == snapshot.topology_generation;
            if current
                && snapshot.is_ready(IntentKey::Direct {
                    source_generation: successor.source_generation,
                })
            {
                candidates.push(Candidate {
                    ordinal: queued.ordinal,
                    crtcs: successor.crtcs.clone(),
                    admitted: Admitted::Direct {
                        successor: successor.clone(),
                    },
                });
            }
        }
        candidates
    }
}
```

- [ ] **Step 5: Replace `admission/token.rs`** with exactly:

```rust
//! Two-phase confirmation (2c-ii design §6). The confirmation boundary is
//! the owner's send, not `begin`: `lock` before `begin`, then exactly one of
//! `confirm` (the owner reported `Dispatched`) or `abort` (`begin` or a
//! pre-IPC `send_on` refused). Only `confirm` consumes admission state.

use std::sync::atomic::{AtomicU64, Ordering};

use super::{Admission, AdmissionDecision, AdmissionError, Admitted, ReadinessSnapshot};

/// Lock serials are unique per process, so a token can only ever match the
/// decider that issued it.
static NEXT_LOCK_SERIAL: AtomicU64 = AtomicU64::new(0);

/// A locked admission. Not `Clone`: it is consumed exactly once, by value.
/// Dropping it unconsumed leaves the decider locked — it fails closed.
#[derive(Debug)]
#[must_use = "a locked admission must be confirmed or aborted"]
pub struct AdmissionToken {
    serial: u64,
    decision: AdmissionDecision,
}

impl AdmissionToken {
    pub fn decision(&self) -> &AdmissionDecision {
        &self.decision
    }
}

/// A confirmed admission and its place in the device admission sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    pub decision: AdmissionDecision,
    pub sequence: u64,
}

impl Admission {
    /// Locks `decision` for dispatch. It must be exactly what `decide`
    /// returns for `snapshot` now, so a generation that changed since —
    /// including a direct successor's layout or topology generation — is
    /// caught here, before the owner holds anything (2c-ii design §7).
    /// Consumes no admission state.
    pub fn lock(
        &mut self,
        decision: AdmissionDecision,
        snapshot: &ReadinessSnapshot,
    ) -> Result<AdmissionToken, AdmissionError> {
        if self.pending.is_some() {
            return Err(AdmissionError::AlreadyLocked);
        }
        if self.decide(snapshot).as_ref() != Some(&decision) {
            return Err(AdmissionError::DecisionMismatch);
        }
        let serial = NEXT_LOCK_SERIAL.fetch_add(1, Ordering::Relaxed);
        self.pending = Some(serial);
        Ok(AdmissionToken { serial, decision })
    }

    /// The owner dispatched: `send` returned `Ok`, or `SendError::Ipc`.
    /// Consumes the admitted intent and advances the admission sequence.
    pub fn confirm(&mut self, token: AdmissionToken) -> Result<Confirmed, AdmissionError> {
        if self.pending != Some(token.serial) {
            return Err(AdmissionError::TokenMismatch);
        }
        match &token.decision.admitted {
            Admitted::Topology { .. } => self.topology = None,
            Admitted::Unflip { .. } => self.unflip = None,
            Admitted::Composed { crtc, .. } => {
                self.composed.remove(crtc);
            }
            Admitted::Direct { .. } => self.direct = None,
        }
        self.sequence += 1;
        // A multi-CRTC admission serves every CRTC it covers.
        for crtc in token.decision.primary_crtcs() {
            self.last_primary.insert(crtc, self.sequence);
        }
        self.pending = None;
        Ok(Confirmed {
            decision: token.decision,
            sequence: self.sequence,
        })
    }

    /// `begin` refused, or `send_on` refused before any IPC. Nothing is
    /// consumed: the decider is exactly as it was before `lock`.
    pub fn abort(&mut self, token: AdmissionToken) -> Result<(), AdmissionError> {
        if self.pending != Some(token.serial) {
            return Err(AdmissionError::TokenMismatch);
        }
        self.pending = None;
        Ok(())
    }

    pub fn is_locked(&self) -> bool {
        self.pending.is_some()
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}
```

- [ ] **Step 6: Run the gate, including Task 4's additions.** Expected: clippy clean in all three configurations; `cargo check` clean on the three targets; `c0_adm`: **30 passed, 0 failed**; full `--lib`: **0 failed** (the coordinator measured 1808 passed, 95 ignored).

- [ ] **Step 7: Stop dirty and report.**

---

## What the coordinator does after each task

1. Mechanical copy check: every file the task wrote equals its plan block after `cargo +nightly fmt` (a python extraction of the fenced blocks, as in 2c-i debt session 1).
2. Rerun the task's gate.
3. After Task 4: run the twelve mutations of the table above against the implemented tree, each by exact line, confirming each run compiled.
4. Commit each task with `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)` and `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
