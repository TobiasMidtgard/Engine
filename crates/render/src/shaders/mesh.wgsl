// Standard PBR mesh feature. Requires lib_types + lib_frame.

struct Material {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    params: vec4<f32>,   // metallic, roughness, reflectance, alpha cutoff
    params2: vec4<f32>,  // subsurface, wind sway, normal scale, occlusion strength
    uv: vec4<f32>,       // uv scale xy, detail strength, detail scale (m)
    mapping: vec4<f32>,  // tiles per meter (1 / texture scale), parallax depth (m), triplanar sharpness, unused
    flags: vec4<u32>,    // x: bits 0-3 alpha mode, 4 unlit, 5 double sided, 6 receives wetness,
                         //    7-12 texture present (base color, normal, metallic/roughness, emissive,
                         //    occlusion, height), 13 detail anchored to the world,
                         //    14-15 mapping (0 UV, 1 triplanar, 2 triplanar in world space),
                         //    16 parallax self-shadowing
};

const TEX_BASE_COLOR: u32 = 128u;
const TEX_NORMAL: u32 = 256u;
const TEX_METALLIC_ROUGHNESS: u32 = 512u;
const TEX_EMISSIVE: u32 = 1024u;
const TEX_OCCLUSION: u32 = 2048u;
const TEX_HEIGHT: u32 = 4096u;
const TEX_ANY: u32 = 8064u;
const DETAIL_IN_WORLD: u32 = 8192u;
const MAPPING_SHIFT: u32 = 14u;
const PARALLAX_SHADOWS: u32 = 65536u;

@group(1) @binding(0) var<storage, read> materials: array<Material>;

// Per-material textures. Untextured materials bind 1x1 defaults; the TEX_* flags say which
// slots hold real images (or generated preset maps).
@group(2) @binding(0) var base_color_texture: texture_2d<f32>;
@group(2) @binding(1) var normal_texture: texture_2d<f32>;
@group(2) @binding(2) var metallic_roughness_texture: texture_2d<f32>;
@group(2) @binding(3) var emissive_texture: texture_2d<f32>;
@group(2) @binding(4) var occlusion_texture: texture_2d<f32>;
@group(2) @binding(5) var height_texture: texture_2d<f32>;
@group(2) @binding(6) var material_sampler: sampler;

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
    // Object-space normal (for object-space triplanar weights) and the object's axes in the
    // world (unit length) that turn its projections into world directions.
    @location(6) object_normal: vec3<f32>,
    @location(7) @interpolate(flat) axis_x: vec3<f32>,
    @location(8) @interpolate(flat) axis_y: vec3<f32>,
    @location(9) @interpolate(flat) axis_z: vec3<f32>,
};

// Wind sway of a vertex (meters, camera-relative): the material's `wind_sway` times the vertex
// color's alpha, its wind weight (0 = still, such as a trunk's base; 1 = full sway, the default
// of meshes without vertex colors), growing with height in the mesh.
fn vertex_wind(in: VertexIn, rel: vec3<f32>) -> vec3<f32> {
    let sway = materials[in.extra.x].params2.y * saturate(in.color.a);
    if (sway <= 0.0) { return vec3<f32>(0.0); }
    let h01 = saturate((in.position.y - bitcast<f32>(in.extra.z)) / max(bitcast<f32>(in.extra.w), 1e-3));
    return wind_offset(rel, h01, sway);
}

// The model's scale along each object axis.
fn vertex_scale(in: VertexIn) -> vec3<f32> {
    return vec3<f32>(
        length(vec3<f32>(in.m0.x, in.m1.x, in.m2.x)),
        length(vec3<f32>(in.m0.y, in.m1.y, in.m2.y)),
        length(vec3<f32>(in.m0.z, in.m1.z, in.m2.z)),
    );
}

// Object-space position in meters (the model's scale applied, not its rotation or offset), so
// surface detail and triplanar textures stick to moving objects at a consistent size.
fn vertex_local(in: VertexIn) -> vec3<f32> {
    return in.position * vertex_scale(in);
}

// Normal of the scaled object-space surface (`vertex_local`).
fn vertex_object_normal(in: VertexIn) -> vec3<f32> {
    return normalize(in.normal / max(vertex_scale(in), vec3<f32>(1e-6)));
}

// World direction of an object axis (column `i` of the model matrix, normalized).
fn vertex_axis(in: VertexIn, i: u32) -> vec3<f32> {
    let c = vec3<f32>(in.m0[i], in.m1[i], in.m2[i]);
    return c / max(length(c), 1e-12);
}

// The vertex color the surface sees. On swaying materials the alpha is the wind weight
// (`vertex_wind`), not opacity.
fn vertex_surface_color(in: VertexIn) -> vec4<f32> {
    let swaying = materials[in.extra.x].params2.y > 0.0;
    return select(in.color, vec4<f32>(in.color.rgb, 1.0), swaying);
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
    out.color = vertex_surface_color(in);
    out.material = in.extra.x;
    out.local = vertex_local(in);
    out.object_normal = vertex_object_normal(in);
    out.axis_x = vertex_axis(in, 0u);
    out.axis_y = vertex_axis(in, 1u);
    out.axis_z = vertex_axis(in, 2u);
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

// ---------------------------------------------------------------- texture projections
// A way of reading the material's textures at this pixel: the mesh UVs, or one of the three
// triplanar projections. `uv` is in texture tiles, `d1`/`d2` its screen derivatives (the
// sampling footprint), `grad_u`/`grad_v` the world-space gradients of u and v along the
// surface (tiles per meter; parallax steps through them), `t`/`b` the world directions in
// which the texture's +x and +y (image up) point (normal maps), `weight` the blend weight.
struct Projection {
    uv: vec2<f32>,
    d1: vec2<f32>,
    d2: vec2<f32>,
    grad_u: vec3<f32>,
    grad_v: vec3<f32>,
    t: vec3<f32>,
    b: vec3<f32>,
    weight: f32,
};

// What one projection reads, after parallax.
struct ProjectionSample {
    base: vec4<f32>,
    tangent_normal: vec3<f32>, // as filtered (shorter than 1 where the texels disagree)
    metallic_roughness: vec4<f32>,
    emissive: vec3<f32>,
    occlusion: f32,
    sun: f32,                  // parallax self-shadowing (1 = lit)
    height: f32,               // height map at the parallax hit (0..1)
};

// The world-space surface gradients of a UV set (see `perturb_normal`): `grad_u` and `grad_v`
// such that moving `dp` along the surface changes the UV by (grad_u·dp, grad_v·dp).
fn uv_gradients(n: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, duv1: vec2<f32>, duv2: vec2<f32>) -> mat2x3<f32> {
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let det = dot(dp1, dp2perp);
    if (abs(det) < 1e-24) { return mat2x3<f32>(vec3<f32>(0.0), vec3<f32>(0.0)); }
    return mat2x3<f32>(
        (dp2perp * duv1.x + dp1perp * duv2.x) / det,
        (dp2perp * duv1.y + dp1perp * duv2.y) / det,
    );
}

// Triplanar projection `k` (0 = along x, 1 = y, 2 = z) of the projection-space point `q`
// (tiles) with screen derivatives `q1`/`q2`. `s` is the side of the axis the surface faces:
// the image's u axis flips with it so no side shows the texture mirrored. `axes` turns the
// projection space into world directions; `tiles_per_m` converts its gradients to world.
fn triplanar_projection(k: u32, s: f32, q: vec3<f32>, q1: vec3<f32>, q2: vec3<f32>, axes: mat3x3<f32>, tiles_per_m: f32, weight: f32) -> Projection {
    var t: vec3<f32>;
    var b: vec3<f32>;
    if (k == 0u) {
        t = vec3<f32>(0.0, 0.0, -s);
        b = vec3<f32>(0.0, 1.0, 0.0);
    } else if (k == 1u) {
        t = vec3<f32>(s, 0.0, 0.0);
        b = vec3<f32>(0.0, 0.0, -1.0);
    } else {
        t = vec3<f32>(s, 0.0, 0.0);
        b = vec3<f32>(0.0, 1.0, 0.0);
    }
    var p: Projection;
    p.uv = vec2<f32>(dot(q, t), -dot(q, b));
    p.d1 = vec2<f32>(dot(q1, t), -dot(q1, b));
    p.d2 = vec2<f32>(dot(q2, t), -dot(q2, b));
    p.t = axes * t;
    p.b = axes * b;
    p.grad_u = p.t * tiles_per_m;
    p.grad_v = -p.b * tiles_per_m;
    p.weight = weight;
    return p;
}

const PARALLAX_MAX_STEPS: i32 = 32;
const PARALLAX_SHADOW_STEPS: i32 = 10;

// Parallax occlusion mapping (Tatarchuk 2006): march the view ray down through the height map
// (1 = the polygon's surface, 0 = `depth` meters below it) in `steps`, refine the hit linearly
// between the last two samples, and return the texture coordinate there with the hit height.
// `offset` is how far the texture coordinate moves when the ray sinks the full depth.
fn parallax_march(uv: vec2<f32>, offset: vec2<f32>, d1: vec2<f32>, d2: vec2<f32>, steps: i32) -> vec3<f32> {
    var prev_uv = uv;
    var prev_ray = 1.0;
    var prev_gap = 1.0 - textureSampleGrad(height_texture, material_sampler, uv, d1, d2).r;
    if (prev_gap <= 0.0) { return vec3<f32>(uv, 1.0); }
    let step = 1.0 / f32(steps);
    for (var i = 1; i <= PARALLAX_MAX_STEPS; i++) {
        if (i > steps) { break; }
        let ray = 1.0 - f32(i) * step;
        let p = uv + offset * (1.0 - ray);
        let h = textureSampleGrad(height_texture, material_sampler, p, d1, d2).r;
        let gap = ray - h;
        if (gap <= 0.0) {
            // The ray crossed the height field between the previous sample and this one.
            let t = prev_gap / max(prev_gap - gap, 1e-6);
            return vec3<f32>(mix(prev_uv, p, t), mix(prev_ray, ray, t));
        }
        prev_uv = p;
        prev_ray = ray;
        prev_gap = gap;
    }
    return vec3<f32>(uv + offset, 0.0);
}

// Soft self-shadowing of the relief toward the sun from the parallax hit (`uv`, height `h`):
// march up toward the light and darken by how far the height field rises above the ray, the
// more the closer to the hit (Tatarchuk 2006). `offset` is the texture coordinate shift for a
// full-depth rise toward the light.
fn parallax_shadow(uv: vec2<f32>, h: f32, offset: vec2<f32>, d1: vec2<f32>, d2: vec2<f32>) -> f32 {
    let rise = 1.0 - h;
    if (rise <= 0.02) { return 1.0; }
    var blocked = 0.0;
    for (var i = 1; i <= PARALLAX_SHADOW_STEPS; i++) {
        let f = f32(i) / f32(PARALLAX_SHADOW_STEPS);
        let ray = h + rise * f;
        let above = textureSampleGrad(height_texture, material_sampler, uv + offset * rise * f, d1, d2).r - ray;
        blocked = max(blocked, above * (1.0 - f) * 12.0);
    }
    return 1.0 - saturate(blocked);
}

// Read every texture through projection `p`, with parallax when the material has a height map.
// `n` is the shaded geometric normal, `v` the direction to the camera, `footprint` the size of a
// pixel on the surface (meters). `shadows` asks for the parallax self-shadow toward the sun.
fn sample_projection(mat: Material, p: Projection, n: vec3<f32>, v: vec3<f32>, footprint: f32, shadows: bool) -> ProjectionSample {
    let flags = mat.flags.x;
    var uv = p.uv;
    var out: ProjectionSample;
    out.sun = 1.0;
    out.height = 1.0;
    let depth = mat.mapping.y;
    if ((flags & TEX_HEIGHT) != 0u && depth > 0.0) {
        let v_n = dot(n, v);
        // Fade out where the relief is under about a pixel deep on screen, and at grazing
        // angles, where the offsets grow without bound.
        let relief_px = depth / max(footprint, 1e-6);
        let fade = saturate(relief_px * 0.5 - 0.25) * smoothstep(0.04, 0.15, v_n);
        if (fade > 0.0) {
            // Sinking the full depth moves the ray this far along the surface (meters).
            let lateral = -(v - n * v_n) / max(v_n, 0.08) * depth * fade;
            let offset = vec2<f32>(dot(p.grad_u, lateral), dot(p.grad_v, lateral));
            let lateral_px = length(lateral) / max(footprint, 1e-6);
            let steps = i32(clamp(lateral_px * 0.5 + relief_px * 0.25, 6.0, f32(PARALLAX_MAX_STEPS)));
            let hit = parallax_march(uv, offset, p.d1, p.d2, steps);
            uv = hit.xy;
            out.height = mix(1.0, hit.z, fade);
            let l = frame.sun_dir.xyz;
            let l_n = dot(n, l);
            if (shadows && (flags & PARALLAX_SHADOWS) != 0u && l_n > 0.0) {
                let toward = (l - n * l_n) / max(l_n, 0.1) * depth * fade;
                let l_offset = vec2<f32>(dot(p.grad_u, toward), dot(p.grad_v, toward));
                out.sun = parallax_shadow(uv, hit.z, l_offset, p.d1, p.d2);
            }
        }
    }
    out.base = vec4<f32>(1.0);
    if ((flags & TEX_BASE_COLOR) != 0u) {
        out.base = textureSampleGrad(base_color_texture, material_sampler, uv, p.d1, p.d2);
    }
    out.tangent_normal = vec3<f32>(0.0, 0.0, 1.0);
    if ((flags & TEX_NORMAL) != 0u) {
        out.tangent_normal = textureSampleGrad(normal_texture, material_sampler, uv, p.d1, p.d2).xyz * 2.0 - 1.0;
    }
    out.metallic_roughness = vec4<f32>(1.0);
    if ((flags & TEX_METALLIC_ROUGHNESS) != 0u) {
        out.metallic_roughness = textureSampleGrad(metallic_roughness_texture, material_sampler, uv, p.d1, p.d2);
    }
    out.emissive = vec3<f32>(1.0);
    if ((flags & TEX_EMISSIVE) != 0u) {
        out.emissive = textureSampleGrad(emissive_texture, material_sampler, uv, p.d1, p.d2).rgb;
    }
    out.occlusion = 1.0;
    if ((flags & TEX_OCCLUSION) != 0u) {
        out.occlusion = textureSampleGrad(occlusion_texture, material_sampler, uv, p.d1, p.d2).r;
    }
    return out;
}

// Extra roughness from a mip-filtered normal map (Toksvig; the vMF fit of Neubelt and Pettineo
// 2013): where the texels under a pixel point different ways, their average is shorter than 1
// and the surface should look rougher, not sparkle. Returns the added alpha² (alpha = roughness²).
fn normal_variance(filtered: vec3<f32>) -> f32 {
    // 8-bit texels are a bit short even when they agree.
    let r = min(length(filtered) + 0.004, 1.0);
    if (r >= 0.9999) { return 0.0; }
    let kappa = (3.0 * r - r * r * r) / (1.0 - r * r);
    return 0.5 / kappa;
}

// Surface gradient of a tangent-space normal `tn` mapped with world directions `t` (+x) and
// `b` (+y), relative to the surface normal `n` (Mikkelsen 2020, "Surface Gradient Based Bump
// Mapping Framework"): blendable between projections, resolved with `normalize(n - gradient)`.
fn surface_gradient(n: vec3<f32>, t: vec3<f32>, b: vec3<f32>, tn: vec3<f32>) -> vec3<f32> {
    let z = max(tn.z, 0.1);
    let g = t * (tn.x / z) + b * (tn.y / z);
    return n * dot(n, g) - g;
}

// ---------------------------------------------------------------- the material at a pixel
// Everything the main pass and the prepass need from the material, computed by the same code so
// the normals and roughness that ambient occlusion, reflections and TAA read from the prepass
// match what is shaded.
struct SurfaceIn {
    rel: vec3<f32>,
    normal: vec3<f32>,
    uv: vec2<f32>,
    color: vec4<f32>,
    material: u32,
    local: vec3<f32>,
    object_normal: vec3<f32>,
    axes: mat3x3<f32>,
    front: bool,
};

struct MaterialSample {
    base: vec4<f32>,        // linear base color (before detail mottling) and alpha
    albedo: vec3<f32>,      // with detail mottling
    normal: vec3<f32>,      // shaded world normal, facing the camera
    smooth_normal: vec3<f32>, // the interpolated vertex normal on the same side (no bumps)
    metallic: f32,
    roughness: f32,
    occlusion: f32,
    emissive: vec3<f32>,
    sun: f32,               // parallax self-shadowing toward the sun (1 = lit)
    subsurface: f32,
    face: vec3<f32>,        // the triangle's geometric normal, toward the camera (the side in view)
    thin: bool,             // a thin sheet (double-sided or alpha-masked): `shade_thin_surface`
};

const MATERIAL_DOUBLE_SIDED: u32 = 32u;

// Whether a material is a thin sheet seen from either side (leaves, petals, cloth, cards):
// double-sided, or alpha-masked (cutout cards, which are often drawn with a triangle per side).
fn material_is_thin(flags: u32) -> bool {
    return (flags & MATERIAL_DOUBLE_SIDED) != 0u || (flags & 15u) == 1u;
}

fn evaluate_material(si: SurfaceIn, shadows: bool) -> MaterialSample {
    let mat = materials[si.material];
    let flags = mat.flags.x;
    let mapping = (flags >> MAPPING_SHIFT) & 3u;
    let tiles_per_m = mat.mapping.x;

    // Screen derivatives up front (uniform control flow): position, mesh UVs, the triplanar
    // projection space and the detail space.
    let dp1 = dpdx(si.rel);
    let dp2 = dpdy(si.rel);
    let uv = si.uv * mat.uv.xy * tiles_per_m;
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    var q = si.local * tiles_per_m;
    if (mapping == 2u) { q = tiling_pos(si.rel) * tiles_per_m; }
    let q1 = dpdx(q);
    let q2 = dpdy(q);
    let dpos = detail_pos(mat, si.local, si.rel);
    let dd1 = dpdx(dpos);
    let dd2 = dpdy(dpos);

    // Back faces of double-sided materials are shaded with the normal turned toward the camera.
    var n = normalize(si.normal);
    if (!si.front) { n = -n; }
    let v = normalize(-si.rel);
    let footprint = max(max(length(dp1), length(dp2)), 1e-6);
    // The side of the triangle in view: its own plane's normal, turned toward the camera
    // (whatever the winding and the vertex normals say).
    let g = cross(dp1, dp2);
    let g_len = length(g);
    var face = select(n, g / g_len, g_len > 1e-24);
    if (dot(face, v) < 0.0) { face = -face; }

    var base = vec4<f32>(1.0);
    var mr = vec4<f32>(1.0);
    var emissive = vec3<f32>(1.0);
    var occlusion = 1.0;
    var sun = 1.0;
    var variance = 0.0;
    var shaded = n;
    if ((flags & TEX_ANY) != 0u) {
        let normal_scale = vec3<f32>(mat.params2.z, mat.params2.z, 1.0);
        if (mapping == 0u) {
            var p: Projection;
            p.uv = uv;
            p.d1 = duv1;
            p.d2 = duv2;
            let g = uv_gradients(n, dp1, dp2, duv1, duv2);
            p.grad_u = g[0];
            p.grad_v = g[1];
            p.weight = 1.0;
            let s = sample_projection(mat, p, n, v, footprint, shadows);
            base = s.base;
            mr = s.metallic_roughness;
            emissive = s.emissive;
            occlusion = s.occlusion;
            sun = s.sun;
            if ((flags & TEX_NORMAL) != 0u) {
                variance = normal_variance(s.tangent_normal);
                shaded = perturb_normal(n, dp1, dp2, duv1, duv2, normalize(s.tangent_normal) * normal_scale);
            }
        } else {
            // Triplanar: weights from the normal in projection space, sharpened, and the
            // smallest ones dropped (smoothly, so no seams appear where they switch off).
            var axes = si.axes;
            var pn = normalize(si.object_normal);
            if (mapping == 2u) {
                axes = mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
                pn = normalize(si.normal);
            }
            var w = pow(abs(pn), vec3<f32>(mat.mapping.z));
            w = max(w / (w.x + w.y + w.z) - 0.02, vec3<f32>(0.0));
            w /= max(w.x + w.y + w.z, 1e-6);
            let side = select(vec3<f32>(-1.0), vec3<f32>(1.0), pn >= vec3<f32>(0.0));
            var gradient = vec3<f32>(0.0);
            base = vec4<f32>(0.0);
            mr = vec4<f32>(0.0);
            emissive = vec3<f32>(0.0);
            occlusion = 0.0;
            sun = 0.0;
            for (var k = 0u; k < 3u; k++) {
                if (w[k] <= 0.0) { continue; }
                let p = triplanar_projection(k, side[k], q, q1, q2, axes, tiles_per_m, w[k]);
                let s = sample_projection(mat, p, n, v, footprint, shadows);
                base += s.base * w[k];
                mr += s.metallic_roughness * w[k];
                emissive += s.emissive * w[k];
                occlusion += s.occlusion * w[k];
                sun += s.sun * w[k];
                if ((flags & TEX_NORMAL) != 0u) {
                    variance += normal_variance(s.tangent_normal) * w[k];
                    gradient += surface_gradient(n, p.t, p.b, normalize(s.tangent_normal) * normal_scale) * w[k];
                }
            }
            let resolved = n - gradient;
            shaded = select(n, normalize(resolved), dot(resolved, resolved) > 1e-12);
        }
    }

    var out: MaterialSample;
    out.base = mat.base_color * si.color * base;
    let detail = surface_detail(mat, shaded, dpos, dp1, dp2, dd1, dd2);
    out.normal = detail.normal;
    out.albedo = detail_albedo(mat, out.base.rgb, dpos, detail.height);
    out.metallic = saturate(mat.params.x * mr.b);
    var alpha = sq(clamp(mat.params.y * mr.g, 0.045, 1.0));
    alpha = sqrt(alpha * alpha + variance);
    // Detail: rougher on the bumps, smoother in the hollows; hollows also see less sky.
    out.roughness = clamp(sqrt(alpha) * (1.0 + 0.3 * mat.uv.z * detail.height), 0.045, 1.0);
    out.occlusion = 1.0;
    if ((flags & TEX_OCCLUSION) != 0u) {
        out.occlusion = mix(1.0, occlusion, mat.params2.w);
    }
    out.occlusion *= 1.0 - 0.3 * mat.uv.z * saturate(-detail.height * 1.5);
    out.emissive = mat.emissive.rgb * emissive;
    out.sun = sun;
    out.subsurface = mat.params2.x;
    out.face = face;
    out.thin = material_is_thin(flags);
    // A thin sheet is shaded on the side in view: a shading normal that points into the sheet
    // (vertex normals that disagree with the winding, a normal map tilting past the plane) is
    // mirrored out of it, so the side away from a light never lights up or catches its glare.
    let into = dot(out.normal, face);
    if (out.thin && into < 0.0) {
        out.normal = normalize(out.normal - 2.0 * into * face);
    }
    out.smooth_normal = n;
    let smooth_into = dot(n, face);
    if (out.thin && smooth_into < 0.0) {
        out.smooth_normal = normalize(n - 2.0 * smooth_into * face);
    }

    // Weather: rain darkens porous surfaces and makes them glossy; snow settles on top faces.
    if ((flags & 64u) != 0u) {
        let n_up = out.normal.y;
        let wet = frame.weather.x * saturate(n_up * 0.7 + 0.5);
        out.albedo *= mix(1.0, 0.55, wet * (1.0 - out.metallic));
        out.roughness = mix(out.roughness, 0.12, wet * 0.85);
        let snow_cover = saturate(frame.weather.y * 10.0) * smoothstep(0.35, 0.8, n_up);
        let snow_noise = value_noise2(tiling_pos(si.rel).xz * 1.7);
        let snow = saturate(snow_cover * (0.75 + 0.5 * snow_noise));
        out.albedo = mix(out.albedo, vec3<f32>(0.9, 0.92, 0.95), snow);
        out.roughness = mix(out.roughness, 0.75, snow);
        out.subsurface = mix(out.subsurface, 0.3, snow);
    }
    return out;
}

fn surface_in(in: VertexOut, front: bool) -> SurfaceIn {
    return SurfaceIn(in.rel, in.normal, in.uv, in.color, in.material, in.local, in.object_normal,
        mat3x3<f32>(in.axis_x, in.axis_y, in.axis_z), front);
}

@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let mat = materials[in.material];
    let flags = mat.flags.x;
    let m = evaluate_material(surface_in(in, front), true);
    if ((flags & 15u) == 1u && m.base.a < mat.params.w) {
        discard;
    }

    var s = default_surface();
    s.albedo = m.albedo;
    s.metallic = m.metallic;
    s.roughness = m.roughness;
    s.reflectance = mat.params.z;
    s.normal = m.normal;
    s.emissive = m.emissive;
    s.occlusion = m.occlusion;
    s.subsurface = m.subsurface;
    s.smooth_normal = m.smooth_normal;

    // Debug views (RenderSettings.debug_view).
    let n = m.normal;
    if (frame.counts.z == 1u) {
        return vec4<f32>(n * 0.5 + 0.5, 1.0);
    }
    if (frame.counts.z == 3u) {
        return vec4<f32>(vec3(sky_visibility(in.rel, m.smooth_normal)), 1.0);
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
    } else if (m.thin) {
        // Leaves, petals, cloth: lit from behind they glow by `subsurface`, never shine.
        color = shade_thin_surface(s, in.rel, m.sun, m.face);
    } else {
        color = shade_surface_shadowed(s, in.rel, m.sun);
    }
    color = apply_fog(color, in.rel);
    if (frame.counts.z == 2u) {
        let depth = view_depth(in.rel);
        var tint = vec3<f32>(1.0, 0.4, 0.4);
        if (depth > frame.shadow_splits[0]) { tint = vec3(0.4, 1.0, 0.4); }
        if (depth > frame.shadow_splits[1]) { tint = vec3(0.4, 0.4, 1.0); }
        if (depth > frame.shadow_splits[2]) { tint = vec3(1.0, 1.0, 0.4); }
        if (depth > frame.shadow_splits[3]) {
            // Magenta where only the far shadow map reaches.
            tint = select(vec3(1.0), vec3(1.0, 0.4, 1.0), length(in.rel) < frame.far_shadow_params.x);
        }
        color *= tint;
    }
    return vec4<f32>(color, m.base.a);
}

// ---------------------------------------------------------------- depth prepass
// Depth, normals, roughness and motion for ambient occlusion, contact shadows and temporal
// anti-aliasing. Same vertex positions as `vs_main` (invariant), so the main pass's depth test
// passes exactly where the prepass drew, and the same `evaluate_material`, so its normals and
// roughness (parallax and triplanar included) are the ones the main pass shades with.

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
    @location(8) object_normal: vec3<f32>,
    @location(9) @interpolate(flat) axis_x: vec3<f32>,
    @location(10) @interpolate(flat) axis_y: vec3<f32>,
    @location(11) @interpolate(flat) axis_z: vec3<f32>,
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
    out.color = vertex_surface_color(in);
    out.material = in.extra.x;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.motion_now = motion_clip(rel);
    out.motion_prev = prev_motion_clip(prev);
    out.local = vertex_local(in);
    out.object_normal = vertex_object_normal(in);
    out.axis_x = vertex_axis(in, 0u);
    out.axis_y = vertex_axis(in, 1u);
    out.axis_z = vertex_axis(in, 2u);
    return out;
}

@fragment
fn fs_prepass(in: PrepassVertexOut, @builtin(front_facing) front: bool) -> PrepassOutput {
    let mat = materials[in.material];
    let si = SurfaceIn(in.rel, in.normal, in.uv, in.color, in.material, in.local, in.object_normal,
        mat3x3<f32>(in.axis_x, in.axis_y, in.axis_z), front);
    let m = evaluate_material(si, false);
    if ((mat.flags.x & 15u) == 1u && m.base.a < mat.params.w) {
        discard;
    }
    return prepass_output(m.normal, m.roughness, in.motion_now, in.motion_prev);
}
