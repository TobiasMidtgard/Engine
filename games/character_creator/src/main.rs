//! Character creator. `cargo run -p character_creator` opens the creator: drag with the left
//! mouse button to turn the person, scroll to zoom, and shape them with the panel's sliders,
//! presets and the random button.
//!
//! `cargo run -p character_creator --release -- --headless --size 1024x1024 --shots <dir>`
//! renders portrait sheets of every preset (front, three-quarter and profile) to `<dir>`;
//! `--only kofi,mei` limits them to some people, `--views front` to some views (`body` and
//! `body34` for the whole person in `--pose walk`, `smile` or `surprise` for expressions).

mod panel;
mod pose;
mod shots;
mod studio;

use engine::prelude::*;
use engine::world::animation::SkeletonPose;
use engine_character::{Appearance, BuiltCharacter, CharacterPlugin};

fn main() {
    let mut app = App::new();
    engine::core::log::init("info,wgpu=warn,naga=warn");
    engine::apply_cli_args(&mut app);
    let shots = shots::ShotPlan::from_args();
    let headless = app.world.resource::<EngineConfig>().headless;
    app.add_plugins(DefaultPlugins)
        .add_plugins(CharacterPlugin)
        // Long hair swings and settles as the person moves.
        .add_plugins(engine_softbody::SoftBodyPlugin)
        .add_systems(Startup, studio::setup)
        .add_systems(
            Update,
            (pose::drive, studio::face_camera, studio::orbit_camera).chain(),
        );
    // Portraits keep full detail however far the camera pulls back; `--lod-scale s` shows the
    // game's detail by distance instead, as if the camera stood s times farther away.
    let lod_scale = std::env::args()
        .skip_while(|a| a != "--lod-scale")
        .nth(1)
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(0.0);
    app.insert_resource(engine_character::CharacterLod {
        distance_scale: lod_scale,
    });
    // `--no-subsurface`: the skin shader's imitation of light under the skin instead of the
    // renderer's subsurface pass (to compare the two).
    app.insert_resource(engine_character::CharacterSubsurface {
        pass: !std::env::args().any(|a| a == "--no-subsurface"),
    });
    // Indoors: still air, so hair hangs and swings only as the person moves.
    {
        let mut weather = app.world.resource_mut::<engine::world::WeatherState>();
        weather.wind = Vec3::ZERO;
        weather.gustiness = 0.0;
    }
    if let Some(plan) = shots {
        app.insert_resource(plan)
            .add_systems(Update, shots::run_shots.after(studio::orbit_camera));
    } else if !headless {
        app.add_plugins(engine_ui::UiPlugin)
            .add_systems(Startup, panel::setup.after(studio::setup))
            .add_systems(Update, (panel::apply_actions, panel::sync).chain());
    }
    app.run();
}

/// The person being edited.
#[derive(Component)]
pub struct Subject;

/// The subject's queried parts for [`subject_eyes`].
pub type SubjectParts<'a> = (
    &'a Transform,
    &'a Appearance,
    Option<&'a BuiltCharacter>,
    Option<&'a SkeletonPose>,
);

/// Eye level of the subject in world space once it is built (the camera aims there).
pub fn subject_eyes<'a>(mut q: impl Iterator<Item = SubjectParts<'a>>) -> Option<(DVec3, bool)> {
    let (t, a, built, skeleton) = q.next()?;
    let built = built?;
    let eyes = pose::eye_point(built, skeleton)?;
    Some((
        t.translation + (t.rotation * eyes).as_dvec3(),
        built.appearance == *a,
    ))
}
