//! Combat movement: attacks with a windup, a strike and a recovery; dash-strikes that close
//! ten meters in a blink; blocking; dodging (a dash's first moments make attacks miss); and
//! counters (hitting a fighter while it recovers does extra damage).
//!
//! Players and NPC AI start attacks the same way, through `MoveIntent::attack` (plus
//! `dash_to` for a dash-strike). The movement module owns the timing, so it is also the one
//! that sends the [`AttackEvent`] telegraph every defender, camera and slow-motion system
//! reacts to, and the [`DamageEvent`] when the blade connects.
//!
//! All timings are in the fighter's own seconds: a fighter with `Tempo::speed = 2.5` winds up,
//! strikes and recovers 2.5 times faster in world time.

use std::collections::VecDeque;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::animation::{AnimationTrigger, Ragdoll};
use engine_world::locomotion::{LocomotionState, MoveIntent};
use engine_world::physics::CharacterController;
use engine_world::{AttackEvent, AttackKind, DamageEvent, DamageKind, NoiseEvent};

use crate::lockon::LockOn;
use crate::mover::{DashRequest, Influence, Mover, flat};
use crate::profile::MovementProfile;

/// A character that fights: team, health, reach and attack timing. Pair with a
/// [`MovementProfile`]. Every field is tunable live.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(CombatState)]
#[serde(default)]
pub struct Fighter {
    /// Fighters on the same team never hit each other.
    pub team: u32,
    pub max_health: f32,
    pub health: f32,
    /// Subtract every `DamageEvent` aimed at this fighter from `health` and fall when it runs
    /// out. Turn off when the game keeps health elsewhere.
    pub track_health: bool,
    /// Damage of one strike.
    pub damage: f32,
    /// How far the blade reaches from the body's surface, m.
    pub reach: f32,
    /// Width of the swing, degrees.
    pub arc_deg: f32,
    /// Own-time seconds from starting a swing to the hit.
    pub windup_secs: f32,
    /// How long the blade is out after the hit, s.
    pub strike_secs: f32,
    /// Recovery after a swing: open to counters, s.
    pub recovery_secs: f32,
    /// Windup after arriving from a dash-strike, s (short: the dash is the telegraph).
    pub dash_windup_secs: f32,
    /// Longest dash-strike, m.
    pub dash_strike_range: f32,
    /// A melee swing at an enemy beyond reach (but within `dash_strike_range`, in front)
    /// becomes a dash-strike.
    pub auto_dash_strike: bool,
    /// Step toward the target during the windup, m/s.
    pub lunge_speed: f32,
    /// Stick multiplier while attacking.
    pub attack_move_scale: f32,
    /// Stick multiplier while blocking.
    pub block_move_scale: f32,
    /// Fraction of damage that goes through a block.
    pub block_damage_scale: f32,
    /// Blocks only stop hits from within this arc in front, degrees.
    pub block_arc_deg: f32,
    /// Damage multiplier when the target is recovering or staggered.
    pub counter_multiplier: f32,
    /// Stunned this long when hit, s.
    pub stagger_secs: f32,
    /// Push on the target per hit, N·s.
    pub knockback: f32,
}

impl Default for Fighter {
    fn default() -> Self {
        Self {
            team: 0,
            max_health: 100.0,
            health: 100.0,
            track_health: true,
            damage: 25.0,
            reach: 1.6,
            arc_deg: 110.0,
            windup_secs: 0.28,
            strike_secs: 0.1,
            recovery_secs: 0.45,
            dash_windup_secs: 0.1,
            dash_strike_range: 9.0,
            auto_dash_strike: true,
            lunge_speed: 3.0,
            attack_move_scale: 0.2,
            block_move_scale: 0.4,
            block_damage_scale: 0.15,
            block_arc_deg: 140.0,
            counter_multiplier: 2.0,
            stagger_secs: 0.35,
            knockback: 160.0,
        }
    }
}

impl Fighter {
    /// A fighter on `team` with `health`.
    pub fn new(team: u32, health: f32) -> Self {
        Self {
            team,
            max_health: health,
            health,
            ..Self::default()
        }
    }
}

/// Where an attack is in its swing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttackPhase {
    /// Dashing in (dash-strike).
    DashIn,
    Windup,
    /// The blade is out; the hit was resolved when this phase began.
    Strike,
    /// Open to counters.
    Recovery,
}

/// One attack in progress.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AttackState {
    pub kind: AttackKind,
    pub phase: AttackPhase,
    /// Own-time seconds left in this phase.
    pub secs_left: f32,
    /// Own-time seconds since the attack started.
    pub secs: f32,
    #[serde(skip)]
    pub target: Option<Entity>,
    /// Swing direction (horizontal, unit).
    pub dir: Vec3,
}

/// How a strike ended for one target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Hit,
    /// Hit a recovering or staggered fighter: extra damage.
    Counter,
    Blocked,
    /// The target was dashing out of the way.
    Dodged,
    /// Nobody in reach.
    Whiff,
}

/// Combat runtime state (added with [`Fighter`]).
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CombatState {
    pub attack: Option<AttackState>,
    /// Stunned for this many seconds (own time).
    pub stagger_secs: f32,
    pub blocking: bool,
    /// Out of the fight (health ran out). See [`CombatState::revive`].
    pub defeated: bool,
    #[serde(skip)]
    pub last_attacker: Option<Entity>,
    pub hits_landed: u32,
    pub hits_taken: u32,
    pub dodges: u32,
    pub blocks: u32,
    /// The last outcome of this fighter's own strikes.
    pub last_outcome: Option<Outcome>,
    buffered: Option<(AttackKind, f32)>,
    prev_attack: bool,
}

impl CombatState {
    /// In the dangerous part of an attack (dashing in, winding up or striking).
    pub fn attacking(&self) -> bool {
        matches!(
            self.attack.map(|a| a.phase),
            Some(AttackPhase::DashIn | AttackPhase::Windup | AttackPhase::Strike)
        )
    }

    /// Open to a counter: recovering from a swing or staggered.
    pub fn vulnerable(&self) -> bool {
        self.stagger_secs > 0.0
            || matches!(self.attack.map(|a| a.phase), Some(AttackPhase::Recovery))
    }

    /// Start an attack on the next step, as if `MoveIntent::attack` had been pressed.
    pub fn request_attack(&mut self, kind: AttackKind) {
        self.buffered = Some((kind, 0.3));
    }

    /// Bring a defeated fighter back with full health.
    pub fn revive(&mut self, fighter: &mut Fighter, mover: &mut Mover) {
        self.defeated = false;
        self.attack = None;
        self.stagger_secs = 0.0;
        self.buffered = None;
        fighter.health = fighter.max_health;
        mover.disabled = false;
    }
}

/// One resolved strike, for tests, AI agents (`movement.combat_log`) and UI.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CombatRecord {
    pub world_secs: f64,
    pub real_secs: f64,
    #[serde(skip)]
    pub attacker: Option<Entity>,
    #[serde(skip)]
    pub target: Option<Entity>,
    pub attacker_id: u64,
    pub target_id: Option<u64>,
    pub kind: AttackKind,
    pub outcome: Outcome,
    pub damage: f32,
}

/// The last 64 strikes.
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
pub struct CombatLog {
    pub records: VecDeque<CombatRecord>,
}

impl CombatLog {
    pub const CAPACITY: usize = 64;

    pub fn push(&mut self, r: CombatRecord) {
        if self.records.len() == Self::CAPACITY {
            self.records.pop_front();
        }
        self.records.push_back(r);
    }

    /// Records where `attacker` struck.
    pub fn by(&self, attacker: Entity) -> impl Iterator<Item = &CombatRecord> {
        self.records
            .iter()
            .filter(move |r| r.attacker == Some(attacker))
    }
}

/// What one fighter looked like at the start of a combat system (so every fighter resolves
/// against the same snapshot).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Snap {
    entity: Entity,
    pos: DVec3,
    team: u32,
    radius: f32,
    defeated: bool,
    evading: bool,
    blocking: bool,
    vulnerable: bool,
    facing: Vec3,
}

/// Own seconds per world second for an entity this step.
fn own_rate(time: &Time, local: Option<&LocalTimeScale>) -> f32 {
    let world = time.delta_secs();
    if world > 0.0 {
        time.delta_for(local) / world
    } else {
        local.map_or(1.0, |l| l.factor)
    }
}

#[allow(clippy::too_many_arguments)]
fn pick_target(
    snaps: &[Snap],
    me: Entity,
    team: u32,
    from: DVec3,
    toward: Option<DVec3>,
    look: Vec3,
    range: f32,
    up: Vec3,
) -> Option<Snap> {
    snaps
        .iter()
        .filter(|s| s.entity != me && s.team != team && !s.defeated)
        .filter_map(|s| {
            let to = flat((s.pos - from).as_vec3(), up);
            let dist = to.length();
            if dist > range + s.radius {
                return None;
            }
            // Score: distance, plus a penalty for being off to the side (or away from the
            // point the dash was aimed at).
            let off = match toward {
                Some(p) => s.pos.distance(p) as f32,
                None => (1.0 - to.normalize_or_zero().dot(look)) * range,
            };
            if toward.is_none() && to.normalize_or_zero().dot(look) < -0.2 {
                return None;
            }
            Some((dist + off * 2.0, *s))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
}

/// Start attacks from `MoveIntent::attack` and tell movement how attacks bend motion.
/// Runs before the locomotion step.
#[allow(clippy::type_complexity)]
pub(crate) fn begin_attacks(
    time: Res<Time>,
    mut snaps: Local<Vec<Snap>>,
    mut attacks: MessageWriter<AttackEvent>,
    mut triggers: MessageWriter<AnimationTrigger>,
    mut q: Query<(
        Entity,
        &Transform,
        &mut Mover,
        &MoveIntent,
        &Fighter,
        &mut CombatState,
        &MovementProfile,
        &CharacterController,
        Option<&LocalTimeScale>,
        Option<&LockOn>,
    )>,
) {
    snaps.clear();
    for (entity, t, m, _, f, cs, _, cc, _, _) in &q {
        snaps.push(Snap {
            entity,
            pos: t.translation,
            team: f.team,
            radius: cc.radius,
            defeated: cs.defeated,
            evading: m.evade_secs > 0.0,
            blocking: cs.blocking,
            vulnerable: cs.vulnerable(),
            facing: m.facing,
        });
    }
    for (entity, t, mut m, intent, f, mut cs, p, _, local, lock) in &mut q {
        let dt = time.delta_for(local);
        if dt <= 0.0 {
            continue;
        }
        let up = m.up;
        cs.stagger_secs = (cs.stagger_secs - dt).max(0.0);
        let pressed = intent.attack.is_some() && !cs.prev_attack;
        cs.prev_attack = intent.attack.is_some();
        if cs.defeated {
            cs.blocking = false;
            cs.buffered = None;
            m.influence = Influence {
                move_scale: 0.0,
                lunge: None,
                face: None,
            };
            continue;
        }
        if let (true, Some(kind)) = (pressed, intent.attack) {
            cs.buffered = Some((kind, 0.3));
        } else if let Some((kind, left)) = cs.buffered {
            cs.buffered = (left > dt).then_some((kind, left - dt));
        }
        cs.blocking =
            intent.block && cs.attack.is_none() && cs.stagger_secs <= 0.0 && m.dash.is_none();

        // (A character's first step sets up its dash charges: attacks wait for it.)
        if cs.attack.is_none()
            && cs.stagger_secs <= 0.0
            && m.mantle.is_none()
            && m.hang.is_none()
            && m.initialized
            && let Some((kind, _)) = cs.buffered.take()
        {
            let look = intent
                .look
                .map(|l| flat(l, up))
                .and_then(|l| l.try_normalize())
                .unwrap_or(m.facing);
            let rate = own_rate(&time, local).max(1e-3);
            // Locked on: that enemy, wherever it stands.
            let locked = lock
                .and_then(|l| l.target)
                .and_then(|e| snaps.iter().find(|s| s.entity == e && !s.defeated))
                .copied();
            let locked_gap =
                locked.map(|s| flat((s.pos - t.translation).as_vec3(), up).length() - s.radius);
            let far_target = || {
                pick_target(
                    &snaps,
                    entity,
                    f.team,
                    t.translation,
                    None,
                    look,
                    f.dash_strike_range,
                    up,
                )
                .filter(|s| {
                    flat((s.pos - t.translation).as_vec3(), up).length() - s.radius > f.reach * 1.5
                })
            };
            let out_of_reach = match locked_gap {
                Some(gap) => gap > f.reach * 1.5 && gap <= f.dash_strike_range,
                None => {
                    pick_target(
                        &snaps,
                        entity,
                        f.team,
                        t.translation,
                        None,
                        look,
                        f.reach * 1.5,
                        up,
                    )
                    .is_none()
                        && far_target().is_some()
                }
            };
            let kind = if kind == AttackKind::Melee
                && f.auto_dash_strike
                && m.dash_charges >= 1.0
                && out_of_reach
            {
                AttackKind::Dash
            } else {
                kind
            };
            let dash = kind == AttackKind::Dash;
            let range = if dash {
                f.dash_strike_range
            } else {
                f.reach * 2.5
            };
            let target = locked
                .filter(|_| locked_gap.is_some_and(|gap| gap <= range))
                .or_else(|| {
                    pick_target(
                        &snaps,
                        entity,
                        f.team,
                        t.translation,
                        if dash { intent.dash_to } else { None },
                        look,
                        range,
                        up,
                    )
                });
            let aim = target.map(|s| s.pos).or(intent.dash_to);
            let dir = aim
                .map(|a| flat((a - t.translation).as_vec3(), up))
                .and_then(|d| d.try_normalize())
                .unwrap_or(look);
            let gap = aim.map_or(0.0, |a| flat((a - t.translation).as_vec3(), up).length())
                - target.map_or(0.0, |s| s.radius);
            let (phase, secs_left, hit_in) = if dash
                && gap > f.reach + 0.3
                && m.dash_charges >= 1.0
                && let Some(to) = aim
            {
                m.pending_dash = Some(DashRequest {
                    to,
                    strike: true,
                    max_distance: Some(f.dash_strike_range),
                });
                let dash_secs =
                    (gap - p.dash_stop_distance).clamp(0.0, f.dash_strike_range) / p.dash_speed();
                (
                    AttackPhase::DashIn,
                    dash_secs,
                    dash_secs + f.dash_windup_secs,
                )
            } else {
                let w = if dash {
                    f.dash_windup_secs
                } else {
                    f.windup_secs
                };
                (AttackPhase::Windup, w, w)
            };
            cs.attack = Some(AttackState {
                kind,
                phase,
                secs_left,
                secs: 0.0,
                target: target.map(|s| s.entity),
                dir,
            });
            attacks.write(AttackEvent {
                attacker: entity,
                target: target.map(|s| s.entity),
                windup_secs: hit_in / rate,
                reach: f.reach,
                kind,
            });
            triggers.write(AnimationTrigger {
                entity,
                name: if dash { "attack_heavy" } else { "attack_light" }.into(),
            });
        }

        // How the attack bends movement this step.
        let target_pos = cs
            .attack
            .and_then(|a| a.target)
            .and_then(|e| snaps.iter().find(|s| s.entity == e))
            .map(|s| (s.pos, s.radius));
        let to_target = target_pos.map(|(p, r)| {
            let d = flat((p - t.translation).as_vec3(), up);
            (d.normalize_or_zero(), d.length() - r)
        });
        m.influence = match cs.attack.map(|a| a.phase) {
            Some(AttackPhase::DashIn) => Influence {
                move_scale: 0.0,
                lunge: None,
                face: to_target.map(|(d, _)| d),
            },
            Some(AttackPhase::Windup) => {
                let lunge = match to_target {
                    Some((d, gap)) if gap > f.reach * 0.7 => d * f.lunge_speed,
                    _ => Vec3::ZERO,
                };
                Influence {
                    move_scale: f.attack_move_scale,
                    lunge: Some(lunge),
                    face: to_target.map(|(d, _)| d).or(cs.attack.map(|a| a.dir)),
                }
            }
            Some(AttackPhase::Strike) => Influence {
                move_scale: 0.0,
                lunge: Some(Vec3::ZERO),
                face: cs.attack.map(|a| a.dir),
            },
            Some(AttackPhase::Recovery) => Influence {
                move_scale: f.attack_move_scale,
                lunge: None,
                face: None,
            },
            None if cs.stagger_secs > 0.0 => Influence {
                move_scale: 0.0,
                lunge: None,
                face: None,
            },
            None if cs.blocking => Influence {
                move_scale: f.block_move_scale,
                lunge: None,
                face: None,
            },
            None => Influence {
                move_scale: 1.0,
                lunge: None,
                face: None,
            },
        };
    }
}

/// Advance attacks and land strikes. Runs after the locomotion step.
#[allow(clippy::type_complexity)]
pub(crate) fn resolve_attacks(
    time: Res<Time>,
    mut snaps: Local<Vec<Snap>>,
    mut damage: MessageWriter<DamageEvent>,
    mut noise: MessageWriter<NoiseEvent>,
    mut log: ResMut<CombatLog>,
    mut q: Query<(
        Entity,
        &Transform,
        &Mover,
        &Fighter,
        &mut CombatState,
        &mut LocomotionState,
        &CharacterController,
        Option<&LocalTimeScale>,
    )>,
) {
    snaps.clear();
    for (entity, t, m, f, cs, _, cc, _) in &q {
        snaps.push(Snap {
            entity,
            pos: t.translation,
            team: f.team,
            radius: cc.radius,
            defeated: cs.defeated,
            evading: m.evade_secs > 0.0,
            blocking: cs.blocking,
            vulnerable: cs.vulnerable(),
            facing: m.facing,
        });
    }
    // Effects on other fighters, applied after the loop: (fighter, what happened to it).
    let mut effects: Vec<(Entity, Outcome)> = Vec::new();
    for (entity, t, m, f, mut cs, mut loco, _, local) in &mut q {
        let dt = time.delta_for(local);
        if let Some(mut a) = cs.attack
            && dt > 0.0
        {
            a.secs += dt;
            match a.phase {
                AttackPhase::DashIn => {
                    if (m.dash.is_none() && m.pending_dash.is_none()) || a.secs > 1.5 {
                        a.phase = AttackPhase::Windup;
                        a.secs_left = f.dash_windup_secs;
                    }
                }
                AttackPhase::Windup => {
                    a.secs_left -= dt;
                    if a.secs_left <= 0.0 {
                        // Re-aim at the target's current position (it may have moved), then swing.
                        if let Some(s) = a.target.and_then(|e| snaps.iter().find(|s| s.entity == e))
                            && let Some(d) =
                                flat((s.pos - t.translation).as_vec3(), m.up).try_normalize()
                        {
                            a.dir = d;
                        }
                        a.phase = AttackPhase::Strike;
                        a.secs_left = f.strike_secs;
                        let outcome = strike(
                            entity,
                            t.translation,
                            m.up,
                            f,
                            &a,
                            &snaps,
                            &time,
                            &mut damage,
                            &mut log,
                            &mut effects,
                        );
                        cs.last_outcome = Some(outcome);
                        match outcome {
                            Outcome::Hit | Outcome::Counter => cs.hits_landed += 1,
                            // A blocked swing knocks the attacker off balance.
                            Outcome::Blocked => cs.stagger_secs = cs.stagger_secs.max(0.3),
                            _ => {}
                        }
                        noise.write(NoiseEvent {
                            position: t.translation,
                            radius: 10.0,
                            source: Some(entity),
                        });
                    }
                }
                AttackPhase::Strike => {
                    a.secs_left -= dt;
                    if a.secs_left <= 0.0 {
                        a.phase = AttackPhase::Recovery;
                        a.secs_left = f.recovery_secs;
                    }
                }
                AttackPhase::Recovery => a.secs_left -= dt,
            }
            cs.attack = (a.phase != AttackPhase::Recovery || a.secs_left > 0.0).then_some(a);
        }
        loco.attacking = cs.attacking();
        loco.vulnerable = cs.vulnerable();
        loco.blocking = cs.blocking;
    }
    for (e, outcome) in effects {
        if outcome == Outcome::Dodged
            && let Ok((_, _, _, _, mut cs, _, _, _)) = q.get_mut(e)
        {
            cs.dodges += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn strike(
    me: Entity,
    from: DVec3,
    up: Vec3,
    f: &Fighter,
    a: &AttackState,
    snaps: &[Snap],
    time: &Time,
    damage: &mut MessageWriter<DamageEvent>,
    log: &mut CombatLog,
    effects: &mut Vec<(Entity, Outcome)>,
) -> Outcome {
    let my_team = snaps
        .iter()
        .find(|s| s.entity == me)
        .map_or(f.team, |s| s.team);
    let half_arc = (f.arc_deg.to_radians() * 0.5).cos();
    let block_cos = (f.block_arc_deg.to_radians() * 0.5).cos();
    let mut best = Outcome::Whiff;
    let rank = |o: Outcome| match o {
        Outcome::Counter => 4,
        Outcome::Hit => 3,
        Outcome::Blocked => 2,
        Outcome::Dodged => 1,
        Outcome::Whiff => 0,
    };
    for s in snaps
        .iter()
        .filter(|s| s.entity != me && s.team != my_team && !s.defeated)
    {
        let to = flat((s.pos - from).as_vec3(), up);
        let dist = to.length();
        let vertical = (s.pos - from).dot(up.as_dvec3()).abs();
        if dist - s.radius > f.reach || vertical > 1.5 {
            continue;
        }
        let dir = to.normalize_or_zero();
        if dist > 0.5 && dir.dot(a.dir) < half_arc {
            continue;
        }
        let outcome = if s.evading {
            Outcome::Dodged
        } else if s.blocking && s.facing.dot(-dir) >= block_cos {
            Outcome::Blocked
        } else if s.vulnerable {
            Outcome::Counter
        } else {
            Outcome::Hit
        };
        let amount = match outcome {
            Outcome::Counter => f.damage * f.counter_multiplier,
            Outcome::Hit => f.damage,
            Outcome::Blocked => f.damage * f.block_damage_scale,
            _ => 0.0,
        };
        if amount > 0.0 {
            let push = if outcome == Outcome::Blocked {
                0.4
            } else {
                1.0
            };
            damage.write(DamageEvent {
                target: s.entity,
                amount,
                kind: DamageKind::Slash,
                point: s.pos + (up * 0.3).as_dvec3(),
                impulse: a.dir * f.knockback * push,
                source: Some(me),
            });
        }
        log.push(CombatRecord {
            world_secs: time.elapsed_secs_f64(),
            real_secs: time.real_elapsed_secs_f64(),
            attacker: Some(me),
            target: Some(s.entity),
            attacker_id: me.to_bits(),
            target_id: Some(s.entity.to_bits()),
            kind: a.kind,
            outcome,
            damage: amount,
        });
        effects.push((s.entity, outcome));
        if rank(outcome) > rank(best) {
            best = outcome;
        }
    }
    if best == Outcome::Whiff {
        log.push(CombatRecord {
            world_secs: time.elapsed_secs_f64(),
            real_secs: time.real_elapsed_secs_f64(),
            attacker: Some(me),
            target: a.target,
            attacker_id: me.to_bits(),
            target_id: a.target.map(Entity::to_bits),
            kind: a.kind,
            outcome: Outcome::Whiff,
            damage: 0.0,
        });
    }
    best
}

/// Apply `DamageEvent`s to fighters: health, knockback, stagger, defeat (ragdoll).
#[allow(clippy::type_complexity)]
pub(crate) fn apply_damage(
    mut commands: Commands,
    mut events: MessageReader<DamageEvent>,
    mut triggers: MessageWriter<AnimationTrigger>,
    mut q: Query<(
        &mut Fighter,
        &mut CombatState,
        &mut Mover,
        &MovementProfile,
        Option<&mut Ragdoll>,
    )>,
) {
    for ev in events.read() {
        let Ok((mut f, mut cs, mut m, p, ragdoll)) = q.get_mut(ev.target) else {
            continue;
        };
        if cs.defeated {
            continue;
        }
        cs.hits_taken += 1;
        cs.last_attacker = ev.source;
        m.knockback += ev.impulse / p.mass_kg.max(1.0);
        if f.track_health {
            f.health -= ev.amount;
        }
        if f.track_health && f.health <= 0.0 {
            f.health = 0.0;
            cs.defeated = true;
            cs.attack = None;
            cs.blocking = false;
            m.disabled = true;
            m.dash = None;
            match ragdoll {
                Some(mut r) => r.weight = 1.0,
                None => {
                    commands.entity(ev.target).insert(Ragdoll { weight: 1.0 });
                }
            }
            triggers.write(AnimationTrigger {
                entity: ev.target,
                name: "death".into(),
            });
            continue;
        }
        if cs.blocking {
            cs.blocks += 1;
            triggers.write(AnimationTrigger {
                entity: ev.target,
                name: "block".into(),
            });
            continue;
        }
        // A clean hit interrupts a swing that hasn't landed yet.
        if matches!(
            cs.attack.map(|a| a.phase),
            Some(AttackPhase::Windup | AttackPhase::DashIn)
        ) {
            cs.attack = None;
            m.dash = None;
        }
        cs.stagger_secs = cs.stagger_secs.max(f.stagger_secs);
        triggers.write(AnimationTrigger {
            entity: ev.target,
            name: "hit_react".into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_is_a_ring() {
        let mut log = CombatLog::default();
        for i in 0..100 {
            log.push(CombatRecord {
                world_secs: i as f64,
                real_secs: i as f64,
                attacker: None,
                target: None,
                attacker_id: 0,
                target_id: None,
                kind: AttackKind::Melee,
                outcome: Outcome::Whiff,
                damage: 0.0,
            });
        }
        assert_eq!(log.records.len(), CombatLog::CAPACITY);
        assert_eq!(log.records.front().unwrap().world_secs, 36.0);
    }

    #[test]
    fn fighter_json_defaults() {
        let f: Fighter = serde_json::from_value(serde_json::json!({"team": 2})).unwrap();
        assert_eq!(f.team, 2);
        assert_eq!(f.reach, Fighter::default().reach);
    }
}
