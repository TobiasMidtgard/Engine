//! Growth look-development: plants grown from growth graphs, rendered headless into PNGs.
//!
//! ```sh
//! cargo run -p engine_foliage --example lookdev_growth --release -- /tmp/lookdev_growth [shot..]
//! ```
//!
//! Shots: `strawberry_row` (one strawberry at eight ages side by side, seedling to ripe
//! berries and runners), `strawberry` (a grown plant up close), `berries` (the fruit up
//! close), `sapling_row` (a sapling at five ages over three years), `sapling` (the three-year-old), `film` (frames of one
//! strawberry growing, `film/NNNNN.png`; `LOOKDEV_FRAMES` frames, default 120, over 130 days),
//! `oak_row` (an oak at six ages, 2 to 24 years, in midsummer), `oak` (the 24-year-old, and from
//! under its crown), `oak_seasons` (a 20-year-old in spring, summer, autumn and winter) and
//! `oak_film` (an oak growing from an acorn for 25 years, `oak_film/NNNNN.png`, 300 frames).
//! 1280x720 unless `LOOKDEV_SIZE=WxH`.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_foliage::growth::GrowingPlant;
use engine_render::prelude::*;
use engine_render::{Exposure, Screenshots};
use engine_world::WeatherState;

const STEP: f64 = 1.0 / 30.0;

fn aim(world: &mut World, eye: DVec3, target: DVec3) {
    let mut q = world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(world) {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

fn set_sun(world: &mut World, to_sun: Vec3) {
    let dir = to_sun.normalize();
    world.resource_mut::<WeatherState>().sun_direction = dir;
    let mut q = world.query_filtered::<&mut Transform, With<DirectionalLight>>();
    for mut t in q.iter_mut(world) {
        let mut sun = Transform::default();
        sun.look_to(-dir, Vec3::Y);
        *t = sun;
    }
}

fn capture(app: &mut App, path: &str, settle: usize) {
    app.update_n(settle);
    app.world
        .resource_mut::<Screenshots>()
        .request(path.to_string());
    app.update();
    app.update();
    println!("saved {path}");
}

fn shot(app: &mut App, out: &str, name: &str, eye: DVec3, target: DVec3, to_sun: Vec3) {
    aim(&mut app.world, eye, target);
    set_sun(&mut app.world, to_sun);
    capture(app, &format!("{out}/{name}.png"), 14);
}

fn spawn(app: &mut App, at: DVec3, preset: &str, seed: u64, age: f32) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            GrowingPlant::new(preset, seed).aged(age),
        ))
        .id()
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/lookdev_growth".into());
    let only: Vec<String> = args.collect();
    let wanted = |name: &str| only.is_empty() || only.iter().any(|o| o == name);
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let size = std::env::var("LOOKDEV_SIZE").ok();
        let (width, height) = size
            .as_deref()
            .and_then(|s| s.split_once('x'))
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .unwrap_or((1280, 720));
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(engine_foliage::FoliagePlugin);
    app.world
        .insert_resource(engine_foliage::FoliageLoading::in_frame());
    app.world.resource_mut::<WeatherState>().wind = Vec3::new(1.0, 0.0, 0.4);
    app.world.resource_mut::<Atmosphere>().ground_color = [0.12, 0.09, 0.06];
    // Garden soil.
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.13, 0.095, 0.07)
                .with_roughness(0.97)
                .with_detail(0.8, 0.02),
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
                fov_y_deg: 40.0,
                near: 0.02,
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
    let sun = Vec3::new(0.6, 0.8, 0.45);

    // Strawberries at eight ages in a row (x), each a meter apart.
    let ages = [8.0, 22.0, 38.0, 52.0, 66.0, 82.0, 100.0, 125.0];
    let row = DVec3::new(0.0, 0.0, 0.0);
    for (i, &age) in ages.iter().enumerate() {
        spawn(
            &mut app,
            row + DVec3::new(i as f64, 0.0, 0.0),
            "strawberry",
            4,
            age,
        );
    }
    // Saplings at five ages, 6 m apart, 50 m away.
    let trees = DVec3::new(0.0, 0.0, 50.0);
    for (i, &age) in [60.0, 150.0, 480.0, 880.0, 1050.0].iter().enumerate() {
        spawn(
            &mut app,
            trees + DVec3::new(i as f64 * 6.0, 0.0, 0.0),
            "sapling",
            9,
            age,
        );
    }
    // A plant for the film, on its own.
    let film_at = DVec3::new(0.0, 0.0, -40.0);
    // Oaks grown from the light-seeking tree node: six ages in midsummer, 16 m apart, and one
    // 20-year-old tree in each season, far off on their own.
    let oaks = DVec3::new(0.0, 0.0, 150.0);
    let oak_years = [2.0, 5.0, 9.0, 14.0, 19.0, 24.0];
    if wanted("oak_row") || wanted("oak") {
        for (i, &y) in oak_years.iter().enumerate() {
            spawn(
                &mut app,
                oaks + DVec3::new(i as f64 * 16.0, 0.0, 0.0),
                "oak",
                3,
                y * 365.0 + 200.0,
            );
        }
    }
    let seasons = DVec3::new(0.0, 0.0, 300.0);
    if wanted("oak_seasons") {
        for (i, &day) in [140.0, 200.0, 304.0, 345.0].iter().enumerate() {
            spawn(
                &mut app,
                seasons + DVec3::new(i as f64 * 16.0, 0.0, 0.0),
                "oak",
                3,
                19.0 * 365.0 + day,
            );
        }
    }

    if wanted("strawberry_row") {
        shot(
            &mut app,
            &out,
            "strawberry_row",
            row + DVec3::new(3.5, 1.5, 3.4),
            row + DVec3::new(3.5, 0.0, 0.0),
            sun,
        );
    }
    if wanted("strawberry") {
        let p = row + DVec3::new(6.0, 0.0, 0.0);
        shot(
            &mut app,
            &out,
            "strawberry",
            p + DVec3::new(0.35, 0.42, 0.55),
            p + DVec3::new(0.0, 0.06, 0.0),
            sun,
        );
    }
    if wanted("berries") {
        let p = row + DVec3::new(7.0, 0.0, 0.0);
        shot(
            &mut app,
            &out,
            "berries",
            p + DVec3::new(0.18, 0.2, 0.3),
            p + DVec3::new(0.0, 0.05, 0.05),
            sun,
        );
    }
    if wanted("sapling_row") {
        shot(
            &mut app,
            &out,
            "sapling_row",
            trees + DVec3::new(12.0, 2.2, 22.0),
            trees + DVec3::new(12.0, 1.5, 0.0),
            sun,
        );
    }
    if wanted("sapling") {
        let p = trees + DVec3::new(24.0, 0.0, 0.0);
        shot(
            &mut app,
            &out,
            "sapling",
            p + DVec3::new(1.6, 1.5, 3.6),
            p + DVec3::new(0.0, 1.3, 0.0),
            sun,
        );
    }
    if wanted("oak_row") {
        shot(
            &mut app,
            &out,
            "oak_row",
            oaks + DVec3::new(40.0, 7.0, 72.0),
            oaks + DVec3::new(40.0, 5.0, 0.0),
            sun,
        );
    }
    if wanted("oak") {
        let p = oaks + DVec3::new(80.0, 0.0, 0.0);
        shot(
            &mut app,
            &out,
            "oak",
            p + DVec3::new(9.0, 3.0, 22.0),
            p + DVec3::new(0.0, 6.5, 0.0),
            sun,
        );
        shot(
            &mut app,
            &out,
            "oak_under",
            p + DVec3::new(2.5, 1.6, 4.0),
            p + DVec3::new(0.0, 5.0, 0.0),
            sun,
        );
    }
    if wanted("oak_seasons") {
        shot(
            &mut app,
            &out,
            "oak_seasons",
            seasons + DVec3::new(24.0, 6.0, 52.0),
            seasons + DVec3::new(24.0, 5.0, 0.0),
            sun,
        );
    }
    if wanted("oak_film") {
        let frames: u32 = std::env::var("LOOKDEV_FRAMES")
            .ok()
            .and_then(|f| f.parse().ok())
            .unwrap_or(300);
        let at = DVec3::new(0.0, 0.0, -400.0);
        let plant = spawn(&mut app, at, "oak", 3, 0.0);
        std::fs::create_dir_all(format!("{out}/oak_film")).expect("film directory");
        set_sun(&mut app.world, sun);
        for f in 0..frames {
            let u = f as f64 / (frames.max(2) - 1) as f64;
            let age = (60.0 + (24.0 * 365.0 + 240.0) * u) as f32;
            app.world.get_mut::<GrowingPlant>(plant).unwrap().age_days = age;
            // The camera backs away and rises as the tree grows, circling a quarter turn.
            let h = (age / 365.0 * 0.58 + 0.4) as f64;
            let a = 0.4 + 1.2 * u;
            let d = 1.2 + h * 1.9;
            let eye = at + DVec3::new(a.cos() * d, 0.3 + h * 0.35, a.sin() * d);
            aim(&mut app.world, eye, at + DVec3::new(0.0, h * 0.5, 0.0));
            capture(
                &mut app,
                &format!("{out}/oak_film/{f:05}.png"),
                if f == 0 { 12 } else { 1 },
            );
        }
    }
    if wanted("film") {
        let frames: u32 = std::env::var("LOOKDEV_FRAMES")
            .ok()
            .and_then(|f| f.parse().ok())
            .unwrap_or(120);
        let plant = spawn(&mut app, film_at, "strawberry", 21, 0.0);
        std::fs::create_dir_all(format!("{out}/film")).expect("film directory");
        set_sun(&mut app.world, sun);
        for f in 0..frames {
            let u = f as f64 / (frames.max(2) - 1) as f64;
            let age = (2.0 + 128.0 * u) as f32;
            app.world.get_mut::<GrowingPlant>(plant).unwrap().age_days = age;
            // The camera backs off a little as the plant spreads, circling slowly.
            let a = 0.6 + 0.5 * u;
            let d = 0.45 + 0.35 * u;
            let eye = film_at + DVec3::new(a.cos() * d, 0.22 + 0.25 * u, a.sin() * d);
            aim(&mut app.world, eye, film_at + DVec3::new(0.0, 0.05, 0.0));
            capture(
                &mut app,
                &format!("{out}/film/{f:05}.png"),
                if f == 0 { 12 } else { 1 },
            );
        }
    }
}
