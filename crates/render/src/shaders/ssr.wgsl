// Screen-space reflections from the depth prepass. Requires lib_types + lib_frame (group 0 is
// the prepass frame group, whose screen-space maps are stand-ins; the real inputs are in group 1).
//
// Every prepass pixel smoother than `RenderSettings::ssr_max_roughness` marches its reflection
// ray across the screen through the depth buffer (evenly in pixels, perspective-correct depth,
// then a binary search). Where the ray hits a surface, the reflection is that surface as it
// looked in the previous frame's finished image (found through the motion vectors, so moving
// things reflect where they are), gathered over a disc that widens with roughness and distance
// so glossy reflections blur like the GGX lobe. Coverage (alpha) fades toward screen edges, the
// end of the ray, rays toward the camera and the roughness limit; `shade_surface` blends the
// rest from the prefiltered sky. Output: reflected radiance (cd/m^2, not pre-exposed), coverage.

@group(1) @binding(0) var normal_input: texture_2d<f32>;   // prepass normals (octahedral xy, roughness, written)
@group(1) @binding(1) var depth_input: texture_depth_2d;   // prepass depth (reverse-Z)
@group(1) @binding(2) var motion_input: texture_2d<f32>;   // screen motion (uv, current - previous)
@group(1) @binding(3) var previous_color: texture_2d<f32>; // last frame's image, pre-exposed by frame.reflections.y

const MAX_STEPS: i32 = 64;
const REFINE_STEPS: i32 = 6;
const GATHER_TAPS: i32 = 8;

@vertex
fn vs_ssr(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

fn size_i() -> vec2<i32> {
    return vec2<i32>(textureDimensions(depth_input));
}

// Screen uv and view depth (positive, meters) of view-space point `p`.
fn project(p: vec3<f32>) -> vec3<f32> {
    let c = frame.proj * vec4<f32>(p, 1.0);
    let ndc = c.xy / c.w;
    return vec3<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, -p.z);
}

// View depth of the scene at `uv` (0 where nothing was drawn).
fn scene_depth(uv: vec2<f32>) -> f32 {
    let px = clamp(vec2<i32>(floor(uv * frame.viewport.xy)), vec2<i32>(0), size_i() - 1);
    let z = textureLoad(depth_input, px, 0);
    if (z <= 0.0) { return 0.0; }
    return scene_linear_depth(uv, z);
}

fn edge_fade(uv: vec2<f32>) -> f32 {
    let d = min(uv, 1.0 - uv);
    return saturate(min(d.x, d.y) / 0.08);
}

struct Hit {
    uv: vec2<f32>,
    along: f32, // 0..1 of the way along the ray
    found: bool,
};

// March from view-space `origin` along `dir` (unit) for up to `max_len` meters.
fn trace(origin: vec3<f32>, dir: vec3<f32>, max_len: f32, jitter: f32) -> Hit {
    var hit: Hit;
    hit.found = false;
    // Keep the end in front of the near plane.
    var len = max_len;
    if (dir.z > 0.0) {
        len = min(len, (-frame.exposure.z * 1.5 - origin.z) / dir.z);
    }
    if (len <= 0.0) { return hit; }
    let a = project(origin);
    let b = project(origin + dir * len);
    // Perspective-correct depth: 1/depth is linear in screen space.
    let inv_a = 1.0 / a.z;
    let inv_b = 1.0 / b.z;
    let pixels = length((b.xy - a.xy) * frame.viewport.xy);
    let steps = clamp(i32(pixels), 1, MAX_STEPS);
    let dk = 1.0 / f32(steps);
    var prev_k = 0.0;
    var k = dk * (0.5 + jitter);
    for (var i = 0; i < steps; i++) {
        let uv = mix(a.xy, b.xy, k);
        if (any(uv < vec2(0.0)) || any(uv > vec2(1.0))) { return hit; }
        let ray = 1.0 / mix(inv_a, inv_b, k);
        let scene = scene_depth(uv);
        // The ray passed behind a surface: accept it if not too far behind (thickness grows
        // with distance and with the depth the step spans).
        let step_span = abs(1.0 / mix(inv_a, inv_b, min(k + dk, 1.0)) - ray);
        let thickness = max(0.1 + 0.02 * scene, step_span * 1.5);
        if (scene > 0.0 && ray > scene && ray - scene < thickness) {
            // Binary search between the last miss and this step.
            var lo = prev_k;
            var hi = k;
            for (var j = 0; j < REFINE_STEPS; j++) {
                let mid = 0.5 * (lo + hi);
                let m_uv = mix(a.xy, b.xy, mid);
                let m_ray = 1.0 / mix(inv_a, inv_b, mid);
                let m_scene = scene_depth(m_uv);
                if (m_scene > 0.0 && m_ray > m_scene) { hi = mid; } else { lo = mid; }
            }
            hit.uv = mix(a.xy, b.xy, hi);
            hit.along = hi;
            hit.found = true;
            return hit;
        }
        prev_k = k;
        k += dk;
    }
    return hit;
}

@fragment
fn fs_ssr(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
    let nr = textureLoad(normal_input, px, 0);
    let z = textureLoad(depth_input, px, 0);
    let max_roughness = frame.reflections.z;
    if (z <= 0.0 || nr.w < 0.5 || nr.z > max_roughness || frame.reflections.y <= 0.0) {
        return vec4<f32>(0.0);
    }
    let roughness = nr.z;
    let alpha = roughness * roughness;
    let uv = (vec2<f32>(px) + 0.5) * frame.viewport.zw;
    let pos = view_position(uv, z);
    let n = normalize((frame.view * vec4<f32>(oct_decode(nr.xy), 0.0)).xyz);
    let v = normalize(-pos);
    // Reflect around the lobe's dominant direction (as `shade_surface` does for the sky).
    let r = reflect(-v, n);
    let dir = normalize(mix(n, r, (1.0 - alpha) * (sqrt(1.0 - alpha) + alpha)));
    let depth = -pos.z;
    let origin = pos + n * (0.02 + 0.002 * depth);
    let noise = interleaved_gradient_noise(vec2<f32>(px) + frame.time.w * 5.588238);
    let hit = trace(origin, dir, min(depth * 3.0 + 30.0, 1000.0), noise);
    if (!hit.found) { return vec4<f32>(0.0); }

    // The hit surface must face the ray.
    let hit_px = clamp(vec2<i32>(floor(hit.uv * frame.viewport.xy)), vec2<i32>(0), size_i() - 1);
    let hit_n = normalize((frame.view * vec4<f32>(oct_decode(textureLoad(normal_input, hit_px, 0).xy), 0.0)).xyz);
    let facing = saturate(-dot(hit_n, dir) * 4.0);

    // Where the hit surface was in the previous frame.
    let prev_uv = hit.uv - textureLoad(motion_input, hit_px, 0).xy;
    let hit_depth = scene_depth(hit.uv);
    let hit_pos = view_position(hit.uv, textureLoad(depth_input, hit_px, 0));
    let hit_dist = distance(hit_pos, origin);

    // Gather over the reflection lobe's footprint: about alpha radians wide at the hit distance.
    let pixel_size = 2.0 * hit_depth / (frame.proj[1][1] * frame.viewport.y);
    let radius = min(hit_dist * alpha / max(pixel_size, 1e-5), 24.0) * frame.viewport.zw;
    var color = vec3<f32>(0.0);
    var weight = 0.0;
    let angle0 = noise * TAU;
    for (var i = 0; i < GATHER_TAPS; i++) {
        // Vogel disc.
        let t = (f32(i) + 0.5) / f32(GATHER_TAPS);
        let a = angle0 + f32(i) * 2.39996323;
        let o = vec2<f32>(cos(a), sin(a)) * sqrt(t) * radius;
        let p = prev_uv + o;
        let w = edge_fade(p);
        color += textureSampleLevel(previous_color, linear_clamp, p, 0.0).rgb * w;
        weight += w;
    }
    if (weight <= 1e-3) { return vec4<f32>(0.0); }
    color = color / weight / frame.reflections.y;

    var coverage = facing * edge_fade(hit.uv) * edge_fade(prev_uv);
    coverage *= 1.0 - smoothstep(0.75, 1.0, hit.along);
    coverage *= 1.0 - smoothstep(max_roughness * 0.7, max_roughness, roughness);
    // Rays toward the camera mostly hit the backs of things the camera can't see.
    coverage *= 1.0 - smoothstep(0.3, 0.8, dir.z);
    return vec4<f32>(min(color, vec3<f32>(60000.0)), saturate(coverage));
}
