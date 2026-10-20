// ============================================================================
// Engine shader library: the atmosphere (sky color, sunlight through the air, aerial
// perspective). Requires lib_types.wgsl and lib_frame.wgsl (group 0).
//
// Rayleigh, Mie and ozone on a spherical planet, after Hillaire 2020 ("A Scalable and
// Production Ready Sky and Atmosphere Rendering Technique"). Look-up tables hold the sunlight's
// transmittance to the top of the atmosphere, the light of all higher orders of scattering
// (so the sky has the brightness and color of real multiple scattering: a pale horizon, a
// lit twilight), and, every frame, the sky as seen from the camera. They are computed by
// atmosphere_luts.wgsl. Works from the ground, from orbit and from the Moon.
//
// Radiance is per lux of sunlight above the atmosphere (multiply by `frame.sun_color`).
// Lengths are meters; `h` is a height above the planet surface, `mu` the cosine of a direction's
// angle from the local zenith.
//
// The air is the planet's (`Atmosphere` on the CPU): Rayleigh and Mie scattering, Mie and ozone
// absorption per meter at the surface (frame.air_*), so Earth's sky is blue and Mars' is
// butterscotch with a blue sunset.
// ============================================================================

const TRANSMITTANCE_LUT_SIZE: vec2<f32> = vec2<f32>(256.0, 64.0);
const MULTISCATTER_LUT_SIZE: vec2<f32> = vec2<f32>(32.0, 32.0);
const SKY_VIEW_LUT_SIZE: vec2<f32> = vec2<f32>(192.0, 108.0);

// ---------------------------------------------------------------- geometry

fn ray_sphere(origin: vec3<f32>, dir: vec3<f32>, center: vec3<f32>, radius: f32) -> vec2<f32> {
    let oc = origin - center;
    let b = dot(oc, dir);
    let c = dot(oc, oc) - radius * radius;
    let h = b * b - c;
    if (h < 0.0) { return vec2<f32>(-1.0, -1.0); }
    let s = sqrt(h);
    return vec2<f32>(-b - s, -b + s);
}

// Height above the planet surface. Exact for the camera (computed in f64 on the CPU).
fn planet_height(p: vec3<f32>) -> f32 {
    if (all(p == vec3<f32>(0.0))) { return frame.camera_world.w; }
    return length(p - frame.planet.xyz) - frame.planet.w;
}

// Distances along a ray from height `h` with zenith cosine `mu` to the sphere `shell` meters
// above the surface: (near, far), (-1, -1) when missed. Numerically stable at planetary scale
// in f32 (no r^2 - R^2 cancellation).
fn ray_shell_mu(h: f32, mu: f32, shell: f32) -> vec2<f32> {
    let radius = frame.planet.w;
    let b = (radius + h) * mu;
    let c = (h - shell) * (2.0 * radius + h + shell);
    let d = b * b - c;
    if (d < 0.0) { return vec2<f32>(-1.0, -1.0); }
    let s = sqrt(d);
    var q = -b - select(1.0, -1.0, b < 0.0) * s;
    if (abs(q) < 1e-6) { q = 1e-6; }
    let t0 = q;
    let t1 = c / q;
    return vec2<f32>(min(t0, t1), max(t0, t1));
}

// `ray_shell_mu` for a camera-relative ray starting at `origin` (height `h`).
fn ray_shell(origin: vec3<f32>, h: f32, dir: vec3<f32>, shell: f32) -> vec2<f32> {
    return ray_shell_mu(h, dot(normalize(origin - frame.planet.xyz), dir), shell);
}

// sqrt(r^2 - R^2) at height h: the distance to the horizon.
fn atmo_rho(h: f32) -> f32 {
    let hh = max(h, 0.0);
    return sqrt(hh * (2.0 * frame.planet.w + hh));
}

// The same at the top of the atmosphere.
fn atmo_top_rho() -> f32 {
    let t = frame.atmosphere.x;
    return sqrt(t * (2.0 * frame.planet.w + t));
}

// Height after going `t` meters from height `h` along zenith cosine `mu`.
fn height_along(h: f32, mu: f32, t: f32) -> f32 {
    let radius = frame.planet.w;
    let q = h * (2.0 * radius + h) + t * (t + 2.0 * (radius + h) * mu); // r(t)^2 - R^2
    return q / (sqrt(radius * radius + q) + radius);
}

// Distance from height h (inside the atmosphere) to its top along zenith cosine mu.
fn distance_to_top(h: f32, mu: f32) -> f32 {
    let radius = frame.planet.w;
    let top = frame.atmosphere.x;
    let b = (radius + h) * mu;
    let c = (top - h) * (2.0 * radius + top + h); // top^2 - r^2
    let d = b * b + c;
    if (d < 0.0) { return 0.0; }
    let s = sqrt(d);
    if (b > 0.0) { return max(c / (b + s), 0.0); }
    return max(s - b, 0.0);
}

// Optical depth (Rayleigh, Mie, ozone; feed it to `extinction`) from `p` at height `h` along
// `dir` to the top of the atmosphere, ray-marched with `samples` steps; x = -1 when the ray hits
// the ground. Kept for shaders outside the renderer (the weather module's cloud composite).
fn optical_depth_to_sun(p: vec3<f32>, h: f32, dir: vec3<f32>, samples: i32) -> vec3<f32> {
    if (ray_shell(p, h, dir, -50.0).x > 0.0) { return vec3<f32>(-1.0); }
    let len = max(ray_shell(p, h, dir, frame.atmosphere.x).y, 0.0);
    var od = vec3<f32>(0.0);
    var prev = 0.0;
    for (var j = 0; j < samples; j++) {
        let f = f32(j + 1) / f32(samples);
        let t = len * f * f;
        let tm = (prev + t) * 0.5;
        od += atmo_density(planet_height(p + dir * tm)) * (t - prev);
        prev = t;
    }
    return od;
}

// How much of the sun's disc is above the horizon at height h for sun zenith cosine mu_s.
fn sun_above_horizon(h: f32, mu_s: f32) -> f32 {
    let mu_horizon = -atmo_rho(h) / (frame.planet.w + max(h, 0.0));
    let w = max(frame.sun_dir.w, 0.002);
    return smoothstep(-w, w, mu_s - mu_horizon);
}

// ---------------------------------------------------------------- the medium

fn phase_rayleigh(cos_t: f32) -> f32 { return 3.0 / (16.0 * PI) * (1.0 + cos_t * cos_t); }

fn phase_mie(cos_t: f32, g: f32) -> f32 {
    let g2 = g * g;
    return 3.0 / (8.0 * PI) * ((1.0 - g2) * (1.0 + cos_t * cos_t))
        / ((2.0 + g2) * pow(max(1.0 + g2 - 2.0 * g * cos_t, 1e-4), 1.5));
}

// `phase_mie` with an anisotropy per color channel (the air's: frame.air_mie_g).
fn phase_mie_rgb(cos_t: f32, g: vec3<f32>) -> vec3<f32> {
    let g2 = g * g;
    return 3.0 / (8.0 * PI) * ((1.0 - g2) * (1.0 + cos_t * cos_t))
        / ((2.0 + g2) * pow(max(1.0 + g2 - 2.0 * g * cos_t, vec3<f32>(1e-4)), vec3<f32>(1.5)));
}

fn ozone_density(h: f32) -> f32 {
    return max(0.0, 1.0 - abs(h - 25000.0) / 15000.0);
}

// Densities (rayleigh, mie, ozone) at height h.
fn atmo_density(h: f32) -> vec3<f32> {
    let hh = max(h, 0.0);
    return vec3<f32>(exp(-hh / frame.atmosphere.y), exp(-hh / frame.atmosphere.z), ozone_density(hh));
}

// Extinction (1/m, or optical depth) of the air for densities (or optical depths) `od` of
// (rayleigh, mie, ozone).
fn extinction(od: vec3<f32>) -> vec3<f32> {
    return frame.air_rayleigh.rgb * od.x + frame.air_mie_extinction.rgb * od.y + frame.air_ozone.rgb * od.z;
}

struct Medium {
    rayleigh: vec3<f32>,    // Rayleigh scattering coefficient (1/m)
    mie: vec3<f32>,         // Mie scattering coefficient (1/m)
    extinction: vec3<f32>,  // total extinction (1/m)
};

fn atmo_medium(h: f32) -> Medium {
    let d = atmo_density(h);
    var m: Medium;
    m.rayleigh = frame.air_rayleigh.rgb * d.x;
    m.mie = frame.air_mie.rgb * d.y;
    m.extinction = extinction(d);
    return m;
}

// ---------------------------------------------------------------- look-up tables

// Texel-center mapping: x in [0, 1] covers the first to the last texel center.
fn unit_to_texel(x: f32, size: f32) -> f32 { return 0.5 / size + x * (1.0 - 1.0 / size); }
fn texel_to_unit(u: f32, size: f32) -> f32 { return (u - 0.5 / size) / (1.0 - 1.0 / size); }

// Transmittance table coordinates (Bruneton 2017's mapping) for height h, zenith cosine mu.
fn transmittance_lut_uv(h: f32, mu: f32) -> vec2<f32> {
    let top = frame.atmosphere.x;
    let hh = clamp(h, 0.0, top);
    let big_h = atmo_top_rho();
    let rho = atmo_rho(hh);
    let d = distance_to_top(hh, mu);
    let d_min = top - hh;
    let d_max = rho + big_h;
    let x_mu = (d - d_min) / max(d_max - d_min, 1e-3);
    return vec2<f32>(
        unit_to_texel(saturate(x_mu), TRANSMITTANCE_LUT_SIZE.x),
        unit_to_texel(rho / big_h, TRANSMITTANCE_LUT_SIZE.y),
    );
}

// (height, zenith cosine) at transmittance table coordinates.
fn transmittance_lut_params(uv: vec2<f32>) -> vec2<f32> {
    let radius = frame.planet.w;
    let top = frame.atmosphere.x;
    let x_mu = texel_to_unit(uv.x, TRANSMITTANCE_LUT_SIZE.x);
    let x_r = texel_to_unit(uv.y, TRANSMITTANCE_LUT_SIZE.y);
    let big_h = atmo_top_rho();
    let rho = big_h * x_r;
    let h = rho * rho / (sqrt(radius * radius + rho * rho) + radius);
    let d_min = top - h;
    let d_max = rho + big_h;
    let d = d_min + x_mu * (d_max - d_min);
    let c = (top - h) * (2.0 * radius + top + h);
    var mu = 1.0;
    if (d > 0.0) { mu = clamp((c - d * d) / (2.0 * (radius + h) * d), -1.0, 1.0); }
    return vec2<f32>(h, mu);
}

// Transmittance from height h (inside the atmosphere) to its top along zenith cosine mu.
// Meaningless for rays that hit the ground (test with `sun_above_horizon` or `ray_shell_mu`).
fn transmittance_to_top(h: f32, mu: f32) -> vec3<f32> {
    return textureSampleLevel(transmittance_lut, linear_clamp, transmittance_lut_uv(h, mu), 0.0).rgb;
}

// Transmittance from height h (anywhere, also above the atmosphere) to space.
fn transmittance_to_space(h: f32, mu: f32) -> vec3<f32> {
    let top = frame.atmosphere.x;
    if (h < top) { return transmittance_to_top(h, mu); }
    let hit = ray_shell_mu(h, mu, top);
    if (hit.x <= 0.0) { return vec3<f32>(1.0); }
    let mu_entry = clamp(((frame.planet.w + h) * mu + hit.x) / (frame.planet.w + top), -1.0, 1.0);
    return transmittance_to_top(top, mu_entry);
}

// The multiple scattering texture holds four tables of MULTISCATTER_LUT_SIZE over (sun zenith
// cosine, height), one above the other: all orders of multiple scattering, the two parts of its
// second order (sunlight scattered once by the air; sunlight off the ground), and the sky's light
// on the ground (`atmosphere_sky_irradiance`).
fn multiscatter_lut_uv(h: f32, mu_s: f32, table: f32) -> vec2<f32> {
    return vec2<f32>(
        unit_to_texel(mu_s * 0.5 + 0.5, MULTISCATTER_LUT_SIZE.x),
        (unit_to_texel(saturate(h / frame.atmosphere.x), MULTISCATTER_LUT_SIZE.y) + table) * 0.25,
    );
}

// Radiance of all scattering orders above the first, per unit scattering coefficient, averaged
// over all directions it arrives from.
fn multiscatter(h: f32, mu_s: f32) -> vec3<f32> {
    return textureSampleLevel(multiscatter_lut, linear_clamp, multiscatter_lut_uv(h, mu_s, 0.0), 0.0).rgb;
}

// `multiscatter` (total) and the two parts of its second order: sunlight the air scattered once
// (which arrives mostly from around the sun when the air scatters forward, as dust does) and
// sunlight off the ground (which arrives from below).
struct MultiScatter {
    total: vec3<f32>,
    sunlit_air: vec3<f32>,
    ground: vec3<f32>,
};

fn multiscatter_parts(h: f32, mu_s: f32) -> MultiScatter {
    var m: MultiScatter;
    m.total = multiscatter(h, mu_s);
    m.sunlit_air = textureSampleLevel(multiscatter_lut, linear_clamp, multiscatter_lut_uv(h, mu_s, 1.0), 0.0).rgb;
    m.ground = textureSampleLevel(multiscatter_lut, linear_clamp, multiscatter_lut_uv(h, mu_s, 2.0), 0.0).rgb;
    return m;
}

// The Henyey-Greenstein phase function per channel, times 4 pi (1 on average over directions).
fn atmo_lobe_hg_rgb(cos_t: vec3<f32>, g: vec3<f32>) -> vec3<f32> {
    let g2 = g * g;
    return (vec3<f32>(1.0) - g2) / pow(max(vec3<f32>(1.0) + g2 - 2.0 * g * cos_t, vec3<f32>(1e-4)), vec3<f32>(1.5));
}

// Light scattered toward the eye by the air (per unit scattering coefficient, phase
// anisotropy `g`) out of multiple scattering `ms`, for a view of zenith cosine `mu` there and
// cosine `nu` to the sun. Hillaire's tables take it as arriving evenly from all directions,
// which forward-scattering dust doesn't see: it scatters the light it gets from around the sun
// on toward directions near the sun, and the light from the ground below on upward. So the
// second order is scattered by where it comes from: the air's (in the sunlight's own phase,
// spread once more: a lobe of `g` times the forward-scattering share `g1` of what lit it; two
// Henyey-Greenstein lobes in a row make one of their product) and the ground's (from the lower
// hemisphere: the phase function's share of it), and only the higher orders are taken as even.
fn multiscatter_inscatter(ms: MultiScatter, mu: f32, nu: f32, g: vec3<f32>, g1: vec3<f32>) -> vec3<f32> {
    let rest = max(ms.total - ms.sunlit_air - ms.ground, vec3<f32>(0.0));
    let air = ms.sunlit_air * atmo_lobe_hg_rgb(vec3<f32>(nu), g * g1);
    // Share of the phase function over the lower hemisphere for a view of zenith cosine mu
    // (Legendre terms up to the third), times 2: the ground's light fills half the sphere.
    let x = -mu;
    let p3 = 0.5 * (5.0 * x * x * x - 3.0 * x);
    let below = clamp(vec3<f32>(0.5) + 0.75 * g * x - 0.4375 * g * g * g * p3, vec3<f32>(0.0), vec3<f32>(1.0));
    return rest + air + ms.ground * (2.0 * below);
}

// Sky light on an upward-facing surface at height h (all scattering orders, from the whole
// sky above it) for sun zenith cosine mu_s, per lux of sun and divided by pi: the radiance of
// a white diffuse surface it lights, as `sky_ambient` gives it.
fn atmosphere_sky_irradiance(h: f32, mu_s: f32) -> vec3<f32> {
    return textureSampleLevel(multiscatter_lut, linear_clamp, multiscatter_lut_uv(h, mu_s, 3.0), 0.0).rgb;
}

// Sky light (as `sky_ambient(n)`, without the ambient fill, per lux of sun) on a surface at
// camera-relative `rel` facing `n`, from the air around it rather than around the camera:
// the sky above it and the sunlit ground below it, by how much of each the surface faces.
fn atmosphere_sky_ambient(rel: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let h = planet_height(rel);
    let up = normalize(rel - frame.planet.xyz);
    let mu_s = dot(up, frame.sun_dir.xyz);
    let sky = atmosphere_sky_irradiance(h, mu_s);
    // The ground: lit by the sun and the sky, seen through the air below the surface (which
    // the sky term's ground-facing half leaves out; it is thin over the ground itself).
    let sun_on_ground = transmittance_to_top(0.0, mu_s) * sun_above_horizon(0.0, mu_s) * saturate(mu_s) * INV_PI;
    let ground = frame.ground.rgb * (sun_on_ground + atmosphere_sky_irradiance(0.0, mu_s));
    let c = dot(n, up);
    return sky * (0.5 + 0.5 * c) + ground * (0.5 - 0.5 * c);
}

// How much a surface at camera-relative `rel` takes its sky light from the air around itself
// (`atmosphere_sky_ambient`) rather than from the environment map, which holds the sky as seen
// from the camera: 0 for surfaces at about the camera's height or above it, 1 for surfaces a
// scale height of air or more below it. From orbit the environment map is black space above
// the planet, and the ground would lose all its sky light.
fn atmosphere_sky_ambient_weight(rel: vec3<f32>) -> f32 {
    if (frame.planet.w <= 0.0) { return 0.0; }
    let below = frame.camera_world.w - planet_height(rel);
    let scale = max(frame.atmosphere.y, frame.atmosphere.z);
    return smoothstep(0.25, 1.0, below / max(scale, 1.0));
}

// Sky-view table coordinates for a camera at height h (Hillaire's mapping: dense at the
// horizon and toward the sun). `light_view_cos` is the cosine of the azimuth between the view
// and the sun.
fn sky_view_lut_uv(h: f32, mu: f32, light_view_cos: f32) -> vec2<f32> {
    let r = frame.planet.w + h;
    let beta = acos(clamp(atmo_rho(h) / r, -1.0, 1.0));
    let zenith_horizon = PI - beta;
    let view_zenith = acos(clamp(mu, -1.0, 1.0));
    var v: f32;
    if (view_zenith < zenith_horizon) {
        v = (1.0 - sqrt(max(1.0 - view_zenith / zenith_horizon, 0.0))) * 0.5;
    } else {
        v = sqrt(saturate((view_zenith - zenith_horizon) / beta)) * 0.5 + 0.5;
    }
    let u = sqrt(saturate(-light_view_cos * 0.5 + 0.5));
    return vec2<f32>(unit_to_texel(u, SKY_VIEW_LUT_SIZE.x), unit_to_texel(v, SKY_VIEW_LUT_SIZE.y));
}

// (zenith cosine, light-view cosine) at sky-view table coordinates.
fn sky_view_lut_params(uv: vec2<f32>, h: f32) -> vec2<f32> {
    let u = saturate(texel_to_unit(uv.x, SKY_VIEW_LUT_SIZE.x));
    let v = saturate(texel_to_unit(uv.y, SKY_VIEW_LUT_SIZE.y));
    let r = frame.planet.w + h;
    let beta = acos(clamp(atmo_rho(h) / r, -1.0, 1.0));
    let zenith_horizon = PI - beta;
    var mu: f32;
    if (v < 0.5) {
        let x = 1.0 - 2.0 * v;
        mu = cos(zenith_horizon * (1.0 - x * x));
    } else {
        let x = v * 2.0 - 1.0;
        mu = cos(zenith_horizon + beta * x * x);
    }
    return vec2<f32>(mu, 1.0 - 2.0 * u * u);
}

// ---------------------------------------------------------------- scattering

struct AtmosphereSample {
    radiance: vec3<f32>,       // in-scattered radiance along the ray (per lux of sun color)
    transmittance: vec3<f32>,  // fraction of background light reaching the eye
    ground_t: f32,             // distance to the planet surface along the ray, or -1
};

// Light scattered toward the eye along a ray from height h0 with zenith cosine mu, between
// distances t0 and t1. mu_s is the sun's zenith cosine at the start, nu the cosine between the
// ray and the sun. Samples are spaced quadratically (dense near t0).
fn integrate_atmosphere(h0: f32, mu: f32, mu_s: f32, nu: f32, t0: f32, t1: f32, steps: i32) -> AtmosphereSample {
    return integrate_atmosphere_spaced(h0, mu, mu_s, nu, t0, t1, steps, 0.0);
}

// `integrate_atmosphere` with the samples dense near t0 (`dense_end` 0), near t1 (1, for rays
// that descend into the air, where it is densest at their end), or in between.
fn integrate_atmosphere_spaced(h0: f32, mu: f32, mu_s: f32, nu: f32, t0: f32, t1: f32, steps: i32, dense_end: f32) -> AtmosphereSample {
    var result: AtmosphereSample;
    result.ground_t = -1.0;
    let radius = frame.planet.w;
    let r0 = radius + h0;
    let pr = phase_rayleigh(nu);
    let pm = phase_mie_rgb(nu, frame.air_mie_g.rgb);
    let len = t1 - t0;
    var prev = 0.0;
    var throughput = vec3<f32>(1.0);
    var radiance = vec3<f32>(0.0);
    for (var i = 0; i < steps; i++) {
        let f = f32(i + 1) / f32(steps);
        let t = len * mix(f * f, 1.0 - (1.0 - f) * (1.0 - f), dense_end);
        let dt = t - prev;
        let tm = t0 + (prev + t) * 0.5;
        prev = t;
        let h = height_along(h0, mu, tm);
        let mu_s_t = clamp((r0 * mu_s + tm * nu) / (radius + h), -1.0, 1.0);
        let m = atmo_medium(h);
        let sun = transmittance_to_top(h, mu_s_t) * sun_above_horizon(h, mu_s_t);
        let ms = multiscatter_parts(h, mu_s_t);
        let mu_t = clamp((r0 * mu + tm) / (radius + h), -1.0, 1.0);
        let g1 = m.mie * frame.air_mie_g.rgb / max(m.mie + m.rayleigh, vec3<f32>(1e-20));
        let mie_ms = multiscatter_inscatter(ms, mu_t, nu, frame.air_mie_g.rgb, g1);
        let s = sun * (m.rayleigh * pr + m.mie * pm) + ms.total * m.rayleigh + mie_ms * m.mie;
        let seg = exp(-m.extinction * dt);
        radiance += throughput * (s - s * seg) / max(m.extinction, vec3<f32>(1e-12));
        throughput *= seg;
    }
    result.radiance = radiance;
    result.transmittance = throughput;
    return result;
}

// Integrate scattering from `origin` (camera-relative) along `dir` for up to `max_dist`
// meters: aerial perspective in front of geometry, or the whole sky.
fn atmosphere(origin: vec3<f32>, dir: vec3<f32>, max_dist: f32, steps: i32) -> AtmosphereSample {
    var result: AtmosphereSample;
    result.radiance = vec3(0.0);
    result.transmittance = vec3(1.0);
    result.ground_t = -1.0;
    if (frame.planet.w <= 0.0) { return result; }
    let h0 = planet_height(origin);
    let up = normalize(origin - frame.planet.xyz);
    let mu = dot(dir, up);
    let sun = frame.sun_dir.xyz;
    let hit_atmo = ray_shell_mu(h0, mu, frame.atmosphere.x);
    if (hit_atmo.y < 0.0) { return result; }
    let t0 = max(hit_atmo.x, 0.0);
    var t1 = min(hit_atmo.y, max_dist);
    let hit_ground = ray_shell_mu(h0, mu, 0.0);
    if (hit_ground.x > 0.0) {
        result.ground_t = hit_ground.x;
        t1 = min(t1, hit_ground.x);
    }
    if (t1 <= t0) { return result; }
    let ground_t = result.ground_t;
    // A ray that descends through a scale height of air or more (from orbit, from a high
    // aircraft) meets most of it near its end: sample it densely there.
    let h_start = select(h0, frame.atmosphere.x, t0 > 0.0);
    let descent = h_start - height_along(h0, mu, t1);
    let dense_end = smoothstep(0.5, 1.5, descent / max(max(frame.atmosphere.y, frame.atmosphere.z), 1.0));
    result = integrate_atmosphere_spaced(h0, mu, dot(sun, up), dot(dir, sun), t0, t1, steps, dense_end);
    result.ground_t = ground_t;
    return result;
}

// Sunlight transmittance to a camera-relative point through the atmosphere (0 in the planet's
// shadow).
fn sun_transmittance(rel: vec3<f32>) -> vec3<f32> {
    if (frame.planet.w <= 0.0) { return vec3(1.0); }
    let h = planet_height(rel);
    let mu_s = dot(normalize(rel - frame.planet.xyz), frame.sun_dir.xyz);
    return transmittance_to_space(h, mu_s) * sun_above_horizon(h, mu_s);
}

// The camera's height for the sky-view table, and whether the table is valid (the camera is
// inside the atmosphere; from above it the sky is integrated per pixel).
fn sky_view_height() -> f32 {
    return clamp(frame.camera_world.w, 1.0, frame.atmosphere.x - 1.0);
}

fn sky_view_valid() -> bool {
    return frame.planet.w > 0.0 && frame.camera_world.w < frame.atmosphere.x - 1.0;
}

// Sky radiance (per lux of sun) in direction `dir` from the camera, from the sky-view table.
// Includes the air in front of the ground for directions below the horizon.
fn sky_view_radiance(dir: vec3<f32>) -> vec3<f32> {
    let up = normalize(-frame.planet.xyz);
    let sun = frame.sun_dir.xyz;
    let mu = dot(dir, up);
    let dir_h = dir - up * mu;
    let sun_h = sun - up * dot(sun, up);
    let lens = length(dir_h) * length(sun_h);
    var light_view_cos = 1.0;
    if (lens > 1e-6) { light_view_cos = clamp(dot(dir_h, sun_h) / lens, -1.0, 1.0); }
    let uv = sky_view_lut_uv(sky_view_height(), mu, light_view_cos);
    return textureSampleLevel(sky_view_lut, linear_clamp, uv, 0.0).rgb;
}

// Transmittance from height h0 to the ground `t` meters away along zenith cosine mu (a ray
// that hits the ground), from the transmittance table.
fn ground_transmittance(h0: f32, mu: f32, t: f32) -> vec3<f32> {
    let radius = frame.planet.w;
    let mu_ground = clamp(((radius + h0) * mu + t) / radius, -1.0, 1.0);
    let to_top_from_ground = transmittance_to_top(0.0, -mu_ground);
    let to_top_from_eye = transmittance_to_top(h0, -mu);
    return min(to_top_from_ground / max(to_top_from_eye, vec3<f32>(1e-6)), vec3<f32>(1.0));
}
