// Probe GI (probe_gi/mod.rs): DDGI (Majercik et al. 2019) traced against the meshes around the
// grid. `cs_trace` shoots each updated probe's rays and lights what they hit; `cs_update` blends
// them into the probe's irradiance and distance maps, which `probe_gi` in lib_frame.wgsl samples.
// Positions are camera-relative, as everywhere.
//
// What the rays trace comes first, composed before this file, one of three ways
// (`probe_gi::shader_source`). All give the probes' buffers (`pass_`, `rays`, `probe_state`,
// `atlas_out`, `updates`), `trace_scene` and `shade_hit`:
// * the frame's ray scene: lib_ray.wgsl and probe_gi_rays.wgsl (ray queries against the scene
//   every feature fills, ray_scene/mod.rs);
// * the probes' own scene on ray-tracing hardware: probe_gi_trace_hw.wgsl and
//   probe_gi_scene_old.wgsl (ray queries against the structures of probe_gi/rt.rs);
// * the probes' own scene elsewhere: probe_gi_trace_sw.wgsl and probe_gi_scene_old.wgsl (a
//   software walk of the meshes' hierarchies, probe_gi/bvh.rs).
//
// Probes with nothing within `SLEEP_REACH` probe spacings sleep (Majercik et al. 2021, section
// 6): they light no surface, so they trace one turn in `SLEEP_TURNS` and keep their light the
// rest of the time. `probe_state.w` is 1 for an awake probe, 2 + turns slept for a sleeping one.

struct GiPass {
    // probes traced this frame, rays per probe, probes cleared after them (they scrolled in and
    // wait for a trace), probes in the grid
    counts: vec4<u32>,
    // rows of this frame's random rotation of the ray directions
    rotation_x: vec4<f32>,
    rotation_y: vec4<f32>,
    rotation_z: vec4<f32>,
    // hysteresis, distance sharpness, instances, shadowed point lights at most
    params: vec4<f32>,
    // world probe index of the grid's first probe (its lowest corner)
    world_origin: vec4<i32>,
    // the probes' own instances' anchor, camera-relative
    anchor: vec4<f32>,
};

const NO_HIT: f32 = 1e30;
const MAX_RAYS: u32 = 256u;
// A probe sleeps with nothing nearer than this many spacings: the probes around a surface are
// at most sqrt(3) spacings from it.
const SLEEP_REACH: f32 = 2.0;
// A sleeping probe traces one of this many turns.
const SLEEP_TURNS: i32 = 8;

struct Hit {
    t: f32,
    // The instance's row (all ones: the ray hit nothing).
    instance: u32,
    // The triangle (the software walk's index, `hit_normal_local` reads its corners).
    triangle: u32,
    // The triangle's normal, unnormalized: in the mesh's space on the probes' own hardware
    // scene, in the scene's space on the ray scene (the software walk leaves it zero and
    // `hit_normal_local` works it out).
    local_n: vec3<f32>,
};

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

// Whether the probe in storage slot `slot`, at world probe `world`, sleeps through this turn: it
// was asleep there in this epoch and it isn't its turn to trace.
fn sleeps_now(slot: u32, world: vec3<i32>) -> bool {
    let state = probe_state[slot];
    if (any(state.xyz != world) || state.w < 2) { return false; }
    let tile = vec2<u32>(slot % PROBE_GI_TILES_X, slot / PROBE_GI_TILES_X) * PROBE_GI_TEXELS;
    if (abs(textureLoad(gi_atlas, vec2<i32>(tile), 1, 0).w - frame.gi_grid.w) > 0.5) { return false; }
    return (state.w - 2) % SLEEP_TURNS != SLEEP_TURNS - 1;
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
    let slot = updated_slot(probe);
    let g = slot_grid(slot);
    if (sleeps_now(slot, pass_.world_origin.xyz + vec3<i32>(g))) { return; }
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
        // Under a canopy, seen through its leaves, which glow in what they hold back.
        let through = canopy_ray_transmittance(origin, dir);
        let sky = sample_env_rough(dir, 0.6) * through + canopy_leaf_radiance(through) * (1.0 - through);
        rays[id.x] = vec4<f32>(sky * PROBE_GI_SCALE, t_max);
        return;
    }
    // The ray's cone: no width at the probe, then its share of the sphere across (4 pi / rays
    // steradians: 0.44 rad for the default 64), which reads most textures at their mean.
    let shaded = shade_hit(origin, dir, hit, spacing, vec2<f32>(0.0, sqrt(4.0 * PI / f32(rays_per_probe))));
    if (shaded.w < 0.0) {
        // The back of a single-sided surface: inside something. DDGI counts these to find
        // probes stuck in walls.
        rays[id.x] = vec4<f32>(0.0, 0.0, 0.0, -hit.t * 0.2);
        return;
    }
    rays[id.x] = vec4<f32>(probe_ray_firefly(shaded.rgb) * PROBE_GI_SCALE, min(hit.t, t_max));
}

// A ray's radiance at most PROBE_RAY_MOST times white on screen (hue kept). A probe's 64 rays
// can't tell a lamp globe (tens of thousands of cd/m² over a few square centimeters) from a
// wall: the one ray in a few frames that hits it carried a sixty-fourth of the probe's light
// at the globe's brightness, and the ground under the lamps flared and faded from frame to
// frame (12% of the night market's pixels changed by more than 8/255 between two paused
// frames, 11 Nov). The lamp's light already reaches the probes as a point light. The same
// limit as the screen-space bounce's firefly weight (ssgi.wgsl), relative to the exposure.
const PROBE_RAY_MOST: f32 = 8.0;

fn probe_ray_firefly(c: vec3<f32>) -> vec3<f32> {
    let most = PROBE_RAY_MOST / max(frame.exposure.x, 1e-12);
    let l = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    return c * min(1.0, most / max(l, 1e-12));
}

// Light leaving the surface point `p` a ray along `dir` hit at distance `t`, toward the ray:
// radiance (cd/m^2, not pre-exposed). `n`: the surface's normal on the ray's side; `albedo`
// and `emissive`: the instance's. Lit by the sun and the nearest `lights_most` point lights
// through shadow rays, its emissive, and last frame's probes (light bounced more than once).
// `spacing` is the probes' spacing, which sizes how far shadow rays look.
fn hit_radiance(p: vec3<f32>, n: vec3<f32>, dir: vec3<f32>, t: f32, spacing: f32, albedo: vec3<f32>, emissive: vec3<f32>, lights_most: u32) -> vec3<f32> {
    let lift = p + n * max(0.002, t * 1e-4);
    var light = vec3<f32>(0.0);
    // Sun, through a shadow ray, and through the leaves above: leaf cards are not in the
    // probes' scene when the canopy map is on, so without them every hit under the crowns read
    // as sunlit and lit the trunks around it with sunny grass (most of their light, and
    // green). The leaves let through the share the canopy map gives the sun's direction, as
    // the probes' missed rays see the sky.
    let l = frame.sun_dir.xyz;
    let n_dot_l = dot(n, l);
    if (n_dot_l > 0.0 && dot(frame.sun_color.rgb, frame.sun_color.rgb) > 0.0) {
        if (trace_scene(lift, l, spacing * 256.0, true).instance == 0xffffffffu
            && sky_volume_wall(lift, l, spacing * 256.0) == NO_HIT) {
            light += frame.sun_color.rgb * sun_transmittance(p) * cloud_shadow(p) * n_dot_l
                * canopy_ray_transmittance(lift, l);
        }
    }
    // Point lights, the nearest few through shadow rays.
    let lights = min(frame.counts.x, lights_most);
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
    return albedo * INV_PI * light + emissive;
}

var<workgroup> shared_rays: array<vec4<f32>, MAX_RAYS>;
var<workgroup> shared_dirs: array<vec3<f32>, MAX_RAYS>;

// One probe per workgroup, one texel of its maps per thread: blend this frame's rays into them.
@compute @workgroup_size(8, 8, 1)
fn cs_update(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>, @builtin(local_invocation_index) thread: u32) {
    let probe = group.x;
    let rays_per_probe = min(pass_.counts.y, MAX_RAYS);
    let slot = updated_slot(probe);
    if (probe >= pass_.counts.x) {
        // Scrolled in, not traced yet: written empty and in no epoch, so `probe_gi` skips it
        // (fading toward the sky guess by its share) rather than reading the probe that
        // scrolled out of the far side; its first trace starts it fresh.
        let tile = vec2<u32>(slot % PROBE_GI_TILES_X, slot / PROBE_GI_TILES_X) * PROBE_GI_TEXELS;
        let texel = vec2<i32>(tile + local.xy);
        textureStore(atlas_out, texel, 0, vec4<f32>(0.0, 0.0, 0.0, 1.0));
        textureStore(atlas_out, texel, 1, vec4<f32>(0.0, 0.0, 0.0, -1.0));
        if (thread == 0u) {
            let g = slot_grid(slot);
            probe_state[slot] = vec4<i32>(pass_.world_origin.xyz + vec3<i32>(g), 0);
        }
        return;
    }
    for (var r = thread; r < rays_per_probe; r += 64u) {
        shared_rays[r] = rays[probe * pass_.counts.y + r];
        shared_dirs[r] = ray_direction(r, pass_.counts.y);
    }
    workgroupBarrier();

    let g = slot_grid(slot);
    let world = pass_.world_origin.xyz + vec3<i32>(g);
    let state = probe_state[slot];
    if (sleeps_now(slot, world)) {
        // Its maps stay as they are (`atlas_out` still holds them); count the turn.
        if (thread == 0u) {
            probe_state[slot] = vec4<i32>(world, state.w + 1);
        }
        return;
    }
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
    var nearest = NO_HIT;
    let sharpness = pass_.params.y;
    for (var r = 0u; r < rays_per_probe; r++) {
        let ray = shared_rays[r];
        let c = dot(texel_dir, shared_dirs[r]);
        nearest = min(nearest, abs(ray.w));
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
    // An asleep probe traces one turn in SLEEP_TURNS: its trace stands for all of them, so it
    // keeps as much of its old light as an awake probe does over that many turns.
    if (!fresh && all(state.xyz == world) && state.w >= 2) { keep = pow(keep, f32(SLEEP_TURNS)); }
    if (fresh) { keep = 0.0; }
    textureStore(atlas_out, texel, 0, vec4<f32>(mix(irradiance, old_irradiance.rgb, keep), 1.0));
    // w: the epoch it was written in (`probe_gi` ignores probes from another).
    textureStore(atlas_out, texel, 1, vec4<f32>(mix(moments, old_moments.xy, keep), in_use, frame.gi_grid.w));
    if (thread == 0u) {
        // Awake while something is near; a fresh probe stays awake for its first turn.
        let asleep = !fresh && nearest > SLEEP_REACH * spacing;
        probe_state[slot] = vec4<i32>(world, select(1, 2, asleep));
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
// With `CUT_REFLECTIONS_ALTERNATE`: last frame's `reflect_out` and the screen motion (uv,
// current - previous). Half the texels are traced a frame, in a checkerboard that swaps each
// frame; the others take last frame's reflection of their surface point, which was traced then
// while the camera rests.
@group(2) @binding(3) var reflect_last: texture_2d<f32>;
@group(2) @binding(4) var reflect_motion: texture_2d<f32>;

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
    if (trial_cut(CUT_REFLECTIONS_ALTERNATE) && frame.camera_delta.w > 0.5
        && ((id.x + id.y + u32(frame.time.w)) & 1u) == 1u) {
        let prev_uv = uv - textureLoad(reflect_motion, px, 0).xy;
        if (all(prev_uv >= vec2<f32>(0.0)) && all(prev_uv < vec2<f32>(1.0))) {
            // (Texel t was traced from pixel 2t: the texel whose pixel is nearest.)
            let last = vec2<i32>(floor(prev_uv * vec2<f32>(full) * 0.5 + 0.25));
            textureStore(reflect_out, id.xy, textureLoad(reflect_last, min(last, vec2<i32>(out_size) - 1), 0));
            return;
        }
    }
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
        // The traced texel's cone: two pixels across (a half-size texel), a pixel sized at its
        // depth, widening as much per meter (as wide all along when orthographic).
        let pixel = 2.0 / (frame.proj[1][1] * frame.viewport.y);
        let cone = select(vec2<f32>(2.0 * pixel * depth, 2.0 * pixel), vec2<f32>(2.0 * pixel, 0.0), frame.exposure.w > 0.5);
        let shaded = shade_hit(origin, dir, hit, spacing, cone);
        // The back of a single-sided surface: the ray started inside something, so it
        // reflects nothing of its own.
        if (shaded.w > 0.0) {
            out = vec4<f32>(min(shaded.rgb, vec3<f32>(60000.0)), max(hit.t, 1e-3));
        }
    }
    textureStore(reflect_out, id.xy, out);
}

// The sun's rays (`cs_sun`; `RenderSettings::ray_sun_shadows`, a measurement for now): one ray
// toward the sun from every pixel the prepass drew, against the same scene, from the side of
// the surface the sun is on (as the shadow maps are read). `sun_out`.r: 1 where the ray reached
// the sky within `SUN_RAY_REACH`, 0 where something stopped it. Nothing reads it yet. The
// normals and depth are `reflect_normal` and `reflect_depth`, at the view's full size.
@group(2) @binding(5) var sun_out: texture_storage_2d<rgba8unorm, write>;
// Rays shot and rays stopped, counted while `COUNT_SUN_RAYS` is set (cleared before the pass).
@group(2) @binding(6) var<storage, read_write> sun_counts: array<atomic<u32>, 4>;

// How far a ray looks for something between its pixel and the sun, meters.
const SUN_RAY_REACH: f32 = 512.0;
// `shade_skip::COUNT_MESH_RUNS`: the counts of the frame's work are asked for.
const COUNT_SUN_RAYS: u32 = 262144u;

@compute @workgroup_size(8, 8, 1)
fn cs_sun(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(sun_out);
    if (any(id.xy >= size)) { return; }
    let px = vec2<i32>(id.xy);
    let nr = textureLoad(reflect_normal, px, 0);
    let z = textureLoad(reflect_depth, px, 0);
    // The sky, a pixel the prepass gave no normal, or no sun: no ray.
    if (z <= 0.0 || nr.w < 0.5 || frame.sun_color.w < 0.5) {
        textureStore(sun_out, id.xy, vec4<f32>(1.0, 0.0, 0.0, 1.0));
        return;
    }
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(size);
    let pos = view_position(uv, z);
    let rel = (frame.inv_view * vec4<f32>(pos, 1.0)).xyz;
    let n = sunlit_side(oct_decode(nr.xy));
    let origin = rel + n * (0.02 + 0.002 * -pos.z);
    let stopped = trace_scene(origin, frame.sun_dir.xyz, SUN_RAY_REACH, true).instance != 0xffffffffu;
    if (shade_skip(COUNT_SUN_RAYS)) {
        atomicAdd(&sun_counts[0], 1u);
        if (stopped) { atomicAdd(&sun_counts[1], 1u); }
    }
    textureStore(sun_out, id.xy, vec4<f32>(select(1.0, 0.0, stopped), 0.0, 0.0, 1.0));
}
