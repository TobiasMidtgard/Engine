// A traced mirror picture's depth (ray_mirror.rs), copied from the `R32Float` texture the trace
// writes into the depth texture its readers bind (depth formats can't be a compute shader's
// storage): one full-screen triangle, each fragment's depth its texel's.

@group(0) @binding(0) var depth_raw: texture_2d<f32>; // reverse-Z: the near plane over the view depth, 0 the sky

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_depth(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 {
    // (At most 1: what a texel's ray meets lies beyond the plane, past the near plane; 1 is a
    // texel whose ray starts inside something.)
    return clamp(textureLoad(depth_raw, vec2<i32>(p.xy), 0).x, 0.0, 1.0);
}
