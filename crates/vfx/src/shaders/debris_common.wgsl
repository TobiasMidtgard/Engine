// ============================================================================
// Debris: solid pieces drawn for particle layers with a `debris` kind. Composed after
// lib_types + particles_common (and lib_frame for the scene passes).
//
// One instance per pool slot, `DEBRIS_VERTICES` vertices each (a flat-shaded triangle list,
// see `crates/vfx/src/debris.rs`); dead slots collapse to nothing. A piece tumbles about an
// axis of its own at the particle's rotation, from a random starting orientation.
// ============================================================================

struct DebrisVertex {
    position: vec4<f32>,
    normal: vec4<f32>,
};

const DEBRIS_VARIANTS: u32 = 4u;
const DEBRIS_VERTICES: u32 = 60u;

@group(1) @binding(0) var<storage, read> particles: array<Particle>;
@group(1) @binding(1) var<storage, read> emitters: array<Emitter>;
@group(1) @binding(2) var<storage, read> pieces: array<DebrisVertex>;

struct Piece {
    alive: bool,
    rel: vec3<f32>,      // camera-relative vertex position
    prev_rel: vec3<f32>, // where the vertex was a time step ago (same camera)
    normal: vec3<f32>,   // world face normal
    color: vec4<f32>,    // albedo, coverage
    kind: u32,           // index into `Debris::ALL`
    emissive: f32,       // cd/m^2 for color 1
};

fn debris_hash(seed: f32, k: f32) -> f32 {
    return hash12(vec2<f32>(seed * 917.3 + k * 13.1, k * 71.7 + 3.3));
}

fn quat_mul(a: vec4<f32>, b: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(a.w * b.xyz + b.w * a.xyz + cross(a.xyz, b.xyz), a.w * b.w - dot(a.xyz, b.xyz));
}

fn quat_axis_angle(axis: vec3<f32>, angle: f32) -> vec4<f32> {
    return vec4<f32>(axis * sin(angle * 0.5), cos(angle * 0.5));
}

fn debris_piece(slot: u32, vertex: u32) -> Piece {
    var out: Piece;
    out.alive = false;
    let p = particles[slot];
    if (p.age >= p.life) { return out; }
    let e = emitters[p.emitter];
    let t = p.age / p.life;
    var col = sample_color(e, t);
    col.a *= e.look.w;
    let size = p.size * sample_size(e, t) * e.look.z;
    if (col.a <= 0.002 || size <= 0.0 || e.direction.w < 0.5) { return out; }

    let kind = u32(e.direction.w) - 1u;
    let variant = min(u32(debris_hash(p.seed, 0.0) * f32(DEBRIS_VARIANTS)), DEBRIS_VARIANTS - 1u);
    let v = pieces[(kind * DEBRIS_VARIANTS + variant) * DEBRIS_VERTICES + vertex];
    if (dot(v.normal.xyz, v.normal.xyz) < 0.5) { return out; } // padding

    // A random starting orientation, tumbling about a random axis.
    let start = normalize(vec4<f32>(
        debris_hash(p.seed, 1.0), debris_hash(p.seed, 2.0),
        debris_hash(p.seed, 3.0), debris_hash(p.seed, 4.0)) - 0.5 + vec4<f32>(0.0, 0.0, 0.0, 1e-3));
    let axis = normalize(vec3<f32>(
        debris_hash(p.seed, 5.0), debris_hash(p.seed, 6.0), debris_hash(p.seed, 7.0)) - 0.5
        + vec3<f32>(1e-3, 0.0, 0.0));
    let q = quat_mul(quat_axis_angle(axis, p.rot), start);
    let dt = e.origin.w;
    let q_prev = quat_mul(quat_axis_angle(axis, p.rot - p.spin * dt), start);

    let local = v.position.xyz * size;
    var center = particle_rel(p, e);
    var prev_center = center - p.vel * dt;
    if (is_local(e)) { prev_center = p.pos - p.vel * dt + e.prev.xyz; }
    // The simulation keeps the particle's center on the ground plane; lift the piece so it
    // rests on it rather than half sunk in.
    if (u32(e.collide.x) != 0u && e.collide.w > 0.5) {
        let lift = max(size * 0.3 - (dot(e.plane.xyz, center) + e.plane.w), 0.0);
        center += e.plane.xyz * lift;
        prev_center += e.plane.xyz * lift;
    }
    out.rel = center + quat_rotate(q, local);
    out.prev_rel = prev_center + quat_rotate(q_prev, local);
    out.normal = quat_rotate(q, v.normal.xyz);
    // No two pieces quite the same shade.
    out.color = vec4<f32>(col.rgb * mix(0.72, 1.12, debris_hash(p.seed, 8.0)), col.a);
    out.kind = kind;
    out.emissive = e.shade.x;
    out.alive = true;
    return out;
}

// Pieces fade by dissolving: a pixel is drawn while the coverage beats its dither threshold.
fn debris_dither(pixel: vec2<f32>, offset: f32) -> f32 {
    let p = pixel + vec2<f32>(offset * 5.588238);
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// Perceptual roughness and metalness of each kind: rock, wood, ice/glass, metal.
fn debris_roughness(kind: u32) -> f32 {
    switch kind {
        case 0u: { return 0.85; }
        case 1u: { return 0.7; }
        case 2u: { return 0.1; }
        default: { return 0.4; }
    }
}

fn debris_metallic(kind: u32) -> f32 {
    return select(0.0, 0.9, kind == 3u);
}
