// Shadow casting for soft-body surfaces (cloth, volumes, ropes). Requires lib_types only.

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tension: vec2<f32>,
    @location(4) burn: vec2<f32>,
    @location(5) material: u32,
};

@vertex
fn vs_shadow(in: VertexIn) -> @builtin(position) vec4<f32> {
    var clip = shadow_view.view_proj * vec4<f32>(in.position, 1.0);
    // Pancake casters behind the near plane onto it so they still cast shadows.
    clip.z = max(clip.z, 0.0);
    return clip;
}
