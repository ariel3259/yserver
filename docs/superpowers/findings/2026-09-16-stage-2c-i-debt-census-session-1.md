# Stage 2c-i debt — session 1 accepted: 28 guards proven by oracle

**Date:** 2026-09-16. **Plan:** `docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-1.md`.
**Spec:** `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` §5.1.

**Implementer:** codex, model `gpt-5.6-luna`, reasoning effort `xhigh`,
`--sandbox workspace-write` (Tasks 1 steps 1–2, 2–7). **Coordinator:** Opus
(verification of every task, all commits, and the [H] steps: Task 1 steps
4–6 and Task 8), since the implementer's sandbox has no GPU/DRM access and a
read-only git directory.

## Verdict — session 1 ACCEPTED

The acceptance census (`tools/guard-census.py --require-oracle`, full
enumeration, with hardware) reports exactly what §5.1 requires:

| Verdict | Required | Result |
| --- | --- | --- |
| `` CAUGHT_BY_ORACLE `` | 28 | **28** |
| `CAUGHT` (already proven, untagged) | 32 | **32** |
| `SURVIVES` (session-2 scope only) | 8 | **8** |
| `CAUGHT_NOT_BY_ORACLE`, `CAUGHT_WHOLE_BODY`, `ORPHAN_TAG`, `A_MANO` | 0 | **0** |

68 sites; exit 0. Before this stage the same surface had 36 survivors (35 in
the published census plus the `} else if` guard found in Task 1); 28 of them
are now each killed by the test their tag is bound to, carrying their own
marker, under a mutation strategy other than whole-body replacement.

## The 28 guards proven by oracle

| Site | Verdict | Marker | Bound test | Strategy |
| --- | --- | --- | --- | --- |
| `` mod.rs adopt `payload.file_owned_alias_present()` `` | CAUGHT_BY_ORACLE | H-adopt-file-owned | c0_2ci_guard_adopt_refuses_live_file_owned_payload | mutate_if_false |
| `` mod.rs adopt_unchecked `self.exhausted` `` | CAUGHT_BY_ORACLE | G-adopt-exhausted | c0_2ci_guard_adopt_refuses_when_exhausted | mutate_if_false |
| `` mod.rs reserve `self.exhausted` `` | CAUGHT_BY_ORACLE | G-reserve-exhausted | c0_2ci_guard_reserve_refuses_when_exhausted | mutate_if_false |
| `` mod.rs register `key.device != self.device \|\| key.incarnation != self.incarnation` `` | CAUGHT_BY_ORACLE | E-register | c0_2ci_guard_register_refuses_foreign_key | mutate_if_false |
| `` mod.rs register `self.exhausted` `` | CAUGHT_BY_ORACLE | G-register-exhausted | c0_2ci_guard_register_refuses_when_exhausted | mutate_if_false |
| `` mod.rs freeze `key.device != self.device \|\| key.incarnation != self.incarnation` `` | CAUGHT_BY_ORACLE | E-freeze | c0_2ci_guard_freeze_refuses_foreign_key | mutate_if_false |
| `` mod.rs cancel `key.device != self.device \|\| key.incarnation != self.incarnation` `` | CAUGHT_BY_ORACLE | E-cancel | c0_2ci_guard_cancel_refuses_foreign_key | mutate_if_false |
| `` mod.rs validate_proof_target `key.device != self.device \|\| key.incarnation != self.incarnation` `` | CAUGHT_BY_ORACLE | E-validate-proof-target | c0_2ci_guard_validate_proof_target_refuses_foreign_key | mutate_if_false |
| `` mod.rs record_kms_discharged `key.device != self.device \|\| key.incarnation != self.incarnation` `` | CAUGHT_BY_ORACLE | E-record-kms-discharged | c0_2ci_guard_record_kms_discharged_refuses_foreign_key | mutate_if_false |
| `` mod.rs apply_teardown_release `proof.incarnation != self.incarnation` `` | CAUGHT_BY_ORACLE | E-teardown-proof | c0_2ci_guard_teardown_release_refuses_foreign_proof | mutate_if_false |
| `` mod.rs apply_teardown_release `key.device != self.device \|\| key.incarnation != self.incarnation` `` | CAUGHT_BY_ORACLE | E-teardown-key | c0_2ci_guard_teardown_release_refuses_foreign_key | mutate_if_false |
| `` mod.rs apply_teardown_release `!avail.frozen` `` | CAUGHT_BY_ORACLE | I-teardown-requires-frozen | c0_2ci_guard_teardown_release_refuses_unfrozen_entry | mutate_if_false |
| `` mod.rs validate_gpu_batch `key.device != self.device \|\| key.incarnation != self.incarnation` #1 `` | CAUGHT_BY_ORACLE | E-batch-entry | c0_2ci_guard_gpu_batch_refuses_foreign_entry | mutate_if_false |
| `` mod.rs validate_gpu_batch `key.device != self.device \|\| key.incarnation != self.incarnation` #2 `` | CAUGHT_BY_ORACLE | E-batch-read-source | c0_2ci_guard_gpu_batch_refuses_foreign_read_source | mutate_if_false |
| `` mod.rs validate_gpu_batch `avail.frozen` #2 `` | CAUGHT_BY_ORACLE | F-read-source-frozen | c0_2ci_guard_gpu_batch_refuses_frozen_read_source | mutate_if_false |
| `` mod.rs validate_gpu_batch `!avail.pending_obligations.contains_key(&obligation_id)` #2 `` | CAUGHT_BY_ORACLE | F-read-source-pending | c0_2ci_guard_gpu_batch_refuses_read_source_without_pending_obligation | mutate_if_false |
| `` mod.rs validate_gpu_batch `s_key.device != self.device \|\| s_key.incarnation != self.incarnation` `` | CAUGHT_BY_ORACLE | E-batch-read-staging | c0_2ci_guard_gpu_batch_refuses_foreign_read_staging | mutate_if_false |
| `` mod.rs validate_gpu_batch `s_avail.frozen` `` | CAUGHT_BY_ORACLE | F-read-staging-frozen | c0_2ci_guard_gpu_batch_refuses_frozen_read_staging | mutate_if_false |
| `` mod.rs validate_gpu_batch `!s_avail.pending_obligations.contains_key(&staging_ob)` `` | CAUGHT_BY_ORACLE | F-read-staging-pending | c0_2ci_guard_gpu_batch_refuses_read_staging_without_pending_obligation | mutate_if_false |
| `` commit.rs consume `let Err((err, recovered)) = self.capacity.move_into_reserved(role, reserved)` `` | CAUGHT_BY_ORACLE | C-retire-move-into-reserved | c0_2ci_guard_completion_retired_returns_failed_move_into_reserved | mutate_swallow |
| `` commit.rs consume `self.capacity.is_vacant(DirectRole::OrdinaryRetirement) && let Err(err) = self .capacity .move_role(role, DirectRole::OrdinaryRetirement)` `` | CAUGHT_BY_ORACLE | C-retire-move-into-ordinary-retirement | c0_2ci_guard_completion_retired_returns_failed_move_into_ordinary_retirement | mutate_swallow |
| `` commit.rs consume `let Some(ref mut role) = res.direct_role && role.role == DirectRole::Submitted && let Err(err) = self.capacity.move_role(role, DirectRole::Current)` `` | CAUGHT_BY_ORACLE | C-retire-submitted-to-current | c0_2ci_guard_completion_retired_returns_failed_submitted_to_current | mutate_swallow |
| `` commit.rs on_available `let Some(err) = transition_error` #1 `` | CAUGHT_BY_ORACLE | C-on-available-releasing-early-return | c0_2ci_guard_on_available_leaves_rejected_untouched_after_releasing_error | mutate_swallow |
| `` commit.rs on_available `let Some(err) = transition_error` #2 `` | CAUGHT_BY_ORACLE | C-on-available-rejected-error | c0_2ci_guard_on_available_returns_rejected_half_error | mutate_swallow |
| `` commit.rs is_resource_releasable `let Some(source) = &res.source && !service.is_releasable(&source.allocation.key())` `` | CAUGHT_BY_ORACLE | D-releasable-source | c0_2ci_guard_on_available_retains_resource_with_busy_source | mutate_if_false |
| `` commit.rs is_resource_releasable `let Some(fallback) = &res.fallback && !service.is_releasable(&fallback.allocation.key())` `` | CAUGHT_BY_ORACLE | D-releasable-fallback | c0_2ci_guard_on_available_retains_resource_with_busy_fallback | mutate_if_false |
| `` commit.rs register_commit_dependencies `!GroupMember::validate_unique(&new_members)` `` | CAUGHT_BY_ORACLE | D-unique-new-members | c0_2ci_guard_commit_dependencies_refuse_duplicate_new_members | mutate_if_false |
| `` transport.rs authorize_write `TransportState::Closed =>` `` | CAUGHT_BY_ORACLE | B-authorize-write-closed | c0_2ci_guard_authorize_write_refuses_every_class_when_closed | mutate_arm |

## The eight survivors, all session-2 scope

| Site | Session 2 |
| --- | --- |
| `` gpu.rs cancel_pre_submit_batch `Some(err) =>` `` | §4.2 — swallowed GPU error at its scene.rs caller |
| `` gpu.rs freeze_uncertain_batch `Some(err) =>` `` | §4.2 — swallowed GPU error at its scene.rs caller |
| `` transport.rs consume_owner_write `self.state != TransportState::Owner` `` | §4.4 — after the handover evidence is strengthened |
| `` transport.rs issue_handover_permit `self.state != TransportState::Quiescing` `` | §4.4 — family A |
| `` transport.rs issue_handover_permit `self.outstanding_owner_writes != 0` `` | §4.4 — family A |
| `` transport.rs publish_owner `permit.device != self.device \|\| permit.incarnation != self.incarnation` `` | §4.4 — family A |
| `` transport.rs publish_owner `self.state != TransportState::Quiescing` `` | §4.4 — family A |
| `` transport.rs publish_owner `self.outstanding_owner_writes != 0` `` | §4.4 — family A |

## What happened along the way

- **Task 1.** The tool reproduced the published census exactly — 67 sites,
  32 caught, 35 survivors, matching per file and function — which is why
  later tasks could rely on it. Its full enumeration found one `} else if`
  guard the hand census could not see, in `commit.rs` `consume`; it survived,
  and by the user's call it joined family C (session 1 went from 27 to 28).
- **F8 stop in Task 5.** The else-if test returned `Ok(())`. Debugged to a
  transcription error by the implementer — `Submitted::new(vec![], vec![old])`
  where the plan has `Submitted::new(vec![old], vec![])` — not a test or
  production defect. Restored to the plan's text; the implementer's stop was
  correct and touched no production code. From Task 6 the implementer
  verified its copy against the plan mechanically before running tests.
- **Coordinator errors, recorded.** The first Task 6 dispatch stated 28
  expected tests instead of 27 (guard total confused with the cumulative test
  count); stopped before any edit and relaunched. An earlier probe used the
  model shorthand `luna`, which the API rejects; `gpt-5.6-luna` is correct.
- **Plan defects found before they bit, each fixed and recorded:** the census
  needed a green-baseline precondition (hardware tests fail where there is no
  GPU, which would have made every mutation look caught); it restored files
  with `git checkout`, which a read-only git directory breaks, so it restores
  from memory; and it refused to run over uncommitted target files, which
  would have blocked every implementer oracle check, so that refusal now
  applies only to authoritative full censuses.
- **Verification discipline.** Every task was checked by the coordinator
  before commit: the file equals the committed content plus the plan's task
  code once rustfmt is applied, the guard tests pass, and a full census with
  hardware proves the task's sites through their own oracles — not the
  implementer's report.

## Gate (coordinator, with hardware)

```
== fmt --check
OK
== clippy default
OK (0 errores/warnings)
== clippy --features tcp-transport
OK (0 errores/warnings)
== clippy --features xdmcp
OK (0 errores/warnings)
== clippy workspace
OK
== c0_2ci
test result: ok. 159 passed; 0 failed; 18 ignored; 0 measured; 1642 filtered out; finished in 0.03s
== c0_2ci --ignored (hardware)
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 1801 filtered out; finished in 1.13s
== flake loop x12
     12 ok. 159 passed; 0 failed
== workspace
binarios ok: 20  fallidos: 0
```

## Next

Session 2 (spec §4): the identity-bound husk token (4.1), the swallowed GPU
errors and their transport close (4.2), the reset-boundary test (4.3), and the
handover evidence followed by family A and `consume_owner_write` (4.4). It gets
its own plan, which is also where these eight survivors are closed.
