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
    veil: vec4<f32>,       // falling rain or snow veils from x (m), fully from y (m) (as the march)
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

// The renderer's fog in front of the sky (`apply_full_sky_fog`) only out to `far` meters along
// `dir`: what lies in front of rain or snow thick enough to hide the rest. Built the same way:
// the volumetric fog to its range, then the analytic height fog, shaded by the far shadow map
// and the cloud shadows, then `limit_fog_opacity`. Its light in rgb, what it lets through in
// alpha; no air, which the march already put over its clouds and rain.
fn fog_in_front(dir: vec3<f32>, pixel: vec2<f32>, far: f32) -> vec4<f32> {
    var end = far;
    if (frame.planet.w > 0.0) {
        let ground = ray_shell_mu(frame.camera_world.w, dot(dir, normalize(-frame.planet.xyz)), 0.0);
        if (ground.x > 0.0) { end = min(end, ground.x); }
    }
    var fog = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var start = 0.0;
    if (frame.volumetrics.x > 0.0) {
        // The range is a view depth: toward the screen's edges it ends further along the ray.
        let depth = max(view_depth(dir), 1e-3);
        fog = volumetric_fog_at(dir, min(frame.volumetrics.x, end * depth));
        start = frame.volumetrics.x / depth;
    }
    if (frame.fog.x > 0.0 && start < end) {
        let tr = exp(-height_fog_optical_depth(dir, start, end));
        if (tr < 0.9999) {
            let sun = height_fog_sun_visibility(dir, start, end, tr, frame_noise(pixel));
            let l = pre_expose(height_fog_radiance(dir, dir * start, 1.0, sun)) * (1.0 - tr);
            fog = vec4<f32>(fog.rgb + l * fog.a, fog.a * tr);
        }
    }
    return limit_fog_opacity(fog);
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
    // Rain or snow thick enough to hide the distance hides the fog beyond it too: in front of
    // the veil lies the fog out to where it is half opaque. Without it a snowfall's grey sky
    // took the far fog's color and the hills, veiled by the snow, stood out against it.
    let depth = textureSampleLevel(cloud_depth, linear_clamp, in.uv, 0.0);
    let veil = 1.0 - exp(-depth.z);
    // The renderer's fog in front of all of it (the sky this replaces gets the same), so a
    // fogged hillside fades into a fogged sky instead of ending in a hard bright edge.
    var fogged = c;
    let w = veil_weight(veil);
    if (w < 1.0) { fogged = apply_full_sky_fog(c, dir, in.position.xy); }
    if (w > 0.0) {
        let back_km = textureSampleLevel(clear_sky, linear_clamp, in.uv, 0.0).a;
        let near = fog_in_front(dir, in.position.xy, rain_distance(half_opacity_depth(depth.z), depth, back_km));
        fogged = mix(fogged, c * near.a + near.rgb, w);
    }
    return vec4<f32>(min(fogged, vec3(60000.0)), 1.0);
}

// Sky pixels when the sky draws no clouds of its own: blend the clouds over it. The sky
// behind has the renderer's fog all the way out; rain or snow thick enough to hide it has
// only the fog in front of it, as in `fs_sky`.
@fragment
fn fs_sky_over(in: FullscreenOut) -> @location(0) vec4<f32> {
    let dir = camera_dir(in.uv);
    let cl = clouds_at(in.uv);
    let depth = textureSampleLevel(cloud_depth, linear_clamp, in.uv, 0.0);
    var c = cl.rgb;
    if (sky.params.y > 0.0) {
        c += pre_expose(rainbow(dir, sky.sun.xyz) * sky.params.y) * (1.0 - exp(-depth.z));
    }
    let w = veil_weight(1.0 - exp(-depth.z));
    if (w > 0.0) {
        let back_km = textureSampleLevel(clear_sky, linear_clamp, in.uv, 0.0).a;
        let near = fog_in_front(dir, in.position.xy, rain_distance(half_opacity_depth(depth.z), depth, back_km));
        c = mix(c, c * near.a + near.rgb * (1.0 - cl.a), w);
    }
    return vec4<f32>(min(c, vec3(60000.0)), cl.a);
}

// How much of a pixel's fog is the fog in front of its rain or snow, from the veil's
// opacity: none below 1%, all above 99% (where the far fog is not worth evaluating).
fn veil_weight(veil: f32) -> f32 {
    return saturate((veil - 0.01) / 0.98);
}

@vertex
fn vs_geo(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 1.0);
}

// How much rain the march counts out to `t` meters: it fades the rain in from `veil.x` to
// `veil.y` (400 m to 2.5 km for rain; up close the rain is particles), so this is the
// integral of that fade.
fn rain_reach(t: f32) -> f32 {
    let span = max(sky.veil.y - sky.veil.x, 1.0);
    let x = saturate((t - sky.veil.x) / span);
    return span * (x * x * x - 0.5 * x * x * x * x) + max(t - sky.veil.y, 0.0);
}

// The distance `rain_reach` counts `reach` meters of rain out to.
fn rain_reach_inverse(reach: f32) -> f32 {
    let span = max(sky.veil.y - sky.veil.x, 1.0);
    if (reach >= 0.5 * span) { return sky.veil.y + reach - 0.5 * span; }
    // Inside the fade x³ - x⁴/2 rises steadily over 0..1.
    var lo = 0.0;
    var hi = 1.0;
    for (var i = 0; i < 10; i++) {
        let m = 0.5 * (lo + hi);
        if (span * (m * m * m - 0.5 * m * m * m * m) < reach) { lo = m; } else { hi = m; }
    }
    return sky.veil.x + span * 0.5 * (lo + hi);
}

// The share of a ray's rain reached `reach` meters of rain out (`rain_reach`), from where the
// march found 10% (`front`) and 90% (`back`): even in between, as the march spreads each
// step's rain over its meters of rain, and the same slope before and after. In meters of rain
// that slope is exact for rain all the way out, and rain that begins far away leaves the ground
// in front of it clear.
fn rain_share(reach: f32, front: f32, back: f32) -> f32 {
    return saturate(0.1 + 0.8 * (reach - front) / max(back - front, 1.0));
}

// Where the rain along this pixel's ray reaches optical depth `tau` (m): `depth` and `back_km`
// are the march's outputs (total optical depth, 10% and 90% points). Inverts `rain_share`.
fn rain_distance(tau: f32, depth: vec4<f32>, back_km: f32) -> f32 {
    let front = rain_reach(depth.w * 1000.0);
    let back = rain_reach(back_km * 1000.0);
    let reach = front + (tau / max(depth.z, 1e-6) - 0.1) / 0.8 * max(back - front, 1.0);
    return rain_reach_inverse(max(reach, 0.0));
}

// The optical depth at which a veil of optical depth `tau` has reached half its opacity: most
// of its light comes from in front of that (ln 2 once it is opaque).
fn half_opacity_depth(tau: f32) -> f32 {
    return -log(0.5 * (1.0 + exp(-tau)));
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
    var veil_mid = dist;
    if (depth.z > 0.0 && dist > sky.veil.x) {
        let back_km = textureSampleLevel(clear_sky, linear_clamp, in.uv, 0.0).a;
        let front = rain_reach(depth.w * 1000.0);
        let back = rain_reach(back_km * 1000.0);
        let tau = depth.z * rain_share(rain_reach(dist), front, back);
        veil = 1.0 - exp(-tau);
        veil_mid = min(dist, rain_distance(half_opacity_depth(tau), depth, back_km));
    }
    // The part of the cloud between its front and back (mountains reaching into it).
    let in_cloud = saturate((dist - depth.x * 1000.0) / max((depth.y - depth.x) * 1000.0, 1.0));
    if (veil <= 0.0 && in_cloud <= 0.0) { discard; }
    let cl = clouds_at(in.uv);
    // Share of everything the sky shows in front of the surface: the rain's, then the cloud's.
    let k_rain = saturate(veil / max(1.0 - cl.a, 1e-3));
    let k = k_rain + (1.0 - k_rain) * in_cloud;
    // The surface already has the renderer's fog over it; what this pass puts in front lies
    // behind that fog too (as the sky's clouds do, `apply_full_sky_fog`). Without it far hills in
    // night rain faded to the unfogged black deck under a glowing fogged sky. The rain's light
    // only lies behind the fog out to where the rain is half opaque, as over the sky: fogged
    // all the way to a hill kilometers away, snow hid the hill in tan. The march already put
    // the air over its rain and clouds, so this is the fog alone.
    let dir = p.xyz / (p.w * dist);
    let k_cloud = k - k_rain;
    var through = 0.0;
    var inscatter = vec3(0.0);
    if (k_rain > 1e-4) {
        let f = fog_in_front(dir, in.position.xy, veil_mid);
        through += k_rain * f.a;
        inscatter += k_rain * f.rgb;
    }
    if (k_cloud > 1e-4) {
        let f = fog_in_front(dir, in.position.xy, dist);
        through += k_cloud * f.a;
        inscatter += k_cloud * f.rgb;
    }
    let alpha = 1.0 - (1.0 - cl.a) * k;
    let rgb = cl.rgb * through + inscatter * (1.0 - cl.a);
    return vec4<f32>(min(rgb, vec3(60000.0)), alpha);
}
