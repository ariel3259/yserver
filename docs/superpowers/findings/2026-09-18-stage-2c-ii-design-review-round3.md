## Verdict

**1 blocking, 1 major, 1 minor**

Coverage: **COMPLETE FOR DECLARED SCOPE**

**Target:** spec revision 3 (+ `53f5fc2e`, `1f8f3482` status lines), scoped to
section 7's receipt, section 11.1 and the C.0 §9.2.1 amendment, with prior round 2.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `69c6d6e2`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.154.0`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Recorded usage:** 66,463 tokens (exit 0). 12/12 excerpts.

**Author verification (2026-09-18):**

- **B-1 — CONFIRMED.** With a per-generation count and "newer payload, older
  ticket, fresh count", one identity rejected once per generation keeps winning
  tier 4 on its old ticket; even without collisions a rejected older identity is
  admitted twice, beyond C.0's `N - 1`. Put to the user as a change to their
  11.1 decision; **the user chose counting per identity** (inherited on
  collision, reset only by `Completed`), with the bound amended to
  `1 + 2(N - 1)` in the spec and in C.0 (§9.2.1 and its two restatements).
- **M-1 — CONFIRMED.** The receipt carried generation numbers only; nothing
  owned the payloads. Revision 4 adds a conductor-owned maintenance store
  (desired / submitted / current per `(CRTC, class)`), and defines the payload's
  path for each terminal outcome, including the dormant handoff to C.0 §10
  recovery on `CompletionUnknown`.
- **m-1 — CONFIRMED.** A1's `Confirmed { decision, sequence }` has no
  `CommitId` (`token.rs`), rightly. The conductor builds the receipt from
  `(CommitId, Confirmed)` and intercepts the commit's `Terminal` event.

This is a design-review result only; it does not establish compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1 — Tier 5 bypasses per-CRTC fairness | **APPLIED** | Tier 5 now requires the per-CRTC round-robin check, with the singular-then-bundle case explicitly specified and tested ([target lines 190–198, 224–235](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:190)). |
| B-2 — Queued direct work lacks eligibility identity | **APPLIED** | Direct descriptors now carry layout/eligibility generation; readiness rechecks eligibility; geometry/layout changes wake admission; invalid successors are terminalized; `lock` revalidates generations ([target lines 108–176, 279–285](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:108)). |
| B-3 — No owner retains post-dispatch admission state | **PARTIAL** | The conductor now owns commit-keyed receipt metadata and post-drop behavior is specified. However, the retry ordering contradicts the starvation bound (B-1), and no owner is assigned the actual maintenance payload/resources needed by `Completed`, rejection re-entry, or unknown recovery (M-1). |
| M-1 — No comparable primary age | **APPLIED** | `PrimaryOrdinal` is device-monotonic, total across shapes, survives replacement and waiting, and is released on admission/withdrawal/terminalization ([target lines 130–138](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:130)). |

## Findings

### Blocking

#### B-1 — Reusing the older ticket makes the stated starvation bound false

C.0 permits an aged identity to wait behind at most `N - 1` older-ticket maintenance admissions ([C.0 lines 1579–1589](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1579)). The amendment and target instead reinsert a rejected generation with its original ticket; a collision keeps that older ticket while resetting the rejection count for the newer payload ([C.0 lines 1591–1604](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1591), [target lines 343–361](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:343)).

Concrete sequence: aged A has ticket 1 and aged B ticket 2. A is admitted and rejected. A re-enters with ticket 1 and wins again before B. B has now waited behind two admissions carrying the same older ticket, although `N - 1` permits one. Worse, if a new A generation arrives during each submission, collision preserves ticket 1 and resets the per-generation rejection count. No generation is rejected twice, so A can monopolize tier 4 indefinitely and B never reaches admission. The exit tests cover a second rejection of an unchanged generation, not this competing-ticket collision ([target lines 465–466](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:465)).

Required correction: define bounded retry ordering separately from generation rejection count. A failed ticket must not repeatedly preempt identities already waiting, and collision must inherit bounded retry debt even when the newer generation’s rejection count starts clean. Amend the numeric bound accordingly and test two competing identities with continuous collision updates.

### Major

#### M-1 — Receipt metadata has no authoritative maintenance-payload owner

The receipt retains only `(CRTC, class, generation, ticket, rejection count)` ([target lines 343–360](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:343)); the pure decider owns no resources ([target lines 75–82, 125–128](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:75)). No component is assigned the corresponding cursor image/LUT and reconstructible resource state after confirmation.

Sequence: a cursor payload is moved into a commit and its desired descriptor is consumed. The ioctl rejects it. The resource contract permits cleanup of rejected new resources ([authoritative spec lines 108–117](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:108)), while the receipt has only a generation number. Re-entry can therefore either reference released state or fail to reconstruct the requested cursor. The same omission leaves `Completed` without an owner that promotes the carried payload to current, and `CompletionUnknown` without the device-independent state that C.0 requires recovery to preserve and remap while admission is stopped ([C.0 lines 1961–1974, 1996–2025](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1961)). The verification table also omits a `Completed` receipt-closure/current-state mutation.

Required correction: assign one authoritative desired/submitted/current maintenance-payload store outside the decider. Define its ownership transitions for all three terminal outcomes, including dormant handoff to recovery before topology remapping, and add evidence that `Completed` closes the receipt and promotes exactly the carried generation.

### Minor

#### m-1 — The receipt is assigned to the wrong producer interface

The target says `confirm(token)` returns an `AdmissionReceipt` containing `CommitId` ([target line 343](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:343)). The implemented pure A1 interface returns `Confirmed { decision, sequence }` and has no owner commit identity ([token.rs lines 20–24, 49–66](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/admission/token.rs:20)). Giving it `CommitId` would breach the decider/conductor boundary.

No A1/A2 redesign is necessary: the conductor already has the owner’s `CommitId`, receives `Confirmed`, and owns the terminal-event route ([admission.rs lines 693–730](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:693), [backend.rs line 19561](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:19561)). Specify that the conductor constructs the receipt from `(CommitId, Confirmed)` and intercepts matching `Terminal` events before forwarding them to the resource consumer.

## Coverage and implementation checks

- **Incorporation:** all four round-2 findings checked against revision 3.
- **Architecture/contracts:** checked receipt production, metadata and payload ownership, terminal routing, recovery handoff, and A1/A2 extensibility.
- **Safety/failure:** checked rejection identity, collision ordering, resource lifetime, `CompletionUnknown`, second-rejection drop, and post-drop progress.
- **Compliance/verification:** checked authoritative stage-2c §§2–4, 6–7; C.0 §9.2.1 and §10; and the receipt-related verification rows.

**Excerpts used: 12/12:** two stage-2c excerpts, four C.0 excerpts, and six A1/A2/owner source excerpts. Exact Plan-B APIs, stage-4 producer representations, unrelated owner paths, fixture correctness, compilation, and test results were not assessed and are not judged sound. Formatting, clippy, test, and portability gates are correctly assigned to implementation.