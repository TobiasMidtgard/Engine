// Standard PBR mesh feature. Requires lib_types + lib_frame.

struct Material {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    params: vec4<f32>,   // metallic, roughness, reflectance, alpha cutoff
    params2: vec4<f32>,  // subsurface, wind sway, unused, unused
    flags: vec4<u32>,    // x: bits 0-3 alpha mode, 4 unlit, 5 double sided, 6 receives wetness
};

@group(1) @binding(0) var<storage, read> materials: array<Material>;

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

@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let mat = materials[in.material];
    let flags = mat.flags.x;
    var base = mat.base_color * in.color;
    if ((flags & 15u) == 1u && base.a < mat.params.w) {
        discard;
    }
    var n = normalize(in.normal);
    if (!front) { n = -n; }

    var s = default_surface();
    s.albedo = base.rgb;
    s.metallic = mat.params.x;
    s.roughness = mat.params.y;
    s.reflectance = mat.params.z;
    s.normal = n;
    s.emissive = mat.emissive.rgb;
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
