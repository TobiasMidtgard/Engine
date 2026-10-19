// Sprites and tile maps, scene pass and depth prepass. Composed as lib_types + lib_frame +
// sprite_common.wgsl + this file.

// `@invariant` keeps the prepass and the scene pass depths bit-identical.
struct SpriteVertexOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) rect: vec4<f32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) params: vec4<f32>,   // alpha cutoff, roughness, translucency, emissive
    @location(5) @interpolate(flat) tangent: vec4<f32>,  // image +x (world), w: roundness
    @location(6) @interpolate(flat) bitangent: vec3<f32>, // image up (world)
    @location(7) @interpolate(flat) flags: u32,
    @location(8) motion_now: vec4<f32>,
    @location(9) motion_prev: vec4<f32>,
};

fn sprite_vertex_out(c: SpriteCorner) -> SpriteVertexOut {
    var out: SpriteVertexOut;
    out.clip = frame.view_proj * vec4<f32>(c.rel, 1.0);
    out.rel = c.rel;
    out.uv = c.uv;
    out.rect = c.rect;
    out.color = c.color;
    out.params = c.params;
    out.tangent = vec4<f32>(c.tangent, c.roundness);
    out.bitangent = c.bitangent;
    out.flags = c.flags;
    out.motion_now = motion_clip(c.rel);
    out.motion_prev = prev_motion_clip(c.prev_rel);
    return out;
}

@vertex
fn vs_sprite(in: SpriteIn) -> SpriteVertexOut {
    return sprite_vertex_out(sprite_corner(in));
}

@vertex
fn vs_tile(in: SpriteTileIn) -> SpriteVertexOut {
    return sprite_vertex_out(sprite_tile_corner(in));
}

// What a pixel of a sprite shows before lighting.
struct SpritePixel {
    base: vec4<f32>,     // linear color and alpha, tint included
    normal: vec3<f32>,   // shading normal (world, toward the camera's side)
    face: vec3<f32>,     // the quad's own normal, toward the camera
};

// Sample the image (and normal map) at this pixel and build the shading normal. Screen
// derivatives are taken first, in uniform control flow.
fn sprite_pixel(in: SpriteVertexOut) -> SpritePixel {
    let d1 = dpdx(in.uv);
    let d2 = dpdy(in.uv);
    let dp1 = dpdx(in.rel);
    let dp2 = dpdy(in.rel);
    let size = vec2<f32>(textureDimensions(sprite_texture, 0));
    let uv = sprite_filter_uv(in.uv, d1, d2, in.rect, size, in.flags);
    var out: SpritePixel;
    out.base = textureSampleGrad(sprite_texture, sprite_sampler, uv, d1, d2) * in.color;

    // The quad faces the camera on the side in view.
    let v = normalize(-in.rel);
    let g = cross(dp1, dp2);
    var face = select(sprite_unit(cross(in.tangent.xyz, in.bitangent)), normalize(g), dot(g, g) > 1e-30);
    if (dot(face, v) < 0.0) { face = -face; }
    out.face = face;
    var t = in.tangent.xyz;
    var b = in.bitangent;
    var n = face;
    // Roundness: bend the normal outward toward the frame's edges, like a rounded volume.
    let roundness = in.tangent.w;
    if (roundness > 0.0) {
        let span = in.rect.zw - in.rect.xy;
        let f = (in.uv - in.rect.xy) / select(span, vec2<f32>(1.0), abs(span) < vec2<f32>(1e-12));
        let p = vec2<f32>(f.x * 2.0 - 1.0, 1.0 - f.y * 2.0);
        n = normalize(face + (t * p.x + b * p.y) * roundness * 1.4);
    }
    if ((in.flags & SPRITE_NORMAL_MAP) != 0u) {
        let tn = textureSampleGrad(sprite_normal_texture, sprite_sampler, uv, d1, d2).xyz * 2.0 - 1.0;
        // Tangent frame around the (bent) normal.
        t = sprite_unit(t - n * dot(n, t));
        b = sprite_unit(b - n * dot(n, b) - t * dot(t, b));
        let mapped = t * tn.x + b * tn.y + n * max(tn.z, 0.0);
        n = select(n, normalize(mapped), dot(mapped, mapped) > 1e-12);
    }
    out.normal = n;
    return out;
}

@fragment
fn fs_sprite(in: SpriteVertexOut) -> @location(0) vec4<f32> {
    let px = sprite_pixel(in);
    let mode = (in.flags >> SPRITE_ALPHA_SHIFT) & 3u;
    if (mode == 2u && px.base.a < in.params.x) {
        discard;
    }
    var color = px.base.rgb;
    if ((in.flags & SPRITE_LIT) != 0u) {
        // A thin sheet (paper, cloth, a leaf card): lit on the side in view, light from behind
        // shines through by `translucency`.
        var s = default_surface();
        s.albedo = px.base.rgb;
        s.metallic = 0.0;
        s.roughness = clamp(in.params.y, 0.045, 1.0);
        s.reflectance = 0.5;
        s.normal = px.normal;
        s.smooth_normal = px.face;
        s.emissive = px.base.rgb * in.params.w;
        s.occlusion = 1.0;
        s.subsurface = saturate(in.params.z);
        color = shade_thin_surface(s, in.rel, 1.0, px.face);
    }
    color = apply_fog(color, in.rel);
    if (mode == 2u) {
        return vec4<f32>(color, 1.0);
    }
    // Premultiplied: blended sprites cover what is behind by alpha, additive ones (alpha 0
    // written) add to it.
    let a = saturate(px.base.a);
    return vec4<f32>(color * a, select(a, 0.0, mode == 1u));
}

// Depth prepass (cut-out sprites): the same pixels as `fs_sprite` keeps.
@fragment
fn fs_sprite_prepass(in: SpriteVertexOut) -> PrepassOutput {
    let px = sprite_pixel(in);
    if (px.base.a < in.params.x) {
        discard;
    }
    let roughness = select(1.0, clamp(in.params.y, 0.045, 1.0), (in.flags & SPRITE_LIT) != 0u);
    return prepass_output(px.normal, roughness, in.motion_now, in.motion_prev);
}
