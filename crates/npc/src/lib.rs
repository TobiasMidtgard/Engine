#![doc = include_str!("../README.md")]

pub mod agent;
mod api;
pub mod art;
pub mod brain;
pub mod combat;
pub mod demo;
pub mod faction;
pub mod features;
pub mod learning;
pub mod locomotion;
pub mod nav;
pub mod perception;
pub mod seed;
mod shift;
pub mod skills;
pub mod smoke;
pub mod squad;
pub mod tactics;
pub mod training;
pub mod visuals;
pub mod wildlife;
pub mod world_model;

pub use agent::{
    AlertLevel, AttackPhase, Defense, Npc, NpcAction, NpcBrain, NpcClock, NpcCombat, NpcMotor,
    NpcOrder, NpcRole, NpcSettings, NpcVitals, NpcWeapon, WeaponStyle,
};
pub use combat::Downed;
pub use faction::{Faction, Factions, Squad};
pub use features::ExplosiveProp;
pub use learning::{OpponentModel, OpponentModels};
pub use locomotion::{FootKind, Footfall, MoverState, SelfPropelled};
pub use nav::{NavAgent, NavGrid, NavSettings, PathStatus};
pub use perception::{BodyHeight, Known, NpcMemory, Perceivables};
pub use seed::Seed;
pub use skills::{NpcSkills, Perception};
pub use squad::{AttackTokens, ReviveClaims, SquadSettings};
pub use training::SparringPost;
pub use visuals::{IntentMarker, Part, StandIn, StandInPart};
pub use wildlife::{
    Animal, AnimalMind, AnimalState, BirdFlight, Herd, PackRole, Species, WildlifeSettings,
};
pub use world_model::{CoverPoint, TacticalWorld, WorldModelSettings};

use engine_core::prelude::*;
use engine_world::WorldShiftSet;

/// System sets of the NPC module, in `FixedUpdate` (all inside `SimSet::Input`, except the
/// fallback mover and hit resolution in `SimSet::Gameplay`).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NpcSet {
    /// World model, clocks, perceivables.
    Prepare,
    /// Senses and memory.
    Perceive,
    /// Squads, decisions, movement intents, attack starts, defenses.
    Decide,
    /// Fallback mover, hit resolution, damage.
    Act,
    /// Pose stand-in bodies and intent markers (`SimSet::PostPhysics`).
    Show,
}

/// NPC AI: perception, memory, tactics, squads, combat, wildlife. See the crate docs.
pub struct NpcPlugin;

impl Plugin for NpcPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(faction::default_factions())
            .init_resource::<NpcSettings>()
            .init_resource::<SquadSettings>()
            .init_resource::<WorldModelSettings>()
            .init_resource::<TacticalWorld>()
            .init_resource::<NavGrid>()
            .init_resource::<Perceivables>()
            .init_resource::<AttackTokens>()
            .init_resource::<ReviveClaims>()
            .init_resource::<smoke::SmokeClouds>()
            .init_resource::<OpponentModels>()
            .init_resource::<WildlifeSettings>()
            .init_resource::<seed::SeedsGiven>()
            .register_component::<Npc>(
                "An AI fighter: home and patrol radius, active flag, preferred range override. \
                 Adds skills, perception, weapon, vitals, faction and runtime state.",
            )
            .register_component::<NpcSkills>(
                "NPC skills: reaction_time (s), accuracy, logic (tactics), aggression, courage, \
                 defense (block/parry/dodge), awareness, teamwork, learning, agility. 0..1.",
            )
            .register_component::<Perception>(
                "Senses: sight range (m), field of view (deg), hearing multiplier, eye height, \
                 seconds to detect, memory seconds.",
            )
            .register_component::<NpcWeapon>(
                "Weapon: melee or ranged, damage, reach/range (m), windup/recovery/cooldown (s), \
                 spread (deg), projectile speed, shield.",
            )
            .register_component::<NpcVitals>("Health, poise (stagger balance 0..1), downed flag.")
            .register_component::<NpcMotor>(
                "Body and speeds: walk/run/sprint (m/s), acceleration, turn rate, dash, radius, height.",
            )
            .register_component::<Faction>("Team id. Put it on the player too (0 = player).")
            .register_component::<Squad>(
                "Squad id: squad mates share sightings, take turns attacking, suppress and flank.",
            )
            .register_component::<ExplosiveProp>(
                "Explodes when damaged: radius (m), damage, health. NPCs shoot it near enemies.",
            )
            .register_component::<Animal>(
                "An animal: species (deer, rabbit, wolf, bird), home, roaming range (m), \
                 skittishness (flight distance multiplier), male. Body, footsteps, faction and \
                 health come from the species.",
            )
            .register_component::<StandIn>(
                "Stand-in body posed from the AI state: color (sRGB hex, default from faction or \
                 species), plain (no alert orb or skill band).",
            )
            .register_component::<SparringPost>(
                "Training post that swings at `target` on an uneven beat; counts blocks, parries, dodges and hits.",
            )
            .register_component::<Herd>(
                "Herd id: animals with the same id graze, move and flee together (herd, pack, flock).",
            )
            .register_component::<Footfall>(
                "Footsteps sent by the NPC fallback mover: foot kind, mass (kg), print size, stride.",
            )
            .register_resource::<NpcSettings>(
                "NPC module settings: built-in locomotion, hit resolution, think rate, LOD distance, \
                 intent overlay, showcase revive timer (s), squad mates reviving the downed \
                 (ally_revive_secs, 0 = off; ally_revive_health 0..1; ally_revive_range m).",
            )
            .register_resource::<WildlifeSettings>(
                "Wildlife: enabled, respawn_secs (killed animals return), thirst_secs, hunger_secs \
                 (wolf packs hunt again), skittishness (all flight distances), howl.",
            )
            .register_resource::<Factions>("Which factions are allied and which never attack.")
            .register_resource::<SquadSettings>(
                "Squad tactics: max simultaneous melee attackers, attack-turn length, callout range.",
            )
            .configure_sets(
                FixedUpdate,
                (NpcSet::Prepare, NpcSet::Perceive, NpcSet::Decide)
                    .chain()
                    .in_set(SimSet::Input),
            )
            .configure_sets(FixedUpdate, NpcSet::Act.in_set(SimSet::Gameplay))
            .configure_sets(FixedUpdate, NpcSet::Show.in_set(SimSet::PostPhysics))
            .add_systems(
                FixedUpdate,
                (
                    (
                        seed::seed_bodies,
                        visuals::character::request_appearances,
                        world_model::rebuild_world_model,
                        wildlife::init_animals,
                        perception::tick_clocks,
                        perception::collect_perceivables,
                    )
                        .chain()
                        .in_set(NpcSet::Prepare),
                    // Both touch `NpcVitals` (fighters' and animals'), so they are chained:
                    // nothing in this schedule runs in an order the scheduler picks
                    // (`tests/schedule_order.rs`).
                    (perception::perceive, wildlife::animal_senses)
                        .chain()
                        .in_set(NpcSet::Perceive),
                    (
                        squad::plan_squads,
                        combat::notice_attacks,
                        combat::defend,
                        brain::think_and_act,
                        wildlife::animal_think,
                        wildlife::animal_bites,
                    )
                        .chain()
                        .in_set(NpcSet::Decide),
                    (
                        locomotion::fallback_move,
                        combat::resolve_attacks,
                        combat::apply_damage,
                        features::explosives,
                        combat::revive,
                        smoke::smoke_clouds,
                        training::swing_posts,
                    )
                        .chain()
                        .in_set(NpcSet::Act),
                    (
                        visuals::build_stand_ins,
                        visuals::pose_stand_ins,
                        visuals::character::write_bodies,
                        visuals::character::wear,
                        visuals::intent_markers,
                        training::build_posts,
                        training::pose_posts,
                        art::arena::flicker_fires,
                        smoke::show_clouds,
                    )
                        .chain()
                        .in_set(NpcSet::Show),
                ),
            )
            // Re-anchoring: what NPCs and animals keep in world space follows the world.
            .add_systems(
                PostUpdate,
                (
                    shift::shift_agents,
                    shift::shift_wildlife,
                    shift::shift_world_model,
                )
                    .in_set(WorldShiftSet::Apply),
            );
        api::register(app);
    }
}
