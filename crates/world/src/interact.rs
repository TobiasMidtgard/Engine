//! Using things in the world: drive a car, man a gun, pull up a sail, fell a tree.
//!
//! The owner of a usable thing puts [`Interactable`] on it (what the prompt says, how close you
//! must be); the player's body carries [`Interactor`]. Every frame the engine picks the one the
//! player would use, [`InteractTarget`]: within its reach of the player, and of those the
//! nearest to where the camera looks. The screen shows its prompt (`E  Drive`), and the
//! `interact` action (E, gamepad West) writes an [`Interact`] message naming it. Its owner
//! reads the message and does the rest, so no module knows the others.
//!
//! While the player is busy with something (driving, at a gun), its owner puts [`Using`] on the
//! actor: the target stays that thing whatever is closer, so the next press goes back to it
//! (to get out); the owner changes its verb meanwhile ("Get out") and removes [`Using`] when done.
//!
//! This is the shape of Unreal Lyra's interaction (an `IInteractableTarget` offers options,
//! a scan picks one, an ability runs it) and of Unity's interactor / interactable pairs.
//!
//! With no [`Interactor`] anywhere (a free camera), the active camera is the actor.
//!
//! What is in use shows its numbers under the prompt through [`Readouts`] (speed, altitude).

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

/// Something the player can use. Its owner reads the [`Interact`] messages that name it.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interactable {
    /// What pressing the key does, shown in the prompt: "Drive", "Fire", "Pull up".
    pub verb: String,
    /// How close the player must be to the use point, meters.
    pub reach_m: f32,
    /// Where it is used, in the entity's own frame (a car's door, a gun's handles), meters.
    pub offset: Vec3,
    /// Off while it can't be used (a gun reloading, a door locked); the prompt hides.
    pub enabled: bool,
}

impl Default for Interactable {
    fn default() -> Self {
        Self {
            verb: "Use".into(),
            reach_m: 2.5,
            offset: Vec3::ZERO,
            enabled: true,
        }
    }
}

impl Interactable {
    pub fn new(verb: impl Into<String>) -> Self {
        Self {
            verb: verb.into(),
            ..Default::default()
        }
    }

    pub fn reach(mut self, meters: f32) -> Self {
        self.reach_m = meters;
        self
    }

    pub fn at(mut self, offset: Vec3) -> Self {
        self.offset = offset;
        self
    }
}

/// The body that uses things: the player's character (the movement module's pilot, a game's
/// walker). Several are allowed; the one nearest something usable acts.
#[derive(Component, Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Interactor;

/// On an actor busy with `target` (driving it, manning it). The target stays picked, at any
/// distance, until its owner removes this.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Using {
    pub target: Entity,
}

/// What the player would use now (written every frame, `Update`).
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct InteractTarget {
    pub actor: Option<Entity>,
    pub target: Option<Entity>,
    /// The target's verb, for the prompt.
    pub verb: String,
    pub distance_m: f32,
}

/// The player pressed `interact` with `target` picked. Its owner acts on it.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct Interact {
    pub actor: Entity,
    pub target: Entity,
    /// The actor's world position when it pressed.
    pub from: DVec3,
}

/// Lines the screen shows under the prompt while the player uses something: label and value
/// (a vehicle's speed, altitude, gear, cruise). The engine clears them at the start of every
/// frame (`First`), so whoever is in use writes its lines each frame and they go when it stops.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct Readouts {
    pub lines: Vec<(String, String)>,
}

impl Readouts {
    /// Sets the line `label` (replacing one of that label), keeping the order lines came in.
    pub fn set(&mut self, label: impl Into<String>, value: impl Into<String>) {
        let (label, value) = (label.into(), value.into());
        match self.lines.iter_mut().find(|l| l.0 == label) {
            Some(line) => line.1 = value,
            None => self.lines.push((label, value)),
        }
    }
}

/// One candidate for [`pick`]: the entity, its use point (world meters) and its reach.
pub type Candidate = (Entity, DVec3, f32);

/// The candidate the player would use: within reach of `actor`, and of those the one with the
/// smallest distance plus 2 m for each unit of `1 - cos` of its angle off the view (`eye`,
/// `forward`), so what you look at wins over what is a little closer behind you. Returns the
/// entity and its distance.
pub fn pick(
    actor: DVec3,
    view: Option<(DVec3, Vec3)>,
    candidates: impl IntoIterator<Item = Candidate>,
) -> Option<(Entity, f32)> {
    let mut best: Option<(f64, Entity, f32)> = None;
    for (e, point, reach) in candidates {
        let d = actor.distance(point);
        if d > reach as f64 {
            continue;
        }
        let off_view = view.map_or(0.0, |(eye, forward)| {
            let to = (point - eye).as_vec3().normalize_or_zero();
            if to == Vec3::ZERO {
                0.0
            } else {
                1.0 - to.dot(forward.normalize_or_zero()) as f64
            }
        });
        let score = d + 2.0 * off_view;
        if best.is_none_or(|(b, ..)| score < b) {
            best = Some((score, e, d as f32));
        }
    }
    best.map(|(_, e, d)| (e, d))
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<InteractTarget>()
        .init_resource::<Readouts>()
        .add_message::<Interact>()
        .register_component::<Interactable>(
            "Usable thing: verb (prompt text), reach_m, offset (use point, local m), enabled. \
             Its owner acts on Interact messages naming it.",
        )
        .register_component::<Interactor>(
            "The player's body: what uses Interactables (E). Without one, the camera does.",
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_what_is_in_reach_and_in_view() {
        let mut world = World::new();
        let [ahead, behind, far] = [(); 3].map(|_| world.spawn_empty().id());
        let at = DVec3::ZERO;
        let candidates = [
            (ahead, DVec3::new(0.0, 0.0, -2.0), 2.5),
            (behind, DVec3::new(0.0, 0.0, 1.5), 2.5),
            (far, DVec3::new(0.0, 0.0, -10.0), 2.5),
        ];
        // Looking ahead: the one in front wins although the one behind is closer.
        let view = Some((DVec3::new(0.0, 1.6, 0.0), Vec3::NEG_Z));
        assert_eq!(pick(at, view, candidates).map(|p| p.0), Some(ahead));
        // With no view, the nearest.
        assert_eq!(pick(at, None, candidates).map(|p| p.0), Some(behind));
        // A long reach brings the far one in, but it stays behind the near one in view.
        let long = [(far, DVec3::new(0.0, 0.0, -10.0), 20.0)];
        assert_eq!(pick(at, view, long).map(|p| p.0), Some(far));
        assert_eq!(pick(at, view, []), None);
    }
}
