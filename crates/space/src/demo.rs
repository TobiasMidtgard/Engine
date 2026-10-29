//! The solar system at real scale, ready to travel through.
//!
//! ```ignore
//! let bodies = engine_space::demo::spawn_solar_system(&mut commands);
//! // Give the bodies surfaces (engine_terrain), e.g.:
//! // commands.entity(bodies["Moon"]).insert(PlanetTerrain::moon(1).with_radius(1_737_400.0));
//! ```
//!
//! Earth anchors the world with the home spot (40° N, 0° E by default, see [`SpaceFrame`])
//! at the world origin. The Moon, the Sun and the planets move along their real orbits; the
//! sun rises and sets as Earth turns. `space.travel {"body": "Moon"}` jumps there, and flying
//! into a body's sphere of influence re-anchors the world on it.

use std::collections::HashMap;

use engine_core::math::DVec3;
use engine_core::prelude::*;

use crate::{CelestialBody, SpaceFrame};

/// Spawn the Sun, the eight planets and the Moon, anchored on Earth, and the stars nearest
/// the Sun to fly to. Returns the entities by name.
pub fn spawn_solar_system(commands: &mut Commands) -> HashMap<String, Entity> {
    let mut out = HashMap::new();
    for body in crate::system::solar_system()
        .into_iter()
        .chain(crate::system::nearby_stars())
        .chain(crate::nearby::planets(false))
    {
        let name = body.name.clone();
        let id = commands.spawn((body, Transform::default())).id();
        out.insert(name, id);
    }
    commands.queue(|world: &mut World| {
        let mut frame = world.resource_mut::<SpaceFrame>();
        if frame.anchor.is_empty() {
            frame.anchor = "Earth".into();
        }
    });
    out
}

/// The space showcase: the solar system (see [`spawn_solar_system`]). The system is global, so
/// `origin` is not used; the home spot is always at the world origin.
pub fn spawn_demo(commands: &mut Commands, _origin: DVec3) {
    spawn_solar_system(commands);
}

/// True if a body by this name exists (for idempotent setup).
pub fn has_body(world: &mut World, name: &str) -> bool {
    let mut q = world.query::<&CelestialBody>();
    q.iter(world).any(|b| b.name == name)
}
