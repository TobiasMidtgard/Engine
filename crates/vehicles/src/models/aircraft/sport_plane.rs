//! The sport plane: a two-seat, side-by-side composite aerobatic trainer on fixed tricycle
//! gear (think Slingsby T67 or Grob G 115 proportions on the simulated 10.5 m wing).
//!
//! Body frame: x right, y up, −z forward, meters, origin at the wing's quarter chord on the
//! fuselage centerline. Every drawn wing, control surface, wheel and the propeller sit where
//! [`crate::presets::sport_plane_aircraft`] and [`crate::presets::sport_plane_wheels`] put the
//! simulated ones.
//!
//! Parts: fuselage with cowling (inlets, exhaust stubs, seams, fasteners), anti-glare panel,
//! bubble canopy with frame over a two-seat cockpit (seats, harnesses, sticks, throttle
//! quadrant, instrument panel with two lit screens and standby gauges), wings with a NACA 24xx
//! section and dihedral, root fillets, flaps and ailerons, tailplane with elevators and trim
//! tabs, fin with rudder, wheel fairings, three-blade propeller with twisted blades and a
//! spinner, navigation lights, strobes, beacon, landing light, pitot tube, antennas, static
//! wicks, livery and registration (fictional).

use std::f32::consts::{FRAC_PI_2, PI};

use engine_core::math::{Mat4, Quat, Vec2, Vec3};
use engine_render::Mesh;

use super::surfaces::*;
use super::toolkit::*;

/// Key prefix of everything the sport plane registers.
pub const PREFIX: &str = "vehicles/aircraft/sport_plane";

pub fn key(name: &str) -> String {
    format!("{PREFIX}/{name}")
}

// ---------------------------------------------------------------- dimensions

/// Fuselage from the cowl lip to the rudder post.
pub const Z_NOSE: f32 = -3.375;
pub const Z_TAIL: f32 = 4.66;
/// Windshield base and canopy rear (where the glass meets the turtle deck).
pub const Z_WS: f32 = -1.55;
pub const Z_CR: f32 = 1.40;
/// Instrument panel face.
pub const Z_PANEL: f32 = -1.36;
/// Propeller disc (the `PropellerVisual` origin) and blade tip radius.
pub const Z_PROP: f32 = -3.42;
pub const PROP_RADIUS: f32 = 0.78;
/// Longest fuselage perimeter (texture u scale), meters.
pub const C_MAX: f32 = 3.80;

/// Wing: root quarter chord (right wing) and the frame it is built in.
pub const WING_ROOT: Vec3 = Vec3::new(0.55, -0.38, 0.05);
pub fn wing_dihedral() -> f32 {
    0.30f32.atan2(4.7)
}
pub fn wing_rotation(side: f32) -> Quat {
    Quat::from_rotation_z(wing_dihedral() * side) * Quat::from_rotation_x(1.5f32.to_radians())
}
/// Local chord at span station `s` (meters from the root): matches the simulated inner and
/// outer panels at their centers (1.45 m at 1.15 m, 1.2 m at 3.5 m).
pub fn wing_chord(s: f32) -> f32 {
    1.572 - 0.1064 * s
}
fn wing_thickness(s: f32) -> f32 {
    0.15 - 0.03 * (s / 4.7).clamp(0.0, 1.0)
}
const WING_CAMBER: f32 = 0.02;
const WING_S0: f32 = -0.22;
pub const WING_TIP: f32 = 4.70;
const TIP_CAP: f32 = 0.07;
/// Control surfaces: fraction of chord and span ranges (meters from the root).
const WING_HINGE: f32 = 0.75;
pub const FLAP_SPAN: (f32, f32) = (0.06, 2.27);
pub const AILERON_SPAN: (f32, f32) = (2.33, 4.42);

/// Tailplane (right half) in its own frame: root at the fuselage, quarter chord at z = 0 of
/// the simulated surface (4.25).
pub const TAIL_Y: f32 = 0.32;
pub const TAIL_Z: f32 = 4.25;
pub fn tail_rotation() -> Quat {
    Quat::from_rotation_x(-1f32.to_radians())
}
/// Tailplane leading and trailing edge (body z) at span x (body, from the centerline).
fn tail_le(x: f32) -> f32 {
    3.985 + 0.11 * (x / 1.85)
}
fn tail_te(x: f32) -> f32 {
    4.86 - 0.11 * (x / 1.85)
}
/// The elevator hinge (unswept), body z.
pub const ELEVATOR_HINGE: f32 = 4.475;
pub const ELEVATOR_SPAN: (f32, f32) = (0.15, 1.80);
const TAIL_TIP: f32 = 1.85;

/// Fin: root height and tip, swept leading edge, unswept rudder hinge.
const FIN_ROOT_Y: f32 = 0.30;
const FIN_TIP_Y: f32 = 1.80;
pub const RUDDER_HINGE: f32 = 4.6325;
fn fin_le(y: f32) -> f32 {
    // Swept ~38° from the root (at the turtle deck) to the tip.
    let t = ((y - 0.42) / (FIN_TIP_Y - 0.42)).clamp(0.0, 1.0);
    3.62 + t * (4.36 - 3.62)
}
fn fin_te(y: f32) -> f32 {
    let t = ((y - FIN_ROOT_Y) / (FIN_TIP_Y - FIN_ROOT_Y)).clamp(0.0, 1.0);
    5.06 - 0.10 * t
}

// ---------------------------------------------------------------- fuselage

/// The fuselage as longitudinal curves; cross-sections are superellipses split at the
/// widest line (the canopy sill).
pub struct Fuselage {
    hw: Curve,
    top: Curve,
    bot: Curve,
    yw: Curve,
    plo: Curve,
    pup: Curve,
    ctop: Curve,
    pc: Curve,
}

impl Fuselage {
    pub fn new() -> Self {
        Self {
            hw: Curve::new(&[
                (-3.375, 0.212),
                (-3.365, 0.255),
                (-3.345, 0.295),
                (-3.31, 0.335),
                (-3.25, 0.375),
                (-3.10, 0.418),
                (-2.80, 0.458),
                (-2.40, 0.497),
                (-2.00, 0.532),
                (-1.50, 0.566),
                (-1.00, 0.590),
                (-0.50, 0.600),
                (0.00, 0.596),
                (0.50, 0.578),
                (0.90, 0.545),
                (1.50, 0.470),
                (2.50, 0.335),
                (3.50, 0.205),
                (4.20, 0.118),
                (4.50, 0.068),
                (4.66, 0.032),
            ]),
            top: Curve::new(&[
                (-3.375, 0.212),
                (-3.365, 0.245),
                (-3.345, 0.272),
                (-3.31, 0.295),
                (-3.25, 0.315),
                (-3.10, 0.338),
                (-2.80, 0.362),
                (-2.40, 0.386),
                (-2.00, 0.405),
                (-1.55, 0.425),
                (-1.00, 0.450),
                (0.00, 0.490),
                (0.90, 0.525),
                (1.40, 0.535),
                (2.00, 0.527),
                (2.80, 0.500),
                (3.60, 0.465),
                (4.20, 0.440),
                (4.50, 0.425),
                (4.66, 0.405),
            ]),
            bot: Curve::new(&[
                (-3.375, -0.212),
                (-3.365, -0.245),
                (-3.345, -0.275),
                (-3.31, -0.300),
                (-3.25, -0.322),
                (-3.10, -0.348),
                (-2.80, -0.372),
                (-2.40, -0.402),
                (-2.00, -0.440),
                (-1.50, -0.495),
                (-1.00, -0.532),
                (-0.40, -0.550),
                (0.40, -0.550),
                (0.90, -0.530),
                (1.50, -0.462),
                (2.50, -0.310),
                (3.50, -0.120),
                (4.20, 0.050),
                (4.50, 0.140),
                (4.66, 0.200),
            ]),
            yw: Curve::new(&[
                (-3.375, 0.0),
                (-3.10, 0.0),
                (-2.40, 0.060),
                (-2.00, 0.130),
                (-1.55, 0.280),
                (-1.00, 0.312),
                (0.00, 0.322),
                (0.90, 0.312),
                (1.50, 0.280),
                (2.50, 0.225),
                (3.50, 0.205),
                (4.20, 0.225),
                (4.66, 0.290),
            ]),
            plo: Curve::new(&[
                (-3.375, 2.0),
                (-3.10, 2.3),
                (-2.40, 2.6),
                (-1.00, 2.7),
                (0.90, 2.6),
                (2.50, 2.3),
                (4.66, 2.0),
            ]),
            pup: Curve::new(&[
                (-3.375, 2.0),
                (-3.10, 2.2),
                (-2.00, 2.3),
                (1.40, 2.3),
                (2.50, 2.2),
                (4.66, 2.0),
            ]),
            ctop: Curve::new(&[
                (Z_WS, 0.425),
                (-1.38, 0.565),
                (-1.15, 0.690),
                (-0.85, 0.790),
                (-0.50, 0.852),
                (-0.15, 0.872),
                (0.25, 0.858),
                (0.60, 0.805),
                (0.95, 0.705),
                (1.20, 0.610),
                (Z_CR, 0.535),
            ]),
            pc: Curve::new(&[
                (Z_WS, 2.3),
                (-1.25, 2.1),
                (0.0, 2.05),
                (1.0, 2.1),
                (Z_CR, 2.3),
            ]),
        }
    }

    pub fn half_width(&self, z: f32) -> f32 {
        self.hw.at(z)
    }
    pub fn sill(&self, z: f32) -> f32 {
        self.yw.at(z)
    }
    pub fn top_at(&self, z: f32) -> f32 {
        self.top.at(z)
    }
    pub fn bottom_at(&self, z: f32) -> f32 {
        self.bot.at(z)
    }
    pub fn canopy_top(&self, z: f32) -> f32 {
        self.ctop.at(z)
    }

    /// Point at station `z`, angle `phi` (−π/2 bottom, 0 right sill, π/2 top, π left sill).
    /// `canopy` uses the canopy's crown instead of the fuselage top.
    pub fn point(&self, z: f32, phi: f32, canopy: bool) -> Vec3 {
        let (s, c) = phi.sin_cos();
        let hw = self.hw.at(z);
        let yw = self.yw.at(z);
        let (p, h) = if s >= 0.0 {
            if canopy && (Z_WS..=Z_CR).contains(&z) {
                (self.pc.at(z), self.ctop.at(z) - yw)
            } else {
                (self.pup.at(z), self.top.at(z) - yw)
            }
        } else {
            (self.plo.at(z), yw - self.bot.at(z))
        };
        let e = 2.0 / p;
        Vec3::new(
            hw * c.signum() * c.abs().powf(e),
            yw + h * s.signum() * s.abs().powf(e),
            z,
        )
    }

    /// Angles that split the arc `a..b` at station `z` into `n` equal lengths, and the
    /// arc's length.
    fn spaced(&self, z: f32, a: f32, b: f32, n: usize, canopy: bool) -> (Vec<f32>, f32) {
        const FINE: usize = 96;
        let mut cum = [0.0f32; FINE + 1];
        let mut prev = self.point(z, a, canopy);
        for k in 1..=FINE {
            let phi = a + (b - a) * k as f32 / FINE as f32;
            let p = self.point(z, phi, canopy);
            cum[k] = cum[k - 1] + (p - prev).length();
            prev = p;
        }
        let total = cum[FINE].max(1e-6);
        let mut out = Vec::with_capacity(n + 1);
        let mut k = 0;
        for i in 0..=n {
            let target = total * i as f32 / n as f32;
            while k < FINE - 1 && cum[k + 1] < target {
                k += 1;
            }
            let t = ((target - cum[k]) / (cum[k + 1] - cum[k]).max(1e-9)).clamp(0.0, 1.0);
            out.push(a + (b - a) * (k as f32 + t) / FINE as f32);
        }
        (out, total)
    }

    /// The point on the right side at height `y` at station `z` (bisection on the angle).
    pub fn side_at_height(&self, z: f32, y: f32) -> Vec3 {
        let (mut lo, mut hi) = (-FRAC_PI_2, FRAC_PI_2);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            if self.point(z, mid, false).y < y {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        self.point(z, 0.5 * (lo + hi), false)
    }
}

impl Default for Fuselage {
    fn default() -> Self {
        Self::new()
    }
}

/// Ring stations: fine at the cowl lip and at the canopy ends.
fn stations() -> Vec<f32> {
    let mut z = Vec::new();
    for k in 0..14 {
        z.push(Z_NOSE + 0.10 * (k as f32 / 13.0).powf(1.7));
    }
    let span = |z: &mut Vec<f32>, a: f32, b: f32, step: f32| {
        let n = ((b - a) / step).ceil() as usize;
        for k in 1..=n {
            z.push(a + (b - a) * k as f32 / n as f32);
        }
    };
    span(&mut z, Z_NOSE + 0.10, Z_WS, 0.07);
    span(&mut z, Z_WS, Z_CR, 0.06);
    span(&mut z, Z_CR, Z_TAIL, 0.085);
    z
}

/// Fuselage surface params on a parametric grid with analytic normals: `params[i][j]` =
/// (z, phi).
fn fuselage_grid(
    f: &Fuselage,
    params: &[Vec<(f32, f32)>],
    canopy: bool,
    uv: impl Fn(usize, usize) -> Vec2,
) -> Mesh {
    let rows: Vec<Vec<Vec3>> = params
        .iter()
        .map(|r| r.iter().map(|&(z, phi)| f.point(z, phi, canopy)).collect())
        .collect();
    let mut m = grid(&rows, false, false, uv);
    // Analytic normals (no creases where grids meet).
    let cols = params[0].len();
    for (i, r) in params.iter().enumerate() {
        for (j, &(z, phi)) in r.iter().enumerate() {
            let h = 1e-3;
            let dp = f.point(z, phi + h, canopy) - f.point(z, phi - h, canopy);
            let dz = f.point(z + h, phi, canopy) - f.point(z - h, phi, canopy);
            let n = dp.cross(dz);
            if n.length_squared() > 1e-16 {
                m.vertices[i * cols + j].normal = n.normalize().to_array();
            }
        }
    }
    m
}

/// The fuselage shell (minus the canopy opening), with UVs: u around (0.5 on the top center,
/// arc length over `C_MAX`), v along (z).
pub fn fuselage_mesh(f: &Fuselage) -> Mesh {
    let zs = stations();
    let (n_lo, n_up) = (22usize, 18usize);
    let v_of = |z: f32| (z - Z_NOSE) / (Z_TAIL - Z_NOSE);
    // Per ring: angles and arc lengths of each quarter.
    struct Ring {
        lr: Vec<f32>,
        ur: Vec<f32>,
        a_lr: f32,
        a_ur: f32,
    }
    let rings: Vec<Ring> = zs
        .iter()
        .map(|&z| {
            let (lr, a_lr) = f.spaced(z, -FRAC_PI_2, 0.0, n_lo, false);
            let (ur, a_ur) = f.spaced(z, 0.0, FRAC_PI_2, n_up, false);
            Ring { lr, ur, a_lr, a_ur }
        })
        .collect();
    let mut out = Mesh::default();
    // Right side lower quarter: phi from -π/2 (bottom) up to 0 (sill); u from the top center.
    let quarter = |rows: &[usize], which: u8| -> Mesh {
        let params: Vec<Vec<(f32, f32)>> = rows
            .iter()
            .map(|&i| {
                let r = &rings[i];
                let z = zs[i];
                match which {
                    // lower right: -π/2 .. 0
                    0 => r.lr.iter().map(|&p| (z, p)).collect(),
                    // upper right: 0 .. π/2
                    1 => r.ur.iter().map(|&p| (z, p)).collect(),
                    // upper left: π/2 .. π (mirror of upper right)
                    2 => r.ur.iter().rev().map(|&p| (z, PI - p)).collect(),
                    // lower left: π .. 3π/2
                    _ => r.lr.iter().rev().map(|&p| (z, PI - p)).collect(),
                }
            })
            .collect();
        fuselage_grid(f, &params, false, |ii, j| {
            let i = rows[ii];
            let r = &rings[i];
            // Arc from the top center.
            let from_top = match which {
                0 => r.a_ur + r.a_lr * (1.0 - j as f32 / n_lo as f32),
                1 => r.a_ur * (1.0 - j as f32 / n_up as f32),
                2 => r.a_ur * (j as f32 / n_up as f32),
                _ => r.a_ur + r.a_lr * (j as f32 / n_lo as f32),
            };
            let u = if which <= 1 {
                0.5 - from_top / C_MAX
            } else {
                0.5 + from_top / C_MAX
            };
            Vec2::new(u, v_of(zs[i]))
        })
    };
    let all: Vec<usize> = (0..zs.len()).collect();
    let front: Vec<usize> = (0..zs.len()).filter(|&i| zs[i] <= Z_WS + 1e-4).collect();
    let rear: Vec<usize> = (0..zs.len()).filter(|&i| zs[i] >= Z_CR - 1e-4).collect();
    out.merge(&quarter(&all, 0));
    out.merge(&quarter(&all, 3));
    for rows in [&front, &rear] {
        out.merge(&quarter(rows, 1));
        out.merge(&quarter(rows, 2));
    }
    out
}

/// The canopy glass over the sill from the windshield base to the turtle deck.
pub fn canopy_mesh(f: &Fuselage) -> Mesh {
    let zs: Vec<f32> = stations()
        .into_iter()
        .filter(|&z| (Z_WS - 1e-4..=Z_CR + 1e-4).contains(&z))
        .collect();
    let n = 22;
    let params: Vec<Vec<(f32, f32)>> = zs
        .iter()
        .map(|&z| {
            let (ur, _) = f.spaced(z, 0.0, FRAC_PI_2, n, true);
            let mut row: Vec<(f32, f32)> = ur.iter().map(|&p| (z, p)).collect();
            row.extend(ur.iter().rev().skip(1).map(|&p| (z, PI - p)));
            row
        })
        .collect();
    fuselage_grid(f, &params, true, |i, j| {
        Vec2::new(j as f32 / (2 * n) as f32, i as f32 / (zs.len() - 1) as f32)
    })
}

// ---------------------------------------------------------------- wings and tail

pub fn wing_panel() -> Panel {
    Panel {
        le: |s| -0.25 * wing_chord(s),
        te: |s| 0.75 * wing_chord(s),
        thick: wing_thickness,
        camber: WING_CAMBER,
        hinge: |s| (WING_HINGE - 0.25) * wing_chord(s),
        gap: 0.006,
    }
}

/// Wing atlas (right wing; the left mirrors it): u is meters over `WING_U`, v the span.
pub const WING_U: f32 = 3.90;
const WING_V: f32 = WING_TIP + TIP_CAP + 0.06 - WING_S0;
fn wing_uv() -> Uv {
    Uv {
        u0: 0.0,
        u_scale: WING_U,
        v0: WING_S0,
        v_scale: WING_V,
    }
}
/// Control surfaces and the fixed trailing-edge strips share columns: they never cover the
/// same span.
const WING_LAYOUT: PanelLayout = PanelLayout {
    control: 0.645,
    te_lower: 0.645,
    te_upper: 0.755,
    cove: 0.90,
};

/// The right wing's fixed parts in the wing frame, by atlas part.
pub fn wing_fixed() -> Vec<(u8, Mesh)> {
    fixed_parts(
        &wing_panel(),
        WING_S0,
        WING_TIP,
        TIP_CAP,
        &[FLAP_SPAN, AILERON_SPAN],
        0.10,
        34,
        wing_uv(),
        WING_LAYOUT,
    )
}

/// The right flap (0) or aileron (1) in the wing frame.
pub fn wing_control(which: usize) -> Mesh {
    let (a, b) = if which == 0 { FLAP_SPAN } else { AILERON_SPAN };
    let g = 0.008;
    control_surface(
        &wing_panel(),
        a + g,
        b - g,
        26,
        wing_uv().at_u(WING_LAYOUT.control),
    )
}

/// Hinge line ends of a wing control surface, wing frame.
pub fn wing_hinge(which: usize) -> (Vec3, Vec3) {
    let p = wing_panel();
    let (a, b) = if which == 0 { FLAP_SPAN } else { AILERON_SPAN };
    (p.hinge_pt(a).0, p.hinge_pt(b).0)
}

pub fn tail_panel() -> Panel {
    Panel {
        le: tail_le,
        te: tail_te,
        thick: |_| 0.10,
        camber: 0.0,
        hinge: |_| ELEVATOR_HINGE,
        gap: 0.005,
    }
}

pub fn fin_panel() -> Panel {
    Panel {
        le: fin_le,
        te: fin_te,
        thick: |y| 0.105 - 0.02 * ((y - FIN_ROOT_Y) / (FIN_TIP_Y - FIN_ROOT_Y)).clamp(0.0, 1.0),
        camber: 0.0,
        hinge: |_| RUDDER_HINGE,
        gap: 0.005,
    }
}

/// Tail atlas (4.4 m square): tailplane island at the top, fin below it, dorsal fillet at
/// the bottom.
const TAIL_UV: f32 = 4.4;
const FIN_V: f32 = 0.45;
const DORSAL_V: f32 = 0.88;
const TAIL_S0: f32 = 0.06;
const FIN_S0: f32 = 0.10;
pub const RUDDER_SPAN: (f32, f32) = (0.20, 1.78);
fn tail_uv() -> Uv {
    Uv {
        u0: 0.0,
        u_scale: TAIL_UV,
        v0: TAIL_S0,
        v_scale: TAIL_UV,
    }
}
fn fin_uv() -> Uv {
    Uv {
        u0: 0.0,
        u_scale: TAIL_UV,
        v0: FIN_S0 - FIN_V * TAIL_UV,
        v_scale: TAIL_UV,
    }
}
const TAIL_LAYOUT: PanelLayout = PanelLayout {
    control: 0.245,
    te_lower: 0.245,
    te_upper: 0.315,
    cove: 0.45,
};
const FIN_LAYOUT: PanelLayout = PanelLayout {
    control: 0.49,
    te_lower: 0.49,
    te_upper: 0.60,
    cove: 0.73,
};

/// Fixed right tailplane (tail frame) by atlas part.
pub fn tailplane_fixed() -> Vec<(u8, Mesh)> {
    fixed_parts(
        &tail_panel(),
        TAIL_S0,
        TAIL_TIP,
        0.05,
        &[ELEVATOR_SPAN],
        0.08,
        24,
        tail_uv(),
        TAIL_LAYOUT,
    )
}

/// Fixed fin (fin frame) by atlas part, plus the dorsal fillet.
pub fn fin_fixed() -> Vec<(u8, Mesh)> {
    let mut parts = fixed_parts(
        &fin_panel(),
        FIN_S0,
        FIN_TIP_Y,
        0.05,
        &[RUDDER_SPAN],
        0.08,
        24,
        fin_uv(),
        FIN_LAYOUT,
    );
    let dorsal = Panel {
        le: |y| {
            let t = ((y - 0.40) / 0.36).clamp(0.0, 1.0);
            2.70 + (fin_le(0.76) - 2.70) * t.powf(0.8)
        },
        te: |_| 4.05,
        thick: |y| 0.035 + 0.03 * ((y - 0.40) / 0.36).clamp(0.0, 1.0),
        camber: 0.0,
        hinge: |_| 4.0,
        gap: 0.0,
    };
    let ss = span_stations(0.30, 0.76, 0.03, &[]);
    let dorsal_uv = Uv {
        v0: 0.30 - DORSAL_V * TAIL_UV,
        ..fin_uv()
    };
    parts.push((PART_TIP + 1, skin(&dorsal, &ss, true, 18, dorsal_uv)));
    parts
}

/// The right elevator (tail frame) and the rudder (fin frame).
pub fn elevator() -> Mesh {
    let g = 0.006;
    control_surface(
        &tail_panel(),
        ELEVATOR_SPAN.0 + g,
        ELEVATOR_SPAN.1 - g,
        22,
        tail_uv().at_u(TAIL_LAYOUT.control),
    )
}

pub fn rudder() -> Mesh {
    control_surface(
        &fin_panel(),
        RUDDER_SPAN.0,
        RUDDER_SPAN.1 - 0.006,
        22,
        fin_uv().at_u(FIN_LAYOUT.control),
    )
}

/// Tail frame (z = body z before the −1° incidence about the quarter chord) to body.
pub fn tail_to_body(side: f32) -> Mat4 {
    let m = Mat4::from_translation(Vec3::new(0.0, TAIL_Y, TAIL_Z))
        * Mat4::from_quat(tail_rotation())
        * Mat4::from_translation(Vec3::new(0.0, 0.0, -TAIL_Z));
    mirror_if(side, m)
}

/// Fin frame (x = body y, y = −body x, z = body z) to body.
pub fn fin_to_body() -> Mat4 {
    Mat4::from_rotation_z(FRAC_PI_2)
}

/// Wing frame to body for a side (+1 right, −1 left; the left is the mirror image).
pub fn wing_to_body(side: f32) -> Mat4 {
    mirror_if(
        side,
        Mat4::from_translation(WING_ROOT) * Mat4::from_quat(wing_rotation(1.0)),
    )
}

fn mirror_if(side: f32, m: Mat4) -> Mat4 {
    if side < 0.0 {
        Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)) * m
    } else {
        m
    }
}

/// Body point of the right wing's upper (or lower) surface at span `s`, chord fraction `x`.
pub fn wing_surface(s: f32, x: f32, upper: bool) -> Vec3 {
    wing_to_body(1.0).transform_point3(wing_panel().pt(s, x, upper))
}
