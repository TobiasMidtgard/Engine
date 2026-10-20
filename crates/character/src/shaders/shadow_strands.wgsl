// Shadow casting of hair. Requires lib_types + common.wgsl + strand_data.wgsl.
//
// Hairs are drawn at least `MIN_HALF_WIDTH` wide in the shadow map, and a hair keeps only the
// share of its texels its true width would cover (random per texel and frame), so a head of
// hair casts a shadow as dense as its hair, and the shadow filter and temporal anti-aliasing
// turn the dither into soft shade instead of the hard specks a few whole hairs would cast.

struct ShadowView {
    view_proj: mat4x4<f32>,
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;

const MIN_HALF_WIDTH: f32 = 0.0002;

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) coverage: f32,
    @location(1) @interpolate(flat) hair: u32,
};

@vertex
fn vs_strand_shadow(@builtin(vertex_index) vertex: u32, @builtin(instance_index) hair: u32) -> ShadowOut {
    let rp = ribbon_point(hair, vertex);
    let light_dir = normalize((inverse_view_dir(shadow_view.view_proj)));
    let side = normalize(cross(rp.tangent, light_dir) + vec3<f32>(1e-6, 0.0, 0.0));
    let half_width = max(rp.radius, MIN_HALF_WIDTH);
    let rel = rp.rel + side * (half_width * ribbon_side(vertex % 6u));
    var out: ShadowOut;
    var clip = shadow_view.view_proj * vec4<f32>(rel, 1.0);
    clip.z = max(clip.z, 0.0);
    out.clip = clip;
    out.coverage = saturate(rp.radius / half_width);
    out.hair = hair;
    return out;
}

// The light's viewing direction from its view-projection matrix (the third row is depth).
fn inverse_view_dir(m: mat4x4<f32>) -> vec3<f32> {
    return vec3<f32>(m[0][2], m[1][2], m[2][2]) + vec3<f32>(0.0, 1e-6, 0.0);
}

@fragment
fn fs_strand_shadow(in: ShadowOut) {
    let frame_seed = fract(shadow_view.time.x * 7.31) * 512.0;
    if (hash13(vec3<f32>(in.clip.xy, f32(in.hair % 4096u) * 0.37 + frame_seed)) >= in.coverage) {
        discard;
    }
}
