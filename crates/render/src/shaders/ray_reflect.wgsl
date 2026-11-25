// Every pixel's reflection (`trial_cut_more::RAY_REFLECTIONS`, probe_gi/mod.rs
// `dispatch_reflections`): one reflection for each prepass pixel up to the roughness limit
// (`frame.reflections.z`), in the frame's ray pass after the sun's trace. Composed after
// probe_gi.wgsl on the frame's ray scene (lib_ray.wgsl, probe_gi_rays.wgsl), whose hits
// ray_hit_light.wgsl lights as `shade_surface` lights a surface (`reflected_hit_light`).
//
// Each pixel's reflection leaves along the lobe's dominant direction, as ssr.wgsl and
// `cs_reflect` reflect, kept above the face the prepass drew (a normal map's bend can turn it
// into the surface). The screen first: the march of ssr.wgsl `trace` through the prepass's
// depth (64 steps, 6 to refine); a surface it meets that faces the reflection is what the
// camera saw there last frame, found through the motion vectors (within the screen's outer
// 8%, where last frame's picture ends, mixed with the ray's answer). Otherwise a ray through
// the ray scene (`trace_reflect`), as long as the roughness lets a reflection be told apart
// (`min(384, 400 exp(-5 roughness))` m: a rough reflection's blur hides what is far). A hit the
// camera sees now (it projects onto the screen where the prepass drew the same surface) is
// taken from last frame's picture too, mixed with its own light over the screen's outer 8%;
// any other hit is lit by its own light: its tile, the sun through a shadow ray and the
// leaves above, the two nearest lamps through shadow rays up to roughness 0.3 (none above),
// last frame's probes, and inside the height fog as `shade_surface` lights a surface there; and
// fogged from the camera to the hit, as last frame's picture holds what the screen answers
// (ray_hit_light.wgsl `reflected_camera_fog`; the main view then fogs the camera's leg to the
// surface over both, `reflect_full_pixel`). A ray that meets nothing past where the march passed
// over the sky (a window, a gap in the trees) sees the sky. ssr.wgsl `fs_reflect_resolve` blurs
// the result over each pixel's lobe.
//
// Last frame's picture exists from the second frame of a view on (`frame.reflections.y`, its
// pre-exposure, is 0 before): until then every reflection is a ray's, lit by its own light.

@group(2) @binding(20) var full_normal: texture_2d<f32>; // prepass normals (octahedral xy, roughness, written)
@group(2) @binding(21) var full_depth: texture_depth_2d; // prepass depth (reverse-Z)
@group(2) @binding(22) var full_motion: texture_2d<f32>; // screen motion (uv, current - previous, without the jitter)
// Last frame's finished picture, pre-exposed by `frame.reflections.y` (the output's size,
// which temporal upscaling makes larger than the view's).
@group(2) @binding(23) var full_picture: texture_2d<f32>;
// Reflected radiance (cd/m^2, not pre-exposed, at most 60000) and the distance to what it met:
// 0 where nothing was met within a shortened ray's reach (what lies past it is the sky cover
// map's to tell) or nothing was traced (the sky, a pixel rougher than the limit), -1 where
// nothing was met within the reach of the cover map's march (open sky; ssr.wgsl tells
// `shade_surface`). The view's size.
@group(2) @binding(24) var full_out: texture_storage_2d<rgba16float, write>;
// Counted while `COUNT_REFLECTIONS` is set (cleared before the pass): the pixels under the
// roughness limit, those the screen answered, the rays traced, and the rays' hits taken from
// last frame's picture.
@group(2) @binding(25) var<storage, read_write> full_counts: array<atomic<u32>, 4>;

const FULL_MARCH_STEPS: i32 = 64;
const FULL_REFINE_STEPS: i32 = 6;
// `shade_skip::COUNT_MESH_RUNS`: the counts of the frame's work are asked for.
const COUNT_REFLECTIONS: u32 = 262144u;
// The screen's outer share over which a hit the camera sees mixes into its own light.
const FULL_SCREEN_EDGE: f32 = 0.08;
// How far above the face the prepass drew a reflection is kept (the cosine to the face).
const FULL_ABOVE_FACE: f32 = 0.05;
// When last frame's picture stands for a surface the screen's march or a ray met
// (`full_picture_stands`). What the camera saw there is the surface's own light and its
// reflection of what lies the other way from the camera, a share of its light that rises as it
// is seen more edge on (Schlick's (1 - cos)^5). Where the camera sees a rough surface face on
// (this share at most FULL_SEEN_OWN_MAX, a cosine of 0.5; its roughness FULL_FACE_ON_ROUGH at
// least, so a light's highlight on it is wide and dim), it looks about the same from every
// side and the picture stands. Elsewhere the picture stands only where the surface reflects
// about the same toward the camera as toward the reflection: its two mirror directions within
// its lobe (`FULL_LOBE_MIN` at the least), so the sun's glare on a glossy floor, or a lamp's
// streak on a wet one, isn't copied into a wall mirror that sees that floor from the other
// side; and the camera's share no more than FULL_FRESNEL_GAP above the reflection's, so the
// underside of a ball seen from just above isn't copied into the mirror under it (dark specks).
// A rough wall seen in the floor in front of it keeps the picture however edge on, and the
// ray's own light, which leaves the surface's reflection out, stands in elsewhere.
const FULL_FRESNEL_GAP: f32 = 0.05;
const FULL_SEEN_OWN_MAX: f32 = 0.032;
// (A dielectric's highlight from a light at this roughness peaks at about 40% of its own light
// where its albedo is 0.3; at 0.15, about a hundred times.)
const FULL_FACE_ON_ROUGH: f32 = 0.6;
// The least half width of a surface's lobe for `full_picture_stands`, radians (6 degrees): a
// nearly smooth surface reflects about the same over that.
const FULL_LOBE_MIN: f32 = 0.1;
// Never more edge on to the camera than this cosine: the pixels there are outlines, which hold
// the background beside them.
const FULL_OUTLINE_MIN: f32 = 0.1;

var<workgroup> full_group_counts: array<atomic<u32>, 4>;

// The march's answer: where on the screen it met a surface, or whether it passed over the sky
// before it went behind anything (`escaped`: it left through open space the camera sees, a
// window or a gap in the trees, `dist` meters from its start), as ssr.wgsl `trace` tells.
struct FullMarch {
    uv: vec2<f32>,
    found: bool,
    escaped: bool,
    dist: f32,
};

// Screen uv and view depth (positive, meters) of view-space point `p` (ssr.wgsl `project`).
fn full_project(p: vec3<f32>) -> vec3<f32> {
    let c = frame.proj * vec4<f32>(p, 1.0);
    let ndc = c.xy / c.w;
    return vec3<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, -p.z);
}

// View depth of the scene at `uv` (0 where nothing was drawn).
fn full_scene_depth(uv: vec2<f32>) -> f32 {
    let size = vec2<i32>(textureDimensions(full_depth));
    let px = clamp(vec2<i32>(floor(uv * frame.viewport.xy)), vec2<i32>(0), size - 1);
    let z = textureLoad(full_depth, px, 0);
    if (z <= 0.0) { return 0.0; }
    return scene_linear_depth(uv, z);
}

// The march of ssr.wgsl `trace` from view-space `origin` along `dir` (unit) for up to
// `max_len` meters: evenly in pixels, perspective-correct depth, the first place the ray passes
// behind a surface by less than its thickness, refined between that step and the one before.
fn full_march(origin: vec3<f32>, dir: vec3<f32>, max_len: f32, jitter: f32) -> FullMarch {
    var hit: FullMarch;
    hit.found = false;
    hit.escaped = false;
    // Keep the end in front of the near plane.
    var len = max_len;
    if (dir.z > 0.0) {
        len = min(len, (-frame.exposure.z * 1.5 - origin.z) / dir.z);
    }
    if (len <= 0.0) { return hit; }
    let a = full_project(origin);
    let b = full_project(origin + dir * len);
    let inv_a = 1.0 / a.z;
    let inv_b = 1.0 / b.z;
    let pixels = length((b.xy - a.xy) * frame.viewport.xy);
    let steps = clamp(i32(pixels), 1, FULL_MARCH_STEPS);
    let dk = 1.0 / f32(steps);
    var prev_k = 0.0;
    var k = dk * (0.5 + jitter);
    var hidden = false;
    var open_k = -1.0;
    for (var i = 0; i < steps; i++) {
        let uv = mix(a.xy, b.xy, k);
        if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) { return hit; }
        let ray = 1.0 / mix(inv_a, inv_b, k);
        let scene = full_scene_depth(uv);
        let step_span = abs(1.0 / mix(inv_a, inv_b, min(k + dk, 1.0)) - ray);
        let thickness = max(0.1 + 0.02 * scene, step_span * 1.5);
        // Over the sky with nothing in the way so far, the ray is in open space the camera sees
        // through to the sky; behind something from there on, it is beyond what that hides (a
        // window's frame and wall seen from inside), still out in the open (ssr.wgsl `trace`).
        if (scene <= 0.0 && !hidden && open_k < 0.0) { open_k = k; }
        hidden = hidden || (scene > 0.0 && ray > scene);
        if (scene > 0.0 && ray > scene && ray - scene < thickness) {
            var lo = prev_k;
            var hi = k;
            for (var j = 0; j < FULL_REFINE_STEPS; j++) {
                let mid = 0.5 * (lo + hi);
                let m_ray = 1.0 / mix(inv_a, inv_b, mid);
                let m_scene = full_scene_depth(mix(a.xy, b.xy, mid));
                if (m_scene > 0.0 && m_ray > m_scene) { hi = mid; } else { lo = mid; }
            }
            // The ray crossed what the screen shows there, rather than passed behind its
            // outline: it lies behind the surface by no more than a surface's thickness, or a
            // pixel before the crossing it lay in front of the surface as the screen shows it
            // there or at the crossing (a step can span several pixels), so it reached the
            // surface's depth within that pixel. Else, under a ball, the march met the ball's
            // outline and its near side's light speckled the reflection of the underside the ray
            // goes on to meet; and toward the ray's vanishing point, where a pixel spans
            // hundreds of metres of the ray, a ray from a lake's far side that had gone far
            // behind a ball peeking in at the top of the screen took the ball's pixels (dark
            // specks), as the mirror's march past the ray's reach did (`mirror_far`). (Allowed
            // as far behind as the ray moves in depth across a pixel, the rule before let both
            // through.)
            let cross_uv = mix(a.xy, b.xy, hi);
            let cross_ray = 1.0 / mix(inv_a, inv_b, hi);
            let cross_scene = full_scene_depth(cross_uv);
            let thick = 0.1 + 0.02 * cross_scene;
            let back_k = max(hi - 1.0 / max(pixels, 1.0), 0.0);
            let back_ray = 1.0 / mix(inv_a, inv_b, back_k);
            let back_scene = full_scene_depth(mix(a.xy, b.xy, back_k));
            if (cross_ray - cross_scene < thick || back_ray < max(back_scene, cross_scene) + thick) {
                hit.uv = cross_uv;
                hit.found = true;
                return hit;
            }
        }
        prev_k = k;
        k += dk;
    }
    if (open_k >= 0.0) {
        hit.escaped = true;
        // Depth changes linearly along the ray.
        let depth = 1.0 / mix(inv_a, inv_b, open_k);
        hit.dist = clamp(select(open_k * len, (-depth - origin.z) / dir.z, abs(dir.z) > 1e-3), 0.0, len);
    }
    return hit;
}

// The camera-relative point of depth `z` at pixel `px` of a `size` view.
fn full_point(px: vec2<i32>, z: f32, size: vec2<i32>) -> vec3<f32> {
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(size);
    return (frame.inv_view * vec4<f32>(view_position(uv, z), 0.0)).xyz;
}

// The face's normal at pixel `px` (camera-relative `rel`, depth `z`) from the depth of the
// pixels beside it, as ray_sun.wgsl `sun_face_normal` finds it: on each axis the side nearer
// in depth, turned toward the camera (`toward`); `fallback` where a side has none.
fn full_face_normal(px: vec2<i32>, rel: vec3<f32>, z: f32, size: vec2<i32>, toward: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let last = size - 1;
    let pl = max(px - vec2<i32>(1, 0), vec2<i32>(0));
    let pr = min(px + vec2<i32>(1, 0), last);
    let pu = max(px - vec2<i32>(0, 1), vec2<i32>(0));
    let pd = min(px + vec2<i32>(0, 1), last);
    let zl = textureLoad(full_depth, pl, 0);
    let zr = textureLoad(full_depth, pr, 0);
    let zu = textureLoad(full_depth, pu, 0);
    let zd = textureLoad(full_depth, pd, 0);
    let left = px.x > 0 && (px.x == last.x || abs(zl - z) < abs(zr - z));
    let up = px.y > 0 && (px.y == last.y || abs(zu - z) < abs(zd - z));
    let zx = select(zr, zl, left);
    let zy = select(zd, zu, up);
    if (zx <= 0.0 || zy <= 0.0) { return fallback; }
    let ex = select(full_point(pr, zr, size) - rel, rel - full_point(pl, zl, size), left);
    let ey = select(full_point(pd, zd, size) - rel, rel - full_point(pu, zu, size), up);
    let c = cross(ey, ex);
    let len = length(c);
    if (!(len > 1e-12)) { return fallback; }
    let n = c / len;
    return select(n, -n, dot(n, toward) < 0.0);
}

// How much of the screen's outer `FULL_SCREEN_EDGE` lies between `uv` and the screen's edge.
fn full_edge(uv: vec2<f32>) -> f32 {
    let d = min(uv, 1.0 - uv);
    return saturate(min(d.x, d.y) / FULL_SCREEN_EDGE);
}

// Last frame's picture at screen `uv` (not pre-exposed).
fn full_seen_at(uv: vec2<f32>) -> vec3<f32> {
    let c = textureSampleLevel(full_picture, linear_clamp, uv, 0.0).rgb;
    return min(c / frame.reflections.y, vec3<f32>(60000.0));
}

// Whether last frame's picture of a surface facing `n` of perceptual `roughness`, seen along
// unit `v` (toward the camera), stands for the light it sends back along unit `back` (toward
// where a reflection's ray comes from): `FULL_FRESNEL_GAP`, `FULL_FACE_ON_ROUGH`,
// `FULL_OUTLINE_MIN`.
fn full_picture_stands(n: vec3<f32>, v: vec3<f32>, back: vec3<f32>, roughness: f32) -> bool {
    let nv = dot(n, v);
    let seen = pow(1.0 - saturate(nv), 5.0);
    if (nv <= FULL_OUTLINE_MIN) { return false; }
    if (seen <= FULL_SEEN_OWN_MAX && roughness >= FULL_FACE_ON_ROUGH) { return true; }
    let reflected = pow(1.0 - saturate(dot(n, back)), 5.0);
    // GGX's lobe: about 1.3 alpha wide each side of its mirror direction (half its peak).
    let lobe = max(FULL_LOBE_MIN, 1.3 * roughness * roughness);
    return seen - reflected < FULL_FRESNEL_GAP
        && dot(reflect(-v, n), reflect(-back, n)) > cos(min(lobe, 1.5));
}

// What last frame's picture shows of camera-relative point `p` on a surface facing `n`, for a
// ray that met it along unit `dir`, where the camera sees that surface there now (the prepass
// drew a surface as deep, facing about the same way, at the pixel `p` projects onto) and its
// picture stands for the ray's (`full_picture_stands`): rgb, and its share (0: the camera
// doesn't see it; under 1 within the screen's outer 8%, where the hit's own light takes over).
fn full_seen(p: vec3<f32>, n: vec3<f32>, dir: vec3<f32>) -> vec4<f32> {
    let c = frame.view_proj * vec4<f32>(p, 1.0);
    if (c.w <= 0.0) { return vec4<f32>(0.0); }
    let ndc = c.xy / c.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) { return vec4<f32>(0.0); }
    let size = vec2<i32>(textureDimensions(full_depth));
    let px = clamp(vec2<i32>(floor(uv * vec2<f32>(size))), vec2<i32>(0), size - 1);
    let z = textureLoad(full_depth, px, 0);
    let nr = textureLoad(full_normal, px, 0);
    if (z <= 0.0 || nr.w < 0.5) { return vec4<f32>(0.0); }
    let depth = view_depth(p);
    let scene = scene_linear_depth((vec2<f32>(px) + 0.5) / vec2<f32>(size), z);
    if (abs(scene - depth) > 0.02 * depth + 0.05 || dot(oct_decode(nr.xy), n) < 0.3) {
        return vec4<f32>(0.0);
    }
    let v = select(normalize(-p), frame.inv_view[2].xyz, frame.exposure.w > 0.5);
    if (!full_picture_stands(n, v, -dir, nr.z)) { return vec4<f32>(0.0); }
    let prev_uv = uv - textureLoad(full_motion, px, 0).xy;
    if (any(prev_uv < vec2<f32>(0.0)) || any(prev_uv > vec2<f32>(1.0))) { return vec4<f32>(0.0); }
    return vec4<f32>(full_seen_at(prev_uv), full_edge(uv) * full_edge(prev_uv));
}

// (A ray's hit is lit by ray_hit_light.wgsl `reflected_hit_light`, which the water's reflection
// ray shares: its light, its lamps and its own shine.)

// The reflection at pixel `pixel`, written to `full_out`, and what it adds to the counts
// (pixels under the limit, answered by the screen, rays, rays' hits taken from the picture).
fn reflect_full_pixel(pixel: vec2<u32>) -> vec4<u32> {
    let px = vec2<i32>(pixel);
    let size = vec2<i32>(textureDimensions(full_depth));
    let nr = textureLoad(full_normal, px, 0);
    let z = textureLoad(full_depth, px, 0);
    let roughness = nr.z;
    if (z <= 0.0 || nr.w < 0.5 || roughness > frame.reflections.z) {
        textureStore(full_out, pixel, vec4<f32>(0.0));
        return vec4<u32>(0u);
    }
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(size);
    let pos = view_position(uv, z);
    let rel = (frame.inv_view * vec4<f32>(pos, 0.0)).xyz;
    let depth = -pos.z;
    let n = oct_decode(nr.xy);
    let alpha = roughness * roughness;
    let v = select(normalize(-rel), frame.inv_view[2].xyz, frame.exposure.w > 0.5);
    // The lobe's dominant direction, as `shade_surface`, ssr.wgsl and `cs_reflect` reflect,
    // above the face the prepass drew: the ray leaves the face, whatever the normal map bends.
    let face = full_face_normal(px, rel, z, size, v, select(n, -n, dot(n, v) < 0.0));
    let r = reflect(-v, n);
    var dir = normalize(mix(n, r, (1.0 - alpha) * (sqrt(1.0 - alpha) + alpha)));
    let above = dot(dir, face);
    if (above < FULL_ABOVE_FACE) {
        dir = normalize(dir + face * (FULL_ABOVE_FACE - above));
    }
    let origin = rel + face * (0.02 + 0.002 * depth);
    let noise = interleaved_gradient_noise(vec2<f32>(px) + frame.time.w * 5.588238);
    // The ray's cone: a pixel across where it starts, widening by a pixel's angle and the
    // lobe's (as wide all along when orthographic), which picks the level of the hit's tile.
    let pixel_m = 2.0 / (abs(frame.proj[1][1]) * frame.viewport.y);
    let cone = select(vec2<f32>(pixel_m * depth, pixel_m + alpha), vec2<f32>(pixel_m, alpha), frame.exposure.w > 0.5);
    // The rays' light is fogged from the camera to the hit, as last frame's picture holds what
    // the screen answers, which the main view fogs again from the camera to the surface. The
    // true reflected leg starts at `origin`, but a reflection the rays fog less than the screen
    // does would change as it left the screen (more of its own light, less of the fog's: darker
    // where the fog outshines it, brighter where it outshines the fog), which shows in motion,
    // where one a little over fogged doesn't (Rendering, 25 Nov). With volumetric fog on, the
    // screen's answers within its range carry the froxels' fog, lit through the shadow cascades
    // and by the lamps and holding the fog volumes, which the rays' fog hasn't: there the two can
    // still step apart (ray_hit_light.wgsl `reflected_camera_fog`). Whoever gives the screen's
    // answers their true leg flips this to the leg from `origin` with it
    // (`vec4<f32>(origin, 1.0)`, whose haze then wants the sun's shadows along it:
    // `reflected_path_fog`), and the water's ray with it (ray_water.wgsl).
    let found = reflect_at(origin, dir, depth, roughness, cone, noise, vec4<f32>(0.0));
    textureStore(full_out, pixel, found.out);
    var counts = found.counts;
    counts.x = 1u;
    return counts;
}

// What a ray lit at camera-relative `hit`, `rgb` (cd/m^2), fogged as `reflect_at`'s `fog_from`
// says: with w 0 from the camera, as `surface_fog` fogs a point the camera sees
// (ray_hit_light.wgsl `reflected_camera_fog`, its samples turned by `noise`: every pixel's
// reflection); with w 1 along the straight path from camera-relative xyz (`reflected_path_fog`:
// a mirror's traced picture, from the mirrored eye).
fn reflected_fog(rgb: vec3<f32>, hit: vec3<f32>, fog_from: vec4<f32>, noise: f32) -> vec3<f32> {
    if (fog_from.w > 0.5) { return reflected_path_fog(rgb, fog_from.xyz, hit); }
    return reflected_camera_fog(rgb, hit, noise);
}

// What a reflection found (`reflect_at`).
struct Reflected {
    // As `full_out` holds it: rgb cd/m^2 (not pre-exposed, at most 60000); w the distance to
    // what it met, -1 open sky past the cover map's reach, 0 nothing.
    out: vec4<f32>,
    // What it adds to the counts: y the screen answered, z a ray was traced, w the ray's hit
    // was taken from last frame's picture (x is the caller's).
    counts: vec4<u32>,
    // The ray met the back of a single-sided surface: it started inside something.
    inside: bool,
    // It met nothing but the sky: past an opening the camera sees the sky through, or past
    // the reach of the cover map's march (and no hit of the march in the screen's outer band).
    sky: bool,
    // Some of `out` is last frame's picture: the screen answered, wholly or by its share in
    // the screen's outer band, or the ray's hit was read from it (the traced mirror holds what
    // the picture shows within the light's cone to the sky round it).
    from_picture: bool,
};

// The reflection along unit `dir` from camera-relative `origin` (just off the surface) of a
// surface of perceptual `roughness` at view depth `depth`, its ray's cone `cone` (as
// `ray_surface` takes it), `noise` its pixel's: the screen first, where last frame's picture
// exists, then a ray through the ray scene. What the ray lights itself is fogged as `fog_from`
// says (`reflected_fog`); what the screen or last frame's picture answers is fogged as the
// camera saw it.
fn reflect_at(origin: vec3<f32>, dir: vec3<f32>, depth: f32, roughness: f32, cone: vec2<f32>, noise: f32, fog_from: vec4<f32>) -> Reflected {
    var found: Reflected;
    found.out = vec4<f32>(0.0);
    found.counts = vec4<u32>(0u);
    found.inside = false;
    found.sky = false;
    found.from_picture = false;
    let size = vec2<i32>(textureDimensions(full_depth));

    // The screen first, where last frame's picture exists. A surface the march met within the
    // screen's outer 8% is last frame's picture only by its share there (`screen`, at
    // `screen_t` meters), the ray's answer the rest, as a ray's hit the camera sees mixes into
    // its own light: answered at once, the picture ended in a seam where its subject left the
    // screen.
    var screen = vec4<f32>(0.0);
    var screen_t = 0.0;
    var escaped = false;
    var escape_t = 0.0;
    let rays_only = (ray_scene.stand_ins.w & RAY_SCENE_REFLECT_RAYS_ONLY) != 0u;
    if (frame.reflections.y > 0.0 && !rays_only) {
        let origin_view = (frame.view * vec4<f32>(origin, 0.0)).xyz;
        let dir_view = normalize((frame.view * vec4<f32>(dir, 0.0)).xyz);
        let m = full_march(origin_view, dir_view, min(depth * 3.0 + 30.0, 1000.0), noise);
        if (m.found) {
            let hit_px = clamp(vec2<i32>(floor(m.uv * frame.viewport.xy)), vec2<i32>(0), size - 1);
            let hit_nr = textureLoad(full_normal, hit_px, 0);
            let hit_n = oct_decode(hit_nr.xy);
            let prev_uv = m.uv - textureLoad(full_motion, hit_px, 0).xy;
            let hit_rel = (frame.inv_view * vec4<f32>(view_position(m.uv, textureLoad(full_depth, hit_px, 0)), 0.0)).xyz;
            let hit_v = select(normalize(-hit_rel), frame.inv_view[2].xyz, frame.exposure.w > 0.5);
            // The surface met faces the reflection (its back is not what the camera saw), the
            // camera's picture of it stands for the reflection's (`full_picture_stands`), and
            // last frame's picture holds it.
            if (dot(hit_n, dir) < -0.1 && full_picture_stands(hit_n, hit_v, -dir, hit_nr.z)
                && all(prev_uv >= vec2<f32>(0.0)) && all(prev_uv <= vec2<f32>(1.0))) {
                screen = vec4<f32>(full_seen_at(prev_uv), full_edge(m.uv) * full_edge(prev_uv));
                screen_t = max(distance(hit_rel, origin), 1e-3);
                if (screen.a >= 1.0) {
                    found.out = vec4<f32>(screen.rgb, screen_t);
                    found.counts.y = 1u;
                    found.from_picture = true;
                    return found;
                }
            }
        }
        escaped = m.escaped;
        escape_t = m.dist;
    }

    // A ray through the ray scene.
    found.counts.z = 1u;
    let t_max = min(384.0, 400.0 * exp(-5.0 * roughness));
    let hit = trace_reflect(origin, dir, t_max);
    // Where the march saw open sky along the reflection, the camera sees through there: an
    // indoor volume's closed face across a window doesn't stop it.
    let wall = select(sky_volume_wall(origin, dir, min(hit.t, t_max)), NO_HIT, escaped);
    var out = vec4<f32>(0.0);
    if (wall < NO_HIT) {
        // A wall of an indoor volume (a cave's rock): lit by the probes around it.
        let p = origin + dir * wall;
        let indirect = probe_gi(p, -dir, -dir);
        out = vec4<f32>(reflected_fog(VOLUME_WALL_ALBEDO * INV_PI * indirect.rgb * indirect.a,
            p, fog_from, noise), max(wall, 1e-3));
    } else if (hit.instance != RAY_NONE) {
        let s = ray_surface(ray_last, origin, dir, cone);
        // The back of a single-sided surface: the ray started inside something, so it
        // reflects nothing of its own.
        found.inside = s.inside;
        if (!s.inside) {
            // (Where the march met it in the screen's outer band, its picture is `screen`'s.)
            var seen = vec4<f32>(0.0);
            if (frame.reflections.y > 0.0 && screen.a <= 0.0) {
                seen = full_seen(origin + dir * hit.t, s.face, dir);
            }
            var lit = vec3<f32>(0.0);
            if (seen.a < 1.0) {
                lit = reflected_fog(reflected_hit_light(origin, dir, hit.t, s, roughness),
                    origin + dir * hit.t, fog_from, noise);
            }
            if (seen.a > 0.0 || screen.a > 0.0) {
                found.counts.w = 1u;
                found.from_picture = true;
            }
            out = vec4<f32>(min(mix(lit, seen.rgb, seen.a), vec3<f32>(60000.0)), max(hit.t, 1e-3));
        }
    } else if (escaped) {
        // The ray met nothing past an opening the camera sees the sky through (a window, a gap
        // in the trees): the sky, which the cover overhead doesn't dim (it would under a roof
        // or a canopy, `shade_surface`), as ssr.wgsl `fs_ssr` reflects an escaped march.
        out = vec4<f32>(min(sample_env_rough(dir, roughness) + frame.ambient.rgb, vec3<f32>(60000.0)), max(escape_t, 1e-3));
        found.sky = true;
    } else if (REFLECT_MISS_SEES_SKY && t_max >= SKY_MARCH_REACH_M) {
        // Nothing within the reach of the cover map's march (`specular_sky_visibility`): the
        // sky there is open as far as the scene's things go.
        out = vec4<f32>(0.0, 0.0, 0.0, -1.0);
        found.sky = true;
    }
    if (screen.a > 0.0) {
        // The march's hit in the screen's outer band: over what the ray met by its share, or
        // as it is where the ray met nothing (something the ray scene leaves out).
        out = select(vec4<f32>(screen.rgb, screen_t), vec4<f32>(mix(out.rgb, screen.rgb, screen.a), out.w), out.w > 0.0);
        found.sky = false;
        found.from_picture = true;
    }
    found.out = out;
    return found;
}

@compute @workgroup_size(8, 8, 1)
fn cs_reflect_full(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) thread: u32) {
    let counting = shade_skip(COUNT_REFLECTIONS);
    if (thread < 4u) { atomicStore(&full_group_counts[thread], 0u); }
    workgroupBarrier();
    var counts = vec4<u32>(0u);
    if (all(id.xy < textureDimensions(full_out))) {
        counts = reflect_full_pixel(id.xy);
    }
    if (counting) {
        if (counts.x > 0u) { atomicAdd(&full_group_counts[0], counts.x); }
        if (counts.y > 0u) { atomicAdd(&full_group_counts[1], counts.y); }
        if (counts.z > 0u) { atomicAdd(&full_group_counts[2], counts.z); }
        if (counts.w > 0u) { atomicAdd(&full_group_counts[3], counts.w); }
    }
    workgroupBarrier();
    if (counting && thread < 4u) {
        let n = atomicLoad(&full_group_counts[thread]);
        if (n > 0u) { atomicAdd(&full_counts[thread], n); }
    }
}
