// ============================================================================
// Debris: shadow cascades. Composed as lib_types + particles_common + debris_common + this.
// ============================================================================

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;

@vertex
fn vs_debris_shadow(@builtin(vertex_index) vi: u32, @builtin(instance_index) slot: u32) -> @builtin(position) vec4<f32> {
    let piece = debris_piece(slot, vi);
    // Mostly dissolved pieces stop casting.
    if (!piece.alive || piece.color.a < 0.5) {
        return vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    return shadow_view.view_proj * vec4<f32>(piece.rel, 1.0);
}
