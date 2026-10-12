// Shadow depth for skinned characters. Requires lib_types (not lib_frame).
//
// Two pipelines use this module:
// - `vs_shadow` (depth only) for opaque surfaces;
// - `vs_shadow_masked` + `fs_shadow_masked` for alpha-masked surfaces (hair cards, lashes),
//   which cut out the same pixels as the scene pass so their shadows have the same holes.

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

// Must match `struct SkinSurface` in skin_draw.wgsl and `GpuSurface` in feature.rs.
struct SkinSurface {
    base_color: vec4<f32>,  // linear RGBA factor
    params: vec4<f32>,      // perceptual roughness, metallic, normal scale, alpha cutoff (0 = none)
    extra: vec4<f32>,       // uv scale xy, subsurface, reflectance
    flags: vec4<u32>,       // x: bit 0 = base color texture present
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;
// The scene pass's surface bind group; only the factors, base color texture and sampler are
// used here.
@group(1) @binding(0) var<uniform> skin_surface: SkinSurface;
@group(1) @binding(1) var base_color_texture: texture_2d<f32>;
@group(1) @binding(4) var surface_sampler: sampler;

struct VertexIn {
    @location(0) position_tension: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
};

fn shadow_clip(in: VertexIn) -> vec4<f32> {
    let p = vec4<f32>(in.position_tension.xyz, 1.0);
    let rel = vec3<f32>(dot(in.m0, p), dot(in.m1, p), dot(in.m2, p));
    var clip = shadow_view.view_proj * vec4<f32>(rel, 1.0);
    // Pancake casters behind the near plane onto it so they still cast shadows.
    clip.z = max(clip.z, 0.0);
    return clip;
}

@vertex
fn vs_shadow(in: VertexIn) -> @builtin(position) vec4<f32> {
    return shadow_clip(in);
}

struct MaskedOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
};

@vertex
fn vs_shadow_masked(in: VertexIn) -> MaskedOut {
    var out: MaskedOut;
    out.clip = shadow_clip(in);
    out.uv = in.uv;
    out.alpha = in.color.a;
    return out;
}

@fragment
fn fs_shadow_masked(in: MaskedOut) {
    let tex = textureSample(base_color_texture, surface_sampler, in.uv * skin_surface.extra.xy);
    var alpha = skin_surface.base_color.a * in.alpha;
    if ((skin_surface.flags.x & 1u) != 0u) {
        alpha *= tex.a;
    }
    if (alpha < skin_surface.params.w) {
        discard;
    }
}
