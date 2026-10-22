# Issue #281: fresh-frame unflip validation record

Date: 2026-10-21
Branch: fix/281-unflip-fresh-frame
Software base for the hardware comparison: master commit 8e71c788

## Scope and evidence sources

This record summarizes the implementation and software evidence from Tasks 1–5 and records the Task 6 gate run. The implementation uses one asynchronous prepare/readiness/group-submit flow for shared, copied, and mixed outputs. The grouped replacement uses each plane's final render or sink-copy dependency, assigns accepted IDs only after atomic acceptance, retires scene acknowledgements through the normal completion route, and keeps direct ownership until all accepted replacements retire. Lifecycle cancellation and bounded notification pacing are covered by the Task 4 and Task 5 backend tests.

Source reports: task-1-report.md, task-2-report.md, task-3-report.md, task-4-report.md plus task-4-report-part2.md, and task-5-report.md under /home/ariel_santangelo/Projects/claude-scratch/unflip-281/sdd. Task 4's final fix-round report and Task 5's final fix-round report supersede their earlier interim counts where they differ.

Every Cargo test command below used the lavapipe environment. Hardware tests were not run in Tasks 1–6.

## Gate commands

The per-task gate uses these commands, with `${LAVAPIPE_ENV_SCRIPT}` and `${FREEBSD_SYSROOT_ENV_SCRIPT}` set locally:

1. `cargo +nightly fmt --all`
2. `cargo build --bin yserver`
3. `source "${LAVAPIPE_ENV_SCRIPT:?set to the local lavapipe environment script}" && cargo test --workspace --locked --features xdmcp`
4. `source "${LAVAPIPE_ENV_SCRIPT:?set to the local lavapipe environment script}" && cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored`
5. `source "${LAVAPIPE_ENV_SCRIPT:?set to the local lavapipe environment script}" && cargo test -p yserver --test render_acceptance -- --ignored`
6. `cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings`
7. `cargo clippy --all-targets -- -D warnings`
8. `tools/check-layout.sh`
9. `source "${FREEBSD_SYSROOT_ENV_SCRIPT:?set to the local FreeBSD sysroot environment script}" && cargo clippy --target x86_64-unknown-freebsd --all-targets --locked --features xdmcp -- -D warnings`

Task 6 also ran the brief's workspace-wide FreeBSD variant: `source "${FREEBSD_SYSROOT_ENV_SCRIPT:?set to the local FreeBSD sysroot environment script}" && cargo clippy --target x86_64-unknown-freebsd --workspace --all-targets --locked --features xdmcp -- -D warnings`.

## Per-task gate results

### Task 1 — scene handoff completeness and identity

- `cargo +nightly fmt --all`, `cargo build --bin yserver`: pass.
- Regular workspace tests: 4,231 passed, 0 failed, 570 ignored.
- Whole ignored workspace suite: 570 passed, 0 failed.
- Ignored render acceptance: 206 passed, 0 failed.
- Default-feature Clippy and xdmcp workspace all-target Clippy: pass with `-D warnings`.
- `tools/check-layout.sh`: `layout: ok`.
- FreeBSD all-target Clippy: pass; compile/lint only.

### Task 2 — materialization before backing migration

- `cargo +nightly fmt --all`, `cargo build --bin yserver`: pass.
- Regular workspace tests: 4,231 passed, 0 failed, 573 ignored.
- Whole ignored workspace suite: 573 passed, 0 failed; no prerequisite skips.
- Ignored render acceptance: 206 passed, 0 failed.
- Xdmcp workspace all-target Clippy and default-feature all-target Clippy: pass with `-D warnings`.
- `tools/check-layout.sh`: `layout: ok`.
- FreeBSD all-target Clippy: pass; compile/lint only.

### Task 3 — prepared-plane ledger and grouped fence readiness

- `cargo +nightly fmt --all`, `cargo build --bin yserver`: pass.
- Regular workspace tests: 4,236 passed, 0 failed, 573 ignored.
- Whole ignored workspace suite: 573 passed, 0 failed.
- Ignored render acceptance: 206 passed, 0 failed.
- Xdmcp workspace all-target Clippy and default-feature all-target Clippy: pass with `-D warnings`.
- `tools/check-layout.sh`: `layout: ok`.
- FreeBSD all-target Clippy: pass; compile/lint only.
- Four new boundary tests were labeled pure boundary; the report did not claim production-backend repetitions for them.

### Task 4 — backend readiness, grouped acceptance, and retirement

- Final reported `cargo +nightly fmt --all`, `cargo build --bin yserver`: pass.
- Regular workspace tests: pass. Final report output includes yserver 1,279 passed / 297 ignored and yserver-render 593 passed / 69 ignored; remaining workspace targets and doctests passed or were ignored as marked.
- Whole ignored suite: pass. The final report lists yserver 297, render acceptance 206, yserver-render 69, GLX TFP 10, DRI3 FD-leak 1, and render doctests 2 as passed, with no failures.
- Ignored render acceptance: 206 passed, 0 failed.
- Xdmcp workspace all-target Clippy and default-feature all-target Clippy: pass with `-D warnings`.
- `tools/check-layout.sh`: `layout: ok`.
- FreeBSD all-target Clippy: pass; compile/lint only.
- Remaining coverage gap in the Task 4 report: lavapipe could not hold a real exported GPU completion fence unsignalled. The shared, mixed, copied, timeout, and lifecycle tests use the explicitly labeled platform readiness boundary.

### Task 5 — lifecycle quiescence and notification pacing

- Final reported `cargo +nightly fmt --all`, `cargo build --bin yserver`: pass.
- Regular workspace test output rows: yserver 1,280 passed / 299 ignored; yserver-core 1,868 passed; transport 2 passed; yserver-protocol 494 passed; yserver-render 593 passed / 69 ignored. All rows had zero failures.
- Whole ignored suite: 587 passed, 0 failed.
- Ignored render acceptance: 206 passed, 0 failed.
- Xdmcp workspace all-target Clippy and default-feature all-target Clippy: pass with `-D warnings`.
- `tools/check-layout.sh`: `layout: ok`.
- FreeBSD all-target Clippy: pass; compile/lint only.
- Count note: the Task 5 prose report says 4,234 regular tests passed, but its listed output rows sum to 4,237 when the transport and other passing target rows are included. This record preserves the exact per-target output rows rather than repeating that inconsistent aggregate.

### Task 6 — documentation and fresh gate

- `cargo +nightly fmt --all`: exit 0, no output.
- `cargo build --bin yserver`: exit 0; `Finished dev profile`.
- Regular workspace tests: 4,237 passed, 0 failed, 587 ignored. Target rows: yserver 1,280 passed / 299 ignored; yserver-core 1,868 passed; transport 2 passed; yserver-protocol 494 passed; yserver-render 593 passed / 69 ignored. Test binaries with ignored-only cases report those cases ignored in this run.
- Whole ignored workspace suite: 587 passed, 0 failed. Target rows include yserver 299, DRI3 FD-leak 1, GLX TFP 10, render acceptance 206, yserver-render 69, and render doctests 2.
- Separate `cargo test -p yserver --test render_acceptance -- --ignored`: 206 passed, 0 failed.
- `cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings`: exit 0; `Finished dev profile`.
- Exact repository CI command `cargo clippy --all-targets -- -D warnings`: exit 0; `Finished dev profile`.
- `tools/check-layout.sh`: exit 0; `layout: ok`.
- Both FreeBSD commands in the gate command list exited 0; each ended with `Finished dev profile`. These were compile/lint checks only.
- Cargo's ignored workspace and render-acceptance logs emitted Mesa `dri_gbm.so` loader warnings. There were no test skips or failures.

Task 6 FreeBSD ruling: the implementation rules require the cross-check before reporting, while this Task 6 brief says the final FreeBSD cross-clippy belongs at the end of Task 7. I ran the explicit Task 6 gate commands now and leave the Task 7 final rerun pending; the Task 6 result is not represented as the post-Task-7 result.

## Repeat protocol and skips

- GPU-backed software tests were invoked only after sourcing the supplied lavapipe environment. No hardware or DRM card node was used.
- Task 1: six live-scene handoff/admission tests ×5 and two other handoff tests ×5 under 16 busy-loop workers (40/40); one asynchronous quarantine test ×20 (20/20). No prerequisite skip.
- Task 2: the three `issue281_configure_` GPU migration tests ×20 each under 16 busy-loop workers (60/60). No prerequisite skip.
- Task 3: its four added pure-boundary cases did not require the 5× backend or 20× GPU repetition protocol; no busy-loop workers were started and no prerequisite skip was emitted.
- Task 4 part 2: nine held-readiness cases ×20 (180/180) and thirteen deterministic cases ×5 (65/65), under 16 busy-loop workers; zero skips. The final Task 4 fix round additionally recorded eight deterministic backend cases ×5 (40/40) and two lavapipe scene cases ×20 (40/40); zero skips.
- Task 5: `issue281_randr_quiescence_drains_deferred_copied_source` ×20, `issue281_pending_unflip_survives_rejection_and_lifecycle` ×20, and the pure-boundary wake predicate test ×5, under 16 temporary workers; zero skips. The final Task 5 fix round also recorded the P1 readiness-error regression ×20 and the three ConfigureWindow migrations ×20 each.
- Task 6: no new behavior tests or repeated backend fixtures were added; its gate ran each required suite once. No prerequisite skips.
- Worker scripts used cleanup traps and waited for their workers. Task reports record no surviving test workers. The non-fatal Mesa loader warnings are not skip lines.

## Test index

Repeat command template used for individual ignored backend filters: `source "${LAVAPIPE_ENV_SCRIPT:?set to the local lavapipe environment script}" && cargo test -p yserver --lib --locked --features xdmcp TEST_FILTER -- --ignored --quiet`. Each row gives the exact filter and repetition result. A later full ignored suite also exercised the final tree once.

| Named scenario/test | Command filter and recorded runs | Skip | End-state evidence or limit |
| --- | --- | --- | --- |
| `issue281_configure_no_background_resize_migrates_materialized_direct_pixels` | Same repeat template; filter of this name ×20 | None | Replacement backing contains the current direct-source pixels after materialization precedes migration; normal unflip retirement releases the direct pins. |
| `issue281_configure_border_change_migrates_materialized_direct_pixels` | Same repeat template; filter of this name ×20 | None | Border migration preserves the materialized direct pixels; normal unflip retirement releases direct pins. |
| `issue281_configure_migrations_materialize_current_and_pending_fallbacks_independently` | Same repeat template; filter of this name ×20 | None | Current and pending fallback identities retain their distinct pixels through migration and retirement. |
| `issue281_fullscreen_exit_flips_fresh_scene` | Same repeat template; filter of this name ×5 | None | Recorded primary FB contains the fresh full scene; prior composed BO remains reserved through replacement retirement; end-state helper runs. |
| `issue281_present_copy_preserves_newer_fallback_pixels` | Same repeat template; filter of this name ×5 | None | Normal Present-Copy writes newer scene C; fallback and grouped primary FB retain C through unflip retirement. |
| `issue281_group_waits_for_every_render_and_retires_once` | Same repeat template; filter of this name ×20 | None | Readiness boundary prevents early or per-CRTC replacement; wrong, old, duplicate, and partial cookies cannot release direct ownership; normal retirement completes. Real unsignalled GPU fence coverage remains unproven. |
| `issue281_shared_unflip_waits_for_every_output_readiness_boundary` | Same repeat template; filter of this name ×20 | None | Group waits for each output row; no accepted ID or early grouped submission while a row is held; normal retirement drains the group. Platform-boundary readiness only. |
| `issue281_mixed_shared_copied_unflip_waits_for_readiness_and_uses_sink_fence` | Same repeat template; filter of this name ×20 | None | Mixed route records sink-copy as the copied plane's final dependency and preserves descriptor ownership; normal grouped retirement. Platform-boundary readiness only. |
| `issue281_copied_only_unflip_uses_sink_dependency_and_signalled_sentinel` | Same repeat template; filter of this name ×20 | None | Fresh copied destination and sink dependency are recorded; `None` is encoded as signalled `-1`; normal retirement. Platform-boundary readiness only. |
| Task 4 brief scenario `issue281_copied_unflip_fences_sink_copy` | No standalone function with this exact name; covered by the three shared/mixed/copied filters above, each ×20 | Not a skip; alias mapping | Do not count a separate execution. The real unsignalled exported-GPU-fence subcase remains unverified. |
| `issue281_copied_sink_export_failure_quarantines_without_wait` | Same repeat template; filter of this name ×20 | None | Export failure publishes no accepted ID; possibly-writing destination remains Submitted/quarantined and pins remain held until production VT quiescence/drain. |
| Actual test `issue281_unflip_prepare_timeout_recovers_lost_and_cancelled_readiness` | Same repeat template; filter of this name ×20 in Task 4 part 2 and ×5 in the final fix-round deterministic set | None | Lost notification recheck advances exact complete rows; cancelled work retries safely once; unresolved writer remains quarantined through the second deadline and lifecycle cleanup. Test gate uses boundary event readiness. |
| Task 4 brief scenario `issue281_unflip_prepare_timeout_recovers_lost_or_cancelled_readiness` | No standalone function with this exact name; covered by `issue281_unflip_prepare_timeout_recovers_lost_and_cancelled_readiness` above | Not a skip; alias mapping | No separate invocation. The actual test uses the name with `and`; its timeout/readiness boundary coverage is recorded above. |
| `issue281_unflip_lifecycle_quiesces_vt_dpms_and_randr` | Same repeat template; filter of this name ×20 | None | VT, DPMS, and RandR quiescence drains prepared state/events before reset; no stale row resubmits and direct ownership is released only after safe drain. |
| `issue281_direct_exit_watchdog_prevents_96s_freeze` | Same repeat template; filter of this name ×5 | None | Watchdog progresses without unrelated input; accepted grouped replacement retires before direct pins are released. |
| `issue281_cow_descendant_unflip_does_not_exit_server` | Same repeat template; filter of this name ×5 | None | Materializes the pinned fallback target without treating CowDescendant identity as fatal; normal grouped retirement completes. |
| `issue281_cancellation_with_none_dependency_recycles_inline` | Same repeat template; filter of this name ×5 | None | Signalled/no-fence dependency cancels inline without a render-notification wait; normal fallback and retirement drain. |
| Task 4 brief scenario `issue281_persistent_group_rejection_uses_degraded_path` | No standalone function with this exact name. Related filters: `p1_degraded_shared_unflip_waits_for_accepted_replacements` ×5 and copied P1 regressions below | Not a skip; mapping and gap | Existing per-output fallback behavior is exercised, but the real held-dependency persistent-EIO/same-BO matrix and descriptor-exhaustion recovery remain incomplete per the Task 4 report. |
| `p1_degraded_shared_unflip_waits_for_accepted_replacements` | Same repeat template; filter of this name ×5 | None | Rejected group publishes no ID; per-output accepted IDs and both retirements are tracked; direct pins survive first retirement and release after final valid retirement. |
| `p1_degraded_copied_unflip_registers_immediately_accepted_replacement` | Same repeat template; filter of this name ×20 in Task 4 part 2; final ignored suite pass | None | Copied fallback registers only its accepted replacement; normal retirement drains copied ownership. |
| `p1_degraded_copied_unflip_has_no_id_before_sink_acceptance` | Same repeat template; filter of this name ×20 | None | No group/replacement ID before source readiness and sink acceptance; cancellation drains before the per-output fallback. |
| `p2_direct_hook_treats_deferred_as_failure_and_unflips` | Same repeat template; ×5 deterministic and ×20 GPU-backed in final Task 4 fix round | None | Fresh-pixel check, final in-flight cursor gate, then one normal grouped retirement; end-state helper confirms no pending ownership. GPU work runs on lavapipe but does not hold a real exported fence unsignalled. |
| `p2_scene_direct_copied_and_unflip_commits_carry_only_the_applied_position` | Same repeat template; ×5 deterministic and ×20 GPU-backed in final Task 4 fix round | None | Fresh copied pixels and only the applied position carrier are checked; scene cursor transition occurs once and normal retirement drains. |
| `p2_tearing_cursor_handoff_transitions_cursor_once` | Same repeat template; filter of this name ×5 in final Task 4 fix round | None | Exact tearing reason, applied position only, one scene-owned cursor transition, and normal end-state cleanup. |
| `issue281_randr_quiescence_drains_deferred_copied_source` | Same repeat template; filter of this name ×20 in Task 5 | None | Production all-off/event/render drain clears ack and source-completion row before topology mutation; discarded work never reaches sink-copy submission. |
| `issue281_pending_unflip_survives_rejection_and_lifecycle` | Same repeat template; filter of this name ×20 in Task 5 | None | Bounded EBUSY retry, VT/DPMS/RANDR cancellation/quiescence, idle sequence separation, no stale resubmit or unsafe BO reuse; finishes through production drains. |
| `stale_unflip_topology_wakes_cancellation_but_notification_wait_does_not_spin` | Same repeat template; filter of this name ×5; explicitly **pure boundary** | None | Predicate check confirms cancellation has a deadline while a notification wait has no immediate busy wake; not an end-to-end backend scenario. |
| `p1_unflip_dependency_readiness_error_does_not_block_backend` | Same repeat template; filter of this name ×20 in Task 5 final fix round | None | Readiness error leaves the dependency and BO quarantined; later worker notification drains safely, with no pending scene/pin/cookie state. |

## Hardware validation — pending Task 7

| Validation | Status | Attribution and evidence |
| --- | --- | --- |
| Two-output NVIDIA RTX 5060 Ti direct-scanout exit repro for issue #281 | Pending Task 7 authorization and run | No local hardware run in Tasks 1–6. Compare the fresh primary framebuffer and visible frame after leaving direct scanout. |
| Two-output AMD RX 6800 grouped replacement | Pending Task 7 evidence | The requested result must be attributed to the maintainer-provided RX 6800 run, never to local validation. No result details were present in the Task 1–6 reports reviewed for this record. |
| master `8e71c788` comparison | Pending Task 7 | No master hardware comparison run is recorded here. |

## Uncovered subcoverage and concerns

- The test fixture could not safely keep a real exported Vulkan sync-file unsignalled under lavapipe. Readiness-boundary tests check grouping, fence provenance, descriptor ownership, quarantine, and retirement, but they do not prove actual KMS acceptance waits on a physically unsignalled render or sink-copy fence.
- Descriptor-exhaustion recovery for both shared and copied recording paths remains incomplete.
- The persistent-EIO path with a real held dependency and same-BO copied fallback remains incomplete. Persistent grouped failure policy stays unchanged.
- Some Task 4 assertions lack mutation evidence, including the P2 fresh-pixel checks, copied same-BO fallback, FD counter, and preparing-output resumption. A duplicate external cursor handoff mutation was not killed because hide was state-idempotent.
- Hardware behavior and the absence of the visible stale-frame flash remain pending Task 7.
- Mesa's missing `dri_gbm.so` loader diagnostics are environmental; they produced no skips or test failures.

## Task 6 RED/GREEN and mutation record

Task 6 changes documentation only, so it introduced no behavioral test, RED run, GREEN run, or mutation. The Task 6 RED/GREEN status is not applicable; the gates above provide software validation of the final branch. Tasks 1–5 mutation evidence remains in their source reports. The decisive evidence includes: Task 1 completeness/identity mutations failed their handoff assertions; Task 2 materialization/readiness mutation failed the expected fallback pixel check; Task 3 pure-boundary mutations exercised captured generation, copied sink-fence choice, and cancellation-row cleanup; Task 4 mutations killed readiness bypass, wrong copied fence, timeout cancellation, export-failure recycling/publication, fallback overwrite, lifecycle stale rows, EBUSY classification, and premature direct release; Task 5 mutations killed retry-budget, unresolved-writer cancellation, idle-sequence completion, pending copied-frame admission, sink-copy observer, stale-row cleanup, and blocking readiness-error changes. The exact failure assertions and revert results are in the linked task reports.

## Self-review

The record distinguishes production Backend routes from pure-boundary predicates and does not label boundary readiness as a real unsignalled GPU fence. Per-test rows map Task 4 brief aliases to the exact functions actually present. Task 5 lifecycle tests end through all-off and old-topology event drains. No hardware result is claimed. Task 7 and its final FreeBSD rerun remain pending.
