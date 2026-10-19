//! The spaceplane's parts outside the textured skins: main engine bells, hover engine wells,
//! RCS ports, landing-gear tires, window frames, the docking port and the crew hatch (the
//! folding legs and doors are in [`super::ship_gear`]).
//! All in the body frame unless noted.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use engine_render::Mesh;

use super::spaceplane::*;
use super::toolkit::*;

// ---------------------------------------------------------------- main engines

/// Bell geometry: throat and exit stations (z, radius), relative to the engine's thruster
/// position (z grows aft).
pub const BELL_THROAT: (f32, f32) = (-0.15, 0.27);
pub const BELL_EXIT: (f32, f32) = (1.15, 0.80);

fn bell_radius(t: f32) -> f32 {
    BELL_THROAT.1 + (BELL_EXIT.1 - BELL_THROAT.1) * (1.0 - (1.0 - t).powf(1.9))
}

/// One main engine around `c` (its thruster position), pointing aft: the outer bell (u
/// around, v along, for the cooling-tube texture), and the dark inside with the throat.
pub fn main_engine(c: Vec3) -> (Mesh, Mesh, Mesh) {
    let n = 28;
    let (zt, rt) = BELL_THROAT;
    let (ze, _) = BELL_EXIT;
    // Outer: chamber, throat, bell, rolled lip.
    let mut outer: Vec<(f32, f32)> = vec![
        (0.0, -0.62),
        (0.40, -0.62),
        (0.40, -0.55),
        (0.36, -0.50),
        (0.36, -0.30),
        (0.31, zt - 0.03),
    ];
    for k in 0..=n {
        let t = k as f32 / n as f32;
        outer.push((bell_radius(t) + 0.022, zt + (ze - zt) * t));
    }
    outer.push((BELL_EXIT.1 + 0.012, ze + 0.012));
    outer.push((BELL_EXIT.1 - 0.004, ze + 0.014));
    let mut bell = lathe(&outer, 56, false);
    // Heat tint toward the exit: straw then blue-grey.
    tint(&mut bell, |p, _| {
        let t = ((p.z - zt) / (ze - zt)).clamp(0.0, 1.0);
        let straw = Vec3::new(1.0, 0.93, 0.78);
        let blue = Vec3::new(0.78, 0.82, 0.95);
        let c = if t < 0.6 {
            Vec3::ONE.lerp(straw, t / 0.6)
        } else {
            straw.lerp(blue, (t - 0.6) / 0.4)
        };
        c.extend(1.0)
    });
    // Inside: inward faces from the throat to the lip.
    let inner_profile: Vec<(f32, f32)> = (0..=n)
        .map(|k| {
            let t = k as f32 / n as f32;
            (bell_radius(t), zt + (ze - zt) * t)
        })
        .chain(std::iter::once((BELL_EXIT.1 - 0.004, ze + 0.014)))
        .collect();
    let mut inside = lathe(&inner_profile, 48, true);
    tint(&mut inside, |p, _| {
        let t = ((p.z - zt) / (ze - zt)).clamp(0.0, 1.0);
        let deep = Vec3::new(0.25, 0.17, 0.10);
        let c = deep.lerp(Vec3::ONE, t.powf(0.6));
        c.extend(1.0)
    });
    // Throat plug (dark) and the injector face.
    let ring: Vec<Vec3> = (0..24)
        .map(|k| {
            let a = k as f32 / 24.0 * TAU;
            Vec3::new(a.cos() * rt, a.sin() * rt, zt + 0.01)
        })
        .collect();
    inside.merge(&fan(&ring, Vec3::Z, |_| Vec2::ZERO));
    // Hardware: manifold rings, gimbal actuators, the flexible boot at the base.
    let mut hw = Mesh::default();
    hw.merge(&ring_at(zt + 0.06, 0.345, 0.045));
    hw.merge(&ring_at(zt + 0.52, bell_radius(0.40) + 0.03, 0.026));
    hw.merge(&ring_at(zt + 1.0, bell_radius(0.77) + 0.028, 0.018));
    for a in [0.0f32, FRAC_PI_2] {
        let d = Vec3::new(a.cos(), a.sin(), 0.0);
        let p0 = d * 0.62 + Vec3::Z * (Z_BASE - c.z + 0.02);
        let p1 = d * 0.42 + Vec3::Z * (zt + 0.06);
        hw.merge(&tube(&[p0, p1], |t| 0.045 - 0.012 * t, 10, true));
        hw.merge(&transformed(
            engine_render::Shape::Sphere {
                radius: 0.05,
                segments: 10,
            }
            .mesh(),
            Mat4::from_translation(p1),
        ));
    }
    let boot: Vec<(f32, f32)> = (0..=10)
        .map(|k| {
            let t = k as f32 / 10.0;
            let r = 0.62 - 0.24 * t + 0.015 * (t * PI * 5.0).sin();
            (r, (Z_BASE - c.z) - 0.02 + 0.16 * t)
        })
        .collect();
    let mut boot_mesh = lathe(&boot, 40, false);
    tint(&mut boot_mesh, |_, _| Vec4::new(0.30, 0.29, 0.27, 1.0));
    hw.merge(&boot_mesh);
    let place = Mat4::from_translation(c);
    (
        transformed(bell, place),
        transformed(inside, place),
        transformed(hw, place),
    )
}

fn ring_at(z: f32, r: f32, tube_r: f32) -> Mesh {
    let path: Vec<Vec3> = (0..=56)
        .map(|k| {
            let a = k as f32 / 56.0 * TAU;
            Vec3::new(a.cos() * r, a.sin() * r, z)
        })
        .collect();
    tube(&path, |_| tube_r, 8, false)
}

// ---------------------------------------------------------------- hover engines

/// Radius of a hover engine's opening in the belly, and of its flange.
pub const HOVER_R: f32 = 0.45;
pub const HOVER_FLANGE: f32 = 0.60;

/// A point on the belly under (x, z), lifted `lift` along the normal: point and normal.
fn on_belly(h: &Hull, x: f32, z: f32, lift: f32) -> (Vec3, Vec3) {
    let (p, n) = h.exit(Vec3::new(x, -1.0, z), Vec3::NEG_Y);
    (p + n * lift, n)
}

/// A hover engine well at (`x`, `z`): the flange hugging the belly, the well wall, louvres
/// across the opening and the nozzle at the top of the well. Returns (metal, dark).
pub fn hover_engine(h: &Hull, x: f32, z: f32) -> (Mesh, Mesh) {
    let n = 48;
    let mut metal = Mesh::default();
    let mut dark = Mesh::default();
    // Flange: an annulus hugging the belly.
    let rows: Vec<Vec<Vec3>> = [HOVER_R - 0.02, HOVER_R + 0.06, HOVER_FLANGE]
        .iter()
        .map(|&r| {
            (0..=n)
                .map(|k| {
                    let a = k as f32 / n as f32 * TAU;
                    on_belly(h, x + a.cos() * r, z + a.sin() * r, 0.006).0
                })
                .collect()
        })
        .collect();
    let mut flange = grid(&rows, false, false, |i, j| {
        Vec2::new(j as f32 / n as f32, i as f32 * 0.5)
    });
    if Vec3::from(flange.vertices[0].normal).y > 0.0 {
        flip_faces(&mut flange);
    }
    metal.merge(&flange);
    let (c, nrm) = on_belly(h, x, z, 0.0);
    let up = -nrm;
    let q = Quat::from_rotation_arc(Vec3::Y, up);
    // Well wall (facing in), from the opening up into the hull.
    let depth = 0.55;
    let wall_rows: Vec<Vec<Vec3>> = [0.0f32, depth]
        .iter()
        .map(|&d| {
            (0..=n)
                .map(|k| {
                    let a = k as f32 / n as f32 * TAU;
                    let p = on_belly(h, x + a.cos() * HOVER_R, z + a.sin() * HOVER_R, 0.0).0;
                    p + up * (d + 0.01)
                })
                .collect()
        })
        .collect();
    let mut wall = grid(&wall_rows, false, false, |_, _| Vec2::ZERO);
    let probe = &wall.vertices[n / 4];
    let toward_axis = c - Vec3::from(probe.position);
    if Vec3::from(probe.normal).dot(toward_axis) < 0.0 {
        flip_faces(&mut wall);
    }
    tint(&mut wall, |p, _| {
        let d = (p - c).dot(up) / depth;
        let k = 1.0 - 0.75 * d.clamp(0.0, 1.0);
        Vec4::new(k, k, k, 1.0)
    });
    dark.merge(&wall);
    // Nozzle at the top of the well, pointing down.
    let nozzle_profile: Vec<(f32, f32)> = (0..=8)
        .map(|k| {
            let t = k as f32 / 8.0;
            (0.12 + 0.24 * (1.0 - (1.0 - t).powf(1.8)), 0.30 * t)
        })
        .collect();
    let mut nozzle = lathe(&nozzle_profile, 32, false);
    nozzle.merge(&lathe(&nozzle_profile, 32, true));
    let top = c + up * depth;
    let place = Mat4::from_translation(top) * Mat4::from_quat(q) * Mat4::from_rotation_x(FRAC_PI_2);
    dark.merge(&transformed(nozzle, place));
    // Ceiling of the well.
    let ceiling: Vec<Vec3> = (0..n)
        .map(|k| {
            let a = k as f32 / n as f32 * TAU;
            top + q * Vec3::new(a.cos() * HOVER_R, 0.0, a.sin() * HOVER_R)
        })
        .collect();
    let mut ceil = fan(&ceiling, -up, |_| Vec2::ZERO);
    tint(&mut ceil, |_, _| Vec4::new(0.2, 0.2, 0.2, 1.0));
    dark.merge(&ceil);
    // Louvres: slats along z, tilted, spanning the opening.
    let slats = 7;
    for k in 0..slats {
        let off = -HOVER_R + (k as f32 + 0.5) * 2.0 * HOVER_R / slats as f32;
        let half_len = (HOVER_R * HOVER_R - off * off).max(0.0).sqrt();
        if half_len < 0.05 {
            continue;
        }
        let center = c + q * Vec3::new(off, 0.07, 0.0);
        let slat = transformed(
            rounded_box(Vec3::new(0.008, 0.06, half_len), 0.004, 2),
            Mat4::from_translation(center) * Mat4::from_quat(q) * Mat4::from_rotation_z(0.45),
        );
        metal.merge(&slat);
    }
    (metal, dark)
}

// ---------------------------------------------------------------- RCS ports

/// A quad of small RCS nozzles set into a surface at `p` facing `n`. Returns (nozzle
/// bodies, dark insides).
pub fn rcs_quad(p: Vec3, n: Vec3) -> (Mesh, Mesh) {
    let q = Quat::from_rotation_arc(Vec3::Z, n);
    let mut body = Mesh::default();
    let mut inside = Mesh::default();
    for (dx, dy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let c = p + q * Vec3::new(dx * 0.048, dy * 0.048, 0.0);
        let place = Mat4::from_translation(c) * Mat4::from_quat(q);
        // A short cup: rim proud of the surface, throat 6 cm down.
        let prof = [
            (0.020, -0.06),
            (0.031, -0.01),
            (0.036, 0.004),
            (0.038, 0.008),
        ];
        body.merge(&transformed(lathe(&prof, 16, false), place));
        let mut cup = lathe(&prof, 16, true);
        let disc: Vec<Vec3> = (0..12)
            .map(|k| {
                let a = k as f32 / 12.0 * TAU;
                Vec3::new(a.cos() * 0.02, a.sin() * 0.02, -0.06)
            })
            .collect();
        cup.merge(&fan(&disc, Vec3::Z, |_| Vec2::ZERO));
        inside.merge(&transformed(cup, place));
    }
    (body, inside)
}

/// The OMS pod's top surface above (x, z): point and normal (body frame, right pod).
pub fn oms_top(x: f32, z: f32) -> (Vec3, Vec3) {
    let r = oms_radius().at(z).max(1e-3);
    let half = OMS_HALF * r;
    let p = 2.4f32;
    let dx = ((x.abs() - OMS_CENTER.x) / half.x).clamp(-0.999, 0.999);
    let fy = (1.0 - dx.abs().powf(p)).max(0.0).powf(1.0 / p);
    let y = OMS_CENTER.y + half.y * fy;
    // Normal of the superellipse x^p/a^p + y^p/b^p = 1.
    let gx = dx.signum() * dx.abs().powf(p - 1.0) / half.x;
    let gy = fy.powf(p - 1.0) / half.y;
    let n = Vec3::new(gx * x.signum(), gy, 0.0).normalize_or(Vec3::Y);
    (Vec3::new(x, y, z), n)
}

// ---------------------------------------------------------------- landing gear

/// A twin-tire wheel set's spinning parts (two tires, hubs and the axle tube between), axis
/// along X, centered on the axle.
pub fn twin_tires(radius: f32, width: f32, gap: f32) -> (Mesh, Mesh) {
    let off = 0.5 * (width + gap);
    let t = super::plane_parts::tire(radius, width);
    let hb = super::plane_parts::hub(radius, width);
    let tires = merged([
        transformed(t.clone(), Mat4::from_translation(Vec3::X * off)),
        transformed(t, Mat4::from_translation(Vec3::NEG_X * off)),
    ]);
    let mut hubs = merged([
        transformed(hb.clone(), Mat4::from_translation(Vec3::X * off)),
        transformed(
            hb,
            Mat4::from_translation(Vec3::NEG_X * off) * Mat4::from_rotation_y(PI),
        ),
    ]);
    // Axle tube.
    hubs.merge(&transformed(
        engine_render::Shape::Cylinder {
            radius: 0.07,
            height: gap + width,
            segments: 20,
        }
        .mesh(),
        Mat4::from_rotation_z(FRAC_PI_2),
    ));
    (tires, hubs)
}

// ---------------------------------------------------------------- windows, hatch, dock

/// The cockpit windows as rounded rectangles in (arc from the top center, z): center,
/// half size, corner radius. Mirrored to both sides.
pub const WINDOWS: &[(Vec2, Vec2, f32)] = &[
    (Vec2::new(0.37, -7.24), Vec2::new(0.29, 0.38), 0.07),
    (Vec2::new(1.02, -7.15), Vec2::new(0.28, 0.35), 0.07),
    (Vec2::new(1.64, -6.98), Vec2::new(0.26, 0.32), 0.07),
    (Vec2::new(0.37, -6.0), Vec2::new(0.25, 0.25), 0.06),
];

/// Hull point at arc `a` from the top center (right side) and station `z`, lifted `lift`.
pub fn hull_at_arc_from_top(h: &Hull, a: f32, z: f32, lift: f32) -> Vec3 {
    let arc = h.half_perimeter(z) - a;
    let phi = h.phi_at_arc(z, arc);
    h.point(z, phi) + h.normal(z, phi) * lift
}

/// Window frames: a raised rim around every window, both sides.
pub fn window_frames(h: &Hull) -> Mesh {
    let mut m = Mesh::default();
    for &(c, half, r) in WINDOWS {
        let n = 40;
        let path: Vec<Vec3> = (0..=n)
            .map(|k| {
                let t = k as f32 / n as f32 * TAU;
                let q = rounded_rect_point(half + Vec2::splat(0.025), r + 0.025, t);
                hull_at_arc_from_top(h, c.x + q.x, c.y + q.y, 0.008)
            })
            .collect();
        let rim = tube(&path, |_| 0.02, 8, false);
        m.merge(&mirrored_x(&rim));
        m.merge(&rim);
    }
    m
}

/// A point on a rounded rectangle's outline at parameter `t` (0..2π), by angle from its center.
fn rounded_rect_point(half: Vec2, r: f32, t: f32) -> Vec2 {
    let dir = Vec2::new(t.cos(), t.sin());
    // March out along the ray to the outline (the SDF is exact enough for this).
    let mut d = 0.0;
    for _ in 0..40 {
        let s = rounded_rect(dir * d, half, r);
        if s.abs() < 1e-5 {
            break;
        }
        d += s;
    }
    dir * d
}

/// Crew hatch hardware on the left side (around [`HATCH`]): hinge block and handle.
pub fn hatch_hardware(h: &Hull) -> Mesh {
    let mut m = Mesh::default();
    let z = HATCH.y;
    let arc_top = HATCH.x;
    let c = hull_at_arc_from_top(h, arc_top, z, 0.0);
    let n = {
        let a = h.half_perimeter(z) - arc_top;
        h.normal(z, h.phi_at_arc(z, a))
    };
    let fwd = hull_at_arc_from_top(h, arc_top, z - HATCH_R - 0.06, 0.03);
    m.merge(&transformed(
        rounded_box(Vec3::new(0.03, 0.12, 0.05), 0.01, 2),
        Mat4::from_translation(fwd) * Mat4::from_quat(Quat::from_rotation_arc(Vec3::X, n)),
    ));
    let h0 = c + n * 0.03 + Vec3::new(0.0, -0.08, 0.20);
    let h1 = c + n * 0.03 + Vec3::new(0.0, 0.08, 0.20);
    m.merge(&tube(
        &[h0 - n * 0.03, h0, h1, h1 - n * 0.03],
        |_| 0.012,
        8,
        true,
    ));
    mirrored_x(&m)
}

/// Hatch center in (arc from the top center, z) on the left side, and radius.
pub const HATCH: Vec2 = Vec2::new(2.55, -5.2);
pub const HATCH_R: f32 = 0.48;

/// A flat plate from a convex outline (x, y), `thickness` along z, centered on z = 0.
fn plate(outline: &[Vec2], thickness: f32) -> Mesh {
    let t = thickness * 0.5;
    let at = |p: Vec2, z: f32| Vec3::new(p.x, p.y, z);
    let front: Vec<Vec3> = outline.iter().map(|&p| at(p, t)).collect();
    let back: Vec<Vec3> = outline.iter().map(|&p| at(p, -t)).collect();
    let mut m = fan(&front, Vec3::Z, |_| Vec2::ZERO);
    m.merge(&fan(&back, Vec3::NEG_Z, |_| Vec2::ZERO));
    let c = outline.iter().copied().sum::<Vec2>() / outline.len() as f32;
    for k in 0..outline.len() {
        let (a, b) = (outline[k], outline[(k + 1) % outline.len()]);
        let e = b - a;
        let mut n = Vec3::new(e.y, -e.x, 0.0).normalize_or(Vec3::X);
        if n.dot(at((a + b) * 0.5 - c, 0.0)) < 0.0 {
            n = -n;
        }
        m.merge(&fan(&[at(a, t), at(b, t), at(b, -t), at(a, -t)], n, |_| {
            Vec2::ZERO
        }));
    }
    m
}

/// The docking port on the spine behind the cockpit: the collar and guide petals (painted
/// grey), the recessed hatch (dark) and the latches and handle (bare metal).
pub const DOCK_Z: f32 = -2.6;
pub fn docking_port(h: &Hull) -> (Mesh, Mesh, Mesh) {
    let y = h.top_at(DOCK_Z);
    let base = Vec3::new(0.0, y - 0.03, DOCK_Z);
    let up = |m: Mesh| {
        transformed(
            m,
            Mat4::from_translation(base) * Mat4::from_rotation_x(-FRAC_PI_2),
        )
    };
    let prof = [
        (0.70, 0.0),
        (0.70, 0.06),
        (0.62, 0.08),
        (0.62, 0.24),
        (0.66, 0.26),
        (0.66, 0.30),
        (0.55, 0.31),
        (0.52, 0.28),
        (0.50, 0.22),
    ];
    let mut light = up(lathe(&prof, 64, false));
    // Guide petals leaning in over the hatch.
    let outline = [
        Vec2::new(-0.2, 0.0),
        Vec2::new(0.2, 0.0),
        Vec2::new(0.07, 0.3),
        Vec2::new(-0.07, 0.3),
    ];
    let petal = plate(&outline, 0.03);
    for k in 0..3 {
        let a = k as f32 / 3.0 * TAU + 0.5;
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        light.merge(&transformed(
            petal.clone(),
            Mat4::from_translation(base + d * 0.6 + Vec3::Y * 0.29)
                * Mat4::from_quat(Quat::from_rotation_arc(Vec3::Z, d))
                * Mat4::from_rotation_x(-0.32),
        ));
    }
    // Recessed hatch with a rim.
    let ring: Vec<Vec3> = (0..48)
        .map(|k| {
            let a = k as f32 / 48.0 * TAU;
            Vec3::new(a.cos() * 0.505, a.sin() * 0.505, 0.225)
        })
        .collect();
    let mut dark = up(fan(&ring, Vec3::Z, |_| Vec2::ZERO));
    dark.merge(&up(lathe(
        &[(0.36, 0.224), (0.37, 0.236), (0.38, 0.224)],
        48,
        false,
    )));
    // Latches around the rim and the hatch handle.
    let mut metal = Mesh::default();
    for k in 0..12 {
        let a = k as f32 / 12.0 * TAU;
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        metal.merge(&transformed(
            rounded_box(Vec3::new(0.035, 0.025, 0.02), 0.006, 2),
            Mat4::from_translation(base + d * 0.6 + Vec3::Y * 0.31) * Mat4::from_rotation_y(-a),
        ));
    }
    let hy = base.y + 0.235;
    metal.merge(&tube(
        &[
            Vec3::new(-0.12, hy, DOCK_Z),
            Vec3::new(-0.10, hy + 0.04, DOCK_Z),
            Vec3::new(0.10, hy + 0.04, DOCK_Z),
            Vec3::new(0.12, hy, DOCK_Z),
        ],
        |_| 0.012,
        8,
        true,
    ));
    (light, dark, metal)
}
