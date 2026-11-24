//! The forest's part of the frame's ray scene (`SceneryFeature::ray_scene`, the ray-traced
//! lighting round's step 5): rocks in reach are solid instances, small far ones stay out, and
//! a forest floor is ground; a rock that leaves the world leaves the rays. Needs a GPU with
//! ray queries (lavapipe has them); passes with a note without.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_foliage::FoliagePlugin;
use engine_foliage::scenery::{GroundLayer, GroundSurface, GroundVertex, Rock};
use engine_render::RenderDisabled;
use engine_render::prelude::*;
use engine_render::settings::trial_cut_more::{RAY_WORLD, RAYS};

fn diag(app: &App, name: &str) -> f64 {
    app.world
        .resource::<Diagnostics>()
        .get(name)
        .unwrap_or(-1.0)
}

fn app() -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    // No plants: baking the library's species would take most of the run.
    app.world.insert_resource(engine_foliage::PlantLibrary {
        species: Vec::new(),
    });
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 96;
        w.height = 64;
    }
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(FoliagePlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.probe_gi = true;
        s.gi_hardware_rays = true;
        s.gi_probes_per_frame = 16;
        // The scene alone: the round's other ray parts stay as they were off, whatever the
        // defaults turn on.
        s.trial_cuts_more = (s.trial_cuts_more & !RAYS) | RAY_WORLD;
    }
    app.world
        .insert_resource(engine_foliage::FoliageLoading::in_frame());
    app
}

fn rock(w: &mut World, at: DVec3, size: f64) -> Entity {
    w.spawn((
        Rock {
            settled: true,
            ..Default::default()
        },
        Transform::from_translation(at).with_scale(Vec3::splat(size as f32)),
    ))
    .id()
}

/// A flat 40 m square of humus around `at`, as a cover would build it.
fn floor(w: &mut World, at: DVec3) -> Entity {
    let mut vertices = Vec::new();
    for z in 0..=4 {
        for x in 0..=4 {
            let mut weights = [0u8; 8];
            weights[0] = 255;
            vertices.push(GroundVertex {
                position: [x as f32 * 10.0 - 20.0, 0.0, z as f32 * 10.0 - 20.0],
                normal: [0.0, 1.0, 0.0],
                weights,
                extra: [0; 4],
                tint: [128; 4],
            });
        }
    }
    let mut indices = Vec::new();
    for z in 0..4u32 {
        for x in 0..4u32 {
            let i = z * 5 + x;
            indices.extend_from_slice(&[i, i + 5, i + 1, i + 1, i + 5, i + 6]);
        }
    }
    w.spawn((
        GroundSurface {
            vertices: Arc::new(vertices),
            indices: Arc::new(indices),
            slots: [GroundLayer::Humus; 8],
            version: 1,
            built_from: Default::default(),
        },
        Transform::from_translation(at),
    ))
    .id()
}

#[test]
fn rocks_and_the_forest_floor_are_in_the_rays() {
    let mut app = app();
    let w = &mut app.world;
    w.spawn((
        Camera::default(),
        Transform::from_xyz(0.0, 2.0, 0.0).looking_at(DVec3::new(0.0, 0.0, -20.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight {
            illuminance: 50_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.4, -0.8, -0.5), Vec3::Y),
    ));
    // Near: in. 300 m out, a pebble-sized one is too small for its distance; a boulder is not.
    let near = rock(w, DVec3::new(0.0, 0.0, -10.0), 2.0);
    rock(w, DVec3::new(0.0, 0.0, -300.0), 0.6);
    rock(w, DVec3::new(30.0, 0.0, -300.0), 10.0);
    floor(w, DVec3::new(0.0, 0.0, -10.0));
    app.update_n(6);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    if diag(&app, "render.ray.supported") != 1.0 {
        eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
        return;
    }
    assert_eq!(diag(&app, "render.ray.on"), 1.0, "no ray scene");
    assert_eq!(
        diag(&app, "render.ray.class_solid"),
        2.0,
        "rocks in the rays"
    );
    assert_eq!(
        diag(&app, "render.ray.class_ground"),
        1.0,
        "the floor in the rays"
    );
    // A rock taken away leaves the rays.
    app.world.despawn(near);
    app.update_n(3);
    assert_eq!(
        diag(&app, "render.ray.class_solid"),
        1.0,
        "a gone rock stayed"
    );
}
