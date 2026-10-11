// Depth-only shadow pass for the standard mesh feature. Requires lib_types (not lib_frame).

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,         // wind velocity xyz
    time: vec4<f32>,         // world seconds
    camera_frac: vec4<f32>,  // camera position modulo 4096 (tiling coordinates)
};

struct Material {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    params: vec4<f32>,
    params2: vec4<f32>,
    flags: vec4<u32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;
@group(1) @binding(0) var<storage, read> materials: array<Material>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) n0: vec4<f32>,
    @location(8) n1: vec4<f32>,
    @location(9) n2: vec4<f32>,
    @location(10) extra: vec4<u32>,
};

fn shadow_wind(rel: vec3<f32>, height01: f32, sway: f32) -> vec3<f32> {
    let w = shadow_view.wind.xyz;
    let speed = length(w);
    let p = rel + shadow_view.camera_frac.xyz;
    let t = shadow_view.time.x;
    let phase = dot(p.xz, vec2(0.13, 0.17));
    let gust = 0.6 + 0.4 * sin(t * 0.7 + phase * 0.3) * sin(t * 1.9 + phase);
    let dir = select(vec3(1.0, 0.0, 0.0), w / max(speed, 1e-3), speed > 0.01);
    let strength = sway * height01 * height01 * min(speed / 10.0, 1.5);
    return dir * strength * gust;
}

@vertex
fn vs_shadow(in: VertexIn) -> @builtin(position) vec4<f32> {
    let p = vec4<f32>(in.position, 1.0);
    var rel = vec3<f32>(dot(in.m0, p), dot(in.m1, p), dot(in.m2, p));
    let sway = materials[in.extra.x].params2.y;
    if (sway > 0.0) {
        let h01 = saturate((in.position.y - bitcast<f32>(in.extra.z)) / max(bitcast<f32>(in.extra.w), 1e-3));
        rel += shadow_wind(rel, h01, sway);
    }
    var clip = shadow_view.view_proj * vec4<f32>(rel, 1.0);
    // Pancake casters behind the near plane onto it so they still cast shadows.
    clip.z = max(clip.z, 0.0);
    return clip;
}
