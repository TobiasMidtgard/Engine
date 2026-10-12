//! Proves the physics contract and its conformance suite hold together: a deliberately tiny
//! backend (axis-aligned boxes, no rotation) written only against `engine_world::physics`
//! passes every check, and fails the right checks when one of its deliberate [`Bugs`] is
//! switched on. It also shows the minimum a replacement physics module has to do.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::Gravity;
use engine_world::physics::*;

#[derive(Default)]
struct MiniPhysics {
    /// Deliberate contract violations, all off for a correct backend. Tests switch them on to
    /// prove the conformance suite catches them.
    bugs: Bugs,
}

#[derive(Resource, Default, Clone, Copy, Debug)]
struct Bugs {
    /// Forget which pairs touch: send `Started` every step of a contact and never `Ended`.
    no_pair_tracking: bool,
    /// Report every contact impulse as 0.
    zero_impulse: bool,
    /// Report contact normals from `b` toward `a`.
    flipped_normal: bool,
}

#[derive(Clone, Copy)]
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
}

#[derive(Resource, Clone)]
struct Shared(Arc<RwLock<Snapshot>>);

#[derive(Resource, Default)]
struct Touching(HashSet<(Entity, Entity)>);

struct Queries(Arc<RwLock<Snapshot>>);

impl Plugin for MiniPhysics {
    fn build(&self, app: &mut App) {
        claim_backend(app, "mini");
        let shared = Arc::new(RwLock::new(Snapshot::default()));
        app.insert_resource(PhysicsQueries(Arc::new(Queries(shared.clone()))))
            .insert_resource(Shared(shared))
            .insert_resource(self.bugs)
            .init_resource::<Touching>()
            .add_systems(
                FixedUpdate,
                (step_bodies, step_characters, publish_snapshot)
                    .chain()
                    .in_set(SimSet::Physics),
            );
    }
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
    let h = half_extents(&c.shape).as_dvec3();
    Aabb {
        entity,
        min: t.translation - h,
        max: t.translation + h,
        layers: c.layers,
        sensor: c.sensor,
        surface: c.surface,
    }
}

/// Push `pos` (half extents `h`) out of `other`; returns the push direction if they overlapped.
fn resolve(pos: &mut DVec3, h: DVec3, other: &Aabb) -> Option<DVec3> {
    let (min, max) = (*pos - h, *pos + h);
    let overlap = DVec3::new(
        (max.x.min(other.max.x) - min.x.max(other.min.x)).max(0.0),
        (max.y.min(other.max.y) - min.y.max(other.min.y)).max(0.0),
        (max.z.min(other.max.z) - min.z.max(other.min.z)).max(0.0),
    );
    if overlap.x <= 0.0 || overlap.y <= 0.0 || overlap.z <= 0.0 {
        return None;
    }
    let center = (other.min + other.max) * 0.5;
    let axis = if overlap.y <= overlap.x && overlap.y <= overlap.z {
        DVec3::Y * (pos.y - center.y).signum()
    } else if overlap.x <= overlap.z {
        DVec3::X * (pos.x - center.x).signum()
    } else {
        DVec3::Z * (pos.z - center.z).signum()
    };
    *pos += axis * (axis.abs().dot(overlap) + 1e-6);
    Some(axis)
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn step_bodies(
    time: Res<Time>,
    gravity: Res<Gravity>,
    bugs: Res<Bugs>,
    mut impulses: MessageReader<ApplyImpulse>,
    mut contacts: MessageWriter<ContactEvent>,
    mut touching: ResMut<Touching>,
    statics: Query<(Entity, &Transform, &Collider, Option<&RigidBody>), Without<Velocity>>,
    mut dynamics: Query<(Entity, &mut Transform, &mut Velocity, &RigidBody, &Collider)>,
) {
    let dt = time.delta_secs_f64();
    let solid: Vec<Aabb> = statics
        .iter()
        .filter(|(_, _, _, b)| b.is_none_or(|b| b.kind == BodyKind::Static))
        .map(|(e, t, c, _)| aabb(e, t, c))
        .collect();
    let pending: Vec<ApplyImpulse> = impulses.read().copied().collect();
    let mut now = HashSet::new();
    for (e, mut t, mut v, body, collider) in &mut dynamics {
        if body.kind != BodyKind::Dynamic {
            continue;
        }
        let h = half_extents(&collider.shape);
        let mass = body
            .mass_kg
            .unwrap_or(collider.density * 8.0 * h.x * h.y * h.z);
        for imp in pending.iter().filter(|i| i.entity == e) {
            v.linear += imp.impulse / mass;
        }
        v.linear += gravity.at(t.translation) * body.gravity_scale * dt as f32;
        let mut pos = t.translation + v.linear.as_dvec3() * dt;
        for s in &solid {
            if let Some(axis) = resolve(&mut pos, h.as_dvec3(), s) {
                let n = axis.as_vec3();
                let into = v.linear.dot(n);
                if into < 0.0 {
                    v.linear -= n * into;
                }
                if axis.y > 0.0 {
                    v.linear *= 0.5; // ground friction stand-in
                }
                now.insert((s.entity, e));
                if bugs.no_pair_tracking || !touching.0.contains(&(s.entity, e)) {
                    contacts.write(ContactEvent {
                        phase: ContactPhase::Started,
                        a: s.entity,
                        b: e,
                        point: pos - axis * h.y as f64,
                        normal: if bugs.flipped_normal { -n } else { n },
                        impulse: if bugs.zero_impulse { 0.0 } else { -into * mass },
                        sensor: false,
                    });
                }
            }
        }
        t.translation = pos;
    }
    if bugs.no_pair_tracking {
        return;
    }
    for &(a, b) in touching.0.difference(&now) {
        contacts.write(ContactEvent {
            phase: ContactPhase::Ended,
            a,
            b,
            point: DVec3::ZERO,
            normal: Vec3::ZERO,
            impulse: 0.0,
            sensor: false,
        });
    }
    touching.0 = now;
}

fn step_characters(
    time: Res<Time>,
    statics: Query<(Entity, &Transform, &Collider), Without<CharacterController>>,
    mut characters: Query<(
        &mut Transform,
        &CharacterController,
        &CharacterMove,
        &mut CharacterState,
        Option<&LocalTimeScale>,
    )>,
) {
    let solid: Vec<Aabb> = statics.iter().map(|(e, t, c)| aabb(e, t, c)).collect();
    for (mut t, cc, mv, mut state, local) in &mut characters {
        // The character's own seconds (contract: `CharacterMove`).
        let dt = time.delta_for(local) as f64;
        let h = DVec3::new(cc.radius as f64, cc.height as f64 * 0.5, cc.radius as f64);
        let start = t.translation;
        let mut pos = start + mv.velocity.as_dvec3() * dt;
        let mut grounded = false;
        for s in &solid {
            if let Some(axis) = resolve(&mut pos, h, s)
                && axis.y > 0.0
            {
                grounded = true;
                state.ground_normal = Vec3::Y;
                state.ground_entity = Some(s.entity);
                state.ground_surface = Some(s.surface);
            }
        }
        state.grounded = grounded;
        state.velocity = ((pos - start) / dt.max(1e-9)).as_vec3();
        t.translation = pos;
    }
}

fn publish_snapshot(shared: Res<Shared>, q: Query<(Entity, &Transform, &Collider)>) {
    let mut snap = shared.0.write().unwrap();
    snap.boxes = q.iter().map(|(e, t, c)| aabb(e, t, c)).collect();
}

impl PhysicsQuery for Queries {
    fn raycast(
        &self,
        origin: DVec3,
        dir: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Option<Hit> {
        let snap = self.0.read().unwrap();
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
        let snap = self.0.read().unwrap();
        out.extend(
            snap.boxes
                .iter()
                .filter(|b| filter.accepts(b.entity, &b.layers, b.sensor))
                .filter(|b| (position - h).cmple(b.max).all() && (position + h).cmpge(b.min).all())
                .map(|b| b.entity),
        );
    }

    fn closest_point(&self, point: DVec3, max_distance: f32, filter: &QueryFilter) -> Option<Hit> {
        let snap = self.0.read().unwrap();
        snap.boxes
            .iter()
            .filter(|b| filter.accepts(b.entity, &b.layers, b.sensor))
            .map(|b| {
                let p = point.clamp(b.min, b.max);
                let d = p.distance(point) as f32;
                Hit {
                    entity: b.entity,
                    point: p,
                    normal: (point - p).normalize_or_zero().as_vec3(),
                    distance: d,
                    surface: b.surface,
                }
            })
            .filter(|h| h.distance <= max_distance)
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }
}

#[test]
fn a_minimal_backend_passes_the_conformance_suite() {
    engine_world::physics::conformance::assert_all(|app| {
        app.add_plugins(MiniPhysics::default());
    });
}

#[test]
fn the_suite_catches_wrong_contact_events() {
    let cases: [(&str, Bugs, &[&str], &str); 3] = [
        (
            "Started every step and never Ended",
            Bugs {
                no_pair_tracking: true,
                ..Bugs::default()
            },
            &["reports_contacts", "reports_contact_end"],
            "Started",
        ),
        (
            "impulse always 0",
            Bugs {
                zero_impulse: true,
                ..Bugs::default()
            },
            &["reports_contacts"],
            "impulse",
        ),
        (
            "normal from b toward a",
            Bugs {
                flipped_normal: true,
                ..Bugs::default()
            },
            &["reports_contacts"],
            "normal",
        ),
    ];
    for (bug, bugs, expected, says) in cases {
        let checks = engine_world::physics::conformance::run_all(|app| {
            app.add_plugins(MiniPhysics { bugs });
        });
        let failed: Vec<_> = checks.iter().filter(|c| c.result.is_err()).collect();
        let names: Vec<&str> = failed.iter().map(|c| c.name).collect();
        let reasons: Vec<String> = failed
            .iter()
            .map(|c| format!("{}: {}", c.name, c.result.as_ref().unwrap_err()))
            .collect();
        assert_eq!(
            names,
            expected,
            "a backend whose contact events have {bug} should fail exactly these checks; it \
             failed:\n  {}",
            reasons.join("\n  ")
        );
        let first = failed[0].result.as_ref().unwrap_err();
        assert!(
            first.contains(says),
            "the error for {bug} should mention {says}: {first}"
        );
    }
}

#[test]
fn no_backend_fails_the_suite_with_clear_reasons() {
    let checks = engine_world::physics::conformance::run_all(|_| {});
    let failed: Vec<_> = checks.iter().filter(|c| c.result.is_err()).collect();
    assert!(
        failed.len() >= 5,
        "the suite should catch a missing backend"
    );
    let first = failed[0].result.as_ref().unwrap_err();
    assert!(first.contains("claim_backend"), "{first}");
}
