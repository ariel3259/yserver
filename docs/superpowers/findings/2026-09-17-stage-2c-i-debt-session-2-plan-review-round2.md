# Stage 2c-i debt, session 2 plan — codex review, round 2

**Target:** `docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md` revision 2 @ `12a32854`
**Result:** 2 blocking, 1 major, 0 minor; coverage complete for declared scope (10/12 excerpts).
Round 1 (`…-round1.md`, same instrument, so the counts compare): 3 blocking, 2 major.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `783089b4`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.154.0`.
Counts are comparable only to other reviews citing this same instrument SHA.

## Author verification (2026-09-17)

- **B-1 — CONFIRMED against the tree, and fixed.** `transport.rs` had five
  raw `self.state` checks (`begin_quiescing`, `authorize_owner_write`,
  `consume_owner_write`, `issue_handover_permit`, `publish_owner`), so a
  close arriving through a `TransportGateHandle` — which owns no
  `&mut TransportGate` — left every transition open. All five now read the
  effective `state()`. Proven by
  `c0_2ci_service_driven_close_is_terminal_for_the_gate`, verified to fail
  when any one is reverted. The three census tags whose condition text
  changed were updated with it.
- **B-2 — CONFIRMED against the tree, and fixed.**
  `scene::present_error_is_device_lost` matched `PresentError::Vk(
  ERROR_DEVICE_LOST)` exactly, and the scene tick latches
  `platform.renderer_failed` on it, so flattening the cause into
  `PresentError::Io` did lose a device loss. The cause now stays structural
  in a new `PresentError::ManagedUnwind`, and the classifier recurses.
  Proven by `c0_2ci_failed_unwind_keeps_a_device_loss_recognisable`, verified
  to fail when the recursion is removed.
- **M-1 — taken in the honest direction.** The user's decision was to accept
  that the tests drive `managed_submit_failure` rather than the real path;
  the reviewer's objection is to the plan *amending* 4.2's criterion to
  match. So revision 3 records the real-path half as **not met** and open,
  beside 4.3's F8 stop, instead of rewriting it, and Task 6 carries both into
  the acceptance record.
- **Incorporation audit** (round 1's findings): B-1, B-2, B-3 APPLIED, M-2
  PARTIAL by the user's stated boundary, M-1 carried forward and now
  answered as above.

Revision 3 (`docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md`)
carries all of it, re-prototyped and re-measured: 179 `c0_2ci` tests, 18
hardware, clippy, fmt, musl and FreeBSD clean, 39 + 3 tagged sites
`CAUGHT_BY_ORACLE`.

No implementation has been dispatched.

---

## Verdict

**2 blocking, 1 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result only. It does not claim that the plan compiles, tests pass, or is approved for implementation.

## Incorporation audit

| Prior finding | Disposition | Basis |
|---|---|---|
| B-1 — husk token discharged while alias remained alive | **APPLIED** | The BO’s DRM alias now moves into `PoolHuskRegistration`; successful unregister drops the alias and decrements its count together ([plan lines 302–351](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:302)). Dropped, foreign, and unknown registrations poison certification. |
| B-2 — service could close an unrelated transport | **APPLIED** | Handles now carry device/incarnation and use the shared closure cell as instance identity; installation rejects foreign identity and replacement ([plan lines 883–907](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:883), [lines 918–956](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:918)). This does not resolve the separate terminal-state defect B-1 below. |
| B-3 — reset test modelled generation replacement | **APPLIED** | The test and commit text now explicitly claim only forced teardown and allocation-key isolation. The undriven real replacement is recorded as the F8 stop authorized by spec 4.3 ([plan lines 1226–1229](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:1226), [spec lines 237–264](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:237)). The acceptance record also preserves the open crossing. |
| M-1 — real `scene.rs` path replaced by inspection | **NOT APPLIED** | The plan still tests `managed_submit_failure` directly and substitutes review-time inspection of the two callers ([plan lines 1048–1180](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:1048), [line 1198](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:1198)). Its proposed amendment does not satisfy the currently authoritative acceptance clause. Carried forward as M-1. |
| M-2 — handover evidence self-asserted | **PARTIAL** | The revision adds exhaustive per-class typed evidence and device/incarnation-bound reservations, but deliberately does not establish evidence provenance through writer/recipient fixtures ([plan lines 1562–1577](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:1562), [lines 1722–1821](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:1722)). That limitation is honestly recorded and, under the user’s stated boundary, live issuers remain stages 3/4 rather than a finding here. |

## Findings

### Blocking

#### B-1 — Handle-driven closure is not terminal for the gate state machine

`TransportGateHandle::close_gate` only sets `forced_closed`; it cannot update the gate’s stored `state` ([transport.rs lines 211–237](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/transport.rs:211)). Although `state()` overlays `Closed`, several transitions inspect raw `self.state`: `begin_quiescing`, `authorize_owner_write`, `consume_owner_write`, `issue_handover_permit`, and `publish_owner` ([transport.rs lines 275–327](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/transport.rs:275), [lines 367–419](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/transport.rs:367), [lines 454–498](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/transport.rs:454)).

Concrete sequence: uncertain GPU submission invokes the plan’s service handle close; the gate’s raw state remains `Legacy`; `begin_quiescing` succeeds, permit issuance and Owner publication succeed, and `authorize_owner_write` can mint a grant even though `state()` reports `Closed`. Tests only assert the displayed state, so this non-terminal behavior survives them. It violates the required affected-transport closure semantics ([spec lines 199–204](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:199)). R8 makes it dormant, not a correct mechanism.

Smallest correction: make every transition and grant operation consult the effective `state()`/closure flag, or place authoritative state in shared storage so handle closure performs an actual terminal transition. Add a test attempting quiescence, permit publication, and grant issuance after service-driven closure.

#### B-2 — Aggregating an unwind error hides fatal Vulkan device loss

When unwind also fails, `managed_submit_failure` replaces the original typed `PresentError` with an `Io` error containing formatted text ([plan lines 1007–1026](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:1007)). The enclosing event path recognizes device loss only as the exact `PresentError::Vk(ERROR_DEVICE_LOST)` variant and latches `renderer_failed` on that classification ([scene.rs lines 140–142](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:140), [lines 4864–4871](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:4864)).

Concrete sequence: Vulkan returns `ERROR_DEVICE_LOST`; cancellation or freezing also fails; the helper returns `PresentError::Io`; the caller no longer recognizes device loss and does not enter its fatal renderer state, permitting retry bookkeeping against a lost device. The proposed tests use `NoFb` and only inspect formatted text, so they cannot detect this regression.

Smallest correction: preserve the primary cause structurally—such as an aggregate error carrying both cause and unwind failure—and make device-loss classification recurse through it. Add a device-loss-plus-unwind-failure test at the consuming error handler.

### Major

#### M-1 — The plan weakens, rather than satisfies, the authoritative real-path criterion

The spec requires deletion of either propagation or closure to fail a named test driving the real `scene.rs` path ([spec lines 193–207](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:193)). The plan’s tests invoke the extracted helper directly, while the two real call sites are checked only by reading. Deleting or bypassing either call leaves every named test and oracle result green.

The plan proposes amending the spec to accept this weaker evidence ([plan lines 117–128](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md:117)), but an implementation plan cannot establish compliance with its authoritative input by rewriting the unmet criterion during Task 1.

Smallest correction: add a non-hardware seam that exercises each real failure arm and mutation-tests removal of its helper call. Otherwise retain 4.2 as an explicit unmet acceptance criterion rather than marking the session accepted.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** audited all five round-1 findings. B-3’s F8 boundary and M-2’s deferred provenance are honestly stated and do not claim the deferred production issuers.
- **Architecture/contracts:** checked husk alias ownership, gate/service correlation, handle-driven closure, submission-error delivery, reset scope, and handover evidence boundaries.
- **Safety/failure semantics:** found a non-terminal asynchronous close and loss of fatal device-error identity.
- **Spec/verification:** checked sections 4.1–4.4, 5, 6, and 8. Hardware, census, formatting, regular clippy, feature builds, workspace tests, musl, and FreeBSD gates are assigned.
- **Excerpts used:** **10/12** beyond the plan and prior review.
- **Unassessed:** detailed implementation outside the concrete hypotheses above and the real generation-replacement crossing recorded by F8; these are not inferred sound.
- **Deferred to implementation:** Rust typing/borrowing, patch applicability, compilation, lint output, test results, mutation execution, hardware behavior, portability builds, and claimed census counts.