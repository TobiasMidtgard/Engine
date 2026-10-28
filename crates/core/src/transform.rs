//! Large-world transforms.
//!
//! [`Transform`] is the local transform you edit: an `f64` translation in meters (so positions
//! stay millimeter-precise out to solar-system distances), an `f32` rotation and scale.
//! [`GlobalTransform`] is the computed world transform, written in `PostUpdate` by
//! [`propagate_transforms_system`] from the `ChildOf`/`Children` hierarchy.
//!
//! Conventions: right-handed, **+Y up, −Z forward, +X right**, meters, radians.
//! On planets "up" is local (away from the planet center); query gravity through
//! `engine_world::Gravity` instead of assuming −Y.
//!
//! The renderer never sees `f64`: it subtracts the camera position in `f64` and only then
//! converts to `f32` ([`GlobalTransform::matrix_relative_to`]), which is what keeps distant
//! objects jitter-free.

use bevy_ecs::prelude::*;
use glam::{DAffine3, DMat3, DVec3, Mat3, Mat4, Quat, Vec3, Vec4};
use serde::{Deserialize, Serialize};

use crate::time::FixedTime;

/// Local position, rotation and scale. Relative to the parent if the entity has `ChildOf`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(GlobalTransform)]
#[serde(default)]
pub struct Transform {
    /// Meters, `f64`.
    pub translation: DVec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: DVec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn from_xyz(x: f64, y: f64, z: f64) -> Self {
        Self::from_translation(DVec3::new(x, y, z))
    }

    pub fn from_translation(translation: DVec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    pub fn from_rotation(rotation: Quat) -> Self {
        Self {
            rotation,
            ..Self::IDENTITY
        }
    }

    pub fn from_scale(scale: Vec3) -> Self {
        Self {
            scale,
            ..Self::IDENTITY
        }
    }

    pub fn with_translation(mut self, translation: DVec3) -> Self {
        self.translation = translation;
        self
    }

    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    /// Uniform scale shortcut.
    pub fn with_uniform_scale(mut self, s: f32) -> Self {
        self.scale = Vec3::splat(s);
        self
    }

    /// Rotate so that −Z points at `target` with `up` as the up hint.
    pub fn looking_at(mut self, target: DVec3, up: Vec3) -> Self {
        self.look_at(target, up);
        self
    }

    /// Rotate so that −Z points at `target` with `up` as the up hint.
    pub fn look_at(&mut self, target: DVec3, up: Vec3) {
        let dir = (target - self.translation).as_vec3();
        self.look_to(dir, up);
    }

    /// Rotate so that −Z points along `direction`.
    pub fn look_to(&mut self, direction: Vec3, up: Vec3) {
        if let Some(rot) = look_rotation(direction, up) {
            self.rotation = rot;
        }
    }

    /// −Z in world space (ignoring parents).
    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }

    pub fn back(&self) -> Vec3 {
        self.rotation * Vec3::Z
    }

    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    pub fn left(&self) -> Vec3 {
        self.rotation * Vec3::NEG_X
    }

    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    pub fn down(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Y
    }

    /// Rotate by `rotation` in world (parent) space.
    pub fn rotate(&mut self, rotation: Quat) {
        self.rotation = (rotation * self.rotation).normalize();
    }

    /// Rotate by `rotation` in local space.
    pub fn rotate_local(&mut self, rotation: Quat) {
        self.rotation = (self.rotation * rotation).normalize();
    }

    pub fn rotate_x(&mut self, angle: f32) {
        self.rotate(Quat::from_rotation_x(angle));
    }

    pub fn rotate_y(&mut self, angle: f32) {
        self.rotate(Quat::from_rotation_y(angle));
    }

    pub fn rotate_z(&mut self, angle: f32) {
        self.rotate(Quat::from_rotation_z(angle));
    }

    pub fn rotate_local_x(&mut self, angle: f32) {
        self.rotate_local(Quat::from_rotation_x(angle));
    }

    pub fn rotate_local_y(&mut self, angle: f32) {
        self.rotate_local(Quat::from_rotation_y(angle));
    }

    /// Move in parent space.
    pub fn translate(&mut self, delta: Vec3) {
        self.translation += delta.as_dvec3();
    }

    /// Move along this transform's own axes (e.g. `Vec3::NEG_Z` = forward).
    pub fn translate_local(&mut self, delta: Vec3) {
        self.translation += (self.rotation * delta).as_dvec3();
    }

    /// Apply a point from local space to parent space.
    pub fn transform_point(&self, point: DVec3) -> DVec3 {
        self.to_affine().transform_point3(point)
    }

    /// `self` followed by `child` (child expressed in self's space).
    pub fn mul_transform(&self, child: &Transform) -> Transform {
        Transform {
            translation: self.transform_point(child.translation),
            rotation: (self.rotation * child.rotation).normalize(),
            scale: self.scale * child.scale,
        }
    }

    pub fn to_affine(&self) -> DAffine3 {
        DAffine3::from_scale_rotation_translation(
            self.scale.as_dvec3(),
            self.rotation.as_dquat(),
            self.translation,
        )
    }

    /// Interpolate: lerp translation/scale, slerp rotation.
    pub fn lerp(&self, other: &Transform, t: f32) -> Transform {
        Transform {
            translation: self.translation.lerp(other.translation, t as f64),
            rotation: self.rotation.slerp(other.rotation, t),
            scale: self.scale.lerp(other.scale, t),
        }
    }
}

/// Rotation whose −Z axis points along `direction`, with `up` as a hint. `None` if degenerate.
pub fn look_rotation(direction: Vec3, up: Vec3) -> Option<Quat> {
    let back = (-direction).try_normalize()?;
    let right = up
        .cross(back)
        .try_normalize()
        .or_else(|| Vec3::X.cross(back).try_normalize())
        .or_else(|| Vec3::Z.cross(back).try_normalize())?;
    let up = back.cross(right);
    Some(Quat::from_mat3(&Mat3::from_cols(right, up, back)).normalize())
}

/// Computed world-space transform. Written by the engine in `PostUpdate`; read it, don't
/// write it (write [`Transform`] instead).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct GlobalTransform(pub DAffine3);

impl Default for GlobalTransform {
    fn default() -> Self {
        Self(DAffine3::IDENTITY)
    }
}

impl From<Transform> for GlobalTransform {
    fn from(t: Transform) -> Self {
        Self(t.to_affine())
    }
}

impl GlobalTransform {
    pub const IDENTITY: Self = Self(DAffine3::IDENTITY);

    pub fn translation(&self) -> DVec3 {
        self.0.translation
    }

    /// Rotation with scale removed.
    pub fn rotation(&self) -> Quat {
        self.to_transform().rotation
    }

    pub fn scale(&self) -> Vec3 {
        self.to_transform().scale
    }

    /// Decompose into translation / rotation / scale (shear is lost).
    pub fn to_transform(&self) -> Transform {
        let (scale, rotation, translation) = self.0.to_scale_rotation_translation();
        Transform {
            translation,
            rotation: rotation.as_quat().normalize(),
            scale: scale.as_vec3(),
        }
    }

    fn axis(&self, local: DVec3) -> Vec3 {
        (self.0.matrix3 * local).normalize_or_zero().as_vec3()
    }

    pub fn forward(&self) -> Vec3 {
        self.axis(DVec3::NEG_Z)
    }

    pub fn back(&self) -> Vec3 {
        self.axis(DVec3::Z)
    }

    pub fn right(&self) -> Vec3 {
        self.axis(DVec3::X)
    }

    pub fn left(&self) -> Vec3 {
        self.axis(DVec3::NEG_X)
    }

    pub fn up(&self) -> Vec3 {
        self.axis(DVec3::Y)
    }

    pub fn transform_point(&self, p: DVec3) -> DVec3 {
        self.0.transform_point3(p)
    }

    pub fn transform_vector(&self, v: Vec3) -> Vec3 {
        (self.0.matrix3 * v.as_dvec3()).as_vec3()
    }

    /// This transform applied to a child's local transform.
    pub fn mul_transform(&self, child: &Transform) -> GlobalTransform {
        GlobalTransform(self.0 * child.to_affine())
    }

    /// `f32` model matrix relative to `origin` (normally the camera position).
    /// This is the only place large-world `f64` positions become `f32`.
    pub fn matrix_relative_to(&self, origin: DVec3) -> Mat4 {
        let m: DMat3 = self.0.matrix3;
        let t = (self.0.translation - origin).as_vec3();
        Mat4::from_cols(
            m.x_axis.as_vec3().extend(0.0),
            m.y_axis.as_vec3().extend(0.0),
            m.z_axis.as_vec3().extend(0.0),
            Vec4::new(t.x, t.y, t.z, 1.0),
        )
    }

    /// World → local.
    pub fn inverse(&self) -> DAffine3 {
        self.0.inverse()
    }

    pub fn affine(&self) -> DAffine3 {
        self.0
    }

    pub fn from_translation_rotation(translation: DVec3, rotation: Quat) -> Self {
        Self(DAffine3::from_rotation_translation(
            rotation.as_dquat(),
            translation,
        ))
    }
}

/// Smooths rendering of entities that move in `FixedUpdate` (physics bodies, simulated
/// characters). The renderer draws them between their last two simulated states, so motion
/// looks smooth even when the display rate differs from the simulation rate.
///
/// After teleporting an entity, call [`TransformInterpolation::reset`] (or just remove and
/// re-add the component) to avoid a one-frame streak.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct TransformInterpolation {
    /// State at the start of the most recent fixed step.
    pub previous: Option<Transform>,
}

impl TransformInterpolation {
    pub fn reset(&mut self) {
        self.previous = None;
    }

    fn visual(&self, current: &Transform, alpha: f32) -> Transform {
        match &self.previous {
            Some(prev) => prev.lerp(current, alpha),
            None => *current,
        }
    }
}

/// Engine system (`FixedPreUpdate`): remember where interpolated entities started this step.
pub fn snapshot_interpolation_system(mut q: Query<(&Transform, &mut TransformInterpolation)>) {
    q.par_iter_mut()
        .for_each(|(t, mut i)| i.previous = Some(*t));
}

type RootQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        Option<&'static TransformInterpolation>,
        &'static mut GlobalTransform,
        Option<&'static Children>,
    ),
    Without<ChildOf>,
>;

type ChildQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        Option<&'static TransformInterpolation>,
        &'static mut GlobalTransform,
        Option<&'static Children>,
    ),
    With<ChildOf>,
>;

/// Children to recompute even when their parent stayed put: moved, newly parented or spawned
/// (inserting a component counts as changing it), or drawn between fixed steps.
type MovedChildQuery<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static ChildOf),
    Or<(
        Changed<Transform>,
        Changed<ChildOf>,
        With<TransformInterpolation>,
    )>,
>;

/// Where a queued child finds its parent's world transform.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum ParentWorld {
    /// Just computed by the parent's walk.
    Known(GlobalTransform),
    /// Read when the child is reached, so a parent updated earlier in the walk is seen.
    Read(Entity),
}

/// Roots to recompute: moved or spawned, or drawn between fixed steps.
type MovedRootQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        Option<&'static TransformInterpolation>,
        &'static mut GlobalTransform,
        Option<&'static Children>,
    ),
    (
        Without<ChildOf>,
        Or<(Changed<Transform>, With<TransformInterpolation>)>,
    ),
>;

/// Engine system (`PostUpdate`, [`crate::schedule::UpdateSet::Propagate`]): compute every
/// [`GlobalTransform`] from the hierarchy.
///
/// Only what can have changed is recomputed: roots that moved (in parallel) or were just
/// unparented, the subtree under an entity whose world transform changed, and every child
/// that moved itself (see [`MovedChildQuery`]). A still valley of 37 k trees with three drawn
/// parts each then costs filtered passes instead of a full walk (5.4 ms → 0.7 ms). Unity
/// and Unreal skip unmoved hierarchies the same way (dirty transforms), as Bevy does since
/// 0.16. So write a [`GlobalTransform`] only together with its [`Transform`], as attachments
/// do: a world transform written alone is no longer overwritten the next frame.
#[allow(clippy::type_complexity)]
pub fn propagate_transforms_system(
    fixed: Res<FixedTime>,
    mut roots: ParamSet<(MovedRootQuery, RootQuery)>,
    mut children: ChildQuery,
    moved: MovedChildQuery,
    mut unparented: RemovedComponents<ChildOf>,
    mut stack: Local<Vec<(Entity, ParentWorld)>>,
    changed_roots: Local<std::sync::Mutex<Vec<(Entity, GlobalTransform)>>>,
) {
    let alpha = fixed.alpha();
    roots
        .p0()
        .par_iter_mut()
        .for_each(|(entity, t, interp, mut global, kids)| {
            let local = interp.map_or(*t, |i| i.visual(t, alpha));
            let new = GlobalTransform::from(local);
            // A world transform written with its Transform (as attachments and teleports do)
            // is already right, but its children still have to follow.
            let written = global.is_changed();
            let moved = *global != new;
            if moved {
                *global = new;
            }
            if (moved || written)
                && kids.is_some()
                && let Ok(mut changed) = changed_roots.lock()
            {
                changed.push((entity, new));
            }
        });

    let mut roots = roots.p1();
    stack.clear();
    for entity in unparented.read() {
        if let Ok((_, t, interp, mut global, kids)) = roots.get_mut(entity) {
            let local = interp.map_or(*t, |i| i.visual(t, alpha));
            let new = GlobalTransform::from(local);
            if *global != new {
                *global = new;
                if let Some(kids) = kids {
                    stack.extend(kids.iter().map(|k| (k, ParentWorld::Known(new))));
                }
            }
        }
    }
    if let Ok(mut changed) = changed_roots.lock() {
        for (root, global) in changed.drain(..) {
            if let Ok((_, _, _, _, Some(kids))) = roots.get(root) {
                stack.extend(kids.iter().map(|k| (k, ParentWorld::Known(global))));
            }
        }
    }
    stack.extend(
        moved
            .iter()
            .map(|(e, c)| (e, ParentWorld::Read(c.parent()))),
    );

    while let Some((entity, parent)) = stack.pop() {
        let parent = match parent {
            ParentWorld::Known(g) => g,
            ParentWorld::Read(p) => match roots.get(p) {
                Ok((_, _, _, g, _)) => *g,
                Err(_) => match children.get(p) {
                    Ok((_, _, g, _)) => *g,
                    Err(_) => continue,
                },
            },
        };
        let Ok((t, interp, mut global, kids)) = children.get_mut(entity) else {
            continue;
        };
        let local = interp.map_or(*t, |i| i.visual(t, alpha));
        let new = parent.mul_transform(&local);
        if *global == new {
            continue;
        }
        *global = new;
        if let Some(kids) = kids {
            stack.extend(kids.iter().map(|k| (k, ParentWorld::Known(new))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[test]
    fn hierarchy_propagates_with_f64_precision() {
        let mut app = App::new();
        let far = 1.5e11; // one astronomical unit
        let parent = app
            .world
            .spawn(
                Transform::from_xyz(far, 0.0, 0.0)
                    .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
            )
            .id();
        let child = app
            .world
            .spawn((Transform::from_xyz(0.0, 0.0, -0.001), ChildOf(parent)))
            .id();
        app.update();
        let g = app.world.get::<GlobalTransform>(child).unwrap();
        // −Z rotated +90° about Y points to −X: the child is 1 mm toward −X.
        let expected = DVec3::new(far - 0.001, 0.0, 0.0);
        assert!(
            (g.translation() - expected).length() < 1e-6,
            "{:?}",
            g.translation()
        );
    }

    #[test]
    fn children_follow_a_root_whose_world_transform_was_written_with_it() {
        let mut app = App::new();
        let root = app.world.spawn(Transform::from_xyz(10.0, 0.0, 0.0)).id();
        let child = app
            .world
            .spawn((Transform::from_xyz(1.0, 0.0, 0.0), ChildOf(root)))
            .id();
        app.update();
        assert!((world_x(&app, child) - 11.0).abs() < 1e-9);
        // A teleport writes both, so the root's world transform is already right.
        let t = Transform::from_xyz(500.0, 0.0, 0.0);
        app.world
            .entity_mut(root)
            .insert((t, GlobalTransform::from(t)));
        app.update();
        assert!(
            (world_x(&app, child) - 501.0).abs() < 1e-9,
            "{}",
            world_x(&app, child)
        );
    }

    fn world_x(app: &App, e: Entity) -> f64 {
        app.world.get::<GlobalTransform>(e).unwrap().translation().x
    }

    #[test]
    fn only_what_moved_is_walked_but_every_change_lands() {
        let mut app = App::new();
        let a = app.world.spawn(Transform::from_xyz(10.0, 0.0, 0.0)).id();
        let b = app.world.spawn(Transform::from_xyz(100.0, 0.0, 0.0)).id();
        let child = app
            .world
            .spawn((Transform::from_xyz(1.0, 0.0, 0.0), ChildOf(a)))
            .id();
        let grandchild = app
            .world
            .spawn((Transform::from_xyz(0.5, 0.0, 0.0), ChildOf(child)))
            .id();
        app.update();
        assert_eq!(world_x(&app, grandchild), 11.5);

        // A still frame keeps everything where it was.
        app.update();
        assert_eq!(world_x(&app, grandchild), 11.5);

        // Only the grandchild moves, under still parents.
        app.world
            .get_mut::<Transform>(grandchild)
            .unwrap()
            .translation
            .x = 2.0;
        app.update();
        assert_eq!(world_x(&app, grandchild), 13.0);

        // Only the root moves: the whole subtree follows.
        app.world.get_mut::<Transform>(a).unwrap().translation.x = 20.0;
        app.update();
        assert_eq!(world_x(&app, child), 21.0);
        assert_eq!(world_x(&app, grandchild), 23.0);

        // The root and the grandchild move in one frame.
        app.world.get_mut::<Transform>(a).unwrap().translation.x = 30.0;
        app.world
            .get_mut::<Transform>(grandchild)
            .unwrap()
            .translation
            .x = 3.0;
        app.update();
        assert_eq!(world_x(&app, grandchild), 34.0);

        // Moved to another still parent.
        app.world.entity_mut(child).insert(ChildOf(b));
        app.update();
        assert_eq!(world_x(&app, child), 101.0);
        assert_eq!(world_x(&app, grandchild), 104.0);

        // Unparented: its own transform is its world transform.
        app.world.entity_mut(child).remove::<ChildOf>();
        app.update();
        assert_eq!(world_x(&app, child), 1.0);
        assert_eq!(world_x(&app, grandchild), 4.0);

        // Spawned under a still parent.
        let late = app
            .world
            .spawn((Transform::from_xyz(5.0, 0.0, 0.0), ChildOf(b)))
            .id();
        app.update();
        assert_eq!(world_x(&app, late), 105.0);
    }

    #[test]
    fn looking_at_points_forward() {
        let t = Transform::from_xyz(0.0, 0.0, 0.0).looking_at(DVec3::new(10.0, 0.0, 0.0), Vec3::Y);
        assert!(t.forward().abs_diff_eq(Vec3::X, 1e-5));
        assert!(t.up().abs_diff_eq(Vec3::Y, 1e-5));
    }

    #[test]
    fn camera_relative_matrix_is_small() {
        let g = GlobalTransform::from(Transform::from_xyz(1.0e12 + 3.0, 0.0, 0.0));
        let m = g.matrix_relative_to(DVec3::new(1.0e12, 0.0, 0.0));
        assert!((m.w_axis.x - 3.0).abs() < 1e-6);
    }
}
