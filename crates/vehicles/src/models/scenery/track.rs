//! The race track scenery around `demo::spawn_track`'s colliders: one continuous asphalt
//! surface along the oval (rubbered racing line, skid marks, sealed cracks, patch repairs,
//! worn paint), FIA-style red/white curbs with a rumble profile in the turns, gravel traps and
//! tire walls on the outside of the turns, double Armco, a concrete wall with catch fencing
//! along the main straight, a covered grandstand, a pit building with pit lane and pit wall,
//! the paddock with the teams' transporters behind it, a start gantry, marshal posts,
//! sponsor boards, a footbridge, the concrete kicker and the dirt drift pad.
//!
//! Layout (driving direction = increasing oval angle): the start/finish line is in the middle
//! of the north straight (angle −π/2) with the grandstand outside and the pits inside, the
//! kicker sits on the south straight, and the turns are the two ends of the oval.

use std::f32::consts::{PI, TAU};
use std::time::Instant;

use engine_core::math::{DVec3, Mat4, Quat, Vec2, Vec3, Vec4};
use engine_core::prelude::*;

use super::kit::{Builder, Frame, gray, hex};
use super::noise::{mix, rand1, rand2, smoothstep, world_fbm};
use super::{BuildStats, Parts, Scene, Surface, tex};
use crate::demo::{JUMP_ANGLE, TRACK_RADII, TRACK_SURFACE, TRACK_WIDTH};

/// Height of the asphalt top above the zone ground (m).
const TOP: f32 = TRACK_SURFACE as f32;
/// Half the asphalt width (m).
const HALF: f32 = TRACK_WIDTH * 0.5;
/// Curbs span this lateral range beyond the asphalt edge (matches the curb colliders).
const CURB: (f32, f32) = (HALF, HALF + 1.0);
/// The drift pad (center relative to the track center, radius) as in `demo::spawn_track`.
pub const DRIFT_PAD: (Vec3, f32) = (Vec3::new(-70.0, 0.0, -20.0), 26.0);
/// The rally car's drift circle radius (m).
const DRIFT_RADIUS: f32 = 15.0;

/// Queue the track scenery around a track centered on `center` (see `demo::spawn_track`).
pub fn spawn(commands: &mut Commands, center: DVec3) {
    commands.queue(move |world: &mut World| {
        build(world, center);
    });
}

/// Build the track scenery now. `None` without a renderer.
pub fn build(world: &mut World, center: DVec3) -> Option<BuildStats> {
    let started = Instant::now();
    let mut scene = Scene::new(world, center, "track")?;
    let o = Oval::new();
    let mut parts = Parts::default();
    road(&o, &mut parts);
    markings(&o, &mut parts);
    rubber(&o, &mut parts);
    curbs(&o, &mut parts);
    runoff(&o, &mut parts);
    barriers(&o, &mut parts);
    grandstand(&o, &mut parts);
    pits(&o, &mut parts);
    pit_back(&o, &mut parts);
    paddock(&o, &mut parts);
    gantry(&o, &mut parts);
    marshal_posts(&o, &mut parts);
    billboards(&o, &mut parts);
    footbridge(&o, &mut parts);
    kicker(&o, &mut parts);
    drift_pad(&mut parts);
    parts.spawn(&mut scene, "track");
    Some(scene.finish("track", started))
}

// ---------------------------------------------------------------- the oval

/// The oval's center line parametrized by distance, with offsets along its outward normal.
pub struct Oval {
    pub rx: f32,
    pub rz: f32,
    angles: Vec<f32>,
    dists: Vec<f32>,
    /// Lap length along the center line (m).
    pub length: f32,
    /// Texture v per meter along the track, so a whole number of asphalt tiles fits a lap.
    pub vk: f32,
}

impl Default for Oval {
    fn default() -> Self {
        Self::new()
    }
}

impl Oval {
    pub fn new() -> Self {
        let (rx, rz) = (TRACK_RADII.0 as f32, TRACK_RADII.1 as f32);
        let n = 8192;
        let mut angles = Vec::with_capacity(n + 1);
        let mut dists = Vec::with_capacity(n + 1);
        let mut acc = 0.0f64;
        let p = |a: f64| (rx as f64 * a.cos(), rz as f64 * a.sin());
        let mut prev = p(0.0);
        for i in 0..=n {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            let q = p(a);
            acc += ((q.0 - prev.0).powi(2) + (q.1 - prev.1).powi(2)).sqrt();
            prev = q;
            angles.push(a as f32);
            dists.push(acc as f32);
        }
        let length = acc as f32;
        let tiles = (length / super::ASPHALT_TILE).round();
        Self {
            rx,
            rz,
            angles,
            dists,
            length,
            vk: tiles * super::ASPHALT_TILE / length,
        }
    }

    /// Oval angle at distance `s` (wraps).
    pub fn angle(&self, s: f32) -> f32 {
        let s = s.rem_euclid(self.length);
        let i = self
            .dists
            .partition_point(|&v| v < s)
            .clamp(1, self.dists.len() - 1);
        let (s0, s1) = (self.dists[i - 1], self.dists[i]);
        let t = (s - s0) / (s1 - s0).max(1e-6);
        self.angles[i - 1] + (self.angles[i] - self.angles[i - 1]) * t
    }

    /// Distance along the lap at oval angle `a`.
    pub fn dist(&self, a: f32) -> f32 {
        let a = a.rem_euclid(TAU);
        let i = self
            .angles
            .partition_point(|&v| v < a)
            .clamp(1, self.angles.len() - 1);
        let (a0, a1) = (self.angles[i - 1], self.angles[i]);
        let t = (a - a0) / (a1 - a0).max(1e-9);
        self.dists[i - 1] + (self.dists[i] - self.dists[i - 1]) * t
    }

    fn point(&self, a: f32) -> Vec3 {
        Vec3::new(self.rx * a.cos(), 0.0, self.rz * a.sin())
    }

    /// Driving direction at distance `s`.
    pub fn dir(&self, s: f32) -> Vec3 {
        let a = self.angle(s);
        Vec3::new(-self.rx * a.sin(), 0.0, self.rz * a.cos()).normalize()
    }

    /// Outward normal at `s` (away from the oval's middle).
    pub fn out(&self, s: f32) -> Vec3 {
        let d = self.dir(s);
        Vec3::new(d.z, 0.0, -d.x)
    }

    /// Local point at distance `s`, offset `d` outward, height `y`.
    pub fn at(&self, s: f32, d: f32, y: f32) -> Vec3 {
        let a = self.angle(s);
        let t = Vec3::new(-self.rx * a.sin(), 0.0, self.rz * a.cos()).normalize();
        let n = Vec3::new(t.z, 0.0, -t.x);
        self.point(a) + n * d + Vec3::Y * y
    }

    /// Radius of curvature of the center line at `s` (m).
    pub fn radius(&self, s: f32) -> f32 {
        let a = self.angle(s);
        let (sn, cs) = a.sin_cos();
        (self.rx * self.rx * sn * sn + self.rz * self.rz * cs * cs).powf(1.5) / (self.rx * self.rz)
    }

    /// 1 in the tight ends of the oval, 0 on the straights.
    pub fn turn(&self, s: f32) -> f32 {
        smoothstep(150.0, 90.0, self.radius(s))
    }

    /// A sweep frame at `s`, `d`, `y` whose profile x points toward the track when
    /// `toward_track` (outward otherwise for d > 0 …): x = −sign(d) · outward.
    pub fn frame(&self, s: f32, d: f32, y: f32, x_axis_out: bool) -> Frame {
        let out = self.out(s);
        Frame {
            pos: self.at(s, d, y),
            side: if x_axis_out { out } else { -out },
            up: Vec3::Y,
            s,
        }
    }

    /// Distance of the start/finish line.
    pub fn start(&self) -> f32 {
        self.dist(1.5 * PI)
    }

    /// Distance of the kicker's low edge.
    pub fn jump(&self) -> f32 {
        self.dist(JUMP_ANGLE as f32)
    }

    /// Distance of the apex of the east (0) or west (1) turn.
    pub fn apex(&self, which: u32) -> f32 {
        if which == 0 { 0.0 } else { self.dist(PI) }
    }

    /// Wrapped difference `s - from` in (−L/2, L/2].
    pub fn delta(&self, s: f32, from: f32) -> f32 {
        let mut d = (s - from).rem_euclid(self.length);
        if d > self.length * 0.5 {
            d -= self.length;
        }
        d
    }
}

/// Distances where the pit straight features sit, relative to the start line.
mod pit {
    /// Pit lane begins to leave the track (driving direction is +s).
    pub const ENTRY: f32 = -100.0;
    /// Fully separated.
    pub const ENTRY_END: f32 = -65.0;
    pub const EXIT: f32 = 65.0;
    pub const EXIT_END: f32 = 100.0;
    /// Pit building extent.
    pub const BUILDING: f32 = 50.0;
    /// Pit wall extent.
    pub const WALL: (f32, f32) = (-68.0, 66.0);
    /// Lateral layout (d, negative = inside the oval).
    pub const WALL_D: f32 = -9.4;
    pub const LANE_OUT: f32 = -10.3;
    pub const FAST_IN: f32 = -16.3;
    pub const FACADE: f32 = -24.0;
    pub const BACK: f32 = -40.0;
    /// Grandstand extent.
    pub const STAND: f32 = 52.0;
    /// Concrete wall + fence extent along the outside.
    pub const OUTER_WALL: f32 = 78.0;
}

/// Lateral position of the outer barrier at `s` (Armco or wall face line).
fn outer_barrier(o: &Oval, s: f32) -> f32 {
    let on_pit = o.delta(s, o.start()).abs() < pit::OUTER_WALL;
    if on_pit {
        10.0
    } else {
        11.0 + 12.0 * o.turn(s)
    }
}

/// Inner edge of the gravel at `s` (0 width where there is no trap).
fn gravel_width(o: &Oval, s: f32) -> f32 {
    let t = o.turn(s);
    if t < 0.02 {
        0.0
    } else {
        (outer_barrier(o, s) - 1.4 - 7.4) * smoothstep(0.02, 0.35, t)
    }
}

// ---------------------------------------------------------------- road surface

fn road(o: &Oval, parts: &mut Parts) {
    let b = parts.get(Surface::Asphalt);
    let rows = (o.length / 1.0).ceil() as usize;
    let ds = o.length / rows as f32;
    let cols: Vec<f32> = (0..=48).map(|i| -HALF + i as f32 * 0.25).collect();
    let base = b.mesh.vertices.len() as u32;
    for r in 0..=rows {
        let s = r as f32 * ds;
        for &d in &cols {
            let p = o.at(s, d, TOP);
            let c = road_tone(o, s, d, p);
            b.vertex_c(
                p,
                Vec3::Y,
                Vec2::new(d, s * o.vk),
                Vec4::new(c[0], c[1], c[2], 1.0),
            );
        }
    }
    let w = cols.len() as u32;
    for r in 0..rows as u32 {
        for c in 0..w - 1 {
            let k = base + r * w + c;
            b.quad_idx(k, k + 1, k + w + 1, k + w);
        }
    }
}

/// Asphalt tint: low-frequency tone, two rubbered wheel tracks along the driven line, more
/// rubber in the braking zones and through the turns, dusty edges.
fn road_tone(o: &Oval, s: f32, d: f32, p: Vec3) -> [f32; 3] {
    let macro_tone =
        1.0 + 0.07 * world_fbm(p.x, p.z, 28.0, 3, 1) + 0.05 * world_fbm(p.x, p.z, 6.0, 2, 2);
    let turn = o.turn(s);
    // The cars run wide through the turns: the groove moves out a little.
    let c = 0.6 * turn;
    let line = |at: f32, w: f32| (-((d - at) / w).powi(2) * 0.5).exp();
    let tracks = (line(c - 0.8, 0.34) + line(c + 0.8, 0.34)).min(1.0);
    let groove = 0.24 * tracks + 0.12 * line(c, 1.9) + 0.05 * line(c, 3.5);
    // Braking zones before each turn.
    let mut braking: f32 = 0.0;
    for k in 0..2 {
        let rel = o.delta(s, o.apex(k));
        braking = braking.max(smoothstep(-120.0, -75.0, rel) * smoothstep(-20.0, -45.0, rel));
    }
    let rubber = groove * (0.75 + 0.25 * turn + 0.5 * braking);
    let dust = 0.12 * smoothstep(4.4, 5.9, d.abs()) + 0.04 * smoothstep(2.5, 5.0, d.abs());
    let v = macro_tone * (1.0 - rubber + dust);
    [v * 1.01, v, v * 0.98]
}

/// A strip along the track from `s0` to `s1` between lateral `d0` and `d1` at height `y`,
/// with the road's UV mapping (so paint and patches line up with the asphalt texture).
#[allow(clippy::too_many_arguments)]
fn strip(b: &mut Builder, o: &Oval, s0: f32, s1: f32, d0: f32, d1: f32, y: f32, step: f32) {
    let n = ((s1 - s0).abs() / step).ceil().max(1.0) as u32;
    let base = b.mesh.vertices.len() as u32;
    for i in 0..=n {
        let s = s0 + (s1 - s0) * i as f32 / n as f32;
        for d in [d0, d1] {
            b.vertex(o.at(s, d, y), Vec3::Y, Vec2::new(d, s * o.vk));
        }
    }
    for i in 0..n {
        let k = base + i * 2;
        b.quad_idx(k, k + 1, k + 3, k + 2);
    }
}

/// A strip whose lateral edges vary along it: `edges(s) -> (d0, d1)`.
fn strip_fn(
    b: &mut Builder,
    o: &Oval,
    s0: f32,
    s1: f32,
    y: f32,
    step: f32,
    edges: impl Fn(f32) -> (f32, f32),
) {
    let n = ((s1 - s0).abs() / step).ceil().max(1.0) as u32;
    let base = b.mesh.vertices.len() as u32;
    for i in 0..=n {
        let s = s0 + (s1 - s0) * i as f32 / n as f32;
        let (d0, d1) = edges(s);
        for d in [d0, d1] {
            b.vertex(o.at(s, d, y), Vec3::Y, Vec2::new(d, s * o.vk));
        }
    }
    for i in 0..n {
        let k = base + i * 2;
        b.quad_idx(k, k + 1, k + 3, k + 2);
    }
}

// ---------------------------------------------------------------- paint

const PAINT_Y: f32 = TOP + 0.003;

fn white() -> Vec4 {
    Vec4::new(0.92, 0.92, 0.9, 1.0)
}

fn markings(o: &Oval, parts: &mut Parts) {
    let st = o.start();
    {
        let b = parts.get(Surface::PaintAsphalt);
        b.color(white());
        // Track-limit lines along both edges.
        for side in [-1.0f32, 1.0] {
            let (d0, d1) = (side * (HALF - 0.27), side * (HALF - 0.04));
            strip(b, o, 0.0, o.length, d0, d1, PAINT_Y, 1.0);
        }
        // Start/finish line and the grid: 12 staggered boxes, 8 m apart, pole on the inside.
        strip(
            b,
            o,
            st - 0.2,
            st + 0.2,
            -HALF + 0.3,
            HALF - 0.3,
            PAINT_Y,
            1.0,
        );
        for k in 0..12 {
            let s = st - 9.0 - k as f32 * 8.0;
            let dc = if k % 2 == 0 { -2.7 } else { 2.7 };
            strip(b, o, s - 0.08, s + 0.08, dc - 1.7, dc + 1.7, PAINT_Y, 1.0);
            for e in [-1.7f32, 1.55] {
                strip(b, o, s - 1.4, s - 0.08, dc + e, dc + e + 0.15, PAINT_Y, 1.0);
            }
        }
        // Pit lane: blend lines from the track edge, fast/working lane divider, speed limit
        // lines.
        let pe = |s: f32| pit_lane_edges(o, s);
        strip_fn(
            b,
            o,
            st + pit::ENTRY,
            st + pit::EXIT_END,
            PAINT_Y,
            1.0,
            |s| {
                let (out, _) = pe(s);
                (out - 0.2, out)
            },
        );
        strip(
            b,
            o,
            st - pit::BUILDING,
            st + pit::BUILDING,
            pit::FAST_IN - 0.08,
            pit::FAST_IN + 0.08,
            PAINT_Y,
            1.0,
        );
        for at in [pit::ENTRY_END + 4.0, pit::EXIT - 4.0] {
            strip(
                b,
                o,
                st + at - 0.15,
                st + at + 0.15,
                pit::FAST_IN,
                pit::LANE_OUT,
                PAINT_Y,
                1.0,
            );
        }
    }
    // Grid position numbers in each box, readable from the car.
    let g = parts.get(Surface::Glyphs);
    g.color(white());
    for k in 0..12 {
        let s = st - 9.0 - k as f32 * 8.0;
        let dc = if k % 2 == 0 { -2.7 } else { 2.7 };
        text_on_track(g, o, &format!("{}", k + 1), s - 2.6, dc, 1.1);
    }
    // Pit lane speed limit.
    for (at, txt) in [(pit::ENTRY_END + 9.0, "60"), (pit::EXIT - 9.0, "60")] {
        text_on_track(
            g,
            o,
            txt,
            st + at,
            (pit::FAST_IN + pit::LANE_OUT) * 0.5,
            1.6,
        );
    }
    text_on_track(
        g,
        o,
        "PIT",
        st + pit::ENTRY_END + 16.0,
        (pit::FAST_IN + pit::LANE_OUT) * 0.5,
        1.6,
    );
}

/// Paint `text` on the track centered at (`s`, `d`), glyphs `h` meters tall, reading in the
/// driving direction.
fn text_on_track(b: &mut Builder, o: &Oval, text: &str, s: f32, d: f32, h: f32) {
    let unit = h / 6.0;
    let n = text.chars().count() as f32;
    let total = (n - 1.0) * super::font::ADVANCE * unit + super::font::WIDTH * unit;
    for (i, c) in text.chars().enumerate() {
        let Some(uv) = tex::glyph_uv(c) else { continue };
        // Driver's right is toward −d (the inside).
        let x = -total * 0.5 + i as f32 * super::font::ADVANCE * unit + 2.0 * unit;
        let gc = o.at(s, d - x, PAINT_Y + 0.0005);
        let fwd = o.dir(s);
        let right = -o.out(s);
        let (hw, hh) = (3.0 * unit, 4.0 * unit);
        // Glyph cell: 6 × 8 units, glyph box centered 1 unit in from each side.
        let center = gc + fwd * unit * 1.0;
        let p = [
            center - right * hw - fwd * hh,
            center + right * hw - fwd * hh,
            center + right * hw + fwd * hh,
            center - right * hw + fwd * hh,
        ];
        let t = [
            Vec2::new(uv[0], uv[3]),
            Vec2::new(uv[2], uv[3]),
            Vec2::new(uv[2], uv[1]),
            Vec2::new(uv[0], uv[1]),
        ];
        b.quad_n(p, t, Vec3::Y);
    }
}

/// Pit lane surface edges at `s`: (toward the track, toward the building).
fn pit_lane_edges(o: &Oval, s: f32) -> (f32, f32) {
    let r = o.delta(s, o.start());
    let entry = smoothstep(pit::ENTRY, pit::ENTRY_END, r);
    let exit = smoothstep(pit::EXIT_END, pit::EXIT, r);
    let k = entry.min(exit);
    let out = mix(-HALF, pit::LANE_OUT, k);
    let fast = mix(-HALF - 0.3, pit::FAST_IN, k);
    let building = smoothstep(pit::BUILDING + 6.0, pit::BUILDING, r.abs());
    (out, mix(fast, pit::FACADE, building))
}

// ---------------------------------------------------------------- rubber, tar, patches

/// A ribbon decal along `path` points (zone-local) `width` wide, alpha per point, UV v in
/// meters along the path.
fn ribbon(b: &mut Builder, path: &[(Vec3, f32)], width: f32) {
    if path.len() < 2 {
        return;
    }
    let base = b.mesh.vertices.len() as u32;
    let mut v = 0.0;
    for i in 0..path.len() {
        let a = path[i.saturating_sub(1)].0;
        let c = path[(i + 1).min(path.len() - 1)].0;
        let dir = (c - a).normalize_or(Vec3::X);
        let side = dir.cross(Vec3::Y).normalize_or(Vec3::Z) * width * 0.5;
        if i > 0 {
            v += (path[i].0 - path[i - 1].0).length();
        }
        let (p, alpha) = path[i];
        let col = Vec4::new(b.color.x, b.color.y, b.color.z, b.color.w * alpha);
        b.vertex_c(p - side, Vec3::Y, Vec2::new(0.0, v), col);
        b.vertex_c(p + side, Vec3::Y, Vec2::new(1.0, v), col);
    }
    for i in 0..path.len() as u32 - 1 {
        let k = base + i * 2;
        b.quad_idx(k, k + 1, k + 3, k + 2);
    }
}

/// Alpha envelope for a mark of length `len` at position `t` (m from its start).
fn envelope(t: f32, len: f32, fade: f32) -> f32 {
    smoothstep(0.0, fade, t) * smoothstep(len, len - fade, t)
}

fn rubber(o: &Oval, parts: &mut Parts) {
    let st = o.start();
    let y = TOP + 0.005;
    // Skid marks: braking into each turn, sliding through it, lock-ups here and there.
    {
        let b = parts.get(Surface::Skid);
        b.color(Vec4::ONE);
        let mut n = 0u32;
        let mut mark = |b: &mut Builder,
                        s0: f32,
                        len: f32,
                        d0: f32,
                        drift: f32,
                        wiggle: f32,
                        alpha: f32,
                        wheels: u32| {
            n += 1;
            let seed = n * 7919;
            let steps = (len / 0.8).ceil() as usize;
            let offsets: &[f32] = if wheels == 4 {
                &[-0.8, 0.8, -0.78, 0.82]
            } else {
                &[-0.8, 0.8]
            };
            for (wi, off) in offsets.iter().enumerate() {
                let a_w = alpha * (0.75 + 0.25 * rand1((seed + wi as u32) as i32, 5));
                let shift = if wi >= 2 { 0.15 } else { 0.0 };
                let path: Vec<(Vec3, f32)> = (0..=steps)
                    .map(|i| {
                        let t = i as f32 / steps as f32 * len;
                        let d = d0
                            + off
                            + shift
                            + drift * (t / len).powi(2)
                            + wiggle * (t * 0.35 + seed as f32).sin();
                        (o.at(s0 + t, d, y), a_w * envelope(t, len, 2.5))
                    })
                    .collect();
                ribbon(b, &path, 0.23);
            }
        };
        for k in 0..2u32 {
            let apex = o.apex(k);
            for j in 0..7u32 {
                let r = rand2(k as i32, j as i32, 31);
                let s0 = apex - 120.0 + r * 55.0;
                let len = 18.0 + 30.0 * rand2(j as i32, k as i32, 32);
                let d0 = (rand2(j as i32, 9, 33) - 0.5) * 1.6;
                mark(
                    b,
                    s0,
                    len,
                    d0,
                    0.6 * (rand1(j as i32, 34) - 0.3),
                    0.04,
                    0.55,
                    2,
                );
            }
            for j in 0..5u32 {
                let r = rand2(k as i32, j as i32 + 20, 35);
                let s0 = apex - 35.0 + r * 70.0;
                let len = 25.0 + 30.0 * rand2(j as i32, k as i32 + 4, 36);
                let d0 = (rand2(j as i32, 19, 37) - 0.3) * 2.0;
                mark(b, s0, len, d0, 2.2, 0.25, 0.4, 4);
            }
        }
        for j in 0..6u32 {
            let s0 = rand1(j as i32, 41) * o.length;
            let len = 6.0 + 10.0 * rand1(j as i32, 42);
            let d0 = (rand1(j as i32, 43) - 0.5) * 6.0;
            mark(b, s0, len, d0, 0.3, 0.02, 0.7, 2);
        }
        // Landing scuffs after the kicker.
        let jump = o.jump();
        for j in 0..5u32 {
            let s0 = jump + 14.0 + 7.0 * rand1(j as i32, 44);
            let d0 = (rand1(j as i32, 45) - 0.5) * 1.2;
            mark(
                b,
                s0,
                4.0 + 5.0 * rand1(j as i32, 46),
                d0,
                0.2,
                0.0,
                0.85,
                2,
            );
        }
        // Burnout marks on the grid.
        for k in 0..4 {
            let s = st - 9.0 - k as f32 * 8.0 - 1.0;
            let dc = if k % 2 == 0 { -2.7 } else { 2.7 };
            mark(b, s, 9.0, dc, 0.1, 0.0, 0.5, 2);
        }
        // Rubber on the ramp: the wheels' paths up the slope.
        let (c, rot) = ramp_frame(o);
        for off in [-0.8f32, 0.8] {
            let path: Vec<(Vec3, f32)> = (0..=8)
                .map(|i| {
                    let z = 3.0 - i as f32 * 0.75;
                    (
                        c + rot * Vec3::new(off, 0.3 + 0.006, z),
                        0.55 * envelope(3.0 - z, 6.0, 0.6),
                    )
                })
                .collect();
            ribbon(b, &path, 0.24);
        }
    }
    // Patch repairs: rectangles of fresh, darker asphalt with sealed edges.
    {
        let mut patches = Vec::new();
        for j in 0..14u32 {
            let s0 = rand1(j as i32, 51) * o.length;
            if o.delta(s0, st).abs() < 25.0 {
                continue;
            }
            let len = 3.0 + 9.0 * rand1(j as i32, 52);
            let w = 2.0 + 4.0 * rand1(j as i32, 53);
            let d0 = -HALF + 0.6 + (TRACK_WIDTH - 1.2 - w) * rand1(j as i32, 54);
            patches.push((s0, len, d0, w));
        }
        let b = parts.get(Surface::AsphaltPatch);
        for &(s0, len, d0, w) in &patches {
            strip(b, o, s0, s0 + len, d0, d0 + w, TOP + 0.002, 1.0);
        }
        let t = parts.get(Surface::Tar);
        t.color(Vec4::ONE);
        for &(s0, len, d0, w) in &patches {
            let e = 0.05;
            strip(t, o, s0 - e, s0 + e, d0 - e, d0 + w + e, TOP + 0.0035, 1.0);
            strip(
                t,
                o,
                s0 + len - e,
                s0 + len + e,
                d0 - e,
                d0 + w + e,
                TOP + 0.0035,
                1.0,
            );
            strip(t, o, s0, s0 + len, d0 - e, d0 + e, TOP + 0.0035, 1.0);
            strip(
                t,
                o,
                s0,
                s0 + len,
                d0 + w - e,
                d0 + w + e,
                TOP + 0.0035,
                1.0,
            );
        }
        // A sealed longitudinal paving joint and transverse cracks.
        let mut s = 0.0;
        while s < o.length {
            let len = 30.0 + 50.0 * rand1(s as i32, 55);
            if rand1(s as i32, 56) < 0.6 {
                let path: Vec<(Vec3, f32)> = (0..=(len / 1.0) as usize)
                    .map(|i| {
                        let ss = s + i as f32;
                        let d = 1.9 + 0.05 * (ss * 0.7).sin() + 0.03 * (ss * 2.3).sin();
                        (o.at(ss, d, TOP + 0.0035), envelope(i as f32, len, 1.0))
                    })
                    .collect();
                ribbon(t, &path, 0.07);
            }
            s += len + 20.0 * rand1(s as i32, 57);
        }
        for j in 0..26u32 {
            let s0 = rand1(j as i32, 58) * o.length;
            let span = 2.0 + 8.0 * rand1(j as i32, 59);
            let d0 = -HALF + (TRACK_WIDTH - span) * rand1(j as i32, 60);
            let path: Vec<(Vec3, f32)> = (0..=(span / 0.25) as usize)
                .map(|i| {
                    let d = d0 + i as f32 * 0.25;
                    let ds = 0.25 * (d * 1.3 + j as f32).sin() + 0.1 * (d * 4.1).sin();
                    (
                        o.at(s0 + ds, d, TOP + 0.0035),
                        envelope(i as f32 * 0.25, span, 0.4),
                    )
                })
                .collect();
            // Ribbon across the track: build along d with the ribbon's own side vector.
            ribbon(t, &path, 0.05);
        }
    }
}

// ---------------------------------------------------------------- curbs

fn curbs(o: &Oval, parts: &mut Parts) {
    let st = o.start();
    let n = (o.length / 1.2).round() as usize;
    let len = o.length / n as f32;
    let red = Vec4::new(0.62, 0.035, 0.03, 1.0);
    let wht = Vec4::new(0.86, 0.86, 0.84, 1.0);
    let b = parts.get(Surface::Curb);
    for side in [1.0f32, -1.0] {
        for i in 0..n {
            let s0 = i as f32 * len;
            let rel = o.delta(s0 + len * 0.5, st);
            // The pit lane crosses the inside curb at its entry and exit.
            if side < 0.0
                && ((pit::ENTRY..pit::ENTRY_END - 5.0).contains(&rel)
                    || (pit::EXIT + 5.0..pit::EXIT_END).contains(&rel))
            {
                continue;
            }
            let rumble = o.turn(s0) > 0.25;
            let frames = if rumble { 19 } else { 2 };
            b.color(if i % 2 == 0 { red } else { wht });
            curb_piece(b, o, side, s0, s0 + len, frames, rumble);
        }
    }
}

/// One curb block between `s0` and `s1` on `side` (+1 outside, −1 inside).
fn curb_piece(b: &mut Builder, o: &Oval, side: f32, s0: f32, s1: f32, frames: usize, rumble: bool) {
    // Profile across the curb (meters beyond the asphalt edge, height above ground).
    let prof: [(f32, f32, f32); 7] = if rumble {
        [
            (0.0, TOP, 0.0),
            (0.1, TOP + 0.004, 0.6),
            (0.22, TOP + 0.006, 1.0),
            (0.8, TOP + 0.006, 1.0),
            (0.91, TOP + 0.004, 0.5),
            (0.985, TOP - 0.01, 0.0),
            (1.0, 0.0, 0.0),
        ]
    } else {
        [
            (0.0, TOP, 0.0),
            (0.08, TOP + 0.007, 0.0),
            (0.2, TOP + 0.01, 0.0),
            (0.8, TOP + 0.01, 0.0),
            (0.92, TOP + 0.007, 0.0),
            (0.985, TOP - 0.01, 0.0),
            (1.0, 0.0, 0.0),
        ]
    };
    let np = prof.len();
    let mut pos = vec![Vec3::ZERO; frames * np];
    for f in 0..frames {
        let s = s0 + (s1 - s0) * f as f32 / (frames - 1) as f32;
        let ridge = if rumble {
            0.5 - 0.5 * (s / 0.2 * TAU).cos()
        } else {
            0.0
        };
        for (j, &(dx, y, r)) in prof.iter().enumerate() {
            let d = side * (CURB.0 + dx);
            // Ridges 15 mm tall: the collider's top (asphalt + 10 mm) is their mean.
            pos[f * np + j] = o.at(s, d, y + r * ridge * 0.015);
        }
    }
    let base = b.mesh.vertices.len() as u32;
    for f in 0..frames {
        for j in 0..np {
            let p = pos[f * np + j];
            let along = pos[(f + 1).min(frames - 1) * np + j] - pos[f.saturating_sub(1) * np + j];
            let across = pos[f * np + (j + 1).min(np - 1)] - pos[f * np + j.saturating_sub(1)];
            let mut n = along.cross(across).normalize_or(Vec3::Y);
            if n.y < 0.0 {
                n = -n;
            }
            let s = s0 + (s1 - s0) * f as f32 / (frames - 1) as f32;
            let mut c = b.color;
            // Contact shadow on the back face.
            if j == np - 1 {
                c *= Vec4::new(0.55, 0.55, 0.55, 1.0);
            }
            b.vertex_c(p, n, Vec2::new(s, prof[j].0), c);
        }
    }
    for f in 0..frames as u32 - 1 {
        for j in 0..np as u32 - 1 {
            let k = base + f * np as u32 + j;
            b.quad_idx(k, k + 1, k + np as u32 + 1, k + np as u32);
        }
    }
}

// ---------------------------------------------------------------- run-off

fn runoff(o: &Oval, parts: &mut Parts) {
    // Gravel traps outside both turns.
    let b = parts.get(Surface::Gravel);
    for k in 0..2 {
        let apex = o.apex(k);
        let (mut s0, mut s1) = (apex, apex);
        while gravel_width(o, s0 - 1.0) > 0.8 {
            s0 -= 1.0;
        }
        while gravel_width(o, s1 + 1.0) > 0.8 {
            s1 += 1.0;
        }
        let rows = ((s1 - s0) / 1.5).ceil() as usize;
        let cols = 24usize;
        let base = b.mesh.vertices.len() as u32;
        for r in 0..=rows {
            let s = s0 + (s1 - s0) * r as f32 / rows as f32;
            let w = gravel_width(o, s).max(0.0);
            for c in 0..=cols {
                let t = c as f32 / cols as f32;
                let d = CURB.1 + 0.35 + t * w;
                let edge = smoothstep(0.0, 0.03, t) * smoothstep(1.0, 0.97, t);
                let ends = smoothstep(0.8, 2.5, w);
                let p0 = o.at(s, d, 0.0);
                let bump = 0.015 * world_fbm(p0.x, p0.z, 1.3, 2, 61);
                let y = (0.05 + bump) * edge * ends;
                let p = Vec3::new(p0.x, y, p0.z);
                let tone = 0.92 + 0.12 * world_fbm(p.x, p.z, 4.0, 3, 62);
                // Dirtier near the edges where it mixes with soil, and where cars went off.
                let dirty = 1.0 - 0.25 * (1.0 - edge.min(ends));
                let v = tone * dirty;
                b.vertex_c(
                    p,
                    Vec3::Y,
                    Vec2::new(p.x, p.z),
                    Vec4::new(v, v * 0.98, v * 0.95, 1.0),
                );
            }
        }
        let w = cols as u32 + 1;
        for r in 0..rows as u32 {
            for c in 0..cols as u32 {
                let k = base + r * w + c;
                b.quad_idx(k, k + 1, k + w + 1, k + w);
            }
        }
    }
    // A strip of asphalt between the inside curb and the pit wall.
    let st = o.start();
    let a = parts.get(Surface::Asphalt);
    a.color(gray(0.92));
    strip_fn(
        a,
        o,
        st + pit::WALL.0,
        st + pit::WALL.1,
        TOP - 0.02,
        1.0,
        |s| {
            let (out, _) = pit_lane_edges(o, s);
            let _ = out;
            (pit::WALL_D + 0.3, -CURB.1 + 0.02)
        },
    );
    // The pit lane surface.
    a.color(gray(0.97));
    strip_fn(a, o, st + pit::ENTRY, st + pit::EXIT_END, TOP, 1.0, |s| {
        let (out, inn) = pit_lane_edges(o, s);
        (inn, out)
    });
}

// ---------------------------------------------------------------- barriers

/// W-beam rail profile (x toward the traffic from the post face, y up from the rail bottom).
const W_BEAM: [(f32, f32); 11] = [
    (0.0, 0.0),
    (0.035, 0.02),
    (0.08, 0.055),
    (0.083, 0.085),
    (0.045, 0.13),
    (0.04, 0.155),
    (0.045, 0.18),
    (0.083, 0.225),
    (0.08, 0.255),
    (0.035, 0.29),
    (0.0, 0.31),
];

/// Double Armco along `d(s)` from `s0` to `s1` (traffic on the side the frame's x points to).
fn armco(parts: &mut Parts, o: &Oval, s0: f32, s1: f32, d: impl Fn(f32) -> f32, facing_out: bool) {
    let step = 2.0;
    let n = ((s1 - s0) / step).ceil().max(1.0) as usize;
    let frames: Vec<Frame> = (0..=n)
        .map(|i| {
            let s = s0 + (s1 - s0) * i as f32 / n as f32;
            o.frame(s, d(s), 0.0, facing_out)
        })
        .collect();
    let g = parts.get(Surface::Galvanized);
    g.color(Vec4::ONE);
    for rail_y in [0.48f32, 0.93] {
        let profile: Vec<Vec2> = W_BEAM
            .iter()
            .map(|&(x, y)| Vec2::new(0.2 + x, rail_y + y))
            .collect();
        g.sweep(&profile, &[false; 11], &frames, false);
    }
    for f in &frames {
        // Posts (sigma section, as a box) and spacer blocks.
        let up = f.up;
        let fwd = f.side.cross(up);
        let rot = Quat::from_mat3(&engine_core::math::Mat3::from_cols(f.side, up, -fwd));
        g.chamfer_box(f.pos + up * 0.6, Vec3::new(0.05, 0.62, 0.075), 0.01, rot);
        for rail_y in [0.48f32, 0.93] {
            g.cuboid(
                f.pos + f.side * 0.13 + up * (rail_y + 0.155),
                Vec3::new(0.07, 0.11, 0.05),
                rot,
            );
        }
    }
}

fn barriers(o: &Oval, parts: &mut Parts) {
    let st = o.start();
    let l = o.length;
    // Outer: Armco everywhere except along the main straight (concrete wall + fence).
    let wall0 = st - pit::OUTER_WALL;
    let wall1 = st + pit::OUTER_WALL;
    armco(
        parts,
        o,
        wall1 - 2.0,
        wall0 + l + 2.0,
        |s| outer_barrier(o, s),
        false,
    );
    // Inner: Armco except along the pit lane.
    let inner0 = st + pit::EXIT_END + 3.0;
    let inner1 = st + pit::ENTRY - 3.0 + l;
    armco(parts, o, inner0, inner1, |_| -11.0, true);
    concrete_wall(parts, o, wall0, wall1, 10.0, false, true);
    // Pit wall with its debris fence.
    concrete_wall(
        parts,
        o,
        st + pit::WALL.0,
        st + pit::WALL.1,
        pit::WALL_D,
        true,
        true,
    );
    tire_walls(o, parts);
}

/// A run of 4 m precast concrete barrier blocks (FIA profile, 1.05 m tall) at `d`, with an
/// optional catch fence on top. `toward_out`: the traffic is on the +d side.
fn concrete_wall(
    parts: &mut Parts,
    o: &Oval,
    s0: f32,
    s1: f32,
    d: f32,
    toward_out: bool,
    fence: bool,
) {
    let prof = [
        Vec2::new(-0.3, 0.0),
        Vec2::new(-0.3, 0.08),
        Vec2::new(-0.2, 0.33),
        Vec2::new(-0.13, 1.02),
        Vec2::new(-0.1, 1.05),
        Vec2::new(0.1, 1.05),
        Vec2::new(0.13, 1.02),
        Vec2::new(0.2, 0.33),
        Vec2::new(0.3, 0.08),
        Vec2::new(0.3, 0.0),
    ];
    let sharp = [
        true, true, false, true, false, false, true, false, true, true,
    ];
    let n = ((s1 - s0) / 4.0).round().max(1.0) as usize;
    let len = (s1 - s0) / n as f32;
    let b = parts.get(Surface::Concrete);
    for i in 0..n {
        let a = s0 + i as f32 * len + 0.015;
        let e = a + len - 0.03;
        let frames: Vec<Frame> = (0..=2)
            .map(|k| o.frame(a + (e - a) * k as f32 / 2.0, d, 0.0, !toward_out))
            .collect();
        // Rubber scuffs and grime low on the traffic face (vertex tint per block).
        let tone = 0.9 + 0.12 * rand1(i as i32, 71);
        b.color(gray(tone));
        b.sweep(&prof, &sharp, &frames, false);
        b.sweep_caps(&prof, &frames[0], &frames[2]);
    }
    if !fence {
        return;
    }
    // Catch fence: posts every 4 m leaning toward the traffic at the top, mesh panels, cables.
    let posts = ((s1 - s0) / 4.0).round().max(1.0) as usize;
    let toward = if toward_out { 1.0 } else { -1.0 };
    {
        let g = parts.get(Surface::Galvanized);
        for i in 0..=posts {
            let s = s0 + (s1 - s0) * i as f32 / posts as f32;
            let base = o.at(s, d, 1.05);
            let knee = base + Vec3::Y * 2.9;
            let top = knee + Vec3::Y * 0.75 + o.out(s) * toward * 0.45;
            g.cylinder(base, knee, 0.055, 0.05, 8, false);
            g.cylinder(knee, top, 0.05, 0.045, 8, true);
        }
        for h in [1.25f32, 2.4, 3.6] {
            let pts: Vec<Vec3> = (0..=posts * 2)
                .map(|i| {
                    let s = s0 + (s1 - s0) * i as f32 / (posts * 2) as f32;
                    o.at(s, d, 1.05 + h - 1.05) + Vec3::Y * 1.05 - Vec3::Y * 1.05
                })
                .collect();
            for w in pts.windows(2) {
                g.cylinder(w[0], w[1], 0.012, 0.012, 4, false);
            }
        }
    }
    let f = parts.get(Surface::Fence);
    let n = posts * 2;
    let base = f.mesh.vertices.len() as u32;
    for i in 0..=n {
        let s = s0 + (s1 - s0) * i as f32 / n as f32;
        let out = o.out(s);
        let p0 = o.at(s, d, 1.1);
        let p1 = o.at(s, d, 3.95);
        let p2 = p1 + Vec3::Y * 0.7 + out * toward * 0.42;
        let n0 = out * toward;
        for (p, v) in [(p0, 0.0), (p1, 2.85), (p2, 3.67)] {
            f.vertex(p, n0, Vec2::new(s, v));
        }
    }
    for i in 0..n as u32 {
        let k = base + i * 3;
        f.quad_idx(k, k + 3, k + 4, k + 1);
        f.quad_idx(k + 1, k + 4, k + 5, k + 2);
    }
}

/// Tire walls (3-high stacks, conveyor belt in front) in front of the Armco in the turns.
fn tire_walls(o: &Oval, parts: &mut Parts) {
    let tire_top = tire_mesh(true);
    let tire_low = tire_mesh(false);
    let mut placements: Vec<(Mat4, bool)> = Vec::new();
    let mut belts: Vec<Vec<Vec3>> = Vec::new();
    for k in 0..2 {
        let apex = o.apex(k);
        let (mut s0, mut s1) = (apex, apex);
        // Tire walls only where an off is most likely: 70 m round the apex of each turn.
        s0 -= 35.0;
        s1 += 35.0;
        let mut belt = Vec::new();
        let mut s = s0;
        let mut i = 0u32;
        while s < s1 {
            let d = outer_barrier(o, s) - 0.42;
            let p = o.at(s, d, 0.0);
            for row in 0..2 {
                let q = p - o.out(s) * (row as f32 * 0.6);
                for h in 0..3 {
                    let jitter = Vec3::new(
                        (rand2(i as i32, h, 81) - 0.5) * 0.04,
                        0.0,
                        (rand2(i as i32, h, 82) - 0.5) * 0.04,
                    );
                    let m = Mat4::from_rotation_translation(
                        Quat::from_rotation_y(rand2(i as i32, h, 83) * TAU),
                        q + jitter + Vec3::Y * (h as f32 * 0.21),
                    );
                    placements.push((m, h == 2 && row == 0));
                }
            }
            belt.push(p - o.out(s) * 1.0);
            s += 0.62 * (outer_barrier(o, s) / (outer_barrier(o, s) + o.radius(s))).max(0.5) + 0.0;
            s += 0.0;
            i += 1;
        }
        belts.push(belt);
    }
    let r = parts.get(Surface::Rubber);
    r.color(Vec4::ONE);
    for (i, (m, top)) in placements.iter().enumerate() {
        // Every fourth top tire painted white, the rest black.
        let mesh = if *top { &tire_top } else { &tire_low };
        let start = r.mesh.vertices.len();
        r.add_mesh(mesh, m);
        if *top && (i / 6) % 4 == 0 {
            for v in &mut r.mesh.vertices[start..] {
                v.color = [5.0, 5.0, 5.0, 1.0];
            }
        }
    }
    // Belts: rubber sheet 1 m tall bulging over the stacks.
    for belt in belts {
        if belt.len() < 2 {
            continue;
        }
        let base = r.mesh.vertices.len() as u32;
        let mut s = 0.0;
        for i in 0..belt.len() {
            let a = belt[i.saturating_sub(1)];
            let c = belt[(i + 1).min(belt.len() - 1)];
            let along = (c - a).normalize_or(Vec3::X);
            let n = along.cross(Vec3::Y).normalize();
            let bulge = n * 0.03 * ((i as f32) * PI).cos();
            if i > 0 {
                s += (belt[i] - belt[i - 1]).length();
            }
            r.vertex_c(
                belt[i] + bulge,
                n,
                Vec2::new(s, 0.0),
                Vec4::new(1.4, 1.4, 1.4, 1.0) * 0.55,
            );
            r.vertex_c(
                belt[i] + bulge + Vec3::Y * 0.95,
                n,
                Vec2::new(s, 0.95),
                Vec4::splat(1.0),
            );
        }
        for i in 0..belt.len() as u32 - 1 {
            let k = base + i * 2;
            r.quad_idx(k, k + 2, k + 3, k + 1);
            // Back face.
        }
        let base2 = r.mesh.vertices.len() as u32;
        let mut s = 0.0;
        for i in 0..belt.len() {
            let a = belt[i.saturating_sub(1)];
            let c = belt[(i + 1).min(belt.len() - 1)];
            let along = (c - a).normalize_or(Vec3::X);
            let n = -along.cross(Vec3::Y).normalize();
            if i > 0 {
                s += (belt[i] - belt[i - 1]).length();
            }
            r.vertex(belt[i] - n * 0.01, n, Vec2::new(s, 0.0));
            r.vertex(belt[i] - n * 0.01 + Vec3::Y * 0.95, n, Vec2::new(s, 0.95));
        }
        for i in 0..belt.len() as u32 - 1 {
            let k = base2 + i * 2;
            r.quad_idx(k, k + 2, k + 3, k + 1);
        }
    }
    // Sponsor banners strapped to the belts.
    let sg = parts.get(Surface::Signs);
    sg.color(Vec4::ONE);
    let banners = ["TITAN TYRES", "APEX OIL", "KESTREL", "NORDLINE"];
    for k in 0..2 {
        let apex = o.apex(k);
        let mut s = apex - 33.0;
        let mut i = 0;
        while s < apex + 32.0 {
            let d = outer_barrier(o, s) - 0.42 - 1.0 - 0.06;
            // Distance along the center line per meter at the belt.
            let k_len = o.radius(s) / (o.radius(s) + d);
            let ds = 2.88 * k_len;
            let idx = tex::board_index(banners[i % banners.len()]).unwrap_or(0);
            board_on_track(sg, o, s, s + ds, d, 0.12, 0.84, idx, false);
            s += ds + 0.6 * k_len;
            i += 1;
        }
    }
}

/// A car tire lying flat (axis up), 0.62 m across, 0.21 m tall, bottom at y = 0. `full`
/// adds the top sidewall and inner bead (only top tires show them).
fn tire_mesh(full: bool) -> engine_render::Mesh {
    let mut b = Builder::new();
    let prof: &[(f32, f32)] = if full {
        &[
            (0.2, 0.2),
            (0.24, 0.21),
            (0.29, 0.205),
            (0.31, 0.17),
            (0.31, 0.04),
            (0.29, 0.005),
            (0.24, 0.0),
        ]
    } else {
        &[(0.305, 0.2), (0.315, 0.105), (0.305, 0.01)]
    };
    let seg = if full { 12u32 } else { 9 };
    let np = prof.len() as u32;
    for i in 0..=seg {
        let a = i as f32 / seg as f32 * TAU;
        let (sn, cs) = a.sin_cos();
        for (j, &(r, y)) in prof.iter().enumerate() {
            let prev = prof[j.saturating_sub(1)];
            let next = prof[(j + 1).min(prof.len() - 1)];
            let dr = next.0 - prev.0;
            let dy = next.1 - prev.1;
            // Normal in the (r, y) plane, pointing away from the tire's section center.
            let mut n2 = Vec2::new(-dy, dr).normalize_or(Vec2::X);
            if (Vec2::new(r - 0.255, y - 0.105)).dot(n2) < 0.0 {
                n2 = -n2;
            }
            let n = Vec3::new(cs * n2.x, n2.y, sn * n2.x);
            let tone = 1.0 - 0.12 * ((j as f32 * 1.7 + i as f32 * 0.3).sin() * 0.5 + 0.5);
            b.vertex_c(
                Vec3::new(cs * r, y, sn * r),
                n,
                Vec2::new(a * r, y),
                Vec4::new(tone, tone, tone, 1.0),
            );
        }
    }
    for i in 0..seg {
        for j in 0..np - 1 {
            let k = i * np + j;
            b.quad_idx(k, k + 1, k + np + 1, k + np);
        }
    }
    b.finish()
}

// ---------------------------------------------------------------- grandstand

fn grandstand(o: &Oval, parts: &mut Parts) {
    let st = o.start();
    let (s0, s1) = (st - pit::STAND, st + pit::STAND);
    let rows = 16usize;
    let (d_front, tread, rise, y0) = (13.25f32, 0.85f32, 0.42f32, 2.2f32);
    let d_back = d_front + tread * rows as f32;
    let y_top = y0 + rise * (rows - 1) as f32;
    let frames: Vec<Frame> = (0..=26)
        .map(|i| o.frame(s0 + (s1 - s0) * i as f32 / 26.0, 0.0, 0.0, true))
        .collect();
    // Stepped deck: front parapet, treads and risers, back wall top.
    let mut prof = vec![
        Vec2::new(d_front - 0.25, 0.0),
        Vec2::new(d_front - 0.25, y0 + 0.95),
        Vec2::new(d_front, y0 + 0.95),
        Vec2::new(d_front, y0),
    ];
    for r in 0..rows {
        let d = d_front + r as f32 * tread;
        let y = y0 + r as f32 * rise;
        if r > 0 {
            prof.push(Vec2::new(d, y));
        }
        prof.push(Vec2::new(d + tread, y));
        if r + 1 < rows {
            prof.push(Vec2::new(d + tread, y + rise));
        }
    }
    prof.push(Vec2::new(d_back, y_top + 1.1));
    prof.push(Vec2::new(d_back + 0.25, y_top + 1.1));
    prof.push(Vec2::new(d_back + 0.25, 0.0));
    // The profile runs outward over the top: its "left" is up/toward the track. Sweep with
    // x = outward.
    let sharp = vec![true; prof.len()];
    {
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.95));
        c.sweep(&prof, &sharp, &frames, true);
        // End walls: one vertical quad per step down to the ground.
        for (f, sgn) in [(&frames[0], -1.0f32), (frames.last().unwrap(), 1.0)] {
            let along = f.side.cross(f.up) * -sgn;
            for w in prof.windows(2) {
                let (a, b) = (w[0], w[1]);
                if (b.x - a.x).abs() < 1e-4 {
                    continue;
                }
                let top_a = f.at(a);
                let top_b = f.at(b);
                let q = [
                    Vec3::new(top_a.x, 0.0, top_a.z),
                    Vec3::new(top_b.x, 0.0, top_b.z),
                    top_b,
                    top_a,
                ];
                let n = along.normalize();
                let uv = q.map(|p| Vec2::new(p.dot(f.side), p.y));
                c.quad_n(q, uv, n);
            }
        }
    }
    // Seats: blue with the circuit sponsor spelled in white across the middle rows.
    let seat = seat_mesh();
    let pattern_text = "VELOCE";
    let p = parts.get(Surface::Plastic);
    let blue = hex(0x1d4a9e);
    let white = Vec4::new(0.85, 0.86, 0.88, 1.0);
    let red = hex(0xb51d1d);
    let length = s1 - s0;
    let per_row = (length / 0.5) as usize;
    for r in 0..rows {
        let d = d_front + r as f32 * tread + 0.42;
        let y = y0 + r as f32 * rise;
        for i in 0..per_row {
            let s = s0 + 0.5 + i as f32 * 0.5;
            // Aisles every 13 m.
            let rel = s - s0;
            if (rel % 13.0) < 1.3 || rel > length - 0.6 {
                continue;
            }
            // Text in seat colors: map (i, r) to glyph units.
            let gx = (i as f32
                - (per_row as f32 * 0.5 - super::font::text_width(pattern_text) * 0.9))
                / 1.8;
            let gy = (rows as f32 - 2.0 - r as f32) * 0.62 - 0.6;
            let ink = super::font::text_coverage(pattern_text, gx, gy, 1.3, 0.3);
            let col = if ink > 0.5 {
                white
            } else if (rel / 13.0) as i32 % 4 == 0 {
                red
            } else {
                blue
            };
            p.color(col);
            let pos = o.at(s, d, y);
            let out = o.out(s);
            let rot = Quat::from_rotation_arc(Vec3::Z, -out);
            let m = Mat4::from_rotation_translation(rot, pos);
            let start = p.mesh.vertices.len();
            p.add_mesh(&seat, &m);
            let sun = 0.9 + 0.1 * rand2(i as i32, r as i32, 91);
            for v in &mut p.mesh.vertices[start..] {
                v.color = [col.x * sun, col.y * sun, col.z * sun, 1.0];
            }
        }
    }
    // Roof: cantilevered sheet on columns at the back, trusses to the front edge.
    let roof_front = (d_front - 1.0, 15.2f32);
    let roof_back = (d_back + 0.6, 14.2f32);
    {
        let c = parts.get(Surface::Cladding);
        c.color(gray(0.78));
        let n = 26;
        let base = c.mesh.vertices.len() as u32;
        for i in 0..=n {
            let s = s0 - 1.0 + (length + 2.0) * i as f32 / n as f32;
            let a = o.at(s, roof_front.0, roof_front.1);
            let b2 = o.at(s, roof_back.0, roof_back.1);
            let up = Vec3::Y;
            c.vertex(a, up, Vec2::new(0.0, s));
            c.vertex(b2, up, Vec2::new(roof_back.0 - roof_front.0, s));
            c.vertex(a - Vec3::Y * 0.06, -up, Vec2::new(0.0, s));
            c.vertex(
                b2 - Vec3::Y * 0.06,
                -up,
                Vec2::new(roof_back.0 - roof_front.0, s),
            );
        }
        for i in 0..n as u32 {
            let k = base + i * 4;
            c.quad_idx(k, k + 1, k + 5, k + 4);
            c.quad_idx(k + 2, k + 3, k + 7, k + 6);
        }
        // Back wall cladding of the concourse.
        c.color(gray(0.55));
        let base = c.mesh.vertices.len() as u32;
        for i in 0..=n {
            let s = s0 + length * i as f32 / n as f32;
            let p0 = o.at(s, d_back + 0.27, 0.0);
            let p1 = o.at(s, d_back + 0.27, y_top + 1.1);
            let out = o.out(s);
            c.vertex(p0, out, Vec2::new(s, 0.0));
            c.vertex(p1, out, Vec2::new(s, y_top + 1.1));
        }
        for i in 0..n as u32 {
            let k = base + i * 2;
            c.quad_idx(k, k + 2, k + 3, k + 1);
        }
    }
    {
        let g = parts.get(Surface::Painted);
        g.color(gray(0.82));
        let mut s = s0;
        while s <= s1 + 0.01 {
            let base = o.at(s, d_back + 0.5, 0.0);
            let top = o.at(s, d_back + 0.5, roof_back.1 + 1.6);
            g.chamfer_box(
                (base + top) * 0.5,
                Vec3::new(0.22, (top.y - base.y) * 0.5, 0.22),
                0.03,
                Quat::IDENTITY,
            );
            // Truss: top and bottom chords to the front edge, with diagonals.
            let tip = o.at(s, roof_front.0, roof_front.1 + 0.05);
            let tip_low = o.at(s, roof_front.0 + 0.6, roof_front.1 - 0.35);
            let heel_low = o.at(s, d_back + 0.5, roof_back.1 - 1.9);
            let top_c = top;
            g.beam(top_c, tip, 0.2, 0.25, 0.02);
            g.beam(heel_low, tip_low, 0.18, 0.18, 0.02);
            for k in 0..6 {
                let t0 = k as f32 / 6.0;
                let t1 = (k + 1) as f32 / 6.0;
                let a = top_c.lerp(tip, t0);
                let b = heel_low.lerp(tip_low, t1);
                let c2 = heel_low.lerp(tip_low, t0);
                g.beam(a, b, 0.09, 0.09, 0.0);
                g.beam(a, c2, 0.09, 0.09, 0.0);
            }
            s += 13.0;
        }
        // Front handrail and aisle rails.
        let rail: Vec<Vec3> = (0..=52)
            .map(|i| o.at(s0 + length * i as f32 / 52.0, d_front - 0.12, y0 + 1.05))
            .collect();
        for w in rail.windows(2) {
            g.cylinder(w[0], w[1], 0.025, 0.025, 6, false);
        }
    }
    // Fascia boards along the roof's front edge.
    let sg = parts.get(Surface::Signs);
    let boards = [
        "VELOCE",
        "APEX OIL",
        "KESTREL",
        "TITAN TYRES",
        "NORDLINE",
        "HELIX",
        "PULSAR",
        "MERIDIAN",
    ];
    let n = 18;
    for i in 0..n {
        let a = s0 + length * i as f32 / n as f32;
        let e = a + length / n as f32 - 0.1;
        let idx = tex::board_index(boards[i % boards.len()]).unwrap_or(0);
        board_on_track(
            sg,
            o,
            a,
            e,
            roof_front.0 - 0.02,
            roof_front.1 - 1.3,
            roof_front.1,
            idx,
            false,
        );
        // Two rows of boards along the front of the stand.
        board_on_track(
            sg,
            o,
            a,
            e,
            d_front - 0.27,
            0.25,
            1.5,
            (idx + 3) % 12,
            false,
        );
        board_on_track(
            sg,
            o,
            a,
            e,
            d_front - 0.27,
            1.62,
            3.0,
            (idx + 7) % 12,
            false,
        );
    }
}

/// A sign board following the track from `s0` to `s1` at lateral `d`, `y0..y1` high, facing
/// inward (toward the track from outside, `outward_face` = false) or outward.
#[allow(clippy::too_many_arguments)]
fn board_on_track(
    b: &mut Builder,
    o: &Oval,
    s0: f32,
    s1: f32,
    d: f32,
    y0: f32,
    y1: f32,
    board: usize,
    outward_face: bool,
) {
    let uv = tex::board_uv(board);
    let n = 4;
    let base = b.mesh.vertices.len() as u32;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let s = s0 + (s1 - s0) * t;
        let nrm = if outward_face { o.out(s) } else { -o.out(s) };
        // Text reads left to right as seen from the track side.
        let u = if outward_face {
            mix(uv[2], uv[0], t)
        } else {
            mix(uv[0], uv[2], t)
        };
        let u = if outward_face {
            u
        } else {
            mix(uv[2], uv[0], t) * 0.0 + u
        };
        b.vertex(o.at(s, d, y0), nrm, Vec2::new(u, uv[3]));
        b.vertex(o.at(s, d, y1), nrm, Vec2::new(u, uv[1]));
    }
    for i in 0..n as u32 {
        let k = base + i * 2;
        b.quad_idx(k, k + 2, k + 3, k + 1);
    }
}

/// A stadium seat facing −Z (toward the track), origin at the tread under its front edge:
/// a moulded pan and backrest, each a prism of its side profile (y up, z back).
fn seat_mesh() -> engine_render::Mesh {
    let mut b = Builder::new();
    let pan = [
        Vec2::new(-0.21, 0.39),
        Vec2::new(0.16, 0.395),
        Vec2::new(0.15, 0.45),
        Vec2::new(-0.17, 0.455),
        Vec2::new(-0.215, 0.43),
    ];
    let back = [
        Vec2::new(0.13, 0.46),
        Vec2::new(0.19, 0.44),
        Vec2::new(0.27, 0.86),
        Vec2::new(0.24, 0.88),
        Vec2::new(0.205, 0.87),
    ];
    for poly in [&pan[..], &back[..]] {
        prism(&mut b, poly, -0.215, 0.215);
    }
    b.finish()
}

/// Extrude a convex polygon in the (z, y) plane (counter-clockwise seen from +x) between
/// `x0` and `x1`, with flat-shaded sides and caps.
fn prism(b: &mut Builder, poly: &[Vec2], x0: f32, x1: f32) {
    let n = poly.len();
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let d = q - p;
        // Outward normal of a counter-clockwise polygon in (z, y) seen from +x.
        let nrm = Vec3::new(0.0, -d.x, d.y).normalize_or(Vec3::Y);
        let c = [
            Vec3::new(x0, p.y, p.x),
            Vec3::new(x1, p.y, p.x),
            Vec3::new(x1, q.y, q.x),
            Vec3::new(x0, q.y, q.x),
        ];
        let len = d.length();
        b.quad_n(
            c,
            [
                Vec2::ZERO,
                Vec2::new(x1 - x0, 0.0),
                Vec2::new(x1 - x0, len),
                Vec2::new(0.0, len),
            ],
            nrm,
        );
    }
    for (x, nx) in [(x0, -1.0f32), (x1, 1.0)] {
        let idx: Vec<u32> = poly
            .iter()
            .map(|p| b.vertex(Vec3::new(x, p.y, p.x), Vec3::X * nx, Vec2::new(p.x, p.y)))
            .collect();
        for k in 1..n - 1 {
            b.tri_idx(idx[0], idx[k], idx[k + 1]);
        }
    }
}

// ---------------------------------------------------------------- pits

fn pits(o: &Oval, parts: &mut Parts) {
    let st = o.start();
    let (s0, s1) = (st - pit::BUILDING, st + pit::BUILDING);
    let bays = 12usize;
    let bay = (s1 - s0) / bays as f32;
    let (front, back) = (pit::FACADE, pit::BACK);
    let h1 = 5.2f32; // garage floor to first floor
    let h2 = 9.4f32; // roof
    let frames = |d: f32, y: f32| -> Vec<Frame> {
        (0..=24)
            .map(|i| o.frame(s0 + (s1 - s0) * i as f32 / 24.0, d, y, true))
            .collect()
    };
    // Concrete frame: columns between bays, first-floor slab with balcony, roof slab, back
    // wall.
    {
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.98));
        for i in 0..=bays {
            let s = s0 + i as f32 * bay;
            let p = o.at(s, front - 0.25, 0.0);
            let out = o.out(s);
            let rot = Quat::from_rotation_arc(Vec3::X, out);
            c.chamfer_box(
                p + Vec3::Y * h1 * 0.5,
                Vec3::new(0.25, h1 * 0.5, 0.3),
                0.02,
                rot,
            );
        }
        // Slabs as swept rectangles (outward-running profiles face up).
        let slab = |c: &mut Builder, d0: f32, d1: f32, y0: f32, y1: f32| {
            let prof = [
                Vec2::new(d0, y0),
                Vec2::new(d0, y1),
                Vec2::new(d1, y1),
                Vec2::new(d1, y0),
                Vec2::new(d0, y0),
            ];
            let f = frames(0.0, 0.0);
            c.sweep(&prof, &[true; 5], &f, true);
            c.sweep_caps(&prof[..4], &f[0], &f[f.len() - 1]);
        };
        slab(c, back, front + 1.8, h1, h1 + 0.35);
        slab(c, back, front + 0.6, h2, h2 + 0.45);
        // Back and end walls.
        c.color(gray(0.9));
        slab(c, back, back + 0.3, 0.0, h2);
        for (s, sgn) in [(s0, -1.0f32), (s1, 1.0)] {
            let f = o.frame(s, 0.0, 0.0, true);
            let along = f.side.cross(f.up) * -sgn;
            let q = [
                f.at(Vec2::new(back, 0.0)),
                f.at(Vec2::new(front, 0.0)),
                f.at(Vec2::new(front, h2)),
                f.at(Vec2::new(back, h2)),
            ];
            let uv = q.map(|p| Vec2::new(p.dot(f.side), p.y));
            c.quad_n(q, uv, along.normalize());
        }
    }
    // Garage doors (ribbed, light gray), every third open onto a dark interior.
    {
        for i in 0..bays {
            let a = s0 + i as f32 * bay + 0.25;
            let e = a + bay - 0.5;
            let open = i % 3 == 1;
            if open {
                let int = parts.get(Surface::Interior);
                int.color(Vec4::ONE);
                // Interior box: floor, back, sides, ceiling.
                let depth = 9.0;
                let quad = |int: &mut Builder, p: [Vec3; 4], n: Vec3| {
                    int.quad_n(p, [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y], n);
                };
                let d_in = front - depth;
                let pa = |s: f32, d: f32, y: f32| o.at(s, d, y);
                let out = o.out((a + e) * 0.5);
                quad(
                    int,
                    [
                        pa(a, d_in, 0.0),
                        pa(e, d_in, 0.0),
                        pa(e, d_in, h1 - 0.5),
                        pa(a, d_in, h1 - 0.5),
                    ],
                    out,
                );
                quad(
                    int,
                    [
                        pa(a, front, h1 - 0.5),
                        pa(e, front, h1 - 0.5),
                        pa(e, d_in, h1 - 0.5),
                        pa(a, d_in, h1 - 0.5),
                    ],
                    -Vec3::Y,
                );
                quad(
                    int,
                    [
                        pa(a, d_in, 0.0),
                        pa(a, front, 0.0),
                        pa(a, front, h1 - 0.5),
                        pa(a, d_in, h1 - 0.5),
                    ],
                    o.dir(a),
                );
                quad(
                    int,
                    [
                        pa(e, front, 0.0),
                        pa(e, d_in, 0.0),
                        pa(e, d_in, h1 - 0.5),
                        pa(e, front, h1 - 0.5),
                    ],
                    -o.dir(e),
                );
                let fl = parts.get(Surface::Slabs);
                fl.color(gray(0.8));
                let f0 = pa(a, d_in, TOP);
                let f1 = pa(e, d_in, TOP);
                let f2 = pa(e, front, TOP);
                let f3 = pa(a, front, TOP);
                let q = [f0, f1, f2, f3];
                fl.quad_n(q, q.map(|p| Vec2::new(p.x, p.z)), Vec3::Y);
                // Ceiling light strips.
                let l = parts.get(Surface::Lamp);
                for k in 0..2 {
                    let d = front - 2.5 - k as f32 * 4.0;
                    let m = (a + e) * 0.5;
                    let c = o.at(m, d, h1 - 0.55);
                    l.cuboid(
                        c,
                        Vec3::new(0.08, 0.03, (e - a) * 0.35),
                        Quat::from_rotation_arc(Vec3::Z, o.dir(m)),
                    );
                }
                // Door rolled up in its box above the opening.
                let cl = parts.get(Surface::Painted);
                cl.color(gray(0.6));
                let m = (a + e) * 0.5;
                cl.chamfer_box(
                    o.at(m, front - 0.35, h1 - 0.3),
                    Vec3::new(0.2, 0.25, (e - a) * 0.5),
                    0.03,
                    Quat::from_rotation_arc(Vec3::Z, o.dir(m)),
                );
            } else {
                let cl = parts.get(Surface::Cladding);
                cl.color(gray(0.82));
                let n = 4;
                let base = cl.mesh.vertices.len() as u32;
                for k in 0..=n {
                    let s = a + (e - a) * k as f32 / n as f32;
                    let out = o.out(s);
                    // Ribs run across the door: swap UVs so the cladding ribs are horizontal.
                    cl.vertex(o.at(s, front - 0.3, 0.0), out, Vec2::new(0.0, s));
                    cl.vertex(
                        o.at(s, front - 0.3, h1 - 0.4),
                        out,
                        Vec2::new((h1 - 0.4) * 2.5, s),
                    );
                }
                for k in 0..n as u32 {
                    let i0 = base + k * 2;
                    cl.quad_idx(i0, i0 + 2, i0 + 3, i0 + 1);
                }
            }
        }
    }
    // First floor glazing (set back), balcony glass balustrade, race control box on top.
    {
        let gl = parts.get(Surface::Glass);
        gl.color(Vec4::ONE);
        let face = |gl: &mut Builder, d: f32, y0: f32, y1: f32, sa: f32, sb: f32| {
            let n = 16;
            let base = gl.mesh.vertices.len() as u32;
            for k in 0..=n {
                let s = sa + (sb - sa) * k as f32 / n as f32;
                let out = o.out(s);
                gl.vertex(o.at(s, d, y0), out, Vec2::new(s, y0));
                gl.vertex(o.at(s, d, y1), out, Vec2::new(s, y1));
            }
            for k in 0..n as u32 {
                let i0 = base + k * 2;
                gl.quad_idx(i0, i0 + 2, i0 + 3, i0 + 1);
            }
        };
        face(gl, front - 0.6, h1 + 0.35, h2, s0, s1);
        face(gl, front + 1.75, h1 + 0.35, h1 + 1.45, s0, s1);
        face(gl, front + 0.2, h2 + 0.45, h2 + 3.6, st - 8.0, st + 8.0);
        // Mullions on the first floor.
        let m = parts.get(Surface::Painted);
        m.color(gray(0.25));
        let mut s = s0;
        while s <= s1 + 0.01 {
            let c = o.at(s, front - 0.55, (h1 + 0.35 + h2) * 0.5);
            m.cuboid(
                c,
                Vec3::new(0.05, (h2 - h1 - 0.35) * 0.5, 0.05),
                Quat::from_rotation_arc(Vec3::Z, o.dir(s)),
            );
            s += 2.0;
        }
        // Race control box frame and roof.
        let c = parts.get(Surface::Painted);
        c.color(gray(0.88));
        for (sa, sb) in [(st - 8.0, st + 8.0)] {
            let f0 = o.frame(sa, 0.0, 0.0, true);
            let f1 = o.frame(sb, 0.0, 0.0, true);
            let prof = [
                Vec2::new(front - 8.0, h2 + 3.6),
                Vec2::new(front - 8.0, h2 + 4.1),
                Vec2::new(front + 0.8, h2 + 4.1),
                Vec2::new(front + 0.8, h2 + 3.6),
                Vec2::new(front - 8.0, h2 + 3.6),
            ];
            let frames: Vec<Frame> = (0..=6)
                .map(|k| o.frame(sa + (sb - sa) * k as f32 / 6.0, 0.0, 0.0, true))
                .collect();
            c.sweep(&prof, &[true; 5], &frames, true);
            c.sweep_caps(&prof[..4], &f0, &f1);
            for s in [sa, sb] {
                let p = o.at(s, front + 0.2, h2 + 0.45);
                c.cuboid(
                    p + Vec3::Y * 1.6,
                    Vec3::new(0.12, 1.6, 0.12),
                    Quat::IDENTITY,
                );
                let q = o.at(s, front - 7.8, h2 + 0.45);
                c.cuboid(q + Vec3::Y * 1.6, Vec3::new(0.3, 1.6, 0.3), Quat::IDENTITY);
            }
            // Back of the control box (solid).
            let n = 8;
            let base = c.mesh.vertices.len() as u32;
            for k in 0..=n {
                let s = sa + (sb - sa) * k as f32 / n as f32;
                let inn = -o.out(s);
                c.vertex(o.at(s, front - 7.9, h2 + 0.45), inn, Vec2::new(s, 0.0));
                c.vertex(o.at(s, front - 7.9, h2 + 3.6), inn, Vec2::new(s, 3.0));
            }
            for k in 0..n as u32 {
                let i0 = base + k * 2;
                c.quad_idx(i0, i0 + 2, i0 + 3, i0 + 1);
            }
        }
    }
    // Team boards on the roof fascia and garage numbers above the doors.
    let sg = parts.get(Surface::Signs);
    let teams = ["KESTREL", "ORBITAL", "VELOCE", "HELIX", "QUANTA", "FULCRUM"];
    for i in 0..bays / 2 {
        let a = s0 + (i * 2) as f32 * bay + 0.3;
        let e = a + bay * 2.0 - 0.6;
        let idx = tex::board_index(teams[i % teams.len()]).unwrap_or(0);
        board_on_track(sg, o, a, e, front + 0.62, h2 - 0.05, h2 + 1.0, idx, true);
    }
    let g = parts.get(Surface::Glyphs);
    g.color(white());
    for i in 0..bays {
        let s = s0 + (i as f32 + 0.5) * bay;
        text_on_wall(
            g,
            o,
            &format!("{}", i + 1),
            s,
            front + 0.02,
            h1 - 0.15,
            0.55,
        );
    }
    // Pit wall stands: little roofed timing stands on the pit lane side of the wall.
    for k in 0..6 {
        let p = parts.get(Surface::Painted);
        let s = st - 45.0 + k as f32 * 18.0;
        let rot = Quat::from_rotation_arc(Vec3::Z, o.dir(s));
        let c = o.at(s, pit::WALL_D - 1.2, 0.0);
        p.color(gray(0.3));
        p.chamfer_box(c + Vec3::Y * 0.55, Vec3::new(0.6, 0.55, 2.4), 0.03, rot);
        p.color(gray(0.85));
        p.chamfer_box(c + Vec3::Y * 2.6, Vec3::new(1.0, 0.06, 2.8), 0.02, rot);
        for e in [-2.2f32, 2.2] {
            p.cuboid(
                c + o.dir(s) * e + Vec3::Y * 1.6,
                Vec3::new(0.04, 1.0, 0.04),
                rot,
            );
        }
        // A row of timing monitors in dark housings, facing the crew in the pit lane.
        for j in 0..4 {
            let m = c + o.dir(s) * ((j as f32 - 1.5) * 0.8) + Vec3::Y * 1.45;
            let p = parts.get(Surface::Painted);
            p.color(gray(0.08));
            p.chamfer_box(m + o.out(s) * 0.03, Vec3::new(0.04, 0.22, 0.36), 0.01, rot);
            let l = parts.get(Surface::Screen);
            l.cuboid(m - o.out(s) * 0.012, Vec3::new(0.005, 0.18, 0.32), rot);
        }
    }
    // Impact attenuator at the pit wall nose.
    let at = st + pit::WALL.0 - 1.6;
    let p = parts.get(Surface::Plastic);
    for k in 0..4 {
        let c = o.at(at + k as f32 * 0.45, pit::WALL_D, 0.0);
        p.color(if k % 2 == 0 {
            hex(0xf2c200)
        } else {
            hex(0x161616)
        });
        let mut b = Builder::new();
        b.cylinder(Vec3::ZERO, Vec3::Y * 0.95, 0.42, 0.4, 12, true);
        let m = Mat4::from_translation(c);
        let start = p.mesh.vertices.len();
        p.add_mesh(&b.finish(), &m);
        let col = p.color;
        for v in &mut p.mesh.vertices[start..] {
            v.color = col.to_array();
        }
    }
}

// ---------------------------------------------------------------- paddock

/// The paddock behind the pit building: a concrete apron with the teams' transporters
/// parked nose-out in a row, awnings, tool carts and two floodlight masts.
fn paddock(o: &Oval, parts: &mut Parts) {
    let st = o.start();
    let (d0, d1) = (pit::BACK + 0.1, pit::BACK - 30.0);
    let (s0, s1) = (st - pit::BUILDING - 4.0, st + pit::BUILDING + 4.0);
    let y = 0.03f32;
    {
        let s = parts.get(Surface::Slabs);
        s.color(Vec4::ONE);
        let n = 40u32;
        let m = 6u32;
        let base = s.mesh.vertices.len() as u32;
        for i in 0..=n {
            let ss = mix(s0, s1, i as f32 / n as f32);
            for j in 0..=m {
                let d = mix(d0, d1, j as f32 / m as f32);
                let p = o.at(ss, d, y);
                let k = 1.0 + 0.08 * world_fbm(p.x, p.z, 10.0, 3, 91);
                s.vertex_c(
                    p,
                    Vec3::Y,
                    Vec2::new(p.x, p.z),
                    Vec4::new(k, k, k * 0.98, 1.0),
                );
            }
        }
        let row = m + 1;
        for i in 0..n {
            for j in 0..m {
                let k = base + i * row + j;
                s.quad_idx(k, k + row, k + row + 1, k + 1);
            }
        }
        // A kerb round the far edge and the ends.
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.85));
        let prof = [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 0.15),
            Vec2::new(0.3, 0.15),
            Vec2::new(0.3, 0.0),
        ];
        let frames: Vec<Frame> = (0..=n)
            .map(|i| o.frame(mix(s0, s1, i as f32 / n as f32), d1, 0.0, true))
            .collect();
        c.sweep(&prof, &[true; 4], &frames, false);
    }
    // Parking bay lines.
    let teams = [
        "KESTREL", "ORBITAL", "VELOCE", "HELIX", "QUANTA", "FULCRUM", "APEX OIL",
    ];
    let gap = 14.0f32;
    let p = parts.get(Surface::PaintSlabs);
    p.color(white());
    let first = st - gap * (teams.len() as f32 - 1.0) * 0.5;
    for k in 0..=teams.len() {
        let s = first + (k as f32 - 0.5) * gap;
        line_on(p, o, s, d0 - 2.0, d1 + 3.0, 0.12, y + 0.003);
    }
    for (k, team) in teams.iter().enumerate() {
        let s = first + k as f32 * gap;
        let idx = tex::board_index(team).unwrap_or(0);
        let color = tex::BOARDS[idx].bg[0];
        let inward = -o.out(s);
        let at = o.at(s, d1 + 4.0, y);
        let yaw = (-inward.x).atan2(-inward.z);
        let truck = super::buildings::Place::new(at, yaw);
        transporter(
            parts,
            truck,
            Vec4::new(color[0], color[1], color[2], 1.0),
            idx,
        );
        if k % 2 == 0 {
            awning(parts, truck, Vec4::new(color[0], color[1], color[2], 1.0));
        }
    }
    // Floodlight masts at both ends of the paddock, aimed at it.
    for (s, toward) in [(s0 + 2.0, st - 20.0), (s1 - 2.0, st + 20.0)] {
        let base = o.at(s, d1 + 1.5, 0.0);
        let target = o.at(toward, (d0 + d1) * 0.5, 0.0);
        super::buildings::floodlight_mast(parts, base, 16.0, target, 4);
    }
}

/// A painted line across the oval at `s` from `da` to `db`.
fn line_on(b: &mut Builder, o: &Oval, s: f32, da: f32, db: f32, w: f32, y: f32) {
    let dir = o.dir(s) * (w * 0.5);
    let (a, c) = (o.at(s, da, y), o.at(s, db, y));
    let q = [a - dir, a + dir, c + dir, c - dir];
    b.quad_n(q, q.map(|v| Vec2::new(v.x, v.z)), Vec3::Y);
}

/// An articulated race transporter facing −z of `at` (front bumper at z = 0, trailer rear
/// at z = 18.1): a high-roof cab-over tractor with rounded corners, a glazed cab, mirrors,
/// steps, tanks and side deflectors, and a box trailer in the team's color with its board on
/// both sides, side skirts, marker lights, rear doors and an underrun bar.
fn transporter(parts: &mut Parts, at: super::buildings::Place, color: Vec4, board: usize) {
    let p = |x: f32, y: f32, z: f32| at.p(x, y, z);
    let r = at.r;
    let fwd = at.v(Vec3::NEG_Z);
    let uv = [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y];
    // A quad on a face of the cab: on the front (x, y at z) or a side (z, y at x).
    let front = |b: &mut Builder, x0: f32, x1: f32, y0: f32, y1: f32, z: f32| {
        b.quad_n(
            [p(x0, y0, z), p(x1, y0, z), p(x1, y1, z), p(x0, y1, z)],
            uv,
            fwd,
        );
    };
    let side = |b: &mut Builder, x: f32, z0: f32, z1: f32, y0: f32, y1: f32| {
        let n = at.v(Vec3::X * x.signum());
        b.quad_n(
            [p(x, y0, z0), p(x, y0, z1), p(x, y1, z1), p(x, y1, z0)],
            uv,
            n,
        );
    };
    // Cab (y 1.14..3.98, z 0.05..2.29), side deflectors back to the trailer, bumper.
    let b = parts.get(Surface::Painted);
    b.color(color);
    b.rounded_box(p(0.0, 2.56, 1.17), Vec3::new(1.24, 1.42, 1.12), 0.24, 3, r);
    b.rounded_box(
        p(0.0, 3.62, -0.07),
        Vec3::new(1.02, 0.035, 0.12),
        0.03,
        1,
        r,
    );
    for s in [-1.0f32, 1.0] {
        b.rounded_box(
            p(s * 1.205, 2.62, 2.95),
            Vec3::new(0.025, 1.3, 0.7),
            0.02,
            1,
            r,
        );
    }
    b.color(gray(0.13));
    b.rounded_box(p(0.0, 0.78, 0.27), Vec3::new(1.23, 0.36, 0.27), 0.09, 2, r);
    for s in [-1.0f32, 1.0] {
        // Front wheel arch lip, the step box under the door and the chassis rails.
        b.rounded_box(
            p(s * 1.04, 1.1, 1.45),
            Vec3::new(0.2, 0.04, 0.64),
            0.03,
            1,
            r,
        );
        b.rounded_box(
            p(s * 1.08, 0.82, 0.72),
            Vec3::new(0.14, 0.32, 0.19),
            0.03,
            1,
            r,
        );
        b.rounded_box(
            p(s * 0.45, 0.95, 4.6),
            Vec3::new(0.07, 0.16, 2.4),
            0.02,
            1,
            r,
        );
    }
    // Grille with chrome slats and a badge.
    b.color(gray(0.04));
    front(b, -0.9, 0.9, 1.42, 2.08, 0.045);
    let ch = parts.get(Surface::Galvanized);
    ch.color(gray(0.95));
    for k in 0..4 {
        let y = 1.52 + k as f32 * 0.16;
        ch.rounded_box(p(0.0, y, 0.03), Vec3::new(0.86, 0.025, 0.02), 0.01, 1, r);
    }
    ch.rounded_box(p(0.0, 2.2, 0.035), Vec3::new(0.2, 0.06, 0.02), 0.02, 1, r);
    for s in [-1.0f32, 1.0] {
        // Grated steps and the tank straps.
        for y in [0.6f32, 0.95] {
            ch.chamfer_box(p(s * 1.12, y, 0.62), Vec3::new(0.11, 0.015, 0.17), 0.005, r);
        }
        ch.cylinder(
            p(s * 0.95, 0.95, 2.45),
            p(s * 0.95, 0.95, 3.95),
            0.32,
            0.32,
            18,
            true,
        );
    }
    let st = parts.get(Surface::Painted);
    st.color(gray(0.1));
    for s in [-1.0f32, 1.0] {
        for z in [2.75f32, 3.65] {
            st.torus(p(s * 0.95, 0.95, z), fwd, 0.325, 0.02, 18, 4);
        }
    }
    // Windscreen and door windows: a rubber surround, the dark cab behind, the glass.
    let f = parts.get(Surface::Rubber);
    f.color(Vec4::ONE);
    front(f, -1.0, 1.0, 2.26, 3.5, 0.046);
    for s in [-1.0f32, 1.0] {
        side(f, s * 1.243, 0.33, 1.5, 2.32, 3.48);
        // Door seams.
        side(f, s * 1.243, 1.56, 1.58, 1.3, 3.56);
        side(f, s * 1.243, 0.3, 1.58, 3.54, 3.56);
    }
    let i = parts.get(Surface::Interior);
    i.color(Vec4::ONE);
    front(i, -0.97, 0.97, 2.3, 3.46, 0.043);
    for s in [-1.0f32, 1.0] {
        side(i, s * 1.245, 0.36, 1.47, 2.35, 3.45);
    }
    let gl = parts.get(Surface::Glass);
    gl.color(Vec4::ONE);
    front(gl, -0.97, 0.97, 2.3, 3.46, 0.04);
    for s in [-1.0f32, 1.0] {
        side(gl, s * 1.248, 0.36, 1.47, 2.35, 3.45);
    }
    // Wipers, door handles and the mirrors on their arms.
    let d = parts.get(Surface::Painted);
    d.color(gray(0.06));
    for x in [-0.75f32, 0.15] {
        d.beam(p(x, 2.34, 0.03), p(x + 0.62, 2.5, 0.03), 0.025, 0.02, 0.0);
    }
    for s in [-1.0f32, 1.0] {
        d.cylinder(
            p(s * 1.18, 3.38, 0.22),
            p(s * 1.46, 3.3, 0.06),
            0.025,
            0.025,
            6,
            false,
        );
        d.cylinder(
            p(s * 1.18, 2.3, 0.22),
            p(s * 1.46, 2.42, 0.06),
            0.02,
            0.02,
            6,
            false,
        );
        d.rounded_box(
            p(s * 1.48, 2.92, 0.05),
            Vec3::new(0.06, 0.32, 0.12),
            0.04,
            2,
            r,
        );
        d.rounded_box(
            p(s * 1.47, 2.47, 0.07),
            Vec3::new(0.05, 0.1, 0.1),
            0.03,
            1,
            r,
        );
    }
    let m = parts.get(Surface::LensClear);
    m.color(gray(0.85));
    for s in [-1.0f32, 1.0] {
        let n = at.v(Vec3::Z);
        let q = |x: f32, y: f32| p(s * 1.48 + x, y, 0.172);
        m.quad_n(
            [q(-0.05, 2.62), q(0.05, 2.62), q(0.05, 3.22), q(-0.05, 3.22)],
            uv,
            n,
        );
        // Head and fog lights in the bumper.
        m.rounded_box(
            p(s * 0.88, 0.97, -0.005),
            Vec3::new(0.25, 0.08, 0.03),
            0.025,
            1,
            r,
        );
        m.rounded_box(
            p(s * 0.62, 0.6, -0.005),
            Vec3::new(0.09, 0.05, 0.02),
            0.02,
            1,
            r,
        );
    }
    let h = parts.get(Surface::Galvanized);
    h.color(gray(0.85));
    for s in [-1.0f32, 1.0] {
        h.chamfer_box(p(s * 1.25, 2.1, 1.4), Vec3::new(0.015, 0.03, 0.1), 0.005, r);
    }
    // Mudflaps behind the drive axles.
    let fl = parts.get(Surface::Rubber);
    for s in [-1.0f32, 1.0] {
        fl.chamfer_box(p(s * 0.95, 0.6, 6.62), Vec3::new(0.28, 0.32, 0.012), 0.0, r);
    }
    // Trailer: the box, roof trim, side skirts, landing legs and the rear end.
    let b = parts.get(Surface::Painted);
    b.color(color);
    b.rounded_box(p(0.0, 2.5, 11.3), Vec3::new(1.275, 1.55, 6.8), 0.1, 2, r);
    b.color(gray(0.9));
    b.chamfer_box(p(0.0, 4.07, 11.3), Vec3::new(1.2, 0.025, 6.75), 0.01, r);
    b.color(color * 0.7);
    for s in [-1.0f32, 1.0] {
        b.chamfer_box(
            p(s * 1.2, 0.72, 10.45),
            Vec3::new(0.03, 0.22, 3.45),
            0.01,
            r,
        );
    }
    b.color(color * 0.85);
    for s in [-1.0f32, 1.0] {
        b.chamfer_box(
            p(s * 0.6, 2.5, 18.11),
            Vec3::new(0.58, 1.45, 0.012),
            0.004,
            r,
        );
    }
    b.color(gray(0.2));
    for s in [-0.6f32, 0.6] {
        b.chamfer_box(p(s, 0.6, 6.5), Vec3::new(0.06, 0.35, 0.06), 0.0, r);
    }
    let g = parts.get(Surface::Galvanized);
    g.color(gray(0.8));
    for x in [-0.95f32, -0.25, 0.25, 0.95] {
        g.cylinder(p(x, 1.1, 18.14), p(x, 3.9, 18.14), 0.018, 0.018, 6, false);
    }
    g.beam(p(-1.1, 0.5, 18.0), p(1.1, 0.5, 18.0), 0.14, 0.12, 0.01);
    for x in [-0.7f32, 0.7] {
        g.beam(p(x, 0.5, 17.8), p(x, 0.98, 17.8), 0.08, 0.08, 0.0);
    }
    let l = parts.get(Surface::Plastic);
    l.color(hex(0x6a0a08));
    for s in [-1.0f32, 1.0] {
        l.chamfer_box(
            p(s * 0.98, 0.78, 18.13),
            Vec3::new(0.2, 0.07, 0.02),
            0.01,
            r,
        );
    }
    l.color(hex(0xc87410));
    for s in [-1.0f32, 1.0] {
        for k in 0..5 {
            let z = 5.2 + k as f32 * 3.0;
            l.chamfer_box(p(s * 1.28, 1.06, z), Vec3::new(0.012, 0.03, 0.05), 0.005, r);
        }
    }
    // Team boards on both sides of the trailer.
    let sg = parts.get(Surface::Signs);
    sg.color(Vec4::ONE);
    let (zf, zr) = (6.0f32, 17.0f32);
    let (y0, h) = (1.2f32, 2.75f32);
    super::buildings::board(sg, p(1.281, y0, zr), p(1.281, y0, zf), h, board);
    super::buildings::board(sg, p(-1.281, y0, zf), p(-1.281, y0, zr), h, board);
    // Wheels: steer axle, drive tandem (twin tires), three trailer axles (super singles).
    for (z, rad, x, hw) in [
        (1.45f32, 0.52f32, 1.0f32, 0.16f32),
        (4.7, 0.52, 0.9, 0.3),
        (5.95, 0.52, 0.9, 0.3),
        (14.6, 0.46, 1.0, 0.2),
        (15.9, 0.46, 1.0, 0.2),
        (17.2, 0.46, 1.0, 0.2),
    ] {
        for s in [-1.0f32, 1.0] {
            truck_wheel(parts, p(s * x, rad, z), at.v(Vec3::X * s), rad, hw);
        }
    }
}

/// A truck wheel at `c` turning about `axis` (pointing out of the vehicle): a tire with
/// rounded shoulders, a dished steel rim, a hub cap and ten wheel nuts.
fn truck_wheel(parts: &mut Parts, c: Vec3, axis: Vec3, r: f32, hw: f32) {
    let t = parts.get(Surface::Rubber);
    t.color(Vec4::ONE);
    let tire = [
        Vec2::new(r * 0.6, hw * 0.86),
        Vec2::new(r * 0.84, hw),
        Vec2::new(r * 0.96, hw * 0.92),
        Vec2::new(r, hw * 0.6),
        Vec2::new(r, -hw * 0.6),
        Vec2::new(r * 0.96, -hw * 0.92),
        Vec2::new(r * 0.84, -hw),
        Vec2::new(r * 0.6, -hw * 0.86),
    ];
    t.lathe(c, axis, &tire, &[false; 8], 28);
    let m = parts.get(Surface::Galvanized);
    m.color(gray(0.72));
    let rim = [
        Vec2::new(0.0, hw * 0.6),
        Vec2::new(r * 0.22, hw * 0.6),
        Vec2::new(r * 0.26, hw * 0.5),
        Vec2::new(r * 0.5, hw * 0.42),
        Vec2::new(r * 0.56, hw * 0.8),
        Vec2::new(r * 0.61, hw * 0.88),
    ];
    m.lathe(c, axis, &rim, &[false, true, true, true, true, false], 28);
    m.color(gray(0.9));
    let face = c + axis * hw * 0.6;
    m.cylinder(face, face + axis * 0.07, r * 0.11, r * 0.09, 12, true);
    let (u, v) = super::kit::basis(axis);
    for k in 0..10 {
        let a = k as f32 / 10.0 * TAU;
        let q = face + (u * a.cos() + v * a.sin()) * r * 0.17;
        m.cylinder(q, q + axis * 0.035, 0.018, 0.018, 6, true);
    }
}

/// An awning off the trailer's +x side: a fabric roof on two posts and a back valance.
fn awning(parts: &mut Parts, at: super::buildings::Place, color: Vec4) {
    let p = |x: f32, y: f32, z: f32| at.p(x, y, z);
    let f = parts.get(Surface::Plastic);
    f.color(color * 0.85 + Vec4::new(0.04, 0.04, 0.04, 0.0));
    let (z0, z1) = (6.0f32, 17.0f32);
    let (x0, x1) = (1.3f32, 6.3f32);
    let (y0, y1) = (4.0f32, 3.3f32);
    f.quad_n(
        [p(x0, y0, z0), p(x1, y1, z0), p(x1, y1, z1), p(x0, y0, z1)],
        [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
        at.v(Vec3::new(0.14, 1.0, 0.0).normalize()),
    );
    f.quad_n(
        [
            p(x0, y0 - 0.01, z1),
            p(x1, y1 - 0.01, z1),
            p(x1, y1 - 0.01, z0),
            p(x0, y0 - 0.01, z0),
        ],
        [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
        at.v(Vec3::new(-0.14, -1.0, 0.0).normalize()),
    );
    f.color(color * 0.6);
    f.chamfer_box(
        p(x1, y1 - 0.2, (z0 + z1) * 0.5),
        Vec3::new(0.02, 0.2, (z1 - z0) * 0.5),
        0.0,
        at.r,
    );
    let g = parts.get(Surface::Galvanized);
    g.color(gray(0.85));
    for z in [z0 + 0.2, (z0 + z1) * 0.5, z1 - 0.2] {
        g.cylinder(
            p(x1 - 0.1, 0.03, z),
            p(x1 - 0.1, y1, z),
            0.04,
            0.04,
            8,
            false,
        );
    }
    // A tool chest and a tyre rack under the awning.
    let c = parts.get(Surface::Painted);
    c.color(hex(0xb81d1d));
    c.chamfer_box(p(2.5, 0.55, 8.0), Vec3::new(0.35, 0.5, 0.8), 0.03, at.r);
    c.color(gray(0.25));
    c.chamfer_box(p(2.5, 1.07, 8.0), Vec3::new(0.36, 0.02, 0.81), 0.01, at.r);
    let rk = parts.get(Surface::Galvanized);
    rk.color(gray(0.6));
    rk.beam(p(2.0, 0.03, 12.0), p(2.0, 1.4, 12.0), 0.05, 0.05, 0.0);
    rk.beam(p(2.0, 0.03, 15.0), p(2.0, 1.4, 15.0), 0.05, 0.05, 0.0);
    rk.beam(p(2.0, 0.7, 12.0), p(2.0, 0.7, 15.0), 0.05, 0.05, 0.0);
    let t = parts.get(Surface::Rubber);
    t.color(Vec4::ONE);
    for k in 0..4 {
        let z = 12.4 + k as f32 * 0.7;
        for yy in [0.38f32, 1.05] {
            t.cylinder(p(1.85, yy, z), p(2.15, yy, z), 0.33, 0.33, 16, true);
        }
    }
}

/// The pit building's paddock side: a roller door and a personnel door into each garage,
/// a row of first-floor windows, downpipes between the bays and a canopy over the doors.
fn pit_back(o: &Oval, parts: &mut Parts) {
    let st = o.start();
    let (s0, s1) = (st - pit::BUILDING, st + pit::BUILDING);
    let bays = 12usize;
    let bay = (s1 - s0) / bays as f32;
    let d = pit::BACK - 0.02;
    let h1 = 5.2f32;
    // A quad on the back wall between s = sa..sb and heights y0..y1, `off` in front of it.
    let wall_quad = |b: &mut Builder, sa: f32, sb: f32, y0: f32, y1: f32, off: f32, ribs: bool| {
        let n = -o.out((sa + sb) * 0.5);
        let q = [
            o.at(sb, d - off, y0),
            o.at(sa, d - off, y0),
            o.at(sa, d - off, y1),
            o.at(sb, d - off, y1),
        ];
        let uv = if ribs {
            [
                Vec2::new(y0 * 1.6, sb),
                Vec2::new(y0 * 1.6, sa),
                Vec2::new(y1 * 1.6, sa),
                Vec2::new(y1 * 1.6, sb),
            ]
        } else {
            q.map(|p| Vec2::new(p.x + p.z, p.y))
        };
        b.quad_n(q, uv, n);
    };
    for i in 0..bays {
        let a = s0 + i as f32 * bay;
        let m = a + bay * 0.5;
        // Roller door with its guides and a bottom rail.
        let (da, db) = (m - 2.6, m + 1.4);
        let c = parts.get(Surface::Cladding);
        c.color(gray(0.78));
        wall_quad(c, da, db, 0.03, 4.0, 0.03, true);
        let g = parts.get(Surface::Painted);
        g.color(gray(0.22));
        for s in [da, db] {
            let p = o.at(s, d - 0.06, 0.0);
            g.cuboid(
                p + Vec3::Y * 2.1,
                Vec3::new(0.06, 2.1, 0.06),
                Quat::from_rotation_arc(Vec3::Z, o.dir(s)),
            );
        }
        wall_quad(g, da, db, 3.95, 4.35, 0.08, false);
        // Personnel door with a vision panel.
        let (pa, pb) = (m + 2.0, m + 3.0);
        g.color(gray(0.3));
        wall_quad(g, pa, pb, 0.03, 2.2, 0.03, false);
        let gl = parts.get(Surface::Glass);
        gl.color(Vec4::ONE);
        wall_quad(gl, pa + 0.25, pb - 0.25, 1.3, 1.9, 0.04, false);
        // First-floor windows: a dark room behind glass, frame and mullions.
        let (wa, wb) = (a + 0.6, a + bay - 0.6);
        let (y0, y1) = (h1 + 1.0, h1 + 2.8);
        let r = parts.get(Surface::Interior);
        r.color(Vec4::ONE);
        wall_quad(r, wa, wb, y0, y1, 0.01, false);
        let gl = parts.get(Surface::Glass);
        wall_quad(gl, wa, wb, y0, y1, 0.03, false);
        let f = parts.get(Surface::Painted);
        f.color(gray(0.25));
        wall_quad(f, wa - 0.08, wb + 0.08, y0 - 0.1, y0, 0.05, false);
        wall_quad(f, wa - 0.08, wb + 0.08, y1, y1 + 0.08, 0.05, false);
        let n = 4;
        for k in 0..=n {
            let s = wa + (wb - wa) * k as f32 / n as f32;
            wall_quad(f, s - 0.04, s + 0.04, y0, y1, 0.05, false);
        }
        // Downpipe on the bay line.
        let p = parts.get(Surface::Painted);
        p.color(gray(0.45));
        let q = o.at(a, d - 0.12, 0.0);
        p.cylinder(q, q + Vec3::Y * 9.3, 0.06, 0.06, 8, false);
    }
    let p = parts.get(Surface::Painted);
    let q = o.at(s1, d - 0.12, 0.0);
    p.cylinder(q, q + Vec3::Y * 9.3, 0.06, 0.06, 8, false);
    // A continuous canopy over the ground floor doors.
    let c = parts.get(Surface::Painted);
    c.color(gray(0.85));
    let prof = [
        Vec2::new(pit::BACK - 2.2, 4.6),
        Vec2::new(pit::BACK - 2.2, 4.85),
        Vec2::new(pit::BACK, 4.85),
        Vec2::new(pit::BACK, 4.6),
        Vec2::new(pit::BACK - 2.2, 4.6),
    ];
    let frames: Vec<Frame> = (0..=24)
        .map(|i| o.frame(s0 + (s1 - s0) * i as f32 / 24.0, 0.0, 0.0, true))
        .collect();
    c.sweep(&prof, &[true; 5], &frames, true);
    c.sweep_caps(&prof[..4], &frames[0], &frames[frames.len() - 1]);
    // Downlights in its soffit, two per bay.
    let l = parts.get(Surface::Lamp);
    l.color(Vec4::ONE);
    for i in 0..bays * 2 {
        let s = s0 + (i as f32 + 0.5) * bay * 0.5;
        l.annulus(o.at(s, pit::BACK - 1.1, 4.595), Vec3::NEG_Y, 0.0, 0.09, 12);
    }
}

/// Paint `text` upright on a wall facing outward at (`s`, `d`), baseline at `y`.
fn text_on_wall(b: &mut Builder, o: &Oval, text: &str, s: f32, d: f32, y: f32, h: f32) {
    let unit = h / 6.0;
    let n = text.chars().count() as f32;
    let total = (n - 1.0) * super::font::ADVANCE * unit + super::font::WIDTH * unit;
    let out = o.out(s);
    // Seen from outside (the +d side), the reader's right is against the driving direction.
    let right = -o.dir(s);
    for (i, c) in text.chars().enumerate() {
        let Some(uv) = tex::glyph_uv(c) else { continue };
        let x = -total * 0.5 + i as f32 * super::font::ADVANCE * unit + 2.0 * unit;
        let center = o.at(s, d, y + 3.0 * unit) + right * x;
        let (hw, hh) = (3.0 * unit, 4.0 * unit);
        let p = [
            center - right * hw - Vec3::Y * hh,
            center + right * hw - Vec3::Y * hh,
            center + right * hw + Vec3::Y * hh,
            center - right * hw + Vec3::Y * hh,
        ];
        let t = [
            Vec2::new(uv[0], uv[3]),
            Vec2::new(uv[2], uv[3]),
            Vec2::new(uv[2], uv[1]),
            Vec2::new(uv[0], uv[1]),
        ];
        b.quad_n(p, t, out);
    }
}

// ---------------------------------------------------------------- start gantry

fn gantry(o: &Oval, parts: &mut Parts) {
    let s = o.start() + 1.0;
    let fwd = o.dir(s);
    let rot = Quat::from_rotation_arc(Vec3::Z, fwd);
    let (d_out, d_in) = (8.6f32, -8.4f32);
    let (y0, y1) = (6.3f32, 7.5f32);
    let g = parts.get(Surface::Painted);
    g.color(gray(0.2));
    for d in [d_out, d_in] {
        let base = o.at(s, d, 0.0);
        g.chamfer_box(
            base + Vec3::Y * (y1 + 0.2) * 0.5,
            Vec3::new(0.3, (y1 + 0.2) * 0.5, 0.3),
            0.03,
            rot,
        );
        g.chamfer_box(
            base + Vec3::Y * 0.15,
            Vec3::new(0.55, 0.15, 0.55),
            0.03,
            rot,
        );
    }
    // Truss: two chords with verticals and diagonals.
    let a0 = o.at(s, d_in, y0);
    let a1 = o.at(s, d_out, y0);
    let b0 = o.at(s, d_in, y1);
    let b1 = o.at(s, d_out, y1);
    for off in [-0.35f32, 0.35] {
        let sh = fwd * off;
        g.beam(a0 + sh, a1 + sh, 0.12, 0.12, 0.01);
        g.beam(b0 + sh, b1 + sh, 0.12, 0.12, 0.01);
        let n = 10;
        for k in 0..=n {
            let t = k as f32 / n as f32;
            g.beam(a0.lerp(a1, t) + sh, b0.lerp(b1, t) + sh, 0.06, 0.06, 0.0);
            if k < n {
                let t1 = (k + 1) as f32 / n as f32;
                g.beam(a0.lerp(a1, t) + sh, b0.lerp(b1, t1) + sh, 0.05, 0.05, 0.0);
            }
        }
    }
    // Light housing hanging below the truss, facing the grid (against the driving direction).
    let c = o.at(s, 0.0, y0 - 0.6);
    g.color(gray(0.06));
    g.chamfer_box(c, Vec3::new(2.2, 0.55, 0.22), 0.03, rot);
    let face = c - fwd * 0.23;
    let right = -o.out(s);
    for col in 0..5 {
        let x = (col as f32 - 2.0) * 0.85;
        for row in 0..3 {
            let y = 0.32 - row as f32 * 0.3;
            let lens = if row == 2 {
                Surface::LensGreen
            } else {
                Surface::LensOff
            };
            let l = parts.get(lens);
            l.color(Vec4::ONE);
            let p = face + right * x + Vec3::Y * y;
            l.cylinder(p + fwd * 0.02, p - fwd * 0.05, 0.11, 0.11, 12, true);
        }
        let g = parts.get(Surface::Painted);
        g.color(gray(0.1));
        // Visors over each light column.
        g.cuboid(
            face + right * x + Vec3::Y * 0.5 - fwd * 0.08,
            Vec3::new(0.16, 0.015, 0.1),
            rot,
        );
    }
    // Sponsor board on the truss (both faces).
    let sg = parts.get(Surface::Signs);
    let idx = tex::board_index("KESTREL").unwrap_or(0);
    let uv = tex::board_uv(idx);
    for sgn in [-1.0f32, 1.0] {
        let n = fwd * sgn;
        let off = n * 0.45;
        let (l, r) = if sgn < 0.0 {
            (o.at(s, 4.0, y0 + 0.1), o.at(s, -4.0, y0 + 0.1))
        } else {
            (o.at(s, -4.0, y0 + 0.1), o.at(s, 4.0, y0 + 0.1))
        };
        let q = [
            l + off,
            r + off,
            r + off + Vec3::Y * 1.2,
            l + off + Vec3::Y * 1.2,
        ];
        let t = [
            Vec2::new(uv[0], uv[3]),
            Vec2::new(uv[2], uv[3]),
            Vec2::new(uv[2], uv[1]),
            Vec2::new(uv[0], uv[1]),
        ];
        sg.quad_n(q, t, n);
    }
}

// ---------------------------------------------------------------- marshal posts, boards

fn marshal_posts(o: &Oval, parts: &mut Parts) {
    let l = o.length;
    let spots = [
        l - 95.0,
        70.0,
        0.25 * l - 40.0,
        0.5 * l - 95.0,
        0.5 * l + 70.0,
    ];
    for (i, &s) in spots.iter().enumerate() {
        let d = outer_barrier(o, s) + 2.8;
        let c = o.at(s, d, 0.0);
        let rot = Quat::from_rotation_arc(Vec3::Z, o.dir(s));
        let inward = -o.out(s);
        let p = parts.get(Surface::Painted);
        // Platform on four legs, cabin, roof, railing.
        p.color(gray(0.55));
        for (x, z) in [(-1.1f32, -1.1f32), (1.1, -1.1), (-1.1, 1.1), (1.1, 1.1)] {
            p.cuboid(
                c + rot * Vec3::new(x, 0.6, z),
                Vec3::new(0.05, 0.6, 0.05),
                rot,
            );
        }
        p.chamfer_box(c + Vec3::Y * 1.25, Vec3::new(1.3, 0.06, 1.3), 0.02, rot);
        p.color(hex(0xe86a10));
        p.chamfer_box(
            c + Vec3::Y * 2.35 - inward * 0.35,
            Vec3::new(1.0, 1.05, 0.65),
            0.03,
            rot,
        );
        p.color(gray(0.9));
        p.chamfer_box(c + Vec3::Y * 3.5, Vec3::new(1.45, 0.07, 1.45), 0.03, rot);
        p.color(gray(0.7));
        for k in 0..2 {
            p.cuboid(
                c + inward * 1.25 + Vec3::Y * (1.75 + k as f32 * 0.45),
                Vec3::new(0.02, 0.02, 1.28),
                rot,
            );
        }
        let gl = parts.get(Surface::Glass);
        gl.cuboid(
            c + Vec3::Y * 2.6 + inward * 0.31,
            Vec3::new(0.85, 0.45, 0.01),
            rot,
        );
        // LED flag panel facing the track.
        let panel = c + inward * 1.4 + Vec3::Y * 2.6 + rot * Vec3::new(0.0, 0.0, 0.0);
        let p = parts.get(Surface::Painted);
        p.color(gray(0.05));
        p.chamfer_box(panel, Vec3::new(0.5, 0.4, 0.06), 0.02, rot);
        p.cuboid(panel - Vec3::Y * 1.3, Vec3::new(0.04, 0.95, 0.04), rot);
        let lens = if i == 2 {
            Surface::LensAmber
        } else {
            Surface::LensGreen
        };
        let le = parts.get(lens);
        le.cuboid(panel + inward * 0.065, Vec3::new(0.42, 0.32, 0.004), rot);
        let g = parts.get(Surface::Glyphs);
        g.color(Vec4::new(0.05, 0.05, 0.05, 1.0));
        let _ = g;
    }
}

fn billboards(o: &Oval, parts: &mut Parts) {
    let names = [
        "TITAN TYRES",
        "APEX OIL",
        "AURORA",
        "PULSAR",
        "NORDLINE",
        "ORBITAL",
        "VELOCE",
        "QUANTA",
    ];
    let mut k = 0usize;
    for which in 0..2 {
        let apex = o.apex(which);
        let mut s = apex - 70.0;
        while s < apex + 70.0 {
            let d = outer_barrier(o, s) + 1.0;
            let idx = tex::board_index(names[k % names.len()]).unwrap_or(0);
            let sg = parts.get(Surface::Signs);
            board_on_track(sg, o, s, s + 6.5, d, 0.9, 2.5, idx, false);
            let p = parts.get(Surface::Painted);
            p.color(gray(0.3));
            board_back(p, o, s, s + 6.5, d + 0.03, 0.9, 2.5);
            for e in [s + 0.5, s + 6.0] {
                let c = o.at(e, d + 0.08, 0.0);
                p.cuboid(
                    c + Vec3::Y * 1.25,
                    Vec3::new(0.05, 1.25, 0.05),
                    Quat::from_rotation_arc(Vec3::Z, o.dir(e)),
                );
            }
            s += 8.5;
            k += 1;
        }
    }
    // Boards on the outer concrete wall's traffic face and on the pit wall.
    let st = o.start();
    let sg = parts.get(Surface::Signs);
    let mut s = st - pit::OUTER_WALL + 4.0;
    while s < st + pit::OUTER_WALL - 4.0 {
        let idx = tex::board_index(names[k % names.len()]).unwrap_or(0);
        board_on_track(sg, o, s, s + 3.8, 10.0 - 0.215, 0.32, 0.98, idx, false);
        s += 4.0;
        k += 1;
    }
}

fn board_back(b: &mut Builder, o: &Oval, s0: f32, s1: f32, d: f32, y0: f32, y1: f32) {
    let n = 4;
    let base = b.mesh.vertices.len() as u32;
    for i in 0..=n {
        let s = s0 + (s1 - s0) * i as f32 / n as f32;
        let out = o.out(s);
        b.vertex(o.at(s, d, y0), out, Vec2::new(s, y0));
        b.vertex(o.at(s, d, y1), out, Vec2::new(s, y1));
    }
    for i in 0..n as u32 {
        let k = base + i * 2;
        b.quad_idx(k, k + 2, k + 3, k + 1);
    }
}

/// A truss footbridge over the south straight with a big sponsor banner.
fn footbridge(o: &Oval, parts: &mut Parts) {
    let s = o.jump() - 80.0;
    let fwd = o.dir(s);
    let rot = Quat::from_rotation_arc(Vec3::Z, fwd);
    let (d0, d1) = (-12.5f32, 13.5f32);
    let (y0, y1) = (6.0f32, 8.4f32);
    let g = parts.get(Surface::Painted);
    g.color(gray(0.85));
    for d in [d0, d1] {
        let base = o.at(s, d, 0.0);
        for off in [-1.2f32, 1.2] {
            g.chamfer_box(
                base + fwd * off + Vec3::Y * y1 * 0.5,
                Vec3::new(0.25, y1 * 0.5, 0.25),
                0.02,
                rot,
            );
        }
        // Stair towers.
        g.chamfer_box(
            base + o.out(s) * d.signum() * 2.0 + Vec3::Y * y0 * 0.5,
            Vec3::new(1.2, y0 * 0.5, 1.8),
            0.03,
            rot,
        );
    }
    for off in [-1.2f32, 1.2] {
        let sh = fwd * off;
        let a0 = o.at(s, d0, y0) + sh;
        let a1 = o.at(s, d1, y0) + sh;
        let b0 = o.at(s, d0, y1) + sh;
        let b1 = o.at(s, d1, y1) + sh;
        g.beam(a0, a1, 0.2, 0.25, 0.02);
        g.beam(b0, b1, 0.2, 0.2, 0.02);
        let n = 12;
        for k in 0..n {
            let t0 = k as f32 / n as f32;
            let t1 = (k + 1) as f32 / n as f32;
            g.beam(a0.lerp(a1, t0), b0.lerp(b1, t0), 0.1, 0.1, 0.0);
            g.beam(a0.lerp(a1, t0), b0.lerp(b1, t1), 0.08, 0.08, 0.0);
        }
    }
    // Deck and roof.
    let mid = o.at(s, (d0 + d1) * 0.5, y0);
    let half = (d1 - d0) * 0.5;
    let rot_x = Quat::from_rotation_arc(Vec3::X, o.out(s));
    g.cuboid(mid, Vec3::new(half, 0.12, 1.3), rot_x);
    g.cuboid(
        o.at(s, (d0 + d1) * 0.5, y1 + 0.15),
        Vec3::new(half + 0.3, 0.08, 1.5),
        rot_x,
    );
    // Banners on both faces.
    let sg = parts.get(Surface::Signs);
    let idx = tex::board_index("TITAN TYRES").unwrap_or(0);
    let uv = tex::board_uv(idx);
    for sgn in [-1.0f32, 1.0] {
        let n = fwd * sgn;
        let off = n * 1.33;
        let (l, r) = if sgn < 0.0 {
            (o.at(s, 8.5, y0 + 0.15), o.at(s, -8.5, y0 + 0.15))
        } else {
            (o.at(s, -8.5, y0 + 0.15), o.at(s, 8.5, y0 + 0.15))
        };
        let q = [
            l + off,
            r + off,
            r + off + Vec3::Y * 2.1,
            l + off + Vec3::Y * 2.1,
        ];
        let t = [
            Vec2::new(uv[0], uv[3]),
            Vec2::new(uv[2], uv[3]),
            Vec2::new(uv[2], uv[1]),
            Vec2::new(uv[0], uv[1]),
        ];
        sg.quad_n(q, t, n);
    }
}

// ---------------------------------------------------------------- kicker

/// The kicker's collider frame (top-face center, rotation) exactly as `demo::spawn_track`.
fn ramp_frame(_o: &Oval) -> (Vec3, Quat) {
    let a = JUMP_ANGLE as f32;
    let jump = Vec3::new(
        TRACK_RADII.0 as f32 * a.cos(),
        TOP,
        TRACK_RADII.1 as f32 * a.sin(),
    );
    let dir = Vec3::new(
        -(TRACK_RADII.0 as f32) * a.sin(),
        0.0,
        TRACK_RADII.1 as f32 * a.cos(),
    )
    .normalize();
    let (len, angle, thick) = (6.0f32, 7.5f32.to_radians(), 0.6f32);
    let rot = crate::demo::facing(dir) * Quat::from_rotation_x(angle);
    let center = jump - rot * Vec3::new(0.0, thick * 0.5, len * 0.5);
    (center, rot)
}

fn kicker(o: &Oval, parts: &mut Parts) {
    let (c, rot) = ramp_frame(o);
    let w = TRACK_WIDTH * 0.5;
    let top = |x: f32, z: f32| c + rot * Vec3::new(x, 0.3, z);
    let chamfer = 0.03;
    let bc = parts.get(Surface::Concrete);
    bc.color(gray(0.93));
    // Top face (the collider's), chamfer strips along the sides and back, vertical faces down
    // to the asphalt.
    let tl = [
        top(-w + chamfer, 3.0),
        top(w - chamfer, 3.0),
        top(w - chamfer, -3.0 + chamfer),
        top(-w + chamfer, -3.0 + chamfer),
    ];
    let up = rot * Vec3::Y;
    let uvt = |p: Vec3| {
        let l = rot.inverse() * (p - c);
        Vec2::new(l.x, l.z)
    };
    bc.quad_n(tl, tl.map(uvt), up);
    let drop = |p: Vec3, d: f32| p - Vec3::Y * d;
    let outer = [top(-w, 3.0), top(w, 3.0), top(w, -3.0), top(-w, -3.0)];
    // Chamfer: right side, back, left side.
    for (a, b, ia, ib) in [(1usize, 2usize, 1usize, 2usize), (2, 3, 2, 3), (3, 0, 3, 0)] {
        let q = [
            tl[ia],
            tl[ib],
            drop(outer[b], chamfer),
            drop(outer[a], chamfer),
        ];
        bc.quad_world(q);
    }
    let ground = |p: Vec3| Vec3::new(p.x, TOP, p.z);
    // Right side (x = +w), back (z = −3), left side (x = −w).
    let faces = [
        (outer[1], outer[2]),
        (outer[2], outer[3]),
        (outer[3], outer[0]),
    ];
    for (a, b) in faces {
        let (a, b) = (drop(a, chamfer), drop(b, chamfer));
        let q = [ground(a), ground(b), b, a];
        let n = (b - a).cross(Vec3::Y).normalize_or(Vec3::X);
        let (u, _) = super::kit::basis(n);
        let uv = q.map(|p| Vec2::new(p.dot(u), p.y));
        bc.quad_n(q, uv, n);
    }
    // Steel lip plate at the low edge.
    let g = parts.get(Surface::Galvanized);
    let fwd = rot * Vec3::NEG_Z;
    let flat_fwd = Vec3::new(fwd.x, 0.0, fwd.z).normalize();
    let side = rot * Vec3::X;
    let low_c = top(0.0, 3.0);
    let plate = [
        low_c - side * w - flat_fwd * 0.35 + Vec3::Y * 0.004,
        low_c + side * w - flat_fwd * 0.35 + Vec3::Y * 0.004,
        top(w, 2.7) + up * 0.004,
        top(-w, 2.7) + up * 0.004,
    ];
    g.quad_world(plate);
    // Worn chevrons pointing in the driving direction, plus hazard stripes on the sides.
    let pc = parts.get(Surface::PaintConcrete);
    pc.color(Vec4::new(0.95, 0.62, 0.04, 1.0));
    let paint_at = |x: f32, z: f32| top(x, z) + up * 0.004;
    for k in 0..3 {
        let z0 = 1.6 - k as f32 * 1.6;
        for sgn in [-1.0f32, 1.0] {
            let p = [
                paint_at(0.0, z0 - 0.9),
                paint_at(0.0, z0 - 0.35),
                paint_at(sgn * 4.6, z0 + 0.95),
                paint_at(sgn * 4.6, z0 + 0.4),
            ];
            let q = if sgn > 0.0 {
                [p[0], p[2], p[3], p[1]]
            } else {
                [p[0], p[1], p[3], p[2]]
            };
            pc.quad_n(q, q.map(uvt), up);
        }
    }
    for x in [-w, w] {
        let n = side * x.signum();
        for k in 0..8 {
            let z0 = 3.0 - k as f32 * 0.75;
            let z1 = z0 - 0.375;
            let a = drop(top(x, z0), chamfer) + n * 0.003;
            let b = drop(top(x, z1), chamfer) + n * 0.003;
            let q = [
                ground(a) + n * 0.003,
                ground(b) + n * 0.003 - flat_fwd * 0.2,
                b,
                a,
            ];
            if (q[2].y - q[1].y) < 0.05 {
                continue;
            }
            pc.color(Vec4::new(0.95, 0.62, 0.04, 1.0));
            let (u, _) = super::kit::basis(n);
            pc.quad_n(q, q.map(|p| Vec2::new(p.dot(u), p.y)), n);
        }
    }
}

// ---------------------------------------------------------------- drift pad

fn drift_pad(parts: &mut Parts) {
    let (c, r) = DRIFT_PAD;
    let b = parts.get(Surface::Dirt);
    let seg = 192u32;
    let ring_r: Vec<f32> = {
        let mut v: Vec<f32> = (0..=52).map(|i| i as f32 * 0.5).collect();
        v.extend([r + 0.4, r + 1.0, r + 1.8]);
        v
    };
    let base = b.mesh.vertices.len() as u32;
    for &rr in ring_r.iter() {
        for i in 0..=seg {
            let a = i as f32 / seg as f32 * TAU;
            let (sn, cs) = a.sin_cos();
            let p0 = c + Vec3::new(cs * rr, 0.0, sn * rr);
            let n1 = world_fbm(p0.x, p0.z, 2.0, 2, 101);
            // Loose dirt thrown out of the drift ring and a lip at the pad's edge.
            let berm = 0.04 * smoothstep(17.0, 18.5, rr) * smoothstep(21.5, 19.0, rr)
                + 0.012 * smoothstep(12.0, 12.8, rr) * smoothstep(13.6, 12.8, rr);
            let y = if rr <= r {
                TOP + berm * (0.6 + 0.4 * n1) + 0.004 * n1
            } else {
                TOP * (1.0 - smoothstep(r, r + 1.8, rr)) + 0.02 * smoothstep(r + 1.8, r + 0.4, rr)
            };
            let p = Vec3::new(p0.x, y, p0.z);
            // Compacted, darker ruts where the car circles; dusty elsewhere.
            let rut = smoothstep(2.2, 0.4, (rr - DRIFT_RADIUS - 0.2).abs());
            let tone = (1.0 + 0.12 * world_fbm(p.x, p.z, 6.0, 3, 102))
                * (1.0 - 0.3 * rut)
                * (1.0 + 0.1 * smoothstep(17.0, 19.0, rr));
            let edge = if rr > r { 0.85 } else { 1.0 };
            let v = tone * edge;
            b.vertex_c(
                p,
                Vec3::Y,
                Vec2::new(p.x, p.z),
                Vec4::new(v, v * 0.97, v * 0.93, 1.0),
            );
        }
    }
    let row = seg + 1;
    for ri in 0..ring_r.len() as u32 - 1 {
        for i in 0..seg {
            let k = base + ri * row + i;
            b.quad_idx(k, k + 1, k + row + 1, k + row);
        }
    }
    // Smooth normals over the berms.
    let start = base as usize;
    let mut sub = engine_render::Mesh {
        vertices: b.mesh.vertices[start..].to_vec(),
        indices: b
            .mesh
            .indices
            .iter()
            .filter(|&&i| i >= base)
            .map(|i| i - base)
            .collect(),
    };
    sub.compute_normals();
    b.mesh.vertices.truncate(start);
    b.mesh.indices.retain(|&i| i < base);
    b.mesh.merge(&sub);
    // Tire marks: overlapping circles around the drift ring.
    let s = parts.get(Surface::SkidDirt);
    s.color(Vec4::ONE);
    for k in 0..16u32 {
        let rr = DRIFT_RADIUS - 1.0 + 2.6 * rand1(k as i32, 111);
        let a0 = rand1(k as i32, 112) * TAU;
        let span = (0.6 + 1.4 * rand1(k as i32, 113)) * PI;
        let n = (span * rr / 0.8) as usize;
        let alpha = 0.35 + 0.4 * rand1(k as i32, 114);
        let path: Vec<(Vec3, f32)> = (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32;
                let a = a0 + span * t;
                let wob = 0.25 * (a * 3.0 + k as f32).sin();
                let p = c + Vec3::new(a.cos() * (rr + wob), TOP + 0.04, a.sin() * (rr + wob));
                (p, alpha * envelope(t * span * rr, span * rr, 3.0))
            })
            .collect();
        ribbon(s, &path, 0.28);
    }
    // Cones: one at the center, eight marking the drift circle, a few at the edge.
    let cone = cone_mesh();
    let p = parts.get(Surface::Plastic);
    let mut spots = vec![c];
    for i in 0..8 {
        let a = i as f32 / 8.0 * TAU;
        spots.push(c + Vec3::new(a.cos(), 0.0, a.sin()) * (DRIFT_RADIUS - 3.5));
    }
    for i in 0..6 {
        let a = i as f32 / 6.0 * TAU + 0.3;
        spots.push(c + Vec3::new(a.cos(), 0.0, a.sin()) * (r - 0.8));
    }
    for (i, q) in spots.iter().enumerate() {
        let m = Mat4::from_rotation_translation(
            Quat::from_rotation_y(i as f32 * 1.3),
            *q + Vec3::Y * TOP,
        );
        let start = p.mesh.vertices.len();
        p.add_mesh(&cone, &m);
        let _ = start;
    }
}

/// A 70 cm traffic cone with two reflective bands (vertex colored), base at y = 0.
fn cone_mesh() -> engine_render::Mesh {
    let mut b = Builder::new();
    let orange = hex(0xff5a0a);
    let white = Vec4::new(0.9, 0.9, 0.9, 1.0);
    b.color(gray(0.08));
    b.chamfer_box(
        Vec3::Y * 0.015,
        Vec3::new(0.19, 0.015, 0.19),
        0.01,
        Quat::IDENTITY,
    );
    let bands = [
        (0.03, 0.17, 0.155, orange),
        (0.17, 0.25, 0.135, white),
        (0.25, 0.4, 0.11, orange),
        (0.4, 0.47, 0.09, white),
        (0.47, 0.7, 0.03, orange),
    ];
    let mut r0 = 0.165;
    for (y0, y1, r1, col) in bands {
        b.color(col);
        b.cylinder(Vec3::Y * y0, Vec3::Y * y1, r0, r1, 14, false);
        r0 = r1;
    }
    b.color(orange);
    b.cylinder(Vec3::Y * 0.7, Vec3::Y * 0.71, 0.03, 0.0, 14, false);
    b.finish()
}
