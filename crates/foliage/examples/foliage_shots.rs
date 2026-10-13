//! Render the foliage showcase from several viewpoints into PNGs, headless:
//!
//! ```sh
//! cargo run -p engine_foliage --example foliage_shots --release -- screenshots/foliage
//! ```
//!
//! Grows the showcase forest and mushroom cave, then photographs them from the demo's
//! viewpoints: the old forest, the trail, the clear-cut, a tree mid-fall and after it lands, the
//! spring, the burn scar, the dry ridge, the clearing ten years later, and the cave.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_foliage::demo;
use engine_foliage::{FoliagePlugin, Plant, PlantLibrary, PlantStage, fell_tree};
use engine_render::prelude::*;
use engine_render::{Exposure, RenderDisabled, Screenshots, SkyLightVolume};
use engine_world::WeatherState;

const FOREST: DVec3 = DVec3::new(0.0, 0.0, 0.0);
const CAVE: DVec3 = DVec3::new(400.0, 0.0, 0.0);
const STEP: f64 = 1.0 / 15.0;

fn sun_direction(hour: f32) -> Vec3 {
    let theta = (hour - 12.0) / 24.0 * std::f32::consts::TAU;
    Vec3::new(theta.sin() * 0.9, theta.cos() * 0.8, -0.35).normalize()
}

fn set_hour(world: &mut World, hour: f32) {
    let dir = sun_direction(hour);
    world.resource_mut::<WeatherState>().sun_direction = dir;
    world.resource_mut::<WeatherState>().time_of_day_h = hour;
    let mut q = world.query_filtered::<&mut Transform, With<DirectionalLight>>();
    for mut t in q.iter_mut(world) {
        let mut sun = Transform::default();
        sun.look_to(-dir, Vec3::Y);
        *t = sun;
    }
}

fn aim(world: &mut World, eye: DVec3, target: DVec3) {
    let mut q = world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(world) {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

fn shot(app: &mut App, out: &str, name: &str, eye: DVec3, target: DVec3, hour: f32) {
    aim(&mut app.world, eye, target);
    set_hour(&mut app.world, hour);
    app.update_n(8);
    let path = format!("{out}/{name}.png");
    app.world
        .resource_mut::<Screenshots>()
        .request(path.clone());
    app.update();
    println!("saved {path}");
}

/// Run frames without drawing them (software rendering is the slow part here).
fn simulate(app: &mut App, frames: usize) {
    let renderer = app.world.remove_resource::<engine_render::Renderer>();
    app.world
        .insert_resource(RenderDisabled("simulating only".into()));
    app.update_n(frames);
    app.world.remove_resource::<RenderDisabled>();
    if let Some(r) = renderer {
        app.world.insert_resource(r);
    }
}

/// A simple cave around the cave demo (the showcase builds its own).
fn cave_walls(commands: &mut Commands, c: DVec3) {
    let rock = StandardMaterial::hex(0x3e3b39).with_roughness(0.9);
    let (w, h, d) = (30.0f32, 9.0f32, 34.0f32);
    let mut wall = |size: [f32; 3], at: DVec3| {
        commands.spawn((
            Transform::from_translation(at),
            MeshRenderer::new(Shape::Cuboid { size }, rock),
        ));
    };
    wall(
        [w, h, 2.0],
        c + DVec3::new(0.0, h as f64 * 0.5, d as f64 * 0.5),
    );
    wall(
        [2.0, h, d],
        c + DVec3::new(-w as f64 * 0.5, h as f64 * 0.5, 0.0),
    );
    wall(
        [2.0, h, d],
        c + DVec3::new(w as f64 * 0.5, h as f64 * 0.5, 0.0),
    );
    wall(
        [w + 24.0, 5.0, d + 16.0],
        c + DVec3::new(0.0, h as f64 + 2.5, 4.0),
    );
    for x in [-1.0, 1.0] {
        wall(
            [10.0, h, 2.0],
            c + DVec3::new(x * 10.0, h as f64 * 0.5, -d as f64 * 0.5),
        );
    }
    commands.spawn((
        Transform::from_translation(c + DVec3::new(0.0, h as f64 * 0.5, 0.0)),
        SkyLightVolume::new([w * 0.5 - 1.0, h * 0.5, d * 0.5 - 1.0])
            .open(BoxFace::NegZ)
            .with_fade(12.0),
    ));
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "screenshots/foliage".into());
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1280;
        w.height = 720;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(FoliagePlugin);

    app.world.resource_mut::<Atmosphere>().ground_color = [0.09, 0.18, 0.04];
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [4000.0, 4000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55743a).with_roughness(0.95),
        ),
    ));
    app.world.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default(),
    ));
    app.world.spawn((
        Camera {
            projection: Projection::Perspective {
                fov_y_deg: 55.0,
                near: 0.1,
            },
            ..Default::default()
        },
        Transform::default(),
    ));
    let mut cams = app.world.query::<&mut Camera>();
    for mut cam in cams.iter_mut(&mut app.world) {
        if let Exposure::Auto { speed, .. } = &mut cam.exposure {
            *speed = 40.0;
        }
    }
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, FOREST);
        demo::spawn_cave_demo(&mut commands, CAVE);
        cave_walls(&mut commands, CAVE);
    }
    app.world.flush();
    // The demo fells a tree every 25 s; this tool fells its own.
    let mut felling = app.world.query::<&mut demo::DemoFelling>();
    for mut d in felling.iter_mut(&mut app.world) {
        d.max_trees = 0;
    }
    let started = std::time::Instant::now();
    app.update();
    println!(
        "grew the forest and cave in {:.1} s",
        started.elapsed().as_secs_f32()
    );
    let stats =
        engine_core::registry::call_api(&mut app.world, "foliage.stats", serde_json::json!({}));
    println!("{}", serde_json::to_string_pretty(&stats.unwrap()).unwrap());

    let forest = demo::forest_viewpoints(FOREST);
    let cave = demo::cave_viewpoints(CAVE);
    let take = |app: &mut App, file: &str, v: &demo::Viewpoint| {
        shot(app, &out, file, v.eye, v.target, v.hour)
    };
    let by_name = |name: &str| *forest.iter().find(|v| v.name == name).expect("viewpoint");
    take(&mut app, "01_forest", &by_name("forest_overview"));
    take(&mut app, "02_trail", &by_name("forest_trail"));
    take(&mut app, "03_clearing", &by_name("forest_clearing"));

    // From the clearing viewpoint, fell a big tree ahead across the view and photograph it.
    let library = app.world.resource::<PlantLibrary>().clone();
    let view = by_name("forest_clearing");
    let fwd = (view.target - view.eye).with_y(0.0).normalize();
    let mut q = app.world.query::<(Entity, &Plant, &Transform)>();
    let tree = q
        .iter(&app.world)
        .filter(|(_, p, t)| {
            let to = (t.translation - view.eye).with_y(0.0);
            p.stage == PlantStage::Alive
                && library.get(p.species).is_tree()
                && p.size > 0.6
                && (18.0..40.0).contains(&to.length())
                && to.normalize().dot(fwd) > 0.85
        })
        .max_by(|a, b| {
            let score =
                |p: &Plant, t: &Transform| p.size - 0.02 * t.translation.distance(view.eye) as f32;
            score(a.1, a.2).total_cmp(&score(b.1, b.2))
        })
        .map(|(e, _, t)| (e, t.translation));
    if let Some((tree, base)) = tree {
        // It falls sideways, toward the middle of the view.
        let to = (base - view.eye).with_y(0.0);
        let side = DVec3::Y.cross(fwd);
        let dir = if to.dot(side) > 0.0 { -side } else { side }.as_vec3();
        let look = base + DVec3::Y * 7.0 + dir.as_dvec3() * 5.0;
        aim(&mut app.world, view.eye, look);
        app.update_n(6);
        fell_tree(&mut app.world, tree, dir);
        // A 20 m tree takes about 3 s to fall.
        simulate(&mut app, 22);
        app.update_n(4);
        app.world
            .resource_mut::<Screenshots>()
            .request(format!("{out}/04_falling.png"));
        app.update();
        simulate(&mut app, 30);
        shot(
            &mut app,
            &out,
            "05_fallen",
            view.eye,
            look - DVec3::Y * 5.0,
            9.5,
        );
    }
    take(&mut app, "06_spring", &by_name("forest_spring"));
    take(&mut app, "07_burn_scar", &by_name("forest_burn_scar"));
    take(&mut app, "08_dry_ridge", &by_name("forest_dry_ridge"));

    // Ten more years in the clearing.
    let years = 10.0;
    let frames = 24;
    let region_scale = 30.0;
    app.world
        .resource_mut::<engine_foliage::EcologySettings>()
        .years_per_second = years / (frames as f32 * STEP as f32) / region_scale;
    simulate(&mut app, frames);
    app.world
        .resource_mut::<engine_foliage::EcologySettings>()
        .years_per_second = 0.0;
    take(
        &mut app,
        "09_clearing_10_years_later",
        &by_name("forest_clearing"),
    );

    take(&mut app, "10_cave", &cave[0]);
    take(&mut app, "11_cave_pool", &cave[1]);
    let stats =
        engine_core::registry::call_api(&mut app.world, "foliage.stats", serde_json::json!({}));
    println!("{}", serde_json::to_string_pretty(&stats.unwrap()).unwrap());
}
