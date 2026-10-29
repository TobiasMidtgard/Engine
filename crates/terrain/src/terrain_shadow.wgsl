// Shadow pass for terrain patches (engine_terrain). Requires lib_types (not lib_frame).

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

// Must match `PlanetGpu` in render.rs field for field, or every planet after the first
// reads another's rotation (only the rotation is used here).
struct PlanetGpu {
    rot: array<vec4<f32>, 3>,
    center: vec4<f32>,
    colors: array<vec4<f32>, 10>,
    params: vec4<f32>,
    params2: vec4<f32>,
    motion: vec4<f32>,
    params3: vec4<f32>,
    rays: array<vec4<f32>, 8>,
    young: array<vec4<f32>, 12>,
    params4: vec4<f32>,
    far: vec4<f32>,                 // far look: map layer (−1 none), near tint, unused ×2
    far_mean: vec4<f32>,            // far look: mean linear albedo, unused
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;
@group(1) @binding(0) var<storage, read> planets: array<PlanetGpu>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec4<f32>,
    @location(5) origin: vec3<f32>,
    @location(6) planet: u32,
    @location(7) lod: vec4<f32>,
};

@vertex
fn vs_shadow(in: VertexIn) -> @builtin(position) vec4<f32> {
    let p = planets[in.planet];
    let rot = mat3x3<f32>(p.rot[0].xyz, p.rot[1].xyz, p.rot[2].xyz);
    var rel = in.origin + rot * in.position;
    // The same geomorph as the main pass (see terrain.wgsl), so shadows match the ground.
    if (in.lod.w > 0.0) {
        var up = p.rot[1].xyz;
        if (p.params2.x > 0.5) {
            up = normalize(rel - p.center.xyz);
        }
        let k = smoothstep(in.lod.z, in.lod.w, length(rel));
        rel += up * (in.normal.w * in.lod.x * 16.0 * k);
    }
    var clip = shadow_view.view_proj * vec4<f32>(rel, 1.0);
    // Pancake casters behind the near plane onto it so they still cast shadows.
    clip.z = max(clip.z, 0.0);
    return clip;
}
