# Stage 3a design — codex review, round 4

**Target:** `docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`
revision 4 (`ff96f3d7`), prior review round 3.

**Result:** 1 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24; the
Present Copy off-screen completion path and the unflip retirement callback
unassessed). Trend: r1 1B 2M, r2 2B 3M, r3 1B 1M 1m, r4 1B 1M.
Incorporation: M-1 and m-1 APPLIED; B-1 TRADED (the wait moved to after on).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-23):**

- **B-1 — CONFIRMED as a wait, REASSIGNED as to its owner.** The composed
  return is the framebuffer Ciii retains for the return
  (`render/admission.rs:844`: `retained_composed_framebuffer` on Legacy, the
  owner's current composed framebuffer on Owner). An `ACTIVE` toggle neither
  creates nor drops it, so after on the unflip is in exactly the state it
  would reach if the client destroyed its window while lit, with no DPMS.
  Rounds 3 and 4 both reached this wait; it is a property of Ciii, not of
  DPMS. Revision 5 proves the equivalence in 3a and moves the question —
  can a direct unit be current with no composed return? — to a section that
  must be answered before the 3a plan; a yes is a Ciii defect, ours, fixed in
  its own commit.
- **M-1 — CONFIRMED, answered by existing machinery.** Legacy already routes
  Presents while dark through the scanout blackout
  (`present_scanout_blackout`, `backend.rs:23360`); revision 5 makes that
  predicate per CRTC on Owner devices (a section 3.8 site) and adds the
  exact-once Present fixture.

---

## Verdict

**1 blocking, 1 major, 0 minor.**  
Coverage: **INCOMPLETE**

This is a design review. It does not establish that the design is implemented, compiles, or passes tests.

## Incorporation audit

| Round 3 finding | Status | Assessment |
| --- | --- | --- |
| B-1 — pre-off unflip can wait forever | **TRADED** | The unflip is removed from the off path, so off has one bounded commit. The same unflip readiness problem can now prevent return to composed after on; see B-1. |
| M-1 — second device has no physical gate owner | **APPLIED** | [3a §5.3.1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:379) assigns the iGPU to the final-tip delivery check and requires an identity-specific unexercised report if no monitor is connected. |
| m-1 — differential omits listening client | **APPLIED** | [3a §4](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:314) now compares requester and listener bytes. |

The earlier rejection, resource-clock, stale-submit, deadline, and capability corrections remain present in the design text. No new regression in those contracts was established.

## Findings

### Blocking

**B-1 — Relight can leave a destroyed client’s direct frame visible indefinitely.**

The design retains a direct client framebuffer through off, relights it with `ACTIVE=1`, then relies on the *ordinary* Ciii unflip if its source is gone ([3a §3.5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:187), [on path](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:205)). Consider a client destroying its window while off when no composed return is established. On lights the retained client frame. Ordinary unflip waits for that return ([admission.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1007)), while the current tick returns before composition when unflip is requested ([backend.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:22551)). Marking full-frame damage does not itself establish a composed return: C.0 says composed buffers are not painted during direct ownership ([C.0 §12.1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2509)). The assertion that ordinary unflip has “its own bounds” does not cover this pre-admission wait.

Specify a post-on path that establishes and wakes the composed return while direct remains safely pinned, and a bounded failure outcome if preparation cannot progress. Test the destroyed-source case with no composed return established. Retain the direct lease until replacement proof; that part of the proposed design agrees with [Cfb §3.3–3.4](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:270) and [C.0’s release milestone](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2132).

### Major

**M-1 — Presents arriving while off lack a completion contract.**

The design says new Presents become composed work while direct scanout is ineligible, but composed offers wait on `OutputPoweredOff` ([3a §3.5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:183), [direct rule](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:202)). For a Present Copy received during a long off interval, the design does not say whether its completion and idle/release obligations are discharged by the copy, wait for on, or depend on a composed scanout that cannot submit while off. C.0 preserves Present pacing and requires terminalization and release to be tracked separately ([C.0 §12](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2388), [§10.4](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2260)).

State the off-interval Copy completion, IdleNotify, and release rule independently of off-CRTC KMS completion. Add a fixture that sends Presents while off, checks their exact-once protocol outcomes, and confirms the older direct frame remains pinned until a proven replacement.

## Coverage and implementation checks

- **Incorporation:** All three round 3 findings assessed; earlier-round corrections checked for design regressions.
- **Architecture:** Checked coordinator/arbiter ownership, topology result routing, mixed-device clock policy, and the common direct-eligibility route. The plan names the arbiter’s per-output power projection as authority, but its propagation through both direct producer and admission remains an implementation check.
- **Safety and spec:** Checked ordering, off-fence deadlines, direct leases, release milestones, damage, and post-on unflip readiness. The retained direct buffer is safe to keep pinned; post-on visual convergence is not established.
- **Verification and limits:** **24/24 read operations** beyond the once-read target and prior review; one source range was 141 lines and exceeded the 120-line per-excerpt limit. Reading stopped. The existing Present Copy fallback’s exact off-screen completion path and unflip retirement callback remain unassessed, not judged sound. The focused follow-up question is what event discharges a Copy Present’s completion and release while its CRTC is off. Real compiler, test, portability, and hardware gates belong to implementation; none were run.
