//! Inverse kinematics: two-bone (legs, arms), FABRIK and CCD chains (tails, spider legs,
//! tentacles), look-at, and foot placement on the ground.
//!
//! The solvers work on a [`PoseEdit`] in model space. The components ([`FootIk`], [`LookAt`],
//! [`ReachIk`]) run them every frame after the graph and procedural motion.
//!
//! Ground under a foot comes from three places, highest surface wins: the `Ground` query
//! (terrain), a downward ray through the physics contract (`PhysicsQueries`, when a physics
//! backend is installed), and [`IkSurface`] boxes, ramps and stairs, which let scenes without
//! physics colliders (and animation test stages) still plant feet correctly.

use engine_core::prelude::*;
use engine_world::physics::{PhysicsQueries, QueryFilter};
use engine_world::{Ground, SurfaceId};

use crate::animator::{Animator, PoseEdit};
use crate::xform::{rotation_between, smoothing};

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
    if l1 < 1e-5 || l2 < 1e-5 {
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
        if let Some(s) = self.ground.sample(DVec3::new(p.x, top, p.z)) {
            consider(GroundHit {
                height: s.height,
                normal: s.normal,
                surface: s.surface,
            });
        }
        if let Some(phys) = self.physics {
            let filter = match self.exclude {
                Some(e) => QueryFilter::excluding(e),
                None => QueryFilter::default(),
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
    #[serde(skip)]
    pub pelvis_offset: f32,
    #[serde(skip)]
    pub foot_offsets: Vec<f32>,
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
            pelvis_offset: 0.0,
            foot_offsets: Vec::new(),
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
}

impl ReachIk {
    pub fn new(chains: Vec<Reach>) -> Self {
        Self { chains }
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
    q.par_iter_mut()
        .for_each(|(entity, mut animator, mut ik, gt)| {
            if !ik.enabled || ik.weight <= 0.0 || ik.legs.is_empty() {
                return;
            }
            let probe = GroundProbe {
                ground: &ground,
                physics: physics.as_deref(),
                surfaces: &surfaces.0,
                exclude: Some(entity),
            };
            let dt = animator.dt;
            solve_feet(&mut animator, &mut ik, gt, &probe, dt);
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
    let mut hits = Vec::with_capacity(legs.len());
    let mut min_offset = f32::INFINITY;
    for (n, &(_, _, foot, _, _)) in legs.iter().enumerate() {
        let ankle = animator.model[foot].t;
        // Lift the animation gives this foot above the character's ground plane.
        let lift = (ankle.dot(up_model) - ik.foot_height).max(0.0);
        let world = to_world(gt, ankle);
        let hit = probe.probe(
            world,
            ik.max_step_up + lift * scale,
            ik.max_step_down + lift * scale,
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
        min_offset = min_offset.min(offset);
    }
    if !min_offset.is_finite() {
        min_offset = 0.0;
    }
    // The pelvis goes down to let the lower foot reach (never up: legs can't stretch).
    let target = min_offset.clamp(-ik.max_step_down, 0.0) * ik.weight;
    ik.pelvis_offset += (target - ik.pelvis_offset) * smoothing(dt, ik.pelvis_smoothing);
    let mut edit = animator.edit();
    edit.translate_model(pelvis, up_model * ik.pelvis_offset);
    for (n, &(upper, lower, foot, toe, knee)) in legs.iter().enumerate() {
        let ankle = edit.position(foot);
        let goal = ankle + up_model * (ik.foot_offsets[n] * ik.weight - ik.pelvis_offset);
        // Keep the animated foot rotation while the leg bends.
        let foot_rot = edit.model[foot].r;
        two_bone(&mut edit, upper, lower, foot, goal, knee, 1.0);
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
            let angle = angle.min(ik.max_tilt_deg.to_radians()) * ik.align * planted * ik.weight;
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
                    let angle = angle.min(50f32.to_radians()) * planted * ik.weight;
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
        if !look.enabled || look.weight <= 0.0 {
            continue;
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

pub(crate) fn reach_ik_system(mut q: Query<(&mut Animator, &ReachIk, &GlobalTransform)>) {
    for (mut animator, reach, gt) in &mut q {
        for chain in &reach.chains {
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
            let target = match s.index_of(&chain.target_bone) {
                Some(t) if !chain.target_bone.is_empty() => {
                    animator.model[t].transform_point(chain.target_offset)
                }
                _ => to_model(gt, chain.target),
            };
            two_bone(
                &mut animator.edit(),
                a,
                b,
                c,
                target,
                chain.pole,
                weight.min(1.0),
            );
        }
    }
}

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
        assert!(low.height.abs() < 1e-6);
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
