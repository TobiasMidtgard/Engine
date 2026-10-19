//! The rally car: a compact three-door WRC-style hatchback (short hood, raked windshield, roof
//! falling into a steep hatch, bolt-on box flares over long-travel wheels), sculpted as a
//! signed-distance field like the sports car. Roof scoop, roof vent and hood louvres are part
//! of the field.
//!
//! Body frame: x right, y up, −z forward, meters; the ground is at y = −[`GROUND`]; profiles
//! use the height above the ground (`h`).

use engine_core::math::{Vec2, Vec3};

use super::details::Proj;
use super::glass::Pane;
use super::sdf::*;
use super::sports::{Feature, LIP};

pub const GROUND: f32 = 0.45;
pub const FRONT_AXLE: f32 = -1.25;
pub const REAR_AXLE: f32 = 1.25;
pub const WHEEL_H: f32 = 0.32;
/// Wheel-arch opening "radius" (a rounded square, see [`arch_rho`]).
pub const ARCH: f32 = 0.405;
pub const LINER_X: f32 = 0.60;

/// Distance from the wheel center in side view, a rounded square above the axle (box flares)
/// and round below.
#[inline]
pub fn arch_rho(dz: f32, dh: f32) -> f32 {
    if dh > 0.0 {
        let (a, b) = (dz.abs(), dh);
        (a * a * a + b * b * b).cbrt()
    } else {
        (dz * dz + dh * dh).sqrt()
    }
}

pub struct Body {
    hood: Table,
    plan: Table,
    hmax: Table,
    floor: Table,
    nose: Table,
    tail: Table,
    roof: Table,
    gh_w: Table,
    belt: Table,
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
        let mut b = Self {
            hood: Table::of(&[
                (-2.05, 0.64),
                (-1.98, 0.728),
                (-1.85, 0.785),
                (-1.6, 0.84),
                (-1.2, 0.878),
                (-0.85, 0.902),
                (-0.5, 0.918),
                (0.5, 0.935),
                (1.5, 0.958),
                (1.95, 0.97),
                (2.05, 0.95),
            ]),
            plan: Table::of(&[
                (-2.05, 0.70),
                (-1.98, 0.78),
                (-1.85, 0.835),
                (-1.6, 0.86),
                (-1.0, 0.865),
                (0.0, 0.87),
                (1.0, 0.87),
                (1.6, 0.865),
                (1.85, 0.85),
                (1.98, 0.80),
                (2.04, 0.74),
            ]),
            hmax: Table::of(&[(-2.05, 0.50), (0.0, 0.55), (2.05, 0.56)]),
            floor: Table::of(&[
                (-2.05, 0.38),
                (-1.9, 0.27),
                (-1.6, 0.22),
                (1.6, 0.22),
                (1.85, 0.28),
                (2.04, 0.36),
            ]),
            nose: Table::of(&[
                (0.22, -1.90),
                (0.30, -1.98),
                (0.40, -2.03),
                (0.52, -2.05),
                (0.62, -2.04),
                (0.70, -2.00),
                (0.80, -1.92),
            ]),
            tail: Table::of(&[
                (0.25, 1.92),
                (0.33, 1.98),
                (0.45, 2.01),
                (0.62, 2.02),
                (0.80, 2.00),
                (0.95, 1.975),
                (1.06, 1.94),
            ]),
            roof: Table::of(&[
                (-0.95, 0.90),
                (-0.80, 0.975),
                (-0.55, 1.10),
                (-0.30, 1.215),
                (-0.05, 1.30),
                (0.15, 1.34),
                (0.40, 1.357),
                (0.80, 1.36),
                (1.20, 1.345),
                (1.50, 1.32),
                (1.66, 1.295),
                (1.76, 1.255),
                (1.84, 1.19),
                (1.90, 1.105),
                (1.94, 1.025),
                (1.97, 0.95),
            ]),
            gh_w: Table::of(&[
                (-0.95, 0.78),
                (0.0, 0.79),
                (1.2, 0.78),
                (1.7, 0.75),
                (1.98, 0.71),
            ]),
            belt: Table::of(&[
                (-0.95, 0.928),
                (0.0, 0.942),
                (1.0, 0.962),
                (1.6, 0.98),
                (1.98, 0.99),
            ]),
            corner_x: Table::from_fn(0.0, 1.0, 2, |_| (0.0, 0.0)),
            features: Features::new(),
        };
        let designed = b.gh_w.clone();
        let flush = |z: f32| designed.at(z).min(b.side_w(z, b.belt.at(z)).0 - 0.006);
        b.gh_w = Table::from_fn(-1.0, 2.05, 1200, |z| {
            (flush(z), (flush(z + 1e-3) - flush(z - 1e-3)) / 2e-3)
        });
        let cx = |z: f32| b.corner(z).0;
        b.corner_x = Table::from_fn(-1.0, 2.05, 600, |z| (cx(z), 0.0));
        b
    }

    /// Half-width of the lower body at `(z, h)` and its slopes along z and h.
    #[inline]
    fn side_w(&self, z: f32, h: f32) -> (f32, f32, f32) {
        let (wp, dwp) = self.plan.eval(z);
        let hm = self.hmax.at(z);
        let (c, dh) = if h > hm {
            (0.35 * (h - hm) * (h - hm), 0.7 * (h - hm))
        } else {
            (0.5 * (hm - h) * (hm - h), -(hm - h))
        };
        // Box flares over the arches and a rolled lip at the opening.
        let mut flare = 0.0;
        for zc in [FRONT_AXLE, REAR_AXLE] {
            let rho = arch_rho(z - zc, h - WHEEL_H);
            flare += 0.072 * sstep(ARCH + 0.19, ARCH + 0.10, rho) * sstep(0.10, 0.20, h);
            flare += 0.010 * bump(ARCH + 0.022, 0.035, rho) * sstep(0.12, 0.25, h);
        }
        (wp - c + flare, dwp, -dh)
    }

    #[inline]
    fn top_h(&self, z: f32, ax: f32) -> (f32, f32, f32) {
        let (tc, dtc) = self.hood.eval(z);
        let wp = self.plan.at(z);
        let u = (ax / wp).min(1.0);
        let crown = 0.014 * (1.0 - u * u);
        // Fender ridges either side of the hood.
        let ridge = 0.012 * bump(0.80, 0.12, u) * (1.0 - sstep(-0.9, -0.7, z));
        (tc + crown + ridge, dtc, -0.028 * u / wp)
    }

    #[inline]
    pub fn lower(&self, p: Vec3) -> f32 {
        let ax = p.x.abs();
        let h = p.y + GROUND;
        let z = p.z;
        let (w, wz, wh) = self.side_w(z, h);
        let d_side = (ax - w) / (1.0 + wz * wz + wh * wh).sqrt();
        let (t, tz, tx) = self.top_h(z, ax);
        let d_top = (h - t) / (1.0 + tz * tz + tx * tx).sqrt();
        let mut d = rmax(d_side, d_top, 0.05);
        let (zf, dzf) = self.nose.eval(h);
        let plan_f = 0.20 * (ax / 0.9).powi(2);
        let d_front = (zf + plan_f - z) / (1.0 + dzf * dzf + (0.4 * ax / 0.85).powi(2)).sqrt();
        d = rmax(d, d_front, 0.07);
        let (zr, dzr) = self.tail.eval(h);
        let plan_r = 0.15 * (ax / 0.9).powi(2);
        let d_rear = (z - zr + plan_r) / (1.0 + dzr * dzr + (0.3 * ax / 0.85).powi(2)).sqrt();
        d = rmax(d, d_rear, 0.06);
        let (b, db) = self.floor.eval(z);
        let d_bot = (b - h) / (1.0 + db * db).sqrt();
        rmax(d, d_bot, 0.035)
    }

    /// Half-width of the lower body (flares included) at `(z, h)`.
    pub fn half_width(&self, z: f32, h: f32) -> f32 {
        self.side_w(z, h).0
    }

    /// Greenhouse half-width at `(z, h)`.
    pub fn gh_side(&self, z: f32, h: f32) -> f32 {
        let dh = (h - self.belt.at(z)).max(0.0);
        self.gh_w.at(z) - 0.22 * dh - 0.30 * dh * dh
    }

    /// Roof height at `(z, |x|)`.
    pub fn roof_at(&self, z: f32, ax: f32) -> f32 {
        self.roof.at(z) - 0.05 * (ax / 0.6).powi(2)
    }

    /// Where the greenhouse top meets its side at `z`: (|x|, h).
    pub fn corner(&self, z: f32) -> (f32, f32) {
        let mut x = self.gh_w.at(z) - 0.1;
        for _ in 0..8 {
            x = self.gh_side(z, self.roof_at(z, x));
        }
        (x, self.roof_at(z, x))
    }

    pub fn belt_at(&self, z: f32) -> f32 {
        self.belt.at(z)
    }

    #[inline]
    pub fn greenhouse(&self, p: Vec3) -> f32 {
        let ax = p.x.abs();
        let h = p.y + GROUND;
        let z = p.z;
        if !(-1.05..=2.05).contains(&z) || h < 0.75 {
            return 0.35_f32.max(-1.05 - z).max(z - 2.05).max(0.75 - h);
        }
        let hb = self.belt.at(z);
        let wb = self.gh_w.at(z);
        let dh = (h - hb).max(0.0);
        let wg = wb - 0.22 * dh - 0.30 * dh * dh;
        let slope = 0.22 + 0.6 * dh;
        let d_side = (ax - wg) / (1.0 + slope * slope).sqrt();
        let (rc, drc) = self.roof.eval(z);
        let r = rc - 0.05 * (ax / 0.6).powi(2);
        let d_top = (h - r) / (1.0 + drc * drc + (0.17 * ax / 0.36).powi(2)).sqrt();
        let d = rmax(d_side, d_top, 0.045);
        d.max(hb - 0.14 - h)
    }

    /// The roof scoop: a low intake on the roof's front, fading into the roof behind.
    #[inline]
    fn scoop(&self, p: Vec3) -> f32 {
        let (z0, z1) = (0.02, 0.58);
        if p.z < z0 - 0.1 || p.z > z1 + 0.1 || p.x.abs() > 0.3 {
            return 1.0;
        }
        let h = p.y + GROUND;
        let k = ((p.z - z0) / (z1 - z0)).clamp(0.0, 1.0);
        let top = self.roof.at(p.z) + 0.058 * (1.0 - k.powf(1.4));
        let d_top = h - top;
        let d_side = p.x.abs() - (0.17 - 0.03 * k);
        let d_front = z0 - p.z;
        let d_bot = self.roof.at(p.z) - 0.05 - h;
        rmax(rmax(rmax(d_top, d_side, 0.03), d_front, 0.02), d_bot, 0.0)
    }

    /// The shell before the openings are carved (where lenses sit).
    pub fn shell_plain(&self, p: Vec3) -> f32 {
        let lower = self.lower(p);
        let gh = self.greenhouse(p);
        let mut d = rmin(lower, gh, 0.02);
        d = smin(d, self.scoop(p), 0.025);
        let ax = p.x.abs();
        let h = p.y + GROUND;
        for zc in [FRONT_AXLE, REAR_AXLE] {
            let rho = arch_rho(p.z - zc, h - WHEEL_H);
            let well = (rho - ARCH).max(LINER_X - ax);
            d = rmax(d, -well, 0.012);
        }
        d
    }

    /// The whole shell with every opening carved.
    pub fn shell(&self, p: Vec3) -> f32 {
        let plain = self.shell_plain(p);
        if plain > 0.05 {
            return plain;
        }
        self.features.carve(self, p, plain)
    }

    /// Negative on the glass areas (`inset`: how far inside the skin the surface being cut
    /// lies).
    pub fn window_field(&self, win: &Windows, p: Vec3, inset: f32, margin: f32) -> f32 {
        let h = p.y + GROUND;
        if h < 0.9 || !(-1.0..2.05).contains(&p.z) {
            return 1.0;
        }
        let ax = p.x.abs();
        let mut side = 1.0f32;
        for pane in &win.sides {
            side = side.min(pane.sd_at(p));
        }
        let side = side.max(self.corner_x.at(p.z) - 0.03 - inset - ax);
        let ws = win.windshield.sd_at(p);
        let back = if p.z > 1.6 { win.back.sd_at(p) } else { 1.0 };
        side.min(ws).min(back).max(0.90 - h) + margin
    }

    /// Negative on the wheel-well surfaces (liner).
    pub fn liner_field(&self, p: Vec3) -> f32 {
        let h = p.y + GROUND;
        let ax = p.x.abs();
        let mut f = 1.0f32;
        for zc in [FRONT_AXLE, REAR_AXLE] {
            let rho = arch_rho(p.z - zc, h - WHEEL_H);
            if rho > ARCH + 0.1 {
                continue;
            }
            let w = self.side_w(p.z, h).0;
            f = f.min((rho - ARCH - 0.02).max(ax - (w - 0.005)));
        }
        f
    }

    /// Negative inside carved openings (more than 4 mm below the plain skin). Continuous
    /// across each opening's rounded lip: the skin is only skipped (1.0) well clear of every
    /// opening, where it is never below the plain surface anyway.
    pub fn pocket_field(&self, p: Vec3) -> f32 {
        if self.features.near(self, p, LIP).is_none() {
            return 1.0;
        }
        self.shell_plain(p) + 0.004
    }

    /// Negative on the lower trim (bumper lips, sills, rear valance, underside).
    pub fn trim_field(&self, p: Vec3) -> f32 {
        let h = p.y + GROUND;
        let z = p.z;
        let line = if z < 0.0 {
            0.26 - 0.02 * sstep(-1.95, -1.70, z)
        } else {
            0.24 + 0.08 * sstep(1.65, 1.90, z)
        };
        h - line
    }

    pub fn windows(&self) -> Windows {
        let n = 32;
        // Windshield (plan view): cowl to header between the A-pillars.
        let (z0, z1) = (-0.79, -0.08);
        let mut lo = Vec::new();
        let mut hi = Vec::new();
        for i in 0..=n {
            let s = i as f32 / n as f32 * 2.0 - 1.0;
            let edge = |z: f32, bend: f32| {
                let xm = self.corner(z).0 - 0.075;
                let x = s * xm;
                Vec2::new(x, z + bend * (x / 0.65).powi(2))
            };
            lo.push(edge(z0, 0.05));
            hi.push(edge(z1, 0.02));
        }
        let windshield = Pane::from_edges(lo, hi, 0.05, Proj::Top, GROUND);
        // Hatch glass (rear view).
        let mut lo = Vec::new();
        let mut hi = Vec::new();
        for i in 0..=n {
            let s = i as f32 / n as f32 * 2.0 - 1.0;
            lo.push(Vec2::new(s * 0.56, 1.045 + 0.02 * s * s));
            hi.push(Vec2::new(s * 0.50, 1.255 - 0.01 * s * s));
        }
        let back = Pane::from_edges(lo, hi, 0.045, Proj::Back, GROUND);
        // Side glass (side view): front door, rear door, quarter light.
        let top = |z: f32| {
            let (_, hc) = self.corner(z - 0.06);
            hc - 0.05
        };
        let bot = |z: f32| self.belt.at(z) + 0.012;
        let mut zs = -0.75f32;
        while top(zs) < bot(zs) + 0.004 {
            zs += 0.002;
        }
        let mut sides = Vec::new();
        for (a, b) in [(zs, 0.52f32), (0.64, 1.50)] {
            let mut lo = Vec::new();
            let mut hi = Vec::new();
            for i in 0..=n {
                let z = a + (b - a) * i as f32 / n as f32;
                // The quarter light tapers toward the rear.
                let t = if a > 0.6 {
                    top(z).min(bot(z) + (b - z) * 0.9)
                } else {
                    top(z)
                };
                lo.push(Vec2::new(z, bot(z)));
                hi.push(Vec2::new(z, t.max(bot(z))));
            }
            sides.push(Pane::from_edges(lo, hi, 0.03, Proj::Side, GROUND));
        }
        Windows {
            windshield,
            back,
            sides,
        }
    }
}

pub struct Windows {
    pub windshield: Pane,
    pub back: Pane,
    pub sides: Vec<Pane>,
}

/// Openings carved into the body (right side only where mirrored).
pub struct Features {
    /// Front view (|x|, h).
    pub headlight: Vec<Vec2>,
    /// Front view (x, h), full width.
    pub grille: Vec<Vec2>,
    pub intake: Vec<Vec2>,
    /// Front view (|x|, h): bumper corner vents.
    pub corner: Vec<Vec2>,
    /// Top view (|x|, z): hood louvres.
    pub louvre: Vec<Vec2>,
    /// Top view: roof vent center z and half extents (x, z).
    pub roof_vent: (f32, Vec2),
    /// Exhaust center (x, h) and radius.
    pub exhaust: (Vec2, f32),
}

fn poly(points: &[(f32, f32)]) -> Vec<Vec2> {
    points.iter().map(|&(a, b)| Vec2::new(a, b)).collect()
}

impl Default for Features {
    fn default() -> Self {
        Self::new()
    }
}

impl Features {
    pub fn new() -> Self {
        Self {
            headlight: poly(&[
                (0.50, 0.668),
                (0.58, 0.725),
                (0.80, 0.742),
                (0.858, 0.705),
                (0.855, 0.648),
                (0.74, 0.628),
                (0.56, 0.632),
            ]),
            grille: poly(&[
                (-0.42, 0.605),
                (0.42, 0.605),
                (0.47, 0.66),
                (0.43, 0.712),
                (-0.43, 0.712),
                (-0.47, 0.66),
            ]),
            intake: poly(&[
                (-0.56, 0.27),
                (0.56, 0.27),
                (0.64, 0.33),
                (0.60, 0.50),
                (-0.60, 0.50),
                (-0.64, 0.33),
            ]),
            corner: poly(&[(0.69, 0.33), (0.83, 0.345), (0.845, 0.44), (0.71, 0.465)]),
            louvre: poly(&[(0.18, -1.56), (0.50, -1.60), (0.52, -1.30), (0.20, -1.25)]),
            roof_vent: (1.19, Vec2::new(0.12, 0.07)),
            exhaust: (Vec2::new(0.0, 0.31), 0.058),
        }
    }

    /// Signed distance to the roof vent outline in plan view.
    pub fn roof_vent_sd(&self, p: Vec3) -> f32 {
        let (zc, half) = self.roof_vent;
        sd_round_rect(Vec2::new(p.x, p.z - zc), half, 0.02)
    }

    /// The roof vent outline in plan view (x, z).
    pub fn roof_vent_outline(&self) -> Vec<Vec2> {
        let (zc, half) = self.roof_vent;
        poly(&[
            (-half.x, zc - half.y),
            (half.x, zc - half.y),
            (half.x, zc + half.y),
            (-half.x, zc + half.y),
        ])
    }

    /// The roof scoop's intake mouth: negative inside the opening.
    pub fn scoop_mouth(body: &Body, p: Vec3) -> f32 {
        let h = p.y + GROUND;
        let r = body.roof.at(0.02);
        (p.x.abs() - 0.13)
            .max(r + 0.01 - h)
            .max(h - (r + 0.045))
            .max(p.z - 0.16)
    }

    /// Which opening a point (below the plain surface) belongs to.
    pub fn at(&self, body: &Body, p: Vec3) -> Option<Feature> {
        self.near(body, p, 0.0)
    }

    /// The opening nearest `p` whose outline, grown by `margin` (m), contains it.
    pub fn near(&self, body: &Body, p: Vec3, margin: f32) -> Option<Feature> {
        let ax = p.x.abs();
        let h = p.y + GROUND;
        let mut best: Option<(f32, Feature)> = None;
        let mut consider = |d: f32, f: Feature| {
            if d < margin && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, f));
            }
        };
        if p.z < -1.6 {
            let fx = Vec2::new(ax, h);
            let front = Vec2::new(p.x, h);
            consider(sd_polygon(fx, &self.headlight) - 0.012, Feature::Headlight);
            consider(sd_polygon(front, &self.grille) - 0.025, Feature::Grille);
            consider(sd_polygon(front, &self.intake) - 0.025, Feature::Grille);
            consider(sd_polygon(fx, &self.corner) - 0.02, Feature::Intake);
        }
        if h > 0.8 {
            consider(
                sd_polygon(Vec2::new(ax, p.z), &self.louvre) - 0.02,
                Feature::Vent,
            );
        }
        if h > 1.3 {
            consider(self.roof_vent_sd(p) - 0.015, Feature::Vent);
            consider(Self::scoop_mouth(body, p) - 0.02, Feature::Vent);
        }
        if p.z > 1.7 {
            consider(
                (Vec2::new(p.x, h) - self.exhaust.0).length() - self.exhaust.1 - 0.02,
                Feature::Exhaust,
            );
        }
        best.map(|(_, f)| f)
    }

    /// Carve the openings into the shell field given the plain value.
    pub fn carve(&self, body: &Body, p: Vec3, plain: f32) -> f32 {
        let ax = p.x.abs();
        let h = p.y + GROUND;
        let mut d = plain;
        let pocket = |d: f32, outline: f32, depth: f32, r: f32| {
            let region = outline.max(-(plain + depth));
            rmax(d, -region, r)
        };
        if p.z < -1.7 {
            // The nose openings are read on the skin under `p`, so their walls stand square to
            // the raked panels round them.
            let q = if plain > -0.15 && self.near(body, p, 0.2).is_some() {
                on_surface(&|q| body.shell_plain(q), p, plain)
            } else {
                p
            };
            let (fx, front) = (
                Vec2::new(q.x.abs(), q.y + GROUND),
                Vec2::new(q.x, q.y + GROUND),
            );
            d = pocket(d, sd_polygon(fx, &self.headlight) - 0.004, 0.07, 0.01);
            d = pocket(d, sd_polygon(front, &self.grille) - 0.02, 0.10, 0.012);
            d = pocket(d, sd_polygon(front, &self.intake) - 0.02, 0.10, 0.014);
            d = pocket(d, sd_polygon(fx, &self.corner) - 0.012, 0.08, 0.01);
        }
        if h > 0.8 && (-1.7..-1.1).contains(&p.z) {
            d = pocket(
                d,
                sd_polygon(Vec2::new(ax, p.z), &self.louvre) - 0.01,
                0.035,
                0.008,
            );
        }
        if h > 1.3 && (1.0..1.4).contains(&p.z) {
            d = pocket(d, self.roof_vent_sd(p), 0.025, 0.006);
        }
        if h > 1.3 && (-0.2..0.4).contains(&p.z) {
            let mouth = Self::scoop_mouth(body, p);
            d = rmax(d, -mouth, 0.008);
        }
        if p.z > 1.75 {
            d = pocket(
                d,
                (Vec2::new(p.x, h) - self.exhaust.0).length() - self.exhaust.1,
                0.12,
                0.006,
            );
        }
        d
    }
}
