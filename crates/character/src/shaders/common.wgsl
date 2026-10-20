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
