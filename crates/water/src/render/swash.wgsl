// ============================================================================
// The swash band (`crate::swash`, whose CPU step this mirrors): the full shallow-water
// equations on a 0.125 m grid along the shore, so waves run up the sand, thin out and drain.
// Finite volumes, HLL fluxes on Audusse's hydrostatic reconstruction (depth ≥ 0, a lake at
// rest stays at rest), minmod slopes where the water is wet all round, SSP-RK2 in time,
// Manning friction, sand soaking water up; in a strip offshore the state is blended toward the
// dispersive surface's waves and the ripple grid's currents (relaxation zones).
//
// Work goes by tiles of 16×16 cells, one workgroup each, from `tiles` (the band, and the
// tiles round it that the band reads, flagged by the top bit when in the band).
//
// Each frame: `sample_targets` samples the surface's waves over the band; then per step `prep_a`
// (slopes of this frame's state), `stage1` (an Euler stage into the second half of `water`),
// `prep_b`, `stage2` (the average, friction, the sand, the strip); then `after` makes and
// fades foam, keeps what the sand remembers and writes the textures the water shader reads.
// ============================================================================

struct Params {
    grid: vec4<f32>,    // cells per side, cell (m), a step's dt (s), the frame's dt (s)
    look: vec4<f32>,    // foam life (s), the lake's waves, the surface's fine level on (1/0), its coarse level on
    fine: vec4<f32>,    // the surface's fine level: corner rel this grid's corner x, z (m), cell (m), cells
    coarse: vec4<f32>,  // its coarse level, the same
    ripple: vec4<f32>,  // the ripple grid, the same
    shift: vec4<i32>,   // cells this grid moved by since the last frame (x, z), pushes, unused
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> water: array<vec4<f32>>;   // (h, hu, hv, foam·h): this frame's, then a stage's
@group(0) @binding(2) var<storage, read_write> slopes: array<vec4<f32>>;  // per cell, along x then z: (h, level, u, v)
@group(0) @binding(3) var<storage, read> ground: array<vec4<f32>>;        // bed, still level (NONE: none), the strip's weight, unused
@group(0) @binding(4) var<storage, read_write> sand: array<vec4<f32>>;    // sheen, damp, mark, foam while covered
@group(0) @binding(5) var<storage, read_write> targets: array<vec4<f32>>; // the surface's height, velocity x, z, foam
@group(0) @binding(6) var<storage, read> tiles: array<u32>;
@group(0) @binding(7) var<storage, read> fine: array<vec2<f32>>;          // (h, φ) per cell
@group(0) @binding(8) var<storage, read> coarse: array<vec2<f32>>;
@group(0) @binding(9) var ripple_tex: texture_2d<f32>;                    // r height, g foam
@group(0) @binding(10) var current_tex: texture_2d<f32>;                  // currents x, z
@group(0) @binding(11) var out_a: texture_storage_2d<rgba16float, write>; // level drawn (m over the datum), depth, strip weight, foam
@group(0) @binding(12) var out_b: texture_storage_2d<rgba16float, write>; // sheen, damp, mark, bed (m over the datum)
@group(0) @binding(13) var<storage, read> waves: array<vec4<f32>>;        // per wave: (dir x, z, k, amp), (ω, phase at the corner, 0, 0)
@group(0) @binding(14) var<storage, read_write> press: array<f32>;        // pressure head of what stands in the water (m)
@group(0) @binding(15) var<storage, read> pushes: array<vec4<f32>>;      // per push: (x, z from the corner, radius, depth), (stir x, z, foam, stirs 1/0)
@group(0) @binding(16) var<storage, read_write> reach: array<f32>;       // damp per REACH_BLOCK² cells

const N: u32 = 512u;
const TILE: u32 = 16u;
const G: f32 = 9.81;
// `swash::` constants.
const DRY_M: f32 = 5e-4;
const H_EPS: f32 = 1e-3;
const MANNING: f32 = 0.025;
const INFILTRATION_MS: f32 = 6e-4;
const MAX_SPEED: f32 = 5.0;
const SHEEN_S: f32 = 4.0;
const DAMP_S: f32 = 90.0;
const MARK_S: f32 = 20.0;
const TABLE_M: f32 = 0.05;
const BORE_SLOPE: f32 = 0.3;
const BORE_FOAM: f32 = 4.0;
const TIP_DEPTH_M: f32 = 0.02;
const TIP_SPEED_MS: f32 = 0.4;
const TIP_FOAM: f32 = 2.0;
// A still level below this is none.
const NONE: f32 = -1e4;
const STIR_RATE: f32 = 8.0;
const MAX_PRESS_M: f32 = 1.0;
const REACH_BLOCK: u32 = 8u;

fn velocity(h: f32, q: f32) -> f32 {
    let h4 = h * h * h * h;
    let e = H_EPS * H_EPS * H_EPS * H_EPS;
    let v = 1.41421356 * h * q / sqrt(h4 + max(h4, e));
    return clamp(v, -MAX_SPEED, MAX_SPEED);
}

fn minmod3(a: f32, b: f32, c: f32) -> f32 {
    if (a > 0.0 && b > 0.0 && c > 0.0) { return min(a, min(b, c)); }
    if (a < 0.0 && b < 0.0 && c < 0.0) { return max(a, max(b, c)); }
    return 0.0;
}

fn slope(l: f32, c: f32, r: f32) -> f32 {
    let theta = 1.3;
    return minmod3(theta * (c - l), 0.5 * (r - l), theta * (r - c));
}

fn at(i: i32, j: i32) -> u32 {
    let m = i32(N) - 1;
    return u32(clamp(j, 0, m)) * N + u32(clamp(i, 0, m));
}

// The cell this invocation steps, and whether its tile is in the band.
struct Work {
    k: u32,
    i: i32,
    j: i32,
    band: bool,
};

fn work(wg: u32, local: vec2<u32>) -> Work {
    let t = tiles[wg];
    let tile = t & 0x7fffffffu;
    let per_row = N / TILE;
    let i = (tile % per_row) * TILE + local.x;
    let j = (tile / per_row) * TILE + local.y;
    return Work(j * N + i, i32(i), i32(j), (t >> 31u) != 0u);
}

// The bed the water flows over: the ground, and the pressure of what stands on the water.
fn bed(k: u32) -> f32 {
    return ground[k].x + press[k];
}

fn wet(base: u32, k: u32) -> bool {
    return water[base + k].x > DRY_M;
}

// (h, level, u, v) of cell k in the state at `base`.
fn quantities(base: u32, k: u32) -> vec4<f32> {
    let c = water[base + k];
    return vec4<f32>(c.x, c.x + bed(k), velocity(c.x, c.y), velocity(c.x, c.z));
}

fn prep(base: u32, wg: u32, local: vec2<u32>) {
    let w = work(wg, local);
    let k = w.k;
    let nb = array<u32, 4>(at(w.i - 1, w.j), at(w.i + 1, w.j), at(w.i, w.j - 1), at(w.i, w.j + 1));
    if (!wet(base, k) || !wet(base, nb[0]) || !wet(base, nb[1]) || !wet(base, nb[2]) || !wet(base, nb[3])) {
        slopes[2u * k] = vec4<f32>(0.0);
        slopes[2u * k + 1u] = vec4<f32>(0.0);
        return;
    }
    let c = quantities(base, k);
    let we = quantities(base, nb[0]);
    let ea = quantities(base, nb[1]);
    let so = quantities(base, nb[2]);
    let no = quantities(base, nb[3]);
    var sx: vec4<f32>;
    var sz: vec4<f32>;
    for (var m = 0; m < 4; m++) {
        sx[m] = slope(we[m], c[m], ea[m]);
        sz[m] = slope(so[m], c[m], no[m]);
    }
    slopes[2u * k] = sx;
    slopes[2u * k + 1u] = sz;
}

@compute @workgroup_size(16, 16)
fn prep_a(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    prep(0u, wg.x, l.xy);
}

@compute @workgroup_size(16, 16)
fn prep_b(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    prep(N * N, wg.x, l.xy);
}

// One side of cell k's face along `axis` (0 x, 1 z), `sign` +1 the far face: depth, bed,
// velocity across and along it, foam.
struct Side {
    h: f32,
    b: f32,
    un: f32,
    ut: f32,
    c: f32,
};

fn side(base: u32, k: u32, axis: u32, sign: f32) -> Side {
    let c = water[base + k];
    let d = slopes[2u * k + axis];
    let h = max(c.x + 0.5 * sign * d.x, 0.0);
    let level = c.x + bed(k) + 0.5 * sign * d.y;
    let u = velocity(c.x, c.y) + 0.5 * sign * d.z;
    let v = velocity(c.x, c.z) + 0.5 * sign * d.w;
    var foam = 0.0;
    if (c.x > DRY_M) { foam = clamp(c.w / c.x, 0.0, 1.0); }
    if (axis == 0u) {
        return Side(h, level - h, u, v, foam);
    }
    return Side(h, level - h, v, u, foam);
}

fn mirrored(s: Side) -> Side {
    return Side(s.h, s.b, -s.un, s.ut, s.c);
}

// Flux through a face (depth, normal and tangential discharge, foam) and the hydrostatic
// corrections each side adds to its normal momentum.
struct Flux {
    f: vec4<f32>,
    corr_l: f32,
    corr_r: f32,
};

fn face_flux(l: Side, r: Side) -> Flux {
    let b = max(l.b, r.b);
    let hl = max(l.h + l.b - b, 0.0);
    let hr = max(r.h + r.b - b, 0.0);
    let corr_l = 0.5 * G * (l.h * l.h - hl * hl);
    let corr_r = 0.5 * G * (r.h * r.h - hr * hr);
    if (hl <= 0.0 && hr <= 0.0) { return Flux(vec4<f32>(0.0), corr_l, corr_r); }
    let cl = sqrt(G * hl);
    let cr = sqrt(G * hr);
    let sl = min(l.un - cl, r.un - cr);
    let sr = max(l.un + cl, r.un + cr);
    let fl = vec3<f32>(hl * l.un, hl * l.un * l.un + 0.5 * G * hl * hl, hl * l.un * l.ut);
    let fr = vec3<f32>(hr * r.un, hr * r.un * r.un + 0.5 * G * hr * hr, hr * r.un * r.ut);
    var f: vec3<f32>;
    if (sl >= 0.0) {
        f = fl;
    } else if (sr <= 0.0) {
        f = fr;
    } else {
        let ul = vec3<f32>(hl, hl * l.un, hl * l.ut);
        let ur = vec3<f32>(hr, hr * r.un, hr * r.ut);
        f = (sr * fl - sl * fr + sl * sr * (ur - ul)) / (sr - sl);
    }
    let c = select(r.c, l.c, f.x >= 0.0);
    return Flux(vec4<f32>(f, f.x * c), corr_l, corr_r);
}

// d(h, hu, hv, foam·h)/dt of cell (i, j) in the state at `base`.
fn rate(base: u32, w: Work) -> vec4<f32> {
    let k = w.k;
    let inv_dx = 1.0 / params.grid.y;
    var d = vec4<f32>(0.0);
    for (var axis = 0u; axis < 2u; axis++) {
        var lo: u32;
        var hi: u32;
        var first: bool;
        var last: bool;
        if (axis == 0u) {
            lo = at(w.i - 1, w.j);
            hi = at(w.i + 1, w.j);
            first = w.i == 0;
            last = w.i == i32(N) - 1;
        } else {
            lo = at(w.i, w.j - 1);
            hi = at(w.i, w.j + 1);
            first = w.j == 0;
            last = w.j == i32(N) - 1;
        }
        let near = side(base, k, axis, -1.0);
        let far = side(base, k, axis, 1.0);
        var beyond_hi = side(base, hi, axis, -1.0);
        if (last) { beyond_hi = mirrored(far); }
        var beyond_lo = side(base, lo, axis, 1.0);
        if (first) { beyond_lo = mirrored(near); }
        let f_hi = face_flux(far, beyond_hi);
        let f_lo = face_flux(beyond_lo, near);
        let source = -G * 0.5 * (near.h + far.h) * (far.b - near.b);
        let dn = -(f_hi.f.y + f_hi.corr_l - f_lo.f.y - f_lo.corr_r) + source;
        let dt_ = -(f_hi.f.z - f_lo.f.z);
        d.x -= (f_hi.f.x - f_lo.f.x) * inv_dx;
        d.w -= (f_hi.f.w - f_lo.f.w) * inv_dx;
        if (axis == 0u) {
            d.y += dn * inv_dx;
            d.z += dt_ * inv_dx;
        } else {
            d.z += dn * inv_dx;
            d.y += dt_ * inv_dx;
        }
    }
    return d;
}

fn clamp_cell(c: vec4<f32>) -> vec4<f32> {
    if (c.x <= 0.0) { return vec4<f32>(0.0); }
    return vec4<f32>(c.x, c.y, c.z, clamp(c.w, 0.0, c.x));
}

@compute @workgroup_size(16, 16)
fn stage1(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    let w = work(wg.x, l.xy);
    if (!w.band) {
        water[N * N + w.k] = water[w.k];
        return;
    }
    water[N * N + w.k] = clamp_cell(water[w.k] + params.grid.z * rate(0u, w));
}

@compute @workgroup_size(16, 16)
fn stage2(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    let w = work(wg.x, l.xy);
    if (!w.band) { return; }
    let k = w.k;
    let dt = params.grid.z;
    let euler = water[N * N + k] + dt * rate(N * N, w);
    var c = clamp_cell(0.5 * (water[k] + euler));
    let g = ground[k];
    // Friction, and the sand soaking water up.
    if (c.x <= DRY_M) {
        c = vec4<f32>(c.x, 0.0, 0.0, c.w);
    } else {
        let speed = length(vec2<f32>(velocity(c.x, c.y), velocity(c.x, c.z)));
        let keep = 1.0 / (1.0 + G * MANNING * MANNING * dt * speed / pow(c.x, 4.0 / 3.0));
        c = vec4<f32>(c.x, c.y * keep, c.z * keep, c.w);
        if (g.y > NONE && g.x > g.y) {
            let h = max(c.x - INFILTRATION_MS * dt, 0.0);
            c *= h / c.x;
        }
    }
    // The strip: toward the surface's waves.
    if (g.z > 0.0 && g.y > NONE) {
        let t = targets[k];
        let h = max(g.y + t.x - bed(k), 0.0);
        c = mix(c, vec4<f32>(h, h * t.y, h * t.z, h * t.w), g.z);
    }
    water[k] = clamp_cell(c);
}

// Bilinear (h, φ) of a surface level at `p` m from this grid's corner, and φ's gradient.
fn level_sample(lv: vec4<f32>, which: u32, p: vec2<f32>) -> vec3<f32> {
    let n = i32(lv.w);
    let g = (p - lv.xy) / lv.z - vec2<f32>(0.5);
    if (any(g < vec2<f32>(1.0)) || any(g > vec2<f32>(f32(n) - 2.0))) { return vec3<f32>(0.0); }
    let i = vec2<i32>(floor(g));
    let f = g - vec2<f32>(i);
    var v: array<vec2<f32>, 16>;
    for (var dj = 0; dj < 4; dj++) {
        for (var di = 0; di < 4; di++) {
            let idx = u32((i.y + dj - 1) * n + i.x + di - 1);
            if (which == 0u) { v[dj * 4 + di] = fine[idx]; } else { v[dj * 4 + di] = coarse[idx]; }
        }
    }
    // Height bilinear from the middle four; φ's slope by differences across them.
    let h = mix(mix(v[5].x, v[6].x, f.x), mix(v[9].x, v[10].x, f.x), f.y);
    let px0 = mix(v[6].y - v[4].y, v[10].y - v[8].y, f.y) * 0.5;
    let px1 = mix(v[7].y - v[5].y, v[11].y - v[9].y, f.y) * 0.5;
    let pz0 = mix(v[9].y - v[1].y, v[10].y - v[2].y, f.x) * 0.5;
    let pz1 = mix(v[13].y - v[5].y, v[14].y - v[6].y, f.x) * 0.5;
    let grad = vec2<f32>(mix(px0, px1, f.x), mix(pz0, pz1, f.y)) / lv.z;
    return vec3<f32>(h, grad);
}

fn ripple_load(p: vec2<f32>) -> vec4<f32> {
    let n = i32(params.ripple.w);
    let g = (p - params.ripple.xy) / params.ripple.z - vec2<f32>(0.5);
    let i = clamp(vec2<i32>(floor(g)), vec2<i32>(0), vec2<i32>(n - 2));
    let fr = clamp(g - vec2<f32>(i), vec2<f32>(0.0), vec2<f32>(1.0));
    let a = textureLoad(ripple_tex, i, 0);
    let b = textureLoad(ripple_tex, i + vec2<i32>(1, 0), 0);
    let c = textureLoad(ripple_tex, i + vec2<i32>(0, 1), 0);
    let d = textureLoad(ripple_tex, i + vec2<i32>(1, 1), 0);
    let ca = textureLoad(current_tex, i, 0);
    let cb = textureLoad(current_tex, i + vec2<i32>(1, 0), 0);
    let cc = textureLoad(current_tex, i + vec2<i32>(0, 1), 0);
    let cd = textureLoad(current_tex, i + vec2<i32>(1, 1), 0);
    let r = mix(mix(a, b, fr.x), mix(c, d, fr.x), fr.y);
    let cur = mix(mix(ca, cb, fr.x), mix(cc, cd, fr.x), fr.y);
    // (height, foam, current x, z)
    return vec4<f32>(r.x, r.y, cur.x, cur.y);
}

@compute @workgroup_size(16, 16)
fn sample_targets(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    let w = work(wg.x, l.xy);
    if (!w.band || ground[w.k].z <= 0.0) { return; }
    let p = (vec2<f32>(f32(w.i), f32(w.j)) + 0.5) * params.grid.y;
    let r = ripple_load(p);
    var eta = r.x;
    var vel = r.zw;
    if (params.look.z > 0.5) {
        let s = level_sample(params.fine, 0u, p);
        eta = s.x;
        vel += s.yz;
    }
    if (params.look.w > 0.5) {
        let s = level_sample(params.coarse, 1u, p);
        eta += s.x;
        vel += s.yz;
    }
    // The lake's own waves, faded in the shallows as the water shader fades them, each moving
    // the water under it at a ω/(k·d) of its height (linear waves, averaged over the depth).
    let depth = max(ground[w.k].y - ground[w.k].x, 0.1);
    for (var i = 0u; i < u32(params.look.y); i++) {
        let a = waves[2u * i];
        let b = waves[2u * i + 1u];
        let shoal = tanh(a.z * depth * 1.2);
        let eta_i = a.w * shoal * sin(a.z * dot(a.xy, p) - b.y);
        eta += eta_i;
        vel += a.xy * eta_i * b.x / (a.z * depth);
    }
    let speed = length(vel);
    if (speed > MAX_SPEED) { vel *= MAX_SPEED / speed; }
    targets[w.k] = vec4<f32>(eta, vel, saturate(r.y));
}

@compute @workgroup_size(16, 16)
fn after(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    let w = work(wg.x, l.xy);
    if (!w.band) { return; }
    let k = w.k;
    let dt = params.grid.w;
    let c = water[k];
    let g = ground[k];
    var s = sand[k];
    let fade = exp(-dt / max(params.look.x, 1e-3));
    var level = c.x + g.x;
    var foam = 0.0;
    if (c.x > DRY_M) {
        let v = vec2<f32>(velocity(c.x, c.y), velocity(c.x, c.z));
        // A bore front: the surface steep and the water running into the slope.
        let here = level;
        var lv: array<f32, 4>;
        let nb = array<u32, 4>(at(w.i + 1, w.j), at(w.i - 1, w.j), at(w.i, w.j + 1), at(w.i, w.j - 1));
        for (var m = 0; m < 4; m++) {
            let o = water[nb[m]];
            lv[m] = select(here, o.x + ground[nb[m]].x, o.x > DRY_M);
        }
        let grad = vec2<f32>(lv[0] - lv[1], lv[2] - lv[3]) * (0.5 / params.grid.y);
        var make = 0.0;
        if (length(grad) > BORE_SLOPE && dot(grad, v) < 0.0) { make += BORE_FOAM * dt; }
        // The tip of the uprush: thin, fast, toward dry sand.
        let ahead = at(w.i + i32(sign(v.x)), w.j + i32(sign(v.y)));
        if (c.x < TIP_DEPTH_M && length(v) > TIP_SPEED_MS && water[ahead].x <= DRY_M) {
            make += TIP_FOAM * dt;
        }
        foam = min((clamp(c.w / c.x, 0.0, 1.0) + make) * fade, 1.0);
        water[k].w = foam * c.x;
        s = vec4<f32>(1.0, 1.0, s.z, foam);
    } else {
        // Just drained: the foam it carried stays on the sand.
        if (s.x >= 1.0) { s.z = max(s.z, s.w); }
        s.w = 0.0;
        water[k].w = 0.0;
        s.x *= exp(-dt / SHEEN_S);
        let table = g.y > NONE && g.x <= g.y + TABLE_M;
        if (!table) { s.y *= exp(-dt / DAMP_S); }
        s.z *= exp(-dt / MARK_S);
        // Drawn just under the wet neighbours' level, so the drawn waterline comes from the
        // real ground's depth test, not from these cells.
        var best = NONE;
        let nb = array<u32, 4>(at(w.i + 1, w.j), at(w.i - 1, w.j), at(w.i, w.j + 1), at(w.i, w.j - 1));
        for (var m = 0; m < 4; m++) {
            let o = water[nb[m]];
            if (o.x > DRY_M) { best = max(best, o.x + ground[nb[m]].x); }
        }
        level = select(g.x - 0.05, best - 0.01, best > NONE);
    }
    sand[k] = s;
    let p = vec2<i32>(w.i, w.j);
    textureStore(out_a, p, vec4<f32>(level, c.x, g.z, foam));
    textureStore(out_b, p, vec4<f32>(s.x, s.y, s.z, g.x));
}

// What stands in the water this frame (`crate::swash::SwashField::push`): its pressure, over
// every tile stepped (the band reads its neighbours' beds); its wash and foam, over the band.
@compute @workgroup_size(16, 16)
fn push(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    let w = work(wg.x, l.xy);
    let k = w.k;
    let p = (vec2<f32>(f32(w.i), f32(w.j)) + 0.5) * params.grid.y;
    let drag = 1.0 - exp(-STIR_RATE * params.grid.w);
    var head = 0.0;
    var c = water[k];
    for (var m = 0u; m < u32(params.shift.z); m++) {
        let a = pushes[2u * m];
        let r = max(a.z, params.grid.y * 0.75);
        let dist = length(p - a.xy) / r;
        if (dist > 2.0) { continue; }
        let g = exp(-2.0 * dist * dist);
        head = min(head + max(a.w, 0.0) * g, MAX_PRESS_M);
        if (!w.band || c.x <= DRY_M) { continue; }
        let b = pushes[2u * m + 1u];
        if (b.w > 0.5) {
            var v = vec2<f32>(velocity(c.x, c.y), velocity(c.x, c.z));
            v += (b.xy - v) * (drag * g);
            c = vec4<f32>(c.x, c.x * v, c.w);
        }
        c.w = min(c.w + c.x * b.z * g, c.x);
    }
    press[k] = head;
    if (w.band) { water[k] = c; }
}

// How damp the sand is per meter, for the shore's wetness (read back now and then).
@compute @workgroup_size(8, 8)
fn reach_damp(@builtin(global_invocation_id) id: vec3<u32>) {
    let m = N / REACH_BLOCK;
    if (id.x >= m || id.y >= m) { return; }
    var d = 0.0;
    for (var j = 0u; j < REACH_BLOCK; j++) {
        for (var i = 0u; i < REACH_BLOCK; i++) {
            d = max(d, sand[(id.y * REACH_BLOCK + j) * N + id.x * REACH_BLOCK + i].y);
        }
    }
    reach[id.y * m + id.x] = d;
}

// A fresh bed: the water at rest, the sand dry, nothing drawn.
@compute @workgroup_size(16, 16)
fn rest(@builtin(global_invocation_id) id: vec3<u32>) {
    let k = id.y * N + id.x;
    let g = ground[k];
    var h = 0.0;
    if (g.y > NONE) { h = max(g.y - g.x, 0.0); }
    water[k] = vec4<f32>(h, 0.0, 0.0, 0.0);
    sand[k] = vec4<f32>(0.0);
    targets[k] = vec4<f32>(0.0);
    textureStore(out_a, vec2<i32>(id.xy), vec4<f32>(NONE, 0.0, 1.0, 0.0));
    textureStore(out_b, vec2<i32>(id.xy), vec4<f32>(0.0, 0.0, 0.0, g.x));
}

// The grid moved by `params.shift` cells: carry the water and the sand along (into the
// scratch halves), new cells at rest.
@compute @workgroup_size(16, 16)
fn shift_out(@builtin(global_invocation_id) id: vec3<u32>) {
    let k = id.y * N + id.x;
    let src = vec2<i32>(id.xy) + params.shift.xy;
    let g = ground[k];
    var c = vec4<f32>(0.0);
    var s = vec4<f32>(0.0);
    if (all(src >= vec2<i32>(0)) && all(src < vec2<i32>(i32(N)))) {
        let o = u32(src.y) * N + u32(src.x);
        c = water[o];
        s = sand[o];
    } else if (g.y > NONE) {
        c = vec4<f32>(max(g.y - g.x, 0.0), 0.0, 0.0, 0.0);
    }
    water[N * N + k] = c;
    slopes[k] = s;
}

@compute @workgroup_size(16, 16)
fn shift_in(@builtin(global_invocation_id) id: vec3<u32>) {
    let k = id.y * N + id.x;
    water[k] = water[N * N + k];
    sand[k] = slopes[k];
    targets[k] = vec4<f32>(0.0);
    textureStore(out_a, vec2<i32>(id.xy), vec4<f32>(NONE, 0.0, 1.0, 0.0));
    textureStore(out_b, vec2<i32>(id.xy), vec4<f32>(sand[k].xyz, ground[k].x));
}
