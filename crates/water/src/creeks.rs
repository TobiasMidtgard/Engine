//! Creeks everywhere, drawn without simulation: a ribbon over each creek the terrain reports,
//! carrying the current and depth the stream shader needs (`shade_stream`, the look of the
//! simulated mountain streams).
//!
//! The terrain says where its creeks run ([`Creeks`], `engine_world::terrain`), and a demo can
//! add its own ([`LocalCreeks`]). Around the camera, [`creek_ribbons_system`] lays a strip of
//! water along each reach and works out from the reach's slope and discharge how the water
//! runs (creeks-research.md, creeks-plan.md):
//!
//! - **Steps and pools.** Steep creeks drop in steps with a pool behind each, spaced by their
//!   slope (Montgomery and Buffington 1997): under a width apart in cascades (slope over
//!   0.065), one to four widths in step-pool reaches (0.03 to 0.065), and none on gentler
//!   plane and pool-riffle beds. Each step drops about 1.5 times the slope over its length
//!   (Abrahams et al. 1995), mostly in a short chute at its end.
//! - **The current** is the discharge over the cross-section, q = Q / w over the depth: slow in
//!   the deep pools, fast and shallow over each step's lip, so the Froude number climbs there
//!   and the shader's white-water rules turn the chutes white. Across, it falls off toward the
//!   banks (1 − s⁴), whose shear draws the eddy-line flecks along them.
//! - **Range.** A creek is drawn while it is about 4 pixels wide or more: out to 310 times its
//!   width (a 1 m brook to 310 m, a 10 m stream to 3 km). Its rings thin with distance.
//!
//! The water's edge runs 15% wider than the creek's width, with its depth falling to nothing,
//! so the carved bank hides where the ribbon ends.

use std::sync::Arc;

use bevy_tasks::Task;
use bevy_tasks::futures::check_ready;
use bytemuck::{Pod, Zeroable};
use engine_core::math::{DVec2, DVec3, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_world::WorldShift;
use engine_world::terrain::{CreekPoint, CreekQuery, CreekReach, Creeks};

/// Creeks a demo or a level adds beside the terrain's own: a creek provider in its entity's
/// space (the entity's translation is added to everything it reports).
#[derive(Component, Clone)]
pub struct LocalCreeks(pub Arc<dyn CreekQuery>);

/// One vertex of a creek's ribbon, as `vs_creek` reads it.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct CreekVertex {
    /// Position relative to [`CreekRibbons::anchor`], and the water's depth (m).
    pub pos_depth: [f32; 4],
    /// Current x, z (m/s), its shear across (1/s), meters from the reach's source.
    pub vel_shear: [f32; 4],
    /// Surface normal.
    pub normal: [f32; 4],
}

/// The ribbons around the camera, for the renderer.
#[derive(Resource, Default)]
pub struct CreekRibbons {
    /// World position the vertices are relative to.
    pub anchor: DVec3,
    pub vertices: Vec<CreekVertex>,
    pub indices: Vec<u32>,
    /// Changes whenever the ribbons are rebuilt.
    pub version: u64,
    /// Reaches drawn.
    pub reaches: usize,
    /// Where the camera was at the last build, and the providers then.
    built_at: Option<DVec3>,
    providers: Vec<(usize, DVec3)>,
    since: f32,
    /// Ribbons being laid on a worker.
    job: Option<Task<Laid>>,
    /// Stretches simulated near the camera, left out of the ribbons
    /// ([`crate::creek_sim`] keeps them), and those the ribbons were last laid without.
    pub cuts: Vec<Cut>,
    cuts_laid: Vec<Cut>,
}

/// Reaches are asked for within this distance of the camera (X and Z), meters.
const QUERY_M: f64 = 3000.0;
/// The ribbons are rebuilt when the camera has moved this far, meters.
const REBUILD_M: f64 = 48.0;
/// How often the camera's move is checked, seconds.
const PERIOD_S: f32 = 0.2;
/// A creek is drawn out to this many times its width (about 4 pixels at 1440p with a 60°
/// field of view; nearer to the camera than this it is wider).
const PIXELS_RANGE: f32 = 310.0;
/// ... but never less than this, nor more than [`QUERY_M`], meters.
const LEAST_RANGE_M: f32 = 150.0;
/// The water's edge, as a share of the creek's half-width: past the bank's foot, so the bank
/// hides it.
const EDGE: f32 = 1.15;
/// Fastest current drawn, m/s.
const MOST_SPEED: f32 = 6.0;
/// A ribbon reaches this far into a stretch a simulation draws, meters.
pub const SKIP_OVERLAP_M: f32 = 2.0;
/// Within this distance the rings follow each step's chute and pool, meters.
const STEP_DETAIL_M: f32 = 120.0;
/// Phase of the step layout is integrated from the source in steps this long, meters, so it
/// stays put however the rings are spaced.
const PHASE_STEP_M: f32 = 0.5;

/// How far a creek `width_m` wide is drawn, meters.
pub fn creek_range(width_m: f32) -> f32 {
    (width_m * PIXELS_RANGE).clamp(LEAST_RANGE_M, QUERY_M as f32)
}

/// The step-pool layout at a slope: (step length in widths, how much of the layout is there,
/// 0 on plane beds .. 1 from step-pool slopes on).
pub fn step_spacing(slope: f32) -> (f32, f32) {
    let s = slope.max(0.0);
    let widths = if s > 0.065 {
        0.8
    } else {
        // 4 widths at 0.03, 1.5 at 0.065.
        4.0 - 2.5 * ((s - 0.03) / 0.035).clamp(0.0, 1.0)
    };
    (widths, smoothstep(0.02, 0.035, s))
}

/// The surface of one step, `f` 0 at the foot of the step above (in its pool) .. 1 at this
/// step's foot: (drop below the reach's even line, as a share of the step's drop S·L; depth
/// factor). The pool runs gently over 80% of it, the chute takes the rest; the deviations
/// average to zero over the step. The pool is deepest just below the step above (its scour
/// hole), the chute shallow and fast.
pub fn step_profile(f: f32) -> (f32, f32) {
    const POOL: f32 = 0.8;
    // Drop below the start of the step: 20% of the step's drop over the pool, the rest in
    // the chute (eased in and out).
    let drop = if f < POOL {
        0.2 * f
    } else {
        let t = smoothstep(POOL, 1.0, f);
        0.16 + 0.84 * t
    };
    // The even line drops f; the step's surface stands above it by f − drop, which averages
    // 0.32 over the step.
    let above = f - drop - 0.32;
    let depth = if f < POOL {
        1.0 + 0.8 * (1.0 - f / POOL)
    } else {
        1.0 - 0.6 * smoothstep(POOL, 0.9, f)
    };
    (above, depth)
}

/// A reach prepared for laying a ribbon: its points (world), meters from the source at each,
/// and the step layout's phase at each.
struct Course {
    points: Vec<CreekPoint>,
    along: Vec<f32>,
    phase: Vec<f32>,
}

/// What the ribbon needs at one place along a reach.
#[derive(Debug, Clone, Copy)]
struct Station {
    at: DVec3,
    dir: Vec2,
    width: f32,
    depth: f32,
    flow: f32,
    slope: f32,
    phase: f32,
}

impl Course {
    fn new(points: Vec<CreekPoint>) -> Self {
        let mut along = Vec::with_capacity(points.len());
        let mut s = 0.0f32;
        for (k, p) in points.iter().enumerate() {
            if k > 0 {
                s += horizontal(points[k - 1].at, p.at);
            }
            along.push(s);
        }
        let mut course = Self {
            points,
            along,
            phase: Vec::new(),
        };
        // The step layout's phase, integrated from the source in short steps: the same
        // wherever the rings fall.
        let mut phase = Vec::with_capacity(course.points.len());
        let mut p = 0.0f32;
        let mut s = 0.0f32;
        for k in 0..course.points.len() {
            let to = course.along[k];
            while s < to {
                let ds = (to - s).min(PHASE_STEP_M);
                let st = course.raw(s + 0.5 * ds);
                let (widths, _) = step_spacing(st.slope);
                p += ds / (widths * st.width.max(0.3));
                s += ds;
            }
            phase.push(p);
        }
        course.phase = phase;
        course
    }

    fn length(&self) -> f32 {
        self.along.last().copied().unwrap_or(0.0)
    }

    /// Segment index and fraction at `s` meters from the source.
    fn locate(&self, s: f32) -> (usize, f32) {
        let n = self.points.len();
        let k = self.along.partition_point(|a| *a <= s).clamp(1, n - 1) - 1;
        let len = (self.along[k + 1] - self.along[k]).max(1e-6);
        (k, ((s - self.along[k]) / len).clamp(0.0, 1.0))
    }

    /// The reach at `s` without the step layout's phase.
    fn raw(&self, s: f32) -> Station {
        let (k, t) = self.locate(s);
        let (a, b) = (&self.points[k], &self.points[k + 1]);
        // Positions on a Catmull-Rom curve through the points (a polyline's corners would
        // kink the ribbon).
        let p = |i: isize| {
            let i = i.clamp(0, self.points.len() as isize - 1) as usize;
            self.points[i].at
        };
        let (p0, p1, p2, p3) = (
            p(k as isize - 1),
            p(k as isize),
            p(k as isize + 1),
            p(k as isize + 2),
        );
        let t64 = t as f64;
        let at = catmull_rom(p0, p1, p2, p3, t64);
        let tangent = catmull_rom_tangent(p0, p1, p2, p3, t64);
        let dir = Vec2::new(tangent.x as f32, tangent.z as f32).normalize_or(Vec2::X);
        // The surface's height runs straight between the points (a curve could overshoot and
        // climb), and the slope is taken over a couple of widths.
        let y = a.at.y + (b.at.y - a.at.y) * t64;
        let width = lerp(a.width_m, b.width_m, t);
        let reach = (2.0 * width).max(4.0);
        let lo = self.height(s - reach);
        let hi = self.height(s + reach);
        let span = (s + reach).min(self.length()) - (s - reach).max(0.0);
        let slope = if span > 0.1 {
            ((lo - hi) / span as f64) as f32
        } else {
            0.0
        };
        Station {
            at: DVec3::new(at.x, y, at.z),
            dir,
            width,
            depth: lerp(a.depth_m, b.depth_m, t),
            flow: lerp(a.flow_m3_s, b.flow_m3_s, t),
            slope,
            phase: 0.0,
        }
    }

    /// The surface's height on the even line at `s`.
    fn height(&self, s: f32) -> f64 {
        let (k, t) = self.locate(s.clamp(0.0, self.length()));
        let (a, b) = (&self.points[k], &self.points[k + 1]);
        a.at.y + (b.at.y - a.at.y) * t as f64
    }

    fn station(&self, s: f32) -> Station {
        let (k, t) = self.locate(s);
        let mut st = self.raw(s);
        st.phase = lerp(self.phase[k], self.phase[k + 1], t);
        st
    }

    /// The water's surface height and centre depth at `s`, steps and pools included.
    fn surface(&self, st: &Station) -> (f64, f32) {
        let (widths, amount) = step_spacing(st.slope);
        let (above, depth) = step_profile(st.phase.fract());
        let step_drop = st.slope * widths * st.width.max(0.3);
        let y = st.at.y + (above * step_drop * amount) as f64;
        let depth = st.depth * lerp(1.0, depth, amount);
        (y, depth.max(0.01))
    }
}

/// A reach's water surface with its steps and pools, as the ribbons draw it: for a terrain
/// (or a demo) to carve a bed that matches, so a simulation of the creek steps and pools too.
pub struct StepCourse(Course);

impl StepCourse {
    /// The reach's points, upstream first.
    pub fn new(points: Vec<CreekPoint>) -> Self {
        Self(Course::new(points))
    }

    /// The surface's height and the depth in the middle, `s` meters from the source and `x`
    /// across it (-1 .. 1, bank to bank, positive to the left looking downstream).
    pub fn surface(&self, s: f32, x: f32) -> (f64, f32) {
        let st = self.0.station(s.clamp(0.0, self.0.length()));
        let (_, amount) = step_spacing(st.slope);
        self.0.surface(&Station {
            phase: st.phase + step_skew(x, st.phase) * amount,
            ..st
        })
    }
}

fn horizontal(a: DVec3, b: DVec3) -> f32 {
    DVec2::new(b.x - a.x, b.z - a.z).length() as f32
}

fn catmull_rom(p0: DVec3, p1: DVec3, p2: DVec3, p3: DVec3, t: f64) -> DVec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (p2 - p0) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3)
}

fn catmull_rom_tangent(p0: DVec3, p1: DVec3, p2: DVec3, p3: DVec3, t: f64) -> DVec3 {
    let t2 = t * t;
    0.5 * ((p2 - p0)
        + 2.0 * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t
        + 3.0 * (3.0 * p1 - p0 - 3.0 * p2 + p3) * t2)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How far each step's lip is moved along the creek at `x` across it (-1..1 bank to bank), in
/// steps: a few tenths, different for every step (the steps are numbered from mid-pool, where
/// moving between two numbers changes the surface least).
fn step_skew(x: f32, phase: f32) -> f32 {
    let n = (phase + 0.6).floor();
    let h1 = (n * 12.9898).sin() * 43758.545;
    let h2 = (n * 78.233).sin() * 12543.27;
    let (h1, h2) = (h1 - h1.floor(), h2 - h2.floor());
    0.09 * (2.3 * x + h1 * std::f32::consts::TAU).sin()
        + 0.05 * (5.1 * x + h2 * std::f32::consts::TAU).sin()
}

/// White water at a station: where the water shoots off a step's lip faster than a wave can
/// run (Froude number over 1), a hydraulic jump boils white at its foot and the froth is
/// carried down the pool (Grant and Mizuyama 1991). Far off, where a step is under a pixel
/// or two, its share averaged over the step, so it doesn't flicker as rings are relaid.
fn white_water(st: &Station, amount: f32, distance: f32) -> f32 {
    if amount <= 0.0 {
        return 0.0;
    }
    let (_, chute) = step_profile(0.95);
    let depth = (st.depth * chute).max(0.01);
    let q = st.flow / st.width.max(0.1);
    let speed = (1.25 * q / (depth * (2.0 / 3.0))).min(MOST_SPEED);
    let froude = speed / (9.81 * depth).sqrt();
    let strength = amount * smoothstep(1.0, 2.5, froude);
    let f = st.phase.fract();
    let local = (1.0 - smoothstep(0.0, 0.35, f)) + 0.5 * smoothstep(0.85, 1.0, f);
    strength * lerp(local, 0.2, smoothstep(80.0, 160.0, distance))
}

/// Positions across the ribbon (as shares of the half-width) by the camera's distance.
fn across(distance: f32) -> &'static [f32] {
    if distance < 80.0 {
        &[-EDGE, -0.8, -0.4, 0.0, 0.4, 0.8, EDGE]
    } else if distance < 300.0 {
        &[-EDGE, -0.55, 0.0, 0.55, EDGE]
    } else {
        &[-EDGE, 0.0, EDGE]
    }
}

/// Lay the ribbon of one reach (`points` in world space) seen from `camera`, relative to
/// `anchor`, onto `vertices` and `indices`. Returns whether any of it was in range.
pub fn lay_ribbon(
    points: Vec<CreekPoint>,
    camera: DVec3,
    anchor: DVec3,
    vertices: &mut Vec<CreekVertex>,
    indices: &mut Vec<u32>,
) -> bool {
    lay_ribbon_around(points, camera, anchor, None, vertices, indices)
}

/// [`lay_ribbon`], leaving out the stretch `skip` (meters from the source, from .. to) where
/// a simulation draws the water ([`crate::creek_sim`]); the ribbon reaches
/// [`SKIP_OVERLAP_M`] into it at either end, so no gap opens between the two.
pub fn lay_ribbon_around(
    points: Vec<CreekPoint>,
    camera: DVec3,
    anchor: DVec3,
    skip: Option<(f32, f32)>,
    vertices: &mut Vec<CreekVertex>,
    indices: &mut Vec<u32>,
) -> bool {
    if points.len() < 2 {
        return false;
    }
    let course = Course::new(points);
    let length = course.length();
    if length < 0.5 {
        return false;
    }
    let mut s = 0.0f32;
    // The ring before, if it was laid: its first vertex and its count.
    let mut last: Option<(u32, usize)> = None;
    let mut any = false;
    loop {
        let st = course.station(s);
        let d = horizontal(st.at, camera).max((st.at - camera).length() as f32);
        let range = creek_range(st.width);
        let near_spacing = (0.6 * st.width).clamp(0.75, 3.0);
        let mut spacing = near_spacing.max(0.01 * d).min(12.0);
        let (widths, amount) = step_spacing(st.slope);
        if amount > 0.0 && d < STEP_DETAIL_M {
            // Near, a tenth of a step apart, so no chute falls between two rings.
            spacing = spacing.min(0.1 * widths * st.width.max(0.3)).max(0.12);
        }
        if d > range {
            last = None;
            if s >= length {
                break;
            }
            // Skip ahead, out of range: nothing here can come into range sooner.
            s = (s + (d - range).max(spacing)).min(length);
            continue;
        }
        if let Some((a, b)) = skip
            && s > a + SKIP_OVERLAP_M
            && s < b - SKIP_OVERLAP_M
        {
            last = None;
            s = (b - SKIP_OVERLAP_M).min(length);
            continue;
        }
        let dir = st.dir;
        let side = Vec2::new(-dir.y, dir.x);
        let half = 0.5 * st.width;
        let q = st.flow / st.width.max(0.1);
        // The surface here and either side (for its slope), with the step layout moved on by
        // `skew` steps.
        let e = 0.25f32;
        let around = [
            course.station((s - e).max(0.0)),
            st,
            course.station((s + e).min(length)),
        ];
        let surface = |k: usize, skew: f32| {
            course.surface(&Station {
                phase: around[k].phase + skew,
                ..around[k]
            })
        };
        let ring = across(d);
        let first = vertices.len() as u32;
        for &a in ring {
            let x = a / EDGE;
            // A step's lip runs across the bed crooked, from boulder to boulder.
            let skew = step_skew(x, st.phase) * amount;
            let (y, depth_c) = surface(1, skew);
            // The surface's slope along the course, for the normal.
            let grade = ((surface(0, skew).0 - surface(2, skew).0) / (2.0 * e) as f64) as f32;
            let normal = Vec3::new(dir.x * grade, 1.0, dir.y * grade).normalize();
            // Mean depth over the parabolic section is two thirds of the centre's; the centre
            // runs a quarter faster than the mean.
            let centre = (1.25 * q / (depth_c * (2.0 / 3.0))).min(MOST_SPEED);
            let white = white_water(
                &Station {
                    phase: st.phase + skew,
                    ..st
                },
                amount,
                d,
            );
            let depth = depth_c * (1.0 - x * x).max(0.0);
            let u = centre * (1.0 - a.abs().min(1.0).powi(4));
            let shear = centre * 4.0 * a.abs().min(1.0).powi(3) / half.max(0.1) * a.signum();
            let off = side * (a * half);
            let p = DVec3::new(st.at.x + off.x as f64, y, st.at.z + off.y as f64) - anchor;
            vertices.push(CreekVertex {
                pos_depth: [p.x as f32, p.y as f32, p.z as f32, depth],
                vel_shear: [dir.x * u, dir.y * u, shear, s],
                normal: [
                    normal.x,
                    normal.y,
                    normal.z,
                    white * (1.0 - 0.6 * x.powi(4)),
                ],
            });
        }
        if let Some((prev, prev_n)) = last {
            stitch(prev, prev_n, first, ring.len(), indices);
            any = true;
        }
        last = Some((first, ring.len()));
        if s >= length {
            break;
        }
        s = (s + spacing).min(length);
    }
    any
}

/// Triangles between two rings of `a_n` and `b_n` vertices (3, 5 or 7 each).
fn stitch(a: u32, a_n: usize, b: u32, b_n: usize, indices: &mut Vec<u32>) {
    // Walk both rings across together, always advancing the one further behind.
    let (mut i, mut j) = (0usize, 0usize);
    while i + 1 < a_n || j + 1 < b_n {
        let ta = (i as f32) / (a_n - 1) as f32;
        let tb = (j as f32) / (b_n - 1) as f32;
        let next_a = (i + 1) as f32 / (a_n - 1) as f32;
        let next_b = (j + 1) as f32 / (b_n - 1) as f32;
        if j + 1 >= b_n || (i + 1 < a_n && next_a - tb <= next_b - ta) {
            indices.extend_from_slice(&[a + i as u32, a + i as u32 + 1, b + j as u32]);
            i += 1;
        } else {
            indices.extend_from_slice(&[a + i as u32, b + j as u32 + 1, b + j as u32]);
            j += 1;
        }
    }
}

/// Ribbons laid for one camera position.
pub struct Laid {
    anchor: DVec3,
    vertices: Vec<CreekVertex>,
    indices: Vec<u32>,
    reaches: usize,
}

/// A creek provider and where its space sits in the world.
pub type Provider = (Arc<dyn CreekQuery>, DVec3);

/// A provider's identity: its address (the same provider keeps it while it lives).
pub fn provider_key(p: &Arc<dyn CreekQuery>) -> usize {
    Arc::as_ptr(p) as *const () as usize
}

/// A stretch of a reach the ribbons leave out: a simulation draws the water there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cut {
    /// [`provider_key`] of its provider.
    pub provider: usize,
    /// The reach's id.
    pub reach: u64,
    /// Meters from the reach's source, from .. to.
    pub from_m: f32,
    pub to_m: f32,
}

/// Every provider of creeks in the world: the terrain's, and each [`LocalCreeks`] where its
/// entity stands.
pub fn gather_providers(
    creeks: Option<&Creeks>,
    locals: &Query<(&LocalCreeks, &GlobalTransform)>,
) -> Vec<Provider> {
    let mut providers: Vec<Provider> = Vec::new();
    if let Some(c) = creeks {
        providers.push((c.0.clone(), DVec3::ZERO));
    }
    for (local, g) in locals {
        providers.push((local.0.clone(), g.translation()));
    }
    providers
}

/// Where the main camera is (else the first focus point).
pub fn main_camera(
    cameras: &Query<(&engine_render::Camera, &GlobalTransform), Without<engine_render::ExtraView>>,
    focus: &engine_world::FocusPoints,
) -> Option<DVec3> {
    cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, g)| g.translation())
        .or_else(|| focus.points.first().copied())
}

/// Lay the ribbons of every reach within reach of `camera`.
pub fn lay_ribbons(providers: &[Provider], camera: DVec3) -> Laid {
    lay_ribbons_cut(providers, camera, &[])
}

/// [`lay_ribbons`], leaving out the `cuts`.
pub fn lay_ribbons_cut(providers: &[Provider], camera: DVec3, cuts: &[Cut]) -> Laid {
    let _span = tracing::info_span!("water", name = "creek ribbons").entered();
    let anchor = (camera / 16.0).round() * 16.0;
    let mut laid = Laid {
        anchor,
        vertices: Vec::new(),
        indices: Vec::new(),
        reaches: 0,
    };
    let mut reaches: Vec<CreekReach> = Vec::new();
    for (provider, offset) in providers {
        let key = provider_key(provider);
        reaches.clear();
        let c = camera - *offset;
        let lo = DVec2::new(c.x - QUERY_M, c.z - QUERY_M);
        let hi = DVec2::new(c.x + QUERY_M, c.z + QUERY_M);
        provider.reaches(lo, hi, &mut reaches);
        for reach in reaches.drain(..) {
            let skip = cuts
                .iter()
                .find(|c| c.provider == key && c.reach == reach.id)
                .map(|c| (c.from_m, c.to_m));
            let points = reach
                .points
                .into_iter()
                .map(|p| CreekPoint {
                    at: p.at + *offset,
                    ..p
                })
                .collect();
            if lay_ribbon_around(
                points,
                camera,
                anchor,
                skip,
                &mut laid.vertices,
                &mut laid.indices,
            ) {
                laid.reaches += 1;
            }
        }
    }
    laid
}

impl CreekRibbons {
    /// Laid as they stand: nothing being laid, and laid without every stretch simulated now.
    pub fn settled(&self) -> bool {
        self.job.is_none() && self.cuts == self.cuts_laid
    }

    fn install(&mut self, laid: Laid) {
        self.anchor = laid.anchor;
        self.vertices = laid.vertices;
        self.indices = laid.indices;
        self.reaches = laid.reaches;
        self.version += 1;
    }
}

/// [`lay_ribbons`] into `out` at once.
pub fn build_ribbons(providers: &[Provider], camera: DVec3, out: &mut CreekRibbons) {
    out.install(lay_ribbons(providers, camera));
}

/// Keeps [`CreekRibbons`] laid around the camera (see the module docs). They are laid on a
/// worker (10 to 25 ms for the demo meadow's 45 creeks on the cloud machine) and drawn where
/// they were until the new ones land.
#[allow(clippy::type_complexity)]
pub fn creek_ribbons_system(
    time: Res<Time>,
    creeks: Option<Res<Creeks>>,
    locals: Query<(&LocalCreeks, &GlobalTransform)>,
    cameras: Query<(&engine_render::Camera, &GlobalTransform), Without<engine_render::ExtraView>>,
    focus: Res<engine_world::FocusPoints>,
    mut shifts: MessageReader<WorldShift>,
    mut ribbons: ResMut<CreekRibbons>,
) {
    let shifted = shifts.read().copied().reduce(|a, next| a.then(&next));
    if let Some(s) = shifted {
        // Drawn where they were until laid again in the new frame, below; one being laid in
        // the old frame is dropped.
        ribbons.anchor = s.point(ribbons.anchor);
        ribbons.built_at = None;
        ribbons.job = None;
    }
    if let Some(laid) = ribbons.job.as_mut().and_then(check_ready) {
        ribbons.job = None;
        ribbons.install(laid);
    }
    ribbons.since += time.delta_secs();
    if (ribbons.since < PERIOD_S && shifted.is_none()) || ribbons.job.is_some() {
        return;
    }
    ribbons.since = 0.0;
    let Some(camera) = main_camera(&cameras, &focus) else {
        return;
    };
    let providers = gather_providers(creeks.as_deref(), &locals);
    let keys: Vec<(usize, DVec3)> = providers
        .iter()
        .map(|(p, at)| (provider_key(p), *at))
        .collect();
    let moved = ribbons
        .built_at
        .is_none_or(|b| b.distance(camera) > REBUILD_M);
    if !moved && keys == ribbons.providers && ribbons.cuts == ribbons.cuts_laid {
        return;
    }
    ribbons.built_at = Some(camera);
    ribbons.providers = keys;
    ribbons.cuts_laid = ribbons.cuts.clone();
    let cuts = ribbons.cuts.clone();
    match AsyncComputeTaskPool::try_get() {
        Some(pool) => {
            ribbons.job =
                Some(pool.spawn(async move { lay_ribbons_cut(&providers, camera, &cuts) }));
        }
        None => {
            let laid = lay_ribbons_cut(&providers, camera, &cuts);
            ribbons.install(laid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn straight(length: f32, slope: f32, width: f32, flow: f32) -> Vec<CreekPoint> {
        (0..=(length / 4.0) as usize)
            .map(|k| {
                let x = k as f64 * 4.0;
                CreekPoint {
                    at: DVec3::new(x, 100.0 - x * slope as f64, 0.0),
                    width_m: width,
                    depth_m: 0.3,
                    flow_m3_s: flow,
                }
            })
            .collect()
    }

    #[test]
    fn steps_follow_the_slope_bands() {
        // Cascades: steps under a width apart; step-pools 1-4 widths; plane beds none.
        assert!(step_spacing(0.1).0 < 1.0);
        let (w, on) = step_spacing(0.04);
        assert!((1.0..=4.0).contains(&w) && on > 0.9, "{w} {on}");
        assert_eq!(step_spacing(0.01).1, 0.0);
        // The step's surface averages out on the even line, and its chute is at the end.
        let n = 1000;
        let mean: f32 = (0..n)
            .map(|k| step_profile((k as f32 + 0.5) / n as f32).0)
            .sum::<f32>()
            / n as f32;
        assert!(mean.abs() < 0.02, "{mean}");
        assert!(step_profile(0.95).1 < step_profile(0.1).1);
    }

    #[test]
    fn chutes_run_fast_and_pools_slow() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let camera = DVec3::new(30.0, 100.0, 5.0);
        assert!(lay_ribbon(
            straight(80.0, 0.05, 2.0, 0.4),
            camera,
            DVec3::ZERO,
            &mut v,
            &mut i
        ));
        let speeds: Vec<f32> = v
            .iter()
            .filter(|p| p.pos_depth[2].abs() < 0.01)
            .map(|p| Vec2::new(p.vel_shear[0], p.vel_shear[1]).length())
            .collect();
        let (lo, hi) = speeds
            .iter()
            .fold((f32::MAX, 0.0f32), |(a, b), s| (a.min(*s), b.max(*s)));
        // Through the pools well under a meter a second; over the lips three times that.
        assert!(lo < 0.8 && hi > 3.0 * lo, "{lo} {hi}");
        // Downstream, all of it.
        assert!(v.iter().all(|p| p.vel_shear[0] >= 0.0));
        // White water at the foot of each step, none down most of the pool.
        let white: Vec<f32> = v.iter().map(|p| p.normal[3]).collect();
        assert!(white.iter().any(|w| *w > 0.6));
        assert!(white.iter().filter(|w| **w < 0.05).count() > white.len() / 3);
        // The ribbon stays within its creek: 15% past the banks, no more.
        assert!(v.iter().all(|p| p.pos_depth[2].abs() <= 1.0 * EDGE + 1e-3));
        // A gentle reach has no steps: its surface follows the even line.
        let (mut v2, mut i2) = (Vec::new(), Vec::new());
        lay_ribbon(
            straight(80.0, 0.005, 2.0, 0.4),
            camera,
            DVec3::ZERO,
            &mut v2,
            &mut i2,
        );
        assert!(v2.iter().all(|p| {
            let x = p.pos_depth[0] as f64;
            (p.pos_depth[1] as f64 - (100.0 - x * 0.005)).abs() < 0.02
        }));
    }

    #[test]
    fn far_reaches_get_fewer_rings_and_the_farthest_none() {
        let (mut near_v, mut near_i) = (Vec::new(), Vec::new());
        let (mut far_v, mut far_i) = (Vec::new(), Vec::new());
        let reach = straight(200.0, 0.04, 1.0, 0.1);
        lay_ribbon(
            reach.clone(),
            DVec3::new(100.0, 100.0, 10.0),
            DVec3::ZERO,
            &mut near_v,
            &mut near_i,
        );
        lay_ribbon(
            reach.clone(),
            DVec3::new(100.0, 100.0, 250.0),
            DVec3::ZERO,
            &mut far_v,
            &mut far_i,
        );
        assert!(
            far_i.len() * 3 < near_i.len(),
            "{} {}",
            far_i.len(),
            near_i.len()
        );
        assert!(!far_i.is_empty());
        // A 1 m brook 600 m off is under 4 pixels wide: not drawn.
        let (mut v, mut i) = (Vec::new(), Vec::new());
        assert!(!lay_ribbon(
            reach,
            DVec3::new(100.0, 100.0, 600.0),
            DVec3::ZERO,
            &mut v,
            &mut i
        ));
        assert!(i.is_empty());
        // Every index points at a vertex.
        assert!(near_i.iter().all(|k| (*k as usize) < near_v.len()));
    }

    #[test]
    fn the_step_layout_stays_put_as_the_camera_moves() {
        // The same place along the reach gets the same surface from two cameras.
        let reach = straight(120.0, 0.05, 2.0, 0.3);
        let course = Course::new(reach);
        let a = course.surface(&course.station(37.3));
        let course2 = Course::new(straight(120.0, 0.05, 2.0, 0.3));
        let b = course2.surface(&course2.station(37.3));
        assert_eq!(a, b);
    }
}
