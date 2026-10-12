//! Rolling resistance: balls and barrels slow down and stop like real ones, sooner on soft
//! ground, and roll forever only when the setting says so.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsSettings};
use engine_world::physics::{Collider, ColliderShape, RigidBody, Velocity};
use engine_world::{SurfaceMaterials, WorldPlugin};

const G: f32 = 9.81;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app
}

/// A long flat strip of `surface` with its top at y = 0.
fn ground(app: &mut App, surface: &str) {
    ground_of(app, Ground::Slab, surface);
}

/// How the ground's collider is made.
#[derive(Clone, Copy, Debug)]
enum Ground {
    /// One box.
    Slab,
    /// A flat heightfield with 1 m cells, like terrain.
    Terrain,
    /// A triangle mesh of 2 m quads, like a road or a floor model.
    Mesh,
}

/// [`ground`] made as `kind`: terrain and meshes touch a ball with one triangle or more.
fn ground_of(app: &mut App, kind: Ground, surface: &str) {
    let surface = app
        .world
        .resource::<SurfaceMaterials>()
        .id(surface)
        .unwrap();
    let (y, shape) = match kind {
        Ground::Slab => (
            -0.5,
            ColliderShape::Cuboid {
                half_extents: Vec3::new(200.0, 0.5, 5.0),
            },
        ),
        Ground::Terrain => {
            let (rows, cols) = (11, 401);
            (
                0.0,
                ColliderShape::Heightfield {
                    heights: vec![0.0; (rows * cols) as usize],
                    rows,
                    cols,
                    size: [400.0, 10.0],
                },
            )
        }
        Ground::Mesh => {
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            for i in 0..=200u32 {
                let x = -200.0 + 2.0 * i as f32;
                vertices.extend([Vec3::new(x, 0.0, -5.0), Vec3::new(x, 0.0, 5.0)]);
                if i > 0 {
                    let (a, b, c, d) = (2 * i - 2, 2 * i - 1, 2 * i, 2 * i + 1);
                    indices.extend([[a, b, c], [c, b, d]]);
                }
            }
            (0.0, ColliderShape::TriMesh { vertices, indices })
        }
    };
    app.world.spawn((
        Transform::from_xyz(0.0, y, 0.0),
        RigidBody::fixed(),
        Collider {
            shape,
            surface,
            ..Default::default()
        },
    ));
}

#[derive(Clone, Copy, Debug)]
enum Roller {
    Ball,
    /// A capsule on its side (a log, a ragdoll's limb).
    Log,
    /// A cylinder on its side.
    Barrel,
}

/// A rolling body of radius 0.3 m launched along +X at `speed`, already rolling without slipping.
fn launch(app: &mut App, roller: Roller, speed: f32) -> Entity {
    let r = 0.3;
    // Capsules and cylinders have their axis along Y: lay them on their side, axis along Z.
    let side = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    let (shape, rotation) = match roller {
        Roller::Ball => (ColliderShape::Sphere { radius: r }, Quat::IDENTITY),
        Roller::Log => (
            ColliderShape::Capsule {
                radius: r,
                half_height: 0.4,
            },
            side,
        ),
        Roller::Barrel => (
            ColliderShape::Cylinder {
                radius: r,
                half_height: 0.4,
            },
            side,
        ),
    };
    let collider = Collider {
        shape,
        ..Default::default()
    };
    app.world
        .spawn((
            Transform {
                translation: DVec3::new(-150.0, r as f64, 0.0),
                rotation,
                ..Default::default()
            },
            // No damping, so only rolling resistance slows it.
            RigidBody {
                angular_damping: 0.0,
                ..RigidBody::dynamic()
            },
            collider,
            Velocity {
                linear: Vec3::new(speed, 0.0, 0.0),
                angular: Vec3::new(0.0, 0.0, -speed / r),
            },
        ))
        .id()
}

fn speed(app: &App, e: Entity) -> f32 {
    app.world.get::<Velocity>(e).unwrap().linear.length()
}

/// Deceleration over `secs` after a short settle.
fn deceleration(app: &mut App, e: Entity, secs: f32) -> f32 {
    app.update_n(30);
    let v0 = speed(app, e);
    app.update_n((secs * 60.0) as usize);
    (v0 - speed(app, e)) / secs
}

#[test]
fn balls_and_logs_on_concrete_slow_like_real_ones() {
    // Rolling without slipping: a = Crr·g / (1 + I/(m·r²)). A capsule of radius 0.3 and
    // half-height 0.4 is 2/3 cylinder (I = m·r²/2) and 1/3 ball (I = 2/5·m·r²).
    for (roller, shape_factor) in [
        (Roller::Ball, 1.4),
        (Roller::Log, 1.0 + 1.0 / 3.0 + 0.4 / 3.0),
    ] {
        let mut app = app();
        ground(&mut app, "concrete");
        let e = launch(&mut app, roller, 4.0);
        let crr = app.world.resource::<PhysicsSettings>().rolling_resistance;
        let expect = crr * G / shape_factor;
        let got = deceleration(&mut app, e, 3.0);
        assert!(
            (got - expect).abs() < 0.2 * expect,
            "{roller:?}: slowed at {got} m/s², expected about {expect}"
        );
    }
}

#[test]
fn balls_slow_the_same_on_terrain_and_meshes() {
    // Terrain and meshes touch a ball through several triangles at once; the resistance must
    // not depend on how the ground is made.
    let decel = |kind: Ground| {
        let mut app = app();
        ground_of(&mut app, kind, "concrete");
        let e = launch(&mut app, Roller::Ball, 4.0);
        deceleration(&mut app, e, 3.0)
    };
    let slab = decel(Ground::Slab);
    for kind in [Ground::Terrain, Ground::Mesh] {
        let got = decel(kind);
        assert!(
            (got - slab).abs() < 0.15 * slab,
            "{kind:?}: slowed at {got} m/s², {slab} on a slab"
        );
    }
}

#[test]
fn barrels_slow_down_and_stop_on_sand() {
    // Rapier's cylinders hop a little as they roll, so check the outcome rather than the rate.
    let after = |crr: f32, surface: &str, secs: usize| {
        let mut app = app();
        app.world
            .resource_mut::<PhysicsSettings>()
            .rolling_resistance = crr;
        ground(&mut app, surface);
        let e = launch(&mut app, Roller::Barrel, 4.0);
        app.update_n(secs * 60);
        speed(&app, e)
    };
    let (free, resisted) = (after(0.0, "concrete", 5), after(0.01, "concrete", 5));
    assert!(
        resisted < free - 0.15,
        "rolling resistance didn't slow a barrel: {resisted} vs {free} m/s"
    );
    let sand = after(0.01, "sand", 6);
    assert!(sand < 0.05, "a barrel still rolls on sand at {sand} m/s");
}

#[test]
fn soft_ground_stops_a_ball_sooner() {
    let mut decel = Vec::new();
    for surface in ["concrete", "grass", "sand"] {
        let mut app = app();
        ground(&mut app, surface);
        let e = launch(&mut app, Roller::Ball, 4.0);
        decel.push(deceleration(&mut app, e, 2.0));
    }
    assert!(
        decel[0] < decel[1] && decel[1] < decel[2],
        "concrete, grass, sand: {decel:?} m/s²"
    );
    // Sand (Crr ≈ 0.13): a 4 m/s ball stops within about 5 s.
    let mut app = app();
    ground(&mut app, "sand");
    let e = launch(&mut app, Roller::Ball, 4.0);
    app.update_n(6 * 60);
    assert!(speed(&app, e) < 0.05, "still rolling at {}", speed(&app, e));
    let w = app.world.get::<Velocity>(e).unwrap().angular.length();
    assert!(w < 0.2, "still turning at {w} rad/s");
}

#[test]
fn zero_rolling_resistance_rolls_forever() {
    let mut app = app();
    app.world
        .resource_mut::<PhysicsSettings>()
        .rolling_resistance = 0.0;
    ground(&mut app, "concrete");
    let e = launch(&mut app, Roller::Ball, 4.0);
    let got = deceleration(&mut app, e, 3.0);
    assert!(
        got.abs() < 0.01,
        "slowed at {got} m/s² with no rolling resistance"
    );
}
