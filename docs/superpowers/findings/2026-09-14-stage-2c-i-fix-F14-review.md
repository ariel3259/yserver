# Stage 2c-i fix session F-14 review + second mutation battery on scope 2

## Verdict — F-14 ACCEPTED. Scope 2 is still not clean: two new majors, one minor.

Reviewed `2de55f90..919952f2` (`e69d32c9` code+tests, `919952f2` fold-back).
Implementer: Sonnet. Reviewer: Opus (this document).

F-14 closed the three findings it was dispatched for. I re-ran the exact
mutations the findings name, choosing them myself rather than reading the
session's report:

| Finding | Mutation | Before F-14 | After F-14 |
| --- | --- | --- | --- |
| S2-M1 | delete the retention check in the registration path | 142/142 green | **fails** `…_retained_member_registers_no_kms_obligation` |
| S2-M2 (a) | drop the pre-pause flush in `set_seat_active` | green | **fails** `…_serviced_time_credits_active_interval_before_pause` |
| S2-M2 (b) | remove the `seat_active` gate in `service_completions` | green | **fails** `…_serviced_time_ignores_wall_clock_while_seat_inactive` |
| S2-m1 | swap cancel for discharge in `cancel_pre_ipc_commit` | green | **fails** `…_cancel_pre_ipc_marks_cancelled_and_cleans_stale_disposition` |

**S2-m1's production decision is right.** F-14 added `KmsDisposition::Cancelled`
rather than making `cancel` delete the disposition entry, on the grounds that
deleting it would make `cancel` and `apply_validated_proof` identical in ledger
effect and therefore make the finding's own mutation permanently undetectable.
That reasoning holds. The implementation uses `get_mut`, not `insert`, so a
cancelled **non-KMS** obligation (a `Gpu` one, say) adds nothing to
`kms_dispositions` — I checked, because an `insert` there would have quietly
filled a KMS-named map with GPU entries.

## A mutation I got wrong, and what it found

My first run of S2-m1's mutation reported **survival**. It was wrong: I replaced
the first textual match of `let _ = service.cancel(key, obligation_id);` in
`commit.rs`, and that string occurs **four** times. I had mutated
`consume`'s `ResourcesStillCurrent` arm, not `cancel_pre_ipc_commit`.

Re-run against each site individually:

| Site | Path | Result |
| --- | --- | --- |
| `commit.rs:643` | `cancel_pre_ipc_commit` | **fails** — S2-m1 genuinely closed |
| `commit.rs:308` | `OwnerEvent::ResourcesStillCurrent` | **survives** |
| `commit.rs:325` | `OwnerEvent::ResourcesReleased` | **survives** |

So the error found a real gap. Recorded as a rule, alongside the
"empty output means nothing ran" one from scope 1: **mutate by location, not by
first textual match, and state which site you hit.** A string-replace mutation
on a repeated line silently tests something other than what you named.

## Second mutation battery on scope 2 (Tasks 5–7)

I told the user before F-14 landed that three fixes would not make scope 2
clean: the first battery's survival rate (3 of 9, against 0 of 14 in scopes 1
and 3) said the surface was under-tested generally, not that I had found the
three holes that existed. This battery tests that claim.

| # | Mutation | Result |
| --- | --- | --- |
| Q1 | a failed GPU ticket is validated and committed instead of quarantined | **4 fail** |
| Q2 | `validate_gpu_batch` accepts an entry whose availability is `frozen` | **SURVIVES** → F14-M2 |
| Q3 | `is_resource_releasable` ignores outstanding `kms_obligations` | **SURVIVES** → F14-M1 |
| Q4 | `freeze_resource_allocations` skips the commit's allocations | **2 fail** |
| Q5 | `quarantine_gpu_batch` freezes none of its entries | **3 fail** |

Eight mutations counting the three cancel sites separately; four survivors
across three findings. The claim held.

## Findings

**F14-M1 (major) — R6's central gating clause is unproven.**

*Invariant:* a `CommitResources` carrying outstanding `KmsRelease` obligations
is not releasable. A displaced buffer is not idle until its release obligation
is discharged.

*Mutation that must fail a named test:* deleting the `kms_obligations` emptiness
clause from the consumer's release gate. Today the suite stays green.

Scope 2's N8 mutated the whole gate to `true` and three tests failed — so the
*allocation* clauses are covered, and that masked this one. The finer mutation
isolates it: only the allocation checks are proven. Failure mode is the one R6
exists to prevent — releasing a buffer whose displacement has not been proven
complete, i.e. one that may still be scanning out.

**F14-M2 (major) — the quarantine boundary in GPU batch validation is unproven.**

*Invariant:* a batch whose entry is `frozen` is refused, not committed. Freezing
is how quarantine is expressed, and quarantine is not a state this stage can
undo.

*Mutation that must fail a named test:* removing the frozen-entry refusal in
`validate_gpu_batch`. Today the suite stays green.

Note what *is* covered, because it is what makes this one easy to miss: Q5 shows
quarantine really does freeze its entries, and Q1 shows a failed ticket really
does quarantine. What no test checks is the other direction — that being frozen
actually stops a subsequent batch from discharging obligations on that entry.

**F14-m1 (minor) — S2-m1 is closed for one of three paths.**

*Invariant:* a rejected commit's KMS registrations are cancelled, not discharged,
observably, on **every** path that cancels them — `cancel_pre_ipc_commit`,
`OwnerEvent::ResourcesStillCurrent` and `OwnerEvent::ResourcesReleased`. R6 names
`ResourcesStillCurrent` verbatim, so if anything it is the primary path and the
pre-IPC one is secondary.

*Mutation that must fail a named test:* swapping cancel for discharge at each of
the three sites, independently. Today only the pre-IPC site is covered.

The production code at all three sites is already correct; this is coverage only.

## Recommendation — stop sampling this surface

Two batteries, seventeen mutations, seven survivors. Scopes 1 and 3 took seven
mutations each and gave up nothing. The gaps in Tasks 5–7 are not a list I am
converging on by sampling — each battery finds more at roughly the same rate,
and I have no reason to believe a third would not.

So the next session should **not** be "fix these three and re-sample". It should
enumerate the guard clauses in the Task 5–7 surface — `commit.rs`'s consumer and
registration paths, `gpu.rs`'s batch state machine, `transport.rs`'s gate — and
require a decisive test per clause, with the clause-deletion mutation as the
acceptance criterion for each. That is a bigger piece of work than F-14 was, and
it is the honest price of the round-1 damage in this area.

If the user would rather ship 2c-i and carry it, the alternative is to accept the
stage with F14-M1/M2/m1 closed and the systematic pass written into 2c-ii's spec
as its first task. That is a legitimate call — it is theirs, not mine — but it
should be made knowingly, not by declaring the surface clean after another three
fixes.

## Gate (this box, after restoring from every mutation)

`cargo +nightly fmt --check` clean · `cargo clippy --all-targets -- -D warnings`
clean · `c0_2ci -- --include-ignored` **146 passed / 0 failed** (142 + F-14's
four) · full `cargo test -p yserver --lib` verified by the session at
**1666/0/90**. F-14 did not run the musl/freebsd checks and said so: it touches
none of `drm/`, `drm_cleanup.rs` or `transport.rs`, which is the resume doc's own
scoping rule for those targets. Correct call.
