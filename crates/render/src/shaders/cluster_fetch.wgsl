// Virtual geometry drawing: the vertex stage fetches its triangle corner from storage buffers.
// Requires cluster_types. Group 3 of the cluster draw pipelines (virtual_geometry/gpu.rs).
//
// Each visible cluster is one instance of a non-indexed draw of CLUSTER_VERTEX_COUNT vertices.
// The draw's first vertex is `slot * entries per slot * CLUSTER_VERTEX_COUNT`, so
// `vertex_index / CLUSTER_VERTEX_COUNT` is where its slot's visible list starts (indirect draws
// can set the first vertex on every backend; the first instance needs an optional feature).

// Vertices of every clustered mesh: position xyz + normal x, normal yz + uv, color.
@group(3) @binding(0) var<storage, read> cluster_vertices: array<vec4<f32>>;
// Per cluster: its vertex indices, then its triangles (three 8-bit corners each).
@group(3) @binding(1) var<storage, read> cluster_indices: array<u32>;
@group(3) @binding(2) var<storage, read> clusters: array<Cluster>;
@group(3) @binding(3) var<storage, read> cluster_instances: array<ClusterInstance>;
// Per drawn cluster: instance, cluster (written by cluster_cull.wgsl).
@group(3) @binding(4) var<storage, read> visible_clusters: array<vec2<u32>>;

struct ClusterCorner {
    instance: u32,
    cluster: u32,
    // False for the corners of a triangle the cluster doesn't have.
    valid: bool,
    position: vec3<f32>,
    normal: vec3<f32>,
    uv: vec2<f32>,
    color: vec4<f32>,
};

fn cluster_corner(vertex_index: u32, instance_index: u32) -> ClusterCorner {
    let entry = visible_clusters[vertex_index / CLUSTER_VERTEX_COUNT + instance_index];
    let corner_index = vertex_index % CLUSTER_VERTEX_COUNT;
    let triangle = corner_index / 3u;
    var corner = corner_index % 3u;
    let cluster = clusters[entry.y];
    var out: ClusterCorner;
    out.instance = entry.x;
    out.cluster = entry.y;
    out.valid = triangle < (cluster.data.z >> 16u);
    if (!out.valid) { return out; }
    if ((cluster_instances[entry.x].extra.y & INSTANCE_MIRRORED) != 0u && corner != 0u) {
        corner = 3u - corner;
    }
    let packed = cluster_indices[cluster.data.x + (cluster.data.z & 0xffffu) + triangle];
    let local = (packed >> (corner * 8u)) & 0xffu;
    let v = (cluster.data.y + cluster_indices[cluster.data.x + local]) * 3u;
    let a = cluster_vertices[v];
    let b = cluster_vertices[v + 1u];
    out.position = a.xyz;
    out.normal = vec3<f32>(a.w, b.x, b.y);
    out.uv = b.zw;
    out.color = cluster_vertices[v + 2u];
    return out;
}

// Where the corners of missing triangles go: one point, so they draw nothing.
const CLUSTER_NOWHERE: vec4<f32> = vec4<f32>(0.0, 0.0, 0.0, 1.0);
