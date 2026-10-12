//! A bench for the look of flames: a grass fire front, a campfire and a burning crate, seen
//! close and from afar by day and by night, rendered into PNGs without a window. `sheet`
//! writes the flame flipbook itself as a contact sheet instead.
//!
//! ```text
//! cargo run -p engine_fire --example flame_look --release -- [out_dir] [seconds] [views] [hour]
//! cargo run -p engine_fire --example flame_look --release -- sheet [out_dir]
//! ```
//!
//! `seconds` of simulation run before the first picture (default 6); `views` is a
//! comma-separated subset of: front, camp, crate, wide, frames (four pictures of the campfire
//! a tenth of a second apart); `hour` sets the sun (default 22, night).

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_fire::flipbook;
use engine_fire::{ContainedFire, FirePlugin, FlameShape, FuelPatch, PatchShape};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::{Combustible, WorldPlugin};

/// (key, eye, target).
const VIEWS: &[(&str, [f64; 3], [f64; 3])] = &[
    ("front", [1.0, 1.7, -3.5], [1.0, 0.8, 6.0]),
    ("camp", [-4.0, 1.4, -5.2], [-4.0, 0.6, -2.0]),
    ("crate", [5.5, 1.6, -4.5], [4.0, 1.0, -1.0]),
    ("wide", [-6.0, 4.0, -16.0], [1.0, 1.0, 4.0]),
];

const CAMP: [f64; 2] = [-4.0, -2.0];
const CRATE: [f64; 2] = [4.0, -1.0];

/// Unit vector toward the sun for an hour of the day (as in the showcase).
fn sun_direction(hour: f32) -> Vec3 {
    let theta = (hour - 12.0) / 24.0 * std::f32::consts::TAU;
    Vec3::new(theta.sin() * 0.9, theta.cos() * 0.8, -0.35).normalize()
}

fn sheet(out: &str) {
    std::fs::create_dir_all(out).expect("output directory");
    let started = std::time::Instant::now();
    let book = flipbook::simulate(flipbook::FLIPBOOK_SIZE, 7);
    eprintln!("simulated in {:.2} s", started.elapsed().as_secs_f32());
    let [w, h, frames] = book.size.map(|s| s as usize);
    // Twelve frames across the loop, then twelve in a row.
    let mut img = image::RgbImage::new((w * 12) as u32, (h * 2) as u32);
    for row in 0..2 {
        for s in 0..12 {
            let f = if row == 0 { s * frames / 12 } else { s };
            for y in 0..h {
                for x in 0..w {
                    let [t, soot] = book.texels[(f * h + y) * w + x];
                    let t = t as f32 / 255.0;
                    let smoke = soot as f32 / 255.0 * 0.5 * (1.0 - t);
                    // A rough fire ramp for looking at it: dark red, orange, yellow, white.
                    let px = [t * 3.0, t * 3.0 - 1.0, t * 3.0 - 2.0]
                        .map(|c| ((c.clamp(0.0, 1.0) + smoke).min(1.0) * 255.0) as u8);
                    let py = (row * h + h - 1 - y) as u32;
                    img.put_pixel((s * w + x) as u32, py, image::Rgb(px));
                }
            }
        }
    }
    let path = format!("{out}/flipbook.png");
    img.save(&path).expect("save sheet");
    // How tall the flame stands: the highest row whose flame passes 0.25, averaged.
    let mut tops = 0.0;
    for f in 0..frames {
        let top = (0..h)
            .rev()
            .find(|&y| (0..w).any(|x| book.texels[(f * h + y) * w + x][0] > 64))
            .unwrap_or(0);
        tops += top as f32;
    }
    eprintln!(
        "{path}; mean flame top {:.0} of {h} rows",
        tops / frames as f32
    );
}

fn spawn_scene(world: &mut World, hour: f32) -> Entity {
    world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [2000.0, 2000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4f5a34).with_roughness(0.95),
        ),
    ));
    // A strip of dry grass to burn, lit along its near edge.
    world.spawn((
        Transform::from_xyz(1.0, 0.0, 9.0),
        FuelPatch::grass(PatchShape::Rect {
            half_extents: [9.0, 6.0],
        })
        .with_load(0.5)
        .with_moisture(0.05),
    ));
    // The campfire: stones round a few logs.
    let stone = StandardMaterial::hex(0x77726b).with_roughness(0.85);
    let log = StandardMaterial::hex(0x2a1e16)
        .with_roughness(0.9)
        .with_emissive([1.0, 0.3, 0.06], 30.0);
    for k in 0..10 {
        let a = k as f64 / 10.0 * std::f64::consts::TAU;
        world.spawn((
            Transform::from_xyz(CAMP[0] + a.cos() * 0.7, 0.1, CAMP[1] + a.sin() * 0.7)
                .with_scale(Vec3::new(1.0, 0.7, 1.0)),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.18,
                    segments: 10,
                },
                stone,
            ),
        ));
    }
    for k in 0..4 {
        let yaw = k as f32 * std::f32::consts::FRAC_PI_2 + 0.4;
        let o = Quat::from_rotation_y(yaw) * Vec3::new(0.22, 0.0, 0.0);
        world.spawn((
            Transform::from_xyz(CAMP[0] + o.x as f64, 0.28, CAMP[1] + o.z as f64)
                .with_rotation(Quat::from_rotation_y(yaw) * Quat::from_rotation_z(0.75)),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 0.06,
                    height: 0.8,
                    segments: 8,
                },
                log,
            ),
        ));
    }
    world.spawn((
        Transform::from_xyz(CAMP[0], 0.05, CAMP[1]),
        Combustible {
            fuel_kg: 0.4,
            moisture: 0.05,
            ignition_c: 250.0,
            temperature_c: 600.0,
            ..Default::default()
        },
        FlameShape {
            radius: 0.35,
            height: 1.1,
            offset: [0.0; 3],
        },
        ContainedFire { perpetual: true },
    ));
    // A wooden crate, well alight.
    world.spawn((
        Transform::from_xyz(CRATE[0], 0.5, CRATE[1]),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [1.0, 1.0, 1.0],
            },
            StandardMaterial::hex(0x3a2a1c).with_roughness(0.9),
        ),
        Combustible {
            fuel_kg: 20.0,
            moisture: 0.05,
            ignition_c: 300.0,
            temperature_c: 700.0,
            ..Default::default()
        },
        FlameShape {
            radius: 0.6,
            height: 1.8,
            offset: [0.0, 0.3, 0.0],
        },
        ContainedFire { perpetual: true },
    ));

    // The sun by day; by night a moon opposite it.
    let to_sun = sun_direction(hour);
    let (dir, light) = if to_sun.y > 0.0 {
        (to_sun, DirectionalLight::default())
    } else {
        (
            -to_sun,
            DirectionalLight {
                color: [0.75, 0.82, 1.0],
                illuminance: 0.3,
                ..Default::default()
            },
        )
    };
    world.spawn((
        light,
        Transform::default().looking_at(-dir.as_dvec3(), Vec3::Y),
    ));
    world
        .spawn((Camera::default(), Transform::from_xyz(0.0, 2.0, -8.0)))
        .id()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("sheet") {
        let out = args
            .get(2)
            .cloned()
            .unwrap_or_else(|| "screenshots/flame_look".into());
        sheet(&out);
        return;
    }
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshots/flame_look".into());
    let seconds: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(6.0);
    let only: Option<Vec<String>> = args
        .get(3)
        .filter(|s| !s.is_empty() && *s != "all")
        .map(|s| s.split(',').map(str::to_string).collect());
    let hour: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(22.0);
    std::fs::create_dir_all(&out).expect("output directory");
    let wanted = |key: &str| only.as_ref().is_none_or(|o| o.iter().any(|k| k == key));

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1280;
        w.height = 720;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, WorldPlugin, FirePlugin));
    let camera = spawn_scene(&mut app.world, hour);
    // Light the grass along its near edge.
    app.update_n(2);
    let _ = engine_core::registry::call_api(
        &mut app.world,
        "fire.ignite",
        serde_json::json!({"at": [1.0, 0.0, 4.0], "radius": 3.0, "temperature_c": 900.0}),
    );

    // Let the simulation run without drawing (much faster on a software GPU).
    let render = app
        .world
        .resource_mut::<Schedules>()
        .remove(Render)
        .expect("render schedule");
    app.world
        .resource_mut::<Schedules>()
        .insert(Schedule::new(Render));
    let started = std::time::Instant::now();
    app.update_n(((seconds * 30.0) as usize).max(2));
    eprintln!(
        "simulated {seconds} s in {:.1} s",
        started.elapsed().as_secs_f32()
    );
    app.world.resource_mut::<Schedules>().insert(render);
    let mut burning = app
        .world
        .query::<(&GlobalTransform, &engine_fire::Burning)>();
    for (gt, b) in burning.iter(&app.world) {
        eprintln!(
            "burning at {:.1}: intensity {:.2}, {:.0} kW",
            gt.translation(),
            b.intensity,
            b.hrr_w / 1000.0
        );
    }

    let mut shots: Vec<(String, [f64; 3], [f64; 3], usize)> = VIEWS
        .iter()
        .filter(|(key, ..)| wanted(key))
        .map(|(key, eye, target)| (key.to_string(), *eye, *target, 6))
        .collect();
    if wanted("frames") {
        for i in 0..4 {
            shots.push((format!("frames{i}"), VIEWS[1].1, VIEWS[1].2, 3));
        }
    }
    for (key, eye, target, settle) in shots {
        let eye = DVec3::from_array(eye);
        let target = DVec3::from_array(target);
        *app.world.get_mut::<Transform>(camera).expect("camera") =
            Transform::from_translation(eye).looking_at(target, Vec3::Y);
        // A few frames for exposure and the environment map to settle.
        let started = std::time::Instant::now();
        app.update_n(settle);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = format!("{out}/{key}.png");
        match capture_now(&mut app.world, &path) {
            Ok(_) => eprintln!("{path} ({:.1} s)", started.elapsed().as_secs_f32()),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
}
