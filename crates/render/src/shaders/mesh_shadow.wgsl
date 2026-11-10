// Shadow pass for the standard mesh feature. Requires lib_types (not lib_frame).
//
// Two pipelines use this module:
// - `vs_shadow` (depth only) for opaque casters;
// - `vs_shadow_masked` + `fs_shadow_masked` for alpha-masked casters (foliage cards, fences),
//   which cut out the same pixels as in the scene so their shadows have holes.

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,         // wind velocity xyz
    time: vec4<f32>,         // world seconds, real seconds, delta, w: 1 = sway at rest
    camera_frac: vec4<f32>,  // camera position modulo 4096 (tiling coordinates)
    bend: vec4<f32>,         // x: benders set (none where swaying rests, time.w 1)
    benders: array<vec4<f32>, 16>, // as Frame.benders (camera-relative center xyz, radius)
    plant_wind: vec4<f32>,   // as Frame.plant_wind
    gusts: GustWaves,        // as Frame.gusts
    gust_offset: vec4<f32>,  // as Frame.gust_offset
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
    // w: the 2048s of a bone plant's color alpha (`plant_packed`, lib_wind.wgsl).
    @location(1) normal: vec4<f32>,
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
    // At rest (maps too coarse to show the motion, `shadows::sway_moves`): one pose that
    // doesn't change with time, so a static image drawn again matches the one before.
    let rest = shadow_view.time.w > 0.5;
    return plant_sway(rel.xz + shadow_view.gust_offset.xy, height01, sway, shadow_view.plant_wind, shadow_view.gusts, shadow_view.time.x, rest);
}

// The air of a bone plant's instance in the shadow maps (`PlantAir`, lib_wind.wgsl): branches
// and limbs only (leaf flutter is finer than a shadow texel), and where swaying rests one pose
// bent by the wind without gusts or sway.
fn shadow_plant_air(in: VertexIn, base: vec3<f32>) -> PlantAir {
    let rest = shadow_view.time.w > 0.5;
    var gusts = shadow_view.gusts;
    if (rest) {
        gusts = GustWaves(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    }
    return PlantAir(shadow_view.plant_wind, gusts, vec2<f32>(in.m0.w, in.m2.w) + shadow_view.gust_offset.xy,
        base.xz + shadow_view.gust_offset.xy, false, select(2u, 1u, rest), select(1.0, 0.0, rest));
}

fn shadow_clip(in: VertexIn) -> vec4<f32> {
    let p = vec4<f32>(in.position, 1.0);
    var rel = vec3<f32>(dot(in.m0, p), dot(in.m1, p), dot(in.m2, p));
    let entry = plant_wind_entry(in.extra.y);
    if (entry != 0u) {
        let air = shadow_plant_air(in, rel);
        let dir = plant_wind_dir(in.n0.xyz, in.n1.xyz, in.n2.xyz, air.wind);
        let w = plant_wind(in.position, in.normal.xyz, plant_packed(in.normal.w, in.color.a), entry, dir, air);
        let q = vec4<f32>(w.pos, 1.0);
        rel = vec3<f32>(dot(in.m0, q), dot(in.m1, q), dot(in.m2, q));
        let h01 = saturate((in.position.y - bitcast<f32>(in.extra.z)) / max(bitcast<f32>(in.extra.w), 1e-3));
        var bend = vec3<f32>(0.0);
        for (var i = 0u; i < min(u32(shadow_view.bend.x), 16u); i++) {
            bend += bender_push(rel, shadow_view.benders[i], h01, w.tip);
        }
        var clip = shadow_view.view_proj * vec4<f32>(rel + bender_limit(bend), 1.0);
        clip.z = max(clip.z, 0.0);
        return clip;
    }
    // Wind weight in the vertex color's alpha, as in `vertex_wind` (mesh.wgsl).
    let sway = materials[in.extra.x].params2.y * saturate(in.color.a);
    if (sway > 0.0) {
        let h01 = saturate((in.position.y - bitcast<f32>(in.extra.z)) / max(bitcast<f32>(in.extra.w), 1e-3));
        // Pushed aside by the benders as in the scene (`bender_offset` in lib_frame.wgsl).
        var bend = vec3<f32>(0.0);
        for (var i = 0u; i < min(u32(shadow_view.bend.x), 16u); i++) {
            bend += bender_push(rel, shadow_view.benders[i], h01, in.color.a);
        }
        rel += shadow_wind(rel, h01, sway) + bender_limit(bend);
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
