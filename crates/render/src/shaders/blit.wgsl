// Final copy of the LDR image to the window surface. Requires lib_types only.

@group(0) @binding(0) var blit_src: texture_2d<f32>;
@group(0) @binding(1) var blit_sampler: sampler;

@vertex
fn vs_blit(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

@fragment
fn fs_blit(in: FullscreenOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(blit_src, blit_sampler, in.uv, 0.0);
}
