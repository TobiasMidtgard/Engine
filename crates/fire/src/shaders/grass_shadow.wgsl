// Grass blades, shadow pass. Requires lib_types + grass_common (no lib_frame).

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;

@vertex
fn vs_grass_shadow(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> @builtin(position) vec4<f32> {
    let b = build_blade(instance, vertex, shadow_view.wind.xyz, shadow_view.time.x, shadow_view.camera_frac.xyz);
    var clip = shadow_view.view_proj * vec4<f32>(b.rel, 1.0);
    clip.z = max(clip.z, 0.0);
    return clip;
}
