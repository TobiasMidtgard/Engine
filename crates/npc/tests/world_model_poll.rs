//! The tactical world notices static colliders that moved without their collider changing
//! (a parent moved, a transform set by hand) within one poll period, though the poll looks at
//! only a slice of the colliders each time.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::NpcPlugin;
use engine_npc::world_model::TacticalWorld;
use engine_world::WorldPlugin;
use engine_world::physics::Collider;

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app
}

fn wall_at(i: usize) -> DVec3 {
    DVec3::new((i % 8) as f64 * 6.0, 1.0, (i / 8) as f64 * 6.0)
}

#[test]
fn walls_moved_by_hand_are_seen_within_a_poll_period() {
    let mut app = app();
    let walls: Vec<Entity> = (0..40)
        .map(|i| {
            app.world
                .spawn((
                    Transform::from_translation(wall_at(i)),
                    Collider::cuboid(Vec3::new(1.5, 1.0, 0.3)),
                ))
                .id()
        })
        .collect();
    for _ in 0..10 {
        app.update();
    }
    let near =
        |w: &TacticalWorld, at: DVec3| w.obstacles.iter().any(|o| (o.center - at).length() < 0.05);
    {
        let w = app.world.resource::<TacticalWorld>();
        assert!(
            (0..40).all(|i| near(w, wall_at(i))),
            "built with every wall"
        );
    }
    // Move every wall 2 m along z by its transform alone: no change to any collider.
    let lift = DVec3::new(0.0, 0.0, 2.0);
    for (i, &e) in walls.iter().enumerate() {
        app.world.get_mut::<Transform>(e).unwrap().translation = wall_at(i) + lift;
    }
    // A little over one poll period (1 s) at 60 frames a second.
    for _ in 0..75 {
        app.update();
    }
    let w = app.world.resource::<TacticalWorld>();
    let missed: Vec<usize> = (0..40).filter(|&i| !near(w, wall_at(i) + lift)).collect();
    assert!(
        missed.is_empty(),
        "walls still at their old place: {missed:?}"
    );
    assert!(
        (0..40).all(|i| !near(w, wall_at(i))),
        "old places forgotten"
    );
}
