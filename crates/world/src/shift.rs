//! Re-anchoring the world: a floating origin, or a new reference frame when the focus enters
//! another planet's sphere of influence.
//!
//! Whoever re-anchors (the space module) writes a [`WorldShift`] in [`WorldShiftSet::Write`].
//! In [`WorldShiftSet::Apply`] this crate moves everything the shared contracts hold: root
//! `Transform`s and their interpolation, physics and character velocities, forces, move
//! intents, world-anchored joints, gravity and the environment fields. Modules that keep
//! world-space state of their own (particle buffers, broadphase caches, simulation grids,
//! audio emitters) read the message and shift it too, in `Apply` so the frame renders right.
//! Both sets run in `PostUpdate` before transforms propagate: before `Write` everything is in
//! the old frame, after `Apply` everything is in the new one.

use engine_core::math::{DQuat, DVec2, DVec3, Quat, Vec3};
use engine_core::prelude::*;

use crate::locomotion::{LocomotionState, MoveIntent};
use crate::physics::{CharacterMove, CharacterState, ExternalForce, Joint, Velocity};
use crate::{FocusPoints, Gravity, Grid2, HeatField, SurfaceLayers, WindField};

/// The world moved to a new reference frame this frame: a point at `old` is now at
/// `rotation * old + translation`, and the old frame moves with `velocity` and spins at
/// `angular_velocity`, both seen from the new one (zero for a plain floating-origin shift).
///
/// Several shifts in one frame apply in order; [`WorldShift::then`] combines them.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct WorldShift {
    pub rotation: DQuat,
    /// Meters.
    pub translation: DVec3,
    /// Velocity of the old frame's origin in the new frame, m/s.
    pub velocity: DVec3,
    /// Angular velocity of the old frame in the new frame, rad/s.
    pub angular_velocity: DVec3,
}

impl Default for WorldShift {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl WorldShift {
    pub const IDENTITY: Self = Self {
        rotation: DQuat::IDENTITY,
        translation: DVec3::ZERO,
        velocity: DVec3::ZERO,
        angular_velocity: DVec3::ZERO,
    };

    /// A floating-origin move: everything moves by `translation`.
    pub fn translate(translation: DVec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    /// A position in the new frame.
    pub fn point(&self, old: DVec3) -> DVec3 {
        self.rotation * old + self.translation
    }

    /// A position in the old frame.
    pub fn inverse_point(&self, new: DVec3) -> DVec3 {
        self.rotation.inverse() * (new - self.translation)
    }

    /// A direction, normal, force or torque in the new frame (rotation only).
    pub fn direction(&self, v: Vec3) -> Vec3 {
        (self.rotation * v.as_dvec3()).as_vec3()
    }

    /// An orientation in the new frame.
    pub fn orientation(&self, q: Quat) -> Quat {
        (self.rotation * q.as_dquat()).normalize().as_quat()
    }

    /// The new-frame velocity of something moving at `v` (old frame) that is now at
    /// `new_position`: rotated, plus the old frame's own motion at that point.
    pub fn velocity_at(&self, new_position: DVec3, v: Vec3) -> Vec3 {
        let lever = new_position - self.translation;
        (self.rotation * v.as_dvec3() + self.velocity + self.angular_velocity.cross(lever))
            .as_vec3()
    }

    /// An angular velocity in the new frame.
    pub fn spin(&self, w: Vec3) -> Vec3 {
        (self.rotation * w.as_dvec3() + self.angular_velocity).as_vec3()
    }

    /// This shift followed by `next`, as one shift.
    pub fn then(&self, next: &WorldShift) -> WorldShift {
        let moved = next.rotation * self.translation;
        WorldShift {
            rotation: (next.rotation * self.rotation).normalize(),
            translation: moved + next.translation,
            velocity: next.rotation * self.velocity
                + next.velocity
                + next.angular_velocity.cross(moved),
            angular_velocity: next.rotation * self.angular_velocity + next.angular_velocity,
        }
    }

    /// True if the shift has no rotation (to within a nanoradian).
    pub fn is_translation_only(&self) -> bool {
        self.rotation.angle_between(DQuat::IDENTITY) < 1e-9
    }

    /// A root transform in the new frame.
    pub fn transform(&self, t: &Transform) -> Transform {
        Transform {
            translation: self.point(t.translation),
            rotation: self.orientation(t.rotation),
            scale: t.scale,
        }
    }
}

/// Where world shifts happen inside `PostUpdate`, before transforms propagate.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorldShiftSet {
    /// Write [`WorldShift`] here.
    Write,
    /// The shared state moves here; shift module caches here too.
    Apply,
}

pub(crate) fn register(app: &mut App) {
    app.add_message::<WorldShift>()
        .configure_sets(
            PostUpdate,
            (WorldShiftSet::Write, WorldShiftSet::Apply)
                .chain()
                .before(UpdateSet::Propagate),
        )
        .add_systems(
            PostUpdate,
            (shift_entities_system, shift_resources_system).in_set(WorldShiftSet::Apply),
        );
}

/// All shifts written this frame, combined.
fn combined(shifts: &mut MessageReader<WorldShift>) -> Option<WorldShift> {
    shifts.read().copied().reduce(|acc, next| acc.then(&next))
}

type RootQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        Option<&'static mut TransformInterpolation>,
    ),
    Without<ChildOf>,
>;

#[allow(clippy::too_many_arguments)]
fn shift_entities_system(
    mut shifts: MessageReader<WorldShift>,
    mut roots: RootQuery,
    mut velocities: Query<(&mut Velocity, Option<&GlobalTransform>)>,
    mut forces: Query<&mut ExternalForce>,
    mut characters: Query<(&mut CharacterState, Option<&GlobalTransform>)>,
    mut character_moves: Query<&mut CharacterMove>,
    mut intents: Query<&mut MoveIntent>,
    mut locomotion: Query<(&mut LocomotionState, Option<&GlobalTransform>)>,
    mut joints: Query<&mut Joint>,
) {
    let Some(s) = combined(&mut shifts) else {
        return;
    };
    // Velocities first: they need each entity's position, and `GlobalTransform` still holds
    // the old frame until transforms propagate after this set.
    let at = |g: Option<&GlobalTransform>| s.point(g.map_or(DVec3::ZERO, |g| g.translation()));
    for (mut v, g) in &mut velocities {
        let p = at(g);
        v.linear = s.velocity_at(p, v.linear);
        v.angular = s.spin(v.angular);
    }
    for (mut c, g) in &mut characters {
        let p = at(g);
        c.velocity = s.velocity_at(p, c.velocity);
        c.ground_normal = s.direction(c.ground_normal);
        c.wall_normal = c.wall_normal.map(|n| s.direction(n));
    }
    for (mut l, g) in &mut locomotion {
        let p = at(g);
        l.velocity = s.velocity_at(p, l.velocity);
    }
    for (mut t, interpolation) in &mut roots {
        *t = s.transform(&t);
        if let Some(mut i) = interpolation
            && let Some(previous) = i.previous
        {
            i.previous = Some(s.transform(&previous));
        }
    }
    for mut f in &mut forces {
        f.force = s.direction(f.force);
        f.torque = s.direction(f.torque);
        f.at = f.at.map(|p| s.point(p));
    }
    // Wanted velocities are set again every step; only their direction carries over.
    for mut m in &mut character_moves {
        m.velocity = s.direction(m.velocity);
    }
    for mut i in &mut intents {
        i.direction = s.direction(i.direction);
        i.look = i.look.map(|l| s.direction(l));
        i.dash_to = i.dash_to.map(|p| s.point(p));
    }
    for mut j in &mut joints {
        if j.body_b.is_none() {
            j.anchor_b = s.point(j.anchor_b);
        }
    }
}

fn shift_resources_system(
    mut shifts: MessageReader<WorldShift>,
    mut gravity: ResMut<Gravity>,
    mut focus: ResMut<FocusPoints>,
    mut surface: ResMut<SurfaceLayers>,
    mut heat: ResMut<HeatField>,
    mut wind: ResMut<WindField>,
) {
    let Some(s) = combined(&mut shifts) else {
        return;
    };
    gravity.uniform = s.direction(gravity.uniform);
    for body in &mut gravity.bodies {
        body.center = s.point(body.center);
    }
    for p in &mut focus.points {
        *p = s.point(*p);
    }
    wind.prevailing = s.direction(wind.prevailing);
    if s.is_translation_only() {
        // Exact: the grids keep their data and move their origin.
        let xz = DVec2::new(s.translation.x, s.translation.z);
        surface.grid.origin += xz;
        heat.grid.origin += xz;
        wind.local.origin += s.translation;
    } else {
        // A horizontal grid can't turn with the world: resample it around the focus.
        let center = focus.primary();
        surface.grid = resampled(&surface.grid, &s, center);
        heat.grid = resampled(&heat.grid, &s, center);
        // Gusts are short-lived: start the local wind afresh, as recentering does.
        let size = DVec3::from_array(wind.local.dims.map(|d| d as f64)) * wind.local.cell as f64;
        wind.local = crate::Grid3::new(
            center - size * 0.5,
            wind.local.cell,
            wind.local.dims,
            Vec3::ZERO,
        );
    }
}

/// `old` (in the old frame) as a grid of the same size and cell centered on `center` (new
/// frame), sampling the nearest old cell under each new cell center at the focus height.
fn resampled<T: Copy>(old: &Grid2<T>, s: &WorldShift, center: DVec3) -> Grid2<T> {
    let size = old.width as f64 * old.cell as f64;
    let mut next = Grid2::new(
        DVec2::new(center.x, center.z) - DVec2::splat(size * 0.5),
        old.cell,
        old.width,
        old.height,
        old.default,
    );
    for y in 0..next.height as i32 {
        for x in 0..next.width as i32 {
            let c = engine_core::math::IVec2::new(x, y);
            let p = s.inverse_point(next.cell_center3(c, center.y));
            if old.contains(p) {
                next.set(c, old.sample_nearest(p));
            }
        }
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tilted() -> WorldShift {
        WorldShift {
            rotation: DQuat::from_euler(engine_core::math::EulerRot::YXZ, 0.7, -0.3, 0.2),
            translation: DVec3::new(1.0e6, -20.0, 3.5e5),
            velocity: DVec3::new(30.0, 0.0, -12.0),
            angular_velocity: DVec3::new(0.0, 7.3e-5, 1.0e-5),
        }
    }

    #[test]
    fn points_round_trip() {
        let s = tilted();
        let p = DVec3::new(12.0, -4.0, 1.0e4);
        assert!((s.inverse_point(s.point(p)) - p).length() < 1e-6);
    }

    #[test]
    fn something_at_rest_moves_with_the_old_frame() {
        let s = tilted();
        let old = DVec3::new(100.0, 0.0, -50.0);
        let p = s.point(old);
        let v = s.velocity_at(p, Vec3::ZERO).as_dvec3();
        let want = s.velocity + s.angular_velocity.cross(s.rotation * old);
        assert!((v - want).length() < 1e-4, "{v} vs {want}");
    }

    #[test]
    fn two_shifts_combine_into_one() {
        let a = tilted();
        let b = WorldShift {
            rotation: DQuat::from_rotation_z(-1.1),
            translation: DVec3::new(-3.0, 4.0e4, 9.0),
            velocity: DVec3::new(0.0, -5.0, 2.0),
            angular_velocity: DVec3::new(2.0e-4, 0.0, 0.0),
        };
        let ab = a.then(&b);
        let p = DVec3::new(7.0, 8.0, -9.0);
        let v = Vec3::new(1.0, -2.0, 3.0);
        let w = Vec3::new(0.1, 0.2, -0.3);
        let (mid, new) = (a.point(p), b.point(a.point(p)));
        assert!((ab.point(p) - new).length() < 1e-6);
        let stepwise = b.velocity_at(new, a.velocity_at(mid, v));
        assert!((ab.velocity_at(new, v) - stepwise).length() < 1e-3);
        assert!((ab.spin(w) - b.spin(a.spin(w))).length() < 1e-6);
        for same in [WorldShift::IDENTITY.then(&a), a.then(&WorldShift::IDENTITY)] {
            assert!((same.point(p) - a.point(p)).length() < 1e-6);
            assert!((same.velocity_at(new, v) - a.velocity_at(new, v)).length() < 1e-4);
        }
    }
}
