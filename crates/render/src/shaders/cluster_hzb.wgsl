// Virtual geometry occlusion culling: a hierarchical depth buffer (HZB) built from the depth
// prepass (virtual_geometry/gpu.rs). Each texel of a level keeps the farthest depth (reverse-Z:
// the smallest value) of the texels it covers one level finer; level 0 covers 2x2 depth
// pixels. A level is half its finer level's size, rounded down, and the last texel of a row or
// column also covers the odd one out, so every pixel is covered at every level.

@group(0) @binding(0) var vg_hzb_out: texture_storage_2d<r32float, write>;
@group(0) @binding(1) var vg_hzb_depth: texture_depth_2d;
@group(0) @binding(2) var vg_hzb_in: texture_2d<f32>;

// Finer texels (first, last) that texel `x` of a level `out_size` wide covers.
fn vg_hzb_span(x: u32, out_size: u32, in_size: u32) -> vec2<u32> {
    let first = min(x * 2u, in_size - 1u);
    let last = select(min(first + 1u, in_size - 1u), in_size - 1u, x + 1u == out_size);
    return vec2<u32>(first, last);
}

@compute @workgroup_size(8, 8)
fn cs_vg_hzb_from_depth(@builtin(global_invocation_id) id: vec3<u32>) {
    let out_size = textureDimensions(vg_hzb_out);
    if (id.x >= out_size.x || id.y >= out_size.y) { return; }
    let in_size = textureDimensions(vg_hzb_depth);
    let sx = vg_hzb_span(id.x, out_size.x, in_size.x);
    let sy = vg_hzb_span(id.y, out_size.y, in_size.y);
    var d = 1.0;
    for (var y = sy.x; y <= sy.y; y++) {
        for (var x = sx.x; x <= sx.y; x++) {
            d = min(d, textureLoad(vg_hzb_depth, vec2<u32>(x, y), 0));
        }
    }
    textureStore(vg_hzb_out, id.xy, vec4<f32>(d, 0.0, 0.0, 0.0));
}

@compute @workgroup_size(8, 8)
fn cs_vg_hzb_reduce(@builtin(global_invocation_id) id: vec3<u32>) {
    let out_size = textureDimensions(vg_hzb_out);
    if (id.x >= out_size.x || id.y >= out_size.y) { return; }
    let in_size = textureDimensions(vg_hzb_in);
    let sx = vg_hzb_span(id.x, out_size.x, in_size.x);
    let sy = vg_hzb_span(id.y, out_size.y, in_size.y);
    var d = 1.0;
    for (var y = sy.x; y <= sy.y; y++) {
        for (var x = sx.x; x <= sx.y; x++) {
            d = min(d, textureLoad(vg_hzb_in, vec2<u32>(x, y), 0).x);
        }
    }
    textureStore(vg_hzb_out, id.xy, vec4<f32>(d, 0.0, 0.0, 0.0));
}
