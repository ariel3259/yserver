# GPU-side clip for clip-masked CopyArea — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the CPU-mask-run + N-transfer-blit + clip-`get_image`-readback path for a clip-masked GXcopy `CopyArea` with **one sampled graphics draw** that binds the clip-mask's depth-1 GPU image and discards masked-out fragments.

**Architecture:** A new dedicated `masked_blit` graphics pipeline (texelFetch, no blend, identity image views) samples src verbatim and thresholds a depth-1 clip mask. The mask is read from a **GC-owned pinned GPU snapshot** (retain-after-free + same-frame ordering via explicit `PipelineBarrier2`). Built in two phases with an empirical byte-exactness gate as the GO/NO-GO checkpoint between them.

**Tech Stack:** Rust, `ash` (Vulkan 1.3), GLSL compiled by `glslc` via `build.rs`, lavapipe for in-sandbox acceptance tests.

**Source design:** `docs/superpowers/specs/2026-06-21-gpu-side-clip-copyarea-design.md` (codex round-3 GO).

---

## Orientation: key files and what they own

- `crates/yserver/src/kms/vk/shaders/*.glsl` — GLSL sources; `build.rs` auto-compiles every `*.glsl` to `$OUT_DIR/<stem>.spv`. Stem is `name.stage` (e.g. `masked_blit.frag`).
- `crates/yserver/src/kms/vk/render_pipeline.rs` — `RenderPipelineCache`: the template for a graphics pipeline (layout, descriptor-set layout, sampler, per-format pipeline build, descriptor writes). We clone a trimmed version for `masked_blit`.
- `crates/yserver/src/kms/vk/ops/render.rs` — `record_render_composite_{open,draws,close}`: the template for emitting a graphics draw into a CB (barrier → begin_rendering → bind → scissor → push consts → draw → end_rendering → barrier back).
- `crates/yserver/src/kms/v2/engine.rs` — `RenderEngine`/`RenderEngineInner`: frame builder, `copy_area`/`cow_copy_area` append, `emit_recorded_copy_area_into_cb`, `close_open_frame`, `rollback_pre_submit`/`rollback_atlas`, `allocate_scratch_image`, `barrier_to_layout`, `clamp_rect`, `decode_x11_pixel_for_storage`/`pack_from_storage`, `descriptor_pool_ring`, `render_pipelines`.
- `crates/yserver/src/kms/v2/frame_builder.rs` — `RecordedOp` enum, `RecordedCopyArea`, `OpenFrame`, `TouchedDrawables`, `FrameLayoutTable`, `push_op_and_set_layouts`.
- `crates/yserver/src/kms/v2/store.rs` — `Storage` (`image_view` IDENTITY, `sample_view` swizzled), `Drawable` (`content_version`, `last_render_ticket`, `current_layout`), `damage`, `touch_render_fence`, `mark_contents_modified`.
- `crates/yserver/src/kms/v2/platform.rs` — `build_sample_view` / `sample_view_components`, drawable storage allocation.
- `crates/yserver/src/kms/v2/backend.rs` — `copy_area` handler (clip branches), `intersect_with_current_clip_live`, `install_clip_mask_cache`, `read_clip_mask_bytes`, `clip_cache_reusable`, `set_clip_pixmap`, the `ClipMaskCache`.
- `crates/yserver/src/kms/v2/telemetry.rs` — `record_copy_area_call`, `record_copy_area_gpu_subrect_at(maskrun: bool)`, `GetImageSite::ClipMask`, `record_get_image_site`.
- `crates/yserver/src/kms/backend.rs` — `ClipMaskCache` struct (cross-version cache).
- `crates/yserver/tests/v2_acceptance.rs` — `#[ignore]` lavapipe tests; pattern `KmsBackendV2::for_tests_with_vk()` + ops + `get_image_pixels_for_tests`.

## Commands you will use

- Build: `cargo build -p yserver`
- Lint (project default — plain, NOT pedantic): `cargo clippy -p yserver`
- Format: `cargo fmt`
- Crate-local tests: `cargo test -p yserver <name>`
- Lavapipe acceptance tests (in-sandbox): `cargo test -p yserver --test v2_acceptance -- --ignored <name>`

> **Memory note:** the `#[ignore = "needs live Vulkan ICD"]` `v2_acceptance` tests DO run in this sandbox under lavapipe. They are the render/clip correctness gate. A vng pass is NOT an HW pass; the HW/vng gate (Task 16) is the release gate.

## Conventions to respect (from the codebase)

- **N1 single-terminal-layout rule:** every drawable an op touches ends the op at `SHADER_READ_ONLY_OPTIMAL`; the op's emit derives barriers from recorded *old* layouts.
- **N8 allocate-before-mutate:** allocate scratch/snapshot images BEFORE mutating any open-frame state, so an allocation failure returns `Err` with the frame untouched.
- **8-writer discipline / content_version:** a write op bumps `content_version` via `store.mark_contents_modified(dst)` and stamps `touch_render_fence(dst)`.
- **No stubs / do-it-right:** no placeholder pipeline, no "sample-zero outside" shortcut. OOB src → `discard`.

---

# PHASE 1 — The `masked_blit` engine primitive + empirical exactness gate

Phase 1 builds a GPU primitive `engine.masked_copy_area(...)` that samples an **arbitrary** mask image (a plain drawable in the tests; the GC-owned snapshot in Phase 2). It is byte-exactness-gated. **Phase 2 does not start until the Task 10 exactness gate passes for at least one format.**

---

### Task 1: `masked_blit` GLSL shaders

**Files:**
- Create: `crates/yserver/src/kms/vk/shaders/masked_blit.vert.glsl`
- Create: `crates/yserver/src/kms/vk/shaders/masked_blit.frag.glsl`

The vertex stage places a quad over the dst rect in absolute dst-pixel coordinates (so `gl_FragCoord.xy` equals the dst drawable pixel). The fragment stage uses `texelFetch` (integer, no filtering) for exact texel selection, thresholds the mask, discards OOB-src, and writes src verbatim.

- [ ] **Step 1: Write the vertex shader**

`masked_blit.vert.glsl`:

```glsl
#version 450

// One quad over the dst rect, mapped to absolute dst-pixel coords so
// gl_FragCoord in the fragment stage equals the dst drawable pixel.
layout(push_constant) uniform PushConsts {
    vec2  dst_origin;   // 0   dst rect offset (pixels)
    vec2  dst_size;     // 8   dst rect extent (pixels)
    vec2  viewport;     // 16  dst image extent (pixels) for NDC
    ivec2 copy_offset;  // 24  src_texel = dst_pixel + copy_offset
    ivec2 clip_offset;  // 32  mask_texel = dst_pixel - clip_offset
    ivec2 src_extent;   // 40  src image extent (texels) for OOB discard
    ivec2 mask_extent;  // 48  mask image extent (texels) for OOB discard
} pc;                   // size = 56

void main() {
    vec2 quad = vec2(float(gl_VertexIndex & 1), float((gl_VertexIndex >> 1) & 1));
    vec2 dst_pixel = pc.dst_origin + quad * pc.dst_size;
    vec2 ndc = dst_pixel / pc.viewport * 2.0 - 1.0;
    gl_Position = vec4(ndc, 0.0, 1.0);
}
```

- [ ] **Step 2: Write the fragment shader**

`masked_blit.frag.glsl`:

```glsl
#version 450

layout(push_constant) uniform PushConsts {
    vec2  dst_origin;
    vec2  dst_size;
    vec2  viewport;
    ivec2 copy_offset;
    ivec2 clip_offset;
    ivec2 src_extent;
    ivec2 mask_extent;
} pc;

// Binding 0: src image (IDENTITY view). Binding 1: mask image (IDENTITY R8 view).
// texelFetch ignores the sampler, but the descriptor type is COMBINED_IMAGE_SAMPLER.
layout(set = 0, binding = 0) uniform sampler2D src_tex;
layout(set = 0, binding = 1) uniform sampler2D mask_tex;

layout(location = 0) out vec4 out_color;

void main() {
    // gl_FragCoord.xy = dst_pixel + 0.5 (Vulkan, origin top-left). Truncate to the pixel.
    ivec2 dst_pixel = ivec2(gl_FragCoord.xy);

    // Clip mask threshold (1-bit clip, NOT alpha coverage). depth-1 "set" is
    // stored as the R8 byte 0x01, read here as 0x01/255 > 0.0. OOB mask = clipped.
    ivec2 mask_texel = dst_pixel - pc.clip_offset;
    if (mask_texel.x < 0 || mask_texel.y < 0
        || mask_texel.x >= pc.mask_extent.x || mask_texel.y >= pc.mask_extent.y) {
        discard;
    }
    if (texelFetch(mask_tex, mask_texel, 0).r <= 0.0) {
        discard;
    }

    // Project to src; discard OOB-src (never a sampled-zero write).
    ivec2 src_texel = dst_pixel + pc.copy_offset;
    if (src_texel.x < 0 || src_texel.y < 0
        || src_texel.x >= pc.src_extent.x || src_texel.y >= pc.src_extent.y) {
        discard;
    }

    // GXcopy = raw copy. Blend is disabled in the pipeline; write the src texel
    // verbatim. For depth-24 BGRA the X/alpha byte is copied exactly (identity
    // view, no force-opaque). 8-bit UNORM round-trips exactly (gated in Task 10).
    out_color = texelFetch(src_tex, src_texel, 0);
}
```

- [ ] **Step 3: Verify the shaders compile**

Run: `cargo build -p yserver`
Expected: PASS. `build.rs` discovers the two new `*.glsl` files and runs `glslc -fshader-stage={vert,frag} --target-env=vulkan1.3 -O`. A GLSL error fails the build with a `glslc failed` panic.

- [ ] **Step 4: Commit**

```bash
git add crates/yserver/src/kms/vk/shaders/masked_blit.vert.glsl crates/yserver/src/kms/vk/shaders/masked_blit.frag.glsl
git commit -m "feat(v2/clip): masked_blit GLSL shaders (texelFetch, threshold mask)"
```

---

### Task 2: `MaskedBlitPushConsts` + `MaskedBlitPipeline` cache

**Files:**
- Create: `crates/yserver/src/kms/vk/masked_blit_pipeline.rs`
- Modify: `crates/yserver/src/kms/vk/mod.rs` (add `pub(crate) mod masked_blit_pipeline;`)

This is a trimmed clone of `render_pipeline.rs`: 2 descriptor bindings (src=0, mask=1), one push-constant range, blend DISABLED, per-`vk::Format` pipeline build keyed in a `HashMap`. NO sampler swizzle reliance — we bind IDENTITY views at descriptor time (Task 6 / Task 13).

- [ ] **Step 1: Write the push-constant struct (matches the GLSL layout exactly)**

In `masked_blit_pipeline.rs`:

```rust
use std::collections::HashMap;
use std::sync::Arc;

use ash::vk;

use crate::kms::vk::device::VkContext;

/// Push constants for the masked_blit draw. `#[repr(C)]`; the field
/// order and offsets MUST match `masked_blit.{vert,frag}.glsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct MaskedBlitPushConsts {
    pub(crate) dst_origin: [f32; 2],  // 0
    pub(crate) dst_size: [f32; 2],    // 8
    pub(crate) viewport: [f32; 2],    // 16
    pub(crate) copy_offset: [i32; 2], // 24  src_texel = dst_pixel + copy_offset
    pub(crate) clip_offset: [i32; 2], // 32  mask_texel = dst_pixel - clip_offset
    pub(crate) src_extent: [i32; 2],  // 40
    pub(crate) mask_extent: [i32; 2], // 48
                                      // size = 56
}

const _: () = assert!(std::mem::size_of::<MaskedBlitPushConsts>() == 56);

impl MaskedBlitPushConsts {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref::<Self>(self).cast::<u8>(),
                std::mem::size_of::<Self>(),
            )
        }
    }
}

const VERTEX_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/masked_blit.vert.spv"));
const FRAGMENT_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/masked_blit.frag.spv"));
```

- [ ] **Step 2: Write the cache struct + constructor (layout, descriptor-set layout, sampler)**

Append to `masked_blit_pipeline.rs`:

```rust
pub(crate) struct MaskedBlitPipeline {
    vk: Arc<VkContext>,
    pub(crate) pipeline_layout: vk::PipelineLayout,
    pub(crate) descriptor_set_layout: vk::DescriptorSetLayout,
    sampler: vk::Sampler,
    pipelines: HashMap<vk::Format, vk::Pipeline>,
}

impl MaskedBlitPipeline {
    pub(crate) fn new(vk: Arc<VkContext>) -> Result<Self, vk::Result> {
        let device = &vk.device;
        // NEAREST sampler: texelFetch ignores it, but COMBINED_IMAGE_SAMPLER
        // descriptors require a sampler object.
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };

        let dsl_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let dsl_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&dsl_bindings);
        let descriptor_set_layout = match unsafe {
            device.create_descriptor_set_layout(&dsl_info, None)
        } {
            Ok(d) => d,
            Err(e) => {
                unsafe { device.destroy_sampler(sampler, None) };
                return Err(e);
            }
        };

        let set_layouts = [descriptor_set_layout];
        let push_const_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<MaskedBlitPushConsts>() as u32)];
        let pl_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&push_const_ranges);
        let pipeline_layout = match unsafe { device.create_pipeline_layout(&pl_info, None) } {
            Ok(p) => p,
            Err(e) => {
                unsafe {
                    device.destroy_descriptor_set_layout(descriptor_set_layout, None);
                    device.destroy_sampler(sampler, None);
                }
                return Err(e);
            }
        };

        Ok(Self {
            vk,
            pipeline_layout,
            descriptor_set_layout,
            sampler,
            pipelines: HashMap::new(),
        })
    }
```

- [ ] **Step 3: Write the per-format pipeline builder (blend DISABLED)**

Append (still inside `impl MaskedBlitPipeline`):

```rust
    /// Get-or-build the pipeline for a dst color format. Blend is DISABLED
    /// (GXcopy raw copy); the frag writes the src texel verbatim.
    pub(crate) fn pipeline_for(
        &mut self,
        color_format: vk::Format,
    ) -> Result<vk::Pipeline, vk::Result> {
        if let Some(p) = self.pipelines.get(&color_format) {
            return Ok(*p);
        }
        let p = Self::build_pipeline(&self.vk, self.pipeline_layout, color_format)?;
        self.pipelines.insert(color_format, p);
        Ok(p)
    }

    fn build_pipeline(
        vk: &VkContext,
        pipeline_layout: vk::PipelineLayout,
        color_format: vk::Format,
    ) -> Result<vk::Pipeline, vk::Result> {
        let device = &vk.device;
        let vert_module = create_shader_module(device, VERTEX_SPV)?;
        let frag_module = match create_shader_module(device, FRAGMENT_SPV) {
            Ok(m) => m,
            Err(e) => {
                unsafe { device.destroy_shader_module(vert_module, None) };
                return Err(e);
            }
        };
        let entry = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_module)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag_module)
                .name(entry),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_STRIP);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // Blend DISABLED — raw copy. Write all channels (incl. the depth-24 X byte).
        let color_blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(false)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(&color_blend_attachments);
        let dynamic_state_array = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state = vk::PipelineDynamicStateCreateInfo::default()
            .dynamic_states(&dynamic_state_array);
        let color_formats = [color_format];
        let mut rendering_info =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats);
        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic_state)
            .layout(pipeline_layout)
            .push_next(&mut rendering_info);
        let pipeline = match unsafe {
            device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
        } {
            Ok(ps) => ps[0],
            Err((_, e)) => {
                unsafe {
                    device.destroy_shader_module(vert_module, None);
                    device.destroy_shader_module(frag_module, None);
                }
                return Err(e);
            }
        };
        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }
        Ok(pipeline)
    }
}
```

- [ ] **Step 4: Write the shader-module helper, descriptor write helper, and Drop**

Append (free functions / Drop impl):

```rust
fn create_shader_module(
    device: &ash::Device,
    spv_bytes: &[u8],
) -> Result<vk::ShaderModule, vk::Result> {
    debug_assert!(spv_bytes.len().is_multiple_of(4));
    let mut code: Vec<u32> = Vec::with_capacity(spv_bytes.len() / 4);
    for chunk in spv_bytes.chunks_exact(4) {
        code.push(u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    let info = vk::ShaderModuleCreateInfo::default().code(&code);
    unsafe { device.create_shader_module(&info, None) }
}

impl MaskedBlitPipeline {
    /// Write src + mask IDENTITY views into a descriptor set acquired from the
    /// ring. Both views MUST be the IDENTITY `image_view` (NOT `sample_view`):
    /// the R8 mask bit lives in `.r` and depth-24 bytes must be raw.
    pub(crate) fn write_views(
        &self,
        set: vk::DescriptorSet,
        src_view: vk::ImageView,
        mask_view: vk::ImageView,
    ) {
        let src_info = [vk::DescriptorImageInfo::default()
            .image_view(src_view)
            .sampler(self.sampler)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let mask_info = [vk::DescriptorImageInfo::default()
            .image_view(mask_view)
            .sampler(self.sampler)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&src_info),
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&mask_info),
        ];
        unsafe { self.vk.device.update_descriptor_sets(&writes, &[]) };
    }
}

impl Drop for MaskedBlitPipeline {
    fn drop(&mut self) {
        let device = &self.vk.device;
        unsafe {
            for (_, p) in self.pipelines.drain() {
                device.destroy_pipeline(p, None);
            }
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            device.destroy_sampler(self.sampler, None);
        }
    }
}
```

- [ ] **Step 5: Register the module**

In `crates/yserver/src/kms/vk/mod.rs`, add alongside the other `mod` lines (verify the exact existing style with `grep -n "render_pipeline" crates/yserver/src/kms/vk/mod.rs`):

```rust
pub(crate) mod masked_blit_pipeline;
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo build -p yserver`
Expected: PASS (the `const _: () = assert!` enforces the 56-byte push-const size at compile time).

- [ ] **Step 7: Commit**

```bash
git add crates/yserver/src/kms/vk/masked_blit_pipeline.rs crates/yserver/src/kms/vk/mod.rs
git commit -m "feat(v2/clip): dedicated masked_blit pipeline (no blend, identity views)"
```

---

### Task 3: Hold the `MaskedBlitPipeline` in the engine + lazy build

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs` (field on `RenderEngineInner`; build in `ensure_render_assets`)

- [ ] **Step 1: Add the field**

Near `render_pipelines: Option<RenderPipelineCache>,` (engine.rs:619), add:

```rust
    /// Dedicated masked_blit pipeline for GPU-side clip CopyArea (depth-1
    /// mask sampled, threshold, raw copy). Built lazily alongside
    /// render_pipelines in `ensure_render_assets`.
    masked_blit: Option<crate::kms::vk::masked_blit_pipeline::MaskedBlitPipeline>,
```

Initialize it `masked_blit: None,` in the `RenderEngineInner { ... }` constructor near `render_pipelines: None,` (engine.rs:1151).

- [ ] **Step 2: Build it lazily in `ensure_render_assets`**

In `ensure_render_assets` (engine.rs:~2507, where `render_pipelines` is built), after the `render_pipelines` block, add:

```rust
        if inner.masked_blit.is_none() {
            let mb = crate::kms::vk::masked_blit_pipeline::MaskedBlitPipeline::new(Arc::clone(
                &inner.vk,
            ))
            .map_err(|e| {
                log::error!("v2 ensure_render_assets: MaskedBlitPipeline::new failed: {e:?}");
                RenderError::Vk(e)
            })?;
            inner.masked_blit = Some(mb);
        }
```

(Match the exact error-construction style of the adjacent `render_pipelines` block — use the same `RenderError` variant it uses; verify with `grep -n "RenderPipelineCache::new" crates/yserver/src/kms/v2/engine.rs`.)

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p yserver`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/yserver/src/kms/v2/engine.rs
git commit -m "feat(v2/clip): hold masked_blit pipeline in engine, build in ensure_render_assets"
```

---

### Task 4: `SampledScratchImage` for self-overlap (SAMPLED + view)

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs` (new struct + allocator near `ScratchImage`/`allocate_scratch_image`)

The existing `ScratchImage` is `TRANSFER_SRC | TRANSFER_DST` only — it cannot be sampled. The masked-blit self-overlap path copies src→scratch (transfer) then **samples** scratch, so scratch needs `TRANSFER_DST | SAMPLED` usage and an IDENTITY view.

- [ ] **Step 1: Write a failing crate-local test**

In `crates/yserver/src/kms/v2/engine.rs` test module (find it with `grep -n "mod tests" crates/yserver/src/kms/v2/engine.rs`), add:

```rust
    #[test]
    #[ignore = "needs live Vulkan ICD"]
    fn sampled_scratch_image_has_view_and_sampled_usage() {
        let Ok(vk) = crate::kms::vk::device::VkContext::new() else {
            eprintln!("skipping: no Vk");
            return;
        };
        let vk = std::sync::Arc::new(vk);
        let s = super::allocate_sampled_scratch_image(&vk, 16, 8, ash::vk::Format::B8G8R8A8_UNORM)
            .expect("allocate sampled scratch");
        assert_ne!(s.view, ash::vk::ImageView::null(), "must expose an IDENTITY view");
        assert!(s.size_bytes > 0);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p yserver --lib sampled_scratch_image_has_view -- --ignored`
Expected: FAIL — `allocate_sampled_scratch_image` / field `view` not found (does not compile, which counts as the failing state).

- [ ] **Step 3: Implement the struct + allocator**

Add near `ScratchImage` (engine.rs:266) and `allocate_scratch_image` (engine.rs:8903):

```rust
/// Scratch image for the masked_blit self-overlap path. Unlike
/// `ScratchImage` (transfer-only), this is `TRANSFER_DST | SAMPLED` with an
/// IDENTITY view so the masked-blit draw can sample it after the src→scratch
/// transfer breaks the read-after-write.
pub(crate) struct SampledScratchImage {
    vk: Arc<VkContext>,
    pub(crate) image: vk::Image,
    pub(crate) view: vk::ImageView,
    memory: vk::DeviceMemory,
    pub(crate) size_bytes: u64,
}

impl std::fmt::Debug for SampledScratchImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SampledScratchImage")
            .field("size_bytes", &self.size_bytes)
            .finish_non_exhaustive()
    }
}

impl Drop for SampledScratchImage {
    fn drop(&mut self) {
        unsafe {
            self.vk.device.destroy_image_view(self.view, None);
            self.vk.device.destroy_image(self.image, None);
            self.vk.device.free_memory(self.memory, None);
        }
    }
}

fn allocate_sampled_scratch_image(
    vk: &Arc<VkContext>,
    width: u32,
    height: u32,
    format: vk::Format,
) -> Result<SampledScratchImage, RenderError> {
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D { width, height, depth: 1 })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = unsafe { vk.device.create_image(&info, None)? };
    let mem_reqs = unsafe { vk.device.get_image_memory_requirements(image) };
    let mem_props = unsafe {
        vk.instance.get_physical_device_memory_properties(vk.physical_device)
    };
    let Some(mt) = (0..mem_props.memory_type_count).find(|&i| {
        mem_reqs.memory_type_bits & (1 << i) != 0
            && mem_props.memory_types[i as usize]
                .property_flags
                .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
    }) else {
        unsafe { vk.device.destroy_image(image, None) };
        return Err(RenderError::Vk(vk::Result::ERROR_FEATURE_NOT_PRESENT));
    };
    let alloc_info = vk::MemoryAllocateInfo::default()
        .allocation_size(mem_reqs.size)
        .memory_type_index(mt);
    let memory = match unsafe { vk.device.allocate_memory(&alloc_info, None) } {
        Ok(m) => m,
        Err(e) => {
            unsafe { vk.device.destroy_image(image, None) };
            return Err(RenderError::Vk(e));
        }
    };
    if let Err(e) = unsafe { vk.device.bind_image_memory(image, memory, 0) } {
        unsafe {
            vk.device.free_memory(memory, None);
            vk.device.destroy_image(image, None);
        }
        return Err(RenderError::Vk(e));
    }
    // IDENTITY view (no .components()) — matches Storage::image_view semantics.
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1),
        );
    let view = match unsafe { vk.device.create_image_view(&view_info, None) } {
        Ok(v) => v,
        Err(e) => {
            unsafe {
                vk.device.free_memory(memory, None);
                vk.device.destroy_image(image, None);
            }
            return Err(RenderError::Vk(e));
        }
    };
    Ok(SampledScratchImage {
        vk: Arc::clone(vk),
        image,
        view,
        memory,
        size_bytes: mem_reqs.size,
    })
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p yserver --lib sampled_scratch_image_has_view -- --ignored`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/v2/engine.rs
git commit -m "feat(v2/clip): SampledScratchImage (TRANSFER_DST|SAMPLED + identity view)"
```

---

### Task 5: `RecordedMaskedCopyArea` struct + `RecordedOp::MaskedCopyArea` variant

**Files:**
- Modify: `crates/yserver/src/kms/v2/frame_builder.rs` (struct + enum variant; `dst_id()`; any exhaustive matches)

The recorded op carries everything `emit` needs: src/dst identities, the mask **image+view** (a snapshot or a test drawable — the frame builder is agnostic), clamped offsets/rects, clip origin, surviving scissors, recorded old layouts, and the optional self-overlap scratch.

- [ ] **Step 0: Define the `SnapshotId` newtype now (codex round-6 finding 1)**

Both `RecordedClipSnapshotRefresh` and `MaskedCopyMask` (Task 7) reference `super::engine::SnapshotId`, but the `ClipSnapshot` registry that uses it does not arrive until Task 11. To keep Phase-1 commits compiling, define the opaque id in `engine.rs` NOW (Task 11 adds only the `ClipSnapshot` struct + the `HashMap<SnapshotId, ClipSnapshot>` registry):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SnapshotId(pub(crate) u64);
```

- [ ] **Step 1: Define the struct**

Near `RecordedCopyArea` (frame_builder.rs:582), add:

```rust
/// Standalone snapshot-refresh op (codex round-4 finding 5 + unification): a
/// `cmd_copy_image` live-clip-pixmap → GC-owned snapshot. This is the SINGLE
/// refresh mechanism — it is appended (by `refresh_clip_snapshot`, Task 11)
/// both at clip-mask install (pixmap guaranteed live → closes retain-after-free)
/// and before any in-scope masked copy whose snapshot version is stale while the
/// pixmap is still alive (closes same-frame mask-write ordering). The masked-blit
/// op itself NEVER refreshes — it only samples the (already-fresh) snapshot.
///
/// Emit (Task 13): live→TRANSFER_SRC, snapshot→TRANSFER_DST, cmd_copy_image,
/// snapshot→SHADER_READ, live→SHADER_READ. Both end at SHADER_READ (N1). The
/// `ALL_COMMANDS` source stage on the live read orders it after any same-frame
/// write to the live mask.
pub(crate) struct RecordedClipSnapshotRefresh {
    pub(crate) snapshot_id: super::engine::SnapshotId,
    pub(crate) snapshot_image: vk::Image,
    pub(crate) snapshot_old_layout: vk::ImageLayout,
    /// Live clip-pixmap drawable read by the copy. A frame participant
    /// (first-touch/ticket/old-layout, recorded in `refresh_clip_snapshot`);
    /// ends at SHADER_READ_ONLY_OPTIMAL.
    pub(crate) live_mask_id: DrawableId,
    pub(crate) live_mask_image: vk::Image,
    pub(crate) live_mask_old_layout: vk::ImageLayout,
    /// Copy region: live mask → snapshot, both at the same texel coords.
    pub(crate) copy_extent: vk::Extent2D,
}

pub(crate) struct RecordedMaskedCopyArea {
    pub(crate) dst_id: DrawableId,
    pub(crate) src_id: DrawableId,
    /// dst color format (selects the masked_blit pipeline).
    pub(crate) dst_format: vk::Format,
    pub(crate) dst_image: vk::Image,
    pub(crate) dst_view: vk::ImageView,
    pub(crate) dst_extent: vk::Extent2D,
    // --- LIVE source drawable (codex round-4 findings 1+2): used by the
    // self-overlap src→scratch COPY and by the src→SHADER_READ barrier on the
    // non-overlap path. NEVER the scratch. ---
    pub(crate) src_image: vk::Image,
    pub(crate) src_old_layout: vk::ImageLayout,
    /// The clamped LIVE source rect offset (= `src_rect.offset`). Used ONLY by
    /// the self-overlap `src → scratch` copy as `src_offset` (codex round-5
    /// finding 1: `copy_offset` is the SAMPLE-space offset and is rewritten to
    /// `−dst_rect.offset` on self-overlap, so it must NOT be reused for the copy).
    pub(crate) live_src_offset: [i32; 2],
    // --- SAMPLED source (what the frag's texelFetch reads): the src IDENTITY
    // view normally, the scratch IDENTITY view on self-overlap. ---
    pub(crate) sample_view: vk::ImageView,
    pub(crate) sample_extent: vk::Extent2D,
    /// Mask image + IDENTITY R8 view to sample. The GC-owned snapshot in
    /// production; a plain depth-1 drawable in the exactness tests.
    pub(crate) mask_image: vk::Image,
    pub(crate) mask_view: vk::ImageView,
    pub(crate) mask_extent: vk::Extent2D,
    /// `mask_texel = dst_pixel - clip_origin`.
    pub(crate) clip_origin: [i32; 2],
    /// `src_texel = dst_pixel + copy_offset`. Non-overlap: src_rect.offset −
    /// dst_rect.offset. Self-overlap (sampling the scratch which holds the
    /// region at (0,0)): −dst_rect.offset.
    pub(crate) copy_offset: [i32; 2],
    /// The clamped dst rect the draw covers (the quad is placed over it).
    pub(crate) dst_rect: vk::Rect2D,
    /// Surviving GC-rect/child/window scissors (>=1). One scissored draw each.
    pub(crate) scissors: Vec<vk::Rect2D>,
    pub(crate) dst_old_layout: vk::ImageLayout,
    pub(crate) mask_old_layout: vk::ImageLayout,
    /// `Some` when `src_id == dst_id`. The LIVE src region (from `src_image`) is
    /// copied here, then `sample_view` points at this scratch's view.
    pub(crate) self_overlap_scratch: Option<super::engine::SampledScratchImage>,
}
```

> The masked op carries NO refresh sub-payload: snapshot freshness is guaranteed by a separate `RecordedClipSnapshotRefresh` op appended earlier in the same frame (Task 14). `mask_old_layout` is therefore the snapshot's layout as left by that refresh (SHADER_READ) or its prior terminal (SHADER_READ) on the cache-hit path.

- [ ] **Step 2: Add the enum variant**

In the `RecordedOp` enum (frame_builder.rs:756), add:

```rust
    MaskedCopyArea(Box<RecordedMaskedCopyArea>),
    ClipSnapshotRefresh(Box<RecordedClipSnapshotRefresh>),
```

`ClipSnapshotRefresh` has NO dst drawable (it writes the engine-owned snapshot, not a `DrawableId`). For `dst_id()` and any exhaustive match that demands a drawable, return the **live mask id** (it is the only drawable the op touches) or handle it as a no-dst arm consistent with how other non-drawable-dst ops are treated — check the `dst_id()` signature (`Option<DrawableId>` vs `DrawableId`) and follow it.

- [ ] **Step 3: Wire it into `dst_id()` and any exhaustive matches**

Find `RecordedOp`'s `dst_id()` (and any other exhaustive `match` over `RecordedOp`, e.g. a `Debug`/size helper or the close-time emit dispatch) with:

```bash
grep -n "RecordedOp::CopyArea" crates/yserver/src/kms/v2/frame_builder.rs crates/yserver/src/kms/v2/engine.rs
```

Add `MaskedCopyArea` AND `ClipSnapshotRefresh` arms everywhere `CopyArea` appears. For `dst_id()`:

```rust
            RecordedOp::MaskedCopyArea(m) => m.dst_id,
            RecordedOp::ClipSnapshotRefresh(r) => r.live_mask_id, // only drawable touched
```

For the close-time emit dispatch in `engine.rs` (the `match op { ... RecordedOp::CopyArea(ca) => emit_recorded_copy_area_into_cb(...) }`), add (both functions land in Task 6):

```rust
            super::frame_builder::RecordedOp::MaskedCopyArea(m) => {
                emit_recorded_masked_copyarea_into_cb(inner, cb, generation, m)?;
            }
            super::frame_builder::RecordedOp::ClipSnapshotRefresh(r) => {
                emit_recorded_clip_snapshot_refresh_into_cb(inner, cb, r)?;
            }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p yserver`
Expected: FAIL — the two emit functions not yet defined. This is expected; Task 6 defines them. (If you want a green checkpoint here, temporarily `todo!()` the arms, but DELETE the `todo!()` in Task 6 — do not commit a `todo!()`.)

- [ ] **Step 5: Commit (after Task 6 makes it build)**

Defer the commit to Task 6 Step 6 so the tree never holds a non-compiling commit.

---

### Task 6: `emit_recorded_masked_copyarea_into_cb` — the draw

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs` (new emit function near `emit_recorded_copy_area_into_cb`, engine.rs:7657)

This task defines TWO emit functions: `emit_recorded_masked_copyarea_into_cb` (the draw) and `emit_recorded_clip_snapshot_refresh_into_cb` (the standalone snapshot copy). Both mirror `barrier_to_layout` + the `record_render_composite_{open,draws,close}` template (ops/render.rs).

**`emit_recorded_masked_copyarea_into_cb` sequence** (NO refresh here — that is the separate op):
1. (self-overlap only) barrier live src → TRANSFER_SRC, scratch → TRANSFER_DST; `cmd_copy_image` live-src→scratch@0,0; barrier scratch → SHADER_READ_ONLY. The draw then samples `sample_view` (= scratch view); `copy_offset` is `−dst_rect.offset`. On self-overlap, dst (== src) is left in TRANSFER_SRC for the next step.
2. barrier src → SHADER_READ_ONLY (non-overlap only — self-overlap samples the scratch, not src), mask → SHADER_READ_ONLY (always; the snapshot is already SHADER_READ after a same-frame refresh, but the barrier still provides the read-after-write dependency), dst → COLOR_ATTACHMENT (old layout is `TRANSFER_SRC` on self-overlap, else `dst_old_layout`).
3. `begin_rendering` on dst (LOAD/STORE), set viewport, bind masked_blit pipeline (for `dst_format`), bind descriptor set (`sample_view`, `mask_view`), then per scissor: set scissor, push consts, `cmd_draw(4,1,0,0)`.
4. `end_rendering`; barrier dst → SHADER_READ_ONLY.

**`emit_recorded_clip_snapshot_refresh_into_cb` sequence:** live→TRANSFER_SRC, snapshot→TRANSFER_DST, `cmd_copy_image` live→snapshot, snapshot→SHADER_READ, live→SHADER_READ (both terminal SHADER_READ, N1).

- [ ] **Step 1: Write the function**

Add near engine.rs:7657. Note the descriptor set is acquired from `inner.descriptor_pool_ring` at the op's `generation`; obtain the generation the same way `emit_recorded_copy_area_into_cb`'s caller does (inspect the surrounding emit loop — there is a per-op `generation` in scope; thread it in as a parameter mirroring how composite emit receives it). The signature below takes `generation` explicitly; adjust the Task-5 dispatch arm to pass it (`grep -n "generation" crates/yserver/src/kms/v2/engine.rs` around the emit loop to find the in-scope binding).

```rust
fn emit_recorded_masked_copyarea_into_cb(
    inner: &mut RenderEngineInner,
    cb: vk::CommandBuffer,
    generation: u64,
    m: &super::frame_builder::RecordedMaskedCopyArea,
) -> Result<(), RenderError> {
    let device = inner.vk.device.clone();

    // NO refresh here: the snapshot is brought up to date by a separate
    // RecordedOp::ClipSnapshotRefresh emitted earlier this frame (Task 14). The
    // masked op only SAMPLES the snapshot, whose `mask_old_layout` is SHADER_READ.

    // (2) Self-overlap: copy the LIVE src region → scratch@(0,0), then sample
    // scratch. `dst_is_transfer_src` tracks that dst (== src) is left in
    // TRANSFER_SRC by this copy, so the (3) dst→COLOR barrier uses the right
    // old layout (codex round-4 finding 1).
    let dst_is_transfer_src = m.self_overlap_scratch.is_some();
    if let Some(scratch) = m.self_overlap_scratch.as_ref() {
        barrier_to_layout(
            &device, cb, m.src_image, m.src_old_layout,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::PipelineStageFlags2::ALL_COMMANDS,
            vk::AccessFlags2::SHADER_SAMPLED_READ | vk::AccessFlags2::TRANSFER_WRITE
                | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_READ,
        );
        barrier_to_layout(
            &device, cb, scratch.image, vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::PipelineStageFlags2::TOP_OF_PIPE, vk::AccessFlags2::empty(),
            vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_WRITE,
        );
        // src region = the clamped LIVE src rect (live_src_offset); scratch holds
        // it at (0,0). NOTE: do NOT use copy_offset here — it is the rewritten
        // sample-space offset (−dst_rect.offset) on this path (finding 1).
        let region = [vk::ImageCopy::default()
            .src_subresource(color_layers())
            .src_offset(vk::Offset3D {
                x: m.live_src_offset[0],
                y: m.live_src_offset[1],
                z: 0,
            })
            .dst_subresource(color_layers())
            .dst_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .extent(vk::Extent3D {
                width: m.dst_rect.extent.width,
                height: m.dst_rect.extent.height,
                depth: 1,
            })];
        unsafe {
            device.cmd_copy_image(
                cb, m.src_image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                scratch.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &region,
            );
        }
        barrier_to_layout(
            &device, cb, scratch.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_WRITE,
            vk::PipelineStageFlags2::FRAGMENT_SHADER, vk::AccessFlags2::SHADER_SAMPLED_READ,
        );
        // NOTE: the COPY reads `m.src_image` (the LIVE drawable, == dst here).
        // The DRAW samples `m.sample_view` (= scratch.view, set in Task 7), and
        // `m.copy_offset` is rewritten so src_texel = dst_pixel - dst_rect.offset.
    } else {
        barrier_to_layout(
            &device, cb, m.src_image, m.src_old_layout,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags2::ALL_COMMANDS,
            vk::AccessFlags2::SHADER_SAMPLED_READ | vk::AccessFlags2::TRANSFER_WRITE
                | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            vk::PipelineStageFlags2::FRAGMENT_SHADER, vk::AccessFlags2::SHADER_SAMPLED_READ,
        );
    }

    // Mask → SHADER_READ_ONLY. `mask_old_layout` is SHADER_READ when the snapshot
    // was just refreshed this frame, but may be UNDEFINED/other for the Phase-1
    // plain-drawable test path — so always emit the transition. A no-op SHADER_READ
    // → SHADER_READ barrier still provides the execution/memory dependency that
    // orders this draw after a same-frame ClipSnapshotRefresh write to the snapshot.
    barrier_to_layout(
        &device, cb, m.mask_image, m.mask_old_layout,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::PipelineStageFlags2::ALL_COMMANDS,
        vk::AccessFlags2::SHADER_SAMPLED_READ | vk::AccessFlags2::TRANSFER_WRITE
            | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        vk::PipelineStageFlags2::FRAGMENT_SHADER, vk::AccessFlags2::SHADER_SAMPLED_READ,
    );

    // (3) dst → COLOR_ATTACHMENT. On self-overlap, dst (== src) was left in
    // TRANSFER_SRC by the (2) copy, so the old layout + producer stage/access
    // differ from the non-overlap case (codex round-4 finding 1).
    let (dst_old, dst_src_stage, dst_src_access) = if dst_is_transfer_src {
        (
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::PipelineStageFlags2::COPY,
            vk::AccessFlags2::TRANSFER_READ,
        )
    } else {
        (
            m.dst_old_layout,
            vk::PipelineStageFlags2::ALL_COMMANDS,
            vk::AccessFlags2::SHADER_SAMPLED_READ
                | vk::AccessFlags2::TRANSFER_WRITE
                | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        )
    };
    barrier_to_layout(
        &device, cb, m.dst_image, dst_old,
        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        dst_src_stage, dst_src_access,
        vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
        vk::AccessFlags2::COLOR_ATTACHMENT_WRITE | vk::AccessFlags2::COLOR_ATTACHMENT_READ,
    );

    // (4) pipeline + descriptor set.
    let mb = inner
        .masked_blit
        .as_mut()
        .ok_or(RenderError::Vk(vk::Result::ERROR_INITIALIZATION_FAILED))?;
    let pipeline = mb.pipeline_for(m.dst_format).map_err(RenderError::Vk)?;
    let pipeline_layout = mb.pipeline_layout;
    let dsl = mb.descriptor_set_layout;
    let set = inner
        .descriptor_pool_ring
        .acquire_set(dsl, generation)
        .map_err(RenderError::Vk)?;
    inner
        .masked_blit
        .as_ref()
        .expect("masked_blit present")
        // Bind the SAMPLED view (src identity view, or scratch view on
        // self-overlap) — NOT the live src image (codex round-4 finding 2).
        .write_views(set, m.sample_view, m.mask_view);

    let render_area = vk::Rect2D {
        offset: vk::Offset2D::default(),
        extent: m.dst_extent,
    };
    let color_attachment = [vk::RenderingAttachmentInfo::default()
        .image_view(m.dst_view)
        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .load_op(vk::AttachmentLoadOp::LOAD)
        .store_op(vk::AttachmentStoreOp::STORE)];
    let rendering_info = vk::RenderingInfo::default()
        .render_area(render_area)
        .layer_count(1)
        .color_attachments(&color_attachment);
    let viewport = [vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: m.dst_extent.width as f32,
        height: m.dst_extent.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    }];
    unsafe {
        device.cmd_begin_rendering(cb, &rendering_info);
        device.cmd_set_viewport(cb, 0, &viewport);
        device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
        device.cmd_bind_descriptor_sets(
            cb, vk::PipelineBindPoint::GRAPHICS, pipeline_layout, 0, &[set], &[],
        );
        let pc = crate::kms::vk::masked_blit_pipeline::MaskedBlitPushConsts {
            dst_origin: [m.dst_rect.offset.x as f32, m.dst_rect.offset.y as f32],
            dst_size: [m.dst_rect.extent.width as f32, m.dst_rect.extent.height as f32],
            viewport: [m.dst_extent.width as f32, m.dst_extent.height as f32],
            copy_offset: m.copy_offset,
            clip_offset: m.clip_origin, // frag: mask_texel = dst_pixel - clip_offset
            // OOB check is against the SAMPLED image (src, or scratch on
            // self-overlap), so push sample_extent (codex round-4 finding 2).
            src_extent: [m.sample_extent.width as i32, m.sample_extent.height as i32],
            mask_extent: [m.mask_extent.width as i32, m.mask_extent.height as i32],
        };
        for s in &m.scissors {
            let sc = [*s];
            device.cmd_set_scissor(cb, 0, &sc);
            device.cmd_push_constants(
                cb, pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0, pc.as_bytes(),
            );
            device.cmd_draw(cb, 4, 1, 0, 0);
        }
        device.cmd_end_rendering(cb);
    }

    // (5) dst → SHADER_READ_ONLY (N1 terminal layout).
    barrier_to_layout(
        &device, cb, m.dst_image, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        vk::PipelineStageFlags2::FRAGMENT_SHADER, vk::AccessFlags2::SHADER_SAMPLED_READ,
    );
    Ok(())
}

/// Standalone snapshot refresh: cmd_copy_image live clip pixmap → GC-owned
/// snapshot, leaving BOTH at SHADER_READ_ONLY_OPTIMAL (N1). The `ALL_COMMANDS`
/// source stage on the live read orders this after any same-frame write to the
/// live mask; the snapshot→SHADER_READ barrier orders a later masked-blit's
/// sample after this copy (the masked op records mask_old_layout = SHADER_READ).
fn emit_recorded_clip_snapshot_refresh_into_cb(
    inner: &mut RenderEngineInner,
    cb: vk::CommandBuffer,
    r: &super::frame_builder::RecordedClipSnapshotRefresh,
) -> Result<(), RenderError> {
    let device = inner.vk.device.clone();
    // live → TRANSFER_SRC.
    barrier_to_layout(
        &device, cb, r.live_mask_image, r.live_mask_old_layout,
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        vk::PipelineStageFlags2::ALL_COMMANDS,
        vk::AccessFlags2::SHADER_SAMPLED_READ | vk::AccessFlags2::TRANSFER_WRITE
            | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_READ,
    );
    // snapshot → TRANSFER_DST.
    barrier_to_layout(
        &device, cb, r.snapshot_image, r.snapshot_old_layout,
        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        vk::PipelineStageFlags2::ALL_COMMANDS,
        vk::AccessFlags2::SHADER_SAMPLED_READ,
        vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_WRITE,
    );
    let region = [vk::ImageCopy::default()
        .src_subresource(color_layers())
        .dst_subresource(color_layers())
        .extent(vk::Extent3D {
            width: r.copy_extent.width,
            height: r.copy_extent.height,
            depth: 1,
        })];
    unsafe {
        device.cmd_copy_image(
            cb, r.live_mask_image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            r.snapshot_image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &region,
        );
    }
    // snapshot → SHADER_READ (a later masked-blit samples it).
    barrier_to_layout(
        &device, cb, r.snapshot_image, vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_WRITE,
        vk::PipelineStageFlags2::FRAGMENT_SHADER, vk::AccessFlags2::SHADER_SAMPLED_READ,
    );
    // live → SHADER_READ (N1 terminal for the live mask drawable).
    barrier_to_layout(
        &device, cb, r.live_mask_image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_READ,
        vk::PipelineStageFlags2::FRAGMENT_SHADER, vk::AccessFlags2::SHADER_SAMPLED_READ,
    );
    Ok(())
}
```

> **Name consistency:** the clip-origin push-const field is named `clip_offset` everywhere — the GLSL (Task 1), the `MaskedBlitPushConsts` struct (Task 2), and this emit (`clip_offset: m.clip_origin`). The frag computes `mask_texel = dst_pixel - clip_offset`. After writing, re-grep to confirm one consistent name across all three.

- [ ] **Step 2: Delete any temporary `todo!()`**

Remove any `todo!()` placeholder added in Task 5 Step 4 (the emit arm now has a real body).

- [ ] **Step 3: Verify `color_layers()` / `color_subresource_range()` helper names**

Run: `grep -n "fn color_layers\|fn color_subresource_range" crates/yserver/src/kms/v2/engine.rs crates/yserver/src/kms/vk/ops/render.rs` — use whichever exact helper the copy emit uses for `ImageSubresourceLayers`. Adjust the `.src_subresource(...)` calls accordingly.

- [ ] **Step 4: Adopt the self-overlap scratch into the submitted-resource set (codex round-4 finding 4)**

The close path walks ops and drains `RecordedCopyArea.self_overlap_scratch` into the submitted op's retired-resource set so the scratch's `Drop` is deferred behind the frame's fence. `RecordedMaskedCopyArea.self_overlap_scratch` is a NEW field of a NEW type (`SampledScratchImage`) and is NOT collected by that walk — without adoption, the open frame drops the scratch image/view/memory while the GPU still reads it → use-after-free.

Find the scratch-collection site:

```bash
grep -n "self_overlap_scratch\|SubmittedOp\|fn close_open_frame\|\.scratch" crates/yserver/src/kms/v2/engine.rs
```

Wire the new op's scratch in:
- `SubmittedOp.scratch` is `Vec<ScratchImage>` (drained via `std::mem::take` at engine.rs:1787). Add a sibling `Vec<SampledScratchImage>` field (it has its own `Drop`), and in the op-walk add a `RecordedOp::MaskedCopyArea(m)` arm that `std::mem::take`s `m.self_overlap_scratch` into it. Drop the whole `SubmittedOp` when the fence retires (existing retirement path).
- Mirror the EXACT pattern the `RecordedOp::CopyArea` arm uses (single source of truth — never also leave a copy on the open frame).
- **Update resource accounting (codex round-5 finding 7 — the compiler does NOT catch this):** `active_resource_bytes` (engine.rs:2274) and the `op.scratch.iter()...size_bytes()` sums (engine.rs:2291, 2296) plus the `op.scratch.len()` at engine.rs:2060 must also account for the new sibling `Vec<SampledScratchImage>`. Add its `size_bytes`/`len` to the same sums, or the memory-pressure reclamation under-counts active GPU memory.
- Update every `SubmittedOp` literal/constructor to initialize the new field (the compiler DOES catch these).

Add a test asserting the scratch survives until fence retirement (or at minimum that a self-overlap masked copy followed by `get_image` returns correct bytes WITHOUT a validation/use-after-free error under `YSERVER_VK_VALIDATION=1`). The Task 10 `v2_masked_copyarea_self_overlap` test already exercises this path end-to-end; ensure it runs with validation enabled.

- [ ] **Step 5: Verify it compiles**

Run: `cargo build -p yserver`
Expected: PASS.

- [ ] **Step 6: Lint**

Run: `cargo clippy -p yserver`
Expected: no NEW warnings from these files (plain clippy, not pedantic).

- [ ] **Step 7: Commit (Tasks 5 + 6 together — first compiling state)**

```bash
cargo fmt
git add crates/yserver/src/kms/v2/frame_builder.rs crates/yserver/src/kms/v2/engine.rs
git commit -m "feat(v2/clip): RecordedMaskedCopyArea op + emit_recorded_masked_copyarea_into_cb"
```

---

### Task 7: `engine.masked_copy_area(...)` append entry point

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs` (new method on `RenderEngine`, near `copy_area`, engine.rs:3225)

Mirrors `copy_area`'s prelude (clamp/project, first-touch, ticket, layout overlay, damage, content_version) but appends a `RecordedOp::MaskedCopyArea`. Takes the mask image+view+layout+extent as parameters (snapshot OR test drawable). The backend (Phase 2) supplies them; for Phase 1 tests, the helper resolves a plain depth-1 drawable's `Storage::image_view`.

- [ ] **Step 1: Define the parameter bundle + method signature**

Add (above the method):

```rust
/// Mask source for a masked CopyArea: the GC-owned snapshot in production, a
/// plain depth-1 drawable in tests. `view` MUST be an IDENTITY R8 view.
///
/// ALL fields are defined here in Task 7 (incl. `snapshot_id`) so that no later
/// task has to widen the struct and update every call site — Phase-1 test
/// callers pass `snapshot_id: None` (codex round-4 finding 8). The masked op
/// only SAMPLES the mask; (re)population is the separate `refresh_clip_snapshot`
/// path (Task 11/14), so there is NO refresh field here.
pub(crate) struct MaskedCopyMask {
    pub(crate) image: vk::Image,
    pub(crate) view: vk::ImageView,
    /// MUST be SHADER_READ when this is a freshly-refreshed snapshot; the emit
    /// transitions to SHADER_READ regardless (handles the test plain-drawable).
    pub(crate) old_layout: vk::ImageLayout,
    pub(crate) extent: vk::Extent2D,
    pub(crate) clip_origin: [i32; 2],
    /// `Some(id)` when the mask is a GC-owned snapshot (Phase 2). `None` for the
    /// Phase-1 plain-drawable test path. Drives snapshot layout/ticket
    /// first-touch tracking on sample (Task 12). When `None`, the mask
    /// layout/ticket are NOT engine-managed.
    pub(crate) snapshot_id: Option<super::engine::SnapshotId>,
}
```

Method on `RenderEngine`:

```rust
#[allow(clippy::too_many_arguments)]
pub(crate) fn masked_copy_area(
    &mut self,
    store: &mut DrawableStore,
    platform: &mut PlatformBackend,
    src: DrawableId,
    dst: DrawableId,
    src_pos: vk::Offset2D,   // src_rect offset (already wire-resolved)
    dst_pos: vk::Offset2D,   // dst_rect offset (X11 negative offsets allowed)
    extent: vk::Extent2D,    // requested copy w/h
    mask: MaskedCopyMask,
    scissors: &[vk::Rect2D], // surviving GC-rect/child/window rects; >=1
) -> Result<(), RenderError> { /* steps below */ }
```

- [ ] **Step 2: Body — resolve images/formats, clamp/project (reuse copy_area arithmetic VERBATIM)**

Inside the method, gather src/dst storage (`store.get(...)`/`get_mut`) for `image`, `image_view` (IDENTITY), `extent`, `format`, `current_layout`. Then reproduce `copy_area`'s clamp/project (engine.rs:3263) exactly:

```rust
    // ENTRY PRELUDE — identical to copy_area (engine.rs:3247-3244, codex round-5
    // finding 4): renderer-failed guard, then flush_render_batch BEFORE any
    // open-frame mutation so this op is chronologically ordered after any
    // pre-existing pending render batch (a barrier inside our CB cannot order
    // against an un-flushed older batch).
    if platform.renderer_failed {
        return Err(RenderError::RendererFailed);
    }
    self.flush_render_batch(store, platform)?;
    let inner = self.inner.as_mut().ok_or(RenderError::NoVk)?; // match the real guard

    let src_d = store.get(src).ok_or(RenderError::UnknownDrawable(src))?;
    let dst_d = store.get(dst).ok_or(RenderError::UnknownDrawable(dst))?;
    let src_extent = src_d.storage.extent;
    let dst_extent = dst_d.storage.extent;
    let src_format = src_d.storage.format;
    let dst_format = dst_d.storage.format;
    let src_image = src_d.storage.image;
    let src_view = src_d.storage.image_view; // IDENTITY
    let dst_image = dst_d.storage.image;
    let dst_view = dst_d.storage.image_view; // IDENTITY

    // Clamp src_rect to src bounds; project dst exactly like copy_area.
    let src_rect = clamp_rect(
        vk::Rect2D { offset: src_pos, extent },
        src_extent,
    );
    let dst_pos_clamped = vk::Offset2D { x: dst_pos.x.max(0), y: dst_pos.y.max(0) };
    let copy_w = u32::try_from(
        (i32::from(dst_pos.x) + i32::try_from(src_rect.extent.width).unwrap_or(0))
            .min(i32::try_from(dst_extent.width).unwrap_or(i32::MAX))
            - dst_pos_clamped.x,
    ).unwrap_or(0).min(src_rect.extent.width);
    let copy_h = u32::try_from(
        (i32::from(dst_pos.y) + i32::try_from(src_rect.extent.height).unwrap_or(0))
            .min(i32::try_from(dst_extent.height).unwrap_or(i32::MAX))
            - dst_pos_clamped.y,
    ).unwrap_or(0).min(src_rect.extent.height);
    if copy_w == 0 || copy_h == 0 {
        return Ok(());
    }
    let dst_rect = vk::Rect2D {
        offset: dst_pos_clamped,
        extent: vk::Extent2D { width: copy_w, height: copy_h },
    };
    // src_texel = dst_pixel + copy_offset.
    let copy_offset = [
        src_rect.offset.x - dst_rect.offset.x,
        src_rect.offset.y - dst_rect.offset.y,
    ];
```

> Copy the exact `dst_pos`/`copy_w`/`copy_h` expressions from the live `copy_area` body (engine.rs:3263) rather than the simplified form above if they differ — they handle the X11 wire negative-offset edge cases. Re-read that block before finalizing.

- [ ] **Step 3: Body — self-overlap scratch (SAMPLED), N8 allocate-before-mutate**

```rust
    let self_overlap_scratch = if src == dst {
        Some(allocate_sampled_scratch_image(&inner.vk.clone(), copy_w, copy_h, src_format)?)
    } else {
        None
    };
    // The op keeps the LIVE src (`src_image`/`src_pre`) for the copy + barrier
    // (codex round-4 finding 2). The DRAW samples `sample_view`/`sample_extent`
    // with `eff_copy_offset`. On self-overlap, sampling the scratch (region at
    // (0,0)) means sample_view=scratch.view and src_texel = dst_pixel −
    // dst_rect.offset; otherwise it samples the src identity view directly.
    let (sample_view, sample_extent, eff_copy_offset) =
        if let Some(s) = self_overlap_scratch.as_ref() {
            (s.view, vk::Extent2D { width: copy_w, height: copy_h },
             [-dst_rect.offset.x, -dst_rect.offset.y])
        } else {
            (src_view, src_extent, copy_offset)
        };
```

- [ ] **Step 4: Body — prelude state (first-touch/ticket/layout/damage) for dst+src(+live mask on refresh)**

Mirror engine.rs:3336-3362 exactly. dst is a write; src is a read. The mask snapshot is NOT a drawable participant here (it's engine-managed; first-touch for rollback is recorded separately in Task 12 when `snapshot_id` is `Some`). The live clip pixmap is handled by the separate `refresh_clip_snapshot` op (Task 11), not here. Then bump `content_version`:

```rust
    let dst_pre = inner.current_layout_for_drawable(store, dst);
    let src_pre = if src == dst { dst_pre } else { inner.current_layout_for_drawable(store, src) };
    let prior_dst = store.get(dst).and_then(|d| d.last_render_ticket.clone());
    let prior_src = if src == dst { prior_dst.clone() } else { store.get(src).and_then(|d| d.last_render_ticket.clone()) };
    let frame_ticket = /* obtain as copy_area does */ ;
    {
        let open = inner.frame_builder.open.as_mut().expect("open");
        open.touched.first_touch(dst, prior_dst);
        open.layouts.first_touch_drawable(dst, dst_pre);
        if src != dst {
            open.touched.first_touch(src, prior_src);
            open.layouts.first_touch_drawable(src, src_pre);
        }
    }
    store.touch_render_fence(dst, frame_ticket.clone());
    if src != dst { store.touch_render_fence(src, frame_ticket.clone()); }
    store.damage(dst, dst_rect);
```

> Read the live `copy_area` body for the exact `frame_ticket` acquisition and the exact `current_layout_for_drawable`/open-frame accessors; reproduce them verbatim. Do NOT invent helper names.

- [ ] **Step 5: Body — build + append the op, set terminal layouts, bump content_version**

```rust
    let payload = Box::new(super::frame_builder::RecordedMaskedCopyArea {
        dst_id: dst, src_id: src, dst_format, dst_image, dst_view, dst_extent,
        // LIVE src drawable (copy + barrier); SAMPLED view/extent for the draw.
        src_image, src_old_layout: src_pre,
        live_src_offset: [src_rect.offset.x, src_rect.offset.y],
        sample_view, sample_extent,
        mask_image: mask.image, mask_view: mask.view, mask_extent: mask.extent,
        clip_origin: mask.clip_origin, copy_offset: eff_copy_offset, dst_rect,
        scissors: scissors.to_vec(),
        dst_old_layout: dst_pre, mask_old_layout: mask.old_layout,
        self_overlap_scratch,
    });
    let layout_updates: &[(DrawableId, vk::ImageLayout)] = if src == dst {
        &[(dst, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
    } else {
        &[
            (dst, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
            (src, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
        ]
    };
    {
        let open = inner.frame_builder.open.as_mut().expect("open");
        open.push_op_and_set_layouts(
            super::frame_builder::RecordedOp::MaskedCopyArea(payload),
            layout_updates,
        );
    }
    store.mark_contents_modified(dst);
    Ok(())
```

> The mask snapshot's layout/ticket are NOT drawable-keyed; they are managed by the snapshot carrier (Task 11) and committed by `refresh_clip_snapshot` / tracked for rollback in Task 12. The masked op does not touch the live clip pixmap at all.

- [ ] **Step 6: Verify it compiles**

Run: `cargo build -p yserver`
Expected: PASS. Resolve any mismatched helper/guard names by reading the live `copy_area` body and matching it exactly.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add crates/yserver/src/kms/v2/engine.rs
git commit -m "feat(v2/clip): engine.masked_copy_area append entry point"
```

---

### Task 8: Test helper — drive `masked_copy_area` from the backend test surface

**Files:**
- Modify: `crates/yserver/src/kms/v2/backend.rs` (a `#[cfg(test)]` or `for_tests`-style shim)

The lavapipe tests need to call `engine.masked_copy_area` with a depth-1 mask drawable. Add a test-only backend method that resolves drawable ids and the mask's IDENTITY view, then calls the engine and closes the frame.

- [ ] **Step 1: Add the shim**

```rust
    #[cfg(test)]
    pub(crate) fn masked_copy_area_for_tests(
        &mut self,
        src_xid: u32,
        dst_xid: u32,
        mask_xid: u32,
        clip_origin: (i32, i32),
        src_x: i16, src_y: i16, dst_x: i16, dst_y: i16, w: u16, h: u16,
        scissors: &[ash::vk::Rect2D],
    ) -> io::Result<()> {
        let src = self.store.lookup(src_xid).expect("src");
        let dst = self.store.lookup(dst_xid).expect("dst");
        let mask_id = self.store.lookup(mask_xid).expect("mask");
        let md = self.store.get(mask_id).expect("mask drawable");
        let mask = crate::kms::v2::engine::MaskedCopyMask {
            image: md.storage.image,
            view: md.storage.image_view, // IDENTITY R8
            old_layout: md.storage.current_layout,
            extent: md.storage.extent,
            clip_origin: [clip_origin.0, clip_origin.1],
            snapshot_id: None,       // plain-drawable test path (not a snapshot)
        };
        self.engine
            .masked_copy_area(
                &mut self.store, &mut self.platform, src, dst,
                ash::vk::Offset2D { x: src_x.into(), y: src_y.into() },
                ash::vk::Offset2D { x: dst_x.into(), y: dst_y.into() },
                ash::vk::Extent2D { width: w.into(), height: h.into() },
                mask, scissors,
            )
            .map_err(|e| io::Error::other(format!("masked_copy_area: {e:?}")))?;
        self.engine
            .flush_for_tests(&mut self.store, &mut self.platform) // match the real test-flush name
            .map_err(|e| io::Error::other(format!("flush: {e:?}")))?;
        Ok(())
    }
```

> Find the real "close/flush the open frame in tests" entry point with `grep -n "fn flush\|close_open_frame\|for_tests" crates/yserver/src/kms/v2/backend.rs crates/yserver/src/kms/v2/engine.rs` and use it (other lavapipe tests already force a flush before `get_image_pixels_for_tests` — copy their approach).

- [ ] **Step 2: Verify it compiles (test cfg)**

Run: `cargo test -p yserver --no-run`
Expected: PASS (compiles the test harness).

- [ ] **Step 3: Commit**

```bash
git add crates/yserver/src/kms/v2/backend.rs
git commit -m "test(v2/clip): masked_copy_area_for_tests shim"
```

---

### Task 9: ⛔ EXACTNESS GATE — `v2_masked_copyarea_matches_cmd_copy_image` (per format)

**Files:**
- Modify: `crates/yserver/tests/v2_acceptance.rs`

This is **THE** gate (spec § Exactness gate). For each in-scope dst/src format, the masked draw over the kept region must be **byte-identical** (raw storage bytes, incl. depth-24 X byte) to a plain `cmd_copy_image`, with a full-ones mask. Any format that fails is **excluded** from scope (Task 14 routing must skip it), not shipped wrong.

> **Readback must expose raw storage bytes (codex round-4 finding 7).** The comparison's validity depends on `get_image_pixels_for_tests` returning the raw storage bytes, NOT a re-packed/alpha-fixed `GetImage`. `pack_from_storage` is a straight `raw.to_vec()` memcpy for depth-24/32 (so the depth-24 X byte IS carried through) and a scanline-padded copy for depth-8 — both expose raw bytes. **Step 0:** confirm this before trusting the gate — read `pack_from_storage` (engine.rs:9236) and verify no depth-24 alpha synthesis on the read path. If a readback path ever force-opaques the X byte, add a raw-storage readback helper (`store.get(id).storage` + a host-visible copy) and compare against THAT instead. Do not let a readback-side fixup mask a real draw-side exactness failure.

- [ ] **Step 1: Write the failing test (depth-32 first)**

```rust
#[test]
#[ignore = "needs live Vulkan ICD"]
fn v2_masked_copyarea_matches_cmd_copy_image_depth32() {
    let mut b = match KmsBackendV2::for_tests_with_vk() {
        Ok(b) => b,
        Err(e) => { eprintln!("skipping: no Vk: {e}"); return; }
    };
    // src 8x8 gradient (depth-32 BGRA).
    let src = b.create_pixmap(None, 32, 8, 8).unwrap().as_raw();
    let mut bytes = vec![0u8; 8 * 8 * 4];
    for y in 0..8 { for x in 0..8 {
        let o = (y * 8 + x) * 4;
        bytes[o] = (x as u8) * 0x20;        // B
        bytes[o + 1] = (y as u8) * 0x20;    // G
        bytes[o + 2] = ((x + y) as u8) * 0x10; // R
        bytes[o + 3] = 0x7F;                // A (must survive verbatim at depth-32)
    }}
    b.put_image(None, src, 32, 8, 8, 0, 0, &bytes).unwrap();

    // dst_masked 8x8 (depth-32), pre-cleared to a sentinel.
    let dst_m = b.create_pixmap(None, 32, 8, 8).unwrap().as_raw();
    b.fill_rectangle(None, dst_m, 0x00000000, 0, 0, 8, 8).unwrap();
    // dst_ref 8x8 (depth-32) for the cmd_copy_image oracle.
    let dst_r = b.create_pixmap(None, 32, 8, 8).unwrap().as_raw();
    b.fill_rectangle(None, dst_r, 0x00000000, 0, 0, 8, 8).unwrap();

    // Full-ones mask 8x8 depth-1.
    let mask = b.create_pixmap(None, 1, 8, 8).unwrap().as_raw();
    let mut mbits = vec![0u8; 4 * 8];
    for row in 0..8 { mbits[row * 4] = 0xFF; }
    b.put_image(None, mask, 1, 8, 8, 0, 0, &mbits).unwrap();

    // Oracle: plain transfer copy src->dst_r.
    b.copy_area_for_tests_transfer(src, dst_r, 0, 0, 0, 0, 8, 8).unwrap(); // see note

    // Masked-blit src->dst_m through the all-ones mask, no scissor restriction.
    let full = [ash::vk::Rect2D {
        offset: ash::vk::Offset2D { x: 0, y: 0 },
        extent: ash::vk::Extent2D { width: 8, height: 8 },
    }];
    b.masked_copy_area_for_tests(src, dst_m, mask, (0, 0), 0, 0, 0, 0, 8, 8, &full).unwrap();

    let out_m = b.get_image_pixels_for_tests(dst_m, 2, 0, 0, 8, 8, !0).unwrap().unwrap();
    let out_r = b.get_image_pixels_for_tests(dst_r, 2, 0, 0, 8, 8, !0).unwrap().unwrap();
    assert_eq!(out_m, out_r, "masked-blit must be byte-identical to cmd_copy_image (depth-32)");
}
```

> **`copy_area_for_tests_transfer`:** the oracle must be a *plain* `cmd_copy_image` with NO clip. The cleanest oracle is the existing `engine.copy_area` with `ClipState::None`. Check whether the public `b.copy_area(None, ...)` with no clip set produces a single transfer (it does — the Pixmap branch is skipped when `current_clip` is `None`). If so, use `b.copy_area(None, src, dst_r, 0,0,0,0,8,8)` directly instead of adding a shim. Prefer the existing path; only add a `_transfer` shim if no clip-free entry exists.

- [ ] **Step 2: Run it — verify PASS or record the failure**

Run: `cargo test -p yserver --test v2_acceptance -- --ignored v2_masked_copyarea_matches_cmd_copy_image_depth32`
Expected: PASS. If it FAILS on byte-exactness, capture the diff; depth-32 failing would invalidate the approach — STOP and report (the spec says exclude failing formats; depth-32 failing means the primitive is wrong, not just one format).

- [ ] **Step 3: Add depth-24 (incl. X byte) and R8 depth-8 variants**

Duplicate the test for:
- `..._depth24`: `create_pixmap(None, 24, ...)`, src filled so the X byte is a non-trivial value (e.g. put_image bytes with `o+3 = 0x33`); assert raw bytes equal incl. byte 3. This proves no force-opaque fixup leaks in (server-owned-α applies to direct *writes*, NOT a GXcopy).
- `..._r8`: `create_pixmap(None, 8, ...)`; mask still depth-1; assert the single-channel bytes match.

- [ ] **Step 4: Run all three**

Run: `cargo test -p yserver --test v2_acceptance -- --ignored v2_masked_copyarea_matches_cmd_copy_image`
Expected: record PASS/FAIL **per format**. Write the result into the plan's "Exactness gate results" table below.

- [ ] **Step 5: Record results + commit**

Fill in:

| Format | Result | In scope? |
|---|---|---|
| BGRA8 depth-32 | _PASS/FAIL_ | _yes/no_ |
| BGRA8 depth-24 (incl. X byte) | _PASS/FAIL_ | _yes/no_ |
| R8 depth-8 | _PASS/FAIL_ | _yes/no_ |

```bash
git add crates/yserver/tests/v2_acceptance.rs docs/superpowers/plans/2026-06-21-gpu-side-clip-copyarea-plan.md
git commit -m "test(v2/clip): masked CopyArea byte-exactness gate vs cmd_copy_image"
```

**⛔ GO/NO-GO:** at least depth-32 MUST pass to proceed. Formats that fail are removed from the Task 14 routing predicate. If ALL fail, STOP — the design's exactness premise is broken; report back.

---

### Task 10: Phase-1 correctness tests (clip origin, clamp/OOB, self-overlap, scissor)

**Files:**
- Modify: `crates/yserver/tests/v2_acceptance.rs`

Each test uses `masked_copy_area_for_tests` and a fabricated mask drawable; asserts against an independently-computed expected image.

- [ ] **Step 1: `v2_masked_copyarea_honors_clip_origin`**

Mask 8×8 with rows 0..4 set; clip_origin (0, 2). dst pixel (x,y) gated by mask texel (x, y-2). So dst rows where `y-2 in [0,4)` i.e. rows 2..6 are copied; rows 0..1 and 6..7 retain the dst sentinel. Assert per-pixel. Also assert a clip_origin that pushes some dst pixels to mask-OOB → those discard (retain sentinel).

- [ ] **Step 2: `v2_masked_copyarea_clamps_negative_dst_and_oob_src`**

`dst_x = -2`: with the clamp/project, only the in-bounds sub-region is written; the masked output must equal the plain transfer-path output for the same negative offset (oracle = `b.copy_area(None, ...)` with no clip). Assert byte-equal AND that pixels outside the legal copy region keep the dst sentinel (no sampled-zero write).

- [ ] **Step 3: `v2_masked_copyarea_self_overlap`**

`src == dst`: pre-fill dst with a gradient; masked copy with `dst_x=2,dst_y=0` (a horizontal scroll), full-ones mask. Oracle = the existing transfer self-overlap path (`b.copy_area(None, dst, dst, ...)` with no clip, which uses `self_overlap_scratch`). Assert byte-equal.

- [ ] **Step 4: `v2_masked_copyarea_scissor_composes_with_gc_rects`**

Pass `scissors = [{0,0,4,8}]` (left half only) with a full-ones mask; assert only the left 4 columns are copied, right 4 retain sentinel. Then pass two scissors `[{0,0,4,8},{6,0,2,8}]` and assert the union is copied, the gap (cols 4..6) retains sentinel — proving per-rect scissored draws.

- [ ] **Step 5: Run all Phase-1 correctness tests**

Run: `cargo test -p yserver --test v2_acceptance -- --ignored v2_masked_copyarea`
Expected: all PASS (excluding any format excluded by Task 9).

- [ ] **Step 6: Commit**

```bash
git add crates/yserver/tests/v2_acceptance.rs
git commit -m "test(v2/clip): masked CopyArea clip-origin/clamp/self-overlap/scissor correctness"
```

---

# PHASE 2 — GC-owned snapshot carrier + backend routing

Phase 2 introduces the **pinned GPU snapshot** (retain-after-free + same-frame ordering) and routes the in-scope `copy_area` cases to `masked_copy_area`. Begins only after Task 9 GO.

---

### Task 11: `ClipSnapshot` carrier + engine-owned registry (state-carrier obligation)

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs` (new `ClipSnapshot` resource + `HashMap<SnapshotId, ClipSnapshot>` registry on `RenderEngineInner`; allocate/get/retire API)

This closes codex round-3 MEDIUM: the snapshot is NOT a `DrawableId`, so it needs its own `current_layout`, `last_render_ticket`, close-failure rollback, and retirement. We model it as an engine-owned resource keyed by an opaque `SnapshotId`, tracked with the same discipline as drawables.

- [ ] **Step 1: Define `ClipSnapshot` + the registry field** (`SnapshotId` already exists from Task 5 Step 0)

```rust
// SnapshotId was defined in Task 5 Step 0 — do NOT redefine it here.

/// GC-owned pinned depth-1 mask snapshot. Sampled by masked_copy_area; written
/// (re-copied from the live clip pixmap) only on the refresh path. Lifetime is
/// the GC's clip-mask install; survives the source pixmap being freed.
pub(crate) struct ClipSnapshot {
    vk: Arc<VkContext>,
    pub(crate) image: vk::Image,
    pub(crate) view: vk::ImageView, // IDENTITY R8
    memory: vk::DeviceMemory,
    pub(crate) extent: vk::Extent2D,
    pub(crate) current_layout: vk::ImageLayout,
    pub(crate) last_render_ticket: Option<FenceTicket>,
    /// content_version of the live mask at last (re)snapshot; gates refresh.
    pub(crate) snapshotted_version: u64,
    pub(crate) size_bytes: u64,
}

impl Drop for ClipSnapshot {
    fn drop(&mut self) {
        unsafe {
            self.vk.device.destroy_image_view(self.view, None);
            self.vk.device.destroy_image(self.image, None);
            self.vk.device.free_memory(self.memory, None);
        }
    }
}
```

On `RenderEngineInner`, add:

```rust
    clip_snapshots: HashMap<SnapshotId, ClipSnapshot>,
    next_snapshot_id: u64,
    /// Snapshots whose Drop is deferred behind a fence (retired this frame).
    retired_snapshots: Vec<(ClipSnapshot, Option<FenceTicket>)>,
```

Initialize all three in the constructor (`HashMap::new()`, `1`, `Vec::new()`).

- [ ] **Step 2: Allocator + create/retire/accessor API (NO `refresh_clip_snapshot` yet)**

`refresh_clip_snapshot` is deliberately NOT defined here — it depends on the Task-12 tracking helpers, and defining it as a `todo!()` would put a panicking stub in a committed tree (violates "no stubs"). It is introduced, fully, in Task 13. This task commits a clean carrier (create/retire/accessors) that compiles and is smoke-tested on its own (codex round-5 finding 6).

```rust
impl RenderEngine {
    /// Create a new pinned R8 snapshot image (TRANSFER_DST | SAMPLED), UNDEFINED
    /// layout, `snapshotted_version = u64::MAX` (forces the first refresh).
    /// Allocation only — the caller (Task 14, at clip-mask install while the
    /// source pixmap is guaranteed live) MUST call `refresh_clip_snapshot`
    /// (Task 13) to populate it BEFORE the first masked use: retain-after-free
    /// requires the snapshot hold real bytes before any later free (finding 5).
    pub(crate) fn create_clip_snapshot(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<SnapshotId, RenderError> {
        let inner = self.inner.as_mut().ok_or(RenderError::NoVk)?;
        // Reuse allocate_sampled_scratch_image's body but with R8_UNORM and a
        // persistent (non-Drop-on-scope) image. Inline the alloc here so the
        // image/memory/view live in ClipSnapshot, not SampledScratchImage.
        let format = vk::Format::R8_UNORM;
        let snap = alloc_clip_snapshot(&inner.vk.clone(), width, height, format)?;
        let id = SnapshotId(inner.next_snapshot_id);
        inner.next_snapshot_id = inner.next_snapshot_id.wrapping_add(1);
        inner.clip_snapshots.insert(id, snap);
        Ok(id)
    }

    pub(crate) fn clip_snapshot_extent(&self, id: SnapshotId) -> Option<vk::Extent2D> {
        self.inner.as_ref()?.clip_snapshots.get(&id).map(|s| s.extent)
    }

    pub(crate) fn clip_snapshot_version(&self, id: SnapshotId) -> Option<u64> {
        self.inner.as_ref()?.clip_snapshots.get(&id).map(|s| s.snapshotted_version)
    }

    /// Retire a snapshot (GC freed / re-allocated at new size). Deferred behind
    /// the snapshot's last_render_ticket so no in-flight frame samples a freed image.
    pub(crate) fn retire_clip_snapshot(&mut self, id: SnapshotId) {
        let Some(inner) = self.inner.as_mut() else { return; };
        if let Some(snap) = inner.clip_snapshots.remove(&id) {
            let guard = snap.last_render_ticket.clone();
            inner.retired_snapshots.push((snap, guard));
        }
    }
}

fn alloc_clip_snapshot(
    vk: &Arc<VkContext>,
    width: u32,
    height: u32,
    format: vk::Format,
) -> Result<ClipSnapshot, RenderError> {
    // identical to allocate_sampled_scratch_image's image/memory/view creation,
    // then wrap in ClipSnapshot with current_layout = UNDEFINED,
    // last_render_ticket = None, snapshotted_version = u64::MAX (force first refresh).
    // ... (clone the alloc body; see Task 4) ...
}
```

- [ ] **Step 3: Drain retired snapshots in BOTH `poll_retired` and `drain_all`**

`retired_snapshots` must be drained at fence completion in `poll_retired` (engine.rs:1195 — drop entries whose guard is signaled) AND fully released at shutdown in `drain_all` (engine.rs:1337 — drop all entries; their `Drop` frees the Vk objects). Covering only `poll_retired` leaks the last batch at teardown (codex round-5 finding 8). Match the pattern the existing `retired_promoted_images`/scratch drains use in both functions.

- [ ] **Step 4: Verify it compiles + a smoke test**

```rust
    #[test]
    #[ignore = "needs live Vulkan ICD"]
    fn clip_snapshot_create_and_retire() {
        // for_tests_with_vk backend; create_clip_snapshot(8,8); assert extent;
        // retire_clip_snapshot; assert no longer present.
    }
```

Run: `cargo test -p yserver --test v2_acceptance -- --ignored clip_snapshot_create_and_retire`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo build -p yserver
git add crates/yserver/src/kms/v2/engine.rs crates/yserver/tests/v2_acceptance.rs
git commit -m "feat(v2/clip): ClipSnapshot carrier + engine registry (layout/ticket/retire)"
```

---

### Task 12: Snapshot layout/ticket tracking + close-failure rollback

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs` (`masked_copy_area` snapshot bookkeeping; `OpenFrame` snapshot first-touch; `rollback_pre_submit` extension)
- Modify: `crates/yserver/src/kms/v2/frame_builder.rs` (`OpenFrame` snapshot overlay)

The snapshot's terminal layout after a masked-blit is `SHADER_READ_ONLY_OPTIMAL`. On a successful close, commit it; on a close FAILURE, restore the pre-frame layout/ticket — mirroring `rollback_atlas`.

- [ ] **Step 1: Add snapshot overlay to `OpenFrame`**

In `frame_builder.rs`, alongside `FrameLayoutTable.atlas`, add a per-frame snapshot record:

```rust
    /// Snapshots touched this frame: id -> (pre_frame_layout, prior_ticket,
    /// prev_snapshotted_version). The third element is load-bearing for rollback
    /// (codex round-4): a failed close must restore the OLD version so the next
    /// frame still re-refreshes. Mirrors the drawable overlay + atlas snapshot.
    pub(crate) snapshot_touch: HashMap<super::engine::SnapshotId,
        (vk::ImageLayout, Option<FenceTicket>, u64)>,
```

Initialize `HashMap::new()` in `OpenFrame::new`/`Default`.

- [ ] **Step 2: Record snapshot first-touch in BOTH ops that touch a snapshot**

Two ops touch a snapshot: `masked_copy_area` (SAMPLES it, when `mask.snapshot_id` is `Some`) and `refresh_clip_snapshot` (WRITES it). Both must record the first-touch snapshot for rollback. Factor a helper on the engine:

```rust
    fn snapshot_first_touch(inner: &mut RenderEngineInner, sid: SnapshotId) {
        // read locals before the mutable open-frame borrow (sibling fields).
        let (layout, ticket, ver) = {
            let snap = inner.clip_snapshots.get(&sid).expect("snapshot");
            (snap.current_layout, snap.last_render_ticket.clone(), snap.snapshotted_version)
        };
        let open = inner.frame_builder.open.as_mut().expect("open");
        open.snapshot_touch.entry(sid).or_insert((layout, ticket, ver));
    }
```

Call `snapshot_first_touch(inner, sid)` at the top of the snapshot-touching block in `masked_copy_area` (when `mask.snapshot_id == Some(sid)`). It is ALSO called from `refresh_clip_snapshot` — but that method lands in Task 13, so this task only wires the `masked_copy_area` (SAMPLE) call site.

- [ ] **Step 3: Commit snapshot terminal state on append (SAMPLE path)**

In `masked_copy_area` (SAMPLE — read-only, advances the ticket but NOT the version):

```rust
    if let Some(sid) = mask.snapshot_id {
        if let Some(snap) = inner.clip_snapshots.get_mut(&sid) {
            snap.current_layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
            snap.last_render_ticket = Some(frame_ticket.clone());
        }
    }
```

(The WRITE-path commit — which also advances `snapshotted_version` — lives in `refresh_clip_snapshot`, Task 13, since that method is defined there.)

- [ ] **Step 4: Extend close-failure rollback (ALL `rollback_atlas` sites)**

Add a `rollback_snapshots` and call it EVERYWHERE `rollback_atlas` is called in `close_open_frame`. There are **five** such sites, not three (codex round-5 finding 5): begin-CB failure (engine.rs:1592), record failure (1659), completion-signal-acquire failure (1710), append failure (1758), and **flush failure (1955)**. The flush-failure site is the most important: append already mutated the snapshot's layout/ticket/version but the submit never completed, so the bytes are still the OLD version. Restoring the version is mandatory or the next frame skips a needed re-refresh and samples stale bytes:

```rust
fn rollback_snapshots(
    inner: &mut RenderEngineInner,
    snapshot_touch: &mut std::collections::HashMap<SnapshotId,
        (vk::ImageLayout, Option<FenceTicket>, u64)>,
) {
    for (id, (pre_layout, prior_ticket, prev_version)) in snapshot_touch.drain() {
        if let Some(snap) = inner.clip_snapshots.get_mut(&id) {
            snap.current_layout = pre_layout;
            snap.last_render_ticket = prior_ticket;
            snap.snapshotted_version = prev_version;
        }
    }
}
```

Wire `rollback_snapshots(inner, &mut open_frame.snapshot_touch)` next to EACH of the five `rollback_atlas` calls. (Grep `rollback_atlas` in engine.rs to enumerate them — do not rely on the line numbers above; they drift.)

- [ ] **Step 5: Verify it compiles + a rollback test (SAMPLE path)**

Add a test that injects a close failure (descriptor ring `test_inject_next_allocate_error` / `test_force_next_reset_failure`) during a `masked_copy_area` that SAMPLES a snapshot (the snapshot need not be populated — the close fails before the draw runs), then asserts the snapshot's `current_layout`, `last_render_ticket`, AND `snapshotted_version` are unchanged from pre-frame. (A refresh-path rollback test is added in Task 13 once `refresh_clip_snapshot` exists.)

Run: `cargo test -p yserver --test v2_acceptance -- --ignored masked_copyarea_snapshot_rollback`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo build -p yserver
git add crates/yserver/src/kms/v2/engine.rs crates/yserver/src/kms/v2/frame_builder.rs crates/yserver/tests/v2_acceptance.rs
git commit -m "feat(v2/clip): snapshot layout/ticket tracking + close-failure rollback"
```

---

### Task 13: Define `refresh_clip_snapshot` — live-mask participation + append the refresh op

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs` (introduce `refresh_clip_snapshot`, NEW method — no `todo!()` ever existed; Task 11 deliberately omitted it)

This introduces the full `refresh_clip_snapshot` (the carrier from Task 11 plus the tracking helpers from Task 12 are now both available). The live clip pixmap is read by the snapshot copy, so it is a first-class frame participant (spec § Frame-builder integration, 4th role). The op leaves both the live mask and the snapshot at SHADER_READ; the snapshot version is committed here (the WRITE path deferred from Task 12 Step 3).

- [ ] **Step 1: Implement the method**

```rust
    pub(crate) fn refresh_clip_snapshot(
        &mut self,
        store: &mut DrawableStore,
        platform: &mut PlatformBackend,
        id: SnapshotId,
        live_mask_id: DrawableId,
        version: u64,
    ) -> Result<(), RenderError> {
        // No-op if already current (read before any mutation).
        if self.inner.as_ref().and_then(|i| i.clip_snapshots.get(&id))
            .map(|s| s.snapshotted_version) == Some(version)
        {
            return Ok(());
        }
        // ENTRY PRELUDE — same as copy_area/masked_copy_area (finding 4):
        // renderer guard + flush_render_batch BEFORE open-frame mutation, so the
        // refresh op is chronologically ordered after any pending render batch.
        if platform.renderer_failed {
            return Err(RenderError::RendererFailed);
        }
        self.flush_render_batch(store, platform)?;
        let inner = self.inner.as_mut().ok_or(RenderError::NoVk)?;

        let lm = store.get(live_mask_id).ok_or(RenderError::UnknownDrawable(live_mask_id))?;
        let live_image = lm.storage.image;
        let copy_extent = inner.clip_snapshots.get(&id).expect("snapshot").extent;
        let snap_image = inner.clip_snapshots.get(&id).expect("snapshot").image;
        let snap_old = inner.clip_snapshots.get(&id).expect("snapshot").current_layout;

        // Live-mask drawable participation (first-touch/ticket/old-layout); it
        // is a READ → terminal SHADER_READ.
        let lm_pre = inner.current_layout_for_drawable(store, live_mask_id);
        let prior_lm = store.get(live_mask_id).and_then(|d| d.last_render_ticket.clone());
        let frame_ticket = /* obtain as copy_area does */;
        {
            let open = inner.frame_builder.open.as_mut().expect("open");
            open.touched.first_touch(live_mask_id, prior_lm);
            open.layouts.first_touch_drawable(live_mask_id, lm_pre);
        }
        store.touch_render_fence(live_mask_id, frame_ticket.clone());

        // Snapshot first-touch for rollback (Task 12 Step 2).
        Self::snapshot_first_touch(inner, id);

        // Append the standalone refresh op + set the live-mask terminal overlay.
        let payload = Box::new(super::frame_builder::RecordedClipSnapshotRefresh {
            snapshot_id: id,
            snapshot_image: snap_image,
            snapshot_old_layout: snap_old,
            live_mask_id,
            live_mask_image: live_image,
            live_mask_old_layout: lm_pre,
            copy_extent,
        });
        {
            let open = inner.frame_builder.open.as_mut().expect("open");
            open.push_op_and_set_layouts(
                super::frame_builder::RecordedOp::ClipSnapshotRefresh(payload),
                &[(live_mask_id, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)],
            );
        }
        // Commit snapshot terminal state + version (Task 12 Step 3, write path).
        if let Some(snap) = inner.clip_snapshots.get_mut(&id) {
            snap.current_layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
            snap.last_render_ticket = Some(frame_ticket.clone());
            snap.snapshotted_version = version;
        }
        Ok(())
    }
```

> Read the live `copy_area`/`masked_copy_area` body for the exact `frame_ticket` acquisition + open-frame accessors and reproduce them verbatim. Split the `inner.clip_snapshots` vs `inner.frame_builder` borrows by reading locals first.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p yserver`
Expected: PASS.

- [ ] **Step 3: Refresh-path rollback test**

Now that `refresh_clip_snapshot` exists, add the WRITE-path rollback test (the SAMPLE-path one was Task 12 Step 5): create a snapshot, `refresh_clip_snapshot` it to version V (records the op + advances `snapshotted_version` to V), inject a close failure (`test_force_next_reset_failure`), and assert the snapshot's `current_layout`, `last_render_ticket`, AND `snapshotted_version` rolled back to their pre-frame values (so the next frame re-refreshes). This exercises the flush-failure `rollback_snapshots` path that the SAMPLE-path test does not.

Run: `cargo test -p yserver --test v2_acceptance -- --ignored clip_snapshot_refresh_rollback`
Expected: PASS.

- [ ] **Step 4: Same-frame ordering check (review, not code)**

Confirm the close-time op order guarantees correctness: when the backend appends `ClipSnapshotRefresh` then `MaskedCopyArea` in the same frame, the refresh emit leaves the snapshot at SHADER_READ and the masked emit's mask→SHADER_READ barrier (with `ALL_COMMANDS` source) provides the read-after-write dependency. The refresh's `ALL_COMMANDS` source on the live read orders it after a same-frame write to the live mask. Both terminal layouts are SHADER_READ (N1).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add crates/yserver/src/kms/v2/engine.rs crates/yserver/tests/v2_acceptance.rs
git commit -m "feat(v2/clip): refresh_clip_snapshot (live-mask participation + append + rollback)"
```

---

### Task 14: Backend routing — detect in-scope copies, manage snapshot, route to masked_copy_area

**Files:**
- Modify: `crates/yserver/src/kms/v2/backend.rs` (the `copy_area` `ClipState::Pixmap` branch, ~12642; snapshot ownership on the GC/clip state; `set_clip_pixmap`/`set_clip_rectangles` snapshot lifecycle)

This is the integration. In the `ClipState::Pixmap` + GXcopy + full-plane-mask + in-scope-format case, route ONE masked draw instead of the run fan-out. Out-of-scope cases (excluded formats from Task 9, non-Copy rop, partial plane, rect-clip-only) stay on the existing path.

- [ ] **Step 1: Add a snapshot handle to the clip state**

Store the `SnapshotId` keyed by the clip-mask pixmap. **Ownership (codex round-4 finding 6):** yserver tracks a SINGLE current clip (`self.core.current_clip` + the single `self.clip_mask_cache`), re-installed via `apply_clip_state` whenever the active GC's clip changes — it is NOT a per-GC table. The snapshot follows that exact model: ONE backend-level field, mirroring `clip_mask_cache`'s lifetime and `(pixmap_xid, drawable_id, content_version)` keying. (Trade-off: alternating between two GCs with different clip masks re-creates the snapshot on each switch — acceptable; it matches how `clip_mask_cache` already behaves, and gkrellm uses one clip mask.)

```rust
    /// GPU snapshot of the current clip-mask pixmap for the masked CopyArea path.
    /// Single current-clip carrier, mirroring `clip_mask_cache`'s ownership.
    /// Created + eagerly populated on install; re-refreshed on content_version
    /// change while the pixmap is live; retired on pixmap free or size change.
    clip_mask_snapshot: Option<ClipMaskSnapshot>,
```

```rust
struct ClipMaskSnapshot {
    pixmap_xid: u32,
    drawable_id: crate::kms::v2::store::DrawableId,
    id: crate::kms::v2::engine::SnapshotId,
    width: u32,
    height: u32,
}
```

- [ ] **Step 2: Snapshot lifecycle + EAGER POPULATION in `install_clip_mask_cache`**

**Hook the SHARED sink, not just `set_clip_pixmap` (codex round-5 finding 3).** The live ChangeGC clip-mask path is `handle_change_gc → resolve_draw_state → apply_clip_state` (backend.rs:12433), which — like `set_clip_pixmap` (12396) and the per-paint `intersect_with_current_clip_live` (6039) — funnels through `install_clip_mask_cache` (backend.rs:6168). Put the snapshot lifecycle THERE so all three callers are covered with one change; it is idempotent (create only on xid/size change; refresh no-ops when the version matches), so the per-paint call adds no cost when nothing changed.

In `install_clip_mask_cache(xid, origin)`, after the existing cache install, when the in-scope predicate holds and `xid` resolves to a live drawable:
1. If no snapshot, or the snapshot's `pixmap_xid`/`drawable_id`/size differ from this `xid`'s → `engine.retire_clip_snapshot(old.id)` (if any) then `engine.create_clip_snapshot(w, h)`; store `ClipMaskSnapshot { pixmap_xid: xid, drawable_id, id, width, height }`.
2. **Eagerly populate it WHILE THE PIXMAP IS LIVE** (finding 5 — closes retain-after-free before first use). Resolve the live `DrawableId` + `content_version` and call:
   ```rust
   self.engine.refresh_clip_snapshot(
       &mut self.store, &mut self.platform, snap.id, live_id, live_version,
   )?;
   ```
   `refresh_clip_snapshot` no-ops when the version already matches, so the per-paint install path costs nothing for a static mask.
- Keep the snapshot across `ClipState::None` (retain-after-free): `install_clip_mask_cache` is not called for `None` (the clip→None branches do not touch it), so the snapshot naturally persists; do NOT retire on clip clear, mirroring the cache contract (backend.rs:12380). After a clear+free, the snapshot still holds the bytes captured at install/last-refresh.

> `install_clip_mask_cache` currently returns `()`. Either keep it `()` and log+swallow a `refresh_clip_snapshot` error (the masked path then falls back to the run path because the snapshot version stays stale), or change the signature to propagate. Prefer log+swallow so a transient refresh failure degrades to the existing CPU path rather than failing the request.

- [ ] **Step 3: Define the in-scope predicate**

Reuse the EXISTING eligibility helper (codex round-6 finding 2 — the real enum variant is `GcFunction::Copy`, NOT `GXcopy`, and `full_mask` is a `u32` tested as `plane_mask == full_mask`; the helper at backend.rs:8904 already encodes this), then add the depth gate:

```rust
fn copy_area_masked_blit_eligible(
    function: yserver_core::backend::GcFunction,
    plane_mask: u32,
    full_mask: u32,
    dst_depth: u8,
) -> bool {
    // GcFunction::Copy + full plane mask (mirror copy_area_clip_gpu_eligible).
    copy_area_clip_gpu_eligible(function, plane_mask, full_mask)
        // AND a dst depth whose format passed the Task 9 byte-exactness gate.
        // Set this arm to EXACTLY the depths Task 9 recorded as PASS (e.g.
        // 32 | 24 | 8). A format that FAILED the gate must NOT appear here.
        && matches!(dst_depth, 32 | 24 | 8)
}
```

> `function`/`plane_mask`/`full_mask` are the same bindings the existing helper is called with in the surrounding `copy_area` body — reuse them. Set the `dst_depth` arm to EXACTLY the formats Task 9 recorded as PASS.

- [ ] **Step 4: Route at the TOP of the `ClipState::Pixmap` branch — BEFORE `intersect_with_current_clip_live`**

**Placement (codex round-6 finding 3):** the existing branch calls `intersect_with_current_clip_live(&[local])` at backend.rs:12652 as its FIRST action — that call runs `install_clip_mask_cache` → `read_clip_mask_bytes` → `engine.get_image` (the per-op clip readback this whole feature exists to kill). The masked route MUST be inserted at the very TOP of the `ClipState::Pixmap` branch (right after the `matches!(... Pixmap ...)` guard at backend.rs:12642), BEFORE the `intersect_with_current_clip_live` call and the run loop. It must NOT call `intersect_with_current_clip_live` at all — it gets its non-mask scissors from `compute_copy_area_scissors` (GC-rect/child/window rects, no mask raster, no readback) and the mask from the GPU snapshot. If eligibility/snapshot conditions don't hold, fall through to the existing `intersect_with_current_clip_live` + run-loop path unchanged.
>
> Note: the snapshot's own population still reads the mask once at install (`install_clip_mask_cache` → `read_clip_mask_bytes`, for the CPU cache that fills/segments still use), but that is install-frequency (≈ per content_version change), NOT per-copy. Task 15's no-readback assertion is about the per-copy path, which this placement satisfies.

First add these engine accessors (mirror `clip_snapshot_extent` from Task 11), so the backend never reaches into engine internals:

```rust
    pub(crate) fn clip_snapshot_image(&self, id: SnapshotId) -> Option<vk::Image> {
        self.inner.as_ref()?.clip_snapshots.get(&id).map(|s| s.image)
    }
    pub(crate) fn clip_snapshot_view(&self, id: SnapshotId) -> Option<vk::ImageView> {
        self.inner.as_ref()?.clip_snapshots.get(&id).map(|s| s.view)
    }
    pub(crate) fn clip_snapshot_layout(&self, id: SnapshotId) -> Option<vk::ImageLayout> {
        self.inner.as_ref()?.clip_snapshots.get(&id).map(|s| s.current_layout)
    }
```

Then route. Note `src`/`dst`/`function`/`plane_mask`/`full_mask`/`dst_target`/`dst_target_depth`/`src_x..height` are the EXACT bindings already live in the surrounding `copy_area` body (read backend.rs:12551-12741 and reuse them verbatim — do not re-resolve):

```rust
    if copy_area_masked_blit_eligible(function, plane_mask, full_mask, dst_target_depth)
        && self.clip_mask_snapshot.is_some()
    {
        // COORDINATE SPACES (codex round-5 finding 2): the masked draw runs in
        // dst BACKING/IMAGE space (gl_FragCoord = image pixel). The existing run
        // path applies `src_off` (source window's offset within its backing) to
        // src and `dst_target.offset` (dst backing offset) to dst, per-subrect
        // (backend.rs:12918-12931). The masked route MUST apply the same shifts:
        //   src_pos  = (src_x + src_off.0, src_y + src_off.1)        [image space]
        //   dst_pos  = (dst_x + dst_target.offset.0, dst_y + .1)     [image space]
        //   clip_off = clip_origin + dst_target.offset  (so frag's
        //              mask_texel = image_pixel - clip_off = logical_pixel - clip_origin)
        //   scissors = local surviving rects + dst_target.offset     [image space]
        // For a plain pixmap dst both offsets are 0 (matches the Phase-1 tests).
        let (sox, soy) = src_off;
        let (tox, toy) = dst_target.offset;

        // Surviving scissors in LOCAL space (Step 5 helper), mapped to image space.
        let scissors: Vec<ash::vk::Rect2D> = self
            .compute_copy_area_scissors(dst_target, dst_x, dst_y, width, height)
            .into_iter()
            .map(|r| ash::vk::Rect2D {
                offset: ash::vk::Offset2D { x: r.offset.x + tox, y: r.offset.y + toy },
                extent: r.extent,
            })
            .collect();
        if scissors.is_empty() { return Ok(()); }

        let snap = self.clip_mask_snapshot.as_ref().unwrap();
        let sid = snap.id;
        // SINGLE refresh mechanism (Task 11/13): re-snapshot only when the live
        // mask changed since the last snapshot AND the source pixmap is still
        // alive. If it was freed, the snapshot (populated at install) is
        // authoritative — retain-after-free.
        if let Some(did) = self.store.lookup(snap.pixmap_xid) {
            let live_ver = self.store.get(did).map(|d| d.content_version);
            if self.engine.clip_snapshot_version(sid) != live_ver {
                if let Some(v) = live_ver {
                    self.engine
                        .refresh_clip_snapshot(&mut self.store, &mut self.platform, sid, did, v)
                        .map_err(|e| io::Error::other(format!("refresh_clip_snapshot: {e:?}")))?;
                }
            }
        }

        let (origin_x, origin_y) = match &self.core.current_clip {
            ClipState::Pixmap { origin, .. } => *origin,
            _ => (0, 0),
        };
        let mask = crate::kms::v2::engine::MaskedCopyMask {
            image: self.engine.clip_snapshot_image(sid).unwrap(),
            view: self.engine.clip_snapshot_view(sid).unwrap(),
            old_layout: self.engine.clip_snapshot_layout(sid).unwrap(),
            extent: self.engine.clip_snapshot_extent(sid).unwrap(),
            // clip origin shifted into image space (see COORDINATE SPACES above).
            clip_origin: [i32::from(origin_x) + tox, i32::from(origin_y) + toy],
            snapshot_id: Some(sid),
        };
        let dst = dst_target.id;
        self.engine.masked_copy_area(
            &mut self.store, &mut self.platform, src, dst,
            ash::vk::Offset2D { x: i32::from(src_x) + sox, y: i32::from(src_y) + soy },
            ash::vk::Offset2D { x: i32::from(dst_x) + tox, y: i32::from(dst_y) + toy },
            ash::vk::Extent2D { width: width.into(), height: height.into() },
            mask, &scissors,
        ).map_err(|e| io::Error::other(format!("masked_copy_area: {e:?}")))?;
        // ONE masked draw replaces the run fan-out. Telemetry: Task 15.
        return Ok(());
    }
    // ... fall through to the existing run-based path for out-of-scope cases ...
```

> `src` is the source `DrawableId` the existing Pixmap branch already resolved (windows via `resolve_paint_target`, pixmaps via the raw store lookup at backend.rs:12578-12605); `src_off` and `dst_target.offset` are the EXACT bindings the existing per-subrect loop uses (backend.rs:12918-12931). Reuse them; do not re-derive. Confirm the precise tuple types/field names (`src_off.0` vs `.x`) against that loop.

- [ ] **Step 5: `compute_copy_area_scissors` helper**

Refactor the existing rect machinery (GC-rect intersect at 12743, child-subtract at 12782, sibling occluders at 12843) into a helper returning `Vec<vk::Rect2D>` of surviving dst rects in **drawable-LOCAL space** (exactly what the existing `sub_rects` loop iterates BEFORE it adds `dst_target.offset`). Reuse it in both the new path (Step 4 maps local→image) and the old path (which adds `dst_target.offset` per-rect as today). Keep behavior identical — extract, don't rewrite; verify the extracted helper reproduces the current `sub_rects` for a window dst (nonzero `dst_target.offset`).

- [ ] **Step 6: Verify it compiles + lint**

Run: `cargo build -p yserver && cargo clippy -p yserver`
Expected: PASS, no new warnings.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add crates/yserver/src/kms/v2/backend.rs crates/yserver/src/kms/v2/engine.rs
git commit -m "feat(v2/clip): route in-scope clip-masked GXcopy CopyArea to masked_copy_area"
```

---

### Task 15: Telemetry — count the masked draw, assert no fan-out + no clip readback

**Files:**
- Modify: `crates/yserver/src/kms/v2/telemetry.rs` (new counter)
- Modify: `crates/yserver/src/kms/v2/backend.rs` (increment at the masked-draw site)
- Modify: `crates/yserver/tests/v2_acceptance.rs` (routing + no-readback tests)

- [ ] **Step 1: Add a counter**

In `telemetry.rs`, add `copy_area_masked_draw: u64` to the bucket + lifetime structs (near `copy_area_gpu_subrect_maskrun`, telemetry.rs:122) and:

```rust
    pub(crate) fn record_copy_area_masked_draw(&mut self) {
        self.bucket.copy_area_masked_draw += 1;
        self.lifetime.copy_area_masked_draw += 1;
    }
```

- [ ] **Step 2: Increment at the masked-draw site**

In the Task-14 routed branch, just before `self.engine.masked_copy_area(...)`, call `self.telemetry.record_copy_area_masked_draw();`. Note: this path does NOT call `record_copy_area_gpu_subrect_at(true)` (no fan-out) and does NOT call `read_clip_mask_bytes` (no `GetImageSite::ClipMask`).

- [ ] **Step 3: `masked_copyarea_routes_to_draw_not_transfer`**

Crate-local (`for_tests_with_vk`): set up a `ClipState::Pixmap` GXcopy copy through an installed clip mask, run one `copy_area`; assert `telemetry.lifetime.copy_area_masked_draw == 1` AND `copy_area_gpu_subrect_maskrun` did NOT increment for that copy.

- [ ] **Step 4: `masked_copyarea_no_clip_get_image`**

Same setup; assert `telemetry.lifetime.get_image_by_site[GetImageSite::ClipMask as usize]` is unchanged across the routed copy (no readback). Run a static-mask scenario (no content_version change) to confirm zero readbacks on the cache-hit path.

- [ ] **Step 5: `noncopy_or_partial_planemask_copy_still_uses_old_path` (scope guard)**

A GXxor (or partial plane-mask) clip-masked copy must NOT route to the masked draw: assert `copy_area_masked_draw` unchanged and the old path's counter (`copy_area_cpu_rop` or `copy_area_gpu_subrect_maskrun`) increments instead.

- [ ] **Step 6: Run all telemetry tests**

Run: `cargo test -p yserver --test v2_acceptance -- --ignored masked_copyarea`
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add crates/yserver/src/kms/v2/telemetry.rs crates/yserver/src/kms/v2/backend.rs crates/yserver/tests/v2_acceptance.rs
git commit -m "feat(v2/clip): masked-draw telemetry + routing/no-readback/scope-guard tests"
```

---

### Task 16: Retain-after-free + same-frame-write integration tests

**Files:**
- Modify: `crates/yserver/tests/v2_acceptance.rs`

These exercise the snapshot's load-bearing guarantees through the full backend path.

- [ ] **Step 1: `v2_masked_copyarea_retains_clip_after_pixmap_freed`**

Install a `ClipState::Pixmap` clip mask (rows 0..4 set), FREE the source pixmap (`free_pixmap`), then a masked GXcopy CopyArea: assert the result still honors the mask (top half copied, bottom half retains dst). Proves the snapshot survives the free and the cache-hit path never touches the freed drawable.

- [ ] **Step 2: `v2_masked_copyarea_mask_written_same_frame`**

In ONE frame: `put_image`/fill the mask pixmap (bumping its `content_version`), then masked CopyArea through it. Assert the result reflects the NEW mask bytes (the refresh re-copy + barrier ordering executed before the masked-blit). Use a fresh backend so the mask starts at a known version.

- [ ] **Step 3: Run + confirm the existing clip suite stays green**

Run:
```
cargo test -p yserver --test v2_acceptance -- --ignored \
  v2_masked_copyarea v2_clip_pixmap_mask_gates_poly_fill render_composite_no_gc_clip_leak
```
Expected: all PASS (no regression in the existing clip-correctness suite).

- [ ] **Step 4: Full lavapipe acceptance sweep**

Run: `cargo test -p yserver --test v2_acceptance -- --ignored`
Expected: 100% pass (per the V1+pixman baseline memory, any deviation is a real regression). Investigate any failure before proceeding.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/tests/v2_acceptance.rs
git commit -m "test(v2/clip): retain-after-free + same-frame-write integration"
```

---

### Task 17: Full verification + HW/vng release gate

**Files:** none (verification only)

- [ ] **Step 1: Clean build + lint + format**

Run:
```
cargo fmt --check
cargo build -p yserver
cargo clippy -p yserver
cargo test -p yserver
```
Expected: all green, no new warnings. (Pedantic is NOT run by default per project convention.)

- [ ] **Step 2: bee + 6-core vng gkrellm telemetry (the spec's HW gate)**

This requires the HW/tmux procedure (memory: `feedback_hw_recipes_user_only`) and coordination (ONE agent per checkout). Run gkrellm under cinnamon on bee with `YSERVER_LOOP_TELEMETRY=1` and confirm the spec's expected deltas:
- `copy_area_gpu_subrect[maskrun]` → ~0 (replaced).
- new `copy_area_masked_draw` ≈ `copy_area_calls` (1 draw/copy, no fan-out).
- `op62` t/s and `req_time%` drop sharply.
- `get_image_by_site[clip]` on the copy path → 0.
- gkrellm core usage drops; NO `DEVICE_LOST`; clip rendering visually correct (gkrellm charts, wmaker title buttons).

> **vng pass is NOT an HW pass** (memory). The `cargo test` + lavapipe sweep is the iteration signal; bee HW is the release gate. Capture the telemetry snapshot and a visible smoke check (gkrellm charts render correctly).

- [ ] **Step 3: Update project memory**

Update `project_client_scheduling_fairness` (the gkrellm storm tracker) with the outcome: maskrun fan-out eliminated for in-scope formats; note any format excluded by the Task 9 gate; note that submit-batching (the other gkrellm root cause) remains separate, open work.

- [ ] **Step 4: Finish the branch**

Use the `superpowers:finishing-a-development-branch` skill to decide merge/PR. Per project rules: master is branch-protected (PR only); draft any PR text for explicit approval before publishing (never publish in the user's name).

---

## Self-review against the spec

**Spec coverage:**
- Sampled masked-blit draw → Tasks 1, 2, 6. ✓
- Dedicated `masked_blit` pipeline, texelFetch, no RENDER premul → Task 2 (blend disabled, own DSL). ✓
- IDENTITY image_view for src + mask → Task 2 `write_views` doc + Task 7 uses `storage.image_view`; snapshot uses IDENTITY R8 view (Task 11). ✓
- Threshold not coverage (`!= 0`) → Task 1 frag (`<= 0.0` discard). ✓
- Mask source = pinned GC-owned GPU snapshot keyed by content_version → Tasks 11–14. ✓
- Re-snapshot only on content_version change; amortized → Task 14 routes through `refresh_clip_snapshot` (no-ops when version matches). ✓
- Snapshot persists after source freed → Task 14 populates eagerly at install + keeps it across `None`/free; Task 16 test. ✓
- Geometry clamp/project verbatim + OOB-src discard → Task 7 Step 2 + Task 1 frag + Task 10. ✓
- Exactness gate empirical, per format, exclude failures → Task 9 (GO/NO-GO) + Task 14 predicate. ✓ (closes obligation 1)
- Self-overlap scratch (SAMPLED), lifetime adopted at close → Tasks 4, 6 (Step 4), 7, 10. ✓
- Rect-clip composition via per-rect scissor → Task 6 draw loop + Task 14 `compute_copy_area_scissors` + Task 10 scissor test. ✓
- Frame-builder integration (dst/src roles in masked op; live-mask + snapshot roles in the standalone refresh op) → Tasks 5, 7, 11, 13. ✓
- Explicit PipelineBarrier2 from recorded old layouts (masked draw + standalone refresh) → Task 6. ✓
- Snapshot state carrier (layout/ticket/version/rollback/retirement) → Tasks 11, 12. ✓ (closes obligation 2)
- Scope IN/OUT → Task 14 predicate; out-of-scope falls through. ✓
- All listed tests → Tasks 9, 10, 15, 16. ✓
- HW/vng gate → Task 17. ✓

**Codex round-4 review resolutions (folded into the tasks above):**
1. Self-overlap dst barrier uses the correct `TRANSFER_SRC → COLOR` old layout (Task 6, `dst_is_transfer_src`).
2. Live src image (copy/barrier) is kept separate from the sampled view (Task 5 struct; Task 6 binds `sample_view`; Task 7 sets both).
3. Live mask is transitioned back `TRANSFER_SRC → SHADER_READ` after the snapshot copy (Task 6 `emit_recorded_clip_snapshot_refresh_into_cb`).
4. `SampledScratchImage` lifetime is adopted into the submitted-resource set at close (Task 6 Step 4).
5. Snapshot is populated eagerly at install while the pixmap is live (Task 14 Step 2) — closes retain-after-free before first use. The refresh became a single standalone `ClipSnapshotRefresh` op used at install AND on same-frame change.
6. Snapshot ownership made explicit as the single-current-clip model, mirroring `clip_mask_cache` (Task 14 Step 1).
7. Readback raw-byte check added as Task 9 Step 0 (verify `pack_from_storage` exposes raw bytes incl. the depth-24 X byte).
8. `MaskedCopyMask` is fully defined up front in Task 7 (no later field-widening churn).
9. Task 14 accessors named concretely (`clip_snapshot_{image,view,layout}`); scissor-extraction boundary defined (Step 5).

**Codex round-5 review resolutions (folded in):**
1. Self-overlap copies the LIVE src region: added `live_src_offset` (= `src_rect.offset`) to the op; the self-overlap copy uses it, NOT the sample-space `copy_offset` (Task 5/6/7).
2. Backend coordinate spaces: Task 14 Step 4 now applies `src_off` to src, `dst_target.offset` to dst, shifts `clip_origin` into image space, and maps scissors local→image — matching the existing run loop (backend.rs:12918-12931).
3. Snapshot lifecycle hooked into the SHARED `install_clip_mask_cache` sink, covering `set_clip_pixmap` AND `apply_clip_state` AND the per-paint path (Task 14 Step 2).
4. `masked_copy_area` and `refresh_clip_snapshot` use the real entry prelude (renderer-failed guard + `flush_render_batch`), not `ensure_open_frame` (Task 7, Task 13).
5. `rollback_snapshots` wired at ALL FIVE `rollback_atlas` sites incl. flush-failure (Task 12 Step 4).
6. No committed `todo!()`: Task 11 defines carrier + create/retire/accessors only; `refresh_clip_snapshot` is introduced whole in Task 13. Each task commits a compiling, non-stub tree.
7. `SampledScratchImage` close-time adoption also updates `active_resource_bytes` accounting (Task 6 Step 4).
8. `retired_snapshots` drained in BOTH `poll_retired` and `drain_all` (Task 11 Step 3).

**Codex round-6 review resolutions (folded in):**
1. `SnapshotId` newtype now defined in Task 5 Step 0 (Phase 1, first use); Task 11 adds only `ClipSnapshot` + the registry — no forward reference, every Phase-1 commit compiles.
2. Eligibility predicate reuses the real `copy_area_clip_gpu_eligible(function, plane_mask, full_mask: u32)` helper (correct `GcFunction::Copy`, not `GXcopy`) + the depth gate (Task 14 Step 3).
3. Masked route placed at the TOP of the `ClipState::Pixmap` branch, BEFORE `intersect_with_current_clip_live` (which triggers the per-op clip readback) — so Task 15's per-copy no-readback assertion holds; non-mask scissors come from `compute_copy_area_scissors`, not the mask-rasterizing intersect (Task 14 Step 4).

Round-6 also independently re-confirmed the round-5 fixes (coordinate-space math, self-overlap `live_src_offset`, rollback at 5 sites, entry prelude, scratch accounting, retired-snapshot drain) all landed correctly.

**Placeholder scan:** No `TODO`/`later`/`todo!()` is ever committed. Deliberate "read the live body and match verbatim" instructions (Task 7 clamp/ticket, Task 13 frame_ticket, Task 14 `src_off`/`dst_target.offset` binding resolution) point at exact line numbers and forbid inventing names; acceptable because the surrounding code is the source of truth and must not be guessed.

**Type consistency:** `MaskedCopyMask` is defined once (Task 7) with all fields incl. `snapshot_id`; the masked op carries NO refresh (the standalone `ClipSnapshotRefresh` op owns refresh). `RecordedMaskedCopyArea` separates `src_image`/`src_old_layout` (live) from `sample_view`/`sample_extent` (sampled). The push-const clip field is `clip_offset` everywhere (GLSL Task 1, struct Task 2, emit Task 6). `copy_offset` semantics (`src_texel = dst_pixel + copy_offset`) are consistent across frag (Task 1), struct (Task 2), and engine (Task 7), including the self-overlap `−dst_rect.offset` rewrite. `snapshot_touch` is a 3-tuple `(layout, ticket, version)` in both Task 12 and `rollback_snapshots`.

## Risks / open items for the implementer

1. **Exactness gate may exclude a format.** Most likely candidate: none expected (8-bit UNORM round-trips), but R8 depth-8 and the depth-24 X byte are the ones to watch. Task 9 is the authority — record actual results and set the Task 14 predicate to match.
2. **`generation` threading into emit (Task 6).** The masked-blit descriptor set must be acquired at the op's generation so `release_up_to` reclaims it correctly. Confirm the in-scope `generation` binding in the close-time emit loop before wiring the dispatch arm. (The standalone refresh op has no descriptor set, so it does not need `generation`.)
3. **Borrow splitting** between `inner.clip_snapshots`, `inner.frame_builder`, and `inner.descriptor_pool_ring` (sibling fields of `RenderEngineInner`) in both `masked_copy_area` and `refresh_clip_snapshot`. Read locals out before mutable borrows.
4. **`compute_copy_area_scissors` extraction (Task 14 Step 5)** must preserve the existing rect behavior bit-for-bit — extract the current logic, don't rewrite it, and keep the old path using the same helper.
5. **Two ops, one frame, ordered only by barriers.** The standalone `ClipSnapshotRefresh` and the `MaskedCopyArea` are separate ops; their correctness relies on (a) the backend appending refresh BEFORE the masked copy, and (b) the masked op recording `mask_old_layout = SHADER_READ` (the snapshot's layout as left by the refresh). Verify the append order in Task 14 and the recorded layout in Task 7.
