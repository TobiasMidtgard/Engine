// Sprites and tile maps (sprites/feature.rs): the instance data and quad corners shared by the
// scene and prepass shader (sprite.wgsl, composed with lib_frame) and the shadow shader
// (sprite_shadow.wgsl, without). Requires lib_types.

// Flag bits (`flags` of a sprite instance, of a tile layer or'ed with its tile's; `FLAG_*` in
// feature.rs).
const SPRITE_FILTER_MASK: u32 = 3u;     // 0 linear, 1 nearest, 2 pixel art
const SPRITE_ALPHA_SHIFT: u32 = 2u;     // (flags >> shift) & 3: 0 blend, 1 additive, 2 cutout
const SPRITE_LIT: u32 = 16u;
const SPRITE_NORMAL_MAP: u32 = 32u;
const SPRITE_FLIP_X: u32 = 64u;
const SPRITE_FLIP_Y: u32 = 128u;
const SPRITE_FLIP_DIAGONAL: u32 = 256u; // swap the image's x and y before the flips (Tiled)

// The image (sRGB color + alpha), its normal map (or a flat 1x1 stand-in) and a trilinear,
// clamped sampler. Filtering beyond trilinear (nearest, pixel art) is done in the shader.
@group(1) @binding(0) var sprite_texture: texture_2d<f32>;
@group(1) @binding(1) var sprite_normal_texture: texture_2d<f32>;
@group(1) @binding(2) var sprite_sampler: sampler;

// One sprite: `SpriteInstance` in feature.rs, drawn as a 4-vertex triangle strip.
struct SpriteIn {
    @builtin(vertex_index) corner: u32,
    @location(0) pos: vec3<f32>,      // camera-relative bottom-left corner of the quad
    @location(1) flags: u32,
    @location(2) ax: vec4<f32>,       // quad width vector (m), w: alpha cutoff
    @location(3) ay: vec4<f32>,       // quad height vector (m), w: perceptual roughness
    @location(4) prev_pos: vec4<f32>, // last frame's corner, relative to last frame's camera; w: translucency
    @location(5) prev_ax: vec4<f32>,  // w: emissive (cd/m^2 for white)
    @location(6) prev_ay: vec4<f32>,  // w: roundness
    @location(7) rect: vec4<f32>,     // the frame's texture rectangle: top-left xy, bottom-right zw
    @location(8) color: vec4<f32>,    // linear RGBA tint
};

// A tile map layer (group 2, one per drawn map at a dynamic offset; `GpuTileLayer`).
struct SpriteTileLayer {
    m0: vec4<f32>,      // map space (m, bottom-left corner at 0) -> camera-relative, rows
    m1: vec4<f32>,
    m2: vec4<f32>,
    p0: vec4<f32>,      // the same last frame, relative to last frame's camera
    p1: vec4<f32>,
    p2: vec4<f32>,
    tile: vec4<f32>,    // tile width, height (m), unused, unused
    color: vec4<f32>,   // linear RGBA tint
    params: vec4<f32>,  // alpha cutoff, perceptual roughness, translucency, emissive
    flags: vec4<u32>,   // x: flags (filter, alpha, lit, normal map)
};
@group(2) @binding(0) var<uniform> sprite_tile_layer: SpriteTileLayer;

// One tile: `TileInstance` in feature.rs.
struct SpriteTileIn {
    @builtin(vertex_index) corner: u32,
    @location(0) tile: vec2<u32>,     // x: column | row << 16 (row 0 at the bottom), y: flip flags
    @location(1) rect: vec4<f32>,     // the tile's texture rectangle: top-left xy, bottom-right zw
};

// A decoded quad corner: what the scene, prepass and shadow passes need.
struct SpriteCorner {
    rel: vec3<f32>,
    prev_rel: vec3<f32>,
    uv: vec2<f32>,
    rect: vec4<f32>,
    tangent: vec3<f32>,    // world direction of the image's +x
    bitangent: vec3<f32>,  // world direction of the image's up
    color: vec4<f32>,
    params: vec4<f32>,     // alpha cutoff, roughness, translucency, emissive
    roundness: f32,
    flags: u32,
};

fn sprite_unit(v: vec3<f32>) -> vec3<f32> {
    let len2 = dot(v, v);
    return select(vec3<f32>(0.0), v * inverseSqrt(max(len2, 1e-30)), len2 > 1e-30);
}

// Texture coordinates and image axes of quad corner `q` (0..1, y up) for a frame `rect` with
// the flip flags, and the quad's width and height directions `ax`, `ay` (unit). Tiled's order:
// the diagonal flip (swapping the image's x and y) first, then the horizontal and vertical ones.
fn sprite_map_corner(c: ptr<function, SpriteCorner>, q: vec2<f32>, rect: vec4<f32>, flags: u32, ax: vec3<f32>, ay: vec3<f32>) {
    let fx = (flags & SPRITE_FLIP_X) != 0u;
    let fy = (flags & SPRITE_FLIP_Y) != 0u;
    let diagonal = (flags & SPRITE_FLIP_DIAGONAL) != 0u;
    // Display coordinates (y down), flipped, then swapped.
    var d = vec2<f32>(q.x, 1.0 - q.y);
    if (fx) { d.x = 1.0 - d.x; }
    if (fy) { d.y = 1.0 - d.y; }
    let img = select(d, d.yx, diagonal);
    (*c).uv = rect.xy + (rect.zw - rect.xy) * img;
    (*c).rect = rect;
    let sx = select(1.0, -1.0, fx);
    let sy = select(1.0, -1.0, fy);
    if (diagonal) {
        (*c).tangent = -ay * sy;
        (*c).bitangent = -ax * sx;
    } else {
        (*c).tangent = ax * sx;
        (*c).bitangent = ay * sy;
    }
}

// Corner `i` of a 4-vertex triangle strip: (0,0), (1,0), (0,1), (1,1).
fn sprite_quad_corner(i: u32) -> vec2<f32> {
    return vec2<f32>(f32(i & 1u), f32((i >> 1u) & 1u));
}

fn sprite_corner(in: SpriteIn) -> SpriteCorner {
    let q = sprite_quad_corner(in.corner);
    var c: SpriteCorner;
    c.rel = in.pos + in.ax.xyz * q.x + in.ay.xyz * q.y;
    c.prev_rel = in.prev_pos.xyz + in.prev_ax.xyz * q.x + in.prev_ay.xyz * q.y;
    sprite_map_corner(&c, q, in.rect, in.flags, sprite_unit(in.ax.xyz), sprite_unit(in.ay.xyz));
    c.color = in.color;
    c.params = vec4<f32>(in.ax.w, in.ay.w, in.prev_pos.w, in.prev_ax.w);
    c.roundness = in.prev_ay.w;
    c.flags = in.flags;
    return c;
}

fn sprite_tile_corner(in: SpriteTileIn) -> SpriteCorner {
    let q = sprite_quad_corner(in.corner);
    let layer = sprite_tile_layer;
    let cell = vec2<f32>(f32(in.tile.x & 0xffffu), f32(in.tile.x >> 16u));
    let local = vec4<f32>((cell + q) * layer.tile.xy, 0.0, 1.0);
    var c: SpriteCorner;
    c.rel = vec3<f32>(dot(layer.m0, local), dot(layer.m1, local), dot(layer.m2, local));
    c.prev_rel = vec3<f32>(dot(layer.p0, local), dot(layer.p1, local), dot(layer.p2, local));
    let ax = sprite_unit(vec3<f32>(layer.m0.x, layer.m1.x, layer.m2.x));
    let ay = sprite_unit(vec3<f32>(layer.m0.y, layer.m1.y, layer.m2.y));
    let flags = layer.flags.x | in.tile.y;
    sprite_map_corner(&c, q, in.rect, flags, ax, ay);
    c.color = layer.color;
    c.params = layer.params;
    c.roundness = 0.0;
    c.flags = flags;
    return c;
}

// The texture coordinate to sample for `uv` (screen derivatives `d1`, `d2`) with the filter in
// `flags`, kept half a texel inside `rect` so neighbouring atlas cells never bleed in.
fn sprite_filter_uv(uv: vec2<f32>, d1: vec2<f32>, d2: vec2<f32>, rect: vec4<f32>, size: vec2<f32>, flags: u32) -> vec2<f32> {
    let mode = flags & SPRITE_FILTER_MASK;
    var p = uv;
    if (mode == 1u) {
        // Nearest: the center of the texel under the pixel.
        p = (floor(uv * size) + 0.5) / size;
    } else if (mode == 2u) {
        // Pixel art ("sharp bilinear"): texel centers, except within half a pixel of a texel
        // edge, where the two texels blend over the pixel the edge crosses.
        let pix = uv * size - 0.5;
        let fw = max(abs(d1 * size) + abs(d2 * size), vec2<f32>(1e-5));
        let fr = clamp((fract(pix) - 0.5) / fw + 0.5, vec2<f32>(0.0), vec2<f32>(1.0));
        p = (floor(pix) + fr + 0.5) / size;
    }
    let lo = min(rect.xy, rect.zw);
    let hi = max(rect.xy, rect.zw);
    let half = 0.5 / size;
    let inner_lo = min(lo + half, (lo + hi) * 0.5);
    let inner_hi = max(hi - half, (lo + hi) * 0.5);
    return clamp(p, inner_lo, inner_hi);
}
