// Virtual geometry culling for every view at once: the camera, the shadow cascades, the sky
// occlusion map and the far shadow map. Requires lib_types and cluster_types. Run as four dispatches:
//
// 1. `cs_instances`, one thread per (view, instance): cull the instance against the view, skip
//    whole levels whose clusters can't be picked from this distance, and append the rest as
//    chunks of 64 clusters.
// 2. `cs_dispatch`: turn the chunk count into indirect dispatch arguments.
// 3. `cs_clusters`, one thread per cluster of each chunk: level of detail (its own error fits
//    under the threshold and its parent's doesn't), frustum and normal cone culling, then append
//    it to its draw slot's visible list and count it in the slot's indirect draw arguments.
// 4. `cs_finish`, one thread per draw slot: clamp each draw to its list and total the stats.
//
// Only 32-bit atomics: works on every backend.

const VIEWS: u32 = 7u;
const CHUNK: u32 = 64u;

struct CullView {
    planes: array<vec4<f32>, 6>,  // camera-relative, inside where dot(xyz, p) + w >= 0
    eye: vec4<f32>,               // camera-relative eye, w = 1 perspective / 0 orthographic
    forward: vec4<f32>,           // view direction, w = 1 to cull clusters facing away
    lod: vec4<f32>,               // pixels per meter (at 1 m when perspective), threshold (px), nearest distance (m), smallest instance radius drawn (m)
    slots: vec4<u32>,             // first draw slot, 1 = one slot for all buckets, entries per slot, 1 = enabled
};

struct CullParams {
    counts: vec4<u32>,            // instances, chunk capacity, draw slots, buckets in the camera view
    views: array<CullView, 7>,
};

// `wgpu::util::DrawIndirectArgs`, filled by the culling.
struct DrawArgs {
    vertex_count: u32,
    instance_count: atomic<u32>,
    first_vertex: u32,            // slot * entries per slot * CLUSTER_VERTEX_COUNT: where the slot's list starts
    first_instance: u32,
};

struct Draws {
    chunk_count: atomic<u32>,
    overflow: atomic<u32>,        // chunks and clusters dropped because a list was full
    unused: vec2<u32>,
    triangles: array<atomic<u32>, 7>,  // drawn per view
    clusters: array<atomic<u32>, 7>,   // drawn per view
    pad: array<u32, 14>,          // the header is 128 bytes
    slots: array<DrawArgs>,
};

@group(0) @binding(0) var<uniform> cull: CullParams;
@group(0) @binding(1) var<storage, read> instances: array<ClusterInstance>;
@group(0) @binding(2) var<storage, read> clusters: array<Cluster>;
// Per level: first cluster, cluster count, smallest own error (f32 bits), largest parent error (f32 bits).
@group(0) @binding(3) var<storage, read> levels: array<vec4<u32>>;
// Per chunk: view, instance, first cluster, end cluster.
@group(0) @binding(4) var<storage, read_write> chunks: array<vec4<u32>>;
@group(0) @binding(5) var<storage, read_write> draws: Draws;
// Per drawn cluster: instance, cluster. Each draw slot owns `entries per slot` of them.
@group(0) @binding(6) var<storage, read_write> visible: array<vec2<u32>>;
// Indirect dispatch arguments for `cs_clusters` (written by `cs_dispatch` only).
@group(1) @binding(0) var<storage, read_write> dispatch: array<u32, 4>;

fn in_view(view: CullView, center: vec3<f32>, radius: f32) -> bool {
    for (var k = 0; k < 6; k++) {
        let p = view.planes[k];
        if (dot(p.xyz, center) + p.w < -radius) { return false; }
    }
    return true;
}

fn to_view(inst: ClusterInstance, p: vec3<f32>) -> vec3<f32> {
    let q = vec4<f32>(p, 1.0);
    return vec3<f32>(dot(inst.m0, q), dot(inst.m1, q), dot(inst.m2, q));
}

// Pixels an error of `error` meters over a sphere can cover: the most at its nearest point.
fn projected_error(view: CullView, error: f32, center: vec3<f32>, radius: f32) -> f32 {
    if (view.eye.w < 0.5) { return error * view.lod.x; }
    let d = max(distance(center, view.eye.xyz) - radius, view.lod.z);
    return error * view.lod.x / d;
}

// A cluster's (or its parent's) error on screen. Both level-of-detail tests go through here, so
// the clusters of a group and the clusters made from it decide with identical numbers and
// exactly one side of every group is drawn.
fn lod_error(view: CullView, inst: ClusterInstance, error: f32, sphere: vec4<f32>) -> f32 {
    let scale = inst.lod.x;
    return projected_error(view, error * scale, to_view(inst, sphere.xyz), sphere.w * scale);
}

// Whether every triangle of a cluster faces away from the view: each normal in the cone (axis,
// cosine of its half angle) points away from every point of the bounding sphere.
fn backfacing(view: CullView, center: vec3<f32>, radius: f32, axis: vec3<f32>, cos_a: f32) -> bool {
    var dir = view.forward.xyz;
    var margin = 0.0;
    if (view.eye.w > 0.5) {
        let d = center - view.eye.xyz;
        let len = length(d);
        if (len <= radius) { return false; }
        dir = d / len;
        margin = radius / len;
    }
    let cos_t = dot(dir, axis);
    let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
    let sin_a = sqrt(max(1.0 - cos_a * cos_a, 0.0));
    // cos(angle to the view direction + cone half angle) >= radius / distance
    return cos_t * cos_a - sin_t * sin_a >= margin;
}

@compute @workgroup_size(64)
fn cs_instances(@builtin(global_invocation_id) id: vec3<u32>) {
    let count = max(cull.counts.x, 1u);
    let v = id.x / count;
    let i = id.x % count;
    if (v >= VIEWS || i >= cull.counts.x) { return; }
    let view = cull.views[v];
    if (view.slots.w == 0u) { return; }
    let inst = instances[i];
    if (v != 0u && (inst.extra.y & INSTANCE_CASTS_SHADOWS) == 0u) { return; }
    let sphere = inst.sphere;
    if (sphere.w < view.lod.w || !in_view(view, sphere.xyz, sphere.w)) { return; }

    // Every error sphere lies inside the instance's sphere, so over a whole level the projected
    // errors are at most (largest error) / (nearest distance) and at least (smallest error) /
    // (farthest distance).
    var near = 1.0;
    var far = 1.0;
    if (view.eye.w > 0.5) {
        let d = distance(sphere.xyz, view.eye.xyz);
        near = max(d - sphere.w, view.lod.z);
        far = max(d + sphere.w, view.lod.z);
    }
    let k = inst.lod.x * view.lod.x;
    let threshold = view.lod.y;
    for (var l = 0u; l < inst.mesh.y; l++) {
        let level = levels[inst.mesh.x + l];
        let largest_parent = bitcast<f32>(level.w) * k / near;
        let smallest_own = bitcast<f32>(level.z) * k / far;
        // All parents fit on screen (a coarser level is drawn), or no cluster of the level fits
        // (a finer one is). A small margin keeps rounding from skipping a level that is needed.
        if (largest_parent * 1.001 <= threshold || smallest_own * 0.999 > threshold) { continue; }
        let n = (level.y + CHUNK - 1u) / CHUNK;
        let base = atomicAdd(&draws.chunk_count, n);
        for (var c = 0u; c < n; c++) {
            if (base + c >= cull.counts.y) {
                atomicAdd(&draws.overflow, n - c);
                break;
            }
            let first = level.x + c * CHUNK;
            chunks[base + c] = vec4<u32>(v, i, first, min(first + CHUNK, level.x + level.y));
        }
    }
}

@compute @workgroup_size(1)
fn cs_dispatch() {
    let n = min(atomicLoad(&draws.chunk_count), cull.counts.y);
    dispatch[0] = min(n, 65535u);
    dispatch[1] = (n + 65534u) / 65535u;
    dispatch[2] = 1u;
    dispatch[3] = 0u;
}

@compute @workgroup_size(64)
fn cs_clusters(
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
    @builtin(local_invocation_index) lane: u32,
) {
    let chunk_index = group.x + group.y * groups.x;
    if (chunk_index >= min(atomicLoad(&draws.chunk_count), cull.counts.y)) { return; }
    let chunk = chunks[chunk_index];
    let c = chunk.z + lane;
    if (c >= chunk.w) { return; }
    let v = chunk.x;
    let view = cull.views[v];
    let inst = instances[chunk.y];
    let cluster = clusters[c];
    let threshold = view.lod.y;

    let own = lod_error(view, inst, cluster.errors.x, cluster.lod_bounds);
    let parent = lod_error(view, inst, cluster.errors.y, cluster.parent_bounds);
    if (own > threshold || parent <= threshold) { return; }

    let center = to_view(inst, cluster.bounds.xyz);
    let radius = cluster.bounds.w * inst.lod.x;
    if (!in_view(view, center, radius)) { return; }
    if (view.forward.w > 0.5 && (inst.extra.y & INSTANCE_CONE_CULL) != 0u && cluster.cone.w > 0.0) {
        let axis = normalize(mat3x3<f32>(inst.n0.xyz, inst.n1.xyz, inst.n2.xyz) * cluster.cone.xyz);
        if (backfacing(view, center, radius, axis, cluster.cone.w)) { return; }
    }

    let bucket = select(inst.mesh.z, 0u, view.slots.y != 0u);
    let slot = view.slots.x + bucket;
    let index = atomicAdd(&draws.slots[slot].instance_count, 1u);
    if (index >= view.slots.z) { return; }  // the list is full (counted in cs_finish)
    visible[slot * view.slots.z + index] = vec2<u32>(chunk.y, c);
    atomicAdd(&draws.triangles[v], cluster.data.z >> 16u);
}

@compute @workgroup_size(64)
fn cs_finish(@builtin(global_invocation_id) id: vec3<u32>) {
    let slot = id.x;
    if (slot >= cull.counts.z) { return; }
    var v = 0u;
    for (var k = 0u; k < VIEWS; k++) {
        let s = cull.views[k].slots;
        let n = select(cull.counts.w, 1u, s.y != 0u);
        if (slot >= s.x && slot < s.x + n) { v = k; }
    }
    let capacity = cull.views[v].slots.z;
    let n = atomicLoad(&draws.slots[slot].instance_count);
    let kept = min(n, capacity);
    atomicStore(&draws.slots[slot].instance_count, kept);
    atomicAdd(&draws.clusters[v], kept);
    if (n > kept) { atomicAdd(&draws.overflow, n - kept); }
}
