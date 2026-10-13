// Mip chains of generated material maps (material_library.rs). Requires lib_types.
//
// Each level is the 2x2 box average of the one above. Base color is stored sRGB-encoded in an
// RGBA8 texture (viewed as sRGB when sampled), so it is averaged in linear light. Normal maps
// are averaged without renormalizing: the shorter the filtered normal, the more its texels
// disagreed, which the mesh shader turns into extra roughness (Toksvig), so bumpy surfaces don't
// sparkle in the distance.

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var dest: texture_storage_2d<rgba8unorm, write>;

fn box_sum(id: vec2<u32>) -> array<vec4<f32>, 4> {
    let src = textureDimensions(source) - 1u;
    let p = id * 2u;
    return array<vec4<f32>, 4>(
        textureLoad(source, min(p, src), 0),
        textureLoad(source, min(p + vec2<u32>(1u, 0u), src), 0),
        textureLoad(source, min(p + vec2<u32>(0u, 1u), src), 0),
        textureLoad(source, min(p + vec2<u32>(1u, 1u), src), 0),
    );
}

@compute @workgroup_size(8, 8)
fn cs_mip_linear(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dest);
    if (id.x >= size.x || id.y >= size.y) { return; }
    let s = box_sum(id.xy);
    textureStore(dest, id.xy, (s[0] + s[1] + s[2] + s[3]) * 0.25);
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = saturate3(c);
    return select(1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055, x * 12.92, x <= vec3<f32>(0.0031308));
}

@compute @workgroup_size(8, 8)
fn cs_mip_srgb(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dest);
    if (id.x >= size.x || id.y >= size.y) { return; }
    let s = box_sum(id.xy);
    let c = (to_linear(s[0].rgb) + to_linear(s[1].rgb) + to_linear(s[2].rgb) + to_linear(s[3].rgb)) * 0.25;
    let a = (s[0].a + s[1].a + s[2].a + s[3].a) * 0.25;
    textureStore(dest, id.xy, vec4<f32>(to_srgb(c), a));
}
