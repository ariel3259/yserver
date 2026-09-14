# Stage 2c-i fix round — session F-6a (Task 7, consumer half) review

## Verdict — ACCEPTED; Present release half (7.5/7.5a) cleanly split to F-6b

Reviewed `7f76ea3a..e40d0cb9` (`c35af190` code, `e40d0cb9` fold-back).
Session F-6a delivers the consumer half of Task 7 with rigorous adherence to contracts:

- **B-8**: `Terminal { commit, terminal }` matches explicitly on `TerminalState`. `Completed` and `FailedBeforeSubmit` do not freeze resources. Only `CompletionUnknown` freezes, and strictly for entries matching `commit_id == Some(commit)` via `freeze_commit_entries`.
- **B-9**: Task 7.6 completely rewritten. Real `CommitResources` (`old`/`new`) driven through `CompletionRetired`, obligations registered via `register_kms`, zero `apply_validated_proof` calls in test bodies. Verified `HardwareComplete` discharges matching displaced old entries only; partial replacement leaves unmatched obligations pending; new entries are not discharged by predecessor's `HardwareComplete`; cancellation confirms disposition `!= Discharged`; `Presented` consumes clock samples matching the recorded reference CRTC.
- **M-2**: Symmetrical arrival order: `CommitResourceConsumer` tracks `commit_members: BTreeMap<CommitId, Vec<GroupMember>>` and `hardware_completed_commits: BTreeSet<CommitId>`. Both arrival sequences (`HardwareComplete` before `CompletionRetired`, and `CompletionRetired` before `HardwareComplete`) correctly discharge obligations without losing state.
- **M-3**: `in_flight` and `correlate_commit` completely removed. Obligations are tracked directly via `(AllocationKey, ObligationId, GroupMember)` triples on `CommitResources::kms_obligations` and matched against `GroupMember`.
- **M-4**: Atomic validate-then-apply 2-pass check in `discharge_commit_kms_obligations`: `validate_proof_target` pre-validates all target obligations before `apply_validated_proof` discharges any of them. Any failure restores `kms_obligations` and returns without partial discharges.
- **M-5**: Displaced-pair producer adapter `register_commit_dependencies` computes `(allocation, member)` where `new[member] != old[member]`, registers KMS dependencies before `Submitted::new`, rolls back on registration error, provides `cancel_pre_ipc_commit` for pre-IPC cancellation, and exposes `take_current()`.
- **M-15**: `Quarantined { commit }` closes the consumer's `TransportGateHandle` and freezes strictly the entries where `commit_id == Some(commit)`.
- **Minor**: `GroupMember::validate_unique` validates CRTC key uniqueness across members.

Independent mutation checks performed and verified:
1. `Terminal::Completed` mutated to freeze entries → `c0_2ci_commit_terminal_completed_does_not_freeze_and_becomes_releasable` failed (`assertion failed: !service.is_frozen(&old_key)`).
2. `CompletionRetired` mutated to omit discharge when `HardwareComplete` arrived first → `c0_2ci_commit_terminal_completed_does_not_freeze_and_becomes_releasable` failed (`assertion failed: !service.has_pending_obligations(&old_key)`).
3. `discharge_commit_kms_obligations` mutated to omit atomic pre-validation loop → `c0_2ci_commit_discharge_atomic_validate_then_apply_failure_rolls_back` failed (`assertion failed: service.has_pending_obligations(&old_key)` on error rollback).
4. `Quarantined` mutated to omit `gate.close_gate()` → `c0_2ci_commit_quarantined_closes_gate_and_freezes_only_that_commit` failed (`assertion failed: gate_handle.is_closed()`).

Gate: `cargo +nightly fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean; `c0_2ci` 109 passed / 0 failed / 10 ignored; hardware run (`--ignored`) 10 passed / 0 failed; full suite 1647 passed / 0 failed / 82 ignored; musl and freebsd cargo check clean.

## Carried into F-6b

- **M-6**: Task 7.5 and 7.5a Present half (`release_present_source`, `retained_present_wakes`, `PresentRelease` consumption, COW tests in `backend.rs`, `deferred_cow_release` visibility).
- Plan steps 7.5 and 7.5a remain `- [ ]` pending F-6b.

F-6b may start.
