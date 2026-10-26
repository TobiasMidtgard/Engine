//! The NPCs' picture of the static world: obstacles taken from static `Collider`s (the physics
//! contract, so no physics crate is needed), cover points along them, line of sight and the
//! height of walkable surfaces (platforms, ramps, crates).
//!
//! Line of sight uses the installed physics backend's `PhysicsQueries` when there is one and
//! this module's own analytic obstacles otherwise, plus a coarse check against the `Ground`
//! query so hills block sight too. The model rebuilds itself when static colliders appear,
//! change, move or disappear, and when terrain is edited.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_world::physics::{
    BodyKind, CharacterController, Collider, ColliderShape, PhysicsBackend, PhysicsQueries,
    QueryFilter, RigidBody,
};
use engine_world::{Ground, Obscurance, TerrainChanged, WorldShift};

/// Spatial hash cell size, meters.
const HASH_CELL: f64 = 8.0;
/// Obstacles wider than this go in a list that every query checks instead of the hash.
const BIG_OBSTACLE: f64 = 256.0;
/// How far an obstacle's cover reaches past its bounds: points stand up to 0.6 m out and are
/// checked against obstacles within a body's radius of them. A change refreshes the cover of
/// every obstacle this close to it.
const COVER_REACH: f64 = 2.0;
/// Regional changes kept for navigation, which drops only the chunks they touch.
const CHANGE_LOG: usize = 64;
/// Grid spacing of the cached ground heights used by line-of-sight checks, meters.
const GROUND_CELL: f64 = 0.5;

/// Shape of an obstacle in its local frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ObstacleShape {
    Box {
        half: Vec3,
    },
    /// Along local Y.
    Cylinder {
        radius: f32,
        half_height: f32,
    },
    Sphere {
        radius: f32,
    },
}

/// One static obstacle (a wall, a crate, a pillar, a platform).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Obstacle {
    /// The collider entity it came from.
    pub entity: Entity,
    pub center: DVec3,
    pub rotation: Quat,
    pub shape: ObstacleShape,
    /// World bounds.
    pub min: DVec3,
    pub max: DVec3,
}

impl Obstacle {
    /// An obstacle at `center` with `rotation` (world bounds are computed).
    pub fn new(entity: Entity, center: DVec3, rotation: Quat, shape: ObstacleShape) -> Self {
        let ext = match shape {
            ObstacleShape::Box { half } => {
                let m = Mat3::from_quat(rotation);
                Vec3::new(
                    m.x_axis.x.abs() * half.x
                        + m.y_axis.x.abs() * half.y
                        + m.z_axis.x.abs() * half.z,
                    m.x_axis.y.abs() * half.x
                        + m.y_axis.y.abs() * half.y
                        + m.z_axis.y.abs() * half.z,
                    m.x_axis.z.abs() * half.x
                        + m.y_axis.z.abs() * half.y
                        + m.z_axis.z.abs() * half.z,
                )
            }
            ObstacleShape::Cylinder {
                radius,
                half_height,
            } => {
                let axis = rotation * Vec3::Y;
                let r = |a: f32| radius * (1.0 - a * a).max(0.0).sqrt();
                Vec3::new(
                    axis.x.abs() * half_height + r(axis.x),
                    axis.y.abs() * half_height + r(axis.y),
                    axis.z.abs() * half_height + r(axis.z),
                )
            }
            ObstacleShape::Sphere { radius } => Vec3::splat(radius),
        };
        Self {
            entity,
            center,
            rotation,
            shape,
            min: center - ext.as_dvec3(),
            max: center + ext.as_dvec3(),
        }
    }

    /// Distance along `dir` (unit) from `origin` to the surface, if hit within `max`. A ray that
    /// starts inside the obstacle doesn't hit it (an eye pressed into a wall still sees out).
    pub fn ray(&self, origin: DVec3, dir: Vec3, max: f32) -> Option<f32> {
        let inv = self.rotation.inverse();
        let o = inv * (origin - self.center).as_vec3();
        let d = inv * dir;
        let t = match self.shape {
            ObstacleShape::Box { half } => ray_box(o, d, half),
            ObstacleShape::Cylinder {
                radius,
                half_height,
            } => ray_cylinder(o, d, radius, half_height),
            ObstacleShape::Sphere { radius } => ray_sphere(o, d, radius),
        }?;
        (t <= max).then_some(t)
    }

    /// Is the world point inside the obstacle grown by `inflate` meters (horizontally only)?
    pub fn contains(&self, p: DVec3, inflate: f32) -> bool {
        let l = self.rotation.inverse() * (p - self.center).as_vec3();
        match self.shape {
            ObstacleShape::Box { half } => {
                l.x.abs() <= half.x + inflate
                    && l.y.abs() <= half.y
                    && l.z.abs() <= half.z + inflate
            }
            ObstacleShape::Cylinder {
                radius,
                half_height,
            } => Vec2::new(l.x, l.z).length() <= radius + inflate && l.y.abs() <= half_height,
            ObstacleShape::Sphere { radius } => l.length() <= radius + inflate,
        }
    }

    /// Top and bottom of the obstacle along the vertical line at `x, z`, if it covers it.
    pub fn vertical_span(&self, x: f64, z: f64) -> Option<(f64, f64)> {
        if x < self.min.x || x > self.max.x || z < self.min.z || z > self.max.z {
            return None;
        }
        let above = DVec3::new(x, self.max.y + 1.0, z);
        let len = (self.max.y - self.min.y) as f32 + 2.0;
        let top = above.y - self.ray(above, Vec3::NEG_Y, len)? as f64;
        let below = DVec3::new(x, self.min.y - 1.0, z);
        let bottom = below.y + self.ray(below, Vec3::Y, len).unwrap_or(1.0) as f64;
        Some((top, bottom))
    }

    /// Horizontal distance from `p` to the obstacle's footprint (0 inside), and the outward
    /// direction (XZ, unit) from the footprint toward `p`.
    pub fn footprint_distance(&self, p: DVec3) -> (f32, Vec3) {
        let l = self.rotation.inverse() * (p - self.center).as_vec3();
        match self.shape {
            ObstacleShape::Box { half } => {
                let q = Vec3::new(l.x.clamp(-half.x, half.x), 0.0, l.z.clamp(-half.z, half.z));
                let mut d = Vec3::new(l.x - q.x, 0.0, l.z - q.z);
                let dist = d.length();
                if dist < 1e-4 {
                    // Inside: push out through the nearest face.
                    let px = half.x - l.x.abs();
                    let pz = half.z - l.z.abs();
                    d = if px < pz {
                        Vec3::new(l.x.signum(), 0.0, 0.0)
                    } else {
                        Vec3::new(0.0, 0.0, l.z.signum())
                    };
                    let world = flatten(self.rotation * d);
                    return (-(px.min(pz)), world);
                }
                (dist, flatten(self.rotation * (d / dist)))
            }
            ObstacleShape::Cylinder { radius, .. } | ObstacleShape::Sphere { radius } => {
                let flat = Vec3::new(
                    (p.x - self.center.x) as f32,
                    0.0,
                    (p.z - self.center.z) as f32,
                );
                let len = flat.length();
                let dir = if len > 1e-4 { flat / len } else { Vec3::X };
                (len - radius, dir)
            }
        }
    }

    /// Is the obstacle standing upright (local Y close to world Y)?
    pub fn upright(&self) -> bool {
        (self.rotation * Vec3::Y).y > 0.95
    }
}

fn flatten(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z).normalize_or_zero()
}

fn ray_box(o: Vec3, d: Vec3, half: Vec3) -> Option<f32> {
    let mut t0 = f32::NEG_INFINITY;
    let mut t1 = f32::INFINITY;
    for i in 0..3 {
        if d[i].abs() < 1e-8 {
            if o[i].abs() > half[i] {
                return None;
            }
        } else {
            let inv = 1.0 / d[i];
            let a = (-half[i] - o[i]) * inv;
            let b = (half[i] - o[i]) * inv;
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
    }
    (t1 >= t0 && t0 >= 0.0).then_some(t0)
}

fn ray_sphere(o: Vec3, d: Vec3, r: f32) -> Option<f32> {
    let b = o.dot(d);
    let c = o.length_squared() - r * r;
    if c <= 0.0 {
        return None;
    }
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let t = -b - disc.sqrt();
    (t >= 0.0).then_some(t)
}

fn ray_cylinder(o: Vec3, d: Vec3, r: f32, h: f32) -> Option<f32> {
    let inside = Vec2::new(o.x, o.z).length_squared() <= r * r && o.y.abs() <= h;
    if inside {
        return None;
    }
    let mut best = f32::INFINITY;
    let a = d.x * d.x + d.z * d.z;
    if a > 1e-10 {
        let b = 2.0 * (o.x * d.x + o.z * d.z);
        let c = o.x * o.x + o.z * o.z - r * r;
        let disc = b * b - 4.0 * a * c;
        if disc >= 0.0 {
            let t = (-b - disc.sqrt()) / (2.0 * a);
            if t >= 0.0 && (o.y + d.y * t).abs() <= h {
                best = t;
            }
        }
    }
    if d.y.abs() > 1e-8 {
        for cap in [-h, h] {
            let t = (cap - o.y) / d.y;
            if t >= 0.0 && t < best {
                let p = o + d * t;
                if p.x * p.x + p.z * p.z <= r * r {
                    best = t;
                }
            }
        }
    }
    best.is_finite().then_some(best)
}

/// How tall a cover point's obstacle is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoverHeight {
    /// Waist-high (0.85–1.5 m): crouch behind, stand to shoot over.
    Low,
    /// Taller than a person: hide behind, lean out at the edges.
    High,
}

/// A spot next to an obstacle where an NPC can hide from threats on the obstacle's far side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoverPoint {
    /// Where the NPC stands (on the walkable surface).
    pub position: DVec3,
    /// Outward normal of the obstacle face (XZ): threats in the opposite direction are covered.
    pub normal: Vec3,
    pub height: CoverHeight,
    /// At the end of a tall face: the NPC can lean out sideways to shoot.
    pub edge: bool,
    /// Index into [`TacticalWorld::obstacles`].
    pub obstacle: u32,
}

/// Static obstacles, cover points and spatial lookups. Rebuilt automatically.
#[derive(Resource, Debug, Default)]
pub struct TacticalWorld {
    pub obstacles: Vec<Obstacle>,
    pub cover: Vec<CoverPoint>,
    hash: HashMap<(i64, i64), Vec<u32>>,
    big: Vec<u32>,
    cover_hash: HashMap<(i64, i64), Vec<u32>>,
    /// Bumped on every rebuild, update and world shift (navigation caches compare it).
    pub generation: u64,
    fingerprint: u64,
    /// Rebuild from the colliders on the next step even if they look unchanged.
    rebuild: bool,
    /// Fingerprint of each collider entity's obstacles, to tell which ones changed.
    per_entity: HashMap<Entity, u64>,
    /// What each recent generation changed: the XZ box it touched, or `None` for everything.
    changes: VecDeque<(u64, Option<(DVec2, DVec2)>)>,
    /// Bumped by every world shift, so a build started in an older frame is thrown away.
    epoch: u64,
    /// Ground heights on a [`GROUND_CELL`] grid for the coarse terrain check in line of sight:
    /// a fight asks thousands per step, and a planet's ground is slow to ask. Emptied when
    /// the ground or the frame changes.
    ground_heights: Mutex<HashMap<(i64, i64, i64), Option<f64>>>,
}

fn hash_key(x: f64, z: f64) -> (i64, i64) {
    (
        (x / HASH_CELL).floor() as i64,
        (z / HASH_CELL).floor() as i64,
    )
}

impl TacticalWorld {
    /// Build from a list of obstacles (used by the rebuild system and by tests).
    pub fn from_obstacles(obstacles: Vec<Obstacle>, ground: &Ground) -> Self {
        let mut world = TacticalWorld {
            obstacles,
            ..Default::default()
        };
        world.index();
        world.generate_cover(ground);
        world.per_entity = entity_fingerprints(&world.obstacles);
        world.fingerprint = fingerprint(&world.obstacles);
        world
    }

    /// Replace the obstacles of the `replaced` entities with `fresh` (all of their obstacles
    /// now; an entity with none is gone) and refresh cover only near what changed and inside
    /// `regions` (XZ boxes where the ground changed). Much cheaper than a rebuild in a big
    /// world, and the result is the same apart from the order of cover points.
    pub fn update(
        &mut self,
        replaced: &HashSet<Entity>,
        fresh: Vec<Obstacle>,
        regions: &[(DVec2, DVec2)],
        ground: &Ground,
    ) {
        if !regions.is_empty() {
            self.forget_ground();
        }
        let mut dirty: Vec<(DVec2, DVec2)> = regions.to_vec();
        let footprint = |o: &Obstacle| (DVec2::new(o.min.x, o.min.z), DVec2::new(o.max.x, o.max.z));
        let mut map = vec![u32::MAX; self.obstacles.len()];
        let mut kept = Vec::with_capacity(self.obstacles.len() + fresh.len());
        for (i, o) in self.obstacles.drain(..).enumerate() {
            if replaced.contains(&o.entity) {
                dirty.push(footprint(&o));
            } else {
                map[i] = kept.len() as u32;
                kept.push(o);
            }
        }
        dirty.extend(fresh.iter().map(footprint));
        for e in replaced {
            self.per_entity.remove(e);
        }
        self.per_entity.extend(entity_fingerprints(&fresh));
        kept.extend(fresh);
        self.obstacles = kept;
        self.index();

        // Cover: keep the points of obstacles far from every change, make the rest afresh.
        let reach = DVec2::splat(COVER_REACH);
        let dirty: Vec<(DVec2, DVec2)> = dirty
            .into_iter()
            .map(|(a, b)| (a - reach, b + reach))
            .collect();
        let mut affected = vec![false; self.obstacles.len()];
        let mut near = Vec::new();
        for &(min, max) in &dirty {
            self.obstacles_near(min, max, &mut near);
            for &i in &near {
                let o = &self.obstacles[i as usize];
                if o.max.x >= min.x && o.min.x <= max.x && o.max.z >= min.y && o.min.z <= max.y {
                    affected[i as usize] = true;
                }
            }
        }
        let mut cover = std::mem::take(&mut self.cover);
        cover.retain_mut(|c| {
            let Some(&i) = map.get(c.obstacle as usize).filter(|&&i| i != u32::MAX) else {
                return false;
            };
            c.obstacle = i;
            !affected[i as usize]
        });
        let mut points = Vec::new();
        for (oi, hit) in affected.iter().enumerate() {
            if *hit {
                self.cover_for(oi, ground, &mut points, &mut cover);
            }
        }
        self.cover = cover;
        self.index_cover();
        self.fingerprint = fingerprint(&self.obstacles);
        self.generation += 1;
        let touched = dirty
            .into_iter()
            .reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)));
        self.log_change(Some(touched.unwrap_or_default()));
    }

    /// The ground's height at `p` (see [`GroundQuery::sample`](engine_world::GroundQuery)),
    /// taken at the nearest point of a half-metre grid and remembered. For coarse checks.
    pub fn cached_ground_height(&self, ground: &Ground, p: DVec3) -> Option<f64> {
        let key = (
            (p.x / GROUND_CELL).round() as i64,
            (p.z / GROUND_CELL).round() as i64,
            (p.y / 32.0).floor() as i64,
        );
        let Ok(mut cache) = self.ground_heights.lock() else {
            return ground.height(p);
        };
        if let Some(&h) = cache.get(&key) {
            return h;
        }
        if cache.len() > 1 << 20 {
            cache.clear();
        }
        let at = DVec3::new(
            key.0 as f64 * GROUND_CELL,
            key.2 as f64 * 32.0 + 16.0,
            key.1 as f64 * GROUND_CELL,
        );
        let h = ground.height(at);
        cache.insert(key, h);
        h
    }

    /// Forget the cached ground heights (the ground changed).
    pub fn forget_ground(&mut self) {
        if let Ok(cache) = self.ground_heights.get_mut() {
            cache.clear();
        }
    }

    fn log_change(&mut self, region: Option<(DVec2, DVec2)>) {
        if self.changes.len() == CHANGE_LOG {
            self.changes.pop_front();
        }
        self.changes.push_back((self.generation, region));
    }

    /// The XZ boxes changed since `generation`, or `None` if that isn't known (a rebuild, a
    /// world shift, or too long ago): then everything may have changed.
    pub fn changes_since(
        &self,
        generation: u64,
    ) -> Option<impl Iterator<Item = (DVec2, DVec2)> + '_> {
        let first = self.changes.front().map_or(self.generation + 1, |c| c.0);
        if generation > self.generation || (generation < self.generation && first > generation + 1)
        {
            return None;
        }
        let recent = self.changes.iter().filter(move |c| c.0 > generation);
        if recent.clone().any(|c| c.1.is_none()) {
            return None;
        }
        Some(recent.filter_map(|c| c.1))
    }

    /// Move the model into a new reference frame (see `WorldShift`): obstacles, cover points
    /// and lookups move with the world, and the generation changes so navigation drops its
    /// chunks. A move is exact; a turned world is also rebuilt from the colliders on the next
    /// step, since cover and walkable tops depend on which way is up.
    pub(crate) fn shift(&mut self, s: &WorldShift) {
        if self.generation == 0 {
            self.epoch += 1; // not built yet: the first build happens in the new frame
            return;
        }
        for o in &mut self.obstacles {
            *o = Obstacle::new(
                o.entity,
                s.point(o.center),
                s.orientation(o.rotation),
                o.shape,
            );
        }
        for c in &mut self.cover {
            c.position = s.point(c.position);
            c.normal = flatten(s.direction(c.normal));
        }
        self.index();
        self.index_cover();
        self.generation += 1;
        self.epoch += 1;
        self.log_change(None);
        self.forget_ground();
        self.fingerprint = fingerprint(&self.obstacles);
        self.rebuild |= !s.is_translation_only();
    }

    fn index(&mut self) {
        self.hash.clear();
        self.big.clear();
        for (i, o) in self.obstacles.iter().enumerate() {
            if o.max.x - o.min.x > BIG_OBSTACLE || o.max.z - o.min.z > BIG_OBSTACLE {
                self.big.push(i as u32);
                continue;
            }
            let (x0, z0) = hash_key(o.min.x, o.min.z);
            let (x1, z1) = hash_key(o.max.x, o.max.z);
            for x in x0..=x1 {
                for z in z0..=z1 {
                    self.hash.entry((x, z)).or_default().push(i as u32);
                }
            }
        }
    }

    /// Obstacles whose bounds may touch the XZ box `[min, max]` (duplicates removed).
    pub fn obstacles_near(&self, min: DVec2, max: DVec2, out: &mut Vec<u32>) {
        out.clear();
        out.extend_from_slice(&self.big);
        let (x0, z0) = hash_key(min.x, min.y);
        let (x1, z1) = hash_key(max.x, max.y);
        for x in x0..=x1 {
            for z in z0..=z1 {
                if let Some(list) = self.hash.get(&(x, z)) {
                    for &i in list {
                        let o = &self.obstacles[i as usize];
                        if o.max.x >= min.x
                            && o.min.x <= max.x
                            && o.max.z >= min.y
                            && o.min.z <= max.y
                            && !out.contains(&i)
                        {
                            out.push(i);
                        }
                    }
                }
            }
        }
    }

    /// First obstacle hit along the segment `from → to`: (distance, obstacle index).
    pub fn raycast(&self, from: DVec3, to: DVec3) -> Option<(f32, u32)> {
        let delta = to - from;
        let len = delta.length();
        if len < 1e-6 {
            return None;
        }
        let dir = (delta / len).as_vec3();
        let mut best: Option<(f32, u32)> = None;
        let test = |i: u32, best: &mut Option<(f32, u32)>| {
            let limit = best.map(|b| b.0).unwrap_or(len as f32);
            if let Some(t) = self.obstacles[i as usize].ray(from, dir, limit)
                && best.is_none_or(|b| t < b.0)
            {
                *best = Some((t, i));
            }
        };
        for &i in &self.big {
            test(i, &mut best);
        }
        // Walk the hash cells the segment crosses (2D DDA).
        let (mut cx, mut cz) = hash_key(from.x, from.z);
        let (ex, ez) = hash_key(to.x, to.z);
        let step_x = if delta.x > 0.0 { 1 } else { -1 };
        let step_z = if delta.z > 0.0 { 1 } else { -1 };
        let next_boundary =
            |c: i64, step: i64| (c + if step > 0 { 1 } else { 0 }) as f64 * HASH_CELL;
        let mut t_max_x = if delta.x.abs() < 1e-12 {
            f64::INFINITY
        } else {
            (next_boundary(cx, step_x) - from.x) / delta.x
        };
        let mut t_max_z = if delta.z.abs() < 1e-12 {
            f64::INFINITY
        } else {
            (next_boundary(cz, step_z) - from.z) / delta.z
        };
        let t_dx = if delta.x.abs() < 1e-12 {
            f64::INFINITY
        } else {
            HASH_CELL / delta.x.abs()
        };
        let t_dz = if delta.z.abs() < 1e-12 {
            f64::INFINITY
        } else {
            HASH_CELL / delta.z.abs()
        };
        let mut visited: [u32; 64] = [u32::MAX; 64];
        let mut n_visited = 0usize;
        for _ in 0..4096 {
            if let Some(list) = self.hash.get(&(cx, cz)) {
                for &i in list {
                    if visited[..n_visited.min(64)].contains(&i) {
                        continue;
                    }
                    if n_visited < 64 {
                        visited[n_visited] = i;
                    }
                    n_visited += 1;
                    test(i, &mut best);
                }
            }
            if (cx, cz) == (ex, ez) {
                break;
            }
            // Stop early once a hit is closer than the next cell.
            let t_next = t_max_x.min(t_max_z);
            if let Some((t, _)) = best
                && (t as f64) < t_next * len
            {
                break;
            }
            if t_max_x < t_max_z {
                t_max_x += t_dx;
                cx += step_x;
            } else {
                t_max_z += t_dz;
                cz += step_z;
            }
        }
        best
    }

    /// Height of the surface someone standing near height `feet` at `x, z` would stand on:
    /// the ground, or the top of a crate, platform or ramp under them. Obstacles more than
    /// `climb` meters above `feet` and obstacles entirely above head height are ignored.
    pub fn surface_height(&self, p: DVec3, feet: f64, climb: f32, ground: &Ground) -> f64 {
        let g = ground
            .height(DVec3::new(p.x, feet + climb as f64 + 0.01, p.z))
            .unwrap_or(feet);
        let mut h = g;
        let mut near = Vec::new();
        self.obstacles_near(DVec2::new(p.x, p.z), DVec2::new(p.x, p.z), &mut near);
        for &i in &near {
            if let Some((top, bottom)) = self.obstacles[i as usize].vertical_span(p.x, p.z)
                && top > h
                && top <= feet + climb as f64 + 0.01
                && bottom <= h + 1.0
            {
                h = top;
            }
        }
        h
    }

    /// Height of the highest surface at `x, z` reachable from the ground by stacking obstacles
    /// that block a body of `body_height` (used by navigation: walls make tall cells that
    /// can't be stepped onto, a platform makes a raised cell reachable by its ramp).
    pub fn stacked_height(&self, x: f64, z: f64, body_height: f32, ground: &Ground) -> (f64, f64) {
        let g = ground.height(DVec3::new(x, 1.0e6, z)).unwrap_or(0.0);
        let mut h = g;
        let mut near = Vec::new();
        self.obstacles_near(DVec2::new(x, z), DVec2::new(x, z), &mut near);
        for _ in 0..4 {
            let mut raised = false;
            for &i in &near {
                if let Some((top, bottom)) = self.obstacles[i as usize].vertical_span(x, z)
                    && top > h + 0.01
                    && bottom < h + body_height as f64
                {
                    h = top;
                    raised = true;
                }
            }
            if !raised {
                break;
            }
        }
        (h, g)
    }

    fn generate_cover(&mut self, ground: &Ground) {
        let mut cover = Vec::new();
        let mut points = Vec::new();
        for oi in 0..self.obstacles.len() {
            self.cover_for(oi, ground, &mut points, &mut cover);
        }
        self.cover = cover;
        self.index_cover();
    }

    /// Add the cover points along obstacle `oi` to `out` (`points` is scratch).
    fn cover_for(
        &self,
        oi: usize,
        ground: &Ground,
        points: &mut Vec<(DVec3, Vec3, bool, u32)>,
        out: &mut Vec<CoverPoint>,
    ) {
        const AGENT: f32 = 0.45;
        const SPACING: f32 = 1.3;
        let o = &self.obstacles[oi];
        if !o.upright() || o.max.x - o.min.x > BIG_OBSTACLE {
            return;
        }
        let base = ground
            .height(DVec3::new(o.center.x, o.max.y, o.center.z))
            .unwrap_or(0.0);
        if o.min.y > base + 0.6 {
            return; // floating: a roof or a bridge, not cover
        }
        let tall = (o.max.y - base) as f32;
        if tall < 0.85 {
            return;
        }
        let height = if tall < 1.5 {
            CoverHeight::Low
        } else {
            CoverHeight::High
        };
        match o.shape {
            ObstacleShape::Box { half } => {
                for (axis, half_n, half_t) in [
                    (Vec3::X, half.x, half.z),
                    (Vec3::NEG_X, half.x, half.z),
                    (Vec3::Z, half.z, half.x),
                    (Vec3::NEG_Z, half.z, half.x),
                ] {
                    let n = flatten(o.rotation * axis);
                    let t = Vec3::new(-n.z, 0.0, n.x);
                    let count = ((2.0 * half_t) / SPACING).floor().max(1.0) as i32;
                    for k in 0..count {
                        let s = if count == 1 {
                            0.0
                        } else {
                            -half_t + 0.35 + (2.0 * half_t - 0.7) * k as f32 / (count - 1) as f32
                        };
                        let edge = count > 1 && (k == 0 || k == count - 1);
                        let p = o.center + (n * (half_n + AGENT + 0.15) + t * s).as_dvec3();
                        points.push((p, n, edge, oi as u32));
                    }
                }
            }
            ObstacleShape::Cylinder { radius, .. } | ObstacleShape::Sphere { radius } => {
                let count = ((TAU * radius) / SPACING).ceil().clamp(4.0, 16.0) as i32;
                for k in 0..count {
                    let a = k as f32 / count as f32 * TAU;
                    let n = Vec3::new(a.cos(), 0.0, a.sin());
                    let p = o.center + (n * (radius + AGENT + 0.15)).as_dvec3();
                    points.push((p, n, height == CoverHeight::High, oi as u32));
                }
            }
        }
        for (p, n, edge, oi) in points.drain(..) {
            let feet = self.surface_height(p, base, 0.4, ground);
            let at = DVec3::new(p.x, feet, p.z);
            if self.blocked_at(at, AGENT, 1.6, Some(oi)) {
                continue;
            }
            out.push(CoverPoint {
                position: at,
                normal: n,
                height,
                edge,
                obstacle: oi,
            });
        }
    }

    fn index_cover(&mut self) {
        self.cover_hash.clear();
        for (i, c) in self.cover.iter().enumerate() {
            self.cover_hash
                .entry(hash_key(c.position.x, c.position.z))
                .or_default()
                .push(i as u32);
        }
    }

    /// Would a body of `radius` and `height` standing at `p` overlap an obstacle (other than
    /// `except`)? Looks through the hash cells in place, so it allocates nothing (animals
    /// steering their flight call it many times a tick).
    pub fn blocked_at(&self, p: DVec3, radius: f32, height: f32, except: Option<u32>) -> bool {
        let r = radius as f64;
        let hit = |i: u32| {
            if Some(i) == except {
                return false;
            }
            let o = &self.obstacles[i as usize];
            if o.max.y < p.y + 0.3 || o.min.y > p.y + height as f64 {
                return false;
            }
            o.footprint_distance(p).0 < radius && {
                // Standing on top of it is fine.
                o.vertical_span(p.x, p.z)
                    .is_none_or(|(top, _)| top > p.y + 0.3)
            }
        };
        if self.big.iter().any(|&i| hit(i)) {
            return true;
        }
        let (min, max) = (DVec2::new(p.x - r, p.z - r), DVec2::new(p.x + r, p.z + r));
        let (x0, z0) = hash_key(min.x, min.y);
        let (x1, z1) = hash_key(max.x, max.y);
        (x0..=x1).any(|x| {
            (z0..=z1).any(|z| {
                self.hash.get(&(x, z)).is_some_and(|list| {
                    list.iter().any(|&i| {
                        let o = &self.obstacles[i as usize];
                        o.max.x >= min.x
                            && o.min.x <= max.x
                            && o.max.z >= min.y
                            && o.min.z <= max.y
                            && hit(i)
                    })
                })
            })
        })
    }

    /// Cover points within `radius` of `center`, appended to `out` as indices.
    pub fn cover_near(&self, center: DVec3, radius: f64, out: &mut Vec<u32>) {
        let (x0, z0) = hash_key(center.x - radius, center.z - radius);
        let (x1, z1) = hash_key(center.x + radius, center.z + radius);
        for x in x0..=x1 {
            for z in z0..=z1 {
                if let Some(list) = self.cover_hash.get(&(x, z)) {
                    for &i in list {
                        if self.cover[i as usize].position.distance_squared(center)
                            <= radius * radius
                        {
                            out.push(i);
                        }
                    }
                }
            }
        }
    }

    /// Push a body of `radius` at `p` (feet height `p.y`) out of obstacles it overlaps, in XZ.
    /// Returns the corrected position.
    pub fn resolve_overlap(&self, mut p: DVec3, radius: f32, height: f32, climb: f32) -> DVec3 {
        let mut near = Vec::new();
        let r = radius as f64 + 0.5;
        self.obstacles_near(
            DVec2::new(p.x - r, p.z - r),
            DVec2::new(p.x + r, p.z + r),
            &mut near,
        );
        for _ in 0..3 {
            let mut moved = false;
            for &i in &near {
                let o = &self.obstacles[i as usize];
                if o.max.y <= p.y + climb as f64 + 0.01 || o.min.y >= p.y + height as f64 {
                    continue;
                }
                if let Some((top, _)) = o.vertical_span(p.x, p.z)
                    && top <= p.y + climb as f64 + 0.01
                {
                    continue; // we can step onto it
                }
                let (d, n) = o.footprint_distance(p);
                if d < radius {
                    p += (n * (radius - d + 0.001)).as_dvec3();
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        p
    }
}

/// Line-of-sight checks that use the physics backend when one is installed and the NPC
/// module's own obstacles otherwise. Both also check the ground (hills hide things).
pub struct Sight<'a> {
    pub world: &'a TacticalWorld,
    pub physics: Option<&'a PhysicsQueries>,
    pub ground: &'a Ground,
    /// Smoke, fog and dust (`None` = clear air).
    pub obscurance: Option<&'a Obscurance>,
}

/// Below this much light getting through, smoke hides a body completely.
pub const OPAQUE_SMOKE: f32 = 0.08;

impl Sight<'_> {
    /// How well someone at `from` can see `to`: 0 when something solid is in the way,
    /// otherwise the fraction of light that gets through smoke, fog and dust (1 = clear).
    pub fn visibility(&self, from: DVec3, to: DVec3, ignore: &[Entity]) -> f32 {
        if !self.clear(from, to, ignore) {
            return 0.0;
        }
        self.haze(from, to)
    }

    /// Fraction of light that gets from `from` to `to` through smoke, fog and dust (solid
    /// obstacles ignored).
    pub fn haze(&self, from: DVec3, to: DVec3) -> f32 {
        self.obscurance
            .map(|o| o.transmittance(from, to))
            .unwrap_or(1.0)
    }

    /// Can someone at `from` see `to`? `ignore` lists the looker and the target (their own
    /// colliders shouldn't block).
    pub fn clear(&self, from: DVec3, to: DVec3, ignore: &[Entity]) -> bool {
        let len = from.distance(to);
        if len < 1e-3 {
            return true;
        }
        let dir = ((to - from) / len).as_vec3();
        let blocked = match self.physics {
            Some(q) => {
                let filter = QueryFilter {
                    exclude: ignore.to_vec(),
                    ..Default::default()
                };
                q.raycast(from, dir, (len - 0.05).max(0.0) as f32, &filter)
                    .is_some()
            }
            None => self
                .world
                .raycast(from, from + (to - from) * ((len - 0.05).max(0.0) / len))
                .is_some_and(|(_, i)| !ignore.contains(&self.world.obstacles[i as usize].entity)),
        };
        if blocked {
            return false;
        }
        // Coarse terrain check.
        if len > 6.0 {
            let steps = ((len / 3.0) as usize).clamp(2, 48);
            for k in 1..steps {
                let p = from.lerp(to, k as f64 / steps as f64);
                if let Some(h) = self
                    .world
                    .cached_ground_height(self.ground, DVec3::new(p.x, p.y + 50.0, p.z))
                    && h > p.y + 0.05
                {
                    return false;
                }
            }
        }
        true
    }
}

/// Settings for the world model.
#[derive(Resource, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldModelSettings {
    /// Seconds between checks for moved static colliders.
    pub poll_secs: f32,
    /// A rebuild of more obstacles than this runs on the async task pool, and the old model
    /// stays in use until it is ready, so a big world never stalls a step.
    pub async_above: usize,
}

impl Default for WorldModelSettings {
    fn default() -> Self {
        Self {
            poll_secs: 1.0,
            async_above: 1500,
        }
    }
}

type ColliderItem<'a> = (
    Entity,
    &'a Collider,
    &'a Transform,
    &'a GlobalTransform,
    Option<&'a RigidBody>,
    Option<&'a ChildOf>,
    Has<CharacterController>,
);

/// Collect obstacles from static colliders.
pub(crate) fn collect_obstacles(
    colliders: &Query<ColliderItem>,
    bodies: &Query<&RigidBody>,
) -> Vec<Obstacle> {
    let mut out = Vec::new();
    for item in colliders.iter() {
        push_collider(&mut out, item, bodies);
    }
    out
}

/// The obstacles of one static collider (none for sensors, characters, moving bodies and
/// terrain-sized shapes).
fn push_collider(
    out: &mut Vec<Obstacle>,
    item: (
        Entity,
        &Collider,
        &Transform,
        &GlobalTransform,
        Option<&RigidBody>,
        Option<&ChildOf>,
        bool,
    ),
    bodies: &Query<&RigidBody>,
) {
    let (entity, collider, local, global, body, parent, character) = item;
    if collider.sensor || character {
        return;
    }
    let dynamic = |b: Option<&RigidBody>| b.is_some_and(|b| b.kind != BodyKind::Static);
    if dynamic(body) || dynamic(parent.and_then(|p| bodies.get(p.parent()).ok())) {
        return;
    }
    // Root entities: use the local transform (valid before transforms propagate).
    let (pos, rot, scale) = if parent.is_some() {
        (global.translation(), global.rotation(), global.scale())
    } else {
        (local.translation, local.rotation, local.scale)
    };
    push_shape(out, entity, pos, rot, scale, &collider.shape);
}

fn push_shape(
    out: &mut Vec<Obstacle>,
    entity: Entity,
    pos: DVec3,
    rot: Quat,
    scale: Vec3,
    shape: &ColliderShape,
) {
    let s = scale.abs();
    match shape {
        ColliderShape::Sphere { radius } => out.push(Obstacle::new(
            entity,
            pos,
            rot,
            ObstacleShape::Sphere {
                radius: radius * s.max_element(),
            },
        )),
        ColliderShape::Cuboid { half_extents } => out.push(Obstacle::new(
            entity,
            pos,
            rot,
            ObstacleShape::Box {
                half: *half_extents * s,
            },
        )),
        ColliderShape::Capsule {
            radius,
            half_height,
        } => out.push(Obstacle::new(
            entity,
            pos,
            rot,
            ObstacleShape::Cylinder {
                radius: radius * s.x.max(s.z),
                half_height: (half_height + radius) * s.y,
            },
        )),
        ColliderShape::Cylinder {
            radius,
            half_height,
        } => out.push(Obstacle::new(
            entity,
            pos,
            rot,
            ObstacleShape::Cylinder {
                radius: radius * s.x.max(s.z),
                half_height: half_height * s.y,
            },
        )),
        ColliderShape::ConvexHull { points } => {
            if let Some((c, h)) = bounds(points.iter().copied()) {
                out.push(Obstacle::new(
                    entity,
                    pos + (rot * (c * s)).as_dvec3(),
                    rot,
                    ObstacleShape::Box { half: h * s },
                ));
            }
        }
        ColliderShape::TriMesh { vertices, .. } => {
            // Small meshes become boxes; big ones (terrain-like) are left to the Ground query.
            if let Some((c, h)) = bounds(vertices.iter().copied())
                && (h * s).max_element() < 25.0
            {
                out.push(Obstacle::new(
                    entity,
                    pos + (rot * (c * s)).as_dvec3(),
                    rot,
                    ObstacleShape::Box { half: h * s },
                ));
            }
        }
        ColliderShape::Heightfield { .. } => {}
        ColliderShape::Compound(parts) => {
            for (offset, r, part) in parts {
                push_shape(
                    out,
                    entity,
                    pos + (rot * (*offset * s)).as_dvec3(),
                    rot * *r,
                    scale,
                    part,
                );
            }
        }
    }
}

fn bounds(points: impl Iterator<Item = Vec3>) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for p in points {
        min = min.min(p);
        max = max.max(p);
    }
    min.is_finite().then(|| {
        (
            (min + max) * 0.5,
            ((max - min) * 0.5).max(Vec3::splat(0.01)),
        )
    })
}

fn obstacle_bits(o: &Obstacle) -> u64 {
    let q = |v: f64| (v * 64.0) as i64 as u64;
    o.entity.to_bits()
        ^ q(o.center.x)
        ^ q(o.center.y).rotate_left(21)
        ^ q(o.center.z).rotate_left(42)
        ^ (o.rotation.x.to_bits() as u64).rotate_left(7)
        ^ (o.rotation.y.to_bits() as u64).rotate_left(29)
        ^ q(o.max.x - o.min.x).rotate_left(13)
        ^ q(o.max.y - o.min.y).rotate_left(35)
        ^ q(o.max.z - o.min.z).rotate_left(51)
}

fn fingerprint(obstacles: &[Obstacle]) -> u64 {
    let mut h = obstacles.len() as u64;
    for o in obstacles {
        h = hash_u64(h ^ obstacle_bits(o));
    }
    h
}

/// A fingerprint of each entity's obstacles, in the order they come.
fn entity_fingerprints(obstacles: &[Obstacle]) -> HashMap<Entity, u64> {
    let mut out: HashMap<Entity, u64> = HashMap::new();
    for o in obstacles {
        let h = out.entry(o.entity).or_insert(1);
        *h = hash_u64(*h ^ obstacle_bits(o));
    }
    out
}

/// A rebuild of the whole model running on the async task pool.
#[derive(Default)]
pub(crate) struct PendingBuild {
    slot: Option<Arc<Mutex<Option<TacticalWorld>>>>,
    /// `TacticalWorld::epoch` when its obstacles were collected.
    epoch: u64,
    /// Ground edits made while it builds, applied once it is in.
    regions: Vec<(DVec2, DVec2)>,
}

/// Keeps [`TacticalWorld`] in step with the static colliders and the ground.
///
/// Only colliders that changed are looked at, and only the cover near them is made again,
/// so streaming terrain tiles (which make no obstacles) and a door swinging somewhere cost
/// next to nothing. A whole rebuild (the first one, or after the world turned) of a big
/// world runs on the async task pool while the old model stays in use.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rebuild_world_model(
    mut model: ResMut<TacticalWorld>,
    colliders: Query<ColliderItem>,
    changed: Query<Entity, Changed<Collider>>,
    bodies: Query<&RigidBody>,
    mut removed: RemovedComponents<Collider>,
    mut terrain: MessageReader<TerrainChanged>,
    ground: Res<Ground>,
    settings: Res<WorldModelSettings>,
    time: Res<Time>,
    mut since_poll: Local<f32>,
    mut pending: Local<PendingBuild>,
) {
    let mut regions: Vec<(DVec2, DVec2)> = terrain
        .read()
        .map(|t| {
            let r = DVec2::splat(t.radius as f64);
            let c = DVec2::new(t.center.x, t.center.z);
            (c - r, c + r)
        })
        .collect();
    let mut touched: HashSet<Entity> = changed.iter().chain(removed.read()).collect();
    if ground.is_changed() {
        model.forget_ground();
    }
    *since_poll += time.real_delta_secs().max(time.delta_secs());
    let mut poll = *since_poll >= settings.poll_secs;

    if let Some(slot) = pending.slot.clone() {
        pending.regions.append(&mut regions);
        let Some(mut built) = slot.lock().ok().and_then(|mut s| s.take()) else {
            return; // still building: the old model serves meanwhile
        };
        pending.slot = None;
        if pending.epoch != model.epoch {
            model.rebuild = true; // the world shifted while it built: start again
        } else {
            built.generation = model.generation + 1;
            built.epoch = model.epoch;
            *model = built;
            regions = std::mem::take(&mut pending.regions);
            poll = true; // catch up with colliders that changed while it built
        }
    }

    if model.generation == 0 || model.rebuild {
        *since_poll = 0.0;
        let obstacles = collect_obstacles(&colliders, &bodies);
        let generation = model.generation + 1;
        let epoch = model.epoch;
        if obstacles.len() > settings.async_above
            && let Some(pool) = AsyncComputeTaskPool::try_get()
        {
            model.rebuild = false;
            let slot = Arc::new(Mutex::new(None));
            let out = slot.clone();
            let ground = ground.clone();
            pool.spawn(async move {
                let built = TacticalWorld::from_obstacles(obstacles, &ground);
                if let Ok(mut out) = out.lock() {
                    *out = Some(built);
                }
            })
            .detach();
            *pending = PendingBuild {
                slot: Some(slot),
                epoch,
                regions,
            };
            return;
        }
        *model = TacticalWorld::from_obstacles(obstacles, &ground);
        model.generation = generation;
        model.epoch = epoch;
        return;
    }

    let fresh = if poll {
        // Every so often look at all of them, for static colliders moved without a change to
        // the collider itself (a parent moved, a transform set by hand).
        *since_poll = 0.0;
        let all = collect_obstacles(&colliders, &bodies);
        let now = entity_fingerprints(&all);
        touched = now
            .iter()
            .filter(|&(e, fp)| model.per_entity.get(e) != Some(fp))
            .map(|(e, _)| *e)
            .chain(
                model
                    .per_entity
                    .keys()
                    .filter(|e| !now.contains_key(e))
                    .copied(),
            )
            .collect();
        all.into_iter()
            .filter(|o| touched.contains(&o.entity))
            .collect()
    } else {
        let mut fresh = Vec::new();
        for &e in &touched {
            if let Ok(item) = colliders.get(e) {
                push_collider(&mut fresh, item, &bodies);
            }
        }
        // Colliders that make no obstacle (terrain tiles, sensors) or the same ones as before
        // change nothing.
        let now = entity_fingerprints(&fresh);
        touched.retain(|e| now.get(e) != model.per_entity.get(e));
        fresh.retain(|o| touched.contains(&o.entity));
        fresh
    };
    if touched.is_empty() && regions.is_empty() {
        return;
    }
    model.update(&touched, fresh, &regions, &ground);
}

/// The physics queries, if a real backend is installed.
pub(crate) fn physics_backend<'a>(
    backend: &Option<Res<PhysicsBackend>>,
    queries: &'a PhysicsQueries,
) -> Option<&'a PhysicsQueries> {
    backend.as_ref().map(|_| queries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(i: u32) -> Entity {
        Entity::from_raw_u32(i).unwrap()
    }

    fn wall(center: DVec3, half: Vec3) -> Obstacle {
        Obstacle::new(e(1), center, Quat::IDENTITY, ObstacleShape::Box { half })
    }

    fn walls(n: u32) -> Vec<Obstacle> {
        (0..n)
            .map(|i| {
                let (x, z) = ((i % 10) as f64 * 6.0, (i / 10) as f64 * 6.0);
                Obstacle::new(
                    e(i + 1),
                    DVec3::new(x, 1.0, z),
                    Quat::from_rotation_y(i as f32 * 0.3),
                    ObstacleShape::Box {
                        half: Vec3::new(1.5, 1.0, 0.3),
                    },
                )
            })
            .collect()
    }

    /// Cover points as sortable keys (the order differs between an update and a rebuild).
    fn cover_keys(w: &TacticalWorld) -> Vec<(i64, i64, i64, bool, u64)> {
        let mut keys: Vec<_> = w
            .cover
            .iter()
            .map(|c| {
                let q = |v: f64| (v * 1000.0).round() as i64;
                let o = &w.obstacles[c.obstacle as usize];
                (
                    q(c.position.x),
                    q(c.position.y),
                    q(c.position.z),
                    c.edge,
                    o.entity.to_bits(),
                )
            })
            .collect();
        keys.sort();
        keys
    }

    #[test]
    fn an_update_matches_a_rebuild() {
        let ground = Ground::default();
        let mut all = walls(60);
        let mut w = TacticalWorld::from_obstacles(all.clone(), &ground);
        // Move one wall next to another, drop one and add a new one.
        let moved = Obstacle::new(
            e(12),
            DVec3::new(13.0, 1.0, 7.5),
            Quat::IDENTITY,
            ObstacleShape::Box {
                half: Vec3::new(1.5, 1.0, 0.3),
            },
        );
        let added = Obstacle::new(
            e(500),
            DVec3::new(30.5, 1.0, 30.5),
            Quat::IDENTITY,
            ObstacleShape::Cylinder {
                radius: 0.6,
                half_height: 1.2,
            },
        );
        let replaced: HashSet<Entity> = [e(12), e(40), e(500)].into();
        w.update(&replaced, vec![moved, added], &[], &ground);
        all.retain(|o| !replaced.contains(&o.entity));
        all.extend([moved, added]);
        let rebuilt = TacticalWorld::from_obstacles(all, &ground);
        let mut a: Vec<u64> = w.obstacles.iter().map(obstacle_bits).collect();
        let mut b: Vec<u64> = rebuilt.obstacles.iter().map(obstacle_bits).collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
        assert_eq!(cover_keys(&w), cover_keys(&rebuilt));
        assert_eq!(w.per_entity, rebuilt.per_entity);
        for i in 0..w.cover.len() {
            let mut near = Vec::new();
            let c = &w.cover[i];
            w.cover_near(c.position, 0.01, &mut near);
            assert!(near.contains(&(i as u32)), "cover point {i} is indexed");
        }
    }

    #[test]
    fn navigation_learns_where_an_update_changed() {
        let ground = Ground::default();
        let mut w = TacticalWorld::from_obstacles(walls(20), &ground);
        let built = w.generation;
        assert!(
            w.changes_since(built)
                .is_some_and(|mut c| c.next().is_none())
        );
        w.update(&[e(3)].into(), Vec::new(), &[], &ground);
        let boxes: Vec<_> = w.changes_since(built).unwrap().collect();
        assert_eq!(boxes.len(), 1);
        let (min, max) = boxes[0];
        assert!(min.x <= 12.0 - 1.5 && max.x >= 12.0 + 1.5, "{boxes:?}");
        assert!(max.x < 40.0, "only near the removed wall: {boxes:?}");
        // A world shift (or a rebuild) changes everything.
        w.shift(&WorldShift::translate(DVec3::new(10.0, 0.0, 0.0)));
        assert!(w.changes_since(built).is_none());
        assert!(w.changes_since(w.generation).is_some());
    }

    #[test]
    fn ray_hits_box_cylinder_sphere() {
        let b = wall(DVec3::new(0.0, 1.0, -5.0), Vec3::new(2.0, 1.0, 0.5));
        let t = b.ray(DVec3::new(0.0, 1.0, 0.0), Vec3::NEG_Z, 10.0).unwrap();
        assert!((t - 4.5).abs() < 1e-4, "{t}");
        assert!(
            b.ray(DVec3::new(0.0, 3.0, 0.0), Vec3::NEG_Z, 10.0)
                .is_none()
        );
        let c = Obstacle::new(
            e(2),
            DVec3::new(10.0, 2.0, 0.0),
            Quat::IDENTITY,
            ObstacleShape::Cylinder {
                radius: 1.0,
                half_height: 2.0,
            },
        );
        let t = c.ray(DVec3::new(0.0, 1.0, 0.0), Vec3::X, 20.0).unwrap();
        assert!((t - 9.0).abs() < 1e-4, "{t}");
        let s = Obstacle::new(
            e(3),
            DVec3::new(0.0, 0.0, 10.0),
            Quat::IDENTITY,
            ObstacleShape::Sphere { radius: 2.0 },
        );
        let t = s.ray(DVec3::ZERO, Vec3::Z, 20.0).unwrap();
        assert!((t - 8.0).abs() < 1e-4, "{t}");
    }

    #[test]
    fn raycast_finds_nearest_through_hash() {
        let w = TacticalWorld::from_obstacles(
            vec![
                wall(DVec3::new(0.0, 1.0, -20.0), Vec3::new(2.0, 1.0, 0.5)),
                wall(DVec3::new(0.0, 1.0, -40.0), Vec3::new(2.0, 1.0, 0.5)),
            ],
            &Ground::default(),
        );
        let (t, i) = w
            .raycast(DVec3::new(0.0, 1.0, 0.0), DVec3::new(0.0, 1.0, -60.0))
            .unwrap();
        assert_eq!(i, 0);
        assert!((t - 19.5).abs() < 1e-3);
        assert!(
            w.raycast(DVec3::new(10.0, 1.0, 0.0), DVec3::new(10.0, 1.0, -60.0))
                .is_none()
        );
    }

    #[test]
    fn cover_points_line_both_sides_of_a_wall() {
        let w = TacticalWorld::from_obstacles(
            vec![wall(DVec3::new(0.0, 1.0, 0.0), Vec3::new(3.0, 1.0, 0.3))],
            &Ground::default(),
        );
        let front = w.cover.iter().filter(|c| c.normal.z > 0.9).count();
        let back = w.cover.iter().filter(|c| c.normal.z < -0.9).count();
        assert!(front >= 3 && back >= 3, "{front} {back}");
        assert!(w.cover.iter().all(|c| c.height == CoverHeight::High));
        for c in &w.cover {
            assert!(!w.blocked_at(c.position, 0.4, 1.6, None), "{c:?}");
        }
    }

    #[test]
    fn surfaces_of_platforms_and_ramps() {
        // A 1.5 m platform and a 20° ramp up to it.
        let platform = wall(DVec3::new(0.0, 0.75, 0.0), Vec3::new(3.0, 0.75, 3.0));
        let angle = 0.35f32;
        let ramp_len = 1.5 / angle.sin();
        let ramp = Obstacle::new(
            e(2),
            DVec3::new(
                0.0,
                0.75 - 0.1,
                3.0 + ramp_len as f64 * 0.5 * angle.cos() as f64,
            ),
            Quat::from_rotation_x(angle),
            ObstacleShape::Box {
                half: Vec3::new(1.0, 0.1, ramp_len * 0.5),
            },
        );
        let w = TacticalWorld::from_obstacles(vec![platform, ramp], &Ground::default());
        let (h, g) = w.stacked_height(0.0, 0.0, 1.8, &Ground::default());
        assert!((h - 1.5).abs() < 1e-3 && g == 0.0, "{h}");
        // Halfway up the ramp is about half the platform height.
        let mid = 3.0 + ramp_len as f64 * 0.5 * angle.cos() as f64;
        let (h, _) = w.stacked_height(0.0, mid, 1.8, &Ground::default());
        assert!((h - 0.75).abs() < 0.2, "{h}");
        // Walking up: from the ground you can't step 1.5 m onto the platform.
        let s = w.surface_height(DVec3::new(0.0, 0.0, 0.0), 0.0, 0.4, &Ground::default());
        assert_eq!(s, 0.0);
        let s = w.surface_height(DVec3::new(0.0, 0.0, 0.0), 1.4, 0.4, &Ground::default());
        assert!((s - 1.5).abs() < 1e-3);
    }

    #[test]
    fn resolve_overlap_pushes_out() {
        let w = TacticalWorld::from_obstacles(
            vec![wall(DVec3::new(0.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 1.0))],
            &Ground::default(),
        );
        let p = w.resolve_overlap(DVec3::new(1.2, 0.0, 0.0), 0.4, 1.8, 0.4);
        assert!(p.x >= 1.399, "{p}");
        let inside = w.resolve_overlap(DVec3::new(0.8, 0.0, 0.1), 0.4, 1.8, 0.4);
        assert!(inside.x >= 1.399, "{inside}");
    }
}
