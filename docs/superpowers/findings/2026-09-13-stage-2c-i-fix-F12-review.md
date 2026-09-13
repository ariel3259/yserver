# Stage 2c-i fix round — session F-12 (Task 3 backend.rs storage accessors and F3-M1) review

## Verdict — ACCEPTED; Final Stage Review next

Reviewed `f43ef0fd..67871480` (`96c10eab` code, `67871480` fold-back).
Session F-12 successfully executes Part 3 of M-19 (converting all remaining 94 `.storage.` call sites in `crates/yserver/src/kms/render/backend.rs` to safe accessor methods) and completely closes finding F3-M1:

- **Elimination of Raw `.storage.` Access in `backend.rs` (M-19 completed)**:
  - Converted all 94 `.storage.` field access sites across `backend.rs` to use safe accessor methods:
    - Content clipping, glyph sources, snapshots, seed backing: migrated to `.extent()`, `.format()`, `.image_view()`, `.image()`.
    - Inferior overlay and planning (`overlay_backing_inferiors`, `plan_backing_inferiors`): migrated to `.extent()`, `.format()`.
    - Test helpers (`drawable_current_layout_for_tests`, `masked_copy_area_for_tests`): migrated to `.current_layout()`, `.extent()`, `.image_view()`.
    - Caching and dimensions (`pending_clip_mask_cache`, `read_fill_pattern_cache`, `refresh_clip_mask_snapshot`, `drawable_dims`): migrated to `.extent()`, `.image_view()`.
    - Redirected geometry check (`update_redirected_backing_geometry`): migrated to `.extent()`.
    - Root background styling (`set_container_background_pixel`, `set_container_background_pixmap`): migrated to `.format()`.
    - Pixel reading (`copy_plane`, `get_image`, `read_depth1_pixmap`): migrated to `.extent()`.
    - Buffer export and promotion (`dri3_export_pixmap_buffers`, `promote_pixmap_exportable`, sync export, present pin): migrated to `.extent()`, `.imported_drawable()`, `.export_metadata(service)`, and `.is_exportable(service)`.
    - Subwindow and COW tests: migrated to `.extent()`.
  - Recount across entire repository: exactly **0** unmigrated `.storage.` field accesses remain.
  - Plan steps 3.3 and 3.5 are now ticked.
- **Closure of F3-M1**:
  - `Storage::is_exportable(&self, service: Option<&mut ResourceService>) -> bool` and `Drawable::is_exportable` read managed storage leases safely via `svc.with_storage_read(lease, StorageAllocation::is_exportable)` instead of panicking on `StorageBacking::Managed`.
  - `Drawable::record_layout_transition` handles `StorageBacking::Managed(lease)` safely: updates `lease.current_layout.set(target_layout)` and records Vulkan image barrier when command buffer and image are valid, without panicking.
  - Residual F4-m1: `#[allow(dead_code)]` on `read_managed_scanout_region_bytes`/`read_scanout_region_for_managed_source` is intentionally retained until non-test callers exist (ruled in F4c review).

## Adversarial Mutation Checks

Two independent mutation checks performed and verified:
1. **Mutating `Drawable::record_layout_transition` on `Managed` to set `vk::ImageLayout::UNDEFINED` instead of `target_layout`**:
   - `c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan` failed with:
     `assertion left == right failed: Drawable::record_layout_transition updates lease.current_layout without panic; left: UNDEFINED, right: COLOR_ATTACHMENT_OPTIMAL`
2. **Mutating `Storage::is_exportable` on `StorageBacking::Managed` to unconditionally return `false`**:
   - `c0_2ci_storage_is_exportable_managed_reserves_read_and_refuses_when_written` failed with:
     `assertion failed: managed.is_exportable(Some(&mut service))`

## Gate Verification

- Format: `cargo +nightly fmt --check` clean.
- Clippy: `cargo clippy --all-targets -- -D warnings` clean (0 warnings, 0 errors).
- Focused Suite: `cargo test -p yserver --lib c0_2ci` — 121 passed; 0 failed; 13 ignored.
- Flake Loop: 12 consecutive runs of `c0_2ci` — 0 flakes.
- Hardware Tests (`--ignored`): 13 passed; 0 failed (all 13 hardware tests passing on real DRM node and Vulkan ICDs).
- Full Suite: `cargo test -p yserver --lib` — 1659 passed; 0 failed; 85 ignored.
- Target Builds: `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-unknown-freebsd` all check cleanly.

## Stage 2c-i Fix Round Status

All fix sessions (F-1, F-1b, F-2, F-2b, F-3, F-3b, F-4, F-4b, F-4c, F-4d, F-5a, F-5b, F-6a, F-6b, F-7, F-8, F-9, F-10, F-11, F-12) are **ACCEPTED**.
Task 3 is complete.
Proceed to Final Stage Review over `76a93356..HEAD` against the 2c-i design spec and rulings.
