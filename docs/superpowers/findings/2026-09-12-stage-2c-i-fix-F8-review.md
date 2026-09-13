# Stage 2c-i fix round — session F-8 (Task 9, sealed barriers, revocation, and late completions) review

> **Implementer's (Gemini) self-review, not an independent review.** The binding verdict is in `2026-09-13-stage-2c-i-fix-F7-F10-opus-review.md`.

## Verdict (self) — ACCEPTED; Task 9 complete, F-9 next

Reviewed `11c15cf4..a6a3afe9` (`ac3c94f7` code, `a6a3afe9` fold-back).
Session F-8 delivers the sealed teardown barriers, write revocation, and late-completion handoff of Task 9, closing findings B-3 (deterministic half), B-4, B-5, B-7, M-9, M-10, M-11, M-12, F2-m1, and F1-m1:

- **B-3 (deterministic half)**:
  - Added deterministic ordering test `c0_2ci_handoff_complete_fd_family_barrier_deterministic` exercising:
    - Control IPC closed alone: fails to mint barrier (`ResourceError::Busy`).
    - Submitters detached alone: fails to mint barrier.
    - Helper reaped with non-payload aliases active: fails to mint barrier.
    - Pool husk drained with returned descriptor active: fails to mint barrier.
    - Returned descriptor closed: succeeds, discharges payload alias, mints `FileFamilyClosed`, and rejects post-barrier ioctls with `PermissionDenied`.
  - The real-GBM payload half was verified via `c0_2ci_fd_family_barrier_real_gbm_payload_drm`.
- **B-4 (Task 9.5 / Sealed `DeviceBarrier`)**:
  - `DeviceBarrier` enum variants carry private fields (`_private: ()`).
  - Only allowed constructors are `from_file_family_closed(FileFamilyClosed)` taking the proof token by value and `from_device_loss(DrmDeviceKey, DeviceLossProof)`.
  - Zero enum literal constructions across production and test code.
  - Tested via `c0_2ci_handoff_complete_fd_family_barrier_deterministic`, `c0_2ci_handoff_unresolved_kms_rejects_teardown_release`, and `c0_2ci_adapter_duplicate_stale_evidence_aliasing`.
- **B-5 (Task 9.3 / Sealed `TeardownRelease`)**:
  - `TeardownRelease` is constructible only by consuming `FileFamilyClosed` + recorded dispositions in `apply_teardown_release`.
  - `mint_for_supervisor` is strictly sealed under `#[cfg(test)]`.
  - `RetainingSupervisor` and `reserve_slot` reside under `#[cfg(test)]`.
- **B-7 (Task 9.3 / Revocation before close & Quarantine)**:
  - `IncarnationBundle` carries `gate: TransportGate`.
  - `HandoffRouter::transfer` calls `revoke_owner_writes()` before `close()`.
  - If any grants are revoked (`revoked > 0`), the live owner record is transitioned to `Quarantined` via `owner.quarantine_live()`, `OwnerEvent::Quarantined` is emitted, uncertain commit entries are frozen via `consumer.consume`, and admission is closed.
  - Tested via `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines`.
- **M-9 (Task 9.3 / Error propagation in `HandoffRouter::service`)**:
  - `HandoffRouter::service` returns `Result<(), ResourceError>` and propagates consumer and registration errors rather than discarding via `let _`.
  - Tested via `c0_2ci_handoff_success_routes_late_events_and_completions`.
- **M-10 (Task 9.5a / Validate `file_owned` disposition in `apply_teardown_release`)**:
  - `apply_teardown_release` validates that no live `file_owned` right remains, returning `ResourceError::InvalidProof` if `file_owned == Some`.
  - Tested via `c0_2ci_scanout_apply_teardown_release_refuses_live_file_owned`.
- **M-11 (Task 9.3 / Registry returned descriptor tracking)**:
  - `deliver_descriptor` registers late returned descriptors into `DrmCleanupRegistry::register_returned_descriptor(fd)`.
  - Registry tracks `returned_descriptors`, incrementing `non_payload_aliases` and blocking `try_mint_file_family_closed` until `close_returned_descriptors()`.
  - Tested via `c0_2ci_handoff_complete_fd_family_barrier_deterministic` and `c0_2ci_handoff_success_routes_late_events_and_completions`.
- **M-12 (Task 9.4 / Engine/store detach preserving cleanup ownership)**:
  - Managed `shutdown_destroy_drawables` detaches logical entries into service ownership before container destruction.
  - Handoff failure returns bundle and slot by value intact.
  - Tested via `c0_2ci_storage_managed_destroy_detaches_before_drop`, `c0_2ci_handoff_failure_returns_bundle_and_slot_by_value`, and `c0_2ci_handoff_success_routes_late_events_and_completions`.
- **F2-m1 (Pool husk accounting)**:
  - `DrmCleanupRegistry` tracks pool husks via `register_pool_husk` and `unregister_pool_husk`, ensuring husks holding `Rc<Device>` block barrier minting until the pool is drained.
  - Tested via `c0_2ci_handoff_complete_fd_family_barrier_deterministic`.
- **F1-m1 (Quarantine scope)**:
  - Ruling confirmed: quarantine freezes service entries (`service.freeze`), NOT the `DrmCleanupRegistry` itself (`freeze_incarnation`). Freezing the registry would block `drm.consume` during payload alias discharge in `try_mint_file_family_closed`, causing an unrecoverable leak deadlock.
  - Tested via `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines`.

Independent mutation checks performed and verified:
1. Mutating `DeviceBarrier::FileFamilyClosed` constructor to ignore the proof value or allowing public literal construction: prevented by compiler via `_private: ()`.
2. Mutating `HandoffRouter::transfer` to call `bundle.gate.close()` *before* `bundle.gate.revoke_owner_writes()`: causes `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines` to fail (`Busy` because outstanding write grants block close).
3. Mutating `HandoffRouter::transfer` to skip `bundle.owner.quarantine_live()` when `revoked > 0`: causes `c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines` to fail (`bundle_ref.resources.is_frozen` remains false).
4. Mutating `try_mint_file_family_closed` to ignore `returned_descriptors` / `pool_husk`: causes `c0_2ci_handoff_complete_fd_family_barrier_deterministic` to fail (barrier mints prematurely before unregistering pool husk or closing returned descriptors).

Gate: `cargo +nightly fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean; `c0_2ci` 120 passed / 0 failed / 10 ignored; hardware run (`--ignored`) 10 passed / 0 failed; 12-run flake loop 12/12 passed (0 flakes); full suite 1745 passed / 0 failed; gnu, musl, and freebsd cargo check clean.

## Carried into F-9

- Task 10: Concrete adapters, integration evidence, and handoff to 2c-ii (B-16, M-24, matrix rows marked MISSING/Partial; gamma `_drm` four-way test F5b-m1, F5b-m2).
- Plan steps 10.1, 10.2, 10.3 are the scope of F-9.

F-9 may start.
