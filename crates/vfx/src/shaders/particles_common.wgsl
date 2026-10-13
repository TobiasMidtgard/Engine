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
    sub_target: vec4<u32>, // layer fed on hit, death, flight (table index + 1, 0 none)
    sub_count: vec4<f32>,  // particles per hit, per death, per second of flight
    sub_chance: vec4<f32>, // chance per hit, death, trail
    sub_keep: vec4<f32>,   // velocity kept per hit, death, trail
    ring: vec4<u32>,       // first slot, size, GPU spawn cursor (0xffffffff none), unused
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

// The fog between the camera and a camera-relative point, as the scene gets it from `apply_fog`
// (volumetric fog with its light shafts and lamps, height fog beyond, aerial perspective far
// out): `through` is how much of the point's light reaches the camera, `inscatter` the light
// the fog adds in front of it (pre-exposed).
struct ParticleFog {
    through: vec3<f32>,
    inscatter: vec3<f32>,
};

fn particle_fog(rel: vec3<f32>) -> ParticleFog {
    // `apply_fog` is linear in the color, so a black and a white point split it.
    var fog: ParticleFog;
    fog.inscatter = apply_fog(vec3<f32>(0.0), rel);
    fog.through = max(apply_fog(vec3<f32>(1.0), rel) - fog.inscatter, vec3<f32>(0.0));
    return fog;
}
