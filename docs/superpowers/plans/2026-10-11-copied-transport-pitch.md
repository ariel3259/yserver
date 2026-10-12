# Copied PRIME transport: pad the transport width so every sink can import it — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The copied scanout route (render on GPU A, scan out on GPU B) works at output widths where A's linear pitch is not a multiple of B's import alignment. The known widths are 1366, 1440 and 1680 on an NVIDIA renderer with a radv sink.

**Architecture:** Only the DMA-BUF **transport** of `CopiedRenderSource` gets a padded width: the logical width rounded up to a multiple of 64 pixels (256 bytes for BGRA8). Everything else keeps the logical extent:
- the optimal render target;
- `width()` / `height()`;
- every copy region (render target → transport on A, transport → destination on B);
- the readbacks and digests;
- the destination BOs.

The padded columns are never read into a destination. If an import still fails, the existing exact-plan qualification reports it as today.

**Tech Stack:** Rust, Vulkan (ash), `VK_EXT_image_drm_format_modifier`, DMA-BUF.

**Finding:** `docs/superpowers/findings/2026-10-11-copied-prime-nvidia-radv-linear-pitch.md` on the fork's reference branch `feat/phase-c0-atomic-kms-migration` (`b4e83809`). It measured:
- NVIDIA RTX 5060 Ti (driver 615.71.09) gives a linear pitch of 5504 bytes at width 1366 (128-byte aligned), and radv rejects it with `ERROR_INVALID_DRM_FORMAT_MODIFIER_PLANE_LAYOUT_EXT`;
- Mesa 26.1.8 radv requires a linear pitch that is a multiple of 256 bytes on GFX9–GFX11 (`ac_surface.c` `ac_surface_get_pitch_align`, `ac_surface_override_offset_stride`);
- 1920, 2560, 3840, 1280 and 1000 import on this host; the extra widths 1440 and 1680 are predicted from the 128-byte rule and are confirmed by this plan's test.

**Base:** `joske/master` `0e5c09ea` (layout refactor phase 2 merged), branch `fix/copied-transport-pitch`, worktree `~/Projects/yserver-fix-copied-pitch`. It is independent of the KMS-owner PR series and goes upstream as its own small PR.

## Global Constraints

- Behaviour changes only for the transport allocation and its sink import. No change to destination BOs, KMS, framebuffers or the render target.
- No environment variable or flag (joske, PR 129). No `cfg(test)` forks in production bodies. No side effects inside `debug_assert!`.
- The padding is a pure function of the logical width and the format's bytes per pixel, named and documented at its definition (why 256 bytes: radv's GFX9+ linear pitch alignment, the strictest known sink; GFX12 needs 128, Intel 64).
- Upstream file-size ceilings: no non-test `.rs` file over 5,000 lines, no test `.rs` file over 6,000.
- Test rules: **A** end-state checks; **C** the real path on hardware (here: the two GPUs of this host, no display needed); **F** new tests 5× under `nproc` busy loops.
- **Hardware safety for the `two_gpu` tests:**
  - only render nodes and Vulkan device files are opened, never a `/dev/dri/card*` node;
  - `nvidia-smi` shows no `ERR!` before the run;
  - `journalctl -k -b | grep -c Xid` does not grow.
- Commits: user's git identity, trailers `Implemented-By: codex (…)` and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, never a session URL.

## Review Focus

1. **A copy region or readback that uses the transport's allocated extent instead of the logical one.** It would read or write the padded columns into the destination or the digest. Expected: every region uses `width()` / `height()` (logical). Pinned by the pixel test of Task 1.
2. **Memory accounting and the qualification probe** (`allocate_exact_for_disposable_probe`) see the padded transport. Expected: the probe uses the same padded allocation as the live pool, so it qualifies what will run.
3. **A width already a multiple of 64.** Expected: no padding; the allocation is byte-identical to today's.

## Gate

1. `cargo +nightly fmt`, then `cargo +nightly fmt -- --check`.
2. `cargo clippy --all-targets $F -- -D warnings` for `F` in `""`, `--features tcp-transport`, `--features xdmcp`.
3. `cargo test -p yserver-core --locked`; `cargo test -p yserver --lib --locked`; each integration test file under `crates/yserver/tests/`.
4. Lavapipe (the existing `--ignored` suite with the lavapipe ICD, `--skip two_gpu`).
5. `cargo test -p yserver --lib --locked two_gpu -- --ignored` with the host ICDs (no `VK_DRIVER_FILES`), with the hardware safety checks above.

---

### Task 1: Pad the copied transport width

**Files:**
- Modify: `crates/yserver/src/kms/vk/scanout/copied.rs` (`CopiedRenderSource::allocate_exact`)
- Modify, if the transport allocation needs the padded width passed through: `crates/yserver/src/kms/vk/target.rs` (`allocate_copied_source_exact`)
- Test: the tests module of `kms/vk/scanout/copied.rs`, or its existing test file

**Interfaces:**
- **Produces:** `fn copied_transport_width(logical_width: u32) -> u32`, which rounds up to a multiple of 64 pixels for BGRA8 (a `const` alignment named for its reason).
- **Allocation:**
  - The transport is allocated and imported at `(copied_transport_width(width), height)`.
  - `allocate_transfer_resources` and the render target stay at `(width, height)`.
  - `CopiedRenderSource` keeps `width()` / `height()` = logical (they already read the render target).

**Invariants:**
- The renderer-side copy (render target → transport) and the sink-side copy (transport import → destination) both use the logical extent. Verify every `vk::ImageCopy`, `BufferImageCopy` and digest in `copied.rs`, and list them in the report.
- The sink import passes the renderer's actual offset and stride for the padded image, unchanged, as today. The fix is in what is allocated, not in what the import descriptor claims.
- A logical width that is already a multiple of 64 allocates exactly as today.

- [ ] **Step 1: Tests first.**
  - `copied_transport_width_rounds_to_64_pixels` (unit): 1366 → 1408, 1440 → 1472, 1680 → 1728, 1920 → 1920, 1 → 64, 64 → 64. **Mutation:** round to 32.
  - `copied_source_imports_on_the_sink_at_unaligned_widths_two_gpu`, `#[ignore = "needs the NVIDIA and AMD Vulkan devices"]`.
    - **Setup:** renderer and sink contexts built as production builds them for copied scanout (`VkContext::new_for_render_device` for the renderer; the sink found through the renderer context's DRM-device inventory, then `VkContext::new_transfer_for_device`).
    - **For** 1366×768, 1440×900, 1680×1050, 1920×1080 and 2560×1440: `CopiedRenderSource::allocate_exact` with the linear plan succeeds.
    - **Then** a pattern rendered into the render target, copied through the transport and read back on the sink matches at the logical extent. Use the existing probe-pattern and digest machinery the copied qualification already uses (`CopiedProbePatternPipeline`, the render/sink digests); say in the report which entry point.
    - **Mutation:** allocate the transport at the logical width. 1366, 1440 and 1680 then fail at import.
  - **If the host lacks either device, the test fails with a clear message, never skips.**
- [ ] **Step 2:** Run them; the unit test fails (missing function), and the `two_gpu` test fails at 1366.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Gate (all five steps), and the load gate for the new tests.
- [ ] **Step 5:** Commit `fix(kms): pad the copied PRIME transport so linear-pitch sinks can import it`.

### Task 2: Upstream PR text (coordinator)

- [ ] Prepare the PR description: the failing widths, the radv rule with its Mesa source lines, the measured NVIDIA pitches, and the test. Push and open the PR only when the user says so.
