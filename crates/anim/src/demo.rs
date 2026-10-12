//! The animation paddock: a showcase of every animation feature, spawned around one point.
//!
//! * **Muscle stage**: an athlete swings a sledgehammer (arms, shoulders and back tense on the
//!   strike, then relax; veins show on the forearms; the ponytail whips), braces in a block,
//!   and hits a double-biceps flex. Next to them, a man in a long-sleeved shirt reaches
//!   forward: the sleeves bunch at the elbows, then the shirt stretches across his back.
//! * **Step course**: a walker climbs a ramp, crosses a deck, walks down stairs (feet planted
//!   by foot IK), then runs back on the flat, leaning into the turns.
//! * **Horse track**: a horse walks, trots, canters and gallops around an oval; its haunches
//!   and shoulders flex with every stride, its tail streams behind.
//! * **Spider garden**: a giant spider picks its way over rocks with eight procedural legs.
//! * **Car loop**: a car drives over speed bumps (springy suspension) and rolls in the corners
//!   while its front wheels steer and every wheel spins.
//!
//! Call [`spawn_demo`] with the zone center. Everything is generated from code.

use std::f32::consts::PI;

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape as MeshShape, StandardMaterial};
use engine_world::Ground;
use engine_world::animation::AnimationParams;
use engine_world::physics::PhysicsQueries;
use serde_json::{Value, json};

use crate::animator::{AnimationLibrary, Animator, AnimatorSettings};
use crate::attach::BoneAttachment;
use crate::build::{car, horse, humanoid, spider};
use crate::ik::{GroundProbe, IkShape, IkSurface, IkSurfaces, LookAt, Reach, ReachIk};
use crate::muscle::{MorphWeights, MuscleRig};
use crate::procedural::spring::SpringChains;
use crate::skin::{SkinMaterial, SkinnedMesh, SkinnedMeshRenderer};
use crate::xform::smoothing;

/// Radius of the paddock, meters.
pub const RADIUS: f64 = 36.0;

/// Where the hammer athlete stands (local x, z) and how far ahead of them the tire lies.
const ATHLETE: DVec3 = DVec3::new(-2.6, 0.0, 5.4);
const TIRE_AHEAD: f64 = 1.3;
/// The hammer in the right hand's frame: the fist's center, and the handle's direction toward
/// the head (perpendicular to the forearm, tipped down a little, as a hand holds a hammer).
const GRIP: Vec3 = Vec3::new(0.0, -0.085, 0.0);
const HANDLE_DIR: Vec3 = Vec3::new(0.0, -0.34, -0.94);

/// The main entities of a spawned demo (for cameras, tests and tools).
#[derive(Debug, Clone, Copy)]
pub struct DemoEntities {
    pub athlete: Entity,
    pub lifter: Entity,
    pub reacher: Entity,
    pub walker: Entity,
    pub horse: Entity,
    pub spider: Entity,
    pub car: Entity,
}

/// Follows a closed, smooth path through `waypoints` (`[x, z, speed]`, meters and m/s,
/// relative to `origin`), standing on whatever ground is there. Sets the `speed`, `turn`
/// and `move_effort` animation parameters.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoPath {
    pub origin: DVec3,
    pub waypoints: Vec<[f32; 3]>,
    /// Meters traveled along the loop.
    pub distance: f32,
    /// Speed multiplier.
    pub speed: f32,
    /// Speed at which `move_effort` reaches 1, m/s.
    pub effort_speed: f32,
    #[serde(skip)]
    samples: Vec<(Vec2, f32, f32)>,
    #[serde(skip)]
    yaw_rate: f32,
    #[serde(skip)]
    started: bool,
}

impl Default for DemoPath {
    fn default() -> Self {
        Self {
            origin: DVec3::ZERO,
            waypoints: Vec::new(),
            distance: 0.0,
            speed: 1.0,
            effort_speed: humanoid::RUN_SPEED,
            samples: Vec::new(),
            yaw_rate: 0.0,
            started: false,
        }
    }
}

impl DemoPath {
    pub fn new(origin: DVec3, waypoints: Vec<[f32; 3]>) -> Self {
        Self {
            origin,
            waypoints,
            ..Default::default()
        }
    }

    /// Length of the loop, meters.
    pub fn length(&mut self) -> f32 {
        self.build();
        self.samples.last().map_or(0.0, |s| s.1)
    }

    fn build(&mut self) {
        if !self.samples.is_empty() || self.waypoints.len() < 3 {
            return;
        }
        let n = self.waypoints.len();
        let p = |i: usize| {
            let w = self.waypoints[i % n];
            Vec2::new(w[0], w[1])
        };
        let mut total = 0.0;
        let mut prev: Option<Vec2> = None;
        for i in 0..n {
            let (p0, p1, p2, p3) = (p(i + n - 1), p(i), p(i + 1), p(i + 2));
            let (s1, s2) = (self.waypoints[i][2], self.waypoints[(i + 1) % n][2]);
            for k in 0..24 {
                let t = k as f32 / 24.0;
                // Centripetal-ish Catmull-Rom (uniform is fine for evenly spaced points).
                let t2 = t * t;
                let t3 = t2 * t;
                let q = 0.5
                    * ((2.0 * p1)
                        + (p2 - p0) * t
                        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3);
                if let Some(pr) = prev {
                    total += (q - pr).length();
                }
                prev = Some(q);
                let st = t * t * (3.0 - 2.0 * t);
                self.samples.push((q, total, s1 + (s2 - s1) * st));
            }
        }
        // Close the loop.
        if let (Some(first), Some(last)) = (self.samples.first().copied(), prev) {
            total += (first.0 - last).length();
            self.samples.push((first.0, total, first.2));
        }
    }

    /// Position, direction and speed at a distance along the loop.
    fn at(&self, distance: f32) -> (Vec2, Vec2, f32) {
        let len = self.samples.last().map_or(0.0, |s| s.1).max(1e-3);
        let d = distance.rem_euclid(len);
        let i = self
            .samples
            .partition_point(|s| s.1 <= d)
            .clamp(1, self.samples.len() - 1);
        let (a, b) = (self.samples[i - 1], self.samples[i]);
        let t = ((d - a.1) / (b.1 - a.1).max(1e-6)).clamp(0.0, 1.0);
        let dir = (b.0 - a.0).normalize_or(Vec2::new(0.0, -1.0));
        (a.0.lerp(b.0, t), dir, a.2 + (b.2 - a.2) * t)
    }
}

/// Fires triggers and sets parameters on a timeline that repeats every `period` seconds.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoScript {
    pub period: f32,
    pub steps: Vec<DemoStep>,
    pub time: f32,
}

/// One step of a [`DemoScript`]: a trigger and/or a parameter value at `at` seconds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoStep {
    pub at: f32,
    pub trigger: String,
    pub param: String,
    pub value: f32,
}

impl DemoStep {
    pub fn trigger(at: f32, name: &str) -> Self {
        Self {
            at,
            trigger: name.into(),
            ..Default::default()
        }
    }
    pub fn param(at: f32, name: &str, value: f32) -> Self {
        Self {
            at,
            param: name.into(),
            value,
            ..Default::default()
        }
    }
}

pub(crate) fn register(app: &mut App) {
    {
        let mut lib = app.world.resource_mut::<AnimationLibrary>();
        let human = lib.add_skeleton(humanoid::skeleton());
        for clip in humanoid::clips(&human) {
            lib.add_clip(clip);
        }
        lib.add_graph(humanoid::graph());
        let h = lib.add_skeleton(horse::skeleton());
        for clip in horse::clips(&h) {
            lib.add_clip(clip);
        }
        lib.add_graph(horse::graph());
        lib.add_skeleton(spider::skeleton());
        lib.add_skeleton(car::skeleton());
    }
    app.register_component::<DemoPath>(
        "Demo mover: follows a closed path of [x, z, speed] waypoints (m, m/s) on the ground.",
    )
    .register_component::<DemoScript>(
        "Demo timeline: triggers and parameter values at times (s), repeating every period.",
    )
    .add_systems(
        Update,
        (demo_path_system, demo_script_system).in_set(UpdateSet::Logic),
    )
    .register_api(
        "anim.spawn",
        "Spawn a demo character: athlete, shirt, lifter, horse, spider or car.",
        r#"{"kind": "horse", "at": [0, 0, 0], "yaw_deg": 0}"#,
        |world, args| {
            let kind = args
                .get("kind")
                .and_then(Value::as_str)
                .ok_or("pass \"kind\": athlete, shirt, lifter, horse, spider or car")?;
            let at = args
                .get("at")
                .and_then(|v| serde_json::from_value::<[f64; 3]>(v.clone()).ok())
                .unwrap_or([0.0; 3]);
            let yaw = args.get("yaw_deg").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let t = Transform::from_translation(DVec3::from(at))
                .with_rotation(Quat::from_rotation_y(yaw.to_radians()));
            let e = match kind {
                "athlete" => spawn_humanoid(world, humanoid::Outfit::Athlete, t),
                "shirt" => spawn_humanoid(world, humanoid::Outfit::Shirt, t),
                "lifter" => spawn_humanoid(world, humanoid::Outfit::Lifter, t),
                "horse" => spawn_horse(world, t),
                "spider" => spawn_spider(world, t),
                "car" => spawn_car(world, t, [0.55, 0.04, 0.02]),
                other => {
                    return Err(format!(
                        "unknown kind {other}; use athlete, shirt, lifter, horse, spider or car"
                    ));
                }
            };
            Ok(json!({ "id": e.to_bits() }))
        },
    )
    .register_api(
        "anim.demo",
        "Spawn the whole animation paddock around a point.",
        r#"{"at": [0, 0, 0]}"#,
        |world, args| {
            let at = args
                .get("at")
                .and_then(|v| serde_json::from_value::<[f64; 3]>(v.clone()).ok())
                .unwrap_or([0.0; 3]);
            let d = spawn_demo_world(world, DVec3::from(at));
            Ok(json!({
                "athlete": d.athlete.to_bits(),
                "lifter": d.lifter.to_bits(),
                "reacher": d.reacher.to_bits(),
                "walker": d.walker.to_bits(),
                "horse": d.horse.to_bits(),
                "spider": d.spider.to_bits(),
                "car": d.car.to_bits(),
            }))
        },
    );
}

/// Spawn the paddock around `origin` (the showcase zone center).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    commands.queue(move |world: &mut World| {
        spawn_demo_world(world, origin);
    });
}

fn mesh_handle(
    world: &mut World,
    key: &str,
    build: impl FnOnce() -> SkinnedMesh,
) -> engine_assets::Handle<SkinnedMesh> {
    world
        .get_resource_or_init::<Assets<SkinnedMesh>>()
        .get_or_insert_keyed(key, build)
}

fn animator(world: &World, skeleton: &str, graph: &str) -> Animator {
    let lib = world.resource::<AnimationLibrary>();
    lib.instantiate(skeleton, graph).unwrap_or_else(|e| {
        tracing::error!("demo animator: {e}");
        Animator::procedural(
            lib.skeletons
                .get(skeleton)
                .cloned()
                .expect("demo skeletons are registered by AnimationPlugin"),
        )
    })
}

fn morphs(world: &World, handle: engine_assets::Handle<SkinnedMesh>) -> MorphWeights {
    let names = world
        .resource::<Assets<SkinnedMesh>>()
        .get(handle)
        .map(|m| m.morph_names())
        .unwrap_or_else(|| Vec::new().into());
    MorphWeights::new(names)
}

/// A person (athlete or shirt) with locomotion graph, muscles, foot IK (and a ponytail).
pub fn spawn_humanoid(world: &mut World, outfit: humanoid::Outfit, transform: Transform) -> Entity {
    let (key, name, material) = match outfit {
        humanoid::Outfit::Athlete => ("anim/humanoid/athlete", "Athlete", SkinMaterial::default()),
        humanoid::Outfit::Lifter => ("anim/humanoid/lifter", "Lifter", SkinMaterial::default()),
        humanoid::Outfit::Shirt => (
            "anim/humanoid/shirt",
            "Shirt",
            SkinMaterial {
                wrinkles: 1.4,
                bunch: 0.016,
                ..Default::default()
            },
        ),
    };
    let handle = mesh_handle(world, key, || humanoid::mesh(outfit));
    let anim = animator(world, humanoid::SKELETON, humanoid::GRAPH);
    let morphs = morphs(world, handle);
    let mut e = world.spawn((
        Name::new(name),
        transform,
        GlobalTransform::from(transform),
        AnimatorSettings::new(humanoid::SKELETON, humanoid::GRAPH),
        anim,
        AnimationParams::default(),
        SkinnedMeshRenderer::new(handle, material),
        morphs,
        MuscleRig::new(humanoid::muscles()),
        humanoid::foot_ik(),
    ));
    if outfit == humanoid::Outfit::Athlete {
        e.insert(SpringChains::new(vec![humanoid::ponytail()]));
    }
    e.id()
}

/// A sledgehammer in `person`'s right hand. The left hand grips the end of the handle while a
/// clip's `grip` curve is on (two-handed swings).
pub fn spawn_hammer(world: &mut World, person: Entity, near: DVec3) -> [Entity; 2] {
    let dir = HANDLE_DIR.normalize();
    let wood = StandardMaterial::hex(0x9b7a52).with_roughness(0.7);
    let steel = StandardMaterial::hex(0x55575c)
        .with_metallic(1.0)
        .with_roughness(0.4);
    let parts = [
        (
            MeshShape::Cylinder {
                radius: 0.018,
                height: 0.85,
                segments: 12,
            },
            wood,
            GRIP + dir * 0.225,
            Quat::from_rotation_arc(Vec3::Y, dir),
        ),
        (
            MeshShape::Cuboid {
                size: [0.1, 0.26, 0.1],
            },
            steel,
            GRIP + dir * 0.6,
            Quat::from_rotation_arc(Vec3::Z, dir),
        ),
    ];
    let ids = parts.map(|(shape, material, offset, rotation)| {
        let t = Transform::from_translation(near);
        world
            .spawn((
                t,
                GlobalTransform::from(t),
                MeshRenderer::new(shape, material),
                BoneAttachment::new(person, "hand_r").with_offset(offset, rotation),
            ))
            .id()
    });
    world.entity_mut(person).insert(ReachIk::new(vec![Reach {
        target_bone: "hand_r".into(),
        // The left wrist sits where the left fist closes around the handle's end.
        target_offset: -dir * 0.11,
        pole: Vec3::new(-0.5, -1.0, 0.3),
        weight_curve: "grip".into(),
        ..Reach::arm(0)
    }]));
    ids
}

/// The point on the tire's near rim the hammer should hit, for a paddock at `origin`.
pub fn tire_strike_point(origin: DVec3) -> DVec3 {
    origin + DVec3::new(ATHLETE.x, 0.72, ATHLETE.z + TIRE_AHEAD - 0.45)
}

/// Where the hammer head is, in world space (for tests and cameras).
pub fn hammer_head(world: &World, person: Entity) -> Option<DVec3> {
    let animator = world.get::<Animator>(person)?;
    let gt = world.get::<GlobalTransform>(person)?;
    let hand = animator.skeleton.index_of("hand_r")?;
    let local = animator.model[hand].transform_point(GRIP + HANDLE_DIR.normalize() * 0.6);
    Some(crate::ik::to_world(gt, local))
}

pub fn spawn_horse(world: &mut World, transform: Transform) -> Entity {
    let handle = mesh_handle(world, "anim/horse", horse::mesh);
    let anim = animator(world, horse::SKELETON, horse::GRAPH);
    let morphs = morphs(world, handle);
    world
        .spawn((
            Name::new("Horse"),
            transform,
            GlobalTransform::from(transform),
            AnimatorSettings::new(horse::SKELETON, horse::GRAPH),
            anim,
            AnimationParams::default(),
            SkinnedMeshRenderer::new(
                handle,
                SkinMaterial {
                    skin_roughness: 0.55,
                    subsurface: 0.2,
                    sheen: 0.55,
                    veins: 0.9,
                    vein_color: [0.12, 0.05, 0.03],
                    ..Default::default()
                },
            ),
            morphs,
            MuscleRig::new(horse::muscles()),
            horse::gait(),
            SpringChains::new(vec![horse::tail()]),
        ))
        .id()
}

pub fn spawn_spider(world: &mut World, transform: Transform) -> Entity {
    let handle = mesh_handle(world, "anim/spider", spider::mesh);
    let skeleton = world
        .resource::<AnimationLibrary>()
        .skeletons
        .get(spider::SKELETON)
        .cloned()
        .expect("spider skeleton");
    world
        .spawn((
            Name::new("Spider"),
            transform,
            GlobalTransform::from(transform),
            AnimatorSettings::new(spider::SKELETON, ""),
            Animator::procedural(skeleton),
            AnimationParams::default(),
            SkinnedMeshRenderer::new(
                handle,
                SkinMaterial {
                    skin_roughness: 0.75,
                    subsurface: 0.05,
                    sheen: 0.9,
                    veins: 0.0,
                    ..Default::default()
                },
            ),
            spider::gait(),
        ))
        .id()
}

pub fn spawn_car(world: &mut World, transform: Transform, paint: [f32; 3]) -> Entity {
    let key = format!("anim/car/{:.2}_{:.2}_{:.2}", paint[0], paint[1], paint[2]);
    let handle = mesh_handle(world, &key, || car::mesh(paint));
    let skeleton = world
        .resource::<AnimationLibrary>()
        .skeletons
        .get(car::SKELETON)
        .cloned()
        .expect("car skeleton");
    world
        .spawn((
            Name::new("Car"),
            transform,
            GlobalTransform::from(transform),
            AnimatorSettings::new(car::SKELETON, ""),
            Animator::procedural(skeleton),
            AnimationParams::default(),
            SkinnedMeshRenderer::new(
                handle,
                SkinMaterial {
                    skin_roughness: 0.22,
                    subsurface: 0.0,
                    sheen: 0.0,
                    wrinkles: 0.0,
                    veins: 0.0,
                    ..Default::default()
                },
            ),
            car::rig(),
        ))
        .id()
}

/// Spawn every exhibit around `origin`. Returns the main characters.
pub fn spawn_demo_world(world: &mut World, origin: DVec3) -> DemoEntities {
    let at = |x: f64, y: f64, z: f64| origin + DVec3::new(x, y, z);
    let facing = |yaw_deg: f32| Quat::from_rotation_y(yaw_deg.to_radians());
    let wood = StandardMaterial::hex(0x8a6a48).with_roughness(0.8);
    let dark_wood = StandardMaterial::hex(0x5b4330).with_roughness(0.85);
    let stone = StandardMaterial::hex(0x8d8a84).with_roughness(0.9);
    let paint_white = StandardMaterial::hex(0xe8e4dc).with_roughness(0.6);
    let sand = StandardMaterial::hex(0xb59a72).with_roughness(0.97);

    let props = |world: &mut World, t: Transform, shape: MeshShape, material: StandardMaterial| {
        world.spawn((
            t,
            GlobalTransform::from(t),
            MeshRenderer::new(shape, material),
        ));
    };
    // Arena floor (visual only, 2 cm).
    props(
        world,
        Transform::from_translation(at(0.0, 0.01, 0.0)),
        MeshShape::Cylinder {
            radius: RADIUS as f32 - 1.0,
            height: 0.02,
            segments: 96,
        },
        sand,
    );
    // Fence around the paddock.
    let posts = 56;
    for k in 0..posts {
        let a0 = k as f64 / posts as f64 * std::f64::consts::TAU;
        let a1 = (k + 1) as f64 / posts as f64 * std::f64::consts::TAU;
        let p0 = DVec3::new(a0.cos(), 0.0, a0.sin()) * (RADIUS - 0.5);
        let p1 = DVec3::new(a1.cos(), 0.0, a1.sin()) * (RADIUS - 0.5);
        // Leave a gate toward +Z for the camera.
        let gate = (a0 - std::f64::consts::FRAC_PI_2).abs() < 0.12;
        props(
            world,
            Transform::from_translation(origin + p0 + DVec3::Y * 0.65),
            MeshShape::Cuboid {
                size: [0.14, 1.3, 0.14],
            },
            dark_wood,
        );
        if gate {
            continue;
        }
        let mid = (p0 + p1) * 0.5;
        let len = (p1 - p0).length() as f32;
        let yaw = (p1 - p0).x.atan2((p1 - p0).z) as f32;
        for h in [0.55, 1.05] {
            props(
                world,
                Transform::from_translation(origin + mid + DVec3::Y * h)
                    .with_rotation(Quat::from_rotation_y(yaw)),
                MeshShape::Cuboid {
                    size: [0.06, 0.12, len + 0.1],
                },
                paint_white,
            );
        }
    }

    // --- Muscle stage: hammer swing at a tire, a flexing lifter, a reach in a shirt.
    let stage = Transform::from_translation(at(0.0, 0.2, 6.0));
    world.spawn((
        stage,
        GlobalTransform::from(stage),
        IkSurface {
            shape: IkShape::Box,
            half_extents: [4.6, 0.2, 1.8],
            surface: 12,
        },
        MeshRenderer::new(
            MeshShape::Cuboid {
                size: [9.2, 0.4, 3.6],
            },
            wood,
        ),
    ));
    let athlete = spawn_humanoid(
        world,
        humanoid::Outfit::Athlete,
        Transform::from_translation(at(ATHLETE.x, 0.4, ATHLETE.z)).with_rotation(facing(180.0)),
    );
    world.entity_mut(athlete).insert(DemoScript {
        period: 11.0,
        steps: vec![
            DemoStep::trigger(0.5, "heavy_swing"),
            DemoStep::param(3.6, "blocking", 1.0),
            DemoStep::trigger(3.6, "block"),
            DemoStep::param(6.4, "blocking", 0.0),
            DemoStep::trigger(7.6, "heavy_swing"),
        ],
        time: 0.0,
    });
    spawn_hammer(world, athlete, at(ATHLETE.x, 1.0, ATHLETE.z));
    // The tractor tire the hammer comes down on; the athlete keeps their eyes on it.
    let tire = Transform::from_translation(at(ATHLETE.x, 0.56, ATHLETE.z + TIRE_AHEAD));
    props(
        world,
        tire,
        MeshShape::Torus {
            radius: 0.45,
            tube: 0.16,
            segments: 40,
        },
        StandardMaterial::hex(0x1d1d1f).with_roughness(0.85),
    );
    world.entity_mut(athlete).insert(LookAt {
        target: at(ATHLETE.x, 0.7, ATHLETE.z + TIRE_AHEAD - 0.4),
        chain: vec![("neck".into(), 0.4), ("head".into(), 0.6)],
        max_angle_deg: 60.0,
        weight: 0.7,
        ..Default::default()
    });
    let lifter = spawn_humanoid(
        world,
        humanoid::Outfit::Lifter,
        Transform::from_translation(at(0.7, 0.4, 5.6)).with_rotation(facing(180.0)),
    );
    world.entity_mut(lifter).insert(DemoScript {
        period: 5.0,
        steps: vec![DemoStep::trigger(0.8, "flex")],
        time: 0.0,
    });
    let reacher = spawn_humanoid(
        world,
        humanoid::Outfit::Shirt,
        Transform::from_translation(at(3.1, 0.4, 5.8)).with_rotation(facing(180.0)),
    );
    world.entity_mut(reacher).insert(DemoScript {
        period: 5.5,
        steps: vec![DemoStep::trigger(0.5, "reach")],
        time: 0.0,
    });

    // --- Step course: ramp up to a deck, stairs down the other side.
    for (t, surface, size, material) in [
        (
            Transform::from_translation(at(-10.0, 0.4, -4.5)),
            IkSurface {
                shape: IkShape::Ramp,
                half_extents: [1.0, 0.4, 2.0],
                surface: 12,
            },
            None,
            wood,
        ),
        (
            Transform::from_translation(at(-10.0, 0.4, -8.0)),
            IkSurface {
                shape: IkShape::Box,
                half_extents: [1.5, 0.4, 1.5],
                surface: 12,
            },
            Some([3.0, 0.8, 3.0]),
            wood,
        ),
        (
            Transform::from_translation(at(-13.0, 0.4, -8.0)).with_rotation(facing(-90.0)),
            IkSurface {
                shape: IkShape::Stairs { steps: 6 },
                half_extents: [1.0, 0.4, 1.5],
                surface: 2,
            },
            None,
            stone,
        ),
    ] {
        world.spawn((t, GlobalTransform::from(t), surface));
        match (surface.shape, size) {
            (IkShape::Box, Some(size)) => props(world, t, MeshShape::Cuboid { size }, material),
            (IkShape::Ramp, _) => {
                let [hx, hy, hz] = surface.half_extents;
                let angle = (2.0 * hy / (2.0 * hz)).atan();
                let len = (4.0 * hz * hz + 4.0 * hy * hy).sqrt();
                // A slab tilted to the slope (rising toward local −Z), plus a filler wedge
                // isn't needed: the slab's underside sits below ground at the low end.
                let slab = Transform {
                    translation: t.translation + DVec3::Y * -0.06,
                    rotation: t.rotation * Quat::from_rotation_x(angle),
                    scale: Vec3::ONE,
                };
                props(
                    world,
                    slab,
                    MeshShape::Cuboid {
                        size: [2.0 * hx, 0.12, len],
                    },
                    material,
                );
            }
            (IkShape::Stairs { steps }, _) => {
                let [hx, hy, hz] = surface.half_extents;
                let n = steps.max(1);
                let depth = 2.0 * hz / n as f32;
                for k in 0..n {
                    let height = 2.0 * hy * (k + 1) as f32 / n as f32;
                    // Step k spans local z from hz - k*depth down to hz - (k+1)*depth.
                    let z = hz - (k as f32 + 0.5) * depth;
                    let local = Vec3::new(0.0, -hy + height * 0.5, z);
                    let st = Transform {
                        translation: t.translation + (t.rotation * local).as_dvec3(),
                        rotation: t.rotation,
                        scale: Vec3::ONE,
                    };
                    props(
                        world,
                        st,
                        MeshShape::Cuboid {
                            size: [2.0 * hx, height, depth],
                        },
                        material,
                    );
                }
            }
            _ => {}
        }
    }
    let walker = spawn_humanoid(
        world,
        humanoid::Outfit::Shirt,
        Transform::from_translation(at(-10.0, 0.0, 1.5)),
    );
    world.entity_mut(walker).insert((
        Name::new("Walker"),
        DemoPath::new(
            origin,
            vec![
                [-10.0, 1.5, 1.4],
                [-10.0, -2.0, 1.3],
                [-10.0, -5.0, 1.2],
                [-10.0, -7.6, 1.1],
                [-11.2, -8.0, 1.0],
                [-13.0, -8.0, 1.0],
                [-14.9, -8.0, 1.2],
                [-16.5, -6.8, 2.6],
                [-16.8, -3.0, 3.8],
                [-16.5, 0.6, 3.8],
                [-15.0, 2.6, 2.4],
                [-12.0, 3.0, 1.6],
            ],
        ),
    ));

    // --- Horse track: an oval with a white rail.
    let horse_path = vec![
        [16.0, 7.0, 1.6],
        [9.0, 5.6, 1.6],
        [5.4, 0.0, 3.4],
        [7.4, -5.6, 3.6],
        [13.0, -8.0, 6.5],
        [20.0, -8.0, 9.0],
        [26.0, -5.0, 11.0],
        [27.6, 0.5, 11.0],
        [25.0, 5.5, 8.0],
        [21.0, 7.4, 4.0],
    ];
    for (k, w) in horse_path.iter().enumerate() {
        let n = horse_path[(k + 1) % horse_path.len()];
        let (a, b) = (Vec2::new(w[0], w[1]), Vec2::new(n[0], n[1]));
        let center = Vec2::new(16.5, -0.3);
        for (r, along) in [(1.32f32, 0.0f32), (0.7, 0.5)] {
            let p = center + (a.lerp(b, along) - center) * r;
            props(
                world,
                Transform::from_translation(at(p.x as f64, 0.5, p.y as f64)),
                MeshShape::Cuboid {
                    size: [0.1, 1.0, 0.1],
                },
                paint_white,
            );
        }
    }
    let horse_e = spawn_horse(
        world,
        Transform::from_translation(at(16.0, 0.0, 7.0)).with_rotation(facing(90.0)),
    );
    world.entity_mut(horse_e).insert(DemoPath {
        effort_speed: 11.0,
        ..DemoPath::new(origin, horse_path)
    });

    // --- Spider garden: rocks to climb over.
    for (x, z, hy, yaw) in [
        (9.0, 13.0, 0.18, 10.0),
        (11.6, 11.2, 0.28, -25.0),
        (13.5, 14.0, 0.14, 40.0),
        (7.2, 10.8, 0.22, 70.0),
        (14.2, 10.6, 0.33, 15.0),
    ] {
        let t = Transform::from_translation(at(x, hy as f64, z)).with_rotation(facing(yaw));
        world.spawn((
            t,
            GlobalTransform::from(t),
            IkSurface {
                shape: IkShape::Box,
                half_extents: [0.7, hy, 0.55],
                surface: 2,
            },
            MeshRenderer::new(
                MeshShape::Cuboid {
                    size: [1.4, hy * 2.0, 1.1],
                },
                stone,
            ),
        ));
    }
    let spider_e = spawn_spider(world, Transform::from_translation(at(8.0, 0.0, 12.0)));
    world.entity_mut(spider_e).insert(DemoPath {
        effort_speed: 2.5,
        ..DemoPath::new(
            origin,
            vec![
                [8.0, 15.0, 1.0],
                [6.0, 12.0, 1.0],
                [8.5, 9.5, 1.1],
                [11.0, 12.0, 0.9],
                [13.5, 15.0, 1.0],
                [16.0, 12.5, 1.1],
                [13.5, 9.6, 1.0],
                [11.0, 12.0, 0.9],
            ],
        )
    });

    // --- Car loop with two speed bumps.
    for (x, z, yaw) in [(-3.0, -26.8, 0.0), (3.0, -17.2, 0.0)] {
        let t = Transform::from_translation(at(x, 0.05, z)).with_rotation(facing(yaw));
        world.spawn((
            t,
            GlobalTransform::from(t),
            IkSurface {
                shape: IkShape::Box,
                half_extents: [0.35, 0.05, 1.6],
                surface: 1,
            },
            MeshRenderer::new(
                MeshShape::Cuboid {
                    size: [0.7, 0.1, 3.2],
                },
                StandardMaterial::hex(0xd8b23a).with_roughness(0.6),
            ),
        ));
    }
    let car_e = spawn_car(
        world,
        Transform::from_translation(at(0.0, 0.0, -27.0)).with_rotation(facing(-90.0)),
        [0.55, 0.04, 0.02],
    );
    world.entity_mut(car_e).insert(DemoPath {
        effort_speed: 10.0,
        ..DemoPath::new(
            origin,
            vec![
                [-6.0, -27.0, 6.5],
                [0.0, -27.0, 7.0],
                [6.0, -27.0, 6.5],
                [9.5, -24.0, 5.5],
                [9.5, -20.0, 5.5],
                [6.0, -17.0, 6.5],
                [0.0, -17.0, 7.0],
                [-6.0, -17.0, 6.5],
                [-9.5, -20.0, 5.5],
                [-9.5, -24.0, 5.5],
            ],
        )
    });

    DemoEntities {
        athlete,
        lifter,
        reacher,
        walker,
        horse: horse_e,
        spider: spider_e,
        car: car_e,
    }
}

#[allow(clippy::type_complexity)]
fn demo_path_system(
    time: Res<Time>,
    ground: Res<Ground>,
    physics: Option<Res<PhysicsQueries>>,
    surfaces: Res<IkSurfaces>,
    mut q: Query<(
        Entity,
        &mut DemoPath,
        &mut Transform,
        Option<&mut AnimationParams>,
        Option<&LocalTimeScale>,
    )>,
) {
    for (entity, mut path, mut t, params, local) in &mut q {
        let dt = time.delta_for(local);
        path.build();
        if path.samples.len() < 2 {
            continue;
        }
        let (_, _, speed) = path.at(path.distance);
        let speed = speed * path.speed;
        path.distance += speed * dt;
        let (p, dir, _) = path.at(path.distance);
        let probe = GroundProbe {
            ground: &ground,
            physics: physics.as_deref(),
            surfaces: &surfaces.0,
            exclude: Some(entity),
        };
        let mut pos = path.origin + DVec3::new(p.x as f64, 0.0, p.y as f64);
        pos.y = t.translation.y;
        let target_y = probe
            .probe(DVec3::new(pos.x, pos.y + 0.3, pos.z), 0.7, 3.0)
            .map_or(path.origin.y, |h| h.height);
        if path.started {
            pos.y += (target_y - pos.y) * smoothing(dt, 0.12) as f64;
        } else {
            pos.y = target_y;
        }
        let yaw = (-dir.x).atan2(-dir.y);
        let old_yaw = {
            let f = t.rotation * Vec3::NEG_Z;
            (-f.x).atan2(-f.z)
        };
        let mut dyaw = yaw - old_yaw;
        while dyaw > PI {
            dyaw -= 2.0 * PI;
        }
        while dyaw < -PI {
            dyaw += 2.0 * PI;
        }
        if dt > 0.0 && path.started {
            path.yaw_rate += (dyaw / dt - path.yaw_rate) * smoothing(dt, 0.25);
        }
        path.started = true;
        t.translation = pos;
        t.rotation = Quat::from_rotation_y(yaw);
        if let Some(mut params) = params {
            params.set("speed", speed);
            params.set("turn", (path.yaw_rate / 1.2).clamp(-1.0, 1.0));
            params.set(
                "move_effort",
                (speed / path.effort_speed.max(0.1)).clamp(0.0, 1.0),
            );
        }
    }
}

#[allow(clippy::type_complexity)]
fn demo_script_system(
    time: Res<Time>,
    mut q: Query<(
        &mut DemoScript,
        Option<&mut Animator>,
        Option<&mut AnimationParams>,
        Option<&LocalTimeScale>,
    )>,
) {
    for (mut script, mut animator, mut params, local) in &mut q {
        let dt = time.delta_for(local);
        if script.period <= 0.0 || dt <= 0.0 {
            continue;
        }
        let from = script.time;
        let to = from + dt;
        script.time = to % script.period;
        for step in &script.steps {
            // Fire steps in (from, to], wrapping around the period.
            let hit = (step.at > from && step.at <= to)
                || (to > script.period && step.at <= to - script.period);
            if !hit {
                continue;
            }
            if !step.trigger.is_empty()
                && let Some(a) = animator.as_mut()
            {
                a.trigger(&step.trigger);
            }
            if !step.param.is_empty()
                && let Some(p) = params.as_mut()
            {
                p.set(&step.param, step.value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_loop_smoothly() {
        let mut path = DemoPath::new(
            DVec3::ZERO,
            vec![
                [0.0, 0.0, 1.0],
                [10.0, 0.0, 2.0],
                [10.0, 10.0, 3.0],
                [0.0, 10.0, 1.0],
            ],
        );
        let len = path.length();
        assert!(len > 30.0 && len < 45.0, "{len}");
        let (a, _, _) = path.at(0.0);
        let (b, _, _) = path.at(len);
        assert!(a.distance(b) < 1e-3);
        // Speeds follow the waypoints.
        let (_, _, s) = path.at(0.0);
        assert!((s - 1.0).abs() < 1e-3);
    }
}
