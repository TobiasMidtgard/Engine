//! Combinable magic (engine-design/spell-combination/magic-system.md, spell-combination.md):
//! a spell is data, woven from effects, shaped by a cast type and modifiers, chained by
//! stages, priced by complexity against a mastery budget, and run by a few shared kernels.
//!
//! | Module | What |
//! |---|---|
//! | [`dials`] | the seven domains and fourteen poles every effect is made of |
//! | [`schools`] | elements, statuses, classes and subclasses, mastery tiers, power |
//! | [`effects`] | the 26 effects |
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
//! | [`summon`] | summoned golems, elementals and wisps that walk or float, fight and fall |
//! | [`tether`] | tethers: a link to one body that works each tick and snaps when stretched |
//! | [`space`] | portals you see through and pass through, blink to the aim point, recall along your path |
//! | [`wards`] | wards you can see: shells, domes and ward walls that crack and shatter |
//! | [`undead`] | raise dead: thralls from fallen bodies, skeleton warriors and mages from the ground |
//! | [`minds`] | mirror images and decoys, command and wrath, fear that sends creatures running |
//! | [`blades`] | conjured blades (glintblade, phalanx, bladefall, spirit blade, bound blade) and imbued weapons |
//! | [`looks`] | how spells look, through the particles module's API |
//! | [`player`] | the player's spell bar: keys 1-8, camera aim, charge, channel, place, HUD |
//! | [`crafter`] | the Spellcrafter: an altar where you build a spell in four steps, see its cost, watch it fly at a dummy and put it on a key |
//! | [`api`] | `magic.*` |
//! | [`demo`] | the Weaving hall |
//!
//! It talks to the rest of the engine only through `engine_world` contracts (damage,
//! impulses, ignition, smoke, noise, projectile traces, attack telegraphs, time scales) and
//! the particles module's public API.

pub mod api;
pub mod blades;
pub mod caster;
pub mod cataclysm;
pub mod combine;
pub mod compile;
pub mod crafter;
pub mod demo;
pub mod design;
pub mod dials;
pub mod effects;
pub mod growth;
pub mod hits;
pub mod kinesis;
pub mod looks;
pub mod meet;
pub mod minds;
pub mod player;
pub mod runtime;
pub mod schools;
pub mod space;
pub mod status;
pub mod stone;
pub mod summon;
pub mod tether;
pub mod undead;
pub mod walls;
pub mod wards;

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
            .init_resource::<player::PlayerMagic>()
            .init_resource::<crafter::Crafter>()
            .init_resource::<wards::WardMeshes>()
            .init_resource::<space::GateMeshes>()
            .add_message::<wards::WardStruck>()
            .add_message::<engine_world::interact::Interact>()
            .register_component::<MagicCaster>(
                "Someone who can cast: mana, level (power), skill per subclass, corruption, \
                 grimoire (spells with growth paths), selected spell, side, hand position. \
                 magic.caster reads and sets it.",
            )
            .register_component::<player::SpellBar>(
                "The player's spell bar: the grimoire spell on each of eight slots (keys 1-8), \
                 each slot's cooldown and its total.",
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
                    player::drive_spell_bars,
                    runtime::begin_casts,
                    runtime::advance_casts,
                    player::flash_refused,
                    space::travel,
                    space::pass_gates,
                    kinesis::grip,
                    kinesis::fly_hurled,
                )
                    .chain()
                    .in_set(MagicSet::Cast),
            )
            .add_systems(Startup, player::bind_actions)
            .add_systems(
                Update,
                (
                    player::equip_player,
                    player::read_spell_keys,
                    player::show_place_rings,
                    crafter::open_at_altars,
                    crafter::run_crafter,
                    crafter::tend_altars,
                    player::write_hud,
                    tether::draw_tethers,
                    kinesis::draw_grips,
                    minds::draw_threads,
                )
                    .chain(),
            )
            .add_systems(Last, crafter::free_cursor)
            .add_systems(
                FixedUpdate,
                (
                    (
                        wards::hold_shields,
                        runtime::fly_missiles,
                        runtime::tick_fields,
                        runtime::tick_beams,
                        runtime::tick_chains,
                        runtime::tick_rain,
                        cataclysm::spawn_cataclysm,
                        cataclysm::fall_meteors,
                        cataclysm::run_faults,
                        cataclysm::apply_blasts,
                        minds::spawn_phantasms,
                        minds::run_phantasms,
                    )
                        .chain(),
                    (
                        blades::spawn_blades,
                        blades::run_blades,
                        blades::ride_blades,
                        blades::strike_imbued,
                    )
                        .chain(),
                    kinesis::guard_rings,
                    meet::meet_spells,
                    undead::raise_dead,
                    summon::run_minions,
                    undead::run_thralls,
                )
                    .chain()
                    .in_set(MagicSet::Carriers),
            )
            .add_systems(
                FixedUpdate,
                (
                    hits::apply_hits,
                    minds::take_minds,
                    minds::tend_minds,
                    hits::take_damage,
                    hits::recover,
                    status::tick_afflictions,
                    wards::tend_wards,
                    space::tend_gates,
                    runtime::follow_looks,
                    runtime::tend_pieces,
                    walls::rise_walls,
                    stone::raise_stone,
                    summon::settle_rubble,
                    cataclysm::tend_cataclysm,
                    blades::tend_imbued,
                )
                    .chain()
                    .in_set(MagicSet::Hits),
            )
            .add_systems(FixedUpdate, runtime::flush.in_set(MagicSet::Flush))
            .add_systems(
                PostUpdate,
                (runtime::sync_transforms, blades::hold_blades).before(UpdateSet::Propagate),
            )
            .add_systems(
                PostUpdate,
                (
                    runtime::follow_world_shift,
                    summon::follow_world_shift,
                    cataclysm::follow_world_shift,
                    blades::follow_world_shift,
                )
                    .in_set(WorldShiftSet::Apply)
                    .before(runtime::sync_transforms),
            );
        api::register(app);
        crafter::register(app);
        wards::register(app);
        space::register(app);
        kinesis::register(app);
        cataclysm::register(app);
        minds::register(app);
        blades::register(app);
        demo::build(app);
    }
}
