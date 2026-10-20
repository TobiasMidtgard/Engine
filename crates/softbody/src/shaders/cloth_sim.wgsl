// Flags, banners and pennants simulated on the GPU (`SimSettings::gpu`).
//
// One workgroup per cloth: a grid of up to 512 particles held in workgroup memory for a
// whole frame of substeps. Each substep integrates gravity, gusting wind (pressure drag and
// skin friction on each particle's share of the cloth) and damping, then solves the
// stretch, shear and bend links (XPBD, one pass in twelve colours of independent links),
// and takes velocities from the motion. Positions are kept relative to the anchor in world axes, so
// a frame's output is the anchor (relative to the camera) plus the particle, written as
// the same vertices the CPU cloth draws.
//
// Must match `GpuFlag` in gpu_cloth.rs.
struct Flag {
    // Anchor relative to the camera; w = seconds since start (gusts).
    anchor: vec4<f32>,
    // Rotation (quaternion xyzw) from the grid's plane to the world.
    rot: vec4<f32>,
    // How far the anchor moved since the last frame; w = substep length (s).
    shift: vec4<f32>,
    // Wind (m/s) at the anchor; w = turbulence (0..1).
    wind: vec4<f32>,
    // Gravity (m/s²); w = damping (1/s).
    gravity: vec4<f32>,
    // Width and height (m); the grid's anchor point (u, v).
    size: vec4<f32>,
    // Softness (compliance × particle mass / substep²) of stretch, shear and bend links;
    // w = 0.5ρ / (kg per m²).
    stiff: vec4<f32>,
    // Pressure drag, skin friction, speed limit (m/s), gust phase (s).
    air: vec4<f32>,
    // Columns, rows, first particle, material.
    grid: vec4<u32>,
    // Pinned edges (1 left, 2 top, 4 right, 8 bottom, 16 top corners), every n-th along the
    // top, substeps this frame, 1 = start from rest.
    pins: vec4<u32>,
    // Rotation (quaternion) of a world shift since the last frame, applied to the kept state.
    turn: vec4<f32>,
}

// Must match `SoftVertex` in render.rs.
struct OutVertex {
    px: f32,
    py: f32,
    pz: f32,
    nx: f32,
    ny: f32,
    nz: f32,
    u: f32,
    v: f32,
    tu: f32,
    tv: f32,
    burn_char: f32,
    burn_glow: f32,
    wet: f32,
    material: u32,
}

@group(0) @binding(0) var<storage, read> flags: array<Flag>;
// Two per particle: position (relative to the anchor), velocity.
@group(0) @binding(1) var<storage, read_write> state: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> verts: array<OutVertex>;

const WG: u32 = 256u;
const PER: u32 = 2u;

var<workgroup> pos: array<vec3<f32>, 512>;

fn rotate(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    let t = 2.0 * cross(q.xyz, v);
    return v + q.w * t + cross(q.xyz, t);
}

fn rest(f: Flag, i: u32, j: u32) -> vec3<f32> {
    let u = f32(i) / f32(f.grid.x - 1u);
    let v = f32(j) / f32(f.grid.y - 1u);
    let local = vec3<f32>((u - f.size.z) * f.size.x, -(v - f.size.w) * f.size.y, 0.0);
    return rotate(f.rot, local);
}

fn pinned(f: Flag, i: u32, j: u32) -> bool {
    let m = f.pins.x;
    let last_i = f.grid.x - 1u;
    if (m & 1u) != 0u && i == 0u {
        return true;
    }
    if (m & 2u) != 0u && j == 0u && (i % max(f.pins.y, 1u) == 0u || i == last_i) {
        return true;
    }
    if (m & 4u) != 0u && i == last_i {
        return true;
    }
    if (m & 8u) != 0u && j == f.grid.y - 1u {
        return true;
    }
    return (m & 16u) != 0u && j == 0u && (i == 0u || i == last_i);
}

// Two smooth pseudo-random signals in -1..1 over space and time (as the CPU solver's).
fn flutter(x: vec3<f32>, t: f32) -> vec2<f32> {
    let a = sin(1.9 * x.x + 2.3 * x.z + 4.1 * t) * sin(1.3 * x.y - 2.9 * t + 0.7 * x.x);
    let b = sin(2.7 * x.z - 1.7 * x.y + 3.3 * t) * sin(1.1 * x.x + 5.3 * t);
    return vec2<f32>(a, b);
}

fn at(f: Flag, i: u32, j: u32) -> vec3<f32> {
    return pos[j * f.grid.x + i];
}

// Front normal (+Z of the grid's plane at rest) from the neighbours.
fn normal_at(f: Flag, i: u32, j: u32) -> vec3<f32> {
    let left = at(f, max(i, 1u) - 1u, j);
    let right = at(f, min(i + 1u, f.grid.x - 1u), j);
    let up = at(f, i, max(j, 1u) - 1u);
    let down = at(f, i, min(j + 1u, f.grid.y - 1u));
    return cross(down - up, right - left);
}

// Solve link `l` of colour `batch` (0-1 stretch along u, 2-3 along v, 4-5 one diagonal,
// 6-7 the other, 8-9 bend along u, 10-11 along v). Links of one colour share no particle,
// so a whole colour is solved at once, Gauss-Seidel style: each link moves its free ends by
// its XPBD correction, the pinned ends giving nothing.
fn solve(f: Flag, l: u32, batch: u32, du: f32, dv: f32) {
    let nu = f.grid.x;
    let nv = f.grid.y;
    let i = l % nu;
    let j = l / nu;
    let c = batch & 1u;
    var a = vec2<u32>(i, j);
    var b = a;
    var rest = 0.0;
    var soft = 0.0;
    var mine = false;
    switch batch >> 1u {
        case 0u: {
            b = vec2<u32>(i + 1u, j);
            rest = du;
            soft = f.stiff.x;
            mine = i % 2u == c;
        }
        case 1u: {
            b = vec2<u32>(i, j + 1u);
            rest = dv;
            soft = f.stiff.x;
            mine = j % 2u == c;
        }
        case 2u: {
            b = vec2<u32>(i + 1u, j + 1u);
            rest = sqrt(du * du + dv * dv);
            soft = f.stiff.y;
            mine = i % 2u == c;
        }
        case 3u: {
            a = vec2<u32>(i + 1u, j);
            b = vec2<u32>(i, j + 1u);
            rest = sqrt(du * du + dv * dv);
            soft = f.stiff.y;
            mine = i % 2u == c && i + 1u < nu;
        }
        case 4u: {
            b = vec2<u32>(i + 2u, j);
            rest = 2.0 * du;
            soft = f.stiff.z;
            mine = (i / 2u) % 2u == c;
        }
        default: {
            b = vec2<u32>(i, j + 2u);
            rest = 2.0 * dv;
            soft = f.stiff.z;
            mine = (j / 2u) % 2u == c;
        }
    }
    if !mine || b.x >= nu || b.y >= nv {
        return;
    }
    let wa = select(1.0, 0.0, pinned(f, a.x, a.y));
    let wb = select(1.0, 0.0, pinned(f, b.x, b.y));
    if wa + wb == 0.0 {
        return;
    }
    let ia = a.y * nu + a.x;
    let ib = b.y * nu + b.x;
    let d = pos[ib] - pos[ia];
    let len = length(d);
    if len < 1e-9 {
        return;
    }
    let corr = d * ((len - rest) / ((wa + wb + soft) * len));
    pos[ia] += corr * wa;
    pos[ib] -= corr * wb;
}

@compute @workgroup_size(256)
fn cs_simulate(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) lid: u32) {
    let f = flags[wg.x];
    let nu = f.grid.x;
    let nv = f.grid.y;
    let n = nu * nv;
    let first = f.grid.z;
    let h = f.shift.w;
    var x: array<vec3<f32>, 2>;
    var v: array<vec3<f32>, 2>;
    var prev: array<vec3<f32>, 2>;
    var pin: array<bool, 2>;

    for (var k = 0u; k < PER; k++) {
        let p = lid + k * WG;
        if p < n {
            let i = p % nu;
            let j = p / nu;
            pin[k] = pinned(f, i, j);
            if f.pins.w != 0u || pin[k] {
                x[k] = rest(f, i, j);
                v[k] = vec3<f32>(0.0);
            } else {
                // The anchor moved: the free cloth stays where it was in the world.
                x[k] = rotate(f.turn, state[2u * (first + p)].xyz) - f.shift.xyz;
                v[k] = rotate(f.turn, state[2u * (first + p) + 1u].xyz);
            }
            pos[p] = x[k];
        }
    }
    workgroupBarrier();

    let du = f.size.x / f32(nu - 1u);
    let dv = f.size.y / f32(nv - 1u);
    let side = cross(f.wind.xyz, vec3<f32>(0.0, 1.0, 0.0));
    for (var s = 0u; s < f.pins.z; s++) {
        let t = f.anchor.w + f.air.w + f32(s) * h;
        // Gravity, wind and damping.
        for (var k = 0u; k < PER; k++) {
            let p = lid + k * WG;
            if p < n && !pin[k] {
                var a = f.gravity.xyz;
                let nn = normal_at(f, p % nu, p / nu);
                let len = length(nn);
                if len > 1e-12 {
                    let nrm = nn / len;
                    let g = flutter(x[k], t);
                    let gust = f.wind.xyz * (1.0 + f.wind.w * g.x) + side * (0.5 * f.wind.w * g.y);
                    let rel = gust - v[k];
                    let vn = dot(rel, nrm);
                    let vt = rel - nrm * vn;
                    a += (nrm * (f.air.x * vn * abs(vn)) + vt * (f.air.y * length(vt))) * f.stiff.w;
                }
                v[k] = (v[k] + a * h) * max(1.0 - f.gravity.w * h, 0.0);
                let speed = length(v[k]);
                if speed > f.air.z {
                    v[k] *= f.air.z / speed;
                }
                prev[k] = x[k];
                x[k] += v[k] * h;
            }
        }
        // Everyone has read the neighbours before anyone moves.
        workgroupBarrier();
        for (var k = 0u; k < PER; k++) {
            let p = lid + k * WG;
            if p < n {
                pos[p] = x[k];
            }
        }
        workgroupBarrier();
        // Stretch, shear and bend links, one colour at a time.
        for (var batch = 0u; batch < 12u; batch++) {
            for (var k = 0u; k < PER; k++) {
                let l = lid + k * WG;
                if l < n {
                    solve(f, l, batch, du, dv);
                }
            }
            workgroupBarrier();
        }
        for (var k = 0u; k < PER; k++) {
            let p = lid + k * WG;
            if p < n {
                x[k] = pos[p];
                if !pin[k] {
                    v[k] = (x[k] - prev[k]) / h;
                }
            }
        }
    }

    // Keep the state and write this frame's vertices.
    for (var k = 0u; k < PER; k++) {
        let p = lid + k * WG;
        if p < n {
            let i = p % nu;
            let j = p / nu;
            state[2u * (first + p)] = vec4<f32>(x[k], 0.0);
            state[2u * (first + p) + 1u] = vec4<f32>(v[k], 0.0);
            let nn = normal_at(f, i, j);
            let nrm = select(rotate(f.rot, vec3<f32>(0.0, 0.0, 1.0)), normalize(nn), length(nn) > 1e-12);
            let il = max(i, 1u) - 1u;
            let ir = min(i + 1u, nu - 1u);
            let ju = max(j, 1u) - 1u;
            let jd = min(j + 1u, nv - 1u);
            let tu = length(at(f, ir, j) - at(f, il, j)) / (du * f32(ir - il)) - 1.0;
            let tv = length(at(f, i, jd) - at(f, i, ju)) / (dv * f32(jd - ju)) - 1.0;
            let w = f.anchor.xyz + x[k];
            verts[first + p] = OutVertex(
                w.x, w.y, w.z,
                nrm.x, nrm.y, nrm.z,
                f32(i) / f32(nu - 1u), f32(j) / f32(nv - 1u),
                tu, tv,
                0.0, 0.0,
                0.0,
                f.grid.w,
            );
        }
    }
}
