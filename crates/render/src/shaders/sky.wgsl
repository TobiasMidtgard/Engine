// Sky: physically based atmosphere (lib_atmosphere: multiple scattering from look-up tables),
// sun disc, a simple cloud layer and stars, behind the volumetric fog. Also renders the
// equirectangular environment map used for ambient light and reflections. Requires lib_types +
// lib_frame + lib_atmosphere.

fn star_field(dir: vec3<f32>) -> vec3<f32> {
    let p = dir * 400.0;
    let cell = floor(p);
    let h = hash13(cell);
    if (h < 0.996) { return vec3(0.0); }
    let f = fract(p) - 0.5;
    let d = length(f);
    let brightness = (h - 0.996) / 0.004;
    let twinkle = 0.7 + 0.3 * sin(frame.time.y * (2.0 + h * 5.0) + h * 100.0);
    return vec3(0.8, 0.85, 1.0) * smoothstep(0.25, 0.0, d) * brightness * twinkle * 0.05;
}

// Cheap 2D cloud layer on a spherical shell. Returns (radiance, coverage alpha).
fn cloud_layer(dir: vec3<f32>, sky: vec3<f32>) -> vec4<f32> {
    let cover = frame.weather.z;
    if (cover <= 0.01 || frame.planet.w <= 0.0) { return vec4(0.0); }
    let base_alt = 1500.0;
    let hit = ray_shell(vec3(0.0), frame.camera_world.w, dir, base_alt);
    var t = hit.y;
    if (hit.x > 0.0) { t = hit.x; }
    if (t <= 0.0 || t > 60000.0) { return vec4(0.0); }
    let p = dir * t + frame.camera_world_frac.xyz;
    let drift = frame.wind.xz * frame.time.x * 1.0;
    let uv = (p.xz + drift) / 2200.0;
    let n = fbm2(uv, 6);
    let detail = fbm2(uv * 4.0 + vec2(3.1, 7.7), 4);
    let shape = n * 0.75 + detail * 0.25;
    let density = smoothstep(1.0 - cover * 0.95, 1.05 - cover * 0.5, shape);
    if (density <= 0.0) { return vec4(0.0); }
    let thickness = density * mix(0.6, 1.0, cover);
    // Light: forward-scattered sun through the cloud plus ambient sky.
    let sun = frame.sun_dir.xyz;
    let sun_col = frame.sun_color.rgb * sun_transmittance(dir * min(t, 20000.0));
    let silver = pow(saturate(dot(dir, sun)), 6.0) * 0.6;
    let self_shadow = exp(-thickness * 2.5 * (1.0 + frame.weather.w * 0.2));
    let lit = sun_col * INV_PI * (0.25 * self_shadow + silver * self_shadow) * 0.9;
    let ambient = sky * 0.6 + frame.ambient.rgb * 0.4;
    var col = lit + ambient * (0.6 + 0.4 * (1.0 - thickness));
    // Storm clouds are darker.
    col *= mix(1.0, 0.35, saturate(frame.weather.w / 20.0));
    // Fade toward the horizon (haze).
    let horizon = saturate(t / 60000.0);
    let alpha = density * (1.0 - horizon * horizon);
    return vec4(col, alpha);
}

// The planet's surface far away (beyond the terrain): lit by the sun and sky, seen through air.
fn planet_ground(dir: vec3<f32>, t: f32) -> vec3<f32> {
    let p = dir * t;
    let n = normalize(p - frame.planet.xyz);
    let sun = frame.sun_dir.xyz;
    let n_dot_l = dot(n, sun);
    let sun_light = frame.sun_color.rgb * sun_transmittance(p + n * 10.0) * saturate(n_dot_l);
    let sky_light = frame.sun_color.rgb * 0.12 * smoothstep(-0.15, 0.3, n_dot_l);
    let albedo = frame.ground.rgb;
    return albedo * (sun_light + sky_light) * INV_PI + frame.ambient.rgb * albedo;
}

fn sky_radiance(dir: vec3<f32>, with_sun: bool) -> vec3<f32> {
    var c = vec3<f32>(0.0);
    // Transmittance toward space along `dir` (sun disc, stars).
    var to_space = vec3<f32>(1.0);
    if (frame.planet.w > 0.0) {
        let h0 = frame.camera_world.w;
        let mu = dot(dir, normalize(-frame.planet.xyz));
        if (sky_view_valid()) {
            c = sky_view_radiance(dir) * frame.sun_color.rgb;
            let ground = ray_shell_mu(h0, mu, 0.0);
            if (ground.x > 0.0) {
                // Below the horizon: the planet, seen through the air.
                return c + planet_ground(dir, ground.x) * ground_transmittance(h0, mu, ground.x);
            }
            to_space = transmittance_to_top(h0, mu);
        } else {
            // Above the atmosphere: integrate per pixel.
            let a = atmosphere(vec3(0.0), dir, 1e12, 32);
            c = a.radiance * frame.sun_color.rgb;
            if (a.ground_t > 0.0) {
                return c + planet_ground(dir, a.ground_t) * a.transmittance;
            }
            to_space = a.transmittance;
        }
    }
    // Night sky floor and stars.
    let night = 1.0 - saturate(frame.sun_dir.y * 4.0 + 0.3);
    c += vec3(0.002, 0.003, 0.006) * night;
    c += star_field(dir) * night * to_space;
    // Moon-ish glow opposite the sun at night.
    let moon_dir = -frame.sun_dir.xyz;
    let moon = smoothstep(0.9997, 0.99985, dot(dir, moon_dir)) * night * 0.5;
    c += vec3(moon);
    if (with_sun) {
        let cos_r = cos(frame.sun_dir.w);
        let cd = dot(dir, frame.sun_dir.xyz);
        if (cd > cos_r) {
            let solid_angle = PI * frame.sun_dir.w * frame.sun_dir.w;
            let mu = saturate((cd - cos_r) / (1.0 - cos_r));
            let limb = 0.4 + 0.6 * sqrt(mu);
            c += frame.sun_color.rgb / solid_angle * to_space * limb;
        }
    }
    let cl = cloud_layer(dir, c);
    c = mix(c, cl.rgb, cl.a);
    return c;
}

fn view_dir(uv: vec2<f32>) -> vec3<f32> {
    if (frame.exposure.w > 0.5) {
        return normalize(-frame.inv_view[2].xyz);
    }
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
    let p = frame.inv_view_proj * ndc;
    return normalize(p.xyz / p.w);
}

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

@fragment
fn fs_sky(in: FullscreenOut) -> @location(0) vec4<f32> {
    let dir = view_dir(in.uv);
    // Fog in front of the sky: sunbeams against the sky, the sun's disc dimmed by mist, and the
    // height fog out to the horizon, as over distant ground.
    let c = apply_full_sky_fog(pre_expose(sky_radiance(dir, true)), dir, in.position.xy);
    return vec4<f32>(min(c, vec3(60000.0)), 1.0);
}

@vertex
fn vs_env(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.5);
}

// Environment map: not pre-exposed, no sun disc (lit analytically). Without a planet the
// ground below the horizon is a plane lit by the sky and sun, so reflections and diffuse light
// from below aren't black.
@fragment
fn fs_env(in: FullscreenOut) -> @location(0) vec4<f32> {
    let dir = equirect_to_dir(in.uv);
    var c = sky_radiance(dir, false);
    if (frame.planet.w <= 0.0) {
        c = mix(c, ground_radiance(), smoothstep(0.0, -0.05, dir.y));
    }
    return vec4<f32>(min(c, vec3(60000.0)), 1.0);
}
