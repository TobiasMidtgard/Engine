// Hair and fur strands, shadow pass. Requires lib_types + strand_common.

struct ShadowView {
    view_proj: mat4x4<f32>,
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;

@vertex
fn vs_strand_shadow(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> @builtin(position) vec4<f32> {
    let p = strand_point(instance, vertex);
    // The light looks along the direction that keeps clip x and y constant.
    let m = shadow_view.view_proj;
    let row0 = vec3<f32>(m[0].x, m[1].x, m[2].x);
    let row1 = vec3<f32>(m[0].y, m[1].y, m[2].y);
    let light = normalize(cross(row0, row1) + vec3(1e-7));
    let side = normalize(cross(p.tangent, light) + vec3(1e-7));
    let s = select(-0.5, 0.5, (vertex & 1u) == 1u);
    let rel = p.pos + side * p.width * 3.0 * s;
    var clip = m * vec4<f32>(rel, 1.0);
    clip.z = max(clip.z, 0.0);
    return clip;
}
