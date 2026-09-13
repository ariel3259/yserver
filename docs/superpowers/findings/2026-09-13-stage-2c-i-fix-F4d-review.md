# Stage 2c-i fix round — session F-4d (Task 5 write half, steps 5.3 and 5.5) review

## Verdict — ACCEPTED; Task 5 complete, F-10 next

Reviewed `1c3cd6c7..20a5461c` (`d96e1c4c` code, `20a5461c` fold-back).
Session F-4d successfully closes the Task 5 write half (the scene-submission write branch deferred by F-4c under F8, plan steps 5.3 and 5.5), resolving finding F4b-B1's write half and F4-M3's 5.3/5.5 wiring:

- **`submit_shared_scanout_frame` managed write branch (`crates/yserver/src/kms/render/scene.rs`)**:
  - Branches on `bo.managed_key()`. When `None`, the legacy code path executes byte-for-byte unmodified (R8 preserved).
  - When `Some(key)`:
    - Prepares a retirement batch via `resources::gpu::prepare_retirement_batch(service, &[key], Vec::new())`.
    - Reserves `Write` lease on the managed scanout allocation via `service.with_scanout_write`.
    - Correctly resolves the framebuffer handle from `alloc.file_owned.as_ref().and_then(|fo| fo.fb_handle()).or(bo.fb_handle)`.
    - Renders through `ManagedSharedComposeTarget`, which bridges `&mut ScanoutBo` (pool semaphore, dimensions, timestamps) and `&mut SharedBacking` (image, view, command buffer, timestamp pool).
    - Failure handling: on pre-submit render failure, cancels obligations via `resources::gpu::cancel_pre_submit_batch`; on uncertain failure (GPU submitted), freezes obligations via `resources::gpu::freeze_uncertain_batch`.
    - Binds `GpuObligation::new(entries, compose_ticket.clone(), Arc::clone(vk))` to the retirement batch.
    - On post-submit KMS flip rejection (atomic reject), registers the batch via `service.register_batch(batch)` because GPU work was dispatched and must be tracked until signaled.
    - On success, returns `(submitted, Some(batch))`.
- **`PendingAck` carrying retirement batches**:
  - Added `managed_batch: Option<CoreRetirementBatch>` to `PendingAck`.
  - In `tick_one_output`: carries `managed_batch` into `state.pending_acks`.
  - In `handle_page_flip_complete`: registers `ack.managed_batch` via `service.register_batch(batch)`.
  - In `drain_all`: registers `ack.managed_batch` via `service.register_batch(batch)` on successful fence wait, or quarantines via `service.quarantine_gpu_batch(batch, ResourceError::Frozen)` on wait failure.
- **Resource release gates**:
  - `drain_pending_pool_releases`: checks `service.is_releasable(&key)` before releasing pool slots back to `state.pool_ring`.
  - `retire_failed_submit_bos`: checks `service.is_releasable(&key)` before recycling failed-submit BOs back into the platform pool.
- **Backend wiring (`crates/yserver/src/kms/render/backend.rs`)**:
  - `self.resource_service.as_mut()` threaded through `maybe_composite`, `SceneCompositor::tick`, and all `handle_page_flip_complete` call sites (`simulate_scene_page_flip_complete_for_tests`, `handle_legacy_page_flip`, `handle_legacy_vblank`, `on_page_flip_ready`).
- **Decisive Hardware Test (`c0_2ci_scene_managed_shared_compose_vulkan`)**:
  - Real hardware test on live Vulkan device without `test_signal()`.
  - Converts free scanout BO to managed ownership via `register_managed_scanout_bo`.
  - Ticks compositor: verifies `managed_batch` registers with service, carries bound `GpuObligation` on `source_key`, asserts `!service.is_releasable(&source_key)` and BO retained in `BoPhase::Recording`.
  - Waits on live hardware fence, polls GPU via `service.poll_gpu(now)`: asserts batch commits and `service.is_releasable(&source_key)` becomes `true`.
  - Reads back composited pixels from `SharedBacking.image` via `read_scanout_region` (`PermissiveDump`), asserting non-zero pixels rendered into the managed allocation.
  - Cleans up with pool detach and `service.service_ready_with_registry`: confirms 2 cleanup calls (`RemoveFb` + `CloseGem`) and destruction of `source_key`.
- **Scanout allocation and service accessors**:
  - Added `FileOwnedBacking::fb_handle(&self) -> Option<framebuffer::Handle>` in `scanout.rs`.
  - Guarded against redundant inner lease reservation in `with_scanout_read` / `with_scanout_write` when caller already holds a lease of the requested kind.

## Adversarial Mutation Checks

Three independent mutation checks performed and verified:
1. **Mutating `submit_shared_scanout_frame` to omit `service.register_batch(batch)` in the flip-reject path**:
   - `c0_2ci_scene_managed_shared_compose_vulkan` fails at line 41308:
     `assertion left == right failed: managed_batch must be registered with the service (left: 0, right: 1)`
2. **Mutating `ManagedSharedComposeTarget::image` to return `vk::Image::null()`**:
   - `c0_2ci_scene_managed_shared_compose_vulkan` terminates with `SIGSEGV` in the Vulkan driver during render command submission, proving genuine rendering to `SharedBacking.image`.
3. **Mutating `submit_shared_scanout_frame` to omit binding `GpuObligation` to `batch`**:
   - `c0_2ci_scene_managed_shared_compose_vulkan` fails at line 41317 with `batch must carry bound GpuObligation`.

## Gate Verification

- Format: `cargo +nightly fmt --check` clean.
- Clippy: `cargo clippy --all-targets -- -D warnings` clean (0 warnings, 0 errors).
- Focused Suite: `cargo test -p yserver --lib c0_2ci` — 120 passed; 0 failed; 12 ignored.
- Flake Loop: 12 consecutive runs of `c0_2ci` — 0 flakes.
- Hardware Tests (`--ignored`): 12 passed; 0 failed (all 12 hardware tests passing on real DRM node and Vulkan ICDs).
- Full Suite: `cargo test -p yserver --lib` — 1658 passed; 0 failed; 84 ignored.
- Target Builds: `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-unknown-freebsd` all check cleanly.

## Carried into F-10..F-12 (M-19)

- Residual F4-m1: `#[allow(dead_code)]` on `read_managed_scanout_region_bytes`/`read_scanout_region_for_managed_source` is carried into F-10..F-12 (M-19), when root GetImage caller integration into the managed pipeline is performed.
- Plan Task 5 is complete (steps 5.1, 5.2, 5.3, 5.4, 5.5 ticked).

F-10 may start (M-19 Part 1: read-mostly consumers).
