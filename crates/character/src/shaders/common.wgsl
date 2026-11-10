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
    brows: vec4<f32>,         // eumelanin, red, grey, the hair's part (rest x off the head's center, m; 0 none)
    beard: vec4<f32>,         // eumelanin, red, grey, density
    eye: vec4<f32>,           // melanin, lipochrome, limbal ring, pupil (0..1)
    eye2: vec4<f32>,          // redness, debug view, subsurface pass on (1: the renderer blurs the skin's diffuse light), unused
    rest_eye_l: vec4<f32>,    // rest-space eye center, radius
    rest_eye_r: vec4<f32>,
    rest_head: vec4<f32>,
    rest_mouth: vec4<f32>,
    top: vec4<f32>,           // shirt color (linear), style (0 shirt, 1 t-shirt, 2 none, 3 gambeson, 4 mail)
    bottom: vec4<f32>,        // trousers color, style (0 trousers, 1 shorts, 2 skirt, 3 hose and boots)
    accent: vec4<f32>,        // accent color, where (0 nowhere, 1 left armband, 2 right, 3 collar, 4 tabard)
    thigh: vec4<f32>,         // a skirt's left thigh capsule: hip joint (base mesh space), radius
    knee: vec4<f32>,          // knee joint, radius (the right thigh mirrors; radius 0: no skirt)
    // Where the lids cross each eye: left upper, left lower, right upper, right lower, each
    // y = a + b x + c x^2 over the eye's own x (unit radius).
    lids: array<vec4<f32>, 4>,
    // The hair shadow grid: low corner (model space before the pose) and cell size, then cells
    // along x, y, z (0: no grid).
    hair_grid: array<vec4<f32>, 2>,
};

@group(1) @binding(0) var<uniform> character: Character;

// This frame's pose and expression, and last frame's (for motion vectors). Must match
// `GpuPose` in feature.rs.
struct Pose {
    info: vec4<u32>,                     // bones, posed (1) or at rest (0), head bone, unused
    gaze: vec4<f32>,                     // yaw, pitch this frame; yaw, pitch last frame (rad)
    eye_l: vec4<f32>,                    // eye centers (model space before posing)
    eye_r: vec4<f32>,
    wear: vec4<f32>,                     // dirt, blood, bruises, wet (Appearance::wear, 0..1)
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
// Fingers have their own bones (rig.rs); a point and normal before skinning.
struct Bent {
    p: vec3<f32>,
    n: vec3<f32>,
};

// A skirt point pushed out of both thighs (`bones`: hips, left and right upper leg, as the
// skirt is skinned), so a striding leg carries the skirt instead of poking through it.
// How far outside the thighs a shell round the legs keeps (0: not a shell): a skirt, a coat's
// skirt, a tabard over it.
fn shell_margin(part: u32) -> f32 {
    switch part {
        case 5u: { return 0.012; }
        case 7u: { return 0.016; }
        case 8u: { return 0.032; }
        default: { return 0.0; }
    }
}

// (`margin`: how far outside the thigh, m: shells over shells stand further out.)
fn clear_thighs(p: vec3<f32>, bones: vec4<u32>, prev: bool, margin: f32) -> vec3<f32> {
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
        let r = mix(character.thigh.w, character.knee.w, t) + margin;
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
// Rest height of the tops of tall boots (with hose), below the knees.
const BOOT_TOP: f32 = -0.5;

// How far inside a crew-neck shirt a rest position is, m (negative outside): below the
// neckline, which dips in front, or out on the shoulders, wider than the neck.
fn collar_depth(r: vec3<f32>) -> f32 {
    let d = vec2<f32>(r.x, r.z - character.rest_head.z + 0.012);
    let theta = atan2(d.x, d.y);
    let front = cos(theta) * 0.5 + 0.5;
    // A gambeson's collar stands up the neck, lower in front under the chin (clothes.rs).
    let neckline = select(0.565 - 0.03 * front, 0.597 - 0.042 * front, padded());
    let shoulders = min(length(d) - 0.082, 0.6 - r.y);
    // A smooth union, so the neckline runs in one curve from the front dip to the back.
    let a = neckline - r.y;
    let k = 0.008;
    let h = saturate(0.5 + 0.5 * (a - shoulders) / k);
    return mix(shoulders, a, h) + k * h * (1.0 - h);
}

// Whether the top is padded: a gambeson, or mail over one.
fn padded() -> bool {
    let top = u32(character.top.w + 0.5);
    return top == 3u || top == 4u;
}

// Whether the legs are in hose and boots.
fn hose() -> bool {
    return u32(character.bottom.w + 0.5) == 3u;
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

// 1 on the outfit's accent: a band round one sleeve, the collar, or a tabard.
fn accent_band(r: vec3<f32>, collar: f32) -> f32 {
    let at = u32(character.accent.w + 0.5);
    if (at == 3u) {
        return collar;
    }
    if (at == 4u) {
        return tabard_band(r);
    }
    if (at == 0u || (at == 1u) != (r.x > 0.0)) {
        return 0.0;
    }
    // Just above a t-shirt's hem, or high on a long sleeve.
    let hem = select(0.115, SLEEVE_HEM - 0.004, u32(character.top.w + 0.5) == 1u);
    let past = sleeve_past(r, hem);
    return smoothstep(-0.041, -0.039, past) * smoothstep(0.001, -0.001, past);
}

// A tabard over the chest and back (its panels below the belt are a shell of their own,
// `skirt::TABARD`): a band down the middle, over the shoulders, short of the collar.
const TABARD_HALF: f32 = 0.125;
fn tabard_band(r: vec3<f32>) -> f32 {
    if (r.y < WAIST - 0.01) {
        return 0.0;
    }
    let half = TABARD_HALF + 0.03 * smoothstep(0.42, 0.52, r.y);
    let side = smoothstep(half + 0.002, half - 0.002, abs(r.x));
    let collar = smoothstep(0.008, 0.014, collar_depth(r));
    return side * collar;
}

// 1 where a mail hauberk's rings are (over the gambeson): the torso, the upper arms to just
// past the elbows, short of the padded collar; not under a tabard.
const MAIL_SLEEVE: f32 = 0.25;
fn mail_cover(r: vec3<f32>) -> f32 {
    if (u32(character.top.w + 0.5) != 4u || r.y < WAIST - 0.01) {
        return 0.0;
    }
    let sleeve = smoothstep(0.002, -0.002, sleeve_past(r, MAIL_SLEEVE));
    let collar = smoothstep(0.02, 0.026, collar_depth(r));
    var tabard = 0.0;
    if (u32(character.accent.w + 0.5) == 4u) {
        tabard = tabard_band(r);
    }
    return sleeve * collar * (1.0 - tabard);
}

// How much of a padded top shows its quilting: not under mail or a tabard.
fn quilted(r: vec3<f32>) -> f32 {
    var tabard = 0.0;
    if (u32(character.accent.w + 0.5) == 4u) {
        tabard = tabard_band(r);
    }
    return (1.0 - mail_cover(r)) * (1.0 - tabard);
}

// Mail rings in a surface's own coordinates (`uv`, m: around and down): rows of riveted rings,
// each linking four, as a staggered lattice of rings. Returns how much ring covers the point
// (0 in the gaps), the ring's height across its wire (0..1), and a per-ring random. `footprint`
// (m per pixel) fades the rings to their average where they get smaller than a pixel.
const MAIL_PITCH: f32 = 0.0085;
fn mail_rings(uv: vec2<f32>, footprint: f32) -> vec3<f32> {
    let radius = MAIL_PITCH * 0.62;
    let wire = MAIL_PITCH * 0.17;
    var cover = 0.0;
    var height = 0.0;
    var id = 0.0;
    for (var row = 0; row < 2; row++) {
        // Every other row shifted half a ring: rings overlap their neighbors below and beside.
        let shift = vec2<f32>(f32(row) * 0.5, f32(row) * 0.5);
        let g = uv / MAIL_PITCH - shift;
        let cell = floor(g + 0.5);
        let d = (g - cell) * MAIL_PITCH;
        // Rings lean down and outward like scales, so their lower half stands out.
        let dist = length(d * vec2<f32>(1.0, 1.15));
        let across = abs(dist - radius) / wire;
        let on = 1.0 - smoothstep(0.75, 1.0, across);
        let h = sqrt(saturate(1.0 - across * across)) * (0.75 + 0.25 * smoothstep(0.0, -radius, d.y));
        if (on > cover) {
            cover = on;
            height = h;
            id = hash13(vec3<f32>(cell, f32(row)));
        }
    }
    // Smaller than a pixel: what the eye sees is the average of rings and gaps.
    let vis = saturate(MAIL_PITCH * 0.6 / max(footprint, 1e-7) - 1.0);
    return vec3<f32>(mix(0.62, cover, vis), mix(0.45, height, vis), id);
}

// Mail coordinates on the body at rest position `r`: around the torso and down it, or around
// and along an arm.
fn mail_uv(r: vec3<f32>) -> vec2<f32> {
    let m = vec3<f32>(select(-1.0, 1.0, r.x > 0.0), 1.0, 1.0);
    if (sleeve_past(r, 0.0) > 0.0) {
        let up = along_limb(r, SHOULDER * m, ELBOW * m);
        return vec2<f32>(up.z * 0.05, -up.x);
    }
    return vec2<f32>(atan2(r.x, r.z) * 0.15, r.y);
}

// A gambeson's quilting at rest position `r`: channels stitched down the torso and round the
// sleeves. Returns the padding's puff (0 on a seam .. 1 between) and how much seam is here.
const QUILT: f32 = 0.048;
fn quilt(r: vec3<f32>) -> vec2<f32> {
    let m = vec3<f32>(select(-1.0, 1.0, r.x > 0.0), 1.0, 1.0);
    var s = atan2(r.x, r.z) * 0.15 / QUILT;
    if (sleeve_past(r, 0.0) > 0.0) {
        let up = along_limb(r, SHOULDER * m, ELBOW * m);
        let lo = along_limb(r, ELBOW * m, WRIST * m);
        s = select(up.x, length(ELBOW - SHOULDER) + lo.x, lo.x >= 0.0) / (QUILT * 0.75);
    }
    let f = fract(s);
    let puff = sqrt(max(sin(f * 3.14159265), 0.0));
    let seam = 1.0 - smoothstep(0.0, 0.07, min(f, 1.0 - f));
    return vec2<f32>(puff, seam);
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
    var thickness = array<f32, 3>(0.004, 0.006, 0.0);
    let g = garment(r);
    var quilting = 0.0;
    if (padded() && covered > 0.0) {
        // Padding a centimetre thick, mail a little more; the quilted channels puff out.
        thickness[0] = select(0.011, 0.013, u32(character.top.w + 0.5) == 4u);
        quilting = quilt(r).x * 0.002 * quilted(r);
    }
    if (hose()) {
        // Hose hug the legs; boot shafts stand off them.
        thickness[1] = select(0.0025, 0.006, r.y < BOOT_TOP);
    }
    return covered * (thickness[g] + cloth_folds(r, g).x + select(0.0, quilting, g == 0u));
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
            if (hose()) {
                // Knit hose crease little; boots wrinkle at the ankle.
                let ankle = exp(-sq((r.y - (SHOE_TOP + 0.04)) / 0.03));
                amp = select(0.0005 + 0.0015 * knee, 0.0006 + 0.003 * ankle, r.y < BOOT_TOP);
                f = fold_rings(s, angle, r, 0.03);
            }
        }
    }
    return vec2<f32>(amp * (f.x - 0.5) * f.y, (1.0 - f.x) * (1.0 - f.x) * f.y * saturate(amp / 0.005));
}

// ---------------------------------------------------------------- hair shadow
// The hair shadow grid (strands::shadow_grid): per cell, the optical depth of crossing its hair
// (r) and its body (g), each as a share of SHADOW_BODY_TAU.
@group(1) @binding(4) var hair_shadow: texture_3d<f32>;
@group(1) @binding(5) var hair_shadow_sampler: sampler;

const SHADOW_REACH: u32 = 24u;
const SHADOW_BODY_TAU: f32 = 4.0;

// Optical depth of the hair (and with `body` 1, the body) between `p` and the outside toward
// `dir` (both in model space before the pose), marched through the grid a cell a step from a
// cell out (what is at `p` itself is not in its way). `jitter` (0..1, per pixel) offsets the
// steps so they don't band.
fn hair_shadow_depth(p: vec3<f32>, dir: vec3<f32>, jitter: f32, body: f32) -> f32 {
    let size = character.hair_grid[1].xyz;
    if (size.x <= 0.0) {
        return 0.0;
    }
    let cell = character.hair_grid[0].w;
    let start = (p - character.hair_grid[0].xyz) / cell;
    var tau = 0.0;
    for (var i = 0u; i < SHADOW_REACH; i++) {
        let q = start + dir * (f32(i) + 1.0 + jitter);
        if (any(q < vec3<f32>(0.0)) || any(q > size)) {
            break;
        }
        let d = textureSampleLevel(hair_shadow, hair_shadow_sampler, q / size, 0.0).rg;
        tau += d.r + d.g * body;
    }
    return tau * SHADOW_BODY_TAU;
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
    // Grey hair: the strands that lost their pigment. Even the whitest head keeps some
    // pigmented strands (MetaHuman sets grey as a share of white hairs, "salt and pepper"),
    // which give white hair its depth; all white it reads as a cloud.
    let white = vec3<f32>(0.78, 0.76, 0.72) * (0.85 + 0.25 * random);
    let is_grey = select(0.0, 1.0, fract(random * 7.31) < grey * 0.85);
    return mix(pigmented, white, is_grey);
}
