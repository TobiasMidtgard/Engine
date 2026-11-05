// Grass: the cull pass. Requires lib_types + lib_frame + grass_common.
//
// One thread a blade: it grows the blade once (`grass_seed`: hashes, clump, maps, color),
// drops it when thinned out by distance or outside the view, works out its tip (wind,
// benders) and writes one 64-byte record that the scene, depth and shadow passes draw. Their
// vertices only bend the blade along its curve (`grass_vertex`), where they used to grow the
// whole blade again at every vertex, three times over in the depth pass (motion vectors).
// After Ghost of Tsushima's grass (Sucker Punch, GDC 2021: a compute pass writes each
// surviving blade's instance data, drawn indirectly) and Jahrmann and Wimmer's culled,
// compacted blades (I3D 2017).
//
// Jobs are chunks of up to 64 blades of one tile: first and end instance, what for (bit 0: the
// screen; bits 1-4: the shadow cascades it reaches) and the segments to draw. Records go to
// groups, each drawn by one indirect draw whose arguments are in `grass_args`: the screen's by
// segments 1..6, then each cascade's by segments 1..2. The CPU sets each draw's vertex count
// and first vertex (16 x the group's first record) and zeroes its count.

// How blades grow (`grass_seed`): blades grow in clumps (after Sucker Punch's grass in Ghost
// of Tsushima, GDC 2021): the nearest of a jittered grid of clump centers pulls its blades in,
// turns them to fan out from it and gives them its own height and color, so a field grows in
// tufts rather than as an even carpet. They sway in the wind (gusts roll across the field in
// broken bands) and are pushed aside by `GrassBender`s. On worn ground (a field's `GrassWear`
// map) blades shorten, lie flat, dry toward straw and thin out, gone where it is worn bare.
// Snow on the ground buries the blades' lower parts, short blades wholly, bows the rest under
// its weight and frosts them. Blades thin out with distance by their rank: blade `i` of a tile
// is kept while i / blades < density x lod(distance), and shrinks into the ground just before
// it goes, so thinning has no visible steps; the survivors get wider to keep the cover.
// Instance index = tile << 16 | blade.

struct GrassTile {
    corner: vec2<f32>,  // the tile's -x -z corner in field-local meters
    field: u32,
    segments: u32,      // quads per blade
    blades: u32,        // blades at full density
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};


struct GrassBenders {
    count: vec4<u32>,
    spheres: array<vec4<f32>, 16>, // camera-relative center xyz, radius (m)
    waves: GustWaves,              // the weather's gust fronts (engine_world::gusts)
    gust_camera: vec4<f32>,        // xy: the camera in the fronts' frame (m), z: their wind speed
};


@group(1) @binding(0) var<storage, read> grass_tiles: array<GrassTile>;
@group(1) @binding(2) var<uniform> grass_benders: GrassBenders;
@group(2) @binding(0) var grass_map: texture_2d<f32>; // ground height (m), density, dryness, unused
@group(2) @binding(1) var grass_wear: texture_2d<f32>; // how worn the ground is (r: 0 .. 1 bare)

const GRASS_TILE: f32 = 8.0;
const GRASS_BLADE_BITS: u32 = 16u;

// The clump a field-local point `p` grows in: the nearest of a grid of centers `size` meters
// apart, each jittered in its cell.
struct GrassClump {
    center: vec2<f32>,  // field-local meters
    dist: f32,          // distance to the center in cells (0 .. about 0.8)
    hash: vec4<f32>,    // the clump's own random numbers
};

fn grass_clump(p: vec2<f32>, size: f32, seed: u32) -> GrassClump {
    var c: GrassClump;
    let g = p / size;
    let cell = vec2<i32>(floor(g));
    var best = 1e9;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let k = cell + vec2<i32>(i, j);
            let h = grass_random(k, 0x51u, seed);
            let q = vec2<f32>(k) + h.xy;
            let d2 = dot(q - g, q - g);
            if (d2 < best) {
                best = d2;
                c.center = q * size;
                c.hash = h;
            }
        }
    }
    c.dist = sqrt(best);
    return c;
}

// Four independent uniform numbers in [0, 1) (PCG4D, Jarzynski and Olano 2020).
fn grass_random(key: vec2<i32>, index: u32, seed: u32) -> vec4<f32> {
    var v = vec4<u32>(bitcast<vec2<u32>>(key), index, seed) * 1664525u + 1013904223u;
    v.x += v.y * v.w;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v.w += v.y * v.z;
    v ^= v >> vec4<u32>(16u);
    v.x += v.y * v.w;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v.w += v.y * v.z;
    return vec4<f32>(v >> vec4<u32>(8u)) * (1.0 / 16777216.0);
}

// The field's map (height, density, dryness) at field-local `p` (meters), bilinear.
fn grass_map_at(f: GrassField, p: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(f.axis_x.w, f.axis_y.w);
    let res = vec2<i32>(f.grid.xy);
    let g = clamp(p / size, vec2<f32>(0.0), vec2<f32>(1.0)) * vec2<f32>(res - 1);
    let i = min(vec2<i32>(floor(g)), res - 2);
    let t = g - vec2<f32>(i);
    let a = textureLoad(grass_map, i, 0);
    let b = textureLoad(grass_map, i + vec2<i32>(1, 0), 0);
    let c = textureLoad(grass_map, i + vec2<i32>(0, 1), 0);
    let d = textureLoad(grass_map, i + vec2<i32>(1, 1), 0);
    return mix(mix(a, b, t.x), mix(c, d, t.x), t.y);
}

// How worn the ground is at field-local `p` (meters), 0 untouched .. 1 bare, bilinear.
fn grass_wear_at(f: GrassField, p: vec2<f32>) -> f32 {
    if (f.wear.w < 0.5) { return 0.0; }
    let res = vec2<i32>(textureDimensions(grass_wear));
    let g = (p - f.wear.xy) * f.wear.z;
    if (any(g < vec2<f32>(0.0)) || any(g > vec2<f32>(res - 1))) { return 0.0; }
    let i = min(vec2<i32>(floor(g)), max(res - 2, vec2<i32>(0)));
    let j = min(i + 1, res - 1);
    let t = g - vec2<f32>(i);
    let a = textureLoad(grass_wear, i, 0).r;
    let b = textureLoad(grass_wear, vec2<i32>(j.x, i.y), 0).r;
    let c = textureLoad(grass_wear, vec2<i32>(i.x, j.y), 0).r;
    let d = textureLoad(grass_wear, j, 0).r;
    return mix(mix(a, b, t.x), mix(c, d, t.x), t.y);
}

// Fraction of blades drawn at distance `d` (must match `lod_factor` in grass.rs).
fn grass_lod(f: GrassField, d: f32) -> f32 {
    let full = f.lod.x;
    let far = f.lod.y;
    let r = full / max(d, 1e-3);
    return min(1.0, r * r) * (1.0 - smoothstep(0.7 * far, far, d));
}

// How far the wind pushes a blade's tip sideways, per meter of blade, at tiling position `p`
// (world, wrapped) and time `t`, in steady wind `wind` that has moved `run` meters of air.
// Bands of stronger gusts roll across the field with the air, through patches of noise drifting
// with it. Everything that moves faster in stronger wind advances with the run, never with
// time x speed, so a change in wind changes how fast it moves on, not where it is.
fn grass_wind(p: vec3<f32>, t: f32, wind: vec3<f32>, run: f32, phase: f32) -> vec3<f32> {
    let speed = length(wind);
    let dir = select(vec3<f32>(1.0, 0.0, 0.0), wind / max(speed, 1e-3), speed > 0.01);
    let along = dot(p, dir);
    let across = dot(p.xz, vec2<f32>(-dir.z, dir.x));

    // Gust fronts ride the air (about 35 m apart), bent into broken bands across it.
    let wave = (along - run) * 0.18 - t * 0.35 + 0.8 * sin(across * 0.07 + t * 0.21);
    let roll = pow(0.5 + 0.5 * sin(wave), 2.0);
    let q = vec2<f32>((along - 0.9 * run) * 0.06 - t * 0.03, across * 0.05);
    let n = 0.65 * value_noise2(q) + 0.35 * value_noise2(q * 2.7 + vec2<f32>(5.3, 1.7));
    let gust = min(roll * (0.5 + 0.75 * smoothstep(0.25, 0.8, n)), 1.0);
    // In strong wind, tufts bob a little out of step with each other, slowly: well under a
    // hertz, a little faster in stronger wind (`phase` is the clump's, so a tuft moves as one).
    let flutter = sin(t * 1.6 + run * 0.12 + phase * 2.4 + along * 0.35) * 0.18
        * smoothstep(3.0, 8.0, speed);
    let strength = min(speed / 9.0, 1.4);
    let idle = 0.03 * sin(t * 1.1 + phase * TAU);
    return dir * (strength * (0.2 + 0.8 * gust + flutter * (0.4 + gust)) + idle);
}


// The wind on a blade at camera-relative `rel` when the weather's gust fronts blow
// (`grass_benders.waves`): the same fronts the trees and everything else feel, met where the
// blade stands. As `grass_wind`, a push per meter of blade, added to the lean and normalized.
//
// - Drag grows as U^2 in a breeze and about as U in a gale (blades streamline):
//   U^2 / (6 m/s + U).
// - The lean saturates: fine grasses lie at most ~70 degrees (1 - exp(-drag / F) of it), F
//   being the drag that flattens a blade 63%: about 12 / `response` (Vogel 1989; Py et al. 2006):
//   a gale lays it at ~40 degrees between fronts and ~60 in them.
// - Blades bow a little behind a front, overshoot as it passes and spring back after it (the
//   honami of a wheat field): the lean follows the front now plus a share of how much stronger
//   it is now than a moment ago, read upwind (the fronts ride the wind).
// - Inside fronts, tall grass carries short waves (1.5-2 m apart at ~1.5 Hz, Py et al.).
// `t` is this frame's time or the previous frame's (motion vectors): the fronts were then
// upwind by the distance the wind has moved since.
fn grass_front_wind(rel: vec3<f32>, t: f32, wind: vec3<f32>, phase: f32, height: f32, response: f32) -> vec3<f32> {
    let speed = length(wind);
    let dir = select(vec3<f32>(1.0, 0.0, 0.0), wind / max(speed, 1e-3), speed > 0.01);
    let d2 = normalize(vec2<f32>(dir.x, dir.z) + vec2<f32>(1e-5, 0.0));
    let back = frame.time.x - t;
    let front_speed = max(grass_benders.gust_camera.z, speed);
    let x = rel.xz + grass_benders.gust_camera.xy + d2 * front_speed * back;
    let g0 = gust_waves(grass_benders.waves, x - d2 * speed * 0.15);
    let g1 = gust_waves(grass_benders.waves, x - d2 * speed * 0.6);
    let stiff = 12.0 / max(response, 0.05);
    let lean0 = grass_lean(speed * (1.0 + g0), stiff);
    let lean1 = grass_lean(speed * (1.0 + g1), stiff);
    var lean = max(lean0 + 0.3 * (lean0 - lean1), 0.0);
    // Short waves through tall grass, only inside the fronts.
    let along = dot(x, d2);
    let wave_m = mix(1.5, 2.2, saturate(speed / 15.0));
    let canopy = 0.15 * lean * smoothstep(0.4, 0.8, height) * saturate(g0 * 3.0)
        * sin(GUST_TAU * (along / wave_m - 1.5 * t) + phase * 0.6);
    lean = min(lean + canopy, 1.35);
    // Tufts still bob a little out of step, slowly, and stir in still air.
    let flutter = 0.12 * sin(t * 1.6 + phase * 2.4 + along * 0.35) * smoothstep(3.0, 8.0, speed);
    let idle = 0.03 * sin(t * 1.1 + phase * TAU);
    // The push (per meter of blade) that leans a blade by `lean` radians: tan(lean) / 1.2
    // (`grass_tip` adds 1.2 x the push to the lean).
    return dir * (tan(lean) / 1.2 + flutter * min(speed / 9.0, 1.0)) + dir * idle;
}

// How far (radians) a blade leans in a steady wind of `u` m/s, `stiff` being the drag that
// flattens it 63% of the way to 70 degrees.
fn grass_lean(u: f32, stiff: f32) -> f32 {
    let drag = u * u / (6.0 + u);
    return 1.22 * (1.0 - exp(-drag / stiff));
}

// Everything about one blade that is the same at every vertex: where it grows, how tall and
// wide, which way it faces and leans, its color. `grass_seed` works it out from the blade's
// tile and index (the costly part: hashes, the clump search, the maps); `grass_tip` adds the
// wind and the benders at a time; `grass_vertex` places one vertex.
struct BladeSeed {
    visible: bool,
    field: u32,
    root: vec3<f32>,     // camera-relative
    face2: vec2<f32>,    // facing, field-local (unit)
    height: f32,         // m, out of the snow
    width: f32,          // m, at the root
    lean: vec3<f32>,     // tip direction in still air
    phase: f32,          // the clump's sway phase
    ripple: f32,         // the ripple's phase (0..1)
    rise: f32,           // a leaf's stalk and a lying leaf's curl (0..1)
    trodden: f32,
    snowed: f32,
    albedo: vec3<f32>,   // before the tip tint, snow and shade along the blade
    clump: vec2<f32>,    // the clump's own numbers that tilt its far normal (0..1)
    flower: f32,         // 1: a flowering stem
};

// The kind of grass (0..kinds-1) at the map point nearest field-local `p` (meters).
fn grass_kind_at(f: GrassField, p: vec2<f32>) -> u32 {
    let size = vec2<f32>(f.axis_x.w, f.axis_y.w);
    let res = vec2<i32>(f.grid.xy);
    let g = clamp(p / size, vec2<f32>(0.0), vec2<f32>(1.0)) * vec2<f32>(res - 1);
    let k = u32(round(textureLoad(grass_map, vec2<i32>(round(g)), 0).w));
    return min(k, max(f.grid.z, 1u) - 1u);
}


// Blade `instance` (tile << 16 | blade): where it grows and how, or not visible when thinned
// out for the screen or, with `shadow`, for the shadow maps (`grass_keep`), worn away or
// buried in snow.
fn grass_seed(instance: u32, shadow: bool) -> BladeSeed {
    var out: BladeSeed;
    out.visible = false;
    let tile = grass_tiles[instance >> GRASS_BLADE_BITS];
    let index = instance & ((1u << GRASS_BLADE_BITS) - 1u);
    out.field = tile.field;
    var f = grass_fields[tile.field];
    let key = vec2<i32>(round(tile.corner / GRASS_TILE));
    let r0 = grass_random(key, index, f.grid.w);
    let r1 = grass_random(key, index, f.grid.w ^ 0x9e3779b9u);
    var local = tile.corner + r0.xy * GRASS_TILE;
    let rank = (f32(index) + 0.5) / f32(tile.blades);
    // A blade thinned out by distance leaves before the costly part (its clump, the maps): the
    // camera is no nearer than across the field's plane, less what the clump's pull can move it.
    let flat_rel = f.origin.xyz + f.axis_x.xyz * local.x + f.axis_z.xyz * local.y;
    let level = flat_rel - f.axis_y.xyz * dot(flat_rel, f.axis_y.xyz);
    let nearest = max(length(level) - 2.0 * f.clump.x, 0.0);
    if (rank >= grass_lod(f, nearest) * grass_keep(f, nearest, shadow)) { return out; }
    // Its kind: the nearest map point to a spot about a meter around it, so kinds meet in
    // ragged drifts. Its entry has the same placement and distances, its kind's look.
    let r2 = grass_random(key, index, f.grid.w ^ 0x2545f491u);
    if (f.grid.z > 1u) {
        let kind = grass_kind_at(f, local + (r2.xy - 0.5) * 1.6);
        out.field = tile.field + kind;
        f = grass_fields[out.field];
    }
    // The clump: blades crowd in toward its center.
    var clump: GrassClump;
    clump.center = local;
    clump.dist = 0.5;
    clump.hash = vec4<f32>(r1.x, 0.5, 0.5, r1.y);
    let clumped = f.clump.x > 0.01;
    if (clumped) {
        clump = grass_clump(local, f.clump.x, f.grid.w);
        let pull = f.clump.y * (0.5 + 0.5 * r1.w);
        local = mix(local, clump.center, pull);
        clump.dist *= 1.0 - pull;
    }
    if (local.x < 0.0 || local.y < 0.0 || local.x > f.axis_x.w || local.y > f.axis_y.w) { return out; }
    let m = grass_map_at(f, local);
    // Meadows grow in patches: taller, greener hollows and shorter, drier rises a few meters
    // across, and the clumps within them.
    let meadow_patch = value_noise2(local * 0.11 + vec2<f32>(f32(f.grid.w & 1023u)));
    let up = f.axis_y.xyz;
    let root = f.origin.xyz + f.axis_x.xyz * local.x + f.axis_z.xyz * local.y + up * m.x;
    let d = length(root);
    let lod = grass_lod(f, d);
    // Worn ground: the blades trodden flat first, then fewer of them, none on a bare path.
    let worn = grass_wear_at(f, local);
    let trodden = smoothstep(0.03, 0.5, worn);
    let keep = m.y * lod * grass_keep(f, d, shadow) * (1.0 - smoothstep(0.2, 0.95, worn)) * f.kind.y;
    if (rank >= keep) { return out; }
    // Shrink into the ground just before being thinned out, and at thin edges of the meadow.
    let fade = saturate((keep - rank) * 10.0);
    // Clumps differ in height, and a tuft stands tallest in its middle.
    let clump_height = 1.0 + f.clump.w * (clump.hash.z * 2.0 - 1.0);
    let dome = mix(1.0, 1.12 - 0.4 * clump.dist, f.clump.y);
    let grown = f.shape.x * (1.0 + f.shape.y * (r0.z * 2.0 - 1.0)) * mix(0.55, 1.0, saturate(m.y))
        * mix(0.75, 1.2, meadow_patch) * clump_height * dome * fade * (1.0 - 0.45 * trodden);
    // Snow buries the lower part of a blade (the ground's snow is drawn on the ground, so what
    // stands out of it is drawn from the ground up).
    let snow = max(grass_snow_depth(f, root), 0.0);
    // Flowering stems stand a little taller and straighter (BLOOM_RISE in grass.rs), above
    // the blades around them.
    let flower = r2.z < f.bloom.w;
    let height = grown * select(1.0, 1.15, flower) - snow;
    if (height < 0.008) { return out; }
    let leaf = f.lush.w;
    // Thinned out, the survivors widen to keep the cover (leaves less: they'd turn into plates).
    // (Out to 8 times as wide: Ghost of Tsushima keeps a quarter of the blades, twice as wide,
    // at each doubling of the distance, so a meadow keeps its cover to 150 m.)
    let widen = clamp(pow(max(lod, 1e-3), -0.6), 1.0, mix(8.0, 1.5, leaf));
    out.width = f.shape.z * (0.75 + 0.5 * r1.x) * widen;
    if (flower) {
        // A flowering stem's width is its head's (the stem a tenth of it).
        out.width = f.kind.x * (0.8 + 0.4 * r1.x) * widen;
    }

    // Blade frame: facing its own way, or fanning out from its clump's center; leaning over
    // toward it, the clump's outer blades the most.
    let angle = r1.y * TAU;
    var face2 = vec2<f32>(cos(angle), sin(angle));
    let out_of = local - clump.center;
    let along_clump = vec2<f32>(cos(clump.hash.w * TAU), sin(clump.hash.w * TAU));
    let fan = select(along_clump, normalize(out_of) + 0.35 * along_clump, dot(out_of, out_of) > 1e-6);
    let fanned = face2 + (normalize(fan) - face2) * f.clump.z;
    face2 = select(face2, normalize(fanned), dot(fanned, fanned) > 1e-6);
    let facing = f.axis_x.xyz * face2.x + f.axis_z.xyz * face2.y;
    let lean = f.shape.w * (0.2 + 0.8 * r1.z) * mix(1.0, 0.55 + 1.1 * clump.dist, f.clump.z)
        * select(1.0, 0.4, flower);

    // Color: fresh to dry by the map, the clump and per blade.
    let cc = f.look.x;
    var dryness = saturate(m.z + (r1.w - 0.5) * 0.35 + (0.5 - meadow_patch) * 0.25 + (clump.hash.y - 0.5) * cc * 1.5);
    dryness = mix(dryness, 1.0, trodden * 0.6);
    var albedo = mix(f.lush.rgb, f.dry.rgb, dryness) * (0.8 + 0.4 * r0.z * r1.w);
    albedo *= 1.0 + cc * (clump.hash.x * 2.0 - 1.0);
    // A slight hue drift between patches and clumps, bluer in the lush hollows.
    albedo *= mix(vec3<f32>(0.9, 1.0, 1.08), vec3<f32>(1.08, 1.02, 0.85), clump.hash.z * 0.5 + meadow_patch * 0.5);

    out.root = root;
    out.face2 = face2;
    out.height = height;
    out.lean = normalize(up + facing * lean);
    out.phase = fract(clump.hash.w + 0.2 * r0.w);
    out.ripple = r0.w;
    out.rise = r1.z;
    out.trodden = trodden;
    out.snowed = smoothstep(0.0, 0.06, snow);
    out.albedo = albedo;
    out.clump = clump.hash.xy;
    out.flower = select(0.0, 1.0, flower);
    out.visible = true;
    return out;
}

// The direction from a blade's root to its tip at time `t`: its lean, swayed by the wind and
// pushed aside by benders; trodden blades lie down and snow bows them over. `tiling` is the
// camera's tiling position (world position modulo 4096 m), so the wind pattern stays put.
fn grass_tip(s: BladeSeed, t: f32, tiling: vec3<f32>) -> vec3<f32> {
    let f = grass_fields[s.field];
    let up = f.axis_y.xyz;
    let facing = f.axis_x.xyz * s.face2.x + f.axis_z.xyz * s.face2.y;
    let root = s.root;
    let height = s.height;
    var tip_dir = s.lean;
    let wind = f.gust.xyz;
    let run = f.gust.w + length(wind) * t;
    var w: vec3<f32>;
    if (grass_benders.waves.broad_a.w > 0.0 || grass_benders.waves.fine_a.w > 0.0) {
        w = grass_front_wind(root, t, wind, s.phase, height, f.dry.w);
    } else {
        w = grass_wind(root + tiling, t, wind, run, s.phase) * f.dry.w;
    }
    tip_dir = normalize(tip_dir + (w - up * dot(w, up)) * 1.2);
    // Benders push blades away from their center and flatten them under it.
    for (var i = 0u; i < min(grass_benders.count.x, 16u); i++) {
        let b = grass_benders.spheres[i];
        let to = root - b.xyz;
        let rise = dot(to, up);
        let flat = to - up * rise;
        let reach = b.w + height * 0.5;
        let dist = length(flat);
        if (dist < reach && rise > -b.w - height && rise < b.w + 0.05) {
            let push = sqrt(1.0 - dist / reach);
            let away = select(facing, flat / max(dist, 1e-4), dist > 1e-3);
            tip_dir = normalize(mix(tip_dir, normalize(away + up * 0.15), push));
        }
    }
    // Trodden blades lie down; snow bows them over.
    return normalize(mix(tip_dir, normalize(facing + up * 0.2), max(s.trodden * 0.75, s.snowed * 0.45)));
}

// A blade's shape at time `t` with its tip pointing along `tip` (`s.tip`, or `s.tip_prev`
// for where it was last frame).
fn grass_shape(s: BladeSeed, tip: vec3<f32>, tip_prev: vec3<f32>, segments: u32) -> BladeShape {
    var out: BladeShape;
    out.field = s.field;
    out.segments = segments;
    out.root = s.root;
    out.face2 = s.face2;
    out.height = s.height;
    out.width = s.width;
    out.tip = tip;
    out.tip_prev = tip_prev;
    out.ripple = s.ripple;
    out.rise = s.rise;
    out.snowed = s.snowed;
    out.albedo = s.albedo;
    out.clump = s.clump;
    out.flower = s.flower;
    return out;
}


// The fraction of a tile's blades kept at distance `d`: on screen all drawn
// (`grass_lod` thins them), in the shadow maps fewer further out (must match `shadow_keep`
// in grass.rs).
fn grass_keep(f: GrassField, d: f32, shadow: bool) -> f32 {
    return select(1.0, 1.0 - smoothstep(0.4 * f.lod.w, f.lod.w, d), shadow);
}

// Snow depth (m) on the ground at camera-relative `rel`. The ground's snow is stored up to
// SURFACE_SNOW_SCALE_M; deeper snow reads as full, so there take the depth by the camera
// (exact) when it is deeper.
fn grass_snow_depth(f: GrassField, rel: vec3<f32>) -> f32 {
    let snow = surface_weather(rel).z * SURFACE_SNOW_SCALE_M;
    return select(snow, max(snow, f.look.w), snow > 0.95 * SURFACE_SNOW_SCALE_M);
}

@group(1) @binding(3) var<storage, read> grass_jobs: array<vec4<u32>>;
@group(1) @binding(4) var<storage, read_write> grass_records: array<GrassRecord>;
@group(1) @binding(5) var<storage, read_write> grass_args: array<atomic<u32>>;
// This field's jobs: first, count.
@group(2) @binding(2) var<uniform> grass_cull: vec4<u32>;

const GRASS_SCREEN_GROUPS: u32 = 6u;
const GRASS_SHADOW_SEGMENTS: u32 = 2u;

// Whether a sphere (camera-relative center, radius) is inside the sides of the view `m`
// projects to (a perspective view, or a cascade's orthographic box seen from the sun).
fn grass_in_view(m: mat4x4<f32>, center: vec3<f32>, radius: f32) -> bool {
    let x = vec4<f32>(m[0].x, m[1].x, m[2].x, m[3].x);
    let y = vec4<f32>(m[0].y, m[1].y, m[2].y, m[3].y);
    let w = vec4<f32>(m[0].w, m[1].w, m[2].w, m[3].w);
    let p = vec4<f32>(center, 1.0);
    let planes = array<vec4<f32>, 4>(w + x, w - x, w + y, w - y);
    for (var i = 0; i < 4; i++) {
        let n = planes[i];
        if (dot(n, p) < -radius * length(n.xyz)) { return false; }
    }
    return true;
}

// Appends a blade to draw group `g`.
fn grass_emit(g: u32, record: GrassRecord) {
    let first = atomicLoad(&grass_args[g * 4u + 2u]) >> 4u;
    let slot = atomicAdd(&grass_args[g * 4u + 1u], 1u);
    grass_records[first + slot] = record;
}

@compute @workgroup_size(64)
fn cs_cull(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    let j = group.x + group.y * 65535u;
    if (j >= grass_cull.y) { return; }
    let job = grass_jobs[grass_cull.x + j];
    let instance = job.x + lane;
    if (instance >= job.y) { return; }
    let screen = (job.z & 1u) != 0u;
    let seed = grass_seed(instance, !screen);
    if (!seed.visible) { return; }
    let up = grass_fields[seed.field].axis_y.xyz;
    // Leaning, swaying and widened edge-on, a blade stays within about its height of the
    // middle of its root's column.
    let center = seed.root + up * (seed.height * 0.5);
    let radius = seed.height * 1.1 + seed.width * 4.0 + 0.02;
    let tiling = frame.camera_world_frac.xyz;
    let segments = clamp(job.w, 1u, 6u);
    if (screen) {
        if (!grass_in_view(frame.view_proj, center, radius)) { return; }
        let tip = grass_tip(seed, frame.time.x, tiling);
        let prev = grass_tip(seed, frame.time.x - frame.time.z, tiling);
        grass_emit(segments - 1u, grass_pack(grass_shape(seed, tip, prev, segments)));
        return;
    }
    let shadow_segments = min(segments, GRASS_SHADOW_SEGMENTS);
    var record: GrassRecord;
    var made = false;
    for (var c = 0u; c < 4u; c++) {
        if ((job.z & (2u << c)) == 0u || !grass_in_view(frame.shadow_cascades[c], center, radius)) {
            continue;
        }
        if (!made) {
            let tip = grass_tip(seed, frame.time.x, tiling);
            record = grass_pack(grass_shape(seed, tip, tip, shadow_segments));
            made = true;
        }
        grass_emit(GRASS_SCREEN_GROUPS + c * GRASS_SHADOW_SEGMENTS + shadow_segments - 1u, record);
    }
}
