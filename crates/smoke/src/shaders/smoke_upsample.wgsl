// Lays the half-resolution smoke (smoke.wgsl with MARCH_SCALE 2) over the scene: one
// full-screen triangle, premultiplied. Where the four low pixels around a scene pixel saw
// about the scene's depth there, it blends them bilinearly; across an edge (one of them saw
// something more than a tenth nearer or farther) it takes the one whose depth matches best,
// so smoke behind a post doesn't bleed over it and the post doesn't leave a dark halo in the
// smoke (nearest-depth upsampling: Bavoil, "Fast rendering of opacity-mapped particles using
// DirectX 11 tessellation and mixed resolutions", NVIDIA 2012; GPU Gems 3 ch. 23,
// "High-speed, off-screen particles").

@group(1) @binding(0) var low: texture_2d<f32>;

struct UpOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_up(@builtin(vertex_index) i: u32) -> UpOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: UpOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

// Proportional to the distance along the view (reverse-Z, infinite far plane); sky is far.
fn far_of(d: f32) -> f32 {
    return 1.0 / max(d, 1e-7);
}

@fragment
fn fs_up(in: UpOut) -> @location(0) vec4<f32> {
    let last = vec2<i32>(textureDimensions(low)) - vec2<i32>(1);
    let lp = in.clip.xy * 0.5 - vec2<f32>(0.5);
    let base = vec2<i32>(floor(lp));
    let f = lp - floor(lp);
    var offsets = array<vec2<i32>, 4>(vec2(0, 0), vec2(1, 0), vec2(0, 1), vec2(1, 1));
    var weights = array<f32, 4>(
        (1.0 - f.x) * (1.0 - f.y), f.x * (1.0 - f.y), (1.0 - f.x) * f.y, f.x * f.y,
    );
    var cs: array<vec4<f32>, 4>;
    var most = 0.0;
    var blended = vec4<f32>(0.0);
    for (var k = 0; k < 4; k++) {
        cs[k] = textureLoad(low, clamp(base + offsets[k], vec2<i32>(0), last), 0);
        most = max(most, cs[k].a);
        blended += cs[k] * weights[k];
    }
    // Most of the screen has no smoke: done before reading any depth.
    if (most < 0.002) { discard; }
    let z = far_of(textureLoad(prepass_depth, vec2<i32>(in.clip.xy), 0));
    var best = vec4<f32>(0.0);
    var best_miss = 1e30;
    var edge = false;
    for (var k = 0; k < 4; k++) {
        let miss = abs(far_of(block_depth(clamp(base + offsets[k], vec2<i32>(0), last))) - z) / z;
        if (miss < best_miss) {
            best_miss = miss;
            best = cs[k];
        }
        edge = edge || miss > 0.1;
    }
    let c = select(blended, best, edge);
    if (c.a < 0.002) { discard; }
    return c;
}
