# Stage 2c-i debt, session 2 plan — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md` @ `66074094`
**Result:** 3 blocking, 2 major, 0 minor; coverage complete for declared scope (12/12 excerpts).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `783089b4`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.154.0`.
Counts are comparable only to other reviews citing this same instrument SHA.

## Author verification (2026-09-17)

- **B-1 — CONFIRMED against the tree.** `ScanoutBo` keeps its own
  `drm: Rc<crate::drm::Device>` field (`vk/scanout.rs`), and
  `detach_managed_entries` leaves the bo in its pool, so consuming the
  registration at detach lets the alias count reach zero while the alias
  lives. The discharge site is inherited from F8-M1 (whose comment already
  says the alias "outlives the managed key"); the plan kept it. The token
  must be discharged where the alias actually ends, which needs a design
  decision: the bo's `Drop` cannot reach the registry.
- **B-2 — CONFIRMED as described.** `TransportGateHandle` is a bare
  `Rc<Cell<bool>>` with no identity, and the plan's `set_transport_gate`
  accepts any handle. Small fix: carry device and incarnation in the handle
  and refuse a mismatched install.
- **B-3 — CONFIRMED against the spec.** Spec 4.3 says an undrivable
  generation-replacement half is an F8 stop to report; plan correction 3
  modelled it instead. Needs the user's call: record the F8 stop (test only
  the forced half plus XID reuse, claiming no more) or build a harness that
  drives `reset_generation`.
- **M-1 — known trade, flagged by the author before the review.** Decision
  pending with B-3.
- **M-2 — PLAUSIBLE, not yet verified.** The spec's own wording (4.4) asks
  for explicit per-class evidence, which the plan supplies; whether it must
  instead come from fixtures that install or disable each writer and a live
  `RecipientSlot` is a scope question to settle before the plan's revision.

No implementation was dispatched. The plan is not approved.

---

## Verdict

**3 blocking, 2 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result only. It does not claim that the plan compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Disposition | Basis |
|---|---|---|
| None | Skipped | The user states this is the first review. |

## Findings

### Blocking

#### B-1 — The husk token is discharged while the alias it accounts for remains alive

The specification requires the token to represent a live husk alias and prohibits certification of zero aliases while such a husk remains live ([spec §4.1, lines 168–189](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:168)). The plan nevertheless consumes the registration in `detach_managed_entries`, while explicitly acknowledging that the BO’s `self.drm` alias is untouched and outlives the managed key ([plan lines 361–372](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:361)). Baseline code confirms that this inventory means “no non-payload alias remaining” and gates the registry’s last close ([drm_cleanup.rs lines 128–157](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:128), [lines 388–439](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:388)).

Concrete sequence: a managed BO receives a registration; detach drops its lease and consumes the registration; the BO and its `Rc<Device>` remain in the pool; the counter reaches zero; `try_mint_file_family_closed` mints while the alias remains alive. The proposed positive test tests a bare token, not this BO lifecycle ([plan lines 480–490](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:480)).

Smallest correction: retain the registration until the BO’s actual DRM alias is dropped, or make detach remove/drop that alias before consuming the token. Add an end-to-end assertion that minting remains blocked while the real BO alias lives and succeeds only after it is gone.

#### B-2 — `ResourceService` cannot identify the “affected transport”

The plan adds an arbitrary `TransportGateHandle` slot and later closes whichever handle is installed ([plan lines 645–672](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:645)). But the handle contains only an `Rc<Cell<bool>>`, with no device, incarnation, or gate identity ([transport.rs lines 211–224](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/transport.rs:211)), while `ResourceService` itself is bound to a device and incarnation ([resources/mod.rs lines 140–176](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:140)).

Concrete sequence: a foreign or replacement gate handle is installed accidentally; an uncertain submission closes that gate; the transport associated with the affected service remains open. This violates the requirement to close the affected transport ([spec lines 199–204](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:199)). R8 makes the defect dormant, not safe.

Smallest correction: bind the handle to device, incarnation, and a unique gate instance; make installation validate the service identity and reject replacement unless a defined handoff proves the old association has no outstanding work. Test foreign and replacement installation.

#### B-3 — The reset correction bypasses the boundary it claims to prove

The specification requires continuation across the reset’s actual generation replacement and says inability to drive that half is an F8 stop ([spec lines 237–264](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:237)). Correction 3 instead manually constructs a fresh `ServerState` ([plan lines 1089–1099](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:1089)) and then claims the reset invariant is proven ([plan lines 897–899](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:897)).

The actual boundary also bumps the generation, shuts down setup producers, force-destroys clients, cancels queued backend work, and filters stale producers ([reset.rs lines 307–369](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/reset.rs:307)). A defect in any connection between those operations and state replacement survives the proposed test because the test performs replacement itself.

Smallest correction: remove correction 3 and either drive `reset_generation` through a suitable integration/test harness or record the specified F8 stop. Do not claim the generation-replacement invariant from manual state construction.

### Major

#### M-1 — Section 4.2’s real-path mutation criterion is replaced by inspection

The authoritative acceptance criterion requires a named test driving the real `scene.rs` path and failing if propagation or closure is deleted ([spec lines 193–207](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:193)). The proposed tests call `managed_submit_failure` directly ([plan lines 781–852](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:781)); bypassing either real call site therefore leaves every test and census result green. The plan explicitly substitutes “checked by reading” ([plan lines 871–872](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:871)), contradicting the claimed self-reporting acceptance property ([spec lines 322–324](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:322)).

Smallest correction: introduce a non-hardware seam around the real failure branches and mutation-test both call sites. Otherwise explicitly leave 4.2 acceptance incomplete rather than amending it to manual inspection.

#### M-2 — Handover “evidence” remains self-asserted and unconnected to a recipient

`WriterCoverageProof::new_for_tests` ignores freely constructed labels, while `permit_for` fabricates a reservation directly from the gate’s own identifiers without installing a recipient slot ([plan lines 1405–1457](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:1405), [lines 1513–1521](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:1513)). Thus tests can still certify Owner handover with no demonstrated mediated/disabled writers and no teardown receiver, the precise forbidden condition identified by the spec ([spec lines 268–288](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:268)).

Smallest correction: mint coverage witnesses from fixtures that actually install or disable each writer class, and mint reservations only through a live `RecipientSlot`/supervisor fixture. Keep production issuers deferred.

### Minor

None.

## Coverage and implementation checks

- Incorporation: skipped; no prior review.
- Architecture/contracts: checked husk accounting ownership, service-to-gate correlation, reset-boundary fidelity, and handover evidence.
- Safety/failure semantics: checked alias lifetime, identity correlation, uncertain-submission closure, late-proof isolation, and evidence provenance.
- Specification/verification: checked sections 4, 5, 6, and 8. The plan assigns formatting, regular clippy, feature builds, workspace tests, musl/FreeBSD checks, census, and hardware execution appropriately.
- Excerpts used: **12/12**. Investigation stopped at the stated limit.
- Unassessed: detailed callers outside the named production paths and the remainder of reset event-loop implementation. They are not inferred sound.
- Deferred to implementation: Rust typing/borrowing, exact test compilation, patch applicability, lint output, target buildability, runtime tests, hardware results, and census counts.