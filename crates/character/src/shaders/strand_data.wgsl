// Hair strand data and ribbon expansion, shared by the color, prepass and shadow passes.
// Requires lib_types + common.wgsl.
//
// Every hair is a polyline of `points_per` points; instance i draws hair i as a ribbon of two
// triangles per segment.

struct StrandParams {
    info: vec4<u32>,   // points per hair, kind (0 scalp, 1 brows, 2 lashes, 3 beard), hairs, guides
    // Detail by distance: hair width scale (fewer hairs drawn, each wider), points stepped
    // over per segment (1 every point).
    lod: vec4<f32>,
};

@group(2) @binding(0) var<uniform> strand_params: StrandParams;
@group(2) @binding(1) var<storage, read> strand_points: array<vec4<f32>>;  // xyz model space, radius
@group(2) @binding(2) var<storage, read> strand_attrs: array<vec4<f32>>;   // occlusion, random, t, unused
// Per hair: the skin vertex it grows from (its expressions move the hair), that vertex's four
// bones and their weights (bytes), the guide it moves with (0xffffffff: none).
@group(2) @binding(3) var<storage, read> strand_roots: array<vec4<u32>>;
// Hair that moves (`info.w` guides): how far each guide point moved from where the pose alone
// carries it (posed model space), this frame's for all guides, then last frame's.
@group(2) @binding(4) var<storage, read> strand_motion: array<vec4<f32>>;
// Per hair that moves: the three guides it moves with and their weights (bytes, 0..255).
@group(2) @binding(5) var<storage, read> strand_follow: array<vec4<u32>>;

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
    // Where the point was last frame (posed model space).
    prev_model: vec3<f32>,
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
    let stride = max(u32(strand_params.lod.y + 0.5), 1u);
    let k = min((segment + end[corner]) * stride, n - 1u);
    let base = hair * n;
    let i = base + k;
    let p = strand_points[i];
    let prev = strand_points[base + select(k - 1u, 0u, k == 0u)].xyz;
    let next = strand_points[base + min(k + 1u, n - 1u)].xyz;
    // The hair rides its root: the root's expressions, then its bones.
    let root = strand_roots[hair];
    let m = morph_at(root.x);
    let bones = unpack_bytes(root.y);
    let weights = vec4<f32>(unpack_bytes(root.z)) / 255.0;
    let now = skin_matrix(bones, weights, false);
    var out: RibbonPoint;
    out.model = (now * vec4<f32>(p.xyz + m.dp, 1.0)).xyz;
    out.prev_model = (skin_matrix(bones, weights, true) * vec4<f32>(p.xyz + m.dp_prev, 1.0)).xyz;
    var along = (now * vec4<f32>(next - prev + vec3<f32>(0.0, 1e-7, 0.0), 0.0)).xyz;
    // Hair that moves goes where its guide went.
    let guides = strand_params.info.w;
    if (guides > 0u && root.w < guides) {
        // A blend of its nearest guides, so neighboring hairs don't part where guides swing
        // apart.
        let f = strand_follow[hair];
        let w = unpack4x8unorm(f.w);
        let half = guides * n;
        var sum = 0.0;
        var now_d = vec3<f32>(0.0);
        var prev_d = vec3<f32>(0.0);
        var slope = vec3<f32>(0.0);
        for (var j = 0u; j < 3u; j++) {
            if (f[j] >= guides || w[j] <= 0.0) {
                continue;
            }
            let g = f[j] * n;
            now_d += strand_motion[g + k].xyz * w[j];
            prev_d += strand_motion[half + g + k].xyz * w[j];
            let ahead = strand_motion[g + min(k + 1u, n - 1u)].xyz;
            let behind = strand_motion[g + select(k - 1u, 0u, k == 0u)].xyz;
            slope += (ahead - behind) * w[j];
            sum += w[j];
        }
        let inv = 1.0 / max(sum, 1e-4);
        out.model += now_d * inv;
        out.prev_model += prev_d * inv;
        along += slope * inv;
    }
    out.rel = model_point(out.model);
    out.tangent = model_dir(along);
    out.radius = p.w * strand_params.lod.x;
    out.index = i;
    return out;
}

fn ribbon_side(corner: u32) -> f32 {
    var side = array<f32, 6>(-1.0, 1.0, -1.0, -1.0, 1.0, 1.0);
    return side[corner];
}

