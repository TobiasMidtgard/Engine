// Generates the tileable 3D noise textures volumetric clouds are carved from (runs once).
//   shape  (128^3): r = Perlin-Worley (billowy base shape), g/b/a = Worley fBm at 3 frequencies
//   detail (32^3):  r/g/b = Worley fBm at 3 frequencies (erodes edges into wisps and curls)
// Standalone: does not use the engine shader library.

@group(0) @binding(0) var shape_out: texture_storage_3d<rgba8unorm, write>;
@group(0) @binding(1) var detail_out: texture_storage_3d<rgba8unorm, write>;

// pcg3d integer hash -> three floats in [0, 1).
fn hash33(p: vec3<u32>) -> vec3<f32> {
    var v = p * 1664525u + vec3<u32>(1013904223u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v = v ^ (v >> vec3<u32>(16u));
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return vec3<f32>(v) * (1.0 / 4294967296.0);
}

fn wrap_cell(c: vec3<f32>, period: f32) -> vec3<u32> {
    return vec3<u32>(c - period * floor(c / period));
}

// 1 - distance to the nearest feature point (cellular noise), tiling every `period` cells.
fn worley(p: vec3<f32>, period: f32) -> f32 {
    let q = p * period;
    let id = floor(q);
    let f = q - id;
    var d = 1e9;
    for (var z = -1; z <= 1; z++) {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let o = vec3<f32>(f32(x), f32(y), f32(z));
                let h = hash33(wrap_cell(id + o, period));
                let r = o + h - f;
                d = min(d, dot(r, r));
            }
        }
    }
    return 1.0 - min(sqrt(d), 1.0);
}

fn worley_fbm(p: vec3<f32>, period: f32) -> f32 {
    return worley(p, period) * 0.625 + worley(p, period * 2.0) * 0.25 + worley(p, period * 4.0) * 0.125;
}

fn gradient(c: vec3<f32>, period: f32) -> vec3<f32> {
    return normalize(hash33(wrap_cell(c, period) + vec3<u32>(77u, 191u, 23u)) * 2.0 - 1.0 + vec3(1e-4));
}

// Gradient (Perlin) noise in about -1..1, tiling every `period` cells.
fn perlin(p: vec3<f32>, period: f32) -> f32 {
    let q = p * period;
    let i = floor(q);
    let f = q - i;
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    var n: array<f32, 8>;
    for (var k = 0; k < 8; k++) {
        let o = vec3<f32>(f32(k & 1), f32((k >> 1u) & 1), f32((k >> 2u) & 1));
        n[k] = dot(gradient(i + o, period), f - o);
    }
    let x0 = mix(n[0], n[1], u.x);
    let x1 = mix(n[2], n[3], u.x);
    let x2 = mix(n[4], n[5], u.x);
    let x3 = mix(n[6], n[7], u.x);
    return mix(mix(x0, x1, u.y), mix(x2, x3, u.y), u.z) * 1.6;
}

fn perlin_fbm(p: vec3<f32>, period: f32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var per = period;
    for (var i = 0; i < 4; i++) {
        sum += amp * perlin(p, per);
        per *= 2.0;
        amp *= 0.5;
    }
    return sum;
}

fn remap(v: f32, l0: f32, h0: f32, l1: f32, h1: f32) -> f32 {
    return l1 + (v - l0) * (h1 - l1) / (h0 - l0);
}

@compute @workgroup_size(4, 4, 4)
fn gen_shape(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(shape_out);
    if (any(id >= size)) { return; }
    let p = (vec3<f32>(id) + 0.5) / vec3<f32>(size);
    let pn = clamp(perlin_fbm(p, 4.0) * 0.5 + 0.5, 0.0, 1.0);
    let w = worley_fbm(p, 4.0);
    // Perlin dilated by Worley: connected, billowy blobs.
    let pw = clamp(remap(pn, w - 1.0, 1.0, 0.0, 1.0), 0.0, 1.0);
    textureStore(shape_out, id, vec4<f32>(pw, w, worley_fbm(p, 8.0), worley_fbm(p, 16.0)));
}

@compute @workgroup_size(4, 4, 4)
fn gen_detail(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(detail_out);
    if (any(id >= size)) { return; }
    let p = (vec3<f32>(id) + 0.5) / vec3<f32>(size);
    textureStore(detail_out, id, vec4<f32>(worley_fbm(p, 2.0), worley_fbm(p, 4.0), worley_fbm(p, 8.0), 1.0));
}
