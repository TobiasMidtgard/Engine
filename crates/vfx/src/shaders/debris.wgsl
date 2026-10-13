// ============================================================================
// Debris: the depth prepass and the opaque scene pass. Composed as lib_types + lib_frame +
// particles_common + debris_common + this.
// ============================================================================

struct DebrisOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) @interpolate(flat) kind: u32,
    @location(4) emissive: f32,
    @location(5) motion_now: vec4<f32>,
    @location(6) motion_prev: vec4<f32>,
};

@vertex
fn vs_debris(@builtin(vertex_index) vi: u32, @builtin(instance_index) slot: u32) -> DebrisOut {
    var out: DebrisOut;
    let piece = debris_piece(slot, vi);
    if (!piece.alive) {
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    out.clip = frame.view_proj * vec4<f32>(piece.rel, 1.0);
    out.rel = piece.rel;
    out.normal = piece.normal;
    out.color = piece.color;
    out.kind = piece.kind;
    out.emissive = piece.emissive;
    out.motion_now = motion_clip(piece.rel);
    out.motion_prev = prev_motion_clip(static_prev_rel(piece.prev_rel));
    return out;
}

// The face normal turned toward the camera, and whether this pixel is dissolved away.
fn debris_normal(in: DebrisOut) -> vec3<f32> {
    let n = normalize(in.normal);
    return select(n, -n, dot(n, in.rel) > 0.0);
}

fn debris_dissolved(in: DebrisOut) -> bool {
    return in.color.a < 0.999 && in.color.a <= debris_dither(in.clip.xy, frame.time.w % 64.0);
}

@fragment
fn fs_debris_prepass(in: DebrisOut) -> PrepassOutput {
    if (debris_dissolved(in)) { discard; }
    return prepass_output(debris_normal(in), debris_roughness(in.kind), in.motion_now, in.motion_prev);
}

@fragment
fn fs_debris(in: DebrisOut) -> @location(0) vec4<f32> {
    if (debris_dissolved(in)) { discard; }
    var s = default_surface();
    s.albedo = in.color.rgb;
    s.roughness = debris_roughness(in.kind);
    s.metallic = debris_metallic(in.kind);
    s.normal = debris_normal(in);
    s.emissive = in.color.rgb * in.emissive;
    var color = shade_surface(s, in.rel);
    color = apply_fog(color, in.rel);
    return vec4<f32>(color, 1.0);
}
