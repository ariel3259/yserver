# Stage 2c-i fix round — session F-13b (Task 8: managed direct seam charges) review

## Verdict — ACCEPTED; F-13c next

Reviewed `04b045f8..5fa653d7` (`cc323714` code, `5fa653d7` fold-back).
Implementer: Sonnet. Reviewer: Opus (this document).

F7-B1 closed: the Successor charge lives in `ScanoutM2State::queued_successor_role`
in lockstep with `queued_successor`; a successful prepare no longer cancels it;
replacing a victim discharges the victim's bare reservation first (the
`Successor` slot is single, so the order matters and Sonnet found that by test);
`managed_dispatch_direct_successor` (new, 8.4) moves Successor → Submitted, occupies
via the existing `DirectCapacity::attach`, and when displacing a Current
pre-reserves `OrdinaryRetirement` and calls `prereserve_retirement` — or keeps the
successor queued and sets `direct_admission_scheduled` when that slot is occupied.
`discharge_bare_reservation` centralizes cancel-or-mark so no token is bare-dropped
(which would close admission). F7-B2 closed: unflip discharges the queued
successor's charge, reserves `ExitRetirement` and `move_into_reserved`s the
`Current` resources there even with `OrdinaryRetirement` occupied, pushing them to
`releasing_resources` for the ordinary consumer path. R8 kept: the seams stay
`#[allow(dead_code)]` with test-only callers; the two production sites that mutate
`queued_successor` only discharge a role that is always `None` there.

Three new deterministic tests
(`…_prepare_direct_candidate_charges_and_replaces_successor`,
`…_dispatch_direct_successor_charges_submitted_then_retires`,
`…_unflip_moves_current_into_exit_retirement_even_if_ordinary_occupied`) drive
`occupied()` counts across prepare → replace → dispatch → `consume(CompletionRetired)`
→ `on_available` → `finish_role`, and the unflip contract.

## Mutation checks (this reviewer)

1. Dispatch discharges the retirement slot instead of `prereserve_retirement` →
   `…_dispatch_direct_successor_charges_submitted_then_retires` fails (`backend.rs:46603`). ✔
2. Unflip drops the `Current` resources (role marked discharged) instead of moving
   them to `releasing_resources` → `…_unflip_moves_current_into_exit_retirement_…`
   fails (`backend.rs:46731`). ✔
3. Sonnet's two (restore the end-of-prepare cancel; skip the ExitRetirement move)
   read consistent with the test bodies.

## Gate (this box)

`cargo +nightly fmt --check` clean · `cargo clippy --all-targets -- -D warnings`
clean · `c0_2ci -- --include-ignored` 137 passed / 0 failed. Fold-back records the
12-run flake loop (0 flakes) and full suite 1662/0/85.

## Deferred (recorded, not carried into F-13c)

**F13b-D1 — the dispatched `CommitResources` carries no leases.**
`managed_dispatch_direct_successor` builds `CommitResources::new(vec![], …)`: the
frame's source/fallback present pins (`present_source_pins`, `share_storage_read`
leases) are not moved into it by value as 8.4's text says ("move Current/New
leases into `Submitted<CommitResources>` by value"). Role accounting — what F7-B1
was about — is right; lease-by-value adoption is the production-activation half
that R8 keeps out of this stage. **Deferred to 2c-iii**, where the managed direct
route gets its real caller; recorded in the resume doc so it is not lost.

F-13c (F4d-M1, F8-M1, F8-M2, F8-m1, F9-m1, F13a-m1) may start.
