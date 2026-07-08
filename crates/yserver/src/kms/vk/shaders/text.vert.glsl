#version 450

#extension GL_EXT_scalar_block_layout : require

// Batched glyph quad vertex shader (#1 glyph draw batching). One
// `vkCmdDraw(4, N_glyphs, ..)` with TRIANGLE_STRIP topology draws the
// whole run: the 4-vertex quad comes from `gl_VertexIndex`, and each
// glyph's destination rect + atlas rect arrive as per-instance vertex
// attributes (VK_VERTEX_INPUT_RATE_INSTANCE). Per-run constants
// (viewport, atlas extent, foreground) stay in push constants.
//
// Atlas coords are in TEXELS; this shader divides by `atlas_extent`
// so recorded instance data is independent of any later atlas resize.
//
// NDC convention matches `composite.vert.glsl`: y increases
// downward; pixel `(0, 0)` lands at NDC `(-1, -1)` (top-left).
//
// `layout(scalar)` packs the push-constant block without std140/std430
// vec4 alignment so the offsets match the host `TextPushConsts`
// `repr(C)` struct directly — no 16-byte pad before `foreground`.

layout(location = 0) in vec2 dst_origin;  // pixels
layout(location = 1) in vec2 dst_size;     // pixels
layout(location = 2) in vec2 atlas_xy;     // texels
layout(location = 3) in vec2 atlas_wh;     // texels

layout(scalar, push_constant) uniform PushConsts {
    vec2 viewport;
    vec2 atlas_extent;  // texels
    vec4 foreground;    // RGB used by fragment shader; alpha is 1.0
} pc;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_foreground;

void main() {
    vec2 quad = vec2(float(gl_VertexIndex & 1), float((gl_VertexIndex >> 1) & 1));

    vec2 dst_pixel = dst_origin + quad * dst_size;
    vec2 ndc = dst_pixel / pc.viewport * 2.0 - 1.0;
    gl_Position = vec4(ndc, 0.0, 1.0);

    v_uv = (atlas_xy + quad * atlas_wh) / pc.atlas_extent;
    v_foreground = pc.foreground;
}
