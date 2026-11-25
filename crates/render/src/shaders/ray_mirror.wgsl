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
// then the sky: last frame's picture where the screen shows the sky in that direction, else
// what the picture's far view (below) drew as its sky there, else the environment map, and
// never the disc of the sun (or of the moon when it is the light) from the screen or the view:
// the water's glitter (water.wgsl `shade_water`) is the sun's reflection, spread by the
// surface's roughness, shadowed and dimmed through the air as the light is, so a disc in the
// picture would be the sun a second time, sharp where the glitter is spread. Within a cone
// round the light (`mirror_at_sun`: three times its radius, wider for the far view's texels)
// the picture takes the sky just past that cone (`mirror_off_sun`), and holds every hit there,
// and what the screen or the view drew there, no brighter than half again the environment
// map's sky (`mirror_sun_cap`: their anti-aliasing blends the disc into the outline of a ridge
// it sets behind). The depth is what the drawn view's would be: the near plane over the depth
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
// thing, so a ridge across the reach shows no line. Where the screen doesn't show the sky that
// way, the sky is the one the view drew there, its clouds and fog as a drawn view's (the sky
// pass's and the weather's), but within the cone round the light: the environment map is a 256
// by 128 picture of the sky (1.4 degrees a texel; the weather also marches its clouds into it
// more cheaply), unfogged, which lost the small clouds (`mirror_far`).

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

// The light's radius the picture keeps its disc out by (radians): the frame's light's, but no
// less than the weather's sun, which it draws 0.0047 in radius whatever the light's size (and
// at least 0.004 when it isn't in charge of the lights).
fn mirror_sun_radius() -> f32 {
    return max(frame.sun_dir.w, 0.0047);
}

// Whether unit `dir` lies within three times the light's radius of the light (the sun, or the
// moon when it is the light), and `widen` (radians) more: where the screen and the far view
// may draw its disc, filtered a little wider. The weather draws the moon at twice its size (a
// moon scaled past three times would show its rim round the cone). The picture takes no sky
// there from either, but the sky just past that cone (`mirror_off_sun`). Only the light's: the
// water's glitter stands for that one alone, and a moon in a daytime sky stays in the picture.
fn mirror_at_sun(dir: vec3<f32>, widen: f32) -> bool {
    return dot(dir, frame.sun_dir.xyz) > cos(3.0 * mirror_sun_radius() + widen);
}

// Unit `dir` turned away from the light, along the great circle through both, to `margin`
// (radians) past the edge of `mirror_at_sun`'s cone: where the picture takes the sky for a
// direction inside it. So the cone holds the sky round the disc, from wherever the sky round
// it comes from, drawn out toward the light (it is a degree or two across), and not a hole of
// the environment map's sky, which has neither the fog the sky passes lay over the sky nor
// what they draw round the sun. It stays `margin`, and at least half a degree, on the plane's
// reflecting side: under a light low over the horizon the way straight off it from a
// direction under it leads under the plane, where the screen, the far view and the
// environment map show the ground. There it turns round the light toward its side above the
// plane, no further than it must (`up`: off the light toward the plane's side; `side`:
// across), to the sky beside the disc.
fn mirror_off_sun(dir: vec3<f32>, margin: f32) -> vec3<f32> {
    let s = frame.sun_dir.xyz;
    var t = dir - s * dot(dir, s);
    if (dot(t, t) < 1e-12) {
        // Straight at the light: any way off it.
        t = cross(s, select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(s.y) > 0.9));
    }
    t = normalize(t);
    let a = 3.0 * mirror_sun_radius() + margin;
    let n = mirror_trace.plane.xyz;
    let up = n - s * dot(s, n);
    // How far along the plane's normal `t` must reach, times sin(a), for the turned direction
    // to lie that far above the plane.
    let lift = sin(max(margin, 0.0087)) - cos(a) * dot(s, n);
    if (dot(t, n) * sin(a) < lift && dot(up, up) > 1e-8) {
        let u = normalize(up);
        let side = cross(s, u);
        // The nearest way round the light that lifts it enough (straight up off the light
        // where none does: the light too far under the plane for the cone to reach above it).
        let c = clamp(lift / (sin(a) * length(up)), -1.0, 1.0);
        let k = sqrt(1.0 - c * c);
        t = u * c + side * select(-k, k, dot(t, side) >= 0.0);
    }
    return s * cos(a) + t * sin(a);
}

// The brightest the picture holds within `mirror_at_sun`'s cone of what the screen or the far
// view drew in front of the light (cd/m^2): their anti-aliasing blends the disc into the
// outline of a ridge or a tree it sets behind, and a sliver of the disc is far brighter than
// the sky. Half as bright again as the environment map's sky there, which has no disc: what
// stands in front of the light, lit from behind, is darker than the sky round it.
fn mirror_sun_cap(dir: vec3<f32>) -> vec3<f32> {
    return sample_env(dir) * 1.5;
}

// The sky along unit `dir` (cd/m^2, not pre-exposed) from last frame's picture, where the
// screen shows the sky in that direction (w 1; else w 0), but within `mirror_at_sun`'s cone.
fn mirror_screen_sky(dir: vec3<f32>) -> vec4<f32> {
    if (frame.reflections.y > 0.0 && !mirror_at_sun(dir, 0.0)) {
        let c = frame.unjittered_view_proj * vec4<f32>(dir, 0.0);
        let p = frame.prev_view_proj * vec4<f32>(dir, 0.0);
        if (c.w > 1e-4 && p.w > 1e-4) {
            let uv = vec2<f32>(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5);
            let prev_uv = vec2<f32>(p.x / p.w * 0.5 + 0.5, 0.5 - p.y / p.w * 0.5);
            if (all(uv > vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0)) && all(prev_uv > vec2<f32>(0.0))
                && all(prev_uv < vec2<f32>(1.0)) && mirror_sky_around(uv)) {
                return vec4<f32>(full_seen_at(prev_uv), 1.0);
            }
        }
    }
    return vec4<f32>(0.0);
}

// The sky along unit `dir` (cd/m^2, not pre-exposed): last frame's picture where the screen
// shows the sky in that direction, else the environment map, as the water's own sky term
// reflects it at roughness 0; within `mirror_at_sun`'s cone, the sky just past it.
fn mirror_sky(dir: vec3<f32>) -> vec3<f32> {
    let d = select(dir, mirror_off_sun(dir, 1e-3), mirror_at_sun(dir, 0.0));
    let seen = mirror_screen_sky(d);
    if (seen.w > 0.0) { return seen.rgb; }
    return sample_env(d);
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

// What the far view drew at the picture's `uv`, bilinear over the four texels around it, with
// only those counted that drew something (its own sky, fogged as its sky pass fogs it, laid
// over the screen's sky would ring a ridge's outline) as deep along the mirrored view as `lo`
// to `hi` (m), each by how far it lies past `lo` toward `full`, smoothly (all alike where
// `full` is no deeper than `lo`): its colour (cd/m^2), how much of the read it covers (the
// weight of the texels counted over theirs and the sky's: a ridge's outline is partly
// covered), and the depth of the nearest of them. A texel's own read keeps to what lies past
// where its own ray gives way to the view: the view is drawn from one near plane for the whole
// picture, short of every texel's reach, so it also holds what the rays hit, which a
// neighbour's read would fringe at a quarter of the picture's size, and what stands under the
// plane in features that don't cut there per pixel. What such a texel hides is unknown rather
// than open, so a nearer thing's outline leaves no hole in a ridge behind it either. Apart,
// the sky it drew there (cd/m^2), from the texels of the read that drew the sky clear of the
// light's disc (`mirror_at_sun` along each one's own ray, widened by `mirror_far_widen` for
// the view's anti-aliasing), and their weight in the read (`clear`, 0 where none did):
// `mirror_far`'s sky where the screen doesn't show it. A texel it counts within that of the
// light it holds no brighter than half again the environment map's sky there
// (`mirror_sun_cap`). Nothing covered and no sky
// without a far view.
struct FarView {
    rgb: vec3<f32>,
    cover: f32,
    depth: f32,
    sky: vec3<f32>,
    clear: f32,
};

fn mirror_far_view(uv: vec2<f32>, lo: f32, full: f32, hi: f32) -> FarView {
    var out: FarView;
    let far = mirror_trace.far;
    if (far.x <= 0.0) { return out; }
    let size = vec2<i32>(textureDimensions(mirror_far_depth));
    let p = uv * vec2<f32>(size) - 0.5;
    let base = vec2<i32>(floor(p));
    let f = p - floor(p);
    let widen = mirror_far_widen();
    var sum = vec3<f32>(0.0);
    var taken = 0.0;
    var open = 0.0;
    var sky = vec3<f32>(0.0);
    var clear = 0.0;
    var nearest = 0.0;
    for (var i = 0; i < 4; i++) {
        let o = vec2<i32>(i & 1, i >> 1u);
        let at = clamp(base + o, vec2<i32>(0), size - 1);
        var w = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        let z = textureLoad(mirror_far_depth, at, 0);
        if (z <= 0.0) {
            open += w;
            if (w > 0.0 && !mirror_at_sun(mirror_far_dir(at, size), widen)) {
                sky += textureLoad(mirror_far_color, at, 0).rgb * w;
                clear += w;
            }
            continue;
        }
        let deep = far.y / z;
        if (deep < lo || deep > hi) { continue; }
        if (full > lo) { w *= smoothstep(lo, full, deep); }
        if (w <= 0.0) { continue; }
        var c = textureLoad(mirror_far_color, at, 0).rgb;
        let toward = mirror_far_dir(at, size);
        if (mirror_at_sun(toward, widen)) { c = min(c, mirror_sun_cap(toward) * far.x); }
        sum += c * w;
        taken += w;
        nearest = max(nearest, z);
    }
    if (clear > 0.0) {
        out.sky = min(sky / (clear * far.x), vec3<f32>(60000.0));
        out.clear = clear;
    }
    if (taken <= 0.0) { return out; }
    out.cover = taken / (taken + open);
    // (Rounding would march the screen for a two-millionth of a whole read.)
    if (out.cover > 0.9999) { out.cover = 1.0; }
    out.rgb = min(sum / (taken * far.x), vec3<f32>(60000.0));
    out.depth = far.y / nearest;
    return out;
}

// What lies along unit `dir` from camera-relative `origin` past the ray's reach that the
// screen shows (a hill beyond 384 m), from last frame's picture: rgb and its distance from
// `origin`; else the sky, at distance 0: the screen's where it shows the sky that way, else
// what the far view drew as its sky there (`view.sky`, its read at the picture's texel, where
// it holds any clear of the cone round the light), else the environment map. The view's sky
// has the clouds and the fog as a drawn view's; the environment map lost the small clouds (a
// 256 by 128 picture, the weather's clouds marched into it more cheaply) and has no fog.
// Within `mirror_at_sun`'s cone the sky is the one just past it (`mirror_off_sun`): the far
// view's sky leaves out its texels within two of the cone (`mirror_far_widen`), so it is read
// again past those. What the screen shows there is no brighter than half again the
// environment map's sky (`mirror_sun_cap`).
fn mirror_far(origin: vec3<f32>, dir: vec3<f32>, noise: f32, view: FarView) -> vec4<f32> {
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
                var seen = full_seen_at(prev_uv);
                if (mirror_at_sun(dir, 0.0)) { seen = min(seen, mirror_sun_cap(dir)); }
                return vec4<f32>(seen, max(distance(hit, origin), 1e-3));
            }
        }
    }
    let widen = mirror_far_widen();
    var d = dir;
    var sky = view;
    if (mirror_at_sun(dir, widen)) {
        d = mirror_off_sun(dir, widen + mirror_far_margin());
        // (Its sky alone: nothing it drew lies that deep.)
        sky = mirror_far_view(mirror_uv(d), 3.0e38, 3.0e38, 3.0e38);
    }
    let seen = mirror_screen_sky(d);
    if (seen.w > 0.0) { return vec4<f32>(seen.rgb, 0.0); }
    if (sky.clear > 0.0) { return vec4<f32>(sky.sky, 0.0); }
    return vec4<f32>(sample_env(d), 0.0);
}

// A texel of the far view across (radians, about: as the tangent span at its middle, which
// overstates the angle off it).
fn mirror_far_texel() -> f32 {
    let size = vec2<f32>(textureDimensions(mirror_far_depth));
    let t = mirror_trace.tangents;
    return max((t.y - t.x) / size.x, (t.w - t.z) / size.y);
}

// How much wider than `mirror_at_sun`'s cone the far view's texels may hold the light's disc
// (radians): two of them, for its anti-aliasing (each frame's jitter moves a texel's sample
// half a texel, and its history's resampling another as the view moves). 0 without a far
// view.
fn mirror_far_widen() -> f32 {
    if (mirror_trace.far.x <= 0.0) { return 0.0; }
    return 2.0 * mirror_far_texel();
}

// How far past the widened cone a read of the far view must lie to take none of the texels
// inside it (a texel and a half), and a little more for the screen's read.
fn mirror_far_margin() -> f32 {
    if (mirror_trace.far.x <= 0.0) { return 1e-3; }
    return 1.5 * mirror_far_texel() + 1e-3;
}

// The direction of the far view's texel `at` of `size`, from the mirrored eye (unit).
fn mirror_far_dir(at: vec2<i32>, size: vec2<i32>) -> vec3<f32> {
    return normalize(mirror_ray((vec2<f32>(at) + 0.5) / vec2<f32>(size)));
}

// The ray from the mirrored eye through the picture's `uv` (its far view's alike), 1 along the
// mirrored view's axis: `u` along it is a point's depth in that view.
fn mirror_ray(uv: vec2<f32>) -> vec3<f32> {
    let m = mirror_trace;
    return m.right.xyz * mix(m.tangents.x, m.tangents.y, uv.x)
        + m.up.xyz * mix(m.tangents.w, m.tangents.z, uv.y) - m.back.xyz;
}

// Where `dir` from the mirrored eye goes through the picture (its far view alike): `mirror_ray`
// undone (its axes are a rotation's). Outside 0..1 off the picture.
fn mirror_uv(dir: vec3<f32>) -> vec2<f32> {
    let m = mirror_trace;
    let z = max(-dot(dir, m.back.xyz), 1e-6);
    let t = vec2<f32>(dot(dir, m.right.xyz), dot(dir, m.up.xyz)) / z;
    return vec2<f32>((t.x - m.tangents.x) / (m.tangents.y - m.tangents.x),
        (t.y - m.tangents.w) / (m.tangents.z - m.tangents.w));
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
    // The texel's ray from the mirrored eye.
    let d = mirror_ray(uv);
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
            // The sky is the view's own where the screen doesn't show it (`mirror_far`).
            let view = mirror_far_view(uv, u + m.far.z / len, u + MIRROR_T_MAX / len, 3.0e38);
            var beyond = vec4<f32>(0.0);
            if (view.cover < 1.0) {
                beyond = mirror_far(origin, dir, noise, view);
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
            // Within the cone round the light every hit no brighter than half again the
            // environment map's sky there (`mirror_sun_cap`): one the screen answers is last
            // frame's picture, which blends the disc into the outline of what stands in front
            // of it (one the rays light, lit from behind, is darker than that).
            if (mirror_at_sun(dir, 0.0)) { rgb = min(rgb, mirror_sun_cap(dir)); }
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
