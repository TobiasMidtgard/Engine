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
// 3. Blur: edge-aware 5x5 by depth and normal.
// `shade_surface` adds the result times the diffuse albedo (`screen_gi`).

@group(1) @binding(0) var normal_input: texture_2d<f32>;   // prepass normals (octahedral xy, roughness, written)
@group(1) @binding(1) var depth_input: texture_depth_2d;   // prepass depth (reverse-Z)
@group(1) @binding(2) var motion_input: texture_2d<f32>;   // screen motion (uv, current - previous)
@group(1) @binding(3) var previous_color: texture_2d<f32>; // last frame's image, pre-exposed by frame.reflections.y
@group(1) @binding(4) var gi_input: texture_2d<f32>;       // temporal: this frame's trace; blur: the temporal result
@group(1) @binding(5) var gi_history: texture_2d<f32>;     // temporal: last frame's result (rgb, view depth in a)

const RAYS: i32 = 2;
const STEPS: i32 = 16;
// How far rays look for surfaces (meters at 10 m from the camera, scaled with distance). Light
// from farther fades out gradually rather than stopping at a hard edge.
const RADIUS: f32 = 6.0;
const HISTORY_WEIGHT: f32 = 0.9;

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
    return linear_depth(uv, z);
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

@fragment
fn fs_trace(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
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
    let azimuth = interleaved_gradient_noise(vec2<f32>(px) + f32(frame_i % 64u) * 5.588238);
    var sum = vec3<f32>(0.0);
    for (var r = 0; r < RAYS; r++) {
        let u = random3(px, frame_i * u32(RAYS) + u32(r));
        // Cosine-weighted direction around the normal.
        let phi = TAU * fract(azimuth + f32(r) / f32(RAYS) + u.x * 0.1);
        let s = sqrt(u.y);
        let dir = normalize(tangent * (cos(phi) * s) + bitangent * (sin(phi) * s) + n * sqrt(1.0 - u.y));
        sum += trace_ray(origin, dir, radius, u.z);
    }
    return vec4<f32>(sum / f32(RAYS), 1.0);
}

@fragment
fn fs_temporal(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
    let z = textureLoad(depth_input, px, 0);
    if (z <= 0.0) { return vec4<f32>(0.0); }
    let uv = (vec2<f32>(px) + 0.5) * frame.viewport.zw;
    let current = textureLoad(gi_input, px, 0).rgb;
    let depth = linear_depth(uv, z);
    // Where this surface point was last frame, and how far from last frame's camera.
    let prev_uv = uv - textureLoad(motion_input, px, 0).xy;
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
fn fs_blur(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
    let center = textureLoad(gi_input, px, 0);
    if (center.a <= 0.0) { return vec4<f32>(0.0); }
    let n = view_normal(px);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            let q = clamp(px + vec2<i32>(x, y) * 2, vec2<i32>(0), size_i() - 1);
            let s = textureLoad(gi_input, q, 0);
            if (s.a <= 0.0) { continue; }
            let wd = exp(-abs(s.a - center.a) / (0.02 * center.a + 0.01));
            let wn = pow(saturate(dot(view_normal(q), n)), 8.0);
            let w = wd * wn * exp(-f32(x * x + y * y) / 6.0);
            sum += s.rgb * w;
            weight += w;
        }
    }
    return vec4<f32>(sum / max(weight, 1e-4), 1.0);
}
