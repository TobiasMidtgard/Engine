// Impostors in the shadow maps: a quad facing the light, cut out by the coverage of the tile
// nearest to the light's direction. Appended to impostor_common.wgsl. Requires lib_types.

struct ShadowView {
    view_proj: mat4x4<f32>,
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;
@group(3) @binding(0) var impostor_albedo: texture_2d_array<f32>;
@group(3) @binding(1) var impostor_normal: texture_2d_array<f32>;
@group(3) @binding(2) var impostor_sampler: sampler;
@group(3) @binding(3) var<storage, read> impostors: array<ImpostorInfo>;

struct ShadowIn {
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(10) extra: vec4<u32>,
};

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) plane: vec3<f32>,
    @location(1) @interpolate(flat) toward: vec3<f32>,
    @location(2) @interpolate(flat) layer: u32,
    // Camera-relative position on the quad, and the light's direction scaled to mesh units.
    @location(3) rel: vec3<f32>,
    @location(4) @interpolate(flat) to_light: vec3<f32>,
};

fn shadow_rows_unapply(m0: vec4<f32>, m1: vec4<f32>, m2: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    let t = m0.xyz * v.x + m1.xyz * v.y + m2.xyz * v.z;
    let s2 = dot(vec3<f32>(m0.x, m1.x, m2.x), vec3<f32>(m0.x, m1.x, m2.x));
    return t / max(s2, 1e-12);
}

@vertex
fn vs_impostor_shadow(in: ShadowIn, @builtin(vertex_index) v: u32) -> ShadowOut {
    let sphere = impostors[in.extra.y].center_radius;
    let center = vec3<f32>(
        dot(in.m0, vec4<f32>(sphere.xyz, 1.0)),
        dot(in.m1, vec4<f32>(sphere.xyz, 1.0)),
        dot(in.m2, vec4<f32>(sphere.xyz, 1.0)),
    );
    let radius = sphere.w * length(vec3<f32>(in.m0.x, in.m1.x, in.m2.x));
    // The light's axes: rows of its orthographic view-projection (depth grows away from it).
    let vp = shadow_view.view_proj;
    let right = normalize(vec3<f32>(vp[0][0], vp[1][0], vp[2][0]));
    let up = normalize(vec3<f32>(vp[0][1], vp[1][1], vp[2][1]));
    let to_light = -normalize(vec3<f32>(vp[0][2], vp[1][2], vp[2][2]));
    let corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    let c = corners[v];
    let offset = (right * c.x + up * c.y) * radius;
    var out: ShadowOut;
    out.clip = vp * vec4<f32>(center + offset, 1.0);
    out.plane = shadow_rows_unapply(in.m0, in.m1, in.m2, offset);
    out.toward = normalize(shadow_rows_unapply(in.m0, in.m1, in.m2, to_light));
    out.layer = in.extra.y;
    out.rel = center + offset;
    out.to_light = to_light * (radius / sphere.w);
    return out;
}

// The baked depth puts each texel where the mesh's surface is, so the crown shadows itself
// and the ground as the mesh would.
@fragment
fn fs_impostor_shadow(in: ShadowOut) -> @builtin(frag_depth) f32 {
    let radius = impostors[in.layer].center_radius.w;
    let tile = impostor_pick_tile(in.toward, vec2<f32>(0.5));
    let t = impostor_texel(tile, in.plane, in.toward, radius, 0.0);
    let coverage = textureSampleLevel(impostor_albedo, impostor_sampler, t.uv, in.layer, 0.0).a;
    if (!t.inside || coverage < 0.5) {
        discard;
    }
    let depth = textureSampleLevel(impostor_normal, impostor_sampler, t.uv, in.layer, 0.0).a / coverage;
    // Toward the light by the surface's height above the quad, less a margin for the baked
    // depth's 8-bit steps (no hardware slope bias applies to a written depth).
    let along = impostor_height(depth, radius) - 0.03 * radius;
    let clip = shadow_view.view_proj * vec4<f32>(in.rel + in.to_light * along, 1.0);
    return clip.z;
}
