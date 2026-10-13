//! A gaff-rigged sailing dinghy that sails itself: the simulated sail drives the hull (through
//! `ApplyImpulse`, see [`Cloth`]), and a helmsman trims the sheet, steers with the rudder and
//! keeps it on a beam reach back and forth across the wind, gybing at each end of its lane.
//!
//! Floating is the physics module's job (every dynamic body in `Water` floats); this file adds
//! what a hull with a centerboard does beyond floating, as forces on the body: it resists
//! sliding sideways far more than moving ahead, the crew keeps it from heeling over, and the
//! rudder turns it. Without water under it, the boat waits at its mooring with the sail up.

use std::f32::consts::FRAC_PI_2;

use engine_core::prelude::*;
use engine_world::physics::{ColliderShape, ExternalForce, MassProperties, RigidBody, Velocity};
use engine_world::{Env, Water};

use crate::{Cloth, SoftMaterial};

/// The boat's frame: bow toward −Z, +X starboard, +Y up, origin on the designed waterline
/// amidships. The rig turns about the mast; the rudder about its hinge.
pub mod dims {
    use super::super::props_boat::dims as boat;

    /// Mast axis (x = 0), meters along the boat.
    pub const MAST_Z: f32 = boat::MAST_Z;
    /// Rudder hinge (x = 0, y = 0), meters along the boat.
    pub const RUDDER_Z: f32 = boat::RUDDER_Z;
    /// Sail corners in rig space (the rig's ZY plane, x = 0), as `[z, y]`: throat, peak,
    /// clew and tack.
    pub const SAIL: [[f32; 2]; 4] = [
        [boat::SAIL_THROAT[2], boat::SAIL_THROAT[1]],
        [boat::SAIL_PEAK[2], boat::SAIL_PEAK[1]],
        [boat::SAIL_CLEW[2], boat::SAIL_CLEW[1]],
        [boat::SAIL_TACK[2], boat::SAIL_TACK[1]],
    ];
    /// Hull and crew, kg.
    pub const MASS_KG: f32 = 170.0;
    /// Center of mass (ballast and crew sit low), boat frame.
    pub const CENTER_OF_MASS: [f32; 3] = [0.0, -0.12, 0.15];
    /// Principal moments of inertia (pitch, yaw, roll), kg·m².
    pub const INERTIA: [f32; 3] = [300.0, 320.0, 110.0];
}

/// Sails a boat around `home`: back and forth across the wind, `range` meters each way.
#[derive(Component, Debug, Clone)]
pub struct Helm {
    pub home: DVec3,
    pub range: f32,
    pub rig: Entity,
    pub rudder: Entity,
    /// Which way along the lane: +1 or −1.
    pub leg: f32,
    /// Gybing onto the next leg: bearing away until the wind is astern.
    pub gybing: bool,
    /// True wind direction (where it blows to), smoothed over several seconds.
    pub wind: Vec3,
    /// Boom angle from the centerline, radians (+ = to starboard).
    pub boom: f32,
    /// Rudder angle, radians (+ = trailing edge to starboard, bow turns to starboard).
    pub rudder_angle: f32,
    /// Seconds it has been on its side (righted after a while).
    pub capsized_for: f32,
    /// Where it waits when there is no water.
    pub mooring: Transform,
    /// Start over from the mooring on the next step (`softbody.sailboat_home`).
    pub restart: bool,
}

/// Lateral resistance of hull and centerboard: N·s/m and N·s²/m².
const SIDE_DRAG: (f32, f32) = (420.0, 650.0);
/// Hull resistance going ahead, on top of the water drag the physics module applies.
const AHEAD_DRAG: (f32, f32) = (8.0, 22.0);
/// Righting moment of crew and hull form against heel and pitch, N·m per radian, and its
/// damping, N·m·s.
const RIGHTING: (f32, f32) = (2600.0, 520.0);
const PITCH: (f32, f32) = (9000.0, 1500.0);
/// Steering: yaw acceleration per radian off course and damping, at full rudder bite.
const STEER: (f32, f32) = (3.0, 3.5);
/// Fastest the boom swings (a gybe), rad/s.
const BOOM_RATE: f32 = 1.6;

/// Spawn the dinghy on the water at `home` (a point on the still surface with a few meters
/// of water under it): it sails `range` meters each way across the wind from there.
pub fn spawn_sailboat(commands: &mut Commands, home: DVec3, range: f32) -> Entity {
    let mooring = Transform::from_translation(home);
    let boat = commands
        .spawn((
            Name::new("Sailing dinghy"),
            mooring,
            TransformInterpolation::default(),
            RigidBody {
                linear_damping: 0.0,
                angular_damping: 0.1,
                can_sleep: false,
                ..RigidBody::dynamic().with_mass_properties(
                    dims::MASS_KG,
                    Vec3::from_array(dims::CENTER_OF_MASS),
                    Vec3::from_array(dims::INERTIA),
                )
            },
            hull_collider(),
            Velocity::default(),
            ExternalForce::default(),
        ))
        .id();
    let rig = commands
        .spawn((
            Name::new("Dinghy rig"),
            Transform::from_xyz(0.0, 0.0, dims::MAST_Z as f64),
            TransformInterpolation::default(),
            ChildOf(boat),
        ))
        .id();
    // The sail lies in the rig's ZY plane: its local X runs aft along the boom.
    let corners = dims::SAIL;
    commands.spawn((
        Name::new("Dinghy sail"),
        Transform::from_rotation(Quat::from_rotation_y(-FRAC_PI_2)),
        Cloth::sail(corners, sailcloth()),
        ChildOf(rig),
    ));
    let rudder = commands
        .spawn((
            Name::new("Dinghy rudder"),
            Transform::from_xyz(0.0, 0.0, dims::RUDDER_Z as f64),
            TransformInterpolation::default(),
            ChildOf(boat),
        ))
        .id();
    commands.entity(boat).insert(Helm {
        home,
        range,
        rig,
        rudder,
        leg: 1.0,
        gybing: false,
        wind: Vec3::ZERO,
        boom: 0.0,
        rudder_angle: 0.0,
        capsized_for: 0.0,
        mooring,
        restart: false,
    });
    dress(commands, boat, rig, rudder);
    boat
}

/// The modeled hull, rig and rudder.
fn dress(commands: &mut Commands, boat: Entity, rig: Entity, rudder: Entity) {
    let models = super::boat_models(commands);
    models.hull.attach(commands, boat, Transform::default());
    models.rig.attach(commands, rig, Transform::default());
    models.rudder.attach(commands, rudder, Transform::default());
}

/// Cream sailcloth: smooth, a little sheen, glowing when the sun is behind it.
fn sailcloth() -> SoftMaterial {
    SoftMaterial {
        roughness: 0.62,
        sheen: 0.15,
        subsurface: 0.45,
        ..SoftMaterial::hex(0xece3cf)
    }
}

/// The hull as the physics module floats it: a box from the keel up, sized so it floats on
/// its designed waterline (the module lifts a body by the fraction of its box under water,
/// and drags it in proportion too, so a tall box drags less).
fn hull_collider() -> engine_world::physics::Collider {
    // Draft 0.20 of a 1.4 m tall box: lift = ρ g V (0.20 / 1.4) = m g.
    let depth = 1.4f32;
    let volume = dims::MASS_KG / (1000.0 * 0.20 / depth);
    let length = 3.0f32;
    let width = volume / (length * depth);
    engine_world::physics::Collider {
        shape: ColliderShape::Compound(vec![(
            Vec3::new(0.0, depth * 0.5 - 0.20, 0.0),
            Quat::IDENTITY,
            ColliderShape::Cuboid {
                half_extents: Vec3::new(width * 0.5, depth * 0.5, length * 0.5),
            },
        )]),
        ..Default::default()
    }
}

/// Signed angle about +Y from `from` to `to` (horizontal directions): + turns toward port.
fn yaw_between(from: Vec3, to: Vec3) -> f32 {
    let (a, b) = (Vec2::new(from.x, from.z), Vec2::new(to.x, to.z));
    // In XZ seen from above with +Y up, turning −Z toward −X (port) is positive.
    (-a.perp_dot(b)).atan2(a.dot(b))
}

/// Where to steer: across the wind on this `leg`, a little up or down to stay near the lane
/// through `home`, or (gybing) dead downwind first. Horizontal unit vector.
pub fn course(wind: Vec3, offset: Vec3, leg: f32, gybing: bool) -> Vec3 {
    let w = Vec3::new(wind.x, 0.0, wind.z).normalize_or(Vec3::X);
    if gybing {
        return w;
    }
    let across = Vec3::Y.cross(w) * leg;
    // Downwind of the lane: head up a little (toward the wind), and the other way.
    let drift = offset.dot(w);
    let up = (drift * 0.04).clamp(-0.35, 0.35);
    (across * up.cos() - w * up.sin()).normalize()
}

/// Boom angle for a wind from `apparent_from` (horizontal, in the boat frame: x starboard,
/// z aft): half the angle off the bow, on the side away from the wind.
pub fn trim(apparent_from: Vec3) -> f32 {
    let off_bow = apparent_from.x.atan2(-apparent_from.z); // + = from starboard
    let angle = (off_bow.abs() * 0.5).clamp(0.12, 1.45);
    -off_bow.signum() * angle
}

/// Sail every dinghy: sheet, rudder, hull forces, gybes, and resets after a capsize.
#[allow(clippy::type_complexity)]
pub fn sail_boats(
    time: Res<Time>,
    env: Env,
    water: Option<Res<Water>>,
    mut boats: Query<(
        &mut Helm,
        &mut Transform,
        &mut Velocity,
        &mut ExternalForce,
        Option<&MassProperties>,
        Option<&mut TransformInterpolation>,
    )>,
    mut parts: Query<&mut Transform, Without<Helm>>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (mut helm, mut t, mut vel, mut push, mass, interp) in &mut boats {
        let pos = t.translation;
        let afloat = water
            .as_ref()
            .and_then(|w| w.sample(pos))
            .is_some_and(|s| s.depth > 0.6 && (pos.y - s.surface_height).abs() < 2.0);
        let rot = t.rotation;
        let up = rot * Vec3::Y;
        if up.y < 0.35 {
            helm.capsized_for += dt;
        } else {
            helm.capsized_for = 0.0;
        }
        let lost = (pos - helm.home).length() > (helm.range * 2.5) as f64;
        let restart = std::mem::take(&mut helm.restart);
        if !afloat || lost || restart || helm.capsized_for > 4.0 {
            // Wait at the mooring (or start over there), head to wind.
            let wind = env.wind(pos);
            let into = -Vec3::new(wind.x, 0.0, wind.z).normalize_or(Vec3::X);
            *t = helm
                .mooring
                .with_rotation(Quat::from_rotation_arc(Vec3::NEG_Z, into));
            helm.boom = 0.0;
            helm.rudder_angle = 0.0;
            if let Ok(mut r) = parts.get_mut(helm.rig) {
                r.rotation = Quat::IDENTITY;
            }
            *vel = Velocity::default();
            *push = ExternalForce::default();
            if let Some(mut i) = interp {
                i.reset();
            }
            helm.capsized_for = 0.0;
            helm.gybing = false;
            continue;
        }

        let forward = rot * Vec3::NEG_Z;
        let side = rot * Vec3::X;
        let com = pos + (rot * mass.map_or(Vec3::ZERO, |m| m.center_of_mass)).as_dvec3();
        // True wind at the sail, smoothed so gusts don't flick the course around.
        let true_wind = env.wind(com + DVec3::Y * 3.0);
        let k = (dt / 6.0).min(1.0);
        helm.wind = if helm.wind == Vec3::ZERO {
            true_wind
        } else {
            helm.wind.lerp(true_wind, k)
        };

        // Course: along the lane, gybing round at each end.
        let offset = (pos - helm.home).as_vec3();
        let w = Vec3::new(helm.wind.x, 0.0, helm.wind.z).normalize_or(Vec3::X);
        let across = Vec3::Y.cross(w) * helm.leg;
        if !helm.gybing && offset.dot(across) > helm.range {
            helm.gybing = true;
        }
        let flat_forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::NEG_Z);
        if helm.gybing && flat_forward.dot(w) > 0.94 {
            helm.gybing = false;
            helm.leg = -helm.leg;
        }
        let want = course(helm.wind, offset, helm.leg, helm.gybing);
        let error = yaw_between(flat_forward, want);

        // Rudder: bites with speed through the water.
        let v = vel.linear;
        let ahead = v.dot(forward);
        let bite = (ahead.abs() / 1.5).clamp(0.6, 1.4);
        let yaw_rate = vel.angular.dot(Vec3::Y);
        let inertia = Vec3::from_array(dims::INERTIA);
        let yaw = (STEER.0 * error - STEER.1 * yaw_rate) * inertia.y * bite;
        let yaw = yaw.clamp(-900.0, 900.0);
        let rudder_target = (-error * 1.6).clamp(-0.6, 0.6);
        helm.rudder_angle += (rudder_target - helm.rudder_angle) * (dt * 4.0).min(1.0);

        // Sheet: half the apparent wind angle, to leeward; the boom swings across in a gybe.
        let apparent = true_wind - v;
        let from = rot.conjugate() * -Vec3::new(apparent.x, 0.0, apparent.z);
        let boom_target = trim(from);
        let step = BOOM_RATE * dt;
        helm.boom += (boom_target - helm.boom).clamp(-step, step);

        // Hull: slides ahead easily, sideways hardly at all; the crew holds it upright.
        let sideways = v.dot(side);
        let mut force = -side * (SIDE_DRAG.0 * sideways + SIDE_DRAG.1 * sideways * sideways.abs());
        force -= forward * (AHEAD_DRAG.0 * ahead + AHEAD_DRAG.1 * ahead * ahead.abs());
        // Turning `up` back to vertical, split into heel (about the bow) and pitch.
        let upright = up.cross(Vec3::Y);
        let mut torque =
            forward * (RIGHTING.0 * upright.dot(forward) - RIGHTING.1 * vel.angular.dot(forward));
        torque += side * (PITCH.0 * upright.dot(side) - PITCH.1 * vel.angular.dot(side));
        torque += Vec3::Y * yaw;
        *push = ExternalForce {
            force,
            torque,
            at: None,
        };

        // Turn the rig and the rudder.
        if let Ok(mut r) = parts.get_mut(helm.rig) {
            r.rotation = Quat::from_rotation_y(helm.boom);
        }
        if let Ok(mut r) = parts.get_mut(helm.rudder) {
            r.rotation = Quat::from_rotation_y(helm.rudder_angle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steers_across_the_wind_and_gybes_through_downwind() {
        let wind = Vec3::new(5.0, 0.0, 0.0);
        let a = course(wind, Vec3::ZERO, 1.0, false);
        let b = course(wind, Vec3::ZERO, -1.0, false);
        assert!(a.dot(wind.normalize()).abs() < 1e-5 && b.dot(a) < -0.999);
        assert_eq!(course(wind, Vec3::ZERO, 1.0, true), Vec3::X);
        // Blown downwind of the lane, it heads up into the wind a little.
        let c = course(wind, Vec3::new(8.0, 0.0, 0.0), 1.0, false);
        assert!(c.x < -0.2, "{c}");
    }

    #[test]
    fn eases_the_boom_to_leeward_by_half_the_wind_angle() {
        // Wind from starboard abeam: boom out to port by about 45°.
        let boom = trim(Vec3::new(1.0, 0.0, 0.0));
        assert!((boom + FRAC_PI_2 * 0.5).abs() < 1e-3, "{boom}");
        // From port on the quarter: boom out to starboard, well eased.
        let boom = trim(Vec3::new(-1.0, 0.0, 1.0));
        assert!(boom > 1.1, "{boom}");
        // Close-hauled from starboard: boom near the centerline, to port.
        let boom = trim(Vec3::new(0.5, 0.0, -1.0));
        assert!((-0.3..-0.12).contains(&boom), "{boom}");
    }

    #[test]
    fn yaw_between_turns_toward_port_positive() {
        let port = yaw_between(Vec3::NEG_Z, Vec3::NEG_X);
        assert!((port - FRAC_PI_2).abs() < 1e-5, "{port}");
        let starboard = yaw_between(Vec3::NEG_Z, Vec3::X);
        assert!((starboard + FRAC_PI_2).abs() < 1e-5, "{starboard}");
    }
}
