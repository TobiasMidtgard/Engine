// Shadow casting of the skin. Requires lib_types + common.wgsl.

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;

@vertex
fn vs_skin_shadow(
    @location(0) position_curvature: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) rest: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) bones: vec4<u32>,
    @location(7) weights: vec4<f32>,
    @builtin(vertex_index) vertex: u32,
) -> @builtin(position) vec4<f32> {
    let m = morph_at(vertex);
    let hand = curl_hand(position_curvature.xyz + m.dp, normalize(normal.xyz + m.dn), bones, weights, false);
    let local = hand.p + hand.n * cloth_lift(rest.xyz, m2.x);
    var p = (skin_matrix(bones, weights, false) * vec4<f32>(local, 1.0)).xyz;
    if (u32(normal.w + 0.5) == 5u) {
        p = clear_thighs(p, bones, false);
    }
    var clip = shadow_view.view_proj * vec4<f32>(model_point(p), 1.0);
    // Pancake casters behind the near plane onto it so they still cast shadows.
    clip.z = max(clip.z, 0.0);
    return clip;
}
