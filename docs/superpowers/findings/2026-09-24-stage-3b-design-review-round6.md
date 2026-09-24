# Stage 3b design — codex review, round 6

**Target:** `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`
revision 6 (`2c351096`), prior review round 5.

**Result:** 2 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24; the copied
route's full return path and gate interception of every RANDR mutation
unassessed). Trend: r1 2B 3M, r2 2B, r3 1B 1M, r4 2B 1M, r5 1B 2M, r6 2B 1M.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):**

- **B-1 — CONFIRMED.** C.0 §10 (line 1985) forbids treating `EACCES`, `ENOENT`
  or device loss as an ordinary rejection. Revision 7 classifies by errno:
  candidate-invalid errnos keep `Ready`; the rest close readiness.
- **B-2 — CONFIRMED.** The hook ran only at promotion, after the commit chose
  `ACTIVE`, and returns a `Result`. Revision 7 stages the projection in
  preparation (pure read of the global level), covers it by freshness, and
  requires an infallible promotion form.
- **M-1 — CONFIRMED.** A text contradiction left from revision 5; revision 7
  says "admitted on `Tier::Topology`, no KMS call".

## Verdict

**2 blocking, 1 major, 0 minor.** Coverage: **INCOMPLETE**.

This is a design review. It does not establish that the design compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| Round 5 B-1, position-only ordering | **PARTIAL** | The design adds the class-1 barrier, but its position-only test still requires no `Tier::Topology` dispatch (M-1). |
| Round 5 M-1, late copied completion | **APPLIED** | Job identity routes completion to the kept output or retired bundle; bundle work cannot offer or submit. |
| Round 5 M-2, rejected displacement | **APPLIED** | The rejected commit’s old-pool `KmsRelease` registrations are cancelled exactly once. |
| Round 4 B-1, position-only scene replacement | **APPLIED** | The existing scene state is updated in place. |
| Round 4 B-2, dark-to-dark pool release | **APPLIED** | A typed proof requires proven off state and a completed displacing commit. |
| Round 4 M-1, index-addressed retirement | **APPLIED** | Retired resources and late work use bundle or job identity. |
| Round 3 B-1, expiry behind Legacy | **APPLIED** | Expired deadlines are serviced before the next admission. |
| Round 3 M-1, disabled pool release | **APPLIED** | Lit, dark, and rejected displacements have distinct dispositions. |
| Round 2 B-1, scene resource handoff | **APPLIED** | Scene retirement retains resources; late jobs have a route to the bundle. |
| Round 2 B-2, validation at gate expiry | **APPLIED** | Expiry runs stateless checks only. |
| Round 1 B-1, atomic `EBUSY` retry | **APPLIED** | Retry is forbidden and readiness closes. |
| Round 1 B-2, whole-request bound | **APPLIED** | Queue, probe, prerequisite, unflip, host-call, and completion stages have stated bounds, with the declared Legacy allowance. |
| Round 1 M-1, position-only request | **PARTIAL** | The logical transaction is specified, but its test contradicts its admission contract (M-1). |
| Round 1 M-2, fallible promotion | **PARTIAL** | Scene construction moved before acceptance; the projection hook remains fallible after it (B-2). |
| Round 1 M-3, requester-less publication | **APPLIED** | Events reach the arbiter immediately; publication is ordered through the gate. |

## Findings

### Blocking

**B-1 — Rejection handling leaves an invalid device `Ready`.** The preparation row leaves the device `Ready` after any `TEST_ONLY` failure, and the real-commit row keeps it `Ready` after every non-`EBUSY` explicit rejection ([3b:502–507](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:502)). C.0 requires errors such as `EACCES`, `ENOENT`/removed objects, and device loss to trigger topology reconstruction or readiness revocation ([C.0:1986–2000](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1986)). For example, master access is lost after the seat check; `TEST_ONLY` returns `EACCES`; the plan leaves the Owner `Ready`, allowing another live submission on the invalid state. Classify preparation and real-commit errors by cause, close readiness or rebuild where C.0 requires it, and test rejection followed by attempted admission.

**B-2 — A new output needs its DPMS target before the commit, but receives it afterward.** The transaction reads the target output’s `dpms_target` to choose `ACTIVE` ([3b:175–179](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:175), [3b:252–260](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:252)). Yet the projection hook runs only after KMS success ([3b:316–340](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:316)); the existing hook removes projections for absent outputs and adds them for present ones ([admission.rs:676–708](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:676)). Enable a previously disabled output while global DPMS is Off: its projection is absent when the commit must choose `ACTIVE=0`. Lighting it violates C.0’s rule that a new output inherits the global level before installation ([C.0:844–849](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:844)); refusing it defeats the specified dark enable.

The same late hook returns a `Result` and can queue actions, despite the claim that every promotion step is infallible and only moves data ([admission.rs:641–708](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:641), [3b:338–340](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:338)). Stage the new projection from the current global level before final validation and dispatch; define an infallible promotion or resolve every possible hook failure before acceptance.

### Major

**M-1 — The position-only test contradicts the ordering contract.** The corrected design admits position-only work on `Tier::Topology` with an empty description and waits for the device slot ([3b:202–208](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:202)). Its evidence instead requires “no `Tier::Topology` dispatch” ([3b:714–715](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:714)). Implementing that test literally permits position promotion while an accepted composed flip still owns the old origin, contrary to C.0’s ordering barrier ([C.0:1452–1462](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1452)). Require topology-tier admission and **no KMS call** in that test; keep the separate held-flip assertion.

### Minor

None.

## Coverage and implementation checks

**24/24 spec/source excerpts used; one excerpt exceeded the 120-line cap by one line.** Incorporation covered round 5 and its carried findings. Architecture checked the projection producer/consumer, lifecycle result boundary, device ordering, and RANDR gate contract. Safety checked rejection, position ordering, retired work, and resource release. Compliance checked the relevant C.0 evidence, ordering, DPMS, deadline, verification, and stage boundaries; formatting, CI clippy, build, portability, fixture, and approved hardware gates remain implementation checks. No builds or tests were run.

Coverage is incomplete because the excerpt budget ended before the full copied-route source/sink return path and every RANDR mutation’s gate interception could be assessed. Those paths are **unverified**, not judged sound. A focused follow-up should ask whether each late copied-route return reaches its retired bundle and whether each configuration-changing RANDR request enters the gate before state-dependent validation.