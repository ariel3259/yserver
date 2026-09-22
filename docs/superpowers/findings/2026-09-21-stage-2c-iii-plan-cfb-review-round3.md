# Plan Cfb — codex review, round 3

**Target:** plan Cfb revision 3 (`8dd345df`), with round 2 as the prior review.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
Coverage **COMPLETE FOR DECLARED SCOPE** (24/24).

**Author verification (2026-09-21), every finding checked against the tree:**

- **B-1 — CONFIRMED.** Only the adoption transaction still holds `Preparing`;
  a normal-completion cleanup failure has no charge. Fixed: charge by origin —
  the final role stays occupied (`finish_role` deferred) for the late case;
  spec 2.6 amended (revision 6); F20b/F20c and a test with `Preparing` occupied.
- **B-2 — CONFIRMED.** The scene ignores `service_completions`' `Result`
  (`scene.rs:3915`, `:3965`) and the handler returns `bool`. Fixed: edges are
  staged inside the service and only the backend step takes them; the service
  revalidates at transfer time; F31 covers the scene sites, F33 the remint.
- **m-1 — CONFIRMED.** The plan cites revision 5.
- Incorporation: M-1, M-2, M-3 APPLIED; B-1 PARTIAL closed here.

Revision 4 incorporates all three.

---

## Verdict

**2 blocking, 0 major, 1 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Round-2 finding | Status | Assessment |
|---|---|---|
| B-1 — service-step cross-layer contract | **PARTIAL** | Revision 3 adds recorded zero edges, moves backend servicing after owner-event routing, and defers destruction ([plan:38](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:38)). It still does not define how results consumed by the scene reach the backend, or require cleanup-time revalidation. See B-2. |
| M-1 — `RoleReservation` is not proof | **APPLIED** | Decision 10 requires an opaque capacity/device/incarnation/service-bound permit and rejects foreign, stale, and wrong-role permits through F9a–F9c ([plan:37](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:37), [plan:93](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:93)). Existing independent `Current`, `Successor`, and `Preparing` slots allow preparation while Current is occupied ([capacity.rs:7](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/capacity.rs:7), [capacity.rs:136](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/capacity.rs:136)); role moves carry the existing lease, while adoption/reuse mints under `Preparing`. |
| M-2 — adoption-failure trigger | **APPLIED** | Seeding `next_generation` to its last value passes the `exhausted` preflight and fails inside the checked increment ([resources/mod.rs:368](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:368)); the plan names that trigger explicitly ([plan:100](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:100)). |
| M-3 — no compliant real-import producer | **APPLIED** | The split and real-import helper are assigned to Task 1 ([plan:40](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:40)). `ProbeFbOwnership::Legacy` carries the device, FB, and GEM needed by adoption ([modeset.rs:1399](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/drm/modeset.rs:1399)); the existing fixture deliberately uses a matching real primary fd without master for PRIME/ADDFB2 ([backend.rs:7029](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:7029)). Actual ioctl success remains a GPU test. |

## Findings

### Blocking

#### B-1 — Normal cleanup failure has no capacity charge it can retain

Task 2 applies the keyless pending-cleanup owner both to post-conversion adoption failure and to “any later registry cleanup,” requiring each entry to retain a `Preparing` charge ([plan:135](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:135)). The specification’s charge belongs to the adoption transaction, where `Preparing` is still reserved ([spec:192](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:192), [spec:215](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:215)).

Concrete failure: a Current allocation retires, its final lease and obligation disappear, its direct role is finished, and registry `RMFB` then fails. There is no `Preparing` charge to transfer. Reserving one can collide with a new candidate; retaining the payload without one defeats bounded accounting. The proposed tests exercise the adoption-failure path and do not establish this later-cleanup case.

Smallest correction: separate the two failure paths. Define a bounded charge/owner for failure after normal role completion—such as retaining the actual final role until cleanup disposition, or a distinct cleanup quota—and add a test where ordinary last-lease cleanup fails while `Preparing` is occupied.

#### B-2 — Phase-A results lack a durable, revalidated handoff to Phase B

Decision 11 says `service_completions` returns zero edges and releasable keys, while the backend consumes them after the scene drain ([plan:38](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:38)). But both scene calls currently discard the successful return value ([scene.rs:3915](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:3915), [scene.rs:3965](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:3965)), and the handler returns only `bool` ([scene.rs:3856](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:3856)). The plan specifies neither threading that batch outward nor leaving it durably queued for the backend.

Thus a scene pass can take the last-lease edge and discard it, leaving the M1 index permanently live. Conversely, if “releasable” is a snapshot retained until Phase B, a newly minted lease can make that snapshot stale; cleanup must not proceed merely because it was releasable earlier. Spec §3.4 permits destruction only with no lease ([spec:278](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:278)).

Smallest correction: define one durable batch protocol. Scene servicing must only stage events or return them through its drain result; the backend alone takes/acknowledges them. After index removal, the service must atomically revalidate current leases/obligations before transferring a payload for cleanup. F31/F33 must cover the scene sites plus remint-before-authorization.

### Major

None.

### Minor

#### m-1 — The plan cites the wrong authoritative spec revision

The plan directs implementation against revision 4 ([plan:22](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:22)), while the authoritative document is revision 5 and says 4.2a was added specifically for this plan ([spec:3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:3), [spec:317](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:317)). Update the revision reference so the fixture requirement cannot be treated as non-authoritative.

## Coverage and implementation checks

- Incorporation: all four round-2 findings audited.
- Architecture: service/scene/backend boundaries, capacity overlap, permit placement, and real-import ownership examined.
- Safety: zero-edge/remint ordering, cleanup authorization, and failed-cleanup ownership traced.
- Specification/evidence: §§2.1–2.6, 3.1–3.4, and 4.2a–4.3 checked.
- Excerpts used: **24/24** beyond the once-read plan and prior review.
- Unassessed—not sound: exhaustive storage-replacement enumeration, the complete frontmost-window eligibility path, and every existing terminal implementation.
- Exact Rust APIs/borrows, compilation, clippy, portability targets, mutations, test results, and real GPU behavior remain deferred to implementation.