# Stage 2c-ii, plan B2 (maintenance in the conductor) — accepted

**Plan:** `docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md`
revision 4, after three codex review rounds (1B 2M, 3B 1M, 3B), with the spec's
§7 "Activation" amended in round 3.
**Implemented by:** codex, model `gpt-5.6-luna`, reasoning effort `xhigh`, one
task per run; the coordinator verified each task outside the sandbox and
committed it. A full `cargo clean` (62.4 GiB) preceded Task 1's verification.

| Task | Commit | Sent back |
| --- | --- | --- |
| 1 — owner events per batch, one wake | `600458b8` | once: F-B2T1-1, the DRM path routed non-clock owner events for devices without a conductor (a production change) — the plan had described today's DRM behaviour wrongly |
| 2 — store, offers, snapshot inputs | `89826ced` | no |
| 3 — dispatch, receipt, bound check | `25ebb1be` | no |
| 4 — terminal outcomes through the receipt | `f088f81d` | no |
| mutation survivors Q9, Q21 | `00e32c10` | test gaps closed; tests only |

## Mutations (coordinator, against the implemented code)

| Mutation | Result |
| --- | --- |
| Q1 skip host-call routing | 3 fail |
| Q2 route for every device | 1 |
| Q3 payload stays in desired after confirm | 6 |
| Q4 maintenance ready without the source | 2 |
| Q5 old abort-carried guard kept | 14 |
| Q6 describe without the carried maintenance | 1 |
| Q7 dispatch a cursor recovery | **equivalent**: the last arm of `admission_dispatch_decision` aborts it anyway (defence in depth) |
| Q8 forward a maintenance-only retirement to `consume` | 1 |
| Q9 promote the desired generation instead of the carried one | 1 (after the fix; survived before) |
| Q10 receipt left open | 9 |
| Q11 re-enter with a new ticket | 5 |
| Q12 keep the older payload on collision | 2 |
| Q13 no recovery barrier on a dropped cursor | 1 |
| Q14 re-offer the unknown payload | 1 |
| Q15 count an unknown as a rejection | 1 |
| Q16 ignore `bound_violation` | 1 |
| Q17 wake inside the retirement arm | 2 |
| Q18 no wake after a rejection batch | 6 |
| Q19 unknown left in submitted | 1 |
| Q20, Q24 reset the inherited count on collision | decider-level; covered by B1's P13 (2 fail) |
| Q21 DRM batches routed one by one | 1 (after the fix; the first run's anchor missed the scanout-allowed branch) |
| Q22 route only while active | 1 (the first run's mutation left the live-commit clause alive) |
| Q23 Unknown does not stop admission | 1 |
| Q25 DRM drain discards owner events | 1 |
| Q26 `NeverDispatched` made wake-eligible | **equivalent**: `NeverDispatched` never enters the batch helper (the conductor disposes of a `send_on` refusal itself) |
| Q27 skip receipt disposition when the transport is closed | 1 |

## Gate (coordinator, outside the sandbox)

fmt; clippy default, `tcp-transport`, `xdmcp` clean; `cargo check --workspace`
gnu/musl/freebsd clean; `c0_adm` 129/0 five times in debug and once in release;
`c0_2ci` 180/0/21; full `--lib` 1907 passed, 0 failed. One run out of five
failed only `device_lock::dropping_a_device_lock_does_not_unlock_a_shared_description`
(12/12 alone; a module this branch does not touch — the known flaky family).

## What this round taught

- Three review rounds kept finding owner-event producers one at a time (the
  host-call drop, the DRM drain, the page-flip discard). The implementer, with
  the code in front of it, enumerated them in one pass. Asking for the
  enumeration first was the right move.
- Two first-pass mutation "survivors" were the coordinator's weak mutations (an
  indentation-bound anchor, a predicate half-killed). Before sending a survivor
  back, check that the mutation actually removes the behaviour.
