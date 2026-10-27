//! Level of detail: physics far from every `SimulationFocus` simulates more coarsely.
//!
//! One pixel of a 1080p view with a 60° field of view spans about a thousandth of the distance
//! (15 cm at 150 m, 60 cm at 600 m), so far away the solver's finest work buys nothing anyone
//! sees. Each region gets a tier from how near its nearest mover is to a focus:
//!
//! * **Full** (0): the settings as they are.
//! * **Coarse** (1, past [`PhysicsSettings::coarse_beyond_m`]): substeps twice as long, so
//!   collisions are found half as often (that is most of what tumbling bodies cost; fewer
//!   solver iterations save little more, and kept a far heap from falling asleep).
//! * **Half rate** (2, past [`PhysicsSettings::half_rate_beyond_m`]): one step every other
//!   frame, with substeps four times as long; in between, its bodies are drawn moving on at their
//!   velocity. A region with characters stays coarse (characters move every frame).
//! * **Resting** ([`REST`], past the world's `SimulationRange`, characters or not): no steps at
//!   all, so it costs nothing; its bodies stay where they are until a focus comes back within
//!   the active range. A region with something travelling faster than [`REST_SPEED`] keeps
//!   stepping coarsely instead, so a far plane doesn't hang in the air.
//!
//! A tier is a region's, and most of a scene shares one region, so every [`LOD_EVERY`] steps far
//! movers split off ([`Simulation::split_far`]): a region's movers are clustered by how near
//! they are to each other (and by joints), and a cluster wholly past the next tier's distance,
//! with an awake body in it and no character, moves into a region of that tier. Sleeping
//! clusters stay where they are unless the region keeps stepping for something else (see
//! below). Regions of different tiers merge as soon as their movers come close
//! ([`Simulation::movers_close`]), so a rock thrown in from afar lands on the near ones at full
//! detail.
//!
//! **Dormant regions.** Rapier walks every contact pair of a region each step, asleep or not,
//! so a settled yard of a thousand bodies costs milliseconds a step while a walker or a swing
//! twenty meters away keeps its region stepping. Box2D v3 keeps sleeping islands in solver sets
//! of their own that no step touches, and PhysX and Jolt leave sleeping islands out of the
//! narrow phase; here, every [`LOD_EVERY`] steps, sleeping clusters well apart from everything
//! awake ([`Simulation::split_dormant`]) leave for a dormant region, which sits steps out
//! while all of it sleeps. It merges back as soon as a mover comes close, and stops being
//! dormant when something in it wakes.

use std::collections::{HashMap, HashSet};

use engine_core::ecs::entity::EntityHashMap;
use engine_core::math::DVec3;
use engine_core::prelude::*;

use crate::settings::PhysicsSettings;
use crate::sim::{Region, RegionId, Simulation};

/// Steps between two looks for far movers to split off.
pub const LOD_EVERY: u32 = 30;

/// Steps between two looks at whether regions of different tiers came close (the merge margin
/// covers this many steps of travel).
pub const CLOSE_EVERY: u32 = 4;

/// A region enters a tier past its distance and keeps it down to this fraction of it (so one
/// at the edge doesn't switch back and forth).
const KEEP: f64 = 0.9;

/// Movers split off this much past the next tier's distance, so the region they form starts
/// in it.
const SPLIT: f64 = 1.1;

/// The tier of a region past the world's `SimulationRange`: it doesn't step.
pub const REST: u8 = 3;

/// A region with a body faster than this (m/s) doesn't rest: it is travelling, not settling.
const REST_SPEED: f32 = 3.0;

/// A body this far below the ground's surface (meters) fell through it, and doesn't count as
/// travelling.
const LOST_BELOW_M: f64 = 50.0;

/// The ground is asked for from this high above a body (its height is a hint for caves: the
/// first surface at or below it is returned).
const GROUND_PROBE_M: f64 = 2000.0;

/// A region keeping at least this many sleeping bodies apart from its awake ones moves them to
/// a dormant region (fewer cost Rapier little).
const DORMANT_MIN: usize = 32;

/// Bodies reaching farther than this many grid cells are checked against every other mover
/// instead of through the grid.
const BIG_CELLS: f64 = 4.0;

/// The distance a tier starts at (`None` for full detail or a tier switched off).
fn threshold(settings: &PhysicsSettings, tier: u8) -> Option<f64> {
    match tier {
        1 => settings.coarse_beyond_m,
        2 => settings.coarse_beyond_m.and(settings.half_rate_beyond_m),
        _ => None,
    }
}

/// The tier of a region whose nearest mover is `nearest` meters from a focus. `rest`: the
/// distances past which a region rests and within which a resting one wakes.
pub(crate) fn tier_for(
    nearest: f64,
    current: u8,
    settings: &PhysicsSettings,
    characters: bool,
    rest: Option<(f64, f64)>,
) -> u8 {
    if let Some((enter, wake)) = rest
        && (nearest > enter || (current == REST && nearest > wake))
    {
        return REST;
    }
    let current = current.min(2);
    let past = |tier: u8| {
        threshold(settings, tier).is_some_and(|d| {
            let d = if current >= tier { d * KEEP } else { d };
            nearest > d
        })
    };
    let top = if characters { 1 } else { 2 };
    (1..=top).take_while(|t| past(*t)).last().unwrap_or(0)
}

impl Region {
    /// How this region steps a frame of `dt` seconds: the number of substeps and their length,
    /// or `None` when it sits this frame out (half rate).
    pub(crate) fn plan(&mut self, dt: f32, settings: &PhysicsSettings) -> Option<(u32, f32)> {
        if dt <= 0.0 || self.tier >= REST {
            return None;
        }
        if self.tier >= 2 && self.owed_secs == 0.0 {
            self.owed_secs = dt;
            return None;
        }
        let span = dt + std::mem::take(&mut self.owed_secs);
        // Fine steps only while two bodies here may meet this frame.
        let fine = self.fine.filter(|_| self.meeting_soon(span));
        let base = crate::fine::finest(Some(settings.max_substep_secs), fine).unwrap_or(0.0);
        let longest = base.max(1e-4) * (1u32 << self.tier.min(2)) as f32;
        // (The cap guards the frame rate at extreme speed-ups; fine steps are few bodies, and
        // shorter than the cap would allow at 60 Hz.)
        let cap = settings.max_substeps.max(1) * if fine.is_some() { 8 } else { 1 };
        let n = ((span / longest).ceil() as u32).clamp(1, cap);
        Some((n, span / n as f32))
    }

    /// Distance from the nearest mover here to the nearest focus, meters.
    fn nearest_mover(&self, foci: &[DVec3]) -> f64 {
        self.world
            .bodies
            .iter()
            .filter(|(_, rb)| !rb.is_fixed())
            .map(|(_, rb)| {
                let p = self.global(rb.translation());
                foci.iter()
                    .map(|f| f.distance(p))
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(f64::INFINITY, f64::min)
    }
}

/// A mover as the level of detail sees it.
pub(crate) struct Member {
    /// `None` for a character (they split off only into dormant regions, resting).
    pub body: Option<Entity>,
    /// The character, for one.
    pub character: Option<Entity>,
    pub at: DVec3,
    pub reach: f64,
    /// Awake, or a character not told to rest.
    pub awake: bool,
}

type Cell = (i64, i64, i64);

/// The grid cells a box `at ± half` touches.
fn cells(at: DVec3, half: f64, size: f64) -> impl Iterator<Item = Cell> {
    let lo = ((at - DVec3::splat(half)) / size).floor();
    let hi = ((at + DVec3::splat(half)) / size).floor();
    let (lo, hi) = (lo.as_i64vec3(), hi.as_i64vec3());
    (lo.x..=hi.x)
        .flat_map(move |x| (lo.y..=hi.y).flat_map(move |y| (lo.z..=hi.z).map(move |z| (x, y, z))))
}

pub(crate) fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (a, b) = (find(parent, a), find(parent, b));
    if a != b {
        parent[a.max(b)] = a.min(b);
    }
}

impl Simulation {
    /// The movers of a region: its bodies and characters.
    pub(crate) fn members(&self, id: RegionId) -> Vec<Member> {
        let region = self.region(id);
        let mut out: Vec<Member> = self
            .bodies
            .iter()
            .filter(|(_, b)| b.region == id)
            .filter_map(|(e, b)| {
                let rb = region.world.bodies.get(b.handle)?;
                Some(Member {
                    body: Some(*e),
                    character: None,
                    at: region.global(rb.translation()),
                    reach: b.reach as f64,
                    awake: !rb.is_sleeping(),
                })
            })
            .collect();
        out.extend(
            self.characters
                .iter()
                .filter(|(_, c)| c.region == id)
                .filter_map(|(e, c)| {
                    let rb = region.world.bodies.get(c.body)?;
                    Some(Member {
                        body: None,
                        character: Some(*e),
                        at: region.global(rb.translation()),
                        reach: 1.0,
                        awake: !self.resting.contains(e),
                    })
                }),
        );
        out
    }

    /// Give every region its tier ([module docs](self)).
    pub(crate) fn update_tiers(&mut self, foci: &[DVec3]) {
        let with_characters: HashSet<RegionId> =
            self.characters.values().map(|c| c.region).collect();
        let settings = self.settings.clone();
        let rest = self
            .range
            .map(|r| (r.active_m * (1.0 + r.hysteresis), r.active_m));
        let look = self.lod_clock.is_multiple_of(LOD_EVERY);
        for (id, r) in self.regions.iter_mut().enumerate() {
            let Some(r) = r else {
                continue;
            };
            let tier = if foci.is_empty() || r.movers == 0 {
                0
            } else {
                let characters = with_characters.contains(&(id as RegionId));
                // Something travelling (a plane on its circuit, a car on the track, a shot
                // flying in) keeps going, coarsely; only what is settling rests.
                let w = &r.world;
                let fast = |rb: &&rapier3d::prelude::RigidBody| {
                    rb.linvel().length_squared() > REST_SPEED * REST_SPEED
                };
                let mut moving = w.islands.active_bodies().filter_map(|h| w.bodies.get(h));
                let travelling = moving.any(|rb| fast(&rb));
                // A body far below the ground fell through it (out there its colliders may
                // not be loaded): it isn't going anywhere, so it doesn't keep its region up.
                // (The ground is asked every few steps, and only out there: it isn't cheap.)
                let nearest = r.nearest_mover(foci);
                if look {
                    let out_there = rest.is_some_and(|(enter, _)| nearest > enter * KEEP);
                    let ground = self.ground.as_ref();
                    let lost = |p: DVec3| {
                        ground
                            .and_then(|g| g.height(p + DVec3::Y * GROUND_PROBE_M))
                            .is_some_and(|h| p.y < h - LOST_BELOW_M)
                    };
                    r.lost = out_there
                        && travelling
                        && w.islands
                            .active_bodies()
                            .filter_map(|h| w.bodies.get(h))
                            .filter(fast)
                            .all(|rb| lost(r.global(rb.translation())));
                }
                let rest = rest.filter(|_| !travelling || r.lost);
                tier_for(nearest, r.tier, &settings, characters, rest)
            };
            r.tier = tier;
        }
    }

    /// Does any mover of region `a` come within `margin` meters of one of region `b`? Counts
    /// some pairs up to twice as far apart (it compares grid cells).
    pub(crate) fn movers_close(&self, a: RegionId, b: RegionId, margin: f64) -> bool {
        let (mut ma, mut mb) = (self.members(a), self.members(b));
        if ma.len() < mb.len() {
            std::mem::swap(&mut ma, &mut mb);
        }
        // Only the larger side's movers near the box around the smaller side's can be close (a
        // dormant yard of a thousand bodies beside a few movers keeps a few).
        let Some((lo, hi)) = mb.iter().fold(None, |acc: Option<(DVec3, DVec3)>, m| {
            let r = DVec3::splat(m.reach + margin);
            Some(match acc {
                None => (m.at - r, m.at + r),
                Some((lo, hi)) => (lo.min(m.at - r), hi.max(m.at + r)),
            })
        }) else {
            return false;
        };
        ma.retain(|m| {
            let r = DVec3::splat(m.reach);
            (m.at + r).cmpge(lo).all() && (m.at - r).cmple(hi).all()
        });
        if ma.is_empty() {
            return false;
        }
        let size = margin.max(1.0);
        let big = |m: &Member| m.reach > size * BIG_CELLS;
        let near = |x: &Member, y: &Member| x.at.distance(y.at) < x.reach + y.reach + margin;
        // Big bodies against everything.
        for x in ma.iter().filter(|m| big(m)) {
            if mb.iter().any(|y| near(x, y)) {
                return true;
            }
        }
        for y in mb.iter().filter(|m| big(m)) {
            if ma.iter().any(|x| near(x, y)) {
                return true;
            }
        }
        let grid: HashSet<Cell> = ma
            .iter()
            .filter(|m| !big(m))
            .flat_map(|m| cells(m.at, m.reach, size))
            .collect();
        mb.iter()
            .filter(|m| !big(m))
            .any(|m| cells(m.at, m.reach + margin, size).any(|c| grid.contains(&c)))
    }

    /// Cluster `members`: those whose boxes, grown by half of `gap`, share a grid cell, and
    /// those joined by a joint. Returns union-find parents (roots through [`find`]).
    pub(crate) fn cluster(&self, members: &[Member], gap: f64) -> Vec<usize> {
        let n = members.len();
        let mut parent: Vec<usize> = (0..n).collect();
        let size = gap.max(1.0);
        let big = |m: &Member| m.reach > size * BIG_CELLS;
        let mut grid: HashMap<Cell, usize> = HashMap::new();
        for (i, m) in members.iter().enumerate() {
            if big(m) {
                for (j, o) in members.iter().enumerate() {
                    if j != i && m.at.distance(o.at) < m.reach + o.reach + gap {
                        union(&mut parent, i, j);
                    }
                }
                continue;
            }
            for c in cells(m.at, m.reach + gap * 0.5, size) {
                match grid.get(&c) {
                    Some(&j) => union(&mut parent, i, j),
                    None => {
                        grid.insert(c, i);
                    }
                }
            }
        }
        let index: EntityHashMap<usize> = members
            .iter()
            .enumerate()
            .filter_map(|(i, m)| m.body.map(|e| (e, i)))
            .collect();
        for j in self.joints.values() {
            if let (Some(&a), Some(&b)) = (
                index.get(&j.desc.body_a),
                j.desc.body_b.and_then(|b| index.get(&b)),
            ) {
                union(&mut parent, a, b);
            }
        }
        parent
    }

    /// Move far clusters of movers into regions of a coarser tier ([module docs](self)). Two
    /// movers less than `gap` meters apart (between their colliders' bounds) stay together.
    pub(crate) fn split_far(&mut self, foci: &[DVec3], gap: f64) {
        if foci.is_empty() || self.settings.coarse_beyond_m.is_none() {
            return;
        }
        let join = self.settings.region_radius_m * 0.5;
        for id in self.live_region_ids() {
            let (frozen, movers, tier) = {
                let r = self.region(id);
                (r.frozen, r.movers, r.tier)
            };
            let Some(next) = threshold(&self.settings, tier + 1) else {
                continue;
            };
            if frozen || movers == 0 {
                continue;
            }
            let members = self.members(id);
            let far = |m: &Member| {
                m.body.is_some()
                    && foci
                        .iter()
                        .map(|f| f.distance(m.at))
                        .fold(f64::INFINITY, f64::min)
                        - m.reach
                        > next * SPLIT
            };
            if !members.iter().any(|m| m.awake && far(m)) {
                continue;
            }
            let mut parent = self.cluster(&members, gap);
            // Clusters wholly far, with an awake body in them, leave.
            let mut clusters: HashMap<usize, (bool, bool, Vec<usize>)> = HashMap::new();
            for (i, m) in members.iter().enumerate() {
                let root = find(&mut parent, i);
                let c = clusters.entry(root).or_insert((true, false, Vec::new()));
                c.0 &= far(m);
                c.1 |= m.awake;
                c.2.push(i);
            }
            let mut leaving: Vec<(usize, Vec<usize>)> = clusters
                .into_iter()
                .filter(|(_, (all_far, awake, _))| *all_far && *awake)
                .map(|(root, (_, _, list))| (root, list))
                .collect();
            leaving.sort_by_key(|(root, _)| *root);
            for (_, list) in leaving {
                let at = list.iter().map(|i| members[*i].at).sum::<DVec3>() / list.len() as f64;
                let bodies: Vec<Entity> = list.iter().filter_map(|i| members[*i].body).collect();
                let target = self
                    .live_regions()
                    .filter(|(r, region)| {
                        *r != id && !region.frozen && region.tier > tier && region.fine.is_none()
                    })
                    .map(|(r, region)| (r, region.anchor.distance(at)))
                    .filter(|(_, d)| *d <= join)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(r, _)| r);
                let target = match target {
                    Some(t) => t,
                    None => {
                        let t = self.create_region(at);
                        // Start it at its tier, so the merge pass this step keeps it apart.
                        self.region_mut(t).tier = tier + 1;
                        t
                    }
                };
                self.migrate_bodies(&bodies, target);
            }
        }
    }
    /// A dormant region whose body woke (or gained an awake one), or whose character was let
    /// go, is an ordinary region again.
    pub(crate) fn wake_dormant(&mut self) {
        let walking: HashSet<RegionId> = self
            .characters
            .iter()
            .filter(|(e, _)| !self.resting.contains(*e))
            .map(|(_, c)| c.region)
            .collect();
        for (id, r) in self.regions.iter_mut().enumerate() {
            let Some(r) = r.as_mut().filter(|r| r.dormant) else {
                continue;
            };
            if walking.contains(&(id as RegionId)) {
                r.dormant = false;
                continue;
            }
            // (The flags, not Rapier's active set: bodies just moved in join that as it steps.)
            r.dormant = r
                .world
                .bodies
                .iter()
                .all(|(_, rb)| rb.is_fixed() || rb.is_sleeping());
        }
    }

    /// Move the sleeping clusters of a region that keeps stepping into a dormant region
    /// ([module docs](self)): clusters all asleep (characters in them told to rest), none
    /// asking for fine steps, at least `gap` meters from every awake body and character.
    pub(crate) fn split_dormant(&mut self, gap: f64) {
        let join = self.settings.region_radius_m * 0.5;
        for id in self.live_region_ids() {
            let (frozen, dormant, movers, tier) = {
                let r = self.region(id);
                (r.frozen, r.dormant, r.movers, r.tier)
            };
            if frozen || dormant || (movers as usize) < DORMANT_MIN {
                continue;
            }
            let members = self.members(id);
            let asleep = members.iter().filter(|m| !m.awake).count();
            if asleep < DORMANT_MIN || asleep == members.len() {
                continue;
            }
            let mut parent = self.cluster(&members, gap);
            // Per cluster: whether all of it sleeps (characters count as awake), and its bodies.
            let mut clusters: HashMap<usize, (bool, Vec<usize>)> = HashMap::new();
            for (i, m) in members.iter().enumerate() {
                let root = find(&mut parent, i);
                let c = clusters.entry(root).or_insert((true, Vec::new()));
                let fine = m.body.is_some_and(|e| {
                    !self.resting.contains(&e)
                        && self.bodies.get(&e).is_some_and(|b| b.fine.is_some())
                });
                c.0 &= !m.awake && !fine;
                c.1.push(i);
            }
            let leaving: Vec<usize> = clusters
                .into_values()
                .filter(|(asleep, _)| *asleep)
                .flat_map(|(_, list)| list)
                .collect();
            if leaving.len() < DORMANT_MIN {
                continue;
            }
            let at = leaving.iter().map(|i| members[*i].at).sum::<DVec3>() / leaving.len() as f64;
            let bodies: Vec<Entity> = leaving.iter().filter_map(|i| members[*i].body).collect();
            let characters: Vec<Entity> = leaving
                .iter()
                .filter_map(|i| members[*i].character)
                .collect();
            let target = self
                .live_regions()
                .filter(|(r, region)| *r != id && region.dormant && !region.frozen)
                .map(|(r, region)| (r, region.anchor.distance(at)))
                .filter(|(_, d)| *d <= join)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(r, _)| r);
            let target = target.unwrap_or_else(|| self.create_region(at));
            let r = self.region_mut(target);
            r.dormant = true;
            r.tier = tier;
            self.migrate_bodies(&bodies, target);
            for e in characters {
                self.migrate_character(e, target);
            }
        }
    }
}
