//! The vehicle test cases in the showcase.
//!
//! * [`spawn_track`]: an oval race track with a jump, two sports cars racing round it, a
//!   rally car drifting on a dirt pad, a runway with one plane taking off and another flying
//!   circuits overhead.
//! * [`spawn_spaceport`]: a VTOL spaceplane hopping off a landing pad (hover engines, dust
//!   and rocket wash) next to a parked one.
//!
//! Every surface a vehicle touches here has a static collider, so the demos work with any
//! physics backend and need nothing else from the game.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::SurfaceMaterials;
use engine_world::physics::{Collider, ColliderShape, RigidBody, Velocity};

use crate::autopilot::{Autopilot, AutopilotMode};
use crate::presets;

/// Half-axes of the oval's center line, meters (x, z).
pub const TRACK_RADII: (f64, f64) = (160.0, 88.0);
/// Asphalt width, meters.
pub const TRACK_WIDTH: f32 = 12.0;
/// Height of the asphalt top above the zone's ground, meters.
pub const TRACK_SURFACE: f64 = 0.06;
/// Where the jump sits on the oval (angle around it, radians).
pub const JUMP_ANGLE: f64 = std::f64::consts::FRAC_PI_2;

const SEGMENTS: usize = 64;

/// A point on the oval's center line at `angle` (radians, counter-clockwise seen from
/// above, 0 = +X), on the asphalt.
pub fn oval_point(center: DVec3, angle: f64) -> DVec3 {
    center
        + DVec3::new(
            angle.cos() * TRACK_RADII.0,
            TRACK_SURFACE,
            angle.sin() * TRACK_RADII.1,
        )
}

/// Driving direction (increasing angle) at `angle`.
pub fn oval_direction(angle: f64) -> Vec3 {
    DVec3::new(
        -angle.sin() * TRACK_RADII.0,
        0.0,
        angle.cos() * TRACK_RADII.1,
    )
    .normalize()
    .as_vec3()
}

/// The racing line: the oval's center line, `n` points.
pub fn oval_path(center: DVec3, n: usize) -> Vec<DVec3> {
    (0..n)
        .map(|i| oval_point(center, i as f64 / n as f64 * std::f64::consts::TAU))
        .collect()
}

/// Rotation that points a vehicle's nose (−Z) along `dir` (horizontal).
pub fn facing(dir: Vec3) -> Quat {
    Quat::from_rotation_y((-dir.x).atan2(-dir.z))
}

fn material(hex: u32, roughness: f32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(roughness)
}

/// A static box with a matching collider: `size` full extents, `surface` by material name.
fn slab(
    commands: &mut Commands,
    size: Vec3,
    transform: Transform,
    visual: Option<StandardMaterial>,
    surface: &'static str,
) {
    let mut e = commands.spawn((transform, RigidBody::fixed()));
    if let Some(m) = visual {
        e.insert(MeshRenderer::new(
            Shape::Cuboid {
                size: size.to_array(),
            },
            m,
        ));
    }
    let id = e.id();
    commands.queue(move |world: &mut World| {
        let surface = world
            .get_resource::<SurfaceMaterials>()
            .map(|m| m.id_or_default(surface))
            .unwrap_or_default();
        if let Ok(mut e) = world.get_entity_mut(id) {
            e.insert(Collider::cuboid(size * 0.5).with_surface(surface));
        }
    });
}

fn disc(
    commands: &mut Commands,
    radius: f32,
    height: f32,
    transform: Transform,
    visual: Option<StandardMaterial>,
    surface: &'static str,
) {
    let mut e = commands.spawn((transform, RigidBody::fixed()));
    if let Some(m) = visual {
        e.insert(MeshRenderer::new(
            Shape::Cylinder {
                radius,
                height,
                segments: 64,
            },
            m,
        ));
    }
    let id = e.id();
    commands.queue(move |world: &mut World| {
        let surface = world
            .get_resource::<SurfaceMaterials>()
            .map(|m| m.id_or_default(surface))
            .unwrap_or_default();
        if let Ok(mut e) = world.get_entity_mut(id) {
            e.insert(Collider {
                shape: ColliderShape::Cylinder {
                    radius,
                    half_height: height * 0.5,
                },
                surface,
                ..Default::default()
            });
        }
    });
}

/// Spawn a preset already moving at `speed` along its nose (so the demo is in full swing
/// the moment a camera arrives).
fn moving(
    commands: &mut Commands,
    preset: &str,
    position: DVec3,
    dir: Vec3,
    speed: f32,
    name: &str,
    mode: AutopilotMode,
) -> Entity {
    let t = Transform::from_translation(position).with_rotation(facing(dir));
    let e = presets::spawn(commands, preset, t).expect("preset");
    commands.entity(e).insert((
        Name::new(name.to_string()),
        Velocity {
            linear: dir * speed,
            angular: Vec3::ZERO,
        },
        Autopilot::new(mode),
    ));
    e
}

/// The ground under the zone: a grass collider (the zone's grass is drawn by the game).
fn ground(commands: &mut Commands, center: DVec3, half: Vec3) {
    slab(
        commands,
        Vec3::new(half.x * 2.0, 1.0, half.z * 2.0),
        Transform::from_translation(center + DVec3::new(0.0, -0.5, 0.0)),
        None,
        "grass",
    );
}

/// Both vehicle demos side by side: the track at `origin` and the spaceport 300 m to its east.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    spawn_track(commands, origin);
    let pad = origin + DVec3::new(300.0, 0.4, 0.0);
    spawn_spaceport(commands, pad, 30.0);
}

/// The vehicle track, centered on `center` (a point on flat ground). Fits in a 340 × 200 m
/// rectangle.
pub fn spawn_track(commands: &mut Commands, center: DVec3) {
    let asphalt = material(0x2c2c2e, 0.75);
    let curb_red = material(0xb3261e, 0.6);
    let curb_white = material(0xe8e6e1, 0.6);
    let line = material(0xf2f0e6, 0.55);
    let concrete = material(0xa8a49c, 0.8);
    let dirt = material(0x7a5a3c, 0.95);
    let (rx, rz) = TRACK_RADII;

    ground(
        commands,
        center,
        Vec3::new((rx + 40.0) as f32, 0.0, (rz + 40.0) as f32),
    );

    // The oval: asphalt segments with red and white curbs on both edges.
    for i in 0..SEGMENTS {
        let a0 = i as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
        let a1 = (i + 1) as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
        let p0 = oval_point(center, a0);
        let p1 = oval_point(center, a1);
        let mid = (p0 + p1) * 0.5;
        let dir = (p1 - p0).as_vec3();
        let len = dir.length();
        let rot = facing(dir / len);
        slab(
            commands,
            Vec3::new(TRACK_WIDTH, 0.12, len + 0.4),
            Transform::from_translation(mid - DVec3::Y * 0.06).with_rotation(rot),
            Some(asphalt),
            "concrete",
        );
        let curb = if i % 2 == 0 { curb_red } else { curb_white };
        for side in [-1.0f32, 1.0] {
            let offset = rot * Vec3::X * side * (TRACK_WIDTH * 0.5 + 0.5);
            slab(
                commands,
                Vec3::new(1.0, 0.1, len + 0.05),
                Transform::from_translation(mid + offset.as_dvec3() - DVec3::Y * 0.04)
                    .with_rotation(rot),
                Some(curb),
                "concrete",
            );
        }
    }
    // Start/finish line.
    let start = oval_point(center, 0.0);
    commands.spawn((
        Transform::from_translation(start + DVec3::Y * 0.004)
            .with_rotation(facing(oval_direction(0.0))),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [TRACK_WIDTH, 0.01, 1.2],
            },
            line,
        ),
    ));

    // The jump: a concrete kicker across the track, 7.5° up over 6 m.
    let jump = oval_point(center, JUMP_ANGLE);
    let dir = oval_direction(JUMP_ANGLE);
    let (len, angle, thick) = (6.0f32, 7.5f32.to_radians(), 0.6f32);
    let rot = facing(dir) * Quat::from_rotation_x(angle);
    // Put the low end of the top face exactly on the asphalt.
    let center_of_ramp = jump - (rot * Vec3::new(0.0, thick * 0.5, len * 0.5)).as_dvec3();
    slab(
        commands,
        Vec3::new(TRACK_WIDTH, thick, len),
        Transform::from_translation(center_of_ramp).with_rotation(rot),
        Some(concrete),
        "concrete",
    );
    for k in 0..4 {
        // Yellow chevrons on the kicker so it reads from the air.
        let along = -len * 0.5 + 0.9 + k as f32 * 1.4;
        commands.spawn((
            Transform::from_translation(
                center_of_ramp + (rot * Vec3::new(0.0, thick * 0.5 + 0.005, along)).as_dvec3(),
            )
            .with_rotation(rot),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [TRACK_WIDTH * 0.8, 0.01, 0.35],
                },
                material(0xe0b020, 0.6),
            ),
        ));
    }

    // Drift pad: packed dirt inside the oval.
    let pad = center + DVec3::new(-70.0, 0.0, -20.0);
    disc(
        commands,
        26.0,
        0.12,
        Transform::from_translation(pad),
        Some(dirt),
        "dirt",
    );

    // Runway inside the oval, along +X.
    let runway = center + DVec3::new(0.0, 0.0, 32.0);
    slab(
        commands,
        Vec3::new(240.0, 0.12, 22.0),
        Transform::from_translation(runway),
        Some(material(0x3a3a3c, 0.8)),
        "concrete",
    );
    for k in 0..12 {
        let x = -110.0 + k as f64 * 20.0;
        commands.spawn((
            Transform::from_translation(runway + DVec3::new(x, 0.065, 0.0)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [9.0, 0.01, 0.6],
                },
                line,
            ),
        ));
    }
    for end in [-1.0f64, 1.0] {
        for k in 0..6 {
            let z = -7.5 + k as f64 * 3.0;
            commands.spawn((
                Transform::from_translation(runway + DVec3::new(end * 114.0, 0.065, z)),
                MeshRenderer::new(
                    Shape::Cuboid {
                        size: [8.0, 0.01, 1.4],
                    },
                    line,
                ),
            ));
        }
    }

    // Two sports cars racing round the oval.
    let path = oval_path(center, SEGMENTS * 2);
    let race = |max_speed: f32| AutopilotMode::Drive {
        path: path.clone(),
        looped: true,
        max_speed,
        lateral_g: 0.95,
    };
    let lift = DVec3::Y * presets::ride_height("sports_car");
    for (angle, speed, name, paint) in [
        (0.25f64, 34.0f32, "Red car", 0xc4161cu32),
        (-0.35, 32.0, "Blue car", 0x1d4fb8),
    ] {
        let p = oval_point(center, angle) + lift;
        let dir = oval_direction(angle);
        let t = Transform::from_translation(p).with_rotation(facing(dir));
        let e = presets::sports_car(commands, t, paint);
        commands.entity(e).insert((
            Name::new(name),
            Velocity {
                linear: dir * speed,
                angular: Vec3::ZERO,
            },
            Autopilot::new(race(46.0)),
        ));
    }

    // Rally car sliding round the dirt pad.
    let radius = 15.0f32;
    let drift_start = pad
        + DVec3::new(
            radius as f64,
            TRACK_SURFACE + presets::ride_height("rally_car"),
            0.0,
        );
    moving(
        commands,
        "rally_car",
        drift_start,
        Vec3::NEG_Z,
        9.0,
        "Rally car",
        AutopilotMode::Drift {
            center: pad,
            radius,
            speed: 10.0,
            slip_deg: 30.0,
            clockwise: false,
        },
    );

    // Planes: one flying circuits over the track, one taking off to join it.
    let alt = 140.0;
    let circuit: Vec<DVec3> = (0..8)
        .map(|i| {
            let a = i as f64 / 8.0 * std::f64::consts::TAU;
            center + DVec3::new(a.cos() * 420.0, alt, a.sin() * 260.0)
        })
        .collect();
    let fly = AutopilotMode::Fly {
        waypoints: circuit.clone(),
        looped: true,
        cruise_speed: 60.0,
        rotate_speed: 32.0,
    };
    let first = circuit[0];
    let toward = (circuit[1] - first).normalize().as_vec3();
    moving(
        commands,
        "sport_plane",
        first,
        toward,
        60.0,
        "Circuit plane",
        fly.clone(),
    );
    let threshold = runway
        + DVec3::new(
            -108.0,
            TRACK_SURFACE + presets::ride_height("sport_plane"),
            0.0,
        );
    let t = Transform::from_translation(threshold).with_rotation(facing(Vec3::X));
    let e = presets::sport_plane(commands, t);
    commands
        .entity(e)
        .insert((Name::new("Departing plane"), Autopilot::new(fly)));
}

/// A spaceplane hopping off the landing pad at `pad` (center of its top surface) and one
/// parked beside it. `pad_radius` is the pad's size (a collider is added; the pad is drawn
/// by the game).
pub fn spawn_spaceport(commands: &mut Commands, pad: DVec3, pad_radius: f32) {
    let thick = 0.4f32;
    disc(
        commands,
        pad_radius,
        thick,
        Transform::from_translation(pad - DVec3::Y * (thick as f64 * 0.5)),
        None,
        "concrete",
    );
    ground(
        commands,
        pad - DVec3::Y * thick as f64,
        Vec3::new(130.0, 0.0, 130.0),
    );
    // An apron for the parked ship.
    let apron = pad + DVec3::new(-58.0, 0.0, 6.0);
    slab(
        commands,
        Vec3::new(34.0, 0.12, 40.0),
        Transform::from_translation(DVec3::new(apron.x, pad.y - thick as f64 + 0.06, apron.z)),
        Some(material(0x9a968e, 0.8)),
        "concrete",
    );
    let hover = 18.0;
    let rest = presets::ride_height("spaceplane");
    let ship_at = pad + DVec3::Y * (rest + hover as f64);
    let e = presets::spaceplane(
        commands,
        Transform::from_translation(ship_at).with_rotation(facing(Vec3::NEG_Z)),
    );
    commands.entity(e).insert((
        Name::new("Hopper"),
        Autopilot::new(AutopilotMode::Hop {
            pad,
            height_m: hover,
            hover_s: 14.0,
            rest_s: 5.0,
        }),
    ));
    let parked_at = DVec3::new(apron.x, pad.y - thick as f64 + 0.12, apron.z) + DVec3::Y * rest;
    let e = presets::spaceplane(
        commands,
        Transform::from_translation(parked_at)
            .with_rotation(facing(Vec3::new(0.3, 0.0, -1.0).normalize())),
    );
    commands.entity(e).insert(Name::new("Parked spaceplane"));
}
