// Ground-truth ambient occlusion (GTAO; Jimenez et al. 2016, "Practical Real-Time Strategies
// for Accurate Indirect Occlusion", following Intel's XeGTAO) from the depth prepass, then an
// edge-aware 4x4 blur. Requires lib_types + lib_frame (group 0 is the prepass frame group, whose
// screen-space maps are stand-ins; the real inputs are in group 1).
//
// For every pixel, a few slices through the view direction are searched in both directions for
// the highest horizon within `RenderSettings::ao_radius`; the cosine-weighted visible arc
// between the two horizons, around the pixel's normal, is its ambient visibility. Slice angles
// and step offsets follow a 4x4 pattern that the blur averages out exactly; with temporal
// anti-aliasing the pattern also turns every frame, so the history integrates many more
// directions.

@group(1) @binding(0) var ao_input: texture_2d<f32>;      // blur: raw occlusion; main pass: unused
@group(1) @binding(1) var normal_input: texture_2d<f32>;  // prepass normals (octahedral xy, roughness, written)
@group(1) @binding(2) var depth_input: texture_depth_2d;  // prepass depth (reverse-Z)

const SLICES: i32 = 3;
const STEPS: i32 = 4;
const HALF_PI: f32 = 1.57079632679;

@vertex
fn vs_ao(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

fn bayer4(p: vec2<i32>) -> f32 {
    let m = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let q = p & vec2<i32>(3);
    return m[q.y * 4 + q.x] / 16.0;
}

fn depth_size() -> vec2<i32> {
    return vec2<i32>(textureDimensions(depth_input));
}

fn view_pos_at(px: vec2<i32>) -> vec3<f32> {
    let z = textureLoad(depth_input, px, 0);
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(depth_size());
    return view_position(uv, z);
}

fn view_normal_at(px: vec2<i32>) -> vec3<f32> {
    let n = oct_decode(textureLoad(normal_input, px, 0).xy);
    return normalize((frame.view * vec4<f32>(n, 0.0)).xyz);
}

@fragment
fn fs_gtao(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
    let size = depth_size();
    let z = textureLoad(depth_input, px, 0);
    if (z <= 0.0 || textureLoad(normal_input, px, 0).w < 0.5) {
        return vec4<f32>(1.0);
    }
    let pos = view_pos_at(px);
    let n = view_normal_at(px);
    let v = normalize(-pos);
    let radius = frame.effects.w;
    // Size of one pixel in view space at this depth.
    var pixel_size = 2.0 / (frame.proj[1][1] * frame.viewport.y);
    if (frame.exposure.w < 0.5) {
        pixel_size *= -pos.z;
    }
    let screen_radius = min(radius / pixel_size, 192.0);
    if (screen_radius < 1.5) {
        return vec4<f32>(1.0);
    }
    let falloff_range = 0.6 * radius;
    let falloff_mul = -1.0 / falloff_range;
    let falloff_add = (radius - falloff_range) / falloff_range + 1.0;

    // Spatial 4x4 pattern (the blur cancels it), turned every frame under TAA.
    var temporal = 0.0;
    if (any(frame.jitter.zw != vec2<f32>(0.0))) {
        temporal = fract(f32(u32(frame.time.w) % 64u) * 0.618034);
    }
    let noise_slice = fract(bayer4(px) + temporal);
    let noise_step = fract(bayer4(px.yx + vec2<i32>(1, 2)) + temporal * 1.618034);

    var visibility = 0.0;
    for (var s = 0; s < SLICES; s++) {
        let phi = (f32(s) + noise_slice) / f32(SLICES) * PI;
        let dir = vec3<f32>(cos(phi), sin(phi), 0.0);
        let omega = vec2<f32>(dir.x, -dir.y); // screen pixels, y down
        let ortho_dir = dir - dot(dir, v) * v;
        let axis = normalize(cross(ortho_dir, v));
        let proj_n = n - axis * dot(n, axis);
        let proj_len = length(proj_n);
        let sign_n = select(-1.0, 1.0, dot(ortho_dir, proj_n) >= 0.0);
        let cos_n = saturate(dot(proj_n, v) / max(proj_len, 1e-5));
        let angle_n = sign_n * acos(cos_n);
        let low0 = cos(angle_n + HALF_PI);
        let low1 = cos(angle_n - HALF_PI);
        var horizon0 = low0;
        var horizon1 = low1;
        for (var k = 0; k < STEPS; k++) {
            var t = (f32(k) + fract(noise_step + f32(s * STEPS + k) * 0.618034)) / f32(STEPS);
            t = t * t;
            let offset = omega * (t * screen_radius + 1.3);
            let p0 = vec2<i32>(floor(vec2<f32>(px) + 0.5 + offset));
            let p1 = vec2<i32>(floor(vec2<f32>(px) + 0.5 - offset));
            if (all(p0 >= vec2<i32>(0)) && all(p0 < size) && textureLoad(depth_input, p0, 0) > 0.0) {
                let d = view_pos_at(p0) - pos;
                let len = length(d);
                let w = saturate(len * falloff_mul + falloff_add);
                horizon0 = max(horizon0, mix(low0, dot(d / max(len, 1e-6), v), w));
            }
            if (all(p1 >= vec2<i32>(0)) && all(p1 < size) && textureLoad(depth_input, p1, 0) > 0.0) {
                let d = view_pos_at(p1) - pos;
                let len = length(d);
                let w = saturate(len * falloff_mul + falloff_add);
                horizon1 = max(horizon1, mix(low1, dot(d / max(len, 1e-6), v), w));
            }
        }
        var h0 = -acos(clamp(horizon1, -1.0, 1.0));
        var h1 = acos(clamp(horizon0, -1.0, 1.0));
        h0 = angle_n + clamp(h0 - angle_n, -HALF_PI, HALF_PI);
        h1 = angle_n + clamp(h1 - angle_n, -HALF_PI, HALF_PI);
        let sin_n = sin(angle_n);
        let arc0 = (cos_n + 2.0 * h0 * sin_n - cos(2.0 * h0 - angle_n)) * 0.25;
        let arc1 = (cos_n + 2.0 * h1 * sin_n - cos(2.0 * h1 - angle_n)) * 0.25;
        visibility += proj_len * (arc0 + arc1);
    }
    visibility = saturate(visibility / f32(SLICES));
    // Fade out with distance (the effect gets sub-pixel and noisy far away).
    let fade = smoothstep(150.0, 300.0, -pos.z);
    return vec4<f32>(mix(visibility, 1.0, fade));
}

// Edge-aware 4x4 box blur: averages exactly one period of the noise pattern, skipping samples
// on other surfaces (depth or normal differ).
@fragment
fn fs_blur(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
    let size = depth_size();
    let z = textureLoad(depth_input, px, 0);
    if (z <= 0.0) {
        return vec4<f32>(1.0);
    }
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(size);
    let depth = linear_depth(uv, z);
    let n = oct_decode(textureLoad(normal_input, px, 0).xy);
    var sum = 0.0;
    var weight = 0.0;
    for (var y = -2; y <= 1; y++) {
        for (var x = -2; x <= 1; x++) {
            let q = clamp(px + vec2<i32>(x, y), vec2<i32>(0), size - 1);
            let zq = textureLoad(depth_input, q, 0);
            if (zq <= 0.0) { continue; }
            let uvq = (vec2<f32>(q) + 0.5) / vec2<f32>(size);
            let dq = linear_depth(uvq, zq);
            let nq = oct_decode(textureLoad(normal_input, q, 0).xy);
            let w = saturate(1.0 - abs(dq - depth) / (0.03 * depth + 0.05)) * saturate(dot(n, nq) * 4.0 - 3.0);
            sum += textureLoad(ao_input, q, 0).r * w;
            weight += w;
        }
    }
    if (weight < 1e-3) {
        return vec4<f32>(textureLoad(ao_input, px, 0).r);
    }
    return vec4<f32>(sum / weight);
}
