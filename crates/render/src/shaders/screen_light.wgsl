// The screen light pass: the lighting terms that depend on where a surface is and which way it
// faces, worked out once for every pixel the prepass drew, for `shade_surface` to load
// (`screen_light` in lib_frame.wgsl; `RenderSettings::trial_cuts` bit `SCREEN_LIGHT`), and the
// fog in front of it for `surface_fog` (bit `SCREEN_FOG`).
// Requires lib_types + lib_frame + lib_atmosphere. Group 0 is the frame group with the
// prepass's depth and normals and the fog's tables bound, and stand-ins for the maps drawn
// after the prepass, so the library's own functions run here unchanged.
//
// Why: a pixel's shader runs once for every triangle that touches its 2x2 block. Counted at
// the overview at 1600x900, the ground's shader runs 2.6 times a pixel and meshes' 2.7 times
// (engine-perf/triangle-view), and each run read the shadow maps, marched the depth toward the
// sun, tapped the sky map's rings and blended eight probes. Here that happens once a pixel.
//
// The surface is the prepass's: its normal stands in for the shading normal (as it does for
// ambient occlusion and reflections), and its roughness for the material's in the direction a
// reflection looks for cover.
//
// Group 1 is the sun's trace (`trial_cut_more::RAY_SUN`, ray_scene/sun.rs): where it ran, the
// sun of a traced pixel is its rays' kept over the frames times what the shadow maps hold that
// the rays don't (`sun_joined`). Views without it bind stand-ins and `sun_join.params.x` 0.

// The trace's history as ray_filters.wgsl wrote it this frame: r the sun's visibility, g the
// frames it holds (0: not traced), b the view depth (negative where the ray met the plants), a
// the half width in pixels of the penumbra there (under 1: a hard shadow, the pixel's own).
@group(1) @binding(0) var sun_history: texture_2d<f32>;
// The shadow maps' still images (renderer.rs `StaticShadows`), a layer a cascade as the live
// maps (`shadow_map`): what the live maps hold and they don't moves.
@group(1) @binding(1) var sun_still_images: texture_depth_2d_array;
@group(1) @binding(2) var<uniform> sun_join: SunJoin;

// ray_scene/sun.rs `JoinUniform` (ray_filters.wgsl reads the same buffer).
struct SunJoin {
    // x: the trace is joined this frame; y: the reach of the traced pixels; z: the distance
    // from which the rays meet the swaying plants (`RayUniform.sun` z); w: the rays' length.
    params: vec4<f32>,
    // x: one depth step of the shadow maps; y: where the join ends (meters; it fades into the
    // maps alone over its last tenth); z: the tangent of the radius of the sun's disc the rays
    // aim at (the filter's); w: the filter's.
    depth: vec4<f32>,
};

// The filtered ray at or under which the trace's answer stands: above it, a still image that
// holds a caster within the rays' length on every texel around the point wins (`sun_joined`).
const SUN_RAY_SURE_LIT: f32 = 0.95;

struct LightOut {
    @location(0) a: vec4<f32>,
    @location(1) b: vec4<f32>,
    @location(2) gi: vec4<f32>,
    @location(3) fog: vec4<f32>,
    @location(4) fog_through: vec4<f32>,
};

@vertex
fn vs_light(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

@fragment
fn fs_light(in: FullscreenOut) -> LightOut {
    var out: LightOut;
    out.a = vec4<f32>(1.0);
    out.b = vec4<f32>(1.0);
    out.gi = vec4<f32>(0.0);
    out.fog = vec4<f32>(0.0);
    out.fog_through = vec4<f32>(1.0);
    let px = vec2<i32>(floor(in.position.xy));
    let z = textureLoad(prepass_depth, px, 0);
    let surface = textureLoad(prepass_normal, px, 0);
    if (z <= 0.0 || surface.w < 0.5) { return out; }
    let uv = (vec2<f32>(px) + 0.5) * frame.viewport.zw;
    let rel = (frame.inv_view * vec4<f32>(view_position(uv, z), 0.0)).xyz;
    let n = oct_decode(surface.xy);
    let v = normalize(-rel);
    let roughness = surface.z * surface.z;

    // The fog and haze in front of the surface (the light maps are stand-ins here, so this
    // is `surface_fog` worked out in full).
    // (None with `SKIP_SCREEN_LIGHT_SUN`: the picture of where the pass's maps are used.)
    if (trial_cut(CUT_SCREEN_FOG) && !shade_skip(SKIP_SCREEN_LIGHT_SUN)) {
        let fog = surface_fog(rel);
        out.fog = vec4<f32>(fog.inscatter, 1.0);
        out.fog_through = vec4<f32>(fog.transmittance, 1.0);
    }
    if (!trial_cut(CUT_SCREEN_LIGHT)) { return out; }

    // The sun: the shadow maps as seen from the side it is on, people's capsules (kept apart:
    // light through a blade of grass takes the maps alone, `ScreenLight::maps`), and the two
    // marches through the depth where anything of it is left.
    let capsules = capsule_occlusion(rel, n);
    if (dot(frame.sun_color.rgb, frame.sun_color.rgb) > 0.0) {
        let side = sunlit_side(n);
        out.a.r = sun_joined(px, rel, side);
        out.b.a = capsules.x;
        if (min(out.a.r, capsules.x) > 0.0) {
            out.a.g = contact_shadow(rel, side);
            out.a.b = distant_shadow(rel, side);
        }
    }

    if (shade_skip(SKIP_SCREEN_LIGHT_SUN)) { out.a.r = 0.0; }

    // The sky: cover overhead, leaves, people, and the cover a reflection sees around it.
    out.a.a = sky_visibility(rel, n);
    out.b.r = capsules.y;
    out.b.g = canopy_last_visibility;
    out.b.b = specular_sky_visibility(rel, dominant_reflection(n, v, roughness), n, roughness);

    out.gi = probe_gi(rel, n, v);
    return out;
}

// ---------------------------------------------------------------- the sun's trace and the maps
// The sun at the pixel `px` (camera-relative `rel`, its sunlit side `n`): `shadow_factor`, or
// where the trace ran for it, the trace's answer (`sun_ray`) times what the shadow maps hold
// that the rays don't (`shadow_rest`: what moves, and what stands farther up-sun than they
// reach), and people's inset maps as `shadow_factor` takes them. Where the rays saw the sun
// whole and the still image holds a caster within their length all round the point, the rays
// do not hold that caster (one too small for them, one the scene left out, one placed since
// their last build): the maps' answer stands there. Over the join's last tenth it fades into
// `shadow_factor`.
fn sun_joined(px: vec2<i32>, rel: vec3<f32>, n: vec3<f32>) -> f32 {
    if (sun_join.params.x < 0.5 || frame.sun_color.w < 0.5 || shade_skip(SKIP_SUN_SHADOW_MAPS)) {
        return shadow_factor(rel, n);
    }
    let h = textureLoad(sun_history, px, 0);
    let dist = length(rel);
    let end = sun_join.depth.y;
    if (h.g <= 0.0 || dist >= end) { return shadow_factor(rel, n); }
    let plants = h.b < 0.0;
    let noise = frame_noise(pixel_of(rel));
    let ray = sun_ray(px, h);
    let p = sun_map_point(rel, n, dist, noise, plants);
    var s = ray * shadow_rest(rel, p, noise);
    if (ray > SUN_RAY_SURE_LIT && s > 0.0 && still_holds_a_caster(p)) {
        s = min(s, cascade_visibility(rel, n, dist, noise));
    }
    if (frame.distant_shadows.z > 0.5) { s = min(s, inset_visibility(rel, n, noise)); }
    let fade = saturate((dist - 0.9 * end) / (0.1 * end));
    if (fade > 0.0) { s = mix(s, shadow_factor(rel, n), fade); }
    return s;
}

// The trace's history `h` at `px`, with its 8 neighbours on the same surface (traced the same
// way, near in depth) inside a penumbra at least two pixels wide: a 3x3 tent, which smooths
// what the history's frames left of the rays' grain there. A narrower penumbra, a hard shadow
// (a thin post's), keeps the pixel's own.
fn sun_ray(px: vec2<i32>, h: vec4<f32>) -> f32 {
    if (h.a < 1.0) { return h.r; }
    let last = vec2<i32>(textureDimensions(sun_history)) - 1;
    let depth = abs(h.b);
    let plants = h.b < 0.0;
    let tolerance = 0.02 * depth + 0.01;
    var sum = h.r * 4.0;
    var weight = 4.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            if (dx == 0 && dy == 0) { continue; }
            let t = textureLoad(sun_history, clamp(px + vec2<i32>(dx, dy), vec2<i32>(0), last), 0);
            if (t.g <= 0.0 || (t.b < 0.0) != plants) { continue; }
            let w = select(1.0, 2.0, dx == 0 || dy == 0) * exp(-abs(abs(t.b) - depth) / tolerance);
            sum += t.r * w;
            weight += w;
        }
    }
    return sum / weight;
}

// Where a receiver falls in the shadow cascades, read as `cascade_visibility` reads them.
struct SunMapPoint {
    inside: bool,
    cascade: u32,
    uv: vec2<f32>,
    // The receiver's depth in the map, with the maps' constant bias (the blocker search's).
    z: f32,
    texel_world: f32,
    z_per_meter: f32,
    n_dot_l: f32,
};

// The cascade by distance `dist` and the dither over each one's last tenth (`noise`), as
// `cascade_visibility` picks it, except before the distance from which the rays meet the swaying
// plants: a pixel whose ray met them (`plants`) reads the first cascade that holds them at rest,
// one whose ray passed them the last that draws them moving, so that the plants are counted once,
// by the ray or by the maps. (The trace dithers that band with the same noise, so the two agree
// but at the band's rounding.)
fn sun_map_point(rel: vec3<f32>, n: vec3<f32>, dist: f32, noise: f32, plants: bool) -> SunMapPoint {
    var out: SunMapPoint;
    var cascade = 0u;
    for (var i = 0u; i < 3u; i++) {
        if (dist > frame.shadow_splits[i]) { cascade = i + 1u; }
    }
    if (cascade < 3u) {
        let split = frame.shadow_splits[cascade];
        let band = split * 0.1;
        if (dist > split - band && noise > (split - dist) / band) { cascade += 1u; }
    }
    let plants_from = sun_join.params.z;
    if (dist <= plants_from) {
        // (`plants_from` is the end of the last cascade that draws them moving: the same
        // number as its split, `RayScene::finish`.)
        var swaying = 0u;
        for (var i = 0u; i < 3u; i++) {
            if (frame.shadow_splits[i] < plants_from) { swaying = i + 1u; }
        }
        cascade = select(min(cascade, swaying), max(cascade, min(swaying + 1u, 3u)), plants);
    }
    out.cascade = cascade;
    let m = frame.shadow_cascades[cascade];
    out.texel_world = 2.0 / (frame.shadow_params.w * length(vec3<f32>(m[0].x, m[1].x, m[2].x)));
    out.z_per_meter = length(vec3<f32>(m[0].z, m[1].z, m[2].z));
    out.n_dot_l = dot(n, frame.sun_dir.xyz);
    let normal_bias = frame.shadow_params.y * out.texel_world * (1.0 - 0.5 * saturate(out.n_dot_l));
    let lp = m * vec4<f32>(rel + n * normal_bias, 1.0);
    out.uv = lp.xy * vec2<f32>(0.5, -0.5) + 0.5;
    out.z = lp.z - frame.shadow_params.x;
    out.inside = all(out.uv >= vec2<f32>(0.0)) && all(out.uv <= vec2<f32>(1.0)) && out.z <= 1.0;
    return out;
}

// The sun the shadow maps let through at `p` (camera-relative `rel`) counting only what the
// trace leaves to them: a texel is a caster when the live map holds one nearer the sun than the
// receiver that the still image doesn't (it moves; nearer than the image by more than a depth
// step), or one farther up-sun than 0.9 of the rays' length (a still caster there is counted by
// both until the rays' end). `cascade_visibility`'s blocker search and filter, each tap read in
// both maps: the filter's four texels by their bilinear weights.
fn shadow_rest(rel: vec3<f32>, p: SunMapPoint, noise: f32) -> f32 {
    if (!p.inside) { return 1.0; }
    let map_size = frame.shadow_params.w;
    let texel = 1.0 / map_size;
    let angle = noise * TAU;
    let tan_sun = tan(frame.sun_dir.w) * frame.effects.z;
    let min_radius = max(frame.shadow_params.z, 0.5);
    let pixel_world = view_depth(rel) * 2.0 / max(abs(frame.proj[1][1]) * frame.viewport.y, 1e-6);
    let texel_px = p.texel_world / max(pixel_world, 1e-6);
    let step = sun_join.depth.x;
    let beyond = 0.9 * sun_join.params.w * p.z_per_meter;

    let search = clamp(30.0 * tan_sun / p.texel_world, min_radius, 16.0);
    let search_px = search * texel_px;
    let blocker_taps = clamp(i32(ceil(2.0 * search_px * search_px)), SHADOW_MIN_TAPS, SHADOW_BLOCKER_TAPS);
    var blocker_sum = 0.0;
    var blockers = 0.0;
    for (var i = 0; i < blocker_taps; i++) {
        let at = p.uv + vogel_disk(i, blocker_taps, angle) * search * texel;
        let live = textureSampleLevel(shadow_map, sky_occlusion_sampler, at, p.cascade, 0);
        let still = textureSampleLevel(sun_still_images, sky_occlusion_sampler, at, p.cascade, 0);
        if (live < max(min(p.z, still - step), p.z - beyond)) {
            blocker_sum += live;
            blockers += 1.0;
        }
    }
    if (blockers < 0.5) { return 1.0; }
    if (trial_cut(CUT_SHADOW_UMBRA) && blockers > f32(blocker_taps) - 0.5) { return 0.0; }
    let blocker_distance = max(p.z - blocker_sum / blockers, 0.0) / max(p.z_per_meter, 1e-9);
    let radius = clamp(blocker_distance * tan_sun / p.texel_world, min_radius, 24.0);
    let slope = sqrt(max(1.0 - p.n_dot_l * p.n_dot_l, 0.0)) / max(abs(p.n_dot_l), 0.2);
    let z_ref = p.z - radius * p.texel_world * min(slope, 5.0) * p.z_per_meter * 0.5;

    let filter_px = radius * texel_px;
    let max_taps = clamp(i32(frame.shading.x), 1, 64);
    let taps = clamp(i32(ceil(4.0 * filter_px * filter_px)), min(SHADOW_MIN_TAPS, max_taps), max_taps);
    var sum = 0.0;
    for (var i = 0; i < taps; i++) {
        let at = p.uv + vogel_disk(i, taps, angle) * radius * texel;
        let live = textureGather(shadow_map, sky_occlusion_sampler, at, p.cascade);
        let still = textureGather(sun_still_images, sky_occlusion_sampler, at, p.cascade);
        // (textureGather's order: (left, bottom), (right, bottom), (right, top), (left, top).)
        let f = fract(at * map_size - 0.5);
        let w = vec4<f32>((1.0 - f.x) * f.y, f.x * f.y, f.x * (1.0 - f.y), (1.0 - f.x) * (1.0 - f.y));
        let caster = live < max(min(vec4<f32>(z_ref), still - step), vec4<f32>(z_ref - beyond));
        sum += dot(select(vec4<f32>(1.0), vec4<f32>(0.0), caster), w);
    }
    return sum / f32(taps);
}

// Whether the still image holds a caster within the rays' length on each of the four texels
// around `p` (not at depth 0: a caster up-sun of the map's near plane, beyond any ray).
fn still_holds_a_caster(p: SunMapPoint) -> bool {
    if (!p.inside) { return false; }
    let d = textureGather(sun_still_images, sky_occlusion_sampler, p.uv, p.cascade);
    let within = p.z - sun_join.params.w * p.z_per_meter;
    return all(d > max(vec4<f32>(within), vec4<f32>(0.0))) && all(d < vec4<f32>(p.z));
}
