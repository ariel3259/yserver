## Verdict

**2 blocking, 2 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** section 16.3 "Evidence regime — revision 3" (lines 3204–3665) of
`docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`,
reviewed against the **rest of the same document** as authority — a deliberate
same-file review of a revision that entered at `39c9b75e` without one (see
`2026-09-16-phase-c0-verification-regime-decision.md`).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `783089b4`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.
First review of this section; a different target from the debt-design rounds,
so its counts are not a trend against theirs.

**Recorded usage:** 53,862 tokens reported by the completed process (exit 0),
excluding the author session. 12/12 bounded excerpts.

**Author verification (2026-09-16), all four checked against the design text:**

- **B-1 — CONFIRMED, verbatim.** Lines 3286–3288: "An executor, owner-completion,
  poison, watchdog, or off-transition-fence failure on either device invalidates
  the shared design and blocks merge. Those are architecture and
  completion-safety properties, not cohort properties." Lines 3545–3548 let the
  same occurrence be classified "as a defect of that driver on that device, which
  is recorded with its exact identity and does not generalize", without blocking.
  One rule says an occurrence is never a cohort property and always blocks; the
  other lets it be attributed to a driver and not block.
- **B-2 — CONFIRMED, and the most consequential.** Every device qualifies on its
  first real install/restore commit (2050–2052). That lifecycle commit's deadline
  derives from "the release cohort's audited/measured
  `LifecycleCompletionObservedMax`" (2189–2191); missing evidence "leaves that
  cohort unvalidated" (2193). Revision 3 removed the campaigns that produced such
  measurements, yet says the cohorts nobody here owns "select `AtomicHardware`
  optimistically at runtime" (3292–3294). No default deadline exists for an
  unmeasured cohort — the only "bootstrap" in the document is CAP-3's prohibition
  of a *readiness* bootstrap (line 458), which is a different thing. As written,
  revision 3 promises runtime qualification to exactly the cohorts that can never
  obtain it. Note for the fix: a conservative *deadline* bootstrap does not
  conflict with CAP-3, which already lets the owner submit the qualification
  commit while readiness is false.
- **M-1 — CONFIRMED.** Across sections 15–16, "leak" appears only in unit-test
  contexts ("cannot leak to an unrelated output", "fail without leak or
  double-close"). Telemetry exports counts that could observe accumulation —
  `IncarnationFdSet` alias and lease counts, helper reaps, resources released at
  the teardown barrier — but no requirement asserts a return to baseline across
  repeated transitions.
- **M-2 — CONFIRMED.** Section 16.3 never mentions DRM master and specifies no
  external supervision for the delivery check; its "seat" and "supervisor"
  mentions are test rows and the teardown parent process. Independently
  corroborated the same day: the stage 2c-i debt design's part 3 found that this
  box's fixtures run without master and the flip-accepted path has never
  executed on it.

No finding was a false positive. Corrections are pending the user's decision on
B-1, which is a release-policy choice rather than a design error.

This is a design-review result only; it does not claim that the code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| None | N/A | This is the first review. Check 1 was skipped as instructed. |

## Findings

### Blocking

#### B-1 — Revision 3 contradicts itself on whether a completion-safety occurrence blocks release

Revision 3 says the soak’s budget remains “zero occurrences” and has merely moved enforcement to runtime ([lines 3238–3242](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3238)). It also says any executor, completion, poison, watchdog, or off-transition failure on either available device invalidates the shared design and blocks merge ([lines 3286–3288](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3286)).

The delivery-check rule later permits the same occurrence to be classified as a device/driver defect, recorded, and allowed not to generalize; it expressly requires no longer zero-occurrence run ([lines 3538–3549](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3538)). Runtime poison and quarantine do not preserve a zero-occurrence release gate: they contain a failure after its first occurrence.

Concrete sequence: DPMS-off loses its fence on the Raphael device; the incarnation is poisoned and safely withdrawn. Lines 3286–3288 require blocking merge, while lines 3545–3548 allow classifying it as that driver/device’s defect and merging. The evidence owner has no authoritative disposition.

Smallest correction: choose one rule. If zero occurrence remains the release budget, every occurrence on available hardware blocks until fixed or the row is rerun cleanly. If driver-local occurrences may ship behind fail-closed degradation, delete the claim that the soak’s zero-occurrence budget is preserved and explicitly define the new, weaker containment guarantee and release disposition.

#### B-2 — Runtime qualification has a circular lifecycle-deadline prerequisite on previously unmeasured devices

Revision 3 promises deterministic per-incarnation qualification on every boot and says lifecycle windows refuse to exist without a validated observation ([lines 3218–3231](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3218)). Section 10.3 derives the lifecycle deadline from the release cohort’s audited/measured `LifecycleCompletionObservedMax`; absent representable evidence, the cohort remains unvalidated ([lines 2183–2194](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2183)). Revision 3 simultaneously eliminates campaigns and says untested cohorts select hardware optimistically at runtime ([lines 3290–3296](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3290)).

Concrete sequence: an unseen Intel device boots and its first real install/restore becomes the mandatory qualification commit ([lines 2050–2067](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2050)). No cohort observation exists from which to construct its lifecycle deadline. The implementation must either wait without a bound, invent an unauthorized default, or leave the device permanently unqualified. Each violates a stated contract.

Smallest correction: define a deterministic bootstrap deadline available before the first observation, including its conservative bound and failure semantics. Measurements may refine later incarnations, but cannot be a prerequisite for the first bounded qualification.

### Major

#### M-1 — The replacement evidence has no detector for cumulative resource leakage

The bounded delivery check stops once every transition class has been exercised and explicitly has no duration budget ([lines 3538–3550](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3538)). Runtime qualification observes structural capability and canonical completion; it does not establish steady-state resource return.

Telemetry records selected counts and high-water marks—leases, helpers, pending work, blobs—but specifies no before/after invariant for process fds, memory, DRM framebuffer/property-blob handles, executor processes, or kernel object counts ([lines 2605–2634](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2605), [lines 2667–2674](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2667)).

Concrete sequence: every VT release/acquire leaks one fd or framebuffer while all fences complete successfully. Qualification remains true, none of the four completion-safety classes fires, and one bounded pass succeeds; a production session eventually exhausts resources.

Smallest correction: add a finite repeated-transition resource-return arm with declared iteration count, baseline/final measurements, permitted steady caches, and zero-growth assertions for owned fds, helpers, memory, framebuffers, blobs, and leases. This need not restore an eight-hour rarity soak.

#### M-2 — The delivery check lacks a DRM-master and external-supervision execution contract

The required set includes real DPMS, VT release/acquire, direct/composed transitions, fullscreen changes, and CRTC disable ([lines 3538–3544](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3538)). Merely assigning scheduling to the author ([lines 3298–3299](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3298)) does not define how the release-tip server obtains DRM master, how VT switching is driven while it relinquishes control, or how the evidence proves each transition reached the tested device rather than failing before KMS submission.

Concrete sequence: the script runs under another compositor or from a session that does not own DRM master. Its requests fail or exercise another display server; transition counters remain incomplete or misleading, yet no prescribed precondition makes the row invalid.

Smallest correction: require an externally supervised recipe that starts the exact build as the sole seat/DRM-master owner, records device/incarnation and master acquisition, drives VT switching outside the server, proves master release and reacquisition, and requires a successful accepted-to-completed KMS transaction for every transition class.

## Coverage and implementation checks

- **Incorporation:** no prior review existed.
- **Architecture/contracts:** checked revision 3 against CAP, qualification, deadlines, terminalization, telemetry, and implementation reachability.
- **Safety/ownership:** checked first-failure disposition, bounded waits, quarantine semantics, cumulative resource return, and DRM-master supervision.
- **Verification:** checked whether runtime qualification and the delivery set can establish the guarantees attributed to the removed soak.

**Excerpts used: 12/12.** Four covered all target lines 3204–3665; six covered relevant authority in §§6.2, 10, 15, and 18; two checked KMS source. Source inspection confirmed that `CompletionCaps` contains the stated incarnation/topology/capability fields ([qualification.rs lines 23–47](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/qualification.rs:23)) and that a matching completed candidate deterministically advances `Awaiting` to `Qualified` ([device.rs lines 951–997](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:951)). It does not establish implementation of every terminalization, lifecycle, telemetry, or launch-path claim.

Unassessed ground includes the complete seat/master acquisition path, every completion-failure branch, and telemetry exporter implementation. These are **unverified, not sound**. Compilation, tests, portability gates, hardware execution, and evidence-manifest validation remain deferred to implementation.