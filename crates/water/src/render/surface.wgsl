// ============================================================================
// Dispersive ripples (`crate::surface`): the surface over the ripple grid evolved exactly for
// every wavelength at once in Fourier space (Tessendorf, "eWave", 2014). Height h and velocity
// potential φ obey, per wavenumber k over water D deep,
//   dh/dt = a·φ,   dφ/dt = −b·h,   a = k·tanh(kD),   b = g + (σ/ρ)·k²,
// so each mode turns at ω = √(ab) (gravity waves plus capillary ones): long waves outrun short
// ones, rings spread into trains and moving hulls draw Kelvin wakes.
//
// It runs on two levels, each with its own buffers and these passes (`crate::surface`): the
// fine one holds the short waves, the coarse one the long, each mode in one of them.
//
// Each frame: `spatial` carries the surface along the currents (semi-Lagrangian), shuts dry
// land and soaks waves up at the grid's edge, and packs z = h + iφ (both real, so one
// complex transform carries both), and packs the pushes the same way (height pushed, and
// the potential a pressure starts: dφ/dt = −g·h − p/ρ); `fft_*_fwd` and `fft_*_push`
// transform them; `evolve` splits the spectra (H = (Z(k) + Z*(−k))/2,
// Φ = (Z(k) − Z*(−k))/2i), adds this level's band of the pushes, turns each mode by ω·dt and
// packs them again; `fft_*_inv` transform back; `resolve` keeps the result, writes the
// texture the water shader reads (height, dh/dx, dh/dz) and the heights read back for
// floating things.
// ============================================================================

struct Params {
    grid: vec4<f32>,  // cells per side, cell size (m), dt (s), depth (m)
    damp: vec4<f32>,  // wave damping (1/s), viscosity (m²/s) for the shortest waves, σ/ρ (m³/s²), pushes on
    shift: vec4<i32>, // cells the grid moved by since the last frame (x, z), unused, unused
    band: vec4<f32>,  // crossover wavenumber (1/m), 0 all waves / 1 the short / 2 the long, sponge (cells), unused
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> surface: array<vec2<f32>>; // (h, φ) per cell
@group(0) @binding(2) var<storage, read_write> work: array<vec2<f32>>;    // three complex fields
@group(0) @binding(3) var<storage, read> pushes: array<vec2<f32>>;        // height pushed, pressure impulse (m·s) per cell
@group(0) @binding(4) var<storage, read> medium: array<vec2<u32>>;        // f16 (vx, vz), (wet, 0)
@group(0) @binding(5) var<storage, read_write> heights: array<f32>;       // read back
@group(0) @binding(6) var out_tex: texture_storage_2d<rgba16float, write>;

const N: u32 = 256u;
const LOG_N: u32 = 8u;
const G: f32 = 9.81;

fn cmul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

fn conj(a: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x, -a.y);
}

// Catmull-Rom weights of the four samples around a point `t` (0..1) past the second.
fn catmull_rom(t: f32) -> vec4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    return 0.5 * vec4<f32>(-t3 + 2.0 * t2 - t, 3.0 * t3 - 5.0 * t2 + 2.0, -3.0 * t3 + 4.0 * t2 + t, t3 - t2);
}

// Last frame's surface at grid coordinates `p` (cell centers at whole numbers, in this
// frame's grid), through a cubic (a bilinear lookup each frame would blur away the short waves
// wherever the water moves); still water off the grid.
fn surface_at(p: vec2<f32>) -> vec2<f32> {
    let q = p + vec2<f32>(params.shift.xy);
    let base = floor(q);
    let wx = catmull_rom(q.x - base.x);
    let wz = catmull_rom(q.y - base.y);
    var s = vec2<f32>(0.0);
    for (var dz = 0; dz < 4; dz++) {
        for (var dx = 0; dx < 4; dx++) {
            let c = vec2<i32>(base) + vec2<i32>(dx - 1, dz - 1);
            if (c.x < 0 || c.y < 0 || c.x >= i32(N) || c.y >= i32(N)) { continue; }
            s += surface[u32(c.y) * N + u32(c.x)] * (wx[dx] * wz[dz]);
        }
    }
    return s;
}

@compute @workgroup_size(8, 8, 1)
fn spatial(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.y * N + id.x;
    let m = medium[i];
    let wet = unpack2x16float(m.y).x;
    var s = vec2<f32>(0.0);
    if (wet > 0.0) {
        // Carried along by the currents and the stream under them.
        let v = unpack2x16float(m.x);
        let back = vec2<f32>(id.xy) - v * (params.grid.z / params.grid.y);
        s = surface_at(back);
        // Soaked up toward the edge, so what leaves one side doesn't come back on the other.
        let edge = f32(min(min(id.x, id.y), min(N - 1u - id.x, N - 1u - id.y)));
        let t = saturate(1.0 - edge / params.band.z);
        s *= exp(-6.0 * t * t * params.grid.z);
    }
    work[i] = s;
    if (params.damp.w > 0.5) {
        let p = pushes[i];
        work[2u * N * N + i] = vec2<f32>(p.x, -G * p.y);
    }
}

var<workgroup> line: array<vec2<f32>, 256>;

// FFT of `line` (loaded in bit-reversed order), in place: 128 threads, one butterfly each per
// stage; `dir` −1 forward, +1 inverse.
fn transform_line(t: u32, dir: f32) {
    for (var s = 0u; s < LOG_N; s++) {
        let half = 1u << s;
        let pos = t & (half - 1u);
        let i = ((t >> s) << (s + 1u)) + pos;
        let j = i + half;
        let ang = dir * TAU * f32(pos) / f32(2u * half);
        let bw = cmul(line[j], vec2<f32>(cos(ang), sin(ang)));
        let a = line[i];
        line[i] = a + bw;
        line[j] = a - bw;
        workgroupBarrier();
    }
}

fn fft(t: u32, base: u32, stride: u32, dir: f32) {
    for (var r = 0u; r < 2u; r++) {
        let i = t + r * 128u;
        line[reverseBits(i) >> (32u - LOG_N)] = work[base + i * stride];
    }
    workgroupBarrier();
    transform_line(t, dir);
    for (var r = 0u; r < 2u; r++) {
        let i = t + r * 128u;
        work[base + i * stride] = line[i];
    }
}

// Workgroup x: the row or column. Field 0 holds the surface going forward, field 1 the
// evolved spectrum coming back.
@compute @workgroup_size(128, 1, 1)
fn fft_rows_fwd(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wg: vec3<u32>) {
    fft(lid.x, wg.x * N, 1u, -1.0);
}

@compute @workgroup_size(128, 1, 1)
fn fft_cols_fwd(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wg: vec3<u32>) {
    fft(lid.x, wg.x, N, -1.0);
}

@compute @workgroup_size(128, 1, 1)
fn fft_rows_push(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wg: vec3<u32>) {
    fft(lid.x, 2u * N * N + wg.x * N, 1u, -1.0);
}

@compute @workgroup_size(128, 1, 1)
fn fft_cols_push(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wg: vec3<u32>) {
    fft(lid.x, 2u * N * N + wg.x, N, -1.0);
}

@compute @workgroup_size(128, 1, 1)
fn fft_rows_inv(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wg: vec3<u32>) {
    fft(lid.x, N * N + wg.x * N, 1u, 1.0);
}

@compute @workgroup_size(128, 1, 1)
fn fft_cols_inv(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wg: vec3<u32>) {
    fft(lid.x, N * N + wg.x, N, 1.0);
}

// Signed wavenumber index of bin `i` (0..N/2−1, then −N/2..−1).
fn wave_index(i: u32) -> f32 {
    return select(f32(i), f32(i) - f32(N), i >= N / 2u);
}

// The share of waves of wavenumber `k` this level holds (`Band::share`).
fn share(k: f32) -> f32 {
    if (params.band.y < 0.5) { return 1.0; }
    let s = saturate(log2(max(k, 1e-6) / params.band.x) + 0.5);
    let c = cos(0.5 * PI * s);
    return select(1.0 - c * c, c * c, params.band.y > 1.5);
}

@compute @workgroup_size(8, 8, 1)
fn evolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.y * N + id.x;
    let mirror = ((N - id.y) % N) * N + (N - id.x) % N;
    let z = work[i];
    let zm = conj(work[mirror]);
    var h = 0.5 * (z + zm);
    let d = 0.5 * (z - zm);
    var phi = vec2<f32>(d.y, -d.x);
    let k = vec2<f32>(wave_index(id.x), wave_index(id.y)) * (TAU / (f32(N) * params.grid.y));
    let kl = length(k);
    if (params.damp.w > 0.5) {
        let pz = work[2u * N * N + i];
        let pm = conj(work[2u * N * N + mirror]);
        let pd = 0.5 * (pz - pm);
        let w = share(kl);
        h += w * 0.5 * (pz + pm);
        phi += w * vec2<f32>(pd.y, -pd.x);
    }
    var out = vec2<f32>(0.0);
    if (kl > 1e-6) {
        let dt = params.grid.z;
        let a = kl * tanh(kl * params.grid.w);
        let b = G + params.damp.z * kl * kl;
        let w = sqrt(a * b);
        let c = cos(w * dt);
        let s = sin(w * dt);
        let fade = exp(-(params.damp.x + params.damp.y * kl * kl) * dt);
        let h2 = (h * c + phi * (a / w * s)) * fade;
        let phi2 = (phi * c - h * (b / w * s)) * fade;
        out = vec2<f32>(h2.x - phi2.y, h2.y + phi2.x);
    }
    work[N * N + i] = out;
}

fn height_at(x: i32, z: i32) -> f32 {
    let c = vec2<u32>(vec2<i32>(clamp(x, 0, i32(N) - 1), clamp(z, 0, i32(N) - 1)));
    let i = c.y * N + c.x;
    return select(0.0, work[N * N + i].x, unpack2x16float(medium[i].y).x > 0.0);
}

@compute @workgroup_size(8, 8, 1)
fn resolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.y * N + id.x;
    let norm = 1.0 / f32(N * N);
    // Dry land stays still: the transform smears a little of the waves onto it.
    let wet = unpack2x16float(medium[i].y).x;
    let s = select(vec2<f32>(0.0), work[N * N + i] * norm, wet > 0.0);
    surface[i] = s;
    heights[i] = s.x;
    let p = vec2<i32>(id.xy);
    let gx = (height_at(p.x + 1, p.y) - height_at(p.x - 1, p.y)) * norm * 0.5 / params.grid.y;
    let gz = (height_at(p.x, p.y + 1) - height_at(p.x, p.y - 1)) * norm * 0.5 / params.grid.y;
    textureStore(out_tex, p, vec4<f32>(s.x, gx, gz, 0.0));
}
