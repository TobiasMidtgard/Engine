// Performance overlay: screen-space quads in clip space, tinted by the 8x8 font atlas (R8;
// cell 0 is solid, for rectangles). Drawn onto the final image, so colors are display values.

@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;

struct OverlayIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct OverlayOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_overlay(in: OverlayIn) -> OverlayOut {
    var out: OverlayOut;
    out.clip = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@fragment
fn fs_overlay(in: OverlayOut) -> @location(0) vec4<f32> {
    let ink = textureSample(atlas, atlas_sampler, in.uv).r;
    return vec4<f32>(max(in.color.rgb, vec3<f32>(0.0)), clamp(in.color.a * ink, 0.0, 1.0));
}
