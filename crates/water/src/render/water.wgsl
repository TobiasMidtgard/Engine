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
    counts: vec4<u32>,   // bodies, showers, puddles on, unused
    under: vec4<f32>,    // underwater absorption rgb, scattering
    under2: vec4<f32>,   // underwater scatter color rgb, surface level rel camera y
};

struct WaterUniform {
    g: Globals,
    bodies: array<GpuBody, 8>,
};

@group(1) @binding(0) var<uniform> water: WaterUniform;
@group(1) @binding(1) var ripple_tex: texture_2d<f32>;      // r height, g foam, b dh/dx, a dh/dz
@group(1) @binding(2) var scene_color: texture_2d<f32>;     // pre-exposed HDR, before transparents
@group(1) @binding(3) var scene_depth: texture_depth_2d;    // reverse-Z
@group(1) @binding(4) var puddle_tex: texture_2d<f32>;      // r depth (m), g ground y rel puddle2.x

struct FlowGpu {
    origin: vec4<f32>,  // corner rel camera x, base y, z, cell (m)
    dims: vec4<f32>,    // cells x, z, unused, unused
    absorb: vec4<f32>,
    scatter: vec4<f32>,
    look: vec4<f32>,
};
@group(2) @binding(0) var<uniform> flow: FlowGpu;
@group(2) @binding(1) var flow_tex: texture_2d<f32>;        // r surface (m above base), g depth, b vel x, a vel z

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
    let sigma_t = look.absorb + vec3<f32>(look.scattering);
    let albedo = vec3<f32>(look.scattering) / max(sigma_t, vec3<f32>(1e-4));
    // Light scattered toward the eye, from all the water around (multiple scattering
    // brightens it well beyond single scattering).
    return look.scatter_color * albedo * e * 0.4;
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

// Full water surface shading seen from above. `n` world normal, `foam` 0..1, `crest` 0..1 how
// high on a wave this point is, `wind` m/s. Returns pre-exposed radiance with fog applied.
// `edge` is how deep the water gets before the waterline is fully drawn, meters.
fn shade_water(rel: vec3<f32>, n_in: vec3<f32>, look: WaterLook, foam_in: f32, crest: f32,
               frag: vec4<f32>, wind: f32, edge: f32) -> vec3<f32> {
    let dist = length(rel);
    let v = -rel / max(dist, 1e-4);
    var n = n_in;
    let ndv0 = dot(n, v);
    if (ndv0 < 0.02) { n = normalize(n + v * (0.02 - ndv0)); }
    let n_dot_v = max(dot(n, v), 1e-3);
    let fresnel = 0.02 + 0.98 * pow(1.0 - n_dot_v, 5.0);
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
    let rough = clamp(look.roughness + 0.01 * wind + dist * 0.00015, 0.02, 0.6);
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
    let sss = pre_expose(look.scatter_color * sun * shadow * behind_sun * crest * 0.012);

    var color = water_col * (1.0 - fresnel) + refl * fresnel + glitter + sss;

    // ---- foam: whitecaps, wakes, splashes, shoreline
    var foam = foam_in;
    let shore = 1.0 - smoothstep(0.0, 0.3, depth_below);
    if (thick < 1e5) {
        let wp = tiling_pos(rel).xz;
        let lap = 0.5 + 0.5 * sin(time_s() * 1.3 + value_noise2(wp * 0.7) * 6.0);
        foam += shore * lap * 0.6;
    }
    foam = saturate(foam * look.foam);
    if (foam > 0.001) {
        let wp = tiling_pos(rel).xz;
        let pattern = fbm2(wp * 2.3 + vec2<f32>(time_s() * 0.07, 0.0), 4);
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
    if (water.bodies[bi].look.w > 0.5) {
        let rp = ripple_at(in.rest);
        n = normalize(n + vec3<f32>(-rp.z, 0.0, -rp.w));
        foam += rp.y * 1.4;
    }
    // Whitecaps where the waves pinch.
    let wind = water.bodies[bi].bed.z;
    foam += saturate((0.55 - in.jac) * 2.5) * saturate((wind - 4.0) * 0.25);
    let wp = tiling_pos(in.rel).xz;
    var g = capillary_gradient(wp, water.bodies[bi].wind.xy, wind, footprint(in.rel));
    g += rain_gradient(wp, rain_at(in.rest), saturate(1.0 - pixel / 0.02));
    n = normalize(n + vec3<f32>(-g.x, 0.0, -g.y));
    var color: vec3<f32>;
    // Seen from below when the camera is under the water.
    if (water.g.rain.z >= 0.0) {
        color = shade_underside(in.rel, n, look, in.clip);
    } else {
        color = shade_water(in.rel, n, look, foam, in.crest, in.clip, wind, 0.05);
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
    var out: FlowOut;
    out.normal = normalize(vec3<f32>(hl - hr, 2.0 * cell, hb - ht));
    out.rel = vec3<f32>(flow.origin.x + (f32(i) + 0.5) * cell, flow.origin.y + y,
                        flow.origin.z + (f32(j) + 0.5) * cell);
    out.clip = frame.view_proj * vec4<f32>(out.rel, 1.0);
    out.depth = s.g;
    out.vel = s.ba;
    return out;
}

@fragment
fn fs_flow(in: FlowOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
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
    var g = mix(g0, g1, wgt) * (1.0 + speed * 1.5);
    g += rain_gradient(wp, rain_at(in.rel.xz), saturate(1.0 - pixel / 0.02));
    let n = normalize(in.normal + vec3<f32>(-g.x, 0.0, -g.y));
    // White water: fast, steep or tumbling.
    let steep = 1.0 - in.normal.y;
    var foam = saturate((speed - 2.2) * 0.35) + saturate(steep * 2.5 - 0.3) * 1.3;
    foam *= saturate(in.depth * 30.0);
    var color: vec3<f32>;
    if (front) {
        // Streams run shallow over riffles: only the last centimeter fades.
        color = shade_water(in.rel, n, look, foam, saturate(steep * 2.0), in.clip, 3.0 + speed, 0.01);
    } else {
        color = shade_underside(in.rel, n, look, in.clip);
    }
    return vec4<f32>(color, 1.0);
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
    return vec4<f32>(color * tr + medium * (1.0 - tr), 1.0);
}
