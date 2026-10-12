// Standard PBR mesh feature. Requires lib_types + lib_frame.

struct Material {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    params: vec4<f32>,   // metallic, roughness, reflectance, alpha cutoff
    params2: vec4<f32>,  // subsurface, wind sway, normal scale, occlusion strength
    uv: vec4<f32>,       // uv scale xy, detail strength, detail scale (m)
    flags: vec4<u32>,    // x: bits 0-3 alpha mode, 4 unlit, 5 double sided, 6 receives wetness,
                         //    7-11 texture present (base color, normal, metallic/roughness, emissive, occlusion),
                         //    12 detail anchored to the world
};

const TEX_BASE_COLOR: u32 = 128u;
const TEX_NORMAL: u32 = 256u;
const TEX_METALLIC_ROUGHNESS: u32 = 512u;
const TEX_EMISSIVE: u32 = 1024u;
const TEX_OCCLUSION: u32 = 2048u;
const DETAIL_IN_WORLD: u32 = 4096u;

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
    // Last frame's model matrix rows, relative to last frame's camera (motion vectors).
    @location(11) p0: vec4<f32>,
    @location(12) p1: vec4<f32>,
    @location(13) p2: vec4<f32>,
};

// `@invariant` keeps the prepass and the main pass depths bit-identical.
struct VertexOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) @interpolate(flat) material: u32,
    @location(5) local: vec3<f32>,
};

// Wind sway of a vertex (meters, camera-relative).
fn vertex_wind(in: VertexIn, rel: vec3<f32>) -> vec3<f32> {
    let sway = materials[in.extra.x].params2.y;
    if (sway <= 0.0) { return vec3<f32>(0.0); }
    let h01 = saturate((in.position.y - bitcast<f32>(in.extra.z)) / max(bitcast<f32>(in.extra.w), 1e-3));
    return wind_offset(rel, h01, sway);
}

// Object-space position in meters (the model's scale applied, not its rotation or offset), so
// surface detail sticks to moving objects at a consistent size.
fn vertex_local(in: VertexIn) -> vec3<f32> {
    let scale = vec3<f32>(
        length(vec3<f32>(in.m0.x, in.m1.x, in.m2.x)),
        length(vec3<f32>(in.m0.y, in.m1.y, in.m2.y)),
        length(vec3<f32>(in.m0.z, in.m1.z, in.m2.z)),
    );
    return in.position * scale;
}

fn vertex_rel(in: VertexIn) -> vec3<f32> {
    let p = vec4<f32>(in.position, 1.0);
    let rel = vec3<f32>(dot(in.m0, p), dot(in.m1, p), dot(in.m2, p));
    return rel + vertex_wind(in, rel);
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    return standard_vertex(in);
}

// The scene pass's vertex math, shared with the virtual geometry path (cluster.wgsl).
fn standard_vertex(in: VertexIn) -> VertexOut {
    let rel = vertex_rel(in);
    var out: VertexOut;
    out.rel = rel;
    out.normal = normalize(mat3x3<f32>(in.n0.xyz, in.n1.xyz, in.n2.xyz) * in.normal);
    out.uv = in.uv;
    out.color = in.color;
    out.material = in.extra.x;
    out.local = vertex_local(in);
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    return out;
}

// ---------------------------------------------------------------- surface detail
// `StandardMaterial::detail`: value-noise fbm in object (or world) space that bumps the normal
// and mottles color and roughness. Octaves finer than a few pixels fade out, so distant
// surfaces stay calm instead of sparkling.

// Detail position: object space, or world space (wrapping every 4096 m) for terrain.
fn detail_pos(mat: Material, local: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    if ((mat.flags.x & DETAIL_IN_WORLD) != 0u) { return tiling_pos(rel); }
    return local;
}

// A rotation (Quilez's octave rotation for fbm).
const DETAIL_TURN: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(0.00, 0.80, 0.60),
    vec3<f32>(-0.80, 0.36, -0.48),
    vec3<f32>(-0.60, -0.48, 0.64),
);

// Bump height in [-1, 1] at `p` (meters) for bumps `scale` meters across; `footprint` is the
// size of a pixel there (meters).
fn detail_height(p: vec3<f32>, scale: f32, footprint: f32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var wavelength = scale;
    var q = p / scale;
    for (var i = 0; i < 5; i++) {
        let fade = saturate(wavelength / (4.0 * footprint) - 0.5);
        if (fade <= 0.0) { break; }
        sum += amp * fade * (value_noise3(q) * 2.0 - 1.0);
        // Turn every octave so the noise lattices don't line up into streaks.
        q = DETAIL_TURN * q * 2.03 + vec3<f32>(17.1, 9.2, 3.7);
        amp *= 0.5;
        wavelength *= 0.4926;
    }
    return sum * 1.2;
}

// Normal bumped by a height field from its differences one pixel right (`dh1`) and down
// (`dh2`), in meters (Mikkelsen 2010, "Bump Mapping Unparametrized Surfaces on the GPU").
fn bump_normal(n: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, dh1: f32, dh2: f32) -> vec3<f32> {
    let r1 = cross(dp2, n);
    let r2 = cross(n, dp1);
    let det = dot(dp1, r1);
    if (abs(det) < 1e-20) { return n; }
    let grad = sign(det) * (dh1 * r1 + dh2 * r2);
    let b = abs(det) * n - grad;
    let len = length(b);
    return select(n, b / len, len > 1e-20);
}

struct Detail {
    normal: vec3<f32>,
    height: f32,  // [-1, 1] at this pixel (0 without detail)
};

// Apply the material's surface detail to normal `n`. `dp1`/`dp2` are the screen derivatives
// of the camera-relative position, `dq1`/`dq2` those of the detail position.
fn surface_detail(mat: Material, n: vec3<f32>, p: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, dq1: vec3<f32>, dq2: vec3<f32>) -> Detail {
    var d: Detail;
    d.normal = n;
    d.height = 0.0;
    let strength = mat.uv.z;
    if (strength <= 0.0) { return d; }
    let scale = mat.uv.w;
    let footprint = max(max(length(dq1), length(dq2)), 1e-5);
    let h0 = detail_height(p, scale, footprint);
    let h1 = detail_height(p + dq1, scale, footprint);
    let h2 = detail_height(p + dq2, scale, footprint);
    let amplitude = 0.06 * scale * strength;
    d.normal = bump_normal(n, dp1, dp2, (h1 - h0) * amplitude, (h2 - h0) * amplitude);
    d.height = h0;
    return d;
}

// Mottled color: broad light/dark patches plus a faint warm/cool shift, and darker hollows.
fn detail_albedo(mat: Material, albedo: vec3<f32>, p: vec3<f32>, height: f32) -> vec3<f32> {
    let strength = mat.uv.z;
    if (strength <= 0.0) { return albedo; }
    let q = p / (8.0 * mat.uv.w);
    let patches = fbm3(q + vec3<f32>(5.2, 1.3, 7.7), 3) * 2.0 - 1.0;
    let hue = (value_noise3(q * 0.7 + vec3<f32>(11.0, 3.0, 5.0)) * 2.0 - 1.0) * 0.08 * strength;
    let light = 1.0 + strength * (0.35 * patches + 0.2 * height);
    return albedo * max(light, 0.0) * vec3<f32>(1.0 + hue, 1.0, 1.0 - hue);
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
    let dpos = detail_pos(mat, in.local, in.rel);
    let dq1 = dpdx(dpos);
    let dq2 = dpdy(dpos);

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
    let detail = surface_detail(mat, n, dpos, dp1, dp2, dq1, dq2);
    n = detail.normal;
    // Back faces of double-sided materials see the mirrored (perturbed) normal.
    if (!front) { n = -n; }

    var s = default_surface();
    s.albedo = detail_albedo(mat, base.rgb, dpos, detail.height);
    s.metallic = mat.params.x;
    s.roughness = mat.params.y;
    if ((flags & TEX_METALLIC_ROUGHNESS) != 0u) {
        s.metallic = saturate(s.metallic * mr_tex.b);
        s.roughness = clamp(s.roughness * mr_tex.g, 0.045, 1.0);
    }
    // Detail: rougher on the bumps, smoother in the hollows; hollows also see less sky.
    s.roughness = clamp(s.roughness * (1.0 + 0.3 * mat.uv.z * detail.height), 0.045, 1.0);
    s.reflectance = mat.params.z;
    s.normal = n;
    s.emissive = mat.emissive.rgb;
    if ((flags & TEX_EMISSIVE) != 0u) {
        s.emissive *= emissive_tex.rgb;
    }
    if ((flags & TEX_OCCLUSION) != 0u) {
        s.occlusion = mix(1.0, occlusion_tex.r, mat.params2.w);
    }
    s.occlusion *= 1.0 - 0.3 * mat.uv.z * saturate(-detail.height * 1.5);
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
    if (frame.counts.z == 4u) {
        return vec4<f32>(vec3(screen_ao(in.rel)), 1.0);
    }
    if (frame.counts.z == 5u) {
        let r = screen_reflection(in.rel);
        return vec4<f32>(pre_expose(r.rgb) * r.a, 1.0);
    }
    if (frame.counts.z == 6u) {
        return vec4<f32>(pre_expose(screen_gi(in.rel)), 1.0);
    }
    // Virtual geometry clusters or levels: the cluster vertex stage puts the debug color in
    // `color` (cluster.wgsl); other meshes show their vertex color. Lit by a plain sun term.
    if (frame.counts.z == 7u || frame.counts.z == 8u) {
        let lambert = 0.25 + 0.75 * saturate(dot(n, frame.sun_dir.xyz));
        return vec4<f32>(in.color.rgb * lambert, 1.0);
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

// ---------------------------------------------------------------- depth prepass
// Depth, normals, roughness and motion for ambient occlusion, contact shadows and temporal
// anti-aliasing. Same vertex positions as `vs_main` (invariant), so the main pass's depth test
// passes exactly where the prepass drew.

struct PrepassVertexOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) @interpolate(flat) material: u32,
    @location(5) motion_now: vec4<f32>,
    @location(6) motion_prev: vec4<f32>,
    @location(7) local: vec3<f32>,
};

@vertex
fn vs_prepass(in: VertexIn) -> PrepassVertexOut {
    return standard_prepass_vertex(in);
}

fn standard_prepass_vertex(in: VertexIn) -> PrepassVertexOut {
    let rel = vertex_rel(in);
    let p = vec4<f32>(in.position, 1.0);
    let prev = vec3<f32>(dot(in.p0, p), dot(in.p1, p), dot(in.p2, p)) + (rel - vec3<f32>(dot(in.m0, p), dot(in.m1, p), dot(in.m2, p)));
    var out: PrepassVertexOut;
    out.rel = rel;
    out.normal = normalize(mat3x3<f32>(in.n0.xyz, in.n1.xyz, in.n2.xyz) * in.normal);
    out.uv = in.uv;
    out.color = in.color;
    out.material = in.extra.x;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.motion_now = motion_clip(rel);
    out.motion_prev = prev_motion_clip(prev);
    out.local = vertex_local(in);
    return out;
}

@fragment
fn fs_prepass(in: PrepassVertexOut, @builtin(front_facing) front: bool) -> PrepassOutput {
    let mat = materials[in.material];
    let flags = mat.flags.x;
    let uv = in.uv * mat.uv.xy;
    let base_tex = textureSample(base_color_texture, material_sampler, uv);
    let normal_tex = textureSample(normal_texture, material_sampler, uv);
    let mr_tex = textureSample(metallic_roughness_texture, material_sampler, uv);
    let dp1 = dpdx(in.rel);
    let dp2 = dpdy(in.rel);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dpos = detail_pos(mat, in.local, in.rel);
    let dq1 = dpdx(dpos);
    let dq2 = dpdy(dpos);

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
    let detail = surface_detail(mat, n, dpos, dp1, dp2, dq1, dq2);
    n = detail.normal;
    if (!front) { n = -n; }
    var roughness = mat.params.y;
    if ((flags & TEX_METALLIC_ROUGHNESS) != 0u) {
        roughness = clamp(roughness * mr_tex.g, 0.045, 1.0);
    }
    roughness = clamp(roughness * (1.0 + 0.3 * mat.uv.z * detail.height), 0.045, 1.0);
    if ((flags & 64u) != 0u) {
        let wet = frame.weather.x * saturate(n.y * 0.7 + 0.5);
        roughness = mix(roughness, 0.12, wet * 0.85);
    }
    return prepass_output(n, roughness, in.motion_now, in.motion_prev);
}
