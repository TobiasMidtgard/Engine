// ============================================================================
// Engine shader library: frame bindings (group 0) and lighting.
// Requires lib_types.wgsl. Group 0 is bound by the renderer before any feature draws.
// ============================================================================

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> point_lights: array<PointLight>;
@group(0) @binding(2) var shadow_map: texture_depth_2d_array;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;
@group(0) @binding(4) var linear_clamp: sampler;
// Equirect sky radiance (cd/m^2, not pre-exposed) prefiltered for reflections: mip 0 is the sharp
// sky, the last mip what a fully rough surface reflects (`sample_env`, `sample_env_rough`).
@group(0) @binding(5) var env_map: texture_2d<f32>;
@group(0) @binding(6) var linear_repeat: sampler;
@group(0) @binding(7) var sky_occlusion_map: texture_depth_2d; // top-down depth of the highest surface (0 = top)
@group(0) @binding(8) var sky_occlusion_sampler: sampler;      // nearest, for textureGather on the map
@group(0) @binding(9) var<storage, read> sky_camera: array<vec4<f32>>; // [0].x: sky visibility at the camera
// Screen-space results of the depth prepass (see `RenderFeature::draw_prepass`). In the prepass
// itself and in passes that write them, these hold neutral 1x1 stand-ins.
@group(0) @binding(10) var ao_map: texture_2d<f32>;          // ambient occlusion (R, 1 = open)
@group(0) @binding(11) var prepass_normal: texture_2d<f32>;  // octahedral world normal xy, perceptual roughness, 1 where written
@group(0) @binding(12) var prepass_depth: texture_depth_2d;  // depth after the prepass (reverse-Z, 0 = nothing)
// Atmosphere look-up tables (see lib_atmosphere.wgsl).
@group(0) @binding(13) var transmittance_lut: texture_2d<f32>; // sunlight transmittance to the top of the atmosphere
@group(0) @binding(14) var multiscatter_lut: texture_2d<f32>;  // light from all higher orders of scattering
@group(0) @binding(15) var sky_view_lut: texture_2d<f32>;      // the sky seen from the camera (this frame)
// Sky light for diffuse surfaces: cosine-weighted sky radiance around each normal (equirect).
@group(0) @binding(16) var env_irradiance: texture_2d<f32>;
@group(0) @binding(17) var env_sampler: sampler; // equirect maps: wraps around, clamps at the poles
// Screen-space reflections: reflected radiance (cd/m^2, not pre-exposed), coverage (see
// `screen_reflection`). A stand-in while it is being drawn.
@group(0) @binding(18) var ssr_map: texture_2d<f32>;
// Volumetric fog (volumetrics.rs): light scattered toward the camera (pre-exposed) and
// transmittance from the camera to the far edge of each depth slice of the froxel grid (see
// `volumetric_fog_at`). A clear stand-in while it is being computed and when it is off.
@group(0) @binding(19) var volumetric_fog: texture_3d<f32>;
// Screen-space global illumination: light bounced onto each pixel from nearby surfaces
// (cd/m^2, not pre-exposed; see `screen_gi`). A stand-in while it is being drawn.
@group(0) @binding(20) var gi_map: texture_2d<f32>;
// Cloud shadows (cloud_shadows.rs): r = sunlight let through by the clouds to the cloud base,
// top-down around the camera (see `cloud_shadow`); g, b, a are free for the module that draws
// it. 1 where nobody draws it, and a white stand-in during `RenderFeature::compute`.
@group(0) @binding(21) var cloud_shadow_map: texture_2d<f32>;

// Multiply final radiance by this before writing to the HDR target.
fn pre_expose(c: vec3<f32>) -> vec3<f32> { return c * frame.exposure.x; }

// Approximate world position (f32). Use camera-relative positions for precision-sensitive work.
fn world_pos(rel: vec3<f32>) -> vec3<f32> { return rel + frame.camera_world.xyz; }

// Stable world-space coordinates for texturing/noise near the camera (wraps every 4096 m).
fn tiling_pos(rel: vec3<f32>) -> vec3<f32> { return rel + frame.camera_world_frac.xyz; }

// Distance along the camera's forward axis (positive in front).
fn view_depth(rel: vec3<f32>) -> f32 {
    return -(frame.view * vec4<f32>(rel, 1.0)).z;
}

// ---------------------------------------------------------------- screen space
// Pixel coordinates (top-left origin, centers at .5) of a camera-relative point in the current,
// jittered view: the pixel a fragment at `rel` was rasterized into.
fn pixel_of(rel: vec3<f32>) -> vec2<f32> {
    let clip = frame.view_proj * vec4<f32>(rel, 1.0);
    let ndc = clip.xy / clip.w;
    return (ndc * vec2<f32>(0.5, -0.5) + 0.5) * frame.viewport.xy;
}

// View-space position (camera at the origin looking down -Z) of the depth-buffer value `z` at
// screen `uv`.
fn view_position(uv: vec2<f32>, z: f32) -> vec3<f32> {
    let p = frame.inv_proj * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, z, 1.0);
    return p.xyz / p.w;
}

// Distance along the view axis of the depth-buffer value `z` at screen `uv` (meters).
fn scene_linear_depth(uv: vec2<f32>, z: f32) -> f32 {
    return -view_position(uv, z).z;
}

// Whether the prepass drew the surface at camera-relative `rel` (so its normal, ambient
// occlusion and motion are in the screen-space maps). Fragments of features without a prepass,
// transparent surfaces and anything in front of the prepass geometry get false.
fn in_prepass(rel: vec3<f32>, pixel: vec2<i32>) -> bool {
    let size = vec2<i32>(textureDimensions(prepass_depth));
    if (any(pixel < vec2<i32>(0)) || any(pixel >= size)) { return false; }
    let z = textureLoad(prepass_depth, pixel, 0);
    if (z <= 0.0 || textureLoad(prepass_normal, pixel, 0).w < 0.5) { return false; }
    let uv = (vec2<f32>(pixel) + 0.5) * frame.viewport.zw;
    let d = view_depth(rel);
    return abs(scene_linear_depth(uv, z) - d) <= 0.01 * d + 0.01;
}

// Screen-space ambient occlusion for the surface at `rel` (1 = open sky and walls far away,
// toward 0 in creases and under objects). 1 when ambient occlusion is off or the surface wasn't
// in the prepass.
fn screen_ao(rel: vec3<f32>) -> f32 {
    let strength = frame.effects.x;
    if (strength <= 0.0) { return 1.0; }
    let pixel = vec2<i32>(floor(pixel_of(rel)));
    if (!in_prepass(rel, pixel)) { return 1.0; }
    let ao = textureLoad(ao_map, pixel, 0).r;
    return pow(saturate(ao), strength);
}

// Screen-space reflection for the surface at `rel`: reflected radiance (cd/m^2, not
// pre-exposed) and how much of the reflection it covers (0 = reflect the sky only). Zero when
// reflections are off or the surface wasn't in the prepass.
fn screen_reflection(rel: vec3<f32>) -> vec4<f32> {
    if (frame.reflections.x <= 0.0) { return vec4<f32>(0.0); }
    let pixel = vec2<i32>(floor(pixel_of(rel)));
    if (!in_prepass(rel, pixel)) { return vec4<f32>(0.0); }
    return textureLoad(ssr_map, pixel, 0);
}

// Diffuse light bounced onto the surface at `rel` from nearby surfaces on screen, as radiance
// (multiply by the diffuse color), or 0 where screen-space GI is off or `rel` isn't what the
// prepass drew.
fn screen_gi(rel: vec3<f32>) -> vec3<f32> {
    if (frame.reflections.w <= 0.0) { return vec3<f32>(0.0); }
    let pixel = vec2<i32>(floor(pixel_of(rel)));
    if (!in_prepass(rel, pixel)) { return vec3<f32>(0.0); }
    return textureLoad(gi_map, pixel, 0).rgb * frame.reflections.w;
}

// Ambient occlusion with interreflections (Jimenez et al. 2016, "Practical Real-Time Strategies
// for Accurate Indirect Occlusion"): bright surfaces bounce light into their own creases.
fn ao_multibounce(visibility: f32, albedo: vec3<f32>) -> vec3<f32> {
    let a = 2.0404 * albedo - 0.3324;
    let b = -4.7951 * albedo + 0.6417;
    let c = 2.7552 * albedo + 0.6903;
    return max(vec3<f32>(visibility), ((visibility * a + b) * visibility + c) * visibility);
}

// Occlusion of reflected environment light from ambient occlusion (Lagarde & de Rousiers 2014).
// `roughness` is the squared (alpha) roughness.
fn specular_occlusion(n_dot_v: f32, visibility: f32, roughness: f32) -> f32 {
    return saturate(pow(n_dot_v + visibility, exp2(-16.0 * roughness - 1.0)) - 1.0 + visibility);
}

// ---------------------------------------------------------------- prepass output
// What a feature's prepass fragment shader returns (`RenderFeature::draw_prepass`; targets from
// `GpuShared::prepass_targets`). Fill it with `prepass_output`.
struct PrepassOutput {
    @location(0) normal: vec4<f32>,  // octahedral world normal xy, perceptual roughness, 1
    @location(1) motion: vec2<f32>,  // screen motion since last frame (uv units, current - previous)
};

// Unjittered clip position of a camera-relative point, for motion vectors. Pass it from the
// vertex to the fragment shader as a vec4 and hand it to `prepass_output`.
fn motion_clip(rel: vec3<f32>) -> vec4<f32> {
    return frame.unjittered_view_proj * vec4<f32>(rel, 1.0);
}

// Clip position of where a point was last frame, from its position relative to LAST frame's
// camera (`prev_rel`). For geometry that doesn't move in the world use `static_prev_rel(rel)`.
fn prev_motion_clip(prev_rel: vec3<f32>) -> vec4<f32> {
    return frame.prev_view_proj * vec4<f32>(prev_rel, 1.0);
}

// Last frame's camera-relative position of a point that is fixed in the world.
fn static_prev_rel(rel: vec3<f32>) -> vec3<f32> {
    return rel + frame.camera_delta.xyz;
}

// Prepass output from the shaded normal (world space, unit), perceptual roughness and the two
// interpolated motion clip positions.
fn prepass_output(normal: vec3<f32>, roughness: f32, clip_now: vec4<f32>, clip_prev: vec4<f32>) -> PrepassOutput {
    var out: PrepassOutput;
    out.normal = vec4<f32>(oct_encode(normal), roughness, 1.0);
    var motion = vec2<f32>(0.0);
    if (clip_prev.w > 1e-6 && clip_now.w > 1e-6) {
        motion = (clip_now.xy / clip_now.w - clip_prev.xy / clip_prev.w) * vec2<f32>(0.5, -0.5);
    }
    out.motion = motion;
    return out;
}

// Wind displacement for a vertex: `sway` meters at the top (`height01` = 1), none at the root.
fn wind_offset(rel: vec3<f32>, height01: f32, sway: f32) -> vec3<f32> {
    if (sway <= 0.0) { return vec3(0.0); }
    let w = frame.wind.xyz;
    let speed = length(w);
    let p = tiling_pos(rel);
    let t = frame.time.x;
    let phase = dot(p.xz, vec2(0.13, 0.17));
    let gust = 0.6 + 0.4 * sin(t * 0.7 + phase * 0.3) * sin(t * 1.9 + phase);
    let flutter = sin(t * 6.0 + phase * 3.0) * 0.15;
    let dir = select(vec3(1.0, 0.0, 0.0), w / max(speed, 1e-3), speed > 0.01);
    let strength = sway * height01 * height01 * min(speed / 10.0, 1.5);
    return dir * strength * (gust + flutter) + vec3(0.0, -0.1, 0.0) * strength * gust;
}

// ---------------------------------------------------------------- shadows
const SHADOW_BLOCKER_TAPS: i32 = 8;
const SHADOW_PCF_TAPS: i32 = 16;
const GOLDEN_ANGLE: f32 = 2.39996323;

// Per-pixel noise in [0, 1) that changes every frame when temporal anti-aliasing is on (its
// history averages it out); fixed otherwise.
fn frame_noise(pixel: vec2<f32>) -> f32 {
    var offset = 0.0;
    if (any(frame.jitter.zw != vec2<f32>(0.0))) {
        offset = f32(u32(frame.time.w) % 64u) * 5.588238;
    }
    return interleaved_gradient_noise(pixel + offset);
}

// Point i of n on a Vogel (sunflower) disk of radius 1, turned by `angle`.
fn vogel_disk(i: i32, n: i32, angle: f32) -> vec2<f32> {
    let r = sqrt((f32(i) + 0.5) / f32(n));
    let theta = f32(i) * GOLDEN_ANGLE + angle;
    return vec2<f32>(cos(theta), sin(theta)) * r;
}

// Sun visibility (0 = in shadow, 1 = lit) at camera-relative `rel` with normal `n`: the shadow
// cascades out to `RenderSettings::shadow_distance`, blending over their last tenth into the
// far shadow map (big casters only, out to `far_shadow_distance`; `far_shadow_visibility`).
fn shadow_factor(rel: vec3<f32>, n: vec3<f32>) -> f32 {
    if (frame.sun_color.w < 0.5) { return 1.0; }
    let depth = view_depth(rel);
    let noise = frame_noise(pixel_of(rel));
    let reach = frame.shadow_splits[3];
    let near = saturate((reach - depth) / (reach * 0.1));
    var s = 1.0;
    if (near > 0.0) { s = cascade_visibility(rel, n, depth, noise); }
    if (near >= 1.0) { return s; }
    return mix(far_shadow_visibility(rel, n, noise), s, near);
}

// Sun visibility from the shadow cascades at view depth `depth` (within their reach).
//
// Percentage-closer soft shadows: a blocker search estimates how far the caster is from the
// receiver, and the filter grows with that distance times the sun's angular size
// (`RenderSettings::sun_penumbra_scale`): crisp where things touch the ground, soft at the end of
// a tree's shadow. Filter taps are rotated per pixel, and cascades blend with a dithered band.
fn cascade_visibility(rel: vec3<f32>, n: vec3<f32>, depth: f32, noise: f32) -> f32 {
    var cascade = 0u;
    for (var i = 0u; i < 3u; i++) {
        if (depth > frame.shadow_splits[i]) { cascade = i + 1u; }
    }
    // Dither into the next cascade over the last 10% of this one.
    if (cascade < 3u) {
        let split = frame.shadow_splits[cascade];
        let band = split * 0.1;
        if (depth > split - band && noise > (split - depth) / band) { cascade += 1u; }
    }
    let map_size = frame.shadow_params.w;
    // World size of one shadow texel in this cascade, and NDC depth per meter (orthographic).
    let m = frame.shadow_cascades[cascade];
    let texel_world = 2.0 / (map_size * length(vec3<f32>(m[0].x, m[1].x, m[2].x)));
    let z_per_meter = length(vec3<f32>(m[0].z, m[1].z, m[2].z));
    let l = frame.sun_dir.xyz;
    let n_dot_l = dot(n, l);
    let normal_bias = frame.shadow_params.y * texel_world * (1.0 - 0.5 * saturate(n_dot_l));
    let p = rel + n * normal_bias;
    let lp = m * vec4<f32>(p, 1.0);
    let uv = lp.xy * vec2<f32>(0.5, -0.5) + 0.5;
    let z = lp.z - frame.shadow_params.x;
    if (any(uv < vec2(0.0)) || any(uv > vec2(1.0)) || z > 1.0) { return 1.0; }
    let texel = 1.0 / map_size;
    let angle = noise * TAU;
    let tan_sun = tan(frame.sun_dir.w) * frame.effects.z;
    let min_radius = max(frame.shadow_params.z, 0.5);

    // Blocker search over the largest penumbra a caster up to 30 m away could make.
    let search = clamp(30.0 * tan_sun / texel_world, min_radius, 16.0);
    var blocker_sum = 0.0;
    var blockers = 0.0;
    for (var i = 0; i < SHADOW_BLOCKER_TAPS; i++) {
        let o = vogel_disk(i, SHADOW_BLOCKER_TAPS, angle) * search * texel;
        let d = textureSampleLevel(shadow_map, sky_occlusion_sampler, uv + o, cascade, 0);
        if (d < z) {
            blocker_sum += d;
            blockers += 1.0;
        }
    }
    if (blockers < 0.5) { return 1.0; }
    let blocker_distance = max(z - blocker_sum / blockers, 0.0) / max(z_per_meter, 1e-9);
    let radius = clamp(blocker_distance * tan_sun / texel_world, min_radius, 24.0);
    // Wider filters reach further across sloped receivers: bias with the slope.
    let slope = sqrt(max(1.0 - n_dot_l * n_dot_l, 0.0)) / max(abs(n_dot_l), 0.2);
    let z_ref = z - radius * texel_world * min(slope, 5.0) * z_per_meter * 0.5;

    var sum = 0.0;
    for (var i = 0; i < SHADOW_PCF_TAPS; i++) {
        let o = vogel_disk(i, SHADOW_PCF_TAPS, angle) * radius * texel;
        sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + o, cascade, z_ref);
    }
    return sum / f32(SHADOW_PCF_TAPS);
}

// Sun visibility from the far shadow map: one coarse map around the camera (texels of several
// meters) holding only big casters, so mountains shade the valley and ridges shade themselves
// kilometers away. 1 where it is off or doesn't reach; fades out over its outer tenth.
const FAR_SHADOW_TAPS: i32 = 12;

fn far_shadow_visibility(rel: vec3<f32>, n: vec3<f32>, noise: f32) -> f32 {
    let params = frame.far_shadow_params;
    let reach = params.x;
    if (reach <= 0.0) { return 1.0; }
    let dist = length(rel);
    let fade = saturate((reach - dist) / (reach * 0.1));
    if (fade <= 0.0) { return 1.0; }
    let texel_world = params.y;
    let z_per_meter = params.z;
    let layer = i32(params.w);
    let l = frame.sun_dir.xyz;
    let n_dot_l = dot(n, l);
    // Surfaces facing away from the sun are dark anyway; their own texels must not shade them.
    let normal_bias = frame.shadow_params.y * texel_world * (1.0 - 0.5 * saturate(n_dot_l));
    let lp = frame.far_shadow * vec4<f32>(rel + n * normal_bias, 1.0);
    let uv = lp.xy * vec2<f32>(0.5, -0.5) + 0.5;
    if (any(uv < vec2(0.0)) || any(uv > vec2(1.0)) || lp.z > 1.0) { return 1.0; }
    // A fixed filter of a texel and a half: wider than the sun's penumbra of a ridge a few
    // kilometers away, so edges stay smooth at this resolution. Biased with the slope like the
    // cascades.
    let radius = 1.5;
    let slope = sqrt(max(1.0 - n_dot_l * n_dot_l, 0.0)) / max(abs(n_dot_l), 0.2);
    let z_ref = lp.z - frame.shadow_params.x - (0.5 + radius * min(slope, 5.0) * 0.5) * texel_world * z_per_meter;
    let texel = 1.0 / frame.shadow_params.w;
    let angle = noise * TAU;
    var sum = 0.0;
    for (var i = 0; i < FAR_SHADOW_TAPS; i++) {
        let o = vogel_disk(i, FAR_SHADOW_TAPS, angle) * radius * texel;
        sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + o, layer, z_ref);
    }
    return mix(1.0, sum / f32(FAR_SHADOW_TAPS), fade);
}

// Screen-space contact shadow toward the sun: marches `frame.effects.y` meters through the
// prepass depth and returns 0 where something closer to the camera blocks the ray. Catches the
// small shadows (pebbles, grass, a foot on the ground) the shadow map is too coarse for. Only
// for surfaces in the prepass, within 60 m.
const CONTACT_STEPS: i32 = 10;

fn contact_shadow(rel: vec3<f32>, n: vec3<f32>) -> f32 {
    let len = frame.effects.y;
    if (len <= 0.0) { return 1.0; }
    let depth = view_depth(rel);
    if (depth > 60.0) { return 1.0; }
    let pixel = pixel_of(rel);
    if (!in_prepass(rel, vec2<i32>(floor(pixel)))) { return 1.0; }
    let l = frame.sun_dir.xyz;
    let noise = frame_noise(pixel.yx + 17.0);
    let start = rel + n * (0.005 + depth * 0.002);
    let size = vec2<i32>(textureDimensions(prepass_depth));
    var shadow = 1.0;
    for (var i = 0; i < CONTACT_STEPS; i++) {
        let t = (f32(i) + noise) / f32(CONTACT_STEPS);
        let q = start + l * (len * t);
        let clip = frame.view_proj * vec4<f32>(q, 1.0);
        if (clip.w <= 0.0) { break; }
        let uv = clip.xy / clip.w * vec2<f32>(0.5, -0.5) + 0.5;
        if (any(uv <= vec2(0.0)) || any(uv >= vec2(1.0))) { break; }
        let px = vec2<i32>(uv * frame.viewport.xy);
        let z = textureLoad(prepass_depth, clamp(px, vec2<i32>(0), size - 1), 0);
        if (z <= 0.0) { continue; }
        let scene = scene_linear_depth(uv, z);
        let ray = view_depth(q);
        let diff = ray - scene;
        // In front of the ray by more than the depth noise, but not so far it's a separate
        // object floating in front (thickness).
        let bias = 0.01 + ray * 0.003;
        let thickness = 0.25 + ray * 0.02;
        if (diff > bias && diff < thickness) {
            // Fade with distance along the ray and toward the 60 m limit.
            shadow = mix(1.0, t * 0.5, 1.0 - smoothstep(40.0, 60.0, depth));
            break;
        }
    }
    return shadow;
}

// ---------------------------------------------------------------- cloud shadows
// Share of the sunlight (0..1) the clouds let through to camera-relative `rel`: multiply
// sunlight by it, with the shadow maps and `sun_transmittance`. `shade_surface`,
// `shade_thin_surface`, the fog's sunlight and the grass already do; shaders that light with
// the sun themselves (terrain, water, foliage) call it. 1 when no module draws cloud shadows.
//
// The map lies on the cloud base, top-down and anchored to the world (cloud_shadows.rs): it is
// read where the sun ray through `rel` crosses the base, so shadows fall away from the sun and
// are right at any height below the clouds. Inside the cloud layer the shadow fades out toward
// its top; beyond the map's square the sunlight is the writer's `outside` value, which the map
// fades into over its outer tenth.
fn cloud_shadow(rel: vec3<f32>) -> f32 {
    let params = frame.cloud_shadow_params;
    if (params.x < 0.5) { return 1.0; }
    let c = frame.cloud_shadow * vec4<f32>(rel, 1.0);
    let edge = max(abs(c.x), abs(c.y));
    var t = params.w;
    if (edge < 1.0) {
        let uv = c.xy * vec2<f32>(0.5, -0.5) + 0.5;
        let map = textureSampleLevel(cloud_shadow_map, linear_clamp, uv, 0.0).r;
        t = mix(map, params.w, saturate((edge - 0.9) * 10.0));
    }
    let in_layer = saturate((rel.y - params.y) / max(params.z, 1.0));
    return mix(saturate(t), 1.0, in_layer);
}

// For modules that draw the cloud shadow map: the camera-relative point on the cloud base at
// map coordinates `uv` (u along world x, v along world z, as `fullscreen_vertex` gives them).
// The texel there holds the sunlight that reaches this point down the sun direction
// (`frame.sun_dir`).
fn cloud_shadow_plane_point(uv: vec2<f32>) -> vec3<f32> {
    let area = frame.cloud_shadow_area;
    return vec3<f32>(area.x + uv.x * area.z, frame.cloud_shadow_params.y, area.y + uv.y * area.z);
}

// ---------------------------------------------------------------- atmosphere
// The sky, sunlight through the air and aerial perspective live in lib_atmosphere.wgsl
// (`atmosphere`, `sun_transmittance`, `sky_view_radiance`, `planet_height`, `ray_shell`).

// ---------------------------------------------------------------- environment lighting
// The sky (and, without a planet, the ground below the horizon) as seen from the camera,
// redrawn and filtered every `environment_interval` frames (environment.rs).
fn sample_env(dir: vec3<f32>) -> vec3<f32> {
    return textureSampleLevel(env_map, env_sampler, dir_to_equirect(dir), 0.0).rgb;
}

// The sky reflected toward `dir` by a surface of perceptual `roughness`: GGX-prefiltered, so
// rough surfaces reflect a blurred sky.
fn sample_env_rough(dir: vec3<f32>, roughness: f32) -> vec3<f32> {
    let lod = saturate(roughness) * f32(textureNumLevels(env_map) - 1u);
    return textureSampleLevel(env_map, env_sampler, dir_to_equirect(dir), lod).rgb;
}

// Light bounced off the ground below (cd/m^2): sky irradiance from above plus sun, times the
// ground albedo. Fades out high above the planet (frame.ground.w).
fn ground_radiance() -> vec3<f32> {
    let sky = textureSampleLevel(env_irradiance, env_sampler, vec2<f32>(0.5, 0.0), 0.0).rgb;
    let sun = frame.sun_camera.rgb * saturate(frame.sun_dir.y) * INV_PI;
    return frame.ground.rgb * frame.ground.w * (sky + sun);
}

// Cosine-weighted sky radiance (cd/m^2) around normal `n`, plus ambient fill: diffuse sky light
// is `sky_ambient(n) * albedo`.
fn sky_ambient(n: vec3<f32>) -> vec3<f32> {
    return textureSampleLevel(env_irradiance, env_sampler, dir_to_equirect(n), 0.0).rgb + frame.ambient.rgb;
}

// `sky_ambient(n)` at a point that sees `sky_vis` of the open sky (`sky_visibility`; multiply
// by it as usual): as in the open where nothing covers it, and arriving more and more evenly
// from all around as cover closes in. Sky light reaching a covered point came in through
// openings and bounced off the walls on the way, so a ceiling indoors isn't lit by the sunlit
// ground outside, nor a cave's floor by the blue sky overhead.
fn sky_ambient_covered(n: vec3<f32>, sky_vis: f32) -> vec3<f32> {
    let around = (sky_ambient(vec3<f32>(0.0, 1.0, 0.0)) + sky_ambient(vec3<f32>(0.0, -1.0, 0.0))
        + sky_ambient(vec3<f32>(1.0, 0.0, 0.0)) + sky_ambient(vec3<f32>(-1.0, 0.0, 0.0))
        + sky_ambient(vec3<f32>(0.0, 0.0, 1.0)) + sky_ambient(vec3<f32>(0.0, 0.0, -1.0))) / 6.0;
    return mix(around, sky_ambient(n), saturate(sky_vis));
}

// Fraction of the open sky seen from camera-relative `rel` with normal `n`: 1 outdoors, toward 0
// in caves, rooms and under overhangs. Multiply sky/ambient light by it. Also the test for
// "is this spot exposed to rain and snow". Uses the top-down sky occlusion map around the
// camera (no cover where the map is off or doesn't reach) and the indoor volumes.
//
// It integrates the view factor of the cover above the point: the ground around the point is
// split into a center disc and rings that double in size out to 96 m, one tap per cell. A
// cell covered at height h above the point hides the part of the sky that looks through it,
// which for an annulus a..b is b²/(b²+h²) - a²/(a²+h²) of an upward-facing surface's sky. So a
// carport 3 m up hides most of the sky, a roof 30 m up only a few percent, and an aircraft
// 100 m up next to nothing. Each tap gathers 2x2 texels and blends them bilinearly, so cover
// fades in smoothly at roof edges. Tilted surfaces weigh each cell by their own cosine toward
// it: a wall ignores the building behind it and a hillside ignores the slope above it.
// The map sees cover from above only, so walls don't keep sky light out: caves, rooms and ship
// interiors add a `SkyLightVolume` (`sky_volume_factor`).
const SKY_CENTER_RADIUS: f32 = 1.5; // m
const SKY_RINGS: u32 = 6u;          // ring k spans 1.5 * 2^k .. 1.5 * 2^(k+1) m
const SKY_RING_TAPS: u32 = 8u;

// Share of an upward-facing surface's sky hidden by cover at height `h` over the annulus a..b.
fn sky_view_factor(a: f32, b: f32, h: f32) -> f32 {
    let h2 = h * h;
    return b * b / (b * b + h2) - a * a / (a * a + h2);
}

fn sky_visibility(rel: vec3<f32>, n: vec3<f32>) -> f32 {
    // The indoor volumes are read in the air in front of the surface (so walls just outside a
    // loosely fitted volume still count as inside), and a surface on or inside a volume's box
    // counts as inside even where that step along its normal crosses a face: a wall's edge
    // where bumps tilt its normal into the ceiling, the rounded foot of a chair leg on the
    // floor. (Those spots read the open sky through the volume's face otherwise.)
    return sky_map_visibility(rel, n) * min(sky_volume_factor(rel + n * 0.2), sky_volume_factor_within(rel, 0.01));
}

// Sky light left at `rel` by the indoor volumes: `sky_light` deep inside, rising to 1 at open
// faces over `fade` meters; 1 outside every volume.
fn sky_volume_factor(rel: vec3<f32>) -> f32 {
    return sky_volume_factor_within(rel, 0.0);
}

// `sky_volume_factor` with every volume's box grown by `margin` meters on each side (a point
// on a face is inside).
fn sky_volume_factor_within(rel: vec3<f32>, margin: f32) -> f32 {
    var f = 1.0;
    let p = vec4<f32>(rel, 1.0);
    for (var i = 0u; i < min(frame.counts.w, 16u); i++) {
        let v = frame.sky_volumes[i];
        let half = v.half_fade.xyz;
        let local = vec3<f32>(dot(v.to_local[0], p), dot(v.to_local[1], p), dot(v.to_local[2], p));
        if (any(abs(local) >= half + margin)) { continue; }
        let q = clamp(local, -half, half);
        // Distance to the nearest open face (+x, -x, +y, -y, +z, -z).
        let mask = u32(v.params.y);
        let to_pos = half - q;
        let to_neg = half + q;
        var open = 1e9;
        if ((mask & 1u) != 0u) { open = min(open, to_pos.x); }
        if ((mask & 2u) != 0u) { open = min(open, to_neg.x); }
        if ((mask & 4u) != 0u) { open = min(open, to_pos.y); }
        if ((mask & 8u) != 0u) { open = min(open, to_neg.y); }
        if ((mask & 16u) != 0u) { open = min(open, to_pos.z); }
        if ((mask & 32u) != 0u) { open = min(open, to_neg.z); }
        f = min(f, mix(1.0, v.params.x, smoothstep(0.0, 1.0, open / v.half_fade.w)));
    }
    return f;
}

// Sky visibility from the top-down occlusion map alone.
fn sky_map_visibility(rel: vec3<f32>, n: vec3<f32>) -> f32 {
    let params = frame.sky_occlusion_params;
    if (params.x < 0.5) { return 1.0; }
    let texel = params.z;
    let depth_range = params.w;
    // Start just off the surface; walls also step one texel and a half out of their own cover.
    let p = rel + n * 0.2 + vec3<f32>(n.x, 0.0, n.z) * (texel * 1.5);
    let c = frame.sky_occlusion * vec4<f32>(p, 1.0);
    let edge = max(abs(c.x), abs(c.y));
    if (edge >= 1.0 || c.z <= 0.0 || c.z >= 1.0) { return 1.0; }
    // Cover must be clearly above the point. Over a surface facing up, its own texel and the
    // bumps around it don't count. Walls have stepped out of their own texel above, and what is
    // over the underside of something is cover (the roof over a ceiling, the ceiling over the
    // top of a wall: with the full margin, the top of every wall under a roof less than
    // 0.3 m + a texel thick, and the ceiling itself, saw open sky).
    let bias = 0.05 + (0.25 + texel) * saturate(n.y);
    // Clip-space steps for one meter along world x and z (orientation-agnostic).
    let step_x = (frame.sky_occlusion * vec4<f32>(1.0, 0.0, 0.0, 0.0)).xy;
    let step_z = (frame.sky_occlusion * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xy;
    let size = vec2<f32>(textureDimensions(sky_occlusion_map));
    var hidden = 0.0;
    for (var i = 0u; i <= SKY_RINGS * SKY_RING_TAPS; i++) {
        var o = vec2<f32>(0.0);
        var a = 0.0;
        var b = SKY_CENTER_RADIUS;
        var share = 1.0;
        if (i > 0u) {
            let ring = (i - 1u) / SKY_RING_TAPS;
            let k = (i - 1u) % SKY_RING_TAPS;
            a = SKY_CENTER_RADIUS * f32(1u << ring);
            b = a * 2.0;
            // Odd rings are turned half a step so taps don't line up radially.
            let angle = (f32(k) + 0.5 * f32(ring & 1u)) * (TAU / f32(SKY_RING_TAPS));
            o = vec2<f32>(cos(angle), sin(angle)) * sqrt((a * a + b * b) * 0.5);
            share = 1.0 / f32(SKY_RING_TAPS);
        }
        let tc = c.xy + step_x * o.x + step_z * o.y;
        let uv = tc * vec2<f32>(0.5, -0.5) + 0.5;
        if (any(uv < vec2(0.0)) || any(uv >= vec2(1.0))) { continue; }
        // Gather order: (0,1), (1,1), (1,0), (0,0) texels of the bilinear footprint.
        let tops = textureGather(sky_occlusion_map, sky_occlusion_sampler, uv);
        var cover = vec4<f32>(0.0);
        for (var j = 0; j < 4; j++) {
            let h = (c.z - tops[j]) * depth_range;
            if (h > bias) {
                let toward = dot(n, vec3<f32>(o.x, h, o.y)) / h;
                cover[j] = sky_view_factor(a, b, h) * max(toward, 0.0);
            }
        }
        let f = fract(uv * size - 0.5);
        hidden += mix(mix(cover.w, cover.z, f.x), mix(cover.x, cover.y, f.x), f.y) * share;
    }
    // Relative to the sky this surface could see at all (half of it for a wall).
    let vis = saturate(1.0 - hidden / max((1.0 + n.y) * 0.5, 0.05));
    // Fade out over the outer eighth of the map so its edge doesn't show.
    return mix(vis, 1.0, saturate((edge - 0.875) * 8.0));
}

// Share of the sky a reflection sees, from camera-relative `rel` toward `dir` (unit; the
// reflection's dominant direction) on a surface with smooth normal `n` and squared (alpha)
// roughness `roughness`: 1 in the open, toward 0 where cover in the top-down sky occlusion map
// rises above the reflection, such as the trees around a forest pond, the far side of a gully or
// the buildings around a square. It marches the map outward along the reflection's heading
// (SPECULAR_SKY_TAPS taps from about 1 m to 55 m, each a quarter farther than the last, the
// sequence shifted per pixel by `frame_noise` so that walls thinner than the gaps between far
// taps still count once temporal anti-aliasing averages it) and compares the steepest cover with
// the reflection's elevation, softened over the lobe's width. Straight-up reflections and points
// off the map see the sky. The map sees cover from above only, so combine this with
// `sky_visibility` (`shade_surface` takes the smaller of the two).
const SPECULAR_SKY_TAPS: u32 = 18u;

fn specular_sky_visibility(rel: vec3<f32>, dir: vec3<f32>, n: vec3<f32>, roughness: f32) -> f32 {
    let params = frame.sky_occlusion_params;
    let flat_len = length(dir.xz);
    if (params.x < 0.5 || flat_len < 1e-3) { return 1.0; }
    let p = rel + n * 0.2;
    let c = frame.sky_occlusion * vec4<f32>(p, 1.0);
    let edge = max(abs(c.x), abs(c.y));
    if (edge >= 1.0 || c.z <= 0.0 || c.z >= 1.0) { return 1.0; }
    // Clip-space step for one meter along the heading (orientation-agnostic).
    let heading = dir.xz / flat_len;
    let step = (frame.sky_occlusion * vec4<f32>(heading.x, 0.0, heading.y, 0.0)).xy;
    // Cover must be clearly above the point, as for `sky_map_visibility`: the ground the surface
    // lies on and its bumps don't count (a reflection below the surface's own horizon is
    // `specular_horizon_fade`'s), so open water stays bright out to the horizon.
    let bias = 0.05 + (0.25 + params.z) * saturate(n.y);
    // Sine of the elevation of the highest cover seen so far.
    var steepest = -1.0;
    var t = pow(1.25, frame_noise(pixel_of(rel).yx + 41.0));
    for (var i = 0u; i < SPECULAR_SKY_TAPS; i++) {
        let uv = (c.xy + step * t) * vec2<f32>(0.5, -0.5) + 0.5;
        if (any(uv < vec2(0.0)) || any(uv >= vec2(1.0))) { break; }
        // The highest of the 2x2 texels around the tap (smaller depth is higher).
        let tops = textureGather(sky_occlusion_map, sky_occlusion_sampler, uv);
        let h = (c.z - min(min(tops.x, tops.y), min(tops.z, tops.w))) * params.w;
        if (h > bias) {
            steepest = max(steepest, h * inverseSqrt(h * h + t * t));
        }
        t *= 1.25;
    }
    // Half the lobe is hidden when the cover is level with the reflection.
    let width = 0.04 + roughness;
    let vis = 1.0 - smoothstep(-width, width, steepest - dir.y);
    return mix(vis, 1.0, saturate((edge - 0.875) * 8.0));
}

// Fade for reflected light whose mirror direction `r` points below the surface's own horizon
// (smooth normal `n`), as a strong normal map's bumps make it do at grazing angles: such a
// reflection would hit the surface itself (Russell 2015, "Horizon occlusion for normal mapped
// reflections"). 1 for a reflection above the horizon.
fn specular_horizon_fade(r: vec3<f32>, n: vec3<f32>) -> f32 {
    let h = saturate(1.0 + 1.3 * dot(r, n));
    return h * h;
}

// How much the haze along a view ray is lit by the sky. From the map: 1 when either end is
// outdoors (the ray then runs mostly through open air), low when looking around under cover;
// the camera's value is computed once per frame (`sky_camera`). Indoor volumes are sampled at
// four points along the ray, so haze deep in a cave stays dark even when the camera stands at
// its mouth, and haze outside stays lit when looking out of one.
fn fog_sky_visibility(rel: vec3<f32>) -> f32 {
    var map = 1.0;
    let at_camera = sky_camera[0].x;
    if (frame.sky_occlusion_params.x > 0.5 && at_camera < 0.99) {
        map = max(at_camera, sky_map_visibility(rel, vec3<f32>(0.0, 1.0, 0.0)));
    }
    var volumes = 1.0;
    if (frame.counts.w > 0u) {
        volumes = 0.0;
        for (var i = 0; i < 4; i++) {
            volumes += sky_volume_factor(rel * ((f32(i) + 0.5) * 0.25)) * 0.25;
        }
    }
    return map * volumes;
}

// ---------------------------------------------------------------- surface shading
// Returns pre-exposed radiance for a lit surface at camera-relative position `rel`.
fn shade_surface(s: Surface, rel: vec3<f32>) -> vec3<f32> {
    return shade_surface_shadowed(s, rel, 1.0);
}

// `shade_surface` with extra sun visibility (0..1) from relief too small for the shadow maps,
// such as parallax self-shadowing: deep mortar joints and the gaps between cobbles stay dark
// in low sun.
fn shade_surface_shadowed(s: Surface, rel: vec3<f32>, sun_visibility: f32) -> vec3<f32> {
    return shade_surface_sides(s, rel, sun_visibility, s.normal, false);
}

// Shading for a thin sheet seen from one side: a leaf, a petal, a curtain, a paper lantern
// (double-sided or alpha-masked cards). `face` is the geometric normal of the side the camera
// sees (unit, toward the camera: the triangle's own normal, not the shading normal), and
// `s.normal` the shading normal on that side.
//
// Light reaches the side in view only when it comes from that side of the sheet; then it
// reflects as on any surface (diffuse and specular). Light from the far side never makes a
// highlight: `s.subsurface` of it shines through instead, diffuse with a soft forward glow
// (looking toward the light through the sheet), wrapped around the terminator, tinted by the
// albedo (deeper, as light that crossed a leaf is) and shadowed by the shadow maps as seen from
// the lit side. Sky light shines through from behind the same way. With `subsurface` 0 the sheet
// is opaque and its far side stays dark. `sun_visibility` is as for `shade_surface_shadowed`.
fn shade_thin_surface(s: Surface, rel: vec3<f32>, sun_visibility: f32, face: vec3<f32>) -> vec3<f32> {
    return shade_surface_sides(s, rel, sun_visibility, face, true);
}

// The color light takes on crossing a thin sheet of diffuse color `c`: the same hue, deeper
// (half way to the color squared over its brightest channel, which keeps that channel).
fn thin_surface_tint(c: vec3<f32>) -> vec3<f32> {
    let peak = max(max(c.r, c.g), max(c.b, 1e-4));
    return mix(c, c * c / peak, 0.5);
}

// Light let through a thin sheet toward the camera (direction `v`) per unit of light arriving
// along `l` on its far side (1/sr): diffuse, wrapped past the terminator of the shading normal
// (`n_dot_l` < 0 is light from behind) and with a forward glow toward the light's direction of
// travel; normalized like a Lambertian lobe.
fn thin_surface_through(n_dot_l: f32, v: vec3<f32>, l: vec3<f32>) -> f32 {
    let wrap = 0.5;
    let reach = saturate((wrap - n_dot_l) / (1.0 + wrap));
    let forward = pow(saturate(dot(v, -l)), 4.0);
    return reach * (0.75 + 0.75 * forward) * INV_PI;
}

// How much of a light from direction `l` falls on the side of a thin sheet in view (x; `face`
// points toward the camera) and on its far side (y). Both are 0 edge-on, so the switch from
// reflected to transmitted light is smooth.
fn thin_surface_sides(face: vec3<f32>, l: vec3<f32>) -> vec2<f32> {
    let c = dot(face, l);
    return vec2<f32>(smoothstep(0.0, 0.08, c), smoothstep(0.0, 0.08, -c));
}

// Surface shading. With `thin`, `s` is the side in view of a thin sheet facing `face`
// (`shade_thin_surface`); otherwise `face` is unused.
fn shade_surface_sides(s: Surface, rel: vec3<f32>, sun_visibility: f32, face: vec3<f32>, thin: bool) -> vec3<f32> {
    let v = normalize(-rel);
    let n = s.normal;
    let roughness = s.roughness * s.roughness;
    let n_dot_v = max(dot(n, v), 1e-4);
    let f0 = mix(vec3(0.16 * s.reflectance * s.reflectance), s.albedo, s.metallic);
    let diffuse_color = s.albedo * (1.0 - s.metallic);
    // What a thin sheet lets through from its far side.
    let translucent = thin && s.subsurface > 0.0;
    let through_color = thin_surface_tint(diffuse_color) * select(0.0, saturate(s.subsurface), translucent);
    var color = vec3(0.0);

    // Sun.
    let l = frame.sun_dir.xyz;
    let n_dot_l_raw = dot(n, l);
    let n_dot_l = saturate(n_dot_l_raw);
    let sun_light = frame.sun_color.rgb * sun_transmittance(rel) * cloud_shadow(rel);
    let sides = thin_surface_sides(face, l);
    if (thin && dot(sun_light, sun_light) > 0.0 && (sides.x > 0.0 || (translucent && sides.y > 0.0))) {
        // Shadowed as seen from the sheet's sunlit side, so the sheet doesn't shadow itself.
        let lit_side = select(-face, face, sides.x > 0.0);
        let shadow = shadow_factor(rel, lit_side) * sun_visibility;
        if (shadow > 0.0 && sides.x > 0.0 && n_dot_l > 0.0) {
            let h = normalize(l + v);
            let spec = d_ggx(saturate(dot(n, h)), roughness) * v_smith_ggx_correlated(n_dot_v, n_dot_l, roughness)
                * f_schlick(f0, saturate(dot(l, h)));
            let contact = contact_shadow(rel, face);
            color += (diffuse_color * INV_PI + spec) * (n_dot_l * sides.x * contact * shadow) * sun_light;
        }
        if (shadow > 0.0 && sides.y > 0.0) {
            color += through_color * (thin_surface_through(n_dot_l_raw, v, l) * sides.y * shadow) * sun_light;
        }
    } else if (!thin && dot(sun_light, sun_light) > 0.0) {
        var shadow = shadow_factor(rel, n) * sun_visibility;
        if (shadow > 0.0 && n_dot_l_raw > 0.0) {
            shadow *= contact_shadow(rel, n);
        }
        let h = normalize(l + v);
        let n_dot_h = saturate(dot(n, h));
        let l_dot_h = saturate(dot(l, h));
        let spec = d_ggx(n_dot_h, roughness) * v_smith_ggx_correlated(n_dot_v, n_dot_l, roughness)
            * f_schlick(f0, l_dot_h);
        let wrap = saturate((n_dot_l_raw + s.subsurface) / ((1.0 + s.subsurface) * (1.0 + s.subsurface)));
        let diffuse = diffuse_color * INV_PI * select(n_dot_l, wrap, s.subsurface > 0.0);
        color += (diffuse + spec * n_dot_l) * sun_light * shadow;
        // Back-lit translucency (leaves glowing against the sun).
        if (s.subsurface > 0.0) {
            let back = pow(saturate(dot(v, -l)), 4.0) * s.subsurface * 0.35;
            color += diffuse_color * back * sun_light * shadow * INV_PI;
        }
    }

    // Point lights.
    let count = frame.counts.x;
    for (var i = 0u; i < count; i++) {
        let light = point_lights[i];
        let to_light = light.position_range.xyz - rel;
        let dist2 = dot(to_light, to_light);
        let range = light.position_range.w;
        if (dist2 > range * range) { continue; }
        let dist = sqrt(dist2);
        let pl = to_light / max(dist, 1e-4);
        let nl = saturate(dot(n, pl));
        if (nl <= 0.0 && s.subsurface <= 0.0) { continue; }
        let falloff = sq(saturate(1.0 - sq(dist2 / (range * range)))) / max(dist2, sq(light.color_radius.w));
        let h = normalize(pl + v);
        let spec = d_ggx(saturate(dot(n, h)), max(roughness, 0.02)) *
            v_smith_ggx_correlated(n_dot_v, nl, roughness) * f_schlick(f0, saturate(dot(pl, h)));
        if (thin) {
            // Reflected from the side in view, let through from the far side.
            let lit = thin_surface_sides(face, pl);
            let reflected = (diffuse_color * INV_PI + spec) * (nl * lit.x);
            let through = through_color * (thin_surface_through(dot(n, pl), v, pl) * lit.y);
            color += (reflected + through) * light.color_radius.rgb * falloff;
            continue;
        }
        let wrap = saturate((dot(n, pl) + s.subsurface) / (1.0 + s.subsurface));
        color += (diffuse_color * INV_PI * max(nl, wrap * s.subsurface) + spec * nl) * light.color_radius.rgb * falloff;
    }

    // Sky / ambient (occluded in caves and interiors; the constant fill stays), darkened in
    // creases by the material's and the screen-space ambient occlusion. How much sky the
    // surface sees comes from its smooth normal, so cover doesn't follow the normal map's
    // wrinkles; the shading normal still picks which part of the sky lights it.
    let smooth_n = surface_smooth_normal(s);
    let sky_vis = sky_visibility(rel, smooth_n);
    let fill = frame.ambient.rgb;
    let irradiance = ((sky_ambient_covered(n, sky_vis) - fill) * sky_vis + fill) * PI;
    // Rough surfaces reflect around a direction between the mirror direction and the normal
    // (Lagarde and de Rousiers 2014).
    let r = reflect(-v, n);
    let dominant = normalize(mix(n, r, (1.0 - roughness) * (sqrt(1.0 - roughness) + roughness)));
    // Reflected sky: what the cover overhead leaves, less the low sky that cover around hides
    // along the reflection (trees around a pond; rough surfaces, whose lobe is wide, rely on
    // the cover overhead alone), faded where bumps reflect from below the surface's horizon.
    var spec_sky = sky_vis;
    let lobe_wide = saturate(roughness * 2.0);
    if (lobe_wide < 1.0) {
        let around = specular_sky_visibility(rel, dominant, smooth_n, roughness);
        spec_sky = min(spec_sky, mix(around, 1.0, lobe_wide));
    }
    spec_sky *= specular_horizon_fade(r, smooth_n);
    let ssr = screen_reflection(rel);
    let spec_env = mix(sample_env_rough(dominant, s.roughness) * spec_sky + fill, ssr.rgb, ssr.a);
    let env_brdf = env_brdf_approx(f0, s.roughness, n_dot_v);
    let visibility = min(s.occlusion, screen_ao(rel));
    color += diffuse_color * irradiance * INV_PI * ao_multibounce(visibility, diffuse_color);
    // Sky light shining through a thin sheet from behind.
    if (translucent) {
        color += through_color * ((sky_ambient_covered(-n, sky_vis) - fill) * sky_vis + fill) * visibility;
    }
    // Light bounced from nearby surfaces (its rays already stop at whatever occludes them).
    color += diffuse_color * screen_gi(rel);
    color += spec_env * env_brdf * specular_occlusion(n_dot_v, visibility, roughness);

    color += s.emissive;
    return pre_expose(color);
}

// ---------------------------------------------------------------- fog / aerial perspective
// Height used by fog: altitude above the planet surface, or world y without a planet. Exact to
// millimeters near the camera (no r - R cancellation at planetary scale).
fn fog_height(rel: vec3<f32>) -> f32 {
    if (frame.planet.w > 0.0) {
        let r0 = frame.planet.w + frame.camera_world.w;
        let q = 2.0 * r0 * dot(rel, normalize(-frame.planet.xyz)) + dot(rel, rel); // r^2 - r0^2
        return frame.camera_world.w + q / (sqrt(max(r0 * r0 + q, 0.0)) + r0);
    }
    return frame.camera_world.y + rel.y;
}

// Extinction of the height fog (1/m) at camera-relative `rel` (`FogSettings`: density at the
// base height, falling off exponentially above it).
fn height_fog_density(rel: vec3<f32>) -> f32 {
    let h = fog_height(rel) - frame.fog.z;
    return frame.fog.x * exp(-max(frame.fog.y, 1e-5) * max(h, -100.0));
}

// Optical depth of the height fog along `dir` from `t0` to `t1` meters from the camera: the
// analytic integral of density * exp(-falloff * height) with height linear along the ray.
// Stable for any distance and altitude (orbit, the Moon): both exponents are clamped.
fn height_fog_optical_depth(dir: vec3<f32>, t0: f32, t1: f32) -> f32 {
    let falloff = max(frame.fog.y, 1e-5);
    let ha = fog_height(dir * t0) - frame.fog.z;
    let hb = fog_height(dir * t1) - frame.fog.z;
    let ea = exp(-falloff * max(ha, -100.0));
    let eb = exp(-falloff * max(hb, -100.0));
    let dh = hb - ha;
    let len = t1 - t0;
    if (abs(dh) > 0.01) {
        return frame.fog.x * len * (ea - eb) / (falloff * dh);
    }
    return frame.fog.x * len * ea;
}

// Sky light reaching fog from every direction (cd/m^2, without the ambient fill): the sky's and
// the ground's radiance averaged over the whole sphere, which fog scatters about evenly.
fn fog_sky_light() -> vec3<f32> {
    let up = textureSampleLevel(env_irradiance, env_sampler, vec2<f32>(0.5, 0.0), 0.0).rgb;
    let down = textureSampleLevel(env_irradiance, env_sampler, vec2<f32>(0.5, 1.0), 0.0).rgb;
    return 0.5 * (up + down);
}

// Radiance (cd/m^2, not pre-exposed) of height fog thick enough to hide everything behind it,
// seen along `dir` with the fog at `at`: sunlight through the air with the fog's phase function
// (`sun`: the share of the fog it reaches past mountains and clouds,
// `height_fog_sun_visibility`) and sky light, both
// scaled by `lit` (the open sky the fog sees), plus the ambient fill. This is the fog beyond
// the volumetric range (or all of it with volumetrics off).
fn height_fog_radiance(dir: vec3<f32>, at: vec3<f32>, lit: f32, sun: f32) -> vec3<f32> {
    let sunlight = frame.sun_color.rgb * sun_transmittance(at) * henyey_greenstein(dot(dir, frame.sun_dir.xyz), frame.fog_color.w);
    return frame.fog_color.rgb * ((sunlight * sun + fog_sky_light()) * lit + frame.ambient.rgb);
}

// Whether sunlight reaches camera-relative point `rel` in the air past the far shadow map's
// casters (mountains, terrain, big buildings): one filtered tap, 1 outside the map.
fn far_shadow_sun(rel: vec3<f32>) -> f32 {
    let params = frame.far_shadow_params;
    let lp = frame.far_shadow * vec4<f32>(rel, 1.0);
    let uv = lp.xy * vec2<f32>(0.5, -0.5) + 0.5;
    if (any(uv < vec2(0.0)) || any(uv > vec2(1.0)) || lp.z > 1.0) { return 1.0; }
    // Two texels toward the sun, so air just above the ground isn't shaded by the ground's own
    // texels.
    let z_ref = lp.z - 2.0 * params.y * params.z;
    return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, i32(params.w), z_ref);
}

// The sunlit share of the light the height fog between `t0` and `t1` meters along `dir` scatters
// toward the camera, from the far shadow map and the cloud shadows: haze in the shadow of a
// mountain or a ridge doesn't glow, so a mountain with the sun behind it keeps its contrast
// instead of hiding behind sunlit haze, low sun casts mountain shadows into the valley's haze,
// and haze under a cloud is as dim as the ground there (its sun glow too). `tr` is the fog's
// transmittance over the segment. The samples sit at equal steps of what the fog scatters
// (1 - transmittance), so a thin layer of ground haze is sampled where the ray crosses it.
// 1 without either.
const HAZE_SHADOW_SAMPLES: i32 = 8;

fn height_fog_sun_visibility(dir: vec3<f32>, t0: f32, t1: f32, tr: f32, noise: f32) -> f32 {
    let mountains = frame.far_shadow_params.x > 0.0 && frame.sun_color.w > 0.5;
    let clouds = frame.cloud_shadow_params.x > 0.5;
    if ((!mountains && !clouds) || tr >= 1.0) { return 1.0; }
    let falloff = max(frame.fog.y, 1e-5);
    let density = max(frame.fog.x, 1e-12);
    let ha = max(fog_height(dir * t0) - frame.fog.z, -100.0);
    let hb = max(fog_height(dir * t1) - frame.fog.z, -100.0);
    let len = t1 - t0;
    let dh = hb - ha;
    let ea = exp(-falloff * ha);
    var sum = 0.0;
    for (var i = 0; i < HAZE_SHADOW_SAMPLES; i++) {
        // Optical depth from t0 at which this share of the segment's scattering is reached,
        // then where along the ray the fog adds up to it (the inverse of
        // `height_fog_optical_depth`).
        let tau = -log(1.0 - (f32(i) + noise) / f32(HAZE_SHADOW_SAMPLES) * (1.0 - tr));
        var t = tau / (density * ea);
        if (abs(dh) > 0.01) {
            let slope = dh / len;
            let e = max(ea - tau * falloff * slope / density, 1e-30);
            t = (-log(e) / falloff - ha) / slope;
        }
        let q = dir * (t0 + clamp(t, 0.0, len));
        var lit = 1.0;
        if (mountains) { lit = far_shadow_sun(q); }
        if (clouds) { lit *= cloud_shadow(q); }
        sum += lit;
    }
    return sum / f32(HAZE_SHADOW_SAMPLES);
}

// Volumetric fog between the camera and view depth `depth` (meters, up to the range) in the
// direction of camera-relative `rel`: light scattered toward the camera (pre-exposed) and
// transmittance. Depth slices are spaced exponentially (fine near the camera); texel z of the
// volume holds the integral to the far edge of slice z.
//
// The four froxel columns around the pixel are read and blended by hand:
// - Between the two edges of the slice the depth falls in, a column's fog is taken as uniform:
//   transmittance falls exponentially and the light builds up as it does in `cs_integrate`.
//   (Blending the edges linearly shows each slice as a band where fog is thick, such as a valley
//   of ground fog seen from above.)
// - The froxels hold the fog along their columns' center rays. Where the height fog changes fast
//   from one column to the next (a thin layer of ground fog seen at a glance, where the rows' rays
//   meet the layer meters apart), blending them misjudges how much fog this pixel's ray crosses,
//   in bands one row of froxels apart. So the height fog's share of the transmittance is set
//   right analytically: what the blended columns' rays cross is swapped for what this ray
//   crosses.
// - What is blended for the light is the fog's radiance (light per opacity), which changes
//   smoothly from column to column (the sun's glow), not the light itself, which is mostly the
//   foggiest column's.
fn volumetric_fog_at(rel: vec3<f32>, depth: f32) -> vec4<f32> {
    let v = frame.volumetrics;
    let clip = frame.unjittered_view_proj * vec4<f32>(rel, 1.0);
    let uv = clip.xy / max(clip.w, 1e-6) * vec2<f32>(0.5, -0.5) + 0.5;
    let d = clamp(depth, 0.0, v.x);
    let s = min(v.w * log2(1.0 + d * v.y) * v.z, v.w - 1e-3); // slice coordinate, 0..slices
    let k = floor(s);
    // How far into the slice `depth` is, in meters (the slices' depth mapping inverted).
    let d0 = (exp2(k / (v.w * v.z)) - 1.0) / v.y;
    let d1 = (exp2((k + 1.0) / (v.w * v.z)) - 1.0) / v.y;
    let f = saturate((d - d0) / max(d1 - d0, 1e-6));

    let grid = vec2<f32>(textureDimensions(volumetric_fog).xy);
    let g = uv * grid - 0.5;
    let i0 = floor(g);
    let w = g - i0;
    var t_froxels = 0.0; // the columns' transmittance, blended
    var t_columns = 0.0; // the height fog's share of it, analytically
    var light = vec3<f32>(0.0);
    var radiance = vec3<f32>(0.0);
    var radiance_weight = 0.0;
    for (var j = 0u; j < 4u; j++) {
        let o = vec2<f32>(f32(j & 1u), f32(j >> 1u));
        let cell = clamp(i0 + o, vec2<f32>(0.0), grid - 1.0);
        let weight = mix(1.0 - w.x, w.x, o.x) * mix(1.0 - w.y, w.y, o.y);
        let fog = froxel_column_fog(vec2<i32>(cell), i32(k), f);
        let height_fog = froxel_column_height_fog_t((cell + 0.5) / grid, d0, d1, f);
        t_froxels += weight * fog.a;
        t_columns += weight * height_fog;
        light += weight * fog.rgb;
        // Columns with next to no fog have no radiance to speak of (and an imprecise one).
        let opacity = 1.0 - fog.a;
        let r = weight * saturate(opacity * 50.0);
        radiance += r * fog.rgb / max(opacity, 1e-4);
        radiance_weight += r;
    }
    let dist = length(rel);
    let t_ray = exp(-height_fog_optical_depth(rel / max(dist, 1e-6), 0.0, dist * d / max(view_depth(rel), 1e-6)));
    let t = clamp(t_froxels * t_ray / max(t_columns, 1e-6), 0.0, 1.0);
    if (radiance_weight > 1e-3) {
        light = radiance / radiance_weight * (1.0 - t);
    }
    return vec4<f32>(light, t);
}

// Column `cell` of the volumetric fog up to fraction `f` of slice `k`: in-scattered light and
// transmittance, the fog taken as uniform within the slice.
fn froxel_column_fog(cell: vec2<i32>, k: i32, f: f32) -> vec4<f32> {
    // The integral to the near edge of the slice (clear air at the camera for k = 0) and to its
    // far edge.
    var near = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    if (k >= 1) { near = textureLoad(volumetric_fog, vec3<i32>(cell, k - 1), 0); }
    let far = textureLoad(volumetric_fog, vec3<i32>(cell, k), 0);
    // Transmittance across the whole slice, then up to the depth.
    let slice_t = clamp(far.a / max(near.a, 1e-6), 1e-6, 1.0);
    let t = pow(slice_t, f);
    var share = f; // of the slice's in-scattered light
    if (slice_t < 0.999) { share = (1.0 - t) / (1.0 - slice_t); }
    return vec4<f32>(near.rgb + (far.rgb - near.rgb) * share, near.a * t);
}

// The height fog's transmittance along the center ray of the froxel column at screen `uv`, to
// fraction `f` of the slice from view depth `d0` to `d1`, taken as `froxel_column_fog` takes it.
fn froxel_column_height_fog_t(uv: vec2<f32>, d0: f32, d1: f32, f: f32) -> f32 {
    // The column's ray, per meter of view depth.
    let view = vec3<f32>((uv.x * 2.0 - 1.0) / frame.proj[0][0], (1.0 - uv.y * 2.0) / frame.proj[1][1], -1.0);
    let ray = (frame.inv_view * vec4<f32>(view, 0.0)).xyz;
    let len = length(ray);
    let dir = ray / len;
    let near = exp(-height_fog_optical_depth(dir, 0.0, d0 * len));
    let slice = exp(-height_fog_optical_depth(dir, d0 * len, d1 * len));
    return near * pow(clamp(slice, 1e-6, 1.0), f);
}

// Keeps fog (in-scattered light, transmittance) from hiding more than
// `FogSettings::max_opacity` of what is behind it.
fn limit_fog_opacity(fog: vec4<f32>) -> vec4<f32> {
    let min_t = 1.0 - frame.fog.w;
    if (fog.a >= min_t) { return fog; }
    return vec4<f32>(fog.rgb * (frame.fog.w / max(1.0 - fog.a, 1e-4)), min_t);
}

// Fog and atmospheric haze between the camera and a surface at camera-relative `rel`, in two
// parts: the light they scatter toward the camera (pre-exposed) and the share of the surface's
// own light that gets through, per channel (the air dims blue more than red). What `apply_fog`
// applies; use it directly where the two parts are needed apart (particles and smoke blended
// over the scene: `color * transmittance` for what they let through, `inscatter` once).
struct SurfaceFog {
    inscatter: vec3<f32>,
    transmittance: vec3<f32>,
};

// Within `RenderSettings::volumetric_fog_range` the fog comes from the volumetric fog (height
// fog and fog volumes, with light shafts, sky occlusion and lamps); beyond it the height fog
// continues analytically from the end of the range, so nothing is counted twice, its sunlight
// shaded by the far shadow map's mountains (`height_fog_sun_visibility`). With
// volumetric fog off the analytic height fog starts at the camera. The atmosphere's aerial
// perspective comes on top.
fn surface_fog(rel: vec3<f32>) -> SurfaceFog {
    let dist = length(rel);
    let dir = rel / max(dist, 1e-4);
    // In-scattered light (pre-exposed) and transmittance between the camera and the point.
    var fog = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var start = 0.0;
    if (frame.volumetrics.x > 0.0) {
        let depth = view_depth(rel);
        let range = frame.volumetrics.x;
        fog = volumetric_fog_at(rel, min(depth, range));
        start = select(dist, dist * range / depth, depth > range);
    }
    if (start < dist) {
        let tr = exp(-height_fog_optical_depth(dir, start, dist));
        if (tr < 0.9999) {
            // Haze inside caves and rooms is lit by neither the sky nor the sun (it still hides
            // what's behind it).
            let lit = fog_sky_visibility(rel);
            let sun = height_fog_sun_visibility(dir, start, dist, tr, frame_noise(pixel_of(rel)));
            let l = pre_expose(height_fog_radiance(dir, dir * start, lit, sun)) * (1.0 - tr);
            fog = vec4<f32>(fog.rgb + l * fog.a, fog.a * tr);
        }
    }
    fog = limit_fog_opacity(fog);
    var out: SurfaceFog;
    out.inscatter = fog.rgb;
    out.transmittance = vec3<f32>(fog.a);
    // Aerial perspective: the atmosphere between the camera and distant geometry (negligible
    // closer than a couple hundred meters).
    if (dist > 200.0) {
        let a = atmosphere(vec3(0.0), dir, dist, 8);
        out.inscatter = out.inscatter * a.transmittance + pre_expose(a.radiance * frame.sun_color.rgb);
        out.transmittance *= a.transmittance;
    }
    return out;
}

// Applies fog and atmospheric haze (`surface_fog`) to pre-exposed color seen at camera-relative
// `rel`.
fn apply_fog(color: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    let f = surface_fog(rel);
    return color * f.transmittance + f.inscatter;
}

// Volumetric fog in front of the sky (up to the volumetric range) for pre-exposed sky color
// seen along `dir`. The sky pass calls this; geometry uses `apply_fog`.
fn apply_sky_fog(color: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    if (frame.volumetrics.x <= 0.0) { return color; }
    let fog = limit_fog_opacity(volumetric_fog_at(dir, frame.volumetrics.x));
    return color * fog.a + fog.rgb;
}
