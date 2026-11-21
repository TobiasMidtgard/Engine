// Probe GI's rays against the ray scene (lib_ray.wgsl composes before this, probe_gi.wgsl
// after): the probes, world reflections and the sun's rays trace the frame's one ray scene
// (`trial_cut_more::RAY_WORLD`), bound at group 1, so the probes' own buffers are group 2
// here, numbered past the reflection's and the sun's inputs.
@group(2) @binding(10) var<uniform> pass_: GiPass;
@group(2) @binding(15) var<storage, read_write> rays: array<vec4<f32>>;
@group(2) @binding(16) var<storage, read_write> probe_state: array<vec4<i32>>;
@group(2) @binding(17) var atlas_out: texture_storage_2d_array<rgba16float, write>;
// The storage slots updated this frame: the traced ones (probes that scrolled into the grid
// first, then a share of the rest in turn), then the cleared ones.
@group(2) @binding(18) var<storage, read> updates: array<u32>;

// The nearest hit along a camera-relative ray within `t_max` (`instance` all ones: none)
// among what holds its shape and is 0.3 m or more, as the probes' own scene held it: no face
// is culled, every triangle is taken whole. `local_n` is the triangle's normal in the scene's
// space here. `first_hit`: a shadow ray, which only tells what stopped it (`ray_stopped` in
// lib_ray.wgsl; its `t` is the ray's end and its `local_n` zero): it passes what casts no
// shadow and is stopped by a stand-in that casts for such a thing, where it goes into the
// stand-in. (The probes' own scene, which holds no stand-in, stopped it at the thing itself.)
fn trace_scene(origin: vec3<f32>, dir: vec3<f32>, t_max: f32, first_hit: bool) -> Hit {
    var hit: Hit;
    hit.triangle = 0u;
    if (first_hit) {
        hit.t = t_max;
        hit.instance = ray_stopped(origin, dir, t_max);
        hit.local_n = vec3<f32>(0.0);
        return hit;
    }
    let found = ray_whole(origin, dir, t_max, RAY_SEES_SHAPES, false);
    hit.t = found.t;
    hit.instance = found.row;
    hit.local_n = found.n;
    return hit;
}

// Light leaving the surface a ray from `origin` along `dir` hit (`hit`, not a miss), toward
// the ray (see `hit_radiance`), and -1 in w for the back of a single-sided surface (inside
// something), else 1.
fn shade_hit(origin: vec3<f32>, dir: vec3<f32>, hit: Hit, spacing: f32) -> vec4<f32> {
    let row = ray_rows[hit.instance];
    var n = normalize(hit.local_n);
    if (dot(n, dir) > 0.0) {
        if ((row.flags & RAY_ROW_DOUBLE_SIDED) == 0u) {
            return vec4<f32>(0.0, 0.0, 0.0, -1.0);
        }
        n = -n;
    }
    let p = origin + dir * hit.t;
    return vec4<f32>(hit_radiance(p, n, dir, hit.t, spacing, row.albedo.rgb, row.emissive.rgb, u32(ray_scene.lights.x)), 1.0);
}
