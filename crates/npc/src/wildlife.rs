//! Dynamic fauna: deer and rabbits that graze, wander as a herd, drink at water, bed down and
//! bolt when something comes near (deer and boar keep the day's hours: out at dawn and dusk to
//! drink, grazing through the morning and afternoon, bedded at midday and in the dark); wild
//! boar that root about in sounders (sows with their young) or alone (males) and run, but turn and charge with their tusks when cornered or
//! wounded, or when a sow's young are threatened; wolf packs that roam and hunt together (one
//! tests the herd and singles out the weakest, two flank it, one lies in wait ahead; they give
//! up a long chase and rest), then feed; bird flocks that peck on the ground and take off
//! together.
//!
//! Animals share the NPC module's senses budget (`Perceivables`), tactical world, nav grid and
//! fallback mover, and run on their own `NpcClock`, so slow motion slows them too.
//!
//! What they react to:
//! * anything of another faction that comes inside their flight distance, which grows when it
//!   runs or attacks and shrinks a lot when it sneaks (crouched), so a careful player can get
//!   close to a grazing herd;
//! * scent carried by the wind: deer and wolves smell what is upwind even without seeing it;
//! * being hurt, loud noises (gunfire, explosions, howls) and fire (`HeatField`);
//! * each other: when one herd member bolts the rest follow a moment later (a deer snorts the
//!   alarm, and each of the others looks toward it before it bolts; the herd runs bunched),
//!   alert deer warn the herd, and one bird taking off lifts the whole flock; a deer with a
//!   wolf at its heels zigzags; wolves give a charging boar room.

use std::collections::HashMap;

use engine_core::math::hash_u64;
use engine_core::prelude::*;
use engine_world::animation::{AnimationParams, AnimationTrigger};
use engine_world::locomotion::{LocomotionMode, LocomotionState, MoveIntent};
use engine_world::{
    DamageEvent, DamageKind, Ground, GroundWear, HeatField, NoiseEvent, ProjectileTrace, Water,
    WeatherState, WindField, WorldImpulse,
};

use crate::agent::{NpcClock, NpcMotor, NpcVitals};
use crate::faction::Faction;
use crate::locomotion::{FootKind, Footfall, MoverState, SelfPropelled};
use crate::nav::{NavAgent, NavGrid, PathStatus};
use crate::perception::{BodyHeight, Perceivables};
use crate::world_model::TacticalWorld;

/// Kinds of animal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Species {
    #[default]
    Deer,
    Rabbit,
    Wolf,
    Bird,
    /// Wild boar (Sus scrofa).
    Boar,
}

impl Species {
    pub const ALL: [Species; 5] = [
        Species::Deer,
        Species::Rabbit,
        Species::Wolf,
        Species::Bird,
        Species::Boar,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Species::Deer => "deer",
            Species::Rabbit => "rabbit",
            Species::Wolf => "wolf",
            Species::Bird => "bird",
            Species::Boar => "boar",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Species::ALL.into_iter().find(|s| s.name() == name)
    }

    pub fn faction(self) -> Faction {
        match self {
            Species::Wolf => Faction::PREDATOR,
            _ => Faction::PREY,
        }
    }

    /// Hunted by wolves (boars fight back).
    pub fn is_prey(self) -> bool {
        matches!(self, Species::Deer | Species::Rabbit | Species::Boar)
    }

    fn traits(self) -> Traits {
        match self {
            Species::Deer => Traits {
                sight: 55.0,
                hearing: 1.3,
                smell: 70.0,
                flight: 20.0,
                health: 60.0,
                stamina_drain: 1.0 / 9.0,
                stamina_regen: 1.0 / 14.0,
                bite: 0.0,
            },
            Species::Rabbit => Traits {
                sight: 28.0,
                hearing: 1.6,
                smell: 0.0,
                flight: 8.0,
                health: 12.0,
                stamina_drain: 1.0 / 6.0,
                stamina_regen: 1.0 / 8.0,
                bite: 0.0,
            },
            Species::Wolf => Traits {
                sight: 65.0,
                hearing: 1.5,
                smell: 90.0,
                flight: 13.0,
                health: 70.0,
                stamina_drain: 1.0 / 26.0,
                stamina_regen: 1.0 / 10.0,
                bite: 20.0,
            },
            Species::Bird => Traits {
                sight: 30.0,
                hearing: 1.4,
                smell: 0.0,
                flight: 8.0,
                health: 5.0,
                stamina_drain: 0.0,
                stamina_regen: 1.0,
                bite: 0.0,
            },
            // Poor eyes, a keen nose; the bite is a slash of the tusks.
            Species::Boar => Traits {
                sight: 32.0,
                hearing: 1.5,
                smell: 90.0,
                flight: 16.0,
                health: 90.0,
                stamina_drain: 1.0 / 12.0,
                stamina_regen: 1.0 / 12.0,
                bite: 25.0,
            },
        }
    }

    /// Body and speeds for the fallback mover.
    pub fn motor(self) -> NpcMotor {
        match self {
            Species::Deer => NpcMotor {
                walk_speed: 1.3,
                run_speed: 6.5,
                sprint_speed: 11.5,
                acceleration: 16.0,
                turn_rate: 6.0,
                radius: 0.4,
                height: 1.5,
                step_height: 0.5,
                ..NpcMotor::default()
            },
            Species::Rabbit => NpcMotor {
                walk_speed: 0.6,
                run_speed: 4.0,
                sprint_speed: 8.5,
                acceleration: 30.0,
                turn_rate: 14.0,
                radius: 0.15,
                height: 0.35,
                step_height: 0.2,
                ..NpcMotor::default()
            },
            Species::Wolf => NpcMotor {
                walk_speed: 1.5,
                run_speed: 7.0,
                sprint_speed: 10.5,
                acceleration: 18.0,
                turn_rate: 8.0,
                radius: 0.3,
                height: 0.85,
                step_height: 0.45,
                ..NpcMotor::default()
            },
            Species::Bird => NpcMotor {
                walk_speed: 0.4,
                run_speed: 1.0,
                sprint_speed: 11.0,
                acceleration: 14.0,
                turn_rate: 10.0,
                radius: 0.1,
                height: 0.2,
                step_height: 0.1,
                ..NpcMotor::default()
            },
            Species::Boar => NpcMotor {
                walk_speed: 1.1,
                run_speed: 5.5,
                sprint_speed: 9.5,
                acceleration: 14.0,
                turn_rate: 6.0,
                radius: 0.35,
                height: 0.95,
                step_height: 0.4,
                ..NpcMotor::default()
            },
        }
    }

    pub fn footfall(self) -> Footfall {
        match self {
            Species::Deer => Footfall {
                kind: FootKind::Hoof,
                mass_kg: 70.0,
                size: [0.06, 0.08],
                stride: 0.9,
            },
            Species::Rabbit => Footfall {
                kind: FootKind::Paw,
                mass_kg: 2.0,
                size: [0.03, 0.05],
                stride: 0.3,
            },
            Species::Wolf => Footfall {
                kind: FootKind::Paw,
                mass_kg: 40.0,
                size: [0.08, 0.1],
                stride: 0.8,
            },
            Species::Bird => Footfall {
                kind: FootKind::None,
                mass_kg: 0.1,
                size: [0.02, 0.03],
                stride: 0.1,
            },
            Species::Boar => Footfall {
                kind: FootKind::Hoof,
                mass_kg: 80.0,
                size: [0.05, 0.06],
                stride: 0.6,
            },
        }
    }

    fn body_height(self) -> f32 {
        match self {
            Species::Deer => 1.3,
            Species::Rabbit => 0.3,
            Species::Wolf => 0.8,
            Species::Bird => 0.2,
            Species::Boar => 0.85,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Traits {
    /// Meters it notices things in daylight.
    sight: f32,
    hearing: f32,
    /// Meters it smells things upwind (0 = no).
    smell: f32,
    /// Distance at which a walking human makes it bolt, meters.
    flight: f32,
    health: f32,
    stamina_drain: f32,
    stamina_regen: f32,
    bite: f32,
}

/// An animal. Spawn it with a `Transform` and optionally a [`Herd`]; species, body, footsteps,
/// faction and health are filled in from the species when the animal appears.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[require(
    NpcMotor,
    MoveIntent,
    LocomotionState,
    AnimationParams,
    MoverState,
    NavAgent,
    NpcClock,
    NpcVitals,
    Footfall,
    BodyHeight,
    Faction,
    AnimalMind
)]
pub struct Animal {
    pub species: Species,
    /// Center of its range; `None` = where it was spawned.
    pub home: Option<DVec3>,
    /// How far from home it roams, meters.
    pub range: f32,
    /// Multiplies its flight distance: 0.5 tame .. 2 very skittish.
    pub skittishness: f32,
    /// Males carry antlers (deer) or tusks (boars).
    pub male: bool,
    /// Young (boars: a striped piglet that keeps with its mother's sounder).
    pub young: bool,
}

impl Default for Animal {
    fn default() -> Self {
        Self::new(Species::Deer)
    }
}

impl Animal {
    pub fn new(species: Species) -> Self {
        Self {
            species,
            home: None,
            range: match species {
                Species::Deer => 30.0,
                Species::Rabbit => 12.0,
                Species::Wolf => 60.0,
                Species::Bird => 15.0,
                Species::Boar => 40.0,
            },
            skittishness: 1.0,
            male: false,
            young: false,
        }
    }
}

/// Animals with the same herd id graze, move and flee together (a deer herd, a wolf pack, a
/// flock of birds).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Herd(pub u32);

/// A wolf's part in its pack's hunt, by its place among the living pack: the first tests the
/// herd and drives the quarry, the next two flank it, a fourth lies in wait ahead of it, and
/// any more drive it with the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PackRole {
    /// Not hunting with a pack (or not a wolf).
    #[default]
    None,
    /// Rushes the herd, singles out the weakest (wounded, young or straggling) and drives it.
    Tester,
    /// Swings wide on the quarry's left (right) to cut its escape line.
    FlankLeft,
    FlankRight,
    /// Waits ahead along the quarry's likely route, then springs.
    Ambusher,
    /// Drives the quarry from behind with the tester.
    Chaser,
}

impl PackRole {
    /// The role of the `slot`th of `n` living wolves in a pack.
    pub fn of(slot: usize, n: usize) -> Self {
        match (slot, n) {
            (_, 0) => PackRole::None,
            (0, _) => PackRole::Tester,
            (1, _) => PackRole::FlankLeft,
            (2, _) => PackRole::FlankRight,
            (3, _) => PackRole::Ambusher,
            _ => PackRole::Chaser,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            PackRole::None => "none",
            PackRole::Tester => "tester",
            PackRole::FlankLeft => "flank-left",
            PackRole::FlankRight => "flank-right",
            PackRole::Ambusher => "ambusher",
            PackRole::Chaser => "chaser",
        }
    }

    /// +1 for the left flank, -1 for the right, 0 otherwise.
    pub fn side(self) -> f32 {
        match self {
            PackRole::FlankLeft => 1.0,
            PackRole::FlankRight => -1.0,
            _ => 0.0,
        }
    }
}

/// How far ahead of the quarry (along the pack's line) a flanker takes its post (m): about
/// level with it, out wide. Posted well ahead, it lies where a quarry breaking off the line
/// to that side runs, and the quarry slips round the outside of it, leaving it on the far
/// side.
const FLANK_POST: f32 = 2.0;

/// A gunshot carries this far (m, times the animal's hearing): a bullet leaving the muzzle (a
/// `ProjectileTrace` faster than sound). Within it, birds burst up, and the rest look up
/// toward it and then move off from it; closer than [`GUNSHOT_BOLT`], they bolt.
pub const GUNSHOT_REACH: f32 = 90.0;
/// A gunshot this close (m) sends deer, boar, rabbits and wolves running at once.
pub const GUNSHOT_BOLT: f32 = 30.0;
/// Bullets faster than this (m/s) crack: slower projectiles (arrows, bolts) are not heard.
const GUNSHOT_SPEED: f32 = 250.0;
/// An explosion (a `WorldImpulse` with energy or heat, or a strong one) is heard this far
/// (m, times the animal's hearing), and everything that hears it bolts.
pub const BLAST_REACH: f32 = 160.0;
/// How far an animal moves off from a gunshot it looked up at (m), and for how long after the
/// shot it keeps moving off (s).
const WARY_DIST: f64 = 35.0;
const WARY_SECS: f64 = 20.0;

/// Where the day is, for the animals that keep hours (deer and boar): they move out at dawn and
/// dusk (up from their beds, down to water to drink, then grazing), graze through the morning
/// and afternoon, and bed down at midday and in the dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DayPart {
    /// Dark: bedded down.
    Night,
    /// First light until the sun is up: moving out.
    Dawn,
    Morning,
    /// The middle of the day (`MIDDAY_H`): bedded down, chewing the cud.
    Midday,
    Afternoon,
    /// Sundown until dark: moving out.
    Dusk,
}

/// How close to the middle of its herd a deer or boar lies up (m).
pub const BED_SPREAD: f64 = 3.0;
/// The hours the herds lie up through the middle of the day.
pub const MIDDAY_H: (f32, f32) = (11.0, 15.0);
/// The sun's height (the y of its direction) under which it is dark (night), and over which it
/// is up (dawn and dusk are between).
pub const DARK_SUN: f32 = -0.1;
pub const RISEN_SUN: f32 = 0.2;

/// Where the day is at `hour` (0..24) with the sun at height `sun_y` (the y of the direction
/// toward it): both as `WeatherState` has them, the day/night cycle writing the two together.
pub fn day_part(hour: f32, sun_y: f32) -> DayPart {
    let morning = hour.rem_euclid(24.0) < 12.5;
    if sun_y < DARK_SUN {
        DayPart::Night
    } else if sun_y < RISEN_SUN {
        if morning {
            DayPart::Dawn
        } else {
            DayPart::Dusk
        }
    } else if (MIDDAY_H.0..MIDDAY_H.1).contains(&hour) {
        DayPart::Midday
    } else if morning {
        DayPart::Morning
    } else {
        DayPart::Afternoon
    }
}

impl DayPart {
    pub fn name(self) -> &'static str {
        match self {
            DayPart::Night => "night",
            DayPart::Dawn => "dawn",
            DayPart::Morning => "morning",
            DayPart::Midday => "midday",
            DayPart::Afternoon => "afternoon",
            DayPart::Dusk => "dusk",
        }
    }

    /// Bedded down (deer and boar).
    pub fn bed_time(self) -> bool {
        matches!(self, DayPart::Night | DayPart::Midday)
    }
}

/// Deer and boar keep the day's hours ([`DayPart`]); rabbits bed down in the dark, wolves
/// hunt through it.
fn keeps_hours(species: Species) -> bool {
    matches!(species, Species::Deer | Species::Boar)
}

/// How far ahead of an animal's middle its muzzle reaches with its head down (m): it stands
/// this far back from the water's edge to drink.
fn drink_reach(species: Species, young: bool) -> f64 {
    let full = match species {
        Species::Deer => 1.05,
        Species::Boar => 0.8,
        Species::Wolf => 0.85,
        Species::Rabbit => 0.15,
        Species::Bird => 0.05,
    };
    if young { full * 0.6 } else { full }
}

/// An ambusher springs when the quarry comes this close (m); a deer fleeing the pack doesn't
/// see it lying low in wait until then.
pub const AMBUSH_RANGE: f32 = 12.0;

/// A pack gives up a chase this long (s, on its own clock) after its first wolf set off, and
/// rests this long before it hunts again.
pub const PACK_CHASE_SECS: f64 = 20.0;
pub const PACK_REST_SECS: f64 = 25.0;
/// The tester picks over the fleeing herd for the weakest this long into its rush (s).
pub const TEST_SECS: f64 = 4.0;
/// Past its test, a pack breaks off from a fit quarry (unhurt, grown) that is this far (m)
/// ahead of its nearest wolf and still pulling away (by `ESCAPE_GAIN` m over `ESCAPE_SECS`),
/// rather than run on to `PACK_CHASE_SECS` after one it can't catch.
pub const ESCAPE_GAP: f32 = 18.0;
pub const ESCAPE_GAIN: f32 = 1.5;
pub const ESCAPE_SECS: f64 = 2.0;
/// A wolf scents deer this far off (m), and stalks a herd it scents until it is past this.
pub const SCENT_REACH: f64 = 110.0;
/// A deer chased by a wolf this close (m) cuts from side to side.
pub const ZIGZAG_RANGE: f32 = 6.0;
/// A deer that raises the alarm stots this long (s) as it sets off.
pub const STOT_SECS: f32 = 0.8;

/// Left of a horizontal heading.
fn left_of(h: Vec3) -> Vec3 {
    Vec3::new(h.z, 0.0, -h.x)
}

fn flat(d: DVec3) -> Vec3 {
    Vec3::new(d.x as f32, 0.0, d.z as f32)
}

fn flat_v(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// What an animal is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum AnimalState {
    /// Head down, eating, a step now and then.
    #[default]
    Graze,
    /// Walking to a new spot near the herd.
    Wander,
    /// At the water's edge, head down.
    Drink,
    /// Lying down (night, tired, full).
    Rest,
    /// Head up, watching something.
    Alert,
    /// Running away.
    Flee,
    /// Wolves: creeping toward prey and spreading out around it.
    Stalk,
    /// Wolves: running prey down.
    Chase,
    /// Boars: charging a threat to slash it with the tusks.
    Charge,
    /// Wolves: eating a kill.
    Feed,
    /// Killed (lies there until it respawns).
    Dead,
    /// Birds: on the ground.
    Perched,
    /// Birds: in the air.
    Flying,
}

impl AnimalState {
    pub fn name(self) -> &'static str {
        match self {
            AnimalState::Graze => "graze",
            AnimalState::Wander => "wander",
            AnimalState::Drink => "drink",
            AnimalState::Rest => "rest",
            AnimalState::Alert => "alert",
            AnimalState::Flee => "flee",
            AnimalState::Stalk => "stalk",
            AnimalState::Chase => "chase",
            AnimalState::Charge => "charge",
            AnimalState::Feed => "feed",
            AnimalState::Dead => "dead",
            AnimalState::Perched => "perched",
            AnimalState::Flying => "flying",
        }
    }
}

/// An animal's mind. Read it to see what an animal is doing and why (`npc.inspect`).
#[derive(Component, Debug, Clone, Default)]
pub struct AnimalMind {
    pub state: AnimalState,
    /// Animal clock time the state started.
    pub since: f64,
    /// What it is watching or running from, and when it last sensed it (animal clock).
    pub threat: Option<Entity>,
    pub threat_seen: f64,
    pub threat_pos: Option<DVec3>,
    /// How dangerous the threat is: ≥ 1 means inside its flight distance.
    pub danger: f32,
    /// Wolves: the animal the pack is hunting.
    pub prey: Option<Entity>,
    /// 0..1 needs.
    pub hunger: f32,
    pub thirst: f32,
    pub fatigue: f32,
    /// 0..1: sprinting drains it; below 0.15 it can only run.
    pub stamina: f32,
    pub home: Option<DVec3>,
    pub destination: Option<DVec3>,
    /// Direction it last fled in (herd mates align with it).
    pub flee_dir: Vec3,
    /// Keep fleeing at least until (animal clock).
    pub flee_until: f64,
    /// A herd mate panicked: join the flight at this time.
    pub contagion_at: Option<f64>,
    pub next_think: f64,
    pub wait_until: f64,
    /// Last time it sensed real danger.
    pub last_danger: f64,
    /// Loudest noise heard since the last think: (where, how loud relative to distance).
    pub heard: Option<(DVec3, f32)>,
    /// A gunshot heard since the last think, too far off to bolt from: where.
    pub(crate) shot: Option<DVec3>,
    /// The last gunshot it moves off from: where, and when (animal clock).
    pub(crate) wary: Option<(DVec3, f64)>,
    /// Hurt since the last think, by something at this position.
    pub hurt_from: Option<DVec3>,
    /// When it was last hurt (animal clock), where the blow came from and who struck it, if
    /// anyone (kept after the think that reacts to it).
    pub wounded: Option<(f64, DVec3, Option<Entity>)>,
    /// Wolves: bite this animal this tick.
    pub bite: Option<Entity>,
    pub bite_ready: f64,
    /// Searched for water: `Some(None)` = none in range.
    pub water: Option<Option<DVec3>>,
    /// Where it stands to drink, a muzzle's reach back from the water's edge, and which way
    /// it faces (toward the water).
    pub drink_at: Option<(DVec3, Vec3)>,
    /// The part of the day it last knew (deer and boar keep hours: [`DayPart`]).
    pub day: Option<DayPart>,
    /// Wolves: its part in the pack's hunt; in place for the pack's rush (an ambusher: lying
    /// in wait); when it joined the pack's chase and when it last gave one up (animal clock).
    pub role: PackRole,
    pub placed: bool,
    pub chase_from: Option<f64>,
    pub gave_up: Option<f64>,
    /// Deer: when the herd's alarm reached it (animal clock), and whether it raised it itself
    /// (snorting and flagging its tail); the nearest wolf running it down, and how far (m).
    pub alarm: Option<(f64, bool)>,
    pub pursuer: Option<(DVec3, f32)>,
    chaser: Option<Entity>,
    /// Flankers: when it came level with the quarry in this chase (animal clock).
    abreast: Option<f64>,
    /// In a chase: the pack's lead on the quarry (its nearest wolf's distance, m) when it
    /// last looked (animal clock), to tell whether a fit quarry is getting away.
    gap_mark: Option<(f64, f32)>,
    /// Deer: it raised the alarm at a predator it saw, so it stots as it sets off (not at a
    /// bang, a blow or fire: then it just runs).
    pub stots: bool,
    /// Away from everything it last saw or smelled that threatens it, weighted by danger.
    escape: Option<Vec3>,
    zig: f32,
    next_zig: f64,
    next_howl: f64,
    /// Boars: when its flight last found no way out, when its tusks last struck, and the
    /// earliest it charges again after breaking off.
    cornered_at: Option<f64>,
    slashed_at: Option<f64>,
    rally_at: f64,
    rng: u64,
    initialized: bool,
}

impl AnimalMind {
    fn random(&mut self) -> f32 {
        self.rng = hash_u64(self.rng.wrapping_add(0x9e37_79b9_7f4a_7c15));
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Deer: 0..1, how hard it is stotting, the first stiff-legged bounds of a flight it
    /// raised the alarm for at a predator it saw (not at a noise or fire, not when wounded,
    /// and not with a wolf already close).
    pub fn stot(&self, now: f64) -> f32 {
        let Some((at, true)) = self.alarm else {
            return 0.0;
        };
        let t = (now - at) as f32;
        let hurt = self.wounded.is_some_and(|(w, _, _)| now - w < 5.0);
        let close = self.pursuer.is_some_and(|p| p.1 < 12.0);
        if !self.stots
            || self.state != AnimalState::Flee
            || hurt
            || close
            || !(0.0..STOT_SECS).contains(&t)
        {
            return 0.0;
        }
        let x = ((t - 0.55) / (STOT_SECS - 0.55)).clamp(0.0, 1.0);
        1.0 - x * x * (3.0 - 2.0 * x)
    }

    fn set(&mut self, state: AnimalState, now: f64) {
        if self.state != state {
            self.state = state;
            self.since = now;
            self.destination = None;
        }
    }
}

/// Flight state of a bird (birds move themselves; the fallback mover leaves them alone).
#[derive(Component, Debug, Clone, Copy, PartialEq, Default)]
pub struct BirdFlight {
    pub velocity: Vec3,
    /// Wing beat phase, radians.
    pub flap: f32,
    pub landing: Option<DVec3>,
    pub airborne_since: f64,
    pub hop_at: f64,
}

/// Wildlife tuning.
#[derive(Resource, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct WildlifeSettings {
    pub enabled: bool,
    /// Killed animals come back after this many seconds (0 = never).
    pub respawn_secs: f32,
    /// Seconds from drinking to thirsty again.
    pub thirst_secs: f32,
    /// Seconds from a meal to a wolf pack hunting again.
    pub hunger_secs: f32,
    /// Multiplies every animal's flight distance.
    pub skittishness: f32,
    /// Wolves howl at night (a loud noise other animals hear).
    pub howl: bool,
}

impl Default for WildlifeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            respawn_secs: 40.0,
            thirst_secs: 150.0,
            hunger_secs: 90.0,
            skittishness: 1.0,
            howl: true,
        }
    }
}

// ------------------------------------------------------------------------------------------
// Setup and senses
// ------------------------------------------------------------------------------------------

type InitItem<'a> = (
    Entity,
    &'a Animal,
    &'a Transform,
    &'a mut AnimalMind,
    &'a mut NpcMotor,
    &'a mut Footfall,
    &'a mut BodyHeight,
    &'a mut Faction,
    &'a mut NpcVitals,
);

/// Fill in species defaults for new animals (components left at their generic defaults).
pub(crate) fn init_animals(
    mut commands: Commands,
    mut q: Query<InitItem, Added<Animal>>,
    seeds: Query<&crate::seed::Seed>,
) {
    for (e, animal, t, mut mind, mut motor, mut foot, mut body, mut faction, mut vitals) in &mut q {
        if mind.initialized {
            continue;
        }
        let s = animal.species;
        // The young are smaller, lighter and frailer; boars are bigger than sows.
        let (size, mass, health) = match (animal.young, s == Species::Boar && animal.male) {
            (true, _) => (0.5, 0.15, 0.25),
            (false, true) => (1.1, 1.4, 1.3),
            _ => (1.0, 1.0, 1.0),
        };
        if *motor == NpcMotor::default() {
            *motor = s.motor();
            motor.radius *= size;
            motor.height *= size;
            motor.step_height *= size;
            if animal.young {
                motor.run_speed *= 0.7;
                motor.sprint_speed *= 0.45;
            }
        }
        if *foot == Footfall::default() {
            *foot = s.footfall();
            foot.mass_kg *= mass;
            foot.size = foot.size.map(|v| v * size);
            foot.stride *= size;
        }
        if *body == BodyHeight::default() {
            *body = BodyHeight(s.body_height() * size);
        }
        if *faction == Faction::default() {
            *faction = s.faction();
        }
        if *vitals == NpcVitals::default() {
            vitals.max_health = s.traits().health * health;
            vitals.health = vitals.max_health;
        }
        mind.initialized = true;
        mind.rng = hash_u64(crate::seed::seed_of(&seeds, e) ^ 0x00A4_1AA1);
        mind.home = Some(animal.home.unwrap_or(t.translation));
        mind.stamina = 1.0;
        // Packs start fed and get hungry over `hunger_secs`.
        mind.hunger = 0.15 * mind.random();
        mind.thirst = mind.random() * 0.6;
        mind.fatigue = mind.random() * 0.4;
        mind.next_think = mind.random() as f64 * 0.3;
        if s == Species::Bird {
            mind.state = AnimalState::Perched;
            commands
                .entity(e)
                .insert((SelfPropelled, BirdFlight::default()));
        }
    }
}

type SenseItem<'a> = (
    Entity,
    &'a Animal,
    Option<&'a Herd>,
    &'a Transform,
    &'a NpcClock,
    &'a mut AnimalMind,
    &'a mut NpcVitals,
);

/// What animals heard this tick, kept from tick to tick so hearing allocates nothing.
#[derive(Default)]
pub(crate) struct Heard {
    noises: Vec<NoiseEvent>,
    /// Gunshots: the muzzle, and who fired.
    shots: Vec<(DVec3, Option<Entity>)>,
    /// Explosions: where.
    blasts: Vec<DVec3>,
}

/// Keeps the loudest noise an animal heard since it last thought.
fn louder(mind: &mut AnimalMind, at: DVec3, level: f32) {
    if mind.heard.is_none_or(|(_, l)| level > l) {
        mind.heard = Some((at, level));
    }
}

/// Noises, gunshots, explosions and damage that reach animals.
#[allow(clippy::too_many_arguments)]
pub(crate) fn animal_senses(
    mut noises: MessageReader<NoiseEvent>,
    mut traces: MessageReader<ProjectileTrace>,
    mut impulses: MessageReader<WorldImpulse>,
    mut hurts: MessageReader<DamageEvent>,
    positions: Query<&Transform>,
    packs: Query<&Herd>,
    mut q: Query<SenseItem>,
    mut heard: Local<Heard>,
) {
    let heard = &mut *heard;
    heard.noises.clear();
    heard.noises.extend(noises.read().copied());
    heard.shots.clear();
    heard.shots.extend(
        traces
            .read()
            .filter(|t| t.speed > GUNSHOT_SPEED)
            .map(|t| (t.from, t.shooter)),
    );
    heard.blasts.clear();
    heard.blasts.extend(
        impulses
            .read()
            .filter(|i| i.energy_j > 0.0 || i.heat_c > 300.0 || i.strength >= 15.0)
            .map(|i| i.center),
    );
    let any = !(heard.noises.is_empty() && heard.shots.is_empty() && heard.blasts.is_empty());
    if any {
        for (e, animal, herd, t, _, mut mind, _) in &mut q {
            let species = animal.species;
            let hearing = species.traits().hearing;
            let pos = t.translation;
            // Explosions: everything that hears one bolts.
            for &at in &heard.blasts {
                if (at.distance(pos) as f32) < BLAST_REACH * hearing {
                    louder(&mut mind, at, 3.0);
                }
            }
            // Gunshots: close, it bolts (a bird always bursts up); further off, it looks up
            // and then moves off.
            for &(at, _) in &heard.shots {
                let d = at.distance(pos) as f32;
                if d >= GUNSHOT_REACH * hearing {
                    continue;
                }
                if d < GUNSHOT_BOLT || species == Species::Bird {
                    louder(&mut mind, at, 2.0);
                } else if mind.shot.is_none_or(|s| s.distance(pos) > at.distance(pos)) {
                    mind.shot = Some(at);
                }
            }
            for n in heard.noises.iter() {
                // Not its own noise, nor its herd's (a mate's snort or its pack's howl: the
                // herd has its own ways of passing an alarm on), nor the bang of a shot it
                // heard as a gunshot.
                if n.source == Some(e)
                    || herd.is_some_and(|h| n.source.and_then(|s| packs.get(s).ok()) == Some(h))
                    || heard
                        .shots
                        .iter()
                        .any(|&(at, by)| by == n.source && at.distance(n.position) < 3.0)
                {
                    continue;
                }
                let d = n.position.distance(t.translation) as f32;
                let reach = n.radius * hearing;
                if d >= reach {
                    continue;
                }
                // Loudness here: 1 at the source .. 0 at the edge, plus how big the noise is.
                let level = (1.0 - d / reach) * (n.radius / 30.0).min(3.0);
                louder(&mut mind, n.position, level);
            }
        }
    }
    for h in hurts.read() {
        let Ok((_, _, _, t, clock, mut mind, mut vitals)) = q.get_mut(h.target) else {
            continue;
        };
        if vitals.downed || h.amount <= 0.0 {
            continue;
        }
        vitals.health -= h.amount;
        let from = h
            .source
            .and_then(|s| positions.get(s).ok())
            .map(|p| p.translation)
            .unwrap_or(h.point);
        let from = if from.distance(t.translation) < 0.1 {
            t.translation + DVec3::new(0.0, 0.0, 1.0)
        } else {
            from
        };
        mind.hurt_from = Some(from);
        mind.wounded = Some((clock.now, from, h.source));
        // A blow is felt at once: it reacts this tick.
        mind.next_think = mind.next_think.min(clock.now);
        if vitals.health <= 0.0 {
            vitals.health = 0.0;
            vitals.downed = true;
            let now = clock.now;
            mind.set(AnimalState::Dead, now);
        }
    }
}

// ------------------------------------------------------------------------------------------
// Thinking and moving
// ------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Snap {
    species: Species,
    young: bool,
    male: bool,
    pos: DVec3,
    vel: Vec3,
    state: AnimalState,
    /// Seconds in its state (each animal runs on its own clock, so times are kept as ages).
    age: f64,
    flee_dir: Vec3,
    prey: Option<Entity>,
    dead: bool,
    /// Sensed danger itself in the last second (not just caught it from the herd).
    sensed: bool,
    threat_pos: Option<DVec3>,
    herd: Option<u32>,
    /// 0..1: how badly hurt it is.
    hurt: f32,
    /// 0..1: how far it has strayed from its herd.
    straggle: f32,
    /// Wolves: in place for the rush, seconds since it joined the pack's chase and since it
    /// last gave one up.
    placed: bool,
    chasing: Option<f64>,
    gave_up: Option<f64>,
}

/// What animals know of each other this tick, kept from tick to tick so thinking allocates
/// nothing once the lists have grown.
#[derive(Default)]
pub(crate) struct AnimalScratch {
    snaps: HashMap<Entity, Snap>,
    herds: HashMap<u32, Vec<Entity>>,
    /// Each herd's living members: the sum of their positions and how many.
    centers: HashMap<u32, (DVec3, f64)>,
    near: Vec<u32>,
}

/// The world as animals need it.
#[derive(SystemParam)]
pub(crate) struct AnimalWorld<'w> {
    world: Res<'w, TacticalWorld>,
    grid: ResMut<'w, NavGrid>,
    ground: Res<'w, Ground>,
    water: Res<'w, Water>,
    /// Worn paths, which routes and grazing steps lean toward.
    wear: Option<Res<'w, GroundWear>>,
    heat: Res<'w, HeatField>,
    weather: Option<Res<'w, WeatherState>>,
    wind: Option<Res<'w, WindField>>,
    seen: Res<'w, Perceivables>,
    settings: Res<'w, WildlifeSettings>,
}

type AnimalItem<'a> = (
    Entity,
    &'a Animal,
    Option<&'a Herd>,
    &'a mut AnimalMind,
    &'a mut MoveIntent,
    &'a mut Transform,
    &'a mut LocomotionState,
    &'a NpcClock,
    &'a NpcMotor,
    &'a mut NpcVitals,
    &'a mut NavAgent,
    Option<&'a mut BirdFlight>,
    Option<&'a mut AnimationParams>,
);

pub(crate) fn animal_think(
    mut w: AnimalWorld,
    mut q: Query<AnimalItem>,
    mut noise: MessageWriter<NoiseEvent>,
    mut scratch: Local<AnimalScratch>,
    seeds: Query<&crate::seed::Seed>,
) {
    if !w.settings.enabled {
        return;
    }
    // Snapshot everyone first, so animals can read each other while updating themselves.
    let AnimalScratch {
        snaps,
        herds,
        centers,
        near,
    } = &mut *scratch;
    snaps.clear();
    herds.retain(|_, list| !list.is_empty());
    herds.values_mut().for_each(Vec::clear);
    centers.clear();
    for (e, animal, herd, mind, _, t, loco, clock, _, vitals, _, bird, _) in q.iter() {
        let ago = |at: Option<f64>| at.map(|at| (clock.now - at).max(0.0));
        snaps.insert(
            e,
            Snap {
                species: animal.species,
                young: animal.young,
                male: animal.male,
                pos: t.translation,
                vel: bird.map(|b| b.velocity).unwrap_or(loco.velocity),
                state: mind.state,
                age: clock.now - mind.since,
                flee_dir: mind.flee_dir,
                prey: mind.prey,
                dead: vitals.downed,
                sensed: clock.now - mind.last_danger < 1.0,
                threat_pos: mind.threat_pos,
                herd: herd.map(|h| h.0),
                hurt: (1.0 - vitals.health / vitals.max_health.max(1e-3)).clamp(0.0, 1.0),
                straggle: 0.0,
                placed: mind.placed,
                chasing: ago(mind.chase_from),
                gave_up: ago(mind.gave_up),
            },
        );
        if let Some(h) = herd {
            herds.entry(h.0).or_default().push(e);
            if !vitals.downed {
                let c = centers.entry(h.0).or_default();
                c.0 += t.translation;
                c.1 += 1.0;
            }
        }
    }
    for list in herds.values_mut() {
        list.sort();
    }
    for s in snaps.values_mut() {
        if let Some((sum, n)) = s.herd.and_then(|h| centers.get(&h))
            && *n > 1.0
        {
            let off = (s.pos - *sum / *n).length() as f32;
            s.straggle = ((off - 4.0) / 8.0).clamp(0.0, 1.0);
        }
    }
    let snaps = &*snaps;
    let herds = &*herds;
    let daylight = w.weather.as_ref().map(|w| w.daylight()).unwrap_or(1.0);
    let part = w.weather.as_ref().map_or(DayPart::Morning, |w| {
        day_part(w.time_of_day_h, w.sun_direction.y)
    });
    let mut replans = 0u32;
    for (
        e,
        animal,
        herd,
        mut mind,
        mut intent,
        mut t,
        mut loco,
        clock,
        motor,
        mut vitals,
        mut nav,
        bird,
        anim,
    ) in &mut q
    {
        let dt = clock.dt;
        if dt <= 0.0 {
            continue;
        }
        let now = clock.now;
        let pos = t.translation;
        let species = animal.species;
        let traits = species.traits();
        let home = mind.home.unwrap_or(pos);
        let mates: &[Entity] = herd
            .and_then(|h| herds.get(&h.0))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        *intent = MoveIntent::default();

        // ------------------------------------------------------------ dead: wait, then respawn
        if mind.state == AnimalState::Dead {
            vitals.downed = true;
            nav.clear();
            if let Some(mut b) = bird {
                // Fall to the ground.
                let floor = w.world.surface_height(pos, pos.y, 0.3, &w.ground);
                b.velocity.x *= 0.9;
                b.velocity.z *= 0.9;
                b.velocity.y -= 9.81 * dt;
                let mut p = pos + (b.velocity * dt).as_dvec3();
                if p.y <= floor {
                    p.y = floor;
                    b.velocity = Vec3::ZERO;
                }
                t.translation = p;
                loco.velocity = b.velocity;
            }
            let respawn = w.settings.respawn_secs;
            if respawn > 0.0 && now - mind.since > respawn as f64 {
                let a = mind.random() * TAU;
                let r = animal.range * (0.5 + 0.4 * mind.random());
                let mut p = home + DVec3::new((a.cos() * r) as f64, 0.0, (a.sin() * r) as f64);
                {
                    let mut ctx = w.grid.ctx(&w.world, &w.ground, &w.water);
                    if species != Species::Bird
                        && let Some(q) = ctx.nearest_walkable(p, 6.0)
                    {
                        p = q;
                    }
                }
                p.y = w.world.surface_height(p, p.y + 50.0, 60.0, &w.ground);
                t.translation = p;
                vitals.health = vitals.max_health;
                vitals.downed = false;
                mind.hunger = 0.2;
                mind.stamina = 1.0;
                mind.threat = None;
                mind.prey = None;
                mind.hurt_from = None;
                mind.wounded = None;
                mind.heard = None;
                let start = if species == Species::Bird {
                    AnimalState::Perched
                } else {
                    AnimalState::Graze
                };
                mind.set(start, now);
            }
            continue;
        }

        // ------------------------------------------------------------ needs and stamina
        let night = daylight < 0.25;
        mind.thirst = (mind.thirst + dt / w.settings.thirst_secs.max(1.0)).min(1.0);
        // The day turning: at dawn and dusk the herds move out, up from their beds (each in its
        // own time) and down to water.
        if keeps_hours(species) && mind.day != Some(part) {
            let moving_out = mind.day.is_some() && matches!(part, DayPart::Dawn | DayPart::Dusk);
            mind.day = Some(part);
            if moving_out {
                mind.thirst = mind.thirst.max(0.8);
                if mind.state == AnimalState::Rest {
                    mind.wait_until = now + 1.0 + 8.0 * mind.random() as f64;
                }
            }
        }
        let hunger_rate = if night { 1.5 } else { 1.0 };
        mind.hunger = (mind.hunger + dt * hunger_rate / w.settings.hunger_secs.max(1.0)).min(1.0);
        if mind.state == AnimalState::Rest {
            mind.fatigue = (mind.fatigue - dt / 40.0).max(0.0);
        } else {
            mind.fatigue = (mind.fatigue + dt / 400.0).min(1.0);
        }
        if loco.mode == LocomotionMode::Sprint {
            mind.stamina = (mind.stamina - traits.stamina_drain * dt).max(0.0);
        } else {
            mind.stamina = (mind.stamina + traits.stamina_regen * dt).min(1.0);
        }

        // ------------------------------------------------------------ senses (a few times a second)
        if now >= mind.next_think {
            mind.next_think = now + 0.18 + mind.random() as f64 * 0.08;
            // A herd mate's alarm runs on its own time: bolt on the beat.
            if let Some(at) = mind.contagion_at {
                mind.next_think = mind.next_think.min(at.max(now));
            }
            let asleep = mind.state == AnimalState::Rest;
            let eye = pos + DVec3::Y * species.body_height() as f64;
            let sight = traits.sight * (0.45 + 0.55 * daylight) * if asleep { 0.5 } else { 1.0 };
            let wind = w
                .wind
                .as_ref()
                .map(|wf| wf.sample(eye))
                .unwrap_or(Vec3::ZERO);
            let range = sight.max(traits.smell).max(traits.flight * 3.0);
            w.seen.near(pos, range as f64, near);
            let mut best: Option<(Entity, DVec3, f32)> = None;
            let mut escape = Vec3::ZERO;
            for &i in near.iter() {
                let p = &w.seen.list[i as usize];
                if p.entity == e {
                    continue;
                }
                let other = snaps.get(&p.entity);
                let d = DVec2::new(p.position.x - pos.x, p.position.z - pos.z).length() as f32;
                // Wolves give a boar charging at them room (not one charging past them at
                // something else).
                let charging = other.is_some_and(|o| {
                    let to_me = Vec3::new((pos.x - o.pos.x) as f32, 0.0, (pos.z - o.pos.z) as f32)
                        .normalize_or_zero();
                    let going = Vec3::new(o.vel.x, 0.0, o.vel.z);
                    o.species == Species::Boar
                        && o.state == AnimalState::Charge
                        && d < 8.0
                        && going.length() > 1.0
                        && going.normalize().dot(to_me) > 0.5
                });
                let threat = match species {
                    Species::Wolf => {
                        (p.faction != Faction::PREY && p.faction != Faction::PREDATOR) || charging
                    }
                    _ => p.faction != Faction::PREY,
                };
                if !threat || other.is_some_and(|o| o.dead) {
                    continue;
                }
                let mut f = match p.mode {
                    LocomotionMode::Crouch => 0.4,
                    LocomotionMode::Idle => 0.75,
                    LocomotionMode::Walk => 1.0,
                    LocomotionMode::Run => 1.35,
                    LocomotionMode::Sprint | LocomotionMode::Dash => 1.7,
                    LocomotionMode::Air => 1.3,
                    _ => 1.0,
                };
                if let Some(o) = other {
                    // Prey can tell a hunting pack from a passing one.
                    f *= match o.state {
                        AnimalState::Chase => 2.2,
                        AnimalState::Charge => 2.0,
                        AnimalState::Stalk => 0.5,
                        AnimalState::Feed | AnimalState::Rest => 0.6,
                        _ => 0.8,
                    };
                }
                if p.attacking {
                    f *= 1.5;
                }
                let flight = traits.flight
                    * animal.skittishness
                    * w.settings.skittishness
                    * f
                    * if asleep { 0.6 } else { 1.0 };
                // Running flat out from a pack, it doesn't see a wolf lying low in wait ahead
                // until it is almost on it (though it may smell it).
                let lying_low = mind.state == AnimalState::Flee
                    && p.mode == LocomotionMode::Crouch
                    && other.is_some_and(|o| o.state == AnimalState::Stalk);
                let mut sensed = false;
                if d < 4.0 {
                    sensed = true;
                } else if d < sight * f.max(0.6) && !(lying_low && d > AMBUSH_RANGE) {
                    let target = p.position + DVec3::Y * (p.height * 0.7) as f64;
                    sensed = w.world.raycast(eye, target).is_none();
                }
                // Scent drifts downwind: smell what's upwind.
                let mut scale = 1.0;
                if !sensed && traits.smell > 0.0 && d < traits.smell && wind.length() > 0.7 {
                    let to_me = Vec3::new(
                        pos.x as f32 - p.position.x as f32,
                        0.0,
                        pos.z as f32 - p.position.z as f32,
                    )
                    .normalize_or_zero();
                    if wind.normalize().dot(to_me) > 0.7 {
                        sensed = true;
                        scale = 0.8;
                    }
                }
                if !sensed {
                    continue;
                }
                let danger = flight / d.max(0.5) * scale;
                if best.is_none_or(|b| danger > b.2) {
                    best = Some((p.entity, p.position, danger));
                }
                // Away from all of them at once, the nearest most (so a deer with a wolf on
                // either side runs on between them rather than from one into the other).
                let off = flat(pos - p.position);
                if off.length() > 0.1 {
                    escape += off.normalize() * danger.min(3.0) * (1.0 + 6.0 / d.max(1.0));
                }
            }
            let mut danger = 0.0;
            let mut danger_at = None;
            mind.escape = None;
            if let Some((te, tp, dg)) = best {
                danger = dg;
                danger_at = Some(tp);
                mind.threat = Some(te);
                mind.threat_seen = now;
                mind.threat_pos = Some(tp);
                mind.escape = (escape.length() > 0.2).then(|| escape.normalize());
            }
            // Being hurt: run, whatever it is.
            if let Some(from) = mind.hurt_from.take() {
                danger = danger.max(2.0);
                danger_at = Some(from);
                mind.threat_pos = Some(from);
                mind.escape = None;
            }
            // Noise: loud and close = run; otherwise look.
            if let Some((at, level)) = mind.heard.take() {
                let dg = if level > 1.0 { 1.2 } else { level * 0.9 };
                if dg > danger {
                    danger = dg;
                    danger_at = Some(at);
                    mind.threat = None;
                    mind.threat_pos = Some(at);
                    mind.escape = None;
                }
            }
            // A gunshot further off: it looks up toward it, then moves off from it (already
            // moving off from one, it carries on).
            if let Some(at) = mind.shot.take() {
                let moving_off = mind.state == AnimalState::Wander && mind.wary.is_some();
                mind.wary = Some((at, now));
                if !moving_off && danger < 0.6 {
                    danger = 0.6;
                    danger_at = Some(at);
                    mind.threat = None;
                    mind.threat_pos = Some(at);
                    mind.escape = None;
                }
            }
            // Fire.
            let heat = w.heat.sample(pos + DVec3::Y * 0.5);
            if heat > 40.0 {
                let g = |dx: f64, dz: f64| w.heat.sample(pos + DVec3::new(dx, 0.5, dz));
                let grad = Vec3::new(g(2.0, 0.0) - g(-2.0, 0.0), 0.0, g(0.0, 2.0) - g(0.0, -2.0));
                let hot = if grad.length() > 1e-3 {
                    pos + (grad.normalize() * 4.0).as_dvec3()
                } else {
                    pos + DVec3::X
                };
                danger = danger.max(1.5);
                danger_at = Some(hot);
                mind.threat_pos = Some(hot);
                mind.escape = None;
            }
            mind.danger = danger;
            if danger >= 0.5 {
                mind.last_danger = now;
            }

            // It ran into danger itself (not caught from the herd): a deer bolting snorts
            // the alarm.
            let raised = danger >= 1.0;
            let sighted = mind.escape.is_some();
            // Herd: panic spreads, alert mates make it look up (only from mates that sensed
            // the danger themselves, so a calm herd doesn't keep itself on edge).
            if species != Species::Wolf {
                let mut heads_up = false;
                for &m in mates {
                    if m == e {
                        continue;
                    }
                    let Some(s) = snaps.get(&m) else { continue };
                    let d = s.pos.distance(pos);
                    let fled = matches!(s.state, AnimalState::Flee | AnimalState::Flying)
                        && s.age < 4.0
                        && s.sensed;
                    if fled
                        && d < 40.0
                        && mind.contagion_at.is_none()
                        && !matches!(mind.state, AnimalState::Flee | AnimalState::Flying)
                    {
                        if species == Species::Deer {
                            // A deer catching the alarm turns its head to where it came from
                            // first, then bolts with the herd.
                            let delay = 0.3 + 0.25 * mind.random() as f64;
                            mind.contagion_at = Some(now + delay);
                            mind.next_think = mind.next_think.min(now + delay);
                            mind.alarm = Some((now, false));
                            if danger < 0.55
                                && let Some(tp) = s.threat_pos
                            {
                                mind.threat_pos = Some(tp);
                            }
                            heads_up = true;
                        } else {
                            let delay = 0.12 + 0.35 * mind.random() as f64;
                            mind.contagion_at = Some(now + delay);
                        }
                        if s.flee_dir.length_squared() > 0.1 {
                            mind.flee_dir = s.flee_dir;
                        }
                    }
                    if s.state == AnimalState::Alert && s.sensed && d < 25.0 && danger < 0.55 {
                        danger = 0.56;
                        if mind.threat_pos.is_none() {
                            mind.threat_pos = Some(s.pos + (s.pos - pos));
                        }
                    }
                }
                if heads_up {
                    danger = danger.max(0.6);
                }
                if mind.contagion_at.is_some_and(|at| now >= at) {
                    mind.contagion_at = None;
                    if danger < 1.0 {
                        danger = 1.0;
                        // A deer runs from what the herd's alarm was about.
                        let alarmed = species == Species::Deer
                            && mind.alarm.is_some_and(|(at, _)| now - at < 2.0);
                        if danger_at.is_none() && alarmed && mind.threat_pos.is_some() {
                            danger_at = mind.threat_pos;
                        } else if danger_at.is_none() {
                            let away = -mind.flee_dir;
                            danger_at = Some(pos + (away * 10.0).as_dvec3());
                            mind.threat_pos = danger_at;
                        }
                    }
                }
                mind.danger = danger;
            }
            // A deer keeps track of the wolf running it down.
            if species == Species::Deer {
                mind.chaser = snaps
                    .iter()
                    .filter(|(_, s)| {
                        s.species == Species::Wolf
                            && s.state == AnimalState::Chase
                            && s.prey == Some(e)
                            && !s.dead
                    })
                    .min_by(|a, b| {
                        a.1.pos
                            .distance_squared(pos)
                            .total_cmp(&b.1.pos.distance_squared(pos))
                    })
                    .map(|(&w, _)| w);
            }

            decide(
                &mut mind, species, e, pos, home, animal, mates, snaps, now, night, danger_at,
                &mut w,
            );
            if species == Species::Deer
                && raised
                && mind.state == AnimalState::Flee
                && mind.since == now
            {
                // Snorts and flags its tail.
                mind.alarm = Some((now, true));
                mind.stots = sighted;
                noise.write(NoiseEvent {
                    position: pos + DVec3::Y * 1.2,
                    radius: 25.0,
                    source: Some(e),
                });
            }
        }
        // The wolf running a deer down, and how far off it is.
        mind.pursuer = mind
            .chaser
            .and_then(|c| snaps.get(&c))
            .filter(|s| s.state == AnimalState::Chase && !s.dead)
            .map(|s| (s.pos, flat(s.pos - pos).length()));

        // ------------------------------------------------------------ act
        if let Some(mut b) = bird {
            fly(
                &mut mind,
                &mut b,
                &mut t,
                &mut loco,
                e,
                crate::seed::seed_of(&seeds, e),
                home,
                mates,
                snaps,
                now,
                dt,
                &w,
            );
            if let Some(mut a) = anim {
                a.set("flap", b.flap);
            }
            continue;
        }
        let run = motor.run_speed.max(0.1);
        let walk = (motor.walk_speed / run).max(0.1);
        match mind.state {
            AnimalState::Graze | AnimalState::Drink | AnimalState::Rest | AnimalState::Feed => {
                // Small steps now and then while grazing (a rooting boar shuffles on slowly, its
                // snout kept down).
                let shuffle = if species == Species::Boar {
                    walk * 0.35
                } else {
                    walk
                };
                if mind.state == AnimalState::Graze
                    && let Some(dest) = mind.destination
                    && walk_to(
                        pos,
                        dest,
                        shuffle,
                        &mut nav,
                        &mut w,
                        now,
                        &mut replans,
                        &mut intent,
                    )
                {
                    mind.destination = None;
                }
                // Drinking: up to its place at the edge, facing the water.
                if mind.state == AnimalState::Drink
                    && let Some((spot, face)) = mind.drink_at
                {
                    let to = flat(spot - pos);
                    if to.length() > 0.12 {
                        intent.direction = to.normalize() * walk * (to.length() / 0.6).min(0.6);
                    }
                    intent.look = Some(face);
                }
                if mind.state == AnimalState::Feed
                    && let Some(dest) = mind.destination
                    && walk_to(
                        pos,
                        dest,
                        walk * 2.0,
                        &mut nav,
                        &mut w,
                        now,
                        &mut replans,
                        &mut intent,
                    )
                {
                    mind.destination = None;
                }
            }
            AnimalState::Wander => {
                if let Some(dest) = mind.destination {
                    // Briskly, moving off from a gunshot.
                    let speed = if species == Species::Wolf || mind.wary.is_some() {
                        walk * 2.0
                    } else {
                        walk
                    };
                    if walk_to(
                        pos,
                        dest,
                        speed,
                        &mut nav,
                        &mut w,
                        now,
                        &mut replans,
                        &mut intent,
                    ) {
                        mind.destination = None;
                        mind.wary = None;
                        if species == Species::Wolf {
                            mind.set(AnimalState::Graze, now);
                            mind.wait_until = now + 3.0 + 6.0 * mind.random() as f64;
                        } else if mind.thirst > 0.75
                            && mind
                                .drink_at
                                .is_some_and(|d| flat(d.0 - pos).length() < 1.5)
                        {
                            mind.set(AnimalState::Drink, now);
                            mind.wait_until = now + 8.0 + 6.0 * mind.random() as f64;
                        } else {
                            mind.set(AnimalState::Graze, now);
                            mind.wait_until = now + 6.0 + 9.0 * mind.random() as f64;
                        }
                    }
                }
            }
            AnimalState::Alert => {
                if let Some(tp) = mind.threat_pos {
                    let d = Vec3::new((tp.x - pos.x) as f32, 0.0, (tp.z - pos.z) as f32);
                    if d.length() > 0.1 {
                        intent.look = Some(d.normalize());
                    }
                    // Wolves back off from people slowly, facing them.
                    if species == Species::Wolf && mind.danger > 0.6 {
                        intent.direction = -d.normalize_or_zero() * walk * 1.5;
                    }
                }
            }
            AnimalState::Flee => {
                let dir = flee_direction(
                    &mut mind,
                    e,
                    species,
                    pos,
                    home,
                    animal.range,
                    motor,
                    mates,
                    snaps,
                    now,
                    &w,
                );
                // A wounded animal runs lame; a deer out ahead of its fleeing herd eases off
                // for the rest to close up (unless a wolf is at its heels).
                let hurt = (1.0 - vitals.health / vitals.max_health.max(1e-3)).clamp(0.0, 1.0);
                let pace = if species == Species::Deer && mind.pursuer.is_none_or(|p| p.1 > 15.0) {
                    herd_pace(e, pos, dir, mates, snaps)
                } else {
                    1.0
                };
                // Stotting, it bounds high and goes on slowly.
                let stot = mind.stot(now);
                intent.direction = dir * (1.0 - 0.35 * hurt) * pace * (1.0 - 0.55 * stot);
                intent.look = Some(dir);
                intent.sprint = mind.stamina > 0.15
                    && (mind.danger > 0.8 || species != Species::Wolf)
                    && stot < 0.5;
                nav.clear();
            }
            AnimalState::Charge => {
                nav.clear();
                charge(&mut mind, pos, &w, now, &mut intent, traits);
            }
            AnimalState::Stalk | AnimalState::Chase => {
                hunt(
                    &mut mind,
                    e,
                    pos,
                    motor,
                    mates,
                    snaps,
                    now,
                    &mut nav,
                    &mut w,
                    &mut replans,
                    &mut intent,
                    traits,
                );
            }
            AnimalState::Dead | AnimalState::Perched | AnimalState::Flying => {}
        }
        // Howl at night.
        if species == Species::Wolf
            && w.settings.howl
            && night
            && mates.first() == Some(&e)
            && now >= mind.next_howl
        {
            if mind.next_howl > 0.0
                && matches!(
                    mind.state,
                    AnimalState::Graze | AnimalState::Rest | AnimalState::Wander
                )
            {
                noise.write(NoiseEvent {
                    position: pos + DVec3::Y,
                    radius: 220.0,
                    source: Some(e),
                });
            }
            mind.next_howl = now + 40.0 + 60.0 * mind.random() as f64;
        }
        if let Some(mut a) = anim {
            let head_down = matches!(
                mind.state,
                AnimalState::Graze | AnimalState::Drink | AnimalState::Feed
            ) && intent.direction.length_squared() < 1e-4;
            a.set("graze", if head_down { 1.0 } else { 0.0 });
            a.set(
                "rest",
                if mind.state == AnimalState::Rest {
                    1.0
                } else {
                    0.0
                },
            );
            a.set(
                "alert",
                if mind.state == AnimalState::Alert {
                    1.0
                } else {
                    0.0
                },
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decide(
    mind: &mut AnimalMind,
    species: Species,
    me: Entity,
    pos: DVec3,
    home: DVec3,
    animal: &Animal,
    mates: &[Entity],
    snaps: &HashMap<Entity, Snap>,
    now: f64,
    night: bool,
    danger_at: Option<DVec3>,
    w: &mut AnimalWorld,
) {
    let danger = mind.danger;
    if species == Species::Bird {
        if danger >= 1.0 && mind.state == AnimalState::Perched {
            mind.set(AnimalState::Flying, now);
        }
        return;
    }
    // ---------------------------------------------------------------- boars turn and fight
    if species == Species::Boar
        && !animal.young
        && boar_fights(mind, animal, me, pos, mates, snaps, now, w)
    {
        return;
    }
    // ---------------------------------------------------------------- danger first
    if danger >= 1.0 {
        if mind.state != AnimalState::Flee {
            mind.set(AnimalState::Flee, now);
            if let Some(at) = danger_at {
                let away = Vec3::new((pos.x - at.x) as f32, 0.0, (pos.z - at.z) as f32);
                if away.length() > 0.1 {
                    mind.flee_dir = away.normalize();
                }
            }
        }
        mind.flee_until = now + if species == Species::Wolf { 2.5 } else { 3.5 };
        mind.prey = None;
        return;
    }
    if mind.state == AnimalState::Flee {
        if now < mind.flee_until || danger > 0.45 {
            return;
        }
        mind.set(AnimalState::Alert, now);
        mind.wait_until = now + 3.0 + 3.0 * mind.random() as f64;
        return;
    }
    let hunting = matches!(mind.state, AnimalState::Stalk | AnimalState::Chase);
    if danger >= 0.55 && !hunting {
        if mind.state != AnimalState::Alert {
            mind.set(AnimalState::Alert, now);
        }
        mind.wait_until = now + 2.5 + 2.0 * mind.random() as f64;
        return;
    }
    if mind.state == AnimalState::Alert && now < mind.wait_until {
        return;
    }
    // ---------------------------------------------------------------- wolves hunt
    if species == Species::Wolf {
        // Its part in the hunt, by its place among the living pack, and what the pack is up
        // to: how long it has been running its quarry (from its first wolf to set off),
        // whether a mate just gave up, whether any is running, whether all are in place.
        let (mut n, mut slot, mut tester) = (0, 0, None);
        let mut pack_chase = mind.chase_from.map_or(0.0, |t| now - t);
        let (mut mate_gave_up, mut pack_charging, mut all_placed) = (false, false, mind.placed);
        let mut any_prey = None;
        let quarry =
            |p: Option<Entity>| p.filter(|p| snaps.get(p).is_some_and(|q| q.species.is_prey()));
        for &m in mates {
            let Some(s) = snaps.get(&m).filter(|s| !s.dead) else {
                continue;
            };
            if n == 0 {
                tester = Some(s);
            }
            if m == me {
                slot = n;
            }
            n += 1;
            if m == me {
                continue;
            }
            pack_chase = pack_chase.max(s.chasing.unwrap_or(0.0));
            mate_gave_up |= s.gave_up.is_some_and(|ago| ago < 2.0);
            pack_charging |= s.state == AnimalState::Chase;
            all_placed &= s.placed || s.state == AnimalState::Chase;
            any_prey = any_prey.or(quarry(s.prey));
        }
        let role = PackRole::of(slot, n.max(1));
        let resting = mind.gave_up.is_some_and(|at| now - at < PACK_REST_SECS);
        // The tester's quarry is the pack's (or else any mate's).
        let tester_prey = tester.filter(|_| slot > 0).and_then(|t| quarry(t.prey));
        let prey = if role == PackRole::Tester {
            mind.prey.or(any_prey)
        } else {
            tester_prey.or(mind.prey).or(any_prey)
        };
        if let Some(mut prey) = prey.filter(|_| !resting) {
            let Some(mut p) = snaps.get(&prey) else {
                mind.prey = None;
                mind.set(AnimalState::Graze, now);
                return;
            };
            mind.role = role;
            if p.dead {
                if mind.state != AnimalState::Feed {
                    mind.set(AnimalState::Feed, now);
                    let slot = mates.iter().position(|&m| m == me).unwrap_or(0) as f32;
                    let a = slot * 2.1;
                    mind.destination =
                        Some(p.pos + DVec3::new(a.cos() as f64 * 1.3, 0.0, a.sin() as f64 * 1.3));
                }
                mind.prey = Some(prey);
                mind.chase_from = None;
                mind.placed = false;
                if now - mind.since > 20.0 {
                    mind.hunger = 0.0;
                    mind.prey = None;
                    mind.role = PackRole::None;
                    mind.set(AnimalState::Rest, now);
                    mind.wait_until = now + 15.0;
                }
                return;
            }
            // The tester picks over the herd for the weakest as it stalks and as its rush
            // sets the herd running (a straggler shows itself then). Once the pack is running
            // one down it takes a clearly weaker one to turn it from it.
            let switch = match mind.state {
                AnimalState::Stalk => Some(0.75),
                AnimalState::Chase if now - mind.since < TEST_SECS => Some(0.5),
                _ => None,
            };
            if role == PackRole::Tester
                && let Some(switch) = switch
                && let Some((q, qs, score)) = weakest(p.herd, pos, snaps)
                && q != prey
                && score < switch * quarry_score(p, p.pos.distance(pos))
            {
                prey = q;
                p = qs;
            }
            let d = p.pos.distance(pos);
            // A running quarry is lost 70 m off; one it is still stalking only beyond its
            // scent (it picks a herd up by scent from further than it could run one down).
            let lost = if pack_chase > 0.0 || pack_charging || mind.state == AnimalState::Chase {
                70.0
            } else {
                SCENT_REACH + 10.0
            };
            // Tested, a fit quarry pulling away from the whole pack is let go.
            let mut outrun = false;
            if mind.state == AnimalState::Chase && pack_chase > TEST_SECS {
                let gap = mates
                    .iter()
                    .filter_map(|m| snaps.get(m))
                    .filter(|s| !s.dead)
                    .map(|s| flat(s.pos - p.pos).length())
                    .fold(d as f32, f32::min);
                match mind.gap_mark {
                    Some((at, was)) if now - at >= ESCAPE_SECS => {
                        let fit = p.hurt < 0.1 && !p.young;
                        outrun = fit && gap > ESCAPE_GAP && gap - was > ESCAPE_GAIN;
                        mind.gap_mark = Some((now, gap));
                    }
                    Some(_) => {}
                    None => mind.gap_mark = Some((now, gap)),
                }
            } else {
                mind.gap_mark = None;
            }
            if d > lost || pack_chase > PACK_CHASE_SECS || mate_gave_up || outrun {
                // Lost it, outrun, or run out of steam: the pack gives up and catches its
                // breath.
                mind.prey = None;
                mind.role = PackRole::None;
                mind.chase_from = None;
                mind.placed = false;
                mind.gave_up = Some(now);
                mind.set(AnimalState::Rest, now);
                mind.wait_until = now + PACK_REST_SECS;
                return;
            }
            mind.prey = Some(prey);
            if mind.state != AnimalState::Chase {
                let spooked = matches!(p.state, AnimalState::Flee | AnimalState::Alert);
                let stalk_secs = if mind.state == AnimalState::Stalk {
                    now - mind.since
                } else {
                    0.0
                };
                let rush = match role {
                    // Lies in wait until the quarry comes to it, or the chase goes another way;
                    // not in place yet when the pack sets off, it keeps heading for its place
                    // ahead of the quarry (rushing it head-on would only turn it back).
                    PackRole::Ambusher => {
                        let away = flat(pos - p.pos).dot(p.vel) <= 0.0;
                        let missed = pack_chase > 3.0 && d > 20.0 && away;
                        d < f64::from(AMBUSH_RANGE) || pack_charging && missed || stalk_secs > 60.0
                    }
                    // Rushes the herd once the pack is in place.
                    PackRole::Tester => {
                        all_placed || spooked || d < 9.0 || stalk_secs > 35.0 || pack_charging
                    }
                    _ => pack_charging || spooked || d < 9.0 || stalk_secs > 40.0,
                };
                if rush {
                    mind.set(AnimalState::Chase, now);
                    mind.chase_from.get_or_insert(now);
                } else if mind.state != AnimalState::Stalk {
                    mind.set(AnimalState::Stalk, now);
                    mind.placed = false;
                }
            }
            return;
        }
        mind.role = PackRole::None;
        mind.chase_from = None;
        mind.placed = false;
        let hungry =
            mind.hunger > 0.45 && !matches!(mind.state, AnimalState::Rest) || mind.hunger > 0.8;
        if hungry && !resting {
            // Look for prey it can see or smell.
            let mut best: Option<(Entity, f64)> = None;
            for (&e, s) in snaps {
                if !s.species.is_prey() || s.dead {
                    continue;
                }
                let d = s.pos.distance(pos);
                // Sight up close, scent trails further out.
                let reach = match s.species {
                    Species::Deer => SCENT_REACH,
                    Species::Boar => 60.0,
                    _ => 35.0,
                };
                if d > reach {
                    continue;
                }
                let eye = pos + DVec3::Y * 0.7;
                if d < 25.0 && w.world.raycast(eye, s.pos + DVec3::Y * 0.8).is_some() {
                    continue;
                }
                let score = quarry_score(s, d);
                if best.is_none_or(|b| score < b.1) {
                    best = Some((e, score));
                }
            }
            if let Some((prey, _)) = best {
                mind.prey = Some(prey);
                mind.set(AnimalState::Stalk, now);
                return;
            }
        }
    }
    // ---------------------------------------------------------------- calm life
    // Deer and boar bed down at midday and in the dark (`DayPart`); rabbits in the dark;
    // wolves are about at night.
    let bed_time = if keeps_hours(species) {
        mind.day.is_some_and(DayPart::bed_time)
    } else {
        night && species == Species::Rabbit
    };
    if mind.water.is_none() && species != Species::Wolf {
        mind.water = Some(find_water(home, animal.range * 3.0, &w.ground, &w.water));
    }
    let leader = mates.first().copied().filter(|&l| l != me);
    let herd_center = {
        let mut sum = DVec3::ZERO;
        let mut n = 0.0;
        for m in mates {
            if let Some(s) = snaps.get(m).filter(|s| !s.dead) {
                sum += s.pos;
                n += 1.0;
            }
        }
        if n > 0.0 { sum / n } else { pos }
    };
    let resting = mind.state == AnimalState::Rest;
    let should_rest = bed_time || mind.fatigue > 0.9;
    if resting {
        if now < mind.wait_until || bed_time {
            return;
        }
        mind.set(AnimalState::Graze, now);
        mind.wait_until = now + 4.0;
        return;
    }
    // Still drinking, or still on its way somewhere: it finishes first.
    let busy = mind.state == AnimalState::Wander
        || mind.state == AnimalState::Drink && now < mind.wait_until;
    // Deer and boar lie up close together; anything else tired lies down near its herd.
    let lie_up = if bed_time && keeps_hours(species) {
        BED_SPREAD
    } else {
        8.0
    };
    if should_rest && !busy && pos.distance(herd_center) < lie_up {
        mind.set(AnimalState::Rest, now);
        mind.wait_until = now + 30.0 + 30.0 * mind.random() as f64;
        return;
    }
    // Time to lie up, away from the rest: it joins them first.
    if bed_time && !busy && mind.state != AnimalState::Alert {
        let a = mind.random() * TAU;
        let r = BED_SPREAD as f32 * (0.3 + 0.5 * mind.random());
        mind.set(AnimalState::Wander, now);
        mind.destination = Some(
            herd_center + DVec3::new(a.cos() as f64 * r as f64, 0.0, a.sin() as f64 * r as f64),
        );
        return;
    }
    match mind.state {
        AnimalState::Wander => {
            // Followers keep up with a leader that moved on.
            if species == Species::Wolf
                && let Some(l) = leader.and_then(|l| snaps.get(&l))
                && l.pos.distance(pos) > 10.0
            {
                let slot = mates.iter().position(|&m| m == me).unwrap_or(0) as f64;
                let side = if (slot as usize).is_multiple_of(2) {
                    1.0
                } else {
                    -1.0
                };
                let back = -l.vel.normalize_or_zero().as_dvec3();
                let off = back * (2.0 + slot) + DVec3::new(-back.z, 0.0, back.x) * side * 1.5;
                mind.destination = Some(l.pos + off);
            }
            if mind.destination.is_none() {
                mind.set(AnimalState::Graze, now);
            }
        }
        AnimalState::Drink | AnimalState::Graze | AnimalState::Alert | AnimalState::Feed => {
            if mind.state == AnimalState::Drink && now >= mind.wait_until {
                mind.thirst = 0.0;
            }
            if now < mind.wait_until && mind.state != AnimalState::Alert {
                // Now and then a small step while grazing (a rooting boar shuffles on often).
                let (often, step) = if species == Species::Boar {
                    (0.12, 1.4)
                } else {
                    (0.04, 1.5)
                };
                if mind.state == AnimalState::Graze
                    && mind.destination.is_none()
                    && mind.random() < often
                {
                    let a = mind.random() * TAU;
                    let dir =
                        lean_to_wear(w.wear.as_deref(), pos, Vec3::new(a.cos(), 0.0, a.sin()));
                    mind.destination = Some(pos + dir.as_dvec3() * step);
                }
                return;
            }
            // Having looked up at a gunshot, it moves off from it.
            if let Some((at, when)) = mind.wary {
                if now - when < WARY_SECS {
                    let away = flat(pos - at).normalize_or(Vec3::X);
                    let dir = Quat::from_rotation_y((mind.random() - 0.5) * 0.6) * away;
                    mind.set(AnimalState::Wander, now);
                    mind.destination = Some(pos + (dir * WARY_DIST as f32).as_dvec3());
                    return;
                }
                mind.wary = None;
            }
            // Pick where to go next.
            let dest = if mind.thirst > 0.75
                && let Some(Some(water)) = mind.water
            {
                // Down to the water: its own place along the bank, facing the water, its
                // muzzle's reach back from the edge.
                let toward = flat(water - home).normalize_or(Vec3::X);
                let along = Vec3::new(-toward.z, 0.0, toward.x);
                let slot = mates.iter().position(|&m| m == me).unwrap_or(0) as f32;
                let gap = if species == Species::Deer { 1.3 } else { 1.0 };
                let off = (slot - (mates.len().max(1) - 1) as f32 * 0.5) * gap
                    + 0.3 * (mind.random() - 0.5);
                let near = water + (along * off).as_dvec3();
                let reach = drink_reach(species, animal.young);
                mind.drink_at = Some(
                    shore(near, toward, &w.ground, &w.water)
                        .map(|(edge, face)| (edge - face.as_dvec3() * reach, face))
                        .unwrap_or((water - toward.as_dvec3() * reach, toward)),
                );
                mind.drink_at.map_or(water, |d| d.0)
            } else if species == Species::Wolf
                && let Some(l) = leader.and_then(|l| snaps.get(&l)).map(|s| s.pos)
            {
                let a = mind.random() * TAU;
                l + DVec3::new(a.cos() as f64 * 4.0, 0.0, a.sin() as f64 * 4.0)
            } else {
                // Stay with the herd, inside the home range.
                let anchor = if mates.len() > 1 {
                    herd_center.lerp(home, 0.3)
                } else {
                    home
                };
                let spread = match species {
                    Species::Rabbit => 5.0,
                    Species::Wolf => animal.range * 0.6,
                    // Piglets keep close to the sounder.
                    Species::Boar if animal.young => 4.0,
                    _ => 9.0,
                };
                let a = mind.random() * TAU;
                let r = spread * mind.random().sqrt();
                let mut p =
                    anchor + DVec3::new(a.cos() as f64 * r as f64, 0.0, a.sin() as f64 * r as f64);
                let off = p - home;
                if off.length() > animal.range as f64 {
                    p = home + off.normalize() * animal.range as f64;
                }
                // Set off a little toward a worn path, if one is near.
                let go = flat(p - pos);
                if go.length() > 0.5 {
                    let dir = lean_to_wear(w.wear.as_deref(), pos, go.normalize());
                    p = pos + (dir * go.length()).as_dvec3();
                }
                p
            };
            mind.set(AnimalState::Wander, now);
            mind.destination = Some(dest);
        }
        _ => {
            mind.set(AnimalState::Graze, now);
            mind.wait_until = now + 3.0;
        }
    }
}

/// Turn a grazing or wandering animal's heading `dir` a little toward worn ground: it
/// compares the wear a few meters ahead to its left and right and turns up to `WEAR_LEAN`
/// toward the more worn side, so game trails form and get used, while herds still spread
/// out to graze. Unchanged without wear.
fn lean_to_wear(wear: Option<&GroundWear>, pos: DVec3, dir: Vec3) -> Vec3 {
    const WEAR_LEAN: f32 = 0.3;
    let Some(wear) = wear else {
        return dir;
    };
    let at = |turn: f32| {
        let d = Quat::from_rotation_y(turn) * dir;
        wear.wear(pos + d.as_dvec3() * 3.0)
    };
    // Positive: the side a positive turn faces is more worn.
    let pull = at(0.5) - at(-0.5);
    if pull.abs() < 0.02 {
        return dir;
    }
    Quat::from_rotation_y(pull.clamp(-1.0, 1.0) * WEAR_LEAN) * dir
}

/// Find the water's edge nearest `home`: walk rings outward until a sample is under water,
/// then step back to dry land.
fn find_water(home: DVec3, max: f32, ground: &Ground, water: &Water) -> Option<DVec3> {
    let wet = |p: DVec3| {
        let g = ground.height(p).unwrap_or(p.y);
        water
            .sample(DVec3::new(p.x, g, p.z))
            .is_some_and(|s| s.surface_height > g + 0.15)
    };
    let mut r = 6.0;
    while r <= max.max(6.0) {
        for k in 0..16 {
            let a = k as f64 / 16.0 * std::f64::consts::TAU;
            let dir = DVec3::new(a.cos(), 0.0, a.sin());
            let p = home + dir * r as f64;
            if wet(p) {
                let mut q = p;
                for _ in 0..(r as usize + 2) {
                    q -= dir;
                    if !wet(q) {
                        q.y = ground.height(q).unwrap_or(home.y);
                        return Some(q);
                    }
                }
            }
        }
        r += 6.0;
    }
    None
}

/// The water's edge from `near`, looking along `toward`: where the bank meets the water to a
/// few centimetres, and the way to the water there, square on to the bank.
fn shore(near: DVec3, toward: Vec3, ground: &Ground, water: &Water) -> Option<(DVec3, Vec3)> {
    let wet = |p: DVec3| {
        let g = ground.height(p).unwrap_or(p.y);
        water
            .sample(DVec3::new(p.x, g, p.z))
            .is_some_and(|s| s.surface_height > g + 0.15)
    };
    let dir = toward.as_dvec3();
    // Step from 4 m back to 8 m on for the first wet ground after dry.
    let mut dry = near - dir * 4.0;
    if wet(dry) {
        return None;
    }
    let mut t = -4.0;
    while t < 8.0 {
        t += 0.25;
        let p = near + dir * t;
        if wet(p) {
            // Halve the step down to a few centimetres.
            let mut wet_at = p;
            for _ in 0..4 {
                let mid = (dry + wet_at) * 0.5;
                if wet(mid) {
                    wet_at = mid;
                } else {
                    dry = mid;
                }
            }
            let mut edge = (dry + wet_at) * 0.5;
            edge.y = ground.height(dry).unwrap_or(near.y);
            // Square on to the bank: toward the water all round it.
            let mut face = Vec3::ZERO;
            for k in 0..16 {
                let a = k as f32 / 16.0 * TAU;
                let d = Vec3::new(a.cos(), 0.0, a.sin());
                if wet(edge + d.as_dvec3() * 1.5) {
                    face += d;
                }
            }
            return Some((edge, face.normalize_or(toward)));
        }
        dry = p;
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn walk_to(
    pos: DVec3,
    dest: DVec3,
    speed: f32,
    nav: &mut NavAgent,
    w: &mut AnimalWorld,
    now: f64,
    replans: &mut u32,
    out: &mut MoveIntent,
) -> bool {
    let flat = DVec2::new(dest.x - pos.x, dest.z - pos.z).length();
    if flat < 0.8 {
        nav.clear();
        return true;
    }
    let stale = nav.goal.is_none_or(|g| g.distance(dest) > 1.5)
        || (nav.path.is_empty() && flat > 1.0)
        || (nav.status == Some(PathStatus::Partial) && now - nav.planned_at > 3.0);
    if stale && *replans < 6 {
        *replans += 1;
        let heat = &w.heat;
        let mut ctx = w
            .grid
            .ctx(&w.world, &w.ground, &w.water)
            .worn(w.wear.as_deref());
        let mut cost = |p: DVec3| {
            let h = heat.sample(p + DVec3::Y * 0.5);
            if h > 120.0 {
                f32::INFINITY
            } else {
                (h / 20.0).max(0.0)
            }
        };
        let status = ctx.find_path(pos, dest, &mut cost, &mut nav.path);
        nav.status = Some(status);
        nav.goal = Some(dest);
        nav.planned_at = now;
        if status == PathStatus::Failed {
            nav.path.clear();
            nav.path.push(dest);
        }
        if status == PathStatus::Partial && nav.path.len() <= 1 {
            // Can't get closer: give up on this spot.
            nav.clear();
            return true;
        }
    }
    if let Some(wp) = nav.next_waypoint(pos, 0.6) {
        let d = Vec3::new((wp.x - pos.x) as f32, 0.0, (wp.z - pos.z) as f32);
        let len = d.length();
        if len > 1e-3 {
            out.direction = d / len * speed;
            out.look = Some(d / len);
        }
    }
    false
}

/// What running with its herd adds to a fleeing animal's course: it runs the way its fleeing
/// mates run and is drawn back to the middle of the herd (deer from 3 m out, harder the
/// further out; the rest from 6 m), and deer keep a little apart close up. A deer the pack is
/// running down (`singled`) is cut off from its herd: no pull to the middle and no running
/// with the others, only keeping clear of them, so it runs its own line from the wolves.
fn with_herd(
    deer: bool,
    singled: bool,
    me: Entity,
    pos: DVec3,
    mates: &[Entity],
    snaps: &HashMap<Entity, Snap>,
) -> Vec3 {
    let mut align = Vec3::ZERO;
    let mut center = DVec3::ZERO;
    let mut apart = Vec3::ZERO;
    let mut n = 0.0;
    for m in mates {
        let Some(s) = snaps.get(m) else { continue };
        if s.dead || s.pos.distance(pos) > 25.0 {
            continue;
        }
        if s.state == AnimalState::Flee && *m != me {
            align += s.flee_dir;
        }
        let off = flat(pos - s.pos);
        if deer && off.length() > 1e-3 && off.length() < 1.6 {
            apart += off.normalize() * (1.6 - off.length());
        }
        center += s.pos;
        n += 1.0;
    }
    if singled {
        return apart;
    }
    let mut dir = apart + align.normalize_or_zero() * if deer { 1.1 } else { 0.8 };
    if n > 1.0 {
        let to_center = flat(center / n - pos);
        let far = to_center.length();
        if deer && far > 3.0 {
            dir += to_center.normalize() * (0.5 + 0.2 * (far - 3.0)).min(1.6);
        } else if far > 6.0 {
            dir += to_center.normalize() * 0.5;
        }
    }
    dir
}

#[allow(clippy::too_many_arguments)]
fn flee_direction(
    mind: &mut AnimalMind,
    me: Entity,
    species: Species,
    pos: DVec3,
    home: DVec3,
    range: f32,
    motor: &NpcMotor,
    mates: &[Entity],
    snaps: &HashMap<Entity, Snap>,
    now: f64,
    w: &AnimalWorld,
) -> Vec3 {
    let mut dir = Vec3::ZERO;
    if let Some(away) = mind.escape {
        dir += away * 2.0;
    } else if let Some(tp) = mind.threat_pos {
        let away = Vec3::new((pos.x - tp.x) as f32, 0.0, (pos.z - tp.z) as f32);
        if away.length() > 0.1 {
            dir += away.normalize() * 2.0;
        }
    }
    // A deer runs from the wolf at its heels above all (one running alongside it, it leaves
    // to the rest of its flight).
    let deer = species == Species::Deer;
    let course = mind.flee_dir;
    let heels = mind
        .pursuer
        .filter(|&(at, d)| course.length_squared() < 0.1 || flat(at - pos).dot(course) < -0.5 * d);
    if deer && let Some((at, d)) = heels.filter(|p| p.1 < 15.0) {
        let away = flat(pos - at);
        if away.length() > 0.1 {
            dir += away.normalize() * 2.0 * (1.0 - d / 15.0);
        }
    }
    if mind.flee_dir.length_squared() > 0.1 {
        dir += mind.flee_dir * 1.2;
    }
    // Run with the herd, unless the pack is running it down.
    dir += with_herd(deer, deer && mind.pursuer.is_some(), me, pos, mates, snaps);
    // Don't run off the map: past twice the home range, curve back.
    let off = pos - home;
    let excess = off.length() as f32 - range * 2.0;
    if excess > 0.0 {
        dir -= off.normalize().as_vec3() * (excess / range).min(2.0) * 2.0;
    }
    dir.y = 0.0;
    let mut dir = dir.normalize_or(Vec3::X);
    // The course it holds (and its mates follow), before any jink.
    mind.flee_dir = dir;
    // Rabbits zig-zag; a deer cuts from side to side at sharp angles once a wolf is close
    // behind it.
    if species == Species::Rabbit {
        if now >= mind.next_zig {
            mind.zig = if mind.zig > 0.0 { -1.0 } else { 1.0 };
            mind.next_zig = now + 0.35 + 0.4 * mind.random() as f64;
        }
        dir = Quat::from_rotation_y(mind.zig * 0.6) * dir;
    } else if deer && heels.is_some_and(|p| p.1 < ZIGZAG_RANGE) {
        if now >= mind.next_zig {
            // Cuts the other way each time, unless that would take it at a wolf running out
            // on that side (a flanker): then it cuts away from it again.
            let toward = |side: f32| {
                let cut = Quat::from_rotation_y(side * 0.9) * dir;
                let mut sum = 0.0;
                for s in snaps.values() {
                    let off = flat(s.pos - pos);
                    let d = off.length();
                    if s.species == Species::Wolf && !s.dead && d > 0.1 && d < 15.0 {
                        sum += cut.dot(off / d).max(0.0) * (1.0 - d / 15.0);
                    }
                }
                sum
            };
            let next = if mind.zig > 0.0 { -1.0 } else { 1.0 };
            mind.zig = if toward(next) > toward(-next) + 0.3 {
                -next
            } else {
                next
            };
            mind.next_zig = now + 0.45 + 0.25 * mind.random() as f64;
        }
        dir = Quat::from_rotation_y(mind.zig * 0.9) * dir;
    }
    // Steer around walls, deep water and fire.
    let blocked = |d: Vec3| {
        [2.0f32, 4.5].iter().any(|&k| {
            let p = pos + (d * k).as_dvec3();
            let foot = w.ground.height(p).unwrap_or(pos.y);
            w.world.blocked_at(
                DVec3::new(p.x, pos.y.max(foot), p.z),
                motor.radius + 0.2,
                motor.height.min(1.2),
                None,
            ) || w.water.submersion(DVec3::new(p.x, foot, p.z)) > 0.5
                || w.heat.sample(p + DVec3::Y * 0.5) > 60.0
        })
    };
    if blocked(dir) {
        let side = if mind.random() < 0.5 { 1.0 } else { -1.0 };
        for k in 1..=5 {
            for s in [side, -side] {
                let cand = Quat::from_rotation_y(s * k as f32 * 0.5) * dir;
                if !blocked(cand) {
                    mind.flee_dir = cand;
                    return cand;
                }
            }
        }
        // No way out.
        mind.cornered_at = Some(now);
    }
    dir
}

/// How fast (0..1) a fleeing herd animal at `pos`, heading `dir`, runs to keep the herd
/// together: easing off when out ahead of the rest.
fn herd_pace(
    me: Entity,
    pos: DVec3,
    dir: Vec3,
    mates: &[Entity],
    snaps: &HashMap<Entity, Snap>,
) -> f32 {
    let (mut sum, mut n) = (DVec3::ZERO, 0.0);
    for m in mates {
        if let Some(s) = snaps
            .get(m)
            .filter(|s| !s.dead && *m != me && s.pos.distance(pos) < 25.0)
        {
            sum += s.pos;
            n += 1.0;
        }
    }
    if n < 1.0 {
        return 1.0;
    }
    let ahead = flat(pos - sum / n).dot(dir);
    (1.0 - 0.08 * (ahead - 2.0).max(0.0)).max(0.7)
}

/// A boar turns on what threatens it when it is cornered (its flight finds no way out, or the
/// threat is on top of it) or wounded, and a sow when her young are close by. It keeps on at
/// the threat until its tusks strike (or the threat gets away, or it runs out of steam), then
/// breaks off and runs for a moment before it may turn again. True when that decided what it
/// does.
#[allow(clippy::too_many_arguments)]
fn boar_fights(
    mind: &mut AnimalMind,
    animal: &Animal,
    me: Entity,
    pos: DVec3,
    mates: &[Entity],
    snaps: &HashMap<Entity, Snap>,
    now: f64,
    w: &AnimalWorld,
) -> bool {
    // Fire always means flight: a boar does not turn to fight in the flames (and leaves off
    // a charge that runs into them).
    if w.heat.sample(pos + DVec3::Y * 0.5) > 40.0 {
        return false;
    }
    let foe = boar_foe(mind, now, w);
    let target = foe.map(|f| f.1);
    let flat = |p: DVec3| DVec2::new(p.x - pos.x, p.z - pos.z).length() as f32;
    // Hurt close by, by something it can't find (gone, or no one at all): it stands its
    // ground facing where the blow came from, crest up, scenting for it, rather than charging
    // at nothing; then it carries on as its senses tell it.
    let hurt_at = mind.wounded.map(|(at, _, _)| at);
    let unseen = foe.is_some_and(|f| f.0.is_none()) && target.is_some_and(|p| flat(p) < 20.0);
    if unseen && hurt_at.is_some_and(|at| now - at < SEARCH_SECS) {
        if mind.state != AnimalState::Alert {
            mind.set(AnimalState::Alert, now);
        }
        mind.wait_until = mind.wait_until.max(now + 1.0);
        return true;
    }
    if mind.state == AnimalState::Charge {
        let struck = mind
            .slashed_at
            .is_some_and(|t| t >= mind.since && now - t > 0.35);
        if !struck
            && now - mind.since < 6.0
            && foe.is_some_and(|f| f.0.is_some() && flat(f.1) < 25.0)
        {
            return true;
        }
        // Away for a moment.
        mind.set(AnimalState::Flee, now);
        mind.flee_until = now + 2.0 + mind.random() as f64;
        mind.rally_at = now + 2.5 + 1.5 * mind.random() as f64;
        if let Some(p) = target.or(mind.threat_pos) {
            let away = Vec3::new((pos.x - p.x) as f32, 0.0, (pos.z - p.z) as f32);
            if away.length() > 0.1 {
                mind.flee_dir = away.normalize();
            }
        }
        return true;
    }
    if mind.danger < 0.5 || now < mind.rally_at {
        return false;
    }
    // Something real to turn on: what just hurt it, or a threat it senses now.
    let wounded = mind.wounded.is_some_and(|(at, _, _)| now - at < WOUND_SECS);
    let Some((Some(_), at, _)) = foe else {
        return false;
    };
    let d = flat(at);
    let cornered = d < 3.0 || (mind.cornered_at.is_some_and(|t| now - t < 1.0) && d < 12.0);
    let young = !animal.male
        && mates.iter().any(|m| {
            *m != me
                && snaps
                    .get(m)
                    .is_some_and(|s| s.young && !s.dead && s.pos.distance(pos) < 12.0)
        });
    if cornered || (wounded && d < 20.0) || (young && d < 14.0) {
        mind.set(AnimalState::Charge, now);
        return true;
    }
    false
}

/// How long a boar remembers who hurt it, and turns on them (s).
const WOUND_SECS: f64 = 15.0;
/// How long a boar hurt by something it can't find stands looking for it (s).
const SEARCH_SECS: f64 = 4.0;

/// What a boar turns on: whoever hurt it in the last [`WOUND_SECS`] (where they are now),
/// else a threat it sensed in the last second (where it is now); hurt by something it can't
/// find and sensing nothing, where the blow came from (with no one to strike). The entity to
/// strike (if there is one), where it is and how it is moving.
fn boar_foe(mind: &AnimalMind, now: f64, w: &AnimalWorld) -> Option<(Option<Entity>, DVec3, Vec3)> {
    let live = |e: Entity| w.seen.get(e).map(|p| (Some(e), p.position, p.velocity));
    let wound = mind.wounded.filter(|(at, _, _)| now - at < WOUND_SECS);
    let sensed = mind
        .threat
        .filter(|_| now - mind.threat_seen < 1.0)
        .and_then(live);
    match wound {
        Some((_, from, source)) => {
            source
                .and_then(live)
                .or(sensed)
                .or(Some((None, from, Vec3::ZERO)))
        }
        None => sensed,
    }
}

/// Boars: run at the threat, head down, and slash it with the tusks when close.
fn charge(
    mind: &mut AnimalMind,
    pos: DVec3,
    w: &AnimalWorld,
    now: f64,
    out: &mut MoveIntent,
    traits: Traits,
) {
    let Some((foe, at, vel)) = boar_foe(mind, now, w) else {
        return;
    };
    let to = Vec3::new((at.x - pos.x) as f32, 0.0, (at.z - pos.z) as f32);
    let d = to.length();
    // Lead a moving target a little.
    let aim = to + Vec3::new(vel.x, 0.0, vel.z) * (d / 9.0).min(0.6);
    let dir = aim.normalize_or(Vec3::X);
    out.direction = dir;
    out.look = Some(dir);
    out.sprint = mind.stamina > 0.1;
    // Only at someone: never a slash at nothing.
    if d < 1.5 && now >= mind.bite_ready && traits.bite > 0.0 && foe.is_some() {
        mind.bite = foe;
        mind.slashed_at = Some(now);
        mind.bite_ready = now + 1.0 + 0.3 * mind.random() as f64;
    }
}

/// How much a wolf wants `s`, `d` m away (lower is better): close and easy first. A deer,
/// and a boar (which fights back) only when nothing easier is about: a piglet sooner, a
/// tusker last of all; the wounded, the young and stragglers sooner still.
fn quarry_score(s: &Snap, d: f64) -> f64 {
    let pick = match s.species {
        Species::Deer => 1.0,
        Species::Boar if s.young => 1.6,
        Species::Boar if s.male => 4.0,
        Species::Boar => 2.6,
        _ => 1.8,
    };
    let weak = 1.0 + 3.0 * s.hurt + if s.young { 0.8 } else { 0.0 } + 1.5 * s.straggle;
    d * pick / weak as f64
}

/// The weakest of a herd as a wolf at `from` sees it, and its score.
fn weakest(
    herd: Option<u32>,
    from: DVec3,
    snaps: &HashMap<Entity, Snap>,
) -> Option<(Entity, &Snap, f64)> {
    let herd = herd?;
    snaps
        .iter()
        .filter(|(_, s)| s.herd == Some(herd) && s.species.is_prey() && !s.dead)
        .map(|(&e, s)| (e, s, quarry_score(s, s.pos.distance(from))))
        .min_by(|a, b| a.2.total_cmp(&b.2))
}

#[allow(clippy::too_many_arguments)]
fn hunt(
    mind: &mut AnimalMind,
    me: Entity,
    pos: DVec3,
    motor: &NpcMotor,
    mates: &[Entity],
    snaps: &HashMap<Entity, Snap>,
    now: f64,
    nav: &mut NavAgent,
    w: &mut AnimalWorld,
    replans: &mut u32,
    out: &mut MoveIntent,
    traits: Traits,
) {
    let Some(p) = mind.prey.and_then(|p| snaps.get(&p)) else {
        return;
    };
    let prey_pos = p.pos;
    let prey_vel = flat_v(p.vel);
    // The line the pack drives along: from its tester through the quarry.
    let tester = mates
        .iter()
        .filter_map(|m| snaps.get(m))
        .find(|s| !s.dead)
        .filter(|_| mind.role != PackRole::Tester)
        .map_or(pos, |s| s.pos);
    let drive = flat(prey_pos - tester).normalize_or(flat(prey_pos - pos).normalize_or(Vec3::X));
    let left = left_of(drive);
    // Chasers beyond the first spread out to its sides.
    let spread = if mates.iter().position(|&m| m == me).unwrap_or(0) % 2 == 0 {
        1.0
    } else {
        -1.0
    };
    let run = motor.run_speed.max(0.1);
    let stalk = motor.walk_speed / run * 1.6;
    let at = |along: f32, side: f32| prey_pos + (drive * along + left * side).as_dvec3();
    if mind.state == AnimalState::Stalk {
        // How far the rest of the herd spreads out from the quarry, to its left and right and
        // ahead of it: the pack keeps its distance from all of them, not just from it.
        let (mut wide_left, mut wide_right) = (0.0f32, 0.0f32);
        let (mut ahead, mut behind) = (0.0f32, 0.0f32);
        let (mut nearest, mut nearest_at) = (flat(prey_pos - pos).length(), prey_pos);
        for s in snaps.values() {
            if s.dead || s.herd.is_none() || s.herd != p.herd {
                continue;
            }
            let d = flat(s.pos - pos).length();
            if d < nearest {
                (nearest, nearest_at) = (d, s.pos);
            }
            let off = flat(s.pos - prey_pos);
            wide_left = wide_left.max(off.dot(left));
            wide_right = wide_right.max(-off.dot(left));
            ahead = ahead.max(off.dot(drive));
            behind = behind.max(-off.dot(drive));
        }
        let wide = |side: f32| if side > 0.0 { wide_left } else { wide_right }.min(12.0);
        let ahead = ahead.min(12.0);
        let behind = behind.min(8.0);
        let along_me = flat(pos - prey_pos).dot(drive);
        // Creep into place: the tester straight at the herd, the flankers out to its sides,
        // the ambusher round beyond it, keeping its distance until it is past, then lying in
        // wait where the quarry will run.
        let mut lying_in_wait = true;
        let (goal, speed) = match mind.role {
            PackRole::FlankLeft | PackRole::FlankRight => {
                // Out wide alongside the back of the herd first, then up to its post.
                let side = mind.role.side();
                let across = (10.0 + wide(side).min(6.0)) * side;
                if along_me < -(behind + 2.0) {
                    lying_in_wait = false;
                    (at(-behind, across), stalk * 2.2)
                } else {
                    (at(FLANK_POST, across), stalk * 2.2)
                }
            }
            PackRole::Ambusher => {
                // Wide round its side, wide again ahead of it, and only then in to its post
                // in front of it, so it never passes close enough to be seen.
                let off = flat(pos - prey_pos);
                let side = if off.dot(left) >= 0.0 { 1.0 } else { -1.0 };
                let along = off.dot(drive);
                if along < 4.0 {
                    lying_in_wait = false;
                    (at(9.0, (30.0 + wide(side)) * side), 1.0)
                } else if along < 20.0 + ahead {
                    lying_in_wait = false;
                    (at(28.0 + ahead, (26.0 + wide(side)) * side), 1.0)
                } else {
                    (at(24.0 + ahead, 0.0), stalk * 2.2)
                }
            }
            PackRole::Chaser => (at(-(10.0f32 + behind).max(14.0), 5.0 * spread), stalk * 1.6),
            _ => (at(-(10.0f32 + behind).max(14.0), 0.0), stalk * 1.6),
        };
        // Once the pack sets off, an ambusher out in front of the quarry lies still and low
        // where it is, to spring as it comes (moving, it would only show itself).
        let charging = mates
            .iter()
            .any(|m| *m != me && snaps.get(m).is_some_and(|s| s.state == AnimalState::Chase));
        if mind.role == PackRole::Ambusher && charging && along_me > 4.0 {
            nav.clear();
            out.direction = Vec3::ZERO;
            out.crouch = true;
            out.look = flat(prey_pos - pos).try_normalize();
            mind.placed = true;
            return;
        }
        // Backs off from any of the herd that wanders too close, before it is seen.
        let clear = 10.0;
        let away = flat(pos - nearest_at).normalize_or_zero();
        let step = if nearest < clear && away != Vec3::ZERO {
            pos + (away * (clear + 1.0 - nearest)).as_dvec3()
        } else {
            goal
        };
        walk_to(pos, step, speed, nav, w, now, replans, out);
        // Trots while the herd is well off, then creeps.
        out.crouch = nearest < 30.0;
        mind.abreast = None;
        let off = pos.distance(goal);
        let near_enough = match mind.role {
            PackRole::FlankLeft | PackRole::FlankRight => 4.0,
            _ => 2.5,
        };
        mind.placed = off < near_enough && lying_in_wait;
        let look = flat(prey_pos - pos);
        if look.length() > 0.1 && off < 3.0 {
            out.look = Some(look.normalize());
        }
        return;
    }
    // Chase. The tester and chasers drive the quarry, leading it; the flankers run out on
    // its sides, level with it, and cut in once they are; a sprung ambusher heads it off.
    nav.clear();
    let d = prey_pos.distance(pos) as f32;
    let heading = if prey_vel.length() > 2.0 {
        prey_vel.normalize()
    } else {
        drive
    };
    let t = (d / motor.sprint_speed.max(1.0)).min(1.2);
    let lead = prey_pos + (prey_vel * t).as_dvec3();
    let trailing =
        mind.role == PackRole::Ambusher && d > 3.0 && flat(pos - prey_pos).dot(heading) < 2.0;
    let aim = match mind.role {
        PackRole::FlankLeft | PackRole::FlankRight => {
            let side = left_of(heading) * mind.role.side();
            let off = flat(pos - prey_pos);
            if off.dot(heading) > -1.5 && off.dot(side) > 0.0 {
                mind.abreast.get_or_insert(now);
            }
            let flank = prey_pos + (prey_vel * t.min(0.25)).as_dvec3();
            let across = off.dot(side).max(0.0);
            let level = off.dot(heading) > -1.0;
            let tiring = prey_vel.length() < motor.sprint_speed * 0.9;
            match mind.abreast {
                // Close enough, or level with it as it tires or turns: cut in at its flank.
                _ if d < 3.0 => flank,
                Some(at) if level && tiring && now - at > 1.0 => flank,
                // Up with it: run alongside it in a lane out on its side, closing it off from
                // that side, easing in to the lane (quicker from far out) and aiming well ahead
                // so as not to lose ground.
                Some(_) => {
                    let lane = (across - 2.0).min(across * 0.7).max(4.5);
                    prey_pos + (prey_vel * 0.6 + side * lane).as_dvec3()
                }
                // Still behind: run out wide and up level with it.
                None => lead + (side * 5.0 + heading * 2.0).as_dvec3(),
            }
        }
        PackRole::Chaser if d > 6.0 => lead + (left * 3.0 * spread).as_dvec3(),
        // Sprung, the ambusher heads the quarry off while it is still coming on; once it is
        // past, it drops back onto its trail well behind it and follows on, rather than
        // cutting in from the side it was on (which only turns it into the flanker on the
        // other side).
        PackRole::Ambusher if trailing => prey_pos - (heading * 12.0).as_dvec3(),
        _ => lead,
    };
    let dir = flat(aim - pos).normalize_or(Vec3::X);
    out.direction = dir;
    out.look = Some(dir);
    // (Following on, it runs, saving itself, unless it is falling far behind.)
    out.sprint = mind.stamina > 0.15 && !(trailing && d < 18.0);
    if d < 1.6 && now >= mind.bite_ready && traits.bite > 0.0 {
        mind.bite = mind.prey;
        mind.bite_ready = now + 0.9 + 0.4 * mind.random() as f64;
    }
}

#[allow(clippy::too_many_arguments)]
fn fly(
    mind: &mut AnimalMind,
    b: &mut BirdFlight,
    t: &mut Transform,
    loco: &mut LocomotionState,
    me: Entity,
    seed: u64,
    home: DVec3,
    mates: &[Entity],
    snaps: &HashMap<Entity, Snap>,
    now: f64,
    dt: f32,
    w: &AnimalWorld,
) {
    let pos = t.translation;
    let floor = w.world.surface_height(pos, pos.y + 0.5, 0.6, &w.ground);
    match mind.state {
        AnimalState::Perched => {
            // Peck and hop about.
            if now >= b.hop_at {
                b.hop_at = now + 0.8 + 2.5 * mind.random() as f64;
                let a = mind.random() * TAU;
                b.velocity = Vec3::new(a.cos() * 0.8, 1.6, a.sin() * 0.8);
            }
            b.velocity.y -= 9.81 * dt;
            let mut p = pos + (b.velocity * dt).as_dvec3();
            // Wander back if it hopped too far.
            if p.distance(home) > 10.0 {
                let back = (home - p).normalize() * 0.02;
                p += DVec3::new(back.x, 0.0, back.z);
            }
            let floor = w.world.surface_height(p, pos.y + 0.3, 0.4, &w.ground);
            if p.y <= floor {
                p.y = floor;
                b.velocity = Vec3::ZERO;
            }
            let flat = Vec3::new(b.velocity.x, 0.0, b.velocity.z);
            if flat.length() > 0.1 {
                t.rotation = Quat::from_rotation_y((-flat.x).atan2(-flat.z));
            }
            t.translation = p;
            b.flap = 0.0;
            loco.velocity = b.velocity;
            loco.mode = LocomotionMode::Idle;
            loco.grounded = true;
            // A mate taking off lifts the flock.
            let flock_up = mates.iter().any(|m| {
                *m != me
                    && snaps.get(m).is_some_and(|s| {
                        s.state == AnimalState::Flying && s.age < 2.0 && s.pos.distance(pos) < 30.0
                    })
            });
            if flock_up && mind.contagion_at.is_none() {
                mind.contagion_at = Some(now + 0.05 + 0.3 * mind.random() as f64);
            }
            if mind.contagion_at.is_some_and(|at| now >= at) {
                mind.contagion_at = None;
                mind.set(AnimalState::Flying, now);
            }
            if mind.state == AnimalState::Flying {
                b.airborne_since = now;
                b.landing = None;
                let a = mind.random() * TAU;
                b.velocity = Vec3::new(a.cos() * 2.0, 6.0, a.sin() * 2.0);
            }
        }
        AnimalState::Flying => {
            let mut acc = Vec3::ZERO;
            let mut sep = Vec3::ZERO;
            let mut align = Vec3::ZERO;
            let mut center = DVec3::ZERO;
            let mut n = 0.0;
            for m in mates {
                if *m == me {
                    continue;
                }
                let Some(s) = snaps.get(m) else { continue };
                if s.state != AnimalState::Flying {
                    continue;
                }
                let d = s.pos - pos;
                let dist = d.length() as f32;
                if dist < 2.0 && dist > 1e-3 {
                    sep -= d.as_vec3() / (dist * dist);
                }
                if dist < 14.0 {
                    align += s.vel;
                    center += s.pos;
                    n += 1.0;
                }
            }
            acc += sep * 6.0;
            if n > 0.0 {
                acc += (align / n - b.velocity) * 0.8;
                acc += (center / n as f64 - pos).as_vec3() * 0.4;
            }
            let airborne = (now - b.airborne_since) as f32;
            let calm = (now - mind.last_danger) as f32;
            if let Some(land) = b.landing {
                // Glide down to the landing spot.
                let to = (land - pos).as_vec3();
                let dist = to.length();
                let want = to.normalize_or_zero() * (dist * 1.2).clamp(1.5, 8.0);
                acc = (want - b.velocity) * 3.0;
                if dist < 0.4 || pos.y <= floor + 0.05 {
                    mind.set(AnimalState::Perched, now);
                    b.velocity = Vec3::ZERO;
                    t.translation = DVec3::new(pos.x, floor, pos.z);
                    return;
                }
            } else {
                // Climb to cruising height and circle home.
                let cruise =
                    floor + 12.0 + 4.0 * ((now * 0.3 + (seed >> 40) as f64 / 1024.0).sin());
                acc.y += ((cruise - pos.y) as f32 * 1.5 - b.velocity.y).clamp(-8.0, 8.0);
                let off = (pos - home).as_vec3();
                let flat = Vec3::new(off.x, 0.0, off.z);
                let r = flat.length();
                if r > 1.0 {
                    let tangent = Vec3::new(-flat.z, 0.0, flat.x) / r;
                    acc += tangent * 2.0 - flat / r * (r - 14.0).max(-4.0) * 0.25;
                }
                if let Some(tp) = mind.threat_pos
                    && calm < 3.0
                {
                    let away = (pos - tp).as_vec3();
                    let ad = away.length();
                    if ad < 20.0 {
                        acc += away.normalize_or_zero() * 10.0 * (1.0 - ad / 20.0);
                    }
                }
                if airborne < 0.8 {
                    acc.y += 10.0;
                }
                if airborne > 10.0 && calm > 8.0 {
                    let a = mind.random() * TAU;
                    let r = 8.0 * mind.random().sqrt();
                    let mut spot = home
                        + DVec3::new(a.cos() as f64 * r as f64, 0.0, a.sin() as f64 * r as f64);
                    spot.y = w.world.surface_height(spot, spot.y + 30.0, 40.0, &w.ground);
                    b.landing = Some(spot);
                }
            }
            acc = acc.clamp_length_max(16.0);
            b.velocity += acc * dt;
            let speed = b.velocity.length();
            let max = 11.0;
            let min = if b.landing.is_some() { 0.0 } else { 4.0 };
            if speed > max {
                b.velocity *= max / speed;
            } else if speed < min && speed > 1e-3 {
                b.velocity *= min / speed;
            }
            let mut p = pos + (b.velocity * dt).as_dvec3();
            if p.y < floor + 0.05 && b.landing.is_none() {
                p.y = floor + 0.05;
                b.velocity.y = b.velocity.y.max(2.0);
            }
            t.translation = p;
            let v = b.velocity;
            if v.length() > 0.5 {
                let yaw = (-v.x).atan2(-v.z);
                let pitch = (v.y / v.length()).asin() * 0.6;
                // Roll into the turn: the wing on the inside of the turn goes down (+z is
                // backward, so a positive roll lifts the right wing, for a turn to the left).
                let left = Vec3::new(v.z, 0.0, -v.x).normalize_or_zero();
                let bank = (acc.dot(left) * 0.06).clamp(-0.8, 0.8);
                t.rotation = Quat::from_rotation_y(yaw)
                    * Quat::from_rotation_x(pitch)
                    * Quat::from_rotation_z(bank);
            }
            let flap_rate = if v.y > 0.5 || airborne < 1.5 {
                18.0
            } else {
                9.0
            };
            b.flap = (b.flap + flap_rate * dt) % TAU;
            loco.velocity = v;
            loco.mode = LocomotionMode::Air;
            loco.grounded = false;
        }
        _ => {
            mind.set(AnimalState::Perched, now);
        }
    }
}

/// Wolves' bites and boars' tusks land as damage.
pub(crate) fn animal_bites(
    mut q: Query<(Entity, &Animal, &Transform, &mut AnimalMind)>,
    mut damage: MessageWriter<DamageEvent>,
    mut triggers: MessageWriter<AnimationTrigger>,
) {
    for (e, animal, t, mut mind) in &mut q {
        let Some(target) = mind.bite.take() else {
            continue;
        };
        damage.write(DamageEvent {
            target,
            amount: animal.species.traits().bite,
            kind: DamageKind::Slash,
            point: t.translation + DVec3::Y * 0.6,
            impulse: (t.forward() * 120.0),
            source: Some(e),
        });
        triggers.write(AnimationTrigger {
            entity: e,
            name: "attack_light".into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deer_at(pos: DVec3, state: AnimalState, flee_dir: Vec3) -> Snap {
        Snap {
            species: Species::Deer,
            young: false,
            male: false,
            pos,
            vel: Vec3::ZERO,
            state,
            age: 1.0,
            flee_dir,
            prey: None,
            dead: false,
            sensed: true,
            threat_pos: None,
            herd: Some(1),
            hurt: 0.0,
            straggle: 0.0,
            placed: false,
            chasing: None,
            gave_up: None,
        }
    }

    #[test]
    fn a_chased_deer_is_cut_off_from_its_herd() {
        // A deer running north (+Z), its herd 10 m off to the east (+X) running east, and
        // one mate brushing past it 1 m to the west.
        let id = |k| Entity::from_raw_u32(k).unwrap();
        let me = id(1);
        let mut snaps = HashMap::new();
        snaps.insert(me, deer_at(DVec3::ZERO, AnimalState::Flee, Vec3::Z));
        for k in 0..3 {
            let at = DVec3::new(10.0, 0.0, f64::from(k) * 2.0 - 2.0);
            snaps.insert(id(2 + k), deer_at(at, AnimalState::Flee, Vec3::X));
        }
        let mates: Vec<Entity> = (1..5).map(id).collect();
        // Running with its herd, it is drawn east, toward the herd's middle and the way the
        // herd runs.
        let free = with_herd(true, false, me, DVec3::ZERO, &mates, &snaps);
        assert!(free.x > 1.0, "a free deer runs with its herd: {free}");
        // Run down by the pack, nothing of that: no pull and no alignment at all.
        let cut = with_herd(true, true, me, DVec3::ZERO, &mates, &snaps);
        assert_eq!(cut, Vec3::ZERO, "the chased deer is cut off from its herd");
        // It still keeps clear of a mate it brushes past, and only that.
        snaps.insert(
            id(5),
            deer_at(DVec3::new(-1.0, 0.0, 0.0), AnimalState::Flee, Vec3::X),
        );
        let mates: Vec<Entity> = (1..6).map(id).collect();
        let cut = with_herd(true, true, me, DVec3::ZERO, &mates, &snaps);
        assert!(
            (cut - Vec3::new(0.6, 0.0, 0.0)).length() < 1e-5,
            "it only keeps clear of the mate beside it: {cut}"
        );
    }

    #[test]
    fn species_round_trip_and_factions() {
        for s in Species::ALL {
            assert_eq!(Species::parse(s.name()), Some(s));
        }
        assert_eq!(Species::Wolf.faction(), Faction::PREDATOR);
        assert_eq!(Species::Deer.faction(), Faction::PREY);
        assert!(Species::Deer.is_prey() && !Species::Wolf.is_prey());
        // Deer outrun wolves in a sprint; wolves outlast them.
        let (deer, wolf) = (Species::Deer, Species::Wolf);
        assert!(deer.motor().sprint_speed > wolf.motor().sprint_speed);
        assert!(deer.traits().stamina_drain > wolf.traits().stamina_drain);
        // Boars are prey that fights back, and wolves run them down.
        let boar = Species::Boar;
        assert_eq!(boar.faction(), Faction::PREY);
        assert!(boar.is_prey() && boar.traits().bite > 0.0);
        assert!(wolf.motor().sprint_speed > boar.motor().sprint_speed);
    }

    #[test]
    fn grazing_steps_lean_a_little_toward_a_worn_trail() {
        /// A worn trail along z = 2.5.
        struct Trail;
        impl engine_world::WearQuery for Trail {
            fn wear(&self, p: DVec3) -> f32 {
                (1.0 - ((p.z - 2.5).abs() as f32 - 0.5)).clamp(0.0, 1.0)
            }
        }
        let wear = GroundWear(std::sync::Arc::new(Trail));
        let pos = DVec3::ZERO;
        // Heading along +X with the trail off to +Z: it turns toward the trail, a little.
        let dir = lean_to_wear(Some(&wear), pos, Vec3::X);
        assert!(dir.z > 0.05, "{dir}");
        assert!(dir.angle_between(Vec3::X) <= 0.31, "{dir}");
        assert!((dir.length() - 1.0).abs() < 1e-4);
        // Heading away from it, it still mostly goes its own way.
        let away = lean_to_wear(Some(&wear), pos, -Vec3::Z);
        assert!(away.z < -0.9, "{away}");
        // Without worn ground (or with none near), headings are left alone.
        assert_eq!(lean_to_wear(None, pos, Vec3::X), Vec3::X);
        let untouched = GroundWear::default();
        assert_eq!(lean_to_wear(Some(&untouched), pos, Vec3::X), Vec3::X);
    }
}
