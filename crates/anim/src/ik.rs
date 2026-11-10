//! Inverse kinematics: two-bone (legs, arms), FABRIK and CCD chains (tails, spider legs,
//! tentacles), look-at, and foot placement on the ground.
//!
//! The solvers work on a [`PoseEdit`] in model space. The components ([`FootIk`], [`LookAt`],
//! [`ReachIk`]) run them every frame after the graph and procedural motion.
//!
//! Ground under a foot comes from three places, highest surface wins: the `Ground` query
//! (terrain), a downward ray through the physics contract (`PhysicsQueries`, when a physics
//! backend is installed; ragdoll parts don't count), and [`IkSurface`] boxes, ramps and
//! stairs, which let scenes without physics colliders (and animation test stages) still plant
//! feet correctly.

use engine_core::prelude::*;
use engine_world::physics::{PhysicsQueries, QueryFilter};
use engine_world::{Ground, SurfaceId};

use crate::animator::{Animator, PoseEdit};
use crate::skeleton::Skeleton;
use crate::xform::{Xf, rotation_between, smoothing, twist_angle};

/// How fast a planted foot's hold takes over, and how long a released one takes to rejoin
/// the animation, s.
const LOCK_IN_SECS: f32 = 0.04;
const LOCK_OUT_SECS: f32 = 0.15;
/// A standing foot steps when the body has turned this far from it, degrees; the step takes
/// `STEP_SECS` and lifts the foot `STEP_HEIGHT` (model units).
const STEP_TURN: f32 = 35.0;
const STEP_SECS: f32 = 0.3;
const STEP_HEIGHT: f32 = 0.07;
/// While a stagger hurries the feet ([`FootIk::hurry`]) a held foot lets go once the body is
/// this share of `lock_reach` away.
const HURRY_REACH: f32 = 0.35;
/// Feet step round only while the body moves slower than this (m/s); a walking foot the body
/// leaves behind just lets go, as the walk lifts it anyway.
const STEP_BELOW_SPEED: f32 = 0.4;
/// A foot coming down only plants once the animation has it (nearly) still on the ground:
/// slower than this plus half the body's speed, m/s (a swinging foot skims low and fast).
const PLANT_SPEED: f32 = 0.5;

// ------------------------------------------------------------------------------- solvers

/// Two-bone IK in model space. Rotates `upper` and `lower` so the end bone's origin reaches
/// `target` (clamped to the chain's reach), bending toward `pole` (a model-space direction
/// the middle joint should point, e.g. forward for knees). Returns how far the end ended up
/// from the target (meters).
pub fn two_bone(
    pose: &mut PoseEdit,
    upper: usize,
    lower: usize,
    end: usize,
    target: Vec3,
    pole: Vec3,
    weight: f32,
) -> f32 {
    if weight <= 0.0 {
        return 0.0;
    }
    let a = pose.position(upper);
    let b = pose.position(lower);
    let c = pose.position(end);
    let target = c.lerp(target, weight.min(1.0));
    let l1 = (b - a).length();
    let l2 = (c - b).length();
    // A broken pose (NaN from upstream) or target is left alone rather than panicking.
    if !(l1 >= 1e-5 && l2 >= 1e-5 && target.is_finite()) {
        return (c - target).length();
    }
    let to_target = target - a;
    let dist = to_target
        .length()
        .clamp((l1 - l2).abs() + 1e-4, l1 + l2 - 1e-4);
    let dir = to_target.normalize_or(c - a);
    // Bend plane: from the pole hint, falling back to the current bend.
    let mut bend = pole - dir * dir.dot(pole);
    if bend.length_squared() < 1e-8 {
        let cur = b - a;
        bend = cur - dir * dir.dot(cur);
    }
    let bend = bend.normalize_or(dir.any_orthonormal_vector());
    let cos_a = ((l1 * l1 + dist * dist - l2 * l2) / (2.0 * l1 * dist)).clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).sqrt();
    let b_goal = a + (dir * cos_a + bend * sin_a) * l1;
    pose.rotate_model(
        upper,
        rotation_between((b - a).normalize(), (b_goal - a).normalize()),
    );
    let b2 = pose.position(lower);
    let c2 = pose.position(end);
    let goal_end = a + dir * dist;
    pose.rotate_model(
        lower,
        rotation_between((c2 - b2).normalize(), (goal_end - b2).normalize_or(c2 - b2)),
    );
    (pose.position(end) - target).length()
}

/// Where the soft reach starts, as a share of the chain's length: a target farther than this
/// is approached ever more slowly, so the limb never snaps straight (Unreal's Basic IK starts
/// stretching at 0.75 of the length; soft IK's falloff the same idea without the stretch).
pub const SOFT_REACH: f32 = 0.97;

/// The most [`two_bone_live`] rolls a hinged limb's upper bone about its length to bend it
/// toward a pole, radians (a shoulder or hip turns about this far either way).
pub const MAX_ROLL: f32 = 100.0 * std::f32::consts::PI / 180.0;

/// How [`two_bone_live`] solves a limb.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limb {
    /// The way the middle joint bends when the animation has the limb straight (or nearly):
    /// a model-space direction, e.g. forward for knees.
    pub pole: Vec3,
    /// 0 bends where the animation bends the middle joint, 1 toward `pole` whatever the
    /// animation does.
    pub pole_weight: f32,
    /// Share of the way to the target, 0..1.
    pub weight: f32,
    /// The end bone's model rotation to put back after the solve (a hand that keeps its grip,
    /// a foot flat on the ground), and how much of it.
    pub end_rotation: Option<(Quat, f32)>,
    /// Approach far targets softly (see [`SOFT_REACH`]).
    pub soft: bool,
    /// The middle joint's hinge axis in the upper bone's own frame (see [`rest_hinge`]): the
    /// upper bone rolls so the limb bends about it, as an elbow or a knee only can. Without
    /// it the animated pose's own hinge is kept (when the limb is bent).
    pub hinge: Option<Vec3>,
}

/// The hinge axis (in `upper`'s own frame) of a limb whose middle joint points `points` in
/// the rest pose (model space: forward for knees, back for elbows): bending turns the lower
/// bone about it by a positive angle. `None` for a degenerate limb.
pub fn rest_hinge(s: &Skeleton, upper: usize, lower: usize, points: Vec3) -> Option<Vec3> {
    let dir = s.rest_model[lower].t - s.rest_model[upper].t;
    let h = points.cross(dir);
    (h.length_squared() > 1e-10).then(|| s.rest_model[upper].r.conjugate() * h.normalize())
}

impl Limb {
    /// Bend as the animation bends, falling back to `pole` for a straight limb.
    pub fn animated(pole: Vec3) -> Self {
        Self {
            pole,
            pole_weight: 0.0,
            weight: 1.0,
            end_rotation: None,
            soft: true,
            hinge: None,
        }
    }

    /// A limb posed exactly where asked (a clip builder's): bent toward `pole`, about its
    /// `hinge`, reaching hard.
    pub fn posed(pole: Vec3, hinge: Option<Vec3>) -> Self {
        Self {
            pole_weight: 1.0,
            soft: false,
            hinge,
            ..Self::animated(pole)
        }
    }
}

/// The distance a two-bone chain of length `reach` actually goes for a target `d` away: `d`
/// up to `SOFT_REACH · reach`, then easing toward `reach` without ever arriving.
pub fn soft_distance(d: f32, reach: f32) -> f32 {
    let start = SOFT_REACH * reach;
    if d <= start {
        return d;
    }
    let room = reach - start;
    start + room * (1.0 - (-(d - start) / room).exp())
}

/// Two-bone IK for a live pose (feet, reaching hands): like [`two_bone`], with what makes a
/// limb read right frame after frame (characters week research, IK 1.2 and 7.1):
/// - the bend comes from where the animation already bends the middle joint (blended toward
///   `limb.pole` by `pole_weight`), so knees and elbows never flip across a fixed pole;
/// - the upper bone keeps its hinge: after the swing it turns about its own length so the
///   middle joint's hinge axis lies across the new bend, the way Unreal's Basic IK aims a
///   secondary axis at the pole (no roll drift for armor plates and twist bones to show);
/// - far targets are reached softly ([`soft_distance`]);
/// - the end bone's rotation can be put back.
///
/// Returns how far the end ended up from the target (meters).
pub fn two_bone_live(
    pose: &mut PoseEdit,
    upper: usize,
    lower: usize,
    end: usize,
    target: Vec3,
    limb: Limb,
) -> f32 {
    if limb.weight <= 0.0 {
        return 0.0;
    }
    let a = pose.position(upper);
    let b = pose.position(lower);
    let c = pose.position(end);
    let end_rotation = limb.end_rotation;
    let target = c.lerp(target, limb.weight.min(1.0));
    let l1 = (b - a).length();
    let l2 = (c - b).length();
    if !(l1 >= 1e-5 && l2 >= 1e-5 && target.is_finite()) {
        return (c - target).length();
    }
    let to_target = target - a;
    let reach = l1 + l2;
    let wanted = to_target.length();
    let d = if limb.soft {
        soft_distance(wanted, reach)
    } else {
        wanted
    };
    let dist = d.clamp((l1 - l2).abs() + 1e-4, reach - 1e-4);
    let dir = to_target.normalize_or(c - a);
    let across = |v: Vec3| v - dir * dir.dot(v);
    // Where the middle joint bends. A limb with a hinge bends about it as the upper bone is
    // rolled now: the animation's roll says where the knee or elbow points (on its hinge, the
    // same as the animated bend; off it, put back onto it), and it stays meaningful when the
    // limb is straight. Without a hinge, the animated bend where there is one, else the pole.
    let animated = across(b - a);
    let hint = across(limb.pole).normalize_or_zero();
    let own = limb
        .hinge
        .map(|h| across(dir.cross(pose.model[upper].r * h)).normalize_or_zero())
        .filter(|v| *v != Vec3::ZERO);
    let pole_weight = limb.pole_weight.clamp(0.0, 1.0);
    let bend = match own {
        // Turned toward the pole about the limb's line, but by no more than the upper bone
        // can roll (`MAX_ROLL`): a pole behind the hinge cannot bend the limb backward.
        Some(own) if hint != Vec3::ZERO => {
            let turn = own.cross(hint).dot(dir).atan2(own.dot(hint));
            let turn = turn.clamp(-MAX_ROLL, MAX_ROLL) * pole_weight;
            Quat::from_axis_angle(dir, turn) * own
        }
        Some(own) => own,
        None => {
            // Trust the animated bend not at all for a straight limb (its direction is
            // noise), fully once clearly bent, eased between so the plane never jumps.
            let t = ((animated.length() / l1 - 0.02) / 0.1).clamp(0.0, 1.0);
            let trust = t * t * (3.0 - 2.0 * t);
            hint.lerp(animated.normalize_or_zero().lerp(hint, pole_weight), trust)
        }
    };
    let bend = if bend.length_squared() < 1e-8 {
        dir.any_orthonormal_vector()
    } else {
        bend.normalize()
    };
    // The hinge the middle joint bends about, where the limb has one. Without it the limb
    // keeps the animated roll: the swing carries the animated hinge along already, and
    // rolling to it again would only add noise.
    let hinge_before = limb
        .hinge
        .map_or(Vec3::ZERO, |h| pose.model[upper].r * h * (l1 * l2));
    let cos_a = ((l1 * l1 + dist * dist - l2 * l2) / (2.0 * l1 * dist)).clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).sqrt();
    let b_goal = a + (dir * cos_a + bend * sin_a) * l1;
    let swing = rotation_between((b - a).normalize(), (b_goal - a).normalize());
    pose.rotate_model(upper, swing);
    let goal_end = a + dir * dist;
    // Keep the hinge: turn the upper bone about its length so the middle joint's hinge
    // axis (carried by the swing) lies across the new bend plane.
    let upper_axis = (b_goal - a).normalize();
    let hinge_goal = upper_axis.cross(goal_end - b_goal);
    if hinge_before.length() > 1e-3 * l1 * l2 && hinge_goal.length() > 1e-3 * l1 * l2 {
        let carried = swing * hinge_before.normalize();
        let along = |v: Vec3| (v - upper_axis * upper_axis.dot(v)).normalize_or_zero();
        let (from, to) = (along(carried), along(hinge_goal.normalize()));
        if from != Vec3::ZERO && to != Vec3::ZERO {
            let angle = from.cross(to).dot(upper_axis).atan2(from.dot(to));
            // A nearly straight limb has hardly a bend plane to roll to (and its direction is
            // noise): roll in only as it bends, from 6° to 20° at the middle joint.
            let flex = (upper_axis.dot((goal_end - b_goal).normalize_or(upper_axis)))
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees();
            let k = ((flex - 6.0) / 14.0).clamp(0.0, 1.0);
            // The bend is kept within `MAX_ROLL` of the hinge above; should the swing still
            // leave nearly a half turn to roll, which way round would flip with noise, so
            // fade the roll out from 120° to none at 170° instead.
            let k = k * ((170.0 - angle.abs().to_degrees()) / 50.0).clamp(0.0, 1.0);
            pose.rotate_model(upper, Quat::from_axis_angle(upper_axis, angle * k));
        }
    }
    let b2 = pose.position(lower);
    let c2 = pose.position(end);
    pose.rotate_model(
        lower,
        rotation_between((c2 - b2).normalize(), (goal_end - b2).normalize_or(c2 - b2)),
    );
    if let Some((r, w)) = end_rotation
        && w > 0.0
    {
        let now = pose.model[end].r;
        pose.set_model_rotation(end, now.slerp(r, w.min(1.0)));
    }
    (pose.position(end) - target).length()
}

/// FABRIK for a chain of bones (`chain[0]` is the root, the last entry the end effector).
/// `iterations` of 4–10 are typical. Returns the remaining distance to the target.
pub fn fabrik(
    pose: &mut PoseEdit,
    chain: &[usize],
    target: Vec3,
    pole: Option<Vec3>,
    iterations: usize,
    tolerance: f32,
) -> f32 {
    let n = chain.len();
    if n < 2 {
        return 0.0;
    }
    let mut p: Vec<Vec3> = chain.iter().map(|&b| pose.position(b)).collect();
    let lengths: Vec<f32> = p.windows(2).map(|w| (w[1] - w[0]).length()).collect();
    let total: f32 = lengths.iter().sum();
    let root = p[0];
    if (target - root).length() >= total {
        // Out of reach: stretch toward the target.
        let dir = (target - root).normalize_or(Vec3::Y);
        for i in 1..n {
            p[i] = p[i - 1] + dir * lengths[i - 1];
        }
    } else {
        // A pole pushes the middle joints sideways first so the chain bends the right way.
        if let Some(pole) = pole {
            for q in p.iter_mut().take(n - 1).skip(1) {
                *q += pole * 0.05 * total;
            }
        }
        for _ in 0..iterations {
            p[n - 1] = target;
            for i in (0..n - 1).rev() {
                let d = (p[i] - p[i + 1]).normalize_or(Vec3::Y);
                p[i] = p[i + 1] + d * lengths[i];
            }
            p[0] = root;
            for i in 1..n {
                let d = (p[i] - p[i - 1]).normalize_or(Vec3::Y);
                p[i] = p[i - 1] + d * lengths[i - 1];
            }
            if (p[n - 1] - target).length() < tolerance {
                break;
            }
        }
    }
    // Turn positions into rotations, root first.
    for i in 0..n - 1 {
        let from = (pose.position(chain[i + 1]) - pose.position(chain[i])).normalize_or(Vec3::Y);
        let to = (p[i + 1] - pose.position(chain[i])).normalize_or(from);
        pose.rotate_model(chain[i], rotation_between(from, to));
    }
    (pose.position(chain[n - 1]) - target).length()
}

/// Cyclic coordinate descent: rotate each joint (end to root) to point the effector at the
/// target, each step limited to `max_step` radians. Good for tails and tentacles curling
/// toward something.
pub fn ccd(
    pose: &mut PoseEdit,
    chain: &[usize],
    target: Vec3,
    iterations: usize,
    max_step: f32,
) -> f32 {
    let n = chain.len();
    if n < 2 {
        return 0.0;
    }
    let effector = chain[n - 1];
    for _ in 0..iterations {
        for i in (0..n - 1).rev() {
            let j = pose.position(chain[i]);
            let e = (pose.position(effector) - j).normalize_or(Vec3::Y);
            let t = (target - j).normalize_or(e);
            let full = rotation_between(e, t);
            let (axis, angle) = full.to_axis_angle();
            let step = Quat::from_axis_angle(axis, angle.min(max_step));
            pose.rotate_model(chain[i], step);
        }
        if (pose.position(effector) - target).length() < 1e-3 {
            break;
        }
    }
    (pose.position(effector) - target).length()
}

/// Turn a chain of bones (spine → head) so `forward` (a model-space axis of the last bone)
/// points at `target`. Each bone takes its share (`shares`, summing to about 1), and the
/// total turn is limited to `max_angle` radians.
pub fn look_at(
    pose: &mut PoseEdit,
    chain: &[(usize, f32)],
    forward_local: Vec3,
    target: Vec3,
    max_angle: f32,
    weight: f32,
) {
    let Some(&(head, _)) = chain.last() else {
        return;
    };
    let eye = pose.position(head);
    let fwd = (pose.model[head].r * forward_local).normalize_or(Vec3::NEG_Z);
    let want = (target - eye).normalize_or(fwd);
    let full = rotation_between(fwd, want);
    let (axis, angle) = full.to_axis_angle();
    let angle = angle.min(max_angle) * weight.clamp(0.0, 1.0);
    if angle < 1e-4 {
        return;
    }
    for &(bone, share) in chain {
        pose.rotate_model(bone, Quat::from_axis_angle(axis, angle * share));
    }
}

// --------------------------------------------------------------------------- ground probe

/// A simple walkable shape for foot placement without physics: a box top, a ramp or a flight
/// of stairs, sized by `half_extents` (meters) and placed by the entity's transform. Ramps
/// and stairs rise toward local −Z, from the bottom at +Z to `2 × half_extents.y` at −Z.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct IkSurface {
    pub shape: IkShape,
    pub half_extents: [f32; 3],
    /// Surface material for footsteps (index into `SurfaceMaterials`).
    pub surface: u16,
}

/// Shape of an [`IkSurface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IkShape {
    #[default]
    Box,
    Ramp,
    Stairs {
        steps: u32,
    },
}

impl Default for IkSurface {
    fn default() -> Self {
        Self {
            shape: IkShape::Box,
            half_extents: [1.0, 0.25, 1.0],
            surface: 12,
        }
    }
}

impl IkSurface {
    /// Top of the surface at local `(x, z)` (local y and normal), if inside the footprint.
    pub fn top(&self, x: f32, z: f32) -> Option<(f32, Vec3)> {
        let [hx, hy, hz] = self.half_extents;
        if x.abs() > hx || z.abs() > hz {
            return None;
        }
        let h = 2.0 * hy;
        // 0 at the +Z edge, 1 at the −Z edge.
        let u = ((hz - z) / (2.0 * hz)).clamp(0.0, 1.0);
        Some(match self.shape {
            IkShape::Box => (hy, Vec3::Y),
            IkShape::Ramp => {
                let slope = h / (2.0 * hz);
                (-hy + h * u, Vec3::new(0.0, 1.0, slope).normalize())
            }
            IkShape::Stairs { steps } => {
                let steps = steps.max(1) as f32;
                let k = (u * steps).floor().min(steps - 1.0) + 1.0;
                (-hy + h * k / steps, Vec3::Y)
            }
        })
    }
}

/// What a probe found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundHit {
    pub height: f64,
    pub normal: Vec3,
    pub surface: SurfaceId,
}

/// Finds the ground under a point from terrain, physics and [`IkSurface`]s.
pub struct GroundProbe<'a> {
    pub ground: &'a Ground,
    pub physics: Option<&'a PhysicsQueries>,
    pub surfaces: &'a [(GlobalTransform, IkSurface)],
    pub exclude: Option<Entity>,
}

impl GroundProbe<'_> {
    /// Highest surface under `p` that is at most `above` meters above it (so feet can step up
    /// onto things) and at most `below` meters below it.
    pub fn probe(&self, p: DVec3, above: f32, below: f32) -> Option<GroundHit> {
        self.find(p, above, below, false, None)
    }

    /// [`probe`](Self::probe) for a point that moves little from one call to the next (a
    /// foot): the terrain's answer is kept in `memo` and, while the point stays within
    /// [`GroundMemo::REACH`], extended along its plane instead of asked again. Physics and
    /// [`IkSurface`]s are still asked every time, since they move.
    pub fn probe_near(
        &self,
        p: DVec3,
        above: f32,
        below: f32,
        memo: &mut GroundMemo,
    ) -> Option<GroundHit> {
        self.find(p, above, below, false, Some(memo))
    }

    /// Only the height of [`probe`](Self::probe), for walkers that follow the ground: the
    /// terrain answers it without its normal and surface lookups (on the showcase's painted
    /// ground those were most of a probe's cost). Equal to `probe(..)?.height`.
    pub fn height(&self, p: DVec3, above: f32, below: f32) -> Option<f64> {
        self.find(p, above, below, true, None).map(|h| h.height)
    }

    fn find(
        &self,
        p: DVec3,
        above: f32,
        below: f32,
        height_only: bool,
        memo: Option<&mut GroundMemo>,
    ) -> Option<GroundHit> {
        let top = p.y + above as f64;
        let bottom = p.y - below as f64;
        let mut best: Option<GroundHit> = None;
        let mut consider = |h: GroundHit| {
            if h.height <= top + 1e-6
                && h.height >= bottom
                && best.is_none_or(|b| h.height > b.height)
            {
                best = Some(h);
            }
        };
        let at = DVec3::new(p.x, top, p.z);
        if height_only {
            if let Some(height) = self.ground.height(at) {
                consider(GroundHit {
                    height,
                    normal: Vec3::Y,
                    surface: SurfaceId::default(),
                });
            }
        } else {
            let ask = || {
                self.ground.sample(at).map(|s| GroundHit {
                    height: s.height,
                    normal: s.normal,
                    surface: s.surface,
                })
            };
            let terrain = match memo {
                Some(m) => m.answer(at, ask),
                None => ask(),
            };
            if let Some(h) = terrain {
                consider(h);
            }
        }
        if let Some(phys) = self.physics {
            // Ragdoll parts aren't ground: a body's own limbs would lift its feet.
            let filter = QueryFilter {
                layers: !crate::ragdoll::RAGDOLL_LAYER,
                exclude: self.exclude.into_iter().collect(),
                include_sensors: false,
            };
            if let Some(hit) = phys.raycast(
                DVec3::new(p.x, top, p.z),
                Vec3::NEG_Y,
                above + below,
                &filter,
            ) {
                consider(GroundHit {
                    height: hit.point.y,
                    normal: hit.normal,
                    surface: hit.surface,
                });
            }
        }
        for (gt, surface) in self.surfaces {
            let local = gt.inverse().transform_point3(p);
            if let Some((y, n)) = surface.top(local.x as f32, local.z as f32) {
                let world = gt.transform_point(DVec3::new(local.x, y as f64, local.z));
                consider(GroundHit {
                    height: world.y,
                    normal: (gt.rotation() * n).normalize(),
                    surface: SurfaceId(surface.surface),
                });
            }
        }
        best
    }
}

/// Gathers the scene's [`IkSurface`]s once per frame for probes.
#[derive(Resource, Default)]
pub struct IkSurfaces(pub Vec<(GlobalTransform, IkSurface)>);

pub(crate) fn gather_surfaces(
    mut out: ResMut<IkSurfaces>,
    q: Query<(&GlobalTransform, &IkSurface)>,
) {
    out.0.clear();
    out.0.extend(q.iter().map(|(g, s)| (*g, *s)));
}

// --------------------------------------------------------------------------- foot placement

/// How far the data under an animator's pose has a foot on the ground (its contact curve over
/// the motion-matching node's share of the pose), when that node has most of the pose.
fn contact_label(animator: &Animator, foot: &str) -> Option<f32> {
    let k = crate::graph::CONTACT_CURVES
        .iter()
        .position(|c| c.strip_prefix("contact.") == Some(foot))?;
    let weight = animator.curve(crate::graph::CONTACT_WEIGHT);
    (weight > 0.5).then(|| animator.curve(crate::graph::CONTACT_CURVES[k]) / weight)
}

/// One leg for [`FootIk`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LegIk {
    pub upper: String,
    pub lower: String,
    /// Ankle bone (its origin is placed `FootIk::foot_height` above the ground).
    pub foot: String,
    /// Toe bone, if any (rotated to follow the ground).
    pub toe: Option<String>,
    /// Model-space direction the knee points (forward for humans, backward for bird legs).
    pub knee_direction: Vec3,
}

impl Default for LegIk {
    fn default() -> Self {
        Self {
            upper: String::new(),
            lower: String::new(),
            foot: String::new(),
            toe: None,
            knee_direction: Vec3::NEG_Z,
        }
    }
}

/// Plants a character's feet on uneven ground: lowers the pelvis so the lower foot reaches,
/// bends each leg with two-bone IK and tilts the feet to the slope. Keeps the animation's own
/// foot lift, so steps still swing.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FootIk {
    pub enabled: bool,
    /// 0..1 blend with the unmodified animation.
    pub weight: f32,
    pub pelvis: String,
    pub legs: Vec<LegIk>,
    /// Height of the ankle above the sole, meters.
    pub foot_height: f32,
    /// How far up a foot may step onto something, meters.
    pub max_step_up: f32,
    /// How far down a foot may reach, meters.
    pub max_step_down: f32,
    /// Pelvis height smoothing time, seconds.
    pub pelvis_smoothing: f32,
    /// 0..1: how much feet tilt to match the ground slope.
    pub align: f32,
    /// Largest foot tilt, degrees.
    pub max_tilt_deg: f32,
    /// Hold planted feet still on the ground (no sliding when the clip's stride doesn't
    /// match the body's motion, or the body turns on the spot).
    pub lock: bool,
    /// Lift above the ground (meters) under which a foot counts as planted.
    pub lock_lift: f32,
    /// Farthest a held foot may lag behind the animation before it lets go, meters.
    pub lock_reach: f32,
    #[serde(skip)]
    pub pelvis_offset: f32,
    #[serde(skip)]
    pub foot_offsets: Vec<f32>,
    #[serde(skip)]
    pub locks: Vec<FootLock>,
    /// Where the body was last frame and how fast it moves over the ground (smoothed), m/s.
    #[serde(skip)]
    pub last_body: Option<DVec3>,
    #[serde(skip)]
    pub body_speed: f32,
    /// Seconds left of a stagger (a blow knocking the body back, see [`HitReaction`]): feet
    /// step even while the body moves fast, as soon as it has moved a little from them.
    ///
    /// [`HitReaction`]: crate::hit::HitReaction
    #[serde(skip)]
    pub hurry: f32,
    /// The leg that steps first while hurried.
    #[serde(skip)]
    pub lead: Option<usize>,
    /// While hurried: how much farther the body is still going (world, along the ground, m).
    /// Feet step to where the animation will have them once it gets there (the capture
    /// point), so two steps catch it rather than a shuffle chasing it.
    #[serde(skip)]
    pub ahead: Vec3,
}

/// The terrain's answer under one foot from an earlier frame (see
/// [`GroundProbe::probe_near`]). On the showcase's painted ground a terrain sample (three
/// heights and the paint's surface) was most of foot IK's cost, asked for every foot of
/// every character each frame, while a standing foot asks about the same spot.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GroundMemo {
    /// Where the terrain was asked (x, top of the probe, z), world.
    at: Option<DVec3>,
    hit: Option<GroundHit>,
}

impl GroundMemo {
    /// How far (meters, along the ground) a probe may move from where the terrain was asked
    /// and still use its answer, extended along the ground's plane there.
    pub const REACH: f64 = 0.1;
    /// How far the probe's top may move up or down (a cave's floor and roof are meters apart).
    pub const RISE: f64 = 0.3;

    /// The terrain's answer at `at`: the kept one extended along its plane while `at` is
    /// near where it was asked, else `ask()`'s (kept for next time).
    fn answer(&mut self, at: DVec3, ask: impl FnOnce() -> Option<GroundHit>) -> Option<GroundHit> {
        if let Some(was) = self.at {
            let d = at - was;
            if d.x * d.x + d.z * d.z <= Self::REACH * Self::REACH && d.y.abs() <= Self::RISE {
                return self.hit.map(|h| {
                    let n = h.normal.as_dvec3();
                    let rise = if n.y > 0.2 {
                        -(n.x * d.x + n.z * d.z) / n.y
                    } else {
                        0.0
                    };
                    GroundHit {
                        height: h.height + rise,
                        ..h
                    }
                });
            }
        }
        let hit = ask();
        *self = Self { at: Some(at), hit };
        hit
    }
}

/// Where a planted foot is held (world space), and how strongly.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FootLock {
    /// The held ankle position; kept while the hold fades after letting go.
    pub at: Option<DVec3>,
    /// The held foot's rotation (world; its heading is kept).
    pub rot: Quat,
    /// Whether the foot is held now.
    pub held: bool,
    /// Whether it let go to step (the body turned or moved away from it while it stood),
    /// rather than lifting with the animation: it then swings over in a small arc.
    pub step: bool,
    /// 0 free .. 1 fully held.
    pub blend: f32,
    /// Where the animation had the ankle last frame (world), for its speed over the ground.
    pub last: Option<DVec3>,
    /// The terrain under this foot, kept between frames.
    pub ground: GroundMemo,
}

impl Default for FootIk {
    fn default() -> Self {
        Self {
            enabled: true,
            weight: 1.0,
            pelvis: "hips".into(),
            legs: Vec::new(),
            foot_height: 0.08,
            max_step_up: 0.5,
            max_step_down: 0.6,
            pelvis_smoothing: 0.08,
            align: 1.0,
            max_tilt_deg: 35.0,
            lock: true,
            lock_lift: 0.02,
            lock_reach: 0.18,
            pelvis_offset: 0.0,
            foot_offsets: Vec::new(),
            locks: Vec::new(),
            last_body: None,
            body_speed: 0.0,
            hurry: 0.0,
            lead: None,
            ahead: Vec3::ZERO,
        }
    }
}

/// Turn the head (and some of the spine) toward a point or an entity.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LookAt {
    pub enabled: bool,
    /// World point to look at (used when `entity` is `None` or missing).
    pub target: DVec3,
    #[serde(skip)]
    pub entity: Option<Entity>,
    /// Bones from the lower spine to the head, with their share of the turn.
    pub chain: Vec<(String, f32)>,
    /// The head bone's local forward axis.
    pub forward: Vec3,
    pub max_angle_deg: f32,
    pub weight: f32,
    /// Smoothing of the look direction, seconds.
    pub smoothing: f32,
    #[serde(skip)]
    pub current: Option<Vec3>,
}

impl Default for LookAt {
    fn default() -> Self {
        Self {
            enabled: true,
            target: DVec3::ZERO,
            entity: None,
            chain: vec![
                ("spine".into(), 0.15),
                ("chest".into(), 0.2),
                ("neck".into(), 0.3),
                ("head".into(), 0.35),
            ],
            forward: Vec3::NEG_Z,
            max_angle_deg: 80.0,
            weight: 1.0,
            smoothing: 0.15,
            current: None,
        }
    }
}

/// Reach hands (or any chain ends) to targets with two-bone IK, e.g. a hand on a weapon
/// grip, a door handle or a ledge.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReachIk {
    pub chains: Vec<Reach>,
    /// How far each fully weighted chain reaching for another bone (an off hand on a
    /// two-handed grip) ended from its seat this frame: meters, and degrees turned off it
    /// when it turns to it. For the pose validator.
    #[serde(skip)]
    pub misses: Vec<(f32, f32)>,
    /// Per chain, for a fist kept turned round its grip as the clip has it: how far its elbow
    /// is turned round the shoulder–fist line from where the clip bends it, and the fist round
    /// the grip from the clip's turn (degrees), last frame: where this frame's search for the
    /// least strained wrist starts.
    #[serde(skip)]
    pub eased: Vec<(f32, f32)>,
}

impl ReachIk {
    pub fn new(chains: Vec<Reach>) -> Self {
        Self {
            chains,
            misses: Vec::new(),
            eased: Vec::new(),
        }
    }
}

/// One reaching chain of a [`ReachIk`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reach {
    pub enabled: bool,
    pub upper: String,
    pub lower: String,
    pub end: String,
    /// World-space target, used when `target_bone` is empty.
    pub target: DVec3,
    /// Reach for a point on another bone of the same skeleton instead (the off hand on a
    /// two-handed grip), `target_offset` in that bone's frame.
    pub target_bone: String,
    pub target_offset: Vec3,
    /// Model-space direction the elbow points.
    pub pole: Vec3,
    pub weight: f32,
    /// Multiply the weight by this animation curve (empty = always on), so clips decide when
    /// the hand grips.
    pub weight_curve: String,
    /// Also turn the end bone to the target bone's frame (times the weight), as an off hand
    /// on a grip held the same way round as the main hand: 0 moves it only.
    pub turn: f32,
    /// The target is one hand's fist on an item the other (`target_bone`) holds:
    /// `target_offset` is where this fist is in the holding hand's standard fist frame, less
    /// [`GRIP`](crate::grip::GRIP), and both hands' measured fists are used
    /// ([`crate::grip`]).
    pub fist: bool,
    /// With `fist` and `turn`: keep the animation's turn of this fist round the handle (about
    /// its axis, from the holding hand's way round), so an off hand the clip wraps round the
    /// grip from its own side stays so; only its place on the grip is fixed.
    pub keep_spin: bool,
}

impl Default for Reach {
    fn default() -> Self {
        Self {
            enabled: true,
            upper: "upper_arm_r".into(),
            lower: "lower_arm_r".into(),
            end: "hand_r".into(),
            target: DVec3::ZERO,
            target_bone: String::new(),
            target_offset: Vec3::ZERO,
            pole: Vec3::new(0.0, -0.3, 1.0),
            weight: 1.0,
            weight_curve: String::new(),
            turn: 0.0,
            fist: false,
            keep_spin: false,
        }
    }
}

impl Reach {
    /// The humanoid's left (`side` 0) or right (1) arm.
    pub fn arm(side: usize) -> Self {
        let s = ["l", "r"][side.min(1)];
        Self {
            upper: format!("upper_arm_{s}"),
            lower: format!("lower_arm_{s}"),
            end: format!("hand_{s}"),
            ..Default::default()
        }
    }
}

/// World → model space for an entity.
pub fn to_model(gt: &GlobalTransform, world: DVec3) -> Vec3 {
    let rel = (world - gt.translation()).as_vec3();
    (gt.rotation().conjugate() * rel) / gt.scale()
}

/// Model → world space for an entity.
pub fn to_world(gt: &GlobalTransform, model: Vec3) -> DVec3 {
    gt.translation() + (gt.rotation() * (model * gt.scale())).as_dvec3()
}

#[allow(clippy::type_complexity)]
pub(crate) fn foot_ik_system(
    ground: Res<Ground>,
    physics: Option<Res<PhysicsQueries>>,
    surfaces: Res<IkSurfaces>,
    mut q: Query<(Entity, &mut Animator, &mut FootIk, &GlobalTransform)>,
) {
    // A few items a piece on the leaf pool (`par_chunks_mut`): a par_iter's compute-pool
    // scope ran whole other systems on its waiting thread (lead, perf week).
    let mut items: Vec<_> = q.iter_mut().map(Some).collect();
    let per = items.len().div_ceil(16).max(1);
    // New ground (an edit dug or built, terrains came or went): no foot keeps its old answer.
    let new_ground = ground.is_changed();
    engine_core::tasks::par_chunks_mut(&mut items, per, |_, chunk| {
        for slot in chunk {
            let Some((entity, mut animator, mut ik, gt)) = slot.take() else {
                continue;
            };
            #[allow(clippy::redundant_closure_call)]
            (|| {
                if !ik.enabled || ik.weight <= 0.0 || ik.legs.is_empty() || !animator.posed() {
                    return;
                }
                if new_ground {
                    for l in &mut ik.locks {
                        l.ground = GroundMemo::default();
                    }
                }
                if animator.fresh() {
                    // Planted feet were left where the body stood when it came to rest.
                    ik.locks.clear();
                    ik.last_body = None;
                }
                let probe = GroundProbe {
                    ground: &ground,
                    physics: physics.as_deref(),
                    surfaces: &surfaces.0,
                    exclude: Some(entity),
                };
                let dt = animator.dt;
                solve_feet(&mut animator, &mut ik, gt, &probe, dt);
            })();
        }
    });
}

/// Foot placement for one character (also usable directly by custom rigs).
pub fn solve_feet(
    animator: &mut Animator,
    ik: &mut FootIk,
    gt: &GlobalTransform,
    probe: &GroundProbe,
    dt: f32,
) {
    let skeleton = animator.skeleton.clone();
    let Some(pelvis) = skeleton.index_of(&ik.pelvis) else {
        return;
    };
    // A body on the floor (the `floor` curve of a get-up) isn't standing on its feet.
    let weight = ik.weight * (1.0 - animator.curve("floor")).clamp(0.0, 1.0);
    let legs: Vec<(usize, usize, usize, Option<usize>, Vec3)> = ik
        .legs
        .iter()
        .filter_map(|l| {
            Some((
                skeleton.index_of(&l.upper)?,
                skeleton.index_of(&l.lower)?,
                skeleton.index_of(&l.foot)?,
                l.toe.as_deref().and_then(|t| skeleton.index_of(t)),
                l.knee_direction,
            ))
        })
        .collect();
    animator.refresh_model();
    let scale = gt.scale().y.max(1e-3);
    let up_model = (gt.rotation().conjugate() * Vec3::Y).normalize();
    // Per foot: how far (model units along up) the ankle must move to sit on the ground.
    ik.foot_offsets.resize(legs.len(), 0.0);
    ik.locks.resize(legs.len(), FootLock::default());
    let can_lock = ik.lock && weight > 0.5 && dt > 0.0;
    let body = gt.translation();
    if dt > 0.0 {
        let moved = ik
            .last_body
            .map_or(0.0, |b| (body - b).with_y(0.0).length()) as f32;
        ik.body_speed += (moved / dt - ik.body_speed) * smoothing(dt, 0.1);
    }
    ik.last_body = Some(body);
    // One foot steps at a time, and only while the body (nearly) stands or staggers.
    let body_speed = ik.body_speed;
    let hurried = ik.hurry > 0.0;
    ik.hurry = (ik.hurry - dt).max(0.0);
    if !hurried {
        ik.lead = None;
        ik.ahead = Vec3::ZERO;
    }
    let ahead = ik.ahead.with_y(0.0).as_dvec3();
    let may_step = body_speed < STEP_BELOW_SPEED || hurried;
    let mut stepping = !may_step || ik.locks.iter().any(|l| !l.held && l.step && l.blend > 0.0);
    let mut hits = Vec::with_capacity(legs.len());
    let mut min_offset = f32::INFINITY;
    for (n, &(_, _, foot, _, _)) in legs.iter().enumerate() {
        let ankle = animator.model[foot].t;
        // Lift the animation gives this foot above the character's ground plane.
        let lift = (ankle.dot(up_model) - ik.foot_height).max(0.0);
        let world = to_world(gt, ankle);
        let hit = probe.probe_near(
            world,
            ik.max_step_up + lift * scale,
            ik.max_step_down + lift * scale,
            &mut ik.locks[n].ground,
        );
        let offset = match hit {
            Some(h) => {
                let want = h.height + ((ik.foot_height + lift) * scale) as f64;
                ((want - world.y) as f32) / scale
            }
            None => 0.0,
        };
        ik.foot_offsets[n] = offset;
        hits.push(hit);
        // A foot that comes down is held where it landed until it lifts again (or the body
        // has moved so far that it must let go), then eased back to the animation.
        // A held foot the body turns or moves away from while it stands steps over to
        // where the animation has it.
        let hurry = if hurried { HURRY_REACH } else { 1.0 };
        let (lock_lift, reach) = (ik.lock_lift, (ik.lock_reach * hurry * scale) as f64);
        // While hurried the lead foot goes first.
        let waits = hurried && ik.lead.is_some_and(|lead| lead != n);
        let rot = gt.rotation() * animator.model[foot].r;
        // A foot is down once it is (nearly) still, or as soon as the animation's data labels
        // it in contact (motion matching), which catches a foot still skimming in.
        let labelled = contact_label(animator, &skeleton.names[foot]);
        let l = &mut ik.locks[n];
        let foot_speed = match (l.last, dt > 0.0) {
            (Some(p), true) => ((p - world).with_y(0.0).length() as f32) / dt,
            _ => 0.0,
        };
        l.last = Some(world);
        let planted =
            foot_speed < PLANT_SPEED + 0.5 * body_speed || labelled.is_some_and(|c| c > 0.5);
        if l.held {
            let drift_from = |p: DVec3| {
                l.at.map_or(f64::INFINITY, |a| {
                    ((a.x - p.x).powi(2) + (a.z - p.z).powi(2)).sqrt()
                })
            };
            // How far the foot is from where it should stand: hurried, where the animation
            // will have it once the body gets where it is going.
            let (drift, wants) = (drift_from(world), drift_from(world + ahead));
            let turned = twist_angle(l.rot * rot.inverse(), Vec3::Y).abs() > STEP_TURN.to_radians();
            if !can_lock || lift > 2.0 * lock_lift {
                l.held = false;
                l.step = false;
            } else if ((wants > reach || turned) && !stepping && !waits)
                || drift > 2.0 * (ik.lock_reach * scale) as f64
            {
                l.held = false;
                l.step = may_step;
                stepping = true;
                if ik.lead == Some(n) {
                    ik.lead = None;
                }
            }
        } else if can_lock && lift < lock_lift && planted && !(l.step && l.blend > 0.0) {
            l.held = true;
            l.step = false;
            l.at = Some(world);
            l.rot = rot;
        }
        l.blend = if l.held {
            (l.blend + dt / LOCK_IN_SECS).min(1.0)
        } else if l.step {
            (l.blend - dt / STEP_SECS).max(0.0)
        } else {
            (l.blend - dt / LOCK_OUT_SECS).max(0.0)
        };
        if hurried && l.step && !l.held && l.blend <= 0.0 && can_lock {
            // A hurried step lands where it was headed, ahead of the animation, and stands
            // there while the body comes over it.
            l.held = true;
            l.step = false;
            l.at = Some(world + ahead);
            l.rot = rot;
            l.blend = 1.0;
        }
        if !l.held && l.blend <= 0.0 {
            l.at = None;
        }
        min_offset = min_offset.min(offset);
    }
    if !min_offset.is_finite() {
        min_offset = 0.0;
    }
    // The pelvis goes down to let the lower foot reach (never up: legs can't stretch).
    let target = min_offset.clamp(-ik.max_step_down, 0.0) * weight;
    ik.pelvis_offset += (target - ik.pelvis_offset) * smoothing(dt, ik.pelvis_smoothing);
    let mut edit = animator.edit();
    edit.translate_model(pelvis, up_model * ik.pelvis_offset);
    for (n, &(upper, lower, foot, toe, knee)) in legs.iter().enumerate() {
        let ankle = edit.position(foot);
        let mut base = ankle;
        // Keep the animated foot rotation while the leg bends (a held foot keeps its own).
        let mut foot_rot = edit.model[foot].r;
        if let Some(l) = ik.locks.get(n).copied()
            && let Some(at) = l.at
            && l.blend > 0.0
        {
            let d = to_model(gt, at) - ankle;
            base += (d - up_model * d.dot(up_model)) * l.blend;
            if l.step && !l.held {
                // Headed for where the body is going.
                let ahead = gt.rotation().conjugate() * ik.ahead / scale;
                base += (ahead - up_model * ahead.dot(up_model)) * (1.0 - l.blend);
            }
            // Only its heading: the animation still rolls it heel to toe.
            let animated = gt.rotation() * foot_rot;
            let yaw = twist_angle(l.rot * animated.inverse(), Vec3::Y);
            foot_rot = (Quat::from_axis_angle(up_model, yaw * l.blend) * foot_rot).normalize();
            if l.step && !l.held {
                base += up_model * STEP_HEIGHT * (std::f32::consts::PI * l.blend).sin();
            }
        }
        let goal = base + up_model * (ik.foot_offsets[n] * weight - ik.pelvis_offset);
        let animated = (edit.model[upper].r, edit.model[lower].r);
        // A planted foot must land exactly on its hold (no soft reach): the pelvis drop
        // above keeps the goal within the leg's length.
        two_bone_live(
            &mut edit,
            upper,
            lower,
            foot,
            goal,
            Limb {
                soft: false,
                // Knees point forward at rest and bend only about that hinge.
                hinge: rest_hinge(&skeleton, upper, lower, Vec3::NEG_Z),
                ..Limb::animated(knee)
            },
        );
        if weight < 1.0 {
            // Fade the knee's re-aim toward `knee` too, not only the goal: at weight 0 the
            // leg keeps its animated bend (knees up while sitting on the floor).
            let solved = (edit.model[upper].r, edit.model[lower].r);
            edit.set_model_rotation(upper, animated.0.slerp(solved.0, weight));
            edit.set_model_rotation(lower, animated.1.slerp(solved.1, weight));
        }
        edit.set_model_rotation(foot, foot_rot);
        // Tilt the foot to the ground, more when planted than when swinging.
        if let Some(h) = hits[n]
            && ik.align > 0.0
        {
            let n_model = (gt.rotation().conjugate() * h.normal).normalize();
            let lift = (ankle.dot(up_model) - ik.foot_height).max(0.0);
            let planted = (1.0 - lift / 0.15).clamp(0.0, 1.0);
            let tilt = rotation_between(up_model, n_model);
            let (axis, angle) = tilt.to_axis_angle();
            let angle = angle.min(ik.max_tilt_deg.to_radians()) * ik.align * planted * weight;
            if angle > 1e-4 {
                edit.rotate_model(foot, Quat::from_axis_angle(axis, angle));
            }
            // A planted toe stays flat on the ground while the heel lifts (toe-off).
            if let Some(t) = toe {
                let fwd = edit.model[t].r * Vec3::NEG_Z;
                let flat = (fwd - n_model * fwd.dot(n_model)).normalize_or_zero();
                if flat != Vec3::ZERO && fwd.dot(n_model) < 0.0 {
                    let bend = rotation_between(fwd, flat);
                    let (axis, angle) = bend.to_axis_angle();
                    let angle = angle.min(50f32.to_radians()) * planted * weight;
                    edit.rotate_model(t, Quat::from_axis_angle(axis, angle));
                }
            }
        }
    }
}

pub(crate) fn look_at_system(
    targets: Query<&GlobalTransform>,
    mut q: Query<(&mut Animator, &mut LookAt, &GlobalTransform)>,
) {
    for (mut animator, mut look, gt) in &mut q {
        if !look.enabled || look.weight <= 0.0 || !animator.posed() {
            continue;
        }
        if animator.fresh() {
            look.current = None;
        }
        let target_world = look
            .entity
            .and_then(|e| targets.get(e).ok())
            .map_or(look.target, |t| t.translation());
        let skeleton = animator.skeleton.clone();
        let chain: Vec<(usize, f32)> = look
            .chain
            .iter()
            .filter_map(|(b, s)| skeleton.index_of(b).map(|i| (i, *s)))
            .collect();
        let Some(&(head, _)) = chain.last() else {
            continue;
        };
        animator.refresh_model();
        let eye = animator.model[head].t;
        let want = (to_model(gt, target_world) - eye).normalize_or(Vec3::NEG_Z);
        let k = smoothing(animator.dt, look.smoothing);
        let dir = match look.current {
            Some(c) => c.lerp(want, k).normalize_or(want),
            None => want,
        };
        look.current = Some(dir);
        let (forward, max, weight) = (look.forward, look.max_angle_deg.to_radians(), look.weight);
        look_at(
            &mut animator.edit(),
            &chain,
            forward,
            eye + dir,
            max,
            weight,
        );
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn reach_ik_system(
    mut q: Query<(
        &mut Animator,
        &mut ReachIk,
        &GlobalTransform,
        Option<&crate::retarget::Retarget>,
        Option<&crate::grip::HeldItems>,
    )>,
) {
    for (mut animator, mut reach, gt, retarget, held) in &mut q {
        let retargeter = retarget.map(|r| &*r.retargeter);
        if !animator.posed() {
            continue;
        }
        let reach = &mut *reach;
        reach.misses.clear();
        for (k, chain) in reach.chains.iter().enumerate() {
            let mut weight = chain.weight;
            if !chain.weight_curve.is_empty() {
                weight *= animator.curve(&chain.weight_curve);
            }
            if !chain.enabled || weight <= 1e-3 {
                continue;
            }
            let s = animator.skeleton.clone();
            let (Some(a), Some(b), Some(c)) = (
                s.index_of(&chain.upper),
                s.index_of(&chain.lower),
                s.index_of(&chain.end),
            ) else {
                continue;
            };
            animator.refresh_model();
            // The end bone's wanted frame (its rotation only matters when it turns).
            let target = match s.index_of(&chain.target_bone) {
                Some(t) if !chain.target_bone.is_empty() => {
                    let offset = Xf::from_t(chain.target_offset);
                    // The fist and the item's seat in it: the reaching hand holds the item
                    // where the target hand's seat puts it.
                    let fit = |bone: &str| match crate::grip::side_of(bone) {
                        Some(side) if chain.fist => {
                            let seat = held
                                .and_then(|h| Some(h.seats[side].xf(h.hands[side].as_ref()?, side)))
                                .unwrap_or(Xf::IDENTITY);
                            crate::grip::fist_correction(retargeter, side).mul(&seat)
                        }
                        _ => Xf::IDENTITY,
                    };
                    let on_grip = animator.model[t].mul(&fit(&chain.target_bone)).mul(&offset);
                    // The animated fist's turn round the handle from the holding hand's way
                    // round, kept (a half turn either way is ambiguous: kept short of it).
                    let fit_end = fit(&chain.end);
                    match crate::grip::side_of(&chain.end)
                        .and_then(|side| Some((side, held?.hands[side]?)))
                    {
                        Some((side, handle)) if chain.fist && chain.keep_spin => {
                            let animated = animator.model[c].mul(&fit_end);
                            let rel = on_grip.inverse().mul(&animated).r;
                            let angle = twist_angle(rel, handle.axis).clamp(-2.6, 2.6);
                            let [a, b] = handle.ends(side);
                            Target::Grip {
                                on_grip,
                                fit_end,
                                mid: (a + b) * 0.5,
                                axis: handle.axis,
                                angle,
                            }
                        }
                        _ => Target::Frame(on_grip.mul(&fit_end.inverse())),
                    }
                }
                _ => Target::Frame(Xf::from_t(to_model(gt, chain.target))),
            };
            let w = weight.min(1.0);
            // A hand on a grip bends its elbow where the clip does (the pole only for a
            // straight arm) and reaches exactly: no soft approach for a seat on a handle.
            let on_bone = !chain.target_bone.is_empty();
            let hinge = rest_hinge(
                &s,
                a,
                b,
                if chain.upper.contains("leg") {
                    Vec3::NEG_Z
                } else {
                    Vec3::Z
                },
            );
            let limb = Limb {
                weight: w,
                soft: !on_bone,
                // The anatomical hinge, not the pole: an elbow points back at rest (a knee,
                // for a leg chain, forward) whichever way this reach wants it to point.
                hinge,
                ..Limb::animated(chain.pole)
            };
            // One way to reach: the elbow turned `swivel` degrees round the shoulder–target
            // line from the clip's bend, the fist `spin` round its grip.
            let reach_with = |edit: &mut PoseEdit, swivel: f32, spin: f32| -> (f32, Xf) {
                let frame = target.frame(spin.to_radians());
                let limb = if !matches!(target, Target::Grip { .. }) {
                    limb
                } else {
                    let shoulder = edit.position(a);
                    let line = (frame.t - shoulder).normalize_or(Vec3::NEG_Y);
                    let bend = edit.position(b) - shoulder;
                    let bend = bend - line * line.dot(bend);
                    Limb {
                        pole: Quat::from_axis_angle(line, swivel.to_radians()) * bend,
                        pole_weight: 1.0,
                        ..limb
                    }
                };
                let miss = two_bone_live(edit, a, b, c, frame.t, limb);
                if chain.turn > 0.0 {
                    let r = edit.model[c].r.slerp(frame.r, (chain.turn * w).min(1.0));
                    edit.set_model_rotation(c, r);
                }
                (miss, frame)
            };
            // A fist the clip turns round its grip may turn its elbow and itself a little
            // further, frame by frame, toward where its wrist strains least.
            let side = crate::grip::side_of(&chain.end);
            let (miss, frame) = match (side, &target) {
                (Some(side), Target::Grip { .. }) if on_bone && chain.upper.contains("arm") => {
                    reach.eased.resize(reach.chains.len(), (0.0, 0.0));
                    let (sw, sp) = reach.eased[k];
                    let mut best: Option<(f32, f32, f32)> = None;
                    let mut local = animator.local.clone();
                    let mut model = animator.model.clone();
                    for swivel in [sw - EASE_STEP, sw, sw + EASE_STEP] {
                        for spin in [sp - EASE_STEP, sp, sp + EASE_STEP] {
                            let (swivel, spin) = (
                                swivel.clamp(-EASE_MAX, EASE_MAX),
                                spin.clamp(-EASE_MAX, EASE_MAX),
                            );
                            local.copy_from_slice(&animator.local);
                            model.copy_from_slice(&animator.model);
                            let mut trial = PoseEdit {
                                skeleton: &s,
                                local: &mut local,
                                model: &mut model,
                            };
                            let (miss, _) = reach_with(&mut trial, swivel, spin);
                            let cost = crate::validate::wrist_strain(&s, &model, b, c, side)
                                + (swivel.abs() + spin.abs()) * EASE_COST
                                + miss * 1000.0;
                            if best.is_none_or(|x| cost < x.0) {
                                best = Some((cost, swivel, spin));
                            }
                        }
                    }
                    let (_, swivel, spin) = best.expect("a reach to try");
                    reach.eased[k] = (swivel, spin);
                    reach_with(&mut animator.edit(), swivel, spin)
                }
                _ => reach_with(&mut animator.edit(), 0.0, 0.0),
            };
            if on_bone && w > 0.99 {
                let turned = if chain.turn > 0.99 {
                    animator.model[c].r.angle_between(frame.r).to_degrees()
                } else {
                    0.0
                };
                reach.misses.push((miss, turned));
            }
        }
    }
}

/// Where a reach goes.
enum Target {
    /// The end bone to this frame.
    Frame(Xf),
    /// A fist onto a grip: the item placement `on_grip` (with `fit_end` the item's seat in
    /// this fist), turned `angle` radians round the grip's line through `mid` along `axis`.
    Grip {
        on_grip: Xf,
        fit_end: Xf,
        mid: Vec3,
        axis: Vec3,
        angle: f32,
    },
}

impl Target {
    /// The end bone's frame, a fist turned `more` radians further round its grip.
    fn frame(&self, more: f32) -> Xf {
        match self {
            Target::Frame(x) => *x,
            Target::Grip {
                on_grip,
                fit_end,
                mid,
                axis,
                angle,
            } => {
                let spin = Xf::from_tr(*mid, Quat::from_axis_angle(*axis, angle + more))
                    .mul(&Xf::from_t(-*mid));
                on_grip.mul(&spin).mul(&fit_end.inverse())
            }
        }
    }
}

/// How far a fist kept on its grip eases its elbow round, or itself round the grip, past
/// where the clip has them in a frame (degrees), at most in all, and what each degree costs
/// against a degree of wrist strain.
const EASE_STEP: f32 = 8.0;
const EASE_MAX: f32 = 90.0;
const EASE_COST: f32 = 0.05;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::{BoneDef, Skeleton};
    use std::sync::Arc;

    fn leg() -> Arc<Skeleton> {
        Arc::new(
            Skeleton::new(
                "leg",
                vec![
                    BoneDef::new("hips", None, Vec3::new(0.0, 1.0, 0.0)),
                    BoneDef::new("thigh", Some("hips"), Vec3::ZERO),
                    BoneDef::new("shin", Some("thigh"), Vec3::new(0.0, -0.5, 0.0)),
                    BoneDef::new("foot", Some("shin"), Vec3::new(0.0, -0.45, 0.0)),
                ],
            )
            .unwrap(),
        )
    }

    #[test]
    fn two_bone_reaches_and_bends_toward_pole() {
        let mut a = Animator::procedural(leg());
        a.refresh_model();
        let target = Vec3::new(0.0, 0.3, -0.1);
        let err = two_bone(&mut a.edit(), 1, 2, 3, target, Vec3::NEG_Z, 1.0);
        assert!(err < 1e-3, "error {err}");
        // Knee went forward (−Z).
        assert!(a.model[2].t.z < -0.05, "knee {:?}", a.model[2].t);
        // Bone lengths are preserved.
        assert!(((a.model[2].t - a.model[1].t).length() - 0.5).abs() < 1e-4);
        assert!(((a.model[3].t - a.model[2].t).length() - 0.45).abs() < 1e-4);
    }

    /// The leg bent by the animation: the knee forward and a little out (+X), as a clip has it.
    fn bent_leg() -> Animator {
        let mut a = Animator::procedural(leg());
        a.refresh_model();
        {
            let mut e = a.edit();
            e.rotate_model(1, Quat::from_rotation_z(0.25) * Quat::from_rotation_x(0.5));
            // The shin bends about the thigh's own X (the knee's hinge).
            let thigh = e.model[1].r;
            e.set_model_rotation(2, thigh * Quat::from_rotation_x(-1.0));
        }
        a.refresh_model();
        a
    }

    #[test]
    fn live_two_bone_keeps_the_animated_bend_and_its_hinge() {
        let mut a = bent_leg();
        let (hip, knee, ankle) = (a.model[1].t, a.model[2].t, a.model[3].t);
        let before = (knee - (hip + ankle) * 0.5).normalize();
        // The ankle pulled up and back a little: a pole straight forward would swing the knee
        // in; the live solve keeps it out where the clip had it.
        let target = ankle + Vec3::new(0.0, 0.12, 0.05);
        let err = two_bone_live(&mut a.edit(), 1, 2, 3, target, Limb::animated(Vec3::NEG_Z));
        assert!(err < 1e-3, "error {err}");
        let (hip, knee, ankle) = (a.model[1].t, a.model[2].t, a.model[3].t);
        let after = (knee - (hip + ankle) * 0.5).normalize();
        assert!(after.dot(before) > 0.97, "{before} -> {after}");
        // The knee is still a hinge: the shin turns about the thigh's own X axis only.
        let local = a.model[1].r.conjugate() * a.model[2].r;
        let (axis, _) = local.to_axis_angle();
        assert!(axis.x.abs() > 0.99, "knee axis {axis}");
        assert!(((knee - hip).length() - 0.5).abs() < 1e-4);
        assert!(((ankle - knee).length() - 0.45).abs() < 1e-4);
    }

    #[test]
    fn live_two_bone_eases_into_full_reach_and_never_snaps_straight() {
        let mut a = bent_leg();
        let hip = a.model[1].t;
        let mut last_bend = f32::MAX;
        // Targets marching out past the leg's reach (0.95 m): the knee straightens smoothly
        // and never locks.
        for k in 0..12 {
            let d = 0.85 + 0.02 * k as f32;
            let target = hip + Vec3::new(0.0, -d, 0.0);
            two_bone_live(&mut a.edit(), 1, 2, 3, target, Limb::animated(Vec3::NEG_Z));
            let (h, kn, an) = (a.model[1].t, a.model[2].t, a.model[3].t);
            let bend = (kn - h).normalize().angle_between((an - kn).normalize());
            assert!(bend > 1e-3, "locked straight at {d}");
            assert!(bend <= last_bend + 1e-4, "bend grew at {d}");
            last_bend = bend;
        }
        assert!(soft_distance(10.0, 1.0) <= 1.0);
        assert!(soft_distance(0.99, 1.0) < 0.99);
        assert_eq!(soft_distance(0.5, 1.0), 0.5);
    }

    #[test]
    fn live_two_bone_puts_the_end_rotation_back() {
        let mut a = bent_leg();
        let keep = a.model[3].r;
        let target = a.model[3].t + Vec3::new(0.05, 0.1, 0.0);
        two_bone_live(
            &mut a.edit(),
            1,
            2,
            3,
            target,
            Limb {
                end_rotation: Some((keep, 1.0)),
                ..Limb::animated(Vec3::NEG_Z)
            },
        );
        assert!(a.model[3].r.angle_between(keep) < 1e-3);
    }

    #[test]
    fn fabrik_and_ccd_reach_targets() {
        let s = Arc::new(
            Skeleton::new(
                "tail",
                (0..6)
                    .map(|i| {
                        let name: &'static str = Box::leak(format!("t{i}").into_boxed_str());
                        let parent: Option<&'static str> = if i == 0 {
                            None
                        } else {
                            Some(Box::leak(format!("t{}", i - 1).into_boxed_str()))
                        };
                        BoneDef::new(
                            name,
                            parent,
                            if i == 0 { Vec3::ZERO } else { Vec3::Z * 0.2 },
                        )
                    })
                    .collect(),
            )
            .unwrap(),
        );
        let chain: Vec<usize> = (0..6).collect();
        let target = Vec3::new(0.4, 0.3, 0.5);
        let mut a = Animator::procedural(s.clone());
        a.refresh_model();
        let e = fabrik(&mut a.edit(), &chain, target, None, 20, 1e-4);
        assert!(e < 0.01, "fabrik error {e}");
        let mut b = Animator::procedural(s);
        b.refresh_model();
        let e = ccd(&mut b.edit(), &chain, target, 20, 0.5);
        assert!(e < 0.02, "ccd error {e}");
    }

    #[test]
    fn look_at_turns_the_head() {
        let s = Arc::new(
            Skeleton::new(
                "h",
                vec![
                    BoneDef::new("neck", None, Vec3::new(0.0, 1.5, 0.0)),
                    BoneDef::new("head", Some("neck"), Vec3::new(0.0, 0.1, 0.0)),
                ],
            )
            .unwrap(),
        );
        let mut a = Animator::procedural(s);
        a.refresh_model();
        let target = Vec3::new(5.0, 1.6, 0.0);
        look_at(
            &mut a.edit(),
            &[(0, 0.5), (1, 0.5)],
            Vec3::NEG_Z,
            target,
            3.0,
            1.0,
        );
        let fwd = a.model[1].r * Vec3::NEG_Z;
        assert!(fwd.dot(Vec3::X) > 0.95, "{fwd:?}");
    }

    #[test]
    fn surfaces_report_ramps_and_stairs() {
        let ramp = IkSurface {
            shape: IkShape::Ramp,
            half_extents: [1.0, 0.5, 2.0],
            surface: 0,
        };
        assert!((ramp.top(0.0, 2.0).unwrap().0 + 0.5).abs() < 1e-5);
        assert!((ramp.top(0.0, -2.0).unwrap().0 - 0.5).abs() < 1e-5);
        // It rises toward −Z, so its normal leans down the slope, toward +Z.
        assert!(ramp.top(0.0, 0.0).unwrap().1.z > 0.0);
        let stairs = IkSurface {
            shape: IkShape::Stairs { steps: 4 },
            ..ramp
        };
        assert!((stairs.top(0.0, 1.9).unwrap().0 - (-0.25)).abs() < 1e-5);
        assert!((stairs.top(0.0, -1.9).unwrap().0 - 0.5).abs() < 1e-5);
        assert!(stairs.top(1.5, 0.0).is_none());
    }

    #[test]
    fn probe_prefers_the_highest_reachable_surface() {
        let ground = Ground::default();
        let surfaces = vec![(
            GlobalTransform::from(Transform::from_xyz(0.0, 0.25, 0.0)),
            IkSurface {
                shape: IkShape::Box,
                half_extents: [1.0, 0.25, 1.0],
                surface: 1,
            },
        )];
        let probe = GroundProbe {
            ground: &ground,
            physics: None,
            surfaces: &surfaces,
            exclude: None,
        };
        let on_box = probe.probe(DVec3::new(0.0, 0.1, 0.0), 0.5, 0.5).unwrap();
        assert!((on_box.height - 0.5).abs() < 1e-6);
        let beside = probe.probe(DVec3::new(3.0, 0.1, 0.0), 0.5, 0.5).unwrap();
        assert!(beside.height.abs() < 1e-6);
        // Too high to step onto: falls back to the ground.
        let low = probe.probe(DVec3::new(0.0, 0.0, 0.0), 0.3, 1.0).unwrap();
        // The height-only probe finds the same heights.
        for (p, above, below) in [
            (DVec3::new(0.0, 0.1, 0.0), 0.5, 0.5),
            (DVec3::new(3.0, 0.1, 0.0), 0.5, 0.5),
            (DVec3::new(0.0, 0.0, 0.0), 0.3, 1.0),
        ] {
            assert_eq!(
                probe.height(p, above, below),
                probe.probe(p, above, below).map(|h| h.height)
            );
        }
        assert!(low.height.abs() < 1e-6);
    }

    #[test]
    fn a_foot_near_where_it_asked_reuses_the_terrain_along_its_plane() {
        // A 1-in-10 slope rising along +x that counts its samples.
        struct Slope(std::sync::atomic::AtomicUsize);
        impl engine_world::GroundQuery for Slope {
            fn sample(&self, p: DVec3) -> Option<engine_world::GroundSample> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some(engine_world::GroundSample {
                    height: 0.1 * p.x,
                    normal: Vec3::new(-0.1, 1.0, 0.0).normalize(),
                    surface: SurfaceId(2),
                })
            }
        }
        let slope = std::sync::Arc::new(Slope(Default::default()));
        let ground = Ground(slope.clone());
        let probe = GroundProbe {
            ground: &ground,
            physics: None,
            surfaces: &[],
            exclude: None,
        };
        let asked = || slope.0.load(std::sync::atomic::Ordering::Relaxed);
        let mut memo = GroundMemo::default();
        let first = probe.probe_near(DVec3::new(1.0, 0.1, 0.0), 0.5, 0.5, &mut memo);
        assert!((first.unwrap().height - 0.1).abs() < 1e-9);
        assert_eq!(asked(), 1);
        // A few centimeters on (a standing foot's sway, a walking foot's frame): the same
        // answer carried up the slope, without asking.
        let near = probe.probe_near(DVec3::new(1.06, 0.12, 0.03), 0.5, 0.5, &mut memo);
        assert!((near.unwrap().height - 0.106).abs() < 1e-6, "{near:?}");
        assert_eq!(near.unwrap().surface, SurfaceId(2));
        assert_eq!(asked(), 1);
        // Farther than the reach, or the probe's top far above: asked again.
        probe.probe_near(DVec3::new(1.3, 0.13, 0.0), 0.5, 0.5, &mut memo);
        assert_eq!(asked(), 2);
        probe.probe_near(DVec3::new(1.3, 1.5, 0.0), 0.5, 2.0, &mut memo);
        assert_eq!(asked(), 3);
        // Range limits still apply to a kept answer: too far below the foot is no ground.
        assert!(
            probe
                .probe_near(DVec3::new(1.3, 1.5, 0.0), 0.5, 0.2, &mut memo)
                .is_none()
        );
        assert_eq!(asked(), 3);
    }

    #[test]
    fn feet_follow_a_raised_step() {
        // A leg standing at the origin with a box 0.2 m tall under it: the ankle rises.
        let skel = leg();
        let mut a = Animator::procedural(skel);
        a.refresh_model();
        let mut ik = FootIk {
            pelvis: "hips".into(),
            legs: vec![LegIk {
                upper: "thigh".into(),
                lower: "shin".into(),
                foot: "foot".into(),
                toe: None,
                knee_direction: Vec3::NEG_Z,
            }],
            foot_height: 0.05,
            pelvis_smoothing: 0.0,
            ..Default::default()
        };
        let ground = Ground::default();
        let surfaces = vec![(
            GlobalTransform::from(Transform::from_xyz(0.0, 0.1, 0.0)),
            IkSurface {
                shape: IkShape::Box,
                half_extents: [0.5, 0.1, 0.5],
                surface: 1,
            },
        )];
        let probe = GroundProbe {
            ground: &ground,
            physics: None,
            surfaces: &surfaces,
            exclude: None,
        };
        let gt = GlobalTransform::default();
        solve_feet(&mut a, &mut ik, &gt, &probe, 0.016);
        let ankle = a.model[3].t;
        assert!((ankle.y - 0.25).abs() < 0.01, "ankle {ankle:?}");
    }
}
