// Game UI: one instanced quad per primitive (boxes, shadows, glyphs, icons, rings, lines,
// glows, planets, galaxies, star fields), drawn onto the tonemapped LDR image.
//
// Composed after lib_types.wgsl (noise helpers). Colors are linear; output is premultiplied
// alpha (blend One, OneMinusSrcAlpha) into an sRGB target, so blending happens in linear space.

struct Globals {
    // width, height, 1/width, 1/height (physical px)
    viewport: vec4<f32>,
    // real seconds, unused...
    time: vec4<f32>,
};

struct Look {
    colors: array<vec4<f32>, 6>,
    atmosphere: vec4<f32>,
    clouds: vec4<f32>,
    lights: vec4<f32>,
    shape: vec4<f32>,
    style: vec4<f32>,
};

struct Looks {
    items: array<Look, 64>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var mask_tex: texture_2d<f32>;
@group(1) @binding(1) var color_tex: texture_2d<f32>;
@group(1) @binding(2) var backdrop_tex: texture_2d<f32>;
@group(1) @binding(3) var samp: sampler;
@group(1) @binding(4) var<uniform> looks: Looks;

const K_RECT: u32 = 0u;
const K_SHADOW: u32 = 1u;
const K_GLYPH: u32 = 2u;
const K_IMAGE: u32 = 3u;
const K_RING: u32 = 4u;
const K_LINE: u32 = 5u;
const K_GLOW: u32 = 6u;
const K_PLANET: u32 = 7u;
const K_PLANET_FLAT: u32 = 8u;
const K_GALAXY: u32 = 9u;
const K_STARFIELD: u32 = 10u;

const F_LINEAR: u32 = 1u;
const F_RADIAL: u32 = 2u;
const F_BACKDROP: u32 = 4u;
const F_DESATURATE: u32 = 8u;
const F_MASK: u32 = 32u;


struct VIn {
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) clip: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) color2: vec4<f32>,
    @location(4) border_color: vec4<f32>,
    @location(5) radius: vec4<f32>,
    @location(6) uv: vec4<f32>,
    @location(7) params: vec4<f32>,
    @location(8) extra: vec4<f32>,
    @location(9) kind: vec4<u32>,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    // local.xy: px from the quad center (unrotated); local.zw: 0..1 across the quad
    @location(0) local: vec4<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) clip: vec4<f32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) color2: vec4<f32>,
    @location(5) @interpolate(flat) border_color: vec4<f32>,
    @location(6) @interpolate(flat) radius: vec4<f32>,
    @location(7) @interpolate(flat) uv: vec4<f32>,
    @location(8) @interpolate(flat) params: vec4<f32>,
    @location(9) @interpolate(flat) extra: vec4<f32>,
    @location(10) @interpolate(flat) kind: vec4<u32>,
    @location(11) screen: vec2<f32>,
};

@vertex
fn vs_main(in: VIn) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[in.vi % 6u];
    let size = in.rect.zw;
    let center = in.rect.xy + size * 0.5;
    let local = (c - 0.5) * size;
    let a = in.extra.x;
    let rot = vec2(local.x * cos(a) - local.y * sin(a), local.x * sin(a) + local.y * cos(a));
    let screen = center + rot;
    var out: VOut;
    let ndc = vec2(screen.x * g.viewport.z * 2.0 - 1.0, 1.0 - screen.y * g.viewport.w * 2.0);
    out.pos = vec4(ndc, 0.0, 1.0);
    out.local = vec4(local, c);
    out.rect = in.rect;
    out.clip = in.clip;
    out.color = in.color;
    out.color2 = in.color2;
    out.border_color = in.border_color;
    out.radius = in.radius;
    out.uv = in.uv;
    out.params = in.params;
    out.extra = in.extra;
    out.kind = in.kind;
    out.screen = screen;
    return out;
}

// Signed distance to a rounded box (radii tl, tr, br, bl), p from the center, y down.
fn sd_round_box(p: vec2<f32>, half: vec2<f32>, r: vec4<f32>) -> f32 {
    var rad = select(r.xw, r.yz, p.x > 0.0);   // left: tl, bl ; right: tr, br
    let rr = select(rad.x, rad.y, p.y > 0.0);
    let q = abs(p) - half + vec2(rr);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0))) - rr;
}

fn premul(c: vec4<f32>) -> vec4<f32> {
    return vec4(c.rgb * c.a, c.a);
}

// Source-over of premultiplied colors.
fn over(top: vec4<f32>, bottom: vec4<f32>) -> vec4<f32> {
    return top + bottom * (1.0 - top.a);
}

fn clip_alpha(screen: vec2<f32>, clip: vec4<f32>) -> f32 {
    let inside = step(clip.xy, screen) * step(screen, clip.zw);
    return inside.x * inside.y;
}

// ---------------------------------------------------------------------------------------
// Planets

fn look(i: u32) -> Look {
    return looks.items[min(i, 63u)];
}

// Terrain height 0..1 for a unit direction.
fn planet_height(d: vec3<f32>, L: Look) -> f32 {
    let seed = vec3(L.shape.x * 17.13, L.shape.x * 3.71, L.shape.x * 9.27);
    let rough = L.shape.z;
    var h = fbm3(d * 1.6 + seed, 6);
    h += 0.35 * rough * (fbm3(d * 5.0 + seed * 1.7, 4) - 0.5);
    return saturate(h);
}

// Albedo of the surface (and whether it is water) at unit direction `d`.
fn planet_surface(d: vec3<f32>, L: Look) -> vec4<f32> {
    let lat = asin(clamp(d.y, -1.0, 1.0));
    let abs_lat = abs(lat) / 1.5707963;
    if (L.style.y > 0.0) {
        // Gas giant: turbulent latitude bands.
        let turb = fbm3(d * vec3(2.0, 6.0, 2.0) + vec3(L.shape.x), 5) - 0.5;
        let b = sin((d.y + turb * L.style.z * 0.35) * L.style.y * 3.14159);
        let t = b * 0.5 + 0.5;
        var col = mix(L.colors[2].rgb, L.colors[3].rgb, t);
        col = mix(col, L.colors[4].rgb, smoothstep(0.6, 1.0, fbm3(d * 8.0 + vec3(3.0), 3)) * 0.4);
        // A great storm.
        let storm_c = normalize(vec3(0.6, -0.35, 0.72));
        let s = smoothstep(0.16, 0.0, length(d - storm_c));
        col = mix(col, L.colors[5].rgb, s * 0.8);
        return vec4(col, 0.0);
    }
    let h = planet_height(d, L);
    let sea = L.shape.y;
    var col: vec3<f32>;
    var water = 0.0;
    if (h < sea) {
        let depth = saturate((sea - h) / 0.12);
        col = mix(L.colors[1].rgb, L.colors[0].rgb, smoothstep(0.0, 1.0, depth));
        water = 1.0;
    } else {
        let e = saturate((h - sea) / max(1.0 - sea, 0.01));
        let dry = smoothstep(0.35, 0.75, fbm3(d * 2.3 + vec3(L.shape.x * 5.0), 4)) * L.style.x
            * (1.0 - smoothstep(0.35, 0.75, abs_lat));
        let low = mix(L.colors[2].rgb, L.colors[3].rgb, dry);
        col = mix(low, L.colors[3].rgb * 0.85, smoothstep(0.15, 0.45, e));
        col = mix(col, L.colors[4].rgb, smoothstep(0.42, 0.7, e));
        col = mix(col, L.colors[5].rgb, smoothstep(0.72, 0.85, e));
        // Beaches.
        col = mix(L.colors[3].rgb * 1.15, col, smoothstep(0.0, 0.025, h - sea));
    }
    // Ice caps.
    let ice_n = fbm3(d * 4.0 + vec3(11.0), 3) * 0.12;
    let ice = smoothstep(L.shape.w - 0.02, L.shape.w + 0.03, abs_lat + ice_n);
    col = mix(col, L.colors[5].rgb, ice);
    water = water * (1.0 - ice);
    return vec4(col, water);
}

fn rot_y(v: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}

fn rot_x(v: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3(v.x, c * v.y - s * v.z, s * v.y + c * v.z);
}

fn cloud_cover(d: vec3<f32>, L: Look, t: f32) -> f32 {
    let drift = rot_y(d, t * 0.01);
    let n = fbm3(drift * 3.2 + vec3(L.shape.x * 2.0, 0.0, 0.0), 5);
    let swirl = fbm3(drift * 9.0 + vec3(n * 2.0), 3);
    let c = n * 0.75 + swirl * 0.25;
    return smoothstep(1.0 - L.clouds.a, 1.0 - L.clouds.a + 0.22, c);
}

fn shade_planet(in: VOut) -> vec4<f32> {
    let L = look(in.kind.z);
    let radius = min(in.rect.z, in.rect.w) * 0.5 / (1.0 + in.extra.y);
    let q = in.local.xy / radius;
    let d2 = dot(q, q);
    let yaw = in.uv.x;
    let pitch = in.uv.y;
    let sun = normalize(vec3(sin(in.uv.z) * cos(in.uv.w), sin(in.uv.w), cos(in.uv.z) * cos(in.uv.w)));
    let atmo = L.atmosphere;
    let px = 1.0 / radius;
    if (d2 > 1.0) {
        // Atmosphere halo outside the limb.
        let r = sqrt(d2);
        let k = (r - 1.0) / max(in.extra.y, 0.01);
        let edge_n = normalize(vec3(q / r, 0.0));
        let lit = smoothstep(-0.35, 0.4, dot(edge_n, sun));
        let a = exp(-k * 4.0) * atmo.a * (0.25 + 0.75 * lit) * step(r, 1.0 + in.extra.y);
        return vec4(atmo.rgb * a, a) * in.params.w;
    }
    let z = sqrt(1.0 - d2);
    let n_view = vec3(q.x, -q.y, z);
    let n = rot_y(rot_x(n_view, -pitch), yaw);
    let surf = planet_surface(n, L);
    var albedo = surf.rgb;
    let water = surf.a;
    let ndl = dot(n_view, sun);
    // Soft terminator.
    let day = smoothstep(-0.12, 0.3, ndl);
    let diffuse = max(ndl, 0.0) * 0.9 + 0.1 * day;
    // Clouds with their own shadow.
    let cov = cloud_cover(n, L, g.time.x) * step(L.style.y, 0.0);
    var col = albedo * diffuse;
    // Ocean glint.
    let h = normalize(sun + vec3(0.0, 0.0, 1.0));
    let spec = pow(max(dot(n_view, h), 0.0), 60.0) * water * day * 0.9;
    col += vec3(1.0, 0.95, 0.85) * spec;
    // City lights on the night side.
    let city = smoothstep(0.62, 0.8, fbm3(n * 22.0 + vec3(L.shape.x), 3))
        * (1.0 - water) * L.lights.a * smoothstep(0.1, -0.15, ndl)
        * smoothstep(0.25, 0.55, fbm3(n * 3.0 + vec3(7.0), 3));
    col += L.lights.rgb * city * 1.6;
    col = mix(col, L.clouds.rgb * (diffuse * 1.05 + 0.02), cov);
    // Atmosphere: rim scattering on the lit side, slight blue tint over the disc.
    let rim = pow(1.0 - z, 2.5);
    col = mix(col, atmo.rgb * (0.2 + day), saturate(rim * atmo.a * 0.9));
    col += atmo.rgb * pow(1.0 - z, 6.0) * day * atmo.a * 0.6;
    // Anti-aliased limb.
    let edge = saturate((1.0 - sqrt(d2)) / px);
    return vec4(col, 1.0) * edge * in.params.w;
}

fn shade_planet_flat(in: VOut) -> vec4<f32> {
    let L = look(in.kind.z);
    let t = in.local.zw;
    let lon = mix(in.uv.x, in.uv.z, t.x);
    let lat = mix(in.uv.y, in.uv.w, t.y);
    let d = vec3(cos(lat) * sin(lon), sin(lat), cos(lat) * cos(lon));
    let surf = planet_surface(d, L);
    var col = surf.rgb;
    // Relief shading from the height gradient (light from the north-west).
    if (L.style.y <= 0.0) {
        let e = 0.004;
        let hx = planet_height(vec3(cos(lat) * sin(lon + e), sin(lat), cos(lat) * cos(lon + e)), L);
        let hy = planet_height(vec3(cos(lat + e) * sin(lon), sin(lat + e), cos(lat + e) * cos(lon)), L);
        let h0 = planet_height(d, L);
        let slope = (h0 - hx) * 18.0 + (hy - h0) * 18.0;
        col *= 1.0 + clamp(slope, -0.4, 0.4) * (1.0 - surf.a);
        let cov = cloud_cover(d, L, g.time.x) * in.extra.z;
        col = mix(col, L.clouds.rgb * 0.95, cov * 0.85);
    }
    // Graticule every 30 degrees.
    let deg = vec2(lon, lat) * 57.29578;
    let gw = abs(vec2((in.uv.z - in.uv.x) / in.rect.z, (in.uv.w - in.uv.y) / in.rect.w)) * 57.29578 * 1.2;
    let gl = abs(fract(deg / 30.0 + 0.5) - 0.5) * 30.0;
    let grid = (1.0 - smoothstep(0.0, gw.x, gl.x)) + (1.0 - smoothstep(0.0, gw.y, gl.y));
    col = mix(col, vec3(0.85, 0.92, 1.0), saturate(grid) * 0.22 * in.extra.w);
    return vec4(col, 1.0) * in.params.w;
}

// ---------------------------------------------------------------------------------------
// Space backgrounds

fn shade_galaxy(in: VOut) -> vec4<f32> {
    let center = in.uv.xy;
    let radius = in.uv.z;
    let p0 = (in.screen - center) / radius;
    // Tilt: squash y.
    let tilt = in.extra.y;
    var p = vec2(p0.x, p0.y / max(tilt, 0.05));
    let a = in.uv.w;
    p = vec2(p.x * cos(a) - p.y * sin(a), p.x * sin(a) + p.y * cos(a));
    let r = length(p);
    if (r > 1.6) {
        return vec4(0.0);
    }
    let theta = atan2(p.y, p.x);
    let arms = 2.0;
    let wind = 4.2;
    let spiral = theta * arms - log(max(r, 0.02)) * wind;
    let n = fbm2(p * 6.0 + vec2(3.0, 1.0), 5);
    let arm = pow(0.5 + 0.5 * cos(spiral + n * 2.2), 3.0);
    let disk = exp(-r * 3.2);
    let dust = smoothstep(0.55, 0.85, fbm2(p * 12.0, 4)) * arm * smoothstep(0.08, 0.3, r);
    let bulge = exp(-r * r * 38.0);
    let clumps = smoothstep(0.62, 0.92, fbm2(p * 28.0 + vec2(n), 3)) * arm * disk;
    var col = vec3(0.45, 0.6, 1.0) * arm * disk * 0.9;
    col += vec3(0.75, 0.82, 1.0) * disk * 0.25;
    col += vec3(1.0, 0.45, 0.65) * clumps * 0.9;
    col += vec3(1.0, 0.86, 0.62) * bulge * 2.2;
    col *= 1.0 - dust * 0.65;
    let intensity = saturate(max(max(col.r, col.g), col.b));
    let fade = 1.0 - smoothstep(1.1, 1.6, r);
    // Additive-ish: low alpha so the background glows through.
    return vec4(col * fade * in.params.w, intensity * fade * 0.3 * in.params.w);
}

fn hash22(p: vec2<f32>) -> vec2<f32> {
    let q = vec2(dot(p, vec2(127.1, 311.7)), dot(p, vec2(269.5, 183.3)));
    return fract(sin(q) * 43758.5453);
}

fn shade_starfield(in: VOut) -> vec4<f32> {
    let pan = in.uv.xy;
    let density = in.uv.z;
    let seed = in.uv.w;
    var col = vec3(0.0);
    // Faint nebula.
    let np = (in.screen + pan * 0.2) / 900.0;
    let neb = fbm2(np * 2.0 + vec2(seed), 5);
    let neb2 = fbm2(np * 3.5 + vec2(seed * 2.0, 4.0), 4);
    col += vec3(0.18, 0.08, 0.32) * smoothstep(0.45, 0.85, neb) * 0.5;
    col += vec3(0.04, 0.16, 0.3) * smoothstep(0.5, 0.9, neb2) * 0.5;
    // Three layers of stars with parallax.
    for (var layer = 0; layer < 3; layer++) {
        let lf = f32(layer);
        let cell = 26.0 + lf * 22.0;
        let p = (in.screen + pan * (0.15 + lf * 0.25)) / cell + vec2(seed * 13.0 + lf * 7.0);
        let id = floor(p);
        let f = fract(p);
        let h = hash22(id);
        if (h.x < density * (0.55 - lf * 0.12)) {
            let star = hash22(id + vec2(5.3, 1.7));
            let d = length(f - star) * cell;
            let size = 0.6 + h.y * (1.3 - lf * 0.3);
            let tw = 0.75 + 0.25 * sin(g.time.x * (1.0 + h.y * 3.0) + h.x * 40.0);
            let b = exp(-d * d / (size * size)) * tw * (0.5 + h.y);
            let temp = hash22(id + vec2(9.1, 3.3)).x;
            let tint = mix(vec3(0.7, 0.8, 1.0), vec3(1.0, 0.85, 0.65), temp);
            col += tint * b;
        }
    }
    return vec4(col * in.params.w, 0.0);
}

// ---------------------------------------------------------------------------------------

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let k = in.kind.x;
    let flags = in.kind.y;
    let ca = clip_alpha(in.screen, in.clip);
    if (ca <= 0.0) {
        discard;
    }
    let half = in.rect.zw * 0.5;
    let p = in.local.xy;
    var out = vec4(0.0);

    if (k == K_RECT) {
        let d = sd_round_box(p, half, in.radius);
        let cov = saturate(0.5 - d);
        if (cov <= 0.0) {
            discard;
        }
        var fill = in.color;
        if ((flags & F_LINEAR) != 0u) {
            let dir = vec2(cos(in.params.z), sin(in.params.z));
            let ext = abs(dir.x) * half.x + abs(dir.y) * half.y;
            let t = saturate(dot(p, dir) / max(ext, 1e-3) * 0.5 + 0.5);
            fill = mix(in.color, in.color2, t);
        } else if ((flags & F_RADIAL) != 0u) {
            // inner at the center, outer at the ellipse touching the edges.
            let t = saturate(length(p / max(half, vec2(1e-3))));
            fill = mix(in.color, in.color2, t);
        }
        var col = premul(fill);
        if ((flags & F_BACKDROP) != 0u) {
            let uv = in.screen * g.viewport.zw;
            let bd = textureSampleLevel(backdrop_tex, samp, uv, 0.0).rgb;
            // Frosted: slightly darkened and desaturated scene behind the panel.
            let l = luminance(bd);
            let frosted = mix(vec3(l), bd, 0.8) * 0.85;
            col = over(col, vec4(frosted, 1.0));
        }
        let bw = in.params.x;
        if (bw > 0.0) {
            let border = saturate(0.5 + d + bw);
            col = mix(col, over(premul(in.border_color), col), border);
        }
        out = col * cov * in.params.w;
    } else if (k == K_SHADOW) {
        let box_center = in.uv.xy + in.uv.zw * 0.5;
        let q = in.screen - box_center;
        let d = sd_round_box(q, in.uv.zw * 0.5, in.radius);
        let sigma = max(in.params.y * 0.5, 0.5);
        let a = 0.5 - 0.5 * tanh(1.2 * d / (sigma * 1.41421));
        out = premul(in.color) * a;
    } else if (k == K_GLYPH) {
        let uv = mix(in.uv.xy, in.uv.zw, in.local.zw);
        var a = textureSampleLevel(mask_tex, samp, uv, 0.0).r;
        // Light text on dark backgrounds looks thin when blended in linear space: thicken it.
        let gamma = mix(1.0, 0.72, saturate(luminance(in.color.rgb) * 1.4));
        a = pow(a, gamma);
        out = premul(in.color) * a;
    } else if (k == K_IMAGE) {
        let uv = mix(in.uv.xy, in.uv.zw, in.local.zw);
        var c = textureSampleLevel(color_tex, samp, uv, 0.0);
        if ((flags & F_DESATURATE) != 0u) {
            c = vec4(vec3(luminance(c.rgb)) * 0.75, c.a);
        }
        out = premul(c * in.color);
        if ((flags & F_MASK) != 0u) {
            let mc = (in.border_color.xy + in.border_color.zw) * 0.5;
            let mh = (in.border_color.zw - in.border_color.xy) * 0.5;
            out = out * saturate(0.5 - sd_round_box(in.screen - mc, mh, in.radius));
        }
    } else if (k == K_RING) {
        let radii = max(in.uv.xy, vec2(0.5));
        let kk = length(p / radii);
        let grad = length(p / (radii * radii));
        let d = (kk - 1.0) * kk / max(grad, 1e-4);
        let band = abs(d) - in.params.x * 0.5;
        var a = saturate(0.5 - band);
        // Arc: 0 = up, clockwise.
        let sweep = in.params.z;
        if (sweep < TAU - 1e-3) {
            var ang = atan2(p.x, -p.y);
            if (ang < 0.0) {
                ang += TAU;
            }
            var rel = ang - in.params.y;
            rel = rel - floor(rel / TAU) * TAU;
            a *= step(rel, sweep);
        }
        let dashes = in.params.w;
        if (dashes > 0.0) {
            var ang = atan2(p.x, -p.y) / TAU + 0.5;
            a *= step(fract(ang * dashes), 0.55);
        }
        out = premul(in.color) * a;
    } else if (k == K_LINE) {
        let hl = in.params.z * 0.5;
        let w = in.params.x * 0.5;
        let dx = max(abs(p.x) - hl, 0.0);
        let d = length(vec2(dx, p.y)) - w;
        var a = saturate(0.5 - d);
        if (in.params.y > 0.0) {
            a *= step(fract((p.x + hl) / in.params.y), 0.55);
        }
        out = premul(in.color) * a;
    } else if (k == K_GLOW) {
        let r = length(p) / max(half.x, 1e-3);
        let a = pow(saturate(1.0 - r), max(in.params.x, 0.1));
        // Additive (alpha 0) unless params.y asks for normal blending.
        out = vec4(in.color.rgb * in.color.a * a, in.color.a * a * in.params.y);
    } else if (k == K_PLANET) {
        out = shade_planet(in);
    } else if (k == K_PLANET_FLAT) {
        out = shade_planet_flat(in) * saturate(0.5 - sd_round_box(p, half, in.radius));
    } else if (k == K_GALAXY) {
        out = shade_galaxy(in);
    } else if (k == K_STARFIELD) {
        out = shade_starfield(in) * saturate(0.5 - sd_round_box(p, half, in.radius));
    }
    return out * ca;
}
