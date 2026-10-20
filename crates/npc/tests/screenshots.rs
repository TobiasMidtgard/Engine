//! Renders the arena fight and the wildlife meadow offscreen after they have run for a while,
//! and checks that the stand-in bodies show up: both squads' team colors below the horizon and
//! the deer in the meadow. Needs a GPU or a software Vulkan driver (lavapipe); passes with a
//! note when neither is available (`RenderDisabled`).
//!
//! The fight runs without drawing between shots, and by default only the three views the
//! checks read are rendered, at [`CHECK_W`] × [`CHECK_H`], so the test stays quick on lavapipe
//! in CI. Set `NPC_SCREENSHOT_DIR` to render every view at [`W`] × [`H`] and keep the PNGs
//! there; the checks then read the full-size views.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;

/// The pictures kept with `NPC_SCREENSHOT_DIR`.
const W: u32 = 960;
const H: u32 = 540;
/// The views the checks read when no pictures are kept. The thresholds are pixel counts, so
/// the smaller frame (fewer pixels per fighter or deer) only makes them harder to pass. On
/// lavapipe a frame of the arena costs about the same at any size (the scene, not the pixels).
const CHECK_W: u32 = 640;
const CHECK_H: u32 = 360;

const ARENA: DVec3 = DVec3::new(0.0, 0.0, 0.0);
const MEADOW: DVec3 = DVec3::new(400.0, 0.0, 0.0);

/// Where to keep the full-size pictures, when they are wanted.
fn picture_dir() -> Option<String> {
    std::env::var("NPC_SCREENSHOT_DIR").ok()
}

fn app(width: u32, height: u32) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    // Big steps so a few seconds of fighting take few frames.
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

/// Run `steps` frames without drawing: the AI, animation and art update as in a rendered frame,
/// but nothing is drawn. The shots don't miss those frames: each shot's camera starts with a
/// cut, which resets the renderer's history and exposure.
fn simulate(app: &mut App, steps: usize) {
    let render = app
        .world
        .resource_mut::<Schedules>()
        .remove(Render)
        .expect("render schedule");
    app.world
        .resource_mut::<Schedules>()
        .insert(Schedule::new(Render));
    app.update_n(steps);
    app.world.resource_mut::<Schedules>().insert(render);
}

fn camera(app: &mut App, eye: DVec3, target: DVec3) -> Entity {
    app.world
        .spawn((
            Camera {
                active: false,
                ..Camera::perspective(FOV_Y_DEG as f32)
            },
            Transform::from_translation(eye).looking_at(target, Vec3::Y),
        ))
        .id()
}

/// Make `cam` the only active camera and save a screenshot (two frames to settle after the cut,
/// then the capture). `None` without a GPU.
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
    if let Some(dir) = picture_dir() {
        let _ = std::fs::create_dir_all(&dir);
        std::fs::copy(&path, std::path::Path::new(&dir).join(name)).expect("copy screenshot");
    }
    eprintln!("wrote {}", path.display());
    Some(image::open(&path).expect("png").to_rgba8())
}

/// [`shoot`] for a view that is only a picture: without `NPC_SCREENSHOT_DIR` the same two frames
/// run without drawing, so the fight is at the same step at the next shot either way.
fn picture(app: &mut App, cam: Entity, name: &str) {
    if picture_dir().is_some() {
        shoot(app, cam, name);
    } else {
        simulate(app, 2);
    }
}

/// Pixels that are clearly red (red fighters), blue (blue fighters) or brown (deer).
fn count(img: &image::RgbaImage, f: impl Fn(f32, f32, f32) -> bool) -> usize {
    count_below(img, 0, f)
}

/// [`count`], skipping the rows above `top` (the sky, which is as blue as a blue tabard).
fn count_below(img: &image::RgbaImage, top: u32, f: impl Fn(f32, f32, f32) -> bool) -> usize {
    img.enumerate_pixels()
        .filter(|(_, y, p)| *y >= top && f(p[0] as f32, p[1] as f32, p[2] as f32))
        .count()
}

/// The image row of the horizon for a level camera at `eye` looking at `target` (60° vertical
/// field of view, as [`camera`] makes): rows above it can only show sky or the tops of the
/// stands, never the arena floor.
fn horizon_row(eye: DVec3, target: DVec3, height: u32) -> u32 {
    let d = target - eye;
    let down = (-d.y).atan2(d.x.hypot(d.z));
    let half = (FOV_Y_DEG.to_radians() * 0.5).tan();
    let ndc = (down.tan() / half).clamp(-1.0, 1.0);
    ((1.0 - ndc) * 0.5 * height as f64).round() as u32
}

/// The screenshot cameras' vertical field of view.
const FOV_Y_DEG: f64 = 60.0;

#[test]
fn arena_and_meadow_screenshots() {
    let (w, h) = if picture_dir().is_some() {
        (W, H)
    } else {
        (CHECK_W, CHECK_H)
    };
    let mut app = app(w, h);
    let overview_at = (
        ARENA + DVec3::new(0.0, 16.0, -31.0),
        ARENA + DVec3::new(0.0, 0.0, 4.0),
    );
    let overview = camera(&mut app, overview_at.0, overview_at.1);
    let ring = ARENA + demo::DUEL_RING;
    let duel = camera(
        &mut app,
        ring + DVec3::new(-2.0, 2.4, -7.5),
        ring + DVec3::new(0.0, 1.0, 0.0),
    );
    let cover_at = (
        ARENA + DVec3::new(-15.0, 4.5, -4.0),
        ARENA + DVec3::new(2.0, 0.8, 9.0),
    );
    let cover = camera(&mut app, cover_at.0, cover_at.1);
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
    // Let the fight develop: squads spot each other, take cover, flank, close in. The first
    // shot starts the renderer (or finds there is no GPU).
    simulate(&mut app, 63);
    let Some(img) = shoot(&mut app, overview, "npc_arena_overview.png") else {
        return;
    };
    let cover_img = shoot(&mut app, cover, "npc_arena_cover.png").unwrap();
    // Both squads show up (counted over two views, so a column in front of one doesn't hide a
    // whole squad, and only below the horizon, so the sky doesn't count as the blue squad).
    let red = |img: &image::RgbaImage, at: (DVec3, DVec3)| {
        let top = horizon_row(at.0, at.1, img.height());
        count_below(img, top, |r, g, b| r > 70.0 && r > g * 1.7 && r > b * 1.7)
    };
    let blue = |img: &image::RgbaImage, at: (DVec3, DVec3)| {
        let top = horizon_row(at.0, at.1, img.height());
        count_below(img, top, |r, g, b| b > 70.0 && b > r * 1.5 && b > g * 1.2)
    };
    let red = red(&img, overview_at) + red(&cover_img, cover_at);
    let blue = blue(&img, overview_at) + blue(&cover_img, cover_at);
    eprintln!("team pixels below the horizon: red {red}, blue {blue}");
    assert!(red > 150, "red fighters visible: {red} px");
    assert!(blue > 150, "blue fighters visible: {blue} px");
    picture(&mut app, stands, "npc_arena_stands.png");
    picture(&mut app, sparring, "npc_sparring_lane.png");
    simulate(&mut app, 5);
    picture(&mut app, duel, "npc_arena_duel.png");
    let img = shoot(&mut app, meadow, "npc_wildlife_meadow.png").unwrap();
    picture(&mut app, herd, "npc_wildlife_herd.png");
    let brown = count(&img, |r, g, b| {
        r > 60.0 && r > g * 1.25 && g > b * 1.15 && r < 220.0
    });
    eprintln!("deer pixels: {brown}");
    assert!(brown > 100, "deer visible: {brown} px");
}
