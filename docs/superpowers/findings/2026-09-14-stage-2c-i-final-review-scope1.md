# Stage 2c-i final stage review — scope 1 of 3 (Tasks 1–4)

## Verdict — HOLDS. No blocking, no major, no minor findings.

Reviewed `76a93356..5678110b` restricted to Tasks 1–4: the ledger
(`resources/{mod,availability,lease}.rs`), DRM cleanup and the fd-family
barrier (`drm_cleanup.rs`), storage (`storage.rs`, `store.rs`) and scanout
payloads (`scanout.rs`, `vk/scanout.rs`, `platform.rs`), against
`docs/handoff-phase-c0-stage-2c-i.md` rulings R1–R12, the plan's Global
Constraints and each task's **Produces** block.

Reviewer: Opus (this document), interactive session, mutations run by hand
on this box. The implementers were Gemini (original) and Sonnet (fix round);
this reviewer wrote none of the code under review.

Excluded from the range deliberately: the three `origin/master` merges
(`3033fa8a`, `3183d8ca`, `474a153d`) and the upstream work they bring —
`vk/glyph.rs`, `vk/text_pipeline.rs`, the text shaders, `tools/`. Not ours,
not this stage's.

## Why this scope is the one that matters

Round 1's verdict rested on the claim that the three tests the handoff calls
decisive (2.5, 4.6, 9.5) did not prove their mechanisms, that **every**
`FileFamilyClosed` in the tree was a fabricated enum literal, and that the
R5 barrier was inverted — a payload waiting for the barrier while the barrier
waited for the payload. Two of those three tests live in this scope. So the
scope-1 question is not "does the code read well" but: **can I still break
the R5/R3/R4 chain without a test noticing?**

## Mutation battery (this reviewer, seven mutations)

Each: apply, run `cargo test -p yserver --lib c0_2ci -- --include-ignored`
(142 tests, hardware included), restore, confirm green again.

| # | Mutation | Ruling | Result |
| --- | --- | --- | --- |
| M1 | `try_mint_file_family_closed` refuses while any payload alias is live — the round-1 B-1 inversion, restored verbatim | R5 | **8 fail**, including all three decisive tests: `…_fd_family_barrier_discharges_payload_alias` (2.5), `c0_2ci_fd_family_barrier_real_gbm_payload_drm` (4.6/9.5), `c0_2ci_handoff_complete_fd_family_barrier_deterministic` (9.5) |
| M2 | barrier mints without walking/discharging the payload aliases | R5 | same 8 fail |
| M3 | `adopt_with_registry` registers no payload alias (round-1 B-2) | R5 | 8 fail, incl. `…_adopt_with_registry_registers_file_owned_alias_only` |
| M4 | `service_ready_with_registry` destroys a ready entry without discharging `file_owned` | R5/R4 | 6 fail, incl. `…_discharges_before_destroy`, `…_retries_failed_discharge` |
| M5 | `GemOwner::Gbm`'s right closes the GEM handle too — two closers on one kernel object | R3 | 3 fail, incl. `c0_2ci_drm_cleanup_gem_owner_gbm_never_closes_gem` and the real-gbm hardware test |
| M6 | `can_destroy` ignores pending obligations | R4/R9 | 10 fail, incl. `…_discharging_file_owned_leaves_shared_intact` and `…_shared_gpu_dependency_persists_after_file_rights_discharge` |
| M7 | `Storage::destroy`'s Managed arm keeps the lease alive | Task 3, M-20 | 2 fail (`…_managed_destroy_transitions_to_detached`, `…_detaches_before_drop`) |

**M1 and M2 are the ones that settle round 1.** The inversion that was the
round-3 *and* round-1 blocker is now caught by eight tests, three of them the
ones the handoff names as deciding the stage. The barrier's preconditions
(`submitters_detached`, `helper_reaped`, `control_closed`,
`non_payload_aliases == 0`) are checked unconditionally in every build, and
outstanding payload aliases are correctly *not* among them — the registry
walks and discharges them, then performs the description's last close.

One methodological note, recorded because it nearly cost a wrong conclusion:
M4's first form made both `match` arms `Ok(())`, the type stopped inferring,
and the run produced **no output at all**. An empty result is not "no test
detects it" — it is "nothing ran". Every mutation run must be confirmed to
have compiled before its result is read.

## Static checks (things a mutation cannot show)

- **R10 — core-thread only: holds.** No `unsafe impl Send`, no
  `unsafe impl Sync`, no `thread::spawn`, no `std::thread` anywhere under
  `resources/` outside test files.
- **R8 — nothing production-active: holds.** `KmsBackend`'s production
  constructor sets `resource_service: None` (`backend.rs:5249`);
  `install_resource_service` has exactly one caller in the whole tree
  (`backend.rs:29494`), inside a `#[cfg(test)]` module. No environment
  switch anywhere under `resources/`.
- **R9 — proofs are never fabricated: holds.** `apply_validated_proof` is
  `pub(in crate::kms::render::resources)` with exactly two producers, both
  correlating real evidence (`commit.rs:550` a KMS completion, `gpu.rs:294`
  a fence). The `store.rs`-facing shim is `#[cfg(test)]`, so a production
  caller would not compile. `FileFamilyClosed` is sealed (`_private: ()`)
  and constructed at exactly one site — inside `try_mint_file_family_closed`
  itself. The same-named variant in `handoff.rs` is an incident kind, not
  the proof.
- **R3 — one closer per kernel object: holds by inspection and by M5.**
  `DrmCleanupRegistry::consume` closes the GEM handle only for
  `GemOwner::Right`; `Gbm` transitions to `Discharged` without closing, and
  the gbm_bo drop (ordered after `RMFB`) is the sole `GEM_CLOSE`.

## What this scope did NOT cover, so the verdict is not overclaimed

- Tasks 5–7 (adapters, completion progress, transport permission boundary)
  and Tasks 8–10 (role transitions, handoff, integration evidence) — scopes
  2 and 3.
- R11's caller inventory at the real sinks: Task 6 work, scope 2.
- R6's displaced-pair producer and R7's transport gate: scopes 2 and 3.
- The upstream-merged glyph work, deliberately (not this stage).

## Carried forward, unchanged

`F13c-m1` and `F13c-m2` (the husk counter's skippable and underflow-silent
accounting) remain 2c-ii spec items; nothing in this scope changes their
status. `F13b-D1` likewise, though it belongs to scope 3's surface.

Scope 2 (Tasks 5–7) is next.
