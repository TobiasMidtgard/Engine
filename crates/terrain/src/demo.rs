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
//! children of the walker entities. [`LabOptions::stand_in_bodies`] adds simple articulated
//! figures instead (a hiker, a dog, a horse and a rover whose legs swing and wheels turn with
//! their steps).
//!
//! Strips, the crater, the trench and the mound are placed relative to the ground height under
//! `origin` when they are applied, so the lab works on uneven ground.

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
    /// Distance walked so far, meters (turns the stand-in rover's wheels).
    pub travelled_m: f64,
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
            travelled_m: 0.0,
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

    /// Where in its stride cycle (two steps) the walker is, 0..1.
    pub fn stride(&self) -> f32 {
        let step = (self.since_step / self.step_m.max(1e-3)).clamp(0.0, 1.0);
        (step + if self.left { 1.0 } else { 0.0 }) * 0.5
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
            w.travelled_m += d as f64;
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

/// A moving part of a stand-in body: a leg or arm swinging about its pivot's X axis, or a
/// wheel turning, in time with its walker.
#[derive(Component, Debug, Clone, Copy)]
pub struct LabLimb {
    pub walker: Entity,
    /// Swing amplitude, radians (0 for wheels).
    pub swing: f32,
    /// Where in the stride cycle the limb is forward-most (0.5 = opposite to 0).
    pub offset: f32,
    /// Wheel radius, meters (0 for legs and arms).
    pub wheel_radius: f32,
}

/// Swings stand-in legs and arms and turns wheels.
pub fn swing_limbs(
    walkers: Query<&DemoWalker>,
    mut limbs: Query<(&LabLimb, &mut Transform), Without<DemoWalker>>,
) {
    for (limb, mut tf) in &mut limbs {
        let Ok(w) = walkers.get(limb.walker) else {
            continue;
        };
        let angle = if limb.wheel_radius > 0.0 {
            -(w.travelled_m / limb.wheel_radius as f64).rem_euclid(std::f64::consts::TAU) as f32
        } else {
            ((w.stride() + limb.offset) * std::f32::consts::TAU).sin() * limb.swing
        };
        tf.rotation = Quat::from_rotation_x(angle);
    }
}

/// Queue a terrain edit from `Commands` at `origin + offset`, with `offset.y` measured from
/// the ground there (its height when the edit is applied), so the lab sits on whatever ground
/// is there.
fn edit(
    commands: &mut Commands,
    origin: DVec3,
    offset: DVec3,
    shape: EditShape,
    op: EditOp,
    material: Option<&'static str>,
) {
    commands.queue(move |world: &mut World| {
        let surface = material.and_then(|m| world.resource::<SurfaceMaterials>().id(m));
        let at = origin + DVec3::new(offset.x, 50.0, offset.z);
        let ground = world
            .get_resource::<engine_world::Ground>()
            .and_then(|g| g.height(at))
            .unwrap_or(origin.y);
        world.write_message(TerrainEdit {
            center: DVec3::new(at.x, ground + offset.y, at.z),
            shape,
            op,
            surface,
            source: None,
        });
    });
}

/// Colors of the stand-in figures.
struct Look {
    shirt: StandardMaterial,
    pants: StandardMaterial,
    skin: StandardMaterial,
    hair: StandardMaterial,
    pack: StandardMaterial,
    boots: StandardMaterial,
    fur: StandardMaterial,
    coat: StandardMaterial,
    dark: StandardMaterial,
    hull: StandardMaterial,
    rubber: StandardMaterial,
    panel: StandardMaterial,
}

impl Look {
    fn new() -> Self {
        Self {
            shirt: StandardMaterial::hex(0x3b5a7b).with_roughness(0.85),
            pants: StandardMaterial::hex(0x3a3a30).with_roughness(0.9),
            skin: StandardMaterial::hex(0xc8987a)
                .with_roughness(0.6)
                .with_subsurface(0.4),
            hair: StandardMaterial::hex(0x2a1c14).with_roughness(0.8),
            pack: StandardMaterial::hex(0x8a3a24).with_roughness(0.8),
            boots: StandardMaterial::hex(0x3a2a1e).with_roughness(0.7),
            fur: StandardMaterial::hex(0x8a6a48)
                .with_roughness(0.95)
                .with_subsurface(0.2),
            coat: StandardMaterial::hex(0x5a3a28)
                .with_roughness(0.7)
                .with_subsurface(0.15),
            dark: StandardMaterial::hex(0x1e1814).with_roughness(0.8),
            hull: StandardMaterial::hex(0xd8d8d0)
                .with_metallic(0.6)
                .with_roughness(0.35),
            rubber: StandardMaterial::hex(0x1a1a1a).with_roughness(0.9),
            panel: StandardMaterial::hex(0x1a2a48)
                .with_metallic(0.3)
                .with_roughness(0.25),
        }
    }
}

fn part(
    commands: &mut Commands,
    parent: Entity,
    shape: Shape,
    mat: &StandardMaterial,
    tf: Transform,
) {
    commands.spawn((tf, MeshRenderer::new(shape, *mat), ChildOf(parent)));
}

/// A swinging limb: a pivot under `walker` at `pivot`, its parts hung below it.
fn limb(
    commands: &mut Commands,
    walker: Entity,
    pivot: Vec3,
    swing: f32,
    offset: f32,
    wheel_radius: f32,
) -> Entity {
    commands
        .spawn((
            Transform::from_translation(pivot.as_dvec3()),
            LabLimb {
                walker,
                swing,
                offset,
                wheel_radius,
            },
            ChildOf(walker),
        ))
        .id()
}

fn at(x: f32, y: f32, z: f32) -> Transform {
    Transform::from_translation(DVec3::new(x as f64, y as f64, z as f64))
}

fn capsule(radius: f32, height: f32) -> Shape {
    Shape::Capsule {
        radius,
        height,
        segments: 12,
    }
}

/// Stand-in figure for a walker (body origin `body_height` above the ground, facing −Z).
fn stand_in_body(
    commands: &mut Commands,
    walker: Entity,
    kind: WalkerKind,
    body_height: f32,
    look: &Look,
) {
    use std::f32::consts::FRAC_PI_2;
    let lying = Quat::from_rotation_x(FRAC_PI_2);
    match kind {
        WalkerKind::Person => {
            // A hiker: legs and arms swing in opposition, origin at the hips (0.88 m).
            for (side, offset) in [(-1.0f32, 0.0f32), (1.0, 0.5)] {
                let leg = limb(
                    commands,
                    walker,
                    Vec3::new(0.1 * side, 0.0, 0.0),
                    0.42,
                    offset,
                    0.0,
                );
                part(
                    commands,
                    leg,
                    capsule(0.075, 0.86),
                    &look.pants,
                    at(0.0, -0.43, 0.0),
                );
                part(
                    commands,
                    leg,
                    Shape::Cuboid {
                        size: [0.11, 0.09, 0.27],
                    },
                    &look.boots,
                    at(0.0, -0.835, -0.05),
                );
                let arm = limb(
                    commands,
                    walker,
                    Vec3::new(0.24 * side, 0.56, 0.0),
                    0.35,
                    offset + 0.5,
                    0.0,
                );
                part(
                    commands,
                    arm,
                    capsule(0.05, 0.6),
                    &look.shirt,
                    at(0.0, -0.28, 0.0),
                );
                part(
                    commands,
                    arm,
                    Shape::Sphere {
                        radius: 0.045,
                        segments: 10,
                    },
                    &look.skin,
                    at(0.0, -0.6, 0.0),
                );
            }
            part(
                commands,
                walker,
                capsule(0.17, 0.62),
                &look.shirt,
                at(0.0, 0.34, 0.0).with_scale(Vec3::new(1.0, 1.0, 0.7)),
            );
            part(
                commands,
                walker,
                capsule(0.05, 0.1),
                &look.skin,
                at(0.0, 0.7, 0.0),
            );
            part(
                commands,
                walker,
                Shape::Sphere {
                    radius: 0.11,
                    segments: 16,
                },
                &look.skin,
                at(0.0, 0.84, 0.0).with_scale(Vec3::new(0.9, 1.05, 1.0)),
            );
            part(
                commands,
                walker,
                Shape::Sphere {
                    radius: 0.115,
                    segments: 16,
                },
                &look.hair,
                at(0.0, 0.88, 0.02).with_scale(Vec3::new(0.95, 0.75, 1.0)),
            );
            part(
                commands,
                walker,
                Shape::Cuboid {
                    size: [0.3, 0.42, 0.17],
                },
                &look.pack,
                at(0.0, 0.42, 0.19),
            );
        }
        WalkerKind::Dog | WalkerKind::Horse => {
            let horse = kind == WalkerKind::Horse;
            let fur = if horse { &look.coat } else { &look.fur };
            // Proportions (meters, relative to the body origin `body_height` up): barrel
            // (radius, length, height), legs where the prints go (see `step_contacts`), neck,
            // head, ears and tail. The capsule axis is +Y; `tilt` turns its top end toward
            // −Z (forward) for negative angles.
            struct Animal {
                barrel: (f32, f32, f32),
                hip: f32,
                legs: (f32, f32),
                thigh: f32,
                shin: f32,
                neck: (f32, f32, Vec3, f32),
                head: (f32, f32, Vec3, f32),
                muzzle: (f32, f32, Vec3, f32),
                ears: (f32, f32, Vec3),
                tail: (f32, f32, Vec3, f32),
            }
            let a = if horse {
                Animal {
                    barrel: (0.36, 1.25, -0.08),
                    hip: -0.25,
                    legs: (0.19, 0.7),
                    thigh: 0.09,
                    shin: 0.045,
                    neck: (0.17, 0.62, Vec3::new(0.0, 0.42, -1.02), -0.62),
                    head: (0.12, 0.42, Vec3::new(0.0, 0.8, -1.42), 1.05),
                    muzzle: (0.09, 0.12, Vec3::new(0.0, 0.62, -1.58), 1.05),
                    ears: (0.035, 0.13, Vec3::new(0.06, 1.05, -1.3)),
                    tail: (0.055, 0.7, Vec3::new(0.0, -0.22, 1.18), -0.35),
                }
            } else {
                Animal {
                    barrel: (0.12, 0.42, 0.05),
                    hip: -0.0,
                    legs: (0.09, 0.3),
                    thigh: 0.04,
                    shin: 0.025,
                    neck: (0.065, 0.16, Vec3::new(0.0, 0.17, -0.34), -0.7),
                    head: (0.075, 0.06, Vec3::new(0.0, 0.27, -0.43), 1.4),
                    muzzle: (0.04, 0.09, Vec3::new(0.0, 0.24, -0.53), 1.45),
                    ears: (0.03, 0.08, Vec3::new(0.045, 0.35, -0.4)),
                    tail: (0.02, 0.24, Vec3::new(0.0, 0.16, 0.38), 0.8),
                }
            };
            let tilted = |p: Vec3, tilt: f32| {
                Transform::from_translation(p.as_dvec3()).with_rotation(Quat::from_rotation_x(tilt))
            };
            // Trotting: diagonal leg pairs move together.
            let leg_len = body_height + a.hip;
            for (x, z, offset) in [
                (-1.0f32, -1.0f32, 0.0f32),
                (1.0, 1.0, 0.0),
                (1.0, -1.0, 0.5),
                (-1.0, 1.0, 0.5),
            ] {
                let leg = limb(
                    commands,
                    walker,
                    Vec3::new(a.legs.0 * x, a.hip, a.legs.1 * z),
                    0.38,
                    offset,
                    0.0,
                );
                // Upper leg (forearm or gaskin), then the slender cannon down to the hoof.
                part(
                    commands,
                    leg,
                    capsule(a.thigh, leg_len * 0.38),
                    fur,
                    at(0.0, -leg_len * 0.25, 0.0),
                );
                part(
                    commands,
                    leg,
                    capsule(a.shin, leg_len * 0.55),
                    fur,
                    at(0.0, -leg_len * 0.66, 0.0),
                );
                part(
                    commands,
                    leg,
                    Shape::Cylinder {
                        radius: a.shin * 1.5,
                        height: a.shin * 2.0,
                        segments: 10,
                    },
                    &look.dark,
                    at(0.0, -leg_len + a.shin, 0.0),
                );
            }
            // Deeper than wide.
            let (r, len, y) = a.barrel;
            part(
                commands,
                walker,
                capsule(r, len),
                fur,
                Transform::from_translation(DVec3::new(0.0, y as f64, 0.0))
                    .with_rotation(lying)
                    .with_scale(Vec3::new(0.85, 1.0, 1.12)),
            );
            for (r, len, p, tilt) in [a.neck, a.head] {
                part(commands, walker, capsule(r, len), fur, tilted(p, tilt));
            }
            let (r, len, p, tilt) = a.muzzle;
            part(
                commands,
                walker,
                capsule(r, len),
                if horse { fur } else { &look.dark },
                tilted(p, tilt),
            );
            let (r, h, p) = a.ears;
            for side in [-1.0f32, 1.0] {
                part(
                    commands,
                    walker,
                    Shape::Cone {
                        radius: r,
                        height: h,
                        segments: 8,
                    },
                    fur,
                    at(p.x * side, p.y, p.z),
                );
            }
            if horse {
                // Mane along the top of the neck.
                part(
                    commands,
                    walker,
                    Shape::Cuboid {
                        size: [0.05, 0.1, 0.72],
                    },
                    &look.dark,
                    tilted(Vec3::new(0.0, 0.62, -0.98), 0.95),
                );
            }
            let (r, len, p, tilt) = a.tail;
            part(
                commands,
                walker,
                capsule(r, len),
                if horse { &look.dark } else { fur },
                tilted(p, tilt),
            );
        }
        WalkerKind::Rover => {
            // A six-wheeled-looking rover reduced to four: chassis, wheels that roll with
            // the distance driven, solar deck and a camera mast. Origin 0.75 m up.
            part(
                commands,
                walker,
                Shape::Cuboid {
                    size: [1.5, 0.38, 2.7],
                },
                &look.hull,
                at(0.0, 0.0, 0.0),
            );
            part(
                commands,
                walker,
                Shape::Cuboid {
                    size: [1.9, 0.05, 1.7],
                },
                &look.panel,
                at(0.0, 0.24, 0.35),
            );
            for (x, z) in [(-0.85f32, 1.1f32), (0.85, 1.1), (-0.85, -1.1), (0.85, -1.1)] {
                let wheel = limb(commands, walker, Vec3::new(x, -0.45, z), 0.0, 0.0, 0.3);
                part(
                    commands,
                    wheel,
                    Shape::Cylinder {
                        radius: 0.3,
                        height: 0.26,
                        segments: 20,
                    },
                    &look.rubber,
                    Transform::default().with_rotation(Quat::from_rotation_z(FRAC_PI_2)),
                );
                part(
                    commands,
                    wheel,
                    Shape::Cuboid {
                        size: [0.28, 0.08, 0.36],
                    },
                    &look.hull,
                    at(0.0, 0.0, 0.0),
                );
                // Suspension arm from the chassis to the hub.
                part(
                    commands,
                    walker,
                    Shape::Cuboid {
                        size: [0.08, 0.08, 0.55],
                    },
                    &look.hull,
                    at(x * 0.88, -0.3, z * 0.78),
                );
            }
            part(
                commands,
                walker,
                Shape::Cylinder {
                    radius: 0.035,
                    height: 0.85,
                    segments: 10,
                },
                &look.hull,
                at(0.45, 0.62, -1.0),
            );
            part(
                commands,
                walker,
                Shape::Cuboid {
                    size: [0.32, 0.16, 0.18],
                },
                &look.hull,
                at(0.45, 1.1, -1.0),
            );
            part(
                commands,
                walker,
                Shape::Cone {
                    radius: 0.22,
                    height: 0.1,
                    segments: 16,
                },
                &look.hull,
                Transform::from_translation(DVec3::new(-0.5, 0.55, 1.0))
                    .with_rotation(Quat::from_rotation_x(-0.5)),
            );
        }
    }
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
    let look = Look::new();
    for (i, (material, kind, load)) in strips.into_iter().enumerate() {
        let offset = DVec3::new(-15.0 + i as f64 * 10.0, 0.0, 0.0);
        let c = origin + offset;
        if let Some(m) = material {
            // Overlapping spheres paint a rounded strip.
            for k in 0..7 {
                edit(
                    commands,
                    origin,
                    offset + DVec3::new(0.0, 0.0, -15.0 + k as f64 * 5.0),
                    EditShape::Sphere { radius: 5.2 },
                    EditOp::Paint,
                    Some(m),
                );
            }
        }
        let (body_height, step, speed) = match kind {
            WalkerKind::Person => (0.88, 0.72, 1.4),
            WalkerKind::Dog => (0.45, 0.55, 1.8),
            WalkerKind::Horse => (1.35, 1.1, 2.2),
            WalkerKind::Rover => (0.8, 0.3, 3.0),
        };
        let walker = commands
            .spawn((
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
            ))
            .id();
        if options.stand_in_bodies {
            stand_in_body(commands, walker, kind, body_height, &look);
        }
    }
    // A rover looping across all four strips.
    let rover = commands
        .spawn((
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
        ))
        .id();
    if options.stand_in_bodies {
        stand_in_body(commands, rover, WalkerKind::Rover, 0.75, &look);
    }

    // Terrain edits beyond the strips: a blast crater, a trench and a built-up mound.
    let far = DVec3::new(0.0, 0.0, -26.0);
    edit(
        commands,
        origin,
        far + DVec3::new(-12.0, 0.5, 0.0),
        EditShape::Sphere { radius: 4.5 },
        EditOp::Remove,
        None,
    );
    edit(
        commands,
        origin,
        far + DVec3::new(-12.0, 0.0, 0.0),
        EditShape::Sphere { radius: 6.5 },
        EditOp::Paint,
        Some("ash"),
    );
    edit(
        commands,
        origin,
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
        origin,
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
        origin,
        far + DVec3::new(15.0, -1.5, 0.0),
        EditShape::Sphere { radius: 3.5 },
        EditOp::Add,
        Some("dirt"),
    );
}
