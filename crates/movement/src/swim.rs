//! Water: wading slows a character as the water deepens, deeper water floats it into
//! swimming, and going in (a jump, a fall, a run off the bank) splashes.
//!
//! Like Unreal's swimming mode, a character swims once enough of it is under water
//! ([`MovementProfile::swim_depth`] over the feet) and walks again where its feet find ground
//! in shallower water. It floats with its head out, swims along the stick (faster with sprint),
//! dives with crouch and climbs out over a bank with jump. Wading slows it the deeper it goes,
//! as in Ghost of Tsushima or Breath of the Wild.
//!
//! The water comes from the world's [`Water`](engine_world::Water) query and everything the
//! character does to it goes out as [`WaterDisturbance`]: a push at the waterline every step it
//! moves through the water (rings and a wake) and a plunge when it goes in (the water module
//! throws spray and plays the splash for one fast enough). Footsteps in shallow water already
//! ring it through their `GroundContact`s.

use engine_core::math::{DVec3, Vec3};
use engine_world::locomotion::{LocomotionMode, MoveIntent};
use engine_world::{WaterDisturbance, WaterSample};

use crate::mover::{Mover, flat, move_towards};
use crate::profile::MovementProfile;

/// Swimming stops (the feet walk again) once the water over them is this fraction of
/// `swim_depth` or less with ground under them; between the two it keeps doing what it did.
const WALK_OUT: f32 = 0.85;
/// Floating, the body's center sits this far below the surface, as a fraction of its height
/// (head and shoulders out).
const FLOAT_DEPTH: f32 = 0.12;
/// Horizontal acceleration while swimming, m/s² (water is slow to speed up in and quick to
/// stop in).
const SWIM_ACCEL: f32 = 4.0;
const SWIM_BRAKE: f32 = 3.0;
/// Vertical speed toward the floating height per meter off it, 1/s, and its limits, m/s.
const FLOAT_SPRING: f32 = 3.0;
const RISE_SPEED: f32 = 2.0;
const DIVE_SPEED: f32 = 1.6;
/// How quickly a plunge's speed is soaked up by the water, m/s².
const WATER_DRAG: f32 = 18.0;
/// At full wading depth (the swimming depth) a character moves this much slower.
const WADE_SLOW: f32 = 0.55;
/// Moving slower than this through the water, m/s, pushes no wake.
const WAKE_MIN_SPEED: f32 = 0.3;

/// The water around one character this step.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InWater {
    /// The water's surface height, world meters.
    pub(crate) surface: f64,
    /// Water over the feet, m.
    pub(crate) depth: f32,
    /// Surface current, m/s.
    pub(crate) current: Vec3,
}

impl InWater {
    /// The water at a character whose feet are at `feet`, if any is over them.
    pub(crate) fn at(sample: Option<WaterSample>, feet: DVec3) -> Option<Self> {
        let w = sample?;
        let depth = (w.surface_height - feet.y) as f32;
        (depth > 0.0).then_some(Self {
            surface: w.surface_height,
            depth,
            current: w.velocity,
        })
    }
}

/// How much a character wading `depth` meters deep is slowed: 1 on dry ground, falling to
/// `1 - WADE_SLOW` at the swimming depth (knee-deep about 0.8, waist-deep about 0.55).
pub(crate) fn wade_scale(depth: f32, p: &MovementProfile) -> f32 {
    let x = (depth / p.swim_depth.max(0.1)).clamp(0.0, 1.0);
    1.0 - WADE_SLOW * x
}

/// What a character in the water does this step, and what it does to the water: the swimming
/// velocity, or `None` on dry ground or wading (move as on land, slowed by `Mover::wade`).
/// `water` is the water at its feet, `grounded` whether they stand on something. (Jumping out
/// over a bank is a mantle, which the caller starts.)
#[allow(clippy::too_many_arguments)]
pub(crate) fn step(
    m: &mut Mover,
    p: &MovementProfile,
    intent: &MoveIntent,
    water: Option<InWater>,
    pos: DVec3,
    radius: f32,
    height: f32,
    v: Vec3,
    wish: Vec3,
    grounded: bool,
    dt: f32,
    disturb: &mut impl FnMut(WaterDisturbance),
    entity: engine_core::prelude::Entity,
) -> Option<Vec3> {
    let up = m.up;
    let was_wet = m.wet;
    let Some(w) = water.filter(|_| p.swim) else {
        m.wet = false;
        m.wade = 1.0;
        if m.mode == LocomotionMode::Swim {
            // Swum somewhere with no water under it (up a ramp, out of a pool's edge).
            m.set_mode(LocomotionMode::Air);
        }
        return None;
    };
    m.wet = true;
    let rel = v - w.current;
    let at = DVec3::new(pos.x, w.surface, pos.z);
    // Going in: a plunge the size of the body (spray when it's fast).
    if !was_wet {
        disturb(WaterDisturbance {
            position: at,
            velocity: rel,
            radius: radius * 1.5,
            mass_kg: p.mass_kg * 0.5,
            source: Some(entity),
        });
    }
    // Pushing through it: rings and a wake from the part under water.
    let submerged = (w.depth / height.max(0.1)).clamp(0.0, 1.0);
    if was_wet && flat(rel, up).length() > WAKE_MIN_SPEED {
        disturb(WaterDisturbance {
            position: at,
            velocity: flat(rel, up),
            radius: radius * (0.8 + 0.6 * submerged),
            mass_kg: p.mass_kg * submerged,
            source: Some(entity),
        });
    }

    let swimming = m.mode == LocomotionMode::Swim;
    let deep =
        w.depth > p.swim_depth || (swimming && !(grounded && w.depth < p.swim_depth * WALK_OUT));
    if !deep {
        m.wade = wade_scale(w.depth, p);
        if swimming {
            m.set_mode(LocomotionMode::Walk);
        }
        return None;
    }
    m.wade = 1.0;
    if !swimming {
        m.set_mode(LocomotionMode::Swim);
        m.dash = None;
        m.wall = None;
    }
    m.grounded = false;
    m.air_secs = 0.0;
    m.fall_speed = 0.0;
    m.coyote = 0.0;
    m.air_jumps_left = p.air_jumps;

    let speed = if intent.sprint {
        p.swim_sprint_speed
    } else {
        p.swim_speed
    };
    let target = wish * speed;
    let vh = flat(v, up);
    let accel = if vh.length() > speed + 0.1 {
        WATER_DRAG
    } else if target.length() < 0.01 {
        SWIM_BRAKE
    } else {
        SWIM_ACCEL
    };
    let vh = move_towards(vh, target, accel * dt);

    // Float at the surface; crouch dives, jump held comes up.
    let float_at = w.surface - (height * FLOAT_DEPTH) as f64;
    let want = if intent.crouch {
        -DIVE_SPEED
    } else {
        ((float_at - pos.y) as f32 * FLOAT_SPRING).clamp(-DIVE_SPEED, RISE_SPEED)
    };
    let vv = v.dot(up);
    let vv = move_towards(Vec3::X * vv, Vec3::X * want, WATER_DRAG * dt).x;
    Some(vh + up * vv)
}
