//! Renders the arena fight and the wildlife meadow offscreen after they have run for a while,
//! and checks that the stand-in bodies show up. Needs a GPU or a software Vulkan driver
//! (lavapipe); passes with a note when neither is available (`RenderDisabled`).
//!
//! The PNGs land in the test's temp dir; set `NPC_SCREENSHOT_DIR` to also copy them there.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;

const W: u32 = 960;
const H: u32 = 540;

const ARENA: DVec3 = DVec3::new(0.0, 0.0, 0.0);
const MEADOW: DVec3 = DVec3::new(400.0, 0.0, 0.0);

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    // Big steps so a few seconds of fighting take few rendered frames.
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.1 });
    app.add_plugins((WorldPlugin, RenderPlugin, NpcPlugin));
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.5, -0.7, 0.6), Vec3::Y),
    ));
    for (center, color) in [(ARENA, 0x8c7a5b), (MEADOW, 0x5d7d3c)] {
        w.spawn((
            Transform::from_translation(center),
            MeshRenderer::new(
                Shape::Plane {
                    size: [160.0, 160.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(color).with_roughness(0.95),
            ),
        ));
    }
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, ARENA);
        demo::spawn_wildlife(&mut commands, MEADOW, 45.0);
    }
    app.world.flush();
    app
}

fn camera(app: &mut App, eye: DVec3, target: DVec3) -> Entity {
    app.world
        .spawn((
            Camera {
                active: false,
                ..Camera::perspective(60.0)
            },
            Transform::from_translation(eye).looking_at(target, Vec3::Y),
        ))
        .id()
}

/// Make `cam` the only active camera and save a screenshot. `None` without a GPU.
fn shoot(app: &mut App, cam: Entity, name: &str) -> Option<image::RgbaImage> {
    let mut q = app.world.query::<(Entity, &mut Camera)>();
    for (e, mut c) in q.iter_mut(&mut app.world) {
        c.active = e == cam;
    }
    app.update_n(2);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(dir) = std::env::var("NPC_SCREENSHOT_DIR") {
        let _ = std::fs::create_dir_all(&dir);
        std::fs::copy(&path, std::path::Path::new(&dir).join(name)).expect("copy screenshot");
    }
    eprintln!("wrote {}", path.display());
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Pixels that are clearly red (red fighters), blue (blue fighters) or brown (deer).
fn count(img: &image::RgbaImage, f: impl Fn(f32, f32, f32) -> bool) -> usize {
    img.pixels()
        .filter(|p| f(p[0] as f32, p[1] as f32, p[2] as f32))
        .count()
}

#[test]
fn arena_and_meadow_screenshots() {
    let mut app = app();
    let overview = camera(
        &mut app,
        ARENA + DVec3::new(0.0, 16.0, -31.0),
        ARENA + DVec3::new(0.0, 0.0, 4.0),
    );
    let ring = ARENA + demo::DUEL_RING;
    let duel = camera(
        &mut app,
        ring + DVec3::new(-2.0, 2.4, -7.5),
        ring + DVec3::new(0.0, 1.0, 0.0),
    );
    let cover = camera(
        &mut app,
        ARENA + DVec3::new(-15.0, 4.5, -4.0),
        ARENA + DVec3::new(2.0, 0.8, 9.0),
    );
    let lane = ARENA + demo::SPARRING_LANE;
    let sparring = camera(
        &mut app,
        lane + DVec3::new(-8.0, 2.6, -4.5),
        lane + DVec3::new(0.0, 1.0, 9.0),
    );
    let stands = camera(
        &mut app,
        ARENA + DVec3::new(-6.0, 7.0, -36.0),
        ARENA + DVec3::new(0.0, 2.0, 20.0),
    );
    let meadow = camera(
        &mut app,
        MEADOW + DVec3::new(-6.0, 9.0, -30.0),
        MEADOW + DVec3::new(0.0, 0.0, 6.0),
    );
    let herd = camera(
        &mut app,
        MEADOW + DVec3::new(-13.0, 2.5, -9.0),
        MEADOW + DVec3::new(-3.0, 0.8, 4.0),
    );
    // Let the fight develop: squads spot each other, take cover, flank, close in.
    app.update_n(3);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping: no GPU");
        return;
    }
    app.update_n(60);
    let Some(img) = shoot(&mut app, overview, "npc_arena_overview.png") else {
        return;
    };
    let red = count(&img, |r, g, b| r > 70.0 && r > g * 1.7 && r > b * 1.7);
    let blue = count(&img, |r, g, b| b > 70.0 && b > r * 1.5 && b > g * 1.2);
    assert!(red > 150, "red fighters visible: {red} px");
    assert!(blue > 150, "blue fighters visible: {blue} px");
    shoot(&mut app, cover, "npc_arena_cover.png");
    shoot(&mut app, stands, "npc_arena_stands.png");
    shoot(&mut app, sparring, "npc_sparring_lane.png");
    app.update_n(5);
    shoot(&mut app, duel, "npc_arena_duel.png");
    let img = shoot(&mut app, meadow, "npc_wildlife_meadow.png").unwrap();
    shoot(&mut app, herd, "npc_wildlife_herd.png");
    let brown = count(&img, |r, g, b| {
        r > 60.0 && r > g * 1.25 && g > b * 1.15 && r < 220.0
    });
    assert!(brown > 100, "deer visible: {brown} px");
}
