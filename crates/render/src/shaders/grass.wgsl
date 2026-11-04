// Grass in the scene and the depth prepass. Requires lib_types + lib_frame + grass_common.

struct GrassOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) albedo: vec3<f32>,
    @location(3) along: f32,
    @location(4) across: f32,
    @location(5) gloss: f32,
    @location(6) shade: f32,
    @location(7) glow: f32,
};

// The blades the cull pass (grass_cull.wgsl) kept this frame, grouped by segments. A group's
// draw starts at vertex 16 x its first record, so vertex index / 16 + instance is the record
// and vertex index % 16 the vertex along the blade (grass_shadow.wgsl reads them the same way).
@group(1) @binding(4) var<storage, read> grass_records: array<GrassRecord>;

fn grass_record(instance: u32, vertex: u32) -> BladeShape {
    return grass_unpack(grass_records[(vertex >> 4u) + instance]);
}

@vertex
fn vs_main(@builtin(instance_index) instance: u32, @builtin(vertex_index) vertex: u32) -> GrassOut {
    var out: GrassOut;
    let s = grass_record(instance, vertex);
    let b = grass_vertex(s, vertex & 15u, frame.time.x, s.tip);
    out.clip = frame.view_proj * vec4<f32>(b.rel, 1.0);
    out.rel = b.rel;
    out.normal = b.normal;
    out.albedo = b.albedo;
    out.along = b.along;
    out.across = b.across;
    out.gloss = b.gloss;
    out.shade = b.shade;
    out.glow = b.glow;
    return out;
}

fn grass_surface(in: GrassOut, front: bool) -> Surface {
    var s = default_surface();
    var n = normalize(in.normal);
    if (!front) { n = -n; }
    s.normal = n;
    // Lower parts of a meadow sit in the shade of the blades around them (`grass_blade`). The
    // midrib is a little paler and glossier than the edges.
    let mid = 1.0 - abs(in.across);
    s.albedo = in.albedo * (0.94 + 0.1 * mid);
    s.occlusion = in.shade;
    s.roughness = mix(0.7, 0.36, in.gloss * mid);
    s.reflectance = 0.35;
    s.subsurface = 0.5;
    // Wet: blades are waxy and shed water, so rain leaves a film and beads that gloss them
    // (smoother, a little more reflective) and darken them only slightly, unlike soil, which
    // soaks it up (Lagarde, "Water drop 2b", 2013: non-porous surfaces keep their albedo).
    let wet = saturate(surface_weather(in.rel).x);
    s.albedo *= 1.0 - 0.12 * wet;
    s.roughness = mix(s.roughness, 0.16, wet * 0.85);
    s.reflectance = mix(s.reflectance, 0.5, wet);
    return s;
}

@fragment
fn fs_main(in: GrassOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let s = grass_surface(in, front);
    // Debug views (RenderSettings.debug_view), as for meshes.
    switch frame.counts.z {
        case 1u: { return vec4<f32>(s.normal * 0.5 + 0.5, 1.0); }
        case 3u: { return vec4<f32>(vec3(sky_visibility(in.rel, s.normal)), 1.0); }
        case 4u: { return vec4<f32>(vec3(screen_ao(in.rel)), 1.0); }
        case 5u: {
            let r = screen_reflection(in.rel);
            return vec4<f32>(pre_expose(r.rgb) * r.a, 1.0);
        }
        case 6u: { return vec4<f32>(pre_expose(screen_gi(in.rel)), 1.0); }
        default: {}
    }
    let color = shade_surface(s, in.rel) + grass_translucency(s, in.rel, in.glow);
    return vec4<f32>(apply_fog(color, in.rel), 1.0);
}

// Sunlight through the blades. A blade is thin, so lit from behind it glows a deeper, yellower
// green, strongest looking toward the sun and toward the tips (the lower parts sit in the
// shade of the meadow).
fn grass_translucency(s: Surface, rel: vec3<f32>, glow: f32) -> vec3<f32> {
    let l = frame.sun_dir.xyz;
    let v = normalize(-rel);
    let through = saturate(-dot(s.normal, l));
    let toward_sun = pow(saturate(dot(v, -l)), 3.0);
    let amount = (through * 0.35 + toward_sun * 0.9) * glow;
    if (amount <= 0.0) { return vec3<f32>(0.0); }
    let lit_side = select(s.normal, -s.normal, dot(s.normal, l) < 0.0);
    let sun = frame.sun_color.rgb * sun_transmittance(rel) * cloud_shadow(rel) * shadow_factor(rel, lit_side);
    let tint = s.albedo * saturate3(s.albedo * 4.0);
    return pre_expose(sun * tint * amount * INV_PI);
}

struct GrassPrepassOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) motion_now: vec4<f32>,
    @location(2) motion_prev: vec4<f32>,
};

@vertex
fn vs_prepass(@builtin(instance_index) instance: u32, @builtin(vertex_index) vertex: u32) -> GrassPrepassOut {
    var out: GrassPrepassOut;
    let s = grass_record(instance, vertex);
    let b = grass_vertex(s, vertex & 15u, frame.time.x, s.tip);
    out.clip = frame.view_proj * vec4<f32>(b.rel, 1.0);
    out.normal = b.normal;
    // Where the blade was last frame: same root, last frame's wind.
    let p = grass_vertex(s, vertex & 15u, frame.time.x - frame.time.z, s.tip_prev);
    out.motion_now = motion_clip(b.rel);
    out.motion_prev = prev_motion_clip(static_prev_rel(p.rel));
    return out;
}

@fragment
fn fs_prepass(in: GrassPrepassOut, @builtin(front_facing) front: bool) -> PrepassOutput {
    var n = normalize(in.normal);
    if (!front) { n = -n; }
    return prepass_output(n, 0.65, in.motion_now, in.motion_prev);
}
