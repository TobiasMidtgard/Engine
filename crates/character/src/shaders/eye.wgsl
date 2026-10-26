// Eyes. Requires lib_types + lib_frame + common.wgsl.
//
// Everything is worked out in the eye's own frame (+Z along the gaze, unit radius). The
// sclera is a soft off-white with faint vessels toward the corners. Inside the cornea the
// view ray is refracted (index 1.376) down to the iris, a plane a little behind the cornea's
// base, so the iris shifts with the view and the pupil looks deep like a real eye's. The iris
// is drawn from its anatomy: radial fibers and crypts, a collarette ring around the pupil,
// darker toward the rim, colored by melanin (blue at none, through green and hazel, to dark
// brown) and lipochrome (yellow-green). The cornea is a near-mirror coat that reflects the sky
// and the sun as sharp catch-lights; the sclera is wet but rougher.

struct EyeIn {
    @location(0) position: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) local: vec4<f32>,
};

struct EyeOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) local: vec3<f32>,
    @location(3) side: f32,
    @location(4) clip_now: vec4<f32>,
    @location(5) clip_prev: vec4<f32>,
};

const CORNEA_ANGLE: f32 = 0.5;
const IRIS_RADIUS: f32 = 0.47;    // sin of the cornea angle: where the iris meets the sclera
const IRIS_DEPTH: f32 = 0.84;     // eye-local z of the iris plane
const CORNEA_IOR: f32 = 1.376;

// Turns the eye about its center to look `yaw` toward the person's left and `pitch` up.
fn gaze_turn(p: vec3<f32>, center: vec3<f32>, yaw: f32, pitch: f32, dir: bool) -> vec3<f32> {
    var d = select(p - center, p, dir);
    // Up first (about the eye's left-right axis), then aside (about the head's up).
    d = vec3<f32>(d.x, d.y * cos(pitch) + d.z * sin(pitch), -d.y * sin(pitch) + d.z * cos(pitch));
    d = vec3<f32>(d.x * cos(yaw) + d.z * sin(yaw), d.y, -d.x * sin(yaw) + d.z * cos(yaw));
    return select(center + d, d, dir);
}

// An axis of the eye's own frame in camera-relative space: turned by the gaze, then by the
// head (posed).
fn eye_axis(a: vec3<f32>) -> vec3<f32> {
    let head = vec4<u32>(pose.info.z, 0u, 0u, 0u);
    let one = vec4<f32>(1.0, 0.0, 0.0, 0.0);
    let d = gaze_turn(a, vec3<f32>(0.0), pose.gaze.x, pose.gaze.y, true);
    return model_dir((skin_matrix(head, one, false) * vec4<f32>(d, 0.0)).xyz);
}

// An axis of the head's frame in camera-relative space (the eye's frame before the gaze).
fn head_axis(a: vec3<f32>) -> vec3<f32> {
    let head = vec4<u32>(pose.info.z, 0u, 0u, 0u);
    let one = vec4<f32>(1.0, 0.0, 0.0, 0.0);
    return model_dir((skin_matrix(head, one, false) * vec4<f32>(a, 0.0)).xyz);
}

// The eye sits deep in its socket under the lids and the brow, so outdoors it sees much less
// of the sky than the cheek does and the upper lid shades it from a high sun. `h`: the point
// in the head's frame (unit eye radius), `sun`: toward the sun in the head's frame. Returns
// (sky seen, sun reaching).
fn socket_light(h: vec3<f32>, sun: vec3<f32>) -> vec2<f32> {
    // Under the upper lid, and into the corners.
    let lid = smoothstep(0.05, 0.55, h.y) * 0.65 + smoothstep(0.5, 0.95, abs(h.x)) * 0.4;
    // Brow, lids and nose hide much of the sky even in the middle of the eye.
    let sky = (1.0 - lid) * 0.6;
    // The upper lid's edge (with the lashes) stands about a third of a radius out from the eye:
    // the higher the sun, the lower its shadow reaches.
    let rise = max(sun.y, 0.0) / max(sun.z, 0.15);
    let edge = 0.4 - 0.35 * rise;
    let shade = smoothstep(edge - 0.1, edge + 0.1, h.y);
    return vec2<f32>(sky, 1.0 - 0.85 * shade);
}

// How much of the room's light reaches a point `h` of the eye (head frame, unit radius) past
// the lids: the upper lid with its lashes shades a broad band below its edge, the lower lid a
// narrow one, and the corners, where both meet, lie deep in shade. Without this the white of
// the eye is lit as brightly as the open cheek and glows.
fn lid_shade(h: vec3<f32>, side: f32) -> f32 {
    let i = select(2u, 0u, side > 0.0);
    let up = character.lids[i];
    let lo = character.lids[i + 1u];
    if (up.x == 0.0 && lo.x == 0.0) {
        return 1.0;
    }
    let x = h.x;
    let below_upper = up.x + up.y * x + up.z * x * x - h.y;
    let above_lower = h.y - (lo.x + lo.y * x + lo.z * x * x);
    return mix(0.3, 1.0, smoothstep(-0.02, 0.38, below_upper))
        * mix(0.55, 1.0, smoothstep(-0.02, 0.16, above_lower));
}

@vertex
fn vs_eye(in: EyeIn) -> EyeOut {
    var out: EyeOut;
    // The base mesh's left eye is on +X.
    let center = select(pose.eye_r.xyz, pose.eye_l.xyz, in.position.x > 0.0);
    let head = vec4<u32>(pose.info.z, 0u, 0u, 0u);
    let one = vec4<f32>(1.0, 0.0, 0.0, 0.0);
    let p = (skin_matrix(head, one, false)
        * vec4<f32>(gaze_turn(in.position.xyz, center, pose.gaze.x, pose.gaze.y, false), 1.0)).xyz;
    let n = (skin_matrix(head, one, false)
        * vec4<f32>(gaze_turn(in.normal.xyz, center, pose.gaze.x, pose.gaze.y, true), 0.0)).xyz;
    let p_prev = (skin_matrix(head, one, true)
        * vec4<f32>(gaze_turn(in.position.xyz, center, pose.gaze.z, pose.gaze.w, false), 1.0)).xyz;
    let rel = model_point(p);
    out.rel = rel;
    out.normal = model_dir(n);
    out.local = in.local.xyz;
    out.side = sign(in.position.x);
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.clip_now = motion_clip(rel);
    out.clip_prev = prev_motion_clip(prev_model_point(p_prev));
    return out;
}

// Iris color for melanin `m` (0..1) and lipochrome `y` (0..1) at radius `t` (0 pupil .. 1 rim).
fn iris_base(m: f32, y: f32) -> vec3<f32> {
    // Blue (scattering, no pigment) -> grey-green -> hazel -> brown -> dark brown.
    let blue = vec3<f32>(0.20, 0.33, 0.52);
    let green = vec3<f32>(0.22, 0.30, 0.15);
    let hazel = vec3<f32>(0.32, 0.20, 0.07);
    let brown = vec3<f32>(0.13, 0.055, 0.02);
    let dark = vec3<f32>(0.035, 0.016, 0.008);
    var c = mix(blue, green, smoothstep(0.1, 0.4, m));
    c = mix(c, hazel, smoothstep(0.35, 0.6, m));
    c = mix(c, brown, smoothstep(0.55, 0.8, m));
    c = mix(c, dark, smoothstep(0.8, 1.0, m));
    return mix(c, c * vec3<f32>(1.25, 1.15, 0.45) + vec3<f32>(0.04, 0.03, 0.0), y * (1.0 - 0.6 * m));
}

fn iris_color(uv: vec2<f32>, seed: f32) -> vec3<f32> {
    let rad = length(uv);
    let t = rad / IRIS_RADIUS;
    let ang = atan2(uv.y, uv.x);
    let m = character.eye.x;
    let y = character.eye.y;
    let pupil = mix(0.22, 0.5, character.eye.w);
    if (t < pupil) {
        return vec3<f32>(0.004);
    }
    let u = (t - pupil) / (1.0 - pupil);
    // Radial fibers, broken up along their length; crypts between them.
    let fiber = value_noise2(vec2<f32>(ang * 38.0, u * 3.0 + seed)) * 0.6 + value_noise2(vec2<f32>(ang * 90.0, u * 8.0 + seed)) * 0.4;
    let crypt = smoothstep(0.62, 0.8, value_noise2(vec2<f32>(ang * 14.0 + seed, u * 6.0)));
    var c = iris_base(m, y);
    // Brighter fibers, darker crypts.
    c *= 0.65 + 0.7 * fiber;
    c *= 1.0 - 0.45 * crypt * smoothstep(0.25, 0.4, u);
    // Collarette: a lighter, warmer ring about a third of the way out (pigment gathers there).
    let collar = exp(-sq((u - 0.3 + 0.03 * sin(ang * 7.0 + seed)) / 0.06));
    c = mix(c, c * vec3<f32>(1.5, 1.3, 0.8) + vec3<f32>(0.03, 0.02, 0.0) * (1.0 - m), collar * 0.6);
    // Pupil edge (the pigment ruff) and the darker limbal ring at the rim.
    c *= mix(0.35, 1.0, smoothstep(0.0, 0.06, u));
    let limbal = character.eye.z;
    c *= 1.0 - limbal * 0.85 * smoothstep(0.78, 0.98, u);
    return c;
}

fn sclera_color(local: vec3<f32>, seed: f32) -> vec3<f32> {
    // Off-white: a sclera is never paper white (and reads as glowing if it is).
    var c = vec3<f32>(0.6, 0.56, 0.52);
    // Warmer and greyer toward the back and the corners.
    let side = abs(local.x);
    c *= mix(1.0, 0.82, smoothstep(0.4, 0.95, side));
    // Vessels: thin branching lines toward the corners.
    let vessels = pow(1.0 - abs(value_noise2(local.xy * vec2<f32>(14.0, 40.0) + seed) * 2.0 - 1.0), 18.0)
        + pow(1.0 - abs(value_noise2(local.xy * vec2<f32>(30.0, 70.0) + seed * 3.0) * 2.0 - 1.0), 26.0) * 0.6;
    let redness = character.eye2.x;
    let where_ = smoothstep(0.45, 0.85, side);
    c = mix(c, vec3<f32>(0.55, 0.12, 0.10), saturate(vessels * where_ * (0.25 + redness)));
    c = mix(c, c * vec3<f32>(1.0, 0.82, 0.8), redness * where_ * 0.5);
    return c;
}

@fragment
fn fs_eye(in: EyeOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let local = normalize(in.local);
    let theta = acos(clamp(local.z, -1.0, 1.0));
    let n = normalize(in.normal);
    let v = normalize(-in.rel);
    let seed = character.look.w * 113.0 + in.side * 7.0;

    // The eye's local frame in camera-relative space (it turns with the gaze and the head).
    let ex = eye_axis(vec3<f32>(1.0, 0.0, 0.0));
    let ey = eye_axis(vec3<f32>(0.0, 1.0, 0.0));
    let ez = eye_axis(vec3<f32>(0.0, 0.0, 1.0));
    let v_local = normalize(vec3<f32>(dot(v, ex), dot(v, ey), dot(v, ez)));
    let n_local = normalize(vec3<f32>(dot(n, ex), dot(n, ey), dot(n, ez)));

    let in_cornea = smoothstep(CORNEA_ANGLE + 0.02, CORNEA_ANGLE - 0.03, theta);
    var albedo = sclera_color(local, seed);
    var under_n = n;
    if (in_cornea > 0.0) {
        // Refract into the cornea and find the iris plane.
        let refr = refract(-v_local, n_local, 1.0 / CORNEA_IOR);
        let p = local * (1.0 + 0.06 * in_cornea);
        let t = (IRIS_DEPTH - p.z) / min(refr.z, -0.05);
        let hit = p + refr * t;
        let iris = iris_color(hit.xy, seed);
        // The iris is a little concave; light it as facing forward.
        albedo = mix(albedo, iris, in_cornea * smoothstep(IRIS_RADIUS + 0.01, IRIS_RADIUS - 0.02, length(hit.xy)));
        under_n = normalize(mix(n, ez, in_cornea * 0.7));
    }
    // Shadow of the socket, the lids and lashes over the eye, worked out in the head's frame
    // (the lids don't turn with the gaze).
    let h = gaze_turn(local, vec3<f32>(0.0), pose.gaze.x, pose.gaze.y, true);
    let sun = frame.sun_dir.xyz;
    let socket = socket_light(
        h,
        vec3<f32>(
            dot(sun, head_axis(vec3<f32>(1.0, 0.0, 0.0))),
            dot(sun, head_axis(vec3<f32>(0.0, 1.0, 0.0))),
            dot(sun, head_axis(vec3<f32>(0.0, 0.0, 1.0))),
        ),
    );
    var s = default_surface();
    s.albedo = albedo;
    s.normal = under_n;
    s.roughness = 0.35;
    s.reflectance = 0.0;
    s.occlusion = socket.x;
    s.subsurface = 0.25;
    let lids = lid_shade(h, in.side);
    var color = shade_surface_shadowed(s, in.rel, socket.y) * lids;
    // The cornea and the tear film: a smooth coat over everything.
    var coat = default_surface();
    coat.albedo = vec3<f32>(0.0);
    coat.normal = n;
    coat.roughness = mix(0.16, 0.05, in_cornea);
    // F0 of the tear film over the sclera (water, 2%) and of the cornea (index 1.376, 2.5%):
    // any stronger and a bright sky veils the iris grey outdoors.
    coat.reflectance = mix(0.35, 0.4, in_cornea);
    coat.occlusion = socket.x;
    color += shade_surface_shadowed(coat, in.rel, socket.y) * mix(0.4, 1.0, lids);
    return vec4<f32>(apply_fog(color, in.rel), 1.0);
}

@fragment
fn fs_eye_prepass(in: EyeOut) -> PrepassOutput {
    return prepass_output(normalize(in.normal), 0.1, in.clip_now, in.clip_prev);
}
