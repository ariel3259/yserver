## Verdict

**3 blocking, 1 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** `docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md`
revision 1 (`46f6d516`), a stage **design**, against the stage-2c design as the
passed parent, with C.0 §9/§7.1/§14 and 2c-i §6 named in the context.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `69c6d6e2`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.154.0`.
Counts are comparable only to other reviews citing this same instrument SHA.
**Not comparable** to any earlier round: `69c6d6e2` raised the effort from
`medium` to `xhigh`.

**Recorded usage:** 51,709 tokens reported by the completed process (exit 0),
excluding the author session. 12/12 bounded excerpts used.

**Author verification (2026-09-18), every finding checked against the tree:**

- **B-1 — CONFIRMED.** C.0 line 1505 ("round-robin across CRTCs") and line 1591
  ("a continuously ready primary CRTC may not take two successive device slots")
  are per CRTC; revision 1's per-unit accounting admits the reviewer's `AB`→`A`
  sequence. Fixed in revision 2: units are storage only, fairness per CRTC, a
  multi-CRTC admission serves every CRTC it covers.
- **B-2 — CONFIRMED.** `DeviceCommitOwner::begin_with_context`
  (`device.rs:1322`) installs the live record and takes the ledger; `send_on`
  (`device.rs:1497`) can then refuse before IPC and retire the record as
  `NeverDispatched`. Fixed: `lock` → `begin` → `send_on` → `confirm`/`abort`,
  the mismatch check moved to `lock`, before the owner holds anything.
- **B-3 — CONFIRMED.** C.0 line 1558: "the oldest ready synchronous primary
  generation for every ready CRTC". Fixed: tier 5 takes every ready CRTC.
- **M-1 — CONFIRMED.** C.0 lines 1587–1589: barriers "may interrupt this bound
  and are measured separately, but cannot reset surviving tickets". Fixed: barriers
  counted apart, three ticket-lifecycle mutations added.

Fixing B-2 exposed a question C.0 does not answer (absorbed maintenance after a
post-dispatch kernel rejection); revision 2 records it as open, section 11.1.

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| — | SKIPPED | No prior review was supplied; check 1 does not apply. |

## Findings

### Blocking

#### B-1 — Per-unit round-robin violates the authoritative per-CRTC fairness contract

The design deliberately changes fairness accounting from CRTCs to ownership units ([target lines 87–100](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:87), [199–209](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:199)). The authority instead requires round-robin across CRTCs and forbids a continuously ready CRTC from taking two consecutive slots while another ready CRTC is owed service ([C.0 lines 1505–1515](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1505), [1591–1595](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1591); [stage 2c lines 134–139](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:134)).

Concrete failure: grouped unit `AB` is admitted while composed intents for A and B remain ready. Because `AB`, `A`, and `B` are distinct unit participants, composed A may win next without any unit taking consecutive turns. CRTC A nevertheless occupied both consecutive commits while B had ready work, violating the normative rule.

Correction: retain units as storage/resource containers, but account fairness per affected CRTC. A multi-CRTC admission must mark every included CRTC served, and eligibility must reject any request containing a CRTC that would take a consecutive turn while another ready CRTC is owed service. Test grouped-to-composed and composed-to-grouped transitions.

#### B-2 — Successful `begin` is not a safe confirmation boundary

The design consumes tickets, ageing, and turns immediately after `begin` succeeds, before dispatch ([target lines 224–238](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:224), [246–268](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:246)). In the actual owner, `begin` installs a live record, reserves the slot, and takes the resource ledger ([device.rs lines 1322–1354](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1322)). The subsequent send may still be proven refused before IPC, terminalizing the record as never dispatched ([device.rs lines 1489–1507](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1489)).

Concrete failure: an aged cursor wins; `begin` succeeds; decider confirmation consumes its ticket; `send_on` returns `Stalled` or `ReservationMismatch` before IPC. The owner releases the record, but no KMS request was dispatched and the desired maintenance identity has lost its ticket. The exact failure described at target lines 224–228 therefore remains possible after `begin`.

The generation-mismatch branch has the dual problem: mismatch occurs after the owner has accepted resources into a live, undispatched record, yet the design merely says to close transport and does not require `cancel_live` or processing of its returned ownership events.

Correction: define a transaction token spanning decider and owner preparation. Validate/lock the decision before `begin`; finalize fairness state only after dispatch crosses the send boundary; abort without consumption on proven pre-dispatch refusal; and specify the resource/protocol disposition for cancellation and uncertain send. Add tests for `begin` success followed by every pre-IPC refusal, not only `begin` refusal.

#### B-3 — Tier 5 permits partial bundles that can starve a ready CRTC

Tier 5 requires only “at least two CRTCs” and discusses included generations without requiring inclusion of all ready group members ([target lines 165–173](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:165), [211–214](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:211)). C.0 requires the oldest ready synchronous generation for **every ready CRTC** in the qualified group ([C.0 lines 1558–1568](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1558)).

With A, B, and C ready, the design allows an A+B bundle. A continuous A/B stream can keep tier 5 eligible and exclude C indefinitely; C earns neither logical retirement nor tier-6 service.

Correction: require every ready synchronous CRTC in the qualified group, with canonical completion coverage and its exact oldest ready generation. Add a three-or-more-CRTC mutation that deliberately drops one ready member.

### Major

#### M-1 — The bound and exit evidence omit required ticket-lifecycle cases and barrier accounting

The target states an unconditional in-flight-plus-`N−1` bound ([lines 216–220](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:216)). C.0 expressly allows finite topology/unflip/recovery barriers to interrupt that bound, requires them to be measured separately, and forbids them from resetting surviving tickets ([C.0 lines 1579–1602](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1579)). For `N=1`, a valid tier-1 barrier would otherwise appear to violate the target’s invariant and could spuriously close transport.

The exit matrix tests generic ageing but not ticket preservation through payload replacement, a new ticket arriving while the old identity is submitted, barrier interruption, or surviving-ticket continuity ([target lines 353–369](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:353)). An implementation that resets an aged cursor’s ticket on replacement can pass the listed mutation.

Correction: specify the base bound plus separately counted finite barriers, and add named mutations for replacement resetting a ticket, barrier resetting relative age, and reusing the consumed ticket for an update arriving while submitted.

## Coverage and implementation checks

- Incorporation: skipped because no prior review exists.
- Architecture/contracts: checked decider/conductor ownership, grouped admission, owner preparation/send boundaries, and retirement ordering.
- Safety/failure: checked resource transfer at `begin`, proven pre-IPC refusal, cancellation, exact ticket consumption, and grouped retirement identity.
- Compliance/verification: checked all seven tiers, per-CRTC fairness, ageing, absorption, bundle membership, bounds, and the exit mutations.

**Excerpts used: 12/12.** Verified ground included C.0 §§9–9.4, stage-2c §§2/4/6/7, 2c-i §6, and targeted owner event/`begin`/send/retirement code. Full coordinate-transport mechanics, C.1 implementation mechanics, hardware qualification, exact module plumbing, compilation, and test-fixture APIs were not assessed because they are explicitly deferred or outside this review.