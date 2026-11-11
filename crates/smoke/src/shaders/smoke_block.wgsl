// The depth (reverse-Z) one pixel of the half-resolution smoke stands for: the nearest of the
// 2x2 scene pixels under it on "black" squares of a checkerboard, the farthest on "white"
// ones. Shared by the march (smoke.wgsl) and the upsample (smoke_upsample.wgsl), so both
// agree on what each low pixel saw.
fn block_depth(low: vec2<i32>) -> f32 {
    let last = vec2<i32>(textureDimensions(prepass_depth)) - vec2<i32>(1);
    let base = low * 2;
    let a = textureLoad(prepass_depth, min(base, last), 0);
    let b = textureLoad(prepass_depth, min(base + vec2<i32>(1, 0), last), 0);
    let c = textureLoad(prepass_depth, min(base + vec2<i32>(0, 1), last), 0);
    let d = textureLoad(prepass_depth, min(base + vec2<i32>(1, 1), last), 0);
    if (((low.x + low.y) & 1) == 0) {
        return max(max(a, b), max(c, d));
    }
    return min(min(a, b), min(c, d));
}

