//! The float test: eight things dropped onto water, each settling the way the real one does.
//! [`spawn_floaters`] places it on any water (the showcase's lake, or a pool); physics floats
//! the bodies on whatever the `Water` resource reports there.
//!
//! Back row (−Z), left to right: a 2.4 m timber post dropped standing on end (it topples and
//! lies down, rolled onto a corner like a log), a raft of five such posts (it floats flat),
//! a half-density crate (it tips onto an edge, 45° over) and a waterlogged crate (0.9 of the
//! water's density: it floats flat, 8 cm showing). Front row (+Z): a plank thrown in on its
//! edge (it lands flat), a light playground ball (it rides high), a sealed empty oil drum and
//! a steel ball (it sinks).
//!
//! It replays every [`FLOAT_CYCLE_SECS`] of world time while a focus point is near. With
//! nobody near for a few seconds it rests, held asleep where it floats (floating bodies never
//! fall asleep by themselves, and their buoyancy samples the water every substep), and it drops
//! again when somebody comes back, like the proving ground's stations.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::FixedTime;
use engine_world::FocusPoints;
use engine_world::SurfaceId;
use engine_world::physics::{Collider, ColliderShape, RigidBody};

use super::art::{Mat, props, timber};
use super::{FAR_REST_SECS, STATION_REST_M, STATION_WAKE_M, Yard};

/// World seconds between drops: the splash, the bobbing and the slow tips play out in about
/// 20 s, then everything floats at rest.
pub const FLOAT_CYCLE_SECS: f64 = 40.0;

/// A float test, on its anchor entity.
#[derive(Component, Debug, Clone)]
pub struct FloatTest {
    /// The row's center on the water surface.
    origin: DVec3,
    surfaces: [SurfaceId; 4],
    /// World seconds since the last drop.
    running: f64,
    /// What the last drop spawned.
    parts: Vec<Entity>,
    /// World seconds with nobody near.
    away: f64,
    /// Held asleep until somebody comes near.
    resting: bool,
    /// Drop again on the cycle (on by default).
    pub replay: bool,
}

/// Put a float test on the water: `at` is the middle of its two rows, on the water surface.
/// The rows are 10.5 m long (X) and 4 m apart (Z); give it about 14 × 8 m of open water at
/// least 2 m deep (the steel ball sinks to the bottom).
pub fn spawn_floaters(commands: &mut Commands, at: DVec3) {
    commands.queue(move |world: &mut World| {
        let surfaces = super::surfaces(world);
        let anchor = world
            .spawn((
                Transform::from_translation(at),
                Name::new("Float test"),
                FloatTest {
                    origin: at,
                    surfaces,
                    running: 0.0,
                    parts: Vec::new(),
                    away: 0.0,
                    resting: false,
                    replay: true,
                },
            ))
            .id();
        drop_floaters(world, anchor);
    });
}

/// Clear what the float test on `anchor` dropped last time and drop everything again.
fn drop_floaters(world: &mut World, anchor: Entity) {
    let Some(test) = world.get::<FloatTest>(anchor).cloned() else {
        return;
    };
    for e in test.parts {
        // Children go with their parent, so some are gone already.
        if let Ok(e) = world.get_entity_mut(e) {
            e.despawn();
        }
    }
    let mut y = Yard::new(world, test.origin, test.surfaces, None);
    build(&mut y);
    let parts = std::mem::take(&mut y.spawned);
    if let Some(mut t) = world.get_mut::<FloatTest>(anchor) {
        t.parts = parts;
        t.running = 0.0;
        t.away = 0.0;
        t.resting = false;
    }
}

/// Drop each float test again when its cycle is up (world time).
pub(crate) fn replay_floaters(world: &mut World) {
    let dt = world
        .get_resource::<FixedTime>()
        .map_or(0.0, |t| t.world_step_secs());
    let focus = world
        .get_resource::<FocusPoints>()
        .map(|f| f.points.clone())
        .unwrap_or_default();
    // (No focus, as in tests and tools: it always runs.)
    let nearest = |at: DVec3| {
        focus
            .iter()
            .map(|p| p.distance(at))
            .fold(if focus.is_empty() { 0.0 } else { f64::INFINITY }, f64::min)
    };
    let (mut due, mut rest) = (Vec::new(), Vec::new());
    for (e, mut t) in world.query::<(Entity, &mut FloatTest)>().iter_mut(world) {
        let d = nearest(t.origin);
        if t.resting {
            if d < STATION_WAKE_M {
                due.push(e);
            }
            continue;
        }
        if d >= STATION_REST_M {
            t.away += dt;
            if t.away >= FAR_REST_SECS {
                t.resting = true;
                rest.extend(t.parts.iter().copied());
            }
            continue;
        }
        t.away = 0.0;
        if !t.replay {
            continue;
        }
        t.running += dt;
        if t.running >= FLOAT_CYCLE_SECS {
            due.push(e);
        }
    }
    for e in due {
        drop_floaters(world, e);
    }
    if !rest.is_empty() {
        let bodies: Vec<Entity> = rest
            .into_iter()
            .filter(|e| world.get::<RigidBody>(*e).is_some())
            .collect();
        let shared = world.resource::<crate::SharedSimulation>().0.clone();
        shared.write().resting.extend(bodies);
    }
}

/// Where each floater drops, relative to the row's center on the surface: (x, z).
const BACK: f64 = -2.0;
const FRONT: f64 = 2.0;
const COLUMNS: [f64; 4] = [-5.25, -1.75, 1.75, 5.25];
/// How far above the water each one's lowest point starts, meters.
const DROP: f64 = 0.25;

fn build(y: &mut Yard) {
    // A 2.4 m post (25 cm square, half-density timber) dropped standing on end, leaning back
    // a touch: it topples, and a half-density square beam floats on a corner (diamond-wise),
    // like the cube.
    let post_half = Vec3::new(0.125, 1.2, 0.125);
    let parts = y.parts("float/post", [Mat::Timber], || {
        [timber::timber_beam(post_half, 3)]
    });
    let c = Collider::cuboid(post_half)
        .with_surface(y.wood)
        .with_density(500.0);
    let at = DVec3::new(COLUMNS[0], post_half.y as f64 + DROP, BACK);
    // Turned a little about its own axis, as a real one never lands square.
    let turn = Quat::from_rotation_x(-0.06) * Quat::from_rotation_y(0.3);
    let post = y.body(at, turn, c, parts);
    y.name(post, "Float test: post");

    // Five such posts bolted into a raft: wide, so it floats flat.
    let beam = Vec3::new(0.125, 0.125, 1.2);
    let raft = timber::raft(beam, 5, 0.01, 0.8);
    let shape = ColliderShape::Compound(
        raft.boxes
            .iter()
            .map(|&(c, half)| {
                (
                    c,
                    Quat::IDENTITY,
                    ColliderShape::Cuboid { half_extents: half },
                )
            })
            .collect(),
    );
    let parts = y.parts("float/raft", [Mat::Timber, Mat::Steel], || {
        let r = timber::raft(beam, 5, 0.01, 0.8);
        [r.timber, r.steel]
    });
    let c = Collider {
        shape,
        surface: y.wood,
        density: 500.0,
        ..Default::default()
    };
    let at = DVec3::new(COLUMNS[1], beam.y as f64 + DROP, BACK);
    let raft = y.body(at, Quat::from_rotation_z(0.04), c, parts);
    y.name(raft, "Float test: raft");

    // An 80 cm crate of half the water's density tips onto an edge (floating flat is unstable
    // between 0.21 and 0.79 of the water's density); a waterlogged one at 0.9 floats flat.
    let half = Vec3::splat(0.4);
    for (col, density, seed, name) in [
        (2, 500.0, 1, "Float test: light crate"),
        (3, 900.0, 2, "Float test: waterlogged crate"),
    ] {
        let at = DVec3::new(COLUMNS[col], half.y as f64 + DROP, BACK);
        let e = y.crate_of(at, half, Quat::from_rotation_z(0.05), seed, density);
        y.name(e, name);
    }

    // A 2 m plank thrown in on its edge lands flat.
    let plank_half = Vec3::new(1.0, 0.03, 0.15);
    let parts = y.parts("float/plank", [Mat::Timber], || {
        [timber::timber_beam(plank_half, 11)]
    });
    let c = Collider::cuboid(plank_half)
        .with_surface(y.wood)
        .with_density(600.0);
    let tilt = 1.0f32;
    let low = plank_half.z * tilt.sin() + plank_half.y * tilt.cos();
    let at = DVec3::new(COLUMNS[0], low as f64 + DROP, FRONT);
    let plank = y.body(at, Quat::from_rotation_x(tilt), c, parts);
    y.name(plank, "Float test: plank");

    // A light rubber ball (150 kg/m³) floats with most of it above the water.
    let ball = y.ball(
        DVec3::new(COLUMNS[1], 0.3 + DROP, FRONT),
        0.3,
        150.0,
        0xd8c23a,
    );
    y.name(ball, "Float test: ball");

    // A sealed empty oil drum: 20 kg of steel around 240 l of air.
    let (r, hh) = (0.3f32, 0.42f32);
    let c = Collider {
        shape: ColliderShape::Cylinder {
            radius: r,
            half_height: hh,
        },
        surface: y.metal,
        density: 20.0 / (std::f32::consts::PI * r * r * 2.0 * hh),
        ..Default::default()
    };
    let parts = y.parts("drum/2f5f9e", [Mat::Drum(0x2f5f9e)], || {
        [props::steel_drum(r, hh)]
    });
    let at = DVec3::new(COLUMNS[2], hh as f64 + DROP, FRONT);
    let drum = y.body(at, Quat::from_rotation_z(0.08), c, parts);
    y.name(drum, "Float test: drum");

    // A 50 cm steel ball sinks.
    let parts = y.parts("steel_ball/0.25", [Mat::Steel], || [props::ball(0.25)]);
    let steel = y.steel_ball(DVec3::new(COLUMNS[3], 0.25 + DROP, FRONT), 0.25, parts);
    y.name(steel, "Float test: steel ball");
}
