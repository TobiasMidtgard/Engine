//! Snow on the trail: a blanket a few centimeters deep over the snow stretch, carried on
//! along the edges of the ice after it and dying out toward the gravel.
//!
//! The blanket is a depth over the ground, not a sheet: 6–12 cm of untouched snow (deeper
//! where it drifted against the verges), trodden down to about 2 cm in a meandering trench
//! where people walk (and a shallower second track where the faster walkers pass), full of
//! old, softened prints, fresh crisp ones and the odd deep posthole where someone stepped
//! aside. Every edge is a rounded lip that curls down into the ground beside it (the verge,
//! the mud it starts on, the ice it ends on), so the outline comes from where the two
//! surfaces meet rather than from a cut-out. Past the edges lie loose patches of the same
//! snow, thin snow frays into holes, and mud carried in on boots browns the first meters of
//! the trench.

use std::f32::consts::PI;

use engine_core::math::{Vec2, Vec3};

use super::dress::{Print, PrintKind, Prints};
use super::procgen::{Rand, fbm_world, smooth};
use super::ribbon::{self, Ribbon, Surf, base, columns};
use super::{Mat, Out};
use crate::demo::{STRETCHES, stretch_start};

/// Width and height of the small rounded lip the snow ends in, meters.
const LIP: f32 = 0.05;
const LIP_H: f32 = 0.02;
/// How fast the snow thickens inward from its edge (meters of depth per meter).
const TAPER: f32 = 0.14;
/// Width and height of the lip round a loose patch, meters.
const PATCH_LIP: f32 = 0.035;
const PATCH_LIP_H: f32 = 0.009;
/// Middle of the passing track (where the faster walkers overtake, toward the loop's inside).
const PASS_X: f32 = 0.8;

/// The snow at one point.
#[derive(Clone, Copy, Debug)]
pub(super) struct SnowAt {
    /// Height of the snow's surface above the origin, meters (under the ground where there
    /// is none).
    pub y: f32,
    /// Depth of the snow before footprints, meters (0 where there is none).
    pub depth: f32,
    /// How far inside the snow's edge, meters (negative outside).
    pub inside: f32,
    /// Footprints and holes: negative in them, a little positive on their rims, meters.
    pub relief: f32,
    /// How trodden: 0 untouched .. 1 the middle of the trench.
    pub trodden: f32,
}

/// The trail's snow (built once, shared by every job that needs it).
pub(super) fn snow() -> &'static Snow {
    static SNOW: std::sync::OnceLock<Snow> = std::sync::OnceLock::new();
    SNOW.get_or_init(Snow::new)
}

/// The snow stretch's blanket: one ribbon from a little before the stretch (it starts in
/// patches on the mud) to its end, fine enough for footprints across the trodden middle and
/// for the rounded lip at the edges.
pub(super) fn ground() -> Out {
    let snow = snow();
    let surface = |s: f64, x: f32, w: Vec2| snow.surface(s, x, w);
    Out {
        parts: ribbon::ribbon(&Ribbon {
            mat: Mat::Snow,
            s0: snow.s0 - 0.95,
            s1: snow.s1 + 0.04,
            fade_in: 0.0,
            fade_out: 0.0,
            ds: 0.02,
            columns: columns(3.4, 0.02, 1.4, 0.03, 3.4, 0.03),
            surface: &surface,
            shadows: true,
        }),
        ..Default::default()
    }
}

/// The snow over the snow stretch and along the ice.
pub(super) struct Snow {
    /// The snow stretch, meters along the trail.
    pub s0: f64,
    pub s1: f64,
    /// The ice stretch after it.
    pub i0: f64,
    pub i1: f64,
    prints: Prints,
}

fn range(name: &str) -> (f64, f64) {
    let s0 = stretch_start(name).unwrap_or(0.0);
    let len = STRETCHES.iter().find(|s| s.0 == name).map_or(0.0, |s| s.1);
    (s0, s0 + len)
}

/// Quarter-round profile of a lip `w` wide: 0 at the edge, 1 a lip's width in.
fn lip(inside: f32, w: f32) -> f32 {
    if inside >= w {
        1.0
    } else if inside > 0.0 {
        let t = 1.0 - inside / w;
        (1.0 - t * t).sqrt()
    } else {
        0.0
    }
}

/// Depth of snow `full` deep, `inside` meters in from its edge: a thin sheet thickening
/// inward that ends in a small rounded lip.
fn edge(full: f32, inside: f32) -> f32 {
    full.min(LIP_H * lip(inside, LIP) + TAPER * inside.max(0.0))
}

/// Middle of the trodden trench at `s` (it meanders a little).
fn trench_middle(s: f64) -> f32 {
    0.06 * fbm_world(s as f32 * 0.4, 1.7, 1.0, 2, 0x5b1)
}

impl Snow {
    pub fn new() -> Self {
        let (s0, s1) = range("snow");
        let (i0, i1) = range("ice");
        let mut p = Prints::walkers(s0 + 0.5, s1 - 0.6, 0x9002, 0.017).0;
        let mut rng = Rand::new(0x5e0);
        // Old prints trodden over and over, softened by melt and fresh falls: most in the
        // trench, some in the passing track, both ways along the trail.
        let n = ((s1 - s0) * 30.0) as usize;
        for _ in 0..n {
            let s = rng.range(s0 as f32 + 0.2, s1 as f32 - 0.4) as f64;
            let pass = rng.chance(0.14);
            let x = if pass {
                PASS_X + rng.normal(0.05)
            } else {
                trench_middle(s) + rng.normal(0.15)
            };
            let back = if rng.chance(0.45) { PI } else { 0.0 };
            let r = rng.f();
            let kind = if r < 0.84 {
                PrintKind::Boot
            } else if r < 0.95 {
                PrintKind::Paw
            } else {
                PrintKind::Hoof
            };
            p.push(Print {
                s,
                x,
                yaw: rng.normal(0.18) + back,
                kind,
                depth: rng.range(0.005, 0.013),
                soft: rng.range(0.7, 1.0),
            });
        }
        // Postholes: a few steps beside the trench where someone stepped aside, sinking
        // most of the way through the untouched snow.
        let mut s = s0 + 1.2;
        while s < s1 - 1.8 {
            if rng.chance(0.55) {
                let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
                let x = side * rng.range(0.62, 1.3);
                let steps = 1 + rng.below(3);
                let mut left = rng.chance(0.5);
                for k in 0..steps {
                    let foot = if left { -0.11 } else { 0.11 };
                    p.push(Print {
                        s: s + k as f64 * rng.range(0.55, 0.7) as f64,
                        x: x + foot + rng.normal(0.03),
                        yaw: side * rng.range(0.1, 0.45) + rng.normal(0.08),
                        kind: PrintKind::Boot,
                        depth: rng.range(0.03, 0.05),
                        // Soft walls: deep snow caves in round a posthole.
                        soft: rng.range(1.3, 1.7),
                    });
                    left = !left;
                }
            }
            s += rng.range(0.8, 1.9) as f64;
        }
        p.sort_by(|a, b| a.s.total_cmp(&b.s));
        Self {
            s0,
            s1,
            i0,
            i1,
            prints: Prints(p),
        }
    }

    /// How trodden the snow is at (s, x): 1 in the middle of the trench, a little in the
    /// passing track.
    fn trench(&self, s: f64, x: f32) -> f32 {
        let c = trench_middle(s);
        let hw = 0.36 + 0.06 * fbm_world(s as f32, 3.3, 1.5, 2, 0x5b2);
        let main = 1.0 - smooth(hw - 0.06, hw + 0.16, (x - c).abs());
        let pass = 0.5 * (1.0 - smooth(0.08, 0.24, (x - PASS_X).abs()));
        main.max(pass)
    }

    /// Fraction of the way along the ice stretch (0 before it).
    fn along_ice(&self, s: f64) -> f32 {
        ((s - self.i0) / (self.i1 - self.i0)).clamp(0.0, 1.0) as f32
    }

    /// Depth of untouched snow, meters: drifted deeper toward the verges, thinning along the
    /// ice.
    fn deep(&self, s: f64, x: f32, w: Vec2) -> f32 {
        let lumps = fbm_world(w.x, w.y, 1.5, 3, 0x5a1);
        let fine = fbm_world(w.x, w.y, 0.35, 2, 0x5a3);
        let drift = smooth(0.7, 1.8, x.abs()) * (0.55 + 0.45 * fbm_world(w.x, w.y, 2.4, 2, 0x5a2));
        let d = 0.066 + 0.024 * lumps + 0.007 * fine + 0.042 * drift;
        d * (1.0 - 0.5 * self.along_ice(s))
    }

    /// How far out the blanket reaches on one side, meters from the centre line.
    fn outer(&self, s: f64, side: f32) -> f32 {
        let k = side * 9.7;
        let big = fbm_world(s as f32, k, 2.2, 3, 0x5c1);
        let small = fbm_world(s as f32, k, 0.35, 2, 0x5c2);
        2.3 + 0.3 * big + 0.07 * small - 0.4 * self.along_ice(s)
    }

    /// Where the blanket starts on one side (the ice lies inside it), meters from the centre
    /// line; negative where it covers the whole path. The trench opens onto the ice over the
    /// last meters of the snow stretch.
    fn inner(&self, s: f64, side: f32) -> f32 {
        let k = side * 5.3;
        let open_at = self.s1 as f32 - 1.75 + 0.45 * fbm_world(side * 3.1, 0.7, 1.0, 1, 0x5d3);
        let open = smooth(open_at, self.s1 as f32 + 0.5, s as f32);
        let edge = 1.02
            + 0.12 * fbm_world(s as f32, k, 1.4, 3, 0x5d1)
            + 0.04 * fbm_world(s as f32, k, 0.3, 2, 0x5d2);
        -0.7 + (edge + 0.7) * open
    }

    /// How far inside the start (off the mud) and the end (into the gravel), meters. Mud
    /// carried in on boots keeps the trodden middle bare for a meter or so more.
    fn along_inside(&self, s: f64, x: f32) -> f32 {
        let mid = trench_middle(self.s0);
        let notch = 1.0 - smooth(0.12, 0.62, (x - mid).abs());
        // (Small wiggles stay small: the snow thickens inward from this line, so any
        // ripple in it shows as stripes along the trail.)
        let start = self.s0 as f32 - 0.45
            + 0.35 * fbm_world(x, 1.3, 0.7, 3, 0x5e1)
            + 0.025 * fbm_world(x, 2.9, 0.2, 2, 0x5e3)
            + notch * (1.05 + 0.12 * fbm_world(x, 0.4, 0.4, 2, 0x5e4));
        let end = self.i1 as f32 - 0.7 + 0.6 * fbm_world(x, 4.1, 0.5, 3, 0x5e2);
        (s as f32 - start).min(end - s as f32)
    }

    /// The snow at (s, x), `w` the world position (meters from the origin).
    pub fn at(&self, s: f64, x: f32, w: Vec2) -> SnowAt {
        let ax = x.abs();
        let side = if x < 0.0 { -1.0 } else { 1.0 };
        let trodden = self.trench(s, x) * (1.0 - self.along_ice(s));
        let main = (self.outer(s, side) - ax)
            .min(ax - self.inner(s, side))
            .min(self.along_inside(s, x));
        let deep = self.deep(s, x, w);
        let packed = 0.021 + 0.004 * fbm_world(w.x, w.y, 0.3, 2, 0x5b3);
        let blanket = deep + (packed - deep) * trodden;
        // Loose patches, common near the blanket's edge and rarer away from it (and only
        // where the snow's ribbons reach).
        let reach = smooth(self.s0 as f32 - 0.85, self.s0 as f32 - 0.6, s as f32)
            * (1.0 - smooth(self.i1 as f32 + 0.1, self.i1 as f32 + 0.35, s as f32));
        let near = (1.0 - smooth(0.0, 0.9, -main)) * reach;
        let patch = (fbm_world(w.x, w.y, 0.45, 3, 0x5f1) - 0.18 - 0.7 * (1.0 - near)) * 0.3;
        let patch_depth = 0.012 + 0.012 * fbm_world(w.x, w.y, 0.2, 2, 0x5f2).max(0.0);
        let main_depth = edge(blanket, main);
        let patch_d = patch_depth.min(PATCH_LIP_H * lip(patch, PATCH_LIP) + 0.2 * patch.max(0.0));
        let (depth, inside) = if main_depth >= patch_d {
            (main_depth, main.max(patch))
        } else {
            (patch_d, patch.max(main))
        };
        let ground = base(x, w);
        if inside <= 0.0 {
            // Off the snow: the surface drops straight under the ground beside it (and
            // under the lawn's frayed edge too).
            let dive = -0.035 * smooth(0.0, 0.006, -inside) + 2.0 * inside;
            return SnowAt {
                y: ground + dive,
                depth: 0.0,
                inside,
                relief: 0.0,
                trodden,
            };
        }
        // Footprints only in the blanket, never through to the ground.
        let relief = self.prints.relief(s, x) * smooth(0.0, 0.3, main);
        let d = (depth + relief).max(depth.min(0.004));
        SnowAt {
            y: ground + d,
            depth,
            inside,
            relief,
            trodden,
        }
    }

    /// Whether thick snow hides the ground at (s, x) (ground under it can be left out).
    pub fn covers(&self, s: f64, x: f32, w: Vec2) -> bool {
        let a = self.at(s, x, w);
        a.inside > 0.04 && a.depth + a.relief.min(0.0) > 0.015
    }

    /// The ribbon surface at (s, x): height, coverage (the thin snow near an edge frays into
    /// holes; negative where it lies under the ground) and tint.
    pub fn surface(&self, s: f64, x: f32, w: Vec2) -> Surf {
        let a = self.at(s, x, w);
        let cover = if a.inside < -0.03 {
            -1.0
        } else {
            1.0 - 0.42 * (1.0 - smooth(0.0, 0.1, a.inside))
        };
        Surf::new(a.y, cover, self.tint(s, &a, w))
    }

    /// Vertex tint: packed and greyer where trodden, browned by carried mud at the start,
    /// blue-grey in the dents, grey where thin.
    fn tint(&self, s: f64, a: &SnowAt, w: Vec2) -> Vec3 {
        let n = fbm_world(w.x, w.y, 1.8, 2, 0x4a);
        let dirt = smooth(-0.3, 0.7, fbm_world(w.x, w.y, 0.6, 3, 0x4b));
        let mut c = Vec3::new(0.99, 1.0, 1.03) * (0.95 + 0.06 * n);
        // Packed snow: greyer, with grit trodden in.
        let packed = Vec3::new(0.86, 0.85, 0.84) * (1.0 - 0.16 * dirt);
        c = c.lerp(c * packed, a.trodden);
        // Mud carried in from the stretch before in clots and smears, fading over the first
        // meters.
        let dent = smooth(0.002, 0.025, -a.relief);
        let clots = smooth(-0.15, 0.35, fbm_world(w.x, w.y, 0.18, 3, 0x4c));
        let carried = (1.0 - smooth(self.s0 as f32 - 0.3, self.s0 as f32 + 4.0, s as f32))
            * (0.8 * a.trodden + 0.5 * dent).min(1.0);
        c = c.lerp(
            Vec3::new(0.4, 0.31, 0.23),
            (carried * (0.25 + 0.75 * clots)).min(0.9),
        );
        // Dents in the shade of their walls, lit by the blue sky.
        c *= Vec3::ONE.lerp(Vec3::new(0.87, 0.91, 0.99), dent);
        // Thin, wet snow at the edges: the ground shows through.
        let thin = 1.0 - smooth(0.004, 0.035, a.depth);
        c * Vec3::ONE.lerp(Vec3::new(0.78, 0.8, 0.78), thin * 0.6)
    }
}
