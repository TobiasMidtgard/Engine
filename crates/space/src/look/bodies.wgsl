// Far views of planets and moons. Each body is one quad around its silhouette (or a
// fullscreen triangle pair when the camera is close); the fragment shader ray-traces the
// spheroid, its rings and its haze, so the limb is exactly round and anti-aliased at any
// size. Requires lib_types + lib_frame.
//
// Body-local frame: +Y is the north pole, longitude 0 is +Z, east is +X. Lengths in the
// fragment shader are in units of the equatorial radius unless named `_m`.

struct Body {
    center: vec4<f32>,  // camera-relative center (m), w: bounding radius of everything drawn (m)
    axis_x: vec4<f32>,  // body +X in world axes (unit), w: equatorial radius (m)
    axis_y: vec4<f32>,  // body +Y (north), w: polar radius (m)
    axis_z: vec4<f32>,  // body +Z, w: radius of the sphere written to depth (m)
    sun: vec4<f32>,     // direction toward the sun in body axes, w: cloud drift (map u offset)
    light: vec4<f32>,   // sun illuminance at the body (lux, linear RGB), w: relief strength
    shade: vec4<f32>,   // minnaert exponent, lunar-lambert weight, glint strength, cloud height (R)
    haze: vec4<f32>,    // relative extinction per channel, w: vertical optical depth
    haze2: vec4<f32>,   // single-scattering albedo per channel, w: scale height (R)
    haze3: vec4<f32>,   // x: forward scattering g, yzw unused
    rings: vec4<f32>,   // inner radius (R), outer radius (R), brightness, unused
    flags: vec4<u32>,   // x: F_* bits
};

const F_RINGS: u32 = 1u;
const F_CLOUDS: u32 = 2u;
const F_RELIEF: u32 = 4u;
const F_AIR: u32 = 8u;         // the renderer's atmosphere is this body's: use it for the haze
const F_FULLSCREEN: u32 = 16u;
const F_HAZE: u32 = 32u;
const F_SOLID_AIR: u32 = 64u;  // unused

@group(1) @binding(0) var<storage, read> bodies: array<Body>;
@group(1) @binding(1) var map_sampler: sampler;   // repeat in u, clamp in v, trilinear, anisotropic
@group(1) @binding(2) var ring_sampler: sampler;  // clamp, trilinear
@group(2) @binding(0) var albedo_map: texture_2d<f32>;
@group(2) @binding(1) var aux_map: texture_2d<f32>;
@group(2) @binding(2) var ring_map: texture_2d<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ray: vec3<f32>,
    @location(1) @interpolate(flat) body: u32,
};

// Angular size of one pixel (radians), at the center of the view.
fn pixel_angle() -> f32 {
    return 2.0 / (frame.proj[1][1] * frame.viewport.y);
}

@vertex
fn vs_body(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let c = corners[vi];
    let b = bodies[ii];
    var out: VsOut;
    out.body = ii;
    if ((b.flags.x & F_FULLSCREEN) != 0u) {
        out.clip = vec4<f32>(c, 0.0, 1.0);
        let p = frame.inv_view_proj * vec4<f32>(c, 1.0, 1.0);
        out.ray = p.xyz / p.w;
        return out;
    }
    let center = b.center.xyz;
    let d = length(center);
    let fwd = center / d;
    var up = vec3<f32>(0.0, 1.0, 0.0);
    if (abs(fwd.y) > 0.9) { up = vec3<f32>(1.0, 0.0, 0.0); }
    let right = normalize(cross(fwd, up));
    let up2 = cross(right, fwd);
    let rb = b.center.w;
    // The quad through the center that holds the cone around the bounding sphere, plus a few
    // pixels for the anti-aliased edge.
    let cone = rb * d / sqrt(max(d * d - rb * rb, 1e-4 * d * d));
    let half = cone + 3.0 * pixel_angle() * d;
    let world = center + (right * c.x + up2 * c.y) * half;
    out.clip = frame.view_proj * vec4<f32>(world, 1.0);
    out.ray = world;
    return out;
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

// Henyey-Greenstein phase, normalized so an even scatterer is 1.
fn phase_hg(cos_t: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / pow(max(1.0 + g2 - 2.0 * g * cos_t, 1e-4), 1.5);
}

// Chapman function: air mass along a ray leaving the surface at cos(zenith) `mu` through an
// exponential atmosphere with radius/scale height `x`.
fn chapman(mu: f32, x: f32) -> f32 {
    return 1.0 / (max(mu, 0.0) + 1.0 / sqrt(PI * x * 0.5));
}

fn depth_of(rel: vec3<f32>) -> f32 {
    let clip = frame.view_proj * vec4<f32>(rel, 1.0);
    return clamp(clip.z / clip.w, 0.0, 1.0);
}

// Ring opacity (face-on) at radius r (R): sampled from the ring profile at a blurred level.
fn ring_alpha_at(b: Body, r: f32, level: f32) -> f32 {
    let u = (r - b.rings.x) / (b.rings.y - b.rings.x);
    if (u < 0.0 || u > 1.0) { return 0.0; }
    return textureSampleLevel(ring_map, ring_sampler, vec2<f32>(u, 0.5), level).a;
}

@fragment
fn fs_body(in: VsOut) -> FsOut {
    let b = bodies[in.body];
    let flags = b.flags.x;
    let dir = normalize(in.ray);
    let r_eq = b.axis_x.w;
    let e = r_eq / b.axis_y.w;
    let ax = b.axis_x.xyz;
    let ay = b.axis_y.xyz;
    let az = b.axis_z.xyz;

    // Camera and ray in body axes, units of the equatorial radius.
    let ow = -b.center.xyz / r_eq;
    let o = vec3<f32>(dot(ow, ax), dot(ow, ay), dot(ow, az));
    let dl = vec3<f32>(dot(dir, ax), dot(dir, ay), dot(dir, az));
    let v = -dl;

    // Spheroid -> unit sphere (stretch y). The closest point of the ray to the center is
    // computed directly (no |o|² - 1, which f32 can't hold for far bodies).
    let os = vec3<f32>(o.x, o.y * e, o.z);
    let ds_raw = vec3<f32>(dl.x, dl.y * e, dl.z);
    let ds_len = length(ds_raw);
    let ds = ds_raw / ds_len;
    let bq = dot(os, ds);
    let q = os - bq * ds;
    let q_len = length(q);
    let h = 1.0 - q_len * q_len;
    let outside = dot(os, os) > 1.0;
    let ahead = bq < 0.0;
    // Surface point on the unit sphere; rays that just miss use the limb point.
    let ps = select(q / max(q_len, 1e-6), q - sqrt(max(h, 0.0)) * ds, h > 0.0);
    let t_surf = (-bq - sqrt(max(h, 0.0))) / ds_len;  // along dl, R
    // Anti-aliased coverage of the disk.
    let pix = pixel_angle() * max(-bq, 1e-3);
    var cover = clamp(0.5 - (q_len - 1.0) / pix, 0.0, 1.0);
    if (!outside || !ahead) { cover = 0.0; }

    // Map coordinates and seam-free derivatives (uniform control flow from here to sampling).
    let lon = atan2(ps.x, ps.z);
    let lat = asin(clamp(ps.y, -1.0, 1.0));
    let u1 = lon * (0.5 * INV_PI) + 0.5;
    let u2 = fract(u1 + 0.5);
    let vv = 0.5 - lat * INV_PI;
    let uv = vec2<f32>(u1, vv);
    let du1 = vec2<f32>(dpdx(u1), dpdy(u1));
    let du2 = vec2<f32>(dpdx(u2), dpdy(u2));
    let du = select(du1, du2, abs(du2) < abs(du1));
    let dv = vec2<f32>(dpdx(vv), dpdy(vv));
    let gx = vec2<f32>(du.x, dv.x);
    let gy = vec2<f32>(du.y, dv.y);

    // Rings: where the ray crosses the equatorial plane.
    let t_ring = -o.y / select(dl.y, 1e-6, abs(dl.y) < 1e-6);
    let pr = o + dl * t_ring;
    let r_ring = length(pr.xz);
    let ring_u = (r_ring - b.rings.x) / max(b.rings.y - b.rings.x, 1e-6);
    let ring_g = vec2<f32>(dpdx(ring_u), dpdy(ring_u));

    let albedo_s = textureSampleGrad(albedo_map, map_sampler, uv, gx, gy);
    let aux_s = textureSampleGrad(aux_map, map_sampler, uv, gx, gy);
    let cloud_uv = vec2<f32>(u1 + b.sun.w, vv);
    let cloud_s = textureSampleGrad(aux_map, map_sampler, cloud_uv, gx, gy).r;
    let ring_s = textureSampleGrad(ring_map, ring_sampler, vec2<f32>(ring_u, 0.5),
                                   vec2<f32>(ring_g.x, 0.0), vec2<f32>(ring_g.y, 0.0));

    let s = normalize(b.sun.xyz);
    let sun_e = b.light.rgb;
    let n = normalize(vec3<f32>(ps.x, ps.y * e, ps.z));
    let p_l = vec3<f32>(ps.x, ps.y / e, ps.z);  // surface point, body axes
    let rel_hit = dir * (t_surf * r_eq);

    // ---------------------------------------------------------------- surface
    var surf = vec3<f32>(0.0);
    if (cover > 0.0) {
        var albedo = albedo_s.rgb;
        var light = sun_e;
        // Relief: tilt the normal by the baked slopes (east, north).
        var nl = n;
        if ((flags & F_RELIEF) != 0u) {
            let east = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), n) + vec3<f32>(1e-6, 0.0, 0.0));
            let north = cross(n, east);
            let slope = (aux_s.gb - 0.5) / 0.25;
            nl = normalize(n - b.light.w * (slope.x * east + slope.y * north));
        }
        let mu0 = dot(nl, s);
        let mu0_smooth = dot(n, s);
        let mu = max(dot(n, v), 0.03);
        // Ring shadow on the planet.
        if ((flags & F_RINGS) != 0u && abs(s.y) > 1e-4) {
            let t = -p_l.y / s.y;
            if (t > 0.0) {
                let a = ring_alpha_at(b, length(p_l.xz + s.xz * t), 2.0);
                let tau = -log(max(1.0 - a, 0.003));
                light *= exp(-tau / abs(s.y));
            }
        }
        // Haze or air in the way of the sunlight: a redder, softer terminator.
        if ((flags & F_AIR) != 0u) {
            light *= sun_transmittance(rel_hit);
        } else if ((flags & F_HAZE) != 0u) {
            let x = 1.0 / max(b.haze2.w, 1e-6);
            light *= exp(-b.haze.rgb * b.haze.w * chapman(mu0_smooth, x));
        }
        // Clouds (Earth): a layer above the ground that also shades it.
        var cloud = 0.0;
        if ((flags & F_CLOUDS) != 0u) {
            cloud = cloud_s;
            let east = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), n) + vec3<f32>(1e-6, 0.0, 0.0));
            let north = cross(n, east);
            let lift = b.shade.w / max(mu0_smooth, 0.08);
            let shift = vec2<f32>(dot(s, east) * lift / max(cos(lat), 0.05) * 0.5 * INV_PI,
                                  -dot(s, north) * lift * INV_PI);
            let shadow = textureSampleLevel(aux_map, map_sampler, cloud_uv + shift, 2.0).r;
            light *= 1.0 - 0.6 * shadow * (1.0 - cloud);
        }
        // Minnaert (limb darkening) and Lunar-Lambert (dusty, bright to the limb).
        let k = b.shade.x;
        let minnaert = pow(max(mu0, 0.0), k) * pow(mu, k - 1.0);
        let lunar = 2.0 * max(mu0, 0.0) / (max(mu0, 0.0) + mu);
        let f = mix(minnaert, lunar, b.shade.y);
        surf = albedo * light * INV_PI * f;
        // Sun glint on open water.
        if (b.shade.z > 0.0 && albedo_s.a > 0.0 && mu0 > 0.0) {
            let hv = normalize(s + v);
            let rough = 0.32;
            let a2 = rough * rough;
            let nh = max(dot(n, hv), 0.0);
            let d = a2 / (PI * sq(nh * nh * (a2 - 1.0) + 1.0));
            let fres = 0.02 + 0.98 * pow(1.0 - max(dot(v, hv), 0.0), 5.0);
            let g = 1.0 / (4.0 * max(mu0, 0.05) * mu);
            surf += light * d * fres * g * mu0 * albedo_s.a * b.shade.z * (1.0 - cloud);
        }
        if ((flags & F_CLOUDS) != 0u) {
            let cmu0 = max(mu0_smooth, 0.0);
            let c_light = select(sun_e, sun_e * sun_transmittance(rel_hit), (flags & F_AIR) != 0u);
            let lit = 0.82 * c_light * INV_PI * (0.85 * cmu0 + 0.15 * pow(cmu0, 0.3) * step(0.0, mu0_smooth));
            surf = mix(surf, lit, cloud);
        }
    }

    // ---------------------------------------------------------------- haze
    var haze_l = vec3<f32>(0.0);
    var haze_t = vec3<f32>(1.0);
    var haze_a = 0.0;
    if ((flags & F_AIR) != 0u) {
        // The renderer's own atmosphere (it also draws the rim around the disk in the sky).
        if (cover > 0.0) {
            let a = atmosphere(vec3<f32>(0.0), dir, t_surf * r_eq, 8);
            haze_t = a.transmittance;
            haze_l = a.radiance * frame.sun_color.rgb;
        }
    } else if ((flags & F_HAZE) != 0u && outside && ahead) {
        let hs = b.haze2.w;
        let x = 1.0 / max(hs, 1e-6);
        var air = 0.0;
        var n_ref = n;
        if (h > 0.0) {
            air = chapman(dot(n, v), x);
        } else {
            let alt = q_len - 1.0;
            air = sqrt(2.0 * PI * x) * exp(-alt / hs);
            n_ref = q / max(q_len, 1e-6);
        }
        let tau = b.haze.rgb * b.haze.w * air;
        haze_t = exp(-tau);
        // Lit haze: the sunlit side of the shell (it reaches a little past the terminator),
        // reddened on its way in.
        let mu_s = dot(n_ref, s);
        let sunlit = smoothstep(-0.18, 0.12, mu_s);
        let t_in = exp(-b.haze.rgb * b.haze.w * chapman(mu_s, x) * 0.5);
        let ph = phase_hg(dot(s, dl), b.haze3.x);
        haze_l = sun_e * (1.0 / (4.0 * PI)) * ph * b.haze2.rgb * (1.0 - haze_t) * sunlit * t_in;
        haze_a = 1.0 - dot(haze_t, vec3<f32>(0.3333));
        // Fade the haze in over the anti-aliased edge of the shell's visible part.
        haze_l *= smoothstep(0.0, 1.0, 1.0 - (q_len - 1.0) / (hs * 12.0));
    }

    // ---------------------------------------------------------------- rings
    var ring_l = vec3<f32>(0.0);
    var ring_a = 0.0;
    var ring_front = false;
    let ring_hit = (flags & F_RINGS) != 0u && t_ring > 0.0 && ring_u >= 0.0 && ring_u <= 1.0;
    if (ring_hit) {
        let alpha_n = ring_s.a;
        let tau = -log(max(1.0 - alpha_n, 0.003));
        let mu_v = max(abs(dl.y), 1e-3);
        let mu_s = max(abs(s.y), 1e-3);
        let lit_side = (o.y > 0.0) == (s.y > 0.0);
        let cos_phase = dot(v, s);
        // Icy particles scatter back toward the sun; dusty thin rings forward.
        let back = 0.6 + 0.9 * exp(-acos(clamp(cos_phase, -1.0, 1.0)) * 1.1);
        let dust = (1.0 - alpha_n) * 0.6 * phase_hg(-cos_phase, 0.7) * 0.15;
        let ph = back + dust;
        var i_f = 0.0;
        if (lit_side) {
            i_f = 2.0 * mu_s / (mu_s + mu_v) * (1.0 - exp(-tau * (1.0 / mu_s + 1.0 / mu_v)));
        } else {
            let dm = mu_s - mu_v;
            if (abs(dm) < 1e-3) {
                i_f = 2.0 * tau / mu_v * exp(-tau / mu_v);
            } else {
                i_f = 2.0 * mu_s / dm * (exp(-tau / mu_s) - exp(-tau / mu_v));
            }
        }
        // The planet's shadow on the rings.
        let o2 = vec3<f32>(pr.x, 0.0, pr.z);
        let d2 = normalize(vec3<f32>(s.x, s.y * e, s.z));
        let b2 = dot(o2, d2);
        let q2 = length(o2 - b2 * d2);
        let shadow = select(1.0, smoothstep(0.985, 1.015, q2), b2 < 0.0);
        ring_l = ring_s.rgb * sun_e * INV_PI * i_f * ph * 0.5 * b.rings.z * shadow;
        ring_a = 1.0 - exp(-tau / mu_v);
        ring_front = !(cover > 0.0 && h > 0.0) || t_ring < t_surf;
    }

    // ---------------------------------------------------------------- compose (front to back)
    // Planet with its haze: the haze veils the disk and glows around it.
    var body_rgb = surf * cover * haze_t + haze_l * select(1.0, cover, (flags & F_AIR) != 0u);
    var body_a = max(cover, haze_a);
    var rgb = vec3<f32>(0.0);
    var alpha = 0.0;
    if (ring_hit && ring_front) {
        rgb = ring_l + (1.0 - ring_a) * body_rgb;
        alpha = ring_a + (1.0 - ring_a) * body_a;
    } else {
        rgb = body_rgb + (1.0 - body_a) * ring_l;
        alpha = body_a + (1.0 - body_a) * ring_a;
    }
    if (alpha <= 1e-5 && dot(rgb, rgb) <= 1e-12) {
        discard;
    }

    // Depth: the ring if it is in front and substantial, else the depth sphere under the body
    // (so terrain wins), else the closest approach of the ray.
    let kd = b.axis_z.w / r_eq;
    let hd = kd * kd - q_len * q_len;
    var t_depth = select(-bq, -bq - sqrt(max(hd, 0.0)), hd > 0.0) / ds_len;
    if (ring_hit && ring_front && ring_a > 0.3) {
        t_depth = t_ring;
    }
    var out: FsOut;
    out.depth = depth_of(dir * (max(t_depth, 1e-6) * r_eq));
    out.color = vec4<f32>(pre_expose(rgb), alpha);
    return out;
}
