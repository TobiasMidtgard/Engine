//! Senses and memory: sight cones with occlusion, hearing (`NoiseEvent`), bullets flying past
//! (`ProjectileTrace`) and being hurt (`DamageEvent`). Detection builds up over time (faster up
//! close, in the center of view, in daylight, when the target moves), and what an NPC acts on
//! lags reality by its reaction time.

use std::collections::{HashMap, VecDeque};

use engine_core::prelude::*;
use engine_world::locomotion::{LocomotionMode, LocomotionState};
use engine_world::physics::{CharacterController, PhysicsBackend, PhysicsQueries};
use engine_world::{DamageEvent, FocusPoints, Ground, NoiseEvent, ProjectileTrace, WeatherState};

use crate::agent::{NpcClock, NpcCombat, NpcMotor, NpcSettings, NpcVitals};
use crate::faction::{Faction, Factions};
use crate::skills::{NpcSkills, Perception};
use crate::world_model::{Sight, TacticalWorld, physics_backend};

/// What an NPC saw of something at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snapshot {
    /// NPC clock time of the observation.
    pub time: f64,
    /// Feet position.
    pub position: DVec3,
    pub velocity: Vec3,
    /// Facing (XZ unit vector).
    pub forward: Vec3,
    pub mode: LocomotionMode,
    pub blocking: bool,
    pub attacking: bool,
    pub vulnerable: bool,
}

/// Something an NPC knows about.
#[derive(Debug, Clone, PartialEq)]
pub struct Known {
    pub entity: Entity,
    pub faction: Faction,
    /// 0 = unaware, 1 = certain.
    pub awareness: f32,
    /// In sight right now.
    pub visible: bool,
    /// NPC clock time it was last seen (or heard, or reported).
    pub last_seen: f64,
    /// Last known feet position.
    pub last_position: DVec3,
    pub last_velocity: Vec3,
    /// Known only from a squad mate's callout.
    pub reported: bool,
    /// Recent observations (newest last), used to act on a reaction-time-old view.
    pub history: VecDeque<Snapshot>,
}

impl Known {
    /// The latest observation at least `reaction` seconds old (what the NPC has had time to
    /// process), or the oldest one it has.
    pub fn perceived(&self, now: f64, reaction: f32) -> Option<&Snapshot> {
        let cutoff = now - reaction as f64;
        self.history
            .iter()
            .rev()
            .find(|s| s.time <= cutoff)
            .or_else(|| self.history.front())
    }

    /// Where it probably is now, extrapolating its last known motion `ahead` seconds (capped).
    pub fn predicted(&self, now: f64, skill: f32) -> DVec3 {
        let since = ((now - self.last_seen) as f32).clamp(0.0, 2.0) * skill;
        self.last_position + (self.last_velocity * since).as_dvec3()
    }
}

/// A sound or disturbance worth looking into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stimulus {
    pub position: DVec3,
    /// NPC clock time.
    pub time: f64,
    /// 0..1.
    pub strength: f32,
}

/// What an NPC knows: entities it has seen, heard or been told about, and the latest
/// disturbance.
#[derive(Component, Debug, Clone, Default)]
pub struct NpcMemory {
    pub known: Vec<Known>,
    pub stimulus: Option<Stimulus>,
    /// Next sight check, NPC clock.
    pub next_look: f64,
}

impl NpcMemory {
    pub fn get(&self, e: Entity) -> Option<&Known> {
        self.known.iter().find(|k| k.entity == e)
    }

    pub fn get_mut(&mut self, e: Entity) -> Option<&mut Known> {
        self.known.iter_mut().find(|k| k.entity == e)
    }

    fn entry(&mut self, e: Entity, faction: Faction, now: f64, at: DVec3) -> &mut Known {
        if let Some(i) = self.known.iter().position(|k| k.entity == e) {
            return &mut self.known[i];
        }
        self.known.push(Known {
            entity: e,
            faction,
            awareness: 0.0,
            visible: false,
            last_seen: now,
            last_position: at,
            last_velocity: Vec3::ZERO,
            reported: false,
            history: VecDeque::new(),
        });
        self.known.last_mut().unwrap()
    }

    /// Tell this NPC about `e` (squad callouts, scripted alerts).
    pub fn report(
        &mut self,
        e: Entity,
        faction: Faction,
        at: DVec3,
        velocity: Vec3,
        now: f64,
        certainty: f32,
    ) {
        let k = self.entry(e, faction, now, at);
        if !k.visible {
            k.last_position = at;
            k.last_velocity = velocity;
            k.last_seen = now;
            k.reported = true;
        }
        k.awareness = k.awareness.max(certainty.clamp(0.0, 1.0));
    }

    /// Record a disturbance if it is stronger or newer than the current one.
    pub fn disturb(&mut self, position: DVec3, now: f64, strength: f32) {
        let replace = self
            .stimulus
            .is_none_or(|s| strength >= s.strength * 0.7 || now - s.time > 4.0);
        if replace {
            self.stimulus = Some(Stimulus {
                position,
                time: now,
                strength,
            });
        }
    }

    /// The most pressing enemy: certain, visible and close beats remembered and far.
    pub fn best_threat(&self, from: DVec3, min_awareness: f32) -> Option<&Known> {
        self.known
            .iter()
            .filter(|k| k.awareness >= min_awareness)
            .max_by(|a, b| {
                let score = |k: &Known| {
                    k.awareness * 2.0 + if k.visible { 1.5 } else { 0.0 }
                        - (k.last_position.distance(from) as f32) * 0.04
                };
                score(a).total_cmp(&score(b))
            })
    }
}

/// One entity others can perceive.
#[derive(Debug, Clone, Copy)]
pub struct Perceivable {
    pub entity: Entity,
    pub faction: Faction,
    pub position: DVec3,
    pub forward: Vec3,
    pub velocity: Vec3,
    /// Body height, meters.
    pub height: f32,
    pub mode: LocomotionMode,
    pub blocking: bool,
    pub attacking: bool,
    pub vulnerable: bool,
}

/// Everything with a [`Faction`] this tick, with a spatial hash (rebuilt every tick).
#[derive(Resource, Debug, Default)]
pub struct Perceivables {
    pub list: Vec<Perceivable>,
    pub index: HashMap<Entity, u32>,
    hash: HashMap<(i64, i64), Vec<u32>>,
}

const PCELL: f64 = 16.0;

impl Perceivables {
    pub fn get(&self, e: Entity) -> Option<&Perceivable> {
        self.index.get(&e).map(|&i| &self.list[i as usize])
    }

    /// Indices of everything within `radius` of `p` (XZ).
    pub fn near(&self, p: DVec3, radius: f64, out: &mut Vec<u32>) {
        out.clear();
        let x0 = ((p.x - radius) / PCELL).floor() as i64;
        let x1 = ((p.x + radius) / PCELL).floor() as i64;
        let z0 = ((p.z - radius) / PCELL).floor() as i64;
        let z1 = ((p.z + radius) / PCELL).floor() as i64;
        let r2 = radius * radius;
        for x in x0..=x1 {
            for z in z0..=z1 {
                if let Some(list) = self.hash.get(&(x, z)) {
                    for &i in list {
                        let q = self.list[i as usize].position;
                        if (q.x - p.x).powi(2) + (q.z - p.z).powi(2) <= r2 {
                            out.push(i);
                        }
                    }
                }
            }
        }
    }
}

/// Body height of an animal or other faction member without a controller (meters). Set by the
/// wildlife module through this component.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BodyHeight(pub f32);

impl Default for BodyHeight {
    fn default() -> Self {
        Self(1.7)
    }
}

type PerceivableItem<'a> = (
    Entity,
    &'a Faction,
    &'a Transform,
    Option<&'a LocomotionState>,
    Option<&'a CharacterController>,
    Option<&'a NpcMotor>,
    Option<&'a BodyHeight>,
    Option<&'a NpcVitals>,
    Option<&'a NpcCombat>,
    Option<&'a NpcClock>,
);

pub(crate) fn tick_clocks(time: Res<Time>, mut q: Query<(&mut NpcClock, Option<&LocalTimeScale>)>) {
    let world_dt = time.delta_secs();
    for (mut clock, local) in &mut q {
        let dt = time.delta_for(local);
        clock.dt = dt;
        clock.now += dt as f64;
        clock.rate = if world_dt > 0.0 { dt / world_dt } else { 1.0 };
    }
}

pub(crate) fn collect_perceivables(mut out: ResMut<Perceivables>, q: Query<PerceivableItem>) {
    out.list.clear();
    out.index.clear();
    for v in out.hash.values_mut() {
        v.clear();
    }
    for (e, faction, t, loco, cc, motor, body, vitals, combat, clock) in &q {
        if vitals.is_some_and(|v| v.downed) {
            continue;
        }
        let now = clock.map(|c| c.now).unwrap_or(0.0);
        let fwd = t.forward();
        let forward = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::NEG_Z);
        let height = cc
            .map(|c| c.height)
            .or(motor.map(|m| m.height))
            .or(body.map(|b| b.0))
            .unwrap_or(1.7);
        let loco = loco.copied().unwrap_or_default();
        let i = out.list.len() as u32;
        out.list.push(Perceivable {
            entity: e,
            faction: *faction,
            position: t.translation,
            forward,
            velocity: loco.velocity,
            height,
            mode: loco.mode,
            blocking: loco.blocking || combat.is_some_and(|c| c.is_blocking(now)),
            attacking: loco.attacking,
            vulnerable: loco.vulnerable || combat.is_some_and(|c| c.is_vulnerable(now)),
        });
        out.index.insert(e, i);
        let key = (
            (t.translation.x / PCELL).floor() as i64,
            (t.translation.z / PCELL).floor() as i64,
        );
        out.hash.entry(key).or_default().push(i);
    }
    out.hash.retain(|_, v| !v.is_empty());
}

/// Light level 0.3 (night) .. 1 (day) from the sun's elevation.
fn daylight(weather: &WeatherState) -> f32 {
    let e = weather.sun_direction.y;
    0.3 + 0.7 * smoothstep(-0.12, 0.08, e)
}

/// Does `a` care about `b` (an enemy to fight, or for passive factions, anything to fear)?
pub fn relevant(factions: &Factions, a: Faction, b: Faction) -> bool {
    factions.hostile(a, b) || (a != b && factions.passive.contains(&a.0))
}

type PerceiverItem<'a> = (
    Entity,
    &'a Faction,
    &'a Transform,
    &'a Perception,
    Option<&'a NpcSkills>,
    &'a NpcClock,
    &'a mut NpcMemory,
    Option<&'a mut NpcCombat>,
    Option<&'a NpcVitals>,
);

#[derive(Default)]
pub(crate) struct Heard {
    noises: Vec<NoiseEvent>,
    traces: Vec<ProjectileTrace>,
    hurts: Vec<DamageEvent>,
    near: Vec<u32>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn perceive(
    mut q: Query<PerceiverItem>,
    seen: Res<Perceivables>,
    factions: Res<Factions>,
    world: Res<TacticalWorld>,
    physics: Res<PhysicsQueries>,
    backend: Option<Res<PhysicsBackend>>,
    ground: Res<Ground>,
    weather: Res<WeatherState>,
    focus: Res<FocusPoints>,
    settings: Res<NpcSettings>,
    mut noises: MessageReader<NoiseEvent>,
    mut traces: MessageReader<ProjectileTrace>,
    mut hurts: MessageReader<DamageEvent>,
    mut scratch: Local<Heard>,
) {
    let heard = &mut *scratch;
    heard.noises.clear();
    heard.noises.extend(noises.read().copied());
    heard.traces.clear();
    heard.traces.extend(traces.read().copied());
    heard.hurts.clear();
    heard.hurts.extend(hurts.read().cloned());
    let sight = Sight {
        world: &world,
        physics: physics_backend(&backend, &physics),
        ground: &ground,
    };
    let light = daylight(&weather);
    let fog = 1.0 - 0.65 * weather.fog.clamp(0.0, 1.0);
    let rain = 1.0 - 0.15 * (weather.precipitation_mm_h / 20.0).clamp(0.0, 1.0);
    let mut near = std::mem::take(&mut heard.near);

    for (me, faction, t, perception, skills, clock, mut memory, mut combat, vitals) in &mut q {
        if vitals.is_some_and(|v| v.downed) {
            continue;
        }
        let now = clock.now;
        let pos = t.translation;
        let awareness_skill = skills.map(|s| s.awareness).unwrap_or(0.8);
        let eye = pos + DVec3::Y * perception.eye_height as f64;

        // ---------------------------------------------------------------- hearing
        for n in &heard.noises {
            if n.source == Some(me) {
                continue;
            }
            let reach = n.radius * perception.hearing * (0.6 + 0.4 * awareness_skill);
            let d = n.position.distance(pos) as f32;
            if d > reach {
                continue;
            }
            let strength = 1.0 - d / reach.max(0.01);
            match n.source.and_then(|s| seen.get(s)) {
                Some(src) if relevant(&factions, *faction, src.faction) => {
                    let k = memory.entry(src.entity, src.faction, now, n.position);
                    if !k.visible {
                        k.last_position = n.position;
                        k.last_seen = now;
                    }
                    k.awareness = (k.awareness + 0.35 + 0.4 * strength)
                        .min(if k.awareness >= 1.0 { 1.0 } else { 0.95 });
                }
                Some(src) if src.faction == *faction => {
                    // A friend shouting or fighting: worth a look.
                    memory.disturb(n.position, now, strength * 0.6);
                }
                _ => memory.disturb(n.position, now, strength),
            }
        }
        // ---------------------------------------------------------------- bullets
        let chest = pos + DVec3::Y * (perception.eye_height as f64 * 0.75);
        for tr in &heard.traces {
            if tr.shooter == Some(me) {
                continue;
            }
            let d = segment_distance(chest, tr.from, tr.to);
            if d > 3.0 {
                continue;
            }
            if let Some(c) = combat.as_deref_mut() {
                c.suppression = (c.suppression + 0.3 * (1.0 - d as f32 / 3.0) + 0.1).min(1.0);
            }
            if let Some(src) = tr.shooter.and_then(|s| seen.get(s))
                && relevant(&factions, *faction, src.faction)
            {
                let k = memory.entry(src.entity, src.faction, now, tr.from);
                if !k.visible {
                    k.last_position = DVec3::new(tr.from.x, src.position.y, tr.from.z);
                    k.last_seen = now;
                }
                k.awareness = k.awareness.max(0.9);
            } else {
                memory.disturb(tr.from, now, 1.0);
            }
        }
        // ---------------------------------------------------------------- pain
        for h in &heard.hurts {
            if h.target != me {
                continue;
            }
            if let Some(c) = combat.as_deref_mut() {
                c.last_hurt = now;
            }
            match h.source.and_then(|s| seen.get(s)) {
                Some(src) if src.entity != me => {
                    let k = memory.entry(src.entity, src.faction, now, src.position);
                    k.last_position = src.position;
                    k.last_seen = now;
                    k.awareness = 1.0;
                }
                _ => {
                    let from = h.point - (h.impulse.normalize_or_zero() * 6.0).as_dvec3();
                    memory.disturb(from, now, 1.0);
                }
            }
        }

        // ---------------------------------------------------------------- sight (staggered)
        if now < memory.next_look {
            continue;
        }
        let far = focus.points.is_empty() || focus.distance(pos) > settings.far_distance as f64;
        memory.next_look = now + if far { 0.8 } else { 0.1 };
        let look_dt = if far { 0.8 } else { 0.1 };
        let fwd = t.forward();
        let fwd = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::NEG_Z);
        let range =
            perception.sight_range * (0.5 + 0.5 * awareness_skill) * light.max(0.45) * fog * rain;
        let half_fov = (perception.fov_deg * 0.5).to_radians().cos();
        let half_periph = (perception.peripheral_deg * 0.5).to_radians().cos();
        for k in memory.known.iter_mut() {
            k.visible = false;
        }
        seen.near(pos, range as f64, &mut near);
        for &i in &near {
            let other = seen.list[i as usize];
            if other.entity == me || !relevant(&factions, *faction, other.faction) {
                continue;
            }
            let to = other.position - pos;
            let dist = to.length() as f32;
            if dist > range || dist < 1e-3 {
                continue;
            }
            let dir = Vec3::new(to.x as f32, 0.0, to.z as f32).normalize_or_zero();
            let cos = if dist < 1.2 { 1.0 } else { dir.dot(fwd) };
            let zone = if cos >= half_fov {
                1.0
            } else if cos >= half_periph {
                0.33
            } else {
                continue;
            };
            // Look at the head, then the chest.
            let head = other.position + DVec3::Y * (other.height as f64 * 0.9);
            let mid = other.position + DVec3::Y * (other.height as f64 * 0.55);
            let ignore = [me, other.entity];
            let visible = sight.clear(eye, head, &ignore) || sight.clear(eye, mid, &ignore);
            if !visible {
                continue;
            }
            let close = 1.0 - smoothstep(range * 0.15, range, dist) * 0.85;
            let stance = match other.mode {
                LocomotionMode::Crouch => 0.45,
                LocomotionMode::Idle => 0.8,
                _ => 1.0,
            };
            let moving = 1.0 + (other.velocity.length() / 5.0).min(1.0) * 0.6;
            let prev = memory.get(other.entity).map(|k| k.awareness).unwrap_or(0.0);
            let rate = zone * close * light * stance * moving * (0.4 + 0.6 * awareness_skill)
                / perception.detection_secs.max(0.05)
                * if prev > 0.5 { 2.0 } else { 1.0 };
            let k = memory.entry(other.entity, other.faction, now, other.position);
            k.visible = true;
            k.reported = false;
            k.awareness = (k.awareness + rate * look_dt).min(1.0);
            k.last_seen = now;
            k.last_position = other.position;
            k.last_velocity = other.velocity;
            k.history.push_back(Snapshot {
                time: now,
                position: other.position,
                velocity: other.velocity,
                forward: other.forward,
                mode: other.mode,
                blocking: other.blocking,
                attacking: other.attacking,
                vulnerable: other.vulnerable,
            });
            while k.history.len() > 24 || k.history.front().is_some_and(|s| now - s.time > 2.0) {
                k.history.pop_front();
            }
        }
        // Forget what has been out of mind for long.
        let memory_secs = perception.memory_secs as f64;
        memory.known.retain_mut(|k| {
            if !k.visible {
                let since = now - k.last_seen;
                if since > 3.0 {
                    k.awareness = (k.awareness - 0.04 * look_dt).max(0.0);
                }
                return since < memory_secs && seen.get(k.entity).is_some();
            }
            true
        });
    }
    heard.near = near;
}

/// Distance from `p` to the segment `a–b`.
pub fn segment_distance(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-12 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}
