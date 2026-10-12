//! Humanoid stand-ins: the parts of a fighter and how they are posed from the AI state.

use std::f32::consts::FRAC_PI_2;

use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::locomotion::{LocomotionMode, LocomotionState, MoveIntent};

use super::*;
use crate::agent::{
    AlertLevel, AttackPhase, Defense, NpcBrain, NpcCombat, NpcVitals, NpcWeapon, WeaponStyle,
};
use crate::art::ArtAssets;
use crate::combat::Downed;
use crate::faction::Faction;
use crate::skills::NpcSkills;

/// The parts of a fighter's body.
pub(super) fn parts(
    stand_in: &StandIn,
    weapon: Option<&NpcWeapon>,
    faction: Option<&Faction>,
    skills: Option<&NpcSkills>,
    _art: &mut ArtAssets,
) -> Vec<(Part, MeshRenderer, Vec3)> {
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
    let cuboid = |x: f32, y: f32, z: f32| Shape::Cuboid { size: [x, y, z] };
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
    parts
        .into_iter()
        .map(|(part, shape, material, scale)| (part, MeshRenderer::new(shape, material), scale))
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn humanoid(
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
    // Set only while the weapon glows (heavy telegraph, parry flash).
    let mut weapon_mat: Option<StandardMaterial> = None;
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
                        weapon_mat = Some(glow(0xff7a1a, 30.0));
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
                    weapon_mat = Some(glow(0xffffff, 25.0));
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
        if hurt {
            materials.push((Part::Torso, glow(0xffffff, 6.0)));
        }
    }
    // Muzzle flash right after a shot.
    if ranged && st.was_winding && !winding {
        st.flash_until = now + 0.07;
    }
    st.was_winding = winding;
    let flash = ranged && now < st.flash_until;
    if let Some(m) = weapon_mat {
        materials.push((Part::Weapon, m));
    }
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
pub(super) fn arm_angle(q: Quat) -> f32 {
    let (x, _, _) = q.to_euler(EulerRot::XYZ);
    x
}
