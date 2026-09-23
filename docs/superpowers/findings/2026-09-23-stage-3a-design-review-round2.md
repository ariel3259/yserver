# Stage 3a design — codex review, round 2

**Target:** `docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`
revision 2 (`3c545ea3`), prior review round 1.

**Result:** 2 blocking, 3 major, 0 minor; coverage INCOMPLETE (24/24; the
umbrella's detailed §4 differential and earlier §16.3 physical DPMS-cycle
clauses unassessed). Round 1: 1B 2M. Incorporation: B-1, M-2 and the author's
addendum APPLIED; M-1 TRADED (it produced B-2 and M-1 here). This pass covered
the areas round 1 could not (§10.3, §16.2 DPMS items).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-23), against C.0:**

- **B-1 — CONFIRMED.** `REC-4` (C.0 lines 808–810) forbids a non-current
  transition from running final `TEST_ONLY` or installing state, and §16.2
  item 9 forbids stale DPMS generations from submitting; revision 2 checked
  freshness only at the result, which cannot undo a physical off. Fixed:
  checks before final `TEST_ONLY` and before dispatch; supersession removes
  the queued entry.
- **B-2 — CONFIRMED.** §10.2 (lines 2131–2148): `PriorBufferReleased` may
  arrive later and does not occupy the slot. Fixed: off is built when the
  unflip's commit is `Completed`; the client buffer stays in the retirement
  ledger.
- **M-1 — CONFIRMED.** §6.4 `Quiescing` closes admission and the only unflip
  entry is the ordinary one (`render/admission.rs:723`). Fixed: a
  transition-owned, tagged unflip is the one exception to the closure.
- **M-2 — CONFIRMED.** §10.3 (lines 2205–2216) has fast and lifecycle
  hardware deadlines; revision 2 named neither. Fixed: lifecycle deadline
  (30 s bootstrap), 2 s `NONBLOCK` watchdog, `HardwareComplete` as the
  milestone, injected missing/late off fence in the evidence.
- **M-3 — CONFIRMED.** §16.2 item 39 requires capability stability through
  DPMS and poison; revision 2 asserted none. Fixed in the evidence.

---

## Verdict

**2 blocking, 3 major, 0 minor.**  
Coverage: **INCOMPLETE**

This is a design review. It does not establish that the design has been implemented or that code compiles or passes tests.

## Incorporation audit

| Round 1 finding | Status | Revision 2 result |
| --- | --- | --- |
| B-1 — rejected DPMS counted as applied | **APPLIED** | Rejection retains an unsatisfied, deferred target; only retired or removal-invalidated projections count toward `Applied` ([3a:231–255](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:231)). |
| M-1 — direct buffer handoff | **TRADED** | The owner unflip supplies a composed replacement and preserves release proof. The new requirement to finish buffer release *before* building off creates B-2 ([3a:185–199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:185)). |
| M-2 — shared resource clock | **APPLIED** | Its stated domain is now any served output lit, with a mixed-device rejection fixture ([3a:128–136](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:128), [3a:316–318](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:316)). |
| Author’s Legacy drain/reset addendum | **APPLIED** | The design requires every server-wide Legacy off/on step to be scoped to Legacy or proved harmless to Owner ([3a:117–127](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:117)). |

## Findings

### Blocking

**B-1 — The result tag does not prevent a superseded commit from being sent.**  
The design specifies a tagged `Tier::Topology` request and checks freshness at the *result* boundary, but does not require the queued request to be revalidated before final `TEST_ONLY` and dispatch ([3a:213–229](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:213)). Off A can be queued, on B can supersede it, and A can then reach the conductor. Quarantining A’s eventual stale success cannot undo a physical off. C.0 forbids a superseded transition from running final validation or installing state ([C.0:801–810](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:801)); item 9 requires stale DPMS generations not to submit ([C.0:2765](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2765)). Require a current tag and epoch check at the conductor’s pre-submit boundary, with stale queued work cancelled. Exercise supersession while topology work is queued and while an earlier executor call is delayed.

**B-2 — Waiting for direct-buffer release can leave DPMS-off without a bound.**  
Revision 2 builds `ACTIVE=0` only after the unflip releases the client buffer and source pin ([3a:185–199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:185)). C.0 separates unflip commit completion from `PriorBufferReleased`: release may occur later and does not occupy the submission slot ([C.0:2126–2148](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2126)). Thus a composed return can complete while its old-buffer release dependency is delayed; the screen stays lit and no off ioctl starts, so neither commit-completion timer bounds the wait. Build off once the composed buffer is canonically installed and safe to retain. Keep the old direct resources in their separate release ledger until proof arrives.

### Major

**M-1 — The required unflip has no admission contract under `Quiescing`.**  
DPMS first closes device admission, then requires a composed unflip to be admitted before off ([3a:178–195](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:178)). C.0 marks `Quiescing` admission closed and classifies unflip as primary replacement, distinct from lifecycle work ([C.0:756–760](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:756), [C.0:1443–1460](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1443)). The existing unflip request is an ordinary admission entry point ([admission.rs:720–743](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:720)). With a direct unit current, closure can block the necessary unflip; opening ordinary admission instead lets unrelated primary work enter. Define a transition-owned, tagged unflip exception while ordinary admission stays closed, and test it with an accepted predecessor that must drain or terminalize.

**M-2 — DPMS-off’s completion timer and failure test are unspecified.**  
C.0 starts a hardware deadline at each accepted ioctl and distinguishes fast primary work from lifecycle work; expiry is `CompletionUnknown` ([C.0:2206–2229](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2206), [C.0:2247–2255](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2247)). The design does not classify the `ACTIVE=0` commit’s timer. A healthy slow off could be poisoned by the fast clamp, or a missing off fence could lack a bounded failure path. State the off timer class and acceptance milestone; the preceding unflip has its own timer. C.0 specifies **separate per-commit bounds**, not one combined unflip-plus-off deadline. Add an injected missing/late off-fence case alongside the proposed four-cycle hardware observation ([3a:320–328](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:320), [C.0:2097–2107](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2097)).

**M-3 — The DPMS differential cannot establish capability stability.**  
Section 4 compares DPMS bytes and power state; section 5 adds no advertised-capability assertion ([3a:279–288](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:279), [3a:300–328](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:300)). An off/on or Owner poison could therefore change the advertised capability while all listed DPMSInfo comparisons pass. Item 39 requires stability through DPMS and poison ([C.0:2914–2917](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2914)). Capture and compare that bit before and after Owner DPMS cycles and injected completion loss.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** all three round 1 findings and the author’s addendum assessed.
- **Architecture and safety:** checked coordinator/driver boundaries, stale results, admission, direct-buffer lifetime, and the existing unflip and suspend entry points. The future `Quiescing` behavior of `admission_is_active` was not verified.
- **Specification and evidence:** assessed C.0 §10.3 and §16.2 items 9, 26, 38, 39, 45, and 57–67, plus the relevant §16.3 lifecycle and bounded-delivery lists. For 3a, the DPMS, poison-entry, and arbiter portions apply; executed VT/hotplug, recovery exits, and the full later-stage delivery script remain deferred. Pure tables cover decisions, but cannot prove pre-submit cancellation or deadline delivery.
- **Reading limit:** **24/24 bounded excerpts** beyond the target and prior review. The umbrella §4 detailed client differential and earlier §16.3 physical DPMS-cycle clauses remain unassessed; the focused follow-up question is whether either adds a 3a client or hardware observation beyond the proposed DPMS-only evidence. Builds, tests, portability gates, and hardware results belong to implementation and were not run.
