// Triangle records of the ray scene (ray_scene/tris.rs): what a hit reads of its triangle
// (`ray_tri_*` in lib_ray.wgsl). Written from the mesh pool's pages in the frame a geometry's
// structure is built, in two dispatches for every pair of pages: `cs_ray_uv_box` finds each
// geometry's uv box and its longest uv edge, `cs_ray_tris` packs the records against them. An
// invocation is a triangle of the dispatch; `jobs` tells which geometry it belongs to.
//
// A record is 5 words:
//   0  the uv of corner 0, 2 x unorm16 over the geometry's uv box
//   1  the uv edge to corner 1, 2 x snorm16 of the geometry's reach (a step is 1/2048 where
//      no edge of the geometry is longer than 16)
//   2  the uv edge to corner 2, likewise
//   3  the normals of corners 0 and 1, octahedral, 4 x snorm8
//   4  low half: the normal of corner 2; high half: the corners' mean colour, the square
//      root of each channel as r5 g6 b5 (red in the top bits)
// The record before a geometry's first, its header, holds the uv box's low corner (2 x f32),
// its size (2 x f32) and the reach (f32).

struct Job {
    // triangles of the jobs before it in this dispatch
    first_triangle: u32,
    triangles: u32,
    // its first index in the index page, and the vertex its indices count from
    first_index: u32,
    first_vertex: u32,
    // its header's record; its triangles' records follow
    header: u32,
};

// A pool page: 9 words a vertex (`PoolVertex`: position, normal as 4 x snorm16, uv, colour as
// 4 half floats), and its `u32` indices.
@group(0) @binding(0) var<storage, read> vertices: array<u32>;
@group(0) @binding(1) var<storage, read> indices: array<u32>;
@group(0) @binding(2) var<storage, read> jobs: array<Job>;
// 5 words a job: the uv box's low corner x, y and high corner x, y as keys that order as the
// floats do (`sortable`), and the longest uv edge along an axis (the bits of a float that is
// not negative order as it does).
@group(0) @binding(3) var<storage, read_write> boxes: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> records: array<u32>;

// Invocations in a row of the dispatch: 1024 groups of 64 (`ROW_GROUPS`, `GROUP` in tris.rs).
const ROW: u32 = 65536u;
// What an edge of 1 stands for at least: 32767 steps of 1/2048.
const EDGE_REACH: f32 = 15.99951171875;
// The largest uv a record tells apart.
const UV_MOST: f32 = 65536.0;

// The job of triangle `t` of the dispatch: the last whose first triangle is not past it.
fn job_of(t: u32) -> u32 {
    var lo = 0u;
    var hi = arrayLength(&jobs);
    while (hi - lo > 1u) {
        let mid = (lo + hi) / 2u;
        if (jobs[mid].first_triangle <= t) { lo = mid; } else { hi = mid; }
    }
    return lo;
}

// The vertices of the three corners of a job's triangle, in index order: the order the
// structure's triangle has (its `primitive_index` is `local`).
fn corners(job: Job, local: u32) -> vec3<u32> {
    let i = job.first_index + local * 3u;
    return vec3<u32>(indices[i], indices[i + 1u], indices[i + 2u]) + vec3<u32>(job.first_vertex);
}

// A uv as stored; 0 for one that is no number (it would spoil its geometry's box).
fn uv_word(bits: u32) -> f32 {
    let finite = (bits & 0x7f800000u) != 0x7f800000u;
    return clamp(bitcast<f32>(select(0u, bits, finite)), -UV_MOST, UV_MOST);
}

fn vertex_uv(v: u32) -> vec2<f32> {
    let w = v * 9u;
    return vec2<f32>(uv_word(vertices[w + 5u]), uv_word(vertices[w + 6u]));
}

fn vertex_position(v: u32) -> vec3<f32> {
    let w = v * 9u;
    return vec3<f32>(bitcast<f32>(vertices[w]), bitcast<f32>(vertices[w + 1u]), bitcast<f32>(vertices[w + 2u]));
}

// The vertex's normal; `face` where it has none.
fn vertex_normal(v: u32, face: vec3<f32>) -> vec3<f32> {
    let w = v * 9u;
    let n = vec3<f32>(unpack2x16snorm(vertices[w + 3u]), unpack2x16snorm(vertices[w + 4u]).x);
    let d = dot(n, n);
    if (d > 1e-8) { return n * inverseSqrt(d); }
    return face;
}

fn vertex_color(v: u32) -> vec3<f32> {
    let w = v * 9u;
    return vec3<f32>(unpack2x16float(vertices[w + 7u]), unpack2x16float(vertices[w + 8u]).x);
}

// A float as a number that orders the same way, for `atomicMin` and `atomicMax`, and back.
fn sortable(x: f32) -> u32 {
    let b = bitcast<u32>(x);
    return select(b ^ 0x80000000u, ~b, (b >> 31u) == 1u);
}

fn unsortable(k: u32) -> f32 {
    return bitcast<f32>(select(~k, k ^ 0x80000000u, (k >> 31u) == 1u));
}

// (Written only where it changes the word: most triangles of a geometry only read it.)
fn lower(at: u32, key: u32) {
    if (key < atomicLoad(&boxes[at])) { atomicMin(&boxes[at], key); }
}

fn raise(at: u32, key: u32) {
    if (key > atomicLoad(&boxes[at])) { atomicMax(&boxes[at], key); }
}

@compute @workgroup_size(64)
fn cs_ray_uv_box(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = id.y * ROW + id.x;
    let j = job_of(t);
    let job = jobs[j];
    let local = t - job.first_triangle;
    if (local >= job.triangles) { return; }
    let c = corners(job, local);
    let uv0 = vertex_uv(c.x);
    let uv1 = vertex_uv(c.y);
    let uv2 = vertex_uv(c.z);
    let lo = min(uv0, min(uv1, uv2));
    let hi = max(uv0, max(uv1, uv2));
    let e = max(abs(uv1 - uv0), abs(uv2 - uv0));
    let b = j * 5u;
    lower(b, sortable(lo.x));
    lower(b + 1u, sortable(lo.y));
    raise(b + 2u, sortable(hi.x));
    raise(b + 3u, sortable(hi.y));
    raise(b + 4u, bitcast<u32>(max(e.x, e.y)));
}

@compute @workgroup_size(64)
fn cs_ray_tris(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = id.y * ROW + id.x;
    let j = job_of(t);
    let job = jobs[j];
    let local = t - job.first_triangle;
    if (local >= job.triangles) { return; }
    // The geometry's uv box and reach, as `cs_ray_uv_box` left them.
    let b = j * 5u;
    let lo = vec2<f32>(unsortable(atomicLoad(&boxes[b])), unsortable(atomicLoad(&boxes[b + 1u])));
    let hi = vec2<f32>(unsortable(atomicLoad(&boxes[b + 2u])), unsortable(atomicLoad(&boxes[b + 3u])));
    let size = max(hi - lo, vec2<f32>(1e-6));
    let reach = max(bitcast<f32>(atomicLoad(&boxes[b + 4u])), EDGE_REACH);
    if (local == 0u) {
        let h = job.header * 5u;
        records[h] = bitcast<u32>(lo.x);
        records[h + 1u] = bitcast<u32>(lo.y);
        records[h + 2u] = bitcast<u32>(size.x);
        records[h + 3u] = bitcast<u32>(size.y);
        records[h + 4u] = bitcast<u32>(reach);
    }
    let c = corners(job, local);
    let p0 = vertex_position(c.x);
    var face = cross(vertex_position(c.y) - p0, vertex_position(c.z) - p0);
    let area = dot(face, face);
    if (area > 1e-30) { face = face * inverseSqrt(area); } else { face = vec3<f32>(0.0, 1.0, 0.0); }
    let n0 = oct_encode(vertex_normal(c.x, face));
    let n1 = oct_encode(vertex_normal(c.y, face));
    let n2 = oct_encode(vertex_normal(c.z, face));
    let uv0 = vertex_uv(c.x);
    let e1 = vertex_uv(c.y) - uv0;
    let e2 = vertex_uv(c.z) - uv0;
    let mean = (vertex_color(c.x) + vertex_color(c.y) + vertex_color(c.z)) / 3.0;
    let shade = sqrt(saturate3(mean));
    let rgb = (u32(round(shade.r * 31.0)) << 11u) | (u32(round(shade.g * 63.0)) << 5u) | u32(round(shade.b * 31.0));
    let r = (job.header + 1u + local) * 5u;
    records[r] = pack2x16unorm((uv0 - lo) / size);
    records[r + 1u] = pack2x16snorm(e1 / reach);
    records[r + 2u] = pack2x16snorm(e2 / reach);
    records[r + 3u] = pack4x8snorm(vec4<f32>(n0, n1));
    records[r + 4u] = (pack4x8snorm(vec4<f32>(n2, 0.0, 0.0)) & 0xffffu) | (rgb << 16u);
}
