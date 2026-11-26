// Strands into the sun's shadow maps (strands.rs): each strand a ribbon at least a texel wide,
// keeping the share of its texels its true width covers (hashed per texel and strand), so hair
// shades the face and neck under it and itself. Cascades draw a quarter of the strands four
// times as wide (their texels are centimeters across). A groom far away draws few strands,
// each many times wider (the same cover on screen, where a pixel is the limit): its ribbons
// stop at two texels either side, so its shadow stays the coat's shape and not a fan of wide
// strips. Requires lib_types + strands_common.

struct ShadowView {
    view_proj: mat4x4<f32>,
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;
@group(1) @binding(0) var<uniform> params: StrandParams;
@group(1) @binding(1) var<storage, read> instances: array<StrandInst>;
@group(1) @binding(2) var<storage, read> posed: array<vec4<f32>>;

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) seed: u32,
    @location(1) keep: f32,
};

// Half widths in clip units: half a texel of a 2048 map (the narrowest ribbon is one texel
// wide) and two texels (the widest is four).
const MIN_HALF: f32 = 1.0 / 2048.0;
const MAX_HALF: f32 = 4.0 / 2048.0;

fn strand_shadow_vertex(v: u32, g: u32, boost: f32) -> ShadowOut {
    var out: ShadowOut;
    out.clip = vec4<f32>(0.0, 0.0, -2.0, 1.0);
    out.keep = 0.0;
    out.seed = 0u;
    var k = 0u;
    for (; k < params.size.w; k++) {
        let inst = instances[k];
        if (g >= inst.more.x && g < inst.more.x + inst.first.w) {
            break;
        }
    }
    if (k >= params.size.w) {
        return out;
    }
    let inst = instances[k];
    let per = inst.first.z;
    let seg = v / 6u;
    if (seg + 1u >= per) {
        return out;
    }
    let corner = v % 6u;
    let end = select(0u, 1u, corner == 1u || corner == 2u || corner == 4u);
    let side = select(-1.0, 1.0, corner == 2u || corner == 4u || corner == 5u);
    let slot = inst.out.x + (g - inst.more.x) * per + seg;
    let pa = posed[slot];
    let pb = posed[slot + 1u];
    // (A point of no width: a cut hair's missing part.)
    if (pa.w < 0.0 || pb.w < 0.0) {
        return out;
    }
    let ca = shadow_view.view_proj * vec4<f32>(strand_model_point(inst, pa.xyz), 1.0);
    let cb = shadow_view.view_proj * vec4<f32>(strand_model_point(inst, pb.xyz), 1.0);
    let dir = cb.xy - ca.xy;
    let across = normalize(vec2<f32>(-dir.y, dir.x) + vec2<f32>(1e-9, 0.0));
    let scale = length(shadow_view.view_proj[0].xyz);
    // The strands' widths this ribbon stands for: `boost` of them, and the last ribbon what is
    // left of the number drawn (a number with a fraction: strands_common `strand_width`), so the
    // ribbons come in by degrees too and sum to the groom's cover.
    let count = f32(inst.first.w - 1u) + bitcast<f32>(inst.out.z);
    let wide = inst.tint.w * clamp(count - boost * f32(g - inst.more.x), 0.0, boost);
    let r = select(pa.w, pb.w, end == 1u) * wide * scale;
    let half = clamp(r, MIN_HALF, MAX_HALF);
    let c = select(ca, cb, end == 1u);
    out.clip = vec4<f32>(c.xy + across * half * side * c.w, c.z, c.w);
    out.keep = r / half;
    // (Seeded by the strand's place in its groom, which stays: its place in the frame moves
    // whenever a nearer groom draws one strand more, and its groom's place in the shared
    // arrays when they are made again.)
    out.seed = (g - inst.more.x) * 61u + seg;
    return out;
}

@vertex
fn vs_shadow_cascade(@builtin(vertex_index) v: u32, @builtin(instance_index) g: u32) -> ShadowOut {
    return strand_shadow_vertex(v, g, 4.0);
}

@vertex
fn vs_shadow_inset(@builtin(vertex_index) v: u32, @builtin(instance_index) g: u32) -> ShadowOut {
    return strand_shadow_vertex(v, g, 1.0);
}

@fragment
fn fs_shadow(in: ShadowOut) {
    if (hash12(in.clip.xy + vec2<f32>(f32(in.seed % 4093u) * 0.731, f32(in.seed % 3011u) * 0.377)) >= in.keep) {
        discard;
    }
}
