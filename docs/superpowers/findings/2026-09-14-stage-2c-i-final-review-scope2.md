# Stage 2c-i final stage review — scope 2 of 3 (Tasks 5–7)

## Verdict — NOT CLEAN. Two majors, one minor. No blocking findings.

Reviewed `76a93356..25d1f1b7` restricted to Tasks 5–7: the GPU/read/write
adapters (`resources/gpu.rs`), completion progress and the commit consumer
(`resources/commit.rs`, `present.rs`), and the transport permission boundary
(`resources/transport.rs`) with its sinks, against rulings **R6**, **R7**,
**R9** and **R11**.

Reviewer: Opus (this document). Nine mutations run by hand on this box; each
applied, run over the full 142-test `c0_2ci` suite with hardware included,
then reverted and confirmed green.

The mechanisms are **present and correct by inspection** — every ruling in
this scope is implemented the way it is written. What the two majors report
is that three of those clauses are **not proven by any test**: I can delete
the mechanism and the whole suite stays green. That is the same class of
defect this fix round existed to repair (F4d-M1), found now in code nobody
had mutation-checked yet.

## Mutation battery

| # | Mutation | Ruling | Result |
| --- | --- | --- | --- |
| N1 | `begin_quiescing` ignores direct ownership and outstanding grants | R7 | **3 fail** |
| N2 | `allows_legacy` true for every non-`Closed` state | R7/R11 | **8 fail** — cursor, direct atomic flip, composed unflip, modeset install, legacy page flip, output disable, the vocabulary table, and the gamma hardware test |
| N3 | `authorize_write` permits writes while `Quiescing` | R7 | **1 fail** |
| N4 | `register_commit_dependencies` registers a `KmsRelease` for members **retained** across the commit | R6 | **SURVIVES — 142/142 green** → S2-M1 |
| N5 | discharge keyed by the bare CRTC number instead of the full `GroupMember` | R6 | **1 fail** (`c0_2ci_commit_topology_replacement_reused_numeric_crtc`) |
| N6 | `cancel_pre_ipc_commit` discharges instead of cancelling | R6 | **SURVIVES — 142/142 green** → S2-m1 |
| N7 | `set_seat_active` stops accounting the serviced interval at a pause | R9 | **SURVIVES** |
| N7b | `service_completions` burns wall time regardless of `seat_active` | R9 | **SURVIVES — 142/142 green** → S2-M2 |
| N8 | `is_resource_releasable` always true (the consumer's central release gate) | Task 7 | **3 fail** |

R7 and R11 come out strong: N2 alone fails eight distinct real sinks, which
is R11's caller inventory being genuinely enforced beneath the entry points
rather than at a mock.

## Findings

**S2-M1 (major) — R6's retained-member clause is proven by nothing, and the
test that claims to prove it never calls the code that decides.**

*Invariant:* an allocation that a grouped commit **retains** at a member
registers no `KmsRelease` obligation for that member. Only a displaced pair
`(allocation, member)` with `new[member] != old[member]` registers.

*Evidence it is unproven:* deleting the retention check in the registration
path leaves all 142 tests green (N4).

The only place that claims this clause is
`c0_2ci_commit_grouped_skip_and_duplicate_protection` (`tests.rs:3880`),
whose comment reads "member2 is retained unchanged across this grouped
commit, so it registers NO KMS obligation (round-4 m-1)". That test never
calls `register_commit_dependencies` at all: it hand-builds the
`kms_obligations` vector, registers the one obligation itself, and then
asserts that a *second allocation which is not part of the commit in any
role* is not dropped. The assertion is tautological — it would hold no
matter what the registration logic did. `c0_2ci_commit_register_dependencies_and_pre_ipc_cancellation`
(`tests.rs:5611`) does call the function, but only for a genuinely displaced
pair.

*Why it matters:* the extra obligation self-cancels in the happy path (the
retained member is in the completed set, so the same completion discharges
it), which is why nothing shows. Under **partial** group completion it does
not: a retained allocation is left gated on a `KmsRelease` for a member that
never completes, so the entry is never destroyable — a permanent leak of a
buffer that is still perfectly current.

*What must be true of the fix:* the mutation above must fail a named test.
Where that test lives and what shape it takes is the implementer's call.

**S2-M2 (major) — R9's serviced-time deadline is proven by nothing, in
either direction.**

*Invariant:* a pending batch's deadline counts **serviced** time, and that
budget pauses while the seat is inactive. Wall time spent VT-away or with
the seat inactive must not expire a batch.

*Evidence it is unproven:* two independent mutations both leave 142/142
green — dropping the pause bookkeeping when the seat goes inactive (N7), and
removing the `seat_active` gate so the budget accrues wall time
unconditionally (N7b). The second is R9's clause verbatim, inverted.

The one existing seat-related test covers only M-16's converse — that
*progress* (polling an already-signalled ticket) is not suppressed by seat
inactivity — which is why removing the pause does not disturb it.

*Why it matters:* R9's whole point is that an expired deadline is never a
proof. A batch registered shortly before a VT switch would be quarantined
for no reason other than the user being away, and quarantine is not a state
this stage can undo.

*What must be true of the fix:* both mutations above must fail a named test.

**S2-m1 (minor) — R6's "a rejected commit cancels, it does not discharge" is
not observable in the ledger, and cancellation leaves a stale disposition
behind.**

Swapping cancellation for proof application in the pre-IPC cancel path
leaves 142/142 green (N6). That is not purely a test gap: `cancel` and
`apply_validated_proof` are the same operation apart from one line —
`apply_validated_proof` also removes the obligation's `kms_dispositions`
entry, and `cancel` does not. So:

- no test **can** distinguish them on the ledger state the assertions look
  at (`has_pending_obligations` is satisfied by both), and the existing
  test's parenthetical "(not pending, not discharged)" is not actually
  checked; and
- a cancelled registration leaves an `Outstanding` disposition keyed by an
  obligation that no longer exists. `record_device_barrier` walks every
  entry's dispositions, flips each `Outstanding` one to `Superseded` and
  marks the entry dirty — so a stale entry causes spurious dirty marks and
  a spurious `kms_disposition()` answer for a commit that never happened,
  for as long as the allocation lives.

It does **not** block teardown: `apply_teardown_release` iterates
`pending_obligations`, which no longer contains the cancelled obligation. I
checked that path specifically because it was the plausible serious version
of this bug, and it does not occur.

*Suggested direction (not a prescription):* if the cancel/discharge
distinction matters, the ledger should record it so a test can see it; if it
does not, R6's wording invites reviewers to hunt for a distinction the data
model cannot express. Either way, cancellation should clean up the
disposition it created.

## What this scope did NOT cover

Tasks 8–10 — role transitions, the retaining handoff, and the integration
evidence — are scope 3, including R7's handoff half (`revoke_owner_writes`
before `close`, every revoked grant `Quarantined`).

## Consequence for the stage

The stage cannot be accepted with two binding rulings unproven. This is a
**tests-only** fix session — the mechanisms are correct; nothing in the
production path changes — so it is small: S2-M1, S2-M2, S2-m1. Scope 3 runs
first, so that session closes everything the final review finds in one pass.
