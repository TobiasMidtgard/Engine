// Trees and shrubs bending on wind bones (plant_wind.rs; engine-design/foliage-wind-research.md
// §3.1-3.2). Composed in front of mesh.wgsl and mesh_shadow.wgsl; needs GustWaves.
//
// A mesh registered with a skeleton (`PlantWindMeshes`) is drawn with its table entry + 1 in the
// instance's `extra.y` bits 16-31 (0: the legacy sway, `StandardMaterial::wind_sway`). Each
// vertex names its bone in its color alpha (`pack_plant_vertex`): bone (14 bits), tip weight (5),
// edge (4), underside (1). `plant_wind` turns the vertex about its bone's pivot and then its
// parents' pivots, deepest first, on the already moved point (exact composition: a parent's turn
// carries its children's pivots), adds the leaves' flutter and bends the trunk by its lean.
//
// The table (`plant_wind_words`, one array of vec4):
// - word 0: species count, first mesh word, first bone word (u32 bits);
// - per species slot s, 8 words from 1 + 8 s: phases now (trunk, limb, branch, flutter cycles),
//   phases last frame, second modes (limb, branch now; limb, branch last frame), response ramp
//   onsets (trunk, limb, branch, leaves; m/s), ramp ends, the most each level turns (limb, branch,
//   spray, radians, already divided by the species' stiffness) and the flutter (m), the spray
//   flip (onset, full m/s, most radians) and the underside's albedo gain, and the trunk (f0 Hz,
//   overshoot exp(-pi zeta), lean per drag per meter of height) and the sway share, and the
//   snow it holds (`snow_hold`, then 3 unused);
// - per mesh entry: first bone (u32 bits), species slot, deepest level drawn, reach (m);
// - per bone, 2 words: pivot xyz and length (mesh space, m); axis and face (octahedral snorm
//   pairs), parent (bits 0-15, 0xffff none) and level (bits 16-17), phase and stiffness / 4
//   (unorm pair).
@group(1) @binding(1) var<storage, read> plant_wind_words: array<vec4<u32>>;

// Word `i` of the table as floats (stored as integers, so integer words never pass through a
// float that a GPU might flush).
fn plant_word(i: u32) -> vec4<f32> {
    return bitcast<vec4<f32>>(plant_wind_words[i]);
}

const PLANT_SPECIES_WORDS: u32 = 9u;
const PLANT_TAU: f32 = 6.28318530718;
// Drag (`plant_drag`) at which a level reaches its most bend: a gale, 15 m/s.
const PLANT_DRAG_FULL: f32 = 10.714;

struct Winded {
    pos: vec3<f32>,
    normal: vec3<f32>,
    // 1 on a leaf card's underside, 0 elsewhere.
    underside: f32,
    // The vertex's tip weight (0 at its bone's pivot, 1 at the tip): how far benders push it.
    tip: f32,
};

// The air a plant instance stands in: the smoothed wind (xyz m/s, w speed), the gust fronts, the
// gust coordinates of the mesh origin and of the vertex at rest, whether this is last frame's
// (motion vectors), the deepest bone level to turn (3 sprays .. 1 limbs, 0 the trunk only), and
// whether it sways (0: the rest pose of shadow maps too coarse to show the motion, which bends
// with the wind but doesn't move with time).
struct PlantAir {
    wind: vec4<f32>,
    gusts: GustWaves,
    root_at: vec2<f32>,
    at: vec2<f32>,
    prev: bool,
    detail: u32,
    swing: f32,
};

// Wind speed's drag on a crown: about U² in a breeze, about U in a gale as leaves and branches
// streamline (Vogel 1989). `engine_foliage::wind::drag` is the same.
fn plant_drag(u: f32) -> f32 {
    let v = max(u, 0.0);
    return v * v / (6.0 + v);
}

fn plant_ramp(onset: f32, full: f32, x: f32) -> f32 {
    return smoothstep(onset, max(full, onset + 1e-3), x);
}

fn plant_oct_decode(w: u32) -> vec3<f32> {
    let e = unpack2x16snorm(w);
    var v = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    let t = max(-v.z, 0.0);
    v.x += select(t, -t, v.x >= 0.0);
    v.y += select(t, -t, v.y >= 0.0);
    return normalize(v);
}

// `v` turned by `angle` about the unit axis `k` (Rodrigues).
fn plant_turn(v: vec3<f32>, k: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return v * c + cross(k, v) * s + k * dot(k, v) * (1.0 - c);
}

fn plant_hash(at: vec2<f32>) -> f32 {
    return fract(sin(dot(floor(at * 4.0), vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

// The table entry of an instance's `extra.y` (0: not a bone plant).
fn plant_wind_entry(extra_y: u32) -> u32 {
    return extra_y >> 16u;
}

// The mesh entry's words: first bone, species slot, deepest level, reach.
fn plant_mesh(entry: u32) -> vec4<u32> {
    let header = plant_wind_words[0];
    return plant_wind_words[header.y + entry - 1u];
}

fn plant_species(slot: u32) -> u32 {
    return 1u + slot * PLANT_SPECIES_WORDS;
}

// The trunk's lean at its top (mesh space x, z; meters): the drag of the broad gusts that passed
// the root a quarter, three quarters and five quarters of a sway period ago, so the tree lags a
// gust, overshoots it by exp(-pi zeta) and swings back like a damped spring (3.2.1), without
// keeping any state; plus its own sway at f0 in the turbulence. `dir` is the wind's direction in
// mesh space, `height` the trunk's length (mesh space).
fn plant_lean(sp: u32, air: PlantAir, dir: vec3<f32>, height: f32, hash: f32) -> vec2<f32> {
    let trunk = plant_word(sp + 7u);
    let speed = air.wind.w;
    if (speed <= 0.01 || trunk.x <= 0.0) { return vec2<f32>(0.0); }
    let period = 1.0 / trunk.x;
    let kappa = trunk.y;
    var f = vec3<f32>(0.0);
    for (var i = 0u; i < 3u; i++) {
        let tau = period * (0.25 + 0.5 * f32(i));
        let g = gust_wave(air.gusts.broad_a, air.gusts.broad_b, air.root_at - air.gusts.broad_a.xy * speed * tau);
        f[i] = plant_drag(speed * (1.0 + g));
    }
    var drag = f.x + kappa * (f.x - f.y) - kappa * kappa * (f.y - f.z);
    let ramps_on = plant_word(sp + 3u);
    let ramps_full = plant_word(sp + 4u);
    if (air.swing > 0.0) {
        let phases = select(plant_word(sp), plant_word(sp + 1u), air.prev);
        let ramp = plant_ramp(ramps_on.x, ramps_full.x, speed);
        drag += 0.4 * trunk.w * plant_drag(speed) * ramp * sin(PLANT_TAU * (phases.x + hash));
    }
    // Storms past a gale lean further, up to 2.5 times a gale's lean.
    drag = clamp(drag, -PLANT_DRAG_FULL, 2.5 * PLANT_DRAG_FULL);
    let flat = vec2<f32>(dir.x, dir.z);
    let len = length(flat);
    if (len < 1e-4) { return vec2<f32>(0.0); }
    return flat / len * (trunk.z * height * drag);
}

// A bone plant vertex's packed color alpha, whole: the mesh pool keeps colors as half floats,
// which hold whole numbers up to 2048, and the 2048s above that in 32767ths in its normal's
// spare lane (`PoolVertex`, mesh.rs; 0 where the color is a full float, as on the cluster path).
fn plant_packed(lane: f32, alpha: f32) -> f32 {
    return alpha + round(lane * 32767.0) * 2048.0;
}

// A bone plant's vertex in the wind (mesh space): `pos`, `normal` at rest, `packed` its color
// alpha, `entry` its instance's table entry, `dir` the wind's direction in mesh space.
fn plant_wind(pos: vec3<f32>, normal: vec3<f32>, packed: f32, entry: u32, dir: vec3<f32>, air: PlantAir) -> Winded {
    let word = u32(max(packed, 0.0) + 0.5);
    let tip = f32((word >> 14u) & 31u) / 31.0;
    let edge = f32((word >> 19u) & 15u) / 15.0;
    // Benders push branches and leaves, not the trunk.
    var out = Winded(pos, normal, f32((word >> 23u) & 1u), select(tip, 0.0, (word & 0x3fffu) == 0u));
    let header = plant_wind_words[0];
    let mesh = plant_mesh(entry);
    let sp = plant_species(mesh.y);
    let detail = min(air.detail, mesh.z);
    let bones = header.z + 2u * mesh.x;
    let speed = air.wind.w;
    let hash = plant_hash(air.root_at);
    let gust = gust_waves(air.gusts, air.at);
    let u = speed * (1.0 + gust);
    let drag = min(plant_drag(u) / PLANT_DRAG_FULL, 1.5);
    let phases = select(plant_word(sp), plant_word(sp + 1u), air.prev);
    let modes = select(plant_word(sp + 2u).xy, plant_word(sp + 2u).zw, air.prev);
    let ramps_on = plant_word(sp + 3u);
    let ramps_full = plant_word(sp + 4u);
    let most = plant_word(sp + 5u);
    let flip = plant_word(sp + 6u);
    let swing = air.swing * plant_word(sp + 7u).w;

    // Leaves flutter along their normal, most at the tip and the edges (Crysis' edge flutter,
    // SpeedTree's ripple), at a rate that barely follows the wind (Tadrist et al. 2018).
    if (detail >= 2 && tip > 0.0 && most.w > 0.0) {
        let ramp = plant_ramp(ramps_on.w, ramps_full.w, u);
        let bone_phase = f32(word & 0x3fffu) * 0.618034;
        let wave = sin(PLANT_TAU * (phases.w + hash + bone_phase + edge * 0.37));
        out.pos += normal * (most.w * ramp * tip * (0.5 + edge) * wave * air.swing);
    }

    // Up the bones from the vertex's own, each turning about its pivot.
    var bone = word & 0x3fffu;
    for (var step = 0u; step < 4u; step++) {
        let w0 = plant_word(bones + 2u * bone);
        let w1 = plant_wind_words[bones + 2u * bone + 1u];
        let level = (w1.z >> 16u) & 3u;
        if (level == 0u) { break; }
        if (level <= detail) {
            let axis = plant_oct_decode(w1.x);
            let shape = unpack2x16unorm(w1.w);
            let stiffness = max(shape.y * 4.0, 0.1);
            var k = cross(axis, dir);
            if (length(k) < 1e-3) { k = cross(axis, plant_oct_decode(w1.y)); }
            k = normalize(k);
            let li = level - 1u;
            let ramp = plant_ramp(ramps_on[level], ramps_full[level], u);
            // Leeward bones go further, windward ones are pressed in (GPU Gems 3, ch. 6).
            let a2 = vec2<f32>(axis.x, axis.z);
            let d2 = vec2<f32>(dir.x, dir.z);
            let facing = dot(a2, d2) / max(length(a2) * length(d2), 1e-4);
            let side = mix(0.5, 1.4, saturate(0.5 + 0.5 * facing));
            let reach = most[li] / stiffness * ramp;
            let bend = reach * drag * side;
            // Its sway at its level's rate, and a weaker second mode that breaks the loop.
            var cycle = phases.z;
            var mode = modes.y;
            if (level == 1u) { cycle = phases.y; mode = modes.x; }
            if (level == 3u) { cycle = phases.w; mode = phases.w + 0.31; }
            let own = shape.x + hash;
            let osc = (sin(PLANT_TAU * (cycle + own)) + 0.25 * sin(PLANT_TAU * (mode + own * 1.7)))
                * reach * (0.4 + 0.6 * min(drag, 1.0)) * swing;
            var twist = 0.3 * osc;
            if (level == 3u) {
                // Sprays twist away from the wind, past 90 degrees showing their undersides.
                let face = plant_oct_decode(w1.y);
                let away = select(-1.0, 1.0, dot(face, dir) >= 0.0);
                twist += away * flip.z * plant_ramp(flip.x, flip.y, u) * (0.6 + 0.4 * min(gust, 1.0));
            }
            let pivot = w0.xyz;
            var r = plant_turn(out.pos - pivot, axis, twist);
            var n = plant_turn(out.normal, axis, twist);
            r = plant_turn(r, k, bend + osc);
            n = plant_turn(n, k, bend + osc);
            out.pos = pivot + r;
            out.normal = n;
        }
        let parent = w1.z & 0xffffu;
        if (parent == 0xffffu) { break; }
        bone = parent;
    }

    // The trunk bends over by its lean, keeping its length about the root (Crysis main bending).
    let root_words = plant_word(bones);
    let root = root_words.xyz;
    let height = max(root_words.w, 1e-3);
    let lean = plant_lean(sp, air, dir, height, hash);
    let rel = out.pos - root;
    let y = max(rel.y, 0.0) / height;
    let length0 = length(rel);
    var bent = rel + vec3<f32>(lean.x, 0.0, lean.y) * (y * y);
    bent = normalize(bent) * length0;
    out.pos = select(out.pos, root + bent, length0 > 1e-5);
    let slope = lean * (2.0 * y / height);
    var n = out.normal;
    n.y -= slope.x * n.x + slope.y * n.z;
    out.normal = normalize(n);
    return out;
}

// The wind's unit direction in mesh space, from the instance's normal matrix columns (the
// inverse model matrix is their transpose).
fn plant_wind_dir(n0: vec3<f32>, n1: vec3<f32>, n2: vec3<f32>, wind: vec4<f32>) -> vec3<f32> {
    let d = select(vec3<f32>(1.0, 0.0, 0.0), wind.xyz / max(wind.w, 1e-3), wind.w > 0.01);
    let m = vec3<f32>(dot(n0, d), dot(n1, d), dot(n2, d));
    return m / max(length(m), 1e-6);
}

// A bone plant vertex's albedo multiplier: its species' underside gain on a leaf's underside
// (paler: silvery aspen, grey-felted willow), so undersides flash pale as sprays flip.
// The share of the ground's snow mesh table entry `entry`'s species holds on its crown.
fn plant_snow_hold(entry: u32) -> f32 {
    return plant_word(plant_species(plant_mesh(entry).y) + 8u).x;
}

fn plant_underside_gain(entry: u32, underside: f32) -> f32 {
    let sp = plant_species(plant_mesh(entry).y);
    return mix(1.0, plant_word(sp + 6u).w, underside);
}
