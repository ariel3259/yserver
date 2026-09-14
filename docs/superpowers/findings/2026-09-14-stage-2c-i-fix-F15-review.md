# Stage 2c-i fix session F-15 review — and the stage's acceptance

## Verdict — F-15 ACCEPTED. **Stage 2c-i is ACCEPTED**, knowingly, with the trade recorded below.

Reviewed `65854a37..df599934` (`b611c115` tests, `df599934` fold-back).
Implementer: Sonnet. Reviewer: Opus (this document). No production code was
touched, which matches the session's premise: all three mechanisms were
already correct.

I re-ran every mutation myself, each addressed **by line number**, not by
textual match:

| Finding | Mutation site | Result |
| --- | --- | --- |
| F14-M1 | `commit.rs:502-504`, the `kms_obligations` clause deleted | **fails** `c0_2ci_commit_kms_obligations_block_release_gate` |
| F14-M2 | `mod.rs:1076-1078`, the frozen-entry refusal deleted | **fails** `c0_2ci_gpu_frozen_entry_refuses_subsequent_valid_batch` |
| F14-m1 | `commit.rs:308` (`ResourcesStillCurrent`) | **fails** `…_resources_still_current_cancels_not_discharges` |
| F14-m1 | `commit.rs:325` (`ResourcesReleased`) | **fails** `…_resources_released_cancels_not_discharges` |
| F14-m1 | `commit.rs:643` (`cancel_pre_ipc_commit`) | **fails** `…_cancel_pre_ipc_marks_cancelled_and_cleans_stale_disposition` |

F-15's own reasoning on the F14-M2 fixture is right and worth keeping: it
registers two obligations on one entry, uses the first to trigger a *real*
quarantine-and-freeze, and proves the **second** — registered before the
freeze — is then refused. That tests the direction that was missing, rather
than re-proving that a failing batch freezes itself.

## Correction to my own finding F14-M1

F-15's fold-back notes that its test had to use a `CommitResources` with an
empty `allocations` vector, because in production every `kms_obligations` key
is also an allocation key and the allocation loop would mask a deleted
`kms_obligations` check. I verified that claim rather than accepting it:
`register_commit_dependencies` only ever pushes obligations drawn from the
same `res`'s own `allocations`, and `allocations` is never drained or mutated
after construction — every other reference to it is a read.

So **the clause is defensive redundancy, and the failure mode I attributed to
F14-M1 was not reachable.** I wrote that deleting it would allow "releasing a
buffer whose displacement has not been proven complete, i.e. one that may
still be scanning out". It would not: while the KMS obligation is pending it
is in that key's `pending_obligations`, so the allocation loop's
`is_releasable` already refuses. My finding overstated the risk.

This is the same shape as scope 3's P3 observation — two guards, one property
— and had I checked the coupling before writing the finding rather than after,
F14-M1 would have been recorded as an observation, not a major.

The test stays: it pins the guard against a future refactor that decouples the
two vectors, and that is worth having. But the record should say what it
proves (the guard) and not claim it closed a reachable hazard.

F14-M2 and F14-m1 are unaffected by this correction — both describe reachable
states, and both are genuinely closed.

## Gate (this box, after restoring from every mutation)

`cargo +nightly fmt --check` clean · `cargo clippy --all-targets -- -D warnings`
clean · `cargo test -p yserver --lib c0_2ci` **131 passed / 0 failed / 18
ignored**, twelve consecutive runs, zero flakes · `-- --ignored` **18 passed /
0 failed** · full `cargo test -p yserver --lib` **1669 passed / 0 failed / 90
ignored**. Musl/freebsd checks not required: this session touches only
`resources/tests.rs`.

## Stage 2c-i — accepted

Every fix session F-1..F-15 is accepted. The final stage review ran in three
scopes; scopes 1 and 3 hold, and scope 2's findings are closed by F-14 and
F-15. Totals across the whole review: **twenty-six mutations**, of which
nineteen were caught on first contact and seven exposed gaps that are now
closed.

Round 1's verdict is answered on its own terms. It said the R5 chain was
inverted and that the three tests the handoff calls decisive (2.5, 4.6, 9.5)
proved nothing. Restoring that inversion today fails eight tests, all three of
them included.

**The trade this acceptance carries, recorded so it is not mistaken for a
clean bill of health.** Mutation sampling of Tasks 5–7 never converged: two
batteries, seventeen mutations, seven survivors, against zero survivors in
fourteen mutations across Tasks 1–4 and 8–10. F-14 and F-15 closed the seven
found. There is no basis for believing a third battery would find none. On
2026-09-14 the user chose to close the known findings and accept the stage
rather than keep sampling, and **2c-ii's spec opens with a systematic
per-guard-clause pass** over `commit.rs` (consumer and registration paths),
`gpu.rs` (the batch state machine) and `transport.rs`: enumerate every guard
clause, require a decisive test per clause, and use clause-deletion as each
one's acceptance criterion.

## What 2c-ii's spec must carry

1. **First task:** the systematic per-guard-clause pass described above.
2. **F13b-D1** — the dispatched `CommitResources` carries no present-pin
   leases by value; that is the activation half R8 excludes.
3. **F13c-m1** — `detach_managed_entries(None)` lets the production route skip
   husk accounting silently. Inert only while R8 holds: once the managed route
   is production-active, the fd-family barrier can never mint again.
4. **F13c-m2** — `unregister_pool_husk`'s `saturating_sub` hides underflow.

Next: `docs/status.md`, then 2c-ii's spec.
