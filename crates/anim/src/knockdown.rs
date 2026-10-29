//! Knockdowns: a blow (or blows) too hard to step out of puts a person on the floor.
//!
//! A pushed body stops in N steps only while its capture point stays inside the region N
//! steps can reach (Koolen, Pratt et al. 2012); past that it falls. The stagger in
//! [`hit`](crate::hit) already glides a struck body to its capture point, so a blow that
//! would carry it farther than [`HitReaction::fall_beyond`] (counting what earlier blows
//! left) knocks it down instead, when a physics backend can catch it.
//!
//! The body goes limp with muscle tone ([`RagdollTone`]) holding a protective shape until
//! it lands: `fall_brace` (arms forward and down) when it falls forward, `fall_back_brace`
//! (chin tucked, hips sitting down, arms back at about 45°, as a judo breakfall) when it
//! falls back or to the side. The blow's push is spread over the parts by their mass, more
//! to those higher up, so the body topples over its feet instead of sliding. Once the hips
//! have lain still for [`HitReaction::down_secs`], the entity is stood where the get-up
//! starts and the ragdoll gets up off its front or back (round 9), as Zordan et al. (2005)
//! hand a simulated body back to captured motion. Blows while it is down push its parts
//! but don't flinch or stagger it, and attackers hold off until it is up.

use engine_core::prelude::*;
use engine_world::animation::{Ragdoll, SkeletonPose};

use crate::animator::Animator;
use crate::hit::HitReaction;
use crate::ragdoll::{PhysicalBlend, RagdollRig, RagdollTone, get_up_placement};

/// Hips moving slower than this count as lying still, m/s.
const STILL_SPEED: f32 = 0.25;
/// Longest a body lies before getting up anyway, s.
const MOST_DOWN_SECS: f32 = 4.0;
/// Longest getting up takes before the knockdown is over anyway, s.
const MOST_RISE_SECS: f32 = 6.0;

/// Where a knocked-down body is in its fall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KnockdownPhase {
    /// Limp, falling and lying on the floor.
    Down,
    /// Getting up.
    Rising,
}

/// A person knocked off their feet (see the module docs). Present until they are up again.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Knockdown {
    pub phase: KnockdownPhase,
    /// Seconds in this phase.
    pub time: f32,
    /// Seconds the hips have lain still.
    pub still: f32,
    /// The blow that did it (world point, N·s), spread over the parts once.
    #[serde(skip)]
    pub blow: Option<(DVec3, Vec3)>,
    /// Where the hips were last frame (world).
    #[serde(skip)]
    hips: Option<DVec3>,
    /// The entity has been stood under the body (a frame before it rises, so the pose it
    /// rises from is where it lies).
    moved: bool,
}

impl Default for Knockdown {
    fn default() -> Self {
        Self {
            phase: KnockdownPhase::Down,
            time: 0.0,
            still: 0.0,
            blow: None,
            hips: None,
            moved: false,
        }
    }
}

impl Knockdown {
    pub fn new(point: DVec3, push: Vec3) -> Self {
        Self {
            blow: Some((point, push)),
            ..Default::default()
        }
    }

    /// Whether the body is still on the floor (not yet getting up).
    pub fn is_down(&self) -> bool {
        self.phase == KnockdownPhase::Down
    }
}

/// The brace for a fall from a push `push` (world) on a body facing `forward` (world):
/// forward when pushed from behind, else back.
pub fn brace_for(push: Vec3, forward: Vec3) -> &'static str {
    if push
        .with_y(0.0)
        .normalize_or_zero()
        .dot(forward.with_y(0.0).normalize_or_zero())
        > 0.3
    {
        "fall_brace"
    } else {
        "fall_back_brace"
    }
}

/// Split `push` among parts at `points` (world, with their masses): by mass, and by height
/// above the lowest part against the hips' (`hips`, world height) from a fifth to twice as
/// much, so the body turns over its feet. Returns each part's share of the push.
pub fn spread_push(points: &[(DVec3, f32)], hips: f64, push: Vec3) -> Vec<(DVec3, Vec3)> {
    let low = points.iter().map(|p| p.0.y).fold(f64::INFINITY, f64::min);
    let span = (hips - low).max(0.1);
    let weight: Vec<f32> = points
        .iter()
        .map(|(p, m)| m * (((p.y - low) / span) as f32).clamp(0.2, 2.0))
        .collect();
    let total: f32 = weight.iter().sum::<f32>().max(1e-6);
    points
        .iter()
        .zip(weight)
        .map(|((p, _), w)| (*p, push * (w / total)))
        .collect()
}

type Downed<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Knockdown,
        &'static mut Transform,
        &'static mut GlobalTransform,
        &'static mut Animator,
        &'static RagdollRig,
        Option<&'static mut Ragdoll>,
        Option<&'static SkeletonPose>,
        Option<&'static mut PhysicalBlend>,
        Option<&'static HitReaction>,
    ),
>;

/// Take knocked-down bodies through their fall: limp and braced, lying still, getting up.
pub(crate) fn knockdown_system(mut commands: Commands, time: Res<Time>, mut q: Downed) {
    let dt = time.delta_secs();
    for (entity, mut k, mut t, mut gt, mut animator, rig, ragdoll, pose, blend, reaction) in &mut q
    {
        let k = &mut *k;
        if let Some((point, push)) = k.blow.take() {
            // Off its feet: limp, toned into the brace for the way it falls.
            let forward = gt.rotation() * Vec3::NEG_Z;
            animator.trigger(brace_for(push, forward));
            commands.entity(entity).insert(RagdollTone::default());
            match ragdoll {
                Some(mut r) => r.weight = 1.0,
                None => {
                    commands.entity(entity).insert(Ragdoll { weight: 1.0 });
                }
            }
            animator.refresh_model();
            let points = rig.def.part_points(&animator.model, &gt);
            let hips = animator
                .skeleton
                .index_of("hips")
                .map_or(gt.translation().y + 1.0, |h| {
                    animator.bone_world(&gt, h).0.y
                });
            let mut pushes = spread_push(&points, hips, push);
            // A little more where it landed, for the turn.
            pushes.push((point, push * 0.15));
            match blend {
                Some(mut b) => b.pushes.extend(pushes),
                None => {
                    commands.entity(entity).insert(PhysicalBlend {
                        pulses: Vec::new(),
                        pushes,
                    });
                }
            }
            k.time = 0.0;
            continue;
        }
        k.time += dt;
        match k.phase {
            KnockdownPhase::Down => {
                let hips = pose.and_then(|p| p.bone_world(&gt, p.index_of("hips")?));
                if let (Some((now, _)), Some(was)) = (hips, k.hips)
                    && dt > 0.0
                {
                    let speed = (now - was).length() as f32 / dt;
                    k.still = if speed < STILL_SPEED {
                        k.still + dt
                    } else {
                        0.0
                    };
                }
                k.hips = hips.map(|h| h.0);
                let lie = reaction.map_or(1.0, |r| r.down_secs);
                let physical = rig.is_physical();
                if (k.still < lie || !physical) && k.time < MOST_DOWN_SECS {
                    continue;
                }
                if !k.moved {
                    // Stand the entity where the get-up starts from the way the body lies,
                    // so it rises there without sliding; or under the hips.
                    let root = gt.translation().distance(t.translation) < 1e-6;
                    let placed = pose.and_then(|p| get_up_placement(&animator.skeleton, p, &gt));
                    if let Some((at, rotation)) = placed {
                        t.translation = at;
                        t.rotation = rotation;
                    } else if let Some((p, _)) = hips {
                        let fell = (p - t.translation).as_vec3();
                        t.translation = DVec3::new(p.x, t.translation.y, p.z);
                        if fell.x.hypot(fell.z) > 0.2 {
                            t.rotation = Quat::from_rotation_y((-fell.x).atan2(-fell.z));
                        }
                    }
                    // The limp pose is worked out from the parts against the entity this frame:
                    // against where it was, it would show a frame displaced and blend back
                    // from there.
                    if root {
                        *gt = GlobalTransform::from(*t);
                    }
                    k.moved = true;
                    // The entity moved under the pose: its hips seem to jump this frame.
                    k.hips = None;
                    continue;
                }
                k.phase = KnockdownPhase::Rising;
                k.time = 0.0;
                if let Some(mut r) = ragdoll {
                    r.weight = 0.0;
                }
            }
            KnockdownPhase::Rising => {
                let up = !rig.is_physical() && !rig.is_blending_back();
                if up || k.time > MOST_RISE_SECS {
                    commands.entity(entity).remove::<(Knockdown, RagdollTone)>();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_push_goes_mostly_to_the_upper_body_and_sums_to_the_blow() {
        let points = [
            (DVec3::new(0.0, 0.1, 0.0), 1.0),
            (DVec3::new(0.0, 0.5, 0.0), 7.0),
            (DVec3::new(0.0, 1.0, 0.0), 11.0),
            (DVec3::new(0.0, 1.4, 0.0), 20.0),
            (DVec3::new(0.0, 1.65, 0.0), 6.0),
        ];
        let push = Vec3::new(0.0, 0.0, 90.0);
        let parts = spread_push(&points, 1.0, push);
        let sum: Vec3 = parts.iter().map(|p| p.1).sum();
        assert!((sum - push).length() < 1e-3, "{sum}");
        // Per kilogram, the chest gets more than the thighs, which get more than the feet.
        let per_kg = |i: usize| parts[i].1.z / points[i].1;
        assert!(per_kg(3) > per_kg(1) && per_kg(1) > per_kg(0));
    }

    #[test]
    fn pushed_from_behind_it_braces_forward() {
        let forward = Vec3::NEG_Z;
        assert_eq!(brace_for(Vec3::new(0.0, 0.0, -50.0), forward), "fall_brace");
        assert_eq!(
            brace_for(Vec3::new(0.0, 0.0, 50.0), forward),
            "fall_back_brace"
        );
        assert_eq!(
            brace_for(Vec3::new(50.0, 0.0, 0.0), forward),
            "fall_back_brace"
        );
    }
}
