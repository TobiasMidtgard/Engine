// Hair strand data and ribbon expansion, shared by the color, prepass and shadow passes.
// Requires lib_types + common.wgsl.
//
// Every hair is a polyline of `points_per` points; instance i draws hair i as a ribbon of two
// triangles per segment.

struct StrandParams {
    info: vec4<u32>,   // points per hair, kind (0 scalp, 1 brows, 2 lashes, 3 beard), hairs, guides
    // Detail by distance: hair width scale (fewer hairs drawn, each wider), points stepped
    // over per segment (1 every point), how far the set has faded in (0..1; hairs hand over to
    // their locks), and 1 for a set of locks (locks.rs).
    lod: vec4<f32>,
    // The box the points are packed in: corner (w: the largest radius) and size.
    box_min: vec4<f32>,
    box_size: vec4<f32>,
};

@group(2) @binding(0) var<uniform> strand_params: StrandParams;
@group(2) @binding(1) var<storage, read> strand_points: array<vec2<u32>>;  // xy, z radius (unorm16, strand_point)
@group(2) @binding(2) var<storage, read> strand_attrs: array<u32>;   // occlusion, random (unorm16 each)
// Per hair: the skin vertex it grows from (its expressions move the hair), that vertex's four
// bones and their weights (bytes), the guide it moves with (0xffffffff: none).
@group(2) @binding(3) var<storage, read> strand_roots: array<vec4<u32>>;
// Hair that moves (`info.w` guides): how far each guide point moved from where the pose alone
// carries it (posed model space), this frame's for all guides, then last frame's.
@group(2) @binding(4) var<storage, read> strand_motion: array<vec4<f32>>;
// Per hair that moves: the three guides it moves with and their weights (bytes, 0..255).
@group(2) @binding(5) var<storage, read> strand_follow: array<vec4<u32>>;
// Locks only, per point: the cross-section's major axis (octahedral, snorm16 x2), then its
// minor half width over the major and the hairs' summed radii over the major half width (half
// floats). The point's radius is the major half width.
@group(2) @binding(6) var<storage, read> strand_shape: array<vec2<u32>>;

struct StrandOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) tangent: vec3<f32>,
    @location(2) attrs: vec2<f32>,  // occlusion, random
    @location(3) coverage: f32,
    @location(4) @interpolate(flat) hair: u32,
    @location(5) clip_now: vec4<f32>,
    @location(6) clip_prev: vec4<f32>,
    // Where the point sits in the rest model space the hair shadow grid is in, and that space's
    // axes in world space (they turn with the head).
    @location(7) rest: vec3<f32>,
    @location(8) axis_x: vec3<f32>,
    @location(9) axis_y: vec3<f32>,
    @location(10) axis_z: vec3<f32>,
    // Locks: across the ribbon (-1..1) and the optical depth of its hairs through its middle
    // (0 for hairs, whose share is `coverage`).
    @location(11) lock: vec2<f32>,
};

struct RibbonPoint {
    model: vec3<f32>,
    // Where the point was last frame (posed model space).
    prev_model: vec3<f32>,
    rel: vec3<f32>,
    tangent: vec3<f32>,
    radius: f32,
    index: u32,
    rest: vec3<f32>,
    axes: mat3x3<f32>,
    // Locks: the cross-section's major axis (camera-relative world), minor over major half
    // width, and the hairs' summed radii over the major half width.
    lock_axis: vec3<f32>,
    lock_minor: f32,
    lock_density: f32,
};

// Point i: model-space position (m) and radius (m).
fn strand_point(i: u32) -> vec4<f32> {
    let q = strand_points[i];
    let xy = unpack2x16unorm(q.x);
    let zr = unpack2x16unorm(q.y);
    let at = strand_params.box_min.xyz + vec3<f32>(xy, zr.x) * strand_params.box_size.xyz;
    return vec4<f32>(at, zr.y * strand_params.box_min.w);
}

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
    let p = strand_point(i);
    let prev = strand_point(base + select(k - 1u, 0u, k == 0u)).xyz;
    let next = strand_point(base + min(k + 1u, n - 1u)).xyz;
    // The hair rides its root: the root's expressions, then its bones.
    let root = strand_roots[hair];
    let m = morph_at(root.x);
    let bones = unpack_bytes(root.y);
    let weights = vec4<f32>(unpack_bytes(root.z)) / 255.0;
    let now = skin_matrix(bones, weights, false);
    var out: RibbonPoint;
    out.model = (now * vec4<f32>(p.xyz + m.dp, 1.0)).xyz;
    out.rest = p.xyz + m.dp;
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
        // (Near enough: the swing is small next to the grid's reach.)
        out.rest += now_d * inv;
        out.prev_model += prev_d * inv;
        along += slope * inv;
    }
    out.rel = model_point(out.model);
    out.tangent = model_dir(along);
    out.radius = p.w * strand_params.lod.x;
    out.index = i;
    out.lock_axis = vec3<f32>(0.0);
    out.lock_minor = 1.0;
    out.lock_density = 0.0;
    if (is_locks()) {
        let shape = strand_shape[i];
        let axis = oct_decode(unpack2x16snorm(shape.x));
        let ratios = unpack2x16float(shape.y);
        out.lock_axis = model_dir((now * vec4<f32>(axis, 0.0)).xyz);
        out.lock_minor = ratios.x;
        out.lock_density = ratios.y;
    }
    out.axes = mat3x3<f32>(
        normalize(model_dir((now * vec4<f32>(1.0, 0.0, 0.0, 0.0)).xyz)),
        normalize(model_dir((now * vec4<f32>(0.0, 1.0, 0.0, 0.0)).xyz)),
        normalize(model_dir((now * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xyz)),
    );
    return out;
}

fn is_locks() -> bool {
    return strand_params.lod.w > 0.5;
}

// Half the ribbon's true width across `side` (unit, square to the hair), and for a lock the
// optical depth of its hairs through its middle seen that way: a lock's elliptical
// cross-section shows wide face-on and narrow edge-on, and as many hairs lie across either
// (summed radii over half the width seen).
fn ribbon_width(rp: RibbonPoint, side: vec3<f32>) -> vec2<f32> {
    if (!is_locks()) {
        return vec2<f32>(rp.radius, 0.0);
    }
    let t = normalize(rp.tangent);
    let u = normalize(rp.lock_axis - t * dot(rp.lock_axis, t) + vec3<f32>(1e-6, 0.0, 0.0));
    let v = cross(t, u);
    let h = max(length(vec2<f32>(rp.radius * dot(u, side), rp.radius * rp.lock_minor * dot(v, side))), 1e-6);
    return vec2<f32>(h, rp.lock_density * rp.radius / h);
}

// The ribbon's `lock` varying: across it, and the optical depth when drawn `half_width` wide
// (wider than true, the same hairs spread thinner).
fn lock_varying(width: vec2<f32>, half_width: f32, corner: u32) -> vec2<f32> {
    return vec2<f32>(ribbon_side(corner), width.y * width.x / half_width);
}

// The share of a pixel a hair or lock covers: a hair's `coverage`; a lock's from its hairs'
// optical depth, densest in its middle (hairs spread evenly over an ellipse cross a chord as
// long as its height there: 4/π √(1-s²) of the mean), faded in by `lod.z`.
fn lock_coverage(lock: vec2<f32>, coverage: f32) -> f32 {
    if (lock.y <= 0.0) {
        return coverage;
    }
    let profile = 1.2732395 * sqrt(max(1.0 - lock.x * lock.x, 0.0));
    return (1.0 - exp(-lock.y * profile)) * strand_params.lod.z;
}

fn ribbon_side(corner: u32) -> f32 {
    var side = array<f32, 6>(-1.0, 1.0, -1.0, -1.0, 1.0, 1.0);
    return side[corner];
}

