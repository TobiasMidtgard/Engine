//! The spaceplane: a double-delta orbiter (Shuttle-like proportions on the simulated 21.6 m
//! hull and 14.6 m span) with a tiled belly, quilted blankets on top, a cockpit window band,
//! OMS pods, three main engines, four hover engines in the belly, RCS ports, elevons, a split
//! body flap, fin and rudder, and twin-tire gear.
//!
//! Body frame: x right, y up, −z forward, meters, origin at the hull's center. Engines,
//! thrusters, wheels and control surfaces sit where [`crate::presets`] simulates them.
//!
//! This file holds the shapes: the hull as longitudinal curves with superellipse sections,
//! the wing, fin and body flap panels, the pods, and the texture atlas layout.

use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::OnceLock;

use engine_core::math::{Mat4, Quat, Vec2, Vec3};
use engine_render::{Mesh, Vertex};

use super::ship_surfaces::{Side, UvFn};
use super::surfaces::Panel;
use super::toolkit::*;

pub const PREFIX: &str = "vehicles/aircraft/spaceplane";

pub fn key(name: &str) -> String {
    format!("{PREFIX}/{name}")
}

// ---------------------------------------------------------------- hull

pub const Z_NOSE: f32 = -10.8;
/// The aft face (base heat shield) the main engines stand out of.
pub const Z_BASE: f32 = 10.3;
/// Hull atlas (1280 × 1792 texels, ≈ 1.2 cm each): meters per texture unit around (u, both
/// halves side by side from the bottom center) and along (v).
pub const HULL_U: f32 = 15.0;
pub const HULL_UV: f32 = 21.6;
pub const HULL_ATLAS: (usize, usize) = (1280, 1792);

pub struct Hull {
    hw: Curve,
    top: Curve,
    bot: Curve,
    ch: Curve,
    pu: Curve,
    pl: Curve,
}

impl Hull {
    pub fn new() -> Self {
        Self {
            hw: Curve::new(&[
                (-10.8, 0.0),
                (-10.77, 0.42),
                (-10.7, 0.68),
                (-10.55, 0.98),
                (-10.3, 1.32),
                (-10.0, 1.60),
                (-9.5, 1.93),
                (-9.0, 2.13),
                (-8.0, 2.34),
                (-6.5, 2.44),
                (-5.0, 2.46),
                (8.0, 2.46),
                (Z_BASE, 2.40),
            ]),
            top: Curve::new(&[
                (-10.8, -0.48),
                (-10.77, -0.15),
                (-10.7, 0.06),
                (-10.5, 0.38),
                (-10.2, 0.72),
                (-9.8, 1.02),
                (-9.2, 1.28),
                (-8.6, 1.43),
                (-8.0, 1.51),
                (-7.7, 1.55),
                (-7.2, 1.80),
                (-6.8, 1.97),
                (-6.4, 2.05),
                (-5.4, 2.07),
                (-4.6, 1.97),
                (-3.8, 1.74),
                (-3.2, 1.64),
                (8.0, 1.63),
                (Z_BASE, 1.56),
            ]),
            bot: Curve::new(&[
                (-10.8, -0.48),
                (-10.77, -0.80),
                (-10.7, -0.93),
                (-10.5, -1.10),
                (-10.2, -1.26),
                (-9.7, -1.40),
                (-9.0, -1.50),
                (-8.0, -1.57),
                (-7.0, -1.60),
                (9.6, -1.60),
                (Z_BASE, -1.55),
            ]),
            ch: Curve::new(&[
                (-10.8, -0.48),
                (-10.6, -0.55),
                (-10.0, -0.72),
                (-9.0, -0.88),
                (-7.5, -0.98),
                (-6.0, -1.0),
                (Z_BASE, -1.0),
            ]),
            pu: Curve::new(&[
                (-10.8, 2.0),
                (-9.5, 2.2),
                (-7.0, 2.6),
                (-4.0, 3.1),
                (Z_BASE, 3.1),
            ]),
            pl: Curve::new(&[
                (-10.8, 2.0),
                (-9.5, 2.6),
                (-8.0, 4.0),
                (-6.0, 5.0),
                (Z_BASE, 5.0),
            ]),
        }
    }

    pub fn half_width(&self, z: f32) -> f32 {
        self.hw.at(z)
    }
    pub fn top_at(&self, z: f32) -> f32 {
        self.top.at(z)
    }
    pub fn bottom_at(&self, z: f32) -> f32 {
        self.bot.at(z)
    }
    pub fn chine(&self, z: f32) -> f32 {
        self.ch.at(z)
    }

    /// Point at station `z`, angle `phi` (−π/2 bottom, 0 right chine, π/2 top, π left).
    pub fn point(&self, z: f32, phi: f32) -> Vec3 {
        let (s, c) = phi.sin_cos();
        let hw = self.hw.at(z);
        let ch = self.ch.at(z);
        let (h, p) = if s >= 0.0 {
            (self.top.at(z) - ch, self.pu.at(z))
        } else {
            (ch - self.bot.at(z), self.pl.at(z))
        };
        let e = 2.0 / p;
        Vec3::new(
            hw * c.signum() * c.abs().powf(e),
            ch + h * s.signum() * s.abs().powf(e),
            z,
        )
    }

    pub fn normal(&self, z: f32, phi: f32) -> Vec3 {
        let h = 1e-3;
        let n = (self.point(z, phi + h) - self.point(z, phi - h))
            .cross(self.point(z + h, phi) - self.point(z - h, phi));
        n.normalize_or(Vec3::new(0.0, 0.0, -1.0))
    }

    /// Angles splitting the right half (bottom center to top center) at `z` into `n_lo`
    /// equal arcs below the chine and `n_up` above, with the arc length from the bottom
    /// center at each.
    pub fn ring(&self, z: f32, n_lo: usize, n_up: usize) -> (Vec<f32>, Vec<f32>) {
        let mut phis = Vec::new();
        let mut arcs = Vec::new();
        let mut base = 0.0;
        for (a, b, n) in [(-FRAC_PI_2, 0.0, n_lo), (0.0, FRAC_PI_2, n_up)] {
            let (p, l) = self.spaced(z, a, b, n);
            let skip = usize::from(!phis.is_empty());
            for (phi, arc) in p.into_iter().zip(l).skip(skip) {
                phis.push(phi);
                arcs.push(base + arc);
            }
            base = *arcs.last().unwrap_or(&0.0);
        }
        (phis, arcs)
    }

    /// Angles splitting `a..b` into `n` equal arcs, and the arc length at each.
    fn spaced(&self, z: f32, a: f32, b: f32, n: usize) -> (Vec<f32>, Vec<f32>) {
        const FINE: usize = 128;
        let mut cum = [0.0f32; FINE + 1];
        let mut prev = self.point(z, a);
        for (k, c) in cum.iter_mut().enumerate().skip(1) {
            let p = self.point(z, a + (b - a) * k as f32 / FINE as f32);
            *c = (p - prev).length();
            prev = p;
        }
        for k in 1..=FINE {
            cum[k] += cum[k - 1];
        }
        let total = cum[FINE];
        let mut phis = Vec::with_capacity(n + 1);
        let mut arcs = Vec::with_capacity(n + 1);
        let mut k = 0;
        for i in 0..=n {
            let target = total * i as f32 / n as f32;
            while k < FINE - 1 && cum[k + 1] < target {
                k += 1;
            }
            let t = ((target - cum[k]) / (cum[k + 1] - cum[k]).max(1e-9)).clamp(0.0, 1.0);
            phis.push(a + (b - a) * (k as f32 + t) / FINE as f32);
            arcs.push(target);
        }
        (phis, arcs)
    }

    /// Arc length of the right half at `z` (bottom center to top center).
    pub fn half_perimeter(&self, z: f32) -> f32 {
        let (_, arcs) = self.ring(z, 24, 24);
        *arcs.last().unwrap_or(&0.0)
    }

    /// The angle on the right half whose point has arc length `arc` from the bottom center.
    pub fn phi_at_arc(&self, z: f32, arc: f32) -> f32 {
        let (phis, arcs) = self.ring(z, 48, 64);
        let k = arcs.partition_point(|&a| a < arc).clamp(1, arcs.len() - 1);
        let t = ((arc - arcs[k - 1]) / (arcs[k] - arcs[k - 1]).max(1e-9)).clamp(0.0, 1.0);
        phis[k - 1] + (phis[k] - phis[k - 1]) * t
    }

    /// Where a ray from `from` along `dir` (mostly ±x, ±y or −z) leaves the hull: point and
    /// outward normal.
    pub fn exit(&self, from: Vec3, dir: Vec3) -> (Vec3, Vec3) {
        if dir.z.abs() > 0.7 {
            // Along the length: find the station whose section passes through (x, y).
            let (mut lo, mut hi) = if dir.z < 0.0 {
                (Z_NOSE, from.z)
            } else {
                (from.z, Z_BASE)
            };
            let inside = |z: f32| self.contains(z, from.x, from.y);
            for _ in 0..40 {
                let mid = 0.5 * (lo + hi);
                if inside(mid) == (dir.z < 0.0) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let z = 0.5 * (lo + hi);
            let phi = self.phi_toward(z, from.x, from.y);
            let n = self.normal(z, phi);
            let n = Vec3::new(n.x * from.x.signum(), n.y, n.z);
            return (Vec3::new(from.x, from.y, z), n);
        }
        let z = from.z;
        let side = if from.x < 0.0 || dir.x < -0.7 {
            -1.0
        } else {
            1.0
        };
        let (mut lo, mut hi) = if dir.y.abs() > 0.7 {
            if dir.y > 0.0 {
                (0.0, FRAC_PI_2)
            } else {
                (-FRAC_PI_2, 0.0)
            }
        } else {
            (-FRAC_PI_2, FRAC_PI_2)
        };
        // Bisect on the coordinate the ray keeps fixed.
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            let p = self.point(z, mid);
            let go_up = if dir.y.abs() > 0.7 {
                // x falls from the chine to the center over the upper half and rises over
                // the lower half.
                if dir.y > 0.0 {
                    p.x > from.x.abs()
                } else {
                    p.x < from.x.abs()
                }
            } else {
                p.y < from.y
            };
            if go_up {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let phi = 0.5 * (lo + hi);
        let p = self.point(z, phi);
        let n = self.normal(z, phi);
        (
            Vec3::new(p.x * side, p.y, z),
            Vec3::new(n.x * side, n.y, n.z),
        )
    }

    /// Whether (x, y) lies inside the section at `z`.
    pub fn contains(&self, z: f32, x: f32, y: f32) -> bool {
        let hw = self.hw.at(z).max(1e-5);
        let ch = self.ch.at(z);
        let (h, p) = if y >= ch {
            (self.top.at(z) - ch, self.pu.at(z))
        } else {
            (ch - self.bot.at(z), self.pl.at(z))
        };
        (x.abs() / hw).powf(p) + ((y - ch).abs() / h.max(1e-5)).powf(p) <= 1.0
    }

    /// The angle pointing from the section's center line toward (x, y) (right half mirrored).
    fn phi_toward(&self, z: f32, x: f32, y: f32) -> f32 {
        let mut best = (f32::MAX, 0.0);
        for k in 0..=180 {
            let phi = -FRAC_PI_2 + PI * k as f32 / 180.0;
            let p = self.point(z, phi);
            let d = (p.x - x.abs()).powi(2) + (p.y - y).powi(2);
            if d < best.0 {
                best = (d, phi);
            }
        }
        best.1
    }
}

impl Default for Hull {
    fn default() -> Self {
        Self::new()
    }
}

/// Hull ring stations: dense over the nose cap.
pub fn hull_stations() -> Vec<f32> {
    let mut z = Vec::new();
    for k in 0..16 {
        let t = k as f32 / 15.0;
        z.push(Z_NOSE + 0.8 * t.powf(1.8));
    }
    let a = Z_NOSE + 0.8;
    let n = ((Z_BASE - a) / 0.14).ceil() as usize;
    for k in 1..=n {
        z.push(a + (Z_BASE - a) * k as f32 / n as f32);
    }
    z
}

pub const HULL_N_LO: usize = 24;
pub const HULL_N_UP: usize = 36;

/// Atlas coordinates of a hull point: u = 0.5 ± arc from the bottom center, v = z.
pub fn hull_uv(arc_from_bottom: f32, side: f32, z: f32) -> Vec2 {
    Vec2::new(
        0.5 + side * arc_from_bottom / HULL_U,
        (z - Z_NOSE) / HULL_UV,
    )
}

/// The hull skin (both halves) with analytic normals, minus the cut-outs (`holes`: circles in
/// the belly, (x, z, radius)).
pub fn hull_mesh(h: &Hull, holes: &[(f32, f32, f32)]) -> Mesh {
    let zs = hull_stations();
    let rings: Vec<(Vec<f32>, Vec<f32>)> = zs
        .iter()
        .map(|&z| h.ring(z, HULL_N_LO, HULL_N_UP))
        .collect();
    let mut out = Mesh::default();
    for side in [1.0f32, -1.0] {
        let mut vertices = Vec::new();
        let cols = HULL_N_LO + HULL_N_UP + 1;
        for (i, &z) in zs.iter().enumerate() {
            let (phis, arcs) = &rings[i];
            for j in 0..cols {
                let p = h.point(z, phis[j]);
                let n = h.normal(z, phis[j]);
                vertices.push(Vertex::new(
                    Vec3::new(p.x * side, p.y, p.z),
                    Vec3::new(n.x * side, n.y, n.z),
                    hull_uv(arcs[j], side, z),
                ));
            }
        }
        let mut indices = Vec::new();
        for i in 0..zs.len() - 1 {
            for j in 0..cols - 1 {
                let a = (i * cols + j) as u32;
                let b = a + 1;
                let d = a + cols as u32;
                let c = d + 1;
                let tri = |t: [u32; 3]| -> bool {
                    let cen = t
                        .iter()
                        .map(|&k| Vec3::from(vertices[k as usize].position))
                        .sum::<Vec3>()
                        / 3.0;
                    !holes.iter().any(|&(x, hz, r)| {
                        cen.y < -1.2 && Vec2::new(cen.x - x, cen.z - hz).length() < r
                    })
                };
                // Right half: phi grows along j (bottom to top) and z along i: du × dv points
                // out; the mirrored half winds the other way.
                let (t1, t2) = if side > 0.0 {
                    ([a, b, c], [a, c, d])
                } else {
                    ([a, c, b], [a, d, c])
                };
                for t in [t1, t2] {
                    if tri(t) {
                        indices.extend_from_slice(&t);
                    }
                }
            }
        }
        out.merge(&Mesh::new(vertices, indices));
    }
    out
}

/// The base heat shield closing the hull's aft end.
pub fn base_mesh(h: &Hull) -> Mesh {
    let ring: Vec<Vec3> = (0..96)
        .map(|k| {
            let phi = -FRAC_PI_2 + std::f32::consts::TAU * k as f32 / 96.0;
            let (s, c) = phi.sin_cos();
            // Right half from the point function, the left mirrored.
            let p = h.point(Z_BASE, (s).atan2(c.abs()));
            Vec3::new(p.x * c.signum(), p.y, Z_BASE)
        })
        .collect();
    fan(&ring, Vec3::Z, |p| Vec2::new(p.x, p.y) * 0.5)
}

// ---------------------------------------------------------------- wings

/// Wing frame: span along x from the hull, dihedral about the body z axis.
pub const WING_Y: f32 = -1.18;
pub fn wing_dihedral() -> f32 {
    3f32.to_radians()
}
pub fn wing_rotation(side: f32) -> Quat {
    Quat::from_rotation_z(wing_dihedral() * side)
}
pub fn wing_to_body(side: f32) -> Mat4 {
    let m =
        Mat4::from_translation(Vec3::new(0.0, WING_Y, 0.0)) * Mat4::from_quat(wing_rotation(1.0));
    if side < 0.0 {
        Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)) * m
    } else {
        m
    }
}

pub const WING_S0: f32 = 2.2;
pub const WING_TIP: f32 = 7.0;
pub const ELEVON_IN: (f32, f32) = (2.55, 4.72);
pub const ELEVON_OUT: (f32, f32) = (4.80, 6.78);
const ELEVON_CHORD: f32 = 0.85;

fn le_curve() -> &'static Curve {
    static C: OnceLock<Curve> = OnceLock::new();
    C.get_or_init(|| {
        Curve::new(&[
            (2.2, -4.6),
            (2.45, -3.0),
            (3.1, 1.0),
            (3.5, 2.75),
            (4.0, 3.8),
            (7.0, 8.4),
        ])
    })
}
pub fn wing_le(s: f32) -> f32 {
    le_curve().at(s)
}
pub fn wing_te(s: f32) -> f32 {
    11.0 - 0.06 * (s - WING_S0)
}
fn wing_thick(s: f32) -> f32 {
    let abs = 0.56 + (0.17 - 0.56) * ((s - 2.45) / (WING_TIP - 2.45)).clamp(0.0, 1.0);
    (abs / (wing_te(s) - wing_le(s))).min(0.12)
}
pub fn wing_panel() -> Panel {
    Panel {
        le: wing_le,
        te: wing_te,
        thick: wing_thick,
        camber: 0.0,
        hinge: |s| wing_te(s) - ELEVON_CHORD,
        gap: 0.02,
    }
}

// ---------------------------------------------------------------- fin

pub const FIN_S0: f32 = 1.40;
pub const FIN_TIP: f32 = 4.6;
pub const RUDDER_SPAN: (f32, f32) = (1.75, 4.48);
pub const RUDDER_HINGE: f32 = 10.05;
pub fn fin_panel() -> Panel {
    Panel {
        le: |y| 6.0 + (y - 1.4),
        te: |y| 10.95 - 0.109 * (y - 1.4),
        thick: |_| 0.075,
        camber: 0.0,
        hinge: |_| RUDDER_HINGE,
        gap: 0.012,
    }
}
/// Fin frame (x = body y, y = −body x, z = body z) to body.
pub fn fin_to_body() -> Mat4 {
    Mat4::from_rotation_z(FRAC_PI_2)
}

// ---------------------------------------------------------------- body flap

/// The body flap's hinge (body frame) and chord: under the main engines, split in two halves
/// that follow the elevons.
pub const FLAP_HINGE: Vec3 = Vec3::new(0.0, -1.72, 10.20);
pub const FLAP_CHORD: f32 = 1.25;
pub const FLAP_HALF_SPAN: (f32, f32) = (0.02, 2.0);

/// The right body-flap half around its hinge (hinge at the origin, chord along +z), with a
/// mapping for the top and bottom faces.
pub fn body_flap(uv: UvFn) -> Mesh {
    let r = 0.14;
    // Section outline from the bottom trailing edge, forward, round the nose, aft on top.
    let section = |x: f32| -> Vec<(Vec3, Side)> {
        let mut pts = Vec::new();
        let n = 8;
        for k in (0..=n).rev() {
            let t = k as f32 / n as f32;
            let z = FLAP_CHORD * t;
            let y = -r + 0.10 * t.powf(1.6);
            pts.push((Vec3::new(x, y, z), Side::Lower));
        }
        for k in 1..12 {
            let a = 1.5 * PI - PI * k as f32 / 12.0;
            pts.push((Vec3::new(x, r * a.sin(), r * a.cos()), Side::Edge));
        }
        for k in 0..=n {
            let t = k as f32 / n as f32;
            let z = FLAP_CHORD * t;
            let y = r - 0.11 * t.powf(1.3);
            pts.push((Vec3::new(x, y, z), Side::Upper));
        }
        pts
    };
    let xs: Vec<f32> = (0..=10)
        .map(|k| FLAP_HALF_SPAN.0 + (FLAP_HALF_SPAN.1 - FLAP_HALF_SPAN.0) * k as f32 / 10.0)
        .collect();
    let mut m = Mesh::default();
    // Three grids (bottom, nose, top) so each side maps to its own island.
    for which in [Side::Lower, Side::Edge, Side::Upper] {
        let rows: Vec<Vec<Vec3>> = xs
            .iter()
            .map(|&x| {
                section(x)
                    .into_iter()
                    .enumerate()
                    .filter(|(k, (_, s))| {
                        *s == which || (which == Side::Edge && (*k == 8 || *k == 20))
                    })
                    .map(|(_, (p, _))| p)
                    .collect()
            })
            .collect();
        let mut g = grid(&rows, false, false, |i, j| uv(rows[i][j], which));
        // Rows run along +x and the outline turns from bottom to top: faces must point out.
        let mid = &g.vertices[g.vertices.len() / 2];
        let p = Vec3::from(mid.position);
        let outward = Vec3::new(0.0, p.y, p.z - 0.4);
        if Vec3::from(mid.normal).dot(outward) < 0.0 {
            flip_faces(&mut g);
        }
        m.merge(&g);
    }
    // End caps.
    for (x, dir) in [(FLAP_HALF_SPAN.0, Vec3::NEG_X), (FLAP_HALF_SPAN.1, Vec3::X)] {
        let outline: Vec<Vec3> = section(x).into_iter().map(|(p, _)| p).collect();
        let t = uv(Vec3::ZERO, Side::Edge);
        m.merge(&fan(&outline, dir, |_| t));
    }
    m
}

// ---------------------------------------------------------------- pods

/// An elongated pod around the z axis: a superellipse section (exponent `p`, half sizes
/// `half`) scaled by `radius(z)`, from `z0` to `z1`, `rows` stations by `around` columns.
/// `uv(t, z)` maps the fraction around (from +x, counterclockwise seen from +z) and the
/// station to atlas coordinates.
#[allow(clippy::too_many_arguments)]
pub fn pod(
    half: Vec2,
    p: f32,
    z0: f32,
    z1: f32,
    radius: &Curve,
    rows: usize,
    around: usize,
    uv: &dyn Fn(f32, f32) -> Vec2,
) -> Mesh {
    let zs: Vec<f32> = (0..=rows)
        .map(|k| {
            let t = k as f32 / rows as f32;
            z0 + (z1 - z0) * (0.5 - 0.5 * (PI * t).cos())
        })
        .collect();
    let ring = |z: f32, a: f32| -> Vec3 {
        let q = superellipse(a.rem_euclid(FRAC_PI_2), p);
        let quadrant = (a.rem_euclid(std::f32::consts::TAU) / FRAC_PI_2) as u32;
        let (x, y) = match quadrant {
            0 => (q.x, q.y),
            1 => (-q.y, q.x),
            2 => (-q.x, -q.y),
            _ => (q.y, -q.x),
        };
        let r = radius.at(z);
        Vec3::new(x * half.x * r, y * half.y * r, z)
    };
    let pts: Vec<Vec<Vec3>> = zs
        .iter()
        .map(|&z| {
            (0..=around)
                .map(|j| ring(z, std::f32::consts::TAU * j as f32 / around as f32))
                .collect()
        })
        .collect();
    let mut m = grid(&pts, false, false, |i, j| {
        uv(j as f32 / around as f32, zs[i])
    });
    let mid = &m.vertices[(rows / 2) * (around + 1) + around / 8];
    if Vec3::from(mid.normal).dot(Vec3::from(mid.position) * Vec3::new(1.0, 1.0, 0.0)) < 0.0 {
        flip_faces(&mut m);
    }
    m
}

/// OMS pod (right): centered on (1.48, 1.32), faired into the hull at the front.
pub const OMS_CENTER: Vec2 = Vec2::new(1.48, 1.32);
pub const OMS_Z: (f32, f32) = (5.2, 10.78);
pub fn oms_radius() -> Curve {
    Curve::new(&[
        (OMS_Z.0, 0.0),
        (5.26, 0.38),
        (5.45, 0.64),
        (5.8, 0.84),
        (6.3, 0.96),
        (7.0, 1.0),
        (10.1, 1.0),
        (10.45, 0.9),
        (10.68, 0.62),
        (OMS_Z.1, 0.0),
    ])
}
pub const OMS_HALF: Vec2 = Vec2::new(0.62, 0.52);

/// Wingtip pod (right): RCS ports top and bottom.
pub const TIP_POD: Vec3 = Vec3::new(7.0, -0.72, 0.0);
pub const TIP_POD_Z: (f32, f32) = (7.45, 10.9);
pub fn tip_pod_radius() -> Curve {
    Curve::new(&[
        (TIP_POD_Z.0, 0.0),
        (7.5, 0.42),
        (7.65, 0.72),
        (7.95, 0.93),
        (8.4, 1.0),
        (10.45, 1.0),
        (10.8, 0.6),
        (TIP_POD_Z.1, 0.0),
    ])
}
pub const TIP_POD_R: f32 = 0.2;

// ---------------------------------------------------------------- wing atlas layout

/// Wing atlas (1024²): meters per texture unit.
pub const WING_UV: f32 = 16.4;

/// Wing islands: lower skin, upper skin (projected straight down onto x–z).
pub fn wing_uv(p: Vec3, side: Side) -> Vec2 {
    match side {
        Side::Lower => Vec2::new((p.x - 2.15) / WING_UV, (p.z + 4.65) / WING_UV),
        Side::Upper => Vec2::new(0.305 + (p.x - 2.15) / WING_UV, (p.z + 4.65) / WING_UV),
        Side::Edge => EDGE_UV,
    }
}

/// Fin islands: left side (the panel's upper) and right side.
pub fn fin_uv(p: Vec3, side: Side) -> Vec2 {
    let u = 0.615 + (p.x - 1.35) / WING_UV;
    let v = (p.z - 5.85) / WING_UV;
    match side {
        Side::Upper => Vec2::new(u, v),
        Side::Lower => Vec2::new(u, v + 0.325),
        Side::Edge => EDGE_UV,
    }
}

/// Body flap islands (right half; the left mirrors it): bottom and top.
pub fn flap_uv(p: Vec3, side: Side) -> Vec2 {
    let u = 0.62 + (p.x + 2.05) / WING_UV;
    let v = 0.66 + p.z / WING_UV;
    match side {
        Side::Lower => Vec2::new(u, v),
        Side::Upper => Vec2::new(u, v + 0.09),
        Side::Edge => EDGE_UV,
    }
}

pub fn oms_uv(t: f32, z: f32) -> Vec2 {
    Vec2::new(0.825 + t * 2.3 / WING_UV, (z - 4.5) / WING_UV)
}

pub fn tip_pod_uv(t: f32, z: f32) -> Vec2 {
    Vec2::new(0.825 + t * 1.27 / WING_UV, 0.39 + (z - 6.8) / WING_UV)
}

/// A plain dark patch for edges, ribs and coves.
pub const EDGE_UV: Vec2 = Vec2::new(0.975, 0.975);

/// A quad covering the plain patch (so it gets painted).
pub fn edge_patch() -> Mesh {
    let v = |u: f32, w: f32| Vertex::new(Vec3::new(u, w, 0.0), Vec3::Z, Vec2::new(u, w));
    Mesh::new(
        vec![
            v(0.955, 0.955),
            v(0.995, 0.955),
            v(0.995, 0.995),
            v(0.955, 0.995),
        ],
        vec![0, 1, 2, 0, 2, 3],
    )
}

/// Wing atlas part ids.
pub const W_WING_LOWER: u8 = 1;
pub const W_WING_UPPER: u8 = 2;
pub const W_FIN_LEFT: u8 = 3;
pub const W_FIN_RIGHT: u8 = 4;
pub const W_FLAP_LOWER: u8 = 5;
pub const W_FLAP_UPPER: u8 = 6;
pub const W_OMS: u8 = 7;
pub const W_TIP_POD: u8 = 8;
pub const W_EDGE: u8 = 9;
