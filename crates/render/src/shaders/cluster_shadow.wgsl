// Virtual geometry: vertex stage of the cluster path for the shadow cascades and the sky
// occlusion map. Composed after mesh_shadow.wgsl, cluster_types and cluster_fetch; placed with
// mesh_shadow.wgsl's `shadow_clip` like the instanced path's casters, masked ones cut out by
// its `fs_shadow_masked`.

// The corner as the instanced path's shadow vertex input.
fn cluster_shadow_in(c: ClusterCorner) -> VertexIn {
    let inst = cluster_instances[c.instance];
    var v: VertexIn;
    v.position = c.position;
    v.normal = vec4<f32>(c.normal, 0.0);
    v.uv = c.uv;
    v.color = c.color;
    v.m0 = inst.m0;
    v.m1 = inst.m1;
    v.m2 = inst.m2;
    v.n0 = inst.n0;
    v.n1 = inst.n1;
    v.n2 = inst.n2;
    v.extra = inst.extra;
    return v;
}

@vertex
fn vs_cluster_shadow(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> @builtin(position) vec4<f32> {
    let c = cluster_corner(vertex, instance);
    if (!c.valid) { return CLUSTER_NOWHERE; }
    return shadow_clip(cluster_shadow_in(c));
}

// Alpha-masked casters (leaf cards, fences): cut out by `fs_shadow_masked` as instances are.
@vertex
fn vs_cluster_shadow_masked(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> MaskedOut {
    let c = cluster_corner(vertex, instance);
    if (!c.valid) {
        var nothing: MaskedOut;
        nothing.clip = CLUSTER_NOWHERE;
        return nothing;
    }
    // As `vs_shadow_masked` (an entry point can't be called).
    let v = cluster_shadow_in(c);
    var out: MaskedOut;
    out.clip = shadow_clip(v);
    let mat = materials[v.extra.x];
    out.uv = v.uv * mat.uv.xy * mat.mapping.x;
    out.alpha = select(v.color.a, 1.0, mat.params2.y > 0.0);
    out.material = v.extra.x;
    return out;
}
