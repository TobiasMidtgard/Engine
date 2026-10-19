// Motion blur (motion_blur.rs). Requires lib_types only (no frame group).
//
// The reconstruction filter of McGuire, Hennessy, Bukowski and Osman 2012 ("A Reconstruction
// Filter for Plausible Motion Blur"), with Jimenez's 2014 refinements (samples along both the
// neighborhood's and the pixel's own velocity, per-pixel jitter), and the samples split into a
// foreground layer with a coverage alpha and the rest normalized, so the result is the physical
// box filter of the motion where nothing occludes and a moving object's smear fades exactly as
// long as it covers a pixel.
//
// velocity   full res: half the blur vector in pixels (the shutter is centered on the frame):
//            the prepass motion vector (moving objects) or the camera's motion and the depth
//            (everything else), scaled from this frame's motion to the exposure and clamped to
//            the largest radius; and the view depth
// tiles      16x16 px tiles: the longest velocity
// neighbors  tiles: the longest velocity that can reach each tile
// gather     full res: each sample counts where its own blur covers the pixel (when in front or
//            at the same depth) or the pixel's blur covers it (when behind: the pixel's own
//            surface moved off it), so moving silhouettes blur over a sharp background and a
//            sharp subject stays sharp over a streaking background

struct MotionBlurParams {
    // The depth buffer's (jittered) clip space -> camera-relative.
    inv_view_proj: mat4x4<f32>,
    // Camera-relative -> unjittered clip space, this frame and last frame (relative to last
    // frame's camera).
    view_proj: mat4x4<f32>,
    prev_view_proj: mat4x4<f32>,
    // Camera origin - last frame's origin (m), w unused.
    camera_delta: vec4<f32>,
    // x: blur scale (exposure / frame interval), y: largest blur radius (px), z: most samples,
    // w: near plane (m)
    params: vec4<f32>,
    // width, height, tile reach (tiles), frame index (noise)
    size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> mb: MotionBlurParams;
@group(0) @binding(1) var mb_linear: sampler;
@group(0) @binding(2) var mb_color: texture_2d<f32>;        // the image to blur
@group(0) @binding(3) var mb_data: texture_2d<f32>;         // velocity (xy) and view depth (z), or tiles
@group(0) @binding(4) var mb_neighbors: texture_2d<f32>;    // longest velocity around each tile
@group(0) @binding(5) var mb_depth: texture_depth_2d;       // final scene depth (reverse-Z)
@group(0) @binding(6) var mb_prepass_depth: texture_depth_2d;
@group(0) @binding(7) var mb_prepass_normal: texture_2d<f32>;
@group(0) @binding(8) var mb_motion: texture_2d<f32>;       // prepass motion (uv, current - previous)

// Pixels per tile side.
const MB_TILE: i32 = 16;
// Farthest view depth stored (m; half floats stop at 65504).
const MB_FAR: f32 = 60000.0;

@vertex
fn vs_motion_blur(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

fn mb_size() -> vec2<i32> {
    return vec2<i32>(mb.size.xy);
}

fn mb_sanitize(c: vec3<f32>) -> vec3<f32> {
    if (any(c != c)) {
        return vec3<f32>(0.0);
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(65000.0));
}

// Screen motion (uv, current - previous) of what pixel `px` shows, with reverse-Z depth `z`.
fn mb_pixel_motion(px: vec2<i32>, z: f32) -> vec2<f32> {
    // The prepass drew this surface: its motion vector (moving objects included).
    let zp = textureLoad(mb_prepass_depth, px, 0);
    if (zp > 0.0 && abs(z - zp) <= 1e-6 + 1e-4 * zp && textureLoad(mb_prepass_normal, px, 0).w > 0.5) {
        return textureLoad(mb_motion, px, 0).xy;
    }
    // Otherwise it is fixed in the world and only the camera moved (the sky: a direction).
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(mb_size());
    let h = mb.inv_view_proj * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, z, 1.0);
    let now = mb.view_proj * h;
    let prev = mb.prev_view_proj * vec4<f32>(h.xyz + mb.camera_delta.xyz * h.w, h.w);
    if (prev.w <= 1e-6 || now.w <= 1e-6) {
        return vec2<f32>(0.0);
    }
    return (now.xy / now.w - prev.xy / prev.w) * vec2<f32>(0.5, -0.5);
}

@fragment
fn fs_velocity(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
    let z = textureLoad(mb_depth, px, 0);
    var v = mb_pixel_motion(px, z) * mb.size.xy * (0.5 * mb.params.x);
    let len = length(v);
    if (len > mb.params.y) {
        v *= mb.params.y / len;
    }
    var depth = MB_FAR;
    if (z > mb.params.w / MB_FAR) {
        depth = mb.params.w / z;
    }
    return vec4<f32>(v, depth, 1.0);
}

@fragment
fn fs_tiles(in: FullscreenOut) -> @location(0) vec4<f32> {
    let t = vec2<i32>(floor(in.position.xy));
    let size = mb_size();
    var longest = vec2<f32>(0.0);
    var longest_sq = 0.0;
    for (var y = 0; y < MB_TILE; y++) {
        for (var x = 0; x < MB_TILE; x++) {
            let q = min(t * MB_TILE + vec2<i32>(x, y), size - 1);
            let v = textureLoad(mb_data, q, 0).xy;
            let l = dot(v, v);
            if (l > longest_sq) {
                longest_sq = l;
                longest = v;
            }
        }
    }
    return vec4<f32>(longest, 0.0, 1.0);
}

@fragment
fn fs_neighbors(in: FullscreenOut) -> @location(0) vec4<f32> {
    let t = vec2<i32>(floor(in.position.xy));
    let tiles = vec2<i32>(textureDimensions(mb_data));
    let reach = i32(mb.size.z);
    var longest = vec2<f32>(0.0);
    var longest_len = 0.0;
    for (var y = -reach; y <= reach; y++) {
        for (var x = -reach; x <= reach; x++) {
            let q = t + vec2<i32>(x, y);
            if (any(q < vec2<i32>(0)) || any(q >= tiles)) {
                continue;
            }
            let v = textureLoad(mb_data, q, 0).xy;
            let l = length(v);
            // The gap between the two tiles (pixels): farther tiles count only if they reach.
            let gap = max(abs(vec2<f32>(f32(x), f32(y))) - 1.0, vec2<f32>(0.0)) * f32(MB_TILE);
            if (l > longest_len && l >= length(gap)) {
                longest_len = l;
                longest = v;
            }
        }
    }
    return vec4<f32>(longest, 0.0, 1.0);
}

// 1 while `dist` is within `radius`, falling off over the last 5% (the edge of a blur segment).
fn mb_cylinder(dist: f32, radius: f32) -> f32 {
    return 1.0 - smoothstep(0.95 * radius, 1.05 * radius, dist);
}

// 1 when depth `za` is in front of or at `zb`, 0 once it is behind it by a few centimeters
// (more far away).
fn mb_not_behind(za: f32, zb: f32) -> f32 {
    let extent = 0.05 + 0.02 * min(za, zb);
    return saturate(1.0 - (za - zb) / extent);
}

// Interleaved gradient noise (Jimenez 2014), shifted every frame.
fn mb_noise(px: vec2<i32>) -> f32 {
    let p = vec2<f32>(px) + 5.588238 * f32(u32(mb.size.w) % 64u);
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

@fragment
fn fs_gather(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(floor(in.position.xy));
    let size = mb_size();
    let color = textureLoad(mb_color, px, 0);
    let vn = textureLoad(mb_neighbors, px / MB_TILE, 0).xy;
    let vn_len = length(vn);
    // Nothing around here moves by half a pixel: leave the image exactly as it is.
    if (vn_len < 0.5) {
        return color;
    }
    let center = textureLoad(mb_data, px, 0);
    let vc = center.xy;
    let vc_len = max(length(vc), 0.5);
    let zc = center.z;
    // Sample along the longest velocity around, and with half of the samples along the pixel's
    // own when it moves in another direction (so a slower background keeps its own streak).
    let own = length(vc) >= 0.5 && abs(dot(vc, vn)) < 0.9 * length(vc) * vn_len;
    let sets = select(1, 2, own);
    let count = clamp(i32(ceil(2.0 * vn_len / 1.5)), 6, max(i32(mb.params.z), 6));
    let per_set = max(count / sets, 3);
    let jitter = mb_noise(px) - 0.5;
    let inv_size = 1.0 / vec2<f32>(size);
    // The pixel itself: its own surface, one pixel of its blur segment.
    var rest = vec4<f32>(mb_sanitize(color.rgb), 1.0) * (0.5 / vc_len);
    var front = vec4<f32>(0.0);
    for (var s = 0; s < sets; s++) {
        let dir = select(vn, vc, s == 1);
        let dir_len = length(dir);
        // Pixels of the segment each sample stands for.
        let step = 2.0 * dir_len / f32(per_set);
        for (var i = 0; i < per_set; i++) {
            let t = (f32(i) + 0.5 + jitter) / f32(per_set) * 2.0 - 1.0;
            let offset = dir * t;
            let dist = abs(t) * dir_len;
            let pos = vec2<f32>(px) + 0.5 + offset;
            let q = clamp(vec2<i32>(floor(pos)), vec2<i32>(0), size - 1);
            let data = textureLoad(mb_data, q, 0);
            let vs_len = max(length(data.xy), 0.5);
            let in_front = mb_not_behind(data.z, zc);
            let behind = mb_not_behind(zc, data.z);
            let front_part = 1.0 - behind;
            let behind_part = 1.0 - in_front;
            let same_part = in_front + behind - 1.0;
            // The sample's blur over the pixel, and the pixel's blur over the sample (what the
            // pixel's surface uncovers as it moves), as a share of their segments.
            let spread = mb_cylinder(dist, vs_len) / (2.0 * vs_len);
            let uncovered = mb_cylinder(dist, vc_len) / (2.0 * vc_len);
            let c = mb_sanitize(textureSampleLevel(mb_color, mb_linear, pos * inv_size, 0.0).rgb);
            let wf = step * front_part * spread;
            let wr = step * (same_part * spread + behind_part * uncovered);
            front += vec4<f32>(c * wf, wf);
            rest += vec4<f32>(c * wr, wr);
        }
    }
    // The foreground covers the pixel for the share of the exposure its smears overlap it.
    let alpha = saturate(front.w / f32(sets));
    let front_rgb = front.rgb / max(front.w, 1e-6);
    let rest_rgb = rest.rgb / rest.w;
    return vec4<f32>(mix(rest_rgb, front_rgb, alpha), color.a);
}
