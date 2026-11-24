//! The water in the renderer's ray scene (`trial_cut_more::RAY_WORLD`): each body within the
//! far reach is one `Water` instance (a disc, a square, or the sea's square that follows the
//! camera a cell at a time), a body beyond it is not, and one that goes leaves the scene.
//! Needs a GPU with ray queries (lavapipe has them); passes with a note without.

use engine_core::prelude::*;
use engine_render::RenderDisabled;
use engine_render::prelude::*;
use engine_render::settings::trial_cut_more::{RAY_WORLD, RAYS};
use engine_water::{WaterBody, WaterPlugin};
use engine_world::WorldPlugin;

fn diag(app: &App, name: &str) -> f64 {
    app.world
        .resource::<Diagnostics>()
        .get(name)
        .unwrap_or(-1.0)
}

fn water_instances(app: &App) -> f64 {
    diag(app, "render.ray.class_water")
}

#[test]
fn each_body_in_reach_is_one_water_instance() {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 160;
        w.height = 120;
    }
    app.add_plugins(RenderPlugin);
    app.add_plugins((WorldPlugin, WaterPlugin));
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.probe_gi = true;
        s.gi_hardware_rays = true;
        s.gi_probes_per_frame = 16;
        // The scene alone, whatever part of the ray-traced lighting is on by default.
        s.trial_cuts_more = (s.trial_cuts_more & !RAYS) | RAY_WORLD;
    }
    let w = &mut app.world;
    w.spawn((
        Camera::perspective(70.0),
        Transform::from_xyz(0.0, 6.0, 0.0).looking_at(DVec3::new(0.0, 0.0, -30.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -1.0, -0.4), Vec3::Y),
    ));
    // In reach: a lake ahead, a pool behind the camera (probes look behind it too), the sea.
    let lake = w
        .spawn((Transform::from_xyz(0.0, 0.0, -40.0), WaterBody::lake(20.0)))
        .id();
    w.spawn((
        Transform::from_xyz(0.0, 1.0, 30.0),
        WaterBody::pool(4.0, 2.0, 1.5),
    ));
    w.spawn((Transform::from_xyz(0.0, -2.0, 0.0), WaterBody::ocean()));
    // Out of reach (384 m from its shore).
    w.spawn((Transform::from_xyz(900.0, 0.0, 0.0), WaterBody::lake(30.0)));
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
    assert_eq!(water_instances(&app), 3.0, "the lake, the pool and the sea");

    app.world.despawn(lake);
    app.update_n(3);
    assert_eq!(water_instances(&app), 2.0, "the lake left");
}
