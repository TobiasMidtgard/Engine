//! The animation paddock: a showcase of every animation feature, spawned around one point.
//!
//! * **Muscle stage**: an athlete swings a sledgehammer (arms, shoulders and back tense on the
//!   strike, then relax; veins show on the forearms; the ponytail whips), braces in a guard
//!   with the handle across the face in both hands, and hits a double-biceps flex. Next to
//!   them, a man in a long-sleeved shirt goes up on tiptoe to take hold of a lantern hanging
//!   from a beam: the sleeve bunches at the elbow as the arm sweeps up, then rides up the
//!   straightened arm while the shirt pulls across his back.
//!   Three spectators stand on the sand in front of the stage: people a character module
//!   draws, who follow the hammer, blink and wince at each strike.
//! * **Step course**: a walker climbs a ramp, crosses a deck, walks down stairs (feet planted
//!   by foot IK), then runs back on the flat, leaning into the turns.
//! * **Horse track**: a horse walks, trots, canters and gallops around an oval; its haunches
//!   and shoulders flex with every stride, its tail streams behind.
//! * **Spider garden**: a giant furred tarantula climbs over boulders on eight procedural legs,
//!   and stops in the open to rear up in threat.
//! * **Car loop**: a car drives over speed bumps (springy suspension) and rolls in the corners
//!   while its front wheels steer and every wheel spins.
//!
//! Call [`spawn_demo`] with the zone center. The people are detailed, textured characters
//! with motion-capture locomotion (Microsoft Rocketbox, MIT; fetched, see
//! `assets/manifests/animation.toml`) driven by the engine's humanoid rig. They load in the
//! background; until then, or if they haven't been fetched, generated bodies stand in.
//! Everything else is generated from code.

use std::collections::HashSet;
use std::f32::consts::PI;

use engine_assets::{AssetServer, Assets};
use engine_core::prelude::*;
use engine_render::{
    GltfScene, MaterialPreset, Mesh, MeshRenderer, Shape as MeshShape, StandardMaterial,
};
use engine_world::animation::{AnimationParams, BodyRequest};
use engine_world::character::{Ancestry, AppearanceRequest};
use engine_world::physics::{Collider, PhysicsQueries, RigidBody};
use engine_world::{DamageEvent, DamageKind, FocusPoints, Ground, SimulationRange};
use serde_json::{Value, json};

use crate::animator::{AnimationLibrary, Animator, AnimatorSettings};
use crate::attach::{BoneAttachment, Hang};
use crate::build::{car, dog, horse, humanoid, rock, spider};
use crate::character::HumanModel;
use crate::hit::HitReaction;
use crate::horse_model::HorseModel;
use crate::ik::{GroundProbe, IkShape, IkSurface, IkSurfaces, LookAt, Reach, ReachIk};
use crate::muscle::{MorphWeights, MuscleRig};
use crate::procedural::spring::SpringChains;
use crate::props;
use crate::retarget::HandGrip;
use crate::shuttle::{ShuttleLeg, ShuttleRun, Watch};
use crate::skin::{SkinMaterial, SkinnedMesh, SkinnedMeshRenderer};
use crate::stunt;
use crate::xform::smoothing;

/// Radius of the paddock, meters.
pub const RADIUS: f64 = 36.0;

/// Where the hammer athlete stands (local x, z) and how far ahead of them the tire lies.
const ATHLETE: DVec3 = DVec3::new(-2.6, 0.0, 5.4);
const TIRE_AHEAD: f64 = 1.3;
/// The dog's run ([x, z, speed], m and m/s from the paddock's center): it walks off at the
/// west end, trots, canters out of the east turn and gallops down the far straight.
/// Start of the shuttle lane (x, z from the paddock's center; it runs towards −Z) and its
/// length, m.
const LANE: DVec3 = DVec3::new(-23.0, 0.0, 7.0);
/// Where the freeflow brawler stands (x, z from the paddock's center), between the step
/// course and the car loop, and how long its round takes, s.
const FREEFLOW: DVec3 = DVec3::new(0.0, 0.0, -10.0);
const FREEFLOW_PERIOD: f32 = 12.0;
/// The weapons yard, south-west of the car loop (x, z from the paddock's center): the
/// sword-and-shield pair, the great sword and its dummy south of them, the archer to the east
/// shooting south at a butt.
const DUEL: DVec3 = DVec3::new(-24.0, 0.0, -16.0);
const DUEL_PERIOD: f32 = 13.0;
const GREATSWORD: DVec3 = DVec3::new(-24.0, 0.0, -21.0);
const GREATSWORD_PERIOD: f32 = 10.0;
const ARCHER: DVec3 = DVec3::new(-18.0, 0.0, -12.0);
const BUTT: DVec3 = DVec3::new(-18.0, 0.0, -24.0);
/// How high the butt's gold is, m.
const BUTT_GOLD: f64 = 1.3;
const ARCHERY_PERIOD: f32 = 9.0;
const LANE_LENGTH: f32 = 15.0;
const DOG_PATH: [[f32; 3]; 10] = [
    [-12.0, 12.0, 1.1],
    [-6.0, 12.0, 2.6],
    [0.0, 12.0, 4.8],
    [3.6, 14.2, 3.4],
    [3.6, 17.8, 3.6],
    [0.0, 20.0, 6.5],
    [-6.0, 20.0, 9.0],
    [-12.0, 20.0, 6.0],
    [-15.6, 17.8, 3.0],
    [-15.6, 14.2, 1.6],
];
/// The realistic horse (`assets/manifests/animation.toml`).
const HORSE_FILE: &str = "anim/animals/horse/Horse.gltf";
/// The hammer in the right hand's frame: the fist's center, and the handle's direction toward
/// the head (perpendicular to the forearm, tipped down a little, as a hand holds a hammer).
const GRIP: Vec3 = Vec3::new(0.0, -0.085, 0.0);
const HANDLE_DIR: Vec3 = Vec3::new(0.0, -0.34, -0.94);

/// The main entities of a spawned demo (for cameras, tests and tools).
#[derive(Debug, Clone, Copy)]
pub struct DemoEntities {
    /// The three spectators in front of the muscle stage (drawn by a character module when
    /// there is one).
    pub spectators: [Entity; 3],
    pub athlete: Entity,
    pub lifter: Entity,
    pub reacher: Entity,
    pub walker: Entity,
    /// Walks a square facing one way: sidesteps and backpedals.
    pub drill: Entity,
    /// Flinches from blows on a timeline.
    pub sparring: Entity,
    /// Throws a jab, cross, hook and uppercut combo at the sparring partner.
    pub boxer: Entity,
    pub horse: Entity,
    pub spider: Entity,
    pub dog: Entity,
    pub car: Entity,
    pub stuntman: Entity,
    pub runner: Entity,
    pub coach: Entity,
    /// Weapons yard: the sword-and-shield fighter who opens the exchange, and the one it
    /// fights.
    pub swordsman: Entity,
    pub shield_partner: Entity,
    /// Swings a great sword at `greatsword_dummy`.
    pub greatsword: Entity,
    pub greatsword_dummy: Entity,
    /// Shoots at the butt.
    pub archer: Entity,
}

/// Follows a closed, smooth path through `waypoints` (`[x, z, speed]`, meters and m/s,
/// relative to `origin`), standing on whatever ground is there. Sets the `speed`, `turn`,
/// `heading` and `move_effort` animation parameters.
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
    /// Keep facing this way (yaw, radians) instead of along the path: a person then
    /// sidesteps and backpedals round it.
    pub facing: Option<f32>,
    #[serde(skip)]
    samples: Vec<(Vec2, f32, f32)>,
    #[serde(skip)]
    yaw_rate: f32,
    /// Velocity on the ground (x, z), m/s, and the acceleration along the facing, m/s²
    /// (smoothed).
    #[serde(skip)]
    velocity: Vec2,
    #[serde(skip)]
    accel: f32,
    /// Rise per meter of the ground along the way (smoothed), and as last measured.
    #[serde(skip)]
    grade: f32,
    #[serde(skip)]
    rise: f32,
    #[serde(skip)]
    started: bool,
    /// Stops along the way.
    pub pauses: Vec<DemoPause>,
    /// The stop under way (index into `pauses`) and seconds spent at it.
    #[serde(skip)]
    pause: Option<(usize, f32)>,
    /// Each stop's parameter as eased so far.
    #[serde(skip)]
    pause_weights: Vec<f32>,
}

/// A stop on a [`DemoPath`]: on reaching waypoint `waypoint` the walker slows to a halt and
/// waits `secs`, with the parameter `param` eased up to 1 while it waits and back to 0 before
/// it moves on.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoPause {
    pub waypoint: usize,
    pub secs: f32,
    pub param: String,
}

/// How far before and after a stop a walker slows down and gets going again, m.
const PAUSE_EASE_M: f32 = 0.8;
/// How long a stop's parameter takes to ease in and out, s.
const PAUSE_PARAM_SECS: f32 = 0.6;

impl Default for DemoPath {
    fn default() -> Self {
        Self {
            origin: DVec3::ZERO,
            waypoints: Vec::new(),
            distance: 0.0,
            speed: 1.0,
            effort_speed: humanoid::RUN_SPEED,
            facing: None,
            samples: Vec::new(),
            yaw_rate: 0.0,
            velocity: Vec2::ZERO,
            accel: 0.0,
            grade: 0.0,
            rise: 0.0,
            started: false,
            pauses: Vec::new(),
            pause: None,
            pause_weights: Vec::new(),
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

    /// Distance along the loop at waypoint `i`, m.
    fn waypoint_distance(&self, i: usize) -> f32 {
        self.samples.get(i * 24).map_or(0.0, |s| s.1)
    }

    /// Move along the loop for `dt` seconds, slowing into each stop, waiting there and picking
    /// up again after it; returns the speed, m/s.
    fn advance(&mut self, dt: f32) -> f32 {
        let len = self.samples.last().map_or(0.0, |s| s.1).max(1e-3);
        if let Some((i, t)) = self.pause {
            let t = t + dt;
            if t < self.pauses[i].secs {
                self.pause = Some((i, t));
                return 0.0;
            }
            self.pause = None;
        }
        let (_, _, base) = self.at(self.distance);
        let d = self.distance.rem_euclid(len);
        // Slow down into a stop and pick up again after it.
        let mut ease = 1.0f32;
        for p in &self.pauses {
            let at = self.waypoint_distance(p.waypoint);
            let ahead = (at - d).rem_euclid(len);
            let behind = (d - at).rem_euclid(len);
            let near = ahead.min(behind) / PAUSE_EASE_M;
            ease = ease.min(0.25 + 0.75 * near.clamp(0.0, 1.0).powf(0.7));
        }
        let speed = base * self.speed * ease;
        let step = speed * dt;
        let reached = self.pauses.iter().position(|p| {
            let ahead = (self.waypoint_distance(p.waypoint) - d).rem_euclid(len);
            ahead > 1e-4 && ahead <= step
        });
        match reached {
            Some(i) => {
                let ahead = (self.waypoint_distance(self.pauses[i].waypoint) - d).rem_euclid(len);
                self.distance += ahead;
                self.pause = Some((i, 0.0));
            }
            None => self.distance += step,
        }
        speed
    }

    /// Ease each stop's parameter toward 1 while the walker waits there (until near the end
    /// of the wait) and toward 0 otherwise; returns (name, value) pairs.
    fn pause_params(&mut self, dt: f32) -> Vec<(String, f32)> {
        self.pause_weights.resize(self.pauses.len(), 0.0);
        let k = smoothing(dt, PAUSE_PARAM_SECS * 0.3);
        let mut out = Vec::with_capacity(self.pauses.len());
        for (i, p) in self.pauses.iter().enumerate() {
            let on = self
                .pause
                .is_some_and(|(j, t)| j == i && t < p.secs - PAUSE_PARAM_SECS);
            let w = &mut self.pause_weights[i];
            *w += (f32::from(u8::from(on)) - *w) * k;
            out.push((p.param.clone(), *w));
        }
        out
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
    /// Where the entity stood when the script first ran (world), for [`DemoStep::home`].
    #[serde(skip)]
    pub home: Option<DVec3>,
}

/// One step of a [`DemoScript`]: a trigger, a parameter value and/or a blow at `at` seconds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoStep {
    pub at: f32,
    pub trigger: String,
    pub param: String,
    pub value: f32,
    /// Strike the entity: a [`DamageEvent`] at this point with this push (N·s), both in its
    /// own frame (it faces −Z).
    pub hit: Option<(Vec3, Vec3)>,
    /// Step back to where it stood when the script started (after blows knocked it away).
    pub home: bool,
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
    pub fn hit(at: f32, point: Vec3, push: Vec3) -> Self {
        Self {
            at,
            hit: Some((point, push)),
            ..Default::default()
        }
    }
    pub fn home(at: f32) -> Self {
        Self {
            at,
            home: true,
            ..Default::default()
        }
    }
}

/// How long the boxer's and sparring partner's routine takes, s: a combo the partner slips
/// and counters, a body shot that folds the partner onto a knee, a combo that knocks them
/// back, then one that knocks them down.
const SPARRING_PERIOD: f32 = 26.0;

/// When the boxer's and sparring partner's routine gets to the body shot, s (the slips come
/// first).
pub const SPARRING_BODY: f32 = 6.0;

/// When the boxer's and sparring partner's routine gets to the combos, s.
pub const SPARRING_COMBOS: f32 = SPARRING_BODY + 7.0;

/// How fast a [`DemoStep::home`] walk back comes (time constant, s): slower than a blow, so
/// the feet shuffle.
const HOME_SECS: f32 = 0.45;

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
        let d = lib.add_skeleton(dog::skeleton(1.0));
        for clip in dog::clips(&d) {
            lib.add_clip(clip);
        }
        lib.add_graph(dog::graph());
        let sp = lib.add_skeleton(spider::skeleton());
        for clip in spider::clips(&sp) {
            lib.add_clip(clip);
        }
        lib.add_graph(spider::graph());
        lib.add_skeleton(car::skeleton());
        let concept = lib.add_skeleton(car::concept_skeleton());
        lib.add_clip(car::concept_door_clip(&concept));
        lib.add_graph(car::concept_parked_graph());
    }
    app.register_component::<DemoPath>(
        "Demo mover: follows a closed path of [x, z, speed] waypoints (m, m/s) on the ground.",
    )
    .register_component::<DemoArcher>(
        "Demo archer: each release of its bow shot looses an arrow from the draw hand at a \
         point (world).",
    )
    .register_component::<DemoScript>(
        "Demo timeline: triggers and parameter values at times (s), repeating every period.",
    )
    .register_component::<ShuttleRun>(
        "Shuttle runner: straight legs from a standing start to a standing stop, turning on \
         the spot between them.",
    )
    .register_component::<Watch>("Watcher: turns where it stands to keep watching a target.")
    .register_component::<stunt::Stunt>(
        "Stunt yard routine: run off the deck, fall limp onto the mat, get up, walk back.",
    )
    .add_systems(
        Update,
        (
            demo_path_system,
            demo_script_system,
            loose_arrows,
            stunt::stunt_system,
            crate::shuttle::shuttle_system,
            crate::shuttle::watch_system,
        )
            .in_set(UpdateSet::Logic),
    )
    .add_systems(PreUpdate, finish_prefetch.run_if(prefetch_pending))
    .register_api(
        "anim.spawn",
        "Spawn a demo character: athlete, shirt, lifter, horse, dog, spider or car.",
        r#"{"kind": "horse", "at": [0, 0, 0], "yaw_deg": 0}"#,
        |world, args| {
            let kind = args
                .get("kind")
                .and_then(Value::as_str)
                .ok_or("pass \"kind\": athlete, shirt, lifter, horse, dog, spider or car")?;
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
                "dog" => spawn_dog(world, t, 1.0),
                "spider" => spawn_spider(world, t),
                "car" => spawn_car(world, t, CAR_PAINT),
                other => {
                    return Err(format!(
                        "unknown kind {other}; use athlete, shirt, lifter, horse, dog, spider or car"
                    ));
                }
            };
            Ok(json!({ "id": e.to_bits() }))
        },
    )
    .register_api(
        "anim.demo_restart",
        "Start every animation paddock's test cases over (camera stops time their shots from \
         here).",
        "{}",
        |world, _| Ok(json!({ "restarted": restart_demos(world).len() })),
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
                "dog": d.dog.to_bits(),
                "car": d.car.to_bits(),
                "stuntman": d.stuntman.to_bits(),
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

/// The spider garden's boulders: x, z (m from the paddock's middle), half height (m), yaw (°).
const SPIDER_ROCKS: [(f64, f64, f32, f32); 5] = [
    (9.0, 13.0, 0.18, 10.0),
    (11.6, 11.2, 0.28, -25.0),
    (13.5, 14.0, 0.14, 40.0),
    (7.2, 10.8, 0.22, 70.0),
    (14.2, 10.6, 0.33, 15.0),
];

fn rock_key(k: usize) -> String {
    format!("anim/rock_{k}")
}

/// Half extents of boulder `k`.
fn rock_half(k: usize) -> Vec3 {
    Vec3::new(0.7, SPIDER_ROCKS[k].2, 0.55)
}

fn spider_rock(k: usize) -> Mesh {
    rock::mesh(rock_half(k), k as u32)
}

/// The paddock's meshes, made without the world ([`build_demo`]) so the slow part of loading
/// the zone can run on a worker thread; [`place_demo`] spawns it in a moment.
pub struct DemoParts {
    origin: DVec3,
    bodies: Vec<(String, SkinnedMesh)>,
    rocks: Vec<(String, Mesh)>,
}

/// Body keys and builders of everything the paddock wears (the generated bodies: people
/// wearing fetched models keep theirs until the model is on).
fn paddock_bodies() -> Vec<(String, Box<dyn FnOnce() -> SkinnedMesh + Send>)> {
    let paint = CAR_PAINT;
    vec![
        (
            "anim/spider".into(),
            Box::new(spider::mesh) as Box<dyn FnOnce() -> SkinnedMesh + Send>,
        ),
        ("anim/horse".into(), Box::new(horse::mesh)),
        (
            format!("anim/{}", dog::skeleton_name(1.0)),
            Box::new(|| dog::mesh(1.0)),
        ),
        (car_key(paint), Box::new(move || car::mesh(paint))),
        (
            "anim/humanoid/athlete".into(),
            Box::new(|| humanoid::mesh(humanoid::Outfit::Athlete)),
        ),
        (
            "anim/humanoid/lifter".into(),
            Box::new(|| humanoid::mesh(humanoid::Outfit::Lifter)),
        ),
        (
            "anim/humanoid/shirt".into(),
            Box::new(|| humanoid::mesh(humanoid::Outfit::Shirt)),
        ),
    ]
}

/// Make the paddock's meshes (generated bodies and boulders) without touching the world, the
/// bodies side by side on scoped threads. Pair with [`place_demo`] (for
/// `StreamCell::built`).
pub fn build_demo(origin: DVec3) -> DemoParts {
    let bodies = std::thread::scope(|scope| {
        let jobs: Vec<_> = paddock_bodies()
            .into_iter()
            .map(|(key, make)| scope.spawn(move || (key, make())))
            .collect();
        jobs.into_iter()
            .map(|j| j.join().expect("paddock body builder"))
            .collect()
    });
    let rocks = (0..SPIDER_ROCKS.len())
        .map(|k| (rock_key(k), spider_rock(k)))
        .collect();
    DemoParts {
        origin,
        bodies,
        rocks,
    }
}

/// Spawn the paddock from what [`build_demo`] made: its meshes go into the asset stores under
/// the keys the paddock looks them up by, so spawning builds none of them.
pub fn place_demo(world: &mut World, parts: DemoParts) -> DemoEntities {
    let origin = parts.origin;
    store_parts(world, parts);
    spawn_demo_world(world, origin)
}

/// Put the made meshes in the asset stores (keeping any already there).
fn store_parts(world: &mut World, parts: DemoParts) {
    {
        let mut bodies = world.get_resource_or_init::<Assets<SkinnedMesh>>();
        for (key, mesh) in parts.bodies {
            if bodies.loaded_by_key(&key).is_none() {
                bodies.insert_keyed(&key, mesh);
            }
        }
    }
    {
        let mut meshes = world.get_resource_or_init::<Assets<Mesh>>();
        for (key, mesh) in parts.rocks {
            if meshes.loaded_by_key(&key).is_none() {
                meshes.insert_keyed(&key, mesh);
            }
        }
    }
}

/// The paddock's meshes being made on a worker ([`prefetch`]).
#[derive(Resource)]
pub struct PaddockLoading {
    parts: std::sync::Mutex<Option<std::sync::mpsc::Receiver<DemoParts>>>,
    ready: bool,
}

/// Start making the paddock's meshes on a worker thread, ahead of the zone: once [`ready`]
/// says they are in, [`spawn_demo`] builds nothing and takes a millisecond or two.
pub fn prefetch(world: &mut World) {
    if world.contains_resource::<PaddockLoading>() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("paddock meshes".into())
        .spawn(move || {
            let _ = tx.send(build_demo(DVec3::ZERO));
        })
        .expect("paddock mesh thread");
    world.insert_resource(PaddockLoading {
        parts: std::sync::Mutex::new(Some(rx)),
        ready: false,
    });
}

/// Whether the meshes [`prefetch`] started are in (true when nothing was prefetched, so a
/// caller never waits on a prefetch it didn't start).
pub fn ready(world: &World) -> bool {
    world
        .get_resource::<PaddockLoading>()
        .is_none_or(|l| l.ready)
}

/// Still waiting on the worker [`prefetch`] started.
fn prefetch_pending(loading: Option<Res<PaddockLoading>>) -> bool {
    loading.is_some_and(|l| !l.ready)
}

/// Store the prefetched meshes when the worker is done.
fn finish_prefetch(
    mut loading: ResMut<PaddockLoading>,
    mut bodies: ResMut<Assets<SkinnedMesh>>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
) {
    let slot = loading.parts.get_mut().expect("paddock prefetch lock");
    let parts = match slot.as_ref().map(|rx| rx.try_recv()) {
        Some(Ok(parts)) => Some(parts),
        Some(Err(std::sync::mpsc::TryRecvError::Empty)) => return,
        // The worker died: spawning builds the meshes itself.
        _ => None,
    };
    *slot = None;
    loading.ready = true;
    let Some(parts) = parts else {
        return;
    };
    for (key, mesh) in parts.bodies {
        if bodies.loaded_by_key(&key).is_none() {
            bodies.insert_keyed(&key, mesh);
        }
    }
    if let Some(meshes) = meshes.as_mut() {
        for (key, mesh) in parts.rocks {
            if meshes.loaded_by_key(&key).is_none() {
                meshes.insert_keyed(&key, mesh);
            }
        }
    }
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

/// Deeper folds for a loose long-sleeved shirt.
fn shirt_detail() -> SkinMaterial {
    SkinMaterial {
        wrinkles: 1.4,
        bunch: 0.016,
        ..Default::default()
    }
}

/// A person (athlete or shirt) with locomotion graph, muscles, foot IK (and a ponytail).
pub fn spawn_humanoid(world: &mut World, outfit: humanoid::Outfit, transform: Transform) -> Entity {
    let name = match outfit {
        humanoid::Outfit::Athlete => "Athlete",
        humanoid::Outfit::Lifter => "Lifter",
        humanoid::Outfit::Shirt => "Shirt",
    };
    let person = world
        .spawn((Name::new(name), transform, GlobalTransform::from(transform)))
        .id();
    give_humanoid_body(world, person, outfit);
    person
}

/// Give an entity that already has its place the generated body of a person: the humanoid
/// rig with its locomotion graph, muscles and foot IK, and the athlete's ponytail.
pub fn give_humanoid_body(world: &mut World, person: Entity, outfit: humanoid::Outfit) {
    let (key, material) = match outfit {
        humanoid::Outfit::Athlete => ("anim/humanoid/athlete", SkinMaterial::default()),
        humanoid::Outfit::Lifter => ("anim/humanoid/lifter", SkinMaterial::default()),
        humanoid::Outfit::Shirt => ("anim/humanoid/shirt", shirt_detail()),
    };
    let handle = mesh_handle(world, key, || humanoid::mesh(outfit));
    let mut anim = animator(world, humanoid::SKELETON, humanoid::GRAPH);
    let morphs = morphs(world, handle);
    // Keep the pace and head start the entity was given.
    let given = world
        .get::<AnimatorSettings>(person)
        .cloned()
        .unwrap_or_default();
    anim.preroll(given.start_secs);
    let mut e = world.entity_mut(person);
    e.insert((
        AnimatorSettings {
            speed: given.speed,
            start_secs: given.start_secs,
            ..AnimatorSettings::new(humanoid::SKELETON, humanoid::GRAPH)
        },
        anim,
        SkinnedMeshRenderer::new(handle, material),
        morphs,
        MuscleRig::new(humanoid::muscles()),
        humanoid::foot_ik(),
    ));
    if outfit == humanoid::Outfit::Athlete {
        e.insert(SpringChains::new(vec![humanoid::ponytail()]));
    }
}

/// A fetched file (`assets/fetched/<relative>`, see `assets/manifests/animation.toml`):
/// under `ENGINE_ASSET_ROOT`, the working directory's `assets`, or this workspace's `assets`
/// (tests and examples run from the crate folder). `None` if it hasn't been fetched.
pub fn fetched_path(relative: &str) -> Option<String> {
    let roots = [
        std::env::var("ENGINE_ASSET_ROOT")
            .ok()
            .map(std::path::PathBuf::from),
        Some("assets".into()),
        Some(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into()),
    ];
    roots
        .into_iter()
        .flatten()
        .map(|root| root.join("fetched").join(relative))
        .find(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
}

/// The root-motion takes motion matching searches, fetched as `anim/moves/{m,f}_<name>.glb`.
pub const MOVES: [&str; 28] = [
    "run_fast_01",
    "run_neutral_01",
    "run_neutral_02",
    "run_slow_01",
    "run_start",
    "run_stop",
    "run_to_walk",
    "turn_left_120",
    "turn_left_120_to_walk",
    "turn_left_180",
    "turn_left_180_to_walk",
    "turn_left_60",
    "turn_left_60_to_walk",
    "turn_left_90",
    "turn_right_120",
    "turn_right_120_to_walk",
    "turn_right_180",
    "turn_right_180_to_walk",
    "turn_right_60",
    "turn_right_60_to_walk",
    "turn_right_90",
    "walk_fast_01",
    "walk_neutral_01",
    "walk_neutral_02",
    "walk_slow_01",
    "walk_start",
    "walk_stop",
    "walk_to_run",
];

/// The fetched [`MOVES`] of a set (`m` or `f`).
pub fn moves(set: &str) -> Vec<String> {
    MOVES
        .iter()
        .filter_map(|m| fetched_path(&format!("anim/moves/{set}_{m}.glb")))
        .collect()
}

/// One of the paddock's detailed people (`athlete`, `lifter`, `reacher`, `walker`) with its
/// motion-capture locomotion, if they have been fetched (`cargo run -p fetch-assets --
/// animation`).
pub fn character_model(name: &str) -> Option<HumanModel> {
    let path = fetched_path(&format!("anim/characters/{name}.glb"))?;
    let set = if name == "athlete" { "f" } else { "m" };
    let clip = |c: &str| fetched_path(&format!("anim/clips/{set}_{c}.glb")).unwrap_or_default();
    Some(HumanModel {
        idle: clip("idle_neutral_01"),
        walk: clip("walk_neutral_01"),
        run: clip("run_neutral_01"),
        moves: moves(set),
        ..HumanModel::new(path)
    })
}

/// Give a person one of the paddock's detailed characters (see [`HumanModel`]) when it has
/// been fetched; otherwise they keep their generated body.
pub fn dress(world: &mut World, person: Entity, character: &str, material: SkinMaterial) {
    static MISSING: std::sync::Once = std::sync::Once::new();
    match character_model(character) {
        Some(model) => {
            world
                .entity_mut(person)
                .insert(model.with_material(material));
        }
        None => MISSING.call_once(|| {
            tracing::info!(
                "animation paddock: detailed people not fetched (cargo run -p fetch-assets -- \
                 animation); using generated bodies"
            )
        }),
    }
}

/// A part of the hammer at `offset` in the right hand's frame. Between swings and blocks the
/// hammer hangs from the fist beside the leg, head down just off the ground.
fn hammer_part(person: Entity, offset: Vec3, rotation: Quat) -> BoneAttachment {
    BoneAttachment::new(person, "hand_r")
        .with_offset(offset, rotation)
        .with_hang(Hang {
            pivot: GRIP,
            fist: -GRIP.y,
            palm: Vec3::new(-0.025, 0.0, 0.0),
            axis: HANDLE_DIR.normalize(),
            toward: Vec3::new(0.12, -1.0, -0.22),
            curves: vec!["grip".into(), "brace".into()],
        })
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
                hammer_part(person, offset, rotation),
            ))
            .id()
    });
    world.entity_mut(person).insert(HandGrip {
        left: 0.0,
        right: 1.0,
    });
    world.entity_mut(person).insert(ReachIk::new(vec![
        Reach {
            target_bone: "hand_r".into(),
            // The left wrist sits where the left fist closes around the handle's end.
            target_offset: -dir * 0.11,
            pole: Vec3::new(-0.5, -1.0, 0.3),
            weight_curve: "grip".into(),
            ..Reach::arm(0)
        },
        Reach {
            target_bone: "hand_r".into(),
            // Blocking, the hands are spread along the handle, the left just below the head;
            // its wrist sits below and behind the handle (in the block's right-hand frame)
            // so the fist closes over it.
            target_offset: GRIP + dir * 0.42 + Vec3::new(-0.07, 0.05, 0.015),
            pole: Vec3::new(-0.6, -1.0, 0.2),
            weight_curve: "guard".into(),
            ..Reach::arm(0)
        },
    ]));
    ids
}

/// Where the spectators stand (z) in front of the muscle stage, facing it.
const SPECTATOR_Z: f64 = 9.0;
/// The spectators: x, seed, sex (0 female .. 1 male), age range, ancestry weights (african,
/// asian, european), whether they watch the tire (or glance about), and when they wince (s
/// into the athlete's round; the strike lands at 1.56 s).
/// The weapons yard's people.
struct Yard {
    swordsman: Entity,
    shield_partner: Entity,
    greatsword: Entity,
    dummy: Entity,
    archer: Entity,
}

/// An archer in the paddock: on each `release` of its bow shot, an arrow flies from the draw
/// hand to `at` (world), a hand's width apart shot to shot.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoArcher {
    pub at: DVec3,
    pub shots: u32,
}

/// The weapons yard: a sword-and-shield exchange (a combo of two cuts and a thrust, an
/// overhead the partner's raised shield takes, a slash into a shield bash and a thrust; then
/// the partner attacks into the fighter's shield), a great sword against a dummy (a sweep, a
/// rising cut and an overhead that knocks it down) and an archer shooting at a butt 12 m off
/// (a quick shot, a full draw held on the anchor, another quick one).
fn weapons_yard(world: &mut World, origin: DVec3) -> Yard {
    let at = |p: DVec3, x: f64, z: f64| origin + p + DVec3::new(x, 0.0, z);
    let facing = |yaw_deg: f32| Quat::from_rotation_y(yaw_deg.to_radians());
    // A floor flush with the sand under the whole yard, for the knocked-down to land on.
    let floor = Transform::from_translation(origin + DVec3::new(-21.5, -0.25, -18.0));
    world.spawn((
        floor,
        GlobalTransform::from(floor),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(6.5, 0.25, 7.5)),
    ));

    // --- Sword and shield: the swordsman on the west, facing east, the partner a step and a
    // half away facing back.
    let swordsman = spawn_humanoid(
        world,
        humanoid::Outfit::Athlete,
        Transform::from_translation(at(DUEL, -0.7, 0.0)).with_rotation(facing(-90.0)),
    );
    dress(world, swordsman, "athlete", SkinMaterial::default());
    let shield_partner = spawn_humanoid(
        world,
        humanoid::Outfit::Shirt,
        Transform::from_translation(at(DUEL, 0.7, 0.0)).with_rotation(facing(90.0)),
    );
    dress(world, shield_partner, "lifter", shirt_detail());
    let light = |at: f32| DemoStep::trigger(at, "attack_light");
    let heavy = |at: f32| DemoStep::trigger(at, "attack_heavy");
    let guard = |at: f32, on: bool| DemoStep::param(at, "shield_guard", if on { 1.0 } else { 0.0 });
    let stance = DemoStep::param(0.02, "sword_stance", 1.0);
    let duel = |steps: Vec<DemoStep>| DemoScript {
        period: DUEL_PERIOD,
        steps,
        time: 0.0,
        home: None,
    };
    for (person, foe, name, steps) in [
        (
            swordsman,
            shield_partner,
            "Swordsman",
            vec![
                stance.clone(),
                // Slash, backhand, thrust.
                light(0.6),
                light(1.02),
                light(1.42),
                DemoStep::home(3.0),
                // An overhead into the raised shield: it bounces off, and the partner cuts
                // back at once while the swordsman recovers.
                heavy(4.0),
                DemoStep::home(6.3),
                // A slash, the shield rammed in, a thrust.
                light(6.6),
                heavy(7.0),
                light(7.45),
                DemoStep::home(8.6),
                // Its shield comes up just as the partner's cut arrives: a deflect, the
                // partner staggers open and the swordsman answers.
                guard(9.15, true),
                guard(9.4, false),
                light(9.42),
                DemoStep::home(12.0),
            ],
        ),
        (
            shield_partner,
            swordsman,
            "Shield partner",
            vec![
                stance.clone(),
                DemoStep::home(2.6),
                guard(3.4, true),
                // The guard counter, as the swordsman's overhead bounces off.
                guard(4.7, false),
                light(4.75),
                DemoStep::home(6.3),
                light(9.0),
                DemoStep::home(12.0),
            ],
        ),
    ] {
        props::spawn_sword(world, person);
        props::spawn_shield(world, person);
        world.entity_mut(person).insert((
            Name::new(name),
            // Knocked back a step by what gets through, never off their feet: the exchange
            // keeps its rhythm (the great sword's dummy shows the knockdown).
            HitReaction {
                fall_beyond: 0.0,
                ..Default::default()
            },
            // Blows don't wind or daze them (a hurt fighter drops its guard): the exchange
            // shows the moves, round after round.
            {
                let mut hurt = crate::hurt::Hurt::default();
                hurt.enabled = false;
                hurt
            },
            crate::combat::Attacker::new("sword_shield", Some(foe)),
            duel(steps),
        ));
    }

    // --- Great sword against a dummy with a shield up: the sweep bounces off it, the
    // overhead breaks the guard, a sweep gets through the opening and a second overhead
    // knocks the dummy down.
    let greatsword = spawn_humanoid(
        world,
        humanoid::Outfit::Athlete,
        Transform::from_translation(at(GREATSWORD, -0.8, 0.0)).with_rotation(facing(-90.0)),
    );
    dress(world, greatsword, "athlete", SkinMaterial::default());
    let dummy = spawn_humanoid(
        world,
        humanoid::Outfit::Shirt,
        Transform::from_translation(at(GREATSWORD, 0.8, 0.0)).with_rotation(facing(90.0)),
    );
    dress(world, dummy, "walker", SkinMaterial::default());
    props::spawn_shield(world, dummy);
    world.entity_mut(dummy).insert((
        Name::new("Great sword dummy"),
        HitReaction {
            // The light cuts knock it back a step; the overhead puts it down.
            fall_beyond: 0.7,
            down_secs: 3.0,
            ..Default::default()
        },
        DemoScript {
            period: GREATSWORD_PERIOD,
            steps: vec![
                guard(0.3, true),
                // Its guard broken, it doesn't get it back up in time.
                guard(3.6, false),
                DemoStep::home(9.0),
            ],
            time: 0.0,
            home: None,
        },
    ));
    props::spawn_greatsword(world, greatsword);
    world.entity_mut(greatsword).insert((
        Name::new("Great sword"),
        HitReaction::default(),
        crate::combat::Attacker::new("greatsword", Some(dummy)),
        DemoScript {
            period: GREATSWORD_PERIOD,
            steps: vec![
                DemoStep::param(0.02, "greatsword_stance", 1.0),
                // The sweep bounces off the shield.
                light(0.8),
                // The overhead breaks the guard, the sweep goes through the opening, and a
                // second overhead puts the dummy down.
                heavy(2.25),
                light(3.4),
                heavy(4.25),
                DemoStep::home(8.5),
            ],
            time: 0.0,
            home: None,
        },
    ));

    // --- Archery: a straw butt on a stand, its face to the archer, and the archer.
    let gold = at(BUTT, 0.0, 0.0) + DVec3::Y * BUTT_GOLD;
    let straw = StandardMaterial::hex(0xc8a868).with_roughness(0.95);
    let stand = StandardMaterial::preset(MaterialPreset::WoodPlanks).with_tint(0.55, 0.47, 0.4);
    // The face is a disc square to Z (a cylinder's axis is its Y).
    let face = Quat::from_rotation_x(PI / 2.0);
    let butt =
        |world: &mut World, shape: MeshShape, material: StandardMaterial, p: DVec3, r: Quat| {
            let t = Transform::from_translation(p).with_rotation(r);
            world
                .spawn((
                    t,
                    GlobalTransform::from(t),
                    MeshRenderer::new(shape, material),
                ))
                .id()
        };
    let target = butt(
        world,
        MeshShape::Cylinder {
            radius: 0.62,
            height: 0.3,
            segments: 32,
        },
        straw,
        gold,
        face,
    );
    world.entity_mut(target).insert(Name::new("Archery butt"));
    // Rings on the face, outermost first, each a little proud of the one behind.
    for (k, (radius, color)) in [
        (0.5, 0xf2f0ea),
        (0.4, 0x202226),
        (0.3, 0x2e7fd0),
        (0.2, 0xd23a2c),
        (0.1, 0xf2c81e),
    ]
    .into_iter()
    .enumerate()
    {
        butt(
            world,
            MeshShape::Cylinder {
                radius,
                height: 0.004,
                segments: 32,
            },
            StandardMaterial::hex(color).with_roughness(0.7),
            gold + DVec3::Z * (0.151 + 0.002 * k as f64),
            face,
        );
    }
    for x in [-0.42, 0.42] {
        butt(
            world,
            MeshShape::Cuboid {
                size: [0.08, 1.9, 0.08],
            },
            stand,
            at(BUTT, x, -0.3) + DVec3::Y * 0.95,
            Quat::from_rotation_x(-0.2),
        );
    }
    let archer = spawn_humanoid(
        world,
        humanoid::Outfit::Shirt,
        Transform::from_translation(at(ARCHER, 0.0, 0.0)),
    );
    dress(world, archer, "reacher", shirt_detail());
    props::spawn_bow(world, archer);
    world.entity_mut(archer).insert((
        Name::new("Archer"),
        crate::combat::Attacker::new("bow", Some(target)),
        DemoArcher {
            at: gold + DVec3::Z * 0.16,
            shots: 0,
        },
        DemoScript {
            period: ARCHERY_PERIOD,
            steps: vec![
                DemoStep::param(0.02, "bow_carry", 1.0),
                light(1.0),
                heavy(3.4),
                light(6.6),
            ],
            time: 0.0,
            home: None,
        },
    ));
    Yard {
        swordsman,
        shield_partner,
        greatsword,
        dummy,
        archer,
    }
}

/// On a paddock archer's `release`, an arrow flies from the draw hand to the butt.
fn loose_arrows(
    mut commands: Commands,
    mut events: MessageReader<engine_world::animation::AnimationEvent>,
    mut archers: Query<(&mut DemoArcher, &Animator, &GlobalTransform)>,
) {
    for e in events.read() {
        if e.name != "release" {
            continue;
        }
        let Ok((mut archer, animator, gt)) = archers.get_mut(e.entity) else {
            continue;
        };
        let Some(hand) = animator.skeleton.index_of("hand_r") else {
            continue;
        };
        let from = animator.bone_world(gt, hand).0;
        // A spiral of hits around the gold.
        let k = archer.shots as f64;
        archer.shots += 1;
        let angle = k * 2.4;
        let spread = 0.05 + 0.04 * (k % 4.0);
        let to = archer.at + DVec3::new(angle.cos(), angle.sin(), 0.0) * spread;
        commands.queue(move |world: &mut World| {
            props::shoot_arrow(world, from, to, 55.0);
        });
    }
}

#[allow(clippy::type_complexity)]
const SPECTATORS: [(f64, u64, f32, [f32; 2], [f32; 3], bool, f32); 3] = [
    (-1.5, 401, 0.0, [28.0, 34.0], [0.7, 0.1, 0.2], true, 1.5),
    (-2.6, 402, 1.0, [55.0, 65.0], [0.1, 0.1, 0.8], false, 1.6),
    (-3.7, 403, 1.0, [19.0, 23.0], [0.1, 0.8, 0.1], true, 1.45),
];

/// Where the reacher stands on the muscle stage (x, z; it faces +Z).
const REACHER: DVec3 = DVec3::new(3.1, 0.0, 5.8);

/// The middle of the lantern the reacher takes hold of, for a paddock at `origin`: a little
/// in front of them and to their right, just above what their wrist reaches flat-footed.
pub fn lantern_grip(origin: DVec3) -> DVec3 {
    origin + DVec3::new(REACHER.x - 0.12, 0.4 + 2.04, REACHER.z + 0.25)
}

/// Where the reacher's right wrist goes to hold the lantern: below the glass and beside it on
/// their right (world −X), so the palm closes on its side.
pub fn lantern_wrist(origin: DVec3) -> DVec3 {
    lantern_grip(origin) + DVec3::new(-0.095, -0.03, 0.0)
}

/// The point on the tire's near rim the hammer should hit, for a paddock at `origin`.
pub fn tire_strike_point(origin: DVec3) -> DVec3 {
    origin + DVec3::new(ATHLETE.x, 0.72, ATHLETE.z + TIRE_AHEAD - 0.45)
}

/// Where the hammer head is, in world space (for tests and cameras).
pub fn hammer_head(world: &World, person: Entity) -> Option<DVec3> {
    let animator = world.get::<Animator>(person)?;
    let gt = world.get::<GlobalTransform>(person)?;
    let head = hammer_part(person, GRIP + HANDLE_DIR.normalize() * 0.6, Quat::IDENTITY);
    Some(crate::ik::to_world(gt, head.socket(animator)?.t))
}

/// A horse; it wears the realistic horse model once that has loaded (when fetched).
pub fn spawn_horse(world: &mut World, transform: Transform) -> Entity {
    let horse = world
        .spawn((
            Name::new("Horse"),
            transform,
            GlobalTransform::from(transform),
        ))
        .id();
    give_horse_body(world, horse, horse_model());
    if horse_model().is_none() {
        static MISSING: std::sync::Once = std::sync::Once::new();
        MISSING.call_once(|| {
            tracing::info!(
                "animation paddock: horse model not fetched (cargo run -p fetch-assets -- \
                 animation); using the generated horse"
            )
        });
    }
    horse
}

/// Give an entity that already has its place a horse: the horse rig with its gaits, muscles
/// and tail, wearing `model` once it has loaded (the generated horse until then).
pub fn give_horse_body(world: &mut World, horse: Entity, model: Option<HorseModel>) {
    let handle = mesh_handle(world, "anim/horse", horse::mesh);
    let anim = animator(world, horse::SKELETON, horse::GRAPH);
    let morphs = morphs(world, handle);
    let mut e = world.entity_mut(horse);
    e.insert((
        AnimatorSettings::new(horse::SKELETON, horse::GRAPH),
        anim,
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
    ));
    if let Some(model) = model {
        e.insert(model);
    }
}

/// The paddock's realistic horse (see [`HorseModel`]), if it has been fetched.
pub fn horse_model() -> Option<HorseModel> {
    Some(HorseModel {
        path: fetched_path(HORSE_FILE)?,
        // The file's horse stands 1.9 m at the withers: make it a 1.6 m riding horse.
        scale: 0.85,
    })
}

pub fn spawn_dog(world: &mut World, transform: Transform, scale: f32) -> Entity {
    let dog = world
        .spawn((
            Name::new("Dog"),
            transform,
            GlobalTransform::from(transform),
        ))
        .id();
    give_dog_body(world, dog, scale);
    dog
}

/// Give an entity that already has its place a dog of `scale` (1 = 0.56 m at the withers):
/// the furred coat, four paws on their gaits, ears and tail on springs, a panting idle.
pub fn give_dog_body(world: &mut World, dog: Entity, scale: f32) {
    let skeleton = dog::skeleton_name(scale);
    {
        let mut lib = world.resource_mut::<AnimationLibrary>();
        if !lib.skeletons.contains_key(&skeleton) {
            lib.add_skeleton(dog::skeleton(scale));
        }
    }
    let handle = mesh_handle(world, &format!("anim/{skeleton}"), || dog::mesh(scale));
    let anim = animator(world, &skeleton, dog::GRAPH);
    let morphs = morphs(world, handle);
    world.entity_mut(dog).insert((
        AnimatorSettings::new(&skeleton, dog::GRAPH),
        anim,
        SkinnedMeshRenderer::new(handle, dog::material(scale)),
        morphs,
        MuscleRig::new(dog::muscles()),
        dog::gait(scale),
        SpringChains::new(dog::springs(scale)),
    ));
}

pub fn spawn_spider(world: &mut World, transform: Transform) -> Entity {
    let spider = world
        .spawn((
            Name::new("Spider"),
            transform,
            GlobalTransform::from(transform),
        ))
        .id();
    give_spider_body(world, spider);
    spider
}

/// Give an entity that already has its place the spider: a furred tarantula on eight
/// procedural legs.
pub fn give_spider_body(world: &mut World, spider: Entity) {
    let handle = mesh_handle(world, "anim/spider", spider::mesh);
    let anim = animator(world, spider::SKELETON, spider::GRAPH);
    world.entity_mut(spider).insert((
        AnimatorSettings::new(spider::SKELETON, spider::GRAPH),
        anim,
        SkinnedMeshRenderer::new(handle, spider::material()),
        spider::gait(),
        SpringChains::new(spider::springs()),
    ));
}

/// The paddock car's paint (linear RGB).
const CAR_PAINT: [f32; 3] = [0.55, 0.04, 0.02];

fn car_key(paint: [f32; 3]) -> String {
    format!("anim/car/{:.2}_{:.2}_{:.2}", paint[0], paint[1], paint[2])
}

/// The paddock car: the detailed concept car when it has been fetched (and the world loads
/// glTF files), else a procedural car in `paint` (linear RGB).
pub fn spawn_car(world: &mut World, transform: Transform, paint: [f32; 3]) -> Entity {
    if let Some(scene) = concept_car_scene(world) {
        let skeleton = world
            .resource::<AnimationLibrary>()
            .skeletons
            .get(car::CONCEPT_SKELETON)
            .cloned()
            .expect("concept car skeleton");
        return world
            .spawn((
                Name::new("Car"),
                transform,
                GlobalTransform::from(transform),
                AnimatorSettings::new(car::CONCEPT_SKELETON, ""),
                Animator::procedural(skeleton),
                AnimationParams::default(),
                car::concept_rig(),
                car::concept_model(scene),
            ))
            .id();
    }
    let handle = mesh_handle(world, &car_key(paint), || car::mesh(paint));
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

/// The concept car's glTF scene, loading, if the file has been fetched and the world has an
/// asset server with the glTF loader (the renderer's).
/// A parked concept car whose doors open and shut ([`car::concept_door_clip`]); `None`
/// without the fetched concept car (the generated car has no doors).
pub fn spawn_parked_car(world: &mut World, transform: Transform) -> Option<Entity> {
    let scene = concept_car_scene(world)?;
    let anim = animator(world, car::CONCEPT_SKELETON, car::CONCEPT_PARKED_GRAPH);
    Some(
        world
            .spawn((
                Name::new("Parked car"),
                transform,
                GlobalTransform::from(transform),
                AnimatorSettings::new(car::CONCEPT_SKELETON, car::CONCEPT_PARKED_GRAPH),
                anim,
                AnimationParams::default(),
                car::concept_rig(),
                car::concept_model(scene),
            ))
            .id(),
    )
}

fn concept_car_scene(world: &mut World) -> Option<engine_assets::Handle<GltfScene>> {
    static MISSING: std::sync::Once = std::sync::Once::new();
    let fetched = world
        .get_resource::<AssetServer>()
        .is_some_and(|server| server.root().join(car::CONCEPT_FILE).exists());
    if !fetched || !world.contains_resource::<Assets<GltfScene>>() {
        MISSING.call_once(|| {
            tracing::info!(
                "animation paddock: concept car not fetched (cargo run -p fetch-assets -- \
                 animation) or no glTF loader; using the generated car"
            )
        });
        return None;
    }
    Some(
        world.resource_scope(|world, mut scenes: Mut<Assets<GltfScene>>| {
            world
                .resource::<AssetServer>()
                .load(&mut scenes, car::CONCEPT_FILE)
        }),
    )
}

/// Spawn every exhibit around `origin`. Returns the main characters.
pub fn spawn_demo_world(world: &mut World, origin: DVec3) -> DemoEntities {
    let before: HashSet<Entity> = world.query::<Entity>().iter(world).collect();
    let entities = build_paddock(world, origin);
    let parts = world
        .query::<Entity>()
        .iter(world)
        .filter(|e| !before.contains(e))
        .collect();
    world.spawn(Paddock {
        origin,
        entities,
        parts,
    });
    entities
}

/// A spawned paddock: where it stands and everything it spawned, so it can start over.
#[derive(Component, Debug, Clone)]
pub struct Paddock {
    pub origin: DVec3,
    pub entities: DemoEntities,
    parts: Vec<Entity>,
}

/// Despawn every paddock and spawn it again in the same place, so its test cases start from
/// the beginning (camera stops time their shots from here, see [`VIEWS`]).
pub fn restart_demos(world: &mut World) -> Vec<DemoEntities> {
    let paddocks: Vec<(Entity, Paddock)> = world
        .query::<(Entity, &Paddock)>()
        .iter(world)
        .map(|(e, p)| (e, p.clone()))
        .collect();
    paddocks
        .into_iter()
        .map(|(root, paddock)| {
            for e in paddock.parts.into_iter().chain([root]) {
                if world.get_entity(e).is_ok() {
                    world.despawn(e);
                }
            }
            spawn_demo_world(world, paddock.origin)
        })
        .collect()
}

/// A close-up of one test case, for camera bookmarks and screenshot tours: eye and target
/// relative to the paddock's origin, the hour it looks best at, and when the moment comes,
/// in seconds after `anim.demo_restart`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    pub at: f32,
}

/// One close-up per test case (framed for a 55° camera).
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "paddock_hammer",
        name: "Sledgehammer strike: arms, shoulders and back tense",
        eye: [-4.74, 1.59, 7.23],
        target: [-2.6, 1.3, 5.9],
        hour: 15.0,
        at: 1.57,
    },
    DemoView {
        key: "paddock_flex",
        name: "Front double-biceps: curl up, hit and squeeze",
        eye: [0.47, 1.8, 7.19],
        target: [0.7, 1.7, 5.6],
        hour: 15.0,
        at: 2.92,
    },
    DemoView {
        key: "paddock_reach",
        name: "Up on tiptoe to the lantern, shirt riding up the raised arm",
        eye: [2.2, 1.75, 8.0],
        target: [3.0, 1.85, 5.95],
        hour: 15.0,
        at: 2.2,
    },
    DemoView {
        key: "paddock_block",
        name: "Braced guard: the hammer handle across the face in both hands",
        eye: [-4.6, 1.55, 7.7],
        target: [-2.65, 1.15, 5.4],
        hour: 15.0,
        at: 4.82,
    },
    DemoView {
        key: "paddock_crowd",
        name: "Spectators watching the hammer: blinking, following it, glancing about",
        eye: [-2.6, 1.45, 6.5],
        target: [-2.6, 1.5, 9.0],
        hour: 15.0,
        at: 1.0,
    },
    DemoView {
        key: "paddock_crowd_wince",
        name: "A spectator winces as the hammer lands",
        eye: [-1.6, 1.4, 8.3],
        target: [-1.5, 1.36, 9.0],
        hour: 15.0,
        at: 1.75,
    },
    DemoView {
        key: "paddock_crowd_glance",
        name: "A spectator glances about between strikes",
        eye: [-2.62, 1.66, 8.35],
        target: [-2.6, 1.62, 9.0],
        hour: 15.0,
        at: 4.5,
    },
    DemoView {
        key: "paddock_stairs",
        name: "Walking down stairs watching the steps, feet planted by IK",
        eye: [-13.15, 1.51, -10.38],
        target: [-12.6, 1.19, -8.01],
        hour: 15.0,
        at: 9.97,
    },
    DemoView {
        key: "paddock_feet_ramp",
        name: "Feet on the ramp, side-on at foot level",
        eye: [-6.8, 0.42, -4.6],
        target: [-10.0, 0.55, -4.6],
        hour: 15.0,
        at: 4.6,
    },
    DemoView {
        key: "paddock_feet_stairs",
        name: "Feet on the stairs, side-on at foot level",
        eye: [-13.0, 0.5, -4.9],
        target: [-13.0, 0.55, -8.0],
        hour: 15.0,
        at: 10.2,
    },
    DemoView {
        key: "paddock_gallop",
        name: "Horse at a gallop, haunches working",
        eye: [22.87, 1.52, -2.65],
        target: [23.33, 1.2, -6.85],
        hour: 15.0,
        at: 11.78,
    },
    DemoView {
        key: "paddock_spider",
        name: "A furred tarantula climbing boulders",
        eye: [9.99, 1.2, 10.66],
        target: [7.24, 0.35, 10.16],
        hour: 15.0,
        at: 6.02,
    },
    DemoView {
        key: "paddock_spider_threat",
        name: "The tarantula rears up in threat, front legs high and fangs out",
        eye: [17.3, 0.75, 10.5],
        target: [16.0, 0.5, 12.4],
        hour: 15.0,
        at: 21.5,
    },
    DemoView {
        key: "paddock_dog_trot",
        name: "Dog trotting, its coat in the sun",
        eye: [-4.6, 0.7, 10.0],
        target: [-5.7, 0.4, 12.0],
        hour: 15.0,
        at: 3.5,
    },
    DemoView {
        key: "paddock_dog",
        name: "Dog at a full rotary gallop",
        eye: [-5.44, 0.65, 22.6],
        target: [-6.3, 0.32, 20.0],
        hour: 15.0,
        at: 9.0,
    },
    DemoView {
        key: "paddock_car",
        name: "Car suspension over a speed bump",
        eye: [0.05, 1.28, -20.59],
        target: [3.0, 0.64, -16.82],
        hour: 15.0,
        at: 4.57,
    },
    DemoView {
        key: "paddock_doors",
        name: "Parked car opening its doors",
        eye: [-0.6, 1.5, -18.4],
        target: [-4.0, 0.75, -21.6],
        hour: 15.0,
        at: 3.6,
    },
    DemoView {
        key: "paddock_face",
        name: "Face under effort: brows knit, eyes narrowed, teeth gritted",
        eye: [0.7, 2.06, 6.45],
        target: [0.7, 2.02, 5.6],
        hour: 15.0,
        at: 1.6,
    },
    DemoView {
        key: "paddock_stunt",
        name: "Stunt fall: off the deck with the run's momentum, braced for the mat",
        eye: [0.0, 1.7, 34.6],
        target: [0.4, 1.2, 28.0],
        hour: 15.0,
        at: 6.55,
    },
    DemoView {
        key: "paddock_stunt_down",
        name: "Ragdoll come to rest on the crash mat, face slack",
        eye: [5.0, 0.95, 29.6],
        target: [3.6, 0.5, 28.1],
        hour: 15.0,
        at: 8.3,
    },
    DemoView {
        key: "paddock_stunt_up",
        name: "Getting up off the mat: a hand and a knee planted",
        eye: [5.6, 1.25, 31.2],
        target: [3.0, 0.6, 27.9],
        hour: 15.0,
        at: 9.9,
    },
    DemoView {
        key: "paddock_footwork",
        name: "Sidestepping round a square, feet planted",
        eye: [-3.2, 1.6, -9.2],
        target: [-6.0, 0.8, -4.6],
        hour: 15.0,
        at: 5.5,
    },
    DemoView {
        key: "paddock_flinch",
        name: "In a boxer's guard, a jab to the chin snaps the head back",
        eye: [-3.6, 1.35, 1.2],
        target: [-6.0, 1.15, 1.0],
        hour: 15.0,
        at: SPARRING_COMBOS + 0.9,
    },
    DemoView {
        key: "paddock_combo",
        name: "Jab, cross, hook: the boxer steps in, each punch freezing both for a beat",
        eye: [-3.9, 1.5, 1.4],
        target: [-6.0, 1.25, 1.5],
        hour: 15.0,
        at: SPARRING_COMBOS + 1.45,
    },
    DemoView {
        key: "paddock_knockdown",
        name: "A heavy uppercut knocks the sparring partner down, braced; they get up",
        eye: [-3.0, 1.4, 0.2],
        target: [-6.0, 0.7, 0.2],
        hour: 15.0,
        at: SPARRING_COMBOS + 7.4,
    },
    DemoView {
        key: "paddock_body_shot",
        name: "A hook to the liver under the guard: a beat later the partner folds onto a knee",
        eye: [-3.3, 1.3, 2.2],
        target: [-6.0, 0.8, 1.3],
        hour: 15.0,
        at: SPARRING_BODY + 2.0,
    },
    DemoView {
        key: "paddock_slip",
        name: "The sparring partner slips the jab and cross, rolls under the hook and counters",
        eye: [-3.2, 1.45, 1.6],
        target: [-6.0, 1.2, 1.5],
        hour: 15.0,
        at: 1.1,
    },
    DemoView {
        key: "paddock_freeflow",
        name: "Chaining: a dummy drops and the brawler leaps straight at the next one",
        eye: [0.5, 1.6, -4.4],
        target: [0.5, 0.9, -10.0],
        hour: 15.0,
        at: 4.0,
    },
    DemoView {
        key: "paddock_sword",
        name: "Sword and shield: a slash, a rising backhand and a thrust, the blade leading",
        eye: [-24.3, 1.45, -13.4],
        target: [-24.0, 1.1, -16.0],
        hour: 15.0,
        at: 1.2,
    },
    DemoView {
        key: "paddock_shield",
        name: "An overhead bounces off a raised shield; the partner cuts back at once",
        eye: [-22.4, 1.5, -13.8],
        target: [-23.9, 1.2, -16.0],
        hour: 15.0,
        at: 4.75,
    },
    DemoView {
        key: "paddock_bash",
        name: "A slash, the shield rammed in, and a thrust",
        eye: [-25.6, 1.4, -13.9],
        target: [-23.9, 1.1, -16.0],
        hour: 15.0,
        at: 7.15,
    },
    DemoView {
        key: "paddock_deflect",
        name: "A shield raised just as the cut lands swats it aside; the attacker staggers open",
        eye: [-25.7, 1.5, -14.2],
        target: [-24.0, 1.2, -16.0],
        hour: 15.0,
        at: 9.3,
    },
    DemoView {
        key: "paddock_greatsword",
        name: "Great sword: the overhead from high guard breaks the dummy's raised shield",
        eye: [-23.8, 1.5, -18.1],
        target: [-24.0, 1.1, -21.0],
        hour: 15.0,
        at: 3.15,
    },
    DemoView {
        key: "paddock_bow",
        name: "Archer: drawn to an anchor under the jaw, loosed at the butt 12 m off",
        eye: [-15.9, 1.55, -12.9],
        target: [-18.0, 1.35, -12.2],
        hour: 15.0,
        at: 1.9,
    },
    DemoView {
        key: "paddock_butt",
        name: "The archer's arrows thud into the butt round the gold and stay",
        eye: [-16.9, 1.55, -21.6],
        target: [-18.0, 1.3, -24.0],
        hour: 15.0,
        at: 7.8,
    },
    DemoView {
        key: "paddock_sprint_start",
        name: "Driving off into a sprint, leaning into it",
        eye: [-18.6, 1.3, 7.8],
        target: [-23.0, 1.0, 5.2],
        hour: 15.0,
        at: 1.35,
    },
    DemoView {
        key: "paddock_sprint_stop",
        name: "Digging in to stop from a sprint, sitting back; the coach turns to watch",
        eye: [-17.8, 1.4, -10.4],
        target: [-22.6, 0.9, -6.4],
        hour: 15.0,
        at: 4.3,
    },
];

fn build_paddock(world: &mut World, origin: DVec3) -> DemoEntities {
    let at = |x: f64, y: f64, z: f64| origin + DVec3::new(x, y, z);
    let facing = |yaw_deg: f32| Quat::from_rotation_y(yaw_deg.to_radians());
    // The renderer's generated materials, mapped in world space so neighboring pieces line
    // up: oiled planks for the stage, deck and ramp, darker planks for the fence posts, cast
    // concrete steps, rock for the spider garden and raked sand underfoot.
    let wood = StandardMaterial::preset(MaterialPreset::WoodPlanks).with_triplanar_world();
    let dark_wood = StandardMaterial::preset(MaterialPreset::WoodPlanks)
        .with_tint(0.55, 0.47, 0.4)
        .with_triplanar_world();
    let stone = StandardMaterial::preset(MaterialPreset::Concrete).with_triplanar_world();
    let rock = StandardMaterial::preset(MaterialPreset::Rock)
        .with_tint(0.95, 0.9, 0.84)
        .with_triplanar_world();
    let paint_white = StandardMaterial::hex(0xe8e4dc).with_roughness(0.6);
    let sand = StandardMaterial::preset(MaterialPreset::Sand)
        .with_tint(0.92, 0.8, 0.66)
        .with_triplanar_world();

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
            DemoStep::param(3.6, "hammer_guard", 1.0),
            DemoStep::param(6.4, "hammer_guard", 0.0),
            DemoStep::trigger(7.6, "heavy_swing"),
        ],
        time: 0.0,
        home: None,
    });
    dress(world, athlete, "athlete", SkinMaterial::default());
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
    // Spectators on the sand in front of the stage, facing the athlete: people a character
    // module draws (they ask for a look by seed), so their faces blink, follow the hammer
    // and wince at each strike through `FaceControls`. Without one they keep anim's bodies.
    let spectators = SPECTATORS.map(|(x, seed, sex, age, ancestry, watch, wince_at)| {
        let t = Transform::from_translation(at(x, 0.0, SPECTATOR_Z)).with_rotation(facing(0.0));
        let person = world
            .spawn((
                Name::new("Spectator"),
                t,
                GlobalTransform::from(t),
                BodyRequest::human(seed),
                // Each a different way into the standing idle, so they shift their weight
                // at different times.
                AnimatorSettings {
                    start_secs: (seed % 400) as f32 * 3.1,
                    ..Default::default()
                },
                AppearanceRequest {
                    seed,
                    sex: Some(sex),
                    age: Some(age),
                    ancestry: Some(Ancestry {
                        african: ancestry[0],
                        asian: ancestry[1],
                        european: ancestry[2],
                    }),
                    ..Default::default()
                },
                // Each strike lands 1.56 s into the swing (at 0.5 s and 7.6 s of the athlete's
                // 11 s round).
                DemoScript {
                    period: 11.0,
                    steps: [
                        DemoStep::trigger(wince_at, "wince"),
                        DemoStep::trigger(wince_at + 7.1, "wince"),
                    ]
                    .into_iter()
                    // The one who glances about stands with his arms folded.
                    .chain((!watch).then(|| DemoStep::param(0.05, "arms_folded", 1.0)))
                    .collect(),
                    time: 0.0,
                    home: None,
                },
            ))
            .id();
        if watch {
            world.entity_mut(person).insert(LookAt {
                target: at(ATHLETE.x, 0.75, ATHLETE.z + TIRE_AHEAD - 0.3),
                chain: vec![
                    ("chest".into(), 0.15),
                    ("neck".into(), 0.35),
                    ("head".into(), 0.5),
                ],
                max_angle_deg: 70.0,
                weight: 0.8,
                ..Default::default()
            });
        }
        person
    });
    let lifter = spawn_humanoid(
        world,
        humanoid::Outfit::Lifter,
        Transform::from_translation(at(0.7, 0.4, 5.6)).with_rotation(facing(180.0)),
    );
    dress(world, lifter, "lifter", SkinMaterial::default());
    world.entity_mut(lifter).insert(DemoScript {
        period: 6.0,
        steps: vec![DemoStep::trigger(0.8, "flex")],
        time: 0.0,
        home: None,
    });
    let reacher = spawn_humanoid(
        world,
        humanoid::Outfit::Shirt,
        Transform::from_translation(at(REACHER.x, 0.4, REACHER.z)).with_rotation(facing(180.0)),
    );
    dress(world, reacher, "reacher", shirt_detail());
    world.entity_mut(reacher).insert(DemoScript {
        period: 6.0,
        steps: vec![DemoStep::trigger(0.5, "reach")],
        time: 0.0,
        home: None,
    });
    // A lantern hanging from a beam just out of reach: the reacher goes up on tiptoe and
    // takes hold of it, the palm on the glass from their right.
    let lantern = lantern_grip(origin);
    world.entity_mut(reacher).insert(ReachIk::new(vec![Reach {
        target: lantern_wrist(origin),
        pole: Vec3::new(1.0, -0.2, -0.4),
        weight_curve: "reach_hand".into(),
        ..Reach::arm(1)
    }]));
    let beam_y = 0.4 + 2.65;
    for x in [REACHER.x - 0.85, REACHER.x + 0.75] {
        props(
            world,
            Transform::from_translation(at(x, 0.4 + 1.325, lantern.z - origin.z)),
            MeshShape::Cuboid {
                size: [0.1, 2.65, 0.1],
            },
            dark_wood,
        );
    }
    props(
        world,
        Transform::from_translation(at(REACHER.x - 0.05, beam_y - 0.07, lantern.z - origin.z)),
        MeshShape::Cuboid {
            size: [1.8, 0.14, 0.12],
        },
        dark_wood,
    );
    let iron = StandardMaterial::hex(0x2a2826)
        .with_metallic(1.0)
        .with_roughness(0.55);
    let glass = StandardMaterial::hex(0xffd9a0)
        .with_roughness(0.2)
        .with_emissive([1.0, 0.72, 0.38], 2.5);
    let top = lantern.y + 0.11;
    for (y, shape, material) in [
        (
            (beam_y - 0.14 + top) * 0.5,
            MeshShape::Cylinder {
                radius: 0.008,
                height: (beam_y - 0.14 - top) as f32,
                segments: 8,
            },
            iron,
        ),
        (
            top,
            MeshShape::Cylinder {
                radius: 0.082,
                height: 0.03,
                segments: 24,
            },
            iron,
        ),
        (
            lantern.y,
            MeshShape::Cylinder {
                radius: 0.066,
                height: 0.19,
                segments: 24,
            },
            glass,
        ),
        (
            lantern.y - 0.105,
            MeshShape::Cylinder {
                radius: 0.08,
                height: 0.025,
                segments: 24,
            },
            iron,
        ),
    ] {
        props(
            world,
            Transform::from_translation(DVec3::new(lantern.x, y, lantern.z)),
            shape,
            material,
        );
    }

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
    dress(world, walker, "walker", SkinMaterial::default());
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

    // Footwork drill: round a square facing one way, walking ahead, cutting forward and to the
    // left at 45°, sidestepping left, backpedalling and sidestepping right.
    let drill = spawn_humanoid(
        world,
        humanoid::Outfit::Shirt,
        Transform::from_translation(at(-4.5, 0.0, -2.0)),
    );
    dress(world, drill, "athlete", SkinMaterial::default());
    let (ahead, side, back) = (1.2, humanoid::SIDESTEP_SPEED, 0.9);
    world.entity_mut(drill).insert((
        Name::new("Footwork drill"),
        DemoPath {
            facing: Some(0.0),
            ..DemoPath::new(
                origin,
                vec![
                    [-4.5, -2.0, ahead],
                    [-4.5, -3.5, ahead],
                    [-5.25, -4.25, ahead],
                    [-6.0, -5.0, ahead],
                    [-6.75, -5.75, side],
                    [-7.5, -6.0, back],
                    [-7.5, -4.0, back],
                    [-7.5, -2.0, side],
                    [-6.0, -2.0, side],
                ],
            )
        },
    ));

    // Sparring partner: in a boxer's guard. First they slip the boxer's jab and cross and roll
    // under the hook, coming up with a cross that snaps the boxer's head back. Then a hook to
    // the liver gets under the guard and a beat later folds them onto a knee; up again, the guard blocks the first punches of the boxer's
    // combo, they are struck in turn on the side of the head and the chin, the stomach and
    // the back of a shoulder, flinching away from each blow; the last combo ends in a heavy
    // uppercut that knocks them down, and they get up and come back to their mark.
    // The floor under the ring, flush with the sand (the terrain may have no collider here),
    // for a knocked-down partner to land on.
    let floor = Transform::from_translation(at(-6.0, -0.25, 0.5));
    world.spawn((
        floor,
        GlobalTransform::from(floor),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(4.0, 0.25, 4.0)),
    ));
    let (b, c) = (SPARRING_BODY, SPARRING_COMBOS);
    let sparring = spawn_humanoid(
        world,
        humanoid::Outfit::Shirt,
        Transform::from_translation(at(-6.0, 0.0, 1.0)).with_rotation(facing(180.0)),
    );
    dress(world, sparring, "lifter", SkinMaterial::default());
    world.entity_mut(sparring).insert((
        Name::new("Sparring partner"),
        DemoScript {
            period: SPARRING_PERIOD,
            steps: vec![
                DemoStep::param(0.05, "guarding", 1.0),
                // Slipping, rolling and countering (crate::combat::Evader), then back to the
                // mark.
                DemoStep::param(0.05, "evasive", 1.0),
                DemoStep::param(2.8, "evasive", 0.0),
                DemoStep::home(3.2),
                // The guard takes the jab; the body hook (about 1 s in) goes under it to the
                // liver, and half a second later the partner goes down on a knee and gets up
                // (crate::hurt). Back to the mark.
                DemoStep::home(b + 5.8),
                // The boxer's combo comes in between 0.6 and 2 s in (see below): the guard
                // takes the jab and the cross, then drops, so the hook and the uppercut land on
                // the head and knock the partner back; back to the mark, guard up.
                DemoStep::param(c + 1.15, "guarding", 0.0),
                DemoStep::param(c + 2.3, "guarding", 1.0),
                DemoStep::home(c + 2.6),
                DemoStep::hit(
                    c + 3.8,
                    Vec3::new(0.02, 1.1, -0.15),
                    Vec3::new(0.0, 0.0, 32.0),
                ),
                DemoStep::hit(
                    c + 5.0,
                    Vec3::new(0.18, 1.42, 0.08),
                    Vec3::new(0.0, 0.0, -24.0),
                ),
                DemoStep::home(c + 5.4),
                // The second combo (from 6.6 s in): only the jab is blocked; the cross and the
                // hook land, leaving the partner's legs wobbling, and the heavy uppercut knocks
                // them down. Up again, guard up and back to the mark.
                DemoStep::param(c + 6.85, "guarding", 0.0),
                DemoStep::param(c + 12.0, "guarding", 1.0),
                DemoStep::home(c + 12.2),
            ],
            time: 0.0,
            home: None,
        },
    ));

    // Boxer: squares up to the sparring partner, digs a hook to the body, then throws two
    // four-punch combos, each press a little before the last punch's cancel window opens (the
    // buffer holds it), stepping in to range on the jab; each punch that lands freezes both
    // for a few frames.
    let boxer = spawn_humanoid(
        world,
        humanoid::Outfit::Athlete,
        Transform::from_translation(at(-6.0, 0.0, 2.0)),
    );
    dress(world, boxer, "athlete", SkinMaterial::default());
    world.entity_mut(boxer).insert((
        Name::new("Boxer"),
        crate::combat::Attacker::new("boxing", Some(sparring)),
        HitReaction::default(),
        DemoScript {
            period: SPARRING_PERIOD,
            steps: vec![
                DemoStep::param(0.05, "guarding", 1.0),
                // Jab, cross, hook, all slipped or rolled under; the counter lands.
                DemoStep::trigger(0.45, "attack_light"),
                DemoStep::trigger(0.72, "attack_light"),
                DemoStep::trigger(1.15, "attack_light"),
                DemoStep::home(3.0),
                // A jab to the guard, then a heavy press: a hook to the body.
                DemoStep::trigger(b + 0.45, "attack_light"),
                DemoStep::trigger(b + 0.72, "attack_heavy"),
                DemoStep::home(b + 2.6),
                DemoStep::trigger(c + 0.45, "attack_light"),
                DemoStep::trigger(c + 0.72, "attack_light"),
                DemoStep::trigger(c + 1.15, "attack_light"),
                DemoStep::trigger(c + 1.62, "attack_light"),
                // Back to the start line once the combo has carried it after the partner.
                DemoStep::home(c + 3.0),
                // Again, finishing with the heavy uppercut (a heavy press after the hook).
                DemoStep::trigger(c + 6.45, "attack_light"),
                DemoStep::trigger(c + 6.72, "attack_light"),
                DemoStep::trigger(c + 7.15, "attack_light"),
                DemoStep::trigger(c + 7.62, "attack_heavy"),
                DemoStep::home(c + 9.0),
            ],
            time: 0.0,
            home: None,
        },
    ));

    // The partner reads the boxer's punches and counters (while `evasive`).
    world.entity_mut(sparring).insert((
        crate::combat::Evader::new(1.0),
        crate::combat::Attacker::new("boxing", Some(boxer)),
    ));

    // --- Freeflow ring: a brawler and three dummies around it, ahead at 1.2 m, to the left at
    // 2.6 m and to the right at 3.6 m. Tapping the light attack, the stick on one dummy after
    // another (crate::freeflow::Freeflow), it drops each with a jab, a cross and a hook and
    // goes straight at the next: a lunge to the left one, a leap to the right one. The
    // dummies' legs are weak (a low `fall_beyond`): the cross and the hook together put one
    // down. They get up, walk back to their marks, and it starts over.
    let ring = |x: f64, z: f64| at(FREEFLOW.x + x, 0.0, FREEFLOW.z + z);
    // A floor flush with the sand for the knocked-down dummies to land on.
    let floor = Transform::from_translation(at(FREEFLOW.x + 0.5, -0.25, FREEFLOW.z));
    world.spawn((
        floor,
        GlobalTransform::from(floor),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(5.0, 0.25, 3.5)),
    ));
    let dummies: Vec<Entity> = [
        (0.0, -1.2, 180.0, "walker"),
        (-2.6, -0.3, -90.0, "reacher"),
        (3.6, 0.5, 90.0, "lifter"),
    ]
    .into_iter()
    .map(|(x, z, yaw, character)| {
        let d = spawn_humanoid(
            world,
            humanoid::Outfit::Shirt,
            Transform::from_translation(ring(x, z)).with_rotation(facing(yaw)),
        );
        dress(world, d, character, SkinMaterial::default());
        world.entity_mut(d).insert((
            Name::new("Freeflow dummy"),
            HitReaction {
                fall_beyond: 0.28,
                down_secs: 2.5,
                ..Default::default()
            },
            DemoScript {
                period: FREEFLOW_PERIOD,
                steps: vec![DemoStep::home(10.0)],
                time: 0.0,
                home: None,
            },
        ));
        d
    })
    .collect();
    let brawler = spawn_humanoid(
        world,
        humanoid::Outfit::Athlete,
        Transform::from_translation(ring(0.0, 0.0)),
    );
    dress(world, brawler, "athlete", SkinMaterial::default());
    // Taps at a steady pace while each dummy is up; the stick swings to the next dummy as the
    // one before goes down (each falls about 1.5 s after the first tap at it).
    let stick = |at: f32, x: f32, z: f32| {
        [
            DemoStep::param(at, "stick_x", x),
            DemoStep::param(at, "stick_z", z),
        ]
    };
    let taps = |from: f32| {
        [0.0, 0.27, 0.7]
            .map(|t| DemoStep::trigger(from + t, "attack_light"))
            .to_vec()
    };
    let mut steps = vec![DemoStep::param(0.05, "guarding", 1.0)];
    steps.extend(stick(0.05, 0.0, -1.0));
    steps.extend(taps(0.5));
    steps.extend(stick(1.6, -1.0, 0.0));
    steps.extend(taps(1.7).into_iter().map(|mut s| {
        // The lunge takes a beat before the jab.
        if s.at > 1.75 {
            s.at += 0.35;
        }
        s
    }));
    steps.extend(stick(3.3, 1.0, 0.1));
    steps.extend(taps(3.4).into_iter().map(|mut s| {
        // The leap takes longer.
        if s.at > 3.45 {
            s.at += 0.6;
        }
        s
    }));
    steps.extend(stick(5.6, 0.0, 0.0));
    steps.push(DemoStep::home(6.5));
    world.entity_mut(brawler).insert((
        Name::new("Freeflow brawler"),
        crate::combat::Attacker::new("boxing", None),
        crate::freeflow::Freeflow {
            foes: dummies,
            ..Default::default()
        },
        DemoScript {
            period: FREEFLOW_PERIOD,
            steps,
            time: 0.0,
            home: None,
        },
    ));

    // --- Shuttle lane: sprint down it, dig in to stop, run back backwards, turn and run
    // sideways back to the start; a coach beside it turns to keep watching.
    let runner = spawn_humanoid(
        world,
        humanoid::Outfit::Athlete,
        Transform::from_translation(at(LANE.x, 0.0, LANE.z)),
    );
    dress(world, runner, "athlete", SkinMaterial::default());
    let (x, z) = (LANE.x as f32, LANE.z as f32);
    world.entity_mut(runner).insert((
        Name::new("Shuttle runner"),
        ShuttleRun {
            origin,
            start: [x, z],
            legs: vec![
                ShuttleLeg {
                    to: [x, z - LANE_LENGTH],
                    facing_deg: 0.0,
                    top: humanoid::SPRINT_SPEED,
                },
                ShuttleLeg {
                    to: [x, z - LANE_LENGTH * 0.45],
                    facing_deg: 0.0,
                    top: humanoid::BACK_RUN_SPEED,
                },
                ShuttleLeg {
                    to: [x, z],
                    facing_deg: -90.0,
                    top: humanoid::SIDE_RUN_SPEED,
                },
            ],
            ..Default::default()
        },
    ));
    let coach = spawn_humanoid(
        world,
        humanoid::Outfit::Shirt,
        Transform::from_translation(at(LANE.x - 2.6, 0.0, LANE.z - LANE_LENGTH as f64 * 0.55))
            .with_rotation(facing(-90.0)),
    );
    dress(world, coach, "reacher", shirt_detail());
    world.entity_mut(coach).insert((
        Name::new("Coach"),
        DemoScript {
            period: 1000.0,
            steps: vec![DemoStep::param(0.05, "arms_folded", 1.0)],
            time: 0.0,
            home: None,
        },
        Watch {
            target: Some(runner),
            ..Default::default()
        },
        LookAt {
            chain: vec![
                ("chest".into(), 0.2),
                ("neck".into(), 0.35),
                ("head".into(), 0.45),
            ],
            max_angle_deg: 75.0,
            weight: 0.9,
            smoothing: 0.15,
            ..Default::default()
        },
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

    // --- Spider garden: flat-topped boulders to climb over.
    for (k, (x, z, hy, yaw)) in SPIDER_ROCKS.into_iter().enumerate() {
        let t = Transform::from_translation(at(x, hy as f64, z)).with_rotation(facing(yaw));
        let mesh = world
            .get_resource_or_init::<Assets<Mesh>>()
            .get_or_insert_keyed(&rock_key(k), || spider_rock(k));
        let half = rock_half(k);
        world.spawn((
            t,
            GlobalTransform::from(t),
            IkSurface {
                shape: IkShape::Box,
                half_extents: half.to_array(),
                surface: 2,
            },
            MeshRenderer::new(mesh, rock),
        ));
    }
    let spider_e = spawn_spider(world, Transform::from_translation(at(8.0, 0.0, 12.0)));
    world.entity_mut(spider_e).insert(DemoPath {
        effort_speed: 2.5,
        // On the far side of the garden, in the open, it stops and rears up in threat.
        pauses: vec![DemoPause {
            waypoint: 5,
            secs: 5.0,
            param: "threat".into(),
        }],
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
        CAR_PAINT,
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

    // Inside the loop, a parked concept car opens and shuts its doors (when fetched). It stands
    // left of center, behind the car loop's camera.
    spawn_parked_car(
        world,
        Transform::from_translation(at(-4.0, 0.0, -22.0)).with_rotation(facing(-90.0)),
    );

    // --- Dog run: a long loop in front of the muscle stage, walk and trot round the ends,
    // canter out of the turns and a full gallop down the far straight.
    let dog_e = spawn_dog(
        world,
        Transform::from_translation(at(-12.0, 0.0, 12.0)).with_rotation(facing(-90.0)),
        1.0,
    );
    world.entity_mut(dog_e).insert(DemoPath {
        effort_speed: 8.0,
        ..DemoPath::new(origin, DOG_PATH.to_vec())
    });

    // --- Stunt yard: off a deck onto a crash mat, limp, and up again.
    let stuntman = stunt::spawn_yard(world, origin);

    let yard = weapons_yard(world, origin);

    DemoEntities {
        spectators,
        athlete,
        lifter,
        reacher,
        walker,
        drill,
        sparring,
        boxer,
        horse: horse_e,
        spider: spider_e,
        dog: dog_e,
        car: car_e,
        stuntman,
        runner,
        coach,
        swordsman: yard.swordsman,
        shield_partner: yard.shield_partner,
        greatsword: yard.greatsword,
        greatsword_dummy: yard.dummy,
        archer: yard.archer,
    }
}

#[allow(clippy::type_complexity)]
fn demo_path_system(
    time: Res<Time>,
    focus: Res<FocusPoints>,
    range: Res<SimulationRange>,
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
    let frame = time.frame_count();
    for (entity, mut path, mut t, params, local) in &mut q {
        // Beyond the simulation range a walker waits where it is (its body rests too).
        if path.started && range.is_resting(&focus, t.translation) {
            continue;
        }
        let dt = time.delta_for(local);
        path.build();
        if path.samples.len() < 2 {
            continue;
        }
        let speed = path.advance(dt);
        let paused = path.pause_params(dt);
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
        // The slope of the ground over the next and last half meter, for the `grade` posture
        // (stairs average out into their pitch).
        let ground_at = |d: f32| {
            let q = pos + DVec3::new((dir.x * d) as f64, 0.0, (dir.y * d) as f64);
            probe
                .probe(DVec3::new(q.x, pos.y + 0.3, q.z), 0.7, 3.0)
                .map(|h| h.height)
        };
        // The grade is smoothed over a fifth of a second: measuring it every fourth frame
        // (staggered across walkers) is plenty.
        if !path.started || (frame + entity.to_bits()).is_multiple_of(4) {
            path.rise = match (ground_at(0.5), ground_at(-0.5)) {
                (Some(ahead), Some(behind)) if speed > 0.1 => (ahead - behind) as f32,
                _ => 0.0,
            };
        }
        let rise = path.rise;
        if path.started {
            pos.y += (target_y - pos.y) * smoothing(dt, 0.12) as f64;
        } else {
            pos.y = target_y;
        }
        let along = (-dir.x).atan2(-dir.y);
        let yaw = path.facing.unwrap_or(along);
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
        let velocity = dir * speed;
        if dt > 0.0 && path.started {
            path.yaw_rate += (dyaw / dt - path.yaw_rate) * smoothing(dt, 0.25);
            let forward = Vec2::new(-yaw.sin(), -yaw.cos());
            let accel = ((velocity - path.velocity) / dt).dot(forward);
            path.accel += (accel - path.accel) * smoothing(dt, 0.2);
            path.grade += (rise - path.grade) * smoothing(dt, 0.2);
        }
        path.velocity = velocity;
        path.started = true;
        t.translation = pos;
        t.rotation = Quat::from_rotation_y(yaw);
        if let Some(mut params) = params {
            params.set("speed", speed);
            params.set("turn", (path.yaw_rate / 1.2).clamp(-1.0, 1.0));
            params.set("heading", (along - yaw + PI).rem_euclid(2.0 * PI) - PI);
            params.set("accel", crate::body::accel_param(path.accel));
            params.set("grade", crate::body::grade_param(path.grade));
            params.set(
                "move_effort",
                (speed / path.effort_speed.max(0.1)).clamp(0.0, 1.0),
            );
            for (name, value) in paused {
                params.set(&name, value);
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn demo_script_system(
    time: Res<Time>,
    mut damage: MessageWriter<DamageEvent>,
    mut q: Query<(
        Entity,
        &mut DemoScript,
        Option<&mut Animator>,
        Option<&mut AnimationParams>,
        Option<&GlobalTransform>,
        Option<&LocalTimeScale>,
        Option<&Transform>,
        Option<&mut HitReaction>,
    )>,
) {
    for (entity, mut script, mut animator, mut params, gt, local, transform, mut reaction) in &mut q
    {
        let dt = time.delta_for(local);
        if script.period <= 0.0 || dt <= 0.0 {
            continue;
        }
        if script.home.is_none() {
            script.home = transform.map(|t| t.translation);
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
            if step.home
                && let (Some(home), Some(t), Some(r)) = (script.home, transform, reaction.as_mut())
            {
                let left = r.stagger.0.as_dvec3();
                r.knock((home - t.translation - left).as_vec3(), HOME_SECS);
            }
            if let (Some((point, push)), Some(gt)) = (step.hit, gt) {
                damage.write(DamageEvent {
                    target: entity,
                    amount: push.length() * 0.2,
                    kind: DamageKind::Blunt,
                    point: gt.translation() + (gt.rotation() * point).as_dvec3(),
                    impulse: gt.rotation() * push,
                    source: None,
                });
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

    #[test]
    fn a_walker_slows_into_a_stop_waits_there_and_moves_on() {
        let mut path = DemoPath {
            pauses: vec![DemoPause {
                waypoint: 1,
                secs: 3.0,
                param: "threat".into(),
            }],
            ..DemoPath::new(
                DVec3::ZERO,
                vec![
                    [0.0, 0.0, 1.0],
                    [10.0, 0.0, 1.0],
                    [10.0, 10.0, 1.0],
                    [0.0, 10.0, 1.0],
                ],
            )
        };
        path.length();
        let stop = path.waypoint_distance(1);
        let dt = 1.0 / 60.0;
        let mut slowest_before = f32::MAX;
        let mut held = 0.0;
        let mut most = 0.0f32;
        let mut t = 0.0;
        while t < 30.0 {
            let waiting = path.pause.is_some();
            let speed = path.advance(dt);
            let w = path.pause_params(dt)[0].1;
            most = most.max(w);
            if path.pause.is_some() {
                held += dt;
                assert!((path.distance - stop).abs() < 1e-4);
                if waiting {
                    assert_eq!(speed, 0.0);
                }
            } else if path.distance < stop && stop - path.distance < 0.3 {
                slowest_before = slowest_before.min(speed);
            }
            t += dt;
        }
        // It eased down before stopping, waited the full stop once and went on round.
        assert!(slowest_before < 0.6, "{slowest_before}");
        assert!((held - 3.0).abs() < 0.05, "held {held}");
        assert!(most > 0.95, "the stop's parameter only reached {most}");
        assert!(path.distance > stop + 5.0, "{}", path.distance);
        assert!(path.pause_params(dt)[0].1 < 0.05);
    }
}
