//! Cruise: a spaceship's drive between planets and stars, Elite's supercruise model. Out of
//! the atmosphere (over [`ENGAGE_ALTITUDE_M`]) J takes the ship out of physics and flies it
//! by hand at a speed set by the nearest gravity well: `k · d`, where `d` is the distance to
//! the nearest body's surface and `k` comes from the throttle lever (up to [`K_MAX`] per
//! second). Travel is logarithmic: leaving a planet the speed grows with the distance, so the
//! edge of the solar system is tens of seconds away and the next star a minute; coming in,
//! the ship slows down by itself and drops out of cruise near the surface, handed back to
//! physics at under [`DROP_SPEED`]. J again drops out anywhere.
//!
//! A cruising ship has no `RigidBody` or `Collider` (both come back on dropping out): moving
//! a body 1e15 m in one step is nothing a physics backend is built for. It moves its own
//! `Transform` in f64 along its nose, turning at fixed rates from its `VehicleControls`
//! (pitch, yaw, steer as roll), so the mouse aim of [`crate::drive`] steers it as in space.
//! The world re-centers on it as it goes (the space module's world shifts), so positions stay
//! fine however far it flies. [`Cruise`] holds its speed; `VehicleTelemetry` reads as usual.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{Collider, RigidBody, Velocity};
use engine_world::{Gravity, VehicleControls};

use crate::chassis::{Vehicle, VehicleTelemetry};
use crate::ship::Spacecraft;

/// Lowest altitude over the nearest body to start cruising, meters.
pub const ENGAGE_ALTITUDE_M: f64 = 12_000.0;
/// Closing on a body under this altitude drops out of cruise, meters.
pub const DROP_ALTITUDE_M: f64 = 40_000.0;
/// A body the nose points at (within [`CAPTURE_RAD`], and smaller than that in view) is the
/// target: the ship flies for a point this high over its surface, beside it, in f64 (a star
/// 4 light years off is a smaller dot than an f32 direction can aim at).
pub const ARRIVE_ALTITUDE_M: f64 = 30_000.0;
pub const CAPTURE_RAD: f64 = 0.035;
/// Fastest a ship comes out of cruise, m/s (physics takes over from there).
pub const DROP_SPEED: f64 = 1_500.0;
/// Speed per meter of distance to the nearest surface at full lever, per second.
pub const K_MAX: f64 = 0.6;
/// The same at zero lever (cruise never stops dead).
pub const K_MIN: f64 = 0.03;
/// Slowest and fastest cruise, m/s (the fastest is some 10⁸ c, for the gaps between stars).
pub const SPEED_MIN: f64 = 300.0;
pub const SPEED_MAX: f64 = 3.0e16;
/// How fast the speed follows its limit, per second (up, down).
const EASE_UP: f64 = 1.2;
const EASE_DOWN: f64 = 4.0;
/// Turn rates at full stick, radians per second (pitch, yaw, roll).
const TURN: Vec3 = Vec3::new(0.7, 0.7, 1.2);
/// Light speed, m/s (for readouts).
pub const C: f64 = 299_792_458.0;

/// On a ship in cruise.
#[derive(Component, Debug, Clone)]
pub struct Cruise {
    /// Speed along the nose, m/s.
    pub speed: f64,
    /// The speed the nearest gravity well allows now, m/s.
    pub limit: f64,
    /// Altitude over the nearest body's surface, meters.
    pub altitude_m: f64,
    /// Where it flies (the nose, or toward the target), unit.
    pub heading: DVec3,
    /// The body it homes on (index in `Gravity::bodies`), if any.
    pub target: Option<usize>,
    /// The arrival point from the target's center, meters.
    side: DVec3,
    /// Turn rates now (local pitch, yaw, roll), radians per second, eased.
    rates: Vec3,
    /// Taken off while cruising, put back on dropping out.
    body: RigidBody,
    collider: Option<Collider>,
}

/// Altitude over the nearest body's surface at `p` and the way up there; without bodies, the
/// height over y = 0.
pub fn altitude(gravity: &Gravity, p: DVec3) -> (f64, DVec3) {
    gravity
        .bodies
        .iter()
        .map(|b| {
            let r = p - b.center;
            (r.length() - b.radius, r.normalize_or(DVec3::Y))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .unwrap_or((p.y, DVec3::Y))
}

/// Why a ship can't cruise now, if it can't.
pub fn refuse(world: &World, ship: Entity) -> Option<String> {
    if world.get::<Spacecraft>(ship).is_none() {
        return Some("only spacecraft cruise".into());
    }
    if world.get::<Cruise>(ship).is_some() {
        return Some("cruising already".into());
    }
    let t = world.get::<Transform>(ship)?;
    let (h, _) = altitude(world.resource::<Gravity>(), t.translation);
    (h < ENGAGE_ALTITUDE_M).then(|| {
        format!(
            "too low to cruise: {:.1} km up, {:.0} km needed",
            h / 1000.0,
            ENGAGE_ALTITUDE_M / 1000.0
        )
    })
}

/// Into cruise, at the speed it flies now. Returns why not, if it can't.
pub fn engage(world: &mut World, ship: Entity) -> Result<(), String> {
    if let Some(why) = refuse(world, ship) {
        return Err(why);
    }
    let Some(body) = world.get::<RigidBody>(ship).cloned() else {
        return Err("no rigid body".into());
    };
    let speed = world
        .get::<Velocity>(ship)
        .map_or(0.0, |v| v.linear.length() as f64);
    let (p, heading) = world
        .get::<Transform>(ship)
        .map_or((DVec3::ZERO, DVec3::NEG_Z), |t| {
            (t.translation, t.forward().as_dvec3())
        });
    let (h, _) = altitude(world.resource::<Gravity>(), p);
    let mut e = world.entity_mut(ship);
    let collider = e.take::<Collider>();
    e.remove::<RigidBody>();
    e.insert(Cruise {
        speed: speed.max(SPEED_MIN),
        limit: speed.max(SPEED_MIN),
        altitude_m: h,
        heading,
        target: None,
        side: DVec3::X,
        rates: Vec3::ZERO,
        body,
        collider,
    });
    if let Some(mut v) = e.get_mut::<Velocity>() {
        *v = Velocity::default();
    }
    Ok(())
}

/// Out of cruise: back to physics along the nose at up to [`DROP_SPEED`].
pub fn drop_out(world: &mut World, ship: Entity) {
    let Some(cruise) = world.get::<Cruise>(ship).cloned() else {
        return;
    };
    let forward = cruise.heading.as_vec3();
    let mut e = world.entity_mut(ship);
    e.remove::<Cruise>();
    e.insert(cruise.body);
    if let Some(c) = cruise.collider {
        e.insert(c);
    }
    e.insert(Velocity {
        linear: forward * cruise.speed.min(DROP_SPEED) as f32,
        angular: Vec3::ZERO,
    });
}

/// The target and where to fly for it: a body the nose points at (within [`CAPTURE_RAD`] of
/// it while it looks smaller than that) is held as the target until the nose turns well off
/// it; the ship flies for the point [`ARRIVE_ALTITUDE_M`] over its surface on the side it came
/// in from, so it arrives skimming past rather than head on. Returns the heading and the
/// distance to that point (without a target: the nose, and none).
fn home(gravity: &Gravity, p: DVec3, forward: DVec3, cruise: &mut Cruise) -> (DVec3, Option<f64>) {
    let bodies = &gravity.bodies;
    // Still on it? Released when the nose is off its center by more than its size and 15°.
    if let Some(i) = cruise.target {
        let keep = bodies.get(i).is_some_and(|b| {
            let to = b.center - p;
            let d = to.length();
            let size = (b.radius / d.max(b.radius)).clamp(0.0, 1.0).asin();
            forward.dot(to / d).clamp(-1.0, 1.0).acos() < size + 0.26
        });
        if !keep {
            cruise.target = None;
        }
    }
    if cruise.target.is_none() {
        let pick = bodies
            .iter()
            .enumerate()
            .filter_map(|(i, b)| {
                let to = b.center - p;
                let d = to.length();
                if d <= b.radius + DROP_ALTITUDE_M * 2.0 {
                    return None;
                }
                let off = forward.dot(to / d).clamp(-1.0, 1.0).acos();
                let size = (b.radius / d).asin();
                (off < CAPTURE_RAD && size < CAPTURE_RAD).then_some((i, off, to / d))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _, to)) = pick {
            cruise.target = Some(i);
            // Beside it on the side the nose points to (any side when dead on): where the
            // line from here touches the sphere `ARRIVE_ALTITUDE_M` over it, so the way in
            // skims that height and never dips below.
            let side = (forward - to * forward.dot(to))
                .try_normalize()
                .unwrap_or_else(|| to.any_orthonormal_vector());
            let b = &bodies[i];
            let a = b.radius + ARRIVE_ALTITUDE_M;
            let d = (b.center - p).length();
            let theta = (a / d).clamp(-1.0, 1.0).acos();
            cruise.side = (-to * theta.cos() + side * theta.sin()) * a;
        }
    }
    let Some(b) = cruise.target.and_then(|i| bodies.get(i)) else {
        return (forward, None);
    };
    let aim = b.center + cruise.side;
    let d = (aim - p).length();
    ((aim - p).normalize_or(forward), Some(d))
}

type Cruising<'a> = (
    Entity,
    &'a mut Transform,
    &'a mut Cruise,
    &'a VehicleControls,
    Option<&'a mut VehicleTelemetry>,
);

/// `FixedUpdate`, after the vehicle systems: flies every cruising ship one step and drops out
/// those closing on a surface.
pub(crate) fn cruise_system(
    time: Res<Time>,
    gravity: Res<Gravity>,
    mut q: Query<Cruising, With<Vehicle>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return;
    }
    for (e, mut t, mut cruise, c, tel) in &mut q {
        // Turn: pitch up, yaw right and roll right as the controls ask, eased in.
        let want = Vec3::new(c.pitch, -c.yaw, -c.steer).clamp(Vec3::NEG_ONE, Vec3::ONE) * TURN;
        let k = 1.0 - (-dt as f32 * 6.0).exp();
        cruise.rates = cruise.rates.lerp(want, k);
        let r = cruise.rates * dt as f32;
        let turn =
            Quat::from_rotation_x(r.x) * Quat::from_rotation_y(r.y) * Quat::from_rotation_z(r.z);
        t.rotation = (t.rotation * turn).normalize();

        // Where to: the nose, or the target.
        let forward = t.forward().as_dvec3();
        let (heading, to_aim) = home(&gravity, t.translation, forward, &mut cruise);
        cruise.heading = heading;

        // Speed: the gravity well's limit (homing on a target, the distance to the arrival
        // point: the way there skims past the body), eased toward, never past it.
        let (h, up) = altitude(&gravity, t.translation);
        let lever = if c.boost {
            1.0
        } else {
            c.throttle.clamp(0.0, 1.0) as f64
        };
        let k = K_MIN + (K_MAX - K_MIN) * lever;
        let room = match (to_aim, cruise.target) {
            // Homing: the way to the arrival point, short of any other body on the way.
            (Some(d), Some(i)) => gravity
                .bodies
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, b)| (t.translation - b.center).length() - b.radius)
                .fold(d, f64::min),
            _ => h,
        }
        .max(1.0);
        let limit = (k * room).clamp(SPEED_MIN, SPEED_MAX);
        let ratio = (limit / cruise.speed.max(1.0)).ln();
        cruise.speed *= ratio.clamp(-EASE_DOWN * dt, EASE_UP * dt).exp();
        cruise.speed = cruise.speed.min((K_MAX * room).max(SPEED_MIN));
        cruise.limit = limit;
        cruise.altitude_m = h;

        let velocity = heading * cruise.speed;
        t.translation += velocity * dt;

        if let Some(mut tel) = tel {
            tel.speed = cruise.speed as f32;
            tel.forward_speed = cruise.speed as f32;
            tel.airspeed = 0.0;
            tel.mach = 0.0;
            tel.altitude_m = h;
            tel.vertical_speed = velocity.dot(up) as f32;
            tel.air_density = 0.0;
            tel.dynamic_pressure = 0.0;
            tel.aoa_deg = 0.0;
            tel.sideslip_deg = 0.0;
            tel.g_load = 0.0;
            tel.wheels_grounded = 0;
            tel.throttle = lever as f32;
        }

        // Arrived, or closing on a surface: back to physics.
        let arrived = to_aim.is_some_and(|d| d < DROP_ALTITUDE_M);
        if arrived || (h < DROP_ALTITUDE_M && velocity.dot(up) < 0.0) {
            commands.queue(move |world: &mut World| drop_out(world, e));
        }
    }
}
