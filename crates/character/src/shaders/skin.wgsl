// Skin. Requires lib_types + lib_frame + common.wgsl.
//
// The color comes from the two pigments of real skin: melanin sets the base tone (a ramp
// through measured skin colors from very fair to very dark), blood flushes the cheeks, nose,
// ears and lips (less visible the darker the skin), and the undertone tilts it warm or cool.
// Lips, eyelids, beard shadow, freckles, moles and the scalp under short hair are painted from
// region masks and cell noise anchored to the rest position (the base mesh before morphs), so
// detail stays put on the skin whatever the face shape.
//
// Relief is computed, not textured: pores (cells, larger on the nose), fine grain, the
// vertical lines of the lips, and wrinkles that deepen with age (forehead lines, crow's feet,
// lines under the eyes, neck rings). Its gradient tilts the normal.
//
// Light diffuses under the skin. With the renderer's subsurface pass on (Jimenez et al.,
// "Separable Subsurface Scattering", 2015; as Unreal's Subsurface Profile and Unity HDRP's
// diffusion profiles do), the skin is lit plainly and marks itself in the prepass; the pass
// blurs its diffuse light along the surface (red farthest) while the specular light, written
// to alpha, stays sharp. Without the pass the shader imitates it: red light is lit with a
// smoothed normal and wraps further past the terminator than green and blue, more where the
// surface curves tightly, and shadow edges turn red. Either way thin parts (ears, nostrils)
// glow when lit from behind. Highlights are two GGX lobes (a sharp oily one and a broad one),
// weaker in the pores.

struct SkinIn {
    @location(0) position_curvature: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) rest: vec4<f32>,
    @location(3) m0: vec4<f32>,  // lips, nose, eyelids, brows
    @location(4) m1: vec4<f32>,  // cheeks, beard, scalp, ears
    @location(5) m2: vec4<f32>,  // cloth, forehead, chin, lid rim
    @location(6) bones: vec4<u32>,
    @location(7) weights: vec4<f32>,
};

struct SkinOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) rest: vec3<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) curvature: f32,
    @location(7) clip_now: vec4<f32>,
    @location(8) clip_prev: vec4<f32>,
    // 0 skin, 1 teeth, 2 gums, 3 tongue, 4 shoes, 5 skirt, 6 hood.
    @location(9) @interpolate(flat) part: u32,
    // Where the point sits before the pose (the hair shadow grid's space), and that space's x
    // and y axes in world space.
    @location(10) bind: vec3<f32>,
    @location(11) axis_x: vec3<f32>,
    @location(12) axis_y: vec3<f32>,
};

@vertex
fn vs_skin(in: SkinIn, @builtin(vertex_index) vertex: u32) -> SkinOut {
    var out: SkinOut;
    let m = morph_at(vertex);
    let n0 = normalize(in.normal.xyz + m.dn);
    // Fingers curl by the hands' grip.
    let hand = curl_hand(in.position_curvature.xyz + m.dp, n0, in.bones, in.weights, false);
    let hand_prev =
        curl_hand(in.position_curvature.xyz + m.dp_prev, n0, in.bones, in.weights, true);
    let n = hand.n;
    // Clothes stand off the skin.
    let lift = n * cloth_lift(in.rest.xyz, in.m2.x);
    let now = skin_matrix(in.bones, in.weights, false);
    var p = (now * vec4<f32>(hand.p + lift, 1.0)).xyz;
    var p_prev = (skin_matrix(in.bones, in.weights, true)
        * vec4<f32>(hand_prev.p + lift, 1.0)).xyz;
    if (u32(in.normal.w + 0.5) == 5u) {
        p = clear_thighs(p, in.bones, false);
        p_prev = clear_thighs(p_prev, in.bones, true);
    }
    let rel = model_point(p);
    out.rel = rel;
    out.normal = model_dir((now * vec4<f32>(n, 0.0)).xyz);
    out.rest = in.rest.xyz;
    out.m0 = in.m0;
    out.m1 = in.m1;
    out.m2 = in.m2;
    out.curvature = in.position_curvature.w;
    out.part = u32(in.normal.w + 0.5);
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.clip_now = motion_clip(rel);
    out.clip_prev = prev_motion_clip(prev_model_point(p_prev));
    out.bind = hand.p + lift;
    out.axis_x = model_dir((now * vec4<f32>(1.0, 0.0, 0.0, 0.0)).xyz);
    out.axis_y = model_dir((now * vec4<f32>(0.0, 1.0, 0.0, 0.0)).xyz);
    return out;
}

// ---------------------------------------------------------------- color
// Skin albedo (linear) by melanin, through measured tones: very fair, fair, medium, olive/tan,
// brown, dark brown, very dark.
fn skin_tone(m: f32) -> vec3<f32> {
    let x = saturate(m) * 6.0;
    let i = min(i32(x), 5);
    let t = x - f32(i);
    var tones = array<vec3<f32>, 7>(
        vec3<f32>(0.80, 0.56, 0.47),
        vec3<f32>(0.70, 0.45, 0.35),
        vec3<f32>(0.56, 0.33, 0.22),
        vec3<f32>(0.40, 0.21, 0.12),
        vec3<f32>(0.25, 0.12, 0.065),
        vec3<f32>(0.13, 0.06, 0.033),
        vec3<f32>(0.065, 0.032, 0.019),
    );
    let s = t * t * (3.0 - 2.0 * t);
    return mix(tones[i], tones[i + 1], s);
}

// ---------------------------------------------------------------- lips
// The red of the lips (vermilion) is outlined in rest space, in mm from the base mesh's mouth
// by distance from the midline: the upper border with its cupid's bow, the lower one rounder,
// meeting at the corners. Morphs move the skin under it, so the outline fits any lip shape,
// and it is as sharp as the pixel (vertex masks are as soft as the mesh is coarse).
const LIP_UPPER = array<vec2<f32>, 9>(
    vec2<f32>(0.0, 2.6), vec2<f32>(3.0, 3.4), vec2<f32>(6.5, 4.2), vec2<f32>(10.0, 3.6),
    vec2<f32>(14.0, 2.0), vec2<f32>(18.0, -0.6), vec2<f32>(21.0, -3.0), vec2<f32>(23.5, -5.6),
    vec2<f32>(26.0, -9.5),
);
const LIP_LOWER = array<vec2<f32>, 9>(
    vec2<f32>(0.0, -18.8), vec2<f32>(6.0, -18.6), vec2<f32>(10.0, -17.8), vec2<f32>(14.0, -16.0),
    vec2<f32>(17.0, -13.6), vec2<f32>(20.0, -10.6), vec2<f32>(22.0, -8.0), vec2<f32>(23.5, -6.2),
    vec2<f32>(26.0, -2.5),
);
// The lips part here (mm below the mouth landmark).
const LIP_LINE: f32 = -7.1;

// Height of a border at `a` mm from the midline, and its slope.
fn lip_border(upper: bool, a: f32) -> vec2<f32> {
    var t = LIP_LOWER;
    if (upper) {
        t = LIP_UPPER;
    }
    var i = 0;
    for (; i < 7; i++) {
        if (a < t[i + 1].x) { break; }
    }
    let p0 = t[i];
    let p1 = t[i + 1];
    let slope = (p1.y - p0.y) / (p1.x - p0.x);
    return vec2<f32>(p0.y + (a - p0.x) * slope, slope);
}

struct LipShape {
    amount: f32,  // 1 on the vermilion
    border: f32,  // distance to its border, mm (positive on the lip)
    upper: f32,   // 1 on the upper lip, 0 on the lower
    line: f32,    // distance from where the lips part, mm
    a: f32,       // distance from the midline, mm
    y: f32,       // height from the mouth landmark, mm
};

fn lip_shape(r: vec3<f32>, footprint: f32) -> LipShape {
    var s: LipShape;
    let q = (r - character.rest_mouth.xyz) * 1000.0;
    let a = abs(q.x);
    s.a = a;
    s.y = q.y;
    let up = lip_border(true, a);
    let lo = lip_border(false, a);
    let du = (up.x - q.y) * inverseSqrt(1.0 + up.y * up.y);
    let dl = (q.y - lo.x) * inverseSqrt(1.0 + lo.y * lo.y);
    s.border = min(du, dl);
    // The border is crisp, but the red deepens over the first millimeter.
    let blur = max(footprint * 1000.0, 0.3);
    s.amount = smoothstep(-blur, blur + 1.0, s.border);
    s.upper = smoothstep(-0.3, 0.3, q.y - LIP_LINE);
    s.line = abs(q.y - LIP_LINE);
    return s;
}

struct SkinDetail {
    height: f32,
    pore: f32,      // 1 inside a pore
    lines: f32,     // in a line of the lips (0..1)
    wrinkle: f32,   // depth of the wrinkle here (0..1)
};

// Skin relief at rest position `r` (meters; larger is outward).
fn skin_relief(r: vec3<f32>, m0: vec4<f32>, m1: vec4<f32>, m2: vec4<f32>, footprint: f32) -> SkinDetail {
    var d: SkinDetail;
    let lip = lip_shape(r, footprint);
    let lips = lip.amount;
    let nose = m0.y;
    let lids = m0.z;
    let cloth = m2.x;
    let pore_size = mix(0.0006, 0.00095, character.skin2.w) * (1.0 + 0.45 * nose - 0.3 * lids);
    // Fade detail that is smaller than a pixel (it would only shimmer).
    let pore_vis = saturate((pore_size / max(footprint, 1e-7) - 1.2) * 0.6);
    var h = 0.0;
    if (pore_vis > 0.0) {
        let c = cells3(r / pore_size);
        let pit = 1.0 - smoothstep(0.08, 0.42, c.f1);
        let amount = (0.55 + 0.6 * nose) * (1.0 - lips) * (1.0 - 0.6 * lids) * (0.6 + 0.6 * c.id);
        d.pore = pit * amount * pore_vis;
        h -= d.pore * 0.0001;
        // Raised skin between pores: a soft cellular ridge pattern.
        h += smoothstep(0.0, 0.25, c.f2 - c.f1) * 0.000018 * pore_vis * (1.0 - lips);
    }
    // Fine grain.
    let grain_vis = saturate((0.0004 / max(footprint, 1e-7) - 1.0) * 0.5);
    if (grain_vis > 0.0) {
        let g = value_noise3(r * vec3<f32>(2600.0, 1400.0, 2600.0)) + value_noise3(r * vec3<f32>(1300.0, 2900.0, 1300.0));
        h += (g - 1.0) * 0.000018 * grain_vis * (1.0 - cloth);
    }
    // Skin lines: two crossing families of fine furrows (the diamond texture of real skin),
    // broken up so they never read as a grid.
    let lines_vis = saturate((0.0013 / max(footprint, 1e-7) - 1.0) * 0.5) * (1.0 - lips) * (1.0 - cloth);
    if (lines_vis > 0.0) {
        let wn = (value_noise3(r * 700.0) - 0.5) * 2.0;
        let a = (r.x * 0.82 + r.y * 0.57) * 6283.0 / 1.3 + wn;
        let b = (-r.x * 0.82 + r.y * 0.57 + r.z * 0.3) * 6283.0 / 1.1 + wn * 1.3;
        let l = pow(1.0 - abs(sin(a)), 6.0) + pow(1.0 - abs(sin(b)), 6.0) * 0.8;
        h -= l * 0.000014 * lines_vis * (0.6 + 0.6 * value_noise3(r * 150.0));
    }
    // Lips: fine vertical lines, fading toward the border; the raised edge of the vermilion
    // (the white roll), and the two ridges of the philtrum from the cupid's bow to the nose
    // with the groove between them.
    if (lips > 0.01) {
        let wave = r.x * 6283.0 / 1.15 + value_noise3(r * 900.0) * 3.0;
        let line = pow(1.0 - abs(sin(wave)), 5.0) * smoothstep(0.3, 1.5, lip.border);
        d.lines = line * lips * saturate((0.0009 / max(footprint, 1e-7) - 1.0));
        h -= d.lines * 0.00005;
    }
    if (lip.a < 28.0 && lip.y > -24.0 && lip.y < 16.0) {
        let edge = lip.border + 0.3;
        h += exp(-edge * edge * 3.0) * mix(0.00005, 0.0001, lip.upper);
        let bow = lip_border(true, 0.0).x;
        let up = (lip.y - bow) / 9.0;
        if (up > -0.2 && up < 1.2) {
            let fade = smoothstep(-0.1, 0.15, up) * (1.0 - smoothstep(0.75, 1.1, up));
            let column = mix(5.6, 3.4, saturate(up));
            let ridge = exp(-sq((lip.a - column) / 1.5));
            let groove = 1.0 - smoothstep(0.0, column, lip.a);
            h += (ridge * 0.0002 - groove * 0.0001) * fade;
        }
    }
    // Wrinkles, with age.
    let wr = character.skin2.z;
    if (wr > 0.01) {
        var w = 0.0;
        let head = character.rest_head.xyz;
        let le = character.rest_eye_l.xyz;
        let re = character.rest_eye_r.xyz;
        // Forehead: wavy horizontal lines between the brows and the hairline.
        let fy = r.y - (le.y + 0.03);
        if (fy > 0.0 && fy < 0.045 && r.z > head.z + 0.06) {
            let wave = fy * 6283.0 / 8.5 + (value_noise3(r * 60.0) - 0.5) * 4.0;
            let fade = smoothstep(0.0, 0.006, fy) * (1.0 - smoothstep(0.03, 0.045, fy)) * (1.0 - smoothstep(0.035, 0.055, abs(r.x)));
            w = max(w, pow(saturate(1.0 - abs(sin(wave * 0.5)) * 1.25), 3.0) * fade * (0.6 + 0.4 * value_noise3(r * 150.0)));
        }
        // Crow's feet: lines fanning out from the outer eye corners, and lines under the eyes.
        for (var s = 0; s < 2; s++) {
            let e = select(re, le, s == 0);
            let side = sign(e.x);
            let corner = e + vec3<f32>(side * 0.0215, -0.001, -0.004);
            let q = r - corner;
            let dist = length(q.xy);
            let lateral = q.x * side;
            if (lateral > -0.002 && dist < 0.022) {
                let ang = atan2(q.y, max(lateral, 1e-4));
                let wave = ang * 9.0 + value_noise3(r * 200.0) * 1.5;
                let fade = smoothstep(0.003, 0.007, dist) * (1.0 - smoothstep(0.012, 0.022, dist));
                w = max(w, pow(saturate(1.0 - abs(sin(wave))), 6.0) * fade);
            }
            let u = r - e;
            let under = length(u.xy) - character.rest_eye_l.w * 1.32;
            if (u.y < -0.004 && abs(u.x) < 0.02 && under > 0.0 && under < 0.012) {
                let wave = under * 6283.0 / 3.2 + value_noise3(r * 300.0) * 3.5;
                let fade = (1.0 - smoothstep(0.004, 0.012, under)) * (1.0 - smoothstep(0.012, 0.02, abs(u.x)))
                    * smoothstep(0.3, 0.6, value_noise3(r * vec3<f32>(250.0, 80.0, 250.0)));
                w = max(w, pow(saturate(1.0 - abs(sin(wave))), 6.0) * fade * 0.55);
            }
        }
        // Neck rings: a few broken creases across the front of the neck.
        let ny = r.y - (head.y - 0.13);
        if (ny < -0.004 && ny > -0.05 && r.z > head.z - 0.03 && cloth < 0.5) {
            let wave = ny * 6283.0 / 13.0 + value_noise3(r * 40.0) * 2.0;
            let fade = smoothstep(-0.05, -0.035, ny) * (1.0 - smoothstep(-0.012, -0.004, ny))
                * (1.0 - smoothstep(0.03, 0.055, abs(r.x)))
                * smoothstep(0.35, 0.6, value_noise3(r * vec3<f32>(25.0, 4.0, 25.0)));
            w = max(w, pow(saturate(1.0 - abs(sin(wave * 0.5)) * 1.2), 4.0) * 0.5 * fade * (1.0 - m1.y * 0.5));
        }
        d.wrinkle = w * wr * saturate((0.0015 / max(footprint, 1e-7) - 0.8));
        h -= d.wrinkle * 0.00022;
    }
    // Knit of the shirt: jersey loops, and a ribbed band along the collar. Twill of the
    // trousers, a belt, and grained leather shoes with a welt above the sole.
    if (cloth > 0.01) {
        let vis = saturate((0.0015 / max(footprint, 1e-7) - 1.0));
        let g = garment(r);
        var relief = 0.0;
        if (g == 0u) {
            let wale = sin(r.x * 6283.0 / 0.9) * 0.5 + 0.5;
            let course = sin(r.y * 6283.0 / 0.7 + wale * 2.0);
            relief = wale * 0.00002 + course * 0.000008;
            let collar = smoothstep(0.004, 0.0, collar_depth(r) - 0.012);
            let rib = sin(atan2(r.x, r.z - character.rest_head.z) * 420.0) * 0.5 + 0.5;
            relief = mix(relief, rib * 0.00006, collar);
        } else if (g == 1u) {
            let twill = sin((r.y * 0.94 + (r.x + r.z) * 0.34) * 6283.0 / 0.8) * 0.5 + 0.5;
            relief = twill * 0.000016 + value_noise3(r * 2500.0) * 0.000006;
            // The belt's edges.
            let belt = belt_band(r);
            relief += belt * 0.00025 - belt * value_noise3(r * 3000.0) * 0.00001;
        } else {
            relief = value_noise3(r * vec3<f32>(1800.0, 1800.0, 1800.0)) * 0.000018;
            relief -= smoothstep(0.0012, 0.0, abs(r.y - (SOLE_TOP + 0.004))) * 0.00012;
        }
        h += cloth * relief * vis;
        // Folds (the vertices are moved by them too, see `cloth_lift`), and the shade in
        // their creases.
        let folds = cloth_folds(r, g) * saturate((0.02 / max(footprint, 1e-7) - 1.0));
        h += cloth * folds.x;
        d.wrinkle = max(d.wrinkle, cloth * folds.y * 2.0);
    }
    d.height = h;
    return d;
}

// ---------------------------------------------------------------- lighting
// Wrapped diffuse per color channel: `w` is how far past the terminator light reaches.
fn wrap3(n_dot_l: vec3<f32>, w: vec3<f32>) -> vec3<f32> {
    let x = saturate3((n_dot_l + w) / (1.0 + w));
    // Energy-conserving-ish: wrapped light is spread over a larger area.
    return x * x * (1.0 + w * 0.5) / (1.0 + w);
}

struct SkinLight {
    albedo: vec3<f32>,
    n: vec3<f32>,        // detail normal
    n_soft: vec3<f32>,   // smoothed normal (light under the skin doesn't see the pores)
    roughness: f32,      // perceptual, sharp lobe
    specular: f32,       // 0..1 scale of the highlights
    scatter: f32,        // 0 none (cloth) .. 1 skin
    curvature: f32,      // 1/m
    thin: f32,           // ears, nostrils: light shines through from behind
    occlusion: f32,
    inside: f32,         // 1 inside the mouth: the light there has bounced off lips and cheeks
    // Hair shadow: the point before the pose and that space's x and y axes in world space (zero:
    // no hair shadow), and a per-pixel offset for the march.
    bind: vec3<f32>,
    axis_x: vec3<f32>,
    axis_y: vec3<f32>,
    jitter: f32,
};

// How much light from direction `l` (world) gets through the hair over the skin. Fibers
// scatter much of what they stop forward, so hair shades skin more softly than it does itself.
fn hair_over_skin(s: SkinLight, l: vec3<f32>) -> f32 {
    if (dot(s.axis_x, s.axis_x) < 0.5) {
        return 1.0;
    }
    let axis_z = cross(s.axis_x, s.axis_y);
    let dir = vec3<f32>(dot(l, s.axis_x), dot(l, s.axis_y), dot(l, axis_z));
    return exp(-0.6 * hair_shadow_depth(s.bind, dir, s.jitter, 0.0));
}

// What reaches the inside of the mouth: as bright, but warmed by the lips and cheeks it bounced
// off on the way in (sky blue straight on the teeth reads as blue teeth).
fn mouth_light(c: vec3<f32>, inside: f32) -> vec3<f32> {
    let lum = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    return mix(c, lum * vec3<f32>(1.18, 0.92, 0.78), inside);
}

fn skin_spec(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, rough: f32) -> f32 {
    let h = normalize(l + v);
    let n_dot_l = saturate(dot(n, l));
    let n_dot_v = max(dot(n, v), 1e-4);
    let a1 = max(rough * rough, 0.01);
    let a2 = min(sq(rough * 1.65), 1.0);
    let d = mix(d_ggx(saturate(dot(n, h)), a2), d_ggx(saturate(dot(n, h)), a1), 0.85);
    let vis = v_smith_ggx_correlated(n_dot_v, n_dot_l, a1);
    let f = f_schlick(vec3<f32>(0.028), saturate(dot(l, h))).x;
    return d * vis * f * n_dot_l;
}

// Sun visibility on skin. Hairs lying close above the skin cast shadows as crisp as they are
// near, which the shadow map's dither leaves as rows of specks along the hairline; but light
// scatters through hair and spreads under the skin, so their shade is soft. Skin averages its
// shadow over a few millimeters around it.
const SKIN_SHADOW_SPREAD: f32 = 0.004;
fn skin_shadow(rel: vec3<f32>, n: vec3<f32>, scatter: f32) -> f32 {
    if (scatter <= 0.0) {
        return shadow_factor(rel, n);
    }
    let t1 = normalize(cross(n, select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > 0.9)));
    let t2 = cross(n, t1);
    let spread = SKIN_SHADOW_SPREAD * scatter;
    var sum = 0.0;
    for (var i = 0; i < 6; i++) {
        let o = vogel_disk(i, 6, 0.4) * spread;
        sum += shadow_factor(rel + t1 * o.x + t2 * o.y, n);
    }
    return sum / 6.0;
}

// Shaded skin, pre-exposed: all of its light, and the specular part (which the subsurface
// pass keeps sharp).
struct SkinShade {
    color: vec3<f32>,
    specular: vec3<f32>,
};

fn shade_skin(s: SkinLight, rel: vec3<f32>) -> SkinShade {
    let v = normalize(-rel);
    let n_dot_v = max(dot(s.n, v), 1e-4);
    // The imitation of scattering, in full where the subsurface pass doesn't run. With the
    // pass two fifths of it stay: the pass's blur spreads light only as far as the screen shows
    // the skin, so the warm terminator a few millimetres wide (light entering past the
    // shadow's edge, which a blur of lit pixels can't create) still needs the wrap.
    let imitate = s.scatter * (1.0 - 0.6 * character.eye2.z);
    // Per-channel normals: red from the smoothed normal, blue from the detailed one.
    let nr = normalize(mix(s.n, s.n_soft, 0.85 * imitate));
    let ng = normalize(mix(s.n, s.n_soft, 0.45 * imitate));
    let nb = s.n;
    // Tighter curves (nose, lips, ears) let light wrap further, relative to their size.
    let curve = saturate(abs(s.curvature) * 0.012);
    let wrap = vec3<f32>(0.32, 0.13, 0.07) * imitate * (0.6 + 0.8 * curve);
    var color = vec3<f32>(0.0);
    var specular = vec3<f32>(0.0);

    let l = frame.sun_dir.xyz;
    let sun = frame.sun_color.rgb * sun_transmittance(rel) * cloud_shadow(rel);
    if (dot(sun, sun) > 0.0) {
        let nl = vec3<f32>(dot(nr, l), dot(ng, l), dot(nb, l));
        // Shadow test from the smooth surface, so pores don't self-shadow at grazing light.
        // The shadow map holds the hair too; the darker of the two counts.
        var shadow = min(skin_shadow(rel, s.n_soft, s.scatter), hair_over_skin(s, l));
        if (nl.z > 0.0) {
            shadow *= contact_shadow(rel, s.n_soft);
        }
        // Light diffusing under the skin reddens shadow edges.
        let sh = vec3<f32>(pow(shadow, mix(1.0, 0.9, imitate)), shadow, pow(shadow, mix(1.0, 1.05, imitate)));
        let diffuse = s.albedo * INV_PI * wrap3(nl, wrap);
        let spec = skin_spec(s.n, v, l, s.roughness) * s.specular;
        color += diffuse * sh * sun;
        specular += spec * shadow * sun;
        // Thin parts glow red when the light is behind them.
        if (s.thin > 0.0) {
            let back = pow(saturate(dot(v, -l)), 3.0) * saturate(-dot(s.n_soft, l) + 0.3);
            color += s.albedo * vec3<f32>(1.0, 0.32, 0.18) * (back * s.thin * 0.6 * shadow) * sun * INV_PI;
        }
    }

    let count = frame.counts.x;
    for (var i = 0u; i < count; i++) {
        let light = point_lights[i];
        let to_light = light.position_range.xyz - rel;
        let dist2 = dot(to_light, to_light);
        let range = light.position_range.w;
        if (dist2 > range * range) { continue; }
        let pl = to_light * inverseSqrt(max(dist2, 1e-8));
        let falloff = sq(saturate(1.0 - sq(dist2 / (range * range)))) / max(dist2, sq(light.color_radius.w));
        let nl = vec3<f32>(dot(nr, pl), dot(ng, pl), dot(nb, pl));
        let diffuse = s.albedo * INV_PI * wrap3(nl, wrap);
        let spec = skin_spec(s.n, v, pl, s.roughness) * s.specular;
        // Light diffusing under the skin reddens the hair's shadow edges too.
        let hair = hair_over_skin(s, pl);
        let sh = vec3<f32>(pow(hair, mix(1.0, 0.9, imitate)), hair, pow(hair, mix(1.0, 1.05, imitate)));
        var lit = diffuse * sh;
        specular += spec * hair * light.color_radius.rgb * falloff;
        if (s.thin > 0.0) {
            let back = pow(saturate(dot(v, -pl)), 3.0) * saturate(-dot(s.n_soft, pl) + 0.3);
            lit += s.albedo * vec3<f32>(1.0, 0.32, 0.18) * back * s.thin * 0.6 * INV_PI;
        }
        color += lit * light.color_radius.rgb * falloff;
    }

    // Sky and environment.
    let sky_vis = sky_visibility(rel, s.n_soft);
    let fill = frame.ambient.rgb;
    let sky_r = (sky_ambient_covered(nr, sky_vis) - fill) * sky_vis + fill;
    let sky_b = (sky_ambient_covered(nb, sky_vis) - fill) * sky_vis + fill;
    let irradiance = mouth_light(mix(sky_b, sky_r, vec3<f32>(1.0, 0.5, 0.0)) * PI, s.inside);
    let visibility = min(s.occlusion, screen_ao(rel));
    // Light bounces inside the skin, so occlusion darkens red less.
    let ao = ao_multibounce(visibility, s.albedo);
    let ao_rgb = mix(ao, sqrt(ao), vec3<f32>(0.5, 0.15, 0.0) * s.scatter);
    color += s.albedo * irradiance * INV_PI * ao_rgb;
    color += s.albedo * screen_gi(rel);
    let rough = s.roughness;
    let r = reflect(-v, s.n);
    let ssr = screen_reflection(rel);
    let env = mix(mouth_light(sample_env_rough(r, rough) * sky_vis + fill, s.inside), ssr.rgb, ssr.a);
    let brdf = env_brdf_approx(vec3<f32>(0.028), rough, n_dot_v);
    specular += env * brdf * s.specular * specular_occlusion(n_dot_v, visibility, rough * rough);
    // Peach fuzz catches light at grazing angles (on the surface: not blurred).
    let fuzz = pow(1.0 - n_dot_v, 4.0) * 0.05 * s.scatter;
    specular += fuzz * (sky_r * PI * INV_PI) * visibility;
    return SkinShade(pre_expose(color + specular), pre_expose(specular));
}

// ---------------------------------------------------------------- fragment
struct SkinSurface {
    light: SkinLight,
};

// Teeth (ivory enamel, a little translucent and greyer toward the biting edge), gums and
// tongue (wet, red, the tongue rough with papillae).
fn mouth_surface(in: SkinOut, front: bool) -> SkinLight {
    var n = normalize(in.normal);
    if (!front) { n = -n; }
    let r = in.rest;
    var s: SkinLight;
    s.n = n;
    s.n_soft = n;
    s.curvature = 0.0;
    s.thin = 0.0;
    if (in.part == 6u) {
        // A hood: the top's heather knit.
        let seed = character.look.w * 917.0;
        let heather = 0.82 + 0.36 * value_noise3(r * vec3<f32>(900.0, 300.0, 900.0) + seed);
        s.albedo = character.top.xyz * heather;
        s.roughness = 0.9;
        s.specular = 0.25;
        s.scatter = 0.0;
        s.occlusion = 0.9;
        return s;
    }
    if (in.part == 5u) {
        // A skirt in the outfit's bottom color: a fine twill, a darker waistband.
        let seed = character.look.w * 917.0;
        let mottle = 0.85 + 0.3 * value_noise3(r * vec3<f32>(400.0, 120.0, 400.0) + seed);
        let band = smoothstep(WAIST - 0.03, WAIST - 0.026, r.y);
        s.albedo = character.bottom.xyz * mottle * (1.0 - 0.3 * band);
        s.roughness = 0.85;
        s.specular = 0.25;
        s.scatter = 0.0;
        s.occlusion = 1.0;
        return s;
    }
    if (in.part == 4u) {
        // Shoes: black grained leather, a welt above pale rubber soles.
        let seed = character.look.w * 917.0;
        let grain = 0.88 + 0.24 * value_noise3(r * 1400.0 + seed);
        let sole = smoothstep(SOLE_TOP + 0.001, SOLE_TOP - 0.001, r.y);
        let welt = smoothstep(0.0012, 0.0, abs(r.y - SOLE_TOP - 0.003));
        s.albedo = mix(vec3<f32>(0.022, 0.019, 0.017) * grain, vec3<f32>(0.42, 0.4, 0.37), sole);
        s.albedo = mix(s.albedo, vec3<f32>(0.12, 0.1, 0.085), welt * 0.7);
        s.roughness = mix(0.36 + 0.1 * grain, 0.75, sole);
        s.specular = mix(0.7, 0.3, sole);
        s.scatter = 0.0;
        s.occlusion = 1.0;
        return s;
    }
    let v = value_noise3(r * 1500.0);
    // Inside the mouth little of the sky reaches.
    s.occlusion = 0.55;
    s.inside = 1.0;
    if (in.part == 1u) {
        // Enamel: warm ivory, a soft sheen (a strong one mirrors the blue sky into the mouth).
        s.albedo = vec3<f32>(0.86, 0.79, 0.65) * (0.94 + 0.08 * v);
        s.roughness = 0.38;
        s.specular = 0.22;
        s.scatter = 0.35;
        s.occlusion = 0.9;
    } else if (in.part == 2u) {
        s.albedo = vec3<f32>(0.5, 0.14, 0.13) * (0.9 + 0.2 * v);
        s.roughness = 0.3;
        s.specular = 0.6;
        s.scatter = 1.0;
    } else {
        let papillae = value_noise3(r * 4000.0);
        s.albedo = vec3<f32>(0.44, 0.17, 0.17) * (0.85 + 0.25 * papillae);
        // Deep in the mouth.
        s.occlusion = 0.4;
        s.roughness = 0.4 + 0.2 * papillae;
        s.specular = 0.5;
        s.scatter = 1.0;
    }
    return s;
}

// How much cloth covers the skin here: the shirt's edge per pixel (the vertex mask is as soft
// as the base mesh is coarse and reaches well past the hem); the vertex mask leaves the hands
// bare.
fn cloth_cover(r: vec3<f32>, vertex_mask: f32) -> f32 {
    return smoothstep(-0.0008, 0.0008, cover_depth(r)) * smoothstep(0.25, 0.75, vertex_mask);
}

fn skin_surface(in: SkinOut, front: bool) -> SkinLight {
    if (in.part != 0u) {
        return mouth_surface(in, front);
    }
    let r = in.rest;
    let footprint = max(length(dpdx(r)), length(dpdy(r)));
    let m0 = in.m0;
    let m1 = in.m1;
    let m2 = vec4<f32>(cloth_cover(r, in.m2.x), in.m2.yzw);
    let lip = lip_shape(r, footprint);
    let lips = lip.amount;
    let nose = m0.y;
    let lids = m0.z;
    let cheeks = m1.x;
    let beard = m1.y;
    let scalp = m1.z;
    let ears = m1.w;
    let cloth = m2.x;
    let forehead = m2.y;
    let chin = m2.z;
    let rim = m2.w;
    var n = normalize(in.normal);
    if (!front) { n = -n; }
    let n_soft = n;

    // Relief: gradient by central differences in rest space, applied in the model's frame.
    let e = max(footprint * 0.5, 0.00008);
    let d0 = skin_relief(r, m0, m1, m2, footprint);
    let hx = skin_relief(r + vec3<f32>(e, 0.0, 0.0), m0, m1, m2, footprint).height - skin_relief(r - vec3<f32>(e, 0.0, 0.0), m0, m1, m2, footprint).height;
    let hy = skin_relief(r + vec3<f32>(0.0, e, 0.0), m0, m1, m2, footprint).height - skin_relief(r - vec3<f32>(0.0, e, 0.0), m0, m1, m2, footprint).height;
    let hz = skin_relief(r + vec3<f32>(0.0, 0.0, e), m0, m1, m2, footprint).height - skin_relief(r - vec3<f32>(0.0, 0.0, e), m0, m1, m2, footprint).height;
    let grad = model_dir_raw(vec3<f32>(hx, hy, hz) / (2.0 * e));
    let tangential = grad - n * dot(grad, n);
    n = normalize(n - tangential);

    // Pigments.
    let seed = character.look.w * 917.0;
    let mel_base = character.skin.x;
    let mottle = value_noise3(r * 40.0 + seed) - 0.5;
    // Blotches a few centimeters across, and finer ones a few millimeters across.
    let fine = value_noise3(r * 300.0 + seed * 1.3) - 0.5;
    var mel = saturate(mel_base * (1.0 + 0.12 * mottle + 0.08 * fine) + 0.03 * lids * (0.5 + mel_base));
    // Freckles and age spots.
    let fr = character.skin2.x;
    let age = character.look.x;
    var spots = 0.0;
    if (fr > 0.0 || age > 50.0) {
        // Freckles: irregular flecks of different sizes and strengths, most on the nose and
        // cheeks.
        let warp = (value_noise3(r * 900.0 + seed) - 0.5) * 0.7;
        let c = cells3(r / 0.0022 + seed + warp);
        let face = saturate(nose * 1.2 + cheeks + forehead * 0.6) * (1.0 - lips);
        let p_freckle = fr * (0.12 + 0.6 * face);
        let size = 0.18 + 0.24 * fract(c.id * 7.13);
        let freckle = select(0.0, 1.0, c.id < p_freckle) * (1.0 - smoothstep(size * 0.55, size, c.f1));
        let c2 = cells3(r / 0.009 + seed * 1.7);
        let p_age = saturate((age - 50.0) / 30.0) * 0.1 * (0.3 + face);
        let age_spot = select(0.0, 1.0, c2.id < p_age) * (1.0 - smoothstep(0.15, 0.4, c2.f1));
        spots = max(freckle * (0.25 + 0.6 * fract(c.id * 13.7)), age_spot * 0.6);
    }
    var albedo = skin_tone(mel);
    albedo = mix(albedo, albedo * vec3<f32>(0.82, 0.66, 0.52), spots);
    // Blood: flushed cheeks, nose, ears and chin, mottled; muted under dark skin.
    let redness = character.skin.y;
    let flush = redness * (0.3 + 0.55 * cheeks + 0.6 * nose + 0.5 * ears + 0.25 * chin + 0.25 * lids)
        * (0.8 + 0.25 * value_noise3(r * 120.0 + seed) + 0.15 * value_noise3(r * 450.0 + seed * 0.7));
    let blood = saturate(flush) * (1.0 - 0.75 * saturate(mel * 1.3));
    albedo *= mix(vec3<f32>(1.0), vec3<f32>(1.0, 0.70, 0.72), blood);
    // Undertone: warm is golden-olive, cool is pinker.
    let ut = character.skin.z;
    albedo *= select(vec3<f32>(1.0, 1.0 - 0.04 * ut, 1.0 + 0.04 * ut), vec3<f32>(1.0, 1.0 - 0.02 * ut, 1.0 - 0.16 * ut), ut > 0.0);
    // Darker, slightly violet skin around the eyes.
    albedo *= mix(vec3<f32>(1.0), vec3<f32>(0.88, 0.83, 0.88), lids * 0.55);
    // Lips: thin skin over blood, rose on fair skin, deeper on dark skin, where the outer
    // band holds more melanin than the moist inner part; a pale, raised line just outside the
    // border (the white roll); the corners tuck into shadow.
    let wet = 1.0 - smoothstep(0.8, 3.5, lip.line);
    let lip_fair = albedo * vec3<f32>(0.88, 0.6, 0.63);
    let lip_dark = albedo * vec3<f32>(0.85, 0.6, 0.68);
    var lip_color = mix(lip_fair, lip_dark, saturate(mel * 1.4));
    let outer = 1.0 - smoothstep(0.2, 2.5, lip.border);
    lip_color *= mix(vec3<f32>(1.0), vec3<f32>(0.72, 0.7, 0.72), outer * saturate(mel * 1.6 - 0.3));
    lip_color *= mix(vec3<f32>(1.0), vec3<f32>(0.86, 0.74, 0.8), wet);
    lip_color *= (0.92 + 0.16 * value_noise3(r * 700.0)) * (1.0 - 0.12 * d0.lines);
    let roll = exp(-sq(lip.border + 0.45) * 4.0) * (1.0 - lips) * (0.4 + 0.6 * lip.upper);
    albedo *= 1.0 + 0.07 * roll * (1.0 - saturate(mel * 1.5));
    albedo = mix(albedo, lip_color, lips);
    let corner = exp(-(sq(lip.a - 23.5) + sq(lip.y - LIP_LINE) * 2.0) / 6.0);
    // The wet, pink rim of the eyelids.
    albedo = mix(albedo, albedo * vec3<f32>(1.0, 0.62, 0.62), rim * 0.7);
    // Moles.
    let moles = character.skin2.y;
    if (moles > 0.0) {
        let c = cells3(r / 0.011 + seed * 2.3);
        let size = 0.12 + 0.1 * fract(c.id * 31.0);
        let mole = select(0.0, 1.0, c.id < moles * 0.05) * (1.0 - smoothstep(size * 0.7, size, c.f1));
        albedo = mix(albedo, skin_tone(saturate(mel + 0.45)) * vec3<f32>(0.75, 0.6, 0.55), mole * (1.0 - cloth));
    }
    // Beard shadow: hair roots under the skin, and stubble dots.
    let beard_shadow = character.look.z * beard;
    if (beard_shadow > 0.0) {
        let bc = hair_color(character.beard.x, character.beard.y, character.beard.z, 0.5);
        let dots = 1.0 - smoothstep(0.1, 0.35, cells3(r / 0.00055 + seed).f1);
        let tint = mix(vec3<f32>(1.0), bc / max(luminance(bc), 0.02) * 0.4 + 0.25, 0.35);
        albedo = mix(albedo, albedo * tint * 0.72, beard_shadow * 0.55);
        albedo = mix(albedo, bc, dots * beard_shadow * 0.5 * saturate((0.0011 / max(footprint, 1e-7) - 1.0)));
    }
    // Scalp under short hair: the color of the roots.
    let scalp_tint = character.hair.w * scalp;
    if (scalp_tint > 0.0) {
        let hc = hair_color(character.hair.x, character.hair.y, character.hair.z, 0.5);
        let dots = 1.0 - smoothstep(0.1, 0.4, cells3(r / 0.0005 + seed).f1);
        albedo = mix(albedo, albedo * 0.75, scalp_tint * 0.5);
        // Root dots only show through hair cut close.
        let stubble = smoothstep(0.75, 0.85, scalp_tint);
        albedo = mix(albedo, hc, scalp_tint * (0.45 + 0.4 * dots * stubble));
    }
    // Brows: the skin between the brow hairs takes a little of their color.
    let brows = m0.w;
    if (brows > 0.0) {
        // (Greyed: the skin shows the hairs' shade more than their hue.)
        let bc0 = hair_color(character.brows.x, character.brows.y, character.brows.z, 0.5);
        let bc = mix(vec3<f32>(luminance(bc0)), bc0, 0.5);
        albedo = mix(albedo, min(albedo, bc * 1.4 + albedo * 0.25), brows * 0.3);
    }
    // Pores hold a little shadow.
    albedo *= 1.0 - 0.12 * d0.pore - 0.15 * d0.wrinkle;

    // Shine: oily T-zone, wet lips and lid rims, matte pores.
    let oil = character.skin.w;
    var rough = 0.56 - 0.12 * oil * saturate(forehead + nose + chin) - 0.08 * oil;
    rough += (value_noise3(r * 300.0 + seed) - 0.5) * 0.06 + (value_noise3(r * 900.0 + seed) - 0.5) * 0.05;
    rough = mix(rough, mix(0.4, 0.28, wet), lips);
    rough = mix(rough, 0.18, rim);
    rough += 0.12 * d0.pore + 0.1 * d0.wrinkle + 0.06 * scalp_tint;
    var specular = (1.0 - 0.6 * d0.pore) * (1.0 - 0.3 * scalp_tint);

    var scatter = 1.0;
    // The shirt: a heather knit in the outfit's color, its collar band a shade darker, and
    // the accent's band. Twill trousers with a dark leather belt; black leather shoes on pale
    // rubber soles.
    if (cloth > 0.0) {
        let g = garment(r);
        var fabric = vec3<f32>(0.0);
        var fabric_rough = 0.9;
        var fabric_spec = 0.25;
        if (g == 0u) {
            let heather = 0.82 + 0.36 * value_noise3(r * vec3<f32>(900.0, 300.0, 900.0) + seed);
            let collar = smoothstep(0.004, 0.0, collar_depth(r) - 0.012);
            fabric = character.top.xyz * heather * (1.0 - 0.25 * collar);
            fabric = mix(fabric, character.accent.xyz * heather, accent_band(r, collar));
        } else if (g == 1u) {
            let mottle = 0.85 + 0.3 * value_noise3(r * vec3<f32>(400.0, 120.0, 400.0) + seed);
            fabric = character.bottom.xyz * mottle;
            let belt = belt_band(r);
            fabric = mix(fabric, vec3<f32>(0.03, 0.019, 0.012), belt);
            fabric_rough = mix(0.85, 0.45, belt);
            fabric_spec = mix(0.25, 0.6, belt);
        } else {
            let grain = 0.9 + 0.2 * value_noise3(r * 900.0 + seed);
            let sole = smoothstep(SOLE_TOP + 0.001, SOLE_TOP - 0.001, r.y);
            fabric = mix(vec3<f32>(0.022, 0.019, 0.017) * grain, vec3<f32>(0.42, 0.4, 0.37), sole);
            fabric_rough = mix(0.42, 0.75, sole);
            fabric_spec = 0.6;
        }
        albedo = mix(albedo, fabric, cloth);
        rough = mix(rough, fabric_rough, cloth);
        specular = mix(specular, fabric_spec, cloth);
        scatter = 1.0 - cloth;
    }
    var s: SkinLight;
    s.albedo = albedo;
    s.n = n;
    s.n_soft = n_soft;
    s.roughness = clamp(rough, 0.12, 1.0);
    s.specular = specular;
    s.scatter = scatter;
    s.curvature = in.curvature;
    s.thin = saturate(ears + nose * 0.3) * scatter;
    s.occlusion = (1.0 - 0.35 * d0.wrinkle) * (1.0 - 0.5 * corner * (1.0 - cloth));
    return s;
}

// Debug views (`eye2.y`): 1 region masks, 2 normals, 3 curvature.
fn skin_debug(in: SkinOut, s: SkinLight) -> vec3<f32> {
    let mode = u32(character.eye2.y + 0.5);
    if (mode == 1u) {
        let lips = lip_shape(in.rest, max(length(dpdx(in.rest)), length(dpdy(in.rest)))).amount;
        return vec3<f32>(lips + in.m1.y * 0.5, in.m0.w + in.m1.z * 0.5, in.m0.y + in.m0.z * 0.5 + in.m2.w);
    }
    if (mode == 2u) {
        return s.n * 0.5 + 0.5;
    }
    let c = in.curvature * 0.02;
    return vec3<f32>(saturate(c), 0.2, saturate(-c));
}

@fragment
fn fs_skin(in: SkinOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var s = skin_surface(in, front);
    if (in.part == 0u) {
        s.bind = in.bind;
        s.axis_x = normalize(in.axis_x);
        s.axis_y = normalize(in.axis_y);
        s.jitter = hash13(vec3<f32>(in.clip.xy, frame.time.w * 0.37));
    }
    if (character.eye2.y > 0.5) {
        return vec4<f32>(pre_expose(skin_debug(in, s) * 2000.0), 1.0);
    }
    let shade = shade_skin(s, in.rel);
    let color = apply_fog(shade.color, in.rel);
    // The subsurface pass blurs color minus alpha: alpha holds the specular light (fogged as
    // the rest is).
    var alpha = 1.0;
    if (character.eye2.z > 0.5) {
        alpha = luminance(shade.specular * surface_fog(in.rel).transmittance);
    }
    return vec4<f32>(color, alpha);
}

@fragment
fn fs_skin_prepass(in: SkinOut, @builtin(front_facing) front: bool) -> PrepassOutput {
    var n = normalize(in.normal);
    if (!front) { n = -n; }
    if (character.eye2.z > 0.5) {
        // How far light spreads under it: skin in full, the gums and tongue less, cloth,
        // shoes and teeth not at all.
        var strength = 0.0;
        switch in.part {
            case 0u: { strength = 1.0 - cloth_cover(in.rest, in.m2.x); }
            case 2u, 3u: { strength = 0.5; }
            default: {}
        }
        return prepass_output_subsurface(n, 0.5, in.clip_now, in.clip_prev, strength);
    }
    return prepass_output(n, 0.5, in.clip_now, in.clip_prev);
}



fn model_dir_raw(d: vec3<f32>) -> vec3<f32> {
    return (character.model * vec4<f32>(d, 0.0)).xyz;
}
