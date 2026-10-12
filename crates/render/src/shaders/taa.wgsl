// Temporal anti-aliasing resolve. Requires lib_types + lib_frame (group 0: the scene frame
// group, for the matrices and the prepass maps).
//
// Every frame the projection is shifted by a sub-pixel jitter (`frame.jitter`). This pass
// reconstructs the current pixel from its 3x3 neighborhood (weighted by where each jittered
// sample really lies), finds where the pixel was last frame (prepass motion vectors, or the
// camera's motion and the depth for everything else, taken from the nearest surface in the
// neighborhood so silhouettes reproject cleanly), samples the history there with a Catmull-Rom
// filter, clips it to the color range of the neighborhood (no ghosting when things change) and
// blends. Colors are compared in a compressed space (c / (1 + luma)) so very bright pixels don't
// flicker.

struct TaaParams {
    // x: history valid (0/1), y: frames accumulated since the last reset, z: minimum weight of
    // the current frame, w: unused
    params: vec4<f32>,
};

@group(1) @binding(0) var current_tex: texture_2d<f32>;
@group(1) @binding(1) var history_tex: texture_2d<f32>;
@group(1) @binding(2) var motion_tex: texture_2d<f32>;
@group(1) @binding(3) var depth_tex: texture_depth_2d;   // final scene depth (reverse-Z)
@group(1) @binding(4) var<uniform> taa: TaaParams;

@vertex
fn vs_taa(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

fn compress(c: vec3<f32>) -> vec3<f32> {
    return c / (1.0 + luminance(c));
}

fn decompress(c: vec3<f32>) -> vec3<f32> {
    return c / max(1.0 - luminance(c), 1e-4);
}

fn rgb_to_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b,
    );
}

fn ycocg_to_rgb(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

fn load_current(px: vec2<i32>, size: vec2<i32>) -> vec3<f32> {
    let c = textureLoad(current_tex, clamp(px, vec2<i32>(0), size - 1), 0).rgb;
    // NaN guard: one bad pixel must never spread through the history. Values past the half-float
    // range (the sun's disk, a lamp's core) are clamped, not dropped, so they still bloom.
    if (any(c != c)) {
        return vec3<f32>(0.0);
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(65000.0));
}

// Screen motion (uv, current - previous) of the surface at pixel `px`.
fn pixel_motion(px: vec2<i32>, size: vec2<i32>) -> vec2<f32> {
    let z = textureLoad(depth_tex, px, 0);
    // The prepass drew this surface: use its motion vector (moving objects included).
    let zp = textureLoad(prepass_depth, px, 0);
    if (zp > 0.0 && abs(z - zp) <= 1e-6 + 1e-4 * zp && textureLoad(prepass_normal, px, 0).w > 0.5) {
        return textureLoad(motion_tex, px, 0).xy;
    }
    // Otherwise assume it is fixed in the world and only the camera moved.
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(size);
    let h = frame.inv_view_proj * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, z, 1.0);
    let now = frame.unjittered_view_proj * h;
    let prev = frame.prev_view_proj * vec4<f32>(h.xyz + frame.camera_delta.xyz * h.w, h.w);
    if (prev.w <= 1e-6 || now.w <= 1e-6) {
        return vec2<f32>(1e4);
    }
    return (now.xy / now.w - prev.xy / prev.w) * vec2<f32>(0.5, -0.5);
}

// 5-tap Catmull-Rom history sample (sharper than bilinear, so the image doesn't soften over time).
fn sample_history(uv: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    let pos = uv * size;
    let p1 = floor(pos - 0.5) + 0.5;
    let f = pos - p1;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let o12 = w2 / w12;
    let t0 = (p1 - 1.0) / size;
    let t3 = (p1 + 2.0) / size;
    let t12 = (p1 + o12) / size;
    var c = textureSampleLevel(history_tex, linear_clamp, vec2<f32>(t12.x, t0.y), 0.0).rgb * (w12.x * w0.y);
    c += textureSampleLevel(history_tex, linear_clamp, vec2<f32>(t0.x, t12.y), 0.0).rgb * (w0.x * w12.y);
    c += textureSampleLevel(history_tex, linear_clamp, vec2<f32>(t12.x, t12.y), 0.0).rgb * (w12.x * w12.y);
    c += textureSampleLevel(history_tex, linear_clamp, vec2<f32>(t3.x, t12.y), 0.0).rgb * (w3.x * w12.y);
    c += textureSampleLevel(history_tex, linear_clamp, vec2<f32>(t12.x, t3.y), 0.0).rgb * (w12.x * w3.y);
    let total = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    return max(c / total, vec3<f32>(0.0));
}

// Move `history` toward the center of the box until it is inside (Playdead/INSIDE TAA).
fn clip_to_box(history: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> vec3<f32> {
    let center = 0.5 * (hi + lo);
    let extent = max(0.5 * (hi - lo), vec3<f32>(1e-5));
    let offset = history - center;
    let units = abs(offset / extent);
    let m = max(units.x, max(units.y, units.z));
    if (m > 1.0) {
        return center + offset / m;
    }
    return history;
}

@fragment
fn fs_taa(in: FullscreenOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(current_tex));
    let sizef = vec2<f32>(size);
    let px = vec2<i32>(floor(in.position.xy));
    let uv = (vec2<f32>(px) + 0.5) / sizef;
    let jitter = frame.jitter.zw;

    // Neighborhood: reconstruction filter, color statistics and the nearest surface.
    var filtered = vec3<f32>(0.0);
    var filter_weight = 0.0;
    var m1 = vec3<f32>(0.0);
    var m2 = vec3<f32>(0.0);
    var nearest = px;
    var nearest_z = -1.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2<i32>(x, y);
            let q = clamp(px + o, vec2<i32>(0), size - 1);
            let c = compress(load_current(q, size));
            // The sample in pixel q was taken at q's center minus the jitter.
            let d = vec2<f32>(o) - jitter;
            let w = exp(-2.29 * dot(d, d));
            filtered += c * w;
            filter_weight += w;
            let ycc = rgb_to_ycocg(c);
            m1 += ycc;
            m2 += ycc * ycc;
            let z = textureLoad(depth_tex, q, 0);
            if (z > nearest_z) {
                nearest_z = z;
                nearest = q;
            }
        }
    }
    filtered /= filter_weight;
    if (taa.params.x < 0.5) {
        return vec4<f32>(decompress(filtered), 1.0);
    }
    let motion = pixel_motion(nearest, size);
    let history_uv = uv - motion;
    if (any(history_uv < vec2<f32>(0.0)) || any(history_uv > vec2<f32>(1.0))) {
        return vec4<f32>(decompress(filtered), 1.0);
    }
    let mean = m1 / 9.0;
    let sigma = sqrt(max(m2 / 9.0 - mean * mean, vec3<f32>(0.0)));
    let gamma = 1.25;
    let history = rgb_to_ycocg(compress(sample_history(history_uv, sizef)));
    let clipped = ycocg_to_rgb(clip_to_box(history, mean - gamma * sigma, mean + gamma * sigma));

    // Accumulate quickly after a reset, then keep a tenth of the current frame. Fast motion
    // keeps a little more of the current frame so moving things stay sharp.
    let frames = taa.params.y;
    let speed = length(motion * sizef);
    var alpha = max(1.0 / (frames + 1.0), taa.params.z);
    alpha = mix(alpha, max(alpha, 0.25), saturate(speed / 32.0));
    let result = mix(clipped, filtered, alpha);
    return vec4<f32>(decompress(max(result, vec3<f32>(0.0))), 1.0);
}
