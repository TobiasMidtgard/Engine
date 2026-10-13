// ============================================================================
// Procedural material generator: building blocks (material_library.rs).
// Requires lib_types. Composed as lib_types + this + one preset (shaders/materials/*.wgsl)
// + material_gen_main.wgsl.
//
// Everything works in tile coordinates `uv` in [0, 1) and repeats with period 1, so every map
// tiles seamlessly: lattice noises wrap their cells, and frequencies are whole numbers of cells
// per tile. Heights are in meters; `mm(x)` converts millimeters to tile units.
// ============================================================================

// ---------------------------------------------------------------- hashing
fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn hash_u2(c: vec2<i32>, seed: u32) -> u32 {
    return pcg(bitcast<u32>(c.x) ^ pcg(bitcast<u32>(c.y) ^ pcg(seed)));
}

// Uniform float in [0, 1) from a hash.
fn u01(h: u32) -> f32 {
    return f32(h >> 8u) * (1.0 / 16777216.0);
}

// Next independent value from a hash (for several random numbers per cell).
fn next_hash(h: u32) -> u32 {
    return pcg(h ^ 0x9e3779b9u);
}

fn wrap_cell(c: vec2<i32>, period: vec2<i32>) -> vec2<i32> {
    return ((c % period) + period) % period;
}

// Random value of a lattice cell, repeating every `period` cells.
fn cell_rand(c: vec2<i32>, period: vec2<i32>, seed: u32) -> f32 {
    return u01(hash_u2(wrap_cell(c, period), seed));
}

fn cell_rand2(c: vec2<i32>, period: vec2<i32>, seed: u32) -> vec2<f32> {
    let h = hash_u2(wrap_cell(c, period), seed);
    return vec2<f32>(u01(h), u01(next_hash(h)));
}

// ---------------------------------------------------------------- noise
// Quintic fade (C2-continuous: no visible creases in derived normals).
fn fade2(f: vec2<f32>) -> vec2<f32> {
    return f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
}

fn grad_at(c: vec2<i32>, period: vec2<i32>, seed: u32) -> vec2<f32> {
    let a = cell_rand(c, period, seed) * TAU;
    return vec2<f32>(cos(a), sin(a));
}

// Tileable gradient (Perlin) noise in about [-1, 1]. `p` in lattice cells; the pattern repeats
// every `period` cells.
fn gnoise(p: vec2<f32>, period: vec2<i32>, seed: u32) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let u = fade2(f);
    let a = dot(grad_at(i, period, seed), f);
    let b = dot(grad_at(i + vec2<i32>(1, 0), period, seed), f - vec2<f32>(1.0, 0.0));
    let c = dot(grad_at(i + vec2<i32>(0, 1), period, seed), f - vec2<f32>(0.0, 1.0));
    let d = dot(grad_at(i + vec2<i32>(1, 1), period, seed), f - vec2<f32>(1.0, 1.0));
    return 1.41 * mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Tileable value noise in [0, 1].
fn vnoise(p: vec2<f32>, period: vec2<i32>, seed: u32) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let u = fade2(f);
    let a = cell_rand(i, period, seed);
    let b = cell_rand(i + vec2<i32>(1, 0), period, seed);
    let c = cell_rand(i + vec2<i32>(0, 1), period, seed);
    let d = cell_rand(i + vec2<i32>(1, 1), period, seed);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Gradient noise at `freq` cells per tile (x and y may differ for stretched patterns).
fn noise(uv: vec2<f32>, freq: vec2<i32>, seed: u32) -> f32 {
    return gnoise(uv * vec2<f32>(freq), freq, seed);
}

// Fractal sum of gradient noise, normalized to about [-1, 1]. Starts at `freq` cells per tile
// and doubles every octave (so it still tiles); `gain` is the amplitude ratio per octave.
fn fbm(uv: vec2<f32>, freq: vec2<i32>, octaves: i32, gain: f32, seed: u32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var norm = 0.0;
    var f = freq;
    for (var i = 0; i < octaves; i++) {
        sum += amp * gnoise(uv * vec2<f32>(f), f, seed + u32(i) * 7919u);
        norm += amp;
        amp *= gain;
        f *= 2;
    }
    return sum / norm;
}

fn fbm1(uv: vec2<f32>, freq: i32, octaves: i32, gain: f32, seed: u32) -> f32 {
    return fbm(uv, vec2<i32>(freq), octaves, gain, seed);
}

// Ridged multifractal in [0, 1]: sharp crests where the noise crosses zero (veins, cracks, rock
// ridges). Each octave is weighted by the one before, so ridges branch off ridges.
fn ridged(uv: vec2<f32>, freq: vec2<i32>, octaves: i32, gain: f32, seed: u32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var norm = 0.0;
    var weight = 1.0;
    var f = freq;
    for (var i = 0; i < octaves; i++) {
        var r = 1.0 - abs(gnoise(uv * vec2<f32>(f), f, seed + u32(i) * 6151u));
        r = r * r * weight;
        weight = saturate(r * 1.5);
        sum += amp * r;
        norm += amp;
        amp *= gain;
        f *= 2;
    }
    return sum / norm;
}

// Two fbm values for domain warping.
fn fbm2v(uv: vec2<f32>, freq: vec2<i32>, octaves: i32, gain: f32, seed: u32) -> vec2<f32> {
    return vec2<f32>(fbm(uv, freq, octaves, gain, seed), fbm(uv, freq, octaves, gain, seed + 1013u));
}

// Domain warp: shift `uv` by `amount` tile units of fbm (still tiles).
fn warp(uv: vec2<f32>, freq: vec2<i32>, octaves: i32, amount: f32, seed: u32) -> vec2<f32> {
    return uv + amount * fbm2v(uv, freq, octaves, 0.5, seed);
}

// ---------------------------------------------------------------- cellular (Worley / Voronoi)
struct Cell {
    f1: f32,             // distance to the nearest feature point (cells)
    f2: f32,             // distance to the second nearest (cells)
    edge: f32,           // distance to the nearest cell border (cells, exact)
    id: u32,             // random id of the nearest cell
    to_center: vec2<f32>,// vector from the point to the nearest feature point (cells)
};

fn feature_point(c: vec2<i32>, period: vec2<i32>, jitter: f32, seed: u32) -> vec2<f32> {
    return vec2<f32>(0.5) + (cell_rand2(c, period, seed) - 0.5) * jitter;
}

// Tileable Voronoi with exact border distance (Quilez, "Voronoi edges"). `freq` cells per tile;
// `jitter` 0 (a regular grid) .. 1 (fully random points).
fn voronoi(uv: vec2<f32>, freq: vec2<i32>, jitter: f32, seed: u32) -> Cell {
    let p = uv * vec2<f32>(freq);
    let n = vec2<i32>(floor(p));
    let f = p - floor(p);
    var md = 8.0;
    var md2 = 8.0;
    var mr = vec2<f32>(0.0);
    var mg = vec2<i32>(0);
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let g = vec2<i32>(i, j);
            let r = vec2<f32>(g) + feature_point(n + g, freq, jitter, seed) - f;
            let d = dot(r, r);
            if (d < md) {
                md2 = md;
                md = d;
                mr = r;
                mg = g;
            } else if (d < md2) {
                md2 = d;
            }
        }
    }
    var me = 8.0;
    for (var j = -2; j <= 2; j++) {
        for (var i = -2; i <= 2; i++) {
            let g = mg + vec2<i32>(i, j);
            let r = vec2<f32>(g) + feature_point(n + g, freq, jitter, seed) - f;
            let dr = r - mr;
            if (dot(dr, dr) > 1e-6) {
                me = min(me, dot(0.5 * (mr + r), normalize(dr)));
            }
        }
    }
    var out: Cell;
    out.f1 = sqrt(md);
    out.f2 = sqrt(md2);
    out.edge = me;
    out.id = hash_u2(wrap_cell(n + mg, freq), seed ^ 0x5bd1e995u);
    out.to_center = mr;
    return out;
}

// Scattered round blobs ("splatter"): for each of `freq`² cells, one blob of random radius
// `rmin..rmax` (cells) whose center is somewhere in the cell. Returns the largest blob height
// profile (1 at the center, 0 at the rim) and the id of that blob in `id_out.x`. Overlapping
// neighbors are considered, so blobs may cross cell borders.
struct Blob {
    h: f32,
    id: u32,
    d: f32,  // distance to the center / radius (0 center .. 1 rim)
    to_center: vec2<f32>,
};

fn blobs(uv: vec2<f32>, freq: vec2<i32>, rmin: f32, rmax: f32, seed: u32) -> Blob {
    let p = uv * vec2<f32>(freq);
    let n = vec2<i32>(floor(p));
    let f = p - floor(p);
    var best: Blob;
    best.h = 0.0;
    best.id = 0u;
    best.d = 2.0;
    best.to_center = vec2<f32>(0.0);
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let g = vec2<i32>(i, j);
            let c = wrap_cell(n + g, freq);
            let h0 = hash_u2(c, seed);
            let center = vec2<f32>(g) + vec2<f32>(u01(h0), u01(next_hash(h0))) - f;
            let r = mix(rmin, rmax, u01(next_hash(next_hash(h0))));
            let d = length(center) / r;
            if (d < 1.0) {
                let h = sqrt(1.0 - d * d) * r;
                if (h > best.h) {
                    best.h = h;
                    best.id = next_hash(next_hash(next_hash(h0)));
                    best.d = d;
                    best.to_center = center;
                }
            }
        }
    }
    return best;
}

// ---------------------------------------------------------------- tiles and bricks
struct Tile {
    local: vec2<f32>, // position from the tile center (tile units)
    half: vec2<f32>,  // half size of the cell (tile units)
    cell: vec2<i32>,  // column, row (wrapped)
    id: u32,
};

// A running-bond grid: `cols` x `rows` cells per tile; every odd row shifts by `offset` of a
// cell (0.5 = half bond). `rows` must be even for the pattern to tile when `offset` != 0.
fn brick_grid(uv: vec2<f32>, cols: i32, rows: i32, offset: f32, seed: u32) -> Tile {
    let fr = uv.y * f32(rows);
    let row = i32(floor(fr));
    let shift = f32(row & 1) * offset;
    let fc = uv.x * f32(cols) + shift;
    let col = i32(floor(fc));
    var t: Tile;
    t.cell = wrap_cell(vec2<i32>(col, row), vec2<i32>(cols, rows));
    t.half = vec2<f32>(0.5 / f32(cols), 0.5 / f32(rows));
    t.local = vec2<f32>((fract(fc) - 0.5) / f32(cols), (fract(fr) - 0.5) / f32(rows));
    t.id = hash_u2(t.cell, seed);
    return t;
}

// Thin straight scratches. Each of `freq`² cells holds a scratch with probability `density`: a
// segment up to `len` cells long (at most 2) through a random point of the cell, at angle `dir`
// (radians) ± `spread`. Returns the strongest scratch at `uv`: up to 1 on its center line
// (scaled by a random depth per scratch), fading to 0 at `width` cells and toward its ends.
fn scratches(uv: vec2<f32>, freq: i32, density: f32, len: f32, width: f32, dir: f32, spread: f32, seed: u32) -> f32 {
    let p = uv * f32(freq);
    let n = vec2<i32>(floor(p));
    var best = 0.0;
    for (var j = -2; j <= 2; j++) {
        for (var i = -2; i <= 2; i++) {
            let g = n + vec2<i32>(i, j);
            let h = hash_u2(wrap_cell(g, vec2<i32>(freq)), seed);
            if (u01(h) > density) {
                continue;
            }
            let center = vec2<f32>(g) + vec2<f32>(u01(pcg(h + 1u)), u01(pcg(h + 2u)));
            let a = dir + rnd(h, 3u) * spread;
            let l = len * (0.3 + 0.7 * u01(pcg(h + 4u)));
            let axis = vec2<f32>(cos(a), sin(a));
            let q = p - center;
            let along = dot(q, axis) / (0.5 * l);
            let dist = abs(dot(q, vec2<f32>(-axis.y, axis.x)));
            let w = width * (0.5 + 0.5 * u01(pcg(h + 5u))) * (1.0 - along * along * 0.6);
            let s = (1.0 - smoothstep(0.0, max(w, 1e-4), dist)) * step(abs(along), 1.0)
                * (0.4 + 0.6 * u01(pcg(h + 6u)));
            best = max(best, s);
        }
    }
    return best;
}

// Signed distance to a rounded box (negative inside).
fn sd_round_box(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// Distance to a segment a-b.
fn sd_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = saturate(dot(pa, ba) / max(dot(ba, ba), 1e-12));
    return length(pa - ba * h);
}

// Wrap a tile-space difference into [-0.5, 0.5) (shortest way around the torus).
fn wrap_delta(d: vec2<f32>) -> vec2<f32> {
    return d - floor(d + 0.5);
}

// ---------------------------------------------------------------- shaping
// Millimeters to tile units.
fn mm(x: f32) -> f32 {
    return x * 0.001 / gen.dims.x;
}

// Tile units to millimeters.
fn to_mm(x: f32) -> f32 {
    return x * gen.dims.x * 1000.0;
}

fn remap(x: f32, a: f32, b: f32) -> f32 {
    return saturate((x - a) / (b - a));
}

// Smooth minimum / maximum (polynomial, k = blend width).
fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = saturate(0.5 + 0.5 * (b - a) / k);
    return mix(b, a, h) - k * h * (1.0 - h);
}

fn smax(a: f32, b: f32, k: f32) -> f32 {
    return -smin(-a, -b, k);
}

// ---------------------------------------------------------------- color
// Linear RGB from an sRGB hex color (0xRRGGBB).
fn hex(c: u32) -> vec3<f32> {
    let s = vec3<f32>(f32((c >> 16u) & 255u), f32((c >> 8u) & 255u), f32(c & 255u)) / 255.0;
    return select(pow((s + 0.055) / 1.055, vec3<f32>(2.4)), s / 12.92, s <= vec3<f32>(0.04045));
}

// Scale brightness and saturation of a linear color (for per-tile variation).
fn vary(c: vec3<f32>, value: f32, saturation: f32) -> vec3<f32> {
    let l = luminance(c);
    return max((vec3<f32>(l) + (c - vec3<f32>(l)) * saturation) * value, vec3<f32>(0.0));
}

// Random value in [-1, 1) from a hash and an index.
fn rnd(h: u32, i: u32) -> f32 {
    return u01(pcg(h + i * 0x68e31da4u)) * 2.0 - 1.0;
}
