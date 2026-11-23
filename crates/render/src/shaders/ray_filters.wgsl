// The sun's trace kept over the frames (`trial_cut_more::RAY_SUN`, ray_scene/sun.rs; pass "ray
// shadows filter", after the frame's ray pass and before the screen light pass, which joins it
// with the shadow maps: screen_light.wgsl `sun_joined`). Each pixel's ray is one draw of the sun's
// disc, 0 or 1; the history averages up to `SUN_HISTORY_MOST` of them, following the surface
// across the screen by the prepass's motion.
//
// The rays around a pixel are read only as far as the penumbra the blockers they met cast
// there (the blockers' distance times the radius of the sun's disc): a shadow whose penumbra is
// narrower than a pixel (a thin post's, one near its caster) is the pixel's own ray, sharp and
// at once, and the history is kept near it, so a caster that moves leaves no trail; a wide one
// (a canopy's, high up) takes the rays of up to the 5x5 pixels around, while the history has
// gathered few and as the bounds the history is kept in.
//
// Composed with lib_types and lib_frame: group 0 is the screen light pass's frame group (the
// prepass's depth and normals bound).

// This frame's trace (ray_sun.wgsl): r the visibility, g the flags over 255, b where a ray was
// stopped how far it went (the square root of its share of the rays' length).
@group(1) @binding(0) var sun_raw: texture_2d<f32>;
// Last frame's history (`sun_history` of screen_light.wgsl, as this pass writes it).
@group(1) @binding(1) var sun_history_prev: texture_2d<f32>;
// The prepass's motion: screen uv now minus last frame's, without the jitter.
@group(1) @binding(2) var sun_motion: texture_2d<f32>;
@group(1) @binding(3) var<uniform> sun_join: SunJoin;

// ray_scene/sun.rs `JoinUniform` (screen_light.wgsl reads the same buffer).
struct SunJoin {
    // x: the light pass joins the trace this frame; y: the reach of the traced pixels; z: the
    // distance from which the rays meet the swaying plants (`RayUniform.sun` z); w: the rays'
    // length (meters).
    params: vec4<f32>,
    // x: one depth step of the shadow maps; y: where the join ends (meters, faded over its last
    // tenth); z: the tangent of the radius of the sun's disc the rays aim at (`RayUniform.sun`
    // w); w: 1 when last frame's history may be read (this pass).
    depth: vec4<f32>,
};

// ray_sun.wgsl's flags.
const SUN_TRACED: u32 = 1u;
const SUN_PLANTS_BY_RAY: u32 = 2u;
// The most frames the history averages (a ray a frame each), and the fewest from which this
// frame's own ray is taken rather than the mean of those around.
const SUN_HISTORY_MOST: f32 = 32.0;
const SUN_HISTORY_OWN: f32 = 4.0;
// How far the history may stray from this frame's rays around: their spread and this. Past it
// by as much again, the history starts over.
const SUN_CLAMP_MARGIN: f32 = 0.1;
// The farthest the rays around are read (pixels: the 5x5 around).
const SUN_WINDOW_MOST: f32 = 2.0;

@vertex
fn vs_sun_filter(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

// The view depth of pixel `px` (0 where the prepass drew nothing there).
fn sun_filter_depth(px: vec2<i32>) -> f32 {
    let z = textureLoad(prepass_depth, px, 0);
    if (z <= 0.0) { return 0.0; }
    return scene_linear_depth((vec2<f32>(px) + 0.5) * frame.viewport.zw, z);
}

// The history at `px`: r the sun's visibility, g the frames it holds (0: the pixel was not
// traced), b the view depth (negative where its ray met the plants), a the half width of the
// penumbra there in pixels, as far as the rays around were read (0: the pixel's own ray).
@fragment
fn fs_sun_temporal(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
    let raw = textureLoad(sun_raw, px, 0);
    let flags = u32(round(raw.g * 255.0));
    let z = textureLoad(prepass_depth, px, 0);
    if ((flags & SUN_TRACED) == 0u || z <= 0.0) {
        return vec4<f32>(1.0, 0.0, 0.0, 0.0);
    }
    let plants = (flags & SUN_PLANTS_BY_RAY) != 0u;
    let uv = (vec2<f32>(px) + 0.5) * frame.viewport.zw;
    let depth = scene_linear_depth(uv, z);

    // This frame's rays of the 5x5 pixels around on the pixel's surface: traced the same way
    // (the plants met or not: inside the band before `sun_join.params.z` neighbours differ), as
    // near in depth as the history's own test asks (-1: another surface). The pixel itself is
    // always among them. And how far the blockers the stopped ones met stand, on average.
    let size = vec2<i32>(textureDimensions(sun_raw));
    let last = size - 1;
    let tolerance = 0.05 * depth + 0.02;
    var rays: array<f32, 25>;
    var went = 0.0;
    var stopped = 0.0;
    for (var k = 0; k < 25; k++) {
        rays[k] = -1.0;
        let q = clamp(px + vec2<i32>(k % 5 - 2, k / 5 - 2), vec2<i32>(0), last);
        let s = textureLoad(sun_raw, q, 0);
        let f = u32(round(s.g * 255.0));
        if ((f & SUN_TRACED) == 0u || ((f & SUN_PLANTS_BY_RAY) != 0u) != plants) { continue; }
        let d = sun_filter_depth(q);
        if (d <= 0.0 || abs(d - depth) > tolerance) { continue; }
        rays[k] = s.r;
        if (s.r < 0.5) {
            went += s.b * s.b * sun_join.params.w;
            stopped += 1.0;
        }
    }

    // The penumbra's half width in pixels (none where no ray around was stopped: full sun),
    // and the rays within it, each by how much of the disc it spans covers the pixel: the
    // pixel's own alone under half a pixel.
    let pixel_world = depth * 2.0 / max(abs(frame.proj[1][1]) * frame.viewport.y, 1e-6);
    var radius = 0.0;
    if (stopped > 0.0) {
        radius = min(went / stopped * sun_join.depth.z / pixel_world, SUN_WINDOW_MOST);
    }
    var sum = 0.0;
    var squares = 0.0;
    var count = 0.0;
    for (var k = 0; k < 25; k++) {
        let w = saturate(radius + 0.5 - length(vec2<f32>(f32(k % 5 - 2), f32(k / 5 - 2))));
        if (rays[k] < 0.0 || w <= 0.0) { continue; }
        sum += w * rays[k];
        squares += w * rays[k] * rays[k];
        count += w;
    }
    let mean = sum / count;
    let deviation = sqrt(max(squares / count - mean * mean, 0.0));

    // Where the surface was last frame, read from the four texels around it that hold the same
    // surface (its depth there as the history's test of ssgi.wgsl takes it) traced the same way.
    // The frames they hold count by how much of the four they cover.
    var history = mean;
    var held = 0.0;
    let prev_uv = uv - textureLoad(sun_motion, px, 0).xy;
    if (sun_join.depth.w > 0.5 && frame.camera_delta.w > 0.5
        && all(prev_uv >= vec2<f32>(0.0)) && all(prev_uv <= vec2<f32>(1.0))) {
        let rel = (frame.inv_view * vec4<f32>(view_position(uv, z), 0.0)).xyz;
        let prev_depth = (frame.prev_view_proj * vec4<f32>(rel + frame.camera_delta.xyz, 1.0)).w;
        let hsize = vec2<i32>(textureDimensions(sun_history_prev));
        let t = prev_uv * vec2<f32>(hsize) - 0.5;
        let base = vec2<i32>(floor(t));
        let f = t - floor(t);
        var weight = 0.0;
        var value = 0.0;
        for (var k = 0; k < 4; k++) {
            let o = vec2<i32>(k & 1, k >> 1u);
            let q = clamp(base + o, vec2<i32>(0), hsize - 1);
            let h = textureLoad(sun_history_prev, q, 0);
            if (h.g <= 0.0 || (h.b < 0.0) != plants) { continue; }
            if (abs(abs(h.b) - prev_depth) >= 0.05 * prev_depth + 0.02) { continue; }
            let w = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
            weight += w;
            value += w * h.r;
            held += w * h.g;
        }
        if (weight > 1e-4) {
            // Kept near what the rays around say now: a shadow that came or went shows at once,
            // and where the history lay well outside them it starts over from them.
            let kept = value / weight;
            let margin = deviation + SUN_CLAMP_MARGIN;
            history = clamp(kept, mean - margin, mean + margin);
            held *= saturate(1.0 - abs(kept - history) / SUN_CLAMP_MARGIN);
        } else {
            held = 0.0;
        }
    }
    let frames = min(held + 1.0, SUN_HISTORY_MOST);
    let sample = select(raw.r, mean, frames <= SUN_HISTORY_OWN);
    let visibility = mix(history, sample, 1.0 / frames);
    return vec4<f32>(visibility, frames, select(depth, -depth, plants), radius);
}
