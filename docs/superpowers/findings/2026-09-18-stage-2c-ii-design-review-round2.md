## Verdict

**3 blocking, 1 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** `docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md`
revision 2 (`58215602` + the 11.1 decision `0a1bbbe3`), with prior
`2026-09-18-stage-2c-ii-design-review-round1.md`.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `69c6d6e2`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.154.0`.
Counts are comparable only to other reviews citing this same instrument SHA
(round 1 of this spec is; nothing earlier is).

**Recorded usage:** 72,604 tokens reported by the completed process (exit 0),
excluding the author session. 12/12 bounded excerpts used.

**Author verification (2026-09-18), every finding checked against the tree:**

- **B-1 — CONFIRMED.** C.0 §9.2.1's consecutive-slot rule has no bundle
  exception; revision 2 applied eligibility to tiers 3 and 6 only. Fixed in
  revision 3: tiers 3, 5 and 6; no exception inferred.
- **B-2 — CONFIRMED.** Stage 2c's v1.5.0 table, 2c-ii row: "Preserve border
  eligibility ... including retirement-promoted successors. A geometry/layout
  change invalidates an earlier decision", with "changed border while successor
  queued" as required coverage. Revision 2 missed that table entirely. Fixed:
  layout/eligibility generation on the descriptor, in readiness and in `lock`;
  layout change is a wake; loss of eligibility invalidates.
- **B-3 — CONFIRMED.** `retire_live` (`device.rs:2248`) returns only commit
  identity and resources, and no component owned the ticket state after
  `confirm`. Fixed: the conductor-owned `AdmissionReceipt`, an outcome table over
  `TerminalState` (`record.rs:29`: `Completed`,
  `FailedBeforeSubmit(IoctlRejected)`, `CompletionUnknown`), the collision rule,
  and the post-drop state; C.0's amendment updated to match. While fixing it, the
  first amendment's "a newer generation ... keeps the ticket" turned out to
  contradict C.0's "a newer update arriving while submitted receives a new
  ticket"; revision 3 resolves it as "newer payload, older ticket".
- **M-1 — CONFIRMED.** No comparable age across composed and direct. Fixed:
  device-monotonic `PrimaryOrdinal`.

This is a design-review result only. It does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1 — Per-unit fairness | **PARTIAL** | Grouped admissions now mark every covered CRTC served, and grouped→composed/composed→grouped cases are covered. However, round-robin eligibility is limited to tiers 3 and 6, leaving tier 5 outside the rule. See B-1 below. |
| B-2 — Confirmation at `begin` | **APPLIED** | `lock` precedes `begin`; confirmation occurs only after `send_on` crosses the IPC boundary; all enumerated pre-IPC refusals abort without consuming fairness state, with resource events handed to 2c-i ([target lines 241–301](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:241)). The separate post-dispatch ownership gap is B-3. |
| B-3 — Partial tier-5 bundles | **APPLIED** | Tier 5 now takes every ready qualified CRTC, assigns logical retirement only to included generations, and has a three-or-more-CRTC mutation ([target lines 218–225](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:218), [412–413](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:412)). |
| M-1 — Barrier/ticket accounting | **APPLIED** | Barriers are measured separately, surviving tickets retain order, replacement preserves tickets, and updates arriving during submission receive new tickets; the exit matrix includes corresponding mutations ([target lines 227–239](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:227), [414–416](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:414)). |

## Findings

### Blocking

#### B-1 — Tier 5 bypasses the per-CRTC consecutive-slot rule

The target applies round-robin eligibility only to tiers 3 and 6 ([lines 204–211](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:204)), while tier 5 admits every ready group member without that condition ([lines 218–225](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:218)). This contradicts both its own unconditional invariant at lines 227–235 and C.0’s rule that a continuously ready CRTC cannot take consecutive device slots while another CRTC has ready primary work ([C.0 lines 1600–1604](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1600)).

Sequence: A is admitted alone; A remains ready and B is ready; tier 5 admits A+B. A has occupied two consecutive slots while B was ready. The existing mutations test bundle membership and served-bit updates, not tier-5 eligibility.

Correction: apply the per-CRTC eligibility rule to tier 5 and add singular→bundle coverage. If an all-ready bundle is intended to be an exception because it services every owed CRTC simultaneously, that exception must first be made explicit in governing C.0; the stage design cannot silently infer it.

#### B-2 — Queued direct work lacks the required layout/eligibility identity

A direct descriptor carries only source-allocation generation and readiness ([target lines 107–127](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:107)). Its readiness predicate checks source waits and retirement capacity, while `lock` checks only generations named by the decision ([lines 133–160](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:133), [253–260](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:253)). Eligibility/layout change is also absent from the wake list.

The parent contract explicitly requires current border eligibility on every direct path and says a geometry/layout change invalidates an earlier decision, including queued and retirement-promoted successors ([stage-2c lines 217–226](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:217)).

Sequence: a successor is queued as direct-eligible; an ancestor gains a border without changing its source allocation generation; retirement later wakes admission; the target’s snapshot and lock still accept the stale successor.

Correction: bind direct descriptors to the relevant eligibility/layout/topology generation, make invalidation a wake, require current direct eligibility in the snapshot and lock, and add the parent’s changed-border-while-queued mutation.

#### B-3 — No owner retains the admission receipt needed after kernel rejection

At confirmation, the token and ticket are consumed; afterward the target delegates entirely to the normal owner lifecycle ([target lines 253–268](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:253), [314–316](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:314)). Yet §11.1 and amended C.0 require a rejected maintenance generation to re-enter with its original ticket and require recognizing a second rejection of that same generation ([target lines 467–486](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:467), [C.0 lines 1591–1598](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1591)).

No component is assigned ownership of `(commit, maintenance generation, original ticket, rejection count)` after confirmation. The current rejection events return only commit identity and resource dispositions ([device.rs lines 2248–2280](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:2248)). The design also leaves “drop, then service under incompatible-maintenance rules” without a concrete successor state; an implementation may either let primary work bypass the rejected desired state or retain an unserviceable incompatibility forever.

Correction: assign an authoritative per-commit admission receipt until terminal outcome; define collision with a newer desired generation; and define the bounded recovery/readiness transition after the second rejection. Verification must cover both metadata survival and the affected CRTC’s subsequent progress—not merely “no third admission.”

### Major

#### M-1 — “Oldest ready primary” has no comparable ordering identity

Tier 6 selects the oldest ready primary ([target line 177](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:177)), but composed scene generations and direct allocation generations are different namespaces, and no device-monotonic primary-ready ordinal or stable tie-break is defined. With initially ready composed A and direct B and no previous turn debt, the required deterministic winner cannot be derived from the design. This conflicts with the required deterministic admission decision ([stage-2c lines 79–83](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:79)).

Correction: define when primary age is assigned, whether it survives temporary capacity loss, and a stable final tie-break across primary shapes and CRTCs.

## Coverage and implementation checks

- **Incorporation:** all four prior findings checked against revision-2 text and mutations.
- **Architecture/contracts:** checked decider/conductor ownership, information delivery, direct eligibility, multi-CRTC accounting, and post-dispatch handoff.
- **Safety/failure:** checked lock/send ordering, pre-IPC refusal, explicit kernel rejection, ticket identity, resource retirement, and recovery progress.
- **Compliance/verification:** checked C.0 §9, relevant §10 lifecycle, stage-2c §§2/4/6/7, and 2c-i §6.

**Excerpts used: 12/12:** three C.0 §9, three stage-2c, one 2c-i §6, two C.0 §10, and three owner-source excerpts. Detailed coordinate transport, C.1 mechanics, hardware qualification, exact module/API placement, `cancel_live`, `capacity.rs`, compilation, and fixture correctness were not assessed; they are out of scope or deferred to implementation and are not judged sound here.