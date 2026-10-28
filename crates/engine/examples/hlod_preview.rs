//! Renders a streamed village from 300 m away twice, headless: loaded (its full meshes), then
//! unloaded with its far-view proxy standing in ([`engine::hlod`]), so the two can be compared,
//! and prints the village's mean brightness in each. The walls are made the way the destruction
//! module makes its buildings: a white material with a stone texture, tinted by vertex colors
//! (darker and greener toward the ground).
//!
//! `cargo run -p engine --example hlod_preview -- <out dir> [WxH]`

use std::path::PathBuf;

use engine::assets::Assets;
use engine::hlod::{HlodSettings, Proxies};
use engine::stream::{StreamCell, StreamCellsExt, StreamingBudget, StreamingPlugin};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::{CellState, StreamedCells, StreamingSource};

/// A stone texture: light blocks with darker mortar lines.
fn stone() -> Image {
    Image::from_fn(64, 64, true, |x, y| {
        let mortar = y % 16 < 2 || (x + if (y / 16) % 2 == 0 { 0 } else { 16 }) % 32 < 2;
        let v = if mortar {
            70
        } else {
            170 + ((x * 7 + y * 13) % 23) as u8
        };
        [v, v - 6, v - 14, 255]
    })
}

/// A box whose vertex colors darken and green toward its foot (`h` meters tall).
fn tinted_box(w: f32, h: f32, d: f32) -> Mesh {
    let mut mesh = Shape::Cuboid { size: [w, h, d] }.mesh();
    for v in &mut mesh.vertices {
        let up = (v.position[1] / h + 0.5).clamp(0.0, 1.0);
        v.color = [0.55 + 0.45 * up, 0.6 + 0.4 * up, 0.5 + 0.5 * up, 1.0];
    }
    mesh
}

/// Sixteen stone houses with tiled roofs, a stone tower and a few trees.
fn village(world: &mut World) {
    let texture = world.resource_mut::<Assets<Image>>().add(stone());
    let walls = StandardMaterial {
        base_color: [1.0; 4],
        base_color_texture: Some(texture),
        uv_scale: [0.25, 0.25],
        ..Default::default()
    };
    let roof = StandardMaterial::hex(0x8e3b26).with_roughness(0.8);
    let tower = StandardMaterial::preset(MaterialPreset::Rock);
    let bark = StandardMaterial::preset(MaterialPreset::Bark);
    let leaves = StandardMaterial::hex(0x3d5e22).with_roughness(0.9);
    let put =
        |world: &mut World, shape: Shape, material: StandardMaterial, at: DVec3, turn: f32| {
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
        let mesh = world
            .resource_mut::<Assets<Mesh>>()
            .add(tinted_box(w, h, d));
        let t = Transform::from_translation(at + DVec3::Y * (h as f64 / 2.0))
            .with_rotation(Quat::from_rotation_y(turn));
        world.spawn((
            t,
            GlobalTransform::from(t),
            MeshRenderer {
                mesh: MeshRef::Handle(mesh),
                material: MaterialRef::Inline(walls),
                cast_shadows: true,
                visible: true,
            },
        ));
        put(
            world,
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
        world,
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
            world,
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
            world,
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

/// Mean linear luminance of the middle of a shot (where the village is; the grass and sky
/// around it are the same in every shot).
fn brightness(path: &std::path::Path) -> f32 {
    let Ok(img) = std::fs::read(path)
        .map_err(|e| e.to_string())
        .and_then(|b| Image::decode(&b, Some("png"), false))
    else {
        return f32::NAN;
    };
    let (w, h) = (img.width, img.height);
    let (mut sum, mut n) = (0.0, 0.0);
    for y in h * 2 / 5..h * 3 / 5 {
        for x in w * 3 / 10..w * 7 / 10 {
            let p = img.pixel(x, y);
            let lin = |c: u8| (c as f32 / 255.0).powf(2.2);
            sum += 0.2126 * lin(p[0]) + 0.7152 * lin(p[1]) + 0.0722 * lin(p[2]);
            n += 1.0;
        }
    }
    sum / n
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
            Ok(_) => println!(
                "{}: village brightness {:.3}",
                path.display(),
                brightness(&path)
            ),
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
