// Probe GI's rays on ray-tracing hardware (probe_gi.wgsl composes after this): ray queries
// against one acceleration structure over the instances near the grid, each instance a mesh's
// own (probe_gi/rt.rs). The same scene and the same answers as the software walk
// (probe_gi_trace_sw.wgsl), which GPUs without ray queries keep.
enable wgpu_ray_query;
enable wgpu_ray_query_vertex_return;

@group(1) @binding(1) var scene: acceleration_structure<vertex_return>;

// The smallest normal f32: a query's nearest hit, so that a hit at exactly 0 doesn't count.
const T_MIN: f32 = 1.17549435e-38;

// The nearest hit along a camera-relative ray within `t_max` (`instance` all ones: none), as
// the software walk finds it: no face is culled, and a hit counts at 0 < t < t_max (a query
// takes tmin <= t <= tmax, so it starts at the smallest normal float and ends one float short
// of `t_max`: a probe exactly on a face, or a lifted ray starting exactly on a wall's plane,
// doesn't hit it at 0, as the walk's `d > 0` doesn't). `first_hit`: stop at the first hit
// (shadow rays). Every instance and geometry is opaque and the query forces it besides, so no
// candidate ever waits for the loop to confirm it.
fn trace_scene(origin: vec3<f32>, dir: vec3<f32>, t_max: f32, first_hit: bool) -> Hit {
    var best: Hit;
    best.t = t_max;
    best.instance = 0xffffffffu;
    best.triangle = 0u;
    best.local_n = vec3<f32>(0.0);
    // No instance, or nothing to cross (a query's range must be a number, at least 0).
    if (u32(pass_.params.z) == 0u || !(t_max > 0.0)) { return best; }
    let t_end = bitcast<f32>(bitcast<u32>(t_max) - 1u);
    var flags = RAY_FLAG_FORCE_OPAQUE;
    if (first_hit) { flags = RAY_FLAG_FORCE_OPAQUE | RAY_FLAG_TERMINATE_ON_FIRST_HIT; }
    var rq: ray_query<vertex_return>;
    rayQueryInitialize(&rq, scene, RayDesc(flags, 0xffu, T_MIN, t_end, origin - pass_.anchor.xyz, dir));
    while (rayQueryProceed(&rq)) {}
    let hit = rayQueryGetCommittedIntersection(&rq);
    if (hit.kind != RAY_QUERY_INTERSECTION_NONE) {
        best.t = hit.t;
        best.instance = hit.instance_custom_data;
        best.triangle = hit.primitive_index;
        if (!first_hit) {
            // The corners in the mesh's space, in index order.
            let v = getCommittedHitVertexPositions(&rq);
            best.local_n = cross(v[1] - v[0], v[2] - v[0]);
        }
    }
    return best;
}

// The normal of the triangle a ray hit, in its mesh's space (unnormalized).
fn hit_normal_local(hit: Hit) -> vec3<f32> {
    return hit.local_n;
}
