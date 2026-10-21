// Grass in the sun's shadow cascades (depth only). Requires lib_types + grass_common (not
// lib_frame). Blades are camera-relative here too, so they thin out exactly as on screen.

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,         // wind velocity xyz
    time: vec4<f32>,         // world seconds
    camera_frac: vec4<f32>,  // camera position modulo 4096 (tiling coordinates)
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;

// Snow depth (m) for `grass_blade`: the ground's by the camera (shadows are cast only nearby).
fn grass_snow_depth(f: GrassField, rel: vec3<f32>) -> f32 {
    return f.look.w;
}

@vertex
fn vs_shadow(@builtin(instance_index) instance: u32, @builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let b = grass_blade(instance, vertex, shadow_view.time.x, shadow_view.wind.xyz, shadow_view.camera_frac.xyz);
    // An invisible blade collapses to one point: no area, nothing drawn.
    return shadow_view.view_proj * vec4<f32>(b.rel, 1.0);
}
