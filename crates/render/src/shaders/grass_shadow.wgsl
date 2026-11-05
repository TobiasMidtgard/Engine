// Grass in the sun's shadow cascades (depth only). Requires lib_types + grass_common (not
// lib_frame). Draws the blades the cull pass (grass_cull.wgsl) kept for a cascade, as
// grass.wgsl draws the screen's.

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,         // wind velocity xyz
    time: vec4<f32>,         // world seconds
    camera_frac: vec4<f32>,  // camera position modulo 4096 (tiling coordinates)
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;
@group(1) @binding(4) var<storage, read> grass_records: array<GrassRecord>;

@vertex
fn vs_shadow(@builtin(instance_index) instance: u32, @builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    var s = grass_unpack(grass_records[(vertex >> 4u) + instance]);
    // A compound leaf (or a shrub's twig) is drawn whole and its leaflets cut out of it on
    // screen only (a discard
    // would cost the cascades their early depth test): its shadow is a smaller whole leaf,
    // about as much shade as the leaflets give.
    let cut = grass_fields[s.field].kind.z;
    if ((cut < 0.0 || cut > 99.5) && s.flower < 0.5) {
        s.width *= 0.6;
    }
    let b = grass_vertex(s, vertex & 15u, shadow_view.time.x, s.tip);
    return shadow_view.view_proj * vec4<f32>(b.rel, 1.0);
}
