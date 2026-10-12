// Shadow pass for terrain patches (engine_terrain). Requires lib_types (not lib_frame).

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

// Must match `PlanetGpu` in render.rs (only the rotation is used here).
struct PlanetGpu {
    rot: array<vec4<f32>, 3>,
    center: vec4<f32>,
    colors: array<vec4<f32>, 10>,
    params: vec4<f32>,
    params2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;
@group(1) @binding(0) var<storage, read> planets: array<PlanetGpu>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(5) origin: vec3<f32>,
    @location(6) planet: u32,
};

@vertex
fn vs_shadow(in: VertexIn) -> @builtin(position) vec4<f32> {
    let p = planets[in.planet];
    let rot = mat3x3<f32>(p.rot[0].xyz, p.rot[1].xyz, p.rot[2].xyz);
    let rel = in.origin + rot * in.position;
    var clip = shadow_view.view_proj * vec4<f32>(rel, 1.0);
    // Pancake casters behind the near plane onto it so they still cast shadows.
    clip.z = max(clip.z, 0.0);
    return clip;
}
