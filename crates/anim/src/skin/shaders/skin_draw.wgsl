// Skinned characters. Requires lib_types + lib_frame.
//
// Vertices come from the compute pass (position + tension, normal + muscle activation) and a
// static buffer (uv, color, detail coordinates, masks). Detail is procedural, so no textures
// are needed:
// - cloth (mask.x): compressed regions bunch outward and fold into rings across the limb axis;
//   stretched regions show diagonal pull lines. Folds darken in their creases.
// - skin (mask.y): subsurface wrap, and a slight flush where muscles work.
// - veins (mask.z): raised branching lines along the limb that appear with muscle activation.
// - sheen (mask.w): soft grazing-angle glow for fabric and short fur.

struct SkinMaterial {
    tint: vec4<f32>,
    params: vec4<f32>,    // skin roughness, cloth roughness, subsurface, metallic
    detail: vec4<f32>,    // wrinkle depth, wrinkle frequency (folds/m), veins, bunch (m)
    vein: vec4<f32>,      // vein rgb, sheen
    emissive: vec4<f32>,  // rgb (cd/m^2)
};

@group(1) @binding(0) var<storage, read> materials: array<SkinMaterial>;

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
    @location(4) pattern: vec3<f32>,
    @location(5) axis: vec3<f32>,
    @location(6) mask: vec4<f32>,
    @location(7) tension: f32,
    @location(8) activation: f32,
    @location(9) @interpolate(flat) material: u32,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let mat = materials[in.extra.x];
    let tension = in.position_tension.w;
    var p = in.position_tension.xyz;
    let n = in.normal_activation.xyz;
    // Compressed cloth gathers outward (sleeves bunching at the elbow).
    p += n * (mat.detail.w * saturate(-tension * 4.0) * in.mask.x);
    let ph = vec4<f32>(p, 1.0);
    let rel = vec3<f32>(dot(in.m0, ph), dot(in.m1, ph), dot(in.m2, ph));
    var out: VertexOut;
    out.rel = rel;
    out.normal = normalize(mat3x3<f32>(in.n0.xyz, in.n1.xyz, in.n2.xyz) * n);
    out.uv = in.uv;
    out.color = in.color;
    out.pattern = in.pattern.xyz;
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

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let mat = materials[in.material];
    let cloth = in.mask.x;
    let skin = in.mask.y;
    let compress = saturate(-in.tension * 5.0) * cloth;
    let stretch = saturate(in.tension * 8.0) * cloth;

    // Coordinates along and around the limb axis (bind pose, so the pattern sticks to the body).
    let a = normalize(in.axis + vec3<f32>(1e-5, 0.0, 0.0));
    let along = dot(in.pattern, a);
    let side_ref = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(a.y) > 0.8);
    let b = normalize(cross(a, side_ref));
    let c = cross(a, b);
    let perp = in.pattern - a * along;
    let around = atan2(dot(perp, c), dot(perp, b));
    let warp = value_noise3(in.pattern * 7.0) * 2.0 - 1.0;
    let freq = mat.detail.y;

    // Compression folds: rings across the axis, wobbly and uneven like real fabric.
    let fold_phase = along * freq + warp * 0.9 + value_noise3(in.pattern * 23.0) * 0.4;
    let fold = sin(fold_phase * TAU);
    let fold_shape = 1.0 - abs(fold);  // sharp ridges
    // Stretch: diagonal pull lines.
    let pull_phase = (along * 0.6 + around * 0.12) * freq * 0.7 + warp * 0.5;
    let pull = 0.5 + 0.5 * sin(pull_phase * TAU);

    // Veins: branching ridges stretched along the axis, only on veined skin, only when working.
    let vein_p = in.pattern * 55.0 - a * (along * 55.0 * 0.8);
    let ridge = 1.0 - abs(value_noise3(vein_p) * 2.0 - 1.0);
    let ridge2 = 1.0 - abs(value_noise3(vein_p * 2.1 + 3.7) * 2.0 - 1.0);
    let vein = smoothstep(0.86, 0.97, max(ridge, ridge2 * 0.96)) * in.mask.z * in.activation
        * mat.detail.z;

    // Fade detail where the pattern gets finer than a pixel.
    let fade = saturate(1.5 - fwidth(fold_phase) * 2.0);
    let depth = mat.detail.x * fade;
    let height = depth * (compress * 0.0045 * fold_shape + stretch * 0.0016 * pull)
        + vein * 0.0012 * fade;

    let dp1 = dpdx(in.rel);
    let dp2 = dpdy(in.rel);
    let n0 = normalize(in.normal);
    let n = bump_normal(n0, dp1, dp2, dpdx(height), dpdy(height));

    var base = mat.tint * in.color;
    var albedo = base.rgb;
    // Creases are darker; stretched fabric thins and lightens a touch.
    albedo *= 1.0 - compress * (0.35 * (1.0 - fold_shape)) * fade;
    albedo *= 1.0 + stretch * 0.06 * pull;
    // Working muscles flush; veins show through the skin.
    albedo *= mix(vec3<f32>(1.0), vec3<f32>(1.07, 0.93, 0.9), saturate(in.activation * skin * 0.6));
    albedo = mix(albedo, mat.vein.rgb * albedo * 1.6, vein * 0.45);

    var s = default_surface();
    s.albedo = saturate3(albedo);
    s.metallic = mat.params.w;
    s.roughness = mix(mat.params.x, mat.params.y, cloth);
    s.reflectance = 0.5;
    s.normal = n;
    s.subsurface = mat.params.z * skin;
    s.occlusion = 1.0 - compress * 0.25;
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
