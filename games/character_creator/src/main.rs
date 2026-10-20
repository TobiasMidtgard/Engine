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
        .add_systems(Startup, studio::setup)
        .add_systems(
            Update,
            (pose::drive, studio::face_camera, studio::orbit_camera).chain(),
        );
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
