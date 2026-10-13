// Mip chain of the previous frame's image for glossy screen-space reflections: level 0 is half
// the screen's resolution, each level below halves the one above. Requires lib_types only.
//
// Every texel takes four bilinear taps 0.75 source texels off its centre, which weighs the 4x4
// source texels around it by the binomial [1 3 3 1] per axis. Chained, the levels blur like a
// Gaussian about 2^level screen pixels wide, so the reflection gather can read the level whose
// blur closes the gaps between its taps over a wide (rough, distant) reflection footprint.

@group(0) @binding(0) var chain_source: texture_2d<f32>;
@group(0) @binding(1) var chain_sampler: sampler;

// A NaN, infinite or negative pixel would otherwise spread over every rough reflection that
// gathers near it; such taps count as black. Checked on the bits, which no compiler optimizes
// away.
fn chain_finite(c: vec3<f32>) -> vec3<f32> {
    let bad = (bitcast<vec3<u32>>(c) & vec3(0x7f800000u)) == vec3(0x7f800000u);
    return clamp(select(c, vec3(0.0), bad), vec3(0.0), vec3(60000.0));
}

fn chain_tap(uv: vec2<f32>) -> vec3<f32> {
    return chain_finite(textureSampleLevel(chain_source, chain_sampler, uv, 0.0).rgb);
}

@vertex
fn vs_chain(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

@fragment
fn fs_chain(in: FullscreenOut) -> @location(0) vec4<f32> {
    let t = 0.75 / vec2<f32>(textureDimensions(chain_source));
    let c = chain_tap(in.uv + vec2(-t.x, -t.y)) + chain_tap(in.uv + vec2(t.x, -t.y))
        + chain_tap(in.uv + vec2(-t.x, t.y)) + chain_tap(in.uv + vec2(t.x, t.y));
    return vec4<f32>(c * 0.25, 1.0);
}
