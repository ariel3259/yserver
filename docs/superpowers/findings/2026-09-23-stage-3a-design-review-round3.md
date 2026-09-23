# Stage 3a design — codex review, round 3

**Target:** `docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`
revision 3 (`97669de7`), prior review round 2.

**Result:** 1 blocking, 1 major, 1 minor; coverage INCOMPLETE (24/24; the Ciii
unflip retirement callback's exact release timing unassessed). Trend: r1 1B 2M,
r2 2B 3M, r3 1B 1M 1m. Incorporation: B-1, B-2, M-2, M-3 APPLIED; M-1 PARTIAL.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-23):**

- **B-1 — CONFIRMED.** The unflip is ready only with exit retirement vacant, a
  composed return established and the shadow materialized
  (`render/admission.rs:1007`); `ComposedReturnNotEstablished` is a
  `Waiting` reason, and the tick returns before composing while an unflip is
  pending (`backend.rs:22551`). No deadline covered the wait.
  **Disposition: the two-phase off is removed, not bounded.** Rounds 1–3 each
  patched the pre-off unflip (release handoff, then `PriorBufferReleased`,
  then `Quiescing`, then preparation). With an `ACTIVE`-only off the plane
  keeps its framebuffer, so nothing requires the direct buffer to leave first:
  the Cfb contract (§3.4, and its release table's "source drawable dropped"
  row) already keeps a direct allocation alive while a lease holds it, even if
  the client destroys its window. Revision 4 keeps direct current and pinned
  through off, makes off CRTCs ineligible for direct while off, and leaves the
  return to composed to the ordinary Ciii unflip after on. Off is one commit
  with one deadline. This also answers the round's unassessed question: no
  unflip retirement is on the off path any more.
- **M-1 — CONFIRMED.** C.0 §16.3 names both available devices
  (lines 3318–3319). Assigned: card1 in 3a; the iGPU in C.0's final-tip
  bounded delivery check, reported unexercised by identity if it has no
  monitor.
- **m-1 — CONFIRMED.** The umbrella's gate compares a listening connection too.
  Added to the 3a differential.

---

## Verdict

**1 blocking, 1 major, 1 minor.**  
Coverage: **INCOMPLETE**

This is a design review. It does not establish that the design is implemented, compiles, or passes tests.

## Incorporation audit

| Round 2 finding | Status | Assessment |
| --- | --- | --- |
| B-1 — superseded topology commit can submit | **APPLIED** | The conductor now checks the tag before final `TEST_ONLY` and dispatch, and removes queued work on supersession ([3a:233](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:233)). These checks also cover supersession between unflip completion and off dispatch, provided the off uses that stated path. |
| B-2 — off waits for direct-buffer release | **APPLIED** | Off follows the unflip’s `Completed` milestone; prior-buffer release remains in the retirement ledger ([3a:199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:199)). |
| M-1 — unflip blocked by `Quiescing` | **PARTIAL** | The transition-owned admission exception is specified. The design still lacks a way to satisfy or fail the unflip’s readiness prerequisites while ordinary admission is closed; see B-1 ([3a:191](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:191)). |
| M-2 — off deadline unspecified | **APPLIED** | Off uses the lifecycle hardware deadline and a separate host-call watchdog, with missing and late fence evidence ([3a:245](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:245), [3a:351](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:351)). |
| M-3 — capability stability untested | **APPLIED** | The fixture compares advertised capability across DPMS cycles and completion loss ([3a:361](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:361)). |

## Findings

### Blocking

**B-1 — Transition-owned unflip can wait forever for a composed return.**  
The off sequence requires an unflip before `ACTIVE=0`, but specifies only an exception to closed admission and says that an unflip failure fails the transition ([3a:186](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:186)). The existing unflip is *not ready* until exit retirement is vacant, a composed return is established, and the direct shadow is materialized ([admission.rs:1007](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1007)). Its tick path retries shadow materialization, then returns before normal scene composition ([backend.rs:22551](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:22551)); a missing composed return is only a waiting state, not a failed submission. An off request with direct scanout current and no established composed return can therefore remain queued without reaching either commit’s deadline or the stated failure edge. That defeats the required convergence of a still-valid DPMS target ([C.0:901](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:901)). Specify how the driver establishes and wakes the composed return and shadow while `Quiescing`, and how an unrecoverable preparation failure terminalizes safely while retaining direct resources.

### Major

**M-1 — The physical DPMS gate has no owner for the second available device.**  
The proposed hardware run covers card1 only ([3a:365](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:365)), matching the umbrella’s 3a milestone ([umbrella:239](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:239)). C.0’s release evidence, however, calls for real old-active off-fence loops and a bounded DPMS delivery check on the devices actually available; it names both the Raphael iGPU and RTX 5060 Ti ([C.0:3318](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3318), [C.0:3579](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3579), [C.0:3591](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3591)). A successful card1 cycle cannot establish delivery on the other display device. Assign the second-device DPMS check to a named later C.0 release gate, or include it in 3a; classify any observed completion-safety failure under C.0’s release disposition ([C.0:3335](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3335)).

### Minor

**m-1 — The DPMS differential omits the listening client.**  
The 3a script compares bytes “to the client” ([3a:312](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:312)). The umbrella’s protocol gate compares the requester *and* a second listening connection ([umbrella:375](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:375)). A stray Owner event sent only to listeners would pass the stated DPMS byte comparison, despite DPMS having no event. Include both connections in the 3a core comparison.

## Coverage and implementation checks

- **Incorporation:** All five round 2 findings assessed against revision 3.
- **Architecture and safety:** Checked coordinator/driver ownership, queued and in-flight supersession, the interval between unflip completion and off dispatch, unflip readiness, and retained-buffer intent. The stated pre-submit checks cover that supersession interval.
- **Spec and evidence:** Checked relevant C.0 lifecycle, ordering, retirement, deadline, and physical DPMS clauses, plus the umbrella’s client gate. **24/24 bounded spec/source excerpts** used beyond the once-read target and prior review.
- **Limits:** The reading limit prevented verification of the existing unflip retirement callback’s exact release timing and the umbrella’s implementation-gate details. Neither is established sound. The focused follow-up question is whether the callback retains the direct source until `PriorBufferReleased` when unflip `Completed` precedes that proof. Compiler, test, portability, and hardware outcomes remain implementation checks; none were run.
