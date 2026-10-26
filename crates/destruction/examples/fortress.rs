//! Renders the destruction showcase headless and saves screenshots of every test case.
//!
//! ```sh
//! cargo run -p engine_destruction --release --example fortress -- [out_dir] [WxH] [only]
//! ```
//!
//! `only` is a comma-separated list of words; just the shots whose names contain one are
//! saved (`cliff,gate`). Time between shots is simulated without drawing. Needs a GPU or
//! lavapipe (`mesa-vulkan-drivers`).

use std::path::PathBuf;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_destruction::{DestructionPlugin, DestructionStats, demo};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use engine_world::WorldPlugin;

/// (seconds into the demo, file name, camera position, look-at point)
fn shots() -> Vec<(f32, &'static str, [f64; 3], [f64; 3])> {
    let mut v: Vec<(f32, &str, [f64; 3], [f64; 3])> = vec![
        (
            0.45,
            "00_establishing",
            [-34.0, 26.0, -62.0],
            [0.0, 3.0, 2.0],
        ),
        (0.5, "01_zone_stop", [-2.0, 17.0, -44.0], [-3.0, 2.5, 4.0]),
        (0.5, "01b_gate", [-14.0, 5.0, -54.0], [0.0, 7.0, -31.0]),
        (0.5, "01c_courtyard", [24.0, 15.0, -26.0], [-6.0, 2.0, 8.0]),
        (
            0.5,
            "01d_breach_outside",
            [64.0, 5.0, -4.0],
            [44.0, 3.5, 2.0],
        ),
        (
            0.55,
            "02_bay1_bullets_vault_hole",
            [17.5, 1.8, -14.0],
            [20.4, 1.5, -8.0],
        ),
        (
            0.6,
            "03_bay2_barricades",
            [11.0, 1.8, -14.5],
            [13.6, 1.3, -8.0],
        ),
        (
            0.65,
            "04_bay3_thermite_doorway",
            [4.5, 1.9, -14.5],
            [6.8, 1.2, -8.0],
        ),
        (
            0.7,
            "05_bay5_explosive_rebar",
            [-4.5, 1.9, -14.0],
            [-6.8, 1.5, -8.0],
        ),
        (
            0.75,
            "06_bay7_brick_blast",
            [-17.0, 2.0, -15.0],
            [-20.4, 1.3, -8.0],
        ),
        (
            2.4,
            "07_live_thermite_burning",
            [2.2, 1.7, -13.0],
            [0.0, 1.2, -8.0],
        ),
        (3.0, "08_cliff_mining", [3.0, 4.0, -6.0], [-4.0, 3.0, 12.0]),
        (
            3.3,
            "09_live_bullets",
            [-11.5, 1.8, -15.5],
            [-13.6, 1.6, -8.0],
        ),
        (
            4.35,
            "10_live_charge_blast",
            [-9.5, 2.4, -17.0],
            [-13.6, 1.3, -8.0],
        ),
        (
            9.0,
            "10b_live_charge_crates_after",
            [-10.5, 1.8, -14.5],
            [-13.6, 0.4, -8.0],
        ),
        (
            4.6,
            "11_cliff_collapse",
            [3.0, 5.0, -7.0],
            [-4.0, 3.5, 12.0],
        ),
        (
            5.6,
            "12_live_thermite_open",
            [2.5, 1.8, -13.5],
            [0.0, 1.2, -8.0],
        ),
        (7.0, "13_cliff_after", [3.0, 5.0, -7.0], [-4.0, 3.5, 12.0]),
        (
            7.6,
            "14_tower_support_colors",
            [9.0, 4.5, -5.0],
            [18.0, 3.2, 9.0],
        ),
        (
            10.5,
            "15_tower_losing_posts",
            [9.0, 4.5, -5.0],
            [18.0, 3.0, 9.0],
        ),
        (12.2, "16_tower_down", [9.0, 4.5, -5.0], [18.0, 2.0, 9.0]),
        (
            12.65,
            "17_gate_blast",
            [-17.0, 4.0, -5.0],
            [-22.0, 2.5, 7.0],
        ),
        (14.0, "18_gate_after", [-17.0, 4.0, -5.0], [-22.0, 2.5, 7.0]),
        (
            14.4,
            "19_arch_blast",
            [-26.0, 6.0, -4.0],
            [-36.0, 3.5, 16.0],
        ),
        (
            16.5,
            "20_arch_after",
            [-26.0, 6.0, -4.0],
            [-36.0, 3.0, 16.0],
        ),
        (0.5, "22_house_roof", [20.5, 8.5, 13.5], [15.0, 3.0, 21.0]),
        (
            0.55,
            "23_house_under_the_roof",
            [16.2, 1.6, 25.4],
            [15.1, 2.1, 20.4],
        ),
        (
            6.3,
            "24_house_roof_charge",
            [20.5, 8.5, 13.5],
            [15.0, 3.0, 21.0],
        ),
        (
            10.2,
            "25_house_hatch_thermite",
            [18.0, 6.0, 17.0],
            [15.6, 3.2, 20.4],
        ),
        (
            12.6,
            "25b_house_hatch_plate_out",
            [18.0, 6.0, 17.0],
            [15.6, 3.2, 20.4],
        ),
        (
            13.0,
            "26_house_under_the_roof_after",
            [16.2, 1.6, 25.4],
            [15.1, 2.1, 20.4],
        ),
        (
            13.2,
            "27_house_roof_after",
            [20.5, 8.5, 13.5],
            [15.0, 3.0, 21.0],
        ),
        (
            1.0,
            "28_reinforce_shot_through",
            [-24.6, 1.8, -13.6],
            [-26.6, 1.4, -8.0],
        ),
        (
            2.3,
            "29_reinforce_behind_carried_in",
            [-23.6, 2.3, -2.6],
            [-27.2, 1.3, -8.2],
        ),
        (
            4.2,
            "30_reinforce_behind_braced",
            [-23.6, 2.3, -2.6],
            [-27.2, 1.3, -8.2],
        ),
        (
            4.4,
            "35_reinforce_behind_struts",
            [-25.9, 1.0, -5.3],
            [-27.2, 0.7, -7.6],
        ),
        (
            5.3,
            "31_reinforce_bullets_stop",
            [-25.3, 1.7, -11.2],
            [-27.0, 1.5, -8.0],
        ),
        (
            7.6,
            "32_reinforce_charge",
            [-24.6, 1.8, -13.6],
            [-26.6, 1.3, -8.0],
        ),
        (
            13.0,
            "33_reinforce_thermite",
            [-24.6, 1.8, -13.6],
            [-26.4, 1.3, -8.0],
        ),
        (
            17.0,
            "21_everything_after",
            [-2.0, 17.0, -44.0],
            [-3.0, 2.5, 4.0],
        ),
        (
            17.6,
            "34_reinforce_behind_cut_through",
            [-23.6, 2.3, -2.6],
            [-27.2, 1.3, -8.2],
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
    demo::spawn_demo(&mut commands, origin.0);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(
        args.get(1)
            .cloned()
            .unwrap_or_else(|| "fortress_shots".into()),
    );
    let (w, h) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    let only: Vec<String> = args
        .get(3)
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_default();
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
    // Textures are made on worker threads; these shots need them from the first frame.
    engine_destruction::assets::wait_for_assets(&mut app.world);

    let mut t = 0.0f32;
    for (at, name, eye, target) in shots() {
        if !only.is_empty() && !only.iter().any(|w| name.contains(w.as_str())) {
            continue;
        }
        // Simulate up to the shot without drawing (a CPU renderer takes seconds a frame).
        let renderer = app.world.remove_resource::<Renderer>();
        if renderer.is_some() {
            app.world
                .insert_resource(RenderDisabled("between shots".into()));
        }
        while t + 1e-4 < at {
            app.update();
            t += step as f32;
        }
        if let Some(r) = renderer {
            app.world.remove_resource::<RenderDisabled>();
            app.world.insert_resource(r);
        }
        {
            // Indoors is about three stops darker than the sunlit courtyard, the back of a
            // wall in its own shadow about one, and right down at its foot a little more.
            let ev100 = if name.contains("under") {
                12.3
            } else if name.contains("behind_struts") {
                13.0
            } else if name.contains("behind") {
                13.6
            } else {
                14.6
            };
            let mut q = app.world.query::<(&mut Transform, &mut Camera)>();
            for (mut c, mut cam) in q.iter_mut(&mut app.world) {
                *c = Transform::from_translation(DVec3::from_array(eye))
                    .looking_at(DVec3::from_array(target), Vec3::Y);
                cam.exposure = Exposure::Manual { ev100 };
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
