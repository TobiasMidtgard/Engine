//! Stones in every creek near the camera: real rocks standing in the water out to
//! [`STONES_M`], which the ribbons' water parts round ([`crate::creeks`]) and the near
//! simulation flows round ([`crate::creek_sim`]).
//!
//! Each reach gets one layout ([`lay_reach_stones`]), by the kind of bed its slope makes
//! (Montgomery and Buffington 1997, as the ribbons' steps): a line of stones shoulder to
//! shoulder along each step's crooked lip where the creek steps and pools (the stones the step
//! is built of), boulders choking the cascades, a stone here and there on gentle plane beds,
//! and boulders along both banks breaking up their edges. Sizes follow the creek's width. The
//! layout depends on the reach alone, so the ribbons, the simulation and the drawn rocks agree
//! and nothing jumps where one hands over to another.
//!
//! [`creek_stones_system`] lays the reaches near the camera on a worker, merges their rocks a
//! stretch at a time (so stretches out of view are culled and far ones simplified) and keeps
//! those within range spawned: big ones out to [`STONES_M`], small ones to [`SMALL_STONES_M`].

use std::collections::HashMap;
use std::sync::Arc;

use bevy_tasks::Task;
use bevy_tasks::futures::check_ready;
use engine_core::math::{DVec2, DVec3, Mat4, Rng, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_world::terrain::{CreekPoint, CreekReach, Creeks};
use engine_world::{Ground, WorldShift};

use crate::creeks::{
    Course, LocalCreeks, Station, gather_providers, main_camera, provider_key, step_skew,
    step_spacing,
};
use crate::demo::props::Part;
use crate::demo::scenery::{merged, spawn_prop};
use crate::demo::valley::StreamStone;
use crate::flow::{FALL_POINTS, FallPoint, Lip};

/// Big stones are drawn this far from the camera, meters.
pub const STONES_M: f64 = 150.0;
/// Stones smaller than [`SMALL_M`] this far.
pub const SMALL_STONES_M: f64 = 80.0;
/// Stones under this size, meters, are small.
const SMALL_M: f32 = 0.5;
/// Reaches this near are laid (a little beyond the farthest stones drawn).
const LAY_M: f64 = 200.0;
/// Stretches of a reach merged into one mesh, meters along it.
const CHUNK_M: f32 = 32.0;
/// Spawned stretches go this much farther out than they come in, meters.
const HYSTERESIS_M: f64 = 20.0;
/// How often the camera is looked at, seconds.
const PERIOD_S: f32 = 0.25;
/// Where along a step's surface its lip runs (`step_profile`: the pool's end, the chute's
/// start), and a little into the chute.
const LIP: f32 = crate::creeks::STEP_POOL;

/// A reach's stones, in the order they lie down it.
#[derive(Debug, Default)]
pub struct ReachStones {
    pub stones: Vec<StreamStone>,
    /// Meters from the reach's source to each stone.
    pub along: Vec<f32>,
    /// Where water pours off a step's lip and falls free into the pool below, in the order
    /// they lie down the reach ([`PourOver`]).
    pub pours: Vec<PourOver>,
}

/// Water pouring off a stretch of a step's lip into the pool below: the lip, its path down
/// and whether the sheet carries on past either end (as [`crate::flow::Falls`] holds them,
/// so the waterfalls' renderer draws it).
///
/// On stepped beds the water leaves each step as a free jet (nappe or tumbling flow) while
/// the critical depth d_c = (q²/g)^(1/3) is under about (1.2 − 0.325 h/l) times the step's
/// drop h (l its length); above that it skims over the steps (Chanson, stepped chutes). Natural
/// step-pool creeks run tumbling at all but high flows (Comiti et al. 2007). At the brink the
/// water is about 0.715 d_c deep (Rouse 1936), so it leaves at q / 0.715 d_c.
#[derive(Debug, Clone)]
pub struct PourOver {
    /// Meters from the reach's source to the lip.
    pub along: f32,
    pub lip: Lip,
    pub path: Vec<FallPoint>,
    pub sides: [bool; 2],
}

/// Pour-overs are drawn this far from the camera, meters.
pub const POURS_M: f64 = 60.0;
/// Steps that drop less than this pour as a ripple over the lip rather than a fall, meters.
const LEAST_POUR_M: f32 = 0.15;

impl ReachStones {
    /// The stones between `from` and `to` meters down the reach (indices).
    pub fn between(&self, from: f32, to: f32) -> std::ops::Range<usize> {
        let a = self.along.partition_point(|s| *s < from);
        let b = self.along.partition_point(|s| *s <= to);
        a..b.max(a)
    }
}

/// A seed for a reach's stones: the same wherever and whenever it is laid.
pub fn reach_seed(reach: &CreekReach) -> u64 {
    let first = reach.points.first().map_or(DVec3::ZERO, |p| p.at);
    reach.id.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ ((first.x * 8.0).round() as i64 as u64).rotate_left(21)
        ^ ((first.z * 8.0).round() as i64 as u64).rotate_left(43)
}

/// Lay a reach's stones (see the module docs). `points` are where the reach runs, `ground` the
/// height of its bed at x, z (the stones sit on it).
pub fn lay_reach_stones(
    points: &[CreekPoint],
    ground: &dyn Fn(f64, f64) -> f64,
    seed: u64,
) -> ReachStones {
    let mut out: Vec<(f32, StreamStone)> = Vec::new();
    if points.len() < 2 {
        return ReachStones::default();
    }
    let course = Course::new(points.to_vec());
    let length = course.length();
    if length < 4.0 {
        return ReachStones::default();
    }
    let mut rng = Rng::new(seed ^ 0xC2EE_5704);
    let place = |rng: &mut Rng, s: f32, x: f32, size: f32, out: &mut Vec<(f32, StreamStone)>| {
        let st = course.station(s.clamp(0.0, length));
        let side = Vec2::new(-st.dir.y, st.dir.x);
        let off = side * (x * 0.5 * st.width);
        let (px, pz) = (st.at.x + off.x as f64, st.at.z + off.y as f64);
        let stretch = Vec3::new(
            rng.range(0.85, 1.25),
            rng.range(0.75, 1.05),
            rng.range(0.85, 1.2),
        );
        let size = size.clamp(0.18, 1.8);
        let k = out.len();
        out.push((
            s,
            StreamStone {
                at: DVec3::new(px, ground(px, pz), pz),
                size,
                // A stone stands about 0.42 of its size over where it is half buried.
                height: size * 0.42 * stretch.y,
                yaw: rng.range(0.0, std::f32::consts::TAU),
                stretch,
                shape: crate::demo::rock_for(size, k).0,
            },
        ));
    };
    // Along each step's lip: stones shoulder to shoulder across the bed, the lip running
    // crooked between them as the ribbon draws it (`step_skew`).
    let last = course.phase.last().copied().unwrap_or(0.0);
    let mut n = 0.0f32;
    while n + LIP < last {
        let lip = n + LIP;
        n += 1.0;
        let Some(s) = course.at_phase(lip) else {
            continue;
        };
        if s < 2.0 || s > length - 2.0 {
            continue;
        }
        let st = course.station(s);
        let (_, amount) = step_spacing(st.slope);
        if rng.f32() > amount * 1.3 - 0.2 {
            continue;
        }
        let width = st.width.max(0.4);
        let mut x = -1.0 + rng.range(0.05, 0.25);
        while x < 0.95 {
            let size = (width * rng.range(0.32, 0.5)).max(0.3) * (0.75 + 0.4 * amount);
            let at = course
                .at_phase(lip - step_skew(x, lip) * amount)
                .unwrap_or(s);
            if rng.f32() < 0.85 {
                let at = at + rng.range(-0.05, 0.05) * width;
                place(&mut rng, at, x, size, &mut out);
                // The step is built of these stones: their crowns break the pool's surface at
                // the lip by a hand or so, rather than damming it higher (the bed is carved
                // to the step already).
                let (level, _) = course.surface(&Station {
                    phase: lip,
                    ..course.station(at.clamp(0.0, length))
                });
                if let Some((_, stone)) = out.last_mut() {
                    let top = level + rng.range(0.04, 0.12) as f64;
                    stone.height = ((top - stone.at.y) as f32).clamp(0.03, stone.height);
                }
            }
            x += 2.0 * size * 0.85 / width;
        }
        // Cascades: boulders strewn in the pools between the lips too.
        if st.slope > 0.065 && rng.f32() < 0.6 {
            let size = width * rng.range(0.35, 0.6);
            let back = 0.4 * step_spacing(st.slope).0 * width;
            let x = rng.range(-0.6, 0.6);
            place(&mut rng, s - back, x, size, &mut out);
        }
    }
    // Plane beds and riffles: a stone here and there, now and then with a smaller one beside.
    let mut s = 3.0f32;
    while s < length - 3.0 {
        let st = course.station(s);
        let width = st.width.max(0.4);
        let (_, amount) = step_spacing(st.slope);
        if rng.f32() > amount {
            let size = (width * rng.range(0.25, 0.5)).max(0.25);
            let x = rng.range(-0.65, 0.65);
            place(&mut rng, s, x, size, &mut out);
            if rng.f32() < 0.35 {
                let beside = x + rng.range(0.25, 0.45) * if x > 0.0 { -1.0 } else { 1.0 };
                let at = s + rng.range(-0.3, 0.3) * width;
                place(&mut rng, at, beside, size * 0.55, &mut out);
            }
        }
        s += width * rng.range(3.0, 6.0);
    }
    // The banks, all the way down: a boulder every few meters on one side or the other, half
    // on the bed and half up the bank.
    let mut s = 1.0f32;
    while s < length - 1.0 {
        let st = course.station(s);
        let width = st.width.max(0.4);
        let side = if rng.f32() < 0.5 { -1.0 } else { 1.0 };
        let size = (width * rng.range(0.4, 0.8)).max(rng.range(0.6, 1.3));
        let x = side * rng.range(0.9, 1.25);
        place(&mut rng, s, x, size, &mut out);
        s += (width * rng.range(1.2, 2.6)).max(rng.range(1.2, 2.5));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    let pours = lay_pours(&course, ground, &out, &mut rng);
    ReachStones {
        along: out.iter().map(|o| o.0).collect(),
        stones: out.into_iter().map(|o| o.1).collect(),
        pours,
    }
}

/// The pour-overs of every step down `course` (see [`PourOver`]): each lip in tumbling flow cut
/// into strips a few tens of centimeters wide across the bed, following its crooked line.
fn lay_pours(
    course: &Course,
    ground: &dyn Fn(f64, f64) -> f64,
    stones: &[(f32, StreamStone)],
    rng: &mut Rng,
) -> Vec<PourOver> {
    let length = course.length();
    let last = course.phase.last().copied().unwrap_or(0.0);
    let mut out: Vec<PourOver> = Vec::new();
    let mut n = 0.0f32;
    while n + LIP < last {
        let lip = n + LIP;
        n += 1.0;
        let Some(s) = course.at_phase(lip) else {
            continue;
        };
        if s < 2.0 || s > length - 2.0 {
            continue;
        }
        let st = course.station(s);
        let (widths, amount) = step_spacing(st.slope);
        if amount < 0.1 {
            continue;
        }
        let width = st.width.max(0.4);
        let half = 0.5 * width;
        let q_mean = st.flow / width;
        let step_length = widths * width;
        // Strips about 0.3 m wide over the wet middle of the bed (the edges are too shallow
        // to pour).
        let strips = ((1.7 * half / 0.3).round() as usize).clamp(3, 10);
        let strip = 1.7 * half / strips as f32;
        let mut joined = false;
        for k in 0..strips {
            let x = -0.85 + (k as f32 + 0.5) * 1.7 / strips as f32;
            let skew = step_skew(x, lip) * amount;
            let Some(at) = course.at_phase(lip - skew) else {
                continue;
            };
            // (`n` already counts on to the next step: its start is this step's foot.)
            let Some(foot_at) = course.at_phase(n - skew) else {
                continue;
            };
            let here = course.station(at);
            let side = Vec2::new(-here.dir.y, here.dir.x);
            let off = side * (x * half);
            let top_y = course.surface(&Station { phase: lip, ..here }).0;
            let foot_y = course
                .surface(&Station {
                    phase: n,
                    ..course.station(foot_at)
                })
                .0;
            let drop = (top_y - foot_y) as f32;
            // Water over a meter of lip here: the parabolic section carries more mid-bed.
            let q = (q_mean * 1.5 * (1.0 - x * x)).max(0.005);
            let critical = (q * q / 9.81).cbrt();
            let skims = critical / drop.max(1e-3) > 1.2 - 0.325 * drop / step_length.max(0.1);
            if drop < LEAST_POUR_M * (0.8 + 0.4 * rng.f32()) || skims {
                // A break in the sheet: the strips either side end here.
                if joined && let Some(prev) = out.last_mut() {
                    prev.sides[1] = false;
                }
                joined = false;
                continue;
            }
            let lip = Lip {
                top: DVec3::new(here.at.x + off.x as f64, top_y, here.at.z + off.y as f64),
                dir: here.dir,
                speed: (q / (0.715 * critical)).clamp(0.3, 4.0),
                q,
                drop,
                width: strip,
                launch: 0.0,
                // Rough rock lips: several times a smooth weir's 0.012.
                turbulence: 0.05,
            };
            let path = pour_path(&lip, foot_y, ground);
            // No water where a boulder stands well out of it (the lip's own stones, a hand
            // over the water, are what it pours between and over).
            let blocked = stones.iter().any(|(along, stone)| {
                (along - at).abs() < 4.0
                    && stone.at.y + stone.height as f64 > top_y + 0.15
                    && path.iter().step_by(2).any(|f| {
                        DVec2::new(f.pos.x - stone.at.x, f.pos.z - stone.at.z).length()
                            < (0.85 * stone.radius()) as f64
                    })
            });
            if blocked {
                if joined && let Some(prev) = out.last_mut() {
                    prev.sides[1] = false;
                }
                joined = false;
                continue;
            }
            out.push(PourOver {
                along: at,
                lip,
                path,
                sides: [joined, true],
            });
            joined = true;
        }
        if joined && let Some(last) = out.last_mut() {
            last.sides[1] = false;
        }
    }
    out.sort_by(|a, b| a.along.total_cmp(&b.along));
    out
}

/// The path of water poured off `lip` (see [`crate::flow::FlowSim::fall_path`]): free fall,
/// and where it meets the face of the step (the bed from `ground`), sliding down it until it
/// reaches the pool's surface at `pool`. [`FALL_POINTS`] segments, evenly spaced.
fn pour_path(lip: &Lip, pool: f64, ground: &dyn Fn(f64, f64) -> f64) -> Vec<FallPoint> {
    const G: f32 = 9.81;
    const H: f32 = 1.0 / 240.0;
    // The sheet's thickness on the rock.
    const SHEET: f32 = 0.04;
    let normal_at = |x: f64, z: f64| {
        let e = 0.05;
        let dx = (ground(x + e, z) - ground(x - e, z)) / (2.0 * e);
        let dz = (ground(x, z + e) - ground(x, z - e)) / (2.0 * e);
        Vec3::new(-dx as f32, 1.0, -dz as f32).normalize()
    };
    let mut p = lip.top;
    let mut v = Vec3::new(lip.dir.x * lip.speed, 0.0, lip.dir.y * lip.speed);
    let mut rock = Vec3::ZERO;
    let mut t = 0.0f32;
    let mut path = vec![(p, v, t, rock)];
    while t < 1.5 {
        t += H;
        if rock == Vec3::ZERO {
            v.y -= G * H;
        } else {
            let down = Vec3::new(0.0, -G, 0.0);
            v += (down - rock * down.dot(rock)) * H;
            v *= 1.0 - 1.2 * H;
        }
        p += (v * H).as_dvec3();
        let bed = ground(p.x, p.z);
        if p.y <= pool {
            p.y = pool;
            path.push((p, v, t, rock));
            break;
        }
        if rock != Vec3::ZERO && p.y > bed + (SHEET + 0.1) as f64 {
            // The face drops away under it: it leaves the rock.
            rock = Vec3::ZERO;
        } else if p.y <= bed + SHEET as f64 {
            if rock == Vec3::ZERO {
                // Hitting the face throws off some of its speed.
                v *= 0.7;
            }
            rock = normal_at(p.x, p.z);
            p.y = bed + SHEET as f64;
            let into = v.dot(rock);
            if into < 0.0 {
                v -= rock * into;
            }
        }
        path.push((p, v, t, rock));
    }
    // Even spacing along the path.
    let mut along = vec![0.0f64; path.len()];
    for k in 1..path.len() {
        along[k] = along[k - 1] + path[k].0.distance(path[k - 1].0);
    }
    let total = along.last().copied().unwrap_or(0.0).max(1e-4);
    let mut k = 0;
    (0..=FALL_POINTS)
        .map(|i| {
            let want = total * i as f64 / FALL_POINTS as f64;
            while k + 2 < path.len() && along[k + 1] < want {
                k += 1;
            }
            let (a, b) = (path[k], path[(k + 1).min(path.len() - 1)]);
            let span = (along[(k + 1).min(path.len() - 1)] - along[k]).max(1e-9);
            let f = ((want - along[k]) / span).clamp(0.0, 1.0);
            FallPoint {
                pos: a.0.lerp(b.0, f),
                vel: a.1.lerp(b.1, f as f32),
                t: a.2 + (b.2 - a.2) * f as f32,
                rock: if f < 0.5 { a.3 } else { b.3 },
            }
        })
        .collect()
}

/// The current round a stone (seen from above), for a point of the water near it: the
/// change in velocity (x, z), the rise of the surface, white water and the turn of the current
/// (shear, 1/s) it adds. `flow` is the current there without the stones, `level` the water's
/// surface and `depth` its depth in the middle of the creek.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AroundStones {
    pub velocity: Vec2,
    pub rise: f32,
    pub white: f32,
    pub shear: f32,
}

/// [`AroundStones`] at `p` (x, z) from `stones` (only the ones within a few of their radii
/// count). Potential flow round a cylinder: the water speeds up past the stone's sides and
/// stalls in front of it, where it heaps up in a pillow U²/2g high (stagnation); behind, a wake
/// trails white where the flow is fast enough to break, with eddy lines down both sides of it.
/// A stone well under the surface does less, as a hump the water slides over.
pub fn around_stones(
    stones: &[StreamStone],
    p: DVec2,
    flow: Vec2,
    level: f64,
    depth: f32,
) -> AroundStones {
    let mut out = AroundStones::default();
    let speed = flow.length();
    if speed < 0.02 {
        return out;
    }
    let along = flow / speed;
    let side = Vec2::new(-along.y, along.x);
    for s in stones {
        let r = s.radius();
        let d = Vec2::new((p.x - s.at.x) as f32, (p.y - s.at.z) as f32);
        let rho = d.length();
        if rho > 4.0 * r || rho < 1e-3 {
            continue;
        }
        // How much of the stone stands in the water's way: 1 where it breaks the surface.
        let top = s.at.y + s.height as f64;
        let below = (level - top) as f32;
        let block = 1.0 - (below / (0.6 * depth.max(0.05))).clamp(0.0, 1.0);
        if block <= 0.0 {
            continue;
        }
        let x = d.dot(along);
        let y = d.dot(side);
        let fade = 1.0 - smoothstep(2.5 * r, 4.0 * r, rho);
        let rr = (r / rho.max(r)).powi(2);
        let (cos, sin) = (x / rho, y / rho);
        let cos2 = cos * cos - sin * sin;
        let sin2 = 2.0 * sin * cos;
        // Potential flow round a cylinder, less the free stream.
        let du = speed * Vec2::new(-rr * cos2, -rr * sin2);
        out.velocity += (along * du.x + side * du.y) * fade * block;
        // The pillow in front.
        let head = speed * speed / (2.0 * 9.81);
        if x < 0.0 {
            out.rise += head * block * (-cos).powi(2) * (-(rho - r).max(0.0) / (0.6 * r)).exp();
        }
        if x > 0.0 {
            // The wake: white where it runs fast enough to break, fading downstream.
            let core = 1.0 - smoothstep(0.6 * r, 1.3 * r, y.abs());
            let down = (-x / (4.0 * r)).exp();
            out.white += block * smoothstep(0.8, 2.0, speed) * down * core * 0.4;
            // Eddy lines down both sides of it.
            let edge = (y.abs() - r) / (0.4 * r);
            out.shear +=
                block * speed / r * (-x / (5.0 * r)).exp() * (-edge * edge).exp() * y.signum();
        }
    }
    out.white = out.white.min(1.0);
    out
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A reach's stones by its provider's [`provider_key`] and its id.
pub type StoneKey = (usize, u64);

/// A stretch of a reach's stones spawned as one rock prop.
struct Spawned {
    entity: Entity,
    /// Its stones' middle and how far they spread from it.
    center: DVec3,
    radius: f64,
    big: bool,
}

/// A stretch's rocks merged to spawn: its reach, (chunk, big), middle, spread and parts.
type Merged = (StoneKey, (u32, bool), DVec3, f64, Vec<Part>);

/// What a worker laid: new reaches' stones and merged stretches to spawn.
pub struct StonesJob {
    laid: Vec<(StoneKey, Arc<ReachStones>)>,
    chunks: Vec<Merged>,
}

/// The stones near the camera (see the module docs).
#[derive(Resource, Default)]
pub struct CreekStones {
    /// No stones (for before pictures and comparisons).
    pub off: bool,
    reaches: HashMap<StoneKey, Arc<ReachStones>>,
    spawned: HashMap<(StoneKey, (u32, bool)), Spawned>,
    job: Option<Task<StonesJob>>,
    since: f32,
    /// Where the camera was when the last job was started.
    asked_at: Option<DVec3>,
    /// Bumped when a reach's stones are laid or dropped.
    pub version: u64,
}

impl CreekStones {
    /// The stones laid for a reach, if they are.
    pub fn reach(&self, key: StoneKey) -> Option<Arc<ReachStones>> {
        self.reaches.get(&key).cloned()
    }

    /// Every reach's stones laid now.
    pub fn all(&self) -> HashMap<StoneKey, Arc<ReachStones>> {
        self.reaches.clone()
    }

    /// Nothing being laid.
    pub fn settled(&self) -> bool {
        self.job.is_none()
    }

    /// Rock props spawned.
    pub fn props(&self) -> usize {
        self.spawned.len()
    }

    /// Rock props spawned with their middle within `r` meters of `p`.
    pub fn props_near(&self, p: DVec3, r: f64) -> usize {
        self.spawned
            .values()
            .filter(|s| s.center.distance(p) < r)
            .count()
    }
}

/// Which stretch (chunk index, big or small) a stone is merged into.
fn chunk_of(along: f32, size: f32) -> (u32, bool) {
    ((along / CHUNK_M).max(0.0) as u32, size >= SMALL_M)
}

fn range_of(big: bool) -> f64 {
    if big { STONES_M } else { SMALL_STONES_M }
}

/// The rocks of `stones` (indices `which`) merged about their middle: (middle, spread, parts).
fn merge_chunk(stones: &[StreamStone], which: &[usize]) -> (DVec3, f64, Vec<Part>) {
    let rocks = crate::demo::art::rocks();
    let center = which.iter().map(|&i| stones[i].at).sum::<DVec3>() / which.len() as f64;
    let radius = which
        .iter()
        .map(|&i| stones[i].at.distance(center) + stones[i].size as f64)
        .fold(0.0, f64::max);
    let parts = merged(which.iter().map(|&i| {
        let s = &stones[i];
        let built = crate::demo::rock_for(s.size, s.shape).1;
        // Its crown `height` over the ground under it: sunk deeper where that is less than its
        // shape stands (a step's stones, set into the bed).
        let stands = s.size * 0.42 * s.stretch.y;
        let sink = (s.height - stands).min(-0.05 * s.size);
        let m = Mat4::from_translation((s.at - center).as_vec3() + Vec3::Y * sink)
            * Mat4::from_rotation_y(s.yaw)
            * Mat4::from_scale(s.stretch * (s.size / built));
        (rocks[s.shape].clone(), m)
    }));
    (center, radius, parts)
}

/// Keeps the stones of the creeks near the camera laid and their rocks spawned (see the module
/// docs).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn creek_stones_system(
    mut commands: Commands,
    time: Res<Time>,
    creeks: Option<Res<Creeks>>,
    locals: Query<(&LocalCreeks, &GlobalTransform)>,
    cameras: Query<(&engine_render::Camera, &GlobalTransform), Without<engine_render::ExtraView>>,
    focus: Res<engine_world::FocusPoints>,
    ground: Option<Res<Ground>>,
    mut shifts: MessageReader<WorldShift>,
    mut state: ResMut<CreekStones>,
) {
    if shifts.read().count() > 0 {
        // Laid in the old frame: all laid again (a rare hitch-free relay on a worker).
        for (_, sp) in state.spawned.drain() {
            commands.entity(sp.entity).despawn();
        }
        state.reaches.clear();
        state.job = None;
        state.asked_at = None;
        state.version += 1;
    }
    if let Some(done) = state.job.as_mut().and_then(check_ready) {
        state.job = None;
        if !done.laid.is_empty() {
            state.version += 1;
        }
        for (key, stones) in done.laid {
            state.reaches.insert(key, stones);
        }
        for (key, chunk, center, radius, parts) in done.chunks {
            if state.spawned.contains_key(&(key, chunk)) {
                continue;
            }
            let entity = spawn_prop(
                &mut commands,
                "Creek stones",
                Transform::from_translation(center),
                parts,
            );
            state.spawned.insert(
                (key, chunk),
                Spawned {
                    entity,
                    center,
                    radius,
                    big: chunk.1,
                },
            );
        }
    }
    state.since += time.delta_secs();
    if state.since < PERIOD_S || state.job.is_some() {
        return;
    }
    state.since = 0.0;
    let Some(camera) = main_camera(&cameras, &focus) else {
        return;
    };
    if state.off {
        for (_, sp) in state.spawned.drain() {
            commands.entity(sp.entity).despawn();
        }
        return;
    }
    // Out of range: gone.
    let mut gone = Vec::new();
    for (k, sp) in &state.spawned {
        if sp.center.distance(camera) - sp.radius > range_of(sp.big) + HYSTERESIS_M {
            gone.push(*k);
        }
    }
    for k in gone {
        if let Some(sp) = state.spawned.remove(&k) {
            commands.entity(sp.entity).despawn();
        }
    }
    // Nothing new to lay or spawn till the camera moves on a little.
    if state.asked_at.is_some_and(|a| a.distance(camera) < 8.0) {
        return;
    }
    let Some(ground) = ground.map(|g| (*g).clone()) else {
        return;
    };
    state.asked_at = Some(camera);
    // The reaches near the camera, and which of their stretches should stand.
    let providers = gather_providers(creeks.as_deref(), &locals);
    let mut wanted: Vec<(StoneKey, Vec<CreekPoint>, Option<Arc<ReachStones>>)> = Vec::new();
    let mut reaches: Vec<CreekReach> = Vec::new();
    for (provider, offset) in &providers {
        let key = provider_key(provider);
        let c = camera - *offset;
        reaches.clear();
        provider.reaches(
            DVec2::new(c.x - LAY_M, c.z - LAY_M),
            DVec2::new(c.x + LAY_M, c.z + LAY_M),
            &mut reaches,
        );
        for reach in reaches.drain(..) {
            let near = reach
                .points
                .iter()
                .any(|p| DVec2::new(p.at.x - c.x, p.at.z - c.z).length() < LAY_M);
            if !near {
                continue;
            }
            let k = (key, reach.id);
            let points: Vec<CreekPoint> = reach
                .points
                .into_iter()
                .map(|p| CreekPoint {
                    at: p.at + *offset,
                    ..p
                })
                .collect();
            let have = state.reaches.get(&k).cloned();
            wanted.push((k, points, have));
        }
    }
    let spawned: Vec<(StoneKey, (u32, bool))> = state.spawned.keys().copied().collect();
    let probe =
        |points: &[CreekPoint]| points.iter().map(|p| p.at.y).fold(f64::MIN, f64::max) + 50.0;
    let work = move || {
        let mut job = StonesJob {
            laid: Vec::new(),
            chunks: Vec::new(),
        };
        for (key, points, have) in wanted {
            let stones = match have {
                Some(s) => s,
                None => {
                    let top = probe(&points);
                    let height =
                        |x: f64, z: f64| ground.height(DVec3::new(x, top, z)).unwrap_or(top - 50.0);
                    let reach = CreekReach {
                        id: key.1,
                        order: 0,
                        points: points.clone(),
                    };
                    let laid = Arc::new(lay_reach_stones(&points, &height, reach_seed(&reach)));
                    job.laid.push((key, laid.clone()));
                    laid
                }
            };
            // Group by stretch; spawn those in range and not standing.
            let mut groups: HashMap<(u32, bool), Vec<usize>> = HashMap::new();
            for (i, s) in stones.stones.iter().enumerate() {
                groups
                    .entry(chunk_of(stones.along[i], s.size))
                    .or_default()
                    .push(i);
            }
            for (chunk, which) in groups {
                if spawned.contains(&(key, chunk)) {
                    continue;
                }
                let near = which
                    .iter()
                    .map(|&i| stones.stones[i].at.distance(camera))
                    .fold(f64::MAX, f64::min);
                if near > range_of(chunk.1) {
                    continue;
                }
                let (center, radius, parts) = merge_chunk(&stones.stones, &which);
                job.chunks.push((key, chunk, center, radius, parts));
            }
        }
        job
    };
    engine_core::tasks::init_task_pools();
    state.job = Some(AsyncComputeTaskPool::get().spawn(async move { work() }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::creeks::meadow;

    fn flat(_: f64, _: f64) -> f64 {
        0.0
    }

    fn straight(length: f32, slope: f32, width: f32) -> Vec<CreekPoint> {
        (0..=(length / 5.0) as usize)
            .map(|i| {
                let s = i as f32 * 5.0;
                CreekPoint {
                    at: DVec3::new(s as f64, (-(s * slope)) as f64, 0.0),
                    width_m: width,
                    depth_m: 0.3,
                    flow_m3_s: 0.4,
                }
            })
            .collect()
    }

    #[test]
    fn steps_are_lined_with_stones_and_gentle_beds_scattered() {
        let steep = lay_reach_stones(&straight(200.0, 0.05, 2.0), &flat, 7);
        let gentle = lay_reach_stones(&straight(200.0, 0.005, 2.0), &flat, 7);
        let inside = |r: &ReachStones| r.stones.iter().filter(|s| s.at.z.abs() < 0.9).count();
        // Step lines: many more stones in the bed of a step-pool reach than a plane bed.
        assert!(
            inside(&steep) > 2 * inside(&gentle),
            "{} {}",
            inside(&steep),
            inside(&gentle)
        );
        // All of them in or on the edges of the channel, sorted down it.
        for r in [&steep, &gentle] {
            assert!(r.stones.iter().all(|s| s.at.z.abs() < 1.0 * 1.4 + 0.1));
            assert!(r.along.windows(2).all(|w| w[0] <= w[1]));
            assert_eq!(r.along.len(), r.stones.len());
        }
        // The same reach laid twice: the same stones.
        let again = lay_reach_stones(&straight(200.0, 0.05, 2.0), &flat, 7);
        assert_eq!(again.stones, steep.stones);
    }

    #[test]
    fn the_water_parts_round_a_stone() {
        let stone = StreamStone {
            at: DVec3::ZERO,
            size: 0.6,
            height: 0.4,
            yaw: 0.0,
            stretch: Vec3::ONE,
            shape: 0,
        };
        let r = stone.radius() as f64;
        let flow = Vec2::new(2.5, 0.0);
        let at = |x: f64, z: f64| around_stones(&[stone], DVec2::new(x, z), flow, 0.3, 0.3);
        // Faster past its sides, slower in front, piled up in front, white behind.
        assert!(at(0.0, 1.2 * r).velocity.x > 0.3);
        assert!(at(-1.3 * r, 0.0).velocity.x < -0.3);
        assert!(at(-1.1 * r, 0.0).rise > 0.05);
        assert!(at(2.0 * r, 0.0).white > 0.15);
        // Far off, nothing.
        assert_eq!(at(10.0, 0.0), AroundStones::default());
        // Deep under the surface, nothing either.
        let deep = around_stones(&[stone], DVec2::new(0.0, 1.2 * r), flow, 3.0, 2.5);
        assert_eq!(deep, AroundStones::default());
    }

    #[test]
    fn the_meadow_creeks_get_stones() {
        let m = meadow();
        let reach = m.reaches.iter().find(|r| r.order == 1).expect("brook");
        let stones = lay_reach_stones(
            &reach.points,
            &|x, z| m.height(x as f32, z as f32) as f64,
            reach_seed(reach),
        );
        assert!(stones.stones.len() > 20, "{}", stones.stones.len());
        // On the ground the meadow carved.
        for s in &stones.stones {
            assert!((s.at.y - m.height(s.at.x as f32, s.at.z as f32) as f64).abs() < 1e-6);
        }
    }

    #[test]
    fn water_pours_off_the_steps_at_normal_flow_and_skims_in_a_flood() {
        // A bed under every step's water.
        let bed = |x: f64, _: f64| -0.05 * x - 0.4;
        let steep = lay_reach_stones(&straight(200.0, 0.05, 2.0), &bed, 7);
        let gentle = lay_reach_stones(&straight(200.0, 0.005, 2.0), &flat, 7);
        let mut flood = straight(200.0, 0.05, 2.0);
        for p in &mut flood {
            p.flow_m3_s = 12.0;
        }
        let flood = lay_reach_stones(&flood, &bed, 7);
        assert!(steep.pours.len() > 30, "{}", steep.pours.len());
        assert!(gentle.pours.is_empty() && flood.pours.is_empty());
        for p in &steep.pours {
            // Off the lip and down, a drop's worth, landing beyond the lip.
            let (first, last) = (p.path[0], p.path[p.path.len() - 1]);
            assert!((first.pos.y - p.lip.top.y).abs() < 1e-6);
            let fell = (first.pos.y - last.pos.y) as f32;
            assert!((fell - p.lip.drop).abs() < 0.02, "{fell} {}", p.lip.drop);
            assert!(last.pos.x > first.pos.x);
            assert!(p.path.windows(2).all(|w| w[1].pos.y <= w[0].pos.y + 1e-6));
        }
        // Each lip's sheet has two ends.
        let ends = steep.pours.iter().filter(|p| !p.sides[0]).count();
        assert_eq!(ends, steep.pours.iter().filter(|p| !p.sides[1]).count());
    }

    #[test]
    fn the_meadow_steps_pour_down_their_carved_faces() {
        let m = meadow();
        let reach = m.reaches.iter().find(|r| r.order == 1).expect("brook");
        let ground = |x: f64, z: f64| m.height(x as f32, z as f32) as f64;
        let stones = lay_reach_stones(&reach.points, &ground, reach_seed(reach));
        assert!(!stones.pours.is_empty());
        for p in &stones.pours {
            // Never through the bed.
            for f in &p.path {
                assert!(f.pos.y >= ground(f.pos.x, f.pos.z) - 0.02, "{f:?}");
            }
        }
    }
}
