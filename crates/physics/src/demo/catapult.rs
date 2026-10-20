//! A counterweight catapult: a tonne of stone in an iron-bound box drops on the short end of an
//! oak arm, the long end whips up and over, and when the arm slams into its stop the granite
//! ball in its cup flies on, over the yard and into a wall of empty crates.
//!
//! Everything here is plain rigid bodies: the arm is one body (beam, counterweight box and cup
//! as one compound collider, with the masses of oak, stone and iron set on it), hinged to the
//! world at its axle, stopped by the hinge's limit where the frame's padded crossbar stands.
//! Nothing lets the ball go: it leaves the cup on its own, because the cup stops and it
//! doesn't.

use std::f32::consts::PI;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_world::physics::{Collider, ColliderShape, JointKind, RigidBody};

use super::Yard;
use super::art::{Mat, catapult as art};

/// The axle's height above the slab, meters.
pub const AXLE: f32 = 2.6;
/// Axle to the ball's center along the arm (the long side), meters.
pub const LONG: f32 = 3.4;
/// Axle to the counterweight's center along the arm (the short side), meters.
pub const SHORT: f32 = 1.0;
/// The arm beam's half thickness (it is square), meters.
pub const BEAM: f32 = 0.12;
/// How far the beam runs past the cup and past the counterweight, meters.
pub const BEAM_PAST_CUP: f32 = 0.3;
pub const BEAM_PAST_WEIGHT: f32 = 0.3;
/// The counterweight box's half extents (along the arm, across it, along the axle), meters.
pub const WEIGHT_HALF: Vec3 = Vec3::new(0.45, 0.45, 0.4);
/// The ball's radius, meters (granite, 2700 kg/m³: 25 kg).
pub const BALL: f32 = 0.15;
/// The cup's walls: thickness and how far they rise above the beam, meters.
pub const CUP_WALL: f32 = 0.03;
pub const CUP_RISE: f32 = 0.6 * BALL;

/// Masses, kg: the counterweight (stone in an iron-bound box), the oak arm with its cup.
pub const WEIGHT_KG: f32 = 1200.0;
pub const ARM_KG: f32 = 190.0;

/// The arm's angle about the axle (Z), radians from level with the cup pointing back along -X:
/// cocked, cup low behind the axle and counterweight up, and at the stop, cup up and 45° back
/// from the top, so the ball flies off 45° up and forward (+X).
pub const COCKED: f32 = 40.0 * PI / 180.0;
pub const STOP: f32 = -45.0 * PI / 180.0;

/// The frame's sides stand this far either side of the arm (their inner faces), meters.
pub const FRAME_GAP: f32 = 0.55;

/// The crate wall the ball is thrown at: its middle this far down range from the axle, meters,
/// and its crates' half size.
pub const WALL_RANGE: f32 = 11.6;
pub const CRATE: f32 = 0.3;
/// Crates across the wall's bottom course (the courses between have one fewer) and courses
/// up it.
pub const WALL_WIDE: usize = 5;
pub const WALL_HIGH: usize = 5;

/// The ball's center in the arm's frame (on the beam's top face, in the cup).
pub fn ball_in_arm() -> Vec3 {
    Vec3::new(-LONG, BEAM + BALL, 0.0)
}

/// The arm's collider parts in its own frame (origin on the axle, X along the arm toward the
/// counterweight, Y across it, out of the cup): (center, half extents).
pub fn arm_parts() -> Vec<(Vec3, Vec3)> {
    let (tip, tail) = (-(LONG + BEAM_PAST_CUP), SHORT + BEAM_PAST_WEIGHT);
    let cup = ball_in_arm();
    let rise = Vec3::Y * (BEAM + CUP_RISE * 0.5);
    let wall_h = CUP_RISE * 0.5;
    vec![
        // The beam.
        (
            Vec3::new((tip + tail) * 0.5, 0.0, 0.0),
            Vec3::new((tail - tip) * 0.5, BEAM, BEAM),
        ),
        // The counterweight box around the beam's end.
        (Vec3::new(SHORT, 0.0, 0.0), WEIGHT_HALF),
        // The cup: walls at the tip end, the axle end and both sides; the beam is its floor and
        // it is open on top (+Y), the way the ball leaves.
        (
            Vec3::new(cup.x - BALL - CUP_WALL, 0.0, 0.0) + rise,
            Vec3::new(CUP_WALL, wall_h, BALL + 2.0 * CUP_WALL),
        ),
        (
            Vec3::new(cup.x + BALL + CUP_WALL, 0.0, 0.0) + rise,
            Vec3::new(CUP_WALL, wall_h, BALL + 2.0 * CUP_WALL),
        ),
        (
            Vec3::new(cup.x, 0.0, BALL + CUP_WALL) + rise,
            Vec3::new(BALL, wall_h, CUP_WALL),
        ),
        (
            Vec3::new(cup.x, 0.0, -(BALL + CUP_WALL)) + rise,
            Vec3::new(BALL, wall_h, CUP_WALL),
        ),
    ]
}

/// The arm's mass properties in its own frame: center of mass and principal moments of
/// inertia (along X, Y, Z), from the oak beam and the counterweight (the cup's few kilograms
/// are in the beam's).
pub fn arm_mass() -> (Vec3, Vec3) {
    let (tip, tail) = (-(LONG + BEAM_PAST_CUP), SHORT + BEAM_PAST_WEIGHT);
    let beam_x = (tip + tail) * 0.5;
    let beam_half = Vec3::new((tail - tip) * 0.5, BEAM, BEAM);
    let total = ARM_KG + WEIGHT_KG;
    let com = Vec3::X * (ARM_KG * beam_x + WEIGHT_KG * SHORT) / total;
    let box_inertia = |m: f32, h: Vec3| {
        let s = h * h * 4.0;
        Vec3::new(s.y + s.z, s.x + s.z, s.x + s.y) * (m / 12.0)
    };
    // Parallel axes: both centers lie on the X axis, offset from the center of mass along X.
    let offset = |m: f32, x: f32| {
        let d = x - com.x;
        Vec3::new(0.0, m * d * d, m * d * d)
    };
    let inertia = box_inertia(ARM_KG, beam_half)
        + offset(ARM_KG, beam_x)
        + box_inertia(WEIGHT_KG, WEIGHT_HALF)
        + offset(WEIGHT_KG, SHORT);
    (com, inertia)
}

/// The ball's mass, kg.
pub fn ball_kg() -> f32 {
    2700.0 * 4.0 / 3.0 * PI * BALL.powi(3)
}

/// How fast the ball leaves the cup without losses, m/s: the energy the arm's center of mass
/// gives up falling from cocked to the stop, less what lifting the ball takes, turns the arm and
/// the ball in it about the axle.
pub fn release_speed() -> f32 {
    let (com, inertia) = arm_mass();
    let arm_kg = ARM_KG + WEIGHT_KG;
    let height = |p: Vec3, angle: f32| (Quat::from_rotation_z(angle) * p).y;
    let ball = ball_in_arm();
    let energy = 9.81
        * (arm_kg * (height(com, COCKED) - height(com, STOP))
            - ball_kg() * (height(ball, STOP) - height(ball, COCKED)));
    let about_axle = inertia.z + arm_kg * com.length_squared() + ball_kg() * ball.length_squared();
    (2.0 * energy / about_axle).sqrt() * ball.length()
}

/// The padded crossbar the arm comes to rest against: how far along the arm from the axle it
/// meets it, and the bar's radius, meters.
pub const CROSSBAR_ALONG: f32 = 1.3;
pub const CROSSBAR: f32 = 0.11;

/// The crossbar's center in the station's frame (origin on the slab under the axle): just
/// touching the arm's leading face where the arm stops.
pub fn crossbar() -> Vec3 {
    Vec3::Y * AXLE + Quat::from_rotation_z(STOP) * Vec3::new(-CROSSBAR_ALONG, BEAM + CROSSBAR, 0.0)
}

/// A timber from `foot` to `head` of half width `w`: (center, rotation, half extents).
fn timber(foot: Vec3, head: Vec3, w: f32) -> (Vec3, Quat, Vec3) {
    let d = head - foot;
    (
        (foot + head) * 0.5,
        Quat::from_rotation_arc(Vec3::Y, d.normalize()),
        Vec3::new(w, d.length() * 0.5, w),
    )
}

/// The frame's timbers in the station's frame: (center, rotation, half extents). Each side has
/// a sill, a front post leaning back to the axle's bearing and a rear post leaning forward to
/// the crossbar; ties join the sides at the sills.
pub fn frame_parts() -> Vec<(Vec3, Quat, Vec3)> {
    let mut parts = Vec::new();
    let w = 0.11;
    let bar = crossbar();
    for s in [-1.0f32, 1.0] {
        let z = s * (FRAME_GAP + w);
        parts.push((
            Vec3::new(-0.2, 0.12, z),
            Quat::IDENTITY,
            Vec3::new(2.2, 0.12, 0.16),
        ));
        parts.push(timber(
            Vec3::new(1.7, 0.24, z),
            Vec3::new(0.0, AXLE - 0.16, z),
            w,
        ));
        parts.push(timber(
            Vec3::new(-2.0, 0.24, z),
            Vec3::new(bar.x, bar.y + CROSSBAR, z),
            w,
        ));
    }
    for x in [-2.1f32, 1.7] {
        parts.push((
            Vec3::new(x, 0.12, 0.0),
            Quat::IDENTITY,
            Vec3::new(0.12, 0.12, FRAME_GAP + 2.0 * w),
        ));
    }
    parts
}

pub(super) fn catapult(y: &mut Yard, base: DVec3) {
    for (at, rotation, half) in frame_parts() {
        y.fixed(
            base + at.as_dvec3(),
            rotation,
            ColliderShape::Cuboid { half_extents: half },
            y.wood,
        );
    }
    let frame = y.parts("catapult_frame", [Mat::Timber, Mat::Dark], || {
        let f = art::frame();
        [f.timber, f.iron]
    });
    y.scenery(base, Quat::IDENTITY, frame);

    let axle = base + DVec3::Y * AXLE as f64;
    let pose = Quat::from_rotation_z(COCKED);
    let collider = Collider {
        shape: ColliderShape::Compound(
            arm_parts()
                .into_iter()
                .map(|(at, half)| {
                    (
                        at,
                        Quat::IDENTITY,
                        ColliderShape::Cuboid { half_extents: half },
                    )
                })
                .collect(),
        ),
        surface: y.wood,
        ..Default::default()
    };
    let parts = y.parts("catapult_arm", [Mat::Timber, Mat::Dark], || {
        let a = art::arm();
        [a.timber, a.iron]
    });
    let arm = y.body(axle, pose, collider, parts);
    y.name(arm, "Catapult arm");
    let (com, inertia) = arm_mass();
    y.world.entity_mut(arm).insert(RigidBody {
        mass_kg: Some(ARM_KG + WEIGHT_KG),
        center_of_mass: Some(com),
        inertia: Some(inertia),
        ..RigidBody::dynamic()
    });
    // Free from a little past cocked to the stop. The hinge's angle counts from the pose it
    // was made in, the other way round from the arm's.
    y.joint(
        JointKind::Revolute {
            axis: Vec3::Z,
            limits: Some([-0.05, COCKED - STOP]),
            motor: None,
        },
        arm,
        None,
        axle,
    );

    let ball_at = axle + (pose * ball_in_arm()).as_dvec3();
    let c = Collider::sphere(BALL)
        .with_surface(y.concrete)
        .with_density(2700.0);
    let parts = y.parts(&format!("catapult_ball/{BALL:.3}"), [Mat::Granite], || {
        [art::ball(BALL)]
    });
    let ball = y.body(ball_at, Quat::IDENTITY, c, parts);
    y.name(ball, "Catapult ball");

    // The crate wall down range, laid in courses like brickwork (each crate on two below), a
    // little apart so they stand on their own.
    let step = 2.0 * CRATE as f64 + 0.01;
    for row in 0..WALL_HIGH {
        let wide = WALL_WIDE - row % 2;
        for k in 0..wide {
            let z = (k as f64 - (wide - 1) as f64 * 0.5) * step;
            let at = base
                + DVec3::new(
                    WALL_RANGE as f64,
                    CRATE as f64 + 0.005 + row as f64 * step,
                    z,
                );
            let c = y.crate_of(
                at,
                Vec3::splat(CRATE),
                Quat::IDENTITY,
                (row * 7 + k) as u32,
                40.0,
            );
            y.name(c, "Catapult target crate");
        }
    }
}
