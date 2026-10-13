// Skinned characters. Requires lib_types + lib_frame.
//
// Vertices come from the compute pass (position + tension, normal + muscle activation) and a
// static buffer (uv, color, detail coordinates, masks). Every draw has a *surface* (group 2):
// base color, tangent-space normal and metallic/roughness textures with their factors
// (imported characters), or the default surface (1x1 neutral textures, flags 0) that shades
// procedural characters exactly as before textures existed. Procedural detail applies on top
// of either, where the vertex masks say so:
// - cloth (mask.x): compressed regions bunch outward and fold into rings across the limb axis;
//   stretched regions show diagonal pull lines. Folds darken in their creases.
// - skin (mask.y): subsurface wrap, and a slight flush where muscles work.
// - veins (mask.z): raised branching lines along the limb that appear with muscle activation.
// - sheen (mask.w): soft grazing-angle glow for fabric and short fur.
//
// Fur: where a vertex grows fur (pattern.w, 0..1 of the material's fur length) the same mesh is
// drawn again as shells (instance extra.z = shell 1..=N, extra.w = N). Each shell sits further
// out along the normal, its tips sagging and combed back, and keeps only the pixels inside a
// tapering strand of a jittered grid in bind-pose space, so strands run straight through the
// shells and stay put on the skin. The skin under the shells is the coat's dark root.

struct SkinMaterial {
    tint: vec4<f32>,
    params: vec4<f32>,    // skin roughness, cloth roughness, subsurface, metallic
    detail: vec4<f32>,    // wrinkle depth, wrinkle frequency (folds/m), veins, bunch (m)
    vein: vec4<f32>,      // vein rgb, sheen
    emissive: vec4<f32>,  // rgb (cd/m^2)
    fur: vec4<f32>,       // length (m), strands per meter, droop, comb
    fur_tip: vec4<f32>,   // tip color multiplier rgb, root brightness
};

// Must match `GpuSurface` in feature.rs and `struct SkinSurface` in skin_shadow.wgsl.
struct SkinSurface {
    base_color: vec4<f32>,  // linear RGBA factor
    params: vec4<f32>,      // perceptual roughness, metallic, normal scale, alpha cutoff (0 = none)
    extra: vec4<f32>,       // uv scale xy, subsurface, reflectance
    flags: vec4<u32>,       // x: SURFACE_* bits
};

const SURFACE_BASE_COLOR: u32 = 1u;
const SURFACE_NORMAL: u32 = 2u;
const SURFACE_METALLIC_ROUGHNESS: u32 = 4u;
const SURFACE_DOUBLE_SIDED: u32 = 8u;
// The surface's roughness and metallic replace the SkinMaterial's skin/cloth choice.
const SURFACE_STANDARD: u32 = 16u;

@group(1) @binding(0) var<storage, read> materials: array<SkinMaterial>;

@group(2) @binding(0) var<uniform> skin_surface: SkinSurface;
@group(2) @binding(1) var base_color_texture: texture_2d<f32>;
@group(2) @binding(2) var normal_texture: texture_2d<f32>;
@group(2) @binding(3) var metallic_roughness_texture: texture_2d<f32>;
@group(2) @binding(4) var surface_sampler: sampler;

struct VertexIn {
    @location(0) position_tension: vec4<f32>,
    @location(1) normal_activation: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    // Instance: model matrix rows (camera-relative), normal matrix columns, extra.
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) n0: vec4<f32>,
    @location(8) n1: vec4<f32>,
    @location(9) n2: vec4<f32>,
    @location(10) extra: vec4<u32>,  // material index, job flags
    @location(11) pattern: vec4<f32>,
    @location(12) axis: vec4<f32>,
    @location(13) mask: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) pattern: vec4<f32>,  // bind-pose position, fur amount
    @location(5) axis: vec3<f32>,
    @location(6) mask: vec4<f32>,
    @location(7) tension: f32,
    @location(8) activation: f32,
    @location(9) @interpolate(flat) material: u32,
    @location(10) shell: vec2<f32>,  // shell height (0 = the skin), shells drawn over it (0/1)
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let mat = materials[in.extra.x];
    let tension = in.position_tension.w;
    var p = in.position_tension.xyz;
    let n = in.normal_activation.xyz;
    // Compressed cloth gathers outward (sleeves bunching at the elbow).
    p += n * (mat.detail.w * saturate(-tension * 4.0) * in.mask.x);
    // Fur shells stand out along the normal.
    let fur_len = mat.fur.x * in.pattern.w;
    var h = 0.0;
    if (in.extra.z > 0u) {
        h = f32(in.extra.z) / f32(max(in.extra.w, 1u));
        p += n * (fur_len * h);
    }
    let ph = vec4<f32>(p, 1.0);
    var rel = vec3<f32>(dot(in.m0, ph), dot(in.m1, ph), dot(in.m2, ph));
    let nw = normalize(mat3x3<f32>(in.n0.xyz, in.n1.xyz, in.n2.xyz) * n);
    if (h > 0.0) {
        // The strands bend more toward their tips: down under their weight and back along the
        // body (model +Z), within the skin's tangent plane so the coat lies flatter.
        let back = normalize(vec3<f32>(in.m0.z, in.m1.z, in.m2.z) + vec3<f32>(0.0, 0.0, 1e-6));
        var lean = vec3<f32>(0.0, -1.0, 0.0) * mat.fur.z + back * mat.fur.w;
        lean -= nw * dot(lean, nw);
        rel += lean * (fur_len * h * h);
    }
    var out: VertexOut;
    out.rel = rel;
    out.normal = nw;
    out.uv = in.uv;
    out.color = in.color;
    out.pattern = in.pattern;
    out.shell = vec2<f32>(h, select(0.0, 1.0, in.extra.w > 0u));
    out.axis = in.axis.xyz;
    out.mask = in.mask;
    out.tension = tension;
    out.activation = in.normal_activation.w;
    out.material = in.extra.x;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    return out;
}

// Bump mapping from a scalar height field without tangents (Mikkelsen, "Bump Mapping
// Unparametrized Surfaces on the GPU", 2010), using screen-space derivatives.
fn bump_normal(n: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, dh1: f32, dh2: f32) -> vec3<f32> {
    let r1 = cross(dp2, n);
    let r2 = cross(n, dp1);
    let det = dot(dp1, r1);
    let grad = sign(det) * (dh1 * r1 + dh2 * r2);
    let b = abs(det) * n - grad;
    let len = length(b);
    return select(n, b / len, len > 1e-12);
}

// Normal mapping without precomputed tangents (Schüler, "Normal Mapping Without Precomputed
// Tangents", ShaderX5 / 2013): the cotangent frame (surface gradients of u and v) comes from
// screen-space derivatives of the position and the texture coordinates. Same conventions as
// the engine's mesh shader:
// - `n` is the interpolated front-face unit normal, `tn` the tangent-space normal (+Y = green
//   up, glTF/OpenGL convention);
// - WebGPU window y points down, the opposite of the GL setting the formula was written for;
//   the sign of n·(dp1 × dp2) makes the frame independent of that (and of back faces);
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

// The interpolated normal `n`, brought to the side of the triangle the camera sees (double-sided
// surfaces). On a back face it points away from the viewer; it is mirrored through the
// triangle's plane (the geometric normal comes from screen-space derivatives). For a normal
// that is perpendicular to the face this is the usual negation, but it also stays continuous
// where bent normals cross the plane inside a triangle, which hair cards often have (their
// normals lean toward the scalp, and duplicated layers mix both sides), so no back-facing
// normals with a mirror-like Fresnel rim and no seams.
fn facing_normal(n: vec3<f32>, dp1: vec3<f32>, dp2: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    let g = cross(dp1, dp2);
    let len = length(g);
    if (len <= 1e-20) {
        return n;
    }
    // Toward the camera: `rel` points from the camera to the surface.
    let toward = g * (-sign(dot(g, rel)) / len);
    let d = dot(n, toward);
    if (d >= 0.0) {
        return n;
    }
    return normalize(n - 2.0 * d * toward);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let mat = materials[in.material];
    let surf = skin_surface;
    let flags = surf.flags.x;
    let cloth = in.mask.x;
    let skin = in.mask.y;
    let compress = saturate(-in.tension * 5.0) * cloth;
    let stretch = saturate(in.tension * 8.0) * cloth;

    // Coordinates along and around the limb axis (bind pose, so the pattern sticks to the body).
    let a = normalize(in.axis + vec3<f32>(1e-5, 0.0, 0.0));
    let along = dot(in.pattern.xyz, a);
    let side_ref = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(a.y) > 0.8);
    let b = normalize(cross(a, side_ref));
    let c = cross(a, b);
    let perp = in.pattern.xyz - a * along;
    let around = atan2(dot(perp, c), dot(perp, b));
    let warp = value_noise3(in.pattern.xyz * 7.0) * 2.0 - 1.0;
    let freq = mat.detail.y;

    // Compression folds: rings across the axis, wobbly and uneven like real fabric.
    let fold_phase = along * freq + warp * 0.9 + value_noise3(in.pattern.xyz * 23.0) * 0.4;
    let fold = sin(fold_phase * TAU);
    let fold_shape = 1.0 - abs(fold);  // sharp ridges
    // Stretch: diagonal pull lines.
    let pull_phase = (along * 0.6 + around * 0.12) * freq * 0.7 + warp * 0.5;
    let pull = 0.5 + 0.5 * sin(pull_phase * TAU);

    // Veins: branching ridges stretched along the axis, only on veined skin, only when working.
    let vein_p = in.pattern.xyz * 55.0 - a * (along * 55.0 * 0.8);
    let ridge = 1.0 - abs(value_noise3(vein_p) * 2.0 - 1.0);
    let ridge2 = 1.0 - abs(value_noise3(vein_p * 2.1 + 3.7) * 2.0 - 1.0);
    // A vein is a few millimeters wide: once it is narrower than a pixel it fades out, so
    // distant arms don't show aliased dark scribbles.
    let vein_fade = 1.0 - smoothstep(0.15, 0.35, length(fwidth(vein_p)));
    let vein = smoothstep(0.86, 0.97, max(ridge, ridge2 * 0.96)) * in.mask.z * in.activation
        * mat.detail.z * vein_fade;

    // Fade detail where the pattern gets finer than a pixel.
    let fade = saturate(1.5 - fwidth(fold_phase) * 2.0);
    let depth = mat.detail.x * fade;
    let height = depth * (compress * 0.0045 * fold_shape + stretch * 0.0016 * pull)
        + vein * 0.0012 * fade;

    // Textures and every derivative up front, in uniform control flow (before the cutout);
    // the flags pick which samples are used, so the default surface shades as untextured.
    let uv = in.uv * surf.extra.xy;
    let base_tex = textureSample(base_color_texture, surface_sampler, uv);
    let normal_tex = textureSample(normal_texture, surface_sampler, uv);
    let mr_tex = textureSample(metallic_roughness_texture, surface_sampler, uv);
    let dp1 = dpdx(in.rel);
    let dp2 = dpdy(in.rel);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dh1 = dpdx(height);
    let dh2 = dpdy(height);
    // The bind-pose face normal, for the strands' cross-section.
    let bind_n = cross(dpdx(in.pattern.xyz), dpdy(in.pattern.xyz));

    var base = surf.base_color * in.color;
    if ((flags & SURFACE_BASE_COLOR) != 0u) {
        base *= base_tex;
    }
    // Alpha cutout (hair cards, lashes); the shadow pass cuts the same pixels.
    if (base.a < surf.params.w) {
        discard;
    }
    // Fur shells: keep the pixels inside a strand. One strand per grid cell, jittered, of
    // random length, tapering to its tip; the cross-section is measured across the strand
    // (in the skin's plane), so every cell the skin passes through can grow one.
    let shell_h = in.shell.x;
    let fur_amount = in.pattern.w;
    var strand = 1.0;
    if (shell_h > 0.0) {
        if (fur_amount < 0.05) {
            discard;
        }
        let q = in.pattern.xyz * mat.fur.y;
        let cell = floor(q);
        let r = vec3<f32>(hash13(cell), hash13(cell + 17.31), hash13(cell + 43.7));
        let f = fract(q) - 0.5 - (r - 0.5) * 0.6;
        let bn = bind_n / max(length(bind_n), 1e-12);
        let across = length(f - bn * dot(f, bn));
        let reach = 0.55 + 0.45 * r.x;  // this strand's length, of the coat's
        let t = shell_h / reach;
        if (t > 1.0 || across > 0.5 * pow(1.0 - t, 0.7)) {
            discard;
        }
        strand = t;
    }

    var n = normalize(in.normal);
    if ((flags & SURFACE_DOUBLE_SIDED) != 0u) {
        n = facing_normal(n, dp1, dp2, in.rel);
    }
    if ((flags & SURFACE_NORMAL) != 0u) {
        let tn = (normal_tex.xyz * 2.0 - 1.0) * vec3<f32>(surf.params.z, surf.params.z, 1.0);
        n = perturb_normal(n, dp1, dp2, duv1, duv2, tn);
    }
    // Folds, pull lines and veins perturb the (normal-mapped) normal.
    n = bump_normal(n, dp1, dp2, dh1, dh2);

    var albedo = mat.tint.rgb * base.rgb;
    // Creases are darker; stretched fabric thins and lightens a touch.
    albedo *= 1.0 - compress * (0.35 * (1.0 - fold_shape)) * fade;
    albedo *= 1.0 + stretch * 0.06 * pull;
    // Working muscles flush; veins show through the skin.
    albedo *= mix(vec3<f32>(1.0), vec3<f32>(1.07, 0.93, 0.9), saturate(in.activation * skin * 0.6));
    albedo = mix(albedo, mat.vein.rgb * albedo * 1.6, vein * 0.3);
    // Fur: dark at the roots (the skin, when shells cover it), lighter and tinted at the tips.
    if (mat.fur.x > 0.0 && fur_amount > 0.0) {
        let root = mat.fur_tip.w;
        if (shell_h > 0.0) {
            albedo *= mix(root, 1.0, strand) * mix(vec3<f32>(1.0), mat.fur_tip.rgb, strand * strand);
        } else if (in.shell.y > 0.0) {
            albedo *= mix(1.0, root, saturate(fur_amount * 2.0));
        }
    }

    var s = default_surface();
    s.albedo = saturate3(albedo);
    if ((flags & SURFACE_STANDARD) != 0u) {
        var roughness = surf.params.x;
        var metallic = surf.params.y;
        if ((flags & SURFACE_METALLIC_ROUGHNESS) != 0u) {
            roughness *= mr_tex.g;
            metallic *= mr_tex.b;
        }
        s.roughness = clamp(roughness, 0.045, 1.0);
        s.metallic = saturate(metallic);
    } else {
        s.metallic = mat.params.w;
        s.roughness = mix(mat.params.x, mat.params.y, cloth);
    }
    s.reflectance = surf.extra.w;
    s.normal = n;
    s.subsurface = max(surf.extra.z, mat.params.z * skin);
    s.occlusion = 1.0 - compress * 0.25;
    if (shell_h > 0.0) {
        // Light scatters through the coat; strands deep in it see less sky.
        s.subsurface = max(s.subsurface, 0.35);
        s.occlusion = mix(0.55, 1.0, strand);
    }
    s.emissive = mat.emissive.rgb;

    if (frame.counts.z == 1u) {
        return vec4<f32>(n * 0.5 + 0.5, 1.0);
    }
    var color = shade_surface(s, in.rel);
    // Sheen: soft retro-reflection at grazing angles (fabric fibers, short fur, peach fuzz).
    let v = normalize(-in.rel);
    let rim = pow(1.0 - saturate(dot(n, v)), 5.0) * mat.vein.w * in.mask.w;
    color += pre_expose(sky_ambient(n) * sky_visibility(in.rel, n)) * s.albedo * rim;
    color = apply_fog(color, in.rel);
    return vec4<f32>(color, 1.0);
}
