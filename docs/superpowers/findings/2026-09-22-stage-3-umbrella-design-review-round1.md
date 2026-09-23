# Stage 3 umbrella design — codex review, round 1

**Target:** `docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md`
revision 1 (`cbc7eb32`), against the C.0 specification as the parent.

**Result:** 2 blocking, 5 major, 0 minor; coverage INCOMPLETE (24/24 excerpts).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
This is the first review on the GPT-6 instrument; nothing here is a trend
against the `da807b70`/`gpt-5.6-sol` rounds.

**Author verification (2026-09-22), every finding checked against the tree:**

- **B-1 — CONFIRMED IN PART.** The device-level half already exists: the 2b
  owner poisons and closes admission (`owner/device.rs:470`, `:545`,
  `poison_unconditionally` at `:703`) and `CompletionUnknown` records are
  quarantined by the ledger. What revision 1 lacked is the lifecycle half: no
  sub-stage before 3d moved the §6.4 state, terminalized the transition or gave
  event ids their dispositions on a failed lifecycle commit. Fixed: 3a executes
  the failure entry edge into `Poisoned`; 3d keeps only the exits (recovery,
  `RecoveryFailed`, `ExecutorStalled`, teardown). The window is fixture-only,
  because production stays `Legacy` until stage 5.
- **B-2 — CONFIRMED.** `drain_ready_crtc_configs` cancels a ready token whose
  client is gone (`core_loop/run.rs:1061`), and the RANDR refresh and change
  notifications live only in that client's continuation
  (`complete_crtc_config`, `process_request.rs:4934`). Today only the PRIME
  qualification probe is asynchronous (`backend.rs:23678`–`23700`: disables
  and probe-less changes return `Applied` synchronously), so cancelling is
  right for a disposable probe and wrong for an Owner commit already on the
  hardware. Fixed: 3b parks every real Owner mutation and splits publication
  (at `Applied`, requester-independent) from the reply (requester only).
- **M-1 — CONFIRMED.** Revision 1 named no effectful owner of the arbiter's
  actions. Fixed: section 2.3.1, the per-device lifecycle driver at the owner
  event routing site (`route_owner_event_batch`, `backend.rs:20801`),
  reporting acknowledged outcomes back to the arbiter.
- **M-2 — CONFIRMED.** No rule said when `dpms_target` follows a topology
  change. Fixed: every topology installation refreshes the projections from the
  current global epoch first; 3a defines it, 3b and 3c prove it at their sites.
- **M-3 — CONFIRMED.** Revision 1 overclaimed: a type cannot know freshness.
  Fixed: section 2.4 now defines one result-disposition boundary in the driver,
  keyed by incarnation, epoch and transition, with the four `REC-4` outcomes;
  the typed tag only makes that check unavoidable.
- **M-4 — CONFIRMED.** Notifications and the reply are written by the core,
  so a `Backend`-trait differential cannot observe their order, a parked
  client or a disconnect. Fixed: layer 1 has two levels; the exit gate drives
  the core request path and compares the bytes on the requester's and a
  listener's connections, including a requester that disconnects while parked.
- **M-5 — CONFIRMED.** C.0 §18.3 said "every use"; appending a stage did not
  change stage 3's exit contract. Fixed: the amendment is applied to C.0 §18
  itself (stage 3 note and a new stage 5), in the same commit as revision 2.

**Unassessed by the reviewer, resolved by the author:** the two direct-scanout
helpers `teardown_direct_before_topology_requery` / `relight_after_direct_teardown`
(`backend.rs:3457`, `:3430`) are the all-off/relight around a topology
change; their callers are RANDR (`:23988`–`24325`, `:3518`, `:3585`) and
the output-topology change (`:14103`). Assigned to 3b, reused by 3c.
**Still unassessed:** whether the physical Owner fixtures support every
proposed hardware cycle — each sub-stage's spec must answer it for its own
cycles.

---

## Verdict

**2 blocking, 5 major, 0 minor**  
**Coverage: INCOMPLETE**

This is a design review, not a claim that the design compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior review | Status |
| --- | --- |
| None | First review; incorporation audit skipped. |

## Findings

### Blocking

**B-1 — Live DPMS precedes its required failure path.** [Design:160](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:160) makes DPMS commits live in 3a, but [design:195](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:195) defers executed poison, recovery, quarantine and stalled-executor handling to 3d. If a 3a DPMS-off commit loses completion evidence, a pure `REC-6` fate table cannot stop admission, retain both possible resource states and run the sole recovery attempt required by [C.0:768](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:768) and [C.0:2044](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2044). Put the executable failure and lease-safe recovery path before the first live lifecycle commit, or defer live DPMS until it exists.

**B-2 — RANDR publication is tied to the requesting client’s lifetime.** The design routes 3b through `begin_crtc_config → Pending → CrtcConfigReady` and requires events and reply at `Applied` ([design:174](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:174), [design:225](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:225)). Today the core cancels a ready token if its client has gone away ([run.rs:1061](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1061)); the RANDR refresh and notifications happen only in that client’s completion continuation ([process_request.rs:4934](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:4934)). An Owner commit can therefore be accepted, the client can disconnect, and hardware can change without notifying other clients. The existing `begin_crtc_config` also returns synchronously for disables and several mode changes ([backend.rs:23685](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:23685), [backend.rs:23780](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:23780)). Specify an Owner path that parks every real mutation and publishes its outcome independently of the requester; cancellation suppresses that client’s reply, not an accepted transition or its broadcast events.

### Major

**M-1 — No effectful owner is assigned to arbiter actions.** The coordinator cannot commit and the arbiter owns no resources; the diagram sends its actions toward admission ([design:68](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:68), [design:105](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:105)). A higher-priority VT release arriving during a submitted topology commit needs prompt admission closure, protocol terminalization and quarantine transfer; admission priority alone cannot perform those effects. [C.0:801](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:801) assigns them to supersession. Name the device-local event-loop owner that applies arbiter actions, owns the resource handoff and returns acknowledged outcomes to the arbiter.

**M-2 — DPMS inheritance lacks a topology handoff.** The coordinator owns the global level while each arbiter owns targets for current outputs ([design:87](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:87), [design:96](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:96)); 3b and 3c introduce or rediscover outputs without stating when that target map is updated. After global DPMS-off, an output discovered during modeset or hotplug could be installed active before inheriting Off, contrary to [C.0:844](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:844). Require topology installation to refresh stable-output projections from the current global epoch first, and to invalidate removed projections exactly once.

**M-3 — Tier typing does not itself prove result freshness.** The proposed `(LifecycleTransitionId, LifecycleEpochId)` tag is useful correlation, but a type cannot know whether a later executor reply belongs to the *current* incarnation and transition ([design:111](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:111)). After replacement, a late success still needs adoption or closure of returned fds and accepted-stale quarantine, even when it cannot publish installed state ([C.0:814](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:814)). Specify one owner-side validation and result-disposition boundary using incarnation plus lifecycle and transition identity; typed tags should make that check unavoidable, rather than claim compile-time freshness.

**M-4 — The differential gate cannot observe its claimed protocol order.** [Design:240](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:240) drives only the `Backend` trait yet claims to compare RANDR notifications with a request’s reply. The core emits notifications and then writes the reply in `complete_crtc_config` ([process_request.rs:4934](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:4934)); backend calls alone do not exercise parking, disconnect or wire order. Keep backend differential tests for backend state, and make the protocol timing exit gate drive the core request and completion path with an Owner fixture.

**M-5 — Adding stage 5 leaves stage 3’s parent requirement unresolved.** The design retains legacy lifecycle writers and `kms_outputs_active` through stage 4 ([design:120](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:120)), while authoritative [C.0 §18.3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:4056) assigns conversion of **every use** of that model to stage 3. Retaining `TransportState` while production remains Legacy is justified, but merely appending stage 5 ([design:305](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:305)) does not revise stage 3’s exit contract. Amend §18.3 and its stage boundary explicitly before treating a 3d exit with those uses present as spec-compliant.

## Coverage and implementation checks

- **Incorporation:** no prior review.
- **Architecture and safety:** checked `REC-1..6`, ordering, recovery handoffs, the RANDR continuation and the admission seam. Findings B-1 through M-3 cover the demonstrated dependency and ownership gaps.
- **Compliance and evidence:** checked §16’s relevant lifecycle cases and §18.3; M-4 and M-5 identify exit-gate and authority gaps. The design assigns format, clippy, tests and portability gates to later implementation; none ran here.
- **Reading limit:** **24/24 bounded excerpts** beyond the once-read design; no builds, tests or hardware runs. The named `relight_after_direct_teardown` and `teardown_direct_before_topology_requery` call paths, and whether physical Owner fixtures can support each proposed hardware cycle, remain unassessed. A focused follow-up should resolve those two assignments; this review makes no soundness claim about them.
