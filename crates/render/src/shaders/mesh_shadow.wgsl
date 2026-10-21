// Shadow pass for the standard mesh feature. Requires lib_types (not lib_frame).
//
// Two pipelines use this module:
// - `vs_shadow` (depth only) for opaque casters;
// - `vs_shadow_masked` + `fs_shadow_masked` for alpha-masked casters (foliage cards, fences),
//   which cut out the same pixels as in the scene so their shadows have holes.

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,         // wind velocity xyz
    time: vec4<f32>,         // world seconds
    camera_frac: vec4<f32>,  // camera position modulo 4096 (tiling coordinates)
};

// Must match `struct Material` in mesh.wgsl and `GpuMaterial` in material.rs.
struct Material {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    params: vec4<f32>,   // metallic, roughness, reflectance, alpha cutoff
    params2: vec4<f32>,  // subsurface, wind sway, normal scale, occlusion strength
    uv: vec4<f32>,       // uv scale xy, detail strength, detail scale
    mapping: vec4<f32>,  // tiles per meter, parallax depth (m), triplanar sharpness, unused
    flags: vec4<u32>,    // x: bits 0-3 alpha mode, 7 base color texture present; y: bits 0-7
                         // the base color's virtual texture (0 = an ordinary texture)
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;
@group(1) @binding(0) var<storage, read> materials: array<Material>;
// Same layout as the scene pass; only the base color texture and sampler are used here.
@group(2) @binding(0) var base_color_texture: texture_2d<f32>;
@group(2) @binding(6) var material_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) n0: vec4<f32>,
    @location(8) n1: vec4<f32>,
    @location(9) n2: vec4<f32>,
    @location(10) extra: vec4<u32>,
};

fn shadow_wind(rel: vec3<f32>, height01: f32, sway: f32) -> vec3<f32> {
    let w = shadow_view.wind.xyz;
    let speed = length(w);
    let p = rel + shadow_view.camera_frac.xyz;
    let t = shadow_view.time.x;
    let phase = dot(p.xz, vec2(0.13, 0.17));
    let gust = 0.6 + 0.4 * sin(t * 0.7 + phase * 0.3) * sin(t * 1.9 + phase);
    let dir = select(vec3(1.0, 0.0, 0.0), w / max(speed, 1e-3), speed > 0.01);
    let strength = sway * height01 * height01 * min(speed / 10.0, 1.5);
    return dir * strength * gust;
}

fn shadow_clip(in: VertexIn) -> vec4<f32> {
    let p = vec4<f32>(in.position, 1.0);
    var rel = vec3<f32>(dot(in.m0, p), dot(in.m1, p), dot(in.m2, p));
    // Wind weight in the vertex color's alpha, as in `vertex_wind` (mesh.wgsl).
    let sway = materials[in.extra.x].params2.y * saturate(in.color.a);
    if (sway > 0.0) {
        let h01 = saturate((in.position.y - bitcast<f32>(in.extra.z)) / max(bitcast<f32>(in.extra.w), 1e-3));
        rel += shadow_wind(rel, h01, sway);
    }
    var clip = shadow_view.view_proj * vec4<f32>(rel, 1.0);
    // Pancake casters behind the near plane onto it so they still cast shadows.
    clip.z = max(clip.z, 0.0);
    return clip;
}

@vertex
fn vs_shadow(in: VertexIn) -> @builtin(position) vec4<f32> {
    return shadow_clip(in);
}

struct MaskedOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
    @location(2) @interpolate(flat) material: u32,
};

@vertex
fn vs_shadow_masked(in: VertexIn) -> MaskedOut {
    var out: MaskedOut;
    out.clip = shadow_clip(in);
    let mat = materials[in.extra.x];
    out.uv = in.uv * mat.uv.xy * mat.mapping.x;
    // On swaying materials the vertex alpha is the wind weight, not opacity.
    out.alpha = select(in.color.a, 1.0, mat.params2.y > 0.0);
    out.material = in.extra.x;
    return out;
}

@fragment
fn fs_shadow_masked(in: MaskedOut) {
    let mat = materials[in.material];
    let d1 = dpdx(in.uv);
    let d2 = dpdy(in.uv);
    var alpha = mat.base_color.a * in.alpha;
    if ((mat.flags.x & 128u) != 0u) {
        // The base color may be a streamed virtual texture (virtual_texture.wgsl): the shadow
        // uses whatever tiles the camera has made resident.
        let vt = mat.flags.y & 255u;
        if (VT_ENABLED && vt != 0u) {
            alpha *= vt_sample(vt, in.uv, d1, d2).a;
        } else {
            alpha *= textureSampleGrad(base_color_texture, material_sampler, in.uv, d1, d2).a;
        }
    }
    if (alpha < mat.params.w) {
        discard;
    }
}

// The sky albedo map (`RenderFeature::draw_sky_albedo`): the diffuse color of the highest
// surface over each spot, seen from above, for light bounced off the ground (`ground_bounce`
// in lib_frame.wgsl). Drawn over the sky occlusion map's depth with the same bias, so only the
// surfaces that set it pass. One color per material (its base color texture's mean): the map's
// texels are a good part of a meter wide.
struct AlbedoOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) material: u32,
};

@vertex
fn vs_shadow_albedo(in: VertexIn) -> AlbedoOut {
    var out: AlbedoOut;
    out.clip = shadow_clip(in);
    let mat = materials[in.extra.x];
    out.uv = in.uv * mat.uv.xy * mat.mapping.x;
    out.color = vec4<f32>(in.color.rgb, select(in.color.a, 1.0, mat.params2.y > 0.0));
    out.material = in.extra.x;
    return out;
}

@fragment
fn fs_shadow_albedo(in: AlbedoOut) -> @location(0) vec4<f32> {
    let mat = materials[in.material];
    let d1 = dpdx(in.uv);
    let d2 = dpdy(in.uv);
    var base = vec4<f32>(1.0);
    var alpha = mat.base_color.a * in.color.a;
    if ((mat.flags.x & 128u) != 0u) {
        let vt = mat.flags.y & 255u;
        if (VT_ENABLED && vt != 0u) {
            base = vt_mean(vt);
        } else {
            base = textureSampleLevel(base_color_texture, material_sampler, vec2<f32>(0.5), 16.0);
        }
        // Alpha-tested cards cut out the same holes as in the occlusion map.
        if ((mat.flags.x & 15u) == 1u) {
            if (VT_ENABLED && vt != 0u) {
                alpha *= vt_sample(vt, in.uv, d1, d2).a;
            } else {
                alpha *= textureSampleGrad(base_color_texture, material_sampler, in.uv, d1, d2).a;
            }
        }
    }
    if ((mat.flags.x & 15u) == 1u && alpha < mat.params.w) {
        discard;
    }
    // Metals bounce little diffuse light: their color goes into the mirror image instead.
    let albedo = mat.base_color.rgb * in.color.rgb * base.rgb * (1.0 - 0.5 * mat.params.x);
    return vec4<f32>(saturate(albedo), 1.0);
}
