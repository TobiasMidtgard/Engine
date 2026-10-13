//! The airfield around `demo::spawn_airfield`'s runway: grooved asphalt with rubber in the
//! touchdown zones, ICAO markings for a 45 m instrument runway (threshold stripes, both
//! designators from the real heading, aiming point, touchdown zone, centerline, side
//! stripes), shoulders and blast pads with chevrons, edge / threshold / end / wing-bar and
//! approach lights, PAPIs, two connector taxiways with holding positions and signs, two
//! concrete aprons, a maintenance hangar, a fire station with its crash tender, a control
//! tower and terminal, ground equipment, floodlight masts and windsocks.
//!
//! Zone-local frame: origin at the runway's middle on the ground, −z toward the runway's
//! `end`, +x to the right of a pilot rolling from `start` to `end`. The start threshold is
//! at z = +L/2. Buildings stay within 90 m of the middle and everything else within 45 m of
//! the centerline (the flat ground the showcase levels for the airfield).

use std::f32::consts::{FRAC_PI_2, PI};
use std::time::Instant;

use engine_core::math::{DVec3, Quat, Vec2, Vec3, Vec4};
use engine_core::prelude::*;

use super::buildings::{self, Hangar, Place, dashed, flat, line, rect, text};
use super::kit::{Builder, gray, hex};
use super::noise::{mix, rand1, smoothstep, world_fbm};
use super::solid::Solid;
use super::{BuildStats, Parts, Scene, Surface};
use crate::demo::facing;

/// Runway width (matches the collider).
pub const WIDTH: f32 = 45.0;
const HALF: f32 = WIDTH * 0.5;
/// Runway top above the ground (the 0.12 m collider is centered on the ground).
const TOP: f32 = 0.06;
const PAINT: f32 = TOP + 0.004;
const SHOULDER: f32 = 7.5;
/// Blast pad length beyond each threshold.
const BLAST: f32 = 40.0;
/// Taxiway and apron surface heights.
const TAXI_Y: f32 = 0.058;
const APRON_Y: f32 = 0.07;
/// Connector taxiways: center z, the side (+1 west / right, −1 east / left), width.
const TAXI_W: f32 = 15.0;
const CONNECTORS: [(f32, f32); 2] = [(0.0, 1.0), (-5.0, -1.0)];
/// Aprons: x range, z range.
const APRON_W: ((f32, f32), (f32, f32)) = ((45.0, 75.0), (-40.0, 35.0));
const APRON_E: ((f32, f32), (f32, f32)) = ((-62.0, -45.0), (-35.0, 50.0));
/// Holding position distance from the runway centerline.
const HOLD: f32 = 40.0;

fn white() -> Vec4 {
    Vec4::new(0.95, 0.95, 0.93, 1.0)
}

fn yellow() -> Vec4 {
    hex(0xf0b414)
}

/// Queue the airfield scenery for a runway from `start` to `end` (see
/// `demo::spawn_airfield`).
pub fn spawn(commands: &mut Commands, start: DVec3, end: DVec3) {
    commands.queue(move |world: &mut World| {
        build(world, start, end);
    });
}

/// Build the airfield scenery now. `None` without a renderer.
pub fn build(world: &mut World, start: DVec3, end: DVec3) -> Option<BuildStats> {
    let started = Instant::now();
    let along = (end - start).with_y(0.0);
    let length = along.length() as f32;
    if length < 400.0 {
        return None;
    }
    let dir = (along / along.length()).as_vec3();
    let mid = (start + end) * 0.5;
    let mut scene = Scene::new(world, mid, "airfield")?.rotated(facing(dir));
    // Magnetic heading of a take-off from `start`, degrees (0 = north = −Z).
    let heading = dir.x.atan2(-dir.z).to_degrees().rem_euclid(360.0);
    let n1 = designator(heading);
    let n2 = designator(heading + 180.0);
    let f = Field {
        h: length * 0.5,
        ends: [
            End {
                sign: 1.0,
                h: length * 0.5,
                number: n1,
            },
            End {
                sign: -1.0,
                h: length * 0.5,
                number: n2,
            },
        ],
    };
    let mut parts = Parts::default();
    runway_surface(&f, &mut parts);
    runway_markings(&f, &mut parts);
    touchdown_rubber(&f, &mut parts);
    taxiways(&f, &mut parts);
    aprons(&mut parts);
    lights(&f, &mut parts);
    papi(&f, &mut parts);
    signs(&f, &mut parts);
    let hangar_lights = buildings(&mut parts);
    parts.spawn(&mut scene, "airfield");
    for p in hangar_lights.into_iter().take(2) {
        scene.light(
            p.as_dvec3(),
            engine_render::PointLight {
                color: [1.0, 0.95, 0.88],
                intensity_lm: 9_000.0,
                range: 24.0,
                radius: 0.3,
            },
        );
    }
    Some(scene.finish("airfield", started))
}

/// The airfield's static collision shapes in its local frame (runway along z, `length`
/// long): the hangar, fire station, tower, terminal, fuel station, masts, windsocks and the
/// ground equipment. The runway's collider comes from `demo::spawn_airfield`.
pub fn solids(length: f32) -> Vec<Solid> {
    let mut out = buildings::hangar_solids(hangar_at(), &maintenance_hangar());
    out.extend(buildings::fire_station_solids(station_at()));
    let (tower, cab_floor) = tower_at();
    out.extend(buildings::tower_solids(tower, cab_floor));
    let (terminal, w, d) = terminal_at();
    out.extend(buildings::terminal_solids(terminal, w, d));
    for (base, h, _) in MASTS {
        out.push(buildings::floodlight_mast_solid(base, h));
    }
    for sign in [1.0f32, -1.0] {
        let end = End {
            sign,
            h: length * 0.5,
            number: String::new(),
        };
        out.push(buildings::windsock_solid(end.at(150.0, -38.0, 0.0)));
    }
    out.push(Solid::cuboid(
        FUEL + Vec3::Y * 0.25,
        Vec3::new(3.6, 0.25, 1.3),
        Quat::IDENTITY,
        "concrete",
    ));
    out.push(Solid {
        center: FUEL + Vec3::Y * 1.5,
        rotation: Quat::from_rotation_z(FRAC_PI_2),
        shape: engine_world::physics::ColliderShape::Cylinder {
            radius: 1.0,
            half_height: 3.0,
        },
        surface: "metal",
    });
    out.push(Solid::cuboid(
        FUEL + Vec3::new(0.0, 0.6, 2.4),
        Vec3::new(0.5, 0.75, 0.35),
        Quat::IDENTITY,
        "metal",
    ));
    let mut equipment = |at: Place, half: Vec3| {
        out.push(Solid::cuboid(at.p(0.0, half.y, 0.0), half, at.r, "metal"));
    };
    equipment(tug_at(), Vec3::new(0.74, 0.6, 1.4));
    for at in carts_at() {
        equipment(at, Vec3::new(0.75, 0.5, 1.4));
    }
    equipment(gpu_at(), Vec3::new(0.62, 0.78, 1.05));
    out
}

/// Runway designator (two digits) for a heading in degrees.
pub fn designator(heading: f32) -> String {
    let mut n = (heading.rem_euclid(360.0) / 10.0).round() as i32;
    if n <= 0 {
        n += 36;
    }
    if n > 36 {
        n -= 36;
    }
    format!("{n:02}")
}

struct Field {
    /// Half the runway length.
    h: f32,
    ends: [End; 2],
}

/// One runway end, seen by a pilot landing over it.
#[derive(Clone, Debug)]
struct End {
    /// +1 for the start end (landing toward −z), −1 for the other.
    sign: f32,
    h: f32,
    number: String,
}

impl End {
    /// Point `t` meters past the threshold (toward the middle), `r` meters to the pilot's
    /// right, at height `y`.
    fn at(&self, t: f32, r: f32, y: f32) -> Vec3 {
        Vec3::new(self.sign * r, y, self.sign * (self.h - t))
    }

    /// Landing direction.
    fn fwd(&self) -> Vec3 {
        Vec3::new(0.0, 0.0, -self.sign)
    }

    fn right(&self) -> Vec3 {
        Vec3::new(self.sign, 0.0, 0.0)
    }
}

/// A horizontal grid over x0..x1 × z0..z1 with height and color per vertex. UVs = (x, z).
#[allow(clippy::too_many_arguments)]
fn grid(
    b: &mut Builder,
    x0: f32,
    x1: f32,
    z0: f32,
    z1: f32,
    dx: f32,
    dz: f32,
    y: impl Fn(f32, f32) -> f32,
    tone: impl Fn(f32, f32) -> Vec4,
) {
    let nx = ((x1 - x0) / dx).ceil().max(1.0) as u32;
    let nz = ((z1 - z0) / dz).ceil().max(1.0) as u32;
    let base = b.mesh.vertices.len() as u32;
    for j in 0..=nz {
        let z = mix(z0, z1, j as f32 / nz as f32);
        for i in 0..=nx {
            let x = mix(x0, x1, i as f32 / nx as f32);
            let c = tone(x, z);
            b.vertex_c(Vec3::new(x, y(x, z), z), Vec3::Y, Vec2::new(x, z), c);
        }
    }
    let row = nx + 1;
    for j in 0..nz {
        for i in 0..nx {
            let k = base + j * row + i;
            b.quad_idx(k, k + 1, k + row + 1, k + row);
        }
    }
}

// ---------------------------------------------------------------- pavement

fn runway_surface(f: &Field, parts: &mut Parts) {
    let h = f.h;
    // Rubber and wear: dark deposits in both touchdown zones, concentrated round the
    // centerline and the main-gear tracks, streaked along the runway; the edges lighter.
    let tone = move |x: f32, z: f32| -> Vec4 {
        let mut k = 1.0 + 0.07 * world_fbm(x, z, 26.0, 3, 41);
        k *= 1.0 + 0.14 * smoothstep(9.0, 21.0, x.abs());
        for sign in [1.0f32, -1.0] {
            let t = h - sign * z;
            let env = smoothstep(120.0, 260.0, t) * smoothstep(720.0, 380.0, t);
            let lateral =
                smoothstep(13.0, 3.0, x.abs()) * (0.7 + 0.3 * smoothstep(1.0, 4.0, x.abs()));
            let streak = 0.55 + 0.45 * world_fbm(x * 30.0, z, 30.0, 2, 43);
            k *= 1.0 - 0.6 * env * lateral * streak;
        }
        // Oil drips along the centerline.
        k *= 1.0 - 0.12 * smoothstep(2.2, 0.0, x.abs());
        Vec4::new(k, k, k, 1.0)
    };
    grid(
        parts.get(Surface::Runway),
        -HALF,
        HALF,
        -h,
        h,
        1.5,
        4.0,
        |_, _| TOP,
        tone,
    );
    // Shoulders: older, lighter asphalt sloping down to the grass.
    let a = parts.get(Surface::Asphalt);
    let shoulder = |x: f32, z: f32| -> Vec4 {
        let k = 1.45 + 0.12 * world_fbm(x, z, 14.0, 3, 47);
        Vec4::new(k * 1.02, k, k * 0.97, 1.0)
    };
    for side in [-1.0f32, 1.0] {
        let (x0, x1) = if side > 0.0 {
            (HALF, HALF + SHOULDER)
        } else {
            (-HALF - SHOULDER, -HALF)
        };
        grid(
            a,
            x0,
            x1,
            -h - BLAST,
            h + BLAST,
            SHOULDER / 3.0,
            10.0,
            |x, _| {
                mix(
                    TOP - 0.004,
                    0.005,
                    ((x.abs() - HALF) / SHOULDER).clamp(0.0, 1.0),
                )
            },
            shoulder,
        );
    }
    // Blast pads.
    for e in &f.ends {
        let (z0, z1) = if e.sign > 0.0 {
            (h, h + BLAST)
        } else {
            (-h - BLAST, -h)
        };
        grid(
            a,
            -HALF,
            HALF,
            z0,
            z1,
            3.0,
            5.0,
            |_, z| {
                mix(
                    TOP - 0.002,
                    0.01,
                    smoothstep(BLAST - 3.0, BLAST, (z.abs() - h).max(0.0)),
                )
            },
            |x, z| {
                let k = 1.15 + 0.1 * world_fbm(x, z, 9.0, 3, 49);
                Vec4::new(k, k, k, 1.0)
            },
        );
    }
    // The pavement's edge: a dark joint between runway and shoulders.
    let t = parts.get(Surface::Tar);
    t.color(Vec4::new(1.0, 1.0, 1.0, 0.7));
    for side in [-1.0f32, 1.0] {
        rect(
            t,
            Vec3::new(side * HALF, 0.0, 0.0),
            Vec3::Z,
            h,
            0.04,
            TOP + 0.001,
        );
    }
}

fn runway_markings(f: &Field, parts: &mut Parts) {
    let h = f.h;
    let b = parts.get(Surface::PaintRunway);
    b.color(white());
    for e in &f.ends {
        let strip = |b: &mut Builder, t0: f32, t1: f32, r0: f32, r1: f32| {
            let p = [
                e.at(t0, r0, PAINT),
                e.at(t1, r0, PAINT),
                e.at(t1, r1, PAINT),
                e.at(t0, r1, PAINT),
            ];
            flat(b, p, true);
        };
        // Threshold: 12 stripes 1.8 m wide, 30 m long, starting 6 m in.
        for k in 0..6 {
            let r0 = 1.5 + k as f32 * 3.3;
            for s in [-1.0f32, 1.0] {
                strip(b, 6.0, 36.0, s * r0, s * (r0 + 1.8));
            }
        }
        // Aiming point: 45 × 6 m blocks 300 m in, inner edges 18 m apart.
        for s in [-1.0f32, 1.0] {
            strip(b, 300.0, 345.0, s * 9.0, s * 15.0);
        }
        // Touchdown zone: three stripes a side at 150 m, two at 450 m.
        for (t, n) in [(150.0f32, 3), (450.0, 2)] {
            for k in 0..n {
                let r0 = 9.0 + k as f32 * 3.3;
                for s in [-1.0f32, 1.0] {
                    strip(b, t, t + 22.5, s * r0, s * (r0 + 1.8));
                }
            }
        }
    }
    // Centerline: 30 m stripes, 20 m gaps, 0.9 m wide, between the designators.
    let from = h - 70.0;
    let n = ((2.0 * from + 20.0) / 50.0).floor() as i32;
    let used = n as f32 * 50.0 - 20.0;
    let z0 = used * 0.5;
    for k in 0..n {
        let za = z0 - k as f32 * 50.0;
        flat(
            b,
            [
                Vec3::new(-0.45, PAINT, za),
                Vec3::new(0.45, PAINT, za),
                Vec3::new(0.45, PAINT, za - 30.0),
                Vec3::new(-0.45, PAINT, za - 30.0),
            ],
            true,
        );
    }
    // Side stripes along both edges.
    for s in [-1.0f32, 1.0] {
        rect(
            b,
            Vec3::new(s * (HALF - 0.45), 0.0, 0.0),
            Vec3::Z,
            h,
            0.45,
            PAINT,
        );
    }
    // Designators: 9 m digits 12 m beyond the threshold stripes.
    let g = parts.get(Surface::Glyphs);
    g.color(white());
    for e in &f.ends {
        let c = e.at(52.5, 0.0, PAINT + 0.001);
        text(g, c, e.right(), e.fwd(), Vec3::Y, &e.number, 9.0, 0.75);
    }
    // Blast pad chevrons (yellow), pointing at the threshold.
    let y = parts.get(Surface::PaintAsphalt);
    y.color(yellow());
    for e in &f.ends {
        for apex in [-6.0f32, -21.0] {
            for s in [-1.0f32, 1.0] {
                let a = e.at(apex, 0.0, TOP + 0.002);
                let c = e.at(apex - 0.6 * (HALF - 1.0), s * (HALF - 1.0), TOP + 0.002);
                line(y, a, c, 0.9, TOP + 0.002);
            }
        }
    }
}

/// A decal ribbon along points (alpha per point), `w` wide, UV v = meters along.
fn ribbon(b: &mut Builder, path: &[(Vec3, f32)], w: f32) {
    if path.len() < 2 {
        return;
    }
    let base = b.mesh.vertices.len() as u32;
    let mut v = 0.0;
    for i in 0..path.len() {
        let a = path[i.saturating_sub(1)].0;
        let c = path[(i + 1).min(path.len() - 1)].0;
        let d = (c - a).normalize_or(Vec3::Z);
        let side = Vec3::new(-d.z, 0.0, d.x) * (w * 0.5);
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

/// Tire marks from touchdowns: pairs of short dark streaks in both touchdown zones.
fn touchdown_rubber(f: &Field, parts: &mut Parts) {
    let b = parts.get(Surface::Skid);
    for (ei, e) in f.ends.iter().enumerate() {
        for k in 0..170 {
            let seed = (ei * 1000 + k) as i32;
            let t0 = mix(180.0, 620.0, rand1(seed, 3).powf(1.4));
            let len = mix(14.0, 70.0, rand1(seed, 5));
            let gear = mix(1.6, 4.2, rand1(seed, 7));
            let off = (rand1(seed, 9) - 0.5) * 5.0;
            let yaw = (rand1(seed, 11) - 0.5) * 0.03;
            let dark = mix(0.35, 0.9, rand1(seed, 13));
            let w = mix(0.22, 0.55, rand1(seed, 15));
            b.color(Vec4::new(1.0, 1.0, 1.0, dark));
            for s in [-1.0f32, 1.0] {
                let pts: Vec<(Vec3, f32)> = (0..=8)
                    .map(|i| {
                        let u = i as f32 / 8.0;
                        let t = t0 + u * len;
                        let r = off + s * gear + yaw * (t - t0);
                        let a = smoothstep(0.0, 0.08, u) * smoothstep(1.0, 0.5, u);
                        (e.at(t, r, PAINT + 0.002), a)
                    })
                    .collect();
                ribbon(b, &pts, w);
            }
        }
    }
}

fn taxiways(f: &Field, parts: &mut Parts) {
    let _ = f;
    let hw = TAXI_W * 0.5;
    for (zc, side) in CONNECTORS {
        let x_apron = if side > 0.0 { APRON_W.0.0 } else { APRON_E.0.1 };
        let (x0, x1) = (HALF, x_apron.abs() + 0.5);
        // Pavement with fillets where it meets the runway.
        let a = parts.get(Surface::Asphalt);
        let fillet = 12.0f32;
        let tone = |x: f32, z: f32| {
            let k = 1.08 + 0.08 * world_fbm(x, z, 11.0, 3, 51);
            Vec4::new(k, k, k, 1.0)
        };
        let n = 10;
        let base = a.mesh.vertices.len() as u32;
        // Columns from the runway edge outward; the width flares within `fillet` of it.
        let cols = 14u32;
        for i in 0..=cols {
            let x = mix(x0, x1, i as f32 / cols as f32);
            let flare = {
                let d = (x - HALF).clamp(0.0, fillet);
                fillet
                    - (fillet * fillet - (fillet - d) * (fillet - d))
                        .max(0.0)
                        .sqrt()
            };
            let w = hw + flare;
            for j in 0..=n {
                let z = zc - w + 2.0 * w * j as f32 / n as f32;
                let p = Vec3::new(side * x, TAXI_Y, z);
                let c = tone(p.x, p.z);
                a.vertex_c(p, Vec3::Y, Vec2::new(p.x, p.z), c);
            }
        }
        let row = n + 1;
        for i in 0..cols {
            for j in 0..n {
                let k = base + i * row + j;
                a.quad_idx(k, k + 1, k + row + 1, k + row);
            }
        }
        // Taxiway shoulders.
        for s in [-1.0f32, 1.0] {
            let z = zc + s * (hw + 1.75);
            let p0 = Vec3::new(side * (HALF + SHOULDER + fillet * 0.4), 0.0, z);
            let p1 = Vec3::new(side * (x1 - 0.5), 0.0, z);
            let a = parts.get(Surface::Asphalt);
            a.color(gray(1.45));
            line(a, p0, p1, 3.5, 0.03);
        }
        // Centerline (yellow) curving onto the runway centerline both ways, edge lines,
        // holding position (pattern A).
        let p = parts.get(Surface::PaintAsphalt);
        p.color(yellow());
        line(
            p,
            Vec3::new(side * (HALF + 0.5), 0.0, zc),
            Vec3::new(side * (x1 - 0.2), 0.0, zc),
            0.15,
            TAXI_Y + 0.003,
        );
        for s in [-1.0f32, 1.0] {
            for e in [hw - 0.2, hw - 0.5] {
                line(
                    p,
                    Vec3::new(side * (HALF + fillet + 0.5), 0.0, zc + s * e),
                    Vec3::new(side * (x1 - 0.2), 0.0, zc + s * e),
                    0.15,
                    TAXI_Y + 0.003,
                );
            }
        }
        for (k, solid) in [(0.0f32, true), (0.3, true), (0.6, false), (0.9, false)] {
            let x = side * (HOLD + 0.45 - k);
            let a = Vec3::new(x, 0.0, zc - hw + 0.3);
            let c = Vec3::new(x, 0.0, zc + hw - 0.3);
            if solid {
                line(p, a, c, 0.15, TAXI_Y + 0.003);
            } else {
                dashed(p, a, c, 0.15, TAXI_Y + 0.003, 0.9, 0.9);
            }
        }
        // Lead-on lines over the runway (yellow on the runway paint layer).
        let r = parts.get(Surface::PaintRunway);
        r.color(yellow());
        let rad = HALF;
        for s in [-1.0f32, 1.0] {
            // Quarter circle from (side·HALF, zc) heading inward to (0, zc + s·rad).
            let center = Vec3::new(side * HALF, 0.0, zc + s * rad);
            let pts: Vec<Vec3> = (0..=16)
                .map(|i| {
                    let a = i as f32 / 16.0 * FRAC_PI_2;
                    center + Vec3::new(-side * a.sin() * rad, 0.0, -s * a.cos() * rad)
                })
                .collect();
            for w in pts.windows(2) {
                line(r, w[0], w[1], 0.15, PAINT + 0.0005);
            }
        }
        let _ = f.h;
    }
}

fn aprons(parts: &mut Parts) {
    for ((x0, x1), (z0, z1)) in [APRON_W, APRON_E] {
        let s = parts.get(Surface::Slabs);
        let tone = |x: f32, z: f32| {
            let k = 1.0 + 0.08 * world_fbm(x, z, 12.0, 3, 61);
            Vec4::new(k, k, k * 0.99, 1.0)
        };
        grid(s, x0, x1, z0, z1, 2.5, 2.5, |_, _| APRON_Y, tone);
        // Edge: a small sloped concrete kerb down to the grass.
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.9));
        let ring = [
            Vec3::new(x0, 0.0, z0),
            Vec3::new(x1, 0.0, z0),
            Vec3::new(x1, 0.0, z1),
            Vec3::new(x0, 0.0, z1),
        ];
        for k in 0..4 {
            let (a, b) = (ring[k], ring[(k + 1) % 4]);
            let d = (b - a).normalize();
            let out = Vec3::new(d.z, 0.0, -d.x);
            let q = [
                a + Vec3::Y * APRON_Y,
                b + Vec3::Y * APRON_Y,
                b + out * 0.25 + Vec3::Y * 0.0,
                a + out * 0.25 + Vec3::Y * 0.0,
            ];
            let n = (out + Vec3::Y * 2.0).normalize();
            c.quad_n(q, q.map(|v| Vec2::new(v.x + v.z, v.y)), n);
        }
    }
    // West apron: taxi lane, three stands with lead-in lines, stop bars and numbers,
    // tie-downs; an apron edge line.
    let lane_x = 52.0f32;
    let (zs0, zs1) = APRON_W.1;
    let p = parts.get(Surface::PaintSlabs);
    p.color(yellow());
    line(
        p,
        Vec3::new(lane_x, 0.0, zs0 + 3.0),
        Vec3::new(lane_x, 0.0, zs1 - 3.0),
        0.15,
        APRON_Y + 0.003,
    );
    line(
        p,
        Vec3::new(APRON_W.0.0, 0.0, 0.0),
        Vec3::new(lane_x, 0.0, 0.0),
        0.15,
        APRON_Y + 0.003,
    );
    let stands = [-32.0f32, -18.0, 16.0];
    for z in stands {
        // Lead-in: a curve off the taxi lane, then straight to the stop bar.
        let r = 7.0f32;
        let sgn = if z < 0.0 { 1.0 } else { -1.0 };
        let center = Vec3::new(lane_x + r, 0.0, z + sgn * r);
        let pts: Vec<Vec3> = (0..=12)
            .map(|i| {
                let a = i as f32 / 12.0 * FRAC_PI_2;
                center + Vec3::new(-a.cos() * r, 0.0, -sgn * a.sin() * r)
            })
            .collect();
        for w in pts.windows(2) {
            line(p, w[0], w[1], 0.15, APRON_Y + 0.003);
        }
        line(
            p,
            Vec3::new(lane_x + r, 0.0, z),
            Vec3::new(69.0, 0.0, z),
            0.15,
            APRON_Y + 0.003,
        );
        line(
            p,
            Vec3::new(69.0, 0.0, z - 1.5),
            Vec3::new(69.0, 0.0, z + 1.5),
            0.3,
            APRON_Y + 0.003,
        );
    }
    // Apron edge and the red safety line in front of the tower building.
    {
        let x = APRON_W.0.1 - 1.0;
        let p = parts.get(Surface::PaintSlabs);
        p.color(hex(0xb02018));
        dashed(
            p,
            Vec3::new(x, 0.0, zs0 + 1.0),
            Vec3::new(x, 0.0, zs1 - 1.0),
            0.12,
            APRON_Y + 0.003,
            2.0,
            1.0,
        );
    }
    let g = parts.get(Surface::Glyphs);
    g.color(yellow());
    for (i, z) in stands.iter().enumerate() {
        text(
            g,
            Vec3::new(71.5, APRON_Y + 0.004, *z),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::X,
            Vec3::Y,
            &format!("{}", i + 1),
            1.8,
            1.0,
        );
    }
    // Tie-down rings.
    let t = parts.get(Surface::Galvanized);
    t.color(Vec4::ONE);
    for z in stands {
        for dz in [-4.5f32, 4.5] {
            let c = Vec3::new(64.0, APRON_Y + 0.01, z + dz);
            t.torus(c, Vec3::Y, 0.06, 0.012, 10, 4);
        }
    }
    // East apron: lead line from the connector into the hangar.
    let p = parts.get(Surface::PaintSlabs);
    p.color(yellow());
    let (zc, _) = CONNECTORS[1];
    line(
        p,
        Vec3::new(APRON_E.0.1, 0.0, zc),
        Vec3::new(-53.0, 0.0, zc),
        0.15,
        APRON_Y + 0.003,
    );
    let r = 9.0f32;
    let center = Vec3::new(-53.0, 0.0, zc + r);
    let pts: Vec<Vec3> = (0..=12)
        .map(|i| {
            let a = i as f32 / 12.0 * FRAC_PI_2;
            center + Vec3::new(-a.sin() * r, 0.0, -a.cos() * r)
        })
        .collect();
    for w in pts.windows(2) {
        line(p, w[0], w[1], 0.15, APRON_Y + 0.003);
    }
    let _ = pts;
}

// ---------------------------------------------------------------- lights

/// An elevated runway light: frangible stem, yellow base can, glass dome.
fn elevated(parts: &mut Parts, p: Vec3, lens: Surface) {
    let c = parts.get(Surface::Painted);
    c.color(hex(0xd8a40a));
    c.cylinder(p, p + Vec3::Y * 0.1, 0.11, 0.11, 10, true);
    let g = parts.get(Surface::Galvanized);
    g.color(Vec4::ONE);
    g.cylinder(p + Vec3::Y * 0.1, p + Vec3::Y * 0.32, 0.03, 0.03, 6, false);
    let l = parts.get(lens);
    l.color(Vec4::ONE);
    l.cylinder(p + Vec3::Y * 0.32, p + Vec3::Y * 0.42, 0.08, 0.07, 10, true);
}

/// A light that shows `front` toward `face` and `back` the other way (threshold / end):
/// frangible stem on a yellow base can, a drum-shaped head tipped up 3° with a domed lens in
/// each lit end.
fn directional(parts: &mut Parts, p: Vec3, face: Vec3, front: Surface, back: Option<Surface>) {
    let c = parts.get(Surface::Painted);
    c.color(hex(0xd8a40a));
    c.cylinder(p, p + Vec3::Y * 0.12, 0.12, 0.12, 14, true);
    let g = parts.get(Surface::Galvanized);
    g.color(Vec4::ONE);
    g.cylinder(p + Vec3::Y * 0.12, p + Vec3::Y * 0.22, 0.03, 0.03, 8, false);
    let center = p + Vec3::Y * 0.3;
    let axis = (face + Vec3::Y * 0.05).normalize();
    let (rear, front_end) = (center - axis * 0.1, center + axis * 0.1);
    let h = parts.get(Surface::Painted);
    h.color(gray(0.2));
    h.cylinder(rear, front_end, 0.1, 0.1, 16, true);
    for (end, dir, s) in [(front_end, axis, Some(front)), (rear, -axis, back)] {
        let Some(s) = s else { continue };
        let l = parts.get(s);
        l.color(Vec4::ONE);
        l.cylinder(end, end + dir * 0.02, 0.085, 0.06, 16, true);
    }
}

fn lights(f: &Field, parts: &mut Parts) {
    let h = f.h;
    // Edge lights every 60 m, 1.5 m outside the edge; amber over the last 400 m toward
    // the far end.
    let n = (2.0 * h / 60.0).round() as i32;
    for k in 0..=n {
        let z = h - k as f32 * (2.0 * h / n as f32);
        for s in [-1.0f32, 1.0] {
            let lens = if z < -h + 400.0 {
                Surface::LensAmber
            } else {
                Surface::LensWhite
            };
            elevated(parts, Vec3::new(s * (HALF + 1.5), 0.0, z), lens);
        }
    }
    for e in &f.ends {
        // Threshold (green, toward the approach) and end lights (red, toward the runway).
        let mut r = -HALF + 0.5;
        while r <= HALF - 0.5 {
            directional(
                parts,
                e.at(-1.2, r, 0.0),
                -e.fwd(),
                Surface::LensGreen,
                Some(Surface::LensRed),
            );
            r += 3.0;
        }
        // Wing bars.
        for s in [-1.0f32, 1.0] {
            for k in 0..5 {
                let r = s * (HALF + 3.0 + k as f32 * 3.0);
                directional(
                    parts,
                    e.at(-1.2, r, 0.0),
                    -e.fwd(),
                    Surface::LensGreen,
                    None,
                );
            }
        }
        // Approach barrettes on the blast pad and a crossbar.
        for t in [-15.0f32, -30.0] {
            for k in -2..=2 {
                let p = e.at(t, k as f32 * 1.0, TOP);
                approach_light(parts, p, -e.fwd());
            }
        }
        for s in [-1.0f32, 1.0] {
            for k in 0..8 {
                let p = e.at(-30.0, s * (4.5 + k as f32 * 1.5), TOP);
                approach_light(parts, p, -e.fwd());
            }
        }
    }
    // Taxiway edge lights (blue) along the connectors and round the aprons.
    let blue = |parts: &mut Parts, p: Vec3| {
        let c = parts.get(Surface::Painted);
        c.color(hex(0xd8a40a));
        c.cylinder(p, p + Vec3::Y * 0.08, 0.09, 0.09, 8, true);
        let g = parts.get(Surface::Galvanized);
        g.color(Vec4::ONE);
        g.cylinder(
            p + Vec3::Y * 0.08,
            p + Vec3::Y * 0.3,
            0.025,
            0.025,
            6,
            false,
        );
        let l = parts.get(Surface::LensBlue);
        l.color(Vec4::ONE);
        l.cylinder(p + Vec3::Y * 0.3, p + Vec3::Y * 0.4, 0.06, 0.05, 10, true);
    };
    for (zc, side) in CONNECTORS {
        let x_apron = if side > 0.0 { APRON_W.0.0 } else { APRON_E.0.1 };
        let mut x = HALF + SHOULDER + 8.0;
        while x < x_apron.abs() - 1.0 {
            for s in [-1.0f32, 1.0] {
                blue(
                    parts,
                    Vec3::new(side * x, 0.0, zc + s * (TAXI_W * 0.5 + 1.2)),
                );
            }
            x += 7.5;
        }
    }
    for ((x0, x1), (z0, z1)) in [APRON_W, APRON_E] {
        let corners = [
            Vec3::new(x0, 0.0, z0),
            Vec3::new(x1, 0.0, z0),
            Vec3::new(x1, 0.0, z1),
            Vec3::new(x0, 0.0, z1),
        ];
        for k in 0..4 {
            let (a, b) = (corners[k], corners[(k + 1) % 4]);
            // The buildings stand along the far side.
            let far = x0.abs().max(x1.abs());
            if (a.x - b.x).abs() < 0.01 && a.x.abs() > far - 0.01 {
                continue;
            }
            let len = (b - a).length();
            let d = (b - a) / len;
            let out = Vec3::new(d.z, 0.0, -d.x);
            let mut t = 0.0;
            while t < len {
                let p = a + d * t + out * 1.0;
                // Leave the connector mouths clear.
                let in_mouth = CONNECTORS.iter().any(|(zc, _)| {
                    (p.z - zc).abs() < TAXI_W * 0.5 + 2.0
                        && (p.x.abs() - x0.abs().min(x1.abs())).abs() < 2.0
                });
                if !in_mouth {
                    blue(parts, p);
                }
                t += 15.0;
            }
        }
    }
}

fn approach_light(parts: &mut Parts, p: Vec3, face: Vec3) {
    let g = parts.get(Surface::Galvanized);
    g.color(Vec4::ONE);
    g.cylinder(p, p + Vec3::Y * 0.45, 0.04, 0.04, 6, false);
    let c = parts.get(Surface::Painted);
    c.color(gray(0.2));
    let head = p + Vec3::Y * 0.55;
    c.cylinder(head - face * 0.12, head + face * 0.05, 0.13, 0.13, 10, true);
    let l = parts.get(Surface::LensWhite);
    l.color(Vec4::ONE);
    l.cylinder(head + face * 0.05, head + face * 0.07, 0.11, 0.11, 10, true);
}

/// Precision approach path indicators on the left of each runway, abeam the aiming point.
fn papi(f: &Field, parts: &mut Parts) {
    for e in &f.ends {
        let face = -e.fwd();
        let rot = buildings::facing_z(face);
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.85));
        let pad = e.at(300.0, -38.0, 0.06);
        c.chamfer_box(pad, Vec3::new(9.5, 0.06, 1.6), 0.03, rot);
        for (k, red) in [(0, true), (1, true), (2, false), (3, false)] {
            let r = -(HALF + 8.0 + k as f32 * 5.0);
            let base = e.at(300.0, r, 0.12);
            let p = parts.get(Surface::Painted);
            p.color(hex(0xd99a0b));
            for (dx, dz) in [(-0.45f32, -0.3f32), (0.45, -0.3), (-0.45, 0.3), (0.45, 0.3)] {
                let q = base + rot * Vec3::new(dx, 0.0, dz);
                p.cylinder(q, q + Vec3::Y * 0.45, 0.04, 0.04, 6, false);
            }
            let body = base + Vec3::Y * 0.75;
            p.color(gray(0.82));
            p.chamfer_box(body, Vec3::new(0.62, 0.3, 0.45), 0.04, rot);
            p.color(gray(0.15));
            p.chamfer_box(
                body + face * 0.5 + Vec3::Y * 0.27,
                Vec3::new(0.62, 0.02, 0.08),
                0.0,
                rot,
            );
            let lens = if red {
                Surface::LensRed
            } else {
                Surface::LensWhite
            };
            let l = parts.get(lens);
            l.color(Vec4::ONE);
            for j in -1..=1 {
                let side = Vec3::new(-face.z, 0.0, face.x);
                let q = body + face * 0.46 + side * (j as f32 * 0.38);
                l.cylinder(q, q + face * 0.03, 0.14, 0.14, 14, true);
            }
        }
    }
}

// ---------------------------------------------------------------- signs

/// A two-panel taxiway sign: a mandatory (red/white) runway designation and a location
/// (black/yellow) panel, readable from `facing`.
fn holding_sign(parts: &mut Parts, center: Vec3, facing: Vec3, runway: &str, location: &str) {
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize();
    let right = Vec3::Y.cross(f).normalize();
    let rot = buildings::facing_z(f);
    let h = 0.9f32;
    let y = 0.45f32;
    let w_run = buildings::text_width(runway, 0.5, 1.0) + 0.5;
    let w_loc = 0.9f32;
    let total = w_run + w_loc;
    let g = parts.get(Surface::Painted);
    g.color(gray(0.12));
    g.chamfer_box(
        center + Vec3::Y * (y + h * 0.5),
        Vec3::new(total * 0.5 + 0.05, h * 0.5 + 0.05, 0.12),
        0.02,
        rot,
    );
    let gal = parts.get(Surface::Galvanized);
    gal.color(Vec4::ONE);
    for s in [-0.38f32, 0.38] {
        let p = center + right * (total * s);
        gal.cylinder(p, p + Vec3::Y * (y + 0.1), 0.04, 0.04, 8, false);
    }
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.85));
    c.chamfer_box(
        center + Vec3::Y * 0.06,
        Vec3::new(total * 0.45, 0.08, 0.35),
        0.02,
        rot,
    );
    for side in [1.0f32, -1.0] {
        let n = f * side;
        let r = right * side;
        let x_run = -total * 0.5 + w_run * 0.5;
        let x_loc = total * 0.5 - w_loc * 0.5;
        // Reader on the back sees the panels in reverse order.
        let (x_run, x_loc) = if side > 0.0 {
            (x_run, x_loc)
        } else {
            (-x_run, -x_loc)
        };
        let face = center + Vec3::Y * (y + h * 0.5) + n * 0.125;
        let p = parts.get(Surface::Painted);
        p.color(hex(0xb3140f));
        let q = |p: &mut Builder, cx: f32, w: f32| {
            let c = face + r * cx;
            p.quad_n(
                [
                    c - r * (w * 0.5) - Vec3::Y * (h * 0.5),
                    c + r * (w * 0.5) - Vec3::Y * (h * 0.5),
                    c + r * (w * 0.5) + Vec3::Y * (h * 0.5),
                    c - r * (w * 0.5) + Vec3::Y * (h * 0.5),
                ],
                [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
                n,
            );
        };
        q(p, x_run, w_run);
        p.color(gray(0.02));
        q(p, x_loc, w_loc);
        // Yellow border on the location panel.
        p.color(yellow());
        let c = face + r * x_loc + n * 0.002;
        for (dx, dy, hw, hh) in [
            (0.0, 0.38, 0.38, 0.03),
            (0.0, -0.38, 0.38, 0.03),
            (0.35, 0.0, 0.03, 0.38),
            (-0.35, 0.0, 0.03, 0.38),
        ] {
            let cc = c + r * dx + Vec3::Y * dy;
            p.quad_n(
                [
                    cc - r * hw - Vec3::Y * hh,
                    cc + r * hw - Vec3::Y * hh,
                    cc + r * hw + Vec3::Y * hh,
                    cc - r * hw + Vec3::Y * hh,
                ],
                [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
                n,
            );
        }
        let g = parts.get(Surface::Glyphs);
        g.color(white());
        text(
            g,
            face + r * x_run + n * 0.003,
            r,
            Vec3::Y,
            n,
            runway,
            0.5,
            1.0,
        );
        g.color(yellow());
        text(
            g,
            face + r * x_loc + n * 0.003,
            r,
            Vec3::Y,
            n,
            location,
            0.5,
            1.0,
        );
    }
}

fn signs(f: &Field, parts: &mut Parts) {
    let pair = format!("{}-{}", f.ends[0].number, f.ends[1].number);
    let pair_rev = format!("{}-{}", f.ends[1].number, f.ends[0].number);
    for ((zc, side), loc) in CONNECTORS.iter().zip(["A", "B"]) {
        // On the pilot's left when taxiing toward the runway (heading −side·x): +z for the
        // west connector, −z for the east one. The designator nearest that side comes
        // first.
        let left = *side;
        let center = Vec3::new(side * (HOLD + 0.5), 0.0, zc + left * (TAXI_W * 0.5 + 4.0));
        let text = if *side > 0.0 { &pair } else { &pair_rev };
        holding_sign(parts, center, Vec3::new(*side, 0.0, 0.0), text, loc);
    }
    // Windsocks near both thresholds on the left, the wind blowing down the runway.
    for e in &f.ends {
        buildings::windsock(parts, e.at(150.0, -38.0, 0.0), Vec3::Z);
    }
}

// ---------------------------------------------------------------- buildings

/// Maintenance hangar on the east apron, doors facing the runway (+x), and the fire station
/// north of it, bays facing the runway.
fn hangar_at() -> Place {
    Place::new(Vec3::new(APRON_E.0.0, 0.0, -8.0), -FRAC_PI_2)
}
fn station_at() -> Place {
    Place::new(Vec3::new(APRON_E.0.0, 0.0, 29.0), -FRAC_PI_2)
}
/// Control tower at the back of the west apron (cab floor height), the terminal south of it
/// (width, depth).
fn tower_at() -> (Place, f32) {
    (Place::new(Vec3::new(79.0, 0.0, 24.0), FRAC_PI_2), 15.5)
}
fn terminal_at() -> (Place, f32, f32) {
    (
        Place::new(Vec3::new(77.0, 0.0, -11.0), FRAC_PI_2),
        28.0,
        9.0,
    )
}
/// Floodlight masts over both aprons: base, height, aim.
const MASTS: [(Vec3, f32, Vec3); 2] = [
    (
        Vec3::new(76.5, 0.0, -38.0),
        18.0,
        Vec3::new(58.0, 0.0, -5.0),
    ),
    (
        Vec3::new(-47.0, 0.0, 22.0),
        16.0,
        Vec3::new(-54.0, 0.0, -10.0),
    ),
];
/// The fuel station's bund center on the west apron.
const FUEL: Vec3 = Vec3::new(70.0, APRON_Y, -38.5);
/// Ground equipment on the west apron: the tug, its two carts and the ground power unit.
fn tug_at() -> Place {
    Place::new(Vec3::new(72.0, APRON_Y, -3.0), PI)
}
fn carts_at() -> [Place; 2] {
    [
        Place::new(Vec3::new(72.0, APRON_Y, -6.6), PI),
        Place::new(Vec3::new(72.0, APRON_Y, -10.3), PI + 0.04),
    ]
}
fn gpu_at() -> Place {
    Place::new(Vec3::new(66.5, APRON_Y, -24.5), 0.3)
}

fn maintenance_hangar() -> Hangar {
    Hangar {
        w: 30.0,
        d: 22.0,
        eave: 9.0,
        rise: 2.6,
        door_h: 8.0,
        open: (0.15, 0.85),
        wall: hex(0xb9c0c4),
        door: hex(0xe4e6e3),
        trim: hex(0x24384f),
        board: super::tex::board_index("AURORA"),
        floor: APRON_Y + 0.06,
    }
}

fn buildings(parts: &mut Parts) -> Vec<Vec3> {
    let lights = buildings::hangar(parts, hangar_at(), &maintenance_hangar());
    buildings::fire_station(parts, station_at());
    fire_forecourt(parts, station_at());
    let (tower, cab_floor) = tower_at();
    buildings::tower(parts, tower, cab_floor);
    let (terminal, w, d) = terminal_at();
    buildings::terminal(parts, terminal, w, d, "AURORA JET CENTER");
    ground_equipment(parts);
    for (base, h, aim) in MASTS {
        buildings::floodlight_mast(parts, base, h, aim, 4);
    }
    // Fuel station on the west apron: a bunded double-skin tank and a dispenser.
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.92));
    let fuel = FUEL;
    c.chamfer_box(
        fuel + Vec3::Y * 0.25,
        Vec3::new(3.6, 0.25, 1.3),
        0.04,
        Quat::IDENTITY,
    );
    let t = parts.get(Surface::Painted);
    t.color(hex(0x2f6b3a));
    t.cylinder(
        fuel + Vec3::new(-3.0, 1.5, 0.0),
        fuel + Vec3::new(3.0, 1.5, 0.0),
        1.0,
        1.0,
        20,
        true,
    );
    t.color(gray(0.85));
    t.chamfer_box(
        fuel + Vec3::new(0.0, 0.6, 2.4),
        Vec3::new(0.5, 0.75, 0.35),
        0.03,
        Quat::IDENTITY,
    );
    let _ = PI;
    lights
}

/// The fire station's forecourt: lane lines from each bay and a hatched keep-clear box.
fn fire_forecourt(parts: &mut Parts, at: Place) {
    let y = APRON_Y + 0.003;
    let p = parts.get(Surface::PaintSlabs);
    p.color(yellow());
    let (x0, x1) = (-10.5f32, 10.5f32);
    let (z0, z1) = (-1.0f32, -10.0f32);
    for (a, b) in [
        ((x0, z0), (x1, z0)),
        ((x1, z0), (x1, z1)),
        ((x1, z1), (x0, z1)),
        ((x0, z1), (x0, z0)),
    ] {
        line(p, at.p(a.0, 0.0, a.1), at.p(b.0, 0.0, b.1), 0.2, y);
    }
    // Diagonal hatching, clipped to the box.
    let mut t = x0 - (z0 - z1);
    while t < x1 {
        let (mut ax, mut az) = (t, z0);
        let (mut bx, mut bz) = (t + (z0 - z1), z1);
        if ax < x0 {
            az -= x0 - ax;
            ax = x0;
        }
        if bx > x1 {
            bz += bx - x1;
            bx = x1;
        }
        if az > bz {
            line(p, at.p(ax, 0.0, az), at.p(bx, 0.0, bz), 0.3, y);
        }
        t += 1.6;
    }
    let g = parts.get(Surface::Glyphs);
    g.color(yellow());
    let c = at.p(0.0, y + 0.001, -11.6);
    // Read from the apron, walking up to the doors.
    text(
        g,
        c,
        at.v(Vec3::NEG_X),
        at.v(Vec3::Z),
        Vec3::Y,
        "KEEP CLEAR",
        1.1,
        1.0,
    );
}

/// Ground equipment on the west apron: a baggage tug with two carts beside the terminal, a
/// ground power unit and cones at the stands, chocks waiting on the stop bars.
fn ground_equipment(parts: &mut Parts) {
    let y = APRON_Y;
    tug(parts, tug_at());
    for at in carts_at() {
        cart(parts, at);
    }
    gpu(parts, gpu_at());
    for (x, z) in [
        (63.5f32, -24.0f32),
        (63.5, -12.0),
        (66.0, -32.0 + 6.5),
        (60.0, 10.0),
        (60.0, 22.0),
        (72.6, 1.2),
    ] {
        cone(parts, Vec3::new(x, y, z));
    }
    for z in [-32.0f32, -18.0, 16.0] {
        for dz in [-0.45f32, 0.45] {
            chock(parts, Vec3::new(70.2, y, z + dz));
        }
    }
}

fn cone(parts: &mut Parts, at: Vec3) {
    let c = parts.get(Surface::Plastic);
    c.color(hex(0xff4a0a));
    c.chamfer_box(
        at + Vec3::Y * 0.02,
        Vec3::new(0.19, 0.02, 0.19),
        0.01,
        Quat::IDENTITY,
    );
    c.cylinder(
        at + Vec3::Y * 0.04,
        at + Vec3::Y * 0.75,
        0.15,
        0.03,
        14,
        true,
    );
    c.color(gray(0.95));
    let r = |y: f32| mix(0.15, 0.03, (y - 0.04) / 0.71) + 0.003;
    for (y0, y1) in [(0.36f32, 0.46f32), (0.54, 0.6)] {
        c.cylinder(
            at + Vec3::Y * y0,
            at + Vec3::Y * y1,
            r(y0),
            r(y1),
            14,
            false,
        );
    }
}

fn chock(parts: &mut Parts, at: Vec3) {
    let c = parts.get(Surface::Rubber);
    c.color(Vec4::ONE);
    c.chamfer_box(
        at + Vec3::Y * 0.07,
        Vec3::new(0.12, 0.07, 0.25),
        0.03,
        Quat::IDENTITY,
    );
    let p = parts.get(Surface::Painted);
    p.color(hex(0xe0a812));
    p.cylinder(
        at + Vec3::new(0.0, 0.12, -0.25),
        at + Vec3::new(0.0, 0.12, 0.25),
        0.015,
        0.015,
        6,
        false,
    );
}

/// Four wheels on axles across x at frame z `zs`, radius `r`, track `half` from the middle.
fn wheels(parts: &mut Parts, at: Place, zs: &[f32], r: f32, half: f32, w: f32) {
    for &z in zs {
        for s in [-1.0f32, 1.0] {
            let t = parts.get(Surface::Rubber);
            t.color(Vec4::ONE);
            let c = at.p(s * half, r, z);
            let axis = at.v(Vec3::X) * s;
            t.cylinder(c - axis * w * 0.5, c + axis * w * 0.5, r, r, 16, true);
            let h = parts.get(Surface::Painted);
            h.color(gray(0.7));
            h.cylinder(
                c + axis * w * 0.5,
                c + axis * (w * 0.5 + 0.01),
                r * 0.55,
                r * 0.55,
                10,
                true,
            );
        }
    }
}

/// A baggage tug facing −z of `at`: low yellow body, seat, canopy on four posts.
fn tug(parts: &mut Parts, at: Place) {
    let p = |x: f32, y: f32, z: f32| at.p(x, y, z);
    let b = parts.get(Surface::Painted);
    b.color(hex(0xe2aa10));
    b.chamfer_box(p(0.0, 0.62, 0.0), Vec3::new(0.72, 0.32, 1.3), 0.06, at.r);
    b.chamfer_box(p(0.0, 1.05, -0.75), Vec3::new(0.66, 0.14, 0.5), 0.05, at.r);
    b.color(gray(0.1));
    b.chamfer_box(p(0.0, 0.45, -1.33), Vec3::new(0.74, 0.12, 0.06), 0.02, at.r);
    b.chamfer_box(p(0.0, 0.5, 1.36), Vec3::new(0.3, 0.1, 0.1), 0.02, at.r);
    // Seat and steering wheel.
    b.color(gray(0.15));
    b.chamfer_box(p(0.0, 1.05, 0.35), Vec3::new(0.32, 0.08, 0.26), 0.04, at.r);
    b.chamfer_box(p(0.0, 1.38, 0.62), Vec3::new(0.32, 0.28, 0.06), 0.04, at.r);
    let g = parts.get(Surface::Galvanized);
    g.color(gray(0.5));
    g.cylinder(
        p(0.0, 1.1, -0.15),
        p(0.0, 1.45, -0.05),
        0.025,
        0.025,
        6,
        false,
    );
    g.torus(
        p(0.0, 1.47, -0.05),
        at.v(Vec3::new(0.0, 1.0, 0.3).normalize()),
        0.17,
        0.018,
        16,
        5,
    );
    // Canopy.
    g.color(gray(0.2));
    for (x, z) in [
        (-0.62f32, -0.45f32),
        (0.62, -0.45),
        (-0.62, 0.95),
        (0.62, 0.95),
    ] {
        g.cylinder(p(x, 0.94, z), p(x, 2.05, z), 0.03, 0.03, 6, false);
    }
    let r = parts.get(Surface::Painted);
    r.color(gray(0.85));
    r.chamfer_box(p(0.0, 2.08, 0.25), Vec3::new(0.72, 0.04, 0.85), 0.02, at.r);
    let a = parts.get(Surface::LensAmber);
    a.color(Vec4::ONE);
    a.cylinder(p(0.0, 2.12, 0.25), p(0.0, 2.24, 0.25), 0.06, 0.05, 10, true);
    wheels(parts, at, &[-0.85, 0.85], 0.3, 0.6, 0.22);
}

/// A covered baggage cart facing −z of `at` with its drawbar forward.
fn cart(parts: &mut Parts, at: Place) {
    let p = |x: f32, y: f32, z: f32| at.p(x, y, z);
    let b = parts.get(Surface::Painted);
    b.color(gray(0.25));
    b.chamfer_box(p(0.0, 0.5, 0.0), Vec3::new(0.75, 0.06, 1.4), 0.02, at.r);
    b.color(hex(0x3f5b78));
    for s in [-1.0f32, 1.0] {
        b.chamfer_box(
            p(s * 0.72, 0.75, 0.0),
            Vec3::new(0.03, 0.2, 1.38),
            0.01,
            at.r,
        );
    }
    b.chamfer_box(p(0.0, 0.75, 1.37), Vec3::new(0.72, 0.2, 0.03), 0.01, at.r);
    // Corner posts, roof and the canvas curtains rolled up.
    let g = parts.get(Surface::Galvanized);
    g.color(gray(0.7));
    for (x, z) in [(-0.7f32, -1.35f32), (0.7, -1.35), (-0.7, 1.35), (0.7, 1.35)] {
        g.cylinder(p(x, 0.56, z), p(x, 1.95, z), 0.025, 0.025, 6, false);
    }
    g.cylinder(p(0.0, 0.42, -1.4), p(0.0, 0.32, -2.2), 0.03, 0.03, 6, false);
    g.torus(p(0.0, 0.32, -2.27), Vec3::Y, 0.07, 0.015, 10, 4);
    let r = parts.get(Surface::Plastic);
    r.color(hex(0x2f4e6e));
    r.chamfer_box(p(0.0, 1.98, 0.0), Vec3::new(0.78, 0.04, 1.45), 0.02, at.r);
    for s in [-1.0f32, 1.0] {
        r.cylinder(
            p(s * 0.76, 1.85, -1.4),
            p(s * 0.76, 1.85, 1.4),
            0.07,
            0.07,
            10,
            true,
        );
    }
    // A couple of bags.
    let bag = parts.get(Surface::Plastic);
    for (x, z, c) in [
        (-0.3f32, -0.6f32, 0x1a1a1c),
        (0.3, -0.5, 0x6a2020),
        (0.0, 0.5, 0x22303e),
    ] {
        bag.color(hex(c));
        bag.chamfer_box(p(x, 0.75, z), Vec3::new(0.25, 0.19, 0.36), 0.06, at.r);
    }
    wheels(parts, at, &[-0.95, 0.95], 0.2, 0.62, 0.14);
}

/// A ground power unit facing −z of `at`: a white cabinet on a small chassis, its cable
/// coiled on the hook.
fn gpu(parts: &mut Parts, at: Place) {
    let p = |x: f32, y: f32, z: f32| at.p(x, y, z);
    let b = parts.get(Surface::Painted);
    b.color(gray(0.88));
    b.chamfer_box(p(0.0, 0.95, 0.0), Vec3::new(0.6, 0.55, 1.0), 0.05, at.r);
    b.color(gray(0.3));
    b.chamfer_box(p(0.0, 0.36, 0.0), Vec3::new(0.55, 0.06, 1.05), 0.02, at.r);
    for k in 0..6 {
        let z = -0.6 + k as f32 * 0.24;
        b.chamfer_box(p(0.605, 1.1, z), Vec3::new(0.005, 0.3, 0.05), 0.0, at.r);
    }
    b.color(hex(0xe0a812));
    b.chamfer_box(p(0.0, 1.52, 0.0), Vec3::new(0.62, 0.03, 1.02), 0.01, at.r);
    let c = parts.get(Surface::Rubber);
    c.color(Vec4::ONE);
    c.torus(p(-0.62, 1.0, 0.3), at.v(Vec3::X), 0.25, 0.03, 16, 5);
    c.torus(p(-0.64, 1.0, 0.3), at.v(Vec3::X), 0.22, 0.03, 16, 5);
    let g = parts.get(Surface::Galvanized);
    g.color(gray(0.6));
    g.cylinder(p(0.0, 0.32, -1.05), p(0.0, 0.3, -1.7), 0.03, 0.03, 6, false);
    wheels(parts, at, &[-0.7, 0.7], 0.22, 0.5, 0.14);
}
