//! The sport plane's smaller parts: root fillets, wheel fairings, cowl inlets and exhaust
//! stubs, canopy frame, cockpit, nose gear, wheels, propeller, lights and antennas. All in
//! the body frame unless noted.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

use super::sport_plane::*;
use super::surfaces::{Panel, Uv, le_spacing, span_stations, surface_uv};
use super::toolkit::*;

/// Body atlas mapping shared by every part painted into it: v = z over the fuselage
/// length, u in meters over `C_MAX` (see `sport_plane::fuselage_mesh`).
pub const BODY_V: f32 = Z_TAIL - Z_NOSE;
pub const BODY_U: f32 = C_MAX;

// ---------------------------------------------------------------- root fillets

/// Where the right wing's surface (chord fraction `x`) meets the fuselage: span station and
/// body point.
fn wing_meets_body(f: &Fuselage, x: f32, upper: bool) -> (f32, Vec3) {
    let (mut lo, mut hi) = (-0.22f32, 0.6f32);
    for _ in 0..28 {
        let mid = 0.5 * (lo + hi);
        let p = wing_surface(mid, x, upper);
        let side = f.side_at_height(p.z, p.y);
        if p.x < side.x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let s = 0.5 * (lo + hi);
    (s, wing_surface(s, x, upper))
}

/// The fillet between the right wing and the fuselage, over (`upper`) or under the wing.
/// UVs land in the body atlas island starting at `u0`, `v0`.
pub fn root_fillet(f: &Fuselage, upper: bool, u0: f32, v0: f32) -> Mesh {
    let n_chord = 40;
    let n_round = 9;
    let mut rows = Vec::new();
    for k in 0..=n_chord {
        let x = le_spacing(k, n_chord, 1.0).min(0.995);
        let r = if upper {
            0.02 + 0.10 * x.powf(1.4)
        } else {
            0.015 + 0.05 * x
        };
        let (s, corner) = wing_meets_body(f, x, upper);
        let a = wing_surface(s + r, x, upper);
        let dy = if upper { r } else { -r };
        let b = f.side_at_height(corner.z, corner.y + dy);
        let b = Vec3::new(b.x, b.y, corner.z);
        let row: Vec<Vec3> = (0..=n_round)
            .map(|j| {
                let t = j as f32 / n_round as f32;
                // Rational quadratic close to a circular arc.
                let w = 0.75;
                let (b0, b1, b2) = ((1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t) * w, t * t);
                (a * b0 + corner * b1 + b * b2) / (b0 + b1 + b2)
            })
            .collect();
        rows.push(row);
    }
    let z0 = rows[0][0].z;
    let uv = Uv {
        u0,
        u_scale: BODY_U,
        v0: z0 - (v0 * BODY_V),
        v_scale: BODY_V,
    };
    let zs: Vec<f32> = rows.iter().map(|r| r[0].z).collect();
    let mut m = surface_uv(rows, uv, |i| zs[i]);
    // Face away from the corner.
    let mid = &m.vertices[m.vertices.len() / 2];
    let out = if upper {
        Vec3::new(1.0, 1.0, 0.0)
    } else {
        Vec3::new(1.0, -1.0, 0.0)
    };
    if Vec3::from(mid.normal).dot(out) < 0.0 {
        flip_faces(&mut m);
    }
    m
}

// ---------------------------------------------------------------- wheel fairings

/// The main wheel fairing around a wheel centered at `c` (body), its inner skin, and the
/// opening's height. UVs go to the body atlas island at `u0`, `v0`.
pub fn wheel_pant(c: Vec3, radius: f32, u0: f32, v0: f32) -> (Mesh, Mesh) {
    let (front, back) = (0.43, 0.50);
    let hw = Curve::new(&[
        (-front, 0.0),
        (-front + 0.03, 0.055),
        (-front + 0.12, 0.115),
        (-0.10, 0.150),
        (0.05, 0.152),
        (0.25, 0.12),
        (back - 0.05, 0.045),
        (back, 0.0),
    ]);
    let top = Curve::new(&[
        (-front, -0.02),
        (-front + 0.06, 0.08),
        (-0.15, 0.25),
        (0.05, 0.31),
        (0.30, 0.20),
        (back, 0.02),
    ]);
    let bot = Curve::new(&[
        (-front, -0.04),
        (-front + 0.06, -0.12),
        (-0.15, -0.175),
        (0.15, -0.175),
        (back - 0.08, -0.10),
        (back, -0.01),
    ]);
    let point = |dz: f32, phi: f32| -> Vec3 {
        let (s, co) = phi.sin_cos();
        let w = hw.at(dz).max(1e-4);
        let (h, p) = if s >= 0.0 {
            (top.at(dz), 2.4)
        } else {
            (-bot.at(dz), 2.2)
        };
        let e = 2.0 / p;
        c + Vec3::new(
            w * co.signum() * co.abs().powf(e),
            h * s.signum() * s.abs().powf(e),
            dz,
        )
    };
    let n_z = 40;
    let dzs: Vec<f32> = (0..=n_z)
        .map(|k| {
            let t = k as f32 / n_z as f32;
            let t = 0.5 - 0.5 * (PI * t).cos();
            -front + (front + back) * t
        })
        .collect();
    // The opening under the tire spans this angle either side of straight down.
    let slot = 0.40f32;
    let slot_z = radius * 0.93;
    let around = 44;
    let build = |phis: &[f32], rows: &[f32]| -> Mesh {
        let pts: Vec<Vec<Vec3>> = rows
            .iter()
            .map(|&dz| phis.iter().map(|&p| point(dz, p)).collect())
            .collect();
        // U by angle around the whole section (0.28 wide), so the open strip and the shell
        // share one continuous island.
        let mut m = grid(&pts, false, false, |i, j| {
            let t = (phis[j] - (-FRAC_PI_2 + 0.40)) / TAU;
            Vec2::new(u0 + t * 0.28, v0 + (rows[i] + front) / BODY_V)
        });
        // Analytic normals.
        let cols = phis.len();
        for (i, &dz) in rows.iter().enumerate() {
            for (j, &p) in phis.iter().enumerate() {
                let h = 1e-3;
                let n = (point(dz, p + h) - point(dz, p - h))
                    .cross(point(dz + h, p) - point(dz - h, p));
                if n.length_squared() > 1e-16 {
                    m.vertices[i * cols + j].normal = n.normalize().to_array();
                }
            }
        }
        m
    };
    // Main shell: everything but the bottom strip (open where the tire comes out).
    let a0 = -FRAC_PI_2 + slot;
    let a1 = 1.5 * PI - slot;
    let main_phis: Vec<f32> = (0..=around)
        .map(|k| a0 + (a1 - a0) * k as f32 / around as f32)
        .collect();
    let mut shell = build(&main_phis, &dzs);
    let strip_phis: Vec<f32> = (0..=6)
        .map(|k| a1 + (a0 + TAU - a1) * k as f32 / 6.0)
        .collect();
    let ahead: Vec<f32> = dzs
        .iter()
        .copied()
        .filter(|&d| d <= -slot_z + 1e-4)
        .collect();
    let behind: Vec<f32> = dzs
        .iter()
        .copied()
        .filter(|&d| d >= slot_z - 1e-4)
        .collect();
    let mut ahead = ahead;
    ahead.push(-slot_z);
    let mut behind_full = vec![slot_z];
    behind_full.extend(behind);
    shell.merge(&build(&strip_phis, &ahead));
    shell.merge(&build(&strip_phis, &behind_full));
    // Inner skin (only seen through the slot): a coarse copy a little smaller, facing in.
    let coarse_z: Vec<f32> = (0..=14)
        .map(|k| {
            let t = 0.5 - 0.5 * (PI * k as f32 / 14.0).cos();
            -front + (front + back) * t
        })
        .collect();
    let coarse_phi: Vec<f32> = (0..=20).map(|k| a0 + (a1 - a0) * k as f32 / 20.0).collect();
    let mut inner = build(&coarse_phi, &coarse_z);
    for v in &mut inner.vertices {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal);
        v.position = (p - n * 0.004).to_array();
    }
    flip_faces(&mut inner);
    (shell, inner)
}

// ---------------------------------------------------------------- cowl details

/// Point and outward normal on the cowl face in the direction (`x`, `y`) from the thrust line.
pub fn cowl_face(f: &Fuselage, x: f32, y: f32) -> (Vec3, Vec3) {
    let phi = y.atan2(x);
    let rho = (x * x + y * y).sqrt();
    let (mut lo, mut hi) = (Z_NOSE, -2.8f32);
    for _ in 0..30 {
        let mid = 0.5 * (lo + hi);
        let q = f.point(mid, phi, false);
        let r = (q.x * q.x + q.y * q.y).sqrt();
        if r < rho {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let z = 0.5 * (lo + hi);
    let h = 1e-3;
    let p = Vec3::new(x, y, z);
    let n = (f.point(z, phi + h, false) - f.point(z, phi - h, false))
        .cross(f.point(z + h, phi, false) - f.point(z - h, phi, false))
        .normalize();
    (p, n)
}

/// The two round cooling inlets beside the spinner: (center, normal, radius).
pub fn inlets(f: &Fuselage) -> [(Vec3, Vec3, f32); 2] {
    let (p, n) = cowl_face(f, 0.292, 0.085);
    let left = Vec3::new(-p.x, p.y, p.z);
    let nl = Vec3::new(-n.x, n.y, n.z);
    [(p, n, 0.068), (left, nl, 0.068)]
}

/// The chin inlet (oil cooler) below the spinner: center, normal, half size.
pub fn chin_inlet(f: &Fuselage) -> (Vec3, Vec3, Vec2) {
    let (p, n) = cowl_face(f, 0.0, -0.272);
    (p, n, Vec2::new(0.11, 0.032))
}

/// A lip ring (torus) of `tube` radius around a circle of `r` at `c` facing `n`.
pub fn lip_ring(c: Vec3, n: Vec3, r: f32, tube_r: f32, sx: f32, sy: f32) -> Mesh {
    let q = Quat::from_rotation_arc(Vec3::Z, n);
    let path: Vec<Vec3> = (0..=48)
        .map(|k| {
            let a = k as f32 / 48.0 * TAU;
            c + q * Vec3::new(a.cos() * r * sx, a.sin() * r * sy, 0.0) + n * tube_r * 0.3
        })
        .collect();
    tube(&path, |_| tube_r, 10, false)
}

/// Exhaust stubs: two per side under the cowl. Returns the pipes and their dark inner ends.
pub fn exhausts() -> (Mesh, Mesh) {
    let mut pipes = Mesh::default();
    let mut ends = Mesh::default();
    for side in [1.0f32, -1.0] {
        for (k, dz) in [0.0f32, 0.075].into_iter().enumerate() {
            let x = side * (0.20 + 0.035 * k as f32);
            let path = [
                Vec3::new(x * 0.9, -0.32, -2.36 + dz),
                Vec3::new(x * 0.95, -0.39, -2.27 + dz),
                Vec3::new(x, -0.45, -2.16 + dz),
                Vec3::new(x * 1.02, -0.475, -2.08 + dz),
            ];
            let mut p = tube(&path, |_| 0.026, 16, false);
            // Heat tint: straw to blue to dark toward the exit.
            tint(&mut p, |pos, _| {
                let t = ((pos.z - (-2.36 + dz)) / 0.28).clamp(0.0, 1.0);
                let straw = Vec3::new(0.95, 0.80, 0.55);
                let blue = Vec3::new(0.55, 0.55, 0.75);
                let soot = Vec3::new(0.18, 0.16, 0.15);
                let c = if t < 0.5 {
                    straw.lerp(blue, t * 2.0)
                } else {
                    blue.lerp(soot, (t - 0.5) * 2.0)
                };
                c.extend(1.0)
            });
            pipes.merge(&p);
            // A dark disc a little inside the open end.
            let dir = (path[3] - path[2]).normalize();
            let c = path[3] - dir * 0.01;
            let ring: Vec<Vec3> = (0..16)
                .map(|j| {
                    let a = j as f32 / 16.0 * TAU;
                    let q = Quat::from_rotation_arc(Vec3::Z, dir);
                    c + q * Vec3::new(a.cos(), a.sin(), 0.0) * 0.024
                })
                .collect();
            ends.merge(&fan(&ring, dir, |_| Vec2::ZERO));
        }
    }
    (pipes, ends)
}

// ---------------------------------------------------------------- canopy frame

/// A point on the canopy section at station `z`, angle `psi` (0 right sill .. π left sill),
/// pushed out by `out` meters.
fn canopy_point(f: &Fuselage, z: f32, psi: f32, out: f32) -> Vec3 {
    let h = 1e-3;
    let p = f.point(z, psi, true);
    let n = (f.point(z, psi + h, true) - f.point(z, psi - h, true))
        .cross(f.point(z + h, psi, true) - f.point(z - h, psi, true))
        .normalize_or(Vec3::Y);
    p + n * out
}

/// Canopy frame: sill rails, the windshield bow, the base and rear edges, and a latch.
pub fn canopy_frame(f: &Fuselage) -> Mesh {
    let mut m = Mesh::default();
    let arc = |z: f32, out: f32| -> Vec<Vec3> {
        (0..=40)
            .map(|k| canopy_point(f, z, PI * k as f32 / 40.0, out))
            .collect()
    };
    // Bow between windshield and canopy, and the front and rear edges.
    m.merge(&tube(&arc(-1.08, 0.006), |_| 0.017, 10, true));
    m.merge(&tube(&arc(Z_WS + 0.012, 0.002), |_| 0.010, 8, true));
    m.merge(&tube(&arc(Z_CR - 0.012, 0.004), |_| 0.016, 8, true));
    // Sill rails.
    for psi in [0.0f32, PI] {
        let path: Vec<Vec3> = (0..=60)
            .map(|k| {
                let z = Z_WS + (Z_CR - Z_WS) * k as f32 / 60.0;
                canopy_point(f, z, psi + if psi == 0.0 { 0.02 } else { -0.02 }, 0.004)
            })
            .collect();
        m.merge(&tube(&path, |_| 0.019, 10, true));
    }
    // Latch handle on the left rail.
    let a = canopy_point(f, -0.55, PI - 0.08, 0.03);
    let b = canopy_point(f, -0.30, PI - 0.08, 0.03);
    m.merge(&tube(&[a, b], |_| 0.008, 8, true));
    m
}

// ---------------------------------------------------------------- cockpit

/// Inner cockpit shell (the tub below the sill), its coaming and the rear bulkhead, with
/// vertex-color occlusion darkening toward the floor; and the dark instrument panel plate
/// and glareshield.
pub fn cockpit_shell(f: &Fuselage) -> (Mesh, Mesh) {
    let inset = 0.022;
    let zs: Vec<f32> = (0..=46)
        .map(|k| Z_PANEL - 0.04 + (Z_CR - (Z_PANEL - 0.04)) * k as f32 / 46.0)
        .collect();
    let inner = |z: f32, phi: f32| -> Vec3 {
        let h = 1e-3;
        let p = f.point(z, phi, false);
        let n = (f.point(z, phi + h, false) - f.point(z, phi - h, false))
            .cross(f.point(z + h, phi, false) - f.point(z - h, phi, false))
            .normalize_or(Vec3::Y);
        p - n * inset
    };
    let n_around = 40;
    let rows: Vec<Vec<Vec3>> = zs
        .iter()
        .map(|&z| {
            (0..=n_around)
                .map(|k| inner(z, PI + PI * k as f32 / n_around as f32))
                .collect()
        })
        .collect();
    let mut tub = grid(&rows, false, true, |i, j| {
        Vec2::new(j as f32 / n_around as f32, i as f32 / 46.0)
    });
    // Coaming strips on the sills (facing up).
    for phi in [0.0f32, PI] {
        let rows: Vec<Vec<Vec3>> = zs
            .iter()
            .map(|&z| vec![f.point(z, phi, false), inner(z, phi)])
            .collect();
        let mut s = grid(&rows, false, false, |_, _| Vec2::ZERO);
        let up = Vec3::from(s.vertices[0].normal);
        if up.y < 0.0 {
            flip_faces(&mut s);
        }
        tub.merge(&s);
    }
    // Rear bulkhead: the whole section at the canopy's rear edge, facing forward.
    let ring: Vec<Vec3> = (0..64)
        .map(|k| {
            f.point(Z_CR, -FRAC_PI_2 + TAU * k as f32 / 64.0, false) * Vec3::new(0.99, 1.0, 1.0)
        })
        .collect();
    tub.merge(&fan(&ring, Vec3::NEG_Z, |_| Vec2::ZERO));
    // Instrument panel back plate, facing aft.
    let mut outline: Vec<Vec3> = (0..=n_around)
        .map(|k| inner(Z_PANEL, PI + PI * k as f32 / n_around as f32))
        .collect();
    let top_y = 0.372;
    let sill_r = outline.last().copied().unwrap_or(Vec3::ZERO);
    let sill_l = outline[0];
    outline.push(Vec3::new(sill_r.x * 0.93, top_y, Z_PANEL));
    outline.push(Vec3::new(sill_l.x * 0.93, top_y, Z_PANEL));
    for p in &mut outline {
        p.z = Z_PANEL;
    }
    let mut dark = fan(&outline, Vec3::Z, |_| Vec2::ZERO);
    // Glareshield: from the windshield base back over the panel, inside the glass.
    let gz: Vec<f32> = (0..=10)
        .map(|k| Z_WS + 0.012 + (Z_PANEL + 0.07 - Z_WS - 0.012) * k as f32 / 10.0)
        .collect();
    let gy = 0.392;
    let glass_x = |z: f32| -> f32 {
        // Half width of the glass at height gy.
        let (mut lo, mut hi) = (0.0f32, FRAC_PI_2);
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if f.point(z, mid, true).y < gy {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        f.point(z, 0.5 * (lo + hi), true).x
    };
    let rows: Vec<Vec<Vec3>> = gz
        .iter()
        .map(|&z| {
            let w = (glass_x(z) - 0.012).max(0.05);
            (0..=16)
                .map(|k| {
                    let x = -w + 2.0 * w * k as f32 / 16.0;
                    Vec3::new(x, gy + 0.02 * (1.0 - (x / 0.56).powi(2)), z)
                })
                .collect()
        })
        .collect();
    let mut shield = grid(&rows, false, false, |_, _| Vec2::ZERO);
    if Vec3::from(shield.vertices[0].normal).y < 0.0 {
        flip_faces(&mut shield);
    }
    // Lip at the back edge.
    let back = rows.last().cloned().unwrap_or_default();
    let lip_rows = vec![
        back.clone(),
        back.iter().map(|p| *p - Vec3::Y * 0.035).collect(),
    ];
    let mut lip = grid(&lip_rows, false, false, |_, _| Vec2::ZERO);
    if Vec3::from(lip.vertices[0].normal).z < 0.0 {
        flip_faces(&mut lip);
    }
    shield.merge(&lip);
    dark.merge(&shield);
    // Occlusion: darker low and forward in the footwells.
    tint(&mut tub, |p, _| {
        let low = smoothstep(-0.55, 0.25, p.y);
        let fwd = smoothstep(Z_PANEL, Z_PANEL + 0.6, p.z);
        let ao = 0.35 + 0.65 * low * (0.6 + 0.4 * fwd);
        Vec4::new(ao, ao, ao, 1.0)
    });
    (tub, dark)
}

/// Seats with headrests (and the harness, returned separately).
pub fn seats() -> (Mesh, Mesh) {
    let mut seat = Mesh::default();
    let mut belts = Mesh::default();
    for x in [-0.27f32, 0.27] {
        let place = |half: Vec3, r: f32, at: Vec3, tilt: f32| -> Mesh {
            transformed(
                rounded_box(half, r, 5),
                Mat4::from_translation(at) * Mat4::from_rotation_x(tilt),
            )
        };
        let back_tilt = -0.36;
        seat.merge(&place(
            Vec3::new(0.205, 0.05, 0.23),
            0.04,
            Vec3::new(x, -0.25, 0.05),
            -0.12,
        ));
        seat.merge(&place(
            Vec3::new(0.205, 0.33, 0.055),
            0.045,
            Vec3::new(x, 0.05, 0.37),
            back_tilt,
        ));
        seat.merge(&place(
            Vec3::new(0.115, 0.085, 0.05),
            0.04,
            Vec3::new(x, 0.47, 0.52),
            back_tilt,
        ));
        // Side bolsters.
        for dx in [-0.19f32, 0.19] {
            seat.merge(&place(
                Vec3::new(0.035, 0.28, 0.06),
                0.03,
                Vec3::new(x + dx, 0.06, 0.34),
                back_tilt,
            ));
        }
        // Harness: two shoulder straps down the backrest, a lap belt and a buckle.
        let back_rot =
            Mat4::from_translation(Vec3::new(x, 0.05, 0.37)) * Mat4::from_rotation_x(back_tilt);
        for dx in [-0.07f32, 0.07] {
            belts.merge(&transformed(
                rounded_box(Vec3::new(0.022, 0.30, 0.004), 0.003, 3),
                back_rot * Mat4::from_translation(Vec3::new(dx, 0.0, -0.058)),
            ));
        }
        belts.merge(&transformed(
            rounded_box(Vec3::new(0.19, 0.022, 0.004), 0.003, 3),
            Mat4::from_translation(Vec3::new(x, -0.196, -0.05)) * Mat4::from_rotation_x(-1.25),
        ));
    }
    tint(&mut seat, |p, _| {
        let ao = 0.55 + 0.45 * smoothstep(-0.32, 0.1, p.y);
        Vec4::new(ao, ao, ao, 1.0)
    });
    (seat, belts)
}

/// Sticks, throttle quadrant and center console with absolute vertex colors (black plastic,
/// colored knobs); draw with a white material.
pub fn controls() -> Mesh {
    let mut m = Mesh::default();
    for x in [-0.27f32, 0.27] {
        let base = Vec3::new(x, -0.47, -0.62);
        let top = Vec3::new(x, -0.03, -0.66);
        m.merge(&tube(&[base, top], |_| 0.011, 10, false));
        // Grip.
        m.merge(&transformed(
            engine_render::Shape::Capsule {
                radius: 0.019,
                height: 0.13,
                segments: 14,
            }
            .mesh(),
            Mat4::from_translation(top + Vec3::new(0.0, 0.05, -0.008))
                * Mat4::from_rotation_x(-0.15),
        ));
        // Boot.
        m.merge(&transformed(
            lathe(
                &[(0.05, 0.0), (0.045, 0.02), (0.02, 0.07), (0.012, 0.09)],
                14,
                false,
            ),
            Mat4::from_translation(base) * Mat4::from_rotation_x(-FRAC_PI_2),
        ));
    }
    // Center console.
    m.merge(&transformed(
        rounded_box(Vec3::new(0.075, 0.13, 0.30), 0.02, 4),
        Mat4::from_translation(Vec3::new(0.0, -0.36, -0.98)),
    ));
    for v in &mut m.vertices {
        v.color = [0.035, 0.036, 0.038, 1.0];
    }
    // Throttle, propeller and mixture levers with black, blue and red knobs.
    let knobs = [
        Vec3::splat(0.03),
        Vec3::new(0.04, 0.10, 0.55),
        Vec3::new(0.62, 0.04, 0.03),
    ];
    for (k, color) in knobs.into_iter().enumerate() {
        let x = -0.035 + 0.035 * k as f32;
        let a = Vec3::new(x, -0.23, -0.96);
        let b = Vec3::new(x, -0.12, -0.92);
        let lever =
            tube(&[a, b], |_| 0.005, 6, false).with_vertex_color(Vec4::new(0.6, 0.6, 0.62, 1.0));
        m.merge(&lever);
        let knob = transformed(
            engine_render::Shape::Sphere {
                radius: 0.016,
                segments: 12,
            }
            .mesh(),
            Mat4::from_translation(b),
        )
        .with_vertex_color(color.extend(1.0));
        m.merge(&knob);
    }
    m
}

/// Instrument panel furniture: screen bezels, standby gauge cases, the radio stack and the
/// switch row (dark plastic).
pub fn panel_furniture() -> Mesh {
    let mut m = Mesh::default();
    let z = Z_PANEL + 0.012;
    for (x, w) in [(-0.27f32, 0.135f32), (0.06, 0.135)] {
        m.merge(&transformed(
            rounded_box(Vec3::new(w, 0.105, 0.014), 0.008, 3),
            Mat4::from_translation(Vec3::new(x, 0.205, z)),
        ));
    }
    for (x, y) in [(0.34f32, 0.255f32), (0.46, 0.255), (0.40, 0.135)] {
        m.merge(&transformed(
            lathe(
                &[
                    (0.0, 0.0),
                    (0.042, 0.0),
                    (0.044, 0.006),
                    (0.044, 0.028),
                    (0.0, 0.028),
                ],
                20,
                false,
            ),
            Mat4::from_translation(Vec3::new(x, y, z - 0.012)),
        ));
    }
    m.merge(&transformed(
        rounded_box(Vec3::new(0.09, 0.075, 0.03), 0.006, 3),
        Mat4::from_translation(Vec3::new(-0.105, 0.0, z + 0.01)),
    ));
    m
}

/// Lit faces: the two screens, the gauge dials and the radio display, as quads with UVs into
/// the screen texture (left half PFD, right half MFD, bottom strip gauges and radio).
pub fn screens() -> Mesh {
    let z = Z_PANEL + 0.0275;
    let mut m = Mesh::default();
    let quad = |c: Vec3, half: Vec2, uv0: Vec2, uv1: Vec2| -> Mesh {
        let v = |dx: f32, dy: f32, u: f32, w: f32| {
            Vertex::new(c + Vec3::new(dx, dy, 0.0), Vec3::Z, Vec2::new(u, w))
        };
        Mesh::new(
            vec![
                v(-half.x, -half.y, uv0.x, uv1.y),
                v(half.x, -half.y, uv1.x, uv1.y),
                v(half.x, half.y, uv1.x, uv0.y),
                v(-half.x, half.y, uv0.x, uv0.y),
            ],
            vec![0, 1, 2, 0, 2, 3],
        )
    };
    m.merge(&quad(
        Vec3::new(-0.27, 0.205, z),
        Vec2::new(0.122, 0.092),
        Vec2::new(0.0, 0.0),
        Vec2::new(0.5, 0.75),
    ));
    m.merge(&quad(
        Vec3::new(0.06, 0.205, z),
        Vec2::new(0.122, 0.092),
        Vec2::new(0.5, 0.0),
        Vec2::new(1.0, 0.75),
    ));
    for (k, (x, y)) in [(0.34f32, 0.255f32), (0.46, 0.255), (0.40, 0.135)]
        .into_iter()
        .enumerate()
    {
        let u0 = 0.125 * k as f32;
        m.merge(&quad(
            Vec3::new(x, y, z + 0.005),
            Vec2::splat(0.038),
            Vec2::new(u0, 0.75),
            Vec2::new(u0 + 0.125, 1.0),
        ));
    }
    m.merge(&quad(
        Vec3::new(-0.105, 0.0, z + 0.022),
        Vec2::new(0.08, 0.06),
        Vec2::new(0.5, 0.75),
        Vec2::new(0.75, 1.0),
    ));
    m
}

// ---------------------------------------------------------------- gear and wheels

/// A tire with its axis along X, centered on the origin: `radius`, `width`. U around the
/// wheel, V across the section from bead to bead (0.5 on the crown).
pub fn tire(radius: f32, width: f32) -> Mesh {
    let rim = radius * 0.54;
    let cy = 0.5 * (radius + rim);
    let (hy, hx) = (0.5 * (radius - rim), 0.5 * width);
    let n = 36;
    let e = 2.0 / 2.6;
    let sp = |x: f32| x.signum() * x.abs().powf(e);
    // a = 0 on the crown, ±π/2 at the widest sidewall, toward ±π at the bead.
    let reach = 0.80 * PI;
    let profile: Vec<(f32, f32)> = (0..=n)
        .map(|k| {
            let a = -reach + 2.0 * reach * k as f32 / n as f32;
            let r = cy + hy * sp(a.cos());
            let z = hx * sp(a.sin());
            (r.max(rim), z)
        })
        .collect();
    // lathe() revolves about +Z; build there, then turn the axis onto X.
    let m = lathe(&profile, 60, false);
    let mut m = transformed(m, Mat4::from_rotation_y(FRAC_PI_2));
    // The crown (middle of the profile) must face away from the axle.
    let v = &m.vertices[(n / 2) * 61];
    let p = Vec3::from(v.position);
    if Vec3::from(v.normal).dot(Vec3::new(0.0, p.y, p.z)) < 0.0 {
        flip_faces(&mut m);
    }
    m
}

/// A two-piece wheel hub with bolts and a brake disc, axis along X; `outer` = +X side faces out.
pub fn hub(radius: f32, width: f32) -> Mesh {
    let rim = radius * 0.53;
    let hw = width * 0.5;
    let profile = [
        (0.0, -hw * 0.95),
        (rim * 0.35, -hw * 0.95),
        (rim * 0.55, -hw * 0.80),
        (rim * 0.92, -hw * 0.78),
        (rim * 1.02, -hw * 0.84),
        (rim * 1.02, hw * 0.84),
        (rim * 0.92, hw * 0.78),
        (rim * 0.55, hw * 0.80),
        (rim * 0.35, hw * 0.98),
        (rim * 0.18, hw * 1.05),
        (0.0, hw * 1.08),
    ];
    let mut m = transformed(lathe(&profile, 32, false), Mat4::from_rotation_y(FRAC_PI_2));
    let v = &m.vertices[m.vertices.len() / 2];
    if Vec3::from(v.normal).dot(Vec3::from(v.position) * Vec3::new(0.0, 1.0, 1.0)) < 0.0 {
        flip_faces(&mut m);
    }
    // Through-bolts on both faces: they show the wheel turning.
    for k in 0..6 {
        let a = k as f32 / 6.0 * TAU;
        let at = Vec3::new(0.0, a.cos(), a.sin()) * rim * 0.70;
        for side in [-1.0f32, 1.0] {
            m.merge(&transformed(
                engine_render::Shape::Cylinder {
                    radius: rim * 0.07,
                    height: 0.012,
                    segments: 6,
                }
                .mesh(),
                Mat4::from_translation(at + Vec3::X * side * hw * 0.82)
                    * Mat4::from_rotation_z(FRAC_PI_2),
            ));
        }
    }
    m
}

/// Nose gear: strut out of the cowl, fork crown and legs to the axle at `axle` (wheel center
/// at rest), torque link. Steel parts and the painted crown come back separately.
pub fn nose_gear(axle: Vec3, radius: f32, width: f32) -> (Mesh, Mesh) {
    let mut steel = Mesh::default();
    let mut paint = Mesh::default();
    let crown_y = axle.y + radius + 0.035;
    let top = Vec3::new(0.0, -0.33, axle.z + 0.06);
    let crown = Vec3::new(0.0, crown_y, axle.z);
    steel.merge(&tube(
        &[top, crown + Vec3::Y * 0.02],
        |t| 0.028 - 0.006 * t,
        14,
        true,
    ));
    let leg_x = width * 0.5 + 0.03;
    paint.merge(&transformed(
        rounded_box(Vec3::new(leg_x + 0.02, 0.018, 0.035), 0.01, 3),
        Mat4::from_translation(crown),
    ));
    for side in [-1.0f32, 1.0] {
        let a = Vec3::new(side * leg_x, crown_y, axle.z);
        let b = Vec3::new(side * leg_x, axle.y - 0.015, axle.z);
        paint.merge(&tube(&[a, b], |_| 0.016, 10, true));
    }
    // Torque link on the strut's front.
    let l0 = Vec3::new(0.0, -0.37, axle.z - 0.0);
    let l1 = Vec3::new(0.0, crown_y + 0.02, axle.z - 0.05);
    steel.merge(&tube(&[l0, l1], |_| 0.008, 6, true));
    (steel, paint)
}

// ---------------------------------------------------------------- propeller

/// Spinner around the prop hub (prop frame: −Z forward from the prop disc).
pub fn spinner() -> Mesh {
    let n = 22;
    let len = 0.38;
    let r0 = 0.203;
    let mut profile: Vec<(f32, f32)> = (0..=n)
        .map(|k| {
            let t = 1.0 - k as f32 / n as f32;
            // Ogive: a rounded point, full radius at the back plate.
            let r = r0 * (1.0 - t.powf(1.7)).max(0.0).powf(0.68);
            (r, -len * t)
        })
        .collect();
    profile.push((r0, 0.03));
    profile.push((r0 * 0.98, 0.06));
    profile.push((0.0, 0.06));
    lathe(&profile, 48, false)
}

/// Three twisted, tapered blades in the prop frame (spin about Z). Colors: black blades,
/// a nickel leading-edge strip and yellow tips (vertex color).
pub fn blades() -> Mesh {
    let mut m = Mesh::default();
    let chord = Curve::new(&[
        (0.10, 0.060),
        (0.18, 0.090),
        (0.30, 0.122),
        (0.45, 0.128),
        (0.62, 0.112),
        (0.72, 0.090),
        (PROP_RADIUS, 0.060),
    ]);
    let thick = Curve::new(&[
        (0.10, 0.45),
        (0.17, 0.22),
        (0.30, 0.13),
        (0.55, 0.09),
        (PROP_RADIUS, 0.06),
    ]);
    let pitch = 1.65;
    let rs: Vec<f32> = (0..=34)
        .map(|k| 0.10 + (PROP_RADIUS - 0.10) * (k as f32 / 34.0).powf(0.85))
        .collect();
    let n = 14;
    for b in 0..3 {
        let th = b as f32 / 3.0 * TAU;
        let er = Vec3::new(th.cos(), th.sin(), 0.0);
        let et = Vec3::new(-th.sin(), th.cos(), 0.0);
        let rows: Vec<Vec<Vec3>> = rs
            .iter()
            .map(|&r| {
                let tip = ((r - 0.74) / (PROP_RADIUS - 0.74)).clamp(0.0, 1.0);
                let round = (1.0 - tip * tip).max(0.0).sqrt().max(0.12);
                let c = chord.at(r) * round;
                let beta = (pitch / (TAU * r)).atan();
                let d = et * beta.cos() - Vec3::Z * beta.sin();
                let ns = -et * beta.sin() - Vec3::Z * beta.cos();
                let t = thick.at(r);
                let camber = 0.035 * smoothstep(0.12, 0.25, r);
                let pt = |x: f32, upper: bool| {
                    let (_, yu, yl) = naca_at(x, t, camber, 0.4, 0.01);
                    let y = if upper { yu } else { yl } * round.max(0.3);
                    er * r + d * (0.35 - x) * c + ns * y * c
                };
                let mut row: Vec<Vec3> = (0..=n)
                    .rev()
                    .map(|k| pt(le_spacing(k, n, 1.0), false))
                    .collect();
                row.extend((1..=n).map(|k| pt(le_spacing(k, n, 1.0), true)));
                row
            })
            .collect();
        let mut blade = grid(&rows, false, false, |i, j| {
            Vec2::new(j as f32 / (2 * n) as f32, i as f32 / 34.0)
        });
        // Orient outward (away from the blade's pitch axis).
        let mid = &blade.vertices[(blade.vertices.len() / 2) + n / 2];
        let p = Vec3::from(mid.position);
        let axis_pt = er * p.dot(er);
        if Vec3::from(mid.normal).dot(p - axis_pt) < 0.0 {
            flip_faces(&mut blade);
        }
        tint(&mut blade, |p, _| {
            let r = p.truncate().length();
            let base = Vec3::splat(0.035);
            let tip = if r > PROP_RADIUS - 0.075 {
                Vec3::new(0.95, 0.72, 0.05)
            } else if r > PROP_RADIUS - 0.09 {
                Vec3::splat(0.9)
            } else {
                base
            };
            tip.extend(1.0)
        });
        m.merge(&blade);
    }
    // Leading-edge strips: brighten vertices at the leading edge (U near the middle).
    for v in &mut m.vertices {
        let u = v.uv[0];
        let r = Vec2::new(v.position[0], v.position[1]).length();
        if (u - 0.5).abs() < 0.07 && r > 0.25 && r < PROP_RADIUS - 0.09 {
            v.color = [0.55, 0.56, 0.58, 1.0];
        }
    }
    m
}

/// The spinning propeller as the eye sees it: a faint disc in the prop frame (about Z), as
/// dense at each radius as the blades cover it, dark with a yellow ring where the tips run.
/// Drawn alpha-blended in place of the blades at speed.
pub fn prop_disc() -> Mesh {
    let mut m = Mesh::default();
    let (segs, rings) = (64u32, 24u32);
    let inner = 0.19;
    for i in 0..=rings {
        let r = inner + (PROP_RADIUS - inner) * i as f32 / rings as f32;
        // Fraction of the circle the three blades sweep through at this radius.
        let chord = if r < 0.3 {
            0.09
        } else if r < 0.62 {
            0.12
        } else {
            0.08
        };
        let covered = (3.0 * chord / (TAU * r)).min(1.0);
        let tip = r > PROP_RADIUS - 0.08;
        let (rgb, a) = if tip {
            ([0.95, 0.72, 0.05], 0.32)
        } else {
            ([0.035; 3], (covered * 1.8).clamp(0.09, 0.4))
        };
        // Fade the outer edge so the disc has no hard rim.
        let a = if i == rings { 0.0 } else { a };
        for j in 0..=segs {
            let t = j as f32 / segs as f32 * TAU;
            let mut v = Vertex::new(
                Vec3::new(t.cos() * r, t.sin() * r, 0.0),
                Vec3::NEG_Z,
                Vec2::new(j as f32 / segs as f32, i as f32 / rings as f32),
            );
            v.color = [rgb[0], rgb[1], rgb[2], a];
            m.vertices.push(v);
        }
    }
    let row = segs + 1;
    for i in 0..rings {
        for j in 0..segs {
            let k = i * row + j;
            m.indices
                .extend_from_slice(&[k, k + row, k + 1, k + 1, k + row, k + row + 1]);
        }
    }
    m
}

// ---------------------------------------------------------------- lights and antennas

/// A lens: an ellipsoid with half sizes `half`, rotated by `rot`, at `c`.
pub fn lens(c: Vec3, half: Vec3, rot: Quat) -> Mesh {
    transformed(
        engine_render::Shape::Sphere {
            radius: 1.0,
            segments: 20,
        }
        .mesh(),
        Mat4::from_translation(c) * Mat4::from_quat(rot) * Mat4::from_scale(half),
    )
}

/// Wingtip light positions (right wing, body frame): navigation lens near the leading edge,
/// strobe behind it and the white position light at the trailing edge.
pub fn wingtip_lights() -> (Vec3, Vec3, Vec3) {
    let p = wing_panel();
    let s = WING_TIP + 0.03;
    let le = (p.le)(WING_TIP);
    let c = p.chord(WING_TIP);
    let m = wing_to_body(1.0);
    (
        m.transform_point3(Vec3::new(s, 0.004, le + 0.10)),
        m.transform_point3(Vec3::new(s, 0.006, le + 0.25)),
        m.transform_point3(Vec3::new(s - 0.01, 0.0, le + 0.93 * c)),
    )
}

/// A swept blade antenna standing on a surface at `base` (along +z its chord).
pub fn blade_antenna(base: Vec3, height: f32, chord: f32, up: Vec3) -> Mesh {
    let p = Panel {
        le: |s| 0.55 * s,
        te: |s| 1.0 - 0.15 * s,
        thick: |_| 0.12,
        camber: 0.0,
        hinge: |_| 0.9,
        gap: 0.0,
    };
    let ss = span_stations(0.0, 1.0, 0.1, &[]);
    let unit = super::surfaces::skin(
        &p,
        &ss,
        true,
        10,
        Uv {
            u0: 0.0,
            u_scale: 1.0,
            v0: 0.0,
            v_scale: 1.0,
        },
    );
    // Panel frame: x = span (up), z = chord; scale into place.
    let q = Quat::from_rotation_arc(Vec3::Y, up);
    transformed(
        unit,
        Mat4::from_translation(base)
            * Mat4::from_quat(q)
            * Mat4::from_rotation_z(FRAC_PI_2)
            * Mat4::from_scale(Vec3::new(height, chord, chord)),
    )
}

/// Pitot tube under the left wing, antennas on the spine and belly, GPS puck.
pub fn small_parts(f: &Fuselage) -> (Mesh, Mesh) {
    let mut metal = Mesh::default();
    let mut black = Mesh::default();
    // Pitot: mast down from the left wing's lower surface, tube forward of the leading edge.
    let s = 3.0;
    let root = wing_surface(s, 0.22, false) * Vec3::new(-1.0, 1.0, 1.0);
    let elbow = root + Vec3::new(0.0, -0.11, -0.04);
    let tip = Vec3::new(elbow.x, elbow.y, wing_surface(s, 0.0, false).z - 0.30);
    metal.merge(&tube(&[root + Vec3::Y * 0.02, elbow], |_| 0.009, 8, false));
    metal.merge(&tube(
        &[elbow, Vec3::new(elbow.x, elbow.y, elbow.z - 0.02), tip],
        |t| 0.0075 - 0.002 * t.powi(4),
        10,
        true,
    ));
    // COM blade on the spine, VHF whip and transponder stub underneath, GPS puck.
    let spine = |z: f32| Vec3::new(0.0, f.top_at(z), z);
    black.merge(&blade_antenna(
        spine(2.05) - Vec3::Y * 0.01,
        0.23,
        0.17,
        Vec3::new(0.0, 1.0, 0.0),
    ));
    black.merge(&transformed(
        rounded_box(Vec3::new(0.05, 0.014, 0.065), 0.012, 4),
        Mat4::from_translation(spine(1.62) + Vec3::Y * 0.004),
    ));
    let belly = |z: f32| Vec3::new(0.0, f.bottom_at(z), z);
    let w0 = belly(1.85) + Vec3::Y * 0.01;
    black.merge(&tube(
        &[w0, w0 + Vec3::new(0.0, -0.30, 0.16)],
        |t| 0.006 - 0.003 * t,
        6,
        true,
    ));
    let t0 = belly(0.95);
    metal.merge(&tube(
        &[t0 + Vec3::Y * 0.01, t0 - Vec3::Y * 0.07],
        |_| 0.005,
        6,
        false,
    ));
    metal.merge(&transformed(
        engine_render::Shape::Sphere {
            radius: 0.011,
            segments: 8,
        }
        .mesh(),
        Mat4::from_translation(t0 - Vec3::Y * 0.075),
    ));
    // Tail tie-down ring under the tail cone.
    let tie = belly(3.9);
    metal.merge(&lip_ring(
        tie - Vec3::Y * 0.03,
        Vec3::X,
        0.025,
        0.005,
        1.0,
        1.0,
    ));
    (metal, black)
}

/// Static wicks: thin rods off a trailing edge at `at` pointing aft-ish, length 0.11.
pub fn wick(at: Vec3, dir: Vec3) -> Mesh {
    tube(
        &[at - dir * 0.01, at + dir * 0.11],
        |t| 0.0032 - 0.0015 * t,
        5,
        true,
    )
}
