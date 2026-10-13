//! The spaceport around `demo::spawn_spaceport`'s landing pad: a concrete pad of jointed
//! panels with a steel flame grate over a plenum in its middle, rocket soot, a painted
//! landing target with hazard bands and the pad code, flush amber perimeter lights (the
//! warm point lights the pad has always had), a drained concrete ring, the parked ship's
//! apron, two hangars on their own apron, a propellant tank farm in a bund, a service
//! tower with a retracted crew access arm, a sound-suppression water tower, precast blast
//! walls, floodlight masts, service roads, camera stands and warning boards.
//!
//! Zone-local frame: world axes, origin on the ground under the pad's center. The 600 m
//! launch tower the showcase places at (70, 60) keeps its 16 m circle clear.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::time::Instant;

use engine_core::math::{DVec3, Quat, Vec2, Vec3, Vec4};
use engine_core::prelude::*;

use super::buildings::{self, Hangar, Place, dashed, flat, line, rect, text};
use super::kit::{Builder, gray, hex};
use super::noise::{mix, rand1, smoothstep, world_fbm};
use super::{BuildStats, Parts, Scene, Surface, tex};

/// Thickness of the pad collider (its top is the pad's surface).
pub const PAD_THICK: f32 = 0.4;
/// The flame grate's radius.
const GRATE_R: f32 = 4.6;
/// The parked ship's apron: center (x, z), size, top above the ground (as in the demo).
const APRON: (Vec2, Vec2, f32) = (Vec2::new(58.0, -45.0), Vec2::new(34.0, 40.0), 0.12);
/// Hangar apron (x range, z range) and its height.
const HANGAR_APRON: ((f32, f32), (f32, f32)) = ((-72.0, -50.0), (-55.0, 40.0));
const HANGAR_Y: f32 = 0.07;
/// Blast wall arcs: radius, from and to angle (degrees, 0 = +x, 90 = +z).
const WALLS: [(f32, f32, f32); 3] = [
    (46.0, 95.0, 171.0),
    (46.0, 189.0, 212.0),
    (42.0, 278.0, 322.0),
];
/// Where the launch tower stands (kept clear).
const LAUNCH_TOWER: Vec2 = Vec2::new(70.0, 60.0);
const SERVICE_TOWER: Vec3 = Vec3::new(40.0, 0.0, 8.0);
/// The sound-suppression water tower south of the pad.
const WATER_TOWER: Vec3 = Vec3::new(-18.0, 0.0, -82.0);

/// Queue the spaceport scenery around the pad whose top center is `pad`.
pub fn spawn(commands: &mut Commands, pad: DVec3, pad_radius: f32) {
    commands.queue(move |world: &mut World| {
        build(world, pad, pad_radius);
    });
}

/// Build the spaceport scenery now. `None` without a renderer.
pub fn build(world: &mut World, pad: DVec3, pad_radius: f32) -> Option<BuildStats> {
    let started = Instant::now();
    let origin = pad - DVec3::Y * PAD_THICK as f64;
    let mut scene = Scene::new(world, origin, "spaceport")?;
    let r = pad_radius;
    let mut parts = Parts::default();
    pad_surface(r, &mut parts);
    flame_grate(&mut parts);
    pad_markings(r, &mut parts);
    pad_lights(r, &mut parts);
    ship_apron(&mut parts);
    hangars(&mut parts);
    blast_walls(&mut parts);
    tank_farm(&mut parts);
    service_tower(&mut parts);
    water_tower(&mut parts);
    // Floodlight masts round the pad; the south-east one stands off the diagonal, out of
    // the overview of the pad from the south-east.
    for deg in [20.0f32, 135.0, 225.0, 315.0] {
        let a = deg.to_radians();
        let base = Vec3::new(a.cos() * 57.0, 0.0, a.sin() * 57.0);
        buildings::floodlight_mast(&mut parts, base, 30.0, Vec3::new(0.0, PAD_THICK, 0.0), 6);
    }
    roads(r, &mut parts);
    camera_stands(&mut parts);
    parts.spawn(&mut scene, "spaceport");
    // The pad's warm perimeter lights.
    for i in 0..12 {
        let a = i as f64 / 12.0 * std::f64::consts::TAU;
        scene.light(
            DVec3::new(a.cos() * 28.0, 0.6, a.sin() * 28.0),
            engine_render::PointLight {
                color: [1.0, 0.5, 0.2],
                intensity_lm: 1500.0,
                range: 12.0,
                radius: 0.1,
            },
        );
    }
    let _ = LAUNCH_TOWER;
    Some(scene.finish("spaceport", started))
}

/// A flat ring between radii `r0` and `r1` at height `y` from angle `a0` to `a1`, UVs = zone
/// (x, z).
fn ring(b: &mut Builder, r0: f32, r1: f32, y: f32, a0: f32, a1: f32, seg: u32) {
    let base = b.mesh.vertices.len() as u32;
    for i in 0..=seg {
        let a = mix(a0, a1, i as f32 / seg as f32);
        let (s, c) = a.sin_cos();
        for r in [r0, r1] {
            let p = Vec3::new(c * r, y, s * r);
            b.vertex(p, Vec3::Y, Vec2::new(p.x, p.z));
        }
    }
    for i in 0..seg {
        let k = base + i * 2;
        b.quad_idx(k, k + 1, k + 3, k + 2);
    }
}

// ---------------------------------------------------------------- the pad

fn pad_surface(r: f32, parts: &mut Parts) {
    let top = PAD_THICK;
    // Top: jointed concrete panels in rings from the grate out to the edge; heat-darkened
    // toward the middle.
    let s = parts.get(Surface::Slabs);
    let rings = 22u32;
    let seg = 144u32;
    let base = s.mesh.vertices.len() as u32;
    for j in 0..=rings {
        let rr = mix(GRATE_R + 0.3, r - 0.06, j as f32 / rings as f32);
        for i in 0..=seg {
            let a = i as f32 / seg as f32 * TAU;
            let p = Vec3::new(a.cos() * rr, top, a.sin() * rr);
            let heat = smoothstep(16.0, 5.0, rr);
            let k = (1.0 - 0.35 * heat) * (1.0 + 0.07 * world_fbm(p.x, p.z, 9.0, 3, 71));
            let c = Vec4::new(k, k * 0.985, k * 0.97, 1.0);
            s.vertex_c(p, Vec3::Y, Vec2::new(p.x, p.z), c);
        }
    }
    let row = seg + 1;
    for j in 0..rings {
        for i in 0..seg {
            let k = base + j * row + i;
            s.quad_idx(k, k + 1, k + row + 1, k + row);
        }
    }
    // Edge: a chamfered arris and the vertical face down to the ground ring.
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.92));
    let base = c.mesh.vertices.len() as u32;
    let prof = [
        (r - 0.06, top, 0.7f32),
        (r, top - 0.06, 0.7),
        (r, top - 0.06, 0.0),
        (r, 0.0, 0.0),
    ];
    for i in 0..=seg {
        let a = i as f32 / seg as f32 * TAU;
        let (sn, cs) = a.sin_cos();
        let out = Vec3::new(cs, 0.0, sn);
        for (k, (rr, y, up)) in prof.iter().enumerate() {
            let n = (out + Vec3::Y * *up).normalize();
            let shade = if k == 3 { 0.7 } else { 1.0 };
            c.vertex_c(
                Vec3::new(cs * rr, *y, sn * rr),
                n,
                Vec2::new(a * r, *y),
                gray(0.92 * shade),
            );
        }
    }
    for i in 0..seg {
        let k = base + i * 4;
        c.quad_idx(k, k + 4, k + 5, k + 1);
        c.quad_idx(k + 2, k + 6, k + 7, k + 3);
    }
    // Ground ring round the pad with a slot drain at its foot.
    c.color(gray(0.88));
    ring(c, r + 0.6, r + 7.5, 0.03, 0.0, TAU, 128);
    let d = parts.get(Surface::Concrete);
    d.color(gray(0.2));
    ring(d, r, r + 0.6, 0.006, 0.0, TAU, 128);
    let g = parts.get(Surface::Grate);
    g.color(gray(0.8));
    ring(g, r + 0.05, r + 0.55, 0.028, 0.0, TAU, 128);
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.85));
    let base = c.mesh.vertices.len() as u32;
    for i in 0..=128u32 {
        let a = i as f32 / 128.0 * TAU;
        let (sn, cs) = a.sin_cos();
        let n = Vec3::new(cs, 6.0, sn).normalize();
        c.vertex(
            Vec3::new(cs * (r + 7.5), 0.03, sn * (r + 7.5)),
            n,
            Vec2::new(a * r, 0.0),
        );
        c.vertex(
            Vec3::new(cs * (r + 7.8), 0.0, sn * (r + 7.8)),
            n,
            Vec2::new(a * r, 0.3),
        );
    }
    for i in 0..128u32 {
        let k = base + i * 2;
        c.quad_idx(k, k + 2, k + 3, k + 1);
    }
}

fn flame_grate(parts: &mut Parts) {
    let top = PAD_THICK;
    // The grate itself, flush with the pad.
    let g = parts.get(Surface::Grate);
    g.color(Vec4::ONE);
    ring(g, 0.0, GRATE_R, top, 0.0, TAU, 48);
    // Steel frame ring and bolts.
    let s = parts.get(Surface::Galvanized);
    s.color(gray(0.45));
    ring(s, GRATE_R - 0.02, GRATE_R + 0.3, top + 0.002, 0.0, TAU, 64);
    for k in 0..24 {
        let a = k as f32 / 24.0 * TAU;
        let p = Vec3::new(
            a.cos() * (GRATE_R + 0.15),
            top + 0.002,
            a.sin() * (GRATE_R + 0.15),
        );
        s.cylinder(p, p + Vec3::Y * 0.025, 0.025, 0.025, 6, true);
    }
    // Soot over the middle of the pad (and the grate).
    let soot = parts.get(Surface::Soot);
    soot.color(Vec4::new(1.0, 1.0, 1.0, 0.95));
    let h = 21.0f32;
    let y = top + 0.008;
    soot.quad_n(
        [
            Vec3::new(-h, y, -h),
            Vec3::new(h, y, -h),
            Vec3::new(h, y, h),
            Vec3::new(-h, y, h),
        ],
        [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
        Vec3::Y,
    );
}

fn pad_markings(r: f32, parts: &mut Parts) {
    let y = PAD_THICK + 0.003;
    let p = parts.get(Surface::PaintSlabs);
    let white = Vec4::new(0.95, 0.95, 0.93, 1.0);
    let yellow = hex(0xf0b414);
    // Rings: white near the edge, the yellow landing circle, a white inner ring.
    p.color(white);
    ring(p, r - 3.0, r - 2.45, y, 0.0, TAU, 160);
    ring(p, 12.0, 12.5, y, 0.0, TAU, 96);
    p.color(yellow);
    ring(p, 21.6, 22.4, y, 0.0, TAU, 144);
    // Crosshair bars.
    p.color(white);
    for k in 0..4 {
        let a = k as f32 * FRAC_PI_2;
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        line(p, d * 5.6, d * 11.4, 0.9, y);
        line(p, d * 13.1, d * 20.8, 0.5, y);
    }
    // Hazard band: slanted yellow and black stripes outside the lights.
    let n = 120u32;
    let (r0, r1) = (r - 1.35, r - 0.35);
    let da = TAU / n as f32;
    for i in 0..n {
        let a0 = i as f32 * da;
        let a1 = a0 + da;
        let slant = da * 0.6;
        p.color(if i % 2 == 0 { yellow } else { gray(0.03) });
        let q = [
            Vec3::new(a0.cos() * r0, y, a0.sin() * r0),
            Vec3::new(a1.cos() * r0, y, a1.sin() * r0),
            Vec3::new((a1 + slant).cos() * r1, y, (a1 + slant).sin() * r1),
            Vec3::new((a0 + slant).cos() * r1, y, (a0 + slant).sin() * r1),
        ];
        flat(p, q, true);
    }
    // Pad code, readable from the south.
    let g = parts.get(Surface::Glyphs);
    g.color(white);
    text(
        g,
        Vec3::new(0.0, y + 0.001, 16.8),
        Vec3::X,
        Vec3::NEG_Z,
        Vec3::Y,
        "LZ-1",
        3.2,
        1.0,
    );
    g.color(yellow);
    text(
        g,
        Vec3::new(0.0, y + 0.001, -16.8),
        Vec3::NEG_X,
        Vec3::Z,
        Vec3::Y,
        "LZ-1",
        3.2,
        1.0,
    );
}

fn pad_lights(r: f32, parts: &mut Parts) {
    let y = PAD_THICK;
    for i in 0..12 {
        let a = i as f32 / 12.0 * TAU;
        let p = Vec3::new(a.cos() * 28.0, y, a.sin() * 28.0);
        let s = parts.get(Surface::Galvanized);
        s.color(gray(0.6));
        s.cylinder(p, p + Vec3::Y * 0.02, 0.3, 0.27, 16, true);
        let l = parts.get(Surface::LensAmber);
        l.color(Vec4::ONE);
        l.cylinder(p + Vec3::Y * 0.02, p + Vec3::Y * 0.06, 0.2, 0.16, 16, true);
    }
    let _ = r;
}

// ---------------------------------------------------------------- aprons and roads

fn slab_grid(b: &mut Builder, x0: f32, x1: f32, z0: f32, z1: f32, y: f32, seed: u32) {
    let nx = ((x1 - x0) / 2.5).ceil() as u32;
    let nz = ((z1 - z0) / 2.5).ceil() as u32;
    let base = b.mesh.vertices.len() as u32;
    for j in 0..=nz {
        let z = mix(z0, z1, j as f32 / nz as f32);
        for i in 0..=nx {
            let x = mix(x0, x1, i as f32 / nx as f32);
            let k = 1.0 + 0.08 * world_fbm(x, z, 12.0, 3, seed);
            b.vertex_c(
                Vec3::new(x, y, z),
                Vec3::Y,
                Vec2::new(x, z),
                Vec4::new(k, k, k * 0.99, 1.0),
            );
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

/// A sloped concrete kerb round a rectangle whose top is at `y`.
fn kerb(parts: &mut Parts, x0: f32, x1: f32, z0: f32, z1: f32, y: f32) {
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.9));
    let pts = [
        Vec3::new(x0, 0.0, z0),
        Vec3::new(x1, 0.0, z0),
        Vec3::new(x1, 0.0, z1),
        Vec3::new(x0, 0.0, z1),
    ];
    for k in 0..4 {
        let (a, b) = (pts[k], pts[(k + 1) % 4]);
        let d = (b - a).normalize();
        let out = Vec3::new(d.z, 0.0, -d.x);
        let w = 0.35 + y;
        let q = [a + Vec3::Y * y, b + Vec3::Y * y, b + out * w, a + out * w];
        let n = (out * y + Vec3::Y * w).normalize();
        c.quad_n(q, q.map(|v| Vec2::new(v.x + v.z, v.y)), n);
    }
}

fn ship_apron(parts: &mut Parts) {
    let (c, size, y) = APRON;
    let (x0, x1) = (c.x - size.x * 0.5, c.x + size.x * 0.5);
    let (z0, z1) = (c.y - size.y * 0.5, c.y + size.y * 0.5);
    slab_grid(parts.get(Surface::Slabs), x0, x1, z0, z1, y, 81);
    kerb(parts, x0, x1, z0, z1, y);
    let p = parts.get(Surface::PaintSlabs);
    let yellow = hex(0xf0b414);
    let py = y + 0.003;
    // Stand box and centerline along the parked ship's heading.
    let fwd = Vec3::new(0.3, 0.0, -1.0).normalize();
    let side = Vec3::new(-fwd.z, 0.0, fwd.x);
    let ctr = Vec3::new(c.x, 0.0, c.y);
    p.color(yellow);
    line(p, ctr - fwd * 17.0, ctr + fwd * 15.0, 0.2, py);
    let (hl, hw) = (13.5f32, 9.5f32);
    let corners = [
        ctr - fwd * hl - side * hw,
        ctr + fwd * hl - side * hw,
        ctr + fwd * hl + side * hw,
        ctr - fwd * hl + side * hw,
    ];
    for k in 0..4 {
        dashed(p, corners[k], corners[(k + 1) % 4], 0.2, py, 1.5, 1.0);
    }
    line(
        p,
        ctr + fwd * 15.0 - side * 2.0,
        ctr + fwd * 15.0 + side * 2.0,
        0.4,
        py,
    );
    // Hazard chevrons on the north edge.
    for k in 0..8 {
        let x = x0 + 2.5 + k as f32 * 4.0;
        p.color(yellow);
        line(
            p,
            Vec3::new(x, 0.0, z0 + 1.0),
            Vec3::new(x + 1.4, 0.0, z0 + 2.4),
            0.5,
            py,
        );
        line(
            p,
            Vec3::new(x + 2.8, 0.0, z0 + 1.0),
            Vec3::new(x + 1.4, 0.0, z0 + 2.4),
            0.5,
            py,
        );
    }
    let g = parts.get(Surface::Glyphs);
    g.color(yellow);
    text(
        g,
        ctr + fwd * 17.5 + Vec3::Y * (py + 0.001),
        side,
        fwd,
        Vec3::Y,
        "SP-2",
        1.8,
        1.0,
    );
    // Tie-down and grounding points.
    let t = parts.get(Surface::Galvanized);
    t.color(Vec4::ONE);
    for (u, v) in [(-6.0f32, -4.0f32), (-6.0, 4.0), (6.0, -7.5), (6.0, 7.5)] {
        let q = ctr + fwd * u + side * v + Vec3::Y * (y + 0.01);
        t.torus(q, Vec3::Y, 0.07, 0.014, 10, 4);
    }
}

fn hangars(parts: &mut Parts) {
    let ((x0, x1), (z0, z1)) = HANGAR_APRON;
    slab_grid(parts.get(Surface::Slabs), x0, x1, z0, z1, HANGAR_Y, 83);
    kerb(parts, x0, x1, z0, z1, HANGAR_Y);
    let specs = [
        (-32.0f32, (0.0f32, 0.0f32), "ORBITAL"),
        (17.5, (0.35, 0.85), "PULSAR"),
    ];
    for (zc, open, name) in specs {
        let h = Hangar {
            w: 34.0,
            d: 28.0,
            eave: 12.0,
            rise: 3.0,
            door_h: 11.0,
            open,
            wall: hex(0xdfe1df),
            door: hex(0xc4c9cd),
            trim: hex(0x2a2f36),
            board: tex::board_index(name),
            floor: HANGAR_Y + 0.06,
        };
        buildings::hangar(parts, Place::new(Vec3::new(x0, 0.0, zc), -FRAC_PI_2), &h);
    }
    // Lead lines into the hangars.
    let p = parts.get(Surface::PaintSlabs);
    p.color(hex(0xf0b414));
    for (zc, _, _) in specs {
        line(
            p,
            Vec3::new(x1 - 1.0, 0.0, zc),
            Vec3::new(x0 + 0.5, 0.0, zc),
            0.2,
            HANGAR_Y + 0.003,
        );
    }
}

/// A precast T-wall unit 1.2 m wide, `h` tall, its face toward `face` (horizontal).
fn t_wall(c: &mut Builder, base: Vec3, face: Vec3, h: f32) {
    let rot = buildings::facing_z(face);
    c.chamfer_box(base + Vec3::Y * 0.2, Vec3::new(0.6, 0.2, 0.9), 0.03, rot);
    c.chamfer_box(
        base + Vec3::Y * (h * 0.5 + 0.2) + face * 0.25,
        Vec3::new(0.58, h * 0.5, 0.16),
        0.03,
        rot,
    );
    // Lifting loops on top.
    let top = base + Vec3::Y * (h + 0.2) + face * 0.25;
    let side = Vec3::new(-face.z, 0.0, face.x);
    for s in [-0.3f32, 0.3] {
        c.cylinder(
            top + side * s,
            top + side * s + Vec3::Y * 0.08,
            0.03,
            0.03,
            4,
            false,
        );
    }
}

fn blast_walls(parts: &mut Parts) {
    let h = 3.8f32;
    let mut boards = Vec::new();
    for (wi, (r, a0, a1)) in WALLS.iter().enumerate() {
        let (a0, a1) = (a0.to_radians(), a1.to_radians());
        let n = ((a1 - a0) * r / 1.24).floor() as i32;
        for k in 0..n {
            let a = a0 + (k as f32 + 0.5) * 1.24 / r;
            let p = Vec3::new(a.cos() * r, 0.0, a.sin() * r);
            let face = -Vec3::new(a.cos(), 0.0, a.sin());
            let shade = 0.86 + 0.14 * rand1(k + wi as i32 * 100, 3);
            let c = parts.get(Surface::Concrete);
            c.color(gray(shade));
            t_wall(c, p, face, h);
            // Scorch on the pad side: soot-darkened faces on units facing the pad.
            if k % 9 == 4 {
                boards.push((p, face));
            }
        }
    }
    // Warning boards on the outside faces.
    let idx = tex::board_index("DANGER ROCKET BLAST").unwrap_or(0);
    let s = parts.get(Surface::Signs);
    s.color(Vec4::ONE);
    for (p, face) in boards {
        let out = -face;
        let side = Vec3::new(-out.z, 0.0, out.x);
        let c = p + face * 0.08 + Vec3::Y * 1.4;
        // Reader outside: right = −side for a board facing out.
        buildings::board(s, c + side * 1.2, c - side * 1.2, 0.6, idx);
    }
}

fn tank_farm(parts: &mut Parts) {
    let (x0, x1, z0, z1) = (-56.0f32, -14.0f32, 62.0f32, 94.0f32);
    // Bund: a 1.2 m wall with a sump floor.
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.9));
    let pts = [
        Vec3::new(x0, 0.0, z0),
        Vec3::new(x1, 0.0, z0),
        Vec3::new(x1, 0.0, z1),
        Vec3::new(x0, 0.0, z1),
    ];
    for k in 0..4 {
        let (a, b) = (pts[k], pts[(k + 1) % 4]);
        let len = (b - a).length();
        let d = (b - a) / len;
        c.chamfer_box(
            (a + b) * 0.5 + Vec3::Y * 0.6,
            Vec3::new(0.15, 0.6, len * 0.5 + 0.15),
            0.02,
            buildings::facing_z(d),
        );
    }
    // The floor inside the bund: crushed stone.
    let g = parts.get(Surface::Gravel);
    g.color(gray(0.9));
    ring_rect(g, x0 + 0.15, x1 - 0.15, z0 + 0.15, z1 - 0.15, 0.03);
    // Two spheres on legs (oxidizer, fuel), two horizontal storage tanks on saddles.
    let r = 6.0f32;
    for (x, col) in [(-44.0f32, gray(0.93)), (-26.0, hex(0xd0d4d6))] {
        let ctr = Vec3::new(x, 1.6 + r, 73.0);
        let p = parts.get(Surface::Painted);
        p.color(col);
        p.sphere(ctr, r, 40, 1.0);
        // Equator band and top manway.
        p.color(gray(0.6));
        p.torus(ctr, Vec3::Y, r + 0.02, 0.08, 64, 6);
        p.cylinder(
            ctr + Vec3::Y * (r - 0.2),
            ctr + Vec3::Y * (r + 0.3),
            0.5,
            0.5,
            16,
            true,
        );
        // Legs to the equator with cross bracing.
        let legs = 8;
        let leg_top: Vec<Vec3> = (0..legs)
            .map(|k| {
                let a = k as f32 / legs as f32 * TAU;
                ctr + Vec3::new(a.cos() * r, 0.0, a.sin() * r)
            })
            .collect();
        for k in 0..legs {
            let top = leg_top[k];
            let bot = Vec3::new(top.x, 0.0, top.z);
            let p = parts.get(Surface::Painted);
            p.color(col * 0.95);
            p.cylinder(bot, top, 0.3, 0.3, 12, false);
            let cc = parts.get(Surface::Concrete);
            cc.color(gray(0.85));
            cc.chamfer_box(
                bot + Vec3::Y * 0.25,
                Vec3::new(0.6, 0.25, 0.6),
                0.03,
                Quat::IDENTITY,
            );
            let n = leg_top[(k + 1) % legs];
            let nb = Vec3::new(n.x, 0.0, n.z);
            let g = parts.get(Surface::Galvanized);
            g.color(Vec4::ONE);
            g.cylinder(
                bot + Vec3::Y * 0.6,
                n - Vec3::Y * 0.8,
                0.035,
                0.035,
                6,
                false,
            );
            g.cylinder(
                nb + Vec3::Y * 0.6,
                top - Vec3::Y * 0.8,
                0.035,
                0.035,
                6,
                false,
            );
        }
        // Ladder with cage up one leg, top platform with railing.
        let a = 0.5f32 * TAU / legs as f32;
        let out = Vec3::new(a.cos(), 0.0, a.sin());
        let g = parts.get(Surface::Galvanized);
        let lad = Vec3::new(ctr.x, 0.0, ctr.z) + out * (r + 0.6);
        let side = Vec3::new(-out.z, 0.0, out.x);
        for s in [-0.22f32, 0.22] {
            g.cylinder(
                lad + side * s,
                lad + side * s + Vec3::Y * (ctr.y + r * 0.8),
                0.025,
                0.025,
                4,
                false,
            );
        }
        let mut yy = 0.3;
        while yy < ctr.y + r * 0.8 {
            g.cylinder(
                lad - side * 0.22 + Vec3::Y * yy,
                lad + side * 0.22 + Vec3::Y * yy,
                0.015,
                0.015,
                4,
                false,
            );
            yy += 0.3;
        }
        let deck_y = ctr.y + r + 0.3;
        let mut rail = Vec::new();
        for k in 0..=12 {
            let a = k as f32 / 12.0 * TAU;
            rail.push(Vec3::new(
                ctr.x + a.cos() * 1.6,
                deck_y,
                ctr.z + a.sin() * 1.6,
            ));
        }
        g.cylinder(
            Vec3::new(ctr.x, deck_y - 0.05, ctr.z),
            Vec3::new(ctr.x, deck_y, ctr.z),
            1.7,
            1.7,
            16,
            true,
        );
        buildings::railing(parts, &rail, 1.0, 1.4);
    }
    for x in [-50.0f32, -32.0] {
        let a = Vec3::new(x, 2.3, 87.5);
        let b = Vec3::new(x + 12.0, 2.3, 87.5);
        let p = parts.get(Surface::Painted);
        p.color(gray(0.9));
        p.cylinder(a, b, 1.6, 1.6, 28, false);
        p.sphere(a, 1.6, 20, 1.0);
        p.sphere(b, 1.6, 20, 1.0);
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.85));
        for t in [0.15f32, 0.5, 0.85] {
            let q = a.lerp(b, t);
            c.chamfer_box(
                Vec3::new(q.x, 0.45, q.z),
                Vec3::new(0.35, 0.45, 1.5),
                0.03,
                Quat::IDENTITY,
            );
        }
    }
    // Pipework: manifold along the bund, then out toward the pad on sleepers.
    let g = parts.get(Surface::Painted);
    g.color(hex(0x9aa3a8));
    let pipe = [
        Vec3::new(-44.0, 1.2, 66.0),
        Vec3::new(-26.0, 1.2, 66.0),
        Vec3::new(-20.0, 1.2, 66.0),
        Vec3::new(-20.0, 1.2, 61.0),
        Vec3::new(-14.0, 0.6, 52.0),
        Vec3::new(-8.0, 0.6, 50.0),
    ];
    for w in pipe.windows(2) {
        for off in [-0.35f32, 0.35] {
            let d = (w[1] - w[0]).normalize();
            let s = Vec3::new(-d.z, 0.0, d.x) * off;
            g.cylinder(w[0] + s, w[1] + s, 0.16, 0.16, 12, false);
        }
    }
    for x in [-44.0f32, -26.0] {
        for off in [-0.35f32, 0.35] {
            g.cylinder(
                Vec3::new(x + off, 1.2, 66.0),
                Vec3::new(x + off, 1.2, 73.0 - 6.0),
                0.12,
                0.12,
                10,
                false,
            );
        }
    }
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.8));
    for w in pipe.windows(2) {
        let n = ((w[1] - w[0]).length() / 3.0).ceil() as u32;
        for k in 0..=n {
            let q = w[0].lerp(w[1], k as f32 / n as f32);
            let d = (w[1] - w[0]).normalize();
            c.chamfer_box(
                Vec3::new(q.x, (q.y - 0.16) * 0.5, q.z),
                Vec3::new(0.7, (q.y - 0.16) * 0.5, 0.2),
                0.02,
                buildings::facing_z(d),
            );
        }
    }
    // Warning boards on the bund.
    let idx = tex::board_index("DANGER ROCKET BLAST").unwrap_or(0);
    let s = parts.get(Surface::Signs);
    s.color(Vec4::ONE);
    buildings::board(
        s,
        Vec3::new(-32.5, 1.25 - 0.6, z0 - 0.16),
        Vec3::new(-37.5, 1.25 - 0.6, z0 - 0.16),
        1.25,
        idx,
    );
}

/// A flat rectangle at `y`, UVs = zone (x, z).
fn ring_rect(b: &mut Builder, x0: f32, x1: f32, z0: f32, z1: f32, y: f32) {
    flat(
        b,
        [
            Vec3::new(x0, y, z0),
            Vec3::new(x1, y, z0),
            Vec3::new(x1, y, z1),
            Vec3::new(x0, y, z1),
        ],
        true,
    );
}

fn service_tower(parts: &mut Parts) {
    let o = SERVICE_TOWER;
    let hw = 2.5f32;
    let h = 32.0f32;
    let level = 4.0f32;
    // Footing.
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.88));
    c.chamfer_box(
        o + Vec3::Y * 0.25,
        Vec3::new(hw + 1.0, 0.25, hw + 1.0),
        0.05,
        Quat::IDENTITY,
    );
    let corners = [
        o + Vec3::new(-hw, 0.5, -hw),
        o + Vec3::new(hw, 0.5, -hw),
        o + Vec3::new(hw, 0.5, hw),
        o + Vec3::new(-hw, 0.5, hw),
    ];
    let s = parts.get(Surface::Painted);
    let steel = hex(0xb8452a);
    s.color(steel);
    for p in corners {
        s.beam(p, p + Vec3::Y * (h - 0.5), 0.4, 0.4, 0.02);
    }
    // Girts and X bracing on each face, per level.
    let levels = ((h - 0.5) / level) as u32;
    for l in 0..=levels {
        let y = l as f32 * level;
        for k in 0..4 {
            let (a, b) = (corners[k], corners[(k + 1) % 4]);
            s.beam(a + Vec3::Y * y, b + Vec3::Y * y, 0.22, 0.28, 0.0);
            if l < levels {
                s.beam(a + Vec3::Y * y, b + Vec3::Y * (y + level), 0.12, 0.12, 0.0);
                s.beam(b + Vec3::Y * y, a + Vec3::Y * (y + level), 0.12, 0.12, 0.0);
            }
        }
    }
    // Decks with railings every second level.
    let mut l = 2;
    while l <= levels {
        let y = 0.5 + l as f32 * level;
        let g = parts.get(Surface::Grate);
        g.color(gray(0.75));
        ring_rect(
            g,
            o.x - hw - 0.8,
            o.x + hw + 0.8,
            o.z - hw - 0.8,
            o.z + hw + 0.8,
            y,
        );
        let rail = [
            Vec3::new(o.x - hw - 0.8, y, o.z - hw - 0.8),
            Vec3::new(o.x + hw + 0.8, y, o.z - hw - 0.8),
            Vec3::new(o.x + hw + 0.8, y, o.z + hw + 0.8),
            Vec3::new(o.x - hw - 0.8, y, o.z + hw + 0.8),
            Vec3::new(o.x - hw - 0.8, y, o.z - hw - 0.8),
        ];
        buildings::railing(parts, &rail, 1.1, 1.5);
        l += 2;
    }
    // Elevator shaft on the east face (clad) and a stair tower on the north.
    let w = parts.get(Surface::Cladding);
    w.color(gray(0.88));
    let ex = o.x + hw + 0.05;
    let e0 = Vec3::new(ex, 0.5, o.z - 1.2);
    let e1 = Vec3::new(ex, 0.5, o.z + 1.2);
    let e2 = Vec3::new(ex + 2.2, 0.5, o.z + 1.2);
    let e3 = Vec3::new(ex + 2.2, 0.5, o.z - 1.2);
    for (a, b) in [(e1, e2), (e2, e3), (e3, e0)] {
        buildings::panel(w, a, b, 0.0, h + 1.5, 0.2);
    }
    let r = parts.get(Surface::Painted);
    r.color(gray(0.3));
    flat(
        r,
        [
            e0 + Vec3::Y * (h + 1.5),
            e3 + Vec3::Y * (h + 1.5),
            e2 + Vec3::Y * (h + 1.5),
            e1 + Vec3::Y * (h + 1.5),
        ],
        true,
    );
    // Crew access arm at 20 m, swung back along the tower's south side (retracted).
    let arm_y = 0.5 + 5.0 * level;
    let pivot = o + Vec3::new(-hw, arm_y, hw);
    let dir = Vec3::Z;
    let len = 11.0f32;
    let s = parts.get(Surface::Painted);
    s.color(steel);
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    for sy in [0.0f32, 2.8] {
        for sx in [-1.0f32, 1.0] {
            s.beam(
                pivot + side * (sx * 1.1) + Vec3::Y * sy,
                pivot + side * (sx * 1.1) + Vec3::Y * sy + dir * len,
                0.16,
                0.16,
                0.0,
            );
        }
    }
    let panels = 6;
    for k in 0..=panels {
        let q = pivot + dir * (len * k as f32 / panels as f32);
        for sx in [-1.0f32, 1.0] {
            s.beam(
                q + side * (sx * 1.1),
                q + side * (sx * 1.1) + Vec3::Y * 2.8,
                0.1,
                0.1,
                0.0,
            );
        }
        s.beam(q - side * 1.1, q + side * 1.1, 0.1, 0.1, 0.0);
        if k < panels {
            let q1 = pivot + dir * (len * (k + 1) as f32 / panels as f32);
            for sx in [-1.0f32, 1.0] {
                s.beam(
                    q + side * (sx * 1.1),
                    q1 + side * (sx * 1.1) + Vec3::Y * 2.8,
                    0.07,
                    0.07,
                    0.0,
                );
            }
        }
    }
    // Enclosed walkway and the white room at its end.
    let w = parts.get(Surface::Cladding);
    w.color(gray(0.92));
    let room = pivot + dir * (len - 1.4) + Vec3::Y * 1.45;
    w.chamfer_box(room, Vec3::new(1.4, 1.4, 1.5), 0.05, Quat::IDENTITY);
    let gl = parts.get(Surface::Glass);
    gl.color(Vec4::ONE);
    for sx in [-1.0f32, 1.0] {
        let a = pivot + side * (sx * 1.0) + Vec3::Y * 0.3;
        let b = a + dir * (len - 3.0);
        gl.quad_n(
            [a, b, b + Vec3::Y * 2.2, a + Vec3::Y * 2.2],
            [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
            side * sx,
        );
    }
    let fl = parts.get(Surface::Grate);
    fl.color(gray(0.7));
    flat(
        fl,
        [
            pivot - side * 1.1 + Vec3::Y * 0.1,
            pivot + side * 1.1 + Vec3::Y * 0.1,
            pivot + side * 1.1 + Vec3::Y * 0.1 + dir * len,
            pivot - side * 1.1 + Vec3::Y * 0.1 + dir * len,
        ],
        true,
    );
    // Lightning mast and obstruction light on top.
    let g = parts.get(Surface::Galvanized);
    g.color(Vec4::ONE);
    let top = o + Vec3::Y * h;
    g.cylinder(top, top + Vec3::Y * 9.0, 0.12, 0.05, 8, false);
    let l = parts.get(Surface::LensRed);
    l.color(Vec4::ONE);
    l.sphere(top + Vec3::Y * 9.15, 0.16, 10, 1.0);
    l.sphere(top + Vec3::new(-hw, 0.4, -hw), 0.12, 10, 1.0);
    // Umbilical drum and cable tray down the tower.
    let p = parts.get(Surface::Painted);
    p.color(gray(0.2));
    p.cylinder(
        pivot + Vec3::new(0.0, 3.4, 1.0),
        pivot + Vec3::new(0.0, 3.4, 2.4),
        0.9,
        0.9,
        20,
        true,
    );
    p.beam(
        o + Vec3::new(-hw - 0.3, 0.5, 0.0),
        o + Vec3::new(-hw - 0.3, h, 0.0),
        0.12,
        0.5,
        0.0,
    );
}

/// The sound-suppression water tower: a spherical tank on a braced, battered four-leg
/// tower, a caged ladder, a railed top deck with an obstruction light, and the deluge main
/// down the middle and along the ground to the pad.
fn water_tower(parts: &mut Parts) {
    let o = WATER_TOWER;
    let r = 7.5f32;
    let bottom = 40.0f32;
    let ctr = o + Vec3::Y * (bottom + r);
    let (foot, head) = (6.5f32, 4.6f32);
    let leg = |sx: f32, sz: f32, t: f32| -> Vec3 {
        let w = mix(foot, head, t);
        o + Vec3::new(sx * w, t * (bottom + r * 0.55), sz * w)
    };
    let corners = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let steel = gray(0.82);
    for (sx, sz) in corners {
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.86));
        let f = leg(sx, sz, 0.0);
        c.chamfer_box(
            f + Vec3::Y * 0.4,
            Vec3::new(1.0, 0.4, 1.0),
            0.05,
            Quat::IDENTITY,
        );
        let p = parts.get(Surface::Painted);
        p.color(steel);
        p.cylinder(f + Vec3::Y * 0.8, leg(sx, sz, 1.0), 0.45, 0.4, 14, false);
    }
    // Tiers of struts and X bracing on each face.
    let tiers = 6;
    let p = parts.get(Surface::Painted);
    p.color(steel);
    for t in 0..=tiers {
        let a = t as f32 / tiers as f32 * 0.92;
        let b = (t + 1) as f32 / tiers as f32 * 0.92;
        for k in 0..4 {
            let (s0, s1) = (corners[k], corners[(k + 1) % 4]);
            if t > 0 {
                p.cylinder(leg(s0.0, s0.1, a), leg(s1.0, s1.1, a), 0.16, 0.16, 8, false);
            }
            if t < tiers {
                p.cylinder(leg(s0.0, s0.1, a), leg(s1.0, s1.1, b), 0.06, 0.06, 6, false);
                p.cylinder(leg(s1.0, s1.1, a), leg(s0.0, s0.1, b), 0.06, 0.06, 6, false);
            }
        }
    }
    // The tank, its equator walkway and the top deck.
    let t = parts.get(Surface::Painted);
    t.color(gray(0.94));
    t.sphere(ctr, r, 48, 1.0);
    t.color(hex(0x24384f));
    t.torus(ctr, Vec3::Y, r + 0.02, 0.35, 64, 8);
    let g = parts.get(Surface::Grate);
    g.color(gray(0.7));
    let deck_y = ctr.y + r + 0.2;
    let rdeck = 2.4f32;
    let gb = parts.get(Surface::Galvanized);
    gb.color(Vec4::ONE);
    gb.cylinder(
        Vec3::new(ctr.x, ctr.y + r - 0.6, ctr.z),
        Vec3::new(ctr.x, deck_y - 0.06, ctr.z),
        rdeck * 0.6,
        rdeck * 0.6,
        16,
        false,
    );
    let g = parts.get(Surface::Grate);
    let n = 24u32;
    let center = g.vertex(
        Vec3::new(ctr.x, deck_y, ctr.z),
        Vec3::Y,
        Vec2::new(ctr.x, ctr.z),
    );
    let first = g.mesh.vertices.len() as u32;
    for i in 0..=n {
        let a = i as f32 / n as f32 * TAU;
        let q = Vec3::new(ctr.x + a.cos() * rdeck, deck_y, ctr.z + a.sin() * rdeck);
        g.vertex(q, Vec3::Y, Vec2::new(q.x, q.z));
    }
    for i in 0..n {
        g.tri_idx(center, first + i, first + i + 1);
    }
    let rail: Vec<Vec3> = (0..=16)
        .map(|i| {
            let a = i as f32 / 16.0 * TAU;
            Vec3::new(
                ctr.x + a.cos() * (rdeck - 0.05),
                deck_y,
                ctr.z + a.sin() * (rdeck - 0.05),
            )
        })
        .collect();
    buildings::railing(parts, &rail, 1.1, 1.2);
    let l = parts.get(Surface::LensRed);
    l.color(Vec4::ONE);
    let g = parts.get(Surface::Galvanized);
    g.color(Vec4::ONE);
    g.cylinder(
        Vec3::new(ctr.x, deck_y, ctr.z),
        Vec3::new(ctr.x, deck_y + 3.0, ctr.z),
        0.06,
        0.04,
        8,
        false,
    );
    let l = parts.get(Surface::LensRed);
    l.sphere(Vec3::new(ctr.x, deck_y + 3.12, ctr.z), 0.14, 10, 1.0);
    // Caged ladder up the south-west leg, then over the tank on a curved run.
    let g = parts.get(Surface::Galvanized);
    let (lx, lz) = corners[0];
    let foot_p = leg(lx, lz, 0.0) + Vec3::Y * 0.8;
    let top_p = leg(lx, lz, 1.0);
    let out = Vec3::new(lx, 0.0, lz).normalize() * 0.7;
    let along = (top_p - foot_p).normalize();
    let side = along.cross(out).normalize() * 0.22;
    for s in [-1.0f32, 1.0] {
        g.cylinder(
            foot_p + out + side * s,
            top_p + out + side * s,
            0.025,
            0.025,
            4,
            false,
        );
    }
    let len = (top_p - foot_p).length();
    let mut d = 0.3;
    while d < len {
        let q = foot_p + along * d + out;
        g.cylinder(q - side, q + side, 0.015, 0.015, 4, false);
        d += 0.3;
    }
    let mut d = 2.5;
    while d < len {
        let q = foot_p + along * d + out * 1.4;
        g.torus(q, along, 0.4, 0.015, 10, 4);
        d += 1.5;
    }
    // Deluge main: down the middle, then along the ground on sleepers to the pad.
    let p = parts.get(Surface::Painted);
    p.color(hex(0x3a6a3a));
    let riser_top = Vec3::new(ctr.x, ctr.y - r + 0.3, ctr.z);
    let elbow = Vec3::new(o.x, 1.1, o.z);
    p.cylinder(riser_top, elbow, 0.55, 0.55, 18, false);
    p.sphere(elbow, 0.55, 18, 1.0);
    let end = Vec3::new(-3.0, 1.1, -38.6);
    p.cylinder(elbow, end, 0.55, 0.55, 18, false);
    for k in 0..4 {
        let q = elbow.lerp(end, k as f32 / 4.0 + 0.1);
        p.color(gray(0.3));
        p.torus(q, (end - elbow).normalize(), 0.56, 0.06, 18, 4);
        p.color(hex(0x3a6a3a));
    }
    let c = parts.get(Surface::Concrete);
    c.color(gray(0.82));
    let run = (end - elbow).length();
    let dir = (end - elbow) / run;
    let n = (run / 6.0).ceil() as u32;
    for k in 0..=n {
        let q = elbow.lerp(end, k as f32 / n as f32);
        c.chamfer_box(
            Vec3::new(q.x, 0.27, q.z),
            Vec3::new(1.0, 0.27, 0.3),
            0.03,
            buildings::facing_z(dir),
        );
    }
}

fn roads(r: f32, parts: &mut Parts) {
    let y = 0.025;
    let roads: [&[Vec3]; 3] = [
        // West: pad ring to the hangar apron, through the gap in the blast walls.
        &[
            Vec3::new(-(r + 7.0), 0.0, 0.0),
            Vec3::new(-HANGAR_APRON.0.1.abs() - 0.5, 0.0, 0.0),
        ],
        // North-east: pad ring to the ship apron.
        &[
            Vec3::new((r + 7.0) * 0.866, 0.0, -(r + 7.0) * 0.5),
            Vec3::new(APRON.0.x - APRON.1.x * 0.5 + 0.5, 0.0, -30.0),
        ],
        // Hangar apron to the tank farm.
        &[
            Vec3::new(-60.0, 0.0, HANGAR_APRON.1.1 - 0.5),
            Vec3::new(-60.0, 0.0, 50.0),
            Vec3::new(-50.0, 0.0, 58.0),
            Vec3::new(-36.0, 0.0, 61.5),
        ],
    ];
    for pts in roads {
        for w in pts.windows(2) {
            let a = parts.get(Surface::Asphalt);
            a.color(gray(1.05));
            line(a, w[0], w[1], 7.0, y);
            let p = parts.get(Surface::PaintAsphalt);
            p.color(Vec4::new(0.95, 0.95, 0.93, 1.0));
            let d = (w[1] - w[0]).normalize();
            let s = Vec3::new(-d.z, 0.0, d.x);
            for e in [-3.2f32, 3.2] {
                line(p, w[0] + s * e, w[1] + s * e, 0.12, y + 0.003);
            }
            p.color(hex(0xf0b414));
            dashed(p, w[0], w[1], 0.12, y + 0.003, 3.0, 6.0);
        }
        // Round off the joints.
        for q in pts.iter().skip(1).take(pts.len().saturating_sub(2)) {
            let a = parts.get(Surface::Asphalt);
            a.color(gray(1.05));
            disc(a, *q, 3.5, y - 0.001, 24);
        }
    }
    let _ = rect;
}

/// A flat disc centered on `c` (x, z) at height `y`, UVs = zone (x, z).
fn disc(b: &mut Builder, c: Vec3, r: f32, y: f32, seg: u32) {
    let center = b.vertex(Vec3::new(c.x, y, c.z), Vec3::Y, Vec2::new(c.x, c.z));
    let first = b.mesh.vertices.len() as u32;
    for i in 0..=seg {
        let a = i as f32 / seg as f32 * TAU;
        let p = Vec3::new(c.x + a.cos() * r, y, c.z + a.sin() * r);
        b.vertex(p, Vec3::Y, Vec2::new(p.x, p.z));
    }
    for i in 0..seg {
        b.tri_idx(center, first + i, first + i + 1);
    }
}

fn camera_stands(parts: &mut Parts) {
    for (k, deg) in [60.0f32, 160.0, 250.0, 335.0].iter().enumerate() {
        let a = deg.to_radians();
        let base = Vec3::new(a.cos() * 37.0, 0.03, a.sin() * 37.0);
        let to_pad = -Vec3::new(a.cos(), 0.0, a.sin());
        let g = parts.get(Surface::Galvanized);
        g.color(Vec4::ONE);
        let head = base + Vec3::Y * 1.6;
        for j in 0..3 {
            let b = j as f32 / 3.0 * TAU + a;
            let foot = base + Vec3::new(b.cos() * 0.6, 0.0, b.sin() * 0.6);
            g.cylinder(foot, head, 0.025, 0.02, 6, false);
        }
        let p = parts.get(Surface::Painted);
        p.color(if k % 2 == 0 { gray(0.85) } else { gray(0.15) });
        let rot = buildings::facing_z(to_pad) * Quat::from_rotation_x(0.35);
        p.chamfer_box(head + Vec3::Y * 0.2, Vec3::new(0.18, 0.18, 0.35), 0.03, rot);
        // Sun hood.
        p.chamfer_box(
            head + Vec3::Y * 0.4 + rot * Vec3::new(0.0, 0.0, 0.25),
            Vec3::new(0.2, 0.01, 0.2),
            0.0,
            rot,
        );
        let l = parts.get(Surface::LensOff);
        l.color(Vec4::ONE);
        let f = head + Vec3::Y * 0.2 + rot * Vec3::new(0.0, 0.0, 0.36);
        l.cylinder(
            f - rot * Vec3::Z * 0.02,
            f + rot * Vec3::Z * 0.02,
            0.09,
            0.09,
            12,
            true,
        );
        // Ballast blocks.
        let c = parts.get(Surface::Concrete);
        c.color(gray(0.8));
        for j in 0..3 {
            let b = j as f32 / 3.0 * TAU + a;
            let foot = base + Vec3::new(b.cos() * 0.6, 0.08, b.sin() * 0.6);
            c.chamfer_box(foot, Vec3::new(0.15, 0.08, 0.15), 0.02, Quat::IDENTITY);
        }
    }
    let _ = PI;
}
