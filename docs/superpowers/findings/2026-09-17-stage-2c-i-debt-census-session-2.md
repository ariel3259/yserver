# Stage 2c-i debt, session 2 — accepted: 71 census sites, zero survivors

**Stage:** Phase C.0, stage 2c-i debt, session 2 (mechanism changes).
**Spec:** `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` section 4.
**Plan:** `docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md`, revision 6.
**Implemented by:** codex, model `gpt-5.6-luna`, reasoning effort `xhigh`, `--sandbox workspace-write`.
The coordinating session verified every task against the plan byte for byte and committed it.

## Result

The census over the full enumeration of the resource service's refusal
surface — `resources/{mod,commit,gpu,transport}.rs`, hardware included —
reports **zero survivors**, with every guard this stage claims proven by the
test its tag is bound to:

| File | Sites | By oracle | Caught | Survivors |
| --- | --- | --- | --- | --- |
| `mod.rs` | 36 | 21 | 15 | 0 |
| `commit.rs` | 13 | 8 | 5 | 0 |
| `gpu.rs` | 3 | 2 | 1 | 0 |
| `transport.rs` | 19 | 8 | 11 | 0 |
| **Total** | **71** | **39** | **32** | **0** |

Session 1 left eight survivors, all of them session-2 scope; all eight are
now `CAUGHT_BY_ORACLE`. The enumeration grew from 68 sites to 71: the
reservation guard of `issue_handover_permit` and the two identity guards of
`ResourceService::set_transport_gate` are new production guards this session
adds.

The three guards added to `drm_cleanup.rs` sit outside the census's default
files and were proven separately, all `CAUGHT_BY_ORACLE`:
`python3 tools/guard-census.py --files drm_cleanup.rs --deterministic-only --require-oracle`.

## The guards this session proves

| Marker | Site | Bound test | Strategy |
| --- | --- | --- | --- |
| A-permit-outstanding | issue_handover_permit `self.outstanding_owner_writes != 0` | c0_2ci_guard_handover_permit_refuses_with_an_outstanding_grant | mutate_if_false |
| A-permit-quiescing | issue_handover_permit `self.state() != TransportState::Quiescing` | c0_2ci_guard_handover_permit_refuses_outside_quiescing | mutate_if_false |
| A-publish-identity | publish_owner `permit.device != self.device || permit.incarnation != self.incarnation` | c0_2ci_guard_publish_owner_refuses_a_permit_for_another_incarnation | mutate_if_false |
| A-publish-outstanding | publish_owner `self.outstanding_owner_writes != 0` | c0_2ci_guard_publish_owner_refuses_with_an_outstanding_grant | mutate_if_false |
| A-publish-quiescing | publish_owner `self.state() != TransportState::Quiescing` | c0_2ci_guard_publish_owner_refuses_once_the_gate_left_quiescing | mutate_if_false |
| B-consume-owner-write-state | consume_owner_write `self.state() != TransportState::Owner` | c0_2ci_guard_consume_owner_write_refuses_once_the_gate_left_owner | mutate_if_false |
| S2-cancel-pre-submit-error | cancel_pre_submit_batch `Some(err) =>` | c0_2ci_guard_failed_pre_submit_cancel_is_reported_and_closes_transport | mutate_arm |
| S2-freeze-uncertain-error | freeze_uncertain_batch `Some(err) =>` | c0_2ci_guard_failed_uncertain_freeze_is_reported_and_closes_transport | mutate_arm |
| S2-gate-install-identity | set_transport_gate `gate.device() != self.device || gate.incarnation() != self.incarnation` | c0_2ci_guard_service_refuses_a_transport_gate_for_another_transport | mutate_if_false |
| S2-gate-install-replacement | set_transport_gate `let Some(installed) = &self.transport_gate && !installed.same_gate(&gate)` | c0_2ci_guard_service_refuses_a_second_transport_gate | mutate_if_false |
| S2-permit-reservation-identity | issue_handover_permit `reservation.device != self.device || reservation.incarnation != self.incarnation` | c0_2ci_guard_handover_permit_refuses_a_reservation_for_another_recipient | mutate_if_false |

## What is NOT claimed

Two acceptance criteria of spec section 4 are **open**, deliberately and on
the record — the measurement above does not cover them, and nothing here
should be read as covering them:

1. **4.2's real-path half.** The named tests drive
   `scene::managed_submit_failure`, which both failure arms of
   `submit_shared_scanout_frame` return as their `Err` value. They do not
   drive the real path: failing its unwind needs Vulkan, DRM and fault
   injection. That the two call sites still call the helper was checked by
   reading, at review (codex round 1 M-1, carried forward as round 2 M-1).
2. **4.3's generation crossing.** `reset_generation` is `pub(crate)` in
   `yserver-core` and needs a live poller, setup registry and input
   inventory, so the test drives the forced teardown through
   `force_destroy_all_clients` and then reuses the numeric XIDs over the same
   backend. Spec 4.3 calls an undrivable half an F8 stop to report rather
   than a reason to add wiring; this is that report. The reset's invariant 6
   remains unproven here.

Also unchanged: production issuers for the handover evidence stay absent
(R8), and minting coverage witnesses from writer fixtures and reservations
from a live `RecipientSlot` remains stages 3/4 work (round 1 M-2, PARTIAL by
decision).

## Reviewer mutations

Every mutation the plan names, run against the accepted tree, each confirmed
to have compiled. Named test first; a mutation that also breaks other tests
lists only its own here.

| Mutation | Test that caught it |
| --- | --- |
| `PoolHuskRegistration::drop` without the poisoning | `c0_2ci_guard_dropped_husk_registration_closes_the_family_barrier` (+2) |
| `unregister_pool_husk` forgets the alias instead of dropping it | `c0_2ci_husk_registration_owns_the_alias_it_counts` |
| no transport close on the uncertain branch | `c0_2ci_guard_failed_uncertain_freeze_is_reported_and_closes_transport` (+1) |
| no transport close when the pre-submit cancel fails | `c0_2ci_guard_failed_pre_submit_cancel_is_reported_and_closes_transport` |
| `present_error_is_device_lost` without its `ManagedUnwind` arm | `c0_2ci_failed_unwind_keeps_a_device_loss_recognisable` |
| `can_destroy` ignores pending obligations | `c0_2ci_reset_forced_teardown_keeps_gated_backing_and_late_proof_skips_reused_xid` (+8) |
| `destroy_now` leaves `by_xid` | `c0_2ci_reset_forced_teardown_keeps_gated_backing_and_late_proof_skips_reused_xid` |
| `apply_validated_proof` resolves the newest sibling entry | `c0_2ci_reset_forced_teardown_keeps_gated_backing_and_late_proof_skips_reused_xid` (+10) |
| each of the five `self.state()` transitions back to the raw field | `c0_2ci_service_driven_close_stops_the_gate_quiescing` (line 354), `c0_2ci_service_driven_close_is_terminal_for_owner_transitions` (453, 484, 543, 576) |

Checked by reading instead, with the reason: `discharge_husk_registration`'s
`None => drop(registration)` arm — `detach_managed_entries` needs a Vulkan
scanout pool and every detach test passes a registry, so no deterministic
test reaches it; what it relies on, a dropped registration failing closed, is
the first mutation above. And the two `return Err(managed_submit_failure(`
arms of `submit_shared_scanout_frame`, per the open criterion 1 above.

## Gate

- `cargo +nightly fmt --check`: clean.
- `cargo clippy --all-targets -- -D warnings`, plus `--features tcp-transport`
  and `--features xdmcp`: no findings in any of the three.
- `cargo check -p yserver` for `x86_64-unknown-linux-musl` and
  `x86_64-unknown-freebsd`: clean.
- `cargo test -p yserver --lib c0_2ci`: **180 passed, 18 ignored**, twelve
  consecutive runs, identical every time.
- Hardware: `cargo test -p yserver --lib c0_2ci -- --ignored`: **18 passed**.
- `cargo test --workspace`: 0 failures.
- Full census: `python3 tools/guard-census.py --require-oracle` over 71 sites.

## Execution record

Three tasks landed on the first attempt. Task 2 and Task 4 each stopped under
F8 — correctly, and on defects of the plan, not of the implementation:

1. **Task 2, revision 4.** Its test called `permit_for`, which Task 4
   introduces, and issuing a permit needs the evidence API Task 4 reshapes.
   The test was split so each half sits in the task that can prove it.
2. **Task 4, revision 5.** The plan's block extractor contained backticks;
   run inside `bash -lc "..."` the shell read them as command substitution and
   the pattern came back empty. The extractor now writes itself to a file and
   builds its fence with `chr(96)`.
3. **Task 4, revision 6.** `permit_for` sat in Task 4's block with no caller
   until Task 5, and each task's gate runs `clippy -D warnings`.

All three share one root cause: the task sequence had been validated as a
single prototype and with `fmt` + `cargo test`, never task by task with each
task's **full gate**. It is now validated that way, which is what caught the
third defect before dispatch rather than after. Worth carrying into the next
plan of this shape.

## Commits

```
2573c1d3 test(kms): prove the Owner handover refusals on bound evidence
5686ee32 fix(kms): bind handover evidence to its recipient and to every writer class
c5fb50b2 docs(plans): session 2 plan revision 6 -- permit_for belongs to Task 5
0171e2dc docs(plans): session 2 plan revision 5 -- a quoting-proof block extractor
dce30975 test(kms): prove the forced teardown keeps a proof-gated backing
726d8d1d fix(kms): propagate a failed managed-batch unwind and close the transport
```
