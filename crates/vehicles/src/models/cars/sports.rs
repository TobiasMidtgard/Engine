//! The sports car: a front-engine GT coupé (long hood, cab-rearward, wide rear haunches),
//! sculpted as a signed-distance field and meshed with surface nets.
//!
//! Body frame: x right, y up, −z forward, meters; the ground is at y = −[`GROUND`]. The
//! profile curves below use the height above the ground (`h`) so they read like a drawing.

use engine_core::math::{Vec2, Vec3};

use super::details::Proj;
use super::sdf::*;

pub use super::glass::Pane;

/// Ground height below the body origin when the car rests on its springs.
pub const GROUND: f32 = 0.45;
/// Front and rear axle positions (z) and the wheel center height above the ground.
pub const FRONT_AXLE: f32 = -1.35;
pub const REAR_AXLE: f32 = 1.30;
pub const WHEEL_H: f32 = 0.33;
/// Wheel-arch opening radii.
pub const ARCH_F: f32 = 0.368;
pub const ARCH_R: f32 = 0.374;
/// Inner wall of the wheel wells (|x|).
pub const LINER_X: f32 = 0.6;

/// The body's profile curves.
pub struct Body {
    hood: Table,
    fender: Table,
    plan: Table,
    hmax: Table,
    cup: Table,
    clow: Table,
    shoulder: Table,
    floor: Table,
    nose: Table,
    tail: Table,
    roof: Table,
    gh_w: Table,
    belt: Table,
    crease: Table,
    corner_x: Table,
    pub features: Features,
}

impl Default for Body {
    fn default() -> Self {
        Self::new()
    }
}

impl Body {
    pub fn new() -> Self {
        let mut b = Self::profiles();
        // The greenhouse starts flush with the door tops (the designed widths only narrow it
        // toward the tail, leaving shelves over the rear haunches).
        let designed = b.gh_w.clone();
        let flush = |z: f32| designed.at(z).min(b.side_w(z, b.belt.at(z)).0 - 0.006);
        let gh = Table::from_fn(-0.7, 2.1, 1400, |z| {
            (flush(z), (flush(z + 1e-3) - flush(z - 1e-3)) / 2e-3)
        });
        b.gh_w = gh;
        let cx = |z: f32| b.corner(z).0;
        b.corner_x = Table::from_fn(-0.7, 2.1, 700, |z| (cx(z), 0.0));
        b.features = Features::new();
        b
    }

    /// Negative on the glass areas of the shell (`inset`: how far inside the shell the
    /// surface being cut lies, 0 for the outer skin) shrunk by `margin`.
    pub fn window_field(&self, win: &Windows, p: Vec3, inset: f32, margin: f32) -> f32 {
        let h = p.y + GROUND;
        if h < 0.75 || !(-0.6..2.0).contains(&p.z) {
            return 1.0;
        }
        let ax = p.x.abs();
        let side = win
            .side
            .sd(Vec2::new(p.z, h))
            .max(self.corner_x.at(p.z) - 0.03 - inset - ax);
        let ws = win.windshield.sd(Vec2::new(p.x, p.z));
        let back = win.back.sd(Vec2::new(p.x, p.z));
        side.min(ws).min(back).max(0.8 - h) + margin
    }

    /// Negative on the wheel-well surfaces (liner), positive on the painted skin.
    pub fn liner_field(&self, p: Vec3) -> f32 {
        let h = p.y + GROUND;
        let ax = p.x.abs();
        let mut f = 1.0f32;
        for (zc, r) in [(FRONT_AXLE, ARCH_F), (REAR_AXLE, ARCH_R)] {
            let rho = Vec2::new(p.z - zc, h - WHEEL_H).length();
            if rho > r + 0.1 {
                continue;
            }
            let w = self.side_w(p.z, h).0;
            f = f.min((rho - r - 0.02).max(ax - (w - 0.005)));
        }
        f
    }

    /// Negative on the lower trim (front lip, side skirts, rear valance and the underside).
    pub fn trim_field(&self, p: Vec3) -> f32 {
        let h = p.y + GROUND;
        let z = p.z;
        let line = if z < 0.0 {
            0.17 + 0.02 * sstep(-2.05, -1.80, z)
        } else {
            0.19 + 0.145 * sstep(1.70, 1.95, z)
        };
        h - line
    }

    /// Negative inside carved openings (more than 4 mm below the plain skin). Continuous
    /// across each opening's rounded lip: the skin is only skipped (1.0) well clear of every
    /// opening, where it is never below the plain surface anyway.
    pub fn pocket_field(&self, p: Vec3) -> f32 {
        if self.features.near(p, LIP).is_none() {
            return 1.0;
        }
        self.shell_plain(p) + 0.004
    }

    fn profiles() -> Self {
        Self {
            features: Features::default(),
            corner_x: Table::from_fn(0.0, 1.0, 2, |_| (0.0, 0.0)),
            // Top of the lower body along the centerline: hood, cowl, beltline, rear deck.
            hood: Table::of(&[
                (-2.30, 0.56),
                (-2.20, 0.615),
                (-2.05, 0.675),
                (-1.80, 0.735),
                (-1.35, 0.79),
                (-0.90, 0.825),
                (-0.50, 0.855),
                (-0.25, 0.875),
                (0.05, 0.905),
                (0.50, 0.925),
                (0.90, 0.94),
                (1.30, 0.945),
                (1.75, 0.95),
                (2.05, 0.968),
                (2.30, 0.982),
            ]),
            // Fender crests rise above the hood and deck near the sides.
            fender: Table::of(&[
                (-2.30, 0.0),
                (-2.10, 0.03),
                (-1.75, 0.065),
                (-1.35, 0.075),
                (-0.95, 0.055),
                (-0.55, 0.02),
                (-0.25, 0.0),
                (0.55, 0.0),
                (0.95, 0.025),
                (1.30, 0.05),
                (1.80, 0.042),
                (2.30, 0.02),
            ]),
            // Plan half-width at the widest height.
            plan: Table::of(&[
                (-2.30, 0.84),
                (-2.10, 0.895),
                (-1.80, 0.93),
                (-1.35, 0.948),
                (-0.95, 0.935),
                (-0.50, 0.905),
                (0.00, 0.893),
                (0.45, 0.905),
                (0.85, 0.945),
                (1.25, 0.985),
                (1.60, 0.968),
                (1.95, 0.905),
                (2.15, 0.85),
                (2.30, 0.78),
            ]),
            // Height of the widest point of the side section.
            hmax: Table::of(&[
                (-2.3, 0.46),
                (-1.4, 0.49),
                (-0.4, 0.52),
                (0.6, 0.56),
                (1.3, 0.62),
                (2.3, 0.62),
            ]),
            // How hard the side curves in above / below the widest point.
            cup: Table::of(&[
                (-2.3, 0.55),
                (-1.4, 0.55),
                (-0.6, 0.85),
                (0.3, 1.0),
                (1.0, 0.95),
                (1.5, 0.8),
                (2.3, 0.75),
            ]),
            clow: Table::of(&[
                (-2.3, 0.35),
                (-1.0, 0.45),
                (0.3, 0.5),
                (1.5, 0.45),
                (2.3, 0.4),
            ]),
            // Shoulder radius between the sides and the top.
            shoulder: Table::of(&[
                (-2.3, 0.06),
                (-1.4, 0.075),
                (-0.5, 0.05),
                (0.3, 0.035),
                (1.0, 0.055),
                (1.5, 0.07),
                (2.3, 0.05),
            ]),
            // Underside.
            floor: Table::of(&[
                (-2.30, 0.24),
                (-2.10, 0.19),
                (-1.85, 0.15),
                (-1.55, 0.125),
                (1.55, 0.13),
                (1.85, 0.17),
                (2.10, 0.25),
                (2.30, 0.34),
            ]),
            // Nose: the front face's z at each height (centerline).
            nose: Table::of(&[
                (0.10, -2.05),
                (0.18, -2.16),
                (0.28, -2.235),
                (0.37, -2.258),
                (0.45, -2.25),
                (0.53, -2.22),
                (0.59, -2.175),
                (0.64, -2.13),
                (0.76, -2.05),
            ]),
            // Tail: the rear face's z at each height.
            tail: Table::of(&[
                (0.22, 1.94),
                (0.34, 2.06),
                (0.50, 2.15),
                (0.66, 2.205),
                (0.82, 2.24),
                (0.92, 2.262),
                (0.97, 2.276),
                (1.01, 2.255),
                (1.06, 2.16),
            ]),
            // Greenhouse: roof line (windshield, roof, fastback).
            roof: Table::of(&[
                (-0.62, 0.55),
                (-0.40, 0.80),
                (-0.25, 0.875),
                (-0.05, 1.01),
                (0.15, 1.12),
                (0.32, 1.205),
                (0.48, 1.24),
                (0.65, 1.25),
                (0.85, 1.238),
                (1.05, 1.20),
                (1.25, 1.135),
                (1.45, 1.065),
                (1.65, 1.005),
                (1.85, 0.965),
                (2.05, 0.90),
            ]),
            // Greenhouse half-width at the beltline.
            gh_w: Table::of(&[
                (-0.6, 0.80),
                (0.8, 0.80),
                (1.1, 0.76),
                (1.4, 0.67),
                (1.7, 0.565),
                (2.0, 0.46),
            ]),
            // Beltline height (bottom of the side glass).
            belt: Table::of(&[
                (-0.6, 0.855),
                (-0.2, 0.868),
                (0.4, 0.885),
                (1.0, 0.91),
                (1.6, 0.94),
                (2.1, 0.965),
            ]),
            crease: Table::of(&[
                (-2.3, 0.62),
                (-1.7, 0.68),
                (-1.0, 0.70),
                (-0.2, 0.705),
                (0.6, 0.72),
                (1.2, 0.76),
                (1.8, 0.78),
                (2.3, 0.78),
            ]),
        }
    }

    /// Half-width of the lower body at `(z, h)` and its slopes along z and h.
    #[inline]
    fn side_w(&self, z: f32, h: f32) -> (f32, f32, f32) {
        let (wp, dwp) = self.plan.eval(z);
        let hm = self.hmax.at(z);
        let (c, dh) = if h > hm {
            let c = self.cup.at(z);
            (c * (h - hm) * (h - hm), 2.0 * c * (h - hm))
        } else {
            let c = self.clow.at(z);
            (c * (hm - h) * (hm - h), -2.0 * c * (hm - h))
        };
        // Character line: above it the side leans in harder (a crisp highlight along the
        // doors that rises over the rear haunch).
        let hk = self.crease.at(z);
        let k = 0.012;
        let e = h - hk;
        let soft = 0.5 * (e + (e * e + k * k).sqrt());
        let slope_k = 0.22 * sstep(-2.2, -1.7, z);
        let (c, dh) = (
            c + slope_k * soft,
            dh + slope_k * 0.5 * (1.0 + e / (e * e + k * k).sqrt()),
        );
        // A sculpted scoop in the lower door, deepest just ahead of the rear wheel.
        let scoop =
            0.016 * sstep(-0.55, 0.55, z) * (1.0 - sstep(0.62, 0.86, z)) * bump(0.40, 0.14, h);
        let c = c + scoop;
        // A rolled lip around each wheel arch.
        let mut lip = 0.0;
        for (zc, r) in [(FRONT_AXLE, ARCH_F), (REAR_AXLE, ARCH_R)] {
            let rho = Vec2::new(z - zc, h - WHEEL_H).length();
            lip += 0.011 * bump(r + 0.028, 0.045, rho) * sstep(0.12, 0.3, h);
        }
        (wp - c + lip, dwp, -dh)
    }

    /// Height of the lower body's top at `(z, |x|)`.
    #[inline]
    fn top_h(&self, z: f32, ax: f32) -> (f32, f32, f32) {
        let (tc, dtc) = self.hood.eval(z);
        let (f, df) = self.fender.eval(z);
        let wp = self.plan.at(z);
        let u = ax / wp;
        let t = ((u - 0.3) / 0.55).clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        let ds = 6.0 * t * (1.0 - t) / 0.55 / wp;
        // A slight crown across the hood center.
        let crown = 0.018 * (1.0 - (u * u).min(1.0));
        (tc + f * s + crown, dtc + df * s, f * ds - 0.036 * u / wp)
    }

    /// The lower body (below the beltline) without openings.
    #[inline]
    pub fn lower(&self, p: Vec3) -> f32 {
        let ax = p.x.abs();
        let h = p.y + GROUND;
        let z = p.z;
        let (w, wz, wh) = self.side_w(z, h);
        let d_side = (ax - w) / (1.0 + wz * wz + wh * wh).sqrt();
        let (t, tz, tx) = self.top_h(z, ax);
        let d_top = (h - t) / (1.0 + tz * tz + tx * tx).sqrt();
        let mut d = rmax(d_side, d_top, self.shoulder.at(z));
        // Nose and tail, rounded in plan view.
        let (zf, dzf) = self.nose.eval(h);
        let plan_f = 0.16 * (ax / 0.92).powi(2);
        let d_front = (zf + plan_f - z) / (1.0 + dzf * dzf + (0.3 * ax / 0.85).powi(2)).sqrt();
        d = rmax(d, d_front, 0.11);
        let (zr, dzr) = self.tail.eval(h);
        let plan_r = 0.17 * (ax / 0.92).powi(2);
        let d_rear = (z - zr + plan_r) / (1.0 + dzr * dzr + (0.34 * ax / 0.85).powi(2)).sqrt();
        d = rmax(d, d_rear, 0.13);
        let (b, db) = self.floor.eval(z);
        let d_bot = (b - h) / (1.0 + db * db).sqrt();
        rmax(d, d_bot, 0.04)
    }

    /// The greenhouse (pillars, roof and the glass areas, solid).
    #[inline]
    pub fn greenhouse(&self, p: Vec3) -> f32 {
        let ax = p.x.abs();
        let h = p.y + GROUND;
        let z = p.z;
        if !(-0.75..=2.2).contains(&z) || h < 0.6 {
            return 0.35_f32.max(-0.75 - z).max(z - 2.2).max(0.6 - h);
        }
        let hb = self.belt.at(z);
        let wb = self.gh_w.at(z);
        let dh = (h - hb).max(0.0);
        let wg = wb - 0.30 * dh - 0.35 * dh * dh;
        let slope = 0.30 + 0.7 * dh;
        let d_side = (ax - wg) / (1.0 + slope * slope).sqrt();
        let (rc, drc) = self.roof.eval(z);
        let r = rc - 0.055 * (ax / 0.6).powi(2);
        let d_top = (h - r) / (1.0 + drc * drc + (0.18 * ax / 0.36).powi(2)).sqrt();
        let d = rmax(d_side, d_top, 0.05);
        d.max(hb - 0.14 - h)
    }

    /// The whole shell with every opening (lamps, grille, intakes, vents, exhausts) carved.
    pub fn shell(&self, p: Vec3) -> f32 {
        let plain = self.shell_plain(p);
        if plain > 0.05 {
            return plain;
        }
        self.features.carve(p, plain, &|q| self.shell_plain(q))
    }

    /// The shell before the openings are carved (where lamp lenses and grilles sit).
    pub fn shell_plain(&self, p: Vec3) -> f32 {
        let lower = self.lower(p);
        let gh = self.greenhouse(p);
        let mut d = rmin(lower, gh, 0.018);
        let ax = p.x.abs();
        let h = p.y + GROUND;
        for (zc, r) in [(FRONT_AXLE, ARCH_F), (REAR_AXLE, ARCH_R)] {
            let rho = Vec2::new(p.z - zc, h - WHEEL_H).length();
            let well = (rho - r).max(LINER_X - ax);
            d = rmax(d, -well, 0.012);
        }
        d
    }
}

/// Whether a surface point sits inside a wheel well (for the liner material).
pub fn in_wheel_well(p: Vec3, slack: f32) -> bool {
    let h = p.y + GROUND;
    [(FRONT_AXLE, ARCH_F), (REAR_AXLE, ARCH_R)]
        .iter()
        .any(|&(zc, r)| {
            Vec2::new(p.z - zc, h - WHEEL_H).length() < r + slack && p.x.abs() > LINER_X - 0.02
        })
}

/// Where the glass goes: the windshield and backlight (outlined in plan view, x–z) and the
/// side windows (outlined in side view, z–h), each with the black frit band width.
pub struct Windows {
    pub windshield: Pane,
    pub back: Pane,
    pub side: Pane,
}

impl Body {
    /// Greenhouse half-width at `(z, h)`.
    pub fn gh_side(&self, z: f32, h: f32) -> f32 {
        let dh = (h - self.belt.at(z)).max(0.0);
        self.gh_w.at(z) - 0.30 * dh - 0.35 * dh * dh
    }

    /// Roof height at `(z, |x|)`.
    pub fn roof_at(&self, z: f32, ax: f32) -> f32 {
        self.roof.at(z) - 0.055 * (ax / 0.6).powi(2)
    }

    /// Where the greenhouse top meets its side at `z`: (|x|, h).
    pub fn corner(&self, z: f32) -> (f32, f32) {
        let mut x = self.gh_w.at(z) - 0.1;
        for _ in 0..8 {
            x = self.gh_side(z, self.roof_at(z, x));
        }
        (x, self.roof_at(z, x))
    }

    /// Height of the beltline at `z`.
    pub fn belt_at(&self, z: f32) -> f32 {
        self.belt.at(z)
    }

    pub fn windows(&self) -> Windows {
        let n = 40;
        // Windshield: cowl to header, between the A-pillars.
        let (z0, z1) = (-0.215, 0.285);
        let mut lo = Vec::new();
        let mut hi = Vec::new();
        for i in 0..=n {
            let s = i as f32 / n as f32 * 2.0 - 1.0;
            let edge = |z: f32, bend: f32| {
                let xm = self.corner(z).0 - 0.085;
                let x = s * xm;
                Vec2::new(x, z + bend * (x / 0.62).powi(2))
            };
            lo.push(edge(z0, 0.07));
            hi.push(edge(z1, 0.035));
        }
        // Rebuild the long edges along the pillars so they follow the corner line.
        let windshield = Pane::from_edges(lo, hi, 0.05, Proj::Top, GROUND);
        // Backlight: roof edge to the deck, between the buttresses.
        let (b0, b1) = (1.0, 1.77);
        let mut lo = Vec::new();
        let mut hi = Vec::new();
        for i in 0..=n {
            let s = i as f32 / n as f32 * 2.0 - 1.0;
            let edge = |z: f32, bend: f32| {
                let xm = self.corner(z).0 - 0.095;
                let x = s * xm;
                Vec2::new(x, z + bend * (x / 0.55).powi(2))
            };
            lo.push(edge(b0, 0.05));
            hi.push(edge(b1, -0.10));
        }
        let back = Pane::from_edges(lo, hi, 0.045, Proj::Top, GROUND);
        // Side glass: beltline up to the roof rail, from the A-pillar back to a swept tip.
        let top = |z: f32| {
            let (_, hc) = self.corner(z - 0.075);
            (hc - 0.045).min(0.975 + (1.50 - z) * 1.25)
        };
        let bot = |z: f32| self.belt.at(z) + 0.012;
        let mut zs = -0.4f32;
        while top(zs) < bot(zs) + 0.004 {
            zs += 0.002;
        }
        let mut ze = 1.8f32;
        while top(ze) < bot(ze) + 0.004 {
            ze -= 0.002;
        }
        let mut lo = Vec::new();
        let mut hi = Vec::new();
        for i in 0..=n * 2 {
            let z = zs + (ze - zs) * i as f32 / (n * 2) as f32;
            lo.push(Vec2::new(z, bot(z)));
            hi.push(Vec2::new(z, top(z).max(bot(z))));
        }
        let side = Pane::from_edges(lo, hi, 0.032, Proj::Side, GROUND);
        Windows {
            windshield,
            back,
            side,
        }
    }
}

/// How far past an opening's outline its rounded lip can still sit below the plain skin
/// (the outline offset plus the rounding, with room for grazing angles), m.
pub const LIP: f32 = 0.05;

/// What a recessed surface point is part of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Headlight,
    Grille,
    Intake,
    TailLight,
    Exhaust,
    Vent,
}

/// Openings carved into the body, outlined in front view (x, h), rear view (x, h) or side
/// view (z, h); right side only (mirrored).
#[derive(Default)]
pub struct Features {
    pub headlight: Vec<Vec2>,
    pub grille: Vec<Vec2>,
    pub intake: Vec<Vec2>,
    pub tail: Vec<Vec2>,
    pub vent: Vec<Vec2>,
    /// Exhaust tip centers (|x|, h) and radius.
    pub exhaust: (Vec2, f32),
}

fn poly(points: &[(f32, f32)]) -> Vec<Vec2> {
    points.iter().map(|&(a, b)| Vec2::new(a, b)).collect()
}

impl Features {
    pub fn new() -> Self {
        Self {
            headlight: poly(&[
                (0.45, 0.575),
                (0.52, 0.61),
                (0.63, 0.626),
                (0.77, 0.627),
                (0.855, 0.603),
                (0.865, 0.57),
                (0.79, 0.543),
                (0.63, 0.535),
                (0.52, 0.548),
            ]),
            grille: poly(&[
                (-0.37, 0.235),
                (0.37, 0.235),
                (0.43, 0.30),
                (0.395, 0.425),
                (0.31, 0.45),
                (-0.31, 0.45),
                (-0.395, 0.425),
                (-0.43, 0.30),
            ]),
            intake: poly(&[
                (0.575, 0.215),
                (0.78, 0.195),
                (0.83, 0.25),
                (0.81, 0.33),
                (0.67, 0.355),
                (0.585, 0.325),
            ]),
            tail: poly(&[
                (0.0, 0.874),
                (0.80, 0.872),
                (0.875, 0.888),
                (0.875, 0.918),
                (0.80, 0.912),
                (0.0, 0.914),
            ]),
            vent: poly(&[
                (-0.95, 0.465),
                (-0.84, 0.465),
                (-0.715, 0.635),
                (-0.825, 0.635),
            ]),
            exhaust: (Vec2::new(0.50, 0.255), 0.052),
        }
    }

    /// Which opening a point (already known to lie below the plain surface) belongs to.
    pub fn at(&self, p: Vec3) -> Option<Feature> {
        self.near(p, 0.0)
    }

    /// The opening nearest `p` whose outline, grown by `margin` (m), contains it.
    pub fn near(&self, p: Vec3, margin: f32) -> Option<Feature> {
        let ax = p.x.abs();
        let h = p.y + GROUND;
        let fx = Vec2::new(ax, h);
        let mut best: Option<(f32, Feature)> = None;
        let mut consider = |d: f32, f: Feature| {
            if d < margin && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, f));
            }
        };
        if p.z < -1.5 {
            consider(sd_polygon(fx, &self.headlight) - 0.012, Feature::Headlight);
            consider(
                sd_polygon(Vec2::new(p.x, h), &self.grille) - 0.03,
                Feature::Grille,
            );
            consider(sd_polygon(fx, &self.intake) - 0.02, Feature::Intake);
        }
        if p.z > 1.7 {
            consider(
                (fx - self.exhaust.0).length() - self.exhaust.1 - 0.02,
                Feature::Exhaust,
            );
        }
        if ax > 0.6 {
            consider(
                sd_polygon(Vec2::new(p.z, h), &self.vent) - 0.015,
                Feature::Vent,
            );
        }
        best.map(|(_, f)| f)
    }

    /// Carve the openings into `d` (the shell field) given the plain shell value `plain`.
    pub fn carve(&self, p: Vec3, plain: f32, skin: &(dyn Fn(Vec3) -> f32 + Sync)) -> f32 {
        let ax = p.x.abs();
        let h = p.y + GROUND;
        let mut d = plain;
        let pocket = |d: f32, outline: f32, depth: f32, r: f32| {
            let region = outline.max(-(plain + depth));
            rmax(d, -region, r)
        };
        if p.z < -1.55 {
            // The nose openings are read on the skin under `p`, so their walls stand square to
            // the raked panels round them (not inside a wheel well, whose wall is skin too).
            let q = if plain > -0.15 && self.near(p, 0.2).is_some() && !in_wheel_well(p, 0.1) {
                on_surface(skin, p, plain)
            } else {
                p
            };
            let (fx, front) = (
                Vec2::new(q.x.abs(), q.y + GROUND),
                Vec2::new(q.x, q.y + GROUND),
            );
            d = pocket(d, sd_polygon(fx, &self.headlight) - 0.004, 0.075, 0.01);
            d = pocket(d, sd_polygon(front, &self.grille) - 0.025, 0.11, 0.014);
            d = pocket(d, sd_polygon(fx, &self.intake) - 0.015, 0.08, 0.012);
        }
        if p.z > 1.75 {
            let fx = Vec2::new(ax, h);
            d = pocket(
                d,
                (fx - self.exhaust.0).length() - self.exhaust.1,
                0.25,
                0.006,
            );
        }
        if ax > 0.6 && (-1.05..-0.6).contains(&p.z) && (0.4..0.7).contains(&h) {
            d = pocket(
                d,
                sd_polygon(Vec2::new(p.z, h), &self.vent) - 0.012,
                0.03,
                0.008,
            );
        }
        d
    }
}
