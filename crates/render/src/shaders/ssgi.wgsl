// Screen-space diffuse global illumination: light bounced off nearby surfaces that are on
// screen (sunlit walls tint the ground beside them, lamps and glowing mushrooms light their
// surroundings, a red carpet warms a white wall). Requires lib_types + lib_frame (group 0 is
// the prepass frame group, whose screen-space maps are stand-ins; the real inputs are in group 1).
//
// 1. Trace: every prepass pixel sends two cosine-distributed rays a few meters through the depth
//    buffer. A ray that hits a surface brings back that surface's radiance from the previous
//    frame's finished image (which already holds last frame's bounce, so light keeps bouncing
//    over frames); a ray that escapes brings nothing, because the sky's light is already in
//    `sky_ambient` times the ambient occlusion. Output: mean radiance per ray (cd/m^2).
// 2. Temporal: blend into last frame's result where the surface was visible (motion vectors,
//    depth check), about 10 frames deep.
// 3. Blur, twice (a-trous): edge-aware 5x5 by depth and normal, first over neighboring pixels,
//    then over every third. Two rays a pixel that now and then hit a small bright surface (a
//    glowing lamp globe) are fireflies; the first pass weights samples down by their brightness
//    (Karis 2014) so one doesn't spread into a blotch, and covering every pixel before the wide
//    pass keeps the wide pass's spacing from showing as a dotted grid.
// `shade_surface` adds the result times the diffuse albedo (`screen_gi`).
//
// The half-size way (`trial_cut::BOUNCE_HALF`, the `_half` entry points and `fs_spread`): steps
// 1 to 3 run on targets half as wide and high, for the pixel at every even column and row, with
// four rays where the full-size way sends two, the wide blur over every second texel; a last
// full-size pass spreads the result to every pixel from the four texels around it, weighted by
// depth and normal as the blurs are (a joint bilateral upsample). Bounce light varies slowly
// across a surface, and the weights keep it from crossing edges.

@group(1) @binding(0) var normal_input: texture_2d<f32>;   // prepass normals (octahedral xy, roughness, written)
@group(1) @binding(1) var depth_input: texture_depth_2d;   // prepass depth (reverse-Z)
@group(1) @binding(2) var motion_input: texture_2d<f32>;   // screen motion (uv, current - previous)
@group(1) @binding(3) var previous_color: texture_2d<f32>; // last frame's image, pre-exposed by frame.reflections.y
@group(1) @binding(4) var gi_input: texture_2d<f32>;       // temporal: this frame's trace; blur: the temporal result
@group(1) @binding(5) var gi_history: texture_2d<f32>;     // temporal: last frame's result (rgb, view depth in a)

const RAYS: i32 = 2;
const RAYS_HALF: i32 = 4;
const STEPS: i32 = 16;
// How far rays look for surfaces (meters at 10 m from the camera, scaled with distance). Light
// from farther fades out gradually rather than stopping at a hard edge.
const RADIUS: f32 = 6.0;
const HISTORY_WEIGHT: f32 = 0.9;
// Samples brighter on screen than about 1 / FIREFLY_SCALE (1 = white) lose weight in the near
// blur: bounce light itself rarely gets that bright, a ray that hit a lamp globe does.
const FIREFLY_SCALE: f32 = 0.25;

@vertex
fn vs_gi(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

fn size_i() -> vec2<i32> {
    return vec2<i32>(textureDimensions(depth_input));
}

fn project(p: vec3<f32>) -> vec3<f32> {
    let c = frame.proj * vec4<f32>(p, 1.0);
    let ndc = c.xy / c.w;
    return vec3<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, -p.z);
}

fn pixel_at(uv: vec2<f32>) -> vec2<i32> {
    return clamp(vec2<i32>(floor(uv * frame.viewport.xy)), vec2<i32>(0), size_i() - 1);
}

fn scene_depth(uv: vec2<f32>) -> f32 {
    let z = textureLoad(depth_input, pixel_at(uv), 0);
    if (z <= 0.0) { return 0.0; }
    return scene_linear_depth(uv, z);
}

// Three independent uniform numbers in [0, 1) (PCG3D, Jarzynski and Olano 2020). Correlated
// noise between the ray direction and the step offset shows up as bands.
fn random3(px: vec2<i32>, index: u32) -> vec3<f32> {
    var v = vec3<u32>(vec2<u32>(px), index) * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3<u32>(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return vec3<f32>(v >> vec3<u32>(8u)) * (1.0 / 16777216.0);
}

fn view_normal(px: vec2<i32>) -> vec3<f32> {
    return normalize((frame.view * vec4<f32>(oct_decode(textureLoad(normal_input, px, 0).xy), 0.0)).xyz);
}

// Radiance (cd/m^2, not pre-exposed) brought back by one ray, or 0 when it escapes.
fn trace_ray(origin: vec3<f32>, dir: vec3<f32>, max_len: f32, jitter: f32) -> vec3<f32> {
    var len = max_len;
    if (dir.z > 0.0) {
        len = min(len, (-frame.exposure.z * 1.5 - origin.z) / dir.z);
    }
    if (len <= 0.0) { return vec3<f32>(0.0); }
    let a = project(origin);
    let b = project(origin + dir * len);
    let inv_a = 1.0 / a.z;
    let inv_b = 1.0 / b.z;
    for (var i = 0; i < STEPS; i++) {
        // Denser near the start, where most occluders are.
        let t = (f32(i) + jitter) / f32(STEPS);
        let k = t * t;
        let uv = mix(a.xy, b.xy, k);
        if (any(uv < vec2(0.0)) || any(uv > vec2(1.0))) { return vec3<f32>(0.0); }
        let ray = 1.0 / mix(inv_a, inv_b, k);
        let scene = scene_depth(uv);
        // Accept surfaces the ray passed behind by less than its thickness (which grows with
        // distance and with the depth the next step spans, so long steps don't skip walls).
        let next = (t + 1.0 / f32(STEPS));
        let span = abs(1.0 / mix(inv_a, inv_b, min(next * next, 1.0)) - ray);
        if (scene > 0.0 && ray > scene && ray - scene < max(0.25 + 0.03 * scene, span)) {
            let px = pixel_at(uv);
            // Only the side of a surface the ray can see lights it.
            let facing = saturate(-dot(view_normal(px), dir) * 3.0);
            let prev_uv = uv - textureLoad(motion_input, px, 0).xy;
            if (any(prev_uv < vec2(0.0)) || any(prev_uv > vec2(1.0))) { return vec3<f32>(0.0); }
            let c = textureSampleLevel(previous_color, linear_clamp, prev_uv, 0.0).rgb;
            let fade = 1.0 - smoothstep(0.35, 1.0, k);
            return min(c / frame.reflections.y, vec3<f32>(60000.0)) * facing * fade;
        }
    }
    return vec3<f32>(0.0);
}

// The mean radiance of `rays` rays from prepass pixel `px`; `cell` (the texel written) seeds
// the random numbers.
fn trace(px: vec2<i32>, cell: vec2<i32>, rays: i32) -> vec4<f32> {
    let z = textureLoad(depth_input, px, 0);
    if (z <= 0.0 || textureLoad(normal_input, px, 0).w < 0.5 || frame.reflections.y <= 0.0) {
        return vec4<f32>(0.0);
    }
    let uv = (vec2<f32>(px) + 0.5) * frame.viewport.zw;
    let pos = view_position(uv, z);
    let n = view_normal(px);
    let depth = -pos.z;
    let origin = pos + n * (0.02 + 0.002 * depth);
    // Orthonormal basis around the normal (Duff et al. 2017).
    let sign = select(-1.0, 1.0, n.z >= 0.0);
    let ba = -1.0 / (sign + n.z);
    let bb = n.x * n.y * ba;
    let tangent = vec3<f32>(1.0 + sign * n.x * n.x * ba, sign * bb, -sign * n.x);
    let bitangent = vec3<f32>(bb, sign + n.y * n.y * ba, -n.y);
    let frame_i = u32(frame.time.w);
    let radius = RADIUS * max(1.0, depth / 10.0);
    // The rays' azimuths spread evenly over neighboring pixels (interleaved gradient noise,
    // rotated each frame); everything else is independent per ray.
    let azimuth = interleaved_gradient_noise(vec2<f32>(cell) + f32(frame_i % 64u) * 5.588238);
    var sum = vec3<f32>(0.0);
    for (var r = 0; r < rays; r++) {
        let u = random3(cell, frame_i * u32(rays) + u32(r));
        // Cosine-weighted direction around the normal.
        let phi = TAU * fract(azimuth + f32(r) / f32(rays) + u.x * 0.1);
        let s = sqrt(u.y);
        let dir = normalize(tangent * (cos(phi) * s) + bitangent * (sin(phi) * s) + n * sqrt(1.0 - u.y));
        sum += trace_ray(origin, dir, radius, u.z);
    }
    return vec4<f32>(sum / f32(rays), 1.0);
}

@fragment
fn fs_trace(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
    return trace(px, px, RAYS);
}

@fragment
fn fs_trace_half(in: FullscreenOut) -> @location(0) vec4<f32> {
    let cell = vec2<i32>(floor(in.position.xy));
    return trace(cell * 2, cell, RAYS_HALF);
}

// This frame's trace at texel `cell` of `gi_input`, made for prepass pixel `px`, blended into
// the history. `cell_uv` is the texel's own center: a still camera reads the history's texel
// itself, never a blend of neighbors.
fn temporal(px: vec2<i32>, cell: vec2<i32>, cell_uv: vec2<f32>) -> vec4<f32> {
    let z = textureLoad(depth_input, px, 0);
    if (z <= 0.0) { return vec4<f32>(0.0); }
    let uv = (vec2<f32>(px) + 0.5) * frame.viewport.zw;
    let current = textureLoad(gi_input, cell, 0).rgb;
    let depth = scene_linear_depth(uv, z);
    // Where this surface point was last frame, and how far from last frame's camera.
    let prev_uv = cell_uv - textureLoad(motion_input, px, 0).xy;
    let rel = (frame.inv_view * vec4<f32>(view_position(uv, z), 1.0)).xyz;
    let prev_depth = (frame.prev_view_proj * vec4<f32>(rel + frame.camera_delta.xyz, 1.0)).w;
    var result = current;
    if (frame.camera_delta.w > 0.5 && all(prev_uv >= vec2(0.0)) && all(prev_uv <= vec2(1.0))) {
        let history = textureSampleLevel(gi_history, linear_clamp, prev_uv, 0.0);
        if (history.a > 0.0 && abs(history.a - prev_depth) < 0.05 * prev_depth + 0.02) {
            result = mix(current, history.rgb, HISTORY_WEIGHT);
        }
    }
    return vec4<f32>(result, depth);
}

@fragment
fn fs_temporal(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
    return temporal(px, px, (vec2<f32>(px) + 0.5) * frame.viewport.zw);
}

@fragment
fn fs_temporal_half(in: FullscreenOut) -> @location(0) vec4<f32> {
    let cell = vec2<i32>(floor(in.position.xy));
    return temporal(cell * 2, cell, (vec2<f32>(cell) + 0.5) / vec2<f32>(textureDimensions(gi_input)));
}

// Edge-aware 5x5 blur of `gi_input` with taps `stride` texels apart. `fireflies` weights each
// sample by 1 / (1 + its brightness on screen), which keeps rare very bright samples from
// dominating. Alpha out: the center's view depth, for a following pass. A texel of `gi_input`
// belongs to the prepass pixel `scale` times its coordinates (1, or 2 on half-size targets).
fn blur(px: vec2<i32>, stride: i32, fireflies: bool, scale: i32) -> vec4<f32> {
    let center = textureLoad(gi_input, px, 0);
    if (center.a <= 0.0) { return vec4<f32>(0.0); }
    let n = view_normal(px * scale);
    let last = vec2<i32>(textureDimensions(gi_input)) - 1;
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            let q = clamp(px + vec2<i32>(x, y) * stride, vec2<i32>(0), last);
            let s = textureLoad(gi_input, q, 0);
            if (s.a <= 0.0) { continue; }
            let wd = exp(-abs(s.a - center.a) / (0.02 * center.a + 0.01));
            let wn = pow(saturate(dot(view_normal(q * scale), n)), 8.0);
            var w = wd * wn * exp(-f32(x * x + y * y) / 6.0);
            if (fireflies) {
                w /= 1.0 + luminance(s.rgb) * frame.reflections.y * FIREFLY_SCALE;
            }
            sum += s.rgb * w;
            weight += w;
        }
    }
    return vec4<f32>(sum / max(weight, 1e-12), center.a);
}

@fragment
fn fs_blur_near(in: FullscreenOut) -> @location(0) vec4<f32> {
    return blur(vec2<i32>(floor(in.position.xy)), 1, true, 1);
}

@fragment
fn fs_blur(in: FullscreenOut) -> @location(0) vec4<f32> {
    let b = blur(vec2<i32>(floor(in.position.xy)), 3, false, 1);
    return vec4<f32>(b.rgb, select(0.0, 1.0, b.a > 0.0));
}

@fragment
fn fs_blur_near_half(in: FullscreenOut) -> @location(0) vec4<f32> {
    return blur(vec2<i32>(floor(in.position.xy)), 1, true, 2);
}

// The wide blur at half size: every second texel is every fourth pixel, a little wider than
// the full-size way's every third. Alpha stays the view depth for `fs_spread`.
@fragment
fn fs_blur_half(in: FullscreenOut) -> @location(0) vec4<f32> {
    return blur(vec2<i32>(floor(in.position.xy)), 2, false, 2);
}

// The half-size result (`gi_input`, view depth in alpha) spread to every pixel: the four
// texels around it by their distance, their depth against this pixel's and their normal against
// its own. A pixel the texel was made for (even column and row) takes that texel alone.
@fragment
fn fs_spread(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
    let z = textureLoad(depth_input, px, 0);
    if (z <= 0.0) { return vec4<f32>(0.0); }
    let uv = (vec2<f32>(px) + 0.5) * frame.viewport.zw;
    let depth = scene_linear_depth(uv, z);
    let n = view_normal(px);
    let last = vec2<i32>(textureDimensions(gi_input)) - 1;
    let cell = px / 2;
    let f = vec2<f32>(px - cell * 2) * 0.5;
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    var nearest = vec3<f32>(0.0);
    var nearest_gap = 1e30;
    for (var y = 0; y <= 1; y++) {
        for (var x = 0; x <= 1; x++) {
            let wx = select(1.0 - f.x, f.x, x == 1);
            let wy = select(1.0 - f.y, f.y, y == 1);
            if (wx * wy <= 0.0) { continue; }
            let q = min(cell + vec2<i32>(x, y), last);
            let s = textureLoad(gi_input, q, 0);
            if (s.a <= 0.0) { continue; }
            let gap = abs(s.a - depth);
            if (gap < nearest_gap) {
                nearest_gap = gap;
                nearest = s.rgb;
            }
            let wd = exp(-gap / (0.02 * depth + 0.01));
            let wn = pow(saturate(dot(view_normal(q * 2), n)), 8.0);
            let w = wx * wy * wd * wn;
            sum += s.rgb * w;
            weight += w;
        }
    }
    // No texel around it is of this pixel's surface (a thin thing between two traced pixels):
    // the one nearest in depth.
    if (weight < 1e-4) { return vec4<f32>(nearest, 1.0); }
    return vec4<f32>(sum / weight, 1.0);
}
