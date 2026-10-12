//! The components that make an entity an NPC, and its runtime state.

use engine_core::prelude::*;
use engine_world::animation::AnimationParams;
use engine_world::locomotion::{LocomotionState, MoveIntent};

use crate::faction::Faction;
use crate::nav::NavAgent;
use crate::perception::NpcMemory;
use crate::skills::{NpcSkills, Perception};

/// Makes an entity an AI-controlled fighter. Requires (and adds with defaults) skills,
/// perception, a weapon, vitals, a faction, a motor and the runtime state components.
///
/// The NPC drives its body through `MoveIntent`, the same input a player produces, so the
/// movement module (or the NPC module's built-in fallback mover) moves it.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[require(
    NpcSkills,
    Perception,
    NpcWeapon,
    NpcVitals,
    Faction,
    NpcMotor,
    MoveIntent,
    LocomotionState,
    AnimationParams,
    NpcBrain,
    NpcMemory,
    NpcCombat,
    NavAgent,
    NpcClock,
    crate::locomotion::MoverState
)]
pub struct Npc {
    /// Where it patrols and returns to; `None` = where it was spawned.
    pub home: Option<DVec3>,
    /// Patrol wander radius around home, meters.
    pub patrol_radius: f32,
    /// Thinks and acts when true (false = statue, for cutscenes).
    pub active: bool,
    /// Override the weapon's preferred fighting distance, meters.
    pub preferred_range: Option<f32>,
}

impl Default for Npc {
    fn default() -> Self {
        Self {
            home: None,
            patrol_radius: 8.0,
            active: true,
            preferred_range: None,
        }
    }
}

/// What a weapon is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WeaponStyle {
    #[default]
    Melee,
    Ranged,
}

/// The NPC's weapon. Presets: [`NpcWeapon::sword`], [`sword_and_shield`](NpcWeapon::sword_and_shield),
/// [`rifle`](NpcWeapon::rifle), [`bow`](NpcWeapon::bow), [`claws`](NpcWeapon::claws).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NpcWeapon {
    pub style: WeaponStyle,
    /// Damage per hit.
    pub damage: f32,
    /// Melee reach from the body center, meters.
    pub reach: f32,
    /// Ranged: effective range, meters (beyond it accuracy falls off fast).
    pub range: f32,
    /// Seconds from starting an attack (the telegraph) to the hit.
    pub windup: f32,
    /// Seconds after the hit before it can act again (open to counters).
    pub recovery: f32,
    /// Seconds between attacks.
    pub cooldown: f32,
    /// Ranged: shot spread at accuracy 0, degrees (accuracy 1 = a tenth of this).
    pub spread_deg: f32,
    /// Ranged: projectile speed, m/s.
    pub projectile_speed: f32,
    /// Carries a shield: blocks from the front also stop arrows and bullets.
    pub shield: bool,
    /// Can block melee at all.
    pub can_block: bool,
}

impl Default for NpcWeapon {
    fn default() -> Self {
        Self::sword()
    }
}

impl NpcWeapon {
    pub fn sword() -> Self {
        Self {
            style: WeaponStyle::Melee,
            damage: 25.0,
            reach: 1.9,
            range: 0.0,
            windup: 0.45,
            recovery: 0.45,
            cooldown: 0.9,
            spread_deg: 0.0,
            projectile_speed: 0.0,
            shield: false,
            can_block: true,
        }
    }

    pub fn sword_and_shield() -> Self {
        Self {
            shield: true,
            damage: 20.0,
            windup: 0.5,
            ..Self::sword()
        }
    }

    pub fn rifle() -> Self {
        Self {
            style: WeaponStyle::Ranged,
            damage: 18.0,
            reach: 1.2,
            range: 40.0,
            windup: 0.35,
            recovery: 0.15,
            cooldown: 0.55,
            spread_deg: 7.0,
            projectile_speed: 600.0,
            shield: false,
            can_block: false,
        }
    }

    pub fn bow() -> Self {
        Self {
            damage: 30.0,
            range: 35.0,
            windup: 0.8,
            cooldown: 1.2,
            spread_deg: 5.0,
            projectile_speed: 60.0,
            ..Self::rifle()
        }
    }

    /// Teeth and claws for predators.
    pub fn claws() -> Self {
        Self {
            damage: 15.0,
            reach: 1.4,
            windup: 0.3,
            recovery: 0.5,
            cooldown: 1.0,
            can_block: false,
            ..Self::sword()
        }
    }

    /// Named preset: `sword`, `sword_and_shield`, `rifle`, `bow`, `claws`.
    pub fn preset(name: &str) -> Option<Self> {
        Some(match name {
            "sword" => Self::sword(),
            "sword_and_shield" => Self::sword_and_shield(),
            "rifle" => Self::rifle(),
            "bow" => Self::bow(),
            "claws" => Self::claws(),
            _ => return None,
        })
    }

    /// Distance it likes to fight at, meters.
    pub fn ideal_range(&self) -> f32 {
        match self.style {
            WeaponStyle::Melee => self.reach * 0.85,
            WeaponStyle::Ranged => (self.range * 0.4).clamp(6.0, 22.0),
        }
    }
}

/// Health and balance. Damage comes in through `DamageEvent`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NpcVitals {
    pub health: f32,
    pub max_health: f32,
    /// Balance: hits and blocked heavy blows drain it; at 0 the NPC staggers (open to a
    /// finisher) and it refills. 0..1.
    pub poise: f32,
    /// Seconds of stagger when poise breaks.
    pub stagger_secs: f32,
    /// Out of the fight (health reached 0).
    pub downed: bool,
}

impl Default for NpcVitals {
    fn default() -> Self {
        Self {
            health: 100.0,
            max_health: 100.0,
            poise: 1.0,
            stagger_secs: 0.9,
            downed: false,
        }
    }
}

impl NpcVitals {
    /// Health fraction 0..1.
    pub fn fraction(&self) -> f32 {
        if self.max_health <= 0.0 {
            0.0
        } else {
            (self.health / self.max_health).clamp(0.0, 1.0)
        }
    }
}

/// Body and speeds, used to plan movement (and by the fallback mover to move).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NpcMotor {
    /// m/s.
    pub walk_speed: f32,
    /// m/s; `MoveIntent::direction` of length 1 means this speed.
    pub run_speed: f32,
    /// m/s with `MoveIntent::sprint`.
    pub sprint_speed: f32,
    /// m/s².
    pub acceleration: f32,
    /// rad/s.
    pub turn_rate: f32,
    /// Dash length, meters (dodges and gap closers).
    pub dash_distance: f32,
    /// Seconds a dash takes.
    pub dash_secs: f32,
    /// Body radius, meters.
    pub radius: f32,
    /// Body height, meters.
    pub height: f32,
    /// Highest step, meters.
    pub step_height: f32,
}

impl Default for NpcMotor {
    fn default() -> Self {
        Self {
            walk_speed: 1.6,
            run_speed: 4.5,
            sprint_speed: 6.8,
            acceleration: 20.0,
            turn_rate: 9.0,
            dash_distance: 3.5,
            dash_secs: 0.22,
            radius: 0.35,
            height: 1.8,
            step_height: 0.4,
        }
    }
}

/// The NPC's own clock: world time scaled by its `LocalTimeScale`, so everything it does,
/// including reacting, slows down with it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default)]
pub struct NpcClock {
    /// Seconds of the NPC's own time since it spawned.
    pub now: f64,
    /// This tick's step, seconds of the NPC's own time.
    pub dt: f32,
    /// How fast the NPC's time runs relative to world time (1 = same).
    pub rate: f32,
}

/// How alert an NPC is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum AlertLevel {
    /// Going about its business.
    #[default]
    Relaxed,
    /// Heard or glimpsed something.
    Suspicious,
    /// Lost an enemy it knows is there.
    Searching,
    /// Fighting.
    Combat,
}

/// What the NPC is doing (the winner of its utility scoring).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum NpcAction {
    #[default]
    Idle,
    /// Wander around home.
    Patrol,
    /// Go look at a noise or glimpse.
    Investigate,
    /// Sweep the area where an enemy was last seen.
    Search,
    /// Fight from the current position or a good firing spot (melee: close in, circle,
    /// strike when there's an opening).
    Engage,
    /// Move to a spot hidden from the enemy.
    TakeCover,
    /// Go around to the enemy's side or back.
    Flank,
    /// Climb to higher ground for a better firing angle.
    SeekHighGround,
    /// Shoot an explosive next to the enemy.
    UseExplosive,
    /// Pull back to cover near allies when hurt or outnumbered.
    Retreat,
    /// Follow a direct order (hold, move, attack).
    Obey,
    /// Downed.
    Down,
}

impl NpcAction {
    pub const ALL: [NpcAction; 12] = [
        NpcAction::Idle,
        NpcAction::Patrol,
        NpcAction::Investigate,
        NpcAction::Search,
        NpcAction::Engage,
        NpcAction::TakeCover,
        NpcAction::Flank,
        NpcAction::SeekHighGround,
        NpcAction::UseExplosive,
        NpcAction::Retreat,
        NpcAction::Obey,
        NpcAction::Down,
    ];

    pub fn name(self) -> &'static str {
        match self {
            NpcAction::Idle => "idle",
            NpcAction::Patrol => "patrol",
            NpcAction::Investigate => "investigate",
            NpcAction::Search => "search",
            NpcAction::Engage => "engage",
            NpcAction::TakeCover => "take_cover",
            NpcAction::Flank => "flank",
            NpcAction::SeekHighGround => "seek_high_ground",
            NpcAction::UseExplosive => "use_explosive",
            NpcAction::Retreat => "retreat",
            NpcAction::Obey => "obey",
            NpcAction::Down => "down",
        }
    }
}

/// A direct order (from game code, a squad leader or the `npc.order` API).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum NpcOrder {
    /// Stay at a point and fight from there.
    Hold { at: DVec3 },
    /// Walk to a point.
    MoveTo { at: DVec3 },
    /// Attack this entity (by bits, see `Entity::to_bits`).
    Attack { target: u64 },
}

/// The NPC's decision state. Read it to see what an NPC is thinking (`npc.inspect`).
#[derive(Component, Debug, Clone, Default)]
pub struct NpcBrain {
    pub action: NpcAction,
    /// NPC clock time the action started.
    pub action_since: f64,
    pub alert: AlertLevel,
    /// Current enemy.
    pub target: Option<Entity>,
    /// Where it is going (cover spot, flank spot, investigation point…).
    pub destination: Option<DVec3>,
    /// Why it picked the destination (shown by `npc.inspect` and the tactical overlay).
    pub destination_reason: &'static str,
    /// Next time it re-thinks, NPC clock.
    pub next_think: f64,
    /// Next time it re-picks a position, NPC clock.
    pub next_position: f64,
    /// Utility scores from the last think, for debugging.
    pub scores: Vec<(NpcAction, f32)>,
    /// Circle direction for melee spacing (+1 / −1).
    pub strafe: f32,
    pub next_strafe_flip: f64,
    /// A direct order overrides utility scoring until done.
    pub order: Option<NpcOrder>,
    /// Squad role this think (`"suppress"`, `"flank"`, `"wait"`, `"attack"`).
    pub role: &'static str,
    /// Random stream for this NPC's decisions.
    pub rng_state: u64,
    /// Where it came from (patrol center when `Npc::home` is not set).
    pub home: Option<DVec3>,
    /// Committed to closing in and striking until (NPC clock).
    pub committed_until: f64,
    /// Next roll to commit to an attack (NPC clock).
    pub next_commit_roll: f64,
    /// The dodge already started for this defense window (its `until`).
    pub dodge_issued: f64,
    /// A high spot near the fight it knows about.
    pub high_ground: Option<DVec3>,
    pub next_high_ground_check: f64,
    /// Patrol/search: wait here until (NPC clock).
    pub wait_until: f64,
    /// The explosive it means to shoot.
    pub explosive: Option<Entity>,
    /// Top candidate spots from the last position choice (for `npc.inspect`).
    pub spots: Vec<crate::tactics::Spot>,
    /// The last spot chosen was in cover.
    pub in_cover: bool,
    /// The cover there is waist-high (crouch behind it).
    pub low_cover: bool,
}

impl NpcBrain {
    /// Deterministic per-NPC random number in 0..1.
    pub fn random(&mut self) -> f32 {
        self.rng_state = hash_u64(self.rng_state.wrapping_add(0x9e37_79b9_7f4a_7c15));
        (self.rng_state >> 40) as f32 / (1u64 << 24) as f32
    }

    pub(crate) fn switch(&mut self, action: NpcAction, now: f64) {
        if self.action != action {
            self.action = action;
            self.action_since = now;
            self.next_position = now;
        }
    }
}

/// What an NPC is doing with its weapon.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AttackPhase {
    #[default]
    Ready,
    /// Telegraphing an attack that lands at `hit_at` (NPC clock). A feint cancels before it.
    Windup {
        target: Option<Entity>,
        hit_at: f64,
        heavy: bool,
        feint: bool,
        /// Ranged: where it's aiming (world point).
        aim: DVec3,
    },
    /// Recovering after a swing or shot until `until`: open to counters.
    Recovery { until: f64 },
    /// Off balance until `until` (parried, poise broken): can't attack or defend.
    Staggered { until: f64 },
}

/// How an NPC is defending right now.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Defense {
    #[default]
    None,
    /// Guard up until `until`.
    Block { until: f64 },
    /// A timed deflection that staggers the attacker if the hit lands before `until`.
    Parry { until: f64 },
    /// Sidestepping until `until` (attacks miss).
    Dodge { until: f64 },
}

/// An attack the NPC noticed coming (from an `AttackEvent`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IncomingAttack {
    pub attacker: Entity,
    /// When it lands, NPC clock.
    pub lands_at: f64,
    /// When the NPC gets to react, NPC clock (`now + reaction_time`).
    pub react_at: f64,
    pub ranged: bool,
    pub reach: f32,
    pub handled: bool,
}

/// Counters for tests, tuning and `npc.inspect`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct CombatStats {
    pub attacks: u32,
    pub hits: u32,
    pub blocked_by_target: u32,
    pub feints: u32,
    pub blocks: u32,
    pub parries: u32,
    pub dodges: u32,
    pub hits_taken: u32,
    pub reacted_too_late: u32,
    pub shots: u32,
    pub explosives_used: u32,
}

/// Weapon and defense state.
#[derive(Component, Debug, Clone, Default)]
pub struct NpcCombat {
    pub phase: AttackPhase,
    pub defense: Defense,
    /// Next attack allowed at (NPC clock).
    pub ready_at: f64,
    pub incoming: Vec<IncomingAttack>,
    /// 0..1: how pinned down by fire it is (decays).
    pub suppression: f32,
    /// NPC clock time it last took damage.
    pub last_hurt: f64,
    pub stats: CombatStats,
}

impl NpcCombat {
    pub fn is_blocking(&self, now: f64) -> bool {
        matches!(self.defense, Defense::Block { until } | Defense::Parry { until } if now < until)
    }

    pub fn is_dodging(&self, now: f64) -> bool {
        matches!(self.defense, Defense::Dodge { until } if now < until)
    }

    pub fn is_vulnerable(&self, now: f64) -> bool {
        matches!(self.phase, AttackPhase::Recovery { until } | AttackPhase::Staggered { until } if now < until)
    }

    pub fn is_staggered(&self, now: f64) -> bool {
        matches!(self.phase, AttackPhase::Staggered { until } if now < until)
    }
}

/// Global NPC settings.
#[derive(Resource, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct NpcSettings {
    /// Move NPCs and animals with the built-in fallback mover (MoveIntent → Transform with
    /// ground following and collision against static obstacles). It steps aside on its own
    /// when a movement module has claimed `LocomotionBackend`.
    pub builtin_locomotion: bool,
    /// The NPC module resolves its own NPCs' hits (melee reach checks, shot accuracy) and
    /// sends `DamageEvent`s. Turn off if a combat module does it.
    pub resolve_hits: bool,
    /// Seconds between thinks for an NPC near the focus (doubles with distance).
    pub think_secs: f32,
    /// Beyond this distance from every focus, NPCs think and perceive 8× less often, meters.
    pub far_distance: f32,
    /// Show where each NPC is going and why (rings and lines in the world).
    pub show_intent: bool,
    /// Downed NPCs get back up after this many seconds (0 = stay down). Demos use it.
    pub revive_secs: f32,
}

impl Default for NpcSettings {
    fn default() -> Self {
        Self {
            builtin_locomotion: true,
            resolve_hits: true,
            think_secs: 0.2,
            far_distance: 150.0,
            show_intent: false,
            revive_secs: 0.0,
        }
    }
}
