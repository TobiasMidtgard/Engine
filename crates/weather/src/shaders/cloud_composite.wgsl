// Puts the low-resolution cloud march on screen: over the sky (with the sun disc, the moon
// and the stars drawn at full resolution), and over geometry the clouds are in front of
// (mountains reaching into them, the camera flying through them).
// Requires lib_types + lib_frame.

struct Celestial {
    sun: vec4<f32>,        // direction toward the sun, angular radius (rad)
    sun_color: vec4<f32>,  // color × illuminance (lux, not dimmed by cloud), sky yaw (rad)
    moon: vec4<f32>,       // direction toward the moon, angular radius (rad)
    moon_color: vec4<f32>, // color × luminance of the full disc (cd/m²), earthshine
    pole: vec4<f32>,       // celestial north pole direction, sidereal angle (rad)
    params: vec4<f32>,     // star brightness (0 by day), rainbow sunlight (lux), march texel size (uv)
};

@group(1) @binding(0) var<uniform> sky: Celestial;
@group(1) @binding(1) var cloud_color: texture_2d<f32>;
@group(1) @binding(2) var cloud_depth: texture_2d<f32>;
@group(1) @binding(3) var clear_sky: texture_2d<f32>;
@group(1) @binding(4) var scene_depth: texture_depth_2d;

fn camera_dir(uv: vec2<f32>) -> vec3<f32> {
    if (frame.exposure.w > 0.5) {
        return normalize(-frame.inv_view[2].xyz);
    }
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
    let p = frame.inv_view_proj * ndc;
    return normalize(p.xyz / p.w);
}

// Transmittance of the air from the camera out along `dir` (0 below the horizon).
fn view_transmittance(dir: vec3<f32>) -> vec3<f32> {
    if (frame.planet.w <= 0.0) { return vec3(1.0); }
    let od = optical_depth_to_sun(vec3(0.0), frame.camera_world.w, dir, 6);
    if (od.x < 0.0) { return vec3(0.0); }
    return exp(-extinction(od));
}

// Rotate `v` around unit axis `k` by `angle`.
fn rotate(v: vec3<f32>, k: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return v * c + cross(k, v) * s + k * dot(k, v) * (1.0 - c);
}

// Stars: at most one per cell of a grid around the sky, each drawn as a soft dot at least
// a pixel wide so none flicker out between pixels. Its flux follows a magnitude drawn from
// the real distribution (about three times as many stars per fainter magnitude), so a few
// bright ones stand out over thousands of faint ones. cd/m², not pre-exposed.
fn star_field(dir: vec3<f32>) -> vec3<f32> {
    let pixel = 2.0 / (frame.proj[1][1] * frame.viewport.y);
    let sigma = max(pixel * 0.6, 3e-4);
    let scale = 90.0;
    let p = dir * scale;
    let base = floor(p);
    // The dot can spill into the next cell toward which this point leans.
    let lean = select(vec3<f32>(-1.0), vec3<f32>(1.0), fract(p) > vec3(0.5));
    var c = vec3<f32>(0.0);
    for (var k = 0u; k < 8u; k++) {
        let cell = base + lean * vec3<f32>(f32(k & 1u), f32((k >> 1u) & 1u), f32((k >> 2u) & 1u));
        let h = hash13(cell);
        if (h < 0.9) { continue; }
        let u = (h - 0.9) / 0.1;
        let offset = vec3<f32>(hash13(cell + 17.3), hash13(cell + 41.9), hash13(cell + 73.1));
        let star = normalize(cell + 0.2 + 0.6 * offset);
        let a = length(dir - star);
        if (a > sigma * 4.0) { continue; }
        // Magnitude 6.5 at the faint end, a handful brighter than 0.
        let magnitude = 6.5 + 2.0 * log2(max(u, 1e-4)) * 0.30103;
        let flux = 2.5e-6 * exp2(-1.3288 * magnitude);
        let twinkle = 0.8 + 0.2 * sin(frame.time.y * (3.0 + u * 7.0) + h * 400.0);
        // Hot blue-white stars and cool orange ones.
        let t = fract(h * 731.0);
        let tint = mix(vec3(1.0, 0.78, 0.55), vec3(0.78, 0.86, 1.0), t);
        c += tint * flux * twinkle * exp(-0.5 * a * a / (sigma * sigma)) / (TAU * sigma * sigma);
    }
    return c;
}

// The Milky Way: a faint, mottled band across the sky with dark dust lanes. cd/m².
fn milky_way(dir: vec3<f32>) -> vec3<f32> {
    let galactic_pole = normalize(sky.pole.xyz * 0.456 + vec3<f32>(0.89, 0.0, 0.0));
    let b = dot(dir, galactic_pole);
    let band = exp(-b * b / (2.0 * 0.12 * 0.12));
    if (band < 0.01) { return vec3(0.0); }
    let along = normalize(cross(galactic_pole, vec3<f32>(0.0, 0.0, 1.0)));
    let l = atan2(dot(dir, cross(galactic_pole, along)), dot(dir, along));
    let q = vec2<f32>(l * 6.0, b * 30.0);
    let clouds = fbm2(q, 5);
    let dust = smoothstep(0.45, 0.7, fbm2(q * 1.7 + vec2(13.1, 7.7), 4)) * exp(-b * b / (2.0 * 0.04 * 0.04));
    // Brighter toward the galactic center.
    let core = 0.5 + 0.5 * sq(saturate(cos(l) * 0.5 + 0.5));
    return vec3<f32>(0.85, 0.9, 1.0) * 6e-4 * band * core * (0.4 + 1.2 * clouds) * (1.0 - 0.8 * dust);
}

// Sun disc, moon with its phase, and the turning star field (pre-exposed).
fn celestial(dir: vec3<f32>) -> vec3<f32> {
    var c = vec3(0.0);
    let sun = sky.sun.xyz;
    let cos_sun = cos(sky.sun.w);
    let cd = dot(dir, sun);
    let moon = sky.moon.xyz;
    let sin_moon = sin(sky.moon.w);
    let md = dot(dir, moon);
    let in_moon = md > cos(sky.moon.w);
    if (cd <= cos_sun && !in_moon && sky.params.x <= 0.0) { return c; }
    let tr = view_transmittance(dir);
    if (cd > cos_sun) {
        let solid_angle = PI * sky.sun.w * sky.sun.w;
        let mu = saturate((cd - cos_sun) / (1.0 - cos_sun));
        let limb = 0.4 + 0.6 * sqrt(mu);
        c += sky.sun_color.rgb / solid_angle * tr * limb;
    }
    if (in_moon) {
        // Point on the moon's sphere this pixel sees; lit where it faces the sun.
        let o = (dir - moon * md) / sin_moon;
        let n = o - moon * sqrt(saturate(1.0 - dot(o, o)));
        let lit = smoothstep(-0.05, 0.1, dot(n, sun));
        // Dark maria and bright highlands.
        let side = normalize(cross(moon, vec3<f32>(0.0, 1.0, 0.0001)));
        let uv = vec2<f32>(dot(o, side), dot(o, cross(side, moon)));
        let maria = 0.72 + 0.28 * smoothstep(0.35, 0.65, fbm2(uv * 3.0 + vec2(4.7, 1.3), 4));
        let edge = smoothstep(1.0, 0.96, length(o));
        // The night exposure would burn the moon to a white blob; the eye sees its face,
        // so its brightness rolls off a few times above the scene's white.
        let moon_light = pre_expose(sky.moon_color.rgb * (lit + sky.moon_color.w) * maria * tr);
        c = pre_expose(c) + moon_light * 6.0 / (6.0 + max(moon_light.r, max(moon_light.g, moon_light.b))) * edge;
        return c;
    } else if (sky.params.x > 0.0) {
        // Into the sky's own axes (they turn when the world's frame turns), then the
        // turning of the Earth.
        let sky_dir = rotate(dir, vec3<f32>(0.0, 1.0, 0.0), sky.sun_color.w);
        let star_dir = rotate(sky_dir, sky.pole.xyz, -sky.pole.w);
        c += (star_field(star_dir) + milky_way(star_dir)) * sky.params.x * tr;
    }
    return pre_expose(c);
}

// One wavelength band of a rainbow: angle from the antisolar point where it peaks (deg) and
// the color it adds (columns sum to white).
struct Band {
    angle: f32,
    color: vec3<f32>,
};

fn rainbow_band(i: i32) -> Band {
    // 410, 450, 490, 530, 570, 610, 660 nm. The primary bow's angle runs from 40.6° (violet)
    // to 42.3° (red); the secondary's from 53.4° (violet) to 50.4° (red).
    var angle = array<f32, 7>(40.65, 40.95, 41.25, 41.55, 41.8, 42.05, 42.3);
    var color = array<vec3<f32>, 7>(
        vec3(0.35, 0.0, 0.6), vec3(0.0, 0.1, 1.0), vec3(0.0, 0.6, 0.6), vec3(0.1, 1.0, 0.1),
        vec3(0.7, 0.8, 0.0), vec3(1.0, 0.35, 0.0), vec3(0.9, 0.0, 0.0),
    );
    var b: Band;
    b.angle = angle[i];
    b.color = color[i] / vec3(3.05, 2.85, 2.3);
    return b;
}

// Rainbow radiance (cd/m² per lux of sunlight on the rain) seen in direction `dir`: the
// primary bow with its bright inside, the fainter reversed secondary, and the dark band
// between them. The sun's disc and the drop sizes blur each color by about half a degree.
fn rainbow(dir: vec3<f32>, sun: vec3<f32>) -> vec3<f32> {
    let theta = degrees(acos(clamp(dot(dir, -sun), -1.0, 1.0)));
    if (theta > 56.0 || theta < 25.0) { return vec3(0.0); }
    var c = vec3<f32>(0.0);
    for (var i = 0; i < 7; i++) {
        let b = rainbow_band(i);
        // Primary: a sharp outer edge, light spilling inward.
        let p = theta - b.angle;
        var primary = exp(-p * p / (2.0 * 0.35 * 0.35));
        if (p < 0.0) { primary = max(primary, 0.12 * exp(p / 6.0)); }
        // Secondary: reversed colors, a sharp inner edge, about 40% as bright.
        let s_angle = 50.4 + (42.3 - b.angle) * (3.0 / 1.65);
        let q = theta - s_angle;
        var secondary = exp(-q * q / (2.0 * 0.5 * 0.5));
        if (q > 0.0) { secondary = max(secondary, 0.1 * exp(-q / 5.0)); }
        c += b.color * (primary + 0.4 * secondary);
    }
    return c * 0.05 * INV_PI;
}

// Clouds sampled with a small tent filter (hides the march's per-pixel jitter).
fn clouds_at(uv: vec2<f32>) -> vec4<f32> {
    let o = sky.params.zw * 0.5;
    var c = textureSampleLevel(cloud_color, linear_clamp, uv + vec2(-o.x, -o.y), 0.0);
    c += textureSampleLevel(cloud_color, linear_clamp, uv + vec2(o.x, -o.y), 0.0);
    c += textureSampleLevel(cloud_color, linear_clamp, uv + vec2(-o.x, o.y), 0.0);
    c += textureSampleLevel(cloud_color, linear_clamp, uv + vec2(o.x, o.y), 0.0);
    return c * 0.25;
}

// The renderer's fog in front of the sky (pre-exposed `color` seen along `dir` at screen
// `pixel`): its height fog beyond the volumetric range, shaded by the mountains' haze shadows
// like geometry's, then the volumetric fog in front of that. Geometry gets the height fog out
// to any distance, so without the far part a fogged hillside ended in a hard bright wall under
// a dark sky; with it the fog thins upward into the sky as it does outside.
fn fog_over_sky(color: vec3<f32>, dir: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    var c = color;
    let start = max(frame.volumetrics.x, 0.0);
    let far = 100000.0;
    let tr = exp(-height_fog_optical_depth(dir, start, far));
    if (tr < 0.9999) {
        let sun = height_fog_sun_visibility(dir, start, far, tr, frame_noise(pixel));
        c = c * tr + pre_expose(height_fog_radiance(dir, dir * start, 1.0, sun)) * (1.0 - tr);
    }
    return apply_sky_fog(c, dir);
}

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

// Sky pixels: clear sky + sun/moon/stars, clouds in front, the renderer's fog in front of all
// (replaces the sky's own picture).
@fragment
fn fs_sky(in: FullscreenOut) -> @location(0) vec4<f32> {
    let dir = camera_dir(in.uv);
    let cl = clouds_at(in.uv);
    var bg = textureSampleLevel(clear_sky, linear_clamp, in.uv, 0.0).rgb;
    bg += celestial(dir);
    var c = cl.rgb + bg * cl.a;
    // A rainbow on sunlit rain opposite a low sun.
    if (sky.params.y > 0.0) {
        let rain = 1.0 - exp(-textureSampleLevel(cloud_depth, linear_clamp, in.uv, 0.0).z);
        c += pre_expose(rainbow(dir, sky.sun.xyz) * sky.params.y) * rain;
    }
    c = fog_over_sky(c, dir, in.position.xy);
    return vec4<f32>(min(c, vec3(60000.0)), 1.0);
}

// Sky pixels when the sky draws no clouds of its own: blend the clouds over it.
@fragment
fn fs_sky_over(in: FullscreenOut) -> @location(0) vec4<f32> {
    let cl = clouds_at(in.uv);
    var c = cl.rgb;
    if (sky.params.y > 0.0) {
        let rain = 1.0 - exp(-textureSampleLevel(cloud_depth, linear_clamp, in.uv, 0.0).z);
        c += pre_expose(rainbow(camera_dir(in.uv), sky.sun.xyz) * sky.params.y) * rain;
    }
    return vec4<f32>(min(c, vec3(60000.0)), cl.a);
}

@vertex
fn vs_geo(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 1.0);
}

// How much rain the march counts out to `t` meters: it fades the rain in from 400 m to 2.5 km
// (up close the rain is particles), so this is the integral of that fade from 400 m.
fn rain_reach(t: f32) -> f32 {
    let x = saturate((t - 400.0) / 2100.0);
    return 2100.0 * (x * x * x - 0.5 * x * x * x * x) + max(t - 2500.0, 0.0);
}

// Geometry pixels: only the part of the rain and cloud in front of the surface. A hill inside
// the rain is veiled by the rain between it and the camera, toward the color of the sky right
// behind it, so in a downpour the far hills and mountains fade into the grey instead of
// standing crisp in front of the curtains.
@fragment
fn fs_geo(in: FullscreenOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    let d = textureLoad(scene_depth, px, 0);
    if (d <= 0.0) { discard; }
    let ndc = vec4<f32>(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0, d, 1.0);
    let p = frame.inv_view_proj * ndc;
    let dist = length(p.xyz / p.w);
    let depth = textureSampleLevel(cloud_depth, linear_clamp, in.uv, 0.0);
    // The rain's optical depth reached at the surface. The march gives where 10% and 90% of
    // it is reached; between them the rain is about even along the ray, and showers beyond
    // the hill don't veil it.
    var veil = 0.0;
    if (depth.z > 0.0 && dist > 400.0) {
        let front = rain_reach(depth.w * 1000.0);
        let back = rain_reach(textureSampleLevel(clear_sky, linear_clamp, in.uv, 0.0).a * 1000.0);
        let share = saturate(0.1 + 0.8 * (rain_reach(dist) - front) / max(back - front, 1.0));
        veil = 1.0 - exp(-depth.z * share);
    }
    // The part of the cloud between its front and back (mountains reaching into it).
    let in_cloud = saturate((dist - depth.x * 1000.0) / max((depth.y - depth.x) * 1000.0, 1.0));
    if (veil <= 0.0 && in_cloud <= 0.0) { discard; }
    let cl = clouds_at(in.uv);
    // Share of everything the sky shows in front of the surface: the rain's, then the cloud's.
    let k_rain = saturate(veil / max(1.0 - cl.a, 1e-3));
    let k = k_rain + (1.0 - k_rain) * in_cloud;
    // The surface already has the renderer's fog over it; what this pass puts in front lies
    // behind that fog too (as the sky's clouds do, `fog_over_sky`). Without it far hills in
    // night rain faded to the unfogged black deck under a glowing fogged sky.
    let fog = surface_fog(p.xyz / p.w);
    let alpha = 1.0 - (1.0 - cl.a) * k;
    let rgb = cl.rgb * k * fog.transmittance + fog.inscatter * (1.0 - alpha);
    return vec4<f32>(min(rgb, vec3(60000.0)), alpha);
}
