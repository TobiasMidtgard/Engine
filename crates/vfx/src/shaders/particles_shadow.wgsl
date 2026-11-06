// ============================================================================
// Smoke and dust casting into the sun's shadow cascades. Composed as lib_types +
// particles_common + debris_common (for the group 1 buffers) + this.
//
// Stochastic shadow maps (McGuire & Enderton 2011): a puff writes depth on a share of its
// pixels equal to its opacity, so the receivers' shadow filter averages it to the light the
// puff lets through, prod(1 - alpha). Research and plan: engine-design/particles-lighting-research.md §4.1.
// ============================================================================

struct ShadowView {
    view_proj: mat4x4<f32>,  // camera-relative -> light clip
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;

// The 4x4 Bayer matrix (0 8 2 10 / 12 4 14 6 / 3 11 1 9 / 15 7 13 5) by bit interleaving.
fn bayer4(q: vec2<u32>) -> u32 {
    let d = q.x ^ q.y;
    return ((d & 1u) << 3u) | ((q.y & 1u) << 2u) | (((d >> 1u) & 1u) << 1u) | ((q.y >> 1u) & 1u);
}

struct ShadowOut {
    @builtin(position) position: vec4<f32>,
    @location(0) corner: vec2<f32>,
    @location(1) coverage: f32,
    @location(2) seed: f32,
};

fn shadow_hash(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.1031);
    q += dot(q, q.zyx + 31.32);
    return fract((q.x + q.y) * q.z);
}

fn shadow_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = shadow_hash(vec3<f32>(i, 1.0));
    let b = shadow_hash(vec3<f32>(i + vec2<f32>(1.0, 0.0), 1.0));
    let c = shadow_hash(vec3<f32>(i + vec2<f32>(0.0, 1.0), 1.0));
    let d = shadow_hash(vec3<f32>(i + vec2<f32>(1.0, 1.0), 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

@vertex
fn vs_particle_shadow(@builtin(vertex_index) vi: u32, @builtin(instance_index) slot: u32) -> ShadowOut {
    var out: ShadowOut;
    out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    let p = particles[slot];
    if (p.age >= p.life) { return out; }
    let e = emitters[p.emitter];
    let t = p.age / p.life;
    let alpha = sample_color(e, t).a * e.look.w;
    let size = p.size * sample_size(e, t) * e.look.z;
    if (alpha <= 0.01 || size <= 0.0) { return out; }

    // A quad facing the sun, turning with the puff; a little smaller than the sprite, whose
    // alpha falls off toward the rim.
    let m = shadow_view.view_proj;
    let axis = normalize(vec3<f32>(m[0][2], m[1][2], m[2][2]));
    let helper = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(axis.y) > 0.9);
    let bx = normalize(cross(helper, axis));
    let by = cross(axis, bx);
    let cr = cos(p.rot);
    let sr = sin(p.rot);
    let right = bx * cr + by * sr;
    let up = by * cr - bx * sr;
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u)) * 2.0 - 1.0;
    let half = size * 0.4;
    let pos = particle_rel(p, e) + (right * corner.x + up * corner.y) * half;
    out.position = m * vec4<f32>(pos, 1.0);
    out.corner = corner;
    out.coverage = alpha;
    out.seed = p.seed;
    return out;
}

@fragment
fn fs_particle_shadow(in: ShadowOut) {
    let r = length(in.corner);
    let wisps = 0.55 + 0.9 * shadow_noise(in.corner * 1.7 + vec2<f32>(in.seed * 37.0, in.seed * 11.0));
    let alpha = in.coverage * (1.0 - smoothstep(0.15, 1.0, r)) * wisps;
    // An ordered (Bayer 4x4) threshold rather than white noise: the receivers' shadow filter
    // spans a few texels, so it averages an ordered pattern to a smooth grey where white
    // noise stays grainy. Each puff shifts the pattern by its seed, so overlapping puffs
    // cover different texels and their shadows stack.
    let frame = u32(fract(shadow_view.time.x * 7.31) * 512.0);
    let shift = vec2<u32>(u32(in.seed * 4096.0) + frame, u32(in.seed * 9973.0) + frame * 3u);
    let q = (vec2<u32>(in.position.xy) + shift) & vec2<u32>(3u);
    if ((f32(bayer4(q)) + 0.5) / 16.0 >= alpha) {
        discard;
    }
}
