// Hair strand data and ribbon expansion, shared by the color, prepass and shadow passes.
// Requires lib_types + common.wgsl.
//
// Every hair is a polyline of `points_per` points; instance i draws hair i as a ribbon of two
// triangles per segment.

struct StrandParams {
    info: vec4<u32>,   // points per hair, kind (0 scalp, 1 brows, 2 lashes, 3 beard), hairs, unused
};

@group(2) @binding(0) var<uniform> strand_params: StrandParams;
@group(2) @binding(1) var<storage, read> strand_points: array<vec4<f32>>;  // xyz model space, radius
@group(2) @binding(2) var<storage, read> strand_attrs: array<vec4<f32>>;   // occlusion, random, t, unused

struct StrandOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) tangent: vec3<f32>,
    @location(2) attrs: vec4<f32>,
    @location(3) coverage: f32,
    @location(4) @interpolate(flat) hair: u32,
    @location(5) clip_now: vec4<f32>,
    @location(6) clip_prev: vec4<f32>,
};

struct RibbonPoint {
    model: vec3<f32>,
    rel: vec3<f32>,
    tangent: vec3<f32>,
    radius: f32,
    index: u32,
};

fn ribbon_point(hair: u32, vertex: u32) -> RibbonPoint {
    let n = strand_params.info.x;
    let segment = vertex / 6u;
    let corner = vertex % 6u;
    // Two triangles per segment: (0,-) (0,+) (1,-) / (1,-) (0,+) (1,+).
    var end = array<u32, 6>(0u, 0u, 1u, 1u, 0u, 1u);
    let k = min(segment + end[corner], n - 1u);
    let base = hair * n;
    let i = base + k;
    let p = strand_points[i];
    let prev = strand_points[base + select(k - 1u, 0u, k == 0u)].xyz;
    let next = strand_points[base + min(k + 1u, n - 1u)].xyz;
    var out: RibbonPoint;
    out.model = p.xyz;
    out.rel = model_point(p.xyz);
    out.tangent = model_dir(next - prev + vec3<f32>(0.0, 1e-7, 0.0));
    out.radius = p.w;
    out.index = i;
    return out;
}

fn ribbon_side(corner: u32) -> f32 {
    var side = array<f32, 6>(-1.0, 1.0, -1.0, -1.0, 1.0, 1.0);
    return side[corner];
}

