# 2c-iii addendum plan — codex review, round 2

**Target:** revision 2 (`4412889f`) of
`docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md`.

**Result:** 1 blocking, 3 major, 0 minor; coverage INCOMPLETE (24/24).
Incorporation: round-1 B-1 and M-1 TRADED, M-2 PARTIAL.

**Reviewer:** `codex exec --sandbox read-only`, single pass; instrument
`docs/superpowers/review/` @ `0245f96b`; `gpt-6-sol` `xhigh`; `codex-cli 0.155.1`.

**Author verification (2026-09-23):** all four CONFIRMED — `hold_direct` set
at `backend.rs:21487` stops the tick before `scene.tick` at `:22529` (B-1);
a new source generation per offer (`admission.rs:660`) restarts the bootstrap
(M-1); `can_enter_direct` needs the one global ordinary-retirement slot
(`resources/capacity.rs:188`) (M-2); the A3 row referred to the wrong test
(M-3). Also verified: direct is eligible only when every output is on the
primary device (`backend.rs:3877`), so round 1's B-1 is unreachable today.

**Disposition — rewritten.** Every revision-2 change traded because the gate
sat in readiness, after a direct frame was prepared. Revision 3 moves it to the
shared eligibility predicate, before preparation: a Present with no return is
simply ineligible and its composition establishes the return. I-0 and I-4 are
gone; `has_current_direct` is recorded as latent in the finding.

---

## Verdict

**1 blocking, 3 major, 0 minor**  
**Coverage: INCOMPLETE**

This is a design review, not a claim that the plan compiles, its tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Revision 2 |
| --- | --- | --- |
| B-1 — device-blind current-direct check | **TRADED** | I-0 scopes the check to the device and adds A4, but applying it to the `ordinary_retirement` readiness branch removes a guard for a globally shared capacity role. See M-2. |
| M-1 — no way to produce a composed return | **TRADED** | I-4 requests full damage, but the queued direct entry holds scene composition while it waits. See B-1 and M-1. |
| M-2 — A3 could pass without testing the intermediate state | **PARTIAL** | A3 now describes that state, but assigns it to “the third test,” which is the unrelated two-device test, and permits A3 to remain unproved. See M-3. |

## Findings

### Blocking

**B-1 — The requested composed return cannot pass the normal scene tick.** [Plan I-4](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:37>) says full-frame invalidation will lead to ordinary composed admission. Preparing the queued direct frame sets `hold_direct` ([backend.rs:21487](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:21487>)); the active-direct tick returns before `scene.tick` while that hold is set ([backend.rs:22529](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:22529>)). Invalidation can make the scene dirty, but it does not itself offer a composed intent. A ready direct entry can therefore wait while its return is never produced, leaving the entry and the unflip precondition stranded. The spec requires a retained framebuffer on every affected output for unflip ([spec §6.1](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:481>)). Define the waiting-entry handoff through the normal tick: allow its composed bootstrap frame to be prepared and admitted while preserving the queued direct frame and the existing protection against racing a *submitted* direct transaction.

### Major

**M-1 — “Once per entry generation” can restart the bootstrap indefinitely.** Each direct offer receives a new source generation ([admission.rs:660](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:660>)), and a new queued frame replaces the old one ([backend.rs:21487](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:21487>)). Under continuous eligible Presents, I-4’s rule permits each replacement to request another full repaint before any composed frame retires ([plan I-4](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:37>)). A6 checks only a second wake of the *same* entry; it cannot establish the claimed one-frame bound across replacement. Keep one outstanding bootstrap request per device until a composed return retires or the wait is cancelled, and test replacements during that request.

**M-2 — Device scoping removes a global retirement-capacity guard.** I-0 converts the `ordinary_retirement` caller along with the entry check ([plan I-0](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:24>)). That role is a single capacity slot: `can_enter_direct` requires it vacant ([capacity.rs:188](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/capacity.rs:188>)). If A has current direct and occupies ordinary retirement, while B has a return and a ready entry, the old snapshot waits; the proposed device-local `current_direct` makes B’s snapshot report ready despite the occupied global role ([admission.rs:831](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:831>), [admission.rs:962](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:962>)). Preserve the global capacity precondition separately from the device-local successor distinction, and test this cross-device state. A4, with B lacking a return, cannot detect it.

**M-3 — A3 has no named test that must prove it.** The table’s third test is `direct_elsewhere`, which varies devices, while A3 requires two outputs on one device with only the first return established ([plan tests and A3](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:59>)). The mutation instructions refer to that “third test” and allow A3 to be reported unproved ([plan:71](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2c-iii-addendum-direct-entry-needs-return.md:71>)). Give the intermediate state its own named production-entry test and make the mutation’s result explicit; if the state is unreachable, report this invariant as unproved rather than counting A3 among passing evidence. The spec calls for named tests and mutations that break them ([spec §8.2](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:576>)).

### Minor

None.

## Coverage and implementation checks

**24/24 bounded excerpts used.** One source excerpt inadvertently exceeded the 120-line limit; coverage is therefore incomplete. I stopped investigation at the budget. Architecture and event-flow checks established that waiting direct is excluded from ready primary candidates, so a composed intent *would* be admissible; scene invalidation is device scoped and marks structural damage, but the tick hold obstructs producing that intent. Safety and ownership checks covered device identity, queued-frame replacement, and global retirement capacity. Spec and verification checks covered the retained-return requirement, multi-device isolation, and named mutation evidence.

I did not trace the `active()` predicate, direct-dispatch refusal recovery, or every affected Cii/Cfb fixture. Those paths and the proposed tests remain unverified. The implementation must use the real compiler and tests for fixture reachability and mutations, and run the assigned formatting, `cargo clippy --all-targets -- -D warnings`, and test gates. No build or test was run here.
