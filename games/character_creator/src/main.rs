//! Character creator. `cargo run -p character_creator` opens the creator: drag with the left
//! mouse button to turn the person, scroll to zoom, and shape them with the panel's sliders,
//! presets and the random button.
//!
//! `cargo run -p character_creator --release -- --headless --size 1024x1024 --shots <dir>`
//! renders portrait sheets of every preset (front, three-quarter and profile) to `<dir>`;
//! `--only kofi,mei` limits them to some people, `--views front` to some views.

mod panel;
mod shots;
mod studio;

use engine::prelude::*;
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
        .add_systems(Update, studio::orbit_camera);
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

/// Eye level of the subject in world space once it is built (the camera aims there).
pub fn subject_eyes(
    q: &Query<(&Transform, &Appearance, Option<&BuiltCharacter>), With<Subject>>,
) -> Option<(DVec3, bool)> {
    let (t, a, built) = q.iter().next()?;
    let built = built?;
    let lm = built.meshes.landmarks?;
    let eyes = (lm.eyes[0] + lm.eyes[1]) * 0.5;
    Some((t.translation + eyes.as_dvec3(), built.appearance == *a))
}
