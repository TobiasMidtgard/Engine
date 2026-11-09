// The strand renderer's shared types (strands.rs, characters week R2). Requires lib_types.
//
// Every drawn strand point has one slot in the frame's point buffers: instance `k`'s points
// start at `inst.out.x`, strand `s` point `j` at `out.x + s * points_per + j`. The slots stay
// put while the set of instances does (strands drawn or not), so last frame's positions
// (`strand_history`) are found where this frame's go.

struct StrandParams {
    size: vec4<u32>,   // tiles across, tiles down, segment reference capacity, instances
    misc: vec4<u32>,   // tile count, 1 when last frame's positions are no history, frame index, voxels per side
    tiles: vec4<u32>,  // offsets in `strand_tiles`: starts, fill cursors, block sums, active list
    screen: vec4<f32>, // width, height (px), the fibre opacity of hair, of fur
};

struct StrandInst {
    rows: array<vec4<f32>, 3>, // model -> camera-relative (3x4, by rows)
    inv: array<vec4<f32>, 3>,  // camera-relative direction -> model direction (rows)
    grid: vec4<f32>,           // the density volume's corner (model space), w: its cell (m)
    tint: vec4<f32>,           // color multiplier, w: width multiplier
    first: vec4<u32>,          // groom's first rest point, first strand, points per strand, strands drawn
    out: vec4<u32>,            // first point slot, guide offsets (u32::MAX: none), groom's first guide point, first voxel
    more: vec4<u32>,           // first drawn strand (over all instances), fibre (0 hair, 1 fur), strands in the groom, 1: a thin layer
    sphere: vec4<f32>,         // camera-relative bounds: center, radius
};

// Per groom strand: root color (rgb) and random, tip color and cuticle roughness as f16 pairs;
// its guides (u32::MAX: none) and the first two weights (unorm16; the third is the rest).
struct StrandInfo {
    color: vec4<u32>,
    follow: vec4<u32>,
};

// A shaded point, as the binning and the raster read it.
struct StrandLit {
    // Pixel position (top-left origin), depth (reverse-Z NDC), half width (px; below 0: not
    // drawn: behind the camera).
    pos: vec4<f32>,
    // rg, (b, opacity) as f16 pairs (pre-exposed, fogged), the motion since last frame (uv),
    // flags (STRAND_LAST: no segment starts here; STRAND_ROOT: the strand's first segment).
    shade: vec4<u32>,
};

const STRAND_LAST: u32 = 1u;
const STRAND_ROOT: u32 = 2u;
// Density volumes hold the fibres' projected area (diameter times length, m^2) in units of
// 1e-10 m^2.
const STRAND_AREA_UNIT: f32 = 1e10;
// Pixels per tile side.
const STRAND_TILE: u32 = 8u;

fn strand_model_point(inst: StrandInst, m: vec3<f32>) -> vec3<f32> {
    let q = vec4<f32>(m, 1.0);
    return vec3<f32>(dot(inst.rows[0], q), dot(inst.rows[1], q), dot(inst.rows[2], q));
}

fn strand_model_dir(inst: StrandInst, d: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(inst.rows[0].xyz, d), dot(inst.rows[1].xyz, d), dot(inst.rows[2].xyz, d));
}

fn strand_to_model_dir(inst: StrandInst, d: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(inst.inv[0].xyz, d), dot(inst.inv[1].xyz, d), dot(inst.inv[2].xyz, d));
}
