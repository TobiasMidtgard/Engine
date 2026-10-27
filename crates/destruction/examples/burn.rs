//! Renders the burning storehouse headless: the timber shed charring and coming down.
//!
//! ```sh
//! cargo run -p engine_destruction --release --example burn -- [out_dir] [WxH]
//! ```
//!
//! Module crates can't use the fire module, so a stand-in fire here lights whatever sits
//! next to something burning and burns its fuel away; there are no flames or smoke, only the
//! char, the embers and the collapse that destruction itself draws. The showcase burns the
//! shed with the real fire module. Needs a GPU or lavapipe (`mesa-vulkan-drivers`).

use std::path::PathBuf;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_destruction::barn::{BarnPiece, spawn_barn};
use engine_destruction::{DestructionPlugin, DestructionStats, StructuralPiece};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use engine_world::{BurnState, Combustible, WorldPlugin};

/// (seconds, file name, camera position, look-at point)
fn shots() -> Vec<(f32, &'static str, [f64; 3], [f64; 3])> {
    let front = ([7.5, 2.4, -7.5], [0.0, 1.6, 0.0]);
    let close = ([2.6, 1.6, -3.4], [0.3, 1.4, 0.6]);
    vec![
        (1.0, "00_storehouse", front.0, front.1),
        (1.2, "01_inside_woodpile", close.0, close.1),
        (10.0, "02_woodpile_burning", close.0, close.1),
        (18.0, "03_walls_catching", front.0, front.1),
        (26.0, "04_charred_glowing", close.0, close.1),
        (34.0, "05_sagging", front.0, front.1),
        (44.0, "06_down", front.0, front.1),
        (70.0, "07_smoldering_ruin", front.0, front.1),
    ]
}

/// Stand-in fire: lit things burn their fuel away in `BURN_SECS`; anything within reach of a
/// burning piece for `CATCH_SECS` lights too (reach grows with the two pieces' sizes).
const BURN_SECS: f32 = 22.0;
const CATCH_SECS: f32 = 3.0;
const REACH_M: f64 = 0.6;

#[derive(Component, Default)]
struct Heat(f32);

fn stand_in_fire(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(
        Entity,
        &GlobalTransform,
        &StructuralPiece,
        &mut Combustible,
        Option<&mut Heat>,
    )>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let size = |p: &StructuralPiece| p.half_extents.max_element() as f64 * 0.6;
    let burning: Vec<(DVec3, f64)> = q
        .iter()
        .filter(|(_, _, _, c, _)| c.state == BurnState::Burning)
        .map(|(_, g, p, ..)| (g.translation(), size(p)))
        .collect();
    for (e, g, piece, mut c, heat) in &mut q {
        match c.state {
            BurnState::Intact => {
                if c.temperature_c >= c.ignition_c {
                    c.state = BurnState::Burning;
                    continue;
                }
                let near = burning
                    .iter()
                    .any(|(p, r)| p.distance(g.translation()) < REACH_M + r + size(piece));
                match heat {
                    Some(mut h) if near => {
                        h.0 += dt;
                        if h.0 >= CATCH_SECS {
                            c.temperature_c = c.ignition_c + 100.0;
                        }
                    }
                    None if near => {
                        commands.entity(e).insert(Heat(dt));
                    }
                    _ => {}
                }
            }
            BurnState::Burning => {
                let full = c.fuel_kg.max(0.3);
                c.fuel_kg = (c.fuel_kg - full * dt / BURN_SECS).max(0.0);
                if c.fuel_kg <= 0.05 {
                    c.state = BurnState::Smoldering;
                }
            }
            BurnState::Smoldering => {
                c.fuel_kg = (c.fuel_kg - 0.01 * dt).max(0.0);
                if c.fuel_kg <= 0.0 {
                    c.state = BurnState::Burnt;
                }
            }
            BurnState::Burnt => {}
        }
    }
}

fn setup(mut commands: Commands) {
    commands.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.6 },
            ..Camera::perspective(55.0)
        },
        Transform::from_xyz(7.0, 3.0, -7.0).looking_at(DVec3::new(0.0, 1.5, 0.0), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.45, -0.55, 0.7), Vec3::Y),
    ));
    commands.spawn((
        Transform::from_xyz(0.0, 0.0, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x5d5a50).with_roughness(0.95),
        ),
    ));
    spawn_barn(&mut commands, DVec3::ZERO, 0.0);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "burn_shots".into()));
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
        .add_systems(Startup, setup)
        .add_systems(FixedUpdate, stand_in_fire);
    // Textures are made on worker threads; these shots need them from the first frame.
    engine_destruction::assets::wait_for_assets(&mut app.world);
    app.world.insert_resource(engine_destruction::LoadAtOnce);

    let mut t = 0.0f32;
    for (at, name, eye, target) in shots() {
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
            let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
            for mut c in q.iter_mut(&mut app.world) {
                *c = Transform::from_translation(DVec3::from_array(eye))
                    .looking_at(DVec3::from_array(target), Vec3::Y);
            }
        }
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
                let standing = app
                    .world
                    .query_filtered::<(), (With<BarnPiece>, With<StructuralPiece>)>()
                    .iter(&app.world)
                    .count();
                let burning = app
                    .world
                    .query::<&Combustible>()
                    .iter(&app.world)
                    .filter(|c| c.state == BurnState::Burning)
                    .count();
                let s = app.world.resource::<DestructionStats>();
                println!(
                    "{t:>5.1}s {} (standing {standing}, burning {burning}, collapses {})",
                    path.display(),
                    s.collapses
                );
            }
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
}
