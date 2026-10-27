//! The soft-body yard: every feature of this module, running, in one place.
//!
//! [`spawn_demo`] builds it around a center point (the showcase calls it at the yard zone):
//!
//! - **Flags** on tall poles with printed patterns, streaming in the world wind, and a field
//!   of 56 pennants simulated on the GPU (`SimSettings::gpu`).
//! - **Curtains**: one shot full of holes by a turret, one set on fire that burns away and
//!   comes back.
//! - **Self-collision**: a wool blanket dropped over a stone ball every few seconds, heaping
//!   up in layers around its foot.
//! - **Rope bridges**: a long one that sags under a walker crossing it, and a short one whose
//!   ropes are cut so it drops, then rebuilt.
//! - **Ropes**: a bead curtain a ball pushes through, a swing, and the cable of a wrecking
//!   ball.
//! - **Soft bodies**: jelly cubes on crates that the wrecking ball knocks off, and a clay
//!   block a press dents for good (plasticity).
//! - **Characters**: a mannequin with a cape and long hair walking in a circle, the same
//!   mannequin in slow motion (its own `LocalTimeScale`), and a dog with fur.
//! - **Mesh colliders**: a crane lowers a silk sheet over a marble statue (the mannequin's body
//!   posed once, its surface a triangle-mesh collider) and lifts it off again (`statue`).
//! - **Water**: dyed silks tied to a bamboo pole across a stream, streaming out soaked in the
//!   current ([`rinse::spawn_rinse`], placed by the world on a real stream).
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

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::Arc;

use engine_core::prelude::*;
use engine_render::{Shape, StandardMaterial};
use engine_world::animation::{BonePose, SkeletonPose};
use engine_world::physics::{Collider, ColliderShape, RigidBody};
use engine_world::{IgniteRequest, ProjectileTrace, WorldShift};

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
pub mod rinse;
pub mod sailboat;
pub mod sculpt;
pub mod skin;
pub mod statue;
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
pub use sailboat::spawn_sailboat;

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
    /// Hang from a crane: held at `high`, lowered slowly to `low`, left there, lifted
    /// briskly back (the unveiling sheet's hook).
    Hoist {
        low: DVec3,
        high: DVec3,
        period: f32,
    },
}

/// Seconds this prop has run, in its own time (`LocalTimeScale`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct DemoClock(pub f32);

/// Walks back and forth across a rope bridge at `speed`, standing on its deck, easing in and
/// out of a stepping turn at each end.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoBridgeWalker {
    pub bridge: Entity,
    /// Along the bridge's local X, meters.
    pub from: f32,
    pub to: f32,
    /// Walking speed, m/s.
    pub speed: f32,
}

/// Meters walked so far: drives a [`DemoSkeleton`]'s gait instead of its clock, so the feet
/// stay planted however the walker speeds up and slows down.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct DemoStride(pub f32);

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

/// Move the props' world-space paths and targets (the hoist, pendulum pivots, turret targets,
/// the boat's home) with the world when it is re-anchored, as the bodies and transforms move.
/// Without it a jump that re-anchors the world (`showcase.goto`) left the unveiling sheet
/// hoisted where the statue used to be. The motions keep +Y as up, which a re-anchoring turns
/// only negligibly within a zone. Runs in `WorldShiftSet::Apply`.
#[allow(clippy::type_complexity)]
pub fn follow_world_shift(
    mut shifts: MessageReader<WorldShift>,
    mut motions: Query<&mut DemoMotion>,
    mut aims: Query<(
        Option<&mut DemoTurret>,
        Option<&mut DemoBurner>,
        Option<&mut DemoCutter>,
        Option<&mut DemoRoll>,
    )>,
    mut cords: Query<&mut statue::DemoCord>,
    mut helms: Query<&mut sailboat::Helm>,
    mut settled: Query<&mut DemoSettled>,
) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    // A remembered body is put back where the world has moved it to.
    for mut kept in &mut settled {
        kept.0.follow_shift(&s);
    }
    for mut m in &mut motions {
        match &mut *m {
            DemoMotion::Pendulum { pivot, swing, .. } => {
                *pivot = s.point(*pivot);
                *swing = s.direction(*swing);
            }
            DemoMotion::Shuttle { from, to, .. } => {
                (*from, *to) = (s.point(*from), s.point(*to));
            }
            DemoMotion::Press { top, .. } => *top = s.point(*top),
            DemoMotion::Circle { center, .. } => *center = s.point(*center),
            DemoMotion::Hoist { low, high, .. } => {
                (*low, *high) = (s.point(*low), s.point(*high));
            }
        }
    }
    for (turret, burner, cutter, roll) in &mut aims {
        if let Some(mut t) = turret {
            t.target = s.point(t.target);
        }
        if let Some(mut b) = burner {
            b.at = s.point(b.at);
        }
        if let Some(mut c) = cutter {
            c.at = s.point(c.at);
        }
        if let Some(mut r) = roll
            && let Some(last) = r.last
        {
            r.last = Some(s.point(last));
        }
    }
    for mut c in &mut cords {
        c.top = s.point(c.top);
    }
    for mut h in &mut helms {
        h.home = s.point(h.home);
    }
}

/// Rebuilds a soft volume (jelly knocked off its crate, dented clay) or a cloth (the
/// dropped blanket) every `period` seconds.
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
        .add_systems(
            PostUpdate,
            statue::cast_statues
                .after(skin::skin_bodies)
                .before(UpdateSet::Propagate),
        )
        .add_systems(FixedPreUpdate, skin::snapshot_poses)
        .add_systems(
            PostUpdate,
            follow_world_shift.in_set(engine_world::WorldShiftSet::Apply),
        );
    app.add_systems(
        FixedUpdate,
        (
            drive_motion,
            walk_bridges,
            animate_skeletons,
            fire_turrets,
            burn_and_restore,
            cut_and_restore,
            respawn_volumes,
            sailboat::sail_boats,
            statue::hang_cords,
            rinse::tie_silks,
        )
            .chain()
            .in_set(SimSet::Gameplay),
    );
    props_anim::register(app);
}

// ---------------------------------------------------------------------------------------
// Spawning

/// Where the propane burner under the burning curtain stands, from the yard's origin. Its
/// flame is an `IgniteRequest`, which a fire module may also carry into fuel on the ground
/// (grass): a world that has one should keep the ground there bare.
pub const CURTAIN_BURNER: DVec3 = DVec3::new(-9.1, 0.0, 6.25);

/// Build the whole yard around `origin` (ground level at `origin.y`).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let c = origin;
    // Procedural textures and materials of the props (made once per world).
    commands.queue(kit::ensure_materials);
    flags(commands, c);
    flag_field(commands, c + DVec3::new(19.0, 0.0, -16.0));
    curtains(commands, c + DVec3::new(-12.0, 0.0, 6.0));
    blanket_drop(commands, c + DVec3::new(-4.5, 0.0, 3.0));
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
    statue::spawn_unveiling(
        commands,
        c + DVec3::new(-14.0, 0.0, -4.0),
        Vec3::new(0.5, 0.0, -0.87),
    );
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
            // Simulated on the GPU: one dispatch for the whole field, at twice the CPU
            // pennants' detail.
            let mut spec = Cloth::flag([1.0, 0.6], material);
            spec.resolution = [20, 12];
            spec.sim.wind_scale = 2.4;
            spec.sim.gpu = true;
            // The kernel's substeps are cheap: keep it at the default rate.
            spec.sim.substeps = SimSettings::default().substeps;
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

fn blanket_drop(commands: &mut Commands, c: DVec3) {
    // A stone ball on the gravel and a wool blanket dropped over it. What doesn't fit over
    // the ball heaps up around its foot; self-collision keeps the heap in layers instead of
    // one sheet.
    let radius = 0.4;
    stone_ball(commands, radius).spawn_solid(
        commands,
        at(c + DVec3::Y * f64::from(radius)),
        ColliderShape::Sphere { radius },
    );
    // Buffalo check: red and charcoal wool.
    let wool = SoftMaterial::hex(0x8f2722)
        .with_pattern(Pattern::Checker(18), 0x221e1c, 0x221e1c)
        .with_roughness(0.95)
        .with_sheen(0.7);
    let blanket = Cloth {
        size: [2.7, 2.7],
        resolution: [40, 40],
        anchor: [0.5, 0.5],
        pins: ClothPins::None,
        mass_per_m2: 0.45,
        shear_compliance: 5e-3,
        bend_compliance: 0.3,
        aero_drag: 0.6,
        material: wool,
        sim: SimSettings {
            self_collide: true,
            thickness: 0.012,
            friction: 0.45,
            damping: 0.15,
            ..SimSettings::default()
        },
        ..Cloth::default()
    };
    // Flat, turned a little and tipped, so one corner lands first.
    let rot = Quat::from_rotation_y(0.6)
        * Quat::from_rotation_z(0.15)
        * Quat::from_rotation_x(-FRAC_PI_2);
    commands.spawn((
        Name::new("Wool blanket"),
        at(c + DVec3::new(0.18, 1.9, 0.1)).with_rotation(rot),
        blanket,
        DemoRespawn { period: 9.0 },
        DemoClock::default(),
    ));
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
            // Setting out from the far tower, into the yard's westerly breeze, so the cloak
            // streams out behind (a following wind wraps it around the legs).
            at(a + DVec3::new(19.5, 3.0, 0.0))
                .with_rotation(Quat::from_rotation_arc(Vec3::NEG_Z, Vec3::NEG_X)),
            TransformInterpolation::default(),
            SoftLoad { mass_kg: 90.0 },
            DemoBridgeWalker {
                bridge,
                from: 18.0,
                to: 1.0,
                speed: 1.25,
            },
            DemoClock::default(),
            DemoStride::default(),
        ))
        .id();
    characters::bridge_walker_body(commands, walker);

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
            // Clay: sinks, and rain doesn't soak in.
            buoyancy: 0.5,
            soak: 0.0,
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

/// How far down a hoist is (0 = up, 1 = down) at fraction `f` of its cycle: up, lowered over
/// a quarter of the cycle, down for a third, lifted in a tenth, up again.
fn hoist_drop(f: f32) -> f32 {
    if f < 0.15 {
        0.0
    } else if f < 0.4 {
        smoothstep(0.15, 0.4, f)
    } else if f < 0.75 {
        1.0
    } else if f < 0.85 {
        1.0 - smoothstep(0.75, 0.85, f)
    } else {
        0.0
    }
}

/// Move kinematic props and walkers.
#[allow(clippy::type_complexity)]
pub fn drive_motion(
    time: Res<Time>,
    mut movers: Query<(
        &mut Transform,
        &DemoMotion,
        &mut DemoClock,
        Option<&LocalTimeScale>,
        Option<&SoftBodySim>,
    )>,
) {
    for (mut t, motion, mut clock, scale, body) in &mut movers {
        // A body waiting for a mesh near it to bake waits where it is, or it would be dragged
        // the whole way in one step when it starts.
        if body.is_some_and(|b| b.waiting_for_bake()) {
            continue;
        }
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
            DemoMotion::Hoist { low, high, period } => {
                let k = hoist_drop((s / period).fract());
                t.translation = high.lerp(low, f64::from(k));
            }
        }
    }
}

/// Walkers stand on the deck of their bridge (which sags under their `SoftLoad`).
pub fn walk_bridges(
    time: Res<Time>,
    bridges: Query<(&Transform, &SoftBodySim), Without<DemoBridgeWalker>>,
    mut walkers: Query<(
        &mut Transform,
        &DemoBridgeWalker,
        &mut DemoClock,
        Option<&mut DemoStride>,
    )>,
) {
    for (mut t, w, mut clock, stride) in &mut walkers {
        clock.0 += time.delta_for(None);
        let Ok((bt, body)) = bridges.get(w.bridge) else {
            continue;
        };
        let out = (w.to - w.from).signum();
        let step = walk_shuttle(clock.0, (w.to - w.from).abs(), w.speed);
        let x = w.from + out * step.along;
        let p = bt.translation + (bt.rotation * Vec3::new(x, 0.0, 0.0)).as_dvec3();
        let deck = body
            .bridge
            .as_ref()
            .and_then(|b| b.deck_height(p + DVec3::Y * 0.2));
        let Some(y) = deck else {
            // Fell off (bridge cut): stay put.
            continue;
        };
        t.translation = DVec3::new(p.x, y, p.z);
        let facing = Quat::from_rotation_arc(Vec3::NEG_Z, Vec3::X * out);
        t.rotation = bt.rotation * facing * Quat::from_rotation_y(step.yaw);
        if let Some(mut stride) = stride {
            stride.0 = step.walked;
        }
    }
}

/// Where a shuttle walk is at a moment: meters `along` the crossing, the turn so far (`yaw`:
/// 0 going out, π coming back) and the meters the feet have `walked`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ShuttleStep {
    along: f32,
    yaw: f32,
    walked: f32,
}

/// Walk out across `length` at `speed`, ease into a stepping half turn at the far end, walk
/// back, turn again. The feet keep stepping slowly through the turns and pick the pace back
/// up over the first and last strides of each crossing.
fn walk_shuttle(t: f32, length: f32, speed: f32) -> ShuttleStep {
    /// Seconds from turning pace to full speed, and for each half turn.
    const RAMP: f32 = 0.8;
    const TURN: f32 = 1.6;
    /// The pace of the feet while turning, as a fraction of `speed`.
    const SLOW: f32 = 0.35;
    let speed = speed.max(0.05);
    let length = length.max(4.0 * speed * RAMP);
    let ramp = speed * RAMP * (1.0 + SLOW) * 0.5;
    let leg = (length - 2.0 * ramp) / speed + 2.0 * RAMP;
    let turn_walk = SLOW * speed * TURN;
    let cycle = 2.0 * (leg + TURN);
    let laps = (t.max(0.0) / cycle).floor();
    let mut s = t.max(0.0) - laps * cycle;
    let mut walked = laps * 2.0 * (length + turn_walk);
    let eased = |s: f32| speed * (SLOW * s + (1.0 - SLOW) * s * s / (2.0 * RAMP));
    // Meters covered `s` seconds into a crossing.
    let cross = |s: f32| {
        if s < RAMP {
            eased(s)
        } else if s < leg - RAMP {
            ramp + speed * (s - RAMP)
        } else {
            length - eased((leg - s).max(0.0))
        }
    };
    let mut yaw = 0.0;
    for (start, dir) in [(0.0, 1.0), (length, -1.0)] {
        if s < leg {
            return ShuttleStep {
                along: start + dir * cross(s),
                yaw,
                walked: walked + cross(s),
            };
        }
        s -= leg;
        walked += length;
        if s < TURN {
            let f = s / TURN;
            return ShuttleStep {
                along: length - start,
                yaw: yaw + PI * f * f * (3.0 - 2.0 * f),
                walked: walked + SLOW * speed * s,
            };
        }
        s -= TURN;
        walked += turn_walk;
        yaw += PI;
    }
    // Only reached through rounding at the very end of a cycle.
    ShuttleStep {
        along: 0.0,
        yaw: TAU,
        walked,
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

/// A demo body as it was once built and settled, put back when the demo restores it instead
/// of building and settling it again: a curtain's 2 s prewarm is 120 steps in one, a step of
/// 100+ ms (the hitch every 24 s on the burning curtain).
#[derive(Component)]
pub struct DemoSettled(Box<SoftBodySim>);

/// Keep `body` as it is now, once it is built and settled, for [`restore`].
fn remember(commands: &mut Commands, e: Entity, body: &SoftBodySim, kept: bool) {
    if !kept && body.steps > 0 && body.pending_prewarm == 0 {
        commands
            .entity(e)
            .insert(DemoSettled(Box::new(body.clone())));
    }
}

/// Not yet remembered, and still settling: its demo waits (lit or cut before it is
/// remembered settled, every lap rebuilt it and settled it again: the burning curtain's 90
/// steps of 10 ms on the PC every 24 s).
fn unsettled(body: &SoftBodySim, settled: Option<&DemoSettled>) -> bool {
    settled.is_none() && (body.steps == 0 || body.pending_prewarm > 0)
}

/// Put a demo body back as it was remembered, or (not yet remembered) rebuild it.
fn restore(body: Option<&mut SoftBodySim>, settled: Option<&DemoSettled>, rebuild: impl FnOnce()) {
    match (body, settled) {
        (Some(body), Some(settled)) => {
            // Drawn anew from the restored triangles.
            let version = body.topology_version.max(settled.0.topology_version) + 1;
            // A bridge keeps the plank entities it has now (a rebuild spawns new ones).
            let planks = body.bridge.as_mut().map(|b| std::mem::take(&mut b.planks));
            *body = (*settled.0).clone();
            body.topology_version = version;
            if let (Some(planks), Some(bridge)) = (planks, body.bridge.as_mut())
                && planks.len() == bridge.planks.len()
            {
                bridge.planks = planks;
            }
        }
        _ => rebuild(),
    }
}

pub fn burn_and_restore(
    mut commands: Commands,
    time: Res<Time>,
    mut burners: Query<(&DemoBurner, &mut DemoClock)>,
    mut cloths: Query<(&mut Cloth, Option<&mut SoftBodySim>, Option<&DemoSettled>)>,
    mut ignite: MessageWriter<IgniteRequest>,
) {
    for (b, mut clock) in &mut burners {
        let before = clock.0;
        clock.0 += time.delta_for(None);
        let Ok((mut cloth, body, settled)) = cloths.get_mut(b.cloth) else {
            continue;
        };
        if let Some(body) = &body
            && before < b.delay
        {
            remember(&mut commands, b.cloth, body, settled.is_some());
            if unsettled(body, settled) {
                clock.0 = before;
                continue;
            }
        }
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
            restore(body.map(|b| b.into_inner()), settled, || {
                cloth.set_changed()
            });
        }
    }
}

pub fn cut_and_restore(
    mut commands: Commands,
    time: Res<Time>,
    mut cutters: Query<(&DemoCutter, &mut DemoClock)>,
    mut bridges: Query<(
        &mut RopeBridge,
        Option<&mut SoftBodySim>,
        Option<&DemoSettled>,
    )>,
) {
    for (c, mut clock) in &mut cutters {
        let before = clock.0;
        clock.0 += time.delta_for(None);
        let Ok((mut spec, body, settled)) = bridges.get_mut(c.bridge) else {
            continue;
        };
        // Put back as it was before the cut: a rebuilt bridge settles for 2 s again, ~19
        // steps a frame of the 16-substep deck (the bridge's 4 ms on the PC).
        if let Some(body) = &body
            && before < c.delay
        {
            remember(&mut commands, c.bridge, body, settled.is_some());
            if unsettled(body, settled) {
                clock.0 = before;
                continue;
            }
        }
        let mut body = body.map(|b| b.into_inner());
        if before < c.delay
            && clock.0 >= c.delay
            && let Some(body) = body.as_deref_mut()
        {
            crate::api::cut(body, c.at, 0.9);
        }
        if clock.0 >= c.period {
            clock.0 = 0.0;
            restore(body, settled, || spec.set_changed());
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn respawn_volumes(
    mut commands: Commands,
    time: Res<Time>,
    mut volumes: Query<(
        Entity,
        &mut SoftVolume,
        &DemoRespawn,
        &mut DemoClock,
        Option<&mut SoftBodySim>,
        Option<&DemoSettled>,
    )>,
    mut cloths: Query<
        (
            Entity,
            &mut Cloth,
            &DemoRespawn,
            &mut DemoClock,
            Option<&mut SoftBodySim>,
            Option<&DemoSettled>,
        ),
        Without<SoftVolume>,
    >,
) {
    let dt = time.delta_for(None);
    for (e, mut spec, r, mut clock, body, settled) in &mut volumes {
        if let Some(body) = &body {
            remember(&mut commands, e, body, settled.is_some());
        }
        clock.0 += dt;
        if clock.0 >= r.period {
            clock.0 = 0.0;
            restore(body.map(|b| b.into_inner()), settled, || spec.set_changed());
        }
    }
    for (e, mut spec, r, mut clock, body, settled) in &mut cloths {
        if let Some(body) = &body {
            remember(&mut commands, e, body, settled.is_some());
        }
        clock.0 += dt;
        if clock.0 >= r.period {
            clock.0 = 0.0;
            restore(body.map(|b| b.into_inner()), settled, || spec.set_changed());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bridge_walk_stays_on_the_deck_never_hurries_and_never_jumps() {
        let (length, speed, dt) = (17.0, 1.25, 1.0 / 120.0);
        let mut last = walk_shuttle(0.0, length, speed);
        assert_eq!((last.along, last.yaw, last.walked), (0.0, 0.0, 0.0));
        let mut turned = 0.0f32;
        for k in 1..(120 * 63) {
            let now = walk_shuttle(k as f32 * dt, length, speed);
            assert!((0.0..=length).contains(&now.along), "{now:?}");
            let moved = (now.along - last.along).abs();
            assert!(moved <= speed * dt * 1.001, "{k}: {last:?} → {now:?}");
            // The feet walk exactly as far as the body moves, except while turning.
            let stepped = now.walked - last.walked;
            assert!(
                stepped >= 0.0 && stepped <= speed * dt * 1.001,
                "{k}: {stepped}"
            );
            let walking = |s: ShuttleStep| (s.yaw / PI).fract() == 0.0;
            if walking(now) && walking(last) {
                assert!((stepped - moved).abs() < 1e-3, "{k}: {stepped} {moved}");
            }
            let dyaw = (now.yaw - last.yaw).rem_euclid(TAU);
            assert!(dyaw < 0.03, "{k}: {last:?} → {now:?}");
            turned += dyaw;
            last = now;
        }
        // 63 s is two round trips (34 m of walking and two half turns each).
        assert!((turned / TAU - 2.0).abs() < 0.3, "{turned}");
    }
}
