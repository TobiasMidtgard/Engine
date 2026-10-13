// Virtual geometry: vertex stages of the cluster path for the depth prepass and the scene pass.
// Composed after mesh.wgsl, cluster_types and cluster_fetch: the fragment stages are mesh.wgsl's
// `fs_prepass` and `fs_main`, and the vertex math is mesh.wgsl's `standard_vertex` and
// `standard_prepass_vertex`, so clustered meshes shade exactly like the instanced path.

// The corner as the instanced path's vertex input.
fn cluster_vertex_in(c: ClusterCorner) -> VertexIn {
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
    v.p0 = inst.p0;
    v.p1 = inst.p1;
    v.p2 = inst.p2;
    return v;
}

// Debug views: every cluster in its own color (`DebugView::Clusters`), or the level of detail
// from blue (full detail) through cyan, green, yellow, orange, red and magenta to white
// (`DebugView::ClusterLevels`).
fn cluster_debug_color(c: ClusterCorner) -> vec4<f32> {
    if (frame.counts.z == 8u) {
        var palette = array<vec3<f32>, 8>(
            vec3<f32>(0.10, 0.30, 1.00),
            vec3<f32>(0.10, 0.80, 0.90),
            vec3<f32>(0.15, 0.85, 0.25),
            vec3<f32>(0.90, 0.90, 0.15),
            vec3<f32>(1.00, 0.55, 0.10),
            vec3<f32>(0.95, 0.15, 0.10),
            vec3<f32>(0.85, 0.20, 0.80),
            vec3<f32>(0.95, 0.95, 0.95),
        );
        return vec4<f32>(palette[min(clusters[c.cluster].data.w, 7u)], 1.0);
    }
    // A hashed hue per cluster.
    var h = c.cluster * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    h = (h >> 22u) ^ h;
    let hue = f32(h & 0xffffu) / 65535.0;
    let rgb = saturate3(abs(fract(vec3<f32>(hue) + vec3<f32>(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0) - 1.0);
    return vec4<f32>(mix(vec3<f32>(1.0), rgb, 0.75) * 0.95, 1.0);
}

@vertex
fn vs_cluster(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> VertexOut {
    let c = cluster_corner(vertex, instance);
    if (!c.valid) {
        var nothing: VertexOut;
        nothing.clip = CLUSTER_NOWHERE;
        return nothing;
    }
    var out = standard_vertex(cluster_vertex_in(c));
    if (frame.counts.z == 7u || frame.counts.z == 8u) {
        out.color = cluster_debug_color(c);
    }
    return out;
}

@vertex
fn vs_cluster_prepass(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> PrepassVertexOut {
    let c = cluster_corner(vertex, instance);
    if (!c.valid) {
        var nothing: PrepassVertexOut;
        nothing.clip = CLUSTER_NOWHERE;
        return nothing;
    }
    return standard_prepass_vertex(cluster_vertex_in(c));
}
