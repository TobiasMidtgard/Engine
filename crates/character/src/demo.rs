//! A row of preset people for the showcase: busts on plinths, facing the visitor.

use engine_core::prelude::*;

use crate::appearance::Appearance;

/// Spawn every preset as a bust in a row along +X at `origin` (facing +Z), 0.6 m apart, with
/// their heads at about eye height.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let presets = Appearance::presets();
    let n = presets.len() as f64;
    for (i, (name, a)) in presets.into_iter().enumerate() {
        let x = (i as f64 - (n - 1.0) * 0.5) * 0.6;
        // The base mesh's head is ~0.73 m above its origin; lift it to eye height.
        commands.spawn((
            Name::new(format!("character {name}")),
            Transform::from_translation(origin + DVec3::new(x, 0.9, 0.0)),
            a,
        ));
    }
}
