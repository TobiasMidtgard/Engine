// The sun's trace (`trial_cut_more::RAY_SUN`, ray_scene/sun.rs): a dispatch of the frame's one
// ray pass. From every pixel the prepass drew within `ray_scene.sun.x` meters of the camera that
// faces the sun (a thin sheet whichever way it faces: light reaches it through) and is not a
// swaying mesh (`prepass_sways`: the shadow maps draw those moving), one ray toward a point of
// the sun's disc, `ray_scene.sun.y` long, against the ray scene's still structure: what casts at
// any size and the ground, and beyond `ray_scene.sun.z` the swaying plants' trunks and leaves
// too (nearer, the maps draw them moving; farther, the still images hold them at rest, as the
// rays do). A cut-out card is read in its texture for a ray's first two candidates, by chance
// after them, and where the still structure holds shadow stand-ins a ray nothing stopped asks
// them as well (`ray_stopped`). The filter (ray_filters.wgsl) keeps `sun_raw` over the frames
// and the screen light pass joins it with the shadow maps (screen_light.wgsl `sun_joined`).
// Skin and, with `CUT_THIN_OWN_SUN`, thin sheets get no ray: their shaders read the shadow
// maps themselves (skin.wgsl, `shade_surface_sides`), never the pass's sun.
//
// Composed after lib_ray (`ray_scene::shader_lib(1)`) and with lib_frame: the prepass's normal
// and depth are the frame's own (group 0), as the light pass reads them.

// r: the sun's visibility at the pixel (1 the ray reached the sun, 0 something stopped it; 1
// where nothing was traced); g: the flags (`SUN_TRACED`, `SUN_PLANTS_BY_RAY`) over 255; b: how
// far the ray went before it was stopped, as the square root of its share of the rays' length
// (finer near, where the penumbra a blocker casts is narrow; 1 where it reached the sun).
@group(2) @binding(0) var sun_raw: texture_storage_2d<rgba8unorm, write>;
// While `COUNT_SUN_TRACE` is set (cleared before the pass): rays, rays stopped, "missed" (the
// still image holds a caster within the ray's length over every texel around the pixel, and a
// ray toward the sun's center reached it) and "false" (the image holds none there, and that ray
// was stopped).
@group(2) @binding(1) var<storage, read_write> sun_trace_counts: array<atomic<u32>, 4>;
// The shadow maps' still images (renderer.rs `StaticShadows`: a layer a cascade, then the far
// map's), what the counts compare the rays with; a 1x1 stand-in where the view has none.
@group(2) @binding(2) var still_images: texture_depth_2d_array;

// `sun_raw`.g: the pixel was traced; its ray met the swaying plants (it lies past
// `ray_scene.sun.z`, the dither over that cascade's last tenth taken as the shadow maps take
// it), so the light pass reads the cascade that holds them at rest.
const SUN_TRACED: u32 = 1u;
const SUN_PLANTS_BY_RAY: u32 = 2u;
// `shade_skip::COUNT_MESH_RUNS`: the counts of the frame's work are asked for.
const COUNT_SUN_TRACE: u32 = 262144u;
// What stops a ray of the trace: what casts at any size and the ground; past `ray_scene.sun.z`
// the plants' trunks and leaves as well.
const SUN_TRACE_CASTS: u32 = 67u; // SOLID | GROUND | SMALL
const SUN_TRACE_PLANTS: u32 = 12u; // SWAY | LEAF
// The ground stops a ray at its front only (a pixel on the drawn ground just under the rays'
// coarser ground is lit), a card where its texture is opaque for the first two candidates and by
// chance after them; the still structure alone.
const SUN_TRACE_RULES: u32 = RULE_GROUND_FRONT | RULE_LEAF_TEXTURE | RULE_STILL_ONLY | (2u << RULE_TESTS_SHIFT);

// The still image's verdict on a point (`still_image_verdict`).
const STILL_NONE: u32 = 0u;
const STILL_OPEN: u32 = 1u;
const STILL_BLOCKED: u32 = 2u;
const STILL_EDGE: u32 = 3u;

// The workgroup's counts, added to `sun_trace_counts` once.
var<workgroup> group_counts: array<atomic<u32>, 4>;

// The camera-relative point of depth `z` at pixel `px` of a `size` view.
fn sun_trace_point(px: vec2<i32>, z: f32, size: vec2<i32>) -> vec3<f32> {
    let uv = (vec2<f32>(px) + 0.5) / vec2<f32>(size);
    return (frame.inv_view * vec4<f32>(view_position(uv, z), 0.0)).xyz;
}

// The face's normal at pixel `px` (camera-relative `rel`, depth `z`), from the depth of the
// pixels beside it: on each axis the side whose depth is nearer the pixel's (the other may lie
// across an edge), turned toward the camera (`toward`). `fallback` where a side has none.
fn sun_face_normal(px: vec2<i32>, rel: vec3<f32>, z: f32, size: vec2<i32>, toward: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let last = size - 1;
    let pl = max(px - vec2<i32>(1, 0), vec2<i32>(0));
    let pr = min(px + vec2<i32>(1, 0), last);
    let pu = max(px - vec2<i32>(0, 1), vec2<i32>(0));
    let pd = min(px + vec2<i32>(0, 1), last);
    let zl = textureLoad(prepass_depth, pl, 0);
    let zr = textureLoad(prepass_depth, pr, 0);
    let zu = textureLoad(prepass_depth, pu, 0);
    let zd = textureLoad(prepass_depth, pd, 0);
    // (At the view's edge the one side there is.)
    let left = px.x > 0 && (px.x == last.x || abs(zl - z) < abs(zr - z));
    let up = px.y > 0 && (px.y == last.y || abs(zu - z) < abs(zd - z));
    let zx = select(zr, zl, left);
    let zy = select(zd, zu, up);
    if (zx <= 0.0 || zy <= 0.0) { return fallback; }
    let ex = select(sun_trace_point(pr, zr, size) - rel, rel - sun_trace_point(pl, zl, size), left);
    let ey = select(sun_trace_point(pd, zd, size) - rel, rel - sun_trace_point(pu, zu, size), up);
    let c = cross(ey, ex);
    let len = length(c);
    if (!(len > 1e-12)) { return fallback; }
    let n = c / len;
    return select(n, -n, dot(n, toward) < 0.0);
}

// A direction toward a point of the sun's disc (`l` its center, `tan_radius` the tangent of its
// radius), uniform over the disc's area, picked by the pixel's draws (`ray_noise`).
fn sun_disc_dir(l: vec3<f32>, tan_radius: f32, pixel: vec2<u32>) -> vec3<f32> {
    let angle = ray_noise(pixel, 0u) * TAU;
    let r = sqrt(ray_noise(pixel, 1u)) * tan_radius;
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(l.y) > 0.9);
    let t = normalize(cross(up, l));
    let b = cross(l, t);
    return normalize(l + (t * cos(angle) + b * sin(angle)) * r);
}

// What the still image of the cascade that holds camera-relative `p` says of the sun there, read
// as the shadow maps are (`cascade_visibility`: the cascade by distance, the normal bias along
// `n`, the sunlit side of the surface), over the 3x3 texels around it: `STILL_BLOCKED` each holds
// a caster within `reach` meters up-sun, `STILL_OPEN` none does, `STILL_EDGE` they differ (a
// shadow's edge, no verdict), `STILL_NONE` past the cascades, without images, or where a caster's
// distance is unknown. `plants`: the ray met the swaying plants; inside the band before
// `ray_scene.sun.z` that takes the cascade past it, the first to hold them at rest, as the
// trace took it (the cascade by distance draws them moving, so its image has none).
fn still_image_verdict(p: vec3<f32>, n: vec3<f32>, reach: f32, plants: bool) -> u32 {
    let dims = textureDimensions(still_images);
    if (dims.x < 2u) { return STILL_NONE; }
    let dist = length(p);
    if (dist > frame.shadow_splits[3]) { return STILL_NONE; }
    var cascade = 0u;
    for (var i = 0u; i < 3u; i++) {
        if (dist > frame.shadow_splits[i]) { cascade = i + 1u; }
    }
    if (plants && dist <= ray_scene.sun.z) {
        // (`sun.z` is one of the splits, the same number: `RayScene::finish`.)
        for (var i = 0u; i < 3u; i++) {
            if (ray_scene.sun.z >= frame.shadow_splits[i]) { cascade = max(cascade, i + 1u); }
        }
    }
    let size = f32(dims.x);
    let m = frame.shadow_cascades[cascade];
    let texel_world = 2.0 / (size * length(vec3<f32>(m[0].x, m[1].x, m[2].x)));
    let z_per_meter = length(vec3<f32>(m[0].z, m[1].z, m[2].z));
    let n_dot_l = dot(n, frame.sun_dir.xyz);
    let q = p + n * (frame.shadow_params.y * texel_world * (1.0 - 0.5 * saturate(n_dot_l)));
    let lp = m * vec4<f32>(q, 1.0);
    let uv = lp.xy * vec2<f32>(0.5, -0.5) + 0.5;
    let z = lp.z - frame.shadow_params.x;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || z > 1.0) { return STILL_NONE; }
    let center = vec2<i32>(uv * size);
    let last = vec2<i32>(dims.xy) - 1;
    let within = reach * z_per_meter;
    var near = 0u;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let at = clamp(center + vec2<i32>(dx, dy), vec2<i32>(0), last);
            let d = textureLoad(still_images, at, cascade, 0);
            // (A caster up-sun of the map's near plane is drawn at depth 0 wherever it stands:
            // where that plane lies within reach, its distance is not known.)
            if (d <= 0.0 && z < within) { return STILL_NONE; }
            if (d < z && z - d < within) { near += 1u; }
        }
    }
    if (near == 9u) { return STILL_BLOCKED; }
    if (near == 0u) { return STILL_OPEN; }
    return STILL_EDGE;
}

// The trace at pixel `pixel`: its visibility and flags written to `sun_raw`, and what it adds to
// the counts (rays, stopped, missed, false; only while `counting`).
fn sun_trace_pixel(pixel: vec2<u32>, counting: bool) -> vec4<u32> {
    let px = vec2<i32>(pixel);
    let size = vec2<i32>(textureDimensions(sun_raw));
    let nr = textureLoad(prepass_normal, px, 0);
    let z = textureLoad(prepass_depth, px, 0);
    let l = frame.sun_dir.xyz;
    let n_shade = oct_decode(nr.xy);
    let thin = nr.w > PREPASS_THIN - 0.5;
    let skin = nr.w > 1.0005 && nr.w < 2.5;
    // The sky, a pixel the prepass gave no normal, no sun, a swaying mesh, a face turned from
    // the sun (its light is the sky's and the probes' alone), skin and a thin sheet that read
    // the shadow maps themselves: no ray.
    if (z <= 0.0 || nr.w < 0.5 || frame.sun_color.w < 0.5 || prepass_sways(nr.w) || skin
        || (thin && trial_cut(CUT_THIN_OWN_SUN)) || !(thin || dot(n_shade, l) > 0.0)) {
        textureStore(sun_raw, pixel, vec4<f32>(1.0, 0.0, 1.0, 1.0));
        return vec4<u32>(0u);
    }
    let rel = sun_trace_point(px, z, size);
    let dist = length(rel);
    if (dist > ray_scene.sun.x) {
        textureStore(sun_raw, pixel, vec4<f32>(1.0, 0.0, 1.0, 1.0));
        return vec4<u32>(0u);
    }
    // The face's normal, toward the camera (along the view's axis when orthographic), on the
    // sun's side of a thin sheet: the ray leaves the face, not the normal map's bend.
    let toward = select(-rel, frame.inv_view[2].xyz, frame.exposure.w > 0.5);
    let fallback = select(n_shade, -n_shade, dot(n_shade, toward) < 0.0);
    var n_face = sun_face_normal(px, rel, z, size, toward, fallback);
    if (thin && dot(n_face, l) < 0.0) { n_face = -n_face; }
    let depth = view_depth(rel);
    let origin = rel + n_face * (0.02 + 0.002 * depth);
    // The plants: by ray past the last cascade that draws them swaying, dithered over its last
    // tenth as `cascade_visibility` dithers into the next cascade (never when every cascade
    // draws them swaying: `sun.z` is then past them all).
    let plants_from = ray_scene.sun.z;
    let band = plants_from * 0.1;
    let noise = frame_noise(vec2<f32>(px) + 0.5);
    let plants = dist > plants_from
        || (dist > plants_from - band && noise > (plants_from - dist) / band);
    let classes = SUN_TRACE_CASTS | select(0u, SUN_TRACE_PLANTS, plants);
    let dir = sun_disc_dir(l, ray_scene.sun.w, pixel);
    let stopped = ray_stopped(origin, dir, ray_scene.sun.y, classes, SUN_TRACE_RULES) != RAY_NONE;
    // (The first blocker the ray found, not always the nearest: the filter takes the mean of
    // those around for the width of the penumbra.)
    let went = select(1.0, sqrt(saturate(ray_found_t / ray_scene.sun.y)), stopped);
    let flags = SUN_TRACED | select(0u, SUN_PLANTS_BY_RAY, plants);
    textureStore(sun_raw, pixel, vec4<f32>(select(1.0, 0.0, stopped), f32(flags) / 255.0, went, 1.0));
    var counts = vec4<u32>(0u);
    if (counting) {
        counts.x = 1u;
        counts.y = select(0u, 1u, stopped);
        // The still image looks along the sun's center: a ray along it is what it is
        // compared with (the disc's ray differs from it in every penumbra).
        let verdict = still_image_verdict(rel, sunlit_side(n_shade), ray_scene.sun.y, plants);
        if (verdict == STILL_BLOCKED || verdict == STILL_OPEN) {
            let center = ray_stopped(origin, l, ray_scene.sun.y, classes, SUN_TRACE_RULES) != RAY_NONE;
            counts.z = select(0u, 1u, verdict == STILL_BLOCKED && !center);
            counts.w = select(0u, 1u, verdict == STILL_OPEN && center);
        }
    }
    return counts;
}

@compute @workgroup_size(8, 8, 1)
fn cs_sun_trace(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) thread: u32) {
    let counting = shade_skip(COUNT_SUN_TRACE);
    if (thread < 4u) { atomicStore(&group_counts[thread], 0u); }
    workgroupBarrier();
    var counts = vec4<u32>(0u);
    if (all(id.xy < textureDimensions(sun_raw))) {
        counts = sun_trace_pixel(id.xy, counting);
    }
    if (counting) {
        if (counts.x > 0u) { atomicAdd(&group_counts[0], counts.x); }
        if (counts.y > 0u) { atomicAdd(&group_counts[1], counts.y); }
        if (counts.z > 0u) { atomicAdd(&group_counts[2], counts.z); }
        if (counts.w > 0u) { atomicAdd(&group_counts[3], counts.w); }
    }
    workgroupBarrier();
    if (counting && thread < 4u) {
        let n = atomicLoad(&group_counts[thread]);
        if (n > 0u) { atomicAdd(&sun_trace_counts[thread], n); }
    }
}
