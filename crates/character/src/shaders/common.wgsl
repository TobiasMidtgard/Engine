// Shared by every character pass. Requires lib_types (and lib_frame for the lighting helpers
// at the bottom, which are compiled only into scene shaders; see `CHARACTER_LIGHTING`).

// Must match `GpuCharacter` in feature.rs.
struct Character {
    model: mat4x4<f32>,       // model -> camera-relative (this frame)
    prev_model: mat4x4<f32>,  // model -> camera-relative of last frame's camera
    skin: vec4<f32>,          // melanin, redness, undertone (-1 cool .. 1 warm), oiliness
    skin2: vec4<f32>,         // freckles, moles, wrinkles (0..1, age included), pore size (0..1)
    look: vec4<f32>,          // age (years), sex (0 female .. 1 male), beard shadow (0..1), seed (0..1)
    hair: vec4<f32>,          // scalp hair: eumelanin (0..1), red (0..1), grey (0..1), scalp tint (0..1)
    brows: vec4<f32>,         // eumelanin, red, grey, unused
    beard: vec4<f32>,         // eumelanin, red, grey, density
    eye: vec4<f32>,           // melanin, lipochrome, limbal ring, pupil (0..1)
    eye2: vec4<f32>,          // redness, debug view, unused, unused
    rest_eye_l: vec4<f32>,    // rest-space eye center, radius
    rest_eye_r: vec4<f32>,
    rest_head: vec4<f32>,
    rest_mouth: vec4<f32>,
};

@group(1) @binding(0) var<uniform> character: Character;

// This frame's pose and expression, and last frame's (for motion vectors). Must match
// `GpuPose` in feature.rs.
struct Pose {
    info: vec4<u32>,                     // bones, posed (1) or at rest (0), head bone, unused
    gaze: vec4<f32>,                     // yaw, pitch this frame; yaw, pitch last frame (rad)
    eye_l: vec4<f32>,                    // eye centers (model space before posing)
    eye_r: vec4<f32>,
    weights: array<vec4<f32>, 16>,       // expression channels this frame
    prev_weights: array<vec4<f32>, 16>,  // and last frame
    bones: array<mat4x4<f32>>,           // this frame's bones, then last frame's
};

@group(1) @binding(1) var<storage, read> pose: Pose;
// Expression shapes per skin vertex: entries [offsets[v], offsets[v + 1]) of `morph_entries`,
// each (channel | dx, dy | dz, nx | ny, nz) with half-float offsets.
@group(1) @binding(2) var<storage, read> morph_offsets: array<u32>;
@group(1) @binding(3) var<storage, read> morph_entries: array<vec4<u32>>;

struct Morph {
    dp: vec3<f32>,
    dn: vec3<f32>,
    dp_prev: vec3<f32>,
};

fn channel_weight(c: u32, prev: bool) -> f32 {
    let w = select(pose.weights[c / 4u], pose.prev_weights[c / 4u], prev);
    return w[c % 4u];
}

// How the expressions of this frame (and the last) move skin vertex `v`.
fn morph_at(v: u32) -> Morph {
    var m: Morph;
    m.dp = vec3<f32>(0.0);
    m.dn = vec3<f32>(0.0);
    m.dp_prev = vec3<f32>(0.0);
    if (v + 1u >= arrayLength(&morph_offsets)) {
        return m;
    }
    let end = morph_offsets[v + 1u];
    for (var i = morph_offsets[v]; i < end; i++) {
        let e = morph_entries[i];
        let c = e.x & 0xffffu;
        let w = channel_weight(c, false);
        let wp = channel_weight(c, true);
        if (w == 0.0 && wp == 0.0) {
            continue;
        }
        let a = unpack2x16float(e.x);
        let b = unpack2x16float(e.y);
        let n0 = unpack2x16float(e.z);
        let n1 = unpack2x16float(e.w);
        let dp = vec3<f32>(a.y, b.x, b.y);
        m.dp += dp * w;
        m.dn += vec3<f32>(n0.x, n0.y, n1.x) * w;
        m.dp_prev += dp * wp;
    }
    return m;
}

// Blended bone matrix (identity at rest) for up to four bones and weights.
fn skin_matrix(bones: vec4<u32>, w: vec4<f32>, prev: bool) -> mat4x4<f32> {
    if (pose.info.y == 0u) {
        return mat4x4<f32>(
            vec4<f32>(1.0, 0.0, 0.0, 0.0),
            vec4<f32>(0.0, 1.0, 0.0, 0.0),
            vec4<f32>(0.0, 0.0, 1.0, 0.0),
            vec4<f32>(0.0, 0.0, 0.0, 1.0),
        );
    }
    let o = select(0u, pose.info.x, prev);
    return pose.bones[o + bones.x] * w.x + pose.bones[o + bones.y] * w.y
        + pose.bones[o + bones.z] * w.z + pose.bones[o + bones.w] * w.w;
}

// ---------------------------------------------------------------- clothes
// Rest-space heights (m) of the waistband's top, the shoes' tops and the soles' tops.
const WAIST: f32 = 0.105;
const SHOE_TOP: f32 = -0.715;
const SOLE_TOP: f32 = -0.802;

// How far inside a crew-neck shirt a rest position is, m (negative outside): below the
// neckline, which dips in front, or out on the shoulders, wider than the neck.
fn collar_depth(r: vec3<f32>) -> f32 {
    let d = vec2<f32>(r.x, r.z - character.rest_head.z + 0.012);
    let theta = atan2(d.x, d.y);
    let neckline = 0.565 - 0.03 * (cos(theta) * 0.5 + 0.5);
    let shoulders = min(length(d) - 0.082, 0.6 - r.y);
    // A smooth union, so the neckline runs in one curve from the front dip to the back.
    let a = neckline - r.y;
    let k = 0.008;
    let h = saturate(0.5 + 0.5 * (a - shoulders) / k);
    return mix(shoulders, a, h) + k * h * (1.0 - h);
}

// What covers a rest position below the neckline: 0 shirt, 1 trousers, 2 shoes.
fn garment(r: vec3<f32>) -> u32 {
    if (r.y < SHOE_TOP) {
        return 2u;
    }
    return select(0u, 1u, r.y < WAIST);
}

// 1 on the belt at the trousers' waistband.
fn belt_band(r: vec3<f32>) -> f32 {
    return smoothstep(WAIST - 0.036, WAIST - 0.034, r.y) * smoothstep(WAIST - 0.002, WAIST - 0.004, r.y);
}

// How far clothes stand off the skin at rest position `r` (`sleeves`: the vertex cloth mask,
// 0 on bare hands).
fn cloth_lift(r: vec3<f32>, sleeves: f32) -> f32 {
    let covered = smoothstep(-0.004, 0.004, collar_depth(r)) * sleeves;
    let thickness = array<f32, 3>(0.004, 0.006, 0.009);
    return covered * thickness[garment(r)];
}

fn unpack_bytes(v: u32) -> vec4<u32> {
    return vec4<u32>(v & 0xffu, (v >> 8u) & 0xffu, (v >> 16u) & 0xffu, v >> 24u);
}

fn model_point(p: vec3<f32>) -> vec3<f32> {
    return (character.model * vec4<f32>(p, 1.0)).xyz;
}

fn model_dir(d: vec3<f32>) -> vec3<f32> {
    return normalize((character.model * vec4<f32>(d, 0.0)).xyz);
}

fn prev_model_point(p: vec3<f32>) -> vec3<f32> {
    return (character.prev_model * vec4<f32>(p, 1.0)).xyz;
}

// ---------------------------------------------------------------- noise
fn hash33c(p: vec3<f32>) -> vec3<f32> {
    var q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yxz + 33.33);
    return fract((q.xxy + q.yxx) * q.zyx);
}

// Cellular noise: distance to the nearest and second nearest jittered cell point, and the
// nearest point's hash (for per-cell variation).
struct Cells {
    f1: f32,
    f2: f32,
    id: f32,
};

fn cells3(p: vec3<f32>) -> Cells {
    let i = floor(p);
    let f = p - i;
    var c: Cells;
    c.f1 = 8.0;
    c.f2 = 8.0;
    c.id = 0.0;
    for (var z = -1; z <= 1; z++) {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let o = vec3<f32>(f32(x), f32(y), f32(z));
                let h = hash33c(i + o);
                let d = length(o + h - f);
                if (d < c.f1) {
                    c.f2 = c.f1;
                    c.f1 = d;
                    c.id = h.x;
                } else if (d < c.f2) {
                    c.f2 = d;
                }
            }
        }
    }
    return c;
}

// ---------------------------------------------------------------- hair color
// Absorption of a hair fiber per unit length (relative) from its pigments, as measured for real
// hair (d'Eon et al. 2011): eumelanin browns and blacks, pheomelanin reds.
fn hair_absorption(eumelanin: f32, red: f32) -> vec3<f32> {
    // Pigment concentrations from the 0..1 sliders: about 0.6 for blonde, 2.3 for brown and
    // 7 for black (relative units of the measured absorption below).
    let eu = 7.0 * pow(max(eumelanin, 0.0), 2.2) + 0.4 * eumelanin;
    let pheo = red * (0.3 + 1.2 * (1.0 - eumelanin));
    return eu * vec3<f32>(0.419, 0.697, 1.37) + pheo * vec3<f32>(0.187, 0.4, 1.05);
}

// Base color of hair (what a lock of it looks like under white light).
fn hair_color(eumelanin: f32, red: f32, grey: f32, random: f32) -> vec3<f32> {
    let sigma = hair_absorption(eumelanin * (0.85 + 0.3 * random), red);
    // The cuticle reflects a little neutral light however dark the pigment, so black hair
    // stays black rather than turning dark red.
    let pigmented = exp(-sigma * 1.6) * 0.92 + vec3<f32>(0.012, 0.0115, 0.011);
    // Grey hair: the strands that lost their pigment.
    let white = vec3<f32>(0.78, 0.76, 0.72) * (0.85 + 0.25 * random);
    let is_grey = select(0.0, 1.0, fract(random * 7.31) < grey);
    return mix(pigmented, white, is_grey);
}
