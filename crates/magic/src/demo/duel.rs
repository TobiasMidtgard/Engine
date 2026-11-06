//! The hall's duel (`spells_duel`): six Adept students on the dais against three Ascended
//! masters at the far end, casting at each other (round-2-plan.md, "A duel").
//!
//! Each duelist thinks in tiers, the way utility AI keeps the choice small (score a few
//! behaviours, then a spell inside the chosen one, then its target): Evade (an enemy bolt
//! will reach it within 0.6 s: blink aside), Ward (low health), Mend (an ally is low: a
//! healing bolt), else Attack. Attack picks the spell that reacts with what the target
//! already carries, as players combo in Genshin and Magicka: frost on a chilled target until
//! it freezes, a heavy storm-and-push bolt to shatter a frozen one, frost into fire for
//! steam, fire into storm for plasma. The downed go limp (`Ragdoll`) and stand again.
//!
//! Between casts they move (round-3-plan.md, "A duel that moves"), picking a spot the way
//! Killzone's AI does (Guerrilla, GDC Europe 2005) and Unreal's Environment Query System
//! (generate points, test them, score them): about twelve points on two rings around the mage,
//! each scored on its range to the target, its own side of the hall, room from allies, the
//! paths of incoming bolts and hostile fields, a pull toward sideways steps, and a small tilt
//! of each mage's own ([`pick_spot`]). They walk there at 1.2-1.6 m/s, facing the target, and
//! turn square to it before they cast.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::animation::Ragdoll;
use engine_world::physics::{PhysicsQueries, QueryFilter};

use super::art::{FLOOR, HallMage, LANES, MAGE_Z, PLINTH_TOP, ProgramProp, mage_look, spawn_mage};
use super::{Hall, Program};
use crate::caster::{Aim, CastRequest, Casting, MagicCaster, SpellChoice, Vitality};
use crate::design::{CastType, Modifier, SpellDesign};
use crate::hits::SpellHit;
use crate::runtime::{Field, FieldKind, Missile, SpellPos};
use crate::schools::{Element, Mastery, Status};
use crate::status::Afflictions;

/// Where the masters stand (x, z from the hall's center): in front of the far dummies.
pub const MASTERS: [(f64, f64); 3] = [(-9.0, -6.5), (0.0, -7.5), (9.0, -6.5)];
/// The students' and masters' sides (`MagicCaster::side`).
pub const STUDENTS: u32 = 1;
pub const MASTERS_SIDE: u32 = 2;

/// A mage in the duel.
#[derive(Component, Debug, Clone)]
pub struct Duelist {
    pub team: u32,
    /// Seconds until the next cast may start.
    pub ready_in: f32,
    /// Seconds between casts (each mage its own rhythm).
    pub rhythm: f32,
    pub target: Option<Entity>,
    /// Seconds left on the current target before choosing again.
    pub keep_target: f32,
    pub blink_ready: f32,
    pub ward_ready: f32,
    pub mend_ready: f32,
    /// Spells cast, for the "every fourth" style rolls (no randomness from entity ids).
    pub count: u32,
    pub seed: u32,
    pub master: bool,
    /// Where it is walking (footwork), and seconds until it looks for a better spot.
    pub goal: Option<DVec3>,
    pub repick_in: f32,
    /// Its walking pace, m/s.
    pub pace: f32,
    /// Whom it turns to face (the target, or an ally it is about to mend).
    pub face: Option<Entity>,
    /// Spots picked, for the tilt's rolls.
    pub picks: u32,
}

impl Duelist {
    pub fn new(team: u32, seed: u32, master: bool) -> Self {
        let r = unit(seed, 0);
        Self {
            team,
            ready_in: 0.3 + r * 1.2,
            rhythm: if master { 2.1 } else { 1.5 } + r * 0.9,
            target: None,
            keep_target: 0.0,
            blink_ready: 0.0,
            ward_ready: 0.0,
            mend_ready: 0.0,
            count: 0,
            seed,
            master,
            goal: None,
            repick_in: 0.4 + unit(seed, 9) * 1.2,
            pace: 1.2 + 0.4 * unit(seed, 10),
            face: None,
            picks: 0,
        }
    }

    fn roll(&self, salt: u32) -> f32 {
        unit(self.seed, self.count.wrapping_mul(31).wrapping_add(salt))
    }
}

/// A repeatable 0..1 from a seed and a counter.
fn unit(seed: u32, n: u32) -> f32 {
    let mut x = seed
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(n.wrapping_mul(0x85EB_CA6B))
        ^ 0x27D4_EB2F;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

/// What the duelist does this time.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    Evade(Vec3),
    Ward,
    Mend(Entity),
    Attack(Entity, SpellDesign),
}

/// What the attacker sees of a target.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TargetState {
    pub frozen: bool,
    pub chill: f32,
    pub frost_aura: bool,
    pub fire: bool,
    pub storm: bool,
}

impl TargetState {
    pub fn of(a: Option<&Afflictions>) -> Self {
        let Some(a) = a else {
            return Self::default();
        };
        Self {
            frozen: a.has(Status::Frozen),
            chill: a.chill,
            frost_aura: a.has_aura(Element::Frost),
            fire: a.has(Status::Burning) || a.has_aura(Element::Fire),
            storm: a.has(Status::Shocked) || a.has_aura(Element::Storm),
        }
    }
}

fn d(fx: &[&str], cast: CastType) -> SpellDesign {
    SpellDesign::new(fx, cast)
}

/// The spell to throw at a target in `state`: the one that reacts with what it carries.
/// `roll` (0..1) breaks ties so a duel doesn't repeat itself.
pub fn pick_attack(state: TargetState, master: bool, roll: f32) -> SpellDesign {
    let frost = || {
        if master {
            d(&["frost"], CastType::Bolt)
                .with(Modifier::Split { count: 3 })
                .with(Modifier::Seek)
        } else {
            d(&["frost"], CastType::Bolt)
        }
    };
    let fire = || {
        if master {
            d(&["fire"], CastType::Bolt).with(Modifier::Blast { radius_m: 2.5 })
        } else {
            d(&["fire"], CastType::Bolt)
        }
    };
    let storm = || {
        if master {
            d(&["storm"], CastType::Chain)
        } else {
            d(&["storm"], CastType::Bolt)
        }
    };
    if state.frozen {
        // A heavy blow shatters ice: force in the bolt makes it heavy.
        return d(&["push", "storm"], CastType::Bolt);
    }
    if state.chill >= 2.0 {
        // Two more and it freezes.
        return frost();
    }
    if state.frost_aura {
        // Fire on frost: steam.
        return if roll < 0.7 { fire() } else { frost() };
    }
    if state.fire {
        // Frost on fire is steam too (the weaker order); storm on fire is plasma.
        return if roll < 0.5 { storm() } else { frost() };
    }
    if state.storm {
        return fire();
    }
    if roll < 0.45 {
        frost()
    } else if roll < 0.8 {
        fire()
    } else {
        storm()
    }
}

/// One duelist's choice, from what it can see.
pub struct View<'a> {
    pub me: Entity,
    pub pos: DVec3,
    pub health: f32,
    pub foes: &'a [(Entity, DVec3, f32, TargetState)],
    pub allies: &'a [(Entity, DVec3, f32)],
    /// The soonest enemy bolt heading at me: seconds to arrive and its direction.
    pub incoming: Option<(f32, Vec3)>,
}

pub fn decide(d: &mut Duelist, v: &View) -> Option<Choice> {
    // Evade: a bolt arrives within 0.6 s and the blink is ready.
    if let Some((t, dir)) = v.incoming
        && t < 0.6
        && d.blink_ready <= 0.0
    {
        let side = Vec3::new(-dir.z, 0.0, dir.x).normalize_or(Vec3::X);
        let side = if d.roll(1) < 0.5 { side } else { -side };
        return Some(Choice::Evade(side));
    }
    if v.health < 0.45 && d.ward_ready <= 0.0 {
        return Some(Choice::Ward);
    }
    if d.mend_ready <= 0.0
        && let Some((ally, _, _)) = v
            .allies
            .iter()
            .filter(|(e, _, h)| *e != v.me && *h < 0.4)
            .min_by(|a, b| a.2.total_cmp(&b.2))
    {
        return Some(Choice::Mend(*ally));
    }
    // Keep a target a while; else the weakest foe, nearer ones a little preferred.
    let alive = |e: Entity| v.foes.iter().find(|f| f.0 == e);
    if d.keep_target <= 0.0 || d.target.and_then(alive).is_none() {
        d.target = v
            .foes
            .iter()
            .min_by(|a, b| {
                let score = |f: &(Entity, DVec3, f32, TargetState)| {
                    f.2 + (f.1.distance(v.pos) as f32) * 0.01
                };
                score(a).total_cmp(&score(b))
            })
            .map(|f| f.0);
        d.keep_target = 4.0 + d.roll(2) * 3.0;
    }
    let target = d.target.and_then(alive)?;
    Some(Choice::Attack(
        target.0,
        pick_attack(target.3, d.master, d.roll(3)),
    ))
}

// ------------------------------------------------------------------------------ footwork

/// Something to keep out of: an enemy bolt in flight, a hostile field, a hostile wall.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Danger {
    Bolt { at: DVec3, vel: Vec3 },
    Area { at: DVec3, radius: f32 },
    Wall { at: DVec3, along: Vec3, half: f32 },
}

impl Danger {
    /// How deep a point stands in it, m (0: clear).
    fn depth(&self, p: DVec3) -> f32 {
        let flat = |v: DVec3| DVec3::new(v.x, 0.0, v.z);
        match *self {
            Danger::Bolt { at, vel } => {
                // Its path over the next 1.5 s, a body's width either side.
                let v = flat(vel.as_dvec3());
                let speed2 = v.length_squared();
                if speed2 < 1.0 {
                    return 0.0;
                }
                let r = flat(p - at);
                let t = (r.dot(v) / speed2).clamp(0.0, 1.5);
                (1.6 - (r - v * t).length() as f32).max(0.0)
            }
            Danger::Area { at, radius } => (radius + 0.8 - flat(p - at).length() as f32).max(0.0),
            Danger::Wall { at, along, half } => {
                let r = flat(p - at);
                let a = flat(along.as_dvec3()).normalize_or(DVec3::X);
                let x = r.dot(a).clamp(-half as f64, half as f64);
                (1.4 - (r - a * x).length() as f32).max(0.0)
            }
        }
    }
}

/// What a duelist knows when it picks a spot. Positions are the hall's (from its center).
pub struct Footing<'a> {
    pub pos: DVec3,
    pub team: u32,
    pub master: bool,
    pub target: Option<DVec3>,
    /// The others on its side.
    pub allies: &'a [DVec3],
    pub dangers: &'a [Danger],
    pub seed: u32,
    /// Spots picked so far (the tilt changes each time).
    pub picks: u32,
}

/// Preferred range to the target, m: students close in, masters keep their distance.
pub fn preferred_range(master: bool) -> (f32, f32) {
    if master { (12.0, 18.0) } else { (9.0, 14.0) }
}

/// The hall a duelist may stand in (x, z from the hall's center) and its side's line: students
/// keep to z > 2, masters to z < -2.
fn out_of_bounds(team: u32, p: DVec3) -> f32 {
    let over = |v: f64, lo: f64, hi: f64| ((lo - v).max(0.0) + (v - hi).max(0.0)) as f32;
    let side = if team == STUDENTS {
        over(p.z, 2.0, 12.0)
    } else {
        over(p.z, -14.0, -2.0)
    };
    side + over(p.x, -19.0, 19.0)
}

/// How bad a spot is (lower is better), and the parts of it, for tests and tuning.
pub fn spot_cost(f: &Footing, p: DVec3, tilt: f32) -> f32 {
    let mut cost = 0.0;
    let moved = DVec3::new(p.x - f.pos.x, 0.0, p.z - f.pos.z);
    let dist = moved.length() as f32;
    if let Some(t) = f.target {
        let (lo, hi) = preferred_range(f.master);
        let r = DVec3::new(t.x - p.x, 0.0, t.z - p.z).length() as f32;
        cost += 0.6 * ((lo - r).max(0.0) + (r - hi).max(0.0));
        // Sideways steps (strafing) beat stepping straight in or backpedalling.
        if dist > 0.1 {
            let u = moved / dist as f64;
            let to = DVec3::new(t.x - f.pos.x, 0.0, t.z - f.pos.z).normalize_or(DVec3::NEG_Z);
            let along = u.dot(to) as f32;
            cost -= 0.35 * (1.0 - along.abs());
            cost += 0.25 * (-along).max(0.0);
        }
    }
    cost += 4.0 * out_of_bounds(f.team, p);
    for a in f.allies {
        let d = DVec3::new(a.x - p.x, 0.0, a.z - p.z).length() as f32;
        cost += 1.2 * (3.0 - d).max(0.0);
    }
    for d in f.dangers {
        cost += 2.5 * d.depth(p);
    }
    cost + 0.08 * dist + 0.25 * tilt
}

/// The best of about twelve spots on two rings around the duelist (and where it stands).
pub fn pick_spot(f: &Footing) -> DVec3 {
    let turn = unit(f.seed, f.picks.wrapping_mul(7)) * std::f32::consts::TAU;
    let mut best = (spot_cost(f, f.pos, unit(f.seed, f.picks * 13 + 99)), f.pos);
    for i in 0..12u32 {
        let ring = if i < 6 { 2.0 } else { 3.8 };
        // The outer ring sits between the inner one's spokes.
        let a =
            turn + std::f32::consts::TAU * (i % 6) as f32 / 6.0 + if i < 6 { 0.0 } else { 0.52 };
        let p = f.pos + DVec3::new((a.cos() * ring) as f64, 0.0, (a.sin() * ring) as f64);
        let c = spot_cost(f, p, unit(f.seed, f.picks * 13 + i));
        if c < best.0 {
            best = (c, p);
        }
    }
    best.1
}

/// The floor under a duelist: the plinths have no collider, so the dais's top is lifted onto
/// them by hand.
fn floor_under(queries: Option<&PhysicsQueries>, origin: DVec3, p: DVec3, me: Entity) -> f64 {
    let ray = queries
        .and_then(|q| {
            q.raycast(
                p + DVec3::Y * 1.5,
                Vec3::NEG_Y,
                4.0,
                &QueryFilter::excluding(me),
            )
        })
        .map(|h| h.point.y)
        .unwrap_or(p.y);
    let on_plinth = LANES.iter().any(|x| {
        let c = origin + DVec3::new(*x, 0.0, MAGE_Z);
        DVec3::new(p.x - c.x, 0.0, p.z - c.z).length() < 0.85
    });
    if on_plinth {
        ray.max(origin.y + PLINTH_TOP)
    } else {
        ray
    }
}

/// Footwork: between casts each duelist picks a spot, walks there facing its target, and
/// turns to it.
#[allow(clippy::type_complexity)]
pub fn duel_footwork(
    time: Res<Time>,
    hall: Option<Res<Hall>>,
    queries: Option<Res<PhysicsQueries>>,
    mut duelists: Query<(
        Entity,
        &mut Duelist,
        &mut Transform,
        &MagicCaster,
        &Vitality,
        Has<Casting>,
    )>,
    missiles: Query<(&Missile, &SpellPos)>,
    fields: Query<(&Field, &SpellPos)>,
) {
    let Some(hall) = hall else { return };
    if hall.program != Program::Duel || !hall.near || hall.restart {
        return;
    }
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let origin = hall.origin;
    let seen: Vec<(Entity, u32, DVec3, bool)> = duelists
        .iter()
        .map(|(e, d, t, _, v, _)| (e, d.team, t.translation, v.down()))
        .collect();
    let at = |e: Entity| seen.iter().find(|s| s.0 == e && !s.3).map(|s| s.2);
    for (e, mut d, mut tf, c, v, casting) in &mut duelists {
        if v.down() {
            d.goal = None;
            continue;
        }
        let pos = tf.translation;
        let target = d.target.and_then(at).or_else(|| {
            seen.iter()
                .filter(|s| s.1 != d.team && !s.3)
                .min_by(|a, b| a.2.distance(pos).total_cmp(&b.2.distance(pos)))
                .map(|s| s.2)
        });
        // Pick a spot every couple of seconds (or once there).
        d.repick_in -= dt;
        if d.repick_in <= 0.0 && !casting {
            let allies: Vec<DVec3> = seen
                .iter()
                .filter(|s| s.1 == d.team && s.0 != e && !s.3)
                .map(|s| s.2 - origin)
                .collect();
            let mut dangers: Vec<Danger> = missiles
                .iter()
                .filter(|(m, _)| m.cast.side != c.side && m.spent.is_none())
                .map(|(m, p)| Danger::Bolt {
                    at: p.pos - origin,
                    vel: m.vel,
                })
                .collect();
            dangers.extend(
                fields
                    .iter()
                    .filter(|(f, _)| {
                        f.cast.side != c.side && f.kind != FieldKind::Burst && f.spent.is_none()
                    })
                    .map(|(f, p)| match f.wall {
                        Some((along, half)) => Danger::Wall {
                            at: p.pos - origin,
                            along,
                            half,
                        },
                        None => Danger::Area {
                            at: p.pos - origin,
                            radius: f.radius,
                        },
                    }),
            );
            let footing = Footing {
                pos: pos - origin,
                team: d.team,
                master: d.master,
                target: target.map(|t| t - origin),
                allies: &allies,
                dangers: &dangers,
                seed: d.seed,
                picks: d.picks,
            };
            let spot = pick_spot(&footing) + origin;
            d.picks += 1;
            d.goal = (spot.distance(pos) > 0.3).then_some(spot);
            d.repick_in = 1.6 + unit(d.seed, d.picks * 3 + 1);
        }
        // Walk while not casting.
        if !casting && let Some(goal) = d.goal {
            let to = DVec3::new(goal.x - pos.x, 0.0, goal.z - pos.z);
            let left = to.length();
            let step = (d.pace * dt) as f64;
            let mut next = if left <= step {
                d.goal = None;
                DVec3::new(goal.x, pos.y, goal.z)
            } else {
                pos + to / left * step
            };
            // Down steps and off plinths at a walking pace, never through the floor.
            let floor = floor_under(queries.as_deref(), origin, next, e);
            let dy = (floor - next.y).clamp(-2.5 * dt as f64, 2.5 * dt as f64);
            next.y = if (floor - next.y).abs() > 0.6 {
                floor
            } else {
                next.y + dy
            };
            tf.translation = next;
        }
        // Face the one it means to cast at (or its target), turning at about 300°/s.
        let face = d.face.and_then(at).or(target);
        if let Some(t) = face {
            let dir = Vec3::new(
                (t.x - tf.translation.x) as f32,
                0.0,
                (t.z - tf.translation.z) as f32,
            );
            if dir.length_squared() > 0.01 {
                let want = Quat::from_rotation_y((-dir.x).atan2(-dir.z));
                let angle = tf.rotation.angle_between(want);
                let k = (5.2 * dt / angle.max(1e-4)).min(1.0);
                tf.rotation = tf.rotation.slerp(want, k);
            }
        }
    }
}

/// How far a duelist at `rotation` would have to turn to face `dir`, rad.
pub fn facing_error(rotation: Quat, dir: Vec3) -> f32 {
    let fwd = rotation * Vec3::NEG_Z;
    let (a, b) = (
        Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::NEG_Z),
        Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z),
    );
    a.dot(b).clamp(-1.0, 1.0).acos()
}

/// Lay the duel out: the students (the hall's mages) go Adept, three masters walk in.
pub fn setup_duel(commands: &mut Commands, origin: DVec3) {
    for (i, (x, z)) in MASTERS.iter().enumerate() {
        let at = origin + DVec3::new(*x, FLOOR, *z);
        let e = spawn_mage(commands, at, 100 + i, Mastery::Ascended1);
        commands.entity(e).insert((
            Transform::from_translation(at)
                .with_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
            MagicCaster {
                learns: false,
                side: MASTERS_SIDE,
                seed: 2000 + i as u64,
                max_mana: 1.0e6,
                mana: 1.0e6,
                mana_regen: 1.0e5,
                ..MagicCaster::new(88.0, Mastery::Ascended1)
            },
            mage_look(100 + i, Mastery::Ascended1, Some([0.85, 0.1, 0.06])),
            ProgramProp,
            Duelist::new(MASTERS_SIDE, 7700 + i as u32 * 17, true),
            Vitality {
                revive_secs: 7.0,
                regen: 6.0,
                ..Vitality::new(900.0)
            },
        ));
    }
}

/// The students' mage setup for the duel.
pub fn student(c: &mut MagicCaster, lane: usize) -> (Duelist, Vitality) {
    c.side = STUDENTS;
    (
        Duelist::new(STUDENTS, 5100 + lane as u32 * 23, false),
        Vitality {
            revive_secs: 6.0,
            regen: 3.0,
            ..Vitality::new(300.0)
        },
    )
}

/// Think and cast, while the duel runs and a camera is near.
#[allow(clippy::type_complexity)]
pub fn run_duel(
    time: Res<Time>,
    hall: Option<Res<Hall>>,
    mut duelists: Query<(
        Entity,
        &mut Duelist,
        &MagicCaster,
        &Vitality,
        &GlobalTransform,
        Option<&Afflictions>,
        Has<Casting>,
    )>,
    missiles: Query<(&Missile, &SpellPos)>,
    mut casts: MessageWriter<CastRequest>,
) {
    let Some(hall) = hall else { return };
    if hall.program != Program::Duel || !hall.near || hall.restart {
        return;
    }
    let dt = time.delta_secs();
    // What everyone can see this step.
    let seen: Vec<(Entity, u32, DVec3, f32, bool, TargetState)> = duelists
        .iter()
        .map(|(e, d, _, v, g, a, _)| {
            (
                e,
                d.team,
                g.translation(),
                v.health / v.max.max(1.0),
                v.down(),
                TargetState::of(a),
            )
        })
        .collect();
    for (e, mut d, c, v, g, _, casting) in &mut duelists {
        d.ready_in -= dt;
        d.keep_target -= dt;
        d.blink_ready -= dt;
        d.ward_ready -= dt;
        d.mend_ready -= dt;
        if v.down() || casting || d.ready_in > 0.0 {
            continue;
        }
        let pos = g.translation();
        let foes: Vec<(Entity, DVec3, f32, TargetState)> = seen
            .iter()
            .filter(|s| s.1 != d.team && !s.4)
            .map(|s| (s.0, s.2, s.3, s.5))
            .collect();
        if foes.is_empty() {
            continue;
        }
        let allies: Vec<(Entity, DVec3, f32)> = seen
            .iter()
            .filter(|s| s.1 == d.team && !s.4)
            .map(|s| (s.0, s.2, s.3))
            .collect();
        let chest = pos + DVec3::Y * 1.2;
        let incoming = missiles
            .iter()
            .filter(|(m, _)| m.cast.side != c.side && m.spent.is_none())
            .filter_map(|(m, p)| {
                let v = m.vel.as_dvec3();
                let speed2 = v.length_squared();
                if speed2 < 1.0 {
                    return None;
                }
                let r = chest - p.pos;
                let t = r.dot(v) / speed2;
                let miss = (r - v * t).length();
                (t > 0.0 && miss < 1.3).then_some((t as f32, m.vel.normalize_or(Vec3::NEG_Z)))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let view = View {
            me: e,
            pos,
            health: v.health / v.max.max(1.0),
            foes: &foes,
            allies: &allies,
            incoming,
        };
        let Some(choice) = decide(&mut d, &view) else {
            continue;
        };
        // Square up before a cast at someone: turn first (footwork turns it), cast after.
        let at_whom = match &choice {
            Choice::Mend(a) => Some(*a),
            Choice::Attack(t, _) => Some(*t),
            _ => None,
        };
        d.face = at_whom;
        if let Some(whom) = at_whom
            && let Some(p) = seen.iter().find(|s| s.0 == whom).map(|s| s.2)
            && facing_error(g.rotation(), (p - pos).as_vec3()) > 0.45
        {
            continue;
        }
        let (spell, aim) = match choice {
            Choice::Evade(side) => {
                d.blink_ready = 5.0 + d.roll(4) * 2.0;
                (d_blink(), Aim::Direction(side))
            }
            Choice::Ward => {
                d.ward_ready = 9.0;
                (
                    SpellDesign::new(&["ward"], CastType::OnSelf),
                    Aim::Direction(Vec3::Y),
                )
            }
            Choice::Mend(ally) => {
                d.mend_ready = 6.0;
                (
                    SpellDesign::new(&["heal"], CastType::Bolt),
                    Aim::Entity(ally),
                )
            }
            Choice::Attack(t, design) => (design, Aim::Entity(t)),
        };
        d.count += 1;
        d.ready_in = d.rhythm * (0.85 + 0.3 * d.roll(5));
        casts.write(CastRequest {
            caster: e,
            spell: SpellChoice::Design(spell),
            aim,
            charge: 0.0,
        });
    }
}

fn d_blink() -> SpellDesign {
    SpellDesign::new(&["blink"], CastType::OnSelf)
}

/// The downed go limp and stand again when they revive.
pub fn limp_when_down(
    mut commands: Commands,
    q: Query<(Entity, &Vitality, Option<&Ragdoll>), With<Duelist>>,
) {
    for (e, v, r) in &q {
        let want = if v.down() { 1.0 } else { 0.0 };
        if r.map(|r| r.weight) != Some(want) && !(r.is_none() && want == 0.0) {
            commands.entity(e).insert(Ragdoll { weight: want });
        }
    }
}

/// The duel's readout: who still stands, and the last few things worth naming.
pub fn duel_lines(
    hall: &mut Hall,
    standing: (usize, usize, usize, usize),
    hits: &mut MessageReader<SpellHit>,
) {
    for h in hits.read() {
        let line = if h.shattered {
            Some("Shattered!".to_string())
        } else if h.killed {
            Some("Down!".to_string())
        } else {
            h.reaction.map(|r| {
                let name = crate::combine::recipe(r).map(|r| r.name).unwrap_or(r);
                format!("{name}!")
            })
        };
        if let Some(line) = line {
            hall.events.push(line);
        }
    }
    let n = hall.events.len();
    if n > 4 {
        hall.events.drain(..n - 4);
    }
    let (s, sn, m, mn) = standing;
    hall.labels = std::iter::once(format!("Students standing {s}/{sn} · Masters {m}/{mn}"))
        .chain(hall.events.iter().cloned())
        .collect();
}

/// The students' places on the dais, for putting blinkers back.
pub fn student_home(origin: DVec3, m: &HallMage) -> Option<DVec3> {
    LANES
        .get(m.lane)
        .map(|x| origin + DVec3::new(*x, PLINTH_TOP, MAGE_Z))
}

#[cfg(test)]
mod tests {
    use super::*;

    trait Has {
        fn has_effect(&self, key: &str) -> bool;
    }

    impl Has for SpellDesign {
        fn has_effect(&self, key: &str) -> bool {
            self.effects.iter().any(|e| e == key)
        }
    }

    #[test]
    fn attacks_chase_reactions() {
        let frozen = TargetState {
            frozen: true,
            ..Default::default()
        };
        assert!(pick_attack(frozen, false, 0.1).has_effect("push"));
        let chilled = TargetState {
            chill: 3.0,
            frost_aura: true,
            ..Default::default()
        };
        assert!(pick_attack(chilled, false, 0.1).has_effect("frost"));
        let wet = TargetState {
            chill: 1.0,
            frost_aura: true,
            ..Default::default()
        };
        assert!(pick_attack(wet, false, 0.1).has_effect("fire"));
        let burning = TargetState {
            fire: true,
            ..Default::default()
        };
        let p = pick_attack(burning, true, 0.2);
        assert!(p.has_effect("storm") && p.cast == CastType::Chain);
    }

    #[test]
    fn duelists_blink_ward_mend_then_attack() {
        let mut w = World::new();
        let me = w.spawn_empty().id();
        let foe = w.spawn_empty().id();
        let friend = w.spawn_empty().id();
        let foes = [(
            foe,
            DVec3::new(0.0, 0.0, -15.0),
            0.9,
            TargetState::default(),
        )];
        let allies = [(me, DVec3::ZERO, 1.0), (friend, DVec3::X * 6.0, 0.3)];
        let mut d = Duelist::new(STUDENTS, 9, false);
        let view = |health, incoming| View {
            me,
            pos: DVec3::ZERO,
            health,
            foes: &foes,
            allies: &allies,
            incoming,
        };
        assert!(matches!(
            decide(&mut d, &view(1.0, Some((0.3, Vec3::Z)))),
            Some(Choice::Evade(_))
        ));
        assert_eq!(decide(&mut d, &view(0.3, None)), Some(Choice::Ward));
        d.ward_ready = 5.0;
        assert_eq!(decide(&mut d, &view(0.3, None)), Some(Choice::Mend(friend)));
        d.mend_ready = 5.0;
        assert!(matches!(
            decide(&mut d, &view(0.3, None)),
            Some(Choice::Attack(t, _)) if t == foe
        ));
    }

    fn footing<'a>(
        pos: DVec3,
        target: DVec3,
        allies: &'a [DVec3],
        dangers: &'a [Danger],
    ) -> Footing<'a> {
        Footing {
            pos,
            team: STUDENTS,
            master: false,
            target: Some(target),
            allies,
            dangers,
            seed: 5123,
            picks: 0,
        }
    }

    #[test]
    fn a_student_closes_to_its_range_without_crossing_the_line() {
        // On the dais, 17.5 m from a master: it steps in, never past z = 2.
        let master = DVec3::new(0.0, 0.0, -7.5);
        let mut pos = DVec3::new(-3.0, 0.0, 10.0);
        for picks in 0..12 {
            let f = Footing {
                picks,
                ..footing(pos, master, &[], &[])
            };
            pos = pick_spot(&f);
            assert!(pos.z >= 2.0 - 1e-6, "{pos}");
        }
        let r = DVec3::new(pos.x - master.x, 0.0, pos.z - master.z).length();
        assert!((9.0..=14.5).contains(&r), "ended {r:.1} m away at {pos}");
    }

    #[test]
    fn spots_keep_off_bolt_paths_and_away_from_allies() {
        let pos = DVec3::new(0.0, 0.0, 6.0);
        let target = DVec3::new(0.0, 0.0, -6.0);
        // A bolt coming straight down the line at it: whatever it picks is off that line.
        let bolt = [Danger::Bolt {
            at: DVec3::new(0.0, 1.2, -4.0),
            vel: Vec3::new(0.0, 0.0, 26.0),
        }];
        let p = pick_spot(&footing(pos, target, &[], &bolt));
        assert!(p.x.abs() > 1.5, "{p}");
        // An ally right beside it: it moves to give room.
        let ally = [DVec3::new(0.8, 0.0, 6.0)];
        let p = pick_spot(&footing(pos, target, &ally, &[]));
        assert!(p.distance(ally[0]) >= 2.5, "{p}");
    }

    #[test]
    fn at_its_range_it_strafes_rather_than_backpedals() {
        let target = DVec3::new(0.0, 0.0, -6.0);
        let f = footing(DVec3::new(0.0, 0.0, 5.5), target, &[], &[]);
        let p = pick_spot(&f);
        let moved = p - f.pos;
        assert!(moved.length() > 0.5, "it moves");
        assert!(moved.x.abs() > moved.z.abs(), "sideways: {moved}");
    }

    #[test]
    fn facing_error_is_the_turn_left() {
        assert!(facing_error(Quat::IDENTITY, Vec3::NEG_Z) < 1e-3);
        let e = facing_error(Quat::IDENTITY, Vec3::X);
        assert!((e - std::f32::consts::FRAC_PI_2).abs() < 1e-3);
        let turned = Quat::from_rotation_y((-1.0f32).atan2(0.0));
        assert!(facing_error(turned, Vec3::X) < 1e-3);
    }

    #[test]
    fn every_duel_spell_fits_its_caster() {
        let student = MagicCaster::new(34.0, Mastery::Adept);
        let master = MagicCaster::new(88.0, Mastery::Ascended1);
        for (c, is_master) in [(&student, false), (&master, true)] {
            for frozen in [false, true] {
                for chill in [0.0, 3.0] {
                    for fire in [false, true] {
                        for roll in [0.1, 0.5, 0.9] {
                            let s = TargetState {
                                frozen,
                                chill,
                                fire,
                                frost_aura: chill > 0.0,
                                storm: false,
                            };
                            let design = pick_attack(s, is_master, roll);
                            assert!(c.compile(&design, 0.0).is_ok(), "{design:?}");
                        }
                    }
                }
            }
            for extra in [
                d_blink(),
                SpellDesign::new(&["ward"], CastType::OnSelf),
                SpellDesign::new(&["heal"], CastType::Bolt),
            ] {
                assert!(c.compile(&extra, 0.0).is_ok(), "{extra:?}");
            }
        }
    }
}
