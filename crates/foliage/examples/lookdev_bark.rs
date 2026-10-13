//! Bark and wood look-development: close-ups of oak, birch, pine, spruce and hazel trunks, a
//! fresh stump and a felled log, rendered headless into PNGs.
//!
//! ```sh
//! cargo run -p engine_foliage --example lookdev_bark --release -- /tmp/lookdev_bark [width height] [shots...]
//! cargo run -p engine_foliage --example lookdev_bark --release -- --dump /tmp/lookdev_bark/textures
//! ```
//!
//! `shots` picks shots by name (`oak`, `birch`, `pine`, `spruce`, `hazel`, `stump`, `log`,
//! `row`, `close` for arm's-length oak, birch and pine); `--dump` saves the painted bark
//! textures instead.
//!
//! The sun rakes across the trunks from the side, which is the light that shows (or betrays)
//! bark relief. Every shot is a close-up a player could walk up to.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_foliage::meshgen::bark::{BarkKind, bark_textures};
use engine_foliage::{FelledLog, FoliagePlugin, Genes, LogPose, Plant, fell_tree, spawn_plant};
use engine_render::prelude::*;
use engine_render::{Exposure, RenderDisabled, Screenshots};
use engine_world::WeatherState;

const STEP: f64 = 1.0 / 15.0;

/// Direction toward the sun: low in the east-north-east, so it grazes the trunks' camera side.
fn sun_toward() -> Vec3 {
    Vec3::new(0.82, 0.42, 0.40).normalize()
}

fn aim(world: &mut World, eye: DVec3, target: DVec3) {
    let mut q = world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(world) {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

fn shot(app: &mut App, out: &str, name: &str, eye: DVec3, target: DVec3) {
    aim(&mut app.world, eye, target);
    app.update_n(5);
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

fn genes(variant: u8) -> Genes {
    Genes {
        variant,
        lean: 0.0,
        ..Default::default()
    }
}

/// Save every bark texture set as PNGs (`<kind>_albedo.png`, `_normal.png`, `_orm.png`).
fn dump_textures(out: &str) {
    std::fs::create_dir_all(out).expect("output directory");
    for kind in BarkKind::ALL {
        let started = std::time::Instant::now();
        let t = bark_textures(kind);
        println!(
            "{kind:?}: painted in {:.2} s, {:.1} MB on the GPU, mean albedo {:?}",
            started.elapsed().as_secs_f32(),
            t.gpu_bytes() as f32 / (1 << 20) as f32,
            t.mean_albedo
        );
        for (name, img) in [
            ("albedo", &t.albedo),
            ("normal", &t.normal),
            ("orm", &t.orm),
        ] {
            let path = format!("{out}/{kind:?}_{name}.png").to_lowercase();
            image::RgbaImage::from_raw(img.width, img.height, img.data.clone())
                .expect("rgba8")
                .save(&path)
                .expect("png");
        }
    }
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/lookdev_bark".into());
    if out == "--dump" {
        dump_textures(
            &args
                .next()
                .unwrap_or_else(|| "/tmp/lookdev_bark/textures".into()),
        );
        return;
    }
    let width: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(640);
    let height: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(360);
    let only: Vec<String> = args.collect();
    let want = |name: &str| only.is_empty() || only.iter().any(|o| name.contains(o.as_str()));
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(FoliagePlugin);

    app.world.resource_mut::<Atmosphere>().ground_color = [0.07, 0.1, 0.04];
    {
        let sun = sun_toward();
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.sun_direction = sun;
        weather.time_of_day_h = 9.0;
    }
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x3d3426).with_roughness(0.97),
        ),
    ));
    let mut sun = Transform::default();
    sun.look_to(-sun_toward(), Vec3::Y);
    app.world.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        sun,
    ));
    app.world.spawn((
        Camera {
            projection: Projection::Perspective {
                fov_y_deg: 50.0,
                near: 0.05,
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

    // A row of mature trees 12 m apart, and an oak to fell behind them.
    let row = [
        ("oak", 0.0, 1u8),
        ("birch", 12.0, 0),
        ("pine", 24.0, 2),
        ("spruce", 36.0, 1),
        ("hazel", 48.0, 0),
    ];
    for (name, x, v) in row {
        spawn_plant(
            &mut app.world,
            name,
            DVec3::new(x, 0.0, 0.0),
            0.95,
            Some(genes(v)),
        )
        .expect("species");
    }
    let felled_at = DVec3::new(0.0, 0.0, -24.0);
    let to_fell = spawn_plant(&mut app.world, "oak", felled_at, 0.9, Some(genes(2))).unwrap();
    app.update();

    // Fell the second oak along +X and let it settle; age it until its twigs have rotted off.
    fell_tree(&mut app.world, to_fell, Vec3::X);
    simulate(&mut app, 70);
    let mut logs = app.world.query::<(&mut FelledLog, &Transform, &LogPose)>();
    let log = logs.iter_mut(&mut app.world).next().map(|(mut l, t, _)| {
        l.years = 2.0;
        *t
    });
    simulate(&mut app, 20);

    let trunk_eye = |x: f64, d: f64, h: f64| DVec3::new(x + d * 0.62, h, d * 0.78);
    if want("oak") {
        shot(
            &mut app,
            &out,
            "01_oak",
            trunk_eye(0.0, 2.6, 1.3),
            DVec3::new(0.0, 1.05, 0.0),
        );
    }
    if want("birch") {
        shot(
            &mut app,
            &out,
            "02_birch",
            trunk_eye(12.0, 1.9, 1.2),
            DVec3::new(12.0, 1.0, 0.0),
        );
    }
    if want("pine") {
        shot(
            &mut app,
            &out,
            "03_pine",
            trunk_eye(24.0, 2.2, 1.3),
            DVec3::new(24.0, 1.05, 0.0),
        );
    }
    if want("spruce") {
        shot(
            &mut app,
            &out,
            "04_spruce",
            trunk_eye(36.0, 2.2, 1.2),
            DVec3::new(36.0, 0.9, 0.0),
        );
    }
    if want("hazel") {
        shot(
            &mut app,
            &out,
            "05_hazel",
            trunk_eye(48.0, 2.4, 1.1),
            DVec3::new(48.0, 0.8, 0.0),
        );
    }
    if want("stump") {
        let stump = felled_at;
        shot(
            &mut app,
            &out,
            "06_stump",
            stump + DVec3::new(-0.9, 1.7, 1.5),
            stump + DVec3::new(0.0, 0.35, 0.0),
        );
    }
    if let (true, Some(t)) = (want("log"), log) {
        let axis = (t.rotation * Vec3::Y).as_dvec3();
        let cut = t.translation;
        let side = DVec3::Y.cross(axis).normalize();
        let butt = cut + axis * 0.6;
        shot(
            &mut app,
            &out,
            "07_log",
            butt - axis * 2.2 + side * 1.4 + DVec3::Y * 1.0,
            butt + axis * 0.9,
        );
    }
    if want("row") {
        shot(
            &mut app,
            &out,
            "08_row",
            DVec3::new(20.0, 1.7, 14.0),
            DVec3::new(20.0, 3.5, 0.0),
        );
    }
    if want("close") {
        // Arm's length, where the relief has to hold up.
        for (name, x) in [
            ("09_oak_close", 0.0),
            ("10_birch_close", 12.0),
            ("11_pine_close", 24.0),
        ] {
            shot(
                &mut app,
                &out,
                name,
                trunk_eye(x, 1.15, 1.45),
                DVec3::new(x, 1.3, 0.0),
            );
        }
    }
    let mut q = app.world.query::<&Plant>();
    println!("{} plants", q.iter(&app.world).count());
}
