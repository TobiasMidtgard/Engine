// What rain and snow did to the ground, drawn from the simulated ground cover around the
// camera: puddles in the dips that mirror the world (screen-space reflections, the sky where
// they run off screen) with raindrop rings, wet darkening, and snow lying in the open.
// The renderer already wets and whitens everything by the amount at the camera; this pass
// adds what differs from place to place. Requires lib_types + lib_frame.

struct GroundFx {
    map: vec4<f32>,     // cover map min corner relative to the camera (x, z), size (m), texels per side
    rain: vec4<f32>,    // rain intensity 0..1, time (s, wrapped), ripples (1/0), puddle depth at 1.0 in the map (m)
    params: vec4<f32>,  // reflection steps, snow meters per unit in the map, height map offset to camera-relative (m), height map valid (1/0)
};

@group(1) @binding(0) var<uniform> fx: GroundFx;
@group(1) @binding(1) var cover: texture_2d<f32>;      // wetness, puddle depth (0..1 of max), snow, scorch
@group(1) @binding(2) var scene_color: texture_2d<f32>;
@group(1) @binding(3) var scene_depth: texture_depth_2d;
@group(1) @binding(4) var ground_height: texture_2d<f32>; // m above a reference, huge = unknown

// How far `rel` sits above the ground (m), from the cached ground heights (bilinear between
// cell centers), and the ground's steepest slope there (rise over run); (0, 0) where the ground
// isn't known yet.
fn above_ground(rel: vec3<f32>) -> vec2<f32> {
    if (fx.params.w < 0.5) { return vec2(0.0); }
    let size = vec2<i32>(textureDimensions(ground_height));
    let g = (rel.xz - fx.map.xy) / fx.map.z * vec2<f32>(size) - 0.5;
    let i = vec2<i32>(floor(g));
    let f = g - floor(g);
    let h00 = textureLoad(ground_height, clamp(i, vec2(0), size - 1), 0).r;
    let h10 = textureLoad(ground_height, clamp(i + vec2(1, 0), vec2(0), size - 1), 0).r;
    let h01 = textureLoad(ground_height, clamp(i + vec2(0, 1), vec2(0), size - 1), 0).r;
    let h11 = textureLoad(ground_height, clamp(i + vec2(1, 1), vec2(0), size - 1), 0).r;
    if (max(max(h00, h10), max(h01, h11)) > 1e20) { return vec2(0.0); }
    let h = mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y) + fx.params.z;
    let cell = fx.map.z / f32(size.x);
    let rise = max(max(abs(h10 - h00), abs(h11 - h01)), max(abs(h01 - h00), abs(h11 - h10)));
    return vec2<f32>(rel.y - h, rise / cell);
}

fn rel_at(px: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(scene_depth));
    let q = clamp(px, vec2<i32>(0), size - 1);
    let d = textureLoad(scene_depth, q, 0);
    let uv = (vec2<f32>(q) + 0.5) / vec2<f32>(size);
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, max(d, 1e-7), 1.0);
    let p = frame.inv_view_proj * ndc;
    return vec4<f32>(p.xyz / p.w, d);
}

// Rings spreading from raindrops hitting standing water. Returns a normal tilt (x, z).
// `footprint` is the pixel's size on the ground (m): rings finer than a pixel fade out
// instead of turning into glittering noise.
fn ripples(p: vec2<f32>, t: f32, footprint: f32) -> vec2<f32> {
    var g = vec2<f32>(0.0);
    for (var layer = 0; layer < 3; layer++) {
        let fl = f32(layer);
        let scale = 2.2 + fl * 1.3;
        // Ring spacing on the ground is about 1 / (scale * 12) m.
        let resolved = saturate(1.6 - footprint * scale * 12.0 * 1.5);
        if (resolved <= 0.0) { continue; }
        let q = p * scale + vec2<f32>(fl * 3.7, fl * 1.3);
        let cell = floor(q);
        let f = fract(q) - 0.5;
        let h = hash12(cell + vec2<f32>(fl * 17.0, 3.0));
        let phase = fract(t * (0.9 + 0.3 * fl) + h);
        let center = (vec2<f32>(hash12(cell + vec2(3.1, 0.2)), hash12(cell + vec2(7.7, 5.3))) - 0.5) * 0.5;
        let d = f - center;
        let r = length(d);
        let x = (r - phase * 0.55) * 12.0;
        let wave = sin(x * PI) * exp(-x * x * 0.5) * sq(1.0 - phase);
        g += d / max(r, 1e-3) * wave * resolved;
    }
    return g * 0.3;
}

// Screen-space reflection along `dir` from camera-relative `start`. rgb pre-exposed, a = hit.
fn reflect_scene(start: vec3<f32>, dir: vec3<f32>, steps: i32) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(scene_color));
    var t = 0.15;
    var step = 0.2;
    for (var i = 0; i < steps; i++) {
        let q = start + dir * t;
        let clip = frame.view_proj * vec4<f32>(q, 1.0);
        if (clip.w <= 0.0) { break; }
        let ndc = clip.xyz / clip.w;
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        if (any(uv < vec2(0.0)) || any(uv > vec2(1.0))) { break; }
        let px = vec2<i32>(uv * size);
        let scene = textureLoad(scene_depth, px, 0);
        // Reverse-Z: larger is closer. Hit when the ray passes just behind a surface.
        if (scene > ndc.z && scene > 0.0) {
            let near = frame.exposure.z;
            let behind = near / ndc.z - near / scene;
            if (behind < max(step * 2.0, 0.5)) {
                let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
                return vec4<f32>(textureLoad(scene_color, px, 0).rgb, smoothstep(0.0, 0.08, edge));
            }
        }
        t += step;
        step *= 1.18;
    }
    return vec4<f32>(0.0);
}

@vertex
fn vs_ground(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 1.0);
}

@fragment
fn fs_ground(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    let c = rel_at(px);
    if (c.w <= 0.0) { discard; }
    let rel = c.xyz;
    // Normal from the depth buffer, using the closer neighbor on each axis (clean edges).
    let r = rel_at(px + vec2<i32>(1, 0)).xyz - rel;
    let l = rel - rel_at(px - vec2<i32>(1, 0)).xyz;
    let d = rel_at(px + vec2<i32>(0, 1)).xyz - rel;
    let u = rel - rel_at(px - vec2<i32>(0, 1)).xyz;
    let dx = select(l, r, dot(r, r) < dot(l, l));
    let dy = select(u, d, dot(d, d) < dot(u, u));
    // At the screen edge a neighbor is missing and the normal can't be found.
    let cr = cross(dy, dx);
    if (dot(cr, cr) < 1e-20) { discard; }
    var n = normalize(cr);
    if (dot(n, -rel) < 0.0) { n = -n; }

    let uv = (rel.xz - fx.map.xy) / fx.map.z;
    if (any(uv <= vec2(0.0)) || any(uv >= vec2(1.0))) { discard; }
    let cov = textureSampleLevel(cover, linear_clamp, uv, 0.0);
    let pos = tiling_pos(rel).xz;
    let ground = above_ground(rel);
    let snow_m = cov.b * fx.params.y;

    // Snow buries whatever stands lower than its surface (grass, the foot of a post) on
    // ground flat enough to hold it. The surface drifts, so the taller blades poke out.
    var buried = 0.0;
    if (snow_m > 0.01 && ground.x > 0.02 && ground.y < 0.6) {
        let surface = snow_m * (0.7 + 0.6 * value_noise2(pos * 1.3));
        buried = 1.0 - smoothstep(surface * 0.8, surface, ground.x);
    }
    if (n.y < 0.75 && buried < 0.01) { discard; }
    let flat_ground = smoothstep(0.85, 0.97, n.y);

    // Standing water: the shore runs where the simulated water depth meets the ground's small
    // unevenness (noise). The depth map is smooth between its 1 m cells, so the shore sits
    // part way up the depth, not at its thin edge, which would spread every puddle into the
    // cells around it. Water stands only once the dips hold a few millimeters, and not under snow.
    let shore = fbm2(pos * 0.45, 4) - 0.5 + (value_noise2(pos * 3.1) - 0.5) * 0.25;
    let depth_mm = cov.g * fx.rain.w * 1000.0;
    let level = (depth_mm - 5.0 + shore * 8.0) / 3.0;
    let snowed = saturate(cov.b * fx.params.y * 40.0);
    // Water stands on the ground itself, not on grass blades, benches or tabletops.
    let on_ground = 1.0 - smoothstep(0.04, 0.08, ground.x);
    let water = smoothstep(0.0, 0.1, level) * flat_ground * (1.0 - snowed) * on_ground;
    // A dark rim of soaked ground around the water, also once the ground around it dries.
    let damp = smoothstep(-1.2, 0.0, level);
    // Wetness and snow the renderer didn't already give the surface: it shades materials that
    // take weather from the same ground state, per pixel (`surface_weather`).
    let applied = surface_weather(rel);
    let extra_wet = saturate(max(cov.r, damp) - applied.x);
    let applied_snow = saturate(applied.z * SURFACE_SNOW_SCALE_M * 10.0);
    let lying = saturate(saturate(snow_m * 10.0) - applied_snow) * smoothstep(0.75, 0.9, n.y);
    let extra_snow = max(lying, buried);
    if (water < 0.003 && extra_wet < 0.01 && extra_snow < 0.01) { discard; }

    let base = textureLoad(scene_color, px, 0).rgb;
    var col = base * (1.0 - 0.4 * extra_wet);

    if (water > 0.003) {
        var tilt = vec2<f32>(0.0);
        let view = normalize(rel);
        if (fx.rain.z > 0.5 && fx.rain.x > 0.0) {
            // Pixel size on the ground, stretched along the view at grazing angles.
            let pixel = 2.0 / (frame.proj[1][1] * frame.viewport.y);
            let footprint = length(rel) * pixel / max(abs(view.y), 0.08);
            tilt = ripples(pos, fx.rain.y, footprint) * fx.rain.x;
        }
        let wn = normalize(vec3<f32>(-tilt.x, 1.0, -tilt.y));
        let refl = reflect(view, wn);
        let cos_v = saturate(dot(-view, wn));
        let fresnel = 0.02 + 0.98 * pow(1.0 - cos_v, 5.0);
        // The sky (from the environment map) where the reflection leaves the screen, dimmed
        // under cover.
        var reflection = pre_expose(sample_env(refl)) * sky_map_visibility(rel, vec3<f32>(0.0, 1.0, 0.0));
        let steps = i32(fx.params.x);
        if (steps > 0) {
            let hit = reflect_scene(rel + wn * 0.02, refl, steps);
            reflection = mix(reflection, hit.rgb, hit.a);
        }
        // Under the film the ground is soaked dark.
        let under = base * 0.55;
        let puddle = mix(under, reflection, saturate(fresnel * 1.15));
        col = mix(col, puddle, water);
    }

    if (extra_snow > 0.01) {
        // Buried pixels show the snow's own surface, which faces up.
        let sn = normalize(mix(n, vec3<f32>(0.0, 1.0, 0.0), buried));
        let sun = frame.sun_camera.rgb * saturate(dot(sn, frame.sun_dir.xyz)) * shadow_factor(rel, sn) * INV_PI;
        let lit = (sky_ambient(sn) * sky_map_visibility(rel, sn) + sun) * 0.9;
        let grain = 0.85 + 0.15 * value_noise2(pos * 6.0);
        col = mix(col, pre_expose(lit) * grain, extra_snow);
    }
    return vec4<f32>(min(col, vec3(60000.0)), 1.0);
}
