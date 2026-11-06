//! Combinable magic (engine-design/spell-combination/magic-system.md, spell-combination.md):
//! a spell is data, woven from effects, shaped by a cast type and modifiers, chained by
//! stages, priced by complexity against a mastery budget, and run by a few shared kernels.
//!
//! | Module | What |
//! |---|---|
//! | [`dials`] | the seven domains and fourteen poles every effect is made of |
//! | [`schools`] | elements, statuses, classes and subclasses, mastery tiers, power |
//! | [`effects`] | the 22 effects |
//! | [`design`] | `SpellDesign`: effects + cast type + modifiers + stages, as JSON |
//! | [`combine`] | the combination table and the systemic fallback |
//! | [`compile`] | from a design and a caster to the numbers a spell flies with |
//! | [`growth`] | known spells growing with mastery |
//! | [`caster`] | casters, health for dummies, cast requests |
//! | [`runtime`] | casting, and the carriers: missiles, fields, beams, chains, rain |
//! | [`hits`] | what happens where a spell lands |
//! | [`status`] | element gauges, reactions, statuses |
//! | [`meet`] | spells that meet |
//! | [`stone`] | Shaping: stone walls, pillars, steps and grips that are physics bodies |
//! | [`looks`] | how spells look, through the particles module's API |
//! | [`api`] | `magic.*` |
//! | [`demo`] | the Weaving hall |
//!
//! It talks to the rest of the engine only through `engine_world` contracts (damage,
//! impulses, ignition, smoke, noise, projectile traces, attack telegraphs, time scales) and
//! the particles module's public API.

pub mod api;
pub mod caster;
pub mod combine;
pub mod compile;
pub mod demo;
pub mod design;
pub mod dials;
pub mod effects;
pub mod growth;
pub mod hits;
pub mod looks;
pub mod meet;
pub mod runtime;
pub mod schools;
pub mod status;
pub mod stone;
pub mod walls;

use engine_core::prelude::*;
use engine_world::WorldShiftSet;

pub use caster::{
    Aim, CastOutcome, CastRequest, Casting, KnownSpell, MagicCaster, MasteryGained, SpellCast,
    SpellChoice, Vitality,
};
pub use combine::{Recipe, Weave, weave};
pub use compile::{CastContext, Spell, compile};
pub use design::{CastType, Modifier, SpellDesign, Stage, TargetFilter, Trigger};
pub use growth::{Growth, grow};
pub use hits::SpellHit;
pub use meet::SpellsMet;
pub use schools::{Element, Mastery, Power, Status, Subclass};
pub use status::Afflictions;

/// System sets, in order inside the fixed step.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum MagicSet {
    /// Gather and release casts (in `SimSet::Gameplay`).
    Cast,
    /// Move carriers and find what they reach (in `SimSet::PostPhysics`).
    Carriers,
    /// Apply hits, run statuses.
    Hits,
    /// Spawn, despawn, show.
    Flush,
}

/// Magic: casting, carriers, hits, statuses, looks, the `magic.*` API and the Weaving hall.
#[derive(Default)]
pub struct MagicPlugin;

impl Plugin for MagicPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<CastRequest>()
            .add_message::<SpellCast>()
            .add_message::<MasteryGained>()
            .add_message::<SpellHit>()
            .add_message::<SpellsMet>()
            .init_resource::<runtime::HitQueue>()
            .init_resource::<runtime::MagicQueue>()
            .init_resource::<runtime::CastStarts>()
            .init_resource::<looks::LookLibrary>()
            .register_component::<MagicCaster>(
                "Someone who can cast: mana, level (power), skill per subclass, corruption, \
                 grimoire (spells with growth paths), selected spell, side, hand position. \
                 magic.caster reads and sets it.",
            )
            .register_component::<Vitality>(
                "Health for things only magic keeps health for (practice dummies, blood-price \
                 casters): health, max, regen, revive_secs.",
            )
            .configure_sets(
                FixedUpdate,
                (MagicSet::Carriers, MagicSet::Hits, MagicSet::Flush)
                    .chain()
                    .in_set(SimSet::PostPhysics),
            )
            .configure_sets(FixedUpdate, MagicSet::Cast.in_set(SimSet::Gameplay))
            .add_systems(
                FixedUpdate,
                (
                    runtime::regen_mana,
                    runtime::begin_casts,
                    runtime::advance_casts,
                )
                    .chain()
                    .in_set(MagicSet::Cast),
            )
            .add_systems(
                FixedUpdate,
                (
                    (
                        runtime::fly_missiles,
                        runtime::tick_fields,
                        runtime::tick_beams,
                        runtime::tick_chains,
                        runtime::tick_rain,
                    )
                        .chain(),
                    meet::meet_spells,
                )
                    .chain()
                    .in_set(MagicSet::Carriers),
            )
            .add_systems(
                FixedUpdate,
                (
                    hits::apply_hits,
                    hits::take_damage,
                    hits::recover,
                    status::tick_afflictions,
                    runtime::follow_looks,
                    runtime::tend_pieces,
                    walls::rise_walls,
                    stone::raise_stone,
                )
                    .chain()
                    .in_set(MagicSet::Hits),
            )
            .add_systems(FixedUpdate, runtime::flush.in_set(MagicSet::Flush))
            .add_systems(
                PostUpdate,
                runtime::sync_transforms.before(UpdateSet::Propagate),
            )
            .add_systems(
                PostUpdate,
                runtime::follow_world_shift
                    .in_set(WorldShiftSet::Apply)
                    .before(runtime::sync_transforms),
            );
        api::register(app);
        demo::build(app);
    }
}
