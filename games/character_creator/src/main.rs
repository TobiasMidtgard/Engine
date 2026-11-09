//! Character creator. `cargo run -p character_creator` opens the creator: drag with the left
//! mouse button to turn the person, scroll to zoom, and shape them with the panel's sliders,
//! presets and the random button.
//!
//! `cargo run -p character_creator --release -- --headless --size 1024x1024 --shots <dir>`
//! renders portrait sheets of every preset (front, three-quarter and profile) to `<dir>`;
//! `--only kofi,mei` limits them to some people, `--views front` to some views (`body` and
//! `body34` for the whole person in `--pose walk`, `smile` or `surprise` for expressions).
//!
//! The Pose and Hands tabs are a posing tool: every joint by its anatomical movements within
//! its range of motion, every finger joint, hand shapes from the hand pose library, mirror,
//! undo and save slots (`designs/slot<n>.json`); the Body tab sets limb proportions. Shots take
//! `--hand fist,point` (or `all`), hand close-ups (`lhand`, `rhand`, `lhandfront`,
//! `lhandpalm`), `--design <file>` for a saved design, and `--ui --tab Pose` to show the panel.

mod joints;
mod panel;
mod pose;
mod posing;
mod props;
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
        .init_resource::<studio::CameraFocus>()
        .init_resource::<props::HeldProps>()
        .add_systems(Startup, studio::setup)
        .add_systems(
            Update,
            (pose::drive, studio::face_camera, studio::orbit_camera).chain(),
        )
        .add_systems(Update, props::update.after(pose::drive));
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
    // `--ui`: the panel in shots too (`--tab Pose` opens a tab), to show the tool itself.
    let ui = std::env::args().any(|a| a == "--ui");
    let shooting = shots.is_some();
    if let Some(plan) = shots {
        app.insert_resource(plan)
            .add_systems(Update, shots::run_shots.after(studio::orbit_camera));
    }
    if ui || (!headless && !shooting) {
        app.add_plugins(engine_ui::UiPlugin)
            .add_systems(
                Startup,
                (panel::setup, joints::setup).chain().after(studio::setup),
            )
            .add_systems(
                Update,
                (panel::apply_actions, panel::sync)
                    .chain()
                    .before(studio::orbit_camera),
            )
            .add_systems(Update, joints::update.after(studio::orbit_camera));
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

/// Where a bone of the subject is in world space, posed (its joint).
pub fn subject_point<'a>(
    mut q: impl Iterator<Item = SubjectParts<'a>>,
    bone: &str,
) -> Option<DVec3> {
    let (t, _, built, skeleton) = q.next()?;
    let p = pose::bone_point(built?, skeleton, bone)?;
    Some(t.translation + (t.rotation * p).as_dvec3())
}

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
