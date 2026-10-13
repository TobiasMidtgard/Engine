// ============================================================================
// Ribbons: trails behind moving effects and lightning beams. Composed as lib_types +
// lib_frame + particles_common + this. Vertices are built on the CPU each frame
// (`RibbonVertex` in gpu.rs); this shader gives each style its look across the ribbon.
// ============================================================================

struct RibbonIn {
    @location(0) pos: vec3<f32>,
    @location(1) dist: f32,
    @location(2) color: vec4<f32>,
    @location(3) across: f32,
    @location(4) style: f32,
    @location(5) additive: f32,
    @location(6) along: f32,
};

struct RibbonOut {
    @builtin(position) position: vec4<f32>,
    @location(0) radiance: vec3<f32>,
    @location(1) coverage: f32,
    @location(2) inscatter: vec3<f32>,
    @location(3) across: f32,
    @location(4) dist: f32,
    @location(5) along: f32,
    @location(6) @interpolate(flat) style: u32,
    @location(7) additive: f32,
};

@vertex
fn vs_ribbon(v: RibbonIn) -> RibbonOut {
    var out: RibbonOut;
    out.position = frame.view_proj * vec4<f32>(v.pos, 1.0);
    let fog = particle_fog(v.pos);
    out.radiance = pre_expose(v.color.rgb) * fog.through;
    out.inscatter = fog.inscatter;
    out.coverage = v.color.a;
    out.across = v.across;
    out.dist = v.dist;
    out.along = v.along;
    out.style = u32(v.style);
    out.additive = v.additive;
    return out;
}

@fragment
fn fs_ribbon(in: RibbonOut) -> @location(0) vec4<f32> {
    let x = in.across;
    var a = 0.0;
    var rgb = in.radiance;
    let t = frame.time.x;
    switch in.style {
        case 1u: { // energy: a bright core with flowing streaks
            let core = exp(-x * x * 10.0);
            let n = value_noise2(vec2<f32>(in.dist * 2.5 - t * 7.0, x * 2.0));
            let streaks = exp(-x * x * 2.5) * (0.25 + 0.75 * n);
            a = saturate(core + streaks * 0.6);
            rgb = mix(rgb, vec3<f32>(max(rgb.r, max(rgb.g, rgb.b))), core * 0.5) * (1.0 + core);
        }
        case 2u: { // lightning: a white-hot filament in a colored glow
            let core = exp(-x * x * 60.0);
            let glow = exp(-x * x * 4.0) * 0.35;
            a = saturate(core + glow);
            let white = vec3<f32>(max(rgb.r, max(rgb.g, rgb.b)));
            rgb = mix(rgb, white * 1.5, core / max(a, 1e-4));
        }
        case 3u: { // smoke: a soft, wispy band
            let n = fbm2(vec2<f32>(in.dist * 0.8 - t * 0.3, x * 1.5), 3);
            a = smoothstep(1.0, 0.1, abs(x)) * (0.4 + n * 0.9);
        }
        default: { // smooth
            a = smoothstep(1.0, 0.0, abs(x));
        }
    }
    a = saturate(a) * in.coverage;
    if (a <= 0.0005) { discard; }
    let opaque = 1.0 - in.additive;
    return vec4<f32>(rgb * a + in.inscatter * (a * opaque), a * opaque);
}
