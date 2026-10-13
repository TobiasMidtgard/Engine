// Virtual geometry: vertex stage of the cluster path for the shadow cascades and the sky
// occlusion map. Composed after mesh_shadow.wgsl, cluster_types and cluster_fetch; placed with
// mesh_shadow.wgsl's `shadow_clip`, like the instanced path's opaque casters.

@vertex
fn vs_cluster_shadow(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> @builtin(position) vec4<f32> {
    let c = cluster_corner(vertex, instance);
    if (!c.valid) { return CLUSTER_NOWHERE; }
    let inst = cluster_instances[c.instance];
    var v: VertexIn;
    v.position = c.position;
    v.normal = c.normal;
    v.uv = c.uv;
    v.color = c.color;
    v.m0 = inst.m0;
    v.m1 = inst.m1;
    v.m2 = inst.m2;
    v.n0 = inst.n0;
    v.n1 = inst.n1;
    v.n2 = inst.n2;
    v.extra = inst.extra;
    return shadow_clip(v);
}
