// Strands into the scene (strands.rs): the rasterized hair over the HDR scene in the transparent
// phase, premultiplied, one quad per tile the raster touched; and its motion over the prepass's
// where it covers most of a pixel, before the scene, so temporal anti-aliasing follows the hair
// rather than what is behind it. Requires lib_types + lib_frame + strands_common.

@group(1) @binding(0) var<uniform> params: StrandParams;
@group(1) @binding(1) var<storage, read> tiles: array<u32>;
@group(1) @binding(2) var hair_color: texture_2d<f32>;
@group(1) @binding(3) var hair_motion: texture_2d<f32>;

@vertex
fn vs_tile(@builtin(vertex_index) v: u32, @builtin(instance_index) k: u32) -> @builtin(position) vec4<f32> {
    let tile = tiles[params.tiles.w + k];
    let corner = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
        vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0),
    )[v];
    let px = (vec2<f32>(f32(tile % params.size.x), f32(tile / params.size.x)) + corner) * f32(STRAND_TILE);
    let uv = min(px / frame.viewport.xy, vec2<f32>(1.0));
    return vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(hair_color, vec2<i32>(pos.xy), 0);
    if (c.a <= 0.0) {
        discard;
    }
    return c;
}

@fragment
fn fs_motion(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let m = textureLoad(hair_motion, vec2<i32>(pos.xy), 0);
    if (m.w < 0.5) {
        discard;
    }
    return vec4<f32>(m.xy, 0.0, 0.0);
}
