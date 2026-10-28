//! Small animations of the yard's props: the bead-curtain ball rolls, the press's handwheel
//! turns as the screw drives the ram, and the pedestal gun swings onto its next target.

use std::f32::consts::TAU;

use engine_core::prelude::*;

use super::{DemoTurret, hash01};

/// Rolls a ball over the ground as it moves (rotation from the distance it travels).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoRoll {
    pub radius: f32,
    pub last: Option<DVec3>,
}

/// Turns a handwheel on a screw as the ram it drives moves up and down: one turn per `lead`
/// meters of travel of `ram`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoScrewWheel {
    pub ram: Entity,
    pub lead: f32,
    pub last: Option<f64>,
    pub angle: f32,
}

/// Which way a part of a gun mount turns to aim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AimAxis {
    /// About +Y (the yoke on its turntable).
    Yaw,
    /// About +X (the gun on its trunnions).
    Pitch,
}

/// Points a gun-mount part at the next target of `turret`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoAim {
    pub turret: Entity,
    pub axis: AimAxis,
    pub angle: f32,
}

/// Adds the prop animations (after the demo's own motion systems).
pub fn register(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        (roll_balls, turn_screw_wheels, aim_guns)
            .chain()
            .after(super::respawn_volumes)
            .in_set(SimSet::Gameplay),
    );
}

/// Where `turret` aims its `n`th shot (the same pattern `fire_turrets` shoots).
fn turret_aim(turret: &DemoTurret, n: u32) -> DVec3 {
    if n % 48 > 40 {
        // Resting: back to the middle of the target.
        return turret.target;
    }
    turret.target
        + DVec3::new(
            ((hash01(n * 2) * 2.0 - 1.0) * turret.spread.x) as f64,
            ((hash01(n * 2 + 1) * 2.0 - 1.0) * turret.spread.y) as f64,
            0.0,
        )
}

/// Turn [`DemoRoll`] balls by the distance they moved since the last step.
pub fn roll_balls(mut balls: Query<(&mut Transform, &mut DemoRoll)>) {
    for (mut t, mut roll) in &mut balls {
        let p = t.translation;
        let last = roll.last.replace(p).unwrap_or(p);
        let d = (p - last).as_vec3() * Vec3::new(1.0, 0.0, 1.0);
        let dist = d.length();
        if dist < 1e-6 || roll.radius <= 0.0 {
            continue;
        }
        let axis = Vec3::Y.cross(d / dist);
        t.rotation = (Quat::from_axis_angle(axis, dist / roll.radius) * t.rotation).normalize();
    }
}

/// Turn [`DemoScrewWheel`] handwheels as their rams move.
pub fn turn_screw_wheels(
    rams: Query<&Transform, Without<DemoScrewWheel>>,
    mut wheels: Query<(&mut Transform, &mut DemoScrewWheel)>,
) {
    for (mut t, mut w) in &mut wheels {
        let Ok(ram) = rams.get(w.ram) else {
            continue;
        };
        let y = ram.translation.y;
        let last = w.last.replace(y).unwrap_or(y);
        if w.lead > 0.0 {
            // Screwing down turns it clockwise seen from above.
            w.angle = (w.angle + ((last - y) as f32 / w.lead) * TAU).rem_euclid(TAU);
        }
        t.rotation = Quat::from_rotation_y(-w.angle);
    }
}

/// Slew [`DemoAim`] gun-mount parts toward their turret's next target.
pub fn aim_guns(
    time: Res<Time>,
    turrets: Query<(&Transform, &DemoTurret), Without<DemoAim>>,
    mut parts: Query<(&mut Transform, &mut DemoAim)>,
) {
    let dt = time.delta_for(None);
    // Slews most of the way within a fraction of the firing period.
    let k = 1.0 - (-14.0 * dt).exp();
    for (mut t, mut aim) in &mut parts {
        let Ok((tt, turret)) = turrets.get(aim.turret) else {
            continue;
        };
        let target = turret
            .manned
            .unwrap_or_else(|| turret_aim(turret, turret.shots + 1));
        let dir = (target - tt.translation)
            .as_vec3()
            .normalize_or(Vec3::NEG_Z);
        let goal = match aim.axis {
            AimAxis::Yaw => (-dir.x).atan2(-dir.z),
            AimAxis::Pitch => dir.y.clamp(-1.0, 1.0).asin(),
        };
        aim.angle += (goal - aim.angle) * k;
        t.rotation = match aim.axis {
            AimAxis::Yaw => Quat::from_rotation_y(aim.angle),
            AimAxis::Pitch => Quat::from_rotation_x(aim.angle),
        };
    }
}
