// Grass blades, shared by the scene and prepass shader (grass.wgsl) and the shadow shader
// (grass_shadow.wgsl). Requires lib_types.
//
// Every blade is generated in the vertex shader from its tile's grid coordinates and its index,
// so the same blade grows in the same spot every frame and nothing per blade is stored. Blades
// grow in clumps (after Sucker Punch's grass in Ghost of Tsushima, GDC 2021): the nearest of a
// jittered grid of clump centers pulls its blades in, turns them to fan out from it and gives
// them its own height and color, so a field grows in tufts rather than as an even carpet. A blade
// is a cubic Bezier from the root (on the field's height map) rising straight, arching over
// toward its tip and drooping under its own weight; it sways in the wind (gusts roll across the
// field in broken bands, and a ripple runs up each blade) and is pushed aside by
// `GrassBender`s. On worn ground (a field's `GrassWear` map) blades shorten, lie flat, dry
// toward straw and thin out, gone where it is worn bare. Snow on the ground (`grass_snow_depth`,
// which each shader including this one defines) buries the blades' lower parts, short blades
// wholly, bows the rest under its weight and frosts them. It is shaded as if rounded across its
// width, and widened in view when seen edge-on so a field stays full instead of breaking into
// slivers. Blades thin out with distance by their rank: blade `i` of a tile is drawn while i / blades < density x lod(distance), and
// shrinks into the ground just before it goes, so thinning has no visible steps; the survivors
// get wider to keep the cover. Instance index = tile << 16 | blade.

struct GrassTile {
    corner: vec2<f32>,  // the tile's -x -z corner in field-local meters
    field: u32,
    segments: u32,      // quads per blade
    blades: u32,        // blades at full density
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

struct GrassField {
    origin: vec4<f32>,  // camera-relative position of the field's -x -z corner (xyz)
    axis_x: vec4<f32>,  // field x axis (unit, world), w: size along x (m)
    axis_y: vec4<f32>,  // field up, w: size along z (m)
    axis_z: vec4<f32>,  // field z axis, w: unused
    grid: vec4<u32>,    // map points along x and z, unused, seed
    shape: vec4<f32>,   // blade height (m), height variation, root width (m), bend
    lush: vec4<f32>,    // fresh albedo (linear), unused
    dry: vec4<f32>,     // dry albedo (linear), wind response
    lod: vec4<f32>,     // full-density distance (m), draw distance (m), unused, unused
    clump: vec4<f32>,   // clump size (m, 0 = none), pull to center, fan out, height spread
    look: vec4<f32>,    // clump color spread, droop, gloss, snow depth by the camera (m)
    tip: vec4<f32>,     // tip tint (rgb, x the blade's color), unused
    wear: vec4<f32>,    // worn-ground map: first point (field-local m, xz), points per m, 1 = present
    gust: vec4<f32>,    // steady wind (m/s, xyz), w: the wind's run (m) less speed x time
};

struct GrassBenders {
    count: vec4<u32>,
    spheres: array<vec4<f32>, 16>, // camera-relative center xyz, radius (m)
};

@group(1) @binding(0) var<storage, read> grass_tiles: array<GrassTile>;
@group(1) @binding(1) var<storage, read> grass_fields: array<GrassField>;
@group(1) @binding(2) var<uniform> grass_benders: GrassBenders;
@group(2) @binding(0) var grass_map: texture_2d<f32>; // ground height (m), density, dryness, unused
@group(2) @binding(1) var grass_wear: texture_2d<f32>; // how worn the ground is (r: 0 .. 1 bare)

const GRASS_TILE: f32 = 8.0;
const GRASS_BLADE_BITS: u32 = 16u;

struct Blade {
    rel: vec3<f32>,     // camera-relative position of this vertex
    normal: vec3<f32>,  // front-side normal (world, unit)
    along: f32,         // 0 at the root, 1 at the tip
    across: f32,        // -1 .. 1 from edge to edge
    albedo: vec3<f32>,
    gloss: f32,         // 0 matte .. 1 glossy along the blade's middle
    visible: bool,
};

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
    return min(1.0, pow(full / max(d, 1e-3), 1.5)) * (1.0 - smoothstep(0.7 * far, far, d));
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
    // Blades bob a little out of step with their neighbours, slowly: well under a hertz, a
    // little faster in stronger wind.
    let flutter = sin(t * 1.6 + run * 0.12 + phase * 2.4 + along * 0.35) * 0.18;
    let strength = min(speed / 9.0, 1.4);
    let idle = 0.03 * sin(t * 1.1 + phase * TAU);
    return dir * (strength * (0.2 + 0.8 * gust + flutter * (0.4 + gust)) + idle);
}

// Vertex `vertex` of blade `instance` at time `t`, in its field's steady wind; `tiling` is the camera's
// tiling position (world position modulo 4096 m) so the wind pattern stays put in the world.
fn grass_blade(instance: u32, vertex: u32, t: f32, tiling: vec3<f32>) -> Blade {
    var out: Blade;
    out.visible = false;
    out.rel = vec3<f32>(0.0);
    out.normal = vec3<f32>(0.0, 1.0, 0.0);
    out.along = 0.0;
    out.across = 0.0;
    out.albedo = vec3<f32>(0.0);
    out.gloss = 0.0;
    let tile = grass_tiles[instance >> GRASS_BLADE_BITS];
    let index = instance & ((1u << GRASS_BLADE_BITS) - 1u);
    let f = grass_fields[tile.field];
    let key = vec2<i32>(round(tile.corner / GRASS_TILE));
    let r0 = grass_random(key, index, f.grid.w);
    let r1 = grass_random(key, index, f.grid.w ^ 0x9e3779b9u);
    var local = tile.corner + r0.xy * GRASS_TILE;
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
    let keep = m.y * lod * (1.0 - smoothstep(0.2, 0.95, worn));
    let rank = (f32(index) + 0.5) / f32(tile.blades);
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
    let height = grown - snow;
    if (height < 0.008) { return out; }
    let snowed = smoothstep(0.0, 0.06, snow);
    let width = f.shape.z * (0.75 + 0.5 * r1.x) * clamp(inverseSqrt(max(lod, 1e-3)), 1.0, 3.0);

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
    let side = cross(up, facing);
    let lean = f.shape.w * (0.2 + 0.8 * r1.z) * mix(1.0, 0.55 + 1.1 * clump.dist, f.clump.z);
    var tip_dir = normalize(up + facing * lean);
    let wind = f.gust.xyz;
    let run = f.gust.w + length(wind) * t;
    let w = grass_wind(root + tiling, t, wind, run, r0.w) * f.dry.w;
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
    tip_dir = normalize(mix(tip_dir, normalize(facing + up * 0.2), max(trodden * 0.75, snowed * 0.45)));
    // Cubic Bezier: rising straight from the root, arching over toward the tip, which droops
    // under its weight the further the blade leans.
    let over = tip_dir - up * dot(tip_dir, up);
    let leaning = length(over);
    let p0 = root;
    let p1 = root + up * (height * 0.38);
    let p2 = root + up * (height * 0.74) + over * (height * 0.5);
    let p3 = root + tip_dir * (height * (1.0 - 0.12 * leaning)) - up * (height * f.look.y * leaning * 0.8);

    // Vertex: two per row, then the tip; rows closer together toward the tip, where it bends.
    let segments = max(tile.segments, 1u);
    var s = f32(vertex & 1u) * 2.0 - 1.0;
    var u = f32(vertex / 2u) / f32(segments);
    if (vertex >= 2u * segments) {
        u = 1.0;
        s = 0.0;
    }
    u = 1.0 - pow(1.0 - u, 1.35);
    let q = 1.0 - u;
    var pos = q * q * q * p0 + 3.0 * q * q * u * p1 + 3.0 * q * u * u * p2 + u * u * u * p3;
    let tangent = normalize(3.0 * q * q * (p1 - p0) + 6.0 * q * u * (p2 - p1) + 3.0 * u * u * (p3 - p2) + up * 1e-4);
    // A ripple runs up the blade, strongest at the tip (barely at all in still air).
    let speed = length(wind);
    let wind_dir = select(facing, wind / max(speed, 1e-3), speed > 0.01);
    let ripple = sin(t * 2.6 + run * 0.2 + r0.w * TAU - u * 2.2) * u * u;
    pos += wind_dir * (ripple * height * (0.003 + 0.06 * min(speed / 8.0, 1.5)) * f.dry.w);
    let half_width = 0.5 * width * pow(q, 0.6);
    // Seen edge-on a blade is widened across the view, so the field keeps its cover (from a
    // few meters out: close up it would read as a broad ribbon).
    let v = normalize(-pos);
    let face_n = normalize(cross(side, tangent));
    let edge = 1.0 - abs(dot(face_n, v));
    var across_view = cross(tangent, v);
    across_view = select(side, normalize(across_view), dot(across_view, across_view) > 1e-8);
    across_view *= sign(dot(across_view, side) + 1e-6);
    out.rel = pos + side * (s * half_width) + across_view * (s * half_width * edge * edge * 1.2 * smoothstep(1.5, 6.0, d));
    // Rounded across the blade; far away, shaded like its clump (one rounded mound).
    var n = normalize(face_n + side * (s * 0.45));
    let clump_n = normalize(up + (f.axis_x.xyz * (clump.hash.x - 0.5) + f.axis_z.xyz * (clump.hash.y - 0.5)) * 0.5);
    n = normalize(mix(n, clump_n, smoothstep(0.15, 1.0, d / f.lod.y) * 0.7));
    out.normal = n;
    out.along = u;
    out.across = s;

    // Color: fresh to dry by the map, the clump and per blade; tips tinted.
    let cc = f.look.x;
    var dryness = saturate(m.z + (r1.w - 0.5) * 0.35 + (0.5 - meadow_patch) * 0.25 + (clump.hash.y - 0.5) * cc * 1.5);
    dryness = mix(dryness, 1.0, trodden * 0.6);
    var albedo = mix(f.lush.rgb, f.dry.rgb, dryness) * (0.8 + 0.4 * r0.z * r1.w);
    albedo *= 1.0 + cc * (clump.hash.x * 2.0 - 1.0);
    // A slight hue drift between patches and clumps, bluer in the lush hollows.
    albedo *= mix(vec3<f32>(0.9, 1.0, 1.08), vec3<f32>(1.08, 1.02, 0.85), clump.hash.z * 0.5 + meadow_patch * 0.5);
    albedo = mix(albedo, albedo * f.tip.rgb, u * u);
    // Snow caught on the blades, most on their arched upper parts.
    albedo = mix(albedo, vec3<f32>(0.82, 0.85, 0.88), snowed * (0.25 + 0.45 * u));
    out.albedo = albedo;
    // Glossy up close; far off the sheen would only sparkle.
    out.gloss = f.look.z * (1.0 - smoothstep(8.0, 35.0, d));
    out.visible = true;
    return out;
}
