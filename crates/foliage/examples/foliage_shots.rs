//! Render the foliage showcase from several viewpoints into PNGs, headless:
//!
//! ```sh
//! cargo run -p engine_foliage --example foliage_shots --release -- screenshots/foliage [shot..]
//! ```
//!
//! Name shots after the directory (by number or name: `06`, `burn_scar`) to take only those.
//!
//! Grows the showcase forest and mushroom cave, then photographs them from the demo's
//! viewpoints: the old forest, the trail, the clear-cut, a tree mid-fall and after it lands, the
//! spring, the burn scar, the dry ridge, the clearing ten years later, and the cave.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_foliage::demo;
use engine_foliage::scenery::random_rock;
use engine_foliage::{
    FallingTree, FelledLog, FoliagePlugin, Plant, PlantLibrary, PlantStage, fell_tree,
};
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

/// How far the tree being felled leans from upright, radians (`None` once it has settled).
fn fall_angle(app: &mut App) -> Option<f32> {
    let mut q = app.world.query::<&FallingTree>();
    q.iter(&app.world).next().map(|f| f.angle)
}

fn set_step(app: &mut App, step_secs: f64) {
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs });
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

/// A simple cave around the cave demo (the showcase builds its own): plain walls and a roof,
/// lined inside with crags so the walls read as rock.
fn cave_walls(commands: &mut Commands, c: DVec3) {
    let rock = StandardMaterial::hex(0x2c2a28).with_roughness(0.9);
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
        // Past the walls' inner faces, or the sky lights them (and the cracks between them).
        SkyLightVolume::new([w * 0.5 + 1.5, h * 0.5 + 0.5, d * 0.5 + 1.5])
            .open(BoxFace::NegZ)
            .with_fade(12.0),
    ));

    // Crags along the inside of the walls, just outside the cave floor's cover.
    let mut rng = Rng::new(0xc4a6);
    let (hx, hz) = (w * 0.5, d * 0.5);
    let mut runs: Vec<(Vec2, Vec2)> = vec![
        (Vec2::new(-hx, hz), Vec2::new(hx, hz)),
        (Vec2::new(-hx, -hz), Vec2::new(-hx, hz)),
        (Vec2::new(hx, -hz), Vec2::new(hx, hz)),
    ];
    for x in [-1.0, 1.0] {
        runs.push((Vec2::new(x * 5.5, -hz), Vec2::new(x * hx, -hz)));
    }
    let mut crags = Vec::new();
    for (a, b) in runs {
        let n = (a.distance(b) / 3.2).ceil() as usize;
        for i in 0..=n {
            let width = rng.range(6.0, 9.0);
            let mut r = random_rock(
                &mut rng,
                a.lerp(b, i as f32 / n as f32),
                width,
                false,
                0.0,
                [0.55, 0.53, 0.5],
            );
            r.size.y = rng.range(h * 0.45, h * 0.85);
            r.rock.bury_m = r.size.y * 0.15;
            r.rock.lichen = 0.1;
            // Outside the cover, rocks keep their place: sink them by hand.
            r.rock.settled = true;
            crags.push(r);
        }
    }
    let crags = crags.into_iter().map(move |r| {
        (
            Transform::from_translation(
                c + DVec3::new(r.pos.x as f64, -r.rock.bury_m as f64, r.pos.y as f64),
            )
            .with_rotation(r.rotation)
            .with_scale(r.size),
            r.rock,
        )
    });
    commands.spawn_batch(crags);
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "screenshots/foliage".into());
    std::fs::create_dir_all(&out).expect("output directory");
    let only: Vec<String> = std::env::args().skip(2).collect();
    let wanted = |file: &str| only.is_empty() || only.iter().any(|o| file.contains(o.as_str()));

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
        if wanted(file) {
            shot(app, &out, file, v.eye, v.target, v.hour)
        }
    };
    let by_name = |name: &str| *forest.iter().find(|v| v.name == name).expect("viewpoint");
    take(&mut app, "01_forest", &by_name("forest_overview"));
    take(&mut app, "02_trail", &by_name("forest_trail"));
    take(&mut app, "03_clearing", &by_name("forest_clearing"));

    // Fell a big tree at the edge of the clear-cut into the open, and photograph it coming
    // down from across the clearing.
    let library = app.world.resource::<PlantLibrary>().clone();
    let view = by_name("forest_clearing");
    let fwd = (view.target - view.eye).with_y(0.0).normalize();
    let clearing = FOREST + DVec3::new(demo::CLEARING[0] as f64, 0.0, demo::CLEARING[1] as f64);
    let mut q = app.world.query::<(Entity, &Plant, &Transform)>();
    // Trees between the camera and the falling one would hide it (bushes and saplings only
    // hide its foot); anything at the camera would fill the frame.
    let standing: Vec<(DVec3, f32, f64)> = q
        .iter(&app.world)
        .filter(|(_, p, _)| p.stage.is_alive())
        .map(|(_, p, t)| {
            let sp = library.get(p.species);
            (t.translation, p.height(sp), p.crown_radius(sp) as f64 + 0.3)
        })
        .collect();
    let in_view = |eye: DVec3, base: DVec3| {
        let to = (base - eye).with_y(0.0);
        let len = to.length();
        let dir = to / len;
        !standing.iter().any(|&(p, h, r)| {
            let rel = (p - eye).with_y(0.0);
            let along = rel.dot(dir);
            h > 6.0 && along > 1.0 && along < len - 4.0 && (rel - dir * along).length() < r
        })
    };
    let crowded = |eye: DVec3| {
        standing
            .iter()
            .any(|&(p, h, r)| h > 1.2 && (p - eye).with_y(0.0).length() < r + 1.0)
    };
    let candidates: Vec<(Entity, DVec3, f32)> = q
        .iter(&app.world)
        .filter(|(_, p, t)| {
            let edge = (t.translation - clearing).with_y(0.0).length();
            p.stage == PlantStage::Alive
                && library.get(p.species).is_tree()
                && p.size > 0.6
                && (demo::CLEARING[2] as f64 - 3.0..demo::CLEARING[2] as f64 + 3.0).contains(&edge)
        })
        .map(|(e, p, t)| (e, t.translation, p.size))
        .collect();
    let camera_for = |base: DVec3| {
        let outward = (base - clearing).with_y(0.0).normalize();
        (outward, (base - outward * 34.0).with_y(view.eye.y + 0.5))
    };
    let felled = candidates.into_iter().max_by(|a, b| {
        // Clear line of sight, nothing at the camera, a big tree, the morning sun as in the
        // clearing view.
        let score = |&(_, base, size): &(Entity, DVec3, f32)| {
            let (outward, eye) = camera_for(base);
            let clear = if in_view(eye, base) { 10.0 } else { 0.0 };
            let room = if crowded(eye) { 0.0 } else { 5.0 };
            clear + room + size + 2.0 * outward.dot(fwd) as f32
        };
        score(a).total_cmp(&score(b))
    });
    if let Some((tree, base, _)) = felled.filter(|_| wanted("04_falling") || wanted("05_fallen")) {
        let (outward, eye) = camera_for(base);
        println!(
            "felling a tree {:.0} m from the camera ({}, {})",
            (base - eye).with_y(0.0).length(),
            if in_view(eye, base) {
                "in plain view"
            } else {
                "partly hidden"
            },
            if crowded(eye) {
                "a plant at the camera"
            } else {
                "room at the camera"
            }
        );
        // Into the clearing, across the view and toward the camera.
        let dir = (DVec3::Y.cross(outward) - outward).normalize();
        let look = base + dir * 10.0 + DVec3::Y * 7.0;
        aim(&mut app.world, eye, look);
        set_hour(&mut app.world, view.hour);
        app.update_n(6);
        fell_tree(&mut app.world, tree, dir.as_vec3());
        // It tips slowly, then comes down fast: photograph it halfway, with time stopped so
        // the frame is sharp.
        let mut frames = 0;
        while fall_angle(&mut app).is_some_and(|a| a < 0.95) && frames < 300 {
            simulate(&mut app, 1);
            frames += 1;
        }
        println!(
            "after {:.1} s it leans {:.0} degrees",
            frames as f64 * STEP,
            fall_angle(&mut app).unwrap_or(0.0).to_degrees()
        );
        if wanted("04_falling") {
            set_step(&mut app, 0.0);
            app.update_n(6);
            app.world
                .resource_mut::<Screenshots>()
                .request(format!("{out}/04_falling.png"));
            app.update();
            println!("saved {out}/04_falling.png");
            set_step(&mut app, STEP);
        }
        // Down and settled: walk up to it, looking across the log from the clearing side.
        let length = {
            let mut q = app.world.query::<&FallingTree>();
            q.iter(&app.world).next().map_or(15.0, |f| f.length as f64)
        };
        while fall_angle(&mut app).is_some() && frames < 600 {
            simulate(&mut app, 1);
            frames += 1;
        }
        simulate(&mut app, 4);
        let mut logs = app.world.query::<(&FelledLog, &Transform)>();
        let lying = logs
            .iter(&app.world)
            .map(|(_, t)| (t.translation, (t.rotation * Vec3::Y).as_dvec3()))
            .min_by(|a, b| a.0.distance(base).total_cmp(&b.0.distance(base)));
        let (eye, target) = match lying {
            Some((butt, axis)) => {
                let mid = butt + axis * length * 0.45;
                let across = DVec3::Y.cross(axis).with_y(0.0).normalize();
                let toward_clearing = if (clearing - mid).dot(across) > 0.0 {
                    across
                } else {
                    -across
                };
                (
                    (mid + toward_clearing * 13.0).with_y(view.eye.y),
                    mid + DVec3::Y * 0.6,
                )
            }
            None => (eye, look - DVec3::Y * 5.0),
        };
        shot(&mut app, &out, "05_fallen", eye, target, view.hour);
    }
    take(&mut app, "06_spring", &by_name("forest_spring"));
    take(&mut app, "07_burn_scar", &by_name("forest_burn_scar"));
    take(&mut app, "08_dry_ridge", &by_name("forest_dry_ridge"));

    // Ten more years in the clearing.
    if wanted("09_clearing_10_years_later") {
        ten_years_later(&mut app, &out, &by_name("forest_clearing"));
    }

    take(&mut app, "10_cave", &cave[0]);
    take(&mut app, "11_cave_pool", &cave[1]);
    let stats =
        engine_core::registry::call_api(&mut app.world, "foliage.stats", serde_json::json!({}));
    println!("{}", serde_json::to_string_pretty(&stats.unwrap()).unwrap());
}

fn ten_years_later(app: &mut App, out: &str, v: &demo::Viewpoint) {
    let years = 10.0;
    let frames = 24;
    let region_scale = 30.0;
    app.world
        .resource_mut::<engine_foliage::EcologySettings>()
        .years_per_second = years / (frames as f32 * STEP as f32) / region_scale;
    simulate(app, frames);
    app.world
        .resource_mut::<engine_foliage::EcologySettings>()
        .years_per_second = 0.0;
    shot(
        app,
        out,
        "09_clearing_10_years_later",
        v.eye,
        v.target,
        v.hour,
    );
}
