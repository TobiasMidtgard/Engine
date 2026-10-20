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
// 0 on bare hands), folds included.
fn cloth_lift(r: vec3<f32>, sleeves: f32) -> f32 {
    let covered = smoothstep(-0.004, 0.004, collar_depth(r)) * sleeves;
    // (The shoes are their own shell; the feet stay inside it.)
    let thickness = array<f32, 3>(0.004, 0.006, 0.0);
    let g = garment(r);
    return covered * (thickness[g] + cloth_folds(r, g).x);
}

// ---------------------------------------------------------------- folds
// Joints of the base mesh at rest (the character's left; the right mirrors in x).
const SHOULDER: vec3<f32> = vec3<f32>(0.168, 0.525, 0.015);
const ELBOW: vec3<f32> = vec3<f32>(0.313, 0.349, 0.013);
const WRIST: vec3<f32> = vec3<f32>(0.431, 0.245, 0.176);
const HIP: vec3<f32> = vec3<f32>(0.11, 0.049, 0.012);
const KNEE: vec3<f32> = vec3<f32>(0.158, -0.369, 0.032);
const ANKLE: vec3<f32> = vec3<f32>(0.22, -0.745, 0.0);

// Where `r` is along the limb from `a` to `b`: x the distance along it from `a` (m), y the
// distance from its axis, z the angle around it.
fn along_limb(r: vec3<f32>, a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let ax = normalize(b - a);
    let t = dot(r - a, ax);
    let off = r - a - ax * t;
    let side = normalize(cross(ax, vec3<f32>(0.0, 0.0, 1.0)));
    let up = cross(ax, side);
    return vec3<f32>(t, length(off), atan2(dot(off, up), dot(off, side)));
}

// Folds of cloth bunched along a limb or around the body (`s`: distance along it, `angle`
// around it): wavy, broken rings, so they read as fabric rather than a spring. Returns the
// ridge (0 in a crease .. 1 on a fold) and how much fold there is here.
fn fold_rings(s: f32, angle: f32, r: vec3<f32>, wavelength: f32) -> vec2<f32> {
    let warp = (value_noise3(r * 12.0) - 0.5) * 2.0
        + sin(angle * 1.5 + value_noise3(r * 6.0) * 4.0) * 0.6;
    let ridge = sin((s / wavelength + warp) * 6.2831853) * 0.5 + 0.5;
    let present = smoothstep(0.25, 0.6, value_noise3(r * vec3<f32>(9.0, 16.0, 9.0) + 3.7));
    return vec2<f32>(ridge, present);
}

// Folds of the shirt and trousers at rest position `r` of garment `g`: height (m, larger is
// outward) and how deep in a crease it is (0..1). Sleeves bunch at the elbows and wrists, the
// shirt blouses over the belt, trouser legs crease behind the knees and break over the shoes.
fn cloth_folds(r: vec3<f32>, g: u32) -> vec2<f32> {
    let m = vec3<f32>(select(-1.0, 1.0, r.x > 0.0), 1.0, 1.0);
    var amp = 0.0;
    var f = vec2<f32>(0.5, 0.0);
    if (g == 0u) {
        let up = along_limb(r, SHOULDER * m, ELBOW * m);
        let lo = along_limb(r, ELBOW * m, WRIST * m);
        let upper = length(ELBOW - SHOULDER);
        let lower = length(WRIST - ELBOW);
        if (up.x > 0.04 && (lo.x < 0.0 && up.y < 0.07 || lo.x >= 0.0 && lo.y < 0.06)) {
            let s = select(up.x, upper + lo.x, lo.x >= 0.0);
            let angle = select(up.z, lo.z, lo.x >= 0.0);
            let e = (s - upper) / 0.055;
            let elbow = exp(-e * e);
            let cuff = smoothstep(upper + lower * 0.5, upper + lower * 0.95, s);
            amp = 0.0008 + 0.005 * elbow + 0.004 * cuff;
            f = fold_rings(s, angle, r, 0.035);
            // Fade in from the shoulder.
            amp *= smoothstep(0.04, 0.1, up.x);
        } else if (r.y < WAIST + 0.08) {
            amp = 0.004 * (1.0 - smoothstep(WAIST + 0.015, WAIST + 0.09, r.y));
            f = fold_rings(r.y, atan2(r.x, r.z) * 2.0, r, 0.035);
        }
    } else if (g == 1u) {
        let up = along_limb(r, HIP * m, KNEE * m);
        let lo = along_limb(r, KNEE * m, ANKLE * m);
        let upper = length(KNEE - HIP);
        let lower = length(ANKLE - KNEE);
        if (r.y < WAIST - 0.06) {
            let s = select(up.x, upper + lo.x, lo.x >= 0.0);
            let angle = select(up.z, lo.z, lo.x >= 0.0);
            let e = (s - upper) / 0.065;
            let knee = exp(-e * e);
            let hem = smoothstep(upper + lower - 0.16, upper + lower, s);
            amp = 0.0008 + 0.004 * knee + 0.006 * hem;
            f = fold_rings(s, angle, r, 0.045);
        }
    }
    return vec2<f32>(amp * (f.x - 0.5) * f.y, (1.0 - f.x) * (1.0 - f.x) * f.y * saturate(amp / 0.005));
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
