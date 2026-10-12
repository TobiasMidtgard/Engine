//! The terrain lab: a showcase of footprints and tracks in mud, snow, sand and grass, and of
//! terrain edits (a blast crater, a dug trench, a built-up mound).
//!
//! ```ignore
//! engine_terrain::demo::spawn_demo(&mut commands, zone_center);
//! ```
//!
//! The ground needs a terrain under it (for example the showcase planet). Four strips are
//! painted with materials; a person, a dog and a horse walk loops through them and a rover
//! drives across, so every kind of print shows side by side. They walk for a while before
//! the first frame, so trails are visible right away. Rain (from the weather module) darkens
//! the soil, fills prints with water and turns dirt to mud; snowfall fills snow prints in.
//!
//! The walkers are print makers only: each is an entity with a [`DemoWalker`] (its kind, path
//! and pace) and a `Transform` that moves along the path facing forward, with no body. Bodies
//! belong to the modules that own people, animals and vehicles; a game attaches theirs as
//! children of the walker entities. [`LabOptions::stand_in_bodies`] adds plain shapes instead
//! for quick tests.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_world::{ContactKind, EditOp, EditShape, GroundContact, SurfaceMaterials, TerrainEdit};

/// What walks: decides the print pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WalkerKind {
    /// Two feet, alternating.
    #[default]
    Person,
    /// Four paws, trotting.
    Dog,
    /// Four hooves.
    Horse,
    /// Four wheels.
    Rover,
}

/// Follows an elliptical loop and presses prints into the ground as it goes.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoWalker {
    pub kind: WalkerKind,
    /// Loop center (world) and half-axes along X and Z, meters.
    pub center: DVec3,
    pub radius_x: f64,
    pub radius_z: f64,
    /// Meters per second.
    pub speed: f32,
    /// Meters between steps (per foot pair for animals, between tread marks for wheels).
    pub step_m: f32,
    /// Weight, kg.
    pub load_kg: f32,
    /// Height of the body origin above the ground, meters.
    pub body_height: f32,
    /// Seconds to walk instantly on the first update (so trails exist from the start).
    pub prewalk_s: f32,
    /// Angle along the loop, radians.
    pub phase: f64,
    pub since_step: f32,
    pub left: bool,
    pub started: bool,
    /// Fixed steps to wait (once a terrain exists) before the pre-walk, so the painted strips
    /// are in place.
    pub wait_steps: u32,
}

impl Default for DemoWalker {
    fn default() -> Self {
        Self {
            kind: WalkerKind::Person,
            center: DVec3::ZERO,
            radius_x: 6.0,
            radius_z: 10.0,
            speed: 1.4,
            step_m: 0.75,
            load_kg: 80.0,
            body_height: 0.9,
            prewalk_s: 40.0,
            phase: 0.0,
            since_step: 0.0,
            left: false,
            started: false,
            wait_steps: 6,
        }
    }
}

impl DemoWalker {
    fn position(&self, phase: f64) -> DVec3 {
        self.center
            + DVec3::new(
                phase.cos() * self.radius_x,
                0.0,
                phase.sin() * self.radius_z,
            )
    }

    /// Yaw (radians, 0 = facing −Z) of the direction of travel at `phase`.
    fn yaw(&self, phase: f64) -> f32 {
        let d = DVec3::new(
            -phase.sin() * self.radius_x,
            0.0,
            phase.cos() * self.radius_z,
        );
        (-d.x).atan2(-d.z) as f32
    }

    /// Contacts for one step at `phase` (positions on the ground plane, y filled later).
    fn step_contacts(&mut self, phase: f64) -> Vec<(DVec3, f32, ContactKind, [f32; 2], f32)> {
        let p = self.position(phase);
        let yaw = self.yaw(phase);
        let fwd = DVec3::new(-(yaw.sin() as f64), 0.0, -(yaw.cos() as f64));
        let side = DVec3::new(-fwd.z, 0.0, fwd.x);
        let mut out = Vec::new();
        self.left = !self.left;
        let s = if self.left { 1.0 } else { -1.0 };
        match self.kind {
            WalkerKind::Person => {
                out.push((
                    p + side * 0.12 * s,
                    yaw,
                    ContactKind::Foot,
                    [0.0, 0.0],
                    self.load_kg,
                ));
            }
            WalkerKind::Dog => {
                // Trot: diagonal pairs.
                out.push((
                    p + fwd * 0.3 + side * 0.09 * s,
                    yaw,
                    ContactKind::Paw,
                    [0.0, 0.0],
                    self.load_kg * 0.5,
                ));
                out.push((
                    p - fwd * 0.3 - side * 0.09 * s,
                    yaw,
                    ContactKind::Paw,
                    [0.0, 0.0],
                    self.load_kg * 0.5,
                ));
            }
            WalkerKind::Horse => {
                out.push((
                    p + fwd * 0.7 + side * 0.18 * s,
                    yaw,
                    ContactKind::Hoof,
                    [0.0, 0.0],
                    self.load_kg * 0.5,
                ));
                out.push((
                    p - fwd * 0.7 - side * 0.18 * s,
                    yaw,
                    ContactKind::Hoof,
                    [0.0, 0.0],
                    self.load_kg * 0.5,
                ));
            }
            WalkerKind::Rover => {
                for (lx, lz) in [(-0.85, 1.1), (0.85, 1.1), (-0.85, -1.1), (0.85, -1.1)] {
                    out.push((
                        p + side * lx + fwd * lz,
                        yaw,
                        ContactKind::Wheel,
                        [0.26, 0.3],
                        self.load_kg * 0.25,
                    ));
                }
            }
        }
        out
    }
}

/// Moves demo walkers and emits their ground contacts.
pub fn walk(
    time: Res<Time>,
    mut q: Query<(&mut DemoWalker, &mut Transform)>,
    ground: Res<engine_world::Ground>,
    terrains: Res<crate::Terrains>,
    mut contacts: MessageWriter<GroundContact>,
) {
    // Wait for the terrain (and the painted strips, applied with it) before walking.
    if terrains.entries.is_empty() {
        return;
    }
    for (mut w, mut tf) in &mut q {
        let perimeter = std::f64::consts::PI
            * (3.0 * (w.radius_x + w.radius_z)
                - ((3.0 * w.radius_x + w.radius_z) * (w.radius_x + 3.0 * w.radius_z)).sqrt());
        let mut dt = time.delta_secs();
        if w.wait_steps > 0 {
            w.wait_steps -= 1;
            continue;
        }
        if !w.started {
            w.started = true;
            dt += w.prewalk_s;
        }
        let mut remaining = w.speed * dt;
        // Walk in sub-steps so every step on the way leaves its print.
        while remaining > 0.0 {
            let until_step = (w.step_m - w.since_step).max(0.0);
            let d = remaining.min(until_step.max(1e-3));
            w.phase += d as f64 / perimeter * std::f64::consts::TAU;
            w.since_step += d;
            remaining -= d;
            if w.since_step >= w.step_m {
                w.since_step = 0.0;
                let phase = w.phase;
                for (p, yaw, kind, size, load) in w.step_contacts(phase) {
                    let g = ground.sample(p + DVec3::Y * 2.0);
                    let y = g.map(|g| g.height).unwrap_or(p.y);
                    contacts.write(GroundContact {
                        position: DVec3::new(p.x, y, p.z),
                        normal: g.map(|g| g.normal).unwrap_or(Vec3::Y),
                        size,
                        yaw,
                        load_kg: load,
                        speed: w.speed,
                        kind,
                        surface: None,
                        entity: None,
                    });
                }
            }
        }
        let p = w.position(w.phase);
        let y = ground.height(p + DVec3::Y * 2.0).unwrap_or(p.y);
        tf.translation = DVec3::new(p.x, y + w.body_height as f64, p.z);
        tf.rotation = Quat::from_rotation_y(w.yaw(w.phase));
    }
}

/// Queue a terrain edit from `Commands`.
fn edit(
    commands: &mut Commands,
    center: DVec3,
    shape: EditShape,
    op: EditOp,
    material: Option<&'static str>,
) {
    commands.queue(move |world: &mut World| {
        let surface = material.and_then(|m| world.resource::<SurfaceMaterials>().id(m));
        world.write_message(TerrainEdit {
            center,
            shape,
            op,
            surface,
            source: None,
        });
    });
}

/// Options for [`spawn_demo_with`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LabOptions {
    /// Give the walkers plain stand-in shapes (capsule, boxes) instead of leaving them for
    /// another module's bodies.
    pub stand_in_bodies: bool,
}

/// Build the terrain lab around `origin` (on flat ground): 4 painted strips 9 m wide and 32 m
/// long along Z (mud, snow, sand, grass from −X to +X), walkers on each, a rover crossing
/// them, and a crater, a trench and a mound on the far side. The walkers have no bodies (see
/// the module docs).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    spawn_demo_with(commands, origin, LabOptions::default());
}

/// [`spawn_demo`] with options.
pub fn spawn_demo_with(commands: &mut Commands, origin: DVec3, options: LabOptions) {
    let strips: [(Option<&'static str>, WalkerKind, f32); 4] = [
        (Some("mud"), WalkerKind::Person, 80.0),
        (Some("snow"), WalkerKind::Dog, 30.0),
        (Some("sand"), WalkerKind::Horse, 500.0),
        (None, WalkerKind::Person, 80.0),
    ];
    let person = StandardMaterial::hex(0x3b4a6b).with_roughness(0.7);
    let dog = StandardMaterial::hex(0x8a6a48).with_roughness(0.9);
    let horse = StandardMaterial::hex(0x5a3a28).with_roughness(0.8);
    let rover = StandardMaterial::hex(0xd8d8d0)
        .with_metallic(0.6)
        .with_roughness(0.35);
    for (i, (material, kind, load)) in strips.into_iter().enumerate() {
        let c = origin + DVec3::new(-15.0 + i as f64 * 10.0, 0.0, 0.0);
        if let Some(m) = material {
            // Overlapping spheres paint a rounded strip.
            for k in 0..7 {
                edit(
                    commands,
                    c + DVec3::new(0.0, 0.0, -15.0 + k as f64 * 5.0),
                    EditShape::Sphere { radius: 5.2 },
                    EditOp::Paint,
                    Some(m),
                );
            }
        }
        let (shape, mat, body_height, step, speed): (Shape, StandardMaterial, f32, f32, f32) =
            match kind {
                WalkerKind::Person => (
                    Shape::Capsule {
                        radius: 0.25,
                        height: 1.75,
                        segments: 16,
                    },
                    person,
                    0.88,
                    0.72,
                    1.4,
                ),
                WalkerKind::Dog => (
                    Shape::Cuboid {
                        size: [0.3, 0.35, 0.8],
                    },
                    dog,
                    0.45,
                    0.55,
                    1.8,
                ),
                WalkerKind::Horse => (
                    Shape::Cuboid {
                        size: [0.6, 0.9, 1.9],
                    },
                    horse,
                    1.35,
                    1.1,
                    2.2,
                ),
                WalkerKind::Rover => (
                    Shape::Cuboid {
                        size: [1.9, 0.6, 3.0],
                    },
                    rover,
                    0.8,
                    0.3,
                    3.0,
                ),
            };
        let mut walker = commands.spawn((
            Transform::from_translation(c),
            DemoWalker {
                kind,
                center: c,
                radius_x: 2.6,
                radius_z: 12.0,
                speed,
                step_m: step,
                load_kg: load,
                body_height,
                phase: i as f64 * 1.3,
                ..Default::default()
            },
        ));
        if options.stand_in_bodies {
            walker.insert(MeshRenderer::new(shape, mat));
        }
    }
    // A rover looping across all four strips.
    let mut rover_walker = commands.spawn((
        Transform::from_translation(origin),
        DemoWalker {
            kind: WalkerKind::Rover,
            center: origin + DVec3::new(0.0, 0.0, 4.0),
            radius_x: 17.0,
            radius_z: 7.0,
            speed: 3.0,
            step_m: 0.25,
            load_kg: 1600.0,
            body_height: 0.75,
            prewalk_s: 20.0,
            ..Default::default()
        },
    ));
    if options.stand_in_bodies {
        rover_walker.insert(MeshRenderer::new(
            Shape::Cuboid {
                size: [1.9, 0.6, 3.0],
            },
            rover,
        ));
    }

    // Terrain edits beyond the strips: a blast crater, a trench and a built-up mound.
    let far = origin + DVec3::new(0.0, 0.0, -26.0);
    edit(
        commands,
        far + DVec3::new(-12.0, 0.5, 0.0),
        EditShape::Sphere { radius: 4.5 },
        EditOp::Remove,
        None,
    );
    edit(
        commands,
        far + DVec3::new(-12.0, 0.0, 0.0),
        EditShape::Sphere { radius: 6.5 },
        EditOp::Paint,
        Some("ash"),
    );
    edit(
        commands,
        far + DVec3::new(-3.0, 0.2, -3.0),
        EditShape::Capsule {
            to: Vec3::new(12.0, 0.0, 4.0),
            radius: 1.3,
        },
        EditOp::Remove,
        None,
    );
    edit(
        commands,
        far + DVec3::new(-3.0, 0.0, -3.0),
        EditShape::Capsule {
            to: Vec3::new(12.0, 0.0, 4.0),
            radius: 2.2,
        },
        EditOp::Paint,
        Some("dirt"),
    );
    edit(
        commands,
        far + DVec3::new(15.0, -1.5, 0.0),
        EditShape::Sphere { radius: 3.5 },
        EditOp::Add,
        Some("dirt"),
    );
}
