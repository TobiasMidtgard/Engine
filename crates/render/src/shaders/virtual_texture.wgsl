// Streamed virtual textures (crates/render/src/virtual_texture). Composed before mesh.wgsl and
// mesh_shadow.wgsl: group 2 bindings 7..11 of the mesh feature's texture sets, shared by every
// set. Every global here starts with `vt_` (WGSL has no namespaces).
//
// `vt_data` holds a header (frame, mip bias, cache side in texels, slots per row, slot levels),
// one record per texture (size, root level, which cache, page table offsets per level, mean
// color, the texture at 4×4 texels) and the page tables. An entry is the slot (bits 0-15) and level (16-20) of the finest
// resident tile covering that tile, bit 31 set; the root tile is always resident.

const VT_TILE: u32 = 128u;
const VT_BORDER: f32 = 8.0;
const VT_SLOT: f32 = 144.0;
const VT_HEADER_WORDS: u32 = 8u;
const VT_INFO_WORDS: u32 = 56u;
const VT_LOW_WORD: u32 = 24u;
const VT_MAX_ANISO: f32 = 8.0;

// Whether the pipeline reads virtual textures at all (`ShaderFeatures`): off until a scene has
// one, so the pipelines of scenes without any stay small.
override VT_ENABLED: bool = true;

@group(2) @binding(7) var<storage, read> vt_data: array<u32>;
@group(2) @binding(8) var vt_cache_linear: texture_2d<f32>;
@group(2) @binding(9) var vt_cache_srgb: texture_2d<f32>;
@group(2) @binding(10) var vt_sampler: sampler;  // a slot's levels (the root's: the mip tail)
// VT_FEEDBACK_BEGIN
@group(2) @binding(11) var<storage, read_write> vt_feedback: array<atomic<u32>>;

// Record a wanted tile in the feedback hash table (the last writer of a slot wins; a lost
// request is asked again in a later frame).
fn vt_feedback_store(key: u32) {
    var h = key * 0x9E3779B1u;
    h ^= h >> 15u;
    h *= 0x85EBCA77u;
    h ^= h >> 13u;
    atomicStore(&vt_feedback[h & (arrayLength(&vt_feedback) - 1u)], key);
}
// VT_FEEDBACK_END

// Set by the fragment entry points: this pixel records the tiles it samples (one pixel in 16
// per frame, in a rotating pattern).
var<private> vt_feedback_on: bool = false;
// The first virtual texture sampled at this pixel, for `DebugView::VirtualTextures`: level
// shown, level wanted, on a tile edge (0/1), 1 once set.
var<private> vt_debug: vec4<f32> = vec4<f32>(0.0);

// Whether this pixel writes feedback this frame.
fn vt_feedback_pixel(frag: vec2<f32>) -> bool {
    let p = vec2<u32>(frag);
    let cell = (p.x & 3u) | ((p.y & 3u) << 2u);
    // Scramble the order the 16 pixels of a 4x4 block take their turn.
    return cell == ((vt_data[0] * 7u + 3u) & 15u);
}

fn vt_level_size(size: vec2<u32>, level: u32) -> vec2<u32> {
    return max(size >> vec2<u32>(level), vec2<u32>(1u));
}

fn vt_pages(size: vec2<u32>, level: u32) -> vec2<u32> {
    return (vt_level_size(size, level) + vec2<u32>(VT_TILE - 1u)) / VT_TILE;
}

// Level of detail of a footprint with screen derivatives `d1`, `d2` (texture coordinates) on a
// texture of `size` texels, anisotropic up to `VT_MAX_ANISO`: the level whose texels match the
// footprint's minor axis (or its major axis over the anisotropy limit).
fn vt_lod(size: vec2<f32>, d1: vec2<f32>, d2: vec2<f32>) -> f32 {
    let t1 = d1 * size;
    let t2 = d2 * size;
    let l1 = dot(t1, t1);
    let l2 = dot(t2, t2);
    let major = sqrt(max(l1, l2));
    let minor = sqrt(min(l1, l2));
    let ratio = min(major / max(minor, 1e-12), VT_MAX_ANISO);
    return log2(max(major / ratio, 1e-12));
}

// Mean color of virtual texture `id` (decoded, as sampling returns it).
fn vt_mean(id: u32) -> vec4<f32> {
    let base = VT_HEADER_WORDS + (id - 1u) * VT_INFO_WORDS;
    return vec4<f32>(unpack2x16float(vt_data[base + 20u]), unpack2x16float(vt_data[base + 21u]));
}

// Virtual texture `id` at 4×4 texels, bilinear at `uv` (wrapping): the level a whole texture's
// mip chain has a few levels past the root slot's tail.
fn vt_low(id: u32, uv: vec2<f32>) -> vec4<f32> {
    let base = VT_HEADER_WORDS + (id - 1u) * VT_INFO_WORDS + VT_LOW_WORD;
    let t = fract(uv) * 4.0 - 0.5;
    let i = floor(t);
    let f = t - i;
    var c: array<vec4<f32>, 4>;
    for (var k = 0u; k < 4u; k++) {
        let xy = (vec2<i32>(i) + vec2<i32>(i32(k & 1u), i32(k >> 1u)) + 4) & vec2<i32>(3);
        let w = base + u32(xy.y * 4 + xy.x) * 2u;
        c[k] = vec4<f32>(unpack2x16float(vt_data[w]), unpack2x16float(vt_data[w + 1u]));
    }
    return mix(mix(c[0], c[1], f.x), mix(c[2], c[3], f.x), f.y);
}

// Sample virtual texture `id` at texture coordinate `uv` (wrapping, like the material sampler)
// with screen derivatives `d1`, `d2`: trilinear and anisotropic, from the finest resident tile
// at or above the wanted level.
fn vt_sample(id: u32, uv: vec2<f32>, d1: vec2<f32>, d2: vec2<f32>) -> vec4<f32> {
    let base = VT_HEADER_WORDS + (id - 1u) * VT_INFO_WORDS;
    let size = vec2<u32>(vt_data[base], vt_data[base + 1u]);
    let vt_flags = vt_data[base + 2u];
    let root = vt_flags & 31u;
    let bias = bitcast<f32>(vt_data[1]);
    let lod = vt_lod(vec2<f32>(size), d1, d2) + bias;
    let wanted = u32(clamp(floor(lod), 0.0, f32(root)));
    let st = fract(uv);

    // The wanted tile and its entry.
    let pages_w = vt_pages(size, wanted);
    let texel_w = st * vec2<f32>(vt_level_size(size, wanted));
    let page_w = min(vec2<u32>(texel_w) / VT_TILE, pages_w - 1u);
    if (vt_feedback_on) {
        vt_feedback_store(id | (wanted << 8u) | (page_w.x << 12u) | (page_w.y << 22u));
    }
    let entry = vt_data[vt_data[base + 4u + wanted] + page_w.y * pages_w.x + page_w.x];
    let slot = entry & 0xffffu;
    let level = (entry >> 16u) & 31u;

    // Where the resident tile (the wanted one or an ancestor) has this point.
    let k = level - wanted;
    let page = min(page_w >> vec2<u32>(k), vt_pages(size, level) - 1u);
    let level_size = vec2<f32>(vt_level_size(size, level));
    let local = st * level_size - vec2<f32>(page * VT_TILE);
    let per_row = vt_data[3];
    let slot_xy = vec2<f32>(f32(slot % per_row), f32(slot / per_row));
    let cache = bitcast<f32>(vt_data[2]);
    let p = (slot_xy * VT_SLOT + VT_BORDER + local) / cache;
    let scale = level_size / cache * exp2(bias);
    let g1 = d1 * scale;
    let g2 = d2 * scale;

    if (vt_debug.w == 0.0) {
        let px = max(max(length(g1), length(g2)) * cache, 1e-4) * 1.5;
        let edge = any(local < vec2<f32>(px)) || any(local > vec2<f32>(f32(VT_TILE) - px));
        vt_debug = vec4<f32>(f32(level), f32(wanted), select(0.0, 1.0, edge), 1.0);
    }

    var c: vec4<f32>;
    if ((vt_flags & 256u) != 0u) {
        c = textureSampleGrad(vt_cache_srgb, vt_sampler, p, g1, g2);
    } else {
        c = textureSampleGrad(vt_cache_linear, vt_sampler, p, g1, g2);
    }
    // The root's slot holds the mip tail down to 16² (RGBA8; 32² with BC7). Beyond it the
    // record's 4×4 level takes over, then the mean color, as the rest of a full mip chain
    // would.
    let tail_end = f32(root + vt_data[4]) - 1.0;
    if (lod <= tail_end) {
        return c;
    }
    let low_level = floor(log2(f32(max(size.x, size.y)))) - 2.0;
    let low = vt_low(id, uv);
    if (lod < low_level) {
        return mix(c, low, saturate((lod - tail_end) / max(low_level - tail_end, 1.0)));
    }
    return mix(low, vt_mean(id), saturate((lod - low_level) * 0.5));
}

// The debug view's color: the level shown (blue full detail .. white 7+), tile edges darker,
// a red tint where a coarser tile stands in for the wanted one.
fn vt_debug_color() -> vec3<f32> {
    if (vt_debug.w == 0.0) {
        return vec3<f32>(0.35);
    }
    var palette = array<vec3<f32>, 8>(
        vec3<f32>(0.10, 0.30, 1.00),
        vec3<f32>(0.10, 0.80, 0.90),
        vec3<f32>(0.15, 0.85, 0.25),
        vec3<f32>(0.90, 0.90, 0.15),
        vec3<f32>(1.00, 0.55, 0.10),
        vec3<f32>(0.95, 0.15, 0.10),
        vec3<f32>(0.85, 0.20, 0.80),
        vec3<f32>(0.95, 0.95, 0.95),
    );
    var c = palette[min(u32(vt_debug.x), 7u)];
    if (vt_debug.x > vt_debug.y) {
        c = mix(c, vec3<f32>(1.0, 0.1, 0.1), 0.45);
    }
    return c * (1.0 - 0.6 * vt_debug.z);
}
