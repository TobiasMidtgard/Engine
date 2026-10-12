//! Renders the Valheim dig-and-build hill headless and saves screenshots of its test cases.
//!
//! ```sh
//! cargo run -p engine_destruction --release --example hill -- [out_dir] [WxH]
//! ```
//!
//! Needs a GPU or lavapipe (`mesa-vulkan-drivers`).

use std::path::PathBuf;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_destruction::{DestructionPlugin, DestructionStats, hill};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;

/// (seconds into the loop, file name, camera position, look-at point)
fn shots() -> Vec<(f32, &'static str, [f64; 3], [f64; 3])> {
    let mut v: Vec<(f32, &str, [f64; 3], [f64; 3])> = vec![
        (0.3, "00_overview", [6.0, 14.0, -40.0], [0.0, 2.0, 0.0]),
        (
            3.0,
            "01_hut_going_up",
            [10.0, 3.0, -18.0],
            [15.0, 2.0, -10.0],
        ),
        (5.5, "02_tunnel", [-2.5, 2.2, -16.0], [-4.0, 1.4, -6.0]),
        (
            8.0,
            "03_hut_support_colors",
            [9.5, 3.5, -16.5],
            [15.0, 2.2, -10.0],
        ),
        (
            11.0,
            "04_chamber_caving_in",
            [-1.0, 7.0, -17.0],
            [-4.5, 2.5, -2.0],
        ),
        (
            14.0,
            "05_after_cave_in",
            [-1.0, 7.0, -17.0],
            [-4.5, 2.5, -2.0],
        ),
        (
            17.0,
            "06_terrace_raised",
            [18.0, 8.0, -6.0],
            [8.5, 3.5, 2.0],
        ),
        (
            19.9,
            "07_hut_half_down",
            [9.5, 3.5, -16.5],
            [15.0, 1.8, -10.0],
        ),
        (
            22.0,
            "08_everything_after",
            [6.0, 14.0, -40.0],
            [0.0, 2.0, 0.0],
        ),
    ];
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

#[derive(Resource)]
struct Origin(DVec3);

fn setup(mut commands: Commands, origin: Res<Origin>) {
    commands.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.6 },
            ..Camera::perspective(55.0)
        },
        Transform::from_xyz(14.0, 6.0, -38.0).looking_at(DVec3::new(0.0, 5.0, 0.0), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.45, -0.55, 0.7), Vec3::Y),
    ));
    commands.spawn((
        Transform::from_xyz(0.0, 0.0, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4a5e2c).with_roughness(0.95),
        ),
    ));
    hill::spawn_voxel_hill(&mut commands, origin.0);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "hill_shots".into()));
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
    let step = 1.0 / 30.0;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: step });
    app.add_plugins((RenderPlugin, WorldPlugin, DestructionPlugin))
        .insert_resource(Origin(DVec3::ZERO))
        .add_systems(Startup, setup);

    let mut t = 0.0f32;
    for (at, name, eye, target) in shots() {
        while t + 1e-4 < at {
            app.update();
            t += step as f32;
        }
        {
            let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
            for mut c in q.iter_mut(&mut app.world) {
                *c = Transform::from_translation(DVec3::from_array(eye))
                    .looking_at(DVec3::from_array(target), Vec3::Y);
            }
        }
        // Render the new view (exposure adapts over a couple of frames) without moving time.
        app.world.resource_mut::<Time>().pause();
        app.update_n(4);
        app.world.resource_mut::<Time>().resume();
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = out.join(format!("{name}.png"));
        match capture_now(&mut app.world, &path) {
            Ok(_) => {
                let s = app.world.resource::<DestructionStats>();
                println!(
                    "{:>5.2}s {} (chunks {}, chips {}, cells {}, rock {:.1} m³, collapses {})",
                    t,
                    path.display(),
                    s.chunks,
                    s.chips,
                    s.cells_destroyed,
                    s.voxels_removed_m3,
                    s.collapses
                );
            }
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
}
