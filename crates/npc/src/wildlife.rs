//! Dynamic fauna: deer and rabbits that graze, wander as a herd, drink at water, bed down at
//! night and bolt when something comes near; wild boar that root about in sounders (sows with
//! their young) or alone (males) and run, but turn and charge with their tusks when cornered or
//! wounded, or when a sow's young are threatened; wolf packs that roam, stalk, encircle and run
//! prey down, then feed; bird flocks that peck on the ground and take off together.
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
//! * each other: when one herd member bolts the rest follow a moment later, alert deer warn
//!   the herd, and one bird taking off lifts the whole flock; wolves give a charging boar
//!   room.

use std::collections::HashMap;

use engine_core::math::hash_u64;
use engine_core::prelude::*;
use engine_world::animation::{AnimationParams, AnimationTrigger};
use engine_world::locomotion::{LocomotionMode, LocomotionState, MoveIntent};
use engine_world::{
    DamageEvent, DamageKind, Ground, HeatField, NoiseEvent, Water, WeatherState, WindField,
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
    /// What it is watching or running from.
    pub threat: Option<Entity>,
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
    /// Hurt since the last think, by something at this position.
    pub hurt_from: Option<DVec3>,
    /// When it was last hurt (animal clock) and where the blow came from (kept after the
    /// think that reacts to it).
    pub wounded: Option<(f64, DVec3)>,
    /// Wolves: bite this animal this tick.
    pub bite: Option<Entity>,
    pub bite_ready: f64,
    /// Searched for water: `Some(None)` = none in range.
    pub water: Option<Option<DVec3>>,
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
pub(crate) fn init_animals(mut commands: Commands, mut q: Query<InitItem, Added<Animal>>) {
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
        mind.rng = hash_u64(e.to_bits() ^ 0x00A4_1AA1);
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

/// Noises and damage that reach animals.
pub(crate) fn animal_senses(
    mut noises: MessageReader<NoiseEvent>,
    mut hurts: MessageReader<DamageEvent>,
    positions: Query<&Transform>,
    mut q: Query<(
        Entity,
        &Animal,
        &Transform,
        &NpcClock,
        &mut AnimalMind,
        &mut NpcVitals,
    )>,
) {
    let noises: Vec<NoiseEvent> = noises.read().copied().collect();
    if !noises.is_empty() {
        for (e, animal, t, _, mut mind, _) in &mut q {
            let hearing = animal.species.traits().hearing;
            for n in &noises {
                if n.source == Some(e) {
                    continue;
                }
                let d = n.position.distance(t.translation) as f32;
                let reach = n.radius * hearing;
                if d >= reach {
                    continue;
                }
                // Loudness here: 1 at the source .. 0 at the edge, plus how big the noise is.
                let level = (1.0 - d / reach) * (n.radius / 30.0).min(3.0);
                if mind.heard.is_none_or(|(_, l)| level > l) {
                    mind.heard = Some((n.position, level));
                }
            }
        }
    }
    for h in hurts.read() {
        let Ok((_, _, t, clock, mut mind, mut vitals)) = q.get_mut(h.target) else {
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
        mind.wounded = Some((clock.now, from));
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
    since: f64,
    flee_dir: Vec3,
    prey: Option<Entity>,
    dead: bool,
    /// Sensed danger itself in the last second (not just caught it from the herd).
    sensed: bool,
}

/// The world as animals need it.
#[derive(SystemParam)]
pub(crate) struct AnimalWorld<'w> {
    world: Res<'w, TacticalWorld>,
    grid: ResMut<'w, NavGrid>,
    ground: Res<'w, Ground>,
    water: Res<'w, Water>,
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
) {
    if !w.settings.enabled {
        return;
    }
    // Snapshot everyone first, so animals can read each other while updating themselves.
    let mut snaps: HashMap<Entity, Snap> = HashMap::new();
    let mut herds: HashMap<u32, Vec<Entity>> = HashMap::new();
    for (e, animal, herd, mind, _, t, loco, clock, _, vitals, _, bird, _) in q.iter() {
        snaps.insert(
            e,
            Snap {
                species: animal.species,
                young: animal.young,
                male: animal.male,
                pos: t.translation,
                vel: bird.map(|b| b.velocity).unwrap_or(loco.velocity),
                state: mind.state,
                since: mind.since,
                flee_dir: mind.flee_dir,
                prey: mind.prey,
                dead: vitals.downed,
                sensed: clock.now - mind.last_danger < 1.0,
            },
        );
        if let Some(h) = herd {
            herds.entry(h.0).or_default().push(e);
        }
    }
    for list in herds.values_mut() {
        list.sort();
    }
    let daylight = w.weather.as_ref().map(|w| w.daylight()).unwrap_or(1.0);
    let mut replans = 0u32;
    let mut near = Vec::new();
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
            let asleep = mind.state == AnimalState::Rest;
            let eye = pos + DVec3::Y * species.body_height() as f64;
            let sight = traits.sight * (0.45 + 0.55 * daylight) * if asleep { 0.5 } else { 1.0 };
            let wind = w
                .wind
                .as_ref()
                .map(|wf| wf.sample(eye))
                .unwrap_or(Vec3::ZERO);
            let range = sight.max(traits.smell).max(traits.flight * 3.0);
            w.seen.near(pos, range as f64, &mut near);
            let mut best: Option<(Entity, DVec3, f32)> = None;
            for &i in &near {
                let p = &w.seen.list[i as usize];
                if p.entity == e {
                    continue;
                }
                let other = snaps.get(&p.entity);
                let d = DVec2::new(p.position.x - pos.x, p.position.z - pos.z).length() as f32;
                // Wolves give a boar charging at them room.
                let charging = other.is_some_and(|o| {
                    o.species == Species::Boar && o.state == AnimalState::Charge && d < 8.0
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
                let mut sensed = false;
                if d < 4.0 {
                    sensed = true;
                } else if d < sight * f.max(0.6) {
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
            }
            let mut danger = 0.0;
            let mut danger_at = None;
            if let Some((te, tp, dg)) = best {
                danger = dg;
                danger_at = Some(tp);
                mind.threat = Some(te);
                mind.threat_pos = Some(tp);
            }
            // Being hurt: run, whatever it is.
            if let Some(from) = mind.hurt_from.take() {
                danger = danger.max(2.0);
                danger_at = Some(from);
                mind.threat_pos = Some(from);
            }
            // Noise: loud and close = run; otherwise look.
            if let Some((at, level)) = mind.heard.take() {
                let dg = if level > 1.0 { 1.2 } else { level * 0.9 };
                if dg > danger {
                    danger = dg;
                    danger_at = Some(at);
                    mind.threat = None;
                    mind.threat_pos = Some(at);
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
            }
            mind.danger = danger;
            if danger >= 0.5 {
                mind.last_danger = now;
            }

            // Herd: panic spreads, alert mates make it look up (only from mates that sensed
            // the danger themselves, so a calm herd doesn't keep itself on edge).
            if species != Species::Wolf {
                for &m in mates {
                    if m == e {
                        continue;
                    }
                    let Some(s) = snaps.get(&m) else { continue };
                    let d = s.pos.distance(pos);
                    let fled = matches!(s.state, AnimalState::Flee | AnimalState::Flying)
                        && now - s.since < 4.0
                        && s.sensed;
                    if fled
                        && d < 40.0
                        && mind.contagion_at.is_none()
                        && !matches!(mind.state, AnimalState::Flee | AnimalState::Flying)
                    {
                        let delay = 0.12 + 0.35 * mind.random() as f64;
                        mind.contagion_at = Some(now + delay);
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
                if mind.contagion_at.is_some_and(|at| now >= at) {
                    mind.contagion_at = None;
                    if danger < 1.0 {
                        danger = 1.0;
                        if danger_at.is_none() {
                            let away = -mind.flee_dir;
                            danger_at = Some(pos + (away * 10.0).as_dvec3());
                            mind.threat_pos = danger_at;
                        }
                    }
                }
                mind.danger = danger;
            }

            decide(
                &mut mind, species, e, pos, home, animal, mates, &snaps, now, night, danger_at,
                &mut w,
            );
        }

        // ------------------------------------------------------------ act
        if let Some(mut b) = bird {
            fly(
                &mut mind, &mut b, &mut t, &mut loco, e, home, mates, &snaps, now, dt, &w,
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
                    let speed = if species == Species::Wolf {
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
                        if species == Species::Wolf {
                            mind.set(AnimalState::Graze, now);
                            mind.wait_until = now + 3.0 + 6.0 * mind.random() as f64;
                        } else if mind.thirst > 0.75 && mind.water.flatten().is_some() {
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
                    species,
                    pos,
                    home,
                    animal.range,
                    motor,
                    mates,
                    &snaps,
                    now,
                    &w,
                );
                intent.direction = dir;
                intent.look = Some(dir);
                intent.sprint =
                    mind.stamina > 0.15 && (mind.danger > 0.8 || species != Species::Wolf);
                mind.flee_dir = dir;
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
                    &snaps,
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
        // A pack mate's quarry is the pack's quarry.
        let pack_prey = mates.iter().filter_map(|m| snaps.get(m)).find_map(|s| {
            s.prey
                .filter(|p| snaps.get(p).is_some_and(|q| q.species.is_prey()))
        });
        if let Some(prey) = mind.prey.or(pack_prey) {
            let Some(p) = snaps.get(&prey) else {
                mind.prey = None;
                mind.set(AnimalState::Graze, now);
                return;
            };
            if p.dead {
                if mind.state != AnimalState::Feed {
                    mind.set(AnimalState::Feed, now);
                    let slot = mates.iter().position(|&m| m == me).unwrap_or(0) as f32;
                    let a = slot * 2.1;
                    mind.destination =
                        Some(p.pos + DVec3::new(a.cos() as f64 * 1.3, 0.0, a.sin() as f64 * 1.3));
                }
                mind.prey = Some(prey);
                if now - mind.since > 20.0 {
                    mind.hunger = 0.0;
                    mind.prey = None;
                    mind.set(AnimalState::Rest, now);
                    mind.wait_until = now + 15.0;
                }
                return;
            }
            let d = p.pos.distance(pos);
            let chasing_secs = if mind.state == AnimalState::Chase {
                now - mind.since
            } else {
                0.0
            };
            if d > 70.0 || chasing_secs > 18.0 {
                // Lost it: catch breath.
                mind.prey = None;
                mind.set(AnimalState::Rest, now);
                mind.wait_until = now + 8.0;
                return;
            }
            mind.prey = Some(prey);
            if mind.state != AnimalState::Chase {
                let close = mates
                    .iter()
                    .filter_map(|m| snaps.get(m))
                    .filter(|s| !s.dead)
                    .all(|s| s.pos.distance(p.pos) < 16.0);
                let spooked = matches!(p.state, AnimalState::Flee | AnimalState::Alert);
                let stalk_secs = if mind.state == AnimalState::Stalk {
                    now - mind.since
                } else {
                    0.0
                };
                let pack_charging = mates
                    .iter()
                    .filter_map(|m| snaps.get(m))
                    .any(|s| s.state == AnimalState::Chase);
                if close || spooked || d < 9.0 || stalk_secs > 25.0 || pack_charging {
                    mind.set(AnimalState::Chase, now);
                } else if mind.state != AnimalState::Stalk {
                    mind.set(AnimalState::Stalk, now);
                }
            }
            return;
        }
        if mind.hunger > 0.45 && !matches!(mind.state, AnimalState::Rest) || mind.hunger > 0.8 {
            // Look for prey it can see or smell.
            let mut best: Option<(Entity, f64)> = None;
            for (&e, s) in snaps {
                if !s.species.is_prey() || s.dead {
                    continue;
                }
                let d = s.pos.distance(pos);
                // Sight up close, scent trails further out; prefer deer, and a boar (which
                // fights back) only when nothing easier is about: a piglet sooner, a tusker
                // last of all.
                let (reach, pick) = match s.species {
                    Species::Deer => (110.0, 1.0),
                    Species::Boar if s.young => (60.0, 1.6),
                    Species::Boar if s.male => (60.0, 4.0),
                    Species::Boar => (60.0, 2.6),
                    _ => (35.0, 1.8),
                };
                if d > reach {
                    continue;
                }
                let eye = pos + DVec3::Y * 0.7;
                if d < 25.0 && w.world.raycast(eye, s.pos + DVec3::Y * 0.8).is_some() {
                    continue;
                }
                // Prefer close.
                let score = d * pick;
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
    // Wolves and boars are about at night; the others bed down.
    let nocturnal = matches!(species, Species::Wolf | Species::Boar);
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
    let should_rest = (night && !nocturnal) || mind.fatigue > 0.9;
    if resting {
        if now < mind.wait_until || (night && !nocturnal) {
            return;
        }
        mind.set(AnimalState::Graze, now);
        mind.wait_until = now + 4.0;
        return;
    }
    if should_rest && pos.distance(herd_center) < 8.0 && mind.state != AnimalState::Wander {
        mind.set(AnimalState::Rest, now);
        mind.wait_until = now + 30.0 + 30.0 * mind.random() as f64;
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
                    mind.destination =
                        Some(pos + DVec3::new(a.cos() as f64 * step, 0.0, a.sin() as f64 * step));
                }
                return;
            }
            // Pick where to go next.
            let dest = if mind.thirst > 0.75
                && let Some(Some(water)) = mind.water
            {
                let a = mind.random() * TAU;
                water + DVec3::new(a.cos() as f64 * 2.0, 0.0, a.sin() as f64 * 2.0)
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
        let mut ctx = w.grid.ctx(&w.world, &w.ground, &w.water);
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

#[allow(clippy::too_many_arguments)]
fn flee_direction(
    mind: &mut AnimalMind,
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
    if let Some(tp) = mind.threat_pos {
        let away = Vec3::new((pos.x - tp.x) as f32, 0.0, (pos.z - tp.z) as f32);
        if away.length() > 0.1 {
            dir += away.normalize() * 2.0;
        }
    }
    if mind.flee_dir.length_squared() > 0.1 {
        dir += mind.flee_dir * 1.2;
    }
    // Run with the herd.
    let mut align = Vec3::ZERO;
    let mut center = DVec3::ZERO;
    let mut n = 0.0;
    for m in mates {
        let Some(s) = snaps.get(m) else { continue };
        if s.dead || s.pos.distance(pos) > 25.0 {
            continue;
        }
        if s.state == AnimalState::Flee {
            align += s.flee_dir;
        }
        center += s.pos;
        n += 1.0;
    }
    dir += align.normalize_or_zero() * 0.8;
    if n > 1.0 {
        let to_center = center / n - pos;
        if to_center.length() > 6.0 {
            dir += to_center.normalize().as_vec3() * 0.5;
        }
    }
    // Don't run off the map: past twice the home range, curve back.
    let off = pos - home;
    let excess = off.length() as f32 - range * 2.0;
    if excess > 0.0 {
        dir -= off.normalize().as_vec3() * (excess / range).min(2.0) * 2.0;
    }
    dir.y = 0.0;
    let mut dir = dir.normalize_or(Vec3::X);
    // Rabbits zig-zag.
    if species == Species::Rabbit {
        if now >= mind.next_zig {
            mind.zig = if mind.zig > 0.0 { -1.0 } else { 1.0 };
            mind.next_zig = now + 0.35 + 0.4 * mind.random() as f64;
        }
        dir = Quat::from_rotation_y(mind.zig * 0.6) * dir;
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
                    return cand;
                }
            }
        }
        // No way out.
        mind.cornered_at = Some(now);
    }
    dir
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
    let live = mind.threat.and_then(|t| w.seen.get(t)).map(|p| p.position);
    let target = live.or(mind.threat_pos);
    let flat = |p: DVec3| DVec2::new(p.x - pos.x, p.z - pos.z).length() as f32;
    if mind.state == AnimalState::Charge {
        let struck = mind
            .slashed_at
            .is_some_and(|t| t >= mind.since && now - t > 0.35);
        if !struck && now - mind.since < 6.0 && target.is_some_and(|p| flat(p) < 25.0) {
            return true;
        }
        // Away for a moment.
        mind.set(AnimalState::Flee, now);
        mind.flee_until = now + 2.0 + mind.random() as f64;
        mind.rally_at = now + 2.5 + 1.5 * mind.random() as f64;
        if let Some(p) = target {
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
    // Something real to turn on: a threat it senses now, or what just hurt it.
    let wounded = mind.wounded.is_some_and(|(at, _)| now - at < 15.0);
    let Some(at) = target.filter(|_| live.is_some() || wounded) else {
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

/// Boars: run at the threat, head down, and slash it with the tusks when close.
fn charge(
    mind: &mut AnimalMind,
    pos: DVec3,
    w: &AnimalWorld,
    now: f64,
    out: &mut MoveIntent,
    traits: Traits,
) {
    let live = mind
        .threat
        .and_then(|t| w.seen.get(t))
        .map(|p| (p.position, p.velocity));
    let Some((at, vel)) = live.or(mind.threat_pos.map(|p| (p, Vec3::ZERO))) else {
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
    if d < 1.5 && now >= mind.bite_ready && traits.bite > 0.0 {
        if live.is_some() {
            mind.bite = mind.threat;
        }
        mind.slashed_at = Some(now);
        mind.bite_ready = now + 1.0 + 0.3 * mind.random() as f64;
    }
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
    let prey_vel = p.vel;
    let alive: Vec<(Entity, DVec3)> = mates
        .iter()
        .filter_map(|m| snaps.get(m).filter(|s| !s.dead).map(|s| (*m, s.pos)))
        .collect();
    let n = alive.len().max(1) as f32;
    let slot = alive.iter().position(|(m, _)| *m == me).unwrap_or(0) as f32;
    let pack_center = alive.iter().fold(DVec3::ZERO, |a, (_, p)| a + *p) / n as f64;
    let from_prey = Vec3::new(
        (pack_center.x - prey_pos.x) as f32,
        0.0,
        (pack_center.z - prey_pos.z) as f32,
    )
    .normalize_or(Vec3::X);
    // Spread around the prey: each wolf takes its own angle.
    let angle = (slot - (n - 1.0) / 2.0) * 0.9;
    let spread = Quat::from_rotation_y(angle) * from_prey;
    let run = motor.run_speed.max(0.1);
    if mind.state == AnimalState::Stalk {
        let goal = prey_pos + (spread * 11.0).as_dvec3();
        walk_to(
            pos,
            goal,
            motor.walk_speed / run * 1.6,
            nav,
            w,
            now,
            replans,
            out,
        );
        out.crouch = true;
        let look = Vec3::new(
            (prey_pos.x - pos.x) as f32,
            0.0,
            (prey_pos.z - pos.z) as f32,
        );
        if look.length() > 0.1 && pos.distance(goal) < 3.0 {
            out.look = Some(look.normalize());
        }
        return;
    }
    // Chase: lead the prey; wolves on the sides cut it off.
    nav.clear();
    let d = prey_pos.distance(pos) as f32;
    let t = (d / motor.sprint_speed.max(1.0)).min(1.2);
    let mut aim = prey_pos + (prey_vel * t).as_dvec3();
    if d > 6.0 {
        aim += (spread * (d * 0.25).min(5.0)).as_dvec3();
    }
    let to = Vec3::new((aim.x - pos.x) as f32, 0.0, (aim.z - pos.z) as f32);
    let dir = to.normalize_or(Vec3::X);
    out.direction = dir;
    out.look = Some(dir);
    out.sprint = mind.stamina > 0.15;
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
                        s.state == AnimalState::Flying
                            && now - s.since < 2.0
                            && s.pos.distance(pos) < 30.0
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
                let cruise = floor + 12.0 + 4.0 * ((now * 0.3 + me.index_u32() as f64).sin());
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
}
