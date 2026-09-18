## Verdict

**1 blocking, 3 major, 1 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** `docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a1-decider.md`
revision 2 (`d9cb85ad`), against spec revision 3.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `69c6d6e2`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.154.0`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Recorded usage:** 59,123 tokens reported by the completed process (exit 0),
excluding the author session. 10/12 bounded excerpts used.

**Author verification (2026-09-18):**

- **B-1 — CONFIRMED.** Spec §5 says no covered CRTC takes a second successive slot
  while *another* CRTC is owed; the plan's "owed CRTC outside it" exempts B inside
  a grouped `{A, B}`. The reviewer's correction (drop "outside") is necessary but
  not sufficient: if the grouped candidate is the only one covering B, B would
  count as owed and the grouped candidate would be refused forever, which is a
  deadlock. Revision 3 defines "owed" as covered by a ready candidate with no
  CRTC served last, and adds a test for that deadlock case.
- **M-1 — CONFIRMED.** Only composed confirmation was tested. Added direct,
  unflip and topology, plus a multi-CRTC unflip served-marking test.
- **M-2 — CONFIRMED.** Added the foreign-token `abort` test and M16.
- **M-3 — CONFIRMED.** Added the widening test and M17.
- **m-1 — CONFIRMED.** The scenario now fixes composed 1 as older.

This is a design-review result only; it does not claim that the future implementation compiles, passes tests, or is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| None | N/A | This is the first completed review. The earlier revision’s review was stopped without findings, as recorded at [plan line 5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a1-decider.md:5). Check 1 is skipped. |

## Findings

### Blocking

#### B-1 — The round-robin predicate improperly exempts owed CRTCs inside a grouped candidate

The plan refuses a tier-6 candidate only when an owed CRTC lies **outside** its CRTC set ([plan lines 321–324](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a1-decider.md:321)). The spec instead says that no covered CRTC may take a second successive slot while **another CRTC** is owed, explicitly denying grouped commits an exception merely because they include that owed CRTC ([spec lines 224–235](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:224), [C.0 lines 1606–1608](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1606)).

Concrete failure:

1. Confirm composed primary A.
2. Queue an older grouped direct candidate `{A,B}` and a newer composed B; both are ready.
3. B is owed and A was served last.
4. Because B is inside `{A,B}`, the plan permits the grouped candidate; oldest-first selects it, giving A two successive slots.
5. The spec requires B alone to win first.

The existing composed→grouped test uses an owed CRTC 3 outside `{1,2}`, so this defective predicate passes every named test.

Smallest correction: remove the “lies outside it” qualification from ordinary round-robin eligibility. Add a test for `A → grouped {A,B}` with composed B ready, plus a mutation that restores the inside-group exemption.

### Major

#### M-1 — Confirmation tests do not prove exact consumption for direct and unflip decisions

`confirm` must remove exactly the admitted intent ([plan lines 285–293](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a1-decider.md:285); [spec lines 267–293](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:267)). The only explicit consumption test confirms a composed intent ([plan lines 297–303](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a1-decider.md:297)).

An implementation can update fairness for a confirmed direct successor but leave its slot occupied; later snapshots can re-admit a generation whose resources were already moved or terminalized. Similarly, an unflip confirmation can leave its barrier permanently pending, or fail to mark every affected CRTC served, without any named test failing.

Add confirmation scenarios for direct, unflip, and topology that assert the exact slot is empty and unrelated slots remain. The multi-CRTC unflip case should also prove every covered CRTC becomes served.

#### M-2 — `abort` token ownership is specified but untested

The plan requires foreign tokens to produce `TokenMismatch` and leave the receiving decider’s lock intact ([plan line 292](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a1-decider.md:292)), but tests this only through `confirm` ([plan line 303](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a1-decider.md:303)).

A faulty `abort` that ignores token identity can unlock decider A using decider B’s token. A may then issue another admission while its original token remains outstanding; B stays locked when its consumed token is dropped. Compilation and all named tests can still succeed.

Add the symmetric foreign-token `abort` scenario and a mutation that omits abort-side identity validation.

#### M-3 — Non-replaceable unflip widening has no verification

The spec makes the unflip/recovery barrier non-replaceable ([spec lines 108–115](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:108)); the plan translates this into widening an existing barrier ([plan lines 140–145](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a1-decider.md:140)). No named test calls `request_unflip` twice.

An implementation that overwrites `{1}` with `{2}` loses CRTC 1’s restoration requirement while satisfying every listed test. Add a `{1}` then `{2}` scenario asserting the stored barrier is `{1,2}`, with an overwrite mutation.

### Minor

#### m-1 — The M4 scenario does not fix ordinal order

The barrier-suppression test says composed 2 wins over composed 1, but does not state which was queued first ([plan line 244](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a1-decider.md:244)). If composed 2 is already older, mutation M4—allowing composed 1 to compete despite its barrier—can survive.

Specify that composed 1 receives the lower ordinal before composed 2. Then only barrier suppression can make composed 2 win.

## Coverage and implementation checks

- Incorporation: skipped because no prior findings exist.
- Architecture/contracts: checked decider/conductor ownership, exact-generation readiness, A2/B boundaries, per-CRTC fairness, and owner dispatch integration.
- Safety/ownership: checked token authority, lock/confirm/abort state transitions, descriptor-stability assumptions, send-boundary semantics, and multi-CRTC accounting.
- Compliance/verification: checked all A1 exit rows and M1–M12 against their named scenarios.

Used **10/12 bounded excerpts**: authoritative spec §§1–7 and §§10.1–10.4, C.0 §9.2.1, stage-2c §4, and the existing owner’s `begin_with_context`/`send_on` paths. The latter confirm that `begin` installs live state while pre-IPC `send_on` refusals retire it, and `SendError::Ipc` is treated as dispatched.

Unassessed by scope: plan-B maintenance/receipt behavior, plan-A2 conductor implementation, producer conversion, hardware behavior, and unrelated source. Compilation, signatures, borrow behavior, formatting, clippy, target checks, feature checks, and mutation execution remain deferred to implementation.