//! Grass and plants part around whoever walks or drives through them.
//!
//! The renderer bends grass blades (and, with Rendering's mesh push, swaying plant leaves)
//! away from [`GrassBender`] spheres, but nothing put one on a walker. Every module that moves
//! something over the ground already says so with a [`GroundContact`] naming the entity (feet
//! from movement and animation, wheels and tracks from vehicles), so this module listens to
//! those and gives the entity a bender of its own, a child that rides along with it:
//!
//! - feet, paws and hooves: one sphere centered under the body, a little above the soles, as
//!   wide as the legs swing (people part grass with their shins and knees, not their toes);
//! - wheels and tracks: one sphere per wheel, at the wheel, sized from its contact patch.
//!
//! A bender follows its walker through jumps (it lifts out of the grass with it, so nothing
//! bends under a flying body) and goes when the walker does. A contact that lands far from
//! its entity (a writer that names something other than the body) is ignored.

use std::collections::HashMap;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_render::GrassBender;
use engine_world::{ContactKind, GroundContact};

/// A [`GrassBender`] this module put on a walker, at its feet or one of its wheels.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct WalkerBender {
    /// Where it sits in the walker's frame, at contact level (before the lift).
    pub at: Vec3,
    /// Under feet, paws or hooves (one per walker, leaving a wake), not at a wheel.
    pub feet: bool,
}

/// Where a walker just was: the grass there is still rising back. A bender left in the
/// world that shrinks to nothing over [`WAKE_SECS`].
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct GrassWake {
    pub radius: f32,
    pub secs_left: f32,
}

/// How long trampled-aside grass takes to stand back up behind a walker, seconds.
pub const WAKE_SECS: f32 = 1.2;

/// A wake bender is left every this many meters walked, and at most this many per walker
/// (the renderer bends for the 16 benders nearest the camera, so they stay few).
const WAKE_STEP_M: f64 = 0.7;
const WAKES_PER_WALKER: usize = 2;

/// How far above the contact a bender's center sits, as a share of its radius: the blades
/// whose roots are under the walker lie flat, the ones beside it lean away.
const LIFT: f32 = 0.6;

/// A contact farther than this from its entity's origin (horizontally) is not that entity's
/// feet or wheels, meters.
const MAX_REACH_M: f64 = 4.0;

/// Two wheel contacts this close (in the vehicle's frame) are the same wheel, meters.
const SAME_WHEEL_M: f32 = 0.7;

/// The most benders one walker gets (a six-wheeler, or a tracked vehicle's two tracks).
const MAX_PER_WALKER: usize = 8;

/// The sphere a contact of `kind` and footprint `size` (width, length) presses, meters.
pub fn bender_radius(kind: ContactKind, size: [f32; 2]) -> f32 {
    match kind {
        // The legs' swing, not the foot: grass parts about knee-wide around a walker.
        ContactKind::Foot => 0.45,
        ContactKind::Paw => 0.32,
        ContactKind::Hoof => 0.55,
        ContactKind::Wheel | ContactKind::Track => 0.3 + 0.5 * size[0].max(size[1]),
        ContactKind::Body => 0.0,
    }
}

fn foot_like(kind: ContactKind) -> bool {
    matches!(
        kind,
        ContactKind::Foot | ContactKind::Paw | ContactKind::Hoof
    )
}

/// Each walker's benders: entity and place in the walker's frame.
#[derive(Default)]
pub struct Walkers(HashMap<Entity, Vec<(Entity, Vec3)>>);

/// Gives every entity that touches the ground with feet, paws, hooves, wheels or tracks a
/// [`GrassBender`] that rides with it (see the module notes).
pub fn bend_grass_for_walkers(
    mut commands: Commands,
    mut contacts: MessageReader<GroundContact>,
    bodies: Query<&GlobalTransform>,
    benders: Query<(), With<WalkerBender>>,
    mut walkers: Local<Walkers>,
) {
    // Forget walkers that left (their benders went with them).
    walkers
        .0
        .retain(|w, list| bodies.contains(*w) && list.iter().all(|(b, _)| benders.contains(*b)));
    for c in contacts.read() {
        let Some(walker) = c.entity else { continue };
        let radius = bender_radius(c.kind, c.size);
        if radius <= 0.0 {
            continue;
        }
        let Ok(body) = bodies.get(walker) else {
            continue;
        };
        let origin = body.translation();
        let off = c.position - origin;
        if DVec3::new(off.x, 0.0, off.z).length() > MAX_REACH_M {
            continue;
        }
        let local = body.inverse().transform_point3(c.position).as_vec3();
        let feet = foot_like(c.kind);
        // Feet: one sphere under the body's middle, at the soles' height.
        let at = if foot_like(c.kind) {
            Vec3::new(0.0, local.y, 0.0)
        } else {
            local
        };
        let list = walkers.0.entry(walker).or_default();
        let same = if foot_like(c.kind) {
            list.first().map(|_| 0)
        } else {
            list.iter()
                .position(|(_, p)| (p.x - at.x).hypot(p.z - at.z) < SAME_WHEEL_M)
        };
        let scale = body.scale().max_element().max(1e-3);
        let place = |at: Vec3| {
            // Children are placed in the walker's (possibly scaled) frame; the radius is not.
            Transform::from_translation((at + Vec3::Y * radius * LIFT / scale).as_dvec3())
        };
        match same {
            Some(i) => {
                let (bender, old) = &mut list[i];
                // The soles' height settles slowly, so a step on a stone doesn't jerk it.
                let at = if foot_like(c.kind) {
                    Vec3::new(0.0, old.y + (at.y - old.y) * 0.3, 0.0)
                } else {
                    at
                };
                if (at - *old).length() > 0.02 {
                    *old = at;
                    commands.entity(*bender).insert((
                        place(at),
                        WalkerBender { at, feet },
                        GrassBender { radius },
                    ));
                }
            }
            None if list.len() < MAX_PER_WALKER => {
                let bender = commands
                    .spawn((
                        place(at),
                        GrassBender { radius },
                        WalkerBender { at, feet },
                        ChildOf(walker),
                    ))
                    .id();
                list.push((bender, at));
            }
            None => {}
        }
    }
}

/// Leaves a shrinking [`GrassWake`] behind each walker's feet as it moves, so the grass it
/// pushed aside stands back up over a second instead of springing up the moment it passes.
pub fn leave_grass_wakes(
    mut commands: Commands,
    time: Res<Time>,
    feet: Query<(Entity, &WalkerBender, &GrassBender, &GlobalTransform)>,
    mut wakes: Query<(Entity, &mut GrassWake, &mut GrassBender), Without<WalkerBender>>,
    mut trails: Local<HashMap<Entity, (DVec3, Vec<Entity>)>>,
) {
    let dt = time.delta_secs();
    for (e, mut wake, mut bender) in &mut wakes {
        wake.secs_left -= dt;
        if wake.secs_left <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        // Eases back: most of the way up in the first half.
        let left = wake.secs_left / WAKE_SECS;
        bender.radius = wake.radius * left * left.sqrt();
    }
    trails.retain(|b, _| feet.contains(*b));
    for (e, walker, bender, gt) in &feet {
        if !walker.feet {
            continue;
        }
        let here = gt.translation();
        let (last, list) = trails.entry(e).or_insert((here, Vec::new()));
        list.retain(|w| wakes.contains(*w));
        // Moved along (a jump or a teleport moves it up or far: no wake in the air).
        let moved = here - *last;
        if moved.length() < WAKE_STEP_M {
            continue;
        }
        let drop = *last;
        *last = here;
        if moved.length() > 6.0 || moved.y.abs() > 0.8 {
            continue;
        }
        if list.len() >= WAKES_PER_WALKER {
            commands.entity(list.remove(0)).despawn();
        }
        list.push(
            commands
                .spawn((
                    Transform::from_translation(drop),
                    GrassBender {
                        radius: bender.radius,
                    },
                    GrassWake {
                        radius: bender.radius,
                        secs_left: WAKE_SECS,
                    },
                ))
                .id(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::Quat;

    fn app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_message::<GroundContact>()
            .add_systems(Update, (bend_grass_for_walkers, leave_grass_wakes).chain());
        app
    }

    fn step(app: &mut App, walker: Entity, at: DVec3, kind: ContactKind) {
        app.world.write_message(GroundContact {
            position: at,
            normal: Vec3::Y,
            size: [0.11, 0.28],
            yaw: 0.0,
            load_kg: 40.0,
            speed: 1.4,
            kind,
            surface: None,
            entity: Some(walker),
        });
        app.update();
    }

    fn benders(app: &mut App) -> Vec<(DVec3, f32)> {
        app.world
            .query::<(&GrassBender, &GlobalTransform, &WalkerBender)>()
            .iter(&app.world)
            .map(|(b, g, _)| (g.translation(), b.radius))
            .collect()
    }

    #[test]
    fn a_walker_gets_one_bender_at_its_feet_that_follows_it() {
        let mut app = app();
        // A character whose origin is at its middle, 0.9 m above the soles.
        let walker = app
            .world
            .spawn(Transform::from_xyz(10.0, 5.9, -3.0).with_rotation(Quat::from_rotation_y(1.0)))
            .id();
        app.update();
        for side in [-0.12, 0.12, -0.12, 0.12] {
            step(
                &mut app,
                walker,
                DVec3::new(10.0 + side, 5.0, -3.0),
                ContactKind::Foot,
            );
        }
        app.update();
        let b = benders(&mut app);
        assert_eq!(b.len(), 1, "{b:?}");
        let (p, r) = b[0];
        assert!((p.x - 10.0).abs() < 1e-3 && (p.z + 3.0).abs() < 1e-3, "{p}");
        // Centered a little above the soles, so the blades under the body lie down.
        assert!(p.y > 5.1 && p.y < 5.5, "{p}");
        assert!((r - 0.45).abs() < 1e-6);

        // It rides along: walk 3 m and jump.
        app.world.get_mut::<Transform>(walker).unwrap().translation += DVec3::new(3.0, 1.5, 0.0);
        app.update_n(2);
        let (p, _) = benders(&mut app)[0];
        assert!((p.x - 13.0).abs() < 1e-3 && p.y > 6.6, "{p}");

        // And goes with it.
        app.world.entity_mut(walker).despawn();
        app.update();
        assert!(benders(&mut app).is_empty());
    }

    #[test]
    fn a_walker_leaves_a_wake_that_stands_back_up() {
        let mut app = app();
        let walker = app.world.spawn(Transform::from_xyz(0.0, 0.9, 0.0)).id();
        app.update();
        step(
            &mut app,
            walker,
            DVec3::new(0.0, 0.0, 0.0),
            ContactKind::Foot,
        );
        let wakes = |app: &mut App| -> Vec<(DVec3, f32)> {
            app.world
                .query::<(&GrassBender, &GlobalTransform, &GrassWake)>()
                .iter(&app.world)
                .map(|(b, g, _)| (g.translation(), b.radius))
                .collect()
        };
        // Walk 3 m along +X at about walking pace.
        for i in 1..=30 {
            app.world
                .get_mut::<Transform>(walker)
                .unwrap()
                .translation
                .x = i as f64 * 0.1;
            app.update_n(4);
        }
        let w = wakes(&mut app);
        assert_eq!(w.len(), WAKES_PER_WALKER, "{w:?}");
        // Behind the walker, at its feet, and already shrinking.
        assert!(
            w.iter().all(|(p, r)| p.x < 3.0 && p.x > 1.0 && *r < 0.45),
            "{w:?}"
        );
        // Standing still, the grass behind stands back up.
        app.update_n(120);
        assert!(wakes(&mut app).is_empty());
    }

    #[test]
    fn a_car_gets_a_bender_per_wheel_and_far_contacts_are_ignored() {
        let mut app = app();
        let car = app.world.spawn(Transform::from_xyz(0.0, 0.6, 0.0)).id();
        app.update();
        let wheels = [(-0.8, -1.3), (0.8, -1.3), (-0.8, 1.3), (0.8, 1.3)];
        for _ in 0..3 {
            for (x, z) in wheels {
                step(&mut app, car, DVec3::new(x, 0.0, z), ContactKind::Wheel);
            }
        }
        // A contact 30 m away isn't this car's.
        step(
            &mut app,
            car,
            DVec3::new(30.0, 0.0, 0.0),
            ContactKind::Wheel,
        );
        app.update();
        let b = benders(&mut app);
        assert_eq!(b.len(), 4, "{b:?}");
        for (x, z) in wheels {
            assert!(
                b.iter()
                    .any(|(p, _)| (p.x - x).abs() < 1e-3 && (p.z - z).abs() < 1e-3),
                "{b:?}"
            );
        }
        // A body thrown on the ground presses nothing here.
        let rock = app.world.spawn(Transform::from_xyz(5.0, 0.3, 0.0)).id();
        step(&mut app, rock, DVec3::new(5.0, 0.0, 0.0), ContactKind::Body);
        assert_eq!(benders(&mut app).len(), 4);
    }
}
