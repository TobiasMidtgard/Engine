//! Pulling cloth, ropes and soft volumes by hand, through the engine's use key
//! (`engine_world::interact`): walk up to a curtain, the statue's sheet, a rope or a jelly,
//! press E ("Pull") and the hand takes the nearest bit of it; walk away and it comes along,
//! dragged off the statue, pulled open, torn if it is held hard enough. E again ("Let go")
//! drops it.
//!
//! The hand is a soft goal ([`crate::solver::Goal`]) from the grabbed particle to a frame
//! riding the player, as Unreal's physics handle and Half-Life 2's gravity gun pull a body
//! toward a target through a spring rather than setting its pose: the rest of the body
//! answers through its own constraints, and the pull's reaction reaches the player's body.

use engine_core::prelude::*;
use engine_world::interact::{Interact, Interactable, Using};

use crate::body::{FrameSource, SoftBodySim, SoftKind};
use crate::solver::Goal;

/// How far from a body's surface the hand reaches, m.
pub const REACH_M: f32 = 1.6;
/// How far in front of the player the hand holds what it took, at most, m.
const HOLD_M: f32 = 0.7;
/// Height of the hand above the player's position, m (its feet or its middle, depending on
/// the controller: a hand at the waist reads well either way).
const HAND_UP_M: f32 = 0.9;
/// Stiffness of the hand, m/N: a 1 kg curtain follows it within a few centimeters, and a
/// pinned sheet held at arm's length pulls hard enough to tear.
const HAND_COMPLIANCE: f32 = 2e-4;
/// The hand lets go when what it holds can't follow it this far, m.
const SLIP_M: f32 = 2.5;

/// What the player's prompt says.
const PULL: &str = "Pull";
const LET_GO: &str = "Let go";

/// A body a hand holds: `actor`'s hand holds particle `particle` through solver frame `frame`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Held {
    pub actor: Entity,
    pub particle: u32,
    pub frame: u16,
}

/// The body's solver frame that follows a hand, kept between grabs (frames can't be removed).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct HandFrame(pub u16);

/// Whether a hand can take hold of this body: cloth, ropes and soft volumes simulated on the
/// CPU that don't ride a skeleton (a cape stays on its wearer).
pub fn graspable(b: &SoftBodySim) -> bool {
    matches!(b.kind, SoftKind::Cloth | SoftKind::Rope | SoftKind::Volume)
        && !b.settings.gpu
        && b.bridge.is_none()
        && !b
            .frames
            .iter()
            .any(|f| matches!(f, FrameSource::Bone { .. } | FrameSource::Blend(_)))
}

/// `PostUpdate` (after transforms propagate): offer every graspable body to the use key, its
/// use point at its middle and its reach over its whole extent.
#[allow(clippy::type_complexity)]
pub fn offer_handholds(
    mut commands: Commands,
    mut bodies: Query<(
        Entity,
        &SoftBodySim,
        &GlobalTransform,
        Option<&mut Interactable>,
        Has<Held>,
    )>,
) {
    for (e, b, t, offer, held) in &mut bodies {
        if !graspable(b) {
            continue;
        }
        let offset = t.inverse().transform_point3(b.center).as_vec3();
        let reach = b.radius + REACH_M;
        let verb = if held { LET_GO } else { PULL };
        match offer {
            Some(mut i) => {
                // Only touch it when something changed, so change detection stays useful.
                if i.offset.distance(offset) > 0.05
                    || (i.reach_m - reach).abs() > 0.05
                    || i.verb != verb
                {
                    i.offset = offset;
                    i.reach_m = reach;
                    i.verb = verb.into();
                }
            }
            None => {
                commands
                    .entity(e)
                    .insert(Interactable::new(verb).reach(reach).at(offset));
            }
        }
    }
}

/// `Update`: E on a body takes hold of the bit of it nearest the hand, or lets go of it.
pub fn grab_and_release(
    mut commands: Commands,
    mut presses: MessageReader<Interact>,
    mut bodies: Query<(&mut SoftBodySim, Option<&Held>, Option<&HandFrame>)>,
    actors: Query<&GlobalTransform>,
) {
    for press in presses.read() {
        let Ok((mut b, held, hand)) = bodies.get_mut(press.target) else {
            continue;
        };
        if !graspable(&b) {
            continue;
        }
        if let Some(h) = held {
            release(&mut commands, press.target, &mut b, *h);
            continue;
        }
        let Ok(actor) = actors.get(press.actor) else {
            continue;
        };
        let (pos, rot) = (actor.translation(), actor.rotation());
        let hand_at = pos + DVec3::Y * HAND_UP_M as f64;
        // The nearest particle that isn't pinned.
        let Some(p) = (0..b.sim.len())
            .filter(|&i| b.sim.inv_mass[i] > 0.0)
            .min_by(|&i, &j| {
                let d = |k: usize| (b.world_position(k) - hand_at).length_squared();
                d(i).total_cmp(&d(j))
            })
        else {
            continue;
        };
        // The hand holds it where it is, or closer in if it is out at arm's length.
        let to = (b.world_position(p) - hand_at).as_vec3();
        let grip = hand_at + to.clamp_length_max(HOLD_M).as_dvec3();
        let origin = b.sim.origin;
        let frame_pos = (pos - origin).as_vec3();
        let frame = match hand {
            Some(&HandFrame(f)) if (f as usize) < b.frames.len() => {
                b.frames[f as usize] = FrameSource::Entity(press.actor);
                b.sim.snap_frame(f, frame_pos, rot);
                f
            }
            _ => {
                let f = b.add_frame(FrameSource::Entity(press.actor), frame_pos, rot);
                commands.entity(press.target).insert(HandFrame(f));
                f
            }
        };
        b.sim.goals.push(Goal {
            p: p as u32,
            frame,
            local: rot.inverse() * (grip - pos).as_vec3(),
            compliance: HAND_COMPLIANCE,
        });
        // It can't stay asleep or swaying far off while it is held.
        b.calm = 0;
        crate::sway::stop(&mut b);
        let h = Held {
            actor: press.actor,
            particle: p as u32,
            frame,
        };
        commands.entity(press.target).insert(h);
        commands.entity(press.actor).insert(Using {
            target: press.target,
        });
    }
}

/// `Update`: a hand lets go of what can't follow it (snagged, pinned fast, the player
/// teleported) and of bodies rebuilt or gone.
pub fn slip(
    mut commands: Commands,
    mut bodies: Query<(Entity, &mut SoftBodySim, &Held)>,
    actors: Query<&GlobalTransform>,
) {
    for (e, mut b, h) in &mut bodies {
        let rebuilt = h.frame as usize >= b.frames.len()
            || b.frames[h.frame as usize] != FrameSource::Entity(h.actor)
            || h.particle as usize >= b.sim.len();
        let far = actors.get(h.actor).map_or(true, |t| {
            let hand_at = t.translation() + DVec3::Y * HAND_UP_M as f64;
            (b.world_position(h.particle as usize) - hand_at).length() > (SLIP_M + HOLD_M) as f64
        });
        if rebuilt {
            // A new simulation: nothing of the old hand is in it.
            forget(&mut commands, e, *h);
        } else if far {
            release(&mut commands, e, &mut b, *h);
        }
    }
}

fn release(commands: &mut Commands, e: Entity, b: &mut SoftBodySim, h: Held) {
    b.sim
        .goals
        .retain(|g| !(g.p == h.particle && g.frame == h.frame));
    if let Some(f) = b.frames.get_mut(h.frame as usize)
        && *f == FrameSource::Entity(h.actor)
    {
        *f = FrameSource::World;
    }
    forget(commands, e, h);
}

fn forget(commands: &mut Commands, e: Entity, h: Held) {
    commands.entity(e).remove::<Held>();
    if let Ok(mut actor) = commands.get_entity(h.actor) {
        actor.remove::<Using>();
    }
}
