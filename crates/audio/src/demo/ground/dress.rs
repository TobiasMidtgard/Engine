//! Each stretch's ground and props: what lies on the trail and along its edges.

use std::f32::consts::{PI, TAU};

use engine_core::math::{Quat, Vec2, Vec3, Vec4};
use engine_render::Mesh;

use super::procgen::{
    Rand, append, blade, fbm_world, hex, leaf, rock, smooth, smooth_normals_from, tube, vert,
};
use super::ribbon::{
    self, PATH_Y, Ribbon, base, columns, ends, frame, fray, point, side_columns, verge_height,
};
use super::{Batch, Mat, Out, Part, structures};
use crate::demo::{STRETCHES, stretch_start};

/// A build job: one piece of the trail, run on the compute pool.
pub(super) type Job = Box<dyn Fn() -> Out + Send + Sync>;

/// Every job that builds the trail.
pub(super) fn jobs() -> Vec<Job> {
    let mut jobs: Vec<Job> = Vec::new();
    for i in 0..STRETCHES.len() {
        jobs.push(Box::new(move || Stretch::new(i).ground()));
        jobs.push(Box::new(move || Stretch::new(i).props()));
    }
    jobs.push(Box::new(verges));
    jobs.push(Box::new(structures::boardwalk));
    jobs.push(Box::new(structures::grating));
    jobs.push(Box::new(structures::underpass));
    jobs.push(Box::new(structures::markers));
    jobs
}

// ------------------------------------------------------------------------------ prints

#[derive(Clone, Copy, PartialEq)]
enum PrintKind {
    Boot,
    Hoof,
    Paw,
}

#[derive(Clone, Copy)]
struct Print {
    s: f64,
    x: f32,
    yaw: f32,
    kind: PrintKind,
    depth: f32,
    /// Softer, older print (wider walls, shallower rim).
    soft: f32,
}

/// Foot, paw and hoof prints of the trail's walkers, sorted along the trail.
struct Prints(Vec<Print>);

impl Prints {
    fn walkers(s0: f64, s1: f64, seed: u64, depth: f32) -> Self {
        let mut rng = Rand::new(seed);
        let mut p = Vec::new();
        let mut walk =
            |rng: &mut Rand, kind, step: f64, stance: f32, depth: f32, soft: f32, x0: f32| {
                let mut s = s0 + rng.range(0.0, step as f32) as f64;
                let mut left = rng.chance(0.5);
                while s < s1 {
                    let side = if left { -1.0 } else { 1.0 };
                    let toe_out = if kind == PrintKind::Boot {
                        0.1 * side
                    } else {
                        0.0
                    };
                    p.push(Print {
                        s: s + rng.normal(0.015) as f64,
                        x: x0 + side * stance + rng.normal(0.015),
                        yaw: toe_out + rng.normal(0.05),
                        kind,
                        depth: depth * rng.range(0.8, 1.15),
                        soft,
                    });
                    left = !left;
                    s += step * rng.range(0.94, 1.06) as f64;
                }
            };
        walk(&mut rng, PrintKind::Boot, 0.72, 0.12, depth, 0.0, 0.0);
        walk(&mut rng, PrintKind::Boot, 0.7, 0.13, depth * 0.6, 1.0, 0.07);
        walk(
            &mut rng,
            PrintKind::Boot,
            1.3,
            0.1,
            depth * 0.75,
            0.3,
            -0.03,
        );
        walk(&mut rng, PrintKind::Paw, 0.26, 0.07, depth * 0.6, 0.2, 0.02);
        walk(
            &mut rng,
            PrintKind::Hoof,
            0.45,
            0.18,
            depth * 1.35,
            0.1,
            0.0,
        );
        p.sort_by(|a, b| a.s.total_cmp(&b.s));
        Self(p)
    }

    /// Relief the prints add at (s, x): negative in them, a raised rim round them.
    fn relief(&self, s: f64, x: f32) -> f32 {
        let i = self.0.partition_point(|p| p.s < s - 0.2);
        let mut h = 0.0f32;
        let mut deepest = 0.0f32;
        for p in self.0[i..].iter().take_while(|p| p.s < s + 0.2) {
            let (sn, cs) = p.yaw.sin_cos();
            let du = (s - p.s) as f32;
            let dv = x - p.x;
            let u = du * cs + dv * sn;
            let v = -du * sn + dv * cs;
            let inside = match p.kind {
                PrintKind::Boot => boot(u, v),
                PrintKind::Hoof => ellipse_in(u, v, 0.07, 0.064),
                PrintKind::Paw => paw(u, v),
            };
            if inside < -0.03 {
                continue;
            }
            let wall = 0.008 + 0.012 * p.soft;
            let mut dip = smooth(-0.003, wall, inside);
            if p.kind == PrintKind::Boot {
                // Heel and ball deeper than the arch.
                dip *=
                    1.0 + 0.25 * (1.0 - smooth(0.0, 0.05, (u + 0.08).abs().min((u - 0.06).abs())));
            }
            if p.kind == PrintKind::Hoof {
                // The frog: a raised V in the middle of the hoof.
                let frog = (1.0 - smooth(0.0, 0.015, (v.abs() * 1.6 + u * 0.6 + 0.02).abs()))
                    * smooth(0.04, -0.03, u);
                dip *= 1.0 - 0.5 * frog;
            }
            let rim =
                (1.0 - ((inside + 0.014) / 0.014).powi(2)).max(0.0) * (inside < 0.0) as u8 as f32;
            deepest = deepest.min(-p.depth * dip);
            h += p.depth * 0.3 * (1.0 - p.soft * 0.6) * rim;
        }
        deepest + h * (1.0 + deepest / 0.02).clamp(0.0, 1.0)
    }
}

/// Signed distance-ish (meters, > 0 inside) of an ellipse.
fn ellipse_in(u: f32, v: f32, ru: f32, rv: f32) -> f32 {
    let q = ((u / ru).powi(2) + (v / rv).powi(2)).sqrt();
    (1.0 - q) * ru.min(rv)
}

/// A boot sole, toe toward +u, 28 cm long.
fn boot(u: f32, v: f32) -> f32 {
    let fore = ellipse_in(u - 0.05, v, 0.09, 0.05);
    let heel = ellipse_in(u + 0.085, v, 0.055, 0.042);
    let arch = ellipse_in(u + 0.015, v + 0.006, 0.08, 0.036);
    fore.max(heel).max(arch)
}

fn paw(u: f32, v: f32) -> f32 {
    let mut d = ellipse_in(u + 0.008, v, 0.022, 0.026);
    for (tu, tv) in [
        (0.028, -0.022),
        (0.036, -0.008),
        (0.036, 0.008),
        (0.028, 0.022),
    ] {
        d = d.max(ellipse_in(u - tu, v - tv, 0.011, 0.009));
    }
    d
}

// --------------------------------------------------------------------------- stretches

struct Stretch {
    i: usize,
    name: &'static str,
    s0: f64,
    s1: f64,
    prints: Prints,
    hollows: Vec<Hollow>,
}

#[derive(Clone, Copy)]
struct Hollow {
    s: f64,
    x: f32,
    rs: f32,
    rx: f32,
    depth: f32,
    seed: u32,
}

impl Hollow {
    /// Depth of the hollow at (s, x) (positive down), with a lumpy irregular shoreline.
    fn at(&self, s: f64, x: f32, w: Vec2) -> f32 {
        let du = (s - self.s) as f32 / self.rs;
        let dv = (x - self.x) / self.rx;
        let wobble = 1.0 + 0.28 * fbm_world(w.x, w.y, 0.35, 3, self.seed);
        let q = (du * du + dv * dv).sqrt() * wobble;
        if q >= 1.0 {
            0.0
        } else {
            self.depth * (1.0 - q * q).powf(1.4)
        }
    }

    fn water_y(&self) -> f32 {
        PATH_Y - self.depth * 0.42
    }
}

/// Snow banks pushed up beside a trodden path, `amp` meters high.
fn bank(x: f32, amp: f32, w: Vec2) -> f32 {
    let ax = x.abs();
    let lump = 0.75 + 0.5 * fbm_world(w.x, w.y, 0.6, 3, 0x5b);
    amp * smooth(0.75, 1.55, ax) * (1.0 - smooth(1.95, 2.55, ax)) * lump
}

fn ruts(s: f64, x: f32, w: Vec2) -> f32 {
    let mut h = 0.0;
    for side in [-1.0f32, 1.0] {
        let c = 0.62 * side + 0.035 * fbm_world(s as f32, side, 2.5, 2, 0x71);
        let d = (x - c).abs();
        let seed = 0x72 + (side > 0.0) as u32;
        let depth = 0.04 * (0.62 + 1.2 * fbm_world(w.x, w.y, 1.1, 2, seed)).max(0.15);
        h -= depth * (1.0 - (d / 0.12).powi(2)).max(0.0).powf(1.2);
        h += 0.012 * (1.0 - ((d - 0.17) / 0.06).powi(2)).max(0.0);
    }
    h
}

/// The level water stands at in the mud's ruts.
const RUT_WATER: f32 = PATH_Y - 0.03;

impl Stretch {
    fn new(i: usize) -> Self {
        let (name, len) = STRETCHES[i];
        let s0 = stretch_start(name).unwrap_or(0.0);
        let s1 = s0 + len;
        let depth = match name {
            "mud" => 0.022,
            "snow" => 0.03,
            "sand" => 0.016,
            "water" => 0.018,
            "grass" => 0.006,
            _ => 0.0,
        };
        let prints = if depth > 0.0 {
            Prints::walkers(s0 + 0.5, s1 - 0.5, 0x9000 + i as u64, depth)
        } else {
            Prints(Vec::new())
        };
        let mut hollows = Vec::new();
        if name == "water" {
            let mut rng = Rand::new(0x4011);
            let mut s = s0 + 1.0;
            while s < s1 - 0.8 {
                let rs = rng.range(0.4, 0.8);
                hollows.push(Hollow {
                    s,
                    x: rng.range(-0.45, 0.45),
                    rs,
                    rx: rng.range(0.32, 0.6),
                    depth: rng.range(0.034, 0.046),
                    seed: rng.seed(),
                });
                s += rs as f64 * 2.0 + rng.range(0.1, 0.5) as f64;
            }
        }
        Self {
            i,
            name,
            s0,
            s1,
            prints,
            hollows,
        }
    }

    /// Height of this stretch's own ground surface at (s, x).
    fn height(&self, s: f64, x: f32, w: Vec2) -> f32 {
        let b = base(x, w);
        let e = ends(s, self.s0, self.s1, 0.7);
        let band = 1.0 - smooth(0.9, 1.3, x.abs());
        match self.name {
            "grass" => {
                let worn = 1.0 - smooth(0.12, 0.45, x.abs());
                b - 0.008 * worn * e + self.prints.relief(s, x) * e
            }
            "mud" => {
                let lumps = fbm_world(w.x, w.y, 0.5, 3, 0x31) * 0.007
                    + fbm_world(w.x, w.y, 0.12, 2, 0x32) * 0.003;
                b + e * band * (ruts(s, x, w) + self.prints.relief(s, x) + lumps)
            }
            "snow" => {
                let amp = 0.2 * smooth(self.s0 as f32, self.s0 as f32 + 1.6, s as f32);
                let trodden = 1.0 - smooth(0.25, 0.65, x.abs());
                let soft = 0.014 + fbm_world(w.x, w.y, 0.8, 3, 0x41) * 0.006;
                b + soft * smooth(self.s0 as f32, self.s0 as f32 + 0.8, s as f32)
                    + bank(x, amp, w)
                    + e * (self.prints.relief(s, x) - 0.011 * trodden)
            }
            "ice" => b + e * band * fbm_world(w.x, w.y, 1.2, 2, 0x51) * 0.002,
            "gravel" => b + e * band * fbm_world(w.x, w.y, 0.4, 3, 0x61) * 0.004,
            "sand" => {
                let dunes = fbm_world(w.x, w.y, 1.3, 3, 0x91) * 0.01;
                let drift = 0.03
                    * smooth(0.85, 1.4, x.abs())
                    * (1.0 - smooth(1.5, 2.0, x.abs()))
                    * (0.6 + 0.6 * fbm_world(w.x, w.y, 0.9, 2, 0x92));
                b + e * (dunes * band + drift + self.prints.relief(s, x))
            }
            "leaves" => {
                let litter = 0.022 * smooth(0.8, 1.4, x.abs()) * (1.0 - smooth(1.6, 2.1, x.abs()));
                b + e * (fbm_world(w.x, w.y, 0.5, 3, 0xa1) * 0.006 * band + litter)
            }
            "water" => {
                let lumps = fbm_world(w.x, w.y, 0.45, 3, 0xb1) * 0.006;
                let hollow: f32 = self.hollows.iter().map(|h| h.at(s, x, w)).sum();
                b + e * (band * (lumps + self.prints.relief(s, x)) - hollow)
            }
            _ => b,
        }
    }

    /// Height of the snow along the ice stretch's edges, continuing the snow stretch's banks
    /// and dying away toward the gravel.
    fn ice_edge_height(&self, s: f64, x: f32, w: Vec2) -> f32 {
        let t = ((s - self.s0) / (self.s1 - self.s0)) as f32;
        let amp = 0.2 - 0.13 * smooth(0.0, 0.35, t);
        let soft = 0.014 + fbm_world(w.x, w.y, 0.8, 3, 0x41) * 0.006;
        let fade = 1.0 - smooth(self.s1 as f32 - 1.2, self.s1 as f32 + 0.4, s as f32);
        base(x, w) + (soft + bank(x, amp, w)) * fade + 0.002
    }

    /// The ground's top where props stand: this stretch's surface on the path, the verge off it.
    fn ground_y(&self, s: f64, x: f32) -> f32 {
        let p = point(s, x, 0.0);
        let w = Vec2::new(p.x, p.z);
        let (solid, _) = self.cover();
        if self.name == "ice" && x.abs() > 1.05 {
            return self.ice_edge_height(s, x, w);
        }
        match self.name {
            "wood" | "metal" | "concrete" if x.abs() < 1.45 => PATH_Y,
            _ if x.abs() <= solid + 0.1 => self.height(s, x, w),
            _ => verge_height(x, w).max(self.height(s, x, w) * (self.name == "snow") as u8 as f32),
        }
    }

    /// Solid half width and ragged limit of this stretch's ground.
    fn cover(&self) -> (f32, f32) {
        match self.name {
            "grass" => (1.2, 1.5),
            "snow" => (2.0, 2.6),
            "ice" => (1.0, 1.25),
            "gravel" => (1.31, 1.33),
            "sand" | "leaves" => (1.2, 1.9),
            _ => (1.05, 1.5),
        }
    }

    fn material(&self) -> Mat {
        match self.name {
            "grass" => Mat::Verge,
            "mud" => Mat::Mud,
            "snow" => Mat::Snow,
            "ice" => Mat::Ice,
            "gravel" => Mat::Gravel,
            "sand" => Mat::Sand,
            "leaves" => Mat::Forest,
            _ => Mat::WetMud,
        }
    }

    /// The stretch's ground ribbons.
    fn ground(&self) -> Out {
        let mut out = Out::default();
        match self.name {
            // Built structures lay their own ground (see `structures`).
            "wood" | "metal" | "concrete" => return out,
            _ => {}
        }
        let (solid, ragged) = self.cover();
        let fine = matches!(self.name, "mud" | "snow" | "sand" | "water" | "grass");
        let cols = if fine {
            columns(ragged, 0.02, 0.55, 0.035, 1.25, 0.06)
        } else {
            columns(ragged, 0.04, 1.2, 0.04, 1.2, 0.06)
        };
        let fade_in = match self.name {
            "gravel" => 0.45,
            "ice" => 0.6,
            "sand" => 0.7,
            _ => 0.8,
        };
        let cover = move |x: f32| fray(x, solid, ragged);
        let height = |s: f64, x: f32, w: Vec2| self.height(s, x, w);
        let tint = |s: f64, x: f32, w: Vec2| self.tint(s, x, w);
        out.parts.extend(ribbon::ribbon(&Ribbon {
            mat: self.material(),
            s0: self.s0,
            s1: self.s1 + 0.04,
            fade_in,
            fade_out: 0.0,
            ds: if fine { 0.02 } else { 0.04 },
            columns: cols,
            cover: &cover,
            height: &height,
            tint: &tint,
            shadows: matches!(self.name, "snow" | "mud" | "sand" | "water"),
        }));
        match self.name {
            "grass" => {
                // The worn earth line down the middle.
                let cover = |x: f32| fray(x, 0.1, 0.4);
                let h = |s: f64, x: f32, w: Vec2| self.height(s, x, w) + 0.0015;
                let tint = |_: f64, _: f32, w: Vec2| {
                    Vec3::splat(0.9 + 0.15 * fbm_world(w.x, w.y, 1.5, 2, 0x13))
                };
                out.parts.extend(ribbon::ribbon(&Ribbon {
                    mat: Mat::Dirt,
                    s0: self.s0 + 0.6,
                    s1: self.s1 - 0.8,
                    fade_in: 1.2,
                    fade_out: 1.2,
                    ds: 0.02,
                    columns: columns(0.5, 0.02, 0.6, 0.02, 0.6, 0.02),
                    cover: &cover,
                    height: &h,
                    tint: &tint,
                    shadows: false,
                }));
            }
            "ice" => {
                // Frost and snow along both edges, creeping onto the ice.
                let cover = |x: f32| {
                    let ax = x.abs();
                    if ax < 1.15 {
                        1.0 - 0.55 * ((1.15 - ax) / 0.3).min(1.0)
                    } else {
                        fray(x, 2.05, 2.5)
                    }
                };
                let h = |s: f64, x: f32, w: Vec2| self.ice_edge_height(s, x, w);
                let tint = |_: f64, x: f32, w: Vec2| snow_tint(x, w);
                for side in [-1.0, 1.0] {
                    out.parts.extend(ribbon::ribbon(&Ribbon {
                        mat: Mat::Snow,
                        s0: self.s0 - 0.04,
                        s1: self.s1 - 0.6,
                        fade_in: 0.0,
                        fade_out: 1.0,
                        ds: 0.04,
                        columns: side_columns(0.85, 2.5, 0.05, side),
                        cover: &cover,
                        height: &h,
                        tint: &tint,
                        shadows: true,
                    }));
                }
            }
            "mud" => {
                // Water standing in the ruts.
                let cover = |_: f32| 1.0;
                let h = |_: f64, _: f32, _: Vec2| RUT_WATER;
                let tint = |_: f64, _: f32, _: Vec2| Vec3::ONE;
                for side in [-1.0, 1.0] {
                    out.parts.extend(ribbon::ribbon(&Ribbon {
                        mat: Mat::Water,
                        s0: self.s0 + 0.8,
                        s1: self.s1 - 0.8,
                        fade_in: 0.0,
                        fade_out: 0.0,
                        ds: 0.1,
                        columns: side_columns(0.45, 0.8, 0.05, side),
                        cover: &cover,
                        height: &h,
                        tint: &tint,
                        shadows: false,
                    }));
                }
            }
            "water" => {
                for h in &self.hollows {
                    out.parts.push(puddle(h));
                }
            }
            _ => {}
        }
        out
    }

    /// Vertex tint of the stretch's ground: large, soft variation that breaks up tiling.
    fn tint(&self, s: f64, x: f32, w: Vec2) -> Vec3 {
        let n = fbm_world(w.x, w.y, 2.2, 3, 0x77 + self.i as u32);
        let ax = x.abs();
        match self.name {
            "grass" => {
                let worn = 1.0 - smooth(0.2, 0.7, ax);
                Vec3::new(1.0, 0.97, 0.9) * (0.92 + 0.14 * n) * (1.0 - 0.15 * worn)
            }
            "mud" => {
                let wet = 1.0 - smooth(0.0, 0.9, ax);
                let r = ruts(s, x, w);
                Vec3::splat(0.95 + 0.12 * n) * (1.0 - 0.15 * wet) * (1.0 + 3.0 * r.min(0.0))
            }
            "snow" => snow_tint(x, w) * (1.0 - 0.06 * (1.0 - smooth(0.2, 0.6, ax))),
            "ice" => {
                Vec3::splat(0.92 + 0.12 * n)
                    * Vec3::new(0.95, 1.0, 1.05)
                        .lerp(Vec3::new(1.4, 1.4, 1.4), smooth(0.75, 1.2, ax))
            }
            "gravel" => Vec3::splat(0.92 + 0.14 * n),
            "sand" => {
                Vec3::new(1.0, 0.98, 0.95)
                    * (0.94 + 0.1 * n)
                    * (1.0 - 0.06 * (1.0 - smooth(0.2, 0.6, ax)))
            }
            "leaves" => Vec3::splat(0.9 + 0.2 * n),
            "water" => {
                let hollow: f32 = self.hollows.iter().map(|h| h.at(s, x, w)).sum();
                Vec3::splat(0.92 + 0.12 * n) * (1.0 - 9.0 * hollow).max(0.55)
            }
            _ => Vec3::ONE,
        }
    }

    /// The stretch's props.
    fn props(&self) -> Out {
        let mut rng = Rand::new(0x5eed + self.i as u64 * 977);
        let mut b = Batch::default();
        let mut out = Out::default();
        match self.name {
            "grass" => self.grass_props(&mut b, &mut rng),
            "mud" => self.mud_props(&mut b, &mut rng),
            "snow" | "ice" => self.frozen_props(&mut b, &mut rng),
            "gravel" => self.gravel_props(&mut b, &mut out, &mut rng),
            "sand" => self.sand_props(&mut b, &mut out, &mut rng),
            "leaves" => self.leaf_props(&mut b, &mut rng),
            "water" => self.water_props(&mut b, &mut rng),
            _ => {}
        }
        // Verge grass along this stretch's edges.
        self.verge_grass(&mut b, &mut rng);
        out.parts.extend(b.finish());
        out
    }

    fn grass_props(&self, b: &mut Batch, rng: &mut Rand) {
        let (s0, s1) = (self.s0 - 0.3, self.s1 + 0.2);
        let area = (s1 - s0) as f32 * 2.6;
        let n = (area * 34.0) as usize;
        for _ in 0..n {
            let s = rng.range(s0 as f32, s1 as f32) as f64;
            let x = rng.range(-1.35, 1.35);
            let ax = x.abs();
            // Bare down the worn line, trampled near it, full at the edges.
            let keep = smooth(0.1, 0.5, ax).max(0.06);
            if !rng.chance(keep) {
                continue;
            }
            let trample = 1.0 - smooth(0.3, 1.0, ax);
            let tall = 0.08 + 0.3 * smooth(0.35, 1.25, ax);
            let y = self.ground_y(s, x);
            let (_, d, _) = frame(s);
            let lean_dir =
                (d * trample + Vec3::new(rng.normal(0.6), 0.0, rng.normal(0.6))).normalize_or(d);
            clump(
                b.at(s, Mat::Grass),
                rng,
                point(s, x, y - 0.004),
                ClumpStyle {
                    blades: (6 + (10.0 * (1.0 - trample * 0.5)) as usize, 18),
                    radius: 0.04,
                    height: (tall * 0.55, tall * (1.1 - 0.35 * trample)),
                    lean: (0.15 + 0.7 * trample, 0.45 + 0.8 * trample),
                    toward: Some(lean_dir),
                    width: (0.004, 0.0075),
                    palette: Palette::Meadow,
                },
            );
        }
        // Short turf between the tufts.
        for _ in 0..(area * 30.0) as usize {
            let s = rng.range(s0 as f32, s1 as f32) as f64;
            let x = rng.range(-1.35, 1.35);
            if !rng.chance(smooth(0.14, 0.55, x.abs())) {
                continue;
            }
            let y = self.ground_y(s, x);
            let h = rng.range(0.04, 0.1);
            clump(
                b.at(s, Mat::Grass),
                rng,
                point(s, x, y - 0.004),
                ClumpStyle {
                    blades: (4, 8),
                    radius: 0.035,
                    height: (h * 0.55, h),
                    lean: (0.2, 0.7),
                    toward: None,
                    width: (0.003, 0.0055),
                    palette: Palette::Meadow,
                },
            );
        }
        // Flowers and clover.
        for _ in 0..150 {
            let s = rng.range(self.s0 as f32 + 0.3, self.s1 as f32 - 0.3) as f64;
            let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
            let x = side * rng.range(0.45, 2.0);
            let y = self.ground_y(s, x);
            let kind = rng.below(3);
            flower(b.at(s, Mat::Flowers), rng, point(s, x, y - 0.003), kind);
        }
    }

    fn mud_props(&self, b: &mut Batch, rng: &mut Rand) {
        // Straw trodden into the mud.
        for _ in 0..110 {
            let s = rng.range(self.s0 as f32 + 0.5, self.s1 as f32 - 0.5) as f64;
            let x = rng.range(-1.2, 1.2);
            let y = self.ground_y(s, x);
            if y < RUT_WATER + 0.002 {
                continue;
            }
            let dir = Vec3::new(rng.normal(1.0), 0.0, rng.normal(1.0)).normalize_or(Vec3::X);
            let col = hex(0x9c8652) * rng.range(0.45, 0.9);
            blade(
                b.at(s, Mat::Straw),
                point(s, x, y - 0.002),
                dir,
                rng.range(0.06, 0.18),
                rng.range(0.003, 0.005),
                PI * 0.5 - rng.range(0.0, 0.12),
                0.0,
                rng.range(0.0, 1.0),
                col * 0.7,
                col,
                0.8,
            );
        }
        // Stones pressed into the mud.
        for _ in 0..110 {
            let s = rng.range(self.s0 as f32 + 0.3, self.s1 as f32 - 0.3) as f64;
            let x = rng.range(-1.5, 1.5);
            let y = self.ground_y(s, x);
            let size = rng.range(0.015, 0.05) * if x.abs() > 1.0 { 1.5 } else { 1.0 };
            stone(b, rng, s, point(s, x, y), size, hex(0x6f675c), 0.45);
        }
    }

    fn frozen_props(&self, b: &mut Batch, rng: &mut Rand) {
        // Frozen grass tips poking out of the snow at the edges.
        let n = ((self.s1 - self.s0) * 40.0) as usize;
        for _ in 0..n {
            let s = rng.range(self.s0 as f32, self.s1 as f32) as f64;
            let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
            let x = side * rng.range(1.75, 2.75);
            let p = point(s, x, 0.0);
            let w = Vec2::new(p.x, p.z);
            let y = if self.name == "ice" {
                self.ice_edge_height(s, x, w)
            } else {
                self.height(s, x, w)
            }
            .max(verge_height(x, w));
            clump(
                b.at(s, Mat::Grass),
                rng,
                point(s, x, y - 0.01),
                ClumpStyle {
                    blades: (4, 12),
                    radius: 0.05,
                    height: (0.05, 0.2),
                    lean: (0.1, 0.5),
                    toward: None,
                    width: (0.003, 0.006),
                    palette: Palette::Frozen,
                },
            );
        }
        if self.name == "snow" {
            // A few stones the snow hasn't covered.
            for _ in 0..10 {
                let s = rng.range(self.s0 as f32 + 1.0, self.s1 as f32 - 0.5) as f64;
                let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
                let x = side * rng.range(1.0, 2.2);
                let p = point(s, x, 0.0);
                let y = self.height(s, x, Vec2::new(p.x, p.z));
                let size = rng.range(0.08, 0.16);
                stone(b, rng, s, point(s, x, y - 0.02), size, hex(0x5d5a56), 0.5);
            }
        }
    }

    fn gravel_props(&self, b: &mut Batch, out: &mut Out, rng: &mut Rand) {
        let palette = [
            hex(0x9a948a),
            hex(0x77716a),
            hex(0xb1a99b),
            hex(0x857a6e),
            hex(0x6a6661),
            hex(0xa39278),
        ];
        // Loose stones on the bed, bigger toward the edges.
        let n = ((self.s1 - self.s0) * 190.0) as usize;
        for _ in 0..n {
            let s = rng.range(self.s0 as f32 - 0.2, self.s1 as f32) as f64;
            let x = rng.range(-1.28, 1.28);
            let edge = smooth(0.7, 1.25, x.abs());
            if x.abs() < 0.5 && rng.chance(0.4) {
                continue;
            }
            let size = rng.range(0.012, 0.03) * (1.0 + 1.6 * edge * rng.f());
            let y = self.ground_y(s, x);
            let col = palette[rng.below(palette.len())];
            stone(b, rng, s, point(s, x, y), size, col, 0.35);
        }
        // Edging boards with stakes, in short straight pieces round the bend.
        let mut s = self.s0;
        let piece = 1.1;
        while s < self.s1 - 0.05 {
            let e = (s + piece).min(self.s1);
            for side in [-1.0f32, 1.0] {
                let x = side * 1.335;
                let a = point(s, x, 0.0);
                let c = point(e, x, 0.0);
                let len = a.distance(c);
                let mid = (a + c) * 0.5;
                let dir = (c - a).normalize_or(Vec3::X);
                let rot = Quat::from_rotation_arc(Vec3::Z, dir);
                let key = format!("edging:{}", (len * 100.0).round() as i32);
                if !out.has_shared(&key) {
                    let seed = rng.seed();
                    out.shared.push((
                        key.clone(),
                        super::procgen::beveled_box(
                            Vec3::new(0.028, 0.13, len + 0.004),
                            0.005,
                            0.25,
                            Vec2::new(super::procgen::unit(seed) * 2.0, 0.1),
                            |p| {
                                let ground = 1.0 - smooth(-0.06, 0.02, p.y);
                                (Vec3::ONE
                                    * (1.0 - 0.45 * ground)
                                    * Vec3::new(1.0, 1.0 - 0.1 * ground, 1.0 - 0.25 * ground))
                                .extend(1.0)
                            },
                        ),
                    ));
                }
                out.parts.push(Part::shared(
                    &key,
                    Mat::Timber,
                    mid + Vec3::Y * 0.02,
                    rot,
                    Vec3::ONE,
                ));
                // A stake on the outside.
                let stake = point(s + 0.08, side * 1.37, 0.0);
                out.parts.push(Part::shared(
                    "stake",
                    Mat::Timber,
                    stake + Vec3::Y * 0.03,
                    Quat::from_rotation_y(rng.range(0.0, 1.0)),
                    Vec3::ONE,
                ));
            }
            s = e;
        }
        out.shared.push((
            "stake".into(),
            super::procgen::beveled_box(
                Vec3::new(0.035, 0.12, 0.035),
                0.006,
                0.5,
                Vec2::ZERO,
                |_| Vec4::new(0.8, 0.78, 0.74, 1.0),
            ),
        ));
    }

    fn sand_props(&self, b: &mut Batch, out: &mut Out, rng: &mut Rand) {
        // Pebbles and shells.
        for _ in 0..140 {
            let s = rng.range(self.s0 as f32 - 0.3, self.s1 as f32) as f64;
            let x = rng.range(-1.8, 1.8);
            let y = self.ground_y(s, x);
            let col = [hex(0x8c8378), hex(0xa79d8c), hex(0x6e675f), hex(0xb7a58a)][rng.below(4)];
            let size = rng.range(0.008, 0.035);
            stone(b, rng, s, point(s, x, y), size, col, 0.4);
        }
        for _ in 0..34 {
            let s = rng.range(self.s0 as f32, self.s1 as f32) as f64;
            let x = rng.range(-1.7, 1.7);
            let y = self.ground_y(s, x);
            shell(b.at(s, Mat::Shell), rng, point(s, x, y + 0.001));
        }
        // Dune grass tufts along the edges.
        for _ in 0..55 {
            let s = rng.range(self.s0 as f32, self.s1 as f32) as f64;
            let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
            let x = side * rng.range(1.25, 2.4);
            let y = self.ground_y(s, x);
            clump(
                b.at(s, Mat::Grass),
                rng,
                point(s, x, y - 0.01),
                ClumpStyle {
                    blades: (14, 34),
                    radius: 0.07,
                    height: (0.25, 0.75),
                    lean: (0.15, 0.5),
                    toward: None,
                    width: (0.003, 0.0055),
                    palette: Palette::Dune,
                },
            );
        }
        // A bleached piece of driftwood beside the path.
        let s = self.s0 + 5.6;
        let x = 1.75;
        let y = self.ground_y(s, x);
        out.parts
            .push(driftwood(point(s, x, y - 0.03), frame(s).1, 0xd71f));
    }

    fn leaf_props(&self, b: &mut Batch, rng: &mut Rand) {
        let autumn = [
            Vec3::new(0.62, 0.16, 0.03),
            Vec3::new(0.75, 0.33, 0.05),
            Vec3::new(0.8, 0.52, 0.1),
            Vec3::new(0.55, 0.36, 0.12),
            Vec3::new(0.42, 0.2, 0.07),
            Vec3::new(0.7, 0.6, 0.18),
            Vec3::new(0.35, 0.12, 0.05),
            Vec3::new(0.3, 0.22, 0.1),
        ];
        let n = ((self.s1 - self.s0 + 1.6) * 560.0) as usize;
        for _ in 0..n {
            let s = rng.range(self.s0 as f32 - 0.8, self.s1 as f32 + 0.8) as f64;
            let x = rng.range(-2.4, 2.4);
            let ax = x.abs();
            // Fewer leaves where the walkers tread, more piled at the edges.
            let keep = (0.25 + 0.75 * smooth(0.2, 0.9, ax)) * (1.0 - smooth(1.9, 2.5, ax));
            let edge_fade = smooth(self.s0 as f32 - 0.8, self.s0 as f32 + 0.4, s as f32)
                * (1.0 - smooth(self.s1 as f32 - 0.4, self.s1 as f32 + 0.8, s as f32));
            if !rng.chance(keep * edge_fade.max(0.15)) {
                continue;
            }
            let trodden = 1.0 - smooth(0.25, 0.7, ax);
            let y = self.ground_y(s, x) + 0.004;
            let pile = rng.range(0.0, 0.012) * smooth(0.8, 1.6, ax);
            let col =
                autumn[rng.below(autumn.len())] * rng.range(0.7, 1.2) * (1.0 - 0.35 * trodden);
            let len = rng.range(0.05, 0.11);
            let cell = rng.below(4) as u32;
            let width = len * if cell == 3 { 0.5 } else { rng.range(0.75, 1.0) };
            let tiltv = Vec3::new(rng.normal(0.35), 1.0, rng.normal(0.35))
                * Vec3::new(1.0 - trodden * 0.8, 1.0, 1.0 - trodden * 0.8);
            let rot = super::procgen::tilt(tiltv, rng.range(0.0, TAU));
            leaf(
                b.at(s, Mat::Leaf),
                point(s, x, y + pile),
                rot,
                len,
                width,
                rng.range(-0.012, 0.008) * (1.0 - trodden),
                rng.range(0.0, 0.012) * (1.0 - trodden),
                cell,
                col.extend(1.0),
            );
        }
        // Twigs.
        for _ in 0..55 {
            let s = rng.range(self.s0 as f32, self.s1 as f32) as f64;
            let x = rng.range(-2.0, 2.0);
            let y = self.ground_y(s, x) + 0.006;
            twig(b.at(s, Mat::Bark), rng, point(s, x, y));
        }
        // Mushrooms in two clusters at the edges.
        for (k, &(fs, fx)) in [(0.3f64, 1.55f32), (0.7, -1.7)].iter().enumerate() {
            let s = self.s0 + (self.s1 - self.s0) * fs;
            for _ in 0..4 + k * 2 {
                let ss = s + rng.normal(0.08) as f64;
                let x = fx + rng.normal(0.07);
                let y = self.ground_y(ss, x);
                mushroom(
                    b.at(ss, Mat::Mushroom),
                    rng,
                    point(ss, x, y - 0.006),
                    k == 1,
                );
            }
        }
    }

    fn water_props(&self, b: &mut Batch, rng: &mut Rand) {
        // Reeds and rushes along the wet edges.
        for _ in 0..70 {
            let s = rng.range(self.s0 as f32, self.s1 as f32) as f64;
            let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
            let x = side * rng.range(1.5, 2.6);
            let y = self.ground_y(s, x);
            clump(
                b.at(s, Mat::Grass),
                rng,
                point(s, x, y - 0.01),
                ClumpStyle {
                    blades: (8, 20),
                    radius: 0.05,
                    height: (0.45, 0.95),
                    lean: (0.02, 0.2),
                    toward: None,
                    width: (0.005, 0.009),
                    palette: Palette::Reed,
                },
            );
        }
        // Stones at the puddles' edges.
        for _ in 0..40 {
            let s = rng.range(self.s0 as f32, self.s1 as f32) as f64;
            let x = rng.range(-1.4, 1.4);
            let y = self.ground_y(s, x);
            let size = rng.range(0.015, 0.05);
            stone(b, rng, s, point(s, x, y), size, hex(0x6b6359), 0.4);
        }
    }

    /// Grass along the outer edges of the stretch.
    fn verge_grass(&self, b: &mut Batch, rng: &mut Rand) {
        let (from, density, palette, tall) = match self.name {
            "snow" | "ice" | "sand" => return,
            "grass" => (1.35, 1.0, Palette::Meadow, 1.0),
            "wood" => (1.32, 1.0, Palette::Meadow, 1.1),
            "metal" => (1.5, 0.9, Palette::Meadow, 1.0),
            "concrete" => (2.55, 0.5, Palette::Meadow, 0.7),
            "leaves" => (1.9, 0.45, Palette::Forest, 0.8),
            "water" => (1.45, 1.0, Palette::Lush, 1.1),
            _ => (1.4, 1.0, Palette::Meadow, 1.0),
        };
        let len = (self.s1 - self.s0) as f32;
        let n = (len * 2.0 * 10.0 * density) as usize;
        for _ in 0..n {
            let s = rng.range(self.s0 as f32, self.s1 as f32) as f64;
            let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
            let ax = from + rng.f().powf(1.8) * (3.3 - from);
            // Thinning out into the lawn.
            if !rng.chance(1.0 - smooth(2.5, 3.4, ax) * 0.8) {
                continue;
            }
            let x = side * ax;
            let y = self.ground_y(s, x);
            let h = tall * (0.16 + 0.36 * (1.0 - smooth(1.8, 3.2, ax))) * rng.range(0.6, 1.25);
            clump(
                b.at(s, Mat::Grass),
                rng,
                point(s, x, y - 0.006),
                ClumpStyle {
                    blades: (7, 16),
                    radius: 0.05,
                    height: (h * 0.5, h),
                    lean: (0.1, 0.55),
                    toward: None,
                    width: (0.0045, 0.008),
                    palette,
                },
            );
        }
        self.turf(b, rng, from, palette, 26.0 * density);
        if matches!(self.name, "grass" | "gravel" | "wood" | "water" | "mud") {
            for _ in 0..(len * 2.5) as usize {
                let s = rng.range(self.s0 as f32, self.s1 as f32) as f64;
                let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
                let x = side * rng.range(from + 0.05, 2.8);
                let y = self.ground_y(s, x);
                let kind = rng.below(3);
                flower(b.at(s, Mat::Flowers), rng, point(s, x, y - 0.004), kind);
            }
        }
    }
}

impl Stretch {
    /// Short turf fringing the path, densest at its edge: many small tufts that give the
    /// lawn's edge depth up close and hide where the path's ground frays out.
    fn turf(&self, b: &mut Batch, rng: &mut Rand, from: f32, palette: Palette, per_m2: f32) {
        let band = 1.5;
        let n = ((self.s1 - self.s0) as f32 * 2.0 * band * per_m2) as usize;
        for _ in 0..n {
            let s = rng.range(self.s0 as f32, self.s1 as f32) as f64;
            let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
            let d = rng.f().powf(1.7) * band;
            let x = side * (from - 0.08 + d);
            let y = self.ground_y(s, x);
            let h = rng.range(0.05, 0.11) * (1.0 + 0.6 * smooth(0.2, 1.2, d));
            clump(
                b.at(s, Mat::Grass),
                rng,
                point(s, x, y - 0.004),
                ClumpStyle {
                    blades: (4, 8),
                    radius: 0.035,
                    height: (h * 0.55, h),
                    lean: (0.15, 0.6),
                    toward: None,
                    width: (0.003, 0.0055),
                    palette,
                },
            );
        }
    }
}

fn snow_tint(x: f32, w: Vec2) -> Vec3 {
    let n = fbm_world(w.x, w.y, 1.8, 2, 0x4a);
    let bank = smooth(0.8, 1.5, x.abs());
    Vec3::new(1.0, 1.0, 1.02) * (0.97 + 0.04 * n) * (1.0 + 0.04 * bank)
}

/// A puddle: a level, dark water surface sitting in a hollow (the ground hides its edges).
fn puddle(h: &Hollow) -> Part {
    let (c, d, r) = frame(h.s);
    let center = c + r * h.x;
    let y = h.water_y();
    let mut m = Mesh::default();
    let n = 40;
    m.vertices.push(vert(
        Vec3::new(0.0, y, 0.0),
        Vec3::Y,
        Vec2::new(center.x, center.z),
        Vec4::ONE,
    ));
    for k in 0..=n {
        let a = k as f32 / n as f32 * TAU;
        let p = d * a.cos() * h.rs * 1.25 + r * a.sin() * h.rx * 1.25;
        m.vertices.push(vert(
            Vec3::new(p.x, y, p.z),
            Vec3::Y,
            Vec2::new(center.x + p.x, center.z + p.z),
            Vec4::ONE,
        ));
    }
    for k in 0..n as u32 {
        let (a, bb) = (1 + k, 2 + k);
        let [pa, pb] = [a, bb].map(|i| Vec3::from(m.vertices[i as usize].position));
        if pa.cross(pb).y > 0.0 {
            m.indices.extend_from_slice(&[0, a, bb]);
        } else {
            m.indices.extend_from_slice(&[0, bb, a]);
        }
    }
    Part::own(m, Mat::Water, Vec3::new(center.x, 0.0, center.z)).no_shadows()
}

// ----------------------------------------------------------------------- the verges

/// The grass verges along both sides of the whole loop (skipped where snow covers them and
/// inside the built stretches).
fn verges() -> Out {
    let mut out = Out::default();
    for (i, &(name, _)) in STRETCHES.iter().enumerate() {
        let st = Stretch::new(i);
        let from = match name {
            "snow" | "ice" => continue,
            "wood" => 1.28,
            "metal" => 1.44,
            "concrete" => 2.28,
            _ => 0.95,
        };
        let cover = |x: f32| fray(x, 2.25, 2.68);
        let h = |_: f64, x: f32, w: Vec2| verge_height(x, w);
        let tint = |_: f64, x: f32, w: Vec2| {
            let n = fbm_world(w.x, w.y, 3.0, 3, 0x99);
            let dry = smooth(0.2, 0.8, fbm_world(w.x, w.y, 5.0, 2, 0x9a));
            Vec3::new(1.0 + 0.12 * dry, 1.0, 0.9 - 0.1 * dry)
                * (0.9 + 0.16 * n)
                * (1.0 - 0.12 * (1.0 - smooth(1.0, 1.6, x.abs())))
        };
        for side in [-1.0, 1.0] {
            // Global columns so overlapping verges of neighbours match exactly.
            let cols: Vec<f32> = (0..=40)
                .map(|k| 0.9 + k as f32 * 0.045)
                .filter(|x| *x >= from - 1e-4)
                .map(|x| x * side)
                .collect();
            let mut cols = cols;
            cols.sort_by(|a, b| a.total_cmp(b));
            out.parts.extend(ribbon::ribbon(&Ribbon {
                mat: Mat::Verge,
                s0: st.s0 - 0.24,
                s1: st.s1 + 0.24,
                fade_in: 0.0,
                fade_out: 0.0,
                ds: 0.08,
                columns: cols,
                cover: &cover,
                height: &h,
                tint: &tint,
                shadows: false,
            }));
        }
    }
    out
}

// -------------------------------------------------------------------------- grasses

#[derive(Clone, Copy, PartialEq)]
enum Palette {
    Meadow,
    Lush,
    Forest,
    Frozen,
    Dune,
    Reed,
}

impl Palette {
    /// Root and tip colours of one blade.
    fn blade(self, rng: &mut Rand) -> (Vec3, Vec3) {
        let v = rng.range(0.8, 1.2);
        match self {
            Palette::Meadow | Palette::Lush | Palette::Forest => {
                let dry = match self {
                    Palette::Meadow => 0.16,
                    Palette::Lush => 0.05,
                    _ => 0.25,
                };
                if rng.chance(dry) {
                    let t = hex(0xa69661) * rng.range(0.6, 1.0);
                    (hex(0x4a4a2a) * 0.6, t)
                } else {
                    let tips = [
                        hex(0x6f9a3a),
                        hex(0x5c8a2c),
                        hex(0x86a548),
                        hex(0x4f7d28),
                        hex(0x7a9640),
                    ];
                    let t = tips[rng.below(tips.len())] * v;
                    (hex(0x24361a) * 0.8, t)
                }
            }
            Palette::Frozen => {
                let t = hex(0xd8dcd8) * rng.range(0.75, 1.0);
                (hex(0x8a8466) * 0.8, t)
            }
            Palette::Dune => {
                let t = if rng.chance(0.35) {
                    hex(0xcdb98a) * v
                } else {
                    hex(0x8fa087) * v
                };
                (hex(0x5d6b4f), t)
            }
            Palette::Reed => {
                let t = if rng.chance(0.2) {
                    hex(0xa89466) * v
                } else {
                    hex(0x5f7a35) * v
                };
                (hex(0x2c3d1c), t)
            }
        }
    }
}

struct ClumpStyle {
    blades: (usize, usize),
    radius: f32,
    height: (f32, f32),
    /// Root lean (radians from vertical), and how much more the tips bend.
    lean: (f32, f32),
    /// Lean every blade this way (trampled), else outward from the clump.
    toward: Option<Vec3>,
    width: (f32, f32),
    palette: Palette,
}

/// A tuft of grass blades rooted round `at`.
fn clump(m: &mut Mesh, rng: &mut Rand, at: Vec3, st: ClumpStyle) {
    let n = st.blades.0 + rng.below(st.blades.1 - st.blades.0 + 1);
    for _ in 0..n {
        let a = rng.range(0.0, TAU);
        let k = rng.f().sqrt();
        let r = st.radius * k;
        let off = Vec3::new(a.cos() * r, 0.0, a.sin() * r);
        let out_dir = Vec3::new(a.cos(), 0.0, a.sin());
        let toward = match st.toward {
            Some(t) => (t + out_dir * 0.35).normalize_or(t),
            None => {
                (out_dir + Vec3::new(rng.normal(0.5), 0.0, rng.normal(0.5))).normalize_or(out_dir)
            }
        };
        let (root, mut tip) = st.palette.blade(rng);
        // Some tips dried yellow.
        if rng.chance(0.25) {
            tip = tip.lerp(hex(0xb5a86a), rng.range(0.2, 0.6));
        }
        // Taller in the middle of the tuft, shorter and more bent at its rim.
        let len = rng.range(st.height.0, st.height.1) * (1.0 - 0.35 * k * rng.f());
        let rel = (len / st.height.1.max(1e-3)).min(1.0);
        let flop = if rng.chance(0.18) {
            rng.range(0.4, 0.9)
        } else {
            0.0
        };
        blade(
            m,
            at + off,
            toward,
            len,
            rng.range(st.width.0, st.width.1) * (0.65 + 0.6 * rel),
            rng.range(0.0, st.lean.0) + k * 0.25,
            rng.range(st.lean.0 * 0.5, st.lean.1) + flop,
            rng.range(-1.0, 1.0),
            root,
            tip,
            0.28,
        );
    }
}

/// A small wild flower: 0 daisy, 1 buttercup, 2 clover.
fn flower(m: &mut Mesh, rng: &mut Rand, at: Vec3, kind: usize) {
    let h = match kind {
        0 => rng.range(0.06, 0.14),
        1 => rng.range(0.18, 0.34),
        _ => rng.range(0.04, 0.09),
    };
    let lean = Vec3::new(rng.normal(0.12), 1.0, rng.normal(0.12)).normalize();
    let toward = Vec3::new(lean.x, 0.0, lean.z).normalize_or(Vec3::X);
    let stem = hex(0x4c7a2c);
    blade(
        m,
        at,
        toward,
        h,
        0.0028,
        lean.y.acos(),
        0.15,
        0.0,
        stem * 0.6,
        stem,
        0.5,
    );
    let top = at + lean * h * 0.985;
    let face = Vec3::new(rng.normal(0.3), 1.0, rng.normal(0.3)).normalize();
    let rot = Quat::from_rotation_arc(Vec3::Y, face) * Quat::from_rotation_y(rng.range(0.0, TAU));
    match kind {
        0 | 1 => {
            let (petals, len, wid, col, cup) = if kind == 0 {
                (13, 0.011, 0.0035, Vec3::new(0.92, 0.92, 0.9), 0.15)
            } else {
                (5, 0.008, 0.0065, Vec3::new(0.95, 0.72, 0.05), 0.8)
            };
            for k in 0..petals {
                let a = k as f32 / petals as f32 * TAU;
                let dir = rot * Vec3::new(a.cos(), cup * 0.4, a.sin()).normalize();
                let side = rot * Vec3::new(-a.sin(), 0.0, a.cos());
                let up = dir.cross(side).normalize_or(Vec3::Y);
                let up = if up.y < 0.0 { -up } else { up };
                let p0 = top + dir * 0.002;
                let p1 = top + dir * len;
                let quad = [
                    p0 - side * wid * 0.3,
                    p0 + side * wid * 0.3,
                    p1 + side * wid * 0.5,
                    p1 - side * wid * 0.5,
                ];
                two_sided_quad(m, quad, up, col.extend(1.0));
            }
            let disc = if kind == 0 {
                Vec3::new(0.9, 0.65, 0.05)
            } else {
                Vec3::new(0.7, 0.6, 0.1)
            };
            ball(m, top, rot, Vec3::new(0.007, 0.004, 0.007), disc);
        }
        _ => {
            let col = if rng.chance(0.6) {
                Vec3::new(0.85, 0.82, 0.8)
            } else {
                Vec3::new(0.75, 0.45, 0.55)
            };
            ball(m, top, rot, Vec3::new(0.016, 0.014, 0.016), col * 0.9);
        }
    }
}

fn two_sided_quad(m: &mut Mesh, q: [Vec3; 4], up: Vec3, c: Vec4) {
    for n in [up, -up] {
        let base = m.vertices.len() as u32;
        for p in q {
            m.vertices
                .push(vert(p, (n + Vec3::Y * 0.3).normalize(), Vec2::ZERO, c));
        }
        // Wind each face toward its own normal.
        let g = (q[1] - q[0]).cross(q[2] - q[0]);
        if g.dot(n) < 0.0 {
            m.indices
                .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        } else {
            m.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
}

// ---------------------------------------------------------------------- small props

/// A stone of about `size` meters resting at `at`, merged into the batch.
fn stone(b: &mut Batch, rng: &mut Rand, s: f64, at: Vec3, size: f32, color: Vec3, bury: f32) {
    let variant = rng.below(KIT);
    let detail = usize::from(size > 0.06);
    let kit = rock_kit(bury);
    let base = &kit[detail * KIT + variant];
    let scale = Vec3::new(
        size * rng.range(0.8, 1.3),
        size * rng.range(0.45, 0.8),
        size * rng.range(0.8, 1.3),
    );
    let rot = Quat::from_rotation_y(rng.range(0.0, TAU))
        * Quat::from_rotation_x(rng.normal(0.12))
        * Quat::from_rotation_z(rng.normal(0.12));
    let tint = color * rng.range(0.8, 1.2);
    append(b.at(s, Mat::Stone), base, rot, scale, at, tint.extend(1.0));
}

const KIT: usize = 12;

/// Unit rocks shared by every stone: `KIT` low-detail then `KIT` higher-detail shapes, for
/// a shallow (< 0.42) or a deep bury.
pub(super) fn rock_kit(bury: f32) -> &'static [Mesh] {
    static SHALLOW: std::sync::OnceLock<Vec<Mesh>> = std::sync::OnceLock::new();
    static DEEP: std::sync::OnceLock<Vec<Mesh>> = std::sync::OnceLock::new();
    let (cell, bury) = if bury < 0.42 {
        (&SHALLOW, 0.35)
    } else {
        (&DEEP, 0.5)
    };
    cell.get_or_init(|| {
        (0..2 * KIT)
            .map(|i| {
                let detail = 1 + (i / KIT) as u32;
                rock(i as u32 * 31 + 7, detail, Vec3::ONE, 0.32, bury, Vec3::ONE)
            })
            .collect()
    })
}

/// A small ball (flower centres, clover heads) of `size` meters.
fn ball(m: &mut Mesh, at: Vec3, rot: Quat, size: Vec3, color: Vec3) {
    let base = &rock_kit(0.0)[3];
    let start = m.vertices.len();
    append(m, base, rot, size, at, Vec4::ONE);
    for v in &mut m.vertices[start..] {
        v.color = color.extend(1.0).to_array();
    }
}

/// A small scallop shell.
fn shell(m: &mut Mesh, rng: &mut Rand, at: Vec3) {
    let start = m.indices.len();
    let base = m.vertices.len() as u32;
    let r = rng.range(0.012, 0.028);
    let col = [
        Vec3::new(0.85, 0.78, 0.7),
        Vec3::new(0.8, 0.6, 0.55),
        Vec3::new(0.6, 0.6, 0.62),
    ][rng.below(3)];
    let rot = Quat::from_rotation_y(rng.range(0.0, TAU)) * Quat::from_rotation_x(rng.normal(0.15));
    let (nr, na) = (5, 14);
    for i in 0..=nr {
        let t = i as f32 / nr as f32;
        for j in 0..=na {
            let a = (j as f32 / na as f32 - 0.5) * 2.3;
            let rib = 0.06 * (a * 9.0).cos().abs();
            let rr = r * t;
            let y = r * 0.35 * (1.0 - t * t) * (1.0 - (a / 1.2).powi(2)).max(0.0) + rr * rib * 0.3;
            let p = Vec3::new(a.sin() * rr, y + 0.001, a.cos() * rr - r * 0.4);
            let c = col * (0.8 + 0.25 * t) * (1.0 - rib * 1.5);
            m.vertices
                .push(vert(rot * p + at, Vec3::Y, Vec2::ZERO, c.extend(1.0)));
        }
    }
    let row = (na + 1) as u32;
    for i in 0..nr as u32 {
        for j in 0..na as u32 {
            let k = base + i * row + j;
            m.indices
                .extend_from_slice(&[k, k + 1, k + row, k + 1, k + row + 1, k + row]);
        }
    }
    fix_up(m, start);
    smooth_normals_from(m, start);
}

/// Wind the triangles from `start` so they face up.
fn fix_up(m: &mut Mesh, start: usize) {
    let (vs, is) = (&m.vertices, &mut m.indices);
    for t in is[start..].as_chunks_mut::<3>().0 {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(vs[i as usize].position));
        if (b - a).cross(c - a).y < 0.0 {
            t.swap(1, 2);
        }
    }
}

/// A fallen twig with a side shoot.
fn twig(m: &mut Mesh, rng: &mut Rand, at: Vec3) {
    let dir = Vec3::new(rng.normal(1.0), 0.0, rng.normal(1.0)).normalize_or(Vec3::X);
    let len = rng.range(0.12, 0.4);
    let r = rng.range(0.003, 0.007);
    let mut pts = Vec::new();
    let side = dir.cross(Vec3::Y);
    let bend = rng.normal(0.04);
    for k in 0..6 {
        let t = k as f32 / 5.0;
        pts.push(at + dir * len * (t - 0.5) + side * bend * (t * PI).sin() + Vec3::Y * r * 0.6);
    }
    let col = hex(0x9a8a7a);
    append(
        m,
        &tube(
            &pts,
            5,
            true,
            |t, _| r * (1.0 - 0.4 * t),
            |_| col.extend(1.0),
        ),
        Quat::IDENTITY,
        Vec3::ONE,
        Vec3::ZERO,
        Vec4::ONE,
    );
    if rng.chance(0.6) {
        let from = pts[2];
        let sd = (dir + side * rng.range(-1.2, 1.2)).normalize();
        let l2 = len * rng.range(0.25, 0.5);
        let p2: Vec<Vec3> = (0..4).map(|k| from + sd * l2 * k as f32 / 3.0).collect();
        append(
            m,
            &tube(
                &p2,
                4,
                true,
                |t, _| r * 0.6 * (1.0 - 0.5 * t),
                |_| col.extend(1.0),
            ),
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::ZERO,
            Vec4::ONE,
        );
    }
}

/// A mushroom: a pale stem and a brown cap (or a red one with white flecks).
fn mushroom(m: &mut Mesh, rng: &mut Rand, at: Vec3, red: bool) {
    let h = rng.range(0.035, 0.09);
    let r = h * rng.range(0.45, 0.7);
    let lean = Vec3::new(rng.normal(0.12), 1.0, rng.normal(0.12)).normalize();
    let stem_r = r * 0.28;
    let pts: Vec<Vec3> = (0..5).map(|k| at + lean * h * k as f32 / 4.0).collect();
    let stem_col = Vec3::new(0.82, 0.78, 0.68);
    append(
        m,
        &tube(
            &pts,
            10,
            false,
            |t, _| stem_r * (1.15 - 0.3 * t),
            |t| (stem_col * (0.75 + 0.25 * t)).extend(1.0),
        ),
        Quat::IDENTITY,
        Vec3::ONE,
        Vec3::ZERO,
        Vec4::ONE,
    );
    let top = at + lean * h;
    let cap_col = if red {
        Vec3::new(0.6, 0.06, 0.02)
    } else {
        Vec3::new(0.42, 0.25, 0.12) * rng.range(0.8, 1.2)
    };
    let (rings, sides) = (6, 18);
    let start = m.indices.len();
    let base = m.vertices.len() as u32;
    let rot = Quat::from_rotation_arc(Vec3::Y, lean);
    let seed = rng.seed();
    for i in 0..=rings {
        let t = i as f32 / rings as f32;
        let phi = t * PI * 0.55;
        for j in 0..=sides {
            let a = j as f32 / sides as f32 * TAU;
            let rr = r * phi.sin() * if t > 0.9 { 0.97 } else { 1.0 };
            let y = r * 0.65 * phi.cos() - r * 0.15;
            let p = Vec3::new(a.cos() * rr, y, a.sin() * rr);
            let fleck =
                red && super::procgen::unit(super::procgen::hash(i, j, seed)) > 0.82 && t < 0.85;
            let c = if fleck {
                Vec3::new(0.9, 0.88, 0.8)
            } else {
                cap_col * (0.7 + 0.4 * (1.0 - t))
            };
            m.vertices
                .push(vert(top + rot * p, Vec3::Y, Vec2::ZERO, c.extend(1.0)));
        }
    }
    let row = (sides + 1) as u32;
    for i in 0..rings as u32 {
        for j in 0..sides as u32 {
            let k = base + i * row + j;
            m.indices
                .extend_from_slice(&[k, k + 1, k + row, k + 1, k + row + 1, k + row]);
        }
    }
    // Underside (gills): a flat disc facing down.
    let gill = m.vertices.len() as u32;
    let under = top + rot * Vec3::new(0.0, -r * 0.15 + r * 0.65 * (PI * 0.55).cos() - 0.002, 0.0);
    m.vertices.push(vert(
        under,
        -lean,
        Vec2::ZERO,
        Vec4::new(0.75, 0.68, 0.55, 1.0),
    ));
    for j in 0..=sides {
        let a = j as f32 / sides as f32 * TAU;
        let rr = r * (PI * 0.55).sin() * 0.97;
        let p = under + rot * Vec3::new(a.cos() * rr, 0.0, a.sin() * rr);
        m.vertices
            .push(vert(p, -lean, Vec2::ZERO, Vec4::new(0.62, 0.55, 0.42, 1.0)));
    }
    for j in 0..sides as u32 {
        m.indices
            .extend_from_slice(&[gill, gill + 2 + j, gill + 1 + j]);
    }
    // Normals for the cap from its shape; outward winding.
    let cap_end = start + (rings * sides * 6) as usize;
    for t in m.indices[start..cap_end].as_chunks_mut::<3>().0 {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].position));
        let out = (a + b + c) / 3.0 - (top - lean * r);
        if (b - a).cross(c - a).dot(out) < 0.0 {
            t.swap(1, 2);
        }
    }
    smooth_normals_from(m, start);
    // The gill disc keeps its downward normal.
    for v in &mut m.vertices[gill as usize..] {
        v.normal = (-lean).to_array();
    }
}

/// A bleached, gnarled log lying at `at` along `dir`, half sunk in the sand.
fn driftwood(at: Vec3, dir: Vec3, seed: u64) -> Part {
    let mut rng = Rand::new(seed);
    let side = dir.cross(Vec3::Y).normalize();
    let len = 1.5;
    let along = (dir * 0.85 + side * 0.5).normalize();
    let mut pts = Vec::new();
    for k in 0..=28 {
        let t = k as f32 / 28.0;
        let wob = fbm_world(t * 3.0, 0.5, 1.0, 2, seed as u32) * 0.12;
        pts.push(
            along * len * (t - 0.5)
                + along.cross(Vec3::Y) * wob
                + Vec3::Y * (0.02 * (t * PI).sin()),
        );
    }
    let col = Vec3::new(0.82, 0.8, 0.76);
    let s2 = seed as u32;
    let mut m = tube(
        &pts,
        14,
        true,
        move |t, a| {
            let taper = 0.075 * (1.0 - 0.45 * t) + 0.02 * (1.0 - smooth(0.0, 0.08, t));
            let gnarl = 1.0
                + 0.18 * fbm_world(t * 9.0, a * 1.3, 1.0, 3, s2)
                + 0.06 * (a * 5.0 + t * 30.0).sin();
            taper * gnarl
        },
        |t| (col * (0.85 + 0.15 * t)).extend(1.0),
    );
    // Two broken branch stubs.
    for (t, ang) in [(0.3f32, 0.9f32), (0.62, -1.1)] {
        let i = (t * 28.0) as usize;
        let from = pts[i];
        let sd = (Quat::from_rotation_y(ang) * along + Vec3::Y * rng.range(0.1, 0.5)).normalize();
        let p: Vec<Vec3> = (0..6).map(|k| from + sd * 0.07 * k as f32).collect();
        append(
            &mut m,
            &tube(
                &p,
                8,
                true,
                |t, _| 0.028 * (1.0 - 0.5 * t),
                |_| col.extend(1.0),
            ),
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::ZERO,
            Vec4::ONE,
        );
    }
    Part::own(m, Mat::Driftwood, at)
}
