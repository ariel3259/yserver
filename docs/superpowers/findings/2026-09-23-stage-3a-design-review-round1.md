# Stage 3a design — codex review, round 1

**Target:** `docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`
revision 1 (`146a94e8`), against C.0 and the stage 3 umbrella revision 5.

**Result:** 1 blocking, 2 major, 0 minor; coverage INCOMPLETE (24/24; the §10.3
deadline interaction and later §16 cases unassessed).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-23):**

- **B-1 — CONFIRMED.** Revision 1 counted any terminal representative toward
  the protocol request's `Applied`, while C.0 allows `Applied` only after
  every projection retires or is invalidated by output removal, and requires a
  topology-scoped latch for an attributable `EINVAL`/`EOPNOTSUPP`. Fixed:
  only `Applied` or removal counts; a rejection leaves the target
  `Deferred(TopologyLatched)` or `Deferred(ReadinessClosed)`, with no retry
  under the same generation.
- **M-1 — CONFIRMED.** Legacy materializes the direct shadow before off and
  stops direct after it (`backend.rs:31057`, `:31084`); revision 1 kept "the
  current buffer" bound without saying it could be a client's. Fixed: the off
  transition runs the Ciii Owner unflip first, so an off CRTC only ever holds
  a composed buffer and the client buffer is released by the unflip's own
  retirement.
- **M-2 — CONFIRMED.** A Legacy off pauses the shared resource service's
  serviced-time clock (`backend.rs:31099`, `resources/mod.rs:319`). Fixed:
  the clock's domain is "any served output lit" across both transports.
- **Author's own finding while verifying M-2:** the rest of Legacy's off path
  is server-wide too — `scene.drain_all` and
  `reset_scanout_bos_for_suspend` (`backend.rs:31087`–`31090`) would
  reach an Owner device's scanout state on a mixed server. Fixed: every
  server-wide step of the Legacy off/on paths is scoped to Legacy devices or
  proven harmless, by an inventory in the plan.

---

## Verdict

**1 blocking, 2 major, 0 minor.**  
Coverage: **INCOMPLETE**

This is a design review, not a claim that the design has been implemented or verified.

## Incorporation audit

| Prior findings | Status |
| --- | --- |
| None; this is the first review. | Check skipped. |

## Findings

### Blocking

**B-1 — A rejected DPMS transition has no truthful disposition or latch path.**  
The design makes `FailedBeforeSubmit` terminal, returns the device to `Ready`, and waits for a new protocol request ([3a:207](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:207)). It then calls the global request `Applied` once every Owner representative has *a terminal disposition* ([3a:200](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:200)). Suppose the combined off request receives an explicit `EINVAL`: the output remains lit, yet the request can be counted as applied. None of C.0’s listed terminal dispositions describes this failed target, and C.0 permits `Applied` only after projections retire or are invalidated by output removal ([C.0:852](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:852), [C.0:883](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:883)). An attributable `EINVAL`/`EOPNOTSUPP` also requires a topology-scoped latch, rather than an unconditional return to `Ready` ([C.0:1992](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1992)).

Define rejection reconciliation: classify the latch or readiness outcome, retain the unsatisfied target without an immediate retry loop, and never aggregate rejection as `Applied`. If the existing disposition vocabulary cannot express that result, amend the authority explicitly.

### Major

**M-1 — Retained direct scanout has no specified wake and release handoff.**  
The design keeps the primary buffer current through off and says the next composed frame replaces it through ordinary admission after on ([3a:158](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:158)). Consider a direct-scanout client buffer current when off arrives. Its framebuffer and source pin must remain held while the plane stays bound. The existing Legacy off path explicitly prepares a direct shadow, then stops direct ownership after disable ([backend.rs:31057](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:31057), [backend.rs:31084](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:31084)); the proposed Owner path does not state how direct ownership exits or how a composed replacement obtains admission. C.0 permits release only after the replacement’s dependency proves the prior buffer unused ([C.0:2132](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2132)).

Specify the ordered direct-to-composed handoff, including the point that releases the client buffer and pin. Exercise off/on with a current direct client buffer, including client destruction while off and a proven composed replacement.

**M-2 — Mixed-device DPMS can pause the shared resource clock while an Owner output remains lit.**  
The design keeps the Legacy DPMS path and allows rejected Owner off to leave its output lit ([3a:113](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:113), [3a:207](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:207)). After a successful Legacy off, the current backend pauses its single resource service’s serviced-time clock ([backend.rs:31099](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:31099)). That clock governs pending-batch expiry ([resources/mod.rs:319](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:319), [resources/mod.rs:359](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:359)). In a mixed fixture where Owner off rejects, the Owner remains lit but its pending resource work can stop accruing serviced time indefinitely.

Define the clock’s power domain—per device, or active while any served output remains lit—and test the mixed rejection case with a pending batch. The planned mixed fixture checks that DPMS reaches both devices, but does not establish this resource-service behavior ([3a:255](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:255)).

## Coverage and implementation checks

- **Incorporation:** no prior review.
- **Architecture and safety:** checked the DPMS fork, result aggregation, rejection, retained direct ownership, and the shared resource clock against targeted source. The ACTIVE-only shape is conditional on canonical evidence; C.0 expressly requires the old-active off fence and truthful failure classification ([C.0:2097](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2097)).
- **Specification and evidence:** pure tables can assess arbiter decisions; fixtures and the proposed card1 test address integration and observed off fences. They cannot establish behavior on an untested driver, for which runtime classification remains the gate.
- **Reading limit:** **24/24 bounded excerpts** beyond the target design; no prior-review artifact existed. The exact §10.3 deadline interaction and later §16 verification cases were not fully assessed because the excerpt budget was exhausted. Those provisions need a focused follow-up; they are **unverified**, not judged sound. Builds, tests, hardware results, and portability checks belong to implementation and were not run.
