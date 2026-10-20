//! Per-body random seeds that don't come from `Entity` ids.
//!
//! Every NPC, animal and sparring post draws its randomness (think pace, strafe side, reaction
//! jitter, block rolls, shot scatter, an animal's needs and wanderings, pose variety) from its
//! [`Seed`]. A seed is a hash of where the body was spawned and what it is, so one more entity
//! or resource anywhere in the app (bevy stores each resource on an entity, which moves every
//! later id) changes nothing. Bodies spawned on the same spot of the same kind get different
//! seeds, in spawn order. Insert a `Seed` yourself to choose one.

use std::collections::HashMap;

use engine_core::prelude::*;

use crate::agent::Npc;
use crate::training::SparringPost;
use crate::wildlife::Animal;

/// A body's random seed, given once on its first step (see the module docs).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Seed(pub u64);

impl Seed {
    /// The same 0..1 value every time for this body.
    pub fn unit(self) -> f32 {
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// How many seeds each spot and kind has given, so bodies spawned on the same spot differ.
#[derive(Resource, Default)]
pub(crate) struct SeedsGiven(HashMap<u64, u64>);

/// `e`'s seed (0 before its first step).
pub(crate) fn seed_of(seeds: &Query<&Seed>, e: Entity) -> u64 {
    seeds.get(e).map_or(0, |s| s.0)
}

/// Bodies that draw randomness and have no seed yet: where they stand and what they are.
type Unseeded<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        Has<Npc>,
        Option<&'static Animal>,
    ),
    (
        Or<(With<Npc>, With<Animal>, With<SparringPost>)>,
        Without<Seed>,
    ),
>;

/// Give each new NPC, animal and sparring post its [`Seed`].
pub(crate) fn seed_bodies(mut commands: Commands, mut given: ResMut<SeedsGiven>, q: Unseeded) {
    for (e, t, npc, animal) in &q {
        let kind = match (npc, animal) {
            (true, _) => 1,
            (_, Some(a)) => 16 + a.species as u64,
            _ => 2,
        };
        let p = t.translation;
        let spot = hash_u64(p.x.to_bits() ^ hash_u64(p.y.to_bits() ^ hash_u64(p.z.to_bits())));
        let base = hash_u64(spot ^ kind.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let n = given.0.entry(base).or_default();
        let seed = hash_u64(base ^ n.wrapping_mul(0xbf58_476d_1ce4_e5b9));
        *n += 1;
        commands.entity(e).try_insert(Seed(seed));
    }
}
