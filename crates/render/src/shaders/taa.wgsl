// Temporal anti-aliasing resolve. Requires lib_types + lib_frame (group 0: the scene frame
// group, for the matrices and the prepass maps).
//
// Every frame the projection is shifted by a sub-pixel jitter (`frame.jitter`). This pass
// reconstructs the current pixel from its 3x3 neighborhood (weighted by where each jittered
// sample really lies), finds where the pixel was last frame (prepass motion vectors, or the
// camera's motion and the depth for everything else, taken from the nearest surface in the
// neighborhood so silhouettes reproject cleanly), samples the history there with a Catmull-Rom
// filter, clips it to the color range of the neighborhood (no ghosting when things change) and
// blends. The color box is built in a compressed space (c / (1 + luma)) so very bright pixels
// don't blow it open, while filtering and blending stay linear so small bright things (the
// sun's disk, a lamp's core) keep their energy and still bloom.
//
// Temporal upscaling (the scene smaller than the history, as Unreal's TAAU and AMD's FSR 2):
// each output pixel reconstructs this frame's value from the scene pixels around it twice, with
// a filter one scene pixel wide (smooth, for when there is no history) and one output pixel wide
// (sharp). The sharp filter's total weight says how close this frame's samples landed to the
// pixel's center, and the pixel takes that much of this frame: over the jitter sequence every
// output pixel gathers samples from all over its area and converges to full resolution. At the
// same size both filters are the plain TAA filter and the weight is about 1.
//
// Thin transparent things (snowflakes, rain, sparks) write no depth or motion and are outliers
// in their neighborhood, so the clipped history would erase them: where the reactive mask
// (taa_reactive.wgsl) marks them, the resolve keeps most of this frame's own pixel.

struct TaaParams {
    // x: history valid (0/1), y: frames accumulated since the last reset, z: minimum weight of
    // the current frame, w: strength of the reactive mask (0: ignored)
    params: vec4<f32>,
};

@group(1) @binding(0) var current_tex: texture_2d<f32>;
@group(1) @binding(1) var history_tex: texture_2d<f32>;
@group(1) @binding(2) var motion_tex: texture_2d<f32>;
@group(1) @binding(3) var depth_tex: texture_depth_2d;   // final scene depth (reverse-Z)
@group(1) @binding(4) var<uniform> taa: TaaParams;
@group(1) @binding(5) var reactive_tex: texture_2d<f32>;

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
    // Scene (input) and output sizes, and output pixels per scene pixel.
    let size = vec2<i32>(textureDimensions(current_tex));
    let sizef = vec2<f32>(size);
    let out_size = vec2<f32>(textureDimensions(history_tex));
    let ratio = out_size / sizef;
    let uv = floor(in.position.xy) / out_size + 0.5 / out_size;
    // This pixel's center in scene pixels, and the scene pixel it lies in.
    let at = uv * sizef;
    let px = clamp(vec2<i32>(floor(at)), vec2<i32>(0), size - 1);
    let jitter = frame.jitter.zw;

    // Neighborhood: reconstruction filters, color statistics and the nearest surface.
    var wide = vec3<f32>(0.0);
    var wide_weight = 0.0;
    var sharp = vec3<f32>(0.0);
    var sharp_weight = 0.0;
    var m1 = vec3<f32>(0.0);
    var m2 = vec3<f32>(0.0);
    var nearest = px;
    var nearest_z = -1.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2<i32>(x, y);
            let q = clamp(px + o, vec2<i32>(0), size - 1);
            let c = load_current(q, size);
            // The sample in pixel q was taken at q's center minus the jitter.
            let d = vec2<f32>(px + o) + 0.5 - jitter - at;
            let w = exp(-2.29 * dot(d, d));
            wide += c * w;
            wide_weight += w;
            let ds = d * ratio;
            let ws = exp(-2.29 * dot(ds, ds));
            sharp += c * ws;
            sharp_weight += ws;
            let ycc = rgb_to_ycocg(compress(c));
            m1 += ycc;
            m2 += ycc * ycc;
            let z = textureLoad(depth_tex, q, 0);
            if (z > nearest_z) {
                nearest_z = z;
                nearest = q;
            }
        }
    }
    wide /= wide_weight;
    // How much of a sample this frame put on this output pixel (about 1 at native size).
    let confidence = saturate(sharp_weight);
    var filtered = mix(wide, sharp / max(sharp_weight, 1e-6), confidence);
    let mask = saturate(taa.params.w * textureLoad(reactive_tex, px, 0).r);
    let center = load_current(px, size);
    // The history's alpha: how many frames' worth of samples it holds (see below).
    if (taa.params.x < 0.5) {
        return vec4<f32>(mix(wide, center, mask), 1.0);
    }
    let motion = pixel_motion(nearest, size);
    let history_uv = uv - motion;
    if (any(history_uv < vec2<f32>(0.0)) || any(history_uv > vec2<f32>(1.0))) {
        return vec4<f32>(mix(wide, center, mask), 1.0);
    }
    let held = textureSampleLevel(history_tex, linear_clamp, history_uv, 0.0).a;
    let mean = m1 / 9.0;
    let sigma = sqrt(max(m2 / 9.0 - mean * mean, vec3<f32>(0.0)));
    let history = rgb_to_ycocg(compress(sample_history(history_uv, out_size)));
    // A thin transparent thing that stays put (a wire fence) is in the history already, and TAA
    // smooths it as anything else; one that moved since (a falling flake) isn't: the history
    // there holds what is behind it.
    var reactive = 0.0;
    if (mask > 0.0) {
        let own = rgb_to_ycocg(compress(center));
        let gap = abs(history.x - own.x) / (abs(own.x - mean.x) + 1e-3);
        reactive = mask * saturate(2.5 * gap - 1.0);
    }
    // A reactive pixel is its own sample: the filter would spread a one-pixel flake thin.
    filtered = mix(filtered, center, reactive);
    let gamma = 1.25;
    // The box can't hold the current sample if that is an outlier (a sub-pixel highlight):
    // widen it to include the filtered value.
    let current = rgb_to_ycocg(compress(filtered));
    let lo = min(mean - gamma * sigma, current);
    let hi = max(mean + gamma * sigma, current);
    // When upscaling, this frame's box describes the scene pixels around this output pixel only
    // as well as its samples landed near it: clip in proportion, or each output pixel would be
    // pulled toward a different frame's box and a still image would keep a fine dither.
    let clipped = decompress(ycocg_to_rgb(mix(history, clip_to_box(history, lo, hi), confidence)));

    // Each pixel averages its samples: the history holds `held` frames' worth (its alpha) and
    // this frame adds `confidence` (1 at native size), so after a reset the frames are averaged
    // evenly, then the newest keeps a tenth. Fast motion keeps a little more of the current
    // frame so moving things stay sharp. When upscaling, the samples that landed near this
    // pixel over the whole jitter sequence are averaged alike, wherever they fell in it.
    let frames = taa.params.y;
    let speed = length(motion * out_size);
    let newest = mix(taa.params.z, max(taa.params.z, 0.25), saturate(speed / 32.0));
    // Upscaling puts a sample near each pixel only every ratio² frames: keep as many frames.
    let kept = clamp(held, 0.0, min(frames, (1.0 / newest - 1.0) * ratio.x * ratio.y));
    var alpha = confidence / max(kept + confidence, 1e-4);
    alpha = max(alpha, 0.85 * reactive);
    let result = mix(clipped, filtered, alpha);
    let weight = min(kept + confidence, 1.0 / max(alpha, 1e-4));
    return vec4<f32>(max(result, vec3<f32>(0.0)), weight);
}
