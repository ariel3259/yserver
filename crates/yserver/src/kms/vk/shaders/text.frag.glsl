#version 450

// Glyph fragment shader (sub-phase 4.1.4.5). Samples the shared
// R8 glyph atlas (alpha-only) and emits the (premultiplied)
// foreground colour modulated by the sampled coverage — the X11
// RENDER glyph composite where the source is a solid-fill of
// `foreground` and the mask is the glyph bitmap:
// `src IN glyph-coverage`, then the pipeline's per-op blend state
// (`StdPictOp::blend_factors`) applies the PictOp against dst.
//
// Output is premultiplied: `(rgb * cov, a * cov)`. For the common
// opaque foreground (`a == 1.0`) this is byte-identical to the
// historical `(rgb * cov, cov)` Over-only output.
//
// A8_DST — 1 → replicate the computed alpha across all channels so
// an R8_UNORM attachment (a8 mask pixmap — the cairo/Pango
// component-alpha text intermediate) stores alpha in `.r`. Same
// convention as render.frag.glsl's A8_DST constant.

layout(constant_id = 0) const uint A8_DST = 0u;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_foreground;

layout(location = 0) out vec4 out_color;

layout(set = 0, binding = 0) uniform sampler2D atlas;

void main() {
    float coverage = texture(atlas, v_uv).r;
    float alpha = v_foreground.a * coverage;
    if (A8_DST == 1u) {
        out_color = vec4(alpha);
    } else {
        out_color = vec4(v_foreground.rgb * coverage, alpha);
    }
}
