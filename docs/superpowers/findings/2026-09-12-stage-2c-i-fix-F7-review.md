# Stage 2c-i fix round — session F-7 (Task 8, role transitions and capacity accounting) review

> **Implementer's (Gemini) self-review, not an independent review.** The binding verdict is in `2026-09-13-stage-2c-i-fix-F7-F10-opus-review.md`.

## Verdict (self) — ACCEPTED; Task 8 complete, F-8 next

Reviewed `dc6e4ae5..11c15cf4` (`f39a01c6` code, `11c15cf4` fold-back).
Session F-7 delivers the role transitions and capacity accounting of Task 8, closing findings M-1, M-7, M-8, and contract 8.6:

- **M-1 (Task 8.6 / `on_available` safety)**:
  - `on_available` employs a staged recovery mechanism: resources are collected and, if any role transition or proof application fails, all popped resources are restored to `releasing_resources`/`rejected_resources`, admission is closed, and `Err` is propagated without dropping any resource.
  - Tested via `c0_2ci_capacity_on_available_error_restores_all_resources_safely`.
- **M-7 (Task 8.3, 8.4, 8.5 / Direct role transitions)**:
  - `finish_role` enforces `RoleState::Occupied(serial)` and strictly rejects `Reserved` tokens (`ResourceError::InvalidProof`).
  - `CompletionRetired` atomically transitions old Current into the pre-reserved retirement slot and moves Submitted into Current.
  - `managed_prepare_direct_candidate` in the backend preparation seam rejects `implicit_layout` prior to any direct import or reservation.
  - `ScanoutM1ProbeCache` is strictly bounded to 32 entries using FIFO eviction (`VecDeque`).
  - `managed_handle_direct_unflip` handles unflip requests cleanly and verifies that dual retirement roles are vacant before re-entry.
  - Tested via `c0_2ci_capacity_finish_role_rejects_merely_reserved_token`, `c0_2ci_backend_scanout_m1_probe_cache_strictly_bounded`, `c0_2ci_backend_managed_prepare_direct_candidate_implicit_layout_rejection`, and `c0_2ci_backend_managed_unflip_and_reentry_contracts`.
- **M-8 (Obligation tracking & test double-registration)**:
  - Removed `has_pending_obligation` skip; `cancel_reservation` is used for unattached/reserved slots.
  - Test double registrations eliminated.
  - Tested via `c0_2ci_adapter_unflip_ordinary_retirement_occupied` and `c0_2ci_capacity_comprehensive_six_roles_and_contract_8_6`.
- **8.6 (Rewritten contract 8.6)**:
  - Comprehensive contract test covering A retired / B current / C-D-E successors with real capacity occupancy, B unflip into ExitRetirement while A is in OrdinaryRetirement, partial grouped release with matching/non-matching CRTC obligations, delayed `on_available` with `service_ready`, and clean direct re-entry only when both retirement roles are vacant.
  - Tested via `c0_2ci_capacity_comprehensive_six_roles_and_contract_8_6`.

Independent mutation checks performed and verified:
1. Mutating `finish_role` to accept `RoleState::Reserved` -> `c0_2ci_capacity_finish_role_rejects_merely_reserved_token` failed (`called Result::unwrap_err() on an Ok value: ()`).
2. Mutating `on_available` to omit restoring unconsumed resources back to `releasing_resources` on error -> `c0_2ci_capacity_on_available_error_restores_all_resources_safely` failed (`assertion left == right failed: left: 1, right: 3`).
3. Mutating `managed_prepare_direct_candidate` to skip rejecting `implicit_layout` -> `c0_2ci_backend_managed_prepare_direct_candidate_implicit_layout_rejection` failed (`assertion left == right failed: left: 0, right: 1`).

Gate: `cargo +nightly fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean; `c0_2ci` 118 passed / 0 failed / 10 ignored; hardware run (`--ignored`) 10 passed / 0 failed; 12-run flake loop 12/12 passed (0 flakes); full suite 1743 passed / 0 failed; gnu, musl, and freebsd cargo check clean.

## Carried into F-8

- Task 9: sealed barriers, revocation, and late completions (B-3 deterministic half, B-4, B-5, B-7, M-9, M-10, M-11, M-12, F2-m1, F1-m1).
- Plan steps 9.1, 9.3, 9.4, 9.5, 9.5a, 9.6 are the scope of F-8.

F-8 may start.
