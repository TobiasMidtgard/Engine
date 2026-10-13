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

/// Half the width of the pit the grating spans, meters.
const PIT_HALF: f32 = 1.27;
/// Height of the bearing bars, meters.
const BAR_H: f32 = 0.03;

/// A walkway of galvanized bar grating panels (bearing bars across, twisted cross rods)
/// set between two steel channel stringers on concrete curbs, over a dark pit of wet silt,
/// stones and leaves, with tubular steel railings. The bar tops are worn bright down the
/// middle where feet go and grimy and rusty toward the sides; the bars darken down into
/// the pit, which a sky-light volume keeps in shade ([`grating_volume`]). Weeds grow up
/// through the gaps along the edges.
pub(super) fn grating() -> Out {
    let (s0, s1) = range("metal");
    let mut out = Out::default();
    let mut b = Batch::default();
    let mut rng = Rand::new(0x9a7e);
    let len = (s1 - s0) as f32;
    let mid = (s0 + s1) * 0.5;

    // The pit floor: dark, wet silt and grit a few centimeters under the bars.
    let floor = |_: f64, x: f32, w: Vec2| {
        let wet = smooth(-0.2, 0.4, fbm_world(w.x, w.y, 0.7, 2, 0x93));
        let tone = 0.4 + 0.12 * fbm_world(w.x, w.y, 0.8, 2, 0x92);
        Surf::new(
            0.003 + fbm_world(w.x, w.y, 0.5, 3, 0x91) * 0.003,
            1.0,
            Vec3::new(1.0, 0.93, 0.84)
                * tone
                * (1.0 - 0.35 * wet)
                * (1.0 - 0.3 * smooth(0.9, PIT_HALF, x.abs())),
        )
    };
    out.parts.extend(ribbon::ribbon(&Ribbon {
        mat: Mat::PitFloor,
        s0: s0 + 0.12,
        s1: s1 - 0.12,
        fade_in: 0.0,
        fade_out: 0.0,
        ds: 0.05,
        columns: columns(PIT_HALF, 0.05, 2.0, 0.05, 2.0, 0.05),
        surface: &floor,
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
            (hex(0x3e3a36) * rng.range(0.7, 1.2)).extend(1.0),
        );
    }
    // Wet, blackened leaves blown in on the pit floor, and dry ones on the bars and caught
    // in the gaps (fewer where feet go).
    for k in 0..(len * 14.0) as usize {
        let in_pit = k % 3 != 0;
        let s = rng.range(s0 as f32 + 0.15, s1 as f32 - 0.15) as f64;
        let x = rng.range(-1.24, 1.24);
        let trodden = 1.0 - smooth(0.35, 0.8, x.abs());
        if !in_pit && rng.chance(0.75 * trodden) {
            continue;
        }
        let browns = [hex(0x8a5a2a), hex(0x6e4a26), hex(0x9a7038), hex(0x5a3c22)];
        let (y, tilt_sd, col) = if in_pit {
            (0.008, 0.25, browns[rng.below(4)] * rng.range(0.35, 0.55))
        } else if rng.chance(0.4) {
            // Wedged in a gap, standing steeply.
            (
                PATH_Y - 0.012,
                1.4,
                browns[rng.below(4)] * rng.range(0.7, 1.0),
            )
        } else {
            (
                PATH_Y + 0.004,
                0.12,
                browns[rng.below(4)] * rng.range(0.75, 1.05),
            )
        };
        let rot = super::procgen::tilt(
            Vec3::new(rng.normal(tilt_sd), 1.0, rng.normal(tilt_sd)),
            rng.range(0.0, 6.3),
        );
        let l = rng.range(0.04, 0.08);
        super::procgen::leaf(
            b.at(s, Mat::Leaf),
            point(s, x, y),
            rot,
            l,
            l * rng.range(0.6, 0.9),
            rng.range(-0.008, 0.002),
            rng.range(0.0, 0.01),
            rng.below(4) as u32,
            col.extend(1.0),
        );
    }
    // Weeds rooted in the silt along the edges, growing up through the gaps in clusters.
    let mut s = s0 + 0.2;
    while s < s1 - 0.2 {
        s += rng.range(0.08, 0.3) as f64;
        let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
        let p = point(s, side, 0.0);
        if fbm_world(p.x, p.z, 0.9, 2, 0x9a1) < 0.05 {
            continue;
        }
        let x = side * (PIT_HALF - 0.03 - rng.f().powf(1.6) * 0.32);
        let tall = rng.range(0.07, 0.24);
        super::dress::clump(
            b.at(s, Mat::Grass),
            &mut rng,
            point(s, x, 0.004),
            super::dress::ClumpStyle {
                blades: (3, 10),
                radius: 0.035,
                height: (tall * 0.5, tall),
                lean: (0.05, 0.35),
                toward: None,
                width: (0.003, 0.006),
                palette: super::dress::Palette::Meadow,
            },
        );
    }

    // Concrete curbs along both sides (just above the ground, under the stringers and the
    // railing posts) and across both ends.
    for side in [-1.0f32, 1.0] {
        let seed = rng.seed();
        let curb = beveled_box(
            Vec3::new(0.28, 0.04, len),
            0.01,
            0.4,
            Vec2::new(rng.f(), 0.0),
            |p| {
                let low = 1.0 - smooth(-0.02, 0.02, p.y);
                let stain = smooth(0.0, 0.6, fbm_world(p.z, p.y * 4.0 + p.x, 0.5, 3, seed));
                // Moss creeping in from the lawn side.
                let moss = smooth(0.2, 0.7, fbm_world(p.z, p.x * 3.0, 0.35, 3, seed ^ 0x55))
                    * smooth(-0.04, 0.14, p.x * side);
                let c = Vec3::splat(0.8 * (1.0 - 0.3 * low) * (1.0 - 0.25 * stain));
                c.lerp(Vec3::new(0.32, 0.38, 0.18), moss * 0.7).extend(1.0)
            },
        );
        let mut part = Part::own(curb, Mat::Concrete, point(mid, side * 1.41, -0.005));
        part.rot = trail_rot(mid);
        out.parts.push(part);
    }
    for ss in [s0 + 0.06, s1 - 0.06] {
        let curb = beveled_box(
            Vec3::new(2.0 * PIT_HALF, 0.07, 0.12),
            0.008,
            0.5,
            Vec2::new(rng.f(), 0.0),
            |p| {
                let low = 1.0 - smooth(-0.03, 0.02, p.y);
                (Vec3::splat(0.8) * (1.0 - 0.3 * low)).extend(1.0)
            },
        );
        let mut part = Part::own(curb, Mat::Concrete, point(ss, 0.0, 0.021));
        part.rot = trail_rot(ss);
        out.parts.push(part);
    }

    // Steel channel stringers on the curbs along both sides, toes outward, the grating set
    // inside them: web, top and bottom flanges, stiffeners at every panel joint and a bolt
    // through the web every half meter.
    let inner = len - 0.24;
    let panels = inner.round().max(1.0) as usize;
    let joint = |k: usize| s0 + 0.12 + (inner as f64) * k as f64 / panels as f64;
    let st_len = len - 0.04;
    for side in [-1.0f32, 1.0] {
        let seed = rng.seed();
        for (size, x, y) in [
            (Vec3::new(0.006, 0.063, st_len), PIT_HALF + 0.003, 0.0465),
            (Vec3::new(0.05, 0.006, st_len), PIT_HALF + 0.025, 0.075),
            (Vec3::new(0.05, 0.006, st_len), PIT_HALF + 0.025, 0.018),
        ] {
            let piece = beveled_box(size, 0.0015, 0.25, Vec2::new(rng.f(), 0.0), |p| {
                weathered_steel(p.z, y + p.y, x * side, seed)
            });
            let mut part = Part::own(piece, Mat::Grating, point(mid, side * x, y));
            part.rot = trail_rot(mid);
            out.parts.push(part);
        }
        let web_out = PIT_HALF + 0.006;
        for k in 0..=panels {
            let s = joint(k).clamp(s0 + 0.05, s1 - 0.05);
            let mut plate = block(Vec3::new(0.043, 0.051, 0.006));
            paint(&mut plate, |p| {
                weathered_steel(s as f32, p.y + 0.0465, side, seed)
            });
            append(
                b.at(s, Mat::Grating),
                &plate,
                trail_rot(s),
                Vec3::ONE,
                point(s, side * (web_out + 0.0215), 0.0465),
                Vec4::ONE,
            );
        }
        let mut s = s0 + 0.3;
        while s < s1 - 0.25 {
            let n = side * frame(s).2;
            let p = point(s, side * web_out, 0.0465);
            let c = weathered_steel(s as f32, 0.04, side, seed).truncate() * 0.85;
            let washer = tube(
                &[p, p + n * 0.002],
                12,
                true,
                |_, _| 0.012,
                |_| c.extend(1.0),
            );
            let head = tube(
                &[p + n * 0.002, p + n * 0.0075],
                6,
                true,
                |_, _| 0.0085,
                |_| c.extend(1.0),
            );
            for m in [&washer, &head] {
                append(
                    b.at(s, Mat::Zinc),
                    m,
                    Quat::IDENTITY,
                    Vec3::ONE,
                    Vec3::ZERO,
                    Vec4::ONE,
                );
            }
            s += 0.5;
        }
    }
    // A support beam along the middle, under the bars.
    {
        let mut beam = block(Vec3::new(0.07, 0.006, inner));
        let web = block(Vec3::new(0.006, 0.022, inner));
        append(
            &mut beam,
            &web,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::new(0.0, -0.014, 0.0),
            Vec4::ONE,
        );
        paint(&mut beam, |_| Vec4::new(0.45, 0.43, 0.4, 1.0));
        let mut part = Part::own(beam, Mat::Grating, point(mid, 0.0, 0.027));
        part.rot = trail_rot(mid);
        out.parts.push(part);
    }

    // Grating panels, each a little different, held down by clips at their corners.
    let panel_len = inner / panels as f32 - 0.006;
    let span = 2.0 * PIT_HALF - 0.012;
    for k in 0..panels {
        let s = (joint(k) + joint(k + 1)) * 0.5;
        let (mut tops, mut steel) = (Mesh::default(), Mesh::default());
        grating_panel(&mut tops, &mut steel, span, panel_len, s, &mut rng);
        let rot = trail_rot(s);
        let at = point(s, 0.0, PATH_Y);
        append(b.at(s, Mat::Tread), &tops, rot, Vec3::ONE, at, Vec4::ONE);
        append(b.at(s, Mat::Grating), &steel, rot, Vec3::ONE, at, Vec4::ONE);
    }

    railings_steel(&mut out, &mut b, s0, s1, 0.015);
    out.parts.extend(b.finish());
    out
}

/// Colour of weathered galvanized steel at `along` meters and `y` up: duller and darker
/// low down, with rust bleeding out in patches toward the ground.
fn weathered_steel(along: f32, y: f32, x: f32, seed: u32) -> Vec4 {
    let low = 1.0 - smooth(0.0, 0.07, y);
    let rust = smooth(0.15, 0.6, fbm_world(along, y * 6.0 + x, 0.3, 3, seed)) * (0.35 + 0.65 * low);
    let grey = Vec3::splat(0.82 - 0.38 * low) * Vec3::new(1.0, 0.98, 0.94);
    grey.lerp(Vec3::new(0.52, 0.3, 0.16), rust * 0.75)
        .extend(1.0)
}

/// Recolour every vertex of `m` by its position.
fn paint(m: &mut Mesh, color: impl Fn(Vec3) -> Vec4) {
    for v in &mut m.vertices {
        v.color = color(Vec3::from(v.position)).to_array();
    }
}

/// A quad `p` (corners in order round it) facing `n`.
fn quad(m: &mut Mesh, p: [Vec3; 4], n: Vec3, uv: [Vec2; 4], c: [Vec4; 4]) {
    let base = m.vertices.len() as u32;
    for i in 0..4 {
        m.vertices.push(vert(p[i], n, uv[i], c[i]));
    }
    let g = (p[1] - p[0]).cross(p[2] - p[0]);
    if g.dot(n) >= 0.0 {
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    } else {
        m.indices
            .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    }
}

/// How one grating bar looks: UVs of its top, its top's colour, and its sides' colour at
/// (x, z) and `t` up its side (0 at the bottom, 1 at the top).
struct BarLook<'a> {
    top_uv: &'a dyn Fn(f32, f32) -> Vec2,
    top: Vec4,
    side: &'a dyn Fn(f32, f32, f32) -> Vec4,
}

/// A flat bar on edge, `BAR_H` tall under y = 0 and `thick` thick, from `a` to `b` along
/// local x at z = `at` (or along z at x = `at` when `along_z`). Its top goes into `tops`,
/// its two long sides into `steel`, split about every 0.2 m for their colours; the
/// underside and ends are never seen.
#[allow(clippy::too_many_arguments)]
fn flat_bar(
    tops: &mut Mesh,
    steel: &mut Mesh,
    along_z: bool,
    at: f32,
    a: f32,
    b: f32,
    thick: f32,
    look: &BarLook,
) {
    let h = thick * 0.5;
    let p = |l: f32, c: f32, y: f32| {
        if along_z {
            Vec3::new(at + c, y, l)
        } else {
            Vec3::new(l, y, at + c)
        }
    };
    let across = if along_z { Vec3::X } else { Vec3::Z };
    let corners = [p(a, -h, 0.0), p(b, -h, 0.0), p(b, h, 0.0), p(a, h, 0.0)];
    quad(
        tops,
        corners,
        Vec3::Y,
        corners.map(|q| (look.top_uv)(q.x, q.z)),
        [look.top; 4],
    );
    let n = ((b - a) / 0.2).ceil().max(1.0) as usize;
    for sign in [-1.0f32, 1.0] {
        for i in 0..n {
            let (la, lb) = (
                a + (b - a) * i as f32 / n as f32,
                a + (b - a) * (i + 1) as f32 / n as f32,
            );
            let q = [
                p(la, sign * h, -BAR_H),
                p(lb, sign * h, -BAR_H),
                p(lb, sign * h, 0.0),
                p(la, sign * h, 0.0),
            ];
            let c = [0.0, 0.0, 1.0, 1.0];
            quad(
                steel,
                q,
                across * sign,
                [0, 1, 2, 3].map(|k| Vec2::new([la, lb, lb, la][k], q[k].y)),
                [0, 1, 2, 3].map(|k| (look.side)(q[k].x, q[k].z, c[k])),
            );
        }
    }
}

/// One grating panel centred on the trail at `s`, its top at y = 0 (local x across, z back
/// along the trail): bearing bars across every 33 mm, twisted square cross rods along every
/// 100 mm, banding bars round it and a clip at each corner. The bar tops go into `tops`
/// (the worn tread, mapped across the walkway and along the trail), everything else into
/// `steel`, darkening and rusting down into the pit.
fn grating_panel(tops: &mut Mesh, steel: &mut Mesh, span: f32, len: f32, s: f64, rng: &mut Rand) {
    let tone = rng.range(0.86, 1.04);
    let v_off = rng.range(0.0, 1.92);
    let seed = rng.seed();
    let along = |z: f32| s as f32 - z;
    let top_uv = |x: f32, z: f32| Vec2::new(x + 1.28, along(z) + v_off);
    // Lighter at the top edge and dark toward the pit; the upper edges polished down the
    // middle where feet go; dirt in patches away from it, grime toward the stringers and
    // rust in patches, mostly in the corners.
    let side = |x: f32, z: f32, t: f32| {
        let at = along(z);
        let joint = 1.0 - smooth(0.0, 0.1, len * 0.5 - z.abs());
        let edge = smooth(span * 0.5 - 0.35, span * 0.5, x.abs());
        let band = 1.0
            - smooth(
                0.3,
                0.85,
                x.abs() + 0.12 * fbm_world(x, at, 0.5, 2, seed ^ 0x3),
            );
        let rust = smooth(
            0.3,
            0.7,
            fbm_world(x, at, 0.25, 3, seed) + 0.3 * edge + 0.25 * joint,
        ) * (1.0 - 0.5 * t);
        let dirt = smooth(0.0, 0.5, fbm_world(x, at, 0.45, 3, seed ^ 0x9)) * (1.0 - 0.6 * band);
        let worn = band * t.powi(3);
        let g = ((0.26 + 0.56 * t.powf(0.8)) * (1.0 - 0.35 * edge) * (1.0 - 0.3 * dirt)
            + 0.22 * worn)
            * tone;
        (Vec3::splat(g) * Vec3::new(1.0, 0.97, 0.92).lerp(Vec3::new(1.0, 0.9, 0.78), dirt))
            .lerp(Vec3::new(0.5, 0.29, 0.16) * (0.55 + 0.45 * t), rust * 0.6)
            .extend(1.0)
    };
    let edge = span * 0.5 - 0.005;
    let bars = (len / 0.033).floor() as usize;
    for k in 0..=bars {
        let z = -len * 0.5 + 0.0075 + (len - 0.015) * k as f32 / bars as f32;
        let bt = tone * rng.range(0.93, 1.05);
        let look = BarLook {
            top_uv: &top_uv,
            top: Vec4::new(bt, bt, bt, 1.0),
            side: &side,
        };
        flat_bar(tops, steel, false, z, -edge, edge, 0.004, &look);
    }
    let band = BarLook {
        top_uv: &top_uv,
        top: Vec4::new(tone, tone, tone, 1.0),
        side: &side,
    };
    for z in [-len * 0.5 + 0.0025, len * 0.5 - 0.0025] {
        flat_bar(tops, steel, false, z, -span * 0.5, span * 0.5, 0.005, &band);
    }
    for x in [-span * 0.5 + 0.0025, span * 0.5 - 0.0025] {
        flat_bar(
            tops,
            steel,
            true,
            x,
            -len * 0.5 + 0.005,
            len * 0.5 - 0.005,
            0.005,
            &band,
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
            |t| side(x, -len * 0.5 + len * t, 0.8),
        );
        append(
            steel,
            &rod,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::ZERO,
            Vec4::ONE,
        );
    }
    // Saddle clips bolted down at the corners.
    for (cx, cz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let at = Vec3::new(cx * (span * 0.5 - 0.07), 0.0, cz * (len * 0.5 - 0.06));
        let c = side(at.x, at.z, 0.9) * Vec4::new(0.85, 0.85, 0.85, 1.0);
        let clip = block(Vec3::new(0.036, 0.004, 0.075));
        append(
            steel,
            &clip,
            Quat::IDENTITY,
            Vec3::ONE,
            at + Vec3::Y * 0.002,
            c,
        );
        let head = tube(
            &[at + Vec3::Y * 0.004, at + Vec3::Y * 0.0095],
            6,
            true,
            |_, _| 0.0075,
            |_| c,
        );
        append(
            steel,
            &head,
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::ZERO,
            Vec4::ONE,
        );
    }
}

/// Tubular steel railings on both sides, the posts standing on base plates at `foot`
/// meters up (grimy toward their feet).
fn railings_steel(out: &mut Out, b: &mut Batch, s0: f64, s1: f64, foot: f32) {
    let x = 1.43;
    let top = 1.07;
    let n = ((s1 - s0) / 1.5).round().max(1.0) as usize;
    let white = |_: f32| Vec4::ONE;
    let grimy = |t: f32| {
        let g = 0.72 + 0.28 * smooth(0.0, 0.12, t);
        Vec4::new(g, g * 0.98, g * 0.94, 1.0)
    };
    out.shared.push(("rail-post".into(), {
        let mut m = tube(
            &[Vec3::ZERO, Vec3::Y * (top - foot)],
            16,
            true,
            |_, _| 0.021,
            grimy,
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
                Vec4::new(0.75, 0.72, 0.68, 1.0),
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
                point(s, side * x, foot),
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

/// Shade in the pit under the grating: the renderer would light the pit floor and the
/// bars' lower sides with the whole open sky, but down there they see little more than the
/// bars and the pit walls. Sky light fades out over the bars' depth to a dim floor, so the
/// gaps read as deep and dark. Spawned with or without a renderer.
pub(super) fn grating_volume(world: &mut World, origin: DVec3) {
    let Some(s0) = stretch_start("metal") else {
        return;
    };
    let (_, s1) = range("metal");
    let mid = (s0 + s1) * 0.5;
    let (_, d) = trail_point(mid);
    let half_y = 0.045;
    world.spawn((
        Name::new("Grating pit shade"),
        Transform::from_translation(origin + point(mid, 0.0, PATH_Y - half_y).as_dvec3())
            .with_rotation(crate::demo::yaw(d)),
        engine_render::SkyLightVolume::new([PIT_HALF + 0.01, half_y, (s1 - s0) as f32 * 0.5 - 0.1])
            .open(engine_render::BoxFace::PosY)
            .with_sky_light(0.12)
            .with_fade(0.05),
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
