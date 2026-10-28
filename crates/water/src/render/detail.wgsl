// ============================================================================
// Fine wind waves (`crate::detail`): two periodic tiles evolved by FFT each frame (Tessendorf
// 2001). `evolve` turns the spectrum amplitudes h0 into this instant's spectra of the slopes,
// the chop's derivatives and the height; `fft_rows` and `fft_cols` transform them back to
// space; `resolve` writes the texture the water shader samples: per tile a layer of
// (dh/dx, dh/dz, height, Jacobian) and a layer of slope moments (dh/dx², dh/dz², dh/dx·dh/dz)
// plus lingering foam, whose mips (`detail_mips.wgsl`) give the slope variance a pixel can't
// resolve (LEAN mapping).
// ============================================================================

struct Params {
    sizes: vec4<f32>, // tile sizes (m) x, y; time (s, wrapped at PERIOD); foam kept per frame
    chop: vec4<f32>,  // horizontal chop λ, Jacobian under which the surface foams, unused, unused
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> spectrum: array<vec4<f32>>;   // h0(k), conj(h0(−k))
@group(0) @binding(2) var<storage, read_write> work: array<vec2<f32>>; // tile, field, z, x
@group(0) @binding(3) var<storage, read_write> foam: array<f32>;       // tile, z, x
@group(0) @binding(4) var out_tex: texture_storage_2d_array<rgba16float, write>;

const N: u32 = 128u;
const LOG_N: u32 = 7u;
const G: f32 = 9.81;
const TENSION: f32 = 7.4e-5;
const PERIOD: f32 = 64.0;

fn cmul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

fn tile_size(c: u32) -> f32 {
    return select(params.sizes.y, params.sizes.x, c == 0u);
}

// Three complex fields, each holding two real ones (both have Hermitian spectra, so one
// inverse transform of A + iB gives a + ib):
//   0: dh/dx + i dh/dz      1: dDx/dx + i dDz/dz      2: dDx/dz + i h
// with the chop D = iλ(k/|k|)h, which pulls water toward the crests (sharp crests, flat troughs).
@compute @workgroup_size(8, 8, 1)
fn evolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = id.z;
    let x = id.x;
    let z = id.y;
    let k = (vec2<f32>(f32(x), f32(z)) - f32(N / 2u)) * (TAU / tile_size(c));
    let kl = length(k);
    let h0 = spectrum[(c * N + z) * N + x];
    var a = vec2<f32>(0.0);
    var b = vec2<f32>(0.0);
    var d = vec2<f32>(0.0);
    if (kl > 1e-6) {
        // Frequencies on whole multiples of 2π/PERIOD, so the tiles repeat as the clock wraps.
        let w0 = TAU / PERIOD;
        let w = floor(sqrt(G * kl + TENSION * kl * kl * kl) / w0) * w0;
        let ph = w * params.sizes.z;
        let e = vec2<f32>(cos(ph), sin(ph));
        let h = cmul(h0.xy, e) + cmul(h0.zw, vec2<f32>(e.x, -e.y));
        let ih = vec2<f32>(-h.y, h.x);
        let lambda = params.chop.x;
        a = k.x * ih - k.y * h;
        b = -lambda / kl * (k.x * k.x * h + k.y * k.y * ih);
        d = -lambda * k.x * k.y / kl * h + ih;
    }
    let at = c * 3u * N * N + z * N + x;
    work[at] = a;
    work[at + N * N] = b;
    work[at + 2u * N * N] = d;
}

var<workgroup> line: array<vec2<f32>, 128>;

// Inverse FFT of `line` (loaded in bit-reversed order), in place: 64 threads, one butterfly
// each per stage.
fn transform_line(t: u32) {
    for (var s = 0u; s < LOG_N; s++) {
        let half = 1u << s;
        let pos = t & (half - 1u);
        let i = ((t >> s) << (s + 1u)) + pos;
        let j = i + half;
        let ang = TAU * f32(pos) / f32(2u * half);
        let bw = cmul(line[j], vec2<f32>(cos(ang), sin(ang)));
        let a = line[i];
        line[i] = a + bw;
        line[j] = a - bw;
        workgroupBarrier();
    }
}

fn fft(t: u32, base: u32, stride: u32) {
    for (var r = 0u; r < 2u; r++) {
        let i = t + r * 64u;
        line[reverseBits(i) >> (32u - LOG_N)] = work[base + i * stride];
    }
    workgroupBarrier();
    transform_line(t);
    for (var r = 0u; r < 2u; r++) {
        let i = t + r * 64u;
        work[base + i * stride] = line[i];
    }
}

// Workgroup (row, field, tile).
@compute @workgroup_size(64, 1, 1)
fn fft_rows(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wg: vec3<u32>) {
    fft(lid.x, (wg.z * 3u + wg.y) * N * N + wg.x * N, 1u);
}

// Workgroup (column, field, tile).
@compute @workgroup_size(64, 1, 1)
fn fft_cols(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wg: vec3<u32>) {
    fft(lid.x, (wg.z * 3u + wg.y) * N * N + wg.x, N);
}

@compute @workgroup_size(8, 8, 1)
fn resolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = id.z;
    let x = id.x;
    let z = id.y;
    let at = c * 3u * N * N + z * N + x;
    // The bins run from −N/2: every other texel flips sign.
    let sign = select(1.0, -1.0, ((x + z) & 1u) == 1u);
    let a = work[at] * sign;
    let b = work[at + N * N] * sign;
    let d = work[at + 2u * N * N] * sign;
    let dxx = b.x;
    let dzz = b.y;
    let dxz = d.x;
    let jac = (1.0 + dxx) * (1.0 + dzz) - dxz * dxz;
    // The slope where the chopped surface actually is: the height's gradient over the
    // displaced positions.
    let inv = 1.0 / max(jac, 0.1);
    let gx = (a.x * (1.0 + dzz) - a.y * dxz) * inv;
    let gz = (a.y * (1.0 + dxx) - a.x * dxz) * inv;
    // Where the surface squeezes, it breaks white; the foam lingers and thins.
    let fi = c * N * N + z * N + x;
    let f = max(foam[fi] * params.sizes.w, saturate((params.chop.y - jac) * 4.0));
    foam[fi] = f;
    let p = vec2<i32>(i32(x), i32(z));
    textureStore(out_tex, p, i32(2u * c), vec4<f32>(gx, gz, d.y, jac));
    textureStore(out_tex, p, i32(2u * c + 1u), vec4<f32>(gx * gx, gz * gz, gx * gz, f));
}
