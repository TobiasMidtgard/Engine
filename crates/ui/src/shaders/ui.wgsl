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
// Planet noise: simplex noise over an integer hash. `planet_noise.rs` mirrors pn_pcg3,
// pn_seed, pn_simplex and the base octaves of pn_terrain step for step (same u32 hash, same
// f32 operations), so game code knows where the coastlines are: keep the two in sync.

const PN_F0: f32 = 1.1;
const PN_LAC: f32 = 2.07;
const PN_WARP_F: f32 = 0.5;
const PN_WARP_A: f32 = 0.12;
const PN_WARP_DECAY: f32 = 0.5;
const PN_NORM: f32 = 68.0;
const PN_H_SCALE: f32 = 0.42;
const PN_MOUNTAINS: f32 = 0.16;
const PN_RIDGE_MEAN: f32 = 0.55;
const PN_MAX_OCT: u32 = 12u;
const PN_SALT_WARP: u32 = 0x68BC21EBu;
const PN_SALT_OCT: u32 = 0x9E3779B9u;

// Surface models (`colors[0].w`, `draw::surface`).
const S_ICE: u32 = 2u;
const S_VOLCANIC: u32 = 3u;
const S_BARREN: u32 = 4u;

// PCG3D (Jarzynski & Olano 2020).
fn pn_pcg3(v0: vec3<u32>) -> vec3<u32> {
    var v = v0 * 1664525u + vec3(1013904223u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v = v ^ (v >> vec3(16u));
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}

fn pn_seed(s: f32) -> u32 {
    return pn_pcg3(vec3(bitcast<u32>(s), 0x2545F491u, 0x9E3779B9u)).x;
}

// Three uniform values in [0, 1) for an integral lattice point.
fn pn_rand3(c: vec3<f32>, seed: u32) -> vec3<f32> {
    let h = pn_pcg3(bitcast<vec3<u32>>(vec3<i32>(c)) ^ vec3(seed));
    return vec3<f32>(h >> vec3(8u)) * (1.0 / 16777216.0);
}

fn pn_grad(c: vec3<f32>, seed: u32) -> vec3<f32> {
    let h = pn_pcg3(bitcast<vec3<u32>>(vec3<i32>(c)) ^ vec3(seed));
    return vec3<f32>(h >> vec3(8u)) * (1.0 / 8388608.0) - 1.0;
}

// 3D simplex noise, about -1..1, with its gradient: (value, d/dx, d/dy, d/dz).
fn pn_simplex(p: vec3<f32>, seed: u32) -> vec4<f32> {
    let s = floor(p + (p.x + p.y + p.z) * (1.0 / 3.0));
    let x0 = p - s + (s.x + s.y + s.z) * (1.0 / 6.0);
    let g = step(x0.yzx, x0);
    let l = 1.0 - g;
    let i1 = min(g, l.zxy);
    let i2 = max(g, l.zxy);
    let x1 = x0 - i1 + (1.0 / 6.0);
    let x2 = x0 - i2 + (1.0 / 3.0);
    let x3 = x0 - 0.5;
    let g0 = pn_grad(s, seed);
    let g1 = pn_grad(s + i1, seed);
    let g2 = pn_grad(s + i2, seed);
    let g3 = pn_grad(s + 1.0, seed);
    let w = max(vec4(0.5) - vec4(dot(x0, x0), dot(x1, x1), dot(x2, x2), dot(x3, x3)), vec4(0.0));
    let w2 = w * w;
    let w4 = w2 * w2;
    let gx = vec4(dot(g0, x0), dot(g1, x1), dot(g2, x2), dot(g3, x3));
    let n = dot(w4, gx);
    let t = w2 * w * gx;
    let dn = -8.0 * (t.x * x0 + t.y * x1 + t.z * x2 + t.w * x3)
        + w4.x * g0 + w4.y * g1 + w4.z * g2 + w4.w * g3;
    return vec4(n, dn) * PN_NORM;
}

// Octaves worth drawing for a pixel footprint `fp` (planet radii per pixel): octave k's
// features are about 0.6 / (F0 * LAC^k) radii across; keep those over ~1.4 px. Fractional:
// the last octave fades in, so zooming never pops.
fn pn_lod(fp: f32) -> f32 {
    let fmax = 0.42 / max(fp, 1e-7);
    return clamp(log2(fmax / PN_F0) / log2(PN_LAC) + 1.0, 1.0, f32(PN_MAX_OCT));
}

struct Terrain {
    // Height 0..1; the sea is below `shape.y`.
    h: f32,
    // dh / d(direction), 3D (project it onto the surface for slopes).
    grad: vec3<f32>,
    // A smooth continental-scale field, about -1..1 (moisture, temperature noise).
    warp: f32,
    // Octave 3 and the finest drawn octave, about -1..1 (texture).
    mid: f32,
    fine: f32,
    // 1 - |octave| for octaves 2, 3 and 5: 1 along long thin ridge lines (cracks, fissures).
    ridge2: f32,
    ridge3: f32,
    ridge5: f32,
};

// Domain-warped fractal terrain with ridged mountain chains inland. The first
// `planet_noise::BASE_OCTAVES` octaves are the CPU's `height`; the rest (up to `lod`) only add
// detail when zoomed in.
fn pn_terrain(d: vec3<f32>, L: Look, lod: f32) -> Terrain {
    var t: Terrain;
    let seed = pn_seed(L.shape.x);
    let p = d * PN_F0;
    let w = pn_simplex(p * PN_WARP_F, seed ^ PN_SALT_WARP);
    // The warp bends the continents; finer octaves follow it less and less (a full-strength
    // warp would stretch them into fibres).
    let disp = w.yzw * PN_WARP_A;
    var g = 1.0;
    var fr = 1.0;
    var amp = 1.0;
    var freq = PN_F0;
    var sum = 0.0;
    var grad = vec3(0.0);
    var ridge = 0.0;
    var rgrad = vec3(0.0);
    var low = 0.0;
    let n_oct = u32(ceil(lod));
    for (var k = 0u; k < n_oct; k++) {
        let wk = saturate(lod - f32(k));
        let n = pn_simplex((p + disp * g) * fr, seed + k * PN_SALT_OCT);
        sum += amp * (n.x * wk);
        // Slopes lag one octave behind: the finest octave is only a pixel or two across and
        // would shade as noise.
        let gk = saturate(lod - f32(k) - 1.0);
        grad += (amp * freq * gk) * n.yzw;
        if (k >= 1u && k <= 4u) {
            let r = 1.0 - abs(n.x);
            ridge += amp * (wk * (r * r) + (1.0 - wk) * 0.64);
            rgrad += (amp * freq * gk * -2.0 * r * sign(n.x)) * n.yzw;
        }
        if (k == 2u) {
            low = sum;
            t.ridge2 = (1.0 - abs(n.x)) * wk;
        }
        if (k == 3u) {
            t.mid = n.x * wk;
            t.ridge3 = (1.0 - abs(n.x)) * wk;
        }
        if (k == 5u) {
            t.ridge5 = (1.0 - abs(n.x)) * wk;
        }
        t.fine = mix(t.fine, n.x, wk);
        fr *= PN_LAC;
        if (k >= 2u) {
            g *= PN_WARP_DECAY;
        }
        amp *= 0.5;
        freq *= PN_LAC;
    }
    if (n_oct <= 2u) {
        low = sum;
    }
    // Ridge octaves too fine to draw count at their mean (small planets keep their heights).
    for (var k = max(n_oct, 1u); k <= 4u; k++) {
        ridge += exp2(-f32(k)) * 0.64;
    }
    let inland = smoothstep(0.02, 0.2, 0.5 + PN_H_SCALE * low - L.shape.y) * L.shape.z;
    t.h = saturate(0.5 + PN_H_SCALE * sum + PN_MOUNTAINS * inland * (ridge - PN_RIDGE_MEAN));
    t.grad = PN_H_SCALE * grad + (PN_MOUNTAINS * inland) * rgrad;
    t.warp = w.x;
    return t;
}

// ---------------------------------------------------------------------------------------
// Planet surfaces

struct PSurf {
    albedo: vec3<f32>,
    // Light the surface gives off (lava, cities), added after lighting.
    emit: vec3<f32>,
    // Height gradient along the surface, for relief shading.
    grad: vec3<f32>,
    // Open water 0..1 (sun glint, sky reflection).
    water: f32,
    // Night-side city lights 0..1 (living worlds).
    city: f32,
};

fn look(i: u32) -> Look {
    return looks.items[min(i, 63u)];
}

fn tangent(v: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    return v - n * dot(v, n);
}

// Land coverage of a pixel: a crisp coastline, antialiased over the pixel's height range.
fn land_cover(h: f32, sea: f32, slope: f32, fp: f32) -> f32 {
    return saturate((h - sea) / max(slope * fp, 1e-6) + 0.5);
}

// Metro areas from the terrain's own octaves, sprinkled with towns from a hash grid (their
// average once a grid cell is smaller than a pixel).
fn city_lights(d: vec3<f32>, hab: f32, t: Terrain, fp: f32, seed: u32) -> f32 {
    let metro = smoothstep(0.78, 1.12, hab * 0.6 + t.mid * 0.5 + t.fine * 0.2) * (0.6 + 0.4 * hab);
    // Lit highways between them (the network of one ridge octave).
    let roads = smoothstep(0.94, 0.99, t.ridge5) * hab * 0.12;
    let cell = 210.0;
    let c = d * cell;
    let rnd = pn_rand3(floor(c), seed ^ 0xC17E5u);
    let f = fract(c) - (0.25 + 0.5 * rnd);
    let town = step(1.0 - 0.3 * hab, rnd.x) * exp(-dot(f, f) * 18.0) * (0.5 + rnd.y);
    let avg = 0.3 * hab * 0.15;
    return metro + roads + mix(town, avg, saturate(fp * cell * 1.2 - 0.4));
}

// Living worlds: oceans with shelves and turquoise shallows; biomes from temperature
// (latitude, height) and moisture (circulation cells, noise, dry interiors); rock and snow
// up high; sea ice and polar caps with ragged edges.
fn surf_living(d: vec3<f32>, L: Look, t: Terrain, fp: f32) -> PSurf {
    var s: PSurf;
    let sea = L.shape.y;
    let gt = tangent(t.grad, d);
    let land = land_cover(t.h, sea, length(gt), fp);
    let e = saturate((t.h - sea) / max(0.85 - sea, 0.12));
    let alat = abs(asin(clamp(d.y, -1.0, 1.0))) / 1.5707963;
    // Temperature: 1 at the equator, 0 at the ice line (`shape.w` of the way to the pole).
    let temp = 1.0 - alat / max(L.shape.w, 0.05) + t.warp * 0.07 + t.mid * 0.04 + t.fine * 0.03;
    let temp_land = temp - e * 0.5;
    // Moisture: wet tropics, dry subtropics (~25 deg), wet mid-latitudes, dry poles.
    var moist = 0.6 + 0.36 * cos(alat * 11.31) * (1.0 - 0.4 * alat);
    moist += t.warp * 0.4 + t.mid * 0.12 + t.fine * 0.04;
    moist -= smoothstep(0.03, 0.22, t.h - sea) * 0.2;
    moist = saturate(moist - (L.style.x - 0.3) * 0.9);

    let forest = L.colors[2].rgb;
    let dry = L.colors[3].rgb;
    let rock = L.colors[4].rgb;
    let snow = L.colors[5].rgb;
    let jungle = forest * vec3(0.72, 0.86, 0.66);
    let grass = mix(forest, dry, 0.3) * vec3(1.05, 1.08, 0.9);
    let savanna = mix(dry, forest, 0.28) * vec3(1.04, 0.98, 0.84);
    let desert = dry * vec3(1.18, 1.08, 0.98);
    let steppe = mix(dry, rock, 0.4);
    let taiga = forest * vec3(0.55, 0.68, 0.62);
    let tundra = mix(rock, forest, 0.25) * vec3(0.95, 0.92, 0.88);
    let hot = mix(mix(desert, savanna, smoothstep(0.18, 0.42, moist)), jungle, smoothstep(0.5, 0.74, moist));
    let mild = mix(mix(steppe, grass, smoothstep(0.14, 0.38, moist)), forest, smoothstep(0.42, 0.66, moist));
    let cold = mix(tundra, taiga, smoothstep(0.25, 0.5, moist));
    var ground = mix(cold, mild, smoothstep(0.2, 0.36, temp_land));
    ground = mix(ground, hot, smoothstep(0.56, 0.72, temp_land));
    ground = mix(tundra, ground, smoothstep(0.03, 0.13, temp_land));
    ground *= 0.9 + 0.12 * t.mid + 0.1 * t.fine;
    ground = mix(ground, rock * (0.9 + 0.2 * t.fine), smoothstep(0.42, 0.7, e + 0.08 * t.mid) * 0.85);
    let beach = (1.0 - smoothstep(0.0, 0.006, t.h - sea)) * smoothstep(0.3, 0.6, temp) * 0.6;
    ground = mix(ground, dry * 1.3, beach);
    // Snow: polar caps everywhere, peaks only where there is moisture to fall.
    let snow_t = temp_land + (L.style.x - 0.3) * 0.35 * smoothstep(0.3, 0.7, temp)
        + t.mid * 0.07 - t.fine * 0.03 - (t.ridge3 - 0.6) * 0.12;
    ground = mix(ground, snow * (0.94 + 0.06 * t.fine), smoothstep(0.04, -0.04, snow_t));

    let depth = max(sea - t.h, 0.0);
    var ocean = mix(L.colors[1].rgb, L.colors[0].rgb, smoothstep(0.0, 0.03, depth));
    ocean = mix(ocean, L.colors[0].rgb * 0.6, smoothstep(0.05, 0.25, depth));
    let shallows = (1.0 - smoothstep(0.0, 0.012, depth)) * smoothstep(0.45, 0.8, temp) * 0.7;
    ocean = mix(ocean, L.colors[1].rgb * vec3(0.9, 1.25, 1.15), shallows);
    ocean *= mix(0.8, 1.0, saturate(temp * 1.5));
    let ice = smoothstep(0.0, -0.06, temp + t.fine * 0.03);
    let pack = mix(snow * 0.94, snow * vec3(0.78, 0.86, 0.95), smoothstep(0.85, 0.97, t.ridge5));
    ocean = mix(ocean, pack, ice);

    s.albedo = mix(ocean, ground, land);
    s.water = (1.0 - land) * (1.0 - ice);
    s.grad = gt * (land + (1.0 - land) * ice * 0.15);
    let hab = land * smoothstep(0.22, 0.4, temp_land) * (1.0 - smoothstep(0.8, 0.98, temp_land))
        * smoothstep(0.12, 0.3, moist) * (1.0 - smoothstep(0.08, 0.3, e))
        * (0.45 + 0.55 * (1.0 - smoothstep(0.0, 0.06, t.h - sea)));
    s.city = city_lights(d, hab, t, fp, pn_seed(L.shape.x));
    return s;
}

// A thin line along the crest of a ridge octave (`r` = 1 - |n|, gradient about `slope` per
// planet radius): `w` wide in noise units, antialiased; fades to its average once thinner than
// a pixel.
fn ridge_line(r: f32, w: f32, slope: f32, fp: f32) -> f32 {
    let px = slope * fp;
    let ww = max(w, px);
    return smoothstep(1.0 - ww - px * 0.5, 1.0 - ww * 0.35, r) * min(1.0, w / ww);
}

// Ice worlds: sheets of pale ice crossed by long ruddy lineae (wide double ridges and a finer
// web), mottled with brownish chaos terrain; snowfields and blue glacier ice on the uplands,
// rock only on the steepest highs. No open water: the "coast" is a soft ice-sheet edge.
fn surf_ice(d: vec3<f32>, L: Look, t: Terrain, fp: f32) -> PSurf {
    var s: PSurf;
    let sea = L.shape.y;
    let gt = tangent(t.grad, d);
    let land = smoothstep(sea - 0.01, sea + 0.04, t.h);
    let e = saturate((t.h - sea) / max(0.85 - sea, 0.12));
    let ruddy = mix(L.colors[4].rgb, vec3(0.55, 0.32, 0.2), 0.75);
    var floe = mix(L.colors[3].rgb, L.colors[2].rgb, smoothstep(-0.6, 0.4, t.warp + t.mid * 0.3));
    floe = mix(floe, L.colors[1].rgb, smoothstep(0.2, 0.8, -t.warp - t.mid * 0.2) * 0.4);
    let chaos = smoothstep(0.25, 0.6, t.mid + t.warp * 0.4 + t.fine * 0.2);
    floe = mix(floe, mix(floe, ruddy, 0.5), chaos * 0.7);
    floe *= 0.96 + 0.06 * t.fine;
    let wide = ridge_line(t.ridge2, 0.1, 12.0, fp) * (0.5 + 0.5 * smoothstep(-0.3, 0.3, t.mid));
    let mid = ridge_line(t.ridge3, 0.06, 24.0, fp);
    let web = ridge_line(t.ridge5, 0.03, 100.0, fp) * (0.4 + 0.6 * smoothstep(-0.2, 0.4, t.warp));
    // Double ridges: a bright crest inside each wide ruddy band.
    let crest = ridge_line(t.ridge2, 0.02, 12.0, fp);
    floe = mix(floe, ruddy, saturate(wide * 0.85 + mid * 0.6 + web * 0.4));
    floe = mix(floe, L.colors[2].rgb * 1.05, crest * wide * 0.7);
    var ground = mix(L.colors[2].rgb, L.colors[5].rgb, smoothstep(-0.2, 0.4, t.mid * 0.5 + e * 1.5));
    ground = mix(ground, L.colors[4].rgb * (0.85 + 0.3 * t.fine), smoothstep(0.62, 0.95, e + 0.12 * t.fine) * 0.6);
    ground = mix(ground, ruddy, web * 0.25);
    ground *= 0.97 + 0.05 * t.fine;
    s.albedo = mix(floe, ground, land);
    s.grad = gt * mix(0.3, 1.0, land);
    return s;
}

// Volcanic worlds: dark basalt and ash, glowing fissures in the lowlands, lava seas under
// drifting crust plates.
fn surf_volcanic(d: vec3<f32>, L: Look, t: Terrain, fp: f32) -> PSurf {
    var s: PSurf;
    let sea = L.shape.y;
    let gt = tangent(t.grad, d);
    let land = land_cover(t.h, sea, length(gt), fp);
    let e = saturate((t.h - sea) / max(0.85 - sea, 0.12));
    var crust = mix(L.colors[2].rgb, L.colors[3].rgb, smoothstep(-0.5, 0.5, t.mid + t.warp * 0.5));
    crust = mix(crust, L.colors[5].rgb, smoothstep(0.35, 0.7, e + 0.1 * t.fine) * 0.6);
    crust *= 0.85 + 0.25 * t.fine;
    let low = 1.0 - smoothstep(0.03, 0.35, e);
    let fis = smoothstep(0.965, 0.995, t.ridge3) * low + smoothstep(0.975, 0.997, t.ridge5) * (0.2 + 0.8 * low);
    // Faint heat around the fissures.
    let warmth = smoothstep(0.85, 0.99, t.ridge3) * low * 0.12;
    // Lava seas: black crust plates drifting on the melt, glowing seams between them, white-hot
    // only where the melt is deep and open.
    let depth = max(sea - t.h, 0.0);
    let pv = t.mid * 0.6 + t.fine * 0.6 + 0.5 - depth * 2.0;
    let plate = smoothstep(0.0, 0.16, pv);
    let open = 1.0 - plate;
    let hot = smoothstep(0.02, 0.14, depth) * open * open;
    let molten = mix(L.colors[1].rgb * 0.75, L.colors[0].rgb, hot);
    let pool = mix(molten * 0.25, L.colors[4].rgb * 0.8, plate);
    s.albedo = mix(pool, crust, land);
    let seam = exp(-sq((pv - 0.03) / 0.05)) * 0.35;
    s.emit = mix(molten * (open * 0.6 + seam), L.lights.rgb * (fis * 1.2 + warmth), land);
    s.grad = gt * land;
    return s;
}

struct Crater {
    h: f32,
    grad: vec3<f32>,
    fresh: f32,
};

// Craters, at most one per lattice cell of `p` (planet direction times a scale): a bowl, a
// raised rim and soft ejecta. Heights scale with the crater (slopes do not).
fn crater_field(p: vec3<f32>, seed: u32) -> Crater {
    var c: Crater;
    let base = floor(p - 0.5);
    for (var i = 0u; i < 8u; i++) {
        let cell = base + vec3(f32(i & 1u), f32((i >> 1u) & 1u), f32((i >> 2u) & 1u));
        let h = pn_pcg3(bitcast<vec3<u32>>(vec3<i32>(cell)) ^ vec3(seed));
        if ((h.x & 255u) > 150u) {
            continue;
        }
        let size = f32(h.y & 255u) / 255.0;
        let rad = 0.1 + 0.26 * size * size;
        let off = p - (cell + vec3<f32>(h >> vec3(8u)) * (1.0 / 16777216.0));
        let dist = length(off);
        let x = dist / rad;
        if (x >= 1.4) {
            continue;
        }
        let inside = step(x, 1.0);
        let rim_e = exp(-sq((x - 1.0) * 4.0));
        let win = 1.0 - smoothstep(1.15, 1.4, x);
        c.h += (0.55 * (x * x - 1.0) * inside + 0.3 * rim_e * win) * rad;
        let dh = 1.1 * x * inside - 9.6 * (x - 1.0) * rim_e * win;
        c.grad += off / max(dist, 1e-5) * dh;
        c.fresh = max(c.fresh, f32((h.z >> 4u) & 15u) / 15.0 * rim_e * win);
    }
    return c;
}

// Airless rock: regolith, dark maria in the basins, craters at three sizes (each fades in
// once its craters span a few pixels).
fn surf_barren(d: vec3<f32>, L: Look, t: Terrain, fp: f32) -> PSurf {
    var s: PSurf;
    let seed = pn_seed(L.shape.x) ^ 0x0C4A7E25u;
    var col = mix(L.colors[2].rgb, L.colors[3].rgb, smoothstep(-0.4, 0.5, t.warp + t.mid * 0.3));
    col = mix(col, L.colors[1].rgb, smoothstep(0.46, 0.36, t.h) * 0.85);
    col = mix(col, L.colors[4].rgb, smoothstep(0.62, 0.8, t.h) * 0.5);
    col *= 0.9 + 0.12 * t.fine;
    var gr = tangent(t.grad, d) * 0.35;
    var fresh = 0.0;
    for (var k = 0u; k < 3u; k++) {
        let scale = 6.0 * pow(2.7, f32(k));
        let vis = smoothstep(2.0, 5.0, 0.2 / (scale * fp));
        if (vis <= 0.0) {
            continue;
        }
        let c = crater_field(d * scale, seed + k * 0x51ED27u);
        gr += tangent(c.grad, d) * (vis * 2.5);
        col *= 1.0 + clamp(c.h * 2.5, -0.5, 0.3) * 0.3 * vis;
        fresh = max(fresh, c.fresh * vis);
    }
    s.albedo = mix(col, L.colors[5].rgb, fresh * 0.35);
    s.grad = gr;
    return s;
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

// Rotate `v` about the unit axis `k` by `a` (counterclockwise seen from the tip of `k`).
fn rot_axis(v: vec3<f32>, k: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    return v * c + cross(k, v) * sin(a) + k * (dot(k, v) * (1.0 - c));
}

fn pn_band_rand(i: f32, seed: u32) -> f32 {
    return f32(pn_pcg3(vec3(bitcast<u32>(i32(i)), seed, 0xBA7Du)).x >> 8u) * (1.0 / 16777216.0);
}

// Gas giants: zones and belts with uneven widths, sheared turbulence and festoons along the
// belt edges, darker poles, a great storm with a spiral core and a string of white ovals.
fn gas_surface(d: vec3<f32>, L: Look, lod: f32) -> PSurf {
    var s: PSurf;
    let seed = pn_seed(L.shape.x);
    let bands = max(L.style.y, 1.0);
    let turb = L.style.z;
    // The storm: an oval vortex; twist the cloud deck around its center.
    let sc = normalize(vec3(0.62, -0.36, 0.7));
    let east = normalize(vec3(sc.z, 0.0, -sc.x));
    let north = cross(sc, east);
    let rel = d - sc;
    let u = dot(rel, east);
    let v = dot(rel, north);
    let re = length(vec2(u * 0.5, v)) / 0.12;
    let twist = 4.0 * exp(-re * re * 0.6) * step(0.0, dot(d, sc));
    let cs = cos(twist);
    let sn = sin(twist);
    let u2 = (u * 0.5 * cs - v * sn) * 2.0;
    let v2 = u * 0.5 * sn + v * cs;
    let dd = normalize(d + east * (u2 - u) + north * (v2 - v));
    // Turbulence stretched along the bands, swirled by eddies (flow along noise contours).
    let n1 = pn_simplex(dd * vec3(2.2, 7.0, 2.2), seed);
    let sw = cross(dd, n1.yzw) * 0.012;
    let n2 = pn_simplex(dd * vec3(6.0, 20.0, 6.0) + sw * 20.0, seed + 1u);
    var fine = n2.x;
    if (lod > 5.0) {
        let n3 = pn_simplex(dd * vec3(16.0, 50.0, 16.0) + cross(dd, n2.yzw) * 0.25, seed + 2u);
        fine += n3.x * 0.6 * saturate(lod - 5.0);
    }
    var y = dd.y + turb * (n1.x * 0.045 + n2.x * 0.014 + sw.y);
    let lon = atan2(dd.x, dd.z);
    let bph = y * bands;
    let br = pn_band_rand(floor(bph), seed);
    let edge = abs(fract(bph) * 2.0 - 1.0);
    // Festoons: waves rolling along the belt edges.
    y += sin(lon * (9.0 + 14.0 * br) + n1.x * 3.0 + br * 6.0) * 0.009 * turb * edge * edge * edge;
    // Zones (light) and belts (dark) with uneven widths, plus thin sub-bands.
    let yb = y * bands * PI;
    let b = sin(yb + 0.8 * sin(yb * 0.35 + 2.0)) + 0.35 * sin(yb * 2.7 + 1.3) + 0.15 * sin(yb * 6.1 + 0.4);
    let zone = smoothstep(-0.45, 0.55, b);
    let tone = pn_band_rand(floor(y * bands * 0.5 + 0.5), seed + 9u);
    var col = mix(L.colors[4].rgb, L.colors[3].rgb, zone);
    col = mix(col, L.colors[2].rgb, tone * 0.6);
    col = mix(col, L.colors[5].rgb * 0.9, smoothstep(0.75, 1.0, tone) * (1.0 - zone) * 0.5);
    col *= 1.0 + turb * (0.16 * fine + 0.08 * n1.x);
    let polar = mix(L.colors[2].rgb, L.colors[4].rgb, 0.5) * vec3(0.72, 0.76, 0.86);
    col = mix(col, polar, smoothstep(0.72, 0.95, abs(d.y)));
    // The storm: brick red, a pale collar, spiral structure from the twist.
    let storm = smoothstep(1.0, 0.45, re);
    let collar = smoothstep(1.5, 1.0, re) * (1.0 - storm);
    col = mix(col, L.colors[3].rgb * 1.08, collar * 0.6);
    col = mix(col, L.colors[5].rgb * (0.75 + 0.45 * zone) * (1.0 + 0.15 * fine), storm * 0.92);
    for (var i = 0u; i < 4u; i++) {
        let lo = 0.9 + f32(i) * 0.75;
        let oc = vec3(0.814 * sin(lo), -0.581, 0.814 * cos(lo));
        let od = length((d - oc) * vec3(0.75, 1.5, 0.75));
        col = mix(col, L.colors[3].rgb * 1.15, smoothstep(0.045, 0.02, od) * 0.85);
    }
    s.albedo = col;
    return s;
}

// Albedo, light, relief and water of the surface at unit direction `d`, with `lod` octaves
// for a pixel footprint of `fp` planet radii.
fn planet_surface(d: vec3<f32>, L: Look, lod: f32, fp: f32) -> PSurf {
    if (L.style.y > 0.0) {
        return gas_surface(d, L, lod);
    }
    let t = pn_terrain(d, L, lod);
    let model = u32(max(L.colors[0].w, 1.0) + 0.5);
    if (model == S_ICE) {
        return surf_ice(d, L, t, fp);
    }
    if (model == S_VOLCANIC) {
        return surf_volcanic(d, L, t, fp);
    }
    if (model == S_BARREN) {
        return surf_barren(d, L, t, fp);
    }
    return surf_living(d, L, t, fp);
}

struct Cloud {
    // Density (`cloud_cover` turns it into coverage) and its gradient.
    c: f32,
    grad: vec3<f32>,
    // Gradient of the fine octaves alone (puffy tops).
    fine: vec3<f32>,
};

// Cloud density field: weather systems streaked along latitude by the jets, three cyclones
// with spiral rain bands and clear eyes, a rainy equatorial belt and clear subtropics. `lod`
// adds detail octaves.
fn cloud_field(d: vec3<f32>, L: Look, time: f32, lod: f32) -> Cloud {
    let seed = pn_seed(L.shape.x) ^ 0x5BD1E995u;
    // Dry worlds have weak storms.
    let storms = saturate(L.clouds.a * 2.2);
    var p = rot_y(d, time * 0.006);
    var bands = 0.0;
    for (var i = 0u; i < 3u; i++) {
        let r = pn_rand3(vec3(f32(i), 3.0, 7.0), seed);
        let south = select(1.0, -1.0, r.z < 0.5);
        let lat = (0.3 + 0.45 * r.x) * south;
        let lon = r.y * TAU;
        let c = vec3(cos(lat) * sin(lon), sin(lat), cos(lat) * cos(lon));
        let size = 0.13 + 0.1 * fract(r.z * 7.0);
        let off = p - c;
        let dist = length(off);
        let k = exp(-sq(dist / size));
        if (k > 0.002) {
            let e1 = normalize(vec3(c.z, 0.0, -c.x));
            let ang = atan2(dot(off, cross(c, e1)), dot(off, e1)) * south;
            let arms = 0.5 + 0.5 * cos(ang * 2.0 + log(dist / size + 0.02) * 3.5);
            let cdo = exp(-sq(dist / (size * 0.3)));
            bands += (k * mix(arms * arms * 0.75 - 0.1, 0.55, cdo) - 1.6 * exp(-sq(dist / (size * 0.055)))) * storms;
            p = rot_axis(p, c, 2.6 * k * south * storms);
        }
    }
    // Weather systems streaked along latitude, carried by a swirling flow (along the contours
    // of a stream function: eddies at every scale), detail frayed by smaller eddies.
    let s = vec3(p.x, p.y * 1.4, p.z);
    let w = pn_simplex(s * 1.6, seed + 11u);
    var c = w.x * 0.55 + bands;
    var grad = w.yzw * (0.55 * 1.6);
    var fine = vec3(0.0);
    var q = s * 3.7 + cross(p, w.yzw) * 0.22;
    var f = 3.7;
    var amp = 0.4;
    let n_oct = u32(ceil(clamp(lod, 0.0, 7.0)));
    for (var k = 0u; k < n_oct; k++) {
        let wk = saturate(lod - f32(k));
        // The two coarse octaves follow the flow; finer ones are plain puffy texture.
        let qk = select(q, d * (f * 1.15) + vec3(f32(k) * 1.7), k >= 2u);
        let n = pn_simplex(qk, seed + 20u + k);
        c += amp * n.x * wk;
        grad += (amp * f * wk) * n.yzw;
        if (k >= 1u && k <= 3u) {
            fine += (amp * f * wk) * n.yzw;
        }
        q = q * 2.11 + cross(p, n.yzw) * 0.05;
        f *= 2.11;
        amp *= 0.58;
    }
    let ay = abs(d.y);
    c += 0.16 * exp(-sq(ay / 0.1)) - 0.16 * exp(-sq((ay - 0.45) / 0.16)) + 0.06 * exp(-sq((ay - 0.78) / 0.14));
    return Cloud(c, grad, fine);
}

// Coverage 0..1 of a cloud density (from `cloud_field`) for the look's cloud amount; `aa` is
// how much the density changes over a pixel (the edge is never sharper than that).
fn cloud_cover(c: f32, L: Look, aa: f32) -> f32 {
    let thr = (0.5 - L.clouds.a) * 0.9;
    let t = saturate((c - thr + aa * 0.5) / max(0.16, aa * 1.5));
    // A thin translucent veil around the masses.
    let veil = smoothstep(thr - 0.12, thr, c) * 0.16;
    return max(t * (2.0 - t), veil);
}

// How thick the cloud is, 0 at its edge to 1 in towering cores.
fn cloud_thickness(c: f32, L: Look) -> f32 {
    return saturate((c - (0.5 - L.clouds.a) * 0.9) / 0.55);
}

// Cloud octaves worth drawing for a footprint of `fp` planet radii per pixel.
fn cloud_lod(fp: f32) -> f32 {
    return clamp(log2(0.13 / max(fp, 1e-7)) / 1.08 + 1.0, 0.0, 7.0);
}

// Display soft clip: linear up to 0.8, then a smooth shoulder toward 1 (clouds, ice and glints
// keep their detail instead of clipping).
fn soft_clip(c: vec3<f32>) -> vec3<f32> {
    let k = 0.8;
    let over = max(c - vec3(k), vec3(0.0));
    return min(c, vec3(k)) + (1.0 - k) * (1.0 - exp(-over / (1.0 - k)));
}

const WARM_LIGHT: vec3<f32> = vec3<f32>(1.0, 0.42, 0.16);

// The atmosphere seen edge-on outside the limb: dense at the surface, thinning out; blue where
// the sun lights it, orange near the terminator, brighter when the sun is behind the planet.
fn planet_halo(q: vec2<f32>, r: f32, sun: vec3<f32>, L: Look, width: f32) -> vec3<f32> {
    let hgt = max(r - 1.0, 0.0) / max(width, 1e-3);
    let en = vec3(q.x, -q.y, 0.0) / max(r, 1e-4);
    let mu = dot(en, sun);
    let dens = exp(-hgt * 4.5) * (1.0 - smoothstep(0.7, 1.0, hgt));
    let lit = smoothstep(-0.2, 0.3, mu);
    let fwd = pow(saturate(-sun.z), 4.0) * smoothstep(-0.7, 0.1, mu) * 1.2;
    let tint = mix(WARM_LIGHT, L.atmosphere.rgb, smoothstep(-0.08, 0.08, mu));
    return tint * dens * saturate(L.atmosphere.a) * (lit + fwd) * 0.9;
}

fn shade_planet(in: VOut) -> vec4<f32> {
    let L = look(in.kind.z);
    let width = in.extra.y;
    let radius = min(in.rect.z, in.rect.w) * 0.5 / (1.0 + width);
    let q = in.local.xy / radius;
    let r = length(q);
    let px = 1.0 / radius;
    let sun = normalize(vec3(sin(in.uv.z) * cos(in.uv.w), sin(in.uv.w), cos(in.uv.z) * cos(in.uv.w)));
    let has_air = L.atmosphere.a > 0.0 && width > 0.0;
    var halo = vec3(0.0);
    if (has_air && r > 1.0 - 2.0 * px) {
        halo = planet_halo(q, max(r, 1.0), sun, L, width);
    }
    let edge = saturate((1.0 - r) / px + 0.5);
    if (edge <= 0.0) {
        return vec4(halo, 0.0) * in.params.w;
    }
    let z = sqrt(max(1.0 - r * r, 0.0));
    let yaw = in.uv.x;
    let pitch = in.uv.y;
    let n = normalize(rot_y(rot_x(vec3(q.x, -q.y, z), -pitch), yaw));
    let sun_p = rot_y(rot_x(sun, -pitch), yaw);
    let view_p = rot_y(rot_x(vec3(0.0, 0.0, 1.0), -pitch), yaw);
    // Footprint of a pixel on the sphere (wider toward the limb).
    let fp = px / sqrt(max(z, 0.03));
    let gas = L.style.y > 0.0;
    let ndl = dot(n, sun_p);
    // Clouds first: under thick cloud the ground needs no fine detail.
    var cov = 0.0;
    var cf: Cloud;
    if (!gas && L.clouds.a > 0.0) {
        cf = cloud_field(n, L, g.time.x, min(cloud_lod(fp), 4.0));
        cov = cloud_cover(cf.c, L, length(tangent(cf.grad, n)) * fp);
    }
    let s = planet_surface(n, L, min(pn_lod(fp), select(7.5, 3.0, cov > 0.995)), fp);
    let air = saturate(L.atmosphere.a);
    // Low sun shines through a lot of air: orange light along the terminator.
    let sun_col = mix(vec3(1.0), WARM_LIGHT, air * 0.28 * (1.0 - smoothstep(0.0, 0.07, ndl)));
    let sun_i = 1.3;
    // Relief: bend the normal along the height gradient; peaks catch light past the terminator.
    let nb = normalize(n - s.grad * (0.07 * L.shape.z));
    let lambert = max(dot(nb, sun_p), 0.0) * smoothstep(-0.12, 0.04, ndl);
    var col = s.albedo * sun_col * (lambert * sun_i) + s.albedo * 0.003;
    if (gas) {
        col *= 0.5 + 0.5 * sqrt(z);
    }
    // Water: a tight sun glint (GGX with Fresnel) and the sky mirrored toward the limb.
    if (s.water > 0.0) {
        let hv = normalize(sun_p + view_p);
        let nh = max(dot(n, hv), 0.0);
        let vh = max(dot(view_p, hv), 0.0);
        let fres = 0.02 + 0.98 * pow(1.0 - vh, 5.0);
        let glint = (d_ggx(nh, 0.07) + 0.2 * d_ggx(nh, 0.22)) * fres / (4.0 * max(z, 0.1));
        let sky = L.atmosphere.rgb * (0.02 + 0.98 * pow(1.0 - z, 5.0)) * 0.3 * air;
        col += (sun_col * glint * sun_i * smoothstep(-0.02, 0.1, ndl) + sky * smoothstep(-0.2, 0.2, ndl)) * s.water;
    }
    // Clouds, and their shadows on the ground: the density a little sunward, from the field's
    // gradient (longer near the terminator). Tops are puffy (the fine octaves bend the normal,
    // shaded sides turn blue-grey) and thick cores brighter than thin edges.
    if (!gas && L.clouds.a > 0.0) {
        let shift = tangent(sun_p, n) * (0.008 / max(ndl, 0.15));
        let sh = cloud_cover(cf.c + dot(cf.grad, shift), L, 0.0);
        col *= 1.0 - sh * 0.5 * (1.0 - cov);
        let cn = normalize(n - tangent(cf.fine, n) * 0.018);
        let bump = dot(cn, sun_p) - ndl;
        let lit = max(ndl + bump * 0.7 * smoothstep(0.0, 0.35, ndl), 0.0) * 0.9 + 0.08 * smoothstep(-0.12, 0.08, ndl);
        let thick = cloud_thickness(cf.c, L);
        let tone = mix(vec3(0.68, 0.73, 0.82), vec3(1.0), saturate(0.72 + bump * 2.0)) * (0.8 + 0.2 * thick);
        col = mix(col, L.clouds.rgb * tone * sun_col * (lit * sun_i), cov);
    }
    // Night side: city lights through thin cloud; lava glows by day and night.
    let night = 1.0 - smoothstep(-0.1, 0.06, ndl);
    col += L.lights.rgb * (s.city * night * L.lights.a * (1.0 - 0.7 * cov) * 1.5);
    col += s.emit * (1.0 - 0.5 * cov);
    // Atmosphere: thin haze over the disc thickening toward the limb, blue where lit, a warm
    // band along the terminator, faint airglow on the night side.
    if (has_air) {
        let haze = 1.0 - exp(-air * 0.03 / (z + 0.035));
        // Near the terminator the light reaching the haze is dimmer and reddened.
        let lit_a = smoothstep(-0.06, 0.16, ndl);
        let tint = mix(WARM_LIGHT * 0.55, L.atmosphere.rgb, smoothstep(-0.02, 0.1, ndl));
        col = col * (1.0 - haze * 0.5) + tint * (haze * lit_a * sun_i);
        col += WARM_LIGHT * (exp(-sq((ndl - 0.01) / 0.03)) * 0.015 * air);
        col += L.atmosphere.rgb * (pow(1.0 - z, 5.0) * 0.03 * air * (1.0 - lit_a));
    }
    let a = edge * in.params.w;
    return vec4(soft_clip(col) * a + halo * ((1.0 - edge) * in.params.w), a);
}

fn shade_planet_flat(in: VOut) -> vec4<f32> {
    let L = look(in.kind.z);
    let t = in.local.zw;
    let lon = mix(in.uv.x, in.uv.z, t.x);
    let lat = mix(in.uv.y, in.uv.w, t.y);
    let cl = cos(lat);
    let sl = sin(lat);
    let d = vec3(cl * sin(lon), sl, cl * cos(lon));
    // Planet radii per pixel: detail octaves follow the zoom (crisp at 16x, no aliasing when
    // the whole world fits).
    let dlon = abs(in.uv.z - in.uv.x) / max(in.rect.z, 1.0);
    let dlat = abs(in.uv.w - in.uv.y) / max(in.rect.w, 1.0);
    let fp = max(max(dlon * cl, dlat), 1e-6);
    let s = planet_surface(d, L, pn_lod(fp), fp);
    var col = s.albedo * mix(1.2, 0.92, saturate(luminance(s.albedo) * 1.4));
    // Hillshade, light from the north-west.
    let east = vec3(cos(lon), 0.0, -sin(lon));
    let north = vec3(-sl * sin(lon), cl, -sl * cos(lon));
    let k = 0.1 * L.shape.z;
    let nrm = normalize(vec3(-dot(s.grad, east) * k, -dot(s.grad, north) * k, 1.0));
    let ldir = vec3(-0.6, 0.6, 0.53);
    col *= clamp(dot(nrm, ldir) / ldir.z, 0.3, 1.8);
    col += s.emit;
    // Clouds, light and detailed: it is a map. Only the denser ones, translucent at the edges
    // and thinning out as the map zooms in; shaded like the hills.
    if (L.style.y <= 0.0 && in.extra.z > 0.0 && L.clouds.a > 0.0) {
        let cf = cloud_field(d, L, g.time.x, cloud_lod(fp));
        let cov = cloud_cover(cf.c - 0.05, L, length(tangent(cf.grad, d)) * fp);
        let thick = cloud_thickness(cf.c, L);
        let cshade = clamp(1.0 + (dot(cf.fine, east) - dot(cf.fine, north)) * 0.012, 0.8, 1.1);
        let zoom = mix(0.45, 1.0, smoothstep(0.0006, 0.004, fp));
        col = mix(col, L.clouds.rgb * (0.88 + 0.12 * thick) * cshade, cov * mix(0.3, 0.75, thick) * zoom * in.extra.z);
    }
    // Graticule every 30 degrees.
    let deg = vec2(lon, lat) * 57.29578;
    let gw = abs(vec2((in.uv.z - in.uv.x) / in.rect.z, (in.uv.w - in.uv.y) / in.rect.w)) * 57.29578 * 1.2;
    let gl = abs(fract(deg / 30.0 + 0.5) - 0.5) * 30.0;
    let grid = (1.0 - smoothstep(0.0, gw.x, gl.x)) + (1.0 - smoothstep(0.0, gw.y, gl.y));
    col = mix(soft_clip(col), vec3(0.85, 0.92, 1.0), saturate(grid) * 0.22 * in.extra.w);
    return vec4(col, 1.0) * in.params.w;
}

// ---------------------------------------------------------------------------------------
// Space backgrounds

fn pn_rand2(c: vec2<f32>, seed: u32) -> vec3<f32> {
    let h = pn_pcg3(vec3(bitcast<vec2<u32>>(vec2<i32>(c)), seed));
    return vec3<f32>(h >> vec3(8u)) * (1.0 / 16777216.0);
}

// 2D simplex noise, about -1..1.
fn pn_simplex2(p: vec2<f32>, seed: u32) -> f32 {
    let i = floor(p + (p.x + p.y) * 0.36602540);
    let a = p - i + (i.x + i.y) * 0.21132487;
    let o = select(vec2(0.0, 1.0), vec2(1.0, 0.0), a.x > a.y);
    let b = a - o + 0.21132487;
    let c = a - 1.0 + 0.42264973;
    let h = max(vec3(0.5) - vec3(dot(a, a), dot(b, b), dot(c, c)), vec3(0.0));
    let ga = pn_rand2(i, seed).xy * 2.0 - 1.0;
    let gb = pn_rand2(i + o, seed).xy * 2.0 - 1.0;
    let gc = pn_rand2(i + 1.0, seed).xy * 2.0 - 1.0;
    let n = h * h * h * h * vec3(dot(a, ga), dot(b, gb), dot(c, gc));
    return dot(n, vec3(90.0));
}

fn fbm_s2(p: vec2<f32>, seed: u32, octaves: u32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0u; i < octaves; i++) {
        sum += amp * pn_simplex2(q, seed + i * PN_SALT_OCT);
        q = mat2x2<f32>(1.6, 1.2, -1.2, 1.6) * q;
        amp *= 0.5;
    }
    return sum;
}

// Star-forming knots in the arms: pink HII regions just past the dust lanes, blue clusters on
// the crests.
fn galaxy_knots(p: vec2<f32>, pw: f32, env: f32, seed: u32) -> vec3<f32> {
    var col = vec3(0.0);
    let hii = exp(-sq((pw - 0.24) / 0.2)) * env;
    let blue = exp(-sq((pw + 0.05) / 0.35)) * env;
    for (var k = 0u; k < 3u; k++) {
        let cell = 0.05 / (1.0 + f32(k) * 1.4);
        let cp = p / cell;
        let rnd = pn_rand2(floor(cp), seed + 31u + k);
        let dd = length(fract(cp) - (vec2(0.3) + 0.4 * rnd.xy));
        let size = 0.05 + 0.13 * fract(rnd.z * 13.7);
        let blob = (exp(-sq(dd / size)) + 0.3 * exp(-dd / (size * 1.6))) * (1.0 - smoothstep(0.18, 0.3, dd));
        if (rnd.z > 0.55) {
            col += vec3(1.0, 0.3, 0.52) * (blob * hii * 4.0 * (0.3 + (rnd.z - 0.55) / 0.45));
        } else if (rnd.z < 0.2) {
            col += vec3(0.62, 0.76, 1.0) * (blob * blue * 2.2 * (0.4 + rnd.z * 3.0));
        }
    }
    return col;
}

// Resolved stars: three grain sizes in disk space, each fading in once its cells span a few
// pixels; dots are round on screen (rotation and tilt undone).
fn galaxy_stars(p: vec2<f32>, dens: f32, radius: f32, tilt: f32, ca: f32, sa: f32, seed: u32) -> vec3<f32> {
    var col = vec3(0.0);
    for (var k = 0u; k < 3u; k++) {
        let cell = 0.012 / pow(2.4, f32(k));
        let vis = smoothstep(2.0, 4.0, cell * radius * sqrt(tilt));
        if (vis <= 0.0) {
            continue;
        }
        let cp = p / cell;
        let rnd = pn_rand2(floor(cp), seed + 50u + k);
        if (rnd.z > dens) {
            continue;
        }
        let o = (fract(cp) - (vec2(0.2) + 0.6 * rnd.xy)) * cell;
        let dpx = vec2(o.x * ca + o.y * sa, (o.y * ca - o.x * sa) * tilt) * radius;
        let b = 0.15 + 0.85 * pow(fract(rnd.z * 31.7), 4.0);
        let tint = mix(vec3(0.72, 0.8, 1.0), vec3(1.0, 0.84, 0.64), fract(rnd.x * 17.3));
        col += tint * (b * exp(-dot(dpx, dpx) / 0.9) * vis);
    }
    return col;
}

fn shade_galaxy(in: VOut) -> vec4<f32> {
    let center = in.uv.xy;
    let radius = in.uv.z;
    let p0 = (in.screen - center) / radius;
    // Tilt squashes y; then turn by the rotation.
    let tilt = max(in.extra.y, 0.05);
    let ca = cos(in.uv.w);
    let sa = sin(in.uv.w);
    let pt = vec2(p0.x, p0.y / tilt);
    let p = vec2(pt.x * ca - pt.y * sa, pt.x * sa + pt.y * ca);
    let r = length(p);
    if (r > 1.6) {
        return vec4(0.0);
    }
    let seed = 0x6A1A7E5u;
    let theta = atan2(p.y, p.x);
    let lr = log(max(r, 0.02));
    // Two arms along theta * 2 - ln(r) * 4.2 = 0 (map.rs puts the star systems there).
    let warp = fbm_s2(p * 2.6 + vec2(1.7, 4.1), seed, 3);
    let ph = theta * 2.0 - lr * 4.2 + warp * 0.6;
    let pw = atan2(sin(ph), cos(ph));
    let arm_env = smoothstep(0.05, 0.28, r) * exp(-max(r - 0.3, 0.0) * 2.2) * (1.0 - smoothstep(0.95, 1.3, r));
    let arm = exp(-pw * pw * 3.2);
    let arm_wide = exp(-pw * pw * 0.8);
    let clumps = fbm_s2(p * 10.0 + vec2(warp * 2.0), seed + 7u, 4);
    let arm_l = arm * arm_env * (0.3 + 1.2 * saturate(0.45 + clumps));
    // Light (HDR): a warm bulge with a smooth cusp, a faint patchy old-star disk, young arms
    // (whiter near the core, blue outside) in a soft violet glow, a halo.
    let bulge = 2.2 * exp(-r / 0.035) + 0.9 * exp(-r * r / 0.01) + 0.28 * exp(-r / 0.13);
    let disk = 0.075 * exp(-r / 0.34) * (1.0 - smoothstep(1.0, 1.4, r)) * (0.75 + 0.5 * saturate(0.5 + clumps));
    let halo = 0.03 * exp(-r / 0.6) * (1.0 - smoothstep(1.05, 1.6, r));
    let young = mix(vec3(0.8, 0.78, 0.92), vec3(0.36, 0.52, 1.0), smoothstep(0.12, 0.5, r));
    var col = vec3(1.0, 0.7, 0.4) * bulge + vec3(0.9, 0.76, 0.66) * disk + young * (arm_l * 0.78)
        + vec3(0.45, 0.42, 0.95) * (arm_wide * arm_env * 0.09) + vec3(0.5, 0.52, 0.8) * halo;
    // Dust lanes along the inner edge of each arm (phase > 0): thick where they hug the arm,
    // breaking into filaments and spurs that cross it; thin patchy dust between the arms.
    let fn1 = fbm_s2(vec2(pw * 2.5, lr * 6.0) + p * 5.0, seed + 13u, 4);
    let fil = fbm_s2(p * 18.0 + vec2(fn1 * 1.5), seed + 17u, 3);
    let feathers = smoothstep(-0.3, 0.35, fn1);
    let lane = exp(-sq((pw - 0.5) / 0.3)) * feathers * (0.55 + 0.6 * smoothstep(-0.2, 0.4, fil));
    let spurs = smoothstep(0.2, 0.5, fil) * arm * smoothstep(-0.3, 0.5, pw) * 0.55;
    let inter = 0.14 * smoothstep(0.0, 0.5, fn1) * (1.0 - arm_wide);
    let dust_env = smoothstep(0.06, 0.25, r) * (1.0 - smoothstep(0.8, 1.15, r));
    let tau = (lane * 1.5 + spurs + inter) * dust_env
        + 0.25 * exp(-r / 0.1) * smoothstep(0.02, 0.09, r) * feathers;
    col *= exp(-tau * vec3(1.0, 1.25, 1.6));
    col += galaxy_knots(p, pw, arm_env * saturate(0.65 + clumps) * saturate(1.0 - tau * 0.5), seed);
    let dens = saturate(arm_l * 0.8 + disk * 3.0 + bulge * 0.2 + 0.03) * 0.75;
    col += galaxy_stars(p, dens, radius, tilt, ca, sa, seed);
    // Tone map: no clipping; the core rolls off to a warm white.
    let mapped = vec3(1.0) - exp(-col * 1.3);
    let fade = 1.0 - smoothstep(1.3, 1.6, r);
    let alpha = saturate((1.0 - exp(-tau)) * 0.45 * dust_env + luminance(mapped) * 0.6) * fade;
    return vec4(mapped * fade, alpha) * in.params.w;
}

// Star color by temperature: 0 hot blue, 0.3 white, 0.6 yellow, 1 red.
fn star_tint(t: f32) -> vec3<f32> {
    let a = mix(vec3(0.6, 0.72, 1.0), vec3(1.0, 0.98, 0.96), smoothstep(0.0, 0.3, t));
    let b = mix(a, vec3(1.0, 0.86, 0.66), smoothstep(0.4, 0.7, t));
    return mix(b, vec3(1.0, 0.62, 0.42), smoothstep(0.75, 1.0, t));
}

fn shade_starfield(in: VOut) -> vec4<f32> {
    let pan = in.uv.xy;
    let density = in.uv.z;
    let seed = pn_seed(in.uv.w);
    // Nebulae: broad faint clouds of gas in two colors, with wisps and dark dust, far away
    // (slow parallax). Kept dark: UI sits on top.
    let np = (in.screen + pan * 0.1) / 1100.0;
    let w = vec2(fbm_s2(np * 1.1, seed, 2), fbm_s2(np * 1.1 + vec2(5.2, 1.3), seed + 1u, 2));
    let n1 = fbm_s2(np * 1.6 + w * 0.8, seed + 2u, 3);
    let n2 = fbm_s2(np * 2.1 - w * 0.7 + vec2(3.1, 7.7), seed + 3u, 3);
    let wisp = fbm_s2(np * 6.0 + w * 2.5, seed + 4u, 4);
    let g1 = smoothstep(-0.1, 0.5, n1) * (0.65 + 0.7 * wisp);
    let g2 = smoothstep(-0.05, 0.55, n2) * (0.65 - 0.6 * wisp);
    var col = vec3(0.28, 0.06, 0.22) * (saturate(g1) * 0.07) + vec3(0.03, 0.13, 0.22) * (saturate(g2) * 0.07)
        + vec3(0.3, 0.13, 0.04) * (saturate(g1 * g2) * 0.03);
    col *= 1.0 - smoothstep(0.1, 0.45, wisp - n1 * 0.3) * 0.6;
    let glow = saturate((g1 + g2) * 0.5);
    // Stars in four layers, many faint and few bright (with a soft glow, the brightest with
    // faint spikes); nearer layers move more with the pan.
    for (var layer = 0u; layer < 4u; layer++) {
        let lf = f32(layer);
        let cell = 7.0 * pow(2.45, lf);
        let p = (in.screen + pan * (0.08 + 0.14 * lf)) / cell + vec2(lf * 17.0, lf * 5.0);
        let h = pn_pcg3(vec3(bitcast<vec2<u32>>(vec2<i32>(floor(p))), seed + layer));
        let ex = f32(h.z & 255u) / 255.0;
        if (ex > density * (0.5 + 0.3 * glow) * (0.55 - 0.05 * lf)) {
            continue;
        }
        let br = f32((h.z >> 8u) & 255u) / 255.0;
        let tp = f32((h.z >> 16u) & 255u) / 255.0;
        let pos = 0.3 + 0.4 * vec2<f32>(h.xy >> vec2(8u)) * (1.0 / 16777216.0);
        let dv = (fract(p) - pos) * cell;
        let d2 = dot(dv, dv);
        let b = (0.05 + 0.95 * pow(br, 5.0)) * (0.35 + 0.45 * lf);
        let sigma = 0.5 + 0.12 * lf + 0.35 * br * br;
        var s = exp(-d2 / (2.0 * sigma * sigma));
        if (layer >= 2u) {
            let gr = 1.2 + 2.5 * br;
            let cut = 1.0 - smoothstep(cell * 0.12, cell * 0.28, sqrt(d2));
            s += 0.1 * br / (1.0 + d2 / (gr * gr)) * cut;
            if (layer == 3u && br > 0.7) {
                let ad = abs(dv);
                let len = 3.0 + 6.0 * br;
                let spike = exp(-ad.x * 1.6) * exp(-ad.y / len) + exp(-ad.y * 1.6) * exp(-ad.x / len);
                s += spike * 0.18 * (br - 0.7) / 0.3 * cut;
            }
        }
        let tw = 0.85 + 0.15 * sin(g.time.x * (0.7 + 2.0 * tp) + br * 40.0);
        col += star_tint(tp * (1.0 - 0.5 * br)) * (b * s * tw);
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
        // A planet's shadow across its rings (DrawCtx::ring_shadowed): a strip as wide as the
        // globe, leading away from the sun, in the ring's own (untilted) plane.
        let pr = in.extra.y;
        if (pr > 0.0) {
            let q = vec2(p.x, -p.y * radii.x / radii.y);
            let d = vec2(sin(in.extra.z), cos(in.extra.z));
            let along = dot(q, d);
            let perp = abs(q.x * d.y - q.y * d.x);
            let soft = pr * 0.04 + 1.0;
            let shade = smoothstep(pr + soft, pr - soft, perp) * smoothstep(0.0, soft, along);
            out = vec4(out.rgb * (1.0 - shade * in.extra.w), out.a);
        }
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
