// ============================================================================
// Particle drawing. Composed as lib_types + lib_frame + particles_common + this.
//
// One instance per pool slot (triangle strip of 4 vertices); dead slots collapse to nothing.
// Sprites are procedural, so there are no textures to load. Output is premultiplied:
// `additive` 0 blends like a normal transparent surface, 1 only adds light.
// ============================================================================

@group(1) @binding(0) var<storage, read> particles: array<Particle>;
@group(1) @binding(1) var<storage, read> emitters: array<Emitter>;
@group(1) @binding(2) var scene_depth: texture_depth_2d;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) radiance: vec3<f32>,  // pre-exposed, already dimmed by fog
    @location(2) coverage: f32,
    @location(3) inscatter: vec3<f32>, // fog radiance in front of the particle (pre-exposed)
    @location(4) additive: f32,
    @location(5) @interpolate(flat) sprite: u32,
    @location(6) seed: f32,
    @location(7) soft: f32,
    @location(8) age: f32,             // 0..1 of its life
};

fn hidden() -> VsOut {
    var out: VsOut;
    out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    return out;
}

@vertex
fn vs_particle(@builtin(vertex_index) vi: u32, @builtin(instance_index) slot: u32) -> VsOut {
    let p = particles[slot];
    if (p.age >= p.life) { return hidden(); }
    let e = emitters[p.emitter];
    let t = p.age / p.life;
    var col = sample_color(e, t);
    col.a *= e.look.w;
    let size = p.size * sample_size(e, t) * e.look.z;
    if (col.a <= 0.002 || size <= 0.0) { return hidden(); }

    let rel = particle_rel(p, e);
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u)) * 2.0 - 1.0;
    let facing = u32(e.spin_look.w);
    let cr = cos(p.rot);
    let sr = sin(p.rot);
    var right: vec3<f32>;
    var up: vec3<f32>;
    var center = rel;
    var half = vec2<f32>(size * 0.5);
    let speed = length(p.vel);
    if (facing == 1u && speed > 1e-3) {
        // Stretched along the velocity, turned to face the camera; the head leads.
        let axis = p.vel / speed;
        let side = cross(axis, -rel);
        let sl = length(side);
        right = select(frame.inv_view[0].xyz, side / max(sl, 1e-6), sl > 1e-5 * length(rel));
        up = axis;
        let len = size + speed * e.look.x;
        half = vec2<f32>(size * 0.5, len * 0.5);
        center = rel - axis * ((len - size) * 0.5);
    } else if (facing == 2u) {
        // Flat, perpendicular to the emitter's up axis.
        let ax = quat_rotate(e.rot, vec3<f32>(1.0, 0.0, 0.0));
        let az = quat_rotate(e.rot, vec3<f32>(0.0, 0.0, 1.0));
        right = ax * cr + az * sr;
        up = az * cr - ax * sr;
    } else {
        let cx = frame.inv_view[0].xyz;
        let cy = frame.inv_view[1].xyz;
        right = cx * cr + cy * sr;
        up = cy * cr - cx * sr;
    }
    let pos = center + right * (corner.x * half.x) + up * (corner.y * half.y);

    var out: VsOut;
    out.position = frame.view_proj * vec4<f32>(pos, 1.0);
    out.uv = corner * 0.5 + 0.5;
    let light = p.light * e.shade.y + vec3<f32>(e.shade.x);
    let fog = particle_fog(rel);
    out.radiance = pre_expose(col.rgb * light) * (1.0 - fog.a);
    out.inscatter = fog.rgb * fog.a;
    // Fade particles that come very close to the camera instead of filling the screen.
    let near_fade = saturate((length(rel) - frame.exposure.z * 4.0) / max(size * 1.5, 0.1));
    out.coverage = col.a * near_fade;
    out.additive = e.shade.z;
    out.sprite = u32(e.spin_look.z);
    out.seed = p.seed;
    out.soft = e.shade.w;
    out.age = t;
    return out;
}

// ---------------------------------------------------------------- sprites
fn ring_line(r: f32, at: f32, width: f32) -> f32 {
    return exp(-sq((r - at) / width));
}

// A magic circle: concentric rings, a band of glyphs, a hexagram and a faint inner glow.
// Lines are drawn thick enough to read from 30 m away at a grazing angle.
fn rune(c: vec2<f32>, seed: f32) -> f32 {
    let r = length(c);
    if (r > 1.0) { return 0.0; }
    var v = ring_line(r, 0.95, 0.03) + ring_line(r, 0.8, 0.02) * 0.9 + ring_line(r, 0.47, 0.022) * 0.8
        + ring_line(r, 0.4, 0.014) * 0.6;
    // Glyph band between the two outer rings.
    let angle = atan2(c.y, c.x) / TAU + 0.5;
    let cells = 28.0;
    let cell = floor(angle * cells);
    let fx = fract(angle * cells);
    let band = (r - 0.83) / 0.1;
    if (band > 0.0 && band < 1.0 && fx > 0.1 && fx < 0.9) {
        let row = floor(band * 2.0);
        let col = floor(fx * 2.0);
        let h = hash12(vec2<f32>(cell * 7.0 + row * 2.0 + col, seed * 97.0 + 3.0));
        let fy = fract(band * 2.0);
        let fc = fract(fx * 2.0);
        let stroke = smoothstep(0.1, 0.25, fy) * smoothstep(0.9, 0.75, fy)
            * smoothstep(0.1, 0.25, fc) * smoothstep(0.9, 0.75, fc);
        v += step(0.4, h) * stroke;
    }
    // Hexagram: two triangles inscribed in the inner ring.
    if (r < 0.81) {
        for (var k = 0; k < 6; k++) {
            let a = f32(k) * (TAU / 6.0) + PI * 0.5;
            let n = vec2<f32>(cos(a), sin(a));
            v += ring_line(dot(c, n), 0.4, 0.018) * smoothstep(0.81, 0.76, r) * 0.9;
        }
    }
    // Dots on a middle ring, and a soft glow filling the circle.
    let dots = fract(angle * 12.0) - 0.5;
    v += exp(-(sq(dots * 2.2) + sq(r - 0.63) * 600.0)) * 1.2;
    v += smoothstep(0.95, 0.2, r) * 0.12;
    return saturate(v);
}

@fragment
fn fs_particle(in: VsOut) -> @location(0) vec4<f32> {
    let c = in.uv * 2.0 - 1.0;
    let r = length(c);
    var a = 0.0;
    var rgb = in.radiance;
    switch in.sprite {
        case 1u: { // spark: a hot streak, brightest at the head
            a = exp(-(c.x * c.x * 10.0)) * smoothstep(1.0, 0.6, abs(c.y)) * mix(0.5, 1.0, saturate(c.y * 0.5 + 0.5));
            rgb *= 1.0 + exp(-c.x * c.x * 40.0);
        }
        case 2u: { // smoke: a billowing puff
            let n = fbm2(c * 1.7 + vec2<f32>(in.seed * 37.0, in.seed * 11.0) + vec2<f32>(0.0, in.age * 0.6), 3);
            let base = smoothstep(1.0, 0.15, r);
            a = saturate((base * (0.55 + n * 1.1) - 0.2) * 1.4);
            // Denser centers read a little darker, wispy edges brighter (fake self-shadowing).
            rgb *= mix(1.15, 0.8, saturate(a));
        }
        case 3u: { // flame: a licking tongue with a hot core
            let q = vec2<f32>(c.x * (1.25 + c.y * 0.55), c.y * 0.85 + 0.1);
            let shape = smoothstep(1.0, 0.25, length(q));
            let n = fbm2(vec2<f32>(c.x * 2.3, c.y * 1.6 - in.age * 4.0) + in.seed * 23.0, 3);
            a = saturate(shape * (0.45 + n * 1.2) * 1.3 - 0.12);
            let core = exp(-(sq(c.x) * 6.0 + sq(c.y + 0.25) * 3.0));
            rgb = mix(rgb, vec3<f32>(max(rgb.r, max(rgb.g, rgb.b))), core * 0.45) * (1.0 + core * 1.5);
        }
        case 4u: { // droplet: clear water, denser at the rim, with a sky highlight
            a = smoothstep(1.0, 0.7, r) * mix(0.6, 1.0, smoothstep(0.3, 0.9, r));
            let hl = exp(-dot(c - vec2<f32>(-0.25, 0.4), c - vec2<f32>(-0.25, 0.4)) * 30.0);
            rgb = rgb * (0.8 + 0.3 * r) + rgb * hl * 2.0;
        }
        case 5u: { // ring
            a = ring_line(r, 0.78, 0.09) + ring_line(r, 0.78, 0.25) * 0.25;
            a *= smoothstep(1.0, 0.94, r);
        }
        case 6u: { // star: a four-point flare
            let rays = exp(-abs(c.x * c.y) * 60.0) * saturate(1.0 - r);
            let core = exp(-r * r * 14.0);
            a = saturate(rays * 1.2 + core);
            rgb *= 1.0 + core * 1.5;
        }
        case 7u: { // rune circle
            a = rune(c, floor(in.seed * 8.0));
            rgb *= 1.0 + a * 0.5;
        }
        case 8u: { // shard: an angular chip, shaded like a facet
            let d = max(abs(c.x) * 1.35 + c.y * 0.55, -c.y * 1.15 + abs(c.x) * 0.3);
            a = smoothstep(0.82, 0.74, d);
            rgb *= 0.65 + 0.5 * saturate(c.x * 0.4 + c.y * 0.5 + 0.5) * (0.7 + in.seed * 0.6);
        }
        case 9u: { // dust: a soft cloud
            let n = value_noise2(c * 2.5 + in.seed * 31.0);
            let base = smoothstep(1.0, 0.0, r);
            a = base * base * (0.5 + 0.5 * n);
        }
        default: { // glow: a soft orb with a white-hot center
            a = exp(-r * r * 4.5) * smoothstep(1.0, 0.85, r);
            let core = exp(-r * r * 18.0);
            rgb = mix(rgb, vec3<f32>(max(rgb.r, max(rgb.g, rgb.b))), core * 0.6) * (1.0 + core * 1.5);
        }
    }

    // Soft particles: fade where the sprite meets the scene behind it.
    var fade = 1.0;
    if (in.soft > 0.0 && frame.exposure.w < 0.5) {
        let scene = textureLoad(scene_depth, vec2<i32>(in.position.xy), 0);
        if (scene > 0.0) {
            let near = frame.exposure.z;
            let behind = near / scene - near / max(in.position.z, 1e-9);
            fade = saturate(behind / in.soft);
        }
    }
    a = a * in.coverage * fade;
    if (a <= 0.0005) { discard; }
    let opaque = 1.0 - in.additive;
    return vec4<f32>(rgb * a + in.inscatter * (a * opaque), a * opaque);
}
