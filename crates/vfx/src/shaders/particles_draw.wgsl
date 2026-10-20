// ============================================================================
// Particle drawing. Composed as lib_types + lib_frame + particles_common + this.
//
// One instance per pool slot (triangle strip of 4 vertices), in the order `cs_sort` left in
// `order` (far to near within each layer); dead slots collapse to nothing.
// Sprites are procedural, so there are no textures to load. Output is premultiplied:
// `additive` 0 blends like a normal transparent surface, 1 only adds light.
// ============================================================================

@group(1) @binding(0) var<storage, read> particles: array<Particle>;
@group(1) @binding(1) var<storage, read> emitters: array<Emitter>;
@group(1) @binding(2) var scene_depth: texture_depth_2d;
@group(1) @binding(3) var scene_color: texture_2d<f32>; // HDR, pre-exposed (heat haze)
@group(1) @binding(4) var<storage, read> order: array<u32>;

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
    @location(9) glint: vec3<f32>,     // sky mirrored by water droplets (pre-exposed); plumes: x the air's thickness
    @location(10) sun: vec3<f32>,      // toward the sun in sprite space (x right, y up, z to eye); 0 if unlit
    @location(11) pixels: f32,         // sprite width on screen in pixels
    // Crowns: the eye and this corner in the splash's own frame (units of its size; eye.w the
    // size in meters), and its rotation to the world.
    @location(12) eye: vec4<f32>,
    @location(13) at: vec3<f32>,
    @location(14) @interpolate(flat) quat: vec4<f32>,
};

// Narrowest a sprite is drawn, pixels.
const MIN_PIXELS: f32 = 1.5;

fn hidden() -> VsOut {
    var out: VsOut;
    out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    return out;
}

@vertex
fn vs_particle(@builtin(vertex_index) vi: u32, @builtin(instance_index) instance: u32) -> VsOut {
    let p = particles[order[instance]];
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
    } else if (facing == 3u) {
        // Standing on the particle along the emitter's direction, turned to face the camera.
        let axis = normalize(quat_rotate(e.rot, e.direction.xyz) + vec3<f32>(0.0, 1e-6, 0.0));
        let side = cross(axis, -rel);
        let sl = length(side);
        right = select(frame.inv_view[0].xyz, side / max(sl, 1e-6), sl > 1e-5 * length(rel));
        up = axis;
        half = vec2<f32>(size * e.look.x * 0.5, size * 0.5);
        center = rel + axis * (size * 0.5);
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
    let is_crown = u32(e.spin_look.z) == 14u;
    if (is_crown) {
        // A crown is traced inside a camera-facing square around all it can reach.
        right = frame.inv_view[0].xyz;
        up = frame.inv_view[1].xyz;
        center = rel + quat_rotate(e.rot, vec3<f32>(0.0, 0.4, 0.0)) * size;
        half = vec2<f32>(size * 0.75);
    }
    // Sprites thinner than MIN_PIXELS on screen grow to it and dim to match, so distant
    // sparks, stars and drops still show (and don't shimmer) with the same total light.
    let px_per_m = frame.proj[1][1] * frame.viewport.y * 0.5 / max(length(rel), 1e-3);
    let width_px = 2.0 * half.x * px_per_m;
    var dim = 1.0;
    if (width_px < MIN_PIXELS && width_px > 0.0 && !is_crown) {
        let grow = MIN_PIXELS / width_px;
        half.x *= grow;
        if ((facing == 1u && speed > 1e-3) || facing == 3u) {
            // A streak keeps its length; only its width grows.
            dim = 1.0 / grow;
        } else {
            half.y *= grow;
            dim = 1.0 / (grow * grow);
        }
    }
    let pos = center + right * (corner.x * half.x) + up * (corner.y * half.y);

    var out: VsOut;
    out.position = frame.view_proj * vec4<f32>(pos, 1.0);
    out.uv = corner * 0.5 + 0.5;
    let light = p.light * e.shade.y + vec3<f32>(e.shade.x);
    let fog = particle_fog(rel);
    out.radiance = pre_expose(col.rgb * light) * fog.through;
    out.inscatter = fog.inscatter;
    // Fade particles that come very close to the camera instead of filling the screen.
    let near_fade = saturate((length(rel) - frame.exposure.z * 4.0) / max(size * 1.5, 0.1));
    out.coverage = col.a * near_fade * dim;
    out.additive = e.shade.z;
    out.sprite = u32(e.spin_look.z);
    out.seed = p.seed;
    out.soft = e.shade.w;
    out.age = t;
    out.pixels = 2.0 * half.x * px_per_m;
    if (e.shade.y > 0.0 && frame.sun_dir.y > -0.05) {
        // Smoke and dust read as volumes when their sun side is brighter (see `volume_shade`).
        let to_eye = -rel / max(length(rel), 1e-4);
        let l = frame.sun_dir.xyz;
        out.sun = vec3<f32>(dot(l, right), dot(l, up), dot(l, to_eye)) * saturate(frame.sun_dir.y * 8.0 + 0.4);
    }
    if (out.sprite == 4u || out.sprite == 19u || out.sprite == 21u) {
        // Clear water mirrors the sky beyond it, just above the horizon.
        let ahead = normalize(vec3<f32>(rel.x, 0.0, rel.z) + vec3<f32>(1e-4, 0.0, 0.0));
        let sky = sample_env(normalize(ahead + vec3<f32>(0.0, 0.4, 0.0)));
        out.glint = pre_expose(sky) * fog.through * min(e.shade.y, 1.0);
    } else if (out.sprite == 13u) {
        out.glint = vec3<f32>(e.extra_vel.w, 0.0, 0.0);
    } else if (out.sprite == 18u) {
        // A fireball's soot is lit by the scene alone, not by its own glow.
        out.glint = pre_expose(p.light * e.shade.y) * fog.through;
    } else if (is_crown) {
        let inv = vec4<f32>(-e.rot.xyz, e.rot.w);
        out.eye = vec4<f32>(quat_rotate(inv, -rel) / size, size);
        out.at = quat_rotate(inv, pos - rel) / size;
        out.quat = e.rot;
        out.glint = fog.through;
    }
    return out;
}

// ---------------------------------------------------------------- splash crown
// A splash's crown, traced inside its sprite: a thin wall of water thrown up in a ring, flaring
// out as it rises, its rim drawn into fingers with a bead on each tip, tearing open and slumping
// back while the jet shoots up from the middle. Units are the sprite's size, y up out of the
// water, the splash at the origin.
struct CrownShape {
    h: f32,     // wall height
    r0: f32,    // radius at the water
    flare: f32, // how far the rim leans out past r0
    tear: f32,  // 0..1, how much of the wall has torn away
    jet: f32,   // jet height
    bead: f32,  // radius of the beads on the fingertips
};

const CROWN_FINGERS: f32 = 17.0;

fn crown_shape(t: f32) -> CrownShape {
    var s: CrownShape;
    let rise = 1.0 - sq(1.0 - saturate(t / 0.36));
    let slump = smoothstep(0.4, 1.0, t);
    s.h = 0.36 * rise * (1.0 - 0.85 * slump);
    s.r0 = 0.13 + 0.09 * saturate(t / 0.5);
    s.flare = 0.03 + 0.17 * rise;
    s.tear = smoothstep(0.28, 0.8, t);
    s.jet = 0.6 * smoothstep(0.3, 0.62, t) * (1.0 - smoothstep(0.72, 1.0, t));
    s.bead = 0.022 * (1.0 - smoothstep(0.42, 0.7, t));
    return s;
}

// Distance to the splash from `p`, and which part is nearest (0 the wall, 1 the jet).
fn crown_sdf(p: vec3<f32>, s: CrownShape, seed: f32) -> vec2<f32> {
    let rho = length(p.xz);
    let ang = atan2(p.z, p.x);
    // Which finger this is nearest, and how far across it (-0.5..0.5).
    let fi = ang / TAU * CROWN_FINGERS + seed * 7.0;
    let k = floor(fi + 0.5);
    let f = fi - k;
    let kk = k - CROWN_FINGERS * floor(k / CROWN_FINGERS);
    let jit = hash12(vec2<f32>(kk, seed * 13.0 + 1.0));
    let wave = value_noise2(vec2<f32>(cos(ang), sin(ang)) * 2.3 + seed * 17.0);
    let finger = pow(saturate(1.0 - abs(f) * 3.4), 2.0) * (0.4 + jit);
    let top = s.h * (0.84 + 0.16 * wave + 0.24 * finger);
    let u = saturate(p.y / max(top, 1e-3));
    let r = s.r0 + s.flare * pow(u, 1.6);
    let w = mix(0.016, 0.005, u) + 0.008 * finger * u;
    var d = max(abs(rho - r) - w, p.y - top);
    d = max(d, -p.y);
    if (s.bead > 0.001) {
        let ac = (k - seed * 7.0) / CROWN_FINGERS * TAU;
        let ht = s.h * (0.84 + 0.16 * wave + 0.24 * (0.4 + jit));
        let tip = vec3<f32>(cos(ac) * (s.r0 + s.flare), ht + s.bead * 0.5, sin(ac) * (s.r0 + s.flare));
        d = min(d, length(p - tip) - s.bead * (0.7 + 0.6 * jit));
    }
    var dj = 1e3;
    if (s.jet > 0.01) {
        // The jet: a column narrowing upward under a heavier drop about to break away.
        let wob = vec2<f32>(value_noise2(vec2<f32>(p.y * 6.0, seed * 3.0)), value_noise2(vec2<f32>(p.y * 6.0 + 4.0, seed * 3.0))) - 0.5;
        let q = p - vec3<f32>(wob.x, 0.0, wob.y) * 0.03 * p.y / max(s.jet, 1e-3);
        let y = clamp(q.y, 0.0, s.jet);
        let jr = mix(0.034, 0.016, y / max(s.jet, 1e-3));
        dj = length(vec2<f32>(length(q.xz), q.y - y)) - jr;
        dj = min(dj, length(q - vec3<f32>(0.0, s.jet + 0.02, 0.0)) - 0.034);
    }
    return select(vec2<f32>(d, 0.0), vec2<f32>(dj, 1.0), dj < d);
}

// Where the thinning wall has torn open.
fn crown_torn(p: vec3<f32>, s: CrownShape, seed: f32) -> bool {
    let u = saturate(p.y / max(s.h, 1e-3));
    let n = fbm3(p * 11.0 + vec3<f32>(seed * 19.0), 3);
    return n < s.tear * (0.25 + 0.7 * u);
}

fn crown_normal(p: vec3<f32>, s: CrownShape, seed: f32) -> vec3<f32> {
    let e = vec2<f32>(0.0015, -0.0015);
    return normalize(
        e.xyy * crown_sdf(p + e.xyy, s, seed).x + e.yyx * crown_sdf(p + e.yyx, s, seed).x
        + e.yxy * crown_sdf(p + e.yxy, s, seed).x + e.xxx * crown_sdf(p + e.xxx, s, seed).x);
}

// One surface of the splash, premultiplied: the sky mirrored in it, the sun's glint, and white
// where the water froths (the rim, the beads, the jet's head).
fn crown_surface(in: VsOut, p: vec3<f32>, rd: vec3<f32>, part: f32, s: CrownShape) -> vec4<f32> {
    var n = crown_normal(p, s, in.seed);
    if (dot(n, rd) > 0.0) { n = -n; }
    let nv = saturate(dot(n, -rd));
    let fres = 0.02 + 0.98 * pow(1.0 - nv, 5.0);
    let nw = quat_rotate(in.quat, n);
    let vw = quat_rotate(in.quat, -rd);
    let sky = pre_expose(sample_env(reflect(-vw, nw)));
    let l = frame.sun_dir.xyz;
    let hv = normalize(l + vw);
    let rel = quat_rotate(in.quat, p - in.eye.xyz) * in.eye.w;
    let lit = shadow_factor(rel, nw) * saturate(dot(nw, l) * 4.0);
    let glint = pre_expose(frame.sun_color.rgb) * (pow(saturate(dot(nw, hv)), 900.0) * 36.0 * fres * lit);
    let u = saturate(p.y / max(s.h, 1e-3));
    let froth = select(smoothstep(0.7, 1.05, u) * 0.5 + smoothstep(0.04, 0.0, p.y) * 0.25, smoothstep(s.jet * 0.8, s.jet, p.y) * 0.4 + 0.05, part > 0.5);
    let body = select(0.025, 0.1, part > 0.5) + froth * 0.6;
    let a = saturate(fres + (1.0 - fres) * body);
    let rgb = sky * fres + glint + in.radiance * ((1.0 - fres) * body);
    return vec4<f32>(rgb, a);
}

fn crown(in: VsOut) -> vec4<f32> {
    let s = crown_shape(in.age);
    let ro = in.eye.xyz;
    let rd = normalize(in.at - ro);
    // Clip the ray to a cylinder around all of it.
    let rmax = s.r0 + s.flare + 0.06;
    let ymax = max(s.h * 1.5, s.jet + 0.08) + 0.03;
    let a2 = dot(rd.xz, rd.xz);
    let b2 = dot(ro.xz, rd.xz);
    let c2 = dot(ro.xz, ro.xz) - rmax * rmax;
    let disc = b2 * b2 - a2 * c2;
    if (disc <= 0.0 || a2 < 1e-8) { discard; }
    let sq_disc = sqrt(disc);
    var t0 = (-b2 - sq_disc) / a2;
    var t1 = (-b2 + sq_disc) / a2;
    if (abs(rd.y) > 1e-6) {
        let ya = (0.0 - ro.y) / rd.y;
        let yb = (ymax - ro.y) / rd.y;
        t0 = max(t0, min(ya, yb));
        t1 = min(t1, max(ya, yb));
    }
    t0 = max(t0, 0.0);
    if (t1 <= t0) { discard; }

    let scene = textureLoad(scene_depth, vec2<i32>(in.position.xy), 0);
    let near = frame.exposure.z;
    var acc = vec4<f32>(0.0);
    var t = t0;
    var inside = false;
    var hits = 0;
    for (var i = 0; i < 72; i++) {
        if (t > t1 || hits >= 2 || acc.a > 0.97) { break; }
        let p = ro + rd * t;
        let d = crown_sdf(p, s, in.seed);
        if (inside) {
            // Step through the water to the far side of the wall (or the jet).
            if (d.x > 0.0015) { inside = false; }
            t += 0.004;
            continue;
        }
        if (d.x < 0.0012) {
            inside = true;
            if (d.y < 0.5 && crown_torn(p, s, in.seed)) { continue; }
            // Hidden behind the scene (the far wall behind a rock, or below the water)?
            let rel = quat_rotate(in.quat, p - ro) * in.eye.w;
            let clip = frame.view_proj * vec4<f32>(rel, 1.0);
            let z = clip.z / clip.w;
            var fade = 1.0;
            if (scene > 0.0) {
                let behind = near / scene - near / max(z, 1e-9);
                fade = saturate(behind / (0.02 * in.eye.w));
            }
            if (fade <= 0.0) { break; }
            let layer = crown_surface(in, p, rd, d.y, s) * fade;
            acc += layer * (1.0 - acc.a);
            hits += 1;
            continue;
        }
        t += max(d.x * 0.65, 0.002);
    }
    let a = acc.a * in.coverage;
    if (a <= 0.0005) { discard; }
    return vec4<f32>(acc.rgb * in.glint * in.coverage + in.inscatter * a, a);
}

// ---------------------------------------------------------------- ripples and foam
// A ripple: a packet of wavelets around 0.75 of the sprite's radius, shorter on the outside as
// on real water, settling over its life. Bends what is behind it and catches the sky on its
// crests; written as a difference with zero alpha, like haze.
fn ripple(in: VsOut, c: vec2<f32>, r: f32, jx: vec2<f32>, jy: vec2<f32>) -> vec4<f32> {
    let settle = in.coverage * sq(1.0 - in.age);
    if (r >= 1.0 || settle <= 0.002) { discard; }
    let x = r - 0.74;
    let env = exp(-sq(x / 0.13)) * smoothstep(1.0, 0.92, r);
    let k = 46.0 + 30.0 * saturate(x * 4.0);
    let phase = x * k + in.seed * 6.0;
    // Height and its slope across the ring.
    let slope = env * (cos(phase) * k * 0.02 - sin(phase) * x * 0.02 * 2.0 / sq(0.13)) * settle;
    let dir = c / max(r, 1e-4);
    // How the sprite's coordinates change per pixel gives the shift on screen.
    let inv = mat2x2<f32>(jx, jy);
    let det = determinant(inv);
    if (abs(det) < 1e-12) { discard; }
    let to_px = mat2x2<f32>(vec2<f32>(inv[1].y, -inv[0].y), vec2<f32>(-inv[1].x, inv[0].x)) * (1.0 / det);
    let shift = to_px * (dir * slope * 0.06);
    let size = vec2<i32>(textureDimensions(scene_color));
    let here = vec2<i32>(in.position.xy);
    let there = clamp(vec2<i32>(in.position.xy + shift), vec2<i32>(0), size - 1);
    if (textureLoad(scene_depth, there, 0) > in.position.z + 1e-7) { discard; }
    let delta = textureLoad(scene_color, there, 0).rgb - textureLoad(scene_color, here, 0).rgb;
    // Crests tilted toward the eye catch more of the sky.
    let crest = saturate(sin(phase)) * env * settle;
    return vec4<f32>(delta + in.radiance * crest * 0.1, 0.0);
}

// Distance to the nearest and second nearest of a jittered grid of cell centers.
fn cells(p: vec2<f32>, seed: f32) -> vec2<f32> {
    let i = floor(p);
    let f = p - i;
    var d1 = 8.0;
    var d2 = 8.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let g = vec2<f32>(f32(x), f32(y));
            let o = vec2<f32>(hash12(i + g + seed), hash12(i + g + seed + 17.3));
            let d = length(g + o - f);
            if (d < d1) { d2 = d1; d1 = d; } else if (d < d2) { d2 = d; }
        }
    }
    return vec2<f32>(d1, d2);
}

// Foam: bubble walls (where two cells meet) in a patchy lace that breaks up as it ages.
fn foam(c: vec2<f32>, r: f32, seed: f32, age: f32, pixels: f32) -> f32 {
    let s = seed * 37.0;
    let big = cells(c * 3.2 + s, s);
    let small = cells(c * 9.0 + s * 1.7, s + 3.0);
    let vary = value_noise2(c * 5.0 + s * 3.0);
    let walls = (smoothstep(0.12, 0.0, big.y - big.x) + 0.35 * smoothstep(0.1, 0.0, small.y - small.x)) * (0.4 + 0.8 * vary);
    let clumps = fbm2(c * 2.2 + s, 3);
    let keep = smoothstep(0.15 + age * 0.3, 0.35 + age * 0.3, clumps + 0.3 * (1.0 - r));
    let fill = 0.35 * smoothstep(0.45, 0.9, clumps) * (1.0 - age * 0.6);
    // Too small on screen to show its bubbles, it shows as their average whiteness.
    let lace = mix(0.5 * (0.6 + 0.8 * vary), walls, smoothstep(40.0, 160.0, pixels));
    return saturate((lace + fill) * keep) * smoothstep(1.0, 0.45, r);
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

// An engine's exhaust flame, `t` from the nozzle (0) out (1) and `c.x` across: a white-hot core,
// a train of shock diamonds down its axis and a turbulent tail that flickers and tears away.
// The diamonds are where the jet meets the air's pressure, so in thin air they fade and the
// jet balloons out sooner. Returns the brightness for color 1, how much it whitens toward the
// core, and the coverage.
fn plume(c: vec2<f32>, t: f32, seed: f32, air: f32) -> vec3<f32> {
    let time = (frame.time.x % 3600.0) + seed * 61.0;
    let x = abs(c.x);
    // Eddies streaming out of the nozzle, and a flicker of the whole flame.
    let n = fbm2(vec2<f32>(c.x * 2.2 + seed * 17.0, t * 7.0 - time * 16.0), 3);
    let pulse = value_noise2(vec2<f32>(seed * 5.0, time * 11.0));
    // The jet leaves at the nozzle's width and spreads, its edge rippling.
    let spread = pow(max(t, 0.0), mix(0.35, 0.6, saturate(air)));
    let rp = mix(0.3, 0.95, spread) * (0.85 + 0.3 * n);
    let reach = 0.72 + 0.22 * pulse;
    let tail = 1.0 - smoothstep(reach - 0.3, reach + 0.1, t + (n - 0.5) * 0.4);
    let body = smoothstep(rp, rp * 0.55, x) * tail;
    let core = smoothstep(0.32, 0.0, x) * exp(-t * 7.0);
    // Bright knots down the axis about a nozzle's width apart, dimmer downstream; the gas
    // glows brighter around each and darker where it expands between them.
    let k = fract(t * 6.5 - 0.35);
    let d = x / 0.2 + abs(k - 0.5) * 2.0;
    let train = smoothstep(0.08, 0.12, t) * smoothstep(0.72, 0.1, t) * smoothstep(0.05, 0.5, air);
    let diamonds = smoothstep(1.0, 0.25, d) * train;
    let banding = mix(1.0, 0.55 + 0.7 * smoothstep(1.8, 0.4, d), train);
    let glow = body * (0.5 + 0.9 * n) * banding + core * 1.2 + diamonds * 1.5;
    let a = saturate(body * (0.4 + 0.5 * n) + core + diamonds);
    return vec3<f32>(glow / max(a, 1e-3), saturate(core * 1.2 + diamonds), a);
}

// Blackbody-like: deep red where it is coolest, `rgb` (the layer's color) in the body and near
// white only where hottest, so overlapping flames don't all wash out to yellow.
fn heat_color(rgb: vec3<f32>, heat: f32) -> vec3<f32> {
    let deep = rgb * vec3<f32>(1.0, 0.4, 0.15);
    let hot = mix(rgb, vec3<f32>(1.0, 0.92, 0.75), 0.65);
    return mix(mix(deep, rgb, smoothstep(0.1, 0.55, heat)), hot, smoothstep(0.55, 1.0, heat));
}

// A whole fire, `t` from its base (0) up (1) and `c.x` across: turbulent heat rising through
// a body that narrows upward and sways more the higher it gets, licks tearing off its top.
// Returns the heat (0 clear .. 1 white hot) and the coverage.
fn blaze(c: vec2<f32>, t: f32, seed: f32) -> vec2<f32> {
    let time = (frame.time.x % 3600.0) + seed * 37.0;
    let p = vec2<f32>(c.x, t);
    // Rising eddies bend the flame; finer ones churn inside it.
    let warp = vec2<f32>(
        fbm2(p * vec2<f32>(2.2, 1.6) + vec2<f32>(seed * 11.0, -time * 1.3), 3),
        fbm2(p * vec2<f32>(2.2, 1.6) + vec2<f32>(5.2 + seed * 7.0, -time * 1.7), 3)) - 0.45;
    let n = fbm2(p * vec2<f32>(3.6, 2.4) + warp * 1.6 + vec2<f32>(seed * 3.0, -time * 2.6), 5);
    let sway = warp.x * 0.7 * t * t;
    let x = abs(c.x - sway);
    // Wide and rounded over the fuel, drawn up into a flickering point.
    let w = 0.78 * pow(saturate(1.0 - t), 0.55) * sqrt(smoothstep(0.0, 0.12, t));
    let body = saturate(1.0 - x / max(w, 1e-3));
    // Licks: the top breaks into a few tongues rising side by side.
    let licks = value_noise2(vec2<f32>(c.x * 3.2 + seed * 5.0, t * 1.5 - time * 1.9));
    // Noise only roughens the edge near the body, so no stray flames stand off on their own.
    let near = smoothstep(0.0, 0.4, saturate(1.2 - x / max(w, 1e-3)));
    let heat = saturate(body * 0.95 + ((n - 0.45) * 1.9 + (licks - 0.5) * 0.8 * t) * near - t * 1.05);
    // The base, right on the fuel, is clear and blue-dim; licks fade as they cool.
    let a = smoothstep(0.1, 0.28, heat) * smoothstep(0.0, 0.1, t);
    return vec2<f32>(heat, a);
}

// ---------------------------------------------------------------- jet
// A fountain's jet from the nozzle (t = 0) up to where it turns over (t = 1): a glassy rod of
// water, clear in the middle and mirroring the sky at its edges, streaked as it flows,
// swelling as it slows and breaking into beads and threads near the top. c.x is across it.
fn jet(c: vec2<f32>, t: f32, seed: f32, rgb: vec3<f32>, sky: vec3<f32>) -> vec4<f32> {
    let time = frame.time.x % 3600.0;
    let w = mix(0.3, 0.85, t * t);
    let x = c.x / w;
    if (abs(x) >= 1.0) { return vec4<f32>(0.0); }
    let thick = sqrt(1.0 - x * x);
    let rise = t * 7.0 - time * 5.0 + seed * 13.0;
    let breakup = smoothstep(0.5, 0.95, t);
    let beads = smoothstep(0.3, 0.55, value_noise2(vec2<f32>(x * 1.5 + seed * 7.0, rise)));
    let keep = mix(1.0, beads, breakup) * smoothstep(1.0, 0.82, t) * smoothstep(0.0, 0.03, t);
    let fresnel = pow(1.0 - thick, 2.0);
    let a = (0.45 + 0.5 * fresnel) * keep * smoothstep(1.0, 0.8, abs(x));
    let streaks = value_noise2(vec2<f32>(x * 5.0 + seed * 3.0, t * 2.5 - time * 3.0));
    let glint = exp(-sq((x + 0.4) / 0.13)) * (0.6 + 0.8 * streaks);
    // Air churned into the water whitens its core.
    let col = rgb * (0.6 + 0.5 * thick) * (0.8 + 0.4 * streaks) + sky * (fresnel * 0.9 + glint);
    return vec4<f32>(col, a);
}

// A column of light rising from a spell circle (t = 0 at the circle, 1 at its top): a hollow
// tube whose walls pile up toward its silhouette, combed into rays that drift up and turn
// slowly round it, brightest at its foot and fading out as it rises. c.x is across it.
fn veil(c: vec2<f32>, t: f32, seed: f32) -> f32 {
    let x = c.x;
    if (abs(x) >= 1.0) { return 0.0; }
    let time = frame.time.x % 3600.0;
    let thick = sqrt(1.0 - x * x);
    // Seen through, a tube's wall is crossed at a slant toward its edges.
    let wall = min(0.3 / max(thick, 0.08), 2.0);
    let around = asin(x);
    let rise = t * 1.6 - time * 0.5;
    // The near wall's rays turn one way across the column and the far wall's the other.
    let near = value_noise2(vec2<f32>(around * 5.0 + time * 0.2 + seed * 11.0, rise));
    let far = value_noise2(vec2<f32>(-around * 5.0 + time * 0.2 + seed * 5.0 + 17.0, rise * 1.2 + 3.1));
    let rays = smoothstep(0.35, 0.85, near) + 0.7 * smoothstep(0.35, 0.85, far);
    // Faint rings of light climb it now and then.
    let pulse = pow(0.5 + 0.5 * sin((t * 2.0 - time * 0.45 + seed) * TAU), 8.0);
    let fade = smoothstep(0.0, 0.04, t) * pow(1.0 - t, 1.8);
    let foot = exp(-t * 14.0);
    return fade * (wall * (0.25 + 0.75 * rays) * (1.0 + 0.6 * pulse) + foot * 1.5)
        * smoothstep(1.0, 0.85, abs(x));
}

// A cluster of ice spikes thrust out from where something froze (uv.y 0 at the root): seven
// faceted, glassy six-sided spikes fanned out from one root, the middle ones tallest, each
// ending in a point. They grow out in the first moments of the particle's life, cloud with
// cracks and thin away at its end. Units are the sprite's height; the quad is square.
fn crystal(in: VsOut, c: vec2<f32>) -> vec4<f32> {
    let p = vec2<f32>(c.x * 0.5, in.uv.y);
    let grow = smoothstep(0.0, 0.06, in.age);
    let gone = smoothstep(0.7, 1.0, in.age);
    // Back spikes first, so the middle ones stand in front.
    var fan = array<f32, 7>(-1.0, 1.0, -0.667, 0.667, -0.333, 0.333, 0.0);
    var found = false;
    var u = 0.0;
    var up = 0.0;
    var q = vec2<f32>(0.0);
    for (var i = 0; i < 7; i++) {
        let k = fan[i];
        let h = hash12(vec2<f32>(k * 7.0 + 3.1, in.seed * 91.0));
        let h2 = hash12(vec2<f32>(in.seed * 53.0, k * 11.0 + 1.7));
        let ang = k * 0.8 + (h - 0.5) * 0.3;
        let dir = vec2<f32>(sin(ang), cos(ang));
        let len = (0.95 - 0.4 * abs(k) - 0.2 * h2) * grow;
        let w = 0.04 + 0.035 * h2 + 0.035 * (1.0 - abs(k));
        let d = p - vec2<f32>(k * 0.07, 0.0);
        let along = dot(d, dir);
        let across = d.x * dir.y - d.y * dir.x;
        let half_w = w * saturate((len - along) / (w * 2.2));
        if (along > 0.0 && along < len && abs(across) < half_w) {
            found = true;
            u = across / half_w;
            up = along / max(len, 1e-3);
            q = d * 9.0 + vec2<f32>(k * 5.0, in.seed * 17.0);
        }
    }
    if (!found) { return vec4<f32>(0.0); }
    // A six-sided prism side on shows three faces: a lit one, a front one and a shaded one.
    let face = select(select(0.55, 1.0, u > -0.4), 0.75, u > 0.4);
    let nx = select(select(-0.75, 0.0, u > -0.4), 0.75, u > 0.4);
    let sun = saturate(dot(vec3<f32>(nx, 0.0, sqrt(1.0 - nx * nx)), in.sun));
    let ridge = exp(-sq((abs(u) - 0.4) / 0.07));
    let rim = smoothstep(0.75, 1.0, abs(u));
    let cracks = smoothstep(0.06, 0.0, abs(value_noise2(q) - 0.5)) * (0.25 + gone);
    // Deep blue down in the root, paling to near white at the tips.
    let tint = mix(vec3<f32>(0.45, 0.7, 1.0), vec3<f32>(1.0), up * 0.8);
    let col = in.radiance * tint * (0.35 * face + 0.5 * sun + 0.25)
        + in.glint * (0.25 + 0.5 * ridge + 0.4 * rim) + in.radiance * cracks;
    let a = (0.6 + 0.3 * rim + 0.2 * ridge + 0.3 * cracks) * (1.0 - gone)
        * smoothstep(1.0, 0.9, abs(u));
    return vec4<f32>(col, saturate(a));
}

// How far a jagged electric channel strays sideways at `x` along it (s picks its path).
fn jag(x: f32, s: f32) -> f32 {
    return (value_noise2(vec2<f32>(x * 6.0, s)) - 0.5) * 0.35 * sqrt(max(x, 0.0))
        + (value_noise2(vec2<f32>(x * 22.0, s + 5.0)) - 0.5) * 0.07 * x;
}

// Glow of a jagged channel `len` long running along +x from the origin: a thin white-hot core
// in a wider halo, thinning out toward its end.
fn channel(p: vec2<f32>, len: f32, s: f32) -> f32 {
    if (p.x < -0.03 || p.x > len) { return 0.0; }
    let d = abs(p.y - jag(p.x, s));
    let thin = smoothstep(len, len * 0.55, p.x);
    return (exp(-sq(d / 0.012)) + 0.3 * exp(-sq(d / 0.06))) * thin;
}

// An electric arc crawling over the ground from where lightning struck (the sprite's center)
// out along +x, with a branch forking off it. It re-strikes along a new path several times a
// second, so it crackles.
fn arc(c: vec2<f32>, seed: f32, age: f32) -> f32 {
    let strike = floor(age * 7.0);
    let s = seed * 17.0 + strike * 3.7;
    var glow = channel(c, 0.95, s);
    let bx = 0.25 + 0.3 * hash12(vec2<f32>(s, 1.3));
    let turn = select(-0.7, 0.7, hash12(vec2<f32>(s, 2.9)) > 0.5);
    let dir = vec2<f32>(cos(turn), sin(turn));
    let q = c - vec2<f32>(bx, jag(bx, s));
    glow += 0.7 * channel(vec2<f32>(dot(q, dir), q.y * dir.x - q.x * dir.y), 0.4, s + 9.0);
    let flicker = 0.55 + 0.45 * hash12(vec2<f32>(s, 4.1));
    return saturate(glow) * flicker;
}

// ---------------------------------------------------------------- fireball
// A ball of burning gas traced through its sprite as a volume: a lumpy body rolled into lobes,
// glowing from inside (near white deep in, reddening outward), turning to dark soot from the
// outside in as it cools with age. Units are the sprite's radius; the eye looks down -z.
fn fireball_field(p: vec3<f32>, seed: f32, age: f32, time: f32) -> vec2<f32> {
    let q = p * 2.2 + vec3<f32>(seed * 17.0, seed * 31.0 - time * 0.4 - age * 1.5, seed * 7.0);
    let lobes = fbm3(q, 3) * 1.14;
    let fine = fbm3(q * 2.2 + vec3<f32>(3.7, 1.9, 5.3), 2) * 1.33;
    let r = length(p);
    let edge = 0.42 + 0.5 * lobes + 0.06 * fine;
    let density = saturate((edge - r) * 7.0);
    // Hot throughout while young, hottest deep inside and in the swelling lobes, cooling
    // from the creases between them as it ages.
    let depth = 1.0 - r / max(edge, 0.05);
    let heat = saturate(1.0 + depth * 0.6 + (fine - 0.5) * 0.9 - (1.0 - lobes) * 0.5 - age * 1.7);
    return vec2<f32>(density, heat);
}

fn fireball(in: VsOut, c: vec2<f32>) -> vec4<f32> {
    let r2 = dot(c, c);
    if (r2 >= 1.0) { return vec4<f32>(0.0); }
    let time = frame.time.x % 3600.0;
    let half_len = sqrt(1.0 - r2);
    let steps = 12;
    let dz = 2.0 * half_len / f32(steps);
    // A per-pixel offset into the first step hides the slices.
    let jitter = fract(sin(dot(in.position.xy, vec2<f32>(12.9898, 78.233))) * 43758.547);
    // Soot is dark smoke lit by the scene, brighter on its sun side.
    let n = vec3<f32>(c, half_len);
    let sun_side = 0.55 + 0.45 * saturate(dot(n, in.sun) * 0.8 + 0.4);
    let soot = in.glint * 0.06 * sun_side;
    var trans = 1.0;
    var light = vec3<f32>(0.0);
    for (var i = 0; i < steps; i++) {
        let z = half_len - (f32(i) + jitter) * dz;
        let f = fireball_field(vec3<f32>(c, z), in.seed, in.age, time);
        let a = 1.0 - exp(-f.x * 6.0 * dz);
        let glow = heat_color(in.radiance, f.y * 0.85) * (2.2 * f.y * f.y * f.y * smoothstep(0.2, 0.55, f.y));
        light += trans * a * (glow + soot);
        trans *= 1.0 - a;
        if (trans < 0.02) { break; }
    }
    let alpha = 1.0 - trans;
    return vec4<f32>(light / max(alpha, 1e-4), alpha);
}

// A portal: the world behind it swirled into a vortex that sinks into the portal's color and
// darkens toward its eye, glowing arms turning inward and a burning rim. `jx` and `jy` are how
// the sprite coordinates change per pixel across and down the screen, to turn a shift within
// the sprite into one on screen at any angle.
fn vortex(in: VsOut, c: vec2<f32>, r: f32, jx: vec2<f32>, jy: vec2<f32>) -> vec4<f32> {
    let a = smoothstep(1.0, 0.88, r) * in.coverage;
    if (a <= 0.002) { discard; }
    let t = (frame.time.x % 3600.0) + in.seed * 97.0;
    // The world bends toward the eye: a twist that tightens inward and breathes, and a pull
    // that shows a shrunken, far-off view in the middle.
    let inner = 1.0 - r;
    let twist = inner * inner * 2.8 + 0.3 * inner * sin(t * 0.8 + r * 7.0);
    let cs = cos(twist);
    let sn = sin(twist);
    let seen = vec2<f32>(c.x * cs - c.y * sn, c.x * sn + c.y * cs) * mix(1.0, 0.55, inner * inner);
    let d = seen - c;
    let det = jx.x * jy.y - jy.x * jx.y;
    var shift = vec2<f32>(0.0);
    if (abs(det) > 1e-12) {
        shift = vec2<f32>(jy.y * d.x - jy.x * d.y, jx.x * d.y - jx.y * d.x) / det;
    }
    // Seen edge-on the shift would run off the screen; keep it within the sprite's reach.
    shift = shift * min(1.0, in.pixels / max(length(shift), 1e-4));
    let size = vec2<i32>(textureDimensions(scene_color));
    let here = vec2<i32>(in.position.xy);
    var there = clamp(vec2<i32>(in.position.xy + shift), vec2<i32>(0), size - 1);
    // Only the world behind the portal shows through it, never something in front (reverse Z).
    if (textureLoad(scene_depth, there, 0) > in.position.z) { there = here; }
    var world = textureLoad(scene_color, there, 0).rgb;
    let peak = max(in.radiance.r, max(in.radiance.g, in.radiance.b));
    let hue = in.radiance / max(peak, 1e-6);
    let lum = dot(world, vec3<f32>(0.2126, 0.7152, 0.0722));
    let sink = smoothstep(0.8, 0.05, r);
    world = mix(world, hue * lum * 1.5, sink * 0.75) * mix(1.0, 0.2, sink * sink);
    // Arms: a few log spirals turning inward, broken up by noise.
    let ang = atan2(c.y, c.x);
    let spiral = ang * 3.0 + log(max(r, 0.02)) * 4.5 + t * 2.2;
    let streak = pow(0.5 + 0.5 * sin(spiral), 5.0);
    let n = fbm2(vec2<f32>(spiral * 0.6, r * 5.0 - t * 0.9) + in.seed * 13.0, 3);
    let arms = streak * (0.35 + n) * smoothstep(0.04, 0.45, r) * smoothstep(1.0, 0.55, r);
    // The rim burns unevenly, flaring and fading as it turns.
    let flare = 0.15 + 3.0 * sq(fbm2(vec2<f32>(ang * 5.0 + t * 1.3, t * 0.7) + in.seed * 7.0, 3));
    let rim = exp(-sq((r - 0.935) / 0.022)) * flare + 0.06 * exp(-sq((r - 0.9) / 0.1));
    let eye = exp(-r * r * 40.0) * 0.5;
    let glow = in.radiance * (arms * 1.1 + rim * 1.5 + eye);
    return vec4<f32>((world + glow) * a, a);
}

@fragment
fn fs_particle(in: VsOut) -> @location(0) vec4<f32> {
    let c = in.uv * 2.0 - 1.0;
    let r = length(c);
    // Taken here, in uniform control flow, for the sprites that need them.
    let jx = dpdx(c);
    let jy = dpdy(c);
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
        case 3u: { // flame: a licking tongue, round at the root and drawn up to a flickering point
            let s = in.seed * 23.0;
            let up = saturate(c.y * 0.5 + 0.5);
            // Rising swirls sway the tongue, more the higher up; finer ones flicker its edges.
            let f = vec2<f32>(c.x * 1.4 + s, c.y * 0.9 - in.age * 3.0 + s * 0.7);
            let w = vec2<f32>(fbm2(f, 2), fbm2(f + vec2<f32>(5.2, 1.3), 2)) - 0.45;
            let e = fbm2(vec2<f32>(c.x * 4.0 + s + 2.7, c.y * 2.5 - in.age * 7.0 + s), 2) - 0.45;
            let p = c + vec2<f32>(w.x * 0.9 + e * 0.35, w.y * 0.4) * (0.25 + up * 0.9);
            // Teardrop: a round root below `yb`, narrowing to a point at the top.
            let yb = -0.35;
            var hw = 0.68 * pow(saturate((0.95 - p.y) / (0.95 - yb)), 1.4);
            if (p.y < yb) {
                hw = 0.68 * sqrt(saturate(1.0 - sq((p.y - yb) / 0.55)));
            }
            let body = smoothstep(1.0, 0.45, abs(p.x) / max(hw, 1e-3)) * step(1e-3, hw);
            // Licks rising through it; where they thin out near the top the tongue tears.
            let n = fbm2(vec2<f32>(p.x * 3.4, p.y * 1.3 - in.age * 5.0) + s, 4);
            let core = exp(-(p.x * p.x * 9.0 + sq(p.y + 0.4) * 3.0));
            let heat = saturate(body * (n * 1.6 - 0.5 + (1.0 - up) * 0.3 + core * 0.45));
            // The root, right over the fuel, is dim and clear.
            a = smoothstep(0.04, 0.25, heat) * (0.25 + 0.75 * smoothstep(-0.95, -0.45, p.y));
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
        case 12u: { // portal: the scene behind it swirled into a vortex
            return vortex(in, c, r, jx, jy);
        }
        case 19u: { // jet: a fountain's rising rod of water, from the nozzle (uv.y 0) up
            let j = jet(c, in.uv.y, in.seed, rgb, in.glint);
            rgb = j.rgb;
            a = j.a;
        }
        case 20u: { // veil: a column of light rising from a spell circle (uv.y 0) up
            a = veil(c, in.uv.y, in.seed);
            rgb *= 1.0 + saturate(a - 0.6);
        }
        case 21u: { // crystal: a cluster of ice spikes from its root (uv.y 0) out
            let k = crystal(in, c);
            rgb = k.rgb;
            a = k.a;
        }
        case 22u: { // arc: electricity crawling out from the sprite's center along +x
            a = arc(c, in.seed, in.age);
            rgb *= 1.0 + 2.0 * a * a;
        }
        case 18u: { // fireball: a burning, cooling volume of gas
            let f = fireball(in, c);
            rgb = f.rgb;
            a = f.a;
        }
        case 17u: { // blaze: a whole fire from its base (uv.y 0) up
            let b = blaze(c, in.uv.y, in.seed);
            a = b.y;
            rgb = heat_color(rgb, b.x) * (0.3 + 1.4 * b.x * b.x * b.x);
        }
        case 14u: { // crown: a splash's wall of water, traced
            return crown(in);
        }
        case 15u: { // ripple: bends the water behind it
            return ripple(in, c, r, jx, jy);
        }
        case 16u: { // foam: a lace of bubbles on the water
            a = foam(c, r, in.seed, in.age, in.pixels);
            rgb *= 0.9 + 0.2 * a;
        }
        case 13u: { // plume: an exhaust flame from the nozzle (uv.y 0) out
            let p = plume(c, in.uv.y, in.seed, in.glint.x);
            a = p.z;
            let lum = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
            rgb = mix(rgb, vec3<f32>(lum), p.y) * p.x;
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
