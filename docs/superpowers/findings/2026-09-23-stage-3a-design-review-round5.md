# Stage 3a design — codex review, round 5

**Target:** `docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`
revision 5 (`d3189dea`), prior review round 4.

**Result:** 0 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24; the unflip
retirement callback and the admission of an unflip requested during
`Quiescing` unassessed). Trend: r1 1B 2M, r2 2B 3M, r3 1B 1M 1m, r4 1B 1M,
r5 0B 1M. Incorporation: B-1 APPLIED by reassignment (judged sound for an
established unflip intent); M-1 PARTIAL.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-23):**

- **M-1 — CONFIRMED.** The core samples one blackout boolean
  (`process_request.rs:10299`) and, when true, executes every source-ready
  parked Present and flushes every queued completion past its due check
  (`:10379`). Revision 5 made only the backend predicate per CRTC. Fixed in
  revision 6: blackout is answered per target CRTC, both core sweeps filter by
  it, producer waits and per-window order are preserved, and a mixed lit/off
  fixture is added. This changes `yserver-core` and the `Backend` trait,
  inside 3a's scope.
- **The round's unassessed question** — is an unflip requested while admission
  is closed reliably admitted after on? — is added to the evidence: the
  request stays as the admission's unflip intent and is decided after on.

---

## Verdict

**0 blocking, 1 major, 0 minor.**  
Coverage: **INCOMPLETE**

This is a design review. It does not establish that the design is implemented, compiles, or passes tests.

## Incorporation audit

| Round-4 finding | Status | Assessment |
| --- | --- | --- |
| B-1 — post-on unflip can wait for a composed return | **APPLIED, by reassignment** | The 3a design removes unflip from DPMS-off and retains the bound buffer through the ACTIVE-only toggle ([3a §3.5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:188)). The inspected readiness check uses exit retirement, a composed return, and the direct shadow ([admission.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1007)); the inspected scene path retains a composed `Current` until a newer composed buffer replaces it ([scene.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2470)). Reassigning the *missing-return* question to Ciii is sound for an established unflip intent. Section 6 explicitly requires its two open cases to be closed before dependent 3a tasks; that work is pending, not claimed complete. |
| M-1 — Presents received while off lack a completion contract | **PARTIAL** | The design names Copy completion, IdleNotify, and release through blackout, and adds an exact-once fixture ([3a §3.5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:203), [§5.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:371)). Existing blackout code flushes parked Copy work and completions. Its core consumer is global, however; the promised per-CRTC behavior is not yet a complete contract. See M-1. |

The earlier-round rejection, resource-clock, stale-submit, deadline, capability, second-device, and listener corrections remain in the design text. No regression in those stated contracts was established.

## Findings

### Blocking

None established.

### Major

**M-1 — Per-CRTC power cannot be conveyed by the existing blackout flush alone.**

The design says an Owner CRTC’s installed power state makes Presents use Legacy’s blackout path ([3a §3.5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:203)). Today the backend supplies one boolean with no CRTC identity ([backend.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:23360)). The core samples it once, then force-executes **all** source-ready parked Presents and force-flushes **all** queued completions ([process_request.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:10299), [blackout branch](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:10379)). The completion sweep’s `force` bypasses each entry’s MSC due check ([completion sweep](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:12510)).

Consider an Owner CRTC that is off while a Legacy CRTC remains lit—a state allowed by independent device projections ([C.0 REC-5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:844)). A global blackout of `true` completes the lit CRTC’s future-target Present early; `false` can leave the off CRTC’s Present parked against a frozen clock. Changing only the backend predicate cannot give the core’s two queues different answers.

Specify how CRTC identity reaches both blackout sweeps, including filtering of parked executions and completions while preserving producer waits and per-window ordering. Add a mixed lit/off fixture with future-target Presents on both CRTCs. This is an event-delivery contract, beyond an API spelling choice.

### Minor

None established.

## Coverage and implementation checks

- **Incorporation:** Both round-4 findings assessed; rounds 1–3 checked for stated-contract regressions.
- **Architecture and safety:** Checked lifecycle ownership, retained composed-buffer and direct-unflip readiness, and the actual blackout consumer. The ACTIVE-only reassignment does not itself resolve Ciii’s two section-6 cases.
- **Spec and evidence:** Checked the relevant `REC-4/5/6`, ordering, deadline, and Present terminalization provisions, including [C.0 §10.4](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2258). A single-dark-CRTC exact-once fixture cannot prove mixed-domain blackout behavior.
- **Limit:** **24/24 bounded excerpts** beyond the target and prior review, including three earlier reviews. The unflip retirement callback, the path that admits an unflip requested during `Quiescing`, and the umbrella’s detailed implementation gates remain unassessed—not judged sound. The focused follow-up question is whether a direct-unflip request raised while admission is closed is reliably admitted after on. Real compiler, test, portability, and hardware checks belong to implementation; none were run.
