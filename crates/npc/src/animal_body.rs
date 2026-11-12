//! Animals in the animation module's bodies (characters week A4). Deer, wolves, boars and
//! rabbits ask it for a body (a `BodyRequest` of their kind, with their seed and, for the young
//! and for big males, their shoulder height). The animal brain moves them as before (their
//! `Transform`, the animation module picks the gait from the speed). It also tells the body
//! what the animal is doing: `alert` (head up, ears forward), `graze` (head down feeding) and
//! `lie` (lying down) as `AnimationParams`, and `flinch` (hurt), `rear` (a deer bolting from
//! something close) and `howl` (a wolf) as triggers.
//!
//! While no module answers (no `SkeletonPose` appears), the sculpted stand-in body
//! (`visuals/animal.rs`) is drawn and posed as before. Once one does, the stand-in and its fur
//! coat are hidden, and they come back if the body goes away. Starlings keep the stand-in: a
//! flying body needs the module's wing rig, a later round.

use engine_core::prelude::*;
use engine_world::animation::BodyRequest;

use crate::agent::NpcSettings;
use crate::seed::Seed;
use crate::wildlife::{Animal, Species};

/// The animation module's body kind for `species` (`None`: it keeps the stand-in).
pub fn kind_of(species: Species) -> Option<&'static str> {
    match species {
        Species::Deer => Some("deer"),
        Species::Wolf => Some("wolf"),
        Species::Boar => Some("boar"),
        Species::Rabbit => Some("rabbit"),
        Species::Bird => None,
    }
}

/// The shoulder height to ask for, m: `None` (the module's typical adult) but for the young
/// (half grown) and the big males (a stag, a tusker).
pub fn withers(animal: &Animal) -> Option<f32> {
    let adult = match animal.species {
        Species::Deer => 1.2,
        Species::Wolf => 0.8,
        Species::Boar => 0.85,
        Species::Rabbit => 0.2,
        Species::Bird => return None,
    };
    if animal.young {
        Some(adult * 0.5)
    } else if animal.male && matches!(animal.species, Species::Deer | Species::Boar) {
        Some(adult * 1.1)
    } else {
        None
    }
}

/// The body `animal` asks for.
pub fn request(animal: &Animal, seed: u64) -> Option<BodyRequest> {
    let kind = kind_of(animal.species)?;
    Some(BodyRequest {
        height: withers(animal),
        ..BodyRequest::new(kind).with_seed(seed)
    })
}

/// Animals that may ask, with what they asked for.
pub(crate) type Asking<'a> = (
    Entity,
    &'a Animal,
    Option<&'a Seed>,
    Option<&'a BodyRequest>,
    Has<AnimalBody>,
);

/// This crate gave the animal its `BodyRequest` (and takes it away again).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct AnimalBody;

/// Give and take away animals' `BodyRequest`s (`NpcSettings::anim_bodies`). Run by the system
/// that asks for fighters' bodies, so its commands add no sync point of their own.
pub(crate) fn give_animal_bodies(
    commands: &mut Commands,
    settings: &NpcSettings,
    q: &Query<Asking, Without<crate::Npc>>,
) {
    for (e, animal, seed, asked, ours) in q {
        let want = settings.anim_bodies && kind_of(animal.species).is_some();
        match (want, asked) {
            (true, None) => {
                let Some(req) = request(animal, seed.map_or(0, |s| s.0)) else {
                    continue;
                };
                commands.entity(e).try_insert((req, AnimalBody));
            }
            (false, Some(_)) if ours => {
                commands.entity(e).try_remove::<(BodyRequest, AnimalBody)>();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deer_wolves_boars_and_rabbits_ask_birds_keep_the_stand_in() {
        for s in Species::ALL {
            let r = request(&Animal::new(s), 7);
            assert_eq!(r.is_some(), s != Species::Bird, "{s:?}");
        }
        let fawn = Animal {
            young: true,
            ..Animal::new(Species::Deer)
        };
        let doe = Animal::new(Species::Deer);
        let stag = Animal {
            male: true,
            ..Animal::new(Species::Deer)
        };
        assert_eq!(request(&doe, 7).unwrap().kind, "deer");
        assert_eq!(request(&doe, 7).unwrap().seed, 7);
        assert_eq!(withers(&doe), None);
        assert!(withers(&fawn).unwrap() < withers(&stag).unwrap());
    }
}
