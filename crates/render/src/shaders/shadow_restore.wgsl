// Puts a part of a shadow map back as its static image holds it (`ShadowTiles` in shadows.rs):
// one triangle over the map, cut by the scissor to the tiles the moving casters drew into the
// frame before. A depth texture can only be copied whole, so the part is drawn. Requires
// lib_types only.

@group(0) @binding(0) var restore_image: texture_depth_2d;

@vertex
fn vs_restore(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_vertex(i, 0.0).position;
}

@fragment
fn fs_restore(@builtin(position) at: vec4<f32>) -> @builtin(frag_depth) f32 {
    return textureLoad(restore_image, vec2<i32>(at.xy), 0);
}
