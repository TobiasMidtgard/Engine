// Probe GI (probe_gi/mod.rs): DDGI (Majercik et al. 2019) traced in software against the
// meshes' hierarchies (probe_gi/bvh.rs). `cs_trace` shoots each updated probe's rays and lights
// what they hit; `cs_update` blends them into the probe's irradiance and distance maps, which
// `probe_gi` in lib_frame.wgsl samples. Positions are camera-relative, as everywhere.

struct GiPass {
    // probes updated this frame, rays per probe, unused, probes in the grid
    counts: vec4<u32>,
    // rows of this frame's random rotation of the ray directions
    rotation_x: vec4<f32>,
    rotation_y: vec4<f32>,
    rotation_z: vec4<f32>,
    // hysteresis, distance sharpness, instances, shadowed point lights at most
    params: vec4<f32>,
    // world probe index of the grid's first probe (its lowest corner)
    world_origin: vec4<i32>,
};

struct BvhNode {
    min: vec3<f32>,
    right_or_first: u32,
    max: vec3<f32>,
    count: u32,
};

struct Instance {
    // rows of the camera-relative -> mesh transform
    to_local_x: vec4<f32>,
    to_local_y: vec4<f32>,
    to_local_z: vec4<f32>,
    // the mesh's first node and first triangle, flags (1: double sided), unused
    mesh: vec4<u32>,
    albedo: vec4<f32>,
    emissive: vec4<f32>,
};

@group(1) @binding(0) var<uniform> pass_: GiPass;
@group(1) @binding(1) var<storage, read> tlas: array<BvhNode>;
@group(1) @binding(2) var<storage, read> instances: array<Instance>;
@group(1) @binding(3) var<storage, read> blas: array<BvhNode>;
@group(1) @binding(4) var<storage, read> triangles: array<vec4<f32>>;
@group(1) @binding(5) var<storage, read_write> rays: array<vec4<f32>>;
@group(1) @binding(6) var<storage, read_write> probe_state: array<vec4<i32>>;
@group(1) @binding(7) var atlas_out: texture_storage_2d_array<rgba16float, write>;
// The storage slots updated this frame: probes that just scrolled into the grid, then a share
// of the rest in turn.
@group(1) @binding(8) var<storage, read> updates: array<u32>;

const STACK: u32 = 48u;
const NO_HIT: f32 = 1e30;
const MAX_RAYS: u32 = 256u;

struct Hit {
    t: f32,
    instance: u32,
    triangle: u32,
};

fn safe_inverse(d: vec3<f32>) -> vec3<f32> {
    let tiny = select(vec3<f32>(-1e-20), vec3<f32>(1e-20), d >= vec3<f32>(0.0));
    return 1.0 / select(d, tiny, abs(d) < vec3<f32>(1e-20));
}

// Where the ray enters the box within `t_max`, or NO_HIT.
fn ray_box(origin: vec3<f32>, inv: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>, t_max: f32) -> f32 {
    let t0 = (lo - origin) * inv;
    let t1 = (hi - origin) * inv;
    let near = max(max(min(t0.x, t1.x), min(t0.y, t1.y)), max(min(t0.z, t1.z), 0.0));
    let far = min(min(max(t0.x, t1.x), max(t0.y, t1.y)), min(max(t0.z, t1.z), t_max));
    return select(NO_HIT, near, near <= far);
}

// Möller-Trumbore: the distance to triangle `t` along the ray, or NO_HIT.
fn ray_triangle(origin: vec3<f32>, dir: vec3<f32>, t: u32) -> f32 {
    let a = triangles[t * 3u].xyz;
    let e1 = triangles[t * 3u + 1u].xyz - a;
    let e2 = triangles[t * 3u + 2u].xyz - a;
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if (abs(det) < 1e-14) { return NO_HIT; }
    let inv_det = 1.0 / det;
    let s = origin - a;
    let u = dot(s, p) * inv_det;
    if (u < 0.0 || u > 1.0) { return NO_HIT; }
    let q = cross(s, e1);
    let v = dot(dir, q) * inv_det;
    if (v < 0.0 || u + v > 1.0) { return NO_HIT; }
    let d = dot(e2, q) * inv_det;
    return select(NO_HIT, d, d > 0.0);
}

// The nearest triangle of instance `i` along a camera-relative ray, if nearer than `best`.
// `first_hit`: stop at the first hit (shadow rays).
fn trace_instance(i: u32, origin_rel: vec3<f32>, dir_rel: vec3<f32>, first_hit: bool, best: ptr<function, Hit>) {
    let inst = instances[i];
    let o4 = vec4<f32>(origin_rel, 1.0);
    let d4 = vec4<f32>(dir_rel, 0.0);
    // Affine, so the ray's parameter is the same in the mesh's space.
    let origin = vec3<f32>(dot(inst.to_local_x, o4), dot(inst.to_local_y, o4), dot(inst.to_local_z, o4));
    let dir = vec3<f32>(dot(inst.to_local_x, d4), dot(inst.to_local_y, d4), dot(inst.to_local_z, d4));
    let inv = safe_inverse(dir);
    let root = inst.mesh.x;
    var stack: array<u32, STACK>;
    var top = 1u;
    stack[0] = root;
    loop {
        if (top == 0u) { break; }
        top -= 1u;
        let index = stack[top];
        let node = blas[index];
        if (ray_box(origin, inv, node.min, node.max, (*best).t) >= NO_HIT) { continue; }
        if (node.count > 0u) {
            for (var k = 0u; k < node.count; k++) {
                let tri = inst.mesh.y + node.right_or_first + k;
                let d = ray_triangle(origin, dir, tri);
                if (d < (*best).t) {
                    (*best).t = d;
                    (*best).instance = i;
                    (*best).triangle = tri;
                    if (first_hit) { return; }
                }
            }
        } else if (top + 2u <= STACK) {
            // Children: the left one right after its parent, the right one where it says.
            stack[top] = root + node.right_or_first;
            stack[top + 1u] = index + 1u;
            top += 2u;
        }
    }
}

// The nearest hit along a camera-relative ray within `t_max` (`instance` all ones: none).
fn trace_scene(origin: vec3<f32>, dir: vec3<f32>, t_max: f32, first_hit: bool) -> Hit {
    var best: Hit;
    best.t = t_max;
    best.instance = 0xffffffffu;
    best.triangle = 0u;
    if (u32(pass_.params.z) == 0u) { return best; }
    let inv = safe_inverse(dir);
    var stack: array<u32, STACK>;
    var top = 1u;
    stack[0] = 0u;
    loop {
        if (top == 0u) { break; }
        top -= 1u;
        let index = stack[top];
        let node = tlas[index];
        if (ray_box(origin, inv, node.min, node.max, best.t) >= NO_HIT) { continue; }
        if (node.count > 0u) {
            for (var k = 0u; k < node.count; k++) {
                trace_instance(node.right_or_first + k, origin, dir, first_hit, &best);
                if (first_hit && best.instance != 0xffffffffu) { return best; }
            }
        } else if (top + 2u <= STACK) {
            stack[top] = node.right_or_first;
            stack[top + 1u] = index + 1u;
            top += 2u;
        }
    }
    return best;
}

fn visible(start: vec3<f32>, end: vec3<f32>) -> bool {
    let d = end - start;
    let len = length(d);
    if (len < 1e-4) { return true; }
    return trace_scene(start, d / len, len, true).instance == 0xffffffffu;
}

// Albedo of the walls the indoor volumes stand for (rock, plaster): what a ray stopped by one
// sees, lit by last frame's probes.
const VOLUME_WALL_ALBEDO: f32 = 0.3;

// Ray `r` of `n`: spherical Fibonacci directions, turned by this frame's rotation.
fn ray_direction(r: u32, n: u32) -> vec3<f32> {
    let i = f32(r) + 0.5;
    let phi = 6.28318530718 * fract(i * 0.61803398875);
    let cos_t = 1.0 - 2.0 * i / f32(n);
    let sin_t = sqrt(saturate(1.0 - cos_t * cos_t));
    let d = vec3<f32>(cos(phi) * sin_t, sin(phi) * sin_t, cos_t);
    return normalize(vec3<f32>(dot(pass_.rotation_x.xyz, d), dot(pass_.rotation_y.xyz, d), dot(pass_.rotation_z.xyz, d)));
}

// The grid coordinate (0..size) of storage slot `slot`.
fn slot_grid(slot: u32) -> vec3<u32> {
    let size = vec3<u32>(frame.gi_size.xyz);
    let c = vec3<u32>(slot % size.x, (slot / size.x) % size.y, slot / (size.x * size.y));
    let offset = vec3<u32>(frame.gi_grid.xyz);
    return (c + size - offset) % size;
}

fn updated_slot(probe: u32) -> u32 {
    return updates[probe] % pass_.counts.w;
}

// What a probe ray sees: radiance (not pre-exposed, scaled by PROBE_GI_SCALE), and the distance
// to what it hit (negative for the back of a single-sided surface).
@compute @workgroup_size(64, 1, 1)
fn cs_trace(@builtin(global_invocation_id) id: vec3<u32>) {
    let rays_per_probe = pass_.counts.y;
    let total = pass_.counts.x * rays_per_probe;
    if (id.x >= total) { return; }
    let probe = id.x / rays_per_probe;
    let r = id.x % rays_per_probe;
    let g = slot_grid(updated_slot(probe));
    let origin = probe_gi_position(vec3<f32>(g));
    let dir = ray_direction(r, rays_per_probe);
    let spacing = frame.gi_volume.w;
    let t_max = spacing * 64.0;
    let hit = trace_scene(origin, dir, t_max, false);
    let wall = sky_volume_wall(origin, dir, min(hit.t, t_max));
    if (wall < NO_HIT) {
        // A wall of an indoor volume, before any mesh: lit by the probes around it.
        let p = origin + dir * wall;
        let indirect = probe_gi(p, -dir, -dir);
        let radiance = VOLUME_WALL_ALBEDO * INV_PI * indirect.rgb * indirect.a;
        rays[id.x] = vec4<f32>(radiance * PROBE_GI_SCALE, wall);
        return;
    }
    if (hit.instance == 0xffffffffu) {
        // The sky and the ground beyond, blurred so the sun's disc doesn't flicker in.
        let sky = sample_env_rough(dir, 0.6);
        rays[id.x] = vec4<f32>(sky * PROBE_GI_SCALE, t_max);
        return;
    }
    let shaded = shade_hit(origin, dir, hit, spacing);
    if (shaded.w < 0.0) {
        // The back of a single-sided surface: inside something. DDGI counts these to find
        // probes stuck in walls.
        rays[id.x] = vec4<f32>(0.0, 0.0, 0.0, -hit.t * 0.2);
        return;
    }
    rays[id.x] = vec4<f32>(shaded.rgb * PROBE_GI_SCALE, min(hit.t, t_max));
}

// Light leaving the surface a ray from `origin` along `dir` hit (`hit`, not a miss), toward the
// ray: radiance (cd/m^2, not pre-exposed), and -1 in w for the back of a single-sided surface
// (inside something), else 1. Lit by the sun and the nearest point lights through shadow rays,
// its emissive, and last frame's probes (light bounced more than once). `spacing` is the
// probes' spacing, which sizes how far shadow rays look.
fn shade_hit(origin: vec3<f32>, dir: vec3<f32>, hit: Hit, spacing: f32) -> vec4<f32> {
    let inst = instances[hit.instance];
    let a = triangles[hit.triangle * 3u].xyz;
    let local_n = cross(triangles[hit.triangle * 3u + 1u].xyz - a, triangles[hit.triangle * 3u + 2u].xyz - a);
    // Normals go through the transposed inverse: the rows of camera-relative -> mesh.
    var n = normalize(local_n.x * inst.to_local_x.xyz + local_n.y * inst.to_local_y.xyz + local_n.z * inst.to_local_z.xyz);
    if (dot(n, dir) > 0.0) {
        if ((inst.mesh.z & 1u) == 0u) {
            return vec4<f32>(0.0, 0.0, 0.0, -1.0);
        }
        n = -n;
    }
    let p = origin + dir * hit.t;
    let lift = p + n * max(0.002, hit.t * 1e-4);
    var light = vec3<f32>(0.0);
    // Sun, through a shadow ray.
    let l = frame.sun_dir.xyz;
    let n_dot_l = dot(n, l);
    if (n_dot_l > 0.0 && dot(frame.sun_color.rgb, frame.sun_color.rgb) > 0.0) {
        if (trace_scene(lift, l, spacing * 256.0, true).instance == 0xffffffffu
            && sky_volume_wall(lift, l, spacing * 256.0) == NO_HIT) {
            light += frame.sun_color.rgb * sun_transmittance(p) * cloud_shadow(p) * n_dot_l;
        }
    }
    // Point lights, the nearest few through shadow rays.
    let lights = min(frame.counts.x, u32(pass_.params.w));
    for (var i = 0u; i < lights; i++) {
        let pl = point_lights[i];
        let to_light = pl.position_range.xyz - p;
        let dist2 = dot(to_light, to_light);
        let range = pl.position_range.w;
        if (dist2 > range * range) { continue; }
        let nl = dot(n, to_light) * inverseSqrt(max(dist2, 1e-8));
        if (nl <= 0.0) { continue; }
        if (!visible(lift, pl.position_range.xyz)) { continue; }
        let falloff = sq(saturate(1.0 - sq(dist2 / (range * range)))) / max(dist2, sq(pl.color_radius.w));
        light += pl.color_radius.rgb * falloff * nl;
    }
    // Last frame's probes: light bounced more than once.
    let indirect = probe_gi(p, n, -dir);
    light += indirect.rgb * indirect.a;
    return vec4<f32>(inst.albedo.rgb * INV_PI * light + inst.emissive.rgb, 1.0);
}

var<workgroup> shared_rays: array<vec4<f32>, MAX_RAYS>;
var<workgroup> shared_dirs: array<vec3<f32>, MAX_RAYS>;

// One probe per workgroup, one texel of its maps per thread: blend this frame's rays into them.
@compute @workgroup_size(8, 8, 1)
fn cs_update(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>, @builtin(local_invocation_index) thread: u32) {
    let probe = group.x;
    let rays_per_probe = min(pass_.counts.y, MAX_RAYS);
    let slot = updated_slot(probe);
    for (var r = thread; r < rays_per_probe; r += 64u) {
        shared_rays[r] = rays[probe * pass_.counts.y + r];
        shared_dirs[r] = ray_direction(r, pass_.counts.y);
    }
    workgroupBarrier();

    let g = slot_grid(slot);
    let world = pass_.world_origin.xyz + vec3<i32>(g);
    let state = probe_state[slot];
    let tile = vec2<u32>(slot % PROBE_GI_TILES_X, slot / PROBE_GI_TILES_X) * PROBE_GI_TEXELS;
    let texel = vec2<i32>(tile + local.xy);
    let old_irradiance = textureLoad(gi_atlas, texel, 0, 0);
    let old_moments = textureLoad(gi_atlas, texel, 1, 0);
    // A probe new to its slot, or written before every probe started over, has no history.
    let fresh = any(state.xyz != world) || state.w == 0 || abs(old_moments.w - frame.gi_grid.w) > 0.5;
    let spacing = frame.gi_volume.w;
    // Distances the visibility test cares about: about a probe's reach.
    let reach = spacing * 1.5;

    let texel_dir = oct_decode((vec2<f32>(local.xy) + 0.5) / f32(PROBE_GI_TEXELS) * 2.0 - 1.0);
    var irradiance = vec3<f32>(0.0);
    var irradiance_w = 0.0;
    var moments = vec2<f32>(0.0);
    var moments_w = 0.0;
    var backfaces = 0u;
    let sharpness = pass_.params.y;
    for (var r = 0u; r < rays_per_probe; r++) {
        let ray = shared_rays[r];
        let c = dot(texel_dir, shared_dirs[r]);
        if (ray.w < 0.0) { backfaces += 1u; }
        if (c <= 0.0) { continue; }
        irradiance += ray.rgb * c;
        irradiance_w += c;
        let d = min(abs(ray.w), reach);
        let w = pow(c, sharpness);
        moments += vec2<f32>(d, d * d) * w;
        moments_w += w;
    }
    irradiance /= max(irradiance_w, 1e-6);
    moments /= max(moments_w, 1e-6);
    // A probe whose rays mostly see the inside of something is stuck in it: it lights nothing.
    let in_use = select(1.0, 0.0, f32(backfaces) > 0.25 * f32(rays_per_probe));

    var keep = pass_.params.x;
    if (fresh) { keep = 0.0; }
    textureStore(atlas_out, texel, 0, vec4<f32>(mix(irradiance, old_irradiance.rgb, keep), 1.0));
    // w: the epoch it was written in (`probe_gi` ignores probes from another).
    textureStore(atlas_out, texel, 1, vec4<f32>(mix(moments, old_moments.xy, keep), in_use, frame.gi_grid.w));
    if (thread == 0u) {
        probe_state[slot] = vec4<i32>(world, 1);
    }
}

// World reflections (`cs_reflect`): what screen-space reflections can't see (off screen, or
// hidden behind something), traced against the same scene as the probes. One ray a texel at
// half resolution, from the top-left pixel of each 2x2 block, along the reflection lobe's
// dominant direction; ssr.wgsl blurs it over the lobe's footprint and blends it under its own
// reflection.
@group(2) @binding(0) var reflect_normal: texture_2d<f32>; // prepass normals (octahedral xy, roughness, written)
@group(2) @binding(1) var reflect_depth: texture_depth_2d; // prepass depth (reverse-Z)
// Reflected radiance (cd/m^2, not pre-exposed) and the distance to what the ray hit; 0 where
// it hit nothing (the sky, which `shade_surface` reflects already) or wasn't traced.
@group(2) @binding(2) var reflect_out: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_reflect(@builtin(global_invocation_id) id: vec3<u32>) {
    let out_size = textureDimensions(reflect_out);
    if (any(id.xy >= out_size)) { return; }
    let full = vec2<i32>(textureDimensions(reflect_depth));
    let px = min(vec2<i32>(id.xy) * 2, full - 1);
    let nr = textureLoad(reflect_normal, px, 0);
    let z = textureLoad(reflect_depth, px, 0);
    if (z <= 0.0 || nr.w < 0.5 || nr.z > frame.reflections.z) {
        textureStore(reflect_out, id.xy, vec4<f32>(0.0));
        return;
    }
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(full);
    let pos = view_position(uv, z);
    let rel = (frame.inv_view * vec4<f32>(pos, 1.0)).xyz;
    let n = oct_decode(nr.xy);
    let alpha = nr.z * nr.z;
    let v = normalize(-rel);
    // The lobe's dominant direction, as `shade_surface` and ssr.wgsl reflect.
    let r = reflect(-v, n);
    let dir = normalize(mix(n, r, (1.0 - alpha) * (sqrt(1.0 - alpha) + alpha)));
    let depth = -pos.z;
    let origin = rel + n * (0.02 + 0.002 * depth);
    let spacing = frame.gi_volume.w;
    let t_max = spacing * 64.0;
    let hit = trace_scene(origin, dir, t_max, false);
    let wall = sky_volume_wall(origin, dir, min(hit.t, t_max));
    var out = vec4<f32>(0.0);
    if (wall < NO_HIT) {
        // A wall of an indoor volume (a cave's rock): lit by the probes around it.
        let p = origin + dir * wall;
        let indirect = probe_gi(p, -dir, -dir);
        out = vec4<f32>(VOLUME_WALL_ALBEDO * INV_PI * indirect.rgb * indirect.a, max(wall, 1e-3));
    } else if (hit.instance != 0xffffffffu) {
        let shaded = shade_hit(origin, dir, hit, spacing);
        // The back of a single-sided surface: the ray started inside something, so it
        // reflects nothing of its own.
        if (shaded.w > 0.0) {
            out = vec4<f32>(min(shaded.rgb, vec3<f32>(60000.0)), max(hit.t, 1e-3));
        }
    }
    textureStore(reflect_out, id.xy, out);
}
