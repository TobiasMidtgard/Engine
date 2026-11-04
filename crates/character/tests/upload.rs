//! A person's meshes reach the GPU on a thread of their own in a windowed game, and in the
//! frame headless (so pictures show everyone asked for). Needs a GPU or lavapipe and the
//! fetched human pack (`cargo run -p fetch-assets`); passes with a note without either.

use std::path::Path;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, RenderStats};
use engine_world::character::Appearance;

use engine_character::{
    BuiltCharacter, CharacterBuild, CharacterPlugin, HumanAssets, HumanPack, RegionName,
};

fn scene(pack: &[u8]) -> App {
    let mut app = App::new();
    app.world.insert_resource(HumanAssets::ready(
        HumanPack::parse(pack).expect("the human pack"),
    ));
    // The GPU starts on the first frame only headless or with a window: start it headless,
    // then say it's a game (`frames_to_draw`).
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 320;
        w.height = 180;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((RenderPlugin, engine_world::WorldPlugin, CharacterPlugin));
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.3, -0.8, -0.5), Vec3::Y),
    ));
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.5 },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 0.1, 0.9).looking_at(DVec3::new(0.0, 0.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        Appearance::default(),
        CharacterBuild {
            region: RegionName::Head,
            levels: 0,
            hair: false,
        },
    ));
    app
}

/// Frames from the person's meshes being built until they are drawn (`None`: not within the
/// limit), or `None` with a note when the test can't run here.
fn frames_to_draw(headless: bool, pack: &[u8]) -> Option<Option<u32>> {
    let mut app = scene(pack);
    app.update();
    if let Some(r) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipped: no GPU ({})", r.0);
        return None;
    }
    app.world.resource_mut::<EngineConfig>().headless = headless;
    // The pack loads and the person builds on threads of their own.
    let started = std::time::Instant::now();
    loop {
        let built = app
            .world
            .query::<&BuiltCharacter>()
            .iter(&app.world)
            .next()
            .is_some();
        if built {
            break;
        }
        if started.elapsed().as_secs() > 120 {
            panic!("the person was never built");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.update();
    }
    for frame in 0..200 {
        app.update();
        let drawn = app
            .world
            .get_resource::<RenderStats>()
            .is_some_and(|s| s.instances > 0);
        if drawn {
            return Some(Some(frame));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Some(None)
}

fn pack() -> Option<Vec<u8>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(engine_character::pack::PACK_PATH);
    let bytes = std::fs::read(path).ok();
    if bytes.is_none() {
        eprintln!("skipped: the human pack isn't fetched (cargo run -p fetch-assets)");
    }
    bytes
}

#[test]
fn a_person_is_uploaded_off_the_frame_in_a_game_and_in_it_headless() {
    let Some(pack) = pack() else {
        return;
    };
    let Some(headless) = frames_to_draw(true, &pack) else {
        return;
    };
    assert_eq!(
        headless,
        Some(0),
        "headless, drawn the frame after it is built"
    );
    let game = frames_to_draw(false, &pack).expect("the GPU was there a moment ago");
    assert!(game.is_some(), "in a game, drawn once its upload is in");
}
