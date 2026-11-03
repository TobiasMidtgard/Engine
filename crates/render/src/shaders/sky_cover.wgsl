// The sky occlusion map's cover pyramid (sky_occlusion.rs, frame binding 25): level L holds,
// for every 2^(L+1) x 2^(L+1) block of map texels, the highest surface (the smallest depth) in
// that block and the eight blocks around it. One load then tells whether anything within
// 2^(L+1) texels of a point stands above it, so `sky_map_visibility` and
// `specular_sky_visibility` skip the rings of taps that would find no cover (open ground,
// meadows, water: most of an outdoor view). Standalone (not composed with lib_frame).
//
// `cs_first` halves the map into `blocks` level 0, `cs_halve` halves blocks level L - 1 into
// level L, and `cs_widen` takes each block level's 3x3 minimum into the pyramid.

@group(0) @binding(0) var map: texture_depth_2d;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var dest: texture_storage_2d<r32float, write>;

fn texel_of(size: vec2<u32>, p: vec2<i32>) -> vec2<i32> {
    return min(p, vec2<i32>(size) - 1);
}

@compute @workgroup_size(8, 8, 1)
fn cs_first(@builtin(global_invocation_id) id: vec3<u32>) {
    let out = textureDimensions(dest);
    if (any(id.xy >= out)) { return; }
    let size = textureDimensions(map);
    let p = vec2<i32>(id.xy) * 2;
    let a = textureLoad(map, texel_of(size, p), 0);
    let b = textureLoad(map, texel_of(size, p + vec2<i32>(1, 0)), 0);
    let c = textureLoad(map, texel_of(size, p + vec2<i32>(0, 1)), 0);
    let d = textureLoad(map, texel_of(size, p + vec2<i32>(1, 1)), 0);
    textureStore(dest, vec2<i32>(id.xy), vec4<f32>(min(min(a, b), min(c, d)), 0.0, 0.0, 0.0));
}

@compute @workgroup_size(8, 8, 1)
fn cs_halve(@builtin(global_invocation_id) id: vec3<u32>) {
    let out = textureDimensions(dest);
    if (any(id.xy >= out)) { return; }
    let size = textureDimensions(source);
    let p = vec2<i32>(id.xy) * 2;
    let a = textureLoad(source, texel_of(size, p), 0).r;
    let b = textureLoad(source, texel_of(size, p + vec2<i32>(1, 0)), 0).r;
    let c = textureLoad(source, texel_of(size, p + vec2<i32>(0, 1)), 0).r;
    let d = textureLoad(source, texel_of(size, p + vec2<i32>(1, 1)), 0).r;
    textureStore(dest, vec2<i32>(id.xy), vec4<f32>(min(min(a, b), min(c, d)), 0.0, 0.0, 0.0));
}

@compute @workgroup_size(8, 8, 1)
fn cs_widen(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(source);
    if (any(id.xy >= size)) { return; }
    var m = 1.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let p = clamp(vec2<i32>(id.xy) + vec2<i32>(x, y), vec2<i32>(0), vec2<i32>(size) - 1);
            m = min(m, textureLoad(source, p, 0).r);
        }
    }
    textureStore(dest, vec2<i32>(id.xy), vec4<f32>(m, 0.0, 0.0, 0.0));
}
