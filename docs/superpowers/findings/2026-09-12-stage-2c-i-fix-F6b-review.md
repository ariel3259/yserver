# Stage 2c-i fix round — session F-6b (Task 7, Present release half) review

## Verdict — ACCEPTED; Task 7 complete, F-7 next

Reviewed `3f04acd6..a242a9da` (`8b0e00d6` code, `a242a9da` fold-back).
Session F-6b delivers the Present release half of Task 7 (Tasks 7.5 and 7.5a), closing blocker M-6:

- **M-6 (Task 7.5)**:
  - Source and fallback pin ownership is decoupled from numeric protocol handles: `PresentPinEntry` owns `Option<StorageLease>` and `DrawableId`.
  - `release_present_source` removes that entry once and calls `store_decref_with_invalidate` for immediate, invalidation-aware cleanup without leaking logical decref obligations.
  - `retained_present_wakes` moves the actual `PinnedWake` into `PresentRelease`; `signal_present_release` consumes and dispatches the pinned wake without numeric protocol XID re-lookup.
  - `CommitResourceConsumer` consumption logic wired:
    - `Presented` marks completion `Emitted` while keeping release `Retained`.
    - `Terminal::FailedBeforeSubmit` suppresses completion (`Suppressed`) while keeping release `Retained`.
    - `on_available` drains `PresentRelease` when allocations are releasable and transitions disposition to `Released`.
  - Replaced fabricated `c0_2ci_cow_deferred_release_and_reclaim` in `resources/tests.rs` with `c0_2ci_present_release_consumption_and_completion_suppression`.
- **M-6 (Task 7.5a)**:
  - Added `c0_2ci_cow_deferred_release_and_reclaim_with_physical_contracts` verifying the complete 6-part contract of plan 7.5a in `backend.rs` with zero boolean hand-setting:
    1. 1->0 edge with active direct scanout defers release, dropping no lease and decref'ing no storage.
    2. 0->1 re-claim edge while `deferred_cow_release` holds reuses identity, clears flag, performs no new allocations/imports, keeping same allocation key and generation.
    3. Direct unflip stop path observes cleared flag and frees nothing; COW survives.
    4. Second unflip without re-claim decrefs exactly once.
    5. Late stop-path / unflip evidence for old identity after fresh 0->1 re-allocation retires nothing.
    6. Materialization failure preserves claim; disconnect releases claim anyway and marks sticky failure.

Independent mutation checks performed and verified:
1. `release_present_source` mutated by omitting `store_decref_with_invalidate` -> `c0_2ci_present_split_source_fallback_pin_ownership_and_release` failed (`assertion left == right failed: left: 2, right: 1`).
2. `make_present_release` mutated by returning `None` wake instead of removing from `retained_present_wakes` -> `c0_2ci_present_retained_wakes_move_into_present_release_and_signal` failed (`assertion failed: release.wake.is_some()`).
3. `consume` for `Terminal::FailedBeforeSubmit` mutated to set `disp.release = ReleaseDisposition::Released` -> `c0_2ci_present_release_consumption_and_completion_suppression` failed (`assertion left == right failed: left: Released, right: Retained`).

Gate: `cargo +nightly fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean; `c0_2ci` 112 passed / 0 failed / 10 ignored; hardware run (`--ignored`) 10 passed / 0 failed; 12-run flake loop 12/12 passed (0 flakes); full suite 1650 passed / 0 failed / 82 ignored; musl and freebsd cargo check clean.

## Carried into F-7

- Task 8: six physical roles, capacity accounting, direct role transitions in `CommitResourceConsumer` (M-1, M-7, M-8).
- Plan steps 8.3, 8.4, 8.5, 8.6 are the scope of F-7.

F-7 may start.
