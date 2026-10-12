// Smoke shadows: darkens the sunlit scene under and behind a smoke volume. Drawn over the scene
// (multiplied) before the smoke itself. For every scene pixel, the volume's light texture says
// how much smoke lies between that point and the sun.

struct SmokeParams {
    min_rel: vec4<f32>,   // box min (camera-relative), voxel size
    size: vec4<f32>,      // box size (m), blend previous -> current
    dims: vec4<f32>,      // voxels, firelight scale
    look: vec4<f32>,      // detail, max steps, tunnel count, unused
    tunnels: array<vec4<f32>, 32>,
};

@group(1) @binding(0) var<uniform> smoke: SmokeParams;
@group(1) @binding(3) var light_tex: texture_3d<f32>;
@group(1) @binding(5) var scene_depth: texture_depth_2d;

// How far below the box's floor shadows still fall (hillsides, ditches), m.
const SHADOW_DROP: f32 = 30.0;
// The longest a shadow stretches away from a low sun, m. Keep in step with render.rs.
const SHADOW_REACH: f32 = 120.0;

struct Bounds {
    lo: vec3<f32>,
    hi: vec3<f32>,
};

// The box and everything it can shade: swept away from the sun and down past its floor.
fn shadow_bounds() -> Bounds {
    let lo = smoke.min_rel.xyz;
    let hi = lo + smoke.size.xyz;
    let sun = frame.sun_dir.xyz;
    let reach = min((smoke.size.y + SHADOW_DROP) / max(sun.y, 0.05), SHADOW_REACH);
    let away = -sun * reach;
    return Bounds(min(lo, lo + away), max(hi, hi + away));
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
};

// Corner of the unit box for vertex `i` of 36 (6 faces x 2 triangles).
fn box_corner(i: u32) -> vec3<f32> {
    var quad = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
        vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0),
    );
    let face = i / 6u;
    let c = quad[i % 6u];
    let side = f32(face % 2u);
    let axis = face / 2u;
    if (axis == 0u) { return vec3(side, c.x, c.y); }
    if (axis == 1u) { return vec3(c.x, side, c.y); }
    return vec3(c.x, c.y, side);
}

@vertex
fn vs_shadow(@builtin(vertex_index) i: u32) -> VsOut {
    let b = shadow_bounds();
    let rel = b.lo + box_corner(i) * (b.hi - b.lo);
    var out: VsOut;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.rel = rel;
    return out;
}

// Where the ray o + t·d enters and leaves the box lo–hi (t_near > t_far: a miss).
fn hit_box(o: vec3<f32>, d: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> vec2<f32> {
    let safe = select(d, vec3(1e-6), abs(d) < vec3(1e-6));
    let ta = (lo - o) / safe;
    let tb = (hi - o) / safe;
    let tn = min(ta, tb);
    let tf = max(ta, tb);
    return vec2<f32>(max(max(tn.x, tn.y), tn.z), min(min(tf.x, tf.y), tf.z));
}

// One tap of the sun's shadow map: whether opaque things already shade this point.
fn sun_visible(rel: vec3<f32>) -> f32 {
    if (frame.sun_color.w < 0.5) { return 1.0; }
    let depth = view_depth(rel);
    if (depth > frame.shadow_splits[3]) { return 1.0; }
    var cascade = 0u;
    for (var i = 0u; i < 3u; i++) {
        if (depth > frame.shadow_splits[i]) { cascade = i + 1u; }
    }
    let lp = frame.shadow_cascades[cascade] * vec4<f32>(rel, 1.0);
    let uv = lp.xy * vec2<f32>(0.5, -0.5) + 0.5;
    let z = lp.z - frame.shadow_params.x * 2.0;
    if (any(uv < vec2(0.0)) || any(uv > vec2(1.0)) || z > 1.0) { return 1.0; }
    return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, cascade, z);
}

@fragment
fn fs_shadow(in: VsOut) -> @location(0) vec4<f32> {
    let sun = frame.sun_dir.xyz;
    if (sun.y <= 0.0 || dot(frame.sun_camera.rgb, frame.sun_camera.rgb) <= 0.0) { discard; }
    // Each pixel once: on the far side of the bounds (also from inside them).
    let face_dist = length(in.rel);
    let dir = in.rel / max(face_dist, 1e-4);
    let b = shadow_bounds();
    let span = hit_box(vec3(0.0), dir, b.lo, b.hi);
    if (face_dist < span.y * 0.999 - 0.01) { discard; }

    // The scene point under this pixel.
    let d = textureLoad(scene_depth, vec2<i32>(in.clip.xy), 0);
    if (d <= 0.0) { discard; }
    let forward = -(frame.view * vec4<f32>(dir, 0.0)).z;
    let p = dir * (frame.exposure.z / d / max(forward, 1e-4));

    // Its ray to the sun through the smoke box.
    let lo = smoke.min_rel.xyz;
    let hit = hit_box(p, sun, lo, lo + smoke.size.xyz);
    if (hit.y <= max(hit.x, 0.0)) { discard; }
    // The light texture holds each voxel's optical depth toward the sun; look it up half a
    // voxel inside where the ray enters (or at the point itself, inside the smoke).
    let q = p + sun * (max(hit.x, 0.0) + smoke.min_rel.w * 0.5);
    let uvw = saturate3((q - lo) / smoke.size.xyz);
    let tau = textureSampleLevel(light_tex, linear_clamp, uvw, 0.0).x;
    let blocked = 1.0 - exp(-tau);
    if (blocked < 0.004) { discard; }

    // Only the sunlight goes: the sun's share of an upward-facing surface's light, where
    // nothing opaque shades it already.
    let e_sun = luminance(frame.sun_camera.rgb) * sun.y * INV_PI;
    let e_sky = luminance(sky_ambient(vec3<f32>(0.0, 1.0, 0.0)));
    let share = e_sun / max(e_sun + e_sky, 1e-6) * sun_visible(p);
    // Fog between the camera and the point isn't shaded by the smoke.
    let fog_t = luminance(apply_fog(vec3(1.0), p) - apply_fog(vec3(0.0), p));
    let k = 1.0 - blocked * share * saturate(fog_t);
    return vec4<f32>(vec3<f32>(k), 1.0);
}
