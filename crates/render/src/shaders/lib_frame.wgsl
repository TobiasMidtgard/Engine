// ============================================================================
// Engine shader library: frame bindings (group 0) and lighting.
// Requires lib_types.wgsl. Group 0 is bound by the renderer before any feature draws.
// ============================================================================

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> point_lights: array<PointLight>;
@group(0) @binding(2) var shadow_map: texture_depth_2d_array;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;
@group(0) @binding(4) var linear_clamp: sampler;
@group(0) @binding(5) var env_map: texture_2d<f32>;   // equirect sky radiance (cd/m^2, not pre-exposed)
@group(0) @binding(6) var linear_repeat: sampler;

// Multiply final radiance by this before writing to the HDR target.
fn pre_expose(c: vec3<f32>) -> vec3<f32> { return c * frame.exposure.x; }

// Approximate world position (f32). Use camera-relative positions for precision-sensitive work.
fn world_pos(rel: vec3<f32>) -> vec3<f32> { return rel + frame.camera_world.xyz; }

// Stable world-space coordinates for texturing/noise near the camera (wraps every 4096 m).
fn tiling_pos(rel: vec3<f32>) -> vec3<f32> { return rel + frame.camera_world_frac.xyz; }

// Distance along the camera's forward axis (positive in front).
fn view_depth(rel: vec3<f32>) -> f32 {
    return -(frame.view * vec4<f32>(rel, 1.0)).z;
}

// Wind displacement for a vertex: `sway` meters at the top (`height01` = 1), none at the root.
fn wind_offset(rel: vec3<f32>, height01: f32, sway: f32) -> vec3<f32> {
    if (sway <= 0.0) { return vec3(0.0); }
    let w = frame.wind.xyz;
    let speed = length(w);
    let p = tiling_pos(rel);
    let t = frame.time.x;
    let phase = dot(p.xz, vec2(0.13, 0.17));
    let gust = 0.6 + 0.4 * sin(t * 0.7 + phase * 0.3) * sin(t * 1.9 + phase);
    let flutter = sin(t * 6.0 + phase * 3.0) * 0.15;
    let dir = select(vec3(1.0, 0.0, 0.0), w / max(speed, 1e-3), speed > 0.01);
    let strength = sway * height01 * height01 * min(speed / 10.0, 1.5);
    return dir * strength * (gust + flutter) + vec3(0.0, -0.1, 0.0) * strength * gust;
}

// ---------------------------------------------------------------- shadows
fn shadow_factor(rel: vec3<f32>, n: vec3<f32>) -> f32 {
    if (frame.sun_color.w < 0.5) { return 1.0; }
    let depth = view_depth(rel);
    var cascade = 0u;
    for (var i = 0u; i < 3u; i++) {
        if (depth > frame.shadow_splits[i]) { cascade = i + 1u; }
    }
    if (depth > frame.shadow_splits[3]) { return 1.0; }
    let map_size = frame.shadow_params.w;
    // World size of one shadow texel in this cascade (cascade matrices are orthographic).
    let m = frame.shadow_cascades[cascade];
    let texel_world = 2.0 / (map_size * length(vec3<f32>(m[0].x, m[1].x, m[2].x)));
    let n_dot_l = dot(n, frame.sun_dir.xyz);
    let normal_bias = frame.shadow_params.y * texel_world * (1.0 - 0.5 * saturate(n_dot_l));
    let p = rel + n * normal_bias;
    let lp = frame.shadow_cascades[cascade] * vec4<f32>(p, 1.0);
    let uv = lp.xy * vec2<f32>(0.5, -0.5) + 0.5;
    let z = lp.z - frame.shadow_params.x;
    if (any(uv < vec2(0.0)) || any(uv > vec2(1.0)) || z > 1.0) { return 1.0; }
    let texel = 1.0 / map_size;
    let radius = frame.shadow_params.z;
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2<f32>(f32(x), f32(y)) * texel * radius;
            sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + o, cascade, z);
        }
    }
    var s = sum / 9.0;
    // Fade out at the end of the last cascade.
    let fade = saturate((frame.shadow_splits[3] - depth) / (frame.shadow_splits[3] * 0.1));
    return mix(1.0, s, fade);
}

// ---------------------------------------------------------------- atmosphere (single scattering)
// Rayleigh + Mie scattering on a spherical planet. Works from the ground and from space.
const RAYLEIGH_BETA: vec3<f32> = vec3<f32>(5.802e-6, 13.558e-6, 33.1e-6);
const MIE_BETA: f32 = 3.996e-6;
const MIE_EXT: f32 = 4.40e-6;
const OZONE_BETA: vec3<f32> = vec3<f32>(0.650e-6, 1.881e-6, 0.085e-6);

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

// Ray vs the sphere `shell` meters above the planet surface, for a ray starting at height `h`.
// Numerically stable at planetary scale in f32 (no |oc|^2 - r^2 cancellation).
fn ray_shell(origin: vec3<f32>, h: f32, dir: vec3<f32>, shell: f32) -> vec2<f32> {
    let b = dot(origin - frame.planet.xyz, dir);
    let c = (h - shell) * (2.0 * frame.planet.w + h + shell);
    let d = b * b - c;
    if (d < 0.0) { return vec2<f32>(-1.0, -1.0); }
    let s = sqrt(d);
    var q = -b - select(1.0, -1.0, b < 0.0) * s;
    if (abs(q) < 1e-6) { q = 1e-6; }
    let t0 = q;
    let t1 = c / q;
    return vec2<f32>(min(t0, t1), max(t0, t1));
}

fn phase_rayleigh(cos_t: f32) -> f32 { return 3.0 / (16.0 * PI) * (1.0 + cos_t * cos_t); }

fn phase_mie(cos_t: f32, g: f32) -> f32 {
    let g2 = g * g;
    return 3.0 / (8.0 * PI) * ((1.0 - g2) * (1.0 + cos_t * cos_t))
        / ((2.0 + g2) * pow(max(1.0 + g2 - 2.0 * g * cos_t, 1e-4), 1.5));
}

fn ozone_density(h: f32) -> f32 {
    return max(0.0, 1.0 - abs(h - 25000.0) / 15000.0);
}

// Densities (rayleigh, mie, ozone) at height h.
fn atmo_density(h: f32) -> vec3<f32> {
    let hh = max(h, 0.0);
    return vec3<f32>(exp(-hh / frame.atmosphere.y), exp(-hh / frame.atmosphere.z), ozone_density(hh));
}

fn extinction(od: vec3<f32>) -> vec3<f32> {
    return RAYLEIGH_BETA * od.x + vec3(MIE_EXT) * od.y + OZONE_BETA * od.z;
}

// Optical depth (rayleigh, mie, ozone) from point p (height h) toward the sun. x < 0 when the
// planet blocks the sun.
fn optical_depth_to_sun(p: vec3<f32>, h: f32, sun: vec3<f32>, samples: i32) -> vec3<f32> {
    if (ray_shell(p, h, sun, -50.0).x > 0.0) { return vec3<f32>(-1.0); }
    let len = max(ray_shell(p, h, sun, frame.atmosphere.x).y, 0.0);
    var od = vec3<f32>(0.0);
    var prev = 0.0;
    for (var j = 0; j < samples; j++) {
        let f = f32(j + 1) / f32(samples);
        let t = len * f * f;
        let tm = (prev + t) * 0.5;
        od += atmo_density(planet_height(p + sun * tm)) * (t - prev);
        prev = t;
    }
    return od;
}

struct AtmosphereSample {
    radiance: vec3<f32>,       // in-scattered radiance along the ray (per lux of sun color)
    transmittance: vec3<f32>,  // fraction of background light reaching the eye
    ground_t: f32,             // distance to the planet surface along the ray, or -1
};

const MULTI_SCATTER: f32 = 1.6;

// Integrate scattering from `origin` (camera-relative) along `dir` for up to `max_dist` meters.
// Samples are spaced quadratically (dense near the eye, sparse far away).
fn atmosphere(origin: vec3<f32>, dir: vec3<f32>, max_dist: f32, steps: i32) -> AtmosphereSample {
    var result: AtmosphereSample;
    result.radiance = vec3(0.0);
    result.transmittance = vec3(1.0);
    result.ground_t = -1.0;
    let r_planet = frame.planet.w;
    if (r_planet <= 0.0) { return result; }
    let h0 = planet_height(origin);
    let hit_atmo = ray_shell(origin, h0, dir, frame.atmosphere.x);
    if (hit_atmo.y < 0.0) { return result; }
    let t0 = max(hit_atmo.x, 0.0);
    var t1 = min(hit_atmo.y, max_dist);
    let hit_ground = ray_shell(origin, h0, dir, 0.0);
    if (hit_ground.x > 0.0) {
        result.ground_t = hit_ground.x;
        t1 = min(t1, hit_ground.x);
    }
    if (t1 <= t0) { return result; }

    let sun = frame.sun_dir.xyz;
    let cos_t = dot(dir, sun);
    let pr = phase_rayleigh(cos_t);
    let pm = phase_mie(cos_t, frame.atmosphere.w);
    let len = t1 - t0;
    var od = vec3<f32>(0.0);
    var sum_r = vec3(0.0);
    var sum_m = vec3(0.0);
    var prev = 0.0;
    for (var i = 0; i < steps; i++) {
        let f = f32(i + 1) / f32(steps);
        let tt = len * f * f;
        let seg = tt - prev;
        let p = origin + dir * (t0 + (prev + tt) * 0.5);
        prev = tt;
        let dens = atmo_density(planet_height(p)) * seg;
        let od_mid = od + dens * 0.5;
        od += dens;
        let sun_od = optical_depth_to_sun(p, planet_height(p), sun, 4);
        if (sun_od.x < 0.0) { continue; }
        let att = exp(-extinction(od_mid + sun_od));
        sum_r += att * dens.x;
        sum_m += att * dens.y;
    }
    result.transmittance = exp(-extinction(od));
    let scatter_r = sum_r * RAYLEIGH_BETA;
    let scatter_m = sum_m * MIE_BETA;
    // Single scattering + a cheap isotropic multiple-scattering term.
    result.radiance = scatter_r * pr + scatter_m * pm + (scatter_r + scatter_m) * (MULTI_SCATTER / (4.0 * PI));
    return result;
}

// Sunlight transmittance to a camera-relative point through the atmosphere.
fn sun_transmittance(rel: vec3<f32>) -> vec3<f32> {
    if (frame.planet.w <= 0.0) { return vec3(1.0); }
    let od = optical_depth_to_sun(rel, planet_height(rel), frame.sun_dir.xyz, 6);
    if (od.x < 0.0) { return vec3(0.0); }
    return exp(-extinction(od));
}

// ---------------------------------------------------------------- environment lighting
fn sample_env(dir: vec3<f32>) -> vec3<f32> {
    return textureSampleLevel(env_map, linear_repeat, dir_to_equirect(dir), 0.0).rgb;
}

// Light bounced off the ground below (cd/m^2): sky from above plus sun, times ground albedo.
// Fades out high above the planet (frame.ground.w).
fn ground_radiance() -> vec3<f32> {
    let sky_up = sample_env(vec3<f32>(0.0, 1.0, 0.0));
    let sun = frame.sun_camera.rgb * saturate(frame.sun_dir.y) * INV_PI;
    return frame.ground.rgb * frame.ground.w * (sky_up + sun);
}

// Approximate cosine-weighted sky radiance (cd/m^2) around normal `n`, plus ambient fill.
fn sky_ambient(n: vec3<f32>) -> vec3<f32> {
    let up = sample_env(normalize(n + vec3(0.0, 0.6, 0.0)));
    let side = sample_env(normalize(vec3(n.x, 0.15, n.z) + vec3(0.0001)));
    let sky = mix(side, up, 0.6);
    let w = saturate(n.y * 0.5 + 0.5);
    return mix(ground_radiance(), sky, w) + frame.ambient.rgb;
}

// Fraction of the open sky seen from camera-relative `rel` with normal `n`: 1 outdoors, toward 0
// in caves, rooms and under overhangs. Multiply sky/ambient light by it. Also the test for
// "is this spot exposed to rain and snow". Uses the top-down sky occlusion map around the
// camera; returns 1 where the map is off or doesn't reach.
fn sky_visibility(rel: vec3<f32>, n: vec3<f32>) -> f32 {
    if (frame.sky_occlusion_params.x < 0.5) { return 1.0; }
    return 1.0;
}

// ---------------------------------------------------------------- surface shading
// Returns pre-exposed radiance for a lit surface at camera-relative position `rel`.
fn shade_surface(s: Surface, rel: vec3<f32>) -> vec3<f32> {
    let v = normalize(-rel);
    let n = s.normal;
    let roughness = s.roughness * s.roughness;
    let n_dot_v = max(dot(n, v), 1e-4);
    let f0 = mix(vec3(0.16 * s.reflectance * s.reflectance), s.albedo, s.metallic);
    let diffuse_color = s.albedo * (1.0 - s.metallic);
    var color = vec3(0.0);

    // Sun.
    let l = frame.sun_dir.xyz;
    let n_dot_l_raw = dot(n, l);
    let n_dot_l = saturate(n_dot_l_raw);
    let sun_light = frame.sun_color.rgb * sun_transmittance(rel);
    if (dot(sun_light, sun_light) > 0.0) {
        let shadow = shadow_factor(rel, n);
        let h = normalize(l + v);
        let n_dot_h = saturate(dot(n, h));
        let l_dot_h = saturate(dot(l, h));
        let spec = d_ggx(n_dot_h, roughness) * v_smith_ggx_correlated(n_dot_v, n_dot_l, roughness)
            * f_schlick(f0, l_dot_h);
        let wrap = saturate((n_dot_l_raw + s.subsurface) / ((1.0 + s.subsurface) * (1.0 + s.subsurface)));
        let diffuse = diffuse_color * INV_PI * select(n_dot_l, wrap, s.subsurface > 0.0);
        color += (diffuse + spec * n_dot_l) * sun_light * shadow;
        // Back-lit translucency (leaves glowing against the sun).
        if (s.subsurface > 0.0) {
            let back = pow(saturate(dot(v, -l)), 4.0) * s.subsurface * 0.35;
            color += diffuse_color * back * sun_light * shadow * INV_PI;
        }
    }

    // Point lights.
    let count = frame.counts.x;
    for (var i = 0u; i < count; i++) {
        let light = point_lights[i];
        let to_light = light.position_range.xyz - rel;
        let dist2 = dot(to_light, to_light);
        let range = light.position_range.w;
        if (dist2 > range * range) { continue; }
        let dist = sqrt(dist2);
        let pl = to_light / max(dist, 1e-4);
        let nl = saturate(dot(n, pl));
        if (nl <= 0.0 && s.subsurface <= 0.0) { continue; }
        let falloff = sq(saturate(1.0 - sq(dist2 / (range * range)))) / max(dist2, sq(light.color_radius.w));
        let h = normalize(pl + v);
        let spec = d_ggx(saturate(dot(n, h)), max(roughness, 0.02)) *
            v_smith_ggx_correlated(n_dot_v, nl, roughness) * f_schlick(f0, saturate(dot(pl, h)));
        let wrap = saturate((dot(n, pl) + s.subsurface) / (1.0 + s.subsurface));
        color += (diffuse_color * INV_PI * max(nl, wrap * s.subsurface) + spec * nl) * light.color_radius.rgb * falloff;
    }

    // Sky / ambient (occluded in caves and interiors; the constant fill stays).
    let sky_vis = sky_visibility(rel, n);
    let fill = frame.ambient.rgb;
    let irradiance = ((sky_ambient(n) - fill) * sky_vis + fill) * PI;
    let r = reflect(-v, n);
    let spec_env = (mix(sample_env(r), sky_ambient(r), saturate(roughness * 1.5)) - fill) * sky_vis + fill;
    let env_brdf = env_brdf_approx(f0, s.roughness, n_dot_v);
    color += (diffuse_color * irradiance * INV_PI + spec_env * env_brdf) * s.occlusion;

    color += s.emissive;
    return pre_expose(color);
}

// ---------------------------------------------------------------- fog / aerial perspective
// Height used by fog: altitude above the planet surface, or world y without a planet.
fn fog_height(rel: vec3<f32>) -> f32 {
    if (frame.planet.w > 0.0) { return planet_height(rel); }
    return frame.camera_world.y + rel.y;
}

// Applies height fog and atmospheric haze to pre-exposed color seen at camera-relative `rel`.
fn apply_fog(color: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    let dist = length(rel);
    let dir = rel / max(dist, 1e-4);
    // Height fog: analytic integral of density * exp(-falloff * height) along the ray. Heights
    // are altitudes above the planet surface (world y when there is no planet). Stable for
    // any distance and altitude (orbit, the Moon): both exponents are clamped.
    let density = frame.fog.x;
    let falloff = max(frame.fog.y, 1e-5);
    let h0 = fog_height(vec3(0.0)) - frame.fog.z;
    let h1 = fog_height(rel) - frame.fog.z;
    let e0 = exp(-falloff * max(h0, -100.0));
    let e1 = exp(-falloff * max(h1, -100.0));
    let dh = h1 - h0;
    var amount = density * dist * e0;
    if (abs(dh) > 0.01) {
        amount = density * dist * (e0 - e1) / (falloff * dh);
    }
    let fog_t = min(1.0 - exp(-amount), frame.fog.w);
    let sun_amt = pow(saturate(dot(dir, frame.sun_dir.xyz)), 8.0) * frame.fog_color.w;
    // Fog is lit by the sky near the horizon and by forward-scattered sun; fog_color is its albedo.
    let horizon = sample_env(normalize(vec3<f32>(dir.x, 0.05, dir.z) + vec3(0.0001)));
    let fog_col = frame.fog_color.rgb * (horizon + frame.ambient.rgb + frame.sun_camera.rgb * sun_amt * INV_PI * 0.05);
    var c = mix(color, pre_expose(fog_col), fog_t);
    // Aerial perspective: cheap atmosphere integral for distant geometry.
    if (dist > 500.0) {
        let a = atmosphere(vec3(0.0), dir, dist, 6);
        c = c * a.transmittance + pre_expose(a.radiance * frame.sun_color.rgb);
    }
    return c;
}
