// ============================================================================
// Water: lakes and oceans (Gerstner waves + interactive ripples), flowing water (shallow-water
// grid), puddles, and the underwater view. Composed after lib_types + lib_frame.
//
// Light model, per pixel above water:
//   reflection (SSR, then the sky) × Fresnel
// + (the refracted scene behind the surface, absorbed along the light's path through the water,
//    plus sun caustics on the bottom) × (1 − Fresnel)
// + light scattered back by the water itself (its color: blue-green, silt brown, algae green)
// + sun glitter, crest glow, foam.
// ============================================================================

const MAX_WAVES: u32 = 24u;
const MAX_BODIES: u32 = 16u;

struct GpuWave {
    a: vec4<f32>, // dir.x, dir.z, k (rad/m), amplitude (m)
    b: vec4<f32>, // horizontal amplitude (m), phase (rad), omega (rad/s), unused
};

struct GpuBody {
    origin: vec4<f32>,  // wave reference point rel camera (x, z), still level rel camera (y), extent kind (0 ∞, 1 circle, 2 rect)
    extent: vec4<f32>,  // extent center rel camera x, z, radius | half x, half z
    rot: vec4<f32>,     // cos yaw, sin yaw, bed kind (0 terrain, 1 flat, 2 bowl), bed depth (m)
    bed: vec4<f32>,     // bowl shore width (m), typical depth (m), wind speed (m/s), wave count
    absorb: vec4<f32>,  // absorption rgb (1/m), scattering (1/m)
    scatter: vec4<f32>, // scatter color rgb, roughness
    look: vec4<f32>,    // foam, caustics, refraction, interactive (1/0)
    grid: vec4<f32>,    // grid center rel camera x, z, inner spacing s0 (m), growth alpha
    grid2: vec4<f32>,   // rings, segments, outer radius (m), max wave amplitude (m)
    wind: vec4<f32>,    // wind direction x, z, current x, z (m/s)
    detail: vec4<f32>,  // fine waves: amplitude scale, cos and sin of the turn from the tiles' wind, slope variance nothing draws
    waves: array<GpuWave, 24>,
};

struct Globals {
    ripple: vec4<f32>,   // grid origin rel camera x, z, size (m), enabled
    rain: vec4<f32>,     // weather rain (mm/h), world time (s, wrapped), underwater body (-1 none), camera depth (m)
    showers: array<vec4<f32>, 4>, // center rel camera x, z, radius (m), rate (mm/h)
    puddle: vec4<f32>,   // origin rel camera x, z, cell (m), texels per side
    puddle2: vec4<f32>,  // height the puddle texture's ground is relative to (rel camera y), unused
    counts: vec4<u32>,   // bodies, showers, puddles on, plunges
    under: vec4<f32>,    // underwater absorption rgb, scattering
    under2: vec4<f32>,   // underwater scatter color rgb, surface level rel camera y
    detail: vec4<f32>,   // fine waves: 1/tile size (1/m) x, y; streams' amplitude scale; ripples from surface_tex (1/0)
    detail2: vec4<f32>,  // fine waves: tiles' wind direction x, z; on (1/0), unused
    plunges: array<vec4<f32>, 8>, // where falls land: foot rel camera xyz, boil radius (m)
    coarse: vec4<f32>,   // dispersive ripples' coarse level: corner rel camera x, z, size (m), drawn (1/0)
    mirror_vp: mat4x4<f32>, // the lake mirror's picture: view-projection of camera-relative points
    mirror: vec4<f32>,   // its exposure / ours, near plane (m), mips, body slot (-1 none)
    mirror2: vec4<f32>,  // the mirrored eye rel camera xyz, the plane's y rel camera
    shelter: vec4<f32>,  // wind exposure mask (crate::shelter): corner rel camera x, z, size (m), body slot (-1 none)
    swash: vec4<f32>,    // the swash (render/swash.rs): corner rel camera x, z, size (m), on (1/0)
    swash2: vec4<f32>,   // its datum rel camera y, cell (m), unused, unused
    waterline: vec4<f32>,  // the camera at the waterline: split per pixel (1/0), lens dome radius (m), lens wetness 0..1, seconds since it surfaced
    waterline2: vec4<f32>, // meters of the dome per pixel, drop seed, unused, unused
    trail: vec4<f32>,      // the ripple grid's trail (`trail.rs`): corner rel camera x, z, size (m), on
};

struct WaterUniform {
    g: Globals,
    bodies: array<GpuBody, 16>,
};

@group(1) @binding(0) var<uniform> water: WaterUniform;
@group(1) @binding(1) var ripple_tex: texture_2d<f32>;      // r height, g foam, b dh/dx, a dh/dz
@group(1) @binding(2) var scene_color: texture_2d<f32>;     // pre-exposed HDR, before transparents
@group(1) @binding(3) var scene_depth: texture_depth_2d;    // reverse-Z
@group(1) @binding(4) var puddle_tex: texture_2d<f32>;      // r depth (m), g ground y rel puddle2.x
@group(1) @binding(5) var current_tex: texture_2d<f32>;     // ripple grid currents: x, z (m/s), unused, bubbles
@group(1) @binding(6) var detail_tex: texture_2d_array<f32>; // fine waves, per tile: (dh/dx, dh/dz, h, Jacobian), (moments, foam)
@group(1) @binding(7) var surface_tex: texture_2d<f32>;     // dispersive ripples on the ripple grid: height, dh/dx, dh/dz
@group(1) @binding(8) var coarse_tex: texture_2d<f32>;      // their long waves, on a grid 4 times larger: the same
@group(1) @binding(9) var mirror_tex: texture_2d<f32>;      // the lake mirror's picture: pre-exposed HDR, mip chain
@group(1) @binding(10) var mirror_depth: texture_depth_2d;  // its depth (reverse-Z, near / view depth)
@group(1) @binding(11) var shelter_tex: texture_2d<f32>;    // wind exposure over the nearest lake: 0 lee .. 1 open
@group(1) @binding(12) var swash_a: texture_2d<f32>;        // the swash: level drawn (m over its datum; below -1e3 none), depth (m), strip weight, foam
@group(1) @binding(13) var swash_b: texture_2d<f32>;
@group(1) @binding(14) var trail_tex: texture_2d<f32>;       // foam and bubbles the ripple grid's wakes left, 4 times wider        // what its sand remembers: sheen, damp, mark, bed (m over the datum)

// The body being shaded reflects through the lake mirror (set by fs_body).
var<private> use_mirror: bool = false;
// How much the bed seen through the water darkens: the swash's sand is soaked, though the scene
// colour the water refracts was drawn dry (the sand decal goes on after it is copied).
var<private> wet_bed: f32 = 1.0;
// The bed seen through running water is the stream's own stones (set by fs_flow), the current
// there (m/s) sorting them by size.
var<private> stream_bed: f32 = 0.0;
var<private> stream_speed: f32 = 0.0;
var<private> stream_depth: f32 = 0.0;

struct FlowGpu {
    origin: vec4<f32>,  // corner rel camera x, base y, z, cell (m)
    dims: vec4<f32>,    // cells x, z, cos and sin of the grid's yaw
    absorb: vec4<f32>,
    scatter: vec4<f32>,
    look: vec4<f32>,
};
@group(2) @binding(0) var<uniform> flow: FlowGpu;
@group(2) @binding(1) var flow_tex: texture_2d<f32>;        // r surface (m above base), g depth, b vel x, a vel z

// Deep water's reflectance (Gordon) gives what one look straight down sees; light scattered
// in from the sides and the view's slant through the bright top layer add to it.
const SCATTER_GAIN: f32 = 2.5;
// Backscattering of the bubble cloud under a fresh wake, 1/m: bubbles swamp what the water's
// own particles scatter back many times over just after a breaker (Terrill, Melville and
// Stramski 2001), so the cloud glows well above the water around it, far below white foam.
const BUBBLE_BB: f32 = 0.06;

// ---------------------------------------------------------------- helpers
struct WaterLook {
    absorb: vec3<f32>,
    scattering: f32,
    scatter_color: vec3<f32>,
    roughness: f32,
    foam: f32,
    caustics: f32,
    refraction: f32,
};

fn body_look(bi: u32) -> WaterLook {
    let b = water.bodies[bi].absorb;
    let s = water.bodies[bi].scatter;
    let l = water.bodies[bi].look;
    return WaterLook(b.rgb, b.w, s.rgb, s.w, l.x, l.y, l.z);
}

fn flow_look() -> WaterLook {
    return WaterLook(flow.absorb.rgb, flow.absorb.w, flow.scatter.rgb, flow.scatter.w,
                     flow.look.x, flow.look.y, flow.look.z);
}

// View depth of a reverse-Z infinite depth value (sky = far away).
fn linear_depth(d: f32) -> f32 {
    if (d <= 1e-7) { return 1e6; }
    return frame.exposure.z / d;
}

fn time_s() -> f32 { return water.g.rain.y; }

// Torn foam: patches with holes in them.
fn froth_noise(p: vec2<f32>) -> f32 {
    return value_noise2(p * 2.3) * 0.6 + value_noise2(p * 6.1 + vec2<f32>(4.1, 2.7)) * 0.4;
}

// Where a fall lands, the water boils: a churned white core where the falling water drives
// air under, and the foam it brings up carried outward in torn patches, thinning as it
// spreads. x: foam, yz: slope of the churned surface.
fn plunge_boil(rel: vec3<f32>) -> vec3<f32> {
    var out = vec3<f32>(0.0);
    let wp = tiling_pos(rel).xz;
    let t = time_s();
    for (var i = 0u; i < min(water.g.counts.w, 8u); i++) {
        let p = water.g.plunges[i];
        let d = rel.xz - p.xz;
        let r = length(d);
        if (r > p.w * 2.5 || abs(rel.y - p.y) > 1.5) { continue; }
        let x = r / p.w;
        let dir = d / max(r, 1e-3);
        // The foam rides the outflow (about 1 m/s): two copies carried outward, cross-faded.
        let ph0 = fract(t * 0.25);
        let ph1 = fract(t * 0.25 + 0.5);
        let n0 = froth_noise(wp - dir * ph0 * 4.0);
        let n1 = froth_noise(wp - dir * ph1 * 4.0 + vec2<f32>(5.3, 1.7));
        let patches = mix(n0, n1, abs(ph0 - 0.5) * 2.0);
        // Froth is bubbles, not paint: fine lace tears holes in it, more as it thins out.
        let lace = value_noise2(wp * 11.0 - dir * ph0 * 44.0) * 0.5
            + value_noise2(wp * 23.0 + vec2<f32>(t * 0.7, 1.3)) * 0.5;
        let core = 1.0 - smoothstep(0.1, 0.7, x + (patches - 0.5) * 0.4);
        let spread = smoothstep(0.45 + 0.1 * x, 0.75 + 0.1 * x, patches) * (1.0 - smoothstep(0.4, 2.4, x));
        let holes = smoothstep(0.1 + 0.2 * x, 0.45 + 0.2 * x, lace);
        out.x = max(out.x, (core * 1.4 + spread * 0.9) * mix(0.5, 1.0, holes));
        // The core heaves and churns.
        let e = 0.05;
        let q = wp * 3.0 + vec2<f32>(t * 2.1, -t * 1.7);
        let h = value_noise2(q);
        let g = vec2<f32>(value_noise2(q + vec2<f32>(e, 0.0)) - h, value_noise2(q + vec2<f32>(0.0, e)) - h) / e;
        let churn = 1.0 - smoothstep(0.0, 1.3, x);
        out = vec3<f32>(out.x, out.yz + g * 0.06 * churn);
    }
    return out;
}

// The boil's outflow at `rel` (m/s, x and z): radially out from the nearest plunge.
fn plunge_flow(rel: vec3<f32>) -> vec2<f32> {
    var v = vec2<f32>(0.0);
    for (var i = 0u; i < min(water.g.counts.w, 8u); i++) {
        let p = water.g.plunges[i];
        let d = rel.xz - p.xz;
        let r = length(d);
        if (r > p.w * 2.5 || abs(rel.y - p.y) > 1.5) { continue; }
        v += d / max(r, 1e-3) * (1.0 - smoothstep(0.3, 2.4, r / p.w));
    }
    return v;
}

// A pixel's footprint on a level surface at camera-relative `rel`: `across` the view (meters)
// and stretched `stretch` times along the horizontal view direction `view`, where at grazing
// angles a pixel covers a long strip of water.
struct Footprint {
    across: f32,
    stretch: f32,
    view: vec2<f32>,
}

fn footprint(rel: vec3<f32>) -> Footprint {
    let dist = length(rel);
    let across = 2.0 * dist / (frame.proj[1][1] * frame.viewport.y);
    let stretch = 1.0 / max(abs(rel.y) / max(dist, 1e-4), 0.03);
    let view = rel.xz / max(length(rel.xz), 1e-4);
    return Footprint(across, stretch, view);
}

// The footprint's length along horizontal direction `dir`: waves running along the view alias
// first at grazing angles, waves running across it stay sharp.
fn footprint_along(fp: Footprint, dir: vec2<f32>) -> f32 {
    let d = dot(dir, fp.view);
    return fp.across * sqrt(1.0 + d * d * (fp.stretch * fp.stretch - 1.0));
}

// One size for the footprint (the geometric mean of its width and length).
fn pixel_footprint(rel: vec3<f32>) -> f32 {
    let fp = footprint(rel);
    return fp.across * sqrt(fp.stretch);
}

// Extent-local offset of a camera-relative x/z point.
fn body_local(bi: u32, rel_xz: vec2<f32>) -> vec2<f32> {
    let d = rel_xz - water.bodies[bi].extent.xy;
    let c = water.bodies[bi].rot.x;
    let s = water.bodies[bi].rot.y;
    return vec2<f32>(c * d.x - s * d.y, s * d.x + c * d.y);
}

// Distance inward from the extent's edge (negative outside).
fn body_inset(bi: u32, local: vec2<f32>) -> f32 {
    let kind = water.bodies[bi].origin.w;
    let e = water.bodies[bi].extent;
    if (kind < 0.5) { return 1e9; }
    if (kind < 1.5) { return e.z - length(local); }
    return min(e.z - abs(local.x), e.w - abs(local.y));
}

// Still-water depth for wave shoaling (terrain beds use the body's typical depth).
fn body_depth(bi: u32, rel_xz: vec2<f32>) -> f32 {
    let kind = water.bodies[bi].rot.z;
    let depth = water.bodies[bi].rot.w;
    if (kind < 0.5) { return water.bodies[bi].bed.y; }
    if (kind < 1.5) { return depth; }
    let inset = body_inset(bi, body_local(bi, rel_xz));
    let t = saturate(inset / max(water.bodies[bi].bed.x, 0.01));
    return depth * t * t * (3.0 - 2.0 * t);
}

struct WaveOut {
    offset: vec3<f32>,
    normal: vec3<f32>,
    jacobian: f32,
    // Mean square slope of the waves faded out for being too short to draw.
    slope_var: f32,
};

// Sum of Gerstner waves at rest position p0 (rel. the body's reference). Waves shorter than
// `min_len` meters fade out (they would alias on the grid or in the pixel).
fn eval_waves(bi: u32, p0: vec2<f32>, depth: f32, min_len: f32) -> WaveOut {
    var off = vec3<f32>(0.0);
    var dhx = 0.0;
    var dhz = 0.0;
    var jxx = 0.0;
    var jzz = 0.0;
    var jxz = 0.0;
    var faded = 0.0;
    let count = min(u32(water.bodies[bi].bed.w), MAX_WAVES);
    for (var i = 0u; i < count; i++) {
        let w = water.bodies[bi].waves[i];
        let k = w.a.z;
        let lod = saturate((TAU / max(k, 1e-4)) / max(min_len, 1e-4) - 1.0);
        let shoal = tanh(k * max(depth, 0.0) * 1.2);
        // What fading takes away from the drawn slopes still roughens the pixel.
        let ka_full = k * w.a.w * shoal;
        faded += 0.5 * ka_full * ka_full * (1.0 - lod * lod);
        let f = shoal * lod;
        if (f <= 0.0) { continue; }
        let a = w.a.w * f;
        let c = w.b.x * f;
        let dir = w.a.xy;
        let theta = k * dot(dir, p0) - w.b.y;
        let s = sin(theta);
        let co = cos(theta);
        off += vec3<f32>(c * dir.x * co, a * s, c * dir.y * co);
        let ka = k * a;
        let kc = k * c;
        dhx += ka * dir.x * co;
        dhz += ka * dir.y * co;
        jxx += kc * dir.x * dir.x * s;
        jzz += kc * dir.y * dir.y * s;
        jxz += kc * dir.x * dir.y * s;
    }
    let tx = vec3<f32>(1.0 - jxx, dhx, -jxz);
    let tz = vec3<f32>(-jxz, dhz, 1.0 - jzz);
    var out: WaveOut;
    out.offset = off;
    out.normal = normalize(cross(tz, tx));
    out.jacobian = (1.0 - jxx) * (1.0 - jzz) - jxz * jxz;
    out.slope_var = faded;
    return out;
}

// ---------------------------------------------------------------- fine wind waves
// The fine waves' tiles (`render/detail.rs`) at world x/z `p`, for a body that shows them at
// `scale` (amplitude) with its wind turned `turn` (cos, sin) from the tiles' wind. The pixel's
// footprint `fp` picks the mip, and what the mip averages away comes back as slope variance
// (LEAN mapping: the mips hold the slopes' second moments too).
struct Detail {
    grad: vec2<f32>,  // dh/dx, dh/dz
    variance: f32,    // mean square slope the pixel doesn't resolve
    height: f32,
    squeeze: f32,     // 1 − Jacobian of the chop: > 0 where crests pinch
    foam: f32,        // foam lingering where they broke
};

fn turn_by(v: vec2<f32>, turn: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(turn.x * v.x - turn.y * v.y, turn.y * v.x + turn.x * v.y);
}

fn detail_at(p: vec2<f32>, scale: f32, turn: vec2<f32>, fp: Footprint) -> Detail {
    var out = Detail(vec2<f32>(0.0), 0.0, 0.0, 0.0, 0.0);
    if (scale <= 0.0 || water.g.detail2.z < 0.5) { return out; }
    // Into the tiles' frame.
    let back = vec2<f32>(turn.x, -turn.y);
    let q = turn_by(p, back);
    let along = turn_by(fp.view, back);
    let side = vec2<f32>(-along.y, along.x);
    // The sampler filters isotropically: the footprint's geometric mean along the view, so
    // grazing views keep their ripples (what it misses is in the variance anyway).
    let len_along = fp.across * sqrt(fp.stretch);
    var g = vec2<f32>(0.0);
    for (var c = 0; c < 2; c++) {
        let inv = select(water.g.detail.y, water.g.detail.x, c == 0);
        let uv = q * inv;
        let ddx = along * (len_along * inv);
        let ddy = side * (fp.across * inv);
        let a = textureSampleGrad(detail_tex, linear_repeat, uv, c * 2, ddx, ddy);
        let m = textureSampleGrad(detail_tex, linear_repeat, uv, c * 2 + 1, ddx, ddy);
        g += a.xy;
        out.variance += max(m.x - a.x * a.x, 0.0) + max(m.y - a.y * a.y, 0.0);
        out.height += a.z;
        out.squeeze += 1.0 - a.w;
        out.foam = max(out.foam, m.w);
    }
    out.grad = turn_by(g, turn) * scale;
    out.variance *= scale * scale;
    out.height *= scale;
    out.squeeze *= scale;
    return out;
}

// The fine waves' height at a vertex `spacing` meters from its neighbors (mips average away
// what the grid can't hold).
fn detail_height(p: vec2<f32>, scale: f32, turn: vec2<f32>, spacing: f32) -> f32 {
    if (scale <= 0.0 || water.g.detail2.z < 0.5) { return 0.0; }
    let q = turn_by(p, vec2<f32>(turn.x, -turn.y));
    var h = 0.0;
    for (var c = 0; c < 2; c++) {
        let inv = select(water.g.detail.y, water.g.detail.x, c == 0);
        // Texels per grid cell; a cell needs two texels' worth of mip to hold a wave.
        let lod = log2(max(spacing * 2.0 * inv * 128.0, 1.0));
        h += textureSampleLevel(detail_tex, linear_repeat, q * inv, c * 2, lod).z;
    }
    return h * scale;
}

// How much of a camera-centred grid (the ripples, the dispersive surface's levels) shows at
// `uv` (0..1 across it): all of it in the middle, fading out over the outer third of a round
// patch. A square cut at the edge read from the air as a sharp-edged block of detailed water
// under a low-flying craft (playtest 2026-10-29 item 7).
fn grid_fade(uv: vec2<f32>) -> f32 {
    return 1.0 - smoothstep(0.65, 0.98, length(uv - vec2<f32>(0.5)) * 2.0);
}

// Interactive ripples at a camera-relative x/z: (height, foam, dh/dx, dh/dz), faded at the edge.
fn ripple_at(rel_xz: vec2<f32>) -> vec4<f32> {
    if (water.g.ripple.w < 0.5) { return vec4<f32>(0.0); }
    // The dispersive surface's long waves, out to 128 m (`surface.wgsl`'s coarse level).
    var long = vec4<f32>(0.0);
    if (water.g.coarse.w > 0.5) {
        let cuv = (rel_xz - water.g.coarse.xy) / water.g.coarse.z;
        if (all(cuv > vec2<f32>(0.0)) && all(cuv < vec2<f32>(1.0))) {
            let s = textureSampleLevel(coarse_tex, linear_clamp, cuv, 0.0);
            long = vec4<f32>(s.x, 0.0, s.y, s.z) * grid_fade(cuv);
        }
    }
    let uv = (rel_xz - water.g.ripple.xy) / water.g.ripple.z;
    if (any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) { return long; }
    let fade = grid_fade(uv);
    var r = textureSampleLevel(ripple_tex, linear_clamp, uv, 0.0);
    if (water.g.detail.w > 0.5) {
        // The dispersive surface's short waves, with the grid's foam.
        let s = textureSampleLevel(surface_tex, linear_clamp, uv, 0.0);
        r = vec4<f32>(s.x, r.y, s.y, s.z);
    }
    return r * fade + long;
}

// ---------------------------------------------------------------- swash
// The swash over the beaches (`crate::swash`), at a camera-relative x/z: (level rel camera y,
// depth m, how much of it runs free 0..1, foam), bilinear over the cells that hold water
// (cells outside the band hold none). w = 0 where there is no swash.
struct Swash {
    level: f32,
    depth: f32,
    w: f32,
    foam: f32,
};

fn swash_at(rel_xz: vec2<f32>) -> Swash {
    var out = Swash(0.0, 0.0, 0.0, 0.0);
    if (water.g.swash.w < 0.5) { return out; }
    let n = i32(textureDimensions(swash_a).x);
    let g = (rel_xz - water.g.swash.xy) / water.g.swash2.y - vec2<f32>(0.5);
    if (any(g < vec2<f32>(0.0)) || any(g > vec2<f32>(f32(n) - 1.0))) { return out; }
    let i = min(vec2<i32>(floor(g)), vec2<i32>(n - 2));
    let f = g - vec2<f32>(i);
    var sum = vec4<f32>(0.0);
    var weight = 0.0;
    for (var c = 0; c < 4; c++) {
        let o = vec2<i32>(c & 1, c >> 1);
        let t = textureLoad(swash_a, i + o, 0);
        if (t.x < -1e3) { continue; }
        let wo = mix(1.0 - f.x, f.x, f32(o.x)) * mix(1.0 - f.y, f.y, f32(o.y));
        sum += t * wo;
        weight += wo;
    }
    if (weight < 1e-4) { return out; }
    let t = sum / weight;
    // Faded out at the window's edge, as the ripple grids are.
    let fade = grid_fade((g + 0.5) / f32(n)) * saturate(weight * 2.0);
    out.level = water.g.swash2.x + t.x;
    out.depth = t.y;
    out.w = saturate(1.0 - t.z) * fade;
    out.foam = t.w;
    return out;
}

// What the sand remembers at a camera-relative x/z: (sheen, damp, mark, ground rel camera y).
fn swash_sand(rel_xz: vec2<f32>) -> vec4<f32> {
    if (water.g.swash.w < 0.5) { return vec4<f32>(0.0, 0.0, 0.0, -1e9); }
    let uv = (rel_xz - water.g.swash.xy) / water.g.swash.z;
    if (any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) { return vec4<f32>(0.0, 0.0, 0.0, -1e9); }
    let t = textureSampleLevel(swash_b, linear_clamp, uv, 0.0);
    return vec4<f32>(t.xyz * grid_fade(uv), water.g.swash2.x + t.w);
}

// The swash's slope at a camera-relative x/z (d level/dx, d level/dz).
fn swash_slope(rel_xz: vec2<f32>) -> vec2<f32> {
    let e = water.g.swash2.y;
    let xp = swash_at(rel_xz + vec2<f32>(e, 0.0));
    let xm = swash_at(rel_xz - vec2<f32>(e, 0.0));
    let zp = swash_at(rel_xz + vec2<f32>(0.0, e));
    let zm = swash_at(rel_xz - vec2<f32>(0.0, e));
    if (xp.w * xm.w * zp.w * zm.w <= 0.0) { return vec2<f32>(0.0); }
    return vec2<f32>(xp.level - xm.level, zp.level - zm.level) / (2.0 * e);
}

// Currents stirred into the ripple grid at a camera-relative x/z: (x, z m/s, 0, bubbles 0..1),
// faded at the edge.
fn current_at(rel_xz: vec2<f32>) -> vec4<f32> {
    if (water.g.ripple.w < 0.5) { return vec4<f32>(0.0); }
    let uv = (rel_xz - water.g.ripple.xy) / water.g.ripple.z;
    if (any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) { return vec4<f32>(0.0); }
    return textureSampleLevel(current_tex, linear_clamp, uv, 0.0) * grid_fade(uv);
}

// Foam and bubbles the ripple grid's wakes left behind at a camera-relative x/z: the trail's
// share, which takes over as the grid fades out (Crest's blend from one level of its cascade
// to the next), and fades out itself in a circle four times as wide.
fn trail_at(rel_xz: vec2<f32>) -> vec2<f32> {
    if (water.g.trail.w < 0.5) { return vec2<f32>(0.0); }
    let tuv = (rel_xz - water.g.trail.xy) / water.g.trail.z;
    if (any(tuv <= vec2<f32>(0.0)) || any(tuv >= vec2<f32>(1.0))) { return vec2<f32>(0.0); }
    var t = textureSampleLevel(trail_tex, linear_clamp, tuv, 0.0).xy * grid_fade(tuv);
    if (water.g.ripple.w > 0.5) {
        let uv = (rel_xz - water.g.ripple.xy) / water.g.ripple.z;
        if (all(uv > vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0))) { t *= 1.0 - grid_fade(uv); }
    }
    return t;
}

// Rain falling at a camera-relative x/z, mm/h.
fn rain_at(rel_xz: vec2<f32>) -> f32 {
    var r = water.g.rain.x;
    for (var i = 0u; i < min(water.g.counts.y, 4u); i++) {
        let s = water.g.showers[i];
        if (length(rel_xz - s.xy) < s.z) { r += s.w; }
    }
    return r;
}

// Gusts: the breeze roughens the water in drifting patches (cat's paws), calm in between.
// About 1 on average.
fn gusts(p: vec2<f32>, wind_dir: vec2<f32>, wind: f32) -> f32 {
    let drift = wind_dir * time_s() * (0.4 + 0.1 * wind);
    let gust = value_noise2(p * 0.09 - drift * 0.09) * 0.65 + value_noise2(p * 0.31 - drift * 0.31) * 0.35;
    return 0.35 + 1.3 * smoothstep(0.3, 0.75, gust);
}

// How much of the wind reaches body `bi`'s water at camera-relative `rel` (crate::shelter):
// 0 in the lee of its banks and crags .. 1 in the open.
fn shelter_at(bi: u32, rel: vec3<f32>) -> f32 {
    let s = water.g.shelter;
    if (i32(bi) != i32(s.w)) { return 1.0; }
    let uv = (rel.xz - s.xy) / s.z;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) { return 1.0; }
    return textureSampleLevel(shelter_tex, linear_clamp, uv, 0.0).r;
}

// Natural slicks (0..1): below about 4 m/s, films of surfactant (algae, pollen) damp the short
// waves in smooth bands and patches drawn out downwind, which drift with the breeze (EPFL's
// Lake Geneva slicks); a stronger wind tears them up.
fn slicks(p: vec2<f32>, wind_dir: vec2<f32>, wind: f32) -> f32 {
    let calm = 1.0 - smoothstep(2.5, 4.5, wind);
    if (calm <= 0.0) { return 0.0; }
    let across = vec2<f32>(-wind_dir.y, wind_dir.x);
    let drift = time_s() * 0.08;
    let q = vec2<f32>(dot(p, wind_dir) * 0.025 - drift * 0.025, dot(p, across) * 0.09);
    let warp = value_noise2(p * 0.05 + vec2<f32>(4.1, 9.3)) * 2.0;
    let bands = value_noise2(q + vec2<f32>(warp, 0.0)) * 0.7 + value_noise2(q * 2.7 + vec2<f32>(1.3, 6.1)) * 0.3;
    return smoothstep(0.52, 0.66, bands) * calm;
}

// Small wind ripples (capillary waves): gradient (dh/dx, dh/dz) of short analytic waves
// spread around the wind direction. `wind` m/s; waves shorter than a few pixels of `fp` (along
// their direction) fade out instead of shimmering.
fn capillary_gradient(p: vec2<f32>, wind_dir: vec2<f32>, wind: f32, fp: Footprint) -> vec2<f32> {
    let base = atan2(wind_dir.y, wind_dir.x);
    let strength = 0.02 + 0.07 * saturate(wind / 8.0);
    var g = vec2<f32>(0.0);
    let t = time_s();
    // Bend the crests a little so the few long waves left at a distance don't line up into
    // fingerprint-like bands.
    let q = p + (vec2<f32>(value_noise2(p * 0.37), value_noise2(p * 0.37 + vec2<f32>(17.3, 5.1))) - 0.5) * 1.0;
    for (var i = 0; i < 12; i++) {
        let fi = f32(i);
        let ang = base + (hash12(vec2<f32>(fi, 3.7)) - 0.5) * 3.0;
        let dir = vec2<f32>(cos(ang), sin(ang));
        let lambda = 0.06 + 0.7 * pow(hash12(vec2<f32>(fi, 9.1)), 1.7);
        let lod = saturate(lambda / (footprint_along(fp, dir) * 4.0) - 1.0);
        if (lod <= 0.0) { continue; }
        let k = TAU / lambda;
        let omega = sqrt(9.81 * k + 0.074 / 1000.0 * k * k * k);
        let phase = hash12(vec2<f32>(fi, 1.3)) * TAU;
        let a = strength * lambda / TAU * lod;
        g += dir * (a * k * cos(k * dot(dir, q) - omega * t + phase));
    }
    // Gusts: the breeze roughens the water in drifting patches (cat's paws), calm in between.
    let drift = wind_dir * t * (0.4 + 0.1 * wind);
    let gust = value_noise2(p * 0.09 - drift * 0.09) * 0.65 + value_noise2(p * 0.31 - drift * 0.31) * 0.35;
    return g * (0.25 + 1.5 * smoothstep(0.3, 0.75, gust));
}

// Raindrop rings: gradient of expanding rings in a scattered grid of drop sites.
fn rain_gradient(p: vec2<f32>, mm_h: f32, fade: f32) -> vec2<f32> {
    if (mm_h <= 0.01 || fade <= 0.0) { return vec2<f32>(0.0); }
    let density = saturate(0.25 + mm_h * 0.06);
    var g = vec2<f32>(0.0);
    let t = time_s();
    for (var layer = 0; layer < 2; layer++) {
        let lf = f32(layer);
        let scale = 2.5 + lf * 1.7;
        let q = p * scale + vec2<f32>(lf * 7.3, lf * 3.1);
        let cell = floor(q);
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let c = cell + vec2<f32>(f32(x), f32(y));
                let h = hash12(c + vec2<f32>(lf * 13.0, 0.0));
                if (h > density) { continue; }
                let period = 0.7 + h * 0.8;
                let life = fract(t / period + hash12(c + vec2<f32>(3.3, lf)));
                let center = c + vec2<f32>(hash12(c + vec2<f32>(1.7, 0.2)), hash12(c + vec2<f32>(2.9, 5.1)));
                let d = q - center;
                let r = length(d);
                let x0 = r - life * 1.1;
                let env = exp(-x0 * x0 * 30.0) * (1.0 - life) * (1.0 - life);
                let dr = (cos(x0 * 22.0) * 22.0 - 60.0 * x0 * sin(x0 * 22.0)) * env;
                g += d / max(r, 1e-3) * dr * 0.012;
            }
        }
    }
    return g * fade;
}

// Bright line network of sunlight focused by the waves, on a bottom at world x/z.
fn caustic_layer(p: vec2<f32>, t: f32) -> f32 {
    let i = floor(p);
    let f = fract(p);
    var d1 = 8.0;
    var d2 = 8.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let g = vec2<f32>(f32(x), f32(y));
            let h = vec2<f32>(hash12(i + g), hash12(i + g + vec2<f32>(19.1, 7.3)));
            let o = 0.5 + 0.42 * sin(t * (0.5 + h * 0.6) + h * TAU);
            let d = length(g + o - f);
            if (d < d1) { d2 = d1; d1 = d; } else if (d < d2) { d2 = d; }
        }
    }
    let edge = 1.0 - smoothstep(0.0, 0.32, d2 - d1);
    return edge * edge;
}

fn caustics(p: vec2<f32>) -> f32 {
    let t = time_s();
    let a = caustic_layer(p * 0.8, t * 0.9);
    let b = caustic_layer(p * 1.15 + vec2<f32>(3.1, 1.7), t * 1.2);
    return a * b * 2.5 + (a + b) * 0.45;
}

// Radiance the water itself sends back (in-scattering of sun and sky), not pre-exposed.
fn medium_radiance(look: WaterLook, rel: vec3<f32>, depth_below: f32) -> vec3<f32> {
    return backscatter(look, rel, depth_below, 0.0, 1.0);
}

// Henyey-Greenstein phase function (1/sr) at the cosine of the scattering angle.
fn phase_hg(cos_t: f32, g: f32) -> f32 {
    let d = 1.0 + g * g - 2.0 * g * cos_t;
    return (1.0 - g * g) / (4.0 * PI * d * sqrt(d));
}

// The water's own light seen along `dir` from inside it, `depth_below` meters under the
// surface, as far as the eye sees: the light coming down scattered once toward the eye,
// L = b·β̃(θ)·E / c per band. Natural waters scatter mostly forward: a Henyey-Greenstein
// phase function with g = 0.924 matches Petzold's measured average (Mobley, Ocean Optics Web
// Book): about 0.004 /sr sideways, a bright halo looking toward the sun, dark looking down
// (the light comes from above: Tyler 1960). The light coming down fades with depth by the
// diffuse attenuation, about (a + bb) / 0.8, not by the beam's a + b. Seen from above the
// water is what it sends back up (`medium_radiance`); from inside it this, which is far less
// saturated: a green lake is a pale green-grey under the water, not a deep green.
fn in_water_radiance(look: WaterLook, dir: vec3<f32>, depth_below: f32) -> vec3<f32> {
    let up = vec3<f32>(0.0, 1.0, 0.0);
    let bb = look.scattering * look.scatter_color;
    let b = look.scattering * look.scatter_color / max(dot(look.scatter_color, vec3<f32>(1.0 / 3.0)), 1e-6);
    let c = look.absorb + b;
    let down = exp(-(look.absorb + bb) / 0.8 * max(depth_below, 0.0));
    // The sun's beam, bent down through the surface (Fresnel's loss is a few percent).
    let sun_in = refract(-frame.sun_dir.xyz, up, 1.0 / 1.333);
    let sun = water_sun(vec3<f32>(0.0)) * max(frame.sun_dir.y, 0.0) * shadow_factor(vec3<f32>(0.0), up);
    let p_sun = phase_hg(dot(sun_in, -dir), 0.924);
    // The sky's light comes down within Snell's window: a broad beam from overhead.
    let sky = sky_ambient(up) * sky_visibility(vec3<f32>(0.0), up) * PI;
    let p_sky = phase_hg(-dir.y, 0.6);
    // Multiple scattering spreads some of it every way.
    let spread = (sun + sky) * (0.15 * b / max(c, vec3<f32>(1e-5))) / (4.0 * PI);
    return pre_expose((b / max(c, vec3<f32>(1e-5)) * (sun * p_sun + sky * p_sky) + spread) * down);
}

// Sunlight reaching camera-relative `rel` through the air and under the clouds' shadows, as the
// ground beside the water gets it (lib_frame `cloud_shadow`).
fn water_sun(rel: vec3<f32>) -> vec3<f32> {
    return frame.sun_color.rgb * sun_transmittance(rel) * cloud_shadow(rel);
}

// `medium_radiance` with `extra_bb` (1/m) more backscattering, white: bubbles, which scatter
// every color alike. What comes back is still tinted by what the water absorbs on the way, so
// a bubble cloud glows in the water's own color, paler: turquoise in a green lake. `sun_vis`:
// how much of the sun reaches the water (a cloud just under the surface sits in its shadows).
fn backscatter(look: WaterLook, rel: vec3<f32>, depth_below: f32, extra_bb: f32, sun_vis: f32) -> vec3<f32> {
    let sun = water_sun(rel) * max(frame.sun_dir.y, 0.0) * sun_vis;
    let sky = sky_ambient(vec3<f32>(0.0, 1.0, 0.0)) * sky_visibility(rel, vec3<f32>(0.0, 1.0, 0.0)) * PI;
    let down = exp(-(look.absorb + vec3<f32>(look.scattering)) * max(depth_below, 0.0));
    let e = (sun * 0.98 + sky) * INV_PI * down;
    // Light the water sends back up from below the surface: Gordon's reflectance of deep
    // water, r = (0.0949 + 0.0794 u) u with u = bb / (a + bb), per band. It is what the
    // particles scatter back (`scatter_color` is backscattering per meter of scattering)
    // against what the water absorbs: blue for pure water, turquoise for rock flour, green
    // for algae, brown for silt and tannins.
    let bb = look.scattering * look.scatter_color + vec3<f32>(extra_bb);
    let u = bb / max(look.absorb + bb, vec3<f32>(1e-5));
    let r = (0.0949 + 0.0794 * u) * u;
    return r * e * PI * SCATTER_GAIN;
}

// Where the mirror's picture shows camera-relative point `p`: uv and view depth.
fn mirror_uv(p: vec3<f32>) -> vec3<f32> {
    let c = water.g.mirror_vp * vec4<f32>(p, 1.0);
    let w = max(c.w, 1e-4);
    return vec3<f32>(c.x / w * 0.5 + 0.5, 0.5 - c.y / w * 0.5, c.w);
}

// The lake mirror (crate::mirror, engine_render::PlanarReflection): the scene rendered from the
// eye mirrored in the lake's plane, so every point of the plane shows what a flat mirror there
// reflects, the crag above the screen's top edge and boats' undersides included. Looks up what
// lies along the reflected ray `r` from surface point `rel`, blurred by the GGX lobe of
// perceptual roughness `rough`. Returns (pre-exposed color, weight): 0 off the picture, or where
// the picture holds something under the plane (a post's foot, a swimmer's legs), which the
// mirrored view can't clip without an oblique near plane.
fn mirror_reflection(rel: vec3<f32>, r: vec3<f32>, rough: f32) -> vec4<f32> {
    let m = water.g.mirror;
    let eye = water.g.mirror2.xyz;
    let plane = water.g.mirror2.w;
    let size = vec2<f32>(textureDimensions(mirror_depth));
    // The flat mirror's ray through this point (the waves are centimeters on a calm lake), and
    // how far beyond the plane it meets something, from the picture's depth there (the sky: far).
    let p = vec3<f32>(rel.x, plane, rel.z);
    let s0 = mirror_uv(p);
    if (any(s0.xy < vec2<f32>(0.0)) || any(s0.xy > vec2<f32>(1.0))) { return vec4<f32>(0.0); }
    let d0 = textureLoad(mirror_depth, vec2<i32>(min(s0.xy * size, size - 1.0)), 0);
    var hit = 4000.0;
    if (d0 > 0.0) {
        hit = clamp((m.y / d0 - s0.z) / s0.z * length(p - eye), 0.0, 4000.0);
    }
    // Something under the plane (a post's foot, a hull) stands in the flat ray's way.
    let flat_ok = d0 <= 0.0 || m.y / d0 > s0.z - 0.05;
    // The waves tilt the ray: look up what lies that far along the tilted one, where the
    // mirrored eye sees it (parallax-correct for small tilts, unlike a fixed uv offset).
    let q = p + r * hit;
    var s = mirror_uv(q);
    if (s.z <= 1e-3) { return vec4<f32>(0.0); }
    // Tilted past the picture's margin: its edge holds about what lies there (mostly sky),
    // trusted less the farther out.
    let edge = min(min(s.x, s.y), min(1.0 - s.x, 1.0 - s.y));
    s = vec3<f32>(clamp(s.xy, vec2<f32>(0.0), vec2<f32>(1.0)), s.z);
    // What the picture holds there must lie beyond the plane: its ray meets the plane at the
    // fraction f of the way from the mirrored eye to q.
    // If the tilted ray's is (the shore's slope under the water, seen along a grazing ray),
    // the flat mirror's ray stands in for it.
    let texel = vec2<i32>(min(s.xy * size, size - 1.0));
    let d = textureLoad(mirror_depth, texel, 0);
    let f = saturate((plane - eye.y) / max(q.y - eye.y, 1e-4));
    if (d > 0.0 && m.y / d < s.z * f - 0.05) {
        if (!flat_ok) { return vec4<f32>(0.0); }
        s = vec3<f32>(s0.xy, s.z);
    }
    // Blur: the lobe's width where the ray ends, as the picture sees it, picks the mip.
    let a = rough * rough;
    let side = normalize(cross(r, vec3<f32>(0.0, 1.0, 0.0)) + vec3<f32>(1e-4, 0.0, 0.0));
    let s2 = mirror_uv(q + side * max(hit, 0.05) * a);
    let blur = length((s2.xy - s.xy) * size);
    let lod = clamp(log2(max(blur, 1.0)), 0.0, m.z - 1.0);
    let color = textureSampleLevel(mirror_tex, linear_clamp, s.xy, lod).rgb * m.x;
    return vec4<f32>(color, 1.0 - smoothstep(0.0, 0.25, -edge));
}

// Screen-space reflection: march the reflected ray against the scene depth copy.
// Returns (pre-exposed color, confidence).
fn trace_reflection(rel: vec3<f32>, r: vec3<f32>, size: vec2<f32>, roughness: f32) -> vec4<f32> {
    if (roughness > 0.5) { return vec4<f32>(0.0); }
    let dist = length(rel);
    var t_prev = 0.0;
    var t = 0.3 + dist * 0.01;
    for (var i = 0; i < 28; i++) {
        let c = frame.view_proj * vec4<f32>(rel + r * t, 1.0);
        if (c.w <= 0.01) { break; }
        let ndc = c.xyz / c.w;
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) { break; }
        if (textureLoad(scene_depth, vec2<i32>(uv * size), 0) > ndc.z) {
            // The ray went behind the scene between the last two steps: bisect for where it
            // meets it, so the hit doesn't jump from step to step (banding).
            var lo = t_prev;
            var hi = t;
            for (var k = 0; k < 6; k++) {
                let mid = 0.5 * (lo + hi);
                let cm = frame.view_proj * vec4<f32>(rel + r * mid, 1.0);
                let nm = cm.xyz / cm.w;
                let um = vec2<f32>(nm.x * 0.5 + 0.5, 0.5 - nm.y * 0.5);
                if (textureLoad(scene_depth, vec2<i32>(clamp(um, vec2<f32>(0.0), vec2<f32>(0.999)) * size), 0) > nm.z) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let ch = frame.view_proj * vec4<f32>(rel + r * hi, 1.0);
            let nh = ch.xyz / ch.w;
            let uh = clamp(vec2<f32>(nh.x * 0.5 + 0.5, 0.5 - nh.y * 0.5), vec2<f32>(0.0), vec2<f32>(0.999));
            let z_scene = linear_depth(textureLoad(scene_depth, vec2<i32>(uh * size), 0));
            if (ch.w - z_scene < 0.15 + hi * 0.06) {
                let edge = min(min(uh.x, uh.y), min(1.0 - uh.x, 1.0 - uh.y));
                let fade = smoothstep(0.0, 0.08, edge) * (1.0 - roughness * 2.0);
                return vec4<f32>(textureSampleLevel(scene_color, linear_clamp, uh, 0.0).rgb, fade);
            }
            // Passed behind something: keep going, it may come out the other side.
        }
        t_prev = t;
        t *= 1.32;
    }
    return vec4<f32>(0.0);
}

// Foam on running water (0..1 pattern at `wp`): streaks drawn out along the current `vel`
// and carried downstream (two copies over 4 s, cross-faded). Streaks run along the two nearest
// of eight fixed headings, blended by the current's own, so where the current turns the pattern
// bends with it instead of winding up.
// Froth's structure, 0..1: aged foam gathers along the walls between the cells of the water
// it floats on and opens in their middles, a web of lines round clear patches (the lace seen
// on any wake or swash from above). High along the walls of two warped cell sizes, about
// 0.55 m and 0.2 m, so the web tears into finer threads as it thins; far off, where a pixel
// spans the cells, their mean.
fn foam_cells(wp: vec2<f32>, rel: vec3<f32>) -> f32 {
    let warp = vec2<f32>(value_noise2(wp * 0.9), value_noise2(wp * 0.9 + vec2<f32>(5.2, 1.3))) - 0.5;
    let q = wp + warp * 0.7;
    let big = stone_cell(q / 0.55 + vec2<f32>(13.1, 7.7));
    let small = stone_cell(q / 0.2 + vec2<f32>(3.9, 21.4));
    let web_big = 1.0 - smoothstep(0.0, 0.22, big.d2 - big.d1);
    let web_small = 1.0 - smoothstep(0.0, 0.3, small.d2 - small.d1);
    let v = 0.2 + 0.55 * web_big + 0.25 * web_small;
    let pixel = pixel_footprint(rel);
    let near = mix(v, 0.2 + 0.55 * web_big + 0.25 * 0.45, smoothstep(0.08, 0.2, pixel));
    return mix(near, 0.5, smoothstep(0.2, 0.5, pixel));
}

fn streak_foam(wp: vec2<f32>, vel: vec2<f32>) -> f32 {
    let t = time_s();
    let ph0 = fract(t * 0.25);
    let ph1 = fract(t * 0.25 + 0.5);
    let fade = abs(ph0 - 0.5) * 2.0;
    let s = atan2(vel.y, vel.x) / (PI * 0.25);
    let k0 = floor(s);
    let p0 = wp - vel * 4.0 * ph0;
    let p1 = wp - vel * 4.0 * ph1;
    var sum = 0.0;
    for (var k = 0; k < 2; k++) {
        let a = (k0 + f32(k)) * PI * 0.25;
        let d = vec2<f32>(cos(a), sin(a));
        let side = vec2<f32>(-d.y, d.x);
        // Three times longer along the current than across it.
        let q0 = vec2<f32>(dot(p0, d) * 0.33, dot(p0, side)) * 2.3;
        let q1 = vec2<f32>(dot(p1, d) * 0.33, dot(p1, side)) * 2.3 + vec2<f32>(5.2, 1.3);
        let w = select(1.0 - (s - k0), s - k0, k == 1);
        sum += w * mix(fbm2(q0, 4), fbm2(q1, 4), fade);
    }
    // Blending two headings evens the pattern out: restore its contrast about the mean.
    let f = s - k0;
    return 0.47 + (sum - 0.47) * inverseSqrt(f * f + (1.0 - f) * (1.0 - f));
}

// Water this deep (m) over whatever lies right behind it froths against it (`contact`).
const CONTACT_M: f32 = 0.12;
// Wet sand against dry, the albedo it keeps (Lagarde: soaked sand about 0.6 of dry).
const WET_SAND: f32 = 0.58;

// Full water surface shading seen from above. `n` world normal, `foam` 0..1, `crest` 0..1 how
// high on a wave this point is, `wind` m/s. Returns pre-exposed radiance with fog applied.
// `edge` is how deep the water gets before the waterline is fully drawn, meters.
// `shore_depth`: water shallower than this laps at the shore (a lake's beach; 0 for none,
// as for streams, which lap by their own depth). `vel`: the current (m/s), which carries the
// foam downstream. `bubbles` 0..1: the cloud churned under foam, lingering after it bursts.
fn shade_water(rel: vec3<f32>, n_in: vec3<f32>, look: WaterLook, foam_in: f32, crest: f32,
               frag: vec4<f32>, wind: f32, edge: f32, shore_depth: f32, vel: vec2<f32>,
               slope_var: f32, bubbles: f32) -> vec3<f32> {
    let dist = length(rel);
    let v = -rel / max(dist, 1e-4);
    var n = n_in;
    let ndv0 = dot(n, v);
    if (ndv0 < 0.02) { n = normalize(n + v * (0.02 - ndv0)); }
    let n_dot_v = max(dot(n, v), 1e-3);
    // Wind roughens the water below what the waves and ripples resolve (`slope_var`, the mean
    // square slope of what the pixel holds but can't draw; all of it adds up to Cox and Munk's
    // 0.003 + 0.00512·U). Rough water blurs the reflections and breaks up the grazing mirror
    // (facets tilt toward the eye).
    let unresolved = sqrt(max(slope_var, 0.0));
    let fresnel = 0.02 + 0.98 * pow(1.0 - n_dot_v, 5.0) * (1.0 - 0.55 * saturate(unresolved * 2.0));
    let size = vec2<f32>(textureDimensions(scene_color));
    let pix = vec2<i32>(frag.xy);
    let uv0 = frag.xy / size;
    let z_frag = max(view_depth(rel), 1e-3);

    // ---- refraction: what's behind the surface, and how much water is in between
    let z0 = linear_depth(textureLoad(scene_depth, pix, 0));
    let thick0 = max(z0 - z_frag, 0.0);
    // Water depth straight below this point, from the ground seen through it: the waterline
    // fades in over a few centimeters instead of showing the mesh edge.
    var shoreline = 1.0;
    var contact = 0.0;
    if (rel.y < 0.0 && z0 < 1e5) {
        let d_behind = -rel.y * max(z0 / z_frag - 1.0, 0.0);
        shoreline = smoothstep(0.0, edge, d_behind);
        // Contact: where something crosses the surface (a post, a hull, a stone, the beach),
        // the water froths in a thin ragged ring against it (depth-difference foam, as Crest
        // and most AAA water do): the "it touches the water" cue.
        let wp = tiling_pos(rel).xz;
        let ragged = 0.5 + value_noise2(wp * 3.1 + vec2<f32>(time_s() * 0.25, 0.0));
        contact = 1.0 - smoothstep(0.0, CONTACT_M * ragged, d_behind);
    }
    let nv = (frame.view * vec4<f32>(n, 0.0)).xyz;
    let strength = look.refraction * 0.05 * saturate(thick0 * 0.7) * min(1.0, 12.0 / z_frag);
    var uv = clamp(uv0 + vec2<f32>(nv.x, -nv.y) * strength, vec2<f32>(0.0), vec2<f32>(0.999));
    var z_bottom = linear_depth(textureLoad(scene_depth, vec2<i32>(uv * size), 0));
    if (z_bottom < z_frag) {
        uv = uv0;
        z_bottom = z0;
    }
    var behind = textureSampleLevel(scene_color, linear_clamp, uv, 0.0).rgb * wet_bed;
    let ratio = z_bottom / z_frag;
    let thick = dist * max(ratio - 1.0, 0.0);
    let bottom_rel = rel * ratio;
    let depth_below = max(rel.y - bottom_rel.y, 0.0);
    if (stream_bed > 0.0) {
        // The bed under the stream's own depth along the bent view ray (the scene copy's depth
        // there holds the channel's ground, which a thin sheet hardly differs from).
        // The flat surface bends the ray as a whole; its ripples only shift the bed by a
        // fraction of the depth (all of it would scramble the stones pixel by pixel).
        let bent = refract(-v, vec3<f32>(0.0, 1.0, 0.0), 1.0 / 1.333);
        let bed_rel = rel + bent * (stream_depth / max(-bent.y, 0.25))
            + vec3<f32>(n.x, 0.0, n.z) * stream_depth * 0.35;
        behind = mix(behind, river_stones(bed_rel, rel, stream_speed), stream_bed * smoothstep(0.0, 0.03, stream_depth));
    }
    let sigma_t = look.absorb + vec3<f32>(look.scattering);
    // Light reaching the bottom lost depth_below on the way down, then `thick` back up to us.
    let t_view = exp(-sigma_t * min(thick, 400.0));
    let t_bottom = t_view * exp(-look.absorb * min(depth_below, 400.0));

    // Sun caustics on the bottom (only where it is in water and lit).
    var lit_bottom = behind;
    let sun_up = saturate(frame.sun_dir.y * 3.0);
    if (look.caustics > 0.0 && sun_up > 0.0 && depth_below > 0.02 && depth_below < 30.0) {
        let bottom_world = tiling_pos(bottom_rel);
        // Project along the refracted sun so the pattern sits under the waves that make it.
        let sun_in = refract(-frame.sun_dir.xyz, vec3<f32>(0.0, 1.0, 0.0), 1.0 / 1.333);
        let p = bottom_world.xz - sun_in.xz / max(-sun_in.y, 0.2) * depth_below;
        let c = caustics(p * 1.4);
        let k = look.caustics * sun_up * saturate(depth_below * 3.0) * exp(-depth_below * 0.18)
            * shadow_factor(rel, vec3<f32>(0.0, 1.0, 0.0));
        lit_bottom = behind * (1.0 + c * k * 1.6);
    }

    let medium = pre_expose(medium_radiance(look, rel, 0.0));
    let water_col = lit_bottom * t_bottom + medium * (1.0 - t_view);

    // ---- reflection
    var r = reflect(-v, n);
    if (r.y < 0.02) { r = normalize(vec3<f32>(r.x, 0.02 + (0.02 - r.y) * 0.5, r.z)); }
    // GGX's α² follows the facets' mean square slope (as Beckmann's m² does).
    let base_a = look.roughness * look.roughness;
    let rough = clamp(sqrt(sqrt(base_a * base_a + slope_var)), 0.02, 0.75);
    let sky_vis = sky_visibility(rel, vec3<f32>(0.0, 1.0, 0.0));
    let fill = frame.ambient.rgb;
    let env = (mix(sample_env(r), sky_ambient(r), saturate(rough * 1.5)) - fill) * sky_vis + fill;
    var refl = pre_expose(env);
    // The lake mirror where it has the answer; screen-space reflections, then the sky, where
    // it doesn't.
    var mirrored = vec4<f32>(0.0);
    if (use_mirror) { mirrored = mirror_reflection(rel, r, rough); }
    if (mirrored.a < 0.999) {
        let ssr = trace_reflection(rel, r, size, rough);
        refl = mix(refl, ssr.rgb, ssr.a);
    }
    refl = mix(refl, mirrored.rgb, mirrored.a);

    // ---- sun glitter
    let l = frame.sun_dir.xyz;
    let sun = water_sun(rel);
    let shadow = shadow_factor(rel, n);
    let h = normalize(l + v);
    let n_dot_l = saturate(dot(n, l));
    // The sun is a disk, not a point: its glitter spreads over at least its own width, so a
    // calm surface mirrors a sun, not a burning pixel.
    let tan_sun = tan(frame.sun_dir.w);
    let a = sqrt(rough * rough * rough * rough + tan_sun * tan_sun);
    let spec = d_ggx(saturate(dot(n, h)), a) * v_smith_ggx_correlated(n_dot_v, max(n_dot_l, 1e-3), a)
        * f_schlick(vec3<f32>(0.02), saturate(dot(l, h))) * n_dot_l;
    let glitter = pre_expose(spec * sun * shadow);

    // ---- light through thin crests (wave tips glow when the sun is behind them)
    let behind_sun = pow(saturate(dot(v, -normalize(vec3<f32>(l.x, 0.0, l.z) + vec3<f32>(0.0001)))), 3.0);
    // Light through a crest: what a meter and a half of this water lets through, tinted by
    // what its particles scatter.
    let bs = look.scatter_color / max(max(look.scatter_color.r, max(look.scatter_color.g, look.scatter_color.b)), 1e-5);
    let through = exp(-look.absorb * 1.5) * mix(vec3<f32>(1.0), bs, 0.5);
    let tint = through / max(max(through.r, max(through.g, through.b)), 1e-5);
    let sss = pre_expose(tint * 0.6 * sun * shadow * behind_sun * crest * 0.012);

    var color = water_col * (1.0 - fresnel) + refl * fresnel + glitter + sss;

    // ---- bubbles: churned under the foam, they rise out over seconds after it has burst. The
    // cloud scatters light back up just under the surface and hides what is below, so a wake
    // fades from white through a pale trail in the water's own color before it clears. It
    // thins in wisps from its edges, as the foam does.
    if (bubbles > 0.001 && thick < 1e5) {
        let wp = tiling_pos(rel).xz;
        let wisps = fbm2(wp * 1.3 + vec2<f32>(3.7, time_s() * 0.03), 3) / 0.875;
        let theta = 1.0 - bubbles * 1.2;
        let dense = smoothstep(theta - 0.25, theta + 0.25, wisps) * saturate(bubbles * 1.5) * 0.8;
        let cloud = pre_expose(backscatter(look, rel, 0.0, BUBBLE_BB, shadow));
        color += (1.0 - fresnel) * dense * (cloud - water_col);
    }

    // ---- foam: whitecaps, wakes, splashes, shoreline
    var foam = foam_in + contact * 0.45;
    let shore = 1.0 - smoothstep(0.0, max(shore_depth, 1e-4), depth_below);
    if (thick < 1e5 && shore_depth > 0.0) {
        let wp = tiling_pos(rel).xz;
        let lap = 0.5 + 0.5 * sin(time_s() * 1.3 + value_noise2(wp * 0.7) * 6.0);
        foam += shore * lap * 0.6;
    }
    foam = saturate(foam * look.foam);
    if (foam > 0.001) {
        let wp = tiling_pos(rel).xz;
        var pattern = fbm2(wp * 2.3 + vec2<f32>(time_s() * 0.07, 0.0), 4);
        // Moving water draws its foam out into streaks along the current (blended in, so
        // a slowing current doesn't leave a seam where one pattern gives way to the other).
        let moving = smoothstep(0.02, 0.15, length(vel));
        if (moving > 0.0) {
            pattern = mix(pattern, streak_foam(wp, vel), moving);
        }
        // Foam is lace, not a sheet (Crest's coverage, HDRP's erosion): a broad noise picks
        // where it thins first; as it ages it opens along the walls between its bubble
        // clusters into a web of streaks, then holes, then scattered flecks.
        let broad = value_noise2(wp * 0.35 + vec2<f32>(7.3, 1.9));
        pattern = mix(pattern, foam_cells(wp, rel), 0.6);
        let theta = 1.0 - foam * (0.7 + 0.6 * broad);
        let cover = smoothstep(theta, theta + 0.15, pattern);
        let up = sky_ambient(n) * sky_vis * PI;
        // Foam is a dense froth: light gets through it, so it glows even with the sun behind.
        // Its albedo is well under snow's (whitecaps reflect at most about 55%, Whitlock 1982;
        // ours is brighter, fresh froth on fresh water), and thin lace lets the water through.
        let foam_col = pre_expose(0.75 * (sun * shadow * max(dot(n, l) * 0.6 + 0.4, 0.0) + up) * INV_PI);
        color = mix(color, foam_col, cover * mix(0.8, 0.95, saturate(foam * 2.0)));
    }
    let ground = textureSampleLevel(scene_color, linear_clamp, uv0, 0.0).rgb * wet_bed;
    // The last centimeters at the waterline are a film over the sand: no water color to speak
    // of, but it darkens the grains a little more and mirrors the sky and the sun like the
    // water does (a swash film's sheen), before the mesh's edge gives way to the ground.
    let film = mix(ground, ground * 0.85 + refl * fresnel + glitter, smoothstep(0.0, 0.35, shoreline));
    return mix(film, apply_fog(color, rel), shoreline);
}

// Where along its steps a ray march starts at pixel `px`, 0..1: interleaved gradient noise
// instead of banding, turned by the golden ratio every frame so TAA averages it away (a fixed
// pattern stays as a fine grid).
fn march_jitter(px: vec2<f32>) -> f32 {
    let ign = fract(52.9829189 * fract(dot(px, vec2<f32>(0.06711056, 0.00583715))));
    return fract(ign + frame.time.w * 0.6180339);
}

// What the underside of the surface at `rel` mirrors along `r` (heading down into the water):
// the screen's view that way where it shows something under the water (from just under the
// surface the mirror image is the view flipped about the horizon: a hull, a post, the bed),
// through the water the light travels; else the water's own light.
fn under_mirror(look: WaterLook, rel: vec3<f32>, r: vec3<f32>, jitter: f32) -> vec3<f32> {
    let c = frame.view_proj * vec4<f32>(rel + r * 50.0, 1.0);
    if (c.w > 1e-4) {
        let uv = vec2<f32>(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5);
        if (all(uv > vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0))) {
            let size = vec2<f32>(textureDimensions(scene_depth));
            let d = textureLoad(scene_depth, vec2<i32>(uv * size), 0);
            if (d > 1e-7) {
                let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
                let p4 = frame.inv_view_proj * vec4<f32>(ndc, d, 1.0);
                let p = p4.xyz / p4.w;
                if (p.y < water.g.under2.w) {
                    let path = length(rel) + length(p - rel);
                    let tr = exp(-(look.absorb + vec3<f32>(look.scattering)) * path);
                    let seen = textureSampleLevel(scene_color, linear_clamp, uv, 0.0).rgb;
                    return seen * tr + under_light(look, r, path, jitter);
                }
            }
        }
    }
    return under_light(look, r, 1e3, jitter);
}

// The surface seen from below: Snell's window to the world above, total internal reflection
// of the deep around it, all through the water between the camera and the surface.
fn shade_underside(rel: vec3<f32>, n_up: vec3<f32>, look: WaterLook, frag: vec4<f32>) -> vec3<f32> {
    let dist = length(rel);
    let v = -rel / max(dist, 1e-4);
    let n = -n_up;
    let cos_i = max(dot(n, v), 1e-3);
    let t = refract(-v, n, 1.333);
    let medium = in_water_radiance(look, -v, water.g.rain.w);
    // What the surface mirrors from below (all of it past the critical angle): the water under
    // it, lit as the view down into it is, and what stands in it.
    var mirrored = medium;
    if (water.g.rain.z >= 0.0) {
        let jitter = march_jitter(frag.xy);
        mirrored = under_mirror(look, rel, reflect(-v, n), jitter);
    }
    var color = mirrored;
    if (dot(t, t) > 0.0) {
        // Leaving the water: Fresnel from the water side.
        let f = 0.02 + 0.98 * pow(1.0 - cos_i, 5.0);
        let size = vec2<f32>(textureDimensions(scene_color));
        let nv = (frame.view * vec4<f32>(n_up, 0.0)).xyz;
        let uv = clamp(frag.xy / size + vec2<f32>(nv.x, -nv.y) * 0.03, vec2<f32>(0.0), vec2<f32>(0.999));
        let above = textureSampleLevel(scene_color, linear_clamp, uv, 0.0).rgb;
        let sky = pre_expose(sample_env(t));
        let world = mix(sky, above, 0.65);
        color = world * (1.0 - f) * 1.4 + mirrored * f;
    }
    let sigma_t = look.absorb + vec3<f32>(look.scattering);
    let tr = exp(-sigma_t * dist);
    return color * tr + medium * (1.0 - tr);
}

// The nearest two of a grid of jittered points (one per cell) to `q`: distances, the
// nearest's cell and the offset from it.
struct StoneCell {
    d1: f32,
    d2: f32,
    id: vec2<f32>,
    off: vec2<f32>,
};

fn stone_cell(q: vec2<f32>) -> StoneCell {
    let cell = floor(q);
    var out = StoneCell(9.0, 9.0, vec2<f32>(0.0), vec2<f32>(0.0));
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let c = cell + vec2<f32>(f32(i), f32(j));
            let o = vec2<f32>(hash12(c), hash12(c + vec2<f32>(19.7, 3.1))) * 0.8 + 0.1;
            let dv = q - c - o;
            let d = length(dv);
            if (d < out.d1) {
                out.d2 = out.d1;
                out.d1 = d;
                out.id = c;
                out.off = dv;
            } else if (d < out.d2) {
                out.d2 = d;
            }
        }
    }
    return out;
}

// One stone's wet color: granite greys and rusty ochres, slate, the odd white quartz, each a
// shade of its own; slow water films them green with algae.
fn stone_albedo(id: vec2<f32>, speed: f32) -> vec3<f32> {
    let h = hash12(id + vec2<f32>(3.3, 0.7));
    let h2 = hash12(id + vec2<f32>(7.1, 5.9));
    var albedo = mix(vec3<f32>(0.19, 0.18, 0.16), vec3<f32>(0.26, 0.20, 0.12), h);
    albedo = mix(albedo, vec3<f32>(0.09, 0.10, 0.11), step(0.7, h2));
    albedo = mix(albedo, vec3<f32>(0.45, 0.44, 0.41), step(0.95, h2));
    albedo *= 0.6 + 0.8 * hash12(id + vec2<f32>(1.1, 9.3));
    return albedo * mix(vec3<f32>(0.8, 0.95, 0.65), vec3<f32>(1.0), smoothstep(0.2, 0.8, speed));
}

// The bed of a stream seen through its water: rounded cobbles packed together with pebbles
// in the gaps between them, wet, lit by the sun and sky through the water (the caller adds
// the caustics and what the water absorbs). The current sorts them: it carries off whatever
// it can lift (Shields 1936), so fast water runs over coarse cobbles, slow water over finer
// ones. Each layer fades to its mean where a pixel spans several of its stones.
fn river_stones(rel: vec3<f32>, surface_rel: vec3<f32>, speed: f32) -> vec3<f32> {
    let wp = tiling_pos(rel).xz;
    // One fixed grid: scaling it by the current (which changes from place to place) would
    // smear the stones into contours. The current sorts them within it instead: big cobbles
    // packed tight where it runs fast, smaller ones with pebbles between where it is slow.
    let size = 0.45;
    let coarse = smoothstep(0.3, 1.8, speed);
    let pixel = pixel_footprint(rel);
    let up = vec3<f32>(0.0, 1.0, 0.0);
    let sun_in = -refract(-frame.sun_dir.xyz, up, 1.0 / 1.333);
    // Shadowed as the surface above it is: the bed itself lies under the shadow map's bias.
    let sun = water_sun(surface_rel) * max(frame.sun_dir.y, 0.0) * shadow_factor(surface_rel, up);
    let sky = sky_ambient(up) * sky_visibility(rel, up) * PI;

    // Cobbles: domes filling most of their cells.
    let c = stone_cell(wp / size);
    let radius = mix(0.22, 0.36, coarse) + 0.12 * hash12(c.id + vec2<f32>(5.5, 2.2));
    let r = c.d1 / radius;
    let on = 1.0 - smoothstep(0.85, 1.0, r);
    var slope = c.off / max(c.d1, 1e-4) * r / sqrt(max(1.0 - r * r, 0.05)) * 0.6;
    // Pebbles: smaller stones packed into the gaps, half in the cobbles' shade.
    let pq = wp / (size * 0.28) + vec2<f32>(31.7, 17.3);
    let pc = stone_cell(pq);
    let pr = saturate(pc.d1 / 0.42);
    let pebble_fade = smoothstep(0.25, 0.8, pixel / (size * 0.28));
    let p_slope = pc.off / max(pc.d1, 1e-4) * pr * 0.7 * (1.0 - pebble_fade);
    slope = mix(p_slope, slope, on);
    let p_alb = mix(stone_albedo(pc.id, speed) * mix(0.35, 1.0, smoothstep(1.0, 0.6, pr)), vec3<f32>(0.11, 0.10, 0.09), pebble_fade);
    var albedo = mix(p_alb * 0.7, stone_albedo(c.id, speed), on);
    let len = length(slope);
    if (len > 2.0) { slope *= 2.0 / len; }
    // Each stone's top tilts away from its middle.
    let n = normalize(vec3<f32>(slope.x, 1.0, slope.y));
    let grain = 0.85 + 0.3 * value_noise2(wp * 23.0);
    var lit = albedo * grain * INV_PI
        * (sun * max(dot(n, sun_in), 0.0) / max(sun_in.y, 0.2) + sky * (0.55 + 0.45 * n.y));
    // Far off, where a pixel spans several cobbles, their mean.
    let mean = vec3<f32>(0.15, 0.14, 0.12) * 0.85 * INV_PI * (sun + sky);
    lit = mix(lit, mean, smoothstep(0.3, 1.0, pixel / size));
    return pre_expose(lit);
}

// ---------------------------------------------------------------- lakes and oceans
struct BodyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) p0: vec2<f32>,
    @location(2) rest: vec2<f32>,
    @location(3) @interpolate(flat) body: u32,
    @location(4) jac: f32,
    @location(5) crest: f32,
};

@vertex
fn vs_body(@builtin(vertex_index) vid: u32, @builtin(instance_index) bi: u32) -> BodyOut {
    let segs = u32(water.bodies[bi].grid2.y);
    let ring = vid / segs;
    let seg = vid % segs;
    let s0 = water.bodies[bi].grid.z;
    let alpha = water.bodies[bi].grid.w;
    var r = s0 * f32(ring);
    if (alpha > 1e-6) { r = s0 * (exp(alpha * f32(ring)) - 1.0) / alpha; }
    let ang = (f32(seg) + 0.5 * f32(ring & 1u)) / f32(segs) * TAU;
    let rest = water.bodies[bi].grid.xy + vec2<f32>(cos(ang), sin(ang)) * r;
    let spacing = max(s0 * exp(alpha * f32(ring)), r * TAU / f32(segs));
    let depth = body_depth(bi, rest);
    let p0 = rest - water.bodies[bi].origin.xz;
    let w = eval_waves(bi, p0, depth, spacing * 2.5);
    var y = water.bodies[bi].origin.y + w.offset.y;
    var free = 0.0;
    if (water.bodies[bi].look.w > 0.5) {
        y += ripple_at(rest).x;
        // Up the beach the swash carries the water (and dives under the sand where it's dry).
        let sw = swash_at(rest);
        free = sw.w;
        y = mix(y, sw.level, free);
    }
    var out: BodyOut;
    out.rel = vec3<f32>(rest.x + w.offset.x, y, rest.y + w.offset.z);
    // The fine waves near the camera, where the grid is fine enough to hold them.
    let bd = water.bodies[bi].detail;
    out.rel.y += detail_height(tiling_pos(out.rel).xz, bd.x, bd.yz, spacing) * (1.0 - free);
    out.clip = frame.view_proj * vec4<f32>(out.rel, 1.0);
    out.p0 = p0;
    out.rest = rest;
    out.body = bi;
    out.jac = w.jacobian;
    out.crest = saturate(w.offset.y / max(water.bodies[bi].grid2.w, 1e-3) * 0.5 + 0.5);
    return out;
}

@fragment
fn fs_body(in: BodyOut) -> @location(0) vec4<f32> {
    // Whether the eye is under the surface here, from the triangles' own facing: exact where
    // the camera straddles the waterline (the surface on one side of the screen seen from
    // below, on the other from above).
    let ng = cross(dpdx(in.rel), dpdy(in.rel));
    let from_below = dot(in.rel, ng * sign(ng.y)) > 0.0;
    let bi = in.body;
    // Under a lake mirror's plane, in its view (`clip_plane_distance` is 0 elsewhere).
    // Past the lake's edge only where the swash has run up the beach.
    var sw = Swash(0.0, 0.0, 0.0, 0.0);
    if (water.bodies[bi].look.w > 0.5) { sw = swash_at(in.rest); }
    let up_the_beach = sw.w > 0.5 && sw.depth > 5e-4;
    if ((body_inset(bi, body_local(bi, in.rest)) < 0.0 && !up_the_beach) || clip_plane_distance(in.rel) < 0.0) { discard; }
    // At the waterline the lens's dome decides which side each pixel sees from (`fs_under`):
    // the surface closer than the dome would cut across that.
    let dome = water.g.waterline.y;
    if (water.g.waterline.x > 0.5 && dot(in.rel, in.rel) < dome * dome) { discard; }
    use_mirror = i32(bi) == i32(water.g.mirror.w);
    wet_bed = mix(1.0, WET_SAND, sw.w);
    let look = body_look(bi);
    let dist = length(in.rel);
    // Size of a pixel here, meters: drop detail finer than that.
    let pixel = pixel_footprint(in.rel);
    let depth = body_depth(bi, in.rest);
    let w = eval_waves(bi, in.p0, depth, pixel * 3.0);
    let wind = water.bodies[bi].bed.z;
    let wp = tiling_pos(in.rel).xz;
    let wind_dir = normalize(water.bodies[bi].wind.xy + vec2<f32>(1e-4, 0.0));
    // How much wind reaches the water here: the lee of the banks and the crag lies calm, and
    // the breeze touches down there only in its strongest gusts (cat's paws). Where it stays
    // under about 4 m/s, slicks smooth the short waves in bands. Only the wind's own waves
    // calm down: wakes, rings and swell come through.
    let exposure = shelter_at(bi, in.rel);
    let g0 = gusts(wp, wind_dir, wind);
    let touch = smoothstep(1.15, 1.6, g0);
    let reach = max(exposure, touch * 0.7);
    let sheltering = water.g.shelter.w > -1.5;
    let film = select(0.0, slicks(wp, wind_dir, wind * reach), sheltering);
    let calm = select(1.0, reach * (1.0 - 0.85 * film), sheltering);
    let tilt = mix(0.2, 1.0, calm);
    var n = normalize(vec3<f32>(w.normal.x * tilt, w.normal.y, w.normal.z * tilt));
    var foam = 0.0;
    var vel = water.bodies[bi].wind.zw;
    var bubbles = 0.0;
    if (water.bodies[bi].look.w > 0.5) {
        let rp = ripple_at(in.rest);
        n = normalize(n + vec3<f32>(-rp.z, 0.0, -rp.w));
        foam += rp.y * 1.6;
        // Foam rides the stirred currents: its streaks follow their swirls.
        let cur = current_at(in.rest);
        vel += cur.xy;
        bubbles = cur.w;
        let trail = trail_at(in.rest);
        foam += trail.x * 1.6;
        bubbles += trail.y;
    }
    // The fine waves, roughened in gusts, calmed in the lee and the slicks.
    let bd = water.bodies[bi].detail;
    let fine = detail_at(wp, bd.x, bd.yz, footprint(in.rel));
    let gust = g0 * calm;
    // Whitecaps where the waves pinch (the long waves and the fine ones riding them), and the
    // foam the fine ones leave as they break, thinning.
    let jac = in.jac - fine.squeeze * gust;
    foam += saturate((0.55 - jac) * 2.5) * saturate((wind * reach - 4.0) * 0.25);
    foam += fine.foam * saturate(bd.x) * saturate((wind * reach - 5.0) * 0.2) * 0.6;
    // In a strong wind the crests break all over, in patches too small and quick for the waves
    // above to pinch: about 3.84e-6·U^3.41 of the sea is white (Monahan), the breakers drawn
    // out downwind. The wind drags the surface along at about 3% of its speed, so foam streaks
    // run with it.
    let caps = 3.84e-6 * pow(max(wind, 0.0), 3.41);
    if (caps > 0.002) {
        let across = vec2<f32>(-wind_dir.y, wind_dir.x);
        let q = vec2<f32>(dot(wp, wind_dir) * 0.35, dot(wp, across)) - vec2<f32>(time_s() * 0.4, 0.0);
        let breaking = value_noise2(q * 1.7) * 0.6 + value_noise2(q * 4.1 + vec2<f32>(3.1, 7.7)) * 0.4;
        // The noise rarely tops 0.9: a threshold of 1 − 1.5·√caps whitens about `caps` of it.
        let edge = 1.0 - min(1.5 * sqrt(caps), 0.45);
        // A breaker is froth, not a sheet: fine noise tears holes in it.
        let lace = value_noise2(wp * 5.3 + vec2<f32>(time_s() * 0.9, 0.0)) * 0.6
            + value_noise2(wp * 13.0 + vec2<f32>(1.7, time_s() * 1.3)) * 0.4;
        // Waves break at their crests: the white gathers there, thin in the troughs.
        let on_crest = 0.3 + 0.7 * smoothstep(0.45, 0.8, in.crest + fine.squeeze);
        foam += smoothstep(edge, edge + 0.15, breaking) * smoothstep(0.3, 0.6, lace) * on_crest;
        // From a near gale on, the foam the breakers leave is drawn into rows along the wind.
        let rows = value_noise2(vec2<f32>(dot(wp, wind_dir) * 0.06, dot(wp, across) * 1.1));
        foam += smoothstep(0.6, 0.8, rows) * 0.35 * smoothstep(10.0, 16.0, wind);
        vel += wind_dir * wind * 0.03;
    }
    // A boat's churned track drains the fine ripples (its turbulence and the films its bubbles
    // bring up), so it lies glassy for a while after the white has gone: the dark slick wake
    // seen from the air and on radar (Peltzer et al. 1992, Milgram et al. 1993).
    let slick = 1.0 - 0.35 * smoothstep(0.05, 0.5, bubbles) * (1.0 - saturate(foam * 3.0));
    var g = fine.grad * gust * slick;
    // What the pixel can't resolve roughens it: waves faded for being too short to draw, fine
    // waves averaged away by the mips, and ripples under 2 cm.
    let slope_var = w.slope_var * tilt * tilt + (fine.variance * gust * gust + bd.w * calm * calm) * slick * slick;
    g += rain_gradient(wp, rain_at(in.rest), saturate(1.0 - pixel / 0.02));
    let boil = plunge_boil(in.rel);
    foam += boil.x;
    bubbles += boil.x * 0.8;
    g += boil.yz;
    vel += plunge_flow(in.rel);
    n = normalize(n + vec3<f32>(-g.x, 0.0, -g.y));
    // The swash: its own slope where it runs free (the lake's waves have become it), and the
    // foam its bores and run-up tips make.
    if (sw.w > 0.0) {
        let ss = swash_slope(in.rest);
        // The wind's fine ripples ride it too, flattening out only in the last few centimeters.
        let keep = mix(0.3, 1.0, saturate(sw.depth / 0.15));
        let n_swash = normalize(vec3<f32>(-ss.x - g.x * keep, 1.0, -ss.y - g.y * keep));
        n = normalize(mix(n, n_swash, sw.w));
        foam += sw.foam * sw.w * 1.4;
    }
    var color: vec3<f32>;
    // Seen from below when the camera is under the water (at the waterline, where it is).
    if (water.g.rain.z >= 0.0 && (water.g.waterline.x < 0.5 || from_below)) {
        color = shade_underside(in.rel, n, look, in.clip);
    } else {
        // Where the swash runs, the shore's procedural lapping gives way to it.
        color = shade_water(in.rel, n, look, foam, in.crest, in.clip, wind, 0.05, 0.3 * (1.0 - sw.w), vel, slope_var, bubbles);
    }
    return vec4<f32>(color, 1.0);
}

// ---------------------------------------------------------------- flowing water
struct FlowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) depth: f32,
    @location(2) vel: vec2<f32>,
    @location(3) normal: vec3<f32>,
    // How fast the current turns here (1/s): eddy lines behind stones and along the banks.
    @location(4) shear: f32,
};

fn flow_load(i: i32, j: i32) -> vec4<f32> {
    let nx = i32(flow.dims.x);
    let nz = i32(flow.dims.y);
    return textureLoad(flow_tex, vec2<i32>(clamp(i, 0, nx - 1), clamp(j, 0, nz - 1)), 0);
}

@vertex
fn vs_flow(@builtin(vertex_index) vid: u32) -> FlowOut {
    let nx = u32(flow.dims.x);
    let i = i32(vid % nx);
    let j = i32(vid / nx);
    let s = flow_load(i, j);
    let cell = flow.origin.w;
    // Dry points were placed on the CPU: continuing the surface into the bank, or under it.
    let y = s.r;
    // Normal from the neighbors' surfaces (dry neighbors count as level with us).
    let l = flow_load(i - 1, j);
    let r = flow_load(i + 1, j);
    let b = flow_load(i, j - 1);
    let t = flow_load(i, j + 1);
    let hl = select(s.r, l.r, l.g > 0.003);
    let hr = select(s.r, r.r, r.g > 0.003);
    let hb = select(s.r, b.r, b.g > 0.003);
    let ht = select(s.r, t.r, t.g > 0.003);
    // Grid axes to world axes: the grid turns with its entity's yaw.
    let c = flow.dims.z;
    let sn = flow.dims.w;
    let turn = mat2x2<f32>(vec2<f32>(c, -sn), vec2<f32>(sn, c));
    var out: FlowOut;
    let gn = vec3<f32>(hl - hr, 2.0 * cell, hb - ht);
    out.normal = normalize(vec3<f32>(turn * gn.xz, gn.y).xzy);
    let along = turn * ((vec2<f32>(f32(i), f32(j)) + 0.5) * cell);
    out.rel = vec3<f32>(flow.origin.x + along.x, flow.origin.y + y, flow.origin.z + along.y);
    out.clip = frame.view_proj * vec4<f32>(out.rel, 1.0);
    out.depth = s.g;
    out.vel = s.ba;
    // The current's curl, from the wet neighbors (dry ones hold no current to shear against).
    let vl = select(s.ba, l.ba, l.g > 0.003);
    let vr = select(s.ba, r.ba, r.g > 0.003);
    let vb = select(s.ba, b.ba, b.g > 0.003);
    let vt = select(s.ba, t.ba, t.g > 0.003);
    out.shear = ((vr.y - vl.y) - (vt.x - vb.x)) / (2.0 * cell);
    return out;
}

// fbm stretched along the current (`stretch`: its scale along and across) and carried down it
// at a steady 1.2 m/s (at the current's own speed, which differs from place to place, the
// pattern would shear apart more every second).
// The pattern is drawn in two fixed directions, the eighths of a turn either side of the
// current's, and cross-faded: turned to the current itself (which turns from place to place,
// while the world position runs to hundreds of meters) it would smear into zigzags.
fn flow_streaks(wp: vec2<f32>, vel: vec2<f32>, stretch: vec2<f32>, t: f32) -> f32 {
    let turns = atan2(vel.y, vel.x) / (PI / 4.0);
    let k = floor(turns);
    var sum = 0.0;
    for (var i = 0; i < 2; i++) {
        // Each direction's own pattern (the same one either side of the wrap at ±π).
        let bin = (k + f32(i)) - 8.0 * floor((k + f32(i)) / 8.0);
        let a = bin * (PI / 4.0);
        let along = vec2<f32>(cos(a), sin(a));
        let q = vec2<f32>(dot(wp, along), dot(wp, vec2<f32>(-along.y, along.x))) * stretch;
        let w = select(1.0 - (turns - k), turns - k, i == 1);
        sum += w * fbm2(q - vec2<f32>(t * 1.2 * stretch.x, 0.0) + bin * 7.3, 3);
    }
    return sum;
}

@fragment
fn fs_flow(in: FlowOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (clip_plane_distance(in.rel) < 0.0) { discard; }
    // Where the surface drops more than its width in one cell, and at least half a meter
    // (`FALL_SLOPE`, `FALL_LEAST_DROP`), the water falls free: the curtain (`fs_falls`) draws
    // it, not the grid's steep triangles.
    let face = normalize(cross(dpdx(in.rel), dpdy(in.rel)));
    let tan_fall = max(1.0, 0.5 / flow.origin.w);
    if (abs(face.y) < inverseSqrt(1.0 + tan_fall * tan_fall) * 1.02) { discard; }
    // Triangles touching water are drawn whole: the terrain hides the parts under it, so
    // the waterline follows the ground rather than the grid.
    if (in.depth < 0.0005) { discard; }
    let look = flow_look();
    let pixel = pixel_footprint(in.rel);
    let speed = length(in.vel);
    stream_bed = 1.0;
    stream_speed = speed;
    stream_depth = in.depth;
    let wp = tiling_pos(in.rel).xz;
    // Flow-mapped ripples: two copies of the pattern scrolled downstream, cross-faded.
    let t = time_s();
    let phase0 = fract(t * 0.6);
    let phase1 = fract(t * 0.6 + 0.5);
    let shift = in.vel * 1.6 * smoothstep(0.05, 0.3, speed);
    let fp = footprint(in.rel);
    // The ripples' directions follow the wind, not the local current: a direction that turns
    // from place to place would wind the pattern up into fine moiré far from the camera.
    let wind_dir = normalize(frame.wind.xz + vec2<f32>(1e-4, 0.0));
    // The fine waves' tiles turned to the wind here, carried downstream.
    let tiles = water.g.detail2.xy;
    let turn = vec2<f32>(dot(tiles, wind_dir), tiles.x * wind_dir.y - tiles.y * wind_dir.x);
    // A stream's surface rolls in boils and bulges bigger than a breeze's ripples: the tiles
    // drawn 2.5 times larger (slopes kept), livelier where the water runs fast, and patchy.
    let big = 2.5;
    var fp_big = fp;
    fp_big.across = fp.across / big;
    let lively = (1.0 + speed * 0.5) * gusts(wp - shift * 0.5, wind_dir, 3.0 + speed);
    let d0 = detail_at((wp - shift * phase0) / big, water.g.detail.z, turn, fp_big);
    let d1 = detail_at((wp - shift * phase1) / big + vec2<f32>(4.7, 1.3), water.g.detail.z, turn, fp_big);
    let wgt = abs(phase0 - 0.5) * 2.0;
    var g = mix(d0.grad, d1.grad, wgt) * lively;
    // The current livens what can be seen; what a pixel can't is the breeze's alone, or
    // fast water would go milky with roughness.
    let slope_var = mix(d0.variance, d1.variance, wgt);
    g += rain_gradient(wp, rain_at(in.rel.xz), saturate(1.0 - pixel / 0.02));
    // Rings, wakes and foam from whatever moves through the stream (the interactive field
    // covers streams near the camera too).
    let rp = ripple_at(in.rel.xz);
    g += rp.zw;
    let boil = plunge_boil(in.rel);
    g += boil.yz;
    // Fast shallow water heaps up into standing waves over the stones of its bed, and breaks
    // white on their crests: chop big enough to see from afar, so rapids never mirror the sky
    // as a smooth sheet. The waves stand where the stones are, trembling.
    let rapid = smoothstep(0.8, 2.6, speed) * saturate(in.depth * 20.0);
    var crests = 0.0;
    if (rapid > 0.0) {
        // In fixed world axes: turned to the local current (which turns from place to place,
        // while the world position runs to hundreds of meters) the pattern smeared into zigzags.
        let q = wp * 1.3 + vec2<f32>(sin(t * 3.1), cos(t * 2.3)) * 0.05;
        let e = 0.05;
        let h = fbm2(q, 3) / 0.875;
        let dh = vec2<f32>(fbm2(q + vec2<f32>(e, 0.0), 3), fbm2(q + vec2<f32>(0.0, e), 3)) / 0.875 - h;
        let slope = dh * 1.3 / e;
        g += slope * 0.08 * rapid * (1.0 + speed * 0.15);
        crests = smoothstep(0.55, 0.75, h) * rapid;
    }
    let n = normalize(in.normal + vec3<f32>(-g.x, 0.0, -g.y));
    // White water: fast, steep or tumbling. Fast water breaks into streaks and patches of
    // white over its riffles, never a white sheet.
    let steep = 1.0 - in.normal.y;
    let churn = saturate((speed - 1.8) * 0.25) * 0.5 + saturate(steep * 2.5 - 0.3) * 1.3;
    // Even tumbling water is never one white sheet: it rolls down in lumps of froth with
    // darker, glassier water between them, all carried along with the current.
    let l0 = fbm2((wp - shift * phase0) * 1.3, 3);
    let l1 = fbm2((wp - shift * phase1) * 1.3 + vec2<f32>(2.9, 6.1), 3);
    let lumps = smoothstep(0.3, 0.65, mix(l0, l1, wgt) / 0.875);
    // (`shade_water` covers nearly everything in foam from about 0.6: the white water stays
    // under that, so the dark glassy water between its lumps shows.)
    var foam = min(min(churn, 1.0) * mix(0.0, 0.55, lumps) + crests * 0.25 * (1.0 - lumps), 0.55);
    // Where the slack water behind a stone meets the current, the eddy line gathers the foam
    // the riffles above it made into a line trailing downstream, torn into flecks.
    let flecks = smoothstep(0.45, 0.7, flow_streaks(wp, in.vel, vec2<f32>(0.7, 6.0), t) / 0.875);
    foam += smoothstep(0.8, 2.5, abs(in.shear)) * smoothstep(0.2, 0.6, speed) * flecks * 0.45;
    foam *= saturate(in.depth * 30.0);
    // Only a pool boils where a fall lands: on running water the current sweeps the froth
    // downstream in the lumps above.
    foam += rp.y * 1.2 + boil.x * (1.0 - smoothstep(0.3, 0.8, speed));
    let lap = 0.5 + 0.5 * sin(t * 1.3 + value_noise2(wp * 0.7) * 6.0);
    foam += (1.0 - smoothstep(0.002, 0.01, in.depth)) * lap * 0.5;
    var color: vec3<f32>;
    if (front) {
        // Streams run shallow over riffles: only the last centimeter fades. Their banks lap
        // white by the stream's own depth (above), not the depth seen through it: a sheet a
        // centimeter or two deep over a slab looks like shoreline all over from here.
        color = shade_water(in.rel, n, look, foam, saturate(steep * 2.0), in.clip, 3.0 + speed, 0.01, 0.0, in.vel, slope_var, 0.0);
    } else {
        color = shade_underside(in.rel, n, look, in.clip);
    }
    return vec4<f32>(color, 1.0);
}

// ---------------------------------------------------------------- falling water
// A curtain arcs out over each lip (`FlowSim::lips`) on its free-fall path down to the water or
// ground below, sliding down any rock face it meets on the way (`FlowSim::fall_path`). It leaves the lip as a glassy sheet and breaks up into aerated white streaks as
// it speeds up; the streak pattern rides down with the water, so it stretches as it falls.
struct FallIn {
    @location(0) rel_q: vec4<f32>,       // camera-relative position, m²/s over the lip
    @location(1) normal_frac: vec4<f32>, // outward normal, 0 at the lip .. 1 at the foot
    @location(2) across_side: vec4<f32>, // direction across the curtain, strip edge (-1, 1)
    @location(3) uv_speed: vec4<f32>,    // meters across, seconds since the lip, lip speed
    @location(4) edge: vec4<f32>,        // x 1 on a side where the fall ends (it frays there)
};

struct FallOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) across: vec3<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) look: vec4<f32>,        // q, frac, side, lip speed
    @location(5) kind: f32,              // 1 spray cloud; -1 spray round a curtain; -2 wet rock under it; under a half a curtain, 0.45 where it slides down rock
    @location(6) edge: f32,              // 1 toward a side where the fall ends
};

@vertex
fn vs_falls(v: FallIn) -> FallOut {
    var out: FallOut;
    out.rel = v.rel_q.xyz;
    out.clip = frame.view_proj * vec4<f32>(out.rel, 1.0);
    out.normal = v.normal_frac.xyz;
    out.across = v.across_side.xyz;
    out.uv = v.uv_speed.xy;
    out.look = vec4<f32>(v.rel_q.w, v.normal_frac.w, v.across_side.w, v.uv_speed.z);
    out.kind = v.uv_speed.w;
    out.edge = v.edge.x;
    return out;
}

// How far in from a fall's ragged outer edge a point is, from `side` (-1..1 across its strip)
// and `edge` (1 toward a side where the fall ends): where neighboring strips carry the sheet
// on, |side| itself; at the fall's ends the edge wanders in and out down the rock or the
// fall (fixed in the world: the brink and the rock shape it), so a fall isn't a ruled
// rectangle. Over 1 is past the edge.
fn fall_side(side: f32, edge: f32, rel: vec3<f32>, u: f32) -> f32 {
    let w = tiling_pos(rel);
    let wander = fbm2(vec2<f32>(w.y * 0.55 + u * 0.13, u * 0.31 + 5.1), 3) / 0.875;
    return abs(side) + edge * (wander - 0.4) * 0.8;
}

// Rock a sheet slides down, and the band round it the flow and its splashes keep wet: darker
// (the water fills its pores, Lagarde 2013) and with a film on it that mirrors the sky at a
// glancing look and glints where the water runs over a bump. Drawn under the sheet,
// darkening what is behind (premultiplied: the color added, alpha the darkening).
fn fall_wet(in: FallOut) -> vec4<f32> {
    let w = tiling_pos(in.rel);
    let side = fall_side(in.look.z, in.edge, in.rel, in.uv.x);
    // Ragged at its rim, with streaks of wetter rock where rills run down.
    let rim = fbm2(vec2<f32>(in.uv.x * 0.9, w.y * 0.7), 3) / 0.875;
    let cover = 1.0 - smoothstep(0.45 + 0.25 * rim, 1.0, side);
    if (cover <= 0.01) { discard; }
    let rills = value_noise2(vec2<f32>(in.uv.x * 2.7 + 3.3, w.y * 0.12));
    let wetness = cover * (0.75 + 0.25 * rills);
    // A decal on the rock: only what lies just behind the strip (the rock it stands off) gets
    // wet, never the far side of the gorge seen past an edge.
    let size = vec2<f32>(textureDimensions(scene_color));
    let pix = vec2<i32>(clamp(in.clip.xy, vec2<f32>(0.0), size - 1.0));
    let gap = linear_depth(textureLoad(scene_depth, pix, 0)) - max(view_depth(in.rel), 1e-3);
    let near = 1.0 - smoothstep(0.7, 1.5, gap);
    let alpha = wetness * 0.45 * near;
    if (alpha < 0.004) { discard; }
    let v = -in.rel / max(length(in.rel), 1e-4);
    var n = in.normal;
    // The film ripples as it runs down.
    let t0 = time_s() - in.uv.y;
    let ripple = value_noise2(vec2<f32>(in.uv.x * 6.0, t0 * 9.0)) - 0.5;
    let along = cross(n, in.across);
    n = normalize(n + along * ripple * 0.35 * wetness);
    let n_dot_v = max(dot(n, v), 1e-3);
    let fresnel = 0.02 + 0.98 * pow(1.0 - n_dot_v, 5.0);
    let sky_vis = sky_visibility(in.rel, n);
    let fill = frame.ambient.rgb;
    let refl = pre_expose((sample_env(reflect(-v, n)) - fill) * sky_vis + fill);
    let l = frame.sun_dir.xyz;
    let h = normalize(l + v);
    let n_dot_l = saturate(dot(n, l));
    let a = 0.08 * 0.08;
    let spec = d_ggx(saturate(dot(n, h)), a) * v_smith_ggx_correlated(n_dot_v, max(n_dot_l, 1e-3), a)
        * f_schlick(vec3<f32>(0.02), saturate(dot(l, h))) * n_dot_l;
    let sun = water_sun(in.rel) * shadow_factor(in.rel, n);
    let color = (refl * fresnel + pre_expose(spec * sun)) * wetness * near;
    // The rock behind is fogged already (rock · T + in-scatter): darken only the rock, and
    // fog the film's light as the rock's.
    let fog = surface_fog(in.rel);
    return vec4<f32>(color * fog.transmittance + fog.inscatter * alpha, alpha);
}

// The cloud of spray where a fall lands: a camera-facing puff (corner -1..1 in `normal.xy`,
// radius `normal.z`, density `look.y`, a seed in `uv.x`) billowing up from the pool.
fn fall_mist(in: FallOut) -> vec4<f32> {
    let c = in.normal.xy;
    let r = length(c);
    if (r >= 1.0) { discard; }
    let t = time_s();
    let p = c * 1.6 + vec2<f32>(in.uv.x, 0.0);
    let billow = fbm2(p + vec2<f32>(0.0, -t * 0.45), 4) / 0.9375;
    let wisps = fbm2(p * 2.3 + vec2<f32>(5.3, -t * 0.9), 3) / 0.875;
    // Densest where it boils off the pool, thinning out as it rises and spreads.
    let shape = (1.0 - smoothstep(0.1, 1.0, r)) * (1.0 - 0.8 * smoothstep(-0.1, 0.9, c.y));
    var alpha = shape * smoothstep(0.15, 0.75, billow * 0.65 + wisps * 0.35) * in.look.y;
    // Soft against the rock behind, the pool and the bank.
    let size = vec2<f32>(textureDimensions(scene_color));
    let pix = vec2<i32>(clamp(in.clip.xy, vec2<f32>(0.0), size - 1.0));
    let z_scene = linear_depth(textureLoad(scene_depth, pix, 0));
    alpha *= saturate((z_scene - max(view_depth(in.rel), 1e-3)) / (in.normal.z * 0.35));
    if (alpha < 0.003) { discard; }
    // Fine droplets: bright toward the sun, lit by the open sky in front of the cliff.
    let v = -in.rel / max(length(in.rel), 1e-4);
    let l = frame.sun_dir.xyz;
    let sun = water_sun(in.rel) * shadow_factor(in.rel, l);
    let phase = 0.3 + 1.6 * pow(saturate(dot(-v, l)), 6.0);
    let out_dir = normalize(vec3<f32>(v.x, 0.0, v.z) + vec3<f32>(0.0, 1e-3, 0.0));
    let n = normalize(out_dir + vec3<f32>(0.0, 0.7, 0.0));
    // A dense cloud scatters many times over: the blue of the sky light washes out to grey.
    let sky_in = sky_ambient(n) * sky_visibility(in.rel, n) * PI;
    let sky = mix(sky_in, vec3<f32>(dot(sky_in, vec3<f32>(0.2126, 0.7152, 0.0722))), 0.6);
    let color = apply_fog(pre_expose(0.9 * (sun * phase + sky) * INV_PI), in.rel);
    return vec4<f32>(color * alpha, alpha);
}

// Streaks of the curtain at `u` meters across for water that left the lip at time `t0`:
// fingers about a meter apart that come and go as the flow over the lip pulses, with finer
// threads in them (`fine` fades those out where a pixel is wider than a thread), 0..1.
fn fall_streaks(u: f32, t0: f32, fine: f32) -> f32 {
    let broad = fbm2(vec2<f32>(u * 1.6, t0 * 2.2), 3) / 0.875;
    let mid = value_noise2(vec2<f32>(u * 5.0 + 11.3, t0 * 5.0));
    let thread = value_noise2(vec2<f32>(u * 17.0 + 3.7, t0 * 11.0));
    let raw = broad * 0.6 + mid * 0.4;
    return smoothstep(0.25, 0.75, mix(raw, raw * 0.7 + thread * 0.3, fine));
}

// How broken up a free-falling jet of `q` m²/s is, `tau` seconds after it left the brink:
// compact and glassy at first, broken into clumps and spray past its break-up length
// L_b = 6 q^0.32 (Horeni 1956; rectangular jets break up at 0.5-3 L_b, Castillo et al.), 0..1.
fn fall_broken(q: f32, tau: f32) -> f32 {
    let drop = 0.5 * 9.81 * tau * tau;
    let lb = 6.0 * pow(max(q, 1e-4), 0.32);
    return smoothstep(0.3, 1.4, drop / lb);
}

// Aerated water lit like froth: the sun wrapping round it and glowing through it with the
// sun behind, and the sky over it.
fn froth_light(rel: vec3<f32>, n: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    let l = frame.sun_dir.xyz;
    let sun = water_sun(rel);
    let shadow = shadow_factor(rel, n);
    // Light bounces many times inside froth: the sky's blue washes out toward grey-white.
    let sky = sky_ambient(n) * sky_visibility(rel, n) * PI;
    let up = mix(sky, vec3<f32>(dot(sky, vec3<f32>(0.2126, 0.7152, 0.0722))), 0.5);
    let wrap = saturate(dot(n, l) * 0.6 + 0.4);
    let through = pow(saturate(dot(-v, l)), 4.0) * 0.8;
    return pre_expose(0.9 * (sun * shadow * (wrap + through) + up) * INV_PI);
}

// The aerated water and spray round a falling core: none over the brink, then, as the jet
// breaks up, white clumps streaming down with it, frayed into drops at their edges, and a
// haze of fine spray between them that thickens toward the foot. Seen edge-on it is
// thicker (the look passes through more of it).
fn fall_shell(in: FallOut) -> vec4<f32> {
    let q = in.look.x;
    let frac = in.look.y;
    let tau = in.uv.y;
    let u = in.uv.x;
    let v = -in.rel / max(length(in.rel), 1e-4);
    let broken = fall_broken(q, tau);
    if (broken < 0.01) { discard; }
    let t0 = time_s() - tau;
    let fine = saturate(1.5 - footprint(in.rel).across * 40.0);
    let body = saturate(q * 6.0 + 0.2);
    // Clumps stretch as the water speeds up: the pattern rides with it (time since the lip).
    let clumps = fbm2(vec2<f32>(u * 1.9, t0 * 4.2), 4) / 0.9375;
    let drops = value_noise2(vec2<f32>(u * 21.0 + 4.1, t0 * 27.0));
    // Fine spray hangs thickest low down, where the clumps have shed most of it.
    let haze = 0.05 + 0.3 * frac * frac;
    var alpha = smoothstep(0.42, 0.78, clumps) * 0.7 + haze * smoothstep(0.2, 0.6, clumps);
    alpha = max(alpha, fine * 0.35 * smoothstep(0.6, 0.95, drops) * smoothstep(0.3, 0.6, clumps));
    let n_dot_v = abs(dot(in.normal, v));
    alpha *= broken * body * mix(1.5, 0.75, n_dot_v);
    // Frayed at the sides, ragged where the fall ends.
    alpha *= 1.0 - smoothstep(0.3, 1.0, fall_side(in.look.z, in.edge, in.rel, u) + (clumps - 0.5) * 0.6);
    // Soft against the rock behind and the pool below.
    let size = vec2<f32>(textureDimensions(scene_color));
    let pix = vec2<i32>(clamp(in.clip.xy, vec2<f32>(0.0), size - 1.0));
    let z_scene = linear_depth(textureLoad(scene_depth, pix, 0));
    alpha *= saturate((z_scene - max(view_depth(in.rel), 1e-3)) / 0.6);
    alpha = min(alpha, 0.92);
    if (alpha < 0.004) { discard; }
    var n = in.normal;
    if (dot(n, v) < 0.0) { n = -n; }
    let color = apply_fog(froth_light(in.rel, n, v) * (0.75 + 0.35 * clumps), in.rel);
    return vec4<f32>(color * alpha, alpha);
}

@fragment
fn fs_falls(in: FallOut) -> @location(0) vec4<f32> {
    if (in.kind > 0.5) { return fall_mist(in); }
    if (in.kind < -1.5) { return fall_wet(in); }
    if (in.kind < -0.5) { return fall_shell(in); }
    // No lip is level: the water gathers into tongues where the rock dips and runs thin
    // between them (the pattern is fixed to the rock, `u` being meters across in the world).
    let gather = value_noise2(vec2<f32>(in.uv.x * 0.45, 3.7)) * 0.65
        + value_noise2(vec2<f32>(in.uv.x * 1.3, 9.1)) * 0.35;
    let tongue = smoothstep(0.3, 0.7, gather);
    let q = in.look.x * mix(0.4, 1.5, tongue);
    let frac = in.look.y;
    let side = in.look.z;
    let tau = in.uv.y;
    let u = in.uv.x;
    let dist = length(in.rel);
    let v = -in.rel / max(dist, 1e-4);
    // The pattern belongs to the water: what is here now left the lip `tau` seconds ago.
    let t0 = time_s() - tau;
    let fine = saturate(1.5 - footprint(in.rel).across * 40.0);
    let s = fall_streaks(u, t0, fine);
    let s_du = fall_streaks(u + 0.03, t0, fine);
    let s_dt = fall_streaks(u, t0 + 0.02, fine);
    // Glassy as it rolls over the lip, white with air a few tenths of a second later. From
    // behind or below, the churned water under the tongue shows.
    let front = dot(in.normal, v) >= 0.0;
    // Past its break-up length the core is no longer a sheet but clumps of white water.
    let broken = fall_broken(in.look.x, max(tau, 0.0));
    var aerated = max(smoothstep(0.03, 0.22, tau + (s - 0.5) * 0.1), select(0.6, 0.0, front));
    aerated = max(aerated, broken);
    // Sliding down rock (`ROCK_KIND`), the sheet tumbles over every bump: white in its
    // streaks, glassier between them.
    let rock = saturate(in.kind / 0.45);
    aerated = max(aerated, rock * mix(0.45, 0.9, s));

    // Coverage: a thick flow stays a closed sheet; lower down and at its edges it frays into
    // fingers with the rock showing between them, and thin threads are see-through.
    let body = saturate(q * 8.0 + 0.15);
    // Over rock the sheet thins into a lace of rivulets from the start, the wet rock showing
    // dark between them.
    let gaps = max(smoothstep(0.1, 1.0, frac) * 0.45 * (1.2 - body * 0.4), rock * (0.3 + 0.15 * frac));
    // Falling free, the fingers tear into packets of water lower down (the pattern rides with
    // the water, so they stretch as it speeds up), each fraying at its edges into spray, and a
    // thin haze of drops hangs between them. A thick flow and a sheet sliding down rock stay
    // whole.
    // (From afar the torn packets blur together with the spray: the fall reads whole.)
    let tear = max(smoothstep(0.15, 0.9, frac) * 0.5, broken) * (1.0 - rock * 0.7) * mix(0.35, 1.0, fine);
    let packets = fbm2(vec2<f32>(u * 3.1 + 7.7, t0 * 7.0), 3) / 0.875;
    let edge = 0.2 + 0.3 * tear;
    var alpha = smoothstep(gaps, gaps + edge, s - (1.0 - packets) * 0.35 * tear * (1.3 - body * 0.7)) * body
        * (0.7 + 0.3 * smoothstep(0.2, 0.7, s));
    let drops = value_noise2(vec2<f32>(u * 23.0 + 1.9, t0 * 31.0));
    alpha = max(alpha, tear * body * 0.3 * smoothstep(0.35, 0.95, drops) * smoothstep(0.1, 0.5, s));
    // From afar those drops blur into a veil across the gaps.
    alpha = max(alpha, (1.0 - fine) * body * 0.4 * smoothstep(0.15, 0.9, frac) * smoothstep(0.0, 0.35, s));
    // Ragged where the fall ends at its sides (threads of it stray past that edge), and
    // fading in where the water comes up to the brink.
    let edge_x = fall_side(side, in.edge, in.rel, u);
    var keep = (1.0 - smoothstep(0.6, 1.0, edge_x + (s - 0.5) * 0.4))
        * (1.0 - smoothstep(0.08, 0.3, -tau * in.look.w));
    alpha *= keep;
    // Sheets from neighboring lips overlap where they spread over rock: each is a thin film
    // there, so together they still read as one lacy veil and not an opaque white slab.
    alpha *= mix(1.0, 0.55, rock);
    // Between the tongues only the strongest streaks pour, as threads.
    alpha *= mix(smoothstep(0.55, 0.85, s) * 0.85, 1.0, tongue);
    // Soft where it meets the rock behind it and the pool it lands in.
    let size = vec2<f32>(textureDimensions(scene_color));
    let pix = vec2<i32>(clamp(in.clip.xy, vec2<f32>(0.0), size - 1.0));
    let z_scene = linear_depth(textureLoad(scene_depth, pix, 0));
    let z_frag = max(view_depth(in.rel), 1e-3);
    // (A sheet on the rock lies a few centimeters off it: soft over far less.)
    keep *= saturate((z_scene - z_frag) / mix(0.5, 0.03, rock));
    alpha *= saturate((z_scene - z_frag) / mix(0.5, 0.03, rock));
    let stray = in.edge * smoothstep(0.75, 1.0, edge_x) * (1.0 - smoothstep(0.88, 1.0, abs(side)));
    alpha = max(alpha, stray * body * 0.7 * smoothstep(0.62, 0.9, s) * saturate((z_scene - z_frag) / 0.03));
    if (alpha < 0.004) { discard; }

    // Streak relief: each finger is rounded across, and the surges in it ripple the tongue.
    // Lit from the side we see: the outer face, or the cliff side and underside from behind.
    let along = cross(in.normal, in.across);
    var n = normalize(in.normal + in.across * (s - s_du) * 6.0 + along * (s - s_dt) * 4.0);
    if (!front) { n = -n; }
    let n_dot_v = max(dot(n, v), 1e-3);

    // Light: sun (wrapping round, and glowing through the froth with the sun behind it),
    // sky, and glints off the wet surface.
    let l = frame.sun_dir.xyz;
    let sun = water_sun(in.rel);
    let shadow = shadow_factor(in.rel, n);
    let sky_vis = sky_visibility(in.rel, n);
    let up = sky_ambient(n) * sky_vis * PI;
    let wrap = saturate(dot(n, l) * 0.6 + 0.4);
    let through = pow(saturate(dot(-v, l)), 4.0) * 0.8;
    let froth = pre_expose(0.9 * (sun * shadow * (wrap + through) + up) * INV_PI);

    // The glassy tongue: what is behind it, bent and dimmed by the water it passes through
    // (a long way at a glancing look, where the stirred water's own color takes over), plus
    // the sky in it.
    let nv = (frame.view * vec4<f32>(n, 0.0)).xyz;
    let uv = clamp(in.clip.xy / size + vec2<f32>(nv.x, -nv.y) * 0.04, vec2<f32>(0.0), vec2<f32>(0.999));
    let behind = textureSampleLevel(scene_color, linear_clamp, uv, 0.0).rgb;
    let thick = clamp(q / max(in.look.w, 0.3), 0.01, 0.5);
    let path = thick / max(n_dot_v, 0.05);
    let tint = exp(-vec3<f32>(0.5, 0.11, 0.08) * path * 6.0 - path * 5.0);
    let stirred = froth * vec3<f32>(0.3, 0.38, 0.36);
    // From below (its underside) it reflects the dark water and rock under it, not the sky.
    let fresnel = (0.02 + 0.98 * pow(1.0 - n_dot_v, 5.0)) * select(0.2, 1.0, front);
    let r = reflect(-v, n);
    let fill = frame.ambient.rgb;
    let refl = pre_expose((sample_env(r) - fill) * sky_vis + fill);
    let glass = (behind * tint + stirred * (1.0 - tint)) * (1.0 - fresnel) + refl * fresnel;

    let h = normalize(l + v);
    let n_dot_l = saturate(dot(n, l));
    let a = 0.12 * 0.12;
    let spec = d_ggx(saturate(dot(n, h)), a) * v_smith_ggx_correlated(n_dot_v, max(n_dot_l, 1e-3), a)
        * f_schlick(vec3<f32>(0.02), saturate(dot(l, h))) * n_dot_l;
    let glint = pre_expose(spec * sun * shadow) * (0.3 + 0.7 * s);

    // Denser froth is whiter; thin threads let a little of the water's own color through.
    var color = mix(glass, froth * (0.7 + 0.45 * s), aerated) + glint;
    // Over the lip the tongue is unbroken: `glass` already lets what's behind show through.
    alpha = mix(max(alpha, body * keep), alpha, aerated);
    color = apply_fog(color, in.rel);
    return vec4<f32>(color * alpha, alpha);
}

// ---------------------------------------------------------------- puddles
struct PuddleOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) depth: f32,
};

@vertex
fn vs_puddle(@builtin(vertex_index) vid: u32) -> PuddleOut {
    let n = u32(water.g.puddle.w);
    let i = vid % n;
    let j = vid / n;
    let s = textureLoad(puddle_tex, vec2<i32>(i32(i), i32(j)), 0);
    let cell = water.g.puddle.z;
    let xz = water.g.puddle.xy + (vec2<f32>(f32(i), f32(j)) + 0.5) * cell;
    var out: PuddleOut;
    out.rel = vec3<f32>(xz.x, water.g.puddle2.x + s.g + 0.004 + s.r * 0.4, xz.y);
    out.clip = frame.view_proj * vec4<f32>(out.rel, 1.0);
    out.depth = s.r;
    return out;
}

@fragment
fn fs_puddle(in: PuddleOut) -> @location(0) vec4<f32> {
    if (clip_plane_distance(in.rel) < 0.0) { discard; }
    let wp = tiling_pos(in.rel).xz;
    // Water fills the low spots first: a noise "micro-relief" decides where the edge runs.
    let relief = fbm2(wp * 0.9, 4);
    let fill = in.depth / 0.035;
    let mask = smoothstep(relief - 0.06, relief + 0.06, fill * 0.85);
    if (mask <= 0.01) { discard; }
    let dist = length(in.rel);
    let v = -in.rel / max(dist, 1e-4);
    let pixel = pixel_footprint(in.rel);
    var g = capillary_gradient(wp, normalize(frame.wind.xz + vec2<f32>(1e-4)), length(frame.wind.xyz), footprint(in.rel)) * 0.35;
    g += rain_gradient(wp, rain_at(in.rel.xz), saturate(1.0 - pixel / 0.02));
    let n = normalize(vec3<f32>(-g.x, 1.0, -g.y));
    let size = vec2<f32>(textureDimensions(scene_color));
    let ground = textureLoad(scene_color, vec2<i32>(in.clip.xy), 0).rgb;
    let n_dot_v = max(dot(n, v), 1e-3);
    let fresnel = 0.02 + 0.98 * pow(1.0 - n_dot_v, 5.0);
    var r = reflect(-v, n);
    r.y = max(r.y, 0.02);
    let sky_vis = sky_visibility(in.rel, vec3<f32>(0.0, 1.0, 0.0));
    let fill_light = frame.ambient.rgb;
    var refl = pre_expose((sample_env(normalize(r)) - fill_light) * sky_vis + fill_light);
    let ssr = trace_reflection(in.rel, normalize(r), size, 0.05);
    refl = mix(refl, ssr.rgb, ssr.a);
    let l = frame.sun_dir.xyz;
    let h = normalize(l + v);
    let spec = d_ggx(saturate(dot(n, h)), 0.004) * v_smith_ggx_correlated(n_dot_v, max(dot(n, l), 1e-3), 0.004)
        * f_schlick(vec3<f32>(0.02), saturate(dot(l, h))) * saturate(dot(n, l));
    let sun = pre_expose(spec * water_sun(in.rel) * shadow_factor(in.rel, n));
    // Wet ground under a film of water: darker, then the mirror on top.
    let under = ground * 0.6;
    let color = under * (1.0 - fresnel) + refl * fresnel + sun;
    return vec4<f32>(mix(ground, color, mask), 1.0);
}

// ---------------------------------------------------------------- wet sand
// Over the scene, where the swash has been: sand darkened while the water is in it (light is
// trapped in the water between the grains, Lekner & Dorf 1988), a glassy sheen while a film is
// on it, and the foam the run left at its top. Blended as dst · a + rgb.
@fragment
fn fs_sand(in: FullscreenOut) -> @location(0) vec4<f32> {
    let pix = vec2<i32>(in.position.xy);
    let d = textureLoad(scene_depth, pix, 0);
    if (d <= 1e-7) { discard; }
    let ndc = vec2<f32>(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    let p = frame.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    let rel = p.xyz / p.w;
    let sand = swash_sand(rel.xz);
    if (water.g.swash2.z > 0.5) {
        // Debug (ENGINE_WATER_SWASH_DEBUG): depth red, damp green, sheen blue, ground step white.
        if (sand.w < -1e8) { discard; }
        let sw = swash_at(rel.xz);
        let off = select(0.0, 1.0, abs(rel.y - sand.w) > 0.25);
        return vec4<f32>(vec3<f32>(saturate(sw.depth * 20.0), sand.y, sand.x) * 0.8 + off * 0.2, 0.0);
    }
    // Only the ground itself (not a post or a boat standing in it).
    if (abs(rel.y - sand.w) > 0.25 || sand.y + sand.z < 0.01) { discard; }
    let wp = tiling_pos(rel).xz;
    // Dries unevenly: the grains' own pattern breaks up the edge.
    let grain = value_noise2(wp * 7.0) * 0.5 + value_noise2(wp * 23.0) * 0.5;
    let damp = saturate(sand.y * (0.85 + 0.3 * grain));
    let dark = mix(1.0, WET_SAND, damp);
    let dist = length(rel);
    let v = -rel / max(dist, 1e-4);
    let n = vec3<f32>(0.0, 1.0, 0.0);
    let n_dot_v = max(dot(n, v), 1e-3);
    let fresnel = 0.02 + 0.98 * pow(1.0 - n_dot_v, 5.0);
    let sky_vis = sky_visibility(rel, n);
    let fill = frame.ambient.rgb;
    let refl = pre_expose((sample_env(reflect(-v, n)) - fill) * sky_vis + fill);
    let l = frame.sun_dir.xyz;
    let h = normalize(l + v);
    let a = 0.06 * 0.06;
    let spec = d_ggx(saturate(dot(n, h)), a) * v_smith_ggx_correlated(n_dot_v, max(dot(n, l), 1e-3), a)
        * f_schlick(vec3<f32>(0.02), saturate(dot(l, h))) * saturate(dot(n, l));
    let shadow = shadow_factor(rel, n);
    let sun = water_sun(rel);
    let sheen = saturate(sand.x * (0.7 + 0.6 * grain));
    var add = (refl * fresnel + pre_expose(spec * sun * shadow)) * sheen;
    // The swash mark: lace of stranded foam, torn as it pops.
    var keep = dark;
    if (sand.z > 0.01) {
        let pattern = mix(fbm2(wp * 3.1, 4), foam_cells(wp, rel), 0.6);
        // Stranded foam is a line of lace popping on the sand, never a sheet, however much of
        // it the swash leaves.
        let theta = 1.0 - sand.z * 0.6;
        let cover = smoothstep(theta, theta + 0.12, pattern) * 0.9;
        let up = sky_ambient(n) * sky_vis * PI;
        let foam_col = pre_expose(0.75 * (sun * shadow * max(dot(n, l) * 0.6 + 0.4, 0.0) + up) * INV_PI);
        add = mix(add, foam_col, cover);
        keep *= 1.0 - cover;
    }
    let fog = surface_fog(rel);
    return vec4<f32>(add * fog.transmittance, keep);
}

// ---------------------------------------------------------------- the waterline
// A camera half in the water sees the waterline cross its lens: below it the water's fog, above
// it the air, a meniscus of water clinging along the line (Crest's and HDRP's underwater views
// split the screen the same way). The line is where the water meets the lens's dome: a pixel
// sees from under the water if the dome's point along its ray is. A dome of 20 cm, the size an
// over-under shot is taken through, holds the line steady across the frame while the water
// laps at it; the 5 cm near plane would flip the whole screen with every ripple.

// The surface's height (rel camera y) over a camera-relative x/z of body `bi`, as `vs_body`
// lifts its mesh there: the Gerstner waves' sideways push undone by a fixed-point step, then
// the ripples, the swash and the fine waves.
fn surface_y(bi: u32, xz: vec2<f32>) -> f32 {
    let origin = water.bodies[bi].origin;
    let s0 = water.bodies[bi].grid.z;
    var rest = xz;
    var w = eval_waves(bi, rest - origin.xz, body_depth(bi, rest), s0 * 2.5);
    rest = xz - w.offset.xz;
    w = eval_waves(bi, rest - origin.xz, body_depth(bi, rest), s0 * 2.5);
    var y = origin.y + w.offset.y;
    var free = 0.0;
    if (water.bodies[bi].look.w > 0.5) {
        y += ripple_at(rest).x;
        let sw = swash_at(rest);
        free = sw.w;
        y = mix(y, sw.level, free);
    }
    let bd = water.bodies[bi].detail;
    return y + detail_height(tiling_pos(vec3<f32>(xz.x, y, xz.y)).xz, bd.x, bd.yz, s0) * (1.0 - free);
}

// The view ray through a screen uv (0..1, y down), camera-relative.
fn view_dir(uv: vec2<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let far = frame.inv_view_proj * vec4<f32>(ndc, 1e-6, 1.0);
    return normalize(far.xyz / far.w);
}

// How far the lens's dome along `dir` is above the water, meters (below 0: under it). The
// water clings to the dome in a capillary wobble a few tenths of a millimeter high.
fn dome_above(dir: vec3<f32>) -> f32 {
    let q = dir * water.g.waterline.y;
    let wobble = (value_noise2(vec2<f32>(atan2(dir.x, dir.z) * 25.0, time_s() * 2.5)) - 0.5) * 4e-4;
    return q.y - surface_y(u32(water.g.rain.z), q.xz) + wobble;
}

// The water's own light along a view ray through `in_water` meters of it from the camera: the
// medium scattering in, brightened in the sheets of light the waves focus down. `jitter` (0..1,
// per pixel) staggers the march.
fn under_light(look: WaterLook, dir: vec3<f32>, in_water: f32, jitter: f32) -> vec3<f32> {
    let level = water.g.under2.w;
    let medium = in_water_radiance(look, dir, water.g.rain.w);
    let tr = exp(-(look.absorb + vec3<f32>(look.scattering)) * in_water);
    var glow = medium * (1.0 - tr);
    // Light shafts: the waves focus sunlight into moving sheets and columns. March the view
    // ray through the first stretch of water; each point is lit through the spot on the surface
    // straight up the refracted sun ray from it, so shafts line up with the sun and the
    // caustics on the bottom.
    let sun_up = saturate(frame.sun_dir.y * 3.0);
    if (sun_up > 0.0) {
        let sun_in = refract(-frame.sun_dir.xyz, vec3<f32>(0.0, 1.0, 0.0), 1.0 / 1.333);
        let up_sun = -sun_in / max(-sun_in.y, 0.2);
        let reach = min(in_water, 30.0);
        var rays = 0.0;
        var weight = 0.0;
        for (var i = 0; i < 12; i++) {
            let t = reach * (f32(i) + jitter) / 12.0;
            let s = dir * t;
            let below = max(level - s.y, 0.0);
            let at = tiling_pos(s).xz + up_sun.xz * below;
            let w = exp(-0.12 * t - 0.06 * below);
            rays += (caustics(at * 1.4) - 0.3) * w;
            weight += w;
        }
        rays /= max(weight, 1e-3);
        // Brightest looking toward the sun, faint looking away.
        let toward = 0.4 + 2.0 * pow(saturate(dot(dir, -sun_in)), 6.0);
        glow *= max(1.0 + rays * 2.2 * toward * sun_up * shadow_factor(vec3<f32>(0.0), vec3<f32>(0.0, 1.0, 0.0)), 0.0);
    }
    return glow;
}

// ---------------------------------------------------------------- underwater
@vertex
fn vs_under(@builtin(vertex_index) vid: u32) -> FullscreenOut {
    return fullscreen_vertex(vid, 0.0);
}

@fragment
fn fs_under(in: FullscreenOut) -> @location(0) vec4<f32> {
    let pix = vec2<i32>(in.position.xy);
    let d = textureLoad(scene_depth, pix, 0);
    var color = textureLoad(scene_color, pix, 0).rgb;
    let ndc = vec2<f32>(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    let dir = view_dir(in.uv);
    // At the waterline only the pixels whose dome is under the water see through it.
    let split = water.g.waterline.x > 0.5;
    var above = 0.0;
    if (split) {
        above = dome_above(dir);
        if (above > 0.0) { discard; }
    }
    var dist = 1e5;
    var rel = dir * 1e5;
    if (d > 1e-7) {
        let p = frame.inv_view_proj * vec4<f32>(ndc, d, 1.0);
        rel = p.xyz / p.w;
        dist = length(rel);
    }
    let level = water.g.under2.w;
    // Only the stretch of the ray below the surface is in water (from the dome, at the
    // waterline: up to the surface over it).
    var in_water = dist;
    if (split) {
        in_water = max(dist - water.g.waterline.y, 0.0);
        if (dir.y > 1e-4) { in_water = min(in_water, -above / dir.y); }
    } else if (dir.y > 1e-4) {
        in_water = min(dist, level / dir.y);
    }
    let look = WaterLook(water.g.under.rgb, water.g.under.w, water.g.under2.rgb, 0.05, 0.0, 1.0, 0.0);
    // Caustics dance on everything under the surface.
    if (d > 1e-7 && rel.y < level) {
        let sun_up = saturate(frame.sun_dir.y * 3.0);
        let depth_below = level - rel.y;
        let k = sun_up * exp(-depth_below * 0.2) * shadow_factor(rel, vec3<f32>(0.0, 1.0, 0.0));
        color *= 1.0 + caustics(tiling_pos(rel).xz * 1.4) * k * 1.4;
        color *= exp(-look.absorb * depth_below * 0.5);
    }
    let tr = exp(-(look.absorb + vec3<f32>(look.scattering)) * in_water);
    let jitter = march_jitter(in.position.xy);
    return vec4<f32>(color * tr + under_light(look, dir, in_water, jitter), 1.0);
}

// ---------------------------------------------------------------- the lens
// Water on the camera: the meniscus along the waterline, and as the camera comes up out of the
// water the sheet that drains off its glass and the drops it leaves behind. A drop has no color
// of its own: it is a tiny wide lens showing the scene shrunk and upside down, dark at its rim
// and with the sky's highlight on its upper side, wider at its foot as it sags (Hargreaves,
// "MotoGP: raindrops keep falling on my lens"). Drops past a critical size slide (gravity beats
// the pinning of contact angle hysteresis: Furmidge 1962); smaller ones stay and evaporate.
// Blended as dst · a + rgb.

// What the lens sees along `uv`: the scene copy taken before the water was drawn, with the
// lake's surface put back as a plane (its deep tint, the sky mirrored), which is all a blurred
// drop shows of it.
fn lens_scene(uv: vec2<f32>) -> vec3<f32> {
    let u = clamp(uv, vec2<f32>(0.0), vec2<f32>(0.999));
    var color = textureSampleLevel(scene_color, linear_clamp, u, 0.0).rgb;
    let level = water.g.under2.w;
    let dir = view_dir(u);
    if (level < 0.0 && dir.y < -1e-4) {
        let size = vec2<f32>(textureDimensions(scene_depth));
        let d = textureLoad(scene_depth, vec2<i32>(u * size), 0);
        var hidden = d <= 1e-7;
        if (!hidden) {
            let ndc = vec2<f32>(u.x * 2.0 - 1.0, 1.0 - u.y * 2.0);
            let p = frame.inv_view_proj * vec4<f32>(ndc, d, 1.0);
            hidden = p.y / p.w < level;
        }
        if (hidden) {
            let look = WaterLook(water.g.under.rgb, water.g.under.w, water.g.under2.rgb, 0.05, 0.0, 1.0, 0.0);
            let fresnel = 0.02 + 0.98 * pow(1.0 - saturate(-dir.y), 5.0);
            let sky = pre_expose(sample_env(reflect(dir, vec3<f32>(0.0, 1.0, 0.0))));
            color = mix(pre_expose(medium_radiance(look, vec3<f32>(0.0), 0.0)), sky, fresnel);
        }
    }
    return color;
}

@fragment
fn fs_lens(in: FullscreenOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let size = vec2<f32>(textureDimensions(scene_color));
    let aspect = size.x / size.y;
    let sky_up = pre_expose(sample_env(vec3<f32>(0.0, 1.0, 0.0)));
    var add = vec3<f32>(0.0);
    var keep = 1.0;
    if (water.g.waterline.x > 0.5) {
        // The meniscus: water climbing the dome a millimeter or two over the line bends the
        // dark water below up into a band, its top edge catching the sky.
        let above = dome_above(view_dir(uv));
        let w = max(0.0015, water.g.waterline2.x * 2.5);
        let t = above / w;
        let band = exp(-(t - 0.5) * (t - 0.5) / 0.5) * 0.85;
        let rim = exp(-(t - 1.35) * (t - 1.35) / 0.03);
        if (band + rim > 0.01) {
            // The film is water: it tints what it bends toward the water's own color.
            let look = WaterLook(water.g.under.rgb, water.g.under.w, water.g.under2.rgb, 0.05, 0.0, 1.0, 0.0);
            let bent = lens_scene(uv + vec2<f32>(0.0, w / max(water.g.waterline2.x, 1e-6) * 2.0 / size.y));
            let tint = mix(bent * exp(-look.absorb * 0.3), pre_expose(medium_radiance(look, vec3<f32>(0.0), 0.0)), 0.5);
            add = tint * band + sky_up * rim * 0.3;
            keep = 1.0 - band;
        }
        // Under the line the lens is in the water: nothing clings to it.
        if (above < 0.0) { return vec4<f32>(add, keep); }
    }
    let wet = water.g.waterline.z;
    if (wet > 0.0) {
        let since = water.g.waterline.w;
        let seed = water.g.waterline2.y;
        let p = vec2<f32>(uv.x * aspect, uv.y);
        // The sheet draining off: its ragged front runs down the glass, gone about a second
        // after the camera is out. It dims what is seen through it a little, and its runnels
        // catch the sky in streaks down the glass.
        let front = since * 1.6 - 0.1 + (value_noise2(vec2<f32>(p.x * 9.0, seed)) - 0.5) * 0.12;
        let sheet = smoothstep(front - 0.03, front + 0.03, uv.y) * saturate(1.0 - since / 1.2);
        if (sheet > 0.0) {
            let runnels = value_noise2(vec2<f32>(p.x * 38.0, p.y * 2.5 - since * 5.0));
            let lip = exp(-pow((uv.y - front) / 0.012, 2.0));
            add += sky_up * sheet * (0.12 * pow(runnels, 6.0) + 0.1 * lip);
            keep *= 1.0 - sheet * (0.2 + 0.15 * runnels);
        }
        // The drops: big ones (some sliding) over a scatter of small ones.
        for (var layer = 0; layer < 2; layer++) {
            let big = layer == 0;
            let cell = select(0.035, 0.09, big);
            var q = p;
            let col = floor(q.x / cell);
            let slides = big && hash12(vec2<f32>(col, seed + 3.0)) > 0.55;
            q.y -= select(0.0, (0.03 + 0.12 * hash12(vec2<f32>(col, seed + 7.0))) * since, slides);
            let id = floor(q / cell);
            let f = q / cell - id;
            let k = id + vec2<f32>(seed * 13.1, f32(layer) * 17.3);
            let h = vec4<f32>(hash12(k), hash12(k + 1.7), hash12(k + 3.1), hash12(k + 5.9));
            if (h.x > select(0.3, 0.2, big)) { continue; }
            // Each drop dries off at its own time as the wetness falls.
            let life = saturate((wet - h.y * 0.9) * 8.0);
            if (life <= 0.0) { continue; }
            let r = mix(0.12, 0.36, h.z);
            let c = vec2<f32>(r + (1.0 - 2.0 * r) * h.w, r + (1.0 - 2.0 * r) * fract(h.w * 7.3 + h.z));
            var d = (f - c) / r;
            d.y *= select(1.0, 0.85, d.y > 0.0);
            let d2 = dot(d, d);
            if (d2 >= 1.0) { continue; }
            let a = smoothstep(1.0, 0.8, sqrt(d2)) * life;
            let r_uv = r * cell;
            let seen = lens_scene(uv - vec2<f32>(d.x / aspect, d.y) * (r_uv + 0.12));
            var drop = seen * (1.0 - 0.55 * smoothstep(0.55, 1.0, sqrt(d2)));
            let hl = d - vec2<f32>(-0.3, -0.45);
            drop += sky_up * 0.5 * exp(-dot(hl, hl) * 30.0);
            add = mix(add, drop, a);
            keep *= 1.0 - a;
        }
    }
    if (keep > 0.999) { discard; }
    return vec4<f32>(add, keep);
}
