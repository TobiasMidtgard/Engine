// Grass blades, shared by the color and shadow passes. Requires lib_types (not lib_frame: the
// shadow pass binds its own group 0), so time, wind and the world-tiling offset come in as
// arguments.
//
// Every blade is one instance: a triangle strip of 2 × (segments + 1) vertices built here
// from 16 bytes of blade data. The draw's first vertex picks the level of detail: vertices
// 0.. are 7 segments, 64.. are 4, 128.. are 2. Blades bend along an arc of constant length
// (so wind and burning droop them without stretching), taper to a point (or carry a seed
// head), and read the meadow's burn map at their root: standing, burning (glowing, blackening,
// collapsing), stubble, or green regrowth.

struct Blade {
    pos: vec2<f32>, // x, z from the meadow's center (m)
    a: u32,         // height (16 bits, 0..2 m), width (8 bits, 0..4 cm), bend (8 bits)
    b: u32,         // yaw (8 bits), color seed (8), density rank (8), flags (8: 1 lush, 2 seed head)
};

struct GrassParams {
    origin: vec4<f32>,  // camera-relative meadow center xyz, unused
    map: vec4<f32>,     // burn map min corner from the center (x, z), map size (x, z), m
    lod: vec4<f32>,     // full-density distance (m), least density fraction, widest boost, sun-up 0..1
    misc: vec4<f32>,    // time of the burn map (unused), wind sway scale, unused, unused
};

@group(1) @binding(0) var<uniform> grass: GrassParams;
@group(1) @binding(1) var<storage, read> blades: array<Blade>;
@group(1) @binding(2) var burn_map: texture_2d<f32>;
@group(1) @binding(3) var burn_sampler: sampler;
@group(1) @binding(4) var ground_map: texture_2d<f32>;  // ground height above the center, one texel per burn-map cell

const LOD_STRIDE: u32 = 64u;

struct BladeVertex {
    rel: vec3<f32>,
    root: vec3<f32>,
    normal: vec3<f32>,
    t: f32,             // 0 at the root .. 1 at the tip
    side: f32,          // -1 .. 1 across the blade
    seed: f32,
    flags: u32,
    state: vec4<f32>,   // per-blade height factor, char, glow, green
    visible: f32,
};

// Fraction of blades drawn at distance `d` (each blade's density rank is compared with it).
fn grass_density(d: f32) -> f32 {
    return clamp(pow(grass.lod.x / max(d, 1e-3), 1.5), grass.lod.y, 1.0);
}

// Ground height under `pos` (x, z from the meadow's center), bilinear between cell centers.
fn ground_height(pos: vec2<f32>) -> f32 {
    let dims = vec2<i32>(textureDimensions(ground_map));
    let f = (pos - grass.map.xy) / grass.map.zw * vec2<f32>(dims) - 0.5;
    let i = vec2<i32>(floor(f));
    let t = f - floor(f);
    let hi = dims - 1;
    let a = clamp(i, vec2<i32>(0), hi);
    let b = clamp(i + 1, vec2<i32>(0), hi);
    let h00 = textureLoad(ground_map, a, 0).r;
    let h10 = textureLoad(ground_map, vec2<i32>(b.x, a.y), 0).r;
    let h01 = textureLoad(ground_map, vec2<i32>(a.x, b.y), 0).r;
    let h11 = textureLoad(ground_map, b, 0).r;
    return mix(mix(h00, h10, t.x), mix(h01, h11, t.x), t.y);
}

fn blade_segments(band: u32) -> u32 {
    if (band == 0u) { return 7u; }
    if (band == 1u) { return 4u; }
    return 2u;
}

// Point on an arc of length `h` whose angle from vertical runs from `a0` to `a1`, at `t`:
// (forward, up).
fn arc_point(h: f32, a0: f32, a1: f32, t: f32) -> vec2<f32> {
    let da = a1 - a0;
    let a = a0 + da * t;
    if (abs(da) < 1e-3) {
        return vec2<f32>(sin(a0), cos(a0)) * h * t;
    }
    return vec2<f32>(cos(a0) - cos(a), sin(a) - sin(a0)) * (h / da);
}

fn build_blade(instance: u32, vertex: u32, wind: vec3<f32>, time: f32, tiling: vec3<f32>) -> BladeVertex {
    var out: BladeVertex;
    let blade = blades[instance];
    let band = vertex / LOD_STRIDE;
    let k = vertex % LOD_STRIDE;
    let segs = blade_segments(band);
    let row = min(k / 2u, segs);
    let side = select(-1.0, 1.0, (k & 1u) == 1u);
    let t = f32(row) / f32(segs);

    var h = f32(blade.a & 0xffffu) / 65535.0 * 2.0;
    var w = f32((blade.a >> 16u) & 0xffu) / 255.0 * 0.04;
    let bend = f32(blade.a >> 24u) / 255.0;
    let yaw = f32(blade.b & 0xffu) / 256.0 * TAU;
    let seed = f32((blade.b >> 8u) & 0xffu) / 255.0;
    let rank = f32((blade.b >> 16u) & 0xffu) / 255.0;
    let flags = blade.b >> 24u;

    let root = grass.origin.xyz + vec3<f32>(blade.pos.x, ground_height(blade.pos), blade.pos.y);
    let dist = length(root);
    let density = grass_density(dist);
    // Thin out with distance (by rank) and widen what's left so the field stays as full.
    let keep = saturate((density - rank) * 30.0);
    w *= clamp(inverseSqrt(density), 1.0, grass.lod.z);

    // Burn state at the root, varied per blade so the front isn't a clean line.
    let uv = (blade.pos - grass.map.xy) / grass.map.zw;
    let state = textureSampleLevel(burn_map, burn_sampler, uv, 0.0);
    let s2 = fract(seed * 7.13 + 0.31);
    let s3 = fract(seed * 13.7 + 0.77);
    let charred = saturate(state.g * 1.3 - s2 * 0.3);
    let green = saturate(state.a * 1.4 - s3 * 0.4);
    var hf = state.r;
    // Stubble: burnt blades end at different lengths.
    hf *= mix(1.0, 0.45 + 1.1 * s2, saturate((0.45 - hf) * 3.0));
    h *= max(hf, 0.0) * keep;

    // Wind: gusts travel across the field; each blade flutters a little on its own.
    let wp = root + tiling;
    let speed = length(wind);
    let wdir = select(vec3<f32>(1.0, 0.0, 0.0), wind / max(speed, 1e-3), speed > 0.01);
    let phase = dot(wp.xz, vec2<f32>(0.13, 0.17)) - dot(wp.xz, wdir.xz) * 0.35;
    let gust = 0.55 + 0.45 * sin(time * 0.9 - dot(wp.xz, wdir.xz) * 0.25) * sin(time * 2.3 + phase);
    let flutter = sin(time * (4.0 + 3.0 * seed) + seed * 40.0) * 0.12;
    let push = min(speed / 8.0, 1.6) * (gust + flutter) * grass.misc.y;

    let fwd = vec3<f32>(cos(yaw), 0.0, sin(yaw));
    let across = vec3<f32>(-sin(yaw), 0.0, cos(yaw));
    let wf = dot(wdir, fwd);
    let ws = dot(wdir, across);
    // Lean (radians from vertical) at the root and the tip: own curl, wind, and burning
    // blades curling over.
    let lush = (flags & 1u) != 0u;
    var a0 = bend * 0.32 + push * wf * 0.25;
    var a1 = a0 + bend * 1.1 + push * wf * 0.9 + charred * 1.1 * (1.0 - hf);
    if (lush) {
        a1 = a0 + bend * 0.7 + push * wf * 0.6;
    }
    let p = arc_point(h, a0, a1, t);
    let a = a0 + (a1 - a0) * t;
    let tangent = fwd * sin(a) + vec3<f32>(0.0, cos(a), 0.0);
    var center = root + fwd * p.x + vec3<f32>(0.0, p.y, 0.0) + across * (ws * push * 0.35 * h * t * t);

    // Width: tapered blade, or a thin stem with a seed head near the top.
    var width = w * pow(max(1.0 - t, 0.0), 0.6) * (0.85 + 0.15 * sin(t * 3.0));
    if ((flags & 2u) != 0u && charred < 0.5) {
        let head = smoothstep(0.62, 0.72, t) * (1.0 - smoothstep(0.9, 1.0, t));
        width = w * (0.22 + 1.6 * head * sin(clamp((t - 0.62) / 0.38, 0.0, 1.0) * PI));
    }
    let pos = center + across * (side * width * 0.5);

    var n = normalize(cross(tangent, across));
    // Rounded across the blade, and a little toward the sky so the field shades softly.
    n = normalize(n + across * side * 0.45);

    out.rel = pos;
    out.root = root;
    out.normal = n;
    out.t = t;
    out.side = side;
    out.seed = seed;
    out.flags = flags;
    out.state = vec4<f32>(hf, charred, state.b, green);
    out.visible = step(0.004, h);
    if (out.visible <= 0.0) {
        // Collapse to a point: no area, nothing drawn.
        out.rel = root;
    }
    return out;
}
