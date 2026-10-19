// ============================================================================
// Particle simulation (compute). Composed as lib_types + lib_frame + particles_common + this.
//
// `cs_update` advances every live particle of every visible emitter by the emitter's time
// step; `cs_spawn` then writes this frame's new particles into their ring slots and
// simulates each one for the part of the step after its birth. One workgroup handles one
// chunk of up to 256 slots. Particles that hit something, die or just fly on in `cs_update`
// can feed another layer (sub-emitters): they append events that `cs_sub_spawn` turns into
// new particles, claiming slots in that layer's ring with an atomic cursor.
// ============================================================================

struct SpawnJob {
    emitter: u32,
    base: u32,
    capacity: u32,
    ring_start: u32,
    count: u32,
    index: u32,
    run_count: u32,
    seed: u32,
    start: vec4<f32>, // emitter position at the start of the step, w step length (s)
    end: vec4<f32>,   // emitter position at the end of the step, w extra seconds (prewarm)
};

struct UpdateJob {
    emitter: u32,
    first: u32,
    count: u32,
    pad: u32,
};

struct SubEvent {
    pos: vec3<f32>,    // camera-relative
    emitter: u32,      // the layer that gets the particles
    vel: vec3<f32>,    // velocity they inherit
    count: u32,
    normal: vec3<f32>, // surface normal on a hit, zero otherwise
    seed: u32,
    back: vec3<f32>,   // where the particle was at the start of the step, from `pos` (trails)
    pad: u32,
};

struct SubEvents {
    count: atomic<u32>,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    items: array<SubEvent>,
};

struct Sim {
    shift: vec4<f32>,   // camera origin movement since last frame, w unused
    counts: vec4<u32>,  // update jobs, spawn jobs, frame index, most sub-emitter events
    depth: vec4<f32>,   // scene collision: enabled, near plane, surface thickness, unused
    // The world re-anchored since last frame: rotation columns (turn_x.w = 1 when set), the
    // old frame's velocity at the last camera origin and its angular velocity.
    turn_x: vec4<f32>,
    turn_y: vec4<f32>,
    turn_z: vec4<f32>,
    frame_velocity: vec4<f32>,
    frame_spin: vec4<f32>,
};

@group(1) @binding(0) var<storage, read_write> particles: array<Particle>;
@group(1) @binding(1) var<storage, read> emitters: array<Emitter>;
@group(1) @binding(2) var<storage, read> spawns: array<SpawnJob>;
@group(1) @binding(3) var<storage, read> updates: array<UpdateJob>;
@group(1) @binding(4) var<uniform> sim: Sim;
@group(1) @binding(5) var scene_depth: texture_depth_2d; // last frame's, reverse Z
@group(1) @binding(6) var<storage, read_write> events: SubEvents;
@group(1) @binding(7) var<storage, read_write> cursors: array<atomic<u32>>;

// Set by `simulate` when the particle hit the ground, water or a surface on screen hard enough
// to count (a spark resting on the ground keeps touching it without hitting it again).
var<private> hit: bool;
var<private> hit_normal: vec3<f32>;
const HIT_SPEED: f32 = 0.5; // m/s into the surface

// ---------------------------------------------------------------- random numbers
fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn rand(s: ptr<function, u32>) -> f32 {
    *s = pcg(*s);
    return f32(*s >> 8u) * (1.0 / 16777216.0);
}

fn random_unit(s: ptr<function, u32>) -> vec3<f32> {
    let z = rand(s) * 2.0 - 1.0;
    let phi = rand(s) * TAU;
    let r = sqrt(max(1.0 - z * z, 0.0));
    return vec3<f32>(r * cos(phi), z, r * sin(phi));
}

// Two unit vectors perpendicular to `n` (and to each other).
fn perpendiculars(n: vec3<f32>) -> mat2x3<f32> {
    let sign = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (sign + n.z);
    let b = n.x * n.y * a;
    return mat2x3<f32>(
        vec3<f32>(1.0 + sign * n.x * n.x * a, sign * b, -sign * n.x),
        vec3<f32>(b, sign + n.y * n.y * a, -n.y),
    );
}

// A random direction within the cone around `axis` whose half-angle has cosine `cos_max`.
fn random_cone(axis: vec3<f32>, cos_max: f32, s: ptr<function, u32>) -> vec3<f32> {
    let cos_t = mix(1.0, cos_max, rand(s));
    let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
    let phi = rand(s) * TAU;
    let p = perpendiculars(axis);
    return p[0] * (sin_t * cos(phi)) + p[1] * (sin_t * sin(phi)) + axis * cos_t;
}

// ---------------------------------------------------------------- motion
// A divergence-free swirl field (each component depends only on the other two axes).
fn swirl(q: vec3<f32>, t: f32) -> vec3<f32> {
    let a = vec3<f32>(
        sin(q.y * 1.31 + t * 0.9) + cos(q.z * 1.73 - t * 0.7),
        sin(q.z * 1.13 + t * 1.1) + cos(q.x * 1.57 + t * 0.6),
        sin(q.x * 1.21 - t * 0.8) + cos(q.y * 1.91 + t * 1.3),
    );
    let r = q * 2.37 + vec3<f32>(3.1, 7.7, 1.3);
    let b = vec3<f32>(
        sin(r.y * 1.07 - t * 1.7) + cos(r.z * 1.29 + t * 1.2),
        sin(r.z * 1.41 + t * 1.5) + cos(r.x * 1.17 - t * 1.9),
        sin(r.x * 1.37 + t * 1.3) + cos(r.y * 1.23 - t * 1.4),
    );
    return (a + b * 0.5) * 0.35;
}

// Camera-relative position of the scene surface seen at pixel `px` of last frame's depth.
fn scene_point(px: vec2<i32>, size: vec2<f32>) -> vec3<f32> {
    let d = textureLoad(scene_depth, clamp(px, vec2<i32>(0), vec2<i32>(size) - 1), 0);
    let uv = (vec2<f32>(px) + 0.5) / size;
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, max(d, 1e-7), 1.0);
    let p = frame.inv_view_proj * ndc;
    return p.xyz / p.w;
}

// Collide with what the camera saw last frame: sparks bounce off walls and props, debris
// lands on them. Returns the surface normal, or zero when the particle is not inside a surface.
fn scene_hit(rel: vec3<f32>, speed: f32, dt: f32) -> vec3<f32> {
    // Last frame's depth was rendered around last frame's camera origin.
    let clip = frame.prev_view_proj * vec4<f32>(rel - sim.shift.xyz, 1.0);
    if (clip.w <= 1e-4) { return vec3<f32>(0.0); }
    let ndc = clip.xyz / clip.w;
    if (abs(ndc.x) >= 1.0 || abs(ndc.y) >= 1.0 || ndc.z <= 0.0) { return vec3<f32>(0.0); }
    let size = vec2<f32>(textureDimensions(scene_depth));
    let px = vec2<i32>((ndc.xy * vec2<f32>(0.5, -0.5) + 0.5) * size);
    let d = textureLoad(scene_depth, px, 0);
    if (d <= 0.0) { return vec3<f32>(0.0); } // sky
    let near = sim.depth.y;
    let behind = near / ndc.z - near / d;
    if (behind < 0.0 || behind > sim.depth.z + speed * dt * 1.5) { return vec3<f32>(0.0); }
    // The surface's normal from neighboring depths, facing the camera.
    let c = scene_point(px, size);
    let dx = scene_point(px + vec2<i32>(2, 0), size) - scene_point(px - vec2<i32>(2, 0), size);
    let dy = scene_point(px + vec2<i32>(0, 2), size) - scene_point(px - vec2<i32>(0, 2), size);
    var n = cross(dy, dx);
    let len = length(n);
    if (len < 1e-8) { return vec3<f32>(0.0); }
    n /= len;
    if (dot(n, c) > 0.0) { n = -n; }
    return n;
}

fn simulate(p: ptr<function, Particle>, e: Emitter, dt: f32) {
    if (dt <= 0.0) { return; }
    var q = *p;
    q.age += dt;
    let rel = particle_rel(q, e);
    var acc = e.forces.xyz;
    if (e.wind.w != 0.0) {
        acc += swirl(tiling_pos(rel) / e.motion.x, frame.time.x) * e.wind.w;
    }
    if (e.motion.y != 0.0 || e.motion.z != 0.0) {
        let axis = quat_rotate(e.rot, vec3<f32>(0.0, 1.0, 0.0));
        let d = rel - e.origin.xyz;
        let radial = d - axis * dot(d, axis);
        let r = length(radial);
        if (r > 1e-4) {
            let rn = radial / r;
            acc += cross(axis, rn) * e.motion.y;
            acc -= rn * (e.motion.z * saturate(r / 0.5));
        }
    }
    // Drag pulls the particle toward the air's velocity.
    let k = 1.0 - exp(-e.forces.w * dt);
    q.vel = mix(q.vel, e.wind.xyz, k);
    q.vel += acc * dt;
    q.pos += q.vel * dt;

    // The ground or water plane under the emitter.
    let mode = u32(e.collide.x);
    if (mode != 0u && e.collide.w > 0.5) {
        let n = e.plane.xyz;
        let dist = dot(n, particle_rel(q, e)) + e.plane.w;
        let vn = dot(q.vel, n);
        if (dist < 0.0 && vn < 0.0) {
            if (-vn > HIT_SPEED) {
                hit = true;
                hit_normal = n;
            }
            if (mode == 2u) {
                q.age = q.life;
            } else if (mode == 3u) {
                q.pos -= n * dist;
                q.vel = vec3<f32>(0.0);
                q.spin = 0.0;
            } else {
                q.pos -= n * dist;
                let vt = q.vel - n * vn;
                q.vel = vt * (1.0 - e.collide.z) - n * (vn * e.collide.y);
                q.spin *= 0.6;
            }
        }
    }
    // Walls, props and characters on screen. Newborn particles get a moment to leave the
    // surface they spawned on (sparks off an anvil, a jet out of its spout).
    if (mode != 0u && sim.depth.x > 0.5 && q.age > 0.06) {
        let speed = length(q.vel);
        let n = scene_hit(particle_rel(q, e), speed, dt);
        let vn = dot(q.vel, n);
        if (vn < -0.1 * speed) {
            if (-vn > HIT_SPEED) {
                hit = true;
                hit_normal = n;
            }
            q.pos -= q.vel * dt; // back to where it was, in front of the surface
            if (mode == 2u) {
                q.age = q.life;
            } else if (mode == 3u) {
                q.vel = vec3<f32>(0.0);
                q.spin = 0.0;
            } else {
                let vt = q.vel - n * vn;
                q.vel = vt * (1.0 - e.collide.z) - n * (vn * e.collide.y);
                q.spin *= 0.6;
            }
        }
    }
    q.rot += q.spin * dt;
    *p = q;
}

// ---------------------------------------------------------------- lighting
// Scene light reaching a particle, per unit albedo (cd/m^2). Translucent particles (smoke)
// scatter sunlight forward and glow at their edges against the sun; opaque ones (droplets,
// chips) act like small diffuse spheres.
fn particle_light(rel: vec3<f32>, translucency: f32) -> vec3<f32> {
    var light = vec3<f32>(0.0);
    let l = frame.sun_dir.xyz;
    let sun = frame.sun_camera.rgb;
    if (dot(sun, sun) > 0.0) {
        let v = rel / max(length(rel), 1e-4);
        let shadow = shadow_factor(rel, l);
        // A smoke puff is thick enough to scatter light many times over, so with the sun
        // behind the viewer it still glows (as a cloud does) rather than going as dark as one
        // bounce of the forward-peaked phase would leave it. Thinner sprites keep the one bounce.
        let multiple = 0.15 * INV_PI * step(0.99, translucency);
        let phase = mix(0.5 * INV_PI, max(phase_mie(dot(v, l), 0.55), multiple), translucency);
        light += sun * (shadow * phase);
    }
    let up = vec3<f32>(0.0, 1.0, 0.0);
    let fill = frame.ambient.rgb;
    let sky = sky_ambient(up) * 0.75 + sky_ambient(-up) * 0.25;
    let vis = sky_map_visibility(rel, up) * sky_volume_factor(rel);
    light += (sky - fill) * vis + fill;
    let iso = mix(0.5 * INV_PI, 0.25 * INV_PI, translucency);
    let count = min(frame.counts.x, 32u);
    for (var i = 0u; i < count; i++) {
        let pl = point_lights[i];
        let d = pl.position_range.xyz - rel;
        let d2 = dot(d, d);
        let range = pl.position_range.w;
        if (d2 > range * range) { continue; }
        // Particles are soft volumes, not surfaces: keep them from blowing out right next to a
        // light (an effect's own light sits in the middle of its smoke and mist).
        let falloff = sq(saturate(1.0 - sq(d2 / (range * range)))) / max(d2, sq(max(pl.color_radius.w, 0.75)));
        light += pl.color_radius.rgb * (falloff * iso);
    }
    return light;
}

// A new particle of emitter `e`, turned by `rot` (local -> world): its offset from the spawn
// point in `pos` and its birth velocity, without the emitter's own motion.
fn born(e: Emitter, rot: vec4<f32>, s: ptr<function, u32>) -> Particle {
    var lp = vec3<f32>(0.0);
    var ld = e.direction.xyz;
    let a = e.shape.y;
    let b = e.shape.z;
    let c = e.shape.w;
    let spread = e.offset.w;
    switch u32(e.shape.x) {
        case 1u: { // sphere
            let u = random_unit(s);
            let r = select(a * pow(rand(s), 1.0 / 3.0), a, b > 0.5);
            lp = u * r;
            ld = u;
        }
        case 2u: { // hemisphere
            var u = random_unit(s);
            u.y = abs(u.y);
            lp = u * (a * pow(rand(s), 1.0 / 3.0));
            ld = u;
        }
        case 3u: { // cone
            let r = a * sqrt(rand(s));
            let phi = rand(s) * TAU;
            let pp = perpendiculars(e.direction.xyz);
            lp = pp[0] * (r * cos(phi)) + pp[1] * (r * sin(phi));
            ld = random_cone(e.direction.xyz, b, s);
        }
        case 4u: { // ring
            let phi = rand(s) * TAU;
            let radial = vec3<f32>(cos(phi), 0.0, sin(phi));
            lp = radial * (a + (rand(s) - 0.5) * b);
            ld = radial;
        }
        case 5u: { // disc
            let phi = rand(s) * TAU;
            let radial = vec3<f32>(cos(phi), 0.0, sin(phi));
            lp = radial * (a * sqrt(rand(s)));
            ld = radial;
        }
        case 6u: { // box
            lp = (vec3<f32>(rand(s), rand(s), rand(s)) * 2.0 - 1.0) * vec3<f32>(a, b, c);
            ld = random_cone(e.direction.xyz, spread, s);
        }
        case 7u: { // line
            lp = vec3<f32>(0.0, (rand(s) - 0.5) * a, 0.0);
            ld = random_cone(e.direction.xyz, spread, s);
        }
        default: { // point
            ld = random_cone(e.direction.xyz, spread, s);
        }
    }
    let wo = quat_rotate(rot, lp + e.offset.xyz);
    let wd = quat_rotate(rot, ld);
    var vel = wd * mix(e.speed_life.x, e.speed_life.y, rand(s)) + quat_rotate(rot, e.extra_vel.xyz);
    if (e.motion.w != 0.0) {
        let axis = quat_rotate(rot, vec3<f32>(0.0, 1.0, 0.0));
        let radial = wo - axis * dot(wo, axis);
        let rl = length(radial);
        if (rl > 1e-4) {
            vel += cross(axis, radial / rl) * e.motion.w;
        }
    }

    var p: Particle;
    p.pos = wo;
    p.vel = vel;
    p.age = 0.0;
    p.life = mix(e.speed_life.z, e.speed_life.w, rand(s));
    p.size = mix(e.size_rot.x, e.size_rot.y, rand(s));
    p.rot = mix(e.size_rot.z, e.size_rot.w, rand(s));
    p.spin = mix(e.spin_look.x, e.spin_look.y, rand(s));
    p.seed = rand(s);
    p.emitter = 0u;
    p.light = vec3<f32>(0.0);
    return p;
}

// The rotation taking +Y to the unit vector `n`.
fn quat_from_up(n: vec3<f32>) -> vec4<f32> {
    if (n.y < -0.9999) { return vec4<f32>(1.0, 0.0, 0.0, 0.0); }
    return normalize(vec4<f32>(n.z, 0.0, -n.x, 1.0 + n.y));
}

fn job_index(wid: vec3<u32>, nwg: vec3<u32>) -> u32 {
    return wid.y * nwg.x + wid.x;
}

// ---------------------------------------------------------------- kernels
@compute @workgroup_size(256)
fn cs_update(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(num_workgroups) nwg: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let j = job_index(wid, nwg);
    if (j >= sim.counts.x) { return; }
    let job = updates[j];
    if (li >= job.count) { return; }
    let slot = job.first + li;
    var p = particles[slot];
    if (p.age >= p.life) { return; }
    let e = emitters[job.emitter];
    if (!is_local(e)) {
        if (sim.turn_x.w > 0.5) {
            // Into the new frame: turned, and carried along with the old frame's motion.
            let r = mat3x3<f32>(sim.turn_x.xyz, sim.turn_y.xyz, sim.turn_z.xyz);
            let turned = r * p.pos;
            p.vel = r * p.vel + sim.frame_velocity.xyz + cross(sim.frame_spin.xyz, turned);
            p.pos = turned;
        }
        p.pos += sim.shift.xyz;
    }
    p.emitter = job.emitter;
    hit = false;
    let start = particle_rel(p, e);
    simulate(&p, e, e.origin.w);
    if (e.shade.y > 0.0 && ((slot + sim.counts.z) & 3u) == 0u && p.age < p.life) {
        p.light = particle_light(particle_rel(p, e), e.look.y);
    }
    particles[slot] = p;

    // Sub-emitters: hits, deaths and flight feed other layers.
    let end = particle_rel(p, e);
    var seed = pcg(slot * 747796405u ^ pcg(sim.counts.z + 0x51ed27u));
    let normal = select(vec3<f32>(0.0), hit_normal, hit);
    let none = vec3<f32>(0.0);
    if (hit && e.sub_target.x != 0u) {
        push_event(e.sub_target.x, u32(e.sub_count.x), e.sub_chance.x, e.sub_keep.x, end, p.vel, normal, none, seed);
    }
    if (p.age >= p.life && e.sub_target.y != 0u) {
        push_event(e.sub_target.y, u32(e.sub_count.y), e.sub_chance.y, e.sub_keep.y, end, p.vel, normal, none, pcg(seed ^ 1u));
    }
    if (e.sub_target.z != 0u) {
        // `count` a second: whole ones, and the rest by chance, spread along the step's path.
        let n = u32(e.sub_count.z * e.origin.w + rand(&seed));
        push_event(e.sub_target.z, n, e.sub_chance.z, e.sub_keep.z, end, p.vel, none, start - end, pcg(seed ^ 2u));
    }
}

// Ask layer `fed` - 1 for `count` new particles at `pos`, with probability `chance`.
fn push_event(
    fed: u32, count: u32, chance: f32, keep: f32,
    pos: vec3<f32>, vel: vec3<f32>, normal: vec3<f32>, back: vec3<f32>, seed: u32,
) {
    var s = seed;
    if (count == 0u || rand(&s) >= chance) { return; }
    let i = atomicAdd(&events.count, 1u);
    if (i >= min(sim.counts.w, arrayLength(&events.items))) { return; }
    events.items[i] = SubEvent(pos, fed - 1u, vel * keep, count, normal, pcg(s), back, 0u);
}

@compute @workgroup_size(256)
fn cs_spawn(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(num_workgroups) nwg: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let j = job_index(wid, nwg);
    if (j >= sim.counts.y) { return; }
    let job = spawns[j];
    if (li >= job.count) { return; }
    let k = job.index + li;
    let slot = job.base + (job.ring_start + li) % max(job.capacity, 1u);
    let e = emitters[job.emitter];
    var s = pcg(job.seed ^ pcg(k * 2654435769u + 1013904223u));
    // Spread evenly over the step so moving emitters leave a continuous stream.
    let t = (f32(k) + rand(&s)) / f32(max(job.run_count, 1u));

    let local = is_local(e);
    var p = born(e, e.rot, &s);
    p.pos += select(mix(job.start.xyz, job.end.xyz, t), vec3<f32>(0.0), local);
    if (!local) {
        p.vel += e.velocity.xyz * e.velocity.w;
    }
    p.emitter = job.emitter;

    // Born part-way through the step (or earlier, when prewarming): catch up.
    let advance = job.end.w + (1.0 - t) * job.start.w;
    if (advance >= p.life) {
        p.age = p.life;
        particles[slot] = p;
        return;
    }
    if (advance > 0.0) {
        let n = clamp(u32(ceil(advance * 30.0)), 1u, 120u);
        let h = advance / f32(n);
        for (var i = 0u; i < n; i++) {
            simulate(&p, e, h);
        }
    }
    if (e.shade.y > 0.0) {
        p.light = particle_light(particle_rel(p, e), e.look.y);
    }
    particles[slot] = p;
}

// One thread per sub-emitter event: claim `count` slots in the fed layer's ring and fill them.
@compute @workgroup_size(64)
fn cs_sub_spawn(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(num_workgroups) nwg: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let i = job_index(wid, nwg) * 64u + li;
    let n = min(atomicLoad(&events.count), min(sim.counts.w, arrayLength(&events.items)));
    if (i >= n) { return; }
    let ev = events.items[i];
    let e = emitters[ev.emitter];
    let cap = e.ring.y;
    if (e.ring.z == 0xffffffffu || cap == 0u) { return; }
    let count = min(ev.count, 256u);
    let first = atomicAdd(&cursors[e.ring.z], count);
    // Out of the surface that was hit, or the effect's own way up.
    let rot = select(e.rot, quat_from_up(ev.normal), dot(ev.normal, ev.normal) > 0.5);
    let at = select(ev.pos, ev.pos - e.origin.xyz, is_local(e));
    for (var k = 0u; k < count; k++) {
        var s = pcg(ev.seed ^ pcg(k * 2654435769u + 1013904223u));
        var p = born(e, rot, &s);
        // Trail particles spread back along the path the particle flew this step.
        p.pos += at + ev.back * ((f32(k) + rand(&s)) / f32(count));
        p.vel += ev.vel;
        p.emitter = ev.emitter;
        if (e.shade.y > 0.0) {
            p.light = particle_light(particle_rel(p, e), e.look.y);
        }
        particles[e.ring.x + (first + k) % cap] = p;
    }
}
