// Volumetric clouds: ray-marched through a layer on the planet's curved shell, shaped by the
// weather map (coverage, cloud type, rain) and 3D noise, lit by the sun or moon with
// multiple-scattering approximations, lightning from inside, and the sky's ambient light.
// Requires lib_types + lib_frame.

struct Clouds {
    layer: vec4<f32>,       // base altitude (m), top altitude (m), extinction (1/m at density 1), max distance (m)
    map: vec4<f32>,         // weather map min corner relative to the camera (x, z), map size (m), 1 if valid
    noise: vec4<f32>,       // noise origin xyz (world minus drift, wrapped), shape noise period (m)
    light: vec4<f32>,       // key light color × illuminance (lux, not dimmed by cloud), march steps
    light_dir: vec4<f32>,   // toward the key light, light steps
    background: vec4<f32>,  // coverage, type, precipitation (0..1), density beyond the map
    flash: vec4<f32>,       // lightning: position relative to the camera, luminous intensity (cd)
    params: vec4<f32>,      // visibility (fades out high above the planet), detail period (m), env steps, night (0 day .. 1 dark night)
    detail: vec4<f32>,      // detail noise origin xyz (wrapped), 1 = draw rain shafts
    veil: vec4<f32>,        // falling rain or snow veils from x (m), fully from y (m); z = snow share 0..1
    shadow: vec4<f32>,      // cloud shadow map: the shading light's dimming at the player, samples toward the light
    far: vec4<f32>,         // far cloud layer drawn by another module: hand over from x to y m of ground distance, z = its weight
};

@group(1) @binding(0) var<uniform> clouds: Clouds;
@group(1) @binding(1) var shape_noise: texture_3d<f32>;
@group(1) @binding(2) var detail_noise: texture_3d<f32>;
@group(1) @binding(3) var weather_map: texture_2d<f32>;

fn remap(v: f32, l0: f32, h0: f32, l1: f32, h1: f32) -> f32 {
    return l1 + (v - l0) * (h1 - l1) / (h0 - l0);
}

// Layer thickness for a cloud type (matches `cloud_thickness_m` in presets.rs).
fn type_thickness(t: f32) -> f32 {
    if (t < 0.5) { return 450.0 + 1350.0 * (t / 0.5); }
    let u = (t - 0.5) / 0.5;
    return 1800.0 + 8200.0 * u * u;
}

fn map_uv(xz: vec2<f32>) -> vec3<f32> {
    let uv = (xz - clouds.map.xy) / clouds.map.z;
    let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    return vec3<f32>(uv, edge);
}

// Coverage, type, precipitation, density at a camera-relative X/Z (bilinear: light samples).
fn weather_at(xz: vec2<f32>) -> vec4<f32> {
    let u = map_uv(xz);
    if (clouds.map.w < 0.5 || u.z <= 0.0) { return clouds.background; }
    let m = textureSampleLevel(weather_map, linear_clamp, u.xy, 0.0);
    return mix(clouds.background, m, smoothstep(0.0, 0.08, u.z));
}

// Same, with a cubic B-spline filter (four bilinear taps) so the map's texel grid never
// shows as creases in the cloud field.
fn weather_at_smooth(xz: vec2<f32>) -> vec4<f32> {
    let u = map_uv(xz);
    if (clouds.map.w < 0.5 || u.z <= 0.0) { return clouds.background; }
    let size = vec2<f32>(textureDimensions(weather_map));
    let p = u.xy * size - 0.5;
    let i = floor(p);
    let f = p - i;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (1.0 - 3.0 * f + 3.0 * f2 - f3) / 6.0;
    let w1 = (4.0 - 6.0 * f2 + 3.0 * f3) / 6.0;
    let w2 = (1.0 + 3.0 * f + 3.0 * f2 - 3.0 * f3) / 6.0;
    let w3 = f3 / 6.0;
    let g0 = w0 + w1;
    let g1 = w2 + w3;
    let h0 = (i - 0.5 + w1 / g0) / size;
    let h1 = (i + 1.5 + w3 / g1) / size;
    let a = textureSampleLevel(weather_map, linear_clamp, vec2<f32>(h0.x, h0.y), 0.0);
    let b = textureSampleLevel(weather_map, linear_clamp, vec2<f32>(h1.x, h0.y), 0.0);
    let c = textureSampleLevel(weather_map, linear_clamp, vec2<f32>(h0.x, h1.y), 0.0);
    let d = textureSampleLevel(weather_map, linear_clamp, vec2<f32>(h1.x, h1.y), 0.0);
    let m = g0.y * (g0.x * a + g1.x * b) + g1.y * (g0.x * c + g1.x * d);
    return mix(clouds.background, m, smoothstep(0.0, 0.08, u.z));
}

// Vertical density profile for a cloud type: flat sheets, rounded heaps, tall towers.
fn height_profile(h01: f32, t: f32) -> f32 {
    let stratus = smoothstep(0.0, 0.2, h01) * (1.0 - smoothstep(0.45, 1.0, h01));
    let cumulus = smoothstep(0.0, 0.07, h01) * (1.0 - smoothstep(0.3, 1.0, h01));
    let tower = smoothstep(0.0, 0.04, h01) * (1.0 - smoothstep(0.85, 1.0, h01));
    if (t < 0.5) { return mix(stratus, cumulus, t * 2.0); }
    return mix(cumulus, tower, (t - 0.5) * 2.0);
}

// Cloud density (0..1, times the map's density) at camera-relative `p` with altitude `h`.
fn cloud_density(p: vec3<f32>, h: f32, w: vec4<f32>, with_detail: bool) -> f32 {
    if (w.r <= 0.002) { return 0.0; }
    let thickness = type_thickness(w.g);
    let h01 = (h - clouds.layer.x) / thickness;
    if (h01 <= 0.0 || h01 >= 1.0) { return 0.0; }
    let np = (p + clouds.noise.xyz) / clouds.noise.w;
    // Big lumps break up the map's smooth outlines (towers, cloud streets, gaps).
    let lump = textureSampleLevel(shape_noise, linear_repeat, np * 0.27 + vec3(0.37, 0.11, 0.73), 0.0).g;
    let tower = smoothstep(0.55, 0.9, w.g);
    // Towers narrow as they climb, then spread into an anvil under the tropopause.
    let anvil = smoothstep(0.75, 1.0, w.g) * smoothstep(0.72, 0.92, h01);
    var cov = w.r * (0.72 + 0.56 * lump) - tower * 0.3 * smoothstep(0.1, 0.7, h01) + anvil * 0.55 * smoothstep(0.4, 0.8, w.r);
    cov = saturate(cov);
    if (cov <= 0.002) { return 0.0; }
    let s = textureSampleLevel(shape_noise, linear_repeat, np, 0.0);
    // Heaped clouds end in cauliflower tops, not one smooth dome: the top sinks between
    // the billows.
    let billows = s.g * 0.7 + s.b * 0.3;
    let hb = h01 + smoothstep(0.35, 0.9, w.g) * 0.35 * (0.7 - billows);
    if (hb >= 1.0) { return 0.0; }
    // Raining decks (nimbostratus, the bases of storms) hang in ragged lumps instead of one
    // flat ceiling; fair-weather heaps keep their flat bases.
    let ragged = smoothstep(0.5, 0.9, w.r) * smoothstep(0.02, 0.15, w.b)
        * (1.0 - smoothstep(0.55, 0.85, w.g));
    let carve = ragged * (1.0 - billows) * 380.0 / thickness;
    let profile = height_profile(saturate(hb), w.g) * smoothstep(carve, carve + 0.06, h01);
    let fbm = s.g * 0.625 + s.b * 0.25 + s.a * 0.125;
    let shape = remap(s.r, fbm - 1.0, 1.0, 0.0, 1.0);
    var d = saturate(remap(shape * profile, 1.0 - cov, 1.0, 0.0, 1.0)) * cov;
    if (d <= 0.0) { return 0.0; }
    if (with_detail) {
        let dp = (p + clouds.detail.xyz) / clouds.params.y;
        let dn = textureSampleLevel(detail_noise, linear_repeat, dp, 0.0);
        let dfbm = dn.r * 0.625 + dn.g * 0.25 + dn.b * 0.125;
        // Wispy at the bottom, billowy at the top.
        let m = mix(dfbm, 1.0 - dfbm, saturate(h01 * 5.0));
        d = saturate(remap(d, m * 0.45, 1.0, 0.0, 1.0));
        // Real clouds end abruptly: droplets evaporate within meters of the edge. Lift the
        // thin fringe the noise leaves so billows get crisp outlines instead of a cotton-wool
        // blur; the cores keep their density.
        d = max(d, 0.3 * smoothstep(0.0, 0.1, d));
    }
    // Towers are far denser than fair-weather heaps: solid, hard-edged, dark underneath.
    return d * w.a * (1.0 + 1.5 * tower);
}

fn hg(cos_t: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * PI * pow(max(1.0 + g2 - 2.0 * g * cos_t, 1e-4), 1.5));
}

// Strong forward lobe (silver lining) plus a little back-scatter.
fn cloud_phase(cos_t: f32, k: f32) -> f32 {
    return mix(hg(cos_t, -0.2 * k), hg(cos_t, 0.8 * k), 0.75);
}

struct CloudResult {
    radiance: vec3<f32>,    // in-scattered radiance (cd/m², not pre-exposed)
    transmittance: f32,
    front: f32,             // distance where the cloud starts (m)
    back: f32,              // distance where it has become nearly opaque (m)
    rain: f32,              // optical depth of the rain shafts along the ray (rainbows, veiling
                            // hills; opacity would saturate long before a far hill does)
    rain_front: f32,        // where the rain shafts' optical depth reaches 10% and 90% of
    rain_back: f32,         // its total along the ray (m)
};

// How much of the clouds at camera-relative `p` to draw: another module's far cloud layer
// (space's cloud tops seen from high up) takes over beyond `clouds.far.x` of ground distance,
// fully by `clouds.far.y`, as much as it is drawn (`FarCloudLayer::near_share`).
fn near_share(p: vec3<f32>) -> f32 {
    if (clouds.far.z <= 0.0 || frame.planet.w <= 0.0) { return 1.0; }
    let a = normalize(p - frame.planet.xyz);
    let b = normalize(-frame.planet.xyz);
    let ground = frame.planet.w * asin(min(length(cross(a, b)), 1.0));
    let t = saturate((ground - clouds.far.x) / max(clouds.far.y - clouds.far.x, 1e-3));
    return 1.0 - clouds.far.z * t * t * (3.0 - 2.0 * t);
}

// How much more cloud than the light march samples shades a point (see march_clouds).
const SELF_SHADOW: f32 = 2.0;

// Interleaved gradient noise: a per-pixel start offset that hides banding. It moves every
// frame (Jimenez's 5.588238 px step) so the renderer's TAA averages the offsets away; a fixed
// pattern left diagonal stripes and wood-grain contours across smooth storm decks.
fn ign(px: vec2<f32>) -> f32 {
    let p = px + 5.588238 * f32(u32(frame.time.w) % 64u);
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// Transmittance of the air above camera-relative `rel` toward the clouds' key light. Not
// `sun_transmittance`: that follows the renderer's brightest light, which can be the other
// one (the moon under a storm's dimmed sun at dusk).
fn key_transmittance(rel: vec3<f32>) -> vec3<f32> {
    if (frame.planet.w <= 0.0) { return vec3(1.0); }
    let h = planet_height(rel);
    let mu = dot(normalize(rel - frame.planet.xyz), clouds.light_dir.xyz);
    return transmittance_to_space(h, mu) * sun_above_horizon(h, mu);
}

fn march_clouds(dir: vec3<f32>, steps: i32, light_steps: i32, jitter: f32) -> CloudResult {
    var r: CloudResult;
    r.radiance = vec3(0.0);
    r.transmittance = 1.0;
    r.front = clouds.layer.w;
    r.back = clouds.layer.w;
    if (clouds.params.x <= 0.001 || frame.planet.w <= 0.0) { return r; }

    let h0 = frame.camera_world.w;
    let base = clouds.layer.x;
    let top = clouds.layer.y;
    let inner = ray_shell(vec3(0.0), h0, dir, base);
    let outer = ray_shell(vec3(0.0), h0, dir, top);
    var t0 = 0.0;
    var t1 = 0.0;
    if (h0 < base) {
        if (ray_shell(vec3(0.0), h0, dir, 0.0).x > 0.0) { return r; }
        t0 = inner.y;
        t1 = outer.y;
    } else if (h0 > top) {
        if (outer.x <= 0.0) { return r; }
        t0 = outer.x;
        t1 = select(outer.y, inner.x, inner.x > 0.0);
    } else {
        t1 = select(outer.y, inner.x, inner.x > 0.0);
    }
    t1 = min(t1, clouds.layer.w);
    if (t1 <= t0) { return r; }

    let ldir = clouds.light_dir.xyz;
    let cos_t = dot(dir, ldir);
    let extinction = clouds.layer.z;
    // Light arriving at the clouds: the key light through the air above the entry point.
    let entry = dir * t0;
    let key = clouds.light.rgb * key_transmittance(entry + vec3(0.0, 50.0, 0.0));
    // Ambient: the clear sky above the clouds, the lit ground below.
    let sky = atmosphere(entry, vec3<f32>(0.0, 1.0, 0.0), 1e12, 6).radiance * clouds.light.rgb * 1.4;
    // Light bounced off the ground below. The ground's color only tints it (a meadow doesn't
    // turn the cloud base green), and under the deck itself the ground is in the cloud's
    // shadow (scaled per sample by the cover there).
    let albedo = weathered_ground();
    let ground_tint = mix(vec3(dot(albedo, vec3<f32>(0.2126, 0.7152, 0.0722))), albedo, 0.4);
    let ground_sun = ground_tint * key * saturate(ldir.y) * INV_PI;
    let ground_sky = ground_tint * sky * 0.4;
    let fill = frame.ambient.rgb;

    // Big strides through empty air; on touching cloud, step back and walk it finely.
    let len = t1 - t0;
    let coarse_dt = clamp(len / f32(steps) * 1.5, 40.0, 1200.0);
    let fine_dt = clamp(len / f32(steps) * 0.5, 20.0, 300.0);
    var t = t0 + coarse_dt * jitter;
    var fine = false;
    var empty = 0;
    var hit = false;
    for (var i = 0; i < steps * 2; i++) {
        if (t >= t1) { break; }
        let p = dir * t;
        let h = planet_height(p);
        let w = weather_at_smooth(p.xz);
        let coarse = cloud_density(p, h, w, false);
        if (coarse <= 0.0) {
            if (fine) {
                empty += 1;
                if (empty > 8) { fine = false; }
                t += fine_dt;
            } else {
                t += coarse_dt;
            }
            continue;
        }
        if (!fine) {
            fine = true;
            empty = 0;
            t = max(t - coarse_dt + fine_dt * jitter, t0);
            continue;
        }
        empty = 0;
        let dt = fine_dt;
        let d = cloud_density(p, h, w, true);
        let share = near_share(p);
        if (d > 0.0 && share > 0.0) {
            let sigma = d * extinction;
            // Optical depth toward the light over samples that double in length, 1.24 km in
            // all however many there are (5 on screen: 40 m to 640 m), so the environment
            // map's cheaper 2-sample march sees as much cloud toward the light as the screen
            // does (it used to reach only 120 m).
            let first = 1240.0 / (exp2(f32(light_steps)) - 1.0);
            var od = 0.0;
            var lt = 0.0;
            for (var j = 0; j < light_steps; j++) {
                let ls = first * exp2(f32(j));
                let q = p + ldir * (lt + ls * 0.5);
                lt += ls;
                od += cloud_density(q, planet_height(q), weather_at(q.xz), j < 2) * ls;
            }
            // Two coarse samples further out: towers kilometers thick shade themselves.
            for (var j = 0; j < 2; j++) {
                let ls = 1500.0 * f32(j + 1);
                let q = p + ldir * (lt + ls * 0.5);
                lt += ls;
                od += cloud_density(q, planet_height(q), weather_at(q.xz), false) * ls * 0.7;
            }
            od *= extinction;
            // Single scattering (strong forward lobe: silver linings) plus multiple scattering
            // as octaves that each reach deeper, scatter less and lose direction (Wrenninge
            // et al.): sunlit sides bright, shaded sides several times darker but never black.
            // Clouds scatter ~all light they absorb (albedo ≈ 1), so the higher orders carry
            // most of the energy of a thick cloud.
            // The march toward the light skips most detail noise and thins its far samples, so
            // it sees about half the cloud really there: without the doubling, heaps lit from
            // the side or seen from above looked evenly bright, with no shaded flanks.
            let shade = od * SELF_SHADOW;
            var light = exp(-shade) * cloud_phase(cos_t, 1.0);
            var oa = 0.25;
            var ob = 1.2;
            var oc = 0.5;
            for (var o = 1; o < 4; o++) {
                light += ob * cloud_phase(cos_t, oc) * exp(-shade * oa);
                oa *= 0.25;
                ob *= 0.6;
                oc *= 0.5;
            }
            // Deep in a thick cloud the octaves die out exponentially, but sunlight still
            // diffuses down through it like through frosted glass, white and without direction:
            // two-stream transmittance 1 / (1 + 0.75 (1 - g) τ), g ≈ 0.85. Without it a storm
            // deck's base was lit only by the blue sky and turned navy.
            light = max(light, saturate(ldir.y + 0.05) * INV_PI / (1.0 + 0.11 * od));
            // Dark edges facing the light ("powder"): fewer photons scattered back out.
            let powder = 1.0 - 0.45 * exp(-sigma * 150.0);
            let h01 = saturate((h - base) / type_thickness(w.g));
            // Rain clouds have dark bellies.
            let belly = 1.0 - 0.55 * saturate(w.b * 6.0) * (1.0 - h01);
            let cover = saturate(w.r * 1.2);
            let ground_below = ground_sun * (1.0 - 0.85 * cover) + ground_sky * (1.0 - 0.5 * cover);
            // The ambient fill is light from all around: a cloud scattering nearly all it gets
            // shines with it as fog does (darkening it made night decks several times darker
            // than the fog under them).
            // The blue sky over the top diffuses down through a thick deck like the sun does
            // (the last term): without it an overcast's underside took only the sun's color
            // and a snowfall looked beige.
            var s = key * light * powder * mix(belly, 1.0, 0.4)
                + mix(ground_below, sky, sqrt(h01)) * belly * (0.5 + 0.5 * h01) + fill
                + sky * (1.0 - sqrt(h01)) * mix(belly, 1.0, 0.4) / (1.0 + 0.11 * od);
            // Lightning lights the cloud from inside: the light diffuses through kilometers of
            // droplets, so the whole cloud base around the channel glows.
            if (clouds.flash.w > 0.0) {
                let to_flash = clouds.flash.xyz - p;
                let d2 = dot(to_flash, to_flash);
                s += vec3<f32>(0.75, 0.82, 1.0) * clouds.flash.w / (d2 + 90000.0)
                    * exp(-sqrt(d2) * 0.0007) * (2.0 / (4.0 * PI));
            }
            let tr = mix(1.0, exp(-sigma * dt), share);
            r.radiance += r.transmittance * s * (1.0 - tr);
            if (!hit && r.transmittance * tr < 0.97) {
                hit = true;
                r.front = t;
            }
            r.transmittance *= tr;
            if (r.transmittance < 0.15 && r.back >= clouds.layer.w) { r.back = t; }
            if (r.transmittance < 0.01) {
                // Treat the rest as opaque (keeps the sun disc from shining through
                // overcast), keeping the light gathered so far at full strength.
                r.radiance /= 1.0 - r.transmittance;
                r.transmittance = 0.0;
                break;
            }
        }
        t += dt;
    }
    if (!hit) {
        r.transmittance = 1.0;
        r.radiance = vec3(0.0);
        return r;
    }
    r.back = min(max(r.back, r.front + 1.0), t1);
    // Far clouds fade toward the layer's draw distance.
    let fade = 1.0 - smoothstep(clouds.layer.w * 0.75, clouds.layer.w, r.front);
    let vis = clouds.params.x * fade;
    // Air between the camera and the cloud: dims it and adds haze in front of it. Under a
    // deck that haze lies in the clouds' shadow and takes their grey light instead of the
    // sun's; lit by the sun over tens of kilometers it glowed tan along the horizon.
    let air = atmosphere(vec3(0.0), dir, r.front, 6);
    let sunlit = select(sun_under_clouds(dir, r.front), 1.0, h0 > top);
    let deck = r.radiance / max(1.0 - r.transmittance, 1e-3);
    let haze = air.radiance * frame.sun_color.rgb * sunlit
        + (vec3(1.0) - air.transmittance) * deck * (1.0 - sunlit) * 0.8;
    r.radiance = (r.radiance * air.transmittance + haze * (1.0 - r.transmittance)) * vis;
    r.transmittance = mix(1.0, r.transmittance, vis);
    return r;
}

// How much of the air along `dir` out to `t` (m) the sun reaches through the clouds above
// it, 0..1.
fn sun_under_clouds(dir: vec3<f32>, t: f32) -> f32 {
    var lit = 0.0;
    for (var i = 0; i < 4; i++) {
        let p = dir * (t * (f32(i) + 0.5) * 0.25);
        lit += 1.0 - 0.9 * saturate(weather_at(p.xz).r * 1.2);
    }
    return lit * 0.25;
}

// How much rain the march counts out to `t` meters along a ray: the rain fades in from
// `veil.x` to `veil.y` (up close it is particles), so this is the integral of that fade.
// The composite's `rain_reach` is the same, so it can place the rain the march found.
fn veil_reach(t: f32) -> f32 {
    let span = max(clouds.veil.y - clouds.veil.x, 1.0);
    let x = saturate((t - clouds.veil.x) / span);
    return span * (x * x * x - 0.5 * x * x * x * x) + max(t - clouds.veil.y, 0.0);
}

// The distance `veil_reach` counts `reach` meters of rain out to.
fn veil_reach_inverse(reach: f32) -> f32 {
    let span = max(clouds.veil.y - clouds.veil.x, 1.0);
    if (reach >= 0.5 * span) { return clouds.veil.y + reach - 0.5 * span; }
    // Inside the fade x³ - x⁴/2 rises steadily over 0..1.
    var lo = 0.0;
    var hi = 1.0;
    for (var i = 0; i < 10; i++) {
        let m = 0.5 * (lo + hi);
        if (span * (m * m * m - 0.5 * m * m * m * m) < reach) { lo = m; } else { hi = m; }
    }
    return clouds.veil.x + span * 0.5 * (lo + hi);
}

// Edge `i` of the rain march's steps: `near` steps from `t0` to `tn`, then 12 to `t1`.
fn shaft_edge(i: i32, near: i32, t0: f32, tn: f32, t1: f32) -> f32 {
    if (i <= near && near > 0) { return mix(t0, tn, f32(i) / f32(near)); }
    return mix(tn, t1, f32(i - near) / 12.0);
}

// Rain or snow falling below the clouds: grey curtains in the distance, and in snowfall a
// white-out a few hundred meters away. Starts a little away from the camera (`clouds.veil`):
// rain right around the player is particles and fog, snow only the flakes close by.
// `deck` is the light under the clouds the ray meets (their underside's color), when it
// meets any.
fn rain_shafts(dir: vec3<f32>, max_t: f32, deck: vec4<f32>) -> CloudResult {
    var r: CloudResult;
    r.radiance = vec3(0.0);
    r.transmittance = 1.0;
    r.front = clouds.layer.w;
    r.back = clouds.layer.w;
    let h0 = frame.camera_world.w;
    let base = clouds.layer.x;
    // Climbing up into the cloud base the rain below fades out over 300 m instead of
    // vanishing the moment the camera crosses the base.
    let below_base = 1.0 - smoothstep(base, base + 300.0, h0);
    if (clouds.detail.w < 0.5 || clouds.params.x <= 0.001 || frame.planet.w <= 0.0 || below_base <= 0.0) {
        return r;
    }
    var t0 = clouds.veil.x;
    var t1 = min(max_t, 40000.0);
    let shell = ray_shell(vec3(0.0), h0, dir, base);
    if (h0 <= base) {
        if (shell.y > 0.0) { t1 = min(t1, shell.y); }
    } else {
        // Above the base the rain starts where the ray comes down through it.
        if (shell.x <= 0.0) { return r; }
        t0 = max(t0, shell.x);
    }
    let ground = ray_shell(vec3(0.0), h0, dir, 0.0);
    if (ground.x > 0.0) { t1 = min(t1, ground.x); }
    if (t1 <= t0) { return r; }
    let sky = atmosphere(vec3(0.0), vec3<f32>(0.0, 1.0, 0.0), 1e12, 6).radiance * clouds.light.rgb;
    // Rain is lit by the grey light under the clouds and a little by the sun from the side.
    let open_sky = sky * 0.55 + clouds.light.rgb * saturate(clouds.light_dir.y) * INV_PI * 0.06 + frame.ambient.rgb;
    // Under a deck the rain sees the deck above and the darker ground below: a little
    // darker than the deck's underside, less so over fresh snow.
    let below = mix(0.7, 0.9, saturate(frame.surface_open.z * SURFACE_SNOW_SCALE_M * 10.0));
    let lit = mix(open_sky, deck.rgb * below + frame.ambient.rgb, deck.a);
    let snow = clouds.veil.z;
    // Snow hides the view within a few hundred meters, so it gets four more steps over its
    // first 2 km: the far steps are kilometers long.
    let tn = select(t0, min(t1, mix(t0, 2000.0, snow)), snow > 0.0);
    let near = select(0, 4, tn > t0 + 1.0);
    let steps = near + 12;
    // Each step counts its sample's rain times the fade's integral over the step (the
    // fade changes a lot within one step), in "meters of rain" as `veil_reach` counts them.
    var od: array<f32, 16>;
    var reach: array<f32, 17>;
    reach[0] = veil_reach(t0);
    var total = 0.0;
    for (var i = 0; i < steps; i++) {
        let a = shaft_edge(i, near, t0, tn, t1);
        let b = shaft_edge(i + 1, near, t0, tn, t1);
        reach[i + 1] = veil_reach(b);
        let p = dir * (0.5 * (a + b));
        let w = weather_at(p.xz);
        let mm_h = w.b * 50.0;
        od[i] = 0.0;
        if (mm_h > 0.0) {
            // Visibility in rain: ~10 km at 1 mm/h, ~1 km at 50 mm/h; drizzle thins to nothing.
            // Snow hides far more for the water it carries: ~1 km at 1 mm/h, ~500 m at
            // 2.5 mm/h, under 300 m in a blizzard.
            let sigma = mix(0.00025 * pow(mm_h, 0.65), 0.003 * pow(mm_h, 0.7), snow)
                * saturate(mm_h * 10.0) * below_base;
            od[i] = sigma * (reach[i + 1] - reach[i]);
            let tr = exp(-od[i]);
            r.radiance += r.transmittance * (1.0 - tr) * lit;
            r.transmittance *= tr;
            total += od[i];
        }
    }
    if (total < 1e-5) {
        r.radiance = vec3(0.0);
        r.transmittance = 1.0;
        return r;
    }
    // Where the rain's optical depth reaches 10% and 90% of its total, each step's share spread
    // evenly over the step's meters of rain. Unlike the first and last wet sample these slide
    // smoothly as the samples move along the ray from pixel to pixel, so far hills don't get
    // bands of veil; the composite reads them back the same way (`rain_share`).
    let lo = total * 0.1;
    let hi = total * 0.9;
    // Where the veil has reached half its opacity: most of its light comes from in front of
    // that (a few hundred meters out in snow).
    let mid = -log(0.5 * (1.0 + exp(-total)));
    var front_r = reach[0];
    var mid_r = reach[steps];
    var back_r = reach[steps];
    var sum = 0.0;
    var found = false;
    var found_mid = false;
    for (var i = 0; i < steps; i++) {
        let next = sum + od[i];
        let per_depth = (reach[i + 1] - reach[i]) / max(od[i], 1e-12);
        if (!found && next >= lo) {
            front_r = reach[i] + (lo - sum) * per_depth;
            found = true;
        }
        if (!found_mid && next >= mid) {
            mid_r = reach[i] + (mid - sum) * per_depth;
            found_mid = true;
        }
        if (next >= hi) {
            back_r = reach[i] + (hi - sum) * per_depth;
            break;
        }
        sum = next;
    }
    r.front = veil_reach_inverse(front_r);
    r.back = veil_reach_inverse(back_r);
    let mid_t = veil_reach_inverse(mid_r);
    // The air in front dims the veil and adds its own light, lit as the veil is: under a deck
    // by the deck's grey rather than the sun (dimming alone turned far rain and snow tan).
    let air = atmosphere(vec3(0.0), dir, mid_t, 4);
    let haze = mix(air.radiance * frame.sun_color.rgb, (vec3(1.0) - air.transmittance) * lit, deck.a);
    r.radiance = (r.radiance * air.transmittance + haze * (1.0 - r.transmittance)) * clouds.params.x;
    r.transmittance = mix(1.0, r.transmittance, clouds.params.x);
    r.rain = select(-log(max(r.transmittance, 1e-30)), total, clouds.params.x >= 1.0);
    return r;
}

// Clouds plus rain shafts in front of them. `front` and `back` stay the clouds' own; the
// rain's span is kept apart so hills inside the rain are veiled by the rain in front of them.
fn clouds_and_rain(dir: vec3<f32>, steps: i32, light_steps: i32, jitter: f32) -> CloudResult {
    var c = march_clouds(dir, steps, light_steps, jitter);
    c.rain_front = clouds.layer.w;
    c.rain_back = clouds.layer.w;
    let cover = 1.0 - c.transmittance;
    var deck = vec4<f32>(c.radiance / max(cover, 1e-3), smoothstep(0.05, 0.6, cover));
    // Looking down from under the deck a ray sees no cloud, yet the rain there is lit by the
    // same deck as the rain just above the horizon: take the deck from the mirrored ray. Without
    // it the veil below the horizon was lit by the open sky and showed as a blue band.
    let up = normalize(-frame.planet.xyz);
    let down = dot(dir, up);
    if (down < 0.0 && frame.planet.w > 0.0 && frame.camera_world.w < clouds.layer.x
        && clouds.detail.w >= 0.5 && clouds.params.x > 0.001) {
        let m = march_clouds(dir - 2.0 * down * up, max(steps / 2, 8), max(light_steps / 2, 1), jitter);
        let mc = 1.0 - m.transmittance;
        deck = vec4<f32>(m.radiance / max(mc, 1e-3), smoothstep(0.05, 0.6, mc));
    }
    let s = rain_shafts(dir, c.front, deck);
    if (s.transmittance >= 1.0) { return c; }
    c.radiance = s.radiance + c.radiance * s.transmittance;
    c.transmittance *= s.transmittance;
    c.rain = s.rain;
    c.rain_front = s.front;
    c.rain_back = s.back;
    return c;
}

// The open ground's color as the weather leaves it (`frame.surface_open`): snow whitens it
// and rain darkens it. The cloud bases and the far ground in the environment map (which lights
// the fog and everything from below) see this, so in a snowfall the valley is lit from below by
// white snow rather than a green planet, and the falling snow hazes white instead of tan.
fn weathered_ground() -> vec3<f32> {
    let open = frame.surface_open;
    let snow = saturate(open.z * SURFACE_SNOW_SCALE_M * 10.0);
    let wet = mix(1.0, 0.6, saturate(open.x));
    return mix(frame.ground.rgb * wet, vec3<f32>(0.8), snow);
}

// The planet's surface far away (as in sky.wgsl).
fn planet_ground(dir: vec3<f32>, t: f32) -> vec3<f32> {
    let p = dir * t;
    let n = normalize(p - frame.planet.xyz);
    let sun = frame.sun_dir.xyz;
    let n_dot_l = dot(n, sun);
    let sun_light = frame.sun_color.rgb * sun_transmittance(p + n * 10.0) * saturate(n_dot_l);
    let sky_light = frame.sun_color.rgb * 0.12 * smoothstep(-0.15, 0.3, n_dot_l);
    let albedo = weathered_ground();
    return albedo * (sun_light + sky_light) * INV_PI + frame.ambient.rgb * albedo;
}

// The sky without sun disc, moon, stars or clouds (cd/m², not pre-exposed).
fn clear_sky(dir: vec3<f32>) -> vec3<f32> {
    let a = atmosphere(vec3(0.0), dir, 1e12, 16);
    let c = a.radiance * frame.sun_color.rgb;
    if (a.ground_t > 0.0) {
        return c + planet_ground(dir, a.ground_t) * a.transmittance;
    }
    // Night sky floor (airglow), params.w = how dark the night is.
    return c + vec3(0.002, 0.003, 0.006) * clouds.params.w;
}

fn camera_dir(uv: vec2<f32>) -> vec3<f32> {
    if (frame.exposure.w > 0.5) {
        return normalize(-frame.inv_view[2].xyz);
    }
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
    let p = frame.inv_view_proj * ndc;
    return normalize(p.xyz / p.w);
}

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

struct MarchOut {
    @location(0) color: vec4<f32>,   // pre-exposed radiance, transmittance
    @location(1) depth: vec4<f32>,   // cloud front, back (km), rain optical depth, rain 10% (km)
    @location(2) sky: vec4<f32>,     // pre-exposed clear sky, rain 90% (km)
};

// Low-resolution march of the whole view (no scene depth; composited per pixel later).
@fragment
fn fs_march(in: FullscreenOut) -> MarchOut {
    let dir = camera_dir(in.uv);
    let r = clouds_and_rain(dir, i32(clouds.light.w), i32(clouds.light_dir.w), ign(in.position.xy));
    var out: MarchOut;
    out.color = vec4<f32>(min(pre_expose(r.radiance), vec3(60000.0)), r.transmittance);
    out.depth = vec4<f32>(r.front * 0.001, r.back * 0.001, r.rain, r.rain_front * 0.001);
    out.sky = vec4<f32>(min(pre_expose(clear_sky(dir)), vec3(60000.0)), r.rain_back * 0.001);
    return out;
}

// Environment map (ambient light and reflections): clear sky with cheaper clouds over it,
// not pre-exposed.
@vertex
fn vs_env(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.5);
}

@fragment
fn fs_env(in: FullscreenOut) -> @location(0) vec4<f32> {
    let dir = equirect_to_dir(in.uv);
    let texel = vec2<f32>(abs(dpdx(in.uv.x)), abs(dpdy(in.uv.y)));
    let steps = i32(clouds.params.z);
    let ground_t = ray_shell(vec3(0.0), frame.camera_world.w, dir, 0.0).x;
    if (ground_t > 0.0 && frame.planet.w > 0.0) {
        // The ground far away, lit by the sky above it as it is now (overcast skies light it
        // softly instead of leaving it black) and by the sun through the clouds.
        let up = normalize(vec3<f32>(dir.x, -dir.y + 0.7, dir.z));
        let r = clouds_and_rain(up, steps, 2, 0.5);
        let sky_up = r.radiance + clear_sky(up) * r.transmittance;
        let a = atmosphere(vec3(0.0), dir, 1e12, 8);
        // Under a deck the far ground, and the haze in front of it, lie in the clouds'
        // shadow too.
        let shade = mix(0.2, 1.0, r.transmittance);
        let lit = planet_ground(dir, ground_t) * shade + weathered_ground() * sky_up * 0.8;
        let haze = a.radiance * frame.sun_color.rgb * shade
            + (vec3(1.0) - a.transmittance) * sky_up * (1.0 - shade) * 0.8;
        let c = haze + lit * a.transmittance;
        return vec4<f32>(min(c, vec3(60000.0)), 1.0);
    }
    if (abs(dir.y) > 0.25) {
        let r = clouds_and_rain(dir, steps, 2, 0.5);
        let c = r.radiance + clear_sky(dir) * r.transmittance;
        return vec4<f32>(min(c, vec3(60000.0)), 1.0);
    }
    // Near the horizon a ray crosses tens of kilometers of cloud layer in a few strides, so
    // one ray per texel lands on different noise from its neighbors and the map gets streaks
    // in azimuth, which the renderer's fog (lit by the horizon color) repeats down the whole
    // screen. Average four rays across the texel, with staggered strides and more of them.
    var c = vec3(0.0);
    for (var k = 0; k < 4; k++) {
        let o = (vec2<f32>(f32(k & 1), f32(k >> 1u)) - 0.5) * 0.5;
        let d = equirect_to_dir(in.uv + o * texel);
        let r = clouds_and_rain(d, steps * 2, 2, (f32(k) + 0.5) * 0.25);
        c += (r.radiance + clear_sky(d) * r.transmittance) * 0.25;
    }
    return vec4<f32>(min(c, vec3(60000.0)), 1.0);
}

// The renderer's cloud shadow map (`cloud_shadows`): per texel, the sunlight that comes down
// through the clouds to the cloud base plane, along the light the renderer shades with
// (`frame.sun_dir`: the sun, or the moon at night). Relative to the dimming the weather already
// gave that light for the deck at the player (`clouds.shadow.x`), so overcast isn't darkened
// twice and the ground under a break in it stays at the dimmed sun.
@fragment
fn fs_cloud_shadow(in: FullscreenOut) -> @location(0) vec4<f32> {
    let l = frame.sun_dir.xyz;
    if (l.y <= 0.0 || frame.planet.w <= 0.0) { return vec4<f32>(1.0); }
    let start = cloud_shadow_plane_point(in.uv);
    // From where the ray enters the layer (the plane sits a little below a curved layer far
    // away) to the highest tops, at most 30 km along a low sun.
    let h = planet_height(start);
    let rise = max(dot(l, normalize(start - frame.planet.xyz)), 0.05);
    let t0 = max(clouds.layer.x - h, 0.0) / rise;
    let t1 = min(max(clouds.layer.y - h, 0.0) / rise, t0 + 30000.0);
    if (t1 <= t0) { return vec4<f32>(1.0); }
    let steps = i32(clouds.shadow.y);
    let dt = (t1 - t0) / f32(steps);
    var od = 0.0;
    for (var i = 0; i < steps; i++) {
        let q = start + l * (t0 + dt * (f32(i) + 0.5));
        od += cloud_density(q, planet_height(q), weather_at_smooth(q.xz), true) * dt;
    }
    od *= clouds.layer.z;
    // Light scattered forward through the cloud softens thin cloud's shadow; under a thick
    // deck only the sky's light is left.
    let through = max(exp(-od), 0.2 * exp(-0.3 * od));
    let t = saturate(through / max(clouds.shadow.x, 1e-4));
    return vec4<f32>(t, t, t, 1.0);
}
