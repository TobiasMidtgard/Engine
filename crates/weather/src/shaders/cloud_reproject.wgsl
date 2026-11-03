// Temporal cloud reconstruction (Schneider 2015, Horizon Zero Dawn; Hillaire 2020, Unreal):
// each frame the march covers one pixel of every 2x2 block of the cloud image, in turn; the
// other three come from last frame's image, reprojected through the cloud's front depth and
// clamped to the fresh samples around them, so a moving cloud or a lightning flash can't
// leave a trail. Where last frame saw nothing (off screen, first frame, camera cut) the fresh
// samples are upsampled. Requires lib_types + lib_frame.

struct Reproject {
    offset: vec4<f32>,   // the block pixel marched this frame (x, y), block side, 1 = history usable
    size: vec4<f32>,     // cloud image size (w, h), march size (w, h)
    exposure: vec4<f32>, // this frame's pre-exposure over the history's
};

@group(1) @binding(0) var<uniform> rp: Reproject;
@group(1) @binding(1) var march_color: texture_2d<f32>;
@group(1) @binding(2) var march_depth: texture_2d<f32>;
@group(1) @binding(3) var march_sky: texture_2d<f32>;
@group(1) @binding(4) var history_color: texture_2d<f32>;
@group(1) @binding(5) var history_depth: texture_2d<f32>;
@group(1) @binding(6) var history_sky: texture_2d<f32>;

struct CloudImage {
    @location(0) color: vec4<f32>,
    @location(1) depth: vec4<f32>,
    @location(2) sky: vec4<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

fn camera_dir(uv: vec2<f32>) -> vec3<f32> {
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
    let p = frame.inv_view_proj * ndc;
    return normalize(p.xyz / p.w);
}

@fragment
fn fs_reproject(in: FullscreenOut) -> CloudImage {
    let q = floor(in.position.xy);
    let side = rp.offset.z;
    let msize = vec2<i32>(rp.size.zw);
    let b = vec2<i32>(floor(q / side));
    var out: CloudImage;
    if (all(q - vec2<f32>(b) * side == rp.offset.xy)) {
        let m = clamp(b, vec2<i32>(0), msize - 1);
        out.color = textureLoad(march_color, m, 0);
        out.depth = textureLoad(march_depth, m, 0);
        out.sky = textureLoad(march_sky, m, 0);
        return out;
    }
    // The four fresh samples around this pixel: upsampled, and their range.
    let muv = ((q - rp.offset.xy) / side + 0.5) / rp.size.zw;
    let up_color = textureSampleLevel(march_color, linear_clamp, muv, 0.0);
    let up_depth = textureSampleLevel(march_depth, linear_clamp, muv, 0.0);
    let up_sky = textureSampleLevel(march_sky, linear_clamp, muv, 0.0);
    if (rp.offset.w < 0.5 || frame.camera_delta.w < 0.5) {
        out.color = up_color;
        out.depth = up_depth;
        out.sky = up_sky;
        return out;
    }
    let near = vec2<i32>(floor((q - rp.offset.xy) / side));
    var lo_c = vec4(1e30);
    var hi_c = vec4(-1e30);
    var lo_d = vec4(1e30);
    var hi_d = vec4(-1e30);
    var lo_s = vec4(1e30);
    var hi_s = vec4(-1e30);
    for (var y = 0; y < 2; y++) {
        for (var x = 0; x < 2; x++) {
            let m = clamp(near + vec2<i32>(x, y), vec2<i32>(0), msize - 1);
            let c = textureLoad(march_color, m, 0);
            let d = textureLoad(march_depth, m, 0);
            let s = textureLoad(march_sky, m, 0);
            lo_c = min(lo_c, c);
            hi_c = max(hi_c, c);
            lo_d = min(lo_d, d);
            hi_d = max(hi_d, d);
            lo_s = min(lo_s, s);
            hi_s = max(hi_s, s);
        }
    }
    // Where this pixel's cloud was last frame: through its front, or at infinity in clear sky.
    let uv = (q + 0.5) / rp.size.xy;
    let dir = camera_dir(uv);
    var clip: vec4<f32>;
    if (up_color.a > 0.999) {
        clip = frame.prev_view_proj * vec4<f32>(dir, 0.0);
    } else {
        clip = frame.prev_view_proj * vec4<f32>(static_prev_rel(dir * up_depth.x * 1000.0), 1.0);
    }
    let prev = vec2<f32>(clip.x, -clip.y) / clip.w * 0.5 + 0.5;
    if (clip.w <= 0.0 || any(prev < vec2(0.0)) || any(prev > vec2(1.0))) {
        out.color = up_color;
        out.depth = up_depth;
        out.sky = up_sky;
        return out;
    }
    let ratio = vec4<f32>(vec3(rp.exposure.x), 1.0);
    // The range is widened a little: clamping to the exact box of four samples flickers.
    let widen = 0.1;
    let hc = textureSampleLevel(history_color, linear_clamp, prev, 0.0) * ratio;
    let hd = textureSampleLevel(history_depth, linear_clamp, prev, 0.0);
    let hs = textureSampleLevel(history_sky, linear_clamp, prev, 0.0) * ratio;
    out.color = clamp(hc, lo_c - (hi_c - lo_c) * widen, hi_c + (hi_c - lo_c) * widen);
    out.depth = clamp(hd, lo_d, hi_d);
    out.sky = clamp(hs, lo_s - (hi_s - lo_s) * widen, hi_s + (hi_s - lo_s) * widen);
    return out;
}
