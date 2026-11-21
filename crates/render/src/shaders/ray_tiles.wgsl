// The ray tiles (ray_scene/tiles.rs): a material's base colour at 128 texels across, with every
// level down to one texel (its mean), in one layer of an RGBA8 array that hits read through an
// sRGB view (`ray_surface` in lib_ray.wgsl). One entry fills a level of a layer, a dispatch a
// level: level 0 samples the texture the material is drawn with, at its mip of about as many
// texels across `rect` as the level has; each level after it samples the level above through
// an sRGB view at the shared corner of each 2x2 block, which is the block's mean in linear
// light (an sRGB view decodes before it filters, as mipmaps.rs makes mips). Stored
// sRGB-encoded, alpha as it is: a compute shader cannot write an sRGB format.

struct TileFill {
    // the part of `fill_source` the layer shows, in its uv: corner (xy), size (zw)
    rect: vec4<f32>,
};

@group(0) @binding(0) var fill_source: texture_2d<f32>;
@group(0) @binding(1) var fill_sampler: sampler;
@group(0) @binding(2) var<uniform> fill: TileFill;
@group(0) @binding(3) var fill_out: texture_storage_2d<rgba8unorm, write>;

fn fill_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = saturate3(c);
    return select(1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055, x * 12.92, x <= vec3<f32>(0.0031308));
}

@compute @workgroup_size(8, 8)
fn cs_tile_fill(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(fill_out);
    if (id.x >= size.x || id.y >= size.y) { return; }
    // The source's texels across the rect against the level's: its mip with about as many
    // (for a level after the first, the view of the level above has that level only).
    let texels = fill.rect.zw * vec2<f32>(textureDimensions(fill_source, 0));
    let lod = max(log2(max(texels.x, texels.y) / f32(size.x)), 0.0);
    let uv = fill.rect.xy + (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size) * fill.rect.zw;
    let c = textureSampleLevel(fill_source, fill_sampler, uv, lod);
    textureStore(fill_out, id.xy, vec4<f32>(fill_to_srgb(c.rgb), c.a));
}
