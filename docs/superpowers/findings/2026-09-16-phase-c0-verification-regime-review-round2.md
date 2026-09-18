## Verdict

**1 blocking, 3 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** section 16.3 revision 4 (`6974ad18`) and the section 10.3 bootstrap
paragraph of `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`,
against the rest of the same document, with `--prior` round 1.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `783089b4`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.
**Comparable to round 1** (same SHA and target): 2 blocking + 2 major → 1 blocking
+ 3 major. **Not converging cleanly**: each revision-4 fix closed its round-1
finding and opened a smaller hole of its own.

**Recorded usage:** 55,383 tokens reported by the completed process (exit 0),
excluding the author session. 10/12 bounded excerpts.

**Author verification (2026-09-16), all four confirmed:**

- **B-1 — CONFIRMED.** Revision 4 left in section 16.3 (lines 3251–3253) "the
  lifecycle window refuses to exist without a validated observation", which the
  new bootstrap contradicts. Line 2226–2228: a hardware timeout after dispatch
  "enters `CompletionUnknown`, closes readiness, and poisons the device
  incarnation" — so the bootstrap's "expiry is handled as any lifecycle
  hardware-completion expiry" does resolve to poison. But lines 2191–2194, left
  in place, say a healthy completion above the 28-second margin "leaves that
  cohort unvalidated rather than poisoning it". A healthy driver completing at
  31 seconds satisfies both rules at once.
- **M-1 — CONFIRMED by reading the author's own text.** Criterion 3's example,
  "an advertised off-transition fence that is never signalled", cannot be
  observed in finite time: a fence still pending at an observation boundary is
  the symptom being classified, not evidence of who caused it.
- **M-2 — CONFIRMED.** Section 18 (around lines 4040–4095) names "the bounded
  delivery check" three times and the resource-return arm not at all. The
  author's "process and DRM debug interfaces otherwise" names no interface, and
  "caches enumerated in advance with their bounds" lets a bounded cache absorb a
  one-per-pass leak across fifty passes.
- **M-3 — CONFIRMED against the code.** `KmsBackend`'s VT release logs "VT
  release — suspended; drmDropMaster", calls `release_master_lock`, and only
  then acknowledges with `VT_RELDISP(1)` (`backend.rs` around 20127–20134). yserver
  is not master while VT-away, so "sole DRM master" as a continuous condition
  cannot hold across the VT cycle the same contract requires.

**Author's note.** When dispatching this round the author named four suspected
weaknesses in revision 4: the bootstrap's expiry semantics, criterion 3's
falsifiability, the resource-return observables and cache loophole, and sole
master versus VT release. All four came back as these findings. Weaknesses
visible before a review should be fixed before dispatching it; this round spent
55k tokens confirming them.

No finding was a false positive. Section 16.3 blocks neither the stage 2c-i debt
stage nor 2c-ii; revision 5 is pending the user's decision on timing.

This is a design-review result only; it does not claim that code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1 — contradictory release dispositions | **PARTIAL** | Revision 4 withdraws the zero-occurrence claim and establishes a three-criterion disposition ([3259–3267](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3259), [3578–3596](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3578)). Criterion 3, however, lacks a finite evidence rule capable of distinguishing a driver violation from shared machinery, kernel-core behavior, or merely late completion. |
| B-2 — no deadline for unmeasured cohorts | **TRADED** | The 30-second bootstrap makes the first qualification commit bounded ([2195–2206](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2195)), but revision 4 retains contradictory statements about requiring prior validation and about whether a healthy completion beyond the representable margin poisons the incarnation. |
| M-1 — no cumulative-leak detector | **PARTIAL** | A warm-up plus 50-pass arm and zero-growth assertions were added ([3598–3614](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3598)). Several asserted quantities lack a defined observable, cache exemptions are insufficiently constrained, and section 18 assigns neither implementation nor final-tip evidence treatment to this arm. |
| M-2 — no DRM-master/supervision contract | **TRADED** | External VT driving, KMS completion, identity, and invalid-row rules were added ([3616–3626](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3616)). The continuous “sole DRM master” condition contradicts the required interval in which yserver has dropped master. |

## Findings

### Blocking

#### B-1 — Bootstrap qualification has contradictory prerequisite and expiry semantics

The new rule creates a 30-second lifecycle window precisely when no audited or measured observation exists ([2195–2205](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2195)). Section 16.3 nevertheless still says the lifecycle window “refuses to exist without a validated observation” ([3249–3252](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3249)).

Expiry is defined: every post-dispatch hardware timeout enters `CompletionUnknown` and poisons the incarnation ([2224–2233](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2224)); continuing-evidence breaches likewise poison it ([2069–2072](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2069)). But the deadline text says a healthy completion above the 28-second representable margin leaves the cohort unvalidated “rather than poisoning it” ([2191–2194](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2191)).

Concrete sequence: an unmeasured driver accepts its qualification commit and signals at 31 seconds. One rule requires the bootstrap window and incarnation poison at 30 seconds; the retained rules say the window cannot exist and characterize such a healthy slow completion as unvalidated rather than poisoned.

Smallest correction: state explicitly that the bootstrap is an exception to the prior-observation prerequisite, and distinguish offline calibration observations from live accepted commits. For live bootstrap commits, specify whether expiry irreversibly poisons under `CompletionUnknown`; then narrow or remove the conflicting “healthy completion above 28 seconds” statement.

### Major

#### M-1 — “Positive driver evidence” is not a finite attribution rule

Criterion 3 permits a non-blocking driver-local disposition based on “positive evidence,” exemplified by a fence “never signalled” at the kernel/driver interface ([3589–3594](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3589)). A finite capture can prove only that a fence remained pending until an observation boundary. That is already the symptom being classified; it does not establish whether the cause is the device driver, DRM core, hardware, executor bookkeeping, or incorrect userspace fence ownership.

Concrete scenario: the fence is pending at the 30-second deadline and signals at 31 seconds. The same trace can be declared either a driver contract violation or a lifecycle-bound/calibration defect, changing whether merge is blocked.

Smallest correction: enumerate acceptable finite attribution evidence and its decision owner—for example, a named driver trace proving commit completion while its returned fence remains pending, a kernel error explicitly attributing failure, or a reproduced minimal raw-KMS case excluding yserver. Deadline expiry or non-reproduction alone must remain shared/unresolved.

#### M-2 — Resource-return assertions lack complete observables and allow self-authorized growth

Section 15 exports alias/lease and helper lifecycle telemetry plus gamma-blob accounting ([2617–2641](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2617), [2682–2694](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2682)). It does not define process-owned-fd, live framebuffer, general property-blob, helper-count, or resident-memory gauges. “Process and DRM debug interfaces otherwise” ([3605–3611](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3605)) names no interface, ownership correlation, privilege requirement, or invalid-evidence outcome. Debug state exposing active DRM objects cannot prove absence of leaked inactive objects.

Moreover, any growth may be exempted as a cache merely by enumerating it in advance with a bound; no rule requires cache occupancy to stabilize after warm-up or prevents a 50-entry “cache” from absorbing a one-per-pass leak.

Section 18’s final-tip evidence and manifest lists include the delivery check but omit the resource-return arm ([4046–4057](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:4046), [4083–4093](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:4083)).

Smallest correction: assign concrete counters/interfaces and ownership keys for every resource class; make unavailable observation invalidate the arm; require named caches to expose occupancy, semantic eviction/bounds, and zero post-warm-up growth; and add the arm to section 18’s final-tip manifest and invalidation rules.

#### M-3 — The execution contract requires mutually exclusive DRM-master states

A valid row must show yserver running as the “sole DRM master” while also showing master released and reacquired on every VT cycle ([3616–3626](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3616)). The actual lifecycle necessarily drops master before acknowledging VT release and reacquires it only on VT acquisition ([backend.rs:20127](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20127), [backend.rs:20143](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20143)).

Thus no row containing the mandatory VT transition can prove continuous sole-master ownership. If “sole” means only while the session is active, that temporal qualification is unstated.

Smallest correction: require yserver to be the sole master whenever its session is active and KMS transactions are counted; require positive proof of no master during its released interval, followed by yserver’s reacquisition before counting the acquire transaction.

## Coverage and implementation checks

- **Incorporation:** audited all four round-1 findings against revision 4.
- **Architecture/contracts:** checked qualification, release disposition, evidence ownership, section 18 assignment, and VT/master integration.
- **Safety/ownership:** checked timeout poisoning, quarantine consequences, resource observability, and master handoff.
- **Verification:** checked whether each new arm can produce finite, attributable, final-tip evidence.

**Excerpts used: 10/12** beyond the target and prior review: nine authority excerpts across §§6.2, 6.3, 6.4, 10, 15, 17, and 18, plus one source excerpt confirming the VT master handoff. Other design sections and implementation correctness were intentionally unassessed. Exact platform availability and privileges of unnamed DRM debug interfaces remain unverified; that absence of a specified interface is itself part of M-2. Compilation, tests, performance execution, and portability gates remain deferred to implementation.