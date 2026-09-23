# 2c-iii addendum plan — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md`
revision 1 (`5947ab6e`).

**Result:** 1 blocking, 2 major, 0 minor; coverage INCOMPLETE (24/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.

**Author verification (2026-09-23):**

- **B-1 — CONFIRMED, and wider than the addendum.** `has_current_direct`
  (`admission.rs:2368`) scans every `current_resources` entry with no device
  filter, although each carries its `CommitKey`; it also gates
  `ordinary_retirement` (`:962`). A device-blind site of the family plan
  Ciii-identity closed. Fixed: I-0 makes it per device, every caller converted.
- **M-1 — CONFIRMED.** Nothing produced a composed frame for a waiting entry.
  Fixed: I-4, the waiting entry requests one full composed frame once per
  entry generation.
- **M-2 — CONFIRMED.** Both outputs moving together cannot distinguish A3.
  Fixed: A3 requires the first-established/second-absent intermediate state,
  or is reported unproved.

---

## Verdict

**1 blocking, 2 major, 0 minor**  
**Coverage: INCOMPLETE**

This is a design review, not a claim that the plan compiles, its tests pass, or implementation is approved.

## Incorporation audit

| Prior review | Status |
| --- | --- |
| None; this is the first review. | Check skipped. |

## Findings

### Blocking

**B-1 — A direct frame on another device can bypass the entry gate.** The plan defines entry and successor status per device but uses the existing `has_current_direct` predicate ([plan, lines 15–17](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:15)). That predicate searches *all* current resources without checking device identity ([source, lines 2368–2378](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:2368)); the composed-return predicate is device-scoped ([source, lines 844–873](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:844)). With A current in direct and B holding an eligible, ready direct entry but no composed return, A makes `current_direct` true and exempts B. This violates the plan’s invariant and the spec’s per-device independence requirement ([spec, lines 496–500](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:496)). Scope the current-direct check to the candidate’s device and test A-current/B-entry. The real producer’s present topology restriction currently masks this mixed-device sequence; it does not make the admission contract device-local.

### Major

**M-1 — Waiting has no bootstrap contract for a device that never composes.** The plan makes direct entry wait for a retained return, then has its test explicitly dispatch a composed frame to release the wait ([plan, lines 15 and 35](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:15)). If an Owner device receives only direct work and has no composed intent, repeated wakes leave the direct entry waiting indefinitely; nothing in the addendum causes a return to be produced. Waiting is necessary for the spec’s unflip precondition ([spec, lines 481–485](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:481)), but indefinite loss of direct admission conflicts with C.0’s preserved direct-scanout outcome ([C.0, lines 2386–2405](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2386)). Specify how waiting triggers or obtains a composed return, or make that capability a prerequisite for Owner admission. Test the no-preexisting-composed-work sequence.

**M-2 — The A3 mutation can pass the named test.** The proposed two-output variant waits before *any* composed frame retires, then admits after a composed frame retires ([plan, lines 35–41](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:35)). A check of only the first output gives the same results if both outputs initially lack returns and both later have them. A multi-output live fixture exists ([source, lines 53826–53858](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:53826)), but output count alone cannot distinguish the mutation. Require the intermediate state **first output established, second absent**, assert that direct still waits, then establish the second return and assert admission. If production composed entries cannot create that state, report A3 as unproved rather than passed.

### Minor

None.

## Coverage and implementation checks

Architecture, safety, and spec checks covered the device scope, successor exemption, return lifetime described in the defect record, and direct-entry liveness. The all-output return scope matches the current whole-output direct topology; exempting a successor is justified while its same-device return remains retained. The plan assigns formatting, clippy, and test gates to implementation ([plan, line 43](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:43)). No build or test was run.

**Excerpts: 24/24.** One 140-line spec read exceeded the 120-line per-excerpt limit; no further investigation was done. The unresolved, narrow fixture question is whether every affected non-Vulkan Cii/Ciii/Cfb/Cp and `c0_adm` direct test can establish a retained return through production entries without weakening assertions. Fixture feasibility and test results are unverified; the real compiler and tests must check them during implementation.
