## Verdict

**1 blocking, 2 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result, not a claim that code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Audit |
|---|---|---|
| B-1 — Missing `Presented` lacked a valid fallback-clock contract | **TRADED** | Revision 3 correctly separates commit-bound `Flip` data from per-CRTC historical `Skip` data, requires fallible lookup, and orders correlation before clock mutation ([plan lines 29–33](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:29)). It then introduces a new non-terminal outcome when history is absent: close the transport and publish nothing. That contradicts mandatory Present terminalization. See B-1. |
| M-1 — Task 1 contracts absent from mutation accounting | **PARTIAL** | S28–S30 were added ([plan lines 79–81](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:79)), and S28/S29 have matching assertions in the real-dispatch test. S30 is assigned to a test whose task description never exercises `begin_with_ledger`’s refusal. See M-1. |

## Findings

### Blocking

#### B-1 — Closing the transport does not terminalize a no-history Present

The specification requires an accepted Present without validated presentation to become `Skip` using the last validated CRTC clock, with no fabricated timestamp ([spec lines 256–261](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:256)). C.0 additionally requires every Present to reach a protocol terminal state and the client FIFO to be unparked ([C.0 lines 2258–2274](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2258)).

Decision 6 instead says that if no historical sample exists, the route closes the transport and lets an unspecified drain terminalize the work; its test explicitly accepts “the transport closes and nothing is published with `(0, 0)`” ([plan lines 31–33](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:31), [line 188](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:188)). The existing `force_close` path only marks the gate closed; it contains no Present drain or terminalization ([transport.rs lines 367–376](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/transport.rs:367)).

Concrete failure: the first accepted direct Present in a CRTC clock epoch loses its `Presented` event before any completion clock has been recorded. At retirement, the plan cannot produce the required `Skip`, closes the gate, and leaves the request/FIFO unterminated. Closing future admission does not discharge this protocol obligation.

Smallest correction: establish before acceptance that every reference CRTC has a validated epoch-local sample—obtaining one through the validated sequence path or refusing/parking admission until it exists—and test that precondition. If accepted work can still reach teardown without any sample, the authoritative specification needs an explicit terminalization rule; “close and publish nothing” cannot be the implementation rule. Resolve reachability during planning, not merely report it after implementation.

### Major

#### M-1 — S30 remains unkillable by its assigned Task 1 test

The exit table assigns S30—allowing Present-bearing descriptions through `begin_with_ledger`—to `c0_conv_cii_present_entry_registers_and_refuses` ([plan line 81](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:81)). But Task 1 defines that test as accepting through the new entry and refusing an invalid `CompletionContext` before its ledger runs; it never requires calling the old `begin_with_ledger` entry with a Present-bearing description ([plan lines 111–114](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:111)).

Concrete failure: remove the old entry’s `page_flip_event`/`present_consumers` refusal. Both described branches of the named test still pass, so S30 survives despite the spec requiring that mutation to fail ([spec lines 586–590](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:586)).

Smallest correction: add an explicit third case to that named test which calls `begin_with_ledger` with an otherwise-valid Present-bearing description and asserts refusal with no closure execution or owner mutation.

#### M-2 — Flip-sample identity lacks evidence for both correlation dimensions

Decision 6 requires the `Flip` sample to match both `CommitId` and reference CRTC ([plan line 30](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:30)). The frame already records a distinguished completion output rather than accepting an arbitrary grouped output ([backend.rs lines 595–600](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:595)).

Nevertheless, Task 6 describes `direct_present_completes_once_vulkan` only as covering both milestone orders ([plan line 188](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:188)). It does not require a foreign-commit sample or distinct same-commit samples from reference and non-reference CRTCs. S25 is nevertheless assigned solely to that test ([plan line 91](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:91)); S17 covers the wrong CRTC only for historical `Skip`, not `Flip`.

Concrete failure: a grouped commit receives `reference=SA` and `other=SB`; an implementation stores the first/last map entry and publishes `Flip(SB)`. All presently specified Task 6 scenarios can pass.

Smallest correction: make the Flip test inject distinct samples for both CRTCs and a stale/foreign commit, then assert that only the current commit’s reference sample produces `Flip`. Add a mutation selecting the non-reference CRTC and assign S25 to evidence that explicitly injects the foreign commit.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** audited both round-2 findings against revision 3’s task and exit-table text.
- **Architecture/contracts:** checked frame authority, commit/CRTC correlation, clock-store ownership, event routing, retirement publication, transport closure, and task dependencies.
- **Safety/failure semantics:** checked registration rollback, pin/frame ownership, stale-event ordering, missing-sample terminalization, and FIFO consequences.
- **Spec/verification:** checked relevant §§3.2–3.4, 5.0–5.7, 8.1–8.3 and C.0 §§6.1, 10.2, 10.4; audited S1–S30 ownership and the revision-specific mutations.

Used **24/24 bounded excerpt units**. Unassessed—and not asserted sound—are the eventual layout-site enumeration, detailed resource-service internals, full activation-time clock seeding, fixture code beyond the bounded range, and cursor internals. Exact Rust signatures, compilation, formatting, clippy, portability, GPU execution, and actual mutation execution remain deferred to implementation.