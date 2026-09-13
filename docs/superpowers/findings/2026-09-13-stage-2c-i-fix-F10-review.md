# Stage 2c-i fix round — session F-10 (Task 3 read-mostly consumers and F3-m1) review

> **Implementer's (Gemini) self-review, not an independent review.** The binding verdict is in `2026-09-13-stage-2c-i-fix-F7-F10-opus-review.md`.

## Verdict (self) — ACCEPTED; F-11 next

Reviewed `32eda95f..d7f66dbe` (`ebae39e0` code, `d7f66dbe` fold-back).
Session F-10 successfully executes Part 1 of M-19 (settling the `Storage` accessor shape and converting read-mostly consumers in `scene.rs`, `frame_builder.rs`, `target.rs`) and completely closes finding F3-m1:

- **`StorageBacking::Detached` (F3-m1 resolved)**:
  - Added `StorageBacking::Detached` variant in `crates/yserver/src/kms/render/resources/storage.rs` and `crates/yserver/src/kms/render/store.rs`.
  - In `Storage::destroy(&platform)` on `Managed`, drops the `StorageLease` immediately (releasing the `Retain` use in `ResourceService` at the call site, not when the wrapper is dropped) and transitions `self.backing` to `StorageBacking::Detached` instead of fabricating an inert `Legacy` stub.
  - Repeating `destroy(&platform)` on `StorageBacking::Detached` is idempotent and safe.
  - Safe accessors on `Detached`: returns `vk::Extent2D::default()`, `depth = 0`, `content_offset = (0, 0)`, `format = vk::Format::UNDEFINED`, `image_view = null()`, `sample_view = null()`, `has_image_view = false`, `is_detached = true`, `is_managed = false`, `is_exportable = false`.
  - Added unit test `c0_2ci_storage_managed_destroy_transitions_to_detached` and extended `c0_2ci_storage_managed_destroy_detaches_before_drop`.
- **Settled `Storage` / `Drawable` Accessor Shape (M-19 Part 1 resolved)**:
  - Extended `PixelIdentity` on `StorageLease` to carry `format: vk::Format`, `image_view: vk::ImageView`, and `sample_view: vk::ImageView`. Constant-time layout and view queries on managed drawables now read directly from `PixelIdentity` without raw borrows or panicking `Deref` calls.
  - Added safe methods on `Storage`: `extent()`, `depth()`, `content_offset()`, `format()`, `image_view()`, `sample_view()`, `has_image_view()`, `is_detached()`.
  - Added forwarding methods on `Drawable`: `extent()`, `image_view()`, `sample_view()`, `has_image_view()`.
- **Read-Mostly Consumers Converted**:
  - `crates/yserver/src/kms/render/scene.rs`: Converted all 17 `.storage.` field accesses to method calls (`.storage.extent()`, `.storage.sample_view()`, `.storage.image_view()`, `.storage.has_image_view()`).
  - `crates/yserver/src/kms/render/frame_builder.rs` (line 1619) and `crates/yserver/src/kms/render/target.rs` (line 23): Confirmed to be doc comments only (0 executable code call sites). Updated comments to reflect method syntax.
- **Recount of Remaining Call Sites**:
  - Exactly 164 `.storage.` call sites remain across the codebase:
    - `crates/yserver/src/kms/render/engine.rs`: 70 sites (assigned to F-11).
    - `crates/yserver/src/kms/render/backend.rs`: 94 sites (86 production + 8 test; assigned to F-12).
  - Plan step 3.3 remains honestly unticked until F-12 completes.

## Adversarial Mutation Checks

Three independent mutation checks performed and verified:
1. **Mutating `Storage::destroy` on `Managed` to fabricate a `Legacy` stub (reverting to pre-F-10 behavior)**:
   - Both `c0_2ci_storage_managed_destroy_detaches_before_drop` and `c0_2ci_storage_managed_destroy_transitions_to_detached` fail with:
     `panicked at crates/yserver/src/kms/render/store.rs:3253:9: Storage::destroy() must transition Managed to Detached`
2. **Mutating `Storage::format` on `StorageBacking::Managed` to return `vk::Format::UNDEFINED` instead of `lease.pixels.format`**:
   - `c0_2ci_storage_managed_destroy_transitions_to_detached` fails with:
     `assertion left == right failed: left: UNDEFINED, right: B8G8R8A8_UNORM`
3. **Mutating `scene.rs` line 5858 `if !source.storage.has_image_view()` to `if source.storage.has_image_view()`**:
   - 7 scene composition tests fail (`a_collapsed_universe_over_emits_but_shows_the_same_pixels`, `a_cow_subtree_claims_nothing`, etc.), proving the accessor is actively exercised and verified.

## Gate Verification

- Format: `cargo +nightly fmt --check` clean.
- Clippy: `cargo clippy --all-targets -- -D warnings` clean (0 warnings, 0 errors).
- Focused Suite: `cargo test -p yserver --lib c0_2ci` — 121 passed; 0 failed; 12 ignored.
- Flake Loop: 12 consecutive runs of `c0_2ci` — 0 flakes.
- Hardware Tests (`--ignored`): 12 passed; 0 failed (all 12 hardware tests passing on real DRM node and Vulkan ICDs).
- Full Suite: `cargo test -p yserver --lib` — 1659 passed; 0 failed; 84 ignored.
- Target Builds: `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-unknown-freebsd` all check cleanly.

## Carried into F-11 and F-12

- **F-11 Scope**: `RenderEngine` in `crates/yserver/src/kms/render/engine.rs` (70 call sites) + M-20 promotion half (`retire_image_after`/`destroy_retired_image` for managed payloads in `promote_drawable_exportable`).
- **F-12 Scope**: `KmsBackend` in `crates/yserver/src/kms/render/backend.rs` (94 call sites) + F3-M1 (`is_exportable`/`record_layout_transition` callers taking service) + final ticking of plan step 3.3.
- Residual F4-m1 (`#[allow(dead_code)]` on `read_managed_scanout_region_bytes`/`read_scanout_region_for_managed_source`) remains scheduled for integration during F-12 when root `GetImage` call sites in `backend.rs` are converted.

F-11 may start (`RenderEngine` in `engine.rs`).
