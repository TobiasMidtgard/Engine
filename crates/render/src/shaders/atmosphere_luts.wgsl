// Atmosphere look-up tables (Hillaire 2020). Requires lib_types + lib_frame + lib_atmosphere.
// Each entry point reads only the group 0 bindings it needs (the frame uniform, the linear
// sampler and the tables computed before it) and writes one table in group 1.

@group(1) @binding(0) var lut_out: texture_storage_2d<rgba16float, write>;

const ISOTROPIC_PHASE: f32 = 0.07957747154; // 1 / (4 pi)

fn lut_uv(id: vec3<u32>) -> vec2<f32> {
    return (vec2<f32>(id.xy) + 0.5) / vec2<f32>(textureDimensions(lut_out));
}

fn outside_lut(id: vec3<u32>) -> bool {
    let size = textureDimensions(lut_out);
    return id.x >= size.x || id.y >= size.y;
}

// Transmittance to the top of the atmosphere for every (height, zenith cosine).
@compute @workgroup_size(8, 8)
fn cs_transmittance(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside_lut(id)) { return; }
    let p = transmittance_lut_params(lut_uv(id));
    let len = distance_to_top(p.x, p.y);
    let steps = 100;
    var depth = vec3<f32>(0.0);
    for (var i = 0; i < steps; i++) {
        let t = (f32(i) + 0.5) / f32(steps) * len;
        depth += atmo_density(height_along(p.x, p.y, t));
    }
    let optical = extinction(depth * (len / f32(steps)));
    textureStore(lut_out, vec2<i32>(id.xy), vec4<f32>(exp(-optical), 1.0));
}

// Multiple scattering for every (sun zenith cosine, height): the second-order light arriving
// at a point from all directions (isotropic phase, including light bounced off the ground),
// summed over all orders as a geometric series with the fraction f_ms that each order
// transfers to the next: Psi = L2 / (1 - f_ms).
@compute @workgroup_size(8, 8)
fn cs_multiscatter(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside_lut(id)) { return; }
    let uv = lut_uv(id);
    let mu_s = texel_to_unit(uv.x, MULTISCATTER_LUT_SIZE.x) * 2.0 - 1.0;
    let h = max(texel_to_unit(uv.y, MULTISCATTER_LUT_SIZE.y) * frame.atmosphere.x, 1.0);
    let radius = frame.planet.w;
    let r0 = radius + h;
    let sin_s = sqrt(max(1.0 - mu_s * mu_s, 0.0));
    let albedo = frame.ground.rgb;
    let steps = 20;
    var light = vec3<f32>(0.0);
    var transfer = vec3<f32>(0.0);
    for (var j = 0; j < 8; j++) {
        let mu = 1.0 - 2.0 * (f32(j) + 0.5) / 8.0;
        let sin_v = sqrt(max(1.0 - mu * mu, 0.0));
        for (var i = 0; i < 8; i++) {
            let phi = 2.0 * PI * (f32(i) + 0.5) / 8.0;
            let nu = sin_v * sin_s * cos(phi) + mu * mu_s;
            var t_end = max(ray_shell_mu(h, mu, frame.atmosphere.x).y, 0.0);
            let ground = ray_shell_mu(h, mu, 0.0);
            let hits_ground = ground.x > 0.0;
            if (hits_ground) { t_end = ground.x; }
            let dt = t_end / f32(steps);
            var throughput = vec3<f32>(1.0);
            for (var k = 0; k < steps; k++) {
                let t = (f32(k) + 0.5) * dt;
                let hk = height_along(h, mu, t);
                let mu_s_t = clamp((r0 * mu_s + t * nu) / (radius + hk), -1.0, 1.0);
                let m = atmo_medium(hk);
                let scattering = m.rayleigh + m.mie;
                let sun = transmittance_to_top(hk, mu_s_t) * sun_above_horizon(hk, mu_s_t);
                let absorbed = (vec3<f32>(1.0) - exp(-m.extinction * dt)) / max(m.extinction, vec3<f32>(1e-12));
                light += throughput * sun * scattering * ISOTROPIC_PHASE * absorbed;
                transfer += throughput * scattering * absorbed;
                throughput *= exp(-m.extinction * dt);
            }
            if (hits_ground) {
                let mu_s_g = clamp((r0 * mu_s + t_end * nu) / radius, -1.0, 1.0);
                let sun = transmittance_to_top(0.0, mu_s_g) * sun_above_horizon(0.0, mu_s_g);
                light += throughput * sun * saturate(mu_s_g) * albedo * INV_PI;
            }
        }
    }
    light /= 64.0;
    transfer /= 64.0;
    let psi = light / max(vec3<f32>(1.0) - transfer, vec3<f32>(1e-3));
    textureStore(lut_out, vec2<i32>(id.xy), vec4<f32>(psi, 1.0));
}

// The sky seen from the camera this frame, for every (zenith, azimuth from the sun).
@compute @workgroup_size(8, 8)
fn cs_sky_view(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside_lut(id)) { return; }
    let h = sky_view_height();
    let p = sky_view_lut_params(lut_uv(id), h);
    let mu = p.x;
    let up = normalize(-frame.planet.xyz);
    let mu_s = clamp(dot(frame.sun_dir.xyz, up), -1.0, 1.0);
    let nu = sqrt(max(1.0 - mu * mu, 0.0)) * sqrt(max(1.0 - mu_s * mu_s, 0.0)) * p.y + mu * mu_s;
    var t_end = max(ray_shell_mu(h, mu, frame.atmosphere.x).y, 0.0);
    let ground = ray_shell_mu(h, mu, 0.0);
    if (ground.x > 0.0) { t_end = ground.x; }
    let s = integrate_atmosphere(h, mu, mu_s, nu, 0.0, t_end, 32);
    textureStore(lut_out, vec2<i32>(id.xy), vec4<f32>(s.radiance, 1.0));
}
