//! The footstep trail: a loop with one stretch per kind of ground (grass, mud, snow, ice,
//! gravel, a wooden boardwalk, a metal grate, a concrete underpass that echoes, sand, fallen
//! leaves and puddles), walked round and round by a hiker, a runner who ignores slow motion, a
//! dog and a horse.
//!
//! Every footstep is a [`GroundContact`] like a real character's, so the trail tests the
//! whole path: contact → surface → ground state (wet, snowy, burnt) → synthesis → 3D mix.
//! Coloured rings on the ground show each sound as it plays.
//!
//! ```ignore
//! engine_audio::demo::spawn_demo(&mut commands, zone.center);
//! ```

use std::f64::consts::PI;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::physical::{Deformation, SurfaceMaterial};
use engine_world::{ContactKind, GroundContact, SurfaceMaterials};

use crate::{AudioCue, ReverbZone};

/// Half the length of the two straights, meters.
pub const HALF_STRAIGHT: f64 = 18.0;
/// Radius of the two bends, meters.
pub const BEND_RADIUS: f64 = 7.0;
const BEND: f64 = PI * BEND_RADIUS;
/// Once round the loop, meters.
pub const TRAIL_LENGTH: f64 = 4.0 * HALF_STRAIGHT + 2.0 * BEND;
/// Width of the path, meters.
pub const TRAIL_WIDTH: f32 = 2.6;

/// The stretches in walking order: surface material name and length in meters. The first
/// straight, the far bend, the second straight (boardwalk, grate, underpass, sand), the near
/// bend.
pub const STRETCHES: &[(&str, f64)] = &[
    ("grass", 12.0),
    ("mud", 12.0),
    ("snow", 12.0),
    ("ice", BEND * 0.5),
    ("gravel", BEND * 0.5),
    ("wood", 9.0),
    ("metal", 9.0),
    ("concrete", 9.0),
    ("sand", 9.0),
    ("leaves", BEND * 0.5),
    ("water", BEND * 0.5),
];

/// Where the trail is `along` meters from the start (relative to the trail's origin), and
/// which way it heads there.
pub fn trail_point(along: f64) -> (DVec3, Vec3) {
    let s = along.rem_euclid(TRAIL_LENGTH);
    let (h, r) = (HALF_STRAIGHT, BEND_RADIUS);
    if s < 2.0 * h {
        (DVec3::new(-h + s, 0.0, -r), Vec3::X)
    } else if s < 2.0 * h + BEND {
        let a = (s - 2.0 * h) / r;
        (
            DVec3::new(h + r * a.sin(), 0.0, -r * a.cos()),
            Vec3::new(a.cos() as f32, 0.0, a.sin() as f32),
        )
    } else if s < 4.0 * h + BEND {
        let t = s - 2.0 * h - BEND;
        (DVec3::new(h - t, 0.0, r), Vec3::NEG_X)
    } else {
        let a = (s - 4.0 * h - BEND) / r;
        (
            DVec3::new(-h - r * a.sin(), 0.0, r * a.cos()),
            Vec3::new(-a.cos() as f32, 0.0, -a.sin() as f32),
        )
    }
}

/// The surface material name `along` meters into the trail.
pub fn surface_at(along: f64) -> &'static str {
    let mut s = along.rem_euclid(TRAIL_LENGTH);
    for &(name, len) in STRETCHES {
        if s < len {
            return name;
        }
        s -= len;
    }
    STRETCHES[STRETCHES.len() - 1].0
}

/// The point on the trail's center line nearest to `offset` (relative to the trail's
/// origin, height ignored): how far along the trail it is and how far `offset` is from it
/// sideways, meters.
pub fn nearest_on_trail(offset: DVec3) -> (f64, f64) {
    let (h, r) = (HALF_STRAIGHT, BEND_RADIUS);
    let (x, z) = (offset.x, offset.z);
    if x > h {
        let a = (x - h).atan2(-z);
        (2.0 * h + a * r, ((x - h).hypot(z) - r).abs())
    } else if x < -h {
        let a = (-h - x).atan2(z);
        (4.0 * h + BEND + a * r, ((x + h).hypot(z) - r).abs())
    } else if z < 0.0 {
        (x + h, (z + r).abs())
    } else {
        (2.0 * h + BEND + (h - x), (z - r).abs())
    }
}

/// The surface material of the trail at `offset` from its origin, or `None` off the path.
/// Lets a landscape paint its ground to match the trail, so anything else walking it (the
/// player, NPCs, rain) finds the same surfaces as the trail's walkers.
pub fn surface_under(offset: DVec3) -> Option<&'static str> {
    let (along, side) = nearest_on_trail(offset);
    (side <= TRAIL_WIDTH as f64 * 0.5).then(|| surface_at(along))
}

/// Where each stretch starts, meters along the trail.
pub fn stretch_start(name: &str) -> Option<f64> {
    let mut s = 0.0;
    for &(n, len) in STRETCHES {
        if n == name {
            return Some(s);
        }
        s += len;
    }
    None
}

/// Someone (or something) walking the trail.
#[derive(Component, Debug, Clone)]
#[require(Transform)]
pub struct TrailWalker {
    /// The trail's origin.
    pub origin: DVec3,
    /// Distance along the loop, meters.
    pub along: f64,
    /// m/s.
    pub speed: f32,
    /// Distance between footfalls (any foot), meters.
    pub step_len: f32,
    /// 2 or 4.
    pub feet: u8,
    pub kind: ContactKind,
    pub mass_kg: f32,
    /// Half the distance between left and right feet, meters.
    pub stance: f32,
    /// Distance between front and hind feet (quadrupeds), meters.
    pub length: f32,
    /// Height of the root above the ground, meters.
    pub height: f32,
    next_foot: u8,
    to_next_step: f32,
}

impl TrailWalker {
    fn new(origin: DVec3, along: f64, kind: ContactKind, mass_kg: f32, speed: f32) -> Self {
        let (step_len, feet, stance, length, height) = match kind {
            ContactKind::Paw => (0.26, 4, 0.08, 0.5, 0.0),
            ContactKind::Hoof => (0.45, 4, 0.18, 1.3, 0.0),
            _ if speed > 2.5 => (1.3, 2, 0.1, 0.0, 0.0),
            _ => (0.72, 2, 0.12, 0.0, 0.0),
        };
        Self {
            origin,
            along,
            speed,
            step_len,
            feet,
            kind,
            mass_kg,
            stance,
            length,
            height,
            next_foot: 0,
            to_next_step: step_len * 0.5,
        }
    }
}

/// Coloured rings on the ground for the sounds the audio module plays: on and around footstep
/// trails, or everywhere.
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundRings {
    pub enabled: bool,
    /// Rings for every sound in the world, not just near trails.
    pub everywhere: bool,
    /// Trail origins (set by [`spawn_demo`]); rings show within 40 m of one.
    pub near: Vec<DVec3>,
}

#[derive(Component, Debug, Clone, Copy)]
struct SoundRing {
    age: f32,
    life: f32,
    size: f32,
    color: usize,
    level: usize,
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<SoundRings>()
        .register_resource::<SoundRings>(
            "Show a coloured ring on the ground for each sound as it plays (enabled): near \
             footstep trails, or everywhere.",
        )
        .add_systems(Update, walk_trail)
        .add_systems(
            PostUpdate,
            (spawn_rings, grow_rings)
                .chain()
                .in_set(UpdateSet::AfterPropagate),
        );
}

/// Build the footstep trail around `origin` (about 54 × 22 m) with its four walkers, and turn
/// on sound rings.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    commands.queue(|world: &mut World| {
        let mut materials = world.resource_mut::<SurfaceMaterials>();
        if materials.id("gravel").is_none() {
            materials.add(SurfaceMaterial {
                name: "gravel".into(),
                friction: 0.7,
                density: 1_800.0,
                absorbency: 0.15,
                deformation: Deformation::Granular { depth_m: 0.01 },
                footstep_sound: "footstep.gravel".into(),
                impact_effect: "impact.gravel".into(),
                ..Default::default()
            });
        }
    });
    commands.queue(move |world: &mut World| {
        let mut rings = world.resource_mut::<SoundRings>();
        rings.enabled = true;
        rings.near.push(origin);
    });

    let mut rng = Rng(0x7A11);
    let mut start = 0.0;
    for &(name, len) in STRETCHES {
        build_stretch(commands, origin, name, start, start + len, &mut rng);
        start += len;
    }
    underpass(commands, origin);

    // The walkers, spread round the loop.
    hiker(
        commands,
        TrailWalker::new(origin, 0.0, ContactKind::Foot, 75.0, 1.4),
        0xb3362c,
        None,
    );
    hiker(
        commands,
        TrailWalker::new(origin, 30.0, ContactKind::Foot, 68.0, 4.0),
        0x2c5fb3,
        Some(LocalTimeScale::unaffected_by_dilation(1.0)),
    );
    dog(
        commands,
        TrailWalker::new(origin, 60.0, ContactKind::Paw, 22.0, 1.8),
    );
    horse(
        commands,
        TrailWalker::new(origin, 86.0, ContactKind::Hoof, 500.0, 1.7),
    );
}

/// A tiny deterministic random source for prop placement.
struct Rng(u64);

impl Rng {
    fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
}

fn mat(hex: u32, roughness: f32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(roughness)
}

fn cuboid(x: f32, y: f32, z: f32) -> Shape {
    Shape::Cuboid { size: [x, y, z] }
}

fn put(commands: &mut Commands, shape: Shape, material: StandardMaterial, transform: Transform) {
    commands.spawn((transform, MeshRenderer::new(shape, material)));
}

/// Yaw that turns the -Z forward axis to `dir`.
fn yaw(dir: Vec3) -> Quat {
    Quat::from_rotation_y((-dir.x).atan2(-dir.z))
}

fn ground_material(name: &str) -> StandardMaterial {
    match name {
        "grass" => mat(0x6e8c3c, 0.95),
        "mud" => mat(0x3f2b1c, 0.22),
        "snow" => mat(0xf1f4f8, 0.85).with_subsurface(0.35),
        "ice" => mat(0xbfe2f0, 0.04).with_subsurface(0.2),
        "gravel" => mat(0x8a847c, 0.95),
        "wood" => mat(0x1d1712, 0.9),
        "metal" => mat(0x18191b, 0.6),
        "concrete" => mat(0xa9a59d, 0.8),
        "sand" => mat(0xd9c08b, 1.0),
        "leaves" => mat(0x4a3420, 0.9),
        "water" => mat(0x4e3c2b, 0.6),
        _ => mat(0x808080, 0.8),
    }
}

/// Lay the ground of one stretch (in pieces round the bends) and dress it with props.
fn build_stretch(
    commands: &mut Commands,
    origin: DVec3,
    name: &str,
    s0: f64,
    s1: f64,
    rng: &mut Rng,
) {
    let ground = ground_material(name);
    let mut s = s0;
    // Straights are laid in one piece, bends in pieces of up to a meter.
    let straights = [
        (0.0, 2.0 * HALF_STRAIGHT),
        (2.0 * HALF_STRAIGHT + BEND, 4.0 * HALF_STRAIGHT + BEND),
    ];
    while s < s1 - 1e-6 {
        let len = match straights.iter().find(|(a, b)| s >= *a && s < *b) {
            Some(&(_, b)) => b.min(s1) - s,
            None => (s1 - s).min(1.0),
        };
        let (pm, dm) = trail_point(s + len * 0.5);
        put(
            commands,
            cuboid(TRAIL_WIDTH, 0.06, len as f32 + 0.06),
            ground,
            Transform::from_translation(origin + pm + DVec3::new(0.0, 0.03, 0.0))
                .with_rotation(yaw(dm)),
        );
        s += len;
    }

    // A post at the start of each stretch with a cap in the colour of its sound rings.
    let (p, d) = trail_point(s0);
    let side = Vec3::new(-d.z, 0.0, d.x) * (TRAIL_WIDTH * 0.5 + 0.35);
    let post = origin + p + side.as_dvec3();
    put(
        commands,
        Shape::Cylinder {
            radius: 0.05,
            height: 1.1,
            segments: 10,
        },
        mat(0x5a4632, 0.8),
        Transform::from_translation(post + DVec3::new(0.0, 0.55, 0.0)),
    );
    let c = ring_color(name);
    put(
        commands,
        Shape::Sphere {
            radius: 0.12,
            segments: 16,
        },
        StandardMaterial::color(c[0], c[1], c[2]).with_emissive(c, 2.0),
        Transform::from_translation(post + DVec3::new(0.0, 1.15, 0.0)),
    );

    // Props along the stretch.
    let n = ((s1 - s0) * 1.0) as usize;
    let along = |rng: &mut Rng| s0 + (s1 - s0) * rng.f() as f64;
    let across = |rng: &mut Rng, w: f32| rng.range(-w, w);
    let at = |s: f64, x: f32, y: f64| {
        let (p, d) = trail_point(s);
        let right = Vec3::new(-d.z, 0.0, d.x);
        (
            origin + p + (right * x).as_dvec3() + DVec3::new(0.0, y, 0.0),
            d,
        )
    };
    match name {
        "grass" => {
            let blade = mat(0x5f8f35, 0.9).with_subsurface(0.4).with_wind_sway(0.08);
            for _ in 0..n * 14 {
                let (p, _) = at(along(rng), across(rng, TRAIL_WIDTH * 0.5), 0.12);
                let h = rng.range(0.12, 0.3);
                put(
                    commands,
                    Shape::Cone {
                        radius: 0.025,
                        height: h,
                        segments: 4,
                    },
                    blade,
                    Transform::from_translation(p + DVec3::new(0.0, h as f64 * 0.5 - 0.06, 0.0))
                        .with_rotation(Quat::from_rotation_y(rng.range(0.0, 6.0))),
                );
            }
        }
        "mud" => {
            let puddle = mat(0x2a2016, 0.05);
            for _ in 0..n / 2 {
                let (p, _) = at(along(rng), across(rng, 0.8), 0.062);
                let r = rng.range(0.15, 0.4);
                put(
                    commands,
                    Shape::Cylinder {
                        radius: r,
                        height: 0.01,
                        segments: 20,
                    },
                    puddle,
                    Transform::from_translation(p).with_scale(Vec3::new(
                        1.0,
                        1.0,
                        rng.range(0.5, 1.0),
                    )),
                );
            }
        }
        "snow" => {
            let snow = mat(0xf4f6fa, 0.85).with_subsurface(0.4);
            for _ in 0..n {
                let x = if rng.f() < 0.5 { -1.0 } else { 1.0 }
                    * (TRAIL_WIDTH * 0.5 + rng.range(0.1, 0.6));
                let (p, d) = at(along(rng), x, 0.0);
                let r = rng.range(0.3, 0.7);
                put(
                    commands,
                    Shape::Sphere {
                        radius: r,
                        segments: 16,
                    },
                    snow,
                    Transform::from_translation(p)
                        .with_rotation(yaw(d))
                        .with_scale(Vec3::new(1.0, rng.range(0.3, 0.5), 1.6)),
                );
            }
        }
        "ice" => {
            let crack = mat(0xe8f6fc, 0.3);
            for _ in 0..n {
                let (p, _) = at(along(rng), across(rng, 0.9), 0.061);
                put(
                    commands,
                    cuboid(rng.range(0.4, 1.0), 0.004, 0.015),
                    crack,
                    Transform::from_translation(p)
                        .with_rotation(Quat::from_rotation_y(rng.range(0.0, 3.1))),
                );
            }
        }
        "gravel" => {
            let stones = [mat(0x9a948c, 0.9), mat(0x6e6a65, 0.9), mat(0xb3ada3, 0.9)];
            for _ in 0..n * 40 {
                let (p, _) = at(along(rng), across(rng, TRAIL_WIDTH * 0.5), 0.065);
                let r = rng.range(0.015, 0.045);
                put(
                    commands,
                    Shape::Sphere {
                        radius: r,
                        segments: 6,
                    },
                    stones[(rng.f() * 3.0) as usize % 3],
                    Transform::from_translation(p).with_scale(Vec3::new(
                        1.0,
                        0.6,
                        rng.range(0.7, 1.3),
                    )),
                );
            }
        }
        "wood" => {
            // A boardwalk: planks across the path with gaps over the dark ground, and rails.
            let planks = [
                mat(0x8a6239, 0.75),
                mat(0x7a5530, 0.75),
                mat(0x946c42, 0.75),
            ];
            let mut s = s0 + 0.16;
            let mut i = 0;
            while s < s1 {
                let (p, d) = at(s, 0.0, 0.09);
                put(
                    commands,
                    cuboid(TRAIL_WIDTH - 0.1, 0.05, 0.28),
                    planks[i % 3],
                    Transform::from_translation(p).with_rotation(yaw(d)),
                );
                s += 0.32;
                i += 1;
            }
            rails(commands, origin, s0, s1, mat(0x6b4a2a, 0.8));
        }
        "metal" => {
            // A grate: steel bars across the path over the dark pit.
            let steel = mat(0x9aa0a8, 0.35).with_metallic(1.0);
            let mut s = s0 + 0.06;
            while s < s1 {
                let (p, d) = at(s, 0.0, 0.08);
                put(
                    commands,
                    cuboid(TRAIL_WIDTH - 0.1, 0.04, 0.04),
                    steel,
                    Transform::from_translation(p).with_rotation(yaw(d)),
                );
                s += 0.12;
            }
            for x in [-1.0f32, 1.0] {
                let (p, d) = at((s0 + s1) * 0.5, x * (TRAIL_WIDTH * 0.5 - 0.05), 0.09);
                put(
                    commands,
                    cuboid(0.08, 0.06, (s1 - s0) as f32),
                    steel,
                    Transform::from_translation(p).with_rotation(yaw(d)),
                );
            }
            rails(commands, origin, s0, s1, steel);
        }
        "sand" => {
            let ripple = mat(0xcdb27c, 1.0);
            let mut s = s0 + 0.3;
            while s < s1 {
                let (p, d) = at(s, rng.range(-0.2, 0.2), 0.062);
                put(
                    commands,
                    cuboid(TRAIL_WIDTH * rng.range(0.6, 0.9), 0.012, 0.06),
                    ripple,
                    Transform::from_translation(p)
                        .with_rotation(yaw(d) * Quat::from_rotation_y(rng.range(-0.15, 0.15))),
                );
                s += 0.35;
            }
        }
        "leaves" => {
            let leaf = [
                mat(0xc2581b, 0.7).with_subsurface(0.4),
                mat(0xd99a2b, 0.7).with_subsurface(0.4),
                mat(0x8c2f1a, 0.7).with_subsurface(0.4),
                mat(0xa67c2e, 0.7).with_subsurface(0.4),
            ];
            for _ in 0..n * 45 {
                let (p, _) = at(along(rng), across(rng, TRAIL_WIDTH * 0.5 + 0.3), 0.065);
                put(
                    commands,
                    cuboid(rng.range(0.06, 0.12), 0.006, rng.range(0.04, 0.08)),
                    leaf[(rng.f() * 4.0) as usize % 4],
                    Transform::from_translation(p).with_rotation(
                        Quat::from_rotation_y(rng.range(0.0, 6.3))
                            * Quat::from_rotation_x(rng.range(-0.3, 0.3)),
                    ),
                );
            }
        }
        "water" => {
            let water = mat(0x2b3a44, 0.02).with_metallic(0.1);
            let mut s = s0 + 0.4;
            while s < s1 - 0.3 {
                let (p, d) = at(s, rng.range(-0.5, 0.5), 0.062);
                let r = rng.range(0.4, 0.9);
                put(
                    commands,
                    Shape::Cylinder {
                        radius: r,
                        height: 0.012,
                        segments: 28,
                    },
                    water,
                    Transform::from_translation(p)
                        .with_rotation(yaw(d))
                        .with_scale(Vec3::new(1.0, 1.0, rng.range(1.0, 1.6))),
                );
                s += r as f64 * 1.6;
            }
        }
        _ => {}
    }
}

/// Posts and a handrail on both sides of a stretch.
fn rails(commands: &mut Commands, origin: DVec3, s0: f64, s1: f64, material: StandardMaterial) {
    for x in [-1.0f32, 1.0] {
        let off = x * (TRAIL_WIDTH * 0.5 + 0.05);
        let mut s = s0 + 0.1;
        while s <= s1 {
            let (p, d) = trail_point(s);
            let right = Vec3::new(-d.z, 0.0, d.x) * off;
            put(
                commands,
                cuboid(0.07, 0.95, 0.07),
                material,
                Transform::from_translation(
                    origin + p + right.as_dvec3() + DVec3::new(0.0, 0.48, 0.0),
                ),
            );
            s += 1.5;
        }
        let (p, d) = trail_point((s0 + s1) * 0.5);
        let right = Vec3::new(-d.z, 0.0, d.x) * off;
        put(
            commands,
            cuboid(0.06, 0.05, (s1 - s0) as f32),
            material,
            Transform::from_translation(origin + p + right.as_dvec3() + DVec3::new(0.0, 0.95, 0.0))
                .with_rotation(yaw(d)),
        );
    }
}

/// The concrete stretch runs through an underpass that echoes: a wall on the inside of the
/// loop, pillars on the outside (so the camera can see in) and a slab roof.
fn underpass(commands: &mut Commands, origin: DVec3) {
    let Some(s0) = stretch_start("concrete") else {
        return;
    };
    let s1 = s0 + 9.0;
    let (mid, d) = trail_point((s0 + s1) * 0.5);
    let right = Vec3::new(-d.z, 0.0, d.x);
    let concrete = mat(0x9d998f, 0.85);
    let dark = mat(0x6f6b64, 0.9);
    let center = origin + mid;
    let rot = yaw(d);
    let half_w = TRAIL_WIDTH as f64 * 0.5 + 0.9;
    let h: f64 = 3.0;
    // Toward the loop's middle is "inside".
    let inside = if (center - origin).dot(right.as_dvec3()) > 0.0 {
        -1.0
    } else {
        1.0
    };
    put(
        commands,
        cuboid(0.4, h as f32, 9.4),
        concrete,
        Transform::from_translation(
            center + (right * (inside * half_w as f32)).as_dvec3() + DVec3::new(0.0, h * 0.5, 0.0),
        )
        .with_rotation(rot),
    );
    put(
        commands,
        cuboid(2.0 * half_w as f32 + 0.8, 0.45, 9.8),
        dark,
        Transform::from_translation(center + DVec3::new(0.0, h + 0.22, 0.0)).with_rotation(rot),
    );
    for k in 0..4 {
        let s = s0 + 0.3 + k as f64 * (8.4 / 3.0);
        let (p, _) = trail_point(s);
        put(
            commands,
            cuboid(0.45, h as f32, 0.45),
            concrete,
            Transform::from_translation(
                origin
                    + p
                    + (right * (-inside * half_w as f32)).as_dvec3()
                    + DVec3::new(0.0, h * 0.5, 0.0),
            )
            .with_rotation(rot),
        );
    }
    commands.spawn((
        Name::new("Underpass echo"),
        Transform::from_translation(center + DVec3::new(0.0, h * 0.5, 0.0)).with_rotation(rot),
        ReverbZone {
            half_extents: Vec3::new(half_w as f32, h as f32 * 0.5 + 0.3, 4.7),
            room: 0.55,
            damping: 0.15,
            wet: 0.42,
            fade_m: 1.2,
        },
    ));
}

fn body_part(
    parent: &mut ChildSpawnerCommands,
    shape: Shape,
    material: StandardMaterial,
    t: Transform,
) {
    parent.spawn((t, MeshRenderer::new(shape, material)));
}

fn hiker(commands: &mut Commands, walker: TrailWalker, jacket: u32, scale: Option<LocalTimeScale>) {
    let jacket = mat(jacket, 0.7);
    let skin = mat(0xc99a78, 0.6).with_subsurface(0.5);
    let trousers = mat(0x2f3338, 0.85);
    let mut e = commands.spawn((Name::new("Trail walker"), walker));
    if let Some(s) = scale {
        e.insert(s);
    }
    e.with_children(|p| {
        body_part(
            p,
            Shape::Capsule {
                radius: 0.2,
                height: 0.75,
                segments: 16,
            },
            jacket,
            Transform::from_xyz(0.0, 1.22, 0.0),
        );
        body_part(
            p,
            Shape::Sphere {
                radius: 0.12,
                segments: 16,
            },
            skin,
            Transform::from_xyz(0.0, 1.72, 0.0),
        );
        for x in [-0.1, 0.1] {
            body_part(
                p,
                Shape::Capsule {
                    radius: 0.075,
                    height: 0.9,
                    segments: 10,
                },
                trousers,
                Transform::from_xyz(x, 0.47, 0.0),
            );
        }
        body_part(
            p,
            cuboid(0.3, 0.4, 0.16),
            mat(0x3c4a2e, 0.8),
            Transform::from_xyz(0.0, 1.28, 0.2),
        );
    });
}

fn dog(commands: &mut Commands, walker: TrailWalker) {
    let fur = mat(0x8b5a2b, 0.85).with_subsurface(0.2);
    commands
        .spawn((Name::new("Trail dog"), walker))
        .with_children(|p| {
            body_part(
                p,
                Shape::Capsule {
                    radius: 0.14,
                    height: 0.7,
                    segments: 14,
                },
                fur,
                Transform::from_xyz(0.0, 0.42, 0.0)
                    .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            );
            body_part(
                p,
                Shape::Sphere {
                    radius: 0.11,
                    segments: 14,
                },
                fur,
                Transform::from_xyz(0.0, 0.58, -0.4),
            );
            body_part(
                p,
                cuboid(0.08, 0.07, 0.14),
                mat(0x5c3a1b, 0.8),
                Transform::from_xyz(0.0, 0.55, -0.52),
            );
            for (x, z) in [(-0.08, -0.22), (0.08, -0.22), (-0.08, 0.22), (0.08, 0.22)] {
                body_part(
                    p,
                    Shape::Cylinder {
                        radius: 0.035,
                        height: 0.36,
                        segments: 8,
                    },
                    fur,
                    Transform::from_xyz(x, 0.18, z),
                );
            }
            body_part(
                p,
                Shape::Cylinder {
                    radius: 0.025,
                    height: 0.3,
                    segments: 6,
                },
                fur,
                Transform::from_xyz(0.0, 0.55, 0.42).with_rotation(Quat::from_rotation_x(-0.8)),
            );
        });
}

fn horse(commands: &mut Commands, walker: TrailWalker) {
    let coat = mat(0x6b4423, 0.6).with_subsurface(0.15);
    let mane = mat(0x1f1712, 0.9);
    commands
        .spawn((Name::new("Trail horse"), walker))
        .with_children(|p| {
            body_part(
                p,
                Shape::Capsule {
                    radius: 0.38,
                    height: 1.8,
                    segments: 18,
                },
                coat,
                Transform::from_xyz(0.0, 1.3, 0.0)
                    .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            );
            body_part(
                p,
                cuboid(0.22, 0.75, 0.3),
                coat,
                Transform::from_xyz(0.0, 1.75, -0.8).with_rotation(Quat::from_rotation_x(-0.5)),
            );
            body_part(
                p,
                cuboid(0.2, 0.22, 0.55),
                coat,
                Transform::from_xyz(0.0, 2.05, -1.1).with_rotation(Quat::from_rotation_x(0.35)),
            );
            body_part(
                p,
                cuboid(0.06, 0.6, 0.12),
                mane,
                Transform::from_xyz(0.0, 1.95, -0.7).with_rotation(Quat::from_rotation_x(-0.5)),
            );
            body_part(
                p,
                Shape::Cylinder {
                    radius: 0.05,
                    height: 0.7,
                    segments: 6,
                },
                mane,
                Transform::from_xyz(0.0, 1.2, 0.95).with_rotation(Quat::from_rotation_x(0.4)),
            );
            for (x, z) in [(-0.2, -0.62), (0.2, -0.62), (-0.2, 0.62), (0.2, 0.62)] {
                body_part(
                    p,
                    Shape::Cylinder {
                        radius: 0.07,
                        height: 1.05,
                        segments: 8,
                    },
                    coat,
                    Transform::from_xyz(x, 0.53, z),
                );
                body_part(
                    p,
                    Shape::Cylinder {
                        radius: 0.08,
                        height: 0.08,
                        segments: 8,
                    },
                    mane,
                    Transform::from_xyz(x, 0.04, z),
                );
            }
        });
}

/// Move the walkers along the trail and put their feet down.
fn walk_trail(
    time: Res<Time>,
    materials: Res<SurfaceMaterials>,
    mut walkers: Query<(
        Entity,
        &mut TrailWalker,
        &mut Transform,
        Option<&LocalTimeScale>,
    )>,
    mut contacts: MessageWriter<GroundContact>,
) {
    for (entity, mut w, mut t, scale) in &mut walkers {
        let dt = time.delta_for(scale);
        if dt <= 0.0 {
            continue;
        }
        let moved = w.speed * dt;
        w.along = (w.along + moved as f64).rem_euclid(TRAIL_LENGTH);
        let (p, d) = trail_point(w.along);
        let right = Vec3::new(-d.z, 0.0, d.x);
        // A little bob with each step.
        let phase = 1.0 - w.to_next_step / w.step_len;
        let bob = if w.feet == 2 {
            0.025 * (phase * std::f32::consts::PI).sin()
        } else {
            0.0
        };
        t.translation = w.origin + p + DVec3::new(0.0, (w.height + bob) as f64 + 0.06, 0.0);
        t.rotation = yaw(d);

        w.to_next_step -= moved;
        while w.to_next_step <= 0.0 {
            w.to_next_step += w.step_len;
            let foot = w.next_foot;
            w.next_foot = (foot + 1) % w.feet;
            // Feet: left, right for two legs; left fore, right hind, right fore, left hind
            // (a walking quadruped's footfall order) for four.
            let (side, fore) = match (w.feet, foot) {
                (2, 0) => (-1.0, 0.0),
                (2, _) => (1.0, 0.0),
                (_, 0) => (-1.0, 0.5),
                (_, 1) => (1.0, -0.5),
                (_, 2) => (1.0, 0.5),
                _ => (-1.0, -0.5),
            };
            let foot_s = w.along + (w.to_next_step - w.step_len) as f64 + (fore * w.length) as f64;
            let (fp, fd) = trail_point(foot_s);
            let surface = surface_at(foot_s);
            let position =
                w.origin + fp + (right * side * w.stance).as_dvec3() + DVec3::new(0.0, 0.06, 0.0);
            let load = if w.feet == 2 {
                w.mass_kg
            } else {
                w.mass_kg * 0.5
            };
            contacts.write(GroundContact {
                position,
                normal: Vec3::Y,
                size: match w.kind {
                    ContactKind::Hoof => [0.13, 0.15],
                    ContactKind::Paw => [0.05, 0.06],
                    _ => [0.1, 0.27],
                },
                yaw: (-fd.x).atan2(-fd.z),
                load_kg: load,
                speed: w.speed,
                kind: w.kind,
                surface: materials.id(surface),
                entity: Some(entity),
            });
        }
    }
}

const RING_LEVELS: usize = 4;

fn ring_color(name: &str) -> [f32; 3] {
    let key = name.rsplit('.').next().unwrap_or(name);
    match key {
        "grass" => [0.35, 1.0, 0.3],
        "mud" => [0.85, 0.5, 0.2],
        "snow" => [0.85, 0.92, 1.0],
        "ice" => [0.3, 0.9, 1.0],
        "gravel" => [0.75, 0.72, 0.68],
        "wood" => [1.0, 0.62, 0.2],
        "metal" => [0.55, 0.65, 1.0],
        "concrete" => [0.9, 0.9, 0.85],
        "sand" => [1.0, 0.85, 0.4],
        "leaves" => [1.0, 0.4, 0.1],
        "water" => [0.2, 0.55, 1.0],
        "explosion" => [1.0, 0.25, 0.1],
        _ => [1.0, 1.0, 1.0],
    }
}

const RING_NAMES: &[&str] = &[
    "grass",
    "mud",
    "snow",
    "ice",
    "gravel",
    "wood",
    "metal",
    "concrete",
    "sand",
    "leaves",
    "water",
    "explosion",
    "other",
];

fn ring_material(color: usize, level: usize) -> StandardMaterial {
    let c = ring_color(RING_NAMES[color.min(RING_NAMES.len() - 1)]);
    let k = (RING_LEVELS - level) as f32 / RING_LEVELS as f32;
    StandardMaterial::color(c[0] * 0.2, c[1] * 0.2, c[2] * 0.2).with_emissive(c, 6.0 * k * k)
}

fn ring_shape() -> Shape {
    Shape::Torus {
        radius: 1.0,
        tube: 0.03,
        segments: 48,
    }
}

fn spawn_rings(mut commands: Commands, rings: Res<SoundRings>, mut cues: MessageReader<AudioCue>) {
    if !rings.enabled {
        cues.clear();
        return;
    }
    for cue in cues.read() {
        let Some(p) = cue.position else { continue };
        if !rings.everywhere && !rings.near.iter().any(|o| o.distance(p) < 40.0) {
            continue;
        }
        let key = cue.name.rsplit('.').next().unwrap_or(&cue.name);
        let color = RING_NAMES
            .iter()
            .position(|n| *n == key)
            .unwrap_or(RING_NAMES.len() - 1);
        let size = 0.35 + 1.6 * cue.loudness.clamp(0.0, 1.5);
        commands.spawn((
            Name::new("Sound ring"),
            SoundRing {
                age: 0.0,
                life: 0.45 + 0.4 * cue.loudness.min(1.0),
                size,
                color,
                level: 0,
            },
            Transform::from_translation(p + DVec3::new(0.0, 0.03, 0.0)).with_uniform_scale(0.05),
            MeshRenderer::new(ring_shape(), ring_material(color, 0)),
        ));
    }
}

fn grow_rings(
    mut commands: Commands,
    time: Res<Time>,
    mut rings: Query<(Entity, &mut SoundRing, &mut Transform, &mut MeshRenderer)>,
) {
    let dt = time.delta_secs();
    for (e, mut r, mut t, mut mesh) in &mut rings {
        r.age += dt;
        let k = r.age / r.life;
        if k >= 1.0 {
            commands.entity(e).despawn();
            continue;
        }
        let grow = 1.0 - (1.0 - k).powi(3);
        let s = r.size * (0.08 + 0.92 * grow);
        t.scale = Vec3::new(s, 1.0, s);
        let level = ((k * RING_LEVELS as f32) as usize).min(RING_LEVELS - 1);
        if level != r.level {
            r.level = level;
            mesh.material = ring_material(r.color, level).into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trail_is_a_closed_loop() {
        let (a, da) = trail_point(0.0);
        let (b, db) = trail_point(TRAIL_LENGTH - 1e-6);
        assert!(a.distance(b) < 1e-3, "{a} vs {b}");
        assert!(da.distance(db) < 1e-3);
        // Continuous: no jumps anywhere.
        let mut last = trail_point(0.0).0;
        let mut s = 0.05;
        while s < TRAIL_LENGTH {
            let p = trail_point(s).0;
            assert!(p.distance(last) < 0.051, "jump at {s}");
            last = p;
            s += 0.05;
        }
        let total: f64 = STRETCHES.iter().map(|s| s.1).sum();
        assert!((total - TRAIL_LENGTH).abs() < 1e-9);
    }

    #[test]
    fn stretches_are_where_they_say() {
        assert_eq!(surface_at(1.0), "grass");
        assert_eq!(surface_at(13.0), "mud");
        assert_eq!(surface_at(25.0), "snow");
        assert_eq!(
            surface_at(stretch_start("concrete").unwrap() + 1.0),
            "concrete"
        );
        assert_eq!(surface_at(TRAIL_LENGTH - 0.5), "water");
        assert_eq!(surface_at(TRAIL_LENGTH + 1.0), "grass");
    }

    #[test]
    fn nearest_point_matches_the_path() {
        let mut s = 0.0;
        while s < TRAIL_LENGTH {
            let (p, d) = trail_point(s);
            let side = Vec3::new(-d.z, 0.0, d.x).as_dvec3();
            for off in [-1.0, 0.0, 0.9] {
                let (along, dist) = nearest_on_trail(p + side * off);
                let gap = (along - s).abs().min(TRAIL_LENGTH - (along - s).abs());
                assert!(
                    gap < 1e-6 && (dist - off.abs()).abs() < 1e-6,
                    "s {s} off {off}: {along} {dist}"
                );
            }
            assert_eq!(surface_under(p), Some(surface_at(s + 1e-9)));
            s += 0.37;
        }
        assert_eq!(surface_under(DVec3::ZERO), None);
        assert_eq!(surface_under(DVec3::new(0.0, 0.0, -7.0 - 2.0)), None);
    }
}
