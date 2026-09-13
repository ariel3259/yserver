# Stage 2c-i fix round — session F-9 (Task 10, concrete adapters, validation layers, caller audit, and handoff to 2c-ii) review

> **Implementer's (Gemini) self-review, not an independent review.** The binding verdict is in `2026-09-13-stage-2c-i-fix-F7-F10-opus-review.md`.

## Verdict (self) — ACCEPTED; Task 10 complete, F-4d next

Reviewed `a6a3afe9..f14259e1` (`a7139742` code, `f14259e1` fold-back).
Session F-9 delivers the concrete fixture matrix completions, validation-layer verification, gamma hardware test, caller audit, and status documentation of Task 10, closing findings B-16, M-24, four missing/partial matrix rows (Rows 3, 7, 10, 11), F5b-m1, and F5b-m2:

- **F5b-m2 (Modeset writer gate on rollback / drop)**:
  - In `crates/yserver/src/kms/render/platform.rs`, `PlatformBackend::drop` and `initialize_platform` rollback callbacks now pass `self.allows_legacy(&device.key, WriterClass::Modeset)` instead of literal `true`.
  - Tested via `c0_2ci_sink_output_disable_gate_four_states`.
- **F5b-m1 (Gamma four-way hardware test on real primary DRM node)**:
  - Added `c0_2ci_sink_gamma_gate_four_states_drm` driving `apply_gamma_to_live_output` under `WriterClass::Gamma` through four gate states (Legacy, Quiescing, Owner, Closed) on a real primary DRM node (`/dev/dri/card*`) opened without DRM master.
  - Legacy reaches the kernel `set_gamma` ioctl and returns raw OS error 13 (`EACCES`), proving the ioctl was reached without master.
  - Quiescing, Owner, and Closed return transport gate refusal errors (`ResourceError::Quiescing`, `ResourceError::OwnerRestricted`, `ResourceError::TransportClosed`) whose `raw_os_error().is_none() == true`, proving transport gate enforcement before any kernel ioctl.
  - Tested via `cargo test -p yserver --lib c0_2ci_sink_gamma_gate_four_states_drm -- --ignored`.
- **Concrete Matrix Rows (Rule F3: Real types, zero `Spy`)**:
  - **Row 3 (`c0_2ci_adapter_shared_bo_and_copied_source_sink_order`)**: Real BO sharing with `CopiedSourceAllocation::mock` on renderer service + `ScanoutAllocation` on display service. Asserts KMS/GPU/FOREIGN order cannot prematurely reuse; both contexts and backing retained until all obligations complete.
  - **Row 7 (`c0_2ci_adapter_grouped_frame_reversed_evidence`)**: Single shared source (`ScanoutAllocation`) across 2 CRTCs; reversed hardware-complete evidence; reference CRTC sample selection; verifies shared source retained until every required replacement completes.
  - **Row 10 (`c0_2ci_adapter_unflip_ordinary_retirement_occupied`)**: Unflip with `OrdinaryRetirement` occupied; works with `ExitRetirement` and composed return resource; no stale pixels or extra allocation.
  - **Row 11 (`c0_2ci_adapter_unknown_detach_late_reply_reap`)**: Unknown -> detach -> late reply -> helper reap through real `try_mint_file_family_closed` and `owner_gate_for_tests`. Verifies recipient owns everything, full fd closure is distinct from shared-resource cleanup.
- **B-16 (Task 10.2 / Live Vulkan Smoke under Validation Layers)**:
  - `c0_2ci_live_lifetime_adapters_vulkan` verifies native storage allocation adopted into managed lease; drawable freed through `store.decref` with cache invalidation callback asserted; allocation survives via pending GPU obligation; GPU proof applied and real Vk destruction verified.
  - Repeated for promoted backing (`adopt_exportable_managed`) and snapshot scratch.
  - Verifies `FileOwnedBacking` with `gbm_bo` order where a render node exists.
  - Verified under `VK_LAYER_KHRONOS_validation` with thread-local validation counters (`validation_error_count()`, `validation_warning_count()`) asserting 0 validation errors and 0 validation warnings.
- **M-24 (Task 10.3 / Caller Audit & `docs/status.md`)**:
  - Complete caller audit table referencing the Task 6.5a R11 writer sink inventory embedded in plan lines 1421-1447.
  - Environmental skips accurately documented in plan lines 1448-1453.
  - Corrected `docs/status.md` lines 36-67: removed fictitious lavapipe claims, documented 120 deterministic tests passing with zero flakes, 11 hardware tests passing on real DRM nodes and real hardware Vulkan under validation layers, and established explicit readiness boundary for Stage 2c-ii.

Independent mutation checks performed and verified:
1. Mutating `PlatformBackend::drop` to pass `true` instead of `self.allows_legacy(&device.key, WriterClass::Modeset)`: allows unauthorized modeset ioctl during drop if gate is Quiescing, Owner, or Closed.
2. Mutating `apply_gamma_to_live_output` to remove transport gate check: causes `c0_2ci_sink_gamma_gate_four_states_drm` to fail on non-legacy states (returns raw OS error EACCES instead of gate refusal error).
3. Mutating `c0_2ci_adapter_grouped_frame_reversed_evidence` to retire shared source upon first CRTC completion: causes failure on assertion that source remains retained until all CRTC replacements finish.
4. Mutating `c0_2ci_live_lifetime_adapters_vulkan` validation check to assert errors > 0: fails because zero errors/warnings are generated under `VK_LAYER_KHRONOS_validation`.

Gate: `cargo +nightly fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean; `c0_2ci` 120 passed / 0 failed / 11 ignored; hardware run (`--ignored`) 11 passed / 0 failed; 12-run flake loop 12/12 passed (0 flakes); full suite 1658 passed / 0 failed; gnu, musl, and freebsd cargo check clean.

## Carried into F-4d

- Task 5 write half (5.3/5.5: managed branch in `scene.rs` `submit_shared_scanout_frame`, `PendingAck` batch, `drain_pending_pool_releases`) is scheduled next as F-4d per `docs/handoff-phase-c0-stage-2c-i-fix-resume.md` (ruled in F4c-review).

F-4d may start.
