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

// The last hit `trace_scene` or `trace_reflect` found for a ray that asks what it meets, as
// `ray_whole` or `ray_nearest` gave it: what the shared body's `Hit` has no room for (the
// triangle, where on it, the turn to its mesh). `shade_hit` shades that hit next (`cs_trace`,
// `cs_reflect` trace nothing in between); shadow rays leave it as it is.
var<private> ray_last: RayHit;

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
    let found = ray_whole(origin, dir, t_max, RAY_SEES_SHAPES);
    ray_last = found;
    hit.t = found.t;
    hit.instance = found.row;
    hit.local_n = found.n;
    return hit;
}

// What world reflections ask of a leaf card's triangles: their texture, for the first 4 a ray
// meets, then their coverage; and the ground only at its front.
const REFLECT_RULES: u32 = RULE_GROUND_FRONT | RULE_LEAF_TEXTURE | (4u << RULE_TESTS_SHIFT);

// The nearest hit along a camera-relative ray within `t_max` that a world reflection sees
// (`instance` all ones: none), as `trace_scene` gives a probe's: every class a view draws, of
// any size, leaf cards through the holes of their texture.
fn trace_reflect(origin: vec3<f32>, dir: vec3<f32>, t_max: f32) -> Hit {
    let found = ray_nearest(origin, dir, t_max, RAY_DRAWN, REFLECT_RULES);
    ray_last = found;
    var hit: Hit;
    hit.t = found.t;
    hit.instance = found.row;
    hit.triangle = 0u;
    hit.local_n = found.n;
    return hit;
}

// Light leaving the surface a ray from `origin` along `dir` hit (`hit`, not a miss), toward
// the ray (see `hit_radiance`), and -1 in w for the back of a single-sided surface (inside
// something), else 1. `cone`: the ray's width at `origin` and how fast it widens (meters per
// meter), which pick the level of the hit's texture. The surface is the hit's texture, smooth
// normal and vertex colour where its row has them (`ray_surface`).
fn shade_hit(origin: vec3<f32>, dir: vec3<f32>, hit: Hit, spacing: f32, cone: vec2<f32>) -> vec4<f32> {
    var found = ray_last;
    if (found.row != hit.instance || found.t != hit.t) {
        // (Not the hit `trace_scene` found last: its face alone, as before the records.)
        found.t = hit.t;
        found.row = hit.instance;
        found.n = hit.local_n;
        found.primitive = RAY_NONE;
    }
    let s = ray_surface(found, dir, cone);
    if (s.inside) { return vec4<f32>(0.0, 0.0, 0.0, -1.0); }
    var p = origin + dir * hit.t;
    if (any(s.n != s.face)) {
        // `hit_radiance` lifts its shadow rays along `n`: off the face by as much as along a
        // flat face's normal, however far `n` leans.
        p += s.face * (max(0.002, hit.t * 1e-4) * (1.0 - dot(s.n, s.face)));
    }
    return vec4<f32>(hit_radiance(p, s.n, dir, hit.t, spacing, s.albedo, s.emissive, u32(ray_scene.lights.x)), 1.0);
}
