//! A zone's herd asks for bodies all at once (the meadow: 25 animals). Their meshes take
//! seconds each to build, so in a game they are made on workers, one build for each mesh
//! however many animals share it, and each body goes on when its mesh is in. Animals that
//! differ only in coat shade share a mesh, shaded by their material.

use engine_anim::{AnimationPlugin, Body, SkinnedMeshRenderer};
use engine_core::off_frame::{OffFrameJobs, finish};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::{AnimationParams, BodyRequest};

#[test]
fn a_herd_gets_its_meshes_built_off_the_frame_once_each() {
    let mut app = App::new();
    // As a game runs (headless runs build at once).
    app.world.resource_mut::<EngineConfig>().headless = false;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    app.update();
    // Four rabbits (the seed shades each coat differently) and a fawn.
    let mut asked = Vec::new();
    for (kind, height, seed) in [
        ("rabbit", None, 1),
        ("rabbit", None, 2),
        ("rabbit", None, 3),
        ("rabbit", None, 4),
        ("deer", Some(0.6), 5),
    ] {
        let t = Transform::from_xyz(asked.len() as f64 * 2.0, 0.0, 0.0);
        let mut r = BodyRequest::new(kind).with_seed(seed);
        r.height = height;
        asked.push(
            app.world
                .spawn((t, GlobalTransform::from(t), r, AnimationParams::default()))
                .id(),
        );
    }
    app.update();
    let bodies = |app: &mut App| app.world.query::<&Body>().iter(&app.world).count();
    // Nothing built on the frame: two builds queued, one for the rabbits and one for the fawn.
    assert_eq!(app.world.resource::<OffFrameJobs>().len(), 2);
    assert_eq!(bodies(&mut app), 0);
    finish(&mut app.world, None);
    assert_eq!(bodies(&mut app), asked.len());
    // The rabbits share one mesh, each shaded by its own material.
    let rabbits: Vec<&SkinnedMeshRenderer> = asked[..4]
        .iter()
        .map(|&e| app.world.get::<SkinnedMeshRenderer>(e).unwrap())
        .collect();
    assert!(rabbits.iter().all(|r| r.mesh == rabbits[0].mesh));
    let tints: Vec<f32> = rabbits.iter().map(|r| r.material.tint[0]).collect();
    assert!(
        tints.iter().any(|&t| (t - tints[0]).abs() > 1e-3),
        "tints {tints:?}"
    );
}
