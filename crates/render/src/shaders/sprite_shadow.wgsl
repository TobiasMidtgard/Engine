// Sprites and tile maps, sun shadow pass (cut-out sprites cast shadows with their holes).
// Composed as lib_types + sprite_common.wgsl + this file (no lib_frame).

// Matches `ShadowViewUniform` (shadows.rs) and `ShadowView` in mesh_shadow.wgsl.
struct SpriteShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

@group(0) @binding(0) var<uniform> sprite_shadow_view: SpriteShadowView;

struct SpriteShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) alpha_cutoff: vec2<f32>, // tint alpha, cutoff
    @location(3) @interpolate(flat) flags: u32,
};

fn sprite_shadow_out(c: SpriteCorner) -> SpriteShadowOut {
    var out: SpriteShadowOut;
    var clip = sprite_shadow_view.view_proj * vec4<f32>(c.rel, 1.0);
    // Pancake casters behind the near plane onto it so they still cast shadows.
    clip.z = max(clip.z, 0.0);
    out.clip = clip;
    out.uv = c.uv;
    out.rect = c.rect;
    out.alpha_cutoff = vec2<f32>(c.color.a, c.params.x);
    out.flags = c.flags;
    return out;
}

@vertex
fn vs_sprite_shadow(in: SpriteIn) -> SpriteShadowOut {
    return sprite_shadow_out(sprite_corner(in));
}

@vertex
fn vs_tile_shadow(in: SpriteTileIn) -> SpriteShadowOut {
    return sprite_shadow_out(sprite_tile_corner(in));
}

@fragment
fn fs_sprite_shadow(in: SpriteShadowOut) {
    let size = vec2<f32>(textureDimensions(sprite_texture, 0));
    // Crisp art casts crisp shadows: nearest texel unless the sprite is filtered linearly.
    var flags = in.flags;
    if ((flags & SPRITE_FILTER_MASK) == 2u) { flags = (flags & ~SPRITE_FILTER_MASK) | 1u; }
    let uv = sprite_filter_uv(in.uv, vec2<f32>(0.0), vec2<f32>(0.0), in.rect, size, flags);
    let a = textureSampleLevel(sprite_texture, sprite_sampler, uv, 0.0).a * in.alpha_cutoff.x;
    if (a < in.alpha_cutoff.y) {
        discard;
    }
}
