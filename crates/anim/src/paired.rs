//! Paired moves: a riposte and its victim played as one. When an [`Attacker`] throws a move
//! with a [`Pairing`](crate::combat::Pairing) (after a deflect or a broken guard), its target
//! starts the pairing's clip on the same frame and gets a [`Riposted`]: over the pairing's
//! `align_secs` it is moved onto its spot in front of the attacker and turned to face it, then
//! held there while the two clips play on one clock (the victim's clip time is the pair's
//! clock); its own moves, footwork and knock-back stop. Nobody else's blows touch either of
//! them meanwhile. The victim's clip ends lying on the back on `get_up_back`'s first pose:
//! just before it ends the graph's `get_up_back` is triggered and the clip fades into it, and
//! once the victim is up the `Riposted` comes off.
//!
//! This is the shape of Unreal's Contextual Animation scenes (roles, an alignment onto the
//! primary actor's warp point, one clip per role) and FromSoftware's throws, where the victim
//! is pinned to a point on the attacker and moved onto it over a short lerp.

use engine_core::prelude::*;

use crate::Animator;
use crate::animator::AnimationLibrary;
use crate::combat::Attacker;
use crate::hit::HitReaction;

/// How long before its clip ends the victim's get-up starts, and the clip fades into it, s.
pub const GET_UP_LEAD: f32 = 0.15;

/// A body in a paired move as its victim (published): who has it, its clip, where it is
/// held (`distance` ahead of them, facing back) and how long it took to get there, and
/// whether it is getting up after (for `up_secs`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Riposted {
    pub by: Entity,
    pub clip: String,
    pub distance: f32,
    pub align_secs: f32,
    pub getting_up: bool,
    pub up_secs: f32,
}

impl Default for Riposted {
    fn default() -> Self {
        Self {
            by: Entity::PLACEHOLDER,
            clip: String::new(),
            distance: 1.4,
            align_secs: 0.25,
            getting_up: false,
            up_secs: 0.0,
        }
    }
}

/// Hold victims on their spots and hand them to the get-up (`AnimSet::Setup`, after the
/// attacks).
#[allow(clippy::type_complexity)]
pub(crate) fn hold_pairs(
    time: Res<Time>,
    library: Res<AnimationLibrary>,
    mut commands: Commands,
    mut q: Query<(
        Entity,
        &mut Riposted,
        &mut Transform,
        &mut Animator,
        Option<&mut Attacker>,
        Option<&mut HitReaction>,
        Option<&LocalTimeScale>,
    )>,
    attackers: Query<&GlobalTransform, Without<Riposted>>,
) {
    for (entity, mut r, mut t, mut animator, attacker, reaction, local) in &mut q {
        let dt = if animator.frozen() > 0.0 {
            0.0
        } else {
            time.delta_for(local)
        };
        // Its own fight is over for now.
        if let Some(mut a) = attacker {
            a.current = None;
            a.recoil = 0.0;
            a.open = 0.0;
            a.buffered = None;
            a.follow = None;
        }
        if let Some(mut h) = reaction {
            h.stagger.0 = Vec3::ZERO;
        }
        if r.getting_up {
            r.up_secs += dt;
            let up = library.clip("get_up_back").map_or(0.0, |c| c.duration);
            if r.up_secs >= up {
                commands.entity(entity).remove::<Riposted>();
            }
            continue;
        }
        // The pair's clock: the victim's clip.
        let clip = animator
            .overlay()
            .filter(|o| o.clip.name == r.clip)
            .map(|o| (o.time, o.clip.duration));
        let Some((now, secs)) = clip else {
            // Its clip was cut short: up off the floor.
            animator.trigger("get_up_back");
            r.getting_up = true;
            continue;
        };
        if now >= secs - GET_UP_LEAD {
            animator.trigger("get_up_back");
            r.getting_up = true;
            continue;
        }
        let Ok(by) = attackers.get(r.by) else {
            continue;
        };
        // Onto its spot in front of the attacker, facing it: eased over the alignment, then
        // pinned.
        let forward = (by.rotation() * Vec3::NEG_Z)
            .with_y(0.0)
            .normalize_or_zero();
        let mut spot = by.translation() + (forward * r.distance).as_dvec3();
        spot.y = t.translation.y;
        let share = if now < r.align_secs {
            (dt / (r.align_secs - now + dt).max(1e-3)).min(1.0)
        } else {
            1.0
        };
        t.translation = t.translation.lerp(spot, share as f64);
        let face = -forward;
        let want = Quat::from_rotation_y((-face.x).atan2(-face.z));
        t.rotation = t.rotation.slerp(want, share);
    }
}
