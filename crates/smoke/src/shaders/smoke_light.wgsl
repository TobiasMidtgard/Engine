// Smoke lighting, once per simulation step: for every voxel, how much smoke lies toward the
// sun, up toward the sky, out toward the horizon and down toward the ground (optical depths),
// and the firelight and lamplight that reach it through the smoke (and from the flames under
// smoke still hot from them).

struct SmokeParams {
    min_rel: vec4<f32>,   // box min (camera-relative), voxel size
    size: vec4<f32>,      // box size (m), blend previous -> current
    dims: vec4<f32>,      // voxels, firelight scale
    look: vec4<f32>,      // detail, max steps, tunnel count, unused
    tunnels: array<vec4<f32>, 32>,
};

@group(1) @binding(0) var<uniform> smoke: SmokeParams;
@group(1) @binding(1) var density: texture_3d<f32>;
@group(1) @binding(2) var light_out: texture_storage_3d<rgba16float, write>;
@group(1) @binding(3) var glow_out: texture_storage_3d<rgba16float, write>;
@group(1) @binding(4) var heat: texture_3d<f32>;

const SUN_STEPS: i32 = 32;
const SKY_STEPS: i32 = 16;
const SIDE_STEPS: i32 = 8;
const LAMP_STEPS: i32 = 6;
// Smoke this much hotter than the air (°C) starts to be lit by the flames under it...
const FIRELIT_FROM_C: f32 = 40.0;
// ...and fully at this.
const FIRELIT_FULL_C: f32 = 500.0;
// Illuminance from the flames on fully hot smoke, lux.
const FIRELIT_LUX: f32 = 300.0;
const FLAME_COLOR: vec3<f32> = vec3<f32>(1.0, 0.42, 0.12);

// Extinction (1/m) at voxel-space point `q` (0 outside the box).
fn sigma_at(q: vec3<f32>) -> f32 {
    let uvw = q / smoke.dims.xyz;
    if (any(uvw < vec3(0.0)) || any(uvw > vec3(1.0))) { return 0.0; }
    return textureSampleLevel(density, linear_clamp, uvw, 0.0).x;
}

// Optical depth from the edge of the voxel at voxel-space `q` along unit `dir`, with steps that
// start at `first` voxels and grow by `growth` each step, until the ray leaves the box. The
// voxel's own smoke doesn't count: in thick smoke the light that reaches the eye is scattered
// near the voxel's sunny edge, not at its center.
fn optical_depth(q: vec3<f32>, dir: vec3<f32>, steps: i32, first: f32, growth: f32) -> f32 {
    var tau = 0.0;
    var step = first;
    var p = q + dir * (0.5 + first * 0.5);
    for (var i = 0; i < steps; i++) {
        if (any(p < vec3(-1.0)) || any(p > smoke.dims.xyz + vec3(1.0))) { break; }
        tau += sigma_at(p) * step;
        p += dir * step;
        step *= growth;
    }
    return tau * smoke.min_rel.w;
}

@compute @workgroup_size(4, 4, 4)
fn cs_light(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = vec3<u32>(smoke.dims.xyz);
    if (any(id >= dims)) { return; }
    let q = vec3<f32>(id) + 0.5;
    let cell = smoke.min_rel.w;

    var tau_sun = 0.0;
    let sun = frame.sun_dir.xyz;
    if (dot(frame.sun_camera.rgb, frame.sun_camera.rgb) > 0.0 && sun.y > -0.2) {
        tau_sun = optical_depth(q, sun, SUN_STEPS, 0.75, 1.12);
    }
    let tau_up = optical_depth(q, vec3(0.0, 1.0, 0.0), SKY_STEPS, 0.75, 1.15);
    let tau_down = optical_depth(q, vec3(0.0, -1.0, 0.0), SKY_STEPS / 2, 0.75, 1.15);
    // Out toward the horizon: the clearest of four directions lets the most sky in.
    var tau_side = 1e9;
    for (var k = 0; k < 4; k++) {
        let a = f32(k) * 1.5707964 + 0.3927;
        let d = vec3<f32>(cos(a), 0.0, sin(a));
        tau_side = min(tau_side, optical_depth(q, d, SIDE_STEPS, 0.75, 1.3));
    }
    textureStore(light_out, id, vec4<f32>(tau_sun, tau_up, tau_down, tau_side));

    // Fires and lamps (point lights), dimmed by the smoke between them and this voxel.
    let rel = smoke.min_rel.xyz + q * cell;
    var glow = vec3(0.0);
    for (var i = 0u; i < frame.counts.x; i++) {
        let light = point_lights[i];
        let to_light = light.position_range.xyz - rel;
        let dist2 = dot(to_light, to_light);
        let range = light.position_range.w;
        if (dist2 > range * range) { continue; }
        let falloff = sq(saturate(1.0 - sq(dist2 / (range * range))))
            / max(dist2, sq(max(light.color_radius.w, cell)));
        let dist = sqrt(dist2);
        let dir = to_light / max(dist, 1e-4);
        let span = min(dist / cell, smoke.dims.x + smoke.dims.y + smoke.dims.z);
        let tau = optical_depth(q, dir, LAMP_STEPS, span / f32(LAMP_STEPS), 1.0);
        glow += light.color_radius.rgb * falloff * exp(-tau);
    }
    // The flames under a plume light it from below over their whole length, which a few point
    // lights at their centers can't: smoke still hot from the fire (it cools within meters of
    // the flames) gets the flames' light, up to a few hundred lux. It shows at dusk and at
    // night, when the plume's underside glows orange; by day the sun outshines it.
    let hot = saturate((textureLoad(heat, vec3<i32>(id), 0).x - FIRELIT_FROM_C)
        / (FIRELIT_FULL_C - FIRELIT_FROM_C));
    glow += FLAME_COLOR * FIRELIT_LUX * hot * sqrt(hot);
    textureStore(glow_out, id, vec4<f32>(glow / smoke.dims.w, 0.0));
}
