//! Ready-made vehicles with realistic numbers, each a starting point to tune:
//!
//! | Preset | What | Numbers |
//! |---|---|---|
//! | [`sports_car`] | Rear-drive coupé with a limited-slip diff | 1 350 kg, 330 hp, 6 gears, ~300 km/h |
//! | [`rally_car`] | All-wheel-drive rally car on long-travel suspension and gravel tires | 1 250 kg, 300 hp, 5 gears |
//! | [`sport_plane`] | Single-engine low-wing propeller plane | 900 kg, 10.5 m span, stalls ≈ 27 m/s |
//! | [`spaceplane`] | VTOL spaceplane: hover engines, three main engines, 20 RCS thrusters, heat shield | 80 t, 1.8 MN main thrust |
//!
//! Every preset spawns the physics body, the vehicle components and its drawn parts (with
//! moving wheels, control surfaces, propeller, plumes and lights). `spawn(commands, kind,
//! transform)` spawns one by name, which is what `vehicles.spawn` uses.

use engine_core::math::{Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::physics::{Collider, ColliderShape, RigidBody, Velocity};
use engine_world::{SurfaceId, VehicleControls};

use crate::aero::{
    AeroSurface, Aircraft, ControlAxis, Propulsion, Propulsor, PropulsorKind, SurfaceControl,
};
use crate::chassis::{AeroBody, Vehicle, VehicleKind};
use crate::drivetrain::{Differential, DriveAxle, Drivetrain, EngineSpec, Gearbox};
use crate::meshes;
use crate::ship::{FlightAssist, HeatShield, Spacecraft, Thruster, ThrusterRole};
use crate::tire::{MagicFormula, TireModel};
use crate::visuals::{BrakeLight, WheelVisual};
use crate::wheels::{AntiRollBar, Assists, SteerInput, Suspension, Wheel, Wheels};

/// Every preset by name.
pub const PRESETS: &[(&str, &str)] = &[
    (
        "sports_car",
        "Rear-drive coupé: 1 350 kg, 330 hp, limited-slip diff, 6-speed automatic.",
    ),
    (
        "rally_car",
        "All-wheel drive rally car: long-travel suspension, gravel tires, 300 hp.",
    ),
    (
        "sport_plane",
        "Single-engine propeller plane: 900 kg, 10.5 m wings, flaps, tricycle gear.",
    ),
    (
        "spaceplane",
        "VTOL spaceplane: hover engines, 3 main engines (1.8 MN), RCS, heat shield, 80 t.",
    ),
];

/// Height of a preset's body origin above flat ground when resting on its wheels, meters
/// (spawn it this high so it settles without a drop).
pub fn ride_height(kind: &str) -> f64 {
    match kind {
        "sports_car" => 0.45,
        "rally_car" => 0.45,
        "sport_plane" => 0.87,
        "spaceplane" => 2.8,
        _ => 1.0,
    }
}

/// Spawn a preset by name. `None` if the name is unknown (see [`PRESETS`]).
pub fn spawn(commands: &mut Commands, kind: &str, transform: Transform) -> Option<Entity> {
    Some(match kind {
        "sports_car" => sports_car(commands, transform, 0xc4161c),
        "rally_car" => rally_car(commands, transform),
        "sport_plane" => sport_plane(commands, transform),
        "spaceplane" => spaceplane(commands, transform),
        _ => return None,
    })
}

// ---------------------------------------------------------------- shared drawing helpers

fn register_meshes(commands: &mut Commands) {
    commands.queue(|world: &mut World| {
        if let Some(mut m) = world.get_resource_mut::<engine_assets::Assets<engine_render::Mesh>>()
        {
            meshes::register(&mut m);
        }
    });
}

fn part(
    commands: &mut Commands,
    parent: Entity,
    mesh: impl Into<MeshRef>,
    material: StandardMaterial,
    transform: Transform,
) -> Entity {
    commands
        .spawn((
            transform,
            MeshRenderer::new(mesh, material),
            ChildOf(parent),
        ))
        .id()
}

fn asset(key: &str) -> MeshRef {
    MeshRef::Asset(key.to_string())
}

fn at(x: f32, y: f32, z: f32) -> Transform {
    Transform::from_xyz(x as f64, y as f64, z as f64)
}

fn rubber() -> StandardMaterial {
    StandardMaterial::hex(0x141414).with_roughness(0.92)
}

fn chrome() -> StandardMaterial {
    StandardMaterial::hex(0xc9ccd1)
        .with_metallic(1.0)
        .with_roughness(0.22)
}

fn glass() -> StandardMaterial {
    let mut m = StandardMaterial::hex(0x0b1118)
        .with_roughness(0.04)
        .with_metallic(0.1);
    m.reflectance = 0.9;
    m.receives_wetness = false;
    m
}

/// A drawn wheel: tire, rim, spokes (so spin shows) and a hub cap.
fn wheel_visual(commands: &mut Commands, parent: Entity, w: &Wheel, index: usize, retracts: bool) {
    let mount = w.mount - Vec3::Y * w.suspension.travel_m * 0.6;
    let wheel = commands
        .spawn((
            Transform::from_translation(mount.as_dvec3()),
            WheelVisual { index, retracts },
            ChildOf(parent),
        ))
        .id();
    let side = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let outward = if w.mount.x >= 0.0 { 1.0 } else { -1.0 };
    part(
        commands,
        wheel,
        Shape::Cylinder {
            radius: w.radius,
            height: w.width,
            segments: 28,
        },
        rubber(),
        Transform::from_rotation(side),
    );
    part(
        commands,
        wheel,
        Shape::Cylinder {
            radius: w.radius * 0.66,
            height: w.width + 0.012,
            segments: 24,
        },
        chrome(),
        Transform::from_rotation(side),
    );
    let face = (w.width * 0.5 + 0.012) * outward;
    for k in 0..5 {
        let a = k as f32 / 5.0 * std::f32::consts::TAU;
        part(
            commands,
            wheel,
            Shape::Cuboid {
                size: [0.03, w.radius * 1.25, w.radius * 0.16],
            },
            StandardMaterial::hex(0x2a2c30)
                .with_metallic(0.8)
                .with_roughness(0.35),
            Transform::from_translation(Vec3::new(face, 0.0, 0.0).as_dvec3())
                .with_rotation(Quat::from_rotation_x(a)),
        );
    }
}

fn tire(grip: f32) -> TireModel {
    TireModel {
        grip,
        ..Default::default()
    }
}

// ---------------------------------------------------------------- cars

/// The sports car's simulation data (no drawing): wheels and drivetrain.
pub fn sports_car_wheels() -> Wheels {
    let susp = |corner| Suspension::tuned(0.20, corner, 0.4, 0.45);
    let front = |x: f32, name: &str| Wheel {
        name: name.into(),
        mount: Vec3::new(x, 0.0, -1.35),
        radius: 0.33,
        width: 0.25,
        inertia: 1.2,
        suspension: susp(350.0),
        tire: tire(1.2),
        steer: SteerInput::Steer,
        max_steer_deg: 34.0,
        brake_nm: 2_400.0,
        handbrake_nm: 0.0,
        ..Default::default()
    };
    let rear = |x: f32, name: &str| Wheel {
        name: name.into(),
        mount: Vec3::new(x, 0.0, 1.30),
        radius: 0.33,
        width: 0.27,
        inertia: 1.3,
        suspension: susp(325.0),
        tire: tire(1.2),
        brake_nm: 1_500.0,
        handbrake_nm: 3_200.0,
        ..Default::default()
    };
    Wheels {
        wheels: vec![
            front(-0.80, "front left"),
            front(0.80, "front right"),
            rear(-0.80, "rear left"),
            rear(0.80, "rear right"),
        ],
        anti_roll: vec![
            AntiRollBar {
                left: 0,
                right: 1,
                stiffness: 20_000.0,
            },
            AntiRollBar {
                left: 2,
                right: 3,
                stiffness: 9_000.0,
            },
        ],
        ..Default::default()
    }
}

/// A rear-drive sports car in `paint` (sRGB hex).
pub fn sports_car(commands: &mut Commands, transform: Transform, paint: u32) -> Entity {
    register_meshes(commands);
    let wheels = sports_car_wheels();
    let body = commands
        .spawn((
            transform,
            TransformInterpolation::default(),
            Vehicle::new(
                "sports car",
                VehicleKind::Car,
                1_350.0,
                Vec3::new(1.8, 1.0, 4.4),
            ),
            RigidBody {
                ccd: true,
                can_sleep: false,
                angular_damping: 0.05,
                ..RigidBody::dynamic().with_mass(1_350.0)
            },
            Collider::cuboid(Vec3::new(0.88, 0.32, 2.2)).with_surface(SurfaceId(11)),
            Velocity::default(),
            VehicleControls::default(),
            Drivetrain::default(),
            AeroBody {
                drag_area: Vec3::new(2.4, 3.2, 0.62),
                downforce_area: 0.3,
                downforce_center: Vec3::new(0.0, 0.0, 0.2),
                angular_drag: 0.5,
            },
        ))
        .id();
    for (i, w) in wheels.wheels.iter().enumerate() {
        wheel_visual(commands, body, w, i, false);
    }
    commands.entity(body).insert(wheels);
    let paint = StandardMaterial::hex(paint)
        .with_metallic(0.55)
        .with_roughness(0.26);
    part(
        commands,
        body,
        asset(meshes::CAR_BODY),
        paint,
        Transform::default(),
    );
    part(
        commands,
        body,
        asset(meshes::CAR_CABIN),
        glass(),
        Transform::default(),
    );
    car_lights(commands, body, 2.2, 0.05, 0.62);
    // A small rear wing.
    part(
        commands,
        body,
        Shape::Cuboid {
            size: [1.5, 0.04, 0.32],
        },
        StandardMaterial::hex(0x18181a).with_roughness(0.5),
        at(0.0, 0.62, 2.0),
    );
    for x in [-0.55f32, 0.55] {
        part(
            commands,
            body,
            Shape::Cuboid {
                size: [0.04, 0.22, 0.12],
            },
            StandardMaterial::hex(0x18181a).with_roughness(0.5),
            at(x, 0.5, 2.0),
        );
    }
    body
}

/// Headlights, brake lights and reverse lights.
fn car_lights(commands: &mut Commands, body: Entity, half_length: f32, y: f32, x: f32) {
    let head = StandardMaterial::hex(0xf4f4ee).with_emissive([1.0, 0.97, 0.9], 2_500.0);
    for side in [-1.0f32, 1.0] {
        part(
            commands,
            body,
            Shape::Cuboid {
                size: [0.36, 0.09, 0.05],
            },
            head,
            at(side * x, y, -half_length - 0.01),
        );
        let off = StandardMaterial::hex(0x5a0808).with_emissive([1.0, 0.03, 0.02], 60.0);
        let on = StandardMaterial::hex(0x8a0a0a).with_emissive([1.0, 0.03, 0.02], 1_800.0);
        commands.spawn((
            at(side * x, y + 0.08, half_length + 0.02),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.42, 0.08, 0.05],
                },
                off,
            ),
            BrakeLight {
                off,
                on,
                reverse: false,
            },
            ChildOf(body),
        ));
        let rev_off = StandardMaterial::hex(0x9a9a9a).with_roughness(0.2);
        let rev_on = rev_off.with_emissive([1.0, 1.0, 1.0], 1_500.0);
        commands.spawn((
            at(side * (x - 0.3), y + 0.08, half_length + 0.02),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.12, 0.06, 0.05],
                },
                rev_off,
            ),
            BrakeLight {
                off: rev_off,
                on: rev_on,
                reverse: true,
            },
            ChildOf(body),
        ));
    }
}

/// The rally car's wheels: long travel, gravel tires.
pub fn rally_car_wheels() -> Wheels {
    let gravel = TireModel {
        grip: 1.02,
        // Gravel tires: softer, peak later (≈ 10°) and slide more progressively.
        lateral: MagicFormula {
            b: 10.0,
            c: 1.45,
            e: -0.3,
        },
        surface_sensitivity: 0.55,
        rolling_resistance: 0.015,
        ..Default::default()
    };
    let susp = |corner| Suspension::tuned(0.28, corner, 0.35, 0.4);
    let wheel = |x: f32, z: f32, front: bool, name: &str| Wheel {
        name: name.into(),
        mount: Vec3::new(x, 0.05, z),
        radius: 0.32,
        width: 0.22,
        inertia: 1.1,
        suspension: susp(if front { 330.0 } else { 300.0 }),
        tire: gravel,
        steer: if front {
            SteerInput::Steer
        } else {
            SteerInput::None
        },
        max_steer_deg: if front { 36.0 } else { 0.0 },
        brake_nm: if front { 2_000.0 } else { 1_500.0 },
        handbrake_nm: if front { 0.0 } else { 3_600.0 },
        ..Default::default()
    };
    Wheels {
        wheels: vec![
            wheel(-0.79, -1.25, true, "front left"),
            wheel(0.79, -1.25, true, "front right"),
            wheel(-0.79, 1.25, false, "rear left"),
            wheel(0.79, 1.25, false, "rear right"),
        ],
        anti_roll: vec![
            AntiRollBar {
                left: 0,
                right: 1,
                stiffness: 12_000.0,
            },
            AntiRollBar {
                left: 2,
                right: 3,
                stiffness: 8_000.0,
            },
        ],
        assists: Assists {
            abs: false,
            traction_control: false,
            steering_limit: 0.3,
            stability_deg: 0.0,
        },
        ..Default::default()
    }
}

/// An all-wheel-drive rally car.
pub fn rally_car(commands: &mut Commands, transform: Transform) -> Entity {
    register_meshes(commands);
    let wheels = rally_car_wheels();
    let lsd = Differential::Limited {
        preload_nm: 120.0,
        lock: 0.45,
    };
    let drivetrain = Drivetrain {
        engine: EngineSpec {
            torque_curve: vec![
                [900.0, 200.0],
                [2_500.0, 380.0],
                [4_500.0, 400.0],
                [6_500.0, 330.0],
                [7_000.0, 280.0],
            ],
            idle_rpm: 950.0,
            redline_rpm: 7_000.0,
            inertia: 0.2,
            engine_brake_nm: 90.0,
        },
        gearbox: Gearbox {
            ratios: vec![3.3, 2.1, 1.6, 1.25, 1.0],
            final_drive: 4.3,
            upshift_rpm: 6_600.0,
            downshift_rpm: 3_600.0,
            launch_rpm: 4_000.0,
            ..Default::default()
        },
        axles: vec![
            DriveAxle {
                left: 0,
                right: 1,
                // Rear-biased like a modern rally car: the tail steps out under power.
                split: 0.3,
                differential: lsd,
            },
            DriveAxle {
                left: 2,
                right: 3,
                split: 0.7,
                differential: lsd,
            },
        ],
        ..Default::default()
    };
    let body = commands
        .spawn((
            transform,
            TransformInterpolation::default(),
            Vehicle::new(
                "rally car",
                VehicleKind::Car,
                1_250.0,
                Vec3::new(1.75, 1.1, 4.1),
            ),
            RigidBody {
                ccd: true,
                can_sleep: false,
                ..RigidBody::dynamic().with_mass(1_250.0)
            },
            Collider::cuboid(Vec3::new(0.86, 0.36, 2.0)).with_surface(SurfaceId(11)),
            Velocity::default(),
            VehicleControls::default(),
            drivetrain,
            AeroBody {
                drag_area: Vec3::new(2.6, 3.4, 0.72),
                downforce_area: 0.15,
                downforce_center: Vec3::new(0.0, 0.0, 0.4),
                angular_drag: 0.5,
            },
        ))
        .id();
    for (i, w) in wheels.wheels.iter().enumerate() {
        wheel_visual(commands, body, w, i, false);
    }
    commands.entity(body).insert(wheels);
    let blue = StandardMaterial::hex(0x1747a6)
        .with_metallic(0.4)
        .with_roughness(0.32);
    part(
        commands,
        body,
        asset(meshes::RALLY_BODY),
        blue,
        Transform::default(),
    );
    part(
        commands,
        body,
        asset(meshes::RALLY_CABIN),
        glass(),
        Transform::default(),
    );
    // Livery stripe, roof scoop and mud flaps.
    let white = StandardMaterial::hex(0xf2f2f2).with_roughness(0.4);
    part(
        commands,
        body,
        Shape::Cuboid {
            size: [0.36, 0.02, 4.0],
        },
        white,
        at(0.0, 0.47, 0.0),
    );
    part(
        commands,
        body,
        Shape::Cuboid {
            size: [0.5, 0.12, 0.5],
        },
        StandardMaterial::hex(0x202022).with_roughness(0.6),
        at(0.0, 1.05, 0.1),
    );
    for x in [-0.8f32, 0.8] {
        part(
            commands,
            body,
            Shape::Cuboid {
                size: [0.26, 0.3, 0.02],
            },
            StandardMaterial::hex(0xe03a1e).with_roughness(0.7),
            at(x, -0.32, 1.66),
        );
    }
    car_lights(commands, body, 2.05, 0.1, 0.6);
    body
}

// ---------------------------------------------------------------- aircraft

/// The sport plane's lifting surfaces.
pub fn sport_plane_aircraft() -> Aircraft {
    let dihedral = 0.30f32.atan2(4.7);
    let wing = |name: &str, x: f32, span: f32, chord: f32, control: SurfaceControl, ff: f32| {
        let side = x.signum();
        let rise = (x.abs() - 0.55) * dihedral.tan();
        AeroSurface {
            name: name.into(),
            position: Vec3::new(x, -0.38 + rise, 0.05),
            // 1.5° incidence: the fuselage flies level at cruise.
            rotation: Quat::from_rotation_z(dihedral * side)
                * Quat::from_rotation_x(1.5f32.to_radians()),
            chord,
            span,
            aspect_ratio: Some(7.5),
            skin_friction: 0.012,
            zero_lift_aoa_deg: -2.5,
            stall_high_deg: 14.0,
            stall_low_deg: -14.0,
            flap_fraction: ff,
            control: Some(control),
            ..Default::default()
        }
    };
    let flaps = SurfaceControl {
        axis: ControlAxis::Flaps,
        gain: 1.0,
        max_deg: 30.0,
        rate_deg_s: 15.0,
    };
    let aileron = |gain| SurfaceControl {
        axis: ControlAxis::Roll,
        gain,
        max_deg: 20.0,
        rate_deg_s: 120.0,
    };
    let elevator = SurfaceControl {
        axis: ControlAxis::Pitch,
        gain: -1.0,
        max_deg: 30.0,
        rate_deg_s: 90.0,
    };
    // Tailplane set 1° leading edge down (2.5° less than the wing): hands off, the plane
    // trims near cruise speed.
    let tail = |name: &str, x: f32| AeroSurface {
        name: name.into(),
        position: Vec3::new(x, 0.32, 4.25),
        rotation: Quat::from_rotation_x(-1f32.to_radians()),
        skin_friction: 0.012,
        chord: 0.75,
        span: 1.6,
        aspect_ratio: Some(4.5),
        flap_fraction: 0.45,
        control: Some(elevator),
        ..Default::default()
    };
    Aircraft {
        surfaces: vec![
            wing("left wing, inner", -1.70, 2.3, 1.45, flaps, 0.25),
            wing("right wing, inner", 1.70, 2.3, 1.45, flaps, 0.25),
            wing(
                "left wing, outer (aileron)",
                -4.05,
                2.4,
                1.2,
                aileron(1.0),
                0.25,
            ),
            wing(
                "right wing, outer (aileron)",
                4.05,
                2.4,
                1.2,
                aileron(-1.0),
                0.25,
            ),
            tail("left tailplane", -1.05),
            tail("right tailplane", 1.05),
            AeroSurface {
                name: "fin (rudder)".into(),
                position: Vec3::new(0.0, 1.05, 4.3),
                rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                chord: 0.95,
                span: 1.5,
                aspect_ratio: Some(2.5),
                flap_fraction: 0.4,
                control: Some(SurfaceControl {
                    axis: ControlAxis::Yaw,
                    gain: 1.0,
                    max_deg: 25.0,
                    rate_deg_s: 90.0,
                }),
                ..Default::default()
            },
        ],
        flaps: 0.0,
        pitch_trim: 0.0,
        gear_down: true,
    }
}

/// The sport plane's landing gear (tricycle).
pub fn sport_plane_wheels() -> Wheels {
    let gear_tire = TireModel {
        grip: 0.9,
        rolling_resistance: 0.02,
        ..Default::default()
    };
    Wheels {
        wheels: vec![
            Wheel {
                name: "nose gear".into(),
                mount: Vec3::new(0.0, -0.55, -2.4),
                radius: 0.2,
                width: 0.12,
                inertia: 0.15,
                suspension: Suspension::tuned(0.2, 140.0, 0.4, 0.5),
                tire: gear_tire,
                steer: SteerInput::Yaw,
                max_steer_deg: 22.0,
                brake_nm: 0.0,
                ..Default::default()
            },
            Wheel {
                name: "left main gear".into(),
                mount: Vec3::new(-1.2, -0.5, 0.45),
                radius: 0.25,
                width: 0.14,
                inertia: 0.3,
                suspension: Suspension::tuned(0.2, 380.0, 0.4, 0.5),
                tire: gear_tire,
                brake_nm: 700.0,
                ..Default::default()
            },
            Wheel {
                name: "right main gear".into(),
                mount: Vec3::new(1.2, -0.5, 0.45),
                radius: 0.25,
                width: 0.14,
                inertia: 0.3,
                suspension: Suspension::tuned(0.2, 380.0, 0.4, 0.5),
                tire: gear_tire,
                brake_nm: 700.0,
                ..Default::default()
            },
        ],
        steer_at_speed: 0.5,
        assists: Assists {
            abs: true,
            traction_control: false,
            steering_limit: 0.0,
            stability_deg: 0.0,
        },
        ..Default::default()
    }
}

/// A single-engine sport plane.
pub fn sport_plane(commands: &mut Commands, transform: Transform) -> Entity {
    let aircraft = sport_plane_aircraft();
    let wheels = sport_plane_wheels();
    let body = commands
        .spawn((
            transform,
            TransformInterpolation::default(),
            Vehicle {
                name: "sport plane".into(),
                kind: VehicleKind::Aircraft,
                mass_kg: 900.0,
                // Real light aircraft: pitch, yaw, roll.
                inertia: Vec3::new(1_480.0, 2_130.0, 1_070.0),
                center_of_mass: Vec3::ZERO,
            },
            RigidBody {
                ccd: true,
                can_sleep: false,
                angular_damping: 0.1,
                ..RigidBody::dynamic().with_mass(900.0)
            },
            Collider {
                shape: ColliderShape::Compound(vec![
                    // Cabin, then the slimmer, raised tail cone (so the tail clears the
                    // runway when the plane rotates on its main gear). Sized so the volume
                    // center (where a backend puts the center of mass) stays near the wing.
                    (
                        Vec3::new(0.0, 0.0, -0.6),
                        Quat::IDENTITY,
                        ColliderShape::Cuboid {
                            half_extents: Vec3::new(0.6, 0.6, 2.2),
                        },
                    ),
                    (
                        Vec3::new(0.0, 0.2, 3.0),
                        Quat::IDENTITY,
                        ColliderShape::Cuboid {
                            half_extents: Vec3::new(0.3, 0.35, 1.4),
                        },
                    ),
                    (
                        Vec3::new(0.0, -0.36, 0.05),
                        Quat::IDENTITY,
                        ColliderShape::Cuboid {
                            half_extents: Vec3::new(5.2, 0.08, 0.7),
                        },
                    ),
                ]),
                surface: SurfaceId(11),
                ..Default::default()
            },
            Velocity::default(),
            VehicleControls::default(),
            Propulsion {
                engines: vec![Propulsor {
                    name: "propeller".into(),
                    position: Vec3::new(0.0, 0.0, -3.4),
                    direction: Vec3::NEG_Z,
                    // An aerobatic-class 220 kW engine and prop.
                    max_thrust_n: 6_000.0,
                    kind: PropulsorKind::Propeller {
                        zero_thrust_speed: 110.0,
                    },
                    spool_secs: 0.8,
                    idle: 0.04,
                    ..Default::default()
                }],
            },
            AeroBody {
                drag_area: Vec3::new(2.5, 4.0, 0.15),
                downforce_area: 0.0,
                downforce_center: Vec3::ZERO,
                angular_drag: 2.0,
            },
        ))
        .id();
    // Drawn parts: fuselage, wings, moving control surfaces, propeller, gear and lights.
    crate::models::aircraft::spawn_sport_plane_parts(commands, body, &aircraft, &wheels);
    commands.entity(body).insert((aircraft, wheels));
    body
}

// ---------------------------------------------------------------- spacecraft

/// The spaceplane's thrusters: 3 mains, 4 hover engines, 20 RCS.
pub fn spaceplane_thrusters() -> Vec<Thruster> {
    let mut t = Vec::new();
    for (i, (x, y)) in [(0.0, 0.3), (-1.5, -0.35), (1.5, -0.35)]
        .into_iter()
        .enumerate()
    {
        t.push(Thruster {
            name: format!("main engine {}", i + 1),
            position: Vec3::new(x, y, 10.6),
            direction: Vec3::NEG_Z,
            max_thrust_n: 600_000.0,
            role: ThrusterRole::Main,
            isp_s: 3_000.0,
            response_secs: 0.6,
            level: 0.0,
        });
    }
    for (i, (x, z)) in [(-1.9, -6.0), (1.9, -6.0), (-1.9, 6.0), (1.9, 6.0)]
        .into_iter()
        .enumerate()
    {
        t.push(Thruster {
            name: format!("hover engine {}", i + 1),
            position: Vec3::new(x, -1.5, z),
            direction: Vec3::Y,
            max_thrust_n: 360_000.0,
            role: ThrusterRole::Hover,
            isp_s: 2_000.0,
            response_secs: 0.25,
            level: 0.0,
        });
    }
    // RCS quads at the nose and tail: up, down, left, right, at both sides of the hull.
    for (end, z) in [("nose", -9.5f32), ("tail", 9.2)] {
        for (dir_name, pos, dir) in [
            ("up L", Vec3::new(-1.2, -1.4, z), Vec3::Y),
            ("up R", Vec3::new(1.2, -1.4, z), Vec3::Y),
            ("down L", Vec3::new(-1.2, 1.4, z), Vec3::NEG_Y),
            ("down R", Vec3::new(1.2, 1.4, z), Vec3::NEG_Y),
            ("left", Vec3::new(2.3, 0.0, z), Vec3::NEG_X),
            ("right", Vec3::new(-2.3, 0.0, z), Vec3::X),
        ] {
            t.push(Thruster {
                name: format!("rcs {end} {dir_name}"),
                position: pos,
                direction: dir,
                max_thrust_n: 30_000.0,
                role: ThrusterRole::Rcs,
                isp_s: 1_500.0,
                response_secs: 0.06,
                level: 0.0,
            });
        }
    }
    // Wingtip roll thrusters and nose retro thrusters.
    for (name, pos, dir) in [
        ("rcs wingtip L up", Vec3::new(-7.0, -0.9, 8.0), Vec3::Y),
        ("rcs wingtip R up", Vec3::new(7.0, -0.9, 8.0), Vec3::Y),
        (
            "rcs wingtip L down",
            Vec3::new(-7.0, -0.5, 8.0),
            Vec3::NEG_Y,
        ),
        ("rcs wingtip R down", Vec3::new(7.0, -0.5, 8.0), Vec3::NEG_Y),
        ("retro L", Vec3::new(-1.0, 0.0, -10.0), Vec3::Z),
        ("retro R", Vec3::new(1.0, 0.0, -10.0), Vec3::Z),
    ] {
        t.push(Thruster {
            name: name.into(),
            position: pos,
            direction: dir,
            max_thrust_n: 30_000.0,
            role: ThrusterRole::Rcs,
            isp_s: 1_500.0,
            response_secs: 0.06,
            level: 0.0,
        });
    }
    t
}

/// The spaceplane's wings (it glides in air) and fin.
pub fn spaceplane_aircraft() -> Aircraft {
    let elevon = |gain_roll: f32| -> (SurfaceControl, SurfaceControl) {
        (
            SurfaceControl {
                axis: ControlAxis::Pitch,
                gain: -1.0,
                max_deg: 20.0,
                rate_deg_s: 40.0,
            },
            SurfaceControl {
                axis: ControlAxis::Roll,
                gain: gain_roll,
                max_deg: 15.0,
                rate_deg_s: 40.0,
            },
        )
    };
    let delta = |name: &str, x: f32, roll_gain: f32| {
        let (pitch, roll) = elevon(roll_gain);
        AeroSurface {
            name: name.into(),
            position: Vec3::new(x, -0.8, 7.2),
            chord: 5.5,
            span: 5.0,
            aspect_ratio: Some(1.8),
            lift_slope: 4.5,
            stall_high_deg: 32.0,
            stall_low_deg: -32.0,
            flap_fraction: 0.15,
            control: Some(pitch),
            control2: Some(roll),
            ..Default::default()
        }
    };
    Aircraft {
        surfaces: vec![
            delta("left delta wing (elevon)", -4.9, 1.0),
            delta("right delta wing (elevon)", 4.9, -1.0),
            AeroSurface {
                name: "fin".into(),
                position: Vec3::new(0.0, 3.0, 8.4),
                rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                chord: 3.0,
                span: 3.2,
                aspect_ratio: Some(1.4),
                lift_slope: 4.0,
                stall_high_deg: 30.0,
                stall_low_deg: -30.0,
                flap_fraction: 0.2,
                control: Some(SurfaceControl {
                    axis: ControlAxis::Yaw,
                    gain: 1.0,
                    max_deg: 20.0,
                    rate_deg_s: 40.0,
                }),
                ..Default::default()
            },
        ],
        flaps: 0.0,
        pitch_trim: 0.0,
        gear_down: true,
    }
}

/// The spaceplane's landing gear.
pub fn spaceplane_wheels() -> Wheels {
    let gear = TireModel {
        grip: 0.9,
        nominal_load_n: 150_000.0,
        rolling_resistance: 0.02,
        ..Default::default()
    };
    let wheel = |name: &str, mount: Vec3, radius: f32, corner: f32, steer: bool| Wheel {
        name: name.into(),
        mount,
        radius,
        width: 0.45,
        inertia: 40.0,
        suspension: Suspension::tuned(0.5, corner, 0.4, 0.6),
        tire: gear,
        steer: if steer {
            SteerInput::Yaw
        } else {
            SteerInput::None
        },
        max_steer_deg: if steer { 25.0 } else { 0.0 },
        brake_nm: if steer { 0.0 } else { 120_000.0 },
        handbrake_nm: if steer { 0.0 } else { 120_000.0 },
        ..Default::default()
    };
    Wheels {
        wheels: vec![
            wheel("nose gear", Vec3::new(0.0, -2.0, -7.5), 0.5, 16_000.0, true),
            wheel(
                "left main gear",
                Vec3::new(-2.2, -1.9, 4.0),
                0.6,
                32_000.0,
                false,
            ),
            wheel(
                "right main gear",
                Vec3::new(2.2, -1.9, 4.0),
                0.6,
                32_000.0,
                false,
            ),
        ],
        assists: Assists {
            abs: true,
            traction_control: false,
            steering_limit: 0.0,
            stability_deg: 0.0,
        },
        ..Default::default()
    }
}

/// A VTOL spaceplane that can fly from the ground to orbit.
pub fn spaceplane(commands: &mut Commands, transform: Transform) -> Entity {
    let thrusters = spaceplane_thrusters();
    let wheels = spaceplane_wheels();
    let aircraft = spaceplane_aircraft();
    let dry = 30_000.0;
    let propellant = 50_000.0;
    let mass = dry + propellant;
    let body = commands
        .spawn((
            transform,
            TransformInterpolation::default(),
            Vehicle::new(
                "spaceplane",
                VehicleKind::Spacecraft,
                mass,
                Vec3::new(9.0, 3.4, 21.8),
            ),
            RigidBody {
                ccd: true,
                can_sleep: false,
                angular_damping: 0.0,
                linear_damping: 0.0,
                ..RigidBody::dynamic().with_mass(mass)
            },
            Collider {
                shape: ColliderShape::Compound(vec![
                    (
                        Vec3::ZERO,
                        Quat::IDENTITY,
                        ColliderShape::Cuboid {
                            half_extents: Vec3::new(2.5, 1.6, 10.8),
                        },
                    ),
                    (
                        Vec3::new(0.0, -0.8, 6.0),
                        Quat::IDENTITY,
                        ColliderShape::Cuboid {
                            half_extents: Vec3::new(7.3, 0.3, 3.5),
                        },
                    ),
                ]),
                surface: SurfaceId(11),
                ..Default::default()
            },
            Velocity::default(),
            VehicleControls::default(),
            AeroBody {
                drag_area: Vec3::new(40.0, 110.0, 6.0),
                downforce_area: 0.0,
                downforce_center: Vec3::ZERO,
                angular_drag: 3.0,
            },
            HeatShield {
                area_m2: 60.0,
                nose_radius_m: 2.5,
                // Thin ceramic tiles (≈ 25 kJ/K per m²): they reach radiative equilibrium
                // within a minute or so, glowing at ≈ 1 500 K at peak heating.
                heat_capacity_j_k: 1.5e6,
                ..Default::default()
            },
        ))
        .id();
    // Drawn parts: tiled hull and wings, engines, thrusters and their plumes, moving control
    // surfaces, gear, heat glow.
    crate::models::aircraft::spawn_spaceplane_parts(commands, body, &aircraft, &wheels, &thrusters);
    commands.entity(body).insert((
        Spacecraft {
            thrusters,
            assist: FlightAssist::default(),
            dry_mass_kg: dry,
            propellant_kg: propellant,
            ..Default::default()
        },
        wheels,
        aircraft,
    ));
    body
}
