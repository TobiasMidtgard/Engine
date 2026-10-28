//! Renders a streamed village from 300 m away twice, headless: loaded (its full meshes), then
//! unloaded with its far-view proxy standing in ([`engine::hlod`]), so the two can be compared.
//!
//! `cargo run -p engine --example hlod_preview -- <out dir> [WxH]`

use std::path::PathBuf;

use engine::hlod::{HlodSettings, Proxies};
use engine::stream::{StreamCell, StreamCellsExt, StreamingBudget, StreamingPlugin};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::{CellState, StreamedCells, StreamingSource};

/// Sixteen plastered houses with tiled roofs, a stone tower and a few trees.
fn village(world: &mut World) {
    let walls = StandardMaterial::preset(MaterialPreset::Plaster);
    let roof = StandardMaterial::hex(0x8e3b26).with_roughness(0.8);
    let tower = StandardMaterial::preset(MaterialPreset::Rock);
    let bark = StandardMaterial::preset(MaterialPreset::Bark);
    let leaves = StandardMaterial::hex(0x3d5e22).with_roughness(0.9);
    let mut put = |shape: Shape, material: StandardMaterial, at: DVec3, turn: f32| {
        let t = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(turn));
        world.spawn((
            t,
            GlobalTransform::from(t),
            MeshRenderer::new(shape, material),
        ));
    };
    for i in 0..16 {
        let (row, col) = ((i / 4) as f64, (i % 4) as f64);
        let at = DVec3::new(-36.0 + col * 24.0, 0.0, -30.0 + row * 20.0);
        let turn = (i as f32 * 0.37).sin() * 0.3;
        let (w, d, h) = (8.0 + (i % 3) as f32 * 2.0, 7.0, 5.0 + (i % 2) as f32 * 2.0);
        put(
            Shape::Cuboid { size: [w, h, d] },
            walls,
            at + DVec3::Y * (h as f64 / 2.0),
            turn,
        );
        put(
            Shape::Cone {
                radius: w * 0.75,
                height: 3.5,
                segments: 4,
            },
            roof,
            at + DVec3::Y * (h as f64 + 1.75),
            turn + std::f32::consts::FRAC_PI_4,
        );
    }
    put(
        Shape::Cylinder {
            radius: 5.0,
            height: 26.0,
            segments: 24,
        },
        tower,
        DVec3::new(40.0, 13.0, 30.0),
        0.0,
    );
    for i in 0..10 {
        let at = DVec3::new(-50.0 + i as f64 * 11.0, 0.0, 42.0 + (i % 3) as f64 * 4.0);
        put(
            Shape::Cylinder {
                radius: 0.4,
                height: 5.0,
                segments: 8,
            },
            bark,
            at + DVec3::Y * 2.5,
            0.0,
        );
        put(
            Shape::Sphere {
                radius: 3.2,
                segments: 16,
            },
            leaves,
            at + DVec3::Y * 7.0,
            0.0,
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "hlod_shots".into()));
    let (w, h) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    std::fs::create_dir_all(&out).expect("create output dir");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = w;
        win.height = h;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, engine_world::WorldPlugin, StreamingPlugin));
    {
        let mut s = app.world.resource_mut::<HlodSettings>();
        s.settle_frames = 3;
        s.cache_dir = out.join("cache");
    }
    // Hard swaps: lavapipe frames take seconds, longer than any fade.
    app.world.resource_mut::<StreamingBudget>().fade_s = 0.0;
    let eye = DVec3::new(-120.0, 45.0, 280.0);
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.2 },
            ..Camera::perspective(18.0)
        },
        Transform::from_translation(eye).looking_at(DVec3::new(0.0, 4.0, 0.0), Vec3::Y),
        StreamingSource::default(),
    ));
    app.world.spawn((
        DirectionalLight {
            color: [1.0, 0.9, 0.78],
            illuminance: 75_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.6, -0.45, -0.5), Vec3::Y),
    ));
    let ground = Transform::default();
    app.world.spawn((
        ground,
        GlobalTransform::from(ground),
        MeshRenderer::new(
            Shape::Plane {
                size: [1200.0, 1200.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x5d6b3c).with_roughness(0.95),
        ),
    ));
    app.stream_cell(StreamCell::new("village", DVec3::ZERO, 70.0, village).load_within(50.0));

    let shot = |app: &mut App, name: &str| {
        app.update_n(3);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            std::process::exit(0);
        }
        let path = out.join(format!("{name}.png"));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{}", path.display()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    };
    let pin = |app: &mut App, v: bool| {
        app.world
            .resource_mut::<StreamedCells>()
            .pins
            .insert("village".into(), v);
    };
    pin(&mut app, true);
    for _ in 0..600 {
        app.update();
        if app.world.resource::<Proxies>().triangles()[0] > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    println!(
        "proxy: {} triangles",
        app.world.resource::<Proxies>().triangles()[0]
    );
    shot(&mut app, "village_loaded");
    pin(&mut app, false);
    app.update();
    assert_eq!(
        app.world.resource::<StreamedCells>().cells[0].state,
        CellState::Unloaded
    );
    shot(&mut app, "village_proxy");
    // Closer, to see what the proxy gives up.
    {
        let mut q = app.world.query::<(&mut Transform, &Camera)>();
        for (mut t, _) in q.iter_mut(&mut app.world) {
            *t = Transform::from_translation(DVec3::new(-60.0, 25.0, 120.0))
                .looking_at(DVec3::new(0.0, 4.0, 0.0), Vec3::Y);
        }
    }
    shot(&mut app, "village_proxy_near");
    pin(&mut app, true);
    shot(&mut app, "village_loaded_near");
}
