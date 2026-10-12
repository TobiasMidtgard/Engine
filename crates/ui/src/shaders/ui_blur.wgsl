// Frosted-glass backdrop: downsample the tonemapped scene and blur it (separable Gaussian).
// Composed after lib_types.wgsl.

struct BlurParams {
    // texel size of the source (1/width, 1/height), direction (x, y)
    texel_dir: vec4<f32>,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> params: BlurParams;

struct FsIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FsIn {
    let uv = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    var out: FsIn;
    out.pos = vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

// 4x4 box-ish downsample (bilinear taps at texel corners).
@fragment
fn fs_down(in: FsIn) -> @location(0) vec4<f32> {
    let t = params.texel_dir.xy;
    var c = vec3(0.0);
    c += textureSampleLevel(src, samp, in.uv + t * vec2(-1.0, -1.0), 0.0).rgb;
    c += textureSampleLevel(src, samp, in.uv + t * vec2(1.0, -1.0), 0.0).rgb;
    c += textureSampleLevel(src, samp, in.uv + t * vec2(-1.0, 1.0), 0.0).rgb;
    c += textureSampleLevel(src, samp, in.uv + t * vec2(1.0, 1.0), 0.0).rgb;
    return vec4(c * 0.25, 1.0);
}

// 9-tap Gaussian using linear sampling (5 fetches).
@fragment
fn fs_blur(in: FsIn) -> @location(0) vec4<f32> {
    let d = params.texel_dir.xy * params.texel_dir.zw;
    var c = textureSampleLevel(src, samp, in.uv, 0.0).rgb * 0.2270270270;
    c += textureSampleLevel(src, samp, in.uv + d * 1.3846153846, 0.0).rgb * 0.3162162162;
    c += textureSampleLevel(src, samp, in.uv - d * 1.3846153846, 0.0).rgb * 0.3162162162;
    c += textureSampleLevel(src, samp, in.uv + d * 3.2307692308, 0.0).rgb * 0.0702702703;
    c += textureSampleLevel(src, samp, in.uv - d * 3.2307692308, 0.0).rgb * 0.0702702703;
    return vec4(c, 1.0);
}
