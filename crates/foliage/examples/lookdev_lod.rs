//! A walk through a wood, headless, filmed frame by frame to judge how trees change their level
//! of detail as the camera moves (playtest #33, pop-in):
//!
//! ```sh
//! cargo run -p engine_foliage --example lookdev_lod --release -- /tmp/lookdev_lod [off]
//! ```
//!
//! Writes `NNNNN.png` frames of a walk at 8 m/s along an avenue of scenery trees (oak, birch,
//! pine, spruce, hazel) 6 m to 260 m away; `off` switches levels at once instead of
//! cross-fading them, for a before. 960x540 and 150 frames at 30 fps unless `LOOKDEV_SIZE=WxH`
//! or `LOOKDEV_FRAMES=N`.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_foliage::lod_fade::LodFades;
use engine_foliage::{FoliagePlugin, PlantLibrary, SceneryTree};
use engine_render::prelude::*;
use engine_render::{Exposure, Screenshots};
use engine_world::WeatherState;

const STEP: f64 = 1.0 / 30.0;
/// Walking speed, m/s.
const SPEED: f64 = 8.0;

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/lookdev_lod".into());
    let off = args.any(|a| a == "off");
    std::fs::create_dir_all(&out).expect("output directory");
    let frames: u32 = std::env::var("LOOKDEV_FRAMES")
        .ok()
        .and_then(|f| f.parse().ok())
        .unwrap_or(150);

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let size = std::env::var("LOOKDEV_SIZE").ok();
        let (width, height) = size
            .as_deref()
            .and_then(|s| s.split_once('x'))
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .unwrap_or((960, 540));
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
    app.world
        .insert_resource(engine_foliage::FoliageLoading::in_frame());
    app.world.resource_mut::<LodFades>().enabled = !off;
    app.world.resource_mut::<WeatherState>().wind = Vec3::new(1.5, 0.0, 0.6);
    app.world.resource_mut::<Atmosphere>().ground_color = [0.16, 0.2, 0.08];
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [4000.0, 4000.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.17, 0.22, 0.08)
                .with_roughness(0.95)
                .with_detail(0.8, 0.03),
        ),
    ));
    let to_sun = Vec3::new(0.5, 0.75, 0.45).normalize();
    app.world.resource_mut::<WeatherState>().sun_direction = to_sun;
    let mut sun = Transform::default();
    sun.look_to(-to_sun, Vec3::Y);
    app.world.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        sun,
    ));
    let mut camera = Camera {
        projection: Projection::Perspective {
            fov_y_deg: 55.0,
            near: 0.05,
        },
        ..Default::default()
    };
    if let Exposure::Auto { speed, .. } = &mut camera.exposure {
        *speed = 20.0;
    }
    app.world.spawn((camera, Transform::default()));

    // An avenue: trees either side of the walk along +z, every 9 m for 300 m, and a loose wood
    // beyond, from 6 m to 260 m off the path.
    let library = app.world.resource::<PlantLibrary>().clone();
    let species = ["oak", "birch", "pine", "spruce", "hazel", "oak"];
    let mut n = 0u64;
    let mut plant = |world: &mut World, at: DVec3| {
        n += 1;
        let name = species[(n as usize * 7 + 3) % species.len()];
        if let Some(tree) = SceneryTree::new(&library, name, n * 31 + 5) {
            world.spawn((Transform::from_translation(at), tree));
        }
    };
    let jitter =
        |k: u64| ((k.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40) as f64 / 16_777_216.0) - 0.5;
    for i in 0..34 {
        let z = i as f64 * 9.0;
        for side in [-1.0, 1.0] {
            plant(
                &mut app.world,
                DVec3::new(side * (6.0 + 2.0 * jitter(i * 2 + 1)), 0.0, z),
            );
        }
    }
    for i in 0..260u64 {
        let side = if i % 2 == 0 { -1.0 } else { 1.0 };
        let x = side * (14.0 + 246.0 * (jitter(i * 3 + 1) + 0.5).powi(2));
        let z = -40.0 + 380.0 * (jitter(i * 3 + 2) + 0.5);
        plant(&mut app.world, DVec3::new(x, 0.0, z));
    }

    let eye = |t: f64| DVec3::new(0.0, 1.7, -30.0 + SPEED * t);
    let look = |t: f64| eye(t) + DVec3::new(0.12, -0.04, 1.0);
    let aim = |world: &mut World, t: f64| {
        let mut q = world.query_filtered::<&mut Transform, With<Camera>>();
        for mut tr in q.iter_mut(world) {
            *tr = Transform::from_translation(eye(t)).looking_at(look(t), Vec3::Y);
        }
    };
    // Settle at the start: every tree picks its level, exposure adapts.
    aim(&mut app.world, 0.0);
    app.update_n(20);
    for f in 0..frames {
        aim(&mut app.world, f as f64 * STEP);
        let path = format!("{out}/{f:05}.png");
        app.world
            .resource_mut::<Screenshots>()
            .request(path.clone());
        app.update();
        if f % 30 == 0 {
            println!("saved {path}");
        }
    }
    // The last screenshot is written a frame later.
    app.update_n(2);
}
