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
const MAX_BODIES: u32 = 8u;

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
    plunges: array<vec4<f32>, 8>, // where falls land: foot rel camera xyz, boil radius (m)
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
@group(1) @binding(5) var current_tex: texture_2d<f32>;     // ripple grid currents: x, z (m/s), turn (1/s)

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
    let count = min(u32(water.bodies[bi].bed.w), MAX_WAVES);
    for (var i = 0u; i < count; i++) {
        let w = water.bodies[bi].waves[i];
        let k = w.a.z;
        let lod = saturate((TAU / max(k, 1e-4)) / max(min_len, 1e-4) - 1.0);
        let f = tanh(k * max(depth, 0.0) * 1.2) * lod;
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
    return out;
}

// Interactive ripples at a camera-relative x/z: (height, foam, dh/dx, dh/dz), faded at the edge.
fn ripple_at(rel_xz: vec2<f32>) -> vec4<f32> {
    if (water.g.ripple.w < 0.5) { return vec4<f32>(0.0); }
    let uv = (rel_xz - water.g.ripple.xy) / water.g.ripple.z;
    if (any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) { return vec4<f32>(0.0); }
    let edge = min(min(uv.x, uv.y), min(1.0 - uv.x, 1.0 - uv.y));
    let fade = smoothstep(0.0, 0.08, edge);
    return textureSampleLevel(ripple_tex, linear_clamp, uv, 0.0) * fade;
}

// Currents stirred into the ripple grid at a camera-relative x/z: (x, z m/s, turn 1/s, 0),
// faded at the edge.
fn current_at(rel_xz: vec2<f32>) -> vec4<f32> {
    if (water.g.ripple.w < 0.5) { return vec4<f32>(0.0); }
    let uv = (rel_xz - water.g.ripple.xy) / water.g.ripple.z;
    if (any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) { return vec4<f32>(0.0); }
    let edge = min(min(uv.x, uv.y), min(1.0 - uv.x, 1.0 - uv.y));
    let fade = smoothstep(0.0, 0.08, edge);
    return textureSampleLevel(current_tex, linear_clamp, uv, 0.0) * fade;
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
    let sun = frame.sun_color.rgb * sun_transmittance(rel) * max(frame.sun_dir.y, 0.0);
    let sky = sky_ambient(vec3<f32>(0.0, 1.0, 0.0)) * sky_visibility(rel, vec3<f32>(0.0, 1.0, 0.0)) * PI;
    let down = exp(-(look.absorb + vec3<f32>(look.scattering)) * max(depth_below, 0.0));
    let e = (sun * 0.98 + sky) * INV_PI * down;
    // Light the water sends back up from below the surface: Gordon's reflectance of deep
    // water, r = (0.0949 + 0.0794 u) u with u = bb / (a + bb), per band. It is what the
    // particles scatter back (`scatter_color` is backscattering per meter of scattering)
    // against what the water absorbs: blue for pure water, turquoise for rock flour, green
    // for algae, brown for silt and tannins.
    let bb = look.scattering * look.scatter_color;
    let u = bb / max(look.absorb + bb, vec3<f32>(1e-5));
    let r = (0.0949 + 0.0794 * u) * u;
    return r * e * PI * SCATTER_GAIN;
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

// Full water surface shading seen from above. `n` world normal, `foam` 0..1, `crest` 0..1 how
// high on a wave this point is, `wind` m/s. Returns pre-exposed radiance with fog applied.
// `edge` is how deep the water gets before the waterline is fully drawn, meters.
// `shore_depth`: water shallower than this laps at the shore (a lake's beach; 0 for none,
// as for streams, which lap by their own depth). `vel`: the current (m/s), which carries the
// foam downstream.
fn shade_water(rel: vec3<f32>, n_in: vec3<f32>, look: WaterLook, foam_in: f32, crest: f32,
               frag: vec4<f32>, wind: f32, edge: f32, shore_depth: f32, vel: vec2<f32>) -> vec3<f32> {
    let dist = length(rel);
    let v = -rel / max(dist, 1e-4);
    var n = n_in;
    let ndv0 = dot(n, v);
    if (ndv0 < 0.02) { n = normalize(n + v * (0.02 - ndv0)); }
    let n_dot_v = max(dot(n, v), 1e-3);
    // Wind roughens the water below what the waves and ripples resolve: the facets' slopes
    // spread by about 0.003 + 0.00512·U (Cox–Munk), more of it unresolved further off. A gale
    // blurs the reflections and breaks up the grazing mirror (facets tilt toward the eye).
    let unresolved = sqrt(0.003 + 0.00512 * wind) * smoothstep(5.0, 14.0, wind)
        * mix(0.6, 1.5, saturate(dist / 80.0));
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
    if (rel.y < 0.0 && z0 < 1e5) {
        shoreline = smoothstep(0.0, edge, -rel.y * max(z0 / z_frag - 1.0, 0.0));
    }
    let nv = (frame.view * vec4<f32>(n, 0.0)).xyz;
    let strength = look.refraction * 0.05 * saturate(thick0 * 0.7) * min(1.0, 12.0 / z_frag);
    var uv = clamp(uv0 + vec2<f32>(nv.x, -nv.y) * strength, vec2<f32>(0.0), vec2<f32>(0.999));
    var z_bottom = linear_depth(textureLoad(scene_depth, vec2<i32>(uv * size), 0));
    if (z_bottom < z_frag) {
        uv = uv0;
        z_bottom = z0;
    }
    let behind = textureSampleLevel(scene_color, linear_clamp, uv, 0.0).rgb;
    let ratio = z_bottom / z_frag;
    let thick = dist * max(ratio - 1.0, 0.0);
    let bottom_rel = rel * ratio;
    let depth_below = max(rel.y - bottom_rel.y, 0.0);
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
    let rough = clamp(look.roughness + 0.01 * wind + dist * 0.00015 + unresolved, 0.02, 0.75);
    let sky_vis = sky_visibility(rel, vec3<f32>(0.0, 1.0, 0.0));
    let fill = frame.ambient.rgb;
    let env = (mix(sample_env(r), sky_ambient(r), saturate(rough * 1.5)) - fill) * sky_vis + fill;
    var refl = pre_expose(env);
    let ssr = trace_reflection(rel, r, size, rough);
    refl = mix(refl, ssr.rgb, ssr.a);

    // ---- sun glitter
    let l = frame.sun_dir.xyz;
    let sun = frame.sun_color.rgb * sun_transmittance(rel);
    let shadow = shadow_factor(rel, n);
    let h = normalize(l + v);
    let n_dot_l = saturate(dot(n, l));
    let a = rough * rough;
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

    // ---- foam: whitecaps, wakes, splashes, shoreline
    var foam = foam_in;
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
        let cover = smoothstep(1.0 - foam, 1.0 - foam + 0.3, pattern + foam * 0.35);
        let up = sky_ambient(n) * sky_vis * PI;
        // Foam is a dense froth: light gets through it, so it glows even with the sun behind.
        let foam_col = pre_expose(0.85 * (sun * shadow * max(dot(n, l) * 0.6 + 0.4, 0.0) + up) * INV_PI);
        color = mix(color, foam_col, cover * 0.95);
    }
    let ground = textureSampleLevel(scene_color, linear_clamp, uv0, 0.0).rgb;
    return mix(ground, apply_fog(color, rel), shoreline);
}

// The surface seen from below: Snell's window to the world above, total internal reflection
// of the deep around it, all through the water between the camera and the surface.
fn shade_underside(rel: vec3<f32>, n_up: vec3<f32>, look: WaterLook, frag: vec4<f32>) -> vec3<f32> {
    let dist = length(rel);
    let v = -rel / max(dist, 1e-4);
    let n = -n_up;
    let cos_i = max(dot(n, v), 1e-3);
    let t = refract(-v, n, 1.333);
    let medium = pre_expose(medium_radiance(look, rel, water.g.rain.w));
    var color = medium;
    if (dot(t, t) > 0.0) {
        // Leaving the water: Fresnel from the water side.
        let f = 0.02 + 0.98 * pow(1.0 - cos_i, 5.0);
        let size = vec2<f32>(textureDimensions(scene_color));
        let nv = (frame.view * vec4<f32>(n_up, 0.0)).xyz;
        let uv = clamp(frag.xy / size + vec2<f32>(nv.x, -nv.y) * 0.03, vec2<f32>(0.0), vec2<f32>(0.999));
        let above = textureSampleLevel(scene_color, linear_clamp, uv, 0.0).rgb;
        let sky = pre_expose(sample_env(t));
        let world = mix(sky, above, 0.65);
        color = world * (1.0 - f) * 1.4 + medium * f;
    }
    let sigma_t = look.absorb + vec3<f32>(look.scattering);
    let tr = exp(-sigma_t * dist);
    return color * tr + medium * (1.0 - tr);
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
    if (water.bodies[bi].look.w > 0.5) { y += ripple_at(rest).x; }
    var out: BodyOut;
    out.rel = vec3<f32>(rest.x + w.offset.x, y, rest.y + w.offset.z);
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
    let bi = in.body;
    if (body_inset(bi, body_local(bi, in.rest)) < 0.0) { discard; }
    let look = body_look(bi);
    let dist = length(in.rel);
    // Size of a pixel here, meters: drop detail finer than that.
    let pixel = pixel_footprint(in.rel);
    let depth = body_depth(bi, in.rest);
    let w = eval_waves(bi, in.p0, depth, pixel * 3.0);
    var n = w.normal;
    var foam = 0.0;
    var vel = water.bodies[bi].wind.zw;
    if (water.bodies[bi].look.w > 0.5) {
        let rp = ripple_at(in.rest);
        n = normalize(n + vec3<f32>(-rp.z, 0.0, -rp.w));
        foam += rp.y * 2.2;
        // Foam rides the stirred currents: its streaks follow their swirls.
        vel += current_at(in.rest).xy;
    }
    // Whitecaps where the waves pinch.
    let wind = water.bodies[bi].bed.z;
    foam += saturate((0.55 - in.jac) * 2.5) * saturate((wind - 4.0) * 0.25);
    let wp = tiling_pos(in.rel).xz;
    // In a strong wind the crests break all over, in patches too small and quick for the waves
    // above to pinch: about 3.84e-6·U^3.41 of the sea is white (Monahan), the breakers drawn
    // out downwind. The wind drags the surface along at about 3% of its speed, so foam streaks
    // run with it.
    let wind_dir = normalize(water.bodies[bi].wind.xy + vec2<f32>(1e-4, 0.0));
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
        foam += smoothstep(edge, edge + 0.1, breaking) * smoothstep(0.3, 0.6, lace) * 1.3;
        // From a near gale on, the foam the breakers leave is drawn into rows along the wind.
        let rows = value_noise2(vec2<f32>(dot(wp, wind_dir) * 0.06, dot(wp, across) * 1.1));
        foam += smoothstep(0.6, 0.8, rows) * 0.35 * smoothstep(10.0, 16.0, wind);
        vel += wind_dir * wind * 0.03;
    }
    var g = capillary_gradient(wp, water.bodies[bi].wind.xy, wind, footprint(in.rel));
    g += rain_gradient(wp, rain_at(in.rest), saturate(1.0 - pixel / 0.02));
    let boil = plunge_boil(in.rel);
    foam += boil.x;
    g += boil.yz;
    vel += plunge_flow(in.rel);
    n = normalize(n + vec3<f32>(-g.x, 0.0, -g.y));
    var color: vec3<f32>;
    // Seen from below when the camera is under the water.
    if (water.g.rain.z >= 0.0) {
        color = shade_underside(in.rel, n, look, in.clip);
    } else {
        color = shade_water(in.rel, n, look, foam, in.crest, in.clip, wind, 0.05, 0.3, vel);
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
    return out;
}

@fragment
fn fs_flow(in: FlowOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
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
    let g0 = capillary_gradient(wp - shift * phase0, wind_dir, 3.0 + speed * 3.0, fp);
    let g1 = capillary_gradient(wp - shift * phase1 + vec2<f32>(4.7, 1.3), wind_dir, 3.0 + speed * 3.0, fp);
    let wgt = abs(phase0 - 0.5) * 2.0;
    var g = mix(g0, g1, wgt) * (1.0 + speed * 0.5);
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
        let across = vec2<f32>(-in.vel.y, in.vel.x) / max(speed, 1e-3);
        let along = in.vel / max(speed, 1e-3);
        // Longer across the current than along it, like the ribs of a riffle.
        let q = vec2<f32>(dot(wp, along) * 1.8, dot(wp, across) * 0.9) + vec2<f32>(sin(t * 3.1), cos(t * 2.3)) * 0.05;
        let e = 0.05;
        let h = fbm2(q, 3) / 0.875;
        let dh = vec2<f32>(fbm2(q + vec2<f32>(e, 0.0), 3), fbm2(q + vec2<f32>(0.0, e), 3)) / 0.875 - h;
        let slope = (along * dh.x * 1.8 + across * dh.y * 0.9) / e;
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
        color = shade_water(in.rel, n, look, foam, saturate(steep * 2.0), in.clip, 3.0 + speed, 0.01, 0.0, in.vel);
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
};

struct FallOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) across: vec3<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) look: vec4<f32>,        // q, frac, side, lip speed
    @location(5) kind: f32,              // 1 spray cloud; -1 spray round a curtain; under a half a curtain, 0.45 where it slides down rock
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
    return out;
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
    let sun = frame.sun_color.rgb * sun_transmittance(in.rel) * shadow_factor(in.rel, l);
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
    let sun = frame.sun_color.rgb * sun_transmittance(rel);
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
    // Frayed at the sides.
    alpha *= 1.0 - smoothstep(0.3, 1.0, abs(in.look.z) + (clumps - 0.5) * 0.6);
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
    let broken = fall_broken(in.look.x, tau);
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
    alpha *= 1.0 - smoothstep(0.6, 1.0, abs(side) + (s - 0.5) * 0.4);
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
    alpha *= saturate((z_scene - z_frag) / mix(0.5, 0.03, rock));
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
    let sun = frame.sun_color.rgb * sun_transmittance(in.rel);
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
    alpha = mix(max(alpha, body), alpha, aerated);
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
    let sun = pre_expose(spec * frame.sun_color.rgb * sun_transmittance(in.rel) * shadow_factor(in.rel, n));
    // Wet ground under a film of water: darker, then the mirror on top.
    let under = ground * 0.6;
    let color = under * (1.0 - fresnel) + refl * fresnel + sun;
    return vec4<f32>(mix(ground, color, mask), 1.0);
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
    let far = frame.inv_view_proj * vec4<f32>(ndc, 1e-6, 1.0);
    let dir = normalize(far.xyz / far.w);
    var dist = 1e5;
    var rel = dir * 1e5;
    if (d > 1e-7) {
        let p = frame.inv_view_proj * vec4<f32>(ndc, d, 1.0);
        rel = p.xyz / p.w;
        dist = length(rel);
    }
    let level = water.g.under2.w;
    // Only the stretch of the ray below the surface is in water.
    var in_water = dist;
    if (dir.y > 1e-4) { in_water = min(dist, level / dir.y); }
    let look = WaterLook(water.g.under.rgb, water.g.under.w, water.g.under2.rgb, 0.05, 0.0, 1.0, 0.0);
    // Caustics dance on everything under the surface.
    if (d > 1e-7 && rel.y < level) {
        let sun_up = saturate(frame.sun_dir.y * 3.0);
        let depth_below = level - rel.y;
        let k = sun_up * exp(-depth_below * 0.2) * shadow_factor(rel, vec3<f32>(0.0, 1.0, 0.0));
        color *= 1.0 + caustics(tiling_pos(rel).xz * 1.4) * k * 1.4;
        color *= exp(-look.absorb * depth_below * 0.5);
    }
    let medium = pre_expose(medium_radiance(look, vec3<f32>(0.0), water.g.rain.w));
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
        // Per-pixel jitter (interleaved gradient noise) instead of banding; TAA smooths it.
        let jitter = fract(52.9829189 * fract(dot(in.position.xy, vec2<f32>(0.06711056, 0.00583715))));
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
    return vec4<f32>(color * tr + glow, 1.0);
}
