//! Shared helpers for the movement integration tests: a deterministic app (60 Hz, manual
//! clock) with the world contracts, the movement module and its stand-in character physics.

#![allow(dead_code)]

use bevy_ecs::world::CommandQueue;
use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_movement::MovementPlugin;
use engine_world::WorldPlugin;
use engine_world::animation::AnimationTrigger;
use engine_world::physics::{Collider, RigidBody};

pub const DT: f64 = 1.0 / 60.0;

/// Animation triggers seen so far (jump, slide, wallrun, mantle, double_jump…).
#[derive(Resource, Default)]
pub struct Triggers(pub Vec<(Entity, String)>);

fn collect(mut seen: ResMut<Triggers>, mut events: MessageReader<AnimationTrigger>) {
    seen.0
        .extend(events.read().map(|t| (t.entity, t.name.clone())));
}

pub fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin).add_plugins(MovementPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: DT });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.init_resource::<Triggers>();
    app.add_systems(Last, collect);
    app
}

/// Run `secs` real seconds.
pub fn run(app: &mut App, secs: f64) {
    app.update_n((secs / DT).round() as usize);
}

/// Run until `done` is true or `max_secs` real seconds pass; returns the seconds run.
pub fn run_until(app: &mut App, max_secs: f64, mut done: impl FnMut(&mut World) -> bool) -> f64 {
    let mut t = 0.0;
    while t < max_secs {
        app.update();
        t += DT;
        if done(&mut app.world) {
            return t;
        }
    }
    t
}

/// A static box (center and full size in meters).
pub fn block(app: &mut App, center: DVec3, size: Vec3) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(center),
            RigidBody::fixed(),
            Collider::cuboid(size * 0.5),
        ))
        .id()
}

/// Run something that spawns through `Commands` (like `demo::spawn_demo`).
pub fn with_commands(app: &mut App, f: impl FnOnce(&mut Commands)) {
    let mut queue = CommandQueue::default();
    let mut commands = Commands::new(&mut queue, &app.world);
    f(&mut commands);
    queue.apply(&mut app.world);
}

pub fn triggered(app: &App, e: Entity, name: &str) -> usize {
    app.world
        .resource::<Triggers>()
        .0
        .iter()
        .filter(|(who, n)| *who == e && n == name)
        .count()
}
