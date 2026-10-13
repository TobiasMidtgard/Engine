// Physically based bloom (Jimenez 2014, "Next Generation Post Processing in Call of Duty"):
// a chain of 13-tap downsamples followed by additive 3x3 tent upsamples. No threshold: the
// whole image blooms a little, bright things (sun glints, spells, glowing mushrooms) a lot.
// Requires lib_types only.

struct BloomParams {
    // xy: 1 / source size, z: 1 on the first downsample (firefly suppression), w: upsample radius (texels)
    params: vec4<f32>,
};

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;
@group(0) @binding(2) var<uniform> bloom: BloomParams;

fn karis_weight(c: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + luminance(c));
}

// A NaN, infinite or negative pixel (a broken shader anywhere in the frame) would otherwise
// spread over the whole screen through the blur chain; such taps count as black. Checked on the
// bits, which no compiler optimizes away.
fn finite_or_black(c: vec3<f32>) -> vec3<f32> {
    let bad = (bitcast<vec3<u32>>(c) & vec3(0x7f800000u)) == vec3(0x7f800000u);
    return clamp(select(c, vec3(0.0), bad), vec3(0.0), vec3(60000.0));
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return finite_or_black(textureSampleLevel(src_tex, src_sampler, uv, 0.0).rgb);
}

@vertex
fn vs_bloom(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

@fragment
fn fs_down(in: FullscreenOut) -> @location(0) vec4<f32> {
    let t = bloom.params.xy;
    let uv = in.uv;
    let a = tap(uv + t * vec2(-2.0, -2.0));
    let b = tap(uv + t * vec2(0.0, -2.0));
    let c = tap(uv + t * vec2(2.0, -2.0));
    let d = tap(uv + t * vec2(-2.0, 0.0));
    let e = tap(uv);
    let f = tap(uv + t * vec2(2.0, 0.0));
    let g = tap(uv + t * vec2(-2.0, 2.0));
    let h = tap(uv + t * vec2(0.0, 2.0));
    let i = tap(uv + t * vec2(2.0, 2.0));
    let j = tap(uv + t * vec2(-1.0, -1.0));
    let k = tap(uv + t * vec2(1.0, -1.0));
    let l = tap(uv + t * vec2(-1.0, 1.0));
    let m = tap(uv + t * vec2(1.0, 1.0));
    if (bloom.params.z > 0.5) {
        // Karis average per 2x2 block keeps single bright pixels from flickering.
        let g0 = (a + b + d + e) * 0.25;
        let g1 = (b + c + e + f) * 0.25;
        let g2 = (d + e + g + h) * 0.25;
        let g3 = (e + f + h + i) * 0.25;
        let g4 = (j + k + l + m) * 0.25;
        let w0 = karis_weight(g0) * 0.125;
        let w1 = karis_weight(g1) * 0.125;
        let w2 = karis_weight(g2) * 0.125;
        let w3 = karis_weight(g3) * 0.125;
        let w4 = karis_weight(g4) * 0.5;
        let sum = g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4;
        return vec4<f32>(sum / max(w0 + w1 + w2 + w3 + w4, 1e-6), 1.0);
    }
    var o = e * 0.125;
    o += (a + c + g + i) * 0.03125;
    o += (b + d + f + h) * 0.0625;
    o += (j + k + l + m) * 0.125;
    return vec4<f32>(o, 1.0);
}

@fragment
fn fs_up(in: FullscreenOut) -> @location(0) vec4<f32> {
    let r = bloom.params.xy * bloom.params.w;
    let uv = in.uv;
    var o = tap(uv) * 4.0;
    o += (tap(uv + vec2(-r.x, 0.0)) + tap(uv + vec2(r.x, 0.0)) + tap(uv + vec2(0.0, -r.y)) + tap(uv + vec2(0.0, r.y))) * 2.0;
    o += tap(uv + vec2(-r.x, -r.y)) + tap(uv + vec2(r.x, -r.y)) + tap(uv + vec2(-r.x, r.y)) + tap(uv + vec2(r.x, r.y));
    return vec4<f32>(o / 16.0, 1.0);
}
