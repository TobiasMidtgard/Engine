//! Animal stand-ins: the parts of deer, wolves, rabbits and birds and how they are posed.

use std::f32::consts::FRAC_PI_2;

use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::locomotion::{LocomotionMode, LocomotionState};

use super::*;
use crate::art::ArtAssets;
use crate::wildlife::{Animal, AnimalState, Species};

/// The parts of an animal's body.
pub(super) fn parts(
    stand_in: &StandIn,
    animal: Option<&Animal>,
    species: Species,
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
    let cone = |radius: f32, height: f32| Shape::Cone {
        radius,
        height,
        segments: 10,
    };
    let cuboid = |x: f32, y: f32, z: f32| Shape::Cuboid { size: [x, y, z] };
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
    parts
        .into_iter()
        .map(|(part, shape, material, scale)| (part, MeshRenderer::new(shape, material), scale))
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn animal_pose(
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
