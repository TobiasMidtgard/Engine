// Grass blades, color pass. Requires lib_types + lib_frame + grass_common.

struct GrassOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) ts: vec4<f32>,                         // t, side, height above the root (m), seed
    @location(3) albedo: vec3<f32>,                     // the blade's own color
    @location(4) state: vec4<f32>,                      // height factor, char, glow, green
    @location(5) @interpolate(flat) flags: u32,
};

fn blade_color(root: vec3<f32>, seed: f32, flags: u32, green: f32) -> vec3<f32> {
    let p = (root + frame.camera_world_frac.xyz).xz;
    let m = value_noise2(p * 0.11);
    let m2 = value_noise2(p * 0.37 + vec2<f32>(7.1, 3.3));
    let s2 = fract(seed * 7.13 + 0.31);
    let s3 = fract(seed * 13.7 + 0.77);
    // Late-summer meadow: straw and pale gold, olive patches, a few grey dead blades and the
    // odd green one.
    var c = mix(vec3<f32>(0.42, 0.30, 0.11), vec3<f32>(0.60, 0.47, 0.24), seed);
    c = mix(c, vec3<f32>(0.25, 0.23, 0.075), smoothstep(0.5, 0.8, m) * 0.75 * s2);
    c = mix(c, vec3<f32>(0.30, 0.27, 0.20), step(0.88, s3) * 0.7);
    c = mix(c, vec3<f32>(0.13, 0.17, 0.045), smoothstep(0.6, 0.9, m2) * 0.55 * (1.0 - s2));
    if ((flags & 1u) != 0u) {
        c = mix(vec3<f32>(0.075, 0.15, 0.025), vec3<f32>(0.16, 0.22, 0.05), seed);
    }
    // Regrowth comes up fresh green.
    c = mix(c, mix(vec3<f32>(0.07, 0.2, 0.025), vec3<f32>(0.12, 0.27, 0.04), s2), green);
    return c;
}

@vertex
fn vs_grass(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> GrassOut {
    let b = build_blade(instance, vertex, frame.wind.xyz, frame.time.x, frame.camera_world_frac.xyz);
    var out: GrassOut;
    out.clip = frame.view_proj * vec4<f32>(b.rel, 1.0);
    out.rel = b.rel;
    out.normal = b.normal;
    out.ts = vec4<f32>(b.t, b.side, b.rel.y - b.root.y, b.seed);
    out.albedo = blade_color(b.root, b.seed, b.flags, b.state.w);
    out.state = b.state;
    out.flags = b.flags;
    return out;
}

fn ember(kelvin: f32) -> vec3<f32> {
    let t = clamp(kelvin, 800.0, 3000.0) / 100.0;
    let g = clamp(0.39008157876 * log(t) - 0.63184144378, 0.0, 1.0);
    let c = pow(vec3<f32>(1.0, g, 0.0), vec3<f32>(2.2));
    return c / max(luminance(c), 1e-4);
}

@fragment
fn fs_grass(in: GrassOut) -> @location(0) vec4<f32> {
    let t = in.ts.x;
    let lush = (in.flags & 1u) != 0u;
    let charred = in.state.y;
    let glow = in.state.z;
    let green = in.state.w;
    var n = normalize(in.normal);
    let v = normalize(-in.rel);
    if (dot(n, v) < 0.0) { n = -n; }
    // Soften toward the sky: a field of blades shades like a rough surface, not like foil.
    n = normalize(mix(n, vec3<f32>(0.0, 1.0, 0.0), 0.3));

    // Along the blade: darker, greener base; bleached tips on dry blades; fine veins.
    var albedo = in.albedo * mix(0.45, 1.0, smoothstep(0.0, 0.45, t));
    if (!lush) {
        albedo = mix(albedo, vec3<f32>(0.66, 0.56, 0.36), smoothstep(0.55, 1.0, t) * 0.4 * (1.0 - green));
    }
    albedo *= 0.9 + 0.1 * sin(in.ts.y * 7.0 + in.ts.w * 30.0);
    // Burnt: black char with pale ash at the tips.
    let char_c = mix(vec3<f32>(0.018, 0.016, 0.014), vec3<f32>(0.16, 0.155, 0.15), smoothstep(0.55, 1.0, t) * 0.45);
    albedo = mix(albedo, char_c, charred);

    var s = default_surface();
    s.albedo = albedo;
    s.normal = n;
    s.roughness = mix(select(0.55, 0.66, lush), 0.95, charred);
    s.reflectance = select(0.45, 0.32, lush);
    s.subsurface = mix(select(0.4, 0.6, lush || green > 0.5), 0.0, charred);
    // Deep in the sward, little sky reaches the lower blades.
    let canopy = max(0.75 * in.state.x, 0.06);
    s.occlusion = mix(0.3, 1.0, smoothstep(0.0, canopy, in.ts.z));

    if (frame.counts.z == 1u) {
        return vec4<f32>(n * 0.5 + 0.5, 1.0);
    }
    var color = shade_surface(s, in.rel);
    // Lower blades are shaded by the ones around them in direct light too.
    color *= mix(0.55, 1.0, smoothstep(0.0, canopy, in.ts.z));
    // Burning and smoldering: the cut ends glow.
    let e = glow * smoothstep(0.45, 1.0, t) * (0.6 + 0.4 * sin(frame.time.x * 9.0 + in.ts.w * 50.0));
    color += pre_expose(ember(1050.0 + 250.0 * glow) * 2200.0 * e * e);
    return vec4<f32>(apply_fog(color, in.rel), 1.0);
}
