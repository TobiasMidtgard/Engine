//! The built stretches: a timber boardwalk, a galvanized bar grating over a drainage pit,
//! the cast-concrete underpass (with its echo and lamps), and the trail markers.

use engine_core::math::{DVec3, Mat3, Quat, Vec2, Vec3, Vec4};
use engine_core::prelude::*;
use engine_render::{Mesh, PointLight};

use super::procgen::{Rand, append, beveled_box, fbm_world, hex, smooth, tube, unit, vert};
use super::ribbon::{self, PATH_Y, Ribbon, Surf, columns, frame, point};
use super::{Batch, Mat, Out, Part, sky_volume};
use crate::ReverbZone;
use crate::demo::{STRETCHES, TRAIL_WIDTH, stretch_start, trail_point};

fn range(name: &str) -> (f64, f64) {
    let s0 = stretch_start(name).unwrap_or(0.0);
    let len = STRETCHES
        .iter()
        .find(|s| s.0 == name)
        .map(|s| s.1)
        .unwrap_or(9.0);
    (s0, s0 + len)
}

/// Rotation whose local X points right of the trail at `s`, Y up and Z backward.
fn trail_rot(s: f64) -> Quat {
    let (_, d, r) = frame(s);
    Quat::from_mat3(&Mat3::from_cols(r, Vec3::Y, -d))
}

/// A plain box with UVs in meters (for thin steel parts).
fn block(size: Vec3) -> Mesh {
    let mut m = engine_render::mesh::cuboid(size);
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal).abs();
        v.uv = if n.y > 0.5 {
            [p.x, p.z]
        } else if n.x > 0.5 {
            [p.z, p.y]
        } else {
            [p.x, p.y]
        };
    }
    m
}

fn pyramid(w: f32, h: f32, color: Vec4) -> Mesh {
    let mut m = Mesh::default();
    let top = Vec3::new(0.0, h, 0.0);
    let c = [
        Vec3::new(-w, 0.0, -w),
        Vec3::new(w, 0.0, -w),
        Vec3::new(w, 0.0, w),
        Vec3::new(-w, 0.0, w),
    ];
    for k in 0..4 {
        let (a, b) = (c[k], c[(k + 1) % 4]);
        let n = (b - a).cross(top - a).normalize();
        let n = if n.dot(a + b) < 0.0 { -n } else { n };
        let base = m.vertices.len() as u32;
        for p in [a, b, top] {
            m.vertices
                .push(vert(p, n, Vec2::new(p.x + p.z, p.y), color));
        }
        let g = (b - a).cross(top - a);
        if g.dot(n) >= 0.0 {
            m.indices.extend_from_slice(&[base, base + 1, base + 2]);
        } else {
            m.indices.extend_from_slice(&[base, base + 2, base + 1]);
        }
    }
    m
}

/// Weathered timber colour: darker and mossy low down, greyer on top.
fn timber_color(p: Vec3, ground_y: f32, tint: Vec3) -> Vec4 {
    let low = 1.0 - smooth(ground_y, ground_y + 0.22, p.y);
    let c = tint * (1.0 - 0.4 * low) * Vec3::new(1.0 - 0.15 * low, 1.0, 1.0 - 0.3 * low);
    c.extend(1.0)
}

// ----------------------------------------------------------------------- boardwalk

/// A low timber boardwalk: beveled planks with gaps on four joists over damp soil, nail
/// heads, overhanging plank ends, posts and handrails, weathered and mossy at the edges.
pub(super) fn boardwalk() -> Out {
    let (s0, s1) = range("wood");
    let mut out = Out::default();
    let mut b = Batch::default();
    let mut rng = Rand::new(0xb0a2d);

    // Damp soil under the boards.
    let soil = |_: f64, x: f32, w: Vec2| {
        Surf::new(
            0.006 + fbm_world(w.x, w.y, 0.4, 3, 0x81) * 0.004 + 0.03 * smooth(1.2, 1.45, x.abs()),
            1.0,
            Vec3::splat(0.75 + 0.15 * fbm_world(w.x, w.y, 1.0, 2, 0x82)),
        )
    };
    out.parts.extend(ribbon::ribbon(&Ribbon {
        mat: Mat::Soil,
        s0: s0 - 0.02,
        s1: s1 + 0.02,
        fade_in: 0.0,
        fade_out: 0.0,
        ds: 0.1,
        columns: columns(1.45, 0.1, 2.0, 0.1, 2.0, 0.1),
        surface: &soil,
        shadows: false,
    }));

    // Joists along the trail, in pieces of up to 3 m.
    let joists = [-1.18f32, -0.4, 0.4, 1.18];
    let mut s = s0;
    while s < s1 - 1e-3 {
        let e = (s + 3.0).min(s1);
        let len = (e - s) as f32;
        let mid = (s + e) * 0.5;
        for &x in &joists {
            let mesh = beveled_box(
                Vec3::new(0.07, 0.034, len),
                0.005,
                0.5,
                Vec2::new(rng.f(), 0.2),
                |p| timber_color(p + Vec3::Y * 0.017, 0.0, Vec3::splat(0.55)),
            );
            let mut part = Part::own(mesh, Mat::Timber, point(mid, x, 0.017));
            part.rot = trail_rot(mid);
            out.parts.push(part);
        }
        s = e;
    }

    // Planks across, with gaps, each with its own grain, tone and wear.
    let pitch = 0.152;
    let mut s = s0 + 0.012 + pitch * 0.5;
    let tones = [
        Vec3::new(1.0, 1.0, 1.0),
        Vec3::new(0.9, 0.93, 0.97),
        Vec3::new(1.06, 0.98, 0.88),
        Vec3::new(0.82, 0.82, 0.82),
        Vec3::new(0.97, 0.94, 0.9),
    ];
    while s < s1 - pitch * 0.4 {
        let tone = tones[rng.below(tones.len())] * rng.range(0.9, 1.08);
        let len = 2.6 + rng.normal(0.01);
        let off = Vec2::new(rng.range(0.0, 2.0), rng.range(0.0, 0.5));
        let moss_seed = rng.seed();
        let mesh = beveled_box(Vec3::new(len, 0.028, 0.14), 0.0045, 0.2, off, |p| {
            let ax = p.x.abs();
            let end = smooth(0.95, 1.3, ax);
            let worn = 1.0 - smooth(0.2, 0.7, ax);
            let moss = end
                * smooth(
                    0.2,
                    0.6,
                    unit(super::procgen::hash((p.x * 8.0) as i32, 0, moss_seed)),
                );
            let under = 1.0 - smooth(-0.014, 0.0, p.y);
            let c = tone * (1.0 + 0.1 * worn) * (1.0 - 0.3 * end) * (1.0 - 0.45 * under);
            let c = c.lerp(Vec3::new(0.32, 0.4, 0.18), moss * 0.55);
            c.extend(1.0)
        });
        let rot = trail_rot(s)
            * Quat::from_rotation_y(rng.normal(0.004))
            * Quat::from_rotation_z(rng.normal(0.003));
        let at = point(s, rng.normal(0.01), PATH_Y - 0.014 + rng.normal(0.0008));
        append(b.at(s, Mat::Planks), &mesh, rot, Vec3::ONE, at, Vec4::ONE);
        // Two nails into every joist.
        for &x in &joists {
            for dz in [-0.042f32, 0.042] {
                let p = point(s - dz as f64, x + rng.normal(0.004), PATH_Y + 0.0002);
                nail(b.at(s, Mat::Steel), p, &mut rng);
            }
        }
        s += pitch + rng.normal(0.002) as f64;
    }
    // End beams closing the boardwalk.
    for ss in [s0 + 0.035, s1 - 0.035] {
        let mesh = beveled_box(
            Vec3::new(2.7, 0.06, 0.07),
            0.006,
            0.4,
            Vec2::new(rng.f(), 0.0),
            |p| timber_color(p + Vec3::Y * 0.03, 0.0, Vec3::splat(0.75)),
        );
        let mut part = Part::own(mesh, Mat::Timber, point(ss, 0.0, 0.03));
        part.rot = trail_rot(ss);
        out.parts.push(part);
    }

    // Posts and handrails.
    railings_timber(&mut out, &mut rng, s0, s1);
    out.parts.extend(b.finish());
    out
}

fn nail(m: &mut Mesh, at: Vec3, rng: &mut Rand) {
    let rust = rng.range(0.0, 1.0);
    let c = Vec3::new(0.55, 0.55, 0.55).lerp(Vec3::new(0.5, 0.3, 0.18), rust * 0.7);
    let head = tube(
        &[at - Vec3::Y * 0.001, at + Vec3::Y * 0.0012],
        7,
        true,
        |_, _| 0.0042,
        |_| c.extend(1.0),
    );
    append(m, &head, Quat::IDENTITY, Vec3::ONE, Vec3::ZERO, Vec4::ONE);
}

fn railings_timber(out: &mut Out, rng: &mut Rand, s0: f64, s1: f64) {
    let x = 1.37;
    let post_h = 1.08;
    let n = ((s1 - s0) / 1.8).round().max(1.0) as usize;
    out.shared.push(("bw-post".into(), {
        let mut m = beveled_box(
            Vec3::new(0.095, post_h + 0.1, 0.095),
            0.01,
            0.3,
            Vec2::new(0.3, 0.1),
            |p| timber_color(p + Vec3::Y * (post_h + 0.1) * 0.5, 0.1, Vec3::splat(0.8)),
        );
        let cap = pyramid(0.052, 0.04, Vec4::new(0.6, 0.6, 0.6, 1.0));
        append(
            &mut m,
            &cap,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::Y * ((post_h + 0.1) * 0.5),
            Vec4::ONE,
        );
        m
    }));
    for side in [-1.0f32, 1.0] {
        for k in 0..=n {
            let s = s0 + 0.12 + (s1 - s0 - 0.24) * k as f64 / n as f64;
            let at = point(s, side * x, (post_h - 0.1) * 0.5);
            out.parts.push(Part::shared(
                "bw-post",
                Mat::Timber,
                at,
                trail_rot(s) * Quat::from_rotation_y(rng.normal(0.01)),
                Vec3::ONE,
            ));
        }
        // Top rail (a flat cap rail) and a mid rail on the inner faces, in pieces.
        let mut s = s0 + 0.06;
        let end = s1 - 0.06;
        while s < end - 1e-3 {
            let e = (s + 4.5).min(end);
            let len = (e - s) as f32;
            let mid = (s + e) * 0.5;
            let top = beveled_box(
                Vec3::new(0.13, 0.042, len),
                0.012,
                0.4,
                Vec2::new(rng.f() * 2.0, 0.2),
                |p| {
                    let c = Vec3::splat(0.92) * (1.0 + 0.08 * smooth(0.0, 0.02, p.y));
                    c.extend(1.0)
                },
            );
            let mut part = Part::own(top, Mat::Timber, point(mid, side * x, post_h + 0.021));
            part.rot = trail_rot(mid);
            out.parts.push(part);
            let midrail = beveled_box(
                Vec3::new(0.04, 0.09, len),
                0.008,
                0.4,
                Vec2::new(rng.f() * 2.0, 0.3),
                |_| Vec4::new(0.85, 0.85, 0.85, 1.0),
            );
            let mut part = Part::own(midrail, Mat::Timber, point(mid, side * (x - 0.068), 0.52));
            part.rot = trail_rot(mid);
            out.parts.push(part);
            s = e;
        }
    }
}

// ------------------------------------------------------------------------- grating

/// A walkway of galvanized bar grating panels (bearing bars across, twisted cross rods)
/// in an angle-iron frame on concrete curbs, over a dark pit with stones and standing
/// water, with tubular steel railings.
pub(super) fn grating() -> Out {
    let (s0, s1) = range("metal");
    let mut out = Out::default();
    let mut b = Batch::default();
    let mut rng = Rand::new(0x9a7e);
    let pit_half = 1.27f32;

    // The pit floor and its puddles.
    let floor = |_: f64, x: f32, w: Vec2| {
        Surf::new(
            0.0045 + fbm_world(w.x, w.y, 0.5, 3, 0x91) * 0.0045,
            1.0,
            Vec3::splat(0.55 + 0.15 * fbm_world(w.x, w.y, 0.8, 2, 0x92))
                * (1.0 - 0.3 * smooth(0.9, 1.27, x.abs())),
        )
    };
    out.parts.extend(ribbon::ribbon(&Ribbon {
        mat: Mat::PitFloor,
        s0: s0 + 0.12,
        s1: s1 - 0.12,
        fade_in: 0.0,
        fade_out: 0.0,
        ds: 0.05,
        columns: columns(pit_half, 0.05, 2.0, 0.05, 2.0, 0.05),
        surface: &floor,
        shadows: false,
    }));
    let water = |_: f64, _: f32, _: Vec2| Surf::new(0.0052, 1.0, Vec3::ONE);
    out.parts.extend(ribbon::ribbon(&Ribbon {
        mat: Mat::Water,
        s0: s0 + 0.15,
        s1: s1 - 0.15,
        fade_in: 0.0,
        fade_out: 0.0,
        ds: 0.25,
        columns: columns(pit_half - 0.02, 0.2, 2.0, 0.2, 2.0, 0.2),
        surface: &water,
        shadows: false,
    }));
    for _ in 0..90 {
        let s = rng.range(s0 as f32 + 0.2, s1 as f32 - 0.2) as f64;
        let x = rng.range(-1.2, 1.2);
        let size = rng.range(0.02, 0.07);
        let base = &super::dress::rock_kit(0.5)[rng.below(12)];
        append(
            b.at(s, Mat::Stone),
            base,
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
            Vec3::new(size, size * 0.6, size * rng.range(0.8, 1.3)),
            point(s, x, 0.002),
            (hex(0x4a4642) * rng.range(0.7, 1.2)).extend(1.0),
        );
    }

    // Concrete curbs along both sides and across both ends.
    let len = (s1 - s0) as f32;
    let mid = (s0 + s1) * 0.5;
    for side in [-1.0f32, 1.0] {
        let curb = beveled_box(
            Vec3::new(0.2, 0.07, len),
            0.008,
            0.5,
            Vec2::new(rng.f(), 0.0),
            |p| {
                let low = 1.0 - smooth(-0.03, 0.02, p.y);
                (Vec3::splat(0.82) * (1.0 - 0.3 * low)).extend(1.0)
            },
        );
        let mut part = Part::own(
            curb,
            Mat::Concrete,
            point(mid, side * (pit_half + 0.1), 0.021),
        );
        part.rot = trail_rot(mid);
        out.parts.push(part);
    }
    for ss in [s0 + 0.06, s1 - 0.06] {
        let curb = beveled_box(
            Vec3::new(2.0 * pit_half, 0.07, 0.12),
            0.008,
            0.5,
            Vec2::new(rng.f(), 0.0),
            |_| Vec4::new(0.8, 0.8, 0.8, 1.0),
        );
        let mut part = Part::own(curb, Mat::Concrete, point(ss, 0.0, 0.021));
        part.rot = trail_rot(ss);
        out.parts.push(part);
    }

    // The angle-iron frame round the opening, with bolts.
    let frame_len = len - 0.24;
    for side in [-1.0f32, 1.0] {
        let x = side * pit_half;
        let vert_leg = block(Vec3::new(0.005, 0.035, frame_len));
        let mut part = Part::own(
            vert_leg,
            Mat::Zinc,
            point(mid, x - side * 0.0025, PATH_Y - 0.0175),
        );
        part.rot = trail_rot(mid);
        out.parts.push(part);
        let flat_leg = block(Vec3::new(0.045, 0.005, frame_len));
        let mut part = Part::own(
            flat_leg,
            Mat::Zinc,
            point(mid, x + side * 0.0225, PATH_Y - 0.0025),
        );
        part.rot = trail_rot(mid);
        out.parts.push(part);
        let mut s = s0 + 0.3;
        while s < s1 - 0.25 {
            let p = point(s, x + side * 0.03, PATH_Y);
            let bolt = tube(
                &[p, p + Vec3::Y * 0.006],
                6,
                true,
                |_, _| 0.0095,
                |_| Vec4::new(0.8, 0.8, 0.8, 1.0),
            );
            append(
                b.at(s, Mat::Zinc),
                &bolt,
                Quat::IDENTITY,
                Vec3::ONE,
                Vec3::ZERO,
                Vec4::ONE,
            );
            s += 0.45;
        }
    }
    // A support beam along the middle, under the bars.
    {
        let mut beam = block(Vec3::new(0.07, 0.006, frame_len));
        let web = block(Vec3::new(0.006, 0.022, frame_len));
        append(
            &mut beam,
            &web,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::new(0.0, -0.014, 0.0),
            Vec4::ONE,
        );
        let mut part = Part::own(beam, Mat::Zinc, point(mid, 0.0, 0.027));
        part.rot = trail_rot(mid);
        out.parts.push(part);
    }

    // Grating panels.
    let inner = len - 0.24;
    let panels = (inner / 1.0).round().max(1.0) as usize;
    let panel_len = inner / panels as f32 - 0.006;
    let span = 2.0 * pit_half - 0.012;
    let key = "grating-panel";
    out.shared
        .push((key.into(), grating_panel(span, panel_len)));
    for k in 0..panels {
        let s = s0 + 0.12 + (inner as f64) * (k as f64 + 0.5) / panels as f64;
        out.parts.push(Part::shared(
            key,
            Mat::Zinc,
            point(s, 0.0, PATH_Y),
            trail_rot(s),
            Vec3::ONE,
        ));
    }

    railings_steel(&mut out, &mut b, s0, s1);
    out.parts.extend(b.finish());
    out
}

/// One grating panel, its top at y = 0: bearing bars across (x) every 33 mm, twisted
/// square cross rods along (z) every 100 mm, banding bars at both ends.
fn grating_panel(span: f32, len: f32) -> Mesh {
    let mut m = Mesh::default();
    let bars = (len / 0.033).floor() as usize;
    for k in 0..=bars {
        let z = -len * 0.5 + 0.004 + (len - 0.008) * k as f32 / bars as f32;
        let bar = block(Vec3::new(span, 0.03, 0.004));
        append(
            &mut m,
            &bar,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::new(0.0, -0.015, z),
            Vec4::ONE,
        );
    }
    let rods = (span / 0.1).floor() as usize;
    for k in 0..=rods {
        let x = -span * 0.5 + 0.02 + (span - 0.04) * k as f32 / rods as f32;
        let pts: Vec<Vec3> = (0..=8)
            .map(|i| Vec3::new(x, -0.004, -len * 0.5 + len * i as f32 / 8.0))
            .collect();
        let rod = tube(
            &pts,
            4,
            false,
            |t, a| {
                // A twisted square bar: the corners turn along its length.
                let twist = (a * 2.0 + t * 40.0).cos().abs();
                0.0035 + 0.0012 * twist
            },
            |_| Vec4::ONE,
        );
        append(
            &mut m,
            &rod,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::ZERO,
            Vec4::ONE,
        );
    }
    for z in [-len * 0.5 + 0.0025, len * 0.5 - 0.0025] {
        let band = block(Vec3::new(span, 0.03, 0.005));
        append(
            &mut m,
            &band,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::new(0.0, -0.015, z),
            Vec4::ONE,
        );
    }
    for x in [-span * 0.5 + 0.0025, span * 0.5 - 0.0025] {
        let band = block(Vec3::new(0.005, 0.03, len));
        append(
            &mut m,
            &band,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::new(x, -0.015, 0.0),
            Vec4::ONE,
        );
    }
    m
}

fn railings_steel(out: &mut Out, b: &mut Batch, s0: f64, s1: f64) {
    let x = 1.43;
    let top = 1.07;
    let n = ((s1 - s0) / 1.5).round().max(1.0) as usize;
    let white = |_: f32| Vec4::ONE;
    out.shared.push(("rail-post".into(), {
        let mut m = tube(
            &[Vec3::ZERO, Vec3::Y * (top - PATH_Y)],
            16,
            true,
            |_, _| 0.021,
            white,
        );
        let plate = block(Vec3::new(0.11, 0.008, 0.11));
        append(
            &mut m,
            &plate,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::Y * 0.004,
            Vec4::ONE,
        );
        for (dx, dz) in [
            (-0.038, -0.038),
            (0.038, -0.038),
            (0.038, 0.038),
            (-0.038, 0.038),
        ] {
            let bolt = tube(
                &[Vec3::new(dx, 0.008, dz), Vec3::new(dx, 0.014, dz)],
                6,
                true,
                |_, _| 0.007,
                white,
            );
            append(
                &mut m,
                &bolt,
                Quat::IDENTITY,
                Vec3::ONE,
                Vec3::ZERO,
                Vec4::ONE,
            );
        }
        m
    }));
    for side in [-1.0f32, 1.0] {
        let mut posts = Vec::new();
        for k in 0..=n {
            let s = s0 + 0.15 + (s1 - s0 - 0.3) * k as f64 / n as f64;
            posts.push(s);
            out.parts.push(Part::shared(
                "rail-post",
                Mat::Zinc,
                point(s, side * x, PATH_Y),
                trail_rot(s),
                Vec3::ONE,
            ));
        }
        // Top rail bending down into the end posts, and a mid rail.
        let (a, e) = (posts[0], posts[n]);
        let mut pts = Vec::new();
        let steps = 60;
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let s = a + (e - a) * t;
            let bend_a = smooth(0.0, 0.12, (s - a) as f32);
            let bend_e = smooth(0.0, 0.12, (e - s) as f32);
            let y = top - 0.12 * (1.0 - bend_a.min(bend_e)).powi(2);
            pts.push(point(s, side * x, y));
        }
        let rail = tube(&pts, 16, true, |_, _| 0.021, white);
        append(
            b.at((a + e) * 0.5, Mat::Zinc),
            &rail,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::ZERO,
            Vec4::ONE,
        );
        let mid: Vec<Vec3> = (0..=20)
            .map(|i| point(a + (e - a) * i as f64 / 20.0, side * x, 0.56))
            .collect();
        let rail = tube(&mid, 12, true, |_, _| 0.017, white);
        append(
            b.at((a + e) * 0.5, Mat::Zinc),
            &rail,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::ZERO,
            Vec4::ONE,
        );
    }
}

// ------------------------------------------------------------------------ underpass

struct Underpass {
    s0: f64,
    s1: f64,
    mid: f64,
    /// +1 if the wall is on the trail's right (toward the loop's middle), else -1.
    inside: f32,
    half_w: f32,
    h: f32,
}

fn underpass_layout() -> Option<Underpass> {
    let s0 = stretch_start("concrete")?;
    let s1 = s0 + 9.0;
    let mid = (s0 + s1) * 0.5;
    let (p, _, right) = frame(mid);
    let inside = if p.dot(right) > 0.0 { -1.0 } else { 1.0 };
    Some(Underpass {
        s0,
        s1,
        mid,
        inside,
        half_w: TRAIL_WIDTH * 0.5 + 0.9,
        h: 3.0,
    })
}

/// The underpass's echo and its sky-light volume (spawned with or without a renderer).
pub(super) fn underpass_volumes(world: &mut World, origin: DVec3) {
    let Some(u) = underpass_layout() else {
        return;
    };
    let (mid, d) = trail_point(u.mid);
    let rot = crate::demo::yaw(d);
    let center = origin + mid + DVec3::new(0.0, u.h as f64 * 0.5, 0.0);
    world.spawn((
        Name::new("Underpass echo"),
        Transform::from_translation(center).with_rotation(rot),
        ReverbZone {
            half_extents: Vec3::new(u.half_w, u.h * 0.5 + 0.3, 4.7),
            room: 0.55,
            damping: 0.15,
            wet: 0.42,
            fade_m: 1.2,
        },
    ));
    // Local +X of `yaw(d)` is the trail's right; the open side is opposite the wall.
    let open = if u.inside > 0.0 {
        engine_render::BoxFace::NegX
    } else {
        engine_render::BoxFace::PosX
    };
    world.spawn((
        Name::new("Underpass shade"),
        Transform::from_translation(center).with_rotation(rot),
        sky_volume([u.half_w + 0.2, u.h * 0.5, 4.9], open, 0.2, 2.5),
    ));
    // The renderer lights a surface facing down with the sunlit ground of the whole valley,
    // but under the roof the ceiling and the tops of the walls mostly see the shaded floor:
    // a darker volume up there, fading in from 1.6 m up, keeps them grey instead of grass
    // green.
    let top = 0.725;
    world.spawn((
        Name::new("Underpass ceiling shade"),
        Transform::from_translation(center + DVec3::new(0.0, (u.h * 0.5 - top + 0.05) as f64, 0.0))
            .with_rotation(rot),
        engine_render::SkyLightVolume::new([u.half_w + 0.4, top, 4.9])
            .open(engine_render::BoxFace::PosZ)
            .open(engine_render::BoxFace::NegZ)
            .open(engine_render::BoxFace::NegY)
            .with_sky_light(0.06)
            .with_fade(1.2),
    ));
}

/// Cast concrete: a board-formed wall on the loop's inside, chamfered pillars on the
/// outside, a slab roof with warm lamps under it, floor slabs with expansion joints and
/// wear, and a channel drain along the wall.
pub(super) fn underpass() -> Out {
    let Some(u) = underpass_layout() else {
        return Out::default();
    };
    let mut out = Out::default();
    let mut b = Batch::default();
    let mut rng = Rand::new(0xc0c2);
    let rot = trail_rot(u.mid);
    let wall_x = u.inside * u.half_w;

    // Wall: rain streaks run down from the top in the texture; dirt splashes up the base.
    let mut wall = beveled_box(
        Vec3::new(0.4, u.h, 9.4),
        0.025,
        0.5,
        Vec2::new(0.4, 0.0),
        |p| {
            let low = 1.0 - smooth(-u.h * 0.5, -u.h * 0.5 + 0.45, p.y);
            (Vec3::new(1.0, 1.0, 0.97) * (1.0 - 0.22 * low)).extend(1.0)
        },
    );
    flip_v(&mut wall, u.h * 0.5);
    out.parts.push(Part {
        rot,
        ..Part::own(wall, Mat::Formwork, point(u.mid, wall_x, u.h * 0.5))
    });

    // Roof slab.
    let roof = beveled_box(
        Vec3::new(2.0 * u.half_w + 0.8, 0.45, 9.8),
        0.03,
        1.0,
        Vec2::new(1.1, 0.7),
        |p| {
            let edge = smooth(4.4, 4.9, p.z.abs());
            (Vec3::splat(0.92) * (1.0 - 0.15 * edge)).extend(1.0)
        },
    );
    out.parts.push(Part {
        rot,
        ..Part::own(roof, Mat::Formwork, point(u.mid, 0.0, u.h + 0.225))
    });

    // Pillars on the open side.
    let pillar_key = "underpass-pillar";
    let mut pillar = beveled_box(
        Vec3::new(0.45, u.h, 0.45),
        0.03,
        0.5,
        Vec2::new(0.0, 0.3),
        |p| {
            let low = 1.0 - smooth(-u.h * 0.5, -u.h * 0.5 + 0.4, p.y);
            (Vec3::splat(1.0) * (1.0 - 0.22 * low)).extend(1.0)
        },
    );
    flip_v(&mut pillar, u.h * 0.5);
    out.shared.push((pillar_key.into(), pillar));
    for k in 0..4 {
        let s = u.s0 + 0.3 + k as f64 * (8.4 / 3.0);
        out.parts.push(Part::shared(
            pillar_key,
            Mat::Formwork,
            point(s, -wall_x, u.h * 0.5),
            rot,
            Vec3::ONE,
        ));
    }

    // Floor slabs with expansion joints; lighter and smoother where people walk, dirtier
    // toward the wall.
    let slabs = 3;
    let slab_len = (u.s1 - u.s0) as f32 / slabs as f32;
    let inside = u.inside;
    for k in 0..slabs {
        let s = u.s0 + slab_len as f64 * (k as f64 + 0.5);
        let seed = rng.seed();
        let slab = beveled_box(
            Vec3::new(4.6, 0.09, slab_len - 0.014),
            0.004,
            0.15,
            Vec2::new(rng.f() * 2.0, rng.f() * 2.0),
            move |p| {
                let worn = 1.0 - smooth(0.25, 0.9, p.x.abs());
                let wallward = smooth(0.8, 2.0, p.x * inside);
                let stain = smooth(
                    0.1,
                    0.6,
                    fbm_world(p.x, p.z + seed as f32 * 0.001, 0.9, 3, seed),
                );
                let c =
                    Vec3::splat(1.0 + 0.1 * worn) * (1.0 - 0.25 * wallward) * (1.0 - 0.18 * stain);
                c.extend(1.0)
            },
        );
        out.parts.push(Part {
            rot: trail_rot(s),
            ..Part::own(slab, Mat::Concrete, point(s, 0.0, PATH_Y - 0.045))
        });
        if k > 0 {
            let js = u.s0 + slab_len as f64 * k as f64;
            let joint = block(Vec3::new(4.58, 0.06, 0.016));
            out.parts.push(Part {
                rot: trail_rot(js),
                ..Part::own(joint, Mat::Sealant, point(js, 0.0, PATH_Y - 0.033))
            });
        }
    }

    // Channel drain along the wall: a dark slot under a galvanized grate.
    let drain_x = inside * 1.68;
    let slot = block(Vec3::new(0.1, 0.004, (u.s1 - u.s0) as f32 - 0.3));
    out.parts.push(Part {
        rot,
        ..Part::own(slot, Mat::LampBody, point(u.mid, drain_x, PATH_Y + 0.0005))
    });
    let mut s = u.s0 + 0.15;
    while s < u.s1 - 0.16 {
        let bar = block(Vec3::new(0.12, 0.007, 0.006));
        append(
            b.at(s, Mat::Zinc),
            &bar,
            trail_rot(s),
            Vec3::ONE,
            point(s, drain_x, PATH_Y + 0.0035),
            Vec4::ONE,
        );
        s += 0.022;
    }
    for dx in [-0.062f32, 0.062] {
        let rail = block(Vec3::new(0.008, 0.008, (u.s1 - u.s0) as f32 - 0.3));
        out.parts.push(Part {
            rot,
            ..Part::own(rail, Mat::Zinc, point(u.mid, drain_x + dx, PATH_Y + 0.004))
        });
    }

    // Lamps under the roof with a conduit joining them.
    let lamp_s = [u.mid - 2.9, u.mid, u.mid + 2.9];
    for &s in &lamp_s {
        let body = beveled_box(Vec3::new(0.18, 0.07, 0.5), 0.012, 0.5, Vec2::ZERO, |_| {
            Vec4::ONE
        });
        out.parts.push(Part {
            rot,
            ..Part::own(body, Mat::LampBody, point(s, 0.0, u.h - 0.035))
        });
        let glass = beveled_box(Vec3::new(0.13, 0.014, 0.44), 0.006, 0.5, Vec2::ZERO, |_| {
            Vec4::ONE
        });
        out.parts.push(Part {
            rot,
            ..Part::own(glass, Mat::Lamp, point(s, 0.0, u.h - 0.074)).no_shadows()
        });
        out.lights.push((
            point(s, 0.0, u.h - 0.2),
            PointLight {
                color: [1.0, 0.76, 0.48],
                intensity_lm: 1800.0,
                range: 11.0,
                radius: 0.12,
            },
        ));
    }
    let conduit: Vec<Vec3> = (0..=24)
        .map(|i| {
            point(
                lamp_s[0] + (lamp_s[2] - lamp_s[0]) * i as f64 / 24.0,
                0.13,
                u.h - 0.015,
            )
        })
        .collect();
    let pipe = tube(
        &conduit,
        8,
        true,
        |_, _| 0.012,
        |_| Vec4::new(0.85, 0.85, 0.85, 1.0),
    );
    append(
        b.at(u.mid, Mat::Zinc),
        &pipe,
        Quat::IDENTITY,
        Vec3::ONE,
        Vec3::ZERO,
        Vec4::ONE,
    );
    let down: Vec<Vec3> = (0..=10)
        .map(|i| {
            let t = i as f32 / 10.0;
            point(
                lamp_s[0] - 0.6,
                wall_x - inside * 0.215,
                u.h - 0.015 - t * 1.1,
            )
        })
        .collect();
    let pipe = tube(
        &down,
        8,
        true,
        |_, _| 0.012,
        |_| Vec4::new(0.85, 0.85, 0.85, 1.0),
    );
    append(
        b.at(u.mid, Mat::Zinc),
        &pipe,
        Quat::IDENTITY,
        Vec3::ONE,
        Vec3::ZERO,
        Vec4::ONE,
    );
    let across: Vec<Vec3> = (0..=10)
        .map(|i| {
            let t = i as f32 / 10.0;
            point(
                lamp_s[0] - 0.6 + 0.6 * t as f64,
                (wall_x - inside * 0.215) * (1.0 - t) + 0.13 * t,
                u.h - 0.015,
            )
        })
        .collect();
    let pipe = tube(
        &across,
        8,
        true,
        |_, _| 0.012,
        |_| Vec4::new(0.85, 0.85, 0.85, 1.0),
    );
    append(
        b.at(u.mid, Mat::Zinc),
        &pipe,
        Quat::IDENTITY,
        Vec3::ONE,
        Vec3::ZERO,
        Vec4::ONE,
    );

    // A few leaves blown in at the ends.
    let mut leaves = Mesh::default();
    for _ in 0..60 {
        let s = if rng.chance(0.5) {
            u.s0 + rng.range(0.0, 1.2) as f64
        } else {
            u.s1 - rng.range(0.0, 1.0) as f64
        };
        let x = rng.range(-1.9, 1.9);
        let col = [
            Vec3::new(0.55, 0.3, 0.08),
            Vec3::new(0.45, 0.25, 0.1),
            Vec3::new(0.65, 0.45, 0.12),
        ][rng.below(3)];
        super::procgen::leaf(
            &mut leaves,
            point(s, x, PATH_Y + 0.004),
            super::procgen::tilt(
                Vec3::new(rng.normal(0.3), 1.0, rng.normal(0.3)),
                rng.range(0.0, 6.3),
            ),
            rng.range(0.05, 0.09),
            rng.range(0.04, 0.07),
            -0.006,
            0.005,
            rng.below(4) as u32,
            col.extend(1.0),
        );
    }
    out.parts.push(Part::own(leaves, Mat::Leaf, Vec3::ZERO));
    out.parts.extend(b.finish());
    out
}

/// Make texture v run down from `top` (beveled boxes give v = height).
fn flip_v(m: &mut Mesh, top: f32) {
    for v in &mut m.vertices {
        let n = Vec3::from(v.normal);
        if n.y.abs() < 0.9 {
            v.uv[1] = top - v.uv[1];
        }
    }
}

// -------------------------------------------------------------------------- markers

/// A wooden marker post at the start of every stretch with a sign plate painted in the
/// stretch's ring colour and a small glowing strip.
pub(super) fn markers() -> Out {
    let mut out = Out::default();
    let post_h = 1.3;
    out.shared.push(("marker-post".into(), {
        let mut m = beveled_box(
            Vec3::new(0.1, post_h, 0.1),
            0.012,
            0.25,
            Vec2::new(0.7, 0.2),
            |p| timber_color(p + Vec3::Y * post_h * 0.5, 0.15, Vec3::splat(0.78)),
        );
        let cap = pyramid(0.056, 0.05, Vec4::new(0.55, 0.55, 0.55, 1.0));
        append(
            &mut m,
            &cap,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::Y * post_h * 0.5,
            Vec4::ONE,
        );
        m
    }));
    out.shared.push((
        "marker-sign".into(),
        beveled_box(
            Vec3::new(0.024, 0.2, 0.32),
            0.006,
            0.5,
            Vec2::new(0.3, 0.05),
            |p| {
                let back = smooth(0.0, -0.012, p.x);
                (Vec3::ONE * (1.0 - 0.3 * back)).extend(1.0)
            },
        ),
    ));
    out.shared.push((
        "marker-glow".into(),
        beveled_box(
            Vec3::new(0.006, 0.022, 0.26),
            0.002,
            1.0,
            Vec2::ZERO,
            |_| Vec4::ONE,
        ),
    ));
    for (i, &(name, _)) in STRETCHES.iter().enumerate() {
        let start = stretch_start(name).unwrap_or(0.0);
        let s = if name == "concrete" {
            start - 0.7
        } else {
            start + 0.25
        };
        let x = 1.9;
        let base = super::ribbon::base(x, {
            let p = point(s, x, 0.0);
            Vec2::new(p.x, p.z)
        });
        let rot = trail_rot(s) * Quat::from_rotation_y(0.12);
        let post_at = point(s, x, base - 0.1 + post_h * 0.5);
        out.parts.push(Part::shared(
            "marker-post",
            Mat::Timber,
            post_at,
            rot,
            Vec3::ONE,
        ));
        // The sign faces the path (local -X), leaning back a little.
        let lean = Quat::from_rotation_z(-0.16);
        let sign_at = post_at + rot * Vec3::new(-0.064, 0.33, 0.0);
        out.parts.push(Part::shared(
            "marker-sign",
            Mat::Sign(i as u8),
            sign_at,
            rot * lean,
            Vec3::ONE,
        ));
        let glow_at = sign_at + rot * lean * Vec3::new(-0.014, 0.07, 0.0);
        out.parts.push(
            Part::shared(
                "marker-glow",
                Mat::Glow(i as u8),
                glow_at,
                rot * lean,
                Vec3::ONE,
            )
            .no_shadows(),
        );
    }
    out
}
