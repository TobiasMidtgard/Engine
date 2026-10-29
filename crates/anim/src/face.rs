//! Living faces for imported characters whose rigs have face joints (Character Studio
//! "Biped" faces, as on the Rocketbox people: eyes, eyelids, jaw, mouth corners, lips, brows
//! and cheeks).
//!
//! A [`Face`] blinks on its own (seeded per entity, so a crowd never blinks together), looks
//! where its [`LookAt`] or [`Face::look_at`] points (the eyes lead the head and stop at their
//! sockets' limits), glances about when it has nothing to look at, and shows what the body
//! is doing: the `effort`, `brace`, `flex` and `strike` curves and the `move_effort`
//! parameter squint the eyes, lower and knit the brows and pull the mouth into a grimace, and
//! a body gone limp (`Ragdoll` weight) lets its lids droop and its jaw hang.
//!
//! It runs after retargeting, on the imported skeleton's pose: [`FaceRig`] finds the face
//! joints and reads their axes from the rest pose (left from eye to eye, ahead from the head
//! to the nose), and moves each joint by a rotation about a pivot (eyes, lids, jaw) or a small
//! offset (mouth, lips, brows, cheeks), so the retargeter stays unchanged. A skeleton without
//! face joints gets no rig and its pose is left as it was.
//!
//! People with an [`Appearance`] (drawn by a character module) get a [`Face`] too, seeded from
//! the appearance: it writes the `FaceControls` contract (ARKit weights and gaze) every frame,
//! with the gaze measured from the humanoid head, for the character module to show.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_world::animation::{AnimationParams, Ragdoll};
use engine_world::character::{self as contract, Appearance, arkit};

use crate::animator::Animator;
use crate::ik::{LookAt, to_model};
use crate::retarget::Retarget;
use crate::skeleton::Skeleton;
use crate::xform::{Xf, smoothing};

/// How far the eyes turn from the head, radians (left and right, up and down).
pub const GAZE_YAW_LIMIT: f32 = 35.0 * std::f32::consts::PI / 180.0;
pub const GAZE_PITCH_LIMIT: f32 = 25.0 * std::f32::consts::PI / 180.0;
/// How long a blink takes, s: closing, then opening.
const BLINK_CLOSE: f32 = 0.06;
const BLINK_OPEN: f32 = 0.11;
/// How far the jaw opens at `jaw` 1, radians.
const JAW_OPEN: f32 = 20.0 * std::f32::consts::PI / 180.0;
/// Distance between the eyes the offsets are written for, m.
const EYE_SPACING: f32 = 0.064;

/// What a face does this frame: the output of [`Face::update`], the input of
/// [`FaceRig::apply`]. Offsets are meters on a face with eyes 6.4 cm apart.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FaceControls {
    /// Eyelids, 0 open .. 1 shut.
    pub lids: f32,
    /// Narrowed eyes (mostly the lower lids rising), 0..1.
    pub squint: f32,
    /// Where the eyes look from the head: left (+) and up (+), radians.
    pub gaze: Vec2,
    /// Jaw, 0 closed .. 1 open.
    pub jaw: f32,
    /// Inner brows up (+) or down (−), m.
    pub brow_inner: f32,
    /// Outer brows up (+) or down (−), m.
    pub brow_outer: f32,
    /// Inner brows drawn toward each other, m.
    pub brow_knit: f32,
    /// Cheeks up, m.
    pub cheeks: f32,
    /// Mouth corners pulled outward, m.
    pub corners_out: f32,
    /// Mouth corners up (+) or down (−), m.
    pub corners_up: f32,
    /// Mouth corners drawn back, m.
    pub corners_back: f32,
    /// Upper lip raised, m.
    pub upper_lip: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Eye(usize),
    LidTop(usize),
    LidBottom(usize),
    Jaw,
    Corner(usize),
    UpperLip(Option<usize>),
    BrowInner(usize),
    BrowOuter(usize),
    BrowMiddle,
    Cheek(usize),
    /// Follows its face parent (tongue, teeth, nose, lower lips).
    Follow,
}

/// Face joint names after the "Bip01 " prefix (any case) and what they do.
const JOINTS: &[(&str, Role)] = &[
    ("leye", Role::Eye(0)),
    ("reye", Role::Eye(1)),
    ("leyeblinktop", Role::LidTop(0)),
    ("reyeblinktop", Role::LidTop(1)),
    ("leyeblinkbottom", Role::LidBottom(0)),
    ("reyeblinkbottom", Role::LidBottom(1)),
    ("mjaw", Role::Jaw),
    ("lmouthcorner", Role::Corner(0)),
    ("rmouthcorner", Role::Corner(1)),
    ("lupperlip", Role::UpperLip(Some(0))),
    ("rupperlip", Role::UpperLip(Some(1))),
    ("mupperlip", Role::UpperLip(None)),
    ("linnereyebrow", Role::BrowInner(0)),
    ("rinnereyebrow", Role::BrowInner(1)),
    ("loutereyebrow", Role::BrowOuter(0)),
    ("routereyebrow", Role::BrowOuter(1)),
    ("mmiddleeyebrow", Role::BrowMiddle),
    ("lcheek", Role::Cheek(0)),
    ("rcheek", Role::Cheek(1)),
    ("mbottomlip", Role::Follow),
    ("lmouthbottom", Role::Follow),
    ("rmouthbottom", Role::Follow),
    ("mtongue", Role::Follow),
    ("lcaninus", Role::Follow),
    ("rcaninus", Role::Follow),
    ("mnose", Role::Follow),
    ("lmasseter", Role::Follow),
    ("rmasseter", Role::Follow),
];

fn role_of(name: &str) -> Option<Role> {
    let lower = name.to_ascii_lowercase();
    let short = lower.strip_prefix("bip01 ").unwrap_or(&lower);
    JOINTS.iter().find(|(n, _)| *n == short).map(|(_, r)| *r)
}

#[derive(Debug, Clone)]
struct FaceJoint {
    bone: usize,
    role: Role,
    rest: Xf,
    /// Index (into `FaceRig::joints`) of the nearest face joint above this one.
    parent: Option<usize>,
}

/// The face joints of one skeleton and the axes they move along (see the module docs).
#[derive(Debug, Clone)]
pub struct FaceRig {
    head: usize,
    head_rest: Xf,
    joints: Vec<FaceJoint>,
    /// The face's left, up and ahead in the rest pose (model space).
    pub left: Vec3,
    pub up: Vec3,
    pub ahead: Vec3,
    /// Eye centers (left, right) in the rest pose, the lids' pivots.
    eyes: [Option<Vec3>; 2],
    /// Model units per meter of offset (from the eye spacing).
    scale: f32,
    /// How far the lids turn to close, radians (top down, bottom up).
    lid_travel: [f32; 2],
}

impl FaceRig {
    /// The face of `skeleton`: needs a head, both eyes and at least the lids or the jaw.
    pub fn new(skeleton: &Skeleton) -> Option<Self> {
        let head = (0..skeleton.len()).find(|&b| {
            let n = skeleton.names[b].to_ascii_lowercase();
            n.strip_prefix("bip01 ").unwrap_or(&n) == "head"
        })?;
        let below_head = |mut b: usize| {
            while let Some(p) = skeleton.parents[b] {
                if p as usize == head {
                    return true;
                }
                b = p as usize;
            }
            false
        };
        let mut joints: Vec<FaceJoint> = Vec::new();
        for b in 0..skeleton.len() {
            let Some(role) = role_of(&skeleton.names[b]) else {
                continue;
            };
            if !below_head(b) {
                continue;
            }
            let mut parent = None;
            let mut p = skeleton.parents[b];
            while let Some(q) = p {
                if let Some(i) = joints.iter().position(|j| j.bone == q as usize) {
                    parent = Some(i);
                    break;
                }
                p = skeleton.parents[q as usize];
            }
            joints.push(FaceJoint {
                bone: b,
                role,
                rest: skeleton.rest_model[b],
                parent,
            });
        }
        let at = |role: Role| joints.iter().find(|j| j.role == role).map(|j| j.rest.t);
        let eyes = [at(Role::Eye(0)), at(Role::Eye(1))];
        let (Some(l), Some(r)) = (eyes[0], eyes[1]) else {
            return None;
        };
        let has_lids = at(Role::LidTop(0)).is_some() || at(Role::LidTop(1)).is_some();
        if !has_lids && at(Role::Jaw).is_none() {
            return None;
        }
        let up = Vec3::Y;
        let left = (l - r).reject_from(up).normalize_or(Vec3::NEG_X);
        let ahead = left.cross(up).normalize();
        let scale = (l - r).length() / EYE_SPACING;
        // Lids close by the angle between them about the eye (the upper lid does most of it).
        let elevation = |lid: Option<Vec3>, eye: Vec3| {
            lid.map(|p| {
                let v = p - eye;
                v.dot(up).atan2(v.dot(ahead).max(1e-4))
            })
        };
        let gap = match (
            elevation(at(Role::LidTop(0)), l),
            elevation(at(Role::LidBottom(0)), l),
        ) {
            (Some(top), Some(bottom)) => (top - bottom).clamp(0.3, 2.0),
            _ => 1.2,
        };
        Some(Self {
            head,
            head_rest: skeleton.rest_model[head],
            joints,
            left,
            up,
            ahead,
            eyes,
            scale,
            lid_travel: [gap * 0.75, gap * 0.25],
        })
    }

    /// Head bone of the skeleton this rig was made for.
    pub fn head(&self) -> usize {
        self.head
    }

    /// Angles (left and up, radians, unclamped) from the head's current pose `model` to look
    /// at `target` (model space).
    pub fn gaze_to(&self, model: &[Xf], target: Vec3) -> Vec2 {
        let Some(h) = model.get(self.head) else {
            return Vec2::ZERO;
        };
        let d = h.mul(&self.head_rest.inverse());
        let eyes = match self.eyes {
            [Some(l), Some(r)] => (l + r) * 0.5,
            _ => self.head_rest.t,
        };
        let dir = d.r.inverse() * (target - d.transform_point(eyes));
        let flat = Vec2::new(dir.dot(self.left), dir.dot(self.ahead));
        Vec2::new(flat.x.atan2(flat.y), dir.dot(self.up).atan2(flat.length()))
    }

    /// Move the face joints of `model` (the skeleton's pose, model space, its face joints
    /// still following the head at their rest offsets) as `c` says.
    pub fn apply(&self, c: &FaceControls, model: &mut [Xf]) {
        let Some(h) = model.get(self.head).copied() else {
            return;
        };
        let d = h.mul(&self.head_rest.inverse());
        let right = -self.left;
        let about = |pivot: Vec3, r: Quat| Xf::from_tr(pivot - r * pivot, r);
        let gaze = Vec2::new(
            c.gaze.x.clamp(-GAZE_YAW_LIMIT, GAZE_YAW_LIMIT),
            c.gaze.y.clamp(-GAZE_PITCH_LIMIT, GAZE_PITCH_LIMIT),
        );
        let lids = c.lids.clamp(0.0, 1.0);
        let squint = c.squint.clamp(0.0, 1.0);
        let k = self.scale;
        let mut edits: Vec<Xf> = Vec::with_capacity(self.joints.len());
        for j in &self.joints {
            let side = |s: usize| if s == 0 { 1.0 } else { -1.0 };
            let own = match j.role {
                Role::Eye(_) => about(
                    j.rest.t,
                    Quat::from_axis_angle(self.up, gaze.x) * Quat::from_axis_angle(right, gaze.y),
                ),
                Role::LidTop(s) => {
                    // Follows the eye up and down; closing turns it down over the eye.
                    let angle =
                        0.6 * gaze.y - self.lid_travel[0] * (lids + 0.2 * squint * (1.0 - lids));
                    about(
                        self.eyes[s].unwrap_or(j.rest.t),
                        Quat::from_axis_angle(right, angle),
                    )
                }
                Role::LidBottom(s) => {
                    let angle =
                        0.3 * gaze.y + self.lid_travel[1] * (lids + 0.8 * squint * (1.0 - lids));
                    about(
                        self.eyes[s].unwrap_or(j.rest.t),
                        Quat::from_axis_angle(right, angle),
                    )
                }
                Role::Jaw => about(
                    j.rest.t,
                    Quat::from_axis_angle(right, -JAW_OPEN * c.jaw.clamp(0.0, 1.0)),
                ),
                Role::Corner(s) => Xf::from_t(
                    (self.left * side(s) * c.corners_out + self.up * c.corners_up
                        - self.ahead * c.corners_back)
                        * k,
                ),
                Role::UpperLip(s) => {
                    Xf::from_t(self.up * c.upper_lip * k * if s.is_some() { 1.0 } else { 0.7 })
                }
                Role::BrowInner(s) => {
                    Xf::from_t((self.up * c.brow_inner - self.left * side(s) * c.brow_knit) * k)
                }
                Role::BrowOuter(_) => Xf::from_t(self.up * c.brow_outer * k),
                Role::BrowMiddle => Xf::from_t(self.up * c.brow_inner * 0.8 * k),
                Role::Cheek(_) => Xf::from_t(self.up * c.cheeks * k),
                Role::Follow => Xf::IDENTITY,
            };
            let edit = match j.parent {
                Some(p) => edits[p].mul(&own),
                None => own,
            };
            if let Some(m) = model.get_mut(j.bone) {
                *m = d.mul(&edit.mul(&j.rest));
            }
            edits.push(edit);
        }
    }
}

/// What drives a face this frame (see [`Face::update`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FaceInputs {
    /// How hard the body works, 0..1.
    pub effort: f32,
    /// How limp it is, 0..1.
    pub slack: f32,
    /// Where something to look at is (left and up from the head, radians), if anywhere.
    pub look: Option<Vec2>,
}

#[derive(Debug, Clone, Default)]
struct FaceState {
    rng: u64,
    next_blink: f32,
    blink: Option<f32>,
    glance: Vec2,
    next_glance: f32,
    gaze: Option<Vec2>,
    effort: f32,
    slack: f32,
    rig: Option<(usize, Option<Arc<FaceRig>>)>,
    controls: FaceControls,
}

/// A living face (see the module docs). Added to every imported person with a face rig.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Face {
    pub enabled: bool,
    /// Blinks per minute (people blink 10 to 20 times a minute).
    pub blink_rate: f32,
    /// How strongly effort and limpness show, 1 = normal.
    pub expressiveness: f32,
    /// World point to look at; otherwise the entity's `LookAt` target, otherwise ahead with
    /// glances about.
    pub look_at: Option<DVec3>,
    #[serde(skip)]
    state: FaceState,
}

impl Default for Face {
    fn default() -> Self {
        Self {
            enabled: true,
            blink_rate: 16.0,
            expressiveness: 1.0,
            look_at: None,
            state: FaceState::default(),
        }
    }
}

impl Face {
    /// A face whose blinks and glances follow `seed` (set at spawn: never an entity's bits,
    /// which shift whenever any crate adds a resource or message).
    pub fn seeded(seed: u64) -> Self {
        let mut f = Self::default();
        f.seed(seed);
        f
    }

    fn seed(&mut self, seed: u64) {
        let s = &mut self.state;
        s.rng = splitmix(seed) | 1;
        s.next_blink = 0.3 + 2.5 * unit(&mut s.rng);
        s.next_glance = 0.5 + unit(&mut s.rng);
    }

    /// What the face did last update.
    pub fn controls(&self) -> FaceControls {
        self.state.controls
    }

    /// Advance by `dt` seconds and return what the face does now.
    pub fn update(&mut self, dt: f32, input: FaceInputs) -> FaceControls {
        if self.state.rng == 0 {
            self.seed(0);
        }
        let blink_rate = self.blink_rate;
        let x = self.expressiveness.max(0.0);
        let s = &mut self.state;
        let up = |a: f32, b: f32, tau: f32| a + (b - a) * smoothing(dt, tau);
        s.effort = up(s.effort, input.effort.clamp(0.0, 1.0), 0.12);
        let slack = input.slack.clamp(0.0, 1.0);
        s.slack = up(s.slack, slack, if slack > s.slack { 0.25 } else { 0.5 });
        let (e, l) = (s.effort * x, s.slack);

        // Blinks: a quick close and a slower open, at seeded random intervals. A limp body
        // doesn't blink.
        let mean = 60.0 / blink_rate.max(1.0);
        s.next_blink -= dt;
        if s.blink.is_none() && s.next_blink <= 0.0 && l < 0.5 {
            s.blink = Some(0.0);
            s.next_blink = mean * (0.35 + 1.3 * unit(&mut s.rng));
        }
        let mut blink = 0.0;
        if let Some(t) = s.blink.as_mut() {
            *t += dt;
            blink = if *t < BLINK_CLOSE {
                *t / BLINK_CLOSE
            } else {
                1.0 - (*t - BLINK_CLOSE) / BLINK_OPEN
            };
            if *t >= BLINK_CLOSE + BLINK_OPEN {
                s.blink = None;
                blink = 0.0;
            }
        }
        let blink = blink.clamp(0.0, 1.0);

        // Gaze: at the target with tiny saccades, or ahead with glances about. The eyes jump
        // (a saccade takes a few hundredths of a second); a limp body's eyes roll up a little.
        s.next_glance -= dt;
        if s.next_glance <= 0.0 {
            let (yaw, pitch, wait) = if input.look.is_some() {
                (1.5f32, 1.0f32, 0.4 + 1.2 * unit(&mut s.rng))
            } else {
                (9.0, 4.0, 0.7 + 2.3 * unit(&mut s.rng))
            };
            s.glance = Vec2::new(
                yaw.to_radians() * (2.0 * unit(&mut s.rng) - 1.0),
                pitch.to_radians() * (2.0 * unit(&mut s.rng) - 1.0),
            );
            s.next_glance = wait;
        }
        let want = input.look.unwrap_or(Vec2::ZERO) + s.glance;
        let want = want.lerp(Vec2::new(0.0, 6f32.to_radians()), l);
        let gaze = match s.gaze {
            Some(g) => g.lerp(want, smoothing(dt, 0.03)),
            None => want,
        };
        s.gaze = Some(gaze);

        s.controls = FaceControls {
            lids: blink.max(0.75 * l),
            squint: (0.7 * e).min(1.0) * (1.0 - l),
            gaze,
            jaw: (0.12 * e).max(0.5 * l),
            brow_inner: -0.006 * e * (1.0 - l),
            brow_outer: -0.002 * e * (1.0 - l),
            brow_knit: 0.0035 * e * (1.0 - l),
            cheeks: 0.003 * e * (1.0 - l),
            corners_out: 0.005 * e * (1.0 - l),
            corners_up: -0.001 * e - 0.002 * l,
            corners_back: 0.002 * e * (1.0 - l),
            upper_lip: 0.0025 * e * (1.0 - l),
        };
        s.controls
    }
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Uniform in [0, 1).
fn unit(rng: &mut u64) -> f32 {
    *rng ^= *rng << 13;
    *rng ^= *rng >> 7;
    *rng ^= *rng << 17;
    (*rng >> 40) as f32 / (1u64 << 24) as f32
}

type FaceQuery<'a> = (
    &'a Animator,
    &'a mut Retarget,
    &'a mut Face,
    &'a GlobalTransform,
    Option<&'a Ragdoll>,
    Option<&'a LookAt>,
    Option<&'a AnimationParams>,
    Option<&'a mut contract::FaceControls>,
);

pub(crate) fn face_system(targets: Query<&GlobalTransform>, mut q: Query<FaceQuery>) {
    for (animator, mut retarget, mut face, gt, ragdoll, look, params, out) in &mut q {
        if !face.enabled || !animator.posed() {
            continue;
        }
        let retarget = &mut *retarget;
        let key = Arc::as_ptr(&retarget.retargeter) as usize;
        if face.state.rig.as_ref().is_none_or(|(k, _)| *k != key) {
            let rig = FaceRig::new(&retarget.retargeter.target).map(Arc::new);
            face.state.rig = Some((key, rig));
        }
        let Some(rig) = face.state.rig.as_ref().and_then(|(_, r)| r.clone()) else {
            continue;
        };
        let target = look_target(&face, look, &targets);
        let input = FaceInputs {
            effort: effort(animator, params),
            slack: slack(ragdoll),
            look: target.map(|t| rig.gaze_to(&retarget.model, to_model(gt, t))),
        };
        let controls = face.update(animator.dt, input);
        rig.apply(&controls, &mut retarget.model);
        if let Some(mut out) = out {
            controls.write_arkit(&mut out);
        }
    }
}

/// Where a face should look: its own point, else its `LookAt`'s target.
fn look_target(
    face: &Face,
    look: Option<&LookAt>,
    targets: &Query<&GlobalTransform>,
) -> Option<DVec3> {
    face.look_at.or_else(|| {
        look.filter(|l| l.enabled && l.weight > 0.0).map(|l| {
            l.entity
                .and_then(|e| targets.get(e).ok())
                .map_or(l.target, |t| t.translation())
        })
    })
}

/// How hard the body works (0..1), from the animation's curves and the move effort.
fn effort(animator: &Animator, params: Option<&AnimationParams>) -> f32 {
    animator
        .curve("effort")
        .max(0.8 * animator.curve("brace"))
        .max(0.8 * animator.curve("flex"))
        // A punch thrown: the jaw sets and the eyes narrow as it lands.
        .max(0.7 * animator.curve("strike"))
        .max(0.5 * params.map_or(0.0, |p| p.get("move_effort")))
}

fn slack(ragdoll: Option<&Ragdoll>) -> f32 {
    ragdoll.map_or(0.0, |r| ((r.weight - 0.3) / 0.5).clamp(0.0, 1.0))
}

/// Brow, cheek and mouth offsets (m) that make a full ARKit weight.
const FULL_OFFSET: f32 = 0.006;

impl FaceControls {
    /// The same expression as ARKit blendshape weights and gaze, for a face another module
    /// draws (`Left`/`Right` are the face's own; these faces are symmetric).
    pub fn write_arkit(&self, out: &mut contract::FaceControls) {
        let w = |v: f32| (v / FULL_OFFSET).clamp(0.0, 1.0);
        out.weights.clear();
        out.weights.resize(contract::ARKIT.len(), 0.0);
        let mut both = |l: usize, r: usize, v: f32| {
            out.set_index(l, v);
            out.set_index(r, v);
        };
        both(
            arkit::EYE_BLINK_LEFT,
            arkit::EYE_BLINK_RIGHT,
            self.lids.clamp(0.0, 1.0),
        );
        both(
            arkit::EYE_SQUINT_LEFT,
            arkit::EYE_SQUINT_RIGHT,
            self.squint.clamp(0.0, 1.0),
        );
        both(
            arkit::CHEEK_SQUINT_LEFT,
            arkit::CHEEK_SQUINT_RIGHT,
            w(self.cheeks * 1.5),
        );
        both(
            arkit::BROW_DOWN_LEFT,
            arkit::BROW_DOWN_RIGHT,
            w(-self.brow_inner + self.brow_knit * 0.5),
        );
        both(
            arkit::MOUTH_SMILE_LEFT,
            arkit::MOUTH_SMILE_RIGHT,
            w(self.corners_up * 2.0),
        );
        both(
            arkit::MOUTH_FROWN_LEFT,
            arkit::MOUTH_FROWN_RIGHT,
            w(-self.corners_up * 2.0),
        );
        both(
            arkit::MOUTH_STRETCH_LEFT,
            arkit::MOUTH_STRETCH_RIGHT,
            w(self.corners_out + self.corners_back),
        );
        both(
            arkit::NOSE_SNEER_LEFT,
            arkit::NOSE_SNEER_RIGHT,
            w(self.upper_lip * 1.5),
        );
        out.set_index(arkit::BROW_INNER_UP, w(self.brow_inner));
        out.set_index(arkit::JAW_OPEN, self.jaw.clamp(0.0, 1.0));
        out.gaze_yaw = self.gaze.x.clamp(-GAZE_YAW_LIMIT, GAZE_YAW_LIMIT);
        out.gaze_pitch = self.gaze.y.clamp(-GAZE_PITCH_LIMIT, GAZE_PITCH_LIMIT);
    }
}

/// Where the eyes sit from the humanoid head bone (model axes at rest: ahead −Z), m.
const EYES_FROM_HEAD: Vec3 = Vec3::new(0.0, 0.09, -0.08);

/// Angles (left and up, radians) from the humanoid head in `model` to `target` (model space).
fn humanoid_gaze(model: &[Xf], head: usize, target: Vec3) -> Vec2 {
    let Some(h) = model.get(head) else {
        return Vec2::ZERO;
    };
    let dir = h.r.inverse() * (target - h.transform_point(EYES_FROM_HEAD));
    let flat = Vec2::new(-dir.x, -dir.z);
    Vec2::new(flat.x.atan2(flat.y), dir.y.atan2(flat.length()))
}

/// Give people with an [`Appearance`] a face and the `FaceControls` it writes.
#[allow(clippy::type_complexity)]
pub(crate) fn give_appearance_faces(
    mut commands: Commands,
    q: Query<
        (
            Entity,
            &Appearance,
            Option<&Face>,
            Option<&contract::FaceControls>,
        ),
        (With<Animator>, Changed<Appearance>),
    >,
) {
    for (e, appearance, face, controls) in &q {
        let mut entity = commands.entity(e);
        if face.is_none() {
            entity.insert(Face::seeded(appearance.seed.wrapping_add(1)));
        }
        if controls.is_none() {
            entity.insert(contract::FaceControls::default());
        }
    }
}

type AppearanceFaceQuery<'a> = (
    &'a Animator,
    &'a mut Face,
    &'a mut contract::FaceControls,
    &'a GlobalTransform,
    Option<&'a Ragdoll>,
    Option<&'a LookAt>,
    Option<&'a AnimationParams>,
);

/// Faces another module draws (people with an [`Appearance`] and no imported face rig): run
/// the face and write the `FaceControls` contract.
pub(crate) fn appearance_face_system(
    targets: Query<&GlobalTransform>,
    mut q: Query<AppearanceFaceQuery, (With<Appearance>, Without<Retarget>)>,
) {
    for (animator, mut face, mut out, gt, ragdoll, look, params) in &mut q {
        if !face.enabled || !animator.posed() {
            continue;
        }
        let Some(head) = animator.skeleton.index_of("head") else {
            continue;
        };
        let target = look_target(&face, look, &targets);
        let input = FaceInputs {
            effort: effort(animator, params),
            slack: slack(ragdoll),
            look: target.map(|t| humanoid_gaze(&animator.model, head, to_model(gt, t))),
        };
        let controls = face.update(animator.dt, input);
        controls.write_arkit(&mut out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A head with Biped face joints, facing −Z (its left is −X), eyes 6.4 cm apart.
    fn face_skeleton() -> Skeleton {
        let joints: [(&str, Option<usize>, Vec3); 13] = [
            ("Bip01", None, Vec3::ZERO),
            ("Bip01 Head", Some(0), Vec3::new(0.0, 1.6, 0.0)),
            ("Bip01 LEye", Some(1), Vec3::new(-0.032, 1.67, -0.08)),
            ("Bip01 REye", Some(1), Vec3::new(0.032, 1.67, -0.08)),
            (
                "Bip01 LEyeBlinkTop",
                Some(1),
                Vec3::new(-0.032, 1.678, -0.088),
            ),
            (
                "Bip01 REyeBlinkTop",
                Some(1),
                Vec3::new(0.032, 1.678, -0.088),
            ),
            (
                "Bip01 LEyeBlinkBottom",
                Some(1),
                Vec3::new(-0.032, 1.664, -0.09),
            ),
            (
                "Bip01 REyeBlinkBottom",
                Some(1),
                Vec3::new(0.032, 1.664, -0.09),
            ),
            ("Bip01 MJaw", Some(1), Vec3::new(0.0, 1.6, -0.03)),
            ("Bip01 LMouthCorner", Some(8), Vec3::new(-0.025, 1.59, -0.1)),
            ("Bip01 RMouthCorner", Some(8), Vec3::new(0.025, 1.59, -0.1)),
            (
                "Bip01 LInnerEyebrow",
                Some(1),
                Vec3::new(-0.015, 1.69, -0.095),
            ),
            (
                "Bip01 RInnerEyebrow",
                Some(1),
                Vec3::new(0.015, 1.69, -0.095),
            ),
        ];
        let names = joints.iter().map(|j| j.0.to_string()).collect();
        let parents = joints.iter().map(|j| j.1.map(|p| p as u16)).collect();
        let rest = joints
            .iter()
            .map(|j| Xf::from_t(j.2 - j.1.map_or(Vec3::ZERO, |p| joints[p].2)))
            .collect();
        Skeleton::from_parts("face", names, parents, rest, None)
    }

    fn posed(s: &Skeleton, rig: &FaceRig, c: &FaceControls) -> Vec<Xf> {
        let mut m = s.rest_model.clone();
        rig.apply(c, &mut m);
        m
    }

    #[test]
    fn the_rig_finds_the_face_and_its_axes() {
        let s = face_skeleton();
        let rig = FaceRig::new(&s).expect("a face rig");
        assert!(rig.left.distance(Vec3::NEG_X) < 1e-4, "left {}", rig.left);
        assert!(
            rig.ahead.distance(Vec3::NEG_Z) < 1e-4,
            "ahead {}",
            rig.ahead
        );
        assert!((rig.scale - 1.0).abs() < 1e-4);
        // Neutral controls leave the pose as it was.
        let m = posed(&s, &rig, &FaceControls::default());
        for (x, rest) in m.iter().zip(&s.rest_model) {
            assert!(x.t.distance(rest.t) < 1e-6);
            assert!(x.r.angle_between(rest.r) < 1e-5);
        }
        // Without face joints there is no rig, and nothing to change.
        assert!(FaceRig::new(&crate::build::humanoid::skeleton()).is_none());
    }

    #[test]
    fn faces_blink_shut_and_open_again_at_their_own_times() {
        let blinks = |seed: u64| {
            let mut f = Face::seeded(seed);
            let mut out = Vec::new();
            let mut shut = false;
            for i in 0..(20.0 / 0.01) as usize {
                let c = f.update(0.01, FaceInputs::default());
                if c.lids > 0.95 && !shut {
                    out.push(i as f32 * 0.01);
                }
                shut = c.lids > 0.95;
                assert!(c.lids <= 1.0);
            }
            out
        };
        let a = blinks(1);
        let b = blinks(2);
        assert!((3..12).contains(&a.len()), "{} blinks in 20 s", a.len());
        assert_ne!(a, b, "two faces blink at different times");
        // A blink opens again within a fifth of a second.
        let mut f = Face::seeded(1);
        let t = (a[0] / 0.01) as usize;
        for _ in 0..t + 1 {
            f.update(0.01, FaceInputs::default());
        }
        for _ in 0..20 {
            f.update(0.01, FaceInputs::default());
        }
        assert!(
            f.controls().lids < 0.05,
            "still shut: {}",
            f.controls().lids
        );
    }

    #[test]
    fn the_eyes_look_at_their_target_within_their_sockets() {
        let s = face_skeleton();
        let rig = FaceRig::new(&s).unwrap();
        let eye = s.bone("Bip01 LEye");
        let mid = Vec3::new(0.0, 1.67, -0.08);
        for (deg, expect) in [(20.0f32, 20.0f32), (-15.0, -15.0), (70.0, 35.0)] {
            // A point to the face's left (−X) by `deg`, 2 m away.
            let a = deg.to_radians();
            let target = mid + Vec3::new(-a.sin(), 0.0, -a.cos()) * 2.0;
            let gaze = rig.gaze_to(&s.rest_model, target);
            assert!((gaze.x.to_degrees() - deg).abs() < 0.5, "{deg}: {gaze}");
            let m = posed(
                &s,
                &rig,
                &FaceControls {
                    gaze,
                    ..Default::default()
                },
            );
            let look = m[eye].r * Vec3::NEG_Z;
            let got = (-look.x).atan2(-look.z).to_degrees();
            assert!((got - expect).abs() < 0.5, "{deg}: eye turned {got}");
            assert!(
                m[eye].t.distance(s.rest_model[eye].t) < 1e-6,
                "turns in place"
            );
        }
        // The eyes follow a turned head.
        let mut m = s.rest_model.clone();
        let head = rig.head();
        let turn = Xf::from_tr(Vec3::ZERO, Quat::from_rotation_y(0.5));
        for x in m.iter_mut().skip(head) {
            *x = turn.mul(x);
        }
        let ahead = turn.transform_point(mid + Vec3::NEG_Z * 2.0);
        assert!(rig.gaze_to(&m, ahead).length() < 1e-3);
    }

    #[test]
    fn effort_squints_knits_the_brows_and_pulls_the_mouth() {
        let s = face_skeleton();
        let rig = FaceRig::new(&s).unwrap();
        let mut f = Face::seeded(3);
        let mut c = FaceControls::default();
        for _ in 0..60 {
            c = f.update(
                1.0 / 60.0,
                FaceInputs {
                    effort: 1.0,
                    ..Default::default()
                },
            );
        }
        let m = posed(&s, &rig, &FaceControls { lids: 0.0, ..c });
        let moved = |n: &str| m[s.bone(n)].t - s.rest_model[s.bone(n)].t;
        let corner = moved("Bip01 LMouthCorner");
        assert!(corner.x < -0.003, "left corner pulled out: {corner}");
        assert!(moved("Bip01 RMouthCorner").x > 0.003);
        let brow = moved("Bip01 LInnerEyebrow");
        assert!(
            brow.y < -0.002 && brow.x > 0.001,
            "brow down and in: {brow}"
        );
        let lower = moved("Bip01 LEyeBlinkBottom");
        assert!(lower.y > 0.0005, "lower lid up: {lower}");
    }

    #[test]
    fn a_limp_face_droops_its_lids_and_drops_its_jaw() {
        let s = face_skeleton();
        let rig = FaceRig::new(&s).unwrap();
        let mut f = Face::seeded(4);
        let mut c = FaceControls::default();
        for _ in 0..120 {
            c = f.update(
                1.0 / 60.0,
                FaceInputs {
                    slack: 1.0,
                    ..Default::default()
                },
            );
        }
        assert!(c.lids > 0.7 && c.jaw > 0.45, "{c:?}");
        let m = posed(&s, &rig, &c);
        let lid = s.bone("Bip01 LEyeBlinkTop");
        assert!(m[lid].t.y < s.rest_model[lid].t.y - 0.004, "lid down");
        let corner = s.bone("Bip01 LMouthCorner");
        assert!(
            m[corner].t.y < s.rest_model[corner].t.y - 0.005,
            "the jaw carries the mouth down"
        );
        // And it doesn't blink while limp.
        for _ in 0..600 {
            c = f.update(
                1.0 / 60.0,
                FaceInputs {
                    slack: 1.0,
                    ..Default::default()
                },
            );
            assert!(c.lids < 0.9);
        }
    }
}
