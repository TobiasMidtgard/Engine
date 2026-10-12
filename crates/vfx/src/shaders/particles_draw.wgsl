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
@group(1) @binding(3) var scene_color: texture_2d<f32>; // HDR, pre-exposed (heat haze)

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
    @location(9) glint: vec3<f32>,     // sky mirrored by water droplets (pre-exposed)
    @location(10) sun: vec3<f32>,      // toward the sun in sprite space (x right, y up, z to eye); 0 if unlit
    @location(11) pixels: f32,         // sprite width on screen in pixels
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
    out.pixels = 2.0 * half.x * frame.proj[1][1] * frame.viewport.y * 0.5 / max(length(rel), 1e-3);
    if (e.shade.y > 0.0 && frame.sun_dir.y > -0.05) {
        // Smoke and dust read as volumes when their sun side is brighter (see `volume_shade`).
        let to_eye = -rel / max(length(rel), 1e-4);
        let l = frame.sun_dir.xyz;
        out.sun = vec3<f32>(dot(l, right), dot(l, up), dot(l, to_eye)) * saturate(frame.sun_dir.y * 8.0 + 0.4);
    }
    if (out.sprite == 4u) {
        // Clear water mirrors the sky beyond it, just above the horizon.
        let ahead = normalize(vec3<f32>(rel.x, 0.0, rel.z) + vec3<f32>(1e-4, 0.0, 0.0));
        let sky = sample_env(normalize(ahead + vec3<f32>(0.0, 0.4, 0.0)));
        out.glint = pre_expose(sky) * (1.0 - fog.a) * min(e.shade.y, 1.0);
    }
    return out;
}

// ---------------------------------------------------------------- sprites
// Directional shading for a puff: a sphere normal roughened by the puff's noise, lit from the
// sun's side. Averages about 1, so the overall light (sun, sky, lights) stays as computed.
fn volume_shade(c: vec2<f32>, wobble: vec2<f32>, sun: vec3<f32>) -> f32 {
    let z = sqrt(max(1.0 - dot(c, c), 0.0));
    let n = normalize(vec3<f32>(c + wobble, z * 0.8 + 0.2));
    let ndl = dot(n, sun);
    return 1.0 + 0.45 * ndl - 0.1 * length(sun);
}

// Temperature shading for flames: cool edges deepen and redden, hot cores whiten. Keeps the hue
// of the flame's own color (blue flames stay blue).
fn flame_shade(rgb: vec3<f32>, heat: f32) -> vec3<f32> {
    let lum = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let deep = max(vec3<f32>(lum) + (rgb - vec3<f32>(lum)) * 1.5, vec3<f32>(0.0)) * 0.35;
    let hot = mix(rgb, vec3<f32>(lum), 0.3) * 1.35;
    let cool = mix(deep, rgb, smoothstep(0.0, 0.55, heat));
    return mix(cool, hot, smoothstep(0.55, 1.0, heat));
}

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

// Heat haze: shift the scene behind the sprite by a rippling offset. Written as the difference
// from the unshifted scene with zero alpha, so particles already drawn behind stay visible.
fn haze(in: VsOut, c: vec2<f32>, r: f32) -> vec4<f32> {
    let fall = smoothstep(1.0, 0.25, r) * in.coverage;
    if (fall <= 0.002) { discard; }
    let flow = vec2<f32>(in.seed * 13.0, in.seed * 7.0 - in.age * 5.0);
    let n = vec2<f32>(value_noise2(c * 3.0 + flow), value_noise2(c * 3.0 + flow + vec2<f32>(5.3, 2.1))) - 0.5;
    // About 3% of the sprite's width at full strength, at least half a pixel to be seen.
    let shift = n * fall * max(in.pixels * 0.06, 1.0);
    let size = vec2<i32>(textureDimensions(scene_color));
    let here = vec2<i32>(in.position.xy);
    let there = clamp(vec2<i32>(in.position.xy + shift), vec2<i32>(0), size - 1);
    // Only pull in what is behind the haze, never something in front of it (reverse Z).
    if (textureLoad(scene_depth, there, 0) > in.position.z) { discard; }
    let delta = textureLoad(scene_color, there, 0).rgb - textureLoad(scene_color, here, 0).rgb;
    return vec4<f32>(delta, 0.0);
}

@fragment
fn fs_particle(in: VsOut) -> @location(0) vec4<f32> {
    let c = in.uv * 2.0 - 1.0;
    let r = length(c);
    var a = 0.0;
    var rgb = in.radiance;
    switch in.sprite {
        case 1u: { // spark: a white-hot core in a glow of deeper color, brightest at the head
            a = exp(-(c.x * c.x * 6.0)) * smoothstep(1.0, 0.6, abs(c.y)) * mix(0.5, 1.0, saturate(c.y * 0.5 + 0.5));
            let lum = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
            let deep = max(vec3<f32>(lum) + (rgb - vec3<f32>(lum)) * 1.5, vec3<f32>(0.0)) * 0.6;
            rgb = mix(deep, rgb * 2.0, exp(-c.x * c.x * 40.0));
        }
        case 2u: { // smoke: a billowing puff, brighter on its sun side
            let q = c * 1.7 + vec2<f32>(in.seed * 37.0, in.seed * 11.0) + vec2<f32>(0.0, in.age * 0.6);
            let n = fbm2(q, 3);
            let base = smoothstep(1.0, 0.15, r);
            a = saturate((base * (0.55 + n * 1.1) - 0.2) * 1.4);
            let wobble = vec2<f32>(fbm2(q + vec2<f32>(5.2, 1.3), 2), fbm2(q + vec2<f32>(1.7, 9.2), 2)) - 0.5;
            // Denser centers read a little darker, wispy edges brighter (fake self-shadowing).
            rgb *= mix(1.15, 0.8, saturate(a)) * volume_shade(c, wobble * 0.9, in.sun);
        }
        case 3u: { // flame: licking tongues; hot cores whiten, cool edges deepen
            let q = vec2<f32>(c.x * (1.2 + c.y * 0.5), c.y * 0.85 + 0.12);
            let shape = smoothstep(1.0, 0.2, length(q));
            let flow = vec2<f32>(in.seed * 17.0, in.seed * 5.0 - in.age * 2.5);
            let warp = vec2<f32>(fbm2(c * 1.3 + flow, 2), fbm2(c * 1.3 + flow + vec2<f32>(3.1, 7.7), 2)) - 0.5;
            let n = fbm2(vec2<f32>(c.x * 2.2, c.y * 1.5 - in.age * 4.5) + warp * 1.4 + in.seed * 23.0, 3);
            let core = exp(-(sq(c.x) * 5.0 + sq(c.y + 0.3) * 2.5));
            let heat = saturate(shape * (0.3 + n * 1.25) * 1.3 - 0.12 + core * 0.35 * shape);
            a = smoothstep(0.03, 0.4, heat);
            rgb = flame_shade(rgb, heat);
        }
        case 4u: { // droplet: a soft drop (a streak when stretched) mirroring the sky
            a = smoothstep(1.0, 0.45, r) * 0.9;
            let hl = exp(-dot(c - vec2<f32>(-0.2, 0.35), c - vec2<f32>(-0.2, 0.35)) * 12.0);
            rgb = rgb * (0.85 + hl * 0.6) + in.glint * (hl * 0.6 + 0.1);
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
            rgb *= volume_shade(c, vec2<f32>(n - 0.5) * 0.6, in.sun);
        }
        case 10u: { // heat haze: bends the scene behind it, adds nothing of its own
            return haze(in, c, r);
        }
        case 11u: { // fiery billow: glowing cracks in a dense puff, cooling to smoke
            let q = c * 2.2 + vec2<f32>(in.seed * 29.0, in.seed * 13.0) + vec2<f32>(0.0, in.age * 0.6);
            let n = fbm2(q, 4);
            let cracks = fbm2(q * 2.1 + vec2<f32>(7.3, 1.9), 2);
            let base = smoothstep(1.0, 0.25, r);
            a = saturate((base * (0.55 + n * 1.1) - 0.2) * 1.8);
            let heat = saturate(base * (0.15 + n * 1.2 + cracks * 0.5) * 1.3 - in.age * 0.9);
            rgb = flame_shade(in.radiance, heat) * smoothstep(0.08, 0.7, heat) * 1.4 + in.radiance * 0.012;
        }
        default: { // glow: a soft orb with a hot center that keeps its color
            a = exp(-r * r * 4.5) * smoothstep(1.0, 0.85, r);
            let core = exp(-r * r * 18.0);
            rgb = mix(rgb, vec3<f32>(max(rgb.r, max(rgb.g, rgb.b))), core * 0.3) * (1.0 + core * 1.2);
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
