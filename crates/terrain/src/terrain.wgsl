// Terrain patches (engine_terrain). Requires lib_types + lib_frame.
//
// Ground color comes from climate (moisture, temperature, altitude, slope) and painted
// layers; weather adds wetness, puddles with rain ripples, mud and snow from the surface
// layers grid; footprints and tracks come from the deformation grid (8 cm cells) as normals,
// occlusion and, on fine patches, a small vertex offset. Open sea is shaded as water.

// Must match `PlanetGpu` in render.rs.
struct PlanetGpu {
    rot: array<vec4<f32>, 3>,       // columns of terrain-local -> world rotation
    center: vec4<f32>,              // terrain origin relative to the camera, base radius
    colors: array<vec4<f32>, 10>,   // lush, dry, soil, rock, sand, snow, dark, ocean, mud, concrete
    params: vec4<f32>,              // snow below °C, rock slope, vegetation (0/1), ocean surface (0/1)
    params2: vec4<f32>,             // sphere (0/1)
};

// Must match `MapsGpu` in render.rs.
struct Maps {
    deform: vec4<f32>,   // origin x/z relative to the camera, cell size, cells per side
    surface: vec4<f32>,  // origin x/z relative to the camera, cell size, cells per side
    flags: vec4<f32>,    // deformation on, surface layers on, LOD debug colors
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

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) altitude: f32,
    @location(3) climate: vec4<f32>,   // moisture, temperature (-60..60 °C), mare, flags/255
    @location(4) paint: vec4<u32>,     // layer index, weight, regional variety, local patches (0..255)
    @location(5) origin: vec3<f32>,
    @location(6) planet: u32,
    @location(7) lod: vec4<f32>,       // vertex spacing (m), level
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) up: vec3<f32>,
    @location(3) climate: vec4<f32>,      // moisture, temperature °C, mare, altitude (m)
    @location(4) ocean_paint: vec4<f32>,  // ocean flag, paint weight, regional variety, local patches (0..1)
    @location(5) @interpolate(flat) ids: vec2<u32>,  // planet, paint layer
    @location(6) lod: vec2<f32>,
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

fn inside01(uv: vec2<f32>) -> f32 {
    return select(0.0, 1.0, all(uv > vec2(0.002)) && all(uv < vec2(0.998)));
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

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let planet = planets[in.planet];
    let rot = planet_rot(planet);
    var rel = in.origin + rot * in.position;
    let up = planet_up(planet, rel);
    // Geomorph toward the parent patch's shape as it nears the distance where it takes over.
    if (in.lod.w > 0.0) {
        let k = smoothstep(in.lod.z, in.lod.w, length(rel));
        rel += up * (in.normal.w * in.lod.x * MORPH_RANGE * k);
    }
    let flags = u32(round(in.climate.w * 255.0));
    let ocean = select(0.0, 1.0, (flags & 2u) != 0u);

    // Prints and tracks lower fine patches a little (shading carries the detail).
    if (maps.flags.x > 0.5 && in.lod.x < 1.5 && ocean < 0.5) {
        let uv = deform_uv(rel);
        let o = 0.35 * in.lod.x / (maps.deform.z * maps.deform.w);
        var d = textureSampleLevel(deform_tex, map_sampler, uv, 0.0).r * 0.4;
        d += textureSampleLevel(deform_tex, map_sampler, uv + vec2(o, 0.0), 0.0).r * 0.15;
        d += textureSampleLevel(deform_tex, map_sampler, uv - vec2(o, 0.0), 0.0).r * 0.15;
        d += textureSampleLevel(deform_tex, map_sampler, uv + vec2(0.0, o), 0.0).r * 0.15;
        d += textureSampleLevel(deform_tex, map_sampler, uv - vec2(0.0, o), 0.0).r * 0.15;
        rel -= up * d * inside01(uv);
    }

    var out: VertexOut;
    out.rel = rel;
    out.normal = normalize(rot * in.normal.xyz);
    out.up = up;
    out.climate = vec4<f32>(in.climate.x, in.climate.y * 120.0 - 60.0, in.climate.z, in.altitude);
    out.ocean_paint = vec4<f32>(ocean, vec3<f32>(in.paint.yzw) / 255.0);
    out.ids = vec2<u32>(in.planet, in.paint.x);
    out.lod = in.lod.xy;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    return out;
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
        default: { return p.colors[C_SOIL].rgb; }
    }
}

// How porous a layer is (how much rain darkens it and how readily it turns to mud).
fn paint_porosity(layer: u32) -> f32 {
    switch (layer) {
        case 6u, 7u: { return 0.35; }
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

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let planet = planets[in.ids.x];
    let up = normalize(in.up);
    var n = normalize(in.normal);
    let moisture = in.climate.x;
    let temp_c = in.climate.y;
    let mare = in.climate.z;
    let altitude = in.climate.w;
    let paint_w = in.ocean_paint.y;
    let variety = in.ocean_paint.z;
    let patches = in.ocean_paint.w;
    let layer = in.ids.y;
    let tp = tiling_pos(in.rel);
    let dist = length(in.rel);

    // Screen derivatives up front (uniform control flow).
    let dp1 = dpdx(in.rel);
    let dp2 = dpdy(in.rel);
    let pixel = max(length(dp1), length(dp2));
    // Capped so far-off lowland at grazing angles never blends toward sea.
    let alt_width = clamp(fwidth(altitude), 1e-3, 2.0);
    let detail_n = tile_fbm(tp, 1.0, 4);
    let detail_f = pnoise3(tp * 8.0, 4096.0 * 8.0);
    let macro_v = tile_fbm(tp, 1.0 / 64.0, 3);
    let meadow_v = tile_fbm(tp, 1.0 / 512.0, 3);
    let bump_h = detail_n * 0.25 + detail_f * 0.03;
    let dh1 = dpdx(bump_h);
    let dh2 = dpdy(bump_h);
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

    // Sea: below sea level, with a smooth coastline between vertices.
    let ocean = planet.params.w * saturate(0.5 - altitude / alt_width);

    // Ground maps around the focus.
    let duv = deform_uv(in.rel);
    let in_deform = inside01(duv) * maps.flags.x * (1.0 - ocean);
    let texel = 1.0 / maps.deform.w;
    let d0 = textureSampleLevel(deform_tex, map_sampler, duv, 0.0);
    let dxp = textureSampleLevel(deform_tex, map_sampler, duv + vec2(texel, 0.0), 0.0).r;
    let dxn = textureSampleLevel(deform_tex, map_sampler, duv - vec2(texel, 0.0), 0.0).r;
    let dzp = textureSampleLevel(deform_tex, map_sampler, duv + vec2(0.0, texel), 0.0).r;
    let dzn = textureSampleLevel(deform_tex, map_sampler, duv - vec2(0.0, texel), 0.0).r;
    let suv = surface_uv(in.rel);
    let in_surface = inside01(suv) * maps.flags.y;
    let layers = textureSampleLevel(surface_tex, map_sampler, suv, 0.0) * in_surface;

    let print_fade = in_deform * (1.0 - smoothstep(maps.deform.z * 1.5, maps.deform.z * 8.0, pixel));
    let depth = d0.r * in_deform;
    let flattened = d0.g * in_deform;

    // Pattern fades: a pattern disappears once a pixel covers several of its cycles.
    let fine_fade = 1.0 - smoothstep(0.3, 3.0, pixel);
    let macro_fade = 1.0 - smoothstep(20.0, 200.0, pixel);

    // ---- Natural ground from climate.
    let slope = 1.0 - saturate(dot(n, up));
    let vegetation = planet.params.z;
    let meadow_fade = 1.0 - smoothstep(40.0, 400.0, pixel);
    let wet_bias = (macro_v - 0.5) * 0.35 * macro_fade + (meadow_v - 0.5) * 0.45 * meadow_fade
        + (variety - 0.5) * 0.5 + (patches - 0.5) * 0.5;
    var albedo = mix(planet.colors[C_DRY].rgb, planet.colors[C_LUSH].rgb, smoothstep(0.2, 0.7, moisture + wet_bias));
    albedo = mix(planet.colors[C_SOIL].rgb, albedo, vegetation * smoothstep(-8.0, 6.0, temp_c));
    // Meadows and hay fields: sunlit dry patches between the woods.
    let field = vegetation * smoothstep(0.56, 0.64, patches + (variety - 0.5) * 0.2) * smoothstep(4.0, 12.0, temp_c);
    albedo = mix(albedo, mix(planet.colors[C_DRY].rgb, planet.colors[C_LUSH].rgb, 0.45), field * 0.35);
    // Woods: dark canopy from afar, a darker forest floor up close (the foliage module draws
    // the trees themselves).
    let forest = vegetation
        * smoothstep(0.42, 0.5, moisture * 0.6 + (0.5 - patches) * 0.9 + (variety - 0.5) * 0.4)
        * smoothstep(-6.0, 4.0, temp_c)
        * (1.0 - smoothstep(1800.0, 2600.0, altitude + (variety - 0.5) * 600.0));
    let canopy = mix(0.35, 0.85, smoothstep(500.0, 2500.0, dist));
    albedo = mix(albedo, planet.colors[C_LUSH].rgb * vec3<f32>(0.45, 0.55, 0.45), forest * canopy);
    // Beaches and dry deserts.
    let beach = vegetation * (1.0 - smoothstep(1.5, 5.0, altitude)) * step(-1.0, altitude);
    let desert = vegetation * (1.0 - smoothstep(0.08, 0.22, moisture + (variety - 0.5) * 0.1)) * smoothstep(12.0, 22.0, temp_c);
    albedo = mix(albedo, planet.colors[C_SAND].rgb, saturate(beach + desert));
    // Moon maria and dark basalt.
    albedo = mix(albedo, planet.colors[C_DARK].rgb, mare);
    // Rock on steep ground and bare alpine heights.
    // Coarse patches average slopes out: lower the thresholds so cliffs still read as rock.
    let rock_slope = planet.params.y * (1.0 - 0.45 * smoothstep(4.0, 150.0, in.lod.x));
    let alpine = smoothstep(2100.0, 3000.0, altitude + (variety - 0.5) * 800.0) * vegetation;
    let rock = saturate(smoothstep(rock_slope - 0.08, rock_slope + 0.05, slope + (detail_n - 0.5) * 0.12 * fine_fade) + alpine * 0.8);
    albedo = mix(albedo, planet.colors[C_ROCK].rgb * (0.85 + 0.3 * variety), rock);
    // Small-scale variation.
    albedo *= mix(1.0, 0.82 + 0.36 * detail_f, fine_fade) * mix(1.0, 0.88 + 0.24 * macro_v, macro_fade);

    // ---- Painted layers (mud, snow, sand, dirt, grass, rock, concrete, ash).
    let paint_col = paint_color(planet, layer);
    albedo = mix(albedo, paint_col * mix(1.0, 0.9 + 0.2 * detail_f, fine_fade), paint_w);
    var porosity = mix(1.0 - rock * 0.6, paint_porosity(layer), paint_w);

    var roughness = mix(0.92, 0.75, rock);
    var occlusion = 1.0;
    var reflectance = 0.5;
    var subsurface = 0.0;

    // ---- Detail bump (fades with distance).
    let bump_fade = (1.0 - smoothstep(0.15, 1.2, pixel)) * (1.0 - ocean);
    n = normalize(mix(n, bump(n, dp1, dp2, dh1, dh2), bump_fade * (0.6 + rock)));

    // ---- Footprints and tracks: normals from the print depth field, darker hollows.
    let cell = maps.deform.z;
    let grad = vec2<f32>(dxp - dxn, dzp - dzn) / (2.0 * cell);
    let print_n = normalize(n + vec3<f32>(grad.x, 0.0, grad.y) * 1.4);
    n = normalize(mix(n, print_n, print_fade));
    let hollow = saturate(depth * 6.0);
    occlusion *= 1.0 - hollow * 0.35 * print_fade;
    let rim = saturate(-depth * 20.0);
    albedo *= 1.0 + rim * 0.06;
    // Trampled grass: flattened blades read slightly darker and yellower.
    albedo = mix(albedo, albedo * vec3<f32>(0.78, 0.82, 0.62), flattened * 0.7);
    roughness = mix(roughness, 0.7, flattened * 0.5);

    // ---- Snow (cold climate, snowfall in the surface layers, painted snow).
    let snow_line = planet.params.x;
    let climate_snow = smoothstep(snow_line + 2.0, snow_line - 3.0, temp_c + (detail_n - 0.5) * 4.0 * fine_fade + (variety - 0.5) * 3.0 + (patches - 0.5) * 2.0)
        * (1.0 - smoothstep(rock_slope - 0.02, rock_slope + 0.14, slope + (detail_n - 0.5) * 0.1 * fine_fade));
    let fallen = max(layers.z, select(0.0, frame.weather.y * 0.1, maps.flags.y < 0.5));
    let weather_snow = smoothstep(0.004, 0.04, fallen + (detail_f - 0.5) * 0.01 * fine_fade)
        * (1.0 - smoothstep(0.55, 0.8, slope));
    let painted_snow = select(0.0, paint_w, layer == 2u);
    var snow = saturate(max(max(climate_snow, weather_snow), painted_snow)) * (1.0 - ocean);
    // Deep prints in thin snow show the ground below.
    let snow_depth = max(fallen, painted_snow * 0.25 + climate_snow * 0.5);
    snow *= 1.0 - smoothstep(0.7, 1.0, depth / max(snow_depth, 1e-3)) * print_fade;
    let packed = saturate(depth * 12.0) * print_fade;
    let snow_col = planet.colors[C_SNOW].rgb * mix(1.0, 0.82, packed) * mix(vec3(1.0), vec3(0.93, 0.96, 1.0), packed);
    albedo = mix(albedo, snow_col, snow);
    roughness = mix(roughness, mix(0.6, 0.45, packed), snow);
    subsurface = mix(subsurface, 0.35, snow);
    porosity *= 1.0 - snow;

    // ---- Rain: wet ground darkens and gets glossy, soil turns to mud, water pools.
    let rain_wet = max(layers.x, select(0.0, frame.weather.x, maps.flags.y < 0.5));
    let wet = saturate(rain_wet) * porosity;
    // Bare soil turns to mud; grass stays grass (darker, with a sheen) unless it is trodden.
    let natural_bare = 1.0 - vegetation * smoothstep(-8.0, 6.0, temp_c);
    let painted_bare = select(0.0, 1.0, layer == 0u || layer == 1u || layer == 4u || layer == 8u);
    let bare = mix(natural_bare, painted_bare, paint_w);
    let trodden = saturate(depth * 25.0) * print_fade + flattened * 0.6;
    let soil_like = (1.0 - rock) * saturate(bare + trodden);
    let mud = saturate(max(select(0.0, paint_w, layer == 1u), wet * soil_like * smoothstep(0.55, 0.9, rain_wet) * 0.8)) * (1.0 - snow);
    albedo = mix(albedo, planet.colors[C_MUD].rgb * mix(1.0, 0.85 + 0.3 * detail_f, fine_fade), mud * 0.85);
    albedo *= mix(1.0, mix(0.72, 0.5, soil_like), wet);
    roughness = mix(roughness, mix(0.55, 0.35, soil_like), wet * 0.8);
    roughness = mix(roughness, 0.22, mud * saturate(rain_wet + 0.3));

    // Puddles: from the surface layers, in low spots, and in prints on soaked ground.
    // Ragged puddle edges, only where the surface layers hold standing water.
    let puddle_level = select(-1.0, layers.y + (detail_n - 0.5) * 0.02, layers.y > 0.0005);
    let print_pool = smoothstep(0.015, 0.04, depth) * smoothstep(0.5, 0.85, rain_wet) * porosity;
    let puddle = saturate(max(smoothstep(0.0, 0.015, puddle_level), print_pool)) * (1.0 - snow) * (1.0 - ocean);
    let ripple_n = bump(up, dp1, dp2, dr1, dr2);
    let still = normalize(mix(up, ripple_n, saturate(frame.weather.w * 0.2) * (1.0 - smoothstep(0.02, 0.2, pixel))));
    n = normalize(mix(n, still, puddle));
    albedo = mix(albedo, albedo * 0.35, puddle);
    roughness = mix(roughness, 0.04, puddle);
    reflectance = mix(reflectance, 0.354, puddle);

    // ---- Scorch marks.
    let scorch = layers.w;
    albedo = mix(albedo, vec3<f32>(0.025, 0.023, 0.022), scorch * 0.9);
    roughness = mix(roughness, 0.95, scorch);

    // ---- Open sea.
    let sea_depth = max(-altitude, 0.0);
    let shallow = exp(-sea_depth / 25.0);
    let water = mix(planet.colors[C_OCEAN].rgb, planet.colors[C_OCEAN].rgb * vec3(1.6, 3.2, 2.6), shallow * 0.7);
    let wave_fade = 1.0 - smoothstep(0.5, 8.0, pixel);
    let wave_n = normalize(mix(up, bump(up, dp1, dp2, dw1, dw2), wave_fade * 0.6));
    albedo = mix(albedo, water, ocean);
    n = normalize(mix(n, wave_n, ocean));
    roughness = mix(roughness, mix(0.12, 0.05, wave_fade), ocean);
    reflectance = mix(reflectance, 0.354, ocean);
    occlusion = mix(occlusion, 1.0, ocean);
    subsurface = mix(subsurface, 0.0, ocean);

    // Debug views (RenderSettings.debug_view).
    if (frame.counts.z == 1u) {
        return vec4<f32>(n * 0.5 + 0.5, 1.0);
    }
    if (frame.counts.z == 3u) {
        return vec4<f32>(vec3(sky_visibility(in.rel, n)), 1.0);
    }

    var s = default_surface();
    s.albedo = saturate3(albedo);
    s.roughness = clamp(roughness, 0.03, 1.0);
    s.reflectance = reflectance;
    s.normal = n;
    s.occlusion = occlusion;
    s.subsurface = subsurface;
    var color = shade_surface(s, in.rel);
    if (maps.flags.z > 0.5) {
        let level = in.lod.y;
        let tint = vec3<f32>(fract(level * 0.37), fract(level * 0.61 + 0.3), fract(level * 0.83 + 0.6));
        color *= mix(vec3(1.0), tint * 1.6 + 0.2, 0.6);
    }
    color = apply_fog(color, in.rel);
    return vec4<f32>(color, 1.0);
}
