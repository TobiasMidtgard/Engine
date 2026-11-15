// Probe GI's rays walked in software (probe_gi.wgsl composes after this): the meshes'
// hierarchies (probe_gi/bvh.rs) packed one after another, and one over the instances near the
// grid. The software path for GPUs without ray queries (probe_gi_trace_hw.wgsl).

struct BvhNode {
    min: vec3<f32>,
    right_or_first: u32,
    max: vec3<f32>,
    count: u32,
};

@group(1) @binding(1) var<storage, read> tlas: array<BvhNode>;
@group(1) @binding(3) var<storage, read> blas: array<BvhNode>;
@group(1) @binding(4) var<storage, read> triangles: array<vec4<f32>>;

const STACK: u32 = 48u;

fn safe_inverse(d: vec3<f32>) -> vec3<f32> {
    let tiny = select(vec3<f32>(-1e-20), vec3<f32>(1e-20), d >= vec3<f32>(0.0));
    return 1.0 / select(d, tiny, abs(d) < vec3<f32>(1e-20));
}

// Where the ray enters the box within `t_max`, or NO_HIT.
fn ray_box(origin: vec3<f32>, inv: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>, t_max: f32) -> f32 {
    let t0 = (lo - origin) * inv;
    let t1 = (hi - origin) * inv;
    let near = max(max(min(t0.x, t1.x), min(t0.y, t1.y)), max(min(t0.z, t1.z), 0.0));
    let far = min(min(max(t0.x, t1.x), max(t0.y, t1.y)), min(max(t0.z, t1.z), t_max));
    return select(NO_HIT, near, near <= far);
}

// Möller-Trumbore: the distance to triangle `t` along the ray, or NO_HIT.
fn ray_triangle(origin: vec3<f32>, dir: vec3<f32>, t: u32) -> f32 {
    let a = triangles[t * 3u].xyz;
    let e1 = triangles[t * 3u + 1u].xyz - a;
    let e2 = triangles[t * 3u + 2u].xyz - a;
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if (abs(det) < 1e-14) { return NO_HIT; }
    let inv_det = 1.0 / det;
    let s = origin - a;
    let u = dot(s, p) * inv_det;
    if (u < 0.0 || u > 1.0) { return NO_HIT; }
    let q = cross(s, e1);
    let v = dot(dir, q) * inv_det;
    if (v < 0.0 || u + v > 1.0) { return NO_HIT; }
    let d = dot(e2, q) * inv_det;
    return select(NO_HIT, d, d > 0.0);
}

// Both hierarchies are walked nearest child first (Aila and Laine, "Understanding the Efficiency
// of Ray Traversal on GPUs", HPG 2009): an inner node tests both children's boxes, goes on into
// the nearer one and keeps the farther one on the stack with where the ray enters it, skipped
// when popped if a hit nearer than that was found meanwhile. A ray through a crowd of trees
// finds the near leaves first and leaves the far crowns unopened. The stack holds the node and
// the entry distance's bits.

// The nearest triangle of instance `i` along an anchor-relative ray, if nearer than `best`.
// `first_hit`: stop at the first hit (shadow rays).
fn trace_instance(i: u32, origin_rel: vec3<f32>, dir_rel: vec3<f32>, first_hit: bool, best: ptr<function, Hit>) {
    let inst = instances[i];
    let o4 = vec4<f32>(origin_rel, 1.0);
    let d4 = vec4<f32>(dir_rel, 0.0);
    // Affine, so the ray's parameter is the same in the mesh's space.
    let origin = vec3<f32>(dot(inst.to_local_x, o4), dot(inst.to_local_y, o4), dot(inst.to_local_z, o4));
    let dir = vec3<f32>(dot(inst.to_local_x, d4), dot(inst.to_local_y, d4), dot(inst.to_local_z, d4));
    let inv = safe_inverse(dir);
    let root = inst.mesh.x;
    if (ray_box(origin, inv, blas[root].min, blas[root].max, (*best).t) >= NO_HIT) { return; }
    var stack: array<vec2<u32>, STACK>;
    var top = 0u;
    var index = root;
    loop {
        let node = blas[index];
        if (node.count > 0u) {
            for (var k = 0u; k < node.count; k++) {
                let tri = inst.mesh.y + node.right_or_first + k;
                let d = ray_triangle(origin, dir, tri);
                if (d < (*best).t) {
                    (*best).t = d;
                    (*best).instance = i;
                    (*best).triangle = tri;
                    if (first_hit) { return; }
                }
            }
        } else {
            // Children: the left one right after its parent, the right one where it says.
            let a = index + 1u;
            let b = root + node.right_or_first;
            let ta = ray_box(origin, inv, blas[a].min, blas[a].max, (*best).t);
            let tb = ray_box(origin, inv, blas[b].min, blas[b].max, (*best).t);
            if (ta < NO_HIT && tb < NO_HIT) {
                let a_first = ta <= tb;
                if (top < STACK) {
                    stack[top] = vec2<u32>(select(a, b, a_first), bitcast<u32>(max(ta, tb)));
                    top += 1u;
                }
                index = select(b, a, a_first);
                continue;
            }
            if (ta < NO_HIT) { index = a; continue; }
            if (tb < NO_HIT) { index = b; continue; }
        }
        // The nearest pending node still in front of the best hit.
        var next = false;
        loop {
            if (top == 0u) { break; }
            top -= 1u;
            if (bitcast<f32>(stack[top].y) < (*best).t) {
                index = stack[top].x;
                next = true;
                break;
            }
        }
        if (!next) { break; }
    }
}

// The nearest hit along a camera-relative ray within `t_max` (`instance` all ones: none).
fn trace_scene(origin: vec3<f32>, dir: vec3<f32>, t_max: f32, first_hit: bool) -> Hit {
    var best: Hit;
    best.t = t_max;
    best.instance = 0xffffffffu;
    best.triangle = 0u;
    best.local_n = vec3<f32>(0.0);
    if (u32(pass_.params.z) == 0u) { return best; }
    let o = origin - pass_.anchor.xyz;
    let inv = safe_inverse(dir);
    if (ray_box(o, inv, tlas[0].min, tlas[0].max, best.t) >= NO_HIT) { return best; }
    var stack: array<vec2<u32>, STACK>;
    var top = 0u;
    var index = 0u;
    loop {
        let node = tlas[index];
        if (node.count > 0u) {
            for (var k = 0u; k < node.count; k++) {
                trace_instance(node.right_or_first + k, o, dir, first_hit, &best);
                if (first_hit && best.instance != 0xffffffffu) { return best; }
            }
        } else {
            let a = index + 1u;
            let b = node.right_or_first;
            let ta = ray_box(o, inv, tlas[a].min, tlas[a].max, best.t);
            let tb = ray_box(o, inv, tlas[b].min, tlas[b].max, best.t);
            if (ta < NO_HIT && tb < NO_HIT) {
                let a_first = ta <= tb;
                if (top < STACK) {
                    stack[top] = vec2<u32>(select(a, b, a_first), bitcast<u32>(max(ta, tb)));
                    top += 1u;
                }
                index = select(b, a, a_first);
                continue;
            }
            if (ta < NO_HIT) { index = a; continue; }
            if (tb < NO_HIT) { index = b; continue; }
        }
        var next = false;
        loop {
            if (top == 0u) { break; }
            top -= 1u;
            if (bitcast<f32>(stack[top].y) < best.t) {
                index = stack[top].x;
                next = true;
                break;
            }
        }
        if (!next) { break; }
    }
    return best;
}

// The normal of the triangle a ray hit, in its mesh's space (unnormalized).
fn hit_normal_local(hit: Hit) -> vec3<f32> {
    let a = triangles[hit.triangle * 3u].xyz;
    return cross(triangles[hit.triangle * 3u + 1u].xyz - a, triangles[hit.triangle * 3u + 2u].xyz - a);
}
