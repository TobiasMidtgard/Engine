//! Stand-in character physics, used only while no physics module is installed.
//!
//! The physics contract says the physics backend moves characters ([`CharacterMove`] in,
//! [`CharacterState`] out) and answers scene queries ([`PhysicsQueries`]). Until a game adds a
//! physics module, nothing would move, so this file does the minimum itself: characters slide
//! against static colliders (as boxes) and stand on the [`Ground`] (terrain), and raycasts see
//! both. It steps aside on its own as soon as a physics backend is installed
//! ([`PhysicsBackend`] exists), so it never fights a real one.
//!
//! It is deliberately simple: no rotation of characters, no rigid bodies, no pushing. A
//! character standing on a box that moves (a lift, a train car) is carried by how far the box
//! moved, as the contract asks.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::Ground;
use engine_world::physics::{
    CharacterController, CharacterMove, CharacterState, Collider, ColliderShape, CollisionLayers,
    Hit, PhysicsBackend, PhysicsQueries, PhysicsQuery, QueryFilter,
};

/// Largest distance a character moves in one collision sub-step, meters.
const SUBSTEP_METERS: f64 = 0.1;
/// Shortest step when a ray marches over the ground, meters.
const MIN_GROUND_STEP: f64 = 0.25;

#[derive(Clone, Copy, Debug)]
struct Aabb {
    entity: Entity,
    min: DVec3,
    max: DVec3,
    layers: CollisionLayers,
    sensor: bool,
    surface: engine_world::SurfaceId,
}

#[derive(Default)]
struct Snapshot {
    boxes: Vec<Aabb>,
    /// How far each box moved since the snapshot before, meters (only boxes that moved).
    moved: HashMap<Entity, DVec3>,
}

/// The stand-in's shared state (its queries read the same snapshot).
#[derive(Resource, Clone)]
pub(crate) struct Fallback {
    snapshot: Arc<RwLock<Snapshot>>,
}

struct Queries {
    snapshot: Arc<RwLock<Snapshot>>,
    ground: Ground,
}

/// True while no physics module is installed.
pub(crate) fn no_physics(backend: Option<Res<PhysicsBackend>>) -> bool {
    backend.is_none()
}

fn half_extents(shape: &ColliderShape) -> Vec3 {
    match shape {
        ColliderShape::Sphere { radius } => Vec3::splat(*radius),
        ColliderShape::Cuboid { half_extents } => *half_extents,
        ColliderShape::Capsule {
            radius,
            half_height,
        }
        | ColliderShape::Cylinder {
            radius,
            half_height,
        } => Vec3::new(*radius, half_height + radius, *radius),
        _ => Vec3::splat(0.5),
    }
}

fn aabb(entity: Entity, t: &Transform, c: &Collider) -> Aabb {
    // Bounding box of the rotated shape.
    let h = half_extents(&c.shape);
    let m = engine_core::math::Mat3::from_quat(t.rotation);
    let h = (m.x_axis.abs() * h.x + m.y_axis.abs() * h.y + m.z_axis.abs() * h.z).as_dvec3();
    Aabb {
        entity,
        min: t.translation - h,
        max: t.translation + h,
        layers: c.layers,
        sensor: c.sensor,
        surface: c.surface,
    }
}

/// Install the stand-in's queries and refresh what they see (FixedUpdate, before gameplay).
#[allow(clippy::type_complexity)]
pub(crate) fn publish(
    mut commands: Commands,
    fallback: Option<Res<Fallback>>,
    ground: Res<Ground>,
    colliders: Query<
        (
            Entity,
            &Transform,
            &GlobalTransform,
            Has<ChildOf>,
            &Collider,
        ),
        Without<CharacterController>,
    >,
) {
    // A child's `Transform` is relative to its parent: it is where its `GlobalTransform` says.
    let boxes: Vec<Aabb> = colliders
        .iter()
        .filter(|(.., c)| !c.sensor)
        .map(|(e, t, g, parented, c)| {
            let t = if parented { g.to_transform() } else { *t };
            aabb(e, &t, c)
        })
        .collect();
    match fallback {
        Some(f) => {
            let mut snap = f.snapshot.write().expect("snapshot lock");
            let before: HashMap<Entity, DVec3> =
                snap.boxes.iter().map(|b| (b.entity, b.min)).collect();
            snap.moved = boxes
                .iter()
                .filter_map(|b| {
                    let d = b.min - *before.get(&b.entity)?;
                    (d != DVec3::ZERO).then_some((b.entity, d))
                })
                .collect();
            snap.boxes = boxes;
        }
        None => {
            let snapshot = Arc::new(RwLock::new(Snapshot {
                boxes,
                moved: HashMap::new(),
            }));
            commands.insert_resource(PhysicsQueries(Arc::new(Queries {
                snapshot: snapshot.clone(),
                ground: ground.clone(),
            })));
            commands.insert_resource(Fallback { snapshot });
        }
    }
}

/// Move characters by `CharacterMove` in their own seconds; collide and report contacts.
#[allow(clippy::type_complexity)]
pub(crate) fn step_characters(
    time: Res<Time>,
    ground: Res<Ground>,
    fallback: Option<Res<Fallback>>,
    mut characters: Query<(
        Entity,
        &mut Transform,
        &CharacterController,
        &CharacterMove,
        &mut CharacterState,
        Option<&LocalTimeScale>,
    )>,
) {
    let Some(fallback) = fallback else {
        return;
    };
    let snap = fallback.snapshot.read().expect("snapshot lock");
    for (entity, mut t, cc, mv, mut state, local) in &mut characters {
        let dt = time.delta_for(local) as f64;
        if dt <= 0.0 {
            continue;
        }
        let h = DVec3::new(cc.radius as f64, cc.height as f64 * 0.5, cc.radius as f64);
        let max_slope_cos = cc.max_slope_deg.to_radians().cos();
        // Carried by the box it stood on, if that moved.
        let carried = state
            .ground_entity
            .and_then(|g| snap.moved.get(&g))
            .copied()
            .unwrap_or(DVec3::ZERO);
        let start = t.translation + carried;
        let travel = mv.velocity.as_dvec3() * dt;
        let steps = (travel.length() / SUBSTEP_METERS).ceil().clamp(1.0, 64.0) as u32;
        let mut pos = start;
        let mut out = CharacterState::default();
        // Only boxes near the swept path matter.
        let reach = h + DVec3::splat(cc.step_height as f64 + 0.1);
        let (lo, hi) = (
            start.min(start + travel) - reach,
            start.max(start + travel) + reach,
        );
        let near: Vec<&Aabb> = snap
            .boxes
            .iter()
            .filter(|b| b.entity != entity && b.min.cmple(hi).all() && b.max.cmpge(lo).all())
            .collect();
        for _ in 0..steps {
            pos += travel / steps as f64;
            for b in &near {
                let Some(axis) = resolve(&mut pos, h, b, cc.step_height as f64, out.grounded)
                else {
                    continue;
                };
                if axis.y > 0.5 {
                    out.grounded = true;
                    out.ground_normal = Vec3::Y;
                    out.ground_entity = Some(b.entity);
                    out.ground_surface = Some(b.surface);
                } else if axis.y < -0.5 {
                    out.ceiling = true;
                } else {
                    out.wall_normal = Some(axis.as_vec3());
                }
            }
            // Ground too steep to stand on blocks like a wall: no walking up it sideways.
            if let Some(g) = ground.sample(pos)
                && pos.y - h.y < g.height - 1e-4
                && g.normal.y < max_slope_cos
            {
                let back = pos - DVec3::new(travel.x, 0.0, travel.z) / steps as f64;
                if ground.sample(back).is_some_and(|b| b.height < g.height) {
                    pos = back;
                    out.wall_normal = Some(g.normal);
                }
            }
            if let Some(g) = ground.sample(pos)
                && pos.y - h.y <= g.height + 1e-4
            {
                pos.y = g.height + h.y;
                out.grounded = true;
                out.ground_normal = g.normal;
                out.ground_entity = None;
                out.ground_surface = Some(g.surface);
            }
        }
        out.velocity = ((pos - start) / dt).as_vec3();
        *state = out;
        t.translation = pos;
    }
}

/// Push `pos` (half extents `h`) out of `b`; returns the push direction if they overlapped.
/// A low box edge (up to `step`) is stepped onto instead of blocking, when on the ground.
fn resolve(pos: &mut DVec3, h: DVec3, b: &Aabb, step: f64, grounded: bool) -> Option<DVec3> {
    let (min, max) = (*pos - h, *pos + h);
    let overlap = DVec3::new(
        max.x.min(b.max.x) - min.x.max(b.min.x),
        max.y.min(b.max.y) - min.y.max(b.min.y),
        max.z.min(b.max.z) - min.z.max(b.min.z),
    );
    if overlap.x <= 0.0 || overlap.y <= 0.0 || overlap.z <= 0.0 {
        return None;
    }
    let center = (b.min + b.max) * 0.5;
    let rise = b.max.y - min.y;
    let axis = if overlap.y <= overlap.x && overlap.y <= overlap.z {
        DVec3::Y * (pos.y - center.y).signum()
    } else if grounded && rise > 0.0 && rise <= step {
        DVec3::Y
    } else if overlap.x <= overlap.z {
        DVec3::X * (pos.x - center.x).signum()
    } else {
        DVec3::Z * (pos.z - center.z).signum()
    };
    let push = if axis.y > 0.0 {
        rise
    } else {
        axis.abs().dot(overlap)
    };
    *pos += axis * (push + 1e-6);
    Some(axis)
}

impl PhysicsQuery for Queries {
    fn raycast(
        &self,
        origin: DVec3,
        dir: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Option<Hit> {
        let snap = self.snapshot.read().expect("snapshot lock");
        let d = dir.as_dvec3();
        let mut best: Option<Hit> = None;
        for b in &snap.boxes {
            if !filter.accepts(b.entity, &b.layers, b.sensor) {
                continue;
            }
            let (mut t0, mut t1, mut normal) = (0.0f64, max_distance as f64, Vec3::ZERO);
            let mut hit = true;
            for axis in 0..3 {
                let (o, dd, lo, hi) = (origin[axis], d[axis], b.min[axis], b.max[axis]);
                if dd.abs() < 1e-12 {
                    if o < lo || o > hi {
                        hit = false;
                    }
                    continue;
                }
                let (mut a, mut c) = ((lo - o) / dd, (hi - o) / dd);
                let mut n = Vec3::ZERO;
                n[axis] = if dd > 0.0 { -1.0 } else { 1.0 };
                if a > c {
                    std::mem::swap(&mut a, &mut c);
                }
                if a > t0 {
                    t0 = a;
                    normal = n;
                }
                t1 = t1.min(c);
            }
            if hit && t0 <= t1 && best.is_none_or(|h| (t0 as f32) < h.distance) {
                best = Some(Hit {
                    entity: b.entity,
                    point: origin + d * t0,
                    normal,
                    distance: t0 as f32,
                    surface: b.surface,
                });
            }
        }
        // The ground (terrain): march along the ray. Each step goes as far as half the ray's
        // height above the ground (at least `MIN_GROUND_STEP`), which can't skip terrain no
        // steeper than about 1:1, so rays far above the ground (or pointing up) take a few
        // dozen samples instead of one every 25 cm.
        let limit = best.map_or(max_distance, |h| h.distance) as f64;
        let mut prev = 0.0;
        let mut s = 0.0;
        while s <= limit {
            let p = origin + d * s;
            let ground = self.ground.sample(p);
            if let Some(g) = ground
                && p.y <= g.height
            {
                // Refine between the last two samples.
                let (mut lo, mut hi) = (prev, s);
                for _ in 0..12 {
                    let mid = (lo + hi) * 0.5;
                    let q = origin + d * mid;
                    if self.ground.height(q).is_some_and(|gh| q.y <= gh) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                let at = origin + d * hi;
                return Some(Hit {
                    entity: Entity::PLACEHOLDER,
                    point: at,
                    normal: self.ground.sample(at).map_or(Vec3::Y, |g| g.normal),
                    distance: hi as f32,
                    surface: g.surface,
                });
            }
            prev = s;
            s += ground.map_or(MIN_GROUND_STEP, |g| {
                ((p.y - g.height) * 0.5).max(MIN_GROUND_STEP)
            });
            if s > limit && prev < limit {
                s = limit;
            }
        }
        best
    }

    fn shapecast(
        &self,
        _: &ColliderShape,
        origin: DVec3,
        _: Quat,
        dir: Vec3,
        max: f32,
        filter: &QueryFilter,
    ) -> Option<Hit> {
        self.raycast(origin, dir, max, filter)
    }

    fn overlap(
        &self,
        shape: &ColliderShape,
        position: DVec3,
        _: Quat,
        filter: &QueryFilter,
        out: &mut Vec<Entity>,
    ) {
        let h = half_extents(shape).as_dvec3();
        let snap = self.snapshot.read().expect("snapshot lock");
        out.extend(
            snap.boxes
                .iter()
                .filter(|b| filter.accepts(b.entity, &b.layers, b.sensor))
                .filter(|b| (position - h).cmple(b.max).all() && (position + h).cmpge(b.min).all())
                .map(|b| b.entity),
        );
    }

    fn closest_point(&self, point: DVec3, max_distance: f32, filter: &QueryFilter) -> Option<Hit> {
        let snap = self.snapshot.read().expect("snapshot lock");
        snap.boxes
            .iter()
            .filter(|b| filter.accepts(b.entity, &b.layers, b.sensor))
            .map(|b| {
                let p = point.clamp(b.min, b.max);
                Hit {
                    entity: b.entity,
                    point: p,
                    normal: (point - p).normalize_or_zero().as_vec3(),
                    distance: p.distance(point) as f32,
                    surface: b.surface,
                }
            })
            .filter(|h| h.distance <= max_distance)
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall() -> Aabb {
        Aabb {
            entity: Entity::PLACEHOLDER,
            min: DVec3::new(1.0, 0.0, -5.0),
            max: DVec3::new(2.0, 3.0, 5.0),
            layers: CollisionLayers::default(),
            sensor: false,
            surface: engine_world::SurfaceId(0),
        }
    }

    #[test]
    fn walls_push_out_sideways_and_low_edges_are_stepped_onto() {
        let h = DVec3::new(0.35, 0.9, 0.35);
        let mut pos = DVec3::new(0.7, 1.0, 0.0);
        assert_eq!(resolve(&mut pos, h, &wall(), 0.35, true), Some(-DVec3::X));
        assert!((pos.x - 0.65).abs() < 1e-5);
        let step = Aabb {
            max: DVec3::new(2.0, 0.2, 5.0),
            ..wall()
        };
        let mut pos = DVec3::new(0.7, 0.9, 0.0);
        assert_eq!(resolve(&mut pos, h, &step, 0.35, true), Some(DVec3::Y));
        assert!((pos.y - 1.1).abs() < 1e-5);
        // In the air the same edge blocks.
        let mut pos = DVec3::new(0.7, 0.9, 0.0);
        assert_eq!(resolve(&mut pos, h, &step, 0.35, false), Some(-DVec3::X));
    }

    #[test]
    fn rays_hit_boxes_and_the_ground() {
        let q = Queries {
            snapshot: Arc::new(RwLock::new(Snapshot {
                boxes: vec![wall()],
                ..Default::default()
            })),
            ground: Ground::default(),
        };
        let f = QueryFilter::default();
        let hit = q
            .raycast(DVec3::new(0.0, 1.0, 0.0), Vec3::X, 5.0, &f)
            .unwrap();
        assert!((hit.distance - 1.0).abs() < 1e-5);
        assert_eq!(hit.normal, -Vec3::X);
        let down = q
            .raycast(DVec3::new(-3.0, 2.0, 0.0), -Vec3::Y, 5.0, &f)
            .unwrap();
        assert!((down.distance - 2.0).abs() < 1e-3, "{down:?}");
        assert!(
            q.raycast(DVec3::new(-3.0, 2.0, 0.0), -Vec3::Y, 1.5, &f)
                .is_none()
        );
    }

    /// A 45° ramp rising along +x from x = 0 (flat below), counting its samples.
    struct Ramp(std::sync::atomic::AtomicUsize);
    impl engine_world::GroundQuery for Ramp {
        fn sample(&self, p: DVec3) -> Option<engine_world::GroundSample> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(engine_world::GroundSample {
                height: p.x.max(0.0),
                normal: Vec3::Y,
                surface: engine_world::SurfaceId(0),
            })
        }
    }

    #[test]
    fn long_rays_over_the_ground_take_few_samples_and_still_hit_slopes() {
        let ramp = Arc::new(Ramp(Default::default()));
        let q = Queries {
            snapshot: Arc::new(RwLock::new(Snapshot::default())),
            ground: Ground(ramp.clone()),
        };
        let f = QueryFilter::default();
        let samples = || ramp.0.swap(0, std::sync::atomic::Ordering::Relaxed);
        // Straight up from just above the ground, 400 m: nothing to hit.
        assert!(
            q.raycast(DVec3::new(-10.0, 0.5, 0.0), Vec3::Y, 400.0, &f)
                .is_none()
        );
        assert!(samples() < 40, "upward ray took too many samples");
        // Level ray at 2 m toward the ramp: hits where the ramp reaches 2 m.
        let hit = q
            .raycast(DVec3::new(-50.0, 2.0, 0.0), Vec3::X, 100.0, &f)
            .unwrap();
        assert!((hit.point.x - 2.0).abs() < 0.01, "{hit:?}");
        assert!(samples() < 300);
        // A ray climbing more gently than the ramp still hits it.
        let dir = Vec3::new(1.0, 0.5, 0.0).normalize();
        let hit = q
            .raycast(DVec3::new(-20.0, 10.0, 0.0), dir, 200.0, &f)
            .unwrap();
        assert!((hit.point.y - hit.point.x).abs() < 0.01, "{hit:?}");
    }
}
