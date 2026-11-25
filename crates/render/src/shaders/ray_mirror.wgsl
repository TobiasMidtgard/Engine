// The mirrors' pictures traced in the frame's ray pass (`trial_cut_more::RAY_MIRROR_PICTURES`,
// ray_mirror.rs; probe_gi/mod.rs `dispatch_mirror_pictures`), after every pixel's reflection.
// Composed after ray_reflect.wgsl, whose `reflect_at` each texel calls: a mirror's picture is
// what its view would draw from the mirrored eye through its off-axis frustum, and each texel's
// ray from that eye meets the plane at a point past which it is exactly the main view's ray
// through that point reflected in the plane. So the texel is that point's reflection at
// roughness 0: the screen first (last frame's picture, where the march meets what the camera
// saw), then a ray through the ray scene 384 m long, its hit lit as every pixel's reflection
// lights its hits (ray_hit_light.wgsl `reflected_hit_light`, or taken from last frame's picture
// where the camera sees it) and, where the ray lights it, fogged over its whole mirrored path
// (below), then, past it, what the screen shows farther along (hills beyond the ray's reach),
// then the sky: last frame's picture where the screen shows the sky in that direction, else the
// environment map. The depth is what the drawn view's would be: the near plane over the depth
// along the mirrored view, 0 for the sky.
// A texel whose ray starts inside something standing in the plane (a post, a hull) gets the
// near plane's depth, nearer than any point of the plane the picture holds, which the
// picture's readers drop as under the plane wherever in the texel they read it (at a grazing
// angle a texel spans metres of the plane), as they drop what a drawn view shows under it.
//
// A hit the rays light takes the fog and haze of its whole mirrored path, from the mirrored eye
// through the plane, as the drawn view fogs what it draws (ray_hit_light.wgsl
// `reflected_path_fog`, from the mirrored eye, where every pixel's reflection fogs its hits from
// the camera, as the screen's answers are: `reflected_camera_fog`); one the screen answers is
// last frame's picture, fogged as the camera saw it.
//
// Not in the picture: what lies past the ray's reach that the screen doesn't show, unless the
// picture has a far view (`trial_cut_more::RAY_MIRROR_FAR`): the mirror's view drawn through
// the same frustum, smaller, with its near plane a tenth short of where the shortest rays end.
// Past its own ray's reach a texel takes what the view drew there, laid by its coverage over
// what the screen shows along the ray or the sky (`mirror_far_view`: only what lies past where
// its ray begins to give way to the view, so the view adds no fringe round what the rays hit),
// and a hit in the last tenth of the reach blends into it by distance where it drew the same
// thing, so a ridge across the reach shows no line.

struct MirrorTrace {
    right: vec4<f32>,    // the mirrored view's right axis (camera-relative); w: its near plane (m)
    up: vec4<f32>,       // its up axis
    back: vec4<f32>,     // its back axis (it looks along -back)
    eye: vec4<f32>,      // the mirrored eye relative to the camera; w: a texel's angle (tangent span)
    tangents: vec4<f32>, // the picture's left, right, bottom and top edges as tangents (x / -z, y / -z)
    plane: vec4<f32>,    // the plane's unit normal (its reflecting side); w: dot(n, point on it), camera-relative
    far: vec4<f32>,      // the far view: x its exposure (0: none this frame), y its near plane (m),
                         // z how far along a texel's ray its hits begin to blend into it (m)
};
@group(2) @binding(26) var<uniform> mirror_trace: MirrorTrace;
// This frame's rays (the picture's history blends them into level 0 of its chain): rgb
// pre-exposed by the frame's exposure, a 1.
@group(2) @binding(27) var mirror_color: texture_storage_2d<rgba16float, write>;
// Reverse-Z depth: the near plane over the depth along the mirrored view, 0 the sky.
@group(2) @binding(28) var mirror_depth_raw: texture_storage_2d<r32float, write>;
// Counted while `COUNT_REFLECTIONS` is set (cleared before the pass): the texels, those the
// screen answered, the rays traced, and the rays' hits taken from last frame's picture.
@group(2) @binding(29) var<storage, read_write> mirror_counts: array<atomic<u32>, 4>;
// The far view's HDR colour (pre-exposed by `far.x`) and its reverse-Z depth (its near plane
// over the depth along the mirrored view, 0 where it drew the sky): a stand-in while it has none.
@group(2) @binding(31) var mirror_far_color: texture_2d<f32>;
@group(2) @binding(32) var mirror_far_depth: texture_depth_2d;

// How far past the ray's reach the screen is looked along for what lies there, from the plane
// (the water's own reach for a reflection, water.wgsl `mirror_reflection`).
const MIRROR_FAR_M: f32 = 4000.0;
// The ray's reach at roughness 0 (`reflect_at`).
const MIRROR_T_MAX: f32 = 384.0;

var<workgroup> mirror_group_counts: array<atomic<u32>, 4>;

// The sky along unit `dir` (cd/m^2, not pre-exposed): last frame's picture where the screen
// shows the sky in that direction, else the environment map, as the water's own sky term
// reflects it at roughness 0.
fn mirror_sky(dir: vec3<f32>) -> vec3<f32> {
    if (frame.reflections.y > 0.0) {
        let c = frame.unjittered_view_proj * vec4<f32>(dir, 0.0);
        let p = frame.prev_view_proj * vec4<f32>(dir, 0.0);
        if (c.w > 1e-4 && p.w > 1e-4) {
            let uv = vec2<f32>(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5);
            let prev_uv = vec2<f32>(p.x / p.w * 0.5 + 0.5, 0.5 - p.y / p.w * 0.5);
            if (all(uv > vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0)) && all(prev_uv > vec2<f32>(0.0))
                && all(prev_uv < vec2<f32>(1.0)) && mirror_sky_around(uv)) {
                return full_seen_at(prev_uv);
            }
        }
    }
    return sample_env(dir);
}

// Whether the screen shows the sky at the pixel `uv` lies in and the 8 around it: every pixel
// the picture's filtered read there blends, and a pixel more for the edges temporal
// anti-aliasing blends. Where a direction's vanishing point lay just off a thing's outline, the
// read blended the thing in: a lake's far edge under a ball peeking in at the top of the
// screen came out dimmed in dashes.
fn mirror_sky_around(uv: vec2<f32>) -> bool {
    let last = vec2<i32>(textureDimensions(full_depth)) - 1;
    let px = vec2<i32>(floor(uv * frame.viewport.xy));
    for (var i = 0; i < 9; i++) {
        let at = clamp(px + vec2<i32>(i % 3 - 1, i / 3 - 1), vec2<i32>(0), last);
        if (textureLoad(full_depth, at, 0) > 0.0) { return false; }
    }
    return true;
}

// What lies along unit `dir` from camera-relative `origin` past the ray's reach that the
// screen shows (a hill beyond 384 m), from last frame's picture: rgb and its distance from
// `origin`; else the sky, at distance 0.
fn mirror_far(origin: vec3<f32>, dir: vec3<f32>, noise: f32) -> vec4<f32> {
    if (frame.reflections.y > 0.0) {
        let start = origin + dir * MIRROR_T_MAX;
        let m = full_march((frame.view * vec4<f32>(start, 0.0)).xyz,
            normalize((frame.view * vec4<f32>(dir, 0.0)).xyz), MIRROR_FAR_M - MIRROR_T_MAX, noise);
        if (m.found) {
            let size = vec2<i32>(textureDimensions(full_depth));
            let px = clamp(vec2<i32>(floor(m.uv * frame.viewport.xy)), vec2<i32>(0), size - 1);
            let prev_uv = m.uv - textureLoad(full_motion, px, 0).xy;
            if (dot(oct_decode(textureLoad(full_normal, px, 0).xy), dir) < -0.1
                && all(prev_uv >= vec2<f32>(0.0)) && all(prev_uv <= vec2<f32>(1.0))) {
                let hit = (frame.inv_view * vec4<f32>(view_position(m.uv, textureLoad(full_depth, px, 0)), 0.0)).xyz;
                return vec4<f32>(full_seen_at(prev_uv), max(distance(hit, origin), 1e-3));
            }
        }
    }
    return vec4<f32>(mirror_sky(dir), 0.0);
}

// What the far view drew at the picture's `uv`, bilinear over the four texels around it, with
// only those counted that drew something (the view's own sky, fogged as its sky pass fogs it,
// would ring a ridge's outline otherwise) as deep along the mirrored view as `lo` to `hi` (m),
// each by how far it lies past `lo` toward `full`, smoothly (all alike where `full` is no
// deeper than `lo`): its colour (cd/m^2), how much of the read it covers (the weight of the
// texels counted over theirs and the sky's: a ridge's outline is partly covered), and the
// depth of the nearest of them. A texel's own read keeps to what lies past where its own ray
// gives way to the view: the view is drawn from one near plane for the whole picture, short of
// every texel's reach, so it also holds what the rays hit, which a neighbour's read would
// fringe at a quarter of the picture's size, and what stands under the plane in features that
// don't cut there per pixel. What such a texel hides is unknown rather than open, so a nearer
// thing's outline leaves no hole in a ridge behind it either. Nothing covered without a far
// view.
struct FarView {
    rgb: vec3<f32>,
    cover: f32,
    depth: f32,
};

fn mirror_far_view(uv: vec2<f32>, lo: f32, full: f32, hi: f32) -> FarView {
    var out: FarView;
    let far = mirror_trace.far;
    if (far.x <= 0.0) { return out; }
    let size = vec2<i32>(textureDimensions(mirror_far_depth));
    let p = uv * vec2<f32>(size) - 0.5;
    let base = vec2<i32>(floor(p));
    let f = p - floor(p);
    var sum = vec3<f32>(0.0);
    var taken = 0.0;
    var open = 0.0;
    var nearest = 0.0;
    for (var i = 0; i < 4; i++) {
        let o = vec2<i32>(i & 1, i >> 1u);
        let at = clamp(base + o, vec2<i32>(0), size - 1);
        var w = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        let z = textureLoad(mirror_far_depth, at, 0);
        if (z <= 0.0) {
            open += w;
            continue;
        }
        let deep = far.y / z;
        if (deep < lo || deep > hi) { continue; }
        if (full > lo) { w *= smoothstep(lo, full, deep); }
        if (w <= 0.0) { continue; }
        sum += textureLoad(mirror_far_color, at, 0).rgb * w;
        taken += w;
        nearest = max(nearest, z);
    }
    if (taken <= 0.0) { return out; }
    out.cover = taken / (taken + open);
    // (Rounding would march the screen for a two-millionth of a whole read.)
    if (out.cover > 0.9999) { out.cover = 1.0; }
    out.rgb = min(sum / (taken * far.x), vec3<f32>(60000.0));
    out.depth = far.y / nearest;
    return out;
}

// Texel `id` of a `size` picture, written to `mirror_color` and `mirror_depth_raw`, and what it
// adds to the counts (the texel, answered by the screen, a ray, a ray's hit from the picture).
fn mirror_texel(id: vec2<u32>, size: vec2<u32>) -> vec4<u32> {
    let m = mirror_trace;
    // With temporal anti-aliasing each frame's ray goes through another point of the texel:
    // the frame's jitter, its axes swapped so that it doesn't move with the pixels' own. The
    // picture's own history (mirror_resolve.wgsl) then gathers the texel's whole area, as a
    // drawn view's own history does (one ray at the texel's centre left the outlines of what it
    // reflects ragged). Without it, the centre (`frame.jitter` is 0).
    let uv = (vec2<f32>(id) + 0.5 + frame.jitter.wz) / vec2<f32>(size);
    // The texel's ray from the mirrored eye, 1 along the mirrored view's axis: `u` along it is
    // a point's depth in that view.
    let d = m.right.xyz * mix(m.tangents.x, m.tangents.y, uv.x)
        + m.up.xyz * mix(m.tangents.w, m.tangents.z, uv.y) - m.back.xyz;
    let n = m.plane.xyz;
    let near = m.right.w;
    let dn = dot(d, n);
    var counts = vec4<u32>(1u, 0u, 0u, 0u);
    var rgb = vec3<f32>(0.0);
    var depth = 0.0;
    if (dn <= 1e-6) {
        // It never meets the plane (the picture's margin over the horizon): the sky, far, as
        // the plane reflects it there.
        rgb = mirror_sky(normalize(d - 2.0 * dn * n));
    } else {
        // Where it meets the plane, and the main view's ray through there, reflected: no lift
        // off the plane's face (the reflected rays of a lake far off leave it at a degree or
        // two), only off the plane itself.
        let u = (m.plane.w - dot(m.eye.xyz, n)) / dn;
        let s = m.eye.xyz + d * u;
        let len = length(d);
        let dir = d / len;
        let origin = s + n * max(0.005, 1e-4 * length(s));
        let noise = interleaved_gradient_noise(vec2<f32>(id) + frame.time.w * 5.588238);
        // (Its cone: a texel across at the plane, widening by a texel's angle.)
        // (Its hits the rays light are fogged over the whole mirrored path, from the mirrored
        // eye.)
        let found = reflect_at(origin, dir, max(view_depth(s), 0.0), 0.0,
            vec2<f32>(m.eye.w * u * len, m.eye.w), noise, vec4<f32>(m.eye.xyz, 1.0));
        counts = vec4<u32>(1u, found.counts.y, found.counts.z, found.counts.w);
        if (found.inside) {
            // Inside something standing in the plane: at the near plane, which readers drop for
            // every point of the plane in the texel (`mirror_reflection`: what the picture holds
            // must lie beyond the point it shades), in the sky's colour, as a drawn view sees
            // through the thing's culled inside (the picture's levels blend it in).
            rgb = mirror_sky(dir);
            depth = 1.0;
        } else if (found.sky || found.out.w <= 0.0) {
            // Past the rays' reach: what the far view drew there, where the picture has one,
            // over what the screen shows farther along the ray or the sky, by its coverage (the
            // screen isn't looked along where the view covers the whole read). Only what lies
            // past where this texel's own ray begins to give way to the view, fully from where
            // it ends, as a hit there blends: anything nearer along it was its ray's to hit.
            let view = mirror_far_view(uv, u + m.far.z / len, u + MIRROR_T_MAX / len, 3.0e38);
            var beyond = vec4<f32>(0.0);
            if (view.cover < 1.0) {
                beyond = mirror_far(origin, dir, noise);
            }
            rgb = beyond.rgb;
            depth = select(0.0, near / (u + beyond.w / len), beyond.w > 0.0);
            if (view.cover > 0.0) {
                rgb = mix(rgb, view.rgb, view.cover);
                depth = max(depth, near / max(view.depth, u));
            }
        } else {
            let t = found.out.w;
            rgb = found.out.rgb;
            depth = near / (u + t / len);
            // In the last tenth of the rays' reach the hit gives way by its distance to what
            // the far view drew there, where that is the same thing (as deep, within a tenth):
            // a ridge across the reach shows no line where the rays end.
            if (m.far.x > 0.0 && t > m.far.z) {
                let at = u + t / len;
                let view = mirror_far_view(uv, max(0.9 * at, u), 0.0, 1.1 * at);
                if (view.cover > 0.0) {
                    rgb = mix(rgb, view.rgb, smoothstep(m.far.z, MIRROR_T_MAX, t) * view.cover);
                }
            }
        }
    }
    textureStore(mirror_color, id, vec4<f32>(pre_expose(min(rgb, vec3<f32>(60000.0))), 1.0));
    textureStore(mirror_depth_raw, id, vec4<f32>(depth, 0.0, 0.0, 0.0));
    return counts;
}

@compute @workgroup_size(8, 8, 1)
fn cs_mirror_picture(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) thread: u32) {
    let counting = shade_skip(COUNT_REFLECTIONS);
    if (thread < 4u) { atomicStore(&mirror_group_counts[thread], 0u); }
    workgroupBarrier();
    var counts = vec4<u32>(0u);
    let size = textureDimensions(mirror_color);
    if (all(id.xy < size)) {
        counts = mirror_texel(id.xy, size);
    }
    if (counting) {
        if (counts.x > 0u) { atomicAdd(&mirror_group_counts[0], counts.x); }
        if (counts.y > 0u) { atomicAdd(&mirror_group_counts[1], counts.y); }
        if (counts.z > 0u) { atomicAdd(&mirror_group_counts[2], counts.z); }
        if (counts.w > 0u) { atomicAdd(&mirror_group_counts[3], counts.w); }
    }
    workgroupBarrier();
    if (counting && thread < 4u) {
        let n = atomicLoad(&mirror_group_counts[thread]);
        if (n > 0u) { atomicAdd(&mirror_counts[thread], n); }
    }
}
