> **Author verification (coordinator, 2026-09-23):** all three findings
> verified in the code before applying. B-1: `resolve_clock_probe`'s `Unknown`
> arm (`owner/device.rs` ~2254) marks the clock failed without releasing the
> slot — CONFIRMED. M-1: CONFIRMED in narrower form — the DPMS description is
> built from the served outputs (`admission.rs` ~1095), so the fix is I-2
> covering every served Owner CRTC plus a missing-record guard; a CRTC first
> activated by modeset/hotplug is 3b/3c. M-2: `begin_clock_probe` acquires the
> slot or fails (`owner/slot.rs` ~150) — CONFIRMED. All applied in plan rev 2.

## Verdict

**1 blocking, 2 major, 0 minor.** Coverage: **COMPLETE FOR DECLARED SCOPE**. This is a design review, not a claim that the implementation builds, passes tests, or is approved.

## Incorporation audit

| Prior review | Status |
| --- | --- |
| None | Skipped as requested. The hardware finding was used as baseline evidence, not treated as a prior review. |

## Findings

### Blocking

**B-1 — An uncertain probe result needs an ExecutorStalled handoff.** The plan says a probe that resolves as a failure leaves DPMS `Deferred(ReadinessClosed)` ([plan, lines 56–63](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:56)). That rule does not distinguish an explicit errno from timeout, IPC loss, or executor failure. C.0 requires the latter to follow `COMMIT-5`/`ExecutorStalled` ([spec, lines 1769–1783](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1769)). The current owner emits `ClockProbeResolved::Unknown` **without releasing the probe slot** ([device.rs, lines 2254–2261](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:2254)). If a probe times out while DPMS waits, treating that event as an ordinary unresolved clock parks DPMS while the slot remains occupied, with no stated stall recovery handoff. Specify separate handling for uncertain outcomes and test the timeout path through the production event route.

### Major

**M-1 — The first activation can wait for a clock that was never installed.** The setup obligation and its test cover CRTCs **active at setup** ([plan, lines 41–48, 112–114](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:41)). A later DPMS-on can include a previously inactive CRTC in `ExpectedCompletionCrtcs` ([admission.rs, lines 1453–1467](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1453)). With no clock record, the proposed wait has no probe to await; submitting a partial clock map is correctly refused by the owner ([device.rs, lines 1343–1373](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1343)). C.0 requires a probe before admitting work on a newly active CRTC ([spec, lines 1769–1783](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1769)). Require the activation path to install and schedule its clock, and cover an initially inactive CRTC that is first activated by DPMS-on.

**M-2 — The scheduling contract lacks a waiting probe before slot acquisition.** The plan requires a pending probe to win the next free slot, and tests it against back-to-back composed frames ([plan, lines 50–54, 115](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md:50)). In the current owner, `begin_clock_probe` acquires the exclusive slot before creating `pending_probe`; it returns an error when another commit or probe holds the slot ([device.rs, lines 2012–2037](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:2012); [slot.rs, lines 150–164](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/slot.rs:150)). Thus an installed clock can have **no pending probe** while another CRTC’s composed commit occupies the slot. A successor commit may win retirement unless a separate waiting intent has priority. The same gap matters when a new clock epoch replaces one whose probe is still in flight. Define who retains unscheduled probe keys and who promotes them at slot retirement; test installation during an occupied slot and replacement of an in-flight epoch. C.0 requires a current result for the new epoch and discarding a stale result ([spec, lines 1776–1784](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1776)).

### Minor

None.

## Coverage and implementation checks

All four checks were performed: incorporation was skipped because there is no prior review; architecture and failure handling were checked against the owner slot, lifecycle submission, and event routing; specification and proposed evidence were checked against the cited C.0 requirements. **24/24 bounded spec and source excerpts** were used. The plan and hardware finding were read once.

The exact production owner setup site and every installer were not verified within the excerpt limit; no conclusion about their soundness is implied. Implementation must establish those sites and run the plan’s assigned formatting, clippy, test, and portability gates with the real compiler and tests. None was run for this review.