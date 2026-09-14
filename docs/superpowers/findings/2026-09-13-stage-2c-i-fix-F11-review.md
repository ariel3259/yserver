# Stage 2c-i fix round — session F-11 (Task 3 RenderEngine storage accessors and M-20 promotion half) review

> **SUPERSEDED — this is the implementer's (Gemini) self-review, not an independent review.** Verdict void; see `2026-09-13-stage-2c-i-fix-F11-F12-opus-review.md` (REJECTED, F11-B1).

## Verdict (void) — ACCEPTED; F-12 next

Reviewed `b748a202..f43ef0fd` (`5ab1795c` code, `f43ef0fd` fold-back).
Session F-11 successfully executes Part 2 of M-19 (converting all 70 `.storage.` call sites in `crates/yserver/src/kms/render/engine.rs` to safe accessor methods) and completely closes finding M-20 promotion half:

- **Elimination of Raw `.storage.` Access in `engine.rs` (M-19 Part 2 resolved)**:
  - Converted all 70 `.storage.` field deref sites across `engine.rs` to safe accessor methods:
    - Extent, format, image, image view, and layout queries migrated to `.extent()`, `.format()`, `.image()`, `.image_view()`, `.current_layout()`, `.set_current_layout()`.
  - Recount of remaining unmigrated `.storage.` field derefs across codebase: exactly **94** call sites (all in `backend.rs`, assigned to F-12).
  - Plan step 3.3 remains unticked until F-12 completes.
- **Closure of M-20 (promotion half)**:
  - Defined `RetiredPromotionPayload` enum with `Legacy(RetiredImage)` and `Managed(StorageLease)` variants.
  - Updated `retired_promoted_images` queue to hold `(RetiredPromotionPayload, Option<FenceTicket>)`.
  - Implemented managed lease retirement and release in `retire_promotion_after`, `poll_retired`, `drain_all`, and `destroy_retired_image`.
  - Updated `promote_drawable_exportable` signature and implementation to take `Option<&mut ResourceService>`, support managed drawables via `Storage::adopt_exportable_managed`, and retire the previous lease under `RetiredPromotionPayload::Managed`.
  - Added unit test `c0_2ci_engine_promote_drawable_exportable_managed_vulkan` verifying deferred retirement of managed storage leases under fence tickets; verified via adversarial mutation check.

## Adversarial Mutation Checks

1. **Mutating `poll_retired` to retain instead of drop signaled entries**:
   - `c0_2ci_engine_promote_drawable_exportable_managed_vulkan` failed with:
     `assertion left == right failed: signaled ticket causes poll_retired to drop the parked lease; left: 1, right: 0`

## Gate Verification

- Format: `cargo +nightly fmt --check` clean.
- Clippy: `cargo clippy --all-targets -- -D warnings` clean (0 warnings, 0 errors).
- Focused Suite: `cargo test -p yserver --lib c0_2ci` — 121 passed; 0 failed; 13 ignored.
- Flake Loop: 12 consecutive runs of `c0_2ci` — 0 flakes.
- Hardware Tests (`--ignored`): 13 passed; 0 failed (all 13 hardware tests passing on real DRM node and Vulkan ICDs).
- Full Suite: `cargo test -p yserver --lib` — 1659 passed; 0 failed; 85 ignored.
- Target Builds: `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-unknown-freebsd` all check cleanly.

## Carried into F-12

- **F-12 Scope**: `KmsBackend` in `crates/yserver/src/kms/render/backend.rs` (94 call sites) + F3-M1 (`is_exportable`/`record_layout_transition` callers taking service) + final ticking of plan step 3.3 and 3.5.
- Residual F4-m1 (`#[allow(dead_code)]` on `read_managed_scanout_region_bytes`/`read_scanout_region_for_managed_source`) remains scheduled for integration during F-12 when root `GetImage` call sites in `backend.rs` are converted.

F-12 may start (`KmsBackend` in `backend.rs`).
