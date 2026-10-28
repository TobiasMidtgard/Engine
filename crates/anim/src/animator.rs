//! Characters: the [`Animator`] component, the shared [`AnimationLibrary`], and the systems
//! that evaluate graphs every frame.

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::prelude::*;
use engine_world::animation::{AnimationParams, AnimationTrigger, Ragdoll, SkeletonPose};
use engine_world::{FocusPoints, SimulationRange};

use crate::clip::{AnimationClip, ClipBinding};
use crate::graph::{AnimGraph, EvalCtx, FiredEvent, GraphInstance};
use crate::motion::MotionDatabase;
use crate::skeleton::Skeleton;
use crate::xform::Xf;

/// How long an `AnimationTrigger` waits for a transition that can use it, seconds.
pub const TRIGGER_LIFETIME: f32 = 0.35;

/// A character resting beyond the [`SimulationRange`] takes a fresh pose this often, seconds,
/// so a far crowd isn't frozen in place.
pub const REST_POSE_SECS: f32 = 2.0;

/// Largest step procedural layers see on the first frame after a rest (the graph itself
/// catches up the whole time).
const WAKE_DT: f32 = 1.0 / 30.0;

/// Skeletons, clips and graphs by name, shared by every character. Builders register their
/// rigs here; AI agents add clips and graphs through `anim.add_clip` / `anim.add_graph`.
#[derive(Resource, Default)]
pub struct AnimationLibrary {
    pub skeletons: HashMap<String, Arc<Skeleton>>,
    pub clips: HashMap<String, Arc<AnimationClip>>,
    pub graphs: HashMap<String, AnimGraph>,
    /// Motion-matching databases (graphs name them in `motion_match` nodes).
    pub motions: HashMap<String, Arc<MotionDatabase>>,
}

impl AnimationLibrary {
    pub fn add_skeleton(&mut self, skeleton: Skeleton) -> Arc<Skeleton> {
        let s = Arc::new(skeleton);
        self.skeletons.insert(s.name.clone(), s.clone());
        s
    }

    pub fn add_clip(&mut self, clip: AnimationClip) -> Arc<AnimationClip> {
        let c = Arc::new(clip);
        self.clips.insert(c.name.clone(), c.clone());
        c
    }

    pub fn add_graph(&mut self, graph: AnimGraph) {
        self.graphs.insert(graph.name.clone(), graph);
    }

    pub fn clip(&self, name: &str) -> Option<Arc<AnimationClip>> {
        self.clips.get(name).cloned()
    }

    pub fn add_motion(&mut self, db: MotionDatabase) -> Arc<MotionDatabase> {
        let d = Arc::new(db);
        self.motions.insert(d.name.clone(), d.clone());
        d
    }

    /// Compile a graph for a skeleton (both by name).
    pub fn instantiate(&self, skeleton: &str, graph: &str) -> Result<Animator, String> {
        let s = self.skeletons.get(skeleton).cloned().ok_or_else(|| {
            let mut names: Vec<_> = self.skeletons.keys().cloned().collect();
            names.sort();
            format!(
                "skeleton {skeleton} is not in the library; known: {}",
                names.join(", ")
            )
        })?;
        let g = if graph.is_empty() {
            None
        } else {
            let def = self
                .graphs
                .get(graph)
                .ok_or_else(|| format!("animation graph {graph} is not in the library"))?;
            Some(GraphInstance::compile_with(
                def,
                &s,
                &|n| self.clip(n),
                &|n| self.motions.get(n).cloned(),
            )?)
        };
        Ok(Animator::new(s, g, skeleton, graph))
    }
}

/// Tunable animation settings of a character. Spawning an entity with just this (and a
/// `Transform`) builds its [`Animator`] from the library.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimatorSettings {
    /// Skeleton name in the [`AnimationLibrary`].
    pub skeleton: String,
    /// Graph name in the library (empty = rest pose plus procedural animation only).
    pub graph: String,
    /// Playback speed multiplier.
    pub speed: f32,
    pub paused: bool,
    /// Move the entity by the clips' root motion instead of playing in place.
    pub root_motion: bool,
    /// Beyond this distance from the nearest simulation focus (meters), the graph updates at
    /// a quarter rate. 0 = always full rate.
    pub lod_distance: f32,
    /// Start the graph this many seconds in, so people sharing clips (a crowd standing about)
    /// don't move in step.
    pub start_secs: f32,
}

impl Default for AnimatorSettings {
    fn default() -> Self {
        Self {
            skeleton: String::new(),
            graph: String::new(),
            speed: 1.0,
            paused: false,
            root_motion: false,
            lod_distance: 80.0,
            start_secs: 0.0,
        }
    }
}

impl AnimatorSettings {
    pub fn new(skeleton: &str, graph: &str) -> Self {
        Self {
            skeleton: skeleton.into(),
            graph: graph.into(),
            ..Default::default()
        }
    }
}

/// The animation runtime of one character: its graph, this frame's pose and what the graph
/// produced (curves, events, root motion). Systems in [`AnimSet`](crate::AnimSet) refine the
/// pose (procedural motion, IK, muscles) before it is published as `SkeletonPose`.
#[derive(Component)]
#[require(SkeletonPose, AnimationParams)]
pub struct Animator {
    pub skeleton: Arc<Skeleton>,
    graph: Option<GraphInstance>,
    skeleton_name: String,
    graph_name: String,
    /// This frame's pose, local to each parent bone.
    pub local: Vec<Xf>,
    /// This frame's pose in model space. Valid after [`Animator::refresh_model`] and after the
    /// output stage.
    pub model: Vec<Xf>,
    /// Blended curve values this frame (`effort`, …).
    pub curves: Vec<(String, f32)>,
    /// Pending triggers (name, seconds left).
    pub triggers: Vec<(String, f32)>,
    /// Events fired this frame.
    pub events: Vec<FiredEvent>,
    /// Root motion this frame, model space.
    pub root_delta: Vec3,
    /// Animation seconds that passed this frame (dilation, local scale and speed applied).
    pub dt: f32,
    /// The graph's output of the last evaluation (restored on frames the graph skips, so
    /// procedural layers and IK don't stack on their own previous result).
    base: Vec<Xf>,
    lod_skipped: f32,
    /// Beyond the simulation range (with its hysteresis).
    resting: bool,
    /// Seconds since the last pose while resting.
    rest_secs: f32,
    /// False on frames that keep the last pose: every pose system skips this animator.
    posed: bool,
    /// The first pose after frames without one: systems with motion history restart it.
    fresh: bool,
    /// A one-shot clip laid over the graph's pose (an attack, a hit reaction).
    overlay: Option<Overlay>,
    /// Seconds the pose holds still (hitstop).
    freeze: f32,
}

/// A clip playing over an [`Animator`]'s graph, from [`Animator::play_over`].
pub struct Overlay {
    pub clip: Arc<AnimationClip>,
    binding: ClipBinding,
    /// Seconds into the clip.
    pub time: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    /// The pose shown when this replaced another overlay: crossfaded from over `fade_in`.
    from: Option<Vec<Xf>>,
    /// Stopped early: (seconds since, fade length).
    stopping: Option<(f32, f32)>,
}

impl Animator {
    pub fn new(
        skeleton: Arc<Skeleton>,
        graph: Option<GraphInstance>,
        skeleton_name: &str,
        graph_name: &str,
    ) -> Self {
        Self {
            local: skeleton.rest.clone(),
            model: skeleton.rest_model.clone(),
            skeleton,
            graph,
            skeleton_name: skeleton_name.into(),
            graph_name: graph_name.into(),
            curves: Vec::new(),
            triggers: Vec::new(),
            events: Vec::new(),
            root_delta: Vec3::ZERO,
            dt: 0.0,
            base: Vec::new(),
            lod_skipped: 0.0,
            resting: false,
            rest_secs: 0.0,
            posed: true,
            fresh: false,
            overlay: None,
            freeze: 0.0,
        }
    }

    /// A procedural-only animator (no graph): the pose starts at rest every frame.
    pub fn procedural(skeleton: Arc<Skeleton>) -> Self {
        let name = skeleton.name.clone();
        Self::new(skeleton, None, &name, "")
    }

    /// Recompute `model` from `local`.
    pub fn refresh_model(&mut self) {
        let skeleton = self.skeleton.clone();
        skeleton.to_model(&self.local, &mut self.model);
    }

    /// Queue a trigger (what `AnimationTrigger` messages do).
    pub fn trigger(&mut self, name: &str) {
        self.triggers.retain(|t| t.0 != name);
        self.triggers.push((name.to_string(), TRIGGER_LIFETIME));
    }

    /// Value of a curve this frame (0 if no playing clip has it).
    pub fn curve(&self, name: &str) -> f32 {
        self.curves
            .iter()
            .find(|c| c.0 == name)
            .map_or(0.0, |c| c.1)
    }

    pub fn graph(&self) -> Option<&GraphInstance> {
        self.graph.as_ref()
    }

    pub fn graph_name(&self) -> &str {
        &self.graph_name
    }

    pub fn skeleton_name(&self) -> &str {
        &self.skeleton_name
    }

    /// Whether this frame poses the character. False while it rests beyond the
    /// [`SimulationRange`] between its occasional poses: the last pose stays, and procedural
    /// layers, IK and the pose output skip it.
    pub fn posed(&self) -> bool {
        self.posed
    }

    /// The first pose after frames without one: procedural layers that
    /// keep motion history (velocities, planted feet, spring particles) start it over.
    pub fn fresh(&self) -> bool {
        self.fresh
    }

    /// Resting beyond the [`SimulationRange`].
    pub fn resting(&self) -> bool {
        self.resting
    }

    /// Edit the pose in model space (IK, procedural motion).
    pub fn edit(&mut self) -> PoseEdit<'_> {
        PoseEdit {
            skeleton: &self.skeleton,
            local: &mut self.local,
            model: &mut self.model,
        }
    }

    /// World transform of a bone for an entity at `gt` (meters, f64 position).
    pub fn bone_world(&self, gt: &GlobalTransform, bone: usize) -> (DVec3, Quat) {
        let b = self.model[bone];
        let rot = gt.rotation();
        (
            gt.translation() + (rot * (b.t * gt.scale())).as_dvec3(),
            rot * b.r,
        )
    }

    /// Play `clip` over the graph's pose from its start, fading in over `fade_in` seconds
    /// and out over its last `fade_out`. Over a clip already playing it crossfades from the
    /// pose shown.
    pub fn play_over(&mut self, clip: Arc<AnimationClip>, fade_in: f32, fade_out: f32) {
        let from = self.overlay.as_ref().map(|_| self.local.clone());
        self.overlay = Some(Overlay {
            binding: ClipBinding::new(&clip, &self.skeleton),
            clip,
            time: 0.0,
            fade_in: fade_in.max(0.0),
            fade_out: fade_out.max(0.0),
            from,
            stopping: None,
        });
    }

    /// Fade the overlay out over `fade` seconds from now.
    pub fn stop_overlay(&mut self, fade: f32) {
        if let Some(o) = &mut self.overlay
            && o.stopping.is_none()
        {
            o.stopping = Some((0.0, fade.max(1e-3)));
        }
    }

    /// The clip playing over the graph, if any.
    pub fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }

    /// Hold the pose still for `secs` (hitstop: a blow lands and both fighters freeze for a
    /// few frames). The graph and any overlay pause with it.
    pub fn freeze_for(&mut self, secs: f32) {
        self.freeze = self.freeze.max(secs);
    }

    /// Seconds of freeze left.
    pub fn frozen(&self) -> f32 {
        self.freeze
    }

    /// Lay the overlay over `local` and advance it.
    fn apply_overlay(&mut self, dt: f32) {
        let Some(o) = &mut self.overlay else {
            return;
        };
        let prev = o.time;
        let d = o.clip.duration.max(1e-4);
        o.time = (o.time + dt).min(d);
        if o.time > prev {
            for e in o.clip.events_between(prev, o.time) {
                self.events.push(FiredEvent {
                    name: e.name.clone(),
                    bone: e.bone.clone(),
                });
            }
        }
        let smooth = |x: f32| {
            let x = x.clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        };
        let fade_in = if o.fade_in > 0.0 {
            smooth(o.time / o.fade_in)
        } else {
            1.0
        };
        let fade_out = match &mut o.stopping {
            Some((t, len)) => {
                *t += dt;
                1.0 - smooth(*t / *len)
            }
            None if o.fade_out > 0.0 => smooth((d - o.time) / o.fade_out),
            None => 1.0,
        };
        let mut pose = self.local.clone();
        o.clip.sample(o.time, &o.binding, &mut pose);
        // Over another overlay: from the pose that showed then, into this one.
        if let Some(from) = &o.from {
            for (p, f) in pose.iter_mut().zip(from) {
                *p = f.lerp(p, fade_in);
            }
        }
        let w = if o.from.is_some() {
            fade_out
        } else {
            fade_in.min(fade_out)
        };
        for (l, p) in self.local.iter_mut().zip(&pose) {
            *l = l.lerp(p, w);
        }
        for c in &o.clip.curves {
            if let Some(v) = c.track.sample(o.time) {
                match self.curves.iter_mut().find(|x| x.0 == c.name) {
                    Some(x) => x.1 = x.1 * (1.0 - w) + v * w,
                    None => self.curves.push((c.name.clone(), v * w)),
                }
            }
        }
        if o.time >= d || o.stopping.is_some_and(|(t, len)| t >= len) {
            self.overlay = None;
        }
    }

    /// Evaluate the graph for `dt` seconds.
    pub fn evaluate(&mut self, params: &AnimationParams, dt: f32) {
        if self.freeze > 0.0 {
            self.freeze -= dt;
            self.hold();
            return;
        }
        self.dt = dt;
        self.events.clear();
        self.curves.clear();
        self.root_delta = Vec3::ZERO;
        match &mut self.graph {
            Some(graph) => {
                let mut ctx = EvalCtx {
                    params,
                    triggers: &mut self.triggers,
                    dt,
                    events: &mut self.events,
                    curves: &mut self.curves,
                    root_delta: Vec3::ZERO,
                };
                graph.evaluate(&mut ctx, &mut self.local);
                self.root_delta = ctx.root_delta;
            }
            None => {
                self.local.clear();
                self.local.extend_from_slice(&self.skeleton.rest);
                self.triggers.retain_mut(|t| {
                    t.1 -= dt;
                    t.1 > 0.0
                });
            }
        }
        self.apply_overlay(dt);
        self.base.clone_from(&self.local);
    }

    /// Run the graph `secs` ahead with default parameters, dropping what it fires (see
    /// [`AnimatorSettings::start_secs`]).
    pub fn preroll(&mut self, secs: f32) {
        if secs > 0.0 {
            self.evaluate(&AnimationParams::default(), secs);
            self.events.clear();
            self.curves.clear();
            self.root_delta = Vec3::ZERO;
        }
    }

    /// Keep last frame's final pose without advancing the graph.
    fn keep(&mut self) {
        self.dt = 0.0;
        self.events.clear();
        self.root_delta = Vec3::ZERO;
    }

    /// Start this frame from the last evaluated pose without advancing the graph.
    fn hold(&mut self) {
        self.dt = 0.0;
        self.events.clear();
        self.root_delta = Vec3::ZERO;
        if self.base.len() == self.local.len() {
            self.local.clone_from(&self.base);
        }
    }
}

/// Model-space editing of a pose that keeps local and model transforms in sync.
pub struct PoseEdit<'a> {
    pub skeleton: &'a Skeleton,
    pub local: &'a mut [Xf],
    pub model: &'a mut [Xf],
}

impl PoseEdit<'_> {
    fn parent_model(&self, bone: usize) -> Xf {
        self.skeleton.parents[bone]
            .map(|p| self.model[p as usize])
            .unwrap_or(Xf::IDENTITY)
    }

    /// Set a bone's model-space rotation (children follow).
    pub fn set_model_rotation(&mut self, bone: usize, r: Quat) {
        let parent = self.parent_model(bone);
        self.local[bone].r = (parent.r.conjugate() * r).normalize();
        self.skeleton
            .update_model_subtree(self.local, self.model, bone);
    }

    /// Rotate a bone in model space by `delta` (children follow).
    pub fn rotate_model(&mut self, bone: usize, delta: Quat) {
        let r = (delta * self.model[bone].r).normalize();
        self.set_model_rotation(bone, r);
    }

    /// Move a bone in model space by `delta` meters (children follow).
    pub fn translate_model(&mut self, bone: usize, delta: Vec3) {
        let parent = self.parent_model(bone);
        let target = self.model[bone].t + delta;
        self.local[bone].t = parent.inverse().transform_point(target);
        self.skeleton
            .update_model_subtree(self.local, self.model, bone);
    }

    /// Rotate a bone's local transform by `delta` in its parent's space (children follow).
    pub fn rotate_local(&mut self, bone: usize, delta: Quat) {
        self.local[bone].r = (delta * self.local[bone].r).normalize();
        self.skeleton
            .update_model_subtree(self.local, self.model, bone);
    }

    pub fn position(&self, bone: usize) -> Vec3 {
        self.model[bone].t
    }
}

pub(crate) fn build_animators(
    mut commands: Commands,
    library: Res<AnimationLibrary>,
    q: Query<(Entity, &AnimatorSettings, Option<&Animator>), Changed<AnimatorSettings>>,
) {
    for (entity, settings, existing) in &q {
        if existing
            .is_some_and(|a| a.skeleton_name == settings.skeleton && a.graph_name == settings.graph)
        {
            continue;
        }
        if settings.skeleton.is_empty() {
            continue;
        }
        match library.instantiate(&settings.skeleton, &settings.graph) {
            Ok(mut animator) => {
                animator.preroll(settings.start_secs);
                commands.entity(entity).insert(animator);
            }
            Err(e) => tracing::warn!("animator for {entity}: {e}"),
        }
    }
}

pub(crate) fn queue_triggers(
    mut triggers: MessageReader<AnimationTrigger>,
    mut q: Query<&mut Animator>,
) {
    for t in triggers.read() {
        if let Ok(mut a) = q.get_mut(t.entity) {
            a.trigger(&t.name);
        }
    }
}

type EvalQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Animator,
        Option<&'static AnimatorSettings>,
        &'static AnimationParams,
        Option<&'static LocalTimeScale>,
        Option<&'static mut Transform>,
        Option<&'static GlobalTransform>,
        Option<&'static Ragdoll>,
    ),
>;

pub(crate) fn evaluate_animators(
    time: Res<Time>,
    focus: Res<FocusPoints>,
    range: Res<SimulationRange>,
    mut q: EvalQuery,
) {
    let frame = time.frame_count();
    q.par_iter_mut().for_each(
        |(entity, mut animator, settings, params, local, transform, gt, ragdoll)| {
            let defaults = AnimatorSettings::default();
            let settings = settings.unwrap_or(&defaults);
            let mut dt = if settings.paused {
                0.0
            } else {
                time.delta_for(local) * settings.speed
            };
            // Beyond the simulation range: keep the last pose and take a fresh one every few
            // seconds. A body in its ragdoll follows physics, so it never rests.
            let was_resting = animator.resting;
            if let Some(gt) = gt {
                let p = gt.translation();
                let limp = ragdoll.is_some_and(|r| r.weight > 0.0);
                animator.resting = !limp
                    && if was_resting {
                        !range.is_active(&focus, p)
                    } else {
                        range.is_resting(&focus, p)
                    };
            }
            if animator.resting {
                if !was_resting {
                    // Spread the poses of a crowd that comes to rest together over the cycle.
                    animator.rest_secs = (entity.to_bits() % 97) as f32 / 97.0 * REST_POSE_SECS;
                }
                animator.rest_secs += dt;
                if animator.rest_secs < REST_POSE_SECS {
                    animator.posed = false;
                    animator.fresh = false;
                    animator.keep();
                    return;
                }
                dt = std::mem::take(&mut animator.rest_secs);
            } else {
                dt += std::mem::take(&mut animator.rest_secs);
            }
            animator.fresh = !animator.posed;
            animator.posed = true;
            // Far away: update at a quarter rate (accumulating time so speed is unchanged).
            if !animator.fresh
                && settings.lod_distance > 0.0
                && let Some(gt) = gt
                && !focus.points.is_empty()
                && focus.distance(gt.translation()) > settings.lod_distance as f64
            {
                animator.lod_skipped += dt;
                if !(frame + entity.to_bits()).is_multiple_of(4) {
                    animator.hold();
                    return;
                }
                dt = std::mem::take(&mut animator.lod_skipped);
            } else {
                dt += std::mem::take(&mut animator.lod_skipped);
            }
            animator.evaluate(params, dt);
            if animator.fresh {
                animator.dt = animator.dt.min(WAKE_DT);
            }
            if settings.root_motion
                && let Some(mut t) = transform
            {
                let d = animator.root_delta;
                let horizontal = Vec3::new(d.x, 0.0, d.z);
                let world = t.rotation * (horizontal * t.scale);
                t.translation += world.as_dvec3();
                // Play in place: the root keeps only its vertical motion.
                let skeleton = animator.skeleton.clone();
                if let Some(root) = skeleton.index_of("root") {
                    let rest = skeleton.rest[root].t;
                    let r = &mut animator.local[root];
                    r.t.x = rest.x;
                    r.t.z = rest.z;
                }
            }
        },
    );
}

/// Publish the final pose: model space, the `SkeletonPose` contract component.
pub(crate) fn publish_pose(mut q: Query<(&mut Animator, &mut SkeletonPose)>) {
    q.par_iter_mut().for_each(|(mut animator, mut pose)| {
        if !animator.posed {
            return;
        }
        animator.refresh_model();
        if pose.names.len() != animator.skeleton.len() {
            pose.names = animator.skeleton.names.clone();
            pose.parents = animator.skeleton.parents.clone();
        }
        pose.bones.clear();
        pose.bones
            .extend(animator.model.iter().map(|m| m.to_bone_pose()));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::BoneDef;

    #[test]
    fn library_errors_list_known_names() {
        let mut lib = AnimationLibrary::default();
        lib.add_skeleton(
            Skeleton::new("biped", vec![BoneDef::new("root", None, Vec3::ZERO)]).unwrap(),
        );
        let e = lib.instantiate("quad", "").err().unwrap();
        assert!(e.contains("biped"), "{e}");
        assert!(lib.instantiate("biped", "").is_ok());
    }

    #[test]
    fn pose_edit_keeps_local_and_model_in_sync() {
        let s = Arc::new(
            Skeleton::new(
                "c",
                vec![
                    BoneDef::new("a", None, Vec3::ZERO),
                    BoneDef::new("b", Some("a"), Vec3::Y),
                    BoneDef::new("c", Some("b"), Vec3::Y),
                ],
            )
            .unwrap(),
        );
        let mut a = Animator::procedural(s);
        a.refresh_model();
        a.edit()
            .rotate_model(1, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        a.edit().translate_model(0, Vec3::X);
        let model = a.model.clone();
        a.refresh_model();
        for (x, y) in model.iter().zip(&a.model) {
            assert!(x.t.abs_diff_eq(y.t, 1e-5), "{:?} vs {:?}", x.t, y.t);
        }
        assert!(a.model[2].t.abs_diff_eq(Vec3::new(0.0, 1.0, 0.0), 1e-5));
    }
}
