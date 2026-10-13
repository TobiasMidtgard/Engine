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
//!
//! The props are fully procedural: meshes from a small toolkit (`meshkit`), textures
//! generated once per world at startup (`texgen`, about 36 MB), shared as keyed assets so
//! repeated props draw instanced (`kit`). [`spawn_review_ground`] adds a cheap textured
//! ground (earth and gravel fraying into grass) for screenshots.
//!
//! [`boat_models`] builds a gaff-rigged clinker dinghy (hull, swinging rig, rudder; its
//! dimensions in [`boat_dims`]) for the sailing cloth, with [`boat_stand`] to show it on.

use std::f32::consts::{FRAC_PI_2, TAU};
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
use crate::rope::{Beads, Rope, RopeBridge, RopeEnd, SoftLoad};
use crate::settings::{Pattern, SimSettings, SoftMaterial};
use crate::volume::{SoftVolume, VolumeShape};

pub mod body;
mod characters;
mod ground;
mod kit;
mod meshkit;
mod props;
mod props_anim;
mod props_boat;
mod props_bridge;
mod props_machines;
mod props_stage;
pub mod sculpt;
pub mod skin;
mod texgen;

#[cfg(test)]
pub(crate) use characters::dog_fur_zones;
pub use characters::{
    DOG, DemoSkeleton, HUMANOID, Rig, animate_skeletons, mannequin_colliders, pose_rig,
    pose_rig_stride, skeleton,
};
use characters::{dog, graphite, ivory, mannequin};
pub use ground::spawn_review_ground;
pub use kit::Model;
use kit::{Mat, register_mesh};
use props::*;
pub use props_anim::{AimAxis, DemoAim, DemoRoll, DemoScrewWheel};
pub use props_boat::{BoatModels, boat_models, boat_stand, dims as boat_dims, hull_points};
use props_bridge::{bridge_plank, plank_size, stone_pier, timber_tower};
use props_machines::{
    GANTRY_PIVOT, PRESS_SCREW, gantry, plank_crate, press_frame, press_ram, press_wheel,
    wrecking_ball,
};
use props_stage::{
    GUN_PIVOT, GUN_TURNTABLE, bead_frame, burner, curtain_ring_xs, curtain_stage, gun_body,
    gun_pedestal, gun_yoke, stone_ball, swing_frame, swing_seat,
};

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
    app.init_resource::<skin::BodyLibrary>()
        .init_resource::<crate::MeshSkins>()
        .add_systems(
            PostUpdate,
            (skin::prepare_bodies, skin::skin_bodies)
                .chain()
                .before(UpdateSet::Propagate),
        )
        .add_systems(FixedPreUpdate, skin::snapshot_poses);
    app.add_systems(
        FixedUpdate,
        (
            drive_motion,
            animate_skeletons,
            walk_bridges,
            fire_turrets,
            burn_and_restore,
            cut_and_restore,
            respawn_volumes,
        )
            .chain()
            .in_set(SimSet::Gameplay),
    );
    props_anim::register(app);
}

// ---------------------------------------------------------------------------------------
// Spawning

/// Build the whole yard around `origin` (ground level at `origin.y`).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let c = origin;
    // Procedural textures and materials of the props (made once per world).
    commands.queue(kit::ensure_materials);
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
        ivory(),
    );
    mannequin(
        commands,
        c + DVec3::new(15.5, 0.0, -6.0),
        2.0,
        0.25,
        "Slow-motion mannequin",
        graphite(),
    );
    dog(commands, c + DVec3::new(-8.0, 0.0, -4.0));
}

fn flags(commands: &mut Commands, c: DVec3) {
    let (pole, pole_collider) = flagpole(commands);
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
        pole.spawn_solid(commands, at(base), pole_collider.clone());
        let mut spec = Cloth::flag([3.0, 2.0], material.with_sheen(0.25));
        spec.sim.wind_scale = 2.2;
        commands.spawn((Name::new(name), at(base + DVec3::new(0.08, 8.8, 0.0)), spec));
    }
}

fn flag_field(commands: &mut Commands, corner: DVec3) {
    let colors = [
        0xe74c3c, 0xf39c12, 0xf1c40f, 0x2ecc71, 0x3498db, 0x9b59b6, 0xecf0f1,
    ];
    let (post, post_collider) = pennant_post(commands);
    for row in 0..8 {
        for col in 0..7 {
            let base = corner + DVec3::new(col as f64 * 1.7, 0.0, row as f64 * 1.7);
            post.spawn_solid(commands, at(base), post_collider.clone());
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
    // Left: velvet with a gold border, shot full of holes.
    let velvet = SoftMaterial::hex(0x7b1e2b)
        .with_pattern(Pattern::Border, 0xc9a227, 0xe8cc6a)
        .with_roughness(0.9)
        .with_sheen(0.8);
    let shot = Cloth::curtain([4.6, 5.6], velvet);
    // Right: a linen curtain that is set on fire, burns away and comes back.
    let linen = SoftMaterial::hex(0xe8dcc4)
        .with_pattern(Pattern::HorizontalStripes(9), 0xd8c8a8, 0xe8dcc4)
        .with_roughness(0.85)
        .with_sheen(0.3)
        .with_subsurface(0.5);
    let burnt = Cloth::curtain([4.6, 5.6], linen);

    // A timber stage with two openings and a brass rod with a ring at every pinned point.
    let mut rings: Vec<f32> = curtain_ring_xs(&shot).iter().map(|x| x - 2.6).collect();
    rings.extend(curtain_ring_xs(&burnt).iter().map(|x| x + 2.6));
    let (stage, stage_collider) = curtain_stage(commands, &rings);
    stage.spawn_solid(commands, at(c), stage_collider);

    let left = c + DVec3::new(-2.6, 6.0, 0.25);
    commands.spawn((Name::new("Shot curtain"), at(left), shot));
    // The turret stands in front and shoots through it: a pedestal gun that swings onto
    // each target before it fires.
    let turret = c + DVec3::new(-2.6, f64::from(GUN_PIVOT), 7.0);
    let turret_entity = commands
        .spawn((
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
        ))
        .id();
    let (pedestal, pedestal_collider) = gun_pedestal(commands);
    let base = pedestal.spawn_solid(
        commands,
        at(turret - DVec3::Y * f64::from(GUN_PIVOT)),
        pedestal_collider,
    );
    let yoke = commands
        .spawn((
            Transform::from_xyz(0.0, f64::from(GUN_TURNTABLE), 0.0),
            DemoAim {
                turret: turret_entity,
                axis: AimAxis::Yaw,
                angle: 0.0,
            },
            ChildOf(base),
        ))
        .id();
    gun_yoke(commands).attach(commands, yoke, Transform::default());
    let gun = commands
        .spawn((
            Transform::from_xyz(0.0, f64::from(GUN_PIVOT - GUN_TURNTABLE), 0.0),
            DemoAim {
                turret: turret_entity,
                axis: AimAxis::Pitch,
                angle: 0.0,
            },
            ChildOf(yoke),
        ))
        .id();
    gun_body(commands).attach(commands, gun, Transform::default());

    let right = c + DVec3::new(2.6, 6.0, 0.25);
    let burning = commands
        .spawn((Name::new("Burning curtain"), at(right), burnt))
        .id();
    let flame = right + DVec3::new(0.3, -5.4, 0.0);
    commands.spawn((
        Name::new("Curtain igniter"),
        at(right),
        DemoBurner {
            cloth: burning,
            at: flame,
            delay: 2.0,
            period: 24.0,
        },
        DemoClock::default(),
    ));
    // A propane burner on a tripod aimed at the hem, where the igniter lights it.
    let nozzle = (flame - c).as_vec3();
    burner(
        commands,
        nozzle,
        Vec3::new(nozzle.x, 0.0, nozzle.z + 0.5),
        Vec3::new(nozzle.x + 0.85, 0.0, nozzle.z + 0.95),
    )
    .spawn(commands, at(c));
}

fn bridges(commands: &mut Commands, c: DVec3) {
    // The long bridge between two timber towers.
    let (a, b) = (
        c + DVec3::new(2.0, 0.0, 10.0),
        c + DVec3::new(24.0, 0.0, 10.0),
    );
    let mut long = RopeBridge {
        length: 19.0,
        planks: 22,
        sag: 1.3,
        ..RopeBridge::default()
    };
    let half_w = long.width * 0.5;
    for (p, side) in [(a, 1.0), (b, -1.0)] {
        let (tower, collider) = timber_tower(commands, side, half_w);
        tower.spawn_solid(commands, at(p), collider);
    }
    let plank = "softbody/yard/plank/long".to_string();
    register_mesh(commands, plank.clone(), bridge_plank(plank_size(&long), 1));
    long.plank_mesh = Some(plank);
    long.plank_material = Some(Mat::Timber.key());
    let bridge = commands
        .spawn((
            Name::new("Rope bridge"),
            at(a + DVec3::new(1.5, 3.0, 0.0)),
            long,
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

    // A short bridge between two sandstone piers that gets cut and drops.
    let (a, b) = (
        c + DVec3::new(-7.0, 0.0, -11.0),
        c + DVec3::new(3.0, 0.0, -11.0),
    );
    let mut short_spec = RopeBridge {
        length: 8.4,
        planks: 10,
        sag: 0.6,
        width: 1.2,
        ..RopeBridge::default()
    };
    let half_w = short_spec.width * 0.5;
    for (p, side, seed) in [(a, 1.0, 1), (b, -1.0, 2)] {
        let (pier, collider) = stone_pier(commands, side, half_w, seed);
        pier.spawn_solid(commands, at(p), collider);
    }
    let plank = "softbody/yard/plank/short".to_string();
    register_mesh(
        commands,
        plank.clone(),
        bridge_plank(plank_size(&short_spec), 2),
    );
    short_spec.plank_mesh = Some(plank);
    short_spec.plank_material = Some(Mat::Timber.key());
    let short = commands
        .spawn((
            Name::new("Cut bridge"),
            at(a + DVec3::new(0.8, 4.0, 0.0)),
            short_spec,
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
    let xs: Vec<f32> = (0..16).map(|i| -1.5 + i as f32 * 0.2).collect();
    let (frame, frame_collider) = bead_frame(commands, &xs, 2.78);
    frame.spawn_solid(commands, at(c), frame_collider);
    let colors = [0xc0392b, 0xe67e22, 0xf1c40f, 0x27ae60, 0x2980b9, 0x8e44ad];
    for (i, &x) in xs.iter().enumerate() {
        let color = colors[i % colors.len()];
        commands.spawn((
            Name::new(format!("Bead string {}", i + 1)),
            at(c + DVec3::new(x as f64, 2.78, 0.0)),
            Rope {
                end_a: RopeEnd::Here { offset: [0.0; 3] },
                end_b: RopeEnd::Free,
                length: 2.5,
                segments: 18,
                radius: 0.0025,
                mass_per_m: 0.25,
                // Alternate strings of cut-glass and smooth lacquered beads.
                beads: Some(Beads {
                    spacing: 0.042,
                    radius: 0.016,
                    elongation: if i % 2 == 0 { 0.85 } else { 1.25 },
                    facets: if i % 2 == 0 { 10 } else { 0 },
                }),
                material: SoftMaterial::hex(color)
                    .with_roughness(if i % 2 == 0 { 0.06 } else { 0.25 })
                    .with_sheen(0.0)
                    .with_wrinkles(0.0),
                ..Rope::default()
            },
        ));
    }
    // A stone ball rolls through the strings and back.
    let ball = commands
        .spawn((
            Name::new("Bead curtain ball"),
            at(c + DVec3::new(0.0, 0.6, 3.0)),
            TransformInterpolation::default(),
            RigidBody::kinematic(),
            Collider::sphere(0.6),
            DemoMotion::Shuttle {
                from: c + DVec3::new(-0.3, 0.6, 3.0),
                to: c + DVec3::new(0.3, 0.6, -3.0),
                period: 9.0,
            },
            DemoRoll {
                radius: 0.6,
                last: None,
            },
            DemoClock::default(),
        ))
        .id();
    stone_ball(commands, 0.6).attach(commands, ball, Transform::default());
}

fn swing(commands: &mut Commands, c: DVec3) {
    let pivot = c + DVec3::new(0.0, 4.1, 0.0);
    let ropes = [-0.3f32, 0.3];
    let (frame, frame_collider) = swing_frame(commands, &ropes, 4.1);
    frame.spawn_solid(commands, at(c), frame_collider);
    let seat = commands
        .spawn((
            Name::new("Swing seat"),
            at(pivot - DVec3::Y * 2.6),
            TransformInterpolation::default(),
            RigidBody::kinematic(),
            Collider::cuboid(Vec3::new(0.35, 0.03, 0.18)),
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
    swing_seat(commands, &ropes).attach(commands, seat, Transform::default());
    for x in ropes {
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
    let jellies = [
        ("Green jelly", 0x2ecc71),
        ("Raspberry jelly", 0xe0245e),
        ("Orange jelly", 0xf39c12),
    ];
    let crate_collider = ColliderShape::Compound(vec![kit::part(
        Vec3::Y * 0.6,
        kit::cuboid(Vec3::splat(1.2)),
    )]);
    for (i, (name, color)) in jellies.into_iter().enumerate() {
        let x = (i as f64 - 1.0) * 2.4;
        // Plank crates, each a little different, turned so their braces don't line up.
        plank_crate(commands, i as u32 + 1).spawn_solid(
            commands,
            at(c + DVec3::new(x, 0.0, 0.0))
                .with_rotation(Quat::from_rotation_y(i as f32 * FRAC_PI_2)),
            crate_collider.clone(),
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
    let swing = Vec3::new(1.0, 0.0, 0.25).normalize();
    let (gantry_model, gantry_collider) = gantry(commands, swing);
    gantry_model.spawn_solid(
        commands,
        at(c + DVec3::new(0.0, 0.0, -1.2)),
        gantry_collider,
    );
    let pivot = c + DVec3::new(0.0, f64::from(GANTRY_PIVOT), -1.2);
    let length = 4.9;
    let ball = commands
        .spawn((
            Name::new("Wrecking ball"),
            at(pivot - DVec3::Y * length as f64),
            TransformInterpolation::default(),
            RigidBody::kinematic(),
            Collider::sphere(0.45),
            DemoMotion::Pendulum {
                pivot,
                swing,
                length,
                amplitude: 0.85,
                period: 4.6,
            },
            DemoClock::default(),
        ))
        .id();
    wrecking_ball(commands, 0.45).attach(commands, ball, Transform::default());
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
    // A cast-iron screw press: bed and table (top at 1 m), columns and crosshead.
    let (frame, frame_collider) = press_frame(commands);
    frame.spawn_solid(commands, at(c), frame_collider);
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
    // The ram rides the columns under the crosshead; it bottoms out where it always did
    // (1.47 m), from a lower top so the screw stays in its nut.
    let top = c + DVec3::Y * 2.15;
    let press = commands
        .spawn((
            Name::new("Press"),
            at(top),
            TransformInterpolation::default(),
            RigidBody::kinematic(),
            Collider::cuboid(Vec3::new(0.25, 0.2, 0.25)),
            DemoMotion::Press {
                top,
                depth: 0.68,
                period: 4.5,
            },
            DemoClock::default(),
        ))
        .id();
    press_ram(commands).attach(commands, press, Transform::default());
    // The handwheel on the screw turns 1.25 times over the stroke.
    let wheel = commands
        .spawn((
            Transform::from_xyz(0.0, f64::from(PRESS_SCREW + 0.36), 0.0),
            DemoScrewWheel {
                ram: press,
                lead: 0.68 / 1.25,
                last: None,
                angle: 0.0,
            },
            ChildOf(press),
        ))
        .id();
    press_wheel(commands).attach(commands, wheel, Transform::default());
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
