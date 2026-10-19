//! Renders the market street (the showcase's UI test area) on its own, on a flat meadow in the
//! afternoon sun, from a few viewpoints, and saves a PNG per view.
//!
//! `cargo run -p engine_ui --example market -- <out_dir> [width] [height] [view ...]`
//!
//! Views: overview, brannoc, produce, cloth, potter, smith, table, corner, square (all by
//! default).
//! Needs a GPU or a software Vulkan driver (lavapipe).

use engine_assets::Assets;
use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{Mesh, RenderDisabled, Vertex, capture_now};
use engine_ui::demo::market::{spawn_market, stall_place, ui_view};
use engine_world::WeatherState;

/// The sun in the afternoon: from the south-west, 38° up.
fn sun_dir() -> Vec3 {
    let (a, e) = (2.4f32, 38f32.to_radians());
    Vec3::new(a.sin() * e.cos(), e.sin(), -a.cos() * e.cos())
}

fn scene(app: &mut App, w: u32, h: u32) {
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = w;
        win.height = h;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.shadow_distance = 120.0;
        s.shadow_map_size = 4096;
    }
    app.world.insert_resource(WeatherState {
        cloud_cover: 0.25,
        sun_direction: sun_dir(),
        time_of_day_h: 15.0,
        ..Default::default()
    });
    app.world.spawn((
        DirectionalLight {
            color: [1.0, 0.96, 0.9],
            illuminance: 100_000.0,
            cast_shadows: true,
            angular_size_deg: 0.53,
        },
        Transform::default().looking_at(-sun_dir().as_dvec3(), Vec3::Y),
    ));
    // A meadow to stand on.
    let r = 400.0f32;
    let v = |x: f32, z: f32| Vertex::new(Vec3::new(x, 0.0, z), Vec3::Y, Vec2::new(x, z));
    let ground = Mesh::new(
        vec![v(-r, -r), v(r, -r), v(r, r), v(-r, r)],
        vec![0, 2, 1, 0, 3, 2],
    );
    let mesh = app.world.resource_mut::<Assets<Mesh>>().add(ground);
    let mat = app
        .world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::hex(0x4f6b2c).with_roughness(0.95));
    app.world
        .spawn((Transform::default(), MeshRenderer::new(mesh, mat)));
    spawn_market(&mut app.world, DVec3::ZERO);
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.6 },
            ..Camera::perspective(55.0)
        },
        Transform::from_xyz(0.0, 2.0, 10.0),
    ));
}

/// Eye and target for a view in front of stall `i`, `dist` m out and `side` m to its right.
fn at_stall(i: usize, dist: f64, side: f64, eye_y: f64) -> (DVec3, DVec3) {
    let (p, yaw) = stall_place(i);
    let front = DVec3::new(yaw.sin() as f64, 0.0, yaw.cos() as f64);
    let right = DVec3::new(front.z, 0.0, -front.x);
    (
        p + front * dist + right * side + DVec3::Y * eye_y,
        p + DVec3::Y * 1.3,
    )
}

fn main() {
    engine_core::log::init("warn");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "screenshots/market".into()),
    );
    let w: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1600);
    let h: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(900);
    let only: Vec<String> = args.iter().skip(3).cloned().collect();
    let want = |s: &str| only.is_empty() || only.iter().any(|o| o == s);
    std::fs::create_dir_all(&dir).expect("output dir");

    let mut app = App::new();
    scene(&mut app, w, h);
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    let views: Vec<(&str, DVec3, DVec3)> = vec![
        (
            "overview",
            DVec3::new(12.0, 3.2, -16.0),
            DVec3::new(0.0, 1.5, 3.0),
        ),
        {
            let (e, t) = ui_view();
            ("brannoc", e, t)
        },
        {
            let (e, t) = at_stall(0, 5.5, 1.2, 1.7);
            ("produce", e, t)
        },
        {
            let (e, t) = at_stall(1, 5.5, -1.2, 1.7);
            ("cloth", e, t)
        },
        {
            let (e, t) = at_stall(3, 5.5, 1.2, 1.7);
            ("potter", e, t)
        },
        {
            let (e, t) = at_stall(4, 5.5, -1.2, 1.7);
            ("smith", e, t)
        },
        (
            "table",
            DVec3::new(2.4, 2.1, 7.4),
            DVec3::new(0.0, 0.9, 5.0),
        ),
        (
            "corner",
            DVec3::new(7.8, 1.75, -6.2),
            DVec3::new(4.8, 0.9, -10.8),
        ),
        (
            "square",
            DVec3::new(-3.0, 9.0, -14.0),
            DVec3::new(-2.0, 0.0, 4.0),
        ),
    ];
    for (name, eye, target) in views {
        if !want(name) {
            continue;
        }
        let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
        for mut t in q.iter_mut(&mut app.world) {
            *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
        }
        app.update_n(12);
        let path = dir.join(format!("{name}.png"));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("wrote {}", path.display()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
}
