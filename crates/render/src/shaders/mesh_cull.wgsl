// Occlusion culling of the instanced meshes the camera draws (mesh_cull.rs), in two phases as
// in Haar and Aaltonen, "GPU-Driven Rendering Pipelines" (SIGGRAPH 2015):
//
// - early (before the depth prepass): each instance is tested against last frame's HZB (the
//   frame's own view-projection, the instance's center moved by how far the camera's origin
//   moved since). Those that may be seen are copied into their batch's part of the culled
//   instance buffer and drawn by the early prepass;
// - late (after it, against the HZB of what it drew): the rest are tested again and those
//   that may be seen are appended after them, drawn by the late prepass. The scene pass draws
//   both.
//
// Nothing that can be seen is left out (what last frame's depth wrongly hid is caught by the
// late test against this frame's), so the picture is the same as without culling.
// Standalone (not composed with lib_frame). Depth is reverse-Z (smaller is farther).

struct Params {
    // Last frame's HZB: its view-projection (camera-relative to that frame's camera).
    early_view_proj: mat4x4<f32>,
    // This frame's view-projection (camera-relative).
    late_view_proj: mat4x4<f32>,
    // xyz: add to a center relative to this frame's camera to get one relative to the camera
    // of last frame's HZB; w: 1 when that HZB is usable.
    early_offset: vec4<f32>,
    // HZB level 0 width and height, its levels, the items.
    hzb: vec4<u32>,
    // Depth buffer width and height, batches, vec4s per instance.
    sizes: vec4<u32>,
};

struct Item {
    // Camera-relative bounding sphere.
    sphere: vec4<f32>,
    // Its batch's draw arguments (`args` index within a phase), its instance (in draw order).
    batch: u32,
    instance: u32,
    pad0: u32,
    pad1: u32,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> items: array<Item>;
@group(0) @binding(2) var<storage, read> source: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read_write> culled: array<vec4<u32>>;
// Draw arguments (5 words each): the early phase's, then the late phase's, then both.
@group(0) @binding(4) var<storage, read_write> args: array<u32>;
@group(0) @binding(5) var<storage, read_write> counts: array<atomic<u32>>;
// Per item: 1 when the early phase drew it.
@group(0) @binding(6) var<storage, read_write> drawn: array<u32>;
@group(0) @binding(7) var hzb: texture_2d<f32>;

const EARLY: u32 = 0u;
const LATE: u32 = 1u;
const BOTH: u32 = 2u;

fn arg(phase: u32, batch: u32, field: u32) -> u32 {
    return (phase * p.sizes.z + batch) * 5u + field;
}

// Whether a sphere (camera-relative to the frame `view_proj` belongs to) is surely hidden: its
// nearest depth lies behind the farthest depth the HZB holds over its screen rectangle. As
// `vg_occluded` in cluster_cull.wgsl.
fn occluded(view_proj: mat4x4<f32>, center: vec3<f32>, radius: f32) -> bool {
    var lo = vec2<f32>(1e30);
    var hi = vec2<f32>(-1e30);
    var nearest = 0.0;
    for (var i = 0u; i < 8u; i++) {
        let corner = center + radius * vec3<f32>(
            select(-1.0, 1.0, (i & 1u) != 0u),
            select(-1.0, 1.0, (i & 2u) != 0u),
            select(-1.0, 1.0, (i & 4u) != 0u),
        );
        let clip = view_proj * vec4<f32>(corner, 1.0);
        // Reaching the camera plane: visible.
        if (clip.w <= 1e-5) { return false; }
        let ndc = clip.xyz / clip.w;
        lo = min(lo, ndc.xy);
        hi = max(hi, ndc.xy);
        nearest = max(nearest, ndc.z);
    }
    // Off screen: the frustum cull kept it for a reason (shadow-only it is not), keep it.
    if (any(hi < vec2<f32>(-1.0)) || any(lo > vec2<f32>(1.0))) { return false; }
    let size = p.sizes.xy;
    let fsize = vec2<f32>(size);
    // Pixels (y down), clamped to the screen.
    let p0 = clamp(vec2<f32>(lo.x, -hi.y) * 0.5 + 0.5, vec2<f32>(0.0), vec2<f32>(1.0)) * fsize;
    let p1 = clamp(vec2<f32>(hi.x, -lo.y) * 0.5 + 0.5, vec2<f32>(0.0), vec2<f32>(1.0)) * fsize;
    let a = min(vec2<u32>(p0), size - 1u);
    let b = min(vec2<u32>(p1), size - 1u);
    // The finest level where the rectangle covers at most 2 x 2 texels. Level `l` texel of a
    // pixel: pixel >> (l + 1), the last texel of a row or column taking the odd ones out.
    let levels = max(p.hzb.z, 1u);
    var level = 0u;
    loop {
        let ta = a >> vec2<u32>(level + 1u);
        let tb = b >> vec2<u32>(level + 1u);
        if ((tb.x - ta.x <= 1u && tb.y - ta.y <= 1u) || level + 1u >= levels) { break; }
        level += 1u;
    }
    let last = max(p.hzb.xy >> vec2<u32>(level), vec2<u32>(1u)) - 1u;
    let ta = min(a >> vec2<u32>(level + 1u), last);
    let tb = min(b >> vec2<u32>(level + 1u), last);
    var farthest = 1.0;
    for (var y = ta.y; y <= tb.y; y++) {
        for (var x = ta.x; x <= tb.x; x++) {
            farthest = min(farthest, textureLoad(hzb, vec2<u32>(x, y), i32(level)).x);
        }
    }
    return nearest < farthest;
}

// Copy the item's instance into its batch's next free place in the culled buffer.
fn emit(item: Item) {
    let k = atomicAdd(&counts[item.batch], 1u);
    let words = p.sizes.w;
    let dst = (args[arg(BOTH, item.batch, 4u)] + k) * words;
    let src = item.instance * words;
    for (var w = 0u; w < words; w++) {
        culled[dst + w] = source[src + w];
    }
}

@compute @workgroup_size(64)
fn cs_early(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.hzb.w) { return; }
    let item = items[id.x];
    var seen = true;
    if (p.early_offset.w > 0.5) {
        seen = !occluded(p.early_view_proj, item.sphere.xyz + p.early_offset.xyz, item.sphere.w);
    }
    drawn[id.x] = select(0u, 1u, seen);
    if (seen) { emit(item); }
}

// Per batch: the early phase draws what it found; the late phase's start after it.
@compute @workgroup_size(64)
fn cs_early_args(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.sizes.z) { return; }
    let n = atomicLoad(&counts[id.x]);
    args[arg(EARLY, id.x, 1u)] = n;
    args[arg(LATE, id.x, 4u)] = args[arg(BOTH, id.x, 4u)] + n;
}

@compute @workgroup_size(64)
fn cs_late(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.hzb.w || drawn[id.x] != 0u) { return; }
    let item = items[id.x];
    if (!occluded(p.late_view_proj, item.sphere.xyz, item.sphere.w)) { emit(item); }
}

// Per batch: the late phase draws what it added; the scene pass draws both.
@compute @workgroup_size(64)
fn cs_late_args(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.sizes.z) { return; }
    let n = atomicLoad(&counts[id.x]);
    args[arg(LATE, id.x, 1u)] = n - args[arg(EARLY, id.x, 1u)];
    args[arg(BOTH, id.x, 1u)] = n;
    // Totals after the batches' counts, read back for the stats: instances and triangles
    // drawn.
    atomicAdd(&counts[p.sizes.z], n);
    atomicAdd(&counts[p.sizes.z + 1u], n * (args[arg(BOTH, id.x, 0u)] / 3u));
}
