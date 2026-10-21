//! Grass look-development: GPU grass fields of several biomes, rendered headless into PNGs.
//!
//! ```sh
//! cargo run -p engine_foliage --example lookdev_grass --release -- /tmp/lookdev_grass [shot..]
//! ```
//!
//! Renders 1280×720 (or `LOOKDEV_SIZE=WxH`). Shots: `meadow` (standing in a summer meadow),
//! `meadow_low` (knee height, the blades up close), `meadow_back` (looking toward a low sun
//! through the blades), `savanna` (tall dry tussocks), `tundra` (short, dense, olive and
//! russet), `marsh` (tall, upright, dark), `alien` (a low-gravity world's violet grass under a
//! paler sky). Each biome is its own field 1 km from the others, on ground of its own color.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{Exposure, GrassField, GrassStyle, Screenshots};
use engine_world::WeatherState;

const STEP: f64 = 1.0 / 30.0;

/// A biome's grass and where its field lies.
struct Biome {
    name: &'static str,
    x: f64,
    style: GrassStyle,
}

fn biomes() -> Vec<Biome> {
    let mut out = Vec::new();
    let mut add = |name, style| {
        let x = out.len() as f64 * 1000.0;
        out.push(Biome { name, x, style });
    };
    add("meadow", GrassStyle::meadow());
    add("savanna", GrassStyle::savanna());
    add("tundra", GrassStyle::tundra());
    add("marsh", GrassStyle::marsh());
    add("alien", GrassStyle::alien());
    out
}

/// Patches: thicker and fresher in hollows, thinner and drier on rises.
fn field(style: GrassStyle) -> GrassField {
    GrassField::flat(160.0, 160.0)
        .sample([161, 161], |x, z| {
            let n =
                engine_core::math::noise::fbm2(x as f64 * 0.045, z as f64 * 0.045, 11, 3, 2.0, 0.5);
            let density = (0.75 + 0.35 * n).clamp(0.15, 1.0);
            let dryness = (style.dryness + 0.25 * (-n)).clamp(0.0, 1.0);
            (0.0, density, dryness)
        })
        .with_style(style)
}

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

fn shot(app: &mut App, out: &str, name: &str, eye: DVec3, target: DVec3, to_sun: Vec3) {
    aim(&mut app.world, eye, target);
    set_sun(&mut app.world, to_sun);
    app.update_n(12);
    let path = format!("{out}/{name}.png");
    app.world
        .resource_mut::<Screenshots>()
        .request(path.clone());
    app.update();
    app.update();
    println!("saved {path}");
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/lookdev_grass".into());
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
        .add_plugins(RenderPlugin);
    // A fresh breeze, so the blades lean and the gusts show.
    app.world.resource_mut::<WeatherState>().wind = Vec3::new(4.0, 0.0, 1.5);
    app.world.resource_mut::<Atmosphere>().ground_color = [0.09, 0.14, 0.05];
    let biomes = biomes();
    for b in &biomes {
        let at = DVec3::new(b.x, 0.0, 0.0);
        let [r, g, bl] = b.style.ground_color();
        app.world.spawn((
            Transform::from_translation(at),
            MeshRenderer::new(
                Shape::Plane {
                    size: [400.0, 400.0],
                    subdivisions: 1,
                },
                StandardMaterial::color(r, g, bl).with_roughness(0.95),
            ),
        ));
        app.world
            .spawn((Transform::from_translation(at), field(b.style)));
    }
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

    let side_sun = Vec3::new(0.8, 0.75, 0.35);
    for b in &biomes {
        let o = DVec3::new(b.x, 0.0, 0.0);
        if b.name == "meadow" {
            if wanted("meadow") {
                let eye = o + DVec3::new(-20.0, 1.65, 20.0);
                shot(
                    &mut app,
                    &out,
                    "meadow",
                    eye,
                    eye + DVec3::new(12.0, -1.6, -14.0),
                    side_sun,
                );
            }
            if wanted("meadow_low") {
                let eye = o + DVec3::new(-10.0, 0.45, 10.0);
                shot(
                    &mut app,
                    &out,
                    "meadow_low",
                    eye,
                    eye + DVec3::new(3.0, -0.25, -3.0),
                    side_sun,
                );
            }
            if wanted("meadow_back") {
                let eye = o + DVec3::new(-10.0, 1.0, 10.0);
                let toward = Vec3::new(0.6, 0.2, -0.75);
                shot(
                    &mut app,
                    &out,
                    "meadow_back",
                    eye,
                    eye + DVec3::new(6.0, -0.9, -7.5),
                    toward,
                );
            }
        } else if wanted(b.name) {
            let eye = o + DVec3::new(-15.0, 1.65, 15.0);
            let mut sun = side_sun;
            if b.name == "alien" {
                sun = Vec3::new(-0.5, 0.45, -0.7);
            }
            shot(
                &mut app,
                &out,
                b.name,
                eye,
                eye + DVec3::new(12.0, -1.5, -14.0),
                sun,
            );
        }
    }
}
