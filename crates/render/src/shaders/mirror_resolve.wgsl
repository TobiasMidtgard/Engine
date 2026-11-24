// The traced mirror picture's own history (`trial_cut_more::RAY_MIRROR_PICTURES`, ray_mirror.rs
// `RayMirrors::finish`), after the trace (ray_mirror.wgsl) and before the picture's levels, with
// temporal anti-aliasing on. Each frame's ray goes through another point of its texel (the
// frame's jitter), and the texel keeps the average of its rays: this frame's blended into what
// the texel held last frame, found where last frame's mirrored view saw the texel's point (along
// its ray at its depth), and kept within what this frame's rays show around it, as the main
// view's history does with its pixels (taa.wgsl). A drawn view so resolves its own jitter in its
// own history; the main view's can't do it for the picture, which the water reads through the
// unjittered view: each frame's picture would move under it by a share of a texel, a pixel or
// more across, and a thin bright line in the reflection would hop and flicker.

struct MirrorResolve {
    // This frame's mirrored view, camera-relative, as the trace's (`MirrorTrace`): its right
    // axis (w: its near plane, m), up axis (w: 1 where last frame's picture holds this one's
    // history, else 0), back axis (w: this frame's exposure over last frame's), its eye and the
    // picture's edges as tangents.
    right: vec4<f32>,
    up: vec4<f32>,
    back: vec4<f32>,
    eye: vec4<f32>,
    tangents: vec4<f32>,
    // Last frame's, relative to this frame's camera.
    prev_right: vec4<f32>,
    prev_up: vec4<f32>,
    prev_back: vec4<f32>,
    prev_eye: vec4<f32>,
    prev_tangents: vec4<f32>,
};
@group(0) @binding(0) var<uniform> r: MirrorResolve;
// This frame's trace: rgb pre-exposed; reverse-Z depth along the mirrored view (0 the sky).
@group(0) @binding(1) var raw_color: texture_2d<f32>;
@group(0) @binding(2) var raw_depth: texture_2d<f32>;
// Last frame's picture (its level 0): rgb, a: the frames it holds.
@group(0) @binding(3) var history: texture_2d<f32>;
@group(0) @binding(4) var linear_clamp: sampler;
// This frame's picture (level 0 of its chain).
@group(0) @binding(5) var out_color: texture_storage_2d<rgba16float, write>;

// Past this many frames held, the newest keeps a tenth, as in the main view's history.
const MIRROR_KEEP_FRAMES: f32 = 9.0;
// The box around this frame's rays (their mean, this many deviations out) the history is kept in.
const MIRROR_BOX: f32 = 1.25;
// Reverse-Z depth of a texel inside something standing in the plane (ray_mirror.wgsl: the near
// plane), which says nothing of where its neighbours were.
const MIRROR_INSIDE: f32 = 0.9999;

// (`luminance` is lib_types'.)
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

// Move `h` toward the centre of the box until it is inside (taa.wgsl `clip_to_box`).
fn clip_to_box(h: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> vec3<f32> {
    let center = 0.5 * (hi + lo);
    let extent = max(0.5 * (hi - lo), vec3<f32>(1e-5));
    let offset = h - center;
    let units = abs(offset / extent);
    let m = max(units.x, max(units.y, units.z));
    if (m > 1.0) {
        return center + offset / m;
    }
    return h;
}

fn load_raw(px: vec2<i32>, size: vec2<i32>) -> vec3<f32> {
    let c = textureLoad(raw_color, clamp(px, vec2<i32>(0), size - 1), 0).rgb;
    // One bad texel must never spread through the history.
    if (any(c != c)) {
        return vec3<f32>(0.0);
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(65000.0));
}

// Last frame's picture at `uv` (Catmull-Rom from five bilinear taps, taa.wgsl
// `sample_history`, so that a moving view doesn't blur it frame by frame): rgb, and the frames
// it holds there.
fn sample_history(uv: vec2<f32>, size: vec2<f32>) -> vec4<f32> {
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
    let mid = textureSampleLevel(history, linear_clamp, t12, 0.0);
    var c = textureSampleLevel(history, linear_clamp, vec2<f32>(t12.x, t0.y), 0.0).rgb * (w12.x * w0.y);
    c += textureSampleLevel(history, linear_clamp, vec2<f32>(t0.x, t12.y), 0.0).rgb * (w0.x * w12.y);
    c += mid.rgb * (w12.x * w12.y);
    c += textureSampleLevel(history, linear_clamp, vec2<f32>(t3.x, t12.y), 0.0).rgb * (w3.x * w12.y);
    c += textureSampleLevel(history, linear_clamp, vec2<f32>(t12.x, t3.y), 0.0).rgb * (w12.x * w3.y);
    let total = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    return vec4<f32>(max(c / total, vec3<f32>(0.0)), mid.a);
}

@compute @workgroup_size(8, 8, 1)
fn cs_mirror_resolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<i32>(textureDimensions(raw_color));
    let px = vec2<i32>(id.xy);
    if (any(px >= size)) {
        return;
    }
    let center = load_raw(px, size);
    if (r.up.w < 0.5) {
        textureStore(out_color, px, vec4<f32>(center, 1.0));
        return;
    }
    // This frame's rays around the texel, and the nearest thing among them (its outline moves
    // with it, as the main view's history takes the nearest pixel's motion).
    var m1 = vec3<f32>(0.0);
    var m2 = vec3<f32>(0.0);
    var nearest = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let q = clamp(px + vec2<i32>(x, y), vec2<i32>(0), size - 1);
            let ycc = rgb_to_ycocg(compress(load_raw(q, size)));
            m1 += ycc;
            m2 += ycc * ycc;
            let z = textureLoad(raw_depth, q, 0).r;
            if (z > nearest && z < MIRROR_INSIDE) {
                nearest = z;
            }
        }
    }
    // The texel's point (along its ray through its centre at that depth: the depth is the near
    // plane over the depth along the mirrored view, and the ray is 1 along it), or for the sky
    // its direction, where last frame's mirrored view saw it.
    let t = r.tangents;
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(size);
    let d = r.right.xyz * mix(t.x, t.y, uv.x) + r.up.xyz * mix(t.w, t.z, uv.y) - r.back.xyz;
    var rel = d;
    if (nearest > 0.0) {
        rel = r.eye.xyz + d * (r.right.w / nearest) - r.prev_eye.xyz;
    }
    let ahead = -dot(rel, r.prev_back.xyz);
    let pt = r.prev_tangents;
    let tan = vec2<f32>(dot(rel, r.prev_right.xyz), dot(rel, r.prev_up.xyz)) / max(ahead, 1e-6);
    let prev_uv = vec2<f32>((tan.x - pt.x) / (pt.y - pt.x), (pt.w - tan.y) / (pt.w - pt.z));
    if (ahead <= 1e-6 || any(prev_uv < vec2<f32>(0.0)) || any(prev_uv > vec2<f32>(1.0))) {
        textureStore(out_color, px, vec4<f32>(center, 1.0));
        return;
    }
    let held = sample_history(prev_uv, vec2<f32>(textureDimensions(history)));
    let mean = m1 / 9.0;
    let sigma = sqrt(max(m2 / 9.0 - mean * mean, vec3<f32>(0.0)));
    // The box can't hold this frame's ray if that is an outlier (a sub-texel highlight): widen
    // it to include it.
    let own = rgb_to_ycocg(compress(center));
    let lo = min(mean - MIRROR_BOX * sigma, own);
    let hi = max(mean + MIRROR_BOX * sigma, own);
    let kept_rgb = decompress(ycocg_to_rgb(clip_to_box(rgb_to_ycocg(compress(held.rgb * r.back.w)), lo, hi)));
    // The frames are averaged evenly after a reset, then the newest keeps a tenth.
    let frames = clamp(held.a, 0.0, MIRROR_KEEP_FRAMES);
    let result = mix(kept_rgb, center, 1.0 / (frames + 1.0));
    textureStore(out_color, px, vec4<f32>(max(result, vec3<f32>(0.0)), frames + 1.0));
}
