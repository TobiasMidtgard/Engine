// Standard PBR mesh feature. Requires lib_types + lib_frame.

struct Material {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    params: vec4<f32>,   // metallic, roughness, reflectance, alpha cutoff
    params2: vec4<f32>,  // subsurface, wind sway, normal scale, occlusion strength
    uv: vec4<f32>,       // uv scale xy, unused zw
    flags: vec4<u32>,    // x: bits 0-3 alpha mode, 4 unlit, 5 double sided, 6 receives wetness,
                         //    7-11 texture present (base color, normal, metallic/roughness, emissive, occlusion)
};

const TEX_BASE_COLOR: u32 = 128u;
const TEX_NORMAL: u32 = 256u;
const TEX_METALLIC_ROUGHNESS: u32 = 512u;
const TEX_EMISSIVE: u32 = 1024u;
const TEX_OCCLUSION: u32 = 2048u;

@group(1) @binding(0) var<storage, read> materials: array<Material>;

// Per-material textures. Untextured materials bind 1x1 defaults; the TEX_* flags say which
// slots hold real images.
@group(2) @binding(0) var base_color_texture: texture_2d<f32>;
@group(2) @binding(1) var normal_texture: texture_2d<f32>;
@group(2) @binding(2) var metallic_roughness_texture: texture_2d<f32>;
@group(2) @binding(3) var emissive_texture: texture_2d<f32>;
@group(2) @binding(4) var occlusion_texture: texture_2d<f32>;
@group(2) @binding(5) var material_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    // Instance: model matrix rows (camera-relative), normal matrix columns, extra data.
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) n0: vec4<f32>,
    @location(8) n1: vec4<f32>,
    @location(9) n2: vec4<f32>,
    @location(10) extra: vec4<u32>,  // material index, flags, mesh min y (f32 bits), mesh height (f32 bits)
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) @interpolate(flat) material: u32,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let p = vec4<f32>(in.position, 1.0);
    var rel = vec3<f32>(dot(in.m0, p), dot(in.m1, p), dot(in.m2, p));
    let mat = materials[in.extra.x];
    let sway = mat.params2.y;
    if (sway > 0.0) {
        let h01 = saturate((in.position.y - bitcast<f32>(in.extra.z)) / max(bitcast<f32>(in.extra.w), 1e-3));
        rel += wind_offset(rel, h01, sway);
    }
    var out: VertexOut;
    out.rel = rel;
    out.normal = normalize(mat3x3<f32>(in.n0.xyz, in.n1.xyz, in.n2.xyz) * in.normal);
    out.uv = in.uv;
    out.color = in.color;
    out.material = in.extra.x;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    return out;
}

// Normal mapping without precomputed tangents (Schüler, "Normal Mapping Without Precomputed
// Tangents", ShaderX5 / 2013): build the cotangent frame (the surface gradients of u and v)
// from screen-space derivatives of the position and the texture coordinates.
//
// `n` is the interpolated front-face unit normal, `tn` the tangent-space normal (+Y = green
// up, glTF/OpenGL convention). Two sign details:
// - WebGPU window y points down, the opposite of the GL setting the formula was written for;
//   multiplying by the sign of n·(dp1 × dp2) makes the frame independent of that.
// - glTF UVs start at the top-left, so its bitangent points toward decreasing v (-∇v).
fn perturb_normal(n: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, duv1: vec2<f32>, duv2: vec2<f32>, tn: vec3<f32>) -> vec3<f32> {
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let orientation = select(-1.0, 1.0, dot(n, cross(dp1, dp2)) >= 0.0);
    let t = (dp2perp * duv1.x + dp1perp * duv2.x) * orientation;
    let b = (dp2perp * duv1.y + dp1perp * duv2.y) * orientation;
    let len2 = max(dot(t, t), dot(b, b));
    if (len2 <= 1e-24) {
        return n;  // degenerate UVs (all equal): keep the interpolated normal
    }
    let inv = inverseSqrt(len2);
    let perturbed = t * (tn.x * inv) - b * (tn.y * inv) + n * tn.z;
    let len = length(perturbed);
    return select(n, perturbed / len, len > 1e-6);
}

@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let mat = materials[in.material];
    let flags = mat.flags.x;

    // Sample every slot and take derivatives up front (uniform control flow); the flags pick
    // which results are used, so untextured materials shade exactly as without textures.
    let uv = in.uv * mat.uv.xy;
    let base_tex = textureSample(base_color_texture, material_sampler, uv);
    let normal_tex = textureSample(normal_texture, material_sampler, uv);
    let mr_tex = textureSample(metallic_roughness_texture, material_sampler, uv);
    let emissive_tex = textureSample(emissive_texture, material_sampler, uv);
    let occlusion_tex = textureSample(occlusion_texture, material_sampler, uv);
    let dp1 = dpdx(in.rel);
    let dp2 = dpdy(in.rel);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);

    var base = mat.base_color * in.color;
    if ((flags & TEX_BASE_COLOR) != 0u) {
        base *= base_tex;
    }
    if ((flags & 15u) == 1u && base.a < mat.params.w) {
        discard;
    }
    var n = normalize(in.normal);
    if ((flags & TEX_NORMAL) != 0u) {
        let tn = (normal_tex.xyz * 2.0 - 1.0) * vec3<f32>(mat.params2.z, mat.params2.z, 1.0);
        n = perturb_normal(n, dp1, dp2, duv1, duv2, tn);
    }
    // Back faces of double-sided materials see the mirrored (perturbed) normal.
    if (!front) { n = -n; }

    var s = default_surface();
    s.albedo = base.rgb;
    s.metallic = mat.params.x;
    s.roughness = mat.params.y;
    if ((flags & TEX_METALLIC_ROUGHNESS) != 0u) {
        s.metallic = saturate(s.metallic * mr_tex.b);
        s.roughness = clamp(s.roughness * mr_tex.g, 0.045, 1.0);
    }
    s.reflectance = mat.params.z;
    s.normal = n;
    s.emissive = mat.emissive.rgb;
    if ((flags & TEX_EMISSIVE) != 0u) {
        s.emissive *= emissive_tex.rgb;
    }
    if ((flags & TEX_OCCLUSION) != 0u) {
        s.occlusion = mix(1.0, occlusion_tex.r, mat.params2.w);
    }
    s.subsurface = mat.params2.x;

    // Weather: rain darkens porous surfaces and makes them glossy; snow settles on top faces.
    if ((flags & 64u) != 0u) {
        let wet = frame.weather.x * saturate(n.y * 0.7 + 0.5);
        s.albedo *= mix(1.0, 0.55, wet * (1.0 - s.metallic));
        s.roughness = mix(s.roughness, 0.12, wet * 0.85);
        let snow_cover = saturate(frame.weather.y * 10.0) * smoothstep(0.35, 0.8, n.y);
        let snow_noise = value_noise2(tiling_pos(in.rel).xz * 1.7);
        let snow = saturate(snow_cover * (0.75 + 0.5 * snow_noise));
        s.albedo = mix(s.albedo, vec3<f32>(0.9, 0.92, 0.95), snow);
        s.roughness = mix(s.roughness, 0.75, snow);
        s.subsurface = mix(s.subsurface, 0.3, snow);
    }

    // Debug views (RenderSettings.debug_view).
    if (frame.counts.z == 1u) {
        return vec4<f32>(n * 0.5 + 0.5, 1.0);
    }
    if (frame.counts.z == 3u) {
        return vec4<f32>(vec3(sky_visibility(in.rel, n)), 1.0);
    }
    var color: vec3<f32>;
    if ((flags & 16u) != 0u) {
        color = s.albedo + pre_expose(s.emissive);
    } else {
        color = shade_surface(s, in.rel);
    }
    color = apply_fog(color, in.rel);
    if (frame.counts.z == 2u) {
        let depth = view_depth(in.rel);
        var tint = vec3<f32>(1.0, 0.4, 0.4);
        if (depth > frame.shadow_splits[0]) { tint = vec3(0.4, 1.0, 0.4); }
        if (depth > frame.shadow_splits[1]) { tint = vec3(0.4, 0.4, 1.0); }
        if (depth > frame.shadow_splits[2]) { tint = vec3(1.0, 1.0, 0.4); }
        if (depth > frame.shadow_splits[3]) { tint = vec3(1.0); }
        color *= tint;
    }
    return vec4<f32>(color, base.a);
}
