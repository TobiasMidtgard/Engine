// GPU skinning. Requires lib_types (no frame bindings).
//
// `skin`: morph targets (gathered per vertex), then linear blend skinning with up to four
// joints, into this instance's slice of `out_vertices`. Also sums muscle activation
// (morph influence × weight) for vein and flush detail.
//
// `tension`: runs after every `skin` dispatch finished (separate pass). Average relative
// edge-length change against the rest pose: < 0 compressed (cloth bunches, folds),
// > 0 stretched (fabric pulls tight).

struct Job {
    vertex_base: u32,  // first vertex of the mesh in rest/ranges
    vertex_count: u32,
    out_base: u32,     // first output vertex of this instance
    joint_base: u32,   // first joint matrix of this instance
    morph_base: u32,   // first morph weight of this instance
    flags: u32,        // 1: apply morphs, 2: tension pass runs
    pad0: u32,
    pad1: u32,
};

struct RestVertex {
    position: vec4<f32>,
    normal: vec4<f32>,
    weights: vec4<f32>,
    joints: vec4<u32>,
};

struct MorphEntry {
    position: vec4<f32>,  // delta xyz, w = muscle influence
    normal: vec4<f32>,    // delta xyz, w = target index (u32 bits)
};

struct SkinnedVertex {
    position: vec3<f32>,
    tension: f32,
    normal: vec3<f32>,
    activation: f32,
};

const JOB_MORPHS: u32 = 1u;

@group(0) @binding(0) var<uniform> job: Job;
@group(0) @binding(1) var<storage, read> rest: array<RestVertex>;
@group(0) @binding(2) var<storage, read> joints: array<vec4<f32>>;      // 3 rows per joint
@group(0) @binding(3) var<storage, read> morph_weights: array<f32>;
@group(0) @binding(4) var<storage, read> ranges: array<vec4<u32>>;      // morph start, morph count, neighbor start, neighbor count
@group(0) @binding(5) var<storage, read> morphs: array<MorphEntry>;
@group(0) @binding(6) var<storage, read> neighbors: array<vec2<u32>>;   // vertex (mesh-local), rest length (f32 bits)
@group(0) @binding(7) var<storage, read_write> out_vertices: array<SkinnedVertex>;

@compute @workgroup_size(64)
fn skin(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= job.vertex_count) {
        return;
    }
    let v = rest[job.vertex_base + i];
    var p = v.position.xyz;
    var n = v.normal.xyz;
    var activation = 0.0;
    if ((job.flags & JOB_MORPHS) != 0u) {
        let r = ranges[job.vertex_base + i];
        for (var k = 0u; k < r.y; k++) {
            let e = morphs[r.x + k];
            let w = morph_weights[job.morph_base + bitcast<u32>(e.normal.w)];
            p += e.position.xyz * w;
            n += e.normal.xyz * w;
            activation += e.position.w * max(w, 0.0);
        }
    }
    var r0 = vec4<f32>(0.0);
    var r1 = vec4<f32>(0.0);
    var r2 = vec4<f32>(0.0);
    var total = 0.0;
    for (var k = 0u; k < 4u; k++) {
        let w = v.weights[k];
        if (w > 0.0) {
            let j = (job.joint_base + v.joints[k]) * 3u;
            r0 += joints[j] * w;
            r1 += joints[j + 1u] * w;
            r2 += joints[j + 2u] * w;
            total += w;
        }
    }
    if (total <= 0.0) {
        r0 = vec4<f32>(1.0, 0.0, 0.0, 0.0);
        r1 = vec4<f32>(0.0, 1.0, 0.0, 0.0);
        r2 = vec4<f32>(0.0, 0.0, 1.0, 0.0);
    }
    let ph = vec4<f32>(p, 1.0);
    let sp = vec3<f32>(dot(r0, ph), dot(r1, ph), dot(r2, ph));
    let sn = vec3<f32>(dot(r0.xyz, n), dot(r1.xyz, n), dot(r2.xyz, n));
    let len = length(sn);
    var out: SkinnedVertex;
    out.position = sp;
    out.tension = 0.0;
    out.normal = select(vec3<f32>(0.0, 1.0, 0.0), sn / len, len > 1e-8);
    out.activation = saturate(activation);
    out_vertices[job.out_base + i] = out;
}

@compute @workgroup_size(64)
fn tension(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= job.vertex_count) {
        return;
    }
    let r = ranges[job.vertex_base + i];
    let p = out_vertices[job.out_base + i].position;
    var sum = 0.0;
    var count = 0.0;
    for (var k = 0u; k < r.w; k++) {
        let nb = neighbors[r.z + k];
        if (nb.x >= job.vertex_count) {
            continue;
        }
        let q = out_vertices[job.out_base + nb.x].position;
        let rest_len = max(bitcast<f32>(nb.y), 1e-6);
        sum += (length(q - p) - rest_len) / rest_len;
        count += 1.0;
    }
    out_vertices[job.out_base + i].tension = clamp(sum / max(count, 1.0), -1.0, 1.0);
}
