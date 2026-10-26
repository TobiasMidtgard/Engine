//! Fine steps: bodies that must meet one at a time step in short substeps, in regions of their
//! own.
//!
//! The solver settles every contact of a substep together. When a ball of a Newton's cradle
//! swings in, the gaps of the whole row close within one 8 ms substep, so all the balls are
//! solved as one lump and swing off together, where real steel passes the blow down the row
//! and only the last ball flies. Billiard racks, clockwork and chains of colliding carts fail
//! the same way. A body with [`FineSteps`] asks for substeps short enough that each touch is
//! its own (about 1 ms: a ball at 2 m/s moves 2 mm, less than the gap between two balls).
//!
//! Stepping a whole scene that finely would cost several times as much, so such bodies leave
//! for a region of their own: a cluster of movers (by distance and joints, as in
//! [`crate::lod`]) holding a body with fine steps moves out of a region that also holds others.
//! A region and a region with fine steps merge only when their movers come within [`MARGIN`]
//! plus three steps of travel, looked at every step, so a ball thrown at the cradle joins it
//! before it hits; the merged region steps finely until the next split puts the cradle and
//! the ball (while it stays close) on their own again.

use std::collections::HashMap;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

use crate::lod::{Member, find};
use crate::settings::PhysicsSettings;
use crate::sim::{Region, RegionId, Simulation};

/// Ask for physics substeps no longer than `substep_secs` while this body moves, so its
/// contacts are met one at a time ([module docs](self)). For a Newton's cradle, billiard balls
/// and other rows of hard bodies that pass a blow on.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct FineSteps {
    /// The longest substep, seconds.
    pub substep_secs: f32,
}

impl Default for FineSteps {
    /// About a millisecond: 1/960 s, 16 substeps a frame at 60 Hz.
    fn default() -> Self {
        Self {
            substep_secs: 1.0 / 960.0,
        }
    }
}

/// Movers of a region with fine steps and of one without closer than this (plus their travel
/// in three steps) share a region, meters.
pub const MARGIN: f64 = 0.5;

/// The shorter of two substep lengths.
pub(crate) fn finest(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

impl Region {
    /// Could two movers here meet within `dt` seconds? Their colliders' boxes, each grown by
    /// twice its body's travel, are compared pair by pair (a region with more than
    /// [`MEETING_BODIES`] movers always may).
    pub(crate) fn meeting_soon(&self, dt: f32) -> bool {
        let w = &self.world;
        let mut boxes: Vec<(Vec3, Vec3)> = Vec::new();
        for (_, rb) in w.bodies.iter().filter(|(_, rb)| !rb.is_fixed()) {
            if boxes.len() >= MEETING_BODIES {
                return true;
            }
            let grow = Vec3::splat(rb.linvel().length() * dt * 2.0 + 0.001);
            let mut bounds: Option<(Vec3, Vec3)> = None;
            for c in rb.colliders() {
                let Some(co) = w.colliders.get(*c) else {
                    continue;
                };
                let a = co.compute_aabb();
                bounds = Some(match bounds {
                    None => (a.mins, a.maxs),
                    Some((lo, hi)) => (lo.min(a.mins), hi.max(a.maxs)),
                });
            }
            if let Some((lo, hi)) = bounds {
                boxes.push((lo - grow, hi + grow));
            }
        }
        boxes.iter().enumerate().any(|(i, a)| {
            boxes[i + 1..]
                .iter()
                .any(|b| a.0.cmple(b.1).all() && b.0.cmple(a.1).all())
        })
    }
}

/// A region with more movers than this always steps finely when one asks for it.
const MEETING_BODIES: usize = 64;

/// Solver iterations per substep in a region whose bodies ask for `fine` substeps: as many
/// per second of world time as everywhere else (at least one). A region stepping in 1 ms
/// substeps already solves each contact many times a frame, and the joint solver's work
/// grows with every substep (16 substeps of 4 iterations made a 5-ball cradle cost 1 ms).
pub(crate) fn iterations(settings: &PhysicsSettings, fine: Option<f32>, sub_dt: f32) -> usize {
    let full = settings.solver_iterations.clamp(1, 64) as usize;
    match fine {
        Some(_) => {
            let share = sub_dt / settings.max_substep_secs.max(1e-4);
            ((full as f32 * share).round() as usize).clamp(1, full)
        }
        None => full,
    }
}

impl Simulation {
    /// Give every region the shortest substep its bodies ask for. Returns whether a region
    /// came to hold bodies with fine steps and others since the last look.
    pub(crate) fn update_fine(&mut self) -> bool {
        let mut fine: HashMap<RegionId, (f32, u32)> = HashMap::new();
        let mut reach: HashMap<RegionId, f64> = HashMap::new();
        for (e, b) in &self.bodies {
            let r = reach.entry(b.region).or_insert(0.0);
            *r = r.max(b.reach as f64);
            // (A body told to rest is held asleep: it needs no steps, fine or not.)
            if let Some(f) = b.fine.filter(|_| !self.resting.contains(e)) {
                let e = fine.entry(b.region).or_insert((f, 0));
                e.0 = e.0.min(f);
                e.1 += 1;
            }
        }
        let mut newly_mixed = false;
        for (id, r) in self.regions.iter_mut().enumerate() {
            let Some(r) = r else {
                continue;
            };
            r.reach = reach.get(&(id as RegionId)).copied().unwrap_or(0.0);
            let here = fine.get(&(id as RegionId));
            r.fine = here.map(|f| f.0);
            let mixed = here.is_some_and(|f| f.1 < r.movers);
            newly_mixed |= mixed && !r.mixed;
            r.mixed = mixed;
        }
        newly_mixed
    }

    /// Does a mover of region `other` (a body, not a character) come within `margin` meters of
    /// one of region `fine` (a small one)? Looked at every step, so every body of `other` gets a box test first.
    pub(crate) fn movers_near(&self, fine: RegionId, other: RegionId, margin: f64) -> bool {
        let near = self.members(fine);
        let Some((lo, hi)) = near.iter().fold(None, |acc: Option<(DVec3, DVec3)>, m| {
            let r = DVec3::splat(m.reach + margin);
            Some(match acc {
                None => (m.at - r, m.at + r),
                Some((lo, hi)) => (lo.min(m.at - r), hi.max(m.at + r)),
            })
        }) else {
            return false;
        };
        let region = self.region(other);
        let (lo, hi) = (
            lo - DVec3::splat(region.reach),
            hi + DVec3::splat(region.reach),
        );
        let (llo, lhi) = (region.local(lo), region.local(hi));
        let close = |at: DVec3, reach: f64| {
            near.iter()
                .any(|m| m.at.distance(at) < m.reach + reach + margin)
        };
        let bodies = region.world.bodies.iter().filter(|(_, rb)| !rb.is_fixed());
        for (_, rb) in bodies {
            let p = rb.translation();
            if p.cmplt(llo).any() || p.cmpgt(lhi).any() {
                continue;
            }
            // Characters don't bring a fine region back (as they don't hold one in,
            // `split_fine`): a walker among the stations would step the whole yard finely.
            let Some(b) = self.bodies.get(&crate::convert::entity_of(rb.user_data)) else {
                continue;
            };
            if close(region.global(p), b.reach as f64) {
                return true;
            }
        }
        false
    }

    /// Move the clusters holding bodies with fine steps out of regions that hold others too
    /// ([module docs](self)). Two movers less than `gap` meters apart stay together.
    pub(crate) fn split_fine(&mut self, gap: f64) {
        let join = self.settings.region_radius_m * 0.5;
        for id in self.live_region_ids() {
            let (frozen, mixed, tier, fine) = {
                let r = self.region(id);
                (r.frozen, r.mixed, r.tier, r.fine)
            };
            if frozen || !mixed {
                continue;
            }
            // Characters step with the rest and don't chain bodies together: walkers among the
            // stations would hold the whole yard in one cluster, all of it stepping finely.
            let members: Vec<Member> = self
                .members(id)
                .into_iter()
                .filter(|m| m.body.is_some())
                .collect();
            let mut parent = self.cluster(&members, gap);
            // Per cluster: holds a body with fine steps, its members.
            let mut clusters: HashMap<usize, (bool, Vec<usize>)> = HashMap::new();
            for (i, m) in members.iter().enumerate() {
                let root = find(&mut parent, i);
                let c = clusters.entry(root).or_insert((false, Vec::new()));
                c.0 |= m.body.is_some_and(|e| {
                    self.bodies.get(&e).is_some_and(|b| b.fine.is_some())
                        && !self.resting.contains(&e)
                });
                c.1.push(i);
            }
            if clusters.values().all(|c| c.0) {
                continue;
            }
            let mut leaving: Vec<(usize, Vec<usize>)> = clusters
                .into_iter()
                .filter(|(_, (fine, _))| *fine)
                .map(|(root, (_, list))| (root, list))
                .collect();
            leaving.sort_by_key(|(root, _)| *root);
            for (_, list) in leaving {
                let at = list.iter().map(|i| members[*i].at).sum::<DVec3>() / list.len() as f64;
                let bodies: Vec<Entity> = list.iter().filter_map(|i| members[*i].body).collect();
                let target = self
                    .live_regions()
                    .filter(|(r, region)| {
                        *r != id && !region.frozen && region.tier == tier && region.fine.is_some()
                    })
                    .map(|(r, region)| (r, region.anchor.distance(at)))
                    .filter(|(_, d)| *d <= join)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(r, _)| r);
                let target = match target {
                    Some(t) => t,
                    None => {
                        let t = self.create_region(at);
                        // Start it as it will be, so the merge pass this step keeps it apart.
                        let r = self.region_mut(t);
                        r.tier = tier;
                        r.fine = fine;
                        t
                    }
                };
                self.migrate_bodies(&bodies, target);
            }
        }
    }
}
