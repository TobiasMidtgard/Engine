// Terrain patches (engine_terrain). Requires lib_types + lib_frame.
//
// Which ground is where comes from climate (moisture, temperature, altitude, slope) and
// painted layers; each ground material (grass, forest floor, soil, rock, sand, gravel,
// concrete) is a baked texture (materials.rs) whose detail multiplies the planet palette's
// color, blended by height so stones and blades poke through their neighbors. Rock is
// projected on three axes at two scales. Weather adds snow and mud that fill the low spots
// first, wetness, puddles with rain ripples; footprints and tracks come from the
// deformation grid as normals, occlusion and, on fine patches, a small vertex offset. Open
// sea is shaded as water.

// Must match `PlanetGpu` in render.rs.
struct PlanetGpu {
    rot: array<vec4<f32>, 3>,       // columns of terrain-local -> world rotation
    center: vec4<f32>,              // terrain origin relative to the camera, base radius
    colors: array<vec4<f32>, 10>,   // lush, dry, soil, rock, sand, snow, dark, ocean, mud, concrete
    params: vec4<f32>,              // snow below °C, rock slope, vegetation (0/1), ocean surface (0/1)
    params2: vec4<f32>,             // sphere (0/1), crater density, largest crater radius, crater seed
    motion: vec4<f32>,              // how far the terrain moved in the world since last frame
    params3: vec4<f32>,             // regolith scattering (0..1), rayed craters, young areas, smallest crater radius
    rays: array<vec4<f32>, 8>,      // rayed craters: center (planet-local unit direction), radius on the ground (m)
    young: array<vec4<f32>, 12>,    // young ground: middle (planet-local direction) × youth, angular radius
    params4: vec4<f32>,             // night lights (0..1), lamp page handoff distance (m), unused ×2
};

// Must match `MapsGpu` in render.rs.
struct Maps {
    deform: vec4<f32>,   // origin x/z relative to the camera, cell size, cells per side
    surface: vec4<f32>,  // origin x/z relative to the camera, cell size, cells per side
    flags: vec4<f32>,    // deformation on, surface layers on, LOD debug colors
    far: vec4<f32>,      // wetness, puddles, snow depth, scorch beyond the surface layers' grid
    wear_origin: vec4<f32>, // worn paths window: corner relative to the camera, cell size
    wear_u: vec4<f32>,      // the window's column axis, cells per side
    wear_v: vec4<f32>,      // the window's row axis, 1 when paths are drawn
    wear_far: vec4<f32>,    // the coarse window: corner relative to the camera, cell size
};

const C_LUSH: u32 = 0u;
const C_DRY: u32 = 1u;
const C_SOIL: u32 = 2u;
const C_ROCK: u32 = 3u;
const C_SAND: u32 = 4u;
const C_SNOW: u32 = 5u;
const C_DARK: u32 = 6u;
const C_OCEAN: u32 = 7u;
const C_MUD: u32 = 8u;
const C_CONCRETE: u32 = 9u;

@group(1) @binding(0) var<storage, read> planets: array<PlanetGpu>;
@group(1) @binding(1) var<uniform> maps: Maps;
@group(1) @binding(2) var deform_tex: texture_2d<f32>;   // r: print depth (m, + down), g: grass flattened 0..1
@group(1) @binding(3) var surface_tex: texture_2d<f32>;  // wetness, puddle depth (m), snow depth (m), scorch
@group(1) @binding(4) var map_sampler: sampler;

// Must match `MaterialsGpu` in render.rs.
struct Materials {
    info: array<vec4<f32>, 18>,  // per material: (mean albedo, tile m), (mean roughness, mean occlusion, height m, 0)
    flags: vec4<f32>,            // textures ready
};
@group(1) @binding(5) var<uniform> mats: Materials;
// The night glow of the world's own lamps: one page per patch with lamps under it (lights.rs).
@group(1) @binding(10) var night_tex: texture_2d_array<f32>;
@group(1) @binding(6) var mat_albedo: texture_2d_array<f32>;  // sRGB albedo, height in alpha
@group(1) @binding(7) var mat_normal: texture_2d_array<f32>;  // normal xy, roughness, occlusion
@group(1) @binding(8) var mat_sampler: sampler;
@group(1) @binding(9) var wear_tex: texture_2d_array<f32>;  // r: path wear 0..1 (wear.rs); layer 1: share of bare path in 4× coarser cells

// Ground materials (`materials::GroundMaterial`).
const M_GRASS: u32 = 0u;
const M_FOREST: u32 = 1u;
const M_SOIL: u32 = 2u;
const M_MUD: u32 = 3u;
const M_ROCK: u32 = 4u;
const M_SNOW: u32 = 5u;
const M_SAND: u32 = 6u;
const M_GRAVEL: u32 = 7u;
const M_CONCRETE: u32 = 8u;
const MAT_COUNT: u32 = 9u;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) altitude: f32,
    @location(3) climate: vec4<f32>,   // moisture, temperature (-130..60 °C), mare, flags/255
    // Weights of the painted layers 1..10 (mud, snow, sand, dirt, grass, rock, concrete, ash,
    // gravel, leaves), regional variety, local patches.
    @location(4) paint_a: vec4<f32>,
    @location(9) paint_b: vec4<f32>,
    @location(10) paint_c: vec4<f32>,
    @location(5) origin: vec3<f32>,
    @location(6) planet: u32,
    @location(7) lod: vec4<f32>,       // vertex spacing (m), level
    @location(8) tint: vec4<f32>,      // hand-made ground tint: sqrt(color), amount
    @location(11) river: vec4<f32>,    // river and lake water (0..1), lake share, river banks, spare
    @location(12) night: vec4<f32>,    // night glow page: face corner a, b, edge, layer·8 + face (−1 none)
};

// `@invariant` keeps the prepass and the scene pass depths bit-identical.
struct VertexOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) up: vec3<f32>,
    @location(3) climate: vec4<f32>,      // moisture, temperature °C, mare, altitude (m)
    @location(4) ocean_paint: vec4<f32>,  // ocean flag, paint weight, regional variety, local patches (0..1)
    @location(5) @interpolate(flat) planet: u32,
    @location(6) lod: vec4<f32>,          // vertex spacing (m), level, convexity (-1 gully .. +1 crest)
    @location(7) tint: vec4<f32>,         // linear color, amount
    // Painted layer weights (see `VertexIn`), interpolated: painted areas meet smoothly
    // instead of in triangle steps.
    @location(8) paint_a: vec4<f32>,
    @location(9) paint_b: vec4<f32>,
    @location(10) paint_c: vec2<f32>,
    @location(11) water: vec3<f32>,       // river and lake water, lake share, river banks
    @location(12) @interpolate(flat) night: vec4<f32>,  // see `VertexIn`
};

// Geomorph offsets are stored in vertex spacings (`chunk::MORPH_RANGE`).
const MORPH_RANGE: f32 = 16.0;

fn planet_rot(p: PlanetGpu) -> mat3x3<f32> {
    return mat3x3<f32>(p.rot[0].xyz, p.rot[1].xyz, p.rot[2].xyz);
}

fn planet_up(p: PlanetGpu, rel: vec3<f32>) -> vec3<f32> {
    if (p.params2.x > 0.5) {
        return normalize(rel - p.center.xyz);
    }
    return p.rot[1].xyz;
}

fn deform_uv(rel: vec3<f32>) -> vec2<f32> {
    return (rel.xz - maps.deform.xy) / (maps.deform.z * maps.deform.w);
}

fn surface_uv(rel: vec3<f32>) -> vec2<f32> {
    let dims = vec2<f32>(textureDimensions(surface_tex));
    return (rel.xz - maps.surface.xy) / (maps.surface.z * dims);
}

// How worn the ground is at a camera-relative point, 0 untouched .. 1 a well-worn path, from
// the window of paths around the camera. Beyond it (fading over its outer cells), from the
// coarse window: there a cell's share of bare path stands in, as the wear that shows that
// much bare ground (a path is far narrower than a pixel there).
fn path_wear(rel: vec3<f32>) -> f32 {
    if (maps.wear_v.w < 0.5) {
        return 0.0;
    }
    let up = cross(maps.wear_u.xyz, maps.wear_v.xyz);
    let d = rel - maps.wear_origin.xyz;
    // (Only near the ground the paths lie on: the plane runs on under other planets.)
    if (abs(dot(d, up)) > 600.0) {
        return 0.0;
    }
    let uv = vec2<f32>(dot(d, maps.wear_u.xyz), dot(d, maps.wear_v.xyz)) / (maps.wear_origin.w * maps.wear_u.w);
    let edge = min(min(uv.x, uv.y), min(1.0 - uv.x, 1.0 - uv.y));
    let near_in = smoothstep(0.0, 0.03, edge);
    var near = 0.0;
    if (near_in > 0.0) {
        near = textureSampleLevel(wear_tex, map_sampler, uv, 0, 0.0).r;
    }
    if (near_in >= 1.0) {
        return near;
    }
    let df = rel - maps.wear_far.xyz;
    let fuv = vec2<f32>(dot(df, maps.wear_u.xyz), dot(df, maps.wear_v.xyz)) / (maps.wear_far.w * maps.wear_u.w);
    let fedge = min(min(fuv.x, fuv.y), min(1.0 - fuv.x, 1.0 - fuv.y));
    let bare = textureSampleLevel(wear_tex, map_sampler, fuv, 1, 0.0).r * smoothstep(0.0, 0.03, fedge);
    let far = select(0.0, 0.45 + 0.4 * bare, bare > 0.004);
    return mix(far, near, near_in);
}

fn inside01(uv: vec2<f32>) -> f32 {
    return select(0.0, 1.0, all(uv > vec2(0.002)) && all(uv < vec2(0.998)));
}

// Wetness, puddle depth, snow depth and scorch. The weather keeps them on a grid around the
// player; beyond it the ground is as wet and snowy as the open ground along the grid's rim
// (fading in over the grid's outer tenth), so rain and snow don't end at a square around the
// player.
fn surface_layers(rel: vec3<f32>) -> vec4<f32> {
    if (maps.flags.y < 0.5) {
        return vec4<f32>(frame.weather.x, 0.0, frame.weather.y * 0.1, 0.0);
    }
    let uv = surface_uv(rel);
    let edge = min(min(uv.x, uv.y), min(1.0 - uv.x, 1.0 - uv.y));
    let grid = textureSampleLevel(surface_tex, map_sampler, uv, 0.0);
    return mix(maps.far, grid, smoothstep(0.0, 0.1, edge));
}

// Value noise whose lattice repeats every `period` cells. Detail patterns are built on
// `tiling_pos` (world position minus a 4096 m camera tile), so with frequencies that are powers
// of two the pattern stays put when the camera crosses a tile boundary.
fn pnoise3(p: vec3<f32>, period: f32) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let i0 = i - period * floor(i / period);
    let i1 = (i0 + 1.0) - period * floor((i0 + 1.0) / period);
    let a = mix(mix(hash13(vec3(i0.x, i0.y, i0.z)), hash13(vec3(i1.x, i0.y, i0.z)), u.x),
                mix(hash13(vec3(i0.x, i1.y, i0.z)), hash13(vec3(i1.x, i1.y, i0.z)), u.x), u.y);
    let b = mix(mix(hash13(vec3(i0.x, i0.y, i1.z)), hash13(vec3(i1.x, i0.y, i1.z)), u.x),
                mix(hash13(vec3(i0.x, i1.y, i1.z)), hash13(vec3(i1.x, i1.y, i1.z)), u.x), u.y);
    return mix(a, b, u.z);
}

// `pnoise3` with its own lattice period per axis, for patterns stretched along one axis.
fn pnoise3v(p: vec3<f32>, period: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let i0 = i - period * floor(i / period);
    let i1 = (i0 + 1.0) - period * floor((i0 + 1.0) / period);
    let a = mix(mix(hash13(vec3(i0.x, i0.y, i0.z)), hash13(vec3(i1.x, i0.y, i0.z)), u.x),
                mix(hash13(vec3(i0.x, i1.y, i0.z)), hash13(vec3(i1.x, i1.y, i0.z)), u.x), u.y);
    let b = mix(mix(hash13(vec3(i0.x, i0.y, i1.z)), hash13(vec3(i1.x, i0.y, i1.z)), u.x),
                mix(hash13(vec3(i0.x, i1.y, i1.z)), hash13(vec3(i1.x, i1.y, i1.z)), u.x), u.y);
    return mix(a, b, u.z);
}

// Wind-packed snow: low drifts and ridges stretched along the prevailing east-west wind,
// 16 m long and 4 m across down to a quarter of that. Height in meters (about ±10 cm).
// Slope of churned ground (sand and snow trampled by many feet): overlapping hollows and
// lumps a hand or two across, as a horizontal tilt to subtract from a normal.
fn churn_tilt(tp: vec3<f32>) -> vec3<f32> {
    let e = 0.05;
    let h = tile_fbm(tp, 5.0, 2);
    let hx = tile_fbm(tp + vec3<f32>(e, 0.0, 0.0), 5.0, 2);
    let hz = tile_fbm(tp + vec3<f32>(0.0, 0.0, e), 5.0, 2);
    return vec3<f32>(hx - h, 0.0, hz - h) / e * 0.04;
}

fn snow_drift(tp: vec3<f32>) -> f32 {
    var sum = 0.0;
    var amp = 0.06;
    var f = vec2<f32>(1.0 / 16.0, 1.0 / 4.0);
    for (var i = 0; i < 3; i++) {
        let q = vec3<f32>(tp.x * f.x, f32(i) * 17.0, tp.z * f.y);
        sum += amp * (pnoise3v(q, vec3<f32>(4096.0 * f.x, 4096.0, 4096.0 * f.y)) * 2.0 - 1.0);
        f *= 2.0;
        amp *= 0.5;
    }
    return sum;
}

// Cliff faces eroded into ribs and runnels along the fall line, 8 m and 32 m across and 16
// times as long (world y is up near the camera): the slope (world xyz) to tilt a rock face's
// normal by, so cliffs read as fluted, broken faces, not smooth clay.
fn rock_ribs(tp: vec3<f32>, pixel: f32) -> vec3<f32> {
    var g = vec3<f32>(0.0);
    var f = 1.0 / 8.0;
    var amp = 0.6;
    for (var o = 0; o < 2; o++) {
        let fade = 1.0 - smoothstep(0.15 / f, 0.4 / f, pixel);
        if (fade > 0.0) {
            let per = vec3<f32>(4096.0 * f, 256.0 * f, 4096.0 * f);
            let q = vec3<f32>(tp.x * f, tp.y * f / 16.0 + f32(o) * 5.0, tp.z * f);
            let e = 0.1;
            let h0 = pnoise3v(q, per);
            let hx = pnoise3v(q + vec3<f32>(e, 0.0, 0.0), per);
            let hz = pnoise3v(q + vec3<f32>(0.0, 0.0, e), per);
            g += vec3<f32>(hx - h0, 0.0, hz - h0) / e * amp * fade;
        }
        f *= 0.25;
        amp *= 1.6;
    }
    return g;
}

struct Crowns {
    cover: f32,        // the pixel's share of crown
    slope: vec2<f32>,  // the crown's slope there (x, z)
    tone: f32,         // per tree, 0..1
    shadow: f32,       // shade cast by the trees between it and the sun, 0..1
    crevice: f32,      // the dark hollows between the crown's leaf clumps, 0..1
};

// Tree crowns seen from above, for woods seen from a few dozen meters out (the foliage module
// draws the trees near the camera): one tree per 8 m cell, set off at random, with a crown of
// random size and height, standing in groups. A tree stands with probability `density`, so
// woods thin out tree by tree at their edges and close their canopy inside. Each crown casts
// a shadow away from the sun (`to_sun`, world) onto the gaps and crowns beside it.
fn crowns(tp: vec3<f32>, density: f32, conifer: f32, pixel: f32, to_sun: vec3<f32>) -> Crowns {
    let cell = 8.0;
    let p = tp.xz / cell;
    let base = floor(p);
    // Shadows fall a crown's height (about half a cell above the gaps) away from the sun.
    let sun_xz = to_sun.xz / max(length(to_sun.xz), 1e-4);
    let reach = clamp(0.5 * length(to_sun.xz) / max(to_sun.y, 0.05), 0.0, 1.0);
    let sun_up = smoothstep(0.0, 0.15, to_sun.y);
    var top = -1.0;
    var cr: Crowns;
    cr.slope = vec2<f32>(0.0);
    cr.tone = 0.5;
    var shade: array<f32, 9>;
    var win = 0;
    var n = 0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            shade[n] = 0.0;
            // Cells wrap with the 4096 m tile, so the trees stay put when the camera crosses one.
            let c = base + vec2<f32>(f32(i), f32(j));
            let k = c - floor(c / 512.0) * 512.0;
            let h = vec3<f32>(hash13(vec3<f32>(k, 3.1)), hash13(vec3<f32>(k, 7.7)), hash13(vec3<f32>(k, 12.9)));
            // Groups of trees a few dozen meters across, clearings between them.
            let group = pnoise3(vec3<f32>(k.x * 0.25, 5.0, k.y * 0.25), 128.0) - 0.5;
            if (h.z < density + group * 0.5 * density * (1.0 - density) * 4.0) {
                // Young trees to broad old ones (a few meters to over 11 m across).
                let size = hash13(vec3<f32>(k, 19.3));
                let tone = hash13(vec3<f32>(k, 29.5));
                // Spruces (most trees in the cold): narrow, pointed cones with a star of
                // branches, casting long shadows.
                let spruce = tone < spruce_share(conifer);
                let r = mix(0.22, 0.74, size * size * (3.0 - 2.0 * size)) * mix(0.8, 1.0, density) * select(1.0, 0.6, spruce);
                let center = c + 0.1 + h.xy * 0.8;
                let o = (p - center) / r;
                // Lumpy outlines: a few big boughs bulge out of each crown.
                let ang = atan2(o.y, o.x);
                let lobes = select(1.0 + 0.16 * sin(ang * 5.0 + h.x * 6.28) + 0.09 * sin(ang * 3.0 + h.y * 6.28),
                    1.0 + 0.2 * abs(sin(ang * 4.0 + h.x * 6.28)) - 0.1, spruce);
                let d2 = dot(o, o) / (lobes * lobes);
                // Taller trees hide the edges of the crowns next to them.
                let rise = mix(0.8, 1.0, hash13(vec3<f32>(k, 23.9)));
                let t = select(1.0 - d2, 1.0 - sqrt(d2), spruce) * rise;
                if (t > top) {
                    top = t;
                    win = n;
                    cr.slope = select(o / sqrt(max(1.0 - d2, 0.09)), o * 1.4 / max(sqrt(d2), 0.15), spruce);
                    cr.tone = tone;
                }
                let s = (p - (center - sun_xz * reach * select(1.0, 2.2, spruce))) / r;
                shade[n] = 1.0 - smoothstep(0.6, 1.0, dot(s, s));
            }
            n++;
        }
    }
    var shadow = 0.0;
    for (var m = 0; m < 9; m++) {
        if (m != win) {
            shadow = max(shadow, shade[m]);
        }
    }
    // Up close a crown is a heap of leaf clumps (about 2 m across) with dark hollows between
    // them, and its outline is lumpy.
    cr.crevice = 0.0;
    let fine = 1.0 - smoothstep(0.25, 1.0, pixel);
    if (fine > 0.0 && top > -0.4) {
        let q = tp.xz * 0.5;
        let qb = floor(q);
        var ct = -1.0;
        var cs = vec2<f32>(0.0);
        for (var j = -1; j <= 1; j++) {
            for (var i = -1; i <= 1; i++) {
                let c = qb + vec2<f32>(f32(i), f32(j));
                let k = c - floor(c / 2048.0) * 2048.0;
                let o = (q - (c + 0.15 + vec2<f32>(hash13(vec3<f32>(k, 41.1)), hash13(vec3<f32>(k, 43.7))) * 0.7)) / 0.8;
                let t = 1.0 - dot(o, o);
                if (t > ct) {
                    ct = t;
                    cs = o / sqrt(max(t, 0.09));
                }
            }
        }
        top += (ct - 0.3) * 0.35 * fine;
        cr.slope += cs * 0.5 * fine;
        cr.crevice = (1.0 - smoothstep(-0.2, 0.6, ct)) * fine;
    }
    // Anti-aliased crown edges; a crown's sunlit top stays out of its neighbors' shadow.
    let edge = pixel / (cell * 0.5);
    cr.cover = smoothstep(-edge, edge, top);
    cr.slope *= cr.cover;
    cr.shadow = shadow * sun_up * (1.0 - smoothstep(0.3, 0.9, top));
    return cr;
}

// Share of crowns that are spruces, for a woods' needle share (`conifer`).
fn spruce_share(conifer: f32) -> f32 {
    return mix(0.12, 0.92, conifer);
}

// Fractal sum of `pnoise3` on tiling coordinates, starting at `freq` cycles per meter (a power
// of two), doubling per octave. Returns about 0..1.
fn tile_fbm(tp: vec3<f32>, freq: f32, octaves: i32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var f = freq;
    for (var i = 0; i < octaves; i++) {
        sum += amp * pnoise3(tp * f + f32(i) * 17.0, 4096.0 * f);
        f *= 2.0;
        amp *= 0.5;
    }
    return sum / (1.0 - 2.0 * amp);
}

// Two fractal noises (0..1) over the planet's surface, from 64 km features down to a couple of
// pixels: per-pixel detail for views from high up, where vertices are kilometers apart.
fn far_noise(p: PlanetGpu, rel: vec3<f32>, pixel: f32) -> vec2<f32> {
    let local = transpose(planet_rot(p)) * (rel - p.center.xyz);
    let km = select(local, normalize(local) * p.center.w, p.params2.x > 0.5) * 0.001;
    let pixel_km = pixel * 0.001;
    var a = 0.0;
    var b = 0.0;
    var norm = 0.0;
    var amp = 0.5;
    var f = 1.0 / 64.0;
    for (var i = 0; i < 9; i++) {
        let keep = 1.0 - smoothstep(0.25, 0.5, f * pixel_km);
        a += amp * keep * pnoise3(km * f + vec3(f32(i) * 7.1), 1.0e6);
        b += amp * keep * pnoise3(km * f + vec3(31.7 + f32(i) * 3.3), 1.0e6);
        norm += amp * keep;
        f *= 2.0;
        amp *= 0.55;
    }
    // Pixels wider than the coarsest features (along the limb of a planet seen from afar)
    // keep none of them: there the noise settles to its middle rather than to zero.
    return mix(vec2<f32>(0.5), vec2<f32>(a, b) / max(norm, 1e-4), saturate(norm * 4.0));
}

// How young the ground in planet-local direction `up` is (0 old .. 1 fresh lava): matches
// `Landforms::youth` in landforms.rs.
fn youth_at(p: PlanetGpu, up: vec3<f32>) -> f32 {
    var y = 0.0;
    let count = min(u32(p.params3.z), 12u);
    for (var i = 0u; i < count; i++) {
        let a = p.young[i];
        let youth = length(a.xyz);
        let d = dot(up, a.xyz / youth);
        if (d > cos(a.w)) {
            y = max(y, youth * (1.0 - smoothstep(0.75, 1.0, acos(min(d, 1.0)) / a.w)));
        }
    }
    return y;
}

// Must match `noise::cell_random32` in noise.rs.
fn crater_hash(c: vec3<i32>, seed: u32, salt: u32) -> f32 {
    var h = (bitcast<u32>(c.x) * 0x8da6b343u) ^ (bitcast<u32>(c.y) * 0xd8163841u)
        ^ (bitcast<u32>(c.z) * 0xcb1ab31fu) ^ seed ^ (salt * 0x9e3779b9u);
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return f32(h >> 8u) / 16777216.0;
}

// ---- Night lights: towns and cities on inhabited worlds glow on the night side, seen from
// a few kilometers off up to orbit (there are no buildings to see closer).
const SODIUM: vec3<f32> = vec3<f32>(1.0, 0.5, 0.16);
const LED: vec3<f32> = vec3<f32>(0.95, 0.88, 0.8);

// Smooth value noise in 0..1 with features `cell` meters wide.
fn light_noise(p: vec3<f32>, cell: f32, salt: u32) -> f32 {
    let q = p / cell;
    let i = vec3<i32>(floor(q));
    var f = fract(q);
    f = f * f * (3.0 - 2.0 * f);
    let a = mix(crater_hash(i, 0x11u, salt), crater_hash(i + vec3<i32>(1, 0, 0), 0x11u, salt), f.x);
    let b = mix(crater_hash(i + vec3<i32>(0, 1, 0), 0x11u, salt), crater_hash(i + vec3<i32>(1, 1, 0), 0x11u, salt), f.x);
    let c = mix(crater_hash(i + vec3<i32>(0, 0, 1), 0x11u, salt), crater_hash(i + vec3<i32>(1, 0, 1), 0x11u, salt), f.x);
    let d = mix(crater_hash(i + vec3<i32>(0, 1, 1), 0x11u, salt), crater_hash(i + vec3<i32>(1, 1, 1), 0x11u, salt), f.x);
    return mix(mix(a, b, f.y), mix(c, d, f.y), f.z);
}

// Lamps scattered `spacing` apart (a few cells left dark), each blob at least most of a
// pixel wide: 1 on average.
fn lamp_field(uv: vec2<f32>, spacing: f32, pixel: f32, salt: u32) -> f32 {
    let lw = max(spacing * 0.08, pixel * 0.6);
    let base = floor(uv / spacing - 0.5);
    var sum = 0.0;
    for (var k = 0; k < 4; k++) {
        let cell = base + vec2<f32>(f32(k & 1), f32(k >> 1));
        let ci = vec3<i32>(i32(cell.x), i32(cell.y), i32(salt));
        if (crater_hash(ci, 0x31u, 1u) < 0.4) {
            continue;
        }
        let at = (cell + 0.1 + 0.8 * vec2<f32>(crater_hash(ci, 0x31u, 2u), crater_hash(ci, 0x31u, 3u))) * spacing;
        let d = uv - at;
        sum += exp(-dot(d, d) / (lw * lw)) * (0.5 + crater_hash(ci, 0x31u, 4u));
    }
    // (Each blob holds π·lw² of light; 60 % of the cells have one.)
    return sum * spacing * spacing / (3.1416 * lw * lw * 0.6);
}

// A settlement's light seen from close enough to tell its parts apart (1 on average, so it
// changes nothing from far off): winding main roads lit brighter, and the lamps of the
// streets between them, in clusters from far and one by one up close.
fn street_lights(local: vec3<f32>, dir: vec3<f32>, turn: f32, pixel: f32) -> f32 {
    let wide = 1.0 - smoothstep(80.0, 400.0, pixel);
    if (wide <= 0.0) {
        return 1.0;
    }
    let e1 = normalize(cross(dir, select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(dir.y) > 0.9)));
    let e2 = cross(dir, e1);
    let a = turn * 6.2832;
    let uv = vec2<f32>(dot(local, e1) * cos(a) + dot(local, e2) * sin(a), -dot(local, e1) * sin(a) + dot(local, e2) * cos(a));
    // Main roads: where a smooth noise crosses its middle value, so they wind and fork.
    let ridge = 1.0 - abs(light_noise(vec3<f32>(uv, 0.0), 2800.0, 63u) * 2.0 - 1.0);
    let mw = max(10.0, pixel * 0.7);
    let k = 2.0 * mw / 2800.0;
    // (Broken into stretches, so they don't close into rings.)
    let roads = smoothstep(1.0 - k, 1.0, ridge) / (k * 1.2) * smoothstep(0.3, 0.5, light_noise(vec3<f32>(uv, 0.0), 1800.0, 64u)) * 1.6;
    let clusters = lamp_field(uv, 160.0, pixel, 6u);
    let near = 1.0 - smoothstep(12.0, 50.0, pixel);
    var lamps = clusters;
    if (near > 0.0) {
        lamps = mix(clusters, clusters * lamp_field(uv, 28.0, pixel, 7u), near);
    }
    return mix(1.0, roads * 0.3 + lamps * 0.7, wide);
}

// The light (radiance, cd/m²) of one size of settlement: places in cells `cell_m` wide,
// `odds` of a cell holding one, `r_min`..`r_max` across (most small, few large), `bright`
// in a large one's middle, `spokes` lit roads out of the bigger ones. `.a` is how near the
// place is, for the roads between them.
fn settlements(p: vec3<f32>, radius: f32, cell_m: f32, salt: u32, odds: f32, r_min: f32, r_max: f32, bright: f32, spokes: u32, pixel: f32) -> vec4<f32> {
    var light = vec3<f32>(0.0);
    var near = 0.0;
    let base = vec3<i32>(floor(p / cell_m - 0.5));
    for (var k = 0u; k < 8u; k++) {
        let c = base + vec3<i32>(i32(k & 1u), i32((k >> 1u) & 1u), i32((k >> 2u) & 1u));
        if (crater_hash(c, 0x5eu, salt) > odds) {
            continue;
        }
        let jitter = vec3<f32>(crater_hash(c, 0x5eu, salt + 1u), crater_hash(c, 0x5eu, salt + 2u), crater_hash(c, 0x5eu, salt + 3u));
        let center = normalize((vec3<f32>(c) + 0.15 + 0.7 * jitter) * cell_m) * radius;
        let size = pow(crater_hash(c, 0x5eu, salt + 4u), 3.0);
        // A place smaller than a pixel spreads over the pixel, as bright in all (no sparkle).
        let r_own = mix(r_min, r_max, size);
        let r = max(r_own, pixel * 0.8);
        let spread = (r_own / r) * (r_own / r);
        let local = p - center;
        let d = length(local);
        near = max(near, exp(-d / (r_own * 8.0)) * (0.3 + size));
        if (d > r * select(3.0, 12.0, spokes > 0u && size > 0.15)) {
            continue;
        }
        // Sprawl with ragged edges: suburbs reach out along a few directions.
        let ragged = light_noise(local, r * 0.6, salt + 5u) * 0.65 + light_noise(local, r * 0.2, salt + 8u) * 0.35;
        let x = d / r * (1.5 - 1.0 * ragged);
        // A bright middle and suburbs trailing off, in lit districts with dark parks and
        // fields between them (which a far pixel averages).
        let quarter = light_noise(local, max(r * 0.08, 150.0), salt + 9u) * 0.7 + light_noise(local, max(r * 0.02, 60.0), salt + 10u) * 0.3;
        let districts = mix(1.0, 0.25 + 1.5 * smoothstep(0.42, 0.56, quarter), 1.0 - smoothstep(r * 0.04, r * 0.15, pixel));
        let fall = exp(-x * x * 5.0) * 1.4 + exp(-x * 2.2) * 0.55 * districts;
        let tone = mix(SODIUM, LED, crater_hash(c, 0x5eu, salt + 6u) * 0.35 + 0.3 * smoothstep(0.3, 0.0, x));
        let streets = street_lights(local, center / radius, crater_hash(c, 0x5eu, salt + 7u), pixel);
        var glow = fall * streets;
        // Big places send lit roads out into the country, straight at first.
        if (spokes > 0u && size > 0.15) {
            let up = center / radius;
            let e1 = normalize(cross(up, select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(up.y) > 0.9)));
            let e2 = cross(up, e1);
            let w = max(80.0, pixel * 0.7);
            for (var j = 0u; j < spokes; j++) {
                let a = (f32(j) + crater_hash(c, 0x5eu, salt + 20u + j) * 0.8) * 6.2832 / f32(spokes);
                let way = e1 * cos(a) + e2 * sin(a);
                let side = cross(up, way);
                let along = dot(local, way);
                // Roads bend with the land.
                let bend = (light_noise(vec3<f32>(along, f32(j) * 977.0, 0.0), r * 0.8, salt + 30u) - 0.5) * r * 0.5;
                let off = dot(local, side) - bend;
                if (along > r * 0.3) {
                    glow += exp(-off * off / (w * w)) * (80.0 / w) * exp(-along / (r * 2.5)) * 0.25;
                }
            }
        }
        light += tone * glow * bright * spread * (0.35 + 0.65 * size);
    }
    return vec4<f32>(light, near);
}

// Every town's and city's light at a planet-local point `p` (meters from the center).
// Cube faces: outward normal, `a` axis, `b` axis (cube.rs `FACES`).
var<private> FACE_N: array<vec3<f32>, 6> = array<vec3<f32>, 6>(
    vec3(1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0),
    vec3(0.0, -1.0, 0.0), vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, -1.0));
var<private> FACE_A: array<vec3<f32>, 6> = array<vec3<f32>, 6>(
    vec3(0.0, 0.0, -1.0), vec3(0.0, 0.0, 1.0), vec3(1.0, 0.0, 0.0),
    vec3(1.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0));
var<private> FACE_B: array<vec3<f32>, 6> = array<vec3<f32>, 6>(
    vec3(0.0, 1.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, -1.0),
    vec3(0.0, 0.0, 1.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 1.0, 0.0));

// The lamps' glow (cd/m², linear RGB) at the planet-local direction `d`, from the page
// `page` (see `VertexIn.night`).
fn lamp_glow(page: vec4<f32>, d: vec3<f32>) -> vec3<f32> {
    let packed = u32(page.w);
    let face = packed % 8u;
    let k = dot(d, FACE_N[face]);
    if (k <= 0.0) {
        return vec3(0.0);
    }
    // Face coordinates (the tangent warp undone), then the page's texture coordinates.
    let ab = atan(vec2<f32>(dot(d, FACE_A[face]), dot(d, FACE_B[face])) / k) * (4.0 / 3.14159265);
    let uv = (ab - page.xy) / page.z;
    let edge = 0.5 / 128.0;
    if (any(uv < vec2(-edge)) || any(uv > vec2(1.0 + edge))) {
        return vec3(0.0);
    }
    return textureSampleLevel(night_tex, map_sampler, clamp(uv, vec2(edge), vec2(1.0 - edge)), packed / 8u, 0.0).rgb;
}

fn night_lights(p: vec3<f32>, radius: f32, pixel: f32) -> vec3<f32> {
    // Where people live in numbers: regions hundreds of kilometers wide are busy or nearly
    // empty (big cities stand in both).
    let region = light_noise(p, 1200000.0, 50u) * 0.6 + light_noise(p, 350000.0, 51u) * 0.4;
    let busy = smoothstep(0.35, 0.72, region);
    // Roads: thin strings of light, the main ones between cities, the rest across busy land.
    let road = 1.0 - abs(light_noise(p, 9000.0, 40u) * 2.0 - 1.0);
    let road2 = 1.0 - abs(light_noise(p, 3000.0, 41u) * 2.0 - 1.0);
    let width = clamp(pixel / 600.0, 0.004, 0.2);
    let on_road = smoothstep(1.0 - 0.015 - width, 1.0, road) + smoothstep(1.0 - 0.02 - width, 1.0, road2) * 0.6;
    let by_road = max(smoothstep(0.9, 0.99, road), smoothstep(0.92, 0.99, road2) * 0.7);
    // (Radiance as satellites measure it: a city's middle about 2 cd/m², its suburbs and
    // small towns a fraction of that.)
    let cities = settlements(p, radius, 120000.0, 10u, 0.6, 4000.0, 25000.0, 3.0, 5u, pixel);
    var light = cities.rgb * (0.35 + 0.65 * busy);
    if (busy > 0.0) {
        let towns = settlements(p, radius, 25000.0, 20u, 0.75, 800.0, 4000.0, 1.6, 0u, pixel);
        // Villages string out along the roads.
        let villages = settlements(p, radius, 5000.0, 30u, 0.7, 250.0, 700.0, 1.0, 0u, pixel);
        let near = saturate(cities.a + towns.a * 0.6);
        light += (towns.rgb + villages.rgb * (0.15 + 0.85 * by_road)) * busy
            + SODIUM * on_road * 0.2 / (1.0 + width * 20.0) * (0.2 + 0.8 * near) * busy;
    }
    return light;
}

// Derivative of smoothstep(e0, e1, x).
fn smoothstep_d(e0: f32, e1: f32, x: f32) -> f32 {
    let t = saturate((x - e0) / (e1 - e0));
    return 6.0 * t * (1.0 - t) / (e1 - e0);
}

// Slope (meters of height per crater radius) of `crater_profile` in shape.rs at `x` crater
// radii from the center: a fresh bowl (flat-floored inside `fl`) with a sharp-crested rim,
// turning into a shallow dish with a rounded rim as it wears.
fn crater_slope(x: f32, depth: f32, rim: f32, fl: f32, worn: f32) -> f32 {
    if (x >= 1.5) {
        return 0.0;
    }
    var bowl = 0.0;
    if (x < 1.0) {
        if (x > fl) {
            bowl = depth * 2.0 * (x - fl) / ((1.0 - fl) * (1.0 - fl));
        }
        bowl += rim * smoothstep_d(0.6, 1.0, x);
    } else {
        bowl = rim * 2.0 * (-smoothstep_d(1.0, 1.5, x) * (1.5 - x) - (1.0 - smoothstep(1.0, 1.5, x)));
    }
    let e = x - 1.05;
    let dish = depth * smoothstep_d(fl * 0.8, 1.25, x) - rim * sign(e) * smoothstep_d(0.0, 0.4, abs(e));
    return mix(bowl, dish, worn);
}

// Height (meters) of `crater_profile` in shape.rs.
fn crater_height(x: f32, depth: f32, rim: f32, fl: f32, worn: f32) -> f32 {
    if (x >= 1.5) {
        return 0.0;
    }
    var bowl = rim * (1.0 - smoothstep(1.0, 1.5, x)) * (1.5 - x) * 2.0;
    if (x < 1.0) {
        let xf = max((x - fl) / (1.0 - fl), 0.0);
        bowl = depth * (xf * xf - 1.0) + rim * smoothstep(0.6, 1.0, x);
    }
    let dish = depth * (smoothstep(fl * 0.8, 1.25, x) - 1.0) + rim * (1.0 - smoothstep(0.0, 0.4, abs(x - 1.05)));
    return mix(bowl, dish, worn);
}

// How much of the sun a crater's own rim hides at `d` (meters from its center, which may lie
// off the surface: see `craters` in shape.rs), 0..1: under a low sun the side of the bowl
// toward the sun lies in the rim's shadow. Normals alone only tilt the light, so without
// this small craters stayed lit to the bottom.
fn crater_shadow(d: vec3<f32>, up: vec3<f32>, sun: vec3<f32>, rc: f32, depth: f32, rim: f32, fl: f32, worn: f32) -> f32 {
    let sin_e = dot(sun, up);
    let flat_sun = sun - up * sin_e;
    let cos_e = length(flat_sun);
    if (sin_e <= 0.0 || cos_e < 1e-4) {
        return 0.0;
    }
    let toward = flat_sun / cos_e;
    let dz = dot(d, up);
    let o = d - up * dz;
    // The rim's circle on the ground, and how far it lies toward the sun.
    let r2 = rc * rc - dz * dz;
    let oo = dot(o, o);
    if (oo >= r2) {
        return 0.0;
    }
    let u = dot(o, toward);
    let to_rim = -u + sqrt(max(r2 - (oo - u * u), 0.0));
    let crest = max(crater_height(1.0, depth, rim, fl, worn), crater_height(1.05, depth, rim, fl, worn));
    let below = crest - crater_height(length(d) / rc, depth, rim, fl, worn) - to_rim * sin_e / cos_e;
    // A soft edge: the sun is half a degree wide, and the rim is not a perfect circle.
    let soft = 0.01 * rc + 0.0045 * to_rim;
    return smoothstep(0.0, 2.0 * soft, below);
}

// The rim's shadow where a crater too small for the mesh is drawn on flat ground: what the
// near rim would hide from the eye (`crater_shadow` toward the eye) shows the far wall in a
// real bowl, mostly lit, so the shadow is left out there. Otherwise a crater seen at a low
// angle with the sun behind the viewer drew a black disc where its lit far wall should be.
fn crater_seen_shadow(d: vec3<f32>, up: vec3<f32>, sun: vec3<f32>, eye: vec3<f32>, rc: f32, depth: f32, rim: f32, fl: f32, worn: f32) -> f32 {
    let shade = crater_shadow(d, up, sun, rc, depth, rim, fl, worn);
    if (shade <= 0.0) {
        return 0.0;
    }
    return shade * (1.0 - crater_shadow(d, up, eye, rc, depth, rim, fl, worn));
}

// Small craters on barren ground near the camera (none of them in the shape): their slopes,
// the albedo gain of fresh ones and their rims' shadows. Cells of 8 m down to 1 m divide the
// 4096 m tiling period, so `tp` keeps them in place.
struct Pocks {
    grad: vec3<f32>,  // slope along the surface
    bright: f32,      // albedo gain of fresh ones
    shadow: f32,      // share of the sun their rims hide
};

fn micro_craters(tp: vec3<f32>, up: vec3<f32>, sun: vec3<f32>, eye: vec3<f32>, pixel: f32, pixel_across: f32) -> Pocks {
    var grad = vec3<f32>(0.0);
    var bright = 0.0;
    var shadow = 0.0;
    var cell = 8.0;
    for (var size_class = 0u; size_class < 4u; size_class++) {
        let radius = cell / 3.2;
        let seen = smoothstep(0.7 * pixel, 2.5 * pixel, radius);
        if (seen <= 0.0) {
            break;
        }
        let base = vec3<i32>(floor(tp / cell - 0.5));
        let cs = 0x5eedu ^ (size_class * 0x9e37u);
        for (var k = 0; k < 8; k++) {
            let c = base + vec3<i32>(k & 1, (k >> 1) & 1, (k >> 2) & 1);
            // Crowded: below a few hundred meters the Moon's craters are as many as the ground
            // holds (new ones wipe out old ones).
            if (crater_hash(c, cs, 1u) > 0.8) {
                continue;
            }
            let center = (vec3<f32>(c) + vec3<f32>(crater_hash(c, cs, 2u), crater_hash(c, cs, 3u), crater_hash(c, cs, 4u))) * cell;
            let rc = radius * (0.5 + 0.5 * crater_hash(c, cs, 5u));
            let d = tp - center;
            let dist = length(d);
            let x = dist / rc;
            if (x >= 1.5) {
                continue;
            }
            let age = crater_hash(c, cs, 6u);
            let worn = smoothstep(0.1, 0.9, age);
            let wear = 1.0 - 0.7 * worn;
            let depth = rc * 0.2 * wear;
            let rim = rc * 0.05 * wear * wear;
            let dh = crater_slope(x, depth, rim, 0.0, worn);
            // Right under the camera a crater meters across would need its bowl in the mesh to
            // read as a pit: its shading and shadow alone look like a stain on flat ground
            // there, so it fades out.
            let flat_near = 1.0 - smoothstep(150.0 * pixel_across, 400.0 * pixel_across, rc);
            grad += d * (dh / (rc * max(dist, 1e-4))) * seen * flat_near;
            shadow = max(shadow, crater_seen_shadow(d, up, sun, eye, rc, depth, rim, 0.0, worn) * seen * flat_near);
            let fresh = 1.0 - smoothstep(0.1, 0.3, age);
            bright += fresh * seen * smoothstep(0.5, 0.9, x) * (1.0 - smoothstep(1.05, 1.5, x)) * 0.25;
        }
        cell *= 0.5;
    }
    var out: Pocks;
    out.grad = grad - up * dot(grad, up);
    out.bright = bright;
    out.shadow = shadow;
    return out;
}

// Slope (x/z) of low lumps and swales in the dust, 2 m and 50 cm across, a few centimeters
// high. Lattice frequencies divide the 4096 m tiling period, so `tp` keeps them in place.
fn dust_lumps(tp: vec3<f32>) -> vec2<f32> {
    let e = 0.04;
    var g = vec2<f32>(0.0);
    var f = 0.5;
    var amp = 0.06;
    for (var o = 0; o < 2; o++) {
        let q = vec3<f32>(tp.x * f, 3.0 + f32(o) * 7.0, tp.z * f);
        let per = 4096.0 * f;
        let h0 = pnoise3(q, per);
        let hx = pnoise3(q + vec3<f32>(e * f, 0.0, 0.0), per);
        let hz = pnoise3(q + vec3<f32>(0.0, 0.0, e * f), per);
        g += vec2<f32>(hx - h0, hz - h0) / e * amp;
        f *= 4.0;
        amp *= 0.3;
    }
    return g;
}

struct Craters {
    slope: vec3<f32>, // gradient of the crater height along the surface (world)
    bright: f32,      // albedo gain of fresh walls and ejecta
    shadow: f32,      // share of the sun the rims hide
};

// The craters the shape scatters (`craters` in shape.rs, same hash and profile) that the
// vertices are too far apart to draw, as slopes for shading, and the bright walls and ejecta
// of fresh ones (only craters at least a pixel across, and 40 m or more); and the shadows the
// rims of all of them down to the smallest cast into their bowls (the shadow maps are far too
// coarse for them).
fn crater_detail(p: PlanetGpu, rel: vec3<f32>, altitude: f32, pixel: f32, pixel_across: f32, spacing: f32) -> Craters {
    var out: Craters;
    out.slope = vec3<f32>(0.0);
    out.bright = 0.0;
    out.shadow = 0.0;
    var radius = p.params2.z;
    let rot = planet_rot(p);
    let local = transpose(rot) * (rel - p.center.xyz);
    let up = normalize(local);
    // Young lava keeps a tenth of the craters (the shape decides at each crater's center).
    let density = p.params2.y * (1.0 - 0.9 * youth_at(p, up));
    let pos = up * p.center.w;
    // The point from each crater's center as a small per-pixel part (camera-relative, down to
    // the base radius) plus the camera's offset from the center, the same for every pixel:
    // subtracting two planet-sized numbers per pixel left stair steps along shadow edges.
    let near = transpose(rot) * rel - up * altitude;
    let cam = -(transpose(rot) * p.center.xyz);
    let seed = u32(p.params2.w);
    let smallest = max(pixel * 0.7, 40.0);
    let smallest_shadow = max(pixel * 0.7, p.params3.w);
    let sun = transpose(rot) * frame.sun_dir.xyz;
    let eye = transpose(rot) * normalize(-rel);
    var grad = vec3<f32>(0.0);
    var size_class = 0u;
    loop {
        if (radius < smallest_shadow || size_class > 20u) {
            break;
        }
        let detail = radius >= smallest;
        // Vertices draw craters a few spacings across; pixels show ones a couple of pixels across.
        let shade = 1.0 - smoothstep(1.5 * spacing, 6.0 * spacing, radius);
        let seen = smoothstep(0.7 * pixel, 2.0 * pixel, radius);
        let cell = radius * 3.2;
        let base = vec3<i32>(floor(pos / cell - 0.5));
        let cs = seed ^ (size_class * 0x9e37u);
        for (var k = 0; k < 8; k++) {
            let c = base + vec3<i32>(k & 1, (k >> 1) & 1, (k >> 2) & 1);
            if (crater_hash(c, cs, 1u) > density) {
                continue;
            }
            let center = (vec3<f32>(c) + vec3<f32>(crater_hash(c, cs, 2u), crater_hash(c, cs, 3u), crater_hash(c, cs, 4u))) * cell;
            let rc = radius * (0.55 + 0.45 * crater_hash(c, cs, 5u));
            let d = near + (cam - center);
            let dist = length(d);
            let x = dist / rc;
            if (x >= 1.5) {
                continue;
            }
            let big = rc > 8000.0;
            let age = crater_hash(c, cs, 6u);
            let worn = smoothstep(0.1, 0.9, age);
            let wear = 1.0 - 0.65 * worn;
            let depth = rc * select(0.18, 0.06, big) * wear;
            let rim = rc * 0.045 * wear * wear;
            // (Large on the screen, the mesh carries the bowl and the shadow maps its shadow.)
            let small = 1.0 - smoothstep(150.0 * pixel_across, 400.0 * pixel_across, rc);
            out.shadow = max(out.shadow, crater_seen_shadow(d, up, sun, eye, rc, depth, rim, select(0.0, 0.55, big), worn) * seen * small);
            if (!detail) {
                continue;
            }
            let dh = crater_slope(x, depth, rim, select(0.0, 0.55, big), worn);
            grad += d * (dh / (rc * max(dist, 1e-3))) * shade;
            // Fresh craters are bright all over (new rock on the floor and walls, a blanket of
            // ejecta thinning out past the rim), so from afar they are bright spots with halos,
            // not rings.
            let fresh = 1.0 - smoothstep(0.08, 0.25, age);
            out.bright += fresh * seen * (1.0 - smoothstep(0.6, 1.5, x)) * 0.45;
        }
        radius *= 0.5;
        size_class += 1u;
    }
    out.slope = rot * (grad - up * dot(grad, up));
    return out;
}

// Rays of bright ejecta from the planet's youngest big craters (`PlanetShape::rayed_craters`):
// streaks of fresh material thrown straight out along great circles, each reaching its own
// distance (6 to 40 crater radii), broken into clumps where secondary craters landed, over a
// bright blanket around the rim. Returns how much of the ground is ray material, 0..1.
fn crater_rays(p: PlanetGpu, rel: vec3<f32>, pixel: f32) -> f32 {
    let count = min(u32(p.params3.y), 8u);
    if (count == 0u) {
        return 0.0;
    }
    let dir = normalize(transpose(planet_rot(p)) * (rel - p.center.xyz));
    var ray = 0.0;
    for (var k = 0u; k < count; k++) {
        let c = p.rays[k];
        // Along the ground (2R asin(chord / 2) stays exact at small distances).
        let d = 2.0 * asin(min(length(dir - c.xyz) * 0.5, 1.0)) * p.center.w;
        let x = d / c.w;
        if (x > 40.0 || x < 0.8) {
            continue;
        }
        let t1 = normalize(cross(c.xyz, select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(c.y) > 0.9)));
        let t2 = cross(c.xyz, t1);
        let around = atan2(dot(dir, t2), dot(dir, t1)) * 0.15915494 + 0.5;
        let salt = f32(k) * 17.0;
        // Streaks narrower than a few pixels fade into their average instead of shimmering.
        let lobe = 6.2831853 * d / pixel;
        let broad = pnoise3(vec3<f32>(around * 24.0, salt, 0.5), 24.0);
        let fine = mix(0.5, pnoise3(vec3<f32>(around * 96.0, salt + 5.0, 0.5), 96.0), smoothstep(96.0, 288.0, lobe));
        let streak = smoothstep(0.4, 0.75, broad) * 0.8 + smoothstep(0.55, 0.8, fine) * 0.35;
        // The broadest streaks reach farthest.
        let reach = mix(8.0, 40.0, broad);
        let clumps = mix(0.5, pnoise3(vec3<f32>(around * 96.0, log2(x) * 6.0, salt), 96.0), smoothstep(1.0, 3.0, 0.115 * d / pixel));
        let along = (1.0 - smoothstep(0.4 * reach, reach, x)) * mix(0.3, 1.0, clumps) * pow(min(1.0, 4.0 / x), 0.6);
        let blanket = (1.0 - smoothstep(1.1, 3.0, x)) * mix(0.7, 1.0, fine);
        ray = max(ray, smoothstep(0.8, 1.1, x) * max(blanket, saturate(streak) * along));
    }
    return ray;
}

// Camera-relative position of a vertex: geomorphed toward its parent patch and lowered by
// prints. Shared by the scene pass and the prepass so both rasterize the same depth.
fn place_vertex(in: VertexIn, planet: PlanetGpu) -> vec3<f32> {
    var rel = in.origin + planet_rot(planet) * in.position;
    let up = planet_up(planet, rel);
    // Geomorph toward the parent patch's shape as it nears the distance where it takes over.
    if (in.lod.w > 0.0) {
        let k = smoothstep(in.lod.z, in.lod.w, length(rel));
        rel += up * (in.normal.w * in.lod.x * MORPH_RANGE * k);
    }
    let flags = u32(round(in.climate.w * 255.0));
    // Prints and tracks lower fine patches a little (shading carries the detail).
    if (maps.flags.x > 0.5 && in.lod.x < 1.5 && (flags & 2u) == 0u) {
        let uv = deform_uv(rel);
        let o = 0.35 * in.lod.x / (maps.deform.z * maps.deform.w);
        var d = textureSampleLevel(deform_tex, map_sampler, uv, 0.0).r * 0.4;
        d += textureSampleLevel(deform_tex, map_sampler, uv + vec2(o, 0.0), 0.0).r * 0.15;
        d += textureSampleLevel(deform_tex, map_sampler, uv - vec2(o, 0.0), 0.0).r * 0.15;
        d += textureSampleLevel(deform_tex, map_sampler, uv + vec2(0.0, o), 0.0).r * 0.15;
        d += textureSampleLevel(deform_tex, map_sampler, uv - vec2(0.0, o), 0.0).r * 0.15;
        rel -= up * d * inside01(uv);
    }
    // Worn paths sink a few centimeters into the ground.
    if (in.lod.x < 1.5 && (flags & 2u) == 0u) {
        rel -= up * smoothstep(0.2, 0.9, path_wear(rel)) * 0.035;
    }
    return rel;
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let planet = planets[in.planet];
    let rot = planet_rot(planet);
    let rel = place_vertex(in, planet);
    let up = planet_up(planet, rel);
    let flags = u32(round(in.climate.w * 255.0));
    let ocean = select(0.0, 1.0, (flags & 2u) != 0u);
    let convex = f32(flags >> 2u) / 63.0 * 2.0 - 1.0;

    var out: VertexOut;
    out.rel = rel;
    out.normal = normalize(rot * in.normal.xyz);
    out.up = up;
    out.climate = vec4<f32>(in.climate.x, in.climate.y * 190.0 - 130.0, in.climate.z, in.altitude);
    let painted = dot(in.paint_a, vec4(1.0)) + dot(in.paint_b, vec4(1.0)) + in.paint_c.x + in.paint_c.y;
    out.ocean_paint = vec4<f32>(ocean, saturate(painted), in.paint_c.zw);
    out.planet = in.planet;
    out.paint_a = in.paint_a;
    out.paint_b = in.paint_b;
    out.paint_c = in.paint_c.xy;
    out.lod = vec4<f32>(in.lod.xy, convex, 0.0);
    out.water = in.river.xyz;
    out.night = in.night;
    out.tint = vec4<f32>(in.tint.rgb * in.tint.rgb, in.tint.a);
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    return out;
}


// ---------------------------------------------------------------- depth prepass
// Depth, normals, roughness and motion for ambient occlusion, contact shadows, reflections and
// temporal anti-aliasing. The patch normal is enough for those, except where water lies still
// (open sea, puddles, flooded prints): that is flat and glossy so it mirrors the scene.

struct PrepassOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) motion_now: vec4<f32>,
    @location(2) motion_prev: vec4<f32>,
    @location(3) up: vec3<f32>,
    @location(4) ocean: f32,
    @location(5) rel: vec3<f32>,
    @location(6) river: f32,
};

@vertex
fn vs_prepass(in: VertexIn) -> PrepassOut {
    let planet = planets[in.planet];
    let rel = place_vertex(in, planet);
    let flags = u32(round(in.climate.w * 255.0));
    var out: PrepassOut;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.normal = normalize(planet_rot(planet) * in.normal.xyz);
    out.up = planet_up(planet, rel);
    out.rel = rel;
    out.motion_now = motion_clip(rel);
    out.motion_prev = prev_motion_clip(static_prev_rel(rel) - planet.motion.xyz);
    out.ocean = select(0.0, 1.0, (flags & 2u) != 0u) * planet.params.w;
    out.river = in.river.x;
    return out;
}

@fragment
fn fs_prepass(in: PrepassOut) -> PrepassOutput {
    // Rain from the surface layers (or the weather where they don't reach), standing water and
    // prints, pooled the way the scene pass pools them (without the material detail).
    let layers = surface_layers(in.rel);
    let rain_wet = saturate(layers.x);
    let duv = deform_uv(in.rel);
    let depth = textureSampleLevel(deform_tex, map_sampler, duv, 0.0).r * inside01(duv) * maps.flags.x;
    let snow = smoothstep(0.005, 0.03, layers.z);
    let pool = max(
        smoothstep(0.0005, 0.015, layers.y),
        smoothstep(0.015, 0.04, depth) * smoothstep(0.5, 0.85, rain_wet),
    );
    let river = smoothstep(0.2, 0.6, in.river) * (1.0 - in.ocean);
    let water = max(max(pool * (1.0 - snow) * (1.0 - in.ocean), in.ocean), river);
    let n = normalize(mix(normalize(in.normal), in.up, water));
    let ground = 0.85 - 0.35 * rain_wet * (1.0 - snow);
    let roughness = mix(ground, mix(0.04, 0.08, in.ocean), water);
    return prepass_output(n, roughness, in.motion_now, in.motion_prev);
}

// Bump the normal by a scalar height field `h` using screen-space derivatives
// (Mikkelsen, "Bump Mapping Unparametrized Surfaces on the GPU", 2010).
fn bump(n: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, dh1: f32, dh2: f32) -> vec3<f32> {
    let r1 = cross(dp2, n);
    let r2 = cross(n, dp1);
    let det = dot(dp1, r1);
    let grad = sign(det) * (dh1 * r1 + dh2 * r2);
    let b = abs(det) * n - grad;
    let len = length(b);
    return select(n, b / len, len > 1e-12);
}

// A hand-made ground's tint that means snow (linear).
const SNOW_TINT = vec3<f32>(0.8, 0.83, 0.88);

// Weight of each painted layer (index 0 = none) at a pixel.
fn paint_weights(in: VertexOut) -> array<f32, 11> {
    return array<f32, 11>(
        0.0, in.paint_a.x, in.paint_a.y, in.paint_a.z, in.paint_a.w,
        in.paint_b.x, in.paint_b.y, in.paint_b.z, in.paint_b.w, in.paint_c.x, in.paint_c.y,
    );
}

// Painted layer -> ground material.
fn paint_material(layer: u32) -> u32 {
    switch (layer) {
        case 1u: { return M_MUD; }
        case 2u: { return M_SNOW; }
        case 3u: { return M_SAND; }
        case 5u: { return M_GRASS; }
        case 6u: { return M_ROCK; }
        case 7u: { return M_CONCRETE; }
        case 9u: { return M_GRAVEL; }
        case 10u: { return M_FOREST; }
        default: { return M_SOIL; }
    }
}

fn paint_color(p: PlanetGpu, layer: u32) -> vec3<f32> {
    switch (layer) {
        case 1u: { return p.colors[C_MUD].rgb; }
        case 2u: { return p.colors[C_SNOW].rgb; }
        case 3u: { return p.colors[C_SAND].rgb; }
        case 4u: { return p.colors[C_SOIL].rgb * 0.85; }
        case 5u: { return p.colors[C_LUSH].rgb; }
        case 6u: { return p.colors[C_ROCK].rgb; }
        case 7u: { return p.colors[C_CONCRETE].rgb; }
        case 8u: { return vec3<f32>(0.03, 0.028, 0.027); }
        case 9u: { return mix(p.colors[C_ROCK].rgb, p.colors[C_SOIL].rgb, 0.35); }
        case 10u: { return p.colors[C_SOIL].rgb * vec3<f32>(0.8, 0.75, 0.7); }
        default: { return p.colors[C_SOIL].rgb; }
    }
}

// How porous a layer is (how much rain darkens it and how readily it turns to mud).
fn paint_porosity(layer: u32) -> f32 {
    switch (layer) {
        case 6u, 7u: { return 0.35; }
        case 9u: { return 0.5; }
        case 2u: { return 0.0; }
        default: { return 1.0; }
    }
}

// Expanding rings from raindrops on standing water: returns a height for bumping.
fn rain_ripples(p: vec2<f32>, t: f32, rate: f32) -> f32 {
    var h = 0.0;
    let cell = floor(p * 2.0);
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            var c = cell + vec2<f32>(f32(i), f32(j));
            // Wrap the cell ids so drops stay put across camera tiles (4096 m · 2 cells/m).
            c -= 8192.0 * floor(c / 8192.0);
            let r1 = hash12(c);
            let r2 = hash12(c + 17.31);
            let center = (cell + vec2<f32>(f32(i), f32(j)) + vec2(r1, r2)) * 0.5;
            let period = 1.0 / max(rate, 0.05);
            let phase = fract(t / period + hash12(c + 3.7));
            let d = length(p - center);
            let radius = phase * 0.35;
            let ring = exp(-sq((d - radius) * 40.0)) * (1.0 - phase);
            h += ring * sin((d - radius) * 90.0);
        }
    }
    return h;
}

// ---------------------------------------------------------------- ground materials

// One material's look at a pixel: albedo detail (texel / mean, multiplied into the palette
// color), height 0..1 for blending, world normal, roughness and occlusion.
struct MatSample {
    detail: vec3<f32>,
    height: f32,
    normal: vec3<f32>,
    roughness: f32,
    occlusion: f32,
};

fn mat_tile(i: u32) -> f32 {
    return mats.info[i * 2u].w;
}

// The material's averages: what it looks like once its texture is too small to see.
fn mat_mean(i: u32, n: vec3<f32>) -> MatSample {
    let b = mats.info[i * 2u + 1u];
    return MatSample(vec3<f32>(1.0), 0.5, n, b.x, b.y);
}

struct Texels {
    albedo: vec4<f32>,
    normal: vec4<f32>,
};

// Two lookups at offsets picked by a slow noise, switched where the noise steps, so the tile
// never visibly repeats (Quilez, "texture repetition", technique 3). The taller texel wins
// near a switch. `uv` is in tiles; `period` is how many tiles the 4096 m camera tile holds.
fn mat_fetch(i: u32, uv: vec2<f32>, gx: vec2<f32>, gy: vec2<f32>, period: f32) -> Texels {
    let k = pnoise3(vec3<f32>(uv * 0.25, f32(i) * 7.31), period * 0.25) * 8.0;
    let ia = floor(k);
    let f = fract(k);
    let oa = sin(vec2<f32>(3.0, 7.0) * ia);
    let ob = sin(vec2<f32>(3.0, 7.0) * (ia + 1.0));
    let a0 = textureSampleGrad(mat_albedo, mat_sampler, uv + oa, i, gx, gy);
    let b0 = textureSampleGrad(mat_albedo, mat_sampler, uv + ob, i, gx, gy);
    let a1 = textureSampleGrad(mat_normal, mat_sampler, uv + oa, i, gx, gy);
    let b1 = textureSampleGrad(mat_normal, mat_sampler, uv + ob, i, gx, gy);
    let t = smoothstep(0.2, 0.8, f + (b0.a - a0.a) * 0.8);
    return Texels(mix(a0, b0, t), mix(a1, b1, t));
}

fn unpack_tn(t: vec4<f32>) -> vec3<f32> {
    let xy = t.xy * 2.0 - 1.0;
    return vec3<f32>(xy, sqrt(saturate(1.0 - dot(xy, xy))));
}

// How much of a material's texture is still visible for a pixel covering `pixel` meters.
fn mat_fade(tile: f32, pixel: f32) -> f32 {
    return 1.0 - smoothstep(tile * 0.06, tile * 0.25, pixel);
}

fn finish_sample(i: u32, tx: Texels, n_detail: vec3<f32>, n: vec3<f32>, fade: f32) -> MatSample {
    let mean = mats.info[i * 2u].rgb;
    let avg = mats.info[i * 2u + 1u];
    var m: MatSample;
    m.detail = mix(vec3<f32>(1.0), tx.albedo.rgb / max(mean, vec3<f32>(1e-3)), fade);
    m.height = mix(0.5, tx.albedo.a, fade);
    m.normal = normalize(mix(n, n_detail, fade));
    m.roughness = mix(avg.x, tx.normal.z, fade);
    m.occlusion = mix(avg.y, tx.normal.w, fade);
    return m;
}

// Ground material projected straight down (x → world X, y → world Z), whiteout-blended
// onto the surface normal, at `scale` × its tile size.
fn sample_planar_at(i: u32, tp: vec3<f32>, n: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, pixel: f32, scale: f32) -> MatSample {
    let tile = mat_tile(i) * scale;
    let fade = mat_fade(tile, pixel) * mats.flags.x;
    if (fade <= 0.0) {
        return mat_mean(i, n);
    }
    let tx = mat_fetch(i, tp.xz / tile, dp1.xz / tile, dp2.xz / tile, 4096.0 / tile);
    let tn = unpack_tn(tx.normal);
    let nd = normalize(vec3<f32>(tn.x + n.x, n.y * tn.z, tn.y + n.z));
    return finish_sample(i, tx, nd, n, fade);
}

// Ground material at its own tile size, handing over to a copy 4× larger further out. Past
// a few dozen meters the small texture is squeezed into a pixel or two along the view and
// reads as flat color; the large copy carries on there as tufts, clods and stones, so
// meadows, fields and roads keep their grain out to a kilometer (a landscape material's
// distance blend). Up close, where the small texture is sharp, it is drawn alone.
fn sample_planar(i: u32, tp: vec3<f32>, n: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, pixel: f32) -> MatSample {
    let tile = mat_tile(i);
    let k = 1.0 - mat_fade(tile, max(length(dp1), length(dp2)));
    if (k <= 0.0) {
        return sample_planar_at(i, tp, n, dp1, dp2, pixel, 1.0);
    }
    let far = sample_planar_at(i, tp, n, dp1, dp2, pixel, 4.0);
    let near_fade = mat_fade(tile, pixel) * mats.flags.x;
    let near = sample_planar_at(i, tp, normalize(mix(n, far.normal, k)), dp1, dp2, pixel, 1.0);
    let mean_occl = max(mats.info[i * 2u + 1u].y, 0.3);
    var m = near;
    m.detail = near.detail * mix(vec3<f32>(1.0), far.detail, k);
    m.height = mix(near.height, mix(far.height, near.height, near_fade * 0.6), k);
    m.roughness = mix(near.roughness, far.roughness, k * (1.0 - near_fade));
    m.occlusion = near.occlusion * mix(1.0, far.occlusion / mean_occl, k);
    return m;
}

// Ground material projected along all three axes, weighted by the normal (Golus,
// "Normal Mapping for a Triplanar Shader", whiteout blend), at `scale` × its tile size.
fn sample_triplanar(i: u32, tp: vec3<f32>, n: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, pixel: f32, scale: f32) -> MatSample {
    let tile = mat_tile(i) * scale;
    let fade = mat_fade(tile, pixel) * mats.flags.x;
    if (fade <= 0.0) {
        return mat_mean(i, n);
    }
    var w = pow(abs(n), vec3<f32>(4.0));
    w /= (w.x + w.y + w.z);
    let sg = select(vec3<f32>(-1.0), vec3<f32>(1.0), n >= vec3<f32>(0.0));
    let period = 4096.0 / tile;
    var albedo = vec4<f32>(0.0);
    var packed = vec4<f32>(0.0);
    var nd = vec3<f32>(0.0);
    var wsum = 0.0;
    if (w.x > 0.02) {
        let flip = vec2<f32>(sg.x, 1.0);
        let tx = mat_fetch(i, tp.zy / tile * flip, dp1.zy / tile * flip, dp2.zy / tile * flip, period);
        var tn = unpack_tn(tx.normal);
        tn.x *= sg.x;
        nd += vec3<f32>(n.x * tn.z, tn.y + n.y, tn.x + n.z) * w.x;
        albedo += tx.albedo * w.x;
        packed += tx.normal * w.x;
        wsum += w.x;
    }
    if (w.y > 0.02) {
        let flip = vec2<f32>(sg.y, 1.0);
        let tx = mat_fetch(i, tp.xz / tile * flip, dp1.xz / tile * flip, dp2.xz / tile * flip, period);
        var tn = unpack_tn(tx.normal);
        tn.x *= sg.y;
        nd += vec3<f32>(tn.x + n.x, n.y * tn.z, tn.y + n.z) * w.y;
        albedo += tx.albedo * w.y;
        packed += tx.normal * w.y;
        wsum += w.y;
    }
    if (w.z > 0.02) {
        let flip = vec2<f32>(-sg.z, 1.0);
        let tx = mat_fetch(i, tp.xy / tile * flip, dp1.xy / tile * flip, dp2.xy / tile * flip, period);
        var tn = unpack_tn(tx.normal);
        tn.x *= -sg.z;
        nd += vec3<f32>(tn.x + n.x, tn.y + n.y, n.z * tn.z) * w.z;
        albedo += tx.albedo * w.z;
        packed += tx.normal * w.z;
        wsum += w.z;
    }
    let inv = 1.0 / max(wsum, 1e-4);
    return finish_sample(i, Texels(albedo * inv, packed * inv), normalize(nd), n, fade);
}

// Rock at three scales: faces and buttresses at 64× the tile for mountainsides kilometers
// away (skipped up close, where the terrain's own shape takes over), blocks at 8×, then the
// close-up texture.
fn sample_rock(tp: vec3<f32>, n: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, pixel: f32) -> MatSample {
    var huge = mat_mean(M_ROCK, n);
    if (pixel > 0.1) {
        huge = sample_triplanar(M_ROCK, tp, n, dp1, dp2, pixel, 64.0);
        let keep = smoothstep(0.1, 0.5, pixel);
        huge.normal = normalize(mix(n, huge.normal, keep));
        huge.detail = mix(vec3<f32>(1.0), huge.detail, keep);
    }
    let big = sample_triplanar(M_ROCK, tp, huge.normal, dp1, dp2, pixel, 8.0);
    let near = sample_triplanar(M_ROCK, tp, big.normal, dp1, dp2, pixel, 1.0);
    let mean_occl = max(mats.info[M_ROCK * 2u + 1u].y, 0.3);
    var m = near;
    m.detail = sqrt(huge.detail * big.detail) * near.detail;
    m.height = mix(big.height, near.height, 0.6);
    m.occlusion = big.occlusion * near.occlusion / mean_occl;
    return m;
}

fn sample_ground(i: u32, tp: vec3<f32>, n: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, pixel: f32) -> MatSample {
    if (i == M_ROCK) {
        return sample_rock(tp, n, dp1, dp2, pixel);
    }
    return sample_planar(i, tp, n, dp1, dp2, pixel);
}

// ---- Parallax relief near the eye (parallax occlusion mapping: Tatarchuk 2006; the
// research is in game-engine-research/planets-wave2.md). Pebbles, clods and grit stand out
// of the ground by their material's height, shift as the eye moves and shade each other
// under a low sun. Heights of the material's texels (albedo alpha) span this much, meters.
var<private> RELIEF_M: array<f32, 9> = array<f32, 9>(
    0.02,   // grass (the turf between blades)
    0.05,   // forest floor: litter, roots and clods
    0.025,  // soil
    0.02,   // mud
    0.0,    // rock: triplanar, drawn without
    0.015,  // snow
    0.012,  // sand ripples
    0.022,  // gravel
    0.004,  // concrete
);

// One material's height for the march: its anti-tiling pair as `mat_fetch` picks it, fixed
// per pixel (the switch noise varies over tiles, not texels).
struct ReliefLayer {
    i: u32,
    tile: f32,
    oa: vec2<f32>,
    ob: vec2<f32>,
    f: f32,
    weight: f32,
};

fn relief_layer(i: u32, tp: vec3<f32>, weight: f32) -> ReliefLayer {
    let tile = mat_tile(i);
    let uv = tp.xz / tile;
    let k = pnoise3(vec3<f32>(uv * 0.25, f32(i) * 7.31), 4096.0 / tile * 0.25) * 8.0;
    let ia = floor(k);
    return ReliefLayer(i, tile, sin(vec2<f32>(3.0, 7.0) * ia), sin(vec2<f32>(3.0, 7.0) * (ia + 1.0)), fract(k), weight);
}

fn relief_height(l: ReliefLayer, xz: vec2<f32>, gx: vec2<f32>, gy: vec2<f32>) -> f32 {
    if (l.weight <= 0.0) {
        return 0.0;
    }
    let uv = xz / l.tile;
    let a = textureSampleGrad(mat_albedo, mat_sampler, uv + l.oa, l.i, gx / l.tile, gy / l.tile).a;
    let b = textureSampleGrad(mat_albedo, mat_sampler, uv + l.ob, l.i, gx / l.tile, gy / l.tile).a;
    return mix(a, b, smoothstep(0.2, 0.8, l.f + (b - a) * 0.8)) * l.weight;
}

// Depth below the relief's top (meters) of the ground at `xz`.
fn relief_depth(a: ReliefLayer, b: ReliefLayer, amp: f32, xz: vec2<f32>, gx: vec2<f32>, gy: vec2<f32>) -> f32 {
    return (1.0 - relief_height(a, xz, gx, gy) - relief_height(b, xz, gx, gy)) * amp;
}

struct Relief {
    // Where the eye's ray meets the relief, as an offset of the tiling position (x, z).
    offset: vec2<f32>,
    // How much of the sun the relief hides there (0..1).
    shadow: f32,
};

// March the eye's ray down into the relief of the two strongest planar materials, then
// toward the sun from where it lands. `strength` fades it all out with distance.
fn relief(w: array<f32, 9>, tp: vec3<f32>, rel: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, strength: f32) -> Relief {
    var out = Relief(vec2<f32>(0.0), 0.0);
    var i0 = 99u;
    var i1 = 99u;
    for (var i = 0u; i < MAT_COUNT; i++) {
        if (RELIEF_M[i] <= 0.0 || w[i] <= 0.003) {
            continue;
        }
        if (i0 == 99u || w[i] > w[i0]) {
            i1 = i0;
            i0 = i;
        } else if (i1 == 99u || w[i] > w[i1]) {
            i1 = i;
        }
    }
    if (i0 == 99u) {
        return out;
    }
    let w0 = w[i0];
    let w1 = select(0.0, w[i1], i1 != 99u);
    let total = w0 + w1;
    let amp = (RELIEF_M[i0] * w0 + select(0.0, RELIEF_M[i1] * w1, i1 != 99u)) / total * strength;
    if (amp < 1e-4) {
        return out;
    }
    let a = relief_layer(i0, tp, w0 / total);
    let b = relief_layer(select(i0, i1, i1 != 99u), tp, w1 / total);
    let gx = dp1.xz;
    let gy = dp2.xz;
    // The materials lie flat along world X/Z, their relief along Y.
    let v = normalize(rel);
    let down = max(-v.y, 0.2);
    let along = v.xz / down;
    let steps = mix(8.0, 24.0, 1.0 - down);
    let dd = amp / steps;
    var d_prev = 0.0;
    var s_prev = relief_depth(a, b, amp, tp.xz, gx, gy);
    var d = 0.0;
    var s = s_prev;
    for (var k = 1; k <= 24; k++) {
        if (f32(k) > steps || d >= s) {
            break;
        }
        d_prev = d;
        s_prev = s;
        d = dd * f32(k);
        s = relief_depth(a, b, amp, tp.xz + along * d, gx, gy);
    }
    // Between the last step above the ground and the first below it (secant).
    let gap_prev = s_prev - d_prev;
    let gap = s - d;
    let t = saturate(gap_prev / max(gap_prev - gap, 1e-5));
    let hit = mix(d_prev, d, t);
    out.offset = along * hit;
    // Toward the sun: does the relief rise above the ray back up to the top?
    let sun = frame.sun_dir.xyz;
    if (sun.y > 0.02 && hit > amp * 0.05) {
        let climb = sun.xz / max(sun.y, 0.08);
        let p = tp.xz + out.offset;
        for (var k = 1; k <= 6; k++) {
            let dk = hit * (1.0 - f32(k) / 7.0);
            let sk = relief_depth(a, b, amp, p + climb * (hit - dk), gx, gy);
            out.shadow = max(out.shadow, saturate((dk - sk) / (0.06 * amp)));
        }
        out.shadow *= saturate(sun.y * 6.0);
    }
    return out;
}

// Cover `height` (0..1) with a layer of the given amount (0..1): low spots fill first, high
// ones last.
fn fill_low(amount: f32, height: f32) -> f32 {
    return smoothstep(height - 0.12, height + 0.12, amount * 1.5 - 0.25);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let planet = planets[in.planet];
    let up = normalize(in.up);
    let n_geo = normalize(in.normal);
    let moisture = in.climate.x;
    let temp_c = in.climate.y;
    let mare = in.climate.z;
    let altitude = in.climate.w;
    let paint_w = in.ocean_paint.y;
    let variety = in.ocean_paint.z;
    let patches = in.ocean_paint.w;
    var pw = paint_weights(in);
    let tp = tiling_pos(in.rel);
    let dist = length(in.rel);
    // Ridge outlines: where a crest hides the slope behind it, a sliver of that back slope
    // thinner than a pixel still fills whole pixels (there is no anti-aliasing). Its normal,
    // turned from the eye and often from the sun, drew a dark line along every ridge. Such
    // pixels stand for the crest around them, so they are shaded as level ground.
    let to_eye = in.rel / -max(dist, 1e-6);
    let ndv = dot(n_geo, to_eye);

    // Screen derivatives up front (uniform control flow).
    let dp1 = dpdx(in.rel);
    let dp2 = dpdy(in.rel);
    let pixel = max(length(dp1), length(dp2));
    // (The pixel's short side: how large things look on the screen, however low the view.)
    let pixel_across = min(length(dp1), length(dp2));
    // The ground textures are filtered anisotropically (up to 16:1), so seen at a glancing
    // angle they stay sharp across the view while foreshortened along it: their detail lasts
    // as long as the pixel's narrow side is small. (Procedural patterns aren't filtered and go
    // by `pixel`, the long side.)
    let tex_pixel = max(min(length(dp1), length(dp2)), pixel / 16.0);
    // Seen from high up, pixels span hundreds of meters or more and vertices kilometers: the
    // per-vertex climate then draws blocky, straight-edged woods, meadows and snowfields.
    // Noise on the planet's surface, per pixel, gives those edges natural shapes.
    let far = smoothstep(40.0, 400.0, pixel);
    var far_n = vec2<f32>(0.5);
    if (far > 0.0) {
        far_n = far_noise(planet, in.rel, pixel);
    }
    // (A sliver: seen edge-on, turned away, and a whole triangle within a pixel or two.)
    let sliver = (1.0 - smoothstep(0.03, 0.15, ndv)) * saturate((dot(up, to_eye) - ndv) * 8.0)
        * smoothstep(0.1, 0.4, pixel / max(in.lod.x, 1e-3));
    // Capped so far-off lowland at grazing angles never blends toward sea.
    let alt_width = clamp(fwidth(altitude), 1e-3, 2.0);
    let detail_n = tile_fbm(tp, 1.0, 4);
    let detail_f = pnoise3(tp * 8.0, 4096.0 * 8.0);
    let macro_v = tile_fbm(tp, 1.0 / 64.0, 3);
    let mid_v = tile_fbm(tp, 1.0 / 8.0, 3);
    let patch_v = tile_fbm(tp, 1.0 / 32.0, 3);
    let meadow_v = tile_fbm(tp, 1.0 / 512.0, 3);
    let wave_t = frame.time.x;
    let wave_h = pnoise3(vec3<f32>(tp.x * 0.25, wave_t * 0.25, tp.z * 0.25), 1024.0) * 0.6
        + pnoise3(vec3<f32>(tp.x * 2.0 + 11.0, wave_t * 0.6, tp.z * 2.0), 8192.0) * 0.15;
    let dw1 = dpdx(wave_h);
    let dw2 = dpdy(wave_h);
    var ripple_h = 0.0;
    if (frame.weather.w > 0.01) {
        ripple_h = rain_ripples(tp.xz, frame.time.x, frame.weather.w * 0.25) * 0.004;
    }
    let dr1 = dpdx(ripple_h);
    let dr2 = dpdy(ripple_h);

    // Sea: below sea level, with a smooth coastline between vertices. Seen from high up the
    // vertices are kilometers apart and the sea-level line across each triangle is straight,
    // so the coast drew the mesh's zigzag; there it wanders by up to a vertex spacing with the
    // per-pixel noise, into capes, bays and inlets.
    let coast_wander = (far_n.y - 0.5) * 1.6 * fwidth(altitude) / max(pixel, 1e-3) * in.lod.x * far;
    // Beaches, shallows and the wash follow the same wandering coast.
    let coast_alt = altitude + coast_wander;
    let ocean = planet.params.w * saturate(0.5 - coast_alt / alt_width);

    // Ground maps around the focus.
    let duv = deform_uv(in.rel);
    let in_deform = inside01(duv) * maps.flags.x * (1.0 - ocean);
    let texel = 1.0 / maps.deform.w;
    let d0 = textureSampleLevel(deform_tex, map_sampler, duv, 0.0);
    let dxp = textureSampleLevel(deform_tex, map_sampler, duv + vec2(texel, 0.0), 0.0).r;
    let dxn = textureSampleLevel(deform_tex, map_sampler, duv - vec2(texel, 0.0), 0.0).r;
    let dzp = textureSampleLevel(deform_tex, map_sampler, duv + vec2(0.0, texel), 0.0).r;
    let dzn = textureSampleLevel(deform_tex, map_sampler, duv - vec2(0.0, texel), 0.0).r;
    let layers = surface_layers(in.rel);

    // Worn paths: the wear, and its slope across the path (for the sunken bed's normal).
    let wear_raw = path_wear(in.rel) * (1.0 - ocean);
    var wear_grad = vec3<f32>(0.0);
    // How worn the ground a meter or so around is: people step off a path to pass each other,
    // so its margins are trodden too.
    var wear_near = 0.0;
    if (maps.wear_v.w > 0.5 && pixel < 1.0) {
        let r = 1.3;
        wear_near = 0.25 * (path_wear(in.rel + maps.wear_u.xyz * r) + path_wear(in.rel - maps.wear_u.xyz * r)
            + path_wear(in.rel + maps.wear_v.xyz * r) + path_wear(in.rel - maps.wear_v.xyz * r)) * (1.0 - ocean);
    }
    if (wear_raw > 0.003) {
        let e = maps.wear_origin.w * 0.75;
        let wu = maps.wear_u.xyz * e;
        let wv = maps.wear_v.xyz * e;
        wear_grad = (maps.wear_u.xyz * (path_wear(in.rel + wu) - path_wear(in.rel - wu))
            + maps.wear_v.xyz * (path_wear(in.rel + wv) - path_wear(in.rel - wv))) / (2.0 * e);
    }

    let print_fade = in_deform * (1.0 - smoothstep(maps.deform.z * 1.5, maps.deform.z * 8.0, pixel));
    let depth = d0.r * in_deform;
    let flattened = d0.g * in_deform;

    // Pattern fades: a pattern disappears once a pixel covers several of its cycles.
    let fine_fade = 1.0 - smoothstep(0.3, 3.0, pixel);
    let macro_fade = 1.0 - smoothstep(20.0, 200.0, pixel);

    // ---- Which ground is here: a palette color and a weight per material.
    let slope = 1.0 - saturate(dot(n_geo, up));
    let vegetation = planet.params.z;
    let veg = vegetation * smoothstep(-8.0, 6.0, temp_c);
    let meadow_fade = 1.0 - smoothstep(40.0, 400.0, pixel);
    // Ridge crests and gullies at the patch's vertex scale: crests are wind-blown, dry and
    // rocky; water, soil, trees and snow collect in gullies. Landforms only: fine patches
    // (craters, trenches, small bumps) keep their ground.
    let convex = in.lod.z * smoothstep(2.0, 8.0, in.lod.x);
    let crest = smoothstep(0.2, 0.7, convex);
    let gully = smoothstep(0.15, 0.6, -convex);
    let wet_bias = (macro_v - 0.5) * 0.35 * macro_fade + (meadow_v - 0.5) * 0.45 * meadow_fade
        + (variety - 0.5) * 0.5 + (patches - 0.5) * 0.5 + gully * 0.25 - crest * 0.2 + (far_n.y - 0.5) * 0.6 * far;
    var col: array<vec3<f32>, 9>;
    var w: array<f32, 9>;
    for (var i = 0u; i < MAT_COUNT; i++) {
        col[i] = vec3<f32>(0.5);
        w[i] = 0.0;
    }
    // Grass: lush to dry by moisture, with sunlit meadows and hay fields between the woods.
    var grass = mix(planet.colors[C_DRY].rgb, planet.colors[C_LUSH].rgb, smoothstep(0.2, 0.7, moisture + wet_bias));
    // (Patches come per vertex, kilometers apart from high up: the per-pixel noise breaks the
    // fields' edges there instead of drawing the mesh's squares.)
    let field = vegetation * smoothstep(0.56 - 0.06 * far, 0.64 + 0.06 * far, patches + (variety - 0.5) * 0.2 + (far_n.x - 0.5) * 0.5 * far)
        * smoothstep(4.0, 12.0, temp_c);
    grass = mix(grass, mix(planet.colors[C_DRY].rgb, planet.colors[C_LUSH].rgb, 0.45), field * 0.35);
    let clump_fade = 1.0 - smoothstep(1.0, 12.0, pixel);
    let worn = smoothstep(0.72, 0.84, patch_v + (patches - 0.5) * 0.3);
    grass = mix(grass, planet.colors[C_DRY].rgb, worn * 0.3 * smoothstep(0.1, 0.5, pixel));
    col[M_GRASS] = grass;
    // Bare ground (cold, barren, regolith) and dark maria.
    // Airless worlds from afar: maria with ragged, gradual shores, and in shades.
    let airless_far = far * (1.0 - vegetation);
    let mare_soft = saturate(mare + (far_n.y - 0.5) * 0.7 * airless_far);
    let mare_col = planet.colors[C_DARK].rgb * mix(1.0, 0.8 + 0.4 * far_n.x, airless_far);
    col[M_SOIL] = mix(planet.colors[C_SOIL].rgb, mare_col, mare_soft);
    // Woods: the forest floor up close, dark canopy from afar (the foliage module draws the
    // trees themselves).
    let forest_edge = (mid_v - 0.5) * 0.08 * clump_fade + (patch_v - 0.5) * 0.06 + (macro_v - 0.5) * 0.12 * macro_fade;
    // Seen from orbit a pixel spans many woods and fields: the woods' share of it grows
    // gradually with the wetness (a sharp threshold traced the triangles of the vertices).
    let orbit = smoothstep(1500.0, 15000.0, pixel);
    let woodland = vegetation
        * smoothstep(0.40 - 0.1 * far - 0.12 * orbit, 0.52 + 0.1 * far + 0.15 * orbit, moisture * 0.6 + (0.5 - patches) * 0.9 + (variety - 0.5) * 0.4 + forest_edge + gully * 0.12 - crest * 0.12 + (far_n.x - 0.5) * 0.9 * far * (1.0 - 0.4 * orbit))
        * smoothstep(-6.0, 4.0, temp_c)
        * (1.0 - smoothstep(1800.0, 2600.0, altitude + (variety - 0.5) * 600.0));
    // Gallery woods: trees line the banks of rivers, in a ragged strip (on into dry country,
    // where nothing else grows).
    let gallery = vegetation
        * smoothstep(0.3, 0.65, in.water.z + (patch_v - 0.5) * 0.25 + (mid_v - 0.5) * 0.15 * clump_fade)
        * smoothstep(-6.0, 4.0, temp_c)
        * (1.0 - smoothstep(1800.0, 2600.0, altitude));
    let forest = max(woodland, gallery);
    // Needle trees in the cold: up mountains and toward the poles, mixed woods in between.
    // (`ForestCover` reports the same share, so the foliage module's trees match the paint.)
    let conifer = smoothstep(11.0, 3.0, temp_c + (variety - 0.5) * 5.0 + (macro_v - 0.5) * 3.0 * macro_fade + (moisture - 0.5) * 2.0);
    // From a couple of hundred meters the canopy (the foliage module draws trees near the camera,
    // and bare leaf litter there read as a brown hole in the land). Up close: ferns and grass
    // with patches of litter between them.
    let canopy = smoothstep(15.0, 90.0, dist);
    // Until a pixel spans a few meters, the canopy shows its trees: crowns with shade between
    // them inside the woods, and meadow between the last trees at their edges.
    // Crowns painted on the ground only pass for trees seen from well above them and from
    // high enough an angle (from lower down they read as flat discs: the canopy stays plain).
    let eye_height = dot(-in.rel, up);
    let crown_k = canopy * (1.0 - smoothstep(1.5, 6.0, pixel)) * step(0.003, forest)
        * smoothstep(30.0, 80.0, eye_height) * smoothstep(0.15, 0.3, dot(to_eye, up));
    var crown = Crowns(1.0, vec2<f32>(0.0), 0.5, 0.0, 0.0);
    var woods = forest;
    if (crown_k > 0.0) {
        crown = crowns(tp, saturate(forest * 1.5 - 0.25), conifer, pixel, frame.sun_dir.xyz);
        woods = mix(forest, max(crown.cover, smoothstep(0.8, 1.0, forest)), crown_k);
    }
    let undergrowth = planet.colors[C_LUSH].rgb * vec3<f32>(0.6, 0.7, 0.55);
    let litter = mix(planet.colors[C_SOIL].rgb * vec3<f32>(0.75, 0.72, 0.65), undergrowth, 0.3);
    let forest_floor = mix(undergrowth, litter, smoothstep(0.6, 0.8, patch_v));
    // Dark needle woods and lighter broadleaf ones, and from high up stands of each.
    let needle = saturate(conifer + (far_n.y - 0.5) * 1.5 * far);
    let canopy_tone = mix(mix(vec3<f32>(0.5, 0.55, 0.7), vec3<f32>(0.56, 0.63, 0.42), far),
        mix(vec3<f32>(0.36, 0.44, 0.62), vec3<f32>(0.34, 0.45, 0.42), far), needle);
    col[M_FOREST] = mix(forest_floor, planet.colors[C_LUSH].rgb * canopy_tone, canopy);
    // Beaches and deserts. A beach is a strip some tens of meters wide above the water, its
    // width wandering along the coast: on a gentle coast it reaches only a little above the
    // sea, on a steeper one a few meters up. From afar the strip is far thinner than the
    // vertices that carry it, so it fades (it drew a white outline around every coast seen
    // from orbit).
    let nu = max(dot(n_geo, up), 0.05);
    let shore_tan = sqrt(max(1.0 - nu * nu, 0.0)) / nu;
    let beach_w = mix(40.0, 160.0, saturate((meadow_v - 0.35) * 2.5));
    let beach_top = clamp(shore_tan * beach_w, 0.8, 5.0);
    let beach_h = coast_alt + (mid_v - 0.5) * 0.5 * beach_top * clump_fade;
    let beach = vegetation * (1.0 - smoothstep(beach_top * 0.55, beach_top * 1.15, beach_h)) * step(-1.0, coast_alt) * (1.0 - 0.85 * smoothstep(30.0, 300.0, pixel));
    // Behind it, grass that salt and wind keep pale and thin.
    let dune = vegetation * planet.params.w * (1.0 - smoothstep(beach_top * 1.5, beach_top * 3.0, beach_h)) * step(0.0, coast_alt);
    let desert = vegetation * (1.0 - smoothstep(0.08, 0.22, moisture + (variety - 0.5) * 0.1 + (far_n.y - 0.5) * 0.15 * far)) * smoothstep(12.0, 22.0, temp_c);
    let sandy = saturate(beach + desert);
    col[M_SAND] = planet.colors[C_SAND].rgb;
    // Deserts are mostly stony plains and plateaus, browner and darker than the pale sand
    // seas between them.
    let reg = desert * smoothstep(0.42, 0.62, far_n.x + (variety - 0.5) * 0.3 + (macro_v - 0.5) * 0.2 * macro_fade);
    col[M_SAND] = mix(col[M_SAND], mix(planet.colors[C_SOIL].rgb * 1.6, planet.colors[C_ROCK].rgb, 0.4), reg * 0.6);
    // Rock on steep ground and bare alpine heights; scree below it. Coarse patches average
    // slopes out, so their thresholds are lower.
    // Grass and woods hold steeper ground below the tree line (rock shows on cliffs there).
    // Between grass slopes and cliffs, mountainsides are a patchwork: outcrops on spurs,
    // grass and scree in the hollows between them.
    let veg_hold = vegetation * (0.03 + 0.06 * (1.0 - smoothstep(1400.0, 2600.0, altitude + (variety - 0.5) * 600.0)));
    let outcrop = (macro_v - 0.5) * 0.4 * macro_fade + (mid_v - 0.5) * 0.12 * clump_fade
        + (far_n.y - 0.5) * 0.3 * far - convex * 0.06;
    let rock_slope = planet.params.y * (1.0 - 0.45 * smoothstep(4.0, 150.0, in.lod.x)) + veg_hold + outcrop;
    let alpine = smoothstep(2100.0, 3000.0, altitude + (variety - 0.5) * 800.0) * vegetation;
    // Sharp crests are bare rock even where their top is level, and mountain crests are stony;
    // rounded hills stay green.
    let sharp = smoothstep(0.5, 0.85, convex);
    let stony_crest = smoothstep(0.3, 0.6, convex) * smoothstep(600.0, 1400.0, altitude + (variety - 0.5) * 400.0);
    let rock = saturate(smoothstep(rock_slope - 0.08, rock_slope + 0.05, slope + (detail_n - 0.5) * 0.12 * fine_fade) + alpine * 0.8 + sharp * 0.8);
    // Weathered rock: warmer, iron-stained faces next to gray ones, water-darkened gullies.
    let stain = saturate((patch_v - 0.55) * 4.0) * macro_fade;
    let rock_col = mix(planet.colors[C_ROCK].rgb * (0.85 + 0.3 * variety), mare_col, mare_soft)
        * mix(vec3<f32>(1.0), vec3<f32>(1.08, 0.98, 0.86), stain * vegetation) * (1.0 - gully * 0.3);
    // Layered rock: bands along altitude, warped so they bend with the land, at 37 m and 10 m.
    let strata_h = altitude + (macro_v - 0.5) * 60.0 * macro_fade + (mid_v - 0.5) * 6.0 * clump_fade;
    let strata = sin(strata_h * 0.17) * 0.6 * (1.0 - smoothstep(4.0, 16.0, pixel))
        + sin(strata_h * 0.61 + 1.3) * 0.4 * (1.0 - smoothstep(1.0, 4.0, pixel));
    // Thick beds hundreds of meters apart, in canyon walls and big cliffs seen from afar.
    let beds_h = altitude + (macro_v - 0.5) * 300.0 * macro_fade;
    let beds = (sin(beds_h * 0.015) * 0.6 + sin(beds_h * 0.0048 + 2.0) * 0.4)
        * (1.0 - smoothstep(150.0, 600.0, pixel)) * (1.0 - 0.75 * vegetation);
    col[M_ROCK] = rock_col * (1.0 + strata * 0.1) * (1.0 + beds * 0.16);
    // Where water runs down cliffs it leaves streaks along the fall line, a few meters wide and
    // tens of meters long: dark varnish, and here and there a pale mineral stain. (World y is
    // up near the camera; elsewhere on a globe the streaks fade out.)
    let steep = smoothstep(0.35, 0.7, slope) * smoothstep(0.7, 0.9, abs(up.y)) * vegetation
        * (1.0 - smoothstep(8.0, 40.0, pixel));
    // Bedded cliffs: beds about nine meters thick, each its own shade of gray or buff. Each bed
    // weathers back from its top edge, so its face leans out toward the bottom (in shadow) and
    // back toward the top (lit), with a dark notch at the bedding plane; harder beds stand out
    // more.
    var bed_tilt = 0.0;
    let bed_fade = steep * (1.0 - smoothstep(1.0, 6.0, pixel));
    if (steep > 0.0) {
        let bed_h = altitude + (macro_v - 0.5) * 40.0 * macro_fade + (patch_v - 0.5) * 4.0 * clump_fade;
        // (Uneven beds, five to thirteen meters.)
        let bh = (bed_h + 7.0 * sin(bed_h * 0.083 + 1.7)) / 9.0;
        let bed = floor(bh);
        let f = bh - bed;
        let hard = hash13(vec3<f32>(bed, 3.1, 7.7));
        let tone = hash13(vec3<f32>(bed, 11.3, 1.9));
        let bed_tone = mix(vec3<f32>(0.86, 0.88, 0.92), vec3<f32>(1.12, 1.04, 0.92), tone);
        col[M_ROCK] *= mix(vec3<f32>(1.0), bed_tone, steep * (1.0 - smoothstep(4.0, 30.0, pixel)));
        let profile = smoothstep(0.0, 0.85, f) * (1.0 - smoothstep(0.92, 1.0, f)) - 0.5;
        // Some stretches of cliff are massive rock with the bedding barely showing.
        let bedded = smoothstep(0.3, 0.6, macro_v + (variety - 0.5) * 0.4);
        bed_tilt = profile * mix(0.3, 1.0, hard) * mix(0.55, 1.0, bedded) * bed_fade;
        col[M_ROCK] *= 1.0 + bed_tilt * 0.2;
    }
    if (steep > 0.0) {
        let s1 = pnoise3v(vec3<f32>(tp.x * 0.25, tp.y * 0.015625, tp.z * 0.25), vec3<f32>(1024.0, 64.0, 1024.0));
        let s2 = pnoise3v(vec3<f32>(tp.x * 0.0625, tp.y * 0.00390625 + 7.0, tp.z * 0.0625), vec3<f32>(256.0, 16.0, 256.0));
        let streak = ((s1 - 0.5) * (1.0 - smoothstep(1.0, 8.0, pixel)) + (s2 - 0.5) * 1.4) * steep;
        col[M_ROCK] *= mix(vec3<f32>(1.0), vec3<f32>(0.6, 0.58, 0.56), saturate(streak * 3.0 - 0.25))
            * mix(1.0, 1.15, saturate(-streak * 3.0 - 0.6));
    }
    let scree = (1.0 - rock) * saturate(smoothstep(rock_slope - 0.14, rock_slope - 0.04, slope) * 0.7 + alpine * 0.6 + stony_crest * 0.9);
    col[M_GRAVEL] = mix(mix(rock_col, planet.colors[C_SOIL].rgb, 0.35), mix(rock_col, col[M_SOIL], 0.6), 1.0 - vegetation);
    col[M_CONCRETE] = planet.colors[C_CONCRETE].rgb;
    col[M_MUD] = planet.colors[C_MUD].rgb;
    col[M_SNOW] = planet.colors[C_SNOW].rgb;

    // Worn, bare patches in open grassland, near the camera only: from afar they read as spots,
    // so there they are drier grass.
    let bare_near = 1.0 - smoothstep(0.1, 0.5, pixel);
    let bare_patch = worn * 0.8 * bare_near;
    w[M_GRASS] = max(veg - woods, 0.0) * (1.0 - bare_patch);
    w[M_FOREST] = min(woods, veg);
    w[M_SOIL] = 1.0 - veg + max(veg - woods, 0.0) * bare_patch;
    for (var i = 0u; i < MAT_COUNT; i++) { w[i] *= 1.0 - sandy; }
    w[M_SAND] = sandy;
    // Barren worlds: stones thrown out by impacts lie everywhere on the regolith, poking out
    // of the dust, and gather in stony patches.
    let barren = 1.0 - vegetation;
    let stony = w[M_SOIL] * barren * (0.3 + smoothstep(0.55, 0.8, patch_v + (detail_n - 0.5) * 0.3 * fine_fade) * 0.4);
    w[M_SOIL] -= stony;
    w[M_GRAVEL] += stony;
    for (var i = 0u; i < MAT_COUNT; i++) { w[i] *= 1.0 - scree; }
    w[M_GRAVEL] += scree;
    for (var i = 0u; i < MAT_COUNT; i++) { w[i] *= 1.0 - rock; }
    w[M_ROCK] += rock;
    // Painted ground, each layer by its weight (snow and mud paint are overlays, below).
    let paint_base = saturate(paint_w - pw[1] - pw[2]);
    for (var i = 0u; i < MAT_COUNT; i++) { w[i] *= 1.0 - paint_base; }
    for (var l = 3u; l < 11u; l++) {
        let p = pw[l];
        if (p > 0.0) {
            let m = paint_material(l);
            // The layer's color for its share of the material.
            col[m] = mix(col[m], paint_color(planet, l), p / max(w[m] + p, 1e-4));
            w[m] += p;
        }
    }
    // Hand-made ground's tint (fields, roads): its color with each material's texture detail.
    // Snow and mud lie on top of the ground and keep their own colors. A tint as white as snow
    // is the level's snow: it settles as snow below (per pixel), and the ground it leaves bare
    // keeps its own color instead of turning white in the shape of the vertex grid.
    // (Far vertices carry the mean over their cell, so a tint partly snow is part white: that
    // share is snow, and the rest of the color is the ground's.)
    let tint_white = smoothstep(0.22, 0.75, min(in.tint.r, min(in.tint.g, in.tint.b)));
    let tint_ground = clamp((in.tint.rgb - SNOW_TINT * tint_white) / max(1.0 - tint_white, 0.05), vec3(0.0), vec3(1.0));
    let tint_a = in.tint.a * (1.0 - tint_white);
    if (tint_a > 0.001) {
        for (var i = 0u; i < MAT_COUNT; i++) {
            if (i != M_SNOW && i != M_MUD) { col[i] = mix(col[i], tint_ground, tint_a); }
        }
    }
    // ---- Worn paths. The edge of a path is ragged (turf holds on in tufts, bare earth bites
    // into it), and a well-used one has a bare middle and a trodden margin. Grass and leaf
    // litter are trodden flat and pale, then thin to packed earth; gravel is pushed aside
    // from the middle; sand is churned and darker.
    var wear = 0.0;
    var path_bare = 0.0;
    if (wear_raw > 0.003 || wear_near > 0.003) {
        // (Tufts a hand or two across, clumps of a meter.)
        let tufts = (tile_fbm(tp, 4.0, 2) - 0.5) * (1.0 - smoothstep(0.01, 0.12, pixel));
        let ragged = (detail_n - 0.5) * 0.5 * fine_fade + tufts * 0.55;
        wear = saturate(wear_raw + ragged * smoothstep(0.02, 0.25, wear_raw));
        let trod = max(smoothstep(0.04, 0.4, wear), smoothstep(0.05, 0.5, wear_near + tufts * 0.3) * 0.7);
        path_bare = smoothstep(0.45, 0.85, wear);
        // Turf gives way to earth; what is left of it is flattened, yellowed straw.
        let turf_lost = (w[M_GRASS] + w[M_FOREST]) * path_bare;
        w[M_GRASS] *= 1.0 - path_bare;
        w[M_FOREST] *= 1.0 - path_bare;
        w[M_SOIL] += turf_lost;
        col[M_GRASS] = mix(col[M_GRASS], mix(planet.colors[C_DRY].rgb, planet.colors[C_SOIL].rgb, 0.35) * 1.05, trod * 0.55);
        col[M_FOREST] = mix(col[M_FOREST], litter * 0.9, trod * 0.6 * (1.0 - canopy));
        // Gravel shoved aside: the middle of the track is finer and earthier.
        let shoved = w[M_GRAVEL] * path_bare * 0.55;
        w[M_GRAVEL] -= shoved;
        w[M_SOIL] += shoved;
        // Beaten earth: dusty and paler than the soil of bare patches (drier, and ground fine),
        // darker and firmer down the middle where the feet fall most.
        let soil_gray = vec3<f32>(dot(col[M_SOIL], vec3<f32>(0.3, 0.5, 0.2)));
        let dusty = mix(col[M_SOIL], soil_gray, 0.4) * 1.1;
        col[M_SOIL] = mix(col[M_SOIL], dusty * mix(1.0, 0.82, smoothstep(0.85, 1.0, wear_raw)), trod);
        col[M_SAND] *= mix(1.0, 0.88, trod);
    }
    // Grass grows in clumps whatever its color (natural, painted or tinted): straw-colored
    // tufts next to dark, dense ones, a couple of meters and several meters across, and
    // sunburnt and lush blue-green stretches tens of meters across.
    let tuft = (tile_fbm(tp, 0.5, 2) - 0.5) * 2.0 * (1.0 - smoothstep(0.05, 0.5, pixel));
    let clump = (mid_v - 0.5) * 2.0 * clump_fade + tuft * 0.5;
    let sward = (patch_v - 0.5) * 2.0 * macro_fade;
    let straw = planet.colors[C_DRY].rgb * 1.1;
    var grass_col = mix(col[M_GRASS], straw, saturate(clump) * 0.45 + saturate(sward - 0.2) * 0.25 + dune * 0.45);
    grass_col *= (1.0 - saturate(-clump) * 0.35) * mix(vec3<f32>(1.0), vec3<f32>(0.88, 0.96, 1.05), saturate(-sward) * 0.5);
    // Hand-painted pasture (its tint sets one color) still lies across the land the way grass
    // grows: lusher and darker in hollows, sun-dried on crests and knolls, in drifts tens to
    // hundreds of meters across. (Natural grass has this in its own color already.)
    let lay = ((meadow_v - 0.5) * 2.5 * meadow_fade + (macro_v - 0.5) * 1.2 * macro_fade + (crest - gully) * 0.8) * in.tint.a;
    grass_col = mix(grass_col, straw, saturate(lay) * 0.5);
    grass_col *= mix(vec3<f32>(1.0), vec3<f32>(0.7, 0.85, 0.95), saturate(-lay));
    col[M_GRASS] = grass_col;
    // Bare ground in patches too: damper, darker earth next to dry, pale stretches.
    let bare_v = 1.0 + (mid_v - 0.5) * 0.3 * clump_fade + (patch_v - 0.5) * 0.2 * macro_fade;
    col[M_SOIL] *= bare_v;
    col[M_SAND] *= bare_v;
    col[M_GRAVEL] *= bare_v;

    // ---- Parallax relief within a few meters (pixels under about 3–8 mm).
    let relief_k = (1.0 - smoothstep(0.003, 0.008, tex_pixel)) * (1.0 - far) * mats.flags.x;
    var rel_shift = Relief(vec2<f32>(0.0), 0.0);
    if (relief_k > 0.0) {
        rel_shift = relief(w, tp, in.rel, dp1, dp2, relief_k);
    }
    let tp_m = tp + vec3<f32>(rel_shift.offset.x, 0.0, rel_shift.offset.y);

    // ---- Sample the materials present and blend them by height.
    var ms: array<MatSample, 9>;
    var bh: array<f32, 9>;
    var hmax = -10.0;
    for (var i = 0u; i < MAT_COUNT; i++) {
        if (w[i] > 0.003) {
            ms[i] = sample_ground(i, tp_m, n_geo, dp1, dp2, tex_pixel);
            // Natural woods from a distance are canopy, not leaf litter: no litter texture
            // there (painted leaves are a level's forest floor and keep theirs).
            if (i == M_FOREST) {
                let c = canopy * (1.0 - saturate(pw[10] / max(w[M_FOREST], 1e-4)));
                // (A canopy is darker than the crowns' sunlit tops: shade between and inside them.)
                // Stands of trees lighter and darker over tens of meters, and once crowns are too
                // small to draw, their speckle.
                let k = crown_k * c / max(canopy, 1e-4);
                let stand = 1.0 + (patch_v - 0.5) * 0.5 * macro_fade + (mid_v - 0.5) * 0.6 * clump_fade * (1.0 - k);
                ms[i].detail = mix(ms[i].detail, vec3<f32>(0.8 * stand), c);
                ms[i].normal = normalize(mix(ms[i].normal, n_geo, c));
                ms[i].height = mix(ms[i].height, 0.5, c);
                // Crowns: sunlit domes, each tree its own shade of green, over dark gaps.
                if (k > 0.0) {
                    let dome = normalize(n_geo + vec3<f32>(crown.slope.x, 0.0, crown.slope.y) * 0.5);
                    // Broadleaf greens, dark spruces (most of the trees in the cold) and now and
                    // then a pale birch.
                    let t = crown.tone;
                    let tree = select(select(mix(vec3<f32>(0.8, 0.86, 0.9), vec3<f32>(1.05, 1.03, 0.9), saturate((t - 0.2) / 0.65)),
                        vec3<f32>(1.2, 1.15, 0.8), t > mix(0.85, 0.97, conifer)), vec3<f32>(0.6, 0.72, 0.8), t < spruce_share(conifer));
                    let lit = mix(vec3<f32>(0.3), tree * 0.7 * (1.0 - crown.crevice * 0.5), crown.cover) * (1.0 - crown.shadow * 0.55);
                    ms[i].normal = normalize(mix(ms[i].normal, dome, k));
                    ms[i].detail = mix(ms[i].detail, lit, k);
                    ms[i].occlusion *= mix(1.0, mix(0.6, 1.0 - crown.crevice * 0.4, crown.cover), k);
                    ms[i].height = mix(ms[i].height, 0.5 + 0.5 * crown.cover, k);
                }
            }
            // Cliff faces: ribs and runnels down the fall line.
            if (i == M_ROCK && steep > 0.0) {
                let g = rock_ribs(tp, pixel) * steep;
                let nr = ms[i].normal;
                ms[i].normal = normalize(nr - (g - nr * dot(g, nr)) * 0.5);
                // Each bed's face leans back toward its top.
                ms[i].normal = normalize(ms[i].normal + up * bed_tilt * 0.9);
                ms[i].occlusion *= 1.0 - max(-bed_tilt, 0.0) * 0.5;
            }
            // Barren worlds' bare ground is powdery regolith, not the cracked earth of the
            // soil texture: most of its relief and pattern is smoothed away (small craters
            // stand in for it).
            // Beaten earth is smooth and fine: the cracks and clods of the soil texture are
            // trodden out of it.
            if (i == M_SOIL && path_bare > 0.0) {
                let k = 0.7 * path_bare;
                ms[i].normal = normalize(mix(ms[i].normal, n_geo, k));
                ms[i].detail = mix(ms[i].detail, vec3<f32>(0.9 + 0.2 * detail_f), k);
                ms[i].height = mix(ms[i].height, 0.35, k);
            }
            if (i == M_SOIL && barren > 0.0) {
                let k = 0.9 * barren;
                ms[i].normal = normalize(mix(ms[i].normal, n_geo, k));
                ms[i].detail = mix(ms[i].detail, vec3<f32>(1.0), k);
                ms[i].height = mix(ms[i].height, 0.5, k);
            }
            // Plants creep over the edges of roads, plazas and beds and bare ground bites
            // into the turf: where growing and bare ground meet, the border wanders instead
            // of following the paint.
            let creep = select(-1.0, 1.0, i == M_GRASS || i == M_FOREST);
            bh[i] = ms[i].height + w[i] + creep * (detail_n - 0.5) * 1.2 * fine_fade;
            hmax = max(hmax, bh[i]);
        }
    }
    var albedo = vec3<f32>(0.0);
    var nsum = vec3<f32>(0.0);
    var roughness = 0.0;
    var occlusion = 0.0;
    var ground_h = 0.0;
    var bsum = 0.0;
    for (var i = 0u; i < MAT_COUNT; i++) {
        if (w[i] > 0.003) {
            // From afar a pixel covers a mix of grounds in proportion to their weights.
            let b = mix(max(bh[i] - hmax + 0.3, 0.0), w[i], far);
            albedo += col[i] * ms[i].detail * b;
            nsum += ms[i].normal * b;
            roughness += ms[i].roughness * b;
            occlusion += ms[i].occlusion * b;
            ground_h += ms[i].height * b;
            bsum += b;
        }
    }
    let inv_b = 1.0 / max(bsum, 1e-4);
    albedo *= inv_b;
    // Pebbles and clods shade the hollows beside them; the light is all one input, so the
    // sun's share comes off the albedo (as for footprints).
    albedo *= 1.0 - rel_shift.shadow * 0.45;
    occlusion *= 1.0 - rel_shift.shadow * 0.1;
    roughness *= inv_b;
    occlusion *= inv_b;
    ground_h *= inv_b;
    var n = normalize(select(n_geo, nsum, bsum > 1e-4));
    // Worn paths: beaten smooth, sunk a few centimeters below the ground beside them (deeper
    // in soft sand), rock polished by the feet.
    if (wear_raw > 0.003) {
        let soft = saturate(w[M_SOIL] + w[M_GRASS] + w[M_FOREST] + w[M_MUD]);
        // (The turf beside a path stands above it by its own thickness.)
        let sink = 0.07 * soft + 0.07 * saturate(w[M_SAND]) + 0.025 * saturate(w[M_GRAVEL]);
        n = normalize(mix(n, n_geo, path_bare * 0.55 * soft));
        let g = wear_grad - up * dot(wear_grad, up);
        n = normalize(n + g * sink);
        // Sand trampled into lumps and hollows.
        let churned = smoothstep(0.1, 0.5, wear) * saturate(w[M_SAND]) * (1.0 - smoothstep(0.05, 0.3, pixel));
        if (churned > 0.0) {
            n = normalize(n - churn_tilt(tp) * churned);
        }
        let polish = smoothstep(0.3, 0.9, wear) * saturate(w[M_ROCK] + w[M_CONCRETE]);
        roughness = mix(roughness, roughness * 0.6, polish);
        albedo *= 1.0 + 0.06 * polish;
        ground_h = mix(ground_h, ground_h * 0.4, path_bare * soft);
        // The turf's edge shades the margin of the path.
        let lip = smoothstep(0.35, 0.6, wear) * (1.0 - smoothstep(0.65, 0.95, wear)) * soft;
        occlusion *= 1.0 - 0.25 * lip;
    }
    // Large-scale variation beyond the textures' reach.
    albedo *= mix(1.0, 0.88 + 0.24 * macro_v, macro_fade) * mix(1.0, 0.9 + 0.2 * far_n.x, far)
        * mix(1.0, 0.8 + 0.4 * far_n.y, airless_far);
    // Sun hidden by the rims of craters too small for the shadow maps.
    var crater_shade = 0.0;
    // Pockmarks near the camera: craters from 2.5 m down to 30 cm, smaller than the shape's.
    if (barren > 0.0 && planet.params2.y > 0.0 && pixel < 0.5) {
        let pock = micro_craters(tp, up, frame.sun_dir.xyz, normalize(-in.rel), pixel, pixel_across);
        // (In the dust, and among the stones strewn on it.)
        let dusty = barren * saturate(w[M_SOIL] * 2.5);
        n = normalize(n - pock.grad * dusty);
        albedo *= 1.0 + pock.bright * dusty;
        crater_shade = max(crater_shade, pock.shadow * dusty);
        // The dust is never quite flat: gentle lumps and swales a meter or two across.
        let lump_k = (1.0 - smoothstep(0.05, 0.4, pixel)) * barren * w[M_SOIL];
        if (lump_k > 0.0) {
            let g = dust_lumps(tp) * lump_k;
            let tilt = vec3<f32>(g.x, 0.0, g.y);
            n = normalize(n - (tilt - up * dot(tilt, up)));
        }
    }
    // Craters too small for the vertices, and fresh craters' bright ejecta.
    if (planet.params2.y > 0.0 && planet.params2.z > 0.0) {
        let crater = crater_detail(planet, in.rel, altitude, pixel, pixel_across, in.lod.x);
        // Past the terminator the walls of real craters sit in the shadow of the land around
        // them (which these slopes cannot cast), so they only shade where the sun is up.
        let sun_up = smoothstep(-0.02, 0.2, dot(up, frame.sun_dir.xyz));
        n = normalize(n - crater.slope * sun_up);
        albedo *= 1.0 + crater.bright;
        crater_shade = max(crater_shade, crater.shadow);
        let ray = crater_rays(planet, in.rel, pixel) * (1.0 - vegetation);
        albedo = mix(albedo, max(albedo, planet.colors[C_ROCK].rgb * 1.25), ray * 0.75);
    }
    var porosity = (1.0 - rock * 0.6) * (1.0 - paint_w);
    for (var l = 1u; l < 11u; l++) { porosity += pw[l] * paint_porosity(l); }
    var reflectance = 0.5;
    var subsurface = 0.0;

    // ---- Footprints and tracks: normals from the print depth field, darker hollows.
    let cell = maps.deform.z;
    let grad = vec2<f32>(dxp - dxn, dzp - dzn) / (2.0 * cell);
    let print_n = normalize(n + vec3<f32>(grad.x, 0.0, grad.y) * 1.4);
    n = normalize(mix(n, print_n, print_fade));
    let hollow = saturate(depth * 6.0);
    occlusion *= 1.0 - hollow * 0.35 * print_fade;
    // Under a low sun a print's near wall shades its floor, which is what makes shallow prints
    // (sand) read as hollows rather than bumps. March the depth grid toward the sun: a cell whose
    // surface rises above the sun ray hides this point. Surface has no direct-light-only input,
    // so the shadow darkens the albedo by roughly the sun's share of the light.
    let sun = frame.sun_dir.xyz;
    let sun_flat = length(sun.xz);
    if (depth > 0.002 && print_fade > 0.01 && sun.y > 0.0 && sun_flat > 1e-3) {
        let step_uv = sun.xz / sun_flat * texel;
        let rise = cell * sun.y / sun_flat;
        var print_shadow = 0.0;
        for (var k = 1; k <= 6; k++) {
            let dk = textureSampleLevel(deform_tex, map_sampler, duv + step_uv * f32(k), 0.0).r;
            print_shadow = max(print_shadow, saturate((depth - dk - rise * f32(k)) / 0.004));
        }
        albedo *= 1.0 - print_shadow * 0.6 * print_fade * saturate(sun.y * 5.0);
    }
    let rim = saturate(-depth * 20.0);
    albedo *= 1.0 + rim * 0.06;
    // Trampled grass: flattened blades read slightly darker and yellower.
    albedo = mix(albedo, albedo * vec3<f32>(0.78, 0.82, 0.62), flattened * 0.7);
    roughness = mix(roughness, 0.7, flattened * 0.5);

    // ---- Snow (cold climate, snowfall in the surface layers, painted snow). It settles on
    // ledges and fills hollows first; steep faces stay bare.
    let snow_slope = 1.0 - saturate(dot(n, up));
    let snow_line = planet.params.x;
    // From afar a pixel spans patchy, drifted and wind-scoured snow, so cover thins out
    // gradually instead of ending at a line.
    let climate_snow = smoothstep(snow_line + 2.0 + 1.5 * far, snow_line - 3.0 - 1.5 * far, temp_c + (detail_n - 0.5) * 4.0 * fine_fade + (variety - 0.5) * 3.0 + (patches - 0.5) * 2.0 + crest * 2.0 - gully * 3.0 + (far_n.x - 0.5) * 8.0 * far)
        * (1.0 - smoothstep(rock_slope - 0.02, rock_slope + 0.14, snow_slope + (detail_n - 0.5) * 0.1 * fine_fade));
    let fallen = layers.z;
    let weather_snow = smoothstep(0.004, 0.04, fallen + (detail_f - 0.5) * 0.01 * fine_fade)
        * (1.0 - smoothstep(0.55, 0.8, snow_slope));
    // Hand-made snow (snow paint, or a snow-white tint) comes per vertex, tens of meters apart
    // on far mountains: on its own it lies in straight-edged blocks across faces too steep to
    // hold it. Per pixel it settles like the climate's snow: off the steep faces, deeper in the
    // hollows, thinner on the crests, its edges ragged instead of following the triangles.
    let hand_snow = max(pw[2], tint_white * smoothstep(0.0, 0.5, in.tint.a));
    let hand_edge = ((patch_v - 0.5) * 0.6 * macro_fade + (macro_v - 0.5) * 0.5 * macro_fade
        + (mid_v - 0.5) * 0.3 * clump_fade + gully * 0.3 - crest * 0.2 + (far_n.x - 0.5) * 0.8 * far)
        * (0.25 + 3.0 * hand_snow * (1.0 - hand_snow));
    let painted_snow = smoothstep(0.3, 0.7, hand_snow + hand_edge)
        * (1.0 - smoothstep(rock_slope + 0.12, rock_slope + 0.32, snow_slope - gully * 0.12 + (detail_n - 0.5) * 0.1 * fine_fade));
    let snow_amount = saturate(max(max(climate_snow, weather_snow), painted_snow)) * (1.0 - ocean);
    var snow = 0.0;
    var glint = 0.0;
    var glint_n = up;
    if (snow_amount > 0.003) {
        // Prints and worn tracks pack the snow; a busy track is trodden to glossy ice.
        let track = smoothstep(0.08, 0.45, wear);
        // (Only the busiest tracks: most stay packed, gray snow.)
        let icy = smoothstep(0.85, 1.0, wear) * 0.6;
        snow = mix(fill_low(snow_amount, ground_h), snow_amount, far);
        // Deep prints in thin snow show the ground below.
        let snow_depth = max(fallen, painted_snow * 0.25 + climate_snow * 0.5);
        snow *= 1.0 - smoothstep(0.7, 1.0, depth / max(snow_depth, 1e-3)) * print_fade;
        // A blanket of snow softens the shapes under it: a dusting shows the ground's relief,
        // a deeper cover buries litter, grass and pebbles (rock keeps more of its shape). The
        // wind lays it in drifts.
        let bury = mix(0.6, 1.0, smoothstep(0.02, 0.12, snow_depth) * (1.0 - rock * 0.5));
        var soft = normalize(mix(n, n_geo, bury * snow));
        let drift_fade = 1.0 - smoothstep(0.5, 6.0, pixel);
        var drift = 0.0;
        if (drift_fade > 0.0) {
            drift = snow_drift(tp);
            let e = 0.25;
            let dh = vec2<f32>(snow_drift(tp + vec3<f32>(e, 0.0, 0.0)), snow_drift(tp + vec3<f32>(0.0, 0.0, e))) - drift;
            // Feet trample the drifts flat and churn the snow.
            soft = normalize(soft - vec3<f32>(dh.x, 0.0, dh.y) / e * drift_fade * (1.0 - track));
            if (track > 0.0) {
                soft = normalize(soft - churn_tilt(tp) * track * (1.0 - icy) * drift_fade * 1.5);
            }
        }
        let sm = sample_planar(M_SNOW, tp, soft, dp1, dp2, tex_pixel);
        // Prints and worn tracks pack the snow.
        let packed = max(saturate(depth * 12.0) * print_fade, track);
        // Drift crests a touch whiter, the hollows between them a touch grayer.
        var snow_col = col[M_SNOW] * sm.detail * mix(1.0, 0.82, packed) * mix(vec3(1.0), vec3(0.93, 0.96, 1.0), packed)
            * mix(1.0, 0.86 + 0.18 * far_n.x, far) * (1.0 + drift * 0.6 * drift_fade);
        // On barren worlds the ice holds the planet's dust: thin frost near the cap's edge and
        // on trough walls takes on the ground's tint, in streaks.
        if (barren > 0.0) {
            let margin = snow_line - temp_c + (macro_v - 0.5) * 10.0 * macro_fade + (far_n.x - 0.5) * 12.0 * far;
            let dusty = barren * (1.0 - smoothstep(6.0, 30.0, margin));
            snow_col *= mix(vec3<f32>(1.0), min(planet.colors[C_SOIL].rgb * 3.0, vec3<f32>(1.0)), vec3<f32>(0.55 * dusty));
        }
        albedo = mix(albedo, snow_col, snow);
        n = normalize(mix(n, sm.normal, snow));
        roughness = mix(roughness, mix(mix(sm.roughness, 0.45, packed), 0.2, icy), snow);
        albedo = mix(albedo, albedo * vec3<f32>(0.82, 0.87, 0.94), icy * snow);
        // The packed track sits below the soft snow beside it.
        let g = wear_grad - up * dot(wear_grad, up);
        n = normalize(n + g * 0.1 * snow);
        // Glints: here and there an ice crystal faces just right and flashes in the sun. In
        // cells at least a pixel across, one in sixteen holds a tilted mirror facet (lit below,
        // on top of the snow's own shading, which it leaves alone).
        if (pixel < 0.03) {
            let cell = exp2(ceil(log2(max(pixel * 1.5, 0.002))));
            let gp = floor(tp / cell);
            glint = step(0.94, hash13(gp + vec3<f32>(0.0, 71.3, 0.0))) * (1.0 - smoothstep(0.012, 0.03, pixel))
                * snow * (1.0 - packed);
            let tilt = vec3<f32>(hash13(gp + 1.7), hash13(gp + 5.3), hash13(gp + 9.1)) * 2.0 - 1.0;
            glint_n = normalize(n + tilt * 0.5);
        }
        occlusion = mix(occlusion, sm.occlusion, snow);
        subsurface = mix(subsurface, 0.35, snow);
        ground_h = mix(ground_h, 1.0, snow);
    }
    porosity *= 1.0 - snow;

    // ---- Rain: wet ground darkens and gets glossy, soil turns to mud, water pools.
    let rain_wet = layers.x;
    let wet = saturate(rain_wet) * porosity;
    // Bare soil turns to mud; grass stays grass (darker, with a sheen) unless it is trodden.
    let natural_bare = 1.0 - veg;
    let bare = natural_bare * (1.0 - paint_w) + pw[1] + pw[4] + pw[8] + pw[10];
    let trodden = saturate(depth * 25.0) * print_fade + flattened * 0.6 + path_bare;
    // Sand soaks water up and darkens far more than grass or stone, glossy where it is
    // saturated, but it drains too fast to turn to mud.
    let sand = saturate(w[M_SAND]);
    let soil_like = (1.0 - rock) * saturate(bare + trodden) * (1.0 - sand);
    let mud_amount = saturate(max(pw[1], wet * soil_like * smoothstep(0.55, 0.9, rain_wet) * 0.8)) * (1.0 - snow);
    var mud = 0.0;
    if (mud_amount > 0.003) {
        mud = fill_low(mud_amount, ground_h) * 0.9;
        let mm = sample_planar(M_MUD, tp, n, dp1, dp2, tex_pixel);
        albedo = mix(albedo, col[M_MUD] * mm.detail * mix(1.0, 0.85 + 0.3 * detail_f, fine_fade), mud);
        n = normalize(mix(n, mm.normal, mud));
        roughness = mix(roughness, mm.roughness * mix(1.0, 0.5, saturate(rain_wet)), mud);
        occlusion = mix(occlusion, mm.occlusion, mud);
        ground_h = mix(ground_h, mm.height * 0.6, mud);
    }
    // Water darkens more in the hollows, where it collects.
    let wet_here = wet * (1.0 + 0.4 * (0.5 - ground_h));
    let wet_dark = mix(mix(0.72, 0.5, soil_like), 0.55, sand * (1.0 - mud));
    albedo *= mix(1.0, wet_dark, saturate(wet_here));
    let wet_rough = mix(mix(0.55, 0.35, soil_like), 0.3, sand * (1.0 - mud));
    roughness = mix(roughness, wet_rough, saturate(wet_here) * 0.8);

    // Puddles: from the surface layers, in prints on soaked ground, and in the low spots of
    // soaked soil and mud.
    // Ragged puddle edges, only where the surface layers hold standing water.
    let puddle_level = select(-1.0, layers.y + (detail_n - 0.5) * 0.02, layers.y > 0.0005);
    let print_pool = smoothstep(0.015, 0.04, depth) * smoothstep(0.5, 0.85, rain_wet) * porosity;
    let water_line = smoothstep(0.7, 1.0, rain_wet) * 0.2 * max(soil_like * 0.5, mud) * porosity;
    let low_pool = smoothstep(water_line + 0.01, water_line - 0.04, ground_h) * step(0.01, water_line);
    let puddle = saturate(max(max(smoothstep(0.0, 0.015, puddle_level), print_pool), low_pool)) * (1.0 - snow) * (1.0 - ocean);
    let ripple_n = bump(up, dp1, dp2, dr1, dr2);
    let still = normalize(mix(up, ripple_n, saturate(frame.weather.w * 0.2) * (1.0 - smoothstep(0.02, 0.2, pixel))));
    n = normalize(mix(n, still, puddle));
    albedo = mix(albedo, albedo * 0.35, puddle);
    roughness = mix(roughness, 0.04, puddle);
    reflectance = mix(reflectance, 0.354, puddle);

    // ---- Rivers and lakes: the water of the channels and basins the shape carves and fills
    // (rivers.rs), still and glossy with ripples drifting on it. Where the vertices resolve a
    // channel its banks are sharp and a little ragged; from afar, where a river is narrower
    // than the vertices, it is a share of the pixel. Frozen in hard frost.
    let river_in = in.water.x;
    var river = 0.0;
    if (river_in > 0.002) {
        let crisp = 1.0 - smoothstep(2.0, 12.0, in.lod.x);
        let banks = smoothstep(0.35, 0.6, river_in + (detail_n - 0.5) * 0.25 * fine_fade);
        river = mix(river_in, banks, crisp) * (1.0 - ocean);
        // A strip of wet mud and gravel along the water's edge.
        let shore = smoothstep(0.03, 0.3, river_in) * (1.0 - river) * crisp * (1.0 - in.water.y / max(river_in, 1e-3));
        albedo = mix(albedo, mix(col[M_SOIL], planet.colors[C_ROCK].rgb, 0.4) * 0.7, saturate(shore) * 0.7);
        roughness = mix(roughness, 0.5, saturate(shore) * 0.5);
        let frozen = smoothstep(-2.0, -8.0, temp_c);
        // Silty, green-brown rivers; lakes clearer and bluer.
        var river_col = mix(planet.colors[C_OCEAN].rgb * vec3<f32>(0.75, 1.1, 0.85), planet.colors[C_SOIL].rgb * 0.5, 0.35);
        river_col = mix(river_col, planet.colors[C_OCEAN].rgb * vec3<f32>(0.8, 1.0, 0.95), saturate(in.water.y / max(river_in, 1e-3)));
        river_col = mix(river_col, planet.colors[C_SNOW].rgb * 0.85, frozen);
        let ripple_fade = (1.0 - smoothstep(0.5, 8.0, pixel)) * (1.0 - frozen);
        let river_n = normalize(mix(up, bump(up, dp1, dp2, dw1, dw2), ripple_fade * 0.25));
        albedo = mix(albedo, river_col, river);
        n = normalize(mix(n, river_n, river));
        roughness = mix(roughness, mix(0.06, 0.5, frozen), river);
        reflectance = mix(reflectance, 0.354, river);
        occlusion = mix(occlusion, 1.0, river);
        subsurface = mix(subsurface, 0.0, river);
    }

    // ---- Scorch marks.
    let scorch = layers.w;
    albedo = mix(albedo, vec3<f32>(0.025, 0.023, 0.022), scorch * 0.9);
    roughness = mix(roughness, 0.95, scorch);

    // ---- Open sea.
    let sea_depth = max(-coast_alt, 0.0);
    // The wind roughens the sea: the mean square slope of its waves (Cox and Munk 1954) sets
    // how wide the sun's glitter spreads, and the waves it raises break higher on the shore.
    let wind = min(length(frame.wind.xz), 20.0);
    let slope_var = 0.003 + 0.00512 * wind;
    let breaker = clamp(0.3 + 0.012 * wind * wind, 0.3, 2.5);
    let wave_period = 6.0 + 0.3 * wind;
    let coast_near = planet.params.w * (1.0 - smoothstep(8.0, 60.0, pixel));
    // Seen from high up, the shelf seas along the coasts are paler and greener, most of all
    // in warm water; cold seas are a deeper, greener blue than the subtropical ones.
    let shelf = exp(-sea_depth / 150.0) * far * (0.35 + 0.65 * smoothstep(5.0, 25.0, temp_c));
    let deep = mix(vec3<f32>(1.0), vec3<f32>(0.8, 1.05, 0.95), (1.0 - smoothstep(0.0, 20.0, temp_c)) * far);
    let deep_col = planet.colors[C_OCEAN].rgb * deep;
    // Water over the coastal shelf scatters more light back (silt, plankton) than the open
    // ocean: paler and greener, least in warm, clear seas.
    let warm = smoothstep(10.0, 22.0, temp_c);
    let coastal = exp(-sea_depth / 25.0) * mix(0.7, 0.45, warm);
    var water = mix(deep_col, deep_col * vec3(1.6, 3.2, 2.6), max(coastal, shelf * 0.55));
    // That water takes the red out of the light within a few meters and the blue and green
    // over tens: in the shallows the bed shows through it (sand, with dark beds of weed and
    // rock), turquoise over sand. Light goes down and back up, on a slant. Warm seas are the
    // clearest; cooler ones are greener and murkier.
    let murk = mix(vec3<f32>(0.42, 0.13, 0.13), vec3<f32>(0.32, 0.07, 0.04), warm);
    let through = exp(-murk * sea_depth * 2.5);
    let weed = smoothstep(0.58, 0.75, patch_v + (detail_n - 0.5) * 0.25 * fine_fade) * macro_fade;
    let bed = mix(planet.colors[C_SAND].rgb * 0.85, mix(planet.colors[C_ROCK].rgb * 0.35, planet.colors[C_LUSH].rgb * 0.3, 0.6), weed * 0.75);
    water = mix(water, bed, through);
    let wave_fade = 1.0 - smoothstep(0.5, 8.0, pixel);
    var wave_n = normalize(mix(up, bump(up, dp1, dp2, dw1, dw2), wave_fade * 0.6));
    var sea_rough = mix(0.12, 0.05, wave_fade);
    // The sun's glitter: the sun mirrored in every wave facet that tilts the right way, a lobe
    // as wide as the waves are steep (added after shading, below; the surface itself stays
    // glossy so the sky mirrors in it). Up close the waves drawn carry part of that slope;
    // the rest is ripples smaller than a pixel. GGX's long tails spread a lobe more than the
    // Beckmann one the slopes were measured with, so its width is taken a little lower.
    var glitter_a = 0.7 * sqrt(mix(slope_var, max(slope_var - 0.012, 0.0025), wave_fade));
    // Surf: the waves steepen and break where the water gets about as shallow as they are
    // high, and their white water runs in to the shore in lines along it (lines of equal
    // depth), each with a sharp front and a fading trail, broken into patches. Froth lines the
    // water's edge. Where the lines get finer than a pixel they blend into their average.
    let surf_depth = breaker * 1.3;
    let line_phase = sea_depth / max(breaker * 0.45, 0.12) + frame.time.x / wave_period + (meadow_v - 0.5) * 3.0;
    let line_f = fract(line_phase);
    let surf_front = smoothstep(0.0, 0.05, line_f) * (1.0 - smoothstep(0.05, 0.6, line_f));
    let broken = smoothstep(0.25, 0.65, detail_n + (mid_v - 0.5) * 0.5 * clump_fade);
    let lines = mix(surf_front * broken, 0.2, smoothstep(0.3, 1.0, fwidth(line_phase)));
    let froth = (1.0 - smoothstep(0.03, 0.08 + breaker * 0.1, sea_depth)) * (0.55 + 0.45 * broken);
    var foam = saturate(lines * (1.0 - smoothstep(surf_depth * 0.6, surf_depth, sea_depth)) + froth) * coast_near;
    // Sea ice on cold seas: floes of snow-covered ice with dark leads of open water between
    // them, thinning out to drift ice at its edge.
    let ice_edge = -9.0 + (far_n.x - 0.5) * 6.0 + (macro_v - 0.5) * 3.0 * macro_fade;
    let sea_ice = smoothstep(ice_edge + 2.0, ice_edge - 2.0, temp_c) * planet.params.z;
    if (sea_ice > 0.0) {
        let floes = smoothstep(0.35, 0.5, far_n.y + (detail_n - 0.5) * 0.2 * fine_fade + (patch_v - 0.5) * 0.3 * macro_fade);
        let cover = sea_ice * mix(floes, 1.0, smoothstep(0.3, 1.0, sea_ice));
        water = mix(water, planet.colors[C_SNOW].rgb * (0.82 + 0.18 * far_n.x), cover);
        wave_n = normalize(mix(wave_n, up, cover));
        sea_rough = mix(sea_rough, 0.6, cover);
        foam *= 1.0 - cover;
    }
    // The beach the waves wash: dark wet sand up to where the biggest ones run, and the thin
    // sheet of each wave sliding up it and back, glossy, with froth along its edge.
    let runup = 0.2 + breaker * 0.6;
    let shore_h = coast_alt + (detail_n - 0.5) * 0.06 * fine_fade;
    let washed = (1.0 - smoothstep(runup * 0.8, runup * 1.2, shore_h)) * step(0.0, coast_alt) * coast_near * (1.0 - ocean);
    let surge = runup * (0.45 + 0.4 * sin(6.2832 * (frame.time.x / wave_period + (meadow_v - 0.5) * 0.7)));
    let swash = (1.0 - smoothstep(surge - 0.02, surge, shore_h)) * washed;
    let swash_edge = smoothstep(surge - 0.06, surge - 0.015, shore_h) * swash * broken;
    albedo = mix(albedo, albedo * 0.6, washed);
    roughness = mix(roughness, 0.4, washed);
    albedo = mix(albedo, mix(albedo * 0.7, water, 0.4), swash);
    roughness = mix(roughness, 0.06, swash);
    reflectance = mix(reflectance, 0.354, swash);
    n = normalize(mix(n, up, swash));
    albedo = mix(albedo, vec3<f32>(0.8, 0.82, 0.82), swash_edge * 0.8);
    roughness = mix(roughness, 0.5, swash_edge);
    water = mix(water, vec3<f32>(0.8, 0.82, 0.82), foam * 0.85);
    wave_n = normalize(mix(wave_n, up, foam));
    sea_rough = mix(sea_rough, 0.5, foam);
    albedo = mix(albedo, water, ocean);
    n = normalize(mix(n, wave_n, ocean));
    roughness = mix(roughness, sea_rough, ocean);
    reflectance = mix(reflectance, 0.354, ocean);
    occlusion = mix(occlusion, 1.0, ocean);
    subsurface = mix(subsurface, 0.0, ocean);

    // Gullies get less sky.
    occlusion *= 1.0 - gully * 0.25 * (1.0 - ocean);

    // Sliver pixels show the level crest.
    n = normalize(mix(n, up, sliver));

    // Debug views (RenderSettings.debug_view).
    if (frame.counts.z == 1u) {
        return vec4<f32>(n * 0.5 + 0.5, 1.0);
    }
    if (frame.counts.z == 3u) {
        return vec4<f32>(vec3(sky_visibility(in.rel, n)), 1.0);
    }
    if (frame.counts.z == 4u) {
        return vec4<f32>(vec3(screen_ao(in.rel)), 1.0);
    }

    // Regolith on airless worlds scatters light back the way it came (Lommel–Seeliger, mixed
    // with Lambert by phase angle as in the lunar-Lambert model): a full Moon is as bright at
    // its limb as in its middle and its relief fades under a high sun, and dust lit from
    // straight behind the viewer brightens (the opposition surge). The surface is lit as
    // Lambertian, so the difference goes into the albedo where the sun is up.
    let regolith = planet.params3.x * (1.0 - ocean);
    if (regolith > 0.0) {
        let v = normalize(-in.rel);
        let l = frame.sun_dir.xyz;
        let mu0 = dot(n, l);
        let phase = acos(clamp(dot(l, v), -1.0, 1.0));
        let share = regolith * mix(1.0, 0.5, smoothstep(0.0, 2.0, phase)) * smoothstep(0.0, 0.05, mu0);
        let lommel = 2.0 / max(mu0 + max(dot(n, v), 0.0), 0.6);
        albedo *= mix(1.0, lommel, share) * (1.0 + 0.3 * regolith * (1.0 - smoothstep(0.0, 0.2, phase)));
    }

    // Seen from far above, the environment map's sky is the camera's: on the night side
    // (past twilight) there is no sky to reflect.
    let own_sky = atmosphere_sky_ambient_weight(in.rel);
    if (own_sky > 0.0 && planet.params2.x > 0.5) {
        occlusion *= mix(1.0, smoothstep(-0.25, 0.05, dot(up, frame.sun_dir.xyz)), own_sky);
    }
    var s = default_surface();
    s.albedo = saturate3(albedo);
    s.roughness = clamp(roughness, 0.03, 1.0);
    s.reflectance = reflectance;
    s.normal = n;
    s.occlusion = occlusion;
    s.subsurface = subsurface;
    // The planet's own shadow past the terminator. An atmosphere's sun transmittance does
    // this already; on airless worlds relief on the night side that faces the sun would catch
    // it. Peaks standing far above the land around them (taken as 3 km over the datum) still
    // see the sun a little below their horizon.
    let to_sun = frame.sun_dir.xyz;
    var planet_lit = 1.0;
    if (planet.params2.x > 0.5) {
        let dip = sqrt(2.0 * max(altitude - 3000.0, 0.0) / planet.center.w);
        planet_lit = smoothstep(-dip - 0.015, -dip + 0.01, dot(up, to_sun));
    }
    planet_lit *= 1.0 - crater_shade;
    var color = shade_surface_shadowed(s, in.rel, planet_lit);
    let glitter = ocean * (1.0 - foam);
    if (glitter > 0.0 && dot(n, to_sun) > 0.0 && planet_lit > 0.0) {
        let v = normalize(-in.rel);
        let h = normalize(to_sun + v);
        let nl = saturate(dot(n, to_sun));
        let nv = max(dot(n, v), 1e-3);
        let spec = d_ggx(saturate(dot(n, h)), glitter_a) * v_smith_ggx_correlated(nv, nl, glitter_a)
            * f_schlick(vec3<f32>(0.02), saturate(dot(v, h))) * nl;
        color += pre_expose(frame.sun_color.rgb * sun_transmittance(in.rel) * shadow_factor(in.rel, n) * planet_lit * spec * glitter);
    }
    // Snow glints: the sun mirrored in a crystal facet (a glossy GGX lobe; facets stacked in
    // the crystal reflect far more than one ice surface's 2 %).
    if (glint > 0.0 && dot(n, to_sun) > 0.0 && planet_lit > 0.0) {
        let v = normalize(-in.rel);
        let d = d_ggx(saturate(dot(glint_n, normalize(to_sun + v))), 0.08);
        let flash = d * 0.15 * 0.25 / max(dot(n, v), 0.2) * glint;
        color += pre_expose(frame.sun_color.rgb * sun_transmittance(in.rel) * shadow_factor(in.rel, n) * planet_lit * flash);
    }
    if (maps.flags.z > 0.5) {
        let level = in.lod.y;
        let tint = vec3<f32>(fract(level * 0.37), fract(level * 0.61 + 0.3), fract(level * 0.83 + 0.6));
        color *= mix(vec3(1.0), tint * 1.6 + 0.2, 0.6);
    }
    // Towns and cities light the night side (from a few kilometers off; up close there are no
    // buildings to hold them). People live on mild, wet, low, gentle land, most by coasts and
    // rivers; not on the sea, ice, deserts or mountains, nor on hand-made ground.
    if (planet.params4.x > 0.0) {
        let night = smoothstep(0.03, -0.1, dot(up, to_sun)) * smoothstep(1500.0, 6000.0, dist);
        let hab = planet.params4.x * (1.0 - ocean) * (1.0 - smoothstep(0.0, 0.3, in.tint.a))
            * smoothstep(-6.0, 4.0, temp_c) * (1.0 - smoothstep(27.0, 35.0, temp_c))
            * mix(0.15, 1.0, smoothstep(0.05, 0.3, moisture + in.water.z))
            * (1.0 - smoothstep(800.0, 2500.0, altitude)) * (1.0 - smoothstep(0.12, 0.3, slope))
            * (0.55 + 0.45 * smoothstep(500.0, 0.0, altitude) + 0.8 * in.water.z);
        if (night * hab > 0.001) {
            let rot = planet_rot(planet);
            let local = transpose(rot) * up;
            color += pre_expose(night_lights(local * planet.center.w, planet.center.w, pixel) * night * hab);
        }
    }
    // The world's own lamps, from where the renderer stops lighting the ground with them.
    if (in.night.w >= 0.0) {
        let handoff = planet.params4.y;
        let night = smoothstep(0.03, -0.1, dot(up, to_sun)) * smoothstep(handoff, handoff * 1.25 + 50.0, dist);
        if (night > 0.0) {
            let local = transpose(planet_rot(planet)) * up;
            color += pre_expose(lamp_glow(in.night, local) * night);
        }
    }
    color = apply_fog(color, in.rel);
    return vec4<f32>(color, 1.0);
}
