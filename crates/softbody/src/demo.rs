//! The soft-body yard: every feature of this module, running, in one place.
//!
//! [`spawn_demo`] builds it around a center point (the showcase calls it at the yard zone):
//!
//! - **Flags** on tall poles with printed patterns, streaming in the world wind, and a field
//!   of 56 pennants (cheap enough to have many).
//! - **Curtains**: one shot full of holes by a turret, one set on fire that burns away and
//!   comes back.
//! - **Rope bridges**: a long one that sags under a walker crossing it, and a short one whose
//!   ropes are cut so it drops, then rebuilt.
//! - **Ropes**: a bead curtain a ball pushes through, a swing, and the cable of a wrecking
//!   ball.
//! - **Soft bodies**: jelly cubes on crates that the wrecking ball knocks off, and a clay
//!   block a press dents for good (plasticity).
//! - **Characters**: a mannequin with a cape and long hair walking in a circle, the same
//!   mannequin in slow motion (its own `LocalTimeScale`), and a dog with fur.
//!
//! Until the physics and animation modules land, the moving props are kinematic and the
//! skeletons are animated procedurally here (the same `SkeletonPose` contract the animation
//! module will write).

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::animation::{BonePose, SkeletonPose};
use engine_world::physics::{Collider, ColliderShape, RigidBody};
use engine_world::{IgniteRequest, ProjectileTrace};

use crate::body::SoftBodySim;
use crate::character::{BoneCapsule, BoneColliders};
use crate::cloth::{Cloth, ClothPins};
use crate::hair::{HairEmitter, HairGroom, HairStyle};
use crate::rope::{Rope, RopeBridge, RopeEnd, SoftLoad};
use crate::settings::{Pattern, SimSettings, SoftMaterial};
use crate::volume::{SoftVolume, VolumeShape};

/// How a demo prop moves (kinematic until the physics module lands).
#[derive(Component, Debug, Clone, PartialEq)]
pub enum DemoMotion {
    /// Swing under `pivot` along the horizontal direction `swing`.
    Pendulum {
        pivot: DVec3,
        swing: Vec3,
        length: f32,
        /// Radians.
        amplitude: f32,
        period: f32,
    },
    /// Glide back and forth between two points.
    Shuttle { from: DVec3, to: DVec3, period: f32 },
    /// Stamp down by `depth` from `top`, hold, and lift.
    Press { top: DVec3, depth: f32, period: f32 },
    /// Walk around a circle, facing the way it goes.
    Circle {
        center: DVec3,
        radius: f32,
        /// Seconds per lap.
        period: f32,
    },
}

/// Seconds this prop has run, in its own time (`LocalTimeScale`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct DemoClock(pub f32);

/// Which procedural rig a demo skeleton uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rig {
    Humanoid,
    Dog,
}

/// Animates this entity's `SkeletonPose` with a walk (or trot) cycle of `cycle` seconds.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoSkeleton {
    pub rig: Rig,
    pub cycle: f32,
}

/// A body part mesh placed on the bones of `owner` every step.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoLimb {
    pub owner: Entity,
    pub from: usize,
    pub to: Option<usize>,
    /// Offset in the `from` bone's space.
    pub offset: Vec3,
}

/// Walks back and forth across a rope bridge, standing on its deck.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoBridgeWalker {
    pub bridge: Entity,
    /// Along the bridge's local X, meters.
    pub from: f32,
    pub to: f32,
    pub period: f32,
}

/// Fires a bullet through a random point of a rectangle every `period` seconds.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoTurret {
    pub target: DVec3,
    /// Half extents of the target rectangle (x, y), meters.
    pub spread: Vec2,
    pub period: f32,
    pub shots: u32,
    pub next: f32,
}

/// Lights the cloth at `at` after `delay` seconds and rebuilds it every `period`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoBurner {
    pub cloth: Entity,
    pub at: DVec3,
    pub delay: f32,
    pub period: f32,
}

/// Cuts a rope bridge at `at` after `delay` seconds and rebuilds it every `period`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoCutter {
    pub bridge: Entity,
    pub at: DVec3,
    pub delay: f32,
    pub period: f32,
}

/// Rebuilds a soft volume (jelly knocked off its crate, dented clay) every `period` seconds.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoRespawn {
    pub period: f32,
}

/// Adds the demo's animation systems (they only touch entities with demo components).
pub fn register(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        (
            drive_motion,
            animate_skeletons,
            place_limbs,
            walk_bridges,
            fire_turrets,
            burn_and_restore,
            cut_and_restore,
            respawn_volumes,
        )
            .chain()
            .in_set(SimSet::Gameplay),
    );
}

// ---------------------------------------------------------------------------------------
// Spawning

fn mat(hex: u32, roughness: f32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(roughness)
}

fn cuboid(size: Vec3) -> Shape {
    Shape::Cuboid {
        size: size.to_array(),
    }
}

fn at(p: DVec3) -> Transform {
    Transform::from_translation(p)
}

/// A static mesh that soft bodies collide with.
fn block(commands: &mut Commands, size: Vec3, material: StandardMaterial, p: DVec3) -> Entity {
    commands
        .spawn((
            at(p),
            MeshRenderer::new(cuboid(size), material),
            RigidBody::fixed(),
            Collider::cuboid(size * 0.5),
        ))
        .id()
}

/// A static mesh soft bodies pass through (decoration).
fn prop(commands: &mut Commands, shape: Shape, material: StandardMaterial, t: Transform) {
    commands.spawn((t, MeshRenderer::new(shape, material)));
}

fn post(
    commands: &mut Commands,
    base: DVec3,
    height: f32,
    radius: f32,
    material: StandardMaterial,
) {
    commands.spawn((
        at(base + DVec3::Y * (height * 0.5) as f64),
        MeshRenderer::new(
            Shape::Cylinder {
                radius,
                height,
                segments: 16,
            },
            material,
        ),
        RigidBody::fixed(),
        Collider {
            shape: ColliderShape::Cylinder {
                radius,
                half_height: height * 0.5,
            },
            ..Collider::default()
        },
    ));
}

/// Build the whole yard around `origin` (ground level at `origin.y`).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let c = origin;
    flags(commands, c);
    flag_field(commands, c + DVec3::new(19.0, 0.0, -16.0));
    curtains(commands, c + DVec3::new(-12.0, 0.0, 6.0));
    bridges(commands, c);
    bead_curtain(commands, c + DVec3::new(-3.0, 0.0, 9.0));
    swing(commands, c + DVec3::new(15.0, 0.0, 4.0));
    jelly_and_wrecking_ball(commands, c + DVec3::new(0.5, 0.0, -1.0));
    clay_press(commands, c + DVec3::new(6.5, 0.0, 2.5));
    mannequin(
        commands,
        c + DVec3::new(9.0, 0.0, -7.0),
        2.6,
        1.0,
        "Mannequin",
    );
    mannequin(
        commands,
        c + DVec3::new(15.5, 0.0, -6.0),
        2.0,
        0.25,
        "Slow-motion mannequin",
    );
    dog(commands, c + DVec3::new(-8.0, 0.0, -4.0));
}

fn flags(commands: &mut Commands, c: DVec3) {
    let metal = mat(0xb8bcc4, 0.35).with_metallic(1.0);
    // Made-up banners (no real country's flag), one per printed pattern.
    let designs = [
        (
            "Cross banner",
            SoftMaterial::hex(0x0e6655).with_pattern(Pattern::Cross, 0xf5b041, 0xfdfefe),
        ),
        (
            "Striped banner",
            SoftMaterial::hex(0x6c3483).with_pattern(
                Pattern::VerticalStripes(3),
                0xf8f9f9,
                0x17a589,
            ),
        ),
        (
            "Sun banner",
            SoftMaterial::hex(0x1b2631).with_pattern(Pattern::Circle, 0xf4d03f, 0xf4d03f),
        ),
        (
            "Sash banner",
            SoftMaterial::hex(0xa93226).with_pattern(Pattern::Diagonal, 0xfdfefe, 0xfdfefe),
        ),
        (
            "Checkered flag",
            SoftMaterial::hex(0x111111).with_pattern(Pattern::Checker(6), 0xf2f2f2, 0xf2f2f2),
        ),
    ];

    for (i, (name, material)) in designs.into_iter().enumerate() {
        let base = c + DVec3::new(-16.0 + i as f64 * 8.0, 0.0, -18.0);
        post(commands, base, 9.0, 0.07, metal);
        prop(
            commands,
            Shape::Sphere {
                radius: 0.12,
                segments: 16,
            },
            mat(0xd4ac0d, 0.3).with_metallic(1.0),
            at(base + DVec3::Y * 9.05),
        );
        let mut spec = Cloth::flag([3.0, 2.0], material.with_sheen(0.25));
        spec.sim.wind_scale = 2.2;
        commands.spawn((Name::new(name), at(base + DVec3::new(0.08, 8.8, 0.0)), spec));
    }
}

fn flag_field(commands: &mut Commands, corner: DVec3) {
    let colors = [
        0xe74c3c, 0xf39c12, 0xf1c40f, 0x2ecc71, 0x3498db, 0x9b59b6, 0xecf0f1,
    ];
    let wood = mat(0x8b6b4a, 0.8);
    for row in 0..8 {
        for col in 0..7 {
            let base = corner + DVec3::new(col as f64 * 1.7, 0.0, row as f64 * 1.7);
            post(commands, base, 2.4, 0.025, wood);
            let k = (row * 7 + col) % colors.len();
            let material = SoftMaterial::hex(colors[k])
                .with_pattern(
                    Pattern::HorizontalStripes(2),
                    colors[(k + 3) % colors.len()],
                    0,
                )
                .with_sheen(0.2);
            let mut spec = Cloth::flag([1.0, 0.6], material);
            spec.resolution = [10, 6];
            spec.sim.wind_scale = 2.4;
            spec.sim.substeps = 8;
            commands.spawn((
                Name::new(format!("Pennant {}", row * 7 + col + 1)),
                at(base + DVec3::new(0.03, 2.35, 0.0)),
                spec,
            ));
        }
    }
}

fn curtains(commands: &mut Commands, c: DVec3) {
    let wood = mat(0x6e4b2f, 0.75);
    // A frame with two openings.
    for x in [-5.2, 0.0, 5.2] {
        block(
            commands,
            Vec3::new(0.3, 6.2, 0.3),
            wood,
            c + DVec3::new(x, 3.1, 0.0),
        );
    }
    block(
        commands,
        Vec3::new(10.8, 0.3, 0.3),
        wood,
        c + DVec3::new(0.0, 6.35, 0.0),
    );
    let rail = mat(0xb08d57, 0.3).with_metallic(1.0);
    prop(
        commands,
        Shape::Cylinder {
            radius: 0.03,
            height: 10.2,
            segments: 12,
        },
        rail,
        Transform::from_translation(c + DVec3::new(0.0, 6.05, 0.25))
            .with_rotation(Quat::from_rotation_z(PI * 0.5)),
    );

    // Left: velvet with a gold border, shot full of holes.
    let velvet = SoftMaterial::hex(0x7b1e2b)
        .with_pattern(Pattern::Border, 0xc9a227, 0xe8cc6a)
        .with_roughness(0.9)
        .with_sheen(0.8);
    let left = c + DVec3::new(-2.6, 6.0, 0.25);
    commands.spawn((
        Name::new("Shot curtain"),
        at(left),
        Cloth::curtain([4.6, 5.6], velvet),
    ));
    // The turret stands in front and shoots through it.
    let turret = c + DVec3::new(-2.6, 1.2, 7.0);
    block(
        commands,
        Vec3::new(0.5, 1.0, 0.5),
        mat(0x2c3e50, 0.5),
        turret - DVec3::Y * 0.7,
    );
    prop(
        commands,
        Shape::Cylinder {
            radius: 0.06,
            height: 0.8,
            segments: 12,
        },
        mat(0x17202a, 0.3).with_metallic(1.0),
        Transform::from_translation(turret + DVec3::new(0.0, 0.0, -0.3))
            .with_rotation(Quat::from_rotation_x(PI * 0.5)),
    );
    commands.spawn((
        Name::new("Curtain turret"),
        at(turret),
        DemoTurret {
            target: c + DVec3::new(-2.6, 3.0, -1.0),
            spread: Vec2::new(1.8, 1.8),
            period: 0.7,
            shots: 0,
            next: 1.5,
        },
        DemoClock::default(),
    ));

    // Right: a linen curtain that is set on fire, burns away and comes back.
    let linen = SoftMaterial::hex(0xe8dcc4)
        .with_pattern(Pattern::HorizontalStripes(9), 0xd8c8a8, 0xe8dcc4)
        .with_roughness(0.85)
        .with_sheen(0.3)
        .with_subsurface(0.5);
    let right = c + DVec3::new(2.6, 6.0, 0.25);
    let burning = commands
        .spawn((
            Name::new("Burning curtain"),
            at(right),
            Cloth::curtain([4.6, 5.6], linen),
        ))
        .id();
    commands.spawn((
        Name::new("Curtain igniter"),
        at(right),
        DemoBurner {
            cloth: burning,
            at: right + DVec3::new(0.3, -5.4, 0.0),
            delay: 2.0,
            period: 24.0,
        },
        DemoClock::default(),
    ));
}

fn bridges(commands: &mut Commands, c: DVec3) {
    let rock = mat(0x7d7468, 0.9);
    let stone = mat(0x948b7e, 0.85);
    // The long bridge between two rock platforms.
    let (a, b) = (
        c + DVec3::new(2.0, 0.0, 10.0),
        c + DVec3::new(24.0, 0.0, 10.0),
    );
    for p in [a, b] {
        block(commands, Vec3::splat(3.0), rock, p + DVec3::Y * 1.5);
    }
    let bridge = commands
        .spawn((
            Name::new("Rope bridge"),
            at(a + DVec3::new(1.5, 3.0, 0.0)),
            RopeBridge {
                length: 19.0,
                planks: 22,
                sag: 1.3,
                ..RopeBridge::default()
            },
        ))
        .id();
    let walker = commands
        .spawn((
            Name::new("Bridge walker"),
            at(a + DVec3::new(2.5, 3.0, 0.0)),
            TransformInterpolation::default(),
            SoftLoad { mass_kg: 90.0 },
            DemoBridgeWalker {
                bridge,
                from: 1.0,
                to: 18.0,
                period: 22.0,
            },
            DemoClock::default(),
        ))
        .id();
    let coat = mat(0xd35400, 0.7);
    commands.spawn((
        Transform::from_xyz(0.0, 0.85, 0.0),
        MeshRenderer::new(
            Shape::Capsule {
                radius: 0.25,
                height: 1.5,
                segments: 16,
            },
            coat,
        ),
        ChildOf(walker),
    ));
    commands.spawn((
        Transform::from_xyz(0.0, 1.78, 0.0),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 0.14,
                segments: 16,
            },
            mat(0xe0b48a, 0.6),
        ),
        ChildOf(walker),
    ));

    // A short bridge between two pillars that gets cut and drops.
    let (a, b) = (
        c + DVec3::new(-7.0, 0.0, -11.0),
        c + DVec3::new(3.0, 0.0, -11.0),
    );
    for p in [a, b] {
        block(
            commands,
            Vec3::new(1.6, 4.0, 1.6),
            stone,
            p + DVec3::Y * 2.0,
        );
    }
    let short = commands
        .spawn((
            Name::new("Cut bridge"),
            at(a + DVec3::new(0.8, 4.0, 0.0)),
            RopeBridge {
                length: 8.4,
                planks: 10,
                sag: 0.6,
                width: 1.2,
                ..RopeBridge::default()
            },
        ))
        .id();
    commands.spawn((
        Name::new("Bridge cutter"),
        at(b),
        DemoCutter {
            bridge: short,
            at: b + DVec3::new(-0.8, 4.3, 0.0),
            delay: 5.0,
            period: 14.0,
        },
        DemoClock::default(),
    ));
}

fn bead_curtain(commands: &mut Commands, c: DVec3) {
    let wood = mat(0x5d4037, 0.8);
    for x in [-1.8, 1.8] {
        block(
            commands,
            Vec3::new(0.2, 2.8, 0.2),
            wood,
            c + DVec3::new(x, 1.4, 0.0),
        );
    }
    block(
        commands,
        Vec3::new(3.8, 0.2, 0.2),
        wood,
        c + DVec3::new(0.0, 2.9, 0.0),
    );
    let colors = [0xc0392b, 0xe67e22, 0xf1c40f, 0x27ae60, 0x2980b9, 0x8e44ad];
    for i in 0..16 {
        let x = -1.5 + i as f32 * 0.2;
        let color = colors[i % colors.len()];
        commands.spawn((
            Name::new(format!("Bead string {}", i + 1)),
            at(c + DVec3::new(x as f64, 2.78, 0.0)),
            Rope {
                end_a: RopeEnd::Here { offset: [0.0; 3] },
                end_b: RopeEnd::Free,
                length: 2.5,
                segments: 18,
                radius: 0.018,
                mass_per_m: 0.25,
                material: SoftMaterial::hex(color).with_roughness(0.3).with_sheen(0.0),
                ..Rope::default()
            },
        ));
    }
    // A ball rolls through the strings and back.
    commands.spawn((
        Name::new("Bead curtain ball"),
        at(c + DVec3::new(0.0, 0.6, 3.0)),
        TransformInterpolation::default(),
        RigidBody::kinematic(),
        Collider::sphere(0.6),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 0.6,
                segments: 32,
            },
            mat(0x1abc9c, 0.25),
        ),
        DemoMotion::Shuttle {
            from: c + DVec3::new(-0.3, 0.6, 3.0),
            to: c + DVec3::new(0.3, 0.6, -3.0),
            period: 9.0,
        },
        DemoClock::default(),
    ));
}

fn swing(commands: &mut Commands, c: DVec3) {
    let wood = mat(0x795548, 0.8);
    for x in [-1.3, 1.3] {
        block(
            commands,
            Vec3::new(0.2, 4.2, 0.2),
            wood,
            c + DVec3::new(x, 2.1, 0.0),
        );
    }
    block(
        commands,
        Vec3::new(2.8, 0.2, 0.2),
        wood,
        c + DVec3::new(0.0, 4.2, 0.0),
    );
    let pivot = c + DVec3::new(0.0, 4.1, 0.0);
    let seat = commands
        .spawn((
            Name::new("Swing seat"),
            at(pivot - DVec3::Y * 2.6),
            TransformInterpolation::default(),
            RigidBody::kinematic(),
            Collider::cuboid(Vec3::new(0.35, 0.03, 0.18)),
            MeshRenderer::new(cuboid(Vec3::new(0.7, 0.06, 0.36)), mat(0xa1887f, 0.7)),
            DemoMotion::Pendulum {
                pivot,
                swing: Vec3::Z,
                length: 2.6,
                amplitude: 0.7,
                period: 3.3,
            },
            DemoClock::default(),
        ))
        .id();
    for side in [-1.0f32, 1.0] {
        let x = side * 0.3;
        commands.spawn((
            Name::new("Swing rope"),
            at(pivot + DVec3::new(x as f64, 0.0, 0.0)),
            Rope {
                end_a: RopeEnd::Here { offset: [0.0; 3] },
                end_b: RopeEnd::Entity {
                    entity: seat,
                    offset: [x, 0.03, 0.0],
                },
                length: 2.62,
                segments: 14,
                radius: 0.016,
                cuttable: false,
                ..Rope::default()
            },
        ));
    }
}

fn jelly_and_wrecking_ball(commands: &mut Commands, c: DVec3) {
    let crate_mat = mat(0x9c6b30, 0.85);
    let jellies = [
        ("Green jelly", 0x2ecc71),
        ("Raspberry jelly", 0xe0245e),
        ("Orange jelly", 0xf39c12),
    ];
    for (i, (name, color)) in jellies.into_iter().enumerate() {
        let x = (i as f64 - 1.0) * 2.4;
        block(
            commands,
            Vec3::splat(1.2),
            crate_mat,
            c + DVec3::new(x, 0.6, 0.0),
        );
        let material = SoftMaterial::hex(color)
            .with_roughness(0.08)
            .with_subsurface(0.9)
            .with_sheen(0.0)
            .with_wrinkles(0.0)
            .with_alpha(0.8);
        commands.spawn((
            Name::new(name),
            at(c + DVec3::new(x, 1.66, 0.0)),
            SoftVolume::jelly(0.85, material),
            DemoRespawn { period: 16.0 },
            DemoClock::default(),
        ));
    }
    // Gantry and wrecking ball swinging along the row.
    let steel = mat(0x566573, 0.4).with_metallic(1.0);
    for x in [-5.0, 5.0] {
        block(
            commands,
            Vec3::new(0.3, 7.0, 0.3),
            steel,
            c + DVec3::new(x, 3.5, -1.2),
        );
    }
    block(
        commands,
        Vec3::new(10.3, 0.3, 0.3),
        steel,
        c + DVec3::new(0.0, 7.0, -1.2),
    );
    let pivot = c + DVec3::new(0.0, 6.85, -1.2);
    let length = 4.9;
    let ball = commands
        .spawn((
            Name::new("Wrecking ball"),
            at(pivot - DVec3::Y * length as f64),
            TransformInterpolation::default(),
            RigidBody::kinematic(),
            Collider::sphere(0.45),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.45,
                    segments: 32,
                },
                mat(0x2d3436, 0.35).with_metallic(1.0),
            ),
            DemoMotion::Pendulum {
                pivot,
                swing: Vec3::new(1.0, 0.0, 0.25).normalize(),
                length,
                amplitude: 0.85,
                period: 4.6,
            },
            DemoClock::default(),
        ))
        .id();
    commands.spawn((
        Name::new("Wrecking ball cable"),
        at(pivot),
        Rope {
            end_a: RopeEnd::Here { offset: [0.0; 3] },
            end_b: RopeEnd::Entity {
                entity: ball,
                offset: [0.0, 0.45, 0.0],
            },
            length: length - 0.42,
            segments: 16,
            radius: 0.02,
            cuttable: false,
            material: SoftMaterial::hex(0x34495e)
                .with_pattern(Pattern::Rope, 0x34495e, 0x34495e)
                .with_roughness(0.4)
                .with_metallic(0.8),
            ..Rope::default()
        },
    ));
}

fn clay_press(commands: &mut Commands, c: DVec3) {
    let steel = mat(0x7f8c8d, 0.4).with_metallic(1.0);
    block(
        commands,
        Vec3::new(1.4, 1.0, 1.4),
        steel,
        c + DVec3::Y * 0.5,
    );
    for x in [-0.9, 0.9] {
        block(
            commands,
            Vec3::new(0.15, 4.0, 0.15),
            steel,
            c + DVec3::new(x, 2.0, 0.0),
        );
    }
    block(
        commands,
        Vec3::new(2.0, 0.25, 0.4),
        steel,
        c + DVec3::new(0.0, 4.1, 0.0),
    );
    let clay = SoftVolume {
        shape: VolumeShape::Box {
            size: [0.8, 0.6, 0.8],
            bevel: 0.08,
        },
        resolution: 6,
        mass_kg: 12.0,
        shape_compliance: 2e-3,
        lattice_compliance: 2e-3,
        plasticity: 4.0,
        yield_m: 0.01,
        material: SoftMaterial::hex(0xc8744a)
            .with_roughness(0.95)
            .with_sheen(0.0)
            .with_subsurface(0.2)
            .with_wrinkles(0.0),
        sim: SimSettings {
            substeps: 12,
            damping: 3.0,
            friction: 0.9,
            thickness: 0.03,
            wind_scale: 0.0,
            ..SimSettings::default()
        },
        ..SoftVolume::default()
    };
    commands.spawn((
        Name::new("Clay block"),
        at(c + DVec3::Y * 1.33),
        clay,
        DemoRespawn { period: 18.0 },
        DemoClock::default(),
    ));
    let top = c + DVec3::Y * 3.0;
    commands.spawn((
        Name::new("Press"),
        at(top),
        TransformInterpolation::default(),
        RigidBody::kinematic(),
        Collider::cuboid(Vec3::new(0.25, 0.2, 0.25)),
        MeshRenderer::new(cuboid(Vec3::new(0.5, 0.4, 0.5)), mat(0xc0392b, 0.5)),
        DemoMotion::Press {
            top,
            depth: 1.53,
            period: 4.5,
        },
        DemoClock::default(),
    ));
}

// ---------------------------------------------------------------------------------------
// Characters

const HUMANOID: [(&str, Option<u16>); 17] = [
    ("pelvis", None),
    ("spine", Some(0)),
    ("chest", Some(1)),
    ("neck", Some(2)),
    ("head", Some(3)),
    ("upper_arm_l", Some(2)),
    ("forearm_l", Some(5)),
    ("hand_l", Some(6)),
    ("upper_arm_r", Some(2)),
    ("forearm_r", Some(8)),
    ("hand_r", Some(9)),
    ("thigh_l", Some(0)),
    ("shin_l", Some(11)),
    ("foot_l", Some(12)),
    ("thigh_r", Some(0)),
    ("shin_r", Some(14)),
    ("foot_r", Some(15)),
];

const DOG: [(&str, Option<u16>); 20] = [
    ("hips", None),
    ("spine", Some(0)),
    ("chest", Some(1)),
    ("neck", Some(2)),
    ("head", Some(3)),
    ("tail_1", Some(0)),
    ("tail_2", Some(5)),
    ("tail_3", Some(6)),
    ("leg_fl", Some(2)),
    ("knee_fl", Some(8)),
    ("paw_fl", Some(9)),
    ("leg_fr", Some(2)),
    ("knee_fr", Some(11)),
    ("paw_fr", Some(12)),
    ("leg_hl", Some(0)),
    ("knee_hl", Some(14)),
    ("paw_hl", Some(15)),
    ("leg_hr", Some(0)),
    ("knee_hr", Some(17)),
    ("paw_hr", Some(18)),
];

fn skeleton(bones: &[(&str, Option<u16>)], rig: Rig) -> SkeletonPose {
    let mut pose = SkeletonPose {
        names: bones
            .iter()
            .map(|(n, _)| n.to_string())
            .collect::<Vec<_>>()
            .into(),
        parents: Arc::from(bones.iter().map(|(_, p)| *p).collect::<Vec<_>>()),
        bones: vec![BonePose::default(); bones.len()],
    };
    pose_rig(rig, 0.0, &mut pose);
    pose
}

/// Direction from angle `a` (radians, toward forward = −Z) hanging down.
fn hang(a: f32) -> Vec3 {
    Vec3::new(0.0, -a.cos(), -a.sin())
}

fn aim(from: Vec3, to: Vec3) -> Quat {
    let d = to - from;
    if d.length_squared() < 1e-10 {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_arc(Vec3::NEG_Y, d.normalize())
    }
}

/// Write a walk (humanoid) or trot (dog) pose at `phase` radians into `pose`.
pub fn pose_rig(rig: Rig, phase: f32, pose: &mut SkeletonPose) {
    let p = &mut pose.bones;
    let set = |p: &mut Vec<BonePose>, i: usize, t: Vec3, r: Quat| {
        p[i].translation = t;
        p[i].rotation = r;
        p[i].scale = Vec3::ONE;
    };
    match rig {
        Rig::Humanoid => {
            let bob = 0.025 * (2.0 * phase).cos();
            let twist = 0.07 * phase.sin();
            let pelvis = Vec3::new(0.0, 0.95 + bob, 0.0);
            let chest = Vec3::new(0.0, 1.30 + bob, -0.02);
            set(p, 0, pelvis, Quat::from_rotation_y(-twist));
            set(p, 1, Vec3::new(0.0, 1.12 + bob, -0.01), Quat::IDENTITY);
            set(p, 2, chest, Quat::from_rotation_y(twist));
            set(p, 3, Vec3::new(0.0, 1.50 + bob, -0.03), Quat::IDENTITY);
            set(p, 4, Vec3::new(0.0, 1.60 + bob, -0.04), Quat::IDENTITY);
            // Facing −Z, the left side is −X.
            for (side, base_arm, base_leg) in [(-1.0f32, 5, 11), (1.0, 8, 14)] {
                let ph = if side > 0.0 { phase } else { phase + PI };
                // Arm swings opposite to the leg on the same side.
                let swing = -0.35 * ph.sin();
                let shoulder = chest + Vec3::new(0.2 * side, 0.15, 0.0);
                let elbow = shoulder + hang(swing) * 0.28 + Vec3::X * (0.02 * side);
                let hand = elbow + hang(swing + 0.35) * 0.26;
                set(p, base_arm, shoulder, aim(shoulder, elbow));
                set(p, base_arm + 1, elbow, aim(elbow, hand));
                set(p, base_arm + 2, hand, aim(elbow, hand));
                // Leg: thigh swings, the knee bends through the swing phase.
                let thigh = 0.4 * ph.sin();
                let knee_bend = 0.1 + 0.9 * ph.cos().max(0.0).powi(2);
                let hip = pelvis + Vec3::new(0.1 * side, -0.05, 0.0);
                let knee = hip + hang(thigh) * 0.43;
                let ankle = knee + hang(thigh - knee_bend) * 0.43;
                set(p, base_leg, hip, aim(hip, knee));
                set(p, base_leg + 1, knee, aim(knee, ankle));
                set(p, base_leg + 2, ankle, Quat::IDENTITY);
            }
        }
        Rig::Dog => {
            let bob = 0.015 * (2.0 * phase).cos();
            let wag = (3.0 * phase).sin();
            let hips = Vec3::new(0.0, 0.55 + bob, 0.28);
            let chest = Vec3::new(0.0, 0.58 + bob, -0.24);
            set(p, 0, hips, Quat::IDENTITY);
            set(p, 1, Vec3::new(0.0, 0.57 + bob, 0.02), Quat::IDENTITY);
            set(p, 2, chest, Quat::IDENTITY);
            set(p, 3, Vec3::new(0.0, 0.68 + bob, -0.38), Quat::IDENTITY);
            let head = Vec3::new(0.0, 0.78 + bob, -0.5);
            set(p, 4, head, Quat::from_rotation_y(0.15 * phase.sin()));
            let t1 = Vec3::new(0.0, 0.6 + bob, 0.4);
            let t2 = t1 + Vec3::new(0.08 * wag, 0.07, 0.18);
            let t3 = t2 + Vec3::new(0.16 * wag, 0.04, 0.17);
            set(p, 5, t1, aim(t1, t2));
            set(p, 6, t2, aim(t2, t3));
            set(p, 7, t3, aim(t2, t3));
            // Trot: diagonal legs move together.
            for (base, side, front, offset) in [
                (8, -1.0f32, true, 0.0),
                (11, 1.0, true, PI),
                (14, -1.0, false, PI),
                (17, 1.0, false, 0.0),
            ] {
                let ph = phase + offset;
                let swing = 0.35 * ph.sin();
                let bend = 0.15 + 0.6 * ph.cos().max(0.0).powi(2);
                let top = if front { chest } else { hips } + Vec3::new(0.11 * side, -0.05, 0.0);
                let knee = top + hang(swing) * 0.24;
                let lower = if front {
                    swing - bend
                } else {
                    swing + bend * 0.6
                };
                let paw = knee + hang(lower) * 0.27;
                set(p, base, top, aim(top, knee));
                set(p, base + 1, knee, aim(knee, paw));
                set(p, base + 2, paw, Quat::IDENTITY);
            }
        }
    }
}

/// A body-part mesh on bones `from` (→ `to`) of `owner`.
#[allow(clippy::too_many_arguments)]
fn limb(
    commands: &mut Commands,
    owner: Entity,
    pose: &SkeletonPose,
    from: &str,
    to: Option<&str>,
    radius: f32,
    offset: Vec3,
    material: StandardMaterial,
) {
    let a = pose.index_of(from).expect("bone");
    let b = to.map(|t| pose.index_of(t).expect("bone"));
    let shape = match b {
        Some(b) => Shape::Capsule {
            radius,
            height: pose.bones[a]
                .translation
                .distance(pose.bones[b].translation)
                + 2.0 * radius,
            segments: 14,
        },
        None => Shape::Sphere {
            radius,
            segments: 20,
        },
    };
    commands.spawn((
        Transform::default(),
        TransformInterpolation::default(),
        MeshRenderer::new(shape, material),
        DemoLimb {
            owner,
            from: a,
            to: b,
            offset,
        },
    ));
}

/// A mannequin with a cape and long hair walking around `center`. `time_scale` < 1 runs it
/// (and its cloth and hair) in slow motion.
fn mannequin(
    commands: &mut Commands,
    center: DVec3,
    radius: f32,
    time_scale: f32,
    name: &str,
) -> Entity {
    let pose = skeleton(&HUMANOID, Rig::Humanoid);
    let mut root = commands.spawn((
        Name::new(name.to_string()),
        Transform::from_translation(center + DVec3::X * radius as f64),
        TransformInterpolation::default(),
        pose.clone(),
        BoneColliders::humanoid(),
        DemoSkeleton {
            rig: Rig::Humanoid,
            cycle: 1.05,
        },
        DemoMotion::Circle {
            center,
            radius,
            period: TAU * radius / 1.25,
        },
        DemoClock::default(),
    ));
    if time_scale != 1.0 {
        root.insert(LocalTimeScale {
            factor: time_scale,
            ignore_world_dilation: false,
        });
    }
    let man = root.id();
    let skin = mat(0xd9c3a0, 0.55);
    let cloth = mat(0x34495e, 0.8);
    limb(
        commands,
        man,
        &pose,
        "pelvis",
        Some("chest"),
        0.15,
        Vec3::ZERO,
        cloth,
    );
    limb(
        commands,
        man,
        &pose,
        "chest",
        Some("neck"),
        0.155,
        Vec3::ZERO,
        cloth,
    );
    limb(
        commands,
        man,
        &pose,
        "neck",
        Some("head"),
        0.05,
        Vec3::ZERO,
        skin,
    );
    limb(
        commands,
        man,
        &pose,
        "head",
        None,
        0.1,
        Vec3::new(0.0, 0.07, 0.0),
        skin,
    );
    for s in ["l", "r"] {
        let b = |n: &str| format!("{n}_{s}");
        limb(
            commands,
            man,
            &pose,
            &b("upper_arm"),
            Some(&b("forearm")),
            0.05,
            Vec3::ZERO,
            cloth,
        );
        limb(
            commands,
            man,
            &pose,
            &b("forearm"),
            Some(&b("hand")),
            0.042,
            Vec3::ZERO,
            skin,
        );
        limb(
            commands,
            man,
            &pose,
            &b("hand"),
            None,
            0.05,
            Vec3::new(0.0, -0.04, 0.0),
            skin,
        );
        limb(
            commands,
            man,
            &pose,
            &b("thigh"),
            Some(&b("shin")),
            0.075,
            Vec3::ZERO,
            cloth,
        );
        limb(
            commands,
            man,
            &pose,
            &b("shin"),
            Some(&b("foot")),
            0.055,
            Vec3::ZERO,
            cloth,
        );
        limb(
            commands,
            man,
            &pose,
            &b("foot"),
            None,
            0.06,
            Vec3::new(0.0, -0.02, -0.06),
            mat(0x1c1c1c, 0.6),
        );
    }
    // Cape tied across the chest, hanging behind the back.
    let cape = SoftMaterial::hex(0x922b3e)
        .with_pattern(Pattern::Border, 0xd4ac0d, 0xf4d03f)
        .with_roughness(0.85)
        .with_sheen(0.7);
    commands.spawn((
        Name::new(format!("{name} cape")),
        Transform::from_xyz(0.0, 1.46, 0.21),
        Cloth {
            pins: ClothPins::Bones {
                rows: 1,
                bones: vec!["chest".into()],
            },
            ..Cloth::cape([0.72, 1.2], cape, &["chest"])
        },
        ChildOf(man),
    ));
    let style = HairStyle {
        melanin: 0.8,
        redness: 0.2,
        ..HairStyle::long()
    };
    commands.spawn((
        Name::new(format!("{name} hair")),
        Transform::default(),
        HairGroom {
            emitters: vec![HairEmitter::Scalp {
                bone: "head".into(),
                // The head mesh: a 0.1 m ball 7 cm above the head bone.
                center: [0.0, 0.07, 0.0],
                radii: [0.101, 0.106, 0.106],
                guides: 300,
                hairline: 0.55,
            }],
            ..HairGroom::long_hair("head", style)
        },
        ChildOf(man),
    ));
    man
}

fn dog(commands: &mut Commands, center: DVec3) {
    let pose = skeleton(&DOG, Rig::Dog);
    let capsules = vec![
        BoneCapsule::capsule("hips", "chest", 0.15),
        BoneCapsule::capsule("neck", "head", 0.07),
        BoneCapsule::sphere("head", 0.1, [0.0, 0.02, -0.04]),
        BoneCapsule::capsule("tail_1", "tail_3", 0.03),
        BoneCapsule::capsule("leg_fl", "paw_fl", 0.04),
        BoneCapsule::capsule("leg_fr", "paw_fr", 0.04),
        BoneCapsule::capsule("leg_hl", "paw_hl", 0.045),
        BoneCapsule::capsule("leg_hr", "paw_hr", 0.045),
    ];
    let radius = 2.0;
    let dog = commands
        .spawn((
            Name::new("Dog"),
            Transform::from_translation(center + DVec3::X * radius as f64),
            TransformInterpolation::default(),
            pose.clone(),
            BoneColliders {
                capsules,
                friction: 0.3,
            },
            DemoSkeleton {
                rig: Rig::Dog,
                cycle: 0.55,
            },
            DemoMotion::Circle {
                center,
                radius,
                period: TAU * radius / 1.6,
            },
            DemoClock::default(),
        ))
        .id();
    let coat = mat(0xb5793c, 0.8);
    let dark = mat(0x3b2a1e, 0.6);
    limb(
        commands,
        dog,
        &pose,
        "hips",
        Some("chest"),
        0.14,
        Vec3::ZERO,
        coat,
    );
    limb(
        commands,
        dog,
        &pose,
        "neck",
        Some("head"),
        0.065,
        Vec3::ZERO,
        coat,
    );
    limb(
        commands,
        dog,
        &pose,
        "head",
        None,
        0.095,
        Vec3::new(0.0, 0.02, -0.03),
        coat,
    );
    limb(
        commands,
        dog,
        &pose,
        "head",
        None,
        0.05,
        Vec3::new(0.0, -0.02, -0.13),
        coat,
    );
    limb(
        commands,
        dog,
        &pose,
        "head",
        None,
        0.022,
        Vec3::new(0.0, 0.0, -0.18),
        dark,
    );
    for side in [-1.0f32, 1.0] {
        limb(
            commands,
            dog,
            &pose,
            "head",
            None,
            0.035,
            Vec3::new(0.07 * side, 0.08, 0.01),
            coat,
        );
    }
    limb(
        commands,
        dog,
        &pose,
        "tail_1",
        Some("tail_2"),
        0.03,
        Vec3::ZERO,
        coat,
    );
    limb(
        commands,
        dog,
        &pose,
        "tail_2",
        Some("tail_3"),
        0.025,
        Vec3::ZERO,
        coat,
    );
    for leg in ["fl", "fr", "hl", "hr"] {
        let b = |n: &str| format!("{n}_{leg}");
        limb(
            commands,
            dog,
            &pose,
            &b("leg"),
            Some(&b("knee")),
            0.045,
            Vec3::ZERO,
            coat,
        );
        limb(
            commands,
            dog,
            &pose,
            &b("knee"),
            Some(&b("paw")),
            0.035,
            Vec3::ZERO,
            coat,
        );
        limb(
            commands,
            dog,
            &pose,
            &b("paw"),
            None,
            0.04,
            Vec3::new(0.0, 0.0, -0.02),
            dark,
        );
    }
    let fur = HairStyle {
        melanin: 0.32,
        redness: 0.75,
        ..HairStyle::fur()
    };
    let body_fur = |bone: &str, to: Option<&str>, radius: f32, guides: u32| HairEmitter::Fur {
        bone: bone.into(),
        to: to.map(Into::into),
        radius,
        guides,
        comb: [0.0, -0.3, 1.0],
    };
    commands.spawn((
        Name::new("Dog fur"),
        Transform::default(),
        HairGroom {
            emitters: vec![
                body_fur("hips", Some("chest"), 0.14, 420),
                body_fur("neck", Some("head"), 0.065, 100),
                body_fur("head", None, 0.095, 90),
                body_fur("leg_fl", Some("knee_fl"), 0.045, 18),
                body_fur("leg_fr", Some("knee_fr"), 0.045, 18),
                body_fur("leg_hl", Some("knee_hl"), 0.045, 18),
                body_fur("leg_hr", Some("knee_hr"), 0.045, 18),
            ],
            style: fur,
            sim: HairGroom::hair_settings(),
        },
        ChildOf(dog),
    ));
    commands.spawn((
        Name::new("Dog tail fur"),
        Transform::default(),
        HairGroom {
            emitters: vec![body_fur("tail_1", Some("tail_3"), 0.03, 50)],
            style: HairStyle {
                length: 0.11,
                segments: 4,
                stiffness: 0.5,
                ..fur
            },
            sim: HairGroom::hair_settings(),
        },
        ChildOf(dog),
    ));
}

// ---------------------------------------------------------------------------------------
// Systems

fn smooth_shuttle(t: f32) -> f32 {
    0.5 - 0.5 * (t * TAU).cos()
}

/// Move kinematic props and walkers.
pub fn drive_motion(
    time: Res<Time>,
    mut movers: Query<(
        &mut Transform,
        &DemoMotion,
        &mut DemoClock,
        Option<&LocalTimeScale>,
    )>,
) {
    for (mut t, motion, mut clock, scale) in &mut movers {
        clock.0 += time.delta_for(scale);
        let s = clock.0;
        match *motion {
            DemoMotion::Pendulum {
                pivot,
                swing,
                length,
                amplitude,
                period,
            } => {
                let a = amplitude * (s / period * TAU).sin();
                let dir = swing * a.sin() - Vec3::Y * a.cos();
                t.translation = pivot + (dir * length).as_dvec3();
                t.rotation = Quat::from_rotation_arc(Vec3::NEG_Y, dir);
            }
            DemoMotion::Shuttle { from, to, period } => {
                t.translation = from.lerp(to, smooth_shuttle(s / period) as f64);
            }
            DemoMotion::Press { top, depth, period } => {
                // Down fast, hold, up slowly, wait.
                let f = (s / period).fract();
                let k = if f < 0.15 {
                    smoothstep(0.0, 0.15, f)
                } else if f < 0.4 {
                    1.0
                } else if f < 0.7 {
                    1.0 - smoothstep(0.4, 0.7, f)
                } else {
                    0.0
                };
                t.translation = top - DVec3::Y * (depth * k) as f64;
            }
            DemoMotion::Circle {
                center,
                radius,
                period,
            } => {
                let a = s / period * TAU;
                t.translation =
                    center + DVec3::new(a.cos() as f64, 0.0, -a.sin() as f64) * radius as f64;
                // Facing along the way it walks (counter-clockwise seen from above).
                let forward = Vec3::new(-a.sin(), 0.0, -a.cos());
                t.rotation = Quat::from_rotation_arc(Vec3::NEG_Z, forward);
            }
        }
    }
}

/// Procedural walk and trot cycles.
pub fn animate_skeletons(mut rigs: Query<(&DemoSkeleton, &DemoClock, &mut SkeletonPose)>) {
    for (rig, clock, mut pose) in &mut rigs {
        let phase = clock.0 / rig.cycle.max(0.05) * TAU;
        pose_rig(rig.rig, phase, &mut pose);
    }
}

/// Put body-part meshes on their bones.
pub fn place_limbs(
    owners: Query<(&Transform, &SkeletonPose), Without<DemoLimb>>,
    mut limbs: Query<(&DemoLimb, &mut Transform)>,
) {
    for (limb, mut t) in &mut limbs {
        let Ok((root, pose)) = owners.get(limb.owner) else {
            continue;
        };
        let g = GlobalTransform::from(*root);
        let Some((a, ra)) = pose.bone_world(&g, limb.from) else {
            continue;
        };
        let offset = (ra * limb.offset).as_dvec3();
        match limb.to.and_then(|b| pose.bone_world(&g, b)) {
            Some((b, _)) => {
                let axis = (b - a).as_vec3();
                t.translation = (a + b) * 0.5 + offset;
                t.rotation = if axis.length_squared() > 1e-10 {
                    Quat::from_rotation_arc(Vec3::Y, axis.normalize())
                } else {
                    ra
                };
            }
            None => {
                t.translation = a + offset;
                t.rotation = ra;
            }
        }
    }
}

/// Walkers stand on the deck of their bridge (which sags under their `SoftLoad`).
pub fn walk_bridges(
    time: Res<Time>,
    bridges: Query<(&Transform, &SoftBodySim), Without<DemoBridgeWalker>>,
    mut walkers: Query<(&mut Transform, &DemoBridgeWalker, &mut DemoClock)>,
) {
    for (mut t, w, mut clock) in &mut walkers {
        clock.0 += time.delta_for(None);
        let Ok((bt, body)) = bridges.get(w.bridge) else {
            continue;
        };
        let f = smooth_shuttle(clock.0 / w.period);
        let x = w.from + (w.to - w.from) * f;
        let p = bt.translation + (bt.rotation * Vec3::new(x, 0.0, 0.0)).as_dvec3();
        let deck = body
            .bridge
            .as_ref()
            .and_then(|b| b.deck_height(p + DVec3::Y * 0.2));
        let target = match deck {
            Some(y) => DVec3::new(p.x, y, p.z),
            // Fell off (bridge cut): stay put.
            None => t.translation,
        };
        t.translation = target;
        let going = if (clock.0 / w.period).fract() < 0.5 {
            1.0
        } else {
            -1.0
        };
        t.rotation = bt.rotation * Quat::from_rotation_arc(Vec3::NEG_Z, Vec3::X * going);
    }
}

fn hash01(x: u32) -> f32 {
    let mut h = x.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    h = ((h >> ((h >> 28) + 4)) ^ h).wrapping_mul(277_803_737);
    h = (h >> 22) ^ h;
    h as f32 / u32::MAX as f32
}

pub fn fire_turrets(
    time: Res<Time>,
    mut turrets: Query<(&Transform, &mut DemoTurret, &mut DemoClock)>,
    mut traces: MessageWriter<ProjectileTrace>,
) {
    for (t, mut turret, mut clock) in &mut turrets {
        clock.0 += time.delta_for(None);
        if clock.0 < turret.next {
            continue;
        }
        turret.next = clock.0 + turret.period;
        turret.shots += 1;
        let n = turret.shots;
        // After 40 holes the curtain gets a rest; the pattern repeats.
        if n % 48 > 40 {
            continue;
        }
        let aim = turret.target
            + DVec3::new(
                ((hash01(n * 2) * 2.0 - 1.0) * turret.spread.x) as f64,
                ((hash01(n * 2 + 1) * 2.0 - 1.0) * turret.spread.y) as f64,
                0.0,
            );
        let dir = (aim - t.translation).normalize();
        traces.write(ProjectileTrace {
            from: t.translation,
            to: t.translation + dir * 30.0,
            radius: 0.02,
            speed: 420.0,
            shooter: None,
        });
    }
}

pub fn burn_and_restore(
    time: Res<Time>,
    mut burners: Query<(&DemoBurner, &mut DemoClock)>,
    mut cloths: Query<&mut Cloth>,
    mut ignite: MessageWriter<IgniteRequest>,
) {
    for (b, mut clock) in &mut burners {
        let before = clock.0;
        clock.0 += time.delta_for(None);
        if before < b.delay && clock.0 >= b.delay {
            ignite.write(IgniteRequest {
                position: b.at,
                radius: 0.35,
                temperature_c: 700.0,
                source: None,
            });
        }
        if clock.0 >= b.period {
            clock.0 = 0.0;
            if let Ok(mut c) = cloths.get_mut(b.cloth) {
                c.set_changed();
            }
        }
    }
}

pub fn cut_and_restore(
    time: Res<Time>,
    mut cutters: Query<(&DemoCutter, &mut DemoClock)>,
    mut bridges: Query<(&mut RopeBridge, Option<&mut SoftBodySim>)>,
) {
    for (c, mut clock) in &mut cutters {
        let before = clock.0;
        clock.0 += time.delta_for(None);
        let Ok((mut spec, body)) = bridges.get_mut(c.bridge) else {
            continue;
        };
        if before < c.delay
            && clock.0 >= c.delay
            && let Some(mut body) = body
        {
            crate::api::cut(&mut body, c.at, 0.9);
        }
        if clock.0 >= c.period {
            clock.0 = 0.0;
            spec.set_changed();
        }
    }
}

pub fn respawn_volumes(
    time: Res<Time>,
    mut volumes: Query<(&mut SoftVolume, &DemoRespawn, &mut DemoClock)>,
) {
    for (mut spec, r, mut clock) in &mut volumes {
        clock.0 += time.delta_for(None);
        if clock.0 >= r.period {
            clock.0 = 0.0;
            spec.set_changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rigs_have_valid_parents_and_keep_feet_near_the_ground() {
        for (bones, rig) in [(&HUMANOID[..], Rig::Humanoid), (&DOG[..], Rig::Dog)] {
            let mut pose = skeleton(bones, rig);
            for (i, p) in pose.parents.iter().enumerate() {
                assert!(p.is_none_or(|p| (p as usize) < i), "parent after child");
            }
            for k in 0..32 {
                pose_rig(rig, k as f32 / 32.0 * TAU, &mut pose);
                let lowest = pose
                    .bones
                    .iter()
                    .map(|b| b.translation.y)
                    .fold(f32::MAX, f32::min);
                assert!((-0.02..0.15).contains(&lowest), "{rig:?} lowest {lowest}");
                assert!(pose.bones.iter().all(|b| b.rotation.is_normalized()));
            }
        }
    }
}
