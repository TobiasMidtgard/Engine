//! CPU skinning for the demo bodies.
//!
//! [`BodyLibrary`] builds every [`BodyPart`] once, on background threads, and shares it. Each
//! [`SkinnedPart`] entity (a child of a character with a `SkeletonPose`) gets its own mesh
//! asset, deformed every frame with dual-quaternion skinning (no candy-wrapper twists, no
//! collapsing elbows and knees), and registered under its key, with its skin weights in
//! [`MeshSkins`], so hair can grow on it ([`crate::HairEmitter::Surface`]). Like
//! `TransformInterpolation`, a body is drawn between the poses its skeleton had at the start
//! and the end of the latest fixed step, so it stays in step with the cloth and hair simulated
//! on it (which are drawn the same way) instead of running a step ahead of them.

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_core::tasks::ComputeTaskPool;
use engine_core::time::FixedTime;
use engine_render::{GpuSkin, MaterialRef, Mesh, MeshRenderer, SkinBone, SkinWeights, Vertex};
use engine_world::animation::{BonePose, SkeletonPose};
use engine_world::{FocusPoints, SimulationRange};

use super::body::{self, BodyPart, BodyQuality, SkinTemplate};
use super::statue::DemoStatue;
use crate::hair::{MeshSkin, MeshSkins};
use crate::settings::SoftBodyConfig;

/// A skinned body part drawn on the skeleton of `owner`.
#[derive(Component, Debug, Clone)]
pub struct SkinnedPart {
    pub part: BodyPart,
    /// The entity with the `SkeletonPose`.
    pub owner: Entity,
    /// Key of the deformed mesh in `Assets<Mesh>`.
    pub key: String,
    pub material: MaterialRef,
    pub mesh: Option<Handle<Mesh>>,
    /// Hair color painted on the scalp under a groom.
    pub scalp: Option<ScalpPaint>,
    /// The owner's pose at the start of the latest fixed step.
    previous: Vec<BonePose>,
    /// The coarse mesh drawn far off (see [`FAR_MESH`]), once built, and whether it is
    /// the one drawn now.
    far: Option<Handle<Mesh>>,
    showing_far: bool,
    /// Far enough off to want the coarse mesh.
    wants_far: bool,
    /// The pose the drawn mesh was last skinned in.
    skinned: Vec<BonePose>,
    /// Which frame of every few it is skinned on, far off (its owner's).
    phase: u32,
}

/// Hair color painted on the scalp under a groom, as games paint the scalp under their hair,
/// so the gaps between strands show more hair instead of bare skin (or fiberglass). The
/// region is a [`HairEmitter::Scalp`](crate::HairEmitter)'s, in the bind frame of `bone`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScalpPaint {
    pub bone: usize,
    pub center: Vec3,
    pub radii: Vec3,
    pub hairline: f32,
    /// Multiplies the vertex color where hair covers the scalp.
    pub tint: [f32; 3],
}

impl SkinnedPart {
    pub fn new(part: BodyPart, owner: Entity, key: &str, material: impl Into<MaterialRef>) -> Self {
        Self {
            part,
            owner,
            key: key.to_string(),
            material: material.into(),
            mesh: None,
            scalp: None,
            previous: Vec::new(),
            far: None,
            showing_far: false,
            wants_far: false,
            skinned: Vec::new(),
            phase: 0,
        }
    }

    pub fn with_scalp(mut self, paint: ScalpPaint) -> Self {
        self.scalp = Some(paint);
        self
    }
}

/// Darken the template's vertex colors toward `paint.tint` where hair covers the scalp.
fn paint_scalp(template: &SkinTemplate, paint: &ScalpPaint, vertices: &mut [Vertex]) {
    let Some(bone) = template.bind.get(paint.bone) else {
        return;
    };
    let to_bone = bone.rotation.conjugate();
    let radii = paint.radii.max(Vec3::splat(0.01));
    for v in vertices {
        let local = to_bone * (Vec3::from(v.position) - bone.translation);
        let d = (local - paint.center) / radii;
        let len = d.length();
        if !(1e-3..1.35).contains(&len) {
            continue;
        }
        // Only the skin of the head itself, not the neck or shoulders below it.
        let w =
            crate::hair::scalp_cover(d / len, paint.hairline) * (1.0 - smoothstep(1.12, 1.35, len));
        for (c, t) in v.color.iter_mut().zip(paint.tint) {
            *c *= 1.0 + (t - 1.0) * w;
        }
    }
}

/// Built body templates, shared by every character that uses them.
#[derive(Resource, Default)]
pub struct BodyLibrary {
    /// By part and whether it's the coarse far mesh.
    ready: HashMap<(BodyPart, bool), Arc<SkinTemplate>>,
    /// Their weights for the GPU skinning.
    weights: HashMap<(BodyPart, bool), Arc<SkinWeights>>,
    skins: HashMap<BodyPart, Arc<MeshSkin>>,
    pending: Vec<((BodyPart, bool), JoinHandle<SkinTemplate>)>,
    /// Mesh resolution (1 = full).
    pub quality: BodyQuality,
    /// Build on the calling thread instead of in the background (tests, screenshots).
    pub blocking: bool,
}

impl BodyLibrary {
    /// The template for `part`, once it's built.
    pub fn get(&self, part: BodyPart) -> Option<Arc<SkinTemplate>> {
        self.ready.get(&(part, false)).cloned()
    }

    /// The coarse template drawn far off, once it's built.
    pub fn get_far(&self, part: BodyPart) -> Option<Arc<SkinTemplate>> {
        self.ready.get(&(part, true)).cloned()
    }

    /// The skin weights of `part`, once it's built.
    pub fn skin(&self, part: BodyPart) -> Option<Arc<MeshSkin>> {
        self.skins.get(&part).cloned()
    }

    fn finish(&mut self, (part, far): (BodyPart, bool), template: SkinTemplate) {
        // Hair grows on the full mesh, by its vertices' weights.
        if !far {
            let skin = MeshSkin {
                joints: template.joints.clone(),
                weights: template.weights.clone(),
                bind: template.bind.clone(),
            };
            self.skins.insert(part, Arc::new(skin));
        }
        let weights = SkinWeights {
            joints: template.joints.clone(),
            weights: template.weights.clone(),
        };
        self.weights.insert((part, far), Arc::new(weights));
        self.ready.insert((part, far), Arc::new(template));
    }

    /// Start building `part` unless it's built or underway.
    pub fn request(&mut self, part: BodyPart) {
        self.request_mesh((part, false));
    }

    /// Start building the coarse far mesh of `part` unless it's built or underway.
    pub fn request_far(&mut self, part: BodyPart) {
        self.request_mesh((part, true));
    }

    fn request_mesh(&mut self, key: (BodyPart, bool)) {
        if self.ready.contains_key(&key) || self.pending.iter().any(|(k, _)| *k == key) {
            return;
        }
        let (part, far) = key;
        let quality = BodyQuality(self.quality.0 * if far { FAR_MESH } else { 1.0 });
        if self.blocking {
            self.finish(key, body::build(part, quality));
            return;
        }
        let spawned = std::thread::Builder::new()
            .name(format!("{part:?} body"))
            .spawn(move || body::build(part, quality));
        match spawned {
            Ok(handle) => self.pending.push((key, handle)),
            Err(_) => self.finish(key, body::build(part, quality)),
        }
    }

    /// Collect finished builds.
    pub fn poll(&mut self) {
        let mut k = 0;
        while k < self.pending.len() {
            if self.pending[k].1.is_finished() {
                let (key, handle) = self.pending.swap_remove(k);
                match handle.join() {
                    Ok(t) => self.finish(key, t),
                    Err(_) => engine_core::log::warn!("building the {:?} body failed", key.0),
                }
            } else {
                k += 1;
            }
        }
    }
}

/// Start building the bodies characters ask for; pick up finished ones.
pub fn prepare_bodies(mut library: ResMut<BodyLibrary>, parts: Query<&SkinnedPart>) {
    for p in &parts {
        library.request(p.part);
        if p.wants_far {
            library.request_far(p.part);
        }
    }
    library.poll();
}

/// `FixedPreUpdate`: remember the pose each part's skeleton starts the fixed step with.
pub fn snapshot_poses(mut parts: Query<&mut SkinnedPart>, poses: Query<&SkeletonPose>) {
    for mut part in &mut parts {
        if let Ok(pose) = poses.get(part.owner) {
            let part = &mut *part;
            part.previous.clone_from(&pose.bones);
        }
    }
}

/// How much coarser the far mesh is (a sixteenth of the vertices), and from how many LOD
/// distances off it is drawn (back to the full mesh a little nearer).
pub const FAR_MESH: f32 = 4.0;
const FAR_FROM: f32 = 2.0;
/// How far a bone may move on the drawn mesh before it is posed again, per metre of
/// distance past the LOD distance (about a pixel on a 1080p screen): a far body is posed
/// only as often as its pose visibly changes (Unreal's update rate optimization for far
/// skeletal meshes), a near one whenever it changes at all. Each new pose is a skinning
/// dispatch on the GPU (on the CPU it was an upload of the whole mesh: 19 MB a frame).
const SKIN_ERROR_PER_M: f32 = 0.0015;
/// Far parts are also skinned at most every `n`-th frame, `n` their distance in multiples of
/// [`FAR_FROM`] × the LOD distance (up to this), each on its own phase so the uploads of a
/// crowd spread over the frames instead of landing together (Unreal's URO frame skipping).
const MAX_SKIN_EVERY: u32 = 4;

/// Give new parts their mesh, in its bind pose, and pose each on the GPU ([`GpuSkin`]) like
/// its skeleton, interpolated within the fixed step like `TransformInterpolation`, as often
/// as it shows: far off a coarse mesh, posed again only when its pose moved visibly, and not
/// at all past the simulation range. Only the bones go to the GPU, and only when they move.
/// A statue is skinned once on the CPU into its pose, which becomes its collider.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn skin_bodies(
    mut commands: Commands,
    library: Res<BodyLibrary>,
    fixed: Option<Res<FixedTime>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    mut skins: ResMut<MeshSkins>,
    mut parts: Query<(
        Entity,
        &mut SkinnedPart,
        Option<&mut GpuSkin>,
        Has<DemoStatue>,
    )>,
    poses: Query<(&SkeletonPose, Option<&GlobalTransform>)>,
    (focus, range, config): (
        Option<Res<FocusPoints>>,
        Option<Res<SimulationRange>>,
        Option<Res<SoftBodyConfig>>,
    ),
    mut scratch: Local<(Vec<BonePose>, Vec<BoneTransform>)>,
    mut frames: Local<(u32, HashMap<Entity, u32>)>,
) {
    let Some(mut meshes) = meshes else {
        return;
    };
    let alpha = fixed.map_or(1.0, |f| f.alpha());
    let lod = config.map_or(SoftBodyConfig::default().lod_distance, |c| c.lod_distance);
    let active = range.map_or(SimulationRange::default().active_m, |r| r.active_m) as f32;
    let (posed, bones) = &mut *scratch;
    let (frame, phases) = &mut *frames;
    *frame = frame.wrapping_add(1);
    for (e, mut part, gpu, statue) in &mut parts {
        let Some(template) = library.get(part.part) else {
            continue;
        };
        let Some(weights) = library.weights.get(&(part.part, false)).cloned() else {
            continue;
        };
        let Ok((pose, at)) = poses.get(part.owner) else {
            continue;
        };
        let dist = match (&focus, at) {
            (Some(f), Some(t)) if !f.points.is_empty() => f.distance(t.translation()) as f32,
            _ => 0.0,
        };
        // A new mesh starts at the current pose; after that, between the last two simulated
        // poses.
        let t = if part.mesh.is_some() { alpha } else { 1.0 };
        interpolate_pose(&part.previous, &pose.bones, t, posed);
        let full = match part.mesh {
            Some(h) => h,
            None => {
                let mut mesh = template.mesh();
                if let Some(paint) = &part.scalp {
                    paint_scalp(&template, paint, &mut mesh.vertices);
                }
                let renderer = |h| MeshRenderer::new(h, part.material.clone());
                if statue {
                    skin(&template, posed, &mut mesh.vertices, bones);
                    let h = meshes.insert_keyed(&part.key, mesh);
                    commands.entity(e).insert(renderer(h));
                    part.mesh = Some(h);
                    continue;
                }
                let h = meshes.insert_keyed(&part.key, mesh);
                if let Some(skin) = library.skin(part.part) {
                    skins.0.insert(part.key.clone(), skin);
                }
                commands
                    .entity(e)
                    .insert((renderer(h), gpu_skin(weights, &template.bind, posed)));
                part.mesh = Some(h);
                part.skinned.clone_from(posed);
                // One phase for all of a body's parts, so they never part at the seams.
                let next = phases.len() as u32;
                part.phase = *phases.entry(part.owner).or_insert(next);
                continue;
            }
        };
        if statue {
            continue;
        }
        // Which mesh to draw: the coarse one far off, once it's built.
        part.wants_far = dist > FAR_FROM * lod * if part.showing_far { 0.9 } else { 1.0 };
        let far = match (part.wants_far, part.far, library.get_far(part.part)) {
            (true, Some(h), Some(t)) => Some((h, t)),
            (true, None, Some(t)) => {
                let mut mesh = t.mesh();
                if let Some(paint) = &part.scalp {
                    paint_scalp(&t, paint, &mut mesh.vertices);
                }
                let h = meshes.insert_keyed(&format!("{} (far)", part.key), mesh);
                part.far = Some(h);
                Some((h, t))
            }
            _ => None,
        };
        let far = far.and_then(|(h, t)| {
            let w = library.weights.get(&(part.part, true)).cloned()?;
            Some((h, t, w))
        });
        if far.is_some() != part.showing_far {
            part.showing_far = far.is_some();
            let (shown, template, weights) = far.unwrap_or((full, template, weights));
            commands.entity(e).insert((
                MeshRenderer::new(shown, part.material.clone()),
                gpu_skin(weights, &template.bind, posed),
            ));
            part.skinned.clone_from(posed);
            continue;
        }
        let Some(mut gpu) = gpu else {
            continue;
        };
        // Posed again only when the pose moved more than shows from here.
        let tolerance = if dist > active {
            f32::INFINITY
        } else {
            (dist - lod).max(0.0) * SKIN_ERROR_PER_M
        };
        let every = ((dist / (FAR_FROM * lod)) as u32).clamp(1, MAX_SKIN_EVERY);
        if frame.wrapping_add(part.phase) % every != 0 {
            continue;
        }
        let moved = pose_change(&part.skinned, posed);
        if moved <= tolerance || moved == 0.0 {
            continue;
        }
        let bind = far.as_ref().map_or(&template.bind, |(_, t, _)| &t.bind);
        set_bones(&mut gpu.bones, bind, posed);
        part.skinned.clone_from(posed);
    }
}

/// A GPU skin with `weights`, posed in `pose` from `bind`.
fn gpu_skin(weights: Arc<SkinWeights>, bind: &[BonePose], pose: &[BonePose]) -> GpuSkin {
    let mut skin = GpuSkin::new(weights);
    set_bones(&mut skin.bones, bind, pose);
    skin
}

/// Each bone's move from `bind` to `pose` (bones the pose lacks stay put).
fn set_bones(out: &mut Vec<SkinBone>, bind: &[BonePose], pose: &[BonePose]) {
    out.clear();
    out.extend(bind.iter().enumerate().map(|(i, b)| {
        let p = pose.get(i).unwrap_or(b);
        SkinBone::from_poses((b.rotation, b.translation), (p.rotation, p.translation))
    }));
}

/// How far the pose moved, about: the most any bone moved, its turn counted at half a metre.
fn pose_change(from: &[BonePose], to: &[BonePose]) -> f32 {
    if from.len() != to.len() {
        return f32::INFINITY;
    }
    from.iter()
        .zip(to)
        .map(|(a, b)| {
            a.translation.distance(b.translation) + 0.5 * a.rotation.angle_between(b.rotation)
        })
        .fold(0.0, f32::max)
}

/// `previous` blended toward `current` by `t` (all of `current` where `previous` lacks a
/// bone).
fn interpolate_pose(previous: &[BonePose], current: &[BonePose], t: f32, out: &mut Vec<BonePose>) {
    out.clear();
    out.extend(
        current
            .iter()
            .enumerate()
            .map(|(i, c)| match previous.get(i) {
                Some(p) if t < 1.0 => BonePose {
                    translation: p.translation.lerp(c.translation, t),
                    rotation: p.rotation.slerp(c.rotation, t).normalize(),
                    ..*c
                },
                _ => *c,
            }),
    );
}

/// One bone's skinning transform (bind pose → current pose) as a rotation, a translation and
/// the dual quaternion of both.
#[derive(Debug, Clone, Copy, Default)]
pub struct BoneTransform {
    real: Quat,
    dual: Quat,
    translation: Vec3,
}

/// Skinning transforms for every bone of `bind` in `pose` (bones the pose lacks stay put).
pub fn bone_transforms(bind: &[BonePose], pose: &[BonePose], out: &mut Vec<BoneTransform>) {
    out.clear();
    for (i, b) in bind.iter().enumerate() {
        let p = pose.get(i).copied().unwrap_or(*b);
        let real = (p.rotation * b.rotation.conjugate()).normalize();
        let translation = p.translation - real * b.translation;
        let t = Quat::from_xyzw(translation.x, translation.y, translation.z, 0.0);
        out.push(BoneTransform {
            real,
            dual: t * real * 0.5,
            translation,
        });
    }
}

/// Deform the template into `out` (same vertex order) for `pose` (model-space bones).
pub fn skin(
    template: &SkinTemplate,
    pose: &[BonePose],
    out: &mut [Vertex],
    bones: &mut Vec<BoneTransform>,
) {
    bone_transforms(&template.bind, pose, bones);
    let bones = &bones[..];
    const CHUNK: usize = 4096;
    match ComputeTaskPool::try_get() {
        Some(pool) if out.len() > 2 * CHUNK => {
            pool.scope(|s| {
                for (k, chunk) in out.chunks_mut(CHUNK).enumerate() {
                    s.spawn(async move { skin_range(template, bones, k * CHUNK, chunk) });
                }
            });
        }
        _ => skin_range(template, bones, 0, out),
    }
}

fn skin_range(template: &SkinTemplate, bones: &[BoneTransform], start: usize, out: &mut [Vertex]) {
    for (k, v) in out.iter_mut().enumerate() {
        let i = start + k;
        let (Some(j), Some(w)) = (template.joints.get(i), template.weights.get(i)) else {
            break;
        };
        let b0 = bones[j[0] as usize];
        let (rot, translation) = if w[0] >= 0.999 {
            (b0.real, b0.translation)
        } else {
            // Blend dual quaternions on the same hemisphere, then normalize.
            let mut real = b0.real * w[0];
            let mut dual = b0.dual * w[0];
            for n in 1..4 {
                if w[n] <= 0.0 {
                    break;
                }
                let b = bones[j[n] as usize];
                let s = if b.real.dot(b0.real) < 0.0 {
                    -w[n]
                } else {
                    w[n]
                };
                real += b.real * s;
                dual += b.dual * s;
            }
            let inv = 1.0 / real.length().max(1e-8);
            let real = real * inv;
            let t = dual * inv * real.conjugate() * 2.0;
            (real, Vec3::new(t.x, t.y, t.z))
        };
        v.position = (rot * template.positions[i] + translation).to_array();
        v.normal = (rot * template.normals[i]).to_array();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::body::{Dog, Human};
    use std::sync::Arc as StdArc;

    fn posed(bind: &[BonePose]) -> SkeletonPose {
        SkeletonPose {
            names: StdArc::from((0..bind.len()).map(|i| format!("b{i}")).collect::<Vec<_>>()),
            parents: StdArc::from(vec![None; bind.len()]),
            bones: bind.to_vec(),
        }
    }

    fn skinned(t: &SkinTemplate, pose: &SkeletonPose) -> Vec<Vertex> {
        let mut out = t.mesh().vertices;
        let mut bones = Vec::new();
        skin(t, &pose.bones, &mut out, &mut bones);
        out
    }

    #[test]
    fn mannequin_mesh_is_clean_and_skinned_vertices_follow_their_bones() {
        let t = body::build(BodyPart::Mannequin, BodyQuality(3.0));
        assert!(
            t.indices.len() / 3 > 3000,
            "{} triangles",
            t.indices.len() / 3
        );
        for ((p, n), w) in t.positions.iter().zip(&t.normals).zip(&t.weights) {
            assert!(p.is_finite() && n.is_finite());
            assert!((n.length() - 1.0).abs() < 1e-3);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
        // A seamless figure from the soles (flat on the ground) to the top of the head.
        let lo = t.positions.iter().fold(Vec3::MAX, |a, p| a.min(*p));
        let hi = t.positions.iter().fold(Vec3::MIN, |a, p| a.max(*p));
        assert!(
            lo.y.abs() < 0.01 && (1.78..1.9).contains(&hi.y),
            "{lo} {hi}"
        );

        // The bind pose reproduces the rest mesh.
        let rest = posed(&t.bind);
        for (v, p) in skinned(&t, &rest).iter().zip(&t.positions) {
            assert!(Vec3::from(v.position).distance(*p) < 1e-4);
        }
        // Raise the right hand: fingertips move with the hand bone, the left side stays.
        let mut pose = posed(&t.bind);
        let hand = Human::ARM_R + 2;
        let lift = Quat::from_rotation_x(-1.2);
        let shoulder = t.bind[Human::ARM_R].translation;
        for b in Human::ARM_R..=hand {
            let bp = &mut pose.bones[b];
            bp.translation = shoulder + lift * (bp.translation - shoulder);
            bp.rotation = lift * bp.rotation;
        }
        let out = skinned(&t, &pose);
        let mut checked = 0;
        for (i, v) in out.iter().enumerate() {
            let rest_p = t.positions[i];
            let moved = Vec3::from(v.position);
            if t.joints[i][0] as usize == hand && t.weights[i][0] > 0.999 {
                let expect = shoulder + lift * (rest_p - shoulder);
                assert!(moved.distance(expect) < 1e-3, "hand vertex {i}");
                checked += 1;
            }
            if rest_p.x < -0.12 {
                assert!(moved.distance(rest_p) < 1e-4, "left side moved at {rest_p}");
            }
            let n = Vec3::from(v.normal);
            assert!(moved.is_finite() && (n.length() - 1.0).abs() < 1e-3);
        }
        assert!(checked > 50, "{checked} hand vertices");
    }

    #[test]
    fn bodies_are_drawn_between_their_last_two_poses() {
        let a = BonePose::default();
        let b = BonePose {
            translation: Vec3::new(0.2, 0.0, 0.0),
            rotation: Quat::from_rotation_y(1.0),
            ..BonePose::default()
        };
        let mut out = Vec::new();
        interpolate_pose(&[a], &[b], 0.5, &mut out);
        assert!(out[0].translation.distance(Vec3::new(0.1, 0.0, 0.0)) < 1e-6);
        assert!(out[0].rotation.angle_between(Quat::from_rotation_y(0.5)) < 1e-4);
        // At the end of the step, or with no earlier pose, the current pose.
        interpolate_pose(&[a], &[b], 1.0, &mut out);
        assert_eq!(out[0], b);
        interpolate_pose(&[], &[b], 0.3, &mut out);
        assert_eq!(out[0], b);
    }

    #[test]
    fn bodies_pose_on_the_gpu_and_far_ones_draw_a_coarse_mesh_posed_only_when_it_shows() {
        use engine_core::time::TimeSource;
        #[derive(Resource)]
        struct Camera(DVec3);
        let mut app = App::new();
        app.add_plugins((engine_world::WorldPlugin, crate::SoftBodyPlugin));
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.world.insert_resource(Assets::<Mesh>::default());
        {
            let mut library = app.world.resource_mut::<BodyLibrary>();
            library.blocking = true;
            library.quality = BodyQuality(3.0);
        }
        app.insert_resource(Camera(DVec3::new(0.0, 1.7, 5.0)));
        app.add_systems(
            PreUpdate,
            |cam: Res<Camera>, mut focus: ResMut<FocusPoints>| {
                focus.points.clear();
                focus.points.push(cam.0);
            },
        );
        let bind = body::build(BodyPart::Mannequin, BodyQuality(3.0)).bind;
        let owner = app.world.spawn((Transform::default(), posed(&bind))).id();
        let e = app
            .world
            .spawn((
                Transform::default(),
                SkinnedPart::new(
                    BodyPart::Mannequin,
                    owner,
                    "test body",
                    MaterialRef::default(),
                ),
                ChildOf(owner),
            ))
            .id();
        app.update_n(3);
        let shown = |app: &App| match &app.world.get::<MeshRenderer>(e).unwrap().mesh {
            engine_render::MeshRef::Handle(h) => *h,
            other => panic!("{other:?}"),
        };
        let version = |app: &App, h| app.world.resource::<Assets<Mesh>>().version(h);
        let bones = |app: &App| app.world.get::<GpuSkin>(e).unwrap().bones.clone();
        let bend = |app: &mut App, angle: f32| {
            let mut pose = app.world.get_mut::<SkeletonPose>(owner).unwrap();
            let b = &mut pose.bones[Human::HEAD_I];
            b.rotation = Quat::from_rotation_x(angle) * bind[Human::HEAD_I].rotation;
        };
        let full = shown(&app);
        let uploaded = version(&app, full);

        // Near: still, it isn't posed again; moved, it is, the head's bone carrying the
        // head as the CPU skinning does.
        let b = bones(&app);
        app.update_n(5);
        assert_eq!(bones(&app), b, "a still body was posed again");
        bend(&mut app, 0.05);
        app.update_n(3);
        let b = bones(&app);
        let head = bind[Human::HEAD_I].translation + Vec3::Y * 0.1;
        let expected = {
            let r = Quat::from_rotation_x(0.05) * bind[Human::HEAD_I].rotation;
            let local =
                bind[Human::HEAD_I].rotation.inverse() * (head - bind[Human::HEAD_I].translation);
            r * local + bind[Human::HEAD_I].translation
        };
        assert!(
            b[Human::HEAD_I].apply(head).distance(expected) < 1e-4,
            "a moving near body wasn't posed"
        );

        // 100 m off: the coarse mesh, posed again only for a change that shows.
        app.insert_resource(Camera(DVec3::new(0.0, 1.7, 100.0)));
        app.update_n(3);
        let far = shown(&app);
        assert_ne!(far, full);
        let n = |h| {
            app.world
                .resource::<Assets<Mesh>>()
                .get(h)
                .unwrap()
                .vertices
                .len()
        };
        assert!(n(far) * 8 < n(full), "{} vs {} vertices", n(far), n(full));
        assert_eq!(bones(&app).len(), bind.len());
        let b = bones(&app);
        bend(&mut app, 0.07);
        app.update_n(3);
        assert_eq!(bones(&app), b, "a 1 cm nod at 100 m was posed");
        bend(&mut app, 1.0);
        app.update_n(8);
        assert_ne!(bones(&app), b, "a big turn at 100 m wasn't posed");

        // Past the simulation range: not posed at all; near again, the full mesh.
        app.insert_resource(Camera(DVec3::new(0.0, 1.7, 400.0)));
        app.update_n(2);
        let b = bones(&app);
        bend(&mut app, -1.0);
        app.update_n(3);
        assert_eq!(bones(&app), b);
        app.insert_resource(Camera(DVec3::new(0.0, 1.7, 5.0)));
        app.update_n(2);
        assert_eq!(shown(&app), full);
        // The meshes stay in their bind pose: only bones go to the GPU.
        assert_eq!(version(&app, full), uploaded, "the mesh was rewritten");
    }

    #[test]
    fn dog_parts_build_on_one_skeleton() {
        for part in [BodyPart::Dog, BodyPart::DogNose, BodyPart::DogEyes] {
            let t = body::build(part, BodyQuality(3.0));
            assert!(!t.indices.is_empty(), "{part:?}");
            assert_eq!(t.bind.len(), Dog::BONES);
            assert!(t.positions.iter().all(|p| p.is_finite() && p.y > -0.005));
            assert!(t.normals.iter().all(|n| (n.length() - 1.0).abs() < 1e-3));
        }
    }
}
