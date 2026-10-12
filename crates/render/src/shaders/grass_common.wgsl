// Grass blades, shared by the scene and prepass shader (grass.wgsl) and the shadow shader
// (grass_shadow.wgsl). Requires lib_types.
//
// Every blade is generated in the vertex shader from its tile's grid coordinates and its index,
// so the same blade grows in the same spot every frame and nothing per blade is stored. A blade
// is a quadratic Bezier from the root (on the field's height map) through a control point above
// it to the tip, which leans, sways in the wind (gusts roll across the field with the wind) and
// is pushed aside by `GrassBender`s. Blades thin out with distance by their rank: blade `i` of a
// tile is drawn while i / blades < density x lod(distance), and shrinks into the ground just
// before it goes, so thinning has no visible steps; the survivors get wider to keep the cover.
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
};

struct GrassBenders {
    count: vec4<u32>,
    spheres: array<vec4<f32>, 16>, // camera-relative center xyz, radius (m)
};

@group(1) @binding(0) var<storage, read> grass_tiles: array<GrassTile>;
@group(1) @binding(1) var<storage, read> grass_fields: array<GrassField>;
@group(1) @binding(2) var<uniform> grass_benders: GrassBenders;
@group(2) @binding(0) var grass_map: texture_2d<f32>; // ground height (m), density, dryness, unused

const GRASS_TILE: f32 = 8.0;
const GRASS_BLADE_BITS: u32 = 16u;

struct Blade {
    rel: vec3<f32>,     // camera-relative position of this vertex
    normal: vec3<f32>,  // front-side normal (world, unit)
    along: f32,         // 0 at the root, 1 at the tip
    albedo: vec3<f32>,
    visible: bool,
};

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

// Fraction of blades drawn at distance `d` (must match `lod_factor` in grass.rs).
fn grass_lod(f: GrassField, d: f32) -> f32 {
    let full = f.lod.x;
    let far = f.lod.y;
    return min(1.0, pow(full / max(d, 1e-3), 1.5)) * (1.0 - smoothstep(0.7 * far, far, d));
}

// How far the wind pushes a blade's tip sideways, per meter of blade, at tiling position `p`
// (world, wrapped) and time `t`. Bands of stronger gusts roll across the field with the wind.
fn grass_wind(p: vec3<f32>, t: f32, wind: vec3<f32>, phase: f32) -> vec3<f32> {
    let speed = length(wind);
    let dir = select(vec3<f32>(1.0, 0.0, 0.0), wind / max(speed, 1e-3), speed > 0.01);
    let along = dot(p, dir);
    let across = dot(p.xz, vec2<f32>(-dir.z, dir.x));
    let wave = along * 0.18 - t * (0.6 + speed * 0.45) + 0.8 * sin(across * 0.07 + t * 0.21);
    let gust = pow(0.5 + 0.5 * sin(wave), 2.0);
    let flutter = sin(t * (2.0 + speed * 0.3) + phase * TAU + along * 0.9) * 0.25;
    let strength = min(speed / 9.0, 1.4);
    let idle = 0.03 * sin(t * 1.3 + phase * TAU);
    return dir * (strength * (0.2 + 0.8 * gust + flutter * (0.4 + gust)) + idle);
}

// Vertex `vertex` of blade `instance` at time `t` in wind `wind`; `tiling` is the camera's
// tiling position (world position modulo 4096 m) so the wind pattern stays put in the world.
fn grass_blade(instance: u32, vertex: u32, t: f32, wind: vec3<f32>, tiling: vec3<f32>) -> Blade {
    var out: Blade;
    out.visible = false;
    out.rel = vec3<f32>(0.0);
    out.normal = vec3<f32>(0.0, 1.0, 0.0);
    out.along = 0.0;
    out.albedo = vec3<f32>(0.0);
    let tile = grass_tiles[instance >> GRASS_BLADE_BITS];
    let index = instance & ((1u << GRASS_BLADE_BITS) - 1u);
    let f = grass_fields[tile.field];
    let key = vec2<i32>(round(tile.corner / GRASS_TILE));
    let r0 = grass_random(key, index, f.grid.w);
    let r1 = grass_random(key, index, f.grid.w ^ 0x9e3779b9u);
    let local = tile.corner + r0.xy * GRASS_TILE;
    if (local.x > f.axis_x.w || local.y > f.axis_y.w) { return out; }
    let m = grass_map_at(f, local);
    // Meadows grow in patches: taller, greener hollows and shorter, drier rises a few meters
    // across, and clumps within them.
    let meadow_patch = value_noise2(local * 0.11 + vec2<f32>(f32(f.grid.w & 1023u)));
    let clump = value_noise2(local * 1.3);
    let up = f.axis_y.xyz;
    let root = f.origin.xyz + f.axis_x.xyz * local.x + f.axis_z.xyz * local.y + up * m.x;
    let d = length(root);
    let lod = grass_lod(f, d);
    let keep = m.y * lod;
    let rank = (f32(index) + 0.5) / f32(tile.blades);
    if (rank >= keep) { return out; }
    // Shrink into the ground just before being thinned out, and at thin edges of the meadow.
    let fade = saturate((keep - rank) * 10.0);
    let height = f.shape.x * (1.0 + f.shape.y * (r0.z * 2.0 - 1.0)) * mix(0.55, 1.0, saturate(m.y))
        * mix(0.75, 1.2, meadow_patch) * mix(0.85, 1.15, clump) * fade;
    if (height < 0.005) { return out; }
    let width = f.shape.z * (0.75 + 0.5 * r1.x) * clamp(inverseSqrt(max(lod, 1e-3)), 1.0, 3.0);

    // Blade frame: facing a random direction, leaning over toward it.
    let angle = r1.y * TAU;
    let facing = f.axis_x.xyz * cos(angle) + f.axis_z.xyz * sin(angle);
    let side = cross(up, facing);
    let lean = f.shape.w * (0.2 + 0.8 * r1.z);
    var tip_dir = normalize(up + facing * lean);
    let w = grass_wind(root + tiling, t, wind, r0.w) * f.dry.w;
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
    let p0 = root;
    let p2 = root + tip_dir * height;
    let p1 = root + up * (height * (0.35 + 0.3 * max(dot(tip_dir, up), 0.0)));

    // Vertex: two per row, then the tip.
    let segments = max(tile.segments, 1u);
    var s = f32(vertex & 1u) * 2.0 - 1.0;
    var u = f32(vertex / 2u) / f32(segments);
    if (vertex >= 2u * segments) {
        u = 1.0;
        s = 0.0;
    }
    let q = 1.0 - u;
    let pos = q * q * p0 + 2.0 * q * u * p1 + u * u * p2;
    let tangent = normalize(2.0 * q * (p1 - p0) + 2.0 * u * (p2 - p1) + up * 1e-4);
    let half_width = 0.5 * width * pow(q, 0.6);
    out.rel = pos + side * (s * half_width);
    // Rounded across the blade; far away, shaded like the ground it covers.
    var n = normalize(cross(side, tangent) + side * (s * 0.45));
    n = normalize(mix(n, up, smoothstep(0.15, 1.0, d / f.lod.y) * 0.7));
    out.normal = n;
    out.along = u;

    // Color: fresh to dry by the map and per blade, tips paler and yellower.
    let dryness = saturate(m.z + (r1.w - 0.5) * 0.35 + (0.5 - meadow_patch) * 0.25);
    var albedo = mix(f.lush.rgb, f.dry.rgb, dryness) * (0.8 + 0.4 * r0.z * r1.w);
    // A slight hue drift between patches, bluer in the lush hollows.
    albedo *= mix(vec3<f32>(0.9, 1.0, 1.08), vec3<f32>(1.08, 1.02, 0.85), clump * 0.5 + meadow_patch * 0.5);
    albedo = mix(albedo, f.dry.rgb * 1.1, 0.35 * u * u);
    out.albedo = albedo;
    out.visible = true;
    return out;
}
