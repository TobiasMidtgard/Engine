// Falling rain and snow around the camera, splashes where drops land, and lightning bolts.
// Everything is generated from the instance index: no particle buffers. Drops are anchored in
// the world (they don't move with the camera) and wrap around a box centered on it. Drops
// under a roof, a tree or an overhang are removed using the renderer's top-down sky
// occlusion map. Requires lib_types + lib_frame.

struct Precip {
    area: vec4<f32>,         // box size xyz (m), streak shutter time (s)
    wrap: array<vec4<f32>, 3>, // per speed class: (fall offset - camera) wrapped into the box xyz, speed factor
    velocity: vec4<f32>,     // fall velocity incl. wind xyz (m/s), drop width (m)
    look: vec4<f32>,         // rain alpha, snow alpha, splash life (s), time (s, wrapped)
    splash: vec4<f32>,       // splash radius around the camera (m), splash size (m), snow flake size (m), unused
    bolt: vec4<f32>,         // bolt color × luminance (cd/m²) rgb, cloud base altitude (m)
};

@group(1) @binding(0) var<uniform> precip: Precip;
// Bolt segments as pairs: (a.xyz camera-relative, width m), (b.xyz, brightness).
@group(1) @binding(1) var<storage, read> segments: array<vec4<f32>>;

const SPEED_CLASSES: u32 = 3u;

fn pcg3(v0: vec3<u32>) -> vec3<f32> {
    var v = v0 * 1664525u + vec3<u32>(1013904223u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v = v ^ (v >> vec3<u32>(16u));
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return vec3<f32>(v) * (1.0 / 4294967296.0);
}

// True when something solid is above `rel` (roof, canopy, overhang) in the sky occlusion map.
fn covered(rel: vec3<f32>) -> bool {
    if (frame.sky_occlusion_params.x < 0.5) { return false; }
    let c = frame.sky_occlusion * vec4<f32>(rel, 1.0);
    if (max(abs(c.x), abs(c.y)) >= 1.0 || c.z <= 0.0 || c.z >= 1.0) { return false; }
    let uv = c.xy * vec2<f32>(0.5, -0.5) + 0.5;
    let size = vec2<f32>(textureDimensions(sky_occlusion_map));
    let top = textureLoad(sky_occlusion_map, vec2<i32>(uv * size), 0);
    return (c.z - top) * frame.sky_occlusion_params.w > 0.15;
}

// Height (camera-relative y) of the highest surface at camera-relative x/z, if known.
fn surface_height(xz: vec2<f32>) -> vec2<f32> {
    if (frame.sky_occlusion_params.x < 0.5) {
        return vec2<f32>(-frame.camera_world.y, 1.0);
    }
    let c = frame.sky_occlusion * vec4<f32>(xz.x, 0.0, xz.y, 1.0);
    if (max(abs(c.x), abs(c.y)) >= 0.95) { return vec2<f32>(0.0, 0.0); }
    let uv = c.xy * vec2<f32>(0.5, -0.5) + 0.5;
    let size = vec2<f32>(textureDimensions(sky_occlusion_map));
    let top = textureLoad(sky_occlusion_map, vec2<i32>(uv * size), 0);
    if (top >= 1.0) { return vec2<f32>(0.0, 0.0); }
    return vec2<f32>((c.z - top) * frame.sky_occlusion_params.w, 1.0);
}

// One pixel's angular size (radians) at the screen center.
fn pixel_angle() -> f32 {
    return 2.0 / (frame.proj[1][1] * frame.viewport.y);
}

fn corner(v: u32) -> vec2<f32> {
    // Two triangles: (0,0) (1,0) (0,1) / (0,1) (1,0) (1,1); x in -1..1, y in 0..1.
    var c = array<vec2<f32>, 6>(
        vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    return c[v % 6u];
}

struct PrecipOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,   // pre-exposed radiance, alpha
    @location(2) misc: vec4<f32>,    // splash: phase
};

fn culled() -> PrecipOut {
    var out: PrecipOut;
    out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    out.uv = vec2(0.0);
    out.color = vec4(0.0);
    out.misc = vec4(0.0);
    return out;
}

fn drop_position(i: u32, h: vec3<f32>) -> vec3<f32> {
    let w = precip.wrap[i % SPEED_CLASSES];
    let size = precip.area.xyz;
    return (fract(h + w.xyz / size) - 0.5) * size;
}

// Fade out near the eye and toward the sides of the box.
fn box_fade(rel: vec3<f32>) -> f32 {
    let half = precip.area.xyz * 0.5;
    let edge = max(abs(rel.x) / half.x, abs(rel.z) / half.z);
    let vertical = abs(rel.y) / half.y;
    return smoothstep(0.25, 0.6, length(rel)) * (1.0 - smoothstep(0.7, 1.0, max(edge, vertical)));
}

// Light a raindrop sees: mostly the sky and surroundings it refracts, glints of lamps.
fn drop_light(rel: vec3<f32>, view: vec3<f32>) -> vec3<f32> {
    var c = sample_env(vec3<f32>(0.0, 1.0, 0.0)) * 0.5 + sample_env(normalize(vec3<f32>(view.x, 0.2, view.z))) * 0.5;
    c += frame.sun_camera.rgb * INV_PI * 0.05 * pow(saturate(dot(view, frame.sun_dir.xyz)), 16.0);
    for (var i = 0u; i < min(frame.counts.x, 16u); i++) {
        let l = point_lights[i];
        let d = l.position_range.xyz - rel;
        let d2 = dot(d, d);
        let range = l.position_range.w;
        if (d2 < range * range) {
            // Drops glow strongly when looking toward the lamp through them.
            let forward = 0.3 + 2.5 * pow(saturate(dot(view, d * inverseSqrt(d2))), 8.0);
            let falloff = sq(saturate(1.0 - sq(d2 / (range * range))));
            c += l.color_radius.rgb * forward * falloff / max(d2, 0.25) * INV_PI * 0.15;
        }
    }
    return c;
}

@vertex
fn vs_rain(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> PrecipOut {
    let h = pcg3(vec3<u32>(i, i * 7u + 3u, 911u));
    let rel = drop_position(i, h);
    let fade = box_fade(rel);
    if (fade <= 0.0 || covered(rel)) { return culled(); }
    let speed = precip.wrap[i % SPEED_CLASSES].w;
    let vel = precip.velocity.xyz * vec3<f32>(1.0, speed, 1.0);
    let len = length(vel) * precip.area.w + precip.velocity.w;
    let axis = normalize(vel);
    let dist = length(rel);
    let to_eye = -rel / max(dist, 1e-3);
    var side = cross(axis, to_eye);
    let side_len = length(side);
    if (side_len < 1e-3) { return culled(); }
    side /= side_len;
    // At least ~1.2 pixels wide; thinner drops become more transparent instead.
    let width = max(precip.velocity.w, dist * pixel_angle() * 1.2);
    let coverage = sqrt(precip.velocity.w / width);
    let c = corner(v);
    let p = rel - axis * len * c.y + side * width * 0.5 * c.x;
    var out: PrecipOut;
    out.misc = vec4(0.0);
    out.position = frame.view_proj * vec4<f32>(p, 1.0);
    out.uv = c;
    let light = drop_light(rel, -to_eye);
    out.color = vec4<f32>(pre_expose(light), precip.look.x * fade * coverage * (0.6 + 0.8 * h.y));
    return out;
}

@fragment
fn fs_rain(in: PrecipOut) -> @location(0) vec4<f32> {
    let across = 1.0 - in.uv.x * in.uv.x;
    let along = smoothstep(0.0, 0.25, in.uv.y) * smoothstep(1.0, 0.6, in.uv.y);
    let a = saturate(in.color.a * across * along);
    return vec4<f32>(min(in.color.rgb, vec3(60000.0)) * a, a);
}

@vertex
fn vs_snow(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> PrecipOut {
    let h = pcg3(vec3<u32>(i, i * 13u + 5u, 577u));
    var rel = drop_position(i, h);
    // Flakes flutter as they fall.
    let t = precip.look.w;
    rel += vec3<f32>(sin(t * (1.1 + h.x) + h.y * TAU), 0.0, cos(t * (0.9 + h.z) + h.x * TAU)) * 0.22;
    let fade = box_fade(rel);
    if (fade <= 0.0 || covered(rel)) { return culled(); }
    let dist = length(rel);
    let to_eye = -rel / max(dist, 1e-3);
    let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), to_eye) + vec3(1e-4, 0.0, 0.0));
    let up = cross(to_eye, right);
    let size = precip.splash.z * (0.6 + 0.8 * h.y);
    // At least ~1.2 pixels wide; flakes smaller than that fade instead of shimmering.
    let width = max(size, dist * pixel_angle() * 1.2);
    let coverage = size / width;
    let c = corner(v);
    let p = rel + (right * c.x + up * (c.y * 2.0 - 1.0)) * width * 0.5;
    var out: PrecipOut;
    out.misc = vec4(0.0);
    out.position = frame.view_proj * vec4<f32>(p, 1.0);
    out.uv = vec2<f32>(c.x, c.y * 2.0 - 1.0);
    // White, lit by sky and sun (with shadows) like any rough surface. Flakes are thin lace
    // that lets part of the light behind them through, so against a bright overcast they
    // read as soft grey specks, not as dark ash.
    let n = normalize(to_eye + vec3<f32>(0.0, 0.6, 0.0));
    let sun = frame.sun_camera.rgb * saturate(dot(n, frame.sun_dir.xyz) * 0.5 + 0.5) * shadow_factor(rel, n) * INV_PI;
    let behind = sample_env(-to_eye);
    let light = (sky_ambient(n) + sun) * 0.9 * 0.65 + behind * 0.4;
    out.color = vec4<f32>(pre_expose(light), precip.look.y * fade * coverage);
    return out;
}

@fragment
fn fs_snow(in: PrecipOut) -> @location(0) vec4<f32> {
    let r = length(in.uv);
    let a = saturate(in.color.a * smoothstep(1.0, 0.35, r));
    return vec4<f32>(min(in.color.rgb, vec3(60000.0)) * a, a);
}

// Splashes: little crowns of droplets where rain lands, on the ground and on roofs.
@vertex
fn vs_splash(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> PrecipOut {
    let h0 = pcg3(vec3<u32>(i, 71u, 3u));
    let life = precip.look.z;
    let t = precip.look.w / life + h0.x;
    let cycle = u32(floor(t));
    let phase = fract(t);
    let h = pcg3(vec3<u32>(i, cycle, 19u));
    let radius = precip.splash.x;
    let xz = (h.xy * 2.0 - 1.0) * radius;
    let ground = surface_height(xz);
    if (ground.y < 0.5) { return culled(); }
    let rel = vec3<f32>(xz.x, ground.x + 0.01, xz.y);
    let dist = length(rel);
    if (dist > radius) { return culled(); }
    let to_eye = -rel / max(dist, 1e-3);
    let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), to_eye) + vec3(1e-4, 0.0, 0.0));
    let size = precip.splash.y * (0.7 + 0.6 * h.z);
    let c = corner(v);
    let p = rel + right * c.x * size + vec3<f32>(0.0, c.y * size, 0.0);
    var out: PrecipOut;
    out.misc = vec4(0.0);
    out.position = frame.view_proj * vec4<f32>(p, 1.0);
    out.uv = vec2<f32>(c.x, c.y);
    let light = drop_light(rel, -to_eye);
    let fade = smoothstep(radius, radius * 0.6, dist) * smoothstep(0.3, 1.0, dist);
    out.color = vec4<f32>(pre_expose(light) * 0.9, precip.look.x * fade * 1.2);
    out.misc = vec4<f32>(phase, 0.0, 0.0, 0.0);
    return out;
}

@fragment
fn fs_splash(in: PrecipOut) -> @location(0) vec4<f32> {
    let phase = in.misc.x;
    let y = in.uv.y;
    var a = 0.0;
    // Five droplets thrown up and out on parabolas.
    for (var k = 0; k < 5; k++) {
        let s = (f32(k) - 2.0) / 2.0;
        let dx = s * 0.8 * phase;
        let dy = 3.2 * phase * (1.0 - phase) * (1.0 - 0.5 * abs(s));
        let d = length(vec2<f32>(in.uv.x - dx, (y - dy) * 2.0));
        a += smoothstep(0.16, 0.04, d);
    }
    // A flat ring at the start.
    let ring = smoothstep(0.12, 0.0, abs(length(vec2<f32>(in.uv.x, y * 4.0)) - phase * 1.2)) * (1.0 - phase);
    a = saturate((a + ring) * in.color.a * (1.0 - phase));
    return vec4<f32>(min(in.color.rgb, vec3(60000.0)) * a, a);
}

// Lightning: blinding channel with a soft halo, fading inside the cloud.
@vertex
fn vs_bolt(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> PrecipOut {
    let a = segments[i * 2u];
    let b = segments[i * 2u + 1u];
    let c = corner(v);
    let p0 = a.xyz;
    let p1 = b.xyz;
    let mid = mix(p0, p1, c.y);
    let dist = length(mid);
    let axis = normalize(p1 - p0 + vec3(1e-5));
    let side = normalize(cross(axis, -mid / max(dist, 1e-3)) + vec3(1e-5));
    let core = max(a.w, dist * pixel_angle() * 1.5);
    let p = mid + side * core * c.x;
    var out: PrecipOut;
    out.misc = vec4(0.0);
    out.position = frame.view_proj * vec4<f32>(p, 1.0);
    out.uv = c;
    let altitude = frame.camera_world.w + mid.y;
    let in_cloud = exp(-max(altitude - precip.bolt.w, 0.0) / 250.0);
    let haze = exp(-dist / 30000.0);
    // Display-referred cap: blinding white, but bloom stays a glow instead of a wall of light.
    out.color = vec4<f32>(min(pre_expose(precip.bolt.rgb * b.w * in_cloud * haze), vec3(40.0)), 1.0);
    return out;
}

@fragment
fn fs_bolt(in: PrecipOut) -> @location(0) vec4<f32> {
    // A hot core with soft edges; the renderer's bloom adds the glow around it.
    let core = smoothstep(1.0, 0.2, abs(in.uv.x));
    return vec4<f32>(min(in.color.rgb, vec3(60000.0)) * core, 0.0);
}
