# Stage 2c-i final stage review — scope 3 of 3 (Tasks 8–10)

## Verdict — HOLDS. No blocking, no major, no minor findings. One observation.

Reviewed `76a93356..d1e8397a` restricted to Tasks 8–10: direct role
transitions and capacity (`resources/capacity.rs`, the managed direct seam
in `backend.rs`), the retaining incarnation handoff (`resources/handoff.rs`,
teardown release in `mod.rs`) and Task 10's integration evidence, against
**R7**'s handoff half, **R8**, **R9** and **R12**.

Reviewer: Opus (this document). Seven mutations plus two static audits, run
by hand on this box; each mutation applied, run over the full 142-test
`c0_2ci` suite with hardware included, reverted, confirmed green.

## Mutation battery

| # | Mutation | Ruling | Result |
| --- | --- | --- | --- |
| P1 | `apply_teardown_release` accepts an entry whose file-owned half is still live | M-10/R4 | **1 fail** (`…_apply_teardown_release_refuses_live_file_owned`) |
| P2 | teardown release accepts an `Outstanding` KMS obligation | 9.5a | **2 fail**, incl. `…_handoff_unresolved_kms_rejects_teardown_release` |
| P3 | `record_device_barrier` ignores the device key — a foreign barrier supersedes | 9.5a | survives; **not a finding**, see the observation below |
| P4 | `HandoffRouter::transfer` closes before revoking | R7/B-7 | **2 fail** |
| P5 | a revoked grant is treated as cancelled — no quarantine | R7 | **1 fail** (`…_revokes_grant_and_quarantines`) |
| P6 | `finish_role` accepts a merely `Reserved` token | F-7 | **1 fail** (`…_finish_role_rejects_merely_reserved_token`) |
| P7 | `on_available` drops the failing resource instead of restoring it | F-7 | **1 fail** (`…_on_available_error_restores_all_resources_safely`) |

## Static audits

- **R12 — hardware tests report honestly: holds.** Every `c0_2ci_*` test
  ending in `_vulkan` or `_drm` was scanned for a bare `return` used as an
  environmental skip. There are none: all report the skip with `panic!`.
  (Tests outside this stage's `c0_2ci_` prefix do use the old bare-`return`
  shape; they predate 2c-i and R12 does not reach them.)
- **R8 — the Task 8 seam is not production-active: holds.**
  `managed_dispatch_direct_successor` and `managed_prepare_direct_candidate`
  carry `#[allow(dead_code)]` and every caller of either sits inside a
  `#[cfg(test)]` module. The one `prereserve_retirement` call that looks
  like production (`backend.rs:18487`) is *inside*
  `managed_dispatch_direct_successor` itself, so it inherits that status —
  I traced the enclosing function rather than trusting the line's position
  relative to the nearest `#[cfg(test)]`, because the attribute at
  `backend.rs:16377` is on a single function, not on a module, and reading
  it as a module boundary would have produced the wrong answer in both
  directions.

## Observation (not a finding)

**P3's survival is redundancy, not a coverage gap.** Plan step 9.5a demands
that "a `FileFamilyClosed` barrier for a second device key does not
supersede this entry's obligation". Deleting the device check in
`record_device_barrier` leaves the suite green — but the property itself is
still proven end to end: the test records a genuine second-device barrier
and asserts teardown release still refuses, and it does, because
`apply_teardown_release`'s `Superseded` arm independently requires
`barrier.device() == self.device`. Two guards enforce one property and the
test observes the outcome rather than either guard.

That is the right thing for a test to assert, so nothing here needs fixing.
Worth recording only so a future reader who deletes the "redundant" check in
`record_device_barrier` knows what they are giving up: `kms_disposition()`
would then report `Superseded` by a foreign device's barrier. No production
code reads that accessor today — its only callers are tests — so the blast
radius is currently zero, and it interacts with S2-m1's stale-disposition
hygiene rather than standing on its own.

## Stage-wide verdict after all three scopes

| Scope | Tasks | Verdict |
| --- | --- | --- |
| 1 | 1–4 (ledger, cleanup, storage, scanout payloads) | HOLDS — 7 mutations, all caught |
| 2 | 5–7 (adapters, completion, transport boundary) | **NOT CLEAN** — S2-M1, S2-M2 major; S2-m1 minor |
| 3 | 8–10 (roles, handoff, integration) | HOLDS — 7 mutations, all caught |

Twenty-one mutations across the stage; eighteen caught, three survived, and
of those three, two are real coverage gaps (S2-M1, S2-M2) and one is a
ledger-expressiveness minor (S2-m1). The round-1 verdict's core complaint —
that the R5 chain was inverted and that the three decisive tests proved
nothing — does not survive contact with the mutations: restoring the
inversion now fails eight tests, including all three.

**The stage is not accepted yet.** One tests-only fix session (**F-14**)
closes S2-M1, S2-M2 and S2-m1. No production code changes; the mechanisms
are correct as written. After F-14 and its review: `docs/status.md`, then
2c-ii's spec, which carries F13b-D1, F13c-m1 and F13c-m2.
