// Next mip of the fine-wave tiles (`detail.wgsl`): the mean of four texels. Slopes average
// to the slope a pixel that size sees; the moments average to what the variance needs.

@group(0) @binding(0) var src: texture_2d_array<f32>;
@group(0) @binding(1) var dst: texture_storage_2d_array<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn downsample(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dst);
    if (id.x >= size.x || id.y >= size.y) { return; }
    let p = vec2<i32>(id.xy) * 2;
    let l = i32(id.z);
    let s = textureLoad(src, p, l, 0) + textureLoad(src, p + vec2<i32>(1, 0), l, 0)
        + textureLoad(src, p + vec2<i32>(0, 1), l, 0) + textureLoad(src, p + vec2<i32>(1, 1), l, 0);
    textureStore(dst, vec2<i32>(id.xy), l, s * 0.25);
}
