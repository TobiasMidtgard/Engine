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
    top: vec4<f32>,           // shirt color (linear), style (0 shirt, 1 t-shirt, 2 none)
    bottom: vec4<f32>,        // trousers color, style (0 trousers, 1 shorts, 2 skirt)
    accent: vec4<f32>,        // accent color, where (0 nowhere, 1 left armband, 2 right, 3 collar)
    thigh: vec4<f32>,         // a skirt's left thigh capsule: hip joint (base mesh space), radius
    knee: vec4<f32>,          // knee joint, radius (the right thigh mirrors; radius 0: no skirt)
    // Per hand, left then right (base mesh space): knuckles, finger length; along the hand,
    // its bone; out of the palm, thumb side of along x palm (+-1); wrist; thumb tip. Length 0:
    // no hand.
    hands: array<vec4<f32>, 10>,
};

@group(1) @binding(0) var<uniform> character: Character;

// This frame's pose and expression, and last frame's (for motion vectors). Must match
// `GpuPose` in feature.rs.
struct Pose {
    info: vec4<u32>,                     // bones, posed (1) or at rest (0), head bone, unused
    gaze: vec4<f32>,                     // yaw, pitch this frame; yaw, pitch last frame (rad)
    eye_l: vec4<f32>,                    // eye centers (model space before posing)
    eye_r: vec4<f32>,
    grip: vec4<f32>,                     // left, right hand grip (0..1); last frame's
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

// ---------------------------------------------------------------- hands
// Finger curl (hands.rs, whose `curl` this mirrors): the fingers bend at their three joints,
// palm side inside, as far as a fist's at a full grip; the thumb bends at its own and turns
// toward the palm.
const THUMB_TURN: f32 = 0.9;
const KNUCKLES: f32 = 0.45;
const HAND: f32 = 0.177;
const THUMB_ROOT: f32 = 0.015;
const JOINT_SPAN: f32 = 0.007;
const DIGIT_STEPS: u32 = 16u;
const FINGER_BENDS: vec3<f32> = vec3<f32>(1.6, 1.9, 1.25);
const THUMB_BENDS: vec3<f32> = vec3<f32>(0.0, 0.7, 1.0);

struct Bent {
    p: vec3<f32>,
    n: vec3<f32>,
};

// `n` turned in the plane of `a` and `b`, from `a` toward `b`.
fn turn(n: vec3<f32>, a: vec3<f32>, b: vec3<f32>, s: f32, c: f32) -> vec3<f32> {
    let x = dot(n, a);
    let y = dot(n, b);
    return n - a * x - b * y + a * (x * c - y * s) + b * (x * s + y * c);
}

// How far a digit has bent by `t` along it from its root (joints at 0, 0.45 and 0.75 of `len`).
fn digit_angle(t: f32, len: f32, span: f32, bends: vec3<f32>) -> f32 {
    return bends.x * smoothstep(-span, span, t)
        + bends.y * smoothstep(0.45 * len - span, 0.45 * len + span, t)
        + bends.z * smoothstep(0.75 * len - span, 0.75 * len + span, t);
}

// A point and normal of a finger (or thumb) bent at its joints, palm side (`up`) inside: the
// finger's bent middle line traced out to the point, the point's height above it carried
// round with it.
fn bend_digit(b: Bent, root: vec3<f32>, along: vec3<f32>, up: vec3<f32>, len: f32,
    bends: vec3<f32>, span: f32) -> Bent {
    let d = b.p - root;
    let s = dot(d, along);
    if (s <= -span || max(bends.x, max(bends.y, bends.z)) <= 0.0) {
        return b;
    }
    let step = (s + span) / f32(DIGIT_STEPS);
    var line = along * -span;
    for (var i = 0u; i < DIGIT_STEPS; i++) {
        let f = digit_angle(-span + (f32(i) + 0.5) * step, len, span, bends);
        line += (along * cos(f) + up * sin(f)) * step;
    }
    let height = dot(d, up);
    let f = digit_angle(s, len, span, bends);
    let sn = sin(f);
    let cs = cos(f);
    return Bent(
        b.p + line - along * s - up * height + (up * cs - along * sn) * height,
        turn(b.n, along, up, sn, cs),
    );
}

// A point and normal (base mesh space) of a hand bent by this frame's grip, or last frame's.
fn curl_hand(p0: vec3<f32>, n0: vec3<f32>, bones: vec4<u32>, w: vec4<f32>, prev: bool) -> Bent {
    var out = Bent(p0, n0);
    for (var side = 0u; side < 2u; side++) {
        let o = side * 5u;
        let k = character.hands[o];
        let a = character.hands[o + 1u];
        if (k.w <= 0.0) {
            continue;
        }
        let bone = u32(a.w + 0.5);
        let on = dot(w, vec4<f32>(bones == vec4<u32>(bone)));
        let g = select(pose.grip.xy, pose.grip.zw, prev);
        let grip = saturate(select(g.x, g.y, side == 1u)) * smoothstep(0.3, 0.7, on);
        if (grip <= 0.0) {
            continue;
        }
        let along = a.xyz;
        let palm = character.hands[o + 2u].xyz;
        let thumb = cross(along, palm) * character.hands[o + 2u].w;
        let wrist = character.hands[o + 3u].xyz;
        let tip = character.hands[o + 4u].xyz;
        let size = k.w / (1.0 - KNUCKLES) / HAND;
        let v = out.p - wrist;
        let edge = min(0.012 + 0.42 * dot(v, along) / size, 0.05) * size;
        // Not the wrist (or a cuff around it) on the thumb's side.
        let thumbness = smoothstep(edge - 0.004, edge + 0.004, dot(v, thumb))
            * smoothstep(0.0, 0.02 * size, dot(v, along));
        let span = JOINT_SPAN * size;
        out = bend_digit(out, k.xyz, along, palm, k.w, FINGER_BENDS * (grip * (1.0 - thumbness)), span);
        let root = wrist + thumb * (THUMB_ROOT * size);
        let toward = normalize(tip - root);
        // The thumb flexes across the palm, toward the little finger.
        let across = palm * 0.3 - thumb;
        let up = normalize(across - toward * dot(across, toward));
        out = bend_digit(out, root, toward, up, length(tip - root), THUMB_BENDS * (grip * thumbness), span);
        // The thumb turns about an axis along the hand, at its root, toward the palm.
        let t = grip * THUMB_TURN * thumbness;
        if (t > 0.0) {
            let q = out.p - root;
            let x = dot(q, thumb);
            let y = dot(q, palm);
            let sn = sin(t);
            let cs = cos(t);
            out = Bent(
                out.p + thumb * (x * cs - y * sn - x) + palm * (x * sn + y * cs - y),
                turn(out.n, thumb, palm, sn, cs),
            );
        }
    }
    return out;
}

// A skirt point pushed out of both thighs (`bones`: hips, left and right upper leg, as the
// skirt is skinned), so a striding leg carries the skirt instead of poking through it.
fn clear_thighs(p: vec3<f32>, bones: vec4<u32>, prev: bool) -> vec3<f32> {
    if (pose.info.y == 0u || character.thigh.w <= 0.0) {
        return p;
    }
    let o = select(0u, pose.info.x, prev);
    var q = p;
    for (var s = 0u; s < 2u; s++) {
        let m = vec3<f32>(select(1.0, -1.0, s == 1u), 1.0, 1.0);
        let b = pose.bones[o + select(bones.y, bones.z, s == 1u)];
        let a = (b * vec4<f32>(character.thigh.xyz * m, 1.0)).xyz;
        let k = (b * vec4<f32>(character.knee.xyz * m, 1.0)).xyz;
        let ab = k - a;
        let t = clamp(dot(q - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
        let c = a + ab * t;
        let r = mix(character.thigh.w, character.knee.w, t) + 0.012;
        let d = q - c;
        let len = length(d);
        // Not up at the hip joint, where the skirt hangs from the hips instead.
        let reach = smoothstep(0.02, 0.15, t);
        if (len < r && reach > 0.0) {
            q += d / max(len, 1e-5) * (r - len) * reach;
        }
    }
    return q;
}

// ---------------------------------------------------------------- clothes
// Rest-space heights (m) of the waistband's top, the shoes' tops and the soles' tops.
const WAIST: f32 = 0.105;
const SHOE_TOP: f32 = -0.715;
const SOLE_TOP: f32 = -0.802;
// The outfit's cuts (clothes.rs): a t-shirt's hem down the upper arm (m from the shoulder
// joint), the shorts' hem height, and the band a woman wears with no top (lower, upper edge).
const SLEEVE_HEM: f32 = 0.12;
const SHORTS_HEM: f32 = -0.17;
const BRIEFS_HEM: f32 = -0.07;
const BAND: vec2<f32> = vec2<f32>(0.32, 0.45);

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

// How far past a sleeve's hem (`hem`: m down the upper arm from the shoulder joint) a rest
// position is, or 1e3 off the arms (`sleeve_past` in clothes.rs).
fn sleeve_past(r: vec3<f32>, hem: f32) -> f32 {
    let m = vec3<f32>(select(-1.0, 1.0, r.x > 0.0), 1.0, 1.0);
    let a = SHOULDER * m;
    let ax = normalize((ELBOW - SHOULDER) * m);
    let t = dot(r - a, ax);
    let off = length(r - a - ax * t);
    // Around the upper arm (the torso is over 0.1 m from its axis), or out on the forearm.
    return select(-1e3, t - hem, off < 0.075 || abs(r.x) > 0.19);
}

// How far inside the clothes' edges a rest position is, m (negative outside): the neckline,
// and the sleeves, shorts and band where the outfit has them (`cover_depth` in clothes.rs).
fn cover_depth(r: vec3<f32>) -> f32 {
    var d = collar_depth(r);
    let top = u32(character.top.w + 0.5);
    if (r.y >= WAIST) {
        if (top == 1u) {
            d = min(d, -sleeve_past(r, SLEEVE_HEM));
        } else if (top == 2u) {
            if (character.look.y > 0.5) {
                d = min(d, WAIST - r.y);
            } else {
                d = min(min(d, -sleeve_past(r, 0.0)), min(r.y - BAND.x, BAND.y - r.y));
            }
        }
    } else {
        let bottom = u32(character.bottom.w + 0.5);
        if (bottom == 1u) {
            d = min(d, r.y - SHORTS_HEM);
        } else if (bottom == 2u) {
            d = min(d, r.y - BRIEFS_HEM);
        }
    }
    return d;
}

// 1 on the outfit's accent: a band round one sleeve, or the collar.
fn accent_band(r: vec3<f32>, collar: f32) -> f32 {
    let at = u32(character.accent.w + 0.5);
    if (at == 3u) {
        return collar;
    }
    if (at == 0u || (at == 1u) != (r.x > 0.0)) {
        return 0.0;
    }
    // Just above a t-shirt's hem, or high on a long sleeve.
    let hem = select(0.115, SLEEVE_HEM - 0.004, u32(character.top.w + 0.5) == 1u);
    let past = sleeve_past(r, hem);
    return smoothstep(-0.041, -0.039, past) * smoothstep(0.001, -0.001, past);
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
    let covered = smoothstep(-0.004, 0.004, cover_depth(r)) * sleeves;
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
