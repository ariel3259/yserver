# Stage 3c design — codex review, round 1

**Target:** `docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md`
revision 1 (`d66fbd02`), first review.

**Result:** 3 blocking, 2 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (24/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-26):** all five CONFIRMED against the tree and C.0.

- **B-1** — C.0 `REC-4` puts `DeviceRemoved` above `VTRelease` and the §10 device-removal row withdraws at once; rev 1 deferred every Owner event while released. Rev 2 exempts `Shutdown`/`DeviceRemoved` and keeps the monitor drained while released.
- **B-2** — C.0 §10 VT-release row (spec line ~2050): an unknown release requests helper termination, keeps the unreaped lease in `ExecutorStalled`, and restarts only after old-lease retirement with a fresh incarnation. Rev 1 reinstalled on the old incarnation. Rev 2 splits known vs unknown outcomes; the unknown device stays closed until 3d creates the fresh incarnation.
- **B-3** — the 3b design admits the forced reprobe to the gate because it can publish; rev 1 gave its timeout no terminal rule. Rev 2: the reprobe owns its turn; on timeout its epoch is invalidated, the reply uses published state, a background `TopologyRebuild` follows; named exception.
- **M-1** — `render/backend.rs` `run_display_rescan` ("a card-level probe error aborts the combined rescan"). Rev 2: one episode over every device, nothing applied if any probe fails; `ENODEV` is removal, not failure.
- **M-2** — `kms/hotplug.rs:33` returns one boolean. Rev 2 specifies typed monitor output, the `dev_t` classifier, and tests through it plus one real-monitor test.

## Review as received

## Verdict

**3 blocking, 2 major, 0 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design review. It makes no claim that the design compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior finding | Status |
| --- | --- |
| None | First review; incorporation audit skipped. |

## Findings

### Blocking

**B-1 — Device removal is deferred while the VT is released.** The design defers *any* Owner lifecycle event during release ([3c:111](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:111)), yet says `DeviceRemoved` withdraws outputs and lets a non-renderer device’s server continue ([3c:219](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:219)). Removal outranks VT release, and device loss requires immediate logical withdrawal ([C.0:782](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:782), [C.0:2054](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2054)). If a non-renderer card disappears while away, its outputs remain advertised; acquire then follows the retained “set-master failure exits” rule ([3c:119](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:119); [current acquire:25433](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:25433)). Exempt removal and shutdown from seat deferral. On acquire, classify a removed non-renderer device before applying the global exit rule.

**B-2 — A timed-out release can be followed by reinstall on a still-leased incarnation.** At one second, the design drops master even if an Owner commit remains in flight; it marks the device “reinstall-required,” while explicitly closing it as `Poisoned` only for a dead executor ([3c:95](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:95)). Acquire then specifies a full reinstall ([3c:121](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:121)). If the old helper is still inside an ioctl when the VT returns, that permits new submission before its lease and old fd family are retired. C.0 requires release to stop alias creation, request helper termination, retain an unreaped lease, and withhold restart until the old-lease barrier permits a fresh incarnation ([C.0:2050](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2050)). Make timeout and unknown completion close admission, cancel work still before dispatch, and gate acquire reinstall on that barrier. The later recovery procedure can remain with 3d.

**B-3 — Forced-reprobe timeout has no terminal publication rule.** `GetScreenResources` may reply from published state after a two-second probe deadline ([3c:174](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:174)), but the design does not say whether the timed-out probe can later publish or how its RANDR gate turn ends. A later result could change topology after the next client mutation has validated against the old state. The existing gate admits forced reprobe in publication order precisely because it can publish ([3b:598](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:598)); the umbrella requires Legacy parity except for named exceptions ([umbrella:351](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:351)). Specify gate ownership, probe invalidation and late-result disposition at timeout. If the reply can contain stale resources, name and justify that client-visible exception.

### Major

**M-1 — Per-device probes lose Legacy’s combined-snapshot failure boundary.** The proposed probe result and topology transaction are per device ([3c:164](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:164), [3c:195](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:195)). Legacy first probes *all* devices and leaves the combined topology untouched if one probe fails ([rescan:15865](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:15865); [resume:14969](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:14969)). If device A reports a change while device B has a transient probe error, the proposed flow can publish A’s change where Legacy publishes none. Define a combined discovery and failure boundary for one hotplug or acquire episode, while retaining per-device commits. Test a two-device episode with one failed or delayed probe.

**M-2 — The udev-to-device event contract is missing from the evidence path.** The current monitor reduces all `add`/`remove`/`change` events to one boolean, losing action and device identity ([hotplug.rs:33](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/hotplug.rs:33)). The design requires a remove for an open device node to raise `DeviceRemoved` and an add for an unopened card to raise `DeviceAddedOrReplaced` ([3c:219](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:219), [3c:233](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:233)). Tests that inject those kinds after the monitor could pass while production never produces them. Specify the typed monitor output, its mapping to an open device incarnation, and a test through that production adapter. A stub still cannot prove actual kernel udev delivery.

## Coverage and implementation checks

- **Incorporation:** no prior review.
- **Architecture and contracts:** checked VT, probe, RANDR gate, multi-device publication, and udev event delivery against the cited baseline.
- **Safety and failure:** checked release timeout, stale work, removal during VT absence, and acquire handoff. The exact kernel blocking behavior of master-drop ioctls was not assessed.
- **Spec and evidence:** reviewed the named-test scenarios against production entries. Fixture tests can cover ordering and dispositions, but a stub cannot prove kernel master handoff, actual VT switching, or udev delivery. The proposed hardware cycles cover normal VT and connector changes, not the failure sequences above.

**Excerpts used: 24/24.** Investigation stopped at the reading limit. Actual fixture wiring, worker-fd probe behavior, and 3d teardown ordering remain unassessed; they are not judged sound here. Formatting, full test suites, CI clippy, portability gates, and hardware execution belong to the implementation plans and real compiler/tests ([umbrella:444](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:444)).