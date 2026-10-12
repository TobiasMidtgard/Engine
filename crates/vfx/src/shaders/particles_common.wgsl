// ============================================================================
// Particles: shared types and helpers. Composed after lib_types + lib_frame.
// The structs match `crates/vfx/src/gpu.rs`.
// ============================================================================

struct Particle {
    pos: vec3<f32>,   // camera-relative (world-space emitters) or emitter-relative (local-space)
    age: f32,         // seconds; dead when age >= life (zeroed memory is dead)
    vel: vec3<f32>,
    life: f32,
    light: vec3<f32>, // scene light per unit albedo (cd/m^2), refreshed every few frames
    size: f32,        // diameter at birth (m)
    rot: f32,
    spin: f32,
    seed: f32,
    emitter: u32,     // index into this frame's emitter table
};

struct Emitter {
    origin: vec4<f32>,     // camera-relative position, w time step (s)
    prev: vec4<f32>,       // camera-relative position last frame, w 1 = local space
    rot: vec4<f32>,        // quaternion, local -> world
    velocity: vec4<f32>,   // emitter velocity, w inherited fraction
    forces: vec4<f32>,     // gravity + acceleration, w drag
    wind: vec4<f32>,       // air velocity, w turbulence (m/s^2)
    motion: vec4<f32>,     // turbulence scale, vortex, attract, orbit speed
    plane: vec4<f32>,      // collision plane normal, d
    collide: vec4<f32>,    // mode, restitution, friction, plane valid
    shape: vec4<f32>,      // code, a, b, c
    offset: vec4<f32>,     // local offset, w spread cosine
    direction: vec4<f32>,  // local direction
    extra_vel: vec4<f32>,  // local extra birth velocity
    speed_life: vec4<f32>, // speed min/max, life min/max
    size_rot: vec4<f32>,   // size min/max, rotation min/max
    spin_look: vec4<f32>,  // spin min/max, sprite, facing
    shade: vec4<f32>,      // emissive (cd/m^2), lit, additive, soft (m)
    look: vec4<f32>,       // stretch, translucency, size mult, opacity mult
    color: array<vec4<f32>, 16>,
    size_curve: array<vec4<f32>, 4>,
};

fn is_local(e: Emitter) -> bool { return e.prev.w > 0.5; }

// v rotated by the unit quaternion q.
fn quat_rotate(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    let t = 2.0 * cross(q.xyz, v);
    return v + q.w * t + cross(q.xyz, t);
}

// Color over life (16 samples, linear interpolation).
fn sample_color(e: Emitter, t: f32) -> vec4<f32> {
    let x = saturate(t) * 15.0;
    let i = u32(floor(x));
    let j = min(i + 1u, 15u);
    return mix(e.color[i], e.color[j], x - f32(i));
}

// Size multiplier over life (16 samples packed in 4 vec4s).
fn size_sample(e: Emitter, i: u32) -> f32 {
    return e.size_curve[i / 4u][i % 4u];
}

fn sample_size(e: Emitter, t: f32) -> f32 {
    let x = saturate(t) * 15.0;
    let i = u32(floor(x));
    let j = min(i + 1u, 15u);
    return mix(size_sample(e, i), size_sample(e, j), x - f32(i));
}

// Where a particle is relative to the camera.
fn particle_rel(p: Particle, e: Emitter) -> vec3<f32> {
    return select(p.pos, p.pos + e.origin.xyz, is_local(e));
}

// Height fog for a camera-relative point, like `apply_fog` without the far aerial perspective:
// rgb = fog radiance (pre-exposed), a = fog opacity.
fn particle_fog(rel: vec3<f32>) -> vec4<f32> {
    let dist = length(rel);
    let dir = rel / max(dist, 1e-4);
    let density = frame.fog.x;
    if (density <= 0.0) { return vec4<f32>(0.0); }
    let falloff = max(frame.fog.y, 1e-5);
    let h0 = fog_height(vec3(0.0)) - frame.fog.z;
    let h1 = fog_height(rel) - frame.fog.z;
    let e0 = exp(-falloff * max(h0, -100.0));
    let e1 = exp(-falloff * max(h1, -100.0));
    let dh = h1 - h0;
    var amount = density * dist * e0;
    if (abs(dh) > 0.01) {
        amount = density * dist * (e0 - e1) / (falloff * dh);
    }
    let fog_t = min(1.0 - exp(-amount), frame.fog.w);
    let sun_amt = pow(saturate(dot(dir, frame.sun_dir.xyz)), 8.0) * frame.fog_color.w;
    let horizon = sample_env(normalize(vec3<f32>(dir.x, 0.05, dir.z) + vec3(0.0001)));
    let outside = horizon + frame.sun_camera.rgb * sun_amt * INV_PI * 0.05;
    let fog_col = frame.fog_color.rgb * (outside + frame.ambient.rgb);
    return vec4<f32>(pre_expose(fog_col), fog_t);
}
