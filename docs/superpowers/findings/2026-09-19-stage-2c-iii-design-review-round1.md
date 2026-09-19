# Stage 2c-iii design — codex review, round 1

**Target:** `docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md`
revision 1 (`c80e55d9`), a stage **design**, against the stage-2c design as the
passed parent, with C.0, 2c-i (+ debt) and 2c-ii named in the context.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `69c6d6e2`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-19), every finding checked against the tree:**

- **B-1 — CONFIRMED.** Revision 1's §4.2 row grouped `Dispatched` with
  `FailedBeforeSubmit` and closed the transaction, yet the transaction is created
  at confirmation (= `Dispatched`) and must survive to `Accepted` and
  `HardwareComplete`. Fixed: the row is split; `Dispatched` retains it.
- **B-2 — CONFIRMED.** §6.4 let P3-2/P3-3 fall back to the debt spec's F8 if no
  non-test caller appeared, contradicting §3.2's "must register". Fixed: the
  caller on the converted `Owner` submit path is an acceptance condition of C1,
  C2, C3 and the stage; its absence stops the stage, it is not deferred.
- **M-1 — CONFIRMED.** `grep` finds neither `present_consumers` nor
  `page_flip_event` in `kms/render/admission.rs`; the conductor builds no
  Present-carrying description, and revision 1 never required the producers to
  carry them. Fixed: new §3.3 (Present carriage), evidence in §4.5/§5.7, row in §8.2.
- **M-2 — CONFIRMED.** Three release gates, one mutation. Fixed: §8.2 row with
  a mutation per gate.
- **M-3 — CONFIRMED.** DMG-5's unflip half had no mutation and no C3 evidence.
  Fixed: §6.1 and a §8.2 row.

Revision 2 incorporates all five.

---

## Verdict

**2 blocking, 3 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a bounded design review, not a claim that the code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| None | N/A | This is the first review; check 1 was skipped as instructed. |

## Findings

### Blocking

#### B-1 — `Dispatched` incorrectly closes the damage transaction

The target groups `Dispatched`, `FailedBeforeSubmit`, and pre-IPC refusal together and says “the transaction closes” ([target lines 229–236](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:229)). This contradicts both authoritative mappings: submission has no damage action, while the transaction must remain available for the later `Accepted` and `HardwareComplete` milestones ([stage-2c lines 152–166](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:152), [C.0 lines 2473–2482](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2473)). `Dispatched` is explicitly nonterminal: it precedes `Accepted` ([C.0 lines 2118–2134](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2118)).

Concrete failure: dispatch closes and drops the captured transaction; successful acceptance can no longer stage it, and hardware completion cannot apply or acknowledge its snapshots.

Required correction: split the row. `Dispatched`/`Submitting` performs no action **and retains the transaction**. Only proven pre-accept refusal or `FailedBeforeSubmit` closes it without staging.

#### B-2 — The design permits 2c-iii to finish without the production registration caller it requires

Section 3.2 correctly makes old/new dependency registration part of dispatch and identifies its absence as the P3-2/P3-3 failure ([target lines 118–130](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:118)). Section 6.4 then weakens that requirement: if C1/C2 still provide no non-test caller, P3-2/P3-3 may remain an F8 item for stages 3/4 ([target lines 362–368](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:362)).

That contradicts the passed stage-2c deliverable: 2c-iii supplies converted producer paths with real ownership transitions and may not defer resource release back to the legacy shortcut ([stage-2c lines 79–87](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:79)). The authoritative lifecycle requires registration before dispatch and release only from real completion evidence ([stage-2c lines 104–117](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:104)).

Concrete failure: fixture-only registration passes owner tests, C3 discovers no production caller, yet the stage is allowed to hand the debt forward. Later activation can dispatch records with empty `kms_obligations`.

Required correction: absence of a non-test caller on the exact real-producer path must block C1/C2/C3 and 2c-iii completion; it cannot be deferred to stages 3/4.

### Major

#### M-1 — The new owner entry has no corresponding producer-to-owner Present contract

The proposed public, fallible context entry correctly closes the current owner API gap ([target lines 151–168](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:151)); the inspected code confirms that the public context entry currently takes a ledger by value, while the CommitId-aware form is private and infallible ([device.rs lines 1322–1379](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1322)).

However, the composed and direct producer contracts never require their prepared intents/builders to preserve `page_flip_event`, Present consumers, serial/FIFO identity, and completion context into that entry ([target lines 206–214](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:206), [lines 288–304](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:288)). Evidence tests only entry validation, not real producer carriage or independent completion ordering.

Concrete failure: a builder drops `present_consumers` while damage and resource ledgers remain correct. All listed damage/release tests can pass, but no `Presented` result reaches the protocol ledger and the client FIFO remains parked. This violates independent Present terminalization ([C.0 lines 2136–2148](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2136), [lines 2258–2285](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2258)).

Required correction: define the producer-to-description Present fields for composed and direct commits, and add real-producer tests for both event orders, missing Present, dropped consumer metadata, and no early idle/release.

#### M-2 — Pool-release evidence does not prove the stated release gates

The target correctly says composed pool reuse requires the ledger’s retirement/replacement milestones and the GPU fence gate ([target lines 238–240](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:238)). Its sole mutation releases at `HardwareComplete` ([target line 406](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:406)).

An implementation that waits until `CompletionRetired` but reuses the pool slot before `PriorBufferReleased` or GPU completion would defeat that mutation while violating the authoritative release boundary ([stage-2c lines 113–117](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:113)).

Required correction: test each gate independently—`CompletionRetired` alone, `PriorBufferReleased` without GPU completion, and final release only after all required evidence.

#### M-3 — DMG-5 verification covers direct entry but not unflip return

The design requires invalidation both on direct entry and composed unflip ([target lines 308–309](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:308), [C.0 lines 2509–2515](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2509)). Yet the exit mutation removes only direct-entry invalidation ([target line 403](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:403)); C3’s hardware sequence does not assert buffer invalidation.

Required correction: add a C3 unflip-specific fixture and mutation proving that every affected composed buffer is invalidated and repainted on return.

### Minor

None.

## Coverage and implementation checks

- Incorporation: skipped; no prior review.
- Architecture/contracts: owner-entry gap, production reachability, producer Present carriage, and route split assessed.
- Safety/ownership: damage lifetime and composed pool release ordering assessed.
- Compliance/evidence: DMG-1–5 and Present/release independence checked against binding clauses.
- Excerpts used: **12/12**.
- Verified source ground: owner context/ledger entries, registration rollback shape, and device-keyed owner-event routing.
- Unassessed—not deemed sound: exact direct/unflip preparation side effects, whether an existing terminal state expresses post-accept restore, and lifecycle/recovery behavior assigned to later stages.
- Deferred to implementation: Rust signatures/borrows, test compilation, formatting, clippy, target portability checks, deterministic suites, and hardware execution.