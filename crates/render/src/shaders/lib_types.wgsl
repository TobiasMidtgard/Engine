// ============================================================================
// Engine shader library: shared types and pure functions (no bindings).
// Every render feature's shader is composed as: lib_types + [lib_frame] + own code.
// ============================================================================

const PI: f32 = 3.14159265359;
const TAU: f32 = 6.28318530718;
const INV_PI: f32 = 0.31830988618;

// Per-frame data. Matches `FrameUniform` in frame.rs. All positions are CAMERA-RELATIVE:
// the camera sits at the origin; add `camera_world` for (low-precision) world positions.
struct Frame {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    inv_view: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    prev_view_proj: mat4x4<f32>,
    camera_world: vec4<f32>,       // xyz world position (f32, approximate), w altitude above sea level
    camera_world_frac: vec4<f32>,  // camera position modulo 4096 m: stable world-space tiling coordinates
    viewport: vec4<f32>,           // width, height, 1/width, 1/height
    time: vec4<f32>,               // world seconds, real seconds, world delta, frame index
    sun_dir: vec4<f32>,            // direction TOWARD the sun, w = angular radius (radians)
    sun_color: vec4<f32>,          // linear RGB * illuminance (lux), w = shadows enabled (1/0)
    sun_camera: vec4<f32>,         // sun illuminance reaching the camera through the atmosphere (lux RGB)
    ambient: vec4<f32>,            // extra ambient fill radiance (cd/m^2) RGB, w = ground albedo
    fog: vec4<f32>,                // density, height falloff, fog base altitude (m above the planet surface; world y without one), max opacity
    fog_color: vec4<f32>,          // fog albedo RGB (lit by sky and sun), w = sun scattering strength
    wind: vec4<f32>,               // wind velocity (m/s) xyz, w = gust phase
    weather: vec4<f32>,            // wetness, snow cover, cloud cover, precipitation (mm/h)
    planet: vec4<f32>,             // planet center relative to camera (xyz), radius (m)
    atmosphere: vec4<f32>,         // atmosphere thickness (m), rayleigh scale height, mie scale height, mie g
    exposure: vec4<f32>,           // pre-exposure multiplier, ev100, near plane, 1 if orthographic
    shadow_cascades: array<mat4x4<f32>, 4>,  // camera-relative -> light clip space
    shadow_splits: vec4<f32>,      // far view distance of each cascade
    shadow_params: vec4<f32>,      // depth bias, normal bias (texels), pcf radius (texels), map size
    counts: vec4<u32>,             // point light count, cascade count, debug view, sky volume count
    ground: vec4<f32>,             // planet surface albedo RGB (beyond the terrain), w = ground-bounce weight (fades with altitude)
    sky_occlusion: mat4x4<f32>,    // camera-relative -> top-down sky occlusion map clip space (standard Z, 0 = top)
    sky_occlusion_params: vec4<f32>, // enabled (1/0), covered width (m), texel size (m), depth range (m)
    sky_volumes: array<SkyVolume, 16>, // indoor volumes, nearest first (`counts.w` used)
    unjittered_view_proj: mat4x4<f32>, // view_proj without the TAA jitter (motion vectors)
    jitter: vec4<f32>,             // TAA jitter: NDC xy, pixels zw
    camera_delta: vec4<f32>,       // camera origin - previous origin (m), w = 1 if the previous frame is valid history
    effects: vec4<f32>,            // AO intensity (0 = off), contact shadow length (m), sun penumbra scale, AO radius (m)
    reflections: vec4<f32>,        // screen-space reflections this frame (1/0), previous frame's pre-exposure (0 = none), max perceptual roughness, screen-space GI strength (0 = off)
    volumetrics: vec4<f32>,        // volumetric fog range (m, 0 = off), 1 / depth scale (1/m), 1 / log2(1 + range / depth scale), depth slices
    far_shadow: mat4x4<f32>,       // camera-relative -> far shadow map clip space (big casters beyond the cascades)
    far_shadow_params: vec4<f32>,  // reach (m, 0 = off), texel size (m), clip depth per meter, shadow map layer
    sky: vec4<f32>,                // the sky's flat cloud layer on (1/0), night (0 day .. 1 night, from the main light's illuminance at the camera), stand-in moon glow opposite a sun below the horizon (0..1), unused
    surface_map: vec4<f32>,        // ground weather map (`surface_weather`): min corner relative to the camera (x, z, m), size along x and z (m, 0 = none)
    surface_open: vec4<f32>,       // the open ground's weather in the map's encoding: beyond the map and on roofs over ground they keep dry
    cloud_shadow: mat4x4<f32>,     // camera-relative -> cloud shadow map clip space, projected along the sun onto the cloud base (`cloud_shadow`)
    cloud_shadow_params: vec4<f32>, // drawn this frame (1/0), cloud base height relative to the camera (m), cloud layer thickness (m), sun transmittance beyond the map
    cloud_shadow_area: vec4<f32>,  // camera-relative x and z of the map corner at uv (0, 0) (m), side (m), texels per side
};

// An indoor space (`SkyLightVolume`): sky light comes in only through its open faces.
struct SkyVolume {
    to_local: array<vec4<f32>, 3>, // rows of camera-relative -> box-local (meters, no scale)
    half_fade: vec4<f32>,          // half extents xyz (m), fade distance from open faces (m)
    params: vec4<f32>,             // sky light left inside, open face mask (+x -x +y -y +z -z bits), unused, unused
};

struct PointLight {
    position_range: vec4<f32>,  // camera-relative position, range (m)
    color_radius: vec4<f32>,    // linear RGB * luminous intensity (cd), source radius (m)
};

fn saturate(x: f32) -> f32 { return clamp(x, 0.0, 1.0); }
fn saturate3(x: vec3<f32>) -> vec3<f32> { return clamp(x, vec3(0.0), vec3(1.0)); }
fn luminance(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }
fn sq(x: f32) -> f32 { return x * x; }

// ---------------------------------------------------------------- hashing & noise
fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn hash13(p: vec3<f32>) -> f32 {
    var p3 = fract(p * 0.1031);
    p3 += dot(p3, p3.zyx + 31.32);
    return fract((p3.x + p3.y) * p3.z);
}

// Interleaved gradient noise (Jimenez 2014): cheap per-pixel noise that a 3x3 neighborhood
// (or temporal anti-aliasing) averages out well. `pixel` in pixels.
fn interleaved_gradient_noise(pixel: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(pixel, vec2<f32>(0.06711056, 0.00583715))));
}

// Octahedral encoding of a unit vector into [-1, 1]^2 (precise in 16-bit floats).
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if (n.z >= 0.0) { return p; }
    return (1.0 - abs(p.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0));
}

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e, 1.0 - abs(e.x) - abs(e.y));
    let t = saturate(-n.z);
    n.x += select(t, -t, n.x >= 0.0);
    n.y += select(t, -t, n.y >= 0.0);
    return normalize(n);
}

fn value_noise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash12(i), hash12(i + vec2(1.0, 0.0)), u.x),
               mix(hash12(i + vec2(0.0, 1.0)), hash12(i + vec2(1.0, 1.0)), u.x), u.y);
}

fn value_noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = mix(mix(hash13(i), hash13(i + vec3(1.0, 0.0, 0.0)), u.x),
                mix(hash13(i + vec3(0.0, 1.0, 0.0)), hash13(i + vec3(1.0, 1.0, 0.0)), u.x), u.y);
    let b = mix(mix(hash13(i + vec3(0.0, 0.0, 1.0)), hash13(i + vec3(1.0, 0.0, 1.0)), u.x),
                mix(hash13(i + vec3(0.0, 1.0, 1.0)), hash13(i + vec3(1.0, 1.0, 1.0)), u.x), u.y);
    return mix(a, b, u.z);
}

fn fbm2(p: vec2<f32>, octaves: i32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < octaves; i++) {
        sum += amp * value_noise2(q);
        q = q * 2.03 + vec2(17.1, 9.2);
        amp *= 0.5;
    }
    return sum;
}

fn fbm3(p: vec3<f32>, octaves: i32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < octaves; i++) {
        sum += amp * value_noise3(q);
        q = q * 2.03 + vec3(17.1, 9.2, 3.7);
        amp *= 0.5;
    }
    return sum;
}

// ---------------------------------------------------------------- PBR (Filament-style)
fn d_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = n_dot_h * roughness;
    let k = roughness / max(1.0 - n_dot_h * n_dot_h + a * a, 1e-5);
    return k * k * INV_PI;
}

fn v_smith_ggx_correlated(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    let a2 = roughness * roughness;
    let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-5);
}

// Henyey-Greenstein phase function (1/sr): the share of light scattered by a medium at angle
// acos(cos_t) from its direction of travel. g > 0 scatters forward (fog glows around the sun and
// lamps you look toward), g = 0 evenly, g < 0 back.
fn henyey_greenstein(cos_t: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * PI * pow(max(1.0 + g2 - 2.0 * g * cos_t, 1e-4), 1.5));
}

fn f_schlick(f0: vec3<f32>, v_dot_h: f32) -> vec3<f32> {
    let f = pow(1.0 - v_dot_h, 5.0);
    return f + f0 * (1.0 - f);
}

// Analytic approximation of the split-sum environment BRDF (Karis, mobile).
fn env_brdf_approx(f0: vec3<f32>, roughness: f32, n_dot_v: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * n_dot_v)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

// Inputs to `shade_surface` (lib_frame.wgsl).
struct Surface {
    albedo: vec3<f32>,
    metallic: f32,
    roughness: f32,      // perceptual roughness
    reflectance: f32,    // dielectric F0 control (0.5 = 4%)
    normal: vec3<f32>,   // world-space unit normal
    emissive: vec3<f32>, // radiance (cd/m^2), not yet pre-exposed
    occlusion: f32,
    subsurface: f32,     // 0..1 light wrap for foliage/skin
    // The normal without normal-map or detail bumps (the interpolated vertex normal, on the
    // same side as `normal`), for large-scale lookups such as how much sky the surface sees:
    // cover doesn't change wrinkle by wrinkle. Zero (the default) means `normal`.
    smooth_normal: vec3<f32>,
};

fn default_surface() -> Surface {
    return Surface(vec3(0.8), 0.0, 0.6, 0.5, vec3(0.0, 1.0, 0.0), vec3(0.0), 1.0, 0.0, vec3(0.0));
}

// `s.smooth_normal` as a unit vector, or `s.normal` when it is unset.
fn surface_smooth_normal(s: Surface) -> vec3<f32> {
    let m = s.smooth_normal;
    let len2 = dot(m, m);
    return select(s.normal, m * inverseSqrt(max(len2, 1e-12)), len2 > 1e-8);
}

// Direction <-> equirectangular UV (u around +Y starting at -Z, v from +Y down).
fn dir_to_equirect(d: vec3<f32>) -> vec2<f32> {
    let u = atan2(d.x, -d.z) * (0.5 * INV_PI) + 0.5;
    let v = acos(clamp(d.y, -1.0, 1.0)) * INV_PI;
    return vec2<f32>(u, v);
}

fn equirect_to_dir(uv: vec2<f32>) -> vec3<f32> {
    let phi = (uv.x - 0.5) * TAU;
    let theta = uv.y * PI;
    return vec3<f32>(sin(theta) * sin(phi), cos(theta), -sin(theta) * cos(phi));
}

// Fullscreen triangle: call with 3 vertices, no buffers.
struct FullscreenOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

fn fullscreen_vertex(index: u32, depth: f32) -> FullscreenOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: FullscreenOut;
    out.position = vec4<f32>(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), depth, 1.0);
    out.uv = uv;
    return out;
}
