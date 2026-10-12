//! Stand-in bodies: simple figures built from primitives and posed every step from what the
//! AI is doing, so behavior reads clearly before real character models exist.
//!
//! Fighters walk and run with swinging limbs, crouch behind low cover, raise their weapon
//! during an attack's telegraph (it glows orange for a heavy guard breaker), hold the weapon
//! across the body to block (a white flash on a parry), lean into dodges, reel when staggered,
//! flash when hit and fall when downed. Shooters shoulder their rifle and show a muzzle flash.
//! A band on the head shows skill (grey rookie, bronze, silver, gold master) and an orb above
//! it shows alertness (yellow suspicious, orange searching, red fighting).
//!
//! Animals graze head-down, raise their heads when alert, lie down to rest, gallop (deer and
//! wolves), hop (rabbits), creep low when stalking, flap and bank in flight, and fall over
//! when killed.
//!
//! With `NpcSettings::show_intent` on, a ring on the ground marks where each NPC decided to
//! go and a line leads to it, colored by why: blue cover, orange flank, purple high ground,
//! grey falling back, yellow investigating or searching.

use std::f32::consts::FRAC_PI_2;

use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::locomotion::{LocomotionMode, LocomotionState, MoveIntent};

use crate::agent::{
    AlertLevel, AttackPhase, Defense, NpcBrain, NpcClock, NpcCombat, NpcSettings, NpcVitals,
    NpcWeapon, WeaponStyle,
};
use crate::combat::Downed;
use crate::faction::Faction;
use crate::locomotion::MoverState;
use crate::skills::NpcSkills;
use crate::wildlife::{Animal, AnimalMind, AnimalState, BirdFlight, Species};

/// Give an NPC or animal a stand-in body. Colors are sRGB hex; `None` picks from the faction
/// (fighters) or species (animals).
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StandIn {
    pub color: Option<u32>,
    /// Hide the alert orb and skill band.
    pub plain: bool,
}

/// One piece of a stand-in body (a child of the NPC or animal).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct StandInPart {
    pub part: Part,
    scale: Vec3,
}

/// Body parts of stand-ins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    Torso,
    Head,
    Band,
    LegL,
    LegR,
    ArmL,
    ArmR,
    Weapon,
    Shield,
    Orb,
    Muzzle,
    // Four-legged animals and birds.
    Body,
    Neck,
    Snout,
    Leg(u8),
    Tail,
    Ear(u8),
    Antler(u8),
    Wing(u8),
}

/// Ground marker showing where an NPC is going (see the module docs).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct IntentMarker {
    pub owner: Entity,
    line: bool,
}

/// Per-body bookkeeping for effects.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct StandInState {
    was_winding: bool,
    flash_until: f64,
    markers: Option<[Entity; 2]>,
}

fn mat(hex: u32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(0.75)
}

fn glow(hex: u32, strength: f32) -> StandardMaterial {
    let m = StandardMaterial::hex(hex);
    let c = [m.base_color[0], m.base_color[1], m.base_color[2]];
    m.with_emissive(c, strength).with_roughness(0.4)
}

const SKIN: u32 = 0xd2a37c;
const STEEL: u32 = 0xb9bec7;
const GUNMETAL: u32 = 0x2c2d30;
const WOOD: u32 = 0x7a5230;

fn faction_color(f: Option<&Faction>) -> u32 {
    match f.map(|f| f.0) {
        Some(0) => 0x3d8a4e,
        Some(1) => 0xa8392c,
        Some(2) => 0x2f5fa8,
        _ => 0x8a8a8a,
    }
}

fn skill_color(level: f32) -> u32 {
    if level < 0.35 {
        0x7d7d7d
    } else if level < 0.6 {
        0xb0773a
    } else if level < 0.8 {
        0xd0d4da
    } else {
        0xf0c030
    }
}

fn species_color(s: Species) -> u32 {
    match s {
        Species::Deer => 0x8d5b34,
        Species::Rabbit => 0x9a8166,
        Species::Wolf => 0x77787a,
        Species::Bird => 0x3f4558,
    }
}

type BuildItem<'a> = (
    Entity,
    &'a StandIn,
    Option<&'a Animal>,
    Option<&'a NpcWeapon>,
    Option<&'a Faction>,
    Option<&'a NpcSkills>,
);

/// Spawn the parts of new stand-ins.
pub(crate) fn build_stand_ins(mut commands: Commands, q: Query<BuildItem, Added<StandIn>>) {
    for (e, stand_in, animal, weapon, faction, skills) in &q {
        let mut parts: Vec<(Part, Shape, StandardMaterial, Vec3)> = Vec::new();
        let one = Vec3::ONE;
        let capsule = |radius: f32, height: f32| Shape::Capsule {
            radius,
            height,
            segments: 12,
        };
        let sphere = |radius: f32| Shape::Sphere {
            radius,
            segments: 16,
        };
        let cone = |radius: f32, height: f32| Shape::Cone {
            radius,
            height,
            segments: 10,
        };
        let cuboid = |x: f32, y: f32, z: f32| Shape::Cuboid { size: [x, y, z] };
        match animal.map(|a| a.species) {
            None => {
                let body = mat(stand_in.color.unwrap_or_else(|| faction_color(faction)));
                let dark = mat(0x2a2a2e);
                parts.push((Part::Torso, capsule(0.19, 0.72), body, one));
                parts.push((Part::Head, sphere(0.13), mat(SKIN), one));
                parts.push((Part::LegL, capsule(0.085, 0.9), dark, one));
                parts.push((Part::LegR, capsule(0.085, 0.9), dark, one));
                parts.push((Part::ArmL, capsule(0.07, 0.62), body, one));
                parts.push((Part::ArmR, capsule(0.07, 0.62), body, one));
                if !stand_in.plain {
                    let level = skills.map(|s| s.level()).unwrap_or(0.5);
                    let band = if level >= 0.8 {
                        glow(skill_color(level), 2.0)
                    } else {
                        mat(skill_color(level)).with_metallic(0.6)
                    };
                    parts.push((
                        Part::Band,
                        Shape::Cylinder {
                            radius: 0.135,
                            height: 0.07,
                            segments: 16,
                        },
                        band,
                        one,
                    ));
                    parts.push((Part::Orb, sphere(0.09), glow(0xf2c94c, 4.0), one));
                }
                let w = weapon.copied().unwrap_or_default();
                match w.style {
                    WeaponStyle::Melee if w.can_block => {
                        parts.push((
                            Part::Weapon,
                            cuboid(0.05, 0.07, 1.0),
                            mat(STEEL).with_metallic(0.9).with_roughness(0.3),
                            one,
                        ));
                    }
                    WeaponStyle::Melee => {}
                    WeaponStyle::Ranged => {
                        parts.push((Part::Weapon, cuboid(0.07, 0.13, 0.9), mat(GUNMETAL), one));
                        parts.push((Part::Muzzle, sphere(0.12), glow(0xffc060, 40.0), one));
                    }
                }
                if w.shield {
                    parts.push((Part::Shield, cuboid(0.5, 0.65, 0.06), mat(WOOD), one));
                }
            }
            Some(species) => {
                let body = mat(stand_in.color.unwrap_or_else(|| species_color(species)));
                let pale = mat(0xe8e2d6);
                let dark = mat(0x2a2522);
                match species {
                    Species::Deer => {
                        parts.push((Part::Body, capsule(0.27, 1.3), body, one));
                        parts.push((Part::Neck, capsule(0.09, 0.65), body, one));
                        parts.push((Part::Head, capsule(0.09, 0.36), body, one));
                        parts.push((Part::Snout, sphere(0.05), dark, one));
                        for k in 0..4 {
                            parts.push((Part::Leg(k), capsule(0.045, 0.95), body, one));
                        }
                        parts.push((Part::Tail, sphere(0.07), pale, Vec3::new(1.0, 1.3, 0.7)));
                        for k in 0..2 {
                            parts.push((Part::Ear(k), cone(0.05, 0.16), body, one));
                        }
                        if animal.is_some_and(|a| a.male) {
                            for k in 0..2 {
                                parts.push((Part::Antler(k), cone(0.03, 0.45), mat(0xd9c9a3), one));
                            }
                        }
                    }
                    Species::Wolf => {
                        parts.push((Part::Body, capsule(0.18, 1.0), body, one));
                        parts.push((Part::Neck, capsule(0.1, 0.34), body, one));
                        parts.push((Part::Head, sphere(0.12), body, one));
                        parts.push((Part::Snout, cone(0.06, 0.2), dark, one));
                        for k in 0..4 {
                            parts.push((Part::Leg(k), capsule(0.04, 0.6), body, one));
                        }
                        parts.push((Part::Tail, capsule(0.05, 0.5), body, one));
                        for k in 0..2 {
                            parts.push((Part::Ear(k), cone(0.045, 0.12), dark, one));
                        }
                    }
                    Species::Rabbit => {
                        parts.push((Part::Body, sphere(0.14), body, Vec3::new(0.9, 0.85, 1.25)));
                        parts.push((Part::Head, sphere(0.075), body, one));
                        for k in 0..2 {
                            parts.push((Part::Ear(k), capsule(0.022, 0.17), body, one));
                        }
                        parts.push((Part::Tail, sphere(0.04), pale, one));
                    }
                    Species::Bird => {
                        parts.push((Part::Body, sphere(0.07), body, Vec3::new(0.85, 0.8, 1.5)));
                        parts.push((Part::Head, sphere(0.045), body, one));
                        parts.push((Part::Snout, cone(0.015, 0.05), mat(0xe0a030), one));
                        for k in 0..2 {
                            parts.push((Part::Wing(k), cuboid(0.24, 0.012, 0.1), body, one));
                        }
                    }
                }
            }
        }
        for (part, shape, material, scale) in parts {
            commands.spawn((
                StandInPart { part, scale },
                Transform::default().with_scale(scale),
                MeshRenderer::new(shape, material),
                ChildOf(e),
            ));
        }
        commands.entity(e).insert(StandInState::default());
    }
}

/// A part's pose relative to its owner.
#[derive(Debug, Clone, Copy)]
struct Placed {
    pos: Vec3,
    rot: Quat,
    visible: bool,
}

fn at(pos: Vec3, rot: Quat) -> Placed {
    Placed {
        pos,
        rot,
        visible: true,
    }
}

const HIDDEN: Placed = Placed {
    pos: Vec3::ZERO,
    rot: Quat::IDENTITY,
    visible: false,
};

/// A limb hanging from `pivot`, rotated by `rot`, `len` long: its mesh center.
fn limb(pivot: Vec3, rot: Quat, len: f32) -> Placed {
    at(pivot + rot * Vec3::new(0.0, -len * 0.5, 0.0), rot)
}

type PoseItem<'a> = (
    &'a StandIn,
    &'a mut StandInState,
    &'a Children,
    &'a LocomotionState,
    Option<&'a MoverState>,
    Option<&'a MoveIntent>,
    Option<&'a NpcClock>,
    Option<&'a NpcCombat>,
    Option<&'a NpcBrain>,
    Option<&'a NpcVitals>,
    Option<&'a Downed>,
    Option<&'a NpcWeapon>,
    (
        Option<&'a Animal>,
        Option<&'a AnimalMind>,
        Option<&'a BirdFlight>,
        Option<&'a Faction>,
    ),
);

/// Pose every stand-in from its owner's state.
pub(crate) fn pose_stand_ins(
    mut owners: Query<PoseItem>,
    mut parts: Query<(&StandInPart, &mut Transform, &mut MeshRenderer)>,
) {
    for (
        stand_in,
        mut st,
        children,
        loco,
        mover,
        intent,
        clock,
        combat,
        brain,
        vitals,
        downed,
        weapon,
        animal,
    ) in &mut owners
    {
        let now = clock.map(|c| c.now).unwrap_or(0.0);
        let phase = mover.map(|m| m.phase).unwrap_or(0.0);
        let speed = Vec3::new(loco.velocity.x, 0.0, loco.velocity.z).length();
        let (animal, mind, bird, faction) = animal;
        let mut materials: Vec<(Part, StandardMaterial)> = Vec::new();
        let placed: Box<dyn Fn(Part) -> Placed> = match animal.map(|a| a.species) {
            None => {
                let pose = humanoid(
                    now,
                    phase,
                    speed,
                    loco,
                    intent,
                    combat,
                    brain,
                    vitals,
                    downed,
                    weapon.copied().unwrap_or_default(),
                    &mut st,
                    &mut materials,
                    stand_in,
                    stand_in.color.unwrap_or_else(|| faction_color(faction)),
                );
                Box::new(pose)
            }
            Some(species) => {
                let state = mind.map(|m| m.state).unwrap_or_default();
                let since = mind.map(|m| m.since).unwrap_or(0.0);
                let biting = mind.is_some_and(|m| now < m.bite_ready - 0.55);
                let flap = bird.map(|b| b.flap).unwrap_or(0.0);
                let male = animal.is_some_and(|a| a.male);
                Box::new(move |p| {
                    animal_pose(
                        species, p, state, since, now, phase, speed, loco, biting, flap, male,
                    )
                })
            }
        };
        for child in children.iter() {
            let Ok((part, mut t, mut r)) = parts.get_mut(child) else {
                continue;
            };
            let p = placed(part.part);
            let new = Transform::from_translation(p.pos.as_dvec3())
                .with_rotation(p.rot)
                .with_scale(part.scale);
            if *t != new {
                *t = new;
            }
            if r.visible != p.visible {
                r.visible = p.visible;
            }
            if let Some((_, m)) = materials.iter().find(|(q, _)| *q == part.part) {
                let m = engine_render::MaterialRef::Inline(*m);
                if r.material != m {
                    r.material = m;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn humanoid(
    now: f64,
    phase: f32,
    speed: f32,
    loco: &LocomotionState,
    intent: Option<&MoveIntent>,
    combat: Option<&NpcCombat>,
    brain: Option<&NpcBrain>,
    vitals: Option<&NpcVitals>,
    downed: Option<&Downed>,
    weapon: NpcWeapon,
    st: &mut StandInState,
    materials: &mut Vec<(Part, StandardMaterial)>,
    stand_in: &StandIn,
    body_color: u32,
) -> impl Fn(Part) -> Placed + use<> {
    let crouch = if intent.is_some_and(|i| i.crouch) || loco.mode == LocomotionMode::Crouch {
        1.0
    } else {
        0.0
    };
    let amp = (speed / 4.5).min(1.0) * 0.75;
    let swing = phase.sin() * amp;
    let mut lean = -(speed / 7.0).min(1.0) * 0.18 + crouch * 0.25;
    let mut roll = 0.0;
    let mut legs = [crouch * 1.0 + swing, crouch * 1.0 - swing];
    let mut arm_l = Quat::from_rotation_x(-swing * 0.8);
    let mut arm_r = Quat::from_rotation_x(swing * 0.8);
    let mut hips = 0.92 - crouch * 0.42;
    let ranged = weapon.style == WeaponStyle::Ranged;
    let mut weapon_mat = if ranged {
        mat(GUNMETAL)
    } else {
        mat(STEEL).with_metallic(0.9).with_roughness(0.3)
    };
    let fighting = brain.is_some_and(|b| b.alert == AlertLevel::Combat);
    let mut winding = false;
    if let Some(c) = combat {
        match c.phase {
            AttackPhase::Windup { hit_at, heavy, .. } => {
                winding = true;
                if ranged {
                    arm_r = Quat::from_rotation_x(1.45);
                    arm_l = Quat::from_rotation_x(1.35) * Quat::from_rotation_z(-0.45);
                } else {
                    let total = (weapon.windup * if heavy { 1.6 } else { 1.0 }).max(0.05) as f64;
                    let p = (1.0 - (hit_at - now) / total).clamp(0.0, 1.0) as f32;
                    let raise = p.sqrt() * if heavy { 3.0 } else { 2.5 };
                    arm_r = Quat::from_rotation_x(raise) * Quat::from_rotation_z(0.25);
                    if heavy {
                        arm_l = Quat::from_rotation_x(raise * 0.95) * Quat::from_rotation_z(-0.3);
                        weapon_mat = glow(0xff7a1a, 30.0);
                    }
                    lean -= 0.12 * p;
                }
            }
            AttackPhase::Recovery { until } if now < until => {
                if ranged {
                    arm_r = Quat::from_rotation_x(1.5);
                    arm_l = Quat::from_rotation_x(1.35) * Quat::from_rotation_z(-0.45);
                } else {
                    arm_r = Quat::from_rotation_x(0.75) * Quat::from_rotation_z(-0.5);
                    lean += 0.28;
                }
            }
            AttackPhase::Staggered { until } if now < until => {
                lean = -0.35 + (now * 12.0).sin() as f32 * 0.05;
                arm_l = Quat::from_rotation_z(-1.0);
                arm_r = Quat::from_rotation_z(1.0);
            }
            _ => {
                if ranged && fighting {
                    arm_r = Quat::from_rotation_x(1.4);
                    arm_l = Quat::from_rotation_x(1.3) * Quat::from_rotation_z(-0.45);
                } else if ranged {
                    arm_r = Quat::from_rotation_x(0.7);
                    arm_l = Quat::from_rotation_x(0.9) * Quat::from_rotation_z(-0.5);
                } else if fighting && speed < 3.0 {
                    // Guard: weapon forward and up.
                    arm_r = Quat::from_rotation_x(0.9) * Quat::from_rotation_z(0.2);
                    arm_l = Quat::from_rotation_x(0.6) * Quat::from_rotation_z(-0.2);
                    legs = [crouch + 0.25 + swing * 0.5, crouch - 0.2 - swing * 0.5];
                    hips -= 0.05;
                }
            }
        }
        match c.defense {
            Defense::Block { until } | Defense::Parry { until } if now < until => {
                arm_r = Quat::from_rotation_x(1.35) * Quat::from_rotation_z(0.95);
                arm_l = Quat::from_rotation_x(1.25) * Quat::from_rotation_z(-0.6);
                lean += 0.05;
                if matches!(c.defense, Defense::Parry { .. }) {
                    weapon_mat = glow(0xffffff, 25.0);
                }
            }
            Defense::Dodge { until } if now < until => {
                let side = loco.velocity.x.signum();
                roll = 0.4 * side;
                hips -= 0.15;
            }
            _ => {}
        }
        // Flash when hit.
        let hurt = c.last_hurt > 0.0 && now - c.last_hurt < 0.12;
        materials.push((
            Part::Torso,
            if hurt {
                glow(0xffffff, 6.0)
            } else {
                mat(body_color)
            },
        ));
    }
    // Muzzle flash right after a shot.
    if ranged && st.was_winding && !winding {
        st.flash_until = now + 0.07;
    }
    st.was_winding = winding;
    let flash = ranged && now < st.flash_until;
    materials.push((Part::Weapon, weapon_mat));
    let orb = if stand_in.plain {
        None
    } else {
        match brain.map(|b| b.alert) {
            Some(AlertLevel::Suspicious) => Some(0xf2c94c),
            Some(AlertLevel::Searching) => Some(0xf2994a),
            Some(AlertLevel::Combat) => Some(0xeb4d4b),
            _ => None,
        }
    };
    if let Some(c) = orb {
        materials.push((Part::Orb, glow(c, 6.0)));
    }
    // Downed: fall backward over half a second.
    let fall = match (vitals.is_some_and(|v| v.downed), downed) {
        (true, Some(d)) => ((now - d.since) / 0.5).clamp(0.0, 1.0) as f32,
        (true, None) => 1.0,
        _ => 0.0,
    };
    let fall = fall * fall * (3.0 - 2.0 * fall);
    let body_rot = Quat::from_rotation_x(fall * FRAC_PI_2);
    let body_lift = Vec3::Y * 0.2 * fall;
    let torso_rot = Quat::from_rotation_x(lean) * Quat::from_rotation_z(roll);
    let hip = Vec3::new(0.0, hips, 0.0);
    let shoulder = move |side: f32| hip + torso_rot * Vec3::new(0.27 * side, 0.55, 0.0);
    let head = hip + torso_rot * Vec3::new(0.0, 0.8, 0.0);
    let arm = [arm_l, arm_r];
    let hand = move |i: usize| {
        let side = if i == 0 { -1.0 } else { 1.0 };
        shoulder(side) + torso_rot * arm[i] * Vec3::new(0.0, -0.58, 0.0)
    };
    let whole = move |p: Placed| -> Placed {
        if fall <= 0.0 {
            return p;
        }
        Placed {
            pos: body_rot * p.pos + body_lift,
            rot: body_rot * p.rot,
            visible: p.visible,
        }
    };
    let weapon_style = weapon.style;
    move |part| {
        let p = match part {
            Part::Torso => at(hip + torso_rot * Vec3::new(0.0, 0.32, 0.0), torso_rot),
            Part::Head => at(head, torso_rot),
            Part::Band => at(head + torso_rot * Vec3::new(0.0, 0.06, 0.0), torso_rot),
            Part::LegL => limb(
                Vec3::new(-0.1, hips, 0.0),
                Quat::from_rotation_x(legs[0]),
                0.9,
            ),
            Part::LegR => limb(
                Vec3::new(0.1, hips, 0.0),
                Quat::from_rotation_x(legs[1]),
                0.9,
            ),
            Part::ArmL => limb(shoulder(-1.0), torso_rot * arm[0], 0.62),
            Part::ArmR => limb(shoulder(1.0), torso_rot * arm[1], 0.62),
            Part::Weapon => match weapon_style {
                WeaponStyle::Melee => {
                    let r = torso_rot * arm[1];
                    at(hand(1) + r * Vec3::new(0.0, 0.0, -0.45), r)
                }
                WeaponStyle::Ranged => {
                    let r = torso_rot * arm[1] * Quat::from_rotation_x(-FRAC_PI_2);
                    at(hand(1) + r * Vec3::new(-0.08, 0.05, -0.25), r)
                }
            },
            Part::Muzzle => {
                if flash {
                    let r = torso_rot * arm[1] * Quat::from_rotation_x(-FRAC_PI_2);
                    at(hand(1) + r * Vec3::new(-0.08, 0.05, -0.8), r)
                } else {
                    HIDDEN
                }
            }
            Part::Shield => {
                let r = torso_rot * arm[0];
                at(
                    hand(0) + torso_rot * Vec3::new(0.05, 0.12, -0.1),
                    r * Quat::from_rotation_x(-arm_angle(arm[0])),
                )
            }
            Part::Orb => {
                return if orb.is_some() && fall <= 0.0 {
                    at(head + Vec3::Y * 0.42, Quat::IDENTITY)
                } else {
                    HIDDEN
                };
            }
            _ => HIDDEN,
        };
        whole(p)
    }
}

/// Pitch of an arm rotation (to keep the shield upright).
fn arm_angle(q: Quat) -> f32 {
    let (x, _, _) = q.to_euler(EulerRot::XYZ);
    x
}

#[allow(clippy::too_many_arguments)]
fn animal_pose(
    species: Species,
    part: Part,
    state: AnimalState,
    since: f64,
    now: f64,
    phase: f32,
    speed: f32,
    loco: &LocomotionState,
    biting: bool,
    flap: f32,
    male: bool,
) -> Placed {
    let moving = speed > 0.2;
    let head_down = matches!(
        state,
        AnimalState::Graze | AnimalState::Drink | AnimalState::Feed
    ) && !moving;
    let alert = state == AnimalState::Alert;
    let resting = state == AnimalState::Rest;
    let dead = state == AnimalState::Dead;
    let low = state == AnimalState::Stalk || loco.mode == LocomotionMode::Crouch;
    let galloping = loco.mode == LocomotionMode::Sprint || speed > 5.0;
    let fall = if dead {
        ((now - since) / 0.6).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let roll = Quat::from_rotation_z(fall * FRAC_PI_2);
    let along = Quat::from_rotation_x(FRAC_PI_2);
    let leg_swing = |k: u8| -> f32 {
        if !moving {
            return 0.0;
        }
        let amp = (speed / 6.0).clamp(0.25, 0.9);
        let front = k < 2;
        let left = k.is_multiple_of(2);
        let offset = if galloping {
            if front { 0.0 } else { PI * 0.8 }
        } else if front == left {
            0.0
        } else {
            PI
        };
        (phase + offset + if galloping && left { 0.25 } else { 0.0 }).sin() * amp
    };
    let place = |p: Placed| -> Placed {
        if fall <= 0.0 {
            return p;
        }
        Placed {
            pos: roll * p.pos + Vec3::Y * 0.0,
            rot: roll * p.rot,
            visible: p.visible,
        }
    };
    match species {
        Species::Deer | Species::Wolf => {
            let deer = species == Species::Deer;
            let (body_y, body_len, leg_len, half_x, half_z) = if deer {
                (1.0, 1.3, 0.95, 0.14, 0.42)
            } else {
                (0.62, 1.0, 0.6, 0.11, 0.33)
            };
            let mut y = body_y;
            if resting {
                y = if deer { 0.32 } else { 0.22 };
            } else if low {
                y -= 0.18;
            } else if dead {
                y = if deer { 0.3 } else { 0.2 };
            }
            if galloping {
                y += (phase * 2.0).sin().abs() * 0.08;
            }
            let pitch = if galloping { (phase).sin() * 0.06 } else { 0.0 };
            let body_rot = Quat::from_rotation_x(pitch);
            let body = Vec3::new(0.0, y, 0.0);
            let front = body + body_rot * Vec3::new(0.0, 0.08, -body_len * 0.38);
            // Neck direction: up-forward, straight up when alert, down to the grass.
            let neck_dir = if head_down {
                Vec3::new(0.0, -0.75, -0.65)
            } else if alert {
                Vec3::new(0.0, 0.97, -0.25)
            } else if low || (!deer && moving) {
                Vec3::new(0.0, 0.25, -0.97)
            } else if deer {
                Vec3::new(0.0, 0.78, -0.62)
            } else {
                Vec3::new(0.0, 0.45, -0.9)
            }
            .normalize();
            let neck_len = if deer { 0.6 } else { 0.3 };
            let neck_rot = Quat::from_rotation_arc(Vec3::Y, neck_dir);
            let neck_end = front + neck_dir * neck_len;
            let lunge = if biting {
                Vec3::new(0.0, -0.05, -0.25)
            } else {
                Vec3::ZERO
            };
            let head = neck_end + lunge;
            let face = if head_down {
                Vec3::new(0.0, -0.9, -0.45)
            } else {
                Vec3::new(0.0, -0.2, -1.0)
            }
            .normalize();
            let head_rot = Quat::from_rotation_arc(Vec3::Y, face);
            let p = match part {
                Part::Body => at(body, body_rot * along),
                Part::Neck => at(front + neck_dir * neck_len * 0.5, neck_rot),
                Part::Head => {
                    if deer {
                        at(head + face * 0.12, head_rot)
                    } else {
                        at(head, Quat::IDENTITY)
                    }
                }
                Part::Snout => {
                    if deer {
                        at(head + face * 0.3, Quat::IDENTITY)
                    } else {
                        at(head + face * 0.17, head_rot)
                    }
                }
                Part::Leg(k) => {
                    let side = if k % 2 == 0 { -1.0 } else { 1.0 };
                    let z = if k < 2 { -half_z } else { half_z };
                    let hip = body + body_rot * Vec3::new(side * half_x, -0.05, z);
                    let rot = if resting || dead {
                        Quat::from_rotation_x(if k < 2 { 1.45 } else { -1.45 })
                    } else {
                        Quat::from_rotation_x(leg_swing(k) + if low { 0.35 } else { 0.0 })
                    };
                    limb(hip, rot, leg_len)
                }
                Part::Tail => {
                    let back = body + body_rot * Vec3::new(0.0, 0.1, body_len * 0.45);
                    if deer {
                        // Flagging white tail when alarmed.
                        let up = matches!(state, AnimalState::Alert | AnimalState::Flee);
                        at(
                            back + if up {
                                Vec3::new(0.0, 0.12, 0.03)
                            } else {
                                Vec3::new(0.0, -0.05, 0.05)
                            },
                            Quat::IDENTITY,
                        )
                    } else {
                        let droop = if state == AnimalState::Chase {
                            1.4
                        } else if low {
                            2.4
                        } else {
                            2.0
                        };
                        let r = Quat::from_rotation_x(droop);
                        at(back + r * Vec3::new(0.0, 0.25, 0.0), r)
                    }
                }
                Part::Ear(k) => {
                    let side = if k == 0 { -1.0 } else { 1.0 };
                    let up = if head_down { face } else { Vec3::Y };
                    let base = head + Vec3::new(side * 0.07, 0.0, 0.0) + up * 0.08;
                    let tilt = Quat::from_rotation_z(-side * if alert { 0.1 } else { 0.5 });
                    at(base + Vec3::new(0.0, 0.06, 0.0), tilt)
                }
                Part::Antler(k) => {
                    if !male {
                        return HIDDEN;
                    }
                    let side = if k == 0 { -1.0 } else { 1.0 };
                    let up = if head_down { face } else { Vec3::Y };
                    let r = Quat::from_rotation_z(-side * 0.45) * Quat::from_rotation_x(0.25);
                    at(head + Vec3::new(side * 0.06, 0.0, 0.0) + up * 0.25, r)
                }
                _ => HIDDEN,
            };
            place(p)
        }
        Species::Rabbit => {
            let hop = if moving {
                (phase).sin().abs() * 0.14
            } else {
                0.0
            };
            let y = 0.13 + hop - if resting { 0.03 } else { 0.0 };
            let sit_up = alert && !moving;
            let body = Vec3::new(0.0, y + if sit_up { 0.04 } else { 0.0 }, 0.0);
            let body_rot = if sit_up {
                Quat::from_rotation_x(0.9)
            } else {
                Quat::IDENTITY
            };
            let head = body
                + if sit_up {
                    Vec3::new(0.0, 0.17, -0.04)
                } else {
                    Vec3::new(0.0, 0.07, -0.16)
                };
            let p = match part {
                Part::Body => at(body, body_rot),
                Part::Head => at(head, Quat::IDENTITY),
                Part::Ear(k) => {
                    let side = if k == 0 { -1.0 } else { 1.0 };
                    let back = if moving || resting { 0.9 } else { 0.15 };
                    let r = Quat::from_rotation_x(back) * Quat::from_rotation_z(-side * 0.15);
                    at(
                        head + Vec3::new(side * 0.03, 0.05, 0.0) + r * Vec3::new(0.0, 0.08, 0.0),
                        r,
                    )
                }
                Part::Tail => at(body + body_rot * Vec3::new(0.0, 0.03, 0.17), Quat::IDENTITY),
                _ => HIDDEN,
            };
            place(p)
        }
        Species::Bird => {
            let flying = state == AnimalState::Flying;
            let peck = !flying && ((now * 2.3 + since).sin() > 0.6);
            let body = Vec3::new(0.0, if flying { 0.0 } else { 0.07 }, 0.0);
            let head = body
                + if peck {
                    Vec3::new(0.0, -0.02, -0.12)
                } else {
                    Vec3::new(0.0, 0.06, -0.09)
                };
            let p = match part {
                Part::Body => at(body, Quat::IDENTITY),
                Part::Head => at(head, Quat::IDENTITY),
                Part::Snout => at(
                    head + Vec3::new(0.0, 0.0, -0.05),
                    Quat::from_rotation_x(-FRAC_PI_2),
                ),
                Part::Wing(k) => {
                    let side = if k == 0 { -1.0 } else { 1.0 };
                    let a = if flying { flap.sin() * 0.9 } else { -0.15 };
                    let r = Quat::from_rotation_z(side * a);
                    let reach = if flying { 0.13 } else { 0.06 };
                    at(body + r * Vec3::new(side * reach, 0.02, 0.0), r)
                }
                _ => HIDDEN,
            };
            place(p)
        }
    }
}

/// Rings and lines on the ground showing where NPCs are going and why.
#[allow(clippy::type_complexity)]
pub(crate) fn intent_markers(
    mut commands: Commands,
    settings: Res<NpcSettings>,
    mut owners: Query<
        (Entity, &Transform, &NpcBrain, &NpcVitals, &mut StandInState),
        Without<IntentMarker>,
    >,
    mut markers: Query<(Entity, &IntentMarker, &mut Transform, &mut MeshRenderer)>,
) {
    if !settings.show_intent {
        for (e, ..) in &markers {
            commands.entity(e).despawn();
        }
        for (.., mut st) in &mut owners {
            st.markers = None;
        }
        return;
    }
    for (e, _, _, _, mut st) in &mut owners {
        if st.markers.is_none() {
            let ring = commands
                .spawn((
                    IntentMarker {
                        owner: e,
                        line: false,
                    },
                    Transform::default(),
                    MeshRenderer::new(
                        Shape::Torus {
                            radius: 0.55,
                            tube: 0.05,
                            segments: 24,
                        },
                        glow(0xffffff, 4.0),
                    )
                    .without_shadows(),
                ))
                .id();
            let line = commands
                .spawn((
                    IntentMarker {
                        owner: e,
                        line: true,
                    },
                    Transform::default(),
                    MeshRenderer::new(
                        Shape::Cuboid {
                            size: [0.05, 0.02, 1.0],
                        },
                        glow(0xffffff, 4.0),
                    )
                    .without_shadows(),
                ))
                .id();
            st.markers = Some([ring, line]);
        }
    }
    for (e, marker, mut t, mut r) in &mut markers {
        let Ok((_, owner_t, brain, vitals, _)) = owners.get(marker.owner) else {
            commands.entity(e).despawn();
            continue;
        };
        let Some(dest) = brain.destination.filter(|_| !vitals.downed) else {
            r.visible = false;
            continue;
        };
        let color = match brain.destination_reason {
            "cover" => 0x3b8cff,
            "flank" => 0xff8a1a,
            "high ground" => 0xb05cff,
            "fall back" => 0x9a9a9a,
            "investigate" | "search" => 0xf2d24c,
            "patrol" => 0x7fd37f,
            _ => 0xffffff,
        };
        let m = engine_render::MaterialRef::Inline(glow(color, 4.0));
        if r.material != m {
            r.material = m;
        }
        let from = owner_t.translation;
        let new = if marker.line {
            let d = dest - from;
            let len = DVec2::new(d.x, d.z).length();
            if len < 1.0 {
                r.visible = false;
                continue;
            }
            let yaw = (-d.x).atan2(-d.z) as f32;
            Transform::from_translation((from + dest) * 0.5 + DVec3::Y * 0.06)
                .with_rotation(Quat::from_rotation_y(yaw))
                .with_scale(Vec3::new(1.0, 1.0, len as f32))
        } else {
            Transform::from_translation(dest + DVec3::Y * 0.06)
        };
        r.visible = true;
        *t = new;
    }
}
