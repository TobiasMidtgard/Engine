// The aerial perspective table (aerial.rs): one thread per screen column marches the column's
// ray from the camera to the table's reach and writes, at the far edge of every depth slice,
// the height fog beyond the volumetric fog's range and the atmosphere between the camera and
// there, as `surface_fog` took them per pixel. After Hillaire 2020 ("A Scalable and Production
// Ready Sky and Atmosphere Rendering Technique", section 5.4). Requires lib_types + lib_frame +
// lib_atmosphere; group 0 is the prepass frame group (binding 24 is a stand-in while the table
// is written).
//
// Parts, side by side along x (C columns each): height fog light (pre-exposed) and
// transmittance; atmosphere light (pre-exposed, times the sun's color) and red transmittance;
// atmosphere green and blue transmittance.

@group(1) @binding(0) var aerial_out: texture_storage_3d<rgba16float, write>;

// Haze shadow samples per slice: the slices together sample the far shadow map and the cloud
// shadows all along the ray (a pixel took 8 for its whole ray).
const SLICE_SHADOW_SAMPLES: i32 = 2;
// Atmosphere steps from the camera to each slice's far edge (as a pixel marched it).
const AIR_STEPS: i32 = 8;

@compute @workgroup_size(8, 8, 1)
fn cs_aerial(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(aerial_out);
    let columns = size.x / 3u;
    if (id.x >= columns || id.y >= size.y) { return; }
    // The column's ray through the unjittered view, and meters along it per meter of depth.
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(f32(columns), f32(size.y));
    let ray = (frame.inv_view * vec4<f32>(view_ray_at_uv(uv), 0.0)).xyz;
    let per_depth = length(ray);
    let dir = ray / per_depth;
    // The height fog starts where the volumetric fog's range (a view depth) ends.
    var fog_start = 0.0;
    if (frame.volumetrics.x > 0.0) { fog_start = frame.volumetrics.x * per_depth; }
    var fog = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var prev = 0.0;
    // `CUT_AERIAL_STEPWISE`: the atmosphere carried on from slice to slice, one step across each
    // slice (64 steps along the ray, each about 15% of its distance long), not marched from
    // the camera again for every slice (64 x 8). The step is `integrate_atmosphere_spaced`'s.
    let stepwise = trial_cut_more(CUT_AERIAL_STEPWISE);
    var air_light = vec3<f32>(0.0);
    var air_through = vec3<f32>(1.0);
    let h0 = frame.camera_world.w;
    let up = normalize(-frame.planet.xyz);
    let mu = dot(dir, up);
    let mu_s = dot(frame.sun_dir.xyz, up);
    let nu = dot(dir, frame.sun_dir.xyz);
    let radius = frame.planet.w;
    let r0 = radius + h0;
    // Where the ray is in the air: from entering it to leaving it or reaching the ground.
    var air_from = 0.0;
    var air_to = -1.0;
    if (stepwise && radius > 0.0) {
        let hit_atmo = ray_shell_mu(h0, mu, frame.atmosphere.x);
        if (hit_atmo.y >= 0.0) {
            air_from = max(hit_atmo.x, 0.0);
            air_to = hit_atmo.y;
            let hit_ground = ray_shell_mu(h0, mu, 0.0);
            if (hit_ground.x > 0.0) { air_to = min(air_to, hit_ground.x); }
        }
    }
    let pr = phase_rayleigh(nu);
    let g_ms = air_mie_g_scattered();
    let pm = phase_mie_rgb(nu, g_ms);
    for (var k = 0u; k < size.z; k++) {
        let t = aerial_slice_distance(f32(k + 1u));
        let a = max(prev, fog_start);
        if (t > a) {
            let tr = exp(-height_fog_optical_depth(dir, a, t));
            if (tr < 0.9999) {
                // As `surface_fog` lights the height fog: haze in caves and rooms sees neither
                // sky nor sun, haze in a mountain's or a cloud's shadow doesn't glow.
                let lit = fog_sky_visibility(dir * t);
                let sun = height_fog_sun_visibility_n(dir, a, t, tr, 0.5, SLICE_SHADOW_SAMPLES);
                let at = height_fog_scatter_point(dir, a, t, tr);
                let in_fog = height_fog_sunlight_along(dir, a, t, tr);
                let l = pre_expose(height_fog_lit(dir, at, in_fog, lit, sun)) * (1.0 - tr);
                fog = vec4<f32>(fog.rgb + l * fog.a, fog.a * tr);
            }
        }
        if (stepwise) {
            let t0 = max(prev, air_from);
            let t1 = min(t, air_to);
            if (t1 > t0) {
                let dt = t1 - t0;
                let tm = (t0 + t1) * 0.5;
                let h = height_along(h0, mu, tm);
                let mu_s_t = clamp((r0 * mu_s + tm * nu) / (radius + h), -1.0, 1.0);
                let m = atmo_medium(h);
                let sun = transmittance_to_top(h, mu_s_t) * sun_above_horizon(h, mu_s_t);
                let ms = multiscatter_parts(h, mu_s_t);
                let mu_t = clamp((r0 * mu + tm) / (radius + h), -1.0, 1.0);
                let g1 = m.mie * g_ms / max(m.mie + m.rayleigh, vec3<f32>(1e-20));
                let mie_ms = multiscatter_inscatter(ms, mu_t, nu, g_ms, g1);
                let sc = sun * (m.rayleigh * pr + m.mie * pm) + ms.total * m.rayleigh + mie_ms * m.mie;
                let seg = exp(-m.extinction * dt);
                air_light += air_through * (sc - sc * seg) / max(m.extinction, vec3<f32>(1e-12));
                air_through *= seg;
            }
        } else {
            let air = atmosphere(vec3<f32>(0.0), dir, t, AIR_STEPS);
            air_light = air.radiance;
            air_through = air.transmittance;
        }
        let light = pre_expose(air_light * frame.sun_color.rgb);
        textureStore(aerial_out, vec3<u32>(id.x, id.y, k), fog);
        textureStore(aerial_out, vec3<u32>(id.x + columns, id.y, k), vec4<f32>(light, air_through.r));
        textureStore(aerial_out, vec3<u32>(id.x + 2u * columns, id.y, k), vec4<f32>(air_through.gb, 0.0, 0.0));
        prev = t;
    }
}
