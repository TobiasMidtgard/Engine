// Grass in the scene and the depth prepass. Requires lib_types + lib_frame + grass_common.

struct GrassOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) albedo: vec3<f32>,
    @location(3) along: f32,
};

// Collapses an invisible blade to a point outside the view.
const GRASS_HIDDEN: vec4<f32> = vec4<f32>(0.0, 0.0, -2.0, 1.0);

@vertex
fn vs_main(@builtin(instance_index) instance: u32, @builtin(vertex_index) vertex: u32) -> GrassOut {
    var out: GrassOut;
    let b = grass_blade(instance, vertex, frame.time.x, frame.wind.xyz, frame.camera_world_frac.xyz);
    out.clip = select(GRASS_HIDDEN, frame.view_proj * vec4<f32>(b.rel, 1.0), b.visible);
    out.rel = b.rel;
    out.normal = b.normal;
    out.albedo = b.albedo;
    out.along = b.along;
    return out;
}

fn grass_surface(in: GrassOut, front: bool) -> Surface {
    var s = default_surface();
    var n = normalize(in.normal);
    if (!front) { n = -n; }
    s.normal = n;
    // Lower parts of a meadow sit in the shade of the blades around them.
    s.albedo = in.albedo * mix(0.7, 1.0, in.along);
    s.occlusion = mix(0.3, 1.0, sqrt(in.along));
    s.roughness = 0.65;
    s.reflectance = 0.35;
    s.subsurface = 0.5;
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
    let color = apply_fog(shade_surface(s, in.rel), in.rel);
    return vec4<f32>(color, 1.0);
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
    let b = grass_blade(instance, vertex, frame.time.x, frame.wind.xyz, frame.camera_world_frac.xyz);
    out.clip = select(GRASS_HIDDEN, frame.view_proj * vec4<f32>(b.rel, 1.0), b.visible);
    out.normal = b.normal;
    // Where the blade was last frame: same root, last frame's wind.
    let p = grass_blade(instance, vertex, frame.time.x - frame.time.z, frame.wind.xyz, frame.camera_world_frac.xyz);
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
