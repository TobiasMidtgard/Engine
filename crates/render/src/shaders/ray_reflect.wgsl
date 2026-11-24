// Every pixel's reflection (`trial_cut_more::RAY_REFLECTIONS`, probe_gi/mod.rs
// `dispatch_reflections`): one reflection for each prepass pixel up to the roughness limit
// (`frame.reflections.z`), in the frame's ray pass after the sun's trace. Composed after
// probe_gi.wgsl on the frame's ray scene (lib_ray.wgsl, probe_gi_rays.wgsl), so a ray's hit is
// lit as a world reflection's is (`hit_radiance`).
//
// Each pixel's reflection leaves along the lobe's dominant direction, as ssr.wgsl and
// `cs_reflect` reflect, kept above the face the prepass drew (a normal map's bend can turn it
// into the surface). The screen first: the march of ssr.wgsl `trace` through the prepass's
// depth (64 steps, 6 to refine); a surface it meets that faces the reflection is what the
// camera saw there last frame, found through the motion vectors (within the screen's outer
// 8%, where last frame's picture ends, mixed with the ray's answer). Otherwise a ray through
// the ray scene (`trace_reflect`), as long as the roughness lets a reflection be told apart
// (`min(384, 400 exp(-5 roughness))` m: a rough reflection's blur hides what is far). A hit the
// camera sees now (it projects onto the screen where the prepass drew the same surface) is
// taken from last frame's picture too, mixed with its own light over the screen's outer 8%;
// any other hit is lit as a world reflection's: its tile, the sun through a shadow ray and the
// leaves above, the two nearest lamps through shadow rays up to roughness 0.3 (none above),
// last frame's probes. A ray that meets nothing past where the march passed over the sky (a
// window, a gap in the trees) sees the sky. ssr.wgsl `fs_reflect_resolve` blurs the result over
// each pixel's lobe.
//
// Last frame's picture exists from the second frame of a view on (`frame.reflections.y`, its
// pre-exposure, is 0 before): until then every reflection is a ray's, lit by its own light.

@group(2) @binding(20) var full_normal: texture_2d<f32>; // prepass normals (octahedral xy, roughness, written)
@group(2) @binding(21) var full_depth: texture_depth_2d; // prepass depth (reverse-Z)
@group(2) @binding(22) var full_motion: texture_2d<f32>; // screen motion (uv, current - previous, without the jitter)
// Last frame's finished picture, pre-exposed by `frame.reflections.y` (the output's size,
// which temporal upscaling makes larger than the view's).
@group(2) @binding(23) var full_picture: texture_2d<f32>;
// Reflected radiance (cd/m^2, not pre-exposed, at most 60000) and the distance to what it met:
// 0 where nothing was met within a shortened ray's reach (what lies past it is the sky cover
// map's to tell) or nothing was traced (the sky, a pixel rougher than the limit), -1 where
// nothing was met within the reach of the cover map's march (open sky; ssr.wgsl tells
// `shade_surface`). The view's size.
@group(2) @binding(24) var full_out: texture_storage_2d<rgba16float, write>;
// Counted while `COUNT_REFLECTIONS` is set (cleared before the pass): the pixels under the
// roughness limit, those the screen answered, the rays traced, and the rays' hits taken from
// last frame's picture.
@group(2) @binding(25) var<storage, read_write> full_counts: array<atomic<u32>, 4>;

const FULL_MARCH_STEPS: i32 = 64;
const FULL_REFINE_STEPS: i32 = 6;
// `shade_skip::COUNT_MESH_RUNS`: the counts of the frame's work are asked for.
const COUNT_REFLECTIONS: u32 = 262144u;
// The lamps a ray's hit asks for their shadow, at most, and the roughest pixel whose hits ask
// at all (a rougher reflection blurs a lamp's light away; the probes carry it).
const FULL_LAMPS_MOST: u32 = 2u;
const FULL_LAMPS_ROUGHNESS: f32 = 0.3;
// The screen's outer share over which a hit the camera sees mixes into its own light.
const FULL_SCREEN_EDGE: f32 = 0.08;
// How far above the face the prepass drew a reflection is kept (the cosine to the face).
const FULL_ABOVE_FACE: f32 = 0.05;

var<workgroup> full_group_counts: array<atomic<u32>, 4>;

// The march's answer: where on the screen it met a surface, or whether it passed over the sky
// before it went behind anything (`escaped`: it left through open space the camera sees, a
// window or a gap in the trees, `dist` meters from its start), as ssr.wgsl `trace` tells.
struct FullMarch {
    uv: vec2<f32>,
    found: bool,
    escaped: bool,
    dist: f32,
};

// Screen uv and view depth (positive, meters) of view-space point `p` (ssr.wgsl `project`).
fn full_project(p: vec3<f32>) -> vec3<f32> {
    let c = frame.proj * vec4<f32>(p, 1.0);
    let ndc = c.xy / c.w;
    return vec3<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, -p.z);
}

// View depth of the scene at `uv` (0 where nothing was drawn).
fn full_scene_depth(uv: vec2<f32>) -> f32 {
    let size = vec2<i32>(textureDimensions(full_depth));
    let px = clamp(vec2<i32>(floor(uv * frame.viewport.xy)), vec2<i32>(0), size - 1);
    let z = textureLoad(full_depth, px, 0);
    if (z <= 0.0) { return 0.0; }
    return scene_linear_depth(uv, z);
}

// The march of ssr.wgsl `trace` from view-space `origin` along `dir` (unit) for up to
// `max_len` meters: evenly in pixels, perspective-correct depth, the first place the ray passes
// behind a surface by less than its thickness, refined between that step and the one before.
fn full_march(origin: vec3<f32>, dir: vec3<f32>, max_len: f32, jitter: f32) -> FullMarch {
    var hit: FullMarch;
    hit.found = false;
    hit.escaped = false;
    // Keep the end in front of the near plane.
    var len = max_len;
    if (dir.z > 0.0) {
        len = min(len, (-frame.exposure.z * 1.5 - origin.z) / dir.z);
    }
    if (len <= 0.0) { return hit; }
    let a = full_project(origin);
    let b = full_project(origin + dir * len);
    let inv_a = 1.0 / a.z;
    let inv_b = 1.0 / b.z;
    let pixels = length((b.xy - a.xy) * frame.viewport.xy);
    let steps = clamp(i32(pixels), 1, FULL_MARCH_STEPS);
    let dk = 1.0 / f32(steps);
    var prev_k = 0.0;
    var k = dk * (0.5 + jitter);
    var hidden = false;
    var open_k = -1.0;
    for (var i = 0; i < steps; i++) {
        let uv = mix(a.xy, b.xy, k);
        if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) { return hit; }
        let ray = 1.0 / mix(inv_a, inv_b, k);
        let scene = full_scene_depth(uv);
        let step_span = abs(1.0 / mix(inv_a, inv_b, min(k + dk, 1.0)) - ray);
        let thickness = max(0.1 + 0.02 * scene, step_span * 1.5);
        // Over the sky with nothing in the way so far, the ray is in open space the camera sees
        // through to the sky; behind something from there on, it is beyond what that hides (a
        // window's frame and wall seen from inside), still out in the open (ssr.wgsl `trace`).
        if (scene <= 0.0 && !hidden && open_k < 0.0) { open_k = k; }
        hidden = hidden || (scene > 0.0 && ray > scene);
        if (scene > 0.0 && ray > scene && ray - scene < thickness) {
            var lo = prev_k;
            var hi = k;
            for (var j = 0; j < FULL_REFINE_STEPS; j++) {
                let mid = 0.5 * (lo + hi);
                let m_ray = 1.0 / mix(inv_a, inv_b, mid);
                let m_scene = full_scene_depth(mix(a.xy, b.xy, mid));
                if (m_scene > 0.0 && m_ray > m_scene) { hi = mid; } else { lo = mid; }
            }
            hit.uv = mix(a.xy, b.xy, hi);
            hit.found = true;
            return hit;
        }
        prev_k = k;
        k += dk;
    }
    if (open_k >= 0.0) {
        hit.escaped = true;
        // Depth changes linearly along the ray.
        let depth = 1.0 / mix(inv_a, inv_b, open_k);
        hit.dist = clamp(select(open_k * len, (-depth - origin.z) / dir.z, abs(dir.z) > 1e-3), 0.0, len);
    }
    return hit;
}

// The camera-relative point of depth `z` at pixel `px` of a `size` view.
fn full_point(px: vec2<i32>, z: f32, size: vec2<i32>) -> vec3<f32> {
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(size);
    return (frame.inv_view * vec4<f32>(view_position(uv, z), 0.0)).xyz;
}

// The face's normal at pixel `px` (camera-relative `rel`, depth `z`) from the depth of the
// pixels beside it, as ray_sun.wgsl `sun_face_normal` finds it: on each axis the side nearer
// in depth, turned toward the camera (`toward`); `fallback` where a side has none.
fn full_face_normal(px: vec2<i32>, rel: vec3<f32>, z: f32, size: vec2<i32>, toward: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let last = size - 1;
    let pl = max(px - vec2<i32>(1, 0), vec2<i32>(0));
    let pr = min(px + vec2<i32>(1, 0), last);
    let pu = max(px - vec2<i32>(0, 1), vec2<i32>(0));
    let pd = min(px + vec2<i32>(0, 1), last);
    let zl = textureLoad(full_depth, pl, 0);
    let zr = textureLoad(full_depth, pr, 0);
    let zu = textureLoad(full_depth, pu, 0);
    let zd = textureLoad(full_depth, pd, 0);
    let left = px.x > 0 && (px.x == last.x || abs(zl - z) < abs(zr - z));
    let up = px.y > 0 && (px.y == last.y || abs(zu - z) < abs(zd - z));
    let zx = select(zr, zl, left);
    let zy = select(zd, zu, up);
    if (zx <= 0.0 || zy <= 0.0) { return fallback; }
    let ex = select(full_point(pr, zr, size) - rel, rel - full_point(pl, zl, size), left);
    let ey = select(full_point(pd, zd, size) - rel, rel - full_point(pu, zu, size), up);
    let c = cross(ey, ex);
    let len = length(c);
    if (!(len > 1e-12)) { return fallback; }
    let n = c / len;
    return select(n, -n, dot(n, toward) < 0.0);
}

// How much of the screen's outer `FULL_SCREEN_EDGE` lies between `uv` and the screen's edge.
fn full_edge(uv: vec2<f32>) -> f32 {
    let d = min(uv, 1.0 - uv);
    return saturate(min(d.x, d.y) / FULL_SCREEN_EDGE);
}

// Last frame's picture at screen `uv` (not pre-exposed).
fn full_seen_at(uv: vec2<f32>) -> vec3<f32> {
    let c = textureSampleLevel(full_picture, linear_clamp, uv, 0.0).rgb;
    return min(c / frame.reflections.y, vec3<f32>(60000.0));
}

// What last frame's picture shows of camera-relative point `p` on a surface facing `n`, where
// the camera sees that surface there now (the prepass drew a surface as deep, facing about the
// same way, at the pixel `p` projects onto): rgb, and its share (0: the camera doesn't see it;
// under 1 within the screen's outer 8%, where the hit's own light takes over).
fn full_seen(p: vec3<f32>, n: vec3<f32>) -> vec4<f32> {
    let c = frame.view_proj * vec4<f32>(p, 1.0);
    if (c.w <= 0.0) { return vec4<f32>(0.0); }
    let ndc = c.xy / c.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) { return vec4<f32>(0.0); }
    let size = vec2<i32>(textureDimensions(full_depth));
    let px = clamp(vec2<i32>(floor(uv * vec2<f32>(size))), vec2<i32>(0), size - 1);
    let z = textureLoad(full_depth, px, 0);
    let nr = textureLoad(full_normal, px, 0);
    if (z <= 0.0 || nr.w < 0.5) { return vec4<f32>(0.0); }
    let depth = view_depth(p);
    let scene = scene_linear_depth((vec2<f32>(px) + 0.5) / vec2<f32>(size), z);
    if (abs(scene - depth) > 0.02 * depth + 0.05 || dot(oct_decode(nr.xy), n) < 0.3) {
        return vec4<f32>(0.0);
    }
    let prev_uv = uv - textureLoad(full_motion, px, 0).xy;
    if (any(prev_uv < vec2<f32>(0.0)) || any(prev_uv > vec2<f32>(1.0))) { return vec4<f32>(0.0); }
    return vec4<f32>(full_seen_at(prev_uv), full_edge(uv) * full_edge(prev_uv));
}

// The light a ray's hit `s` at distance `t` along `dir` from `origin` sends back along it, as
// `shade_hit` lights a world reflection's hit, with the lamps of a pixel of `roughness`.
fn full_hit_light(origin: vec3<f32>, dir: vec3<f32>, t: f32, s: RaySurface, roughness: f32) -> vec3<f32> {
    var p = origin + dir * t;
    if (any(s.n != s.face)) {
        // `hit_radiance` lifts its shadow rays along `n`: off the face by as much as along a
        // flat face's normal, however far `n` leans.
        p += s.face * (max(0.002, t * 1e-4) * (1.0 - dot(s.n, s.face)));
    }
    let lamps = select(0u, min(FULL_LAMPS_MOST, u32(ray_scene.lights.x)), roughness <= FULL_LAMPS_ROUGHNESS);
    return hit_radiance(p, s.n, dir, t, frame.gi_volume.w, s.albedo, s.emissive, lamps);
}

// The reflection at pixel `pixel`, written to `full_out`, and what it adds to the counts
// (pixels under the limit, answered by the screen, rays, rays' hits taken from the picture).
fn reflect_full_pixel(pixel: vec2<u32>) -> vec4<u32> {
    let px = vec2<i32>(pixel);
    let size = vec2<i32>(textureDimensions(full_depth));
    let nr = textureLoad(full_normal, px, 0);
    let z = textureLoad(full_depth, px, 0);
    let roughness = nr.z;
    if (z <= 0.0 || nr.w < 0.5 || roughness > frame.reflections.z) {
        textureStore(full_out, pixel, vec4<f32>(0.0));
        return vec4<u32>(0u);
    }
    var counts = vec4<u32>(1u, 0u, 0u, 0u);
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(size);
    let pos = view_position(uv, z);
    let rel = (frame.inv_view * vec4<f32>(pos, 0.0)).xyz;
    let depth = -pos.z;
    let n = oct_decode(nr.xy);
    let alpha = roughness * roughness;
    let v = select(normalize(-rel), frame.inv_view[2].xyz, frame.exposure.w > 0.5);
    // The lobe's dominant direction, as `shade_surface`, ssr.wgsl and `cs_reflect` reflect,
    // above the face the prepass drew: the ray leaves the face, whatever the normal map bends.
    let face = full_face_normal(px, rel, z, size, v, select(n, -n, dot(n, v) < 0.0));
    let r = reflect(-v, n);
    var dir = normalize(mix(n, r, (1.0 - alpha) * (sqrt(1.0 - alpha) + alpha)));
    let above = dot(dir, face);
    if (above < FULL_ABOVE_FACE) {
        dir = normalize(dir + face * (FULL_ABOVE_FACE - above));
    }
    let origin = rel + face * (0.02 + 0.002 * depth);
    let noise = interleaved_gradient_noise(vec2<f32>(px) + frame.time.w * 5.588238);

    // The screen first, where last frame's picture exists. A surface the march met within the
    // screen's outer 8% is last frame's picture only by its share there (`screen`, at
    // `screen_t` meters), the ray's answer the rest, as a ray's hit the camera sees mixes into
    // its own light: answered at once, the picture ended in a seam where its subject left the
    // screen.
    var screen = vec4<f32>(0.0);
    var screen_t = 0.0;
    var escaped = false;
    var escape_t = 0.0;
    let rays_only = (ray_scene.stand_ins.w & RAY_SCENE_REFLECT_RAYS_ONLY) != 0u;
    if (frame.reflections.y > 0.0 && !rays_only) {
        let origin_view = (frame.view * vec4<f32>(origin, 0.0)).xyz;
        let dir_view = normalize((frame.view * vec4<f32>(dir, 0.0)).xyz);
        let m = full_march(origin_view, dir_view, min(depth * 3.0 + 30.0, 1000.0), noise);
        if (m.found) {
            let hit_px = clamp(vec2<i32>(floor(m.uv * frame.viewport.xy)), vec2<i32>(0), size - 1);
            let hit_n = oct_decode(textureLoad(full_normal, hit_px, 0).xy);
            let prev_uv = m.uv - textureLoad(full_motion, hit_px, 0).xy;
            // The surface met faces the reflection (its back is not what the camera saw), and
            // last frame's picture holds it.
            if (dot(hit_n, dir) < -0.1 && all(prev_uv >= vec2<f32>(0.0)) && all(prev_uv <= vec2<f32>(1.0))) {
                let hit_rel = (frame.inv_view * vec4<f32>(view_position(m.uv, textureLoad(full_depth, hit_px, 0)), 0.0)).xyz;
                screen = vec4<f32>(full_seen_at(prev_uv), full_edge(m.uv) * full_edge(prev_uv));
                screen_t = max(distance(hit_rel, origin), 1e-3);
                if (screen.a >= 1.0) {
                    textureStore(full_out, pixel, vec4<f32>(screen.rgb, screen_t));
                    counts.y = 1u;
                    return counts;
                }
            }
        }
        escaped = m.escaped;
        escape_t = m.dist;
    }

    // A ray through the ray scene.
    counts.z = 1u;
    let t_max = min(384.0, 400.0 * exp(-5.0 * roughness));
    let hit = trace_reflect(origin, dir, t_max);
    // Where the march saw open sky along the reflection, the camera sees through there: an
    // indoor volume's closed face across a window doesn't stop it.
    let wall = select(sky_volume_wall(origin, dir, min(hit.t, t_max)), NO_HIT, escaped);
    var out = vec4<f32>(0.0);
    if (wall < NO_HIT) {
        // A wall of an indoor volume (a cave's rock): lit by the probes around it.
        let p = origin + dir * wall;
        let indirect = probe_gi(p, -dir, -dir);
        out = vec4<f32>(VOLUME_WALL_ALBEDO * INV_PI * indirect.rgb * indirect.a, max(wall, 1e-3));
    } else if (hit.instance != RAY_NONE) {
        // The ray's cone: a pixel across where it starts, widening by a pixel's angle and the
        // lobe's (as wide all along when orthographic), which picks the level of the hit's
        // tile.
        let pixel_m = 2.0 / (abs(frame.proj[1][1]) * frame.viewport.y);
        let cone = select(vec2<f32>(pixel_m * depth, pixel_m + alpha), vec2<f32>(pixel_m, alpha), frame.exposure.w > 0.5);
        let s = ray_surface(ray_last, origin, dir, cone);
        // The back of a single-sided surface: the ray started inside something, so it
        // reflects nothing of its own.
        if (!s.inside) {
            // (Where the march met it in the screen's outer band, its picture is `screen`'s.)
            var seen = vec4<f32>(0.0);
            if (frame.reflections.y > 0.0 && screen.a <= 0.0) {
                seen = full_seen(origin + dir * hit.t, s.face);
            }
            var lit = vec3<f32>(0.0);
            if (seen.a < 1.0) {
                lit = full_hit_light(origin, dir, hit.t, s, roughness);
            }
            if (seen.a > 0.0 || screen.a > 0.0) { counts.w = 1u; }
            out = vec4<f32>(min(mix(lit, seen.rgb, seen.a), vec3<f32>(60000.0)), max(hit.t, 1e-3));
        }
    } else if (escaped) {
        // The ray met nothing past an opening the camera sees the sky through (a window, a gap
        // in the trees): the sky, which the cover overhead doesn't dim (it would under a roof
        // or a canopy, `shade_surface`), as ssr.wgsl `fs_ssr` reflects an escaped march.
        out = vec4<f32>(min(sample_env_rough(dir, roughness) + frame.ambient.rgb, vec3<f32>(60000.0)), max(escape_t, 1e-3));
    } else if (REFLECT_MISS_SEES_SKY && t_max >= SKY_MARCH_REACH_M) {
        // Nothing within the reach of the cover map's march (`specular_sky_visibility`): the
        // sky there is open as far as the scene's things go.
        out = vec4<f32>(0.0, 0.0, 0.0, -1.0);
    }
    if (screen.a > 0.0) {
        // The march's hit in the screen's outer band: over what the ray met by its share, or
        // as it is where the ray met nothing (something the ray scene leaves out).
        out = select(vec4<f32>(screen.rgb, screen_t), vec4<f32>(mix(out.rgb, screen.rgb, screen.a), out.w), out.w > 0.0);
    }
    textureStore(full_out, pixel, out);
    return counts;
}

@compute @workgroup_size(8, 8, 1)
fn cs_reflect_full(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) thread: u32) {
    let counting = shade_skip(COUNT_REFLECTIONS);
    if (thread < 4u) { atomicStore(&full_group_counts[thread], 0u); }
    workgroupBarrier();
    var counts = vec4<u32>(0u);
    if (all(id.xy < textureDimensions(full_out))) {
        counts = reflect_full_pixel(id.xy);
    }
    if (counting) {
        if (counts.x > 0u) { atomicAdd(&full_group_counts[0], counts.x); }
        if (counts.y > 0u) { atomicAdd(&full_group_counts[1], counts.y); }
        if (counts.z > 0u) { atomicAdd(&full_group_counts[2], counts.z); }
        if (counts.w > 0u) { atomicAdd(&full_group_counts[3], counts.w); }
    }
    workgroupBarrier();
    if (counting && thread < 4u) {
        let n = atomicLoad(&full_group_counts[thread]);
        if (n > 0u) { atomicAdd(&full_counts[thread], n); }
    }
}
