// Probe GI's own scene (probe_gi.wgsl composes after this, the rays' walk before it): the
// meshes around the grid as the probes gather them themselves, for the software walk
// (probe_gi_trace_sw.wgsl, GPUs without ray queries) and for the hardware walk of before the
// ray scene (probe_gi_trace_hw.wgsl, while `trial_cut_more::RAY_WORLD` is off). The instances
// are relative to an anchor that moves in 64 m steps (`GiPass::anchor`), so they stay put
// while the camera moves.
struct Instance {
    // rows of the anchor-relative -> mesh transform
    to_local_x: vec4<f32>,
    to_local_y: vec4<f32>,
    to_local_z: vec4<f32>,
    // the software walk's first node and first triangle of the mesh, flags (1: double sided),
    // the mesh's index
    mesh: vec4<u32>,
    albedo: vec4<f32>,
    emissive: vec4<f32>,
};

@group(1) @binding(0) var<uniform> pass_: GiPass;
@group(1) @binding(2) var<storage, read> instances: array<Instance>;
@group(1) @binding(5) var<storage, read_write> rays: array<vec4<f32>>;
@group(1) @binding(6) var<storage, read_write> probe_state: array<vec4<i32>>;
@group(1) @binding(7) var atlas_out: texture_storage_2d_array<rgba16float, write>;
// The storage slots updated this frame: the traced ones (probes that scrolled into the grid
// first, then a share of the rest in turn), then the cleared ones.
@group(1) @binding(8) var<storage, read> updates: array<u32>;

// A world reflection's ray that meets nothing may still pass what this scene lacks (it holds
// the meshes near the probes, not all that the sky cover map does): the map's march dims it.
const REFLECT_MISS_SEES_SKY: bool = false;

// A world reflection's ray: what a probe's sees (this scene holds no leaf card and no stand-in).
fn trace_reflect(origin: vec3<f32>, dir: vec3<f32>, t_max: f32) -> Hit {
    return trace_scene(origin, dir, t_max, false);
}

// Whether something stops a sun's measuring ray (`cs_sun`): a hit's shadow ray in this scene.
fn trace_sun(origin: vec3<f32>, dir: vec3<f32>, t_max: f32) -> bool {
    return trace_scene(origin, dir, t_max, true).instance != 0xffffffffu;
}

// Light leaving the surface a ray from `origin` along `dir` hit (`hit`, not a miss), toward
// the ray (see `hit_radiance`), and -1 in w for the back of a single-sided surface (inside
// something), else 1. `cone` (the ray's width where it started, and its widening) picks a
// texture's level where a hit has one; this scene's hits have none.
fn shade_hit(origin: vec3<f32>, dir: vec3<f32>, hit: Hit, spacing: f32, cone: vec2<f32>) -> vec4<f32> {
    let inst = instances[hit.instance];
    let local_n = hit_normal_local(hit);
    // Normals go through the transposed inverse: the rows of camera-relative -> mesh.
    var n = normalize(local_n.x * inst.to_local_x.xyz + local_n.y * inst.to_local_y.xyz + local_n.z * inst.to_local_z.xyz);
    if (dot(n, dir) > 0.0) {
        if ((inst.mesh.z & 1u) == 0u) {
            return vec4<f32>(0.0, 0.0, 0.0, -1.0);
        }
        n = -n;
    }
    let p = origin + dir * hit.t;
    return vec4<f32>(hit_radiance(p, n, dir, hit.t, spacing, inst.albedo.rgb, inst.emissive.rgb, u32(pass_.params.w)), 1.0);
}
